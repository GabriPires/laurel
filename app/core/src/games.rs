//! Which programs are games (spec game.detection, decision of 2026-10-23): the games
//! installed through the Steam, Epic, Riot and Xbox libraries (stores.rs reads
//! their files); the curated list in `games.json` (games from other launchers,
//! store entries that are not games, helper and client executables); and the
//! user's own choices (game.manual-add), which always win.
//!
//! A game is identified by its store and its ID there (`steam:1808500`,
//! `riot:valorant`), also when the curated list finds a store game its library did
//! not (A32: `epic:Fortnite`); by `exe:<file name>` for the other games of the
//! curated list; by the curated key of a game found by its window
//! (`minecraft:java`); and by `user:<path>` for games the user added.
//!
//! Paths come from Windows and are handled as text, the same on every platform:
//! `/` counts as `\`, and case does not matter.

use serde_json::{json, Value};

const SHIPPED: &str = include_str!("../games.json");

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

/// A program the user marked as a game, by full path.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomGame {
    pub path: String,
    pub name: String,
}

/// A detected game the user does not want recorded, by game key.
#[derive(Clone, Debug, PartialEq)]
pub struct IgnoredGame {
    pub key: String,
    pub name: String,
}

/// The user's own lists (game.manual-add), from the settings.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserGames<'a> {
    pub custom: &'a [CustomGame],
    pub ignored: &'a [IgnoredGame],
}

impl UserGames<'_> {
    fn ignores(&self, key: &str) -> bool {
        self.ignored.iter().any(|g| g.key == key)
    }
}

/// A game found in a store library.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreGame {
    pub id: GameId,
    /// The install folder, as the store wrote it.
    pub dir: String,
    /// Lowercase, backslashes, ending with a backslash, for prefix matching.
    prefix: String,
}

impl StoreGame {
    pub fn new(id: GameId, dir: String) -> StoreGame {
        let prefix = normalize_dir(&dir);
        StoreGame { id, dir, prefix }
    }
}

/// A game that runs in a shared program from any folder, such as Minecraft (Java)
/// in the javaw.exe of whichever Java runtime its launcher brought (games.json,
/// windowGames).
#[derive(Clone, Debug)]
struct WindowGame {
    id: GameId,
    /// Lowercase program file names.
    exes: Vec<String>,
    /// Window classes, compared without case.
    classes: Vec<String>,
    /// Lowercase parts of the command line, any one of which marks the game.
    command_line: Vec<String>,
    /// Lowercase title prefixes, used when the command line cannot be read.
    titles: Vec<String>,
}

/// The curated list (games.json).
#[derive(Clone, Debug)]
pub struct Curated {
    window_games: Vec<WindowGame>,
    not_games: Vec<String>,
    blocked_exes: Vec<String>,
    blocked_patterns: Vec<String>,
    /// (lowercase file name, game): a store game's entry has its store key
    /// (`epic:Fortnite`), any other `exe:<lowercase file name>`.
    games: Vec<(String, GameId)>,
}

impl Curated {
    /// The list shipped with Laurel.
    pub fn shipped() -> Curated {
        Curated::parse(SHIPPED).expect("app/core/games.json is valid")
    }

    /// A list in the format of games.json.
    pub fn parse(text: &str) -> Result<Curated, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let list = |g: &Value, key: &str| -> Vec<String> { g[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_lowercase).collect() };
        let games = v["games"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let exe = g["exe"].as_str()?.to_lowercase();
                let key = g["key"].as_str().map_or_else(|| format!("exe:{exe}"), String::from);
                Some((exe, GameId { key, name: g["name"].as_str()?.to_string(), source: "list" }))
            })
            .collect();
        let window_games = v["windowGames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| {
                let id = GameId { key: g["key"].as_str()?.to_string(), name: g["name"].as_str()?.to_string(), source: "list" };
                Some(WindowGame { id, exes: list(g, "exes"), classes: list(g, "classes"), command_line: list(g, "commandLine"), titles: list(g, "titles") })
            })
            .collect();
        Ok(Curated { window_games, not_games: list(&v, "notGames"), blocked_exes: list(&v, "blockedExes"), blocked_patterns: list(&v, "blockedPatterns"), games })
    }

    /// Whether a store entry (`steam:228980`) is listed as not a game.
    pub fn not_game(&self, key: &str) -> bool {
        self.not_games.contains(&key.to_lowercase())
    }

    /// Whether a program file name (lowercase) is on the blocklist.
    fn blocks(&self, file: &str) -> bool {
        self.blocked_exes.iter().any(|b| b == file) || self.blocked_patterns.iter().any(|p| file.contains(p.as_str()))
    }
}

