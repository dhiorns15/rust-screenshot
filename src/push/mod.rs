//! Places RSnip sends screenshots and recordings to ("destinations"), each set to push new items
//! by itself or only when asked. This holds what a destination is, how the list is kept, and what
//! counts as a complete one. The secrets (passwords, keys, webhook URLs) are not here: they live in
//! the system's credential store (see `secrets`), by the destination's id. Each kind's talking to
//! its server is in a module of its own.

mod custom;
mod discord;
mod immich;
mod s3;
mod sftp;
mod slack;
mod webdav;

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::gallery::env::Env;
use crate::options;

/// One place to push to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Destination {
    /// Short and unique; names the destination's secret in the credential store. Never changes.
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: Kind,
    /// New items are pushed as they're saved (otherwise only when chosen).
    pub auto: bool,
    /// When it was set to push automatically (Unix seconds); only what's saved after is pushed.
    pub auto_since: Option<u64>,
}

/// What kind of place it is, and what's needed to reach it apart from the secret.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Kind {
    /// A folder on a server over SSH. No secret: it uses the user's SSH keys and agent.
    Sftp {
        host: String,
        #[serde(default = "default_port")]
        port: u16,
        user: String,
        folder: String,
        #[serde(default)]
        public_url: String,
    },
    /// A WebDAV folder. Secret: the password.
    WebDav {
        /// The folder's address.
        url: String,
        user: String,
        #[serde(default)]
        public_url: String,
    },
    /// An S3-compatible bucket. Secret: the secret access key.
    S3 {
        endpoint: String,
        region: String,
        bucket: String,
        #[serde(default)]
        prefix: String,
        access_key: String,
        #[serde(default)]
        public_url: String,
    },
    /// A Discord channel's webhook. Secret: the webhook's address, as it has the token in it.
    Discord {},
    /// An Immich server. Secret: an API key.
    Immich { server: String },
    /// A Slack channel, posted to as the user's own Slack app. Secret: its bot token.
    Slack {
        /// The channel's ID ("C0123ABCDEF"), not its name.
        channel: String,
    },
    /// Any HTTP upload API, described as ShareX's custom uploaders are (see `custom`). Secret, if it
    /// needs one: a token, put wherever `{secret}` is written.
    Custom {
        url: String,
        #[serde(default)]
        method: String,
        /// The form field the file goes in; empty: the file is the whole body.
        #[serde(default)]
        file_field: String,
        /// Form fields, "name: value" a line.
        #[serde(default)]
        fields: String,
        /// Headers, "Name: value" a line.
        #[serde(default)]
        headers: String,
        /// Where the link is in the reply, e.g. "{json:data.url}".
        #[serde(default)]
        link: String,
    },
}

fn default_port() -> u16 {
    22
}

impl Kind {
    /// An empty one of each kind, in the order they're offered.
    pub const ALL: [fn() -> Kind; 7] = [Kind::sftp, Kind::webdav, Kind::s3, Kind::discord, Kind::slack, Kind::immich, Kind::custom];

    pub fn sftp() -> Kind {
        Kind::Sftp { host: String::new(), port: default_port(), user: String::new(), folder: String::new(), public_url: String::new() }
    }

    pub fn webdav() -> Kind {
        Kind::WebDav { url: String::new(), user: String::new(), public_url: String::new() }
    }

    pub fn s3() -> Kind {
        Kind::S3 {
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            prefix: String::new(),
            access_key: String::new(),
            public_url: String::new(),
        }
    }

    pub fn discord() -> Kind {
        Kind::Discord {}
    }

    pub fn immich() -> Kind {
        Kind::Immich { server: String::new() }
    }

    pub fn slack() -> Kind {
        Kind::Slack { channel: String::new() }
    }

    pub fn custom() -> Kind {
        Kind::Custom {
            url: String::new(),
            method: "POST".into(),
            file_field: "file".into(),
            fields: String::new(),
            headers: String::new(),
            link: String::new(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Kind::Sftp { .. } => "SFTP",
            Kind::WebDav { .. } => "WebDAV",
            Kind::S3 { .. } => "S3-compatible",
            Kind::Discord { .. } => "Discord",
            Kind::Immich { .. } => "Immich",
            Kind::Slack { .. } => "Slack",
            Kind::Custom { .. } => "Custom uploader",
        }
    }

    /// What the secret is called, for kinds that have one.
    pub fn secret_label(&self) -> Option<&'static str> {
        match self {
            Kind::Sftp { .. } => None,
            Kind::WebDav { .. } => Some("Password"),
            Kind::S3 { .. } => Some("Secret access key"),
            Kind::Discord { .. } => Some("Webhook URL"),
            Kind::Immich { .. } => Some("API key"),
            Kind::Slack { .. } => Some("Bot token"),
            Kind::Custom { .. } => Some("Token"),
        }
    }

