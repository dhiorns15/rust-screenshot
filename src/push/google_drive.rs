//! Google Drive: uploads into a folder of the user's My Drive, through RSnip's own Google app,
//! signed in to with a code shown in Settings (see `oauth`). It asks only for `drive.file`, so RSnip
//! sees and changes only the files and folders it made itself, nothing else in the Drive. What's
//! kept as the secret is the refresh token; the folder's id is kept with the place, once found or
//! made. Files over 5 MB go up in a resumable session, in pieces.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{encode, oauth, Kind, Saved};
use crate::http::{self, Body, Part, Response};

/// RSnip's Google app: the ID and secret of its OAuth client (of the "TVs and Limited Input
/// devices" type), from the Clients page of its project in the Google Cloud console. Google calls
/// the second a secret, but an installed app can't keep one and it isn't treated as one; it has to
/// be sent all the same. Empty: this build can't sign in to Google Drive.
pub const GOOGLE_CLIENT_ID: &str = "";
pub const GOOGLE_CLIENT_SECRET: &str = "";

const OAUTH: &str = "https://oauth2.googleapis.com";
const API: &str = "https://www.googleapis.com";
/// Only the files RSnip makes, and nothing else in the Drive.
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const FOLDER: &str = "application/vnd.google-apps.folder";

/// The most that goes up in one request (5 MB, as Google says for a multipart upload).
const SINGLE_MAX: u64 = 5 * 1024 * 1024;
/// How much goes in each request of a resumable upload. Google wants a multiple of 256 KiB.
const CHUNK: u64 = 8 * 1024 * 1024;

/// Where Google is, the app it's reached as, and how big an upload may be before it goes in
/// pieces (all different in tests).
struct Service<'a> {
    oauth: &'a str,
    api: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
    single_max: u64,
    chunk: u64,
}

const LIVE: Service = Service { oauth: OAUTH, api: API, client_id: GOOGLE_CLIENT_ID, client_secret: GOOGLE_CLIENT_SECRET, single_max: SINGLE_MAX, chunk: CHUNK };

/// Whether this build has RSnip's Google app, so it can sign in.
pub fn available() -> bool {
    !LIVE.client_id.is_empty()
}

/// Signs in with a code, which `shown` is given to show, then waits for the user to enter it
/// (until `cancelled`); the refresh token, and whose Drive it is if Google says.
pub fn sign_in(cancelled: &AtomicBool, shown: impl FnOnce(&oauth::DeviceCode)) -> Result<(String, Option<String>), String> {
    sign_in_at(&LIVE, cancelled, shown)
}

/// Checks the sign-in, and says whose Drive it is.
pub fn test(saved: &mut Saved) -> Result<String, String> {
    test_at(&LIVE, saved)
}

/// Uploads `file` into `folder` (a path in My Drive, made if it isn't there; empty for the top of
/// it), known by `folder_id` if a push found it before; the link to it, if `share_link`. A folder
/// found anew is learned (see `Saved::learn`), for the next push.
pub fn upload(saved: &mut Saved, folder: &str, folder_id: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    upload_at(&LIVE, saved, folder, folder_id, share_link, file)
}

fn provider<'a>(service: &Service<'a>) -> oauth::Provider<'a> {
    oauth::Provider {
        name: "Google Drive",
        token_url: format!("{}/token", service.oauth),
        device_url: format!("{}/device/code", service.oauth),
        client_id: service.client_id,
        client_secret: service.client_secret,
        scope: SCOPE,
        unknown_client: "Google doesn't know this build's app: its Google client ID or secret is wrong",
    }
}

/// An error if this build has no Google app to sign in with.
fn ready(service: &Service) -> Result<(), String> {
    if service.client_id.is_empty() {
        return Err("This build of RSnip can't reach Google Drive: it was made without RSnip's Google app".into());
    }
    Ok(())
}

