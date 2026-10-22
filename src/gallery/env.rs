//! What the library window reads and writes outside itself: the screenshots folder, its settings,
//! and the file another launch leaves to ask it to open something. Tests get a temporary folder and
//! in-memory settings, so they never touch the real ones.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::platform;

pub struct Env {
    pub dir: PathBuf,
    /// Another `--gallery` launch drops the file it wants opened here.
    pub request_file: PathBuf,
    /// Ask GitHub for a newer release (only where the tray app doesn't).
    pub check_updates: bool,
    settings: Store,
}

enum Store {
    /// The registry on Windows, `settings.json` elsewhere.
    Platform,
    #[cfg_attr(not(test), allow(dead_code))]
    Memory(RefCell<BTreeMap<String, u32>>),
}

impl Env {
    pub fn real() -> Env {
        Env {
            dir: platform::screenshots_dir(),
            request_file: request_file(),
            check_updates: cfg!(not(windows)) && platform::setting("CheckUpdates") != Some(0),
            settings: Store::Platform,
        }
    }

    /// A sandbox in `dir`, with settings kept in memory and no network access.
    #[cfg(test)]
    pub fn sandbox(dir: PathBuf) -> Env {
        Env { request_file: dir.join("request.txt"), dir, check_updates: false, settings: Store::Memory(RefCell::default()) }
    }

    pub fn setting(&self, name: &str) -> Option<u32> {
        match &self.settings {
            Store::Platform => platform::setting(name),
            Store::Memory(values) => values.borrow().get(name).copied(),
        }
    }

    pub fn set_setting(&self, name: &str, value: u32) {
        match &self.settings {
            Store::Platform => platform::set_setting(name, value),
            Store::Memory(values) => {
                values.borrow_mut().insert(name.to_owned(), value);
            }
        }
    }
}

/// Where a second `--gallery` launch leaves its request for the running library.
pub fn request_file() -> PathBuf {
    std::env::temp_dir().join("rsnip-gallery-request.txt")
}
