//! Live preview of the shape being dragged out.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

use super::crop::crop_shade;
use super::{Drag, Tool, ToolState};
use crate::gallery::doc::arrow_geometry;

pub(super) fn preview(
    painter: &egui::Painter,
    drag: &Drag,
    tools: &ToolState,
    scale: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
    bounds: Rect,
    canvas: Rect,
) {
    let color = tools.color();
    let stroke = Stroke::new((tools.width * scale).max(1.0), color);
    let r = Rect::from_two_pos(to_screen(drag.start), to_screen(drag.current));
    match tools.tool {
        Tool::Box => {
            painter.rect_stroke(r, 0.0, stroke);
        }
        Tool::Ellipse => {
            let points = (0..64)
                .map(|i| {
                    let t = i as f32 / 64.0 * std::f32::consts::TAU;
                    r.center() + Vec2::new(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
        Tool::Highlight => {
            painter.rect_filled(r, 0.0, color.gamma_multiply(0.45));
        }
        Tool::Blur => {
            painter.rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.6));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE));
            painter.text(r.center(), Align2::CENTER_CENTER, "Blur", FontId::proportional(14.0), Color32::WHITE);
        }
        Tool::Arrow => {
            let (shaft_end, head) = arrow_geometry(drag.start, drag.current, tools.width);
            painter.line_segment([to_screen(drag.start), to_screen(shaft_end)], stroke);
            painter.add(egui::Shape::convex_polygon(head.iter().map(|p| to_screen(*p)).collect(), color, Stroke::NONE));
        }
        Tool::Pen => {
            let points = drag.points.iter().map(|p| to_screen(*p)).collect();
            painter.add(egui::Shape::line(points, stroke));
        }
        Tool::Crop => {
            let size = Rect::from_two_pos(drag.start, drag.current).intersect(bounds).size();
            crop_shade(painter, r, canvas, size, false);
        }
        Tool::Text | Tool::Step => {}
    }
}
