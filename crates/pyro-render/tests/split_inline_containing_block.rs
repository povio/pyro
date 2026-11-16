//! The containing block of an absolutely positioned box in a positioned inline box split around
//! block-level content (CSS 2.1 § 10.1 4.1): the bounding box of the padding boxes of the inline's
//! first fragment (its head, at the end of the line before its block-level content) and its last
//! (its tail, on the line after), not the box blitz-dom lays out for the block-level content. An
//! empty head or tail is a point at the end or the start of its line, or at the top or the bottom
//! of the block-level content without such a line (Chromium). WPT
//! `css-position/position-absolute-in-inline-003`, `position-absolute-dynamic-relayout-005/006`.
use std::sync::Arc;

use pyro_render::*;

/// Serves Ahem (`tests/fonts`) as `ahem.ttf`.
struct AhemFetch;
impl Fetcher for AhemFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/ahem.ttf") {
            Ok(include_bytes!("fonts/Ahem.ttf").to_vec())
        } else {
            Err("offline".into())
        });
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn count(img: &RgbaImage, color: [u8; 3]) -> usize {
    img.data.chunks_exact(4).filter(|px| px[..3] == color).count()
}

const RED: [u8; 3] = [255, 0, 0];

#[test]
fn the_containing_block_starts_at_the_line_of_the_head() {
    // "B" ends the line at 60px; the span's empty head is on it, so the box 60px above its
    // containing block's top is at the container's top.
    let img = render(
        "<div style='height:100px;width:100px;background:red;font:20px/1 A;color:white'>
           <div style='height:60px'></div>B
           <span style='position:relative'><div></div>AA
             <div style='position:absolute;top:-60px;width:100px;height:100px;background:green'></div></span></div>",
    );
    assert_eq!(count(&img, RED), 0);
}

#[test]
fn a_split_inline_without_inline_content_is_a_zero_width_containing_block() {
    // The span holds nothing but a block: its head and tail are empty, a containing block 0px wide:
    // the red box 80% as wide is invisible, the green square in it overflows it.
    let img = render(
        "<span style='position:relative'><div>
           <div style='position:absolute;width:80%;background:red'>
           <div style='width:100px;height:100px;background:green'></div></div></div></span>",
    );
    assert_eq!(count(&img, RED), 0);
    assert_eq!(count(&img, [0, 128, 0]), 10000);
}
