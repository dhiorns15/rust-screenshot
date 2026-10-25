//! A custom uploader: any HTTP upload API, described the way ShareX's custom uploaders are, and
//! imported from their `.sxcu` files. The request is the address, method, headers and form fields
//! as written, with `{secret}` (the token kept in the credential store), `{filename}` and
//! `{base64:…}` filled in; the link comes out of the reply by a template like `{json:data.url}`.

use std::path::Path;

use serde_json::{Map, Value};

use super::{encode, webdav::base64, Kind};
use crate::http::{self, Body, Multipart, Response};

/// How the upload is made, as the destination keeps it.
pub struct Uploader<'a> {
    pub url: &'a str,
    pub method: &'a str,
    /// The form field the file goes in; empty: the file is the whole body.
    pub file_field: &'a str,
    /// Form fields, "name: value" a line.
    pub fields: &'a str,
    /// Headers, "Name: value" a line.
    pub headers: &'a str,
    /// Where the link is in the reply: "{json:data.url}", "{response}", "{header:Location}".
    pub link: &'a str,
    pub secret: &'a str,
}

/// What can be written in the address, headers and fields; the rest are ShareX's that RSnip
/// doesn't fill in.
const REQUEST_PLACEHOLDERS: [&str; 3] = ["secret", "filename", "base64"];
const LINK_PLACEHOLDERS: [&str; 4] = ["json", "response", "header", "filename"];

/// What's wrong with how it's described, if anything, in words for the user.
pub fn problems(u: &Uploader) -> Option<String> {
    if !["POST", "PUT", "PATCH"].contains(&u.method.trim().to_ascii_uppercase().as_str()) {
        return Some("The method has to be POST, PUT or PATCH".into());
    }
    if let Err(e) = pairs(u.headers, "header").and(pairs(u.fields, "form field")) {
        return Some(e);
    }
    if u.file_field.trim().is_empty() && !u.fields.trim().is_empty() {
        return Some("Form fields need a file field to go with: with the file as the whole body there's no form".into());
    }
    unsupported([u.url, u.headers, u.fields].concat().as_str(), &REQUEST_PLACEHOLDERS)
        .or_else(|| unsupported(u.link, &LINK_PLACEHOLDERS))
        .map(|p| format!("RSnip doesn't fill in {{{p}}}"))
}

/// Sends a request without a file to the address, to see that it's there: any reply counts, as
/// an upload address seldom answers anything but an upload.
pub fn test(u: &Uploader) -> Result<String, String> {
    let url = fill_url(u.url, u.secret, "test.png");
    let headers = filled_pairs(u.headers, u.secret, "test.png")?;
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let reply = http::request("GET", &url, &headers, Body::Empty)?;
    Ok(format!("Reached {} (it replied {} to a test). Push something to check the upload itself", http::host_of(&url), reply.status))
}

/// Uploads `file`; the link is what the template takes out of the reply, if it's there.
pub fn upload(u: &Uploader, file: &Path) -> Result<Option<String>, String> {
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let url = fill_url(u.url, u.secret, &name);
    let mut headers = filled_pairs(u.headers, u.secret, &name)?;
    let has_type = headers.iter().any(|(n, _)| n.eq_ignore_ascii_case("Content-Type"));
    let body = if u.file_field.trim().is_empty() {
        if !has_type {
            headers.push(("Content-Type".into(), http::content_type(file).into()));
        }
        Body::File(file.to_path_buf())
    } else {
        let mut form = Multipart::new();
        for (field, value) in filled_pairs(u.fields, u.secret, &name)? {
            form = form.text(&field, &value);
        }
        let (content_type, body) = form.file(u.file_field.trim(), &name, http::content_type(file), file).finish();
        headers.retain(|(n, _)| !n.eq_ignore_ascii_case("Content-Type"));
        headers.push(("Content-Type".into(), content_type));
        body
    };
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let method = u.method.trim().to_ascii_uppercase();
    let reply = http::request(&method, &url, &headers, body)?.check(http::host_of(&url))?;
    Ok(link(u.link, &reply, &name))
}

