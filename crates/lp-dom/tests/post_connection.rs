//! Post-connection steps the tree queues for the bindings: "prepare the
//! script element" for scripts the parser did not insert (insertion, a child
//! inserted, a `src` added), in one order with the iframe steps.

use lp_dom::iframe::IframeChange;
use lp_dom::script::{ResourceKind, ResourceReason};
use lp_dom::*;

/// A watched (browsing-context) document with a body, queues drained.
fn page() -> (Dom, NodeId, NodeId) {
    let mut dom = Dom::new();
    dom.set_resource_tracking(true);
    let doc = dom.create_document(DocumentKind::Html, "http://t/");
    dom.set_document_active(doc, true).unwrap();
    let html = dom.create_element(doc, "html").unwrap();
    dom.append_child(doc, html).unwrap();
    let body = dom.create_element(doc, "body").unwrap();
    dom.append_child(html, body).unwrap();
    dom.watch_iframes(doc).unwrap();
    drain(&mut dom, doc);
    dom.take_resource_events();
    (dom, doc, body)
}

/// The queued post-connection steps, released as the bindings would.
fn drain(dom: &mut Dom, doc: NodeId) -> Vec<(NodeId, IframeChange)> {
    let events = dom.take_iframe_events(doc);
    for event in &events {
        dom.release(event.node);
    }
    events.iter().map(|e| (e.node, e.change)).collect()
}

fn script(dom: &mut Dom, doc: NodeId, text: &str) -> NodeId {
    let script = dom.create_element(doc, "script").unwrap();
    if !text.is_empty() {
        let t = dom.create_text(doc, text).unwrap();
        dom.append_child(script, t).unwrap();
    }
    script
}

#[test]
fn scripts_and_iframes_of_a_fragment_queue_in_tree_order() {
    let (mut dom, doc, body) = page();
    let fragment = dom.create_fragment(doc).unwrap();
    let first = script(&mut dom, doc, "a()");
    let frame = dom.create_element(doc, "iframe").unwrap();
    let legacy = dom.create_element(doc, "frame").unwrap();
    let last = script(&mut dom, doc, "b()");
    for node in [first, frame, legacy, last] {
        dom.append_child(fragment, node).unwrap();
    }
    assert!(drain(&mut dom, doc).is_empty(), "nothing is connected yet");
    dom.append_child(body, fragment).unwrap();
    assert_eq!(
        drain(&mut dom, doc),
        [
            (first, IframeChange::Script),
            (frame, IframeChange::Connected),
            (legacy, IframeChange::Connected),
            (last, IframeChange::Script)
        ]
    );
}

#[test]
fn a_script_in_a_shadow_tree_is_queued_when_its_host_connects() {
    let (mut dom, doc, body) = page();
    let host = dom.create_element(doc, "div").unwrap();
    let root = dom.attach_shadow(host, ShadowRootMode::Closed).unwrap();
    let inner = script(&mut dom, doc, "a()");
    dom.append_child(root, inner).unwrap();
    let light = script(&mut dom, doc, "b()");
    dom.append_child(host, light).unwrap();
    assert!(drain(&mut dom, doc).is_empty());
    dom.append_child(body, host).unwrap();
    // Shadow-including tree order: the shadow tree before the children.
    assert_eq!(drain(&mut dom, doc), [(inner, IframeChange::Script), (light, IframeChange::Script)]);
}

#[test]
fn children_changed_steps_come_before_the_inserted_nodes_steps() {
    let (mut dom, doc, body) = page();
    let outer = script(&mut dom, doc, "");
    dom.append_child(body, outer).unwrap();
    drain(&mut dom, doc);
    dom.take_resource_events();
    // A fragment of a nested script and then text, inserted into the
    // connected empty script: the outer script's children changed steps run
    // before the nested script's post-connection steps.
    let fragment = dom.create_fragment(doc).unwrap();
    let nested = script(&mut dom, doc, "n()");
    dom.append_child(fragment, nested).unwrap();
    let text = dom.create_text(doc, "o()").unwrap();
    dom.append_child(fragment, text).unwrap();
    dom.append_child(outer, fragment).unwrap();
    assert_eq!(drain(&mut dom, doc), [(outer, IframeChange::Script), (nested, IframeChange::Script)]);
    let resources: Vec<_> = dom.take_resource_events().iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert!(resources.contains(&(outer, ResourceKind::Script, ResourceReason::ChildrenChanged)), "{resources:?}");

    // Removing a child, or changing a child's data, prepares nothing.
    dom.remove_node(text).unwrap();
    assert!(drain(&mut dom, doc).is_empty());
}

