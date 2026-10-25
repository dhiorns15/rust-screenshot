//! S3-compatible storage (AWS, Cloudflare R2, Backblaze B2, Wasabi, MinIO): a PUT of the file,
//! signed with Signature Version 4. Addresses are path-style (`<endpoint>/<bucket>/<key>`), which
//! they all take, and the body isn't hashed (`UNSIGNED-PAYLOAD`), so it streams from disk.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::http::{self, Body};

/// Where to put things, and as whom.
pub struct Bucket<'a> {
    /// "https://s3.eu-west-1.amazonaws.com", "https://<account>.r2.cloudflarestorage.com", ...
    pub endpoint: &'a str,
    /// "us-east-1"; R2 takes "auto".
    pub region: &'a str,
    pub bucket: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
}

/// The region to sign for: the one given, else the one in the endpoint's name
/// ("s3.us-east-005.backblazeb2.com", "s3.eu-west-1.amazonaws.com"), "auto" for Cloudflare R2, and
/// otherwise "us-east-1", which MinIO and most others take.
pub fn region(endpoint: &str, given: &str) -> String {
    if !given.trim().is_empty() {
        return given.trim().to_owned();
    }
    let host = authority(endpoint.trim()).unwrap_or_default().split(':').next().unwrap_or_default().to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if host.ends_with(".r2.cloudflarestorage.com") {
        "auto".into()
    } else if labels.len() >= 4 && labels[0] == "s3" {
        labels[1].into()
    } else {
        "us-east-1".into()
    }
}

/// The SHA-256 of nothing, for requests without a body.
const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Checks that the bucket is there and these keys may use it.
pub fn test(b: &Bucket) -> Result<String, String> {
    let reply = send(b, "HEAD", &format!("/{}", encode(b.bucket)), EMPTY, &[], Body::Empty)?;
    match reply.status {
        200..=299 => Ok(format!("Connected to {}", b.bucket)),
        // A HEAD reply has no body to explain itself.
        301 => Err(format!("{} is in another region than {}", b.bucket, b.region)),
        403 => Err("Access denied: check the access key ID, the secret key and that they may use this bucket".into()),
        404 => Err(format!("There's no bucket named {}", b.bucket)),
        _ => Err(reply.check("The storage service").err().unwrap_or_default()),
    }
}

/// Uploads `file` as `key` ("Screenshots/a.png").
pub fn put(b: &Bucket, key: &str, file: &Path) -> Result<(), String> {
    let path = format!("/{}/{}", encode(b.bucket), encode_key(key));
    let headers = [("Content-Type", http::content_type(file))];
    let reply = send(b, "PUT", &path, "UNSIGNED-PAYLOAD", &headers, Body::File(file.to_path_buf()))?;
    if reply.is_success() {
        return Ok(());
    }
    // S3 explains itself in XML: <Error><Code>…</Code><Message>…</Message></Error>.
    match xml_text(&reply.body, "Message").or_else(|| xml_text(&reply.body, "Code")) {
        Some(message) => Err(format!("The storage service replied {}: {message}", reply.status)),
        None => Err(reply.check("The storage service").err().unwrap_or_default()),
    }
}

/// Signs and sends a request for `path` (already encoded) on the bucket's endpoint.
fn send(b: &Bucket, method: &str, path: &str, payload: &str, extra: &[(&str, &str)], body: Body) -> Result<http::Response, String> {
    let endpoint = b.endpoint.trim_end_matches('/');
    let host = authority(endpoint).ok_or("The endpoint has to start with http:// or https://")?;
    let date = amz_date(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    let signed = [("host", host), ("x-amz-content-sha256", payload), ("x-amz-date", date.as_str())];
    let authorization = authorization(b, method, path, "", &signed, payload, &date);
    let mut headers: Vec<(&str, &str)> = vec![("x-amz-content-sha256", payload), ("x-amz-date", &date), ("Authorization", &authorization)];
    headers.extend_from_slice(extra);
    http::request(method, &format!("{endpoint}{path}"), &headers, body)
}

/// The Authorization header for a request, as Signature Version 4 works it out. `headers` are the
/// signed ones, names in lower case and sorted; `date` is `amz_date`'s.
fn authorization(b: &Bucket, method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload: &str, date: &str) -> String {
    authorization_for(b, "s3", method, path, query, headers, payload, date)
}

#[allow(clippy::too_many_arguments)]
fn authorization_for(b: &Bucket, service: &str, method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload: &str, date: &str) -> String {
    let canonical_headers: String = headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect();
    let signed_headers = headers.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(";");
    let request = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload}");
    let day = &date[..8];
    let scope = format!("{day}/{}/{service}/aws4_request", b.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}", hex(&Sha256::digest(request.as_bytes())));
    let mut key = hmac(format!("AWS4{}", b.secret_key).as_bytes(), day.as_bytes());
    for part in [b.region, service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}", b.access_key)
}

