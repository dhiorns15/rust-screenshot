//! Settings → Push to: the places screenshots and recordings are sent to, and the form for adding
//! or changing one. Testing a destination and saving or forgetting its secret happen off the UI
//! thread (the keyring can wait for the user to unlock it); each answer arrives on a channel that
//! is polled every frame, like the folder chooser's.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align, Color32, Layout, RichText, Ui};

use super::{row, SettingsApp};
use crate::gallery::env::{PickedFolder, PushEvent, TrayRequest};
use crate::gallery::theme::{self, icon};
use crate::push::history::{self, Entry};
use crate::push::{self, CodeEvent, Destination, Kind, Pkce, SignIn as SignInWay};

type Answer<T> = Receiver<Result<T, String>>;

#[derive(Default)]
pub struct PushState {
    /// The form for a new or changed destination, while it's open.
    form: Option<Form>,
    /// The latest test of each destination, by id.
    tests: BTreeMap<String, Test>,
    /// The destination being asked "Remove it?", by id.
    removing: Option<String>,
    /// Removed destinations' secrets being forgotten.
    forgetting: Vec<Answer<()>>,
    /// A ShareX uploader file being chosen, to import.
    importing: Option<PickedFolder>,
    /// The push history, and when it was read.
    history: Option<(Instant, Vec<Entry>)>,
    /// A failed push being tried again, by hand.
    retrying: Option<Retrying>,
}

struct Retrying {
    /// The destination's id and name, and the file's name.
    id: String,
    to: String,
    file: String,
    events: Receiver<PushEvent>,
    /// Whether it went, and the link to it there if it has one.
    pushed: Option<Option<String>>,
}

enum Test {
    Running(Answer<String>),
    Done(Result<String, String>),
}

struct Form {
    /// Changing a saved destination (rather than adding one), whose secret may be kept.
    editing: bool,
    destination: Destination,
    /// The port as typed.
    port: String,
    /// The secret as typed.
    secret: String,
    /// Saving the secret, before the destination is saved.
    saving: Option<Answer<()>>,
    /// Why it couldn't be saved.
    error: Option<String>,
    /// An FTPS server's certificate being fetched, then its fingerprint, for the user to trust or not.
    fetching: Option<Answer<String>>,
    offered: Option<String>,
    /// Signing in, for the kinds that do.
    sign_in: SignIn,
}

/// Signing in, for a kind that does (see `Kind::sign_in`). The refresh token that comes of it is
/// kept as the form's `secret`, to be saved with the rest.
#[derive(Default)]
struct SignIn {
    /// Signed in just now, for this app key (Dropbox's; empty for the others).
    done: Option<String>,
    /// When changing a destination, what its saved sign-in is for, as `done`.
    saved_for: Option<String>,
    /// Pasting a code back (Dropbox's), once started.
    pasted: Option<PastedCode>,
    /// Showing a code to enter on the service's page (the others'), once started.
    shown: Option<ShownCode>,
    /// Signed in just now as whom, if the service said.
    who: Option<String>,
}

impl SignIn {
    /// What it's signed in for, just now or before, if it is.
    fn signed_in_for(&self) -> Option<&str> {
        self.done.as_deref().or(self.saved_for.as_deref())
    }
}

/// Signing in by pasting a code back: the browser is opened on the service's page, which shows the
/// code once RSnip is allowed.
struct PastedCode {
    /// What the code is traded in with.
    pkce: Pkce,
    /// The page that was opened, shown to copy where no browser opens (over xrdp, say).
    url: String,
    /// Whether the browser opened, until it's known.
    opening: Option<Answer<()>>,
    /// Why the browser didn't open, when it's known it didn't.
    unopened: Option<String>,
    /// The link was copied.
    copied: bool,
    /// The app key it was started with.
    app_key: String,
    /// The code as pasted.
    code: String,
    exchanging: Option<Answer<String>>,
}

/// Signing in with a code shown here, which the user enters on the service's page, on this
/// computer or any other (a phone, say).
struct ShownCode {
    events: Receiver<CodeEvent>,
    /// Set to stop waiting for the user.
    cancel: Arc<AtomicBool>,
    /// The code, and the page to enter it on, once the service has given them.
    code: Option<(String, String)>,
    /// What was copied last: "link" or "code".
    copied: Option<&'static str>,
}

impl Drop for ShownCode {
    /// No more waiting once it's left behind: cancelled, signed in, or the form closed.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What was clicked in the sign-in rows of the form.
#[derive(Default)]
struct SignInClicks {
    sign_in: bool,
    finish: bool,
    copy: bool,
    copy_code: bool,
    cancel: bool,
}

impl Form {
    fn new(destination: Destination, editing: bool) -> Form {
        let mut kind = destination.kind.clone();
        let port = kind.port_mut().map(|p| p.to_string()).unwrap_or_default();
        let saved_for = (editing && kind.sign_in().is_some()).then(|| app_key(&kind).to_owned());
        let sign_in = SignIn { saved_for, ..SignIn::default() };
        Form { editing, destination, port, secret: String::new(), saving: None, error: None, fetching: None, offered: None, sign_in }
    }

