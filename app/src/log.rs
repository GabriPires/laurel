//! Plain-text logs in `<work-dir>/logs`: the app's `app.log` and the engine's
//! `engine.log`. Each one moves to `<name>.old.log` when it passes `MAX_BYTES`,
//! replacing the older copy, so a log never takes more than twice that on disk;
//! `app.log` also moves aside at each start, so the previous session is kept.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const MAX_BYTES: u64 = 5 << 20;

static LOG: OnceLock<Mutex<Rotating>> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// A log file that rotates by size, keeping one older copy.
pub struct Rotating {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    max: u64,
}

impl Rotating {
    /// Appends to `path`; with `fresh`, the current file first becomes the old copy.
    pub fn open(path: &Path, fresh: bool, max: u64) -> Rotating {
        let mut log = Rotating { path: path.to_path_buf(), file: None, size: 0, max };
        if fresh {
            log.rotate();
        }
        log.reopen();
        log
    }

    /// `<dir>/<name>.old.log` for `<dir>/<name>.log`.
    pub fn old_path(path: &Path) -> PathBuf {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        path.with_file_name(format!("{stem}.old.log"))
    }

    fn reopen(&mut self) {
        self.file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).ok();
        self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
    }

    fn rotate(&mut self) {
        self.file = None;
        if self.path.exists() {
            let _ = std::fs::rename(&self.path, Rotating::old_path(&self.path));
        }
    }

    pub fn line(&mut self, text: &str) {
        if self.size >= self.max {
            self.rotate();
            self.reopen();
        }
        if let Some(file) = self.file.as_mut() {
            if writeln!(file, "{text}").is_ok() {
                self.size += text.len() as u64 + 1;
            }
        }
    }
}

pub fn init(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let _ = LOG.set(Mutex::new(Rotating::open(&dir.join("app.log"), true, MAX_BYTES)));
    let _ = DIR.set(dir.to_path_buf());
    Ok(())
}

/// The logs folder (app.log, engine.log and their old copies), once `init` ran.
pub fn dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

pub fn line(msg: &str) {
    if let Some(log) = LOG.get() {
        log.lock().unwrap().line(&format!("[{}] {msg}", crate::win::clock()));
    }
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}
pub(crate) use log;
