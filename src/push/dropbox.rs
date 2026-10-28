//! Dropbox: uploads the file into a folder of the user's Dropbox, as an app of their own (so RSnip
//! needs no key of its own to be registered with Dropbox). The app signs in with PKCE (see `oauth`)
//! and what's kept as the secret is the refresh token, traded for an access token at each push.
//! Files over 150 MiB go up in pieces, in an upload session, since recordings can be big.

use std::io::Read;
use std::path::Path;

use serde_json::{json, Value};

use super::{oauth, Saved};
use crate::http::{self, Body, Response};

pub(super) const API: &str = "https://api.dropboxapi.com";
const CONTENT: &str = "https://content.dropboxapi.com";

/// The most one upload request may hold (150 MiB, Dropbox's limit).
const SINGLE_MAX: u64 = 150 * 1024 * 1024;
/// How much goes in each request of an upload session. Dropbox wants a multiple of 4 MiB.
const CHUNK: u64 = 8 * 1024 * 1024;

/// Where Dropbox is, and how big an upload may be before it goes in pieces (smaller in tests).
struct Service<'a> {
    api: &'a str,
    content: &'a str,
    single_max: u64,
    chunk: u64,
}

const LIVE: Service = Service { api: API, content: CONTENT, single_max: SINGLE_MAX, chunk: CHUNK };

/// Signing in to the user's own app at `api`, whose key is `app_key`.
pub(super) fn provider<'a>(api: &str, app_key: &'a str) -> oauth::Provider<'a> {
    oauth::Provider {
        name: "Dropbox",
        token_url: format!("{api}/oauth2/token"),
        device_url: String::new(),
        client_id: app_key.trim(),
        client_secret: "",
        scope: "",
        unknown_client: "Dropbox doesn't know this app key: check it against the app's Settings page on dropbox.com/developers",
    }
}

/// Checks the sign-in, and says whose Dropbox it is; and, if pushes make links, that the app may.
pub fn test(app_key: &str, saved: &mut Saved, share_link: bool) -> Result<String, String> {
    test_at(&LIVE, app_key, saved, share_link)
}

/// Uploads `file` into `folder`, named as it is (or with a number added if one by that name is
/// there already); the link to it, if `share_link`.
pub fn upload(app_key: &str, saved: &mut Saved, folder: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    upload_at(&LIVE, app_key, saved, folder, share_link, file)
}

fn test_at(service: &Service, app_key: &str, saved: &mut Saved, share_link: bool) -> Result<String, String> {
    let oauth::Access { token, scope: scopes } = oauth::refresh(&provider(service.api, app_key), saved)?;
    // A push would put the file there and then find it can't make the link, so it's said now.
    if share_link && scopes.is_some_and(|s| !s.split_whitespace().any(|s| s == "sharing.write")) {
        return Err("The Dropbox app can't make links: turn on sharing.write under Permissions in the app's page at dropbox.com/developers (and choose Submit), then sign in again. Or turn off Share link".into());
    }
    let account = rpc(service, "users/get_current_account", &token, None)?;
    let name = ["display_name", "given_name"].iter().find_map(|key| account["name"][key].as_str().filter(|n| !n.is_empty()));
    Ok(match name.or(account["email"].as_str()) {
        Some(name) => format!("Connected as {name}"),
        None => "Connected".into(),
    })
}

fn upload_at(service: &Service, app_key: &str, saved: &mut Saved, folder: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let size = std::fs::metadata(file).map_err(|e| format!("Couldn't read {name}: {e}"))?.len();
    let token = oauth::refresh(&provider(service.api, app_key), saved)?.token;
    let path = path_of(folder, &name);
    let saved = if size <= service.single_max {
        let arg = json!({ "path": path, "mode": "add", "autorename": true, "mute": true });
        content(service, "files/upload", &token, &arg, Body::File(file.to_path_buf()))?
    } else {
        upload_in_pieces(service, &token, &path, file, size)?
    };
    if !share_link {
        return Ok(None);
    }
    // Where Dropbox put it, which differs from `path` if it had to rename the file.
    let saved_at = saved["path_display"].as_str().or(saved["path_lower"].as_str()).unwrap_or(&path);
    // It's there now: a failed link mustn't fail the push, or trying again would put up another copy.
    // Test connection says if the app can't make links at all.
    Ok(link(service, &token, saved_at).ok())
}

