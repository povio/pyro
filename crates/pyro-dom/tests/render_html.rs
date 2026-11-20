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
fn script_text_handlers_and_comments_are_dropped() {
    let (dom, d) = doc(concat!(
        "<!DOCTYPE html><html><head><script>alert(1)</script></head>",
        "<body onload=\"x()\"><!-- c --><noscript>no js</noscript><template><p>t</p></template>",
        "<iframe src=\"/frame\"></iframe><p>kept</p></body></html>",
    ));
    let html = render(&dom.borrow(), d);
    // A script stays (without its text): it counts for sibling selectors (WPT CSS2 `content-072`).
    assert_eq!(
        html,
        "<!DOCTYPE html><html><head><script></script></head><body><iframe></iframe><p>kept</p></body></html>"
    );
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
    // The flat tree: each slot keeps its element (a `display: contents` box) with its assigned
    // nodes, or its fallback content, in it.
    assert!(
        html.contains(
            r#"<div id="host"><h1><slot name="title"><span slot="title">T</span></slot></h1><p><slot>light</slot></p><slot name="none">fallback</slot></div>"#
        ),
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
    // A script is an element without its text.
    let script = tree
        .nodes
        .iter()
        .position(|n| matches!(&n.kind, RenderNodeKind::Element { name, .. } if &*name.local == "script"))
        .expect("script element");
    assert!(tree.nodes.iter().all(|n| n.parent != script as u32 || !matches!(n.kind, RenderNodeKind::Text(_))));
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

#[test]
fn render_tree_keeps_event_handler_attributes_markup_drops() {
    // `content: attr(onclick)` reads them in the renderer; markup for another engine leaves them out.
    use pyro_dom::snapshot::{render_key, RenderNodeKind};
    let (dom, d) = doc("<p id=p onclick=\"PASS();\" title=t>x</p>");
    let p = find(&dom.borrow(), d, "#p");
    let opts = RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None };
    let tree = dom.borrow().render_tree(d, &opts).unwrap();
    let node = tree.nodes.iter().find(|n| n.key == render_key(p)).unwrap();
    match &node.kind {
        RenderNodeKind::Element { attrs, .. } => assert_eq!(
            attrs.iter().map(|(n, v)| (&*n.local, v.as_str())).collect::<Vec<_>>(),
            [("id", "p"), ("onclick", "PASS();"), ("title", "t")]
        ),
        k => panic!("{k:?}"),
    }
    assert!(render(&dom.borrow(), d).contains("<p id=\"p\" title=\"t\">x</p>"));
}

fn frame(html: &str, client: &str) -> FrameDocument {
    FrameDocument { html: html.into(), url: "http://test/frame".into(), scroll: (0.0, 0.0), client: client.into(), frames: Default::default(), sheets: Default::default() }
}

/// Every attribute the renderer reads from the engine is the engine's: a page attribute whose
/// serialised name is one (`srcdoc`, `data-pyro-frame`, the `data-pyro-` family), in any case or
/// namespace (`setAttributeNS`), is not written, in markup or in a keyed tree, and the engine's
/// come first. A frame carries only the index of its frame table entry: its document and client
/// are never in the markup. A page's namespaced `srcdoc` on an iframe of its own, written first
/// with its namespace dropped, made the renderer show page content under a genuine child frame's
/// client (security review cycles 1-3, HIGH 1).
#[test]
fn page_attributes_never_stand_in_for_the_engines() {
    let (dom, d) = doc(
        "<iframe id=f data-pyro-frame-client='OTHER#doc' data-pyro-frame-url='http://evil.test/' data-pyro-frame='9:0'></iframe>\
         <iframe id=g data-pyro-frame-client='OTHER#doc'></iframe><div id=x data-pyro-key=1 DATA-PYRO-RENDER-CLIP></div>",
    );
    let (f, g) = (find(&dom.borrow(), d, "#f"), find(&dom.borrow(), d, "#g"));
    {
        let mut dom = dom.borrow_mut();
        for el in [f, g] {
            dom.set_attribute_ns(el, Some("http://evil.test/ns"), "srcdoc", "<p>EVILNS</p>").unwrap();
            dom.set_attribute_ns(el, Some("http://evil.test/ns"), "e:srcdoc", "<p>EVILPREFIX</p>").unwrap();
            dom.set_attribute_ns(el, None, "SRCDOC", "<p>EVILCASE</p>").unwrap();
            dom.set_attribute_ns(el, Some("http://evil.test/ns"), "DATA-PYRO-FRAME", "1:0").unwrap();
        }
        dom.set_frame_document(f, Some(frame("<p>GENUINE</p>", "F#L")));
    }
    let key = |id: NodeId| pyro_dom::snapshot::render_key(id).to_string();
    let html = render(&dom.borrow(), d);
    for leaked in ["OTHER", "evil.test", "EVIL", "GENUINE", "F#L", "srcdoc", "data-pyro-key", "DATA-PYRO", "9:0"] {
        assert!(!html.contains(leaked), "{leaked}: {html}");
    }
    let table = dom.borrow().frame_table(d);
    let version = table.entries[0].1.version();
    assert_eq!(table.nonce.len(), 32, "128 bits");
    assert!(html.contains(&format!("data-pyro-frame=\"{}:{}:{version:016x}\"", table.nonce, key(f))), "{html}");
    assert_eq!(html.matches("data-pyro-frame=").count(), 1, "only the engine's, on the frame with a document: {html}");
    // The keyed tree: the same attributes, the engine's first.
    let tree = dom
        .borrow()
        .render_tree(d, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    for node in &tree.nodes {
        if let pyro_dom::snapshot::RenderNodeKind::Element { attrs, .. } = &node.kind {
            for (name, value) in attrs {
                let local = name.local.to_string().to_ascii_lowercase();
                assert!(local != "srcdoc" && !value.contains("EVIL") && !value.contains("OTHER"), "{name:?}={value}");
                if local.starts_with("data-pyro-") {
                    assert!(name.ns == markup5ever::ns!() && node.key == pyro_dom::snapshot::render_key(f), "{name:?}");
                }
            }
        }
    }
    // The table holds the document under that index.
    let table = dom.borrow().frame_table(d);
    assert_eq!(table.entries.len(), 1);
    let entry = &table.entries[0];
    assert_eq!((entry.0.as_str(), entry.1.client.as_str(), entry.1.html.as_str()), (key(f).as_str(), "F#L", "<p>GENUINE</p>"));
}

/// The frame documents a page sets cover its own documents only: a store shared by pages (a popup
/// shares its opener's) keeps another page's iframe entries when one page, with no frames of its
/// own, refreshes its (empty) set.
#[test]
fn frame_documents_are_replaced_per_page() {
    let (dom, opener) = doc("<iframe id=f></iframe>");
    let popup = parse_document(&dom, "http://test/popup", "<p>popup</p>");
    let f = find(&dom.borrow(), opener, "#f");
    let document = frame("<p>inner</p>", "F#L");
    let has = |dom: &Dom| dom.frame_table(opener).entries.iter().any(|(_, d)| d.html.contains("inner"));
    dom.borrow_mut().set_frame_documents(&[opener], vec![(f, document)]);
    assert!(has(&dom.borrow()));
    // The popup refreshes its own frames (it has none).
    dom.borrow_mut().set_frame_documents(&[popup], Vec::new());
    dom.borrow_mut().set_frame_content_heights(&[popup], &[]);
    assert!(has(&dom.borrow()), "the opener's iframe keeps its document");
    // The opener refreshing its own set without the frame does drop it.
    dom.borrow_mut().set_frame_documents(&[opener], Vec::new());
    assert!(!has(&dom.borrow()));
}


/// Raw text cannot change the tree when the markup is parsed again: a `<style>`'s text (set by
/// script, CSSOM, or a text node) holding its own end tag in any form is written so that the
/// reparse gives the same elements, never an element the attribute filter did not see (a frame
/// marker forged in style text: security review cycle 4, 1a).
#[test]
fn raw_text_survives_a_reparse_unchanged() {
    let hostile = [
        "</style><iframe data-pyro-frame=\"x:1:0\"></iframe>",
        "</STYLE ><iframe></iframe>",
        "</style\t><p>x</p>",
        "</style/><p>x</p>",
        "a { content: \"</style><b>\" } /* </style><i> */",
        "<!-- </style> --><iframe></iframe>",
        "</styles><p>y</p>",
    ];
    for text in hostile {
        let (dom, d) = doc("<style id=s></style><p id=after>after</p>");
        let s = find(&dom.borrow(), d, "#s");
        {
            let mut dom = dom.borrow_mut();
            let t = dom.create_text(d, text).unwrap();
            dom.append_child(s, t).unwrap();
        }
        let elements = |dom: &Dom, doc: NodeId| -> Vec<String> {
            dom.query_selector_all(doc, &parse_selectors("*").unwrap())
                .unwrap()
                .into_iter()
                .map(|e| dom.tag_name(e).unwrap())
                .collect()
        };
        let before = elements(&dom.borrow(), d);
        let html = render(&dom.borrow(), d);
        let (again, d2) = doc(&html);
        assert_eq!(elements(&again.borrow(), d2), before, "{text:?} -> {html}");
    }
}

/// A frame marker is accepted only with this document's nonce: the nonce is 128 random bits per
/// document, absent from everything page content can reach, so a marker the page made up (even
/// one naming a real index and version) names nothing (cycle 4, 1a).
#[test]
fn frame_markers_carry_a_per_document_nonce() {
    let (dom, d) = doc("<iframe id=f></iframe>");
    let (other, d2) = doc("<iframe id=g></iframe>");
    let f = find(&dom.borrow(), d, "#f");
    dom.borrow_mut().set_frame_document(f, Some(frame("<p>x</p>", "F#L")));
    let html = render(&dom.borrow(), d);
    let nonce = dom.borrow().frame_table(d).nonce;
    assert_eq!(nonce.len(), 32);
    assert!(nonce.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(render(&dom.borrow(), d), html, "the nonce stays for the document's life");
    assert_ne!(other.borrow().frame_table(d2).nonce, nonce, "each document its own");
}

/// A page attribute keeps its namespace and name in the snapshot (`xml:lang`, `xlink:href`, an
/// author namespace), in the keyed tree exactly; only one the renderer could read as an engine
/// attribute is left out, whatever its namespace or prefix (`e:srcdoc`, a namespaced `srcdoc`).
/// Namespaced attributes were all dropped on HTML elements (cycle 4 run: `:lang()` via
/// `xml:lang`, `[ns|attr]` selectors).
#[test]
fn namespaced_page_attributes_are_kept() {
    let (dom, d) = doc("<p id=p>x</p>");
    let p = find(&dom.borrow(), d, "#p");
    {
        let mut dom = dom.borrow_mut();
        dom.set_attribute_ns(p, Some("http://www.w3.org/XML/1998/namespace"), "xml:lang", "fr").unwrap();
        dom.set_attribute_ns(p, Some("http://author.test/ns"), "a:lang", "de").unwrap();
        dom.set_attribute_ns(p, Some("http://author.test/ns"), "e:srcdoc", "<p>EVIL</p>").unwrap();
        dom.set_attribute_ns(p, Some("http://author.test/other"), "srcdoc", "<p>EVIL</p>").unwrap();
    }
    let tree = dom
        .borrow()
        .render_tree(d, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let attrs: Vec<(String, String, String)> = tree
        .nodes
        .iter()
        .filter(|n| n.key == pyro_dom::snapshot::render_key(p))
        .flat_map(|n| match &n.kind {
            pyro_dom::snapshot::RenderNodeKind::Element { attrs, .. } => attrs.clone(),
            _ => Vec::new(),
        })
        .map(|(name, value)| (name.ns.to_string(), name.local.to_string(), value))
        .collect();
    assert!(attrs.contains(&("http://www.w3.org/XML/1998/namespace".into(), "lang".into(), "fr".into())), "{attrs:?}");
    assert!(attrs.contains(&("http://author.test/ns".into(), "lang".into(), "de".into())), "{attrs:?}");
    assert!(!attrs.iter().any(|(_, local, _)| local == "srcdoc"), "{attrs:?}");
}
