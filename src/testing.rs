//! Help for the tests: saying when one is skipped, and taking turns on the sound device.

#[cfg(windows)]
use std::sync::{Mutex, MutexGuard};

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

/// Tests that use the sound device (recording from it, or playing through it) take turns, so
/// their timings don't suffer from each other's. Hold it for the test. Only the Windows tests
/// use the device from the test itself (on Linux the recorder and player are other programs).
#[cfg(windows)]
pub fn sound_device() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
