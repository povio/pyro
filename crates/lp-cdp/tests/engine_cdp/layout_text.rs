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
