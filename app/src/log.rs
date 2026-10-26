//! Plain-text logs in `<work-dir>/logs`: the app's `app.log` and the engine's
//! `engine.log`, every line with the local time. Each one moves to
//! `<name>.old.log` when it passes `MAX_BYTES`, replacing the older copy. `app.log`
//! also moves aside at each start: the previous sessions are `app.1.log` (the last
//! one) to `app.4.log`, for testers who send their logs after a few sessions.
//! `export` puts all of them in one .zip to send, with the Windows user name hidden.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const MAX_BYTES: u64 = 5 << 20;
/// Previous sessions of `app.log` kept as `app.1.log` to `app.<n>.log`.
pub const APP_SESSIONS: usize = 4;

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
    /// Appends to `path`; with `sessions` above 0, the current file first becomes
    /// `<name>.1.log`, the older ones move up, and the one past `sessions` goes.
    pub fn open(path: &Path, sessions: usize, max: u64) -> Rotating {
        if sessions > 0 && path.exists() {
            for i in (1..sessions).rev() {
                let _ = std::fs::rename(Rotating::session_path(path, i), Rotating::session_path(path, i + 1));
            }
            let _ = std::fs::rename(path, Rotating::session_path(path, 1));
        }
        let mut log = Rotating { path: path.to_path_buf(), file: None, size: 0, max };
        log.reopen();
        log
    }

    /// `<dir>/<name>.<i>.log` for `<dir>/<name>.log`: the i-th previous session.
    pub fn session_path(path: &Path, i: usize) -> PathBuf {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        path.with_file_name(format!("{stem}.{i}.log"))
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

/// `text` with the Windows user name in paths replaced by `{user}`.
pub fn mask_user(text: &str, user: &str) -> String {
    if user.len() < 2 {
        return text.to_string();
    }
    let mut out = text.to_string();
    for sep in ["\\\\", "\\", "/"] {
        let needle = format!("users{sep}{}", user.to_lowercase());
        let lower = out.to_lowercase();
        // Lowercasing keeps the byte offsets of ASCII paths; other names are left alone.
        if lower.len() != out.len() {
            continue;
        }
        let mut masked = String::with_capacity(out.len());
        let mut at = 0;
        while let Some(found) = lower[at..].find(&needle) {
            let start = at + found;
            let end = start + needle.len();
            let next = out[end..].chars().next();
            masked.push_str(&out[at..start + "users".len() + sep.len()]);
            if next.is_some_and(|c| c.is_alphanumeric()) {
                masked.push_str(&out[start + "users".len() + sep.len()..end]);
            } else {
                masked.push_str("{user}");
            }
            at = end;
        }
        masked.push_str(&out[at..]);
        out = masked;
    }
    out
}

/// Writes `<dest>` as a .zip with every file of the logs folder under `logs/` and
/// the `extra` files, each text with the Windows user name hidden (mask_user).
pub fn export(dest: &Path, extra: &[(&str, String)]) -> Result<usize, String> {
    let dir = dir().ok_or("the logs folder is not set")?;
    let user = std::env::var("USERNAME").unwrap_or_default();
    let mut entries: Vec<(String, String)> = extra.iter().map(|(name, text)| (name.to_string(), mask_user(text, &user))).collect();
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    names.sort();
    for path in names {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        entries.push((format!("logs/{}", path.file_name().unwrap_or_default().to_string_lossy()), mask_user(&String::from_utf8_lossy(&bytes), &user)));
    }
    write_zip(dest, &entries)?;
    Ok(entries.len())
}

/// A .zip of deflated UTF-8 text files (ZIP 2.0, no ZIP64: logs stay small).
fn write_zip(dest: &Path, entries: &[(String, String)]) -> Result<(), String> {
    use flate2::{write::DeflateEncoder, Compression};
    let (time, date) = crate::win::dos_time();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, text) in entries {
        let data = text.as_bytes();
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).map_err(|e| e.to_string())?;
        let packed = encoder.finish().map_err(|e| e.to_string())?;
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        // Version 2.0, UTF-8 names (bit 11), deflate.
        let common = |b: &mut Vec<u8>| {
            for v in [20u16, 0x0800, 8, time, date] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for v in [crc, packed.len() as u32, data.len() as u32] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&(name.len() as u16).to_le_bytes());
            b.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        common(&mut central);
        for v in [0u16, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let (cd_offset, cd_size, n) = (out.len() as u32, central.len() as u32, entries.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, n, n] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    std::fs::write(dest, out).map_err(|e| e.to_string())
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}
pub(crate) use log;
