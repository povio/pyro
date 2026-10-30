//! `Dom::render_html`: the document as a renderer sees it.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::snapshot::RenderHtml;
use lp_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

fn find(dom: &Dom, d: NodeId, css: &str) -> NodeId {
    dom.query_selector(d, &parse_selectors(css).unwrap()).unwrap().unwrap()
}

fn render(dom: &Dom, d: NodeId) -> String {
    render_with(dom, d, &|_| None, &|_| None, None)
}

fn render_with(
    dom: &Dom,
    d: NodeId,
    style_text: &dyn Fn(NodeId) -> Option<String>,
    href: &dyn Fn(NodeId) -> Option<String>,
    mark: Option<(NodeId, &str)>,
) -> String {
    dom.render_html(d, &RenderHtml { style_text, stylesheet_href: href, mark }).unwrap()
}

#[test]
fn form_state_is_written_into_markup() {
    let (dom, d) = doc(concat!(
        "<input id=t value=old><input id=c type=checkbox checked><input id=r type=radio>",
        "<textarea id=a>old</textarea>",
        "<select id=s><option id=o1 selected>a</option><option id=o2>b</option></select>",
        "<input id=f type=file>",
    ));
    {
        let mut dom = dom.borrow_mut();
        let (t, c, r, a, o2) =
            (find(&dom, d, "#t"), find(&dom, d, "#c"), find(&dom, d, "#r"), find(&dom, d, "#a"), find(&dom, d, "#o2"));
        dom.set_value(t, "typed \"in\"").unwrap();
        dom.set_checked(c, false).unwrap();
        dom.set_checked(r, true).unwrap();
        dom.set_value(a, "line 1\n<b>").unwrap();
        dom.set_option_selected(o2, true).unwrap();
    }
    let html = render(&dom.borrow(), d);
    assert!(html.contains(r#"<input id="t" value="typed &quot;in&quot;">"#), "{html}");
    assert!(html.contains(r#"<input id="c" type="checkbox">"#), "{html}");
    assert!(html.contains(r#"<input id="r" type="radio" checked>"#), "{html}");
    assert!(html.contains("<textarea id=\"a\">\nline 1\n&lt;b&gt;</textarea>"), "{html}");
    assert!(html.contains(r#"<option id="o1">a</option><option id="o2" selected>b</option>"#), "{html}");
    assert!(html.contains(r#"<input id="f" type="file">"#), "{html}");
}

#[test]
fn scripts_handlers_and_comments_are_dropped() {
    let (dom, d) = doc(concat!(
        "<!DOCTYPE html><html><head><script>alert(1)</script></head>",
        "<body onload=\"x()\"><!-- c --><noscript>no js</noscript><template><p>t</p></template>",
        "<iframe src=\"/frame\"></iframe><p>kept</p></body></html>",
    ));
    let html = render(&dom.borrow(), d);
    assert_eq!(html, "<!DOCTYPE html><html><head></head><body><iframe></iframe><p>kept</p></body></html>");
}

#[test]
fn sheets_come_from_the_caller() {
    let (dom, d) = doc(concat!(
        "<head><style id=s>p { color: red }</style><style id=e>em { color: red }</style>",
        "<link id=l rel=stylesheet href=a.css><link id=m rel=stylesheet href=missing.css></head><p>x</p>",
    ));
    let dom = dom.borrow();
    let (s, l) = (find(&dom, d, "#s"), find(&dom, d, "#l"));
    // CSSOM replaced the first sheet's text; the second keeps its own.
    let html = render_with(
        &dom,
        d,
        &|n| (n == s).then(|| "p { color: blue }".to_owned()),
        &|n| (n == l).then(|| "http://test/a.css".to_owned()),
        None,
    );
    assert!(html.contains(r#"<style id="s">p { color: blue }</style>"#), "{html}");
    assert!(html.contains(r#"<style id="e">em { color: red }</style>"#), "{html}");
    assert!(html.contains(r#"<link id="l" rel="stylesheet" href="http://test/a.css">"#), "{html}");
    assert!(!html.contains("missing.css"), "{html}");
}

#[test]
fn clip_target_is_marked() {
    let (dom, d) = doc("<div id=a></div><div id=b></div>");
    let dom = dom.borrow();
    let b = find(&dom, d, "#b");
    let html = render_with(&dom, d, &|_| None, &|_| None, Some((b, "data-clip")));
    assert!(html.contains(r#"<div id="a"></div><div id="b" data-clip></div>"#), "{html}");
}

#[test]
fn shadow_trees_are_flattened_with_slots() {
    let (dom, d) = doc(r#"<div id=host><span slot=title>T</span>light</div>"#);
    {
        let mut dom = dom.borrow_mut();
        let host = find(&dom, d, "#host");
        let root = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let h = dom.create_element(d, "h1").unwrap();
        let named = dom.create_element(d, "slot").unwrap();
        dom.set_attribute(named, "name", "title").unwrap();
        let p = dom.create_element(d, "p").unwrap();
        let default = dom.create_element(d, "slot").unwrap();
        let empty = dom.create_element(d, "slot").unwrap();
        dom.set_attribute(empty, "name", "none").unwrap();
        let fallback = dom.create_text(d, "fallback").unwrap();
        dom.append_child(empty, fallback).unwrap();
        dom.append_child(h, named).unwrap();
        dom.append_child(p, default).unwrap();
        for n in [h, p, empty] {
            dom.append_child(root, n).unwrap();
        }
    }
    let html = render(&dom.borrow(), d);
    assert!(
        html.contains(r#"<div id="host"><h1><span slot="title">T</span></h1><p>light</p>fallback</div>"#),
        "{html}"
    );
}

#[test]
fn render_tree_has_the_html_content_with_stable_keys() {
    use lp_dom::snapshot::{render_key, RenderNodeKind, SYNTHETIC_TEXT, TOP};
    let (dom, d) = doc("<style>p{}</style><p id=p class=a>hi</p><textarea id=t>x</textarea><script>1</script>");
    let (p, t) = (find(&dom.borrow(), d, "#p"), find(&dom.borrow(), d, "#t"));
    dom.borrow_mut().set_value(t, "typed").unwrap();
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: Some((p, "data-x")) };
    let tree = dom.borrow().render_tree(d, &opts).unwrap();
    assert_eq!(tree.mark, Some(render_key(p)));
    assert_eq!(tree.nodes[0].parent, TOP);
    let element = |key: u64| tree.nodes.iter().find(|n| n.key == key).unwrap();
    match &element(render_key(p)).kind {
        // The mark is not an attribute of the tree.
        RenderNodeKind::Element { name, attrs } => {
            assert_eq!(&*name.local, "p");
            assert_eq!(
                attrs.iter().map(|(n, v)| (&*n.local, v.as_str())).collect::<Vec<_>>(),
                [("id", "p"), ("class", "a")]
            );
        }
        k => panic!("{k:?}"),
    }
    // A textarea's value stands for its children.
    let value = element(render_key(t) | SYNTHETIC_TEXT);
    assert_eq!(value.kind, RenderNodeKind::Text("typed".into()));
    assert_eq!(tree.nodes[value.parent as usize].key, render_key(t));
    assert!(tree
        .nodes
        .iter()
        .all(|n| !matches!(&n.kind, RenderNodeKind::Element { name, .. } if &*name.local == "script")));
    // Keys survive a mutation elsewhere; a changed attribute changes only that node.
    dom.borrow_mut().set_attribute(p, "class", "b").unwrap();
    let again = dom.borrow().render_tree(d, &opts).unwrap();
    assert_eq!(
        again.nodes.iter().map(|n| n.key).collect::<Vec<_>>(),
        tree.nodes.iter().map(|n| n.key).collect::<Vec<_>>()
    );
    let changed: Vec<u64> = again.nodes.iter().zip(&tree.nodes).filter(|(a, b)| a != b).map(|(a, _)| a.key).collect();
    assert_eq!(changed, [render_key(p)]);
}
