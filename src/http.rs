//! Plain HTTP(S) requests for pushing files to other places: any method, a body that streams from
//! disk, and replies handed back as they come (a 4xx is a reply, not an error). The update check
//! has its own way of fetching (see `update`).

use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// How much of a reply is kept. Anything bigger is cut off.
const MAX_REPLY: u64 = 8 * 1024 * 1024;
/// How long a message from a server is when shown to the user.
const MAX_MESSAGE: usize = 200;

/// One piece of a request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Bytes(Vec<u8>),
    /// Read from disk as it's sent.
    File(PathBuf),
}

/// What a request sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Empty,
    /// A small body of its own (a form, some JSON).
    Bytes(Vec<u8>),
    /// A file, streamed from disk.
    File(PathBuf),
    /// Pieces sent one after the other, as one body.
    Parts(Vec<Part>),
}

/// A server's reply, whatever its status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// A header's value; names don't care about case.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The reply if it succeeded, else a message for the user: "Immich replied 401: Invalid API
    /// key", with what the server said as far as it can be made out.
    pub fn check(self, service: &str) -> Result<Response, String> {
        if self.is_success() {
            return Ok(self);
        }
        Err(match self.server_message() {
            Some(message) => format!("{service} replied {}: {message}", self.status),
            None => format!("{service} replied {}", self.status),
        })
    }

    /// What the server says went wrong: a JSON `message`, `error_description` or `error`, else the
    /// first line of the text.
    fn server_message(&self) -> Option<String> {
        let text = match serde_json::from_slice::<Value>(&self.body) {
            // Valid JSON without any of those says nothing useful.
            Ok(json) => ["message", "error_description", "error"].iter().find_map(|key| match json.get(key)? {
                Value::String(s) => Some(s.clone()),
                // {"error": {"message": "..."}}
                Value::Object(o) => o.get("message")?.as_str().map(str::to_owned),
                _ => None,
            })?,
            Err(_) => String::from_utf8_lossy(&self.body).into_owned(),
        };
        let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        Some(if line.chars().count() > MAX_MESSAGE {
            let cut: String = line.chars().take(MAX_MESSAGE).collect();
            format!("{}...", cut.trim_end())
        } else {
            line.to_owned()
        })
    }
}

/// Sends a request and returns the reply. A 4xx or 5xx is still `Ok`; `Err` means nothing came
/// back (no connection, a timeout, a file that couldn't be read) and is worded for the user.
pub fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Body) -> Result<Response, String> {
    let scheme = url.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https")) {
        // Not the address itself: some carry a secret (a Discord webhook's token).
        return Err("The address has to start with http:// or https://".into());
    }
    let host = host_of(url);
    let (reader, length) = open(body)?;

    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        // WebDAV has methods of its own (MKCOL, PROPFIND).
        .allow_non_standard_methods(true)
        // A redirect would need the body sent again; the caller can follow it with the new address.
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .timeout_per_write(Some(Duration::from_secs(60)))
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .user_agent(format!("RSnip/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut builder = ureq::http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    if let Some(length) = length {
        builder = builder.header("Content-Length", length);
    }
    let sending = match reader {
        Some(reader) => ureq::SendBody::from_owned_reader(reader),
        None => ureq::SendBody::none(),
    };
    let request = builder.body(sending).map_err(|e| format!("Couldn't send a request to {host}: {e}"))?;

    let mut reply = agent.run(request).map_err(|e| format!("Couldn't reach {host}: {e}"))?;
    let status = reply.status().as_u16();
    let headers = reply
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned())))
        .collect();
    let body = reply
        .body_mut()
        .with_config()
        .limit(MAX_REPLY)
        .read_to_vec()
        .map_err(|e| format!("Couldn't reach {host}: {e}"))?;
    Ok(Response { status, headers, body })
}

/// The server in an address, for messages: "https://user@example.com:8443/a?b" is "example.com:8443".
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority.rsplit('@').next().unwrap_or(authority)
}

type Reader = Box<dyn Read + Send>;

