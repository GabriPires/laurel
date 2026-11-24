//! Capture folder, file names and space limits (specs storage.location,
//! storage.limits): where clips and screenshots go (the user's folder, else
//! Videos\Laurel), with the rules of laurel_app_core::storage given the settings,
//! Windows' clock and free space, and the app's log.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use laurel_app_core::storage::{self, DiskGuard, GB};
pub use laurel_app_core::storage::{check_writable, move_items, unique};

use crate::library::Library;
use crate::settings::Store;
use crate::win;

pub struct Storage {
    store: Arc<Store>,
    /// A fixed capture folder that ignores the settings (the self-test uses one,
    /// so it never writes into the user's folder).
    override_folder: Option<PathBuf>,
    guard: DiskGuard,
}

impl Storage {
    pub fn new(store: Arc<Store>, override_folder: Option<PathBuf>) -> Storage {
        Storage { store, override_folder, guard: DiskGuard::default() }
    }

    /// The capture folder: the user's choice, else Videos\Laurel.
    pub fn folder(&self) -> PathBuf {
        if let Some(folder) = &self.override_folder {
            return folder.clone();
        }
        let folder = self.store.get().folder;
        if !folder.is_empty() {
            return PathBuf::from(folder);
        }
        win::videos_folder().unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Videos")).join("Laurel")
    }

    pub fn clips_dir(&self) -> PathBuf {
        self.folder().join("Clips")
    }

    pub fn screenshots_dir(&self) -> PathBuf {
        self.folder().join("Screenshots")
    }

    /// Renames a file the engine just saved after the naming pattern (fields
    /// {game}, {date}, {time}) and moves the engine's thumbnail along. Returns the
    /// new path; on failure the file keeps its engine name.
    pub fn apply_naming(&self, saved: &str, game: Option<&str>, thumbs_dir: &Path) -> String {
        self.apply_naming_with(saved, game, None, thumbs_dir)
    }

    /// Same as apply_naming, with ` - <extra>` after the pattern (an automatic clip's
    /// achievement, autoclip.steam-achievements).
    pub fn apply_naming_with(&self, saved: &str, game: Option<&str>, extra: Option<&str>, thumbs_dir: &Path) -> String {
        storage::apply_naming(saved, &self.store.get().naming, game, extra, thumbs_dir, &win::date_time, &crate::log::line)
    }

    /// Removes items until the capture folder fits the limit, Recently deleted
    /// first, then the oldest items not marked to keep; returns the removed paths.
    pub fn enforce_limit(&self, library: &Library) -> Vec<String> {
        let settings = self.store.get();
        if !settings.limit_enabled {
            return Vec::new();
        }
        storage::enforce_limit(library, settings.limit_gb as u64 * GB, settings.recycle, &crate::log::line)
    }

    /// Free bytes on the capture drive and the configured minimum.
    pub fn free_space(&self) -> (Option<u64>, u64) {
        let folder = self.folder();
        let probe = if folder.exists() { folder } else { folder.parent().map(Path::to_path_buf).unwrap_or(folder) };
        (win::free_bytes(&probe), self.store.get().min_free_gb as u64 * GB)
    }

    /// Whether recording should pause (`Some(true)`), resume (`Some(false)`) or
    /// stay as it is (`None`), with hysteresis so it does not flap.
    pub fn disk_transition(&self) -> Option<bool> {
        let (Some(free), min) = self.free_space() else { return None };
        self.guard.transition(free, min)
    }

    pub fn is_paused(&self) -> bool {
        self.guard.is_paused()
    }

    pub fn state(&self, library: &Library) -> Value {
        let (free, min) = self.free_space();
        let (deleted_count, deleted_bytes) = library.deleted_size();
        json!({
            "folder": self.folder(),
            "usedBytes": library.total_size(),
            "deletedCount": deleted_count,
            "deletedBytes": deleted_bytes,
            "freeBytes": free,
            "minFreeBytes": min,
            "pausedForDisk": self.is_paused(),
        })
    }
}
