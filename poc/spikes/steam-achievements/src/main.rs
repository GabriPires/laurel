//! Spike for route A of the Steam achievement auto-clip: a process apart from
//! Laurel loads the Steam API library that is already on the PC (the game's own
//! steam_api64.dll, else another game's), starts it as the game's app, and watches
//! the player's achievements. Nothing from Valve is shipped with it, and it never
//! touches the game's process. It also measures its own cost.
//!
//! Usage: steam-achievements --appid N [--dll PATH] [--seconds S] [--interval MS] [--stop-file PATH]
//!   --appid     the Steam app to watch (the running game; 480 is Valve's test app)
//!   --dll       a steam_api64.dll to use instead of searching the Steam libraries
//!   --seconds   how long to watch (default 60)
//!   --interval  time between two checks of the achievements (default 1000)
//!   --stop-file stop early once this file exists (how a test stops it when the game closes)
//! While it runs, Steam counts this process as part of the app.
//! New unlocks are printed as "[laurel] achievement at hh:mm:ss.mmm: ..." lines.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{s, w, HSTRING, PCSTR};
use windows::Win32::Foundation::{FILETIME, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows::Win32::System::SystemInformation::GetLocalTime;
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

fn clock() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
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

fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let pattern = format!("\"{key}\"");
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(&pattern)?.trim();
            Some(rest.trim_matches('"').replace("\\\\", "\\"))
        })
        .collect()
}

