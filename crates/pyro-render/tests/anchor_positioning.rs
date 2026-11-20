use pyro_dom::cssom::{ParsedRule, parse_one_rule};
use pyro_dom::stylo::declarations::DeclarationBlock;

#[test]
fn try_order_uses_containing_block_axes() {
    for (container, consumer, vertical) in [("horizontal-tb", "vertical-rl", false), ("vertical-rl", "horizontal-tb", true)] {
        for (order, wide) in [("most-width", true), ("most-height", false), ("most-inline-size", !vertical), ("most-block-size", vertical)] {
            let html = format!("<!doctype html><style>main{{position:relative;width:100px;height:100px;writing-mode:{container}}}b{{position:absolute;writing-mode:{consumer};left:200px;top:0;width:10px;height:10px;position-try-order:{order};position-try-fallbacks:--wide,--tall}}@position-try --wide{{left:0;top:60px}}@position-try --tall{{left:60px;top:0}}</style><main><b></b></main>");
            let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
            doc.viewport_mut().window_size = (400, 300);
            doc.resolve(0.0);
            let target = doc.get_node(doc.query_selector("b").unwrap().unwrap()).unwrap().final_layout();
            assert_eq!((target.location.x,target.location.y), if wide { (0.,60.) } else { (60.,0.) }, "{container} {order}");
        }
    }
}

#[test]
fn physical_anchor_geometry_is_bounded_across_writing_modes() {
    for mode in ["horizontal-tb", "vertical-lr", "vertical-rl"] {
        for depth in [1, 12] {
            let html = format!("<!doctype html><style>main{{position:relative;width:100px;height:80px;writing-mode:{mode}}}i{{display:block;width:5px;height:7px;anchor-name:--a}}b{{writing-mode:horizontal-tb;position:absolute;left:anchor(--a right);top:anchor(--a bottom);width:anchor-size(--a width);height:anchor-size(--a height)}}</style><main>{}<i></i>{}<b></b></main>", "<div>".repeat(depth), "</div>".repeat(depth));
            let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
            doc.viewport_mut().window_size = (400, 300);
            doc.resolve(0.0);
            let layout = |tag| doc.get_node(doc.query_selector(tag).unwrap().unwrap()).unwrap().final_layout();
            let target = layout("b");
            assert_eq!((target.size.width, target.size.height), (5., 7.), "{mode}");
            assert_eq!((target.location.x, target.location.y), (if mode == "vertical-rl" { 100. } else { 5. }, 7.), "{mode}");
            let steps = doc.anchor_geometry_steps();
            assert!(steps > 0 && steps <= (depth as u64 + 1) * doc.anchor_candidate_visits());
        }
    }
}

#[test]
fn transformed_anchor_walk_is_bounded_by_depth() {
    for depth in [4, 40] {
        let html = format!("<!doctype html><style>main{{position:relative;transform:scale(2)}}i{{display:block;width:20px;height:20px;anchor-name:--a;transform:scale(2);transform-origin:0 0}}b{{position:absolute;left:anchor(--a right);width:anchor-size(--a width)}}</style><main>{}<i></i>{}<b></b></main>", "<div style='transform:translateX(1px)'>".repeat(depth), "</div>".repeat(depth));
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        let anchor = doc.query_selector("i").unwrap().unwrap();
        let target = doc.query_selector("b").unwrap().unwrap();
        for width in [40.,60.] {
            if width == 60. { doc.set_style_property(anchor,"transform","scale(3)"); doc.resolve(0.0); }
            let layout = doc.get_node(target).unwrap().final_layout();
            assert_eq!((layout.location.x,layout.size.width),(depth as f32 + width,width));
        }
        assert!(doc.anchor_geometry_steps() > 0 && doc.anchor_geometry_steps() <= (depth as u64 + 1) * doc.anchor_candidate_visits());
    }
}

#[test]
fn moving_and_resizing_anchor_updates_geometry_in_one_resolve() {
    let html = "<!doctype html><style>body{margin:0}main{position:relative;width:100px;height:100px}i{display:block;width:10px;height:20px;anchor-name:--a}b{position:absolute;position-anchor:--a;top:anchor(bottom);width:anchor-size(width);height:20px;position-try-fallbacks:--up}@position-try --up{top:auto;bottom:anchor(top)}</style><main><i></i><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let anchor = doc.query_selector("i").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    let before = doc.anchor_try_visits();
    assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, 20.0);
    doc.set_style_property(anchor, "position", "relative");
    doc.set_style_property(anchor, "top", "75px");
    doc.set_style_property(anchor, "width", "37px");
    doc.resolve(0.0);
    let layout = doc.get_node(target).unwrap().final_layout();
    assert_eq!((layout.location.y, layout.size.width), (55.0, 37.0));
    assert!(doc.anchor_try_visits() - before <= 2);
}

