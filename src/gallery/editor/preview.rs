//! Drawing annotations live with egui: the shape being dragged out, and shapes being moved or
//! resized (which are left out of the rendered canvas meanwhile).

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke};

use super::crop::{crop_shade, cut_shade, cut_strip, shade_outside, strip_pixels};
use super::text::{preview_font, shadow_offset};
use super::{Drag, Tool, ToolState};
use crate::gallery::doc::{arrow_geometry, ellipse_points, zoom_connectors, zoom_inset, Shape, SPOTLIGHT_DIM};

/// The shape a finished drag with the current tool draws, if it's big enough to keep. `bounds` is
/// the image, which a zoom's magnified copy is placed inside.
pub(super) fn shape_from_drag(drag: &Drag, tools: &ToolState, bounds: Rect) -> Option<Shape> {
    let rect = Rect::from_two_pos(drag.start, drag.current);
    let big_enough = rect.width() >= 2.0 && rect.height() >= 2.0;
    let color = tools.color();
    let width = tools.width;
    Some(match tools.tool {
        Tool::Box if big_enough => Shape::Box { rect, color, width },
        Tool::Ellipse if big_enough => Shape::Ellipse { rect, color, width },
        Tool::Highlight if big_enough => Shape::Highlight { rect, color },
        Tool::Blur if big_enough => Shape::Blur { rect },
        Tool::Pixelate if big_enough => Shape::Pixelate { rect },
        Tool::Redact if big_enough => Shape::Redact { rect, color },
        Tool::Spotlight if big_enough => Shape::Spotlight { rect },
        Tool::Zoom if big_enough => Shape::Zoom { rect, inset: zoom_inset(rect, bounds), color, width },
        Tool::Line if drag.start.distance(drag.current) >= 2.0 => Shape::Line { from: drag.start, to: drag.current, color, width },
        Tool::Arrow if drag.start.distance(drag.current) >= 4.0 => Shape::Arrow { from: drag.start, to: drag.current, color, width },
        Tool::Pen => Shape::Pen { points: drag.points.clone(), color, width },
        _ => return None,
    })
}

/// Shows the shape being dragged out. `texture` is the rendered image shown on `canvas`.
#[allow(clippy::too_many_arguments)]
pub(super) fn preview(
    painter: &egui::Painter,
    drag: &Drag,
    tools: &ToolState,
    scale: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
    bounds: Rect,
    canvas: Rect,
    texture: Option<egui::TextureId>,
) {
    if tools.tool == Tool::Crop {
        let r = Rect::from_two_pos(to_screen(drag.start), to_screen(drag.current));
        let size = Rect::from_two_pos(drag.start, drag.current).intersect(bounds).size();
        crop_shade(painter, r, canvas, size, false);
    } else if tools.tool == Tool::CutOut {
        let (axis, strip) = cut_strip(drag.start, drag.current, bounds.size());
        let r = Rect::from_two_pos(to_screen(strip.min), to_screen(strip.max));
        cut_shade(painter, r, canvas, axis, strip_pixels(axis, strip), false);
    } else if let Some(shape) = shape_from_drag(drag, tools, bounds) {
        paint_shape(painter, &shape, scale, to_screen, canvas, false, texture);
    }
}

