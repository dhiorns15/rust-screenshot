//! An open screenshot: its annotations, undo history, and rasterising everything to pixels.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use eframe::egui::{Color32, Pos2, Rect, Vec2};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

const MAX_HISTORY: usize = 200;

/// Annotations, in image pixel coordinates.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Box { rect: Rect, color: Color32, width: f32 },
    Ellipse { rect: Rect, color: Color32, width: f32 },
    Highlight { rect: Rect, color: Color32 },
    Blur { rect: Rect },
    Arrow { from: Pos2, to: Pos2, color: Color32, width: f32 },
    Pen { points: Vec<Pos2>, color: Color32, width: f32 },
    Text { pos: Pos2, text: String, color: Color32, size: f32, shadow: bool },
    Step { center: Pos2, number: u32, color: Color32, diameter: f32 },
}

impl Shape {
    /// The area the shape covers (text without its click padding).
    pub fn bounds(&self, font: Option<&FontVec>) -> Rect {
        match self {
            Shape::Box { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Highlight { rect, .. } | Shape::Blur { rect } => *rect,
            Shape::Arrow { from, to, .. } => Rect::from_two_pos(*from, *to),
            Shape::Pen { points, .. } => Rect::from_points(points),
            Shape::Text { pos, text, size, .. } => text_bounds(*pos, text, *size, font).shrink(4.0),
            Shape::Step { center, diameter, .. } => Rect::from_center_size(*center, Vec2::splat(*diameter)),
        }
    }

    /// Whether `p` is on the shape, with `tol` pixels of slack. Outlines only count their line,
    /// so things drawn inside a box stay clickable.
    pub fn hit(&self, p: Pos2, tol: f32, font: Option<&FontVec>) -> bool {
        match self {
            Shape::Box { rect, width, .. } => {
                let reach = width / 2.0 + tol;
                let inner = rect.shrink(reach);
                rect.expand(reach).contains(p) && !(inner.is_positive() && inner.contains(p))
            }
            Shape::Ellipse { rect, width, .. } => polyline_distance(&ellipse_points(*rect), true, p) <= width / 2.0 + tol,
            // The head is wider than the shaft.
            Shape::Arrow { from, to, width, .. } => segment_distance(p, *from, *to) <= width * 1.5 + tol,
            Shape::Pen { points, width, .. } => polyline_distance(points, false, p) <= width / 2.0 + tol,
            _ => self.bounds(font).expand(tol).contains(p),
        }
    }

    pub fn translated(&self, d: Vec2) -> Shape {
        let mut s = self.clone();
        match &mut s {
            Shape::Box { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Highlight { rect, .. } | Shape::Blur { rect } => {
                *rect = rect.translate(d)
            }
            Shape::Arrow { from, to, .. } => {
                *from += d;
                *to += d;
            }
            Shape::Pen { points, .. } => points.iter_mut().for_each(|p| *p += d),
            Shape::Text { pos, .. } => *pos += d,
            Shape::Step { center, .. } => *center += d,
        }
        s
    }

    /// The shape stretched so its bounds `from` land on `to`. Text and steps scale with the height
    /// and width respectively, so callers keep their aspect ratio.
    pub fn resized(&self, from: Rect, to: Rect) -> Shape {
        let map = |p: Pos2| to.min + (p - from.min) / from.size().max(Vec2::splat(0.001)) * to.size();
        let mut s = self.clone();
        match &mut s {
            Shape::Box { rect, .. } | Shape::Ellipse { rect, .. } | Shape::Highlight { rect, .. } | Shape::Blur { rect } => *rect = to,
            Shape::Arrow { from: a, to: b, .. } => {
                *a = map(*a);
                *b = map(*b);
            }
            Shape::Pen { points, .. } => points.iter_mut().for_each(|p| *p = map(*p)),
            Shape::Text { pos, size, .. } => {
                *size *= to.height() / from.height().max(1.0);
                *pos = to.min;
            }
            Shape::Step { center, diameter, .. } => {
                *diameter = to.width().max(1.0);
                *center = to.center();
            }
        }
        s
    }
}

/// Points around the ellipse inscribed in `rect`.
pub fn ellipse_points(rect: Rect) -> Vec<Pos2> {
    (0..64)
        .map(|i| {
            let t = i as f32 / 64.0 * std::f32::consts::TAU;
            rect.center() + Vec2::new(t.cos() * rect.width() / 2.0, t.sin() * rect.height() / 2.0)
        })
        .collect()
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

fn polyline_distance(points: &[Pos2], closed: bool, p: Pos2) -> f32 {
    let Some(&first) = points.first() else { return f32::INFINITY };
    let closing = closed.then(|| (*points.last().unwrap(), first));
    let segments = points.windows(2).map(|w| (w[0], w[1])).chain(closing);
    segments.map(|(a, b)| segment_distance(p, a, b)).fold(p.distance(first), f32::min)
}

#[derive(Clone)]
pub struct Snapshot {
    pub base: Arc<RgbaImage>,
    pub shapes: Vec<Shape>,
}

pub struct Doc {
    pub path: PathBuf,
    history: Vec<Snapshot>,
    index: usize,
    /// History index matching the file on disk, if still reachable.
    saved: Option<usize>,
    /// Bumped whenever the current snapshot changes, so views know to re-render.
    pub version: u64,
    /// The shape and property the current step's `amend` changed, so further changes merge into it.
    amending: Option<(usize, &'static str)>,
}

impl Doc {
    #[cfg(test)]
    pub fn from_image(img: RgbaImage) -> Doc {
        let snapshot = Snapshot { base: Arc::new(img), shapes: Vec::new() };
        Doc { path: PathBuf::new(), history: vec![snapshot], index: 0, saved: Some(0), version: 0, amending: None }
    }

    /// Opens a screenshot, restoring its editable annotations if RSnip saved them.
    pub fn open(path: &Path) -> Result<Doc, String> {
        let snapshot = match load_layers(path) {
            Some(snapshot) => snapshot,
            None => {
                let img = image::open(path).map_err(|e| format!("Can't open {}: {e}", path.display()))?;
                Snapshot { base: Arc::new(img.to_rgba8()), shapes: Vec::new() }
            }
        };
        Ok(Doc { path: path.to_path_buf(), history: vec![snapshot], index: 0, saved: Some(0), version: 0, amending: None })
    }

    /// Records the un-annotated image and the annotations next to the just-saved PNG at
    /// `self.path`, so reopening it keeps everything editable.
    pub fn save_layers(&self) -> Result<(), String> {
        let snapshot = self.current();
        if snapshot.shapes.is_empty() {
            // The PNG alone is the whole picture.
            remove_layers(&self.path);
            return Ok(());
        }
        let (json, base) = layer_paths(&self.path).ok_or("Bad file name")?;
        let dir = json.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| format!("Can't create {}: {e}", dir.display()))?;
        hide(dir);
        snapshot.base.save(&base).map_err(|e| format!("Can't save layers: {e}"))?;
        let stamp = FileStamp::of(&self.path).ok_or("Saved file is missing")?;
        let layers = Layers { version: 1, png: stamp, shapes: snapshot.shapes.clone() };
        let bytes = serde_json::to_vec(&layers).map_err(|e| e.to_string())?;
        std::fs::write(&json, bytes).map_err(|e| format!("Can't save layers: {e}"))
    }

    pub fn current(&self) -> &Snapshot {
        &self.history[self.index]
    }

    pub fn size(&self) -> Vec2 {
        let base = &self.current().base;
        Vec2::new(base.width() as f32, base.height() as f32)
    }

    fn push(&mut self, snapshot: Snapshot) {
        self.amending = None;
        self.history.truncate(self.index + 1);
        if self.saved.is_some_and(|s| s > self.index) {
            self.saved = None;
        }
        self.history.push(snapshot);
        self.index += 1;
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
            self.index -= 1;
            self.saved = self.saved.and_then(|s| s.checked_sub(1));
        }
        self.version += 1;
    }

    pub fn add(&mut self, shape: Shape) {
        let mut next = self.current().clone();
        next.shapes.push(shape);
        self.push(next);
    }

    /// Replaces the shape at `index`, or removes it when `shape` is `None`.
    pub fn replace(&mut self, index: usize, shape: Option<Shape>) {
        let mut next = self.current().clone();
        match shape {
            Some(shape) => next.shapes[index] = shape,
            None => {
                next.shapes.remove(index);
            }
        }
        self.push(next);
    }

    /// Replaces the shape at `index` like `replace`, except that repeated changes to the same
    /// shape's same property (e.g. while dragging a slider) share one undo step.
    pub fn amend(&mut self, index: usize, shape: Shape, property: &'static str) {
        if self.amending == Some((index, property)) && self.saved != Some(self.index) {
            self.history[self.index].shapes[index] = shape;
            self.version += 1;
        } else {
            self.replace(index, Some(shape));
            self.amending = Some((index, property));
        }
    }

    /// The top-most annotation under `p`, with `tol` pixels of slack.
    pub fn shape_at(&self, p: Pos2, tol: f32, font: Option<&FontVec>) -> Option<usize> {
        self.current().shapes.iter().rposition(|s| s.hit(p, tol, font))
    }

    /// The top-most text annotation under `p`.
    pub fn text_at(&self, p: Pos2, font: Option<&FontVec>) -> Option<usize> {
        let shapes = self.current().shapes.iter().enumerate().rev();
        shapes
            .filter_map(|(i, shape)| match shape {
                Shape::Text { pos, text, size, .. } => Some((i, text_bounds(*pos, text, *size, font))),
                _ => None,
            })
            .find(|(_, bounds)| bounds.contains(p))
            .map(|(i, _)| i)
    }

    /// Flattens the annotations and crops; undoable like any other edit.
    pub fn crop(&mut self, rect: Rect, font: Option<&FontVec>) {
        let rendered = render(self.current(), font);
        let (x, y) = (rect.min.x.max(0.0) as u32, rect.min.y.max(0.0) as u32);
        let w = (rect.width() as u32).min(rendered.width().saturating_sub(x));
        let h = (rect.height() as u32).min(rendered.height().saturating_sub(y));
        if w == 0 || h == 0 {
            return;
        }
        let cropped = image::imageops::crop_imm(&rendered, x, y, w, h).to_image();
        self.push(Snapshot { base: Arc::new(cropped), shapes: Vec::new() });
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    pub fn undo(&mut self) {
        if self.can_undo() {
            self.amending = None;
            self.index -= 1;
            self.version += 1;
        }
    }

    pub fn redo(&mut self) {
        if self.can_redo() {
            self.amending = None;
            self.index += 1;
            self.version += 1;
        }
    }

    pub fn dirty(&self) -> bool {
        self.saved != Some(self.index)
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = path;
        self.saved = Some(self.index);
    }

    pub fn next_step_number(&self) -> u32 {
        let max = self.current().shapes.iter().filter_map(|s| match s {
            Shape::Step { number, .. } => Some(*number),
            _ => None,
        });
        max.max().unwrap_or(0) + 1
    }
}

/// Loads a font for text and step numbers, matching what the editor shows.
#[cfg(test)]
pub fn load_font() -> Option<FontVec> {
    FontVec::try_from_vec(load_font_bytes()?).ok()
}

/// Editable annotations stored beside a flattened PNG, in `<folder>/.rsnip/<name>.json`
/// with the un-annotated image as `<name>.base.png`.
#[derive(Serialize, Deserialize)]
struct Layers {
    version: u32,
    /// The PNG these layers belong to; if it changes outside RSnip, the layers are stale.
    png: FileStamp,
    shapes: Vec<Shape>,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct FileStamp {
    len: u64,
    modified_ns: u128,
}

impl FileStamp {
    fn of(path: &Path) -> Option<FileStamp> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(FileStamp { len: meta.len(), modified_ns: modified.as_nanos() })
    }
}

fn layer_paths(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = path.parent()?.join(".rsnip");
    let name = path.file_name()?.to_string_lossy();
    Some((dir.join(format!("{name}.json")), dir.join(format!("{name}.base.png"))))
}

fn load_layers(path: &Path) -> Option<Snapshot> {
    let (json, base) = layer_paths(path)?;
    let layers: Layers = serde_json::from_slice(&std::fs::read(json).ok()?).ok()?;
    if layers.version != 1 || FileStamp::of(path)? != layers.png {
        return None;
    }
    let base = image::open(base).ok()?.to_rgba8();
    Some(Snapshot { base: Arc::new(base), shapes: layers.shapes })
}

/// Deletes the saved layers for `path`, e.g. when the screenshot is deleted.
pub fn remove_layers(path: &Path) {
    if let Some((json, base)) = layer_paths(path) {
        let _ = std::fs::remove_file(json);
        let _ = std::fs::remove_file(base);
    }
}

/// Deletes saved layers in `dir` whose screenshot is gone, e.g. deleted in Explorer.
pub fn prune_layers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir.join(".rsnip")) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let shot = name.strip_suffix(".base.png").or_else(|| name.strip_suffix(".json"));
        if shot.is_some_and(|shot| !dir.join(shot).exists()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Keeps the `.rsnip` folder out of the way in Explorer.
fn hide(dir: &Path) {
    use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES};
    let wide = crate::util::wide_os(dir.as_os_str());
    unsafe {
        let attrs = GetFileAttributesW(wide.as_ptr());
        if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_HIDDEN == 0 {
            SetFileAttributesW(wide.as_ptr(), attrs | FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

/// Raw font file, also handed to egui so the live text preview uses the same glyphs.
pub fn load_font_bytes() -> Option<Vec<u8>> {
    let fonts = std::env::var_os("WINDIR").map(|w| Path::new(&w).join("Fonts"))?;
    ["seguisb.ttf", "segoeui.ttf", "arial.ttf"].iter().find_map(|name| std::fs::read(fonts.join(name)).ok())
}

/// Arrow geometry shared by the preview and the renderer: end of the shaft and the head triangle.
pub fn arrow_geometry(from: Pos2, to: Pos2, width: f32) -> (Pos2, [Pos2; 3]) {
    let delta = to - from;
    let len = delta.length().max(0.001);
    let dir = delta / len;
    let head = (width * 4.0 + 8.0).min(len);
    let base = to - dir * head;
    let perp = Vec2::new(-dir.y, dir.x) * head * 0.55;
    (base + dir * 1.0, [to, base + perp, base - perp])
}

pub fn text_size(font: &FontVec, text: &str, size: f32) -> Vec2 {
    let scaled = font.as_scaled(PxScale::from(size));
    let width = text
        .lines()
        .map(|line| line.chars().map(|c| scaled.h_advance(font.glyph_id(c))).sum::<f32>())
        .fold(0.0, f32::max);
    let lines = text.lines().count().max(1) as f32;
    Vec2::new(width, lines * scaled.height() + (lines - 1.0) * scaled.line_gap())
}

/// Clickable area of a text annotation, padded a little so it's easy to hit.
pub fn text_bounds(pos: Pos2, text: &str, size: f32, font: Option<&FontVec>) -> Rect {
    let extent = match font {
        Some(font) => text_size(font, text, size),
        None => Vec2::new(text.chars().count() as f32 * size * 0.55, size * 1.3),
    };
    Rect::from_min_size(pos, extent).expand(4.0)
}

fn paint(color: Color32) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
    paint.anti_alias = true;
    paint
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

fn sk_rect(r: Rect) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y)
}

/// Draws the snapshot's annotations onto a copy of its base image.
pub fn render(snapshot: &Snapshot, font: Option<&FontVec>) -> RgbaImage {
    let base = &snapshot.base;
    let (w, h) = base.dimensions();
    let Some(mut pixmap) = Pixmap::new(w, h) else { return (**base).clone() };
    for (dst, src) in pixmap.data_mut().chunks_exact_mut(4).zip(base.as_raw().chunks_exact(4)) {
        let a = src[3] as u16;
        dst.copy_from_slice(&[
            (src[0] as u16 * a / 255) as u8,
            (src[1] as u16 * a / 255) as u8,
            (src[2] as u16 * a / 255) as u8,
            src[3],
        ]);
    }

    let id = Transform::identity();
    for shape in &snapshot.shapes {
        match shape {
            Shape::Box { rect, color, width } => {
                if let Some(r) = sk_rect(*rect) {
                    let path = PathBuilder::from_rect(r);
                    pixmap.stroke_path(&path, &paint(*color), &stroke(*width), id, None);
                }
            }
            Shape::Ellipse { rect, color, width } => {
                if let Some(path) = sk_rect(*rect).and_then(PathBuilder::from_oval) {
                    pixmap.stroke_path(&path, &paint(*color), &stroke(*width), id, None);
                }
            }
            Shape::Highlight { rect, color } => {
                if let Some(r) = sk_rect(*rect) {
                    // Multiply behaves like a real highlighter: dark text stays dark.
                    let mut p = paint(*color);
                    p.blend_mode = BlendMode::Multiply;
                    pixmap.fill_rect(r, &p, id, None);
                }
            }
            Shape::Blur { rect } => blur(&mut pixmap, *rect),
            Shape::Arrow { from, to, color, width } => {
                let (shaft_end, head) = arrow_geometry(*from, *to, *width);
                let mut pb = PathBuilder::new();
                pb.move_to(from.x, from.y);
                pb.line_to(shaft_end.x, shaft_end.y);
                if let Some(path) = pb.finish() {
                    pixmap.stroke_path(&path, &paint(*color), &stroke(*width), id, None);
                }
                let mut pb = PathBuilder::new();
                pb.move_to(head[0].x, head[0].y);
                pb.line_to(head[1].x, head[1].y);
                pb.line_to(head[2].x, head[2].y);
                pb.close();
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint(*color), FillRule::Winding, id, None);
                }
            }
            Shape::Pen { points, color, width } => {
                let mut pb = PathBuilder::new();
                if let Some(first) = points.first() {
                    pb.move_to(first.x, first.y);
                    // A single click still leaves a dot.
                    pb.line_to(first.x + 0.01, first.y);
                }
                for p in points.iter().skip(1) {
                    pb.line_to(p.x, p.y);
                }
                if let Some(path) = pb.finish() {
                    pixmap.stroke_path(&path, &paint(*color), &stroke(*width), id, None);
                }
            }
            Shape::Text { pos, text, color, size, shadow } => {
                if let Some(font) = font {
                    if *shadow {
                        draw_text_shadow(&mut pixmap, font, *pos, text, *size);
                    }
                    draw_text(&mut pixmap, font, *pos, text, *color, *size);
                }
            }
            Shape::Step { center, number, color, diameter } => {
                if let Some(path) = PathBuilder::from_circle(center.x, center.y, diameter / 2.0) {
                    pixmap.fill_path(&path, &paint(*color), FillRule::Winding, id, None);
                    pixmap.stroke_path(&path, &paint(Color32::WHITE), &stroke((diameter / 16.0).max(1.5)), id, None);
                }
                if let Some(font) = font {
                    let label = number.to_string();
                    let size = diameter * 0.55;
                    let extent = text_size(font, &label, size);
                    draw_text(&mut pixmap, font, *center - extent / 2.0, &label, Color32::WHITE, size);
                }
            }
        }
    }

    let mut out = RgbaImage::new(w, h);
    for (dst, src) in out.as_mut().chunks_exact_mut(4).zip(pixmap.data().chunks_exact(4)) {
        let a = src[3] as u16;
        let un = |c: u8| (c as u16 * 255).checked_div(a).map_or(0, |v| v.min(255) as u8);
        dst.copy_from_slice(&[un(src[0]), un(src[1]), un(src[2]), src[3]]);
    }
    out
}

/// Calls `f(x, y, coverage)` for every pixel the laid-out text touches.
fn glyph_coverage(font: &FontVec, pos: Pos2, text: &str, size: f32, mut f: impl FnMut(i32, i32, f32)) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let mut baseline = pos.y + scaled.ascent();
    for line in text.lines() {
        let mut x = pos.x;
        let mut prev = None;
        for c in line.chars() {
            let glyph_id = font.glyph_id(c);
            if let Some(prev) = prev {
                x += scaled.kern(prev, glyph_id);
            }
            let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    f(bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32, coverage.clamp(0.0, 1.0))
                });
            }
            x += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }
        baseline += scaled.height() + scaled.line_gap();
    }
}

