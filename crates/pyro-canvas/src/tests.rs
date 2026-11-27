use super::*;

fn state(clip: &[ClipPath]) -> DrawState<'_> {
    DrawState {
        transform: Affine::IDENTITY,
        alpha: 1.0,
        composite: Composite::SourceOver,
        clip,
        shadow: None,
        smoothing: true, filter: None,
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Paint {
    Paint::Color(Color::from_rgba8(r, g, b, 255))
}

fn px(s: &mut Surface, x: i64, y: i64) -> [u8; 4] {
    let d = s.get_image_data(x, y, 1, 1);
    [d[0], d[1], d[2], d[3]]
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    Rect::new(x, y, x + w, y + h).to_path(0.1)
}

#[test]
fn unallocated_until_drawn() {
    let mut s = Surface::new(10, 10);
    assert_eq!(px(&mut s, 0, 0), [0, 0, 0, 0]);
    assert!(!s.is_allocated());
    s.clear_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Affine::IDENTITY, &[]);
    assert!(!s.is_allocated());
    s.fill(&rect(0.0, 0.0, 5.0, 5.0), false, &rgb(255, 0, 0), &state(&[]));
    assert!(s.is_allocated());
    assert_eq!(px(&mut s, 2, 2), [255, 0, 0, 255]);
    assert_eq!(px(&mut s, 7, 7), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, -1, 0), [0, 0, 0, 0]);
}

#[test]
fn composite_operators() {
    let mut s = Surface::new(10, 10);
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &rgb(255, 0, 0), &state(&[]));
    let mut st = state(&[]);
    st.composite = Composite::DestinationOut;
    s.fill(&rect(0.0, 0.0, 5.0, 10.0), false, &rgb(0, 0, 255), &st);
    assert_eq!(px(&mut s, 2, 2), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, 7, 2), [255, 0, 0, 255]);
    st.composite = Composite::Copy;
    s.fill(&rect(0.0, 0.0, 2.0, 2.0), false, &rgb(0, 255, 0), &st);
    assert_eq!(px(&mut s, 1, 1), [0, 255, 0, 255]);
    assert_eq!(px(&mut s, 7, 7), [0, 0, 0, 0], "copy clears outside the shape");
    st.composite = Composite::Multiply;
    s.fill(&rect(0.0, 0.0, 2.0, 2.0), false, &rgb(128, 128, 128), &st);
    assert_eq!(px(&mut s, 1, 1), [0, 128, 0, 255]);
}

#[test]
fn global_alpha_and_clip() {
    let mut s = Surface::new(10, 10);
    let clip = [ClipPath { path: rect(0.0, 0.0, 5.0, 10.0), evenodd: false }];
    let mut st = state(&clip);
    st.alpha = 0.5;
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 2, 2), [255, 0, 0, 128]);
    assert_eq!(px(&mut s, 7, 2), [0, 0, 0, 0]);
}

#[test]
fn gradients_and_patterns() {
    let mut s = Surface::new(100, 10);
    let g = Gradient {
        kind: GradientKind::Linear { start: Point::new(0.0, 0.0), end: Point::new(100.0, 0.0) },
        stops: vec![(0.0, Color::from_rgba8(255, 0, 0, 255)), (1.0, Color::from_rgba8(0, 0, 255, 255))],
        oklab: false,
    };
    s.fill(&rect(0.0, 0.0, 100.0, 10.0), false, &Paint::Gradient(Arc::new(g)), &state(&[]));
    let left = px(&mut s, 0, 5);
    let right = px(&mut s, 99, 5);
    assert!(left[0] > 240 && left[2] < 15, "{left:?}");
    assert!(right[2] > 240 && right[0] < 15, "{right:?}");
    let mid = px(&mut s, 50, 5);
    assert!((100..156).contains(&mid[0]) && (100..156).contains(&mid[2]), "{mid:?}");

    let mut tile = Pixmap::new(2, 2);
    tile.data_as_u8_slice_mut()[..4].copy_from_slice(&[0, 255, 0, 255]);
    let p = Pattern { image: Bitmap::new(tile).unwrap(), repeat_x: true, repeat_y: true, transform: Affine::IDENTITY };
    let mut s = Surface::new(10, 10);
    let mut st = state(&[]);
    st.smoothing = false;
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &Paint::Pattern(Arc::new(p)), &st);
    assert_eq!(px(&mut s, 0, 0), [0, 255, 0, 255]);
    assert_eq!(px(&mut s, 1, 0), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, 4, 4), [0, 255, 0, 255]);
}

