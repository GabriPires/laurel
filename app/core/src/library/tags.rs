//! Tags (spec library.tags): names the user puts on clips and screenshots. The
//! standard tags ship with Laurel, in three groups; each is stored by a fixed ID,
//! which never changes, and shown in the interface's language. The custom tags are
//! the ones the user creates, under the rules of [`check_name`]. The library index
//! records the custom tags and which tags each item has.
//!
//! The names of the standard tags and of the groups are interface texts in
//! English, the keys of their translations (app/i18n/<lang>.json);
//! tools/i18n-check.py reads them from this file.

use std::cmp::Ordering;

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// A group of the tag picker.
#[derive(Debug)]
pub struct Group {
    pub id: &'static str,
    pub name: &'static str,
}

/// The picker's groups of standard tags, in their order.
pub const GROUPS: [Group; 3] = [
    Group { id: "plays", name: "Plays" },
    Group { id: "moments", name: "Moments" },
    Group { id: "other", name: "Other" },
];

/// The picker's group of custom tags, after [`GROUPS`].
pub const CUSTOM_GROUP: Group = Group { id: "custom", name: "Your tags" };

/// A tag that ships with Laurel.
#[derive(Debug)]
pub struct StandardTag {
    /// What the index stores.
    pub id: &'static str,
    /// The ID of its group in [`GROUPS`].
    pub group: &'static str,
    pub name: &'static str,
}

/// The standard tags, in the order the picker lists them and an item's tags show.
pub const STANDARD: [StandardTag; 12] = [
    StandardTag { id: "mvp", group: "plays", name: "MVP" },
    StandardTag { id: "clutch", group: "plays", name: "Clutch" },
    StandardTag { id: "kill", group: "plays", name: "Kill" },
    StandardTag { id: "multi-kill", group: "plays", name: "Multi-kill" },
    StandardTag { id: "victory", group: "plays", name: "Victory" },
    StandardTag { id: "lucky", group: "plays", name: "Lucky" },
    StandardTag { id: "funny", group: "moments", name: "Funny" },
    StandardTag { id: "fail", group: "moments", name: "Fail" },
    StandardTag { id: "jump-scare", group: "moments", name: "Jump scare" },
    StandardTag { id: "glitch", group: "moments", name: "Glitch" },
    StandardTag { id: "scenery", group: "other", name: "Scenery" },
    StandardTag { id: "montage", group: "other", name: "Montage" },
];

/// The standard tag with this ID.
pub fn standard(id: &str) -> Option<&'static StandardTag> {
    STANDARD.iter().find(|tag| tag.id == id)
}

/// How many of the items selected in the library have a tag, as the selection bar's
/// picker shows it: all of them (✓), some (–) or none (empty).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagState {
    OnAll,
    OnSome,
    OnNone,
}

impl TagState {
    /// The state of a tag that `having` of the `selected` items have.
    pub fn of(having: usize, selected: usize) -> TagState {
        match having {
            0 => TagState::OnNone,
            n if n == selected => TagState::OnAll,
            _ => TagState::OnSome,
        }
    }

    /// What a click on the tag in the selection bar's picker does: on – or empty it
    /// puts the tag on every selected item (true), on ✓ it takes it off every one
    /// (false).
    pub fn click_puts_on(self) -> bool {
        self != TagState::OnAll
    }

    /// The state as the interface reads it: "all", "some" or "none".
    pub fn as_str(self) -> &'static str {
        match self {
            TagState::OnAll => "all",
            TagState::OnSome => "some",
            TagState::OnNone => "none",
        }
    }
}

/// A tag the user created (a custom tag), as the library index keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomTag {
    /// What the index stores on its items: "custom-" and a number the index never
    /// gives again, so that renaming the tag changes only its name.
    pub id: String,
    pub name: String,
}

/// Why a custom tag was not created, renamed or deleted.
#[derive(Debug, PartialEq, Eq)]
pub enum TagError {
    /// The name breaks a rule of [`check_name`].
    Name(NameError),
    /// No custom tag has this ID: a standard tag's cannot change.
    Unknown(String),
    /// The library index could not be read or written.
    Index(String),
}

impl From<NameError> for TagError {
    fn from(e: NameError) -> TagError {
        TagError::Name(e)
    }
}

/// Why a name cannot be a custom tag's.
#[derive(Debug, PartialEq, Eq)]
pub enum NameError {
    /// Nothing is left once the spaces at its ends go.
    Empty,
    /// It has more than [`MAX_NAME`] characters.
    TooLong,
    /// It is this other tag's name, ignoring case, accents and spaces.
    Taken(String),
}

/// The most characters a custom tag's name has (spec library.tags), counted as
/// code points, Rust's `char`: "👍🏽" counts two, by the owner's choice of 2026-11-21.
/// The picker does not count them: it offers "Create" for a longer name and shows
/// this refusal.
pub const MAX_NAME: usize = 24;

/// The name a custom tag gets from what the user typed: without the spaces at its
/// ends, of 1 to [`MAX_NAME`] characters, and none of the `taken` names (the other
/// tags') once both are folded, lowercase and without accents or spaces.
pub fn check_name<'a>(name: &str, taken: impl IntoIterator<Item = &'a str>) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > MAX_NAME {
        return Err(NameError::TooLong);
    }
    let folded = fold(name);
    if let Some(other) = taken.into_iter().find(|other| fold(other) == folded) {
        return Err(NameError::Taken(other.to_string()));
    }
    Ok(name.to_string())
}

/// A tag's name as two names are compared: lowercase, without accents and without
/// spaces (Unicode's White_Space, as `trim` in [`check_name`]). The picker folds
/// them with the same steps, in the same order (app/ui/library.js, foldName), so
/// that it offers "Create" only for a name this does not refuse as taken.
pub(crate) fn fold(name: &str) -> String {
    name.to_lowercase().nfd().filter(|c| !is_combining_mark(*c) && !c.is_whitespace()).collect()
}

/// Puts an item's tags, each its ID and, for a custom tag, its name, in the order
/// its chips show: the standard tags in the order of [`STANDARD`], then the custom
/// tags by name (see [`by_name`]), then any other ID by its text.
pub(crate) fn sort_chips(tags: &mut [(String, Option<String>)]) {
    let rank = |(id, name): &(String, Option<String>)| STANDARD.iter().position(|tag| tag.id == id).unwrap_or(STANDARD.len() + usize::from(name.is_none()));
    tags.sort_by(|a, b| {
        let names = match (&a.1, &b.1) {
            (Some(x), Some(y)) => by_name(x, y),
            _ => Ordering::Equal,
        };
        rank(a).cmp(&rank(b)).then(names).then_with(|| a.0.cmp(&b.0))
    });
}

/// The order of the custom tags, in the picker and on an item: by name, ignoring
/// case, accents and spaces.
pub(crate) fn by_name(a: &str, b: &str) -> Ordering {
    fold(a).cmp(&fold(b)).then_with(|| a.cmp(b))
}
