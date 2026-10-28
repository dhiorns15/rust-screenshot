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

/// Where another launch leaves its request for the window holding `claim_single_instance(name)`
/// (next to the lock, so it's this user's own).
pub fn request_path(name: &str) -> PathBuf {
    lock_path(name).with_extension("request")
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
    let exe = program();
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
    runtime_dir().join(format!("{}.lock", name.to_lowercase()))
}

/// The folder for RSnip's lock files, the requests its processes leave each other, and passing
/// files: `rsnip` in the session's runtime folder (`$XDG_RUNTIME_DIR`, else `/run/user/<uid>`),
/// which is this user's alone. In a shared folder like /tmp, another user could leave a request
/// (to quit, say) that this one can't remove. Without a runtime folder, a folder of its own in
/// the temp folder, or in `~/.cache` if another user has that name.
pub fn runtime_dir() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    let absolute = |var| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    let runtime = absolute("XDG_RUNTIME_DIR").unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    first_private(&[runtime.join("rsnip"), std::env::temp_dir().join(format!("rsnip-{uid}"))]).unwrap_or_else(|| {
        let cache = absolute("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"));
        let _ = std::fs::create_dir_all(&cache);
        let dir = cache.join("rsnip-run");
        let _ = private_dir(&dir);
        dir
    })
}

/// The first of `dirs` that is, or can be made, a private folder (see `private_dir`).
fn first_private(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find(|dir| private_dir(dir).is_ok()).cloned()
}

/// Makes `dir` (its parent must be there) a folder only this user can get into, unless it's
/// something else already: a link, a file, or another user's folder.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(std::io::Error::other(format!("{} isn't a folder of this user's", dir.display())));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
    reap(Command::new(program()).args(args).process_group(0).spawn());
}

/// Opens the Settings window, on System info if `system_info`.
pub fn open_settings(system_info: bool) {
    let args: &[&str] = if system_info { &["--settings", "--system-info"] } else { &["--settings"] };
    reap(Command::new(program()).args(args).spawn());
}

/// Opens the Settings window on Recording.
pub fn open_recording_settings() {
    reap(Command::new(program()).args(["--settings", "--recording"]).spawn());
}

/// Opens the Settings window on Updates, checking for one.
pub fn open_update_settings() {
    reap(Command::new(program()).args(["--settings", "--updates"]).spawn());
}

/// Opens the library window.
pub fn open_library() {
    spawn_self(&[]);
}

/// Runs a capture or recording (`--region`, `--record`, ...), which the icon's process does if
/// there is one.
pub fn start_capture(flag: &str) {
    spawn_self(&[flag]);
}

/// Opens the capture panel.
pub fn open_capture_panel() {
    reap(Command::new(program()).arg("--capture-panel").spawn());
}

/// The desktop places the capture panel (there's no telling where the icon is, and Wayland doesn't
/// let windows place themselves).
pub fn flyout_anchor(_at: Option<(i32, i32)>, _moved_to: Option<(i32, i32)>) -> Option<super::super::capture_panel::Anchor> {
    None
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
    let _ = Command::new("sh").args(["-c", wait, "sh", &std::process::id().to_string()]).arg(program()).process_group(0).spawn();
}

/// Starts the library from the newly installed program, after an update (from Settings, when the
/// library isn't open: it needn't wait for Settings to close).
pub fn start_updated_library() {
    use std::os::unix::process::CommandExt;
    reap(Command::new(program()).process_group(0).spawn());
}

/// The program to run RSnip again with: this one, or what an update put in its place.
pub fn program() -> PathBuf {
    std::env::current_exe().map_or_else(|_| PACKAGED.into(), installed_program)
}

/// The program at `exe`'s path now. Once the package manager (or install.sh) has replaced it, this
/// copy's own path reads as "/usr/bin/rsnip (deleted)", which can't be run.
fn installed_program(exe: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    match exe.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
        Some(path) => PathBuf::from(std::ffi::OsStr::from_bytes(path)),
        None => exe,
    }
}

