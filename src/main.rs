//! RSnip: a small open-source Snagit-style screenshot tool.
//!
//! On Windows it's the full app (see `win_main.rs`). Elsewhere only the screenshot library and
//! editor are available: `rsnip [file]`.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod gallery;
mod icon_art;
mod options;
mod shortcuts;
mod update;

#[cfg(windows)]
include!("win_main.rs");

#[cfg(not(windows))]
fn main() {
    if std::env::args().any(|a| a == "--settings") {
        return gallery::run_settings();
    }
    let file = std::env::args().skip(1).find(|a| !a.starts_with("--")).map(std::path::PathBuf::from);
    gallery::run(file);
}
