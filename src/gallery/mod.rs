//! Screenshot library and editor window (`rsnip --gallery [file]`).

#[cfg(test)]
mod app_tests;
mod color;
mod doc;
#[cfg(windows)]
mod dragout;
mod library;
mod platform;
mod editor;
mod env;
mod theme;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ab_glyph::FontVec;
use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, Key, KeyboardShortcut, Layout, Modifiers, RichText, Sense,
    Stroke, TextureHandle, TextureOptions, Vec2, ViewportCommand,
};

use crate::update;
use doc::{apply_effects, flatten, render, Doc, Effects};
use editor::{Editor, Tool, ToolState, GROUP_STARTS, PALETTE, PALETTE_NAMES, TEXT_SIZES, TOOLS};
use env::Env;
use theme::{icon, IconButton};

pub const TITLE: &str = "RSnip Library";
const THUMB: Vec2 = Vec2::new(200.0, 112.0);
const RESCAN_EVERY: Duration = Duration::from_secs(2);

pub fn run(file: Option<PathBuf>) {
    if !platform::claim_single_instance(TITLE) {
        // The running library picks this up and opens the file.
        let request = file.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = std::fs::write(env::request_file(), request);
        return;
    }
    let _ = std::fs::remove_file(env::request_file());

    let size = 64;
    let bgra = crate::icon_art::render(size);
    let rgba = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(Arc::new(egui::IconData { rgba, width: size as u32, height: size as u32 })),
        ..Default::default()
    };
    let _ = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, Env::real(), file)))));
}

struct Entry {
    path: PathBuf,
    name: String,
    modified: SystemTime,
    /// A screen recording, which opens in the default player rather than the editor.
    video: bool,
}

enum Pending {
    Open(PathBuf),
    Close,
}

/// Where the library sits in the window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dock {
    Left,
    /// A filmstrip along the bottom, leaving the editor the full width.
    Bottom,
}

impl Dock {
    const SETTING: &'static str = "LibraryDock";

    /// Unknown values (or none) mean the default, on the left.
    fn from_setting(value: Option<u32>) -> Dock {
        match value {
            Some(1) => Dock::Bottom,
            _ => Dock::Left,
        }
    }

    fn to_setting(self) -> u32 {
        match self {
            Dock::Left => 0,
            Dock::Bottom => 1,
        }
    }
}

struct App {
    env: Env,
    entries: Vec<Entry>,
    last_scan: Instant,
    filter: String,
    thumbs: HashMap<PathBuf, (SystemTime, TextureHandle)>,
    requested: HashSet<PathBuf>,
    thumb_tx: Sender<(PathBuf, SystemTime)>,
    thumb_rx: Receiver<(PathBuf, SystemTime, Option<ColorImage>)>,
    editor: Option<Editor>,
    tools: ToolState,
    font: Option<FontVec>,
    /// Action waiting on the "unsaved changes" dialog.
    pending: Option<Pending>,
    allow_close: bool,
    status: Option<(String, Instant)>,
    /// Border/shadow added to everything the editor outputs.
    effects: Effects,
    /// Text recognition running for "Copy text".
    ocr: Option<Receiver<Result<String, String>>>,
    /// A newer release, once the update check has found one.
    newer: Option<update::Release>,
    update_rx: Option<Receiver<update::Release>>,
    /// Library items selected for bulk actions.
    selected: HashSet<PathBuf>,
    /// Where Shift+click ranges start.
    anchor: Option<PathBuf>,
    dock: Dock,
    /// How wide the toolbar's actions were last frame, to decide whether they fit beside the tools.
    actions_width: f32,
}