#[test]
fn strokes_dashes_and_transforms() {
    let mut s = Surface::new(20, 20);
    let mut path = BezPath::new();
    path.move_to((0.0, 10.0));
    path.line_to((20.0, 10.0));
    s.stroke(&path, &Stroke::new(4.0), &rgb(0, 0, 255), &state(&[]));
    assert_eq!(px(&mut s, 5, 9), [0, 0, 255, 255]);
    assert_eq!(px(&mut s, 5, 13), [0, 0, 0, 0]);
    // Scaled 2x vertically: the pen is 8 px tall.
    let mut s = Surface::new(20, 40);
    let mut st = state(&[]);
    st.transform = Affine::scale_non_uniform(1.0, 2.0);
    s.stroke(&(st.transform * path.clone()), &Stroke::new(4.0), &rgb(0, 0, 255), &st);
    assert_eq!(px(&mut s, 5, 17), [0, 0, 255, 255]);
    assert_eq!(px(&mut s, 5, 23), [0, 0, 255, 255]);
    // Dashes 5 on, 5 off.
    let mut s = Surface::new(20, 20);
    s.stroke(&path, &Stroke::new(4.0).with_dashes(0.0, [5.0, 5.0]), &rgb(0, 0, 255), &state(&[]));
    assert_eq!(px(&mut s, 2, 10), [0, 0, 255, 255]);
    assert_eq!(px(&mut s, 7, 10), [0, 0, 0, 0]);
}

#[test]
fn image_data_round_trip_and_draw_image() {
    let mut s = Surface::new(4, 4);
    let data: Vec<u8> = (0..16).flat_map(|i| [i * 10, 0, 255 - i * 10, 255]).collect();
    s.put_image_data(&data, 4, 4, 0, 0, (0, 0, 4, 4));
    assert_eq!(s.get_image_data(0, 0, 4, 4), data);
    let snap = s.snapshot().unwrap();
    let mut t = Surface::new(8, 8);
    let mut st = state(&[]);
    st.smoothing = false;
    t.draw_image(&snap, Rect::new(0.0, 0.0, 4.0, 4.0), Rect::new(0.0, 0.0, 8.0, 8.0), &st);
    assert_eq!(px(&mut t, 0, 0), [0, 0, 255, 255]);
    assert_eq!(px(&mut t, 7, 7), [150, 0, 105, 255]);
}

#[test]
fn shadow_is_drawn_offset() {
    let mut s = Surface::new(20, 20);
    let mut st = state(&[]);
    st.shadow = Some(Shadow { color: Color::from_rgba8(0, 0, 0, 255), blur: 0.0, offset: Vec2::new(5.0, 5.0) });
    s.fill(&rect(0.0, 0.0, 5.0, 5.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 2, 2), [255, 0, 0, 255]);
    assert_eq!(px(&mut s, 7, 7), [0, 0, 0, 255]);
    assert_eq!(px(&mut s, 12, 12), [0, 0, 0, 0]);
}

