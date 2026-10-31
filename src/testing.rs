//! Help for the tests: saying when one is skipped.

/// Says a test was skipped, and why, on stderr and, where `RSNIP_SKIPS` names a file, on a line
/// added to it (CI shows that file). `cargo test` hides the stderr of a test that passes, so
/// without that a test that skipped looks the same as one that passed.
#[track_caller]
pub fn skipped(why: impl std::fmt::Display) {
    let at = std::panic::Location::caller();
    let line = format!("{}:{} skipped: {why}", at.file(), at.line());
    eprintln!("{line}");
    if let Some(path) = std::env::var_os("RSNIP_SKIPS") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}
