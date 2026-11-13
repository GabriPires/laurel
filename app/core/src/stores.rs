//! The files the store launchers keep about their installed games (spec
//! game.detection), read from their text: Steam's libraryfolders.vdf and
//! appmanifest_<id>.acf, Epic's <id>.item manifests, Riot's
//! <product>.<patchline>.product_settings.yaml and the MicrosoftGame.config of
//! Microsoft Store (GDK) games. Finding the stores in the registry and listing
//! their folders stays in the app (app/src/games.rs), which hands each file's path
//! and text here.

use serde_json::Value;

use crate::games::{Curated, GameId, StoreGame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    Steam,
    Epic,
    Riot,
    Xbox,
}

impl Store {
    /// The store's part of a game key (`steam:1808500`).
    pub fn name(self) -> &'static str {
        match self {
            Store::Steam => "steam",
            Store::Epic => "epic",
            Store::Riot => "riot",
            Store::Xbox => "xbox",
        }
    }
}

/// A game a store lists: its ID in the store, its name and its install folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub dir: String,
}

/// One file of a store, as the app found it: its path, and its text or why it
/// could not be read.
#[derive(Clone, Debug)]
pub struct StoreFile {
    pub path: String,
    pub text: Result<String, String>,
}

/// What the app found of one store: its files, and notes on what it could not
/// look at (the store is not installed, a folder cannot be listed).
#[derive(Clone, Debug)]
pub struct Listing {
    pub store: Store,
    pub files: Vec<StoreFile>,
    pub notes: Vec<String>,
}

/// Why a store's file gave no installed game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// The file could not be read.
    Unreadable(String),
    /// The file lacks what the store's format has: what is missing or wrong.
    Invalid(&'static str),
    /// An Epic entry outside the games category (an engine, a plugin).
    NotAGame,
    /// The curated list says the entry is not a game (a redistributable, a tool).
    ListedAsNotGame,
    /// The install folder the file names does not exist.
    NoFolder(String),
}

/// What the scan of one store saw, for the log (A32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub store: Store,
    pub found: usize,
    pub read: usize,
    pub accepted: usize,
    /// Each rejected file's path and why.
    pub rejected: Vec<(String, Rejected)>,
    pub notes: Vec<String>,
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Rejected::Unreadable(e) => write!(f, "unreadable: {e}"),
            Rejected::Invalid(what) => f.write_str(what),
            Rejected::NotAGame => f.write_str("not in the games category"),
            Rejected::ListedAsNotGame => f.write_str("listed as not a game"),
            Rejected::NoFolder(dir) => write!(f, "install folder missing: {dir}"),
        }
    }
}

/// The rejected files a scan line names; the rest are counted.
const LISTED_REJECTED: usize = 10;

/// One line: `epic: 3 files found, 3 read, 2 accepted; rejected: <path> (<why>), …;
/// <note>`.
impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}: {} files found, {} read, {} accepted", self.store.name(), self.found, self.read, self.accepted)?;
        if !self.rejected.is_empty() {
            let listed: Vec<String> = self.rejected.iter().take(LISTED_REJECTED).map(|(path, why)| format!("{path} ({why})")).collect();
            write!(f, "; rejected: {}", listed.join(", "))?;
            if self.rejected.len() > LISTED_REJECTED {
                write!(f, ", and {} more", self.rejected.len() - LISTED_REJECTED)?;
            }
        }
        for note in &self.notes {
            write!(f, "; {note}")?;
        }
        Ok(())
    }
}

/// The reports of `now` that differ from the same store's in `before`: a store's
/// line is logged at start and again only when it changed.
pub fn changed<'a>(before: &[Report], now: &'a [Report]) -> Vec<&'a Report> {
    now.iter().filter(|r| !before.contains(r)).collect()
}

/// The folder part of a path (`C:\a\b.acf` → `C:\a`).
fn parent(path: &str) -> &str {
    path.rsplit_once(['\\', '/']).map_or("", |(dir, _)| dir)
}