/// "Name: value" lines as pairs; empty lines are skipped.
fn pairs(text: &str, what: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| match line.split_once(':') {
            Some((name, value)) if !name.trim().is_empty() => Ok((name.trim().to_owned(), value.trim().to_owned())),
            _ => Err(format!("Each {what} needs a name, a colon and its value: \"{line}\"")),
        })
        .collect()
}

fn filled_pairs(text: &str, secret: &str, name: &str) -> Result<Vec<(String, String)>, String> {
    Ok(pairs(text, "line")?.into_iter().map(|(n, v)| (n, fill(&v, secret, name))).collect())
}

/// `text` with `{secret}` and `{filename}` filled in, then `{base64:…}` encoded.
fn fill(text: &str, secret: &str, name: &str) -> String {
    let text = text.replace("{secret}", secret).replace("{filename}", name);
    replace_each(&text, "base64", |inner| Some(base64(inner.as_bytes())))
}

/// The same for an address, where what's filled in is percent-encoded.
fn fill_url(url: &str, secret: &str, name: &str) -> String {
    fill(url.trim(), &encode(secret), &encode(name))
}

/// `text` with each `{kind:…}` replaced by `with` of what's after the colon (`{kind}` gives "").
/// `None` from `with` leaves the whole result `None`.
fn try_replace_each(text: &str, kind: &str, mut with: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(&format!("{{{kind}")) {
        let after = &rest[start + 1 + kind.len()..];
        let (inner, end) = match after.chars().next() {
            Some('}') => ("", 1),
            Some(':') => match after.find('}') {
                Some(close) => (&after[1..close], close + 1),
                None => break,
            },
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
                continue;
            }
        };
        out.push_str(&rest[..start]);
        out.push_str(&with(inner)?);
        rest = &after[end..];
    }
    out.push_str(rest);
    Some(out)
}

fn replace_each(text: &str, kind: &str, with: impl FnMut(&str) -> Option<String>) -> String {
    try_replace_each(text, kind, with).unwrap_or_else(|| text.to_owned())
}

/// The link the template makes of `reply`; `None` if there's no template or the reply hasn't
/// what it asks for.
fn link(template: &str, reply: &Response, name: &str) -> Option<String> {
    let template = template.trim();
    if template.is_empty() {
        return None;
    }
    let json: Option<Value> = serde_json::from_slice(&reply.body).ok();
    let text = String::from_utf8_lossy(&reply.body);
    let link = try_replace_each(template, "json", |path| json_at(json.as_ref()?, path))?;
    let link = try_replace_each(&link, "response", |_| Some(text.trim().to_owned()))?;
    let link = try_replace_each(&link, "header", |name| reply.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))?;
    let link = link.replace("{filename}", &encode(name));
    Some(link.trim().to_owned()).filter(|l| !l.is_empty())
}

/// The text at `path` ("data.url", "files[0].url", "$.data.url") in `json`.
fn json_at(json: &Value, path: &str) -> Option<String> {
    let path = path.trim().trim_start_matches('$').trim_start_matches('.');
    let mut at = json;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        let (key, indexes) = part.split_once('[').map_or((part, ""), |(k, i)| (k, i));
        if !key.is_empty() {
            at = at.get(key)?;
        }
        for index in indexes.split('[').map(|i| i.trim_end_matches(']')).filter(|i| !i.is_empty()) {
            at = at.get(index.parse::<usize>().ok()?)?;
        }
    }
    match at {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The first `{name…}` in `text` that isn't one of `known`.
fn unsupported(text: &str, known: &[&str]) -> Option<String> {
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let next = rest[name.len()..].chars().next();
        if !name.is_empty() && matches!(next, Some('}' | ':')) && !known.contains(&name.as_str()) {
            return Some(name);
        }
    }
    None
}

