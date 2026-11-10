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
