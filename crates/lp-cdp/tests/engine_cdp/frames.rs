//! Phase-1 child-frame contract over the real browser and protocol, with no
//! injected frame records or substituted DOM. These tests intentionally fail
//! until the browser dependency and CDP routing are integrated together.
use super::*;

fn enable(h: &mut Harness) {
    h.page("Page.enable", json!({}));
    h.page("DOM.enable", json!({}));
    h.page("Runtime.enable", json!({}));
}

fn add_srcdoc(h: &mut Harness, id: &str, text: &str, sandbox: Option<&str>) -> Vec<Value> {
    let html =
        format!("<!doctype html><head><script>globalThis.pageScriptRan = true</script></head><body><p>{text}</p>");
    let expression = format!(
        "(() => {{ const f = document.createElement('iframe'); f.id = {}; f.name = {}; {} f.srcdoc = {}; document.body.appendChild(f); }})()",
        json!(id), json!(id),
        sandbox.map(|s| format!("f.setAttribute('sandbox', {});", json!(s))).unwrap_or_default(),
        json!(html),
    );
    let mut messages = h.page_with_events("Runtime.evaluate", json!({"expression":expression}));
    messages.extend(h.run_for(150));
    messages
}

fn child(h: &mut Harness, name: &str) -> Value {
    let reply = h.page("Page.getFrameTree", json!({}));
    let tree = &reply["result"]["frameTree"];
    let children = tree["childFrames"].as_array().unwrap_or_else(|| panic!("missing native child frames: {reply}"));
    children
        .iter()
        .find(|c| c["frame"]["name"] == name)
        .unwrap_or_else(|| panic!("missing child {name}: {reply}"))
        .clone()
}

fn default_context(messages: &[Value], frame: &str) -> i64 {
    messages
        .iter()
        .rev()
        .find_map(|message| {
            let context = &message["params"]["context"];
            (message["method"] == "Runtime.executionContextCreated"
                && context["auxData"]["frameId"] == frame
                && context["auxData"]["isDefault"] == true)
                .then(|| context["id"].as_i64())
                .flatten()
        })
        .unwrap_or_else(|| panic!("missing default context for {frame}: {messages:#?}"))
}

fn eval_in(h: &mut Harness, context: i64, expression: &str) -> Value {
    let response =
        h.page("Runtime.evaluate", json!({"contextId":context,"expression":expression,"returnByValue":true}));
    assert!(response.get("error").is_none() && response["result"].get("exceptionDetails").is_none(), "{response}");
    response["result"]["result"]["value"].clone()
}

#[test]
fn child_frame_sandbox_srcdoc_has_real_context_and_one_page_session() {
    let mut h = Harness::new();
    enable(&mut h);
    let events = add_srcdoc(&mut h, "sandboxed", "child-content", Some("allow-downloads"));
    let tree = child(&mut h, "sandboxed");
    let frame = tree["frame"]["id"].as_str().unwrap();
    assert_ne!(frame, h.frame_id);
    assert_eq!(tree["frame"]["parentId"], h.frame_id);
    assert_eq!(tree["frame"]["url"], "about:srcdoc");
    let context = default_context(&events, frame);
    assert_eq!(
        eval_in(&mut h, context, "[document.querySelector('p').textContent, typeof pageScriptRan]"),
        json!(["child-content", "undefined"])
    );
    assert_eq!(eval_value(&mut h, "document.querySelector('iframe').contentDocument === null"), true);
    let attached = events
        .iter()
        .position(|m| m["method"] == "Page.frameAttached" && m["params"]["frameId"] == frame)
        .expect("frameAttached");
    let created = events
        .iter()
        .position(|m| {
            m["method"] == "Runtime.executionContextCreated" && m["params"]["context"]["auxData"]["frameId"] == frame
        })
        .expect("child context");
    assert!(attached < created, "{events:#?}");
    assert!(!events.iter().any(|m| m["method"] == "Target.attachedToTarget"), "child stays in its parent target");
    assert!(events.iter().filter(|m| m.get("method").is_some()).all(|m| m["sessionId"] == h.session));
}

