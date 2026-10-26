//! Supervises the laurel-engine process: starts it, restarts it when it dies and
//! exchanges JSON-RPC messages with it over its stdin/stdout (engine/src/rpc.rs).
//! The engine's stderr (its log) goes to `<work-dir>/logs/engine.log`, each line with
//! the local time.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
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
    quitting: AtomicBool,
    /// Set by `restart`: the next exit is asked for, not a failure.
    restarting: AtomicBool,
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
            quitting: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
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
    pub fn shutdown(&self) {
        if self.0.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.0.child.lock().unwrap().as_mut().map(|c| c.try_wait()) {
                Some(Ok(None)) => {}
                _ => return,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        log!("engine did not exit after shutdown; killing it");
        self.kill();
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
        std::thread::spawn(move || {
            let mut file = crate::log::Rotating::open(&log_path, 0, crate::log::MAX_BYTES);
            file.line(&format!("[{}] ==== engine start", crate::win::clock()));
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                file.line(&format!("[{}] {line}", crate::win::clock()));
            }
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
        log!("engine exited: {status}");
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
            Some("fatal") => {
                let text = params.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                log!("engine fatal error: {text}");
                *fatal.lock().unwrap() = Some(text);
            }
            _ => log!("engine notification: {msg}"),
        }
    }
}
