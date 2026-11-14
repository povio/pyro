//! A clip path, mask or filter reference cycle longer than usvg breaks (it sets a reference that
//! closes a cycle of one or two elements to `none`) was followed without end and overflowed the
//! stack: WPT css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg ended the
//! browser. The reference that closes such a cycle is dropped as usvg drops a short one.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

#[test]
fn reference_cycles_of_clip_paths_and_masks_render() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: r##"<!doctype html><body style="margin:0;background:#fff">
<svg width="200" height="100" style="display:block">
  <clipPath id="clip0"><rect width="1" height="1" clip-path="url(#clip)"/></clipPath>
  <clipPath id="clip2"><rect width="100" height="100" clip-path="url(#clip0)"/></clipPath>
  <clipPath id="clip"><rect width="1" height="1" clip-path="url(#clip2)"/></clipPath>
  <mask id="m1"><rect width="100" height="100" fill="white" mask="url(#m3)"/></mask>
  <mask id="m2"><rect width="100" height="100" fill="white" mask="url(#m1)"/></mask>
  <mask id="m3"><rect width="100" height="100" fill="white" mask="url(#m2)"/></mask>
  <circle r="50" clip-path="url(#clip)"/>
  <rect width="10" height="10" mask="url(#m1)"/>
  <rect x="150" y="50" width="50" height="50" fill="#0f0"/>
</svg>"##
            .into(),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let i = (75 * img.width as usize + 175) * 4;
    assert_eq!(img.data[i..i + 3], [0, 255, 0], "the SVG is drawn");
}