/// An upload session: it's opened, sent in chunks, and then closed into a file at `path`.
fn upload_in_pieces(service: &Service, token: &str, path: &str, file: &Path, size: u64) -> Result<Value, String> {
    let couldnt = |e: std::io::Error| format!("Couldn't read {}: {e}", file.display());
    let started = content(service, "files/upload_session/start", token, &json!({ "close": false }), nothing())?;
    let session = started["session_id"].as_str().ok_or("Dropbox didn't start the upload")?;
    let mut reader = std::fs::File::open(file).map_err(couldnt)?;
    let mut offset = 0;
    while offset < size {
        let mut piece = Vec::new();
        (&mut reader).take(service.chunk.min(size - offset)).read_to_end(&mut piece).map_err(couldnt)?;
        if piece.is_empty() {
            return Err(format!("{} was shorter than it was a moment ago", file.display()));
        }
        let arg = json!({ "cursor": { "session_id": session, "offset": offset }, "close": false });
        let length = piece.len() as u64;
        content(service, "files/upload_session/append_v2", token, &arg, Body::Bytes(piece))?;
        offset += length;
    }
    let commit = json!({ "path": path, "mode": "add", "autorename": true, "mute": true });
    let arg = json!({ "cursor": { "session_id": session, "offset": offset }, "commit": commit });
    content(service, "files/upload_session/finish", token, &arg, nothing())
}

/// A link that opens the file at `path` in Dropbox.
fn link(service: &Service, token: &str, path: &str) -> Result<String, String> {
    let url = format!("{}/2/sharing/create_shared_link_with_settings", service.api);
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", "application/json")];
    let reply = http::request("POST", &url, &headers, Body::Bytes(json!({ "path": path }).to_string().into_bytes()))?;
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    if reply.is_success() {
        return json["url"].as_str().map(str::to_owned).ok_or_else(|| "Dropbox's reply had no link".to_owned());
    }
    // A file only has the one link; if it has one already (it's pushed again with the same name,
    // and autorename didn't change it), that's the one.
    if reply.status == 409 && tags(&json["error"]).iter().any(|t| t == "shared_link_already_exists") {
        if let Some(url) = first_url(&json["error"]) {
            return Ok(url);
        }
    }
    Err(explain(&reply))
}

/// The first web address named `url` in `value`: where the metadata of a link that exists is.
fn first_url(value: &Value) -> Option<String> {
    match value {
        Value::Object(o) => o.get("url").and_then(Value::as_str).map(str::to_owned).or_else(|| o.values().find_map(first_url)),
        Value::Array(a) => a.iter().find_map(first_url),
        _ => None,
    }
}

/// A body with nothing in it, sent with a length of 0 (a POST without one goes in chunks, which a
/// server may take for a request that isn't finished).
fn nothing() -> Body {
    Body::Bytes(Vec::new())
}

/// Calls an endpoint that takes and gives JSON (no body at all for `None`); what it gave.
fn rpc(service: &Service, route: &str, token: &str, arg: Option<&Value>) -> Result<Value, String> {
    let bearer = format!("Bearer {token}");
    let mut headers = vec![("Authorization", bearer.as_str())];
    let body = match arg {
        Some(arg) => {
            headers.push(("Content-Type", "application/json"));
            Body::Bytes(arg.to_string().into_bytes())
        }
        None => nothing(),
    };
    answer(http::request("POST", &format!("{}/2/{route}", service.api), &headers, body)?)
}

/// Calls an endpoint that takes a file's bytes, with its arguments in a header; what it gave.
fn content(service: &Service, route: &str, token: &str, arg: &Value, body: Body) -> Result<Value, String> {
    let bearer = format!("Bearer {token}");
    let arg = ascii_json(arg);
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", "application/octet-stream"), ("Dropbox-API-Arg", arg.as_str())];
    answer(http::request("POST", &format!("{}/2/{route}", service.content), &headers, body)?)
}

/// What a reply says if it succeeded (nothing at all is `null`), else why not, in words.
fn answer(reply: Response) -> Result<Value, String> {
    if reply.is_success() {
        return Ok(serde_json::from_slice(&reply.body).unwrap_or_default());
    }
    Err(explain(&reply))
}

