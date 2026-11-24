//! A34 of the stability tests: the shell of the watch on OBS's game capture lines,
//! whose rule is laurel_engine_core::hook_stop (when the engine clears the game
//! hook's stop signal it left itself, and the notes of a hook waiting in a game for
//! its first frame). Here are the Windows calls (opening a game's process, reading
//! its token, clearing OBS's event in it), the thread that looks at the waits every
//! second, and the state the libobs log handler feeds from its own threads.
//!
//! The names read here are OBS 32.1.2's (wiki/projeto/runtime-obs.md).

use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laurel_engine_core::hook_stop::{GameProcesses, StopSignalWatch};

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

/// How often the waits are looked at.
const NOTE_POLL: Duration = Duration::from_secs(1);

static STATE: Mutex<Option<StopSignalWatch>> = Mutex::new(None);

/// Clears the engine's own stop signals, and notes the hook waiting for a first
/// frame, from start until stop (or drop).
pub struct Watch {
    /// The thread that notes the waits, and the sender whose drop ends it.
    note_thread: Option<(Sender<()>, JoinHandle<()>)>,
}

impl Watch {
    pub fn start() -> Watch {
        if let Ok(mut state) = STATE.lock() {
            *state = Some(StopSignalWatch::default());
        }
        let (stop, stopped) = channel::<()>();
        let thread = std::thread::Builder::new().name("laurel: game hook waits".into()).spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(NOTE_POLL) {
                if let Some(state) = STATE.lock().ok().as_mut().and_then(|state| state.as_mut()) {
                    for note in state.waits(Instant::now()) {
                        log!("{note}");
                    }
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
/// It runs in the thread that logged it, so that the engine clears the hook's stop
/// signal right after OBS set it.
pub fn on_game_capture_line(source: &str, message: &str) {
    let Ok(mut state) = STATE.lock() else { return };
    if let Some(state) = state.as_mut() {
        if let Some(note) = state.line(source, message, Instant::now(), &mut Windows) {
            log!("{note}");
        }
    }
}

/// The game processes as Windows sees them.
struct Windows;

impl GameProcesses for Windows {
    fn started(&mut self, pid: u32) -> Option<u64> {
        let process = open_process(pid)?;
        let (mut created, mut exited, mut kernel, mut user) = (0, 0, 0, 0);
        let ok = unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
        ok.then_some(created)
    }

    /// Whether the process runs in an app container (a Microsoft Store app).
    fn store_app(&mut self, pid: u32) -> bool {
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

    fn clear_stop_signal(&mut self, pid: u32, exe: &str) -> bool {
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
