//! The Win32 calls the app needs, declared by hand and linked with `raw-dylib`
//! (no import libraries are needed when cross-compiling from WSL). Window handles
//! are passed as `isize`.

use std::ffi::c_void;

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

/// FOLDERID_Videos.
const FOLDERID_VIDEOS: Guid = Guid(0x18989B1D, 0x99B5, 0x455B, [0x84, 0x1C, 0xAB, 0x7C, 0x74, 0xE4, 0xDD, 0xFC]);

/// SHFILEOPSTRUCTW (natural alignment on 64-bit Windows).
#[repr(C)]
struct ShFileOp {
    hwnd: isize,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    any_aborted: i32,
    name_mappings: *mut c_void,
    progress_title: *const u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn GetLocalTime(time: *mut SystemTime);
    fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetCurrentProcess() -> isize;
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    fn GetLastError() -> u32;
    fn QueryFullProcessImageNameW(process: isize, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "shell32", kind = "raw-dylib")]
extern "system" {
    fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: isize, path: *mut *mut u16) -> i32;
    fn SHFileOperationW(op: *mut ShFileOp) -> i32;
    fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
}

#[link(name = "ole32", kind = "raw-dylib")]
extern "system" {
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "advapi32", kind = "raw-dylib")]
extern "system" {
    fn RegSetKeyValueW(key: isize, subkey: *const u16, name: *const u16, kind: u32, data: *const c_void, len: u32) -> i32;
    fn RegDeleteKeyValueW(key: isize, subkey: *const u16, name: *const u16) -> i32;
    fn RegGetValueW(key: isize, subkey: *const u16, name: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
    fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
    fn GetTokenInformation(token: isize, class: u32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetForegroundWindow() -> isize;
    fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
    fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn SetWindowPos(hwnd: isize, insert_after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn IsWindowVisible(hwnd: isize) -> i32;
    fn IsIconic(hwnd: isize) -> i32;
    fn EnumWindows(callback: extern "system" fn(isize, isize) -> i32, param: isize) -> i32;
    fn GetWindow(hwnd: isize, cmd: u32) -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn GetWindowTextW(hwnd: isize, text: *mut u16, max: i32) -> i32;
    fn GetClassNameW(hwnd: isize, name: *mut u16, max: i32) -> i32;
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(ex_style: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize, instance: isize, param: *mut c_void) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
    fn GetRawInputData(input: isize, command: u32, data: *mut c_void, size: *mut u32, header_size: u32) -> u32;
    fn GetAsyncKeyState(vk: i32) -> i16;
    fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
    fn SendInput(count: u32, inputs: *const KeyInput, size: i32) -> u32;
    fn SetWindowDisplayAffinity(hwnd: isize, affinity: u32) -> i32;
    fn GetWindowDisplayAffinity(hwnd: isize, affinity: *mut u32) -> i32;
}

#[link(name = "dwmapi", kind = "raw-dylib")]
extern "system" {
    fn DwmGetWindowAttribute(hwnd: isize, attribute: u32, value: *mut c_void, size: u32) -> i32;
}

#[link(name = "shcore", kind = "raw-dylib")]
extern "system" {
    fn GetDpiForMonitor(monitor: isize, kind: u32, dpi_x: *mut u32, dpi_y: *mut u32) -> i32;
}

#[link(name = "winmm", kind = "raw-dylib")]
extern "system" {
    fn PlaySoundW(sound: *const c_void, module: isize, flags: u32) -> i32;
}

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;
const GWL_EXSTYLE: i32 = -20;
pub const WS_EX_TOPMOST: isize = 0x0000_0008;
pub const WS_EX_TRANSPARENT: isize = 0x0000_0020;
pub const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
pub const WS_EX_APPWINDOW: isize = 0x0004_0000;
pub const WS_EX_LAYERED: isize = 0x0008_0000;
pub const WS_EX_NOACTIVATE: isize = 0x0800_0000;
const HWND_TOPMOST: isize = -1;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SW_HIDE: i32 = 0;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
const DWMWA_CLOAKED: u32 = 14;
const GW_OWNER: u32 = 4;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const MDT_EFFECTIVE_DPI: u32 = 0;
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
const SND_ASYNC: u32 = 0x0001;
const SND_NODEFAULT: u32 = 0x0002;
const SND_MEMORY: u32 = 0x0004;

pub const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
pub const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const REG_SZ: u32 = 1;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
const SM_CXSMICON: i32 = 49;
const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const FO_DELETE: u32 = 3;
const FOF_SILENT: u16 = 0x0004;
const FOF_NOCONFIRMATION: u16 = 0x0010;
const FOF_ALLOWUNDO: u16 = 0x0040;
const FOF_NOERRORUI: u16 = 0x0400;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Local date and time as (`YYYY-MM-DD`, `hh-mm-ss`), for file names.
pub fn date_time() -> (String, String) {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    (format!("{:04}-{:02}-{:02}", t.year, t.month, t.day), format!("{:02}-{:02}-{:02}", t.hour, t.minute, t.second))
}

/// Bytes free for this user on the volume that holds `dir`.
pub fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    let path = wide(&dir.to_string_lossy());
    let (mut free, mut total, mut total_free) = (0, 0, 0);
    (unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, &mut total_free) } != 0).then_some(free)
}

