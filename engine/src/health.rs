//! A24 and A25 of the stability tests: the engine watches its own health and, when
//! it can no longer record, ends so that the app starts a new engine.
//!
//! A24: the libobs graphics device (Direct3D 11) can be lost: DXGI_ERROR_DEVICE_REMOVED
//! after a GPU driver is installed or updated, a GPU reset or removal. libobs
//! rebuilds the device only when Present on a swap chain fails, and the engine,
//! which has no display, never presents; on a friend's laptop the device stayed lost
//! for hours, and neither the game nor its window could be captured. A new engine
//! process gets a new device.
//!
//! A25: with no video frame encoded, libobs keeps every audio encoder's input without
//! limit (obs-encoder.c buffer_audio) and the replay buffer's stop waits for a packet
//! that never comes, so the output stays active; the engine ran out of memory hours
//! later. The watch ends the engine once an active replay buffer has gone 20 s
//! without a new video frame, whatever the cause.
//!
//! Also, after a driver install the high-performance GPU may come back after the
//! engine started on the other one (A13): while the replay buffer is stopped, the
//! engine ends when the automatic choice of GPU (gpu.rs) is the high-performance GPU
//! and not its own, at two checks in a row (so a failed query cannot restart
//! engines one after another).
//!
//! Every 2 s the watch thread checks the device, and the output; OBS's own "Device
//! Removed Reason" line and the shared-texture watch (recorder.rs) wake it at once.
//! On a problem it logs why, tells the app ("unhealthy"), and hands the problem to
//! the main thread, which shuts libobs down in order (releasing a game's hook) and
//! exits with the problem's code. A shutdown that hangs (the destroy of a stuck
//! output waits for its stop forever) is cut short after 10 s.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::json;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::gpu;
use crate::obs::{ffi, WeakOutput};

/// Exit codes the app names (app/src/engine.rs describe_exit).
pub const EXIT_DEVICE_LOST: i32 = 60;
pub const EXIT_STALLED: i32 = 61;
pub const EXIT_GPU_CHANGED: i32 = 62;

const POLL: Duration = Duration::from_secs(2);
/// Polls without a new video frame, with the replay buffer active, before the
/// engine ends (20 s): a healthy one gets about one frame per output frame.
const STALL_POLLS: u32 = 10;
/// Polls between two checks of the GPU choice (30 s), while not recording.
const GPU_POLLS: u32 = 15;
/// How long the main thread gets to shut down in order after a problem.
const FORCE_EXIT_AFTER: Duration = Duration::from_secs(10);

const DXGI_ERROR_DEVICE_REMOVED: u32 = 0x887A_0005;

/// Why the engine has to end, and its exit code.
#[derive(Clone, Debug)]
pub struct Problem {
    pub code: i32,
    pub reason: String,
}

/// Test hooks (the "simulateHealth" request and the `simulate` script command):
/// they stand in for a lost device or a replay buffer without video, which cannot be
/// caused on a working PC. `STALL_HANG` also holds the shutdown, as a stuck output's
/// destroy does, so that the forced end is tested too.
const SIM_NONE: u8 = 0;
const SIM_DEVICE: u8 = 1;
const SIM_STALL: u8 = 2;
const SIM_STALL_HANG: u8 = 3;
static SIMULATED: AtomicU8 = AtomicU8::new(SIM_NONE);

enum Control {
    Wake,
    Stop,
}

/// The running watch's channel, for `wake`.
static WATCH: Mutex<Option<Sender<Control>>> = Mutex::new(None);

