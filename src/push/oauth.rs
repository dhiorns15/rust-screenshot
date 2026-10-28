//! Signing in with OAuth, and staying signed in. There are two ways in. Dropbox's is a desktop
//! app's (PKCE, RFC 7636): no app secret to keep, just a one-off code the user copies from
//! Dropbox's page back into RSnip. The other is the device authorization grant (RFC 8628): RSnip
//! shows a short code, and the user enters it on the service's page from any browser, a phone's
//! too. What comes of either is a refresh token, which is what RSnip keeps (see `Saved`) and trades
//! for a short-lived access token at each push. A service may give a new refresh token in place of
//! the one it was given, and then that's the one kept.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{encode, Saved};
use crate::http::{self, Body};

/// Where the user is sent to allow RSnip in Dropbox.
const AUTHORIZE: &str = "https://www.dropbox.com/oauth2/authorize";

/// How long to wait between asking whether a device sign-in is done, when the service doesn't
/// say (RFC 8628 section 3.2), and how much longer after it says to slow down (section 3.5).
const INTERVAL: Duration = Duration::from_secs(5);
const SLOWER: Duration = Duration::from_secs(5);
/// How long a device code lasts, when the service doesn't say.
const EXPIRES: Duration = Duration::from_secs(15 * 60);

/// What a service is, as far as signing in to it goes.
#[cfg_attr(not(test), allow(dead_code))]
pub struct Provider<'a> {
    /// Its name, for messages: "Dropbox".
    pub name: &'a str,
    /// Where codes and refresh tokens are traded for tokens.
    pub token_url: String,
    /// Where a device sign-in starts, for a service that has one.
    pub device_url: String,
    pub client_id: &'a str,
    /// For a service that wants one even from an app that can't keep it secret (Google, for an
    /// installed app); empty for none.
    pub client_secret: &'a str,
    /// The permissions a device sign-in asks for, space-separated.
    pub scope: &'a str,
    /// What to say when the service doesn't know the app.
    pub unknown_client: &'a str,
}

/// One Dropbox sign-in in progress: the secret that proves the code is being used by the one who
/// asked for it. Made when the browser is opened; needed again to trade the code in.
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
        self.exchange_at(&super::dropbox::provider(super::dropbox::API, app_key), code)
    }

    pub(super) fn exchange_at(&self, provider: &Provider, code: &str) -> Result<String, String> {
        let form = format!(
            "grant_type=authorization_code&code={}&code_verifier={}&client_id={}",
            encode(code.trim()),
            self.verifier,
            encode(provider.client_id)
        );
        let reply = token(provider, &form, "Dropbox didn't accept that code: it may have been used already or have expired, so choose Sign in to Dropbox again for a new one")?;
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

/// An access token, good for an hour or a few, with the permissions it carries if the service says.
pub struct Access {
    pub token: String,
    pub scope: Option<String>,
}

/// Trades the refresh token in `saved` for an access token. If the service gives a new refresh
/// token with it (Microsoft does, every time), `saved` takes that in its place.
pub fn refresh(provider: &Provider, saved: &mut Saved) -> Result<Access, String> {
    let refresh_token = saved.secret().unwrap_or_default().to_owned();
    let mut form = format!("grant_type=refresh_token&refresh_token={}&client_id={}", encode(&refresh_token), encode(provider.client_id));
    if !provider.client_secret.is_empty() {
        form.push_str(&format!("&client_secret={}", encode(provider.client_secret)));
    }
    let reply = token(provider, &form, &format!("{} signed RSnip out: sign in again in Settings → Push to", provider.name))?;
    let access = reply["access_token"].as_str().map(str::to_owned).ok_or_else(|| format!("{}'s reply had no access token", provider.name))?;
    if let Some(replaced) = reply["refresh_token"].as_str().filter(|t| !t.is_empty() && *t != refresh_token) {
        saved.replace(replaced);
    }
    Ok(Access { token: access, scope: reply["scope"].as_str().map(str::to_owned) })
}

/// Calls the service's token endpoint with `form`; its reply if it succeeded. `refused` is what to
/// say when the service won't honour the code or refresh token it was given.
fn token(provider: &Provider, form: &str, refused: &str) -> Result<Value, String> {
    let reply = post_form(&provider.token_url, form)?;
    if reply.is_success() {
        return serde_json::from_slice(&reply.body).map_err(|_| format!("{}'s reply made no sense", provider.name));
    }
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    Err(match json["error"].as_str() {
        Some("invalid_grant") => refused.to_owned(),
        Some(error) => refusal(provider, error).unwrap_or_else(|| reply.check(provider.name).err().unwrap_or_default()),
        None => reply.check(provider.name).err().unwrap_or_default(),
    })
}

fn post_form(url: &str, form: &str) -> Result<http::Response, String> {
    let headers = [("Content-Type", "application/x-www-form-urlencoded")];
    http::request("POST", url, &headers, Body::Bytes(form.as_bytes().to_vec()))
}

/// What an OAuth error that isn't about the sign-in itself means, in words.
fn refusal(provider: &Provider, error: &str) -> Option<String> {
    let name = provider.name;
    Some(match error {
        "invalid_client" | "unauthorized_client" => provider.unknown_client.to_owned(),
        "admin_policy_enforced" | "org_internal" => format!("Your organisation doesn't let RSnip use {name}: ask its administrator"),
        "rate_limit_exceeded" => format!("{name} is getting too many sign-ins from RSnip right now: try again in a while"),
        _ => return None,
    })
}

/// A device sign-in under way: the code the user enters, and the page they enter it on.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_url: String,
    device_code: String,
    /// How long to wait between asking whether the user's done.
    interval: Duration,
    /// How long the code lasts.
    expires_in: Duration,
}