    /// Whether it can do without its secret: a custom uploader may need no token.
    pub fn secret_optional(&self) -> bool {
        matches!(self, Kind::Custom { .. })
    }

    /// Where it is, for showing in a list; never the secret.
    pub fn target(&self) -> String {
        match self {
            Kind::Sftp { host, user, folder, .. } => {
                let at = if user.is_empty() { host.clone() } else { format!("{user}@{host}") };
                if folder.is_empty() { at } else { format!("{at}:{folder}") }
            }
            Kind::WebDav { url, .. } => url.clone(),
            Kind::S3 { endpoint, bucket, prefix, .. } => {
                let prefix = prefix.trim_matches('/');
                if prefix.is_empty() { format!("{bucket} on {endpoint}") } else { format!("{bucket}/{prefix} on {endpoint}") }
            }
            Kind::Discord { .. } => String::new(),
            Kind::Immich { server } => server.clone(),
            Kind::Slack { channel } => channel.clone(),
            // Without the query, which may carry a key.
            Kind::Custom { url, .. } => url.split('?').next().unwrap_or_default().to_owned(),
        }
    }
}

impl Destination {
    /// A new, empty destination of `kind`, named for it, pushing by hand.
    pub fn new(kind: Kind) -> Destination {
        Destination { id: new_id(), name: kind.name().to_owned(), kind, auto: false, auto_since: None }
    }

    /// What's missing or wrong before it can be saved, in words for the user.
    pub fn problems(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Give it a name".into());
        }
        let required = |value: &str, what: &str| value.trim().is_empty().then(|| format!("Enter {what}"));
        let address = |value: &str, what: &str| {
            if value.trim().is_empty() {
                Some(format!("Enter {what}"))
            } else if !is_web_address(value) {
                Some(format!("{} must start with http:// or https://", capitalised(what)))
            } else {
                None
            }
        };
        // Optional, but a web address if given.
        let public = |value: &str| {
            (!value.trim().is_empty() && !is_web_address(value)).then(|| "The link to share must start with http:// or https://".to_owned())
        };
        match &self.kind {
            // No user name is fine: ~/.ssh/config or the local one is used.
            Kind::Sftp { host, port, public_url, .. } => required(host, "the server's address")
                .or_else(|| (*port == 0).then(|| "Enter a port from 1 to 65535".to_owned()))
                .or_else(|| public(public_url)),
            Kind::WebDav { url, user, public_url } => {
                address(url, "the folder's address").or_else(|| required(user, "the user name")).or_else(|| public(public_url))
            }
            // No region is fine: it's worked out from the endpoint (s3::region).
            Kind::S3 { endpoint, bucket, access_key, public_url, .. } => address(endpoint, "the endpoint")
                .or_else(|| required(bucket, "the bucket's name"))
                .or_else(|| required(access_key, "the access key"))
                .or_else(|| public(public_url)),
            Kind::Discord { .. } => None,
            Kind::Immich { server } => address(server, "the server's address"),
            Kind::Slack { channel } if channel.trim().starts_with('#') => Some("Use the channel's ID, from its details (like C0123ABCDEF), not its name".into()),
            Kind::Slack { channel } => required(channel, "the channel's ID"),
            Kind::Custom { url, .. } => address(url, "the upload address").or_else(|| custom::problems(&uploader(&self.kind, ""))),
        }
    }
}

/// "the folder's address" as "The folder's address".
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Starts with http:// or https:// and has something after it.
fn is_web_address(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    ["http://", "https://"].iter().any(|scheme| lower.len() > scheme.len() && lower.starts_with(scheme))
}

