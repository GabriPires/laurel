//! Automatic clips of Steam achievements (spec autoclip.steam-achievements).
//!
//! While a Steam game is recorded and the option is on, Laurel runs laurel-steam
//! (steam/, MIT license, a separate program): it loads the Steam API library already
//! on the PC as the game's app and reports each new unlock. An unlock is clipped
//! when its event is on: any achievement, or one that fewer than 10% or 5% of the
//! players have, from Valve's public global percentages (cached per game for a
//! week). The clip is saved AUTO_CLIP_DELAY after the unlock, so it shows the moment
//! and what follows; unlocks inside that wait go into the same clip. The helper
//! stops when the game closes, when Laurel quits (its standard input closes), or
//! when the option is turned off.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::detect::{Detector, Mode};
use crate::log::log;
use crate::settings::{Settings, Store};

const WATCH_EVERY: Duration = Duration::from_secs(2);
const PERCENTS_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Under these shares of the players an achievement is rare or ultra rare.
pub const RARE: f64 = 10.0;
pub const ULTRA_RARE: f64 = 5.0;

struct Helper {
    appid: u32,
    key: String,
    name: String,
    child: Child,
    /// Closing it tells the helper to stop.
    stdin: Option<ChildStdin>,
}

struct Pending {
    due: Instant,
    achievements: Vec<Value>,
}

#[derive(Default)]
struct State {
    helper: Option<Helper>,
    /// The game whose helper failed, not started again until another game.
    failed: Option<u32>,
    pending: Option<Pending>,
    /// Global percentages by app, from the cache or Valve's Web API.
    percents: HashMap<u32, Arc<HashMap<String, f64>>>,
    /// What the helper said when it was ready, for the settings page.
    ready: Option<Value>,
}

pub struct Achievements {
    app: AppHandle,
    store: Arc<Store>,
    cache_dir: PathBuf,
    state: Mutex<State>,
    /// AUTO_CLIP_DELAY in milliseconds (shorter in the self-test).
    delay_ms: AtomicU64,
}

pub const AUTO_CLIP_DELAY: Duration = Duration::from_secs(15);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// laurel-steam.exe: LAUREL_STEAM, else next to the app, else the development
/// layout (laurel-dev\laurel-app next to laurel-dev\laurel-steam).
fn helper_exe() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    std::env::var_os("LAUREL_STEAM").map(PathBuf::from).unwrap_or_else(|| {
        let bundled = exe_dir.join("laurel-steam.exe");
        if bundled.exists() { bundled } else { exe_dir.join("..").join("laurel-steam").join("laurel-steam.exe") }
    })
}

/// Which event clips an unlock: ultra rare, rare or any, by the settings and the
/// share of players who have it (unknown shares only match "any").
pub fn clipped_as(settings: &Settings, percent: Option<f64>) -> Option<&'static str> {
    let under = |limit: f64| percent.is_some_and(|p| p < limit);
    if settings.steam_ultra && under(ULTRA_RARE) {
        Some("ultra-rare")
    } else if settings.steam_rare && under(RARE) {
        Some("rare")
    } else if settings.steam_any {
        Some("any")
    } else {
        None
    }
}

impl Achievements {
    pub fn new(app: AppHandle, store: Arc<Store>, work_dir: &std::path::Path) -> Arc<Achievements> {
        Arc::new(Achievements {
            app,
            store,
            cache_dir: work_dir.join("steam"),
            state: Mutex::new(State::default()),
            delay_ms: AtomicU64::new(AUTO_CLIP_DELAY.as_millis() as u64),
        })
    }

    pub fn start(self: &Arc<Achievements>) {
        let me = self.clone();
        std::thread::spawn(move || loop {
            me.tick();
            std::thread::sleep(WATCH_EVERY);
        });
    }

