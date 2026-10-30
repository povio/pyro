use std::sync::Arc;

use lp_render::worker::{match_baseline, Baseline, MatchOptions};
use lp_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn snap(html: &str) -> Snapshot {
    Snapshot {
        html: html.into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

const CARD: &str = r#"<!doctype html><html><head><style>
  body { margin: 0; background: #f3f4f6; }
  .card { width: 300px; height: 120px; margin: 40px; background: #6d28d9; }
</style></head><body><div class="card" id="card" data-lp-render-clip></div></body></html>"#;

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    img.data[i..i + 4].try_into().unwrap()
}

#[test]
fn renders_deterministically_on_the_cpu() {
    let mut r = Renderer::new();
    let (a, ha, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let (b, hb, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!(ha, hb);
    assert!(compare(&a, &b, 0, None).equal);
    assert_eq!(pixel(&a, 100, 100), [0x6d, 0x28, 0xd9, 255]);
    assert_eq!(decode_png(&encode_png(&a)).unwrap(), a);
}

#[test]
fn element_area_is_the_border_box() {
    let mut r = Renderer::new();
    let (img, _, _) = r.render(&snap(CARD), Area::Element, Backend::Cpu, &fetcher()).unwrap();
    assert_eq!((img.width, img.height), (300, 120));
    assert_eq!(pixel(&img, 0, 0), [0x6d, 0x28, 0xd9, 255]);
    assert_eq!(pixel(&img, 299, 119), [0x6d, 0x28, 0xd9, 255]);
}

#[test]
fn a_hash_hit_skips_raster_and_a_change_fails() {
    let mut r = Renderer::new();
    let (base, _, _) = r.render(&snap(CARD), Area::Viewport, Backend::Cpu, &fetcher()).unwrap();
    let b = Baseline::from_image(base, None);
    let opts = MatchOptions::default();
    let first = match_baseline(&mut r, &snap(CARD), Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(first.pass && !first.hash_hit);
    let second = match_baseline(&mut r, &snap(CARD), Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(second.pass && second.hash_hit && second.timings.raster.is_zero());
    let changed = snap(&CARD.replace("#6d28d9", "#6d28da"));
    let third = match_baseline(&mut r, &changed, Area::Viewport, Some(&b), opts, &fetcher()).unwrap();
    assert!(!third.pass && third.diff_pixels == 300 * 120, "{third:?}");
    assert!(third.actual.is_some() && third.diff.is_some());
    let tolerant = MatchOptions { tolerance: 1, ..opts };
    assert!(match_baseline(&mut r, &changed, Area::Viewport, Some(&b), tolerant, &fetcher()).unwrap().pass);
}

#[test]
fn sheets_and_images_come_through_the_bridge() {
    // A 1x1 red PNG served by the fetcher, a link sheet from the snapshot.
    let red = encode_png(&RgbaImage { width: 1, height: 1, data: vec![255, 0, 0, 255] });
    struct One(Vec<u8>);
    impl Fetcher for One {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            let body = self.0.clone();
            std::thread::spawn(move || done(if url.ends_with("red.png") { Ok(body) } else { Err("404".into()) }));
        }
    }
    let mut s = snap(
        r#"<!doctype html><head><link rel="stylesheet" href="http://test/a.css"></head><body><img src="red.png" width="50" height="50"></body>"#,
    );
    s.sheets.insert("http://test/a.css".into(), "body { margin: 0; background: #00ff00 }".into());
    let fetcher: Arc<dyn Fetcher> = Arc::new(One(red));
    let mut r = Renderer::new();
    let (img, _, t) = r.render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    assert_eq!(pixel(&img, 25, 25), [255, 0, 0, 255], "image painted ({t:?})");
    assert_eq!(pixel(&img, 200, 200), [0, 255, 0, 255], "linked sheet applied");
}
