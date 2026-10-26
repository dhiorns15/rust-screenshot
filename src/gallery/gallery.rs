//! The gallery page: every screenshot and recording as a grid of large thumbnails, sorted and grouped.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::iter::Peekable;
use std::ops::Range;
use std::path::PathBuf;
use std::str::Chars;
use std::time::SystemTime;

use eframe::egui::{self, Align, Key, Layout, Modifiers, Rect, RichText, Vec2};

use super::library::{group_label, search_hint, Action, Day, HEADER_HEIGHT};
use super::theme::{self, icon, IconButton};
use super::{App, Entry};

/// How wide the window is before the toolbar has room for what's counted or selected beside the
/// sort buttons, rather than on a row of its own.
pub(super) const ONE_ROW: f32 = 1100.0;
/// The least width of a thumbnail's cell; the cells share the row's width out evenly.
const CELL_MIN: f32 = 260.0;
/// Room around a thumbnail in its cell, and below it for the name.
const CELL_PAD: f32 = 8.0;
const NAME_HEIGHT: f32 = 32.0;

/// What the gallery is ordered by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sort {
    Date,
    Name,
    Size,
    Type,
}

impl Sort {
    pub(super) const SETTING: &'static str = "GallerySort";
    pub(super) const DESCENDING: &'static str = "GalleryDescending";
    const ALL: [Sort; 4] = [Sort::Date, Sort::Name, Sort::Size, Sort::Type];

    /// Unknown values (or none) mean the default, by date.
    pub(super) fn from_setting(value: Option<u32>) -> Sort {
        match value {
            Some(1) => Sort::Name,
            Some(2) => Sort::Size,
            Some(3) => Sort::Type,
            _ => Sort::Date,
        }
    }

    pub(super) fn to_setting(self) -> u32 {
        Sort::ALL.iter().position(|&s| s == self).unwrap_or(0) as u32
    }

    fn label(self) -> &'static str {
        match self {
            Sort::Date => "Date",
            Sort::Name => "Name",
            Sort::Size => "Size",
            Sort::Type => "Type",
        }
    }

    /// Whether it's backwards (newest or largest first), as set, or by default: newest and
    /// largest first, and names from A.
    pub(super) fn descending(self, setting: Option<u32>) -> bool {
        setting.map_or(matches!(self, Sort::Date | Sort::Size), |value| value == 1)
    }

    /// What the order is called, for the button that reverses it.
    fn order(self, descending: bool) -> &'static str {
        match (self, descending) {
            (Sort::Date, true) => "Newest first",
            (Sort::Date, false) => "Oldest first",
            (Sort::Name, false) => "A–Z",
            (Sort::Name, true) => "Z–A",
            (Sort::Size, true) => "Largest first",
            (Sort::Size, false) => "Smallest first",
            (Sort::Type, false) => "Screenshots first",
            (Sort::Type, true) => "Recordings first",
        }
    }
}

/// The label of a group by type, which also keeps it folded (see `library::TYPE_PREFIX`).
pub(super) fn type_label(video: bool) -> &'static str {
    if video {
        "type:Recordings"
    } else {
        "type:Screenshots"
    }
}

