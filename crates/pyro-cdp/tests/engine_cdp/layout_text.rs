//! Driver reads that depend on layout-computed text: generated content in
//! accessible names.
use super::*;

/// Names from content include generated `::before` / `::after` strings
/// (Playwright's `getPseudoContent`): spaced unless the pseudo-element is
/// inline, skipped when hidden or when nothing is generated.
#[test]
fn driver_accessible_name_includes_generated_content() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
#icon::before{content:"Close"}
#pre::before{content:"Step";display:block}#pre::after{content:"!"}
#hidden::before{content:"No";visibility:hidden}
#attr::after{content:attr(data-label)}
</style><body>
<button id="icon"></button>
<button id="pre">one</button>
<button id="hidden">Yes</button>
<button id="attr" data-label="Go">Next</button>
<button id="plain">Plain</button>
"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("icon", "name", json!({"css": "#icon"})),
            ("pre", "name", json!({"css": "#pre"})),
            ("hidden", "name", json!({"css": "#hidden"})),
            ("attr", "name", json!({"css": "#attr"})),
            ("plain", "name", json!({"css": "#plain"})),
            ("byName", "count", json!({"role": "button", "name": "Close", "exact": true})),
        ],
    );
    let want =
        json!({"icon": "Close", "pre": "Step one!", "hidden": "Yes", "attr": "NextGo", "plain": "Plain", "byName": 1});
    assert_eq!(reads, want);
}

/// A page without generated content skips the pseudo-element lookups; a
/// `::before` a script adds later still reaches the names (the document's
/// "may have generated content" flag is raised by the next style pass).
#[test]
fn driver_accessible_name_sees_generated_content_added_later() {
    let mut h = Harness::new();
    load_html(&mut h, r#"<!doctype html><body><button id="b">Go</button><button id="c">Stay</button>"#);
    let before = run_reads(
        &mut h,
        &[
            ("b", "name", json!({"css": "#b"})),
            ("count", "count", json!({"role": "button", "name": "Go", "exact": true})),
        ],
    );
    assert_eq!(before, json!({"b": "Go", "count": 1}));
    let script = r#"document.head.append(Object.assign(document.createElement('style'), {textContent: '#b::before{content:"Now "}'})), 1"#;
    let r = h.page("Runtime.evaluate", json!({"expression": script, "returnByValue": true}));
    assert_eq!(r["result"]["result"]["value"], json!(1), "{r}");
    let after = run_reads(
        &mut h,
        &[
            ("b", "name", json!({"css": "#b"})),
            ("c", "name", json!({"css": "#c"})),
            ("count", "count", json!({"role": "button", "name": "Now Go", "exact": true})),
        ],
    );
    assert_eq!(after, json!({"b": "Now Go", "c": "Stay", "count": 1}));
}

/// A span's padding and border are in its box: its client rect is its text
/// and both sides, a span over two lines has a rect per line, and a point in
/// its padding is in it.
#[cfg(feature = "render")]
#[test]
fn a_spans_padding_and_border_are_in_its_client_rects_and_hit() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>body{margin:0;font:20px/40px monospace}p{margin:0;width:8ch}
span{padding:6px 10px;border:2px solid}</style><body>
<p><span id="plain" style="padding:0;border:0">000</span></p>
<p id="p">0 <span id="one">000</span></p>
<p><span id="two">0000 0000</span></p>"#,
    );
    let reads = eval_value(
        &mut h,
        r#"(() => {
  const rect = (id) => document.getElementById(id).getBoundingClientRect();
  const [plain, one, two] = ['plain', 'one', 'two'].map(rect);
  const rects = [...document.getElementById('two').getClientRects()];
  const at = (x, y) => document.elementFromPoint(x, y).id;
  const mid = one.top + one.height / 2;
  return {
    // (Rects are in layout units, 1/64 px.)
    width: Math.round((one.width - plain.width) * 10) / 10,
    height: Math.round((one.height - plain.height) * 10) / 10,
    lines: rects.length,
    pitch: rects[1].top - rects[0].top,
    union: [two.top === rects[0].top, two.bottom === rects[1].bottom],
    hits: [at(one.left + 3, mid), at(one.right - 3, mid), at(one.left + one.width / 2, one.top + 3), at(one.left - 3, mid)],
  };
})()"#,
    );
    let want = json!({
        "width": 24, "height": 16, "lines": 2, "pitch": 40, "union": [true, true],
        "hits": ["one", "one", "one", "p"],
    });
    assert_eq!(reads, want);
}
