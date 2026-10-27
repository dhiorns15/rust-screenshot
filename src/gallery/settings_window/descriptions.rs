//! Settings → Library & editor → Descriptions: describing screenshots with an AI model, with the
//! user's own key (see `describe`), so they can be searched for what's in them. Saving or reading
//! the key, listing the models, the test and describing the library all happen off the UI thread
//! (the keyring can wait for the user to unlock it, and the network is slow); each answer arrives
//! on a channel that is polled every frame, like the Push to section's. New screenshots are
//! described by the process that pushes (see `gallery::describer`); the library is described here,
//! so it can be watched and stopped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText, Ui};

use super::{row, SettingsApp};
use crate::describe::{self, Described, Model, Provider};
use crate::gallery::describer::{self, Event};
use crate::gallery::theme;
use crate::options;

type Answer<T> = Receiver<Result<T, String>>;

/// How much of the test's description is shown.
const SAMPLE_SHOWN: usize = 300;

#[derive(Default)]
pub struct DescribeState {
    /// The key as typed.
    key: String,
    /// Whether a key is saved for a provider, once found out.
    saved: Option<(Provider, bool)>,
    asking: Option<(Provider, Answer<bool>)>,
    saving: Option<(Provider, Answer<()>)>,
    forgetting: Option<(Provider, Answer<()>)>,
    /// Why the key couldn't be saved or forgotten.
    problem: Option<String>,
    models: Option<(Provider, Models)>,
    test: Option<Test>,
    library: Option<Library>,
}

enum Models {
    Loading(Answer<Vec<Model>>),
    Loaded(Vec<Model>),
    Failed(String),
}

enum Test {
    Running(Answer<Described>),
    Done(Result<Described, String>),
}

/// Describing the screenshots from before.
enum Library {
    /// What it would come to, to be agreed to.
    Offered { files: Vec<PathBuf>, cost: Option<f64>, provider: Provider, model: String },
    Running { events: Receiver<Event>, stop: Arc<AtomicBool>, total: usize, described: usize, skipped: usize },
    /// How it ended.
    Done(String),
}

fn error_color() -> egui::Color32 {
    egui::Color32::from_rgb(255, 120, 110)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "1 screenshot", "340 screenshots".
fn screenshots(n: usize) -> String {
    if n == 1 { "1 screenshot".into() } else { format!("{n} screenshots") }
}

/// The part of `text` to show, cut at a space if it's long.
fn start_of(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text.to_owned(),
    }
}

impl SettingsApp {
    fn describe_provider(&self) -> Provider {
        Provider::from_setting(options::DESCRIBE_PROVIDER.read(|name| self.env.setting(name)))
    }

    /// The model chosen, or the provider's own.
    fn describe_model(&self, provider: Provider) -> String {
        describer::chosen_model(provider, self.env.text_setting(options::DESCRIBE_MODEL))
    }

    /// The Descriptions card.
    pub(super) fn descriptions(&mut self, ui: &mut Ui) {
        self.poll_describe();
        let on = self.get(options::DESCRIBE);
        let mut turned = None;
        theme::card(ui, |ui| {
            let title = "Describe screenshots with AI";
            let detail = "Each new screenshot is sent to Anthropic or OpenAI, with your own API key, and described so you can search for what's in it";
            let mut now_on = on;
            if row(ui, title, detail, |ui| theme::switch(ui, &mut now_on, title)).changed() {
                turned = Some(now_on);
            }
            if on {
                self.describe_options(ui);
            }
        });
        if let Some(on) = turned {
            self.set(options::DESCRIBE.name, on as u32);
            if on {
                // Only what's taken from now on is sent by itself.
                self.set(options::DESCRIBE_SINCE, now() as u32);
            }
            self.env.set_text_setting(options::DESCRIBE_STATUS, "");
            self.describe = DescribeState::default();
        }
    }

