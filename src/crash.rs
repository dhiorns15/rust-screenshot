//! Crash reports. When RSnip panics, what happened and where in its code is written to a file, so
//! it isn't lost (the Windows app has no console to print it to). The next launch says RSnip closed
//! unexpectedly, and Settings → System info shows the latest report and copies it for a bug report.
//!
//! A report holds the panic message, where it happened and the backtrace, with the home folder
//! written as `~`; never screenshots or what's in them. Linux release builds keep function names
//! for the backtrace (not line numbers); Windows ones have no symbols in the exe, so there the
//! message and the line it came from are what's left.
//!
//! Crashes outside Rust's own checks (a segfault in a graphics driver, an access violation in a
//! Windows API) get a shorter report too: see `native`.

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
    install_into(dir(), part(&args));
}

fn install_into(dir: PathBuf, part: &'static str) {
    native::install(&dir, part);
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
        let _ = save(&dir, &panic, home().as_deref());
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

/// The frames of a `std::backtrace::Backtrace` that name a function, from the one that panicked
/// (the panic hook and Rust's panic machinery above it are left out), up to `MAX_FRAMES`.
pub fn frames(backtrace: &str) -> String {
    // Each named frame, with its `at file:line` lines.
    let mut frames: Vec<(String, Vec<String>)> = Vec::new();
    let mut keep = false;
    for line in backtrace.lines() {
        let trimmed = line.trim_start();
        if let Some((number, symbol)) = trimmed.split_once(": ").filter(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            keep = symbol != "<unknown>";
            if keep {
                frames.push((format!("  {number}: {symbol}\n"), Vec::new()));
            }
        } else if let Some(frame) = frames.last_mut().filter(|_| keep && trimmed.starts_with("at ")) {
            frame.1.push(format!("        {trimmed}\n"));
        }
    }
    const MACHINERY: [&str; 6] = ["rsnip::crash::install_into", "std::panicking::", "std::sys::backtrace::__rust_end_short_backtrace", "__rustc::rust_begin_unwind", "core::panicking::", "std::backtrace::"];
    let skip = frames.iter().rposition(|(symbol, _)| MACHINERY.iter().any(|m| symbol.split_once(": ").is_some_and(|(_, s)| s.starts_with(m)))).map_or(0, |i| i + 1);
    // A panic inside the hook itself would leave nothing; keep everything then.
    let skip = if skip == frames.len() { 0 } else { skip };
    frames.into_iter().skip(skip).take(MAX_FRAMES).flat_map(|(symbol, at)| std::iter::once(symbol).chain(at)).collect()
}

/// `2026-10-23 14:05 UTC`.
pub fn utc(secs: u64) -> String {
    let [year, month, day, hour, minute] = civil(secs);
    format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Seconds since 1970 as UTC year, month, day, hour and minute (no allocation, for `native`).
pub fn civil(secs: u64) -> [u64; 5] {
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

/// Crashes that skip the panic hook: on Linux a fault or abort signal, on Windows an unhandled
/// exception (an access violation, a stack overflow). A report is written with only what is safe
/// in a crashing process (no allocation, no locks), then the crash goes on as if this weren't here.
mod native {
    use std::path::Path;
    use std::sync::OnceLock;

    #[cfg(unix)]
    pub use signals::install;
    #[cfg(windows)]
    pub use exceptions::install;

    /// Everything the handler needs, worked out beforehand.
    struct Prepared<D> {
        /// The crash folder, in the system's encoding.
        dir: D,
        /// `RSnip 0.4.2 (release, x86_64-linux)`.
        version: String,
        part: &'static str,
    }

    fn prepare<D>(dir: &Path, part: &'static str, encode: impl FnOnce(&Path) -> D) -> Option<Prepared<D>> {
        // Made now: a crashing process may not manage it.
        std::fs::create_dir_all(dir).ok()?;
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let version = format!("RSnip {} ({profile}, {}-{})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS);
        Some(Prepared { dir: encode(dir), version, part })
    }

    /// `crash-<time>-<pid>.txt`, the name `list` reads the time from.
    fn file_name(out: &mut Buf, time: u64) {
        out.push(b"crash-");
        out.number(time, 1);
        out.push(b"-");
        out.number(std::process::id() as u64, 1);
        out.push(b".txt");
    }

    /// The report, laid out like a panic's so `list` and `summary` read it the same way. `message`
    /// says what happened, `location` where (or is empty), and `details` follow.
    fn report(out: &mut Buf, version: &[u8], part: &[u8], time: u64, message: &[u8], location: &[u8]) {
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
        out.push(message);
        out.push(b"\nLocation  ");
        out.push(if location.is_empty() { b"unknown".as_slice() } else { location });
        out.push(b"\n");
    }

    /// Buffers for the one report a process writes (only the first crash gets past `REPORTED`),
    /// kept off the stack: a signal handler's is small, and a stack overflow leaves little.
    struct Scratch(std::cell::UnsafeCell<([Buf; 4], [u16; 1024])>);

    unsafe impl Sync for Scratch {}

    static SCRATCH: Scratch = Scratch(std::cell::UnsafeCell::new(([const { Buf::new() }; 4], [0; 1024])));

    /// # Safety
    /// Only by the handler that won `REPORTED`, once.
    unsafe fn scratch() -> &'static mut ([Buf; 4], [u16; 1024]) {
        &mut *SCRATCH.0.get()
    }

    /// A fixed-size buffer: nothing in a crashing process may allocate.
    pub struct Buf {
        pub bytes: [u8; 4096],
        pub len: usize,
    }

    impl Buf {
        pub const fn new() -> Buf {
            Buf { bytes: [0; 4096], len: 0 }
        }

        pub fn as_bytes(&self) -> &[u8] {
            &self.bytes[..self.len]
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

        /// `n` as `0x` and hex digits, padded to `width`.
        #[cfg_attr(unix, allow(dead_code))]
        fn hex(&mut self, mut n: u64, width: usize) {
            let mut digits = [0u8; 16];
            let mut i = digits.len();
            while n > 0 || digits.len() - i < width.max(1) {
                i -= 1;
                digits[i] = b"0123456789ABCDEF"[(n % 16) as usize];
                n /= 16;
            }
            self.push(b"0x");
            self.push(&digits[i..]);
        }
    }

    #[cfg(unix)]
    mod signals {
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;
        use std::sync::atomic::Ordering;

        use super::{OnceLock, Prepared};

        const SIGNALS: [libc::c_int; 5] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGABRT];

        /// What each signal was handled by before.
        static PREVIOUS: OnceLock<[libc::sigaction; SIGNALS.len()]> = OnceLock::new();
        static PREPARED: OnceLock<Prepared<Vec<u8>>> = OnceLock::new();

        pub fn install(dir: &Path, part: &'static str) {
            let Some(prepared) = super::prepare(dir, part, |d| d.as_os_str().as_bytes().to_vec()) else { return };
            let mut previous: [libc::sigaction; SIGNALS.len()] = unsafe { std::mem::zeroed() };
            for (signal, previous) in SIGNALS.iter().zip(previous.iter_mut()) {
                unsafe { libc::sigaction(*signal, std::ptr::null(), previous) };
            }
            if PREVIOUS.set(previous).is_err() || PREPARED.set(prepared).is_err() {
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
            if let Some(prepared) = PREPARED.get() {
                if !crate::crash::REPORTED.swap(true, Ordering::SeqCst) {
                    write_report(prepared, signal);
                }
            }
            // Hand the signal on, as if this handler weren't here. A fault happens again when this
            // returns, and the previous handler sees it as it was (Rust's needs the faulting address
            // to tell a stack overflow); a signal that was sent (abort, kill) is sent again.
            let (Some(previous), Some(i)) = (PREVIOUS.get(), SIGNALS.iter().position(|&s| s == signal)) else { return };
            let fault = signal != libc::SIGABRT && !info.is_null() && unsafe { (*info).si_code } > 0;
            unsafe {
                libc::sigaction(signal, &previous[i], std::ptr::null_mut());
                if !fault {
                    libc::raise(signal);
                }
            }
        }

        fn write_report(prepared: &Prepared<Vec<u8>>, signal: libc::c_int) {
            let mut now: libc::timespec = unsafe { std::mem::zeroed() };
            unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
            let time = now.tv_sec.max(0) as u64;
            let ([path, text, ..], _) = unsafe { super::scratch() };
            path.push(&prepared.dir);
            path.push(b"/");
            super::file_name(path, time);
            path.push(b"\0");
            report(text, prepared, signal, time);
            unsafe {
                let fd = libc::open(path.bytes.as_ptr() as *const libc::c_char, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644);
                if fd >= 0 {
                    let bytes = text.as_bytes();
                    libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
                    libc::close(fd);
                }
            }
        }

        fn report(out: &mut super::Buf, prepared: &Prepared<Vec<u8>>, signal: libc::c_int, time: u64) {
            let message = match signal {
                libc::SIGSEGV => b"Segmentation fault (SIGSEGV)".as_slice(),
                libc::SIGBUS => b"Bus error (SIGBUS)",
                libc::SIGILL => b"Illegal instruction (SIGILL)",
                libc::SIGFPE => b"Arithmetic error (SIGFPE)",
                libc::SIGABRT => b"Aborted (SIGABRT)",
                _ => b"Signal",
            };
            super::report(out, prepared.version.as_bytes(), prepared.part.as_bytes(), time, message, b"");
            out.push(b"\nNo backtrace (a crash outside Rust, e.g. in a graphics driver or system library)\n");
        }

        #[cfg(test)]
        #[test]
        fn the_report_reads_like_a_panics() {
            let prepared = Prepared { dir: Vec::new(), version: "RSnip 1.2.3 (debug, x86_64-linux)".into(), part: "library" };
            let mut out = super::Buf::new();
            report(&mut out, &prepared, libc::SIGSEGV, 1_792_764_312);
            let text = std::str::from_utf8(out.as_bytes()).unwrap();
            assert!(text.starts_with("RSnip crash report\n\nTime      2026-10-23 14:05 UTC\nVersion   RSnip 1.2.3 (debug, x86_64-linux)\n"), "{text}");
            assert_eq!(crate::crash::summary(text), "library: Segmentation fault (SIGSEGV)");
        }
    }

    #[cfg(windows)]
    mod exceptions {
        use std::os::windows::ffi::OsStrExt;
        use std::path::Path;
        use std::ptr::{null, null_mut};
        use std::sync::atomic::{AtomicUsize, Ordering};

        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_WRITE, HMODULE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, WriteFile, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL};
        use windows_sys::Win32::System::Diagnostics::Debug::{RtlCaptureStackBackTrace, SetUnhandledExceptionFilter, EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER};
        use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
        use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

        use super::{Buf, OnceLock, Prepared};

        /// Let Windows go on as without this filter (Windows Error Reporting, then the process ends).
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        /// Return addresses listed in a report.
        const FRAMES: usize = 32;

        static PREPARED: OnceLock<Prepared<Vec<u16>>> = OnceLock::new();
        /// The filter that was there before, as a function pointer (0 for none).
        static PREVIOUS: AtomicUsize = AtomicUsize::new(0);

        pub fn install(dir: &Path, part: &'static str) {
            let Some(prepared) = super::prepare(dir, part, |d| d.as_os_str().encode_wide().collect()) else { return };
            if PREPARED.set(prepared).is_err() {
                return;
            }
            let previous = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
            PREVIOUS.store(previous.map_or(0, |f| f as usize), Ordering::SeqCst);
        }

        unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
            if let Some(prepared) = PREPARED.get() {
                if !info.is_null() && !(*info).ExceptionRecord.is_null() && !crate::crash::REPORTED.swap(true, Ordering::SeqCst) {
                    write_report(prepared, &*(*info).ExceptionRecord);
                }
            }
            match PREVIOUS.load(Ordering::SeqCst) {
                0 => EXCEPTION_CONTINUE_SEARCH,
                previous => {
                    let previous: LPTOP_LEVEL_EXCEPTION_FILTER = std::mem::transmute(previous);
                    previous.map_or(EXCEPTION_CONTINUE_SEARCH, |f| f(info))
                }
            }
        }

        unsafe fn write_report(prepared: &Prepared<Vec<u16>>, record: &windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_RECORD) {
            let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            GetSystemTimeAsFileTime(&mut ft);
            // FILETIME counts 100 ns ticks since 1601.
            let ticks = (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64;
            let time = ticks.saturating_sub(116_444_736_000_000_000) / 10_000_000;

            let ([name, text, message, location], path) = super::scratch();
            name.push(b"\\");
            super::file_name(name, time);
            let dir = &prepared.dir[..prepared.dir.len().min(path.len() - name.len - 1)];
            path[..dir.len()].copy_from_slice(dir);
            for (i, b) in name.as_bytes().iter().enumerate() {
                path[dir.len() + i] = *b as u16;
            }

            let mut frames = [null_mut(); FRAMES];
            let captured = RtlCaptureStackBackTrace(0, FRAMES as u32, frames.as_mut_ptr(), null_mut()) as usize;
            let what = Exception { code: record.ExceptionCode as u32, address: record.ExceptionAddress as usize, frames: &frames[..captured] };
            report(text, message, location, prepared, time, &what);

            let file = CreateFileW(path.as_ptr(), GENERIC_WRITE, 0, null(), CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, null_mut());
            if file != INVALID_HANDLE_VALUE {
                let mut written = 0;
                WriteFile(file, text.bytes.as_ptr(), text.len as u32, &mut written, null_mut());
                CloseHandle(file);
            }
        }

        /// What went wrong: the exception code, where, and the return addresses on the stack.
        struct Exception<'a> {
            code: u32,
            address: usize,
            frames: &'a [*mut core::ffi::c_void],
        }

        /// The report into `out`, using `message` and `location` to put its lines together.
        fn report(out: &mut Buf, message: &mut Buf, location: &mut Buf, prepared: &Prepared<Vec<u16>>, time: u64, what: &Exception) {
            message.push(match what.code {
                0xC000_0005 => b"Access violation".as_slice(),
                0xC000_00FD => b"Stack overflow",
                0xC000_001D => b"Illegal instruction",
                0xC000_0094 => b"Integer division by zero",
                0xC000_0096 => b"Privileged instruction",
                0xC000_0006 => b"In-page error (a file it was reading went away)",
                0x8000_0003 => b"Breakpoint",
                _ => b"Exception",
            });
            message.push(b" (");
            message.hex(what.code as u64, 8);
            message.push(b")");
            module_offset(location, what.address);
            super::report(out, prepared.version.as_bytes(), prepared.part.as_bytes(), time, message.as_bytes(), location.as_bytes());
            out.push(b"\nAddresses (look these up with the release's rsnip.pdb)\n");
            for frame in what.frames {
                out.push(b"  ");
                module_offset(out, *frame as usize);
                out.push(b"\n");
            }
        }

        /// `rsnip.exe+0x1A2B3C`: the module an address is in and how far into it, which stays the
        /// same from run to run (the module's address doesn't).
        fn module_offset(out: &mut Buf, address: usize) {
            let mut module: HMODULE = null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if address == 0 || unsafe { GetModuleHandleExW(flags, address as *const u16, &mut module) } == 0 {
                out.hex(address as u64, 1);
                return;
            }
            let mut path = [0u16; 512];
            let len = unsafe { GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32) } as usize;
            let path = &path[..len.min(path.len())];
            let name = path.rsplit(|&c| c == b'\\' as u16 || c == b'/' as u16).next().unwrap_or(path);
            for &c in name {
                out.push(&[if c < 128 { c as u8 } else { b'?' }]);
            }
            out.push(b"+");
            out.hex((address - module as usize) as u64, 1);
        }

        #[cfg(test)]
        #[test]
        fn the_report_reads_like_a_panics() {
            let prepared = Prepared { dir: Vec::new(), version: "RSnip 1.2.3 (debug, x86_64-windows)".into(), part: "tray app" };
            let (mut out, mut message, mut location) = (Box::new(Buf::new()), Box::new(Buf::new()), Box::new(Buf::new()));
            let here = the_report_reads_like_a_panics as *mut core::ffi::c_void;
            let what = Exception { code: 0xC000_0005, address: 0, frames: &[here] };
            report(&mut out, &mut message, &mut location, &prepared, 1_792_764_312, &what);
            let text = std::str::from_utf8(out.as_bytes()).unwrap();
            assert!(text.starts_with("RSnip crash report\n\nTime      2026-10-23 14:05 UTC\nVersion   RSnip 1.2.3 (debug, x86_64-windows)\n"), "{text}");
            assert_eq!(crate::crash::summary(text), "tray app: Access violation (0xC0000005) (0x0)");
            let exe = std::env::current_exe().unwrap();
            let exe = exe.file_name().unwrap().to_str().unwrap();
            assert!(text.contains(&format!("\nAddresses (look these up with the release's rsnip.pdb)\n  {exe}+0x")), "{text}");
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
        let backtrace = "   0: rsnip::gallery::doc::render\n             at ./src/gallery/doc.rs:30:25\n   1: <unknown>\n             at ./x.rs:1:1\n   2: rsnip::main\n";
        assert_eq!(frames(backtrace), "  0: rsnip::gallery::doc::render\n        at ./src/gallery/doc.rs:30:25\n  2: rsnip::main\n");
        assert_eq!(frames("   0: <unknown>\n   1: <unknown>\n"), "", "a release build has none");
        let long: String = (0..100).map(|i| format!("  {i}: f{i}\n")).collect();
        assert_eq!(frames(&long).lines().count(), MAX_FRAMES);
    }

    #[test]
    fn frames_start_where_it_panicked() {
        let backtrace = "   0: rsnip::crash::install_into::{closure#0}\n   1: std::panicking::panic_with_hook\n   2: __rustc::rust_begin_unwind\n   3: core::panicking::panic_fmt\n   4: rsnip::gallery::doc::render\n             at ./src/gallery/doc.rs:3:1\n   5: core::panicking::assert_failed\n   6: rsnip::main\n";
        assert_eq!(frames(backtrace), "  6: rsnip::main\n", "after the last of Rust's panic frames");
        let backtrace = "   0: rsnip::crash::install_into::{closure#0}\n   1: core::panicking::panic_fmt\n   2: rsnip::gallery::doc::render\n   3: rsnip::main\n";
        assert_eq!(frames(backtrace), "  2: rsnip::gallery::doc::render\n  3: rsnip::main\n");
        assert_eq!(frames("   0: std::panicking::begin_panic\n").lines().count(), 1, "nothing else to show");
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

    /// Run by the tests below in a child process, which it crashes as `RSNIP_CRASH` says.
    #[test]
    fn crash_child() {
        let Some(dir) = std::env::var_os("RSNIP_CRASH_DIR") else { return };
        install_into(PathBuf::from(dir), "library");
        #[cfg(windows)]
        unsafe {
            // No "RSnip stopped working" dialog to wait on.
            use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_NOGPFAULTERRORBOX};
            SetErrorMode(SEM_NOGPFAULTERRORBOX);
        }
        match std::env::var("RSNIP_CRASH").as_deref() {
            Ok("panic") => panic!("boom"),
            // A real fault (as a driver bug would make), not a raised signal.
            _ => unsafe { std::ptr::write_volatile(std::hint::black_box(8usize) as *mut u8, 1) },
        }
        unreachable!("the crash ends the process");
    }

    /// Runs `crash_child` crashing as `how`, and returns how it ended and the reports it left.
    fn crash(how: &str) -> (std::process::ExitStatus, Vec<Crash>, PathBuf) {
        let dir = temp(&format!("child-{how}"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash::tests::crash_child", "--test-threads=1"])
            .env("RSNIP_CRASH_DIR", &dir)
            .env("RSNIP_CRASH", how)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        (status, list(&dir), dir)
    }

    #[test]
    fn a_panic_writes_a_report() {
        let (status, crashes, dir) = crash("panic");
        assert!(!status.success());
        assert_eq!(crashes.len(), 1, "{status:?}");
        assert!(crashes[0].summary.starts_with("library: boom (src"), "{}", crashes[0].summary);
        let text = std::fs::read_to_string(&crashes[0].path).unwrap();
        if cfg!(unix) || cfg!(debug_assertions) {
            assert!(text.contains("\nBacktrace\n") && text.contains("crash_child"), "named frames: {text}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fault_writes_a_report_and_still_ends_the_process() {
        let (status, crashes, dir) = crash("fault");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGSEGV), "it still ends as a segfault: {status:?}");
        }
        #[cfg(windows)]
        assert_eq!(status.code(), Some(0xC000_0005u32 as i32), "it still ends as an access violation: {status:?}");
        assert_eq!(crashes.len(), 1, "one report, not one per signal");
        let expected = if cfg!(windows) { "library: Access violation (0xC0000005) (rsnip" } else { "library: Segmentation fault (SIGSEGV)" };
        assert!(crashes[0].summary.starts_with(expected), "{}", crashes[0].summary);
        let _ = std::fs::remove_dir_all(dir);
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