    /// What must be fixed before it can be saved.
    fn problem(&self) -> Option<String> {
        self.destination.problems().or_else(|| self.sign_in_problem()).or_else(|| {
            let label = self.destination.kind.secret_label()?;
            let needed = !self.editing && !self.destination.kind.secret_optional();
            (needed && self.secret.is_empty()).then(|| format!("Enter the {}", lowercase_first(label)))
        })
    }

    /// A kind that signs in needs to be signed in to (Dropbox with the app key that's in the form).
    fn sign_in_problem(&self) -> Option<String> {
        let kind = &self.destination.kind;
        kind.sign_in()?;
        let name = kind.name();
        match self.sign_in.signed_in_for() {
            None => Some(format!("Sign in to {name} first")),
            Some(key) if key.trim() != app_key(kind).trim() => Some(format!("The app key has changed: sign in to {name} again")),
            Some(_) => None,
        }
    }
}

/// The app a sign-in is for: Dropbox's key, of the user's own app; empty for the others, which
/// sign in to RSnip's.
fn app_key(kind: &Kind) -> &str {
    match kind {
        Kind::Dropbox { app_key, .. } => app_key,
        _ => "",
    }
}

/// What a click in the list asks for.
enum Action {
    Auto(String, bool),
    Test(Destination),
    Edit(Destination),
    AskRemove(String),
    /// Open this kind's section of docs/destinations.md.
    Help(Kind),
    Remove(String),
    CancelRemove,
}

/// One text box in the form.
struct Field<'a> {
    label: &'static str,
    hint: &'static str,
    value: &'a mut String,
    /// A few lines high, for one entry a line.
    multiline: bool,
}

fn fields(kind: &mut Kind) -> Vec<Field<'_>> {
    let field = |label, hint, value| Field { label, hint, value, multiline: false };
    let lines = |label, hint, value| Field { label, hint, value, multiline: true };
    match kind {
        Kind::Sftp { host, user, folder, public_url, .. } => vec![
            field("Server", "nas.example.com", host),
            field("User", "", user),
            field("Folder", "Empty for your home folder on the server", folder),
            field("Link to share", "Optional: where the pushed files can be opened, e.g. https://img.example.com/", public_url),
        ],
        Kind::WebDav { url, user, public_url } => vec![
            field("Folder address", "https://cloud.example.com/remote.php/dav/files/me/Screenshots/", url),
            field("User", "", user),
            field("Link to share", "Optional: where the pushed files can be opened, e.g. https://img.example.com/", public_url),
        ],
        Kind::S3 { endpoint, region, bucket, prefix, access_key, public_url } => vec![
            field("Endpoint", "https://s3.us-east-1.amazonaws.com", endpoint),
            field("Region", "Optional: taken from the endpoint", region),
            field("Bucket", "", bucket),
            field("Folder in the bucket", "Optional", prefix),
            field("Access key", "", access_key),
            field("Link to share", "Optional: where the pushed files can be opened, e.g. https://img.example.com/", public_url),
        ],
        Kind::Discord {} => vec![],
        Kind::Immich { server } => vec![field("Server address", "https://photos.example.com", server)],
        Kind::Slack { channel } => vec![field("Channel ID", "C0123ABCDEF, from the channel's details", channel)],
        Kind::PhotoPrism { server, album } => vec![
            field("Server address", "https://photos.example.com", server),
            field("Album", "Optional: made if it isn't there", album),
        ],
        Kind::Ftps { host, user, folder, public_url, .. } => vec![
            field("Server", "nas.example.com", host),
            field("User", "", user),
            field("Folder", "Empty for the folder the server starts in", folder),
            field("Link to share", "Optional: where the pushed files can be opened, e.g. https://img.example.com/", public_url),
        ],
        Kind::Smb { share, folder, user, public_url } => vec![
            field("Share", "\\\\nas\\shots", share),
            field("Folder", "Optional: in the share", folder),
            field("User", "Empty to sign in as you are (a guest on Linux)", user),
            field("Link to share", "Optional: where the pushed files can be opened, e.g. https://img.example.com/", public_url),
        ],
        Kind::Dropbox { app_key, folder, .. } => vec![
            field("App key", "From the Settings tab of your app at dropbox.com/developers", app_key),
            field("Folder", "/Screenshots, in the app's folder (or Dropbox, with full access)", folder),
        ],
        Kind::GoogleDrive { folder, .. } => vec![field("Folder", "Screenshots, in My Drive (made if it isn't there); empty for the top", folder)],
        Kind::OneDrive { folder, .. } => vec![field("Folder", "Screenshots, in Apps/RSnip (made if it isn't there); empty for Apps/RSnip itself", folder)],
        Kind::Custom { url, method, file_field, fields, headers, link } => vec![
            field("Upload address", "https://i.example.com/api/upload", url),
            field("Method", "POST", method),
            field("File field", "Empty to send the file as the whole body", file_field),
            lines("Form fields", "name: value, one a line", fields),
            lines("Headers", "Authorization: Bearer {secret}", headers),
            field("Link", "Where the reply has it, e.g. {json:data.url} or {response}", link),
        ],
    }
}

