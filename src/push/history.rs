//! What was pushed where, and how it went: a line of JSON a push, in a file next to the crash
//! reports. The library (pushing by hand) and the automatic pusher (maybe in another process) both
//! add to it; Settings shows the latest, and System info each destination's last.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Destination;

/// How many are kept.
const KEEP: usize = 200;
/// The file is cut back to `KEEP` once it's this big, rather than at every push.
const TRIM_AT: u64 = 128 * 1024;

/// One push.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// When, in Unix seconds.
    pub at: u64,
    /// The destination's id, and its name and kind then.
    pub id: String,
    pub name: String,
    pub kind: String,
    /// The file's name.
    pub file: String,
    /// Pushed by itself (Automatically) rather than chosen.
    pub auto: bool,
    /// What went wrong; `None` if it was pushed.
    pub error: Option<String>,
}

impl Entry {
    pub fn new(destination: &Destination, file: &Path, auto: bool, error: Option<String>) -> Entry {
        Entry {
            at: now(),
            id: destination.id.clone(),
            name: destination.name.clone(),
            kind: destination.kind.name().to_owned(),
            file: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            auto,
            // Enough to say what happened; a server's whole page isn't worth keeping.
            error: error.map(|e| e.chars().take(500).collect()),
        }
    }
}

/// `%LOCALAPPDATA%\RSnip\push-history.jsonl`, or `~/.local/state/rsnip/push-history.jsonl`.
pub fn file() -> PathBuf {
    crate::crash::dir().with_file_name("push-history.jsonl")
}

/// Adds `entry` to the history at `path`. A history that can't be written is no reason for a
/// push to fail, so this says nothing if it can't.
pub fn record(path: &Path, entry: &Entry) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(line) = serde_json::to_string(entry) else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > TRIM_AT) {
        let mut entries = load(path);
        entries.truncate(KEEP);
        let text: String = entries.iter().rev().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        let new = path.with_extension("jsonl.new");
        if std::fs::write(&new, text).is_ok() {
            let _ = std::fs::rename(&new, path);
        }
    }
}

/// The history at `path`, newest first; lines that can't be read are skipped.
pub fn load(path: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut entries: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    entries.reverse();
    entries
}

/// "just now", "5 min ago", "3 h ago", "2 days ago": how long before `now` `at` was.
pub fn ago(at: u64, now: u64) -> String {
    match now.saturating_sub(at) {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "a day ago".into(),
        s => format!("{} days ago", s / 86_400),
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::push::Kind;

    #[test]
    fn keeps_the_newest_first_and_cuts_back_when_big() {
        let dir = std::env::temp_dir().join(format!("rsnip-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("push-history.jsonl");
        assert!(load(&path).is_empty(), "none yet");
        let d = Destination::new(Kind::immich());
        record(&path, &Entry::new(&d, Path::new("/x/a.png"), false, None));
        record(&path, &Entry::new(&d, Path::new("/x/b.png"), true, Some("Immich didn't accept the API key".into())));
        let entries = load(&path);
        assert_eq!(entries.iter().map(|e| e.file.as_str()).collect::<Vec<_>>(), ["b.png", "a.png"]);
        assert_eq!((entries[0].auto, entries[0].kind.as_str(), entries[0].error.is_some()), (true, "Immich", true));

        // A damaged line is skipped, not the whole history.
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{not json\n").unwrap();
        assert_eq!(load(&path).len(), 2);

        // About 600 bytes a line, so it passes TRIM_AT after some 220.
        let long = "x".repeat(2000);
        for _ in 0..300 {
            record(&path, &Entry::new(&d, Path::new("/x/c.png"), true, Some(long.clone())));
        }
        let entries = load(&path);
        assert!((KEEP..=KEEP + 100).contains(&entries.len()), "{}", entries.len());
        assert!(std::fs::metadata(&path).unwrap().len() <= TRIM_AT + 1024, "cut back");
        assert_eq!((entries[0].file.as_str(), entries[0].error.as_ref().unwrap().len()), ("c.png", 500));
    }

    #[test]
    fn says_how_long_ago() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(1000, 1000 + 5 * 60), "5 min ago");
        assert_eq!(ago(1000, 1000 + 3 * 3600 + 5), "3 h ago");
        assert_eq!(ago(0, 86_400 + 5), "a day ago");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
        assert_eq!(ago(2000, 1000), "just now", "a clock that went back");
    }
}
