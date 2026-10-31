//! Finding screenshots by what's in them: the text read from each one, an AI model's description of
//! it (see `describe`), the app and window it was captured from, and its name. What's known about a screenshot is cached beside it, in
//! `.rsnip/<name>.meta.json`.
//!
//! Several processes note things there: the library (the text it read), the automatic pusher and
//! Settings' pushes (where it went) and the describer (what it shows). Each change reads the file
//! afresh and writes it back whole, holding the folder's `.rsnip/meta.lock` meanwhile (see
//! `update`), so none undoes another's, and no reader sees half a file.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::doc::FileStamp;

/// Whether the library reads the text in screenshots (on unless set to 0).
pub const SETTING: &str = "SearchText";

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Meta {
    /// The app the screenshot was captured from, e.g. "Google Chrome".
    #[serde(default)]
    pub app: String,
    /// That window's title.
    #[serde(default)]
    pub window: String,
    /// The text read from the screenshot, as the file was at `stamp`.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub stamp: Option<FileStamp>,
    /// What an AI model said it shows, with keywords. It describes the capture, so an edit later
    /// leaves it be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The model that wrote it, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub described_by: Option<String>,
    /// Where it's been pushed, by destination id (see `push`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pushed: BTreeMap<String, Pushed>,
}

/// One push of a screenshot or recording.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Pushed {
    /// When, in seconds since 1970.
    pub at: u64,
    /// Where it can be seen there, if the place gives a link.
    #[serde(default)]
    pub link: Option<String>,
    /// When the file was last changed as it was pushed (seconds since 1970), if that was noted:
    /// an edit saved during a long push is newer, so the automatic pusher sends it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u64>,
}

impl Pushed {
    /// The file as it was when pushed: the time it was last changed then, if known, else when the
    /// push finished (a push finishes after the file settles, so that's at least as late).
    pub fn of(&self) -> u64 {
        self.of.unwrap_or(self.at)
    }
}

/// Notes that `path` was pushed to the destination `id`, and the link to it there.
pub fn record_push(path: &Path, id: &str, link: Option<String>) -> Result<(), String> {
    record_push_of(path, id, link, None)
}

/// The same, for a push of the file as it was last changed at `modified` (what the automatic
/// pusher saw before pushing it).
pub fn record_push_of(path: &Path, id: &str, link: Option<String>, modified: Option<std::time::SystemTime>) -> Result<(), String> {
    let secs = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let at = secs(std::time::SystemTime::now());
    update(path, |meta| {
        meta.pushed.insert(id.to_owned(), Pushed { at, link, of: modified.map(secs) });
    })
}

pub(super) fn meta_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    Some(path.parent()?.join(".rsnip").join(format!("{name}.meta.json")))
}

/// What's known about `path`; nothing if it was never recorded, or the file is damaged.
pub fn load(path: &Path) -> Meta {
    meta_path(path).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Replaces what's known about `path` with `meta`, whole; the app changes part of it (`update`),
/// so another process's note isn't written over.
#[cfg(test)]
pub fn save(path: &Path, meta: &Meta) -> Result<(), String> {
    let file = meta_path(path).ok_or("Bad file name")?;
    let _lock = lock(file.parent().unwrap())?;
    write_whole(&file, meta)
}

/// Changes what's known about `path` with `change`, from the file as it is now, not a copy kept
/// since: another process may have noted something meanwhile, and the lock is held from reading
/// it to writing it back, so that's kept.
pub fn update(path: &Path, change: impl FnOnce(&mut Meta)) -> Result<(), String> {
    let file = meta_path(path).ok_or("Bad file name")?;
    let _lock = lock(file.parent().unwrap())?;
    let mut meta = load(path);
    change(&mut meta);
    write_whole(&file, &meta)
}

/// How long a writer waits for another to finish before going ahead anyway: a lock left stuck
/// (which the system lets go of when its process ends, so only one that hangs) shouldn't freeze
/// the library.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// The lock writers in `dir` (a folder's `.rsnip`) take turns with: `meta.lock` there, held until
/// the file returned is dropped; `None` if it can't be had, and the write goes ahead without it.
/// Makes the folder, if need be.
fn lock(dir: &Path) -> Result<Option<File>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    super::platform::hide(dir);
    let Ok(file) = File::options().create(true).truncate(false).write(true).open(dir.join("meta.lock")) else { return Ok(None) };
    let until = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return Ok(None),
        }
    }
}

