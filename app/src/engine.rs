//! Supervises the laurel-engine process: starts it, restarts it when it dies and
//! exchanges JSON-RPC messages with it over its stdin/stdout (engine/src/rpc.rs).
//! The engine's stderr (its log) goes to `<work-dir>/logs/engine.log`, each line with
//! the local time, and so does the way each engine ended (A19: engine.log keeps more
//! history than the app sessions kept in the logs package).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::log::log;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Quick failures in a row before the supervisor gives up (the UI can restart it).
const MAX_QUICK_FAILURES: u32 = 5;
/// A run longer than this resets the failure count.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// Exit status when Windows cannot load a DLL the engine needs: the Microsoft Visual
/// C++ runtime, which the OBS runtime requires, or a file of a damaged OBS runtime.
const STATUS_DLL_NOT_FOUND: u32 = 0xC000_0135;
/// How long `shutdown` waits for the line that tells how the engine ended (A21).
const EXIT_LINE_WAIT: Duration = Duration::from_secs(2);

/// "exit code 0xC0000005 (STATUS_ACCESS_VIOLATION)": codes past 0xFFFF in hex, as
/// Windows writes them, with the name of the known ones (A19).
fn describe_exit(status: &std::process::ExitStatus) -> String {
    let Some(code) = status.code().map(|c| c as u32) else {
        return status.to_string();
    };
    let name = match code {
        101 => Some("a Rust panic"),
        0xC000_0005 => Some("STATUS_ACCESS_VIOLATION"),
        0xC000_001D => Some("STATUS_ILLEGAL_INSTRUCTION"),
        0xC000_00FD => Some("STATUS_STACK_OVERFLOW"),
        0xC000_0135 => Some("STATUS_DLL_NOT_FOUND"),
        0xC000_013A => Some("STATUS_CONTROL_C_EXIT"),
        0xC000_0374 => Some("STATUS_HEAP_CORRUPTION"),
        0xC000_0409 => Some("STATUS_STACK_BUFFER_OVERRUN, a fast fail"),
        0x4001_0004 => Some("DBG_TERMINATE_PROCESS"),
        0x8000_0003 => Some("STATUS_BREAKPOINT"),
        0xE06D_7363 => Some("a C++ exception"),
        _ => None,
    };
    let code = if code > 0xFFFF { format!("0x{code:08X}") } else { code.to_string() };
    match name {
        Some(name) => format!("exit code {code} ({name})"),
        None => format!("exit code {code}"),
    }
}

pub struct EngineConfig {
    pub exe: PathBuf,
    pub work_dir: PathBuf,
    /// Options after `<work-dir> --rpc`, computed at every start so
    /// they follow the settings (for example the capture folder).
    pub options: Box<dyn Fn() -> Vec<String> + Send + Sync>,
}

type Reply = Result<Value, String>;

#[derive(Clone)]
pub struct Engine(Arc<Inner>);

struct Inner {
    app: AppHandle,
    cfg: EngineConfig,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    next_id: AtomicU64,
    state: Mutex<Value>,
    microphone: Mutex<Value>,
    quitting: AtomicBool,
    /// Set by `restart`: the next exit is asked for, not a failure.
    restarting: AtomicBool,
    /// Whether a supervisor thread runs (see `resume`).
    supervising: AtomicBool,
    /// Engines whose end engine.log has told, and a signal for each (A21).
    exits_logged: (Mutex<u64>, Condvar),
}

/// Clears `Inner::supervising` when the supervisor returns.
struct Supervising<'a>(&'a AtomicBool);

impl Drop for Supervising<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Engine {
    pub fn start(app: AppHandle, cfg: EngineConfig) -> Engine {
        let engine = Engine(Arc::new(Inner {
            app,
            cfg,
            stdin: Mutex::new(None),
            child: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            state: Mutex::new(json!({ "state": "starting" })),
            microphone: Mutex::new(json!({ "state": "starting" })),
            quitting: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            supervising: AtomicBool::new(false),
            exits_logged: (Mutex::new(0), Condvar::new()),
        }));
        let supervisor = engine.clone();
        std::thread::Builder::new().name("engine-supervisor".into()).spawn(move || supervisor.supervise()).unwrap();
        engine
    }

    /// `{ "state": "starting" | "ready" | "stopped" | "failed", ... }`; "ready" carries
    /// the engine's `ready` notification as `info`.
    pub fn state(&self) -> Value {
        self.0.state.lock().unwrap().clone()
    }

    /// The microphone's state from the engine's "microphone" notifications (A16 of the
    /// stability tests): `{ "state": "starting" | "ok" | "failed" | "denied" | "off",
    /// "code": <Windows error> }`. Each engine start begins at "starting".
    pub fn microphone(&self) -> Value {
        self.0.microphone.lock().unwrap().clone()
    }

    fn set_microphone(&self, value: Value) {
        *self.0.microphone.lock().unwrap() = value.clone();
        let _ = self.0.app.emit("microphone", value);
    }

