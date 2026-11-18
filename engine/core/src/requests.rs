//! A38 of the stability tests: the watch on the request the engine is answering.
//! The engine answers the app's requests one at a time on one thread; when one
//! has run for `STUCK_AFTER`, the engine writes the stack of each of its threads
//! to its log, once for that request, so that a thread stuck for good shows
//! where. The app gives up on a clip request at the same 40 s.

use std::time::{Duration, Instant};

/// How long a request runs before the watch reports it.
pub const STUCK_AFTER: Duration = Duration::from_secs(40);

/// A request that has run for `STUCK_AFTER` or longer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stuck {
    pub method: String,
    pub running_for: Duration,
}

/// The request being answered, if one is, and whether it was reported.
#[derive(Debug, Default)]
pub struct RequestWatch {
    running: Option<Running>,
}

#[derive(Debug)]
struct Running {
    method: String,
    since: Instant,
    reported: bool,
}

impl RequestWatch {
    /// The engine began answering `method` at `now`.
    pub fn started(&mut self, method: &str, now: Instant) {
        self.running = Some(Running { method: method.to_string(), since: now, reported: false });
    }

    /// The engine answered it.
    pub fn finished(&mut self) {
        self.running = None;
    }

    /// The request being answered when it has run for `STUCK_AFTER` or more and
    /// was not reported yet; each request is reported once.
    pub fn check(&mut self, now: Instant) -> Option<Stuck> {
        let running = self.running.as_mut().filter(|r| !r.reported)?;
        let running_for = now.saturating_duration_since(running.since);
        if running_for < STUCK_AFTER {
            return None;
        }
        running.reported = true;
        Some(Stuck { method: running.method.clone(), running_for })
    }
}