fn help(kind: &Kind) -> &'static str {
    match kind {
        Kind::Sftp { .. } => "Uses your SSH keys and settings (ssh-agent, ~/.ssh/config); RSnip never stores an SSH password.",
        Kind::WebDav { .. } => "Works with Nextcloud, ownCloud and other WebDAV servers.",
        Kind::S3 { .. } => "Works with AWS, Cloudflare R2, Backblaze B2, Wasabi and MinIO.",
        Kind::Discord { .. } => "Server Settings > Integrations > Webhooks > Copy Webhook URL",
        Kind::Immich { .. } => "Account Settings > API Keys",
        Kind::Slack { .. } => "Your Slack app's Bot User OAuth Token (xoxb-…), with the files:write scope; invite the app to the channel. docs/destinations.md has an app manifest to start from.",
        Kind::PhotoPrism { .. } => "Settings > Account > Apps and Devices: an app password with full access, or the files and sessions scopes (and albums, for an album).",
        Kind::Ftps { .. } => "FTP over TLS: port 21 for explicit TLS, 990 for implicit. Plain FTP isn't offered, as it sends the password unencrypted.",
        Kind::Smb { .. } => "A Windows or Samba share, a NAS's say. On Linux it needs Samba's smbclient.",
        Kind::Dropbox { .. } => "Uses a free Dropbox app of your own, made as docs/destinations.md says: enter its App key, then sign in to Dropbox.",
        Kind::GoogleDrive { .. } => "Sign in with a code, on this computer or any other (a phone too). RSnip sees only the files and folders it puts in your Drive.",
        Kind::OneDrive { .. } => "Sign in with a code, on this computer or any other (a phone too). RSnip can reach only its own folder in your OneDrive, Apps/RSnip.",
        Kind::Custom { .. } => "Any upload API, like ShareX's custom uploaders. {secret} is replaced by the token, {filename} by the file's name.",
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Trims a pasted secret: a webhook address or key often comes with a stray space or newline, but
/// a password's spaces are its own.
fn tidy_secret(secret: &str, password: bool) -> String {
    if password { secret.trim_matches(['\r', '\n']).to_owned() } else { secret.trim().to_owned() }
}

fn error_color() -> Color32 {
    Color32::from_rgb(255, 120, 110)
}

impl SettingsApp {
    pub(super) fn destinations(&mut self, ui: &mut Ui) {
        self.poll_push();
        let list = push::load(&self.env);
        let muted = theme::palette(ui).muted;
        ui.label(RichText::new("Send screenshots and recordings to other places, as they're saved or when you choose.").small().color(muted));
        if list.is_empty() && self.push.form.is_none() {
            theme::card(ui, |ui| {
                row(ui, "Nowhere yet. Add a place to push to.", "", |ui| {
                    if let Some(added) = add_menu(ui) {
                        self.add(ui.ctx(), added);
                    }
                });
            });
        }
        let mut action = None;
        for d in &list {
            let editing_this = self.push.form.as_ref().is_some_and(|f| f.editing && f.destination.id == d.id);
            if editing_this {
                self.form_card(ui);
            } else {
                action = self.destination_card(ui, d).or(action);
            }
        }
        if self.push.form.as_ref().is_some_and(|f| !f.editing) {
            self.form_card(ui);
        } else if self.push.form.is_none() && !list.is_empty() {
            if let Some(added) = add_menu(ui) {
                self.add(ui.ctx(), added);
            }
        }
        if let Some(action) = action {
            self.apply(ui, list, action);
        }
        self.recent(ui);
    }

    /// The latest pushes, by hand and automatic, read again every few seconds while shown: the
    /// automatic pusher may be in another process.
    fn recent(&mut self, ui: &mut Ui) {
        if self.push.history.as_ref().is_none_or(|(read, _)| read.elapsed() > Duration::from_secs(2)) {
            self.push.history = Some((Instant::now(), history::load(&self.env.history_file)));
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let Some((_, entries)) = &self.push.history else { return };
        if entries.is_empty() {
            return;
        }
        let (muted, now) = (theme::palette(ui).muted, history::now());
        let destinations = if entries.iter().any(|e| e.error.is_some()) { push::load(&self.env) } else { Vec::new() };
        let mut retry = None;
        ui.add_space(6.0);
        ui.label(RichText::new("Recent pushes").strong());
        theme::card(ui, |ui| {
            for (i, e) in entries.iter().take(10).enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let how = if e.auto { "automatically" } else { "by hand" };
                let (text, color) = match &e.error {
                    None => (format!("Pushed {how}, {}", history::ago(e.at, now)), muted),
                    Some(error) => (format!("Failed {how}, {}: {error}", history::ago(e.at, now)), error_color()),
                };
                // Only the newest push of a file to a place is worth trying again.
                let retryable = e.error.is_some() && !entries[..i].iter().any(|newer| newer.id == e.id && newer.file == e.file);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if retryable {
                            let destination = destinations.iter().find(|d| d.id == e.id);
                            let file = self.env.dir.join(&e.file);
                            let this = self.push.retrying.as_ref().is_some_and(|r| r.id == e.id && r.file == e.file);
                            let (enabled, why) = match destination {
                                None => (false, "This place was removed from Push to"),
                                Some(_) if !file.is_file() => (false, "The file isn't in RSnip's folder any more"),
                                Some(_) => (self.push.retrying.is_none(), "Still pushing"),
                            };
                            let button = ui.add_enabled(enabled, egui::Button::new(if this { "Retrying…" } else { "Retry" }));
                            if button.on_disabled_hover_text(why).clicked() {
                                retry = destination.map(|d| (d.clone(), file));
                            }
                        }
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.label(format!("{} to {}", e.file, e.name));
                            ui.add(egui::Label::new(RichText::new(text).small().color(color)).wrap());
                        });
                    });
                });
            }
        });
        if let Some((destination, file)) = retry {
            let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let events = self.env.push_files(ui.ctx(), &destination, vec![file]);
            self.push.retrying = Some(Retrying { id: destination.id, to: destination.name, file: name, events, pushed: None });
        }
    }

    /// Opens the form for a new destination, or asks for the ShareX file to make it from.
    fn add(&mut self, ctx: &egui::Context, added: Added) {
        match added {
            Added::Kind(kind) => self.push.form = Some(Form::new(Destination::new(kind), false)),
            Added::Sxcu => self.push.importing = Some(self.env.pick_file(ctx, "Choose a ShareX custom uploader", ("ShareX custom uploader", "*.sxcu"))),
        }
    }

    /// A ShareX uploader file, once chosen: the form, filled in from it.
    fn import(&mut self) {
        let Some(Ok(picked)) = self.push.importing.as_ref().map(|rx| rx.try_recv()) else { return };
        self.push.importing = None;
        let file = match picked {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(e) => return self.note = Some(format!("Couldn't open the file chooser: {e}")),
        };
        let imported = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| push::import_sxcu(&text));
        match imported {
            Ok((destination, secret)) => {
                let mut form = Form::new(destination, false);
                form.secret = secret.unwrap_or_default();
                self.push.form = Some(form);
            }
            Err(e) => {
                let name = file.file_name().unwrap_or_default().to_string_lossy();
                self.note = Some(format!("Couldn't use {name}: {e}"));
            }
        }
    }

    /// Picks up the answers that have arrived.
    fn poll_push(&mut self) {
        self.import();
        for test in self.push.tests.values_mut() {
            if let Test::Running(rx) = test {
                if let Ok(result) = rx.try_recv() {
                    *test = Test::Done(result);
                }
            }
        }
        self.poll_retry();
        let mut failed = None;
        self.push.forgetting.retain(|rx| match rx.try_recv() {
            Ok(result) => {
                failed = failed.take().or(result.err());
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        if let Some(e) = failed {
            self.note = Some(format!("Removed, but couldn't delete its saved password or key: {e}"));
        }
        if let Some(form) = self.push.form.as_mut() {
            if let Some(Ok(fetched)) = form.fetching.as_ref().map(|rx| rx.try_recv()) {
                form.fetching = None;
                match fetched {
                    Ok(fingerprint) => form.offered = Some(fingerprint),
                    Err(e) => form.error = Some(format!("Couldn't get the server's certificate: {e}")),
                }
            }
        }
        if let Some(form) = self.push.form.as_mut() {
            poll_sign_in(form);
        }
        let saved = self.push.form.as_mut().and_then(|form| {
            let result = form.saving.as_ref()?.try_recv().ok()?;
            form.saving = None;
            match result {
                Ok(()) => Some(form.destination.clone()),
                Err(e) => {
                    form.error = Some(format!("Couldn't save the {}: {e}", lowercase_first(form.destination.kind.secret_label().unwrap_or("secret"))));
                    None
                }
            }
        });
        if let Some(destination) = saved {
            self.commit(destination);
        }
    }

    /// Takes in how the retry is going; when it's done, the new row is shown at once, and the link
    /// copied as for a push from the library.
    fn poll_retry(&mut self) {
        let Some(r) = &mut self.push.retrying else { return };
        let mut done = false;
        for event in r.events.try_iter() {
            match event {
                PushEvent::Pushed(_, link) => r.pushed = Some(link),
                PushEvent::Done => done = true,
                PushEvent::Pushing(_) | PushEvent::Failed(..) => {}
            }
        }
        if !done {
            return;
        }
        let Some(r) = self.push.retrying.take() else { return };
        self.push.history = None;
        match r.pushed {
            Some(Some(link)) => self.copy(&link, &format!("Pushed {} to {}, link copied", r.file, r.to)),
            Some(None) => self.note = Some(format!("Pushed {} to {}", r.file, r.to)),
            // The new row says what went wrong.
            None => {}
        }
    }

    /// Writes `destination` to the list, replacing the one with its id, and closes the form.
    fn commit(&mut self, destination: Destination) {
        let mut list = push::load(&self.env);
        match list.iter_mut().find(|d| d.id == destination.id) {
            // Whether it pushes automatically is changed in the list, not here; what pushes have
            // learned about the place since the form was opened is kept.
            Some(existing) => {
                let mut kind = destination.kind;
                kind.learn(&existing.kind);
                (existing.name, existing.kind) = (destination.name, kind);
            }
            None => list.push(destination),
        }
        self.save_destinations(&list);
        self.push.form = None;
    }

    fn save_destinations(&self, list: &[Destination]) {
        push::save(&self.env, list);
        self.env.tell_tray(TrayRequest::SettingsChanged);
    }

    fn apply(&mut self, ui: &Ui, mut list: Vec<Destination>, action: Action) {
        match action {
            Action::Auto(id, auto) => {
                if let Some(d) = list.iter_mut().find(|d| d.id == id) {
                    d.auto = auto;
                    d.auto_since = auto.then(now);
                }
                self.save_destinations(&list);
            }
            Action::Test(d) => {
                let rx = self.env.test_destination(ui.ctx(), &d, None);
                self.push.tests.insert(d.id, Test::Running(rx));
            }
            Action::Edit(d) => self.push.form = Some(Form::new(d, true)),
            Action::AskRemove(id) => self.push.removing = Some(id),
            Action::Help(kind) => self.env.open(std::path::Path::new(&doc_url(&kind))),
            Action::CancelRemove => self.push.removing = None,
            Action::Remove(id) => {
                list.retain(|d| d.id != id);
                self.save_destinations(&list);
                self.push.forgetting.push(self.env.forget_secret(ui.ctx(), &id));
                self.push.tests.remove(&id);
                self.push.removing = None;
            }
        }
    }

    fn destination_card(&mut self, ui: &mut Ui, d: &Destination) -> Option<Action> {
        let muted = theme::palette(ui).muted;
        let mut action = None;
        theme::card(ui, |ui| {
            if self.push.removing.as_deref() == Some(&d.id) {
                let detail = if d.kind.secret_label().is_some() { "Its saved password or key is deleted too" } else { "" };
                row(ui, &format!("Remove {}?", d.name), detail, |ui| {
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::CancelRemove);
                    }
                    if ui.add(theme::primary_button("Remove")).clicked() {
                        action = Some(Action::Remove(d.id.clone()));
                    }
                });
                return;
            }
            let target = d.kind.target();
            let detail = if target.is_empty() { d.kind.name().to_owned() } else { format!("{} · {target}", d.kind.name()) };
            let testing = matches!(self.push.tests.get(&d.id), Some(Test::Running(_)));
            row(ui, &d.name, &detail, |ui| {
                if ui.button("Remove").clicked() {
                    action = Some(Action::AskRemove(d.id.clone()));
                }
                if ui.button("Edit").clicked() {
                    action = Some(Action::Edit(d.clone()));
                }
                if ui.add_enabled(!testing, egui::Button::new(if testing { "Testing…" } else { "Test" })).clicked() {
                    action = Some(Action::Test(d.clone()));
                }
                if help_button(ui, &d.kind) {
                    action = Some(Action::Help(d.kind.clone()));
                }
            });
            ui.separator();
            let detail = if d.auto { "Pushed as soon as they're saved" } else { "Only pushed when you choose" };
            row(ui, "New screenshots and recordings", detail, |ui| {
                theme::group(ui, |ui| {
                    // Right to left, so they read Automatically, By hand.
                    for (label, value) in [("By hand", false), ("Automatically", true)] {
                        if ui.add(egui::Button::selectable(d.auto == value, label)).clicked() && d.auto != value {
                            action = Some(Action::Auto(d.id.clone(), value));
                        }
                    }
                })
            });
        });
        match self.push.tests.get(&d.id) {
            Some(Test::Done(Ok(found))) => {
                ui.label(RichText::new(format!("{}: {found}", d.name)).small().color(muted));
            }
            Some(Test::Done(Err(e))) => {
                ui.label(RichText::new(format!("{}: {e}", d.name)).small().color(error_color()));
            }
            _ => {}
        }
        action
    }

    /// Does what was clicked in the sign-in rows.
    fn sign_in_click(&mut self, ctx: &egui::Context, form: &mut Form, clicks: &SignInClicks) {
        if clicks.sign_in || clicks.finish {
            form.error = None;
        }
        match form.destination.kind.sign_in() {
            Some(SignInWay::PasteCode) => self.pasted_click(ctx, form, clicks),
            Some(SignInWay::ShowCode) => self.shown_click(ctx, form, clicks),
            None => {}
        }
    }

    /// Asks the service for a code to show, stops waiting for it to be entered, or copies the
    /// page or the code.
    fn shown_click(&mut self, ctx: &egui::Context, form: &mut Form, clicks: &SignInClicks) {
        if clicks.sign_in {
            let cancel = Arc::new(AtomicBool::new(false));
            let events = self.env.sign_in_with_code(ctx, &form.destination.kind, cancel.clone());
            let shown = ShownCode { events, cancel, code: None, copied: None };
            form.sign_in = SignIn { shown: Some(shown), saved_for: form.sign_in.saved_for.take(), ..SignIn::default() };
        } else if clicks.cancel {
            form.sign_in.shown = None;
        } else if let Some(ShownCode { code: Some((code, url)), copied, .. }) = &mut form.sign_in.shown {
            let (text, what) = if clicks.copy_code { (code, "code") } else if clicks.copy { (url, "link") } else { return };
            match self.env.copy_text(text) {
                Ok(()) => *copied = Some(what),
                Err(e) => form.error = Some(format!("Couldn't copy it: {e}")),
            }
        }
    }

    /// Opens the service's page to allow RSnip, or trades the code that page showed for the sign-in.
    fn pasted_click(&mut self, ctx: &egui::Context, form: &mut Form, clicks: &SignInClicks) {
        if clicks.sign_in {
            match Pkce::new() {
                Ok(pkce) => {
                    let key = app_key(&form.destination.kind).trim().to_owned();
                    let url = pkce.authorize_url(&key);
                    let opening = Some(self.env.open_link(ctx, std::path::Path::new(&url)));
                    let pasted = PastedCode { pkce, url, opening, unopened: None, copied: false, app_key: key, code: String::new(), exchanging: None };
                    form.sign_in = SignIn { pasted: Some(pasted), saved_for: form.sign_in.saved_for.take(), ..SignIn::default() };
                }
                Err(e) => form.error = Some(e),
            }
        } else if let Some(pasted) = &mut form.sign_in.pasted {
            if clicks.finish {
                pasted.exchanging = Some(self.env.finish_pasted_sign_in(ctx, &pasted.pkce, &pasted.app_key, &pasted.code));
            }
            if clicks.copy {
                match self.env.copy_text(&pasted.url) {
                    Ok(()) => pasted.copied = true,
                    Err(e) => form.error = Some(format!("Couldn't copy it: {e}")),
                }
            }
        }
    }

    fn form_card(&mut self, ui: &mut Ui) {
        let Some(mut form) = self.push.form.take() else { return };
        let muted = theme::palette(ui).muted;
        let (mut save, mut cancel) = (false, false);
        theme::card(ui, |ui| {
            let kind_name = form.destination.kind.name();
            let title = if form.editing { format!("Edit {}", form.destination.name) } else { format!("Add {kind_name}") };
            let mut help_clicked = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                help_clicked = help_button(ui, &form.destination.kind);
            });
            if help_clicked {
                self.env.open(std::path::Path::new(&doc_url(&form.destination.kind)));
            }
            ui.label(RichText::new(help(&form.destination.kind)).small().color(muted));
            ui.add_space(4.0);
            let busy = form.saving.is_some();
            let mut fetch = false;
            let mut clicks = SignInClicks::default();
            let available = self.env.can_sign_in(&form.destination.kind);
            ui.add_enabled_ui(!busy, |ui| {
                egui::Grid::new("destination-form").num_columns(2).spacing([16.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                    text_row(ui, "Name", "", &mut form.destination.name, false);
                    let has_port = form.destination.kind.port_mut().is_some();
                    let default_port = if matches!(form.destination.kind, Kind::Ftps { .. }) { "21" } else { "22" };
                    for (i, f) in fields(&mut form.destination.kind).into_iter().enumerate() {
                        if f.multiline {
                            lines_row(ui, f.label, f.hint, f.value);
                        } else {
                            text_row(ui, f.label, f.hint, f.value, false);
                        }
                        // The port follows the server.
                        if has_port && i == 0 {
                            text_row(ui, "Port", default_port, &mut form.port, false);
                        }
                    }
                    if form.destination.kind.sign_in().is_some() {
                        clicks = sign_in_rows(ui, &mut form, available);
                    } else if let Some(label) = form.destination.kind.secret_label() {
                        let hint = match (form.editing, form.destination.kind.secret_optional()) {
                            (true, _) => "Leave empty to keep the saved one",
                            (false, true) => "Optional",
                            (false, false) => "",
                        };
                        // A webhook address is worth seeing to check it was pasted whole; it's
                        // still kept in the credential store.
                        let masked = !matches!(form.destination.kind, Kind::Discord { .. });
                        text_row(ui, label, hint, &mut form.secret, masked);
                    }
                    if matches!(form.destination.kind, Kind::Ftps { .. }) {
                        fetch = certificate_row(ui, &mut form);
                    }
                });
            });
            if fetch {
                form.error = None;
                if let Kind::Ftps { host, port, .. } = &form.destination.kind {
                    form.fetching = Some(self.env.fetch_certificate(ui.ctx(), host.trim(), *port));
                }
            }
            if let Some(port) = form.destination.kind.port_mut() {
                *port = form.port.trim().parse().unwrap_or(0);
            }
            self.sign_in_click(ui.ctx(), &mut form, &clicks);
            if let Some(e) = &form.error {
                ui.label(RichText::new(e).color(error_color()));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let problem = form.problem();
                save = ui.add_enabled(problem.is_none() && !busy, theme::primary_button(if busy { "Saving…" } else { "Save" })).clicked();
                cancel = ui.button("Cancel").clicked();
                // Why Save can't be pressed yet, where it's seen rather than only on hover.
                if let Some(problem) = problem {
                    ui.label(RichText::new(problem).small().color(muted));
                }
            });
        });
        if cancel {
            return;
        }
        if save {
            form.error = None;
            for f in fields(&mut form.destination.kind) {
                *f.value = f.value.trim().to_owned();
            }
            form.destination.name = form.destination.name.trim().to_owned();
            let secret = tidy_secret(&form.secret, form.destination.kind.secret_label() == Some("Password"));
            if secret.is_empty() {
                let destination = form.destination.clone();
                return self.commit(destination);
            }
            let d = &form.destination;
            form.saving = Some(self.env.save_secret(ui.ctx(), &d.id, &d.name, &secret));
        }
        self.push.form = Some(form);
    }
}

