//! Describing screenshots with an AI model by themselves (see `describe`), so they can be searched
//! for what's in them. One RSnip process does it, the one that pushes automatically (see
//! `auto_push`: the tray app on Windows, the background icon on Linux, else the library). While
//! Settings → Library & editor → Descriptions is on and a key is saved, every few seconds it looks
//! in the save folder for a screenshot taken since it was turned on that has no description, and
//! asks for one, a screenshot at a time. When the provider is busy it waits, and when the key (or
//! the account) won't do it stops, tells Settings (through `options::DESCRIBE_STATUS`) and waits
//! for that to be cleared, which Settings does when the key or provider is changed. The Describe
//! the library button of Settings describes the older ones, in the Settings window's own process
//! (`describe_all`): that one is there to be watched and stopped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{is_video, library_files_with_times, platform, search};
use crate::describe::{self, Described, Problem, Provider, Service};
use crate::options;

/// How often the folder is looked at.
const EVERY: Duration = Duration::from_secs(5);
/// A file this new may still be being written.
const SETTLE: Duration = Duration::from_secs(2);
/// How long a key read from the credential store (or that there's none) is kept, so a new one is
/// picked up.
const KEY_KEPT: Duration = Duration::from_secs(60);
/// How long to wait after the credential store wouldn't give the key (say the keyring is locked:
/// it's never asked to unlock from here, which is left to Settings).
const KEYRING_WAIT: Duration = Duration::from_secs(60);

/// What the describer reads and does outside itself, so tests can stand in for it.
pub trait World {
    fn setting(&self, name: &str) -> Option<u32>;
    fn text_setting(&self, name: &str) -> Option<String>;
    fn set_text_setting(&self, name: &str, value: &str);
    fn dir(&self) -> PathBuf;
    /// The key saved for `provider`.
    fn key(&self, provider: Provider) -> Result<Option<String>, String>;
    fn describe(&self, provider: Provider, key: &str, model: &str, image: &Path) -> Result<Described, Problem>;
}

/// This computer's settings, credential store and network.
struct Real;

impl World for Real {
    fn setting(&self, name: &str) -> Option<u32> {
        platform::setting(name)
    }

    fn text_setting(&self, name: &str) -> Option<String> {
        platform::text_setting(name)
    }

    fn set_text_setting(&self, name: &str, value: &str) {
        platform::set_text_setting(name, value);
    }

    fn dir(&self) -> PathBuf {
        platform::screenshots_dir()
    }

    fn key(&self, provider: Provider) -> Result<Option<String>, String> {
        crate::secrets::get_quietly(provider.secret_id())
    }

    fn describe(&self, provider: Provider, key: &str, model: &str, image: &Path) -> Result<Described, Problem> {
        describe::describe(&Service::new(provider, key), model, image)
    }
}

/// Describes new screenshots from this process, while no other RSnip process does (the caller has
/// the push lock). `notify` shows a desktop notification (title, text).
pub fn start(notify: impl Fn(&str, &str) + Send + 'static) {
    std::thread::spawn(move || {
        let mut describer = Describer::default();
        loop {
            // One after another while there are more, then a rest.
            while describer.step(&Real, SystemTime::now(), &notify) {}
            std::thread::sleep(EVERY);
        }
    });
}

/// What it has learned since it started.
#[derive(Default)]
pub struct Describer {
    /// Nothing will work until Settings changes something (see `Problem::Key`).
    blocked: bool,
    /// Not before this: the provider is busy, or the credential store wouldn't open.
    wait_until: Option<SystemTime>,
    /// The key for a provider, and when it was read.
    key: Option<(Provider, Option<String>, SystemTime)>,
    /// The credential store wouldn't give the key, which Settings is showing.
    keyring_failed: bool,
    /// Screenshots the provider declined, or that couldn't be read, so they're not asked about again.
    skipped: HashSet<PathBuf>,
}

