//! `Dom::render_html`: the document as a renderer sees it.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::snapshot::RenderHtml;
use pyro_dom::*;

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
    dom.render_html(d, &RenderHtml { style_text, stylesheet_href: href, mark, adopted: None }).unwrap()
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
    use pyro_dom::snapshot::{render_key, RenderNodeKind, SYNTHETIC_TEXT, TOP};
    let (dom, d) = doc("<style>p{}</style><p id=p class=a>hi</p><textarea id=t>x</textarea><script>1</script>");
    let (p, t) = (find(&dom.borrow(), d, "#p"), find(&dom.borrow(), d, "#t"));
    dom.borrow_mut().set_value(t, "typed").unwrap();
    let opts =
        RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: Some((p, "data-x")), adopted: None };
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

#[test]
fn textarea_attribute_delta_preserves_typed_value_and_synthetic_child() {
    use pyro_dom::snapshot::{render_key, DeltaOp, SYNTHETIC_TEXT};
    let (dom, d) = doc("<form><textarea id=t>default</textarea></form>");
    let textarea = find(&dom.borrow(), d, "#t");
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    dom.borrow_mut().set_value(textarea, "typed\n<keep>").unwrap();
    let before = dom.borrow().render_tree(d, &opts).unwrap();
    let version = dom.borrow().version();
    for (name, value) in [
        ("class", "highlighted"),
        ("style", "width:180px;height:60px"),
        ("aria-label", "Updated notes"),
        ("data-tracking", "stable-value"),
    ] {
        dom.borrow_mut().set_attribute(textarea, name, value).unwrap();
    }
    let after = dom.borrow().render_tree(d, &opts).unwrap();
    let delta =
        dom.borrow().render_delta(d, version, &opts).expect("attribute-only textarea updates must remain incremental");
    let key = render_key(textarea);
    let rendered = after.nodes.iter().find(|node| node.key == key).unwrap();
    assert!(delta
        .ops
        .iter()
        .any(|op| matches!(op, DeltaOp::Node { key: k, kind } if *k == key && *kind == rendered.kind)));
    assert!(
        delta.ops.iter().all(|op| matches!(op, DeltaOp::Node { .. })),
        "attribute updates must not replace the value child"
    );
    let value = |tree: &pyro_dom::snapshot::RenderTree| {
        tree.nodes.iter().find(|node| node.key == key | SYNTHETIC_TEXT).unwrap().kind.clone()
    };
    assert_eq!(value(&after), value(&before));
    assert_eq!(dom.borrow().value(textarea).unwrap(), "typed\n<keep>");
}

#[test]
fn iframe_attribute_delta_carries_the_frame_element() {
    use pyro_dom::snapshot::{render_key, DeltaOp};
    let (dom, d) = doc("<body><iframe id=f></iframe></body>");
    let frame = find(&dom.borrow(), d, "#f");
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let version = dom.borrow().version();
    dom.borrow_mut().set_attribute(frame, "style", "width:500px").unwrap();
    let after = dom.borrow().render_tree(d, &opts).unwrap();
    let delta = dom.borrow().render_delta(d, version, &opts).expect("incremental");
    let key = render_key(frame);
    let rendered = after.nodes.iter().find(|node| node.key == key).unwrap();
    assert!(
        delta.ops.iter().any(|op| matches!(op, DeltaOp::Node { key: k, kind } if *k == key && *kind == rendered.kind)),
        "{:?}",
        delta.ops.len()
    );
}

