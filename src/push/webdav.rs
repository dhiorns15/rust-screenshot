//! WebDAV (Nextcloud, ownCloud, Synology and QNAP NAS, Box, pCloud, ...): a PUT into a folder,
//! made first (a level at a time) if it isn't there.

use std::path::Path;

use crate::http::{self, Body, Response};

/// A folder on a WebDAV server, and who to sign in as.
pub struct Folder<'a> {
    /// "https://cloud.example.com/remote.php/dav/files/me/Screenshots"
    pub url: &'a str,
    pub user: &'a str,
    pub password: &'a str,
}

/// Checks the address and the sign-in, without changing anything.
pub fn test(f: &Folder) -> Result<String, String> {
    let reply = send(f, "PROPFIND", &folder(f.url), &[("Depth", "0")], Body::Empty)?;
    match reply.status {
        200..=299 => Ok("Connected".into()),
        404 => Ok("Connected. The folder will be made with the first push".into()),
        _ => Err(problem(reply)),
    }
}

/// Uploads `file` into the folder as `name`, making the folder if need be.
pub fn put(f: &Folder, name: &str, file: &Path) -> Result<(), String> {
    let url = format!("{}{}", folder(f.url), super::encode(name));
    let content_type = [("Content-Type", http::content_type(file))];
    let mut reply = send(f, "PUT", &url, &content_type, Body::File(file.to_path_buf()))?;
    // 409 Conflict: the folder isn't there.
    if reply.status == 409 {
        make_folder(f, &folder(f.url), 0)?;
        reply = send(f, "PUT", &url, &content_type, Body::File(file.to_path_buf()))?;
    }
    if reply.is_success() { Ok(()) } else { Err(problem(reply)) }
}

/// Makes `url`, and the folders above it that are missing.
fn make_folder(f: &Folder, url: &str, depth: usize) -> Result<(), String> {
    let reply = send(f, "MKCOL", url, &[], Body::Empty)?;
    match reply.status {
        // 405: it's there already.
        200..=299 | 405 => Ok(()),
        409 if depth < 16 => {
            let parent = parent(url).ok_or("Couldn't make the folder on the server")?;
            make_folder(f, &parent, depth + 1)?;
            make_folder(f, url, depth + 1)
        }
        _ => Err(problem(reply)),
    }
}

fn send(f: &Folder, method: &str, url: &str, extra: &[(&str, &str)], body: Body) -> Result<Response, String> {
    let authorization = format!("Basic {}", base64(format!("{}:{}", f.user, f.password).as_bytes()));
    let mut headers = vec![("Authorization", authorization.as_str())];
    headers.extend_from_slice(extra);
    http::request(method, url, &headers, body)
}

/// What a failed reply means, in words.
fn problem(reply: Response) -> String {
    match reply.status {
        401 => "The server didn't accept the user name or password".into(),
        403 => "The server doesn't allow uploading to that folder".into(),
        507 => "The server is out of space".into(),
        _ => reply.check("The WebDAV server").err().unwrap_or_default(),
    }
}

/// The folder's address, ending in one slash.
fn folder(url: &str) -> String {
    format!("{}/", url.trim_end_matches('/'))
}

/// The folder above a folder's address ("https://h/a/b/" is "https://h/a/"), short of the server.
fn parent(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://")?;
    let (host, path) = rest.split_once('/')?;
    let up = path.rsplit_once('/').map_or("", |(up, _)| up);
    Some(if up.is_empty() { format!("{scheme}://{host}/") } else { format!("{scheme}://{host}/{up}/") })
}

/// Standard base64 with padding (RFC 4648), for Basic sign-in.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_folders() {
        // RFC 4648's examples.
        for (text, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(text.as_bytes()), encoded);
        }
        assert_eq!(folder("https://h/dav/Shots"), "https://h/dav/Shots/");
        assert_eq!(folder("https://h/dav/Shots//"), "https://h/dav/Shots/");
        assert_eq!(parent("https://h/dav/Shots/").as_deref(), Some("https://h/dav/"));
        assert_eq!(parent("https://h/dav/").as_deref(), Some("https://h/"));
        assert_eq!(parent("https://h/"), None);
    }
}
