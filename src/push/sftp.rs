//! SFTP: runs the system's OpenSSH `sftp` (built into Windows 10 and 11, and on every Linux
//! desktop) in batch mode, so it signs in with the user's own keys, ssh-agent and `~/.ssh/config`.
//! RSnip never asks for or keeps an SSH password: a server that needs one fails the test, saying so.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A folder on an SSH server.
pub struct Server<'a> {
    pub host: &'a str,
    pub port: u16,
    /// Empty: whatever `~/.ssh/config` or the local user name says.
    pub user: &'a str,
    /// "uploads/screenshots", from the home folder, or "/srv/www/shots".
    pub folder: &'a str,
}

/// Signs in and looks at the folder, without changing anything.
pub fn test(s: &Server) -> Result<String, String> {
    let folder = s.folder.trim_end_matches('/');
    let batch = if folder.is_empty() { "pwd\n".to_owned() } else { format!("cd {}\n", quote(folder)) };
    match run(s, &batch) {
        Ok(()) => Ok("Connected".into()),
        // The folder isn't there yet: it's made with the first push.
        Err(e) if e.contains("No such file") || e.contains("not found") => Ok("Connected. The folder will be made with the first push".into()),
        Err(e) => Err(e),
    }
}

/// Uploads `file` into the folder as `name`, making the folder (a level at a time) if need be.
pub fn put(s: &Server, name: &str, file: &Path) -> Result<(), String> {
    run(s, &batch(s.folder, name, file))
}

/// The commands `put` sends: a "-mkdir" (failures ignored) for each level of the folder, then the put.
fn batch(folder: &str, name: &str, file: &Path) -> String {
    let folder = folder.trim_end_matches('/');
    let mut batch = String::new();
    let mut so_far = String::new();
    for (i, part) in folder.split('/').enumerate() {
        if i > 0 {
            so_far.push('/');
        }
        so_far.push_str(part);
        if !part.is_empty() && part != "~" {
            batch.push_str(&format!("-mkdir {}\n", quote(&so_far)));
        }
    }
    let remote = if folder.is_empty() { name.to_owned() } else { format!("{folder}/{name}") };
    batch.push_str(&format!("put {} {}\n", quote(&file.to_string_lossy()), quote(&remote)));
    batch
}

/// Runs `sftp` with `batch` on its standard input, and says what went wrong in words.
fn run(s: &Server, batch: &str) -> Result<(), String> {
    let program = program().ok_or(MISSING)?;
    let target = if s.user.is_empty() { s.host.to_owned() } else { format!("{}@{}", s.user, s.host) };
    let mut command = Command::new(program);
    // No questions: there's no terminal to answer them in. A host it hasn't met is added to
    // known_hosts; one whose key has changed is refused.
    command.args(["-b", "-", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=15"]);
    command.args(["-P", &s.port.to_string(), "--", &target]);
    command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up from the tray app.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("Couldn't run sftp: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(batch.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("sftp failed: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(explain(&String::from_utf8_lossy(&out.stderr)))
}

const MISSING: &str = "SFTP needs OpenSSH's sftp. On Windows: Settings → System → Optional features → OpenSSH Client; on Linux, the openssh-client package";

/// sftp's complaint, as advice where it's a usual one.
fn explain(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("sftp>")).collect();
    let all = lines.join(" ");
    if all.contains("Permission denied (") {
        "The server didn't accept your SSH keys. RSnip uses your keys and ssh-agent, not passwords: add a key to the server (ssh-copy-id) and check it with ssh in a terminal".into()
    } else if all.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        "The server's host key has changed since you last connected. If that's expected, remove its old key with ssh-keygen -R <host>".into()
    } else if all.contains("Could not resolve hostname") {
        "Couldn't find that server: check the host name".into()
    } else if all.contains("Connection refused") || all.contains("timed out") {
        "Couldn't connect to the server: check the host and port, and that SSH is running there".into()
    } else {
        match lines.last() {
            Some(last) => format!("sftp: {last}"),
            None => "sftp failed".into(),
        }
    }
}

/// A path for sftp's batch commands, quoted.
fn quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// sftp: on the PATH, or where Windows installs it.
fn program() -> Option<PathBuf> {
    let name = if cfg!(windows) { "sftp.exe" } else { "sftp" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        dirs.push(root.join(r"System32\OpenSSH"));
    }
    dirs.into_iter().map(|dir| dir.join(name)).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_each_level_then_puts() {
        let file = Path::new("/home/a/RSnip/Screenshot 2026-10-24 091500.png");
        assert_eq!(
            batch("uploads/shots/", "Screenshot 2026-10-24 091500.png", file),
            "-mkdir \"uploads\"\n-mkdir \"uploads/shots\"\nput \"/home/a/RSnip/Screenshot 2026-10-24 091500.png\" \"uploads/shots/Screenshot 2026-10-24 091500.png\"\n"
        );
        assert_eq!(batch("/srv/www", "a.png", Path::new("a.png")), "-mkdir \"/srv\"\n-mkdir \"/srv/www\"\nput \"a.png\" \"/srv/www/a.png\"\n");
        assert_eq!(batch("", "a \"b\".png", Path::new("C:\\x\\a.png")), "put \"C:\\\\x\\\\a.png\" \"a \\\"b\\\".png\"\n");
    }

    #[test]
    fn explains_the_usual_failures() {
        assert!(explain("user@h: Permission denied (publickey,password).\r\nConnection closed").contains("ssh-copy-id"));
        assert!(explain("ssh: Could not resolve hostname nope: Name or service not known").contains("host name"));
        assert_eq!(explain("sftp> put a b\nremote open(\"b\"): Permission denied\n"), "sftp: remote open(\"b\"): Permission denied");
    }
}
