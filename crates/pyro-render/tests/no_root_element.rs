//! A page can remove its document element, and can append any element in its place. Found by
//! the WPT crashtests (bench/wpt/README.md, "Crashtests"): layout queries and screenshots of
//! such a document panicked.
//!
//! - No document element (WPT dom/nodes/crashtests/documentElement-remove-descendant-query.html,
//!   css/css-animations/keyframes-remove-documentElement-crash.html): `root_element()` unwrapped
//!   nothing in the capture and in the document's size. The capture is the empty canvas.
//! - A root element that is inline content, a `<br>` with a text child (WPT css/css-text/
//!   white-space/pre-line-br-with-whitespace-child-crash.html): the document node was made an
//!   inline root, and its inline layout had no element to go to.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::snapshot::{RenderHtml, RenderNodeKind, RenderTree};
use pyro_dom::{Dom, NodeId};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn tree(dom: &DomRef, doc: NodeId) -> Arc<Tree> {
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let t: RenderTree = dom.borrow().render_tree(doc, &opts).unwrap();
    Arc::new(Tree {
        nodes: t
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
    })
}

fn snapshot(tree: Arc<Tree>) -> Snapshot {
    Snapshot {
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        resource_scope: "doc".into(),
        live: Some(Live { page: 1, tree, clip: None, interaction: None }),
        ..Default::default()
    }
}

/// Lays out and captures the page's persistent document; the image, when it could be painted.
fn capture(r: &mut Renderer, dom: &DomRef, doc: NodeId) -> Result<RgbaImage, String> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snap = snapshot(tree(dom, doc));
    r.sync_layout(&snap, &fetcher).map_err(|e| format!("layout: {e:?}"))?;
    let painted = r.paint(&snap, Area::Viewport, &fetcher).map_err(|e| format!("paint: {e:?}"))?;
    Ok(r.raster(painted, Backend::Cpu).0)
}

fn page() -> (DomRef, NodeId, Renderer) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = parse_document(
        &dom,
        "http://test/",
        "<!doctype html><style>br { white-space: pre-line }</style><body style='margin:0;background:#0f0'><span id=s>text</span>",
    );
    let mut r = Renderer::new();
    let img = capture(&mut r, &dom, doc).unwrap();
    assert_eq!(img.data[..3], [0, 255, 0], "the page is painted before its element goes");
    (dom, doc, r)
}

#[test]
fn a_document_without_its_element_is_an_empty_canvas() {
    let (dom, doc, mut r) = page();
    let html = dom.borrow().document_element(doc).unwrap().unwrap();
    dom.borrow_mut().remove_child(doc, html).unwrap();
    let img = capture(&mut r, &dom, doc).expect("a capture, not a panic");
    assert_eq!((img.width, img.height), (200, 100));
    let first = img.data[..4].to_vec();
    assert!(img.data.chunks_exact(4).all(|p| p == first) && first[..3] != [0, 255, 0], "nothing is painted");
    assert_eq!(r.document_size(1), Some((200.0, 100.0)), "the document is as large as the viewport");
    assert!(r.page_layout(1).is_none_or(|l| l.boxes.is_empty()), "nothing is laid out");
}

#[test]
fn a_root_element_that_is_inline_content_is_laid_out() {
    let (dom, doc, mut r) = page();
    let br = {
        let mut dom = dom.borrow_mut();
        let html = dom.document_element(doc).unwrap().unwrap();
        dom.remove_child(doc, html).unwrap();
        let br = dom.create_element(doc, "br").unwrap();
        let text = dom.create_text(doc, "").unwrap();
        dom.append_child(br, text).unwrap();
        dom.append_child(doc, br).unwrap();
        br
    };
    capture(&mut r, &dom, doc).expect("a capture, not a panic");
    // The page then changes the text (the crashtest's last step).
    let text = dom.borrow().first_child(br).unwrap().unwrap();
    dom.borrow_mut().set_text_content(text, " ").unwrap();
    capture(&mut r, &dom, doc).expect("a capture, not a panic");
}
