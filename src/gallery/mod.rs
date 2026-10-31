//! Screenshot library and editor window (`rsnip --gallery [file]`).

#[cfg(test)]
mod app_tests;
mod auto_push;
pub(crate) mod camera;
#[cfg(not(windows))]
mod background;
#[cfg(not(windows))]
mod bring_in;
mod clean_up;
mod describer;
#[cfg(all(test, target_os = "linux"))]
mod desktop_tests;
#[cfg(not(windows))]
mod pick_area;
#[cfg(not(windows))]
mod file_clipboard;
#[cfg(not(windows))]
mod portal;
#[cfg(not(windows))]
mod recorder;
#[cfg(not(windows))]
mod screenshot;
#[cfg(not(windows))]
mod scroll;
#[cfg(test)]
mod capture_panel_tests;
#[cfg(test)]
mod settings_tests;
mod color;
mod doc;
#[cfg(windows)]
mod dragout;
#[allow(clippy::module_inception)]
mod gallery;
mod info;
mod library;
mod platform;
mod player;
mod pushing;
mod editor;
pub mod env;
mod search;
mod capture_panel;
mod settings_rows;
mod settings_window;
mod system_info;
mod theme;
pub(crate) mod words;

#[cfg(windows)]
pub use auto_push::start as start_auto_push;
pub use camera::run as run_camera;
pub use capture_panel::run as run_capture_panel;
pub use settings_window::run as run_settings;
#[cfg(not(windows))]
pub use background::{capture, quit as quit_background, record, run as run_background};
#[cfg(not(windows))]
pub use screenshot::{Capture, Mode as CaptureMode};
#[cfg(not(windows))]
pub use platform::runtime_dir;

/// `rsnip --pick-area <frame> [purpose]`: prints the area picked on the frame (see `pick_area`).
#[cfg(not(windows))]
pub fn pick_area(frame: &Path, purpose: Option<&str>) {
    if let Some(a) = pick_area::run(frame, pick_area::Purpose::parse(purpose)) {
        println!("{} {} {} {}", a.x, a.y, a.width, a.height);
    }
}

/// `rsnip --update`: checks for a newer version and installs it where RSnip can (a copy from the
/// .deb or .rpm, as Settings → Updates does), else says where to get it. Exits with 1 if that
/// failed.
#[cfg(not(windows))]
pub fn run_update() {
    let fail = |what: &str, e: String| {
        eprintln!("{what}: {e}");
        std::process::exit(1);
    };
    println!("Checking for updates…");
    let release = match update::check(crate::options::RELEASE_CANDIDATES.read(platform::setting)) {
        Ok(Some(release)) => release,
        Ok(None) => return println!("You have the latest version, {}.", update::Version::current()),
        Err(e) => return fail("Couldn't check", e),
    };
    println!("RSnip {} is available (you have {}).", release.version, update::Version::current());
    let Some(package) = platform::package() else {
        return println!("This copy wasn't installed from a .deb or .rpm package, so it can't update itself. Download the new version from {}", release.page);
    };
    println!("Installing it; you may be asked for your password…");
    match update::package::install(&release, package) {
        Ok(()) => println!("Installed. Open RSnip again to use the new version."),
        Err(e) => fail("Couldn't install the update", e),
    }
}

/// `rsnip --check-recording`: records a moment of a test picture as a recording would be made now
/// and says what it came out as, or what's missing (exiting with 1).
#[cfg(not(windows))]
pub fn check_recording() {
    match recorder::check(crate::options::RECORD_HEVC.read(platform::setting)) {
        Ok(made) => println!("{made}"),
        Err(e) => {
            eprintln!("Recording doesn't work: {e}");
            std::process::exit(1);
        }
    }
}
#[cfg(windows)]
pub use settings_window::TITLE as SETTINGS_TITLE;
#[cfg(windows)]
pub use capture_panel::TITLE as CAPTURE_PANEL_TITLE;

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
use editor::{Editor, TextStatus, Tool, ToolState, GROUP_STARTS, PALETTE, PALETTE_NAMES, TEXT_SIZES, TOOLS};
use env::Env;
use pushing::Pushing;
use theme::{icon, Appearance, IconButton};

pub const TITLE: &str = "RSnip Library";
/// What Linux desktops know the windows by; matches `StartupWMClass` in the menu entry that
/// installer/linux/install.sh writes.
const APP_ID: &str = "rsnip";
const THUMB: Vec2 = Vec2::new(200.0, 112.0);
const RESCAN_EVERY: Duration = Duration::from_secs(2);

pub fn run(file: Option<PathBuf>) {
    if !platform::claim_single_instance("Gallery", TITLE) {
        // The running library picks this up and opens the file.
        let request = file.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = std::fs::write(env::request_file(), request);
        return;
    }
    let _ = std::fs::remove_file(env::request_file());
    // Closing the library leaves RSnip in the background, where there's a tray icon for it.
    #[cfg(not(windows))]
    if crate::options::RUN_IN_BACKGROUND.read(platform::setting) {
        platform::start_tray();
    }
    // Here only while the tray app or background icon doesn't (see `auto_push`).
    #[cfg(not(windows))]
    auto_push::start(background::notify);
    #[cfg(windows)]
    auto_push::start(|_, _| {});

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(window_icon())
            .with_app_id(APP_ID),
        ..Default::default()
    };
    let _ = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, Env::real(), file)))));
}

/// A screenshot or recording the library lists.
fn is_library_file(path: &Path) -> bool {
    let ext = path.extension().map(|x| x.to_ascii_lowercase());
    matches!(ext.as_deref().and_then(|x| x.to_str()), Some("png" | "jpg" | "jpeg" | "gif" | "mp4" | "webm"))
}