/// Takes in how signing in is going.
fn poll_sign_in(form: &mut Form) {
    poll_shown(form);
    let Some(pasted) = &mut form.sign_in.pasted else { return };
    if let Some(Ok(opened)) = pasted.opening.as_ref().map(|rx| rx.try_recv()) {
        pasted.opening = None;
        pasted.unopened = opened.err();
    }
    if let Some(Ok(traded)) = pasted.exchanging.as_ref().map(|rx| rx.try_recv()) {
        pasted.exchanging = None;
        match traded {
            Ok(token) => {
                form.secret = token;
                let done = Some(std::mem::take(&mut pasted.app_key));
                form.sign_in = SignIn { done, ..SignIn::default() };
            }
            Err(e) => form.error = Some(e),
        }
    }
}

/// Takes in the code to show, once the service gives it, and how signing in with it ended.
fn poll_shown(form: &mut Form) {
    let Some(shown) = &mut form.sign_in.shown else { return };
    let mut ended = None;
    for event in shown.events.try_iter() {
        match event {
            CodeEvent::Code { user_code, verification_url } => shown.code = Some((user_code, verification_url)),
            CodeEvent::SignedIn { refresh_token, name } => ended = Some(Ok((refresh_token, name))),
            CodeEvent::Failed(e) => ended = Some(Err(e)),
        }
    }
    match ended {
        Some(Ok((token, who))) => {
            form.secret = token;
            form.sign_in = SignIn { done: Some(String::new()), who, ..SignIn::default() };
        }
        Some(Err(e)) => {
            form.error = Some(e);
            form.sign_in.shown = None;
        }
        None => {}
    }
}

