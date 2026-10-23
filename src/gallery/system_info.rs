//! Settings → System info: the facts a bug report needs (which build, which system and graphics
//! driver, which settings), so whoever picks up an issue can tell where it came from without a
//! round of questions. *Copy* puts it on the clipboard as plain text, to paste into the issue.
//!
//! It holds no screenshots or file names, and paths in the home folder are written `~/…`, so it can
//! be pasted into a public issue as is. It is taken when the section opens.

use std::path::{Path, PathBuf};

use eframe::egui;

use super::env::Env;
use super::theme::Appearance;
use super::{platform, Dock};
use crate::options;
use crate::shortcuts::{self, Action};

/// What the report reads from the system (`platform::host`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Host {
    /// `Windows 11 Pro 24H2 (build 26100.2605)`, `Ubuntu 24.04.1 LTS`.
    pub os: String,
    /// Linux: the kernel release.
    pub kernel: Option<String>,
    /// Linux: the desktop and its session type (`GNOME, wayland`).
    pub desktop: Option<String>,
    pub cpu: Option<String>,
    pub threads: usize,
    /// Installed and available memory, in bytes.
    pub memory: Option<u64>,
    pub memory_available: Option<u64>,
    /// How this copy was set up (`Installer`, `Standalone`, …).
    pub install: String,
    pub program: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// Where the settings are kept.
    pub settings: String,
    /// The font file for text annotations.
    pub font: Option<PathBuf>,
    /// Linux: the programs RSnip runs, and whether each was found.
    pub helpers: Vec<(&'static str, bool)>,
}

/// The OpenGL driver the window is drawn with, as it names itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub renderer: String,
    pub vendor: String,
    pub version: String,
}

/// The window's OpenGL driver, if it is drawn with OpenGL (tests draw with wgpu).
pub fn gpu(frame: &eframe::Frame) -> Option<Gpu> {
    use eframe::glow::{self, HasContext};
    let gl = frame.gl()?;
    let get = |parameter| unsafe { gl.get_parameter_string(parameter) }.trim().to_string();
    Some(Gpu { renderer: get(glow::RENDERER), vendor: get(glow::VENDOR), version: get(glow::VERSION) })
}

/// One titled group of label/value rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(&'static str, String)>,
}

impl Section {
    fn new(title: &'static str) -> Section {
        Section { title, rows: Vec::new() }
    }

    fn row(&mut self, label: &'static str, value: impl Into<String>) {
        self.rows.push((label, value.into()));
    }
}

/// The whole report. `ctx` adds what only the window knows (its display and scale).
pub fn report(env: &Env, host: &Host, gpu: Option<&Gpu>, ctx: &egui::Context) -> Vec<Section> {
    vec![application(env, host), system(host), graphics(gpu, ctx), settings(env), crashes(env, host), files(env, host)]
}

/// The report as plain text: a title, then each section with its values in one column.
pub fn text(sections: &[Section]) -> String {
    let width = sections.iter().flat_map(|s| &s.rows).map(|(l, _)| l.chars().count()).max().unwrap_or(0);
    let mut out = String::from("RSnip System Info\n");
    for s in sections {
        out.push('\n');
        out.push_str(s.title);
        out.push('\n');
        for (label, value) in &s.rows {
            let pad = width - label.chars().count() + 2;
            out.push_str(&format!("  {label}{}{value}\n", " ".repeat(pad)));
        }
    }
    out
}

fn application(env: &Env, host: &Host) -> Section {
    let mut s = Section::new("Application");
    s.row("Version", format!("RSnip {}", env.version));
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    s.row("Build", format!("{profile}, {}-{}", std::env::consts::ARCH, std::env::consts::OS));
    s.row("Install", host.install.clone());
    s.row("Program", path_or_unknown(host.program.as_deref(), host));
    s
}

