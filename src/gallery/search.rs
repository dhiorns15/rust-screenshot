//! Finding screenshots by what's in them: the text read from each one, the app and window it was
//! captured from, and its name. What's known about a screenshot is cached beside it, in
//! `.rsnip/<name>.meta.json`.

use std::path::{Path, PathBuf};

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
}

fn meta_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    Some(path.parent()?.join(".rsnip").join(format!("{name}.meta.json")))
}

/// What's known about `path`; nothing if it was never recorded, or the file is damaged.
pub fn load(path: &Path) -> Meta {
    meta_path(path).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(path: &Path, meta: &Meta) -> Result<(), String> {
    let file = meta_path(path).ok_or("Bad file name")?;
    let dir = file.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    super::platform::hide(dir);
    std::fs::write(file, serde_json::to_vec(meta).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn remove(path: &Path) {
    if let Some(file) = meta_path(path) {
        let _ = std::fs::remove_file(file);
    }
}

/// Records where a capture came from, before its file is written.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn record_source(path: &Path, app: &str, window: &str) {
    let meta = Meta { app: app.to_owned(), window: window.to_owned(), ..load(path) };
    let _ = save(path, &meta);
}

/// The text hasn't been read yet, or the file changed since (e.g. it was edited).
pub fn needs_text(meta: &Meta, path: &Path) -> bool {
    meta.text.is_none() || meta.stamp != FileStamp::of(path)
}

fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Every word of `query` appears somewhere in the name, app, window title or text, ignoring case.
pub fn matches(query: &str, name: &str, meta: &Meta) -> bool {
    let fields = [name, &meta.app, &meta.window, meta.text.as_deref().unwrap_or("")].map(str::to_lowercase);
    words(query).iter().all(|w| fields.iter().any(|f| f.contains(w.as_str())))
}

/// The first line of text with a word of `query` in it, shortened.
pub fn matched_line<'a>(query: &str, meta: &'a Meta) -> Option<&'a str> {
    let words = words(query);
    let line = meta.text.as_deref()?.lines().map(str::trim).find(|l| {
        let l = l.to_lowercase();
        words.iter().any(|w| l.contains(w.as_str()))
    })?;
    Some(match line.char_indices().nth(120) {
        Some((end, _)) => &line[..end],
        None => line,
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
        Meta { app: app.into(), window: window.into(), text: Some(text.into()), stamp: None }
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
    fn describes_the_source_and_the_matching_line() {
        let m = meta("Google Chrome", "Invoices - Acme", "Total due\n   Invoice #42 for March  ");
        assert_eq!(matched_line("invoice", &m), Some("Invoice #42 for March"));
        assert_eq!(describe("invoice", &m).unwrap(), "Google Chrome — Invoices - Acme\n“Invoice #42 for March”");
        assert_eq!(describe("", &Meta::default()), None);
        let long = meta("", "", &"x".repeat(500));
        assert_eq!(matched_line("x", &long).unwrap().len(), 120);
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