/// A ShareX custom uploader (`.sxcu`) as a destination's name, its kind, and the token taken out
/// of it (a header, form field or query parameter called something like "token" or "key"), which
/// is left as `{secret}` in its place.
pub fn import(text: &str) -> Result<(String, Kind, Option<String>), String> {
    let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| NOT_SXCU.to_owned())?;
    let text_of = |key: &str| json[key].as_str().unwrap_or_default().trim().to_owned();
    let url = text_of("RequestURL");
    if !json.is_object() || url.is_empty() {
        return Err(NOT_SXCU.into());
    }
    let method = match text_of("RequestMethod") {
        m if m.is_empty() => text_of("RequestType"),
        m => m,
    };
    let method = if method.is_empty() { "POST".to_owned() } else { method.to_ascii_uppercase() };
    let file_field = match (json["Body"].as_str(), text_of("FileFormName")) {
        (Some("MultipartFormData") | None, field) if !field.is_empty() => field,
        (Some("MultipartFormData"), _) => return Err("It sends a form but doesn't say which field the file goes in (FileFormName)".into()),
        (Some("Binary") | None, _) => String::new(),
        (Some(other), _) => return Err(format!("It sends the file as {other}; RSnip sends a form (MultipartFormData) or the file alone (Binary)")),
    };

    let mut parameters = entries(&json["Parameters"]);
    let mut headers = entries(&json["Headers"]);
    let mut arguments = entries(&json["Arguments"]);
    // The token, if there's one: kept as the secret, not in the settings.
    let secretish = |name: &str| ["token", "key", "auth", "secret", "pass"].iter().any(|w| name.to_ascii_lowercase().contains(w));
    let found: Vec<&mut (String, String)> =
        [&mut parameters, &mut headers, &mut arguments].into_iter().flatten().filter(|p| secretish(&p.0) && !p.1.is_empty() && !p.1.contains('{')).collect();
    let secret = match found.len() {
        0 => None,
        1 => found.into_iter().next().map(|p| std::mem::replace(&mut p.1, "{secret}".into())),
        _ => return Err(format!("It has more than one key or token ({}); RSnip keeps one", found.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(", "))),
    };

    let mut url = url;
    if !parameters.is_empty() {
        let query: Vec<String> = parameters.iter().map(|(n, v)| format!("{}={}", encode_keeping(n), encode_keeping(v))).collect();
        url = format!("{url}{}{}", if url.contains('?') { '&' } else { '?' }, query.join("&"));
    }
    let lines = |pairs: &[(String, String)]| pairs.iter().map(|(n, v)| format!("{n}: {v}")).collect::<Vec<_>>().join("\n");
    let kind = Kind::Custom {
        url,
        method,
        file_field,
        fields: lines(&arguments),
        headers: lines(&headers),
        link: dollar_to_braces(&text_of("URL")),
    };
    let Kind::Custom { url, method, file_field, fields, headers, link } = &kind else { unreachable!() };
    let u = Uploader { url, method, file_field, fields, headers, link, secret: "" };
    if let Some(problem) = problems(&u) {
        return Err(problem);
    }
    let name = match text_of("Name") {
        n if n.is_empty() => "Custom uploader".to_owned(),
        n => n,
    };
    Ok((name, kind, secret))
}

const NOT_SXCU: &str = "That isn't a ShareX custom uploader (.sxcu) file";

/// An object's string, number and boolean entries, in order.
fn entries(value: &Value) -> Vec<(String, String)> {
    let empty = Map::new();
    value
        .as_object()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|(name, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(_) | Value::Bool(_) => v.to_string(),
                _ => return None,
            };
            Some((name.clone(), text))
        })
        .collect()
}

/// Percent-encodes `text` for a query, but not its `{placeholders}`.
fn encode_keeping(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else { break };
        out.push_str(&encode(&rest[..start]));
        out.push_str(&rest[start..start + end + 1]);
        rest = &rest[start + end + 1..];
    }
    out.push_str(&encode(rest));
    out
}