fn draw_text(pixmap: &mut Pixmap, font: &FontVec, pos: Pos2, text: &str, color: Color32, size: f32) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.data_mut();
    let src = [color.r(), color.g(), color.b()];
    glyph_coverage(font, pos, text, size, |px, py, coverage| {
        if px < 0 || py < 0 || px >= w || py >= h {
            return;
        }
        let i = ((py * w + px) * 4) as usize;
        let a = coverage * color.a() as f32 / 255.0;
        for c in 0..3 {
            data[i + c] = (src[c] as f32 * a + data[i + c] as f32 * (1.0 - a)).round() as u8;
        }
        data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)).round() as u8;
    });
}

/// Soft dark drop shadow, offset down-right, so text reads on busy or light backgrounds.
fn draw_text_shadow(pixmap: &mut Pixmap, font: &FontVec, pos: Pos2, text: &str, size: f32) {
    const OPACITY: f32 = 0.85;
    // Blurring thin strokes spreads them out; boosting afterwards keeps a solid core with soft edges.
    const BOOST: f32 = 2.0;
    let offset = (size * 0.06).max(1.5);
    let radius = ((size * 0.05).round() as isize).max(1);
    let pad = radius as i32 * 3 + 2;
    let extent = text_size(font, text, size);
    let origin_x = (pos.x + offset).floor() as i32 - pad;
    let origin_y = (pos.y + offset).floor() as i32 - pad;
    let (rw, rh) = (extent.x.ceil() as i32 + 2 * pad + 2, extent.y.ceil() as i32 + 2 * pad + 2);

    // Coverage goes in the alpha channel so the existing 4-channel box blur can soften it.
    let mut mask = vec![0u8; (rw * rh * 4) as usize];
    glyph_coverage(font, pos + Vec2::splat(offset), text, size, |x, y, coverage| {
        let (mx, my) = (x - origin_x, y - origin_y);
        if mx >= 0 && my >= 0 && mx < rw && my < rh {
            let i = ((my * rw + mx) * 4 + 3) as usize;
            mask[i] = mask[i].max((coverage * 255.0) as u8);
        }
    });
    for _ in 0..2 {
        box_blur(&mut mask, rw as usize, rh as usize, 4, rw as usize * 4, radius);
        box_blur(&mut mask, rh as usize, rw as usize, rw as usize * 4, 4, radius);
    }

    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.data_mut();
    for my in 0..rh {
        let y = origin_y + my;
        if y < 0 || y >= h {
            continue;
        }
        for mx in 0..rw {
            let x = origin_x + mx;
            let a = (mask[((my * rw + mx) * 4 + 3) as usize] as f32 / 255.0 * BOOST).min(1.0) * OPACITY;
            if x < 0 || x >= w || a <= 0.0 {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                data[i + c] = (data[i + c] as f32 * (1.0 - a)).round() as u8;
            }
            data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)).round() as u8;
        }
    }
}

