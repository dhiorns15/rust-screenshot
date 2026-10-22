//! Screenshot library and editor window (`rsnip --gallery [file]`).

mod doc;
#[cfg(windows)]
mod dragout;
mod library;
mod platform;
mod editor;

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
use editor::{Editor, Tool, ToolState, PALETTE, TEXT_SIZES, TOOLS};

pub const TITLE: &str = "RSnip Library";
const THUMB: Vec2 = Vec2::new(200.0, 112.0);
const RESCAN_EVERY: Duration = Duration::from_secs(2);

/// Another `--gallery` launch drops the file it wants opened here.
fn request_file() -> PathBuf {
    std::env::temp_dir().join("rsnip-gallery-request.txt")
}

pub fn run(file: Option<PathBuf>) {
    if !platform::claim_single_instance(TITLE) {
        // The running library picks this up and opens the file.
        let request = file.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = std::fs::write(request_file(), request);
        return;
    }
    let _ = std::fs::remove_file(request_file());

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
    let _ = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(App::new(cc, file)))));
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

struct App {
    dir: PathBuf,
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
}

impl App {
    fn new(cc: &eframe::CreationContext, file: Option<PathBuf>) -> App {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let font_bytes = doc::load_font_bytes();
        if let Some(bytes) = font_bytes.clone() {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(editor::ANNOTATION_FONT.to_owned(), egui::FontData::from_owned(bytes));
            fonts.families.insert(
                egui::FontFamily::Name(editor::ANNOTATION_FONT.into()),
                vec![editor::ANNOTATION_FONT.to_owned()],
            );
            cc.egui_ctx.set_fonts(fonts);
        }
        let (thumb_tx, jobs) = channel::<(PathBuf, SystemTime)>();
        let (results, thumb_rx) = channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            for (path, modified) in jobs {
                let thumb = image::open(&path).ok().map(|img| {
                    let t = img.thumbnail(THUMB.x as u32 * 2, THUMB.y as u32 * 2).to_rgba8();
                    ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw())
                });
                if results.send((path, modified, thumb)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });

