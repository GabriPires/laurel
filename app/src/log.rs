//! Plain-text app log (`<work-dir>/logs/app.log`, recreated at each start).

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

pub fn init(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let file = File::create(dir.join("app.log"))?;
    let _ = LOG.set(Mutex::new(file));
    Ok(())
}

pub fn line(msg: &str) {
    if let Some(file) = LOG.get() {
        let mut file = file.lock().unwrap();
        let _ = writeln!(file, "[{}] {msg}", crate::win::clock());
    }
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}
pub(crate) use log;
