//! Windows behind the game detector's Platform (laurel_app_core::detect).

use std::time::Instant;

use laurel_app_core::detect::{AppWindow, Platform};

/// The windows, processes and clock of this PC.
pub struct Win32;

impl Platform for Win32 {
    fn app_windows(&self) -> Vec<AppWindow> {
        crate::app_windows()
            .into_iter()
            .map(|w| AppWindow { hwnd: w.hwnd, pid: w.pid, title: w.title, class: w.class, width: w.rect.right - w.rect.left, height: w.rect.bottom - w.rect.top })
            .collect()
    }

    fn foreground_window(&self) -> isize {
        crate::foreground_window()
    }

    fn window_of(&self, hwnd: isize, pid: u32) -> bool {
        crate::window_of(hwnd, pid)
    }

    fn process_path(&self, pid: u32) -> Option<String> {
        crate::process_path(pid)
    }

    fn process_command_line(&self, pid: u32) -> Option<String> {
        crate::process_command_line(pid)
    }

    fn process_elevated(&self, pid: u32) -> Option<bool> {
        crate::process_elevated(pid)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}
