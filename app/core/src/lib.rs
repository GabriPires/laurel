//! The app's logic that needs neither Windows nor Tauri: its rules, and the
//! orchestration that drives them through traits, moved here from `app/src` area by
//! area so that they run and are tested on any platform. `app/src` keeps the shell:
//! the Windows calls, Tauri, the engine process and the threads.
//!
//! The tests live in `tests/` and never in `src/`: the public repository gets this
//! crate without them.

pub mod games;
pub mod hotkeys;
pub mod stores;
pub mod clip;