/// The last part of a path.
fn last(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
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

/// The library folders in Steam's steamapps\libraryfolders.vdf.
pub fn steam_libraries(vdf: &str) -> Vec<String> {
    vdf_values(vdf, "path")
}

/// A game from Steam's `<library>\steamapps\appmanifest_<id>.acf`; it is installed
/// in `<library>\steamapps\common\<installdir>`.
fn steam_manifest(path: &str, text: &str) -> Result<Listed, Rejected> {
    let first = |key: &'static str, missing: &'static str| vdf_values(text, key).into_iter().next().ok_or(Rejected::Invalid(missing));
    let (id, name, dir) = (first("appid", "no appid")?, first("name", "no name")?, first("installdir", "no installdir")?);
    Ok(Listed { id, name, dir: format!("{}\\common\\{dir}", parent(path)) })
}

/// A game from an Epic manifest (`<ProgramData>\Epic\EpicGamesLauncher\Data\Manifests\<id>.item`).
fn epic_manifest(text: &str) -> Result<Listed, Rejected> {
    let m: Value = serde_json::from_str(text).map_err(|_| Rejected::Invalid("not JSON"))?;
    if !m["AppCategories"].as_array().is_some_and(|c| c.iter().any(|v| v == "games")) {
        return Err(Rejected::NotAGame);
    }
    let field = |key: &str, missing: &'static str| m[key].as_str().map(String::from).ok_or(Rejected::Invalid(missing));
    Ok(Listed { id: field("AppName", "no AppName")?, name: field("DisplayName", "no DisplayName")?, dir: field("InstallLocation", "no InstallLocation")? })
}

/// The folder of a Riot product's settings file and its `<product>.<patchline>` parts.
fn riot_folder(path: &str) -> Result<(&str, &str, &str), Rejected> {
    let folder = last(parent(path));
    let (product, patchline) = folder.split_once('.').ok_or(Rejected::Invalid("no patchline in the folder name"))?;
    Ok((folder, product, patchline))
}

/// A Riot product from `<ProgramData>\Riot Games\Metadata\<product>.<patchline>\<product>.<patchline>.product_settings.yaml`.
fn riot_settings(path: &str, text: &str) -> Result<Listed, Rejected> {
    let (folder, product, patchline) = riot_folder(path)?;
    let dir = text.lines().find_map(|l| l.trim().strip_prefix("product_install_full_path:")).ok_or(Rejected::Invalid("no product_install_full_path"))?;
    let dir = dir.trim().trim_matches('"');
    let name = match product {
        "league_of_legends" => "League of Legends".to_string(),
        "valorant" => "VALORANT".to_string(),
        "bacon" => "Legends of Runeterra".to_string(),
        "teamfighttactics" => "Teamfight Tactics".to_string(),
        other => other.replace('_', " "),
    };
    let id = if patchline == "live" { product.to_string() } else { folder.to_string() };
    Ok(Listed { id, name, dir: dir.to_string() })
}

/// The value of `attr="..."` after `element` in a MicrosoftGame.config.
fn xml_attr(text: &str, element: &str, attr: &str) -> Option<String> {
    let start = text.find(element)?;
    let rest = &text[start..];
    let at = rest.find(&format!("{attr}=\""))? + attr.len() + 2;
    Some(rest[at..].split('"').next()?.to_string())
}

/// A Microsoft Store (GDK) game from `<drive>:\XboxGames\<game>\Content\MicrosoftGame.config`;
/// it is installed in that Content folder.
fn xbox_config(path: &str, text: &str) -> Result<Listed, Rejected> {
    let id = xml_attr(text, "<Identity", "Name").ok_or(Rejected::Invalid("no Identity Name"))?;
    let name = xml_attr(text, "<ShellVisuals", "DefaultDisplayName").ok_or(Rejected::Invalid("no ShellVisuals DefaultDisplayName"))?;
    Ok(Listed { id, name, dir: parent(path).to_string() })
}

/// The game in a store library that one of the store's files names.
fn read_file(store: Store, file: &StoreFile, curated: &Curated, dir_exists: &impl Fn(&str) -> bool) -> Result<StoreGame, Rejected> {
    // The Metadata folder holds other folders too ("Riot Client"), with no file to
    // read: their name says why they are not games.
    if store == Store::Riot {
        riot_folder(&file.path)?;
    }
    let text = file.text.as_ref().map_err(|e| Rejected::Unreadable(e.clone()))?;
    let listed = match store {
        Store::Steam => steam_manifest(&file.path, text),
        Store::Epic => epic_manifest(text),
        Store::Riot => riot_settings(&file.path, text),
        Store::Xbox => xbox_config(&file.path, text),
    }?;
    let key = format!("{}:{}", store.name(), listed.id);
    if curated.not_game(&key) {
        return Err(Rejected::ListedAsNotGame);
    }
    if !dir_exists(&listed.dir) {
        return Err(Rejected::NoFolder(listed.dir));
    }
    Ok(StoreGame::new(GameId { key, name: listed.name, source: store.name() }, listed.dir))
}

/// The games of a store library, from its files: those that read, are not listed as
/// not games in the curated list, and whose folder exists (`dir_exists`); and what
/// the scan saw, for the log (A32: the scan had left no trace of a game it missed).
pub fn read(listing: &Listing, curated: &Curated, dir_exists: impl Fn(&str) -> bool) -> (Vec<StoreGame>, Report) {
    let mut report = Report { store: listing.store, found: listing.files.len(), read: 0, accepted: 0, rejected: Vec::new(), notes: listing.notes.clone() };
    let mut games = Vec::new();
    for file in &listing.files {
        if file.text.is_ok() {
            report.read += 1;
        }
        match read_file(listing.store, file, curated, &dir_exists) {
            Ok(game) => games.push(game),
            Err(why) => report.rejected.push((file.path.clone(), why)),
        }
    }
    report.accepted = games.len();
    (games, report)
}
