//! RSnip's keyboard shortcuts in GNOME's own settings, set from Settings → Shortcuts on Linux. Each
//! action's key is a custom shortcut (`org.gnome.settings-daemon.plugins.media-keys`), which GNOME
//! runs on X11 and Wayland alike and shows in its Keyboard settings; Print Screen can be taken from
//! GNOME's screenshot tool (`org.gnome.shell.keybindings show-screenshot-ui`) and given back.
//!
//! It all goes through `gsettings`, run by the caller (`Env::gsettings`, a stand-in in tests). Only
//! RSnip's own custom shortcuts, those whose command runs RSnip, are changed or removed; the others
//! are only read, to tell them apart.

use std::collections::BTreeMap;

use eframe::egui::Key;

use crate::shortcuts::{Action, COUNT};

pub const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
pub const LIST: &str = "custom-keybindings";
/// The relocatable schema each custom shortcut has, at its own path under `BASE`.
pub const CUSTOM: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
pub const BASE: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";
pub const SHELL: &str = "org.gnome.shell.keybindings";
/// The keys that open GNOME's screenshot tool.
pub const SCREENSHOT_UI: &str = "show-screenshot-ui";
/// Print Screen, as GNOME names it.
pub const PRINT: &str = "Print";
/// Where the .deb and .rpm install RSnip (see `platform::package`): its commands are RSnip's
/// whichever copy is running.
const PACKAGED: &str = "/usr/bin/rsnip";

/// Runs `gsettings` with these arguments: what it printed, or why it failed.
pub type Run<'a> = &'a dyn Fn(&[&str]) -> Result<String, String>;

/// The desktop is GNOME (`$XDG_CURRENT_DESKTOP` is a list like `ubuntu:GNOME`).
pub fn is_gnome(current_desktop: &str) -> bool {
    current_desktop.split(':').any(|d| d.eq_ignore_ascii_case("GNOME"))
}

/// The command-line flag that runs `action` (see `main::action`).
pub fn flag(action: Action) -> &'static str {
    match action {
        Action::Region => "--region",
        Action::Full => "--full",
        Action::Last => "--last",
        Action::Scroll => "--scroll",
        Action::Record => "--record",
        Action::Text => "--text",
    }
}

/// The command a shortcut runs for `action`: `program`, quoted if it needs to be, and its flag.
pub fn command(program: &str, action: Action) -> String {
    let plain = !program.is_empty() && program.chars().all(|c| c.is_ascii_alphanumeric() || "/._+-".contains(c));
    let program = if plain { program.to_owned() } else { format!("'{}'", program.replace('\'', r"'\''")) };
    format!("{program} {}", flag(action))
}

/// The action `command` runs, if it's one of RSnip's: `program`, the package's or plain `rsnip`,
/// with just that action's flag.
fn action_of(command: &str, program: &str) -> Option<Action> {
    let (first, rest) = first_word(command);
    if first != program && first != PACKAGED && first != "rsnip" {
        return None;
    }
    Action::ALL.into_iter().find(|&a| rest.trim() == flag(a))
}

/// A command's first word, unquoted as GLib's shell parsing does it, and the rest.
fn first_word(command: &str) -> (String, &str) {
    let command = command.trim_start();
    let mut word = String::new();
    let mut quote = None;
    let mut chars = command.char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => return (word, &command[i..]),
            (None, '\'' | '"') => quote = Some(c),
            (None | Some('"'), '\\') => word.extend(chars.next().map(|(_, c)| c)),
            (Some(q), c) if c == q => quote = None,
            (_, c) => word.push(c),
        }
    }
    (word, "")
}

/// A GVariant string as `gsettings` prints it (`'Print'`, or `"it's"`), unquoted.
fn parse_string(text: &str) -> Option<String> {
    let (s, rest) = string_at(text.trim())?;
    rest.trim().is_empty().then_some(s)
}

