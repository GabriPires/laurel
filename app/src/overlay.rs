//! In-game notices (spec overlay.in-game) and alert sounds (spec alerts.sound).
//!
//! The overlay is the transparent `overlay` window (ui/overlay.html): click-through,
//! never activated, topmost and excluded from screen capture. It is shown only
//! while notices are on screen, at the top right of the foreground window (the
//! game), and hidden again when idle so it does not sit over the game for nothing.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::log::log;
use crate::settings::{Settings, Store};
use crate::sound::{Alert, Sounds};
use crate::win;

/// Logical size of the overlay window; notices stack inside it from the top right.
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 420.0;
const MARGIN: f64 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Clip,
    /// A clip Laurel saved by itself (an achievement).
    AutoClip,
    Screenshot,
    Info,
    Error,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Clip => "clip",
            Kind::AutoClip => "auto-clip",
            Kind::Screenshot => "screenshot",
            Kind::Info => "info",
            Kind::Error => "error",
        }
    }

    fn alert(self) -> Option<Alert> {
        match self {
            Kind::Clip => Some(Alert::Clip),
            Kind::AutoClip => Some(Alert::AutoClip),
            Kind::Screenshot => Some(Alert::Screenshot),
            Kind::Error => Some(Alert::Error),
            Kind::Info => None,
        }
    }

    /// How long a notice stays; the window hides `EXIT` after the last one. While
    /// the window is visible the game leaves independent flip (measured on
    /// 2026-10-26), so clip and screenshot notices keep it 1.5 s in all.
    fn hold(self) -> Duration {
        Duration::from_millis(match self {
            Kind::Clip | Kind::AutoClip | Kind::Screenshot => 1300,
            Kind::Info => 4000,
            Kind::Error => 6000,
        })
    }
}

/// The notice's exit animation (ui/overlay.css, 180 ms) and a little slack.
const EXIT: Duration = Duration::from_millis(200);

#[derive(Clone)]
pub struct Notifier(Arc<Inner>);

struct Inner {
    app: AppHandle,
    store: Arc<Store>,
    /// The alert sounds and the volume they were rendered at.
    sounds: Mutex<(u8, Sounds)>,
    hwnd: AtomicIsize,
    /// Whether Windows keeps the overlay out of screen capture. On some Windows 10
    /// PCs it does not (A10 of the stability tests); the overlay then shows only
    /// critical notices, which come while Laurel is not recording the game, so no
    /// clip can hold them.
    excluded: AtomicBool,
    next_id: AtomicU64,
    /// The overlay hides once the last notice on screen has expired.
    visible_until: Mutex<Instant>,
}

impl Notifier {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Notifier {
        let volume = store.get().volume;
        Notifier(Arc::new(Inner {
            app,
            store,
            sounds: Mutex::new((volume, Sounds::new(volume))),
            hwnd: AtomicIsize::new(0),
            excluded: AtomicBool::new(true),
            next_id: AtomicU64::new(1),
            visible_until: Mutex::new(Instant::now()),
        }))
    }

    /// Turns the `overlay` window into a click-through, non-activating overlay.
    /// LAUREL_OVERLAY_CAPTURABLE=1 leaves it visible to screen capture (tests only).
    pub fn init_window(&self) -> Result<(), String> {
        let window = self.0.app.get_webview_window("overlay").ok_or("no overlay window")?;
        window.set_ignore_cursor_events(true).map_err(|e| e.to_string())?;
        let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
        let capturable = std::env::var("LAUREL_OVERLAY_CAPTURABLE").is_ok_and(|v| v == "1");
        match win::make_overlay(hwnd, capturable) {
            Ok(()) => log!("overlay ready (excluded from capture: {})", !capturable),
            Err(e) => {
                self.0.excluded.store(false, Ordering::SeqCst);
                log!("overlay ready for critical notices only: {e}");
            }
        }
        self.0.hwnd.store(hwnd, Ordering::SeqCst);
        Ok(())
    }

    /// Whether the overlay is kept out of screen capture (false: critical notices only).
    pub fn excluded(&self) -> bool {
        self.0.excluded.load(Ordering::SeqCst)
    }

    /// Self-test: acts as a PC whose overlay Windows cannot keep out of capture.
    pub fn set_excluded(&self, excluded: bool) {
        self.0.excluded.store(excluded, Ordering::SeqCst);
    }

    pub fn hwnd(&self) -> isize {
        self.0.hwnd.load(Ordering::SeqCst)
    }

    pub fn settings(&self) -> Settings {
        self.0.store.get()
    }

    pub fn set_settings(&self, new: Settings) -> std::io::Result<()> {
        self.0.store.set(new)
    }

    /// Plays the kind's sound and shows the notice over the foreground window,
    /// each according to the settings.
    pub fn notify(&self, kind: Kind, title: &str, detail: &str) {
        self.show(kind, title, detail, false);
    }

    /// An error notice that Laurel is not recording the game (it runs as
    /// administrator, the disk is almost full, the engine failed): shown over the
    /// game even with notices over games turned off, which would leave only a
    /// sound (A15 of the stability tests). The game leaves independent flip while
    /// it shows, but nothing is being recorded then.
    pub fn notify_critical(&self, title: &str, detail: &str) {
        self.show(Kind::Error, title, detail, true);
    }

    /// A notice shown over the foreground window even with notices over games
    /// turned off, for what the player must see once, such as a hotkey that an
    /// update changed (idea 14). Errors go through notify_critical.
    pub fn notify_always(&self, kind: Kind, title: &str, detail: &str) {
        self.show(kind, title, detail, true);
    }

    fn show(&self, kind: Kind, title: &str, detail: &str, critical: bool) {
        let settings = self.settings();
        let sound = settings.sounds && settings.volume > 0 && (kind != Kind::AutoClip || settings.auto_clip_sound);
        if let (true, Some(alert)) = (sound, kind.alert()) {
            let mut sounds = self.0.sounds.lock().unwrap();
            if sounds.0 != settings.volume {
                *sounds = (settings.volume, Sounds::new(settings.volume));
            }
            sounds.1.play(alert);
        }
        let hwnd = self.hwnd();
        if hwnd == 0 || !(critical || settings.overlay && self.excluded()) {
            return;
        }
        if critical {
            let what = if kind == Kind::Error { "critical notice" } else { "notice over the screen" };
            log!("{what} shown: {title}");
        }
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let _ = self.0.app.emit_to("overlay", "notice", json!({ "id": id, "kind": kind.name(), "title": title, "detail": detail, "holdMs": kind.hold().as_millis() as u64 }));
        let _ = self.0.app.run_on_main_thread(move || {
            let (anchor, scale) = win::foreground_anchor();
            let (w, h, m) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32, (MARGIN * scale) as i32);
            win::show_no_activate(hwnd, anchor.right - w - m, anchor.top + m, w, h);
        });
        let until = Instant::now() + kind.hold() + EXIT;
        {
            let mut visible_until = self.0.visible_until.lock().unwrap();
            *visible_until = (*visible_until).max(until);
        }
        let notifier = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
            if Instant::now() >= *notifier.0.visible_until.lock().unwrap() {
                let _ = notifier.0.app.run_on_main_thread(move || win::hide(hwnd));
            }
        });
    }
}
