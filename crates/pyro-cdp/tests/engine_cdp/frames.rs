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
    // Child geometry is reported in main-frame coordinates, never child-local.
    let geometry = h.page("DOM.getContentQuads", json!({"objectId":node}));
    let expected = top_level_origin(&mut h, "same-origin", context, "document.body");
    assert_eq!(&geometry["result"]["quads"][0].as_array().unwrap()[..2], &expected[..], "{geometry}");
}

/// (x, y) of `selector`'s client rect inside the child frame `frame_id`'s
/// document, offset by the owner iframe's client rect in the main frame.
fn top_level_origin(h: &mut Harness, frame_id: &str, context: i64, expr: &str) -> [Value; 2] {
    let owner = eval_value(
        h,
        &format!(
            "(() => {{ const f = document.getElementById({}); const r = f.getBoundingClientRect(); return [r.x + f.clientLeft, r.y + f.clientTop]; }})()",
            json!(frame_id)
        ),
    );
    let inner =
        eval_in(h, context, &format!("(() => {{ const r = {expr}.getBoundingClientRect(); return [r.x, r.y]; }})()"));
    [
        json!(owner[0].as_f64().unwrap() + inner[0].as_f64().unwrap()),
        json!(owner[1].as_f64().unwrap() + inner[1].as_f64().unwrap()),
    ]
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
            .filter(|m| m["method"] == "Page.lifecycleEvent" && m["params"]["frameId"] == *frame)
            .map(|m| m["params"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["DOMContentLoaded", "load", "networkAlmostIdle", "networkIdle"],
            "frame {frame}: {events:#?}"
        );
    }
}

#[test]
fn child_http_navigation_fetch_and_body_capture_preserve_other_frames() {
    let (address, server) = serve_html(4, "<!doctype html><p>http-child</p>");
    let mut h = Harness::new();
    enable(&mut h);
    h.page("Network.enable", json!({}));
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    let root = h.page("Page.navigate", json!({"url":format!("http://{address}/root")}));
    let root_loader = root["result"]["loaderId"].clone();
    h.run_for(80);
    add_srcdoc(&mut h, "http-first", "first", None);
    add_srcdoc(&mut h, "http-second", "second", None);
    let first = child(&mut h, "http-first")["frame"]["id"].as_str().unwrap().to_owned();
    let second = child(&mut h, "http-second")["frame"]["id"].as_str().unwrap().to_owned();
    let second_reply = h.page("Page.navigate", json!({"frameId":second,"url":format!("http://{address}/second")}));
    let second_loader = second_reply["result"]["loaderId"].clone();
    h.run_for(80);
    let mut events =
        h.page_with_events("Page.navigate", json!({"frameId":first,"url":format!("http://{address}/first")}));
    events.extend(h.run_for(80));
    let first_context = default_context(&events, &first);
    let first_loader = child(&mut h, "http-first")["frame"]["loaderId"].clone();
    let request =
        events.iter().find(|e| e["method"] == "Network.requestWillBeSent" && e["params"]["frameId"] == first).unwrap();
    assert_eq!(request["params"]["requestId"], first_loader);
    assert_eq!(request["params"]["type"], "Document");
    assert!(events.iter().any(|e| e["method"] == "Network.responseReceived" && e["params"]["frameId"] == first));
    assert!(!events.iter().any(|e| matches!(
        e["method"].as_str(),
        Some("Page.loadEventFired" | "Page.domContentEventFired" | "Runtime.executionContextsCleared")
    )));
    for loader in [&root_loader, &second_loader, &first_loader] {
        let body = h.page("Network.getResponseBody", json!({"requestId":loader}));
        assert!(body["result"]["body"].as_str().is_some_and(|s| s.contains("http-child")), "{body}");
    }
    let mut events = h.page_with_events(
        "Runtime.evaluate",
        json!({"contextId":first_context,
        "expression":"fetch('/body').then(r => r.text())","awaitPromise":true,"returnByValue":true}),
    );
    events.extend(h.run_for(80));
    let request =
        events.iter().find(|e| e["method"] == "Network.requestWillBeSent" && e["params"]["type"] == "Fetch").unwrap();
    assert_eq!(request["params"]["frameId"], first);
    assert_eq!(request["params"]["loaderId"], first_loader);
    let fetch_id = request["params"]["requestId"].clone();
    assert!(h.page("Network.getResponseBody", json!({"requestId":fetch_id}))["result"]["body"].is_string());
    h.page("Page.navigate", json!({"frameId":first,"url":"about:blank"}));
    assert!(h.page("Network.getResponseBody", json!({"requestId":fetch_id}))["error"].is_object());
    assert!(h.page("Network.getResponseBody", json!({"requestId":second_loader}))["result"]["body"].is_string());
    let history = h.page("Page.getNavigationHistory", json!({}));
    assert_eq!(
        history["result"]["entries"].as_array().unwrap().len(),
        1,
        "child navigation does not alter root history (the root's first navigation replaced about:blank): {history}"
    );
    server.join().unwrap();
}