/// Writes `meta` to a new file beside `file`, then renames it over `file`, so a reader never finds
/// it half-written.
fn write_whole(file: &Path, meta: &Meta) -> Result<(), String> {
    let bytes = serde_json::to_vec(meta).map_err(|e| e.to_string())?;
    let temp = file.with_extension(format!("{}.saving", std::process::id()));
    let written = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

pub fn remove(path: &Path) {
    if let Some(file) = meta_path(path) {
        let _ = std::fs::remove_file(file);
    }
}

/// Notes the description `model` gave of `path`.
pub fn record_description(path: &Path, description: &str, model: &str) -> Result<(), String> {
    update(path, |meta| {
        meta.description = Some(description.to_owned());
        meta.described_by = Some(model.to_owned());
    })
}

/// Records where a capture came from, before its file is written.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn record_source(path: &Path, app: &str, window: &str) {
    let _ = update(path, |meta| {
        meta.app = app.to_owned();
        meta.window = window.to_owned();
    });
}

/// The text hasn't been read yet, or the file changed since (e.g. it was edited).
pub fn needs_text(meta: &Meta, path: &Path) -> bool {
    meta.text.is_none() || meta.stamp != FileStamp::of(path)
}

fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Every word of `query` appears somewhere in the name, app, window title, text or description,
/// ignoring case.
pub fn matches(query: &str, name: &str, meta: &Meta) -> bool {
    let fields = [name, &meta.app, &meta.window, meta.text.as_deref().unwrap_or(""), meta.description.as_deref().unwrap_or("")].map(str::to_lowercase);
    words(query).iter().all(|w| fields.iter().any(|f| f.contains(w.as_str())))
}

/// The first line of text, or else of the description, with a word of `query` in it, shortened (a
/// description's first line is a sentence or two, so it gets more room than a line of text).
pub fn matched_line(query: &str, meta: &Meta) -> Option<String> {
    fn found<'a>(text: &'a Option<String>, words: &[String]) -> Option<&'a str> {
        text.iter().flat_map(|t| t.lines()).map(str::trim).find(|l| {
            let l = l.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
        })
    }
    let words = words(query);
    let (line, max) = found(&meta.text, &words).map(|l| (l, 120)).or_else(|| found(&meta.description, &words).map(|l| (l, 400)))?;
    Some(match line.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None => line.to_owned(),
    })
}

