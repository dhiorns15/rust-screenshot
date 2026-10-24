//! Pushing new screenshots and recordings by themselves, to the destinations set to
//! Automatically in Settings → Push to. One RSnip process does it: the tray app on Windows, the
//! background icon on Linux, else the library (whichever claims it first). Every few seconds it
//! looks in the save folder for what's been saved since the destination was set to Automatically
//! and hasn't been pushed there; a failure is tried again later, and reported if it keeps failing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{library_files, platform, search};
use crate::options;
use crate::push::{self, Destination};

/// How often the folder is looked at.
const EVERY: Duration = Duration::from_secs(5);
/// A file this new may still be being written.
const SETTLE: Duration = Duration::from_secs(2);
/// After a failure, how long until each try again; after the last, it's reported and left.
const RETRIES: [Duration; 3] = [Duration::from_secs(60), Duration::from_secs(5 * 60), Duration::from_secs(30 * 60)];

/// Pushes automatically from this process, while no other RSnip process does (it takes over when
/// that one ends, say the library closing with the background icon still there). `notify` shows
/// a desktop notification (title, text).
pub fn start(notify: impl Fn(&str, &str) + Send + 'static) {
    std::thread::spawn(move || {
        while !platform::claim_lock("Push") {
            std::thread::sleep(EVERY);
        }
        let mut tries = Tries::default();
        loop {
            run_once(&mut tries, SystemTime::now(), &notify);
            std::thread::sleep(EVERY);
        }
    });
}

/// Failed pushes: by destination id and file, how many tries so far and when to try next (`None`:
/// given up on, until RSnip starts again).
#[derive(Default)]
struct Tries(HashMap<(String, PathBuf), (usize, Option<SystemTime>)>);

impl Tries {
    /// Notes a failure at `now`; true if that was the last try.
    fn failed(&mut self, id: &str, file: &Path, now: SystemTime) -> bool {
        let entry = self.0.entry((id.to_owned(), file.to_path_buf())).or_insert((0, None));
        let wait = RETRIES.get(entry.0);
        *entry = (entry.0 + 1, wait.map(|wait| now + *wait));
        wait.is_none()
    }

    /// Whether it's waiting to be tried again, or given up on.
    fn waiting(&self, id: &str, file: &Path, now: SystemTime) -> bool {
        self.0.get(&(id.to_owned(), file.to_path_buf())).is_some_and(|(_, next)| next.is_none_or(|next| now < next))
    }
}

fn run_once(tries: &mut Tries, now: SystemTime, notify: &dyn Fn(&str, &str)) {
    let destinations: Vec<Destination> = platform::text_setting(options::DESTINATIONS)
        .and_then(|json| serde_json::from_str::<Vec<Destination>>(&json).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.auto)
        .collect();
    if destinations.is_empty() {
        return;
    }
    let dir = platform::screenshots_dir();
    let files: Vec<(PathBuf, SystemTime)> =
        library_files(&dir).into_iter().filter_map(|f| Some((f.clone(), std::fs::metadata(&f).and_then(|m| m.modified()).ok()?))).collect();
    for destination in &destinations {
        let due: Vec<PathBuf> = files.iter().filter(|(file, modified)| is_due(destination, file, *modified, now, tries)).map(|(f, _)| f.clone()).collect();
        if due.is_empty() {
            continue;
        }
        // Read once for all of them; the keyring may have to be unlocked.
        let secret = match destination.kind.secret_label() {
            None => Ok(None),
            Some(label) => crate::secrets::get(&destination.id).and_then(|s| s.map(Some).ok_or(format!("No {} is saved for it", label.to_lowercase()))),
        };
        for file in due {
            let result = secret.clone().and_then(|secret| push::push(destination, secret.as_deref(), &file));
            let result = result.and_then(|link| search::record_push(&file, &destination.id, link));
            match result {
                Ok(()) => {
                    tries.0.remove(&(destination.id.clone(), file));
                }
                Err(e) => {
                    if tries.failed(&destination.id, &file, now) && options::NOTIFY.read(platform::setting) {
                        let name = file.file_name().unwrap_or_default().to_string_lossy();
                        notify(&format!("Couldn't push to {}", destination.name), &format!("{name}: {e}"));
                    }
                }
            }
        }
    }
}

/// Whether `file` (last changed at `modified`) should be pushed to `destination` now: it's been
/// saved since the destination was set to Automatically, it's finished being written, it hasn't
/// been pushed there, and it isn't waiting to be tried again.
fn is_due(destination: &Destination, file: &Path, modified: SystemTime, now: SystemTime, tries: &Tries) -> bool {
    let Some(since) = destination.auto_since.filter(|_| destination.auto) else { return false };
    if modified < UNIX_EPOCH + Duration::from_secs(since) || now.duration_since(modified).unwrap_or_default() < SETTLE {
        return false;
    }
    if tries.waiting(&destination.id, file, now) {
        return false;
    }
    !search::load(file).pushed.contains_key(&destination.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::Kind;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn pushes_what_was_saved_since_once_its_written_and_not_twice() {
        let dir = super::super::tests::sandbox_dir("auto-push");
        let file = dir.join("Screenshot 2026-10-24 091500.png");
        std::fs::write(&file, b"png").unwrap();
        let mut d = Destination::new(Kind::Immich { server: "https://photos.example.com".into() });
        let tries = Tries::default();
        assert!(!is_due(&d, &file, at(2_000), at(2_100), &tries), "by hand only");
        d.auto = true;
        d.auto_since = Some(1_500);
        assert!(is_due(&d, &file, at(2_000), at(2_100), &tries));
        assert!(!is_due(&d, &file, at(1_000), at(2_100), &tries), "saved before it was set to Automatically");
        assert!(!is_due(&d, &file, at(2_099), at(2_100), &tries), "still being written");
        search::record_push(&file, &d.id, None).unwrap();
        assert!(!is_due(&d, &file, at(2_000), at(2_100), &tries), "pushed already");
        let other = Destination { auto: true, auto_since: Some(1_500), ..Destination::new(Kind::discord()) };
        assert!(is_due(&other, &file, at(2_000), at(2_100), &tries), "each destination for itself");
    }

    #[test]
    fn tries_again_later_then_reports_and_stops() {
        let dir = super::super::tests::sandbox_dir("auto-push-retry");
        let file = dir.join("a.png");
        std::fs::write(&file, b"png").unwrap();
        let d = Destination { auto: true, auto_since: Some(0), ..Destination::new(Kind::Immich { server: "https://photos.example.com".into() }) };
        let mut tries = Tries::default();
        let mut now = at(10_000);
        let mut waited = Vec::new();
        while !tries.failed(&d.id, &file, now) {
            assert!(!is_due(&d, &file, at(5_000), now, &tries), "waits");
            let mut later = now;
            while !is_due(&d, &file, at(5_000), later, &tries) {
                later += Duration::from_secs(10);
            }
            waited.push(later.duration_since(now).unwrap().as_secs());
            now = later;
        }
        assert_eq!(waited, [60, 300, 1800], "a minute, five, half an hour");
        assert!(!is_due(&d, &file, at(5_000), now + Duration::from_secs(86_400), &tries), "then left alone");
        let other = dir.join("b.png");
        std::fs::write(&other, b"png").unwrap();
        assert!(is_due(&d, &other, at(5_000), now, &tries), "others still go");
    }
}
