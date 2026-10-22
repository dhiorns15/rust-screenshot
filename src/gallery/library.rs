//! The library panel: screenshots grouped by date, with multi-select, drag-out and bulk actions.

use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Align, Align2, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use super::{dragout, open_with_shell, show_in_folder, App, THUMB};

const ROW_HEIGHT: f32 = THUMB.y + 40.0;
const HEADER_HEIGHT: f32 = 30.0;

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
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        // FILETIME counts 100 ns ticks since 1601.
        let since_1970 = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        let ticks = since_1970.as_nanos() as u64 / 100 + 116_444_736_000_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
        unsafe {
            if FileTimeToSystemTime(&ft, &mut utc) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
                local = utc;
            }
        }
        Day { year: local.wYear as i32, month: local.wMonth as u32, day: local.wDay as u32 }
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
}

impl App {
    pub(super) fn library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Library");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Folder").on_hover_text(self.dir.display().to_string()).clicked() {
                    open_with_shell(&self.dir);
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search").desired_width(f32::INFINITY));
        ui.add_space(4.0);

        let filter = self.filter.to_lowercase();
        let visible: Vec<usize> =
            (0..self.entries.len()).filter(|&i| filter.is_empty() || self.entries[i].name.to_lowercase().contains(&filter)).collect();
        // Forget selected files that were deleted or renamed.
        self.selected.retain(|p| self.entries.iter().any(|e| &e.path == p));
        if self.selected.len() > 1 {
            self.bulk_bar(ui);
        } else {
            ui.label(RichText::new(format!("{} screenshots", visible.len())).small().weak());
        }
        if visible.is_empty() {
            ui.add_space(20.0);
            ui.label("No screenshots yet. Press your capture hotkey to take one.");
            return;
        }

        let today = Day::local(SystemTime::now());
        let mut rows = Vec::new();
        let mut last_label = String::new();
        for &i in &visible {
            let label = group_label(Day::local(self.entries[i].modified), today);
            if label != last_label {
                rows.push(Row::Header(label.clone()));
                last_label = label;
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
                // Only rows in view are drawn.
                if y + h >= viewport.min.y && y <= viewport.max.y {
                    let rect = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(width, h));
                    match row {
                        Row::Header(label) => {
                            let at = rect.left_bottom() + Vec2::new(6.0, -8.0);
                            ui.painter().text(at, Align2::LEFT_BOTTOM, label, egui::FontId::proportional(13.0), ui.visuals().strong_text_color());
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

        let order: Vec<PathBuf> = visible.iter().map(|&i| self.entries[i].path.clone()).collect();
        match action {
            Some(Action::Click(path, mods)) => self.click(path, mods, &order),
            Some(Action::Drag(path)) => {
                let paths = self.targets(&path, &order);
                // Blocks until the drop; Explorer, chat apps and mail clients take them as files.
                if let Err(e) = dragout::drag_files(&paths) {
                    self.set_status(format!("Couldn't drag: {e}"));
                }
            }
            Some(Action::Menu(path, label)) => {
                let paths = self.targets(&path, &order);
                match label {
                    "Open in default app" => paths.iter().for_each(|p| open_with_shell(p)),
                    "Show in folder" => show_in_folder(&path),
                    "Pin to screen" => paths.iter().for_each(|p| crate::pin::spawn(p)),
                    "Copy as files" => self.copy_files(&paths),
                    "Delete" => self.delete(&paths),
                    _ => {}
                }
            }
            None => {}
        }
    }

    /// One thumbnail row; returns what was done with it.
    fn item(&mut self, ui: &mut egui::Ui, rect: Rect, i: usize) -> Option<Action> {
        let entry = &self.entries[i];
        let response = ui.interact(rect, ui.id().with(("entry", &entry.path)), Sense::click_and_drag());
        let selected = self.selected.contains(&entry.path);
        let open = self.editor.as_ref().is_some_and(|e| e.doc.path == entry.path);
        let painter = ui.painter_at(rect);
        let fill = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
        if selected {
            painter.rect_filled(rect.shrink(2.0), 6.0, fill);
        } else if response.hovered() {
            painter.rect_filled(rect.shrink(2.0), 6.0, ui.visuals().widgets.hovered.bg_fill);
        }
        if open && !selected {
            painter.rect_stroke(rect.shrink(2.0), 6.0, Stroke::new(1.0f32, fill));
        }

        let thumb_area = Rect::from_min_size(rect.min + Vec2::new((rect.width() - THUMB.x) / 2.0, 8.0), THUMB);
        match self.thumbs.get(&entry.path) {
            Some((modified, tex)) if *modified == entry.modified => {
                let fit = (THUMB.x / tex.size_vec2().x).min(THUMB.y / tex.size_vec2().y);
                let r = Rect::from_center_size(thumb_area.center(), tex.size_vec2() * fit);
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(thumb_area, 4.0, Color32::from_gray(40));
                // MP4s have no thumbnail; GIFs show their first frame.
                let mp4 = entry.video && !entry.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"));
                if mp4 {
                    painter.text(thumb_area.center(), Align2::CENTER_CENTER, "▶", egui::FontId::proportional(36.0), Color32::from_gray(170));
                } else if self.requested.insert(entry.path.clone()) {
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
        if response.drag_started() {
            action = Some(Action::Drag(path.clone()));
        }
        response.context_menu(|ui| {
            let labels: &[&'static str] = if many {
                &["Copy as files", "Pin to screen", "Delete"]
            } else {
                &["Open in default app", "Show in folder", "Pin to screen", "Copy as files", "Delete"]
            };
            for &label in labels {
                let text = if many && label != "Pin to screen" { format!("{label} ({})", self.selected.len()) } else { label.to_string() };
                if ui.button(text).clicked() {
                    action = Some(Action::Menu(path.clone(), label));
                    ui.close_menu();
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
            if super::is_video(&path) {
                open_with_shell(&path);
            } else {
                self.request_open(path);
            }
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
        match crate::output::copy_files_to_clipboard(std::ptr::null_mut(), paths) {
            Ok(()) if paths.len() == 1 => self.set_status("Copied the file"),
            Ok(()) => self.set_status(format!("Copied {} files", paths.len())),
            Err(e) => self.set_status(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
