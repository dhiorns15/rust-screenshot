//! The Redact tool's Auto-redact: finds email addresses, IP addresses, keys and tokens, card
//! numbers and phone numbers in the image's text and covers each with a solid box.

use std::net::{Ipv4Addr, Ipv6Addr};

use eframe::egui::{self, Color32, Rect};

use super::{Editor, TextStatus};
use crate::gallery::doc::Shape;
use crate::gallery::words::Word;

/// What Auto-redact is up to, for the options row.
pub(super) enum AutoRedact {
    /// Waiting for the image's text to be read.
    Waiting,
    /// What it did, said while the image is still as it was after.
    Said { version: u64, text: String },
}

impl Editor {
    /// Starts Auto-redact, reading the image's text first if it hasn't been.
    pub fn start_auto_redact(&mut self) {
        self.auto_redact = Some(AutoRedact::Waiting);
    }

    pub fn auto_redacting(&self) -> bool {
        matches!(self.auto_redact, Some(AutoRedact::Waiting))
    }

    /// What Auto-redact just did, while that's still true of the image.
    pub fn auto_redact_note(&self) -> Option<&str> {
        match &self.auto_redact {
            Some(AutoRedact::Said { version, text }) if *version == self.doc.version => Some(text),
            _ => None,
        }
    }

    /// Covers what Auto-redact found once the text has been read: all in one undo step.
    pub(super) fn finish_auto_redact(&mut self, ctx: &egui::Context, color: Color32) {
        if !self.auto_redacting() {
            return;
        }
        let text = match self.text_status() {
            TextStatus::Reading => return,
            TextStatus::Unavailable => {
                self.auto_redact = None;
                return;
            }
            TextStatus::Failed(e) => e,
            TextStatus::Found { .. } => {
                // A little extra, as the boxes the text was found in are tight around it.
                let image = Rect::from_min_size(egui::Pos2::ZERO, self.doc.size());
                let boxes: Vec<_> = find(self.words()).into_iter().map(|r| r.expand(2.0).intersect(image)).collect();
                match boxes.len() {
                    0 => "Nothing to redact".to_owned(),
                    n => {
                        self.doc.add_all(boxes.into_iter().map(|rect| Shape::Redact { rect, color }));
                        let them = if n == 1 { "it" } else { "them" };
                        format!("Redacted {n}: undo brings {them} back")
                    }
                }
            }
        };
        self.auto_redact = Some(AutoRedact::Said { version: self.doc.version, text });
        ctx.request_repaint();
    }
}

/// Where the sensitive things are in `words`: one box for each, around all the words it spans.
pub fn find(words: &[Word]) -> Vec<Rect> {
    let mut found = Vec::new();
    for line in words.chunk_by(|a, b| a.line == b.line) {
        let texts: Vec<&str> = line.iter().map(|w| w.text.as_str()).collect();
        let mut claimed = vec![false; line.len()];
        let mut claim = |first: usize, len: usize| {
            if claimed[first..first + len].iter().any(|&c| c) {
                return;
            }
            claimed[first..first + len].fill(true);
            found.push(line[first..first + len].iter().skip(1).fold(line[first].rect, |all, w| all.union(w.rect)));
        };
        for (i, text) in texts.iter().enumerate() {
            if candidates(text).any(|c| is_email(c) || is_ip(c) || is_secret(c)) {
                claim(i, 1);
            }
        }
        // Card and phone numbers can be spread over several words.
        let loose: Vec<&str> = texts.iter().map(|t| t.trim_start_matches(QUOTES).trim_end_matches(TRAILING)).collect();
        for i in 0..loose.len() {
            if let Some(len) = card_words(&loose[i..]).or_else(|| phone_words(&loose[i..])) {
                claim(i, len);
            }
        }
    }
    found
}

const QUOTES: [char; 3] = ['"', '\'', '`'];
const TRAILING: [char; 9] = ['"', '\'', '`', ',', ';', ':', '!', '?', '.'];

/// What to look for in a word: itself without the punctuation around it, and what follows a `=` or
/// `:` in it (`API_KEY=…`, `"token":"…"`).
fn candidates(word: &str) -> impl Iterator<Item = &str> {
    let value = word.rsplit_once(['=', ':']).map(|(_, v)| trim(v)).filter(|v| !v.is_empty());
    std::iter::once(trim(word)).chain(value)
}

