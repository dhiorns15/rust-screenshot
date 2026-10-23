//! RSnip: a small open-source Snagit-style screenshot tool.
//!
//! On Windows it's the full app (see `win_main.rs`). Elsewhere only the screenshot library and
//! editor are available: `rsnip [file]`.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod crash;
mod gallery;
mod icon_art;
mod options;
mod shortcuts;
mod update;

#[cfg(windows)]
include!("win_main.rs");

#[cfg(not(windows))]
fn main() {
    crash::install();
    if std::env::args().any(|a| a == "--settings") {
        return gallery::run_settings();
    }
    if std::env::args().any(|a| a == "--tray") {
        return gallery::run_background();
    }
    if std::env::args().any(|a| a == "--tray-quit") {
        return gallery::quit_background();
    }
    if let Some(path) = std::env::args().skip_while(|a| a != "--write-icon").nth(1) {
        if let Err(e) = gallery::write_icon(std::path::Path::new(&path)) {
            eprintln!("Couldn't write the icon: {e}");
            std::process::exit(1);
        }
        return;
    }
    let file = std::env::args().skip(1).find(|a| !a.starts_with("--")).map(std::path::PathBuf::from);
    gallery::run(file);
}