#[test]
fn child_fetch_interception_and_detach_reply_are_frame_scoped() {
    let mut h = Harness::new();
    enable(&mut h);
    h.page("Network.enable", json!({}));
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    add_srcdoc(&mut h, "routed", "before", None);
    let frame = child(&mut h, "routed")["frame"]["id"].as_str().unwrap().to_owned();
    h.page("Fetch.enable", json!({}));
    let session = h.session.clone();
    let navigation = h.send(Some(&session), "Page.navigate", json!({"frameId":frame,"url":"http://127.0.0.1:1/child"}));
    let paused = until_event(&mut h, "Fetch.requestPaused");
    let paused = paused.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap();
    assert_eq!(paused["params"]["frameId"], frame);
    assert_eq!(paused["params"]["resourceType"], "Document");
    let mut events = h.page_with_events("Fetch.fulfillRequest", json!({"requestId":paused["params"]["requestId"],
        "responseCode":200,"responseHeaders":[{"name":"content-type","value":"text/html"}],"body":"PHA+cm91dGVkPC9wPg=="}));
    if !events.iter().any(|e| e["id"] == navigation) {
        events.extend(h.until_response(navigation));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|e| {
        e["method"] == "Page.lifecycleEvent" && e["params"]["frameId"] == frame && e["params"]["name"] == "load"
    }) {
        assert!(Instant::now() < deadline, "child load missing: {events:#?}");
        h.browser.host_mut().run_until_work(Some(deadline), || false);
        h.browser.host_mut().flush();
        events.extend(h.drain());
    }
    let context = default_context(&events, &frame);
    assert_eq!(eval_in(&mut h, context, "document.querySelector('p').textContent"), "routed");
    let navigation =
        h.send(Some(&session), "Page.navigate", json!({"frameId":frame,"url":"http://127.0.0.1:1/detached"}));
    until_event(&mut h, "Fetch.requestPaused");
    let mut events =
        h.page_with_events("Runtime.evaluate", json!({"expression":"document.querySelector('iframe').remove()"}));
    if !events.iter().any(|e| e["id"] == navigation) {
        events.extend(h.until_response(navigation));
    }
    let reply = events.iter().find(|e| e["id"] == navigation).unwrap();
    assert!(reply["result"]["errorText"].is_string(), "{events:#?}");
    assert_eq!(events.iter().filter(|e| e["id"] == navigation).count(), 1);
    assert_eq!(h.page("Fetch.disable", json!({}))["result"], json!({}));
}

#[test]
fn child_backend_ids_are_document_scoped_and_geometry_uses_main_frame_coordinates() {
    let mut h = Harness::new();
    enable(&mut h);
    let root = h.page("Runtime.evaluate", json!({"expression":"document.body"}));
    let root = h.page("DOM.describeNode", json!({"objectId":root["result"]["result"]["objectId"],"depth":0}));
    let root_id = root["result"]["node"]["backendNodeId"].clone();
    let events = add_srcdoc(&mut h, "nodes", "child", None);
    let frame = child(&mut h, "nodes")["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&events, &frame);
    let object = h.page("Runtime.evaluate", json!({"contextId":context,"expression":"document.body"}));
    let object_id = object["result"]["result"]["objectId"].clone();
    let body = h.page("DOM.describeNode", json!({"objectId":object_id,"depth":0}));
    let backend = body["result"]["node"]["backendNodeId"].clone();
    let resolved = h.page("DOM.resolveNode", json!({"backendNodeId":backend}));
    assert!(resolved["result"]["object"]["objectId"].is_string(), "{resolved}");
    let root_context = default_context(&h.page_with_events("Runtime.enable", json!({})), &h.frame_id);
    assert!(h.page("DOM.resolveNode", json!({"backendNodeId":backend,"executionContextId":root_context}))["error"]
        .is_object());
    // Child-frame geometry is in main-frame viewport coordinates (Chrome's
    // same-process iframes): the owner's content box origin plus the node's
    // rect.
    let owner = eval_value(
        &mut h,
        "(() => { const f = document.getElementById('nodes'); const r = f.getBoundingClientRect(); return [r.x + f.clientLeft, r.y + f.clientTop]; })()",
    );
    let inner = eval_in(
        &mut h,
        context,
        "(() => { const r = document.body.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; })()",
    );
    let (ox, oy) = (owner[0].as_f64().unwrap(), owner[1].as_f64().unwrap());
    let (x, y) = (inner[0].as_f64().unwrap() + ox, inner[1].as_f64().unwrap() + oy);
    let (w, hgt) = (inner[2].as_f64().unwrap(), inner[3].as_f64().unwrap());
    let quads = h.page("DOM.getContentQuads", json!({"backendNodeId":backend}));
    assert_eq!(quads["result"]["quads"][0], json!([x, y, x + w, y, x + w, y + hgt, x, y + hgt]), "{quads}");
    let model = h.page("DOM.getBoxModel", json!({"backendNodeId":backend}));
    assert_eq!(model["result"]["model"]["border"][0].as_f64(), Some(x), "{model}");
    assert_eq!(h.page("DOM.scrollIntoViewIfNeeded", json!({"backendNodeId":backend}))["result"], json!({}));
    h.page("Page.navigate", json!({"frameId":frame,"url":"about:blank"}));
    assert!(h.page("DOM.describeNode", json!({"backendNodeId":backend}))["error"].is_object());
    assert!(h.page("DOM.describeNode", json!({"objectId":object_id}))["error"].is_object());
    assert_eq!(
        h.page("DOM.describeNode", json!({"backendNodeId":root_id,"depth":0}))["result"]["node"]["nodeName"],
        "BODY"
    );
}

#[test]
fn concurrent_child_navigation_world_installation_and_origin_are_isolated() {
    let (address, server) = serve_html(1, "<!doctype html><p>parent</p>");
    let mut h = Harness::new();
    enable(&mut h);
    h.page("Page.navigate", json!({"url":format!("http://{address}/root")}));
    h.run_for(80);
    let first_events = add_srcdoc(&mut h, "first", "one", None);
    let second_events = add_srcdoc(&mut h, "second", "two", Some("allow-scripts"));
    let first = child(&mut h, "first")["frame"].clone();
    let second = child(&mut h, "second")["frame"].clone();
    assert_eq!(first["securityOrigin"], format!("http://{address}"));
    assert_eq!(second["securityOrigin"], "://");
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    let first_old = default_context(&first_events, first_id);
    let second_old = default_context(&second_events, second_id);
    let events = h.page_with_events(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"globalThis.utilityMarker = 91",
        "worldName":"utility","runImmediately":true}),
    );
    let root_id = h.frame_id.clone();
    for id in [root_id.as_str(), first_id, second_id] {
        let context = events
            .iter()
            .find(|e| {
                e["method"] == "Runtime.executionContextCreated"
                    && e["params"]["context"]["auxData"]["frameId"] == *id
                    && e["params"]["context"]["name"] == "utility"
            })
            .unwrap_or_else(|| panic!("missing utility for {id}: {events:#?}"))["params"]["context"]["id"]
            .as_i64()
            .unwrap();
        assert_eq!(
            h.page("Runtime.evaluate", json!({"contextId":context,"expression":"utilityMarker","returnByValue":true}))
                ["result"]["result"]["value"],
            91
        );
    }
    assert_eq!(eval_in(&mut h, first_old, "typeof utilityMarker"), "undefined");
    h.page("Fetch.enable", json!({}));
    let session = h.session.clone();
    let one =
        h.send(Some(&session), "Page.navigate", json!({"frameId":first_id,"url":format!("http://{address}/one")}));
    let paused_one = until_event(&mut h, "Fetch.requestPaused");
    let paused_one =
        paused_one.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["params"]["requestId"].clone();
    let two =
        h.send(Some(&session), "Page.navigate", json!({"frameId":second_id,"url":format!("http://{address}/two")}));
    let paused_two = until_event(&mut h, "Fetch.requestPaused");
    let paused_two =
        paused_two.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["params"]["requestId"].clone();
    for (pause, request, frame, old) in
        [(paused_two, two, second_id, second_old), (paused_one, one, first_id, first_old)]
    {
        let mut events = h.page_with_events(
            "Fetch.fulfillRequest",
            json!({"requestId":pause,"responseCode":200,"body":"PHA+bmV3PC9wPg=="}),
        );
        if !events.iter().any(|e| e["id"] == request) {
            events.extend(h.until_response(request));
        }
        assert_eq!(events.iter().find(|e| e["id"] == request).unwrap()["result"]["frameId"], frame);
        assert!(!events.iter().any(|e| e["method"] == "Runtime.executionContextsCleared"));
        assert!(events
            .iter()
            .any(|e| e["method"] == "Runtime.executionContextDestroyed" && e["params"]["executionContextId"] == old));
        let current = default_context(&events, frame);
        assert_eq!(eval_in(&mut h, current, "typeof utilityMarker"), "undefined");
        let utility = events
            .iter()
            .find(|e| {
                e["method"] == "Runtime.executionContextCreated"
                    && e["params"]["context"]["auxData"]["frameId"] == frame
                    && e["params"]["context"]["name"] == "utility"
            })
            .unwrap();
        assert_eq!(eval_in(&mut h, utility["params"]["context"]["id"].as_i64().unwrap(), "utilityMarker"), 91);
    }
    h.page("Fetch.disable", json!({}));
    server.join().unwrap();
}