impl App {
    fn new(ctx: &egui::Context, env: Env, file: Option<PathBuf>) -> App {
        theme::apply(ctx);
        let font_bytes = doc::load_font_bytes();
        let mut fonts = egui::FontDefinitions::default();
        theme::add_icons(&mut fonts);
        if let Some(bytes) = font_bytes.clone() {
            fonts.font_data.insert(editor::ANNOTATION_FONT.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.insert(
                egui::FontFamily::Name(editor::ANNOTATION_FONT.into()),
                vec![editor::ANNOTATION_FONT.to_owned()],
            );
        }
        ctx.set_fonts(fonts);
        let (thumb_tx, jobs) = channel::<(PathBuf, SystemTime)>();
        let (results, thumb_rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            for (path, modified) in jobs {
                let thumb = image::open(&path).ok().map(|img| {
                    let t = img.thumbnail(THUMB.x as u32 * 2, THUMB.y as u32 * 2).to_rgba8();
                    ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw())
                });
                if results.send((path, modified, thumb)).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });

        let mut app = App {
            entries: Vec::new(),
            last_scan: Instant::now(),
            filter: String::new(),
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            thumb_tx,
            thumb_rx,
            editor: None,
            tools: ToolState::default(),
            font: font_bytes.and_then(|b| FontVec::try_from_vec(b).ok()),
            pending: None,
            allow_close: false,
            status: None,
            effects: load_effects(&env),
            ocr: None,
            newer: None,
            update_rx: env.check_updates.then(|| check_for_update(ctx)),
            selected: HashSet::new(),
            anchor: None,
            dock: Dock::from_setting(env.setting(Dock::SETTING)),
            actions_width: 320.0,
            env,
        };
        app.rescan();
        doc::prune_layers(&app.env.dir);
        match file {
            Some(file) => app.open(&file),
            None => {
                if let Some(first) = app.entries.iter().find(|e| !e.video).map(|e| e.path.clone()) {
                    app.open(&first);
                }
            }
        }
        app
    }

