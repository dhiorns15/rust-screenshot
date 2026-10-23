//! The Copy text tool: the text recognised in the image picked out over it, to select and copy.

use std::sync::mpsc::{channel, Receiver, TryRecvError};

use ab_glyph::FontVec;
use eframe::egui::{self, Color32, Key, Painter, Pos2, Rect, Response, Ui};

use super::Editor;
use crate::gallery::doc::render;
use crate::gallery::theme::ACCENT;
use crate::gallery::words::{self, Word};

/// The text recognised in the image as it was at doc `version`.
pub(super) struct Recognised {
    version: u64,
    reading: Reading,
    /// The selected words, from where the selection started to where it ends, in reading order.
    selection: Option<(usize, usize)>,
}

enum Reading {
    Busy(Receiver<Result<Vec<Word>, String>>),
    Done(Result<Vec<Word>, String>),
}

/// What the Copy text tool has found, for the toolbar.
#[derive(Debug, PartialEq)]
pub enum TextStatus {
    /// Nothing on this system reads text.
    Unavailable,
    Reading,
    Failed(String),
    Found { words: usize, selected: usize },
}

impl Editor {
    /// Reads the image's text in the background, unless it's been read since it last changed.
    pub(super) fn read_text(&mut self, ctx: &egui::Context, font: Option<&FontVec>) {
        let Some(reader) = &self.reader else { return };
        if let Some(r) = self.recognised.as_mut().filter(|r| r.version == self.doc.version) {
            if let Reading::Busy(rx) = &r.reading {
                let done = match rx.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Disconnected) => Err("Text recognition stopped unexpectedly".into()),
                    Err(TryRecvError::Empty) => return,
                };
                r.reading = Reading::Done(done);
                // The toolbar above was drawn this frame while still reading; show what was found.
                ctx.request_repaint();
            }
            return;
        }
        // What's shown, annotations and all, so blurred or redacted text stays hidden.
        let img = render(self.doc.current(), font);
        let (tx, rx) = channel();
        let (reader, ctx) = (reader.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(reader(&img));
            ctx.request_repaint();
        });
        self.recognised = Some(Recognised { version: self.doc.version, reading: Reading::Busy(rx), selection: None });
    }

    pub fn text_status(&self) -> TextStatus {
        match self.recognised.as_ref().map(|r| &r.reading) {
            _ if self.reader.is_none() => TextStatus::Unavailable,
            None | Some(Reading::Busy(_)) => TextStatus::Reading,
            Some(Reading::Done(Err(e))) => TextStatus::Failed(e.clone()),
            Some(Reading::Done(Ok(words))) => TextStatus::Found { words: words.len(), selected: self.selected_words().len() },
        }
    }

    /// The words read from the image as it is now.
    fn words(&self) -> &[Word] {
        match &self.recognised {
            Some(Recognised { version, reading: Reading::Done(Ok(words)), .. }) if *version == self.doc.version => words,
            _ => &[],
        }
    }

    fn selected_words(&self) -> &[Word] {
        match self.recognised.as_ref().and_then(|r| r.selection) {
            Some((a, b)) => self.words().get(a.min(b)..=a.max(b)).unwrap_or(&[]),
            None => &[],
        }
    }

    /// The selected text, if any is selected.
    pub fn selected_text(&self) -> Option<String> {
        let words = self.selected_words();
        (!words.is_empty()).then(|| words::text(words))
    }

    /// All the text read from the image, if there is any.
    pub fn all_text(&self) -> Option<String> {
        let words = self.words();
        (!words.is_empty()).then(|| words::text(words))
    }

    pub fn select_all_text(&mut self) {
        let last = self.words().len().checked_sub(1);
        self.select_words(last.map(|last| (0, last)));
    }

    fn select_words(&mut self, selection: Option<(usize, usize)>) {
        if let Some(r) = &mut self.recognised {
            r.selection = selection;
        }
    }

    /// The word under `p` (image pixels), allowing `slack` pixels around it.
    fn word_at(&self, p: Pos2, slack: f32) -> Option<usize> {
        self.words().iter().position(|w| w.rect.expand(slack).contains(p))
    }

    /// The word closest to `p` (image pixels) on the line of text closest to it.
    fn nearest_word(&self, p: Pos2) -> Option<usize> {
        let dx = |r: Rect| (r.left() - p.x).max(p.x - r.right()).max(0.0);
        let dy = |r: Rect| (r.top() - p.y).max(p.y - r.bottom()).max(0.0);
        let words = self.words();
        let line = words.iter().min_by(|a, b| dy(a.rect).total_cmp(&dy(b.rect)))?.line;
        (0..words.len()).filter(|&i| words[i].line == line).min_by(|&a, &b| dx(words[a].rect).total_cmp(&dx(words[b].rect)))
    }

    /// Selecting with the Copy text tool: drag across words, click one, or double- or triple-click
    /// a line.
    pub(super) fn text_selection_input(&mut self, ui: &Ui, response: &Response, to_img: impl Fn(Pos2) -> Pos2, scale: f32) {
        let slack = 4.0 / scale;
        if let Some(p) = response.hover_pos() {
            if self.text_drag || self.word_at(to_img(p), slack).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
        }
        if response.drag_started() {
            let start = ui.input(|i| i.pointer.press_origin()).and_then(|p| self.nearest_word(to_img(p)));
            self.select_words(start.map(|i| (i, i)));
            self.text_drag = start.is_some();
        }
        if let (true, Some(p)) = (self.text_drag, response.interact_pointer_pos()) {
            let end = self.nearest_word(to_img(p));
            if let (Some(end), Some(r)) = (end, &mut self.recognised) {
                if let Some(sel) = &mut r.selection {
                    sel.1 = end;
                }
            }
        }
        if response.drag_stopped() {
            self.text_drag = false;
        }
        if response.double_clicked() || response.triple_clicked() {
            let on = response.interact_pointer_pos().and_then(|p| self.word_at(to_img(p), slack));
            let line = on.map(|i| self.words()[i].line);
            let words = self.words();
            let first = words.iter().position(|w| Some(w.line) == line);
            let last = words.iter().rposition(|w| Some(w.line) == line);
            self.select_words(first.zip(last));
        } else if response.clicked() {
            let on = response.interact_pointer_pos().and_then(|p| self.word_at(to_img(p), slack));
            self.select_words(on.map(|i| (i, i)));
        }
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.select_words(None);
        }
    }

    /// Dims the image except its text, and tints the selected words.
    pub(super) fn paint_text_selection(&self, painter: &Painter, canvas: Rect, to_screen: impl Fn(Pos2) -> Pos2) {
        let words = self.words();
        let Some(tex) = &self.texture else { return };
        if words.is_empty() {
            return;
        }
        painter.rect_filled(canvas, 0.0, Color32::from_black_alpha(110));
        let size = self.doc.size();
        let image = Rect::from_min_size(Pos2::ZERO, size);
        let show = |r: Rect| {
            let r = r.expand(2.0).intersect(image);
            let uv = Rect::from_min_max((r.min.to_vec2() / size).to_pos2(), (r.max.to_vec2() / size).to_pos2());
            let screen = Rect::from_min_max(to_screen(r.min), to_screen(r.max));
            painter.image(tex.id(), screen, uv, Color32::WHITE);
            screen
        };
        for word in words {
            show(word.rect);
        }
        // The selection runs unbroken along each line, gaps between words included.
        let mut line: Option<(usize, Rect)> = None;
        for word in self.selected_words() {
            match &mut line {
                Some((n, r)) if *n == word.line => *r = r.union(word.rect),
                _ => {
                    if let Some((_, r)) = line {
                        painter.rect_filled(show(r), 2.0, ACCENT.gamma_multiply(0.45));
                    }
                    line = Some((word.line, word.rect));
                }
            }
        }
        if let Some((_, r)) = line {
            painter.rect_filled(show(r), 2.0, ACCENT.gamma_multiply(0.45));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::gallery::doc::Doc;

    fn editor(lines: &[&'static str]) -> Editor {
        let mut editor = Editor::new(Doc::from_image(image::RgbaImage::new(400, 200)));
        let words = words::example(lines);
        editor.reader = Some(Arc::new(move |_| Ok(words.clone())));
        editor
    }

    fn read(editor: &mut Editor) {
        let ctx = egui::Context::default();
        for _ in 0..200 {
            editor.read_text(&ctx, None);
            if editor.text_status() != TextStatus::Reading {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never finished reading");
    }

    #[test]
    fn selects_from_the_nearest_words_in_reading_order() {
        let mut editor = editor(&["Hello there", "second line here"]);
        read(&mut editor);
        assert_eq!(editor.text_status(), TextStatus::Found { words: 5, selected: 0 });
        // From "there" back to "second": a selection made backwards reads forwards.
        let there = editor.nearest_word(Pos2::new(95.0, 20.0)).unwrap();
        let second = editor.nearest_word(Pos2::new(5.0, 52.0)).unwrap();
        editor.select_words(Some((there, second)));
        assert_eq!(editor.selected_text().as_deref(), Some("there\nsecond"));
        editor.select_all_text();
        assert_eq!(editor.selected_text(), editor.all_text());
        assert_eq!(editor.all_text().as_deref(), Some("Hello there\nsecond line here"));
    }

    #[test]
    fn prefers_words_on_the_same_line() {
        let mut editor = editor(&["a much longer first line", "b"]);
        read(&mut editor);
        // Right of "b", well past its end, but on its line.
        let i = editor.nearest_word(Pos2::new(200.0, 50.0)).unwrap();
        assert_eq!(editor.words()[i].text, "b");
    }

    #[test]
    fn an_edit_reads_the_text_again() {
        let mut editor = editor(&["one"]);
        read(&mut editor);
        editor.select_all_text();
        editor.doc.add(crate::gallery::doc::Shape::Redact { rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(50.0, 50.0)), color: Color32::BLACK });
        assert_eq!(editor.all_text(), None, "what was read before the edit is gone");
        read(&mut editor);
        assert_eq!(editor.text_status(), TextStatus::Found { words: 1, selected: 0 });
    }

    #[test]
    fn says_why_text_couldnt_be_read() {
        let mut editor = editor(&[]);
        editor.reader = Some(Arc::new(|_| Err("No language".into())));
        read(&mut editor);
        assert_eq!(editor.text_status(), TextStatus::Failed("No language".into()));
        editor.reader = None;
        assert_eq!(editor.text_status(), TextStatus::Unavailable);
    }
}