/// The user's Videos folder (it can be redirected, for example to OneDrive).
pub fn videos_folder() -> Option<std::path::PathBuf> {
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_VIDEOS, 0, 0, &mut p) != 0 || p.is_null() {
            return None;
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        let path = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p.cast());
        Some(path.into())
    }
}

/// Deletes a file, to the Recycle Bin when `recycle` is set, without any dialog.
pub fn delete_file(path: &std::path::Path, recycle: bool) -> Result<(), String> {
    if !recycle {
        return std::fs::remove_file(path).map_err(|e| e.to_string());
    }
    let mut from = wide(&path.to_string_lossy());
    from.push(0); // the list ends with a second NUL
    let mut op = ShFileOp {
        hwnd: 0,
        func: FO_DELETE,
        from: from.as_ptr(),
        to: std::ptr::null(),
        flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
        any_aborted: 0,
        name_mappings: std::ptr::null_mut(),
        progress_title: std::ptr::null(),
    };
    match unsafe { SHFileOperationW(&mut op) } {
        0 if op.any_aborted == 0 => Ok(()),
        code => Err(format!("could not move {} to the Recycle Bin (code {code})", path.display())),
    }
}

/// A REG_SZ value, if present.
pub fn reg_string(root: isize, subkey: &str, name: &str) -> Option<String> {
    let (key, value) = (wide(subkey), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let rc = unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    (rc == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

/// A REG_DWORD value, if present.
pub fn reg_dword(root: isize, subkey: &str, name: &str) -> Option<u32> {
    let (key, value) = (wide(subkey), wide(name));
    let mut data = 0u32;
    let mut len = 4u32;
    let rc = unsafe { RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut len) };
    (rc == 0).then_some(data)
}

/// Whether the taskbar uses the light theme (Settings → Personalization → Colors).
pub fn taskbar_light() -> bool {
    reg_dword(HKEY_CURRENT_USER, PERSONALIZE_KEY, "SystemUsesLightTheme") == Some(1)
}

/// The size of a small icon, as the tray shows it, at the system's DPI.
pub fn small_icon_size() -> u32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16) as u32
}

/// The command line registered to start Laurel with Windows, if any (spec app.startup).
pub fn autostart_command(name: &str) -> Option<String> {
    reg_string(HKEY_CURRENT_USER, RUN_KEY, name)
}

/// Registers (or removes) a command to run at sign-in, for this user only.
pub fn set_autostart(name: &str, command: Option<&str>) -> Result<(), String> {
    let (key, value) = (wide(RUN_KEY), wide(name));
    let rc = match command {
        Some(cmd) => {
            let data = wide(cmd);
            unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) }
        }
        None => match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
            2 => 0, // ERROR_FILE_NOT_FOUND: already off
            rc => rc,
        },
    };
    if rc == 0 { Ok(()) } else { Err(format!("registry error {rc}")) }
}

/// Local time as `hh:mm:ss.mmm`, for log lines.
pub fn clock() -> String {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}.{:03}", t.hour, t.minute, t.second, t.millis)
}

/// Size of the primary monitor in physical pixels (the app is per-monitor DPI aware
/// through the Tauri manifest).
pub fn primary_monitor_size() -> (u32, u32) {
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    (w.max(1) as u32, h.max(1) as u32)
}

pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow() }
}

