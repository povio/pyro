//! A `<link>`'s `media` is its sheet's media list: a print sheet applies nothing on screen, a
//! screen sheet applies. (Its change after the sheet loaded is covered by WPT html/semantics
//! `the-link-element/stylesheet-media-change-no-fetch`: a static snapshot loads no change.)
use std::sync::Arc;

use pyro_render::*;

/// Serves `red.css`; anything else fails.
struct CssFetch;
impl Fetcher for CssFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/red.css") { Ok(b"body { background: #f00 }".to_vec()) } else { Err("offline".into()) });
    }
}

fn corner(media: &str) -> [u8; 3] {
    let fetcher: Arc<dyn Fetcher> = Arc::new(CssFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:#00f}}</style><link rel=stylesheet href=red.css{media}>"),
        base_url: "http://test/".into(),
        width: 100,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    [img.data[0], img.data[1], img.data[2]]
}

#[test]
fn a_link_media_attribute_limits_its_sheet() {
    assert_eq!(corner(""), [255, 0, 0], "no media: the sheet applies");
    assert_eq!(corner(" media=screen"), [255, 0, 0], "screen");
    assert_eq!(corner(" media=print"), [0, 0, 255], "print: not on screen");
}
