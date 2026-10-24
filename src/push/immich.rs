//! Immich, the self-hosted photo library: uploads the file as an asset with an API key.

use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::http::{self, Body, Multipart};

/// Checks the server and the API key.
pub fn test(server: &str, key: &str) -> Result<String, String> {
    let reply = http::request("GET", &format!("{}/api/users/me", server.trim_end_matches('/')), &[("x-api-key", key)], Body::Empty)?;
    match reply.status {
        200..=299 => {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            Ok(match json["email"].as_str() {
                Some(email) => format!("Connected as {email}"),
                None => "Connected".into(),
            })
        }
        401 => Err("Immich didn't accept the API key".into()),
        404 => Err("That isn't an Immich server (no /api/users/me there)".into()),
        _ => Err(reply.check("Immich").err().unwrap_or_default()),
    }
}

/// Uploads `file`; the link opens it in Immich. One already there (the same file) counts as done.
pub fn upload(server: &str, key: &str, file: &Path) -> Result<Option<String>, String> {
    let server = server.trim_end_matches('/');
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let meta = std::fs::metadata(file).map_err(|e| format!("Couldn't read {name}: {e}"))?;
    let secs = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let modified = secs(meta.modified());
    let created = match secs(meta.created()) {
        0 => modified,
        created => created.min(modified),
    };
    let (content_type, body) = Multipart::new()
        // Immich tells duplicates apart by this, per device.
        .text("deviceAssetId", &format!("{name}-{}-{modified}", meta.len()))
        .text("deviceId", "RSnip")
        .text("fileCreatedAt", &iso(created))
        .text("fileModifiedAt", &iso(modified))
        .file("assetData", &name, http::content_type(file), file)
        .finish();
    let headers = [("x-api-key", key), ("Content-Type", content_type.as_str()), ("Accept", "application/json")];
    let reply = http::request("POST", &format!("{server}/api/assets"), &headers, body)?;
    match reply.status {
        // 201 created, 200 a duplicate.
        200..=299 => {
            let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
            Ok(json["id"].as_str().map(|id| format!("{server}/photos/{id}")))
        }
        401 => Err("Immich didn't accept the API key".into()),
        403 => Err("This API key may not upload. Give it the asset.upload permission in Immich".into()),
        _ => Err(reply.check("Immich").err().unwrap_or_default()),
    }
}

/// `2026-10-24T09:15:00.000Z`.
fn iso(secs: u64) -> String {
    let [year, month, day, hour, minute] = crate::crash::civil(secs);
    format!("{year}-{month:02}-{day:02}T{hour:02}:{minute:02}:{:02}.000Z", secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_iso_8601() {
        assert_eq!(iso(1_440_938_160), "2015-08-30T12:36:00.000Z");
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
    }
}
