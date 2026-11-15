//! What the renderer knows of the document beyond its nodes ([`Snapshot::xml_document`],
//! [`Snapshot::content_language`]):
//! - in an XML document (XHTML), type selectors match element names case-sensitively, so `DIV`
//!   does not match `div` (WPT css/CSS2/syntax/case-sensitive-007); in HTML it does;
//! - the HTTP `Content-Language` of the document is the last fallback of `:lang()`, after the
//!   elements' `lang` and a `<meta http-equiv="content-language">` pragma
//!   (WPT css/CSS2/selectors/lang-selector-005).
//!
//! A kept (live) document is built anew when either changes.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const HEIGHT: u32 = 20;

/// Whether each 20px row of the rendered `snapshot` is drawn green.
fn green(snapshot: Snapshot, rows: usize, renderer: &mut Renderer) -> Vec<bool> {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let img = renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    (0..rows)
        .map(|row| {
            let i = ((row * HEIGHT as usize + 10) * img.width as usize + 10) * 4;
            img.data[i..i + 3] == [0, 255, 0]
        })
        .collect()
}

fn page(head: &str, css: &str, rows: &[&str]) -> Snapshot {
    let body: String = rows.iter().map(|r| format!("<div style='height:20px'>{r}</div>")).collect();
    Snapshot {
        html: format!(
            "<!doctype html><html><head>{head}<style>body{{margin:0;background:#fff}} \
             i{{display:block;height:20px}} {css}</style></head><body>{body}</body></html>"
        ),
        base_url: "http://test/".into(),
        width: 40,
        height: HEIGHT * rows.len() as u32,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

#[test]
fn xml_documents_match_type_selectors_case_sensitively() {
    let css = "I { background: #0f0 } i[TITLE] { background: #0f0 }";
    let rows = ["<i></i>", "<i title=x></i>"];
    let html = green(page("", css, &rows), rows.len(), &mut Renderer::new());
    assert_eq!(html, [true, true]);
    let xml = green(Snapshot { xml_document: true, ..page("", css, &rows) }, rows.len(), &mut Renderer::new());
    assert_eq!(xml, [false, false]);
}

#[test]
fn content_language_is_the_last_fallback_of_lang() {
    let css = "i:lang(fr) { background: #0f0 }";
    let rows = ["<i></i>", "<i lang=de></i>", "<b lang=fr-CA><i></i></b>"];
    let fr = |head: &str| Snapshot { content_language: Some("fr".into()), ..page(head, css, &rows) };
    assert_eq!(green(fr(""), rows.len(), &mut Renderer::new()), [true, false, true]);
    // The pragma wins over the protocol.
    let pragma = "<meta http-equiv=content-language content=en>";
    assert_eq!(green(fr(pragma), rows.len(), &mut Renderer::new()), [false, false, true]);
    assert_eq!(green(page("", css, &rows), rows.len(), &mut Renderer::new()), [false, false, true]);
}

/// A kept document of the page as one green-if-matched `<i>` (`<html><body><i>`).
fn live(tree: &Arc<Tree>, xml_document: bool, content_language: Option<&str>) -> Snapshot {
    Snapshot {
        base_url: "http://test/".into(),
        sheets: Default::default(),
        width: 40,
        height: HEIGHT,
        device_scale_factor: 1.0,
        live: Some(Live { page: 7, tree: tree.clone(), clip: None, interaction: None }),
        xml_document,
        content_language: content_language.map(str::to_owned),
        ..Default::default()
    }
}

#[test]
fn a_kept_document_follows_the_facts() {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(
        &dom,
        "http://test/",
        "<!doctype html><style>body{margin:0;background:#fff} i{display:block;height:20px} \
         I:lang(fr){background:#0f0}</style><i></i>",
    );
    let tree = dom
        .borrow()
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let tree = Arc::new(Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| TreeNode {
                key: node.key,
                parent: node.parent,
                kind: match node.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(text) => TreeKind::Text(text),
                },
            })
            .collect(),
    });
    let mut renderer = Renderer::new();
    assert_eq!(green(live(&tree, false, Some("fr")), 1, &mut renderer), [true]);
    assert_eq!(green(live(&tree, true, Some("fr")), 1, &mut renderer), [false]);
    assert_eq!(green(live(&tree, false, Some("fr")), 1, &mut renderer), [true]);
    assert_eq!(green(live(&tree, false, None), 1, &mut renderer), [false]);
}