    fn rescan(&mut self) {
        self.last_scan = Instant::now();
        let Ok(dir) = std::fs::read_dir(&self.env.dir) else { return };
        let mut entries: Vec<Entry> = dir
            .flatten()
            .filter(|e| {
                let ext = e.path().extension().map(|x| x.to_ascii_lowercase());
                matches!(ext.as_deref().and_then(|x| x.to_str()), Some("png" | "jpg" | "jpeg" | "gif" | "mp4"))
            })
            .filter_map(|e| {
                let modified = e.metadata().ok()?.modified().ok()?;
                let name = e.path().file_stem()?.to_string_lossy().into_owned();
                let video = is_video(&e.path());
                Some(Entry { path: e.path(), name, modified, video })
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
        let listed: HashSet<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        let vanished = self.entries.iter().any(|e| !listed.contains(e.path.as_path()));
        if vanished {
            doc::prune_layers(&self.env.dir);
        }
        self.entries = entries;
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.doc.dirty())
    }

    /// Opens a file, asking first if the current one has unsaved edits.
    fn request_open(&mut self, path: PathBuf) {
        if self.editor.as_ref().is_some_and(|e| e.doc.path == path) {
            return;
        }
        // Text stays selected until committed; don't lose it when switching files.
        if let Some(editor) = &mut self.editor {
            editor.finish_text(&self.tools);
        }
        if self.dirty() {
            self.pending = Some(Pending::Open(path));
        } else {
            self.open(&path);
        }
    }

    fn open(&mut self, path: &Path) {
        match Doc::open(path) {
            Ok(doc) => self.editor = Some(Editor::new(doc)),
            Err(e) => self.set_status(e),
        }
    }

    /// The edited image as it's exported: annotations flattened, plus any border/shadow.
    fn rendered(&mut self) -> Option<image::RgbaImage> {
        let editor = self.editor.as_mut()?;
        editor.finish_text(&self.tools);
        Some(apply_effects(render(editor.doc.current(), self.font.as_ref()), self.effects))
    }

    fn set_effects(&mut self, effects: Effects) {
        self.effects = effects;
        self.env.set_setting("ExportBorder", effects.border.is_some() as u32);
        if let Some(c) = effects.border {
            self.env.set_setting("ExportBorderColor", u32::from_be_bytes([0, c.r(), c.g(), c.b()]));
        }
        self.env.set_setting("ExportShadow", effects.shadow as u32);
    }

    fn save(&mut self, as_copy: bool) -> bool {
        let Some(img) = self.rendered() else { return false };
        let framed = self.effects.any();
        let editor = self.editor.as_mut().unwrap();
        let path = if as_copy { copy_path(&editor.doc.path, "png") } else { editor.doc.path.clone() };
        // Only a drop shadow needs transparency; plain screenshots stay smaller as RGB.
        let img = if img.pixels().any(|p| p[3] < 255) {
            image::DynamicImage::ImageRgba8(img)
        } else {
            image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).to_rgb8())
        };
        let result = img.save_with_format(&path, image::ImageFormat::Png);
        match result {
            Ok(()) => {
                editor.doc.mark_saved(path.clone());
                let layers = editor.doc.save_layers(framed);
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                match layers {
                    Ok(()) => self.set_status(format!("Saved {name}")),
                    Err(e) => self.set_status(format!("Saved {name}, but annotations won't stay editable: {e}")),
                }
                self.rescan();
                true
            }
            Err(e) => {
                self.set_status(format!("Couldn't save: {e}"));
                false
            }
        }
    }

    /// Saves a JPG copy next to the screenshot; the editor keeps working on the original.
    fn save_jpg(&mut self) {
        let Some(img) = self.rendered() else { return };
        let path = copy_path(&self.editor.as_ref().unwrap().doc.path, "jpg");
        match save_jpg(&img, &path) {
            Ok(()) => {
                self.set_status(format!("Saved {}", path.file_name().unwrap_or_default().to_string_lossy()));
                self.rescan();
            }
            Err(e) => self.set_status(format!("Couldn't save: {e}")),
        }
    }

    /// Puts the edited image on the clipboard as a file, so it pastes as an attachment.
    fn copy_as_file(&mut self) {
        let Some(img) = self.rendered() else { return };
        let name = self.editor.as_ref().unwrap().doc.path.file_name().unwrap_or_default().to_owned();
        let dir = std::env::temp_dir().join("RSnip");
        let path = dir.join(name).with_extension("png");
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| img.save_with_format(&path, image::ImageFormat::Png).map_err(|e| e.to_string()))
            .and_then(|()| platform::copy_files(&[path]));
        match result {
            Ok(()) => self.set_status("Copied as a file — paste it into a folder, chat or email"),
            Err(e) => self.set_status(format!("Couldn't copy: {e}")),
        }
    }

    fn copy(&mut self) {
        let Some(img) = self.rendered() else { return };
        match platform::copy_image(&img) {
            Ok(()) => self.set_status("Copied to clipboard"),
            Err(e) => self.set_status(e),
        }
    }

    fn pin(&mut self) {
        let Some(img) = self.rendered() else { return };
        if let Err(e) = platform::pin(&img) {
            self.set_status(format!("Couldn't pin: {e}"));
        }
    }

    /// Starts reading the text in the edited image; `poll` copies it when done.
    fn copy_text(&mut self) {
        let Some(img) = self.rendered() else { return };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(platform::read_text(&img));
        });
        self.ocr = Some(rx);
        self.set_status("Reading text…");
    }

    /// Moves screenshots to the Recycle Bin, with their editing data.
    fn delete(&mut self, paths: &[PathBuf]) {
        let mut failed = 0;
        for path in paths {
            if !platform::recycle(path) {
                failed += 1;
                continue;
            }
            doc::remove_layers(path);
            if self.editor.as_ref().is_some_and(|e| &e.doc.path == path) {
                self.editor = None;
            }
            self.thumbs.remove(path);
            self.selected.remove(path);
        }
        self.rescan();
        match (failed, paths.len()) {
            (0, 1) => self.set_status("Moved to Recycle Bin"),
            (0, n) => self.set_status(format!("Moved {n} screenshots to Recycle Bin")),
            (f, _) => self.set_status(format!("Couldn't delete {f} of them")),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, modified, thumb)) = self.thumb_rx.try_recv() {
            // Files that can't be read stay "requested", so they aren't retried every frame.
            if let Some(thumb) = thumb {
                self.requested.remove(&path);
                let tex = ctx.load_texture(path.to_string_lossy(), thumb, TextureOptions::LINEAR);
                self.thumbs.insert(path, (modified, tex));
            }
        }
        if let Some(rx) = &self.ocr {
            if let Ok(result) = rx.try_recv() {
                self.ocr = None;
                let copied = result.and_then(|text| {
                    if text.trim().is_empty() {
                        return Err("No text found in this image".into());
                    }
                    platform::copy_text(&text).map(|()| text.lines().count())
                });
                match copied {
                    Ok(1) => self.set_status("Copied 1 line of text"),
                    Ok(n) => self.set_status(format!("Copied {n} lines of text")),
                    Err(e) => self.set_status(e),
                }
            }
        }
        if let Some(release) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.newer = Some(release);
            self.update_rx = None;
        }
        if self.last_scan.elapsed() >= RESCAN_EVERY {
            self.rescan();
        }
        if let Ok(request) = std::fs::read_to_string(&self.env.request_file) {
            let _ = std::fs::remove_file(&self.env.request_file);
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.rescan();
            if !request.trim().is_empty() {
                self.request_open(PathBuf::from(request.trim()));
            }
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = self.editor.as_ref().is_some_and(|e| e.is_typing()) || ctx.egui_wants_keyboard_input();
        if typing || self.editor.is_none() {
            return;
        }
        let shortcut = |m, k| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || shortcut(Modifiers::COMMAND, Key::Y) {
            self.editor.as_mut().unwrap().doc.redo();
        }
        if shortcut(Modifiers::COMMAND, Key::Z) {
            self.editor.as_mut().unwrap().doc.undo();
        }
        if shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            self.save(true);
        } else if shortcut(Modifiers::COMMAND, Key::S) {
            self.save(false);
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            self.copy();
        }
        if shortcut(Modifiers::COMMAND, Key::Num0) {
            self.editor.as_mut().unwrap().zoom = None;
        }
        for (tool, _, key, _) in TOOLS {
            if shortcut(Modifiers::NONE, key) {
                self.tools.tool = tool;
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let enabled = self.editor.is_some();
        // Tools on the left and actions on the right, or the actions on a row of their own when the
        // window is too narrow for both. (A row around the right-to-left layout keeps it from
        // taking the panel's whole height.)
        let mut fits = true;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(enabled, |ui| self.tool_buttons(ui));
            fits = actions_fit(ui.available_width(), self.actions_width);
            if fits {
                self.actions_row(ui, enabled);
            }
        });
        if !fits {
            ui.horizontal(|ui| self.actions_row(ui, enabled));
        }
    }

    /// The tools as icon buttons, one rounded group per kind of tool.
    fn tool_buttons(&mut self, ui: &mut egui::Ui) {
        for group in tool_groups() {
            theme::group(ui, |ui| {
                for (tool, label, _, help) in group {
                    let button = IconButton::new(theme::tool_icon(tool), label).selected(self.tools.tool == tool);
                    let response = ui.add(button).on_hover_ui(|ui| {
                        ui.label(RichText::new(label).strong());
                        ui.label(help);
                    });
                    if response.clicked() {
                        if let Some(editor) = &mut self.editor {
                            editor.finish_text(&self.tools);
                        }
                        self.tools.tool = tool;
                    }
                }
            });
        }
    }

    /// The actions, right-aligned; remembers how wide they are for the next frame's layout.
    fn actions_row(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let width = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let right = ui.max_rect().right();
            self.actions(ui, enabled);
            right - ui.min_rect().left()
        });
        self.actions_width = width.inner;
    }

    /// Undo/redo, Copy, Save and the Export menu, added right to left.
    fn actions(&mut self, ui: &mut egui::Ui, enabled: bool) {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(format!("{}  Export  {}", icon::EXPORT, icon::CARET_DOWN), |ui| self.export_menu(ui))
        });
        let save = theme::primary_button(theme::label(icon::FLOPPY_DISK, "Save"));
        if ui.add_enabled(enabled && self.dirty(), save).on_hover_text("Overwrite the screenshot (Ctrl+S)").clicked() {
            self.save(false);
        }
        let copy = egui::Button::new(theme::label(icon::COPY, "Copy"));
        if ui.add_enabled(enabled, copy).on_hover_text("Copy to clipboard (Ctrl+C)").clicked() {
            self.copy();
        }
        ui.add_space(6.0);
        let (can_undo, can_redo) = self.editor.as_ref().map_or((false, false), |e| (e.doc.can_undo(), e.doc.can_redo()));
        if ui.add_enabled(can_redo, IconButton::new(icon::ARROW_CLOCKWISE, "Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
            let editor = self.editor.as_mut().unwrap();
            editor.finish_text(&self.tools);
            editor.doc.redo();
        }
        if ui.add_enabled(can_undo, IconButton::new(icon::ARROW_COUNTER_CLOCKWISE, "Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
            let editor = self.editor.as_mut().unwrap();
            editor.finish_text(&self.tools);
            editor.doc.undo();
        }
    }

    /// The row under the toolbar: only what applies to the current tool or selected annotation,
    /// which these edit (otherwise they set the tool's defaults), and a hint on how to use it.
    fn tool_options(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            ui.spacing_mut().slider_width = 120.0;
            if self.tools.uses_color() {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (color, name) in PALETTE.into_iter().zip(PALETTE_NAMES) {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                    let selected = *self.tools.color_mut() == color;
                    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
                    let center = rect.center();
                    if selected {
                        ui.painter().circle_stroke(center, 9.5, Stroke::new(2.0f32, Color32::WHITE));
                    } else if response.hovered() {
                        ui.painter().circle_stroke(center, 9.5, Stroke::new(1.5f32, theme::MUTED));
                    }
                    ui.painter().circle(center, 7.0, color, Stroke::new(1.0f32, Color32::from_black_alpha(90)));
                    if response.clicked() {
                        *self.tools.color_mut() = color;
                    }
                }
                ui.add_space(4.0);
                self.custom_color(ui);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
            }
            if self.tools.style_tool() == Tool::Text {
                ui.label(RichText::new("Size").color(theme::MUTED));
                ui.add(egui::Slider::new(self.tools.text_size_mut(), TEXT_SIZES).step_by(1.0).fixed_decimals(0))
                    .on_hover_text("Text size. Or drag the handle on the selected text's corner.");
                ui.add_space(4.0);
                ui.checkbox(self.tools.shadow_mut(), "Shadow")
                    .on_hover_text("Drop shadow behind text, for readability on busy backgrounds");
            } else if self.tools.uses_width() {
                ui.label(RichText::new("Size").color(theme::MUTED));
                ui.add(egui::Slider::new(self.tools.width_mut(), 1.0..=24.0).step_by(1.0).fixed_decimals(0));
            }
            let selected = self.tools.selection.is_some();
            if selected {
                ui.add_space(4.0);
                let delete = egui::Button::new(theme::label(icon::TRASH, "Delete"));
                if ui.add(delete).on_hover_text("Delete the selected annotation (Del)").clicked() {
                    if let Some(editor) = &mut self.editor {
                        editor.delete_selected(&mut self.tools);
                    }
                }
            }
            let hint = if selected {
                "Drag to move it, or drag its handles to resize it. Esc deselects it"
            } else {
                TOOLS.iter().find(|t| t.0 == self.tools.tool).map_or("", |t| t.3)
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(hint).color(theme::MUTED)).truncate());
            });
        });
    }

    /// A rainbow ring that opens a full colour picker; it shows the colour when it isn't a swatch.
    fn custom_color(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Pick any colour"));
        let center = rect.center();
        let segments = 24;
        for k in 0..segments {
            let angle = |k: usize| k as f32 / segments as f32 * std::f32::consts::TAU;
            let point = |a: f32| center + Vec2::angled(a) * 7.0;
            let hue = egui::ecolor::Hsva::new(k as f32 / segments as f32, 0.8, 1.0, 1.0);
            ui.painter().line_segment([point(angle(k)), point(angle(k + 1))], Stroke::new(3.0f32, Color32::from(hue)));
        }
        let color = *self.tools.color_mut();
        if !PALETTE.contains(&color) {
            ui.painter().circle_filled(center, 4.0, color);
            ui.painter().circle_stroke(center, 9.5, Stroke::new(2.0f32, Color32::WHITE));
        } else if response.hovered() {
            ui.painter().circle_stroke(center, 9.5, Stroke::new(1.5f32, theme::MUTED));
        }
        let response = response.on_hover_text("Pick any colour");
        egui::Popup::from_toggle_button_response(&response)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| color::picker(ui, self.tools.color_mut()));
    }

    fn export_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(200.0);
        if ui.button(theme::label(icon::FLOPPY_DISK, "Save copy…")).on_hover_text("Save as a new file (Ctrl+Shift+S)").clicked() {
            self.save(true);
            ui.close();
        }
        if ui.button(theme::label(icon::FILE_JPG, "Save copy as JPG")).clicked() {
            self.save_jpg();
            ui.close();
        }
        if platform::EXTRAS {
            let as_file = ui.button(theme::label(icon::PAPERCLIP, "Copy as file"));
            if as_file.on_hover_text("Paste it into a folder, chat or email as an attachment").clicked() {
                self.copy_as_file();
                ui.close();
            }
            ui.separator();
            let reading = self.ocr.is_some();
            let copy_text = ui.add_enabled(!reading, egui::Button::new(theme::label(icon::TEXT_AA, "Copy text (OCR)")));
            if copy_text.on_hover_text("Recognise the text in the image and copy it").clicked() {
                self.copy_text();
                ui.close();
            }
            let pin = ui.button(theme::label(icon::PUSH_PIN, "Pin to screen"));
            if pin.on_hover_text("Keep the image on top of other windows for reference").clicked() {
                self.pin();
                ui.close();
            }
        }
        ui.separator();
        ui.label(RichText::new("Added to copies, saves and exports").small().weak());
        let mut fx = self.effects;
        let mut border = fx.border.is_some();
        ui.horizontal(|ui| {
            ui.checkbox(&mut border, "Border");
            let mut color = fx.border.unwrap_or(Color32::from_gray(32));
            if border && egui::color_picker::color_edit_button_srgba(ui, &mut color, egui::color_picker::Alpha::Opaque).changed() {
                fx.border = Some(color);
            }
            fx.border = border.then_some(fx.border.unwrap_or(color));
        });
        ui.checkbox(&mut fx.shadow, "Drop shadow");
        if fx != self.effects {
            self.set_effects(fx);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(editor) = &self.editor {
                let name = editor.doc.path.file_name().unwrap_or_default().to_string_lossy();
                let size = editor.doc.size();
                let marker = if editor.doc.dirty() { " •" } else { "" };
                ui.label(format!("{name}{marker}"));
                ui.label(RichText::new(format!("{} × {} px", size.x, size.y)).weak());
            }
            if let Some((text, at)) = &self.status {
                if at.elapsed() < Duration::from_secs(4) {
                    ui.label(RichText::new(text).color(theme::ACCENT));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(release) = &self.newer {
                    let link = ui.link(format!("RSnip {} is available", release.version));
                    if link.on_hover_text(&release.page).clicked() {
                        platform::open(Path::new(&release.page));
                    }
                }
                let Some(editor) = &mut self.editor else { return };
                if ui.small_button("1:1").on_hover_text("Actual size").clicked() {
                    editor.zoom = Some(1.0);
                }
                if ui.small_button("Fit").on_hover_text("Ctrl+0").clicked() {
                    editor.zoom = None;
                }
                ui.label(format!("{:.0}%", editor.shown_scale * 100.0));
                ui.label(RichText::new("Ctrl+scroll to zoom").weak().small());
            });
        });
    }

    fn unsaved_dialog(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            return;
        }
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("This screenshot has edits that haven't been saved.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for label in ["Save", "Discard", "Cancel"] {
                        if ui.button(label).clicked() {
                            choice = Some(label);
                        }
                    }
                });
            });
        let Some(choice) = choice else { return };
        let pending = self.pending.take().unwrap();
        if choice == "Cancel" || (choice == "Save" && !self.save(false)) {
            return;
        }
        match pending {
            Pending::Open(path) => self.open(&path),
            Pending::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.poll(ctx);
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing {
            if let Some(editor) = &mut self.editor {
                editor.finish_text(&self.tools);
            }
        }
        if closing && self.dirty() && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        if self.pending.is_none() {
            self.shortcuts(ctx);
        }

        let bar = |fill| egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(10, 7)).stroke(egui::Stroke::new(1.0f32, theme::BORDER));
        egui::Panel::top("toolbar").frame(bar(theme::PANEL)).show_separator_line(false).show(ui, |ui| self.toolbar(ui));
        if self.editor.is_some() {
            egui::Panel::top("options").frame(bar(theme::SUBPANEL)).show_separator_line(false).show(ui, |ui| self.tool_options(ui));
        }
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| self.status_bar(ui));
        let library = egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::same(10));
        match self.dock {
            Dock::Left => {
                egui::Panel::left("library")
                    .frame(library)
                    .resizable(true)
                    .default_size(250.0)
                    .size_range(200.0..=420.0)
                    .show(ui, |ui| self.library(ui));
            }
            Dock::Bottom => {
                egui::Panel::bottom("library_strip")
                    .frame(library)
                    .resizable(false)
                    .exact_size(library::STRIP_HEIGHT)
                    .show(ui, |ui| self.library(ui));
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::CANVAS)).show(ui, |ui| {
            match &mut self.editor {
                Some(editor) => editor.ui(ui, &mut self.tools, self.font.as_ref()),
                None => {
                    ui.centered_and_justified(|ui| ui.label("Select a screenshot from the library."));
                }
            }
        });
        self.unsaved_dialog(ctx);
    }
}

