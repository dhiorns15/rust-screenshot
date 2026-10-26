//! The library on Linux and other Unix desktops. Screenshots come from the desktop's screenshot
//! portal (see `screenshot`), or from another tool (the desktop's own, Flameshot or grim) whose
//! results RSnip opens and annotates too.

use std::cell::RefCell;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

use image::RgbaImage;

use super::super::system_info::{self, Host};

/// Pinning and dragging files out are Windows-only.
pub const EXTRAS: bool = false;
const NOT_HERE: &str = "Not available on this system";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// True if this is the only window of its kind (held with a lock file until the process exits).
pub fn claim_single_instance(name: &str, _title: &str) -> bool {
    static LOCK: OnceLock<File> = OnceLock::new();
    let Ok(file) = File::options().create(true).truncate(false).write(true).open(lock_path(name)) else { return true };
    if file.try_lock().is_err() {
        return false;
    }
    let _ = LOCK.set(file);
    true
}

/// True if no other RSnip process holds the lock `name` (with no window, unlike
/// `claim_single_instance`); this one then holds it until it exits.
pub fn claim_lock(name: &str) -> bool {
    static HELD: std::sync::Mutex<Vec<File>> = std::sync::Mutex::new(Vec::new());
    let Ok(file) = File::options().create(true).truncate(false).write(true).open(lock_path(name)) else { return false };
    if file.try_lock().is_err() {
        return false;
    }
    HELD.lock().map(|mut held| held.push(file)).is_ok()
}

// There's no tray app here: no startup entry, global shortcuts or installer.
/// The autostart entry that starts RSnip's background icon when you sign in (freedesktop's
/// autostart, which GNOME, KDE and the rest follow).
fn autostart_file() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("autostart").join("rsnip.desktop")
}

/// RSnip starts in the background when you sign in.
pub fn start_with_windows() -> bool {
    autostart_file().is_file()
}

/// False if it couldn't be changed.
pub fn set_start_with_windows(on: bool) -> bool {
    let file = autostart_file();
    if !on {
        return std::fs::remove_file(&file).is_ok() || !file.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=RSnip\nComment=RSnip in the background, for screenshots and recordings\nExec={} --tray\nIcon=rsnip\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        desktop_quote(&exe.to_string_lossy())
    );
    file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) && std::fs::write(&file, entry).is_ok()
}