    fn describe_options(&mut self, ui: &mut Ui) {
        let provider = self.describe_provider();
        let model = self.describe_model(provider);
        let muted = theme::palette(ui).muted;
        self.look_up_key(ui.ctx(), provider);

        ui.separator();
        row(ui, "Provider", "Who describes them, and bills you for it", |ui| {
            theme::group(ui, |ui| {
                // Right to left, so they read Anthropic, OpenAI.
                for choice in Provider::ALL.into_iter().rev() {
                    if ui.add(egui::Button::selectable(provider == choice, choice.name())).clicked() && provider != choice {
                        self.set(options::DESCRIBE_PROVIDER.name, choice.to_setting());
                        // Another provider has other models.
                        self.env.set_text_setting(options::DESCRIBE_MODEL, "");
                        self.env.set_text_setting(options::DESCRIBE_STATUS, "");
                        self.describe = DescribeState::default();
                    }
                }
            })
        });

        ui.separator();
        self.key_row(ui, provider);

        ui.separator();
        self.model_row(ui, provider, &model);

        ui.separator();
        let saved = self.describe.saved == Some((provider, true));
        let testing = matches!(self.describe.test, Some(Test::Running(_)));
        row(ui, "Try it", "Describes a small sample picture, to see that the key and model work", |ui| {
            let button = egui::Button::new(if testing { "Testing…" } else { "Test" });
            if ui.add_enabled(saved && !testing, button).on_disabled_hover_text(if testing { "" } else { "Save a key first" }).clicked() {
                self.describe.test = Some(Test::Running(self.env.describe_sample(ui.ctx(), provider, &model)));
            }
        });
        match &self.describe.test {
            Some(Test::Done(Ok(d))) => {
                ui.label(RichText::new(format!("It works. {} said: {}", d.model, start_of(&d.text, SAMPLE_SHOWN))).small().color(muted));
            }
            Some(Test::Done(Err(e))) => {
                ui.label(RichText::new(e).small().color(error_color()));
            }
            _ => {}
        }

        ui.separator();
        self.library_row(ui, provider, &model, saved);

        if let Some(status) = self.env.text_setting(options::DESCRIBE_STATUS).filter(|s| !s.is_empty()) {
            ui.label(RichText::new(format!("Last problem describing new screenshots: {status}")).small().color(error_color()));
        }
    }

    /// The API key: a box to paste it in, and whether one is saved.
    fn key_row(&mut self, ui: &mut Ui, provider: Provider) {
        let muted = theme::palette(ui).muted;
        let saved = self.describe.saved == Some((provider, true));
        let saving = self.describe.saving.is_some();
        let busy = saving || self.describe.forgetting.is_some();
        let (mut save, mut forget) = (false, false);
        ui.horizontal(|ui| {
            let label = ui.label("API key");
            let hint = if saved { "Leave empty to keep the saved one" } else { "Paste your key" };
            let typing = egui::TextEdit::singleline(&mut self.describe.key).password(true).hint_text(hint).desired_width(300.0);
            ui.add_enabled(!busy, typing).labelled_by(label.id);
            let ready = !self.describe.key.trim().is_empty() && !busy;
            save = ui.add_enabled(ready, theme::primary_button(if saving { "Saving…" } else { "Save" })).clicked();
            if saved {
                forget = ui.add_enabled(!busy, egui::Button::new("Forget")).on_hover_text("Delete the saved key").clicked();
            }
        });
        let store = if cfg!(windows) { "Windows' Credential Manager" } else { "the desktop's keyring" };
        let state = if saved { "A key is saved" } else { "No key is saved" };
        ui.label(RichText::new(format!("{state}. Get one at {}. It's kept in {store}, never in RSnip's settings.", provider.keys_page())).small().color(muted));
        if let Some(problem) = &self.describe.problem {
            ui.label(RichText::new(problem).small().color(error_color()));
        }
        if save {
            self.describe.problem = None;
            let label = format!("RSnip: {} key for descriptions", provider.name());
            let rx = self.env.save_secret(ui.ctx(), provider.secret_id(), &label, self.describe.key.trim());
            self.describe.saving = Some((provider, rx));
        }
        if forget {
            self.describe.problem = None;
            self.describe.forgetting = Some((provider, self.env.forget_secret(ui.ctx(), provider.secret_id())));
        }
    }