/// Whether pushes make links, for the kinds that can be told.
fn share_link(kind: &mut Kind) -> Option<&mut bool> {
    match kind {
        Kind::Dropbox { share_link, .. } | Kind::GoogleDrive { share_link, .. } | Kind::OneDrive { share_link, .. } => Some(share_link),
        _ => None,
    }
}

/// The rows of the form's grid for a kind that signs in: whether to make links, and signing in
/// (if this build can: `available`).
fn sign_in_rows(ui: &mut Ui, form: &mut Form, available: bool) -> SignInClicks {
    let muted = theme::palette(ui).muted;
    if let Some(share_link) = share_link(&mut form.destination.kind) {
        ui.label("Share link");
        ui.horizontal(|ui| {
            theme::switch(ui, share_link, "Share link");
            ui.label(RichText::new("Copy a link to each push, as other places do").small().color(muted));
        });
        ui.end_row();
    }
    match form.destination.kind.sign_in() {
        Some(SignInWay::PasteCode) => pasted_rows(ui, form),
        Some(SignInWay::ShowCode) => shown_rows(ui, form, available),
        None => SignInClicks::default(),
    }
}

/// Signing in with a code shown here: the button, how it's going, and the code and the page to
/// enter it on.
fn shown_rows(ui: &mut Ui, form: &mut Form, available: bool) -> SignInClicks {
    let mut clicks = SignInClicks::default();
    let muted = theme::palette(ui).muted;
    let name = form.destination.kind.name();
    let sign_in = &mut form.sign_in;
    let waiting = sign_in.shown.is_some();
    let status = if !available {
        format!("Signing in to {name} isn't available in this build of RSnip: it was made without RSnip's app for {name}")
    } else if sign_in.done.is_some() {
        match &sign_in.who {
            Some(who) => format!("Signed in as {who}. Save to keep it"),
            None => "Signed in. Save to keep it".to_owned(),
        }
    } else {
        match sign_in.shown.as_ref().map(|s| s.code.is_some()) {
            Some(true) => "Waiting for you to sign in…".to_owned(),
            Some(false) => format!("Asking {name} for a code…"),
            None if sign_in.saved_for.is_some() => "Signed in. Sign in again only to use another account".to_owned(),
            None => "Not signed in yet".to_owned(),
        }
    };
    ui.label(name);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let why = if available { "Signing in already" } else { "Not in this build of RSnip" };
            clicks.sign_in = ui.add_enabled(available && !waiting, egui::Button::new(format!("Sign in to {name}"))).on_disabled_hover_text(why).clicked();
            if waiting {
                clicks.cancel = ui.button("Cancel sign-in").clicked();
            }
        });
        ui.label(RichText::new(status).small().color(muted));
    });
    ui.end_row();

    let Some(ShownCode { code: Some((code, url)), copied, .. }) = &sign_in.shown else { return clicks };
    let text = ui.label("Go to");
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut url.as_str()).desired_width(220.0)).labelled_by(text.id);
        clicks.copy = ui.button("Copy link").clicked();
    });
    ui.end_row();

    ui.label("Enter the code");
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(code).monospace().size(20.0).strong());
            clicks.copy_code = ui.button("Copy code").clicked();
        });
        let note = match copied {
            Some("code") => "Copied: paste it where the page asks for it".to_owned(),
            Some(_) => "Copied: paste it into a browser's address bar".to_owned(),
            None => format!("On this computer or any other (a phone too): go to the page, enter the code, and allow RSnip in {name}"),
        };
        ui.label(RichText::new(note).small().color(muted));
    });
    ui.end_row();
    clicks
}