/// HMAC-SHA256 (RFC 2104).
fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(data).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `20261024T091500Z`.
fn amz_date(secs: u64) -> String {
    let [year, month, day, hour, minute] = crate::crash::civil(secs);
    format!("{year}{month:02}{day:02}T{hour:02}{minute:02}{:02}Z", secs % 60)
}

/// The host (and port, if written) of an http(s) address.
fn authority(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    Some(authority.rsplit('@').next().unwrap_or(authority))
}

/// What S3 takes unencoded is RFC 3986's unreserved characters, as for any name in an address.
use super::encode;

/// A key, encoded a segment at a time so its slashes stay.
fn encode_key(key: &str) -> String {
    key.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// The text of the first `<tag>…</tag>` in a reply.
fn xml_text(body: &[u8], tag: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let start = text.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = start + text[start..].find(&format!("</{tag}>"))?;
    Some(text[start..end].trim().to_owned()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_as_amazons_examples_do() {
        // From the Signature Version 4 test suite ("get-vanilla").
        let b = Bucket { endpoint: "", region: "us-east-1", bucket: "", access_key: "AKIDEXAMPLE", secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY" };
        let headers = [("host", "example.amazonaws.com"), ("x-amz-date", "20150830T123600Z")];
        assert_eq!(
            authorization_for(&b, "service", "GET", "/", "", &headers, EMPTY, "20150830T123600Z"),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        // From S3's documentation: GET an object, with a Range header.
        let b = Bucket { endpoint: "", region: "us-east-1", bucket: "", access_key: "AKIAIOSFODNN7EXAMPLE", secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY" };
        let headers = [("host", "examplebucket.s3.amazonaws.com"), ("range", "bytes=0-9"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")];
        assert_eq!(
            authorization(&b, "GET", "/test.txt", "", &headers, EMPTY, "20130524T000000Z"),
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn region_comes_from_the_endpoint_unless_given() {
        assert_eq!(region("https://s3.us-east-005.backblazeb2.com", ""), "us-east-005");
        assert_eq!(region("https://s3.eu-west-1.amazonaws.com/", " "), "eu-west-1");
        assert_eq!(region("https://S3.EU-Central-1.wasabisys.com", ""), "eu-central-1");
        assert_eq!(region("https://abc123.r2.cloudflarestorage.com", ""), "auto");
        assert_eq!(region("https://s3.amazonaws.com", ""), "us-east-1");
        assert_eq!(region("http://192.168.1.5:9000", ""), "us-east-1");
        assert_eq!(region("http://minio:9000", ""), "us-east-1");
        assert_eq!(region("https://s3.us-east-005.backblazeb2.com", " eu-west-2 "), "eu-west-2");
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2, and test case 6's key longer than a block.
        assert_eq!(hex(&hmac(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(
            hex(&hmac(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn dates_hosts_keys_and_errors() {
        assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
        assert_eq!(authority("http://localhost:9000/"), Some("localhost:9000"));
        assert_eq!(authority("https://u@acc.r2.cloudflarestorage.com"), Some("acc.r2.cloudflarestorage.com"));
        assert_eq!(authority("s3.amazonaws.com"), None);
        assert_eq!(encode_key("Screens/Screenshot 2026-10-24 091500 (2).png"), "Screens/Screenshot%202026-10-24%20091500%20%282%29.png");
        let error = b"<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message></Error>";
        assert_eq!(xml_text(error, "Message").as_deref(), Some("The specified bucket does not exist"));
        assert_eq!(xml_text(b"<Error></Error>", "Message"), None);
    }
}