/// The GVariant string at the start of `text`, and what follows it.
fn string_at(text: &str) -> Option<(String, &str)> {
    let quote = text.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let mut out = String::new();
    let mut chars = text.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                c => out.push(c),
            },
            c if c == quote => return Some((out, &text[i + 1..])),
            c => out.push(c),
        }
    }
    None
}

/// `text` as a GVariant string, for `gsettings set`.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'"))
}

/// A GVariant list of strings as `gsettings` prints it: `['a', 'b']`, or `@as []` when empty.
fn parse_list(text: &str) -> Option<Vec<String>> {
    let text = text.trim();
    let mut rest = text.strip_prefix("@as").unwrap_or(text).trim_start().strip_prefix('[')?;
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            return after.trim().is_empty().then_some(out);
        }
        if !out.is_empty() {
            rest = rest.strip_prefix(',')?.trim_start();
        }
        let (s, after) = string_at(rest)?;
        out.push(s);
        rest = after;
    }
}

/// `items` as a GVariant list of strings.
fn list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|i| quote(i)).collect::<Vec<_>>().join(", "))
}

/// The modifiers held with a key.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

/// A key pressed in the Settings window, with the modifiers held, as a GNOME accelerator
/// (`<Shift><Control>4`), or why it can't be a shortcut. Letters are as the keyboard's layout has
/// them; other keys are taken where they are, so Shift+4 is `<Shift>4` rather than a dollar sign.
pub fn accelerator(key: Key, physical: Option<Key>, mods: Mods) -> Result<String, &'static str> {
    let letter = |k: Key| matches!(keysym(k), Some(name) if name.len() == 1 && name.as_bytes()[0].is_ascii_lowercase());
    let key = if letter(key) { key } else { physical.filter(|&p| keysym(p).is_some()).unwrap_or(key) };
    let Some(name) = keysym(key) else { return Err("That key can't be a shortcut; try another.") };
    let function_key = name.len() > 1 && name.starts_with('F') && name[1..].bytes().all(|b| b.is_ascii_digit());
    if !(mods.ctrl || mods.alt || mods.super_key || function_key) {
        return Err("Hold Ctrl, Alt or Super with that key; only function keys work on their own.");
    }
    Ok(accelerator_name(mods, name))
}

/// GNOME's (GTK's) way of writing a key with modifiers.
fn accelerator_name(mods: Mods, key: &str) -> String {
    let held = [(mods.shift, "<Shift>"), (mods.ctrl, "<Control>"), (mods.alt, "<Alt>"), (mods.super_key, "<Super>")];
    held.into_iter().filter(|h| h.0).map(|h| h.1).chain([key]).collect()
}

/// The X key name (keysym) for an egui key, as GNOME's accelerators use them.
fn keysym(key: Key) -> Option<&'static str> {
    use Key::*;
    let letters = "abcdefghijklmnopqrstuvwxyz";
    let digits = "0123456789";
    let one = |set: &'static str, i: usize| Some(&set[i..i + 1]);
    Some(match key {
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U | V | W | X | Y | Z => {
            return one(letters, [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z].iter().position(|&k| k == key)?);
        }
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => {
            return one(digits, [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9].iter().position(|&k| k == key)?);
        }
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        F21 => "F21",
        F22 => "F22",
        F23 => "F23",
        F24 => "F24",
        ArrowDown => "Down",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        ArrowUp => "Up",
        Tab => "Tab",
        Backspace => "BackSpace",
        Enter => "Return",
        Space => "space",
        Insert => "Insert",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "Page_Up",
        PageDown => "Page_Down",
        Comma => "comma",
        Backslash => "backslash",
        Slash => "slash",
        Backtick => "grave",
        Minus => "minus",
        Period => "period",
        Equals => "equal",
        Semicolon => "semicolon",
        Quote => "apostrophe",
        OpenBracket => "bracketleft",
        CloseBracket => "bracketright",
        Colon => "colon",
        Pipe => "bar",
        Questionmark => "question",
        Exclamationmark => "exclam",
        OpenCurlyBracket => "braceleft",
        CloseCurlyBracket => "braceright",
        Plus => "plus",
        _ => return None,
    })
}

