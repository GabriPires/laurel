//! Where the app finds ffmpeg.exe, FFmpeg's own program built by
//! tools/ffmpeg-build.py on the OBS runtime's FFmpeg libraries, so it lives next to
//! the engine. The command lines are laurel_app_core::media, and running it is
//! laurel_app_win::run_ffmpeg.

use std::path::{Path, PathBuf};

use laurel_app_core::media::Ffmpeg;

/// LAUREL_FFMPEG, else ffmpeg.exe next to the engine, in the runtime's bin\64bit.
pub fn find_ffmpeg(engine: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LAUREL_FFMPEG") {
        return Some(PathBuf::from(p));
    }
    Some(engine.parent()?.join("ffmpeg.exe")).filter(|p| p.exists())
}

/// FFmpeg for the library, run from `program` without a console window.
pub fn ffmpeg(program: PathBuf) -> Ffmpeg {
    Ffmpeg::new(move |args| crate::win::run_ffmpeg(&program, args))
}
