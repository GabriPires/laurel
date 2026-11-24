//! FFmpeg for the library: thumbnails for files without one (spec
//! library.thumbnails), the frames of the trim timeline, and trims (spec edit.trim):
//! fast without re-encoding, or exact with the video re-encoded. The library asks
//! for these through the [`Media`] trait; [`Ffmpeg`] is the real one, which builds
//! FFmpeg's command lines and hands them to a function that runs the program.
//! ffmpeg.exe is FFmpeg's own program built by tools/ffmpeg-build.py, which uses the
//! FFmpeg libraries of the OBS runtime, so it lives next to the engine; the app runs
//! it without a console window (laurel_app_win::run_ffmpeg).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// What the library asks of FFmpeg. Each call writes its output file, replacing one
/// already there, and returns an error with FFmpeg's reason when it cannot.
pub trait Media: Send + Sync {
    /// A 384-pixel-wide PNG of the frame at `at_secs`.
    fn thumbnail(&self, input: &Path, output: &Path, at_secs: f64) -> Result<(), String>;
    /// Small JPEG frames at the given times (seconds), `width` wide, for the trim
    /// timeline.
    fn frames(&self, input: &Path, outputs: &[(f64, PathBuf)], width: u32) -> Result<(), String>;
    /// Copies every stream without re-encoding, from the keyframe at or before
    /// `seek` until the packets whose decode time reaches `to` (see Library::trim).
    fn trim(&self, input: &Path, output: &Path, seek: f64, to: f64) -> Result<(), String>;
    /// Re-encodes `start..end` seconds so the copy starts exactly there, the video
    /// at most at `max_bitrate` bits per second when given.
    fn trim_exact(&self, input: &Path, output: &Path, start: f64, end: f64, max_bitrate: Option<u64>) -> Result<(), String>;
}

/// The real [`Media`]: FFmpeg's command lines, each given to `run`, which runs
/// FFmpeg's program with them and says whether it ended well, else why not.
pub struct Ffmpeg {
    run: Box<dyn Fn(&[OsString]) -> Result<(), String> + Send + Sync>,
}

/// The options every run of FFmpeg's program takes, before the others: no banner,
/// only errors on its error output, and an output already there replaced.
const COMMON_ARGS: [&str; 4] = ["-hide_banner", "-v", "error", "-y"];

/// The video encoders of the exact trim, in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    Nvenc,
    X264,
}

fn args<'a>(parts: impl IntoIterator<Item = &'a OsStr>) -> Vec<OsString> {
    parts.into_iter().map(OsStr::to_os_string).collect()
}

/// The thumbnail's arguments: the frame at `at_secs`, 384 pixels wide.
pub fn thumbnail_args(input: &Path, output: &Path, at_secs: f64) -> Vec<OsString> {
    let at = format!("{at_secs:.3}");
    args(["-ss".as_ref(), at.as_ref(), "-i".as_ref(), input.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), "scale=384:-2".as_ref(), output.as_os_str()])
}

/// The arguments of one timeline frame: the frame at `at_secs`, `width` wide.
pub fn frame_args(input: &Path, output: &Path, at_secs: f64, width: u32) -> Vec<OsString> {
    let (at, scale) = (format!("{at_secs:.3}"), format!("scale={width}:-2"));
    args(["-ss".as_ref(), at.as_ref(), "-i".as_ref(), input.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), scale.as_ref(), "-q:v".as_ref(), "5".as_ref(), output.as_os_str()])
}

/// The fast trim's arguments: every stream copied from the keyframe at or before
/// `seek` until the packets whose decode time reaches `to`.
pub fn trim_args(input: &Path, output: &Path, seek: f64, to: f64) -> Vec<OsString> {
    let (s, e) = (format!("{seek:.6}"), format!("{to:.6}"));
    args([
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
    ])
}