#[test]
fn run_immediately_tolerates_a_script_removing_a_later_frame() {
    let mut h = Harness::new();
    enable(&mut h);
    add_srcdoc(&mut h, "removed-by-init", "child", None);
    let result = h.page("Page.addScriptToEvaluateOnNewDocument", json!({
        "worldName":"remove-child", "runImmediately":true,
        "source":"const child = document.querySelector('iframe'); if (child) child.remove(); globalThis.initCompleted = true;"
    }));
    assert!(
        result["result"]["identifier"].is_string(),
        "script-induced frame removal must not fail installation: {result}"
    );
    assert!(h.page("Page.getFrameTree", json!({}))["result"]["frameTree"].get("childFrames").is_none());
    // Installation still applies to a later document, not just the first
    // world evaluated before the old child disappeared.
    let events = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    let context = events
        .iter()
        .find(|e| e["method"] == "Runtime.executionContextCreated" && e["params"]["context"]["name"] == "remove-child")
        .unwrap()["params"]["context"]["id"]
        .as_i64()
        .unwrap();
    assert_eq!(eval_in(&mut h, context, "initCompleted"), true);
}

// Focus routing contracts: Input commands belong to the page's native focus
// chain, independently of which Runtime context last executed inspector JS.
fn focus_fixture(h: &mut Harness, name: &str, sandbox: Option<&str>) -> (String, i64) {
    let messages = add_srcdoc(h, name, "focus fixture", sandbox);
    let id = child(h, name)["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&messages, &id);
    eval_in(h, context, "document.body.innerHTML = '<input id=field><input id=other>'; true");
    (id, context)
}

fn insert(h: &mut Harness, text: &str) {
    let result = h.page("Input.insertText", json!({"text":text}));
    assert_eq!(result["result"], json!({}), "{result}");
}

#[test]
fn frame_focus_parent_input_remains_usable_with_live_children() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, child_context) = focus_fixture(&mut h, "child", None);
    eval_value(&mut h, "const field=document.createElement('input');field.id='parent';document.body.appendChild(field);field.focus();true");
    // Inspector evaluation in a child is not a focus transition.
    eval_in(&mut h, child_context, "document.querySelector('#field').value");
    insert(&mut h, "parent");
    assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "parent");
    assert_eq!(eval_in(&mut h, child_context, "document.querySelector('#field').value"), "");
}