/// A screenshot's tooltip: where it came from, and why it matched the search.
pub fn describe(query: &str, meta: &Meta) -> Option<String> {
    let mut lines = Vec::new();
    match (meta.app.is_empty(), meta.window.is_empty()) {
        (false, false) => lines.push(format!("{} — {}", meta.app, meta.window)),
        (false, true) => lines.push(meta.app.clone()),
        (true, false) => lines.push(meta.window.clone()),
        (true, true) => {}
    }
    if let Some(line) = matched_line(query, meta) {
        lines.push(format!("“{line}”"));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(app: &str, window: &str, text: &str) -> Meta {
        Meta { app: app.into(), window: window.into(), text: Some(text.into()), ..Meta::default() }
    }

    #[test]
    fn every_word_must_match_some_field() {
        let m = meta("Google Chrome", "Invoices - Acme", "Total due\nInvoice #42 for March");
        assert!(matches("", "Screenshot 1", &m), "an empty search matches everything");
        assert!(matches("invoice", "Screenshot 1", &m));
        assert!(matches("CHROME march", "Screenshot 1", &m), "words can match different fields, any case");
        assert!(matches("acme screenshot", "Screenshot 1", &m));
        assert!(!matches("invoice april", "Screenshot 1", &m), "all words must match");
        assert!(matches("shot", "Screenshot 1", &Meta::default()), "names still match without text");
    }

    #[test]
    fn the_description_is_searched_too() {
        let mut m = meta("Google Chrome", "Sales", "Q3");
        m.description = Some("A bar chart of quarterly sales.\nKeywords: chart, revenue".into());
        assert!(matches("BAR chart", "Screenshot 1", &m));
        assert!(matches("revenue q3 chrome", "Screenshot 1", &m), "any field, any word");
        assert!(!matches("pie", "Screenshot 1", &m));
        assert_eq!(matched_line("revenue", &m).as_deref(), Some("Keywords: chart, revenue"));
        assert_eq!(matched_line("q3", &m).as_deref(), Some("Q3"), "the text first");
        assert_eq!(describe("revenue", &m).unwrap(), "Google Chrome — Sales\n“Keywords: chart, revenue”");
    }

    #[test]
    fn the_description_is_kept_beside_the_text_and_pushes() {
        let dir = crate::gallery::tests::sandbox_dir("meta-description");
        let path = dir.join("shot.png");
        std::fs::write(&path, b"one").unwrap();
        record_push(&path, "dest", None).unwrap();
        let mut m = load(&path);
        m.text = Some("hello".into());
        save(&path, &m).unwrap();
        record_description(&path, "A page.", "claude-opus-5-5").unwrap();
        let m = load(&path);
        assert_eq!((m.text.as_deref(), m.description.as_deref(), m.described_by.as_deref()), (Some("hello"), Some("A page."), Some("claude-opus-5-5")));
        assert!(m.pushed.contains_key("dest"));
        std::fs::write(meta_path(&path).unwrap(), br#"{"app":"Notepad"}"#).unwrap();
        assert_eq!(load(&path).description, None, "older files have none");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn changes_made_at_once_are_all_kept_and_the_file_is_always_whole() {
        let dir = crate::gallery::tests::sandbox_dir("meta-at-once");
        let path = dir.join("shot.png");
        std::fs::write(&path, b"one").unwrap();
        record_source(&path, "Notepad", "notes.txt");
        // As the library, the pusher and the describer would, each noting its own thing while a
        // reader keeps looking: no note undoes another, and the reader never sees half a file.
        let writers: Vec<_> = (0..6)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for n in 0..30 {
                        match w % 3 {
                            0 => record_push(&path, &format!("dest{w}-{n}"), None).unwrap(),
                            1 => record_description(&path, &format!("Description {w}-{n}."), "m").unwrap(),
                            _ => update(&path, |m| m.text = Some(format!("text {w}-{n}"))).unwrap(),
                        }
                    }
                })
            })
            .collect();
        let reader = {
            let path = path.clone();
            std::thread::spawn(move || (0..300).all(|_| meta_path(&path).and_then(|p| std::fs::read(p).ok()).is_none_or(|b| serde_json::from_slice::<Meta>(&b).is_ok())))
        };
        for w in writers {
            w.join().unwrap();
        }
        assert!(reader.join().unwrap(), "a reader caught the file half-written");
        let m = load(&path);
        assert_eq!(m.pushed.len(), 60, "every push noted: {:?}", m.pushed.keys());
        assert_eq!((m.app.as_str(), m.window.as_str()), ("Notepad", "notes.txt"), "kept through it all");
        assert!(m.description.is_some() && m.text.is_some());
        let mut left: Vec<String> = std::fs::read_dir(dir.join(".rsnip")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["meta.lock", "shot.png.meta.json"], "nothing half-written left behind");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn describes_the_source_and_the_matching_line() {
        let m = meta("Google Chrome", "Invoices - Acme", "Total due\n   Invoice #42 for March  ");
        assert_eq!(matched_line("invoice", &m).as_deref(), Some("Invoice #42 for March"));
        assert_eq!(describe("invoice", &m).unwrap(), "Google Chrome — Invoices - Acme\n“Invoice #42 for March”");
        assert_eq!(describe("", &Meta::default()), None);
        let long = meta("", "", &"x".repeat(500));
        assert_eq!(matched_line("x", &long), Some(format!("{}…", "x".repeat(120))));
        // A description's first line has more room.
        let mut described = Meta { description: Some(format!("{}\nKeywords: y", "y".repeat(500))), ..Meta::default() };
        assert_eq!(matched_line("y", &described), Some(format!("{}…", "y".repeat(400))));
        described.description = Some("A short one.".into());
        assert_eq!(matched_line("short", &described).as_deref(), Some("A short one."));
    }

    #[test]
    fn metadata_round_trips_and_knows_when_it_is_stale() {
        let dir = crate::gallery::tests::sandbox_dir("meta");
        let path = dir.join("shot.png");
        std::fs::write(&path, b"one").unwrap();
        assert_eq!(load(&path), Meta::default(), "nothing recorded yet");
        record_source(&path, "Notepad", "notes.txt");
        let mut m = load(&path);
        assert_eq!((m.app.as_str(), m.window.as_str()), ("Notepad", "notes.txt"));
        assert!(needs_text(&m, &path));
        m.text = Some("hello".into());
        m.stamp = FileStamp::of(&path);
        save(&path, &m).unwrap();
        assert!(!needs_text(&load(&path), &path));
        std::fs::write(&path, b"edited").unwrap();
        assert!(needs_text(&load(&path), &path), "changed since it was read");
        std::fs::write(meta_path(&path).unwrap(), b"{garbage").unwrap();
        assert_eq!(load(&path), Meta::default(), "damaged metadata is ignored");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