pub struct Watch {
    control: Sender<Control>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Stops the checks (before the recorder and libobs shut down); a problem
    /// already raised keeps its forced end.
    pub fn stop(mut self) {
        *WATCH.lock().unwrap() = None;
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Checks the device now (OBS logged a failure on a lost device).
pub fn wake() {
    if let Some(tx) = WATCH.lock().ok().as_ref().and_then(|w| w.as_ref()) {
        let _ = tx.send(Control::Wake);
    }
}

/// Starts a test problem: "device", "stall" or "stall-hang".
pub fn simulate(kind: &str) -> Result<(), String> {
    let value = match kind {
        "device" => SIM_DEVICE,
        "stall" => SIM_STALL,
        "stall-hang" => SIM_STALL_HANG,
        other => return Err(format!("bad health problem {other}, expected device, stall or stall-hang")),
    };
    SIMULATED.store(value, Ordering::SeqCst);
    log!("[laurel] health: simulating {kind}");
    wake();
    Ok(())
}

/// Whether the shutdown should hang (the "stall-hang" test).
pub fn hang_requested() -> bool {
    SIMULATED.load(Ordering::SeqCst) == SIM_STALL_HANG
}

fn removed_name(code: u32) -> &'static str {
    match code {
        0x887A_0005 => "DXGI_ERROR_DEVICE_REMOVED",
        0x887A_0006 => "DXGI_ERROR_DEVICE_HUNG",
        0x887A_0007 => "DXGI_ERROR_DEVICE_RESET",
        0x887A_0020 => "DXGI_ERROR_DRIVER_INTERNAL_ERROR",
        _ => "error",
    }
}

/// Why the libobs graphics device was lost, if it was: what Direct3D 11's
/// GetDeviceRemovedReason returns (S_OK while the device works).
pub fn device_lost() -> Option<String> {
    if SIMULATED.load(Ordering::SeqCst) == SIM_DEVICE {
        return Some(format!("0x{DXGI_ERROR_DEVICE_REMOVED:08X} ({}, simulated)", removed_name(DXGI_ERROR_DEVICE_REMOVED)));
    }
    let code = unsafe {
        ffi::obs_enter_graphics();
        let mut code = None;
        if ffi::gs_get_device_type() == ffi::GS_DEVICE_DIRECT3D_11 {
            let raw = ffi::gs_get_device_obj();
            if let Some(device) = ID3D11Device::from_raw_borrowed(&raw) {
                if let Err(e) = device.GetDeviceRemovedReason() {
                    code = Some(e.code().0 as u32);
                }
            }
        }
        ffi::obs_leave_graphics();
        code
    }?;
    Some(format!("0x{code:08X} ({})", removed_name(code)))
}

/// Starts the watch. `output` is the replay buffer (none without one); `adapter`
/// is the GPU the engine renders on when gpu.rs chose it (not with `--adapter`).
/// `raise` gets the problem on the watch thread; the main thread then shuts down.
pub fn start(output: Option<WeakOutput>, adapter: Option<gpu::Adapter>, raise: impl Fn(Problem) + Send + 'static) -> Watch {
    let (tx, rx) = channel();
    *WATCH.lock().unwrap() = Some(tx.clone());
    let device = match device_lost() {
        None => "ok".to_string(),
        Some(reason) => format!("lost ({reason})"),
    };
    log!("[laurel] health: graphics device {device}; checking it and the replay buffer every {} s", POLL.as_secs());
    let thread = std::thread::Builder::new()
        .name("laurel: health".into())
        .spawn(move || watch(output, adapter, rx, raise))
        .map_err(|e| log!("[laurel] health: no thread for the checks: {e}"))
        .ok();
    Watch { control: tx, thread }
}

/// What the watch remembers between polls.
#[derive(Default)]
struct Seen {
    polls: u32,
    /// Video frames the replay buffer had at the last poll, and polls since it got one.
    frames: Option<i32>,
    still: u32,
    /// GPU checks in a row that preferred another GPU.
    other_gpu: u32,
}

fn watch(output: Option<WeakOutput>, adapter: Option<gpu::Adapter>, control: Receiver<Control>, raise: impl Fn(Problem)) {
    let mut seen = Seen::default();
    loop {
        let timed = match control.recv_timeout(POLL) {
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Control::Wake) => false,
            Err(RecvTimeoutError::Timeout) => true,
        };
        if let Some(problem) = check(output.as_ref(), adapter.as_ref(), timed, &mut seen) {
            end(problem, &raise);
            return;
        }
    }
}

fn check(output: Option<&WeakOutput>, adapter: Option<&gpu::Adapter>, timed: bool, seen: &mut Seen) -> Option<Problem> {
    if let Some(reason) = device_lost() {
        return Some(Problem { code: EXIT_DEVICE_LOST, reason: format!("the graphics device was lost: {reason}") });
    }
    if !timed {
        return None;
    }
    seen.polls += 1;
    let stalled = matches!(SIMULATED.load(Ordering::SeqCst), SIM_STALL | SIM_STALL_HANG);
    match output.and_then(WeakOutput::state) {
        Some((true, total)) => {
            seen.other_gpu = 0;
            if !stalled && seen.frames != Some(total) {
                seen.frames = Some(total);
                seen.still = 0;
            } else {
                seen.still += 1;
            }
            if seen.still >= STALL_POLLS {
                let secs = u64::from(seen.still) * POLL.as_secs();
                let simulated = if stalled { ", simulated" } else { "" };
                return Some(Problem {
                    code: EXIT_STALLED,
                    reason: format!("the replay buffer got no video frame for {secs} s ({total} frames since it started{simulated})"),
                });
            }
        }
        _ => {
            seen.frames = None;
            seen.still = 0;
            if let Some(current) = adapter.filter(|_| seen.polls % GPU_POLLS == 0) {
                let now = gpu::choose(None);
                match now.adapter.filter(|a| now.reason == "high performance" && !a.same_gpu(current)) {
                    Some(chosen) => {
                        seen.other_gpu += 1;
                        if seen.other_gpu >= 2 {
                            return Some(Problem {
                                code: EXIT_GPU_CHANGED,
                                reason: format!("the high-performance GPU is now {}, and the engine renders on {}", chosen.name, current.name),
                            });
                        }
                    }
                    None => seen.other_gpu = 0,
                }
            }
        }
    }
    None
}

/// Logs the problem, tells the app, gives the main thread FORCE_EXIT_AFTER to
/// shut down in order, and ends the process if it has not by then.
fn end(problem: Problem, raise: &impl Fn(Problem)) {
    log!("[laurel] health: {}; the engine ends with code {} to be started again", problem.reason, problem.code);
    crate::rpc::notify("unhealthy", json!({ "reason": problem.reason, "code": problem.code }));
    let code = problem.code;
    let _ = std::thread::Builder::new().name("laurel: forced end".into()).spawn(move || {
        std::thread::sleep(FORCE_EXIT_AFTER);
        log!("[laurel] health: the engine did not shut down in {} s; ending it now", FORCE_EXIT_AFTER.as_secs());
        crate::crash::terminate(code as u32);
    });
    raise(problem);
}
