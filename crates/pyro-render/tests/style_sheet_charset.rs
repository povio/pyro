//! A linked sheet's text comes from the page, decoded already (CSS Syntax: byte order mark, HTTP
//! charset, `@charset`, then the link's or document's encoding), and must not be decoded again by
//! the `@charset` it starts with: WPT css/CSS2/syntax/at-charset-07x, where an ISO-8859-1 sheet's
//! `.tést` selector came out as `.tÃ©st`.
use std::collections::HashMap;
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

#[test]
fn a_linked_sheet_is_not_decoded_again_by_its_charset_rule() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    for charset in ["iso-8859-1", "shift_jis", "utf-8"] {
        let snapshot = Snapshot {
            html: "<!doctype html><link rel=stylesheet href='http://test/s.css' data-pyro-sheet=s>\
                   <body style='margin:0;background:#fff'><div class='t\u{e9}st' style='width:40px;height:40px'></div>"
                .into(),
            base_url: "http://test/".into(),
            sheets: HashMap::from([(
                "s".to_owned(),
                format!("@charset \"{charset}\";\n.t\u{e9}st {{ background: #0f0 }}"),
            )]),
            width: 100,
            height: 100,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
        let i = (20 * img.width as usize + 20) * 4;
        assert_eq!(img.data[i..i + 3], [0, 255, 0], "@charset {charset}: the rule applies");
    }
}
