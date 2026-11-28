use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use pyro_render::{Area, Backend, Fetcher, Renderer, Snapshot};
use std::sync::Arc;

const SOURCE: &str = r#"<!doctype html><style>
body {margin:0} #outer {container: shell / inline-size; width:240px; padding:20px; border:5px solid black; box-sizing:content-box}
#target {width:10px;height:20px;background:red}
@container shell (width >= 230px) {#target {width:80px;background:lime}}
@container shell (width >= 280px) {#target {width:120px;background:blue}}
#inner {container: inner / inline-size;width:50%;height:40px}
#nested {width:10px;height:10px;background:blue}
@container inner (width >= 100px) {#nested {width:60px}}
#vertical {container-type:inline-size;writing-mode:vertical-rl;width:60px;height:160px}
#verticalTarget {width:5px;height:5px}
@container (height >= 150px) {#verticalTarget{height:30px}}
@container (width >= 50px) {#verticalTarget{width:40px}}
#responsive {container-type:inline-size;width:80vw}
#responsiveTarget {width:5px;height:5px}
@container (width >= 600px) {#responsiveTarget{width:50px}}
</style><div id=outer><div id=target></div><div id=inner><div id=nested></div></div></div><div id=vertical><div id=verticalTarget></div></div><div id=responsive><div id=responsiveTarget></div></div>"#;

fn size(doc: &HtmlDocument, selector: &str) -> (f32, f32) {
    let id = doc.query_selector(selector).unwrap().unwrap();
    let size = doc.get_node(id).unwrap().final_layout().size;
    (size.width, size.height)
}

#[test]
fn entering_size_containment_restyles_existing_descendants() {
    let mut doc = HtmlDocument::from_html("<style>#outer {width:200px;contain:strict} #inner{width:15px} @container shell (min-width:100px){#inner{width:50px}}</style><section id=outer><div id=inner></div></section>", DocumentConfig::default());
    doc.resolve(0.0);
    let outer = doc.query_selector("#outer").unwrap().unwrap();
    assert_eq!(size(&doc, "#inner").0, 15.0);
    doc.set_style_property(outer, "container", "shell / size");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#inner").0, 50.0);
}

#[test]
fn renderer_container_queries_restyle_after_layout_resize_and_hidden_restore() {
    let mut doc = HtmlDocument::from_html(SOURCE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#outer"), (290.0, 110.0));
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    assert_eq!(size(&doc, "#inner"), (120.0, 40.0));
    assert_eq!(size(&doc, "#nested"), (60.0, 10.0));
    let outer = doc.query_selector("#outer").unwrap().unwrap();
    doc.set_style_property(outer, "width", "180px");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));
    assert_eq!(size(&doc, "#nested"), (10.0, 10.0));
    doc.set_style_property(outer, "display", "none");
    doc.resolve(0.0);
    doc.set_style_property(outer, "width", "240px");
    doc.set_style_property(outer, "display", "block");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    assert_eq!(size(&doc, "#nested"), (60.0, 10.0));
}

#[test]
fn renderer_container_queries_respect_writing_mode_axis_and_viewport_changes() {
    let mut doc = HtmlDocument::from_html(SOURCE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#verticalTarget"), (5.0, 30.0));
    assert_eq!(size(&doc, "#responsiveTarget"), (50.0, 5.0));
    doc.viewport_mut().window_size = (500, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#responsiveTarget"), (5.0, 5.0));
}

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

#[test]
fn renderer_container_queries_paint_the_matched_rule() {
    let snapshot = Snapshot {
        html: SOURCE.into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let (image, _, _) = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
    let offset = (35 * image.width as usize + 50) * 4;
    assert_eq!(&image.data[offset..offset + 4], &[0, 255, 0, 255]);
}

#[test]
fn renderer_container_queries_converge_when_an_outer_rule_resizes_a_named_inner_container() {
    let mut doc = HtmlDocument::from_html(
        r#"<!doctype html><style>
        #outer {container:outer / inline-size; width:900px}
        #inner {container:inner / inline-size; width:200px}
        #target {width:10px;height:20px}
        @container outer (width >= 800px) {#inner {width:600px}}
        @container inner (width >= 500px) {#target {width:60px}}
        </style><div id=outer><div id=inner><div id=target></div></div></div>"#,
        DocumentConfig::default(),
    );
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#inner"), (600.0, 20.0));
    assert_eq!(size(&doc, "#target"), (60.0, 20.0));
    let outer = doc.query_selector("#outer").unwrap().unwrap();
    doc.set_style_property(outer, "display", "none");
    doc.resolve(0.0);
    doc.set_style_property(outer, "width", "400px");
    doc.set_style_property(outer, "display", "block");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#inner"), (200.0, 20.0));
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));
}

#[test]
fn style_only_reads_follow_actual_container_style_and_viewport_changes() {
    let mut doc = HtmlDocument::from_html(SOURCE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    let outer = doc.query_selector("#outer").unwrap().unwrap();
    doc.set_style_property(outer, "width", "180px");
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));
    assert_eq!(size(&doc, "#nested"), (10.0, 10.0));
    doc.set_style_property(outer, "width", "300px");
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#target"), (120.0, 20.0));
    doc.viewport_mut().window_size = (500, 600);
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#responsiveTarget"), (5.0, 5.0));
}

#[test]
fn style_only_reads_without_container_queries_never_force_layout() {
    let mut doc = HtmlDocument::from_html(
        "<!doctype html><style>#target{width:80px;height:20px}</style><div id=target></div>",
        DocumentConfig::default(),
    );
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    let before = pyro_render::blitz_passes().1;
    let target = doc.query_selector("#target").unwrap().unwrap();
    doc.set_style_property(target, "width", "120px");
    doc.resolve_style(0.0);
    assert!(doc.get_node(target).unwrap().primary_styles().is_some());
    assert_eq!(pyro_render::blitz_passes().1, before);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (120.0, 20.0));
}

#[test]
fn removing_and_readding_container_rules_updates_style_only_layout_dependencies() {
    let mut doc = HtmlDocument::from_html(SOURCE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    let style_node = doc.query_selector("style").unwrap().unwrap();
    let plain = doc.make_stylesheet("#target{width:10px;height:20px}", style::stylesheets::Origin::Author);
    doc.add_stylesheet_for_node(plain, style_node);
    let before = pyro_render::blitz_passes().1;
    doc.resolve_style(0.0);
    assert_eq!(pyro_render::blitz_passes().1, before, "removing the last query must restore style-only resolution");
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));

    let css = SOURCE.split("<style>").nth(1).unwrap().split("</style>").next().unwrap();
    let queried = doc.make_stylesheet(css, style::stylesheets::Origin::Author);
    doc.add_stylesheet_for_node(queried, style_node);
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));
    assert_eq!(size(&doc, "#nested"), (60.0, 10.0));
}

#[test]
fn container_queries_match_originating_element_pseudos_without_reborrowing_styles() {
    let mut doc = HtmlDocument::from_html(
        r#"<!doctype html><style>
        #c{container-type:inline-size;width:200px}
        #c::before{content:'';display:block;width:10px;height:10px}
        @container(width>100px){#c::before{width:80px}}
        </style><div id=c></div>"#,
        DocumentConfig::default(),
    );
    doc.resolve(0.0);
    let c = doc.query_selector("#c").unwrap().unwrap();
    let before = doc.get_node(c).unwrap().before().unwrap();
    assert_eq!(doc.get_node(before).unwrap().final_layout().size.width, 80.0);
    doc.set_style_property(c, "width", "90px");
    doc.resolve_style(0.0);
    let before = doc.get_node(c).unwrap().before().unwrap();
    assert_eq!(doc.get_node(before).unwrap().final_layout().size.width, 10.0);
}

#[test]
fn style_queries_on_normal_containers_preserve_style_only_no_layout() {
    let mut doc = HtmlDocument::from_html(
        "<!doctype html><style>#c{--theme:dark}#target{width:80px;height:20px}@container style(--theme:dark){#target{height:30px}}</style><div id=c><div id=target></div></div>",
        DocumentConfig::default(),
    );
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (80.0, 30.0));
    let target = doc.query_selector("#target").unwrap().unwrap();
    doc.set_style_property(target, "width", "120px");
    let before = pyro_render::blitz_passes().1;
    doc.resolve_style(0.0);
    assert_eq!(pyro_render::blitz_passes().1, before);
    assert_eq!(size(&doc, "#target"), (80.0, 30.0));
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (120.0, 30.0));
}

#[test]
fn clean_container_query_style_reads_reuse_layout_until_geometry_dependencies_change() {
    let mut doc = HtmlDocument::from_html(SOURCE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    let generation = doc.tree().geometry_generation();
    let layout_passes = pyro_render::blitz_passes().1;
    for _ in 0..3 {
        doc.resolve_style(0.0);
    }
    assert_eq!(
        (doc.tree().geometry_generation(), pyro_render::blitz_passes().1),
        (generation, layout_passes),
        "clean style reads must reuse geometry without running layout",
    );
    assert_eq!(size(&doc, "#target"), (80.0, 20.0));

    let outer = doc.query_selector("#outer").unwrap().unwrap();
    doc.set_style_property(outer, "width", "180px");
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));
    doc.viewport_mut().window_size = (500, 600);
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#responsiveTarget"), (5.0, 5.0));

    doc.set_style_property(outer, "display", "none");
    doc.resolve_style(0.0);
    let hidden_clean = (doc.tree().geometry_generation(), pyro_render::blitz_passes().1);
    for _ in 0..3 {
        doc.resolve_style(0.0);
    }
    assert_eq!((doc.tree().geometry_generation(), pyro_render::blitz_passes().1), hidden_clean);
    doc.set_style_property(outer, "display", "block");
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#target"), (10.0, 20.0));

    // A newly added query can need its first size sample even when its rule
    // leaves computed styles unchanged until that sample becomes available.
    let mut added = HtmlDocument::from_html(
        "<!doctype html><style>#c{container-type:inline-size;width:100%}#target{width:10px;height:20px}</style><div id=c><div id=target></div></div>",
        DocumentConfig::default(),
    );
    added.viewport_mut().window_size = (800, 600);
    added.resolve(0.0);
    let sheet_node = added.query_selector("style").unwrap().unwrap();
    let sheet = added.make_stylesheet(
        "#c{container-type:inline-size;width:100%}#target{width:10px;height:20px}@container(width>600px){#target{width:80px}}",
        style::stylesheets::Origin::Author,
    );
    added.add_stylesheet_for_node(sheet, sheet_node);
    added.resolve_style(0.0);
    assert_eq!(size(&added, "#target"), (80.0, 20.0));
    // Percentage widths can change layout without changing computed style.
    added.viewport_mut().window_size = (500, 600);
    // Device inspection must not consume the pending geometry invalidation.
    let _ = added.stylist_device().media_type();
    added.resolve_style(0.0);
    assert_eq!(size(&added, "#target"), (10.0, 20.0));
}

/// pyro: container units (`cqw`, `cqh`, `cqi`, `cqb`, `cqmin`, `cqmax`) parse and resolve against
/// the nearest size container, without any `@container` rule in the document; they were dropped
/// as invalid (Stylo parsed them for Gecko only), so `width: 50cqw` was `auto` and a
/// `radial-gradient()` positioned with them was not drawn (WPT
/// css/css-images/radial-gradient-container-relative-units-*).
#[test]
fn container_units_resolve_against_the_size_container() {
    const UNITS: &str = r#"<!doctype html><style>
body {margin:0}
#c {container-type:size;width:200px;height:100px}
#w {width:50cqw;height:10cqh;background:blue}
#i {width:25cqi;height:20cqb}
#m {width:50cqmin;height:10cqmax}
#no {width:50cqw;height:5px}
</style><div id=c><div id=w></div><div id=i></div><div id=m></div></div><div id=no></div>"#;
    let mut doc = HtmlDocument::from_html(UNITS, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#w"), (100.0, 10.0));
    assert_eq!(size(&doc, "#i"), (50.0, 20.0));
    assert_eq!(size(&doc, "#m"), (50.0, 20.0));
    // Without a container, the small viewport.
    assert_eq!(size(&doc, "#no"), (400.0, 5.0));
    let container = doc.query_selector("#c").unwrap().unwrap();
    doc.set_style_property(container, "width", "300px");
    doc.resolve_style(0.0);
    assert_eq!(size(&doc, "#w"), (150.0, 10.0));
    assert_eq!(size(&doc, "#no"), (400.0, 5.0));
}

/// pyro: a container query with an unknown feature selects no container, so it never matches,
/// even where its known part is true (WPT css-conditional container-queries/query-evaluation,
/// container-selection-unknown-features).
#[test]
fn container_query_with_an_unknown_feature_never_matches() {
    const UNKNOWN: &str = r#"<!doctype html><style>
#c {container-type:size;width:200px;height:100px}
i {display:block;width:1px;height:1px}
@container (width > 0px) {#known {width:10px}}
@container (width > 0px) or (foo: bar) {#or {width:20px}}
@container (width > 0px) or foo(bar) {#fn {width:30px}}
</style><div id=c><i id=known></i><i id=or></i><i id=fn></i></div>"#;
    let mut doc = HtmlDocument::from_html(UNKNOWN, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#known"), (10.0, 1.0));
    assert_eq!(size(&doc, "#or"), (1.0, 1.0));
    assert_eq!(size(&doc, "#fn"), (1.0, 1.0));
}

/// pyro: a table or a table cell with `container-type` is still selected as the query container
/// but has no size (size containment does not apply to it): size queries fail and container
/// units resolve against the next size container out, also after a change to `display: table`
/// (WPT css-conditional container-queries ineligible-containment,
/// container-units-ineligible-container).
#[test]
fn a_table_is_not_a_size_query_container() {
    const TABLES: &str = r#"<!doctype html><style>
#outer {container-type:size;width:300px;height:200px}
.c {container-type:inline-size;width:200px}
i {display:block;width:1px;height:1px}
@container (min-width: 1px) {i {width:20px}}
#u {width:10cqw;height:10cqh}
</style><div id=outer><div class=c id=t style="display:table"><i id=a></i><div id=u></div></div><div class=c id=b><i id=bi></i></div></div>"#;
    let mut doc = HtmlDocument::from_html(TABLES, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (1.0, 1.0));
    assert_eq!(size(&doc, "#u"), (30.0, 20.0));
    assert_eq!(size(&doc, "#bi"), (20.0, 1.0));
    let b = doc.query_selector("#b").unwrap().unwrap();
    doc.set_style_property(b, "display", "table-cell");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#bi"), (1.0, 1.0));
}

/// pyro: an element that becomes a size container, or whose container-name changes, restyles its
/// descendants: their queries select it (WPT css-conditional container-queries
/// container-type-invalidation, container-name-invalidation, query-container-name-dynamic).
#[test]
fn becoming_a_container_or_renaming_one_restyles_descendants() {
    const BECOME: &str = r#"<!doctype html><style>
#outer {width:300px}
#mid {width:250px}
i {display:block;width:1px;height:1px}
@container (min-width: 300px) {#a {width:20px}}
@container named (min-width: 1px) {#a {height:20px}}
</style><div id=outer><div id=mid><i id=a></i></div></div>"#;
    let mut doc = HtmlDocument::from_html(BECOME, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (1.0, 1.0));
    let outer = doc.query_selector("#outer").unwrap().unwrap();
    doc.set_style_property(outer, "container-type", "inline-size");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (20.0, 1.0));
    doc.set_style_property(outer, "container-name", "named");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (20.0, 20.0));
    let mid = doc.query_selector("#mid").unwrap().unwrap();
    doc.set_style_property(mid, "container-type", "inline-size");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (1.0, 20.0));
}

/// pyro: `@container scroll-state(scrollable: ...)` matches the edges a `container-type:
/// scroll-state` scroll container can scroll towards at the last layout: an `overflow: auto` box
/// with taller content scrolls towards its bottom only, an `overflow: visible` box towards none,
/// and a horizontally overflowing RTL box towards its left (its inline end). The first style pass
/// sees no edges; the pass after layout styles the subtrees again (WPT css-conditional
/// container-queries/scroll-state `scroll-state-initially-scrollable`, `-scrollable-wm`).
#[test]
fn scroll_state_scrollable_queries_follow_the_layout_scroll_range() {
    const SCROLLABLE: &str = r#"<!doctype html><style>
body {margin:0}
.s {container-type:scroll-state;width:100px;height:100px}
.auto {overflow:auto}
.tall {width:10px;height:300px}
.wide {width:300px;height:1px}
i {display:block;width:1px;height:1px}
@container scroll-state(scrollable: bottom) {i {width:20px}}
@container scroll-state(scrollable: top) {i {height:20px}}
@container scroll-state(scrollable: none) {i {width:5px}}
@container scroll-state(scrollable: inline-end) {i {height:7px}}
@container scroll-state((scrollable: left) and (not (scrollable: right))) {i {width:9px}}
</style><div class="s auto"><i id=a></i><div class=tall></div></div><div class=s><i id=b></i><div class=tall></div></div><div class="s auto" style="direction:rtl"><i id=c></i><div class=wide></div></div>"#;
    let mut doc = HtmlDocument::from_html(SCROLLABLE, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (20.0, 1.0));
    assert_eq!(size(&doc, "#b"), (5.0, 1.0));
    assert_eq!(size(&doc, "#c"), (9.0, 7.0));
}

/// pyro: `@container scroll-state(scrolled: ...)` matches the directions of the container's last
/// scroll on each axis, kept until the next scroll on that axis (CSS Conditional 5 § 7.3.2; WPT
/// css-conditional container-queries/scroll-state `scroll-state-scrolled-hv`). Nothing matches
/// but `none` before the first one, and `scrollable` follows the offsets the embedder tells.
#[test]
fn scroll_state_scrolled_queries_follow_the_last_scroll_directions() {
    const SCROLLED: &str = r#"<!doctype html><style>
body {margin:0}
.s {container-type:scroll-state;width:100px;height:100px;overflow:auto}
.big {width:300px;height:300px}
i {display:block;width:1px;height:1px}
@container scroll-state(scrolled: none) {i {width:5px}}
@container scroll-state(scrolled: bottom) {i {height:20px}}
@container scroll-state(scrolled: right) {i {height:30px}}
@container scroll-state(scrolled: top) {i {height:40px}}
@container scroll-state(scrollable: top) {i {width:9px}}
</style><div class=s id=s><i id=a></i><div class=big></div></div>"#;
    let mut doc = HtmlDocument::from_html(SCROLLED, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (5.0, 1.0));
    let scroller = doc.query_selector("#s").unwrap().unwrap();
    doc.set_timeline_scroll([(scroller, (0.0, 50.0))].into(), (0.0, 0.0));
    doc.set_scroll_directions([(scroller, (0, 1))].into());
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (9.0, 20.0));
    doc.set_timeline_scroll([(scroller, (30.0, 50.0))].into(), (0.0, 0.0));
    doc.set_scroll_directions([(scroller, (1, 1))].into());
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (9.0, 30.0));
}

/// pyro: `@container scroll-state(snapped: ...)` matches a container that is the snap target of
/// a scroll container on the axis, `block` and `inline` by the container's own writing mode (WPT
/// css-conditional container-queries/scroll-state `scroll-state-snapped-wm`, `-snapped-none`).
#[test]
fn scroll_state_snapped_queries_follow_the_snap_targets() {
    const SNAPPED: &str = r#"<!doctype html><style>
body {margin:0}
.s {container-type:scroll-state;width:50px;height:50px}
.v {writing-mode:vertical-lr}
i {display:block;width:1px;height:1px}
@container scroll-state(snapped: none) {i {width:5px}}
@container scroll-state(snapped: y) {i {height:20px}}
@container scroll-state(snapped: block) {i {width:30px}}
@container scroll-state(snapped: inline) {i {width:40px}}
@container scroll-state(snapped: x) {i {height:60px}}
</style><div class=s id=h><i id=a></i></div><div class="s v" id=v><i id=b></i></div>"#;
    let mut doc = HtmlDocument::from_html(SNAPPED, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (5.0, 1.0));
    assert_eq!(size(&doc, "#b"), (5.0, 1.0));
    let (h, v) = (doc.query_selector("#h").unwrap().unwrap(), doc.query_selector("#v").unwrap().unwrap());
    doc.set_snapped([(h, (false, true)), (v, (false, true))].into());
    doc.resolve(0.0);
    // Snapped on y: the block axis of a horizontal container, the inline axis of a vertical one.
    assert_eq!(size(&doc, "#a"), (30.0, 20.0));
    assert_eq!(size(&doc, "#b"), (40.0, 20.0));
}

/// pyro: a size feature reads the container's unrounded size, and a writing-mode change of the
/// container evaluates the logical features of its descendants again (WPT css-conditional
/// container-queries `size-feature-fractional`, `size-container-writing-mode-change`).
#[test]
fn size_features_read_fractional_sizes_and_follow_the_writing_mode() {
    const FRACTIONAL: &str = r#"<!doctype html><style>
body {margin:0}
#c {container-type:size;width:100.5px;height:50.5px}
i {display:block;width:1px;height:1px}
@container (width = 100.5px) {i {width:7px}}
@container (inline-size = 50.5px) {i {height:9px}}
</style><div id=c><i id=a></i></div>"#;
    let mut doc = HtmlDocument::from_html(FRACTIONAL, DocumentConfig::default());
    doc.viewport_mut().window_size = (800, 600);
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (7.0, 1.0));
    let container = doc.query_selector("#c").unwrap().unwrap();
    doc.set_style_property(container, "writing-mode", "vertical-lr");
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#a"), (7.0, 9.0));
}
