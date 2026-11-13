//! A34 of the stability tests: the engine clears the game hook's stop signal that it
//! left itself, so that a game that draws its first frame late still gets the hook.
//!
//! In OBS 32.1.2 (plugins/win-capture/game-capture.c), a game capture that stops
//! (stop_capture) sets the hook's stop signal, an auto-reset event named
//! "CaptureHook_Stop<pid>" that the hook creates inside the game, and then logs
//! "capture stopped". Only a hook that captures reads the signal. When the inject
//! helper of the compatibility hook ends (after 4 s) before the game drew a frame,
//! the game capture stops and the signal stays set: every later OBS hook attempt on
//! that process finds it on its first tick ("hook stop signal received") and stops,
//! setting it again, and the game stays on the window fallback.
//!
//! The rule (the owner's decisions of 2026-11-11, issues #2 and #6 of the private
//! repository): after a "capture stopped" that did not come from a signal OBS found
//! ("hook stop signal received" just before it), the engine clears the signal, on a
//! game process that never hooked in this engine and whose first stop line in this
//! engine was such a lone "capture stopped". A process whose first stop line was
//! "hook stop signal received" has a signal the engine found already set: after an
//! orderly restart of the engine it is what makes a hook that captured for the old
//! engine let go of that capture (A26), so it is never cleared. The same holds when
//! the engine itself removes a game capture (the window fallback, the end of a hook
//! retry), which logs the same line, but not at the engine's orderly end: the watch
//! stops before the recorder lets its sources go.
//!
//! A process hooked in this engine once OBS saw the hook's ready signal ("capture
//! initializing!"): from then on the hook captures and reads the stop signal itself,
//! also when OBS then fails to open its texture (A13) or the device is lost (A24) and
//! the attempt ends with a lone "capture stopped".
//!
//! Left alone, with one line each: a Microsoft Store game (OBS keeps its signals in
//! the app's own namespace), a process whose signal the engine cannot open (a game
//! running as administrator), and a process the engine cannot open at all.
//!
//! The watch also notes, in the engine's log, a hook that is in the game while OBS
//! waits for its first frame (A34; issue #5 of the private repository): from OBS's
//! "map id: CaptureHook_HookInfo<pid>", which comes only after OBS opened the texture
//! mutexes the hook creates when it loads in the game, until "capture initializing!"
//! (the hook's ready signal, its first frame) or the attempt's end. A stuck stop
//! signal ends each attempt on its first tick ("hook stop signal received"), so the
//! note tells a hook that sees no frame of the game apart from a stuck signal. The
//! first note comes after FIRST_NOTE, longer than the inject helper's 4 s and shorter
//! than the app's 10 s hook retries; the next ones each three times as far, at most
//! NOTE_GAP_MAX apart. An attempt lasts at most one of those retries in the app, so
//! the note also says how long ago OBS first found the hook in that process, while
//! no attempt there has had a first frame.
//!
//! The names and lines read here are OBS 32.1.2's (wiki/projeto/runtime-obs.md).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[link(name = "kernel32", kind = "raw-dylib")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn GetProcessTimes(process: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn ResetEvent(event: isize) -> i32;
    fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetLastError() -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
