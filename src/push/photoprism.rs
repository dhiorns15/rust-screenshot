//! PhotoPrism: uploads the file as PhotoPrism's own web upload does (into the user's upload
//! folder, then imported, into an album if one is named), signed in with an app password.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::http::{self, Body, Multipart};

/// Checks the server and the app password, and says whose it is.
pub fn test(server: &str, password: &str) -> Result<String, String> {
    let (_, name) = user(server, password)?;
    Ok(format!("Connected as {name}"))
}

/// Uploads `file` and imports it, into `album` (by its title, made if need be) unless that's
/// empty. PhotoPrism doesn't say which photo it became, so there's no link.
pub fn upload(server: &str, password: &str, album: &str, file: &Path) -> Result<Option<String>, String> {
    let server = server.trim_end_matches('/');
    let (uid, _) = user(server, password)?;
    // Names this upload's folder on the server.
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let path = format!("{server}/api/v1/users/{uid}/upload/rsnip{nanos:x}");
    let bearer = format!("Bearer {password}");
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let (content_type, body) = Multipart::new().file("files", &name, http::content_type(file), file).finish();
    http::request("POST", &path, &[("Authorization", &bearer), ("Content-Type", &content_type)], body)?.check("PhotoPrism")?;
    let albums: Vec<&str> = Some(album.trim()).filter(|a| !a.is_empty()).into_iter().collect();
    let import = json!({ "albums": albums }).to_string().into_bytes();
    http::request("PUT", &path, &[("Authorization", &bearer), ("Content-Type", "application/json")], Body::Bytes(import))?.check("PhotoPrism")?;
    Ok(None)
}

/// The app password's user: its id, and its name to show.
fn user(server: &str, password: &str) -> Result<(String, String), String> {
    let url = format!("{}/api/v1/session", server.trim_end_matches('/'));
    let reply = http::request("GET", &url, &[("Authorization", &format!("Bearer {password}"))], Body::Empty)?;
    match reply.status {
        200..=299 => {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            let text = |key: &str| json["user"][key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
            let uid = text("UID").ok_or("That doesn't look like a PhotoPrism server (its session has no user)")?;
            Ok((uid, text("DisplayName").or(text("Name")).unwrap_or_default()))
        }
        401 => Err("PhotoPrism didn't accept the app password".into()),
        403 => Err("The app password needs the files and sessions scopes (and albums, to put photos in an album), or full access".into()),
        404 => Err("That isn't a PhotoPrism server (no /api/v1/session there)".into()),
        _ => Err(reply.check("PhotoPrism").err().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    #[test]
    fn uploads_then_imports_into_the_album() {
        let file = std::env::temp_dir().join(format!("rsnip-photoprism-{}", std::process::id())).join("a b.png");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"png bytes").unwrap();
        let session = r#"{"user":{"UID":"u1","Name":"me","DisplayName":""}}"#;
        let ok = r#"{"code":200}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", session.into()), ("200 OK", "", ok.into()), ("200 OK", "", ok.into())]);
        assert_eq!(upload(&format!("{server}/"), "app-pw", " Screenshots ", &file), Ok(None));
        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/api/v1/session");
        assert_eq!(requests[0].header("Authorization"), Some("Bearer app-pw"));
        let (post, put) = (&requests[1], &requests[2]);
        assert!(post.path.starts_with("/api/v1/users/u1/upload/rsnip") && put.path == post.path, "{} then {}", post.path, put.path);
        assert!(String::from_utf8_lossy(&post.body).contains("name=\"files\"; filename=\"a b.png\""));
        assert_eq!((put.method.as_str(), put.body.as_slice()), ("PUT", br#"{"albums":["Screenshots"]}"#.as_slice()));

        let (server, requests) = serve_each(vec![("200 OK", "", session.into()), ("200 OK", "", ok.into()), ("200 OK", "", ok.into())]);
        upload(&server, "app-pw", "", &file).unwrap();
        assert_eq!(requests.join().unwrap()[2].body, br#"{"albums":[]}"#, "no album");
    }

    #[test]
    fn says_whose_password_and_what_it_lacks() {
        let (server, _) = serve_each(vec![("200 OK", "", r#"{"user":{"UID":"u1","Name":"me","DisplayName":"Me Too"}}"#.into())]);
        assert_eq!(test(&server, "pw"), Ok("Connected as Me Too".into()));
        let (server, _) = serve_each(vec![("401 Unauthorized", "", String::new())]);
        assert_eq!(test(&server, "pw"), Err("PhotoPrism didn't accept the app password".into()));
        let (server, _) = serve_each(vec![("403 Forbidden", "", String::new())]);
        assert!(test(&server, "pw").unwrap_err().contains("files and sessions scopes"));
    }
}