/// An accelerator's modifiers and key, however it's written (`<Ctrl>`, `<Primary>`, `<Mod1>`, ...).
fn parse_accelerator(accel: &str) -> (Mods, &str) {
    let mut mods = Mods::default();
    let mut rest = accel.trim();
    while let Some((name, after)) = rest.strip_prefix('<').and_then(|r| r.split_once('>')) {
        match name.to_ascii_lowercase().as_str() {
            "shift" => mods.shift = true,
            "control" | "ctrl" | "ctl" | "primary" => mods.ctrl = true,
            "alt" | "mod1" => mods.alt = true,
            "super" | "mod4" | "meta" | "hyper" => mods.super_key = true,
            _ => {}
        }
        rest = after;
    }
    (mods, rest)
}

/// An accelerator written one way, to compare with others.
pub fn normalize(accel: &str) -> String {
    let (mods, key) = parse_accelerator(accel);
    accelerator_name(mods, &key.to_lowercase())
}

/// The two accelerators are the same key.
pub fn same(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

/// An accelerator for people: `Ctrl+Shift+4`, `PrtScn`, written as `shortcuts::fallback_name`
/// writes Windows' shortcuts.
pub fn label(accel: &str) -> String {
    let (mods, key) = parse_accelerator(accel);
    let names = [("Print", "PrtScn"), ("BackSpace", "Backspace"), ("Return", "Enter"), ("space", "Space"), ("Page_Up", "Page Up"), ("Page_Down", "Page Down")];
    let symbols = [
        ("comma", ","),
        ("backslash", "\\"),
        ("slash", "/"),
        ("grave", "`"),
        ("minus", "-"),
        ("period", "."),
        ("equal", "="),
        ("semicolon", ";"),
        ("apostrophe", "'"),
        ("bracketleft", "["),
        ("bracketright", "]"),
        ("colon", ":"),
        ("bar", "|"),
        ("question", "?"),
        ("exclam", "!"),
        ("braceleft", "{"),
        ("braceright", "}"),
        ("plus", "+"),
    ];
    let named = names.iter().chain(&symbols).find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, shown)| shown.to_string());
    let key = named.unwrap_or_else(|| if key.chars().count() == 1 { key.to_uppercase() } else { key.replace('_', " ") });
    let held = [(mods.ctrl, "Ctrl"), (mods.alt, "Alt"), (mods.shift, "Shift"), (mods.super_key, "Super")];
    held.into_iter().filter(|h| h.0).map(|h| h.1.to_owned()).chain([key]).collect::<Vec<_>>().join("+")
}

/// RSnip's shortcuts as GNOME has them.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct State {
    /// Each action's key (an accelerator like `<Shift><Control>4`), if it has one, by
    /// `Action::index`.
    pub keys: [Option<String>; COUNT],
    /// GNOME's screenshot tool has Print Screen.
    pub gnome_has_print: bool,
}

impl State {
    /// The action that has Print Screen, if one does.
    pub fn print_screen(&self) -> Option<Action> {
        Action::ALL.into_iter().find(|a| self.keys[a.index()].as_deref().is_some_and(|k| same(k, PRINT)))
    }
}

/// One of RSnip's custom shortcuts.
struct Entry {
    path: String,
    action: Action,
    binding: String,
}

fn get(run: Run, schema: &str, key: &str) -> Result<String, String> {
    run(&["get", schema, key]).map(|out| out.trim().to_owned())
}

fn set_string(run: Run, path: &str, key: &str, value: &str) -> Result<(), String> {
    run(&["set", &format!("{CUSTOM}:{path}"), key, &quote(value)]).map(drop)
}

