//! Passwords and keys for the places RSnip sends screenshots to, kept in the system's credential
//! store (Credential Manager on Windows, the Secret Service on Linux: GNOME Keyring, KWallet, ...)
//! so they never reach `settings.json` or the registry. Each belongs to a destination, by its id.
//! Nothing here prints or logs a secret, or puts one in an error.

/// The secret saved for the destination `id`, if there is one. On Linux the keyring may ask the
/// user to unlock it first, for up to a minute.
pub fn get(id: &str) -> Result<Option<String>, String> {
    imp::get(id, true)
}

/// The secret saved for the destination `id`, as `get` gives it, but without asking the user to
/// unlock the keyring: an error if it's locked. For what runs by itself, on a timer, where a
/// window asking for the password out of nowhere (again and again) would be no good.
pub fn get_quietly(id: &str) -> Result<Option<String>, String> {
    imp::get(id, false)
}

/// Saves `secret` for the destination `id` (replacing any), named `label` where the user can see it.
pub fn set(id: &str, label: &str, secret: &str) -> Result<(), String> {
    imp::set(id, label, secret)
}

/// Forgets the secret for the destination `id`; fine if there isn't one.
pub fn delete(id: &str) -> Result<(), String> {
    imp::delete(id)
}

/// `secret` in pieces of at most `size` bytes (at least 4), split between characters; one piece,
/// maybe empty, if it fits.
#[cfg(any(windows, test))]
fn split(secret: &str, size: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = secret;
    while rest.len() > size {
        let at = (1..=size).rev().find(|&i| rest.is_char_boundary(i)).unwrap_or(size);
        let (part, after) = rest.split_at(at);
        parts.push(part);
        rest = after;
    }
    parts.push(rest);
    parts
}

#[cfg(windows)]
mod imp {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    use crate::util::wide;

    /// The most a credential can hold (CRED_MAX_CREDENTIAL_BLOB_SIZE), and its comment (characters).
    const MAX_SECRET: usize = 5 * 512;
    const MAX_COMMENT: usize = 255;
    /// The most credentials one secret is kept in, for one longer than a credential holds (as a
    /// Microsoft refresh token can be).
    const MAX_PARTS: usize = 8;

    /// Where the `n`th part of destination `id`'s secret is kept: the first where a whole one always
    /// was, the others beside it.
    fn target(id: &str, n: usize) -> Vec<u16> {
        wide(&if n == 0 { format!("RSnip/destination/{id}") } else { format!("RSnip/destination/{id}/part{}", n + 1) })
    }

    fn failed(what: &str) -> String {
        format!("Couldn't {what} in Windows' Credential Manager (error {})", unsafe { GetLastError() })
    }

    /// A credential read from the store, freed when dropped.
    struct Credential(*mut CREDENTIALW);

    impl Drop for Credential {
        fn drop(&mut self) {
            unsafe { CredFree(self.0 as *const _) };
        }
    }

    /// Credential Manager never asks anything, so `_ask` makes no difference.
    pub fn get(id: &str, _ask: bool) -> Result<Option<String>, String> {
        let Some(mut bytes) = read(&target(id, 0))? else { return Ok(None) };
        for n in 1..MAX_PARTS {
            match read(&target(id, n))? {
                Some(part) => bytes.extend(part),
                None => break,
            }
        }
        String::from_utf8(bytes).map(Some).map_err(|_| "The saved password isn't text".to_owned())
    }