/// A short id that's unique on this computer: the time in milliseconds and a count, in hex.
fn new_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{millis:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// The saved destinations; none if there are none or the list is damaged.
pub fn load(env: &Env) -> Vec<Destination> {
    env.text_setting(options::DESTINATIONS).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

/// Saves the list of destinations (not their secrets).
pub fn save(env: &Env, destinations: &[Destination]) {
    if let Ok(json) = serde_json::to_string(destinations) {
        env.set_text_setting(options::DESTINATIONS, &json);
    }
}

/// Checks that `destination` can be reached with `secret` (its password, key or webhook address),
/// without changing anything there, and says what it found.
pub fn test(destination: &Destination, secret: Option<&str>) -> Result<String, String> {
    let secret = secret.unwrap_or_default();
    match &destination.kind {
        Kind::Sftp { host, port, user, folder, .. } => sftp::test(&sftp::Server { host, port: *port, user, folder }),
        Kind::WebDav { url, user, .. } => webdav::test(&webdav::Folder { url, user, password: secret }),
        Kind::S3 { endpoint, region, bucket, access_key, .. } => {
            s3::test(&s3::Bucket { endpoint, region: &s3::region(endpoint, region), bucket, access_key, secret_key: secret })
        }
        Kind::Discord {} => discord::test(secret),
        Kind::Immich { server } => immich::test(server, secret),
        Kind::Slack { .. } => slack::test(secret),
        Kind::Custom { .. } => custom::test(&uploader(&destination.kind, secret)),
    }
}

/// Sends `file` to `destination`, with its secret; the link to it there, if there is one.
pub fn push(destination: &Destination, secret: Option<&str>, file: &Path) -> Result<Option<String>, String> {
    let secret = secret.unwrap_or_default();
    let name = file.file_name().ok_or("Not a file")?.to_string_lossy().into_owned();
    match &destination.kind {
        Kind::Sftp { host, port, user, folder, public_url } => {
            sftp::put(&sftp::Server { host, port: *port, user, folder }, &name, file)?;
            Ok(link(public_url, &name))
        }
        Kind::WebDav { url, user, public_url } => {
            webdav::put(&webdav::Folder { url, user, password: secret }, &name, file)?;
            Ok(link(public_url, &name))
        }
        Kind::S3 { endpoint, region, bucket, prefix, access_key, public_url } => {
            let prefix = prefix.trim_matches('/');
            let key = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            s3::put(&s3::Bucket { endpoint, region: &s3::region(endpoint, region), bucket, access_key, secret_key: secret }, &key, file)?;
            Ok(link(public_url, &key))
        }
        Kind::Discord {} => discord::post(secret, file),
        Kind::Immich { server } => immich::upload(server, secret, file),
        Kind::Slack { channel } => slack::post(secret, channel, file),
        Kind::Custom { .. } => custom::upload(&uploader(&destination.kind, secret), file),
    }
}

/// A custom uploader's description, with `secret`.
fn uploader<'a>(kind: &'a Kind, secret: &'a str) -> custom::Uploader<'a> {
    let Kind::Custom { url, method, file_field, fields, headers, link } = kind else { unreachable!("not a custom uploader") };
    custom::Uploader { url, method, file_field, fields, headers, link, secret }
}

/// A ShareX custom uploader file (`.sxcu`) as a destination, and the token taken out of it.
pub fn import_sxcu(text: &str) -> Result<(Destination, Option<String>), String> {
    let (name, kind, secret) = custom::import(text)?;
    Ok((Destination { name, ..Destination::new(kind) }, secret))
}

/// The secret saved for `destination`, which may wait for the keyring to be unlocked; an error if
/// it needs one and has none.
pub fn saved_secret(destination: &Destination) -> Result<Option<String>, String> {
    let Some(label) = destination.kind.secret_label() else { return Ok(None) };
    match crate::secrets::get(&destination.id)? {
        Some(secret) => Ok(Some(secret)),
        None if destination.kind.secret_optional() => Ok(None),
        None => Err(format!("No {} is saved; choose Edit to enter it", label.to_lowercase())),
    }
}

/// Where something pushed as `path` can be seen: under the public address given, if any.
fn link(public_url: &str, path: &str) -> Option<String> {
    let base = public_url.trim().trim_end_matches('/');
    (!base.is_empty()).then(|| format!("{base}/{}", path.split('/').map(encode).collect::<Vec<_>>().join("/")))
}

/// Percent-encodes all but the unreserved characters (RFC 3986), for a name in an address.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::serve_each;

    fn every_kind() -> Vec<Kind> {
        Kind::ALL.iter().map(|make| make()).collect()
    }

    /// A screenshot to push, with a space in its name.
    fn shot(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rsnip-push-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.png");
        std::fs::write(&file, b"png bytes").unwrap();
        file
    }

    fn to(kind: Kind) -> Destination {
        Destination::new(kind)
    }

    #[test]
    fn webdav_makes_the_folder_then_puts() {
        let reply = |status| (status, "", String::new());
        let (server, requests) = serve_each(vec![reply("409 Conflict"), reply("409 Conflict"), reply("201 Created"), reply("201 Created"), reply("201 Created")]);
        let d = to(Kind::WebDav { url: format!("{server}/dav/shots"), user: "me".into(), public_url: "https://files.example.com/shots/".into() });
        let link = push(&d, Some("pw"), &shot("webdav")).unwrap();
        assert_eq!(link.as_deref(), Some("https://files.example.com/shots/a%20b.png"));
        let requests = requests.join().unwrap();
        let sent: Vec<String> = requests.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
        assert_eq!(sent, ["PUT /dav/shots/a%20b.png", "MKCOL /dav/shots/", "MKCOL /dav/", "MKCOL /dav/shots/", "PUT /dav/shots/a%20b.png"]);
        assert_eq!(requests[0].header("Authorization"), Some("Basic bWU6cHc="));
        assert_eq!(requests[4].body, b"png bytes");
        assert_eq!(requests[4].header("Content-Type"), Some("image/png"));

        let (server, _) = serve_each(vec![reply("401 Unauthorized")]);
        let d = to(Kind::WebDav { url: server, user: "me".into(), public_url: String::new() });
        assert_eq!(test(&d, Some("wrong")), Err("The server didn't accept the user name or password".into()));
    }

    #[test]
    fn s3_puts_a_signed_object_under_the_prefix() {
        let (server, requests) = serve_each(vec![("200 OK", "", String::new())]);
        let bucket = Kind::S3 {
            endpoint: server.clone(),
            region: "auto".into(),
            bucket: "shots".into(),
            prefix: "/rsnip/".into(),
            access_key: "AKID".into(),
            public_url: "https://cdn.example.com".into(),
        };
        let link = push(&to(bucket.clone()), Some("secret"), &shot("s3")).unwrap();
        assert_eq!(link.as_deref(), Some("https://cdn.example.com/rsnip/a%20b.png"));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str()), ("PUT", "/shots/rsnip/a%20b.png"));
        assert_eq!(request.header("x-amz-content-sha256"), Some("UNSIGNED-PAYLOAD"));
        let auth = request.header("Authorization").unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKID/") && auth.contains("/auto/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="), "{auth}");
        assert_eq!(request.body, b"png bytes");

        let xml = "<?xml version=\"1.0\"?>\n<Error><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match</Message></Error>";
        let (server, _) = serve_each(vec![("403 Forbidden", "", xml.to_owned())]);
        let Kind::S3 { region, bucket, prefix, access_key, public_url, .. } = bucket else { unreachable!() };
        let wrong = to(Kind::S3 { endpoint: server, region, bucket, prefix, access_key, public_url });
        assert_eq!(push(&wrong, Some("wrong"), &shot("s3-wrong")), Err("The storage service replied 403: The request signature we calculated does not match".into()));
    }

    #[test]
    fn discord_posts_the_file_and_links_to_it() {
        let reply = r#"{"id":"1","attachments":[{"url":"https://cdn.discordapp.com/attachments/1/2/a_b.png"}]}"#;
        let (server, requests) = serve_each(vec![("200 OK", "Content-Type: application/json\r\n", reply.to_owned())]);
        let webhook = format!("{server}/api/webhooks/1/secret-token");
        let link = push(&to(Kind::discord()), Some(&webhook), &shot("discord")).unwrap();
        assert_eq!(link.as_deref(), Some("https://cdn.discordapp.com/attachments/1/2/a_b.png"));
        let request = requests.join().unwrap().remove(0);
        assert_eq!(request.path, "/api/webhooks/1/secret-token?wait=true");
        let body = String::from_utf8_lossy(&request.body);
        assert!(body.contains("name=\"files[0]\"; filename=\"a b.png\"") && body.contains("png bytes"), "{body}");

        let (server, _) = serve_each(vec![("404 Not Found", "", "{\"message\": \"Unknown Webhook\"}".into())]);
        let error = test(&to(Kind::discord()), Some(&format!("{server}/api/webhooks/1/secret-token"))).unwrap_err();
        assert!(error.contains("doesn't know this webhook") && !error.contains("secret-token"), "never shows the token: {error}");
        let (server, _) = serve_each(vec![("413 Payload Too Large", "", String::new())]);
        let error = push(&to(Kind::discord()), Some(&format!("{server}/api/webhooks/1/t")), &shot("discord-big")).unwrap_err();
        assert!(error.starts_with("Too big for Discord"), "{error}");
    }

    #[test]
    fn immich_uploads_an_asset_and_links_to_it() {
        let (server, requests) = serve_each(vec![("201 Created", "", r#"{"id":"7c1e","status":"created"}"#.to_owned())]);
        let link = push(&to(Kind::Immich { server: format!("{server}/") }), Some("KEY"), &shot("immich")).unwrap();
        assert_eq!(link, Some(format!("{server}/photos/7c1e")));
        let request = requests.join().unwrap().remove(0);
        assert_eq!((request.method.as_str(), request.path.as_str(), request.header("x-api-key")), ("POST", "/api/assets", Some("KEY")));
        let body = String::from_utf8_lossy(&request.body);
        for field in ["name=\"deviceId\"\r\n\r\nRSnip", "name=\"fileCreatedAt\"", "name=\"fileModifiedAt\"", "name=\"assetData\"; filename=\"a b.png\""] {
            assert!(body.contains(field), "{field} in {body}");
        }

        let (server, requests) = serve_each(vec![("200 OK", "", r#"{"email":"me@example.com","name":"Me"}"#.to_owned())]);
        assert_eq!(test(&to(Kind::Immich { server }), Some("KEY")), Ok("Connected as me@example.com".into()));
        assert_eq!(requests.join().unwrap()[0].path, "/api/users/me");
        let (server, _) = serve_each(vec![("401 Unauthorized", "", String::new())]);
        assert_eq!(test(&to(Kind::Immich { server }), Some("nope")), Err("Immich didn't accept the API key".into()));
    }

    #[test]
    fn custom_uploaders_come_from_sharex_files_and_push() {
        let (server, requests) = serve_each(vec![("200 OK", "", r#"{"files":[{"url":"https://z.example.com/u/a.png"}]}"#.into())]);
        let sxcu = serde_json::json!({
            "Name": "Zipline", "RequestURL": format!("{server}/api/upload"), "Headers": {"Authorization": "tok"},
            "Body": "MultipartFormData", "FileFormName": "file", "URL": "{json:files[0].url}",
        });
        let (d, secret) = import_sxcu(&sxcu.to_string()).unwrap();
        assert_eq!((d.name.as_str(), secret.as_deref(), d.problems()), ("Zipline", Some("tok"), None));
        assert_eq!(d.kind.target(), format!("{server}/api/upload"));
        assert_eq!(push(&d, secret.as_deref(), &shot("custom")), Ok(Some("https://z.example.com/u/a.png".into())));
        assert_eq!(requests.join().unwrap()[0].header("Authorization"), Some("tok"));
        assert!(d.kind.secret_optional() && !Kind::immich().secret_optional());
    }

    #[test]
    fn links_need_a_public_address() {
        assert_eq!(link("", "a.png"), None);
        assert_eq!(link(" https://x.example.com/s/ ", "dir/a #1.png").as_deref(), Some("https://x.example.com/s/dir/a%20%231.png"));
    }

    fn filled(kind: Kind) -> Destination {
        let mut d = Destination::new(kind);
        d.kind = match d.kind {
            Kind::Sftp { port, .. } => Kind::Sftp { host: "nas.local".into(), port, user: "me".into(), folder: "/shots".into(), public_url: String::new() },
            Kind::WebDav { .. } => Kind::WebDav { url: "https://dav.example.com/shots".into(), user: "me".into(), public_url: "https://x.example.com".into() },
            Kind::S3 { .. } => Kind::S3 {
                endpoint: "https://s3.example.com".into(),
                region: "auto".into(),
                bucket: "shots".into(),
                prefix: "rsnip/".into(),
                access_key: "AKIA".into(),
                public_url: String::new(),
            },
            Kind::Discord { .. } => Kind::Discord {},
            Kind::Immich { .. } => Kind::Immich { server: "https://photos.example.com".into() },
            Kind::Slack { .. } => Kind::Slack { channel: "C0123ABCDEF".into() },
            Kind::Custom { .. } => Kind::Custom {
                url: "https://i.example.com/api/upload".into(),
                method: "POST".into(),
                file_field: "file".into(),
                fields: String::new(),
                headers: "Authorization: {secret}".into(),
                link: "{json:url}".into(),
            },
        };
        d
    }

    #[test]
    fn every_kind_survives_saving_and_loading() {
        let env = Env::sandbox(std::env::temp_dir());
        let mut all: Vec<Destination> = every_kind().into_iter().map(filled).collect();
        all[1].auto = true;
        all[1].auto_since = Some(1_700_000_000);
        save(&env, &all);
        assert_eq!(load(&env), all);
        let json = env.text_setting(options::DESTINATIONS).unwrap();
        assert!(json.contains(r#""kind":"WebDav""#) && json.contains(r#""kind":"Discord""#), "{json}");
    }

    #[test]
    fn a_missing_or_damaged_list_is_empty() {
        let env = Env::sandbox(std::env::temp_dir());
        assert!(load(&env).is_empty());
        env.set_text_setting(options::DESTINATIONS, "{not json");
        assert!(load(&env).is_empty());
        env.set_text_setting(options::DESTINATIONS, r#"[{"id":"a","name":"x","kind":"Nope","auto":false,"auto_since":null}]"#);
        assert!(load(&env).is_empty());
    }

    #[test]
    fn the_port_defaults_to_22() {
        let json = r#"{"id":"a","name":"x","kind":"Sftp","host":"h","user":"u","folder":"","auto":false,"auto_since":null}"#;
        let d: Destination = serde_json::from_str(json).unwrap();
        assert!(matches!(d.kind, Kind::Sftp { port: 22, .. }));
    }

    /// Against real servers, as ci/test-destinations.sh starts them: rclone's S3 and WebDAV at
    /// `RSNIP_TEST_S3` and `RSNIP_TEST_WEBDAV`, and OpenSSH's SFTP at the host `RSNIP_TEST_SFTP`.
    /// What each keeps shows here in the folder `RSNIP_TEST_<KIND>_DIR`.
    #[test]
    #[ignore = "needs the servers ci/test-destinations.sh starts"]
    fn real_servers() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} isn't set: run ci/test-destinations.sh"));
        let png = shot("real");
        // A recording big enough that it has to stream.
        let mp4 = png.with_file_name("Recording 2026-10-24 091500.mp4");
        std::fs::write(&mp4, (0..12_000_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>()).unwrap();
        // The server has `file`, whole, in `folder`.
        let check = |folder: &Path, file: &Path| {
            let there = folder.join(file.file_name().unwrap());
            assert!(std::fs::read(&there).is_ok_and(|bytes| bytes == std::fs::read(file).unwrap()), "{} isn't the same", there.display());
        };

        let s3 = var("RSNIP_TEST_S3");
        let bucket = Kind::S3 {
            endpoint: s3.clone(),
            region: "us-east-1".into(),
            bucket: "shots".into(),
            prefix: "rsnip/today".into(),
            access_key: "rsnip".into(),
            public_url: format!("{s3}/shots"),
        };
        let d = to(bucket);
        assert_eq!(test(&d, Some("rsnip-secret")), Ok("Connected to shots".into()));
        assert!(test(&d, Some("wrong")).unwrap_err().starts_with("Access denied"));
        for file in [&png, &mp4] {
            let link = push(&d, Some("rsnip-secret"), file).unwrap();
            let name = encode(&file.file_name().unwrap().to_string_lossy());
            assert_eq!(link, Some(format!("{s3}/shots/rsnip/today/{name}")), "a link, as it has a public address");
            check(&Path::new(&var("RSNIP_TEST_S3_DIR")).join("shots/rsnip/today"), file);
        }
        println!("S3: pushed");

        let d = to(Kind::WebDav { url: format!("{}/shots/today", var("RSNIP_TEST_WEBDAV")), user: "rsnip".into(), public_url: String::new() });
        assert!(test(&d, Some("rsnip-secret")).is_ok());
        assert_eq!(test(&d, Some("wrong")), Err("The server didn't accept the user name or password".into()));
        for file in [&png, &mp4] {
            assert_eq!(push(&d, Some("rsnip-secret"), file), Ok(None), "no link without a public address");
            check(&Path::new(&var("RSNIP_TEST_WEBDAV_DIR")).join("shots/today"), file);
        }
        println!("WebDAV: pushed, making the folders");

        let d = to(Kind::Sftp { host: var("RSNIP_TEST_SFTP"), port: 22, user: "rsnip".into(), folder: "upload/shots/today".into(), public_url: String::new() });
        test(&d, None).unwrap();
        for file in [&png, &mp4] {
            assert_eq!(push(&d, None, file), Ok(None));
            check(&Path::new(&var("RSNIP_TEST_SFTP_DIR")).join("shots/today"), file);
        }
        let stranger = to(Kind::Sftp { host: var("RSNIP_TEST_SFTP"), port: 22, user: "nobody".into(), folder: String::new(), public_url: String::new() });
        assert!(test(&stranger, None).unwrap_err().contains("didn't accept your SSH keys"));
        println!("SFTP: pushed, making the folders");
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::BTreeSet<String> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn kinds_say_which_secret_they_need() {
        let labels: Vec<_> = every_kind().iter().map(|k| (k.name(), k.secret_label())).collect();
        assert_eq!(
            labels,
            [
                ("SFTP", None),
                ("WebDAV", Some("Password")),
                ("S3-compatible", Some("Secret access key")),
                ("Discord", Some("Webhook URL")),
                ("Slack", Some("Bot token")),
                ("Immich", Some("API key")),
                ("Custom uploader", Some("Token")),
            ]
        );
    }

    #[test]
    fn empty_ones_have_problems_and_filled_ones_do_not() {
        for kind in every_kind() {
            let d = Destination::new(kind.clone());
            let complete = matches!(kind, Kind::Discord { .. });
            assert_eq!(d.problems().is_none(), complete, "{}", kind.name());
            assert_eq!(filled(kind.clone()).problems(), None, "{}", kind.name());
        }
    }

    #[test]
    fn problems_name_what_to_fix() {
        let mut d = filled(Kind::sftp());
        d.name = "  ".into();
        assert_eq!(d.problems().as_deref(), Some("Give it a name"));
        d.name = "NAS".into();
        d.kind = Kind::Sftp { host: "".into(), port: 22, user: "me".into(), folder: "".into(), public_url: "".into() };
        assert_eq!(d.problems().as_deref(), Some("Enter the server's address"));
        d.kind = Kind::Sftp { host: "h".into(), port: 0, user: "me".into(), folder: "".into(), public_url: "".into() };
        assert_eq!(d.problems().as_deref(), Some("Enter a port from 1 to 65535"));
        d.kind = Kind::Sftp { host: "h".into(), port: 65535, user: "me".into(), folder: "".into(), public_url: "ftp://x".into() };
        assert_eq!(d.problems().as_deref(), Some("The link to share must start with http:// or https://"));

        let mut d = filled(Kind::webdav());
        d.kind = Kind::WebDav { url: "dav.example.com".into(), user: "me".into(), public_url: "".into() };
        assert_eq!(d.problems().as_deref(), Some("The folder's address must start with http:// or https://"));
        d.kind = Kind::WebDav { url: "http://".into(), user: "me".into(), public_url: "".into() };
        assert!(d.problems().is_some(), "a scheme alone isn't an address");
        d.kind = Kind::WebDav { url: "HTTP://dav.example.com".into(), user: "me".into(), public_url: "".into() };
        assert_eq!(d.problems(), None);

        let mut d = filled(Kind::immich());
        d.kind = Kind::Immich { server: "".into() };
        assert_eq!(d.problems().as_deref(), Some("Enter the server's address"));

        let mut d = filled(Kind::s3());
        d.kind = Kind::S3 { endpoint: "https://e".into(), region: "".into(), bucket: "b".into(), prefix: "".into(), access_key: "k".into(), public_url: "".into() };
        assert_eq!(d.problems(), None, "the region comes from the endpoint");
        d.kind = Kind::S3 { endpoint: "https://e".into(), region: "".into(), bucket: "".into(), prefix: "".into(), access_key: "k".into(), public_url: "".into() };
        assert_eq!(d.problems().as_deref(), Some("Enter the bucket's name"));
    }

    #[test]
    fn targets_read_well() {
        assert_eq!(filled(Kind::sftp()).kind.target(), "me@nas.local:/shots");
        assert_eq!(filled(Kind::s3()).kind.target(), "shots/rsnip on https://s3.example.com");
        assert_eq!(filled(Kind::discord()).kind.target(), "");
    }
}