fn system(host: &Host) -> Section {
    let mut s = Section::new("System");
    s.row("OS", host.os.clone());
    if let Some(kernel) = &host.kernel {
        s.row("Kernel", kernel.clone());
    }
    if let Some(desktop) = &host.desktop {
        s.row("Desktop", desktop.clone());
    }
    let cpu = host.cpu.clone().unwrap_or_else(|| std::env::consts::ARCH.to_string());
    s.row("CPU", format!("{cpu}, {} threads", host.threads));
    let memory = match (host.memory, host.memory_available) {
        (Some(total), Some(available)) => format!("{} installed, {} available", bytes(total), bytes(available)),
        (Some(total), None) => format!("{} installed", bytes(total)),
        _ => "unknown".into(),
    };
    s.row("Memory", memory);
    if !host.helpers.is_empty() {
        s.row("Helper programs", helpers(&host.helpers));
    }
    s
}

fn graphics(gpu: Option<&Gpu>, ctx: &egui::Context) -> Section {
    let mut s = Section::new("Graphics");
    match gpu {
        Some(g) => {
            s.row("Renderer", if g.vendor.is_empty() { g.renderer.clone() } else { format!("{} ({})", g.renderer, g.vendor) });
            s.row("OpenGL", g.version.clone());
        }
        None => s.row("Renderer", "not reported"),
    }
    let ppp = ctx.pixels_per_point();
    let (monitor, native) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().native_pixels_per_point));
    if let Some(m) = monitor.filter(|m| m.x > 0.0 && m.y > 0.0) {
        s.row("Display", format!("{:.0}×{:.0} px", m.x * ppp, m.y * ppp));
    }
    let os = native.filter(|n| *n > 0.0).map_or(String::new(), |n| format!(" (system {:.0}%)", n * 100.0));
    s.row("Scale", format!("{:.0}%{os}", ppp * 100.0));
    let window = ctx.content_rect().size() * ppp;
    s.row("Window", format!("{:.0}×{:.0} px", window.x, window.y));
    s
}

fn settings(env: &Env) -> Section {
    let mut s = Section::new("Settings");
    let flag = |f: options::Flag| on_off(f.read(|name| env.setting(name)));
    s.row("Theme", Appearance::from_setting(env.setting(Appearance::SETTING)).label());
    s.row("Library position", if Dock::from_setting(env.setting(Dock::SETTING)) == Dock::Bottom { "Bottom" } else { "Left" });
    if !env.tray_app {
        let icon = match (options::RUN_IN_BACKGROUND.read(|name| env.setting(name)), env.tray_running()) {
            (_, true) => "showing",
            (true, false) => "not showing (no tray on this desktop?)",
            (false, false) => "off",
        };
        s.row("Background icon", icon);
        s.row("Start when you sign in", on_off(env.start_with_windows()));
    }
    if env.tray_app {
        s.row("Tray app", if env.tray_running() { "running" } else { "not running" });
        s.row("Start with Windows", on_off(env.start_with_windows()));
        let table = shortcuts::load(|name| env.setting(name));
        let set: Vec<String> = Action::ALL
            .into_iter()
            .filter(|&a| shortcuts::get(&table, a) != 0)
            .map(|a| format!("{} {}", platform::shortcut_name(shortcuts::get(&table, a)), a.label().to_lowercase()))
            .collect();
        s.row("Shortcuts", if set.is_empty() { "none".to_string() } else { set.join(", ") });
        s.row("Take over Win+Shift+S", flag(options::WIN_SHIFT_S));
        s.row("Windows' Print Screen capture", on_off(env.snipping_key()));
        s.row("Save to the folder", flag(options::SAVE_TO_FOLDER));
        let get = |name: &str| env.setting(name);
        let record = if options::RECORD_GIF.read(get) {
            "GIF".to_string()
        } else {
            let codec = if options::RECORD_HEVC.read(get) { "H.265" } else { "H.264" };
            format!("MP4, {codec}, {} fps", options::RECORD_FPS.read(get))
        };
        s.row("Record as", record);
    }
    let ocr = match &env.ocr {
        Some(_) => format!("available, read for search: {}", on_off(env.setting(super::search::SETTING) != Some(0))),
        None => "not available".into(),
    };
    s.row("Text recognition", ocr);
    let mut updates = format!("check automatically: {}", flag(options::CHECK_UPDATES));
    if env.tray_app {
        updates += &format!(", install automatically: {}", flag(options::AUTO_UPDATE));
    }
    s.row("Updates", updates);
    s
}