pub fn window_rect(hwnd: isize) -> Rect {
    let mut r = Rect::default();
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

/// Where in-game notices go: the visible part of the foreground window (its frame
/// without the invisible resize borders) on its monitor, or the whole monitor when
/// the window is minimized or off screen. Also returns that monitor's scale factor.
pub fn foreground_anchor() -> (Rect, f64) {
    unsafe {
        let hwnd = GetForegroundWindow();
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MonitorInfo { size: std::mem::size_of::<MonitorInfo>() as u32, ..Default::default() };
        GetMonitorInfoW(monitor, &mut info);
        let mut frame = Rect::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut frame as *mut Rect).cast(), std::mem::size_of::<Rect>() as u32) != 0 {
            GetWindowRect(hwnd, &mut frame);
        }
        let m = info.monitor;
        let visible = Rect { left: frame.left.max(m.left), top: frame.top.max(m.top), right: frame.right.min(m.right), bottom: frame.bottom.min(m.bottom) };
        let anchor = if visible.right - visible.left < 200 || visible.bottom - visible.top < 150 { m } else { visible };
        let (mut dpi_x, mut dpi_y) = (96, 96);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        (anchor, dpi_x as f64 / 96.0)
    }
}

/// Makes a window an overlay: no activation, no taskbar button or Alt+Tab entry
/// (the app-window style Tauri sets would force one), topmost, and (unless
/// `capturable`) excluded from every screen capture API.
pub fn make_overlay(hwnd: isize, capturable: bool) -> Result<(), String> {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_APPWINDOW) | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST);
        if !capturable && SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
            return Err("SetWindowDisplayAffinity failed".into());
        }
    }
    Ok(())
}

pub fn ex_style(hwnd: isize) -> isize {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
}

pub fn display_affinity(hwnd: isize) -> u32 {
    let mut affinity = 0;
    unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) };
    affinity
}

/// Shows a window at a position without activating it, above other topmost windows.
pub fn show_no_activate(hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    unsafe { SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW) };
}

pub fn hide(hwnd: isize) {
    unsafe { ShowWindow(hwnd, SW_HIDE) };
}

pub fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(hwnd) != 0 }
}

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: extern "system" fn(isize, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: isize,
    icon: isize,
    cursor: isize,
    background: isize,
    menu_name: *const u16,
    class_name: *const u16,
    icon_sm: isize,
}

#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    x: i32,
    y: i32,
    private: u32,
}

#[repr(C)]
struct RawInputDevice {
    usage_page: u16,
    usage: u16,
    flags: u32,
    target: isize,
}

/// INPUT with a KEYBDINPUT (40 bytes on 64-bit Windows, the size of the union's
/// largest member, MOUSEINPUT).
#[repr(C)]
struct KeyInput {
    kind: u32,
    _align: u32,
    vk: u16,
    scan: u16,
    flags: u32,
    time: u32,
    extra: usize,
    _pad: [u8; 8],
}

const HWND_MESSAGE: isize = -3;
const WM_INPUT: u32 = 0x00FF;
const WM_APP_RAW_INPUT: u32 = 0x8000 + 1;
const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
const RIDEV_REMOVE: u32 = 0x0000_0001;
const RIDEV_INPUTSINK: u32 = 0x0000_0100;
const RI_KEY_BREAK: u16 = 1;
const RI_KEY_E0: u16 = 2;
/// RI_MOUSE_BUTTON_1_DOWN .. RI_MOUSE_BUTTON_5_DOWN.
const MOUSE_DOWN_FLAGS: [(u16, u8); 5] = [(0x0001, 1), (0x0004, 2), (0x0010, 3), (0x0040, 4), (0x0100, 5)];
const MAPVK_VK_TO_VSC: u32 = 0;
const MAPVK_VK_TO_CHAR: u32 = 2;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 0x0002;

/// A key or mouse button event from Raw Input.
pub enum RawInput {
    Key { vk: u16, down: bool, e0: bool },
    MouseDown(u8),
}

static RAW_HANDLER: std::sync::OnceLock<fn(RawInput)> = std::sync::OnceLock::new();

