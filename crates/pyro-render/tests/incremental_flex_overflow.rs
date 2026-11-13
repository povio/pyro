//! Wrapped flex overflow must follow incremental geometry, not the previous width.
//! Chrome/native reproduction: /tmp/tos-wrapped-flex-overflow-minimal.html.
use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
    NodeId,
};
use pyro_render::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};
struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}
struct Page {
    dom: DomRef,
    doc: NodeId,
    renderer: Renderer,
}
impl Page {
    fn new(html: &str) -> Self {
        let dom = Rc::new(RefCell::new(pyro_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        Self { dom, doc, renderer: Renderer::new() }
    }
    fn measure(&mut self, width: u32, ids: &[&str]) -> Vec<LayoutBox> {
        let d = self.dom.borrow();
        let keys: Vec<_> =
            ids.iter().map(|id| render_key(d.get_element_by_id(self.doc, id).unwrap().unwrap())).collect();
        let tree = d
            .render_tree(
                self.doc,
                &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None },
            )
            .unwrap();
        let tree = Tree {
            nodes: tree
                .nodes
                .into_iter()
                .map(|n| TreeNode {
                    key: n.key,
                    parent: n.parent,
                    kind: match n.kind {
                        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                        RenderNodeKind::Text(t) => TreeKind::Text(t),
                    },
                })
                .collect(),
        };
        let snap = Snapshot {
            base_url: "http://test/".into(),
            width,
            height: 768,
            device_scale_factor: 1.0,
            live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
            ..Default::default()
        };
        self.renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
        keys.into_iter().map(|key| self.renderer.element_box(1, key).unwrap()).collect()
    }
}
#[test]
fn wrapped_descendant_overflow_shrinks_after_viewport_resize() {
    let mut p = Page::new(
        r#"<!doctype html><style>body{margin:0}.outer{width:60vw;display:flex}section{min-width:0;display:flex;flex-wrap:wrap}span{width:200px;height:20px;flex-shrink:0}</style><div class=outer><section id=wrap><span>A</span><span>B</span><span>C</span><span>D</span></section></div>"#,
    );
    for width in [1920, 1280, 1024, 1280, 1024] {
        let b = p.measure(width, &["wrap"])[0];
        assert_eq!(b.scroll_width, b.client_width, "viewport{width}: stale wrapped overflow {b:?}");
        assert_eq!(b.height, if width == 1920 { 20.0 } else { 40.0 });
        let unchanged = p.measure(width, &["wrap"])[0];
        assert_eq!(unchanged.scroll_width, b.scroll_width, "unchanged layout remains stable");
    }
}
#[test]
fn actual_overflow_survives_resize_and_clipping_stops_ancestor_propagation() {
    let mut p = Page::new(
        r#"<!doctype html><style>body{margin:0}.outer{width:60vw;display:flex}section{width:100%;min-width:0}i{display:block;width:900px;height:20px}.clip{overflow:hidden}</style><div id=clippedParent class=outer><section id=clipped class=clip><i></i></section></div><div id=visibleParent class=outer><section id=visible><i></i></section></div>"#,
    );
    for width in [1920, 1024, 1920, 1024] {
        let b = p.measure(width, &["clippedParent", "clipped", "visibleParent", "visible"]);
        assert_eq!(b[0].scroll_width, b[0].client_width, "clipped descendant leaks: {b:?}");
        for i in [1, 2, 3] {
            assert_eq!(b[i].scroll_width, b[i].client_width.max(900.0), "real overflow lost at{width}: {b:?}");
        }
    }
}
#[test]
fn content_shrink_removes_old_overflow_without_losing_remaining_content() {
    let mut p = Page::new(
        r#"<!doctype html><style>body{margin:0}#outer{width:150px}#content{white-space:nowrap;font:16px monospace}</style><div id=outer><div id=content>ABCDEFGHIJKLMNOPQRSTUVWX</div></div>"#,
    );
    let before = p.measure(1024, &["outer", "content"]);
    assert!(before[0].scroll_width > before[0].client_width);
    let content = p.dom.borrow().get_element_by_id(p.doc, "content").unwrap().unwrap();
    p.dom.borrow_mut().set_text_content(content, "ABC").unwrap();
    let after = p.measure(1024, &["outer", "content"]);
    for b in after {
        assert_eq!(b.scroll_width, b.client_width, "old text overflow survived: {b:?}");
    }
}
