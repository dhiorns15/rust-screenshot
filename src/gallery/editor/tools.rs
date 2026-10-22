//! Tool definitions and the per-tool settings shown in the toolbar.

use eframe::egui::{Color32, Key};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
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

pub const TOOLS: [(Tool, &str, Key, &str); 9] = [
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

pub struct ToolState {
    pub tool: Tool,
    /// Remembered per tool, so the highlighter stays yellow while arrows stay red.
    pub(super) colors: [Color32; TOOLS.len()],
    pub width: f32,
    /// Font size in image pixels.
    pub text_size: f32,
    pub text_shadow: bool,
}

pub const TEXT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=300.0;

impl Default for ToolState {
    fn default() -> Self {
        let mut colors = [PALETTE[0]; TOOLS.len()];
        colors[Tool::Highlight as usize] = PALETTE[2];
        ToolState { tool: Tool::Arrow, colors, width: 4.0, text_size: 28.0, text_shadow: true }
    }
}

impl ToolState {
    pub fn color(&self) -> Color32 {
        self.colors[self.tool as usize]
    }

    pub fn color_mut(&mut self) -> &mut Color32 {
        &mut self.colors[self.tool as usize]
    }

    /// The Text tool's colour, whichever tool is selected.
    pub(super) fn text_color(&self) -> Color32 {
        self.colors[Tool::Text as usize]
    }

    pub fn uses_color(&self) -> bool {
        !matches!(self.tool, Tool::Blur | Tool::Crop)
    }

    pub(super) fn step_diameter(&self) -> f32 {
        20.0 + self.width * 4.0
    }
}