pub struct Catalog {
    store_games: Vec<StoreGame>,
    curated: Curated,
}

/// A path as Windows compares it: `/` as `\`, and case ignored.
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// `C:/Riot Games/VALORANT/live` and `C:\Games\` both become `c:\riot games\valorant\live\`.
fn normalize_dir(dir: &str) -> String {
    let mut s = normalize_path(dir);
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

fn file_name_lower(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

impl Catalog {
    pub fn new(curated: Curated, mut store_games: Vec<StoreGame>) -> Catalog {
        // Longest folder first, so a game inside another game's folder wins.
        store_games.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
        Catalog { store_games, curated }
    }

    pub fn store_games(&self) -> &[StoreGame] {
        &self.store_games
    }

    /// Whether the program at `exe_path` is on the blocklist: never a game.
    pub fn blocks(&self, exe_path: &str) -> bool {
        self.curated.blocks(&file_name_lower(exe_path))
    }

    /// Games found per store, for the UI and the log.
    pub fn counts(&self) -> Value {
        let mut counts = serde_json::Map::new();
        for g in &self.store_games {
            let n = counts.get(g.id.source).and_then(Value::as_u64).unwrap_or(0);
            counts.insert(g.id.source.to_string(), json!(n + 1));
        }
        Value::Object(counts)
    }

    /// Decides whether the program at `exe_path` is a game. The user's added games
    /// come first, then the store libraries, then the curated list; the user's
    /// ignored games and the blocklist exclude the rest.
    pub fn classify(&self, exe_path: &str, user: &UserGames) -> Verdict {
        let lower = normalize_path(exe_path);
        if let Some(custom) = user.custom.iter().find(|g| normalize_path(&g.path) == lower) {
            return Verdict::Game(GameId { key: format!("user:{lower}"), name: custom.name.clone(), source: "user" });
        }
        let file = file_name_lower(&lower);
        let found = self
            .store_games
            .iter()
            .find(|g| lower.starts_with(&g.prefix))
            .map(|g| g.id.clone())
            .or_else(|| self.curated.games.iter().find(|(exe, _)| *exe == file).map(|(_, id)| id.clone()));
        match found {
            None => Verdict::NotGame,
            Some(_) if self.curated.blocks(&file) => Verdict::NotGame,
            Some(id) if user.ignores(&id.key) => Verdict::Ignored(id),
            Some(id) => Verdict::Game(id),
        }
    }

    /// Decides whether a window is a game's: by its program as `classify` does, and
    /// for the games of `windowGames`, by the program's file name, the window class
    /// and the command line (read only for those candidates), or the title when the
    /// command line cannot be read. Minecraft (Java) runs in the javaw.exe of the
    /// Java runtime its launcher brought, a different one for each launcher and
    /// often for each modpack, so its path says nothing.
    pub fn classify_window(&self, exe_path: &str, class: &str, title: &str, command_line: impl FnOnce() -> Option<String>, user: &UserGames) -> Verdict {
        let verdict = self.classify(exe_path, user);
        if verdict != Verdict::NotGame {
            return verdict;
        }
        let file = file_name_lower(exe_path);
        let class = class.to_lowercase();
        let Some(game) = self.curated.window_games.iter().find(|g| g.exes.contains(&file) && g.classes.contains(&class)) else {
            return Verdict::NotGame;
        };
        let matches = match command_line() {
            Some(line) => {
                let line = line.to_lowercase();
                game.command_line.iter().any(|m| line.contains(m.as_str()))
            }
            None => {
                let title = title.trim().to_lowercase();
                game.titles.iter().any(|t| title.starts_with(t.as_str()))
            }
        };
        match matches {
            false => Verdict::NotGame,
            true if user.ignores(&game.id.key) => Verdict::Ignored(game.id.clone()),
            true => Verdict::Game(game.id.clone()),
        }
    }
}