/// Signed in on another device: the refresh token to keep, and an access token to use now.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct SignedIn {
    pub refresh_token: String,
    pub access_token: String,
}

/// Asks for a code for the user to enter (RFC 8628 section 3.1).
#[cfg_attr(not(test), allow(dead_code))]
pub fn start_device(provider: &Provider) -> Result<DeviceCode, String> {
    let form = format!("client_id={}&scope={}", encode(provider.client_id), encode(provider.scope));
    let reply = post_form(&provider.device_url, &form)?;
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    if !reply.is_success() {
        // Google says that it's had too many in `error_code`.
        let error = json["error"].as_str().or(json["error_code"].as_str()).unwrap_or_default();
        return Err(refusal(provider, error).unwrap_or_else(|| reply.check(provider.name).err().unwrap_or_default()));
    }
    let text = |key: &str| json[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let seconds = |key: &str, default: Duration| json[key].as_u64().filter(|&s| s > 0).map_or(default, Duration::from_secs);
    let missing = || format!("{}'s reply had no code to sign in with", provider.name);
    Ok(DeviceCode {
        user_code: text("user_code").ok_or_else(missing)?,
        // Google calls it verification_url, as the drafts did; the RFC (and Microsoft) verification_uri.
        verification_url: text("verification_uri").or_else(|| text("verification_url")).ok_or_else(missing)?,
        device_code: text("device_code").ok_or_else(missing)?,
        interval: seconds("interval", INTERVAL),
        expires_in: seconds("expires_in", EXPIRES),
    })
}

/// Waits for the user to sign in with `code` elsewhere, asking the service every so often, until
/// they have (or said no, or the code expired, or `cancelled` was set).
#[cfg_attr(not(test), allow(dead_code))]
pub fn finish_device(provider: &Provider, code: &DeviceCode, cancelled: &AtomicBool) -> Result<SignedIn, String> {
    finish_device_with(provider, code, cancelled, |wait| {
        // In steps, so a cancel is noticed straight away.
        let step = Duration::from_millis(100);
        let mut waited = Duration::ZERO;
        while waited < wait && !cancelled.load(Ordering::Relaxed) {
            std::thread::sleep(step);
            waited += step;
        }
    })
}

/// What a device sign-in is waiting for, after asking.
enum Poll {
    Pending,
    SlowDown,
    Done(SignedIn),
}

/// As `finish_device`, with `pause` doing the waiting (it's to return early once `cancelled` is set).
#[cfg_attr(not(test), allow(dead_code))]
fn finish_device_with(provider: &Provider, code: &DeviceCode, cancelled: &AtomicBool, mut pause: impl FnMut(Duration)) -> Result<SignedIn, String> {
    let mut interval = code.interval;
    let mut waited = Duration::ZERO;
    loop {
        pause(interval);
        waited += interval;
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        match poll(provider, code)? {
            Poll::Done(signed_in) => return Ok(signed_in),
            Poll::Pending => {}
            Poll::SlowDown => interval += SLOWER,
        }
        // The service says when the code has expired, but in case it never does.
        if waited >= code.expires_in {
            return Err(expired());
        }
    }
}

fn expired() -> String {
    "The code expired before you signed in: choose Sign in for a new one".into()
}

/// Asks once whether the user has signed in (RFC 8628 section 3.4).
#[cfg_attr(not(test), allow(dead_code))]
fn poll(provider: &Provider, code: &DeviceCode) -> Result<Poll, String> {
    let mut form = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code={}&client_id={}", encode(&code.device_code), encode(provider.client_id));
    if !provider.client_secret.is_empty() {
        form.push_str(&format!("&client_secret={}", encode(provider.client_secret)));
    }
    let reply = post_form(&provider.token_url, &form)?;
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or_default();
    if reply.is_success() {
        let field = |key: &str| json[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        return match (field("refresh_token"), field("access_token")) {
            (Some(refresh_token), Some(access_token)) => Ok(Poll::Done(SignedIn { refresh_token, access_token })),
            _ => Err(format!("{} signed in but didn't give RSnip a way to stay signed in", provider.name)),
        };
    }
    let name = provider.name;
    match json["error"].as_str().unwrap_or_default() {
        "authorization_pending" => Ok(Poll::Pending),
        "slow_down" => Ok(Poll::SlowDown),
        // Microsoft says authorization_declined.
        "access_denied" | "authorization_declined" => Err(format!("You didn't allow RSnip in {name}: choose Sign in to try again")),
        "expired_token" | "invalid_grant" | "bad_verification_code" => Err(expired()),
        error => Err(refusal(provider, error).unwrap_or_else(|| reply.check(name).err().unwrap_or_default())),
    }
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
pub(super) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::http::tests::serve_each;

    /// A sign-in that remembers what it was given in place of the refresh token.
    pub fn keeping(secret: &str) -> (Saved, Arc<Mutex<Vec<String>>>) {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let into = kept.clone();
        let saved = Saved::new(Some(secret.to_owned()), move |new| {
            into.lock().unwrap().push(new.to_owned());
            Ok(())
        });
        (saved, kept)
    }

    /// A service at `server` that signs in with a device code.
    fn provider<'a>(server: &str, secret: &'a str) -> Provider<'a> {
        Provider {
            name: "Drive",
            token_url: format!("{server}/token"),
            device_url: format!("{server}/device/code"),
            client_id: "client 1",
            client_secret: secret,
            scope: "files offline",
            unknown_client: "Drive doesn't know RSnip",
        }
    }

    fn code(interval: u64) -> DeviceCode {
        DeviceCode { user_code: "ABCD-EFGH".into(), verification_url: "https://example.com/device".into(), device_code: "D 1".into(), interval: Duration::from_secs(interval), expires_in: Duration::from_secs(60) }
    }

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
        let at = |api: &str, key: &str, code: &str| pkce.exchange_at(&super::super::dropbox::provider(api, key), code);
        assert_eq!(at(&api, "key1", " the code+ "), Ok("R-1".into()));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/oauth2/token"));
        assert_eq!(request.header("Content-Type"), Some("application/x-www-form-urlencoded"));
        assert_eq!(request.body, b"grant_type=authorization_code&code=the%20code%2B&code_verifier=verifier&client_id=key1");

        let (api, _) = serve_each(vec![("200 OK", "", r#"{"access_token":"sl.A"}"#.into())]);
        assert!(at(&api, "key1", "c").unwrap_err().contains("way to stay signed in"));
        let used = r#"{"error":"invalid_grant","error_description":"code has been used"}"#;
        let (api, _) = serve_each(vec![("400 Bad Request", "", used.into())]);
        assert!(at(&api, "key1", "c").unwrap_err().starts_with("Dropbox didn't accept that code"));
        let (api, _) = serve_each(vec![("401 Unauthorized", "", r#"{"error":"invalid_client"}"#.into())]);
        assert!(at(&api, "nope", "c").unwrap_err().starts_with("Dropbox doesn't know this app key"));
    }

    #[test]
    fn a_refresh_token_is_traded_for_an_access_token_or_means_signing_in_again() {
        let refresh = |api: &str, key: &str, token: &str| {
            let (mut saved, _) = keeping(token);
            super::refresh(&super::super::dropbox::provider(api, key), &mut saved).map(|access| access.token)
        };
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

    #[test]
    fn a_refresh_token_given_in_place_of_the_old_one_is_kept_and_used_from_then_on() {
        let replies = vec![
            ("200 OK", "", r#"{"access_token":"A1","refresh_token":"R-2","expires_in":3600}"#.to_owned()),
            ("200 OK", "", r#"{"access_token":"A2","refresh_token":"R-2"}"#.to_owned()),
            ("200 OK", "", r#"{"access_token":"A3"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let urls = provider(&server, "s3cret");
        let (mut saved, kept) = keeping("R-1");
        for expected in ["A1", "A2", "A3"] {
            assert_eq!(refresh(&urls, &mut saved).unwrap().token, expected);
        }
        assert_eq!(saved.secret(), Some("R-2"));
        assert_eq!(*kept.lock().unwrap(), ["R-2"], "kept once: the same one again, or none, changes nothing");
        let bodies: Vec<String> = requests.join().unwrap().iter().map(|r| String::from_utf8_lossy(&r.body).into_owned()).collect();
        assert_eq!(bodies[0], "grant_type=refresh_token&refresh_token=R-1&client_id=client%201&client_secret=s3cret", "with the client's secret, when it has one");
        assert!(bodies[1].contains("refresh_token=R-2&") && bodies[2].contains("refresh_token=R-2&"), "the new one is used: {bodies:?}");
    }

    #[test]
    fn a_device_sign_in_shows_a_code_then_waits_slowing_down_when_told() {
        let started = r#"{"device_code":"D 1","user_code":"ABCD-EFGH","verification_url":"https://www.google.com/device","expires_in":1800,"interval":5}"#;
        let (server, requests) = serve_each(vec![("200 OK", "", started.into())]);
        let urls = provider(&server, "");
        let shown = start_device(&urls).unwrap();
        assert_eq!((shown.user_code.as_str(), shown.verification_url.as_str()), ("ABCD-EFGH", "https://www.google.com/device"));
        assert_eq!((shown.interval, shown.expires_in), (Duration::from_secs(5), Duration::from_secs(1800)));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/device/code"));
        assert_eq!(request.body, b"client_id=client%201&scope=files%20offline");

        // Microsoft's, as the RFC has it.
        let started = r#"{"device_code":"D","user_code":"CODE","verification_uri":"https://microsoft.com/devicelogin","message":"..."}"#;
        let (server, _) = serve_each(vec![("200 OK", "", started.into())]);
        let urls = provider(&server, "");
        let shown = start_device(&urls).unwrap();
        assert_eq!(shown.verification_url, "https://microsoft.com/devicelogin");
        assert_eq!((shown.interval, shown.expires_in), (INTERVAL, EXPIRES), "as the RFC says when it isn't said");

        let replies = vec![
            ("428 Precondition Required", "", r#"{"error":"authorization_pending","error_description":"Precondition Required"}"#.to_owned()),
            ("403 Forbidden", "", r#"{"error":"slow_down"}"#.to_owned()),
            ("400 Bad Request", "", r#"{"error":"authorization_pending"}"#.to_owned()),
            ("200 OK", "", r#"{"access_token":"A","expires_in":3599,"refresh_token":"R","scope":"files","token_type":"Bearer"}"#.to_owned()),
        ];
        let (server, requests) = serve_each(replies);
        let urls = provider(&server, "s");
        let mut waits = Vec::new();
        let signed_in = finish_device_with(&urls, &code(5), &AtomicBool::new(false), |wait| waits.push(wait.as_secs()));
        assert_eq!(signed_in, Ok(SignedIn { refresh_token: "R".into(), access_token: "A".into() }));
        assert_eq!(waits, [5, 5, 10, 10], "five seconds more after slow_down");
        let requests = requests.join().unwrap();
        assert_eq!(requests[0].path, "/token");
        assert_eq!(requests[0].body, b"grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=D%201&client_id=client%201&client_secret=s");
    }

    #[test]
    fn a_device_sign_in_ends_when_refused_expired_or_cancelled() {
        let ends = |status: &'static str, body: &str| {
            let (server, _) = serve_each(vec![(status, "", body.to_owned())]);
            let urls = provider(&server, "");
            finish_device_with(&urls, &code(1), &AtomicBool::new(false), |_| {}).unwrap_err()
        };
        let denied = "You didn't allow RSnip in Drive: choose Sign in to try again";
        assert_eq!(ends("403 Forbidden", r#"{"error":"access_denied","error_description":"Forbidden"}"#), denied);
        assert_eq!(ends("400 Bad Request", r#"{"error":"authorization_declined"}"#), denied, "as Microsoft says it");
        let expired = "The code expired before you signed in: choose Sign in for a new one";
        assert_eq!(ends("400 Bad Request", r#"{"error":"expired_token"}"#), expired);
        assert_eq!(ends("401 Unauthorized", r#"{"error":"invalid_client"}"#), "Drive doesn't know RSnip");
        assert_eq!(ends("400 Bad Request", r#"{"error":"admin_policy_enforced"}"#), "Your organisation doesn't let RSnip use Drive: ask its administrator");
        assert_eq!(ends("500 Internal Server Error", ""), "Drive replied 500");

        // A code that runs out without the service saying so.
        let pending = || ("428 Precondition Required", "", r#"{"error":"authorization_pending"}"#.to_owned());
        let (server, _) = serve_each(vec![pending(), pending(), pending()]);
        let urls = provider(&server, "");
        let short = DeviceCode { expires_in: Duration::from_secs(30), ..code(10) };
        assert_eq!(finish_device_with(&urls, &short, &AtomicBool::new(false), |_| {}), Err(expired.into()));

        // Cancelled while waiting: nothing more is asked.
        let (server, requests) = serve_each(vec![pending()]);
        let urls = provider(&server, "");
        let cancelled = AtomicBool::new(false);
        let mut waits = 0;
        let result = finish_device_with(&urls, &code(1), &cancelled, |_| {
            waits += 1;
            if waits == 2 {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(result, Err("Cancelled".into()));
        assert_eq!(requests.join().unwrap().len(), 1, "asked once, before the cancel");
        // The real wait ends early on a cancel.
        let started = std::time::Instant::now();
        let (server, _) = serve_each(Vec::new());
        let urls = provider(&server, "");
        assert_eq!(finish_device(&urls, &code(60), &AtomicBool::new(true)), Err("Cancelled".into()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_device_sign_in_that_cant_start_says_why() {
        let fails = |status: &'static str, body: &str| {
            let (server, _) = serve_each(vec![(status, "", body.to_owned())]);
            let urls = provider(&server, "");
            start_device(&urls).unwrap_err()
        };
        assert_eq!(fails("401 Unauthorized", r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#), "Drive doesn't know RSnip");
        assert_eq!(fails("403 Forbidden", r#"{"error_code":"rate_limit_exceeded"}"#), "Drive is getting too many sign-ins from RSnip right now: try again in a while");
        assert_eq!(fails("400 Bad Request", r#"{"error":"invalid_scope","error_description":"Invalid device flow scope"}"#), "Drive replied 400: Invalid device flow scope");
        assert_eq!(fails("200 OK", r#"{"device_code":"D"}"#), "Drive's reply had no code to sign in with");
    }
}