/// `arg` quoted for a desktop entry's `Exec` line.
fn desktop_quote(arg: &str) -> String {
    let mut out = String::from('"');
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The background icon is running (it holds its lock; see `background::run`).
pub fn tray_running() -> bool {
    instance_running("Tray")
}

/// Another process holds `claim_single_instance(name)`.
pub fn instance_running(name: &str) -> bool {
    File::options().write(true).open(lock_path(name)).is_ok_and(|f| f.try_lock().is_err())
}

fn lock_path(name: &str) -> PathBuf {
    let user = std::env::var("USER").unwrap_or_default();
    std::env::temp_dir().join(format!("rsnip-{}-{user}.lock", name.to_lowercase()))
}

pub fn tell_tray(request: super::super::env::TrayRequest) {
    if request == super::super::env::TrayRequest::Quit {
        super::super::background::quit();
    }
}

/// Starts the background icon, unless it's running already.
pub fn start_tray() {
    spawn_self(&["--tray"]);
}

/// Runs RSnip again with `args`, on its own, so it outlives this process and the terminal.
pub fn spawn_self(args: &[&str]) {
    use std::os::unix::process::CommandExt;
    if let Ok(exe) = std::env::current_exe() {
        reap(Command::new(exe).args(args).process_group(0).spawn());
    }
}

/// Opens the Settings window, on System info if `system_info`.
pub fn open_settings(system_info: bool) {
    if let Ok(exe) = std::env::current_exe() {
        let args: &[&str] = if system_info { &["--settings", "--system-info"] } else { &["--settings"] };
        reap(Command::new(exe).args(args).spawn());
    }
}

/// Waits for a process RSnip started, in the background, so it doesn't stay a zombie after it ends
/// while RSnip runs on (which `relaunch_when_closed` would take for still running).
fn reap(child: std::io::Result<std::process::Child>) {
    if let Ok(mut child) = child {
        std::thread::spawn(move || child.wait());
    }
}

pub fn snipping_key() -> bool {
    false
}

pub fn installed() -> bool {
    false
}

/// Where the .deb and .rpm install RSnip (see installer/linux/build-packages.sh).
const PACKAGED: &str = "/usr/bin/rsnip";

/// The package this copy came from, if it's one (so updates can be installed the same way).
pub fn package() -> Option<Package> {
    // After an update replaces it, the old program reads as "/usr/bin/rsnip (deleted)".
    let program = std::env::current_exe().ok()?;
    if !program.to_string_lossy().starts_with(PACKAGED) {
        return None;
    }
    Some(if Path::new("/var/lib/dpkg/info/rsnip.list").exists() { Package::Deb } else { Package::Rpm })
}

/// Starts the library again once this process has ended: the newly installed program, after an
/// update.
pub fn relaunch_when_closed() {
    use std::os::unix::process::CommandExt;
    let wait = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2""#;
    let _ = Command::new("sh").args(["-c", wait, "sh", &std::process::id().to_string()]).arg(updated_program()).process_group(0).spawn();
}

/// Starts the library from the newly installed program, after an update (from Settings, when the
/// library isn't open: it needn't wait for Settings to close).
pub fn start_updated_library() {
    use std::os::unix::process::CommandExt;
    reap(Command::new(updated_program()).process_group(0).spawn());
}

/// The program an update installed. This copy's own path reads as "/usr/bin/rsnip (deleted)" once
/// the package manager has replaced it.
fn updated_program() -> PathBuf {
    if package().is_some() { PathBuf::from(PACKAGED) } else { std::env::current_exe().unwrap_or_else(|_| PACKAGED.into()) }
}

pub fn shortcut_name(key: u32) -> String {
    crate::shortcuts::fallback_name(key)
}

pub use crate::update::package::Package;

/// Where screenshots and recordings are saved: the folder chosen in Settings, else `~/RSnip` (see
/// `options::save_folder`).
pub fn screenshots_dir() -> PathBuf {
    let chosen = text_setting(crate::options::SAVE_FOLDER);
    crate::options::save_folder(chosen, legacy_save_folder(), &home(), |dir| set_text_setting(crate::options::SAVE_FOLDER, &dir.to_string_lossy()))
}

/// `~/Pictures/Screenshots`, or wherever the desktop's Pictures folder is: where RSnip saved before
/// there was a choice.
/// It's also where the desktop's screenshot tool saves (see `bring_in`). Asked once, as the
/// folder is looked up every frame in Settings.
pub fn legacy_save_folder() -> PathBuf {
    static FOLDER: OnceLock<PathBuf> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let pictures = Command::new("xdg-user-dir")
                .arg("PICTURES")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| PathBuf::from(s.trim()))
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home().join("Pictures"));
            pictures.join("Screenshots")
        })
        .clone()
}

fn settings_path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("rsnip").join("settings.json")
}

fn settings() -> Settings {
    std::fs::read(settings_path()).map(|b| parse_settings(&b)).unwrap_or_default()
}

fn save_settings(all: &Settings) {
    let path = settings_path();
    if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec_pretty(all)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, json);
    }
}

/// Settings: numbers, and some text (a folder, say).
type Settings = serde_json::Map<String, serde_json::Value>;

