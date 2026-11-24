//! Clip and screenshot library (specs library.browse, library.thumbnails,
//! edit.trim, library.trash, library.tags): the files in the capture folders,
//! reconciled with the library index (a SQLite file, [`Index`]), with thumbnails
//! for every item and the [`tags`] the user puts on them.
//! Deleted items wait in Recently deleted, a hidden folder in the capture folder,
//! until they are restored or their days run out. FFmpeg comes in through
//! [`Media`], and what only Windows and the app do (the Recycle Bin, a hidden
//! folder, the log, the interface's language) through [`Desktop`].

mod index;
pub mod tags;

pub use index::{game_from_name, FileFacts, Index};

use tags::{CustomTag, TagError, TagState};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::media::Media;
use crate::{mp4, storage};

/// What the library needs from Windows and from the app around it.
pub trait Desktop: Send + Sync {
    /// Marks a folder hidden.
    fn hide(&self, folder: &Path);
    /// Removes a file for good: to the Windows Recycle Bin when `recycle`, else
    /// deleted.
    fn remove(&self, file: &Path, recycle: bool) -> Result<(), String>;
    /// A line in the app's log.
    fn log(&self, line: &str);
    /// A text the interface shows, in the user's language.
    fn text(&self, text: Text) -> String;
}

/// The library's texts that the interface shows; the app translates them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    /// "It is being trimmed. Try again when the trimmed copy is saved." (A3)
    BeingTrimmed,
    /// "The end must come after the start"
    EndBeforeStart,
    /// "the original is in Recently deleted"
    OriginalInRecentlyDeleted,
}

/// A path with the system's separators: the engine reports clips with forward
/// slashes, and the interface may give Windows paths with them too.
pub fn native_path(path: &str) -> String {
    if MAIN_SEPARATOR == '\\' { path.replace('/', "\\") } else { path.to_string() }
}

/// Recently deleted, next to Clips and Screenshots: moving a file there is a rename.
const TRASH: &str = ".deleted";

pub struct Library {
    index: Mutex<Index>,
    /// Clips and screenshots folders; they change with the capture folder.
    dirs: Mutex<(PathBuf, PathBuf)>,
    thumbs_dir: PathBuf,
    media: Option<Box<dyn Media>>,
    desktop: Box<dyn Desktop>,
    /// Clips ffmpeg is reading for a trim or its timeline (lowercase paths): they
    /// cannot be moved to Recently deleted meanwhile (stability tests, finding A3).
    reading: Mutex<Vec<String>>,
}

/// Marks a clip as being read until dropped (see Library::reading).
struct Reading<'a> {
    library: &'a Library,
    key: String,
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        let mut reading = self.library.reading.lock().unwrap();
        if let Some(i) = reading.iter().position(|k| *k == self.key) {
            reading.remove(i);
        }
    }
}