/// The screenshots and recordings in `dir`, as the library lists them.
fn library_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_library_file(p) && p.is_file()).collect()
}

/// The same, with when each last changed (those that can't be read are left out).
fn library_files_with_times(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    library_files(dir).into_iter().filter_map(|f| Some((f.clone(), std::fs::metadata(&f).and_then(|m| m.modified()).ok()?))).collect()
}

/// Whether a listed file is a screenshot or recording by name (see `options::screenshot_name`),
/// rather than a picture kept in the same folder: all the clean-up and "Move them here" touch.
fn is_screenshot(path: &Path) -> bool {
    path.file_name().is_some_and(|name| crate::options::screenshot_name(&name.to_string_lossy()))
}

/// The screenshots and recordings in `dir` by name (see `is_screenshot`).
fn screenshot_files(dir: &Path) -> Vec<PathBuf> {
    library_files(dir).into_iter().filter(|p| is_screenshot(p)).collect()
}

/// Moves the screenshots and recordings in `from` to `to`, with their edits and search data (see
/// `doc::move_shot`); pictures of the user's own there stay. What moved before an error stays moved.
fn move_library(from: &Path, to: &Path) -> Result<env::MoveReport, String> {
    std::fs::create_dir_all(to).map_err(|e| format!("Can't create {}: {e}", to.display()))?;
    let mut report = env::MoveReport::default();
    for path in screenshot_files(from) {
        match doc::move_shot(&path, to)? {
            doc::Moved::Moved => report.moved += 1,
            doc::Moved::AlreadyThere => report.already_there += 1,
        }
    }
    Ok(report)
}

struct Entry {
    path: PathBuf,
    name: String,
    modified: SystemTime,
    /// The file's size in bytes.
    len: u64,
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

/// What the window shows under the toolbar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    /// The editor, with the library beside it.
    Editor,
    /// Everything as a grid of large thumbnails.
    Gallery,
}

impl Page {
    const SETTING: &'static str = "LibraryPage";

    /// Unknown values (or none) mean the default, the editor.
    fn from_setting(value: Option<u32>) -> Page {
        match value {
            Some(1) => Page::Gallery,
            _ => Page::Editor,
        }
    }

    fn to_setting(self) -> u32 {
        match self {
            Page::Editor => 0,
            Page::Gallery => 1,
        }
    }
}

struct App {
    env: Env,
    entries: Vec<Entry>,
    last_scan: Instant,
    filter: String,
    thumbs: HashMap<PathBuf, (SystemTime, TextureHandle)>,
    /// Thumbnails asked for, as of the file's modified time.
    requested: HashMap<PathBuf, SystemTime>,
    thumb_tx: Sender<(PathBuf, SystemTime)>,
    thumb_rx: Receiver<(PathBuf, SystemTime, Option<ColorImage>)>,
    editor: Option<Editor>,
    /// The recording open in the viewer, instead of an editor.
    player: Option<player::Player>,
    /// A recording to save a copy of, and the folder chooser while it's open.
    copying_to: Option<(PathBuf, env::PickedFolder)>,
    /// Files being pushed to a destination, chosen by hand (see `push_to`).
    pushing: Option<Pushing>,
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
    /// Start again when this window closes, into an update that was just installed.
    restart: bool,
    update_rx: Option<Receiver<update::Release>>,
    /// Library items selected for bulk actions.
    selected: HashSet<PathBuf>,
    /// Where Shift+click ranges start.
    anchor: Option<PathBuf>,
    dock: Dock,
    page: Page,
    /// How the gallery is ordered, and whether that's backwards (newest, largest or Z first).
    sort: gallery::Sort,
    descending: bool,
    /// A gallery item the keyboard moved the selection to, to scroll into view.
    reveal: Option<PathBuf>,
    /// What the gallery's info panel read from the file it shows (see `info`).
    details: Option<info::Details>,
    /// Dates folded in the library, by their heading (see `library::parse_folded`).
    collapsed: HashSet<String>,
    /// What's known about each listed screenshot for search (see `search`).
    metas: HashMap<PathBuf, search::Meta>,
    /// When each one's meta file last changed, so a change made elsewhere (the automatic pusher
    /// noting a link) is read again.
    meta_times: HashMap<PathBuf, Option<SystemTime>>,
    /// Screenshots waiting for their text to be read, and the reader's channels.
    reading: HashSet<PathBuf>,
    read_tx: Option<Sender<PathBuf>>,
    read_rx: Option<Receiver<(PathBuf, Option<doc::FileStamp>, String)>>,
    /// How wide the toolbar's actions were last frame, to decide whether they fit beside the tools.
    actions_width: f32,
    /// How much room the tool groups took last frame besides their buttons.
    tools_spacing: f32,
    /// How wide the Editor | Gallery buttons were last frame, with the space after them.
    page_width: f32,
    /// RSnip crashed since the user last heard about it (see `crash`): when.
    crashed: Option<u64>,
}

