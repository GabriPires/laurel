//! Minimal MP4 reader for the library (spec library.browse): duration, video size,
//! frame rate and keyframe times (for the fast trim, spec edit.trim) from the `moov`
//! box, without decoding. Only box headers are read until `moov` is found, so large
//! clips cost a few reads.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Mp4Info {
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio_tracks: u32,
}

pub fn probe(path: &Path) -> Result<Mp4Info, String> {
    read_moov(path).map(|moov| parse_moov(&moov))
}

/// Presentation times in seconds of the video frames, in order, and of the
/// keyframes among them, the first frame at 0. The trim snaps to the frames, and
/// its fast mode starts at the keyframe at or before the chosen start.
#[derive(Clone, Debug, Default)]
pub struct VideoFrames {
    pub times: Vec<f64>,
    pub keyframes: Vec<f64>,
    /// How far decode times run behind presentation times (B-frames), in seconds:
    /// the first frame's composition offset.
    pub delay: f64,
}

pub fn video_frames(path: &Path) -> Result<VideoFrames, String> {
    let moov = read_moov(path)?;
    for (kind, trak) in boxes(&moov) {
        let Some(mdia) = child(trak, b"mdia").filter(|_| kind == b"trak") else { continue };
        if child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide".as_slice()) {
            continue;
        }
        let (scale, _) = child(mdia, b"mdhd").map(timing).unwrap_or((0, 0));
        let stbl = child(mdia, b"minf").and_then(|m| child(m, b"stbl")).ok_or("no sample table")?;
        // Decode time of every sample from the time-to-sample table.
        let stts = child(stbl, b"stts").unwrap_or_default();
        let mut dts = Vec::new();
        let mut t = 0u64;
        for i in 0..be32(stts, 4) as usize {
            let (count, delta) = (be32(stts, 8 + i * 8), be32(stts, 12 + i * 8));
            for _ in 0..count {
                dts.push(t);
                t += delta;
            }
        }
        // Composition offsets (B-frames), signed in version 1.
        let mut cts = vec![0i64; dts.len()];
        if let Some(ctts) = child(stbl, b"ctts") {
            let signed = ctts.first() == Some(&1);
            let mut n = 0;
            for i in 0..be32(ctts, 4) as usize {
                let (count, raw) = (be32(ctts, 8 + i * 8), be32(ctts, 12 + i * 8));
                let offset = if signed { raw as u32 as i32 as i64 } else { raw as i64 };
                for _ in 0..count {
                    if let Some(c) = cts.get_mut(n) {
                        *c = offset;
                    }
                    n += 1;
                }
            }
        }
        let first = cts.first().copied().unwrap_or(0);
        let time = |i: usize| (dts[i] as i64 + cts[i] - first).max(0) as f64 / scale.max(1) as f64;
        let mut times: Vec<f64> = (0..dts.len()).map(time).collect();
        // Without a sync sample table every sample is a keyframe.
        let keyframes = match child(stbl, b"stss") {
            Some(stss) => (0..be32(stss, 4) as usize).map(|i| be32(stss, 8 + i * 4) as usize).filter(|n| *n >= 1 && *n <= dts.len()).map(|n| time(n - 1)).collect(),
            None => times.clone(),
        };
        times.sort_by(f64::total_cmp);
        return Ok(VideoFrames { times, keyframes, delay: first.max(0) as f64 / scale.max(1) as f64 });
    }
    Err("no video track".into())
}

fn read_moov(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut pos = 0;
    while pos + 8 <= len {
        file.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).map_err(|e| e.to_string())?;
        let mut size = u32::from_be_bytes(head[0..4].try_into().unwrap()) as u64;
        let mut header = 8;
        if size == 1 {
            file.read_exact(&mut head[8..16]).map_err(|e| e.to_string())?;
            size = u64::from_be_bytes(head[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < header {
            break;
        }
        if &head[4..8] == b"moov" {
            let mut moov = vec![0u8; (size - header) as usize];
            file.read_exact(&mut moov).map_err(|e| e.to_string())?;
            return Ok(moov);
        }
        pos += size;
    }
    Err("no moov box".into())
}

/// Child boxes of a box payload as (type, payload).
fn boxes(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos + 8 > data.len() {
            return None;
        }
        let mut size = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        let kind = &data[pos + 4..pos + 8];
        let mut header = 8;
        if size == 1 && pos + 16 <= data.len() {
            size = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().ok()?) as usize;
            header = 16;
        } else if size == 0 {
            size = data.len() - pos;
        }
        if size < header || pos + size > data.len() {
            return None;
        }
        let payload = &data[pos + header..pos + size];
        pos += size;
        Some((kind, payload))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| *k == kind).map(|(_, p)| p)
}

fn be32(d: &[u8], at: usize) -> u64 {
    d.get(at..at + 4).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()) as u64)
}

fn be64(d: &[u8], at: usize) -> u64 {
    d.get(at..at + 8).map_or(0, |b| u64::from_be_bytes(b.try_into().unwrap()))
}

/// (timescale, duration) of an `mvhd` or `mdhd` payload, version 0 or 1.
fn timing(d: &[u8]) -> (u64, u64) {
    if d.first() == Some(&1) {
        (be32(d, 20), be64(d, 24))
    } else {
        (be32(d, 12), be32(d, 16))
    }
}

fn parse_moov(moov: &[u8]) -> Mp4Info {
    let mut info = Mp4Info::default();
    if let Some(mvhd) = child(moov, b"mvhd") {
        let (scale, duration) = timing(mvhd);
        if scale > 0 {
            info.duration_ms = duration * 1000 / scale;
        }
    }
    for (kind, trak) in boxes(moov) {
        if kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else { continue };
        let handler = child(mdia, b"hdlr").and_then(|h| h.get(8..12)).unwrap_or_default();
        if handler == b"soun" {
            info.audio_tracks += 1;
        }
        if handler != b"vide" {
            continue;
        }
        if let Some(tkhd) = child(trak, b"tkhd") {
            let at = if tkhd.first() == Some(&1) { 88 } else { 76 };
            info.width = (be32(tkhd, at) >> 16) as u32;
            info.height = (be32(tkhd, at + 4) >> 16) as u32;
        }
        // Frame rate from the most common frame duration in the time-to-sample
        // table; samples divided by duration drifts on short clips.
        let (scale, _) = child(mdia, b"mdhd").map(timing).unwrap_or((0, 0));
        let stts = child(mdia, b"minf").and_then(|m| child(m, b"stbl")).and_then(|s| child(s, b"stts")).unwrap_or_default();
        let delta = (0..be32(stts, 4) as usize).map(|i| (be32(stts, 8 + i * 8), be32(stts, 12 + i * 8))).max_by_key(|(count, _)| *count).map_or(0, |(_, d)| d);
        if scale > 0 && delta > 0 {
            info.fps = scale as f64 / delta as f64;
        }
    }
    info
}

/// Width and height from a PNG header.
pub fn png_size(path: &Path) -> Option<(u32, u32)> {
    let mut head = [0u8; 24];
    File::open(path).ok()?.read_exact(&mut head).ok()?;
    (&head[..8] == b"\x89PNG\r\n\x1a\n").then(|| (be32(&head, 16) as u32, be32(&head, 20) as u32))
}
