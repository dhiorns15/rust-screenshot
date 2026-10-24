//! Places RSnip sends screenshots and recordings to ("destinations"), each set to push new items
//! by itself or only when asked. This holds what a destination is, how the list is kept, and what
//! counts as a complete one. The secrets (passwords, keys, webhook URLs) are not here: they live in
//! the system's credential store (see `secrets`), by the destination's id.

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
}

fn default_port() -> u16 {
    22
}

impl Kind {
    /// An empty one of each kind, in the order they're offered.
    pub const ALL: [fn() -> Kind; 5] = [Kind::sftp, Kind::webdav, Kind::s3, Kind::discord, Kind::immich];

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

    pub fn name(&self) -> &'static str {
        match self {
            Kind::Sftp { .. } => "SFTP",
            Kind::WebDav { .. } => "WebDAV",
            Kind::S3 { .. } => "S3-compatible",
            Kind::Discord { .. } => "Discord",
            Kind::Immich { .. } => "Immich",
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
        }
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
            Kind::Sftp { host, port, user, public_url, .. } => required(host, "the server's address")
                .or_else(|| (*port == 0).then(|| "Enter a port from 1 to 65535".to_owned()))
                .or_else(|| required(user, "the user name"))
                .or_else(|| public(public_url)),
            Kind::WebDav { url, user, public_url } => {
                address(url, "the folder's address").or_else(|| required(user, "the user name")).or_else(|| public(public_url))
            }
            Kind::S3 { endpoint, region, bucket, access_key, public_url, .. } => address(endpoint, "the endpoint")
                .or_else(|| required(region, "the region"))
                .or_else(|| required(bucket, "the bucket's name"))
                .or_else(|| required(access_key, "the access key"))
                .or_else(|| public(public_url)),
            Kind::Discord { .. } => None,
            Kind::Immich { server } => address(server, "the server's address"),
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
/// and says what it found.
pub fn test(destination: &Destination, secret: Option<&str>) -> Result<String, String> {
    // Steps 4 and 5 replace this with a real check per kind.
    let _ = secret;
    Err(format!("Testing {} comes in a later step", destination.kind.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<Kind> {
        Kind::ALL.iter().map(|make| make()).collect()
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
                ("Immich", Some("API key")),
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
        assert_eq!(d.problems().as_deref(), Some("Enter the region"));
    }

    #[test]
    fn targets_read_well() {
        assert_eq!(filled(Kind::sftp()).kind.target(), "me@nas.local:/shots");
        assert_eq!(filled(Kind::s3()).kind.target(), "shots/rsnip on https://s3.example.com");
        assert_eq!(filled(Kind::discord()).kind.target(), "");
    }
}