#[test]
fn frame_focus_child_text_and_keys_follow_native_focus_in_each_sandbox() {
    for sandbox in [None, Some("allow-scripts"), Some("")] {
        let mut h = Harness::new();
        enable(&mut h);
        let (_, child_context) = focus_fixture(&mut h, "child", sandbox);
        eval_value(&mut h, "const field=document.createElement('input');field.id='parent';document.body.appendChild(field);field.focus();true");
        eval_in(&mut h, child_context, "document.querySelector('#field').focus();true");
        // Root inspector work must not steal child focus either.
        assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "");
        insert(&mut h, "child");
        let key = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"X","code":"KeyX","text":"X","windowsVirtualKeyCode":88}),
        );
        assert_eq!(key["result"], json!({}), "{key}");
        let key = h.page("Input.dispatchKeyEvent", json!({"type":"keyUp","key":"X","code":"KeyX"}));
        assert_eq!(key["result"], json!({}), "{key}");
        assert_eq!(eval_in(&mut h, child_context, "document.querySelector('#field').value"), "childX");
        assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "");
        eval_value(&mut h, "document.querySelector('#parent').focus();true");
        insert(&mut h, "root");
        assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "root");
        assert_eq!(eval_in(&mut h, child_context, "document.querySelector('#field').value"), "childX");
    }
}

#[test]
fn frame_focus_sibling_switch_and_detach_never_redirect_to_old_parent_input() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, a) = focus_fixture(&mut h, "a", None);
    let (_, b) = focus_fixture(&mut h, "b", None);
    eval_value(&mut h, "const field=document.createElement('input');field.id='parent';document.body.appendChild(field);field.focus();true");
    eval_in(&mut h, a, "document.querySelector('#field').focus();true");
    insert(&mut h, "A");
    eval_in(&mut h, b, "document.querySelector('#field').focus();true");
    insert(&mut h, "B");
    assert_eq!(eval_in(&mut h, a, "document.querySelector('#field').value"), "A");
    assert_eq!(eval_in(&mut h, b, "document.querySelector('#field').value"), "B");
    eval_value(&mut h, "document.querySelector('#b').remove();true");
    insert(&mut h, "discarded");
    assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "");
    assert_eq!(eval_in(&mut h, a, "document.querySelector('#field').value"), "A");
    eval_value(&mut h, "document.querySelector('#parent').focus();true");
    insert(&mut h, "P");
    assert_eq!(eval_value(&mut h, "document.querySelector('#parent').value"), "P");
}

#[test]
fn frame_focus_dom_focus_accepts_child_node_and_reports_top_level_geometry() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, context) = focus_fixture(&mut h, "child", None);
    let object =
        h.page("Runtime.evaluate", json!({"contextId":context,"expression":"document.querySelector('#field')"}));
    let object = object["result"]["result"]["objectId"].as_str().unwrap();
    let focus = h.page("DOM.focus", json!({"objectId":object}));
    assert_eq!(focus["result"], json!({}), "{focus}");
    insert(&mut h, "focused");
    assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "focused");
    let geometry = h.page("DOM.getBoxModel", json!({"objectId":object}));
    let border = geometry["result"]["model"]["border"].as_array().unwrap_or_else(|| panic!("{geometry}"));
    let local = eval_in(&mut h, context, "document.querySelector('#field').getBoundingClientRect().y");
    assert!(border[1].as_f64().unwrap() > local.as_f64().unwrap(), "offset by the owner iframe: {geometry}");
}

#[test]
fn frame_focus_keydown_retargets_text_within_same_document() {
    let mut h = Harness::new();
    enable(&mut h);
    eval_value(&mut h, "document.body.innerHTML='<input id=first><input id=second>';const first=document.querySelector('#first');const second=document.querySelector('#second');first.addEventListener('keydown',()=>second.focus(),{once:true});first.focus();true");
    let key = h.page(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"x","code":"KeyX","text":"x","windowsVirtualKeyCode":88}),
    );
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(
        eval_value(&mut h, "[document.querySelector('#first').value,document.querySelector('#second').value]"),
        json!(["", "x"])
    );
}

#[test]
fn frame_focus_keydown_cross_frame_switch_suppresses_old_key_text() {
    for raw in [false, true] {
        let mut h = Harness::new();
        enable(&mut h);
        let (_, context) = focus_fixture(&mut h, "child", None);
        eval_value(&mut h, "const first=document.createElement('input');first.id='first';document.body.appendChild(first);first.addEventListener('keydown',()=>document.querySelector('iframe').contentDocument.querySelector('#field').focus(),{once:true});first.focus();true");
        let key = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":if raw {"rawKeyDown"} else {"keyDown"},"key":"x","code":"KeyX","text":if raw {""} else {"x"},"windowsVirtualKeyCode":88}),
        );
        assert_eq!(key["result"], json!({}), "{key}");
        if raw {
            let key = h.page("Input.dispatchKeyEvent", json!({"type":"char","key":"x","text":"x","code":"KeyX"}));
            assert_eq!(key["result"], json!({}), "{key}");
        }
        let key = h.page("Input.dispatchKeyEvent", json!({"type":"keyUp","key":"x","code":"KeyX"}));
        assert_eq!(key["result"], json!({}), "{key}");
        assert_eq!(eval_value(&mut h, "document.querySelector('#first').value"), "");
        assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "");
        insert(&mut h, "next");
        assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "next");
    }
}

