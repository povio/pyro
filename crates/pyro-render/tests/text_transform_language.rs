//! `text-transform` follows the language's case mappings (ICU's, as Chromium's): in Turkish `i`
//! upper-cases to dotted `İ` and dotless `ı` to `I` (WPT `CSS2/text/text-transform-bicameral-005`).
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
        html: format!("<!doctype html><meta charset=utf-8><style>body{{margin:8px;background:white;font:40px serif}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

#[test]
fn turkish_upper_cases_i_to_dotted_capital_i() {
    let upper =
        |lang: &str, text: &str| render(&format!("<div lang={lang} style='text-transform:uppercase'>{text}</div>"));
    let plain = |text: &str| render(&format!("<div>{text}</div>"));
    assert_eq!(upper("tr", "i").data, plain("\u{130}").data, "i is İ in Turkish");
    assert_eq!(upper("tr", "\u{131}").data, plain("I").data, "ı is I");
    assert_eq!(upper("en", "i").data, plain("I").data, "i is I in English");
    assert_ne!(plain("I").data, plain("\u{130}").data);
}