/// The toolbar's tools split into its groups, in order.
fn tool_groups() -> Vec<Vec<(Tool, &'static str, Key, &'static str)>> {
    let mut groups: Vec<Vec<_>> = Vec::new();
    for entry in TOOLS {
        if groups.is_empty() || GROUP_STARTS.contains(&entry.0) {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(entry);
    }
    groups
}

/// Whether the toolbar's actions fit beside the tools, with a gap, or need a row of their own.
fn actions_fit(available: f32, actions_width: f32) -> bool {
    available >= actions_width + 12.0
}

/// Looks for a newer release in the background (see `Env::check_updates`: on Windows the tray app
/// does this, and can install it).
fn check_for_update(ctx: &egui::Context) -> Receiver<update::Release> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if let Ok(Some(release)) = update::check() {
            let _ = tx.send(release);
            ctx.request_repaint();
        }
    });
    rx
}

/// "name (edited).png", "name (edited 2).png", ...
fn copy_path(path: &Path, ext: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut candidate = dir.join(format!("{stem} (edited).{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} (edited {n}).{ext}"));
        n += 1;
    }
    candidate
}

fn save_jpg(img: &image::RgbaImage, path: &Path) -> Result<(), String> {
    let rgb = image::DynamicImage::ImageRgba8(flatten(img)).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90).encode_image(&rgb).map_err(|e| e.to_string())
}