#[test]
fn frame_focus_nested_detach_returns_key_target_to_root_body() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, outer) = focus_fixture(&mut h, "outer", None);
    let mut messages = h.page_with_events("Runtime.evaluate", json!({"contextId":outer,"expression":"const f=document.createElement('iframe');f.id='inner';f.name='inner';f.srcdoc='<input id=field>';document.body.appendChild(f);true"}));
    messages.extend(h.run_for(150));
    let inner_tree = &child(&mut h, "outer")["childFrames"][0];
    let inner_id = inner_tree["frame"]["id"].as_str().unwrap();
    let inner = default_context(&messages, inner_id);
    eval_value(
        &mut h,
        "globalThis.keyTargets=[];document.addEventListener('keydown',e=>keyTargets.push(e.target.tagName));true",
    );
    eval_in(&mut h, inner, "document.querySelector('#field').focus();true");
    assert_eq!(eval_value(&mut h, "document.activeElement.id"), "outer");
    assert_eq!(eval_in(&mut h, outer, "document.activeElement.id"), "inner");
    eval_in(&mut h, outer, "document.querySelector('#inner').remove();true");
    assert_eq!(eval_value(&mut h, "document.activeElement.tagName"), "BODY");
    assert_eq!(eval_in(&mut h, outer, "document.activeElement.tagName"), "BODY");
    let key = h.page("Input.dispatchKeyEvent", json!({"type":"rawKeyDown","key":"x","code":"KeyX"}));
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(eval_value(&mut h, "keyTargets"), json!(["BODY"]));
}

#[test]
fn frame_focus_raw_char_suppression_is_one_shot_and_reset_by_key_boundaries() {
    fn key(h: &mut Harness, kind: &str, text: &str) {
        let result = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":"x","code":"KeyX","text":text,"windowsVirtualKeyCode":88}),
        );
        assert_eq!(result["result"], json!({}), "{result}");
    }
    for scenario in ["one-shot", "keyup-reset", "keydown-reset", "insert-independent", "combined", "empty-down"] {
        let mut h = Harness::new();
        enable(&mut h);
        let (_, context) = focus_fixture(&mut h, "child", None);
        eval_value(&mut h, "const first=document.createElement('input');first.id='first';document.body.appendChild(first);first.addEventListener('keydown',()=>document.querySelector('iframe').contentDocument.querySelector('#field').focus(),{once:true});first.focus();true");
        let combined = matches!(scenario, "combined" | "empty-down");
        key(&mut h, if combined { "keyDown" } else { "rawKeyDown" }, if scenario == "combined" { "x" } else { "" });
        match scenario {
            "one-shot" => {
                key(&mut h, "char", "x");
                assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "");
            }
            "keyup-reset" => key(&mut h, "keyUp", ""),
            "keydown-reset" => key(&mut h, "rawKeyDown", ""),
            "insert-independent" => insert(&mut h, "I"),
            _ => {}
        }
        key(&mut h, "char", "x");
        assert_eq!(eval_value(&mut h, "document.querySelector('#first').value"), "", "{scenario}");
        assert_eq!(
            eval_in(&mut h, context, "document.querySelector('#field').value"),
            if scenario == "insert-independent" { "I" } else { "x" },
            "{scenario}"
        );
    }
}

#[test]
fn frame_focus_invalid_char_does_not_consume_pending_suppression() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, context) = focus_fixture(&mut h, "child", None);
    eval_value(&mut h, "const field=document.createElement('input');document.body.appendChild(field);field.addEventListener('keydown',()=>document.querySelector('iframe').contentDocument.querySelector('#field').focus(),{once:true});field.focus();true");
    let raw = h.page("Input.dispatchKeyEvent", json!({"type":"rawKeyDown","key":"x","code":"KeyX"}));
    assert_eq!(raw["result"], json!({}), "{raw}");
    let invalid = h.page("Input.dispatchKeyEvent", json!({"type":"char","text":"x","modifiers":32}));
    assert_eq!(invalid["error"]["code"], -32602, "{invalid}");
    let key = h.page("Input.dispatchKeyEvent", json!({"type":"char","text":"x"}));
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "");
    let key = h.page("Input.dispatchKeyEvent", json!({"type":"char","text":"x"}));
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "x");
}

#[test]
fn frame_focus_char_suppression_is_page_scoped() {
    let mut h = Harness::new();
    enable(&mut h);
    let (_, context) = focus_fixture(&mut h, "child", None);
    eval_value(&mut h, "const field=document.createElement('input');document.body.appendChild(field);field.addEventListener('keydown',()=>document.querySelector('iframe').contentDocument.querySelector('#field').focus(),{once:true});field.focus();true");
    let raw = h.page("Input.dispatchKeyEvent", json!({"type":"rawKeyDown","key":"x","code":"KeyX"}));
    assert_eq!(raw["result"], json!({}), "{raw}");
    let other_context = h.call(None, "Target.createBrowserContext", json!({}))["result"]["browserContextId"].clone();
    let target = h.send(None, "Target.createTarget", json!({"url":"about:blank","browserContextId":other_context}));
    let messages = h.until_response(target);
    let other_session = messages.iter().find(|m| m["method"] == "Target.attachedToTarget").unwrap()["params"]
        ["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = h.call(
        Some(&other_session),
        "Runtime.evaluate",
        json!({"expression":"document.body.innerHTML='<input>';document.querySelector('input').focus();true"}),
    );
    assert!(other["result"].get("exceptionDetails").is_none(), "{other}");
    let key = h.call(Some(&other_session), "Input.dispatchKeyEvent", json!({"type":"char","text":"B"}));
    assert_eq!(key["result"], json!({}), "{key}");
    let other = h.call(
        Some(&other_session),
        "Runtime.evaluate",
        json!({"expression":"document.querySelector('input').value","returnByValue":true}),
    );
    assert_eq!(other["result"]["result"]["value"], "B");
    let key = h.page("Input.dispatchKeyEvent", json!({"type":"char","text":"A"}));
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "");
    let key = h.page("Input.dispatchKeyEvent", json!({"type":"char","text":"A"}));
    assert_eq!(key["result"], json!({}), "{key}");
    assert_eq!(eval_in(&mut h, context, "document.querySelector('#field').value"), "A");
}