/// The exact trim's arguments with one encoder: `start..end` seconds re-encoded,
/// the video with `encoder` at the quality asked, every audio track to AAC.
/// `max_bitrate` (bits per second, the original's) caps the video: at the quality
/// asked, a busy game came out at twice the recording's rate or more (stability
/// tests, finding A4), and a copy cannot hold more detail than its original.
///
/// The app snaps `start` to a frame's time and `end` to the end of the last frame
/// to keep; half a millisecond earlier on both keeps the first and the last frame
/// whatever the rounding (ffmpeg keeps the frames from `start` and before `end`).
pub fn trim_exact_args(input: &Path, output: &Path, start: f64, end: f64, max_bitrate: Option<u64>, encoder: Encoder) -> Vec<OsString> {
    let (s, d) = (format!("{:.6}", (start - 0.0005).max(0.0)), format!("{:.6}", end - start));
    // A one-second buffer: with two, x264 came out 10% above the original on a
    // 4 s trim of a noisy clip; NVENC stayed at the original's rate either way.
    let cap = max_bitrate.filter(|b| *b > 0).map(|b| b.to_string());
    let video: &[&str] = match encoder {
        Encoder::Nvenc => &["h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "19", "-b:v", "0"],
        Encoder::X264 => &["libx264", "-preset", "veryfast", "-crf", "18"],
    };
    let mut parts: Vec<&OsStr> = vec!["-ss".as_ref(), s.as_ref(), "-i".as_ref(), input.as_os_str(), "-t".as_ref(), d.as_ref(), "-map".as_ref(), "0".as_ref(), "-c:v".as_ref()];
    parts.extend(video.iter().map(|a| OsStr::new(a)));
    if let Some(rate) = &cap {
        parts.extend(["-maxrate", rate, "-bufsize", rate].map(OsStr::new));
    }
    parts.extend(["-c:a".as_ref(), "aac".as_ref(), "-b:a".as_ref(), "160k".as_ref(), "-movflags".as_ref(), "+faststart".as_ref(), output.as_os_str()]);
    args(parts)
}

impl Ffmpeg {
    pub fn new(run: impl Fn(&[OsString]) -> Result<(), String> + Send + Sync + 'static) -> Ffmpeg {
        Ffmpeg { run: Box::new(run) }
    }

    /// Runs FFmpeg's program with the common options, then `args`.
    fn run(&self, args: &[OsString]) -> Result<(), String> {
        let line: Vec<OsString> = COMMON_ARGS.iter().map(OsString::from).chain(args.iter().cloned()).collect();
        (self.run)(&line)
    }
}

impl Media for Ffmpeg {
    fn thumbnail(&self, input: &Path, output: &Path, at_secs: f64) -> Result<(), String> {
        self.run(&thumbnail_args(input, output, at_secs))
    }

    /// Four ffmpeg processes at a time.
    fn frames(&self, input: &Path, outputs: &[(f64, PathBuf)], width: u32) -> Result<(), String> {
        for chunk in outputs.chunks(4) {
            let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
                let handles: Vec<_> = chunk.iter().map(|(at, out)| scope.spawn(move || self.run(&frame_args(input, out, *at, width)))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("frame thread panicked".into()))).collect()
            });
            results.into_iter().collect::<Result<Vec<_>, _>>()?;
        }
        Ok(())
    }

    fn trim(&self, input: &Path, output: &Path, seek: f64, to: f64) -> Result<(), String> {
        self.run(&trim_args(input, output, seek, to))
    }

    /// The video with NVENC, or x264 when NVENC is not available.
    fn trim_exact(&self, input: &Path, output: &Path, start: f64, end: f64, max_bitrate: Option<u64>) -> Result<(), String> {
        self.run(&trim_exact_args(input, output, start, end, max_bitrate, Encoder::Nvenc)).or_else(|nvenc| {
            let _ = std::fs::remove_file(output);
            self.run(&trim_exact_args(input, output, start, end, max_bitrate, Encoder::X264)).map_err(|x264| format!("{nvenc}; {x264}"))
        })
    }
}