/// Strong blur (three box-blur passes ≈ Gaussian), enough to make text unreadable.
fn blur(pixmap: &mut Pixmap, rect: Rect) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let x0 = (rect.min.x.floor() as i32).clamp(0, w);
    let y0 = (rect.min.y.floor() as i32).clamp(0, h);
    let x1 = (rect.max.x.ceil() as i32).clamp(0, w);
    let y1 = (rect.max.y.ceil() as i32).clamp(0, h);
    let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    if rw == 0 || rh == 0 {
        return;
    }
    let stride = w as usize * 4;
    let data = pixmap.data_mut();
    let mut region = vec![0u8; rw * rh * 4];
    for y in 0..rh {
        let src = (y0 as usize + y) * stride + x0 as usize * 4;
        region[y * rw * 4..(y + 1) * rw * 4].copy_from_slice(&data[src..src + rw * 4]);
    }
    let radius = ((rw.min(rh) / 6).clamp(4, 14)) as isize;
    for _ in 0..3 {
        box_blur(&mut region, rw, rh, 4, rw * 4, radius);
        box_blur(&mut region, rh, rw, rw * 4, 4, radius);
    }
    for y in 0..rh {
        let dst = (y0 as usize + y) * stride + x0 as usize * 4;
        data[dst..dst + rw * 4].copy_from_slice(&region[y * rw * 4..(y + 1) * rw * 4]);
    }
}

