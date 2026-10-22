//! Checks GitHub for a newer RSnip release and, on Windows, downloads and runs its installer.

use std::fmt;

const LATEST: &str = "https://api.github.com/repos/dhiorns15/rust-screenshot/releases/latest";
/// How the installer is named among a release's downloads.
const INSTALLER_SUFFIX: &str = "-windows-x64-setup.exe";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parses "1.2.3" or "v1.2.3". A pre-release or build suffix ("-beta", "+abc") is ignored.
    pub fn parse(s: &str) -> Option<Version> {
        let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
        let parts: Vec<u32> = core.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        match parts[..] {
            [major, minor, patch] => Some(Version(major, minor, patch)),
            [major, minor] => Some(Version(major, minor, 0)),
            _ => None,
        }
    }

    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is x.y.z")
    }

    /// Packed into one number, for remembering which version the user was told about.
    #[cfg(windows)]
    pub fn pack(self) -> u32 {
        (self.0.min(1023) << 20) | (self.1.min(1023) << 10) | self.2.min(1023)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    /// The release's web page.
    pub page: String,
    /// The Windows installer, if the release has one.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub installer: Option<Download>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Download {
    pub url: String,
    /// Lowercase hex SHA-256, as GitHub reports it.
    pub sha256: Option<String>,
}

/// Reads GitHub's description of a release.
pub fn parse(json: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("Unexpected reply from GitHub: no version")?;
    let version = Version::parse(tag).ok_or_else(|| format!("Unrecognised version {tag}"))?;
    let page = v["html_url"].as_str().unwrap_or("https://github.com/dhiorns15/rust-screenshot/releases").to_owned();
    let installer = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(INSTALLER_SUFFIX)))
        .and_then(|a| {
            Some(Download {
                url: a["browser_download_url"].as_str()?.to_owned(),
                sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase),
            })
        });
    Ok(Release { version, page, installer })
}

/// The latest release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let release = parse(&get(LATEST)?)?;
    Ok((release.version > Version::current()).then_some(release))
}

#[cfg(windows)]
fn get(url: &str) -> Result<Vec<u8>, String> {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Networking::WinHttp::*;

    use crate::util::wide;

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let error = || format!("Couldn't reach GitHub ({})", std::io::Error::last_os_error());

    unsafe {
        let url_w = wide(url);
        let mut host = [0u16; 256];
        let mut path = [0u16; 2048];
        let mut parts: URL_COMPONENTS = zeroed();
        parts.dwStructSize = size_of::<URL_COMPONENTS>() as u32;
        parts.lpszHostName = host.as_mut_ptr();
        parts.dwHostNameLength = host.len() as u32;
        parts.lpszUrlPath = path.as_mut_ptr();
        parts.dwUrlPathLength = path.len() as u32;
        if WinHttpCrackUrl(url_w.as_ptr(), 0, 0, &mut parts) == 0 || parts.nScheme != WINHTTP_INTERNET_SCHEME_HTTPS {
            return Err(format!("Not an https URL: {url}"));
        }

        let agent = wide(concat!("RSnip/", env!("CARGO_PKG_VERSION")));
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(error());
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000);
        let connect = Handle(WinHttpConnect(session.0, host.as_ptr(), parts.nPort, 0));
        if connect.0.is_null() {
            return Err(error());
        }
        let verb = wide("GET");
        // Redirects (downloads are served from another host) are followed automatically.
        let request =
            Handle(WinHttpOpenRequest(connect.0, verb.as_ptr(), path.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE));
        if request.0.is_null()
            || WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(error());
        }
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut c_void,
            &mut len,
            null_mut(),
        );
        if status != 200 {
            return Err(format!("GitHub replied with HTTP {status}"));
        }
        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read) == 0 {
                return Err(error());
            }
            if read == 0 {
                return Ok(body);
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
    }
}

#[cfg(not(windows))]
fn get(url: &str) -> Result<Vec<u8>, String> {
    let agent = concat!("RSnip/", env!("CARGO_PKG_VERSION"));
    // Some distributions (e.g. Ubuntu) come with wget but not curl.
    get_with(&[
        ("curl", &["-fsSL", "--max-time", "30", "-A", agent, url]),
        ("wget", &["-q", "-O", "-", "--timeout=30", "-U", agent, url]),
    ])
}

/// Runs the first of `tools` that's installed, and returns what it printed.
#[cfg(not(windows))]
fn get_with(tools: &[(&str, &[&str])]) -> Result<Vec<u8>, String> {
    for (program, args) in tools {
        let out = match std::process::Command::new(program).args(*args).output() {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Couldn't run {program}: {e}")),
        };
        if !out.status.success() {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let detail = if detail.is_empty() { format!("{program} failed ({})", out.status) } else { detail };
            return Err(format!("Couldn't reach GitHub: {detail}"));
        }
        return Ok(out.stdout);
    }
    let names: Vec<&str> = tools.iter().map(|(program, _)| *program).collect();
    Err(format!("Install {} to check for updates", names.join(" or ")))
}

#[cfg(windows)]
pub use install::*;

/// Updating an installed copy: download the new installer and run it silently. The installer closes
/// RSnip, replaces it, and starts the new version (`/relaunch=yes`, see installer/rsnip.iss).
#[cfg(windows)]
mod install {
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{get, Release};
    use crate::util::{wide, wide_os};

