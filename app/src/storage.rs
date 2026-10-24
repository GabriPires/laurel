//! Capture folder, file names and space limits (specs storage.location,
//! storage.limits): where clips and screenshots go, how they are named, the
//! folder size limit that removes the oldest items, and the free-space guard that
//! pauses recording when the drive is almost full.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::library::Library;
use crate::settings::Store;
use crate::win;

const GB: u64 = 1024 * 1024 * 1024;
/// Recording resumes once free space is this much above the minimum.
const RESUME_MARGIN: u64 = GB;

pub struct Storage {
    store: Arc<Store>,
    /// A fixed capture folder that ignores the settings (the self-test uses one,
    /// so it never writes into the user's folder).
    override_folder: Option<PathBuf>,
    paused: AtomicBool,
}

/// Windows file-name rules: no reserved characters or names, no trailing dots or
/// spaces, a sane length.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let mut cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned.truncate(cleaned.char_indices().nth(120).map_or(cleaned.len(), |(i, _)| i));
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];
    let stem = cleaned.split('.').next().unwrap_or_default().to_ascii_uppercase();
    match cleaned.as_str() {
        "" => "Laurel".into(),
        _ if reserved.contains(&stem.as_str()) => format!("{cleaned} clip"),
        _ => cleaned,
    }
}

/// `dir/<name>.<ext>`, with " (2)", " (3)"... when that name is taken.
fn unique(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{name}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// Moves a file, copying it when the destination is on another drive.
/// The date and time of the save, from the engine's name for the file ("Laurel
/// <date> <time>", maybe with " (2)" after it), so the final name keeps the moment
/// the engine chose even when the save ends a second later; else the time now.
fn saved_at(stem: &str) -> (String, String) {
    let mut parts = stem.split(' ');
    let digits = |s: &str, dashes: &[usize]| s.char_indices().all(|(i, c)| if dashes.contains(&i) { c == '-' } else { c.is_ascii_digit() });
    if let (Some("Laurel"), Some(date), Some(time)) = (parts.next(), parts.next(), parts.next()) {
        if date.len() == 10 && digits(date, &[4, 7]) && time.len() == 8 && digits(time, &[2, 5]) {
            return (date.to_string(), time.to_string());
        }
    }
    win::date_time()
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to).or_else(|_| {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)
    })
}

impl Storage {
    pub fn new(store: Arc<Store>, override_folder: Option<PathBuf>) -> Storage {
        Storage { store, override_folder, paused: AtomicBool::new(false) }
    }

    /// The capture folder: the user's choice, else Videos\Laurel.
    pub fn folder(&self) -> PathBuf {
        if let Some(folder) = &self.override_folder {
            return folder.clone();
        }
        let folder = self.store.get().folder;
        if !folder.is_empty() {
            return PathBuf::from(folder);
        }
        win::videos_folder().unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Videos")).join("Laurel")
    }

    pub fn clips_dir(&self) -> PathBuf {
        self.folder().join("Clips")
    }

    pub fn screenshots_dir(&self) -> PathBuf {
        self.folder().join("Screenshots")
    }

