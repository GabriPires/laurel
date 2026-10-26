//! Laurel's Steam helper (MIT license, apart from Laurel's GPL code).
//!
//! Laurel starts it when it detects a running Steam game and stops it when the game
//! closes. It loads the Steam API library that is already on the PC (the game's own
//! steam_api64.dll, else another installed game's), starts it as the game's app, and
//! reports the player's new achievement unlocks. It ships nothing from Valve and
//! never touches the game's process. While it runs, Steam counts it as part of the
//! game, which is already running.
//!
//! Usage: laurel-steam --appid N [--dll PATH] [--interval MS]
//! Output: one JSON object per line on standard output:
//!   {"event":"ready","appid":N,"library":"...","origin":"game"|"other"|"given","achievements":N,"unlocked":N}
//!   {"event":"unlocked","id":"...","name":"...","description":"...","atMs":N}
//!   {"event":"error","message":"..."}          (then it exits with code 1)
//!   {"event":"stopped","passes":N,"cpuMs":N,"workingSetMb":N}
//! It stops when its standard input closes or reads "quit".

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows::core::{w, HSTRING, PCSTR};
use windows::Win32::Foundation::{FILETIME, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

type InitFlat = unsafe extern "C" fn(*mut c_char) -> i32;
type InitBool = unsafe extern "C" fn() -> bool;
type NoArgs = unsafe extern "C" fn();
type Accessor = unsafe extern "C" fn() -> *mut c_void;
type RequestCurrentStats = unsafe extern "C" fn(*mut c_void) -> bool;
type GetNumAchievements = unsafe extern "C" fn(*mut c_void) -> u32;
type GetAchievementName = unsafe extern "C" fn(*mut c_void, u32) -> *const c_char;
type GetAchievementAndUnlockTime = unsafe extern "C" fn(*mut c_void, *const c_char, *mut bool, *mut u32) -> bool;
type GetAchievementDisplayAttribute = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *const c_char;

fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn fail(message: &str) -> ! {
    emit(json!({ "event": "error", "message": message }));
    std::process::exit(1);
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn steam_path() -> Option<PathBuf> {
    let mut buf = [0u16; 520];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Valve\Steam"), w!("SteamPath"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size))
    };
    status.is_ok().then(|| PathBuf::from(String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]).replace('/', "\\")))
}

/// The values of `"key" "value"` lines of a Valve key-value text file.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let pattern = format!("\"{key}\"");
    text.lines()
        .filter_map(|l| Some(l.trim().strip_prefix(&pattern)?.trim().trim_matches('"').replace("\\\\", "\\")))
        .collect()
}

fn find_dll(dir: &Path, depth: u32) -> Option<PathBuf> {
    let mut dirs = vec![];
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            dirs.push(p);
        } else if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("steam_api64.dll")) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.iter().find_map(|d| find_dll(d, depth - 1))
}

