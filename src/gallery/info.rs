//! The gallery's info panel: what's known about the selected screenshot or recording, and what to
//! do with it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};

use super::library::Action;
use super::{doc, platform, player, theme, App, Entry};

/// The panel is only shown in a window at least this wide, so the gallery keeps its room.
pub(super) const MIN_WINDOW: f32 = 900.0;
/// How much of a screenshot's text the panel shows.
const TEXT_SHOWN: usize = 300;
/// And of its description; the rest is there when it's hovered.
const DESCRIPTION_SHOWN: usize = 360;
/// The preview is never taller than this.
const PREVIEW_HEIGHT: f32 = 260.0;

/// What's read from a file to describe it, kept while the file stays the same.
#[derive(Clone)]
pub(super) struct Details {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
    /// Width and height in pixels.
    size: Option<(u32, u32)>,
    /// How long a recording is.
    length: Option<Duration>,
    /// A recording with a sound track.
    sound: bool,
    /// Has annotations that can still be edited.
    edited: bool,
}

impl Details {
    fn read(entry: &Entry) -> Details {
        let (size, length, sound) = if entry.video {
            let info = player::info(&entry.path);
            (info.size, info.duration, info.sound)
        } else {
            (image::image_dimensions(&entry.path).ok(), None, false)
        };
        Details { path: entry.path.clone(), modified: entry.modified, len: entry.len, size, length, sound, edited: !entry.video && doc::has_layers(&entry.path) }
    }
}

/// "PNG screenshot", "MP4 recording".
fn kind(path: &Path, video: bool) -> String {
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_uppercase();
    match (video, ext.as_str()) {
        (true, "WEBM") => "WebM recording".into(),
        (true, ext) => format!("{ext} recording"),
        (false, "PNG") => "PNG screenshot".into(),
        (false, "JPG" | "JPEG") => "JPG image".into(),
        (false, ext) => format!("{ext} image"),
    }
}

/// "512 bytes", "86 KB", "2.1 MB".
pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let (mut size, mut unit) = (bytes as f64 / 1024.0, 0);
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if size >= 100.0 {
        format!("{size:.0} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// "26 Oct 2026, 10:25", in local time.
pub(super) fn date_and_time(t: SystemTime) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (year, month, day, hour, minute) = platform::local_time(t);
    format!("{day} {} {year}, {hour:02}:{minute:02}", MONTHS[(month as usize).clamp(1, 12) - 1])
}

/// The first part of `text`, for the panel.
fn text_start(text: &str) -> String {
    shortened(text, TEXT_SHOWN)
}

/// The first `max` characters of `text`, with an ellipsis if there were more.
fn shortened(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_owned(),
    }
}

/// A small muted caption with what it says under it.
fn field(ui: &mut egui::Ui, caption: &str, value: &str) {
    caption_only(ui, caption);
    ui.label(value);
}

fn caption_only(ui: &mut egui::Ui, caption: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(caption).small().color(theme::palette(ui).muted));
}

impl App {
    /// The details of entry `i`, read again only if its file changed.
    fn details(&mut self, i: usize) -> Details {
        let entry = &self.entries[i];
        match &self.details {
            Some(d) if d.path == entry.path && d.modified == entry.modified && d.len == entry.len => d.clone(),
            _ => {
                let details = Details::read(entry);
                self.details = Some(details.clone());
                details
            }
        }
    }

