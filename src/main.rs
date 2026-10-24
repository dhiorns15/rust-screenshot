//! RSnip: a small open-source Snagit-style screenshot tool.
//!
//! On Windows it's the full app (see `win_main.rs`). Elsewhere only the screenshot library and
//! editor are available: `rsnip [file]`.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod crash;
mod gallery;
mod http;
mod icon_art;
mod mp4;
mod options;
mod push;
mod secrets;
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
        return gallery::run_background(None);
    }
    if std::env::args().any(|a| a == "--record") {
        return gallery::record(true);
    }
    if std::env::args().any(|a| a == "--record-screen") {
        return gallery::record(false);
    }
    if let Some(frame) = std::env::args().skip_while(|a| a != "--pick-area").nth(1) {
        return gallery::pick_area(std::path::Path::new(&frame));
    }
    if std::env::args().any(|a| a == "--check-recording") {
        return gallery::check_recording();
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
