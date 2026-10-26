//! The library panel: screenshots grouped by date, with multi-select, drag-out and bulk actions.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Align, Align2, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::theme::{self, icon, IconButton};
use super::{open_with_shell, platform, search, App, Dock, Page, THUMB};

const ROW_HEIGHT: f32 = THUMB.y + 40.0;
pub(super) const HEADER_HEIGHT: f32 = 30.0;
/// A filmstrip column: a thumbnail and its name.
const COLUMN_WIDTH: f32 = THUMB.x + 16.0;
/// Room above the filmstrip for the date labels.
const LABEL_HEIGHT: f32 = 20.0;
/// Space between dates in the filmstrip.
const GROUP_GAP: f32 = 14.0;
/// A folded date in the filmstrip: just its heading, as a chip.
const FOLDED_WIDTH: f32 = 120.0;
/// Where the folded dates are kept, one a line.
pub const FOLDED_SETTING: &str = "CollapsedGroups";
/// The bottom library panel: header row, date labels, thumbnails, scroll bar and margins.
pub const STRIP_HEIGHT: f32 = 34.0 + LABEL_HEIGHT + ROW_HEIGHT + 16.0;

/// Where each filmstrip column starts, with a gap where the date changes; and the total width. A
/// folded date is one narrow column, which all its screenshots share.
fn strip_columns(labels: &[String], folded: &HashSet<String>) -> (Vec<f32>, f32) {
    let mut xs = Vec::with_capacity(labels.len());
    let mut x = 0.0;
    // Where this date's first column started.
    let mut start = 0.0;
    for (n, label) in labels.iter().enumerate() {
        let is_folded = folded.contains(label);
        if n == 0 || *label != labels[n - 1] {
            if n > 0 {
                x += GROUP_GAP;
            }
            start = x;
            if is_folded {
                x += FOLDED_WIDTH;
            }
        }
        if is_folded {
            xs.push(start);
        } else {
            xs.push(x);
            x += COLUMN_WIDTH;
        }
    }
    (xs, x)
}

/// The folded dates as they're kept in the settings, one a line. "Today" is never kept, so the
/// newest screenshots are always there to see when the library opens.
pub(super) fn parse_folded(text: &str) -> HashSet<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && *l != "Today").map(String::from).collect()
}

fn folded_text(folded: &HashSet<String>) -> String {
    let mut labels: Vec<&str> = folded.iter().map(String::as_str).filter(|l| *l != "Today").collect();
    labels.sort_unstable();
    labels.join("\n")
}

/// Where the label of a date spanning `start..end` goes when the strip is scrolled to `scroll`: at
/// its first column, or held at the left edge while its columns are scrolled past, but never beyond
/// its last column.
fn sticky_label_x(start: f32, end: f32, scroll: f32) -> f32 {
    scroll.clamp(start, (end - COLUMN_WIDTH).max(start))
}

pub(super) fn search_hint() -> String {
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
    /// A date, and how many screenshots it has.
    Header(String, usize),
    /// Index into `App::entries`.
    Item(usize),
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Header(..) => HEADER_HEIGHT,
            Row::Item(_) => ROW_HEIGHT,
        }
    }
}

/// What the user did in the list or the gallery this frame.
pub(super) enum Action {
    /// In the library a plain click selects and opens it...
    Click(PathBuf, egui::Modifiers),
    /// ...in the gallery it only selects (opening takes a double click or Enter).
    Select(PathBuf, egui::Modifiers),
    /// Open it in the editor or the player.
    Open(PathBuf),
    Drag(PathBuf),
    Menu(PathBuf, &'static str),
    /// Push this (or the selected ones, if it's one of them) there.
    Push(PathBuf, Box<crate::push::Destination>),
    CopyLink(String),
    /// Fold or unfold a date.
    ToggleGroup(String),
}

impl App {
    /// The entries the search shows, as indexes into `entries`; also forgets selected files that
    /// were deleted or renamed.
    pub(super) fn visible(&mut self) -> Vec<usize> {
        let none = search::Meta::default();
        let visible = (0..self.entries.len())
            .filter(|&i| {
                let entry = &self.entries[i];
                search::matches(&self.filter, &entry.name, self.metas.get(&entry.path).unwrap_or(&none))
            })
            .collect();
        self.selected.retain(|p| self.entries.iter().any(|e| &e.path == p));
        visible
    }

    pub(super) fn library(&mut self, ui: &mut egui::Ui) {
        let visible = self.visible();
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

        // What's in view, for Shift+click and for what actions apply to: not what's folded away.
        let order: Vec<PathBuf> = visible.iter().zip(&labels).filter(|(_, label)| !self.folded(label)).map(|(&i, _)| self.entries[i].path.clone()).collect();
        if let Some(action) = action {
            self.apply_action(ui.ctx(), action, &order);
        }
    }

