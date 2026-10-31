//! Property tests (proptest) for the document model: the fast image effects against their
//! reference versions, undo/redo over random edits, saved layers round-tripping, and corrupt files.

use proptest::prelude::*;

use super::*;

/// A deterministic image of `w`×`h` from `seed`.
fn noise(w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..w * h * 4)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

fn pixmap(w: usize, h: usize, data: &[u8]) -> Pixmap {
    let mut p = Pixmap::new(w as u32, h as u32).unwrap();
    p.data_mut().copy_from_slice(data);
    p
}

/// A rect anywhere around (and partly or wholly outside) a `w`×`h` image.
fn any_rect(w: usize, h: usize) -> impl Strategy<Value = Rect> {
    let (w, h) = (w as f32, h as f32);
    (-20.0..w + 20.0, -20.0..h + 20.0, 0.0f32..w + 40.0, 0.0f32..h + 40.0)
        .prop_map(|(x, y, rw, rh)| Rect::from_min_size(Pos2::new(x, y), Vec2::new(rw, rh)))
}

/// Up to 100 pixels a side, so the blur's radius (a sixth of the shorter side, at most 14) reaches its cap.
fn image_and_rect() -> impl Strategy<Value = (usize, usize, u64, Rect)> {
    (1usize..100, 1usize..100, any::<u64>()).prop_flat_map(|(w, h, seed)| any_rect(w, h).prop_map(move |r| (w, h, seed, r)))
}

/// Builds one of every kind of shape from a few numbers.
fn shape(kind: u8, a: f32, b: f32, c: f32, d: f32) -> Shape {
    let rect = Rect::from_two_pos(Pos2::new(a, b), Pos2::new(c, d));
    let (from, to) = (Pos2::new(a, b), Pos2::new(c, d));
    let color = Color32::from_rgb(a as u8, b as u8, c as u8);
    match kind % 12 {
        0 => Shape::Box { rect, color, width: 1.0 + d % 20.0 },
        1 => Shape::Ellipse { rect, color, width: 3.0 },
        2 => Shape::Highlight { rect, color },
        3 => Shape::Blur { rect },
        4 => Shape::Arrow { from, to, color, width: 4.0 },
        5 => Shape::Pen { points: vec![from, to, Pos2::new(a, d)], color, width: 2.0 },
        6 => Shape::Text { pos: from, text: format!("t{kind}"), color, size: 8.0 + c % 90.0, shadow: kind.is_multiple_of(2) },
        7 => Shape::Step { center: from, number: kind as u32, color, diameter: 20.0 },
        8 => Shape::Line { from, to, color, width: 2.5 },
        9 => Shape::Redact { rect, color },
        10 => Shape::Spotlight { rect },
        _ => Shape::Pixelate { rect },
    }
}

fn any_shape() -> impl Strategy<Value = (u8, f32, f32, f32, f32)> {
    (any::<u8>(), 0.0f32..300.0, 0.0f32..200.0, 0.0f32..300.0, 0.0f32..200.0)
}

#[derive(Debug, Clone)]
enum Op {
    Add((u8, f32, f32, f32, f32)),
    Remove(usize),
    Move(usize, f32),
    /// Like a slider drag: merges into one undo step while repeated.
    Recolor(usize, u8),
    Undo,
    Redo,
    Save,
}

fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any_shape().prop_map(Op::Add),
        1 => any::<usize>().prop_map(Op::Remove),
        1 => (any::<usize>(), -50.0f32..50.0).prop_map(|(i, d)| Op::Move(i, d)),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(i, c)| Op::Recolor(i, c)),
        2 => Just(Op::Undo),
        1 => Just(Op::Redo),
        1 => Just(Op::Save),
    ]
}

fn apply(doc: &mut Doc, op: &Op) {
    let count = doc.current().shapes.len();
    match *op {
        Op::Add((k, a, b, c, d)) => doc.add(shape(k, a, b, c, d)),
        Op::Remove(i) if count > 0 => doc.replace(i % count, None),
        Op::Move(i, d) if count > 0 => {
            let moved = doc.current().shapes[i % count].translated(Vec2::splat(d));
            doc.replace(i % count, Some(moved));
        }
        Op::Recolor(i, c) if count > 0 => {
            let i = i % count;
            let shape = match doc.current().shapes[i].clone() {
                Shape::Box { rect, width, .. } => Shape::Box { rect, color: Color32::from_gray(c), width },
                other => other,
            };
            doc.amend(i, shape, "color");
        }
        Op::Undo => doc.undo(),
        Op::Redo => doc.redo(),
        Op::Save => doc.mark_saved(PathBuf::new()),
        _ => {}
    }
}

