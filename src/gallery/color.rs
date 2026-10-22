//! The custom colour picker: a saturation/brightness square, a hue bar and a hex field.

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui::ecolor::Hsva;

use super::theme;

const SQUARE: Vec2 = Vec2::new(200.0, 140.0);
const HUE_BAR: Vec2 = Vec2::new(200.0, 14.0);

/// What the picker remembers between frames: the hue (lost in the colour itself when it's grey)
/// and the hex field as typed.
#[derive(Clone)]
struct State {
    hsva: Hsva,
    hex: String,
    /// The colour these were made for; if it changed elsewhere, start again from it.
    color: Color32,
}

pub fn picker(ui: &mut Ui, color: &mut Color32) {
    let id = ui.id().with("colour_picker");
    let mut state = ui
        .data(|d| d.get_temp::<State>(id))
        .filter(|s| s.color == *color)
        .unwrap_or_else(|| State { hsva: Hsva::from(*color), hex: hex(*color), color: *color });
    let before = state.hsva;

    sv_square(ui, &mut state.hsva);
    ui.add_space(6.0);
    hue_bar(ui, &mut state.hsva);
    ui.add_space(8.0);
    let mut changed = state.hsva != before;
    if changed {
        state.hex = hex(Color32::from(state.hsva));
    }
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(Vec2::new(28.0, 24.0), Sense::hover());
        ui.painter().rect_filled(swatch, 4.0, Color32::from(state.hsva));
        let label = ui.label(RichText::new("Hex").color(theme::MUTED));
        let field = egui::TextEdit::singleline(&mut state.hex).desired_width(90.0).char_limit(7);
        if ui.add(field).labelled_by(label.id).changed() {
            if let Some(parsed) = parse_hex(&state.hex) {
                state.hsva = Hsva::from(parsed);
                changed = true;
            }
        }
    });

    // Only a change writes the colour, so just opening the picker never alters an annotation.
    if changed {
        *color = Color32::from(state.hsva);
    }
    state.color = *color;
    ui.data_mut(|d| d.insert_temp(id, state));
}

fn sv_square(ui: &mut Ui, hsva: &mut Hsva) {
    let (rect, response) = ui.allocate_exact_size(SQUARE, Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        hsva.s = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        hsva.v = 1.0 - ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    }
    // A grid of vertices, each coloured exactly; the GPU blends between them.
    let n = 16;
    let mut mesh = Mesh::default();
    for j in 0..=n {
        for i in 0..=n {
            let (s, v) = (i as f32 / n as f32, 1.0 - j as f32 / n as f32);
            let pos = rect.min + Vec2::new(rect.width() * i as f32 / n as f32, rect.height() * j as f32 / n as f32);
            mesh.colored_vertex(pos, Color32::from(Hsva::new(hsva.h, s, v, 1.0)));
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            mesh.add_triangle(a, a + 1, a + n + 2);
            mesh.add_triangle(a, a + n + 2, a + n + 1);
        }
    }
    ui.painter().add(mesh);
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0f32, theme::BORDER), egui::StrokeKind::Outside);
    let marker = Pos2::new(rect.left() + hsva.s * rect.width(), rect.top() + (1.0 - hsva.v) * rect.height());
    ui.painter().circle_stroke(marker, 6.0, Stroke::new(2.0f32, Color32::WHITE));
    ui.painter().circle_stroke(marker, 7.5, Stroke::new(1.0f32, Color32::from_black_alpha(160)));
}

fn hue_bar(ui: &mut Ui, hsva: &mut Hsva) {
    let (rect, response) = ui.allocate_exact_size(HUE_BAR, Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        hsva.h = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    }
    let n = 36;
    let mut mesh = Mesh::default();
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let x = rect.left() + rect.width() * t;
        let c = Color32::from(Hsva::new(t, 1.0, 1.0, 1.0));
        mesh.colored_vertex(Pos2::new(x, rect.top()), c);
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), c);
    }
    for i in 0..n as u32 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 3);
        mesh.add_triangle(a, a + 3, a + 2);
    }
    ui.painter().add(mesh);
    let x = rect.left() + hsva.h * rect.width();
    let knob = Rect::from_center_size(Pos2::new(x, rect.center().y), Vec2::new(6.0, rect.height() + 6.0));
    ui.painter().rect(knob, 3.0, Color32::WHITE, Stroke::new(1.0f32, Color32::from_black_alpha(160)), egui::StrokeKind::Outside);
}

/// `#RRGGBB`.
pub fn hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// `#RRGGBB` or `RRGGBB` (any case), else `None`.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let digits = s.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_junk() {
        let c = Color32::from_rgb(0x12, 0xAB, 0x34);
        assert_eq!(hex(c), "#12AB34");
        assert_eq!(parse_hex("#12ab34"), Some(c));
        assert_eq!(parse_hex(" 12AB34 "), Some(c));
        for junk in ["", "#", "#12AB3", "#12AB345", "#12AG34", "#ééé", "rgb(1,2,3)"] {
            assert_eq!(parse_hex(junk), None, "{junk}");
        }
    }

    #[test]
    fn hue_survives_greys() {
        // Pure colours convert to HSV and back exactly enough to be the same Color32.
        for c in [Color32::from_rgb(230, 40, 40), Color32::from_rgb(30, 120, 240), Color32::WHITE, Color32::BLACK] {
            assert_eq!(Color32::from(Hsva::from(c)), c);
        }
    }
}
