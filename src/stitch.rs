//! Joins the frames of a scrolling capture into one tall image.
//!
//! Each new frame is compared with the previous one row by row (rows are hashed, so this is cheap):
//! rows that stay put at the top and bottom are a sticky header/footer, and the scroll distance is
//! the offset at which the remaining rows line up best. The rows that scrolled into view are then
//! appended above the footer.

use crate::capture::Image;

pub enum Step {
    /// The frame scrolled by this many pixels and its new rows were added.
    Added(usize),
    /// Nothing moved: the end of the page.
    End,
    /// The frame couldn't be lined up with the previous one.
    Lost,
}

pub struct Stitcher {
    width: usize,
    height: usize,
    /// The result so far, ending with the latest frame's footer.
    out: Vec<u8>,
    prev: Vec<Row>,
}

#[derive(Clone, Copy, PartialEq)]
struct Row {
    hash: u64,
    /// Every pixel the same colour; such rows match anywhere, so they don't count as evidence.
    plain: bool,
}

fn rows(img: &Image) -> Vec<Row> {
    img.data
        .chunks_exact(img.width as usize * 4)
        .map(|row| {
            // FNV-1a
            let hash = row.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
            let plain = row.chunks_exact(4).all(|px| px == &row[..4]);
            Row { hash, plain }
        })
        .collect()
}

impl Stitcher {
    pub fn new(first: &Image) -> Stitcher {
        Stitcher { width: first.width as usize, height: first.height as usize, out: first.data.clone(), prev: rows(first) }
    }

    /// Height of the result so far.
    pub fn height(&self) -> usize {
        self.out.len() / (self.width * 4)
    }

    pub fn push(&mut self, frame: &Image) -> Step {
        if frame.width as usize != self.width || frame.height as usize != self.height {
            return Step::Lost;
        }
        let next = rows(frame);
        if next == self.prev {
            return Step::End;
        }
        let h = self.height;
        let top = (0..h).take_while(|&y| next[y] == self.prev[y]).count();
        let bottom = (top..h).rev().take_while(|&y| next[y] == self.prev[y]).count();
        let (start, end) = (top, h - bottom);

        // Find the scroll distance d with next[y] == prev[y + d] for the most informative rows.
        let mut best = None;
        for d in 1..end - start {
            let mut matched = 0;
            let mut informative = 0;
            for (&a, &b) in next[start..end - d].iter().zip(&self.prev[start + d..end]) {
                if !(a.plain && b.plain) {
                    informative += 1;
                    matched += (a == b) as usize;
                }
            }
            let good = matched >= 4 && matched * 10 >= informative * 6;
            if good && best.is_none_or(|(_, m)| matched > m) {
                best = Some((d, matched));
            }
        }
        let Some((d, _)) = best else { return Step::Lost };

        // Replace the old footer with the newly revealed rows plus the new footer.
        let row_bytes = self.width * 4;
        self.out.truncate(self.out.len() - bottom * row_bytes);
        self.out.extend_from_slice(&frame.data[(end - d) * row_bytes..]);
        self.prev = next;
        Step::Added(d)
    }

    pub fn finish(self) -> Image {
        let height = self.height() as i32;
        Image { width: self.width as i32, height, data: self.out }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall "page" whose rows all differ, with some blank stretches like real documents.
    fn page(width: usize, height: usize) -> Vec<[u8; 4]> {
        (0..height)
            .map(|y| if y % 50 < 8 { [255, 255, 255, 255] } else { [(y * 7) as u8, (y * 13 / 3) as u8, (y / 2) as u8, 255] })
            .flat_map(|row| {
                // Vary along the row too, so rows aren't plain.
                (0..width).map(move |x| [row[0].wrapping_add(x as u8), row[1], row[2], 255])
            })
            .collect()
    }

    /// What the window shows when scrolled to `offset`, with optional sticky header/footer.
    fn frame(page: &[[u8; 4]], width: usize, offset: usize, height: usize, header: usize, footer: usize) -> Image {
        let mut data = Vec::new();
        for y in 0..height {
            let row: Vec<u8> = if y < header {
                (0..width).flat_map(|x| [10, 20, (x % 256) as u8, 255]).collect()
            } else if y >= height - footer {
                (0..width).flat_map(|x| [200, 10, (x % 256) as u8, 255]).collect()
            } else {
                page[(offset + y) * width..(offset + y + 1) * width].iter().flatten().copied().collect()
            };
            data.extend(row);
        }
        Image { width: width as i32, height: height as i32, data }
    }

    fn stitch(header: usize, footer: usize) {
        let (w, h, view) = (64, 1200, 300);
        let page = page(w, h);
        let offsets = [0, 120, 260, 330, 330];
        let mut s = Stitcher::new(&frame(&page, w, offsets[0], view, header, footer));
        for (i, &o) in offsets.iter().enumerate().skip(1) {
            match s.push(&frame(&page, w, o, view, header, footer)) {
                Step::Added(d) => assert_eq!(d, o - offsets[i - 1]),
                Step::End => assert_eq!(o, offsets[i - 1], "only the repeated frame ends it"),
                Step::Lost => panic!("lost at offset {o}"),
            }
        }
        let out = s.finish();
        let expected = frame(&page, w, 0, view + 330, header, footer);
        assert_eq!((out.width, out.height), (expected.width, expected.height));
        assert!(out.data == expected.data, "stitched image matches the page");
    }

    #[test]
    fn stitches_plain_scrolling() {
        stitch(0, 0);
    }

    #[test]
    fn keeps_sticky_header_and_footer_once() {
        stitch(40, 25);
    }

    #[test]
    fn reports_unrelated_frames_as_lost() {
        let (w, view) = (64, 200);
        let page = page(w, 2000);
        let mut s = Stitcher::new(&frame(&page, w, 0, view, 0, 0));
        assert!(matches!(s.push(&frame(&page, w, 1500, view, 0, 0)), Step::Lost));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

        /// Any scroll steps that leave some overlap, with any sticky header and footer, give back
        /// the page with the header and footer once.
        #[test]
        fn random_scrolling_rebuilds_the_page(
            header in 0usize..40,
            footer in 0usize..30,
            view in 150usize..300,
            steps in proptest::collection::vec(1usize..1000, 1..8),
        ) {
            let (w, h) = (48, 1200);
            let page = page(w, h);
            let visible = view - header - footer;
            let mut offset = 0;
            let mut s = Stitcher::new(&frame(&page, w, 0, view, header, footer));
            for step in steps {
                // Keep enough rows overlapping to line frames up, and stay on the page.
                let step = 1 + step % (visible - 10);
                if offset + step + view > h {
                    break;
                }
                offset += step;
                let added = matches!(s.push(&frame(&page, w, offset, view, header, footer)), Step::Added(d) if d == step);
                proptest::prop_assert!(added, "scrolled {step} at {offset}");
            }
            let out = s.finish();
            let expected = frame(&page, w, 0, view + offset, header, footer);
            proptest::prop_assert_eq!((out.width, out.height), (expected.width, expected.height));
            proptest::prop_assert!(out.data == expected.data);
        }
    }
}