    /// Does what the user did in the library or the gallery. `order` is what's in view, for
    /// Shift+click and for what an action on one of several selected applies to.
    pub(super) fn apply_action(&mut self, ctx: &egui::Context, action: Action, order: &[PathBuf]) {
        match action {
            Action::ToggleGroup(label) => self.toggle_group(&label),
            Action::Click(path, mods) => self.click(path, mods, order),
            Action::Select(path, mods) => self.select(path, mods, order),
            Action::Open(path) => {
                self.request_open(path);
                self.set_page(Page::Editor);
            }
            Action::Drag(path) => {
                let paths = self.targets(&path, order);
                // Blocks until the drop; Explorer, chat apps and mail clients take them as files.
                if let Err(e) = platform::drag_files(&paths) {
                    self.set_status(format!("Couldn't drag: {e}"));
                }
            }
            Action::Menu(path, label) => {
                let paths = self.targets(&path, order);
                match label {
                    "Open in default app" => paths.iter().for_each(|p| open_with_shell(p)),
                    "Show in folder" => self.env.show_in_folder(&path),
                    "Pin to screen" => paths.iter().for_each(|p| platform::pin_file(p)),
                    "Copy as files" => self.copy_files(&paths),
                    "Delete" => self.delete(&paths),
                    _ => {}
                }
            }
            Action::Push(path, destination) => {
                let paths = self.targets(&path, order);
                self.push_to(ctx, &destination, paths);
            }
            Action::CopyLink(link) => match self.env.copy_text(&link) {
                Ok(()) => self.set_status("Link copied"),
                Err(e) => self.set_status(format!("Couldn't copy: {e}")),
            },
        }
    }

    fn searching(&self) -> bool {
        !self.filter.trim().is_empty()
    }

    /// Searching shows everything that matches, folded dates or not.
    pub(super) fn folded(&self, label: &str) -> bool {
        !self.searching() && self.collapsed.contains(label)
    }

    /// What a date's heading says: whether it's folded (not while searching, when it can't be),
    /// the date, and how many screenshots it has.
    fn heading_text(&self, label: &str, count: usize) -> String {
        if self.searching() {
            return format!("{label} ({count})");
        }
        let caret = if self.folded(label) { icon::CARET_RIGHT } else { icon::CARET_DOWN };
        format!("{caret} {label} ({count})")
    }

    /// How wide a date's heading is, with a little room around it.
    fn heading_width(&self, ui: &egui::Ui, label: &str, count: usize) -> f32 {
        let text = ui.painter().layout_no_wrap(self.heading_text(label, count), egui::FontId::proportional(13.0), Color32::WHITE);
        text.size().x + 8.0
    }