/// A readable game name: the name the app found for the game, else the hooked
/// window's title with invisible characters (some games pad their titles with them)
/// removed and spaces collapsed, else the executable name.
pub fn game_name(game: &Value) -> Option<String> {
    // The name from the store library or the user's list, when the app found the game.
    if let Some(name) = game["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
        return Some(name.to_string());
    }
    let invisible = |c: char| c.is_control() || matches!(c, '\u{00AD}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
    let title: String = game["title"].as_str().unwrap_or_default().chars().filter(|c| !invisible(*c)).collect();
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if !title.is_empty() {
        return Some(title);
    }
    let exe = game["exe"].as_str().unwrap_or_default();
    (!exe.is_empty()).then(|| exe.trim_end_matches(".exe").to_string())
}

fn modified_ms(path: &Path) -> i64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

fn file_size(path: &Path) -> i64 {
    std::fs::metadata(path).map_or(0, |m| m.len() as i64)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `dir/<file name>`, with " (2)", " (3)"... when that name is taken.
fn free_name(dir: &Path, file: &Path) -> Result<PathBuf, String> {
    let stem = file.file_stem().ok_or("bad file path")?.to_string_lossy();
    let ext = file.extension().map(|e| e.to_string_lossy()).unwrap_or_default();
    Ok(storage::unique(dir, &stem, &ext))
}

impl Library {
    /// The library of the capture folder's clips and screenshots, with its index and
    /// thumbnails in `work_dir`; without FFmpeg, no thumbnails are made and nothing
    /// can be trimmed.
    pub fn open(work_dir: &Path, clips_dir: PathBuf, shots_dir: PathBuf, media: Option<Box<dyn Media>>, desktop: Box<dyn Desktop>) -> Result<Library, String> {
        let index = Index::open(&work_dir.join("library.sqlite"))?;
        let thumbs_dir = work_dir.join("thumbnails");
        std::fs::create_dir_all(&thumbs_dir).map_err(|e| e.to_string())?;
        Ok(Library { index: Mutex::new(index), dirs: Mutex::new((clips_dir, shots_dir)), thumbs_dir, media, desktop, reading: Mutex::new(vec![]) })
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap()
    }

    /// Clips, screenshots and thumbnails folders.
    pub fn dirs(&self) -> [PathBuf; 3] {
        let (clips, shots) = self.dirs.lock().unwrap().clone();
        [clips, shots, self.thumbs_dir.clone()]
    }

    pub fn thumbs_dir(&self) -> &Path {
        &self.thumbs_dir
    }

    /// The Recently deleted folder, in the capture folder.
    pub fn trash_dir(&self) -> PathBuf {
        let clips = self.dirs.lock().unwrap().0.clone();
        clips.parent().map_or_else(|| clips.join(TRASH), |folder| folder.join(TRASH))
    }

    /// Creates the Recently deleted folder if needed, hidden.
    pub fn ensure_trash(&self) -> Result<PathBuf, String> {
        let trash = self.trash_dir();
        std::fs::create_dir_all(&trash).map_err(|e| format!("{}: {e}", trash.display()))?;
        self.desktop.hide(&trash);
        Ok(trash)
    }

    /// Points the library at a new capture folder; call `scan` afterwards.
    pub fn set_dirs(&self, clips_dir: PathBuf, shots_dir: PathBuf) {
        *self.dirs.lock().unwrap() = (clips_dir, shots_dir);
    }

    /// Total size of the indexed files, which all live in the capture folder.
    pub fn total_size(&self) -> u64 {
        self.index().total_size()
    }

    /// The next item the size limit removes (spec storage.limits): the one deleted
    /// first, else the oldest item not marked to keep.
    pub fn oldest_removable(&self) -> Option<String> {
        self.index().oldest_removable()
    }

    pub fn remove(&self, path: &str) {
        let _ = self.index().remove(path);
    }

    /// Removes a file's cached timeline frames, named "<stem>-<size>-<modified>".
    fn remove_timeline(&self, file: &Path) {
        let stem = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        for entry in std::fs::read_dir(self.thumbs_dir.join("timeline")).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&stem).and_then(|r| r.strip_prefix('-'));
            if rest.is_some_and(|r| r.split('-').count() == 2 && r.split('-').all(|n| n.parse::<i64>().is_ok())) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    /// Deletes an item at the user's request (specs library.browse, library.trash):
    /// the file moves to Recently deleted, where `restore` brings it back and
    /// `expire` removes it for good after the days chosen; the thumbnail stays, the
    /// timeline frames go. Returns the item at its new place, or null when its file
    /// was already gone (the entry is then dropped).
    pub fn delete(&self, path: &str) -> Result<Value, String> {
        let path = native_path(path);
        self.get(&path)?.filter(|i| i["deletedMs"].is_null()).ok_or("item not found")?;
        if self.reading.lock().unwrap().contains(&path.to_lowercase()) {
            return Err(self.desktop.text(Text::BeingTrimmed));
        }
        let file = Path::new(&path);
        if !file.exists() {
            self.purge(&path, false)?;
            return Ok(Value::Null);
        }
        let target = free_name(&self.ensure_trash()?, file)?;
        storage::rename_when_free(file, &target, &|line| self.desktop.log(line)).map_err(|e| format!("{}: {e}", file.display()))?;
        let item = self.index().mark_deleted(&path, &target.to_string_lossy(), now_ms())?;
        self.remove_timeline(file);
        Ok(item)
    }

    /// Brings a deleted item back where it was, under a free name if that one was
    /// taken since.
    pub fn restore(&self, path: &str) -> Result<Value, String> {
        let path = native_path(path);
        let item = self.get(&path)?.ok_or("item not found")?;
        let original = PathBuf::from(item["originalPath"].as_str().ok_or("the item is not in Recently deleted")?);
        let dir = original.parent().ok_or("bad file path")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let target = if original.exists() { free_name(dir, &original)? } else { original };
        std::fs::rename(&path, &target).map_err(|e| format!("{path}: {e}"))?;
        self.index().mark_restored(&path, &target.to_string_lossy())
    }

    /// Removes an item for good, with its thumbnail and timeline frames: the file
    /// goes to the Windows Recycle Bin when `recycle`, else it is deleted.
    pub fn purge(&self, path: &str, recycle: bool) -> Result<(), String> {
        let path = native_path(path);
        let item = self.get(&path)?;
        let file = Path::new(&path);
        if file.exists() {
            self.desktop.remove(file, recycle)?;
        }
        self.remove(&path);
        if let Some(thumb) = item.as_ref().and_then(|i| i["thumbnail"].as_str()) {
            let _ = std::fs::remove_file(thumb);
        }
        self.remove_timeline(file);
        Ok(())
    }

    /// Removes for good the items deleted `days` or more days ago; returns their paths.
    pub fn expire(&self, days: u32, recycle: bool) -> Vec<String> {
        let paths = self.index().expired(days, now_ms());
        paths
            .into_iter()
            .filter(|path| match self.purge(path, recycle) {
                Ok(()) => true,
                Err(e) => {
                    self.desktop.log(&format!("recently deleted: {e}"));
                    false
                }
            })
            .collect()
    }

    /// How many items wait in Recently deleted, and their size in bytes.
    pub fn deleted_size(&self) -> (u64, u64) {
        self.index().deleted_size()
    }

    /// Moves an entry to a new path, and its original place too when given (a
    /// deleted item when the capture folder moves).
    pub fn rename(&self, from: &str, to: &str, original: Option<&str>) {
        let _ = self.index().rename(from, to, original);
    }

    pub fn set_kept(&self, path: &str, kept: bool) -> Result<Value, String> {
        let path = native_path(path);
        self.index().set_kept(&path, kept)
    }

    /// Puts a tag on an item, or takes it off (spec library.tags), and returns the
    /// item.
    pub fn set_tag(&self, path: &str, tag: &str, on: bool) -> Result<Value, String> {
        self.index().set_tag(&native_path(path), tag, on)
    }

    /// The custom tags (spec library.tags), by name.
    pub fn custom_tags(&self) -> Result<Vec<CustomTag>, String> {
        self.index().custom_tags()
    }

    /// Creates a custom tag (spec library.tags) with the name the user typed;
    /// `local_names` are the standard tags' names in the interface's language.
    pub fn create_tag(&self, name: &str, local_names: &[&str]) -> Result<CustomTag, TagError> {
        self.index().create_tag(name, local_names)
    }

    /// Renames a custom tag (spec library.tags), as [`Index::rename_tag`] does it.
    pub fn rename_tag(&self, id: &str, name: &str, local_names: &[&str]) -> Result<CustomTag, TagError> {
        self.index().rename_tag(id, name, local_names)
    }

    /// Deletes a custom tag and takes it off every item (spec library.tags), as
    /// [`Index::delete_tag`] does it.
    pub fn delete_tag(&self, id: &str) -> Result<(), TagError> {
        self.index().delete_tag(id)
    }

    fn thumbnail_for(&self, path: &Path, kind: &str, duration_ms: i64) -> Option<String> {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        // The engine writes "<clip stem>.png" when it saves a clip.
        let thumb = self.thumbs_dir.join(if kind == "clip" { format!("{stem}.png") } else { format!("{stem}.screenshot.png") });
        if !thumb.exists() {
            let ffmpeg = self.media.as_ref()?;
            let at = if kind == "clip" { duration_ms as f64 / 2000.0 } else { 0.0 };
            if let Err(e) = ffmpeg.thumbnail(path, &thumb, at) {
                self.desktop.log(&format!("thumbnail for {} failed: {e}", path.display()));
                return None;
            }
        }
        Some(thumb.to_string_lossy().into_owned())
    }

    /// Indexes one file (or refreshes its entry) and returns the item.
    pub fn add(&self, path: &str, game: Option<String>) -> Result<Value, String> {
        let file = self.measure(path);
        self.index().add(&file, game.as_deref())
    }

    /// What the index records of a file: its kind, date, size, video or image
    /// size, and its thumbnail, made when missing.
    fn measure(&self, path: &str) -> FileFacts {
        let path = PathBuf::from(native_path(path));
        let kind = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) { "clip" } else { "screenshot" };
        let (duration_ms, width, height, fps, audio_tracks) = if kind == "clip" {
            let info = mp4::probe(&path).unwrap_or_default();
            (info.duration_ms as i64, info.width, info.height, info.fps, info.audio_tracks)
        } else {
            let (w, h) = mp4::png_size(&path).unwrap_or((0, 0));
            (0, w, h, 0.0, 0)
        };
        let thumbnail = self.thumbnail_for(&path, kind, duration_ms);
        FileFacts {
            path: path.to_string_lossy().into_owned(),
            kind,
            created_ms: modified_ms(&path),
            duration_ms,
            width,
            height,
            fps,
            audio_tracks,
            size: file_size(&path),
            thumbnail,
        }
    }

    /// Marks an item as an automatic clip of an achievement and returns it.
    pub fn set_achievement(&self, path: &str, achievement: &Value) -> Result<Value, String> {
        self.index().set_achievement(&native_path(path), achievement)
    }

    fn get(&self, path: &str) -> Result<Option<Value>, String> {
        self.index().get(path)
    }

    /// Reconciles the index with the folders: new files are added, removed files
    /// leave the index, and missing thumbnails are regenerated. A file found in
    /// Recently deleted without an entry (a rebuilt index) counts as deleted now.
    pub fn scan(&self) -> Result<(usize, usize), String> {
        let (clips_dir, shots_dir) = self.dirs.lock().unwrap().clone();
        let media_in = |dir: &Path, exts: &[&str]| -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|p| p.extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x))))
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        let mut on_disk = media_in(&clips_dir, &["mp4"]);
        on_disk.extend(media_in(&shots_dir, &["png", "jpg"]));
        let in_trash = media_in(&self.trash_dir(), &["mp4", "png", "jpg"]);
        // Entries whose file is gone, or that are not in the capture folder any more.
        let found: Vec<String> = on_disk.iter().chain(&in_trash).cloned().collect();
        let removed = self.index().remove_missing(&found)?;
        let indexed = self.index().entries()?;
        for path in in_trash.iter().filter(|p| !indexed.iter().any(|(i, _)| i == *p)) {
            self.add(path, None)?;
            let file = Path::new(path);
            let home = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) { &clips_dir } else { &shots_dir };
            let original = home.join(file.file_name().unwrap_or_default()).to_string_lossy().into_owned();
            self.index().mark_found_deleted(path, &original, now_ms())?;
        }
        // Items indexed without a game get the one in their file name.
        self.index().fill_games_from_file_names()?;
        let mut added = 0;
        for path in &on_disk {
            match indexed.iter().find(|(p, _)| p == path) {
                None => {
                    self.add(path, None)?;
                    added += 1;
                }
                Some((_, thumb)) if !thumb.as_deref().is_some_and(|t| Path::new(t).exists()) => {
                    self.add(path, None)?;
                }
                _ => {}
            }
        }
        Ok((added, removed))
    }

    /// Items newest first, optionally of one kind and one game; deleted ones apart.
    pub fn list(&self, kind: Option<&str>, game: Option<&str>) -> Result<Vec<Value>, String> {
        self.index().list(kind, game)
    }

    /// The items in Recently deleted, the last deleted first.
    pub fn list_deleted(&self) -> Result<Vec<Value>, String> {
        self.index().list_deleted()
    }

    /// Each tag in use with how many items have it, apart those in Recently deleted
    /// (spec library.tags), as [`Index::tag_counts`] gives them.
    pub fn tag_counts(&self) -> Result<Value, String> {
        self.index().tag_counts()
    }

    /// Each tag with how many of the items at `paths` have it, all, some or none
    /// (spec library.tags), as [`Index::tag_states`] gives them.
    pub fn tag_states(&self, paths: &[String]) -> Result<BTreeMap<String, TagState>, String> {
        let paths: Vec<String> = paths.iter().map(|path| native_path(path)).collect();
        self.index().tag_states(&paths.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// A click on a tag in the selection bar's picker over the items at `paths`
    /// (spec library.tags), as [`Index::click_tag`] does it: each item, or why it
    /// did not change.
    pub fn click_tag(&self, paths: &[String], tag: &str) -> Result<Vec<Result<Value, String>>, String> {
        let paths: Vec<String> = paths.iter().map(|path| native_path(path)).collect();
        self.index().click_tag(&paths.iter().map(String::as_str).collect::<Vec<_>>(), tag)
    }

    /// Games that have items of a kind, for the filter.
    pub fn games(&self, kind: Option<&str>) -> Result<Vec<String>, String> {
        self.index().games(kind)
    }

    /// Saves `start..end` seconds of a clip as a new clip next to it, keeping the
    /// original (spec edit.trim), and indexes it with the same game and tags: fast
    /// (stream copy from the keyframe at or before `start`) or exact (re-encoded).
    pub fn trim(&self, path: &str, start: f64, end: f64, exact: bool) -> Result<Value, String> {
        let ffmpeg = self.media.as_ref().ok_or("ffmpeg.exe was not found")?;
        if end <= start {
            return Err(self.desktop.text(Text::EndBeforeStart));
        }
        let source = PathBuf::from(native_path(path));
        let stem = source.file_stem().ok_or("bad clip path")?.to_string_lossy().into_owned();
        let dir = source.parent().ok_or("bad clip path")?;
        let out = trim_path(dir, &stem);
        let _reading = self.reading(&source);
        if exact {
            let bitrate = mp4::probe(&source).map(|info| info.video_bitrate).ok();
            ffmpeg.trim_exact(&source, &out, start, end, bitrate)?;
        } else {
            // Stream copy seeks to the keyframe at or before the time asked, so
            // half a millisecond after `start` (a keyframe's time) keeps that
            // keyframe; it stops at the packets whose decode time reaches `to`, and
            // decode times run behind by the B-frame delay, so it is taken off the
            // end: the copy then holds the frames before `end` (checked frame by
            // frame on NVENC clips with two B-frames).
            let delay = mp4::video_frames(&source).map(|f| f.delay).unwrap_or(0.0);
            ffmpeg.trim(&source, &out, start + 0.0005, end - delay - 0.0005)?;
        }
        let copy = self.measure(&out.to_string_lossy());
        // The index is let go before the clip stops being read, as before.
        let item = self.index().add_trim(&source.to_string_lossy(), &copy)?;
        Ok(item)
    }

    /// Puts a trimmed copy in its original's place ("Save and delete original",
    /// spec edit.trim; friends' request and owner's decision of 2026-11-07): the
    /// original goes to Recently deleted, where it can be restored, and the copy
    /// takes its name, its date, its Keep mark, its achievement and its tags. The
    /// copy's thumbnail gets a name of its own, so a later trim of the clip does
    /// not find it. Returns `{ item, deleted }`: the copy in its new place and the
    /// original in Recently deleted (null when its file was already gone).
    pub fn replace_with_trim(&self, original: &str, copy: &str) -> Result<Value, String> {
        let (original, copy) = (native_path(original), native_path(copy));
        let item = self.get(&original)?.filter(|i| i["deletedMs"].is_null()).ok_or("item not found")?;
        let copied = self.get(&copy)?.filter(|i| i["deletedMs"].is_null()).ok_or("trimmed copy not found")?;
        if original.eq_ignore_ascii_case(&copy) || Path::new(&original).parent() != Path::new(&copy).parent() {
            return Err("the trimmed copy is not next to its original".into());
        }
        let modified = std::fs::metadata(&original).and_then(|m| m.modified()).ok();
        let deleted = self.delete(&original)?;
        storage::rename_when_free(Path::new(&copy), Path::new(&original), &|line| self.desktop.log(line))
            .map_err(|e| format!("{copy}: {e} ({})", self.desktop.text(Text::OriginalInRecentlyDeleted)))?;
        // The file's date too, so a rebuilt index keeps the clip in its place.
        if let Some(time) = modified {
            let _ = std::fs::File::options().write(true).open(&original).and_then(|f| f.set_modified(time));
        }
        self.remove_timeline(Path::new(&copy));
        let thumbnail = copied["thumbnail"].as_str().filter(|t| Path::new(t).exists()).and_then(|old| {
            let stem = Path::new(&original).file_stem()?.to_string_lossy().into_owned();
            let new = self.thumbs_dir.join(format!("{stem}.trim-{}.png", now_ms()));
            std::fs::rename(old, &new).ok().map(|_| new.to_string_lossy().into_owned())
        });
        let fresh = self.index().take_place(&copy, &item, thumbnail.as_deref(), now_ms())?;
        Ok(json!({ "item": fresh, "deleted": deleted }))
    }
}

impl Library {
    /// Whether ffmpeg is reading this clip for a trim or its timeline.
    pub fn is_reading(&self, path: &str) -> bool {
        self.reading.lock().unwrap().contains(&native_path(path).to_lowercase())
    }

    /// Marks `source` as being read by ffmpeg until the result is dropped.
    fn reading(&self, source: &Path) -> Reading<'_> {
        let key = source.to_string_lossy().to_lowercase();
        self.reading.lock().unwrap().push(key.clone());
        Reading { library: self, key }
    }

    /// Frame and keyframe times of a clip (spec edit.trim), from the file itself.
    pub fn video_frames(&self, path: &str) -> Result<mp4::VideoFrames, String> {
        mp4::video_frames(Path::new(&native_path(path)))
    }

    /// `count` small frames spread over a clip for the trim timeline, cached in the
    /// thumbnails folder by file name, size and modification time.
    pub fn timeline(&self, path: &str, count: usize) -> Result<Vec<PathBuf>, String> {
        let ffmpeg = self.media.as_ref().ok_or("ffmpeg.exe was not found")?;
        let source = PathBuf::from(native_path(path));
        let duration = mp4::probe(&source)?.duration_ms as f64 / 1000.0;
        let key = format!("{}-{}-{}", source.file_stem().unwrap_or_default().to_string_lossy(), file_size(&source), modified_ms(&source));
        let dir = self.thumbs_dir.join("timeline").join(key);
        let frames: Vec<(f64, PathBuf)> = (0..count).map(|i| ((i as f64 + 0.5) * duration / count as f64, dir.join(format!("{i:02}.jpg")))).collect();
        if !frames.iter().all(|(_, p)| p.exists()) {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let _reading = self.reading(&source);
            ffmpeg.frames(&source, &frames, 192)?;
        }
        Ok(frames.into_iter().map(|(_, p)| p).collect())
    }
}

/// `<stem> (trim).mp4`, or `(trim 2)` and up when taken; a trim of a trim keeps
/// one "(trim N)" instead of piling them up.
fn trim_path(dir: &Path, stem: &str) -> PathBuf {
    let mut base = stem.to_string();
    if let Some(open) = base.rfind(" (trim") {
        let tail = &base[open + 6..];
        if tail == ")" || tail.strip_prefix(' ').and_then(|t| t.strip_suffix(')')).is_some_and(|n| n.parse::<u32>().is_ok()) {
            base.truncate(open);
        }
    }
    let mut out = dir.join(format!("{base} (trim).mp4"));
    let mut n = 2;
    while out.exists() {
        out = dir.join(format!("{base} (trim {n}).mp4"));
        n += 1;
    }
    out
}
