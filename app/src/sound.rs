//! Alert sounds (spec alerts.sound). The tones are synthesized as small WAV images
//! and played with the Windows PlaySound API, so they do not depend on a webview
//! and also play over exclusive fullscreen games.

use std::f32::consts::TAU;

const RATE: u32 = 44_100;

#[derive(Clone, Copy)]
pub enum Alert {
    /// Two rising notes (A5, E6).
    Clip,
    /// One short high note (G6).
    Screenshot,
    /// Two falling low notes (G4, E-flat 4).
    Error,
}

impl Alert {
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Alert::Clip => &[(880.0, 90), (1318.5, 170)],
            Alert::Screenshot => &[(1568.0, 80)],
            Alert::Error => &[(392.0, 150), (311.1, 240)],
        }
    }
}

/// The three alert sounds rendered at one volume.
pub struct Sounds {
    clip: &'static [u8],
    screenshot: &'static [u8],
    error: &'static [u8],
}

impl Sounds {
    /// `volume` is 0 to 100. The images are leaked on purpose: PlaySound reads them
    /// asynchronously, and they are only rebuilt when the volume changes.
    pub fn new(volume: u8) -> Sounds {
        let gain = (volume.min(100) as f32 / 100.0).powi(2);
        let render = |a: Alert| -> &'static [u8] { Box::leak(wav(a.notes(), gain).into_boxed_slice()) };
        Sounds { clip: render(Alert::Clip), screenshot: render(Alert::Screenshot), error: render(Alert::Error) }
    }

    pub fn play(&self, alert: Alert) {
        crate::win::play_wav(match alert {
            Alert::Clip => self.clip,
            Alert::Screenshot => self.screenshot,
            Alert::Error => self.error,
        });
    }
}

/// 16-bit mono PCM: each note is a sine with a soft second harmonic, a 5 ms attack
/// and an exponential decay, followed by 20 ms of silence.
fn wav(notes: &[(f32, u32)], gain: f32) -> Vec<u8> {
    let mut pcm: Vec<i16> = Vec::new();
    for &(freq, ms) in notes {
        let n = RATE * ms / 1000;
        let length = ms as f32 / 1000.0;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let envelope = (t / 0.005).min(1.0) * (-4.0 * t / length).exp();
            let wave = 0.8 * (TAU * freq * t).sin() + 0.2 * (TAU * 2.0 * freq * t).sin();
            pcm.push((wave * envelope * gain * 0.5 * i16::MAX as f32) as i16);
        }
    }
    pcm.extend(std::iter::repeat(0).take((RATE / 50) as usize));

    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}