fn crashes(env: &Env, host: &Host) -> Section {
    let mut s = Section::new("Crashes");
    let crashes = crate::crash::list(&env.crash_dir);
    match crashes.first() {
        Some(last) => {
            s.row("Last crash", format!("{}, {}", crate::crash::utc(last.time), last.summary));
            s.row("Crash reports", format!("{} in {}", crashes.len(), tidy_path(&env.crash_dir, host.home.as_deref())));
        }
        None => s.row("Last crash", "none"),
    }
    s
}

fn files(env: &Env, host: &Host) -> Section {
    let mut s = Section::new("Files");
    let count = std::fs::read_dir(&env.dir).map(|d| d.flatten().filter(|e| super::is_library_file(&e.path())).count());
    let folder = tidy_path(&env.dir, host.home.as_deref());
    s.row("Screenshots", match count {
        Ok(n) => format!("{folder} ({n} {})", if n == 1 { "file" } else { "files" }),
        Err(_) => format!("{folder} (not found)"),
    });
    s.row("Settings", host.settings.clone());
    s.row("Annotation font", host.font.as_deref().map_or("none found".to_string(), |f| tidy_path(f, host.home.as_deref())));
    s
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn path_or_unknown(path: Option<&Path>, host: &Host) -> String {
    path.map_or("unknown".to_string(), |p| tidy_path(p, host.home.as_deref()))
}

/// `xdg-open, fc-match; missing: curl`.
fn helpers(found: &[(&str, bool)]) -> String {
    let names = |want: bool| found.iter().filter(|(_, f)| *f == want).map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
    match (names(true), names(false)) {
        (have, missing) if missing.is_empty() => have,
        (have, missing) if have.is_empty() => format!("missing: {missing}"),
        (have, missing) => format!("{have}; missing: {missing}"),
    }
}

/// `path` with `home` written as `~`, so a pasted report doesn't carry the user's account name.
pub fn tidy_path(path: &Path, home: Option<&Path>) -> String {
    match home.filter(|h| !h.as_os_str().is_empty()).and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// `15.6 GB`, `812 MB`.
pub fn bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let f = b as f64;
    if f >= 1024.0 * MB {
        format!("{:.1} GB", f / (1024.0 * MB))
    } else {
        format!("{:.0} MB", f / MB)
    }
}

/// `Windows 11 Pro 24H2 (build 26100.2605)`. Windows 11 still calls itself "Windows 10" in
/// `ProductName`; its builds start at 22000.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_name(product: Option<&str>, display: Option<&str>, build: Option<&str>, ubr: Option<u32>) -> String {
    let mut name = product.unwrap_or("Windows").to_string();
    if build.and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22000) {
        if let Some(rest) = name.strip_prefix("Windows 10") {
            name = format!("Windows 11{rest}");
        }
    }
    if let Some(d) = display {
        name = format!("{name} {d}");
    }
    match (build, ubr) {
        (Some(b), Some(u)) => format!("{name} (build {b}.{u})"),
        (Some(b), None) => format!("{name} (build {b})"),
        _ => name,
    }
}

/// `PRETTY_NAME` from os-release (`Ubuntu 24.04.1 LTS`), else `NAME VERSION_ID`.
#[cfg_attr(windows, allow(dead_code))]
pub fn os_release_name(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|v| !v.is_empty())
    };
    field("PRETTY_NAME").or_else(|| match (field("NAME"), field("VERSION_ID")) {
        (Some(n), Some(v)) => Some(format!("{n} {v}")),
        (n, _) => n,
    })
}

/// The CPU model from `/proc/cpuinfo` (x86 `model name`, Arm `Model` or `Hardware`).
#[cfg_attr(windows, allow(dead_code))]
pub fn cpuinfo_model(text: &str) -> Option<String> {
    ["model name", "Model", "Hardware"].iter().find_map(|key| {
        text.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == *key).map(|(_, v)| v.trim().to_string())).filter(|v| !v.is_empty())
    })
}

/// A `/proc/meminfo` value (`MemTotal`, `MemAvailable`), in bytes.
#[cfg_attr(windows, allow(dead_code))]
pub fn meminfo(text: &str, key: &str) -> Option<u64> {
    let kb: u64 = text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    kb.checked_mul(1024)
}