/// Opens a body for sending, and works out its length up front (so it goes with a Content-Length
/// rather than in chunks, which many servers refuse).
fn open(body: Body) -> Result<(Option<Reader>, Option<u64>), String> {
    let parts = match body {
        Body::Empty => return Ok((None, None)),
        Body::Bytes(bytes) => vec![Part::Bytes(bytes)],
        Body::File(path) => vec![Part::File(path)],
        Body::Parts(parts) => parts,
    };
    let mut total = 0u64;
    let mut readers: Vec<Reader> = Vec::new();
    for part in parts {
        match part {
            Part::Bytes(bytes) => {
                total += bytes.len() as u64;
                readers.push(Box::new(Cursor::new(bytes)));
            }
            Part::File(path) => {
                let couldnt = |e: io::Error| format!("Couldn't read {}: {e}", path.display());
                let file = std::fs::File::open(&path).map_err(couldnt)?;
                total += file.metadata().map_err(couldnt)?.len();
                readers.push(Box::new(file));
            }
        }
    }
    let mut chain: Reader = Box::new(io::empty());
    for reader in readers.into_iter().rev() {
        chain = Box::new(reader.chain(chain));
    }
    Ok((Some(chain), Some(total)))
}

/// A `multipart/form-data` body, built a field at a time. Files in it are read as they're sent.
pub struct Multipart {
    boundary: String,
    parts: Vec<Part>,
    /// The bytes since the last file, which go out together.
    pending: Vec<u8>,
}

impl Multipart {
    pub fn new() -> Multipart {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let boundary = format!("----RSnip{nanos:x}{:x}{:x}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed));
        Multipart { boundary, parts: Vec::new(), pending: Vec::new() }
    }

    /// A text field.
    pub fn text(mut self, name: &str, value: &str) -> Multipart {
        self.start(&format!("Content-Disposition: form-data; name=\"{}\"", quoted(name)));
        self.pending.extend_from_slice(value.as_bytes());
        self.pending.extend_from_slice(b"\r\n");
        self
    }

    /// A file field, sent under `file_name`.
    pub fn file(mut self, name: &str, file_name: &str, content_type: &str, path: &Path) -> Multipart {
        self.start(&format!(
            "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {content_type}",
            quoted(name),
            quoted(file_name)
        ));
        self.parts.push(Part::Bytes(std::mem::take(&mut self.pending)));
        self.parts.push(Part::File(path.to_path_buf()));
        self.pending.extend_from_slice(b"\r\n");
        self
    }

    /// The Content-Type header to send (it carries the boundary) and the body.
    pub fn finish(mut self) -> (String, Body) {
        self.pending.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        self.parts.push(Part::Bytes(self.pending));
        (format!("multipart/form-data; boundary={}", self.boundary), Body::Parts(self.parts))
    }

    /// The boundary line and headers that open a field.
    fn start(&mut self, headers: &str) {
        self.pending.extend_from_slice(format!("--{}\r\n{headers}\r\n\r\n", self.boundary).as_bytes());
    }
}

impl Default for Multipart {
    fn default() -> Multipart {
        Multipart::new()
    }
}

/// A name or file name fit for between quotes in a header.
fn quoted(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\r' | '\n')).map(|c| if c == '"' { '\'' } else { c }).collect()
}

/// The Content-Type for a file, from its extension.
pub fn content_type(path: &Path) -> &'static str {
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// A pretend server for tests (here and in `push`).
#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// What the test server received.
    pub struct Received {
        pub method: String,
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Received {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }
    }

    /// Answers one request with `status`, `reply_headers` and `reply_body`, and hands back what
    /// it was sent. Returns the address to use.
    fn serve(status: &'static str, reply_headers: &'static str, reply_body: &'static str) -> (String, JoinHandle<Received>) {
        let (url, handle) = serve_each(vec![(status, reply_headers, reply_body.to_owned())]);
        (url, std::thread::spawn(move || handle.join().unwrap().remove(0)))
    }