impl App {
    fn new(ctx: &egui::Context, env: Env, file: Option<PathBuf>) -> App {
        theme::apply(ctx, Appearance::from_setting(env.setting(Appearance::SETTING)));
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
                let size = (THUMB.x as u32 * 2, THUMB.y as u32 * 2);
                let thumb = match player::Kind::of(&path) {
                    // Recordings show their first frame.
                    Some(player::Kind::Mp4 | player::Kind::WebM) => player::first_frame(&path, size),
                    _ => image::open(&path).ok().map(|img| {
                        let t = img.thumbnail(size.0, size.1).to_rgba8();
                        ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw())
                    }),
                };
                if results.send((path, modified, thumb)).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });

        let sort = gallery::Sort::from_setting(env.setting(gallery::Sort::SETTING));
        let mut app = App {
            entries: Vec::new(),
            last_scan: Instant::now(),
            filter: String::new(),
            thumbs: HashMap::new(),
            requested: HashMap::new(),
            thumb_tx,
            thumb_rx,
            editor: None,
            player: None,
            copying_to: None,
            pushing: None,
            tools: ToolState::default(),
            font: font_bytes.and_then(|b| FontVec::try_from_vec(b).ok()),
            pending: None,
            allow_close: false,
            status: None,
            effects: load_effects(&env),
            ocr: None,
            newer: None,
            restart: false,
            update_rx: env.check_updates.then(|| check_for_update(ctx, crate::options::RELEASE_CANDIDATES.read(|n| env.setting(n)))),
            selected: HashSet::new(),
            anchor: None,
            dock: Dock::from_setting(env.setting(Dock::SETTING)),
            page: Page::from_setting(env.setting(Page::SETTING)),
            sort,
            descending: sort.descending(env.setting(gallery::Sort::DESCENDING)),
            reveal: None,
            details: None,
            collapsed: library::parse_folded(&env.text_setting(library::FOLDED_SETTING).unwrap_or_default()),
            metas: HashMap::new(),
            meta_times: HashMap::new(),
            reading: HashSet::new(),
            read_tx: None,
            read_rx: None,
            actions_width: 320.0,
            tools_spacing: 72.0,
            page_width: 150.0,
            crashed: crate::crash::unseen(&env.crash_dir, env.setting(crate::crash::SEEN_SETTING)).map(|c| c.time),
            env,
        };
        if let Some(ocr) = app.env.ocr.clone() {
            let (tx, rx) = start_reader(ocr, ctx.clone());
            app.read_tx = Some(tx);
            app.read_rx = Some(rx);
        }
        app.rescan();
        doc::prune_layers(&app.env.dir);
        match file {
            Some(file) => {
                app.open(&file);
                // Asked for by name: show it, whichever page was last used.
                app.page = Page::Editor;
            }
            // The gallery is for choosing, so nothing's opened until it's chosen.
            None if app.page == Page::Gallery => {}
            None => {
                if let Some(first) = app.entries.iter().find(|e| !e.video).map(|e| e.path.clone()) {
                    app.open(&first);
                }
            }
        }
        app
    }

    /// Switches between the editor and the gallery, and remembers it.
    fn set_page(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.env.set_setting(Page::SETTING, page.to_setting());
        }
    }

    fn rescan(&mut self) {
        self.last_scan = Instant::now();
        let Ok(dir) = std::fs::read_dir(&self.env.dir) else { return };
        let mut entries: Vec<Entry> = dir
            .flatten()
            .filter(|e| is_library_file(&e.path()))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let modified = meta.modified().ok()?;
                let name = e.path().file_stem()?.to_string_lossy().into_owned();
                let video = is_video(&e.path());
                Some(Entry { path: e.path(), name, modified, len: meta.len(), video })
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
        let listed: HashSet<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        let vanished = self.entries.iter().any(|e| !listed.contains(e.path.as_path()));
        if vanished {
            doc::prune_layers(&self.env.dir);
            // Deleted elsewhere, or cleaned up: not left open, where saving would bring it back,
            // unless it has edits that would be lost.
            if self.editor.as_ref().is_some_and(|e| !e.doc.path.exists() && !e.doc.dirty()) {
                self.editor = None;
            }
            if self.player.as_ref().is_some_and(|p| !p.path.exists()) {
                self.player = None;
            }
            self.selected.retain(|path| path.exists());
        }
        self.metas.retain(|path, _| listed.contains(path.as_path()));
        self.meta_times.retain(|path, _| listed.contains(path.as_path()));
        for e in &entries {
            let changed = search::meta_path(&e.path).and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
            if !self.metas.contains_key(&e.path) || self.meta_times.get(&e.path) != Some(&changed) {
                self.metas.insert(e.path.clone(), search::load(&e.path));
                self.meta_times.insert(e.path.clone(), changed);
            }
        }
        self.entries = entries;
        self.read_new_text();
    }

    /// Sends screenshots whose text hasn't been read (or changed since) to the reader, newest first.
    fn read_new_text(&mut self) {
        if self.env.setting(search::SETTING) == Some(0) {
            return;
        }
        let Some(tx) = &self.read_tx else { return };
        let unread: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| !e.video && !self.reading.contains(&e.path))
            .filter(|e| self.metas.get(&e.path).is_some_and(|m| search::needs_text(m, &e.path)))
            .map(|e| e.path.clone())
            .collect();
        for path in unread {
            if tx.send(path.clone()).is_ok() {
                self.reading.insert(path);
            }
        }
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    fn dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.doc.dirty())
    }

    /// Opens a file, asking first if the current one has unsaved edits.
    fn request_open(&mut self, path: PathBuf) {
        if self.editor.as_ref().is_some_and(|e| e.doc.path == path) || self.player.as_ref().is_some_and(|p| p.path == path) {
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
        if is_video(path) {
            self.editor = None;
            self.player = player::Player::open(path, &self.env);
            return;
        }
        match Doc::open(path) {
            Ok(doc) => {
                let mut editor = Editor::new(doc);
                editor.reader = self.env.read_words.clone();
                self.editor = Some(editor);
                self.player = None;
            }
            Err(e) => self.set_status(e),
        }
    }

    /// Saves `from`..`to` of the open recording as a new one beside it, and opens that.
    fn trim(&mut self, from: Duration, to: Duration) {
        let Some(src) = self.player.as_ref().map(|p| p.path.clone()) else { return };
        let dst = player::trimmed_path(&src);
        match player::trim(&src, &dst, from, to) {
            Ok((kept_from, kept_to)) => {
                self.rescan();
                self.selected = [dst.clone()].into();
                self.open(&dst);
                let name = dst.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let moved = if kept_from + Duration::from_millis(20) < from { ", from the keyframe before the start" } else { "" };
                self.set_status(format!("Saved {name}: {} long{moved}", player::clock(kept_to.saturating_sub(kept_from))));
            }
            Err(e) => self.set_status(format!("Couldn't trim it: {e}")),
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
        let path = if as_copy { copy_path(&editor.doc.path, "png") } else { png_path(&editor.doc.path) };
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
        // On Linux in this user's own folder (see `runtime_dir`), as /tmp is shared.
        #[cfg(not(windows))]
        let dir = platform::runtime_dir().join("copied");
        #[cfg(windows)]
        let dir = std::env::temp_dir().join("RSnip");
        let path = dir.join(name).with_extension("png");
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| img.save_with_format(&path, image::ImageFormat::Png).map_err(|e| e.to_string()))
            .and_then(|()| self.env.copy_files(&[path]));
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
        let Some(read) = self.env.read_words.clone() else { return };
        let Some(img) = self.rendered() else { return };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(read(&img).map(|w| words::text(&w)));
        });
        self.ocr = Some(rx);
        self.set_status("Reading text…");
    }

    /// Puts text read from the image on the clipboard.
    fn copy_words(&mut self, text: &str) {
        match self.env.copy_text(text) {
            Ok(()) if text.lines().count() == 1 => self.set_status("Copied 1 line of text"),
            Ok(()) => self.set_status(format!("Copied {} lines of text", text.lines().count())),
            Err(e) => self.set_status(e),
        }
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
            if self.player.as_ref().is_some_and(|p| &p.path == path) {
                self.player = None;
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
        self.poll_pushing();
        if let Some(picked) = self.copying_to.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
            let (src, _) = self.copying_to.take().unwrap();
            match picked {
                Ok(Some(dir)) => {
                    let to = free_name(&dir, &src);
                    match std::fs::copy(&src, &to) {
                        Ok(_) => self.set_status(format!("Saved a copy to {}", to.display())),
                        Err(e) => self.set_status(format!("Couldn't save a copy: {e}")),
                    }
                }
                Ok(None) => {}
                Err(e) => self.set_status(format!("Couldn't open the folder chooser: {e}")),
            }
        }
        while let Ok((path, modified, thumb)) = self.thumb_rx.try_recv() {
            // Files that can't be read stay "requested", so they're retried only once they change
            // (a recording caught while it was still being written), not every frame.
            if let Some(thumb) = thumb {
                self.requested.remove(&path);
                let tex = ctx.load_texture(path.to_string_lossy(), thumb, TextureOptions::LINEAR);
                self.thumbs.insert(path, (modified, tex));
            }
        }
        if let Some(rx) = &self.read_rx {
            for (path, stamp, text) in rx.try_iter() {
                self.reading.remove(&path);
                if self.metas.contains_key(&path) {
                    // Read again, not the copy kept here: pushing records itself in it too.
                    let mut meta = search::load(&path);
                    meta.text = Some(text);
                    meta.stamp = stamp;
                    let _ = search::save(&path, &meta);
                    self.metas.insert(path, meta);
                }
            }
        }
        if let Some(rx) = &self.ocr {
            if let Ok(result) = rx.try_recv() {
                self.ocr = None;
                let copied = result.and_then(|text| {
                    if text.trim().is_empty() {
                        return Err("No text found in this image".into());
                    }
                    Ok(text)
                });
                match copied {
                    Ok(text) => self.copy_words(&text),
                    Err(e) => self.set_status(e),
                }
            }
        }
        if let Some(release) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.newer = Some(release);
            self.update_rx = None;
        }
        if self.last_scan.elapsed() >= RESCAN_EVERY {
            // Settings may have changed in the Settings window, the folder too.
            self.dock = Dock::from_setting(self.env.setting(Dock::SETTING));
            self.effects = load_effects(&self.env);
            self.env.refresh_dir();
            #[cfg(not(windows))]
            self.env.bring_in_screenshots();
            self.rescan();
        }
        if let Ok(request) = std::fs::read_to_string(&self.env.request_file) {
            let _ = std::fs::remove_file(&self.env.request_file);
            match request.trim() {
                // Out of the way of a recording, on Linux (see `background`).
                "--hide" => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                // Into the update Settings installed, once any edits are saved (see `Env::restart_after_update`).
                "--restart" => {
                    self.restart = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                request => {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    self.rescan();
                    if !request.is_empty() && request != "--show" {
                        self.request_open(PathBuf::from(request));
                        self.page = Page::Editor;
                    }
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = self.editor.as_ref().is_some_and(|e| e.is_typing()) || ctx.egui_wants_keyboard_input();
        if typing || self.editor.is_none() || self.page != Page::Editor {
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
        let copying_text = self.tools.tool == Tool::CopyText;
        if copying_text && shortcut(Modifiers::COMMAND, Key::A) {
            self.editor.as_mut().unwrap().select_all_text();
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            // With the Copy text tool, the selected text; otherwise the image.
            match self.editor.as_ref().unwrap().selected_text().filter(|_| copying_text) {
                Some(text) => self.copy_words(&text),
                None => self.copy(),
            }
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

    fn toolbar(&mut self, ui: &mut egui::Ui, visible: &[usize]) {
        let enabled = self.editor.is_some();
        let width = ui.available_width();
        // The Editor | Gallery buttons, then undo and redo, on the left, the tools centred on the
        // window, and the actions on the right. The tools are named under their icons when
        // there's room, and move off centre when that's the only way to fit the actions beside
        // them; when it isn't, the actions go on a row of their own. (A row around the
        // right-to-left layout keeps it from taking the panel's whole height.)
        let history = self.page_width + 2.0 * (IconButton::new(icon::ARROW_CLOCKWISE, "Redo").size(ui).x + 8.0);
        let captioned = history + self.tools_spacing + tool_buttons_width(ui, true) <= width;
        let tools = self.tools_spacing + tool_buttons_width(ui, captioned);
        let room = width - tools - ACTIONS_GAP - self.actions_width;
        let fits = room >= history;
        let centred = (width - tools) / 2.0;
        let indent = if fits { centred.min(room) } else { centred };
        // All of the row's height from the start, so everything is centred on the same line.
        let row = Vec2::new(width, if captioned { 48.0 } else { 32.0 });
        if self.page == Page::Gallery {
            // What's counted or selected goes on a row of its own in a narrow window, as the
            // Editor page's actions do.
            let one_row = width >= gallery::ONE_ROW;
            ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                self.page_buttons(ui);
                self.gallery_bar(ui, visible, one_row);
            });
            if !one_row {
                ui.horizontal(|ui| self.count_or_bulk_bar(ui, visible));
            }
            return;
        }
        ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            self.page_buttons(ui);
            self.history(ui);
            ui.add_space((indent - history).max(0.0));
            let drawn = ui.add_enabled_ui(enabled, |ui| self.tool_buttons(ui, captioned)).response.rect.width();
            self.tools_spacing = drawn - tool_buttons_width(ui, captioned);
            if fits {
                self.actions_row(ui, enabled);
            }
        });
        if !fits {
            ui.horizontal(|ui| self.actions_row(ui, enabled));
        }
    }

    /// Editor | Gallery, at the left of the toolbar.
    fn page_buttons(&mut self, ui: &mut egui::Ui) {
        let left = ui.cursor().left();
        theme::group(ui, |ui| {
            for (page, label) in [(Page::Editor, "Editor"), (Page::Gallery, "Gallery")] {
                if ui.add(egui::Button::selectable(self.page == page, label)).clicked() {
                    self.set_page(page);
                }
            }
        });
        self.page_width = ui.cursor().left() - left;
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo) = self.editor.as_ref().map_or((false, false), |e| (e.doc.can_undo(), e.doc.can_redo()));
        if ui.add_enabled(can_undo, IconButton::new(icon::ARROW_COUNTER_CLOCKWISE, "Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
            let editor = self.editor.as_mut().unwrap();
            editor.finish_text(&self.tools);
            editor.doc.undo();
        }
        if ui.add_enabled(can_redo, IconButton::new(icon::ARROW_CLOCKWISE, "Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
            let editor = self.editor.as_mut().unwrap();
            editor.finish_text(&self.tools);
            editor.doc.redo();
        }
    }

    /// The tools as icon buttons, one rounded group per kind of tool.
    fn tool_buttons(&mut self, ui: &mut egui::Ui, captioned: bool) {
        for group in tool_groups() {
            theme::group_top(ui, |ui| {
                for (tool, label, _, help) in group {
                    let button = IconButton::new(theme::tool_icon(tool), label).selected(self.tools.tool == tool).captioned(captioned);
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

    /// Copy, Save and the Export menu, added right to left: for the screenshot in the editor, or
    /// what applies to the recording in the player.
    fn actions(&mut self, ui: &mut egui::Ui, enabled: bool) {
        if let Some(path) = self.player.as_ref().map(|p| p.path.clone()) {
            return self.recording_actions(ui, &path);
        }
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
    }

    /// A recording has nothing to save (Trim saves its own copy); it's copied as a file.
    fn recording_actions(&mut self, ui: &mut egui::Ui, path: &Path) {
        ui.menu_button(format!("{}  Export  {}", icon::EXPORT, icon::CARET_DOWN), |ui| {
            ui.set_min_width(200.0);
            if ui.add_enabled(self.copying_to.is_none(), egui::Button::new(theme::label(icon::FLOPPY_DISK, "Save copy to…"))).on_hover_text("Copy the recording to another folder").clicked() {
                let picking = self.env.pick_folder(ui.ctx(), "Save a copy of the recording to", &self.env.dir);
                self.copying_to = Some((path.to_owned(), picking));
                ui.close();
            }
            if ui.button(theme::label(icon::FOLDER_OPEN, "Show in folder")).clicked() {
                self.env.show_in_folder(path);
                ui.close();
            }
            self.push_menu(ui, &[path.to_owned()]);
        });
        let save = theme::primary_button(theme::label(icon::FLOPPY_DISK, "Save"));
        ui.add_enabled(false, save).on_disabled_hover_text("Recordings aren't changed here. Trim saves the part you keep as a new recording");
        let copy = ui.button(theme::label(icon::COPY, "Copy"));
        if copy.on_hover_text("Copy the recording as a file, to paste into a folder, chat or email").clicked() {
            match self.env.copy_files(&[path.to_owned()]) {
                Ok(()) => self.set_status("Copied as a file — paste it into a folder, chat or email"),
                Err(e) => self.set_status(format!("Couldn't copy: {e}")),
            }
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
                        ui.painter().circle_stroke(center, 9.5, Stroke::new(1.5f32, theme::palette(ui).muted));
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
                ui.label(RichText::new("Size").color(theme::palette(ui).muted));
                ui.add(egui::Slider::new(self.tools.text_size_mut(), TEXT_SIZES).step_by(1.0).fixed_decimals(0))
                    .on_hover_text("Text size. Or drag the handle on the selected text's corner.");
                ui.add_space(4.0);
                ui.checkbox(self.tools.shadow_mut(), "Shadow")
                    .on_hover_text("Drop shadow behind text, for readability on busy backgrounds");
            } else if self.tools.uses_width() {
                ui.label(RichText::new("Size").color(theme::palette(ui).muted));
                ui.add(egui::Slider::new(self.tools.width_mut(), 1.0..=24.0).step_by(1.0).fixed_decimals(0));
            } else if self.tools.tool == Tool::CopyText {
                self.copy_text_options(ui);
            } else if self.tools.tool == Tool::Redact {
                self.redact_options(ui);
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
            let redacted = self.editor.as_ref().filter(|_| self.tools.tool == Tool::Redact).and_then(|e| e.auto_redact_note());
            let hint = if selected {
                "Drag to move it, or drag its handles to resize it. Esc deselects it"
            } else if let Some(note) = redacted {
                note
            } else {
                TOOLS.iter().find(|t| t.0 == self.tools.tool).map_or("", |t| t.3)
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(hint).color(theme::palette(ui).muted)).truncate());
            });
        });
    }

    /// The Copy text tool's buttons, or why there's nothing to copy.
    fn copy_text_options(&mut self, ui: &mut egui::Ui) {
        let Some(editor) = &mut self.editor else { return };
        let muted = theme::palette(ui).muted;
        let (selected, any) = match editor.text_status() {
            TextStatus::Found { words, selected } if words > 0 => (selected > 0, true),
            TextStatus::Found { .. } => {
                ui.label(RichText::new("No text found in this image").color(muted));
                return;
            }
            // No spinner: it would redraw the window every frame, which is slow where there's no
            // graphics card to do it (a virtual machine), and the text is shown the moment it's read.
            TextStatus::Reading => {
                ui.label(RichText::new("Reading text…").color(muted));
                return;
            }
            TextStatus::Failed(e) => {
                ui.label(RichText::new(e).color(muted));
                return;
            }
            TextStatus::Unavailable => {
                ui.label(RichText::new("Copying text needs Tesseract: install the tesseract-ocr package, then reopen RSnip").color(muted));
                return;
            }
        };
        let copy = egui::Button::new(theme::label(icon::COPY, "Copy selection"));
        if ui.add_enabled(selected, copy).on_hover_text("Copy the selected text (Ctrl+C)").clicked() {
            let text = editor.selected_text().unwrap_or_default();
            self.copy_words(&text);
            return;
        }
        if ui.add_enabled(any, egui::Button::new("Copy all text")).clicked() {
            let text = editor.all_text().unwrap_or_default();
            self.copy_words(&text);
            return;
        }
        if ui.add_enabled(any, egui::Button::new("Select all")).on_hover_text("Ctrl+A").clicked() {
            editor.select_all_text();
        }
    }

    /// The Redact tool's Auto-redact button, where the system can read text.
    fn redact_options(&mut self, ui: &mut egui::Ui) {
        let Some(editor) = &mut self.editor else { return };
        if editor.reader.is_none() {
            return;
        }
        let auto = egui::Button::new(theme::label(icon::MAGIC_WAND, "Auto-redact"));
        let hover = "Cover every email address, IP address, API key or token, card number and phone number found in the text, in one step";
        if ui.add_enabled(!editor.auto_redacting(), auto).on_hover_text(hover).clicked() {
            editor.start_auto_redact();
        }
        // No spinner, as in `copy_text_options`.
        if editor.auto_redacting() {
            ui.label(RichText::new("Reading text…").color(theme::palette(ui).muted));
        }
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
            ui.painter().circle_stroke(center, 9.5, Stroke::new(1.5f32, theme::palette(ui).muted));
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
        let as_file = ui.button(theme::label(icon::PAPERCLIP, "Copy as file"));
        if as_file.on_hover_text("Paste it into a folder, chat or email as an attachment").clicked() {
            self.copy_as_file();
            ui.close();
        }
        if ui.button(theme::label(icon::FOLDER_OPEN, "Show in folder")).clicked() {
            if let Some(editor) = &self.editor {
                self.env.show_in_folder(&editor.doc.path);
            }
            ui.close();
        }
        if let Some(path) = self.editor.as_ref().map(|e| e.doc.path.clone()) {
            // What's pushed is the file, so edits go once they're saved.
            let dirty = self.dirty();
            ui.add_enabled_ui(!dirty, |ui| self.push_menu(ui, &[path])).response.on_disabled_hover_text("Save your edits first, so they go too");
        }
        if platform::EXTRAS || self.env.read_words.is_some() {
            ui.separator();
        }
        if self.env.read_words.is_some() {
            let reading = self.ocr.is_some();
            let copy_text = ui.add_enabled(!reading, egui::Button::new(theme::label(icon::TEXTBOX, "Copy text (OCR)")));
            if copy_text.on_hover_text("Recognise all the text in the image and copy it. To copy some of it, use the Copy text tool").clicked() {
                self.copy_text();
                ui.close();
            }
        }
        if platform::EXTRAS {
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
            let editing = self.page == Page::Editor;
            if let Some(editor) = self.editor.as_ref().filter(|_| editing) {
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
                if let Some(time) = self.crashed {
                    let link = ui.link(RichText::new("RSnip closed unexpectedly. Report it").color(Color32::from_rgb(255, 196, 90)));
                    if link.on_hover_text("Shows what happened in Settings → System info").clicked() {
                        self.env.set_setting(crate::crash::SEEN_SETTING, time as u32);
                        self.env.open_system_info();
                        self.crashed = None;
                    }
                }
                if let Some(release) = &self.newer {
                    let link = ui.link(format!("RSnip {} is available", release.version));
                    // Installed where this copy can (from the .deb or .rpm), else downloaded.
                    if self.env.can_install_update() {
                        if link.on_hover_text("Install it from Settings → Updates").clicked() {
                            self.env.open_update_settings();
                        }
                    } else if link.on_hover_text(&release.page).clicked() {
                        self.env.open(Path::new(&release.page));
                    }
                }
                let Some(editor) = self.editor.as_mut().filter(|_| editing) else { return };
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
        theme::set_appearance(ctx, Appearance::from_setting(self.env.setting(Appearance::SETTING)));
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
        } else if closing && self.restart {
            self.restart = false;
            #[cfg(not(windows))]
            platform::relaunch_when_closed();
        }
        if self.pending.is_none() {
            self.shortcuts(ctx);
        }

        let p = theme::palette(ctx);
        let bar = |fill| egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(10, 7)).stroke(egui::Stroke::new(1.0f32, p.border));
        let gallery = self.page == Page::Gallery;
        // What the gallery shows and counts; the library works out its own.
        let visible = if gallery { self.visible() } else { Vec::new() };
        egui::Panel::top("toolbar").frame(bar(p.panel)).show_separator_line(false).show(ui, |ui| self.toolbar(ui, &visible));
        if self.editor.is_some() && !gallery {
            egui::Panel::top("options").frame(bar(p.subpanel)).show_separator_line(false).show(ui, |ui| self.tool_options(ui));
        }
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| self.status_bar(ui));
        let library = egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::same(10));
        // Beside the gallery, if the window has the room.
        if gallery && ui.available_width() >= info::MIN_WINDOW {
            egui::Panel::right("info")
                .frame(library)
                .resizable(true)
                .default_size(280.0)
                .size_range(220.0..=420.0)
                .show(ui, |ui| self.info(ui));
        }
        match self.dock {
            _ if gallery => {}
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
        let canvas = egui::Frame::NONE.fill(p.canvas).inner_margin(egui::Margin::same(if gallery { 12 } else { 0 }));
        egui::CentralPanel::default().frame(canvas).show(ui, |ui| {
            if gallery {
                return self.gallery(ui, &visible);
            }
            match (&mut self.editor, &mut self.player) {
                (Some(editor), _) => editor.ui(ui, &mut self.tools, self.font.as_ref()),
                (None, Some(player)) => {
                    match player.ui(ui) {
                        Some(player::Request::Trim(from, to)) => self.trim(from, to),
                        Some(player::Request::Muted(muted)) => self.env.set_setting(crate::options::PLAYER_MUTED.name, muted as u32),
                        None => {}
                    }
                }
                (None, None) => {
                    ui.centered_and_justified(|ui| ui.label("Select a screenshot or recording from the library."));
                }
            }
        });
        self.unsaved_dialog(ctx);
    }
}

/// Reads the text in screenshots one at a time on a worker thread: paths in, (path, the file's
/// stamp when read, text) out.
#[allow(clippy::type_complexity)]
fn start_reader(ocr: env::Ocr, ctx: egui::Context) -> (Sender<PathBuf>, Receiver<(PathBuf, Option<doc::FileStamp>, String)>) {
    let (jobs, inbox) = channel::<PathBuf>();
    let (results, rx) = channel();
    std::thread::spawn(move || {
        for path in inbox {
            let stamp = doc::FileStamp::of(&path);
            // A failed read (e.g. no OCR language installed) counts as no text, so it isn't
            // retried until the file changes.
            let text = ocr(&path).unwrap_or_default();
            if results.send((path, stamp, text)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (jobs, rx)
}

/// Records where a capture came from (app and window title), for search. Called before its file
/// appears in the library (see `working_path`), so the library finds it there.
#[cfg(windows)]
pub fn record_source(path: &Path, app: &str, window: &str) {
    search::record_source(path, app, window);
}

/// Where the recording or screenshot that will be `path` is written until it's finished: under the
/// same name in the save folder's hidden `.rsnip`, so the library and the automatic pusher never
/// see it half-written. `move_into_place` moves it into place.
pub fn working_path(path: &Path) -> Result<PathBuf, String> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err("Bad file name".into()) };
    let dir = dir.join(".rsnip");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    platform::hide(&dir);
    Ok(dir.join(name))
}

/// Moves a finished file from its `working_path` to `path`, in the same folder.
pub fn move_into_place(working: &Path, path: &Path) -> Result<(), String> {
    // A virus scanner can hold a file just written for a moment.
    let mut tries = 0;
    loop {
        match std::fs::rename(working, path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => return Err(format!("Couldn't move the file into {}: {e}", path.parent().unwrap_or(path).display())),
        }
    }
}

/// RSnip's icon for a window's title bar and taskbar button.
fn window_icon() -> Arc<egui::IconData> {
    let size = 64;
    let bgra = crate::icon_art::render(size);
    let rgba = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    Arc::new(egui::IconData { rgba, width: size as u32, height: size as u32 })
}

/// `rsnip --system-info`: what Settings → System info shows, for a bug report when the window
/// won't open (it can't say what only the window knows: the graphics driver and display).
pub fn print_system_info() {
    let env = Env::real();
    print!("{}", system_info::text(&system_info::report(&env, &env.host(), None, None)));
}

/// Opens Settings on System info, for the tray app's crash notification.
#[cfg(windows)]
pub fn open_system_info() {
    platform::open_settings(true);
}

/// Saves RSnip's icon as a PNG, for the menu entry installer/linux/install.sh writes.
#[cfg(not(windows))]
pub fn write_icon(path: &Path) -> Result<(), String> {
    let size = 256;
    let rgba: Vec<u8> = crate::icon_art::render(size).chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    image::save_buffer(path, &rgba, size as u32, size as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
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

/// The least room between the tools and the actions.
const ACTIONS_GAP: f32 = 16.0;

/// How wide the tool buttons are, without the space around and between them.
fn tool_buttons_width(ui: &egui::Ui, captioned: bool) -> f32 {
    TOOLS.iter().map(|t| IconButton::new(theme::tool_icon(t.0), t.1).captioned(captioned).size(ui).x).sum()
}

/// Looks for a newer release in the background (see `Env::check_updates`: on Windows the tray app
/// does this, and can install it).
fn check_for_update(ctx: &egui::Context, release_candidates: bool) -> Receiver<update::Release> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if let Ok(Some(release)) = update::check(release_candidates) {
            let _ = tx.send(release);
            ctx.request_repaint();
        }
    });
    rx
}

/// Where saving the screenshot at `path` goes: over itself when it's a PNG; else (a JPG copy
/// opened again, say) a PNG beside it, "name.png" or else `copy_path`'s, as the editor saves PNGs
/// and a JPG holding PNG data wouldn't open anywhere. The JPG stays as it was.
fn png_path(path: &Path) -> PathBuf {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png")) {
        return path.to_owned();
    }
    let png = path.with_extension("png");
    if png.exists() { copy_path(path, "png") } else { png }
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

/// Where a copy of `file` goes in `dir`: under its own name, else "name (2).ext", and so on.
fn free_name(dir: &Path, file: &Path) -> PathBuf {
    let name = file.file_name().unwrap_or_default();
    let (stem, ext) = (file.file_stem().unwrap_or_default().to_string_lossy(), file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default());
    let mut candidate = dir.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

fn save_jpg(img: &image::RgbaImage, path: &Path) -> Result<(), String> {
    let rgb = image::DynamicImage::ImageRgba8(flatten(img)).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90).encode_image(&rgb).map_err(|e| e.to_string())
}

/// Screen recordings; they open in the player.
fn is_video(path: &Path) -> bool {
    player::Kind::of(path).is_some()
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

    /// Held by each test that renders on the GPU: lavapipe (the software GPU on Linux CI) can crash
    /// when many devices are created at once.
    #[cfg(target_os = "linux")]
    pub(crate) fn gpu() -> std::sync::MutexGuard<'static, ()> {
        static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An empty folder of its own for one test.
    pub(crate) fn sandbox_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recordings_appear_only_once_finished() {
        let dir = sandbox_dir("working-recording");
        let path = dir.join("Recording 2026-10-24 213000.mp4");
        let working = working_path(&path).unwrap();
        std::fs::write(&working, b"half").unwrap();
        assert!(library_files(&dir).is_empty(), "not listed, so not pushed, while it's written");
        move_into_place(&working, &path).unwrap();
        assert_eq!(library_files(&dir), std::slice::from_ref(&path));
        assert!(!working.exists());
        doc::prune_layers(&dir);
        assert!(path.exists());
    }

    #[test]
    fn tools_are_grouped_in_order() {
        let groups = tool_groups();
        let flat: Vec<Tool> = groups.iter().flatten().map(|t| t.0).collect();
        assert_eq!(flat, TOOLS.map(|t| t.0), "every tool once, in toolbar order");
        let starts: Vec<Tool> = groups.iter().skip(1).map(|g| g[0].0).collect();
        assert_eq!(starts, GROUP_STARTS);
        assert_eq!(groups[0].iter().map(|t| t.0).collect::<Vec<_>>(), [Tool::Select, Tool::CopyText]);
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
    fn reads_rendered_text_back() {
        use super::doc::{load_font, render, Shape, Snapshot};
        let Some(read) = super::platform::read_words() else {
            eprintln!("skipped: no text recognition here");
            return;
        };
        let base = image::RgbaImage::from_pixel(420, 80, image::Rgba([255, 255, 255, 255]));
        let text = Shape::Text {
            pos: egui::Pos2::new(12.0, 14.0),
            text: "Hello RSnip 123".into(),
            color: egui::Color32::BLACK,
            size: 40.0,
            shadow: false,
        };
        let img = render(&Snapshot { base: std::sync::Arc::new(base), shapes: vec![text] }, load_font().as_ref());
        match read(&img) {
            Ok(words) => {
                let read = super::words::text(&words);
                assert!(read.to_lowercase().contains("hello rsnip 123"), "read {read:?}");
                // Each word is where it was drawn.
                let hello = &words[0].rect;
                assert!(hello.left() >= 8.0 && hello.left() < 30.0 && hello.top() >= 10.0 && hello.bottom() <= 70.0, "{hello:?}");
                assert!(words[1].rect.left() > hello.right(), "{words:?}");
            }
            // Server images (like CI's) may not have an OCR language installed.
            Err(e) if e.starts_with("No text-recognition language") => eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
    }
}
