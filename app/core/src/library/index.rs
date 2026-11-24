//! The library index (specs library.browse, library.trash, edit.trim,
//! autoclip.steam-achievements, library.tags): the SQLite file that records the
//! clips and screenshots in the capture folder, their game, their Keep mark,
//! achievement and tags, and the items waiting in Recently deleted. The app moves
//! the files, measures them and makes their thumbnails; this module keeps the index
//! in step with what it did.
//!
//! Paths are text, as the app gives them (Windows paths): an item's key is its path,
//! compared exactly. The time comes in as a parameter (milliseconds since the Unix
//! epoch), so that the rules never read the clock.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::tags::{self, CustomTag, TagError, TagState};

const DAY_MS: i64 = 86_400_000;

/// The library index, open on its SQLite file.
pub struct Index {
    db: Connection,
}

/// A file as the app measured it, for `Index::add`.
#[derive(Clone, Debug)]
pub struct FileFacts {
    pub path: String,
    /// "clip" or "screenshot".
    pub kind: &'static str,
    /// The file's modification time, which stands for when it was made.
    pub created_ms: i64,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio_tracks: u32,
    pub size: i64,
    pub thumbnail: Option<String>,
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

/// The game in the name of the file at `path`, a Windows path given as text.
fn game_in_file_name(path: &str) -> Option<String> {
    game_from_name(file_stem(path)?)
}

/// The file name without its extension, as Windows' `Path::file_stem` gives it
/// (both `\` and `/` separate folders).
fn file_stem(path: &str) -> Option<&str> {
    let name = path.rsplit(['\\', '/']).next().filter(|n| !n.is_empty() && *n != "..")?;
    match name.rsplit_once('.') {
        Some((before, _)) if !before.is_empty() => Some(before),
        _ => Some(name),
    }
}

impl Index {
    /// Opens the index at `file`, creating it if needed, and adds the columns that
    /// an index made by an older version lacks.
    pub fn open(file: &Path) -> Result<Index, String> {
        let db = Connection::open(file).map_err(|e| e.to_string())?;
        // An item's tags follow its path through item_tags' foreign key (below),
        // which SQLite enforces only with this on. The SQLite built into Laurel
        // turns it on by default, as it did in every version, so a version from
        // before the tags moves them too; the pragma keeps it on whatever the build.
        db.execute_batch("PRAGMA foreign_keys = ON;").map_err(|e| e.to_string())?;
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
        // The achievement an automatic clip was saved for (autoclip.steam-achievements),
        // as JSON, added in 0.1.0-rc.4.
        for (column, kind) in [("deleted_ms", "INTEGER"), ("original_path", "TEXT"), ("achievement", "TEXT")] {
            if !columns.iter().any(|c| c == column) {
                db.execute_batch(&format!("ALTER TABLE items ADD COLUMN {column} {kind};")).map_err(|e| e.to_string())?;
            }
        }
        // Tags (spec library.tags), added in 0.1.0-rc.18: which tags each item has.
        // They move with the item when its path changes (to Recently deleted and
        // back, a moved capture folder) and go with its entry.
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS item_tags (
                path TEXT NOT NULL REFERENCES items(path) ON UPDATE CASCADE ON DELETE CASCADE,
                tag TEXT NOT NULL,
                PRIMARY KEY (path, tag)
            );",
        )
        .map_err(|e| e.to_string())?;
        // The custom tags (spec library.tags), also from 0.1.0-rc.18, kept even when no
        // item has them. A tag's ID is "custom-<n>": AUTOINCREMENT never gives a number
        // again, even after the tag with the highest one is deleted.
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS custom_tags (
                n INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Index { db })
    }