fn sign_in_at(service: &Service, cancelled: &AtomicBool, shown: impl FnOnce(&oauth::DeviceCode)) -> Result<(String, Option<String>), String> {
    ready(service)?;
    let provider = provider(service);
    let code = oauth::start_device(&provider)?;
    shown(&code);
    let signed_in = oauth::finish_device(&provider, &code, cancelled)?;
    Ok((signed_in.refresh_token, whose(service, &signed_in.access_token).ok().flatten()))
}

fn test_at(service: &Service, saved: &mut Saved) -> Result<String, String> {
    ready(service)?;
    let token = oauth::refresh(&provider(service), saved)?.token;
    Ok(match whose(service, &token)? {
        Some(name) => format!("Connected as {name}"),
        None => "Connected".into(),
    })
}

/// Whose Drive it is: their name, else their email address, if Google says.
fn whose(service: &Service, token: &str) -> Result<Option<String>, String> {
    let about = call(service, "GET", "drive/v3/about?fields=user", token, None)?;
    let user = &about["user"];
    Ok(["displayName", "emailAddress"].iter().find_map(|key| user[key].as_str().filter(|s| !s.is_empty())).map(str::to_owned))
}

fn upload_at(service: &Service, saved: &mut Saved, folder: &str, folder_id: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    ready(service)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let size = std::fs::metadata(file).map_err(|e| format!("Couldn't read {name}: {e}"))?.len();
    let token = oauth::refresh(&provider(service), saved)?.token;
    let path = folders(folder);
    // What an earlier push of the same lot found is newer than what the place has kept.
    let known = match &saved.learned {
        Some(Kind::GoogleDrive { folder: was, folder_id: id, .. }) if was.trim() == folder.trim() => id.clone(),
        _ => folder_id.to_owned(),
    };
    let parent = if path.is_empty() { String::new() } else { folder_at(service, &token, &path, &known)? };
    if parent != known && !path.is_empty() {
        saved.learn(Kind::GoogleDrive { folder: folder.to_owned(), share_link, folder_id: parent.clone() });
    }
    let metadata = json!({ "name": name, "mimeType": http::content_type(file), "parents": [if parent.is_empty() { "root" } else { &parent }] });
    let made = if size <= service.single_max { multipart(service, &token, &metadata, file)? } else { resumable(service, &token, &metadata, file, size)? };
    if !share_link {
        return Ok(None);
    }
    let id = made["id"].as_str().ok_or("Google Drive's reply had no file in it")?;
    // It's there now: a failed link mustn't fail the push, or trying again would put up another
    // copy. A Workspace account may not be allowed links for anyone.
    Ok(share(service, &token, id).ok().map(|()| made["webViewLink"].as_str().map_or_else(|| format!("https://drive.google.com/file/d/{id}/view"), str::to_owned)))
}

/// The folders of `folder`, from the top of My Drive down.
fn folders(folder: &str) -> Vec<&str> {
    folder.split(['/', '\\']).map(str::trim).filter(|f| !f.is_empty()).collect()
}

/// The id of the folder at `path`, made if it isn't there. The one found before, `known`, is still
/// it if it's still there and not in the bin; otherwise (it was deleted, so the next push makes
/// it again) it's looked for, folder by folder. Looked up only once found, as a folder just made
/// may not show in a search for a while.
fn folder_at(service: &Service, token: &str, path: &[&str], known: &str) -> Result<String, String> {
    if !known.is_empty() {
        let reply = request(service, "GET", &format!("drive/v3/files/{}?fields=id,trashed", encode(known)), token, None)?;
        let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
        match reply.status {
            404 => {}
            _ if !reply.is_success() => return Err(explain(&reply)),
            _ if json["trashed"].as_bool() == Some(true) => {}
            _ => return Ok(known.to_owned()),
        }
    }
    let mut parent = "root".to_owned();
    for name in path {
        let query = format!("name = '{}' and mimeType = '{FOLDER}' and '{parent}' in parents and trashed = false", quoted(name));
        let found = call(service, "GET", &format!("drive/v3/files?q={}&spaces=drive&pageSize=1&fields=files(id)", encode(&query)), token, None)?;
        parent = match found["files"][0]["id"].as_str() {
            Some(id) => id.to_owned(),
            None => {
                let folder = json!({ "name": name, "mimeType": FOLDER, "parents": [parent] });
                let made = call(service, "POST", "drive/v3/files?fields=id", token, Some(&folder))?;
                made["id"].as_str().ok_or("Google Drive didn't make the folder")?.to_owned()
            }
        };
    }
    Ok(parent)
}

