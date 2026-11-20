//! A text control shows its value in its `text-transform` with full, context-aware case mapping
//! in the element's language (Unicode SpecialCasing), as inline text is: Greek final sigma,
//! Turkish dotted i, Lithuanian dot above.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const STYLE: &str = "<style>body{margin:0;background:white}
  textarea{display:block;box-sizing:border-box;width:380px;height:60px;margin:0 0 20px;border:0;
  padding:0 10px;font:32px Arial;background:white;color:black;outline:none;resize:none}</style>";

fn render(body: &str) -> RgbaImage {
    let s = Snapshot {
        html: format!("<!doctype html>{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 260,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn differing(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.data.chunks(4).zip(b.data.chunks(4)).filter(|(p, q)| p[..3] != q[..3]).count()
}

/// `test` renders as `reference` and unlike `wrong`.
fn shows(test: &str, reference: &str, wrong: &str) {
    let (t, r, w) = (render(test), render(reference), render(wrong));
    assert_eq!(differing(&t, &r), 0, "{test}\n  differs from\n{reference}");
    assert!(differing(&t, &w) > 0, "{test}\n  renders as the wrong mapping\n{wrong}");
}

#[test]
fn greek_capital_sigma_lowercases_to_final_sigma_at_a_words_end() {
    shows(
        "<textarea style='text-transform:lowercase'>ΟΣ ΟΣΟΣ</textarea>",
        "<textarea>ος οσος</textarea>",
        "<textarea>οσ οσοσ</textarea>",
    );
}

#[test]
fn turkish_and_lithuanian_casing_follow_the_language() {
    shows(
        "<textarea lang=tr style='text-transform:uppercase'>iı</textarea>",
        "<textarea>İI</textarea>",
        "<textarea>II</textarea>",
    );
    // Lithuanian keeps the dot of `i` under an accent: `Ì` lowercases to `i̇̀`.
    shows(
        "<textarea lang=lt style='text-transform:lowercase'>ÌX</textarea>",
        "<textarea>i&#x307;&#x300;x</textarea>",
        "<textarea>ìx</textarea>",
    );
}