    /// Sends a request and waits for its response.
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let id = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.0.pending.lock().unwrap().insert(id, tx);
        let sent = match self.0.stdin.lock().unwrap().as_mut() {
            Some(stdin) => {
                let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
                writeln!(stdin, "{msg}").and_then(|_| stdin.flush()).map_err(|e| format!("engine pipe: {e}"))
            }
            None => Err("the engine is not running".to_string()),
        };
        if let Err(e) = sent {
            self.0.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        rx.recv_timeout(timeout).unwrap_or_else(|_| {
            self.0.pending.lock().unwrap().remove(&id);
            Err(format!("the engine did not answer {method} in time"))
        })
    }

    /// Asks the engine to quit and stops supervising it; kills it if it hangs.
    ///
    /// A21 of the stability tests: the line that tells how the engine ended comes
    /// from the supervisor once it sees the exit, so Laurel, which closes (or runs
    /// an update's installer) right after this, waits a little for that line.
    /// Otherwise the last engine of every session looked like one that ended
    /// without a word (A5, A19).
    pub fn shutdown(&self) {
        if self.0.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        let running = matches!(self.0.child.lock().unwrap().as_mut().map(|c| c.try_wait()), Some(Ok(None)));
        let logged = *self.0.exits_logged.0.lock().unwrap();
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(5);
        let exited = loop {
            if !matches!(self.0.child.lock().unwrap().as_mut().map(|c| c.try_wait()), Some(Ok(None))) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if !exited {
            log!("engine did not exit after shutdown; killing it");
            self.kill();
        }
        if running {
            let (count, signal) = &self.0.exits_logged;
            let (_count, waited) = signal.wait_timeout_while(count.lock().unwrap(), EXIT_LINE_WAIT, |n| *n == logged).unwrap();
            if waited.timed_out() {
                log!("engine.log did not tell how the engine ended within {} s", EXIT_LINE_WAIT.as_secs());
            }
        }
    }

    /// Supervises the engine again after `shutdown`, when Laurel goes on after all
    /// (an update whose installer did not start).
    pub fn resume(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.supervising.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.0.supervising.load(Ordering::SeqCst) || !self.0.quitting.swap(false, Ordering::SeqCst) {
            return;
        }
        log!("engine resumed");
        let supervisor = self.clone();
        std::thread::Builder::new().name("engine-supervisor".into()).spawn(move || supervisor.supervise()).unwrap();
    }

    /// Terminates the engine process; the supervisor restarts it unless quitting.
    pub fn kill(&self) {
        if let Some(child) = self.0.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    /// Restarts the engine with fresh options (new settings), or restarts
    /// supervision after the supervisor gave up. A restart asked for is not counted
    /// as a failure and starts again at once.
    pub fn restart(&self) {
        if self.state()["state"] == "failed" {
            let supervisor = self.clone();
            std::thread::spawn(move || supervisor.supervise());
        } else {
            self.0.restarting.store(true, Ordering::SeqCst);
            self.kill();
        }
    }

    fn set_state(&self, state: Value) {
        log!("engine state: {state}");
        *self.0.state.lock().unwrap() = state.clone();
        let _ = self.0.app.emit("engine-state", state);
    }

    fn supervise(&self) {
        self.0.supervising.store(true, Ordering::SeqCst);
        let _supervising = Supervising(&self.0.supervising);
        let mut quick_failures = 0;
        let mut restarted = false;
        while !self.0.quitting.load(Ordering::SeqCst) {
            self.set_state(if restarted { json!({ "state": "starting", "restart": true }) } else { json!({ "state": "starting" }) });
            let started = Instant::now();
            let message = match self.run_once() {
                Ok(status) => format!("the engine exited ({status})"),
                Err(e) => e,
            };
            if self.0.quitting.load(Ordering::SeqCst) {
                break;
            }
            restarted = self.0.restarting.swap(false, Ordering::SeqCst);
            if restarted {
                log!("engine restarted to apply new settings");
                continue;
            }
            // A5: while Windows ends the session it also ends the engine (a console
            // program), and would end a new one at once. The supervisor waits, and
            // starts the engine again only if the session goes on; that end is not
            // counted as a failure.
            if crate::win::session_ending() {
                log!("engine not restarted: Windows is ending the session");
                self.set_state(json!({ "state": "stopped", "message": "Windows is ending the session" }));
                while crate::win::session_ending() && !self.0.quitting.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                }
                if self.0.quitting.load(Ordering::SeqCst) {
                    break;
                }
                log!("the session goes on: the engine starts again");
                restarted = true;
                continue;
            }
            quick_failures = if started.elapsed() > HEALTHY_RUN { 1 } else { quick_failures + 1 };
            if quick_failures >= MAX_QUICK_FAILURES {
                self.set_state(json!({ "state": "failed", "message": message }));
                return;
            }
            self.set_state(json!({ "state": "stopped", "message": message }));
            std::thread::sleep(Duration::from_secs(1 << quick_failures.min(4)));
        }
        self.set_state(json!({ "state": "stopped", "message": "Laurel is closing", "closing": true }));
    }

    /// Runs one engine process until it exits.
    fn run_once(&self) -> Result<String, String> {
        let cfg = &self.0.cfg;
        let options = (cfg.options)();
        self.set_microphone(json!({ "state": "starting" }));
        let mut child = Command::new(&cfg.exe)
            .arg(&cfg.work_dir)
            .arg("--rpc")
            .args(&options)
            .current_dir(cfg.exe.parent().unwrap_or(&cfg.work_dir))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", cfg.exe.display()))?;
        log!("engine started: pid {} ({} {:?})", child.id(), cfg.exe.display(), options);

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        *self.0.stdin.lock().unwrap() = child.stdin.take();
        *self.0.child.lock().unwrap() = Some(child);

        let reader = self.clone();
        let fatal = Arc::new(Mutex::new(None::<String>));
        let fatal_in = fatal.clone();
        let stdout_thread = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<Value>(&line) {
                    Ok(msg) => reader.on_message(msg, &fatal_in),
                    Err(e) => log!("engine sent a non-JSON line ({e}): {line}"),
                }
            }
        });
        let log_path = cfg.work_dir.join("logs").join("engine.log");
        let (exit_tx, exit_rx) = channel::<String>();
        let logger = self.clone();
        std::thread::spawn(move || {
            let mut file = crate::log::Rotating::open(&log_path, 0, crate::log::MAX_BYTES);
            file.line(&format!("[{}] ==== engine start", crate::win::clock()));
            // Bytes, not `lines()`: a line that is not UTF-8 would end the reading,
            // and with it the log of this engine.
            let mut stderr = BufReader::new(stderr);
            let mut buf = Vec::new();
            while stderr.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                let line = String::from_utf8_lossy(&buf);
                file.line(&format!("[{}] {}", crate::win::clock(), line.trim_end_matches(['\r', '\n'])));
                buf.clear();
            }
            // A19: how this engine ended, after its last line.
            if let Ok(exit) = exit_rx.recv_timeout(Duration::from_secs(5)) {
                file.line(&format!("[{}] ==== engine exited: {exit}", crate::win::clock()));
            }
            let (count, signal) = &logger.0.exits_logged;
            *count.lock().unwrap() += 1;
            signal.notify_all();
        });

        let exited = loop {
            let polled = self.0.child.lock().unwrap().as_mut().map(|c| c.try_wait());
            match polled {
                Some(Ok(Some(status))) => break Ok(status),
                Some(Ok(None)) => std::thread::sleep(Duration::from_millis(200)),
                Some(Err(e)) => break Err(format!("waiting for the engine: {e}")),
                None => break Err("the engine handle was lost".to_string()),
            }
        };
        *self.0.stdin.lock().unwrap() = None;
        let _ = stdout_thread.join();
        for (_, tx) in self.0.pending.lock().unwrap().drain() {
            let _ = tx.send(Err("the engine stopped".into()));
        }
        let status = exited?;
        let exit = describe_exit(&status);
        log!("engine exited: {exit}");
        let _ = exit_tx.send(exit);
        let fatal = fatal.lock().unwrap().take().or_else(|| {
            (status.code().map(|c| c as u32) == Some(STATUS_DLL_NOT_FOUND))
                .then(|| "a DLL is missing: install the Microsoft Visual C++ Redistributable (x64) or reinstall Laurel".to_string())
        });
        Ok(match fatal {
            Some(msg) => format!("{status}: {msg}"),
            None => status.to_string(),
        })
    }

    fn on_message(&self, msg: Value, fatal: &Mutex<Option<String>>) {
        if let Some(id) = msg.get("id").and_then(Value::as_u64) {
            if let Some(tx) = self.0.pending.lock().unwrap().remove(&id) {
                let reply = match msg.get("error") {
                    Some(err) => Err(err.get("message").and_then(Value::as_str).unwrap_or("engine error").to_string()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(reply);
            }
            return;
        }
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match msg.get("method").and_then(Value::as_str) {
            Some("ready") => self.set_state(json!({ "state": "ready", "info": params })),
            // The video encoder changed while resuming (spec quality.encoding).
            Some("encoder") => {
                let mut state = self.state();
                if state["state"] == "ready" {
                    for (key, value) in params.as_object().into_iter().flatten() {
                        state["info"][key] = value.clone();
                    }
                    self.set_state(state);
                }
            }
            // A16: the microphone started, failed, or Windows denies it.
            Some("microphone") => {
                log!("engine microphone: {params}");
                self.set_microphone(params);
            }
            Some("fatal") => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                log!("engine fatal error: {text}");
                *fatal.lock().unwrap() = Some(text);
            }
            _ => log!("engine notification: {msg}"),
        }
    }
}