    /// Answers a request (each on its own connection) with each of `replies` in turn: status,
    /// headers ending in "\r\n" each, body. Hands back what it was sent.
    pub fn serve_each(replies: Vec<(&'static str, &'static str, String)>) -> (String, JoinHandle<Vec<Received>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || replies.into_iter().map(|(status, headers, body)| answer(&listener, status, headers, &body)).collect());
        (url, handle)
    }

    fn answer(listener: &TcpListener, status: &str, reply_headers: &str, reply_body: &str) -> Received {
        {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut data = Vec::new();
            let mut chunk = [0u8; 8192];
            let end = loop {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "the client hung up before finishing its headers");
                data.extend_from_slice(&chunk[..n]);
                if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let head = String::from_utf8(data[..end].to_vec()).unwrap();
            let mut lines = head.split("\r\n");
            let mut first = lines.next().unwrap().split(' ');
            let (method, path) = (first.next().unwrap().to_owned(), first.next().unwrap().to_owned());
            let headers: Vec<(String, String)> =
                lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.to_owned(), v.trim().to_owned())).collect();
            let length = headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, v)| v.parse::<usize>().unwrap());
            let mut body = data[end..].to_vec();
            while body.len() < length {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0, "the client hung up before finishing its body");
                body.extend_from_slice(&chunk[..n]);
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\n{reply_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{reply_body}",
                reply_body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            Received { method, path, headers, body }
        }
    }

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-http-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn puts_a_file_with_its_length() {
        let contents: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let path = temp_file("put.mp4", &contents);
        let (url, server) = serve("201 Created", "ETag: \"abc\"\r\n", "");
        let reply = request("PUT", &format!("{url}/dav/put.mp4"), &[("Authorization", "Basic eDp5")], Body::File(path)).unwrap();
        assert_eq!(reply.status, 201);
        assert_eq!(reply.header("etag"), Some("\"abc\""));
        let got = server.join().unwrap();
        assert_eq!((got.method.as_str(), got.path.as_str()), ("PUT", "/dav/put.mp4"));
        assert_eq!(got.header("content-length"), Some("200000"));
        assert_eq!(got.header("transfer-encoding"), None);
        assert_eq!(got.header("authorization"), Some("Basic eDp5"));
        assert_eq!(got.header("user-agent"), Some(concat!("RSnip/", env!("CARGO_PKG_VERSION"))));
        assert_eq!(got.body, contents);
    }

    #[test]
    fn sends_any_method() {
        let (url, server) = serve("201 Created", "", "");
        let reply = request("MKCOL", &format!("{url}/dav/Screenshots/"), &[], Body::Empty).unwrap();
        assert!(reply.is_success());
        let got = server.join().unwrap();
        assert_eq!((got.method.as_str(), got.path.as_str()), ("MKCOL", "/dav/Screenshots/"));
        assert!(got.body.is_empty());
    }

    #[test]
    fn sends_a_body_with_a_custom_method() {
        let (url, server) = serve("207 Multi-Status", "", "<multistatus/>");
        let reply = request("PROPFIND", &url, &[("Depth", "0")], Body::Bytes(b"<propfind/>".to_vec())).unwrap();
        assert_eq!(reply.body, b"<multistatus/>");
        let got = server.join().unwrap();
        assert_eq!(got.method, "PROPFIND");
        assert_eq!(got.header("depth"), Some("0"));
        assert_eq!(got.body, b"<propfind/>");
    }

    #[test]
    fn sends_multipart_with_a_text_field_and_a_file() {
        let contents = b"\x89PNG not really, but with \r\n--inside it".to_vec();
        let path = temp_file("shot.png", &contents);
        let (type_header, body) = Multipart::new()
            .text("title", "A title")
            .file("file", "shot.png", content_type(&path), &path)
            .text("after", "x")
            .finish();
        let boundary = type_header.strip_prefix("multipart/form-data; boundary=").unwrap().to_owned();
        let (url, server) = serve("200 OK", "", "{}");
        request("POST", &url, &[("Content-Type", &type_header)], body).unwrap();
        let got = server.join().unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nA title\r\n").as_bytes());
        expected.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes(),
        );
        expected.extend_from_slice(&contents);
        expected.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"after\"\r\n\r\nx\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(got.body, expected);
        assert_eq!(got.header("content-length"), Some(expected.len().to_string().as_str()));
        assert_eq!(got.header("content-type"), Some(type_header.as_str()));
    }

    #[test]
    fn boundaries_differ() {
        assert_ne!(Multipart::new().finish().0, Multipart::new().finish().0);
        assert_eq!(quoted("a\"b
c"), "a'bc");
    }

    #[test]
    fn turns_a_json_error_into_a_message() {
        let (url, server) = serve("401 Unauthorized", "", r#"{"statusCode":401,"message":"Invalid API key"}"#);
        let reply = request("GET", &url, &[], Body::Empty).unwrap();
        server.join().unwrap();
        assert_eq!(reply.status, 401);
        assert_eq!(reply.check("Immich").unwrap_err(), "Immich replied 401: Invalid API key");
    }

    #[test]
    fn error_messages_come_from_the_likely_places() {
        let reply = |status, body: &str| Response { status, headers: Vec::new(), body: body.as_bytes().to_vec() };
        let message = |r: Response| r.check("Discord").unwrap_err();
        assert_eq!(message(reply(400, r#"{"error":"bad hook"}"#)), "Discord replied 400: bad hook");
        assert_eq!(message(reply(400, r#"{"error":{"message":"nested"}}"#)), "Discord replied 400: nested");
        assert_eq!(message(reply(400, r#"{"error":"x","error_description":"described"}"#)), "Discord replied 400: described");
        assert_eq!(message(reply(403, "\n  Forbidden here \nsecond line")), "Discord replied 403: Forbidden here");
        assert_eq!(message(reply(500, "")), "Discord replied 500");
        assert_eq!(message(reply(500, r#"{"code":1}"#)), "Discord replied 500");
        let long = message(reply(500, &"y".repeat(500)));
        assert_eq!(long, format!("Discord replied 500: {}...", "y".repeat(200)));
        assert!(reply(204, "").check("Discord").is_ok());
    }

    #[test]
    fn header_names_ignore_case() {
        let reply = Response { status: 200, headers: vec![("Content-Type".into(), "text/plain".into())], body: Vec::new() };
        assert_eq!(reply.header("content-type"), Some("text/plain"));
        assert_eq!(reply.header("CONTENT-TYPE"), Some("text/plain"));
        assert_eq!(reply.header("etag"), None);
    }

    #[test]
    fn says_when_the_server_cant_be_reached() {
        // A port nothing listens on: bind one, then let it go.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let error = request("GET", &format!("http://127.0.0.1:{port}/x"), &[], Body::Empty).unwrap_err();
        assert!(error.starts_with(&format!("Couldn't reach 127.0.0.1:{port}: ")), "{error}");
    }

    #[test]
    fn refuses_other_kinds_of_address_and_missing_files() {
        assert!(request("GET", "ftp://example.com/x", &[], Body::Empty).unwrap_err().contains("http://"));
        let error = request("PUT", "http://127.0.0.1:9/x", &[], Body::File(PathBuf::from("/no/such/file.png"))).unwrap_err();
        assert!(error.starts_with("Couldn't read "), "{error}");
    }

    #[test]
    fn names_the_host() {
        assert_eq!(host_of("https://user:pw@example.com:8443/a?b#c"), "example.com:8443");
        assert_eq!(host_of("http://nas.local"), "nas.local");
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type(Path::new("a.PNG")), "image/png");
        assert_eq!(content_type(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(content_type(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(content_type(Path::new("a.gif")), "image/gif");
        assert_eq!(content_type(Path::new("a.mp4")), "video/mp4");
        assert_eq!(content_type(Path::new("a.webm")), "video/webm");
        assert_eq!(content_type(Path::new("a.rsnip")), "application/octet-stream");
        assert_eq!(content_type(Path::new("noextension")), "application/octet-stream");
    }
}
