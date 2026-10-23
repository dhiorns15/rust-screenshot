//! The text recognised in an image, word by word with where each word is, so the editor's Copy
//! text tool can select it. Windows' own engine reads it there; elsewhere Tesseract does, when it's
//! installed.

use eframe::egui::{vec2, Pos2, Rect};

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// Where it is, in image pixels.
    pub rect: Rect,
    /// Which line of text it's on, counting from 0. Words come in reading order.
    pub line: usize,
}

/// `words` as text: spaces between words, a newline between lines.
pub fn text(words: &[Word]) -> String {
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            out.push(if words[i - 1].line == word.line { ' ' } else { '\n' });
        }
        out.push_str(&word.text);
    }
    out
}

/// Tesseract's word list (`tesseract image - tsv`), with each box scaled by `scale` (to undo
/// enlarging the image for it).
#[cfg_attr(windows, allow(dead_code))]
pub fn from_tesseract_tsv(tsv: &str, scale: f32) -> Vec<Word> {
    let mut words = Vec::new();
    let mut last_line = None;
    let mut line = 0;
    // Columns: level page block paragraph line word left top width height confidence text.
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.split('\t').collect();
        let [level, _, block, par, ln, _, left, top, width, height, _, text] = cols[..] else { continue };
        let text = text.trim();
        if level != "5" || text.is_empty() {
            continue;
        }
        let num = |s: &str| s.parse::<f32>().ok();
        let (Some(left), Some(top), Some(width), Some(height)) = (num(left), num(top), num(width), num(height)) else { continue };
        let id = (block, par, ln);
        if last_line.is_some_and(|last| last != id) {
            line += 1;
        }
        last_line = Some(id);
        let rect = Rect::from_min_size(Pos2::new(left * scale, top * scale), vec2(width, height) * scale);
        words.push(Word { text: text.to_owned(), rect, line });
    }
    words
}

#[cfg(test)]
pub fn example(lines: &[&str]) -> Vec<Word> {
    // Each word 10 px per letter, 20 px tall, one line every 30 px.
    let mut words = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let mut x = 10.0;
        for w in text.split_whitespace() {
            let width = w.len() as f32 * 10.0;
            let rect = Rect::from_min_size(Pos2::new(x, 10.0 + line as f32 * 30.0), vec2(width, 20.0));
            words.push(Word { text: w.to_owned(), rect, line });
            x += width + 10.0;
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_words_into_lines() {
        assert_eq!(text(&example(&["Hello there", "second line"])), "Hello there\nsecond line");
        assert_eq!(text(&[]), "");
    }

    #[test]
    fn reads_tesseracts_word_list() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t400\t100\t-1\t\n\
                   4\t1\t1\t1\t1\t0\t10\t10\t200\t20\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t80\t20\t96.5\tHello\n\
                   5\t1\t1\t1\t1\t2\t100\t10\t90\t20\t95.1\tRSnip\n\
                   5\t1\t1\t1\t2\t1\t10\t40\t40\t20\t91.0\t123\n\
                   5\t1\t1\t1\t2\t2\t60\t40\t40\t20\t10.0\t \n\
                   5\t1\t2\t1\t1\t1\t10\t80\t40\t20\t90.0\tnext";
        let words = from_tesseract_tsv(tsv, 0.5);
        assert_eq!(text(&words), "Hello RSnip\n123\nnext");
        assert_eq!(words[1].rect, Rect::from_min_max(Pos2::new(50.0, 5.0), Pos2::new(95.0, 15.0)));
        assert_eq!(words.iter().map(|w| w.line).collect::<Vec<_>>(), [0, 0, 1, 2]);
    }
}
