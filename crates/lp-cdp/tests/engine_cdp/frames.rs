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
        2,
        "child navigation does not alter root history: {history}"
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
fn child_backend_ids_are_document_scoped_and_input_does_not_silently_target_root() {
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
    for method in ["DOM.focus", "DOM.scrollIntoViewIfNeeded", "DOM.getBoxModel"] {
        let error = h.page(method, json!({"backendNodeId":backend}));
        assert!(error["error"]["message"].as_str().is_some_and(|s| s.contains("child frame")), "{error}");
    }
    for (method, params) in
        [("Input.insertText", json!({"text":"x"})), ("Input.dispatchKeyEvent", json!({"type":"keyDown","key":"x"}))]
    {
        let error = h.page(method, params);
        assert!(error["error"]["message"].as_str().is_some_and(|s| s.contains("child frames")), "{error}");
    }
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
