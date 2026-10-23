//! Crash reports. When RSnip panics, what happened and where in its code is written to a file, so
//! it isn't lost (the Windows app has no console to print it to). The next launch says RSnip closed
//! unexpectedly, and Settings → System info shows the latest report and copies it for a bug report.
//!
//! A report holds the panic message, where it happened and the backtrace, with the home folder
//! written as `~`; never screenshots or what's in them. Release builds have no symbols, so their
//! backtraces are left out, but the message and the line it came from are still there.
//!
//! On Linux, crashes outside Rust's own checks (a segfault in a graphics driver, an abort in a C
//! library) get a shorter report too: see `native`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many reports are kept; older ones are deleted.
const KEEP: usize = 10;
/// Backtrace frames kept in a report.
const MAX_FRAMES: usize = 40;
/// The time of the newest crash the user has been told about.
pub const SEEN_SETTING: &str = "CrashSeen";
/// A report has been written for this process, so a signal that follows (the abort after a panic)
/// doesn't write another.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Where reports are written: `%LOCALAPPDATA%\RSnip\crashes`, or `~/.local/state/rsnip/crashes`.
pub fn dir() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("RSnip"))
    } else {
        var("XDG_STATE_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("state"))).map(|d| d.join("rsnip"))
    };
    base.unwrap_or_else(|| std::env::temp_dir().join("rsnip")).join("crashes")
}

/// Writes a report for every panic, then reports it as before (on a console, if there is one).
pub fn install() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let part = part(&args);
    #[cfg(unix)]
    native::install(&dir(), part);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        REPORTED.store(true, Ordering::SeqCst);
        let panic = Panic {
            time: now(),
            part,
            thread: std::thread::current().name().unwrap_or("unnamed").to_string(),
            message: info.payload_as_str().unwrap_or("(no message)").to_string(),
            location: info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        };
        let _ = save(&dir(), &panic, home().as_deref());
        default(info);
    }));
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn home() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(var).ok().filter(|h| h.len() > 1)
}

/// Which part of RSnip was running, from its command line (never the file it was given).
pub fn part(args: &[String]) -> &'static str {
    match args.first().map(String::as_str) {
        Some("--settings") => "Settings",
        Some("--pin") => "pin",
        Some("--gallery") => "library",
        _ if cfg!(windows) => "tray app",
        _ => "library",
    }
}

/// What a panic hook knows about a panic.
pub struct Panic {
    /// Seconds since 1970.
    pub time: u64,
    pub part: &'static str,
    pub thread: String,
    pub message: String,
    pub location: Option<String>,
    pub backtrace: String,
}

impl Panic {
    /// The report, with `home` written as `~`.
    pub fn text(&self, home: Option<&str>) -> String {
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let message = self.message.trim().replace('\n', "\n          ");
        let mut out = format!(
            "RSnip crash report\n\nTime      {}\nVersion   RSnip {} ({profile}, {}-{})\nWhere     {}, thread {}\nMessage   {message}\nLocation  {}\n",
            utc(self.time),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH,
            std::env::consts::OS,
            self.part,
            self.thread,
            self.location.as_deref().unwrap_or("unknown"),
        );
        match frames(&self.backtrace) {
            frames if frames.is_empty() => out.push_str("\nNo backtrace (this build has no symbols)\n"),
            frames => {
                out.push_str("\nBacktrace\n");
                out.push_str(&frames);
            }
        }
        match home {
            Some(home) => out.replace(home, "~"),
            None => out,
        }
    }
}

/// The frames of a `std::backtrace::Backtrace` that name a function, up to `MAX_FRAMES`.
pub fn frames(backtrace: &str) -> String {
    let mut out = String::new();
    let (mut kept, mut keep) = (0, false);
    for line in backtrace.lines() {
        let trimmed = line.trim_start();
        if let Some((number, symbol)) = trimmed.split_once(": ").filter(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            keep = symbol != "<unknown>" && kept < MAX_FRAMES;
            kept += keep as usize;
            if keep {
                out.push_str(&format!("  {number}: {symbol}\n"));
            }
        } else if keep && trimmed.starts_with("at ") {
            out.push_str(&format!("        {trimmed}\n"));
        }
    }
    out
}

