//! Screenshot library and editor window (`rsnip --gallery [file]`).

mod doc;
mod editor;

use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use ab_glyph::FontVec;
use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, Key, KeyboardShortcut, Layout, Modifiers, RichText, Sense,
    Stroke, TextureHandle, TextureOptions, Vec2, ViewportCommand,
};
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};

use crate::capture::Image;
use crate::util::{wide, wide_os};
use doc::{render, Doc};
use editor::{Editor, Tool, ToolState, PALETTE, TEXT_SIZES, TOOLS};

const TITLE: &str = "RSnip Library";
const THUMB: Vec2 = Vec2::new(200.0, 112.0);
const ROW_HEIGHT: f32 = 112.0 + 40.0;
const RESCAN_EVERY: Duration = Duration::from_secs(2);

/// Another `--gallery` launch drops the file it wants opened here.
fn request_file() -> PathBuf {
    std::env::temp_dir().join("rsnip-gallery-request.txt")
}

pub fn run(file: Option<PathBuf>) {
    let name = wide("Local\\RSnip.Gallery");
    let _mutex = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let request = file.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = std::fs::write(request_file(), request);
        let title = wide(TITLE);
        unsafe {
            let hwnd = FindWindowW(null(), title.as_ptr());
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }
        return;
    }
    let _ = std::fs::remove_file(request_file());

    let size = 64;
    let bgra = crate::icon::render(size);
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
            dir: crate::output::screenshots_dir(),
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
        };
        app.rescan();
        match file {
            Some(file) => app.open(&file),
            None => {
                if let Some(first) = app.entries.first().map(|e| e.path.clone()) {
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
                matches!(ext.as_deref().and_then(|x| x.to_str()), Some("png" | "jpg" | "jpeg"))
            })
            .filter_map(|e| {
                let modified = e.metadata().ok()?.modified().ok()?;
                let name = e.path().file_stem()?.to_string_lossy().into_owned();
                Some(Entry { path: e.path(), name, modified })
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
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

    fn rendered(&mut self) -> Option<image::RgbaImage> {
        let editor = self.editor.as_mut()?;
        editor.finish_text(&self.tools);
        Some(render(editor.doc.current(), self.font.as_ref()))
    }

    fn save(&mut self, as_copy: bool) -> bool {
        let Some(img) = self.rendered() else { return false };
        let editor = self.editor.as_mut().unwrap();
        let path = if as_copy { copy_path(&editor.doc.path) } else { editor.doc.path.clone() };
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let result = match path.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("png") => rgb.save(&path),
            _ => rgb.save_with_format(&path, image::ImageFormat::Png),
        };
        match result {
            Ok(()) => {
                editor.doc.mark_saved(path.clone());
                let layers = editor.doc.save_layers();
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

    fn copy(&mut self) {
        let Some(img) = self.rendered() else { return };
        let (width, height) = (img.width() as i32, img.height() as i32);
        let data = img.into_raw().chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        match crate::output::copy_to_clipboard(null_mut(), &Image { width, height, data }) {
            Ok(()) => self.set_status("Copied to clipboard"),
            Err(e) => self.set_status(e),
        }
    }

    fn delete(&mut self, path: &Path) {
        if recycle(path) {
            doc::remove_layers(path);
            if self.editor.as_ref().is_some_and(|e| e.doc.path == path) {
                self.editor = None;
            }
            self.thumbs.remove(path);
            self.rescan();
            self.set_status("Moved to Recycle Bin");
        } else {
            self.set_status("Couldn't delete the file");
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, modified, thumb)) = self.thumb_rx.try_recv() {
            self.requested.remove(&path);
            if let Some(thumb) = thumb {
                let tex = ctx.load_texture(path.to_string_lossy(), thumb, TextureOptions::LINEAR);
                self.thumbs.insert(path, (modified, tex));
            }
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
            let dirty = self.dirty();
            if ui.add_enabled(enabled && dirty, egui::Button::new("Save")).on_hover_text("Overwrite the screenshot (Ctrl+S)").clicked() {
                self.save(false);
            }
            if ui.add_enabled(enabled, egui::Button::new("Save copy")).on_hover_text("Save as a new file (Ctrl+Shift+S)").clicked() {
                self.save(true);
            }
        });
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Library");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Folder").on_hover_text(self.dir.display().to_string()).clicked() {
                    open_with_shell(&self.dir);
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search").desired_width(f32::INFINITY));
        ui.add_space(4.0);

        let filter = self.filter.to_lowercase();
        let visible: Vec<usize> =
            (0..self.entries.len()).filter(|&i| filter.is_empty() || self.entries[i].name.to_lowercase().contains(&filter)).collect();
        ui.label(RichText::new(format!("{} screenshots", visible.len())).small().weak());
        if visible.is_empty() {
            ui.add_space(20.0);
            ui.label("No screenshots yet. Press your capture hotkey to take one.");
            return;
        }

        let current = self.editor.as_ref().map(|e| e.doc.path.clone());
        let mut clicked = None;
        let mut action: Option<(PathBuf, &str)> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_HEIGHT, visible.len(), |ui, range| {
            for &i in &visible[range] {
                let entry = &self.entries[i];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
                let selected = current.as_deref() == Some(entry.path.as_path());
                let painter = ui.painter();
                if selected {
                    painter.rect_filled(rect.shrink(2.0), 6.0, ui.visuals().selection.bg_fill.gamma_multiply(0.6));
                } else if response.hovered() {
                    painter.rect_filled(rect.shrink(2.0), 6.0, ui.visuals().widgets.hovered.bg_fill);
                }

                let thumb_area = egui::Rect::from_min_size(rect.min + Vec2::new((rect.width() - THUMB.x) / 2.0, 8.0), THUMB);
                match self.thumbs.get(&entry.path) {
                    Some((modified, tex)) if *modified == entry.modified => {
                        let fit = (THUMB.x / tex.size_vec2().x).min(THUMB.y / tex.size_vec2().y);
                        let r = egui::Rect::from_center_size(thumb_area.center(), tex.size_vec2() * fit);
                        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0));
                        painter.image(tex.id(), r, uv, Color32::WHITE);
                    }
                    _ => {
                        painter.rect_filled(thumb_area, 4.0, Color32::from_gray(40));
                        if self.requested.insert(entry.path.clone()) {
                            let _ = self.thumb_tx.send((entry.path.clone(), entry.modified));
                        }
                    }
                }
                painter.text(
                    egui::Pos2::new(rect.center().x, thumb_area.max.y + 14.0),
                    Align2::CENTER_CENTER,
                    &entry.name,
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );

                if response.clicked() {
                    clicked = Some(entry.path.clone());
                }
                response.context_menu(|ui| {
                    for label in ["Open in default app", "Show in folder", "Delete"] {
                        if ui.button(label).clicked() {
                            action = Some((entry.path.clone(), label));
                            ui.close_menu();
                        }
                    }
                });
            }
        });

        if let Some(path) = clicked {
            self.request_open(path);
        }
        match action {
            Some((path, "Open in default app")) => open_with_shell(&path),
            Some((path, "Show in folder")) => show_in_folder(&path),
            Some((path, "Delete")) => self.delete(&path),
            _ => {}
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
fn copy_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut candidate = dir.join(format!("{stem} (edited).png"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} (edited {n}).png"));
        n += 1;
    }
    candidate
}

fn open_with_shell(path: &Path) {
    crate::shell_open(path);
}

fn show_in_folder(path: &Path) {
    let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

fn recycle(path: &Path) -> bool {
    // pFrom must be double-null-terminated.
    let mut from = wide_os(path.as_os_str());
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: null_mut(),
        lpszProgressTitle: null(),
    };
    unsafe { SHFileOperationW(&mut op) == 0 && op.fAnyOperationsAborted == 0 }
}
