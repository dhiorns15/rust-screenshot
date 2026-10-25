//! The library panel: screenshots grouped by date, with multi-select, drag-out and bulk actions.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Align, Align2, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::theme::{self, icon, IconButton};
use super::{open_with_shell, platform, search, App, Dock, THUMB};

const ROW_HEIGHT: f32 = THUMB.y + 40.0;
const HEADER_HEIGHT: f32 = 30.0;
/// A filmstrip column: a thumbnail and its name.
const COLUMN_WIDTH: f32 = THUMB.x + 16.0;
/// Room above the filmstrip for the date labels.
const LABEL_HEIGHT: f32 = 20.0;
/// Space between dates in the filmstrip.
const GROUP_GAP: f32 = 14.0;
/// The bottom library panel: header row, date labels, thumbnails, scroll bar and margins.
pub const STRIP_HEIGHT: f32 = 34.0 + LABEL_HEIGHT + ROW_HEIGHT + 16.0;

/// Where each filmstrip column starts, with a gap where the date changes; and the total width.
fn strip_columns(labels: &[String]) -> (Vec<f32>, f32) {
    let mut xs = Vec::with_capacity(labels.len());
    let mut x = 0.0;
    for (n, label) in labels.iter().enumerate() {
        if n > 0 && *label != labels[n - 1] {
            x += GROUP_GAP;
        }
        xs.push(x);
        x += COLUMN_WIDTH;
    }
    (xs, x)
}

/// Where the label of a date spanning `start..end` goes when the strip is scrolled to `scroll`: at
/// its first column, or held at the left edge while its columns are scrolled past, but never beyond
/// its last column.
fn sticky_label_x(start: f32, end: f32, scroll: f32) -> f32 {
    scroll.clamp(start, (end - COLUMN_WIDTH).max(start))
}

fn search_hint() -> String {
    format!("{}  Search text, apps and names", icon::MAGNIFYING_GLASS)
}

/// A calendar day in local time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Day {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Day {
    /// Days since 1970-01-01 (proleptic Gregorian calendar).
    fn number(self) -> i64 {
        let (y, m) = if self.month <= 2 { (self.year as i64 - 1, self.month as i64 + 9) } else { (self.year as i64, self.month as i64 - 3) };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * m + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// 0 = Monday … 6 = Sunday.
    fn weekday(self) -> i64 {
        (self.number() + 3).rem_euclid(7)
    }

    /// The local calendar day of `t`.
    pub fn local(t: SystemTime) -> Day {
        let (year, month, day) = platform::local_date(t);
        Day { year, month, day }
    }
}

/// The library heading a screenshot from `day` sits under, seen on `today`.
pub fn group_label(day: Day, today: Day) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let (d, t) = (day.number(), today.number());
    if d >= t {
        "Today".into()
    } else if d == t - 1 {
        "Yesterday".into()
    } else if d >= t - today.weekday() {
        "This week".into()
    } else if (day.year, day.month) == (today.year, today.month) {
        "Earlier this month".into()
    } else {
        format!("{} {}", MONTHS[(day.month as usize).clamp(1, 12) - 1], day.year)
    }
}

/// The items from `a` to `b` inclusive, in list order, for Shift+click.
pub fn range(order: &[PathBuf], a: &PathBuf, b: &PathBuf) -> Vec<PathBuf> {
    let (Some(i), Some(j)) = (order.iter().position(|p| p == a), order.iter().position(|p| p == b)) else {
        return vec![b.clone()];
    };
    order[i.min(j)..=i.max(j)].to_vec()
}

enum Row {
    Header(String),
    /// Index into `App::entries`.
    Item(usize),
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Header(_) => HEADER_HEIGHT,
            Row::Item(_) => ROW_HEIGHT,
        }
    }
}

/// What the user did in the list this frame.
enum Action {
    Click(PathBuf, egui::Modifiers),
    Drag(PathBuf),
    Menu(PathBuf, &'static str),
    /// Push this (or the selected ones, if it's one of them) there.
    Push(PathBuf, crate::push::Destination),
    CopyLink(String),
}

impl App {
    pub(super) fn library(&mut self, ui: &mut egui::Ui) {
        let none = search::Meta::default();
        let visible: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let entry = &self.entries[i];
                search::matches(&self.filter, &entry.name, self.metas.get(&entry.path).unwrap_or(&none))
            })
            .collect();
        // Forget selected files that were deleted or renamed.
        self.selected.retain(|p| self.entries.iter().any(|e| &e.path == p));