/// `2026-10-23 14:05 UTC`.
pub fn utc(secs: u64) -> String {
    let [year, month, day, hour, minute] = civil(secs);
    format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Seconds since 1970 as UTC year, month, day, hour and minute (no allocation, for `native`).
fn civil(secs: u64) -> [u64; 5] {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    [year as u64, month as u64, day as u64, rest / 3600, rest % 3600 / 60]
}

/// Writes the report into `dir` and deletes all but the newest `KEEP`.
pub fn save(dir: &Path, panic: &Panic, home: Option<&str>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("crash-{}-{}.txt", panic.time, std::process::id()));
    std::fs::write(&path, panic.text(home))?;
    for old in list(dir).into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(old.path);
    }
    Ok(path)
}

/// A saved report.
#[derive(Clone, Debug, PartialEq)]
pub struct Crash {
    pub path: PathBuf,
    /// Seconds since 1970.
    pub time: u64,
    /// `library: index out of bounds (src/gallery/doc.rs:12:5)`.
    pub summary: String,
}

/// The reports in `dir`, newest first.
pub fn list(dir: &Path) -> Vec<Crash> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut crashes: Vec<Crash> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let time = name.strip_prefix("crash-")?.strip_suffix(".txt")?.split('-').next()?.parse().ok()?;
            let summary = std::fs::read_to_string(e.path()).map(|t| summary(&t)).unwrap_or_default();
            Some(Crash { path: e.path(), time, summary })
        })
        .collect();
    crashes.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.path.cmp(&a.path)));
    crashes
}

/// The newest report, if it is newer than the one the user was last told about.
pub fn unseen(dir: &Path, seen: Option<u32>) -> Option<Crash> {
    list(dir).into_iter().next().filter(|c| c.time > u64::from(seen.unwrap_or(0)))
}

/// One line from a report: where, what and which line of code.
pub fn summary(text: &str) -> String {
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)).map(str::trim).unwrap_or("");
    let part = field("Where").split(',').next().unwrap_or("");
    let mut message: String = field("Message").chars().take(120).collect();
    if field("Message").chars().count() > 120 {
        message.push('…');
    }
    match field("Location") {
        "" | "unknown" => format!("{part}: {message}"),
        location => format!("{part}: {message} ({location})"),
    }
}

/// Crashes that skip the panic hook, caught as signals: a report is written with only what is safe
/// to do inside a signal handler (no allocation, no locks), then the signal goes on to whatever
/// handled it before (Rust's stack-overflow check, or the default: the process ends).
#[cfg(unix)]
mod native {
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;

    const SIGNALS: [libc::c_int; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT];

    /// Everything the handler needs, worked out beforehand.
    pub struct Prepared {
        /// The crash folder, as bytes.
        dir: Vec<u8>,
        /// `RSnip 0.4.2 (release, x86_64-linux)`.
        version: String,
        part: &'static str,
        /// What each signal was handled by before.
        previous: [libc::sigaction; SIGNALS.len()],
    }

    static PREPARED: OnceLock<Prepared> = OnceLock::new();

