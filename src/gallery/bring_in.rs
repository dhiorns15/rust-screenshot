//! On Linux, screenshots come from the desktop's own tool, which saves them to the Pictures
//! folder's Screenshots whatever RSnip's folder is. When RSnip's is another, new ones are moved
//! into it as they're taken (Settings → Library & editor), so the library finds them there. Both
//! the background icon and the library do it; whichever gets to a file first moves it.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::options;

/// How long a new file is left alone, so it isn't moved while it's still being written.
const SETTLE: Duration = Duration::from_secs(1);

/// Moves new screenshots and recordings from `from` (the desktop's folder) into `to`, if that's
/// another folder and it's turned on; `setting` and `set_setting` are RSnip's settings. Only what
/// was taken since it was turned on moves: what was there before is for "Move them here". Returns
/// how many moved.
pub fn run(from: &Path, to: &Path, setting: impl Fn(&str) -> Option<u32>, set_setting: impl Fn(&str, u32)) -> usize {
    if from == to || !options::BRING_IN_SCREENSHOTS.read(&setting) || !from.is_dir() {
        return 0;
    }
    let now = SystemTime::now();
    let Some(since) = setting(options::BRING_IN_SINCE) else {
        // The first time: from now on.
        set_setting(options::BRING_IN_SINCE, now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32));
        return 0;
    };
    bring_in(from, to, UNIX_EPOCH + Duration::from_secs(since.into()), now)
}

/// Moves what in `from` was modified at or after `since`, and not in the last second, into `to`.
fn bring_in(from: &Path, to: &Path, since: SystemTime, now: SystemTime) -> usize {
    let mut moved = 0;
    for path in super::library_files(from) {
        let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
        if modified < since || now.duration_since(modified).unwrap_or_default() < SETTLE {
            continue;
        }
        if std::fs::create_dir_all(to).is_ok() && super::doc::move_shot(&path, to) == Ok(super::doc::Moved::Moved) {
            moved += 1;
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brings_in_whats_new_once_its_written() {
        let root = super::super::tests::sandbox_dir("bring-in");
        let (desktop, rsnip) = (root.join("Pictures/Screenshots"), root.join("RSnip"));
        std::fs::create_dir_all(&desktop).unwrap();
        let at = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);
        let shot = |name: &str, secs: u64| {
            std::fs::write(desktop.join(name), b"png").unwrap();
            std::fs::File::options().write(true).open(desktop.join(name)).unwrap().set_modified(at(secs)).unwrap();
        };
        shot("Screenshot from before.png", 1_000);
        shot("Screenshot from now.png", 2_000);
        shot("Screenshot being written.png", 2_010);
        std::fs::write(desktop.join("notes.txt"), b"not a screenshot").unwrap();
        assert_eq!(bring_in(&desktop, &rsnip, at(1_500), at(2_010)), 1);
        assert!(rsnip.join("Screenshot from now.png").exists());
        assert!(desktop.join("Screenshot from before.png").exists(), "taken before it was turned on");
        assert!(desktop.join("Screenshot being written.png").exists(), "not finished yet");
        assert!(desktop.join("notes.txt").exists());
        assert_eq!(bring_in(&desktop, &rsnip, at(1_500), at(2_012)), 1, "once it's settled");
    }

    #[test]
    fn starts_from_when_its_first_on_and_stays_off_when_off() {
        let root = super::super::tests::sandbox_dir("bring-in-settings");
        let (desktop, rsnip) = (root.join("Screenshots"), root.join("RSnip"));
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("a.png"), b"png").unwrap();
        // Taken a while ago, so it's settled.
        std::fs::File::options().write(true).open(desktop.join("a.png")).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(1_000)).unwrap();
        let settings = std::cell::RefCell::new(std::collections::HashMap::<String, u32>::new());
        let get = |name: &str| settings.borrow().get(name).copied();
        let set = |name: &str, value: u32| {
            settings.borrow_mut().insert(name.to_owned(), value);
        };
        assert_eq!(run(&desktop, &desktop, get, set), 0, "it's the same folder");
        assert_eq!(run(&desktop, &rsnip, get, set), 0);
        assert!(get(options::BRING_IN_SINCE).is_some(), "from now on");
        assert!(desktop.join("a.png").exists(), "what was there stays");
        set(options::BRING_IN_SINCE, 0);
        set(options::BRING_IN_SCREENSHOTS.name, 0);
        assert_eq!(run(&desktop, &rsnip, get, set), 0, "turned off");
        set(options::BRING_IN_SCREENSHOTS.name, 1);
        assert_eq!(run(&desktop, &rsnip, get, set), 1);
    }
}
