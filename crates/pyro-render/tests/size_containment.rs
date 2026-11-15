//! Size containment (`contain: size`, css-contain-2 §3.1) with `contain-intrinsic-size`
//! (css-sizing-4 §4.1; blitz-dom `layout/mod.rs` `size_contained_size`): the box is sized as if
//! it were empty with a content of the intrinsic size override, and its content is laid out in
//! that box. Box sizes are read off the painted pixels, as in WPT's
//! `css/css-sizing/contain-intrinsic-size` reftests.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

const GREEN: [u8; 3] = [0, 128, 0];

/// The width and height of the green area that starts at the top left corner.
fn green_size(img: &RgbaImage) -> (usize, usize) {
    let at = |x: usize, y: usize| {
        let i = (y * img.width as usize + x) * 4;
        [img.data[i], img.data[i + 1], img.data[i + 2]] == GREEN
    };
    ((0..img.width as usize).take_while(|&x| at(x, 0)).count(), (0..img.height as usize).take_while(|&y| at(0, y)).count())
}

#[track_caller]
fn check(body: &str, want: (usize, usize)) {
    assert_eq!(green_size(&render(body)), want, "{body}");
}

/// A size-contained block is as tall as its intrinsic size override whatever it holds, and as
/// wide as a block is; shrink-to-fit takes the override's width.
#[test]
fn the_intrinsic_size_override_is_the_content_size() {
    let tall = "<div style='height:500px;width:300px'></div>";
    check(&format!("<div style='background:green;contain:size;contain-intrinsic-size:111px 50px'>{tall}</div>"), (400, 50));
    check(
        &format!("<div style='background:green;contain:size;contain-intrinsic-size:111px 50px;float:left'>{tall}</div>"),
        (111, 50),
    );
    check(
        &format!("<div style='width:max-content'><div style='background:green;contain:size;contain-intrinsic-size:111px 50px'>{tall}</div></div>"),
        (111, 50),
    );
    // Sizes win over the override; padding is added to it.
    check(
        &format!("<div style='background:green;contain:size;contain-intrinsic-size:111px 50px;width:60px;padding-bottom:10px'>{tall}</div>"),
        (60, 60),
    );
    // `auto <length>` is the length (no size is remembered).
    check(
        &format!("<div style='background:green;contain:size;contain-intrinsic-size:auto 80px auto 40px;float:left'>{tall}</div>"),
        (80, 40),
    );
}

/// Without an override a size-contained box is sized as an empty one, and without size
/// containment the override does nothing.
#[test]
fn size_containment_alone_and_the_override_alone() {
    check("<div style='background:green;contain:size;float:left;padding:10px'><div style='height:50px;width:50px'></div></div>", (20, 20));
    check(
        "<div style='background:green;contain-intrinsic-size:111px 50px;float:left'><div style='height:20px;width:30px'></div></div>",
        (30, 20),
    );
}

/// Flex and grid containers, and the logical properties in a vertical writing mode.
#[test]
fn other_boxes_with_size_containment() {
    for display in ["inline-flex", "inline-grid", "inline-block"] {
        check(
            &format!(
                "<div style='display:{display};vertical-align:top;background:green;contain:size;contain-intrinsic-size:70px 30px'>\
                 <div style='height:200px;width:200px'></div></div>"
            ),
            (70, 30),
        );
    }
    check(
        "<div style='float:left;writing-mode:vertical-rl;background:green;contain:size;\
         contain-intrinsic-inline-size:100px;contain-intrinsic-block-size:50px'></div>",
        (50, 100),
    );
}