#[test]
fn textarea_non_attribute_changes_and_special_trees_keep_the_full_snapshot_fallback() {
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    for case in ["value-then-attribute", "attribute-then-value", "text", "children", "shadow", "link"] {
        let (dom, d) =
            doc("<div id=host></div><textarea id=t>default</textarea><link id=sheet rel=stylesheet href=a.css>");
        let textarea = find(&dom.borrow(), d, "#t");
        let version = dom.borrow().version();
        {
            let mut dom = dom.borrow_mut();
            match case {
                "value-then-attribute" => {
                    dom.set_value(textarea, "new typed value").unwrap();
                    dom.set_attribute(textarea, "class", "updated").unwrap();
                }
                "attribute-then-value" => {
                    dom.set_attribute(textarea, "class", "updated").unwrap();
                    dom.set_value(textarea, "new typed value").unwrap();
                }
                "text" => {
                    dom.set_text_content(textarea, "new default").unwrap();
                }
                "children" => {
                    let child = dom.create_text(d, "appended").unwrap();
                    dom.append_child(textarea, child).unwrap();
                }
                "shadow" => {
                    let host = find(&dom, d, "#host");
                    dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
                    dom.set_attribute(textarea, "class", "updated").unwrap();
                }
                "link" => {
                    let link = find(&dom, d, "#sheet");
                    dom.set_attribute(link, "media", "print").unwrap();
                }
                _ => unreachable!(),
            }
        }
        assert!(
            dom.borrow().render_delta(d, version, &opts).is_none(),
            "{case}: special content/state must retain its fallback"
        );
    }
}