#[test]
fn child_frame_owner_description_and_isolated_world_use_native_identity() {
    let mut h = Harness::new();
    enable(&mut h);
    add_srcdoc(&mut h, "same-origin", "inside", None);
    let tree = child(&mut h, "same-origin");
    let frame = tree["frame"]["id"].as_str().unwrap();
    let object = h.page("Runtime.evaluate", json!({"expression":"document.querySelector('iframe')"}));
    let object = object["result"]["result"]["objectId"].as_str().unwrap();
    let described = h.page("DOM.describeNode", json!({"objectId":object,"depth":1}));
    assert_eq!(described["result"]["node"]["frameId"], frame, "{described}");
    assert_eq!(described["result"]["node"]["contentDocument"]["nodeType"], 9, "{described}");
    let owner = h.page("DOM.getFrameOwner", json!({"frameId":frame}));
    assert_eq!(owner["result"]["backendNodeId"], described["result"]["node"]["backendNodeId"], "{owner}");
    let world = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"child-utility"}));
    let context = world["result"]["executionContextId"].as_i64().unwrap_or_else(|| panic!("{world}"));
    assert_eq!(eval_in(&mut h, context, "document.querySelector('p').textContent"), "inside");
    assert_eq!(eval_in(&mut h, context, "globalThis.childOnly = 17; childOnly"), 17);
    assert_eq!(eval_value(&mut h, "typeof childOnly"), "undefined");
    let node = h.page("Runtime.evaluate", json!({"contextId":context,"expression":"document.body"}));
    let node = node["result"]["result"]["objectId"].as_str().unwrap();
    let body = h.page("DOM.describeNode", json!({"objectId":node,"depth":0}));
    assert_eq!(body["result"]["node"]["nodeName"], "BODY", "{body}");
    let geometry = h.page("DOM.getContentQuads", json!({"objectId":node}));
    assert!(
        geometry["error"]["message"].as_str().is_some_and(|s| s.contains("child frame")),
        "phase1 cannot pretend child-local coordinates are top-level: {geometry}"
    );
}

#[test]
fn child_frame_navigation_and_removal_preserve_sibling_and_main_contexts() {
    let mut h = Harness::new();
    enable(&mut h);
    let first_events = add_srcdoc(&mut h, "first", "one", None);
    let sibling_events = add_srcdoc(&mut h, "second", "two", None);
    let first = child(&mut h, "first")["frame"]["id"].as_str().unwrap().to_owned();
    let sibling = child(&mut h, "second")["frame"]["id"].as_str().unwrap().to_owned();
    let old_context = default_context(&first_events, &first);
    let sibling_context = default_context(&sibling_events, &sibling);
    eval_in(&mut h, sibling_context, "globalThis.siblingMarker = 42");
    eval_value(&mut h, "globalThis.mainMarker = 7");
    let mut events = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression":"document.getElementById('first').srcdoc = '<p>replacement</p>'"}),
    );
    events.extend(h.run_for(150));
    assert!(
        !events.iter().any(|m| m["method"] == "Runtime.executionContextsCleared"),
        "child navigation must not clear the target's other contexts: {events:#?}"
    );
    assert_eq!(
        eval_in(&mut h, sibling_context, "[siblingMarker, document.querySelector('p').textContent]"),
        json!([42, "two"])
    );
    assert_eq!(eval_value(&mut h, "mainMarker"), 7);
    assert_eq!(child(&mut h, "first")["frame"]["id"], first, "navigation preserves frame identity");
    let stale = h.page("Runtime.evaluate", json!({"contextId":old_context,"expression":"1"}));
    assert!(stale.get("error").is_some(), "old child realm must be disposed: {stale}");
    let current = default_context(&events, &first);
    let mut removed =
        h.page_with_events("Runtime.evaluate", json!({"expression":"document.getElementById('first').remove()"}));
    removed.extend(h.run_for(50));
    let destroyed = removed
        .iter()
        .position(|m| {
            m["method"] == "Runtime.executionContextDestroyed" && m["params"]["executionContextId"] == current
        })
        .expect("destroyed child context");
    let detached = removed
        .iter()
        .position(|m| m["method"] == "Page.frameDetached" && m["params"]["frameId"] == first)
        .expect("detached child frame");
    assert!(destroyed < detached, "{removed:#?}");
    assert!(h.page("DOM.getFrameOwner", json!({"frameId":first})).get("error").is_some());
    assert_eq!(eval_in(&mut h, sibling_context, "siblingMarker"), 42);
}

#[test]
fn child_frame_late_runtime_and_lifecycle_enable_replay_each_live_frame() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    add_srcdoc(&mut h, "late", "late-frame", None);
    h.run_for(700);
    let tree = child(&mut h, "late");
    let child_id = tree["frame"]["id"].as_str().unwrap();
    let events = h.page_with_events("Runtime.enable", json!({}));
    let child_context = default_context(&events, child_id);
    default_context(&events, &h.frame_id);
    assert_eq!(eval_in(&mut h, child_context, "document.querySelector('p').textContent"), "late-frame");
    let events = h.page_with_events("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    for frame in [&h.frame_id, child_id] {
        let names: Vec<_> = events
            .iter()
            .filter(|m| m["method"] == "Page.lifecycleEvent" && m["params"]["frameId"] == frame.as_ref())
            .map(|m| m["params"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["DOMContentLoaded", "load", "networkAlmostIdle", "networkIdle"],
            "frame {frame}: {events:#?}"
        );
    }
}
