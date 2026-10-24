//! Local sharing actions for library items (spec share.local): copy the file to
//! the clipboard (paste into Discord or Explorer), copy its path, show it in
//! Explorer and open it in the default player.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::Path;

#[link(name = "user32", kind = "raw-dylib")]
extern "system" {
    fn OpenClipboard(owner: isize) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, data: isize) -> isize;
    fn CloseClipboard() -> i32;
    fn RegisterClipboardFormatW(name: *const u16) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalLock(mem: isize) -> *mut c_void;
    fn GlobalUnlock(mem: isize) -> i32;
    fn GlobalFree(mem: isize) -> isize;
}

#[link(name = "shell32", kind = "raw-dylib")]
extern "system" {
    fn ShellExecuteW(hwnd: isize, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
}

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;
const DROPEFFECT_COPY: u32 = 1;
const SW_SHOWNORMAL: i32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows paths with backslashes (the engine reports clips with forward slashes).
pub fn windows_path(path: &str) -> String {
    path.replace('/', "\\")
}

/// Moves `bytes` into a movable global block, as the clipboard requires.
unsafe fn global(bytes: &[u8]) -> Result<isize, String> {
    let mem = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len());
    if mem == 0 {
        return Err("GlobalAlloc failed".into());
    }
    let p = GlobalLock(mem);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
    GlobalUnlock(mem);
    Ok(mem)
}

/// Opens the clipboard, replaces its content with `items` (format, bytes) and
/// closes it. Blocks the clipboard owns are not freed by us once set.
fn set_clipboard(items: &[(u32, Vec<u8>)]) -> Result<(), String> {
    unsafe {
        if OpenClipboard(0) == 0 {
            return Err("the clipboard is in use by another app".into());
        }
        EmptyClipboard();
        let mut result = Ok(());
        for (format, bytes) in items {
            match global(bytes) {
                Ok(mem) => {
                    if SetClipboardData(*format, mem) == 0 {
                        GlobalFree(mem);
                        result = Err("SetClipboardData failed".into());
                    }
                }
                Err(e) => result = Err(e),
            }
        }
        CloseClipboard();
        result
    }
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let bytes: Vec<u8> = wide(text).iter().flat_map(|c| c.to_le_bytes()).collect();
    set_clipboard(&[(CF_UNICODETEXT, bytes)])
}

/// Puts the file on the clipboard as a file list (CF_HDROP) marked as a copy, so
/// pasting in Discord, a chat or Explorer attaches or copies the file itself.
pub fn copy_file(path: &str) -> Result<(), String> {
    let path = windows_path(path);
    if !Path::new(&path).exists() {
        return Err(format!("file not found: {path}"));
    }
    // DROPFILES: pFiles (offset of the list), pt.x, pt.y, fNC, fWide; then the
    // NUL-separated wide path list, ended by an extra NUL.
    let mut drop: Vec<u8> = Vec::new();
    drop.extend_from_slice(&20u32.to_le_bytes());
    drop.extend_from_slice(&[0u8; 12]);
    drop.extend_from_slice(&1u32.to_le_bytes());
    for c in wide(&path).into_iter().chain(std::iter::once(0)) {
        drop.extend_from_slice(&c.to_le_bytes());
    }
    let effect_format = unsafe { RegisterClipboardFormatW(wide("Preferred DropEffect").as_ptr()) };
    set_clipboard(&[(CF_HDROP, drop), (effect_format, DROPEFFECT_COPY.to_le_bytes().to_vec())])
}

/// Opens an Explorer window with the file selected.
pub fn show_in_folder(path: &str) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", windows_path(path)))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens the file with its default app (the video player for clips), or a folder in
/// Explorer.
pub fn open(path: &str) -> Result<(), String> {
    shell_open(&windows_path(path))
}

/// Opens a web address in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    shell_open(url)
}

fn shell_open(target: &str) -> Result<(), String> {
    let (op, file) = (wide("open"), wide(target));
    let code = unsafe { ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    if code > 32 {
        Ok(())
    } else {
        Err(format!("could not open the file (ShellExecute {code})"))
    }
}