        let mut app = App {
            dir: platform::screenshots_dir(),
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
            effects: load_effects(),
            ocr: None,
            newer: None,
            update_rx: check_for_update(&cc.egui_ctx),
            selected: HashSet::new(),
            anchor: None,
        };
        app.rescan();
        doc::prune_layers(&app.dir);
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
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return };
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
            doc::prune_layers(&self.dir);
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
        platform::set_setting("ExportBorder", effects.border.is_some() as u32);
        if let Some(c) = effects.border {
            platform::set_setting("ExportBorderColor", u32::from_be_bytes([0, c.r(), c.g(), c.b()]));
        }
        platform::set_setting("ExportShadow", effects.shadow as u32);
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
        if let Ok(request) = std::fs::read_to_string(request_file()) {
            let _ = std::fs::remove_file(request_file());
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
        let typing = self.editor.as_ref().is_some_and(|e| e.is_typing()) || ctx.wants_keyboard_input();
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
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let enabled = self.editor.is_some();
            for (tool, label, _, help) in TOOLS {
                let selected = self.tools.tool == tool;
                if ui.add_enabled(enabled, egui::SelectableLabel::new(selected, label)).on_hover_text(help).clicked() {
                    if let Some(editor) = &mut self.editor {
                        editor.finish_text(&self.tools);
                    }
                    self.tools.tool = tool;
                }
            }
            ui.separator();

            ui.add_enabled_ui(enabled && self.tools.uses_color(), |ui| {
                for color in PALETTE {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                    let selected = *self.tools.color_mut() == color;
                    ui.painter().rect_filled(rect, 3.0, color);
                    let outline = if selected { Stroke::new(2.0f32, Color32::WHITE) } else { Stroke::new(1.0f32, Color32::from_gray(70)) };
                    ui.painter().rect_stroke(rect.expand(if selected { 2.0 } else { 0.0 }), 4.0, outline);
                    if response.clicked() {
                        *self.tools.color_mut() = color;
                    }
                }
                egui::color_picker::color_edit_button_srgba(ui, self.tools.color_mut(), egui::color_picker::Alpha::Opaque);
            });
            // These edit the selected annotation when there is one, otherwise the tool's defaults.
            if self.tools.style_tool() == Tool::Text {
                ui.add_enabled(enabled, egui::Slider::new(self.tools.text_size_mut(), TEXT_SIZES).step_by(1.0).text("Size"))
                    .on_hover_text("Text size. Or drag the handle on the selected text's corner.");
                ui.add_enabled(enabled, egui::Checkbox::new(self.tools.shadow_mut(), "Shadow"))
                    .on_hover_text("Drop shadow behind text, for readability on busy backgrounds");
            } else {
                let size_applies = self.tools.uses_width();
                ui.add_enabled(enabled && size_applies, egui::Slider::new(self.tools.width_mut(), 1.0..=24.0).step_by(1.0).text("Size"));
            }
            if self.tools.selection.is_some() && ui.button("Delete").on_hover_text("Delete the selected annotation (Del)").clicked() {
                if let Some(editor) = &mut self.editor {
                    editor.delete_selected(&mut self.tools);
                }
            }
            ui.separator();

            let (can_undo, can_redo) = self.editor.as_ref().map_or((false, false), |e| (e.doc.can_undo(), e.doc.can_redo()));
            if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                let editor = self.editor.as_mut().unwrap();
                editor.finish_text(&self.tools);
                editor.doc.undo();
            }
            if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Ctrl+Y").clicked() {
                let editor = self.editor.as_mut().unwrap();
                editor.finish_text(&self.tools);
                editor.doc.redo();
            }
            ui.separator();
            if ui.add_enabled(enabled, egui::Button::new("Copy")).on_hover_text("Copy to clipboard (Ctrl+C)").clicked() {
                self.copy();
            }
            if platform::EXTRAS {
                if ui.add_enabled(enabled, egui::Button::new("Pin")).on_hover_text("Keep the image on top of other windows for reference").clicked() {
                    self.pin();
                }
                let reading = self.ocr.is_some();
                if ui.add_enabled(enabled && !reading, egui::Button::new("Copy text")).on_hover_text("Recognise the text in the image and copy it (OCR)").clicked() {
                    self.copy_text();
                }
            }
            let dirty = self.dirty();
            if ui.add_enabled(enabled && dirty, egui::Button::new("Save")).on_hover_text("Overwrite the screenshot (Ctrl+S)").clicked() {
                self.save(false);
            }
            if ui.add_enabled(enabled, egui::Button::new("Save copy")).on_hover_text("Save as a new file (Ctrl+Shift+S)").clicked() {
                self.save(true);
            }
            ui.add_enabled_ui(enabled, |ui| ui.menu_button("Export ▾", |ui| self.export_menu(ui)));
        });
    }

    fn export_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("Save copy as JPG").clicked() {
            self.save_jpg();
            ui.close_menu();
        }
        if platform::EXTRAS && ui.button("Copy as file").on_hover_text("Paste it into a folder, chat or email as an attachment").clicked() {
            self.copy_as_file();
            ui.close_menu();
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
                    ui.label(RichText::new(text).color(Color32::from_rgb(120, 200, 255)));
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
                if ui.small_button("100%").clicked() {
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        egui::TopBottomPanel::top("toolbar").frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(8.0)).show(ctx, |ui| self.toolbar(ui));
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| self.status_bar(ui));
        egui::SidePanel::left("library").resizable(true).default_width(240.0).width_range(200.0..=420.0).show(ctx, |ui| {
            self.library(ui)
        });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(Color32::from_gray(24))).show(ctx, |ui| {
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

/// "name (edited).png", "name (edited 2).png", ...
/// Looks for a newer release in the background, unless `CheckUpdates` is off. On Windows the tray
/// app does this (and can install it), so the library only checks on other systems.
fn check_for_update(ctx: &egui::Context) -> Option<Receiver<update::Release>> {
    if cfg!(windows) || platform::setting("CheckUpdates") == Some(0) {
        return None;
    }
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if let Ok(Some(release)) = update::check() {
            let _ = tx.send(release);
            ctx.request_repaint();
        }
    });
    Some(rx)
}

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

fn load_effects() -> Effects {
    let get = platform::setting;
    let [_, r, g, b] = get("ExportBorderColor").unwrap_or(0x202020).to_be_bytes();
    Effects {
        border: (get("ExportBorder") == Some(1)).then_some(Color32::from_rgb(r, g, b)),
        shadow: get("ExportShadow") == Some(1),
    }
}

#[cfg(test)]
mod tests {
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
