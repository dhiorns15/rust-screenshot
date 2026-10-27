//! Signing in to Dropbox as a desktop app does (OAuth with PKCE, RFC 7636): no app secret to keep,
//! just a one-off code the user copies from Dropbox's page back into RSnip. What comes of it is a
//! refresh token, which is what RSnip keeps (see `dropbox`) and trades for a short-lived access
//! token at each push.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::encode;
use crate::http::{self, Body};

/// Where the user is sent to allow RSnip.
const AUTHORIZE: &str = "https://www.dropbox.com/oauth2/authorize";

/// One sign-in in progress: the secret that proves the code is being used by the one who asked
/// for it. Made when the browser is opened; needed again to trade the code in.
#[derive(Clone)]
pub struct Pkce {
    verifier: String,
}

impl Pkce {
    /// A new random verifier: 32 random bytes as 43 characters of base64url.
    pub fn new() -> Result<Pkce, String> {
        let mut bytes = [0u8; 32];
        rustls::crypto::ring::default_provider().secure_random.fill(&mut bytes).map_err(|_| "This computer couldn't make a random number".to_owned())?;
        Ok(Pkce { verifier: base64url(&bytes) })
    }

    /// The page where Dropbox asks the user to allow RSnip and then shows the code to copy (there's
    /// no redirect address: that's what makes it show the code). `offline` is what gets a refresh
    /// token, so RSnip stays signed in.
    pub fn authorize_url(&self, app_key: &str) -> String {
        format!(
            "{AUTHORIZE}?client_id={}&response_type=code&code_challenge={}&code_challenge_method=S256&token_access_type=offline",
            encode(app_key.trim()),
            challenge(&self.verifier)
        )
    }

    /// Trades the code Dropbox showed for the refresh token.
    pub fn exchange(&self, app_key: &str, code: &str) -> Result<String, String> {
        self.exchange_at(super::dropbox::API, app_key, code)
    }

    pub(super) fn exchange_at(&self, api: &str, app_key: &str, code: &str) -> Result<String, String> {
        let form = format!(
            "grant_type=authorization_code&code={}&code_verifier={}&client_id={}",
            encode(code.trim()),
            self.verifier,
            encode(app_key.trim())
        );
        let reply = token(api, &form, "Dropbox didn't accept that code: it may have been used already or have expired, so choose Sign in to Dropbox again for a new one")?;
        reply["refresh_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "Dropbox signed in but didn't give RSnip a way to stay signed in: the app must be allowed to use refresh tokens (don't change its access type)".to_owned())
    }
}

/// The S256 challenge for `verifier`: the hash of it, base64url.
fn challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// Trades a refresh token for an access token, good for a few hours.
pub fn refresh(api: &str, app_key: &str, refresh_token: &str) -> Result<String, String> {
    refresh_with_scopes(api, app_key, refresh_token).map(|(token, _)| token)
}

/// The same, with the permissions the app was given ("files.content.write sharing.write ..."), if
/// Dropbox says.
pub fn refresh_with_scopes(api: &str, app_key: &str, refresh_token: &str) -> Result<(String, Option<String>), String> {
    let form = format!("grant_type=refresh_token&refresh_token={}&client_id={}", encode(refresh_token), encode(app_key.trim()));
    let reply = token(api, &form, "Dropbox signed RSnip out: sign in again in Settings → Push to")?;
    let access = reply["access_token"].as_str().map(str::to_owned).ok_or_else(|| "Dropbox's reply had no access token".to_owned())?;
    Ok((access, reply["scope"].as_str().map(str::to_owned)))
}

/// Calls Dropbox's token endpoint with `form`; its reply if it succeeded. `refused` is what to say
/// when Dropbox won't honour the code or refresh token it was given.
fn token(api: &str, form: &str, refused: &str) -> Result<Value, String> {
    let headers = [("Content-Type", "application/x-www-form-urlencoded")];
    let reply = http::request("POST", &format!("{api}/oauth2/token"), &headers, Body::Bytes(form.as_bytes().to_vec()))?;
    if reply.is_success() {
        return serde_json::from_slice(&reply.body).map_err(|_| "Dropbox's reply made no sense".to_owned());
    }
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    Err(match json["error"].as_str() {
        Some("invalid_grant") => refused.to_owned(),
        Some("invalid_client") => "Dropbox doesn't know this app key: check it against the app's Settings page on dropbox.com/developers".to_owned(),
        _ => reply.check("Dropbox").err().unwrap_or_default(),
    })
}

