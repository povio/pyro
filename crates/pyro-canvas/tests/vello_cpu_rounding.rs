//! vendor/vello_cpu's division by 255 in its 8-bit pipeline, checked through the crate's public
//! API. The vendored crate's own unit tests (`src/util.rs`, `src/fine/lowp/mod.rs`) do not run in
//! the workspace (cargo tests no path dependency that has dev-dependencies), so the values they
//! pin are checked here too, where `cargo test -p pyro-canvas` runs them; pyro-render's
//! `compositing_rounding.rs` checks the same through pages.

use kurbo::Rect;
use peniko::color::{AlphaColor, Srgb};
use vello_cpu::{Pixmap, RenderContext, RenderSettings, Resources};

type Color = AlphaColor<Srgb>;

/// `v / 255` rounded to the nearest integer (a quotient by 255 is never a half).
fn round_div_255(v: u32) -> u32 {
    (v * 2 + 255) / 510
}

/// The premultiplied RGBA8 pixels of a `w` by `h` context after `draw`, single-threaded, in the
/// default render mode (the 8-bit pipeline pyro uses).
fn render(w: u16, h: u16, draw: impl FnOnce(&mut RenderContext)) -> Vec<[u8; 4]> {
    let mut ctx = RenderContext::new_with(w, h, RenderSettings { num_threads: 0, ..RenderSettings::default() });
    draw(&mut ctx);
    let mut pixmap = Pixmap::new(w, h);
    ctx.flush();
    ctx.render(&mut pixmap, &mut Resources::new());
    pixmap.data_as_u8_slice().chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()
}

fn fill(ctx: &mut RenderContext, color: Color, x0: f64, y0: f64, x1: f64, y1: f64) {
    ctx.set_paint(color);
    ctx.fill_rect(&Rect::new(x0, y0, x1, y1));
}

/// Every product of two bytes, `g * (255 - a)`: rows of opaque grey `g` under columns of black of
/// alpha `a`, each pixel covered whole. Upstream's `(v + 255) >> 8` is a level high for 26% of
/// them and a level low for 2%.
#[test]
fn every_byte_product_divides_by_255_to_nearest() {
    let pixels = render(256, 256, |ctx| {
        for g in 0..=255_u8 {
            fill(ctx, Color::from_rgba8(g, g, g, 255), 0.0, f64::from(g), 256.0, f64::from(g) + 1.0);
        }
        for a in 0..=255_u8 {
            fill(ctx, Color::from_rgba8(0, 0, 0, a), f64::from(a), 0.0, f64::from(a) + 1.0, 256.0);
        }
    });
    let mut wrong = Vec::new();
    for g in 0..=255_u32 {
        for a in 0..=255_u32 {
            let got = pixels[(g * 256 + a) as usize];
            let want = round_div_255(g * (255 - a)) as u8;
            if got != [want, want, want, 255] {
                wrong.push((g, a, got[0], want));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of 65536 products off, first (grey, alpha, got, want): {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
}

/// The pixel at (2, 2) of white with `layers` filled over it in turn, each over the whole 8 by 8
/// context (unmasked: the interior's fill) and from x = 0.5 (masked: (2, 2) is a covered pixel of
/// the edge's strip, composited through its coverage mask).
fn over_white(layers: &[Color]) -> ([u8; 4], [u8; 4]) {
    let at = |x0: f64| {
        render(8, 8, |ctx| {
            fill(ctx, Color::WHITE, 0.0, 0.0, 8.0, 8.0);
            for &layer in layers {
                fill(ctx, layer, x0, 0.0, 8.0, 8.0);
            }
        })[2 * 8 + 2]
    };
    (at(0.0), at(0.5))
}

/// Translucent colours over translucent colours come out as Chromium paints them, the opaque
/// colours WPT's box-shadow-multiple-001 and box-shadow-041 spell out. Upstream's division gave
/// `#FFD240` for the orange (and `#BF7F80` for the purple as the page paints it: pyro-render's
/// `compositing_rounding.rs`).
#[test]
fn translucent_colours_over_translucent_colours_round_to_nearest() {
    let half = |r, g, b| Color::from_rgba8(r, g, b, 128);
    let yellow = half(255, 255, 0);
    for (top, want) in [
        (half(128, 0, 128), [191, 127, 127, 255]), // purple: #BF7F7F
        (half(255, 165, 0), [255, 210, 63, 255]),  // orange: #FFD23F
        (half(0, 0, 255), [127, 127, 191, 255]),   // blue: #7F7FBF
        (half(255, 0, 255), [255, 127, 191, 255]), // fuchsia: #FF7FBF
    ] {
        let (plain, masked) = over_white(&[yellow, top]);
        assert_eq!(plain, want, "{top:?} over yellow over white");
        assert_eq!(masked, want, "{top:?} over yellow over white, through an edge's mask");
    }
    // Orange over blue: the inset shadow of box-shadow-041 and its reference's nested box.
    assert_eq!(over_white(&[half(0, 0, 255), half(255, 165, 0)]), ([191, 146, 127, 255], [191, 146, 127, 255]));
}

/// White seen through three layers of `.99` red fades to nothing, as in Chromium
/// (t32-opacity-zorder-c): upstream rounded 1 * 3 / 255 up to 1 for ever.
#[test]
fn a_translucent_layer_hides_a_level_of_what_lies_under_it() {
    let red = Color::from_rgba8(255, 0, 0, 252);
    assert_eq!(over_white(&[red, red, red]), ([255, 0, 0, 255], [255, 0, 0, 255]));
}