/// Screen recordings; they open in the default player.
fn is_video(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("gif" | "mp4"))
}

fn open_with_shell(path: &Path) {
    platform::open(path);
}

fn load_effects(env: &Env) -> Effects {
    let get = |name| env.setting(name);
    let [_, r, g, b] = get("ExportBorderColor").unwrap_or(0x202020).to_be_bytes();
    Effects {
        border: (get("ExportBorder") == Some(1)).then_some(Color32::from_rgb(r, g, b)),
        shadow: get("ExportShadow") == Some(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder of its own for one test.
    pub(crate) fn sandbox_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn tools_are_grouped_in_order() {
        let groups = tool_groups();
        let flat: Vec<Tool> = groups.iter().flatten().map(|t| t.0).collect();
        assert_eq!(flat, TOOLS.map(|t| t.0), "every tool once, in toolbar order");
        let starts: Vec<Tool> = groups.iter().skip(1).map(|g| g[0].0).collect();
        assert_eq!(starts, GROUP_STARTS);
        assert_eq!(groups[0].iter().map(|t| t.0).collect::<Vec<_>>(), [Tool::Select]);
    }

    #[test]
    fn actions_move_to_their_own_row_when_they_dont_fit() {
        assert!(actions_fit(600.0, 320.0));
        assert!(actions_fit(332.0, 320.0));
        assert!(!actions_fit(331.0, 320.0), "needs a gap beside the tools");
    }

    #[test]
    fn dock_setting_round_trips_and_ignores_junk() {
        for dock in [Dock::Left, Dock::Bottom] {
            assert!(Dock::from_setting(Some(dock.to_setting())) == dock);
        }
        assert!(Dock::from_setting(None) == Dock::Left);
        assert!(Dock::from_setting(Some(7)) == Dock::Left);
    }

    #[test]
    fn export_effects_load_from_settings_and_survive_junk() {
        let env = Env::sandbox(sandbox_dir("effects"));
        assert!(!load_effects(&env).any(), "off by default");
        env.set_setting("ExportBorder", 1);
        env.set_setting("ExportBorderColor", 0x00FF8000);
        env.set_setting("ExportShadow", 1);
        let fx = load_effects(&env);
        assert_eq!(fx.border, Some(Color32::from_rgb(255, 128, 0)));
        assert!(fx.shadow);
        // Junk in the top byte or unexpected flag values doesn't break anything.
        env.set_setting("ExportBorderColor", 0xFFFF_FFFF);
        env.set_setting("ExportShadow", 9);
        let fx = load_effects(&env);
        assert_eq!(fx.border, Some(Color32::WHITE));
        assert!(!fx.shadow);
    }

    #[test]
    fn library_opens_in_a_sandbox() {
        let dir = sandbox_dir("library");
        for (name, shade) in [("Screenshot a.png", 40u8), ("Screenshot b.png", 200)] {
            image::RgbaImage::from_pixel(64, 32, image::Rgba([shade, shade, shade, 255])).save(dir.join(name)).unwrap();
        }
        let env = Env::sandbox(dir.clone());
        env.set_setting(Dock::SETTING, Dock::Bottom.to_setting());
        let ctx = egui::Context::default();
        let app = App::new(&ctx, env, None);
        assert_eq!(app.entries.len(), 2);
        assert!(app.dock == Dock::Bottom, "the dock comes from the sandbox's settings");
        assert!(app.editor.is_some(), "the newest screenshot opens");
        assert!(app.update_rx.is_none(), "no network in tests");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_jpg_copies_beside_the_original() {
        let dir = std::env::temp_dir().join(format!("rsnip-jpg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("shot.png");
        let path = super::copy_path(&original, "jpg");
        assert_eq!(path.file_name().unwrap(), "shot (edited).jpg");
        let img = image::RgbaImage::from_pixel(30, 20, image::Rgba([0, 0, 0, 0]));
        super::save_jpg(&img, &path).unwrap();
        let back = image::open(&path).unwrap().to_rgb8();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back.dimensions(), (30, 20));
        assert!(back.get_pixel(15, 10).0.iter().all(|&c| c > 245), "transparency flattens to white");
    }

    #[test]
    #[cfg(windows)]
    fn reads_rendered_text_back() {
        use super::doc::{load_font, render, Shape, Snapshot};
        let base = image::RgbaImage::from_pixel(420, 80, image::Rgba([255, 255, 255, 255]));
        let text = Shape::Text {
            pos: egui::Pos2::new(12.0, 14.0),
            text: "Hello RSnip 123".into(),
            color: egui::Color32::BLACK,
            size: 40.0,
            shadow: false,
        };
        let img = render(&Snapshot { base: std::sync::Arc::new(base), shapes: vec![text] }, load_font().as_ref());
        match super::platform::read_text(&img) {
            Ok(read) => assert!(read.to_lowercase().contains("hello rsnip 123"), "read {read:?}"),
            // Server images (like CI's) may not have an OCR language installed.
            Err(e) if e.starts_with("No text-recognition language") => eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
    }
}
