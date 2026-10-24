//! Which programs are games (spec game.detection, decision of 2026-10-23): the games
//! installed through the Steam, Epic, Riot and Xbox libraries, found from their
//! install folders; the curated list in `app/games.json` (games from other
//! launchers, store entries that are not games, helper and client executables); and
//! the user's own choices (game.manual-add), which always win.
//!
//! A game is identified by its store and its ID there (`steam:1808500`,
//! `riot:valorant`), or by `exe:<file name>` for the curated list and `user:<path>`
//! for games the user added.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::settings::Settings;
use crate::win;

const CURATED: &str = include_str!("../games.json");

#[derive(Clone, Debug, PartialEq)]
pub struct GameId {
    pub key: String,
    pub name: String,
    /// steam, epic, riot, xbox, list (curated) or user.
    pub source: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Game(GameId),
    /// A game the user chose not to record.
    Ignored(GameId),
    NotGame,
}

/// A game found in a store library.
#[derive(Clone, Debug)]
pub struct Installed {
    pub id: GameId,
    pub dir: PathBuf,
    /// Lowercase, backslashes, ending with a backslash, for prefix matching.
    prefix: String,
}

struct Curated {
    not_games: Vec<String>,
    blocked_exes: Vec<String>,
    blocked_patterns: Vec<String>,
    /// (lowercase file name, name)
    games: Vec<(String, String)>,
}

impl Curated {
    fn load() -> Curated {
        let v: Value = serde_json::from_str(CURATED).expect("app/games.json is valid JSON");
        let strings = |key: &str| -> Vec<String> { v[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_lowercase).collect() };
        let games = v["games"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| Some((g["exe"].as_str()?.to_lowercase(), g["name"].as_str()?.to_string())))
            .collect();
        Curated { not_games: strings("notGames"), blocked_exes: strings("blockedExes"), blocked_patterns: strings("blockedPatterns"), games }
    }

    fn blocks(&self, file: &str) -> bool {
        self.blocked_exes.iter().any(|b| b == file) || self.blocked_patterns.iter().any(|p| file.contains(p.as_str()))
    }
}

pub struct Catalog {
    installed: Vec<Installed>,
    curated: Curated,
}

/// `C:/Riot Games/VALORANT/live` and `C:\Games\` both become `c:\riot games\valorant\live\`.
fn normalize_dir(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().replace('/', "\\").to_lowercase();
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

impl Catalog {
    /// Reads the store libraries (a few small files each).
    pub fn scan() -> Catalog {
        let curated = Curated::load();
        let mut installed = Vec::new();
        for (store, found) in [("steam", steam_games()), ("epic", epic_games()), ("riot", riot_games()), ("xbox", xbox_games())] {
            for (id, name, dir) in found {
                let key = format!("{store}:{id}");
                if curated.not_games.contains(&key.to_lowercase()) || !dir.is_dir() {
                    continue;
                }
                let prefix = normalize_dir(&dir);
                installed.push(Installed { id: GameId { key, name, source: store }, dir, prefix });
            }
        }
        // Longest folder first, so a game inside another game's folder wins.
        installed.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
        Catalog { installed, curated }
    }

    pub fn installed(&self) -> &[Installed] {
        &self.installed
    }

    /// Games found per store, for the UI and the log.
    pub fn counts(&self) -> Value {
        let mut counts = serde_json::Map::new();
        for g in &self.installed {
            let n = counts.get(g.id.source).and_then(Value::as_u64).unwrap_or(0);
            counts.insert(g.id.source.to_string(), json!(n + 1));
        }
        Value::Object(counts)
    }

    /// Decides whether the program at `exe_path` is a game. The user's added games
    /// come first, then the store libraries, then the curated list; the user's
    /// ignored games and the blocklist exclude the rest.
    pub fn classify(&self, exe_path: &str, settings: &Settings) -> Verdict {
        let lower = exe_path.replace('/', "\\").to_lowercase();
        if let Some(custom) = settings.custom_games.iter().find(|g| g.path.to_lowercase() == lower) {
            return Verdict::Game(GameId { key: format!("user:{lower}"), name: custom.name.clone(), source: "user" });
        }
        let file = file_name_lower(&lower);
        let found = self
            .installed
            .iter()
            .find(|g| lower.starts_with(&g.prefix))
            .map(|g| g.id.clone())
            .or_else(|| self.curated.games.iter().find(|(exe, _)| *exe == file).map(|(exe, name)| GameId { key: format!("exe:{exe}"), name: name.clone(), source: "list" }));
        match found {
            None => Verdict::NotGame,
            Some(_) if self.curated.blocks(&file) => Verdict::NotGame,
            Some(id) if settings.ignored_games.iter().any(|g| g.key == id.key) => Verdict::Ignored(id),
            Some(id) => Verdict::Game(id),
        }
    }
}

/// Values of `"key" "value"` pairs in a Valve KeyValues text file, unescaped.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let quoted = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&quoted)?.trim();
            let value = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(value.replace("\\\\", "\\"))
        })
        .collect()
}

