//! SMB shares (Windows file sharing, Samba, a NAS). On Windows, a copy to the share's
//! `\\server\share` path, signed in as the user given (else as you are). On Linux, Samba's
//! `smbclient`, with the password in its environment rather than on its command line where other
//! programs could see it.

use std::path::Path;

/// A folder on a share, and who to sign in as (no user: as you are on Windows, a guest on Linux).
pub struct Share<'a> {
    /// "\\nas\shots", "//nas/shots" or "smb://nas/shots", maybe with folders after it.
    pub share: &'a str,
    pub folder: &'a str,
    pub user: &'a str,
    pub password: &'a str,
}

/// The server, the share, and the folders below it to push into: those after the share in its
/// address, then `folder`'s. `None` if there's no server and share.
pub fn parse(share: &str, folder: &str) -> Option<(String, String, Vec<String>)> {
    let share = share.trim();
    let share = match share.get(..6) {
        Some(scheme) if scheme.eq_ignore_ascii_case("smb://") => &share[6..],
        _ => share,
    };
    let mut parts = share.split(['\\', '/']).filter(|p| !p.is_empty()).map(str::to_owned);
    let (server, name) = (parts.next()?, parts.next()?);
    let folders = parts.chain(folder.split(['\\', '/']).filter(|p| !p.is_empty()).map(str::to_owned)).collect();
    Some((server, name, folders))
}

pub const BAD_SHARE: &str = "Enter the share as \\\\server\\share";

/// Looks at the share and the folder, without changing anything.
pub fn test(s: &Share) -> Result<String, String> {
    platform::test(s)
}

/// Copies `file` into the folder as `name`, making the folder if need be.
pub fn put(s: &Share, name: &str, file: &Path) -> Result<(), String> {
    platform::put(s, name, file)
}

#[cfg(windows)]
mod platform {
    use std::io::ErrorKind;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::NetworkManagement::WNet::{WNetAddConnection2W, CONNECT_TEMPORARY, NETRESOURCEW, RESOURCETYPE_DISK};

    use super::{parse, Share, BAD_SHARE};

    pub fn test(s: &Share) -> Result<String, String> {
        let (root, dir) = connect(s)?;
        std::fs::read_dir(&root).map_err(|e| explain(&e))?;
        match std::fs::metadata(&dir) {
            Ok(m) if m.is_dir() => Ok("Connected".into()),
            Ok(_) => Err("The folder's name is taken by a file on the share".into()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok("Connected. The folder will be made with the first push".into()),
            Err(e) => Err(explain(&e)),
        }
    }

    pub fn put(s: &Share, name: &str, file: &Path) -> Result<(), String> {
        let (_, dir) = connect(s)?;
        std::fs::create_dir_all(&dir).map_err(|e| explain(&e))?;
        std::fs::copy(file, dir.join(name)).map(|_| ()).map_err(|e| explain(&e))
    }

    /// The share's `\\server\share` and the folder in it, signed in as the user if there's one.
    fn connect(s: &Share) -> Result<(PathBuf, PathBuf), String> {
        let (server, share, folders) = parse(s.share, s.folder).ok_or(BAD_SHARE)?;
        let root = PathBuf::from(format!(r"\\{server}\{share}"));
        let dir = folders.iter().fold(root.clone(), |dir, f| dir.join(f));
        if !s.user.trim().is_empty() {
            let wide = |text: &std::ffi::OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
            let mut remote = wide(root.as_os_str());
            let (password, user) = (wide(s.password.as_ref()), wide(s.user.trim().as_ref()));
            let resource = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpRemoteName: remote.as_mut_ptr(), ..unsafe { std::mem::zeroed() } };
            // For this session only: nothing is remembered for the next sign-in.
            let code = unsafe { WNetAddConnection2W(&resource, password.as_ptr(), user.as_ptr(), CONNECT_TEMPORARY) };
            if code != 0 {
                return Err(match code {
                    1219 => format!("Windows is already connected to {server} as someone else: leave the user empty to use that connection, or disconnect it first"),
                    _ => explain(&std::io::Error::from_raw_os_error(code as i32)),
                });
            }
        }
        Ok((root, dir))
    }