    /// The panel beside the gallery: the selected screenshot or recording, or what several come to.
    pub(super) fn info(&mut self, ui: &mut egui::Ui) {
        // In list order, whichever way the gallery is sorted.
        let selected: Vec<usize> = (0..self.entries.len()).filter(|&i| self.selected.contains(&self.entries[i].path)).collect();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match selected[..] {
            [] => {
                ui.label(RichText::new("Select a screenshot or recording to see its details.").color(theme::palette(ui).muted));
            }
            [i] => self.info_one(ui, i),
            _ => self.info_many(ui, &selected),
        });
    }

    fn info_one(&mut self, ui: &mut egui::Ui, i: usize) {
        let details = self.details(i);
        let entry = &self.entries[i];
        let (path, modified, video) = (entry.path.clone(), entry.modified, entry.video);
        let meta = self.metas.get(&path).cloned().unwrap_or_default();

        self.preview(ui, i);
        ui.add_space(8.0);
        ui.label(RichText::new(path.file_name().unwrap_or_default().to_string_lossy()).strong().size(15.0));
        ui.add_space(4.0);
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::primary_button("Open")).clicked() {
                action = Some(Action::Open(path.clone()));
            }
            for (text, label) in [("Show in folder", "Show in folder"), ("Copy as file", "Copy as files"), ("Delete", "Delete")] {
                if ui.button(text).clicked() {
                    action = Some(Action::Menu(path.clone(), label));
                }
            }
        });

        field(ui, "Kind", &kind(&path, video));
        if let Some((width, height)) = details.size {
            field(ui, "Dimensions", &format!("{width} × {height} px"));
        }
        if let Some(length) = details.length {
            let sound = if details.sound { ", with sound" } else { "" };
            field(ui, "Length", &format!("{}{sound}", player::clock(length)));
        }
        field(ui, "File size", &human_size(details.len));
        field(ui, "Date", &date_and_time(modified));
        let from = match (meta.app.is_empty(), meta.window.is_empty()) {
            (false, false) => format!("{} — {}", meta.app, meta.window),
            (false, true) => meta.app.clone(),
            (true, false) => meta.window.clone(),
            (true, true) => String::new(),
        };
        if !from.is_empty() {
            field(ui, "From", &from);
        }
        if details.edited {
            field(ui, "Edited", "With annotations you can still change");
        }
        if !meta.pushed.is_empty() {
            let destinations = crate::push::load(&self.env);
            let places: Vec<(&str, Option<&String>)> = meta
                .pushed
                .iter()
                .filter_map(|(id, pushed)| Some((destinations.iter().find(|d| &d.id == id)?.name.as_str(), pushed.link.as_ref())))
                .collect();
            if !places.is_empty() {
                caption_only(ui, "Pushed to");
                for (name, link) in places {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if let Some(link) = link {
                            if ui.small_button("Copy link").on_hover_text(format!("Copy the link at {name}")).clicked() {
                                action = Some(Action::CopyLink(link.clone()));
                            }
                        }
                    });
                }
            }
        }
        if let Some(description) = meta.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            caption_only(ui, "Description");
            let shown = shortened(description, DESCRIPTION_SHOWN);
            let label = ui.label(&shown);
            if shown != description {
                label.on_hover_text(description);
            }
            if let Some(model) = meta.described_by.as_deref().filter(|m| !m.is_empty()) {
                ui.label(RichText::new(format!("Described by {model}")).small().color(theme::palette(ui).muted));
            }
        }
        let text = meta.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if let Some(text) = text {
            field(ui, "Text", &text_start(text));
            if ui.small_button("Copy text").on_hover_text("Copy all the text read from it").clicked() {
                self.copy_words(text);
            }
        }

        if let Some(action) = action {
            self.apply_action(ui.ctx(), action, &[path]);
        }
    }

    /// Several selected: how many and how big, and what to do with them all.
    fn info_many(&mut self, ui: &mut egui::Ui, selected: &[usize]) {
        let total: u64 = selected.iter().map(|&i| self.entries[i].len).sum();
        let paths: Vec<PathBuf> = selected.iter().map(|&i| self.entries[i].path.clone()).collect();
        ui.label(RichText::new(format!("{} selected, {} in all", paths.len(), human_size(total))).strong().size(15.0));
        ui.add_space(4.0);
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for (text, label) in [("Copy as files", "Copy as files"), ("Delete", "Delete")] {
                if ui.button(format!("{text} ({})", paths.len())).clicked() {
                    action = Some(Action::Menu(paths[0].clone(), label));
                }
            }
        });
        if let Some(action) = action {
            self.apply_action(ui.ctx(), action, &paths);
        }
    }

    /// The thumbnail, as wide as the panel; it's asked for if it's not loaded yet (it may be
    /// somewhere in the gallery that isn't drawn).
    fn preview(&mut self, ui: &mut egui::Ui, i: usize) {
        let entry = &self.entries[i];
        let width = ui.available_width();
        let (area, _) = ui.allocate_exact_size(Vec2::new(width, (width * 9.0 / 16.0).min(PREVIEW_HEIGHT)), Sense::hover());
        ui.painter().rect_filled(area, 6.0, theme::palette(ui).raised);
        match self.thumbs.get(&entry.path) {
            Some((modified, tex)) if *modified == entry.modified => {
                let fit = (area.width() / tex.size_vec2().x).min(area.height() / tex.size_vec2().y);
                let shown = Rect::from_center_size(area.center(), tex.size_vec2() * fit);
                ui.painter().image(tex.id(), shown, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), egui::Color32::WHITE);
            }
            _ => {
                if self.requested.insert(entry.path.clone(), entry.modified) != Some(entry.modified) {
                    let _ = self.thumb_tx.send((entry.path.clone(), entry.modified));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_in_the_units_people_read() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1023), "1023 bytes");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(150 * 1024), "150 KB");
        assert_eq!(human_size(2_200_000), "2.1 MB");
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn kinds_say_what_the_file_is() {
        assert_eq!(kind(Path::new("a/Screenshot.png"), false), "PNG screenshot");
        assert_eq!(kind(Path::new("a.JPEG"), false), "JPG image");
        assert_eq!(kind(Path::new("a.mp4"), true), "MP4 recording");
        assert_eq!(kind(Path::new("a.gif"), true), "GIF recording");
        assert_eq!(kind(Path::new("a.webm"), true), "WebM recording");
    }

    #[test]
    fn long_text_is_cut_short() {
        assert_eq!(text_start("  Total due \n"), "Total due");
        let long = "word ".repeat(100);
        let shown = text_start(&long);
        assert!(shown.ends_with('…') && shown.chars().count() <= TEXT_SHOWN + 1, "{shown:?}");
        assert_eq!(text_start(&"é".repeat(TEXT_SHOWN)), "é".repeat(TEXT_SHOWN), "exactly as much as fits");
        assert_eq!(shortened("  A page. ", DESCRIPTION_SHOWN), "A page.");
    }
}