fn register_devices(hwnd: isize, mouse: bool) -> Result<(), String> {
    let size = std::mem::size_of::<RawInputDevice>() as u32;
    let keyboard = RawInputDevice { usage_page: 1, usage: 6, flags: RIDEV_INPUTSINK, target: hwnd };
    if unsafe { RegisterRawInputDevices(&keyboard, 1, size) } == 0 {
        return Err("could not register for keyboard raw input".into());
    }
    let mouse_device = if mouse {
        RawInputDevice { usage_page: 1, usage: 2, flags: RIDEV_INPUTSINK, target: hwnd }
    } else {
        RawInputDevice { usage_page: 1, usage: 2, flags: RIDEV_REMOVE, target: 0 }
    };
    // Removing a mouse registration that does not exist fails harmlessly.
    if unsafe { RegisterRawInputDevices(&mouse_device, 1, size) } == 0 && mouse {
        return Err("could not register for mouse raw input".into());
    }
    Ok(())
}

extern "system" fn raw_input_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_APP_RAW_INPUT => {
            if let Err(e) = register_devices(hwnd, wparam != 0) {
                crate::log::log!("hotkeys: {e}");
            }
            0
        }
        WM_INPUT => {
            let mut buf = [0u64; 8];
            let mut size = std::mem::size_of_val(&buf) as u32;
            let header = 24; // RAWINPUTHEADER on 64-bit Windows
            let n = unsafe { GetRawInputData(lparam, RID_INPUT, buf.as_mut_ptr().cast(), &mut size, header) };
            if n != u32::MAX && n as usize >= header as usize + 16 {
                let bytes: &[u8] = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), n as usize) };
                let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
                let kind = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let event = match kind {
                    // RAWKEYBOARD: MakeCode, Flags, Reserved, VKey, ...
                    RIM_TYPEKEYBOARD => {
                        let flags = u16_at(24 + 2);
                        Some(RawInput::Key { vk: u16_at(24 + 6), down: flags & RI_KEY_BREAK == 0, e0: flags & RI_KEY_E0 != 0 })
                    }
                    // RAWMOUSE: usFlags, padding, usButtonFlags, usButtonData, ...
                    RIM_TYPEMOUSE => {
                        let buttons = u16_at(24 + 4);
                        MOUSE_DOWN_FLAGS.iter().find(|(flag, _)| buttons & flag != 0).map(|(_, n)| RawInput::MouseDown(*n))
                    }
                    _ => None,
                };
                if let (Some(event), Some(handler)) = (event, RAW_HANDLER.get()) {
                    handler(event);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Creates the hidden message-only window that receives Raw Input on the calling
/// thread, registered for the keyboard. Run `run_message_loop` on the same thread.
pub fn raw_input_window(handler: fn(RawInput)) -> Result<isize, String> {
    RAW_HANDLER.set(handler).map_err(|_| "raw input already started".to_string())?;
    let class = wide("LaurelRawInput");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WndClassExW {
            size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: raw_input_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: 0,
            cursor: 0,
            background: 0,
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            icon_sm: 0,
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("could not register the raw input window class".into());
        }
        let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, 0, instance, std::ptr::null_mut());
        if hwnd == 0 {
            return Err("could not create the raw input window".into());
        }
        register_devices(hwnd, false)?;
        Ok(hwnd)
    }
}

pub fn run_message_loop() {
    let mut msg = Msg::default();
    while unsafe { GetMessageW(&mut msg, 0, 0, 0) } > 0 {
        unsafe { DispatchMessageW(&msg) };
    }
}

/// Changes the mouse registration from any thread (the window's thread applies it).
pub fn request_raw_input(hwnd: isize, mouse: bool) {
    unsafe { PostMessageW(hwnd, WM_APP_RAW_INPUT, mouse as usize, 0) };
}

pub fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// The character a key types in the current keyboard layout (dead keys included).
pub fn key_char(vk: u16) -> Option<char> {
    let code = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) } & 0xFFFF;
    char::from_u32(code).filter(|c| !c.is_control() && *c != '\0')
}

/// Sends key presses as if typed (self-test only; they go to the focused window).
pub fn send_keys(keys: &[(u16, bool)]) -> u32 {
    let inputs: Vec<KeyInput> = keys
        .iter()
        .map(|&(vk, up)| KeyInput {
            kind: INPUT_KEYBOARD,
            _align: 0,
            vk,
            scan: unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16,
            flags: if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            extra: 0,
            _pad: [0; 8],
        })
        .collect();
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<KeyInput>() as i32) }
}