/// `name` as it goes between single quotes in a Drive search.
fn quoted(name: &str) -> String {
    name.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Uploads a small file and what's known of it in one request, as multipart/related.
fn multipart(service: &Service, token: &str, metadata: &Value, file: &Path) -> Result<Value, String> {
    let boundary = boundary();
    let head = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: {}\r\n\r\n", http::content_type(file));
    let body = Body::Parts(vec![Part::Bytes(head.into_bytes()), Part::File(file.to_path_buf()), Part::Bytes(format!("\r\n--{boundary}--\r\n").into_bytes())]);
    let bearer = format!("Bearer {token}");
    let content_type = format!("multipart/related; boundary={boundary}");
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", content_type.as_str())];
    let url = format!("{}/upload/drive/v3/files?uploadType=multipart&fields=id,webViewLink", service.api);
    answer(http::request("POST", &url, &headers, body)?)
}

/// A boundary for multipart/related that a file won't have in it.
fn boundary() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("RSnip-{nanos:x}-{:x}", std::process::id())
}

/// A resumable upload: the session is started with what's known of the file, which is then sent
/// in pieces, each from where Google says it got to.
fn resumable(service: &Service, token: &str, metadata: &Value, file: &Path, size: u64) -> Result<Value, String> {
    let couldnt = |e: std::io::Error| format!("Couldn't read {}: {e}", file.display());
    let bearer = format!("Bearer {token}");
    let length = size.to_string();
    let headers = [
        ("Authorization", bearer.as_str()),
        ("Content-Type", "application/json; charset=UTF-8"),
        ("X-Upload-Content-Type", http::content_type(file)),
        ("X-Upload-Content-Length", length.as_str()),
    ];
    let url = format!("{}/upload/drive/v3/files?uploadType=resumable&fields=id,webViewLink", service.api);
    let started = http::request("POST", &url, &headers, Body::Bytes(metadata.to_string().into_bytes()))?;
    if !started.is_success() {
        return Err(explain(&started));
    }
    let session = started.header("Location").ok_or("Google Drive didn't start the upload")?.to_owned();
    let mut reader = std::fs::File::open(file).map_err(couldnt)?;
    let mut offset = 0;
    loop {
        // All of it arrived, Google says, but the file isn't made: a session gone wrong.
        if offset >= size {
            return Err("Google Drive didn't finish the upload: try again".into());
        }
        let mut piece = Vec::new();
        reader.seek(SeekFrom::Start(offset)).map_err(couldnt)?;
        (&mut reader).take(service.chunk.min(size - offset)).read_to_end(&mut piece).map_err(couldnt)?;
        if piece.is_empty() {
            return Err(format!("{} was shorter than it was a moment ago", file.display()));
        }
        let range = format!("bytes {offset}-{}/{size}", offset + piece.len() as u64 - 1);
        let reply = http::request("PUT", &session, &[("Content-Range", range.as_str())], Body::Bytes(piece))?;
        match reply.status {
            // The session's address is all it needs: no token.
            200 | 201 => return answer(reply),
            308 => {
                // What Google has, "bytes=0-1234"; nothing yet if it doesn't say.
                let got = reply.header("Range").and_then(|r| r.rsplit('-').next()?.trim().parse::<u64>().ok()).map_or(0, |last| last + 1);
                if got <= offset {
                    return Err("Google Drive stopped taking the upload: try again".into());
                }
                offset = got;
            }
            _ => return Err(explain(&reply)),
        }
    }
}

/// Lets anyone with the link see the file.
fn share(service: &Service, token: &str, id: &str) -> Result<(), String> {
    let anyone = json!({ "type": "anyone", "role": "reader" });
    call(service, "POST", &format!("drive/v3/files/{}/permissions?fields=id", encode(id)), token, Some(&anyone)).map(|_| ())
}