#[test]
fn cyclic_anchor_references_do_not_schedule_another_pass() {
    let html = "<!doctype html><style>main{position:relative}i,b{position:absolute;width:10px;height:10px}i{anchor-name:--a;left:anchor(--b right,4px)}b{anchor-name:--b;left:anchor(--a right,8px)}</style><main><i></i><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let first = doc.query_selector("i").unwrap().unwrap();
    let second = doc.query_selector("b").unwrap().unwrap();
    assert_eq!(doc.get_node(first).unwrap().final_layout().location.x, 4.0);
    assert_eq!(doc.get_node(second).unwrap().final_layout().location.x, 14.0);
    let visits = doc.anchor_candidate_visits();
    assert!((2..=8).contains(&visits));
    doc.resolve(0.0);
    assert_eq!(doc.anchor_candidate_visits(), visits);
}

#[test]
fn intervening_scroll_keeps_a_fitting_option_without_retesting_the_list() {
    let html = "<!doctype html><style>main{position:relative;width:400px;height:399px}section{width:400px;height:400px;overflow:scroll}i{display:block;width:100px;height:100px;margin:300px 275px 300px 150px;anchor-name:--a}b{position:absolute;width:100px;height:100px;position-anchor:--a;left:anchor(left);bottom:anchor(top);position-try-fallbacks:--left,--right}@position-try --left{left:auto;bottom:auto;right:anchor(left);top:anchor(top)}@position-try --right{left:anchor(right);bottom:auto;top:anchor(top)}</style><main><section><i></i></section><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    let target = doc.query_selector("b").unwrap().unwrap();
    let scroller = doc.query_selector("section").unwrap().unwrap();
    for (offset, expected_x, max_trials) in [(201., 50., 2), (1., 50., 1), (0., 150., 1)] {
        let before = doc.anchor_try_visits();
        assert!(doc.set_timeline_scroll([(scroller,(0.,offset))].into(), (0.,0.)));
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, expected_x);
        assert!(doc.anchor_try_visits() - before <= max_trials);
    }
}

#[test]
fn scroll_fit_invalidation_coalesces_before_layout() {
    let html = "<!doctype html><style>section{width:100px;height:100px;overflow:auto;position:relative}i{display:block;height:25px;margin-top:75px;anchor-name:--a}b{position:absolute;width:20px;height:25px;position-anchor:--a;top:anchor(bottom);position-try-fallbacks:--up}@position-try --up{top:auto;bottom:anchor(top)}p{height:300px}</style><section><i></i><b></b><p></p></section><aside></aside>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let target = doc.query_selector("b").unwrap().unwrap();
    let scroller = doc.query_selector("section").unwrap().unwrap();
    let unrelated = doc.query_selector("aside").unwrap().unwrap();
    assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, 50.0);
    let trials = doc.anchor_try_visits();
    assert!(!doc.set_timeline_scroll([(unrelated, (0., 30.))].into(), (0., 0.)));
    for offset in 1..=50 {
        assert!(doc.set_timeline_scroll([(scroller, (0., f64::from(offset)))].into(), (0., 0.)));
    }
    assert_eq!(doc.anchor_try_visits(), trials);
    doc.resolve(0.0);
    assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, 100.0);
    assert!(doc.anchor_try_visits() <= trials + 1);
    assert!(!doc.set_timeline_scroll([(scroller, (0., 50.))].into(), (0., 0.)));
}

#[test]
fn scroll_fit_splits_area_at_visual_anchor_edges() {
    let html = "<!doctype html><style>main{position:relative;width:200px;height:200px}section{width:200px;height:200px;overflow:scroll}i{display:block;anchor-name:--a;height:50px;margin:150px 0}b{position:absolute;position-anchor:--a;position-area:bottom;position-try-fallbacks:flip-block;width:50px;height:50px}</style><main><section><i></i></section><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let scroller = doc.query_selector("section").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, 100.0);
    for (scroll, expected) in [(101., 200.), (40., 100.)] {
        let before = doc.anchor_try_visits();
        assert!(doc.set_timeline_scroll([(scroller, (0., scroll))].into(), (0., 0.)));
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, expected);
        assert!(doc.anchor_try_visits() - before <= 1);
    }
}

