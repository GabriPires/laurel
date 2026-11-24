//! The library's shell: laurel_app_core::library::Library (the clips and
//! screenshots in the capture folder, Recently deleted, trims, thumbnails, tags and
//! the index) with the runtime's FFmpeg, and Windows behind its Desktop trait
//! (laurel_app_win::WinDesktop) with app.log and the interface's language.

use std::path::{Path, PathBuf};

use laurel_app_core::library::tags::{self, CustomTag, NameError, TagError};
use laurel_app_core::library::Text;
pub use laurel_app_core::library::{game_name, Library};
use laurel_app_core::media::Media;
use laurel_app_win::WinDesktop;
use serde_json::{json, Value};

use crate::i18n::{t, tr};

/// The library's texts in the interface's language.
fn text(text: Text) -> String {
    match text {
        Text::BeingTrimmed => t!("It is being trimmed. Try again when the trimmed copy is saved."),
        Text::EndBeforeStart => t!("The end must come after the start"),
        Text::OriginalInRecentlyDeleted => t!("the original is in Recently deleted"),
    }
}

/// The library of the capture folder's clips and screenshots, its index and
/// thumbnails in `work_dir`, with FFmpeg's program when found.
pub fn open(work_dir: &Path, clips_dir: PathBuf, shots_dir: PathBuf, ffmpeg: Option<PathBuf>) -> Result<Library, String> {
    let media = ffmpeg.map(|program| Box::new(crate::media::ffmpeg(program)) as Box<dyn Media>);
    Library::open(work_dir, clips_dir, shots_dir, media, Box::new(WinDesktop::new(crate::log::line, text)))
}

/// The picker's groups and every tag (spec library.tags), in their order: the
/// standard tags with their names in the interface's language and the English
/// ones, which the search also finds, then the custom tags by name, which have no
/// English name.
pub fn tag_catalog(library: &Library) -> Result<Value, String> {
    let groups: Vec<Value> = tags::GROUPS.iter().chain([&tags::CUSTOM_GROUP]).map(|group| json!({ "id": group.id, "name": tr(group.name) })).collect();
    let standard = tags::STANDARD.iter().map(|tag| json!({ "id": tag.id, "group": tag.group, "name": tr(tag.name), "english": tag.name }));
    let custom = library.custom_tags()?.into_iter().map(|tag| json!({ "id": tag.id, "group": tags::CUSTOM_GROUP.id, "name": tag.name }));
    Ok(json!({ "groups": groups, "tags": standard.chain(custom).collect::<Vec<Value>>() }))
}

/// Each tag, by ID, with how many of the items at `paths` have it, as the
/// selection bar's picker shows it (spec library.tags): `{"kill": "some", "funny":
/// "all", "mvp": "none"}`.
pub fn tag_states(library: &Library, paths: &[String]) -> Result<Value, String> {
    Ok(library.tag_states(paths)?.into_iter().map(|(tag, state)| (tag, json!(state.as_str()))).collect())
}

/// The standard tags' names in the interface's language, which a custom tag's
/// name cannot repeat either.
fn local_names() -> Vec<String> {
    tags::STANDARD.iter().map(|tag| tr(tag.name)).collect()
}

/// Creates a custom tag with the name the user typed.
pub fn create_tag(library: &Library, name: &str) -> Result<CustomTag, String> {
    let local = local_names();
    library.create_tag(name, &local.iter().map(String::as_str).collect::<Vec<_>>()).map_err(tag_error)
}

/// Renames a custom tag (spec library.tags) with the name the user typed.
pub fn rename_tag(library: &Library, id: &str, name: &str) -> Result<CustomTag, String> {
    let local = local_names();
    library.rename_tag(id, name, &local.iter().map(String::as_str).collect::<Vec<_>>()).map_err(tag_error)
}

/// Deletes a custom tag (spec library.tags) and takes it off every item.
pub fn delete_tag(library: &Library, id: &str) -> Result<(), String> {
    library.delete_tag(id).map_err(tag_error)
}

/// Why a custom tag was not created, renamed or deleted, in the interface's language.
fn tag_error(e: TagError) -> String {
    match e {
        TagError::Name(e) => name_error(e),
        TagError::Unknown(_) => t!("This tag no longer exists"),
        TagError::Index(e) => e,
    }
}

/// Why a name cannot be a custom tag's, in the interface's language.
pub fn name_error(e: NameError) -> String {
    match e {
        NameError::Empty => t!("Type a name for the tag"),
        NameError::TooLong => t!("A tag's name can have up to {max} characters", max = tags::MAX_NAME),
        NameError::Taken(other) => t!("There is already a tag named “{name}”", name = other),
    }
}