/// Older ShareX wrote `$json:data.link$`; now it's `{json:data.link}`.
fn dollar_to_braces(template: &str) -> String {
    if template.contains('{') || !template.contains('$') {
        return template.to_owned();
    }
    let mut out = String::new();
    for (i, part) in template.split('$').enumerate() {
        if i % 2 == 1 { out.push_str(&format!("{{{part}}}")) } else { out.push_str(part) }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    fn uploader<'a>(url: &'a str, file_field: &'a str, fields: &'a str, headers: &'a str, link: &'a str) -> Uploader<'a> {
        Uploader { url, method: "POST", file_field, fields, headers, link, secret: "s3cr3t" }
    }

    /// A file to upload, one per test: tests run at once, and one rewriting a file another is
    /// sending would cut its upload short.
    fn shot(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-custom-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.png");
        std::fs::write(&file, b"png bytes").unwrap();
        file
    }

    #[test]
    fn uploads_a_form_and_takes_the_link_from_the_reply() {
        let reply = r#"{"files":[{"url":"https://i.example.com/u/abc.png"}]}"#;
        let (server, requests) = serve_each(vec![("200 OK", "Content-Type: application/json\r\n", reply.to_owned())]);
        let url = format!("{server}/api/upload?name={{filename}}&k={{secret}}");
        let u = uploader(&url, "file", "folder: shots\nnote: {filename}", "Authorization: {secret}\nX-Basic: Basic {base64:me:{secret}}", "{json:files[0].url}");
        assert_eq!(upload(&u, &shot("form")), Ok(Some("https://i.example.com/u/abc.png".into())));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/api/upload?name=a%20b.png&k=s3cr3t"));
        assert_eq!(request.header("Authorization"), Some("s3cr3t"));
        assert_eq!(request.header("X-Basic"), Some(format!("Basic {}", base64(b"me:s3cr3t")).as_str()));
        let body = String::from_utf8_lossy(&request.body);
        for part in ["name=\"folder\"\r\n\r\nshots", "name=\"note\"\r\n\r\na b.png", "name=\"file\"; filename=\"a b.png\"", "png bytes"] {
            assert!(body.contains(part), "{part} in {body}");
        }
    }

    #[test]
    fn sends_the_file_alone_when_theres_no_file_field() {
        let (server, requests) = serve_each(vec![("201 Created", "Location: https://f.example.com/x\r\n", "https://f.example.com/raw/x\n".into())]);
        let url = format!("{server}/upload/{{filename}}");
        let u = Uploader { method: "put", ..uploader(&url, "", "", "", "{response}") };
        assert_eq!(upload(&u, &shot("raw")), Ok(Some("https://f.example.com/raw/x".into())));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("PUT", "/upload/a%20b.png"));
        assert_eq!((request.header("Content-Type"), request.body.as_slice()), (Some("image/png"), b"png bytes".as_slice()));

        let (server, _) = serve_each(vec![("200 OK", "Location: https://f.example.com/y\r\n", String::new())]);
        let u = uploader(&server, "", "", "", "{header:location}#{filename}");
        assert_eq!(upload(&u, &shot("header")), Ok(Some("https://f.example.com/y#a%20b.png".into())));
    }

    #[test]
    fn says_what_the_server_said_and_needs_no_link() {
        let (server, _) = serve_each(vec![("401 Unauthorized", "", r#"{"error":"bad token"}"#.into())]);
        let error = upload(&uploader(&server, "file", "", "", ""), &shot("fails")).unwrap_err();
        assert!(error.starts_with("127.0.0.1:") && error.ends_with("replied 401: bad token"), "{error}");
        let (server, _) = serve_each(vec![("200 OK", "", r#"{"ok":true}"#.into())]);
        assert_eq!(upload(&uploader(&server, "file", "", "", "{json:data.url}"), &shot("nolink")), Ok(None), "the reply has no link");
        let (server, _) = serve_each(vec![("405 Method Not Allowed", "", String::new())]);
        assert!(test(&uploader(&server, "file", "", "", "")).unwrap().contains("replied 405"));
    }

    #[test]
    fn json_paths_and_placeholders() {
        let json: Value = serde_json::from_str(r#"{"data":{"link":"a","id":7},"files":[["x","y"]]}"#).unwrap();
        assert_eq!(json_at(&json, "data.link").as_deref(), Some("a"));
        assert_eq!(json_at(&json, "$.data.id").as_deref(), Some("7"));
        assert_eq!(json_at(&json, "files[0][1]").as_deref(), Some("y"));
        assert_eq!(json_at(&json, "data.nope"), None);
        assert_eq!(unsupported("https://x/{random:1|9}?{secret}", &REQUEST_PLACEHOLDERS).as_deref(), Some("random"));
        assert_eq!(unsupported("{\"json\": 1} {secret}", &REQUEST_PLACEHOLDERS), None, "braces that aren't placeholders");
        let u = uploader("https://x.example.com", "file", "", "", "{regex:(.*)|1}");
        assert_eq!(problems(&u).as_deref(), Some("RSnip doesn't fill in {regex}"));
        assert_eq!(problems(&uploader("https://x", "", "a: b", "", "")).as_deref().map(|p| p.starts_with("Form fields need")), Some(true));
        assert_eq!(problems(&uploader("https://x", "f", "", "no colon", "")).as_deref(), Some("Each header needs a name, a colon and its value: \"no colon\""));
        assert_eq!(problems(&Uploader { method: "GET", ..uploader("https://x", "f", "", "", "") }).as_deref(), Some("The method has to be POST, PUT or PATCH"));
    }

    #[test]
    fn imports_sharex_uploaders() {
        // Zipline's: the token in a header, the link in the reply's files.
        let zipline = r#"{"Version":"16.0.1","Name":"Zipline","DestinationType":"ImageUploader, FileUploader","RequestMethod":"POST",
            "RequestURL":"https://zipline.example.com/api/upload","Headers":{"Authorization":"eyJabc.def","x-zipline-format":"random"},
            "Body":"MultipartFormData","FileFormName":"file","URL":"{json:files[0].url}"}"#;
        let (name, kind, secret) = import(zipline).unwrap();
        assert_eq!((name.as_str(), secret.as_deref()), ("Zipline", Some("eyJabc.def")));
        assert_eq!(
            kind,
            Kind::Custom {
                url: "https://zipline.example.com/api/upload".into(),
                method: "POST".into(),
                file_field: "file".into(),
                fields: String::new(),
                headers: "Authorization: {secret}\nx-zipline-format: random".into(),
                link: "{json:files[0].url}".into(),
            }
        );

        // XBackBone's (older ShareX): the token a form field, the link in $json:…$.
        let xbackbone = r#"{"RequestType":"POST","RequestURL":"https://xbb.example.com/upload","FileFormName":"upload",
            "Arguments":{"file":"%filename","token":"token_abc"},"URL":"$json:url$"}"#;
        let (name, kind, secret) = import(xbackbone).unwrap();
        assert_eq!((name.as_str(), secret.as_deref()), ("Custom uploader", Some("token_abc")));
        let Kind::Custom { fields, link, file_field, .. } = kind else { panic!() };
        assert_eq!((fields.as_str(), link.as_str(), file_field.as_str()), ("file: %filename\ntoken: {secret}", "{json:url}", "upload"));

        // A key in the query; the file as the whole body.
        let query = r#"{"Name":"Raw","RequestMethod":"PUT","RequestURL":"https://h.example.com/up?x=1","Parameters":{"key":"k 1","name":"{filename}"},"Body":"Binary","URL":"{response}"}"#;
        let (_, kind, secret) = import(query).unwrap();
        assert_eq!(secret.as_deref(), Some("k 1"));
        let Kind::Custom { url, file_field, method, .. } = kind else { panic!() };
        assert_eq!((url.as_str(), file_field.as_str(), method.as_str()), ("https://h.example.com/up?x=1&key={secret}&name={filename}", "", "PUT"));

        // What RSnip can't do is said, by name.
        assert_eq!(import("not json"), Err(NOT_SXCU.into()));
        assert_eq!(import(r#"{"Name":"x"}"#), Err(NOT_SXCU.into()));
        let json_body = r#"{"RequestURL":"https://h.example.com","Body":"JSON","Data":"{}"}"#;
        assert!(import(json_body).unwrap_err().contains("as JSON"));
        let regex = r#"{"RequestURL":"https://h.example.com","Body":"MultipartFormData","FileFormName":"f","URL":"{regex:href=\"(.*)\"|1}"}"#;
        assert_eq!(import(regex), Err("RSnip doesn't fill in {regex}".into()));
        let two = r#"{"RequestURL":"https://h.example.com","FileFormName":"f","Headers":{"X-Api-Key":"a"},"Arguments":{"token":"b"}}"#;
        assert_eq!(import(two), Err("It has more than one key or token (X-Api-Key, token); RSnip keeps one".into()));
    }
}