    /// Windows' complaint, as advice where it's a usual one.
    pub(super) fn explain(e: &std::io::Error) -> String {
        match e.raw_os_error() {
            // ERROR_LOGON_FAILURE, ERROR_INVALID_PASSWORD
            Some(1326 | 86) => "The server didn't accept the user name or password".into(),
            // ERROR_BAD_NETPATH, ERROR_BAD_NET_NAME
            Some(53) => "Couldn't reach the server: check its name, and that file sharing is on there".into(),
            Some(67) => "The server has no share by that name".into(),
            // ERROR_NETNAME_DELETED, ERROR_UNEXP_NET_ERR, ERROR_SEM_TIMEOUT, ERROR_NETWORK_UNREACHABLE, ERROR_HOST_UNREACHABLE
            Some(64 | 59 | 121 | 1231 | 1232) => "Lost touch with the server: check that it's on and reachable from this computer".into(),
            _ if e.kind() == std::io::ErrorKind::PermissionDenied => "The share didn't let RSnip in, or write there: check the user and what it may do".into(),
            _ => e.to_string(),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::{parse, Share, BAD_SHARE};

    pub fn test(s: &Share) -> Result<String, String> {
        let (server, share, folders) = parse(s.share, s.folder).ok_or(BAD_SHARE)?;
        let command = if folders.is_empty() { "ls".to_owned() } else { format!("cd {}", quote(&folders.join("/"))) };
        match run(s, &server, &share, &command) {
            Ok(()) => Ok("Connected".into()),
            Err(Some(out)) if out.contains("NT_STATUS_OBJECT_PATH_NOT_FOUND") || out.contains("NT_STATUS_OBJECT_NAME_NOT_FOUND") => {
                Ok("Connected. The folder will be made with the first push".into())
            }
            Err(out) => Err(explain(out.as_deref())),
        }
    }

    pub fn put(s: &Share, name: &str, file: &Path) -> Result<(), String> {
        let (server, share, folders) = parse(s.share, s.folder).ok_or(BAD_SHARE)?;
        run(s, &server, &share, &commands(&folders, name, file)).map_err(|out| explain(out.as_deref()))
    }

    /// smbclient's commands for a push: a mkdir for each level of the folder (one that's there
    /// already only complains), then the put. smbclient goes on after a failed command and exits
    /// with how the last one went, so the put is last.
    pub(super) fn commands(folders: &[String], name: &str, file: &Path) -> String {
        let mut commands: Vec<String> = (1..=folders.len()).map(|n| format!("mkdir {}", quote(&folders[..n].join("/")))).collect();
        let remote = folders.iter().cloned().chain([name.to_owned()]).collect::<Vec<_>>().join("/");
        commands.push(format!("put {} {}", quote(&file.to_string_lossy()), quote(&remote)));
        commands.join("; ")
    }

    /// A name for smbclient's commands, which have no way to quote a quote.
    fn quote(text: &str) -> String {
        format!("\"{}\"", text.replace('"', "_"))
    }

    /// Runs smbclient on the share; on failure, what it said (`None`: there's no smbclient).
    fn run(s: &Share, server: &str, share: &str, commands: &str) -> Result<(), Option<String>> {
        let program = program().ok_or(None)?;
        let mut command = Command::new(program);
        command.arg(format!("//{server}/{share}")).args(["-c", commands]).env("LC_ALL", "C");
        let user = s.user.trim();
        if user.is_empty() {
            command.arg("-N");
        } else {
            // smbclient reads the password from PASSWD when -U doesn't carry one.
            command.args(["-U", user]).env("PASSWD", s.password);
        }
        let out = command.stdin(Stdio::null()).output().map_err(|e| Some(e.to_string()))?;
        if out.status.success() {
            return Ok(());
        }
        Err(Some(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
    }

    /// smbclient's complaint, as advice where it's a usual one.
    pub(super) fn explain(out: Option<&str>) -> String {
        let Some(out) = out else { return "SMB needs Samba's smbclient: install the smbclient package (samba-client on Fedora)".into() };
        let has = |code: &str| out.contains(code);
        if has("NT_STATUS_LOGON_FAILURE") || has("NT_STATUS_WRONG_PASSWORD") {
            "The server didn't accept the user name or password".into()
        } else if has("NT_STATUS_BAD_NETWORK_NAME") {
            "The server has no share by that name".into()
        } else if has("NT_STATUS_ACCESS_DENIED") {
            "The share didn't let RSnip in, or write there: check the user and what it may do".into()
        } else if has("NT_STATUS_CONNECTION_RESET") || has("NT_STATUS_CONNECTION_DISCONNECTED") || has("NT_STATUS_CONNECTION_ABORTED") {
            "Lost touch with the server: check that it's on and reachable from this computer".into()
        } else if has("Connection to") || has("NT_STATUS_HOST_UNREACHABLE") || has("NT_STATUS_IO_TIMEOUT") || has("NT_STATUS_CONNECTION_REFUSED") {
            "Couldn't reach the server: check its name, and that file sharing is on there".into()
        } else {
            match out.lines().map(str::trim).rfind(|l| !l.is_empty()) {
                Some(line) => format!("smbclient: {line}"),
                None => "smbclient failed".into(),
            }
        }
    }

    fn program() -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|dir| dir.join("smbclient")).find(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_are_read_however_theyre_written() {
        let parsed = |share: &str, folder: &str| parse(share, folder).map(|(s, n, f)| format!("{s} {n} {}", f.join("/")));
        assert_eq!(parsed(r"\\nas\shots", "").as_deref(), Some("nas shots "));
        assert_eq!(parsed("//nas/shots/2026/", r"rsnip\today").as_deref(), Some("nas shots 2026/rsnip/today"));
        assert_eq!(parsed("SMB://192.168.1.5/Public", "/x/").as_deref(), Some("192.168.1.5 Public x"));
        assert_eq!(parsed(r"\\nas", "x"), None, "a server alone isn't a share");
        assert_eq!(parsed("", ""), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn makes_each_level_then_puts_last() {
        let folders = ["a b".to_owned(), "c".to_owned()];
        assert_eq!(
            platform::commands(&folders, "Shot \"1\".png", Path::new("/home/me/RSnip/Shot 1.png")),
            "mkdir \"a b\"; mkdir \"a b/c\"; put \"/home/me/RSnip/Shot 1.png\" \"a b/c/Shot _1_.png\""
        );
        assert_eq!(platform::commands(&[], "a.png", Path::new("/a.png")), "put \"/a.png\" \"a.png\"");
    }

    #[cfg(not(windows))]
    #[test]
    fn explains_the_usual_failures() {
        assert_eq!(platform::explain(Some("session setup failed: NT_STATUS_LOGON_FAILURE\n")), "The server didn't accept the user name or password");
        assert_eq!(platform::explain(Some("tree connect failed: NT_STATUS_BAD_NETWORK_NAME")), "The server has no share by that name");
        assert!(platform::explain(Some("do_connect: Connection to nohost failed (Error NT_STATUS_NOT_FOUND)")).starts_with("Couldn't reach"));
        assert!(platform::explain(Some("do_connect: Connection to nas failed (Error NT_STATUS_CONNECTION_RESET)")).starts_with("Lost touch"));
        assert!(platform::explain(None).contains("install the smbclient package"));
    }

    #[cfg(windows)]
    #[test]
    fn explains_the_usual_failures() {
        let error = |code| platform::explain(&std::io::Error::from_raw_os_error(code));
        assert_eq!(error(1326), "The server didn't accept the user name or password");
        assert_eq!(error(67), "The server has no share by that name");
        assert!(error(64).starts_with("Lost touch with the server"));
        assert!(error(5).starts_with("The share didn't let RSnip in"));
    }
}
