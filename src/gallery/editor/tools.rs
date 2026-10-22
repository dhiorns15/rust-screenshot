//! Tool definitions and the per-tool settings shown in the toolbar.

use eframe::egui::{Color32, Key};

use crate::gallery::doc::Shape;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Arrow,
    Box,
    Ellipse,
    Highlight,
    Blur,
    Pen,
    Text,
    Step,
    Crop,
}

pub const TOOLS: [(Tool, &str, Key, &str); 10] = [
    (Tool::Select, "Select", Key::V, "Click an annotation to select it, then move, resize, restyle or delete it (V)"),
    (Tool::Arrow, "Arrow", Key::A, "Drag to draw an arrow (A)"),
    (Tool::Box, "Box", Key::B, "Drag to draw a box (B)"),
    (Tool::Ellipse, "Ellipse", Key::E, "Drag to draw an ellipse (E)"),
    (Tool::Highlight, "Highlight", Key::H, "Drag to highlight an area (H)"),
    (Tool::Blur, "Blur", Key::U, "Drag to blur sensitive content (U)"),
    (Tool::Pen, "Pen", Key::P, "Draw freehand (P)"),
    (Tool::Text, "Text", Key::T, "Click to type text (T)"),
    (Tool::Step, "Step", Key::N, "Click to add numbered steps 1, 2, 3… (N)"),
    (Tool::Crop, "Crop", Key::C, "Drag to select an area to crop, then confirm (C)"),
];

/// egui font family holding the renderer's font, so the text preview matches the saved result.
pub const ANNOTATION_FONT: &str = "annotation";

pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(230, 40, 40),
    Color32::from_rgb(255, 140, 0),
    Color32::from_rgb(255, 225, 0),
    Color32::from_rgb(40, 180, 80),
    Color32::from_rgb(30, 120, 240),
    Color32::from_rgb(150, 70, 220),
    Color32::from_rgb(20, 20, 20),
    Color32::WHITE,
];

pub const TEXT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=300.0;

/// The toolbar-editable properties of a selected annotation. Fields its kind doesn't have are unused.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Style {
    pub kind: Tool,
    pub color: Color32,
    pub width: f32,
    pub text_size: f32,
    pub shadow: bool,
}

/// Steps are sized by the same slider as line widths.
fn step_diameter(width: f32) -> f32 {
    20.0 + width * 4.0
}

impl Style {
    pub fn of(shape: &Shape) -> Style {
        let plain = |kind, color, width| Style { kind, color, width, text_size: 0.0, shadow: false };
        match shape {
            Shape::Box { color, width, .. } => plain(Tool::Box, *color, *width),
            Shape::Ellipse { color, width, .. } => plain(Tool::Ellipse, *color, *width),
            Shape::Highlight { color, .. } => plain(Tool::Highlight, *color, 0.0),
            Shape::Blur { .. } => plain(Tool::Blur, Color32::TRANSPARENT, 0.0),
            Shape::Arrow { color, width, .. } => plain(Tool::Arrow, *color, *width),
            Shape::Pen { color, width, .. } => plain(Tool::Pen, *color, *width),
            Shape::Text { color, size, shadow, .. } => Style { kind: Tool::Text, color: *color, width: 0.0, text_size: *size, shadow: *shadow },
            Shape::Step { color, diameter, .. } => plain(Tool::Step, *color, (diameter - 20.0) / 4.0),
        }
    }

    /// `shape` restyled.
    pub fn apply(&self, shape: &Shape) -> Shape {
        let mut s = shape.clone();
        match &mut s {
            Shape::Box { color, width, .. } | Shape::Ellipse { color, width, .. } | Shape::Arrow { color, width, .. } | Shape::Pen { color, width, .. } => {
                *color = self.color;
                *width = self.width;
            }
            Shape::Highlight { color, .. } => *color = self.color,
            Shape::Blur { .. } => {}
            Shape::Text { color, size, shadow, .. } => {
                *color = self.color;
                *size = self.text_size;
                *shadow = self.shadow;
            }
            Shape::Step { color, diameter, .. } => {
                *color = self.color;
                *diameter = step_diameter(self.width);
            }
        }
        s
    }

    /// Names the property that differs from `other`, so edits to it can share an undo step.
    pub fn changed(&self, other: &Style) -> &'static str {
        if self.color != other.color {
            "color"
        } else if self.width != other.width {
            "width"
        } else if self.text_size != other.text_size {
            "size"
        } else {
            "shadow"
        }
    }
}

pub struct ToolState {
    pub tool: Tool,
    /// Remembered per tool, so the highlighter stays yellow while arrows stay red.
    pub(super) colors: [Color32; TOOLS.len()],
    pub width: f32,
    /// Font size in image pixels.
    pub text_size: f32,
    pub text_shadow: bool,
    /// The selected annotation's style, published by the editor each frame. While it's set, the
    /// toolbar edits it instead of the tool defaults, and the editor applies the changes.
    pub selection: Option<Style>,
}

impl Default for ToolState {
    fn default() -> Self {
        let mut colors = [PALETTE[0]; TOOLS.len()];
        colors[Tool::Highlight as usize] = PALETTE[2];
        ToolState { tool: Tool::Arrow, colors, width: 4.0, text_size: 28.0, text_shadow: true, selection: None }
    }
}

impl ToolState {
    /// The current tool's colour for drawing new shapes.
    pub fn color(&self) -> Color32 {
        self.colors[self.tool as usize]
    }

    /// The tool whose settings the toolbar shows: the selected annotation's kind, if any.
    pub fn style_tool(&self) -> Tool {
        self.selection.map_or(self.tool, |s| s.kind)
    }

    pub fn color_mut(&mut self) -> &mut Color32 {
        match &mut self.selection {
            Some(s) => &mut s.color,
            None => &mut self.colors[self.tool as usize],
        }
    }

    pub fn width_mut(&mut self) -> &mut f32 {
        match &mut self.selection {
            Some(s) => &mut s.width,
            None => &mut self.width,
        }
    }

    pub fn text_size_mut(&mut self) -> &mut f32 {
        match &mut self.selection {
            Some(s) => &mut s.text_size,
            None => &mut self.text_size,
        }
    }

    pub fn shadow_mut(&mut self) -> &mut bool {
        match &mut self.selection {
            Some(s) => &mut s.shadow,
            None => &mut self.text_shadow,
        }
    }

    /// The Text tool's colour, whichever tool is selected.
    pub(super) fn text_color(&self) -> Color32 {
        self.colors[Tool::Text as usize]
    }

    pub fn uses_color(&self) -> bool {
        !matches!(self.style_tool(), Tool::Select | Tool::Blur | Tool::Crop)
    }

    /// Whether the Size slider means a line width (or step size).
    pub fn uses_width(&self) -> bool {
        !matches!(self.style_tool(), Tool::Select | Tool::Highlight | Tool::Blur | Tool::Crop | Tool::Text)
    }

    pub(super) fn step_diameter(&self) -> f32 {
        step_diameter(self.width)
    }
}
