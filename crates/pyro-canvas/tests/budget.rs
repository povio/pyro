//! The process's bitmap budget follows the pixels, not the surface that
//! drew them: its own test binary, since the budget is process-wide.

use pyro_canvas::kurbo::{Affine, Rect, Shape};
use pyro_canvas::{allocated_bytes, Bitmap, Color, Composite, DrawState, Paint, Pixmap, Surface};

fn state() -> DrawState<'static> {
    DrawState {
        transform: Affine::IDENTITY,
        alpha: 1.0,
        composite: Composite::SourceOver,
        clip: &[],
        shadow: None,
        smoothing: true,
    }
}

fn dot(s: &mut Surface) {
    let path = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1);
    s.fill(&path, false, &Paint::Color(Color::from_rgba8(255, 0, 0, 255)), &state());
}

#[test]
fn retained_bitmaps_count_toward_the_budget() {
    const BIG: usize = 16_384 * 2_048 * 4;
    assert_eq!(allocated_bytes(), 0);

    // `transferToImageBitmap` in a loop: each snapshot outlives the
    // surface's bitmap and keeps its count, so the loop stops at the cap.
    let mut kept = Vec::new();
    let mut surface = Surface::new(16_384, 2_048);
    for _ in 0..64 {
        dot(&mut surface);
        match surface.snapshot() {
            Some(bitmap) => kept.push(bitmap),
            None => break,
        }
        surface.resize(16_384, 2_048);
    }
    assert_eq!(kept.len(), 16, "2 GiB holds 16 bitmaps of 128 MiB");
    assert_eq!(allocated_bytes(), 16 * BIG);
    // Past the cap a new bitmap is refused.
    assert!(Bitmap::new(Pixmap::new(1, 1)).is_none());
    dot(&mut surface);
    assert!(!surface.is_allocated());

    // A temporary surface's snapshot (`createImageBitmap` resizing) keeps
    // its count after the surface is dropped.
    kept.pop();
    let resized = {
        let mut temp = Surface::new(16_384, 2_048);
        temp.draw_image(&kept[0], Rect::new(0.0, 0.0, 1.0, 1.0), Rect::new(0.0, 0.0, 2.0, 2.0), &state());
        temp.snapshot().unwrap()
    };
    assert_eq!(allocated_bytes(), 16 * BIG);

    drop(kept);
    drop(resized);
    drop(surface);
    assert_eq!(allocated_bytes(), 0);
}