/// Base64 with the URL-safe alphabet and no padding (RFC 4648 section 5), as PKCE wants.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        // Three bytes make four characters; fewer make one more than they have bytes.
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    #[test]
    fn the_challenge_is_rfc_7636s() {
        // Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifiers_are_random_and_fit_the_rules() {
        let (a, b) = (Pkce::new().unwrap(), Pkce::new().unwrap());
        assert_ne!(a.verifier, b.verifier);
        // 43 to 128 characters of A-Z a-z 0-9 - . _ ~
        assert_eq!(a.verifier.len(), 43);
        assert!(a.verifier.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)), "{}", a.verifier);
    }

    #[test]
    fn base64url_has_no_padding_or_plus_and_slash() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn the_authorize_page_gets_the_challenge_and_asks_for_a_refresh_token() {
        let pkce = Pkce { verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into() };
        assert_eq!(
            pkce.authorize_url(" abc 123 "),
            "https://www.dropbox.com/oauth2/authorize?client_id=abc%20123&response_type=code&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256&token_access_type=offline"
        );
    }

    #[test]
    fn the_code_is_traded_for_a_refresh_token() {
        let reply = r#"{"access_token":"sl.A","token_type":"bearer","expires_in":14400,"refresh_token":"R-1","scope":"files.content.write","uid":"1"}"#;
        let (api, requests) = serve_each(vec![("200 OK", "", reply.into())]);
        let pkce = Pkce { verifier: "verifier".into() };
        assert_eq!(pkce.exchange_at(&api, "key1", " the code+ "), Ok("R-1".into()));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/oauth2/token"));
        assert_eq!(request.header("Content-Type"), Some("application/x-www-form-urlencoded"));
        assert_eq!(request.body, b"grant_type=authorization_code&code=the%20code%2B&code_verifier=verifier&client_id=key1");

        let (api, _) = serve_each(vec![("200 OK", "", r#"{"access_token":"sl.A"}"#.into())]);
        assert!(pkce.exchange_at(&api, "key1", "c").unwrap_err().contains("way to stay signed in"));
        let used = r#"{"error":"invalid_grant","error_description":"code has been used"}"#;
        let (api, _) = serve_each(vec![("400 Bad Request", "", used.into())]);
        assert!(pkce.exchange_at(&api, "key1", "c").unwrap_err().starts_with("Dropbox didn't accept that code"));
        let (api, _) = serve_each(vec![("401 Unauthorized", "", r#"{"error":"invalid_client"}"#.into())]);
        assert!(pkce.exchange_at(&api, "nope", "c").unwrap_err().starts_with("Dropbox doesn't know this app key"));
    }

    #[test]
    fn a_refresh_token_is_traded_for_an_access_token_or_means_signing_in_again() {
        let (api, requests) = serve_each(vec![("200 OK", "", r#"{"access_token":"sl.NEW","token_type":"bearer","expires_in":14400}"#.into())]);
        assert_eq!(refresh(&api, " key1 ", "R 1"), Ok("sl.NEW".into()));
        let request = requests.join().unwrap().remove(0);
        assert_eq!(request.body, b"grant_type=refresh_token&refresh_token=R%201&client_id=key1");
        assert_eq!(request.header("Authorization"), None, "a public app has no secret to send");

        let (api, _) = serve_each(vec![("400 Bad Request", "", r#"{"error":"invalid_grant","error_description":"refresh token is malformed"}"#.into())]);
        assert_eq!(refresh(&api, "key1", "R"), Err("Dropbox signed RSnip out: sign in again in Settings → Push to".into()));
        let (api, _) = serve_each(vec![("500 Internal Server Error", "", String::new())]);
        assert_eq!(refresh(&api, "key1", "R"), Err("Dropbox replied 500".into()));
    }
}
