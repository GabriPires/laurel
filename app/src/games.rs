//! Reads the store libraries from the PC (spec game.detection): where each store
//! keeps its files, from the registry and the usual folders, and their text. Which
//! programs are games, and what those files say, is decided in
//! laurel_app_core::games and laurel_app_core::stores.

use std::path::{Path, PathBuf};

pub use laurel_app_core::games::{Catalog, GameId, Verdict};
use laurel_app_core::games::Curated;
use laurel_app_core::stores::{self, Listing, Report, Store, StoreFile};

use crate::win;

/// Reads the store libraries (a few small files each), with what the scan of each
/// store saw, for the log (A32).
pub fn scan() -> (Catalog, Vec<Report>) {
    let curated = Curated::shipped();
    let mut installed = Vec::new();
    let mut reports = Vec::new();
    for listing in [steam_files(), epic_files(), riot_files(), xbox_files()] {
        let (games, report) = stores::read(&listing, &curated, |dir| Path::new(dir).is_dir());
        installed.extend(games);
        reports.push(report);
    }
    (Catalog::new(curated, installed), reports)
}

fn listing(store: Store) -> Listing {
    Listing { store, files: Vec::new(), notes: Vec::new() }
}

/// The entries of a store's folder; one that cannot be listed gets a note in the
/// scan's line: `<what> (<folder>: <error>)`.
fn list(dir: &Path, what: &str, listing: &mut Listing) -> Option<std::fs::ReadDir> {
    std::fs::read_dir(dir).map_err(|e| listing.notes.push(format!("{what} ({}: {e})", dir.display()))).ok()
}

fn file(path: &Path) -> StoreFile {
    StoreFile { path: path.to_string_lossy().into_owned(), text: std::fs::read_to_string(path).map_err(|e| e.to_string()) }
}

fn steam_files() -> Listing {
    let mut listing = listing(Store::Steam);
    let Some(steam) = win::reg_string(win::HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath")
        .or_else(|| win::reg_string(win::HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
    else {
        listing.notes.push("no Steam folder in the registry".into());
        return listing;
    };
    let steam = PathBuf::from(steam.replace('/', "\\"));
    let folders = steam.join(r"steamapps\libraryfolders.vdf");
    let mut libraries = match std::fs::read_to_string(&folders) {
        Ok(text) => stores::steam_libraries(&text),
        Err(e) => {
            listing.notes.push(format!("no library list ({}: {e})", folders.display()));
            Vec::new()
        }
    };
    if libraries.is_empty() {
        libraries.push(steam.to_string_lossy().into_owned());
    }
    for library in libraries {
        let apps = Path::new(&library).join("steamapps");
        let Some(entries) = list(&apps, "a library cannot be listed", &mut listing) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                listing.files.push(file(&entry.path()));
            }
        }
    }
    listing
}

fn program_data() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()))
}

fn epic_files() -> Listing {
    let mut listing = listing(Store::Epic);
    let manifests = program_data().join(r"Epic\EpicGamesLauncher\Data\Manifests");
    let Some(entries) = list(&manifests, "no manifest folder", &mut listing) else { return listing };
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|x| x == "item") {
            listing.files.push(file(&entry.path()));
        }
    }
    listing
}

/// Riot products: `<ProgramData>\Riot Games\Metadata\<product>.<patchline>\<product>.<patchline>.product_settings.yaml`.
fn riot_files() -> Listing {
    let mut listing = listing(Store::Riot);
    let metadata = program_data().join(r"Riot Games\Metadata");
    let Some(entries) = list(&metadata, "no Metadata folder", &mut listing) else { return listing };
    for entry in entries.flatten() {
        let folder = entry.file_name().to_string_lossy().into_owned();
        listing.files.push(file(&entry.path().join(format!("{folder}.product_settings.yaml"))));
    }
    listing
}

/// Microsoft Store (GDK) games in `<drive>:\XboxGames\<game>\Content`.
fn xbox_files() -> Listing {
    let mut listing = listing(Store::Xbox);
    let mut folders = 0;
    for drive in 'C'..='Z' {
        let Ok(entries) = std::fs::read_dir(format!(r"{drive}:\XboxGames")) else { continue };
        folders += 1;
        for entry in entries.flatten() {
            listing.files.push(file(&entry.path().join(r"Content\MicrosoftGame.config")));
        }
    }
    if folders == 0 {
        listing.notes.push(r"no XboxGames folder on any drive".into());
    }
    listing
}
