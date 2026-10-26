//! The gallery page: every screenshot and recording as a grid of large thumbnails, by date.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Align, Key, Layout, Modifiers, Rect, RichText, Vec2};

use super::library::{group_label, search_hint, Action, Day, HEADER_HEIGHT};
use super::{theme, App};

/// The least width of a thumbnail's cell; the cells share the row's width out evenly.
const CELL_MIN: f32 = 260.0;
/// Room around a thumbnail in its cell, and below it for the name.
const CELL_PAD: f32 = 8.0;
const NAME_HEIGHT: f32 = 32.0;

/// A date's screenshots, in the order they're shown.
struct Group {
    label: String,
    /// How many there are, folded away or not.
    count: usize,
    /// Indexes into `App::entries`; none while the date is folded.
    items: Vec<usize>,
}

/// What the grid is made of, top to bottom.
enum Row {
    /// Index into the groups.
    Header(usize),
    /// A run of cells: positions in the flat list of what's shown.
    Cells(Range<usize>),
}

/// How many cells fit across `width`.
fn columns(width: f32) -> usize {
    ((width / CELL_MIN).floor() as usize).max(1)
}

/// The groups' rows for `columns` across, and every item shown in order.
fn rows(groups: &[Group], columns: usize) -> (Vec<Row>, Vec<usize>) {
    let (mut rows, mut shown) = (Vec::new(), Vec::new());
    for (g, group) in groups.iter().enumerate() {
        rows.push(Row::Header(g));
        for chunk in group.items.chunks(columns) {
            rows.push(Row::Cells(shown.len()..shown.len() + chunk.len()));
            shown.extend_from_slice(chunk);
        }
    }
    (rows, shown)
}

/// Where an arrow key takes the selection from position `at` in what's shown, given the runs of
/// positions that are on one line. Up and down keep to the same column where there is one.
fn arrow(lines: &[Range<usize>], at: usize, key: Key) -> usize {
    let last = lines.last().map_or(0, |l| l.end.saturating_sub(1));
    let Some(line) = lines.iter().position(|l| l.contains(&at)) else { return at.min(last) };
    let column = at - lines[line].start;
    let same_column = |l: &Range<usize>| l.start + column.min(l.len() - 1);
    match key {
        Key::ArrowLeft => at.saturating_sub(1),
        Key::ArrowRight => (at + 1).min(last),
        Key::ArrowUp if line > 0 => same_column(&lines[line - 1]),
        Key::ArrowDown if line + 1 < lines.len() => same_column(&lines[line + 1]),
        _ => at,
    }
}

