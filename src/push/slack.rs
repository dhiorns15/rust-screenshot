//! Slack: uploads the file to a channel as a Slack app's bot, in Slack's three steps (ask for an
//! upload address, send the file there, then finish it into the channel). The app is the user's
//! own, with the `files:write` scope; its bot token is the secret.

use std::path::Path;

use serde_json::{json, Value};

use super::encode;
use crate::http::{self, Body};

const API: &str = "https://slack.com/api";

/// Checks the token, and says whose it is.
pub fn test(token: &str) -> Result<String, String> {
    test_at(API, token)
}

/// Posts `file` to `channel` (its ID); the link is the file's page in Slack.
pub fn post(token: &str, channel: &str, file: &Path) -> Result<Option<String>, String> {
    post_at(API, token, channel, file)
}

fn test_at(api: &str, token: &str) -> Result<String, String> {
    let reply = call(api, "auth.test", token, "application/x-www-form-urlencoded", Vec::new())?;
    let text = |key: &str| reply[key].as_str().unwrap_or_default().to_owned();
    Ok(format!("Connected as {} to {}", text("user"), text("team")))
}

fn post_at(api: &str, token: &str, channel: &str, file: &Path) -> Result<Option<String>, String> {
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let length = std::fs::metadata(file).map_err(|e| format!("Couldn't read {name}: {e}"))?.len();
    let form = format!("filename={}&length={length}", encode(&name));
    let ticket = call(api, "files.getUploadURLExternal", token, "application/x-www-form-urlencoded", form.into_bytes())?;
    let (Some(upload_url), Some(id)) = (ticket["upload_url"].as_str(), ticket["file_id"].as_str()) else {
        return Err("Slack didn't say where to upload the file".into());
    };
    http::request("POST", upload_url, &[("Content-Type", "application/octet-stream")], Body::File(file.to_path_buf()))?.check("Slack")?;
    let done = json!({ "files": [{ "id": id, "title": name }], "channel_id": channel.trim() });
    let reply = call(api, "files.completeUploadExternal", token, "application/json; charset=utf-8", done.to_string().into_bytes())?;
    Ok(reply["files"][0]["permalink"].as_str().map(str::to_owned))
}

/// Calls a Web API method; its reply if Slack says "ok", else what went wrong, in words.
fn call(api: &str, method: &str, token: &str, content_type: &str, body: Vec<u8>) -> Result<Value, String> {
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("Content-Type", content_type)];
    let reply = http::request("POST", &format!("{api}/{method}"), &headers, Body::Bytes(body))?.check("Slack")?;
    let json: Value = serde_json::from_slice(&reply.body).map_err(|_| "Slack's reply made no sense".to_owned())?;
    if json["ok"].as_bool() == Some(true) {
        return Ok(json);
    }
    Err(explain(json["error"].as_str().unwrap_or("unknown_error")))
}

/// Slack's error code as advice.
fn explain(error: &str) -> String {
    match error {
        "invalid_auth" | "not_authed" | "token_revoked" | "token_expired" | "account_inactive" => {
            "Slack didn't accept the bot token: copy the Bot User OAuth Token (xoxb-…) from the app's OAuth & Permissions page".into()
        }
        "missing_scope" => "The Slack app needs the files:write scope: add it under OAuth & Permissions, then reinstall the app".into(),
        "not_in_channel" => "The app isn't in that channel: invite it there first (/invite @ the app's name)".into(),
        "channel_not_found" | "invalid_channel" => "Slack can't find that channel: use its ID, from the channel's details (it looks like C0123ABCDEF)".into(),
        other => format!("Slack said {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    #[test]
    fn uploads_in_three_steps_and_links_to_the_file() {
        let file = std::env::temp_dir().join(format!("rsnip-slack-{}", std::process::id())).join("a b.png");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"png bytes").unwrap();
        let (files, uploads) = serve_each(vec![("200 OK", "", "OK - 9".into())]);
        let ticket = json!({ "ok": true, "upload_url": format!("{files}/upload/v1/abc"), "file_id": "F123" }).to_string();
        let done = r#"{"ok":true,"files":[{"id":"F123","title":"a b.png","permalink":"https://team.slack.com/files/U1/F123/a_b.png"}]}"#;
        let (api, calls) = serve_each(vec![("200 OK", "", ticket), ("200 OK", "", done.into())]);
        assert_eq!(post_at(&api, "xoxb-1", " C0123 ", &file), Ok(Some("https://team.slack.com/files/U1/F123/a_b.png".into())));

        let calls = calls.join().unwrap();
        assert_eq!(calls[0].path, "/files.getUploadURLExternal");
        assert_eq!(calls[0].header("Authorization"), Some("Bearer xoxb-1"));
        assert_eq!(calls[0].body, b"filename=a%20b.png&length=9");
        let upload = uploads.join().unwrap().remove(0);
        assert_eq!((upload.path.as_str(), upload.body.as_slice()), ("/upload/v1/abc", b"png bytes".as_slice()));
        assert_eq!(calls[1].path, "/files.completeUploadExternal");
        let sent: Value = serde_json::from_slice(&calls[1].body).unwrap();
        assert_eq!(sent, json!({ "files": [{ "id": "F123", "title": "a b.png" }], "channel_id": "C0123" }));
    }

    #[test]
    fn says_whose_token_and_what_went_wrong() {
        let (api, _) = serve_each(vec![("200 OK", "", r#"{"ok":true,"user":"rsnip","team":"Acme"}"#.into())]);
        assert_eq!(test_at(&api, "xoxb-1"), Ok("Connected as rsnip to Acme".into()));
        let (api, _) = serve_each(vec![("200 OK", "", r#"{"ok":false,"error":"invalid_auth"}"#.into())]);
        assert!(test_at(&api, "nope").unwrap_err().starts_with("Slack didn't accept the bot token"));
        assert!(explain("not_in_channel").contains("/invite"));
        assert_eq!(explain("ratelimited"), "Slack said ratelimited");
    }
}
