//! A37 of the stability tests: the frames the video encoder skips while the
//! replay buffer records. The engine samples OBS's counters every few seconds
//! (engine/src/frames.rs); for each minute of recording, the watch reports one in
//! which the encoder skipped more than 5% of the frames, and the first minute back
//! at 5% or less after one, and never anything outside the recording.

use std::time::{Duration, Instant};

/// OBS's frame counters at one moment: the frames the encoder skipped and the
/// video output's frames, both since the encoder last started, and the frames
/// lagged by rendering, since libobs started. Over a minute, what they grew by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    pub skipped: u32,
    pub total: u32,
    pub lagged: u32,
}

impl FrameCounts {
    /// What the counters grew by since `start`, or None when one is lower: OBS
    /// zeroed them when the encoder started again.
    fn since(self, start: FrameCounts) -> Option<FrameCounts> {
        Some(FrameCounts {
            skipped: self.skipped.checked_sub(start.skipped)?,
            total: self.total.checked_sub(start.total)?,
            lagged: self.lagged.checked_sub(start.lagged)?,
        })
    }
}

/// One sample while the replay buffer records: OBS's counters, and what the
/// recorder records and with which encoder (the description the lines give).
#[derive(Clone, Copy, Debug)]
pub struct Sample<'a> {
    pub counts: FrameCounts,
    pub recorded: &'a str,
}

/// One minute of recording: what the counters grew by, and how long it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Minute {
    pub frames: FrameCounts,
    pub length: Duration,
}

impl Minute {
    /// The share of the minute's frames the encoder skipped, in percent.
    pub fn skipped_percent(&self) -> f64 {
        if self.frames.total == 0 {
            return 0.0;
        }
        f64::from(self.frames.skipped) * 100.0 / f64::from(self.frames.total)
    }

    /// More than 5% skipped, counted in whole frames so that exactly 5% is not.
    fn above(&self) -> bool {
        u64::from(self.frames.skipped) * 100 > u64::from(self.frames.total) * SKIPPED_ABOVE_PERCENT
    }
}

/// A line for the engine's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// The encoder skipped more than 5% of the minute's frames.
    Skipping(Minute),
    /// The first minute at 5% or less after one above.
    Recovered(Minute),
}

const MINUTE: Duration = Duration::from_secs(60);
/// More than this share of a minute's frames skipped, in percent, makes a line.
pub const SKIPPED_ABOVE_PERCENT: u64 = 5;

/// The watch on the skipped frames.
#[derive(Debug, Default)]
pub struct SkipWatch {
    minute: Option<Begun>,
    /// Whether the last minute was above 5%.
    above: bool,
}

/// When the current minute began, the counters then, and what was recorded.
#[derive(Debug)]
struct Begun {
    at: Instant,
    counts: FrameCounts,
    recorded: String,
}

impl SkipWatch {
    /// The sample at `now`, or None when the replay buffer is not recording.
    pub fn sample(&mut self, now: Instant, sample: Option<Sample>) -> Option<Report> {
        // Outside the recording nothing is said, and a minute that a stop cut
        // short is not reported; the next recording starts afresh.
        let Some(Sample { counts, recorded }) = sample else {
            *self = SkipWatch::default();
            return None;
        };
        // The first sample of a recording begins a minute, and so does one with
        // a counter lower than at the minute's start (OBS zeroed them when the
        // encoder started again between two samples), or one of something else
        // recorded, so that a minute's line names what was recorded in all of it.
        let grown = self.minute.as_ref().filter(|m| m.recorded == recorded).and_then(|m| Some((m.at, counts.since(m.counts)?)));
        let Some((since, frames)) = grown else {
            self.minute = Some(Begun { at: now, counts, recorded: recorded.to_string() });
            return None;
        };
        let length = now.saturating_duration_since(since);
        if length < MINUTE {
            return None;
        }
        self.minute = Some(Begun { at: now, counts, recorded: recorded.to_string() });
        // A minute without frames says nothing: the video output was held, not
        // back to normal.
        if frames.total == 0 {
            return None;
        }
        let minute = Minute { frames, length };
        let was_above = std::mem::replace(&mut self.above, minute.above());
        match (self.above, was_above) {
            (true, _) => Some(Report::Skipping(minute)),
            (false, true) => Some(Report::Recovered(minute)),
            (false, false) => None,
        }
    }
}
