//! Moving old screenshots and recordings to the Recycle Bin (the Trash on Linux), if Settings →
//! Library & editor says to. Run by the process that pushes automatically (see `auto_push`), a
//! minute after it starts and then daily. Whatever an automatic destination hasn't received yet
//! stays, so nothing goes before it's backed up.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{auto_push, doc, is_screenshot, library_files_with_times, platform};
use crate::options;
use crate::push::Destination;

/// How long after RSnip starts the first run is.
pub const FIRST: Duration = Duration::from_secs(60);
/// How often it runs after that.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The files, given with when each last changed, that haven't changed for `days` days and that no
/// automatic destination (of those given) has yet to get. Edited screenshots count by when they
/// were last saved. Only screenshots and recordings by name (see `is_screenshot`): pictures of the
/// user's own in the same folder are never moved.
pub fn due(files: &[(PathBuf, SystemTime)], days: u32, destinations: &[Destination], now: SystemTime) -> Vec<PathBuf> {
    let age = Duration::from_secs(u64::from(days) * 24 * 60 * 60);
    files
        .iter()
        .filter(|(file, _)| is_screenshot(file))
        .filter(|(_, modified)| now.duration_since(*modified).is_ok_and(|old| old >= age))
        .filter(|(file, modified)| !destinations.iter().any(|d| auto_push::to_push(d, file, *modified)))
        .map(|(file, _)| file.clone())
        .collect()
}

/// Moves what's due in `dir` away with `remove` (which says if it did), taking their edits and
/// search data along; returns how many moved.
fn move_due(dir: &Path, days: u32, destinations: &[Destination], now: SystemTime, remove: impl Fn(&Path) -> bool) -> usize {
    let mut moved = 0;
    for file in due(&library_files_with_times(dir), days, destinations, now) {
        if remove(&file) {
            doc::remove_layers(&file);
            moved += 1;
        }
    }
    moved
}

/// Cleans up now if Settings says to, noting it when any were moved.
pub fn run(now: SystemTime) {
    if !options::CLEAN_UP.read(platform::setting) {
        return;
    }
    // Never deleted for good: only moved where they can be put back from.
    let dir = platform::screenshots_dir();
    if !platform::has_recycle_bin(&dir) {
        return;
    }
    let days = options::CLEAN_UP_DAYS.read(platform::setting);
    let moved = move_due(&dir, days, &auto_push::destinations(), now, platform::recycle);
    if moved > 0 {
        let at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        platform::set_text_setting(options::CLEAN_UP_LAST, &format!("{at} {moved}"));
    }
}

