//! FTPS (FTP over TLS): runs the system's `curl` (built into Windows 10 and 11, and on Linux
//! desktops), as SFTP runs OpenSSH's `sftp`. The user name and password go to curl on its
//! standard input, never on its command line where other programs could see them. Plain FTP,
//! which would send the password as it is, isn't offered.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::encode;

/// A folder on an FTPS server, and how to sign in.
pub struct Server<'a> {
    pub host: &'a str,
    /// 21 for explicit TLS (AUTH TLS), the usual; 990 for implicit.
    pub port: u16,
    pub user: &'a str,
    pub password: &'a str,
    /// "uploads/shots", from the folder the server starts in, or "/srv/www/shots".
    pub folder: &'a str,
}

/// Signs in and lists the folder, without changing anything.
pub fn test(s: &Server) -> Result<String, String> {
    match run(s, &url(s, ""), None) {
        Ok(()) => Ok("Connected".into()),
        // The folder isn't there yet: it's made with the first push.
        Err((9 | 78, _)) => Ok("Connected. The folder will be made with the first push".into()),
        Err((code, stderr)) => Err(explain(code, &stderr)),
    }
}

/// Uploads `file` into the folder as `name`, making the folder if need be.
pub fn put(s: &Server, name: &str, file: &Path) -> Result<(), String> {
    run(s, &url(s, name), Some(file)).map_err(|(code, stderr)| explain(code, &stderr))
}

/// The address of `name` in the folder (the folder itself if `name` is empty, ending in "/").
fn url(s: &Server, name: &str) -> String {
    let scheme = if s.port == 990 { "ftps" } else { "ftp" };
    let host = if s.host.contains(':') && !s.host.starts_with('[') { format!("[{}]", s.host) } else { s.host.trim().to_owned() };
    let folder = s.folder.trim();
    // A leading %2F makes the path start at the server's root rather than where it starts.
    let mut path = if folder.starts_with('/') { "/%2F".to_owned() } else { "/".to_owned() };
    for part in folder.split('/').filter(|p| !p.is_empty()) {
        path.push_str(&encode(part));
        path.push('/');
    }
    path.push_str(&encode(name));
    format!("{scheme}://{host}:{}{path}", s.port)
}

/// curl's arguments: with `file`, an upload; without, a listing.
fn args(s: &Server, file: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = ["--silent", "--show-error", "--connect-timeout", "15", "--config", "-"].map(String::from).to_vec();
    if s.port != 990 {
        // TLS or nothing: never the password in the clear.
        args.push("--ssl-reqd".into());
    }
    if cfg!(windows) {
        // Windows' TLS checks whether the certificate was revoked, which fails when it can't
        // reach the authority that says so; this checks when it can.
        args.push("--ssl-revoke-best-effort".into());
    }
    match file {
        Some(file) => args.extend(["--ftp-create-dirs".into(), "--upload-file".into(), file.to_string_lossy().into_owned()]),
        None => args.push("--list-only".into()),
    }
    args
}

/// What curl reads on its standard input: the address and who to sign in as.
fn config(s: &Server, url: &str) -> String {
    let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    format!("url = {}\nuser = {}\n", quoted(url), quoted(&format!("{}:{}", s.user, s.password)))
}

/// Runs curl; on failure, its exit code and what it said.
fn run(s: &Server, url: &str, file: Option<&Path>) -> Result<(), (i32, String)> {
    let program = program().ok_or((-1, String::new()))?;
    let mut command = Command::new(program);
    command.args(args(s, file)).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up from the tray app.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| (-2, e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(config(s, url).as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| (-2, e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    Err((out.status.code().unwrap_or(-2), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// curl's exit code as advice, where it's a usual one.
fn explain(code: i32, stderr: &str) -> String {
    match code {
        -1 => "FTPS needs curl: it's built into Windows 10 and 11; on Linux, install the curl package".into(),
        6 => "Couldn't find that server: check its name".into(),
        7 | 28 => "Couldn't connect to the server: check its name and port, and that FTPS is on there".into(),
        67 => "The server didn't accept the user name or password".into(),
        64 => "The server doesn't offer FTP over TLS. RSnip won't send your password unencrypted".into(),
        60 => "The server's certificate isn't one this computer trusts (a self-signed one, say): give the server one from a known authority, or trust it in the system's settings".into(),
        9 | 25 | 78 => "The server didn't let RSnip write there: check the folder and that the user may write to it".into(),
        _ => match stderr.lines().map(str::trim).rfind(|l| !l.is_empty()) {
            Some(line) => line.to_owned(),
            None => format!("curl failed ({code})"),
        },
    }
}

/// curl: Windows' own, else on the PATH.
fn program() -> Option<PathBuf> {
    let name = if cfg!(windows) { "curl.exe" } else { "curl" };
    let mut dirs = Vec::new();
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        dirs.push(root.join("System32"));
    }
    dirs.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
    dirs.into_iter().map(|dir| dir.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server<'a>(port: u16, folder: &'a str) -> Server<'a> {
        Server { host: "nas.local", port, user: "me", password: "p\"w\\1", folder }
    }

    #[test]
    fn addresses_start_where_the_server_does_unless_absolute() {
        assert_eq!(url(&server(21, "shots/today/"), "a b.png"), "ftp://nas.local:21/shots/today/a%20b.png");
        assert_eq!(url(&server(2121, "/srv/www"), "a.png"), "ftp://nas.local:2121/%2Fsrv/www/a.png");
        assert_eq!(url(&server(990, ""), ""), "ftps://nas.local:990/");
        assert_eq!(url(&Server { host: "fe80::1", ..server(21, "") }, ""), "ftp://[fe80::1]:21/");
    }

    #[test]
    fn tls_is_required_and_the_password_goes_on_stdin() {
        let explicit = args(&server(21, ""), Some(Path::new("a.png")));
        assert!(explicit.contains(&"--ssl-reqd".to_owned()) && explicit.ends_with(&["--ftp-create-dirs".into(), "--upload-file".into(), "a.png".into()]));
        assert!(!explicit.iter().any(|a| a.contains("p\"w")), "never on the command line");
        assert!(!args(&server(990, ""), None).contains(&"--ssl-reqd".to_owned()), "implicit TLS is TLS from the start");
        assert!(args(&server(21, ""), None).ends_with(&["--list-only".into()]));
        assert_eq!(config(&server(21, ""), "ftp://h:21/"), "url = \"ftp://h:21/\"\nuser = \"me:p\\\"w\\\\1\"\n");
    }

    #[test]
    fn explains_the_usual_failures() {
        assert_eq!(explain(67, "curl: (67) Access denied: 530"), "The server didn't accept the user name or password");
        assert!(explain(64, "").contains("won't send your password unencrypted"));
        assert_eq!(explain(55, "curl: (55) Failed sending data\n"), "curl: (55) Failed sending data");
    }
}