        let bottom = self.dock == Dock::Bottom;
        if bottom {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Library").strong().size(15.0));
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(search_hint()).desired_width(220.0));
                self.count_or_bulk_bar(ui, &visible);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.library_buttons(ui));
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Library").strong().size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.library_buttons(ui));
            });
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(search_hint()).desired_width(f32::INFINITY));
            ui.horizontal_wrapped(|ui| self.count_or_bulk_bar(ui, &visible));
        }
        ui.add_space(4.0);
        if visible.is_empty() {
            ui.add_space(12.0);
            let text = if self.entries.is_empty() { "No screenshots yet. Press your capture hotkey to take one." } else { "Nothing matches." };
            ui.label(RichText::new(text).color(theme::palette(ui).muted));
            return;
        }

        let today = Day::local(SystemTime::now());
        let labels: Vec<String> = visible.iter().map(|&i| group_label(Day::local(self.entries[i].modified), today)).collect();
        let action = if bottom { self.strip(ui, &visible, &labels) } else { self.list(ui, &visible, &labels) };

        let order: Vec<PathBuf> = visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        match action {
            Some(Action::Click(path, mods)) => self.click(path, mods, &order),
            Some(Action::Drag(path)) => {
                let paths = self.targets(&path, &order);
                // Blocks until the drop; Explorer, chat apps and mail clients take them as files.
                if let Err(e) = platform::drag_files(&paths) {
                    self.set_status(format!("Couldn't drag: {e}"));
                }
            }
            Some(Action::Menu(path, label)) => {
                let paths = self.targets(&path, &order);
                match label {
                    "Open in default app" => paths.iter().for_each(|p| open_with_shell(p)),
                    "Show in folder" => self.env.show_in_folder(&path),
                    "Pin to screen" => paths.iter().for_each(|p| platform::pin_file(p)),
                    "Copy as files" => self.copy_files(&paths),
                    "Delete" => self.delete(&paths),
                    _ => {}
                }
            }
            Some(Action::Push(path, destination)) => {
                let paths = self.targets(&path, &order);
                self.push_to(ui.ctx(), &destination, paths);
            }
            Some(Action::CopyLink(link)) => match self.env.copy_text(&link) {
                Ok(()) => self.set_status("Link copied"),
                Err(e) => self.set_status(format!("Couldn't copy: {e}")),
            },
            None => {}
        }
    }

    /// Where the library goes (left or bottom), and the screenshots folder; added right to left.
    fn library_buttons(&mut self, ui: &mut egui::Ui) {
        if ui.add(IconButton::new(icon::GEAR_SIX, "Settings")).on_hover_text("Settings").clicked() {
            self.env.open_settings();
        }
        if ui.add(IconButton::new(icon::FOLDER_OPEN, "Open the folder")).on_hover_text(format!("Open the folder\n{}", self.env.dir.display())).clicked() {
            open_with_shell(&self.env.dir);
        }
        theme::group(ui, |ui| {
            for (dock, icon, label) in [(Dock::Bottom, icon::SQUARE_HALF_BOTTOM, "Library along the bottom"), (Dock::Left, icon::SIDEBAR_SIMPLE, "Library on the left")] {
                if ui.add(IconButton::new(icon, label).selected(self.dock == dock)).on_hover_text(label).clicked() && self.dock != dock {
                    self.dock = dock;
                    self.env.set_setting(Dock::SETTING, dock.to_setting());
                }
            }
        });
    }

    /// How many of the `visible` entries there are, or what to do with the selected ones.
    fn count_or_bulk_bar(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        if self.selected.len() > 1 {
            self.bulk_bar(ui);
        } else {
            let recordings = visible.iter().filter(|&&i| self.entries[i].video).count();
            let n = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
            let mut what = n(visible.len() - recordings, "screenshot", "screenshots");
            if recordings > 0 {
                what = format!("{what}, {}", n(recordings, "recording", "recordings"));
            }
            let reading = match self.reading.len() {
                0 => String::new(),
                n => format!(" · reading text, {n} left"),
            };
            ui.label(RichText::new(format!("{what}{reading}")).small().color(theme::palette(ui).muted));
        }
    }

    /// The left-hand list: date headings and one thumbnail per row. Only rows in view are drawn.
    fn list(&mut self, ui: &mut egui::Ui, visible: &[usize], labels: &[String]) -> Option<Action> {
        let mut rows = Vec::new();
        for (n, &i) in visible.iter().enumerate() {
            if n == 0 || labels[n] != labels[n - 1] {
                rows.push(Row::Header(labels[n].clone()));
            }
            rows.push(Row::Item(i));
        }
        let total: f32 = rows.iter().map(Row::height).sum();

        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
            ui.set_min_height(total);
            let origin = ui.min_rect().min;
            let width = ui.available_width();
            let mut y = 0.0;
            for row in &rows {
                let h = row.height();
                if y + h >= viewport.min.y && y <= viewport.max.y {
                    let rect = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(width, h));
                    match row {
                        Row::Header(label) => {
                            let at = rect.left_bottom() + Vec2::new(4.0, -8.0);
                            ui.painter().text(at, Align2::LEFT_BOTTOM, label, egui::FontId::proportional(13.0), theme::palette(ui).muted);
                        }
                        Row::Item(i) => {
                            if let Some(done) = self.item(ui, rect, *i) {
                                action = Some(done);
                            }
                        }
                    }
                }
                y += h;
            }
        });
        action
    }

    /// The bottom filmstrip: thumbnails side by side, with each date's label above its first one
    /// (kept in view while its screenshots are). Only columns in view are drawn.
    fn strip(&mut self, ui: &mut egui::Ui, visible: &[usize], labels: &[String]) -> Option<Action> {
        let (xs, total) = strip_columns(labels);

        let mut action = None;
        egui::ScrollArea::horizontal().auto_shrink(false).show_viewport(ui, |ui, viewport| {
            ui.set_min_size(Vec2::new(total, LABEL_HEIGHT + ROW_HEIGHT));
            // The mouse wheel scrolls the strip sideways.
            if ui.rect_contains_pointer(ui.clip_rect()) {
                let delta = ui.input(|i| i.smooth_scroll_delta);
                if delta.x == 0.0 && delta.y != 0.0 {
                    ui.scroll_with_delta(Vec2::new(delta.y, 0.0));
                }
            }
            let origin = ui.min_rect().min;
            let mut n = 0;
            while n < visible.len() {
                // One date's columns: n..end.
                let end = (n..visible.len()).find(|&m| labels[m] != labels[n]).unwrap_or(visible.len());
                let (start_x, end_x) = (xs[n], xs[end - 1] + COLUMN_WIDTH);
                if end_x >= viewport.min.x && start_x <= viewport.max.x {
                    let label_x = sticky_label_x(start_x, end_x, viewport.min.x);
                    let at = origin + Vec2::new(label_x + 4.0, LABEL_HEIGHT - 4.0);
                    ui.painter().text(at, Align2::LEFT_BOTTOM, &labels[n], egui::FontId::proportional(13.0), theme::palette(ui).muted);
                    for m in n..end {
                        if xs[m] + COLUMN_WIDTH >= viewport.min.x && xs[m] <= viewport.max.x {
                            let rect = Rect::from_min_size(origin + Vec2::new(xs[m], LABEL_HEIGHT), Vec2::new(COLUMN_WIDTH, ROW_HEIGHT));
                            if let Some(done) = self.item(ui, rect, visible[m]) {
                                action = Some(done);
                            }
                        }
                    }
                }
                n = end;
            }
        });
        action
    }

    /// One thumbnail row; returns what was done with it.
    fn item(&mut self, ui: &mut egui::Ui, rect: Rect, i: usize) -> Option<Action> {
        let entry = &self.entries[i];
        let response = ui.interact(rect, ui.id().with(("entry", &entry.path)), Sense::click_and_drag());
        let selected = self.selected.contains(&entry.path);
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &entry.name));
        // Where it came from, and why it matched the search.
        let tip = self.metas.get(&entry.path).and_then(|m| search::describe(&self.filter, m));
        let response = match tip {
            Some(tip) => response.on_hover_text(tip),
            None => response,
        };
        let open = self.editor.as_ref().is_some_and(|e| e.doc.path == entry.path);
        let painter = ui.painter_at(rect);
        let tile = rect.shrink(3.0);
        if selected {
            painter.rect_filled(tile, 8.0, theme::ACCENT.gamma_multiply(0.22));
            painter.rect_stroke(tile, 8.0, Stroke::new(1.5f32, theme::ACCENT), egui::StrokeKind::Middle);
        } else if response.hovered() {
            painter.rect_filled(tile, 8.0, theme::palette(ui).raised);
        }
        if open && !selected {
            painter.rect_stroke(tile, 8.0, Stroke::new(1.0f32, theme::ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Middle);
        }

        let thumb_area = Rect::from_min_size(rect.min + Vec2::new((rect.width() - THUMB.x) / 2.0, 8.0), THUMB);
        match self.thumbs.get(&entry.path) {
            Some((modified, tex)) if *modified == entry.modified => {
                let fit = (THUMB.x / tex.size_vec2().x).min(THUMB.y / tex.size_vec2().y);
                let r = Rect::from_center_size(thumb_area.center(), tex.size_vec2() * fit);
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(thumb_area, 4.0, theme::palette(ui).raised);
                if entry.video {
                    painter.text(thumb_area.center(), Align2::CENTER_CENTER, "▶", egui::FontId::proportional(36.0), Color32::from_gray(170));
                }
                if self.requested.insert(entry.path.clone()) {
                    let _ = self.thumb_tx.send((entry.path.clone(), entry.modified));
                }
            }
        }
        if entry.video {
            let ext = entry.path.extension().unwrap_or_default().to_string_lossy().to_uppercase();
            let badge = Rect::from_min_size(thumb_area.min + Vec2::new(6.0, 6.0), Vec2::new(34.0, 16.0));
            painter.rect_filled(badge, 3.0, Color32::from_black_alpha(180));
            painter.text(badge.center(), Align2::CENTER_CENTER, ext, egui::FontId::proportional(10.0), Color32::WHITE);
        }
        painter.text(
            Pos2::new(rect.center().x, thumb_area.max.y + 14.0),
            Align2::CENTER_CENTER,
            &entry.name,
            egui::FontId::proportional(12.0),
            ui.visuals().text_color(),
        );

        let path = entry.path.clone();
        let many = selected && self.selected.len() > 1;
        let mut action = None;
        if response.clicked() {
            action = Some(Action::Click(path.clone(), ui.input(|i| i.modifiers)));
        }
        if response.drag_started() && platform::EXTRAS {
            action = Some(Action::Drag(path.clone()));
        }
        response.context_menu(|ui| {
            let labels: &[&'static str] = if many {
                &["Copy as files", "Pin to screen", "Delete"]
            } else {
                &["Open in default app", "Show in folder", "Pin to screen", "Copy as files", "Delete"]
            };
            let windows_only = ["Copy as files", "Pin to screen"];
            for &label in labels.iter().filter(|l| platform::EXTRAS || !windows_only.contains(l)) {
                let text = if many && label != "Pin to screen" { format!("{label} ({})", self.selected.len()) } else { label.to_string() };
                if ui.button(text).clicked() {
                    action = Some(Action::Menu(path.clone(), label));
                    ui.close();
                }
            }
            ui.separator();
            let push = if many { format!("Push to ({})", self.selected.len()) } else { "Push to".into() };
            // What's pushed is the file, so the open one waits for its edits to be saved, as in Export.
            let open = self.editor.as_ref().filter(|e| e.doc.dirty()).map(|e| &e.doc.path);
            let unsaved = open.is_some_and(|open| *open == path || (many && self.selected.contains(open)));
            ui.add_enabled_ui(!unsaved, |ui| ui.menu_button(theme::label(icon::CLOUD_ARROW_UP, &push), |ui| {
                let destinations = crate::push::load(&self.env);
                if destinations.is_empty() && ui.button("Set up places to push to…").clicked() {
                    self.env.open_settings();
                    ui.close();
                }
                for destination in destinations {
                    if ui.add_enabled(self.pushing.is_none(), egui::Button::new(&destination.name)).on_disabled_hover_text("Still pushing").clicked() {
                        action = Some(Action::Push(path.clone(), destination));
                        ui.close();
                    }
                }
            }))
            .response
            .on_disabled_hover_text("Save your edits first, so they go too");
            if !many {
                for (name, link) in self.links(&path) {
                    if ui.button(theme::label(icon::LINK, &format!("Copy link ({name})"))).clicked() {
                        action = Some(Action::CopyLink(link));
                        ui.close();
                    }
                }
            }
        });
        action
    }

    /// Plain click opens and selects one; Ctrl+click toggles; Shift+click selects a range.
    fn click(&mut self, path: PathBuf, mods: egui::Modifiers, order: &[PathBuf]) {
        if mods.command {
            if !self.selected.remove(&path) {
                self.selected.insert(path.clone());
            }
            self.anchor = Some(path);
        } else if mods.shift {
            let anchor = self.anchor.clone().unwrap_or_else(|| path.clone());
            self.selected = range(order, &anchor, &path).into_iter().collect();
        } else {
            self.selected = [path.clone()].into();
            self.anchor = Some(path.clone());
            self.request_open(path);
        }
    }

    /// The files an action on `path` applies to: the whole selection if `path` is part of it.
    fn targets(&self, path: &PathBuf, order: &[PathBuf]) -> Vec<PathBuf> {
        if self.selected.contains(path) && self.selected.len() > 1 {
            order.iter().filter(|p| self.selected.contains(*p)).cloned().collect()
        } else {
            vec![path.clone()]
        }
    }

    fn bulk_bar(&mut self, ui: &mut egui::Ui) {
        let paths: Vec<PathBuf> = self.entries.iter().map(|e| e.path.clone()).filter(|p| self.selected.contains(p)).collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} selected", paths.len())).strong());
            if platform::EXTRAS && ui.small_button("Copy as files").on_hover_text("Paste them into a folder, chat or email").clicked() {
                self.copy_files(&paths);
            }
            if ui.small_button("Delete").on_hover_text("Move them to the Recycle Bin").clicked() {
                self.delete(&paths);
            }
            if ui.small_button("Clear").clicked() {
                self.selected.clear();
            }
        });
    }

    fn copy_files(&mut self, paths: &[PathBuf]) {
        match platform::copy_files(paths) {
            Ok(()) if paths.len() == 1 => self.set_status("Copied the file"),
            Ok(()) => self.set_status(format!("Copied {} files", paths.len())),
            Err(e) => self.set_status(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filmstrip_gaps_only_where_the_date_changes() {
        let labels: Vec<String> = ["Today", "Today", "Yesterday", "March 2026", "March 2026"].map(String::from).into();
        let (xs, total) = strip_columns(&labels);
        let c = COLUMN_WIDTH;
        assert_eq!(xs, [0.0, c, 2.0 * c + GROUP_GAP, 3.0 * c + 2.0 * GROUP_GAP, 4.0 * c + 2.0 * GROUP_GAP]);
        assert_eq!(total, 5.0 * c + 2.0 * GROUP_GAP);
        assert_eq!(strip_columns(&[]), (vec![], 0.0));
    }

    #[test]
    fn date_labels_stick_to_the_left_edge_within_their_columns() {
        let c = COLUMN_WIDTH;
        // A date with three columns starting at 100.
        let (start, end) = (100.0, 100.0 + 3.0 * c);
        assert_eq!(sticky_label_x(start, end, 0.0), start, "not scrolled to yet: at its first column");
        assert_eq!(sticky_label_x(start, end, 150.0), 150.0, "scrolled into: held at the edge");
        assert_eq!(sticky_label_x(start, end, end), end - c, "scrolled past: stays over its last column");
        // A single column never moves.
        assert_eq!(sticky_label_x(start, start + c, 160.0), start);
    }

    fn day(year: i32, month: u32, day: u32) -> Day {
        Day { year, month, day }
    }

    #[test]
    fn day_numbers_and_weekdays() {
        assert_eq!(day(1970, 1, 1).number(), 0);
        assert_eq!(day(2000, 3, 1).number(), 11_017);
        // 2026-10-22 is a Thursday.
        assert_eq!(day(2026, 10, 22).weekday(), 3);
    }

    #[test]
    fn groups_by_how_long_ago() {
        let today = day(2026, 10, 22); // Thursday
        assert_eq!(group_label(day(2026, 10, 22), today), "Today");
        assert_eq!(group_label(day(2026, 10, 21), today), "Yesterday");
        assert_eq!(group_label(day(2026, 10, 19), today), "This week", "Monday");
        assert_eq!(group_label(day(2026, 10, 18), today), "Earlier this month", "last Sunday");
        assert_eq!(group_label(day(2026, 9, 30), today), "September 2026");
        assert_eq!(group_label(day(2025, 12, 31), today), "December 2025");
        // Early in a month, last week can fall in the previous month.
        assert_eq!(group_label(day(2026, 9, 30), day(2026, 10, 1)), "Yesterday");
        assert_eq!(group_label(day(2026, 9, 28), day(2026, 10, 1)), "This week");
    }

    #[test]
    fn shift_click_selects_a_range_either_way() {
        let order: Vec<PathBuf> = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
        assert_eq!(range(&order, &order[1], &order[3]), order[1..=3]);
        assert_eq!(range(&order, &order[3], &order[1]), order[1..=3]);
        assert_eq!(range(&order, &PathBuf::from("gone"), &order[2]), [order[2].clone()]);
    }
}
