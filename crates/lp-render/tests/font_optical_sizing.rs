#![cfg(target_os = "macos")]
use lp_render::*;
use std::sync::Arc;
struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}
fn image(extra: &str) -> RgbaImage {
    use_fork_safe_fonts();
    let html=format!("<!doctype html><style>body{{margin:0;background:white;font-family:system-ui;font-size:13px;font-weight:500;{extra}}}</style>TOS Platform");
    let snap = Snapshot {
        html,
        base_url: "http://test/".into(),
        width: 320,
        height: 90,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap().0
}
#[test]
fn optical_auto_matches_explicit_font_size_and_differs_from_none() {
    let auto = image("");
    let explicit = image("font-variation-settings:'opsz' 13");
    assert!(compare(&auto, &explicit, 0, None).equal, "default auto must use the font size for its optical axis");
    assert!(
        !compare(&auto, &image("font-optical-sizing:none"), 0, None).equal,
        "none must retain the font's default optical size"
    );
}
#[test]
fn explicit_optical_axis_overrides_auto_and_none() {
    let expected = image("font-optical-sizing:none");
    assert!(compare(&expected, &image("font-variation-settings:'opsz' 28"), 0, None).equal);
    assert!(compare(&expected, &image("font-optical-sizing:none;font-variation-settings:'opsz' 28"), 0, None).equal);
}