/// A folder of its own for one test case.
fn temp_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("rsnip-prop-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn pixelate_matches_the_reference((w, h, seed, rect) in image_and_rect()) {
        let data = noise(w, h, seed);
        let mut fast = pixmap(w, h, &data);
        let region = pixel_region(&fast, rect);
        pixelate(&mut fast, rect);
        let mut slow = data.clone();
        if let Some(region) = region {
            reference::pixelate(&mut slow, w, region);
        }
        prop_assert!(fast.data() == slow.as_slice());
    }

    #[test]
    fn blur_matches_the_reference((w, h, seed, rect) in image_and_rect()) {
        let data = noise(w, h, seed);
        let mut fast = pixmap(w, h, &data);
        let region = pixel_region(&fast, rect);
        blur(&mut fast, rect);
        let mut slow = data.clone();
        if let Some(region) = region {
            reference::blur(&mut slow, w, region);
        }
        prop_assert!(fast.data() == slow.as_slice());
    }

    #[test]
    fn effects_stay_inside_their_region((w, h, seed, rect) in image_and_rect()) {
        let data = noise(w, h, seed);
        let mut p = pixmap(w, h, &data);
        let region = pixel_region(&p, rect);
        blur(&mut p, rect);
        pixelate(&mut p, rect);
        for (i, (a, b)) in p.data().chunks_exact(4).zip(data.chunks_exact(4)).enumerate() {
            let (x, y) = (i % w, i / w);
            let inside = region.is_some_and(|(x0, y0, rw, rh)| (x0..x0 + rw).contains(&x) && (y0..y0 + rh).contains(&y));
            prop_assert!(inside || a == b, "pixel {x},{y} outside {region:?} changed");
        }
    }

    #[test]
    fn undoing_everything_and_redoing_it_returns_to_each_end(ops in prop::collection::vec(any_op(), 0..60)) {
        let mut doc = Doc::from_image(RgbaImage::new(300, 200));
        for op in &ops {
            apply(&mut doc, op);
            prop_assert_eq!(doc.dirty(), doc.saved != Some(doc.index));
        }
        let end = doc.current().shapes.clone();
        let could_redo = doc.can_redo();
        let mut undone = 0;
        while doc.can_undo() {
            doc.undo();
            undone += 1;
        }
        prop_assert!(doc.current().shapes.is_empty(), "back to the plain image");
        for _ in 0..undone {
            prop_assert!(doc.can_redo());
            doc.redo();
        }
        prop_assert_eq!(doc.can_redo(), could_redo, "and whatever was undone before is still there to redo");
        prop_assert!(doc.current().shapes == end, "redo returns to where we were");
    }

    #[test]
    fn saved_layers_reopen_with_the_same_annotations(shapes in prop::collection::vec(any_shape(), 0..10)) {
        let dir = temp_dir("layers");
        let path = dir.join("shot.png");
        RgbaImage::from_pixel(300, 200, image::Rgba([9, 99, 199, 255])).save(&path).unwrap();
        let mut doc = Doc::open(&path).unwrap();
        for &(k, a, b, c, d) in &shapes {
            doc.add(shape(k, a, b, c, d));
        }
        doc.save_layers(false).unwrap();
        let reopened = Doc::open(&path).unwrap();
        prop_assert!(reopened.current().shapes == doc.current().shapes);
        prop_assert!(*reopened.current().base == *doc.current().base);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn garbage_layers_open_as_the_plain_image(junk in prop::collection::vec(any::<u8>(), 0..300)) {
        let dir = temp_dir("junk");
        let path = dir.join("shot.png");
        RgbaImage::from_pixel(40, 30, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let (json, base) = layer_paths(&path).unwrap();
        std::fs::create_dir_all(json.parent().unwrap()).unwrap();
        std::fs::write(&json, &junk).unwrap();
        std::fs::write(base, &junk).unwrap();
        let doc = Doc::open(&path).unwrap();
        prop_assert!(doc.current().shapes.is_empty());
        prop_assert_eq!(doc.size(), Vec2::new(40.0, 30.0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// The oracles above are handed `pixel_region`'s answer, so this checks it on its own.
#[test]
fn pixel_region_is_the_whole_pixels_the_rect_touches_inside_the_image() {
    let p = Pixmap::new(10, 8).unwrap();
    let r = |x0, y0, x1, y1| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
    assert_eq!(pixel_region(&p, r(2.3, 2.7, 5.1, 6.0)), Some((2, 2, 4, 4)), "partly covered pixels count");
    assert_eq!(pixel_region(&p, r(-5.0, -5.0, 50.0, 50.0)), Some((0, 0, 10, 8)), "clipped to the image");
    assert_eq!(pixel_region(&p, r(7.5, 1.0, 30.0, 3.0)), Some((7, 1, 3, 2)));
    assert_eq!(pixel_region(&p, r(12.0, 1.0, 30.0, 3.0)), None, "off the right");
    assert_eq!(pixel_region(&p, r(-9.0, -9.0, -1.0, -1.0)), None, "off the top left");
    assert_eq!(pixel_region(&p, r(3.0, 3.0, 3.0, 5.0)), None, "no width");
    assert_eq!(pixel_region(&p, r(3.0, 3.0, 3.5, 3.5)), Some((3, 3, 1, 1)), "within one pixel");
}

#[test]
fn layers_with_a_damaged_base_image_or_a_stale_stamp_open_plain() {
    let dir = temp_dir("damaged");
    let path = dir.join("shot.png");
    RgbaImage::from_pixel(60, 40, image::Rgba([5, 5, 5, 255])).save(&path).unwrap();
    let mut doc = Doc::open(&path).unwrap();
    doc.add(shape(0, 1.0, 2.0, 30.0, 20.0));
    doc.save_layers(false).unwrap();
    let (json, base) = layer_paths(&path).unwrap();

    // A truncated base image.
    let bytes = std::fs::read(&base).unwrap();
    std::fs::write(&base, &bytes[..bytes.len() / 2]).unwrap();
    assert!(Doc::open(&path).unwrap().current().shapes.is_empty());

    // Valid layers, but the PNG was changed by another program since.
    std::fs::write(&base, &bytes).unwrap();
    assert_eq!(Doc::open(&path).unwrap().current().shapes.len(), 1, "intact layers load");
    RgbaImage::from_pixel(60, 40, image::Rgba([7, 7, 7, 255])).save(&path).unwrap();
    assert!(Doc::open(&path).unwrap().current().shapes.is_empty(), "stale stamp");

    // An unknown future format version.
    let text = std::fs::read_to_string(&json).unwrap().replace("\"version\":1", "\"version\":99");
    std::fs::write(&json, text).unwrap();
    assert!(Doc::open(&path).unwrap().current().shapes.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