/// One box-blur pass along lines of `len` pixels; `step` is the byte distance between
/// neighbouring pixels in a line, `line_step` the distance between lines.
fn box_blur(px: &mut [u8], len: usize, lines: usize, step: usize, line_step: usize, r: isize) {
    let mut out = vec![0u8; len * 4];
    let last = len as isize - 1;
    for line in 0..lines {
        let start = line * line_step;
        for c in 0..4 {
            let at = |i: isize| px[start + i.clamp(0, last) as usize * step + c] as u32;
            let mut sum: u32 = (-r..=r).map(at).sum();
            for i in 0..len as isize {
                out[i as usize * 4 + c] = (sum / (2 * r as u32 + 1)) as u8;
                sum += at(i + r + 1);
                sum -= at(i - r);
            }
        }
        for i in 0..len {
            px[start + i * step..start + i * step + 4].copy_from_slice(&out[i * 4..i * 4 + 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_undoes() {
        let base = RgbaImage::from_pixel(40, 30, image::Rgba([255, 255, 255, 255]));
        let mut doc = Doc::from_image(base);
        doc.add(Shape::Box { rect: Rect::from_min_max(Pos2::new(5.0, 5.0), Pos2::new(20.0, 20.0)), color: Color32::RED, width: 2.0 });
        doc.add(Shape::Blur { rect: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(40.0, 30.0)) });
        assert!(doc.dirty());
        let out = render(doc.current(), None);
        assert_eq!(out.dimensions(), (40, 30));
        // The box edge was red before blurring; afterwards it is a blend, not pure white.
        assert_ne!(out.get_pixel(5, 12).0, [255, 255, 255, 255]);
        doc.crop(Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(30.0, 25.0)), None);
        assert_eq!(doc.size(), Vec2::new(20.0, 15.0));
        doc.undo();
        doc.undo();
        doc.undo();
        assert!(!doc.dirty());
        assert_eq!(doc.next_step_number(), 1);
    }

    #[test]
    fn text_shadow_darkens_background() {
        let Some(font) = load_font() else { return };
        let white = RgbaImage::from_pixel(160, 60, image::Rgba([255, 255, 255, 255]));
        let snapshot = |shadow| Snapshot {
            base: Arc::new(white.clone()),
            shapes: vec![Shape::Text { pos: Pos2::new(8.0, 8.0), text: "Hi there".into(), color: Color32::WHITE, size: 32.0, shadow }],
        };
        let darkest = |img: &RgbaImage| img.pixels().map(|p| p.0[0]).min().unwrap();
        assert_eq!(darkest(&render(&snapshot(false), Some(&font))), 255);
        let d = darkest(&render(&snapshot(true), Some(&font)));
        assert!(d < 128, "darkest shadow pixel {d}");
    }

    #[test]
    fn annotations_stay_editable_after_save_and_reopen() {
        let dir = std::env::temp_dir().join(format!("rsnip-layers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let base = RgbaImage::from_pixel(60, 40, image::Rgba([200, 200, 200, 255]));
        base.save(&path).unwrap();

        let mut doc = Doc::open(&path).unwrap();
        doc.add(Shape::Text { pos: Pos2::new(5.0, 5.0), text: "keep me".into(), color: Color32::RED, size: 16.0, shadow: true });
        render(doc.current(), None).save(&path).unwrap();
        doc.mark_saved(path.clone());
        doc.save_layers().unwrap();

        let reopened = Doc::open(&path).unwrap();
        assert!(matches!(&reopened.current().shapes[..], [Shape::Text { text, .. }] if text == "keep me"));
        assert_eq!(reopened.current().base.get_pixel(0, 0).0, [200, 200, 200, 255]);

        // Changing the PNG elsewhere makes the layers stale, so it opens flat.
        RgbaImage::from_pixel(60, 40, image::Rgba([0, 0, 0, 255])).save(&path).unwrap();
        assert!(Doc::open(&path).unwrap().current().shapes.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edits_text_in_place() {
        let base = RgbaImage::from_pixel(200, 100, image::Rgba([255, 255, 255, 255]));
        let mut doc = Doc::from_image(base);
        let text = |s: &str| Shape::Text { pos: Pos2::new(10.0, 10.0), text: s.into(), color: Color32::RED, size: 20.0, shadow: true };
        doc.add(text("hello"));
        let hit = doc.text_at(Pos2::new(15.0, 20.0), None);
        assert_eq!(hit, Some(0));
        assert_eq!(doc.text_at(Pos2::new(190.0, 90.0), None), None);
        doc.replace(0, Some(text("changed")));
        assert!(matches!(&doc.current().shapes[0], Shape::Text { text, .. } if text == "changed"));
        doc.replace(0, None);
        assert!(doc.current().shapes.is_empty());
        doc.undo();
        assert_eq!(doc.current().shapes.len(), 1);
    }

    #[test]
    fn hit_testing_follows_the_drawn_lines() {
        let rect = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(110.0, 60.0));
        let boxed = Shape::Box { rect, color: Color32::RED, width: 4.0 };
        assert!(boxed.hit(Pos2::new(60.0, 11.0), 3.0, None), "on the outline");
        assert!(!boxed.hit(Pos2::new(60.0, 35.0), 3.0, None), "inside an outline is empty");
        let ellipse = Shape::Ellipse { rect, color: Color32::RED, width: 4.0 };
        assert!(ellipse.hit(Pos2::new(60.0, 10.0), 3.0, None));
        assert!(!ellipse.hit(Pos2::new(12.0, 12.0), 3.0, None), "the corner is outside an ellipse");
        let arrow = Shape::Arrow { from: Pos2::new(0.0, 0.0), to: Pos2::new(100.0, 0.0), color: Color32::RED, width: 4.0 };
        assert!(arrow.hit(Pos2::new(50.0, 5.0), 3.0, None));
        assert!(!arrow.hit(Pos2::new(50.0, 20.0), 3.0, None));
        let highlight = Shape::Highlight { rect, color: Color32::YELLOW };
        assert!(highlight.hit(Pos2::new(60.0, 35.0), 3.0, None), "filled shapes hit inside");

        let pen = Shape::Pen { points: vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0)], color: Color32::RED, width: 2.0 };
        let to = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(20.0, 40.0));
        match pen.resized(pen.bounds(None), to) {
            Shape::Pen { points, .. } => assert_eq!(points, vec![Pos2::new(0.0, 0.0), Pos2::new(20.0, 40.0)]),
            _ => unreachable!(),
        }
    }

    #[test]
    fn amends_merge_per_property() {
        let mut doc = Doc::from_image(RgbaImage::new(50, 50));
        let rect = Rect::from_min_max(Pos2::new(5.0, 5.0), Pos2::new(20.0, 20.0));
        let boxed = |width: f32, color: Color32| Shape::Box { rect, color, width };
        doc.add(boxed(2.0, Color32::RED));
        doc.amend(0, boxed(3.0, Color32::RED), "width");
        doc.amend(0, boxed(5.0, Color32::RED), "width");
        doc.amend(0, boxed(5.0, Color32::BLUE), "color");
        doc.undo();
        assert!(doc.current().shapes[0] == boxed(5.0, Color32::RED), "colour change is its own step");
        doc.undo();
        assert!(doc.current().shapes[0] == boxed(2.0, Color32::RED), "width changes merged");

        // Never merge into the saved state, so saving still marks the change dirty.
        doc.amend(0, boxed(3.0, Color32::RED), "width");
        doc.mark_saved(PathBuf::new());
        doc.amend(0, boxed(4.0, Color32::RED), "width");
        assert!(doc.dirty());
        doc.undo();
        assert!(doc.current().shapes[0] == boxed(3.0, Color32::RED));
    }

    #[test]
    fn prunes_layers_of_deleted_screenshots() {
        let dir = std::env::temp_dir().join(format!("rsnip-prune-{}", std::process::id()));
        let layers = dir.join(".rsnip");
        std::fs::create_dir_all(&layers).unwrap();
        std::fs::write(dir.join("kept.png"), b"png").unwrap();
        for name in ["kept.png.json", "kept.png.base.png", "gone.png.json", "gone.png.base.png", "notes.txt"] {
            std::fs::write(layers.join(name), b"x").unwrap();
        }
        prune_layers(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&layers).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(left, ["kept.png.base.png", "kept.png.json", "notes.txt"]);
    }
}
