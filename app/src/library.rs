//! Clip and screenshot library (specs library.browse, library.thumbnails,
//! edit.trim, library.trash): a SQLite index of the files in the capture folders,
//! reconciled with the disk, with thumbnails for every item. Deleted items wait in
//! Recently deleted, a hidden folder in the capture folder, until they are restored
//! or their days run out.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::i18n::t;
use crate::{media, mp4, share, storage, win};

/// Recently deleted, next to Clips and Screenshots: moving a file there is a rename.
const TRASH: &str = ".deleted";
const DAY_MS: i64 = 86_400_000;

pub struct Library {
    db: Mutex<Connection>,
    /// Clips and screenshots folders; they change with the capture folder.
    dirs: Mutex<(PathBuf, PathBuf)>,
    thumbs_dir: PathBuf,
    ffmpeg: Option<PathBuf>,
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

/// The game in a file name made by the default pattern, "{game} {date} {time}"
/// with maybe " (2)" or " (trim)" after it, for files indexed without a game (found
/// on disk, or a rebuilt index). "Laurel" is the engine's name for no game.
pub fn game_from_name(stem: &str) -> Option<String> {
    let stamp = |s: &str| {
        let b = s.as_bytes();
        b.len() == 20 && b[0] == b' ' && b[11] == b' ' && [5, 8, 14, 17].iter().all(|&i| b[i] == b'-')
            && b.iter().enumerate().all(|(i, c)| [0, 5, 8, 11, 14, 17].contains(&i) || c.is_ascii_digit())
    };
    let mut base = stem.trim_end();
    loop {
        let cut = base.len().checked_sub(20)?;
        if base.get(cut..).is_some_and(stamp) {
            let game = base[..cut].trim();
            return (!game.is_empty() && game != "Laurel").then(|| game.to_string());
        }
        // Drop one " (...)" from the end and try again.
        let open = base.strip_suffix(')').and_then(|b| b.rfind(" ("))?;
        base = &base[..open];
    }
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
    pub fn open(work_dir: &Path, clips_dir: PathBuf, shots_dir: PathBuf, ffmpeg: Option<PathBuf>) -> Result<Library, String> {
        let db = Connection::open(work_dir.join("library.sqlite")).map_err(|e| e.to_string())?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS items (
                path TEXT PRIMARY KEY,
                kind TEXT NOT NULL,          -- clip | screenshot
                game TEXT,
                created_ms INTEGER NOT NULL,
                duration_ms INTEGER,
                width INTEGER,
                height INTEGER,
                fps REAL,
                audio_tracks INTEGER,
                size INTEGER NOT NULL,
                thumbnail TEXT,
                kept INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| e.to_string())?;
        // Recently deleted (library.trash), added after the first releases: when an
        // item was deleted, and where it came from.
        let columns: Vec<String> = db
            .prepare("SELECT name FROM pragma_table_info('items')")
            .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect())
            .map_err(|e| e.to_string())?;
        for (column, kind) in [("deleted_ms", "INTEGER"), ("original_path", "TEXT")] {
            if !columns.iter().any(|c| c == column) {
                db.execute_batch(&format!("ALTER TABLE items ADD COLUMN {column} {kind};")).map_err(|e| e.to_string())?;
            }
        }
        let thumbs_dir = work_dir.join("thumbnails");
        std::fs::create_dir_all(&thumbs_dir).map_err(|e| e.to_string())?;
        Ok(Library { db: Mutex::new(db), dirs: Mutex::new((clips_dir, shots_dir)), thumbs_dir, ffmpeg })
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
        win::set_hidden(&trash);
        Ok(trash)
    }

    /// Points the library at a new capture folder; call `scan` afterwards.
    pub fn set_dirs(&self, clips_dir: PathBuf, shots_dir: PathBuf) {
        *self.dirs.lock().unwrap() = (clips_dir, shots_dir);
    }

