//! A `<style>` element's sheet is its text as it is: an HTML `<style>` is raw text and an XML
//! CDATA section is literal, so a character reference left in the text (`&lt;`, `&#x46;`) is
//! not decoded again (WPT css/CSS2/generated-content/content-177).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Whether each 20px row of `rows` is drawn green under the sheet `css`.
fn green(css: &str, rows: &[&str]) -> Vec<bool> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let body: String = rows.iter().map(|r| format!("<div style='height:20px'>{r}</div>")).collect();
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:#fff}} i{{display:block;height:20px}} {css}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 40,
        height: 20 * rows.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    (0..rows.len())
        .map(|row| {
            let i = ((row * 20 + 10) * img.width as usize + 10) * 4;
            img.data[i..i + 3] == [0, 255, 0]
        })
        .collect()
}

#[test]
fn character_references_in_style_text_stay_literal() {
    // The sheet's attribute selectors compare with the attribute values the parser decoded:
    // `&amp;lt;` in markup is the four characters `&lt;`.
    let css = "i[title='&lt;'], i[title='&#x46;'] { background: #0f0 }";
    let rows = ["<i title='&amp;lt;'></i>", "<i title='&amp;#x46;'></i>", "<i title='&lt;'></i>", "<i title='F'></i>"];
    assert_eq!(green(css, &rows), [true, true, false, false]);
}