/// The game's own steam_api64.dll, else the first one in another installed game:
/// the library only talks to the Steam client, which serves the app named by
/// SteamAppId.
fn locate_dll(appid: u32) -> Option<(PathBuf, &'static str)> {
    let steam = steam_path()?;
    let mut libraries = vdf_values(&std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")).unwrap_or_default(), "path");
    libraries.push(steam.to_string_lossy().into_owned());
    for lib in &libraries {
        let apps = Path::new(lib).join("steamapps");
        let Ok(manifest) = std::fs::read_to_string(apps.join(format!("appmanifest_{appid}.acf"))) else { continue };
        if let Some(dll) = vdf_values(&manifest, "installdir").first().and_then(|dir| find_dll(&apps.join("common").join(dir), 8)) {
            return Some((dll, "game"));
        }
    }
    libraries.iter().find_map(|lib| {
        let common = Path::new(lib).join(r"steamapps\common");
        std::fs::read_dir(common).into_iter().flatten().flatten().find_map(|game| find_dll(&game.path(), 8)).map(|dll| (dll, "other"))
    })
}

struct Api(HMODULE);

impl Api {
    fn get<T: Copy>(&self, name: &str) -> Option<T> {
        let c = CString::new(name).ok()?;
        let f = unsafe { GetProcAddress(self.0, PCSTR(c.as_ptr() as _)) }?;
        Some(unsafe { std::mem::transmute_copy(&f) })
    }

    /// The newest SteamAPI_SteamUserStats_vNNN accessor the library exports.
    fn user_stats(&self) -> Option<Accessor> {
        (8..=20).rev().find_map(|v| self.get::<Accessor>(&format!("SteamAPI_SteamUserStats_v{v:03}")))
    }

    /// Starts the library with the newer flat init, else the older one.
    fn init(&self) -> Result<(), String> {
        if let Some(f) = self.get::<InitFlat>("SteamAPI_InitFlat") {
            let mut msg = [0 as c_char; 1024];
            match unsafe { f(msg.as_mut_ptr()) } {
                0 => Ok(()),
                r => Err(format!("SteamAPI_InitFlat returned {r}: {}", text(msg.as_ptr()))),
            }
        } else if let Some(f) = self.get::<InitBool>("SteamAPI_InitSafe").or_else(|| self.get::<InitBool>("SteamAPI_Init")) {
            if unsafe { f() } {
                Ok(())
            } else {
                Err("Steam API init failed (is Steam running?)".into())
            }
        } else {
            Err("the library has no Steam API init function".into())
        }
    }
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn cpu_ms() -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let ticks = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    (ticks(k) + ticks(u)) / 10_000
}

fn working_set_mb() -> f64 {
    let mut m = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut m, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
    }
    m.WorkingSetSize as f64 / 1048576.0
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn main() {
    let appid: u32 = arg("--appid").and_then(|a| a.parse().ok()).unwrap_or_else(|| fail("usage: laurel-steam --appid N [--dll PATH] [--interval MS]"));
    let interval = Duration::from_millis(arg("--interval").and_then(|a| a.parse().ok()).unwrap_or(1000));
    let (dll, origin) = match arg("--dll") {
        Some(p) => (PathBuf::from(p), "given"),
        None => locate_dll(appid).unwrap_or_else(|| fail("no steam_api64.dll in the Steam libraries")),
    };

    // Standard input closing (Laurel quit or stopped it) or "quit" ends the watch.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) if l.trim() != "quit" => continue,
                    _ => break,
                }
            }
            stop.store(true, Ordering::SeqCst);
        });
    }

    std::env::set_var("SteamAppId", appid.to_string());
    std::env::set_var("SteamGameId", appid.to_string());
    let module = unsafe { LoadLibraryW(&HSTRING::from(dll.as_os_str())) }.unwrap_or_else(|e| fail(&format!("cannot load {}: {e}", dll.display())));
    let api = Api(module);
    if let Err(e) = api.init() {
        fail(&e);
    }
    let run_callbacks = api.get::<NoArgs>("SteamAPI_RunCallbacks");
    let stats = unsafe { api.user_stats().unwrap_or_else(|| fail("the library has no SteamUserStats accessor"))() };
    let get_num = api.get::<GetNumAchievements>("SteamAPI_ISteamUserStats_GetNumAchievements").unwrap_or_else(|| fail("no GetNumAchievements"));
    let get_name = api.get::<GetAchievementName>("SteamAPI_ISteamUserStats_GetAchievementName").unwrap_or_else(|| fail("no GetAchievementName"));
    let get_ach = api.get::<GetAchievementAndUnlockTime>("SteamAPI_ISteamUserStats_GetAchievementAndUnlockTime").unwrap_or_else(|| fail("no GetAchievementAndUnlockTime"));
    let get_attr = api.get::<GetAchievementDisplayAttribute>("SteamAPI_ISteamUserStats_GetAchievementDisplayAttribute").unwrap_or_else(|| fail("no GetAchievementDisplayAttribute"));
    // Older libraries ask for the stats; newer ones fetch them by themselves.
    if let Some(request) = api.get::<RequestCurrentStats>("SteamAPI_ISteamUserStats_RequestCurrentStats") {
        unsafe { request(stats) };
    }

    // The achievements can be read once the stats arrived.
    let wait = Instant::now();
    let (mut names, mut unlocked): (Vec<CString>, Vec<bool>) = (vec![], vec![]);
    loop {
        if let Some(f) = run_callbacks {
            unsafe { f() };
        }
        let n = unsafe { get_num(stats) };
        if n > 0 {
            names = (0..n).filter_map(|i| CString::new(text(unsafe { get_name(stats, i) })).ok()).collect();
            let mut ready = true;
            unlocked = names
                .iter()
                .map(|name| {
                    let (mut done, mut time) = (false, 0u32);
                    ready &= unsafe { get_ach(stats, name.as_ptr(), &mut done, &mut time) };
                    done
                })
                .collect();
            if ready {
                break;
            }
        }
        if wait.elapsed() > Duration::from_secs(15) || stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    emit(json!({
        "event": "ready", "appid": appid, "library": dll.to_string_lossy(), "origin": origin,
        "achievements": names.len(), "unlocked": unlocked.iter().filter(|u| **u).count(),
    }));

    let mut passes = 0u64;
    while !stop.load(Ordering::SeqCst) {
        if let Some(f) = run_callbacks {
            unsafe { f() };
        }
        for (i, name) in names.iter().enumerate() {
            if unlocked[i] {
                continue;
            }
            let (mut done, mut time) = (false, 0u32);
            if unsafe { get_ach(stats, name.as_ptr(), &mut done, &mut time) } && done {
                unlocked[i] = true;
                emit(json!({
                    "event": "unlocked",
                    "id": name.to_string_lossy(),
                    "name": text(unsafe { get_attr(stats, name.as_ptr(), c"name".as_ptr()) }),
                    "description": text(unsafe { get_attr(stats, name.as_ptr(), c"desc".as_ptr()) }),
                    "atMs": now_ms(),
                }));
            }
        }
        passes += 1;
        // Short sleeps, so a stop is noticed quickly.
        let slept = Instant::now();
        while slept.elapsed() < interval && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    emit(json!({ "event": "stopped", "passes": passes, "cpuMs": cpu_ms(), "workingSetMb": (working_set_mb() * 10.0).round() / 10.0 }));
    if let Some(f) = api.get::<NoArgs>("SteamAPI_Shutdown") {
        unsafe { f() };
    }
}