#[test]
fn mouse_input_at_main_frame_coordinates_reaches_child_frame_content() {
    // Playwright clicks inside an iframe at main-frame coordinates.
    let mut h = Harness::new();
    enable(&mut h);
    let events = add_srcdoc(&mut h, "clicky", "child", None);
    // The approximate layout sizes iframes from their width/height only.
    eval_value(&mut h, "document.getElementById('clicky').setAttribute('width', '300'); document.getElementById('clicky').setAttribute('height', '150'); 1");
    let frame = child(&mut h, "clicky")["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&events, &frame);
    eval_in(
        &mut h,
        context,
        "(() => { const b = document.createElement('button'); b.id = 'b'; b.style.cssText = 'width:80px;height:30px'; \
           globalThis.hits = []; b.addEventListener('click', e => hits.push(e.isTrusted)); document.body.appendChild(b); return 1; })()",
    );
    let owner = eval_value(
        &mut h,
        "(() => { const f = document.getElementById('clicky'); const r = f.getBoundingClientRect(); return [r.x + f.clientLeft, r.y + f.clientTop]; })()",
    );
    let inner = eval_in(&mut h, context, "(() => { const r = document.getElementById('b').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()");
    let x = owner[0].as_f64().unwrap() + inner[0].as_f64().unwrap();
    let y = owner[1].as_f64().unwrap() + inner[1].as_f64().unwrap();
    for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":kind,"x":x,"y":y,"button":"left","buttons":buttons,"clickCount":1}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
    }
    assert_eq!(eval_in(&mut h, context, "hits"), json!([true]));
}

#[test]
fn child_frame_viewport_is_its_iframe_content_box() {
    let mut h = Harness::new();
    enable(&mut h);
    let events = add_srcdoc(&mut h, "sized", "inside", None);
    let frame = child(&mut h, "sized")["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&events, &frame);
    // A default iframe's content box is 300x150 (Chrome: the same).
    let metrics = "(() => [innerWidth, innerHeight, document.documentElement.clientWidth, \
                   document.querySelector('p').getBoundingClientRect().width, \
                   matchMedia('(max-width: 300px)').matches, matchMedia('(min-width: 301px)').matches])()";
    assert_eq!(eval_in(&mut h, context, metrics), json!([300, 150, 300, 284, true, false]));
    // The parent resizing the iframe resizes the child's viewport; padding
    // and borders are outside it.
    eval_value(
        &mut h,
        "(() => { const f = document.getElementById('sized'); f.style.cssText = 'width: 500px; height: 80px; padding: 3px; border: 4px solid'; return 0 })()",
    );
    assert_eq!(eval_in(&mut h, context, metrics), json!([500, 80, 500, 484, false, true]));
    // The top document keeps the emulated viewport; a percentage-sized
    // iframe follows it.
    assert_eq!(eval_value(&mut h, "innerWidth > 500"), true);
    eval_value(&mut h, "document.getElementById('sized').style.width = '50%'");
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":800,"height":600,"deviceScaleFactor":1,"mobile":false}),
    );
    assert_eq!(eval_in(&mut h, context, "innerWidth"), json!(392));
}

#[test]
fn child_window_gets_resize_when_its_iframe_box_changes() {
    let mut h = Harness::new();
    enable(&mut h);
    let events = add_srcdoc(&mut h, "resized", "inside", None);
    let frame = child(&mut h, "resized")["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&events, &frame);
    eval_in(&mut h, context, "globalThis.seen = []; onresize = () => seen.push([innerWidth, innerHeight]); innerWidth");
    // The parent restyles the iframe; nothing in the child reads its layout.
    eval_value(&mut h, "document.getElementById('resized').style.width = '500px'; 0");
    h.run_for(100);
    assert_eq!(eval_in(&mut h, context, "seen"), json!([[500, 150]]));
    // An emulated resize that leaves the iframe's box alone fires nothing
    // in the child; one that changes it fires once.
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":900,"height":600,"deviceScaleFactor":1,"mobile":false}),
    );
    h.run_for(100);
    assert_eq!(eval_in(&mut h, context, "seen.length"), json!(1));
    eval_value(&mut h, "document.getElementById('resized').style.width = '50%'; 0");
    h.run_for(100);
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":800,"height":600,"deviceScaleFactor":1,"mobile":false}),
    );
    h.run_for(100);
    assert_eq!(eval_in(&mut h, context, "seen"), json!([[500, 150], [442, 150], [392, 150]]));
}