/// The settings in a `settings.json`; anything unreadable means defaults.
fn parse_settings(bytes: &[u8]) -> Settings {
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// A number setting, if it's set to one.
fn number(all: &Settings, name: &str) -> Option<u32> {
    all.get(name)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_files_for_the_file_manager() {
        let path = Path::new("/home/a/RSnip/Recording 2026-10-24 091500 (trimmed)#1.mp4");
        assert_eq!(file_uri(path), "file:///home/a/RSnip/Recording%202026-10-24%20091500%20%28trimmed%29%231.mp4");
        assert_eq!(file_uri(Path::new("/tmp/café.png")), "file:///tmp/caf%C3%A9.png");
    }

    #[test]
    fn quotes_paths_for_the_autostart_entry() {
        assert_eq!(desktop_quote("/home/a b/.local/bin/rsnip"), r#""/home/a b/.local/bin/rsnip""#);
        assert_eq!(desktop_quote(r#"/x/$y"z\w"#), r#""/x/\$y\"z\\w""#);
    }

    #[test]
    fn damaged_settings_mean_defaults() {
        let all = parse_settings(br#"{"LibraryDock": 1, "CheckUpdates": 0, "SaveFolder": "/home/a/RSnip"}"#);
        assert_eq!((number(&all, "LibraryDock"), number(&all, "CheckUpdates")), (Some(1), Some(0)));
        assert_eq!(all["SaveFolder"].as_str(), Some("/home/a/RSnip"), "text settings sit beside them");
        for junk in [&b""[..], b"{", b"null", b"[1,2]", b"\xff\xfe"] {
            assert!(parse_settings(junk).is_empty(), "{junk:?}");
        }
        // A value of the wrong kind is unset, without losing the rest.
        let all = parse_settings(br#"{"LibraryDock": -1, "Notify": "bottom", "Theme": 4294967296, "CheckUpdates": 0}"#);
        assert_eq!(["LibraryDock", "Notify", "Theme", "CheckUpdates"].map(|n| number(&all, n)), [None, None, None, Some(0)]);
    }
}

pub fn setting(name: &str) -> Option<u32> {
    number(&settings(), name)
}

pub fn set_setting(name: &str, value: u32) {
    let mut all = settings();
    all.insert(name.to_owned(), value.into());
    save_settings(&all);
}

/// A text setting, like the folder screenshots are saved to; None if it's missing or empty.
pub fn text_setting(name: &str) -> Option<String> {
    settings().get(name)?.as_str().map(str::trim).filter(|t| !t.is_empty()).map(String::from)
}

pub fn set_text_setting(name: &str, value: &str) {
    let mut all = settings();
    all.insert(name.to_owned(), value.into());
    save_settings(&all);
}

thread_local! {
    // On X11 and Wayland the copied data is served by this process, so the clipboard stays open.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

fn with_clipboard(f: impl FnOnce(&mut arboard::Clipboard) -> Result<(), arboard::Error>) -> Result<(), String> {
    CLIPBOARD.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        f(cell.as_mut().unwrap()).map_err(|e| e.to_string())
    })
}

pub fn copy_image(img: &RgbaImage) -> Result<(), String> {
    let data = arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes: img.as_raw().into() };
    with_clipboard(|c| c.set_image(data))
}

pub fn copy_text(text: &str) -> Result<(), String> {
    with_clipboard(|c| c.set_text(text))
}

/// Puts files on the clipboard, to paste into a folder, chat or email: as GNOME's Files does where
/// that reaches the apps, else as `text/uri-list`.
pub fn copy_files(paths: &[PathBuf]) -> Result<(), String> {
    if super::super::file_clipboard::usable() && super::super::file_clipboard::copy(paths).is_ok() {
        return Ok(());
    }
    with_clipboard(|c| c.set().file_list(paths))
}

pub fn open(path: &Path) {
    reap(Command::new("xdg-open").arg(path).spawn());
}

/// Opens the file manager on `path`'s folder with it selected, where the file manager can (GNOME's
/// Files, Dolphin, Nemo and Caja answer `org.freedesktop.FileManager1`), else just the folder.
pub fn show_in_folder(path: &Path) {
    let (path, folder) = (path.to_owned(), path.parent().unwrap_or(path).to_owned());
    std::thread::spawn(move || {
        let shown = zbus::blocking::Connection::session().is_ok_and(|conn| {
            let (uris, startup_id) = (vec![file_uri(&path)], "");
            conn.call_method(Some("org.freedesktop.FileManager1"), "/org/freedesktop/FileManager1", Some("org.freedesktop.FileManager1"), "ShowItems", &(uris, startup_id)).is_ok()
        });
        if !shown {
            open(&folder);
        }
    });
}

/// `path` as a file:// URI, with anything but letters, digits and `/-._~` percent-encoded.
pub(in crate::gallery) fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The desktop's folder chooser, starting in `current`; `None` if it was cancelled.
pub fn pick_folder(title: &str, current: &Path) -> Result<Option<PathBuf>, String> {
    super::super::portal::pick_folder(title, current)
}

/// The desktop's file chooser, offering the files matching `filter` (a name and a pattern).
pub fn pick_file(title: &str, filter: (&str, &str)) -> Result<Option<PathBuf>, String> {
    super::super::portal::pick_file(title, filter)
}

/// What deleted files go to, for Settings.
pub const RECYCLE_BIN: &str = "Trash";

/// Whether files in `dir` would go to the Trash. Where they can't, moving them fails and they stay.
pub fn has_recycle_bin(_dir: &Path) -> bool {
    true
}

/// Moves a file to the Trash.
pub fn recycle(path: &Path) -> bool {
    trash::delete(path).is_ok()
}

/// A folder starting with a dot is already hidden.
pub fn hide(_dir: &Path) {}

/// The font for text annotations.
pub fn font_bytes() -> Option<Vec<u8>> {
    std::fs::read(font_path()?).ok()
}

/// The desktop's sans-serif, or a common one.
fn font_path() -> Option<PathBuf> {
    let matched = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif:semibold"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|f| f.ends_with(".ttf") || f.ends_with(".otf"));
    let common = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    matched.into_iter().chain(common.map(String::from)).map(PathBuf::from).find(|f| f.is_file())
}

/// The programs RSnip runs: opening files and links, finding Pictures and the annotation font,
/// checking for updates (curl or wget), and pushing over SFTP and SMB.
const HELPERS: [&str; 9] = ["xdg-open", "xdg-user-dir", "fc-match", "curl", "wget", "tesseract", "gst-launch-1.0", "sftp", "smbclient"];

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// What System info reports about this machine, from `/etc/os-release`, `/proc` and the session.
pub fn host() -> Host {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let os = read("/etc/os-release").or_else(|| read("/usr/lib/os-release")).and_then(|t| system_info::os_release_name(&t));
    let desktop = match (var("XDG_CURRENT_DESKTOP"), var("XDG_SESSION_TYPE")) {
        (Some(d), Some(t)) => Some(format!("{d}, {t}")),
        (d, t) => d.or(t),
    };
    let meminfo = read("/proc/meminfo").unwrap_or_default();
    let home = home();
    let install = match package() {
        Some(Package::Deb) => ".deb package",
        Some(Package::Rpm) => ".rpm package",
        // installer/linux/install.sh copies it here.
        None if std::env::current_exe().is_ok_and(|p| p.parent() == Some(home.join(".local/bin").as_path())) => "install.sh",
        None => "Standalone",
    };
    Host {
        os: os.unwrap_or_else(|| std::env::consts::OS.to_string()),
        kernel: read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_string()).filter(|k| !k.is_empty()),
        desktop,
        cpu: read("/proc/cpuinfo").and_then(|t| system_info::cpuinfo_model(&t)),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory: system_info::meminfo(&meminfo, "MemTotal"),
        memory_available: system_info::meminfo(&meminfo, "MemAvailable"),
        install: install.into(),
        program: std::env::current_exe().ok(),
        settings: system_info::tidy_path(&settings_path(), Some(&home)),
        font: font_path(),
        helpers: HELPERS.map(|h| (h, on_path(h))).to_vec(),
        home: Some(home),
    }
}

/// A new file in `dir` named like Windows' captures: "Recording 2026-10-23 181500.mp4", with
/// " (2)" and so on if that's taken.
pub fn new_path(dir: &Path, kind: &str, ext: &str) -> PathBuf {
    let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let base = format!("{kind} {:04}-{:02}-{:02} {:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    path
}

/// The local calendar date of `t` as (year, month, day).
pub fn local_date(t: SystemTime) -> (i32, u32, u32) {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

pub fn pin(_img: &RgbaImage) -> Result<(), String> {
    Err(NOT_HERE.into())
}

pub fn pin_file(_path: &Path) {}

/// Why H.265 can't be chosen.
pub const HEVC_MISSING: &str = "H.265 needs GStreamer's x265 encoder (gstreamer1.0-plugins-bad on Debian and Ubuntu, gst-plugins-bad on Arch)";

/// What's missing for recording, if anything (see `recorder::problem`).
pub fn recording_problem(hevc: bool) -> Option<String> {
    super::super::recorder::problem(hevc)
}

/// Whether recordings can be H.265 (an encoder for it is installed in GStreamer).
pub fn hevc_available() -> bool {
    super::super::recorder::hevc_available()
}

/// What reads the text in screenshots, for Settings.
pub const TEXT_RECOGNITION: &str = "Tesseract";

/// Tesseract's text recognition on an image file, for search, if it's installed. It reads the
/// whole library in the background, so at the lowest priority.
pub fn ocr() -> Option<super::super::env::Ocr> {
    on_path("tesseract").then(|| {
        std::sync::Arc::new(|path: &Path| {
            let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
            tesseract(&img, true).map(|words| super::super::words::text(&words))
        }) as super::super::env::Ocr
    })
}

/// Tesseract's text recognition word by word, for copying text out of an image, if it's installed
/// (the `tesseract-ocr` package, or `tesseract` on some distributions).
pub fn read_words() -> Option<super::super::env::ReadWords> {
    on_path("tesseract").then(|| std::sync::Arc::new(|img: &RgbaImage| tesseract(img, false)) as super::super::env::ReadWords)
}

fn tesseract(img: &RgbaImage, background: bool) -> Result<Vec<super::super::words::Word>, String> {
    // It reads small UI text far better enlarged; the boxes are scaled back afterwards.
    let scale = if img.width().max(img.height()) <= 2000 { 2 } else { 1 };
    let big;
    let img = if scale == 1 {
        img
    } else {
        big = image::imageops::resize(img, img.width() * scale, img.height() * scale, image::imageops::FilterType::CatmullRom);
        &big
    };
    let mut png = Vec::new();
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let mut command = Command::new("tesseract");
    // On one core it's several times faster: on several, its threads mostly spin waiting for each
    // other, worst on a virtual machine (0.15 s rather than 0.58 s, on all four of one's cores).
    command.env("OMP_THREAD_LIMIT", "1");
    if background {
        use std::os::unix::process::CommandExt;
        // SAFETY: only setpriority, which is async-signal-safe, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 19);
                Ok(())
            });
        }
    }
    let out = command
        .args(["stdin", "-", "tsv"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            // It reads the whole image before writing anything, so this can't block on its output.
            child.stdin.take().expect("piped").write_all(&png)?;
            child.wait_with_output()
        });
    let out = out.map_err(|e| format!("Couldn't run Tesseract: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Tesseract couldn't read the text: {}", why.lines().last().unwrap_or("").trim()));
    }
    Ok(super::super::words::from_tesseract_tsv(&String::from_utf8_lossy(&out.stdout), 1.0 / scale as f32))
}

pub fn drag_files(_paths: &[PathBuf]) -> Result<(), String> {
    Err(NOT_HERE.into())
}