    pub fn install(dir: &Path, part: &'static str) {
        // Made now: a crashing process may not manage it.
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let version = format!("RSnip {} ({profile}, {}-{})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS);
        let mut previous: [libc::sigaction; SIGNALS.len()] = unsafe { std::mem::zeroed() };
        for (signal, previous) in SIGNALS.iter().zip(previous.iter_mut()) {
            unsafe { libc::sigaction(*signal, std::ptr::null(), previous) };
        }
        if PREPARED.set(Prepared { dir: dir.as_os_str().as_bytes().to_vec(), version, part, previous }).is_err() {
            return;
        }
        for signal in SIGNALS {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as *const () as libc::sighandler_t;
                // The alternate stack Rust sets up, so a stack overflow can be reported too.
                action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    extern "C" fn handler(signal: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
        let Some(prepared) = PREPARED.get() else { return };
        if !super::REPORTED.swap(true, Ordering::SeqCst) {
            write_report(prepared, signal);
        }
        // Hand the signal on, as if this handler weren't here. A fault happens again when this
        // returns, and the previous handler sees it as it was (Rust's needs the faulting address to
        // tell a stack overflow); a signal that was sent (abort, kill) is sent again.
        let Some(i) = SIGNALS.iter().position(|&s| s == signal) else { return };
        let fault = signal != libc::SIGABRT && !info.is_null() && unsafe { (*info).si_code } > 0;
        unsafe {
            libc::sigaction(signal, &prepared.previous[i], std::ptr::null_mut());
            if !fault {
                libc::raise(signal);
            }
        }
    }

    fn write_report(prepared: &Prepared, signal: libc::c_int) {
        let mut now: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
        let time = now.tv_sec.max(0) as u64;
        let mut path = Buf::new();
        path.push(&prepared.dir);
        path.push(b"/crash-");
        path.number(time, 1);
        path.push(b"-");
        path.number(std::process::id() as u64, 1);
        path.push(b".txt\0");
        let mut text = Buf::new();
        report(&mut text, prepared.version.as_bytes(), prepared.part.as_bytes(), signal, time);
        unsafe {
            let fd = libc::open(path.bytes.as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644);
            if fd >= 0 {
                libc::write(fd, text.bytes.as_ptr() as *const libc::c_void, text.len);
                libc::close(fd);
            }
        }
    }

    /// The report, laid out like a panic's so `list` and `summary` read it the same way.
    pub fn report(out: &mut Buf, version: &[u8], part: &[u8], signal: libc::c_int, time: u64) {
        let [year, month, day, hour, minute] = super::civil(time);
        out.push(b"RSnip crash report\n\nTime      ");
        out.number(year, 4);
        out.push(b"-");
        out.number(month, 2);
        out.push(b"-");
        out.number(day, 2);
        out.push(b" ");
        out.number(hour, 2);
        out.push(b":");
        out.number(minute, 2);
        out.push(b" UTC\nVersion   ");
        out.push(version);
        out.push(b"\nWhere     ");
        out.push(part);
        out.push(b", native code\nMessage   ");
        out.push(match signal {
            libc::SIGSEGV => b"Segmentation fault (SIGSEGV)".as_slice(),
            libc::SIGBUS => b"Bus error (SIGBUS)",
            libc::SIGILL => b"Illegal instruction (SIGILL)",
            libc::SIGFPE => b"Arithmetic error (SIGFPE)",
            libc::SIGABRT => b"Aborted (SIGABRT)",
            _ => b"Signal",
        });
        out.push(b"\nLocation  unknown\n\nNo backtrace (a crash outside Rust, e.g. in a graphics driver or system library)\n");
    }

    /// A fixed-size buffer: nothing in the handler may allocate.
    pub struct Buf {
        pub bytes: [u8; 1024],
        pub len: usize,
    }

    impl Buf {
        pub fn new() -> Buf {
            Buf { bytes: [0; 1024], len: 0 }
        }

        fn push(&mut self, bytes: &[u8]) {
            let n = bytes.len().min(self.bytes.len() - self.len);
            self.bytes[self.len..self.len + n].copy_from_slice(&bytes[..n]);
            self.len += n;
        }

        /// `n` in decimal, padded with zeros to `width` digits.
        fn number(&mut self, mut n: u64, width: usize) {
            let mut digits = [0u8; 20];
            let mut i = digits.len();
            while n > 0 || digits.len() - i < width.max(1) {
                i -= 1;
                digits[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
            self.push(&digits[i..]);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_report_reads_like_a_panics() {
            let mut out = Buf::new();
            report(&mut out, b"RSnip 1.2.3 (debug, x86_64-linux)", b"library", libc::SIGSEGV, 1_792_764_312);
            let text = std::str::from_utf8(&out.bytes[..out.len]).unwrap();
            assert!(text.starts_with("RSnip crash report\n\nTime      2026-10-23 14:05 UTC\nVersion   RSnip 1.2.3 (debug, x86_64-linux)\n"), "{text}");
            assert_eq!(super::super::summary(text), "library: Segmentation fault (SIGSEGV)");
        }

        /// Run by `a_segfault_writes_a_report` in a child process, which it crashes.
        #[test]
        fn crash_child() {
            let Some(dir) = std::env::var_os("RSNIP_CRASH_CHILD") else { return };
            install(Path::new(&dir), "library");
            // A real fault (as a driver bug would make), not a raised signal.
            unsafe { std::ptr::write_volatile(std::hint::black_box(8usize) as *mut u8, 1) };
            unreachable!("the fault ends the process");
        }

        #[test]
        fn a_segfault_writes_a_report() {
            let dir = std::env::temp_dir().join(format!("rsnip-native-crash-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash::native::tests::crash_child", "--test-threads=1"])
                .env("RSNIP_CRASH_CHILD", &dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGSEGV), "it still ends as a segfault: {status:?}");
            let crashes = super::super::list(&dir);
            assert_eq!(crashes.len(), 1, "one report, not one per signal");
            assert_eq!(crashes[0].summary, "library: Segmentation fault (SIGSEGV)");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panic_at(time: u64) -> Panic {
        Panic {
            time,
            part: "library",
            thread: "main".into(),
            message: "index out of bounds: the len is 2 but the index is 5".into(),
            location: Some("src/gallery/doc.rs:12:5".into()),
            backtrace: String::new(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn times_are_utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc(1_792_764_312), "2026-10-23 14:05 UTC");
    }

    #[test]
    fn only_named_frames_are_kept() {
        let backtrace = "   0: rsnip::crash::install::{{closure}}\n             at ./src/crash.rs:30:25\n   1: <unknown>\n             at ./x.rs:1:1\n   2: rsnip::main\n";
        assert_eq!(frames(backtrace), "  0: rsnip::crash::install::{{closure}}\n        at ./src/crash.rs:30:25\n  2: rsnip::main\n");
        assert_eq!(frames("   0: <unknown>\n   1: <unknown>\n"), "", "a release build has none");
        let long: String = (0..100).map(|i| format!("  {i}: f{i}\n")).collect();
        assert_eq!(frames(&long).lines().count(), MAX_FRAMES);
    }

    #[test]
    fn a_report_says_what_where_and_hides_home() {
        let mut panic = panic_at(1_792_764_312);
        panic.message = "couldn't open /home/someone/Pictures\nsecond line".into();
        panic.backtrace = "   0: rsnip::main\n             at /home/someone/rsnip/src/main.rs:3:1\n".into();
        let text = panic.text(Some("/home/someone"));
        assert!(text.starts_with("RSnip crash report\n\nTime      2026-10-23 14:05 UTC\n"), "{text}");
        assert!(text.contains("\nWhere     library, thread main\n"));
        assert!(text.contains("\nMessage   couldn't open ~/Pictures\n          second line\n"));
        assert!(text.contains("\nLocation  src/gallery/doc.rs:12:5\n"));
        assert!(text.contains("\nBacktrace\n  0: rsnip::main\n        at ~/rsnip/src/main.rs:3:1\n"));
        assert!(!text.contains("someone"));
        assert!(panic_at(0).text(None).contains("No backtrace"));
    }

    #[test]
    fn reports_are_listed_newest_first_and_pruned() {
        let dir = temp("list");
        assert!(list(&dir).is_empty(), "no folder yet");
        for time in 1..=KEEP as u64 + 3 {
            save(&dir, &panic_at(time * 100), None).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "not a report").unwrap();
        let crashes = list(&dir);
        assert_eq!(crashes.len(), KEEP, "the oldest are deleted");
        assert_eq!(crashes[0].time, (KEEP as u64 + 3) * 100);
        assert_eq!(crashes[0].summary, "library: index out of bounds: the len is 2 but the index is 5 (src/gallery/doc.rs:12:5)");

        assert_eq!(unseen(&dir, None).map(|c| c.time), Some(1300));
        assert_eq!(unseen(&dir, Some(1200)).map(|c| c.time), Some(1300));
        assert_eq!(unseen(&dir, Some(1300)), None, "already told");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summaries_are_one_short_line() {
        let text = format!("Where     tray app, thread main\nMessage   {}\nLocation  unknown\n", "x".repeat(200));
        assert_eq!(summary(&text), format!("tray app: {}…", "x".repeat(120)));
    }

    #[test]
    fn the_part_comes_from_the_command_line() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(part(&args(&["--settings"])), "Settings");
        assert_eq!(part(&args(&["--pin", "/home/someone/secret.png"])), "pin");
        assert_eq!(part(&args(&["--gallery"])), "library");
        assert_eq!(part(&args(&[])), if cfg!(windows) { "tray app" } else { "library" });
    }
}