impl Describer {
    /// Describes one screenshot if there is one to describe, at `now`; true if there may be more
    /// to do straight away.
    pub fn step(&mut self, world: &dyn World, now: SystemTime, notify: &dyn Fn(&str, &str)) -> bool {
        if !options::DESCRIBE.read(|name| world.setting(name)) {
            *self = Describer::default();
            return false;
        }
        if self.blocked {
            if world.text_setting(options::DESCRIBE_STATUS).is_some_and(|status| !status.is_empty()) {
                return false;
            }
            self.blocked = false;
            self.key = None;
        }
        if self.wait_until.is_some_and(|until| now < until) {
            return false;
        }
        // Without a time to count from, nothing is described by itself (the library is Settings').
        let Some(since) = world.setting(options::DESCRIBE_SINCE) else { return false };
        let provider = Provider::from_setting(options::DESCRIBE_PROVIDER.read(|name| world.setting(name)));
        let model = chosen_model(provider, world.text_setting(options::DESCRIBE_MODEL));
        // The key first: without one, the folder's search data isn't worth reading every few seconds.
        let key = match self.key(world, provider, now) {
            Ok(key) => key,
            Err(e) => {
                self.wait_until = Some(now + KEYRING_WAIT);
                self.keyring_failed = true;
                set_status(world, &e);
                return false;
            }
        };
        // It gave the key (or that there's none) after all, so that's no longer the problem.
        if std::mem::take(&mut self.keyring_failed) {
            set_status(world, "");
        }
        // Nothing to describe with; Settings says so.
        let Some(key) = key else { return false };
        let dir = world.dir();
        let Some(file) = due(&library_files_with_times(&dir), UNIX_EPOCH + Duration::from_secs(u64::from(since)), now, &self.skipped).into_iter().next() else { return false };

        let described = world.describe(provider, &key, &model, &file).and_then(|d| search::record_description(&file, &d.text, &d.model).map_err(Problem::Skip));
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        match described {
            Ok(()) => {
                set_status(world, "");
                true
            }
            Err(Problem::Later(wait, why)) => {
                self.wait_until = Some(now + wait);
                set_status(world, &why);
                false
            }
            Err(Problem::Key(why)) => {
                self.blocked = true;
                if options::NOTIFY.read(|name| world.setting(name)) && world.text_setting(options::DESCRIBE_STATUS).as_deref() != Some(why.as_str()) {
                    notify("Couldn't describe screenshots", &why);
                }
                set_status(world, &why);
                false
            }
            Err(Problem::Skip(why)) => {
                self.skipped.insert(file);
                set_status(world, &format!("{name}: {why}"));
                true
            }
        }
    }

    /// The key for `provider`: kept for a minute, as it comes from the credential store.
    fn key(&mut self, world: &dyn World, provider: Provider, now: SystemTime) -> Result<Option<String>, String> {
        if let Some((kept_for, key, at)) = &self.key {
            if *kept_for == provider && now.duration_since(*at).is_ok_and(|age| age < KEY_KEPT) {
                return Ok(key.clone());
            }
        }
        let key = world.key(provider)?.map(|key| key.trim().to_owned()).filter(|key| !key.is_empty());
        self.key = Some((provider, key.clone(), now));
        Ok(key)
    }
}

/// Writes what stopped the describing for Settings to show (empty: nothing does), if it's changed.
fn set_status(world: &dyn World, status: &str) {
    if world.text_setting(options::DESCRIBE_STATUS).unwrap_or_default() != status {
        world.set_text_setting(options::DESCRIBE_STATUS, status);
    }
}

/// The model chosen in Settings, else the provider's own.
pub fn chosen_model(provider: Provider, chosen: Option<String>) -> String {
    chosen.map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).unwrap_or_else(|| provider.default_model().to_owned())
}

/// The screenshots among `files` to describe by themselves at `now`: taken since `since`, finished
/// being written, not asked about already and without a description. The oldest first.
fn due(files: &[(PathBuf, SystemTime)], since: SystemTime, now: SystemTime, skipped: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let mut due: Vec<&(PathBuf, SystemTime)> = files
        .iter()
        .filter(|(file, modified)| *modified >= since && now.duration_since(*modified).unwrap_or_default() >= SETTLE && !is_video(file) && !skipped.contains(file))
        .filter(|(file, _)| search::load(file).description.is_none())
        .collect();
    due.sort_by_key(|(_, modified)| *modified);
    due.into_iter().map(|(file, _)| file.clone()).collect()
}

/// The screenshots in `dir` without a description, newest first (recordings aren't described).
pub fn undescribed(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(PathBuf, SystemTime)> = library_files_with_times(dir).into_iter().filter(|(file, _)| !is_video(file) && search::load(file).description.is_none()).collect();
    files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    files.into_iter().map(|(file, _)| file).collect()
}