/// Approximates how the renderer draws `shape`, in screen points. A zoom magnifies `texture`,
/// the rendered image shown on `canvas`.
pub(super) fn paint_shape(
    painter: &egui::Painter,
    shape: &Shape,
    scale: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
    canvas: Rect,
    matching_font: bool,
    texture: Option<egui::TextureId>,
) {
    let screen_rect = |r: &Rect| Rect::from_two_pos(to_screen(r.min), to_screen(r.max));
    let stroke = |width: f32, color: Color32| Stroke::new((width * scale).max(1.0), color);
    match shape {
        Shape::Box { rect, color, width } => {
            painter.rect_stroke(screen_rect(rect), 0.0, stroke(*width, *color), egui::StrokeKind::Middle);
        }
        Shape::Ellipse { rect, color, width } => {
            let points = ellipse_points(*rect).into_iter().map(&to_screen).collect();
            painter.add(egui::Shape::closed_line(points, stroke(*width, *color)));
        }
        Shape::Highlight { rect, color } => {
            painter.rect_filled(screen_rect(rect), 0.0, color.gamma_multiply(0.45));
        }
        Shape::Blur { rect } | Shape::Pixelate { rect } => {
            let r = screen_rect(rect);
            painter.rect_filled(r, 0.0, Color32::from_gray(128).gamma_multiply(0.6));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::WHITE), egui::StrokeKind::Middle);
            let label = if matches!(shape, Shape::Blur { .. }) { "Blur" } else { "Pixelate" };
            painter.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(14.0), Color32::WHITE);
        }
        Shape::Redact { rect, color } => {
            painter.rect_filled(screen_rect(rect), 0.0, *color);
        }
        Shape::Spotlight { rect } => {
            let r = screen_rect(rect);
            shade_outside(painter, r, canvas, Color32::from_black_alpha(SPOTLIGHT_DIM));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0f32, Color32::from_white_alpha(160)), egui::StrokeKind::Middle);
        }
        Shape::Line { from, to, color, width } => {
            painter.line_segment([to_screen(*from), to_screen(*to)], stroke(*width, *color));
        }
        Shape::Arrow { from, to, color, width } => {
            let (shaft_end, head) = arrow_geometry(*from, *to, *width);
            painter.line_segment([to_screen(*from), to_screen(shaft_end)], stroke(*width, *color));
            painter.add(egui::Shape::convex_polygon(head.iter().map(|p| to_screen(*p)).collect(), *color, Stroke::NONE));
        }
        Shape::Pen { points, color, width } => {
            let points = points.iter().map(|p| to_screen(*p)).collect();
            painter.add(egui::Shape::line(points, stroke(*width, *color)));
        }
        Shape::Text { pos, text, color, size, shadow } => {
            let font_id = preview_font(size * scale, matching_font);
            let at = to_screen(*pos);
            if *shadow {
                let galley = painter.layout_no_wrap(text.clone(), font_id.clone(), Color32::from_black_alpha(170));
                painter.galley(at + shadow_offset(*size, scale), galley, Color32::BLACK);
            }
            painter.galley(at, painter.layout_no_wrap(text.clone(), font_id, *color), *color);
        }
        Shape::Step { center, number, color, diameter } => {
            let (c, r) = (to_screen(*center), diameter * scale / 2.0);
            painter.circle(c, r, *color, Stroke::new((diameter / 16.0).max(1.5) * scale, Color32::WHITE));
            painter.text(c, Align2::CENTER_CENTER, number.to_string(), preview_font(diameter * 0.55 * scale, matching_font), Color32::WHITE);
        }
        Shape::Zoom { rect, inset, color, width } => {
            let (source, copy) = (screen_rect(rect), screen_rect(inset));
            if let Some(texture) = texture {
                let uv = |p: Pos2| ((p - canvas.min) / canvas.size()).to_pos2().clamp(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(texture, copy, Rect::from_min_max(uv(source.min), uv(source.max)), Color32::WHITE);
            }
            for r in [source, copy] {
                painter.rect_stroke(r, 0.0, stroke(*width, *color), egui::StrokeKind::Middle);
            }
            for (a, b) in zoom_connectors(*rect, *inset).into_iter().flatten() {
                painter.line_segment([to_screen(a), to_screen(b)], stroke((width / 2.0).max(1.0), *color));
            }
        }
    }
}

/// `p` moved onto the nearest 45° line through `start`, keeping its distance.
pub(super) fn snap_45(start: Pos2, p: Pos2) -> Pos2 {
    let d = p - start;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.angle() / step).round() * step;
    start + egui::Vec2::angled(angle) * d.length()
}