#[test]
fn shadow_of_geometry_off_the_bitmap() {
    // 2d.shadow.*: a shape above the canvas, its shadow offset onto it.
    let mut s = Surface::new(100, 50);
    let mut st = state(&[]);
    st.shadow = Some(Shadow { color: Color::from_rgba8(0, 255, 0, 255), blur: 0.0, offset: Vec2::new(0.0, 50.0) });
    s.fill(&rect(0.0, -50.0, 100.0, 50.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 50, 25), [0, 255, 0, 255]);
    assert_eq!(px(&mut s, 0, 0), [0, 255, 0, 255]);
    assert_eq!(px(&mut s, 99, 49), [0, 255, 0, 255]);
    // A blurred shadow of a shape just off the bitmap reaches onto it.
    let mut s = Surface::new(50, 50);
    let mut st = state(&[]);
    st.shadow = Some(Shadow { color: Color::from_rgba8(0, 0, 0, 255), blur: 10.0, offset: Vec2::ZERO });
    s.fill(&rect(-20.0, 0.0, 20.0, 50.0), false, &rgb(255, 0, 0), &st);
    let edge = px(&mut s, 0, 25)[3];
    assert!((100..156).contains(&edge), "{edge}");
    // The clipping region still cuts the shadow.
    let clip = [ClipPath { path: rect(0.0, 0.0, 50.0, 25.0), evenodd: false }];
    let mut s = Surface::new(100, 50);
    let mut st = state(&clip);
    st.shadow = Some(Shadow { color: Color::from_rgba8(0, 255, 0, 255), blur: 0.0, offset: Vec2::new(0.0, 50.0) });
    s.fill(&rect(0.0, -50.0, 100.0, 50.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 10, 10), [0, 255, 0, 255]);
    assert_eq!(px(&mut s, 75, 10)[3], 0);
    assert_eq!(px(&mut s, 10, 40)[3], 0);
}

/// The canvas of the 2d.shadow.* WPT pages: 100 by 50, red, and a shadow
/// state with `color` offset by (`x`, `y`).
fn shadow_case(color: Color, x: f64, y: f64) -> (Surface, Option<Shadow>) {
    let mut s = Surface::new(100, 50);
    s.fill(&rect(0.0, 0.0, 100.0, 50.0), false, &rgb(255, 0, 0), &state(&[]));
    (s, Some(Shadow { color, blur: 0.0, offset: Vec2::new(x, y) }))
}

#[test]
fn wpt_shadow_controls() {
    let green = Color::from_rgba8(0, 255, 0, 255);
    // 2d.shadow.alpha.2: a translucent shadow of an opaque fill above.
    let (mut s, shadow) = shadow_case(Color::from_rgba8(0, 0, 255, 128), 0.0, 50.0);
    let mut st = state(&[]);
    st.shadow = shadow;
    s.fill(&rect(0.0, -50.0, 100.0, 50.0), false, &rgb(255, 0, 0), &st);
    let p = px(&mut s, 50, 25);
    assert!(p[0].abs_diff(127) <= 2 && p[1] == 0 && p[2].abs_diff(127) <= 2 && p[3] == 255, "{p:?}");
    // 2d.shadow.stroke.basic: a wide stroke above.
    let (mut s, shadow) = shadow_case(green, 0.0, 50.0);
    let mut st = state(&[]);
    st.shadow = shadow;
    let mut line = BezPath::new();
    line.move_to((0.0, -25.0));
    line.line_to((100.0, -25.0));
    s.stroke(&line, &Stroke::new(50.0), &rgb(255, 0, 0), &st);
    for x in [1, 50, 98] {
        assert_eq!(px(&mut s, x, 25), [0, 255, 0, 255]);
    }
    // 2d.shadow.transform.2: the offset ignores the transform (rotate(π)).
    let (mut s, shadow) = shadow_case(green, 0.0, 50.0);
    let mut st = state(&[]);
    st.shadow = shadow;
    st.transform = Affine::rotate(std::f64::consts::PI);
    s.fill(&(st.transform * rect(-100.0, 0.0, 100.0, 50.0)), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 50, 25), [0, 255, 0, 255]);
    // 2d.shadow.composite.1: shadow and drawing composite one after the other.
    let (mut s, shadow) = shadow_case(Color::from_rgba8(255, 0, 0, 255), 100.0, 0.0);
    let mut st = state(&[]);
    st.shadow = shadow;
    st.composite = Composite::Xor;
    s.fill(&rect(-100.0, 0.0, 200.0, 50.0), false, &rgb(0, 255, 0), &st);
    assert_eq!(px(&mut s, 50, 25), [0, 255, 0, 255]);
    // 2d.shadow.outside: shadows from all four sides.
    let (mut s, _) = shadow_case(green, 0.0, 0.0);
    for (r, (x, y)) in [
        (rect(-100.0, 0.0, 25.0, 50.0), (100.0, 0.0)),
        (rect(175.0, 0.0, 25.0, 50.0), (-100.0, 0.0)),
        (rect(25.0, -100.0, 50.0, 25.0), (0.0, 100.0)),
        (rect(25.0, 125.0, 50.0, 25.0), (0.0, -100.0)),
    ] {
        let mut st = state(&[]);
        st.shadow = Some(Shadow { color: green, blur: 0.0, offset: Vec2::new(x, y) });
        s.fill(&r, false, &rgb(255, 0, 0), &st);
    }
    for (x, y) in [(12, 25), (87, 25), (50, 12), (50, 37)] {
        assert_eq!(px(&mut s, x, y), [0, 255, 0, 255]);
    }
}

