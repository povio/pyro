use pyro_dom::cssom::{ParsedRule, parse_one_rule};
use pyro_dom::stylo::declarations::DeclarationBlock;

#[test]
fn duplicate_names_in_unrelated_shadow_trees_cost_no_candidate_visits() {
    fn measure(count: usize) -> u64 {
        let mut html = String::from("<!doctype html><style>body{margin:0}</style>");
        for index in 0..count {
            html.push_str(&format!("<section id=h{index}><i style='display:block;anchor-name:--ref;width:99px'></i></section>"));
        }
        html.push_str("<i style='display:block;anchor-name:--ref;width:32px'></i><b style='position:absolute;left:anchor(--ref right)'></b>");
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        let marker = blitz_dom::QualName::new(None, blitz_dom::PYRO_STATE_NS.into(), "shadow-host".into());
        for index in 0..count {
            let host = doc.query_selector(&format!("#h{index}")).unwrap().unwrap();
            doc.mutate().set_attribute(host, marker.clone(), "");
        }
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        let target = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, 32.0);
        doc.anchor_candidate_visits()
    }
    let baseline = measure(0);
    assert!(baseline > 0);
    assert_eq!(measure(64), baseline);
}

#[test]
fn resolved_minimum_size_preserves_unanchored_auto() {
    let html = "<!doctype html><style>main{anchor-name:--ref;width:42px}b{position:absolute;min-width:anchor-size(--ref width);max-width:anchor-size(--missing width)}</style><main></main><b></b>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let reference = doc.query_selector("main").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    assert_eq!(doc.resolved_style_value(reference, "min-width"), "0px");
    assert_eq!(doc.resolved_style_value(target, "min-width"), "42px");
    assert_eq!(doc.resolved_style_value(target, "max-width"), "none");
}

#[test]
fn fallback_trials_stop_at_first_fit() {
    fn measure(options: &str) -> u64 {
        let html = format!(
            "<!doctype html><style>@position-try --good {{left:3px}} main{{width:100px;height:100px;position:relative}} b{{position:absolute;left:120px;width:15px;height:10px;position-try-fallbacks:{options}}}</style><main><b></b></main>"
        );
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        let id = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(id).unwrap().final_layout().location.x, 3.0);
        doc.anchor_try_visits()
    }
    let first = measure("--good");
    assert!(first > 0);
    assert_eq!(first, measure(&format!("--good{}", ", --missing".repeat(128))));
    assert!(measure(&format!("{}--good", "--missing, ".repeat(32))) <= 34 * first);
}

#[test]
fn scroll_compensation_does_not_repeat_name_lookup() {
    let html = "<!doctype html><style>main{position:relative}section{height:50px;overflow:auto}i{display:block;height:200px;anchor-name:--ref}b{position:absolute;position-anchor:--ref;top:anchor(bottom)}</style><main><section><i></i></section><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let visits = doc.anchor_candidate_visits();
    let scroller = doc.query_selector("section").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    for offset in 0..100 {
        doc.get_node_mut(scroller).unwrap().scroll_offset_mut().y = f64::from(offset);
        assert_eq!(doc.get_node(target).unwrap().anchor_scroll_offset().y, -f64::from(offset));
    }
    assert_eq!(visits, doc.anchor_candidate_visits());
}

#[test]
fn unrelated_names_do_not_increase_lookup_work() {
    fn measure(unrelated: usize, anchored: bool) -> u64 {
        let mut html =
            String::from("<!doctype html><style>body{margin:0}#ref{anchor-name:--ref;width:32px;height:14px}</style>");
        for index in 0..unrelated {
            html.push_str(&format!("<div style='anchor-name:--other{index}'></div>"));
        }
        html.push_str("<div id=ref></div>");
        if anchored {
            html.push_str("<b id=box style='position:absolute;left:anchor(--ref right);top:anchor(--ref bottom)'></b>");
        }
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        if anchored {
            let box_id = doc.query_selector("#box").unwrap().unwrap();
            let position = doc.get_node(box_id).unwrap().final_layout().location;
            assert_eq!((position.x, position.y), (32.0, 14.0));
        }
        doc.anchor_candidate_visits()
    }
    let visits = measure(0, true);
    assert!(visits > 0 && visits <= 8);
    assert_eq!(measure(256, true), visits);
    assert_eq!(measure(256, false), 0);
}

#[test]
fn anchor_positioning_descriptor_filter() {
    let rule = parse_one_rule(
        "@position-try --sample { color: red; --a: 1; left: anchor(right); bottom: 2px !important }",
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
        rule,
        ParsedRule::Other { rule_type: 0, text: "@position-try --sample { left: anchor(right); }".into() }
    );
    assert!(parse_one_rule("@position-try --one --two {}", &mut Vec::new()).is_err());
}

#[test]
fn anchor_positioning_fallback_nesting_is_bounded() {
    let value = |count: usize| format!("left: {}1px{}", "anchor(left, ".repeat(count), ")".repeat(count));
    let parsed = DeclarationBlock::parse(&value(12)).value("left");
    assert_eq!(parsed.matches("anchor(").count(), 12);
    assert!(DeclarationBlock::parse(&value(100)).is_empty());
}