extern "system" {
    fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
    fn GetTokenInformation(token: isize, class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_IS_APP_CONTAINER: u32 = 29;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const SYNCHRONIZE: u32 = 0x0010_0000;
const WAIT_OBJECT_0: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;

/// The hook's wait for the game's first frame is noted first after this long...
const FIRST_NOTE: Duration = Duration::from_secs(5);
/// ...then each time it has gone three times as long, at most this far apart.
const NOTE_GAP_MAX: Duration = Duration::from_secs(300);
/// How often the waits are looked at.
const NOTE_POLL: Duration = Duration::from_secs(1);

/// A game process: its id and its start time, since Windows reuses ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ProcessKey {
    pid: u32,
    created: u64,
}

/// What OBS logged last about one game capture source.
#[derive(Default)]
struct GameSource {
    /// The executable of the current hook attempt.
    exe: String,
    /// The process of the current hook attempt, once OBS opened its hook info.
    process: Option<ProcessKey>,
    /// "hook stop signal received": the "capture stopped" that follows is OBS
    /// setting again the signal it found.
    signal_found: bool,
    /// The hook is in the game and OBS waits for its first frame.
    waiting: Option<Wait>,
}

/// OBS waiting for the first frame of the hook in a game process.
struct Wait {
    pid: u32,
    since: Instant,
    /// How long the wait has to go before the next note.
    next_note: Duration,
}

#[derive(Clone, Copy, PartialEq)]
enum FirstStop {
    /// A lone "capture stopped": the signal is the engine's own.
    Own,
    /// "hook stop signal received": the engine found the signal already set.
    Found,
}

#[derive(Default)]
struct Process {
    /// When OBS first opened the hook in this process ("map id").
    hook_found: Option<Instant>,
    first_stop: Option<FirstStop>,
    /// The hook started a capture for this engine ("capture initializing!").
    hooked: bool,
    /// A Microsoft Store game, or a signal the engine could not open.
    left_alone: bool,
}

#[derive(Default)]
struct State {
    sources: HashMap<String, GameSource>,
    processes: HashMap<ProcessKey, Process>,
    /// Game processes the engine could not open, already logged once.
    unopened: HashSet<u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Clears the engine's own stop signals, and notes the hook waiting for a first
/// frame, from start until stop (or drop).
pub struct Watch {
    /// The thread that notes the waits, and the sender whose drop ends it.
    note_thread: Option<(Sender<()>, JoinHandle<()>)>,
}

impl Watch {
    pub fn start() -> Watch {
        if let Ok(mut state) = STATE.lock() {
            *state = Some(State::default());
        }
        let (stop, stopped) = channel::<()>();
        let thread = std::thread::Builder::new().name("laurel: game hook waits".into()).spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(NOTE_POLL) {
                if let Some(state) = STATE.lock().ok().as_mut().and_then(|state| state.as_mut()) {
                    state.note_waits(Instant::now());
                }
            }
        });
        if let Err(e) = &thread {
            log!("[laurel] game hook: could not start the thread that notes the hook's waits: {e}");
        }
        Watch { note_thread: thread.ok().map(|thread| (stop, thread)) }
    }

    /// Stops clearing and noting: the recorder calls it before it lets its sources go.
    pub fn stop(&mut self) {
        if let Ok(mut state) = STATE.lock() {
            *state = None;
        }
        if let Some((stop, thread)) = self.note_thread.take() {
            drop(stop);
            let _ = thread.join();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reads a game capture line of libobs' log: the source's name and OBS's message.
pub fn on_game_capture_line(source: &str, message: &str) {
    let Ok(mut state) = STATE.lock() else { return };
    if let Some(state) = state.as_mut() {
        state.handle(source, message);
    }
}

impl State {
    fn handle(&mut self, name: &str, message: &str) {
        let attempt = message.strip_prefix("attempting to hook process: ").or_else(|| message.strip_prefix("attempting to hook fullscreen process: "));
        if let Some(exe) = attempt {
            self.sources.insert(name.to_string(), GameSource { exe: exe.to_string(), ..GameSource::default() });
            return;
        }
        if let Some(pid) = message.strip_prefix("map id: CaptureHook_HookInfo").and_then(|id| id.parse().ok()) {
            let now = Instant::now();
            let source = self.sources.entry(name.to_string()).or_default();
            source.waiting = Some(Wait { pid, since: now, next_note: FIRST_NOTE });
            source.process = process_key(pid);
            match source.process {
                None if self.unopened.insert(pid) => log!("[laurel] game hook: could not open {} (pid {pid}); the engine leaves its stop signal alone", source.exe),
                Some(key) if !self.processes.contains_key(&key) => {
                    let process = self.processes.entry(key).or_default();
                    process.hook_found = Some(now);
                    if store_app(pid) {
                        log!("[laurel] game hook: {} (pid {pid}) is a Microsoft Store app; OBS keeps its stop signal in the app's own namespace, which the engine does not clear", source.exe);
                        process.left_alone = true;
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(source) = self.sources.get_mut(name) else { return };
        if ends_wait(message) {
            source.waiting = None;
        }
        let Some(key) = source.process else { return };
        match message {
            "hook stop signal received" => {
                source.signal_found = true;
                self.processes.entry(key).or_default().first_stop.get_or_insert(FirstStop::Found);
            }
            "capture stopped" => {
                if std::mem::take(&mut source.signal_found) {
                    return;
                }
                let process = self.processes.entry(key).or_default();
                let own = *process.first_stop.get_or_insert(FirstStop::Own) == FirstStop::Own;
                if own && !process.hooked && !process.left_alone && !clear(key.pid, &source.exe) {
                    process.left_alone = true;
                }
            }
            // OBS saw the hook's ready signal: the hook captures for this engine from
            // now on, also when OBS then fails to open its texture (A13) and stops.
            "capture initializing!" => self.processes.entry(key).or_default().hooked = true,
            _ => {}
        }
    }

    /// Notes each hook in a game that has waited for its first frame past its next note.
    fn note_waits(&mut self, now: Instant) {
        for (name, source) in &mut self.sources {
            let Some(wait) = source.waiting.as_mut() else { continue };
            let waited = now.saturating_duration_since(wait.since);
            if waited < wait.next_note {
                continue;
            }
            let process = source.process.and_then(|key| self.processes.get(&key)).filter(|process| !process.hooked);
            let found = match process.and_then(|process| process.hook_found) {
                Some(found) => format!("; no first frame since OBS found it there {} s ago", now.saturating_duration_since(found).as_secs()),
                None => String::new(),
            };
            log!("[laurel] game hook: the hook is in {} (pid {}) and has waited {} s for the game's first frame (source '{name}'){found}", source.exe, wait.pid, waited.as_secs());
            while wait.next_note <= waited {
                wait.next_note += wait.next_note.saturating_mul(2).min(NOTE_GAP_MAX);
            }
        }
    }
}

/// The lines after "map id" that end OBS's wait for the hook's first frame: the
/// first frame ("capture initializing!"), the attempt's end ("capture stopped", which
/// also follows "hook stop signal received"), and the failures of the rest of
/// init_hook, which end the attempt before it is active and so without "capture
/// stopped".
fn ends_wait(message: &str) -> bool {
    matches!(message, "capture initializing!" | "capture stopped")
        || message.starts_with("init_hook_info: get_hook_info failed")
        || message.starts_with("init_hook_info: failed to map data view")
        || message.starts_with("init_events: ")
}

/// A Windows handle, closed when dropped.
struct Handle(isize);

impl Handle {
    fn new(raw: isize) -> Option<Handle> {
        (raw != 0).then_some(Handle(raw))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_process(pid: u32) -> Option<Handle> {
    Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// The process and its start time, if it can be opened.
fn process_key(pid: u32) -> Option<ProcessKey> {
    let process = open_process(pid)?;
    let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
    let ok = unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    ok.then_some(ProcessKey { pid, created })
}

/// Whether the process runs in an app container (a Microsoft Store app).
fn store_app(pid: u32) -> bool {
    let Some(process) = open_process(pid) else { return false };
    let mut raw = 0;
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut raw) } == 0 {
        return false;
    }
    let token = Handle(raw);
    let (mut app, mut length) = (0u32, 0);
    unsafe { GetTokenInformation(token.0, TOKEN_IS_APP_CONTAINER, (&mut app as *mut u32).cast(), 4, &mut length) };
    app != 0
}

/// Clears the stop signal of the hook in `pid` and logs it; false, logged once, when
/// the engine cannot and leaves this process alone from now on.
fn clear(pid: u32, exe: &str) -> bool {
    let name: Vec<u16> = format!("CaptureHook_Stop{pid}").encode_utf16().chain(Some(0)).collect();
    let Some(event) = Handle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, name.as_ptr()) }) else {
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => log!("[laurel] game hook: no stop signal left in {exe} (pid {pid}): the game closed or its hook is gone"),
            code => {
                let why = if code == ERROR_ACCESS_DENIED { " (access denied: the game may run as administrator)" } else { "" };
                log!("[laurel] game hook: could not open the stop signal in {exe} (pid {pid}): Windows error {code}{why}; not trying again for this process");
            }
        }
        return false;
    };
    // A wait that succeeds clears an auto-reset event; the reset covers a manual one.
    let was_set = unsafe { WaitForSingleObject(event.0, 0) } == WAIT_OBJECT_0;
    unsafe { ResetEvent(event.0) };
    let note = if was_set { "" } else { " (it was not set)" };
    log!("[laurel] game hook: cleared the stop signal the engine left in {exe} (pid {pid}) when a game capture stopped without the hook{note}");
    true
}