fn trim(s: &str) -> &str {
    s.trim_matches(['(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '`', ',', ';', '!', '?', '.'])
}

fn is_email(s: &str) -> bool {
    let s = s.strip_prefix("mailto:").unwrap_or(s);
    let Some((local, domain)) = s.split_once('@') else { return false };
    let labels: Vec<&str> = domain.split('.').collect();
    !local.is_empty()
        && local.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
        && labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
}

/// An IPv4 or IPv6 address, possibly with a port or a prefix length after it.
fn is_ip(s: &str) -> bool {
    let v4 = s.split(['/', ':']).next().unwrap_or("");
    if v4.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    // "[::1]:8080" and "fe80::1%eth0" are addresses too.
    let v6 = s.split([']', '/', '%']).next().unwrap_or("");
    v6.matches(':').count() >= 2 && v6.contains(|c: char| c.is_ascii_digit()) && v6.parse::<Ipv6Addr>().is_ok()
}

/// A key or token: one with a well-known prefix, a JWT, or a long string that looks random.
fn is_secret(s: &str) -> bool {
    let from = |prefix: &str, min: usize, ok: fn(char) -> bool| s.strip_prefix(prefix).is_some_and(|rest| rest.len() >= min && rest.chars().all(ok));
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    from("sk-", 16, word)
        || ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"].iter().any(|p| from(p, 20, |c| c.is_ascii_alphanumeric()))
        || from("github_pat_", 20, word)
        || ["AKIA", "ASIA"].iter().any(|p| from(p, 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        || ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"].iter().any(|p| from(p, 10, word))
        || is_jwt(s)
        || looks_random(s)
}

fn is_jwt(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    s.starts_with("eyJ") && parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// Long, with letters and digits mixed through it and plenty of different characters, which words
/// and names (even long snake_case ones) and dates don't have.
fn looks_random(s: &str) -> bool {
    if s.len() < 24 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return false;
    }
    let digits = s.chars().filter(char::is_ascii_digit).count();
    let letters = s.chars().filter(char::is_ascii_alphabetic).count();
    let mixed = s.as_bytes().windows(2).filter(|w| w[0].is_ascii_digit() != w[1].is_ascii_digit() && w[0].is_ascii_alphanumeric() && w[1].is_ascii_alphanumeric()).count();
    let mut distinct: Vec<char> = s.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    // Separators only turn up in keys that also mix upper and lower case.
    let separated = s.contains(['_', '-']);
    let cased = s.chars().any(|c| c.is_ascii_uppercase()) && s.chars().any(|c| c.is_ascii_lowercase());
    digits >= (s.len() / 10).max(3) && letters >= 4 && mixed >= 3 && distinct.len() >= 12 && (!separated || cased)
}

/// How many of `words` (from the first) are a card number: 13 to 19 digits that pass the Luhn
/// check, in groups of at least four.
fn card_words(words: &[&str]) -> Option<usize> {
    let mut digits = String::new();
    let mut found = None;
    for (n, word) in words.iter().enumerate() {
        let grouped = word.chars().all(|c| c.is_ascii_digit() || c == '-') && word.starts_with(|c: char| c.is_ascii_digit()) && word.ends_with(|c: char| c.is_ascii_digit());
        if !grouped || word.chars().filter(char::is_ascii_digit).count() < 4 {
            break;
        }
        digits.extend(word.chars().filter(char::is_ascii_digit));
        if digits.len() > 19 {
            break;
        }
        // Real cards start 2 to 6; this leaves out things like timestamps in milliseconds.
        if digits.len() >= 13 && digits.starts_with(['2', '3', '4', '5', '6']) && luhn(&digits) {
            found = Some(n + 1);
        }
    }
    found
}

/// Whether the digits pass the Luhn check that card numbers are made to.
fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .enumerate()
        .map(|(i, d)| if i % 2 == 1 { [0, 2, 4, 6, 8, 1, 3, 5, 7, 9][d as usize] } else { d })
        .sum();
    sum.is_multiple_of(10)
}

/// How many of `words` (from the first) are a phone number: `+` and 8 to 15 digits, or the usual
/// ways of grouping the digits of a US (555 123 4567) or national (020 7946 0958) number.
/// Dates, versions, times and plain numbers aren't.
fn phone_words(words: &[&str]) -> Option<usize> {
    let international = words.first()?.trim_start_matches('(').starts_with('+');
    let mut groups: Vec<&str> = Vec::new();
    let mut found = None;
    for (n, word) in words.iter().enumerate().take(6) {
        let phone_like = word.chars().enumerate().all(|(i, c)| c.is_ascii_digit() || "-.()".contains(c) || (c == '+' && n == 0 && i <= 1)) && word.contains(|c: char| c.is_ascii_digit());
        if !phone_like {
            break;
        }
        groups.extend(word.split(|c: char| !c.is_ascii_digit()).filter(|g| !g.is_empty()));
        let digits: usize = groups.iter().map(|g| g.len()).sum();
        if digits > 15 {
            break;
        }
        let lengths: Vec<usize> = groups.iter().map(|g| g.len()).collect();
        let national = groups[0].starts_with('0') && (10..=11).contains(&digits) && ((2..=4).contains(&groups.len()) && (3..=5).contains(&groups[0].len()) && lengths[1..].iter().all(|&l| l >= 3) || lengths == [2; 5]);
        let ok = if international { digits >= 8 } else { matches!(lengths[..], [3, 3, 4] | [1, 3, 3, 4]) || national };
        if ok {
            found = Some(n + 1);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::gallery::doc::Doc;
    use crate::gallery::words::example;

    /// The text of the words each box found covers.
    fn found(lines: &[&str]) -> Vec<String> {
        let words = example(lines);
        find(&words)
            .into_iter()
            .map(|r| words.iter().filter(|w| r.contains_rect(w.rect)).map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn nothing(line: &str) {
        assert_eq!(found(&[line]), Vec::<String>::new(), "{line}");
    }

    #[test]
    fn finds_email_addresses() {
        assert_eq!(found(&["Write to dan.h+rsnip@example.co.uk, or"]), ["dan.h+rsnip@example.co.uk,"]);
        assert_eq!(found(&["(bob@mail.example.com)"]), ["(bob@mail.example.com)"]);
        assert_eq!(found(&["Email:a@b.io"]), ["Email:a@b.io"]);
        nothing("user@localhost @mention not@ a@b.c");
    }

    #[test]
    fn finds_ip_addresses() {
        assert_eq!(found(&["server 10.0.0.1 and (192.168.1.20)."]), ["10.0.0.1", "(192.168.1.20)."]);
        assert_eq!(found(&["10.0.0.1:8080 10.0.0.1: 10.0.0.0/24"]), ["10.0.0.1:8080", "10.0.0.1:", "10.0.0.0/24"]);
        assert_eq!(found(&["at 2001:db8::ff00:42:8329, [::1]:443 fe80::1%eth0"]), ["2001:db8::ff00:42:8329,", "[::1]:443", "fe80::1%eth0"]);
        nothing("version 1.2.3 10.0.19045.5000 256.1.1.1 12:30:45 10:30 std::vec:: ::");
    }

    #[test]
    fn finds_keys_and_tokens() {
        let keys = [
            "sk-proj-abcdEFGH1234ijklMNOP5678",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_11ABCDEFG0abcdefghijkl_mnopqrstuvwxyz",
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-1234567890-abcdefghij",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "Zk3jX9qLm2Vb7Rt5Yp8Wc1Nd4Hs6Gf0A",
        ];
        for key in keys {
            assert_eq!(found(&[&format!("token {key}")]), [key], "{key}");
        }
        assert_eq!(found(&["API_KEY=AKIAIOSFODNN7EXAMPLE,"]), ["API_KEY=AKIAIOSFODNN7EXAMPLE,"]);
        assert_eq!(found(&["\"token\":\"ghp_abcdefghijklmnopqrstuvwxyz0123456789\","]).len(), 1);
    }

    #[test]
    fn leaves_ordinary_long_words_alone() {
        nothing("get_user_profile_settings_v2_handler");
        nothing("release-2026-10-25-final-build-notes");
        nothing("supercalifragilisticexpialidocious2026");
        nothing("Pneumonoultramicroscopicsilicovolcanoconiosis");
        nothing("123e4567-e89b-12d3-a456-426614174000");
        nothing("/usr/local/lib/python3.12/site-packages/numpy");
        nothing("sk-short ghp_short AKIA123 eyJabc.def eyJabc.def.");
    }

    #[test]
    fn finds_card_numbers_with_the_luhn_check() {
        assert!(luhn("4111111111111111") && luhn("378282246310005") && !luhn("4111111111111112"));
        assert_eq!(found(&["card 4111 1111 1111 1111 expires"]), ["4111 1111 1111 1111"]);
        assert_eq!(found(&["4111-1111-1111-1111, 5500005555555559"]), ["4111-1111-1111-1111,", "5500005555555559"]);
        assert_eq!(found(&["amex 3782 822463 10005."]), ["3782 822463 10005."]);
        nothing("4111 1111 1111 1112");
        nothing("1730000000000 17300000000000");
        nothing("order 4111 1111 1111");
    }

    #[test]
    fn finds_phone_numbers() {
        assert_eq!(found(&["call +1 555 123 4567 now"]), ["+1 555 123 4567"]);
        assert_eq!(found(&["+44 20 7946 0958, (+49)151-2345678"]), ["+44 20 7946 0958,", "(+49)151-2345678"]);
        assert_eq!(found(&["+15551234567"]), ["+15551234567"]);
        assert_eq!(found(&["(555) 123-4567 555-123-4567 555.123.4567 1-800-555-0199"]), ["(555) 123-4567", "555-123-4567", "555.123.4567", "1-800-555-0199"]);
        assert_eq!(found(&["020 7946 0958 07911 123456 01 23 45 67 89"]), ["020 7946 0958", "07911 123456", "01 23 45 67 89"]);
    }

    #[test]
    fn leaves_dates_versions_times_and_numbers_alone() {
        nothing("2026-10-25 25/10/2026 10:30:45 v1.2.3 1.2.3 +5 points");
        nothing("12 345 6789 1234567890 555-1234 100,000,000 +1234567");
        nothing("0.5.3 01-02-2026 3.14159265");
    }

    #[test]
    fn boxes_the_words_each_match_spans() {
        let words = example(&["Call +1 555 123 4567 or", "mail a@b.io now"]);
        let boxes = find(&words);
        // "+1 555 123 4567" runs from the start of "+1" to the end of "4567"; 10 px per letter.
        assert_eq!(boxes, [Rect::from_min_max(egui::pos2(60.0, 10.0), egui::pos2(210.0, 30.0)), Rect::from_min_max(egui::pos2(60.0, 40.0), egui::pos2(120.0, 60.0))]);
    }

    fn editor(lines: &[&'static str]) -> Editor {
        let mut editor = Editor::new(Doc::from_image(image::RgbaImage::new(400, 200)));
        let words = example(lines);
        editor.reader = Some(Arc::new(move |_| Ok(words.clone())));
        editor
    }

    /// Runs Auto-redact to the end, frame by frame as the text is read in the background.
    fn auto_redact(editor: &mut Editor) {
        let ctx = egui::Context::default();
        editor.start_auto_redact();
        for _ in 0..200 {
            editor.read_text(&ctx, None);
            editor.finish_auto_redact(&ctx, Color32::BLUE);
            if !editor.auto_redacting() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never finished");
    }

    #[test]
    fn covers_everything_found_in_one_undo_step() {
        let mut editor = editor(&["mail a@b.io", "from 10.0.0.1 now"]);
        auto_redact(&mut editor);
        let image = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let covered: Vec<Shape> = [Rect::from_min_max(egui::pos2(60.0, 10.0), egui::pos2(120.0, 30.0)), Rect::from_min_max(egui::pos2(60.0, 40.0), egui::pos2(140.0, 60.0))]
            .into_iter()
            .map(|r| Shape::Redact { rect: r.expand(2.0).intersect(image), color: Color32::BLUE })
            .collect();
        assert!(editor.doc.current().shapes == covered);
        assert_eq!(editor.auto_redact_note(), Some("Redacted 2: undo brings them back"));
        editor.doc.undo();
        assert!(editor.doc.current().shapes.is_empty());
        assert_eq!(editor.auto_redact_note(), None, "no longer true once undone");
    }

    #[test]
    fn says_when_theres_nothing_to_redact() {
        let mut editor = editor(&["just some words", "and 1.2.3"]);
        auto_redact(&mut editor);
        assert!(editor.doc.current().shapes.is_empty() && !editor.doc.can_undo());
        assert_eq!(editor.auto_redact_note(), Some("Nothing to redact"));
    }

    #[test]
    fn a_match_is_only_boxed_once() {
        // A card number is also digits a phone number could be. (Single words are found first.)
        assert_eq!(found(&["4111 1111 1111 1111 and 10.0.0.1"]), ["10.0.0.1", "4111 1111 1111 1111"]);
    }
}
