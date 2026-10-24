//! Pushing screenshots and recordings to a destination by hand: the Push to menu (in Export, and
//! the library's right-click menu), progress in the status bar, and the links that come back.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;

use super::env::PushEvent;
use super::theme::{self, icon};
use super::App;
use crate::push::{self, Destination};

/// Files being pushed to a destination.
pub struct Pushing {
    to: String,
    total: usize,
    events: Receiver<PushEvent>,
    links: Vec<String>,
    pushed: usize,
    /// The first thing that went wrong, and how many did.
    failed: Option<(String, usize)>,
}

impl App {
    /// The Push to menu for `files`: the destinations from Settings → Push to.
    pub(super) fn push_menu(&mut self, ui: &mut egui::Ui, files: &[PathBuf]) {
        ui.menu_button(theme::label(icon::CLOUD_ARROW_UP, "Push to"), |ui| {
            let destinations = push::load(&self.env);
            if destinations.is_empty() && ui.button("Set up places to push to…").clicked() {
                self.env.open_settings();
                ui.close();
            }
            for destination in destinations {
                let label = if destination.auto { format!("{} (also pushed automatically)", destination.name) } else { destination.name.clone() };
                let button = ui.add_enabled(self.pushing.is_none(), egui::Button::new(label));
                if button.on_disabled_hover_text("Still pushing").clicked() {
                    self.push_to(ui.ctx(), &destination, files.to_vec());
                    ui.close();
                }
            }
        });
    }

    /// Starts pushing `files` to `destination`.
    pub(super) fn push_to(&mut self, ctx: &egui::Context, destination: &Destination, files: Vec<PathBuf>) {
        if self.pushing.is_some() || files.is_empty() {
            return;
        }
        let total = files.len();
        let events = self.env.push_files(ctx, destination, files);
        self.pushing = Some(Pushing { to: destination.name.clone(), total, events, links: Vec::new(), pushed: 0, failed: None });
        self.set_status(format!("Pushing to {}…", destination.name));
    }

    /// Takes in how the push is going, and says so in the status bar.
    pub(super) fn poll_pushing(&mut self) {
        let Some(p) = &mut self.pushing else { return };
        let mut status = None;
        let mut done = false;
        let mut changed = Vec::new();
        for event in p.events.try_iter() {
            match event {
                PushEvent::Pushing(i) if p.total > 1 => status = Some(format!("Pushing {} of {} to {}…", i + 1, p.total, p.to)),
                PushEvent::Pushing(_) => {}
                PushEvent::Pushed(file, link) => {
                    p.pushed += 1;
                    p.links.extend(link);
                    changed.push(file);
                }
                PushEvent::Failed(_, error) => {
                    p.failed = Some(match p.failed.take() {
                        Some((first, n)) => (first, n + 1),
                        None => (error, 1),
                    });
                }
                PushEvent::Done => done = true,
            }
        }
        // The library shows what's been pushed where (Copy link).
        for file in changed {
            if self.metas.contains_key(&file) {
                self.metas.insert(file.clone(), super::search::load(&file));
            }
        }
        if let Some(status) = status {
            self.set_status(status);
        }
        if !done {
            return;
        }
        let Some(p) = self.pushing.take() else { return };
        let copied = !p.links.is_empty() && self.env.copy_text(&p.links.join("\n")).is_ok();
        let link = match (copied, p.links.len()) {
            (false, _) => "",
            (true, 1) => ", link copied",
            (true, _) => ", links copied",
        };
        self.set_status(match p.failed {
            None if p.total == 1 => format!("Pushed to {}{link}", p.to),
            None => format!("Pushed {} to {}{link}", p.pushed, p.to),
            Some((error, _)) if p.total == 1 => format!("Couldn't push to {}: {error}", p.to),
            Some((error, failed)) => format!("Pushed {} of {} to {}{link}; {failed} failed: {error}", p.pushed, p.total, p.to),
        });
    }

    /// The links to `file` where it's been pushed, named for the destinations still in Settings.
    pub(super) fn links(&self, file: &PathBuf) -> Vec<(String, String)> {
        let Some(meta) = self.metas.get(file) else { return Vec::new() };
        let destinations = push::load(&self.env);
        meta.pushed
            .iter()
            .filter_map(|(id, pushed)| {
                let name = &destinations.iter().find(|d| &d.id == id)?.name;
                Some((name.clone(), pushed.link.clone()?))
            })
            .collect()
    }
}