/// SHELLEXECUTEINFOW (natural alignment on 64-bit Windows).
#[repr(C)]
struct ShellExecuteInfo {
    size: u32,
    mask: u32,
    hwnd: isize,
    verb: *const u16,
    file: *const u16,
    parameters: *const u16,
    directory: *const u16,
    show: i32,
    inst_app: isize,
    id_list: *mut c_void,
    class: *const u16,
    hkey_class: isize,
    hot_key: u32,
    icon: isize,
    process: isize,
}

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION: u32 = 20;
const SYNCHRONIZE: u32 = 0x0010_0000;
const SW_SHOWNORMAL: i32 = 1;
const ERROR_CANCELLED: u32 = 1223;

fn token_elevated(process: isize) -> Option<bool> {
    unsafe {
        let mut token = 0;
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let (mut elevated, mut len) = (0u32, 0u32);
        let ok = GetTokenInformation(token, TOKEN_ELEVATION, (&mut elevated as *mut u32).cast(), 4, &mut len) != 0;
        CloseHandle(token);
        ok.then_some(elevated != 0)
    }
}

/// Whether Laurel itself runs as administrator.
pub fn self_elevated() -> bool {
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

/// Whether a process runs as administrator; None when its token cannot be read,
/// which for a process Laurel can open at all means elevated (or protected).
pub fn process_elevated(pid: u32) -> Option<bool> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process == 0 {
            return None;
        }
        let elevated = token_elevated(process);
        CloseHandle(process);
        elevated
    }
}

/// Waits up to `ms` for a process to exit.
pub fn wait_for_exit(pid: u32, ms: u32) {
    unsafe {
        let process = OpenProcess(SYNCHRONIZE, 0, pid);
        if process != 0 {
            WaitForSingleObject(process, ms);
            CloseHandle(process);
        }
    }
}

/// Starts a program as administrator (the UAC prompt). Err("cancelled") when the
/// user says no.
pub fn run_as_admin(exe: &std::path::Path, parameters: &str) -> Result<(), String> {
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(parameters));
    let mut info = ShellExecuteInfo {
        size: std::mem::size_of::<ShellExecuteInfo>() as u32,
        mask: 0,
        hwnd: 0,
        verb: verb.as_ptr(),
        file: file.as_ptr(),
        parameters: params.as_ptr(),
        directory: std::ptr::null(),
        show: SW_SHOWNORMAL,
        inst_app: 0,
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        hkey_class: 0,
        hot_key: 0,
        icon: 0,
        process: 0,
    };
    if unsafe { ShellExecuteExW(&mut info) } != 0 {
        return Ok(());
    }
    match unsafe { GetLastError() } {
        ERROR_CANCELLED => Err("cancelled".into()),
        code => Err(format!("could not start as administrator (error {code})")),
    }
}

/// A top-level window a player could be looking at.
#[derive(Clone, Debug)]
pub struct AppWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub rect: Rect,
}

extern "system" fn collect_window(hwnd: isize, param: isize) -> i32 {
    let list = unsafe { &mut *(param as *mut Vec<isize>) };
    list.push(hwnd);
    1
}

fn window_string(hwnd: isize, get: unsafe extern "system" fn(isize, *mut u16, i32) -> i32) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { get(hwnd, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Visible, unowned, not minimized, not cloaked top-level windows that are not tool
/// windows, in Z order (spec game.detection).
pub fn app_windows() -> Vec<AppWindow> {
    let mut handles: Vec<isize> = Vec::new();
    unsafe { EnumWindows(collect_window, &mut handles as *mut Vec<isize> as isize) };
    handles
        .into_iter()
        .filter(|&hwnd| unsafe {
            let mut cloaked = 0u32;
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
            IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 && GetWindow(hwnd, GW_OWNER) == 0 && cloaked == 0 && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW == 0
        })
        .map(|hwnd| {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            AppWindow { hwnd, pid, title: window_string(hwnd, GetWindowTextW), class: window_string(hwnd, GetClassNameW), rect: window_rect(hwnd) }
        })
        .collect()
}

/// Full path of a process's executable; works for elevated and protected games too.
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process == 0 {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Plays a WAV image asynchronously; the image must outlive the playback.
pub fn play_wav(wav: &'static [u8]) {
    unsafe { PlaySoundW(wav.as_ptr().cast(), 0, SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
}