/// What describing `files` with `model` would cost, about, in dollars (`None` if the model's price
/// isn't known).
pub fn estimate(provider: Provider, model: &str, files: &[PathBuf]) -> Option<f64> {
    files.iter().map(|file| describe::cost(provider, model, image::image_dimensions(file).unwrap_or(describe::TYPICAL_SIZE))).sum()
}

/// How describing a number of screenshots at once is going.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    Described(PathBuf),
    /// Not described, but the rest go on.
    Skipped(PathBuf, String),
    /// That's all: with what stopped it, if something did (rather than the end of the list, or Stop).
    Finished(Option<String>),
}

/// How many times a screenshot is tried when the provider is busy, and the longest wait between.
const TRIES: usize = 3;
const LONGEST_WAIT: Duration = Duration::from_secs(120);

/// Describes `files` one after another with `describe`, noting each description in the
/// screenshot's search data, until they're done or `stop` is set. Reports each on `report`.
pub fn describe_all(files: &[PathBuf], describe: &dyn Fn(&Path) -> Result<Described, Problem>, stop: &AtomicBool, report: &dyn Fn(Event)) {
    let finished = |why: Option<String>| report(Event::Finished(why));
    for file in files {
        if stop.load(Ordering::Relaxed) {
            return finished(None);
        }
        // Another process may have described it since the list was made.
        if search::load(file).description.is_some() {
            continue;
        }
        let mut tries = 0;
        let result = loop {
            tries += 1;
            match describe(file) {
                Err(Problem::Later(wait, why)) => {
                    if tries == TRIES {
                        return finished(Some(why));
                    }
                    if !wait_unless_stopped(wait.min(LONGEST_WAIT), stop) {
                        return finished(None);
                    }
                }
                other => break other,
            }
        };
        match result.and_then(|d| search::record_description(file, &d.text, &d.model).map_err(Problem::Skip)) {
            Ok(()) => report(Event::Described(file.clone())),
            Err(Problem::Skip(why)) => report(Event::Skipped(file.clone(), why)),
            Err(problem) => return finished(Some(problem.message().to_owned())),
        }
    }
    finished(None)
}

