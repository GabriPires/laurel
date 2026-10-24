//! FFmpeg helpers for the library: thumbnails for files without one (spec
//! library.thumbnails), the frames of the trim timeline, and trims (spec edit.trim):
//! fast without re-encoding, or exact with the video re-encoded.
//! ffmpeg.exe is FFmpeg's own program built by tools/ffmpeg-build.py, which uses the
//! FFmpeg libraries of the OBS runtime, so it lives next to the engine.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// LAUREL_FFMPEG, else ffmpeg.exe next to the engine, in the runtime's bin\64bit.
pub fn find_ffmpeg(engine: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LAUREL_FFMPEG") {
        return Some(PathBuf::from(p));
    }
    Some(engine.parent()?.join("ffmpeg.exe")).filter(|p| p.exists())
}

fn run(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = Command::new(ffmpeg)
        .args(["-hide_banner", "-v", "error", "-y"])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run ffmpeg: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// A 384-pixel-wide PNG of the frame at `at_secs`.
pub fn thumbnail(ffmpeg: &Path, input: &Path, output: &Path, at_secs: f64) -> Result<(), String> {
    let at = format!("{at_secs:.3}");
    run(ffmpeg, &["-ss".as_ref(), at.as_ref(), "-i".as_ref(), input.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), "scale=384:-2".as_ref(), output.as_os_str()])
}

/// Small JPEG frames at `times` (seconds), `width` wide, for the trim timeline;
/// four ffmpeg processes at a time.
pub fn frames(ffmpeg: &Path, input: &Path, outputs: &[(f64, std::path::PathBuf)], width: u32) -> Result<(), String> {
    let scale = format!("scale={width}:-2");
    for chunk in outputs.chunks(4) {
        let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(at, out)| {
                    let at = format!("{at:.3}");
                    let scale = scale.as_str();
                    scope.spawn(move || {
                        run(ffmpeg, &["-ss".as_ref(), at.as_ref(), "-i".as_ref(), input.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), scale.as_ref(), "-q:v".as_ref(), "5".as_ref(), out.as_os_str()])
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("frame thread panicked".into()))).collect()
        });
        results.into_iter().collect::<Result<Vec<_>, _>>()?;
    }
    Ok(())
}

/// Re-encodes `start..end` seconds so the copy starts exactly there: the video with
/// NVENC, or x264 when NVENC is not available, every audio track to AAC.
///
/// The app snaps `start` to a frame's time and `end` to the end of the last frame
/// to keep; half a millisecond earlier on both keeps the first and the last frame
/// whatever the rounding (ffmpeg keeps the frames from `start` and before `end`).
pub fn trim_exact(ffmpeg: &Path, input: &Path, output: &Path, start: f64, end: f64) -> Result<(), String> {
    let (s, d) = (format!("{:.6}", (start - 0.0005).max(0.0)), format!("{:.6}", end - start));
    let encode = |video: &[&str]| {
        let mut args: Vec<&std::ffi::OsStr> = vec!["-ss".as_ref(), s.as_ref(), "-i".as_ref(), input.as_os_str(), "-t".as_ref(), d.as_ref(), "-map".as_ref(), "0".as_ref(), "-c:v".as_ref()];
        args.extend(video.iter().map(|a| std::ffi::OsStr::new(a)));
        args.extend(["-c:a".as_ref(), "aac".as_ref(), "-b:a".as_ref(), "160k".as_ref(), "-movflags".as_ref(), "+faststart".as_ref(), output.as_os_str()]);
        run(ffmpeg, &args)
    };
    encode(&["h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "19", "-b:v", "0"]).or_else(|nvenc| {
        let _ = std::fs::remove_file(output);
        encode(&["libx264", "-preset", "veryfast", "-crf", "18"]).map_err(|x264| format!("{nvenc}; {x264}"))
    })
}

/// Copies every stream without re-encoding, from the keyframe at or before `seek`
/// until the packets whose decode time reaches `to` (see Library::trim).
pub fn trim(ffmpeg: &Path, input: &Path, output: &Path, seek: f64, to: f64) -> Result<(), String> {
    let (s, e) = (format!("{seek:.6}"), format!("{to:.6}"));
    run(
        ffmpeg,
        &[
            "-ss".as_ref(),
            s.as_ref(),
            "-to".as_ref(),
            e.as_ref(),
            "-i".as_ref(),
            input.as_os_str(),
            "-map".as_ref(),
            "0".as_ref(),
            "-c".as_ref(),
            "copy".as_ref(),
            "-avoid_negative_ts".as_ref(),
            "make_zero".as_ref(),
            output.as_os_str(),
        ],
    )
}
