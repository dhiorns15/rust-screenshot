//! Discord: posts the file to a channel through a webhook. The webhook's address carries its token,
//! so it's kept with the secrets and never shown or put in a message.

use std::path::Path;

use serde_json::Value;

use crate::http::{self, Body, Multipart};

/// Checks that the webhook is there.
pub fn test(webhook: &str) -> Result<String, String> {
    let reply = http::request("GET", webhook, &[], Body::Empty)?;
    match reply.status {
        200..=299 => {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            Ok(match json["name"].as_str() {
                Some(name) => format!("Connected to the {name} webhook"),
                None => "Connected".into(),
            })
        }
        401 | 404 => Err(GONE.into()),
        _ => Err(reply.check("Discord").err().unwrap_or_default()),
    }
}

const GONE: &str = "Discord doesn't know this webhook. It may have been deleted: copy its URL again";

/// Posts `file`; the link is to the attachment Discord keeps.
pub fn post(webhook: &str, file: &Path) -> Result<Option<String>, String> {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let (content_type, body) = Multipart::new().file("files[0]", &name, http::content_type(file), file).finish();
    // Wait for the message, so the reply says where the file went.
    let url = format!("{webhook}{}wait=true", if webhook.contains('?') { '&' } else { '?' });
    let reply = http::request("POST", &url, &[("Content-Type", &content_type)], body)?;
    match reply.status {
        200..=299 => {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            Ok(json["attachments"][0]["url"].as_str().map(str::to_owned))
        }
        401 | 404 => Err(GONE.into()),
        413 => {
            let size = std::fs::metadata(file).map_or(0, |m| m.len());
            Err(format!("Too big for Discord ({:.1} MB). Without a server boost, it takes files up to 10 MB", size as f64 / 1e6))
        }
        _ => Err(reply.check("Discord").err().unwrap_or_default()),
    }
}