/// Calls the Drive API at `route`, with JSON for `body`; what it gave.
fn call(service: &Service, method: &str, route: &str, token: &str, body: Option<&Value>) -> Result<Value, String> {
    answer(request(service, method, route, token, body)?)
}

fn request(service: &Service, method: &str, route: &str, token: &str, body: Option<&Value>) -> Result<Response, String> {
    let bearer = format!("Bearer {token}");
    let mut headers = vec![("Authorization", bearer.as_str())];
    let body = match body {
        Some(json) => {
            headers.push(("Content-Type", "application/json"));
            Body::Bytes(json.to_string().into_bytes())
        }
        None => Body::Empty,
    };
    http::request(method, &format!("{}/{route}", service.api), &headers, body)
}

/// What a reply says if it succeeded (nothing at all is `null`), else why not, in words.
fn answer(reply: Response) -> Result<Value, String> {
    if reply.is_success() {
        return Ok(serde_json::from_slice(&reply.body).unwrap_or_default());
    }
    Err(explain(&reply))
}

/// A reply that wasn't a success, as advice.
fn explain(reply: &Response) -> String {
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    let error = &json["error"];
    // The reasons, as the older errors list them and the newer details do.
    let reasons: Vec<String> = ["errors", "details"]
        .iter()
        .filter_map(|key| error[key].as_array())
        .flatten()
        .filter_map(|e| e["reason"].as_str())
        .map(|r| r.to_ascii_lowercase().replace('_', ""))
        .collect();
    let has = |reason: &str| reasons.iter().any(|r| r == reason);
    if reply.status == 401 {
        return "Google Drive signed RSnip out: sign in again in Settings → Push to".into();
    }
    if has("storagequotaexceeded") || has("quotaexceeded") {
        return "Your Google Drive is full: make some room, or get more space".into();
    }
    if reply.status == 429 || ["ratelimitexceeded", "userratelimitexceeded", "dailylimitexceeded", "sharingratelimitexceeded"].iter().any(|r| has(r)) {
        return match reply.header("Retry-After") {
            Some(seconds) => format!("Google Drive says RSnip is sending too many requests: try again in {seconds} seconds"),
            None => "Google Drive says RSnip is sending too many requests: try again later".into(),
        };
    }
    if has("insufficientpermissions") || has("insufficientfilepermissions") || has("appnotauthorizedtofile") {
        return "Google Drive won't let RSnip change that: sign in again in Settings → Push to".into();
    }
    if reply.status == 404 {
        return "Google Drive couldn't find the folder: push again, and it's made again".into();
    }
    match error["message"].as_str().filter(|m| !m.is_empty()) {
        Some(message) => format!("Google Drive said {}", message.trim_end_matches('.')),
        None => reply.clone().check("Google Drive").err().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::{serve_each, Received};

    const TOKEN_REPLY: &str = r#"{"access_token":"ya29.ACCESS","expires_in":3599,"scope":"https://www.googleapis.com/auth/drive.file","token_type":"Bearer"}"#;

    fn file(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-google-drive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn at(server: &str) -> Service<'_> {
        Service { oauth: server, api: server, client_id: "client-1", client_secret: "secret-1", single_max: SINGLE_MAX, chunk: CHUNK }
    }

    fn token(refresh: &str) -> Saved {
        Saved::unsaved(Some(refresh.to_owned()))
    }

    /// Pushes `file` with a sign-in of its own; what came of it, and the folder's id if it was
    /// found anew.
    fn push(service: &Service, folder: &str, known: &str, share_link: bool, file: &Path) -> (Result<Option<String>, String>, Option<String>) {
        let mut saved = token("R");
        let pushed = upload_at(service, &mut saved, folder, known, share_link, file);
        (pushed, learned(&saved))
    }

    fn learned(saved: &Saved) -> Option<String> {
        match &saved.learned {
            Some(Kind::GoogleDrive { folder_id, .. }) => Some(folder_id.clone()),
            _ => None,
        }
    }

    fn json_of(request: &Received) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    /// The search in a request for a folder, decoded.
    fn query(request: &Received) -> String {
        let q = request.path.split_once("q=").unwrap().1.split('&').next().unwrap();
        let bytes = q.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&q[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn makes_the_folder_uploads_in_one_request_and_links_for_anyone() {
        let replies = vec![
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", r#"{"files":[]}"#.to_owned()),
            ("200 OK", "", r#"{"id":"F1"}"#.to_owned()),
            ("200 OK", "", r#"{"id":"I1","webViewLink":"https://drive.google.com/file/d/I1/view?usp=drivesdk"}"#.to_owned()),
            ("200 OK", "", r#"{"id":"anyoneWithLink"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let png = file("a b's.png", b"png bytes");
        let pushed = push(&at(&server), " Screenshots ", "", true, &png);
        assert_eq!(pushed, (Ok(Some("https://drive.google.com/file/d/I1/view?usp=drivesdk".into())), Some("F1".into())));

        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/token");
        assert_eq!(requests[0].body, b"grant_type=refresh_token&refresh_token=R&client_id=client-1&client_secret=secret-1");

        let find = &requests[1];
        assert_eq!(find.method, "GET");
        assert!(find.path.starts_with("/drive/v3/files?q="), "{}", find.path);
        assert_eq!(query(find), "name = 'Screenshots' and mimeType = 'application/vnd.google-apps.folder' and 'root' in parents and trashed = false");
        assert_eq!(find.header("Authorization"), Some("Bearer ya29.ACCESS"));

        let make = &requests[2];
        assert_eq!((make.method.as_str(), make.path.as_str()), ("POST", "/drive/v3/files?fields=id"));
        assert_eq!(json_of(make), json!({ "name": "Screenshots", "mimeType": FOLDER, "parents": ["root"] }));

        let put = &requests[3];
        assert_eq!((put.method.as_str(), put.path.as_str()), ("POST", "/upload/drive/v3/files?uploadType=multipart&fields=id,webViewLink"));
        let content_type = put.header("Content-Type").unwrap();
        let boundary = content_type.strip_prefix("multipart/related; boundary=").unwrap_or_else(|| panic!("{content_type}"));
        let metadata = json!({ "name": "a b's.png", "mimeType": "image/png", "parents": ["F1"] });
        let expected = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: image/png\r\n\r\npng bytes\r\n--{boundary}--\r\n");
        assert_eq!(String::from_utf8_lossy(&put.body), expected);

        let share = &requests[4];
        assert_eq!((share.method.as_str(), share.path.as_str()), ("POST", "/drive/v3/files/I1/permissions?fields=id"));
        assert_eq!(json_of(share), json!({ "type": "anyone", "role": "reader" }));
    }

    #[test]
    fn a_folder_found_before_is_checked_and_used_or_made_again_when_its_gone() {
        let saved = || ("200 OK", "", r#"{"id":"I1"}"#.to_owned());
        // Still there.
        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", r#"{"id":"F1","trashed":false}"#.into()), saved()]);
        assert_eq!(push(&at(&server), "Screenshots", "F1", false, &file("kept.png", b"x")), (Ok(None), None), "nothing new to keep");
        let requests = requests.join().unwrap();
        assert_eq!(requests[1].path, "/drive/v3/files/F1?fields=id,trashed");
        assert_eq!(requests.len(), 3, "not looked for, nor any link asked for");

        // Deleted, or in the bin: made again, and that one's id is the one to keep.
        for gone in [("404 Not Found", r#"{"error":{"code":404,"message":"File not found: F1."}}"#), ("200 OK", r#"{"id":"F1","trashed":true}"#)] {
            let replies = vec![("200 OK", "", TOKEN_REPLY.to_owned()), (gone.0, "", gone.1.to_owned()), ("200 OK", "", r#"{"files":[]}"#.to_owned()), ("200 OK", "", r#"{"id":"F2"}"#.to_owned()), saved()];
            let (server, requests) = serve_each(replies);
            assert_eq!(push(&at(&server), "Screenshots", "F1", false, &file("again.png", b"x")), (Ok(None), Some("F2".into())), "{gone:?}");
            let requests = requests.join().unwrap();
            assert_eq!(requests[3].method, "POST");
            let metadata = String::from_utf8_lossy(&requests[4].body).into_owned();
            assert!(metadata.contains(r#""parents":["F2"]"#), "{metadata}");
        }
    }

    #[test]
    fn the_next_push_of_a_lot_goes_to_the_folder_the_first_found_and_its_kept_once() {
        let replies = vec![
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", r#"{"files":[]}"#.to_owned()),
            ("200 OK", "", r#"{"id":"F1"}"#.to_owned()),
            ("200 OK", "", r#"{"id":"I1"}"#.to_owned()),
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", r#"{"id":"F1","trashed":false}"#.to_owned()),
            ("200 OK", "", r#"{"id":"I2"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let kept = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = kept.clone();
        let mut saved = token("R").remembering(move |kind| into.lock().unwrap().push(kind.clone()));
        for name in ["one.png", "two.png"] {
            assert_eq!(upload_at(&at(&server), &mut saved, "S", "", false, &file(name, b"x")), Ok(None));
        }
        assert_eq!(*kept.lock().unwrap(), [Kind::GoogleDrive { folder: "S".into(), share_link: false, folder_id: "F1".into() }]);
        let requests = requests.join().unwrap();
        assert_eq!(requests[5].path, "/drive/v3/files/F1?fields=id,trashed", "not searched for again, as a folder just made may not show yet");
    }

    #[test]
    fn folders_in_folders_are_found_or_made_one_by_one_and_the_top_needs_none() {
        let replies = vec![
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", r#"{"files":[{"id":"F1"}]}"#.to_owned()),
            ("200 OK", "", r#"{"files":[]}"#.to_owned()),
            ("200 OK", "", r#"{"id":"F2"}"#.to_owned()),
            ("200 OK", "", r#"{"id":"I1"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        assert_eq!(push(&at(&server), "/Shots/Dan's 2026\\", "", false, &file("deep.png", b"x")), (Ok(None), Some("F2".into())));
        let requests = requests.join().unwrap();
        assert!(query(&requests[1]).starts_with("name = 'Shots' and") && query(&requests[1]).ends_with("'root' in parents and trashed = false"));
        assert!(query(&requests[2]).starts_with(r"name = 'Dan\'s 2026' and") && query(&requests[2]).contains("'F1' in parents"), "{}", query(&requests[2]));
        assert_eq!(json_of(&requests[3])["parents"], json!(["F1"]));

        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", r#"{"id":"I1"}"#.into())]);
        assert_eq!(push(&at(&server), " / ", "", false, &file("top.png", b"x")), (Ok(None), None));
        let body = String::from_utf8_lossy(&requests.join().unwrap()[1].body).into_owned();
        assert!(body.contains(r#""parents":["root"]"#), "{body}");
    }

    #[test]
    fn a_big_file_goes_up_in_pieces_from_where_google_got_to() {
        let contents: Vec<u8> = (0..25u8).collect();
        let big = file("big.mp4", &contents);
        // The session's own address, elsewhere.
        let session = vec![
            ("308 Resume Incomplete", "Range: bytes=0-9\r\n", String::new()),
            // Only part of the second piece arrived: the rest goes again.
            ("308 Resume Incomplete", "Range: bytes=0-14\r\n", String::new()),
            ("200 OK", "", r#"{"id":"I9","webViewLink":"https://drive.google.com/file/d/I9/view"}"#.to_owned()),
        ];
        let (uploads, chunks) = serve_each(session);
        let location: &'static str = Box::leak(format!("Location: {uploads}/upload/drive/v3/files?uploadType=resumable&upload_id=U1\r\n").into_boxed_str());
        let replies = vec![
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", r#"{"files":[{"id":"F1"}]}"#.to_owned()),
            ("200 OK", location, String::new()),
            ("200 OK", "", "{}".to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let small = Service { single_max: 10, chunk: 10, ..at(&server) };
        assert_eq!(push(&small, "Rec", "", true, &big), (Ok(Some("https://drive.google.com/file/d/I9/view".into())), Some("F1".into())));

        let requests = requests.join().unwrap();
        let start = &requests[2];
        assert_eq!((start.method.as_str(), start.path.as_str()), ("POST", "/upload/drive/v3/files?uploadType=resumable&fields=id,webViewLink"));
        assert_eq!(json_of(start), json!({ "name": "big.mp4", "mimeType": "video/mp4", "parents": ["F1"] }));
        assert_eq!((start.header("X-Upload-Content-Type"), start.header("X-Upload-Content-Length")), (Some("video/mp4"), Some("25")));
        assert_eq!(start.header("Content-Type"), Some("application/json; charset=UTF-8"));

        let chunks = chunks.join().unwrap();
        let sent: Vec<(&str, &[u8])> = chunks.iter().map(|c| (c.header("Content-Range").unwrap(), c.body.as_slice())).collect();
        assert_eq!(sent, [("bytes 0-9/25", &contents[..10]), ("bytes 10-19/25", &contents[10..20]), ("bytes 15-24/25", &contents[15..])]);
        assert_eq!(chunks.iter().map(|c| c.method.as_str()).collect::<Vec<_>>(), ["PUT"; 3]);
        assert!(chunks.iter().all(|c| c.path == "/upload/drive/v3/files?uploadType=resumable&upload_id=U1" && c.header("Authorization").is_none()));
        assert_eq!(requests[3].path, "/drive/v3/files/I9/permissions?fields=id");
    }

    #[test]
    fn the_real_limits_are_googles() {
        assert_eq!(SINGLE_MAX, 5 * 1024 * 1024);
        assert_eq!(CHUNK % (256 * 1024), 0, "Google wants whole 256 KiB pieces but the last");
    }

    #[test]
    fn a_link_that_cant_be_made_leaves_the_file_up_without_one() {
        let refused = r#"{"error":{"code":403,"message":"The user does not have sufficient permissions for this file.","errors":[{"reason":"publishOutNotPermitted"}]}}"#;
        let replies = vec![("200 OK", "", TOKEN_REPLY.to_owned()), ("200 OK", "", r#"{"id":"I1","webViewLink":"https://drive.google.com/x"}"#.to_owned()), ("403 Forbidden", "", refused.to_owned())];
        let (server, _) = serve_each(replies);
        assert_eq!(push(&at(&server), "", "", true, &file("nolink.png", b"x")), (Ok(None), None));
        // Without webViewLink in the reply, it's made from the id.
        let replies = vec![("200 OK", "", TOKEN_REPLY.to_owned()), ("200 OK", "", r#"{"id":"I2"}"#.to_owned()), ("200 OK", "", "{}".to_owned())];
        let (server, _) = serve_each(replies);
        assert_eq!(push(&at(&server), "", "", true, &file("id.png", b"x")), (Ok(Some("https://drive.google.com/file/d/I2/view".into())), None));
    }

    #[test]
    fn says_whose_drive_it_is() {
        let about = r#"{"user":{"kind":"drive#user","displayName":"Dan H","emailAddress":"d@example.com"}}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", about.into())]);
        assert_eq!(test_at(&at(&server), &mut token("R")), Ok("Connected as Dan H".into()));
        let call = &requests.join().unwrap()[1];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/drive/v3/about?fields=user"));
        assert_eq!(call.header("Authorization"), Some("Bearer ya29.ACCESS"));
        let (server, _) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", r#"{"user":{"emailAddress":"d@example.com"}}"#.into())]);
        assert_eq!(test_at(&at(&server), &mut token("R")), Ok("Connected as d@example.com".into()));

        let revoked = r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        let (server, _) = serve_each(vec![("400 Bad Request", "", revoked.into())]);
        assert_eq!(test_at(&at(&server), &mut token("R")), Err("Google Drive signed RSnip out: sign in again in Settings → Push to".into()));
        let (server, _) = serve_each(vec![("401 Unauthorized", "", r#"{"error":"invalid_client","error_description":"Unauthorized"}"#.into())]);
        assert_eq!(test_at(&at(&server), &mut token("R")), Err("Google doesn't know this build's app: its Google client ID or secret is wrong".into()));
    }

    #[test]
    fn signs_in_with_a_code_and_says_as_whom() {
        let started = r#"{"device_code":"D1","user_code":"GQVQ-JKEC","verification_url":"https://www.google.com/device","expires_in":1800,"interval":0}"#;
        let signed_in = r#"{"access_token":"ya29.A","expires_in":3599,"refresh_token":"1//R","scope":"https://www.googleapis.com/auth/drive.file","token_type":"Bearer"}"#;
        let replies = vec![("200 OK", "", started.to_owned()), ("200 OK", "", signed_in.to_owned()), ("200 OK", "", r#"{"user":{"displayName":"Dan H"}}"#.to_owned())];
        let (server, requests) = serve_each(replies);
        let mut shown = None;
        let result = sign_in_at(&at(&server), &AtomicBool::new(false), |code| shown = Some((code.user_code.clone(), code.verification_url.clone())));
        assert_eq!(result, Ok(("1//R".into(), Some("Dan H".into()))));
        assert_eq!(shown, Some(("GQVQ-JKEC".into(), "https://www.google.com/device".into())));
        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/device/code");
        assert_eq!(requests[0].body, b"client_id=client-1&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file");
        assert_eq!(requests[1].path, "/token");
        assert_eq!(requests[2].header("Authorization"), Some("Bearer ya29.A"));
    }

    #[test]
    fn a_build_without_rsnips_google_app_says_so() {
        let none = Service { client_id: "", ..at("http://127.0.0.1:9") };
        let message = "This build of RSnip can't reach Google Drive: it was made without RSnip's Google app";
        assert_eq!(test_at(&none, &mut token("R")), Err(message.into()));
        assert_eq!(upload_at(&none, &mut token("R"), "", "", true, &file("none.png", b"x")), Err(message.into()));
        assert_eq!(sign_in_at(&none, &AtomicBool::new(false), |_| panic!("no code")), Err(message.into()));
        assert_eq!(available(), !GOOGLE_CLIENT_ID.is_empty());
    }

    #[test]
    fn explains_what_google_refuses() {
        let reply = |status, body: &str| Response { status, headers: Vec::new(), body: body.as_bytes().to_vec() };
        let error = |status: u16, reason: &str, message: &str| reply(status, &json!({ "error": { "code": status, "message": message, "errors": [{ "domain": "global", "reason": reason, "message": message }] } }).to_string());
        assert_eq!(explain(&reply(401, r#"{"error":{"code":401,"message":"Request had invalid authentication credentials."}}"#)), "Google Drive signed RSnip out: sign in again in Settings → Push to");
        assert_eq!(explain(&error(403, "storageQuotaExceeded", "The user's Drive storage quota has been exceeded.")), "Your Google Drive is full: make some room, or get more space");
        assert_eq!(explain(&error(403, "userRateLimitExceeded", "User rate limit exceeded.")), "Google Drive says RSnip is sending too many requests: try again later");
        let details = r#"{"error":{"code":403,"message":"Quota exceeded","status":"PERMISSION_DENIED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED"}]}}"#;
        assert_eq!(explain(&reply(403, details)), "Google Drive says RSnip is sending too many requests: try again later", "as the newer errors say it");
        let limited = Response { headers: vec![("Retry-After".into(), "30".into())], ..reply(429, "") };
        assert_eq!(explain(&limited), "Google Drive says RSnip is sending too many requests: try again in 30 seconds");
        assert_eq!(explain(&error(403, "appNotAuthorizedToFile", "The user has not granted the app write access")), "Google Drive won't let RSnip change that: sign in again in Settings → Push to");
        assert_eq!(explain(&error(404, "notFound", "File not found: F1.")), "Google Drive couldn't find the folder: push again, and it's made again");
        assert_eq!(explain(&error(400, "badRequest", "Invalid value for parents.")), "Google Drive said Invalid value for parents");
        assert_eq!(explain(&reply(500, "Internal Server Error")), "Google Drive replied 500: Internal Server Error");
    }
}