#[test]
fn hostile_geometry_finishes() {
    let mut s = Surface::new(50, 50);
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.curve_to((1e300, 1e300), (-1e300, 1e300), (10.0, 10.0));
    s.fill(&path, false, &rgb(255, 0, 0), &state(&[]));
    s.stroke(&path, &Stroke::new(1e300), &rgb(255, 0, 0), &state(&[]));
    let _ = px(&mut s, 0, 0);
    // Tiny dashes along a long line, and a pen through a huge transform.
    let mut line = BezPath::new();
    line.move_to((0.0, 10.5));
    line.line_to((1e6, 10.5));
    s.stroke(&line, &Stroke::new(1.0).with_dashes(0.0, [1e-3, 1e-3]), &rgb(0, 0, 255), &state(&[]));
    assert_eq!(px(&mut s, 5, 10)[2], 255, "too many dashes draw solid");
    let mut st = state(&[]);
    st.transform = Affine::new([1e-9, 0.0, 0.0, 1e9, 0.0, 0.0]);
    s.stroke(&(st.transform * line.clone()), &Stroke::new(1e9).with_caps(kurbo::Cap::Round), &rgb(0, 0, 255), &st);
    // drawImage into a huge, rotated rectangle.
    let image = Bitmap::new(Pixmap::new(2, 2)).unwrap();
    st.transform = Affine::rotate(0.3);
    s.draw_image(&image, Rect::new(0.0, 0.0, 2.0, 2.0), Rect::new(-1e12, -1e12, 1e12, 1e12), &st);
    // Text at an absurd scale.
    let layout = text::layout(
        "W",
        &text::TextStyle {
            family: "sans-serif",
            size: 10.0,
            weight: 400.0,
            italic: false,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            lang: None,
            variant_caps: "normal",
            kerning: true,
        },
    );
    st.transform = Affine::scale(1e200);
    s.text(&layout, Point::ZERO, 1.0, None, &rgb(0, 0, 0), &st);
    let _ = px(&mut s, 0, 0);
    let mut big = Surface::new(MAX_SIDE + 1, 1);
    big.fill(&rect(0.0, 0.0, 5.0, 5.0), false, &rgb(255, 0, 0), &state(&[]));
    assert!(!big.is_allocated());
}

#[test]
fn encodes_png() {
    let mut s = Surface::new(3, 2);
    s.fill(&rect(0.0, 0.0, 3.0, 2.0), false, &rgb(255, 0, 0), &state(&[]));
    let png = s.encode(Encoding::Png).unwrap();
    let decoded = codec::decode(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (3, 2));
    assert_eq!(&decoded.data_as_u8_slice()[..4], &[255, 0, 0, 255]);
    assert!(s.encode(Encoding::Jpeg(0.9)).unwrap().starts_with(&[0xff, 0xd8]));
    assert!(s.encode(Encoding::Webp).unwrap().starts_with(b"RIFF"));
}