/// Made-up values for the sandbox, so tests and snapshots don't depend on the machine.
#[cfg(test)]
pub fn example_host(home: &Path) -> Host {
    Host {
        os: "Example OS 1.0".into(),
        kernel: Some("6.1.0".into()),
        desktop: Some("GNOME, wayland".into()),
        cpu: Some("Example CPU @ 3.0GHz".into()),
        threads: 8,
        memory: Some(16 << 30),
        memory_available: Some(9 << 30),
        install: "Standalone".into(),
        program: Some(home.join("rsnip")),
        home: Some(home.to_path_buf()),
        settings: "~/settings.json".into(),
        font: None,
        helpers: vec![("xdg-open", true), ("curl", false)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_11_is_named_from_its_build() {
        let name = windows_name(Some("Windows 10 Pro"), Some("24H2"), Some("26100"), Some(2605));
        assert_eq!(name, "Windows 11 Pro 24H2 (build 26100.2605)");
        assert_eq!(windows_name(Some("Windows 10 Home"), Some("22H2"), Some("19045"), None), "Windows 10 Home 22H2 (build 19045)");
        assert_eq!(windows_name(None, None, None, None), "Windows");
    }

    #[test]
    fn os_release_prefers_the_pretty_name() {
        assert_eq!(os_release_name("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(os_release_name("NAME=Arch\nVERSION_ID=rolling\n").as_deref(), Some("Arch rolling"));
        assert_eq!(os_release_name("PRETTY_NAME_EXTRA=x\nNAME='Fedora Linux'\n").as_deref(), Some("Fedora Linux"));
        assert_eq!(os_release_name(""), None);
    }

    #[test]
    fn cpu_and_memory_from_proc() {
        let x86 = "processor\t: 0\nmodel name\t: AMD Ryzen 7 5800X 8-Core Processor\n";
        assert_eq!(cpuinfo_model(x86).as_deref(), Some("AMD Ryzen 7 5800X 8-Core Processor"));
        assert_eq!(cpuinfo_model("processor : 0\nHardware : BCM2835\nModel : Raspberry Pi 4 Model B\n").as_deref(), Some("Raspberry Pi 4 Model B"));
        assert_eq!(cpuinfo_model("processor : 0\n"), None);

        let mem = "MemTotal:       16318412 kB\nMemFree:         1000 kB\nMemAvailable:    9000000 kB\n";
        assert_eq!(meminfo(mem, "MemTotal"), Some(16318412 * 1024));
        assert_eq!(meminfo(mem, "MemAvailable"), Some(9000000 * 1024));
        assert_eq!(meminfo(mem, "Mem"), None, "a whole key only");
        assert_eq!(meminfo("MemTotal: lots\n", "MemTotal"), None);
    }

    #[test]
    fn home_is_written_as_a_tilde() {
        let home = Path::new("/home/someone");
        assert_eq!(tidy_path(Path::new("/home/someone/Pictures/Screenshots"), Some(home)), format!("~{}Pictures/Screenshots", std::path::MAIN_SEPARATOR));
        assert_eq!(tidy_path(home, Some(home)), "~");
        assert_eq!(tidy_path(Path::new("/opt/rsnip"), Some(home)), "/opt/rsnip");
        assert_eq!(tidy_path(Path::new("/home/someoneelse/x"), Some(home)), "/home/someoneelse/x", "whole folder names only");
        assert_eq!(tidy_path(Path::new("/x"), None), "/x");
    }

    #[test]
    fn sizes_and_helpers_read_naturally() {
        assert_eq!(bytes(16 << 30), "16.0 GB");
        assert_eq!(bytes(812 << 20), "812 MB");
        assert_eq!(helpers(&[("xdg-open", true), ("fc-match", true), ("curl", false)]), "xdg-open, fc-match; missing: curl");
        assert_eq!(helpers(&[("xdg-open", true)]), "xdg-open");
        assert_eq!(helpers(&[("curl", false)]), "missing: curl");
    }

    #[test]
    fn text_lines_up_the_values() {
        let sections = [
            Section { title: "One", rows: vec![("A", "1".into()), ("Longer", "2".into())] },
            Section { title: "Two", rows: vec![("B", "3".into())] },
        ];
        assert_eq!(text(&sections), "RSnip System Info\n\nOne\n  A       1\n  Longer  2\n\nTwo\n  B       3\n");
    }
}