    /// A date's heading, with its text at the bottom left of `rect`. Clicking it folds the date.
    pub(super) fn heading(&self, ui: &mut egui::Ui, rect: Rect, label: &str, count: usize) -> Option<Action> {
        let mut color = theme::palette(ui).muted;
        let mut action = None;
        if !self.searching() {
            let response = ui.interact(rect, ui.id().with(("group", label)), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{label}, {count}")));
            if response.hovered() {
                color = theme::palette(ui).text;
                ui.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                action = Some(Action::ToggleGroup(label.to_owned()));
            }
        }
        let at = rect.left_bottom() + Vec2::new(4.0, -4.0);
        ui.painter().text(at, Align2::LEFT_BOTTOM, self.heading_text(label, count), egui::FontId::proportional(13.0), color);
        action
    }

    /// A folded date in the filmstrip: a chip in place of its thumbnails.
    fn chip(&self, ui: &mut egui::Ui, rect: Rect, label: &str, count: usize) -> Option<Action> {
        let response = ui.interact(rect, ui.id().with(("group", label)), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{label}, {count}")));
        let p = theme::palette(ui);
        let fill = if response.hovered() { p.hover } else { p.raised };
        ui.painter().rect_filled(rect, 8.0, fill);
        let center = rect.center();
        ui.painter().text(center - Vec2::new(0.0, 24.0), Align2::CENTER_CENTER, icon::CARET_RIGHT, egui::FontId::proportional(16.0), p.muted);
        ui.painter().text(center, Align2::CENTER_CENTER, label, egui::FontId::proportional(12.0), p.text);
        ui.painter().text(center + Vec2::new(0.0, 18.0), Align2::CENTER_CENTER, format!("({count})"), egui::FontId::proportional(12.0), p.muted);
        response.clicked().then(|| Action::ToggleGroup(label.to_owned()))
    }

    /// Folds or unfolds a date, and remembers it. What's open in the editor stays open.
    fn toggle_group(&mut self, label: &str) {
        if !self.collapsed.remove(label) {
            self.collapsed.insert(label.to_owned());
            // Out of sight, so not selected: a bulk action shouldn't reach what can't be seen.
            let today = Day::local(SystemTime::now());
            let hidden: Vec<&PathBuf> = self.entries.iter().filter(|e| group_label(Day::local(e.modified), today) == label).map(|e| &e.path).collect();
            self.selected.retain(|p| !hidden.contains(&p));
        }
        self.env.set_text_setting(FOLDED_SETTING, &folded_text(&self.collapsed));
    }

    /// Where the library goes (left or bottom), and the screenshots folder; added right to left.
    fn library_buttons(&mut self, ui: &mut egui::Ui) {
        self.folder_buttons(ui);
        theme::group(ui, |ui| {
            for (dock, icon, label) in [(Dock::Bottom, icon::SQUARE_HALF_BOTTOM, "Library along the bottom"), (Dock::Left, icon::SIDEBAR_SIMPLE, "Library on the left")] {
                if ui.add(IconButton::new(icon, label).selected(self.dock == dock)).on_hover_text(label).clicked() && self.dock != dock {
                    self.dock = dock;
                    self.env.set_setting(Dock::SETTING, dock.to_setting());
                }
            }
        });
    }

    /// Settings and the screenshots folder; added right to left.
    pub(super) fn folder_buttons(&mut self, ui: &mut egui::Ui) {
        if ui.add(IconButton::new(icon::GEAR_SIX, "Settings")).on_hover_text("Settings").clicked() {
            self.env.open_settings();
        }
        if ui.add(IconButton::new(icon::FOLDER_OPEN, "Open the folder")).on_hover_text(format!("Open the folder\n{}", self.env.dir.display())).clicked() {
            open_with_shell(&self.env.dir);
        }
    }

    /// How many of the `visible` entries there are, or what to do with the selected ones.
    pub(super) fn count_or_bulk_bar(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
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
                let count = labels[n..].iter().take_while(|l| **l == labels[n]).count();
                rows.push(Row::Header(labels[n].clone(), count));
            }
            if !self.folded(&labels[n]) {
                rows.push(Row::Item(i));
            }
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
                        Row::Header(label, count) => {
                            let text = Rect::from_min_max(rect.min, rect.max - Vec2::new(0.0, 4.0));
                            if let Some(done) = self.heading(ui, text, label, *count) {
                                action = Some(done);
                            }
                        }
                        Row::Item(i) => {
                            if let Some(done) = self.item(ui, rect, *i, THUMB) {
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
        let folded = if self.searching() { HashSet::new() } else { self.collapsed.clone() };
        let (xs, total) = strip_columns(labels, &folded);

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
                let is_folded = folded.contains(&labels[n]);
                let (start_x, end_x) = (xs[n], if is_folded { xs[n] + FOLDED_WIDTH } else { xs[end - 1] + COLUMN_WIDTH });
                if end_x >= viewport.min.x && start_x <= viewport.max.x && is_folded {
                    let rect = Rect::from_min_size(origin + Vec2::new(start_x, LABEL_HEIGHT + 8.0), Vec2::new(FOLDED_WIDTH, THUMB.y));
                    if let Some(done) = self.chip(ui, rect, &labels[n], end - n) {
                        action = Some(done);
                    }
                } else if end_x >= viewport.min.x && start_x <= viewport.max.x {
                    let label_x = sticky_label_x(start_x, end_x, viewport.min.x);
                    let heading = Rect::from_min_size(origin + Vec2::new(label_x, 0.0), Vec2::new(self.heading_width(ui, &labels[n], end - n), LABEL_HEIGHT));
                    if let Some(done) = self.heading(ui, heading, &labels[n], end - n) {
                        action = Some(done);
                    }
                    for m in n..end {
                        if xs[m] + COLUMN_WIDTH >= viewport.min.x && xs[m] <= viewport.max.x {
                            let rect = Rect::from_min_size(origin + Vec2::new(xs[m], LABEL_HEIGHT), Vec2::new(COLUMN_WIDTH, ROW_HEIGHT));
                            if let Some(done) = self.item(ui, rect, visible[m], THUMB) {
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
    pub(super) fn item(&mut self, ui: &mut egui::Ui, rect: Rect, i: usize, thumb: Vec2) -> Option<Action> {
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

        let thumb_area = Rect::from_min_size(rect.min + Vec2::new((rect.width() - thumb.x) / 2.0, 8.0), thumb);
        match self.thumbs.get(&entry.path) {
            Some((modified, tex)) if *modified == entry.modified => {
                let fit = (thumb.x / tex.size_vec2().x).min(thumb.y / tex.size_vec2().y);
                let r = Rect::from_center_size(thumb_area.center(), tex.size_vec2() * fit);
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(thumb_area, 4.0, theme::palette(ui).raised);
                if entry.video {
                    painter.text(thumb_area.center(), Align2::CENTER_CENTER, "▶", egui::FontId::proportional(36.0), Color32::from_gray(170));
                }
                if self.requested.insert(entry.path.clone(), entry.modified) != Some(entry.modified) {
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
        let gallery = self.page == Page::Gallery;
        if response.double_clicked() && gallery {
            action = Some(Action::Open(path.clone()));
        } else if response.clicked() {
            let mods = ui.input(|i| i.modifiers);
            action = Some(if gallery { Action::Select(path.clone(), mods) } else { Action::Click(path.clone(), mods) });
        }
        if response.drag_started() && platform::EXTRAS {
            action = Some(Action::Drag(path.clone()));
        }
        response.context_menu(|ui| {
            if let Some(done) = self.item_menu(ui, &path, many) {
                action = Some(done);
            }
        });
        action
    }

    /// The right-click menu of a thumbnail: what to do with it, or with all the selected ones if
    /// it's one of them (`many`).
    fn item_menu(&self, ui: &mut egui::Ui, path: &PathBuf, many: bool) -> Option<Action> {
        let mut action = None;
        let labels: &[&'static str] = if many {
            &["Copy as files", "Pin to screen", "Delete"]
        } else {
            &["Open in default app", "Show in folder", "Pin to screen", "Copy as files", "Delete"]
        };
        let windows_only = ["Pin to screen"];
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
        let unsaved = open.is_some_and(|open| open == path || (many && self.selected.contains(open)));
        ui.add_enabled_ui(!unsaved, |ui| ui.menu_button(theme::label(icon::CLOUD_ARROW_UP, &push), |ui| {
            let destinations = crate::push::load(&self.env);
            if destinations.is_empty() && ui.button("Set up places to push to…").clicked() {
                self.env.open_settings();
                ui.close();
            }
            for destination in destinations {
                if ui.add_enabled(self.pushing.is_none(), egui::Button::new(&destination.name)).on_disabled_hover_text("Still pushing").clicked() {
                    action = Some(Action::Push(path.clone(), Box::new(destination)));
                    ui.close();
                }
            }
        }))
        .response
        .on_disabled_hover_text("Save your edits first, so they go too");
        if !many {
            for (name, link) in self.links(path) {
                if ui.button(theme::label(icon::LINK, &format!("Copy link ({name})"))).clicked() {
                    action = Some(Action::CopyLink(link));
                    ui.close();
                }
            }
        }
        action
    }

    /// Ctrl+click toggles; Shift+click selects a range; a plain click selects one.
    fn select(&mut self, path: PathBuf, mods: egui::Modifiers, order: &[PathBuf]) {
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
            self.anchor = Some(path);
        }
    }

    /// In the library, a plain click also opens it.
    fn click(&mut self, path: PathBuf, mods: egui::Modifiers, order: &[PathBuf]) {
        self.select(path.clone(), mods, order);
        if !mods.command && !mods.shift {
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
            if ui.small_button("Copy as files").on_hover_text("Paste them into a folder, chat or email").clicked() {
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
        match self.env.copy_files(paths) {
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
        let (xs, total) = strip_columns(&labels, &HashSet::new());
        let c = COLUMN_WIDTH;
        assert_eq!(xs, [0.0, c, 2.0 * c + GROUP_GAP, 3.0 * c + 2.0 * GROUP_GAP, 4.0 * c + 2.0 * GROUP_GAP]);
        assert_eq!(total, 5.0 * c + 2.0 * GROUP_GAP);
        assert_eq!(strip_columns(&[], &HashSet::new()), (vec![], 0.0));
    }

    #[test]
    fn a_folded_date_is_one_narrow_column() {
        let labels: Vec<String> = ["Today", "Yesterday", "Yesterday", "Yesterday", "March 2026"].map(String::from).into();
        let folded: HashSet<String> = ["Yesterday".to_string(), "Last year".to_string()].into();
        let (xs, total) = strip_columns(&labels, &folded);
        let (c, f) = (COLUMN_WIDTH, FOLDED_WIDTH);
        let yesterday = c + GROUP_GAP;
        assert_eq!(xs, [0.0, yesterday, yesterday, yesterday, yesterday + f + GROUP_GAP]);
        assert_eq!(total, yesterday + f + GROUP_GAP + c);
    }

    #[test]
    fn folded_dates_are_kept_without_today() {
        let folded: HashSet<String> = ["Yesterday", "March 2026", "Today"].map(String::from).into();
        assert_eq!(folded_text(&folded), "March 2026\nYesterday");
        assert_eq!(parse_folded("March 2026\nToday\n\n  Yesterday \n"), ["March 2026", "Yesterday"].map(String::from).into());
        assert!(parse_folded("").is_empty());
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