/// `value` as JSON that's all ASCII, as a header needs (Dropbox: non-ASCII characters and 0x7F as
/// \uXXXX). Only strings hold such characters, so it's safe to change them wherever they are.
fn ascii_json(value: &Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() && c != '\u{7f}' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// Where a file called `name` goes in `folder`: "/Screenshots/a.png", or "/a.png" in the top one.
fn path_of(folder: &str, name: &str) -> String {
    let folder = folder.trim().replace('\\', "/");
    let folder = folder.trim_matches('/');
    if folder.is_empty() { format!("/{name}") } else { format!("/{folder}/{name}") }
}

/// Every ".tag" in `value`, which is how Dropbox names what kind of error (or part of one) it is.
fn tags(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    collect_tags(value, &mut found);
    found
}

fn collect_tags(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(o) => {
            for (key, inner) in o {
                match (key.as_str(), inner.as_str()) {
                    (".tag", Some(tag)) => found.push(tag.to_owned()),
                    _ => collect_tags(inner, found),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|inner| collect_tags(inner, found)),
        _ => {}
    }
}

/// A reply that wasn't a success, as advice.
fn explain(reply: &Response) -> String {
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    let tags = tags(&json["error"]);
    let has = |tag: &str| tags.iter().any(|t| t == tag);
    if reply.status == 429 || has("too_many_requests") || has("too_many_write_operations") {
        let wait = reply.header("Retry-After").map(str::to_owned).or_else(|| json["error"]["retry_after"].as_u64().map(|s| s.to_string()));
        return match wait {
            Some(seconds) => format!("Dropbox says RSnip is sending too many requests: try again in {seconds} seconds"),
            None => "Dropbox says RSnip is sending too many requests: try again later".into(),
        };
    }
    if has("missing_scope") {
        let scope = json["error"]["required_scope"].as_str().unwrap_or("files.content.write and sharing.write");
        return format!(
            "The Dropbox app needs the {scope} permission: turn it on under Permissions in the app's page at dropbox.com/developers (and choose Submit), then sign in again in Settings → Push to"
        );
    }
    if reply.status == 401 {
        return "Dropbox signed RSnip out: sign in again in Settings → Push to".into();
    }
    if tags.iter().any(|t| t.ends_with("insufficient_space")) {
        return "Your Dropbox is full: make some room, or get more space".into();
    }
    if has("upload_traffic_limit_reached") {
        return "Dropbox has stopped uploads from this account for now: try again later".into();
    }
    if has("malformed_path") || has("disallowed_name") {
        return "Dropbox doesn't accept that folder or file name: check the folder in Settings → Push to".into();
    }
    if has("no_write_permission") || has("access_restricted") {
        return "The Dropbox app may not write to that folder: check the folder, and the app's access type".into();
    }
    if has("email_not_verified") {
        return "Dropbox won't make links until the email address of this account is verified".into();
    }
    if has("access_denied") {
        return "Dropbox won't let RSnip share that: the account's settings may not allow links".into();
    }
    if has("payload_too_large") || has("too_large") {
        return "Too big for Dropbox".into();
    }
    match json["error_summary"].as_str().filter(|s| !s.is_empty()) {
        Some(summary) => format!("Dropbox said {}", summary.trim_end_matches('.').trim_end_matches('/')),
        None => reply.clone().check("Dropbox").err().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    const TOKEN_REPLY: &str = r#"{"access_token":"sl.ACCESS","token_type":"bearer","expires_in":14400}"#;

    fn file(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-dropbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A sign-in with the refresh token `refresh`.
    fn token(refresh: &str) -> Saved {
        Saved::unsaved(Some(refresh.to_owned()))
    }

    fn at(server: &str) -> Service<'_> {
        Service { api: server, content: server, single_max: SINGLE_MAX, chunk: CHUNK }
    }

    fn json_of(request: &crate::http::tests::Received) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    #[test]
    fn uploads_with_a_fresh_access_token_and_links_to_where_dropbox_put_it() {
        let saved = r#"{".tag":"file","name":"Ünï b (1).png","path_display":"/Screenshots/Ünï b (1).png","id":"id:1","rev":"1","size":9}"#;
        let linked = r#"{".tag":"file","url":"https://www.dropbox.com/scl/fi/x/b.png?rlkey=k&dl=0","name":"b (1).png"}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", saved.into()), ("200 OK", "", linked.into())]);
        let png = file("Ünï b.png", b"png bytes");
        let link = upload_at(&at(&server), "key1", &mut token("R-1"), " /Screenshots/ ", true, &png);
        assert_eq!(link, Ok(Some("https://www.dropbox.com/scl/fi/x/b.png?rlkey=k&dl=0".into())));

        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/oauth2/token");
        assert_eq!(requests[0].body, b"grant_type=refresh_token&refresh_token=R-1&client_id=key1");

        let put = &requests[1];
        assert_eq!((put.method.as_str(), put.path.as_str()), ("POST", "/2/files/upload"));
        assert_eq!(put.header("Authorization"), Some("Bearer sl.ACCESS"));
        assert_eq!(put.header("Content-Type"), Some("application/octet-stream"));
        let arg = put.header("Dropbox-API-Arg").unwrap();
        assert!(arg.is_ascii(), "{arg}");
        assert!(arg.contains(r#""path":"/Screenshots/\u00dcn\u00ef b.png""#), "{arg}");
        let parsed: Value = serde_json::from_str(arg).unwrap();
        assert_eq!(parsed, json!({ "path": "/Screenshots/Ünï b.png", "mode": "add", "autorename": true, "mute": true }));
        assert_eq!(put.body, b"png bytes");

        // The link is to the file as Dropbox saved it (renamed, here), not as RSnip named it.
        let share = &requests[2];
        assert_eq!(share.path, "/2/sharing/create_shared_link_with_settings");
        assert_eq!(share.header("Authorization"), Some("Bearer sl.ACCESS"));
        assert_eq!(share.header("Content-Type"), Some("application/json"));
        assert_eq!(json_of(share), json!({ "path": "/Screenshots/Ünï b (1).png" }));
    }

    #[test]
    fn without_a_link_it_only_uploads_and_the_top_folder_is_a_slash() {
        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", r#"{"name":"a.png"}"#.into())]);
        assert_eq!(upload_at(&at(&server), "key1", &mut token("R"), "", false, &file("top.png", b"x")), Ok(None));
        let requests = requests.join().unwrap();
        assert_eq!(requests.len(), 2, "no link asked for");
        let parsed: Value = serde_json::from_str(requests[1].header("Dropbox-API-Arg").unwrap()).unwrap();
        assert_eq!(parsed["path"], "/top.png");
    }

    #[test]
    fn a_link_that_exists_is_the_link() {
        let saved = r#"{"path_display":"/S/a.png"}"#;
        let exists = r#"{"error_summary":"shared_link_already_exists/..","error":{".tag":"shared_link_already_exists","shared_link_already_exists":{".tag":"metadata","metadata":{".tag":"file","url":"https://www.dropbox.com/s/old/a.png?dl=0","name":"a.png"}}}}"#;
        let (server, _) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", saved.into()), ("409 Conflict", "", exists.into())]);
        assert_eq!(upload_at(&at(&server), "key1", &mut token("R"), "S", true, &file("exists.png", b"x")), Ok(Some("https://www.dropbox.com/s/old/a.png?dl=0".into())));

        // Without its address there's no link to hand back, but the file is up, so the push did its job.
        let bare = r#"{"error_summary":"shared_link_already_exists/..","error":{".tag":"shared_link_already_exists"}}"#;
        let (server, _) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", saved.into()), ("409 Conflict", "", bare.into())]);
        assert_eq!(upload_at(&at(&server), "key1", &mut token("R"), "S", true, &file("bare.png", b"x")), Ok(None));
    }

    #[test]
    fn a_big_file_goes_up_in_pieces_in_order() {
        let contents: Vec<u8> = (0..25u8).collect();
        let big = file("big.mp4", &contents);
        let start = (r#"{"session_id":"S1"}"#).to_owned();
        let saved = r#"{"path_display":"/Rec/big.mp4"}"#.to_owned();
        let replies = vec![
            ("200 OK", "", TOKEN_REPLY.to_owned()),
            ("200 OK", "", start),
            ("200 OK", "", String::new()),
            ("200 OK", "", String::new()),
            ("200 OK", "", String::new()),
            ("200 OK", "", saved),
            ("200 OK", "", r#"{"url":"https://www.dropbox.com/s/b?dl=0"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let small = Service { api: &server, content: &server, single_max: 10, chunk: 10 };
        assert_eq!(upload_at(&small, "key1", &mut token("R"), "Rec", true, &big), Ok(Some("https://www.dropbox.com/s/b?dl=0".into())));

        let requests = requests.join().unwrap();
        let args: Vec<Value> = requests[1..6].iter().map(|r| serde_json::from_str(r.header("Dropbox-API-Arg").unwrap()).unwrap()).collect();
        let paths: Vec<&str> = requests[1..6].iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/2/files/upload_session/start", "/2/files/upload_session/append_v2", "/2/files/upload_session/append_v2", "/2/files/upload_session/append_v2", "/2/files/upload_session/finish"]
        );
        assert_eq!(args[0], json!({ "close": false }));
        assert!(requests[1].body.is_empty());
        for (i, (offset, bytes)) in [(0u64, &contents[..10]), (10, &contents[10..20]), (20, &contents[20..])].into_iter().enumerate() {
            assert_eq!(args[1 + i], json!({ "cursor": { "session_id": "S1", "offset": offset }, "close": false }));
            assert_eq!(requests[2 + i].body, bytes, "piece {i}");
            assert_eq!(requests[2 + i].header("Content-Type"), Some("application/octet-stream"));
            assert_eq!(requests[2 + i].header("Authorization"), Some("Bearer sl.ACCESS"));
        }
        assert_eq!(
            args[4],
            json!({
                "cursor": { "session_id": "S1", "offset": 25 },
                "commit": { "path": "/Rec/big.mp4", "mode": "add", "autorename": true, "mute": true },
            })
        );
        assert!(requests[5].body.is_empty());
        assert_eq!(json_of(&requests[6]), json!({ "path": "/Rec/big.mp4" }));
    }

    #[test]
    fn the_real_limits_are_dropboxs() {
        assert_eq!(SINGLE_MAX, 150 * 1024 * 1024);
        assert_eq!(CHUNK % (4 * 1024 * 1024), 0, "Dropbox wants whole 4 MiB pieces but the last");
    }

    #[test]
    fn says_whose_dropbox_it_is() {
        let account = r#"{"account_id":"dbid:1","name":{"given_name":"Dan","surname":"H","display_name":"Dan H"},"email":"d@example.com"}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", account.into())]);
        assert_eq!(test_at(&at(&server), "key1", &mut token("R"), false), Ok("Connected as Dan H".into()));
        let call = &requests.join().unwrap()[1];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/2/users/get_current_account"));
        assert_eq!(call.header("Authorization"), Some("Bearer sl.ACCESS"));
        assert_eq!(call.header("Content-Type"), None, "the call takes no body");
        assert!(call.body.is_empty());
        assert_eq!((call.header("Content-Length"), call.header("Transfer-Encoding")), (Some("0"), None), "sent whole, not in chunks");

        let (server, _) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("200 OK", "", r#"{"email":"d@example.com"}"#.into())]);
        assert_eq!(test_at(&at(&server), "key1", &mut token("R"), false), Ok("Connected as d@example.com".into()));
    }

    #[test]
    fn says_when_the_app_cant_make_links() {
        let account = r#"{"name":{"display_name":"Dan H"}}"#;
        let scoped = |scope: &str| format!(r#"{{"access_token":"sl.ACCESS","token_type":"bearer","expires_in":14400,"scope":"{scope}"}}"#);
        let (server, _) = serve_each(vec![("200 OK", "", scoped("account_info.read files.content.write"))]);
        let error = test_at(&at(&server), "key1", &mut token("R"), true).unwrap_err();
        assert!(error.starts_with("The Dropbox app can't make links: turn on sharing.write"), "{error}");
        // Without links it's fine, and so is an app that may make them.
        let (server, _) = serve_each(vec![("200 OK", "", scoped("account_info.read files.content.write")), ("200 OK", "", account.into())]);
        assert_eq!(test_at(&at(&server), "key1", &mut token("R"), false), Ok("Connected as Dan H".into()));
        let (server, _) = serve_each(vec![("200 OK", "", scoped("account_info.read files.content.write sharing.write")), ("200 OK", "", account.into())]);
        assert_eq!(test_at(&at(&server), "key1", &mut token("R"), true), Ok("Connected as Dan H".into()));
    }

    #[test]
    fn a_revoked_sign_in_says_to_sign_in_again() {
        let refused = r#"{"error":"invalid_grant","error_description":"refresh token is malformed"}"#;
        let (server, _) = serve_each(vec![("400 Bad Request", "", refused.into())]);
        let message = "Dropbox signed RSnip out: sign in again in Settings → Push to";
        assert_eq!(test_at(&at(&server), "key1", &mut token("R"), false), Err(message.into()));
        // The access token is refused too (the app was removed from the account in between).
        let revoked = r#"{"error_summary":"invalid_access_token/...","error":{".tag":"invalid_access_token"}}"#;
        let (server, _) = serve_each(vec![("200 OK", "", TOKEN_REPLY.into()), ("401 Unauthorized", "", revoked.into())]);
        assert_eq!(upload_at(&at(&server), "key1", &mut token("R"), "", false, &file("revoked.png", b"x")), Err(message.into()));
    }

    #[test]
    fn explains_what_dropbox_refuses() {
        let reply = |status, body: &str| Response { status, headers: Vec::new(), body: body.as_bytes().to_vec() };
        let missing = reply(401, r#"{"error_summary":"missing_scope/..","error":{".tag":"missing_scope","required_scope":"sharing.write"}}"#);
        let message = explain(&missing);
        assert!(message.starts_with("The Dropbox app needs the sharing.write permission: turn it on under Permissions"), "{message}");
        assert!(message.ends_with("then sign in again in Settings → Push to"));

        let full = r#"{"error_summary":"path/insufficient_space/..","error":{".tag":"path","reason":{".tag":"insufficient_space"}}}"#;
        assert_eq!(explain(&reply(409, full)), "Your Dropbox is full: make some room, or get more space");
        let finish = r#"{"error_summary":"path/team_folder_insufficient_space/.","error":{".tag":"path","path":{".tag":"team_folder_insufficient_space"}}}"#;
        assert!(explain(&reply(409, finish)).starts_with("Your Dropbox is full"));
        let malformed = r#"{"error_summary":"path/malformed_path/..","error":{".tag":"path","reason":{".tag":"malformed_path","malformed_path":"x"}}}"#;
        assert!(explain(&reply(409, malformed)).starts_with("Dropbox doesn't accept that folder or file name"));
        let denied = r#"{"error_summary":"path/no_write_permission/..","error":{".tag":"path","reason":{".tag":"no_write_permission"}}}"#;
        assert!(explain(&reply(409, denied)).starts_with("The Dropbox app may not write to that folder"));

        let limited = Response { headers: vec![("Retry-After".into(), "30".into())], ..reply(429, r#"{"error_summary":"too_many_requests/..","error":{"reason":{".tag":"too_many_requests"},"retry_after":30}}"#) };
        assert_eq!(explain(&limited), "Dropbox says RSnip is sending too many requests: try again in 30 seconds");
        assert_eq!(explain(&reply(429, "")), "Dropbox says RSnip is sending too many requests: try again later");

        let other = r#"{"error_summary":"path/conflict/file/..","error":{".tag":"path","reason":{".tag":"conflict"}}}"#;
        assert_eq!(explain(&reply(409, other)), "Dropbox said path/conflict/file");
        assert_eq!(explain(&reply(500, "Internal Server Error")), "Dropbox replied 500: Internal Server Error");
    }

    #[test]
    fn header_arguments_are_ascii() {
        let arg = json!({ "path": "/日本 \u{7f}/😀.png", "n": 1 });
        let text = ascii_json(&arg);
        assert!(text.is_ascii() && !text.contains('\u{7f}'), "{text}");
        assert!(text.contains(r"\u65e5\u672c") && text.contains(r"\ud83d\ude00") && text.contains(r"\u007f"), "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), arg, "means the same");
    }

    #[test]
    fn folders_are_tidied_into_paths() {
        assert_eq!(path_of("/Screenshots", "a b.png"), "/Screenshots/a b.png");
        assert_eq!(path_of(" Screenshots/2026\\ ", "a.png"), "/Screenshots/2026/a.png");
        assert_eq!(path_of("/", "a.png"), "/a.png");
        assert_eq!(path_of("", "a.png"), "/a.png");
    }
}
