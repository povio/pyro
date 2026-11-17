//! List item numbers come from the `list-item` counter (CSS Lists 3 § 4.6): `li value`
//! (`counter-set`), `ol start` (`counter-reset`), `ol reversed` (`reversed(list-item)`, counting
//! down from the initial value of CSS Lists 3 § 4.4.2) and the counter properties of the items.
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
        html: format!("<!doctype html><style>body{{margin:0;font:20px/30px monospace}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data
}

fn same(test: &str, reference: &str) {
    assert!(render(test) == render(reference), "{test} should render as {reference}");
}

#[test]
fn value_start_and_reversed_number_the_items() {
    let three_two_one = "<ol><li value=3>x<li value=2>x<li value=1>x</ol>";
    assert!(render(three_two_one) != render("<ol><li>x<li>x<li>x</ol>"), "value numbers the items");
    same("<ol reversed><li>x<li>x<li>x</ol>", three_two_one);
    same("<ol reversed start=3><li>x<li>x<li>x</ol>", three_two_one);
    same("<ol style='counter-reset: reversed(list-item)'><li>x<li>x<li>x</ol>", three_two_one);
    same("<ol start=3><li>x<li>x</ol>", "<ol><li value=3>x<li value=4>x</ol>");
    // The initial value of a reversed list counts the items' increments up to the first set.
    same(
        "<ol reversed><li>x<li value=6>x<li>x</ol>",
        "<ol><li value=7>x<li value=6>x<li value=5>x</ol>",
    );
    same(
        "<ol reversed><li>x<li>x<li style='counter-increment: list-item -2'>x</ol>",
        "<ol><li value=5>x<li value=4>x<li value=2>x</ol>",
    );
    // `counter-set` and `counter-increment` on items.
    same(
        "<ol><li>x<li style='counter-increment: list-item 5'>x</ol>",
        "<ol><li>x<li value=6>x</ol>",
    );
    same("<ol><li style='counter-set: list-item 1'>x<li style='counter-set: list-item 1'>x</ol>", "<ol><li>x<li value=1>x</ol>");
}

#[test]
fn items_in_nested_blocks_continue_the_list() {
    let numbered = "<ol><li>x<li>x</ol>";
    assert!(render(numbered) != render("<ol><li>x<li value=1>x</ol>"));
    same("<ol><li>x</li><div><li>x</li></div></ol>", numbered);
}