fn find_dll(dir: &Path, depth: u32) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = vec![];
    for e in entries.flatten() {
        let p = e.path();
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

/// The game's own steam_api64.dll, else the first one found in another game.
fn locate_dll(appid: u32) -> Option<(PathBuf, &'static str)> {
    let steam = steam_path()?;
    let mut libraries = vdf_values(&std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")).unwrap_or_default(), "path");
    libraries.push(steam.to_string_lossy().into_owned());
    for lib in &libraries {
        let apps = Path::new(lib).join("steamapps");
        if let Ok(manifest) = std::fs::read_to_string(apps.join(format!("appmanifest_{appid}.acf"))) {
            if let Some(dir) = vdf_values(&manifest, "installdir").first() {
                if let Some(dll) = find_dll(&apps.join("common").join(dir), 8) {
                    return Some((dll, "the game's own"));
                }
            }
        }
    }
    for lib in &libraries {
        let common = Path::new(lib).join(r"steamapps\common");
        for game in std::fs::read_dir(&common).into_iter().flatten().flatten() {
            if let Some(dll) = find_dll(&game.path(), 8) {
                return Some((dll, "another game's"));
            }
        }
    }
    None
}

struct Api {
    module: HMODULE,
}

impl Api {
    fn get<T: Copy>(&self, name: &str) -> Option<T> {
        let c = CString::new(name).unwrap();
        let f = unsafe { GetProcAddress(self.module, PCSTR(c.as_ptr() as _)) }?;
        Some(unsafe { std::mem::transmute_copy(&f) })
    }

    /// The newest SteamAPI_SteamUserStats_vNNN accessor the library exports.
    fn user_stats(&self) -> Option<(String, Accessor)> {
        (8..=20).rev().find_map(|v| {
            let name = format!("SteamAPI_SteamUserStats_v{v:03}");
            self.get::<Accessor>(&name).map(|f| (name, f))
        })
    }
}

fn cpu_seconds() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
    t(k) + t(u)
}

fn memory_mb() -> (f64, f64) {
    let mut m = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut m, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
    }
    (m.WorkingSetSize as f64 / 1048576.0, m.PeakWorkingSetSize as f64 / 1048576.0)
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn main() {
    let Some(appid) = arg("--appid").and_then(|a| a.parse::<u32>().ok()) else {
        eprintln!("usage: steam-achievements --appid N [--dll PATH] [--seconds S] [--interval MS]");
        std::process::exit(2);
    };
    let seconds: u64 = arg("--seconds").and_then(|a| a.parse().ok()).unwrap_or(60);
    let interval: u64 = arg("--interval").and_then(|a| a.parse().ok()).unwrap_or(1000);
    let stop_file = arg("--stop-file").map(PathBuf::from);
    let (dll, origin) = match arg("--dll") {
        Some(p) => (PathBuf::from(p), "given"),
        None => match locate_dll(appid) {
            Some(found) => found,
            None => {
                eprintln!("[steam-spike] no steam_api64.dll found in the Steam libraries");
                std::process::exit(1);
            }
        },
    };
    println!("[steam-spike] library {} ({origin})", dll.display());
    std::env::set_var("SteamAppId", appid.to_string());
    std::env::set_var("SteamGameId", appid.to_string());
    let started = Instant::now();
    let module = match unsafe { LoadLibraryW(&HSTRING::from(dll.as_os_str())) } {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[steam-spike] cannot load the library: {e}");
            std::process::exit(1);
        }
    };
    let api = Api { module };

    let init = if let Some(f) = api.get::<InitFlat>("SteamAPI_InitFlat") {
        let mut msg = [0 as c_char; 1024];
        let r = unsafe { f(msg.as_mut_ptr()) };
        (r == 0, format!("SteamAPI_InitFlat -> {r} {}", text(msg.as_ptr())))
    } else if let Some(f) = api.get::<InitBool>("SteamAPI_InitSafe").or_else(|| api.get::<InitBool>("SteamAPI_Init")) {
        let ok = unsafe { f() };
        (ok, format!("SteamAPI_InitSafe/Init -> {ok}"))
    } else {
        (false, "no init function".into())
    };
    println!("[steam-spike] {} after {} ms", init.1, started.elapsed().as_millis());
    if !init.0 {
        std::process::exit(1);
    }
    let run_callbacks = api.get::<NoArgs>("SteamAPI_RunCallbacks");
    let shutdown = api.get::<NoArgs>("SteamAPI_Shutdown");
    let Some((accessor_name, accessor)) = api.user_stats() else {
        eprintln!("[steam-spike] no SteamUserStats accessor");
        std::process::exit(1);
    };
    let stats = unsafe { accessor() };
    let get_num = api.get::<GetNumAchievements>("SteamAPI_ISteamUserStats_GetNumAchievements").expect("GetNumAchievements");
    let get_name = api.get::<GetAchievementName>("SteamAPI_ISteamUserStats_GetAchievementName").expect("GetAchievementName");
    let get_ach = api.get::<GetAchievementAndUnlockTime>("SteamAPI_ISteamUserStats_GetAchievementAndUnlockTime").expect("GetAchievementAndUnlockTime");
    let get_attr = api.get::<GetAchievementDisplayAttribute>("SteamAPI_ISteamUserStats_GetAchievementDisplayAttribute").expect("GetAchievementDisplayAttribute");
    if let Some(request) = api.get::<RequestCurrentStats>("SteamAPI_ISteamUserStats_RequestCurrentStats") {
        let ok = unsafe { request(stats) };
        println!("[steam-spike] RequestCurrentStats -> {ok}");
    }
    println!("[steam-spike] interface {accessor_name}");

    // Wait until the stats are there: the achievements can be read.
    let wait = Instant::now();
    let mut names: Vec<CString> = vec![];
    let mut unlocked: Vec<bool> = vec![];
    while wait.elapsed() < Duration::from_secs(15) {
        if let Some(f) = run_callbacks {
            unsafe { f() };
        }
        let n = unsafe { get_num(stats) };
        if n > 0 {
            names = (0..n).map(|i| CString::new(text(unsafe { get_name(stats, i) })).unwrap()).collect();
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
        std::thread::sleep(Duration::from_millis(100));
    }
    println!(
        "[steam-spike] stats ready after {} ms: {} achievements, {} unlocked",
        wait.elapsed().as_millis(),
        names.len(),
        unlocked.iter().filter(|u| **u).count()
    );

    // Watch: one pass reads every achievement not yet unlocked.
    let (cpu_start, watch) = (cpu_seconds(), Instant::now());
    let (mut passes, mut total_us, mut max_us) = (0u64, 0u128, 0u128);
    while watch.elapsed() < Duration::from_secs(seconds) && !stop_file.as_ref().is_some_and(|f| f.exists()) {
        let pass = Instant::now();
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
                let display = text(unsafe { get_attr(stats, name.as_ptr(), s!("name").0 as _) });
                println!("[laurel] achievement at {}: {} \"{display}\" (unlock time {time})", clock(), name.to_string_lossy());
            }
        }
        let us = pass.elapsed().as_micros();
        passes += 1;
        total_us += us;
        max_us = max_us.max(us);
        std::thread::sleep(Duration::from_millis(interval));
    }
    let cpu = cpu_seconds() - cpu_start;
    let watched = watch.elapsed().as_secs_f64();
    let (ws, peak) = memory_mb();
    println!(
        "[steam-spike] {passes} passes, {} us on average, {max_us} us at most; CPU {:.3} s in {watched:.0} s ({:.3}% of one core); working set {ws:.1} MB, peak {peak:.1} MB",
        if passes > 0 { total_us / passes as u128 } else { 0 },
        cpu,
        cpu / watched * 100.0
    );
    if let Some(f) = shutdown {
        unsafe { f() };
    }
}