    /// The model drop-down, with what it costs.
    fn model_row(&mut self, ui: &mut Ui, provider: Provider, model: &str) {
        let saved = self.describe.saved == Some((provider, true));
        if saved && self.describe.models.as_ref().is_none_or(|(p, _)| *p != provider) {
            self.describe.models = Some((provider, Models::Loading(self.env.describe_models(ui.ctx(), provider))));
        }
        let cost = describe::cost(provider, model, describe::TYPICAL_SIZE);
        let detail = match cost {
            Some(each) => format!("About {} per 100 screenshots", describe::dollars(each * 100.0)),
            None => "Cost unknown for this model".to_owned(),
        };
        let mut retry = false;
        let mut pick = None;
        row(ui, "Model", &detail, |ui| {
            let combo = egui::ComboBox::from_id_salt("describe-model").width(240.0).truncate().selected_text(model);
            let list = match &self.describe.models {
                Some((p, Models::Loaded(list))) if *p == provider => list.as_slice(),
                _ => &[],
            };
            let loaded = !list.is_empty();
            ui.add_enabled_ui(loaded, |ui| {
                combo.show_ui(ui, |ui| {
                    // Each with what 100 screenshots would cost, to compare them.
                    let muted = theme::palette(ui).muted;
                    egui::Grid::new("describe-models").num_columns(2).show(ui, |ui| {
                        ui.label("");
                        ui.label(RichText::new("Per 100").small().color(muted));
                        ui.end_row();
                        for m in list {
                            if ui.selectable_label(m.id == model, &m.id).clicked() {
                                pick = Some(m.id.clone());
                            }
                            let price = describe::cost(provider, &m.id, describe::TYPICAL_SIZE).map_or("?".to_owned(), |each| describe::dollars(each * 100.0));
                            ui.label(RichText::new(price).color(muted));
                            ui.end_row();
                        }
                    });
                    // One chosen before that the provider no longer lists is still the one used.
                    if !list.iter().any(|m| m.id == model) {
                        let _ = ui.selectable_label(true, model);
                    }
                });
            });
            if matches!(self.describe.models, Some((_, Models::Failed(_)))) {
                retry = ui.small_button("Try again").clicked();
            }
        });
        match &self.describe.models {
            Some((_, Models::Loading(_))) => {
                ui.label(RichText::new("Loading the models…").small().color(theme::palette(ui).muted));
            }
            Some((_, Models::Failed(e))) => {
                ui.label(RichText::new(format!("Couldn't list the models: {e}")).small().color(error_color()));
            }
            _ if !saved => {
                ui.label(RichText::new("Save a key to choose from the models listed there. Until then, the default is used.").small().color(theme::palette(ui).muted));
            }
            _ => {}
        }
        if let Some(id) = pick.filter(|id| id != model) {
            self.env.set_text_setting(options::DESCRIBE_MODEL, &id);
            self.describe.test = None;
        }
        if retry {
            self.describe.models = None;
        }
    }