fn steam_games() -> Vec<(String, String, PathBuf)> {
    let Some(steam) = win::reg_string(win::HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath")
        .or_else(|| win::reg_string(win::HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
    else {
        return vec![];
    };
    let steam = PathBuf::from(steam.replace('/', "\\"));
    let mut libraries = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")).map(|t| vdf_values(&t, "path")).unwrap_or_default();
    if libraries.is_empty() {
        libraries.push(steam.to_string_lossy().into_owned());
    }
    let mut games = Vec::new();
    for library in libraries {
        let apps = Path::new(&library).join("steamapps");
        let Ok(entries) = std::fs::read_dir(&apps) else { continue };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
            let first = |key: &str| vdf_values(&text, key).into_iter().next();
            if let (Some(id), Some(name), Some(dir)) = (first("appid"), first("name"), first("installdir")) {
                games.push((id, name, apps.join("common").join(dir)));
            }
        }
    }
    games
}

fn program_data() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()))
}

fn epic_games() -> Vec<(String, String, PathBuf)> {
    let dir = program_data().join(r"Epic\EpicGamesLauncher\Data\Manifests");
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "item"))
        .filter_map(|e| serde_json::from_str::<Value>(&std::fs::read_to_string(e.path()).ok()?).ok())
        .filter(|m| m["AppCategories"].as_array().is_some_and(|c| c.iter().any(|v| v == "games")))
        .filter_map(|m| Some((m["AppName"].as_str()?.to_string(), m["DisplayName"].as_str()?.to_string(), PathBuf::from(m["InstallLocation"].as_str()?))))
        .collect()
}

/// Riot products: `<ProgramData>\Riot Games\Metadata\<product>.<patchline>\*.product_settings.yaml`.
fn riot_games() -> Vec<(String, String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(program_data().join(r"Riot Games\Metadata")) else { return vec![] };
    let mut games = Vec::new();
    for entry in entries.flatten() {
        let folder = entry.file_name().to_string_lossy().into_owned();
        let Some((product, patchline)) = folder.split_once('.') else { continue };
        let settings = entry.path().join(format!("{folder}.product_settings.yaml"));
        let Ok(text) = std::fs::read_to_string(settings) else { continue };
        let Some(dir) = text.lines().find_map(|l| l.trim().strip_prefix("product_install_full_path:")) else { continue };
        let dir = dir.trim().trim_matches('"');
        let name = match product {
            "league_of_legends" => "League of Legends".to_string(),
            "valorant" => "VALORANT".to_string(),
            "bacon" => "Legends of Runeterra".to_string(),
            "teamfighttactics" => "Teamfight Tactics".to_string(),
            other => other.replace('_', " "),
        };
        let id = if patchline == "live" { product.to_string() } else { folder.clone() };
        games.push((id, name, PathBuf::from(dir)));
    }
    games
}

/// The value of `attr="..."` after `element` in a MicrosoftGame.config.
fn xml_attr(text: &str, element: &str, attr: &str) -> Option<String> {
    let start = text.find(element)?;
    let rest = &text[start..];
    let at = rest.find(&format!("{attr}=\""))? + attr.len() + 2;
    Some(rest[at..].split('"').next()?.to_string())
}

/// Microsoft Store (GDK) games in `<drive>:\XboxGames\<game>\Content`.
fn xbox_games() -> Vec<(String, String, PathBuf)> {
    let mut games = Vec::new();
    for drive in 'C'..='Z' {
        let Ok(entries) = std::fs::read_dir(format!(r"{drive}:\XboxGames")) else { continue };
        for entry in entries.flatten() {
            let content = entry.path().join("Content");
            let Ok(text) = std::fs::read_to_string(content.join("MicrosoftGame.config")) else { continue };
            let id = xml_attr(&text, "<Identity", "Name");
            let name = xml_attr(&text, "<ShellVisuals", "DefaultDisplayName");
            if let (Some(id), Some(name)) = (id, name) {
                games.push((id, name, content));
            }
        }
    }
    games
}