/// The custom shortcuts' paths, and which of them are RSnip's (`program`'s, see `action_of`).
fn entries(run: Run, program: &str) -> Result<(Vec<String>, Vec<Entry>), String> {
    let text = get(run, MEDIA_KEYS, LIST)?;
    let paths = parse_list(&text).ok_or_else(|| format!("GNOME's list of custom shortcuts reads as {text}"))?;
    let mut ours = Vec::new();
    for path in &paths {
        let schema = format!("{CUSTOM}:{path}");
        // One that can't be read is someone else's to sort out.
        let Some(action) = get(run, &schema, "command").ok().and_then(|c| parse_string(&c)).and_then(|c| action_of(&c, program)) else { continue };
        let binding = get(run, &schema, "binding").ok().and_then(|b| parse_string(&b)).unwrap_or_default();
        ours.push(Entry { path: path.clone(), action, binding });
    }
    Ok((paths, ours))
}

/// RSnip's shortcuts, as GNOME's settings have them.
pub fn read(run: Run, program: &str) -> Result<State, String> {
    let (_, ours) = entries(run, program)?;
    let mut state = State::default();
    for entry in ours.into_iter().filter(|e| !e.binding.is_empty()) {
        state.keys[entry.action.index()].get_or_insert(entry.binding);
    }
    let screenshot_keys = get(run, SHELL, SCREENSHOT_UI).ok().and_then(|keys| parse_list(&keys)).unwrap_or_default();
    state.gnome_has_print = screenshot_keys.iter().any(|k| same(k, PRINT));
    Ok(state)
}

/// Gives `accel` to `action`, taking it from whichever other action of RSnip's had it (returned).
/// An action without a custom shortcut gets a new one, `rsnipN`, at the end of the list.
pub fn set(run: Run, program: &str, action: Action, accel: &str) -> Result<Option<Action>, String> {
    let (paths, ours) = entries(run, program)?;
    let taken: Vec<&Entry> = ours.iter().filter(|e| e.action != action && same(&e.binding, accel)).collect();
    let mut kept: Vec<String> = paths.iter().filter(|p| !taken.iter().any(|e| &e.path == *p)).cloned().collect();
    match ours.iter().find(|e| e.action == action) {
        Some(entry) => set_string(run, &entry.path, "binding", accel)?,
        None => {
            let path = (0..).map(|n| format!("{BASE}rsnip{n}/")).find(|p| !paths.contains(p)).unwrap_or_default();
            set_string(run, &path, "name", &format!("RSnip: {}", action.label()))?;
            set_string(run, &path, "command", &command(program, action))?;
            set_string(run, &path, "binding", accel)?;
            // Listed last, so GNOME finds it whole.
            kept.push(path);
        }
    }
    if kept != paths {
        run(&["set", MEDIA_KEYS, LIST, &list(&kept)])?;
    }
    forget(run, &taken);
    Ok(taken.first().map(|e| e.action))
}

/// Removes `action`'s custom shortcuts.
pub fn clear(run: Run, program: &str, action: Action) -> Result<(), String> {
    remove(run, program, |e| e.action == action)
}

/// Removes RSnip's custom shortcuts that `which` picks.
fn remove(run: Run, program: &str, which: impl Fn(&Entry) -> bool) -> Result<(), String> {
    let (paths, ours) = entries(run, program)?;
    let gone: Vec<&Entry> = ours.iter().filter(|e| which(e)).collect();
    if gone.is_empty() {
        return Ok(());
    }
    let kept: Vec<String> = paths.into_iter().filter(|p| !gone.iter().any(|e| &e.path == p)).collect();
    run(&["set", MEDIA_KEYS, LIST, &list(&kept)])?;
    forget(run, &gone);
    Ok(())
}

/// Resets what custom shortcuts no longer listed had, so nothing of them is left behind.
fn forget(run: Run, entries: &[&Entry]) {
    for entry in entries {
        for key in ["name", "command", "binding"] {
            // They're out of the list already, so GNOME doesn't run them either way.
            let _ = run(&["reset", &format!("{CUSTOM}:{}", entry.path), key]);
        }
    }
}

