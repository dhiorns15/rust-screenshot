//! OneDrive: uploads into RSnip's own folder in the user's OneDrive (`Apps/RSnip`, which OneDrive
//! makes for the app), through RSnip's own Microsoft app, signed in to with a code shown in
//! Settings (see `oauth`). It asks only for `Files.ReadWrite.AppFolder` (and `User.Read`, to say
//! whose OneDrive it is), so RSnip can reach nothing else there. What's kept as the secret is the
//! refresh token, which Microsoft replaces each time it's used: the new one is kept each time (see
//! `Saved`). Files over 4 MB go up in an upload session, in pieces.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

use super::{encode, oauth, Saved};
use crate::http::{self, Body, Response};

/// RSnip's Microsoft app: the Application (client) ID of its Entra app registration (for any
/// Entra ID tenant and personal Microsoft accounts, with public client flows allowed). It has no
/// secret. Empty: this build can't sign in to OneDrive.
pub const MICROSOFT_CLIENT_ID: &str = "";

/// For work and school accounts, and personal ones.
const LOGIN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// Only the app's own folder, who the user is, and staying signed in.
const SCOPE: &str = "Files.ReadWrite.AppFolder User.Read offline_access";
/// The app's own folder in the user's OneDrive.
const APP_FOLDER: &str = "me/drive/special/approot";

/// The most that goes up in one request (4 MB, as Microsoft says for a simple upload).
const SINGLE_MAX: u64 = 4 * 1024 * 1024;
/// How much goes in each request of an upload session (10 MiB). Microsoft wants a multiple of
/// 320 KiB.
const CHUNK: u64 = 32 * 320 * 1024;

/// Where Microsoft is, the app it's reached as, and how big an upload may be before it goes in
/// pieces (all different in tests).
struct Service<'a> {
    login: &'a str,
    graph: &'a str,
    client_id: &'a str,
    single_max: u64,
    chunk: u64,
}

const LIVE: Service = Service { login: LOGIN, graph: GRAPH, client_id: MICROSOFT_CLIENT_ID, single_max: SINGLE_MAX, chunk: CHUNK };

/// Whether this build has RSnip's Microsoft app, so it can sign in.
pub fn available() -> bool {
    !LIVE.client_id.is_empty()
}

/// Signs in with a code, which `shown` is given to show, then waits for the user to enter it
/// (until `cancelled`); the refresh token, and whose OneDrive it is if Microsoft says.
pub fn sign_in(cancelled: &AtomicBool, shown: impl FnOnce(&oauth::DeviceCode)) -> Result<(String, Option<String>), String> {
    sign_in_at(&LIVE, cancelled, shown)
}

/// Checks the sign-in, and says whose OneDrive it is.
pub fn test(saved: &mut Saved) -> Result<String, String> {
    test_at(&LIVE, saved)
}

/// Uploads `file` into `folder` in the app's folder (made if it isn't there; empty for the app's
/// folder itself), named as it is or with a number added if one by that name is there already;
/// the link to it, if `share_link`.
pub fn upload(saved: &mut Saved, folder: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    upload_at(&LIVE, saved, folder, share_link, file)
}

fn provider<'a>(service: &Service<'a>) -> oauth::Provider<'a> {
    oauth::Provider {
        name: "OneDrive",
        token_url: format!("{}/token", service.login),
        device_url: format!("{}/devicecode", service.login),
        client_id: service.client_id,
        client_secret: "",
        scope: SCOPE,
        unknown_client: "Microsoft doesn't know this build's app: its Microsoft client ID is wrong, or the app doesn't allow public client flows",
    }
}