/// Waits `time`; false if `stop` was set meanwhile.
fn wait_unless_stopped(time: Duration, stop: &AtomicBool) -> bool {
    let end = std::time::Instant::now() + time;
    while std::time::Instant::now() < end {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100).min(time));
    }
    !stop.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// Settings in memory, a folder, and a model whose answers the test chooses.
    struct Fake {
        dir: PathBuf,
        numbers: RefCell<BTreeMap<String, u32>>,
        texts: RefCell<BTreeMap<String, String>>,
        key: RefCell<Result<Option<String>, String>>,
        /// What each ask for a description gets, in turn; then a plain description.
        answers: RefCell<Vec<Result<Described, Problem>>>,
        /// The files asked about, with the model and key used.
        asked: RefCell<Vec<(String, String, String)>>,
        key_reads: Cell<usize>,
        /// How many times the folder was looked in.
        dir_reads: Cell<usize>,
    }

    impl Fake {
        fn new(name: &str) -> Fake {
            let fake = Fake {
                dir: crate::gallery::tests::sandbox_dir(name),
                numbers: Default::default(),
                texts: Default::default(),
                key: RefCell::new(Ok(Some("sk-test".into()))),
                answers: Default::default(),
                asked: Default::default(),
                key_reads: Cell::new(0),
                dir_reads: Cell::new(0),
            };
            fake.numbers.borrow_mut().insert(options::DESCRIBE.name.into(), 1);
            fake.numbers.borrow_mut().insert(options::DESCRIBE_SINCE.into(), 1000);
            fake
        }

        /// A screenshot (or recording) last changed at `secs`.
        fn file(&self, name: &str, secs: u64) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, b"data").unwrap();
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(at(secs)).unwrap();
            path
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().iter().map(|(file, ..)| file.clone()).collect()
        }

        fn status(&self) -> String {
            self.texts.borrow().get(options::DESCRIBE_STATUS).cloned().unwrap_or_default()
        }
    }

    impl World for Fake {
        fn setting(&self, name: &str) -> Option<u32> {
            self.numbers.borrow().get(name).copied()
        }

        fn text_setting(&self, name: &str) -> Option<String> {
            self.texts.borrow().get(name).cloned()
        }

        fn set_text_setting(&self, name: &str, value: &str) {
            self.texts.borrow_mut().insert(name.to_owned(), value.to_owned());
        }

        fn dir(&self) -> PathBuf {
            self.dir_reads.set(self.dir_reads.get() + 1);
            self.dir.clone()
        }

        fn key(&self, _: Provider) -> Result<Option<String>, String> {
            self.key_reads.set(self.key_reads.get() + 1);
            self.key.borrow().clone()
        }

        fn describe(&self, provider: Provider, key: &str, model: &str, image: &Path) -> Result<Described, Problem> {
            let name = image.file_name().unwrap().to_string_lossy().into_owned();
            self.asked.borrow_mut().push((name.clone(), format!("{}:{model}", provider.name()), key.to_owned()));
            let mut answers = self.answers.borrow_mut();
            if answers.is_empty() {
                return Ok(Described { text: format!("Picture of {name}"), model: model.to_owned() });
            }
            answers.remove(0)
        }
    }

    fn silent(_: &str, _: &str) {}

    /// Steps until there's nothing more to do.
    fn run(describer: &mut Describer, fake: &Fake, now: u64) {
        while describer.step(fake, at(now), &silent) {}
    }

    #[test]
    fn nothing_is_described_while_it_is_off() {
        let fake = Fake::new("describer-off");
        fake.file("Screenshot a.png", 2000);
        fake.numbers.borrow_mut().insert(options::DESCRIBE.name.into(), 0);
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert!(fake.asked().is_empty() && fake.key_reads.get() == 0, "no key read, nothing sent");
        // Not on by default either.
        fake.numbers.borrow_mut().remove(options::DESCRIBE.name);
        run(&mut describer, &fake, 5000);
        assert!(fake.asked().is_empty());
    }

    #[test]
    fn nothing_is_described_without_a_key_or_a_start() {
        let fake = Fake::new("describer-no-key");
        fake.file("Screenshot a.png", 2000);
        *fake.key.borrow_mut() = Ok(None);
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert!(fake.asked().is_empty());
        assert_eq!(fake.status(), "", "Settings shows there's no key");
        fake.numbers.borrow_mut().remove(options::DESCRIBE_SINCE);
        *fake.key.borrow_mut() = Ok(Some("sk".into()));
        run(&mut Describer::default(), &fake, 5000);
        assert!(fake.asked().is_empty(), "without a time to count from, the whole library isn't sent");
    }

    #[test]
    fn describes_the_newer_screenshots_oldest_first_and_leaves_the_rest() {
        let fake = Fake::new("describer-newer");
        fake.file("Screenshot old.png", 500);
        fake.file("Screenshot b.png", 3000);
        fake.file("Screenshot a.png", 2000);
        fake.file("Recording c.mp4", 2500);
        fake.file("Screenshot unfinished.png", 4999);
        let described = fake.file("Screenshot done.png", 2200);
        search::record_description(&described, "Already.", "claude-haiku-5-5").unwrap();
        fake.numbers.borrow_mut().insert(options::DESCRIBE_PROVIDER.name.into(), 1);
        fake.texts.borrow_mut().insert(options::DESCRIBE_MODEL.into(), "gpt-6-luna".into());
        fake.texts.borrow_mut().insert(options::DESCRIBE_STATUS.into(), "Earlier trouble".into());
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert_eq!(fake.asked(), ["Screenshot a.png", "Screenshot b.png"], "newer than it was turned on, no recordings, none twice");
        assert_eq!(fake.asked.borrow()[0].1, "OpenAI:gpt-6-luna");
        assert_eq!(fake.asked.borrow()[0].2, "sk-test");
        assert_eq!(fake.key_reads.get(), 1, "the key is read once for them all");
        assert_eq!(fake.status(), "", "going well again");
        let a = search::load(&fake.dir.join("Screenshot a.png"));
        assert_eq!((a.description.as_deref(), a.described_by.as_deref()), (Some("Picture of Screenshot a.png"), Some("gpt-6-luna")));
        assert_eq!(search::load(&fake.dir.join("Screenshot old.png")).description, None);
        assert_eq!(search::load(&described).description.as_deref(), Some("Already."), "left as it was");
        assert_eq!(search::load(&fake.dir.join("Recording c.mp4")).description, None);

        // The one that was still being written is described once it's settled.
        run(&mut describer, &fake, 5003);
        assert_eq!(fake.asked().last().map(String::as_str), Some("Screenshot unfinished.png"));
        run(&mut describer, &fake, 5100);
        assert_eq!(fake.asked().len(), 3, "and nothing is asked twice");
    }

    #[test]
    fn the_default_model_is_the_providers() {
        let fake = Fake::new("describer-default");
        fake.file("Screenshot a.png", 2000);
        fake.file("Screenshot b.png", 2100);
        fake.texts.borrow_mut().insert(options::DESCRIBE_MODEL.into(), "  ".into());
        run(&mut Describer::default(), &fake, 5000);
        assert_eq!(fake.asked.borrow()[0].1, "Anthropic:claude-opus-5-5");
        assert_eq!(chosen_model(Provider::OpenAi, None), "gpt-6.1-sol");
        assert_eq!(chosen_model(Provider::OpenAi, Some(" gpt-6-luna ".into())), "gpt-6-luna");
    }

    #[test]
    fn keeps_what_the_text_reader_and_pushes_wrote() {
        let fake = Fake::new("describer-meta");
        let file = fake.file("Screenshot a.png", 2000);
        search::record_push(&file, "dest", Some("https://example.com/a".into())).unwrap();
        let mut meta = search::load(&file);
        meta.text = Some("Invoice".into());
        meta.app = "Chrome".into();
        search::save(&file, &meta).unwrap();
        run(&mut Describer::default(), &fake, 5000);
        let after = search::load(&file);
        assert_eq!((after.text.as_deref(), after.app.as_str()), (Some("Invoice"), "Chrome"));
        assert_eq!(after.pushed["dest"].link.as_deref(), Some("https://example.com/a"));
        assert!(after.description.is_some());
    }

    #[test]
    fn waits_when_the_provider_is_busy_and_then_carries_on() {
        let fake = Fake::new("describer-busy");
        fake.file("Screenshot a.png", 2000);
        fake.answers.borrow_mut().push(Err(Problem::Later(Duration::from_secs(90), "Anthropic's rate limit was reached".into())));
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert_eq!(fake.asked().len(), 1);
        assert_eq!(fake.status(), "Anthropic's rate limit was reached");
        run(&mut describer, &fake, 5060);
        assert_eq!(fake.asked().len(), 1, "not yet");
        run(&mut describer, &fake, 5090);
        assert_eq!(fake.asked().len(), 2, "as asked, 90 seconds later");
        assert_eq!(fake.status(), "");
        assert!(search::load(&fake.dir.join("Screenshot a.png")).description.is_some());
    }

    #[test]
    fn stops_for_a_bad_key_until_settings_clear_it() {
        let fake = Fake::new("describer-key");
        fake.file("Screenshot a.png", 2000);
        fake.file("Screenshot b.png", 2100);
        fake.answers.borrow_mut().push(Err(Problem::Key("The key wasn't accepted by Anthropic".into())));
        let notices = RefCell::new(Vec::new());
        let notify = |title: &str, text: &str| notices.borrow_mut().push(format!("{title}: {text}"));
        let mut describer = Describer::default();
        while describer.step(&fake, at(5000), &notify) {}
        assert_eq!(fake.status(), "The key wasn't accepted by Anthropic");
        assert_eq!(*notices.borrow(), ["Couldn't describe screenshots: The key wasn't accepted by Anthropic"]);
        for now in [5010, 6000, 90_000] {
            assert!(!describer.step(&fake, at(now), &notify));
        }
        assert_eq!(fake.asked().len(), 1, "no more tries, and no more notices");
        assert_eq!(notices.borrow().len(), 1);

        // Settings clears it when a key is saved.
        *fake.key.borrow_mut() = Ok(Some("sk-new".into()));
        fake.set_text_setting(options::DESCRIBE_STATUS, "");
        while describer.step(&fake, at(90_100), &notify) {}
        assert_eq!(fake.asked().len(), 3, "both are described now");
        assert_eq!(fake.asked.borrow()[1].2, "sk-new", "with the new key");
    }

    #[test]
    fn a_screenshot_the_provider_declines_is_skipped_for_good() {
        let fake = Fake::new("describer-declined");
        fake.file("Screenshot a.png", 2000);
        fake.file("Screenshot b.png", 2100);
        fake.answers.borrow_mut().push(Err(Problem::Skip("Anthropic declined to describe it".into())));
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert_eq!(fake.asked(), ["Screenshot a.png", "Screenshot b.png"], "the next goes on");
        assert_eq!(fake.status(), "", "the next one went well");
        run(&mut describer, &fake, 5100);
        assert_eq!(fake.asked().len(), 2, "the declined one isn't asked again");
        assert_eq!(search::load(&fake.dir.join("Screenshot a.png")).description, None);
    }

    #[test]
    fn a_key_the_credential_store_wont_give_is_tried_again_later() {
        let fake = Fake::new("describer-keyring");
        fake.file("Screenshot a.png", 2000);
        *fake.key.borrow_mut() = Err("No keyring is running".into());
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert_eq!(fake.status(), "No keyring is running");
        run(&mut describer, &fake, 5030);
        assert_eq!(fake.key_reads.get(), 1);
        *fake.key.borrow_mut() = Ok(Some("sk".into()));
        run(&mut describer, &fake, 5000 + KEYRING_WAIT.as_secs());
        assert_eq!(fake.asked().len(), 1);
    }

    #[test]
    fn without_a_key_the_folder_isnt_looked_in() {
        let fake = Fake::new("describer-key-first");
        fake.file("Screenshot a.png", 2000);
        *fake.key.borrow_mut() = Ok(None);
        let mut describer = Describer::default();
        for now in (5000..5060).step_by(5) {
            run(&mut describer, &fake, now);
        }
        assert_eq!((fake.dir_reads.get(), fake.key_reads.get()), (0, 1), "no search data read, and the key looked for once a minute");
        run(&mut describer, &fake, 5060);
        assert_eq!(fake.key_reads.get(), 2);

        // Nor while the keyring is locked, which Settings shows.
        *fake.key.borrow_mut() = Err("The keyring is locked".into());
        run(&mut describer, &fake, 5120);
        run(&mut describer, &fake, 5150);
        assert_eq!((fake.dir_reads.get(), fake.key_reads.get()), (0, 3));
        assert_eq!(fake.status(), "The keyring is locked");

        // Unlocked, it carries on within a minute.
        *fake.key.borrow_mut() = Ok(Some("sk".into()));
        run(&mut describer, &fake, 5180);
        assert_eq!(fake.asked(), ["Screenshot a.png"]);
        assert_eq!(fake.status(), "");
    }

    #[test]
    fn a_keyring_unlocked_again_is_no_longer_shown_as_the_problem() {
        let fake = Fake::new("describer-unlocked");
        *fake.key.borrow_mut() = Err("The keyring is locked".into());
        let mut describer = Describer::default();
        run(&mut describer, &fake, 5000);
        assert_eq!(fake.status(), "The keyring is locked");
        *fake.key.borrow_mut() = Ok(Some("sk".into()));
        run(&mut describer, &fake, 5000 + KEYRING_WAIT.as_secs());
        assert_eq!((fake.status(), fake.asked().len()), (String::new(), 0), "cleared, with nothing to describe");
    }

    fn fake_files(fake: &Fake, names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|name| fake.file(name, 2000)).collect()
    }

    /// Runs `describe_all` and gives the events.
    fn all(files: &[PathBuf], describe: &dyn Fn(&Path) -> Result<Described, Problem>, stop: &AtomicBool) -> Vec<Event> {
        let events = RefCell::new(Vec::new());
        describe_all(files, describe, stop, &|e| events.borrow_mut().push(e));
        events.into_inner()
    }

    fn ok(file: &Path) -> Result<Described, Problem> {
        Ok(Described { text: format!("Picture of {}", file.file_name().unwrap().to_string_lossy()), model: "m".into() })
    }

    #[test]
    fn the_library_is_described_a_screenshot_at_a_time_without_recordings() {
        let fake = Fake::new("describer-library");
        let files = fake_files(&fake, &["Screenshot a.png", "Screenshot b.jpg", "Recording c.mp4", "Recording d.gif"]);
        search::record_description(&files[1], "Done.", "m").unwrap();
        assert_eq!(super::super::library_files(&fake.dir).len(), 4);
        let todo = undescribed(&fake.dir);
        assert_eq!(todo, [files[0].clone()], "no recordings, none described");
        let events = all(&todo, &ok, &AtomicBool::new(false));
        assert_eq!(events, [Event::Described(files[0].clone()), Event::Finished(None)]);
        assert_eq!(search::load(&files[0]).description.as_deref(), Some("Picture of Screenshot a.png"));
        assert!(undescribed(&fake.dir).is_empty());
    }

    #[test]
    fn the_library_newest_goes_first() {
        let fake = Fake::new("describer-order");
        let old = fake.file("Screenshot a.png", 100);
        let new = fake.file("Screenshot b.png", 300);
        let middle = fake.file("Screenshot c.png", 200);
        assert_eq!(undescribed(&fake.dir), [new, middle, old]);
    }

    #[test]
    fn stop_ends_it_where_it_is() {
        let fake = Fake::new("describer-stop");
        let files = fake_files(&fake, &["Screenshot a.png", "Screenshot b.png", "Screenshot c.png"]);
        let stop = AtomicBool::new(false);
        let events = all(
            &files,
            &|file| {
                if file.ends_with("Screenshot b.png") {
                    stop.store(true, Ordering::Relaxed);
                }
                ok(file)
            },
            &stop,
        );
        assert_eq!(events, [Event::Described(files[0].clone()), Event::Described(files[1].clone()), Event::Finished(None)]);
        assert_eq!(search::load(&files[2]).description, None);
    }

    #[test]
    fn a_declined_screenshot_is_reported_and_the_rest_go_on() {
        let fake = Fake::new("describer-library-declined");
        let files = fake_files(&fake, &["Screenshot a.png", "Screenshot b.png"]);
        let events = all(&files, &|file| if file.ends_with("Screenshot a.png") { Err(Problem::Skip("declined".into())) } else { ok(file) }, &AtomicBool::new(false));
        assert_eq!(events, [Event::Skipped(files[0].clone(), "declined".into()), Event::Described(files[1].clone()), Event::Finished(None)]);
    }

    #[test]
    fn a_bad_key_ends_it_and_says_why() {
        let fake = Fake::new("describer-library-key");
        let files = fake_files(&fake, &["Screenshot a.png", "Screenshot b.png"]);
        let asked = Cell::new(0);
        let events = all(
            &files,
            &|_| {
                asked.set(asked.get() + 1);
                Err(Problem::Key("Your Anthropic account is out of credit".into()))
            },
            &AtomicBool::new(false),
        );
        assert_eq!(events, [Event::Finished(Some("Your Anthropic account is out of credit".into()))]);
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn a_busy_provider_is_tried_three_times() {
        let fake = Fake::new("describer-library-busy");
        let files = fake_files(&fake, &["Screenshot a.png", "Screenshot b.png"]);
        let tries = Cell::new(0);
        let events = all(
            &files,
            &|file| {
                tries.set(tries.get() + 1);
                match tries.get() {
                    1 | 2 => Err(Problem::Later(Duration::ZERO, "busy".into())),
                    _ if file.ends_with("Screenshot a.png") => ok(file),
                    _ => Err(Problem::Later(Duration::ZERO, "busy".into())),
                }
            },
            &AtomicBool::new(false),
        );
        // The first goes through on its third try; the second fails all three.
        assert_eq!(events, [Event::Described(files[0].clone()), Event::Finished(Some("busy".into()))]);
        assert_eq!(tries.get(), 6);
    }

    #[test]
    fn already_described_by_the_time_its_turn_comes() {
        let fake = Fake::new("describer-library-race");
        let files = fake_files(&fake, &["Screenshot a.png"]);
        search::record_description(&files[0], "From the background.", "m").unwrap();
        let events = all(&files, &|_| panic!("asked again"), &AtomicBool::new(false));
        assert_eq!(events, [Event::Finished(None)]);
    }

    #[test]
    fn the_cost_of_the_library_adds_up_the_pictures() {
        let fake = Fake::new("describer-estimate");
        let a = fake.dir.join("a.png");
        image::RgbaImage::new(1920, 1080).save(&a).unwrap();
        let unreadable = fake.file("b.png", 2000);
        let one = describe::cost(Provider::Anthropic, "claude-opus-5-5", (1920, 1080)).unwrap();
        let both = estimate(Provider::Anthropic, "claude-opus-5-5", &[a.clone(), unreadable]).unwrap();
        assert!((both - 2.0 * one).abs() < 1e-9, "a picture that can't be measured counts as a typical one");
        assert_eq!(estimate(Provider::Anthropic, "claude-unknown", &[a]), None);
        assert_eq!(estimate(Provider::Anthropic, "claude-opus-5-5", &[]), Some(0.0));
    }
}