    pub fn set_delay(&self, delay: Duration) {
        self.delay_ms.store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    /// Global percentages known without asking Valve (the self-test).
    pub fn seed_percents(&self, appid: u32, percents: HashMap<String, f64>) {
        self.state.lock().unwrap().percents.insert(appid, Arc::new(percents));
    }

    /// The Steam game whose achievements should be watched now, if any.
    fn wanted(&self) -> Option<(u32, String, String)> {
        let s = self.store.get();
        let detector = self.app.state::<Arc<Detector>>();
        if !(s.auto_clip && s.steam_achievements) || detector.mode() != Mode::Detect || !detector.enabled() || !detector.recording() {
            return None;
        }
        let game = detector.current()?;
        if game.id.source != "steam" || s.steam_excluded.iter().any(|g| g.key == game.id.key) {
            return None;
        }
        let appid = game.id.key.strip_prefix("steam:")?.parse().ok()?;
        Some((appid, game.id.key, game.id.name))
    }

    fn tick(self: &Arc<Achievements>) {
        let wanted = self.wanted();
        let mut s = self.state.lock().unwrap();
        // A helper that ended by itself is dropped (and not started again for that game).
        if let Some(h) = s.helper.as_mut() {
            if let Ok(Some(status)) = h.child.try_wait() {
                log!("steam: the helper for {} ended ({status})", h.name);
                s.failed = Some(h.appid);
                s.helper = None;
                s.ready = None;
            }
        }
        let running = s.helper.as_ref().map(|h| h.appid);
        if running == wanted.as_ref().map(|w| w.0) {
            return;
        }
        if let Some(h) = s.helper.take() {
            stop(h);
            s.ready = None;
        }
        if wanted.as_ref().map(|w| w.0) != s.failed {
            s.failed = None;
        }
        let Some((appid, key, name)) = wanted else { return };
        if s.failed == Some(appid) {
            return;
        }
        let exe = helper_exe();
        let spawned = Command::new(&exe)
            .args(["--appid", &appid.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                log!("steam: could not start {}: {e}", exe.display());
                s.failed = Some(appid);
                return;
            }
        };
        log!("steam: watching the achievements of {name} [{key}] (pid {})", child.id());
        let stdout = child.stdout.take();
        let stdin = child.stdin.take();
        s.helper = Some(Helper { appid, key, name: name.clone(), child, stdin });
        drop(s);
        if let Some(stdout) = stdout {
            let me = self.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(event) => me.on_event(appid, &name, event),
                        Err(_) => log!("steam: {line}"),
                    }
                }
            });
        }
        let me = self.clone();
        std::thread::spawn(move || {
            me.percents(appid);
        });
    }

    fn on_event(self: &Arc<Achievements>, appid: u32, game: &str, event: Value) {
        match event["event"].as_str().unwrap_or_default() {
            "ready" => {
                log!(
                    "steam: {game}: {} achievements, {} unlocked; library {} ({})",
                    event["achievements"], event["unlocked"], event["library"].as_str().unwrap_or_default(), event["origin"].as_str().unwrap_or_default()
                );
                self.state.lock().unwrap().ready = Some(event);
                let _ = self.app.emit("autoclip-state", self.status());
            }
            "unlocked" => self.on_unlock(appid, game, &event),
            "stopped" => log!("steam: helper for {game} stopped: {} passes, {} ms of CPU, {} MB", event["passes"], event["cpuMs"], event["workingSetMb"]),
            "error" => log!("steam: helper for {game}: {}", event["message"].as_str().unwrap_or_default()),
            _ => log!("steam: {event}"),
        }
    }

    /// Clips an unlock when its event is on; unlocks during the wait join the
    /// pending clip.
    pub fn on_unlock(self: &Arc<Achievements>, appid: u32, game: &str, event: &Value) {
        let id = event["id"].as_str().unwrap_or_default().to_string();
        let percent = self.percents(appid).get(&id).copied();
        let settings = self.store.get();
        let tier = clipped_as(&settings, percent);
        let shown = event["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&id).to_string();
        log!(
            "steam: {game}: unlocked {id} \"{shown}\" ({}): {}",
            percent.map_or("share unknown".into(), |p| format!("{p}% of players")),
            tier.map_or("not clipped".into(), |t| format!("clipped as {t}"))
        );
        let Some(tier) = tier else { return };
        let achievement = json!({
            "appid": appid, "id": id, "name": shown, "description": event["description"], "percent": percent, "tier": tier,
            "unlockedMs": event["atMs"].as_u64().unwrap_or_else(now_ms),
        });
        let mut s = self.state.lock().unwrap();
        if let Some(pending) = s.pending.as_mut().filter(|p| Instant::now() < p.due) {
            pending.achievements.push(achievement);
            return;
        }
        let due = Instant::now() + Duration::from_millis(self.delay_ms.load(Ordering::SeqCst));
        s.pending = Some(Pending { due, achievements: vec![achievement] });
        drop(s);
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let Some(pending) = me.state.lock().unwrap().pending.take() else { return };
            let mut first = pending.achievements[0].clone();
            if pending.achievements.len() > 1 {
                first["others"] = pending.achievements[1..].iter().map(|a| a["name"].clone()).collect();
            }
            let source = format!("achievement {} [{}]", first["id"].as_str().unwrap_or_default(), first["tier"].as_str().unwrap_or_default());
            crate::save_clip_for(&me.app, &source, Some(first));
        });
    }

    /// The game's global percentages: from the cache when younger than a week, else
    /// from Valve's Web API (no key needed), else an older cache; empty when none.
    fn percents(&self, appid: u32) -> Arc<HashMap<String, f64>> {
        if let Some(p) = self.state.lock().unwrap().percents.get(&appid) {
            return p.clone();
        }
        let file = self.cache_dir.join(format!("percents-{appid}.json"));
        let cached = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let fresh = cached.as_ref().and_then(|c| c["fetchedMs"].as_u64()).is_some_and(|ms| now_ms().saturating_sub(ms) < PERCENTS_FOR.as_millis() as u64);
        let parse = |v: &Value| -> HashMap<String, f64> {
            v.as_object().into_iter().flatten().filter_map(|(k, p)| Some((k.clone(), p.as_f64()?))).collect()
        };
        let map = if fresh {
            parse(&cached.as_ref().unwrap()["percents"])
        } else {
            let path = format!("/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v0002/?gameid={appid}&format=json");
            match crate::win::https_get("api.steampowered.com", &path).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| e.to_string())) {
                Ok(v) => {
                    let map: HashMap<String, f64> = v["achievementpercentages"]["achievements"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["percent"].as_str().and_then(|p| p.parse().ok()).or(a["percent"].as_f64())?)))
                        .collect();
                    let _ = std::fs::create_dir_all(&self.cache_dir);
                    let _ = std::fs::write(&file, json!({ "fetchedMs": now_ms(), "percents": map }).to_string());
                    log!("steam: global percentages of app {appid}: {} achievements", map.len());
                    map
                }
                Err(e) => {
                    log!("steam: global percentages of app {appid} unavailable: {e}");
                    cached.as_ref().map(|c| parse(&c["percents"])).unwrap_or_default()
                }
            }
        };
        let map = Arc::new(map);
        self.state.lock().unwrap().percents.insert(appid, map.clone());
        map
    }

    /// For the settings page: the game being watched and its achievements.
    pub fn status(&self) -> Value {
        let s = self.state.lock().unwrap();
        match (&s.helper, &s.ready) {
            (Some(h), ready) => json!({ "watching": h.name, "key": h.key, "achievements": ready.as_ref().map(|r| r["achievements"].clone()), "unlocked": ready.as_ref().map(|r| r["unlocked"].clone()) }),
            (None, _) => json!({ "watching": null }),
        }
    }

    /// Stops the helper now (Laurel quitting).
    pub fn shutdown(&self) {
        if let Some(h) = self.state.lock().unwrap().helper.take() {
            stop(h);
        }
    }
}

/// Closes the helper's input so it ends by itself, then kills it if it does not.
fn stop(mut h: Helper) {
    drop(h.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = h.child.try_wait() {
            log!("steam: stopped watching {}", h.name);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = h.child.kill();
    let _ = h.child.wait();
    log!("steam: stopped watching {} (killed)", h.name);
}
