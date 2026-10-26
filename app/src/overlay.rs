//! In-game notices (spec overlay.in-game) and alert sounds (spec alerts.sound).
//!
//! The overlay is the transparent `overlay` window (ui/overlay.html): click-through,
//! never activated, topmost and excluded from screen capture. It is shown only
//! while notices are on screen, at the top right of the foreground window (the
//! game), and hidden again when idle so it does not sit over the game for nothing.

use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
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
    Screenshot,
    Info,
    Error,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Clip => "clip",
            Kind::Screenshot => "screenshot",
            Kind::Info => "info",
            Kind::Error => "error",
        }
    }

    fn alert(self) -> Option<Alert> {
        match self {
            Kind::Clip => Some(Alert::Clip),
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
            Kind::Clip | Kind::Screenshot => 1300,
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
        win::make_overlay(hwnd, capturable)?;
        self.0.hwnd.store(hwnd, Ordering::SeqCst);
        log!("overlay ready (excluded from capture: {})", !capturable);
        Ok(())
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
        let settings = self.settings();
        if let (true, Some(alert)) = (settings.sounds && settings.volume > 0, kind.alert()) {
            let mut sounds = self.0.sounds.lock().unwrap();
            if sounds.0 != settings.volume {
                *sounds = (settings.volume, Sounds::new(settings.volume));
            }
            sounds.1.play(alert);
        }
        let hwnd = self.hwnd();
        if !settings.overlay || hwnd == 0 {
            return;
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