/// Signing in by pasting a code back, and the code to paste.
fn pasted_rows(ui: &mut Ui, form: &mut Form) -> SignInClicks {
    let mut clicks = SignInClicks::default();
    let muted = theme::palette(ui).muted;
    let name = form.destination.kind.name();
    let has_key = !app_key(&form.destination.kind).trim().is_empty();
    let sign_in = &mut form.sign_in;
    let status = match (sign_in.done.is_some(), sign_in.saved_for.is_some()) {
        (true, _) => "Signed in. Save to keep it".to_owned(),
        (false, true) => "Signed in. Sign in again only to use another account or after changing the app's permissions".to_owned(),
        (false, false) if sign_in.pasted.is_some() => format!("Allow RSnip in the browser, then paste the code {name} shows below"),
        (false, false) => "Not signed in yet".to_owned(),
    };
    ui.label(name);
    ui.vertical(|ui| {
        let ready = has_key && sign_in.pasted.as_ref().is_none_or(|p| p.exchanging.is_none());
        clicks.sign_in = ui.add_enabled(ready, egui::Button::new(format!("Sign in to {name}"))).on_disabled_hover_text("Enter the app key first").clicked();
        ui.label(RichText::new(status).small().color(muted));
    });
    ui.end_row();

    if let Some(pasted) = &mut sign_in.pasted {
        // The page to allow RSnip, for when no browser opened: xdg-open can't always find one.
        let text = ui.label("Sign-in link");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut pasted.url.as_str()).desired_width(220.0)).labelled_by(text.id);
                clicks.copy = ui.button("Copy link").clicked();
            });
            let note = match (&pasted.unopened, pasted.copied) {
                (_, true) => RichText::new("Copied: paste it into a browser's address bar").color(muted),
                (Some(e), false) => RichText::new(format!("The browser didn't open ({e}): copy this link into one")).color(error_color()),
                (None, false) => RichText::new("If no browser opened, copy this link into one").color(muted),
            };
            ui.label(note.small());
        });
        ui.end_row();

        let text = ui.label(format!("Code from {name}"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut pasted.code).desired_width(220.0)).labelled_by(text.id);
            let exchanging = pasted.exchanging.is_some();
            let ready = !pasted.code.trim().is_empty() && !exchanging;
            clicks.finish = ui.add_enabled(ready, egui::Button::new(if exchanging { "Signing in…" } else { "Finish signing in" })).clicked();
        });
        ui.end_row();
    }
    clicks
}