#[test]
fn scrolled_render_copy_preserves_offsets_without_mutating_document() {
    let (dom, document) = doc("<div id=outer data-pyro-scroll-top=999><div id=inner>content</div></div>");
    let dom = dom.borrow();
    let outer = find(&dom, document, "#outer");
    let inner = find(&dom, document, "#inner");
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let html = dom
        .render_html_scrolled(
            document,
            &opts,
            &[(pyro_dom::snapshot::render_key(outer), (24.0, 80.0)), (pyro_dom::snapshot::render_key(inner), (0.0, 16.0))],
        )
        .unwrap();
    assert!(html.contains(r#"data-pyro-scroll-left="24" data-pyro-scroll-top="80""#), "{html}");
    assert!(html.contains(r#"data-pyro-scroll-left="0" data-pyro-scroll-top="16""#), "{html}");
    assert!(!html.contains("999"));
    assert_eq!(dom.get_attribute(outer, "data-pyro-scroll-top").unwrap(), Some("999"));
    assert!(!dom.render_html(document, &opts).unwrap().contains("data-pyro-scroll"));
}

#[test]
fn password_render_copies_mask_current_values_without_mutating_inputs() {
    use pyro_dom::snapshot::{render_key, RenderNodeKind};
    let (dom, document) = doc(
        r#"<input id="password" type="PaSsWoRd" value="attribute-secret"><input id="plain" value="visible"><input id="empty" type="password">"#,
    );
    let password = find(&dom.borrow(), document, "#password");
    dom.borrow_mut().set_value(password, "typed-secret").unwrap();
    let html = render(&dom.borrow(), document);
    assert!(!html.contains("attribute-secret"));
    assert!(!html.contains("typed-secret"));
    assert!(html.contains(&format!("value=\"{}\"", "•".repeat(12))));
    assert!(html.contains(r#"<input id="plain" value="visible">"#));
    assert!(html.contains(r#"<input id="empty" type="password">"#));
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let tree = dom.borrow().render_tree(document, &opts).unwrap();
    let node = tree.nodes.iter().find(|node| node.key == render_key(password)).unwrap();
    match &node.kind {
        RenderNodeKind::Element { attrs, .. } => {
            assert_eq!(attrs.iter().find(|(name, _)| &*name.local == "value").unwrap().1, "•".repeat(12));
        }
        _ => panic!("expected password input"),
    }
    assert_eq!(dom.borrow().value(password).unwrap(), "typed-secret");
    dom.borrow_mut().set_attribute(password, "type", "text").unwrap();
    assert!(render(&dom.borrow(), document).contains("typed-secret"));
}

#[test]
fn render_tree_marks_the_top_layer_by_rank() {
    use pyro_dom::snapshot::{render_key, RenderNodeKind, STATE_NS};
    let (dom, d) = doc("<dialog id=a></dialog><div id=p popover></div><dialog id=b open></dialog>");
    let (a, p, b) = (find(&dom.borrow(), d, "#a"), find(&dom.borrow(), d, "#p"), find(&dom.borrow(), d, "#b"));
    let state = |dom: &Dom, el: NodeId| -> Option<String> {
        let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
        let tree = dom.render_tree(d, &opts).unwrap();
        let node = tree.nodes.iter().find(|n| n.key == render_key(el)).unwrap();
        let RenderNodeKind::Element { attrs, .. } = &node.kind else { panic!("not an element") };
        attrs.iter().find(|(n, _)| &*n.ns == STATE_NS && &*n.local == "state").map(|(_, v)| v.clone())
    };
    let has = |s: Option<String>, token: &str| s.unwrap_or_default().split(' ').any(|t| t == token);
    assert!(!state(&dom.borrow(), a).unwrap_or_default().contains("top-layer"));
    assert!(!state(&dom.borrow(), b).unwrap_or_default().contains("top-layer"));
    {
        let mut dom = dom.borrow_mut();
        dom.set_attribute(a, "open", "").unwrap();
        dom.set_modal(a, true).unwrap();
        dom.set_popover_showing(p, true).unwrap();
    }
    assert!(has(state(&dom.borrow(), a), "top-layer=0"));
    assert!(has(state(&dom.borrow(), p), "top-layer=1"));
    // Leaving the top layer moves the ones above it down.
    dom.borrow_mut().set_modal(a, false).unwrap();
    assert!(!state(&dom.borrow(), a).unwrap_or_default().contains("top-layer"));
    assert!(has(state(&dom.borrow(), p), "top-layer=0"));
    // A page cannot write the state attribute itself.
    dom.borrow_mut().set_attribute_ns(b, Some(STATE_NS), "state", "modal top-layer=9").unwrap();
    dom.borrow_mut().set_attribute_ns(a, Some(STATE_NS), "x:state", "top-layer=9").unwrap();
    assert!(!state(&dom.borrow(), b).unwrap_or_default().contains("top-layer"));
    assert!(!state(&dom.borrow(), a).unwrap_or_default().contains("top-layer"));
}

#[test]
fn a_changed_container_delta_carries_the_node_alone_as_the_fresh_tree_has_it() {
    use pyro_dom::snapshot::{render_key, DeltaOp};
    // A wide, deep subtree under the changed nodes: the delta must not
    // depend on it (one op per changed node, equal to the fresh tree's).
    let mut rows = String::new();
    for i in 0..200 {
        rows.push_str(&format!("<tr><td>r{i}</td><td><div class=flex><span>{i}</span></div></td></tr>"));
    }
    let (dom, d) = doc(&format!("<body><div class=page id=page><table><tbody>{rows}</tbody></table></div></body>"));
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let page = find(&dom.borrow(), d, "#page");
    let body = find(&dom.borrow(), d, "body");
    let td = find(&dom.borrow(), d, "td");
    let text = dom.borrow().first_child(td).unwrap().unwrap();
    let version = dom.borrow().version();
    {
        let mut dom = dom.borrow_mut();
        dom.set_attribute(page, "style", "width: 607px").unwrap();
        dom.set_attribute(body, "class", "dark").unwrap();
        dom.set_char_data(text, "changed").unwrap();
    }
    let after = dom.borrow().render_tree(d, &opts).unwrap();
    let delta = dom.borrow().render_delta(d, version, &opts).expect("incremental");
    let mut keys: Vec<u64> = delta
        .ops
        .iter()
        .map(|op| match op {
            DeltaOp::Node { key, kind } => {
                let fresh = after.nodes.iter().find(|n| n.key == *key).expect("rendered");
                assert_eq!(*kind, fresh.kind, "op for {key:#x}");
                *key
            }
            other => panic!("{other:?}"),
        })
        .collect();
    keys.sort_unstable();
    let mut want = vec![render_key(page), render_key(body), render_key(text)];
    want.sort_unstable();
    assert_eq!(keys, want);
}