    /// Total size of the indexed files, which all live in the capture folder.
    pub fn total_size(&self) -> u64 {
        self.db.lock().unwrap().query_row("SELECT COALESCE(SUM(size), 0) FROM items", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64
    }

    /// The next item the size limit removes (spec storage.limits): the one deleted
    /// first, else the oldest item not marked to keep.
    pub fn oldest_removable(&self) -> Option<String> {
        self.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT path FROM items WHERE kept = 0 OR deleted_ms IS NOT NULL ORDER BY deleted_ms IS NULL, deleted_ms, created_ms LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn remove(&self, path: &str) {
        let _ = self.db.lock().unwrap().execute("DELETE FROM items WHERE path = ?1", [path]);
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
        let path = share::windows_path(path);
        self.get(&path)?.filter(|i| i["deletedMs"].is_null()).ok_or("item not found")?;
        let file = Path::new(&path);
        if !file.exists() {
            self.purge(&path, false)?;
            return Ok(Value::Null);
        }
        let target = free_name(&self.ensure_trash()?, file)?;
        std::fs::rename(file, &target).map_err(|e| format!("{}: {e}", file.display()))?;
        let key = target.to_string_lossy().into_owned();
        self.db
            .lock()
            .unwrap()
            .execute("UPDATE items SET path = ?2, original_path = ?1, deleted_ms = ?3 WHERE path = ?1", params![path, key, now_ms()])
            .map_err(|e| e.to_string())?;
        self.remove_timeline(file);
        self.get(&key)?.ok_or_else(|| "item not indexed".into())
    }

    /// Brings a deleted item back where it was, under a free name if that one was
    /// taken since.
    pub fn restore(&self, path: &str) -> Result<Value, String> {
        let path = share::windows_path(path);
        let item = self.get(&path)?.ok_or("item not found")?;
        let original = PathBuf::from(item["originalPath"].as_str().ok_or("the item is not in Recently deleted")?);
        let dir = original.parent().ok_or("bad file path")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let target = if original.exists() { free_name(dir, &original)? } else { original };
        std::fs::rename(&path, &target).map_err(|e| format!("{path}: {e}"))?;
        let key = target.to_string_lossy().into_owned();
        self.db
            .lock()
            .unwrap()
            .execute("UPDATE items SET path = ?2, original_path = NULL, deleted_ms = NULL WHERE path = ?1", [&path, &key])
            .map_err(|e| e.to_string())?;
        self.get(&key)?.ok_or_else(|| "item not indexed".into())
    }

    /// Removes an item for good, with its thumbnail and timeline frames: the file
    /// goes to the Windows Recycle Bin when `recycle`, else it is deleted.
    pub fn purge(&self, path: &str, recycle: bool) -> Result<(), String> {
        let path = share::windows_path(path);
        let item = self.get(&path)?;
        let file = Path::new(&path);
        if file.exists() {
            win::delete_file(file, recycle)?;
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
        let cutoff = now_ms() - days as i64 * DAY_MS;
        let paths: Vec<String> = {
            let db = self.db.lock().unwrap();
            db.prepare("SELECT path FROM items WHERE deleted_ms IS NOT NULL AND deleted_ms <= ?1")
                .and_then(|mut stmt| stmt.query_map([cutoff], |r| r.get(0))?.collect())
                .unwrap_or_default()
        };
        paths
            .into_iter()
            .filter(|path| match self.purge(path, recycle) {
                Ok(()) => true,
                Err(e) => {
                    crate::log::line(&format!("recently deleted: {e}"));
                    false
                }
            })
            .collect()
    }

    /// How many items wait in Recently deleted, and their size in bytes.
    pub fn deleted_size(&self) -> (u64, u64) {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM items WHERE deleted_ms IS NOT NULL", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))
            .unwrap_or((0, 0))
    }

    /// Moves an entry to a new path, and its original place too when given (a
    /// deleted item when the capture folder moves).
    pub fn rename(&self, from: &str, to: &str, original: Option<&str>) {
        let _ = self.db.lock().unwrap().execute("UPDATE items SET path = ?2, original_path = COALESCE(?3, original_path) WHERE path = ?1", params![from, to, original]);
    }

    pub fn set_kept(&self, path: &str, kept: bool) -> Result<Value, String> {
        let path = share::windows_path(path);
        self.db.lock().unwrap().execute("UPDATE items SET kept = ?2 WHERE path = ?1", params![path, kept]).map_err(|e| e.to_string())?;
        self.get(&path)?.ok_or_else(|| "item not found".into())
    }

    fn thumbnail_for(&self, path: &Path, kind: &str, duration_ms: i64) -> Option<String> {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        // The engine writes "<clip stem>.png" when it saves a clip.
        let thumb = self.thumbs_dir.join(if kind == "clip" { format!("{stem}.png") } else { format!("{stem}.screenshot.png") });
        if !thumb.exists() {
            let ffmpeg = self.ffmpeg.as_ref()?;
            let at = if kind == "clip" { duration_ms as f64 / 2000.0 } else { 0.0 };
            if let Err(e) = media::thumbnail(ffmpeg, path, &thumb, at) {
                crate::log::line(&format!("thumbnail for {} failed: {e}", path.display()));
                return None;
            }
        }
        Some(thumb.to_string_lossy().into_owned())
    }

    /// Indexes one file (or refreshes its entry) and returns the item.
    pub fn add(&self, path: &str, game: Option<String>) -> Result<Value, String> {
        let path = PathBuf::from(share::windows_path(path));
        let kind = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) { "clip" } else { "screenshot" };
        let (duration_ms, width, height, fps, audio_tracks) = if kind == "clip" {
            let info = mp4::probe(&path).unwrap_or_default();
            (info.duration_ms as i64, info.width, info.height, info.fps, info.audio_tracks)
        } else {
            let (w, h) = mp4::png_size(&path).unwrap_or((0, 0));
            (0, w, h, 0.0, 0)
        };
        let thumbnail = self.thumbnail_for(&path, kind, duration_ms);
        let key = path.to_string_lossy().into_owned();
        // The game given wins, then the one already indexed, then the file name's.
        let named = path.file_stem().and_then(|s| game_from_name(&s.to_string_lossy()));
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO items (path, kind, game, created_ms, duration_ms, width, height, fps, audio_tracks, size, thumbnail)
                 VALUES (?1, ?2, COALESCE(?3, ?12), ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(path) DO UPDATE SET game = COALESCE(?3, game, ?12), duration_ms = excluded.duration_ms,
                   width = excluded.width, height = excluded.height, fps = excluded.fps, audio_tracks = excluded.audio_tracks,
                   size = excluded.size, thumbnail = COALESCE(excluded.thumbnail, thumbnail)",
                params![key, kind, game, modified_ms(&path), duration_ms, width, height, fps, audio_tracks, file_size(&path), thumbnail, named],
            )
            .map_err(|e| e.to_string())?;
        self.get(&key)?.ok_or_else(|| "item not indexed".into())
    }

    fn get(&self, path: &str) -> Result<Option<Value>, String> {
        self.db.lock().unwrap().query_row("SELECT * FROM items WHERE path = ?1", [path], row_json).optional().map_err(|e| e.to_string())
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
        let indexed: Vec<(String, Option<String>)> = {
            let db = self.db.lock().unwrap();
            let mut stmt = db.prepare("SELECT path, thumbnail FROM items").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
            rows.flatten().collect()
        };
        let mut removed = 0;
        // Entries whose file is gone, or that are not in the capture folder any more.
        for (path, _) in indexed.iter().filter(|(p, _)| !on_disk.contains(p) && !in_trash.contains(p)) {
            self.db.lock().unwrap().execute("DELETE FROM items WHERE path = ?1", [path]).map_err(|e| e.to_string())?;
            removed += 1;
        }
        for path in in_trash.iter().filter(|p| !indexed.iter().any(|(i, _)| i == *p)) {
            self.add(path, None)?;
            let file = Path::new(path);
            let home = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")) { &clips_dir } else { &shots_dir };
            let original = home.join(file.file_name().unwrap_or_default()).to_string_lossy().into_owned();
            self.db
                .lock()
                .unwrap()
                .execute("UPDATE items SET original_path = ?2, deleted_ms = ?3 WHERE path = ?1", params![path, original, now_ms()])
                .map_err(|e| e.to_string())?;
        }
        // Items indexed without a game get the one in their file name.
        {
            let db = self.db.lock().unwrap();
            let unnamed: Vec<String> = db
                .prepare("SELECT path FROM items WHERE game IS NULL")
                .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect())
                .map_err(|e| e.to_string())?;
            for path in unnamed {
                if let Some(game) = Path::new(&path).file_stem().and_then(|s| game_from_name(&s.to_string_lossy())) {
                    db.execute("UPDATE items SET game = ?2 WHERE path = ?1", [&path, &game]).map_err(|e| e.to_string())?;
                }
            }
        }
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
        let db = self.db.lock().unwrap();
        let mut stmt = db
            .prepare("SELECT * FROM items WHERE deleted_ms IS NULL AND (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR game = ?2) ORDER BY created_ms DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![kind, game], row_json).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// The items in Recently deleted, the last deleted first.
    pub fn list_deleted(&self) -> Result<Vec<Value>, String> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT * FROM items WHERE deleted_ms IS NOT NULL ORDER BY deleted_ms DESC").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], row_json).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// Games that have items of a kind, for the filter.
    pub fn games(&self, kind: Option<&str>) -> Result<Vec<String>, String> {
        let db = self.db.lock().unwrap();
        let mut stmt = db
            .prepare("SELECT DISTINCT game FROM items WHERE game IS NOT NULL AND deleted_ms IS NULL AND (?1 IS NULL OR kind = ?1) ORDER BY game")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([kind], |r| r.get(0)).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// Saves `start..end` seconds of a clip as a new file next to it, keeping the
    /// original (spec edit.trim), and indexes it with the same game.
    /// Saves `start..end` seconds as a new clip next to the original: fast (stream
    /// copy from the keyframe at or before `start`) or exact (re-encoded).
    pub fn trim(&self, path: &str, start: f64, end: f64, exact: bool) -> Result<Value, String> {
        let ffmpeg = self.ffmpeg.as_ref().ok_or("ffmpeg.exe was not found")?;
        if end <= start {
            return Err(t!("The end must come after the start"));
        }
        let source = PathBuf::from(share::windows_path(path));
        let stem = source.file_stem().ok_or("bad clip path")?.to_string_lossy().into_owned();
        let dir = source.parent().ok_or("bad clip path")?;
        let out = trim_path(dir, &stem);
        if exact {
            media::trim_exact(ffmpeg, &source, &out, start, end)?;
        } else {
            // Stream copy seeks to the keyframe at or before the time asked, so
            // half a millisecond after `start` (a keyframe's time) keeps that
            // keyframe; it stops at the packets whose decode time reaches `to`, and
            // decode times run behind by the B-frame delay, so it is taken off the
            // end: the copy then holds the frames before `end` (checked frame by
            // frame on NVENC clips with two B-frames).
            let delay = mp4::video_frames(&source).map(|f| f.delay).unwrap_or(0.0);
            media::trim(ffmpeg, &source, &out, start + 0.0005, end - delay - 0.0005)?;
        }
        let game = self.get(&source.to_string_lossy())?.and_then(|item| item["game"].as_str().map(String::from));
        self.add(&out.to_string_lossy(), game)
    }
}

impl Library {
    /// Frame and keyframe times of a clip (spec edit.trim), from the file itself.
    pub fn video_frames(&self, path: &str) -> Result<mp4::VideoFrames, String> {
        mp4::video_frames(Path::new(&share::windows_path(path)))
    }

    /// `count` small frames spread over a clip for the trim timeline, cached in the
    /// thumbnails folder by file name, size and modification time.
    pub fn timeline(&self, path: &str, count: usize) -> Result<Vec<PathBuf>, String> {
        let ffmpeg = self.ffmpeg.as_ref().ok_or("ffmpeg.exe was not found")?;
        let source = PathBuf::from(share::windows_path(path));
        let duration = mp4::probe(&source)?.duration_ms as f64 / 1000.0;
        let key = format!("{}-{}-{}", source.file_stem().unwrap_or_default().to_string_lossy(), file_size(&source), modified_ms(&source));
        let dir = self.thumbs_dir.join("timeline").join(key);
        let frames: Vec<(f64, PathBuf)> = (0..count).map(|i| ((i as f64 + 0.5) * duration / count as f64, dir.join(format!("{i:02}.jpg")))).collect();
        if !frames.iter().all(|(_, p)| p.exists()) {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            media::frames(ffmpeg, &source, &frames, 192)?;
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

fn row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "path": r.get::<_, String>("path")?,
        "kind": r.get::<_, String>("kind")?,
        "game": r.get::<_, Option<String>>("game")?,
        "createdMs": r.get::<_, i64>("created_ms")?,
        "durationMs": r.get::<_, Option<i64>>("duration_ms")?,
        "width": r.get::<_, Option<i64>>("width")?,
        "height": r.get::<_, Option<i64>>("height")?,
        "fps": r.get::<_, Option<f64>>("fps")?,
        "audioTracks": r.get::<_, Option<i64>>("audio_tracks")?,
        "size": r.get::<_, i64>("size")?,
        "thumbnail": r.get::<_, Option<String>>("thumbnail")?,
        "kept": r.get::<_, i64>("kept")? != 0,
        "deletedMs": r.get::<_, Option<i64>>("deleted_ms")?,
        "originalPath": r.get::<_, Option<String>>("original_path")?,
    }))
}