/// A label and a single-line text box for it, as a row of the form's grid.
fn text_row(ui: &mut Ui, label: &str, hint: &str, value: &mut String, password: bool) {
    let text = ui.label(label);
    ui.add(egui::TextEdit::singleline(value).hint_text(hint).password(password).desired_width(360.0)).labelled_by(text.id);
    ui.end_row();
}

/// An FTPS server's certificate: checked by the computer, or one the user trusts (a NAS's
/// self-signed one), with what's needed to trust it or stop. Whether to fetch the certificate.
fn certificate_row(ui: &mut Ui, form: &mut Form) -> bool {
    let Kind::Ftps { host, port, trusted, .. } = &mut form.destination.kind else { return false };
    let mut fetch = false;
    let text = ui.label("Certificate");
    ui.vertical(|ui| {
        let small = |text: &str| RichText::new(text).small();
        if !trusted.is_empty() {
            ui.label(small("Trusted, as this one:"));
            ui.label(small(trusted).monospace());
            if ui.button("Stop trusting it").clicked() {
                trusted.clear();
            }
        } else if let Some(offered) = form.offered.clone() {
            ui.label(small("The server shows this one. Trust it only if your server's settings show the same:"));
            ui.label(small(&offered).monospace());
            ui.horizontal(|ui| {
                if ui.add(theme::primary_button("Trust it")).clicked() {
                    *trusted = offered;
                    form.offered = None;
                }
                if ui.button("Don't").clicked() {
                    form.offered = None;
                }
            });
        } else {
            ui.label(small("Checked by this computer's trusted authorities"));
            let fetching = form.fetching.is_some();
            let ready = !host.trim().is_empty() && *port != 0;
            let button = egui::Button::new(if fetching { "Getting it…" } else { "Trust the server's certificate…" });
            fetch = ui.add_enabled(ready && !fetching, button).on_hover_text("For a self-signed one, a NAS's say: see it, then trust that one").clicked();
        }
    })
    .response
    .labelled_by(text.id);
    ui.end_row();
    fetch
}