#[test]
fn transformed_frame_quads_and_input_share_content_coordinates() {
    // A mobile Vitest viewport is an iframe inside a scaled wrapper. A CDP
    // locator must report root coordinates; Input must invert the same map.
    for (transform, expected) in [
        ("none", [348.0, 138.0, 408.0, 138.0, 408.0, 168.0, 348.0, 168.0]),
        ("scale(.8,.6)", [286.4, 94.8, 334.4, 94.8, 334.4, 112.8, 286.4, 112.8]),
        ("translateX(250px) rotate(90deg)", [182.0, 338.0, 182.0, 398.0, 152.0, 398.0, 152.0, 338.0]),
    ] {
        let mut h = Harness::new();
        enable(&mut h);
        let events = add_srcdoc(&mut h, "scaled", "child", None);
        let frame = child(&mut h, "scaled")["frame"]["id"].as_str().unwrap().to_owned();
        let context = default_context(&events, &frame);
        eval_value(&mut h, &format!(
            "document.body.style.cssText='margin:0;position:absolute;left:40px;top:30px;transform-origin:0 0;transform:{}'; \
             const f=document.getElementById('scaled'); f.style.cssText='width:400px;height:200px;border:5px solid;padding:3px'; \
             0", transform));
        assert_eq!(eval_in(&mut h, context, "[innerWidth,innerHeight]"), json!([400, 200]), "{transform}");
        eval_in(&mut h, context,
            "document.body.innerHTML='<button id=target style=\"position:absolute;left:300px;top:100px;width:60px;height:30px\">Next</button>'; \
             window.hits=[];document.getElementById('target').onclick=e=>hits.push([Math.round(e.clientX),Math.round(e.clientY),e.isTrusted]);0");
        let object =
            h.page("Runtime.evaluate", json!({"contextId":context,"expression":"document.getElementById('target')"}));
        let object = object["result"]["result"]["objectId"].as_str().unwrap();
        for method in ["DOM.getContentQuads", "DOM.getBoxModel"] {
            let result = h.page(method, json!({"objectId":object}));
            let quad = if method == "DOM.getContentQuads" {
                &result["result"]["quads"][0]
            } else {
                &result["result"]["model"]["border"]
            };
            for (actual, expected) in quad.as_array().expect("quad").iter().zip(expected) {
                assert!((actual.as_f64().unwrap() - expected).abs() < 0.05, "{transform}: {result}");
            }
        }
        let (x, y) = ((expected[0] + expected[4]) / 2.0, (expected[1] + expected[5]) / 2.0);
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            let result = h.page(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":x,"y":y,"button":"left","buttons":buttons,"clickCount":1}),
            );
            assert_eq!(result["result"], json!({}), "{result}");
        }
        assert_eq!(eval_in(&mut h, context, "hits"), json!([[330, 115, true]]), "{transform}");
    }
}

/// HTML user activation across frames: a click in a child activates its parent too ("activation
/// notification" reaches ancestors), and one document consuming the activation consumes it in
/// every document of the frame tree, so the same click cannot gate two requests in two frames.
#[test]
fn user_activation_reaches_ancestors_and_is_consumed_across_frames() {
    let mut h = Harness::new();
    enable(&mut h);
    let html = "<!doctype html><body style='margin:0;height:100vh'>child";
    let expression = format!(
        "(() => {{ document.body.style.margin = '0'; const f = document.createElement('iframe'); f.id = 'act'; f.name = 'act'; \
           f.allowFullscreen = true; f.style.cssText = 'border:0;width:300px;height:150px'; f.srcdoc = {}; document.body.appendChild(f); }})()",
        json!(html)
    );
    let mut events = h.page_with_events("Runtime.evaluate", json!({"expression":expression}));
    events.extend(h.run_for(150));
    let frame = child(&mut h, "act")["frame"]["id"].as_str().unwrap().to_owned();
    let context = default_context(&events, &frame);
    let request =
        "document.body.requestFullscreen().then(() => document.exitFullscreen().then(() => 'granted'), e => e.name)";
    let fullscreen = |h: &mut Harness, context: Option<i64>| {
        let mut params = json!({"expression":request,"awaitPromise":true,"returnByValue":true});
        if let Some(context) = context {
            params["contextId"] = json!(context);
        }
        h.page("Runtime.evaluate", params)["result"]["result"]["value"].clone()
    };
    let click = |h: &mut Harness| {
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            let r = h.page(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":50,"y":50,"button":"left","buttons":buttons,"clickCount":1}),
            );
            assert_eq!(r["result"], json!({}), "{r}");
        }
    };
    // No activation yet.
    assert_eq!(fullscreen(&mut h, None), json!("TypeError"));
    // A click in the child: the parent may use it...
    click(&mut h);
    assert_eq!(fullscreen(&mut h, None), json!("granted"));
    // ...and having consumed it, the child cannot use it again.
    assert_eq!(fullscreen(&mut h, Some(context)), json!("TypeError"));
    // The other way round: the child uses a fresh click, then the parent has none left.
    click(&mut h);
    assert_eq!(fullscreen(&mut h, Some(context)), json!("granted"));
    assert_eq!(fullscreen(&mut h, None), json!("TypeError"));
}

/// Runs `expression` (a promise) in the main frame and returns its value.
fn eval_await_main(h: &mut Harness, expression: &str) -> Value {
    let r = h.page("Runtime.evaluate", json!({"expression":expression,"awaitPromise":true,"returnByValue":true}));
    assert!(r["result"].get("exceptionDetails").is_none() && r.get("error").is_none(), "{expression}: {r}");
    r["result"]["result"]["value"].clone()
}

