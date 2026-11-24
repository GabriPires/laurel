//! Plain-text logs in `<work-dir>/logs`: the app's `app.log` and the engine's
//! `engine.log`, every line with the local time. Each one moves to
//! `<name>.old.log` when it passes `MAX_BYTES`, replacing the older copy. `app.log`
//! also moves aside at each start: the previous sessions are `app.1.log` (the last
//! one) to `app.4.log`, for testers who send their logs after a few sessions.
//! `export` puts all of them in one .zip to send, with the Windows user name hidden.
//! The files, the masking and the .zip are laurel_app_core::log.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub use laurel_app_core::log::{Rotating, MAX_BYTES};
use laurel_app_core::log::APP_SESSIONS;

static LOG: OnceLock<Mutex<Rotating>> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn init(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let _ = LOG.set(Mutex::new(Rotating::open(&dir.join("app.log"), APP_SESSIONS, MAX_BYTES)));
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

/// Writes `<dest>` as a .zip with every file of the logs folder under `logs/` and
/// the `extra` files, each text with the Windows user name hidden.
pub fn export(dest: &Path, extra: &[(&str, String)]) -> Result<usize, String> {
    let dir = dir().ok_or("the logs folder is not set")?;
    laurel_app_core::log::export(dir, dest, extra, &std::env::var("USERNAME").unwrap_or_default(), crate::win::dos_time())
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}
pub(crate) use log;