/// When the last run that moved any did, and how many it moved, from its setting.
pub fn last_run(setting: Option<String>) -> Option<(u64, usize)> {
    let setting = setting?;
    let (at, moved) = setting.split_once(' ')?;
    Some((at.parse().ok()?, moved.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::Kind;

    const DAY: u64 = 24 * 60 * 60;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn immich(auto_since: Option<u64>) -> Destination {
        Destination { auto: auto_since.is_some(), auto_since, ..Destination::new(Kind::Immich { server: "https://photos.example.com".into() }) }
    }

    #[test]
    fn takes_whats_old_enough_and_nothing_an_automatic_push_is_waiting_on() {
        let dir = super::super::tests::sandbox_dir("clean-up-due");
        let now = at(1_000 * DAY);
        let old = (dir.join("Screenshot old.png"), at(900 * DAY));
        let recent = (dir.join("Screenshot recent.png"), at(990 * DAY));
        let exactly = (dir.join("Screenshot exactly.png"), now - Duration::from_secs(30 * DAY));
        // A picture of the user's own, kept in the same folder: never RSnip's to move.
        let photo = (dir.join("holiday.jpg"), at(100 * DAY));
        let files = [old.clone(), recent.clone(), exactly.clone(), photo];
        let names = |due: Vec<PathBuf>| due.into_iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();

        assert_eq!(names(due(&files, 30, &[], now)), ["Screenshot old.png", "Screenshot exactly.png"], "no destinations");
        assert_eq!(names(due(&files, 7, &[], now)), ["Screenshot old.png", "Screenshot recent.png", "Screenshot exactly.png"]);
        assert_eq!(names(due(&files, 365, &[], now)), Vec::<String>::new());
        assert_eq!(due(&[(dir.join("Screenshot future.png"), now + Duration::from_secs(DAY))], 7, &[], now), Vec::<PathBuf>::new(), "a clock that was wrong");

        // Set to Automatically before it was saved, and not pushed yet.
        let waiting = immich(Some(800 * DAY));
        assert!(due(&files, 30, std::slice::from_ref(&waiting), now).is_empty(), "kept until it's pushed");
        // Saved before then, so never pushed by itself.
        let later = immich(Some(975 * DAY));
        assert_eq!(names(due(&files, 30, &[later], now)), ["Screenshot old.png", "Screenshot exactly.png"]);
        // Only automatic ones count.
        let by_hand = Destination { auto: false, ..waiting.clone() };
        assert_eq!(names(due(&files, 30, &[by_hand], now)), ["Screenshot old.png", "Screenshot exactly.png"]);

        // Pushed since it last changed: free to go. Edited after that: kept again.
        std::fs::write(&old.0, b"png").unwrap();
        crate::gallery::search::record_push(&old.0, &waiting.id, None).unwrap();
        let pushed = crate::gallery::search::load(&old.0).pushed[&waiting.id].at;
        let now = at(pushed + 100 * DAY);
        assert_eq!(names(due(&[(old.0.clone(), at(pushed - 100))], 30, std::slice::from_ref(&waiting), now)), ["Screenshot old.png"], "pushed");
        assert!(due(&[(old.0.clone(), at(pushed + 5))], 30, std::slice::from_ref(&waiting), now).is_empty(), "edited since");
        // Any one still waiting is enough to keep it.
        let other = immich(Some(800 * DAY));
        assert!(due(&[(old.0.clone(), at(pushed - 100))], 30, &[waiting, other], now).is_empty(), "the other hasn't got it");
    }

    #[test]
    fn moves_the_files_with_their_edits_and_search_data_and_leaves_the_rest() {
        let dir = super::super::tests::sandbox_dir("clean-up-run");
        std::fs::create_dir_all(dir.join(".rsnip")).unwrap();
        for file in ["Screenshot old.png", "Screenshot new.png", "Recording old.mp4", "notes.txt", "holiday.jpg", ".rsnip/Screenshot old.png.json", ".rsnip/Screenshot old.png.base.png", ".rsnip/Screenshot old.png.meta.json", ".rsnip/Screenshot new.png.json"] {
            std::fs::write(dir.join(file), b"data").unwrap();
        }
        let now = SystemTime::now();
        let aged = |name: &str, days: u64| std::fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(now - Duration::from_secs(days * DAY)).unwrap();
        aged("Screenshot old.png", 40);
        aged("Recording old.mp4", 31);
        aged("notes.txt", 400);
        aged("holiday.jpg", 400);
        aged("Screenshot new.png", 3);

        // Not the Recycle Bin: the tests mustn't touch the real one.
        let moved = move_due(&dir, 30, &[], now, |path| std::fs::remove_file(path).is_ok());
        assert_eq!(moved, 2);
        for gone in ["Screenshot old.png", "Recording old.mp4", ".rsnip/Screenshot old.png.json", ".rsnip/Screenshot old.png.base.png", ".rsnip/Screenshot old.png.meta.json"] {
            assert!(!dir.join(gone).exists(), "{gone}");
        }
        for kept in ["Screenshot new.png", "notes.txt", "holiday.jpg", ".rsnip/Screenshot new.png.json"] {
            assert!(dir.join(kept).exists(), "{kept}");
        }

        // One that can't be moved stays, with its edits.
        std::fs::write(dir.join("Screenshot stuck.png"), b"data").unwrap();
        std::fs::write(dir.join(".rsnip/Screenshot stuck.png.json"), b"data").unwrap();
        aged("Screenshot stuck.png", 90);
        assert_eq!(move_due(&dir, 30, &[], now, |_| false), 0);
        assert!(dir.join("Screenshot stuck.png").exists() && dir.join(".rsnip/Screenshot stuck.png.json").exists());
    }

    #[test]
    fn reads_when_it_last_moved_any() {
        assert_eq!(last_run(Some("1700000000 12".into())), Some((1_700_000_000, 12)));
        assert_eq!(last_run(None), None);
        assert_eq!(last_run(Some("soon".into())), None);
        assert_eq!(last_run(Some("1700000000 many".into())), None);
    }
}