    /// True if this copy came from the installer (so it can update itself), rather than being run
    /// standalone.
    pub fn installed() -> bool {
        std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("unins000.exe").exists())).unwrap_or(false)
    }

    /// Downloads the release's installer to the temp folder, checked against GitHub's SHA-256.
    pub fn download(release: &Release) -> Result<PathBuf, String> {
        let installer = release.installer.as_ref().ok_or("This release has no installer")?;
        let expected = installer.sha256.as_deref().ok_or("GitHub gave no checksum for the installer")?;
        let bytes = get(&installer.url)?;
        if sha256_hex(&bytes) != expected {
            return Err("The download was damaged (its checksum doesn't match)".into());
        }
        let path = std::env::temp_dir().join(format!("rsnip-v{}-setup.exe", release.version));
        std::fs::write(&path, bytes).map_err(|e| format!("Couldn't save the installer: {e}"))?;
        Ok(path)
    }

    pub(super) fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Asks the library window to close, so its unsaved edits can be saved, and waits for it.
    /// False if it is still open after `timeout` (the user kept editing).
    pub fn close_library(timeout: Duration) -> bool {
        let title = wide(crate::gallery::TITLE);
        let start = Instant::now();
        unsafe {
            let window = FindWindowW(null(), title.as_ptr());
            if window.is_null() {
                return true;
            }
            PostMessageW(window, WM_CLOSE, 0, 0);
            while start.elapsed() < timeout {
                if FindWindowW(null(), title.as_ptr()).is_null() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        false
    }

    pub fn run_installer(path: &Path) -> Result<(), String> {
        let (verb, file) = (wide("open"), wide_os(path.as_os_str()));
        let args = wide("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /relaunch=yes");
        let result = unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), args.as_ptr(), null(), SW_SHOWNORMAL) };
        if result as isize > 32 {
            Ok(())
        } else {
            Err(format!("Couldn't start the installer ({})", std::io::Error::last_os_error()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn downloads_with_the_first_tool_installed() {
        let missing = "rsnip-no-such-tool";
        assert_eq!(get_with(&[(missing, &[]), ("echo", &["hi"])]).unwrap(), b"hi\n");
        assert_eq!(get_with(&[(missing, &[]), ("rsnip-nor-this", &[])]).unwrap_err(), format!("Install {missing} or rsnip-nor-this to check for updates"));
        assert!(get_with(&[("false", &[])]).unwrap_err().starts_with("Couldn't reach GitHub: false failed"));
    }

    #[test]
    fn parses_and_orders_versions() {
        assert_eq!(Version::parse("v0.2.1"), Some(Version(0, 2, 1)));
        assert_eq!(Version::parse("1.10"), Some(Version(1, 10, 0)));
        assert_eq!(Version::parse("2.0.0-beta.1"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("v1"), None);
        assert_eq!(Version::parse("1.x.0"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 3, 1).to_string(), "0.3.1");
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[cfg(windows)]
    #[test]
    fn packs_versions() {
        assert_eq!(Version(0, 3, 1).pack(), (3 << 10) | 1);
        assert!(Version(1, 0, 0).pack() > Version(0, 1023, 1023).pack());
    }

    const RELEASE: &str = r#"{
        "tag_name": "v0.3.0",
        "html_url": "https://github.com/dhiorns15/rust-screenshot/releases/tag/v0.3.0",
        "assets": [
            {"name": "rsnip-v0.3.0-linux-x64.tar.gz", "browser_download_url": "https://example.com/linux", "digest": "sha256:aa"},
            {"name": "rsnip-v0.3.0-windows-x64-setup.exe", "browser_download_url": "https://example.com/setup", "digest": "sha256:ABCDEF"},
            {"name": "rsnip.exe", "browser_download_url": "https://example.com/exe", "digest": "sha256:bb"}
        ]
    }"#;

    #[test]
    fn parses_releases() {
        let r = parse(RELEASE.as_bytes()).unwrap();
        assert_eq!(r.version, Version(0, 3, 0));
        assert!(r.page.ends_with("/tag/v0.3.0"));
        let installer = r.installer.unwrap();
        assert_eq!(installer.url, "https://example.com/setup");
        assert_eq!(installer.sha256.as_deref(), Some("abcdef"));

        let r = parse(br#"{"tag_name": "v0.3.0", "assets": [{"name": "rsnip-v0.3.0-windows-x64-setup.exe", "browser_download_url": "u"}]}"#).unwrap();
        assert_eq!(r.installer.unwrap().sha256, None);
        assert!(parse(br#"{"tag_name": "v0.3.0", "assets": []}"#).unwrap().installer.is_none());
        assert!(parse(br#"{"message": "Not Found"}"#).is_err());
        assert!(parse(b"<html>").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn hashes_sha256() {
        assert_eq!(install::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    proptest::proptest! {
        #[test]
        fn versions_order_like_their_numbers(a in proptest::prelude::any::<(u32, u32, u32)>(), b in proptest::prelude::any::<(u32, u32, u32)>()) {
            let (va, vb) = (Version(a.0, a.1, a.2), Version(b.0, b.1, b.2));
            proptest::prop_assert_eq!(va.cmp(&vb), a.cmp(&b));
            proptest::prop_assert_eq!(Version::parse(&format!("v{va}")), Some(va));
        }

        #[test]
        fn parsing_never_panics(s in ".{0,20}") {
            let _ = Version::parse(&s);
        }

        #[test]
        fn release_parsing_never_panics(json in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200)) {
            let _ = parse(&json);
        }
    }

    /// Talks to GitHub, so it only runs on request: `cargo test -- --ignored latest_release`.
    #[test]
    #[ignore]
    fn latest_release() {
        let release = parse(&get(LATEST).unwrap()).unwrap();
        println!("latest: {} at {}", release.version, release.page);
        #[cfg(windows)]
        {
            let path = download(&release).unwrap();
            println!("installer verified: {}", path.display());
            std::fs::remove_file(path).unwrap();
        }
    }
}
