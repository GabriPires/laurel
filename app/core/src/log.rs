//! The logs' files and the .zip to send (Laurel's logs folder holds `app.log`,
//! `engine.log` and their older copies): a log file that moves to `<name>.old.log`
//! when it passes its size limit and, for `app.log`, aside at each start, keeping
//! the previous sessions; the Windows user name hidden in the texts sent; the .zip
//! itself; and the Windows name `info.txt` gives (A6). The app keeps the log of its
//! own lines, with the local time.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MAX_BYTES: u64 = 5 << 20;
/// Previous sessions of `app.log` kept as `app.1.log` to `app.<n>.log`.
pub const APP_SESSIONS: usize = 4;

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

/// `text` with the Windows user name in paths replaced by `{user}`: after
/// "users" and a slash, a backslash or two, in any case, unless the name goes on
/// with a letter or a digit (another user's name that starts the same).
pub fn mask_user(text: &str, user: &str) -> String {
    if user.len() < 2 {
        return text.to_string();
    }
    let folders = ["users\\\\", "users\\", "users/"];
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let found = if c.eq_ignore_ascii_case(&'u') {
            folders.iter().find_map(|folder| {
                let folder = starts_without_case(&text[at..], folder)?;
                Some((folder, at + folder + starts_without_case(&text[at + folder..], user)?))
            })
        } else {
            None
        };
        match found {
            Some((folder, end)) if !text[end..].chars().next().is_some_and(char::is_alphanumeric) => {
                out.push_str(&text[at..at + folder]);
                out.push_str("{user}");
                at = end;
            }
            Some((_, end)) => {
                out.push_str(&text[at..end]);
                at = end;
            }
            None => {
                out.push(c);
                at += c.len_utf8();
            }
        }
    }
    out
}

/// How many bytes of `text` its start takes when it is `prefix` in any case,
/// compared letter by letter (lowercasing a whole text can change its length).
fn starts_without_case(text: &str, prefix: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for p in prefix.chars() {
        let (_, c) = chars.next()?;
        if !c.to_lowercase().eq(p.to_lowercase()) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// Writes `<dest>` as a .zip with every file of the logs folder `dir` under
/// `logs/` and the `extra` files, each text with the Windows user name `user`
/// hidden (mask_user); `dos_time` is the (time, date) the entries are stamped with.
pub fn export(dir: &Path, dest: &Path, extra: &[(&str, String)], user: &str, dos_time: (u16, u16)) -> Result<usize, String> {
    let mut entries: Vec<(String, String)> = extra.iter().map(|(name, text)| (name.to_string(), mask_user(text, user))).collect();
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    names.sort();
    for path in names {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        entries.push((format!("logs/{}", path.file_name().unwrap_or_default().to_string_lossy()), mask_user(&String::from_utf8_lossy(&bytes), user)));
    }
    write_zip(dest, &entries, dos_time)?;
    Ok(entries.len())
}

/// A .zip of deflated UTF-8 text files (ZIP 2.0, no ZIP64: logs stay small).
fn write_zip(dest: &Path, entries: &[(String, String)], (time, date): (u16, u16)) -> Result<(), String> {
    use flate2::{write::DeflateEncoder, Compression};
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

/// The registry's ProductName still says Windows 10 on Windows 11, whose builds
/// start at 22000 (stability tests, finding A6).
pub fn windows_name(product: &str, build: &str) -> String {
    match product.strip_prefix("Windows 10") {
        Some(edition) if build.parse::<u32>().is_ok_and(|b| b >= 22000) => format!("Windows 11{edition}"),
        _ => product.to_string(),
    }
}