/// Iframe events as Chrome fires them (verified against Chrome 158 on the same pages): a static
/// iframe's load comes before its parent's; a removed frame's document gets `pagehide`,
/// `visibilitychange` (hidden) and, when its container's `allow` permits it, `unload`, right after
/// the removal and before the frame goes; its descendants get `pagehide` and `visibilitychange` only.
#[test]
fn iframe_load_order_and_removal_events_follow_chrome() {
    // An http page: `allow="unload"` ('src') matches the frame's origin, which an opaque data: URL
    // page's would not.
    let (addr, server) = serve_html(
        1,
        "<!doctype html><script>window.log = []; addEventListener('load', () => log.push('parent load'));</script>\
         <iframe id=s srcdoc='<p>static' onload=\"log.push('iframe load')\"></iframe>",
    );
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    server.join().unwrap();
    assert_eq!(eval_value(&mut h, "log"), json!(["iframe load", "parent load"]));
    let events = eval_await_main(
        &mut h,
        r#"(async () => {
          const wait = (ms) => new Promise((r) => setTimeout(r, ms));
          const out = {};
          const listen = (log, name, w) => { for (const t of ['pagehide', 'visibilitychange', 'unload']) w.addEventListener(t, () => log.push(name + ':' + t + ':' + w.document.visibilityState)); };
          for (const allow of [false, true]) {
            const log = [];
            const f = document.createElement('iframe');
            if (allow) f.allow = 'unload';
            f.srcdoc = '<p>x';
            document.body.appendChild(f);
            await wait(200);
            listen(log, 'f', f.contentWindow);
            f.remove();
            log.push('after remove');
            out[allow ? 'allow' : 'plain'] = log;
          }
          const log = [];
          const f = document.createElement('iframe');
          f.allow = 'unload';
          f.srcdoc = "<iframe allow=unload srcdoc='<p>in'></iframe>";
          document.body.appendChild(f);
          await wait(400);
          listen(log, 'outer', f.contentWindow);
          listen(log, 'inner', f.contentWindow.frames[0]);
          f.remove();
          out.nested = log;
          return out;
        })()"#,
    );
    assert_eq!(
        events,
        json!({
            "plain": ["f:pagehide:visible", "f:visibilitychange:hidden", "after remove"],
            "allow": ["f:pagehide:visible", "f:visibilitychange:hidden", "f:unload:hidden", "after remove"],
            "nested": ["outer:pagehide:visible", "outer:visibilitychange:hidden", "outer:unload:hidden",
                       "inner:pagehide:visible", "inner:visibilitychange:hidden"]
        })
    );
}

/// Moving an iframe reloads it (a new frame navigates), except when it is moved from its own
/// document's window `load` listener: then, as in Chrome, the new frame stays on its initial
/// about:blank, fires no `load` and does not hold its parent's load, so a load listener that moves
/// its iframe does not loop (WPT dom/nodes/crashtests/multiple-append-mutated-in-unload).
#[test]
fn moving_an_iframe_reloads_it_unless_moved_from_its_own_load() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<!doctype html><body><div id=a></div><div id=b></div><script>window.calls = 0;\
         function moveMe() { calls++; if (calls < 20) document.getElementById('b').appendChild(document.getElementById('self')); }\
         addEventListener('load', () => { window.parentLoadedAfter = calls; });</script>\
         <iframe id=self srcdoc=\"<script>addEventListener('load', () => parent.moveMe())</script>\"></iframe>",
    );
    assert_eq!(
        eval_await_main(
            &mut h,
            r#"(async () => {
              await new Promise((r) => setTimeout(r, 400));
              const self = document.getElementById('self');
              const out = { calls, parentLoadedAfter: window.parentLoadedAfter, selfBody: self.contentDocument.body ? self.contentDocument.body.innerHTML : null };
              const f = document.createElement('iframe');
              let loads = 0;
              f.onload = () => loads++;
              f.srcdoc = '<p>x';
              document.getElementById('a').appendChild(f);
              await new Promise((r) => setTimeout(r, 300));
              document.getElementById('b').appendChild(f);
              await new Promise((r) => setTimeout(r, 300));
              out.plainMoveLoads = loads;
              return out;
            })()"#,
        ),
        json!({"calls": 1, "parentLoadedAfter": 1, "selfBody": "", "plainMoveLoads": 2})
    );
}

/// Fullscreen delegation through a parent that inherited its origin: a nested srcdoc frame with
/// `allow="fullscreen"` ('src') inside a srcdoc frame of an http page has that page's origin as its
/// declared origin (its about:srcdoc parent's inherited one), so it may use fullscreen.
#[test]
fn fullscreen_delegates_through_an_inherited_origin_parent() {
    let (addr, server) = serve_html(1, "<!doctype html><body>top");
    let mut h = Harness::new();
    enable(&mut h);
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    server.join().unwrap();
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"new Promise((done) => {
            const f = document.createElement('iframe');
            f.allowFullscreen = true;
            f.srcdoc = "<iframe allow=fullscreen srcdoc='<p>in'></iframe><iframe srcdoc='<p>plain'></iframe>";
            document.body.appendChild(f);
            setTimeout(() => {
              const [a, b] = [f.contentWindow.frames[0], f.contentWindow.frames[1]];
              done([f.contentDocument.fullscreenEnabled, a.document.fullscreenEnabled, b.document.fullscreenEnabled]);
            }, 400);
          })"#, "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!([true, true, false]), "{r}");
}
