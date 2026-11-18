//! A38 of the stability tests: the watch on the app's requests. The engine answers
//! them one at a time on its main thread (rpc.rs); in package 10 that thread
//! stopped after a clip's file was written, and nothing in engine.log said where.
//! When a request has run for 40 s, a thread of its own writes the stack of every
//! other thread of the engine to engine.log (crash.rs), once for that request
//! (the rule is laurel_engine_core::requests). The app gives up on a clip request
//! at the same 40 s and restarts an engine that does not answer its ping either.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laurel_engine_core::requests::RequestWatch;

use crate::crash;

/// How often the watch looks at the request being answered.
const POLL: Duration = Duration::from_secs(1);

pub struct Watch {
    requests: Arc<Mutex<RequestWatch>>,
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

/// Starts the watch thread.
pub fn start() -> Watch {
    let requests = Arc::new(Mutex::new(RequestWatch::default()));
    let (stop, stopped) = channel::<()>();
    let watched = requests.clone();
    let thread = std::thread::Builder::new()
        .name("laurel: request watch".into())
        .spawn(move || loop {
            match stopped.recv_timeout(POLL) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
            // The lock is not held while the stacks are written: the request
            // thread takes it when its request ends.
            let stuck = watched.lock().unwrap().check(Instant::now());
            if let Some(stuck) = stuck {
                crash::write_all_stacks(&format!("the request \"{}\" has run for {:.1} s", stuck.method, stuck.running_for.as_secs_f64()));
            }
        })
        .map_err(|e| log!("[laurel] no thread for the request watch: {e}"))
        .ok();
    Watch { requests, stop, thread }
}

impl Watch {
    /// The engine begins answering `method`.
    pub fn started(&self, method: &str) {
        self.requests.lock().unwrap().started(method, Instant::now());
    }

    /// The engine answered it.
    pub fn finished(&self) {
        self.requests.lock().unwrap().finished();
    }

    /// Ends the watch thread.
    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