#[test]
fn changed_rule_media_updates_only_subscribed_blocks() {
    use blitz_dom::{local_name, ns, QualName};
    let html = "<!doctype html><style>section{position:relative;width:100px;height:100px}b{position:absolute;left:999px;width:10px;height:10px;position-try-fallbacks:--changed}aside b{position-try-fallbacks:--stable}@position-try --stable{left:4px}</style><style id=sheet media=print>@position-try --changed{left:8px}</style><section><b id=target></b></section><aside><section><b></b></section></aside>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let target = doc.query_selector("#target").unwrap().unwrap();
    let sheet = doc.query_selector("#sheet").unwrap().unwrap();
    let media = QualName::new(None, ns!(), local_name!("media"));
    assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, 999.0);
    for (value, expected) in [("screen", 8.0), ("print", 999.0)] {
        let before = doc.anchor_try_visits();
        let list_visits = doc.anchor_list_visits();
        doc.mutate().set_attribute(sheet, media.clone(), value);
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, expected);
        assert_eq!(doc.anchor_list_visits() - list_visits, 1);
        assert_eq!(doc.anchor_try_visits() - before, u64::from(value == "screen"));
    }
}

#[test]
fn fallback_budget_is_shared_with_container_query_relayout() {
    let html = "<!doctype html><style>section{position:relative;container-type:inline-size;width:100px;height:100px}b{position:absolute;left:999px;width:10px;height:10px;position-try-fallbacks:--bad,--bad,--bad,--bad,--bad}@position-try --bad{left:800px}@container(min-width:80px){b{width:31px}}</style><section><b></b></section>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let target = doc.query_selector("b").unwrap().unwrap();
    assert_eq!(doc.get_node(target).unwrap().final_layout().size.width, 31.0);
    assert_eq!(doc.anchor_try_visits(), 5);
}

#[test]
fn later_absolute_fragment_does_not_expose_its_inflow_anchor() {
    for prior in [false,true] {
        let html = format!("<!doctype html><style>main{{position:relative;width:200px;height:100px}}i,em{{display:block;width:20px;height:10px;anchor-name:--a}}section{{position:absolute;left:80px;top:0}}b{{position:absolute;left:anchor(--a right,7px);top:0;width:10px;height:10px}}</style><main>{}<b></b><section><em></em></section></main>", if prior { "<i></i>" } else { "" });
        let mut doc = blitz_html::HtmlDocument::from_html(&html, Default::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        let section = doc.query_selector("section").unwrap().unwrap();
        let target = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, if prior {20.} else {7.});
        doc.set_style_property(section,"position","relative");
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x,100.);
    }
}

#[test]
fn indexed_lookup_excludes_fixed_siblings_hoisted_past_the_query_container() {
    let html = "<!doctype html><style>section{position:relative}i,em{display:block;anchor-name:--a}i{width:21px}em{position:fixed;width:51px}b{position:absolute;width:anchor-size(--a width,1px)}</style><section><i></i><em></em><b></b></section>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let target = doc.query_selector("b").unwrap().unwrap();
    assert_eq!(doc.get_node(target).unwrap().final_layout().size.width, 21.0);
    let later = doc.query_selector("em").unwrap().unwrap();
    doc.set_style_property(later, "position", "relative");
    doc.resolve(0.0);
    assert_eq!(doc.get_node(target).unwrap().final_layout().size.width, 51.0);
    doc.mutate().remove_node(later);
    doc.resolve(0.0);
    assert_eq!(doc.get_node(target).unwrap().final_layout().size.width, 21.0);
}

#[test]
fn duplicate_mixed_parent_groups_use_bounded_predecessor_queries() {
    fn measure(count: usize) -> (u64, u64) {
        let mut html = String::from("<!doctype html><style>section{position:relative}i,em,b{position:absolute;height:1px}i,em{anchor-name:--a}em{width:211px}b{width:anchor-size(--a width,1px)}</style>");
        for index in 0..count {
            html.push_str(&format!("<section><i style='width:{}px'></i><b id=b{index}></b><em></em></section>", index + 2));
        }
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        for index in 0..count {
            let node = doc.query_selector(&format!("#b{index}")).unwrap().unwrap();
            assert_eq!(doc.get_node(node).unwrap().final_layout().size.width, (index + 2) as f32);
        }
        (doc.anchor_candidate_visits(), doc.anchor_index_work())
    }
    assert_eq!(measure(0), (0, 0));
    let small = measure(32);
    let large = measure(128);
    assert!(small.1 > 0 && large.0 <= small.0 * 4 && large.1 <= small.1 * 8, "{small:?} {large:?}");
}