#[test]
fn text_has_ink_and_metrics() {
    let style = text::TextStyle {
        family: "sans-serif",
        size: 20.0,
        weight: 400.0,
        italic: false,
        letter_spacing: 0.0,
        word_spacing: 0.0,
        lang: None,
        variant_caps: "normal",
        kerning: true,
    };
    let layout = text::layout("Hello", &style);
    assert!(layout.width > 30.0 && layout.width < 80.0, "{}", layout.width);
    assert!(layout.ascent > 10.0 && layout.descent > 0.0);
    let ink = layout.ink.unwrap();
    assert!(ink.y0 < -10.0 && ink.y1 <= 1.0, "{ink:?}");
    let mut s = Surface::new(100, 40);
    s.text(&layout, Point::new(5.0, 30.0), 1.0, None, &rgb(0, 0, 0), &state(&[]));
    let data = s.get_image_data(0, 0, 100, 40);
    assert!(data.chunks_exact(4).filter(|p| p[3] > 128).count() > 50);
    assert_eq!(text::layout("", &style).width, 0.0);
}

#[test]
fn huge_sizes_and_offsets_do_not_overflow() {
    // A canvas size no bitmap can have: the pen limit is computed in f64.
    let mut s = Surface::new(u32::MAX, u32::MAX);
    let mut line = BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((1.0, 1.0));
    s.stroke(&line, &Stroke::new(1.0), &rgb(0, 0, 0), &state(&[]));
    assert!(!s.is_allocated());
    // Shadow offsets far past the bitmap (saturating casts) draw no shadow.
    let mut s = Surface::new(8, 8);
    for offset in [Vec2::new(0.0, -1e300), Vec2::new(1e300, 0.0), Vec2::new(-1e300, 1e300)] {
        let mut st = state(&[]);
        st.shadow = Some(Shadow { color: Color::from_rgba8(0, 0, 0, 255), blur: 0.0, offset });
        s.fill(&rect(2.0, 2.0, 2.0, 2.0), false, &rgb(255, 0, 0), &st);
    }
    assert_eq!(px(&mut s, 0, 0)[3], 0);
    assert_eq!(px(&mut s, 3, 3), [255, 0, 0, 255]);
}

#[test]
fn snapshots_keep_their_pixels_and_count() {
    let mut s = Surface::new(4, 4);
    s.fill(&rect(0.0, 0.0, 4.0, 4.0), false, &rgb(255, 0, 0), &state(&[]));
    let snap = s.snapshot().unwrap();
    assert_eq!(snap.byte_len(), 64);
    // A write copies the shared bitmap; the snapshot is unchanged.
    s.fill(&rect(0.0, 0.0, 4.0, 4.0), false, &rgb(0, 0, 255), &state(&[]));
    assert_eq!(&snap.data_as_u8_slice()[..4], &[255, 0, 0, 255]);
    assert_eq!(px(&mut s, 0, 0), [0, 0, 255, 255]);
    // A full clear while shared drops the surface's bitmap, not the snapshot's.
    let snap2 = s.snapshot().unwrap();
    s.clear_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Affine::IDENTITY, &[]);
    assert_eq!(px(&mut s, 0, 0)[3], 0);
    assert_eq!(&snap2.data_as_u8_slice()[..4], &[0, 0, 255, 255]);
}

#[test]
fn clear_operator() {
    assert_eq!(Composite::from_name("clear"), Some(Composite::Clear));
    assert_eq!(Composite::Clear.name(), "clear");
    let mut s = Surface::new(10, 10);
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &Paint::Color(Color::from_rgba8(0, 255, 0, 128)), &state(&[]));
    let mut st = state(&[]);
    st.composite = Composite::Clear;
    // The paint's colour and alpha do not matter: where it is drawn becomes
    // transparent black.
    s.fill(&rect(0.0, 0.0, 5.0, 10.0), false, &Paint::Color(Color::from_rgba8(0, 0, 255, 191)), &st);
    assert_eq!(px(&mut s, 2, 2), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, 7, 2), [0, 255, 0, 128], "outside the shape is kept");
}