    /// The custom tags, by name, the order of the picker's group "Your tags".
    pub fn custom_tags(&self) -> Result<Vec<CustomTag>, String> {
        let mut stmt = self.db.prepare("SELECT 'custom-' || n, name FROM custom_tags").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok(CustomTag { id: r.get(0)?, name: r.get(1)? })).map_err(|e| e.to_string())?;
        let mut custom: Vec<CustomTag> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        custom.sort_by(|a, b| tags::by_name(&a.name, &b.name));
        Ok(custom)
    }

    /// Creates a custom tag with the name the user typed (spec library.tags) and
    /// returns it. The name follows [`Index::custom_name`]'s rules.
    pub fn create_tag(&self, name: &str, local_names: &[&str]) -> Result<CustomTag, TagError> {
        let name = self.custom_name(name, local_names, None)?;
        self.db.execute("INSERT INTO custom_tags (name) VALUES (?1)", [&name]).map_err(|e| TagError::Index(e.to_string()))?;
        Ok(CustomTag { id: format!("custom-{}", self.db.last_insert_rowid()), name })
    }

    /// Renames the custom tag with this ID (spec library.tags) and returns it. Its
    /// items keep it, since they store its ID. The new name follows
    /// [`Index::custom_name`]'s rules, and can be the tag's own name written another
    /// way ("boss fight" for "Boss fight"): a taken name never joins two tags.
    pub fn rename_tag(&self, id: &str, name: &str, local_names: &[&str]) -> Result<CustomTag, TagError> {
        if !self.is_custom_tag(id).map_err(TagError::Index)? {
            return Err(TagError::Unknown(id.to_string()));
        }
        let name = self.custom_name(name, local_names, Some(id))?;
        self.db.execute("UPDATE custom_tags SET name = ?2 WHERE 'custom-' || n = ?1", [id, &name]).map_err(|e| TagError::Index(e.to_string()))?;
        Ok(CustomTag { id: id.to_string(), name })
    }

    /// The name a custom tag gets from what the user typed, under the rules of
    /// [`tags::check_name`]: it cannot repeat another custom tag's (but the one with
    /// the ID `renamed`), nor a standard tag's in English or in `local_names`, the
    /// standard tags' names in the interface's language, which the app gives.
    fn custom_name(&self, name: &str, local_names: &[&str], renamed: Option<&str>) -> Result<String, TagError> {
        let custom = self.custom_tags().map_err(TagError::Index)?;
        let others = custom.iter().filter(|tag| Some(tag.id.as_str()) != renamed).map(|tag| tag.name.as_str());
        let taken = others.chain(local_names.iter().copied()).chain(tags::STANDARD.iter().map(|tag| tag.name));
        Ok(tags::check_name(name, taken)?)
    }

    /// Deletes the custom tag with this ID (spec library.tags) and takes it off every
    /// item, those in Recently deleted too. Its ID is never given again.
    pub fn delete_tag(&self, id: &str) -> Result<(), TagError> {
        if !self.is_custom_tag(id).map_err(TagError::Index)? {
            return Err(TagError::Unknown(id.to_string()));
        }
        let index = |e: rusqlite::Error| TagError::Index(e.to_string());
        let tx = self.db.unchecked_transaction().map_err(index)?;
        tx.execute("DELETE FROM item_tags WHERE tag = ?1", [id]).map_err(index)?;
        tx.execute("DELETE FROM custom_tags WHERE 'custom-' || n = ?1", [id]).map_err(index)?;
        tx.commit().map_err(index)
    }

    /// Whether a custom tag has this ID.
    fn is_custom_tag(&self, id: &str) -> Result<bool, String> {
        self.db.query_row("SELECT EXISTS (SELECT 1 FROM custom_tags WHERE 'custom-' || n = ?1)", [id], |r| r.get(0)).map_err(|e| e.to_string())
    }

    /// Total size of the indexed files, Recently deleted included.
    pub fn total_size(&self) -> u64 {
        self.db.query_row("SELECT COALESCE(SUM(size), 0) FROM items", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as u64
    }

    /// The next item the size limit removes (spec storage.limits): the one deleted
    /// first, else the oldest item not marked to keep.
    pub fn oldest_removable(&self) -> Option<String> {
        self.db
            .query_row(
                "SELECT path FROM items WHERE kept = 0 OR deleted_ms IS NOT NULL ORDER BY deleted_ms IS NULL, deleted_ms, created_ms LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()
    }

    /// Drops an entry.
    pub fn remove(&self, path: &str) -> Result<(), String> {
        self.db.execute("DELETE FROM items WHERE path = ?1", [path]).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Records that an item's file moved to Recently deleted at `to`, now: it
    /// remembers where it came from. Returns the item at its new place.
    pub fn mark_deleted(&self, path: &str, to: &str, now_ms: i64) -> Result<Value, String> {
        self.db
            .execute("UPDATE items SET path = ?2, original_path = ?1, deleted_ms = ?3 WHERE path = ?1", params![path, to, now_ms])
            .map_err(|e| e.to_string())?;
        self.written(to)
    }

    /// Records that a deleted item's file came back from Recently deleted to `to`.
    /// Returns the item at its new place.
    pub fn mark_restored(&self, path: &str, to: &str) -> Result<Value, String> {
        self.db
            .execute("UPDATE items SET path = ?2, original_path = NULL, deleted_ms = NULL WHERE path = ?1", [path, to])
            .map_err(|e| e.to_string())?;
        self.written(to)
    }

    /// The items deleted `days` or more days before `now_ms`, which leave for good.
    pub fn expired(&self, days: u32, now_ms: i64) -> Vec<String> {
        let cutoff = now_ms - days as i64 * DAY_MS;
        self.db
            .prepare("SELECT path FROM items WHERE deleted_ms IS NOT NULL AND deleted_ms <= ?1")
            .and_then(|mut stmt| stmt.query_map([cutoff], |r| r.get(0))?.collect())
            .unwrap_or_default()
    }

    /// How many items wait in Recently deleted, and their size in bytes.
    pub fn deleted_size(&self) -> (u64, u64) {
        self.db
            .query_row("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM items WHERE deleted_ms IS NOT NULL", [], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))
            .unwrap_or((0, 0))
    }

    /// Moves an entry to a new path, and its original place too when given (a
    /// deleted item when the capture folder moves).
    pub fn rename(&self, from: &str, to: &str, original: Option<&str>) -> Result<(), String> {
        self.db
            .execute("UPDATE items SET path = ?2, original_path = COALESCE(?3, original_path) WHERE path = ?1", params![from, to, original])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Sets or clears an item's Keep mark (spec library.browse), which the size
    /// limit respects, and returns the item.
    pub fn set_kept(&self, path: &str, kept: bool) -> Result<Value, String> {
        self.db.execute("UPDATE items SET kept = ?2 WHERE path = ?1", params![path, kept]).map_err(|e| e.to_string())?;
        self.get(path)?.ok_or_else(|| "item not found".into())
    }

    /// Puts a tag on an item, or takes it off, and returns the item. Only a
    /// standard tag or a custom tag, by its ID, can be put on, and the tags of an
    /// item in Recently deleted stay as they are until it is restored.
    pub fn set_tag(&self, path: &str, tag: &str, on: bool) -> Result<Value, String> {
        if on && tags::standard(tag).is_none() && !self.is_custom_tag(tag)? {
            return Err(format!("unknown tag: {tag}"));
        }
        let item = self.get(path)?.ok_or("item not found")?;
        if !item["deletedMs"].is_null() {
            return Err("the item is in Recently deleted".into());
        }
        let sql = if on { "INSERT OR IGNORE INTO item_tags (path, tag) VALUES (?1, ?2)" } else { "DELETE FROM item_tags WHERE path = ?1 AND tag = ?2" };
        self.db.execute(sql, [path, tag]).map_err(|e| e.to_string())?;
        self.written(path)
    }

    /// Indexes one file (or refreshes its entry) and returns the item. The game
    /// given wins, then the one already indexed, then the file name's.
    pub fn add(&self, file: &FileFacts, game: Option<&str>) -> Result<Value, String> {
        let named = game_in_file_name(&file.path);
        self.db
            .execute(
                "INSERT INTO items (path, kind, game, created_ms, duration_ms, width, height, fps, audio_tracks, size, thumbnail)
                 VALUES (?1, ?2, COALESCE(?3, ?12), ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(path) DO UPDATE SET game = COALESCE(?3, game, ?12), duration_ms = excluded.duration_ms,
                   width = excluded.width, height = excluded.height, fps = excluded.fps, audio_tracks = excluded.audio_tracks,
                   size = excluded.size, thumbnail = COALESCE(excluded.thumbnail, thumbnail)",
                params![file.path, file.kind, game, file.created_ms, file.duration_ms, file.width, file.height, file.fps, file.audio_tracks, file.size, file.thumbnail, named],
            )
            .map_err(|e| e.to_string())?;
        self.written(&file.path)
    }

    /// Indexes a trimmed copy of `source` (spec edit.trim) with the source's game
    /// and tags (spec library.tags); Keep and the achievement stay with the source.
    pub fn add_trim(&self, source: &str, copy: &FileFacts) -> Result<Value, String> {
        let game = self.get(source)?.and_then(|item| item["game"].as_str().map(String::from));
        self.add(copy, game.as_deref())?;
        self.db
            .execute("INSERT OR IGNORE INTO item_tags (path, tag) SELECT ?2, tag FROM item_tags WHERE path = ?1", [source, &copy.path])
            .map_err(|e| e.to_string())?;
        self.written(&copy.path)
    }

    /// Marks an item as an automatic clip of an achievement and returns it.
    pub fn set_achievement(&self, path: &str, achievement: &Value) -> Result<Value, String> {
        self.db.execute("UPDATE items SET achievement = ?2 WHERE path = ?1", [path, &achievement.to_string()]).map_err(|e| e.to_string())?;
        self.written(path)
    }

    /// The item at `path`, deleted or not.
    pub fn get(&self, path: &str) -> Result<Option<Value>, String> {
        self.db.query_row(&format!("SELECT {ITEM_COLUMNS} FROM items WHERE path = ?1"), [path], row_json).optional().map_err(|e| e.to_string())
    }

    /// The item an operation just wrote at `path`.
    fn written(&self, path: &str) -> Result<Value, String> {
        self.get(path)?.ok_or_else(|| "item not indexed".into())
    }

    /// Every entry's path and thumbnail.
    pub fn entries(&self) -> Result<Vec<(String, Option<String>)>, String> {
        let mut stmt = self.db.prepare("SELECT path, thumbnail FROM items").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// Drops the entries whose path is not among `found`, the files in the capture
    /// folder and in Recently deleted; returns how many went.
    pub fn remove_missing(&self, found: &[String]) -> Result<usize, String> {
        let mut removed = 0;
        for (path, _) in self.entries()?.iter().filter(|(p, _)| !found.contains(p)) {
            self.remove(path)?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Records a file found in Recently deleted without an entry (a rebuilt index)
    /// as deleted now, from `original`.
    pub fn mark_found_deleted(&self, path: &str, original: &str, now_ms: i64) -> Result<(), String> {
        self.db
            .execute("UPDATE items SET original_path = ?2, deleted_ms = ?3 WHERE path = ?1", params![path, original, now_ms])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Items indexed without a game get the one in their file name (an index made
    /// by a build from before rc.1, which did not read file names).
    pub fn fill_games_from_file_names(&self) -> Result<(), String> {
        let unnamed: Vec<String> = self
            .db
            .prepare("SELECT path FROM items WHERE game IS NULL")
            .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect())
            .map_err(|e| e.to_string())?;
        for path in unnamed {
            if let Some(game) = game_in_file_name(&path) {
                self.db.execute("UPDATE items SET game = ?2 WHERE path = ?1", [&path, &game]).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Items newest first, optionally of one kind and one game; deleted ones apart.
    pub fn list(&self, kind: Option<&str>, game: Option<&str>) -> Result<Vec<Value>, String> {
        let mut stmt = self
            .db
            .prepare(&format!("SELECT {ITEM_COLUMNS} FROM items WHERE deleted_ms IS NULL AND (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR game = ?2) ORDER BY created_ms DESC"))
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![kind, game], row_json).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// The items in Recently deleted, the last deleted first.
    pub fn list_deleted(&self) -> Result<Vec<Value>, String> {
        let mut stmt = self.db.prepare(&format!("SELECT {ITEM_COLUMNS} FROM items WHERE deleted_ms IS NOT NULL ORDER BY deleted_ms DESC")).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], row_json).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// Each tag in use, by ID, with how many items have it (spec library.tags),
    /// `{"funny": {"items": 2, "deleted": 1}}`: `items` outside Recently deleted, for
    /// the side panel's Tags section, and `deleted` in it, so that a custom tag's
    /// delete can count every item it leaves. A tag left only on deleted items is
    /// still in use.
    pub fn tag_counts(&self) -> Result<Value, String> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT tag, SUM(items.deleted_ms IS NULL), SUM(items.deleted_ms IS NOT NULL)
                 FROM item_tags JOIN items ON items.path = item_tags.path GROUP BY tag",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).map_err(|e| e.to_string())?;
        let mut counts = serde_json::Map::new();
        for row in rows {
            let (tag, items, deleted) = row.map_err(|e| e.to_string())?;
            counts.insert(tag, json!({ "items": items, "deleted": deleted }));
        }
        Ok(Value::Object(counts))
    }

    /// Each tag, standard or custom, by ID, with how many of the items at `paths`
    /// have it (spec library.tags), as the selection bar's picker shows it: all of
    /// them (✓), some (–) or none (empty).
    pub fn tag_states(&self, paths: &[&str]) -> Result<BTreeMap<String, TagState>, String> {
        let mut having: BTreeMap<String, usize> = tags::STANDARD.iter().map(|tag| (tag.id.to_string(), 0)).collect();
        for tag in self.custom_tags()? {
            having.insert(tag.id, 0);
        }
        let mut stmt = self.db.prepare("SELECT tag FROM item_tags WHERE path = ?1").map_err(|e| e.to_string())?;
        for path in paths {
            for tag in stmt.query_map([path], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())? {
                *having.entry(tag.map_err(|e| e.to_string())?).or_default() += 1;
            }
        }
        Ok(having.into_iter().map(|(tag, n)| (tag, TagState::of(n, paths.len()))).collect())
    }

    /// A click on a tag in the selection bar's picker (spec library.tags): on – or
    /// empty (see [`Index::tag_states`]) it puts the tag on every item at `paths`, on
    /// ✓ it takes it off every one. Each item as [`Index::set_tag`] gives it, or why
    /// it did not change.
    pub fn click_tag(&self, paths: &[&str], tag: &str) -> Result<Vec<Result<Value, String>>, String> {
        // A tag the index does not know is put on, which set_tag refuses.
        let on = self.tag_states(paths)?.get(tag).is_none_or(|state| state.click_puts_on());
        Ok(paths.iter().map(|path| self.set_tag(path, tag, on)).collect())
    }

    /// Games that have items of a kind, for the filter.
    pub fn games(&self, kind: Option<&str>) -> Result<Vec<String>, String> {
        let mut stmt = self
            .db
            .prepare("SELECT DISTINCT game FROM items WHERE game IS NOT NULL AND deleted_ms IS NULL AND (?1 IS NULL OR kind = ?1) ORDER BY game")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([kind], |r| r.get(0)).map_err(|e| e.to_string())?;
        Ok(rows.flatten().collect())
    }

    /// A trimmed copy takes its original's place ("Save and delete original", spec
    /// edit.trim): the entry of `copy` moves to the original's path and takes its
    /// date, Keep mark, achievement and tags (spec library.tags) in place of its
    /// own, and `thumbnail` when given. `original` is the original's item as the
    /// index gave it before it went to Recently deleted, where its entry keeps its
    /// own tags. Returns the copy at its new place.
    pub fn take_place(&self, copy: &str, original: &Value, thumbnail: Option<&str>, now_ms: i64) -> Result<Value, String> {
        let path = original["path"].as_str().ok_or("item not indexed")?;
        let achievement = (!original["achievement"].is_null()).then(|| original["achievement"].to_string());
        self.db
            .execute(
                "UPDATE items SET path = ?2, created_ms = ?3, kept = ?4, achievement = ?5, thumbnail = COALESCE(?6, thumbnail) WHERE path = ?1",
                params![copy, path, original["createdMs"].as_i64().unwrap_or(now_ms), original["kept"].as_bool().unwrap_or(false), achievement, thumbnail],
            )
            .map_err(|e| e.to_string())?;
        self.db.execute("DELETE FROM item_tags WHERE path = ?1", [path]).map_err(|e| e.to_string())?;
        for tag in original["tags"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            self.db.execute("INSERT OR IGNORE INTO item_tags (path, tag) VALUES (?1, ?2)", [path, tag]).map_err(|e| e.to_string())?;
        }
        self.written(path)
    }
}

/// The columns of an item as `row_json` reads it: its own and its tags, as a JSON
/// list of each tag's ID and, for a custom tag, its name.
const ITEM_COLUMNS: &str = "*, (SELECT json_group_array(json_array(item_tags.tag, custom_tags.name)) FROM item_tags
    LEFT JOIN custom_tags ON item_tags.tag = 'custom-' || custom_tags.n WHERE item_tags.path = items.path) AS tags";

/// An item as the interface reads it.
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
        "achievement": r.get::<_, Option<String>>("achievement")?.and_then(|a| serde_json::from_str::<Value>(&a).ok()),
        "tags": tags_in_chip_order(&r.get::<_, String>("tags")?),
    }))
}

/// An item's tag IDs, from the JSON list the query gives, in the order of its chips.
fn tags_in_chip_order(list: &str) -> Vec<String> {
    let mut tags: Vec<(String, Option<String>)> = serde_json::from_str(list).unwrap_or_default();
    tags::sort_chips(&mut tags);
    tags.into_iter().map(|(id, _)| id).collect()
}