/// Where docs/destinations.md says how to set up `kind`.
fn doc_url(kind: &Kind) -> String {
    format!("{}/blob/main/docs/destinations.md#{}", super::REPO, kind.doc_anchor())
}

/// The ? that opens `kind`'s section of docs/destinations.md; whether it was clicked.
fn help_button(ui: &mut Ui, kind: &Kind) -> bool {
    ui.button(icon::QUESTION).on_hover_text(format!("How to set up {}, and what it needs", kind.name())).clicked()
}

/// A label and a text box a few lines high, for one entry a line.
fn lines_row(ui: &mut Ui, label: &str, hint: &str, value: &mut String) {
    let text = ui.label(label);
    ui.add(egui::TextEdit::multiline(value).hint_text(hint).desired_rows(3).desired_width(360.0)).labelled_by(text.id);
    ui.end_row();
}

/// What the Add… menu was asked for.
enum Added {
    Kind(Kind),
    /// A custom uploader, from a ShareX file.
    Sxcu,
}

/// The Add… button, a menu of the kinds of place; the one chosen.
fn add_menu(ui: &mut Ui) -> Option<Added> {
    let mut chosen = None;
    ui.menu_button("Add…", |ui| {
        for make in Kind::ALL {
            let kind = make();
            if ui.button(kind.name()).clicked() {
                chosen = Some(Added::Kind(kind));
                ui.close();
            }
        }
        if ui.button("Custom uploader from a ShareX file (.sxcu)…").clicked() {
            chosen = Some(Added::Sxcu);
            ui.close();
        }
    });
    chosen
}