#[test]
fn clear_operator_within_the_clip() {
    let mut s = Surface::new(10, 10);
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &rgb(0, 255, 0), &state(&[]));
    let clip = [ClipPath { path: rect(-20.0, -20.0, 10.0, 10.0), evenodd: false }];
    let mut st = state(&clip);
    st.composite = Composite::Clear;
    s.fill(&rect(0.0, 0.0, 5.0, 10.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 2, 2), [0, 255, 0, 255], "outside the clip is kept");
    let clip = [ClipPath { path: rect(0.0, 0.0, 3.0, 10.0), evenodd: false }];
    let mut st = state(&clip);
    st.composite = Composite::Clear;
    s.fill(&rect(0.0, 0.0, 5.0, 10.0), false, &rgb(255, 0, 0), &st);
    assert_eq!(px(&mut s, 1, 1), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, 4, 1), [0, 255, 0, 255]);
}

#[test]
fn transparent_image_source() {
    let dst = Rect::new(0.0, 0.0, 5.0, 5.0);
    let whole = Rect::new(0.0, 0.0, 5.0, 5.0);
    // Operators a transparent source leaves alone: nothing changes, nothing
    // is allocated.
    let mut s = Surface::new(10, 10);
    s.draw_transparent_image(5, 5, whole, dst, &state(&[]));
    assert!(!s.is_allocated());
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &Paint::Color(Color::from_rgba8(0, 255, 255, 128)), &state(&[]));
    for op in [Composite::SourceOver, Composite::DestinationOut, Composite::Xor, Composite::Lighter, Composite::Multiply] {
        let mut st = state(&[]);
        st.composite = op;
        s.draw_transparent_image(5, 5, whole, dst, &st);
        assert_eq!(px(&mut s, 2, 2), [0, 255, 255, 128], "{op:?}");
    }
    // Those that don't: transparent black, over the whole bitmap as for any
    // other source.
    for op in [Composite::Copy, Composite::SourceIn, Composite::SourceOut, Composite::DestinationIn] {
        let mut s = Surface::new(10, 10);
        s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &Paint::Color(Color::from_rgba8(0, 255, 255, 128)), &state(&[]));
        let mut st = state(&[]);
        st.composite = op;
        s.draw_transparent_image(5, 5, whole, dst, &st);
        assert_eq!(px(&mut s, 2, 2), [0, 0, 0, 0], "{op:?}");
        assert_eq!(px(&mut s, 7, 7), [0, 0, 0, 0], "{op:?}");
    }
    // destination-atop keeps the destination where the source is transparent
    // (Fa = 1 - aB, Fb = aA = 0): transparent black too.
    let mut s = Surface::new(10, 10);
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &Paint::Color(Color::from_rgba8(0, 255, 255, 128)), &state(&[]));
    let mut st = state(&[]);
    st.composite = Composite::DestinationAtop;
    s.draw_transparent_image(5, 5, whole, dst, &st);
    assert_eq!(px(&mut s, 2, 2), [0, 0, 0, 0]);
    // The clip still applies, and a source rectangle off the image draws nothing.
    let mut s = Surface::new(10, 10);
    s.fill(&rect(0.0, 0.0, 10.0, 10.0), false, &rgb(0, 255, 0), &state(&[]));
    let clip = [ClipPath { path: rect(0.0, 0.0, 3.0, 10.0), evenodd: false }];
    let mut st = state(&clip);
    st.composite = Composite::Copy;
    s.draw_transparent_image(5, 5, whole, dst, &st);
    assert_eq!(px(&mut s, 1, 1), [0, 0, 0, 0]);
    assert_eq!(px(&mut s, 7, 7), [0, 255, 0, 255]);
    let mut st = state(&[]);
    st.composite = Composite::Copy;
    s.draw_transparent_image(5, 5, Rect::new(6.0, 6.0, 8.0, 8.0), dst, &st);
    assert_eq!(px(&mut s, 7, 7), [0, 255, 0, 255]);
}