/// Gives Print Screen to `action`, taking it from GNOME's screenshot tool; returns that tool's keys
/// from before, to put back (`give_back_print_screen`), if they were changed.
pub fn take_print_screen(run: Run, program: &str, action: Action) -> Result<Option<String>, String> {
    let before = get(run, SHELL, SCREENSHOT_UI)?;
    let keys = parse_list(&before).ok_or_else(|| format!("GNOME's screenshot keys read as {before}"))?;
    let kept: Vec<String> = keys.iter().filter(|k| !same(k, PRINT)).cloned().collect();
    let changed = kept.len() != keys.len();
    if changed {
        run(&["set", SHELL, SCREENSHOT_UI, &list(&kept)])?;
    }
    set(run, program, action, PRINT)?;
    Ok(changed.then_some(before))
}

/// Takes Print Screen from RSnip's actions, and gives GNOME's screenshot tool back its keys from
/// before (`saved`, as `take_print_screen` returned them), if there are.
pub fn give_back_print_screen(run: Run, program: &str, saved: Option<&str>) -> Result<(), String> {
    remove(run, program, |e| same(&e.binding, PRINT))?;
    if let Some(saved) = saved.filter(|s| parse_list(s).is_some()) {
        run(&["set", SHELL, SCREENSHOT_UI, saved])?;
    }
    Ok(())
}