#[test]
fn only_a_src_the_script_lacked_prepares_a_connected_script() {
    let (mut dom, doc, body) = page();
    let s = script(&mut dom, doc, "");
    dom.set_attribute(s, "type", "text/template").unwrap();
    dom.append_child(body, s).unwrap();
    assert_eq!(drain(&mut dom, doc), [(s, IframeChange::Script)]);
    dom.take_resource_events();

    // Changing `type` (or any other attribute) does not prepare it again.
    dom.set_attribute(s, "type", "text/javascript").unwrap();
    dom.remove_attribute(s, "type").unwrap();
    dom.set_attribute(s, "async", "").unwrap();
    assert!(drain(&mut dom, doc).is_empty());
    assert!(dom.take_resource_events().iter().all(|e| e.kind != ResourceKind::Script));

    // A `src` where there was none does; changing or removing it does not.
    dom.set_attribute(s, "src", "/a.js").unwrap();
    assert_eq!(drain(&mut dom, doc), [(s, IframeChange::Script)]);
    let resources: Vec<_> = dom.take_resource_events().iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert_eq!(resources, [(s, ResourceKind::Script, ResourceReason::AttrChanged)]);
    dom.set_attribute(s, "src", "/b.js").unwrap();
    dom.remove_attribute(s, "src").unwrap();
    assert!(drain(&mut dom, doc).is_empty());
    assert!(dom.take_resource_events().is_empty());
}

#[test]
fn started_and_frameless_scripts_are_not_queued() {
    let (mut dom, doc, body) = page();
    let started = script(&mut dom, doc, "a()");
    dom.mark_script_started(started).unwrap();
    // The cloning steps copy "already started".
    let clone = dom.clone_node(started, true, None).unwrap();
    assert!(dom.script_info(clone).unwrap().already_started);
    let fresh = script(&mut dom, doc, "b()");
    let fresh_clone = dom.clone_node(fresh, true, None).unwrap();
    assert!(!dom.script_info(fresh_clone).unwrap().already_started);
    dom.append_child(body, started).unwrap();
    dom.append_child(body, clone).unwrap();
    assert!(drain(&mut dom, doc).is_empty(), "already started scripts are not prepared again");

    // A document without a browsing context runs no script.
    let other = dom.create_document(DocumentKind::Html, "http://t/other");
    let root = dom.create_element(other, "html").unwrap();
    dom.append_child(other, root).unwrap();
    let inert = script(&mut dom, other, "c()");
    dom.append_child(root, inert).unwrap();
    assert!(dom.take_iframe_events(other).is_empty());
    assert!(drain(&mut dom, doc).is_empty());
}

/// An inserted subtree of one element (holding text or nothing) is examined
/// without the subtree walks: it queues what the same element queues inside
/// a wrapper, whose insertion takes the walks.
#[test]
fn an_element_inserted_alone_queues_what_it_queues_inside_a_subtree() {
    fn build(dom: &mut Dom, doc: NodeId, name: &str, text: bool) -> NodeId {
        let e = dom.create_element(doc, name).unwrap();
        if name == "link" {
            dom.set_attribute(e, "rel", "stylesheet").unwrap();
        }
        if text {
            let t = dom.create_text(doc, "x()").unwrap();
            dom.append_child(e, t).unwrap();
        }
        e
    }
    for name in ["script", "iframe", "frame", "style", "link", "img", "p"] {
        for text in [false, true] {
            // Alone.
            let (mut dom, doc, body) = page();
            let alone = build(&mut dom, doc, name, text);
            dom.append_child(body, alone).unwrap();
            let steps: Vec<_> = drain(&mut dom, doc).into_iter().map(|(_, change)| change).collect();
            let resources: Vec<_> = dom.take_resource_events().iter().map(|e| (e.kind, e.reason)).collect();

            // Inside a `div`.
            let (mut dom, doc, body) = page();
            let inner = build(&mut dom, doc, name, text);
            let div = dom.create_element(doc, "div").unwrap();
            dom.append_child(div, inner).unwrap();
            assert!(drain(&mut dom, doc).is_empty(), "{name}: nothing is connected yet");
            dom.take_resource_events();
            dom.append_child(body, div).unwrap();
            let wrapped_steps: Vec<_> = drain(&mut dom, doc).into_iter().map(|(_, change)| change).collect();
            let wrapped: Vec<_> = dom.take_resource_events().iter().map(|e| (e.kind, e.reason)).collect();

            assert_eq!(steps, wrapped_steps, "{name} (text: {text})");
            assert_eq!(resources, wrapped, "{name} (text: {text})");
            let expects_step = matches!(name, "script" | "iframe" | "frame");
            assert_eq!(!steps.is_empty(), expects_step, "{name} (text: {text}): {steps:?}");
            let expects_resource = name != "p";
            assert_eq!(!resources.is_empty(), expects_resource, "{name} (text: {text}): {resources:?}");
        }
    }
}

/// A text node inserted into a connected `<style>` reports the style's
/// children changed, and one inserted while detached reports nothing.
#[test]
fn a_text_node_inserted_alone_reports_its_style_parent() {
    let (mut dom, doc, body) = page();
    let style = dom.create_element(doc, "style").unwrap();
    let detached = dom.create_text(doc, "a{}").unwrap();
    dom.append_child(style, detached).unwrap();
    assert!(dom.take_resource_events().is_empty(), "a detached style loads nothing");
    dom.append_child(body, style).unwrap();
    dom.take_resource_events();
    let text = dom.create_text(doc, "b{}").unwrap();
    dom.append_child(style, text).unwrap();
    let resources: Vec<_> = dom.take_resource_events().iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert_eq!(resources, [(style, ResourceKind::StyleText, ResourceReason::ChildrenChanged)]);
}