/// Names the way a person would order them: ignoring case, and numbers by their value, so
/// "Shot 2" comes before "Shot 10".
fn natural(a: &str, b: &str) -> Ordering {
    fn number(chars: &mut Peekable<Chars>) -> String {
        let mut digits = String::new();
        while let Some(c) = chars.next_if(char::is_ascii_digit) {
            digits.push(c);
        }
        digits.trim_start_matches('0').to_owned()
    }
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (x, y) = (number(&mut a), number(&mut b));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Puts the entries `shown` (indexes into `entries`) in the order `sort` says, backwards if
/// `descending`; what's equal is newest first, then by name.
fn sort_entries(entries: &[Entry], shown: &mut [usize], sort: Sort, descending: bool) {
    shown.sort_by(|&a, &b| {
        let (a, b) = (&entries[a], &entries[b]);
        let by = match sort {
            Sort::Date => a.modified.cmp(&b.modified),
            Sort::Name => natural(&a.name, &b.name),
            Sort::Size => a.len.cmp(&b.len),
            Sort::Type => a.video.cmp(&b.video),
        };
        let by = if descending { by.reverse() } else { by };
        by.then_with(|| b.modified.cmp(&a.modified)).then_with(|| natural(&a.name, &b.name))
    });
}

/// A group's screenshots, in the order they're shown.
struct Group {
    /// What the group is called and kept folded by; none for no heading.
    label: String,
    /// How many there are, folded away or not.
    count: usize,
    /// Indexes into `App::entries`; none while the group is folded.
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
        // Without a label, there's no heading: it's all one run.
        if !group.label.is_empty() {
            rows.push(Row::Header(g));
        }
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
    /// What the toolbar shows on the gallery page, after the page buttons: the search box, how it's
    /// sorted, how many there are or what to do with the selected ones, and Settings and the
    /// folder.
    pub(super) fn gallery_bar(&mut self, ui: &mut egui::Ui, visible: &[usize], one_row: bool) {
        // The search box gives way when the rest needs the room: the sort buttons, the folder
        // buttons, and what's counted or selected if that's on this row too.
        let others = match (one_row, self.selected.len() > 1) {
            (false, _) => 410.0,
            (true, false) => 510.0,
            (true, true) => 740.0,
        };
        let search = (ui.available_width() - others).clamp(120.0, 260.0);
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(search_hint()).desired_width(search));
        ui.label(RichText::new("Sort:").color(theme::palette(ui).muted));
        theme::group(ui, |ui| {
            for sort in Sort::ALL {
                if ui.add(egui::Button::selectable(self.sort == sort, sort.label())).clicked() && self.sort != sort {
                    self.sort = sort;
                    // Each starts in its usual direction, which the button next to them reverses.
                    self.descending = sort.descending(None);
                    self.env.set_setting(Sort::SETTING, sort.to_setting());
                    self.env.set_setting(Sort::DESCENDING, self.descending as u32);
                }
            }
        });
        let order = self.sort.order(self.descending);
        // Phosphor names the two by their bars; the arrow points the way the list runs.
        let arrow = if self.descending { icon::SORT_ASCENDING } else { icon::SORT_DESCENDING };
        if ui.add(IconButton::new(arrow, order)).on_hover_text(format!("{order} (click to reverse)")).clicked() {
            self.descending = !self.descending;
            self.env.set_setting(Sort::DESCENDING, self.descending as u32);
        }
        if one_row {
            self.count_or_bulk_bar(ui, visible);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.folder_buttons(ui));
    }

    /// The `visible` entries in the gallery's order, in the groups it has headings for: dates when
    /// ordered by date, screenshots and recordings by type, and none by name or size.
    fn groups(&self, visible: &[usize]) -> Vec<Group> {
        let mut sorted = visible.to_vec();
        sort_entries(&self.entries, &mut sorted, self.sort, self.descending);
        let today = Day::local(SystemTime::now());
        let mut groups: Vec<Group> = Vec::new();
        for i in sorted {
            let entry = &self.entries[i];
            let label = match self.sort {
                Sort::Date => group_label(Day::local(entry.modified), today),
                Sort::Type => type_label(entry.video).to_owned(),
                Sort::Name | Sort::Size => String::new(),
            };
            if groups.last().is_none_or(|g| g.label != label) {
                groups.push(Group { label: label.clone(), count: 0, items: Vec::new() });
            }
            let group = groups.last_mut().unwrap();
            group.count += 1;
            if !self.folded(&label) {
                group.items.push(i);
            }
        }
        groups
    }

    pub(super) fn gallery(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        if visible.is_empty() {
            let text = if self.entries.is_empty() { "No screenshots yet. Press your capture hotkey to take one." } else { "Nothing matches." };
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(text).color(theme::palette(ui).muted)));
            return;
        }
        let groups = self.groups(visible);

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
    fn names_order_by_their_numbers_and_ignore_case() {
        let mut names = ["Shot 10", "shot 2", "Shot 1", "Shot 02", "Screenshot 2026-10-26 102531", "screenshot 2026-10-26 093000", "Shot"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["screenshot 2026-10-26 093000", "Screenshot 2026-10-26 102531", "Shot", "Shot 1", "shot 2", "Shot 02", "Shot 10"]);
        assert_eq!(natural("a", "A"), Ordering::Equal);
        assert_eq!(natural("a1", "a01"), Ordering::Equal, "leading zeros don't count");
    }

    #[test]
    fn sorts_are_remembered_as_numbers_and_start_in_their_usual_direction() {
        for sort in Sort::ALL {
            assert_eq!(Sort::from_setting(Some(sort.to_setting())), sort);
        }
        assert_eq!(Sort::from_setting(None), Sort::Date);
        assert_eq!(Sort::from_setting(Some(9)), Sort::Date);
        assert_eq!(Sort::ALL.map(|s| s.descending(None)), [true, false, true, false]);
        assert!(Sort::Name.descending(Some(1)) && !Sort::Date.descending(Some(0)), "what was set wins");
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