/// A stand-in for `gsettings` and the settings it changes, for tests: the schemas RSnip uses, with
/// GNOME's defaults, and a record of each call.
#[derive(Default, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Fake {
    /// GNOME's schemas are installed; otherwise every call fails, as on a desktop without them.
    pub gnome: bool,
    /// What's been set, by schema (with its path, for a custom shortcut) and key, as GVariant text.
    pub values: BTreeMap<(String, String), String>,
    /// Each call's arguments.
    pub calls: Vec<Vec<String>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Fake {
    pub fn run(&mut self, args: &[&str]) -> Result<String, String> {
        self.calls.push(args.iter().map(|a| a.to_string()).collect());
        if !self.gnome {
            return Err(format!("No such schema “{}”", args.get(1).unwrap_or(&"")));
        }
        let default = |schema: &str, key: &str| match (schema, key) {
            (MEDIA_KEYS, LIST) => Ok("@as []".to_owned()),
            (SHELL, SCREENSHOT_UI) => Ok("['Print']".to_owned()),
            (s, "name" | "command" | "binding") if s.strip_prefix(CUSTOM).is_some_and(|p| p.starts_with(':')) => Ok("''".to_owned()),
            _ => Err(format!("No such key “{key}”")),
        };
        match args {
            ["get", schema, key] => match self.values.get(&(schema.to_string(), key.to_string())) {
                Some(value) => Ok(format!("{value}\n")),
                None => default(schema, key).map(|v| format!("{v}\n")),
            },
            ["set", schema, key, value] => {
                default(schema, key)?;
                self.values.insert((schema.to_string(), key.to_string()), value.to_string());
                Ok(String::new())
            }
            ["reset", schema, key] => {
                self.values.remove(&(schema.to_string(), key.to_string()));
                Ok(String::new())
            }
            _ => Err(format!("gsettings can't {args:?}")),
        }
    }

    /// Adds a custom shortcut at `path` (a name under `BASE`, like `custom0`), as GNOME's Keyboard
    /// settings would.
    pub fn add_custom(&mut self, name: &str, title: &str, command: &str, binding: &str) {
        let path = format!("{BASE}{name}/");
        let mut paths = self.list();
        paths.push(path.clone());
        self.values.insert((MEDIA_KEYS.into(), LIST.into()), list(&paths));
        for (key, value) in [("name", title), ("command", command), ("binding", binding)] {
            self.values.insert((format!("{CUSTOM}:{path}"), key.into()), quote(value));
        }
    }

    /// The custom shortcuts' paths.
    pub fn list(&self) -> Vec<String> {
        self.values.get(&(MEDIA_KEYS.into(), LIST.into())).and_then(|l| parse_list(l)).unwrap_or_default()
    }

    /// What the custom shortcut `name` (like `rsnip0`) has for `key`, unquoted.
    pub fn custom(&self, name: &str, key: &str) -> Option<String> {
        self.values.get(&(format!("{CUSTOM}:{BASE}{name}/"), key.into())).and_then(|v| parse_string(v))
    }

    /// GNOME's screenshot tool's keys.
    pub fn screenshot_keys(&self) -> Vec<String> {
        self.values.get(&(SHELL.into(), SCREENSHOT_UI.into())).map_or_else(|| vec![PRINT.to_owned()], |v| parse_list(v).unwrap_or_default())
    }

    /// The calls that changed something at `path` (a name under `BASE`).
    pub fn changes_to(&self, name: &str) -> Vec<&Vec<String>> {
        let at = format!("{CUSTOM}:{BASE}{name}/");
        self.calls.iter().filter(|c| c[0] != "get" && c.get(1) == Some(&at)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const PROGRAM: &str = "/opt/RSnip App/rsnip";

    fn mods(shift: bool, ctrl: bool, alt: bool, super_key: bool) -> Mods {
        Mods { shift, ctrl, alt, super_key }
    }

    #[test]
    fn keys_become_gnome_accelerators() {
        let ctrl = mods(false, true, false, false);
        assert_eq!(accelerator(Key::Num4, Some(Key::Num4), mods(true, true, false, false)), Ok("<Shift><Control>4".into()));
        assert_eq!(accelerator(Key::S, Some(Key::S), mods(false, false, false, true)), Ok("<Super>s".into()));
        assert_eq!(accelerator(Key::R, None, mods(true, true, true, true)), Ok("<Shift><Control><Alt><Super>r".into()));
        assert_eq!(accelerator(Key::F5, Some(Key::F5), Mods::default()), Ok("F5".into()), "function keys work alone");
        assert_eq!(accelerator(Key::F12, None, mods(true, false, false, false)), Ok("<Shift>F12".into()));
        assert_eq!(accelerator(Key::PageDown, None, ctrl), Ok("<Control>Page_Down".into()));
        assert_eq!(accelerator(Key::Slash, None, ctrl), Ok("<Control>slash".into()));
        // Shift+/ reads as a question mark, but the key is the slash.
        assert_eq!(accelerator(Key::Questionmark, Some(Key::Slash), mods(true, true, false, false)), Ok("<Shift><Control>slash".into()));
        // A letter is the layout's (Z on a German keyboard is where Y is on an American one).
        assert_eq!(accelerator(Key::Z, Some(Key::Y), ctrl), Ok("<Control>z".into()));
        assert!(accelerator(Key::A, Some(Key::A), Mods::default()).is_err(), "typing keys need Ctrl, Alt or Super");
        assert!(accelerator(Key::Num4, Some(Key::Num4), mods(true, false, false, false)).is_err(), "Shift isn't enough");
        assert!(accelerator(Key::Copy, None, ctrl).is_err(), "no GNOME name");
    }

    #[test]
    fn accelerators_are_shown_and_compared_however_theyre_written() {
        assert_eq!(label("<Shift><Control>4"), "Ctrl+Shift+4");
        assert_eq!(label("<Super>s"), "Super+S");
        assert_eq!(label("Print"), "PrtScn");
        assert_eq!(label("<Primary><Alt>Page_Up"), "Ctrl+Alt+Page Up");
        assert_eq!(label("<Control>slash"), "Ctrl+/");
        assert!(same("<Ctrl><Shift>S", "<Shift><Control>s"));
        assert!(same("print", PRINT));
        assert!(!same("<Shift>Print", PRINT));
    }

    #[test]
    fn gvariant_text_is_read_and_written() {
        assert_eq!(parse_list("@as []"), Some(vec![]));
        assert_eq!(parse_list("[]"), Some(vec![]));
        assert_eq!(parse_list("['/a/', '/b/']\n"), Some(vec!["/a/".into(), "/b/".into()]));
        assert_eq!(parse_list(r#"["it's", 'x\'y']"#), Some(vec!["it's".into(), "x'y".into()]));
        assert_eq!(parse_list("['a' 'b']"), None);
        assert_eq!(parse_string("'Print'\n"), Some("Print".into()));
        assert_eq!(parse_string(&quote(r"a 'b' \c")), Some(r"a 'b' \c".into()));
        assert_eq!(parse_list(&list(&["it's".into(), "/b/".into()])), Some(vec!["it's".into(), "/b/".into()]));
    }

    #[test]
    fn commands_run_this_program_and_are_told_apart_from_others() {
        assert_eq!(command("/usr/bin/rsnip", Action::Region), "/usr/bin/rsnip --region");
        assert_eq!(command(PROGRAM, Action::Record), "'/opt/RSnip App/rsnip' --record");
        assert_eq!(command("/home/o'neil/rsnip", Action::Text), r"'/home/o'\''neil/rsnip' --text");
        assert_eq!(action_of(&command(PROGRAM, Action::Record), PROGRAM), Some(Action::Record));
        assert_eq!(action_of(&command("/home/o'neil/rsnip", Action::Text), "/home/o'neil/rsnip"), Some(Action::Text));
        assert_eq!(action_of("rsnip --scroll", PROGRAM), Some(Action::Scroll), "set up by hand");
        assert_eq!(action_of("/usr/bin/rsnip  --full ", PROGRAM), Some(Action::Full), "the package's");
        assert_eq!(action_of("rsnip --record-screen", PROGRAM), None, "not one of the actions");
        assert_eq!(action_of("rsnip --delay 5", PROGRAM), None);
        assert_eq!(action_of("/home/a/rsnip --region", PROGRAM), None, "another copy's");
        assert_eq!(action_of("flameshot gui", PROGRAM), None);
    }

    #[test]
    fn only_gnome_is_gnome() {
        assert!(is_gnome("ubuntu:GNOME"));
        assert!(is_gnome("GNOME"));
        assert!(!is_gnome("KDE"));
        assert!(!is_gnome("GNOME-Flashback-ish"));
        assert!(!is_gnome(""));
    }

    /// A fake GNOME with someone else's custom shortcut, and a way to run gsettings on it.
    fn gnome() -> RefCell<Fake> {
        let mut fake = Fake { gnome: true, ..Default::default() };
        fake.add_custom("custom0", "Terminal", "gnome-terminal", "<Control><Alt>t");
        RefCell::new(fake)
    }

    fn run(fake: &RefCell<Fake>) -> impl Fn(&[&str]) -> Result<String, String> + '_ {
        move |args| fake.borrow_mut().run(args)
    }

    #[test]
    fn setting_a_key_adds_rsnips_own_custom_shortcut_and_leaves_the_others() {
        let fake = gnome();
        assert_eq!(set(&run(&fake), PROGRAM, Action::Region, "<Shift><Control>4"), Ok(None));
        {
            let f = fake.borrow();
            assert_eq!(f.list(), [format!("{BASE}custom0/"), format!("{BASE}rsnip0/")]);
            assert_eq!(f.custom("rsnip0", "name").as_deref(), Some("RSnip: Capture region"));
            assert_eq!(f.custom("rsnip0", "command").as_deref(), Some("'/opt/RSnip App/rsnip' --region"));
            assert_eq!(f.custom("rsnip0", "binding").as_deref(), Some("<Shift><Control>4"));
            // The new one's keys were set before it was listed.
            let sets: Vec<&str> = f.calls.iter().filter(|c| c[0] == "set").map(|c| c[2].as_str()).collect();
            assert_eq!(sets, ["name", "command", "binding", LIST]);
        }
        assert_eq!(set(&run(&fake), PROGRAM, Action::Record, "<Super>r"), Ok(None));
        assert_eq!(fake.borrow().custom("rsnip1", "binding").as_deref(), Some("<Super>r"));
        // A new key for an action changes its own shortcut.
        assert_eq!(set(&run(&fake), PROGRAM, Action::Region, "<Control>F5"), Ok(None));
        assert_eq!(fake.borrow().list().len(), 3);
        let state = read(&run(&fake), PROGRAM).unwrap();
        assert_eq!(state.keys[Action::Region.index()].as_deref(), Some("<Control>F5"));
        assert_eq!(state.keys[Action::Record.index()].as_deref(), Some("<Super>r"));
        assert!(state.gnome_has_print, "GNOME's default");

        clear(&run(&fake), PROGRAM, Action::Region).unwrap();
        let f = fake.borrow();
        assert_eq!(f.list(), [format!("{BASE}custom0/"), format!("{BASE}rsnip1/")]);
        assert_eq!(f.custom("rsnip0", "binding"), None, "reset");
        assert!(f.changes_to("custom0").is_empty(), "someone else's is never changed");
        assert_eq!(f.custom("custom0", "binding").as_deref(), Some("<Control><Alt>t"));
    }

    #[test]
    fn a_key_another_action_has_moves_and_hand_made_ones_count_as_rsnips() {
        let fake = gnome();
        fake.borrow_mut().add_custom("custom1", "Record", "rsnip --record", "<Shift><Control>5");
        let state = read(&run(&fake), PROGRAM).unwrap();
        assert_eq!(state.keys[Action::Record.index()].as_deref(), Some("<Shift><Control>5"), "it runs RSnip");
        assert_eq!(set(&run(&fake), PROGRAM, Action::Full, "<Control><Shift>5"), Ok(Some(Action::Record)));
        let f = fake.borrow();
        assert_eq!(f.list(), [format!("{BASE}custom0/"), format!("{BASE}rsnip0/")]);
        assert_eq!(f.custom("rsnip0", "command").as_deref(), Some("'/opt/RSnip App/rsnip' --full"));
        assert!(f.changes_to("custom0").is_empty());
    }

    #[test]
    fn print_screen_is_taken_from_gnomes_screenshot_tool_and_given_back() {
        let fake = gnome();
        fake.borrow_mut().values.insert((SHELL.into(), SCREENSHOT_UI.into()), "['Print', '<Super>p']".into());
        let saved = take_print_screen(&run(&fake), PROGRAM, Action::Region).unwrap();
        assert_eq!(saved.as_deref(), Some("['Print', '<Super>p']"));
        assert_eq!(fake.borrow().screenshot_keys(), ["<Super>p"], "only Print Screen is taken");
        let state = read(&run(&fake), PROGRAM).unwrap();
        assert_eq!(state.print_screen(), Some(Action::Region));
        assert!(!state.gnome_has_print);

        // Taking it again, for another action, has nothing more to save.
        assert_eq!(take_print_screen(&run(&fake), PROGRAM, Action::Text), Ok(None));
        assert_eq!(read(&run(&fake), PROGRAM).unwrap().print_screen(), Some(Action::Text));

        give_back_print_screen(&run(&fake), PROGRAM, saved.as_deref()).unwrap();
        assert_eq!(fake.borrow().screenshot_keys(), ["Print", "<Super>p"]);
        let state = read(&run(&fake), PROGRAM).unwrap();
        assert_eq!(state.print_screen(), None);
        assert_eq!(state.keys, State::default().keys, "RSnip has no shortcuts left");
        assert!(fake.borrow().changes_to("custom0").is_empty());
    }

    #[test]
    fn without_gnomes_schemas_nothing_can_be_read() {
        let fake = RefCell::new(Fake::default());
        assert!(read(&run(&fake), PROGRAM).is_err());
        assert!(set(&run(&fake), PROGRAM, Action::Region, "<Super>r").is_err());
    }
}