#[test]
fn duplicate_sibling_names_and_consumers_scale_linearly() {
    fn visits(count: usize) -> u64 {
        let html = format!("<!doctype html><style>main{{position:relative}}i{{display:block;height:1px;anchor-name:--a}}b{{position:absolute;top:anchor(--a bottom)}}</style><main>{}{}</main>", "<i></i>".repeat(count), "<b></b>".repeat(count));
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        let last = doc.query_selector("b:last-child").unwrap().unwrap();
        assert_eq!(doc.get_node(last).unwrap().final_layout().location.y, count as f32);
        doc.anchor_candidate_visits()
    }
    let small = visits(32);
    assert!((32..=256).contains(&small));
    assert!(visits(128) <= small * 4 + 4);
}

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
fn only_five_valid_fallbacks_get_geometry_trials() {
    for (bad, missing, expected) in [(4, 32, 2.0), (5, 0, 200.0), (200, 0, 200.0)] {
        let list = format!("{}{}--good", "--absent,".repeat(missing), "--bad,".repeat(bad));
        let html = format!("<!doctype html><style>main{{position:relative;width:100px;height:100px}}b{{position:absolute;width:10px;height:10px;left:200px;position-try-fallbacks:{list}}}@position-try --bad{{left:201px}}@position-try --good{{left:2px}}</style><main><b></b></main>");
        let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
        doc.viewport_mut().window_size = (400, 300);
        doc.resolve(0.0);
        let target = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x, expected);
        assert_eq!(doc.anchor_try_visits(), 5);
        assert_eq!(doc.anchor_list_visits(), missing as u64 + 5);
    }
}

#[test]
fn valid_options_are_shared_but_trial_styles_are_not() {
    for count in [16, 64] {
        let list = format!("{}--fit", "--absent,".repeat(count));
        let html = format!("<!doctype html><style>main{{position:relative;width:100px;height:80px}}b{{position:absolute;left:200px;width:5px;height:5px;position-try-fallbacks:{list}}}@position-try --fit{{left:var(--x)}}</style><main>{}</main>", (0..count).map(|index| format!("<b id=t{index} style='--x:{index}px'></b>")).collect::<String>());
        let mut doc = blitz_html::HtmlDocument::from_html(&html, Default::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        assert_eq!(doc.anchor_list_visits(), count as u64 + 1);
        assert_eq!(doc.anchor_try_visits(), count as u64);
        for index in 0..count {
            let id = doc.query_selector(&format!("#t{index}")).unwrap().unwrap();
            assert_eq!(doc.get_node(id).unwrap().final_layout().location.x,index as f32);
        }
    }
}

#[test]
fn scope_index_stores_declarations_not_name_ancestor_pairs() {
    for count in [16,64] {
        let mut html = "<!doctype html><main style='anchor-scope:all;position:relative'>".to_string();
        html.push_str(&"<section>".repeat(count));
        for index in 0..count {
            html.push_str(&format!("<div style='anchor-scope:--a{index}'><i style='anchor-name:--a{index};display:block;width:1px'></i><i style='anchor-name:--a{index};display:block;width:3px'></i><b style='position:absolute;left:anchor(--a{index} right)'></b></div>"));
        }
        let mut doc = blitz_html::HtmlDocument::from_html(&html, Default::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        let id = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(id).unwrap().final_layout().location.x,3.);
        assert_eq!(doc.anchor_boundary_entries(), 2 * (count + 1));
    }
}

#[test]
fn visibility_chain_is_solved_once_per_geometry_generation() {
    for count in [16,64] {
        let mut html = "<!doctype html><style>main{position:relative}i,b{display:block;width:3px;height:3px}b{position:absolute;top:anchor(bottom);position-visibility:anchor-visible}</style><main><i style='anchor-name:--a0'></i>".to_string();
        for index in 1..=count {
            html.push_str(&format!("<b id=t{index} style='anchor-name:--a{index};position-anchor:--a{}'></b>",index-1));
        }
        let mut doc = blitz_html::HtmlDocument::from_html(&html, Default::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        for _ in 0..2 {
            let root = doc.root_element().id;
            doc.get_node_mut(root).unwrap().scroll_offset_mut().y += 1.;
            let before = doc.anchor_clip_steps();
            for index in (1..=count).rev() {
                let id = doc.query_selector(&format!("#t{index}")).unwrap().unwrap();
                assert!(!doc.get_node(id).unwrap().hidden_by_anchor());
            }
            assert_eq!(doc.anchor_clip_steps() - before, count + 1);
        }
    }
}

#[test]
fn only_rendered_fallback_is_remembered_on_resize() {
    for rendered in [false,true] {
        let html = "<!doctype html><style>main{position:relative;width:100px;height:100px}b{position:absolute;left:120px;top:0;width:10px;height:10px;position-try-fallbacks:--fit}@position-try --fit{left:40px}</style><main><b></b></main>";
        let mut doc = blitz_html::HtmlDocument::from_html(html, Default::default());
        doc.viewport_mut().window_size = (400,300);
        doc.resolve(0.0);
        let main = doc.query_selector("main").unwrap().unwrap();
        let target = doc.query_selector("b").unwrap().unwrap();
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x,40.);
        if rendered { doc.commit_anchor_frame(); }
        for (width,sync_x) in [(30,120.),(90,40.),(170,120.),(30,120.)] {
            doc.set_style_property(main,"width",&format!("{width}px"));
            doc.resolve(0.0);
            assert_eq!(doc.get_node(target).unwrap().final_layout().location.x,if rendered {40.} else {sync_x});
        }
        doc.set_style_property(target,"position-try-fallbacks","--fit, --other");
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.x,120.);
    }
}