    /// What the credential `name` holds, if there is one.
    fn read(name: &[u16]) -> Result<Option<Vec<u8>>, String> {
        let mut found: *mut CREDENTIALW = null_mut();
        if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut found) } == 0 {
            return if unsafe { GetLastError() } == ERROR_NOT_FOUND { Ok(None) } else { Err(failed("read the saved password")) };
        }
        let credential = Credential(found);
        let (blob, size) = unsafe { ((*credential.0).CredentialBlob, (*credential.0).CredentialBlobSize as usize) };
        let bytes: &[u8] = if blob.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(blob, size) } };
        Ok(Some(bytes.to_vec()))
    }

    /// Saves `secret` in as many credentials as it needs, and removes the parts a longer one before
    /// it left.
    pub fn set(id: &str, label: &str, secret: &str) -> Result<(), String> {
        let parts = super::split(secret, MAX_SECRET);
        if parts.len() > MAX_PARTS {
            return Err(format!("That's too long to save in Windows' Credential Manager (the limit is {} bytes)", MAX_SECRET * MAX_PARTS));
        }
        for (n, part) in parts.iter().enumerate() {
            write(&target(id, n), label, part)?;
        }
        (parts.len()..MAX_PARTS).try_for_each(|n| remove(&target(id, n)))
    }

    /// Saves `secret` as the credential `name`, replacing any.
    fn write(name: &[u16], label: &str, secret: &str) -> Result<(), String> {
        let comment = wide(&label.chars().take(MAX_COMMENT).collect::<String>());
        let user = wide("RSnip");
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_ptr() as *mut u16,
            Comment: comment.as_ptr() as *mut u16,
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: null_mut(),
            TargetAlias: null_mut(),
            UserName: user.as_ptr() as *mut u16,
        };
        // CredWriteW only reads the credential, and replaces one with the same name.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(failed("save the password"));
        }
        Ok(())
    }

    pub fn delete(id: &str) -> Result<(), String> {
        (0..MAX_PARTS).try_for_each(|n| remove(&target(id, n)))
    }

    /// Removes the credential `name`; fine if there isn't one.
    fn remove(name: &[u16]) -> Result<(), String> {
        if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 && unsafe { GetLastError() } != ERROR_NOT_FOUND {
            return Err(failed("remove the saved password"));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::collections::HashMap;
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::Duration;

    use zbus::blocking::{connection, Connection, Proxy};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

    const SERVICE: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
    const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
    const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
    const NO_KEYRING: &str = "No keyring is running. Install GNOME Keyring or KWallet (any Secret Service) to save passwords.";
    /// How long the keyring has to answer a call, and the user its window asking to unlock it
    /// (which may never show, say over remote desktop).
    const WAIT: Duration = Duration::from_secs(60);
    const LOCKED: &str = "The keyring is locked. RSnip waits for it to be unlocked (Settings asks for that when it needs a password).";
    const NOT_UNLOCKED: &str = "The keyring is locked, and wasn't unlocked within a minute of asking.";

    /// A secret as the Secret Service takes and gives it: session, parameters, value, content type.
    type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    /// A session with the Secret Service (it carries secrets unencrypted, over the session bus, as
    /// the "plain" algorithm does), closed when dropped.
    struct Keyring {
        conn: Connection,
        session: OwnedObjectPath,
    }

    /// What went wrong talking to the Secret Service, in words: that there is none, or what it said.
    fn failed(e: zbus::Error) -> String {
        let name = match &e {
            zbus::Error::MethodError(name, ..) => name.to_string(),
            zbus::Error::FDO(fdo) => format!("{fdo:?}"),
            _ => String::new(),
        };
        if ["ServiceUnknown", "NameHasNoOwner", "NotSupported", "Spawn"].iter().any(|s| name.contains(s)) {
            NO_KEYRING.to_owned()
        } else if matches!(&e, zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut) {
            "The keyring didn't answer within a minute".to_owned()
        } else {
            format!("The keyring wouldn't save or read the password ({e})")
        }
    }

    impl Keyring {
        fn open() -> Result<Keyring, String> {
            let conn = connection::Builder::session()
                .and_then(|builder| builder.method_timeout(WAIT).build())
                .map_err(|e| format!("Couldn't reach the desktop's session bus: {e}"))?;
            let service = Proxy::new(&conn, SERVICE, SERVICE_PATH, SERVICE_IFACE).map_err(failed)?;
            let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).map_err(failed)?;
            Ok(Keyring { conn, session })
        }

        fn service(&self) -> Result<Proxy<'_>, String> {
            Proxy::new(&self.conn, SERVICE, SERVICE_PATH, SERVICE_IFACE).map_err(failed)
        }

        fn proxy<'a>(&'a self, path: &'a OwnedObjectPath, interface: &'a str) -> Result<Proxy<'a>, String> {
            Proxy::new(&self.conn, SERVICE, path.as_str(), interface).map_err(failed)
        }

        /// The collection secrets are kept in: the user's default one ("login"), which is never
        /// created here.
        fn collection(&self) -> Result<OwnedObjectPath, String> {
            let alias: OwnedObjectPath = self.service()?.call("ReadAlias", &("default",)).map_err(failed)?;
            if alias.as_str() != "/" {
                return Ok(alias);
            }
            let fallback = OwnedObjectPath::try_from(DEFAULT_COLLECTION).map_err(|e| e.to_string())?;
            let exists = self.proxy(&fallback, COLLECTION_IFACE)?.get_property::<bool>("Locked").is_ok();
            if exists { Ok(fallback) } else { Err("The keyring has no default collection to save passwords in.".to_owned()) }
        }

        /// Waits for the user to answer what `prompt` asks (in the keyring's own window), if it is
        /// one, for up to a minute; then it's dismissed.
        fn prompt(&self, prompt: &OwnedObjectPath) -> Result<(), String> {
            if prompt.as_str() == "/" {
                return Ok(());
            }
            let proxy = self.proxy(prompt, "org.freedesktop.Secret.Prompt")?;
            // Listen before calling, so the answer can't be missed.
            let mut completed = proxy.receive_signal("Completed").map_err(failed)?;
            proxy.call_method("Prompt", &("",)).map_err(failed)?;
            // The wait has no end of its own, so it's on a thread that's waited for only so long.
            // Dismissing the prompt answers it, which ends that thread.
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(completed.next());
            });
            let message = match rx.recv_timeout(WAIT) {
                Ok(Some(message)) => message,
                Ok(None) | Err(RecvTimeoutError::Disconnected) => return Err("The keyring stopped answering".to_owned()),
                Err(RecvTimeoutError::Timeout) => {
                    let _ = proxy.call_method("Dismiss", &());
                    return Err(NOT_UNLOCKED.to_owned());
                }
            };
            let (dismissed, _): (bool, OwnedValue) = message.body().deserialize().map_err(|e| e.to_string())?;
            if dismissed { Err("Cancelled".to_owned()) } else { Ok(()) }
        }

        /// Unlocks `paths`, asking the user if need be; unless `ask` is false, when locked is an
        /// error.
        fn unlock(&self, paths: Vec<OwnedObjectPath>, ask: bool) -> Result<Vec<OwnedObjectPath>, String> {
            if paths.is_empty() {
                return Ok(paths);
            }
            if !ask {
                return Err(LOCKED.to_owned());
            }
            let (mut unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self.service()?.call("Unlock", &(&paths,)).map_err(failed)?;
            if prompt.as_str() != "/" {
                self.prompt(&prompt)?;
                // The prompt's result lists what it unlocked, but it was asked for all of them.
                unlocked = paths;
            }
            Ok(unlocked)
        }

        /// The items for the destination `id`, unlocked: by asking the user if need be when `ask`,
        /// else only those unlocked already (an error if all of them are locked).
        fn find(&self, id: &str, ask: bool) -> Result<Vec<OwnedObjectPath>, String> {
            let attributes = HashMap::from([("application", "rsnip"), ("destination", id)]);
            let (mut items, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self.service()?.call("SearchItems", &(attributes,)).map_err(failed)?;
            // Without asking, an unlocked one will do.
            if ask || items.is_empty() {
                items.extend(self.unlock(locked, ask)?);
            }
            Ok(items)
        }
    }

    impl Drop for Keyring {
        fn drop(&mut self) {
            if let Ok(session) = Proxy::new(&self.conn, SERVICE, self.session.as_str(), "org.freedesktop.Secret.Session") {
                let _ = session.call_method("Close", &());
            }
        }
    }

    pub fn get(id: &str, ask: bool) -> Result<Option<String>, String> {
        let keyring = Keyring::open()?;
        let items = keyring.find(id, ask)?;
        if items.is_empty() {
            return Ok(None);
        }
        let secrets: HashMap<OwnedObjectPath, Secret> =
            keyring.service()?.call("GetSecrets", &(&items, ObjectPath::from(&keyring.session))).map_err(failed)?;
        let Some((_, _, value, _)) = items.iter().find_map(|item| secrets.get(item)) else {
            return Ok(None);
        };
        String::from_utf8(value.clone()).map(Some).map_err(|_| "The saved password isn't text".to_owned())
    }

    pub fn set(id: &str, label: &str, secret: &str) -> Result<(), String> {
        let keyring = Keyring::open()?;
        let collection = keyring.collection()?;
        keyring.unlock(vec![collection.clone()], true)?;
        let attributes = HashMap::from([("application".to_owned(), "rsnip".to_owned()), ("destination".to_owned(), id.to_owned())]);
        let properties: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(format!("RSnip: {label}"))),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes)),
        ]);
        let secret: Secret = (keyring.session.clone(), Vec::new(), secret.as_bytes().to_vec(), "text/plain; charset=utf8".to_owned());
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) =
            keyring.proxy(&collection, COLLECTION_IFACE)?.call("CreateItem", &(properties, secret, true)).map_err(failed)?;
        keyring.prompt(&prompt)
    }

    pub fn delete(id: &str) -> Result<(), String> {
        let keyring = Keyring::open()?;
        for item in keyring.find(id, true)? {
            let prompt: OwnedObjectPath = keyring.proxy(&item, "org.freedesktop.Secret.Item")?.call("Delete", &()).map_err(failed)?;
            keyring.prompt(&prompt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real credential store (Credential Manager; on Linux a Secret Service with an
    /// unlocked keyring), so `cargo test` leaves it out: `cargo test secrets -- --ignored`.
    #[test]
    #[ignore = "uses the real credential store"]
    fn keeps_a_secret_in_the_credential_store() {
        let id = format!("test-{}", std::process::id());
        let _ = delete(&id);
        assert_eq!(get(&id), Ok(None), "nothing yet");
        set(&id, "A test", "first, with ünicode").unwrap();
        assert_eq!(get(&id), Ok(Some("first, with ünicode".to_owned())));
        set(&id, "A test", "second").unwrap();
        assert_eq!(get(&id), Ok(Some("second".to_owned())), "replaced, not added");
        assert_eq!(get(&format!("{id}-other")), Ok(None), "other destinations are separate");
        // Longer than one credential holds on Windows, then shorter again.
        let long = format!("{}ü{}", "a".repeat(2559), "b".repeat(5000));
        set(&id, "A test", &long).unwrap();
        assert_eq!(get(&id), Ok(Some(long)), "kept whole");
        set(&id, "A test", "short").unwrap();
        assert_eq!(get(&id), Ok(Some("short".to_owned())), "with none of the longer one's parts");
        delete(&id).unwrap();
        assert_eq!(get(&id), Ok(None), "gone");
        delete(&id).unwrap();
    }

    #[test]
    fn long_secrets_are_split_between_characters() {
        assert_eq!(split("", 4), [""]);
        assert_eq!(split("abcd", 4), ["abcd"]);
        assert_eq!(split("abcdefghi", 4), ["abcd", "efgh", "i"]);
        // ü is two bytes, so it goes whole into the next piece.
        assert_eq!(split("abcüdef", 4), ["abc", "üde", "f"]);
        let long = "ñ".repeat(3000);
        let parts = split(&long, 2560);
        assert!(parts.iter().all(|p| p.len() <= 2560));
        assert_eq!(parts.concat(), long);
    }
}
