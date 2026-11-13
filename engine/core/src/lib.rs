//! The engine's logic that needs neither Windows nor libobs: what it reads in OBS's
//! log lines and decides from them, moved here from `engine/src` area by area so
//! that it runs and is tested on any platform. `engine/src` keeps the shell: libobs,
//! the Windows calls and the threads.
//!
//! The tests live in `tests/` and never in `src/`: the public repository gets this
//! crate without them.
