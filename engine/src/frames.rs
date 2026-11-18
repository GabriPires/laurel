//! A37 of the stability tests: in package 10, Minecraft on a laptop that encodes
//! with the copy encoder (A28) had stretches in which the encoder skipped 40.1% to
//! 95.4% of the frames, and engine.log showed it only in OBS's summary when the
//! recording stopped. While the replay buffer records, a thread samples OBS's
//! frame counters every 2 s and writes a line for each minute in which the encoder
//! skipped more than 5% of the frames, and one for the first minute back at 5% or
//! less (the rule is laurel_engine_core::frames), with what the recorder records
//! and the encoder. A normal session writes none.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laurel_engine_core::frames::{Minute, Report, Sample, SkipWatch, SKIPPED_ABOVE_PERCENT};

use crate::obs::{self, WeakOutput};

/// How often the watch samples the counters.
const POLL: Duration = Duration::from_secs(2);

pub struct Watch {
    /// What the recorder records and with which encoder, for the lines.
    description: Arc<Mutex<String>>,
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

/// Starts the watch on the replay buffer `output`.
pub fn start(output: WeakOutput) -> Watch {
    let description = Arc::new(Mutex::new(String::new()));
    let (stop, stopped) = channel::<()>();
    let recorded = description.clone();
    let thread = std::thread::Builder::new()
        .name("laurel: frame watch".into())
        .spawn(move || {
            let mut watch = SkipWatch::default();
            loop {
                match stopped.recv_timeout(POLL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
                let recording = output.state().is_some_and(|(active, _)| active);
                let recorded = recorded.lock().unwrap().clone();
                let sample = recording.then(|| Sample { counts: obs::frame_counts(), recorded: &recorded });
                if let Some(report) = watch.sample(Instant::now(), sample) {
                    log!("[laurel] frames: {}; recording {recorded}", report_text(&report));
                }
            }
        })
        .map_err(|e| log!("[laurel] no thread for the frame watch: {e}"))
        .ok();
    Watch { description, stop, thread }
}

fn report_text(report: &Report) -> String {
    let numbers = |m: &Minute| {
        format!(
            "{} of {} frames skipped by the encoder ({:.1}%) in the last {:.0} s, {} lagged by rendering",
            m.frames.skipped,
            m.frames.total,
            m.skipped_percent(),
            m.length.as_secs_f64(),
            m.frames.lagged
        )
    };
    match report {
        Report::Skipping(minute) => numbers(minute),
        Report::Recovered(minute) => format!("back at {SKIPPED_ABOVE_PERCENT}% or less, {}", numbers(minute)),
    }
}

impl Watch {
    /// What the recorder records now and with which encoder.
    pub fn set_description(&self, description: String) {
        *self.description.lock().unwrap() = description;
    }

    /// Ends the watch thread.
    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
