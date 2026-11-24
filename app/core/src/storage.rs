//! Capture folder rules (specs storage.location, storage.limits): how clips and
//! screenshots are named, free names for new files, the rename that waits for a
//! file another process still holds, the folder size limit that removes the oldest
//! items, moving the items to a new capture folder, and the free-space guard that
//! pauses recording when the drive is almost full. The app gives the settings, the
//! clock, the drive's free space and its log.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::library::Library;

pub const GB: u64 = 1024 * 1024 * 1024;
/// Recording resumes once free space is this much above the minimum.
pub const RESUME_MARGIN: u64 = GB;

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
pub fn unique(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{name}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// Renames a clip the engine just saved, trying again for a while when another
/// process still has it open (os error 32, a sharing violation): a moment after a
/// save, something sometimes does (stability tests, finding A11). A rename that
/// needed more than one try leaves a line in `log`.
pub fn rename_when_free(from: &Path, to: &Path, log: &dyn Fn(&str)) -> std::io::Result<()> {
    const SHARING_VIOLATION: i32 = 32;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut tries = 1;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                tries += 1;
            }
            Ok(()) if tries > 1 => {
                log(&format!("renamed {} on try {tries}: it was in use", from.display()));
                return Ok(());
            }
            result => return result,
        }
    }
}

/// The date and time of the save, from the engine's name for the file ("Laurel
/// <date> <time>", maybe with " (2)" after it), so the final name keeps the moment
/// the engine chose even when the save ends a second later; None for another name.
pub fn saved_at(stem: &str) -> Option<(String, String)> {
    let mut parts = stem.split(' ');
    let digits = |s: &str, dashes: &[usize]| s.char_indices().all(|(i, c)| if dashes.contains(&i) { c == '-' } else { c.is_ascii_digit() });
    if let (Some("Laurel"), Some(date), Some(time)) = (parts.next(), parts.next(), parts.next()) {
        if date.len() == 10 && digits(date, &[4, 7]) && time.len() == 8 && digits(time, &[2, 5]) {
            return Some((date.to_string(), time.to_string()));
        }
    }
    None
}

/// Moves a file, copying it when the destination is on another drive.
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to).or_else(|_| {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)
    })
}

/// Creates the folder if needed and proves it is writable with a probe file.
pub fn check_writable(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    let probe = folder.join(".laurel-write-test");
    std::fs::write(&probe, b"laurel").map_err(|e| format!("cannot write to {}: {e}", folder.display()))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

/// Renames a file the engine just saved after the naming `pattern` (fields
/// {game}, {date}, {time}), with ` - <extra>` after it when given (an automatic
/// clip's achievement, autoclip.steam-achievements), and moves the engine's
/// thumbnail along. The date and time are the engine's (see saved_at), else `now`.
/// Returns the new path; on failure the file keeps its engine name.
pub fn apply_naming(saved: &str, pattern: &str, game: Option<&str>, extra: Option<&str>, thumbs_dir: &Path, now: &dyn Fn() -> (String, String), log: &dyn Fn(&str)) -> String {
    let path = PathBuf::from(crate::library::native_path(saved));
    let (Some(dir), Some(ext), Some(old_stem)) = (path.parent(), path.extension(), path.file_stem()) else {
        return saved.to_string();
    };
    let (date, time) = saved_at(&old_stem.to_string_lossy()).unwrap_or_else(now);
    let mut name = pattern.replace("{game}", game.unwrap_or("Laurel")).replace("{date}", &date).replace("{time}", &time);
    if let Some(extra) = extra.filter(|e| !e.trim().is_empty()) {
        name = format!("{name} - {extra}");
    }
    let ext = ext.to_string_lossy();
    // The engine already names files "Laurel <date> <time>"; the same name stays
    // as it is instead of becoming "… (2)".
    let wanted = format!("{}.{ext}", sanitize(&name));
    if path.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&wanted)) {
        return path.to_string_lossy().into_owned();
    }
    let target = unique(dir, &sanitize(&name), &ext);
    if let Err(e) = rename_when_free(&path, &target, log) {
        log(&format!("could not rename {}: {e}", path.display()));
        return path.to_string_lossy().into_owned();
    }
    let old_thumb = thumbs_dir.join(format!("{}.png", old_stem.to_string_lossy()));
    if let Some(new_stem) = target.file_stem() {
        let _ = std::fs::rename(&old_thumb, thumbs_dir.join(format!("{}.png", new_stem.to_string_lossy())));
    }
    target.to_string_lossy().into_owned()
}

/// Removes items until the capture folder fits `limit` bytes, Recently deleted
/// first, then the oldest items not marked to keep; returns the removed paths. A
/// removal that fails stops it, with a line in `log`.
pub fn enforce_limit(library: &Library, limit: u64, recycle: bool, log: &dyn Fn(&str)) -> Vec<String> {
    let mut removed = Vec::new();
    while library.total_size() > limit {
        let Some(path) = library.oldest_removable() else { break };
        match library.purge(&path, recycle) {
            Ok(()) => removed.push(path),
            Err(e) => {
                log(&format!("storage limit: {e}"));
                break;
            }
        }
    }
    removed
}

/// Moves every indexed item from the old capture folder to the new one, those in
/// Recently deleted included, with the place they would be restored to; returns how
/// many moved and why the others did not.
pub fn move_items(library: &Library, from: &Path, to: &Path) -> (usize, Vec<String>) {
    let (mut moved, mut failed) = (0, Vec::new());
    let mut items = library.list(None, None).unwrap_or_default();
    items.extend(library.list_deleted().unwrap_or_default());
    for item in items {
        let Some(path) = item["path"].as_str().map(PathBuf::from) else { continue };
        let Ok(rel) = path.strip_prefix(from) else { continue };
        let target = to.join(rel);
        if let Some(dir) = target.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let original = item["originalPath"].as_str().and_then(|o| Path::new(o).strip_prefix(from).ok()).map(|rel| to.join(rel).to_string_lossy().into_owned());
        match move_file(&path, &target) {
            Ok(()) => {
                library.rename(&path.to_string_lossy(), &target.to_string_lossy(), original.as_deref());
                moved += 1;
            }
            Err(e) => failed.push(format!("{}: {e}", path.display())),
        }
    }
    (moved, failed)
}

/// The free-space guard (spec storage.limits): recording pauses below the minimum
/// free space and resumes only RESUME_MARGIN above it, so that it does not flap.
#[derive(Default)]
pub struct DiskGuard {
    paused: AtomicBool,
}

impl DiskGuard {
    /// Whether recording should pause (`Some(true)`), resume (`Some(false)`) or
    /// stay as it is (`None`), with `free` bytes free and a minimum of `min`.
    pub fn transition(&self, free: u64, min: u64) -> Option<bool> {
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
}