impl App {
    /// What the toolbar shows on the gallery page, after the page buttons: the search box, how
    /// many there are or what to do with the selected ones, and Settings and the folder.
    pub(super) fn gallery_bar(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(search_hint()).desired_width(260.0));
        self.count_or_bulk_bar(ui, visible);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.folder_buttons(ui));
    }

    pub(super) fn gallery(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        if visible.is_empty() {
            let text = if self.entries.is_empty() { "No screenshots yet. Press your capture hotkey to take one." } else { "Nothing matches." };
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(text).color(theme::palette(ui).muted)));
            return;
        }
        let today = Day::local(SystemTime::now());
        let labels: Vec<String> = visible.iter().map(|&i| group_label(Day::local(self.entries[i].modified), today)).collect();
        let mut groups: Vec<Group> = Vec::new();
        for (&i, label) in visible.iter().zip(&labels) {
            if groups.last().is_none_or(|g| g.label != *label) {
                groups.push(Group { label: label.clone(), count: 0, items: Vec::new() });
            }
            let group = groups.last_mut().unwrap();
            group.count += 1;
            if !self.folded(label) {
                group.items.push(i);
            }
        }

        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
            let width = ui.available_width();
            let columns = columns(width);
            let cell = Vec2::new(width / columns as f32, 0.0);
            let thumb = Vec2::new(cell.x - 2.0 * CELL_PAD, (cell.x - 2.0 * CELL_PAD) * 9.0 / 16.0);
            let cell = Vec2::new(cell.x, thumb.y + CELL_PAD + NAME_HEIGHT);
            let (rows, shown) = rows(&groups, columns);
            let height = |row: &Row| match row {
                Row::Header(_) => HEADER_HEIGHT,
                Row::Cells(_) => cell.y,
            };
            let order: Vec<PathBuf> = shown.iter().map(|&i| self.entries[i].path.clone()).collect();
            let lines: Vec<Range<usize>> = rows.iter().filter_map(|r| if let Row::Cells(c) = r { Some(c.clone()) } else { None }).collect();
            if let Some(done) = self.gallery_keys(ui.ctx(), &lines, &order) {
                action = Some(done);
            }

            ui.set_min_height(rows.iter().map(height).sum());
            let origin = ui.min_rect().min;
            let reveal = self.reveal.take().and_then(|path| order.iter().position(|p| *p == path));
            let mut y = 0.0;
            for row in &rows {
                let h = height(row);
                let line = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(width, h));
                if let (Row::Cells(c), Some(at)) = (row, reveal) {
                    if c.contains(&at) {
                        ui.scroll_to_rect(line, None);
                    }
                }
                if y + h >= viewport.min.y && y <= viewport.max.y {
                    match row {
                        Row::Header(g) => {
                            let text = Rect::from_min_max(line.min, line.max - Vec2::new(0.0, 4.0));
                            if let Some(done) = self.heading(ui, text, &groups[*g].label, groups[*g].count) {
                                action = Some(done);
                            }
                        }
                        Row::Cells(c) => {
                            for (k, &i) in shown[c.clone()].iter().enumerate() {
                                let rect = Rect::from_min_size(line.min + Vec2::new(k as f32 * cell.x, 0.0), cell);
                                if let Some(done) = self.item(ui, rect, i, thumb) {
                                    action = Some(done);
                                }
                            }
                        }
                    }
                }
                y += h;
            }
        });

        let order: Vec<PathBuf> = groups.iter().flat_map(|g| &g.items).map(|&i| self.entries[i].path.clone()).collect();
        if let Some(action) = action {
            self.apply_action(ui.ctx(), action, &order);
        }
    }

    /// The keyboard, when no text box wants it: arrows move the selection, Enter opens it,
    /// Ctrl+A selects everything shown and Esc nothing.
    fn gallery_keys(&mut self, ctx: &egui::Context, lines: &[Range<usize>], order: &[PathBuf]) -> Option<Action> {
        if ctx.egui_wants_keyboard_input() || self.pending.is_some() {
            return None;
        }
        let pressed = |m, k| ctx.input_mut(|i| i.consume_key(m, k));
        let at = self.anchor.as_ref().and_then(|a| order.iter().position(|p| p == a));
        for key in [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown] {
            if pressed(Modifiers::NONE, key) && !order.is_empty() {
                // With nothing to move from, the first one.
                let to = at.map_or(0, |at| arrow(lines, at, key));
                self.selected = [order[to].clone()].into();
                self.anchor = Some(order[to].clone());
                self.reveal = Some(order[to].clone());
                return None;
            }
        }
        if pressed(Modifiers::NONE, Key::Enter) && self.selected.len() == 1 {
            return self.selected.iter().next().map(|p| Action::Open(p.clone()));
        }
        if pressed(Modifiers::COMMAND, Key::A) {
            self.selected = order.iter().cloned().collect::<HashSet<_>>();
        }
        if pressed(Modifiers::NONE, Key::Escape) {
            self.selected.clear();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(label: &str, items: &[usize], count: usize) -> Group {
        Group { label: label.into(), count, items: items.to_vec() }
    }

    #[test]
    fn as_many_columns_as_fit() {
        assert_eq!(columns(100.0), 1, "at least one");
        assert_eq!(columns(CELL_MIN), 1);
        assert_eq!(columns(2.0 * CELL_MIN - 1.0), 1);
        assert_eq!(columns(2.0 * CELL_MIN), 2);
        assert_eq!(columns(1280.0), 4);
    }

    #[test]
    fn each_date_starts_a_new_row_and_folded_ones_have_only_a_heading() {
        let groups = [group("Today", &[0, 1, 2, 3, 4], 5), group("Yesterday", &[], 2), group("March 2026", &[7], 1)];
        let (rows, shown) = rows(&groups, 2);
        assert_eq!(shown, [0, 1, 2, 3, 4, 7]);
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Header(g) => format!("header {g}"),
                Row::Cells(c) => format!("{}..{}", c.start, c.end),
            })
            .collect();
        assert_eq!(kinds, ["header 0", "0..2", "2..4", "4..5", "header 1", "header 2", "5..6"]);
    }

    #[test]
    fn arrows_move_by_one_and_by_line() {
        // Rows of 3, 3, 1 (the last on its own, as a new date would be).
        let lines = [0..3, 3..6, 6..7];
        assert_eq!(arrow(&lines, 1, Key::ArrowRight), 2);
        assert_eq!(arrow(&lines, 2, Key::ArrowRight), 3, "on to the next line");
        assert_eq!(arrow(&lines, 6, Key::ArrowRight), 6, "stops at the end");
        assert_eq!(arrow(&lines, 0, Key::ArrowLeft), 0, "and the start");
        assert_eq!(arrow(&lines, 4, Key::ArrowUp), 1);
        assert_eq!(arrow(&lines, 4, Key::ArrowDown), 6, "the last line is short: its only one");
        assert_eq!(arrow(&lines, 1, Key::ArrowUp), 1, "nothing above the first line");
        assert_eq!(arrow(&lines, 6, Key::ArrowDown), 6, "or below the last");
        assert_eq!(arrow(&lines, 9, Key::ArrowLeft), 6, "an item that's gone: the nearest end");
    }
}