/// How long `gsettings` has to answer: a stuck settings service shouldn't hold up Settings longer.
const GSETTINGS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Runs `gsettings` (GNOME's settings, for its keyboard shortcuts) with `args`: what it printed,
/// or why it failed.
pub fn gsettings(args: &[&str]) -> Result<String, String> {
    run_gsettings(Command::new("gsettings"), args)
}

/// `gsettings`, as `command` (tests give it settings of their own).
pub fn run_gsettings(mut command: Command, args: &[&str]) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "gsettings isn't installed".to_owned(),
        _ => format!("Couldn't run gsettings: {e}"),
    })?;
    // Read as it comes, so a long answer can't fill the pipe and stop it finishing.
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut out);
            }
            String::from_utf8_lossy(&out).into_owned()
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = std::time::Instant::now() + GSETTINGS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("gsettings didn't answer".to_owned());
            }
        }
    };
    let (out, err) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    match status.success() {
        true => Ok(out),
        false if err.trim().is_empty() => Err(format!("gsettings failed ({status})")),
        false => Err(err.trim().to_owned()),
    }
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

/// The settings, for reading one; anything unreadable means defaults.
fn settings() -> Settings {
    read_settings(&settings_path(), 1).unwrap_or_default()
}

/// Changes the settings: they're read, changed and written back whole. The tray, the library,
/// Settings, the capture panel and the describer all do this, so a damaged file is never written
/// over as if it were empty, and one change never undoes another (see `change_settings_in`).
fn change_settings(change: impl FnOnce(&mut Settings)) {
    if let Err(e) = change_settings_in(&settings_path(), change) {
        eprintln!("RSnip couldn't save its settings: {e}");
    }
}

/// How many times a writer reads the settings before taking them for damaged.
const READ_TRIES: u32 = 5;

/// How long a writer waits for another to finish changing the settings before going ahead anyway:
/// a lock left stuck shouldn't freeze a window.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Changes the settings in `path`, holding `lock_settings` while they're read, changed and written,
/// so two processes changing different settings at once keep both. If they still can't be read
/// after a few tries, the file is kept as `settings.json.bak` before new settings are started; if
/// it can't be kept, nothing is written.
fn change_settings_in(path: &Path, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let _lock = lock_settings(path, LOCK_WAIT);
    let mut all = match read_settings(path, READ_TRIES) {
        Ok(all) => all,
        Err(why) => {
            let backup = path.with_extension("json.bak");
            std::fs::copy(path, &backup).map_err(|e| format!("{} can't be read ({why}), nor kept aside ({e})", path.display()))?;
            eprintln!("RSnip's settings couldn't be read ({why}): they're kept in {}, and new ones started", backup.display());
            Settings::new()
        }
    };
    change(&mut all);
    save_settings(path, &all).map_err(|e| e.to_string())
}

/// The lock writers of the settings in `path` take turns with: `settings.json.lock` beside it (not
/// the settings file, which each write replaces), held until the file returned is dropped. None if
/// it can't be had within `wait`, and the change goes ahead without it. Readers needn't take it, as
/// they only ever see a whole file (see `save_settings`).
///
/// It's waited for on a thread of its own, so the system hands it on the moment it's let go (trying
/// now and then, a busy writer could take it back each time). If the wait runs out, that thread lets
/// it go as soon as it has it.
fn lock_settings(path: &Path, wait: std::time::Duration) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let (dir, name) = (path.parent()?, path.file_name()?);
    let lock = dir.join(format!("{}.lock", name.to_string_lossy()));
    let _ = std::fs::create_dir_all(dir);
    let file = match File::options().create(true).truncate(false).write(true).mode(0o600).open(&lock) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("RSnip couldn't open {} ({e}): changing its settings without it", lock.display());
            return None;
        }
    };
    if file.try_lock().is_ok() {
        return Some(file);
    }
    let (send, locked) = std::sync::mpsc::channel();
    std::thread::spawn(move || send.send(file.lock().map(|()| file)));
    match locked.recv_timeout(wait) {
        Ok(Ok(file)) => Some(file),
        Ok(Err(e)) => {
            eprintln!("RSnip couldn't lock {} ({e}): changing its settings without it", lock.display());
            None
        }
        Err(_) => {
            eprintln!("RSnip's settings were still being changed elsewhere after {wait:?} ({} is held): changing them anyway", lock.display());
            None
        }
    }
}

