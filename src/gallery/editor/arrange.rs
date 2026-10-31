//! Copying, pasting and duplicating annotations.

use ab_glyph::FontVec;
use eframe::egui::{Event, Key, Pos2, Rect, Ui, Vec2};
use serde::{Deserialize, Serialize};

use super::{Editor, Tool, ToolState};
use crate::gallery::doc::{slid_inside, Shape};

/// How far a pasted or duplicated annotation lands from the one it copies, so it's seen to be new.
const PASTE_OFFSET: f32 = 16.0;

/// A copied annotation on the clipboard: `{"rsnip-shape": {...}}`, under a name of its own so
/// ordinary text is never taken for one.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Copied<S> {
    #[serde(rename = "rsnip-shape")]
    shape: S,
}

/// `shape` as the clipboard text Ctrl+C puts there.
pub fn shape_to_text(shape: &Shape) -> String {
    serde_json::to_string(&Copied { shape }).unwrap_or_default()
}

/// The annotation in clipboard text that `shape_to_text` made, if that's what it is. Not one that
/// couldn't be drawn (no points, say, or somewhere absurd).
pub fn shape_from_text(text: &str) -> Option<Shape> {
    let shape = serde_json::from_str::<Copied<Shape>>(text.trim()).ok()?.shape;
    let b = shape.bounds(None);
    let sane = |p: Pos2| p.x.abs() < 1e6 && p.y.abs() < 1e6;
    (b.is_finite() && sane(b.min) && sane(b.max)).then_some(shape)
}

/// What the editor's Ctrl+ shortcuts for the selection ask for this frame.
enum Shortcut {
    Duplicate,
}

/// Ctrl+D, from this frame's keys.
fn shortcut(ui: &Ui) -> Option<Shortcut> {
    ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            Event::Key { key: Key::D, pressed: true, modifiers, .. } if modifiers.command && !modifiers.shift => Some(Shortcut::Duplicate),
            _ => None,
        })
    })
}

impl Editor {
    /// The selected annotation, unless the doc has changed under the selection since (an undo in
    /// the same frame, say).
    pub(super) fn selected_index(&self) -> Option<usize> {
        let sel = self.selection.as_ref()?;
        (sel.version == self.doc.version && sel.index < self.doc.current().shapes.len()).then_some(sel.index)
    }

    /// The selected annotation as clipboard text, for Ctrl+C (and Ctrl+X); `None` when nothing is
    /// selected.
    pub fn copy_selected(&self) -> Option<String> {
        self.selected_index().map(|i| shape_to_text(&self.doc.current().shapes[i]))
    }

    /// Pastes the annotation that `text` (from the clipboard) holds, a little below and right of
    /// where it was copied from (further each time the same one is pasted again), kept on the
    /// image, and selects it; one undo step. False if `text` holds no annotation.
    pub fn paste(&mut self, text: &str, tools: &mut ToolState, font: Option<&FontVec>) -> bool {
        let Some(shape) = shape_from_text(text) else { return false };
        let times = match &mut self.pasted {
            Some((last, times)) if last == text => {
                *times += 1;
                *times
            }
            _ => {
                self.pasted = Some((text.to_owned(), 1));
                1
            }
        };
        self.place_copy(&shape, PASTE_OFFSET * times as f32, tools, font);
        true
    }

    /// Adds a copy of the selected annotation beside it and selects that (Ctrl+D), leaving the
    /// clipboard alone.
    fn duplicate_selected(&mut self, tools: &mut ToolState, font: Option<&FontVec>) {
        if let Some(i) = self.selected_index() {
            let shape = self.doc.current().shapes[i].clone();
            self.place_copy(&shape, PASTE_OFFSET, tools, font);
        }
    }

    /// Adds `shape` moved `offset` pixels right and down, then slid back onto the image if that
    /// took it off, and selects it. A step gets the next number, as a new one would.
    fn place_copy(&mut self, shape: &Shape, offset: f32, tools: &mut ToolState, font: Option<&FontVec>) {
        let moved = shape.translated(Vec2::splat(offset));
        let bounds = moved.bounds(font);
        let image = Rect::from_min_size(Pos2::ZERO, self.doc.size());
        let mut placed = moved.translated(slid_inside(bounds, image).min - bounds.min);
        if let Shape::Step { number, .. } = &mut placed {
            *number = self.doc.next_step_number();
        }
        self.doc.add(placed);
        // Tools that can't hold a selection give way to Select, so what was pasted can be moved.
        if matches!(tools.tool, Tool::Text | Tool::Crop | Tool::CutOut | Tool::CopyText) {
            tools.tool = Tool::Select;
            self.last_tool = Tool::Select;
        }
        self.select(self.doc.current().shapes.len() - 1, tools);
    }

    /// Ctrl+D duplicates the selection.
    pub(super) fn arrange_keys(&mut self, ui: &Ui, tools: &mut ToolState, font: Option<&FontVec>) {
        if self.selected_index().is_none() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if let Some(Shortcut::Duplicate) = shortcut(ui) {
            self.duplicate_selected(tools, font);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    #[test]
    fn only_a_copied_annotation_reads_as_one() {
        let arrow = Shape::Arrow { from: Pos2::new(1.0, 2.0), to: Pos2::new(30.0, 40.0), color: Color32::RED, width: 4.0 };
        let text = shape_to_text(&arrow);
        assert!(text.starts_with("{\"rsnip-shape\":"), "{text}");
        assert!(shape_from_text(&text) == Some(arrow.clone()));
        assert!(shape_from_text(&format!("  {text}\n")).is_some(), "whitespace around it is fine");

        let shape = serde_json::to_string(&arrow).unwrap();
        for not in ["", "hello", "{}", "null", &shape, &format!("{{\"rsnip-shape\": {shape}, \"more\": 1}}"), "{\"rsnip-shape\": {\"Circle\": {}}}"] {
            assert!(shape_from_text(not).is_none(), "{not:?} isn't an annotation");
        }
        let nowhere = Shape::Pen { points: Vec::new(), color: Color32::RED, width: 4.0 };
        assert!(shape_from_text(&shape_to_text(&nowhere)).is_none(), "nothing to draw");
        let far = Shape::Blur { rect: Rect::from_min_size(Pos2::new(1e9, 0.0), Vec2::splat(10.0)) };
        assert!(shape_from_text(&shape_to_text(&far)).is_none(), "nowhere near any image");
    }
}
