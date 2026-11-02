use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use lp_render::{Area, Backend, Fetcher, Renderer, Snapshot};
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
    let before = lp_render::blitz_phases()[4];
    let target = doc.query_selector("#target").unwrap().unwrap();
    doc.set_style_property(target, "width", "120px");
    doc.resolve_style(0.0);
    assert!(doc.get_node(target).unwrap().primary_styles().is_some());
    assert_eq!(lp_render::blitz_phases()[4], before);
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
    let before = lp_render::blitz_phases()[4];
    doc.resolve_style(0.0);
    assert_eq!(lp_render::blitz_phases()[4], before, "removing the last query must restore style-only resolution");
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
    let before = lp_render::blitz_phases()[4];
    doc.resolve_style(0.0);
    assert_eq!(lp_render::blitz_phases()[4], before);
    assert_eq!(size(&doc, "#target"), (80.0, 30.0));
    doc.resolve(0.0);
    assert_eq!(size(&doc, "#target"), (120.0, 30.0));
}