/// The settings in `path`, none if there's no file yet, else why they can't be read after `tries`
/// tries: an older RSnip, which wrote the file in place, may be caught half way through.
fn read_settings(path: &Path, tries: u32) -> Result<Settings, String> {
    let mut tried = 0;
    loop {
        let why = match std::fs::read(path) {
            Ok(bytes) => match parse_settings(&bytes) {
                Some(all) => return Ok(all),
                None => "it isn't settings".to_owned(),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::new()),
            Err(e) => e.to_string(),
        };
        tried += 1;
        if tried >= tries {
            return Err(why);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Writes `all` to a new file beside `path`, then renames it over `path`, so another process never
/// reads it half-written. Only this user can read it, as it has the destinations' details.
fn save_settings(path: &Path, all: &Settings) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Err(std::io::ErrorKind::InvalidInput.into()) };
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(all)?;
    let temp = dir.join(format!(".{}.{}-{}", name.to_string_lossy(), std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    // One left by a process that crashed with this one's number.
    let _ = std::fs::remove_file(&temp);
    let written = File::options().write(true).create_new(true).mode(0o600).open(&temp).and_then(|mut file| {
        file.write_all(&json)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Settings: numbers, and some text (a folder, say).
type Settings = serde_json::Map<String, serde_json::Value>;

/// The settings in a `settings.json`; None if it isn't settings (damaged, or empty).
fn parse_settings(bytes: &[u8]) -> Option<Settings> {
    serde_json::from_slice(bytes).ok()
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
    fn runs_the_installed_program_after_an_update_replaced_this_one() {
        assert_eq!(installed_program("/usr/bin/rsnip (deleted)".into()), Path::new("/usr/bin/rsnip"));
        assert_eq!(installed_program("/home/a/.local/bin/rsnip (deleted)".into()), Path::new("/home/a/.local/bin/rsnip"));
        // Anything else is run as it is, like a build in target/.
        for exe in ["/usr/bin/rsnip", "/home/a/rust-screenshot/target/release/rsnip", "/home/a/(deleted) x/rsnip"] {
            assert_eq!(installed_program(exe.into()), Path::new(exe));
        }
    }

    #[test]
    fn keeps_requests_next_to_the_lock() {
        let (lock, request) = (lock_path("Settings"), request_path("Settings"));
        assert_eq!((request.parent(), request.file_stem()), (lock.parent(), lock.file_stem()));
        assert_ne!(request, lock);
    }

    #[test]
    fn quotes_paths_for_the_autostart_entry() {
        assert_eq!(desktop_quote("/home/a b/.local/bin/rsnip"), r#""/home/a b/.local/bin/rsnip""#);
        assert_eq!(desktop_quote(r#"/x/$y"z\w"#), r#""/x/\$y\"z\\w""#);
    }

    #[test]
    fn damaged_settings_mean_defaults() {
        let all = parse_settings(br#"{"LibraryDock": 1, "CheckUpdates": 0, "SaveFolder": "/home/a/RSnip"}"#).unwrap();
        assert_eq!((number(&all, "LibraryDock"), number(&all, "CheckUpdates")), (Some(1), Some(0)));
        assert_eq!(all["SaveFolder"].as_str(), Some("/home/a/RSnip"), "text settings sit beside them");
        for junk in [&b""[..], b"{", b"null", b"[1,2]", b"\xff\xfe"] {
            assert!(parse_settings(junk).is_none(), "{junk:?}");
        }
        // A value of the wrong kind is unset, without losing the rest.
        let all = parse_settings(br#"{"LibraryDock": -1, "Notify": "bottom", "Theme": 4294967296, "CheckUpdates": 0}"#).unwrap();
        assert_eq!(["LibraryDock", "Notify", "Theme", "CheckUpdates"].map(|n| number(&all, n)), [None, None, None, Some(0)]);
    }

    /// A change that sets `name` to `value`.
    fn set(name: &str, value: impl Into<serde_json::Value>) -> impl FnOnce(&mut Settings) {
        let (name, value) = (name.to_owned(), value.into());
        move |all| {
            all.insert(name, value);
        }
    }

    #[test]
    fn settings_are_replaced_whole_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::gallery::tests::sandbox_dir("settings-write");
        let path = dir.join("rsnip").join("settings.json");
        assert_eq!(read_settings(&path, 1), Ok(Settings::new()), "none yet");
        change_settings_in(&path, set("LibraryDock", 1)).unwrap();
        change_settings_in(&path, set("SaveFolder", "/home/a/RSnip")).unwrap();
        let all = read_settings(&path, 1).unwrap();
        assert_eq!((number(&all, "LibraryDock"), all["SaveFolder"].as_str()), (Some(1), Some("/home/a/RSnip")));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let mut left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        left.sort();
        assert_eq!(left, ["settings.json", "settings.json.lock"], "no new files left beside it but the lock");
        assert_eq!(std::fs::metadata(path.with_extension("json.lock")).unwrap().permissions().mode() & 0o777, 0o600);

        // Another process reading while this one writes always finds the settings whole.
        let big: Settings = (0..2000).map(|n| (format!("Setting{n}"), n.into())).collect();
        save_settings(&path, &big).unwrap();
        let reader = {
            let path = path.clone();
            std::thread::spawn(move || (0..300).all(|_| read_settings(&path, 1).is_ok_and(|all| all.len() == 2000)))
        };
        for _ in 0..300 {
            save_settings(&path, &big).unwrap();
        }
        assert!(reader.join().unwrap(), "a reader caught the file half-written");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_settings_are_kept_not_written_over() {
        let dir = crate::gallery::tests::sandbox_dir("settings-damaged");
        let (path, backup) = (dir.join("settings.json"), dir.join("settings.json.bak"));
        // Half of one, as an older RSnip writing in place could leave it.
        let damaged = br#"{"LibraryDock": 1, "Destinations": [{"id": "a""#;
        std::fs::write(&path, damaged).unwrap();
        assert!(read_settings(&path, READ_TRIES).is_err(), "tried again, and still damaged");
        assert_eq!(read_settings(&path, 1).unwrap_or_default(), Settings::new(), "readers get defaults");
        change_settings_in(&path, set("CheckUpdates", 0)).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), damaged, "kept aside");
        assert_eq!(number(&read_settings(&path, 1).unwrap(), "CheckUpdates"), Some(0));

        // A writer that finds them half-written waits for the rest, rather than starting afresh.
        std::fs::remove_file(&backup).unwrap();
        std::fs::write(&path, &damaged[..10]).unwrap();
        let finish = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(60));
                std::fs::write(path, br#"{"LibraryDock": 1, "Theme": 2}"#).unwrap();
            })
        };
        change_settings_in(&path, set("CheckUpdates", 0)).unwrap();
        finish.join().unwrap();
        let all = read_settings(&path, 1).unwrap();
        assert_eq!(["LibraryDock", "Theme", "CheckUpdates"].map(|n| number(&all, n)), [Some(1), Some(2), Some(0)]);
        assert!(!backup.exists());

        // Nothing is written if the damaged file can't be kept.
        let unreadable = dir.join("folder.json");
        std::fs::create_dir(&unreadable).unwrap();
        assert!(change_settings_in(&unreadable, set("CheckUpdates", 0)).is_err());
        assert!(unreadable.is_dir() && !dir.join("folder.json.bak").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// How many times each writer in `changes_at_once_are_all_kept` changes its setting.
    const ROUNDS: u32 = 50;

    /// Starts `threads` writers in this process, each setting its own `{who}{n}` to 0, 1, ... up to
    /// `ROUNDS - 1`.
    fn writers(path: &Path, who: &str, threads: u32) -> Vec<std::thread::JoinHandle<()>> {
        let writer = |n| {
            let (path, name) = (path.to_owned(), format!("{who}{n}"));
            std::thread::spawn(move || (0..ROUNDS).for_each(|value| change_settings_in(&path, set(&name, value)).unwrap()))
        };
        (0..threads).map(writer).collect()
    }

    /// Run by `changes_at_once_are_all_kept` in child processes: other RSnip windows, changing
    /// settings of their own.
    #[test]
    fn settings_writer_child() {
        let (Some(path), Ok(who)) = (std::env::var_os("RSNIP_SETTINGS_PATH"), std::env::var("RSNIP_SETTINGS_WRITER")) else { return };
        writers(Path::new(&path), &who, 3).into_iter().for_each(|writer| writer.join().unwrap());
    }

    #[test]
    fn changes_at_once_are_all_kept() {
        let dir = crate::gallery::tests::sandbox_dir("settings-at-once");
        let path = dir.join("settings.json");
        change_settings_in(&path, set("Theme", 2)).unwrap();
        let children = ["ChildA", "ChildB"].map(|who| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "gallery::platform::unix::tests::settings_writer_child", "--test-threads=1"])
                .env("RSNIP_SETTINGS_PATH", &path)
                .env("RSNIP_SETTINGS_WRITER", who)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        });
        writers(&path, "Thread", 6).into_iter().for_each(|writer| writer.join().unwrap());
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let all = read_settings(&path, 1).unwrap();
        let mut names: Vec<_> = (0..6).map(|n| format!("Thread{n}")).collect();
        names.extend(["ChildA", "ChildB"].iter().flat_map(|who| (0..3).map(move |n| format!("{who}{n}"))));
        for name in &names {
            assert_eq!(number(&all, name), Some(ROUNDS - 1), "{name} lost a change");
        }
        assert_eq!((number(&all, "Theme"), all.len()), (Some(2), names.len() + 1), "and nothing else lost");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_held_settings_lock_is_waited_for_then_gone_past() {
        use std::time::{Duration, Instant};
        let dir = crate::gallery::tests::sandbox_dir("settings-lock");
        let path = dir.join("rsnip").join("settings.json");
        let held = lock_settings(&path, LOCK_WAIT).expect("made, with its folder");
        let started = Instant::now();
        assert!(lock_settings(&path, Duration::from_millis(100)).is_none(), "held elsewhere");
        assert!(started.elapsed() >= Duration::from_millis(100), "waited for first");

        // A writer waiting has it as soon as it's let go.
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || lock_settings(&path, LOCK_WAIT).is_some())
        };
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
        assert!(waiter.join().unwrap());

        // Held for good (by a stuck RSnip, say), a change goes ahead without it after a while.
        let held = lock_settings(&path, LOCK_WAIT).unwrap();
        let started = Instant::now();
        change_settings_in(&path, set("CheckUpdates", 0)).unwrap();
        assert!(started.elapsed() >= LOCK_WAIT);
        assert_eq!(number(&read_settings(&path, 1).unwrap(), "CheckUpdates"), Some(0));
        drop(held);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn makes_private_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::gallery::tests::sandbox_dir("private-dir");
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        let new = dir.join("new");
        private_dir(&new).unwrap();
        assert_eq!(mode(&new), 0o700);
        let open = dir.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        private_dir(&open).unwrap();
        assert_eq!(mode(&open), 0o700, "made private");
        let (file, link) = (dir.join("file"), dir.join("link"));
        std::fs::write(&file, "").unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();
        assert!(private_dir(&file).is_err() && private_dir(&link).is_err() && private_dir(&dir.join("no/such")).is_err());
        // The first one that will do.
        assert_eq!(first_private(&[dir.join("no/such"), link.clone(), file.clone(), open.clone(), new.clone()]), Some(open));
        assert_eq!(first_private(&[link, file]), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub fn setting(name: &str) -> Option<u32> {
    number(&settings(), name)
}

pub fn set_setting(name: &str, value: u32) {
    change_settings(|all| {
        all.insert(name.to_owned(), value.into());
    });
}

/// A text setting, like the folder screenshots are saved to; None if it's missing or empty.
pub fn text_setting(name: &str) -> Option<String> {
    settings().get(name)?.as_str().map(str::trim).filter(|t| !t.is_empty()).map(String::from)
}

pub fn set_text_setting(name: &str, value: &str) {
    change_settings(|all| {
        all.insert(name.to_owned(), value.into());
    });
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

/// Opens `path` as `open` does, then tells `then` (on another thread) whether xdg-open managed to:
/// it's missing, or fails when there's no browser, over xrdp say.
pub fn open_then(path: &Path, then: impl FnOnce(Result<(), String>) + Send + 'static) {
    match Command::new("xdg-open").arg(path).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                then(match child.wait() {
                    Ok(status) if status.success() => Ok(()),
                    Ok(status) => Err(format!("xdg-open failed, {status}")),
                    Err(e) => Err(format!("xdg-open: {e}")),
                })
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => then(Err("xdg-open isn't installed".into())),
        Err(e) => then(Err(format!("couldn't run xdg-open: {e}"))),
    }
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
    let (year, month, day, ..) = local_time(t);
    (year, month, day)
}

/// The local date and time of `t` as (year, month, day, hour, minute).
pub fn local_time(t: SystemTime) -> (i32, u32, u32, u32, u32) {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() as libc::time_t);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32)
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

/// A microphone, or an output, to record from: `id` is what's saved in Settings, `name` what's
/// shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundDevice {
    pub id: String,
    pub name: String,
}

/// The microphones the sound server (PulseAudio, or PipeWire's) knows, without the outputs'
/// monitors.
pub fn microphones() -> Vec<SoundDevice> {
    super::super::recorder::microphones()
}

/// The outputs whose sound can be recorded: their monitors.
pub fn outputs() -> Vec<SoundDevice> {
    super::super::recorder::outputs()
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
