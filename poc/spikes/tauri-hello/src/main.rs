//! Phase 3 feasibility spike: a minimal Tauri 2 window cross-compiled from WSL.
//! When the page finishes loading in WebView2 it writes `page-loaded.txt` next to
//! the executable and exits, so the run can be verified without a human.
#![windows_subsystem = "windows"]

use tauri::webview::PageLoadEvent;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .on_page_load(|webview, payload| {
            if let PageLoadEvent::Finished = payload.event() {
                let marker = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.join("page-loaded.txt")));
                if let Some(path) = marker {
                    let _ = std::fs::write(path, format!("loaded {}\n", payload.url()));
                }
                let app = webview.app_handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    app.exit(0);
                });
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