#[test]
fn forced_layouts_at_one_frame_time_get_fresh_fallback_budgets() {
    let html = "<!doctype html><style>main{position:relative;width:100px;height:100px}i{display:block;position:relative;anchor-name:--a;width:10px;height:10px}b{position:absolute;position-anchor:--a;top:anchor(bottom);height:20px;width:5px;position-try-fallbacks:--up}@position-try --up{top:auto;bottom:anchor(top)}</style><main><i></i><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    let anchor = doc.query_selector("i").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    for step in 0..512 {
        let use_fallback = step % 2 == 1;
        let before = doc.anchor_try_visits();
        doc.set_style_property(anchor, "top", if use_fallback { "85px" } else { "0px" });
        doc.resolve(0.0);
        assert_eq!(doc.get_node(target).unwrap().final_layout().location.y, if use_fallback { 65.0 } else { 10.0 });
        assert_eq!(doc.anchor_try_visits() - before, u64::from(use_fallback));
    }
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
fn clipped_scroll_axis_and_removed_scrollport_ignore_stale_offsets() {
    let html = "<!doctype html><style>main{position:relative}section{overflow:clip scroll;width:50px;height:50px}i{display:block;anchor-name:--a;width:120px;height:120px}b{position:absolute;position-anchor:--a;left:anchor(left);top:anchor(top)}</style><main><section><i></i></section><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let scroller = doc.query_selector("section").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    let sample = |doc: &blitz_dom::BaseDocument| {
        let reads = std::cell::Cell::new(0);
        let delta = doc.get_node(target).unwrap().anchor_scroll_offset_with(|id| {
            assert_eq!(id, Some(scroller));
            reads.set(reads.get() + 1);
            blitz_dom::Point { x: 11.0, y: 7.0 }
        });
        (delta.x, delta.y, reads.get())
    };
    assert_eq!(sample(&doc), (0.0, -7.0, 1));
    let visits = doc.anchor_candidate_visits();
    for _ in 0..64 { assert_eq!(sample(&doc), (0.0, -7.0, 1)); }
    assert_eq!(visits, doc.anchor_candidate_visits());
    doc.set_style_property(scroller, "overflow", "visible");
    doc.resolve(0.0);
    assert_eq!(sample(&doc), (0.0, 0.0, 0));
}

#[test]
fn transformed_scroll_offset_is_a_vector_with_one_source_read() {
    let html = "<!doctype html><style>main{position:relative}section{transform:translateY(40px) scale(2,3);overflow:auto;width:50px;height:50px}i{display:block;width:200px;height:200px;anchor-name:--a}b{position:absolute;position-anchor:--a;position-area:right}</style><main><section><i></i></section><b></b></main>";
    let mut doc = blitz_html::HtmlDocument::from_html(html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 300);
    doc.resolve(0.0);
    let scroller = doc.query_selector("section").unwrap().unwrap();
    let target = doc.query_selector("b").unwrap().unwrap();
    let reads = std::cell::Cell::new(0);
    let visits = doc.anchor_candidate_visits();
    for amount in 1..=64 {
        let delta = doc.get_node(target).unwrap().anchor_scroll_offset_with(|id| {
            assert_eq!(id, Some(scroller));
            reads.set(reads.get() + 1);
            blitz_dom::Point { x: amount.into(), y: amount.into() }
        });
        assert_eq!((delta.x, delta.y), (f64::from(-2 * amount), f64::from(-3 * amount)));
    }
    assert_eq!(reads.get(), 64);
    assert_eq!(visits, doc.anchor_candidate_visits());
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