    /// Creates the folder if needed and proves it is writable with a probe file.
    pub fn check_writable(folder: &Path) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        let probe = folder.join(".laurel-write-test");
        std::fs::write(&probe, b"laurel").map_err(|e| format!("cannot write to {}: {e}", folder.display()))?;
        let _ = std::fs::remove_file(probe);
        Ok(())
    }

    /// Renames a file the engine just saved after the naming pattern (fields
    /// {game}, {date}, {time}) and moves the engine's thumbnail along. Returns the
    /// new path; on failure the file keeps its engine name.
    pub fn apply_naming(&self, saved: &str, game: Option<&str>, thumbs_dir: &Path) -> String {
        let path = PathBuf::from(crate::share::windows_path(saved));
        let (Some(dir), Some(ext), Some(old_stem)) = (path.parent(), path.extension(), path.file_stem()) else {
            return saved.to_string();
        };
        let (date, time) = saved_at(&old_stem.to_string_lossy());
        let name = self.store.get().naming.replace("{game}", game.unwrap_or("Laurel")).replace("{date}", &date).replace("{time}", &time);
        let ext = ext.to_string_lossy();
        // The engine already names files "Laurel <date> <time>"; the same name stays
        // as it is instead of becoming "… (2)".
        let wanted = format!("{}.{ext}", sanitize(&name));
        if path.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&wanted)) {
            return path.to_string_lossy().into_owned();
        }
        let target = unique(dir, &sanitize(&name), &ext);
        if let Err(e) = std::fs::rename(&path, &target) {
            crate::log::line(&format!("could not rename {}: {e}", path.display()));
            return path.to_string_lossy().into_owned();
        }
        let old_thumb = thumbs_dir.join(format!("{}.png", old_stem.to_string_lossy()));
        if let Some(new_stem) = target.file_stem() {
            let _ = std::fs::rename(&old_thumb, thumbs_dir.join(format!("{}.png", new_stem.to_string_lossy())));
        }
        target.to_string_lossy().into_owned()
    }

    /// Removes the oldest items that are not marked to keep until the capture
    /// folder fits the limit; returns the removed paths.
    pub fn enforce_limit(&self, library: &Library) -> Vec<String> {
        let settings = self.store.get();
        if !settings.limit_enabled {
            return Vec::new();
        }
        Self::enforce_limit_bytes(library, settings.limit_gb as u64 * GB, settings.recycle)
    }

    pub fn enforce_limit_bytes(library: &Library, limit: u64, recycle: bool) -> Vec<String> {
        let mut removed = Vec::new();
        while library.total_size() > limit {
            let Some(path) = library.oldest_removable() else { break };
            match win::delete_file(Path::new(&path), recycle) {
                Ok(()) => {
                    library.remove(&path);
                    removed.push(path);
                }
                Err(e) => {
                    crate::log::line(&format!("storage limit: {e}"));
                    break;
                }
            }
        }
        removed
    }

    /// Free bytes on the capture drive and the configured minimum.
    pub fn free_space(&self) -> (Option<u64>, u64) {
        let folder = self.folder();
        let probe = if folder.exists() { folder } else { folder.parent().map(Path::to_path_buf).unwrap_or(folder) };
        (win::free_bytes(&probe), self.store.get().min_free_gb as u64 * GB)
    }

    /// Whether recording should pause (`Some(true)`), resume (`Some(false)`) or
    /// stay as it is (`None`), with hysteresis so it does not flap.
    pub fn disk_transition(&self) -> Option<bool> {
        let (Some(free), min) = self.free_space() else { return None };
        let paused = self.paused.load(Ordering::SeqCst);
        if !paused && free < min {
            self.paused.store(true, Ordering::SeqCst);
            Some(true)
        } else if paused && free >= min + RESUME_MARGIN {
            self.paused.store(false, Ordering::SeqCst);
            Some(false)
        } else {
            None
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn state(&self, library: &Library) -> Value {
        let (free, min) = self.free_space();
        json!({
            "folder": self.folder(),
            "usedBytes": library.total_size(),
            "freeBytes": free,
            "minFreeBytes": min,
            "pausedForDisk": self.is_paused(),
        })
    }

    /// Moves every indexed item from the old capture folder to the new one.
    pub fn move_items(library: &Library, from: &Path, to: &Path) -> (usize, Vec<String>) {
        let (mut moved, mut failed) = (0, Vec::new());
        for item in library.list(None, None).unwrap_or_default() {
            let Some(path) = item["path"].as_str().map(PathBuf::from) else { continue };
            let Ok(rel) = path.strip_prefix(from) else { continue };
            let target = to.join(rel);
            if let Some(dir) = target.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match move_file(&path, &target) {
                Ok(()) => {
                    library.rename(&path.to_string_lossy(), &target.to_string_lossy());
                    moved += 1;
                }
                Err(e) => failed.push(format!("{}: {e}", path.display())),
            }
        }
        (moved, failed)
    }
}