    /// Describing the screenshots from before: asks first, then shows how far it is.
    fn library_row(&mut self, ui: &mut Ui, provider: Provider, model: &str, saved: bool) {
        let muted = theme::palette(ui).muted;
        enum Click {
            Offer,
            Start,
            Cancel,
            Stop,
        }
        let mut click = None;
        match &self.describe.library {
            Some(Library::Offered { files, cost, .. }) => {
                let price = match cost {
                    Some(cost) => format!("about {}", describe::dollars(*cost)),
                    None => "cost unknown for this model".to_owned(),
                };
                row(ui, &format!("{}, {price} — Describe them?", screenshots(files.len())), &format!("Each is sent to {} to be described", provider.name()), |ui| {
                    if ui.add(theme::primary_button("Describe")).clicked() {
                        click = Some(Click::Start);
                    }
                    if ui.button("Cancel").clicked() {
                        click = Some(Click::Cancel);
                    }
                });
            }
            Some(Library::Running { stop, total, described, skipped, .. }) => {
                let stopping = stop.load(Ordering::Relaxed);
                row(ui, &format!("Described {} of {total}", described + skipped), "Keep Settings open: closing it stops this, and you can carry on later", |ui| {
                    if ui.add_enabled(!stopping, egui::Button::new(if stopping { "Stopping…" } else { "Stop" })).clicked() {
                        click = Some(Click::Stop);
                    }
                });
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            done => {
                row(ui, "Describe the library", "The screenshots from before you turned this on, and any missed", |ui| {
                    if ui.add_enabled(saved, egui::Button::new("Describe the library…")).on_disabled_hover_text("Save a key first").clicked() {
                        click = Some(Click::Offer);
                    }
                });
                if let Some(Library::Done(how)) = done {
                    ui.label(RichText::new(how).small().color(muted));
                }
            }
        }
        match click {
            None => {}
            Some(Click::Offer) => {
                let files = describer::undescribed(&self.env.dir);
                if files.is_empty() {
                    self.describe.library = Some(Library::Done("Every screenshot has a description already".into()));
                } else {
                    let cost = describer::estimate(provider, model, &files);
                    self.describe.library = Some(Library::Offered { files, cost, provider, model: model.to_owned() });
                }
            }
            Some(Click::Cancel) => self.describe.library = None,
            Some(Click::Start) => {
                if let Some(Library::Offered { files, provider, model, .. }) = self.describe.library.take() {
                    let total = files.len();
                    let stop = Arc::new(AtomicBool::new(false));
                    let events = self.env.describe_library(ui.ctx(), provider, &model, files, stop.clone());
                    self.describe.library = Some(Library::Running { events, stop, total, described: 0, skipped: 0 });
                }
            }
            Some(Click::Stop) => {
                if let Some(Library::Running { stop, .. }) = &self.describe.library {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    /// Asks whether a key is saved for `provider`, if that's not known.
    fn look_up_key(&mut self, ctx: &egui::Context, provider: Provider) {
        let known = self.describe.saved.is_some_and(|(p, _)| p == provider);
        let asking = self.describe.asking.as_ref().is_some_and(|(p, _)| *p == provider);
        if !known && !asking {
            self.describe.asking = Some((provider, self.env.describe_key_saved(ctx, provider)));
        }
    }

    /// Picks up the answers that have arrived.
    fn poll_describe(&mut self) {
        let state = &mut self.describe;
        if let Some((provider, Ok(answer))) = state.asking.as_ref().map(|(p, rx)| (*p, rx.try_recv())) {
            state.asking = None;
            // A key that can't be looked up is as good as none; saving one says why.
            state.saved = Some((provider, answer.unwrap_or(false)));
        }
        if let Some((provider, Ok(answer))) = state.saving.as_ref().map(|(p, rx)| (*p, rx.try_recv())) {
            state.saving = None;
            match answer {
                Ok(()) => {
                    state.key.clear();
                    state.saved = Some((provider, true));
                    // The models are listed again with it, and anything that stopped is tried again.
                    state.models = None;
                    state.test = None;
                    self.env.set_text_setting(options::DESCRIBE_STATUS, "");
                }
                Err(e) => state.problem = Some(format!("Couldn't save the key: {e}")),
            }
        }
        if let Some((provider, Ok(answer))) = state.forgetting.as_ref().map(|(p, rx)| (*p, rx.try_recv())) {
            state.forgetting = None;
            match answer {
                Ok(()) => {
                    state.saved = Some((provider, false));
                    state.models = None;
                    state.test = None;
                }
                Err(e) => state.problem = Some(format!("Couldn't forget the key: {e}")),
            }
        }
        if let Some((_, models)) = &mut state.models {
            if let Models::Loading(rx) = models {
                if let Ok(answer) = rx.try_recv() {
                    *models = match answer {
                        Ok(list) => Models::Loaded(list),
                        Err(e) => Models::Failed(e),
                    };
                }
            }
        }
        if let Some(Test::Running(rx)) = &state.test {
            if let Ok(answer) = rx.try_recv() {
                state.test = Some(Test::Done(answer));
            }
        }
        self.poll_library();
    }

    fn poll_library(&mut self) {
        let Some(Library::Running { events, stop, total, described, skipped }) = &mut self.describe.library else { return };
        let mut finished = None;
        for event in events.try_iter() {
            match event {
                Event::Described(_) => *described += 1,
                Event::Skipped(..) => *skipped += 1,
                Event::Finished(why) => finished = Some(why),
            }
        }
        let Some(why) = finished else { return };
        let (stopped, total, described, skipped) = (stop.load(Ordering::Relaxed), *total, *described, *skipped);
        let left_out = match skipped {
            0 => String::new(),
            n => format!(", {n} couldn't be"),
        };
        let how = match why {
            Some(why) => format!("Stopped after {described} of {total}: {why}"),
            None if stopped => format!("Stopped after {described} of {total}{left_out}"),
            None => format!("Described {}{left_out}", screenshots(described)),
        };
        self.describe.library = Some(Library::Done(how));
    }
}
