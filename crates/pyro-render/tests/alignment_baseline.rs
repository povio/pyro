//! The `alignment-baseline` longhand (and an inherited `dominant-baseline`) align a span's baseline
//! of that name with its parent's (css-inline-3, as Blink does), while `vertical-align`'s keywords
//! keep CSS 2.1's alignments: `alignment-baseline: middle` on a span of the parent's font moves
//! nothing, `vertical-align: middle` moves it.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> Vec<u8> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;font:60px/2 monospace}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data
}

#[test]
fn the_longhand_aligns_baselines_of_its_name() {
    let plain = render("<div>X<span>X</span></div>");
    for value in ["middle", "text-top", "text-bottom"] {
        assert!(
            render(&format!("<div>X<span style='alignment-baseline:{value}'>X</span></div>")) == plain,
            "alignment-baseline: {value}"
        );
        assert!(
            render(&format!("<div style='dominant-baseline:{value}'>X<span>X</span></div>")) == plain,
            "dominant-baseline: {value}"
        );
    }
    assert!(render("<div>X<span style='vertical-align:middle'>X</span></div>") != plain, "vertical-align: middle");
    assert!(render("<div>X<span style='vertical-align:text-top'>X</span></div>") != plain, "vertical-align: text-top");
}