/// An error if this build has no Microsoft app to sign in with.
fn ready(service: &Service) -> Result<(), String> {
    if service.client_id.is_empty() {
        return Err("This build of RSnip can't reach OneDrive: it was made without RSnip's Microsoft app".into());
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

/// Whose OneDrive it is: their name, else the address they sign in with, if Microsoft says.
fn whose(service: &Service, token: &str) -> Result<Option<String>, String> {
    let me = answer(request(service, "GET", "me", token, None)?)?;
    Ok(["displayName", "userPrincipalName", "mail"].iter().find_map(|key| me[key].as_str().filter(|s| !s.is_empty())).map(str::to_owned))
}

fn upload_at(service: &Service, saved: &mut Saved, folder: &str, share_link: bool, file: &Path) -> Result<Option<String>, String> {
    ready(service)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let size = std::fs::metadata(file).map_err(|e| format!("Couldn't read {name}: {e}"))?.len();
    let token = oauth::refresh(&provider(service), saved)?.token;
    let folders: Vec<&str> = folder.split(['/', '\\']).map(str::trim).filter(|f| !f.is_empty()).collect();
    let item = format!("{APP_FOLDER}:/{}:", folders.iter().chain([&name.as_str()]).map(|part| encode(part)).collect::<Vec<_>>().join("/"));
    let put = || if size <= service.single_max { simple(service, &token, &item, file) } else { in_pieces(service, &token, &item, file, size) };
    let made = match put()? {
        Ok(made) => made,
        // The folder isn't there: it's made, folder by folder, and the file goes again.
        Err(reply) if reply.status == 404 && !folders.is_empty() => {
            make_folders(service, &token, &folders)?;
            put()?.map_err(|reply| explain(&reply))?
        }
        Err(reply) => return Err(explain(&reply)),
    };
    if !share_link {
        return Ok(None);
    }
    let id = made["id"].as_str().ok_or("OneDrive's reply had no file in it")?;
    // It's there now: a failed link mustn't fail the push, or trying again would put up another copy.
    Ok(link(service, &token, id).ok())
}

/// The first request of an upload: what came of it, or the reply if it didn't succeed (to see
/// whether the folder is missing). The outer error is any other failure.
type Sent = Result<Result<Value, Response>, String>;

/// Uploads a small file in one request.
fn simple(service: &Service, token: &str, item: &str, file: &Path) -> Sent {
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", http::content_type(file))];
    let url = format!("{}/{item}/content?@microsoft.graph.conflictBehavior=rename", service.graph);
    let reply = http::request("PUT", &url, &headers, Body::File(file.to_path_buf()))?;
    Ok(if reply.is_success() { Ok(serde_json::from_slice(&reply.body).unwrap_or_default()) } else { Err(reply) })
}

/// An upload session: it's opened for the file, which is then sent in pieces to the address it
/// gave, each from where Microsoft says it got to.
fn in_pieces(service: &Service, token: &str, item: &str, file: &Path, size: u64) -> Sent {
    let couldnt = |e: std::io::Error| format!("Couldn't read {}: {e}", file.display());
    let renamed = json!({ "item": { "@microsoft.graph.conflictBehavior": "rename" } });
    let started = request(service, "POST", &format!("{item}/createUploadSession"), token, Some(&renamed))?;
    if !started.is_success() {
        return Ok(Err(started));
    }
    let json: Value = serde_json::from_slice(&started.body).unwrap_or_default();
    let session = json["uploadUrl"].as_str().ok_or("OneDrive didn't start the upload")?.to_owned();
    let mut reader = std::fs::File::open(file).map_err(couldnt)?;
    let mut offset = 0;
    loop {
        // All of it arrived, Microsoft says, but the file isn't made: a session gone wrong.
        if offset >= size {
            return Err("OneDrive didn't finish the upload: try again".into());
        }
        let mut piece = Vec::new();
        reader.seek(SeekFrom::Start(offset)).map_err(couldnt)?;
        (&mut reader).take(service.chunk.min(size - offset)).read_to_end(&mut piece).map_err(couldnt)?;
        if piece.is_empty() {
            return Err(format!("{} was shorter than it was a moment ago", file.display()));
        }
        let range = format!("bytes {offset}-{}/{size}", offset + piece.len() as u64 - 1);
        // The session's address is all it needs: no token, which it may even refuse.
        let reply = http::request("PUT", &session, &[("Content-Range", range.as_str())], Body::Bytes(piece))?;
        match reply.status {
            200 | 201 => return Ok(Ok(serde_json::from_slice(&reply.body).unwrap_or_default())),
            202 => {
                // What's still wanted, from the first gap: ["26-"].
                let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
                let next = json["nextExpectedRanges"][0].as_str().and_then(|r| r.split('-').next()?.trim().parse::<u64>().ok());
                match next {
                    Some(next) if next > offset => offset = next,
                    _ => return Err("OneDrive stopped taking the upload: try again".into()),
                }
            }
            _ => return Err(explain(&reply)),
        }
    }
}

/// Makes `folders` in the app's folder, each in the one before; those there already are fine.
fn make_folders(service: &Service, token: &str, folders: &[&str]) -> Result<(), String> {
    let mut parent = APP_FOLDER.to_owned();
    for (i, name) in folders.iter().enumerate() {
        let folder = json!({ "name": name, "folder": {}, "@microsoft.graph.conflictBehavior": "fail" });
        let reply = request(service, "POST", &format!("{parent}/children"), token, Some(&folder))?;
        if !reply.is_success() && reply.status != 409 {
            return Err(explain(&reply));
        }
        parent = format!("{APP_FOLDER}:/{}:", folders[..=i].iter().map(|f| encode(f)).collect::<Vec<_>>().join("/"));
    }
    Ok(())
}

/// A link that opens the file for anyone who has it; where a work or school account may not make
/// those, one for anyone in the organisation.
fn link(service: &Service, token: &str, id: &str) -> Result<String, String> {
    let route = format!("me/drive/items/{}/createLink", encode(id));
    let mut refused = None;
    for scope in ["anonymous", "organization"] {
        let reply = request(service, "POST", &route, token, Some(&json!({ "type": "view", "scope": scope })))?;
        if reply.is_success() {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            return json["link"]["webUrl"].as_str().map(str::to_owned).ok_or_else(|| "OneDrive's reply had no link".to_owned());
        }
        // Signed out, or too many requests, is the same for any link.
        if reply.status == 401 || reply.status == 429 {
            return Err(explain(&reply));
        }
        refused = Some(reply);
    }
    Err(refused.map(|reply| explain(&reply)).unwrap_or_default())
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
    http::request(method, &format!("{}/{route}", service.graph), &headers, body)
}

/// What a reply says if it succeeded, else why not, in words.
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
    let code = error["code"].as_str().unwrap_or_default().to_ascii_lowercase();
    if reply.status == 401 || code == "invalidauthenticationtoken" {
        return "OneDrive signed RSnip out: sign in again in Settings → Push to".into();
    }
    if reply.status == 507 || code == "quotalimitreached" || code == "insufficientstorage" {
        return "Your OneDrive is full: make some room, or get more space".into();
    }
    if reply.status == 429 || code == "activitylimitreached" || code == "throttledrequest" {
        return match reply.header("Retry-After") {
            Some(seconds) => format!("OneDrive says RSnip is sending too many requests: try again in {seconds} seconds"),
            None => "OneDrive says RSnip is sending too many requests: try again later".into(),
        };
    }
    if code == "accessdenied" {
        return "OneDrive won't let RSnip do that: sign in again in Settings → Push to, or ask your organisation's administrator".into();
    }
    if reply.status == 413 {
        return "Too big for OneDrive".into();
    }
    match error["message"].as_str().filter(|m| !m.is_empty()) {
        Some(message) => format!("OneDrive said {}", message.trim_end_matches('.')),
        None => reply.clone().check("OneDrive").err().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::{serve_each, Received};
    use crate::push::oauth::tests::keeping;

    /// Microsoft gives a new refresh token each time.
    fn token_reply(refresh: &str) -> String {
        format!(r#"{{"token_type":"Bearer","scope":"Files.ReadWrite.AppFolder User.Read","expires_in":3599,"access_token":"EwA.ACCESS","refresh_token":"{refresh}"}}"#)
    }

    fn file(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-onedrive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn at(server: &str) -> Service<'_> {
        Service { login: server, graph: server, client_id: "client-1", single_max: SINGLE_MAX, chunk: CHUNK }
    }

    fn token(refresh: &str) -> Saved {
        Saved::unsaved(Some(refresh.to_owned()))
    }

    fn json_of(request: &Received) -> Value {
        serde_json::from_slice(&request.body).unwrap()
    }

    #[test]
    fn puts_a_small_file_in_the_app_folder_and_links_for_anyone() {
        let made = r#"{"id":"01ABC","name":"a b (1).png","webUrl":"https://onedrive.live.com/?id=01ABC"}"#;
        let linked = r#"{"id":"p1","roles":["read"],"link":{"type":"view","scope":"anonymous","webUrl":"https://1drv.ms/i/s!abc"}}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", token_reply("R-2")), ("201 Created", "", made.into()), ("201 Created", "", linked.into())]);
        let png = file("a b.png", b"png bytes");
        assert_eq!(upload_at(&at(&server), &mut token("R-1"), " Screenshots/Dan's ", true, &png), Ok(Some("https://1drv.ms/i/s!abc".into())));

        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/token");
        assert_eq!(requests[0].body, b"grant_type=refresh_token&refresh_token=R-1&client_id=client-1", "no secret: it's a public client");
        let put = &requests[1];
        assert_eq!((put.method.as_str(), put.path.as_str()), ("PUT", "/me/drive/special/approot:/Screenshots/Dan%27s/a%20b.png:/content?@microsoft.graph.conflictBehavior=rename"));
        assert_eq!(put.header("Authorization"), Some("Bearer EwA.ACCESS"));
        assert_eq!(put.header("Content-Type"), Some("image/png"));
        assert_eq!(put.body, b"png bytes");
        let share = &requests[2];
        assert_eq!((share.method.as_str(), share.path.as_str()), ("POST", "/me/drive/items/01ABC/createLink"));
        assert_eq!(json_of(share), json!({ "type": "view", "scope": "anonymous" }));
    }

    #[test]
    fn the_refresh_token_microsoft_gives_back_is_kept_and_used_by_the_next_push() {
        let made = || ("201 Created", "", r#"{"id":"01ABC"}"#.to_owned());
        let (server, requests) = serve_each(vec![("200 OK", "", token_reply("R-2")), made(), ("200 OK", "", token_reply("R-3")), made()]);
        let (mut saved, kept) = keeping("R-1");
        for name in ["one.png", "two.png"] {
            assert_eq!(upload_at(&at(&server), &mut saved, "", false, &file(name, b"x")), Ok(None));
        }
        assert_eq!(*kept.lock().unwrap(), ["R-2", "R-3"], "each one kept");
        assert_eq!(saved.secret(), Some("R-3"));
        let requests = requests.join().unwrap();
        assert!(String::from_utf8_lossy(&requests[2].body).contains("refresh_token=R-2&"), "the second push used the first's");
        assert_eq!(requests[1].path, "/me/drive/special/approot:/one.png:/content?@microsoft.graph.conflictBehavior=rename", "no folder: the app's own");

        // Test connection keeps it too.
        let (server, _) = serve_each(vec![("200 OK", "", token_reply("R-4")), ("200 OK", "", r#"{"displayName":"Dan H","userPrincipalName":"dan@example.com"}"#.into())]);
        assert_eq!(test_at(&at(&server), &mut saved), Ok("Connected as Dan H".into()));
        assert_eq!(kept.lock().unwrap().last().map(String::as_str), Some("R-4"));
    }

    #[test]
    fn a_missing_folder_is_made_and_the_file_goes_again() {
        let missing = r#"{"error":{"code":"itemNotFound","message":"Item does not exist"}}"#;
        let replies = vec![
            ("200 OK", "", token_reply("R-2")),
            ("404 Not Found", "", missing.to_owned()),
            ("201 Created", "", r#"{"id":"F1","name":"Shots"}"#.to_owned()),
            ("409 Conflict", "", r#"{"error":{"code":"nameAlreadyExists","message":"Name already exists"}}"#.to_owned()),
            ("201 Created", "", r#"{"id":"01ABC"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        assert_eq!(upload_at(&at(&server), &mut token("R"), "Shots/2026", false, &file("deep.png", b"x")), Ok(None));
        let requests = requests.join().unwrap();
        let sent: Vec<String> = requests.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        assert_eq!(
            sent[1..],
            [
                "PUT /me/drive/special/approot:/Shots/2026/deep.png:/content?@microsoft.graph.conflictBehavior=rename",
                "POST /me/drive/special/approot/children",
                "POST /me/drive/special/approot:/Shots:/children",
                "PUT /me/drive/special/approot:/Shots/2026/deep.png:/content?@microsoft.graph.conflictBehavior=rename",
            ]
        );
        assert_eq!(json_of(&requests[2]), json!({ "name": "Shots", "folder": {}, "@microsoft.graph.conflictBehavior": "fail" }));
        assert_eq!(json_of(&requests[3])["name"], "2026");

        // Not in a folder, a 404 is just an error.
        let (server, _) = serve_each(vec![("200 OK", "", token_reply("R-2")), ("404 Not Found", "", missing.to_owned())]);
        assert_eq!(upload_at(&at(&server), &mut token("R"), "", false, &file("top.png", b"x")), Err("OneDrive said Item does not exist".into()));
    }

    #[test]
    fn a_big_file_goes_up_in_pieces_from_where_microsoft_got_to() {
        let contents: Vec<u8> = (0..25u8).collect();
        let big = file("big.mp4", &contents);
        let session = vec![
            ("202 Accepted", "", r#"{"expirationDateTime":"2026-10-28T09:21:55Z","nextExpectedRanges":["10-"]}"#.to_owned()),
            // Only part of the second piece arrived: the rest goes again.
            ("202 Accepted", "", r#"{"nextExpectedRanges":["15-24"]}"#.to_owned()),
            ("201 Created", "", r#"{"id":"01BIG","name":"big.mp4","size":25}"#.to_owned()),
        ];
        let (uploads, pieces) = serve_each(session);
        let started = format!(r#"{{"uploadUrl":"{uploads}/up/fe69874","expirationDateTime":"2026-10-28T09:21:55Z"}}"#);
        let linked = r#"{"link":{"webUrl":"https://1drv.ms/v/s!big"}}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", token_reply("R-2")), ("200 OK", "", started), ("201 Created", "", linked.into())]);
        let small = Service { single_max: 10, chunk: 10, ..at(&server) };
        assert_eq!(upload_at(&small, &mut token("R"), "Rec", true, &big), Ok(Some("https://1drv.ms/v/s!big".into())));

        let requests = requests.join().unwrap();
        let start = &requests[1];
        assert_eq!((start.method.as_str(), start.path.as_str()), ("POST", "/me/drive/special/approot:/Rec/big.mp4:/createUploadSession"));
        assert_eq!(json_of(start), json!({ "item": { "@microsoft.graph.conflictBehavior": "rename" } }));
        let pieces = pieces.join().unwrap();
        let sent: Vec<(&str, &[u8])> = pieces.iter().map(|p| (p.header("Content-Range").unwrap(), p.body.as_slice())).collect();
        assert_eq!(sent, [("bytes 0-9/25", &contents[..10]), ("bytes 10-19/25", &contents[10..20]), ("bytes 15-24/25", &contents[15..])]);
        assert!(pieces.iter().all(|p| p.method == "PUT" && p.path == "/up/fe69874" && p.header("Authorization").is_none()), "no token to the session's address");
        assert_eq!(requests[2].path, "/me/drive/items/01BIG/createLink");
    }

    #[test]
    fn the_real_limits_are_microsofts() {
        assert_eq!(SINGLE_MAX, 4 * 1024 * 1024);
        assert_eq!(CHUNK % (320 * 1024), 0, "Microsoft wants whole 320 KiB pieces but the last");
    }

    #[test]
    fn a_tenant_without_links_for_anyone_gets_one_for_the_organisation_or_none() {
        let made = || ("201 Created", "", r#"{"id":"01ABC"}"#.to_owned());
        let refused = r#"{"error":{"code":"invalidRequest","message":"Anonymous sharing links are disabled"}}"#;
        let organisation = r#"{"link":{"type":"view","scope":"organization","webUrl":"https://contoso-my.sharepoint.com/:i:/g/abc"}}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", token_reply("R")), made(), ("400 Bad Request", "", refused.into()), ("201 Created", "", organisation.into())]);
        assert_eq!(upload_at(&at(&server), &mut token("R"), "", true, &file("work.png", b"x")), Ok(Some("https://contoso-my.sharepoint.com/:i:/g/abc".into())));
        assert_eq!(json_of(&requests.join().unwrap()[3]), json!({ "type": "view", "scope": "organization" }));

        // Neither: the file is up, without a link.
        let denied = r#"{"error":{"code":"accessDenied","message":"Access denied"}}"#;
        let (server, _) = serve_each(vec![("200 OK", "", token_reply("R")), made(), ("403 Forbidden", "", denied.into()), ("403 Forbidden", "", denied.into())]);
        assert_eq!(upload_at(&at(&server), &mut token("R"), "", true, &file("nolink.png", b"x")), Ok(None));
    }

    #[test]
    fn signs_in_with_a_code_and_says_as_whom() {
        let started = r#"{"user_code":"FX3QA9UK2","device_code":"DAQABAAEAAA","verification_uri":"https://microsoft.com/devicelogin","expires_in":900,"interval":0,"message":"To sign in, use a web browser"}"#;
        let pending = r#"{"error":"authorization_pending","error_description":"AADSTS70016: OAuth 2.0 device flow error. Authorization is pending."}"#;
        let replies = vec![("200 OK", "", started.to_owned()), ("400 Bad Request", "", pending.to_owned()), ("200 OK", "", token_reply("R-1")), ("200 OK", "", r#"{"userPrincipalName":"dan@example.com"}"#.to_owned())];
        let (server, requests) = serve_each(replies);
        let mut shown = None;
        let result = sign_in_at(&at(&server), &AtomicBool::new(false), |code| shown = Some((code.user_code.clone(), code.verification_url.clone())));
        assert_eq!(result, Ok(("R-1".into(), Some("dan@example.com".into()))));
        assert_eq!(shown, Some(("FX3QA9UK2".into(), "https://microsoft.com/devicelogin".into())));
        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/devicecode");
        assert_eq!(requests[0].body, b"client_id=client-1&scope=Files.ReadWrite.AppFolder%20User.Read%20offline_access");
        assert_eq!(requests[3].path, "/me");
    }

    #[test]
    fn a_build_without_rsnips_microsoft_app_says_so() {
        let none = Service { client_id: "", ..at("http://127.0.0.1:9") };
        let message = "This build of RSnip can't reach OneDrive: it was made without RSnip's Microsoft app";
        assert_eq!(test_at(&none, &mut token("R")), Err(message.into()));
        assert_eq!(upload_at(&none, &mut token("R"), "", true, &file("none.png", b"x")), Err(message.into()));
        assert_eq!(sign_in_at(&none, &AtomicBool::new(false), |_| panic!("no code")), Err(message.into()));
        assert_eq!(available(), !MICROSOFT_CLIENT_ID.is_empty());
    }

    #[test]
    fn a_revoked_sign_in_says_to_sign_in_again() {
        let expired = r#"{"error":"invalid_grant","error_description":"AADSTS700082: The refresh token has expired due to inactivity."}"#;
        let (server, _) = serve_each(vec![("400 Bad Request", "", expired.into())]);
        assert_eq!(test_at(&at(&server), &mut token("R")), Err("OneDrive signed RSnip out: sign in again in Settings → Push to".into()));
        let (server, _) = serve_each(vec![("400 Bad Request", "", r#"{"error":"unauthorized_client","error_description":"AADSTS700016"}"#.into())]);
        assert!(test_at(&at(&server), &mut token("R")).unwrap_err().starts_with("Microsoft doesn't know this build's app"));
    }

    #[test]
    fn explains_what_microsoft_refuses() {
        let reply = |status, body: &str| Response { status, headers: Vec::new(), body: body.as_bytes().to_vec() };
        let error = |status: u16, code: &str, message: &str| reply(status, &json!({ "error": { "code": code, "message": message } }).to_string());
        assert_eq!(explain(&error(401, "InvalidAuthenticationToken", "Access token has expired")), "OneDrive signed RSnip out: sign in again in Settings → Push to");
        assert_eq!(explain(&error(507, "quotaLimitReached", "Insufficient Space Available")), "Your OneDrive is full: make some room, or get more space");
        let limited = Response { headers: vec![("Retry-After".into(), "120".into())], ..error(429, "activityLimitReached", "Too many requests") };
        assert_eq!(explain(&limited), "OneDrive says RSnip is sending too many requests: try again in 120 seconds");
        assert_eq!(explain(&error(503, "throttledRequest", "")), "OneDrive says RSnip is sending too many requests: try again later");
        assert!(explain(&error(403, "accessDenied", "Access denied")).starts_with("OneDrive won't let RSnip do that"));
        assert_eq!(explain(&error(400, "invalidRequest", "Invalid request.")), "OneDrive said Invalid request");
        assert_eq!(explain(&reply(500, "Internal Server Error")), "OneDrive replied 500: Internal Server Error");
    }
}
