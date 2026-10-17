//! CDP over the real engine (lp-browser + lp-js on JavaScriptCore), driven
//! in-process with the message sequence Playwright 1.59 uses, including
//! HTTP/data navigation, Network/Fetch and per-context storage.

use std::time::{Duration, Instant};

use lp_cdp::engine::EngineHost;
use lp_cdp::outbox::{Outbound, OutboundRx, Outbox};
use lp_cdp::target::Browser;
use serde_json::{json, Value};

struct Harness {
    browser: Browser<EngineHost>,
    out: Outbox,
    rx: OutboundRx,
    next_id: u64,
    session: String,
    frame_id: String,
}

fn decode(bytes: &[u8]) -> Value {
    let header = match bytes[1] {
        126 => 4,
        127 => 10,
        _ => 2,
    };
    serde_json::from_slice(&bytes[header..]).unwrap()
}

impl Harness {
    fn new() -> Harness {
        Self::with_budget(None)
    }

    fn with_budget(budget: Option<lp_cdp::network_protocol::BodyBudget>) -> Harness {
        Self::build(budget, None)
    }

    fn with_handle_limit(limit: usize) -> Harness {
        Self::build(None, Some(limit))
    }

    fn build(budget: Option<lp_cdp::network_protocol::BodyBudget>, handle_limit: Option<usize>) -> Harness {
        let (out, rx) = Outbox::channel(lp_cdp::outbox::DEFAULT_OUTBOUND_BUDGET);
        let mut host = match budget {
            Some(budget) => EngineHost::with_body_budget(out.clone(), budget),
            None => EngineHost::new(out.clone()),
        }
        .expect("JavaScriptCore engine");
        if let Some(limit) = handle_limit {
            host.set_handle_limit(limit);
        }
        let mut h = Harness {
            browser: Browser::new(host),
            out,
            rx,
            next_id: 1,
            session: String::new(),
            frame_id: String::new(),
        };
        h.call(None, "Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}));
        let ctx = h.call(None, "Target.createBrowserContext", json!({"disposeOnDetach":true}));
        let ctx = ctx["result"]["browserContextId"].as_str().unwrap().to_owned();
        let id = h.send(None, "Target.createTarget", json!({"url":"about:blank","browserContextId":ctx}));
        let messages = h.until_response(id);
        let attached = messages.iter().find(|m| m["method"] == "Target.attachedToTarget").expect("attached");
        h.session = attached["params"]["sessionId"].as_str().unwrap().to_owned();
        h.frame_id = attached["params"]["targetInfo"]["targetId"].as_str().unwrap().to_owned();
        h
    }

    fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            message["sessionId"] = json!(s);
        }
        let mut sink = self.out.clone();
        self.browser.handle_text(&message.to_string(), &mut sink);
        self.browser.host_mut().flush();
        id
    }

    /// Drains everything already produced (no waiting).
    fn drain(&mut self) -> Vec<Value> {
        let mut all = Vec::new();
        while let Some(item) = self.rx.try_recv() {
            match item {
                Outbound::Frame(f) => all.push(decode(f.as_ref())),
                Outbound::Exceeded => panic!("outbound budget exceeded"),
            }
        }
        all
    }

    /// Runs the engine until the response to `id` arrives; returns every
    /// message seen up to and including it.
    fn until_response(&mut self, id: u64) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        loop {
            seen.extend(self.drain());
            if seen.iter().any(|m| m["id"] == id) {
                return seen;
            }
            assert!(Instant::now() < deadline, "no response to {id}; saw {seen:#?}");
            let host = self.browser.host_mut();
            host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
            host.flush();
        }
    }

    fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let id = self.send(session, method, params);
        let messages = self.until_response(id);
        messages.into_iter().find(|m| m["id"] == id).unwrap()
    }

    fn page(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.call(Some(&session), method, params)
    }

    fn page_with_events(&mut self, method: &str, params: Value) -> Vec<Value> {
        let session = self.session.clone();
        let id = self.send(Some(&session), method, params);
        self.until_response(id)
    }
}

fn methods(messages: &[Value]) -> Vec<String> {
    messages.iter().map(|m| m["method"].as_str().map_or_else(|| format!("#{}", m["id"]), str::to_owned)).collect()
}

#[test]
fn playwright_page_initialization_sequence() {
    let mut h = Harness::new();
    assert_eq!(h.page("Page.enable", json!({}))["result"], json!({}));
    let tree = h.page("Page.getFrameTree", json!({}));
    assert_eq!(tree["result"]["frameTree"]["frame"]["id"], h.frame_id.as_str(), "target id is the main frame id");
    assert_eq!(tree["result"]["frameTree"]["frame"]["url"], "about:blank");

    let events = h.page_with_events("Runtime.enable", json!({}));
    let created: Vec<&Value> = events.iter().filter(|m| m["method"] == "Runtime.executionContextCreated").collect();
    assert_eq!(created.len(), 1, "{events:#?}");
    assert_eq!(created[0]["params"]["context"]["auxData"]["isDefault"], true);
    assert_eq!(created[0]["params"]["context"]["auxData"]["frameId"], h.frame_id.as_str());

    let script = h.page("Page.addScriptToEvaluateOnNewDocument", json!({"source":"","worldName":"__pw_utility"}));
    assert!(script["result"]["identifier"].is_string());
    let frame = h.frame_id.clone();
    let events = h.page_with_events(
        "Page.createIsolatedWorld",
        json!({"frameId":frame,"worldName":"__pw_utility","grantUniveralAccess":true}),
    );
    let created =
        events.iter().find(|m| m["method"] == "Runtime.executionContextCreated").expect("isolated context event");
    assert_eq!(created["params"]["context"]["name"], "__pw_utility");
    assert_eq!(created["params"]["context"]["auxData"]["type"], "isolated");
    let response = events.last().unwrap();
    assert_eq!(response["result"]["executionContextId"], created["params"]["context"]["id"]);

    for (method, params) in [
        ("Log.enable", json!({})),
        ("Network.enable", json!({})),
        ("Emulation.setFocusEmulationEnabled", json!({"enabled":true})),
        (
            "Emulation.setDeviceMetricsOverride",
            json!({"width":1280,"height":720,"deviceScaleFactor":1,"mobile":false,"screenWidth":1280,"screenHeight":720}),
        ),
        (
            "Emulation.setEmulatedMedia",
            json!({"media":"","features":[{"name":"prefers-color-scheme","value":"light"}]}),
        ),
        ("Browser.getWindowForTarget", json!({})),
        ("Runtime.runIfWaitingForDebugger", json!({})),
    ] {
        let r = h.page(method, params);
        assert!(r.get("error").is_none(), "{method}: {r}");
    }

    let lifecycle = h.page_with_events("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    let names: Vec<&str> = lifecycle
        .iter()
        .filter(|m| m["method"] == "Page.lifecycleEvent")
        .map(|m| m["params"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["DOMContentLoaded", "load", "networkAlmostIdle", "networkIdle"]);
}

#[test]
fn evaluate_call_function_and_handles() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let r = h.page("Runtime.evaluate", json!({"expression":"1 + 2","returnByValue":true}));
    assert_eq!(r["result"]["result"], json!({"type":"number","value":3,"description":"3"}));

    let r = h.page("Runtime.evaluate", json!({"expression":"-0"}));
    assert_eq!(r["result"]["result"]["unserializableValue"], "-0");

    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"new Promise(r => setTimeout(() => r({late: true}), 20))","awaitPromise":true,"returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!({"late": true}), "awaited across a real timer");

    let r = h.page("Runtime.evaluate", json!({"expression":"({a: 1, get b() { return 2 }, c: [1]})"}));
    let object_id = r["result"]["result"]["objectId"].as_str().expect("object handle").to_owned();
    assert_eq!(r["result"]["result"]["type"], "object");

    let r = h.page(
        "Runtime.callFunctionOn",
        json!({
            "functionDeclaration": "function(x, y, z) { return [this.a, x === null, y === undefined, typeof z, z.c.length]; }",
            "objectId": object_id,
            "arguments": [{"value": null}, {}, {"objectId": object_id}],
            "returnByValue": true,
        }),
    );
    assert_eq!(r["result"]["result"]["value"], json!([1, true, true, "object", 1]), "explicit null stays null: {r}");

    let r = h.page("Runtime.getProperties", json!({"objectId": object_id, "ownProperties": true}));
    let props = r["result"]["result"].as_array().unwrap();
    let names: Vec<&str> = props.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"a") && names.contains(&"b") && names.contains(&"c"), "{names:?}");
    let b = props.iter().find(|p| p["name"] == "b").unwrap();
    assert!(b.get("get").is_some() && b.get("value").is_none(), "accessor reported as get: {b}");

    assert_eq!(h.page("Runtime.releaseObject", json!({"objectId": object_id}))["result"], json!({}));
    let r =
        h.page("Runtime.callFunctionOn", json!({"functionDeclaration":"function(){ return 1 }","objectId": object_id}));
    assert_eq!(r["error"]["message"], "Could not find object with given id");
}

#[test]
fn exceptions_and_console_events() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let r =
        h.page("Runtime.evaluate", json!({"expression":"function f() { throw new RangeError('bad range'); }\nf()"}));
    let details = &r["result"]["exceptionDetails"];
    let description = details["exception"]["description"].as_str().unwrap();
    assert!(description.starts_with("RangeError: bad range\n    at f ("), "V8-style description: {description}");
    assert_eq!(details["text"], "Uncaught");
    assert!(details["exception"].get("objectId").is_none(), "exception handles are not retained");

    let r =
        h.page("Runtime.evaluate", json!({"expression":"Promise.reject(new TypeError('nope'))","awaitPromise":true}));
    assert!(r["result"]["exceptionDetails"]["exception"]["description"]
        .as_str()
        .unwrap()
        .starts_with("TypeError: nope"));

    let events = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression":"console.log('hello', 42, {k: 1}); setTimeout(() => { throw new Error('async boom') }, 0); 0"}),
    );
    let console = events.iter().find(|m| m["method"] == "Runtime.consoleAPICalled").expect("console event");
    assert_eq!(console["params"]["type"], "log");
    assert_eq!(console["params"]["args"][0]["value"], "hello");
    assert_eq!(console["params"]["args"][1]["value"], 42);
    assert!(console["params"]["args"][2]["objectId"].is_string());

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut thrown = None;
    while thrown.is_none() && Instant::now() < deadline {
        let host = h.browser.host_mut();
        host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        host.flush();
        thrown = h.drain().into_iter().find(|m| m["method"] == "Runtime.exceptionThrown");
    }
    let thrown = thrown.expect("uncaught timer exception reported");
    assert!(thrown["params"]["exceptionDetails"]["exception"]["description"]
        .as_str()
        .unwrap()
        .starts_with("Error: async boom"));
}

#[test]
fn navigation_events_contexts_and_failures() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Runtime.enable", json!({}));
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    let before = h.page("Runtime.evaluate", json!({"expression":"window"}));
    let old_object = before["result"]["result"]["objectId"].as_str().unwrap().to_owned();
    let old_context: i64 = old_object.split('.').next().unwrap().parse().unwrap();

    let messages = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    let order = methods(&messages);
    let pos =
        |name: &str| order.iter().position(|m| m == name).unwrap_or_else(|| panic!("{name} missing in {order:?}"));
    let response = messages.iter().position(|m| m["result"]["loaderId"].is_string()).expect("navigate response");
    assert!(pos("Page.frameStartedLoading") < response, "{order:?}");
    // Upstream order: the navigate result precedes lifecycle events and the
    // new execution context is announced after frameNavigated.
    let rest = h.drain();
    let all: Vec<String> = methods(&messages).into_iter().chain(methods(&rest)).collect();
    let at = |name: &str| all.iter().position(|m| m == name).unwrap_or_else(|| panic!("{name} missing in {all:?}"));
    assert!(at("Runtime.executionContextsCleared") < at("Page.frameNavigated"), "{all:?}");
    assert!(at("Page.frameNavigated") < at("Runtime.executionContextCreated"), "{all:?}");
    assert!(at("Page.frameNavigated") < at("Page.loadEventFired"), "{all:?}");

    let r = h.page("Runtime.evaluate", json!({"expression":"1","contextId": old_context}));
    assert_eq!(r["error"]["message"], "Cannot find context with specified id");
    let r = h.page("Runtime.callFunctionOn", json!({"functionDeclaration":"function(){}","objectId": old_object}));
    assert_eq!(r["error"]["message"], "Could not find object with given id", "handles die with their realm");

    let r = h.page("Page.navigate", json!({"url":"http://127.0.0.1:1/"}));
    assert!(r["result"]["errorText"].as_str().unwrap().contains("ERR_CONNECTION_REFUSED"), "explicit failure: {r}");
    let r = h.page("Runtime.evaluate", json!({"expression":"40 + 2", "returnByValue": true}));
    assert_eq!(r["result"]["result"]["value"], 42, "page still usable after a failed navigation: {r}");
    assert!(r["result"].get("exceptionDetails").is_none(), "{r}");
}

#[test]
fn unsupported_commands_fail_explicitly() {
    let mut h = Harness::new();
    for method in [
        "DOM.getDocument",
        "Input.dispatchTouchEvent",
        "Fetch.getResponseBody",
        "Page.captureScreenshot",
        "Runtime.addBinding",
    ] {
        let r = h.page(method, json!({}));
        assert_eq!(r["error"]["code"], -32000, "{method}: {r}");
    }
    let r = h.page("Fetch.enable", json!({"patterns":[{"requestStage":"Response"}]}));
    assert!(r["error"]["message"].as_str().unwrap().contains("not supported"), "{r}");
    let r = h.page("Emulation.setLocaleOverride", json!({"locale":"xx-XX"}));
    assert!(r["error"]["message"].as_str().unwrap().contains("not supported"), "{r}");
}

#[test]
fn closing_target_and_context_releases_pages() {
    let mut h = Harness::new();
    let frame = h.frame_id.clone();
    let messages = {
        let id = h.send(None, "Target.closeTarget", json!({"targetId": frame}));
        h.until_response(id)
    };
    assert_eq!(methods(&messages).first().map(String::as_str), Some("Target.detachedFromTarget"));
    let r = h.page("Runtime.evaluate", json!({"expression":"1"}));
    assert_eq!(r["error"]["code"], -32001, "session is gone with its target");
    assert!(h.browser.host().thread.browser.pages.is_empty(), "engine page disposed");
}

// ---- Regressions for review thr_xhdsjj53cj on 3d9d8d3 ----

impl Harness {
    /// Installs `reviewGc()`, which runs a synchronous full GC.
    fn install_gc(&mut self) {
        let realm = self.browser.host().thread.browser.pages[0].main_frame.main_realm().unwrap().clone();
        let scope = realm.scope().unwrap();
        let gc = scope
            .function("reviewGc", |s, _, _| {
                let gc = lp_js::sys::spi::get().synchronous_gc_for_debugging.expect("GC SPI");
                // SAFETY: called on the engine thread with the live context of this callback.
                unsafe { gc(s.ctx()) };
                Ok(s.undefined())
            })
            .unwrap();
        scope.global().set("reviewGc", gc.value()).unwrap();
    }

    fn attach_second_session(&mut self) -> String {
        let frame = self.frame_id.clone();
        let r = self.call(None, "Target.attachToTarget", json!({"targetId": frame, "flatten": true}));
        r["result"]["sessionId"].as_str().unwrap().to_owned()
    }

    fn run_for(&mut self, ms: u64) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let host = self.browser.host_mut();
            host.run_until_work(Some(Instant::now() + Duration::from_millis(10)), || false);
            host.flush();
            seen.extend(self.drain());
        }
        seen
    }
}

#[test]
fn call_arguments_survive_gc_during_compilation() {
    let mut h = Harness::new();
    h.install_gc();
    let args: Vec<Value> =
        (0..64).map(|i| json!({"value": {"token": format!("token-{i}"), "payload": [i, i, i]}})).collect();
    let context = h.page("Runtime.evaluate", json!({"expression":"1"}));
    let _ = context;
    let realm_id = h.browser.host().thread.browser.pages[0].main_frame.main_realm().unwrap().id().0;
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({
            "executionContextId": realm_id,
            "functionDeclaration": "(()=>{ reviewGc(); return function(...args){ return args.map(a => a.token).join(',') } })()",
            "arguments": args,
            "returnByValue": true,
        }),
    );
    let expected = (0..64).map(|i| format!("token-{i}")).collect::<Vec<_>>().join(",");
    assert_eq!(r["result"]["result"]["value"], expected.as_str(), "{r}");
}

#[test]
fn no_reply_reaches_a_detached_session() {
    let mut h = Harness::new();
    let a = h.attach_second_session();
    let b = h.session.clone();
    let id = h.send(
        Some(&a),
        "Runtime.evaluate",
        json!({"expression":"new Promise(r => globalThis.resolveLater = r)","awaitPromise":true}),
    );
    h.drain();
    h.call(None, "Target.detachFromTarget", json!({"sessionId": a}));
    h.call(Some(&b), "Runtime.evaluate", json!({"expression":"resolveLater({x: 1}); 0"}));
    let later = h.run_for(50);
    assert!(later.iter().all(|m| m["id"] != id), "reply sent to detached session: {later:?}");
    assert!(later.iter().all(|m| m["sessionId"] != a.as_str()), "{later:?}");
}

#[test]
fn navigation_fails_pending_awaited_evaluations() {
    let mut h = Harness::new();
    let session = h.session.clone();
    let id =
        h.send(Some(&session), "Runtime.evaluate", json!({"expression":"new Promise(() => {})","awaitPromise":true}));
    let mut seen = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    seen.extend(h.run_for(20));
    let reply = seen.iter().find(|m| m["id"] == id).expect("pending evaluation answered after its context died");
    assert_eq!(reply["error"]["message"], "Execution context was destroyed.");
}

#[test]
fn object_groups_are_inherited() {
    let mut h = Harness::new();
    let r = h.page("Runtime.evaluate", json!({"expression":"({inner: {deep: 1}})","objectGroup":"g"}));
    let outer = r["result"]["result"]["objectId"].as_str().unwrap().to_owned();
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration":"function(){ return this.inner }","objectId": outer}),
    );
    let inner = r["result"]["result"]["objectId"].as_str().unwrap().to_owned();
    let props = h.page("Runtime.getProperties", json!({"objectId": inner, "ownProperties": true}));
    assert!(props["result"]["result"].is_array());
    h.page("Runtime.releaseObjectGroup", json!({"objectGroup":"g"}));
    for id in [&outer, &inner] {
        let r = h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": true}));
        assert_eq!(r["error"]["message"], "Could not find object with given id", "inherited group released: {r}");
    }
}

#[test]
fn run_immediately_creates_the_isolated_world() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let events = h.page_with_events(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"globalThis.reviewImmediate = 123","worldName":"fresh","runImmediately":true}),
    );
    let created = events.iter().find(|m| m["method"] == "Runtime.executionContextCreated").expect("world announced");
    let context = created["params"]["context"]["id"].clone();
    let frame = h.frame_id.clone();
    let r = h.page("Page.createIsolatedWorld", json!({"frameId": frame, "worldName":"fresh"}));
    assert_eq!(r["result"]["executionContextId"], context);
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"typeof reviewImmediate","contextId": context,"returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], "number");
    let r = h.page("Runtime.evaluate", json!({"expression":"typeof reviewImmediate","returnByValue":true}));
    assert_eq!(r["result"]["result"]["value"], "undefined", "main world untouched");
}

#[test]
fn console_without_runtime_is_not_retained_and_floods_close_the_connection() {
    let mut h = Harness::new();
    h.page("Runtime.evaluate", json!({"expression":"for (let i = 0; i < 200000; i++) console.log(i, {i})"}));
    assert!(!h.out.is_exceeded(), "nothing collected without Runtime.enable");
    h.page("Runtime.enable", json!({}));
    let session = h.session.clone();
    h.send(Some(&session), "Runtime.evaluate", json!({"expression":"for (let i = 0; i < 200000; i++) console.log(i)"}));
    assert!(h.out.is_exceeded(), "a flood beyond the event budget closes the connection");
}

#[test]
fn page_disable_suppresses_page_events() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Page.disable", json!({}));
    let messages = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    assert!(messages.iter().all(|m| !m["method"].as_str().unwrap_or("").starts_with("Page.")), "{messages:?}");
}

#[test]
fn window_bounds_round_trip() {
    let mut h = Harness::new();
    let w = h.page("Browser.getWindowForTarget", json!({}));
    let window = w["result"]["windowId"].clone();
    h.page("Browser.setWindowBounds", json!({"windowId": window, "bounds": {"width": 800, "height": 600}}));
    let w = h.page("Browser.getWindowForTarget", json!({}));
    assert_eq!(w["result"]["bounds"]["width"], 800);
    assert_eq!(w["result"]["bounds"]["height"], 600);
    assert_eq!(w["result"]["bounds"]["left"], 0);
}

fn wait_for_network_finish(h: &mut Harness, events: &mut Vec<Value>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|e| e["method"] == "Network.loadingFinished") {
        assert!(Instant::now() < deadline, "missing network finish: {events:#?}");
        h.browser.host_mut().run_until_work(Some(deadline), || false);
        h.browser.host_mut().flush();
        events.extend(h.drain());
    }
}

#[test]
fn network_response_body_and_disable_release_capture() {
    use lp_cdp::network_protocol::{BodyBudget, BodyLimits};
    let budget = BodyBudget::new(BodyLimits { total: 1024, per_body: 1024 });
    let mut h = Harness::with_budget(Some(budget.clone()));
    assert_eq!(h.page("Network.enable", json!({}))["result"], json!({}));
    let mut events = h.page_with_events("Page.navigate", json!({"url":"data:text/html,hello"}));
    wait_for_network_finish(&mut h, &mut events);
    let request = events.iter().find(|e| e["method"] == "Network.requestWillBeSent").unwrap();
    let id = request["params"]["requestId"].clone();
    assert_eq!(request["params"]["frameId"], h.frame_id);
    assert_eq!(request["params"]["requestId"], request["params"]["loaderId"]);
    let response = events.iter().find(|e| e["method"] == "Network.responseReceived").unwrap();
    assert_eq!(response["params"]["requestId"], id);
    assert_eq!(response["params"]["response"]["status"], 200);
    assert_eq!(
        h.page("Network.getResponseBody", json!({"requestId":id}))["result"],
        json!({"body":"hello","base64Encoded":false})
    );
    assert_eq!(budget.used(), 5);
    assert_eq!(h.page("Network.disable", json!({}))["result"], json!({}));
    assert_eq!(budget.used(), 0);
    assert!(h.page("Network.getResponseBody", json!({"requestId":id}))["error"].is_object());
    let events = h.page_with_events("Page.navigate", json!({"url":"data:text/html,uncaptured"}));
    assert!(!events.iter().any(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Network."))));
    assert_eq!(budget.used(), 0);
}

#[test]
fn network_events_only_reach_enabled_sessions_and_close_releases_body() {
    use lp_cdp::network_protocol::{BodyBudget, BodyLimits};
    let budget = BodyBudget::new(BodyLimits { total: 1024, per_body: 1024 });
    let mut h = Harness::with_budget(Some(budget.clone()));
    let target = h.frame_id.clone();
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true}));
    let second = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    h.page("Network.enable", json!({}));
    let mut events = h.page_with_events("Page.navigate", json!({"url":"data:text/html,body"}));
    wait_for_network_finish(&mut h, &mut events);
    for event in events.iter().filter(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Network."))) {
        assert_eq!(event["sessionId"], h.session);
        assert_ne!(event["sessionId"], second);
    }
    assert_eq!(budget.used(), 4);
    h.call(None, "Target.closeTarget", json!({"targetId":target}));
    assert_eq!(budget.used(), 0);
}

#[test]
fn cdp_contexts_map_to_distinct_browser_network_contexts() {
    use lp_cdp::target::Host;
    let (out, _) = Outbox::channel(1024 * 1024);
    let mut host = EngineHost::new(out).unwrap();
    let a = host.create_context().unwrap();
    let b = host.create_context().unwrap();
    let (pa, _) = host.create_page(a).unwrap();
    let (pb, _) = host.create_page(b).unwrap();
    let ca = host.thread.browser.page(lp_browser::PageId(pa as u32)).unwrap().context;
    let cb = host.thread.browser.page(lp_browser::PageId(pb as u32)).unwrap().context;
    assert_ne!(ca, cb);
    assert!(host.thread.browser.context_net(ca).is_some());
    assert!(host.thread.browser.context_net(cb).is_some());
    host.dispose_context(a);
    assert!(host.thread.browser.context_net(ca).is_none());
    assert!(host.thread.browser.context_net(cb).is_some());
    host.close_all();
    assert!(host.thread.browser.context_net(cb).is_none());
}

#[test]
fn network_http_redirect_response_and_body() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "request did not arrive");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0 && request.len() + n <= 8192);
                request.extend_from_slice(&chunk[..n]);
            }
            let response: &[u8] = if index == 0 {
                b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 201 Created\r\nContent-Type: text/html\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
            };
            stream.write_all(response).unwrap();
        }
    });
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    let mut events = h.page_with_events("Page.navigate", json!({"url":format!("http://{address}/redirect")}));
    wait_for_network_finish(&mut h, &mut events);
    let requests: Vec<_> = events.iter().filter(|e| e["method"] == "Network.requestWillBeSent").collect();
    assert_eq!(requests.len(), 2, "{events:#?}");
    assert_eq!(requests[0]["params"]["requestId"], requests[1]["params"]["requestId"]);
    assert_eq!(requests[1]["params"]["redirectResponse"]["status"], 302);
    let response = events.iter().find(|e| e["method"] == "Network.responseReceived").unwrap();
    assert_eq!(response["params"]["response"]["status"], 201);
    assert_eq!(response["params"]["response"]["url"], format!("http://{address}/final"));
    let id = response["params"]["requestId"].clone();
    assert_eq!(
        h.page("Network.getResponseBody", json!({"requestId":id}))["result"],
        json!({"body":"hello","base64Encoded":false})
    );
    server.join().unwrap();
}

fn until_event(h: &mut Harness, method: &str) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    loop {
        events.extend(h.drain());
        if events.iter().any(|e| e["method"] == method) {
            return events;
        }
        assert!(Instant::now() < deadline, "missing {method}: {events:#?}");
        h.browser.host_mut().run_until_work(Some(deadline), || false);
        h.browser.host_mut().flush();
    }
}

#[test]
fn invalid_fetch_decision_keeps_request_paused_for_valid_retry() {
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    h.page("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}));
    let session = h.session.clone();
    h.send(Some(&session), "Page.navigate", json!({"url":"http://127.0.0.1:1/fulfilled"}));
    let events = until_event(&mut h, "Fetch.requestPaused");
    let paused = events.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap();
    let id = paused["params"]["requestId"].clone();
    let request = events.iter().find(|e| e["method"] == "Network.requestWillBeSent").unwrap();
    assert_eq!(paused["params"]["networkId"], request["params"]["requestId"]);
    assert!(h.page("Fetch.fulfillRequest", json!({"requestId":id,"responseCode":200,"body":"=="}))["error"].is_object());
    assert!(h.page("Fetch.fulfillRequest", json!({"requestId":id,"responseCode":200,"binaryResponseHeaders":"=="}))
        ["error"]
        .is_object());
    assert!(h.page(
        "Fetch.fulfillRequest",
        json!({"requestId":id,"responseCode":200,"responseHeaders":[],"binaryResponseHeaders":""})
    )["error"]
        .is_object());
    let mut events = h.page_with_events(
        "Fetch.fulfillRequest",
        json!({"requestId":id,"responseCode":200,"binaryResponseHeaders":"Y29udGVudC10eXBlOiB0ZXh0L2h0bWwAeC1yb3V0ZTogYmluYXJ5AHgtcm91dGU6IGFnYWluAA==","body":"aGk="}),
    );
    assert!(events.iter().any(|e| e["result"] == json!({})), "fulfill accepted: {events:#?}");
    wait_for_network_finish(&mut h, &mut events);
    let response = events.iter().find(|e| e["method"] == "Network.responseReceived").unwrap();
    assert_eq!(response["params"]["response"]["headers"]["x-route"], "binary\nagain");
    let request_id = paused["params"]["networkId"].clone();
    assert_eq!(h.page("Network.getResponseBody", json!({"requestId":request_id}))["result"]["body"], "hi");
    assert!(h.page("Fetch.continueRequest", json!({"requestId":id}))["error"].is_object(), "decision cannot be reused");
}

#[test]
fn disabling_fetch_resumes_a_paused_navigation() {
    let mut h = Harness::new();
    h.page("Fetch.enable", json!({}));
    let session = h.session.clone();
    let navigation = h.send(Some(&session), "Page.navigate", json!({"url":"http://127.0.0.1:1/continued"}));
    let paused = until_event(&mut h, "Fetch.requestPaused");
    let id = paused.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["params"]["requestId"].clone();
    let mut events = h.page_with_events("Fetch.disable", json!({}));
    if !events.iter().any(|e| e["id"] == navigation) {
        events.extend(h.until_response(navigation));
    }
    let response = events.iter().find(|e| e["id"] == navigation).unwrap();
    assert!(
        response["result"]["errorText"].as_str().unwrap().contains("ERR_CONNECTION_REFUSED"),
        "request resumed: {response}"
    );
    assert!(h.page("Fetch.continueRequest", json!({"requestId":id}))["error"].is_object());
}

#[test]
fn detaching_fetch_session_resumes_navigation_for_other_session() {
    let mut h = Harness::new();
    let target = h.frame_id.clone();
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true}));
    let intercepting = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    h.call(Some(&intercepting), "Fetch.enable", json!({}));
    let session = h.session.clone();
    let navigation = h.send(Some(&session), "Page.navigate", json!({"url":"http://127.0.0.1:1/detached"}));
    let events = until_event(&mut h, "Fetch.requestPaused");
    assert_eq!(events.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["sessionId"], intercepting);
    let detach = h.send(None, "Target.detachFromTarget", json!({"sessionId":intercepting}));
    let mut events = h.until_response(detach);
    if !events.iter().any(|e| e["id"] == navigation) {
        events.extend(h.until_response(navigation));
    }
    let response = events.iter().find(|e| e["id"] == navigation).unwrap();
    assert!(
        response["result"]["errorText"].as_str().unwrap().contains("ERR_CONNECTION_REFUSED"),
        "request resumed: {response}"
    );
}

#[test]
fn storage_cookies_are_isolated_and_default_context_is_distinct() {
    let mut h = Harness::new();
    let a = h.call(None, "Target.createBrowserContext", json!({}))["result"]["browserContextId"].clone();
    let b = h.call(None, "Target.createBrowserContext", json!({}))["result"]["browserContextId"].clone();
    assert_eq!(
        h.call(
            None,
            "Storage.setCookies",
            json!({"browserContextId":a,"cookies":[
                {"name":"a","value":"1","url":"https://example.com/path","expires":-1,"httpOnly":true,"sameSite":"Lax"}
            ]})
        )["result"],
        json!({})
    );
    let cookies = h.call(None, "Storage.getCookies", json!({"browserContextId":a}))["result"]["cookies"].clone();
    assert_eq!(cookies[0]["name"], "a");
    assert_eq!(cookies[0]["domain"], "example.com");
    assert_eq!(cookies[0]["secure"], true);
    assert_eq!(cookies[0]["httpOnly"], true);
    assert_eq!(cookies[0]["session"], true);
    assert_eq!(cookies[0]["expires"], -1.0);
    assert_eq!(h.call(None, "Storage.getCookies", json!({"browserContextId":b}))["result"]["cookies"], json!([]));
    assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"], json!([]));
    h.call(None, "Storage.setCookies", json!({"cookies":[{"name":"default","value":"x","domain":".example.com"}]}));
    assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"][0]["domain"], ".example.com");
    h.call(None, "Storage.clearCookies", json!({"browserContextId":a}));
    assert_eq!(h.call(None, "Storage.getCookies", json!({"browserContextId":a}))["result"]["cookies"], json!([]));
    assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"][0]["name"], "default");
    h.call(None, "Target.disposeBrowserContext", json!({"browserContextId":a}));
    assert!(h.call(None, "Storage.getCookies", json!({"browserContextId":a}))["error"].is_object());
}

#[test]
fn storage_invalid_batch_is_atomic_and_partitioned_cookies_fail_explicitly() {
    let mut h = Harness::new();
    for bad in [
        json!({"name":"bad;name","value":"x","domain":"example.com"}),
        json!({"name":"x","value":"x","domain":"example.com","path":"relative"}),
        json!({"name":"x","value":"x","domain":"example.com","partitionKey":{"topLevelSite":"https://example.com","hasCrossSiteAncestor":false}}),
        json!({"name":"x","value":"x","domain":"example.com","sameSite":"invalid"}),
    ] {
        let result = h.call(
            None,
            "Storage.setCookies",
            json!({"cookies":[
                {"name":"valid","value":"x","domain":"example.com"},bad
            ]}),
        );
        assert!(result["error"].is_object(), "{result}");
        assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"], json!([]));
    }
    assert!(h.call(None, "Storage.getCookies", json!({"browserContextId":4}))["error"].is_object());
}

#[test]
fn storage_negative_expiry_is_session_and_zero_expires_deletes() {
    let mut h = Harness::new();
    h.call(
        None,
        "Storage.setCookies",
        json!({"cookies":[{"name":"session","value":"x","domain":"example.com","expires":-10}]}),
    );
    assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"][0]["session"], true);
    h.call(
        None,
        "Storage.setCookies",
        json!({"cookies":[{"name":"session","value":"x","domain":"example.com","expires":0}]}),
    );
    assert_eq!(h.call(None, "Storage.getCookies", json!({}))["result"]["cookies"], json!([]));
}

#[test]
fn fetch_redirect_issues_a_new_pause_id_and_links_the_previous_hop() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "intercepted request never continued");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut request = Vec::new();
        let mut bytes = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut bytes).unwrap();
            assert!(n > 0 && request.len() + n < 8192);
            request.extend_from_slice(&bytes[..n]);
        }
        stream
            .write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    h.page("Fetch.enable", json!({}));
    let session = h.session.clone();
    h.send(Some(&session), "Page.navigate", json!({"url":format!("http://{address}/redirect")}));
    let events = until_event(&mut h, "Fetch.requestPaused");
    let first = events.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["params"].clone();
    h.page("Fetch.continueRequest", json!({"requestId":first["requestId"]}));
    let events = until_event(&mut h, "Fetch.requestPaused");
    let second = events.iter().find(|e| e["method"] == "Fetch.requestPaused").unwrap()["params"].clone();
    server.join().unwrap();
    assert_ne!(first["requestId"], second["requestId"], "each redirect hop needs its own Fetch id");
    assert_eq!(second["redirectedRequestId"], first["requestId"]);
    assert_eq!(second["networkId"], first["networkId"], "Network id spans the redirect chain");
    assert!(h.page("Fetch.failRequest", json!({"requestId":first["requestId"],"errorReason":"Aborted"}))["error"]
        .is_object());
    let mut events = h.page_with_events(
        "Fetch.fulfillRequest",
        json!({"requestId":second["requestId"],"responseCode":200,"body":"ZG9uZQ=="}),
    );
    wait_for_network_finish(&mut h, &mut events);
    assert_eq!(h.page("Network.getResponseBody", json!({"requestId":second["networkId"]}))["result"]["body"], "done");
}

#[test]
fn input_keyboard_uses_native_focus_default_actions_and_cancellation() {
    let mut h = Harness::new();
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":r#"
        document.body.innerHTML = '<input id="field"><button id="next">Next</button>';
        globalThis.field = document.querySelector('#field');
        globalThis.events = [];
        for (const type of ['keydown','keypress','keyup','beforeinput','input'])
          field.addEventListener(type, e => events.push([e.type, e.isTrusted]));
        field.focus();
    "#}),
    );
    assert!(r.get("error").is_none(), "{r}");
    for (method, params) in [
        (
            "Input.dispatchKeyEvent",
            json!({"type":"keyDown","key":"a","code":"KeyA","text":"a","windowsVirtualKeyCode":65}),
        ),
        ("Input.dispatchKeyEvent", json!({"type":"keyUp","key":"a","code":"KeyA","windowsVirtualKeyCode":65})),
        ("Input.insertText", json!({"text":"β"})),
    ] {
        assert_eq!(h.page(method, params)["result"], json!({}));
    }
    let r = h.page("Runtime.evaluate", json!({"expression":"[field.value, events]","returnByValue":true}));
    assert_eq!(
        r["result"]["result"]["value"],
        json!([
            "aβ",
            [
                ["keydown", true],
                ["keypress", true],
                ["beforeinput", true],
                ["input", true],
                ["keyup", true],
                ["beforeinput", true],
                ["input", true]
            ]
        ])
    );
    h.page("Runtime.evaluate", json!({"expression":"field.addEventListener('beforeinput', e => e.preventDefault(), {once:true}); field.setSelectionRange(0,2)"}));
    assert_eq!(h.page("Input.insertText", json!({"text":"blocked"}))["result"], json!({}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"field.value","returnByValue":true}))["result"]["result"]
            ["value"],
        "aβ"
    );
    assert_eq!(h.page("Input.insertText", json!({"text":"replacement"}))["result"], json!({}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"field.value","returnByValue":true}))["result"]["result"]
            ["value"],
        "replacement"
    );
    h.page("Input.dispatchKeyEvent", json!({"type":"rawKeyDown","key":"Tab","code":"Tab"}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"document.activeElement.id","returnByValue":true}))["result"]
            ["result"]["value"],
        "next"
    );
}

#[test]
fn input_validation_rejects_before_delivering_events() {
    let mut h = Harness::new();
    h.page(
        "Runtime.evaluate",
        json!({"expression":"globalThis.keyEvents=0; document.addEventListener('keydown',()=>keyEvents++)"}),
    );
    for params in [
        json!({"type":"invalid"}),
        json!({"type":"keyDown","modifiers":16}),
        json!({"type":"keyDown","location":4}),
        json!({"type":"keyDown","commands":["undo"]}),
        json!({"type":"keyDown","timestamp":1}),
        json!({"type":"keyDown","windowsVirtualKeyCode":256}),
    ] {
        assert!(h.page("Input.dispatchKeyEvent", params).get("error").is_some());
    }
    for params in [
        json!({"type":"mousePressed","x":0,"y":0,"button":"invalid"}),
        json!({"type":"mousePressed","x":0,"y":0,"buttons":32}),
        json!({"type":"mousePressed","x":0,"y":0,"pointerType":"pen"}),
        json!({"type":"mousePressed","x":0,"y":0,"force":0.8}),
    ] {
        assert!(h.page("Input.dispatchMouseEvent", params).get("error").is_some());
    }
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"keyEvents","returnByValue":true}))["result"]["result"]["value"],
        0
    );
}

#[test]
fn input_mouse_hit_tests_and_runs_trusted_checkbox_activation() {
    let mut h = Harness::new();
    h.page("Runtime.evaluate", json!({"expression":r#"
      document.body.innerHTML='<input id="check" type="checkbox" style="position:absolute;left:20px;top:20px;width:40px;height:40px">';
      globalThis.clicks=[];
      document.querySelector('#check').addEventListener('click', e => clicks.push([e.isTrusted,e.clientX,e.clientY]));
    "#}));
    for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":kind,"x":30,"y":30,"button":"left","buttons":buttons,"clickCount":1}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"[document.querySelector('#check').checked,clicks]","returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!([true, [[true, 30, 30]]]));
}

#[test]
fn dom_backend_nodes_adopt_across_worlds_and_expire_on_navigation() {
    let mut h = Harness::new();
    assert_eq!(h.page("DOM.enable", json!({}))["result"], json!({}));
    let object = h.page("Runtime.evaluate", json!({"expression":"document.body.innerHTML='<button id=action>Press</button>'; document.querySelector('#action')"}))["result"]["result"]["objectId"].clone();
    let described = h.page("DOM.describeNode", json!({"objectId":object}));
    let node = &described["result"]["node"];
    assert_eq!(node["nodeName"], "BUTTON");
    assert_eq!(node["localName"], "button");
    assert_eq!(node["attributes"], json!(["id", "action"]));
    assert_eq!(node["children"][0]["nodeValue"], "Press");
    let backend = node["backendNodeId"].clone();
    assert!(backend.as_u64().unwrap() > 0);
    let frame = h.frame_id.clone();
    let world = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"dom-adopt"}))["result"]
        ["executionContextId"]
        .clone();
    let resolved =
        h.page("DOM.resolveNode", json!({"backendNodeId":backend,"executionContextId":world,"objectGroup":"nodes"}));
    assert_eq!(resolved["result"]["object"]["subtype"], "node");
    let adopted = resolved["result"]["object"]["objectId"].clone();
    let r = h.page("Runtime.callFunctionOn", json!({"objectId":adopted,"functionDeclaration":"function(){ return [this.id, this===document.querySelector('#action')] }","returnByValue":true}));
    assert_eq!(r["result"]["result"]["value"], json!(["action", true]));
    assert_eq!(
        h.page("DOM.describeNode", json!({"objectId":adopted,"depth":0}))["result"]["node"]["backendNodeId"],
        backend
    );
    h.page("Runtime.releaseObjectGroup", json!({"objectGroup":"nodes"}));
    assert!(h.page("DOM.describeNode", json!({"objectId":adopted})).get("error").is_some());
    let events = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    assert!(events.iter().any(|e| e["method"] == "DOM.documentUpdated"));
    assert!(h.page("DOM.resolveNode", json!({"backendNodeId":backend})).get("error").is_some());
    // This session never enabled Page: navigation must still release its roots.
    assert_eq!(
        h.page("Runtime.getProperties", json!({"objectId":object}))["error"]["message"],
        "Could not find object with given id"
    );
}

#[test]
fn dom_geometry_matches_native_rect_and_rejects_detached_or_foreign_objects() {
    let mut h = Harness::new();
    let object = h.page("Runtime.evaluate",json!({"expression":"document.body.innerHTML='<input id=box style=\"width:40px;height:30px\">';document.querySelector('#box')"}))["result"]["result"]["objectId"].clone();
    let r = h.page("Runtime.evaluate",json!({"expression":"JSON.parse(JSON.stringify(document.querySelector('#box').getBoundingClientRect()))","returnByValue":true}));
    let rect = &r["result"]["result"]["value"];
    let x = rect["x"].as_f64().unwrap();
    let y = rect["y"].as_f64().unwrap();
    let quad = json!([x, y, x + 40.0, y, x + 40.0, y + 30.0, x, y + 30.0]);
    assert_eq!(h.page("DOM.getContentQuads", json!({"objectId":object}))["result"]["quads"], json!([quad]));
    assert_eq!(h.page("DOM.getBoxModel", json!({"objectId":object}))["result"]["model"]["border"], quad);
    assert_eq!(h.page("DOM.focus", json!({"objectId":object}))["result"], json!({}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"document.activeElement.id","returnByValue":true}))["result"]
            ["result"]["value"],
        "box"
    );
    // A node connected to an inert document is not rendered in this frame.
    {
        let frame = &h.browser.host().thread.browser.pages[0].main_frame;
        let page = frame.page_dom.as_ref().unwrap().clone();
        let realm = frame.main_realm().unwrap().clone();
        let node = {
            let mut dom = page.dom.borrow_mut();
            let doc = dom.create_document(lp_dom::DocumentKind::Html, "about:blank");
            let node = dom.create_element(doc, "input").unwrap();
            dom.append_child(doc, node).unwrap();
            node
        };
        let scope = realm.scope().unwrap();
        let wrapper = lp_webapi::wrap_node(&scope, &page, node).unwrap();
        scope.global().set("inertNode", wrapper.value()).unwrap();
    }
    let inert = h.page("Runtime.evaluate", json!({"expression":"inertNode"}))["result"]["result"]["objectId"].clone();
    assert_eq!(
        h.page("DOM.getBoxModel", json!({"objectId":inert}))["error"]["message"],
        "Node is detached from document"
    );
    let spoof = h.page("Runtime.evaluate", json!({"expression":"({nodeType:1,nodeName:'DIV'})"}))["result"]["result"]
        ["objectId"]
        .clone();
    assert_eq!(h.page("DOM.describeNode", json!({"objectId":spoof}))["error"]["message"], "Object is not a DOM node");
    h.page("Runtime.evaluate", json!({"expression":"document.querySelector('#box').remove()"}));
    assert_eq!(
        h.page("DOM.scrollIntoViewIfNeeded", json!({"objectId":object}))["error"]["message"],
        "Node is detached from document"
    );
    assert!(
        h.page("DOM.describeNode", json!({"objectId":object})).get("result").is_some(),
        "a rooted detached node remains describable"
    );
    assert!(h.page("DOM.describeNode", json!({"objectId":object,"backendNodeId":1})).get("error").is_some());
    assert!(h.page("DOM.describeNode", json!({"objectId":object,"depth":33})).get("error").is_some());
}

#[test]
fn user_agent_override_validates_atomically_and_updates_page_worlds() {
    let mut h = Harness::new();
    let probe = "[navigator.userAgent,navigator.platform,navigator.language,Array.from(navigator.languages)]";
    let initial = h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]
        ["value"]
        .clone();
    let locale = h.page("Emulation.setUserAgentOverride", json!({"userAgent":"","acceptLanguage":"en-US"}));
    assert_eq!(locale["result"], json!({}), "exact TOS locale-only request: {locale}");
    let language = h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}));
    assert_eq!(language["result"]["result"]["value"][0], initial[0]);
    assert_eq!(language["result"]["result"]["value"][2], "en-US");
    let frame = h.frame_id.clone();
    let world = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"identity"}))["result"]
        ["executionContextId"]
        .clone();
    assert_eq!(
        h.page(
            "Emulation.setUserAgentOverride",
            json!({"userAgent":"Protocol/1","acceptLanguage":"fr-FR,fr;q=0.8","platform":"FixtureOS"})
        )["result"],
        json!({})
    );
    let expected = json!(["Protocol/1", "FixtureOS", "fr-FR", ["fr-FR", "fr"]]);
    for params in [
        json!({"expression":probe,"returnByValue":true}),
        json!({"expression":probe,"returnByValue":true,"contextId":world}),
    ] {
        assert_eq!(h.page("Runtime.evaluate", params)["result"]["result"]["value"], expected);
    }
    for params in [
        json!({}),
        json!({"userAgent":123}),
        json!({"userAgent":"ok","acceptLanguage":false}),
        json!({"userAgent":"ok","platform":[]}),
        json!({"userAgent":"bad\r\nheader"}),
        json!({"userAgent":"ok","acceptLanguage":"bad\nheader"}),
    ] {
        let response = h.page("Emulation.setUserAgentOverride", params.clone());
        assert_eq!(response["error"]["code"], -32602, "{params}: {response}");
    }
    let metadata = h.page("Emulation.setUserAgentOverride", json!({"userAgent":"ignored","userAgentMetadata":{}}));
    assert_eq!(metadata["error"]["code"], -32000, "unsupported client hints: {metadata}");
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        expected,
        "invalid requests did not mutate existing state"
    );
    assert!(h.page("Page.navigate", json!({"url":"data:text/html,<p>identity</p>"}))["error"].is_null());
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        expected
    );
    assert_eq!(h.page("Emulation.setUserAgentOverride", json!({"userAgent":""}))["result"], json!({}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        initial
    );
}

#[test]
fn device_metrics_change_native_dimensions_persist_and_clear() {
    let mut h = Harness::new();
    let probe =
        "[innerWidth,innerHeight,devicePixelRatio,screen.width,screen.height,matchMedia('(width: 640px)').matches]";
    let baseline = h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]
        ["value"]
        .clone();
    let metrics = json!({"width":640,"height":480,"deviceScaleFactor":2,"mobile":false,"screenWidth":1024,"screenHeight":768,"screenOrientation":{"angle":0,"type":"landscapePrimary"},"dontSetVisibleSize":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", metrics)["result"], json!({}));
    let expected = json!([640, 480, 2, 1024, 768, true]);
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        expected
    );
    let frame = h.frame_id.clone();
    let world = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"metrics"}))["result"]
        ["executionContextId"]
        .clone();
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true,"contextId":world}))["result"]
            ["result"]["value"],
        expected
    );
    h.page(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":"globalThis.initDimensions=[innerWidth,innerHeight,devicePixelRatio]"}),
    );
    h.page("Page.navigate", json!({"url":"data:text/html,<p>metrics</p>"}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"initDimensions","returnByValue":true}))["result"]["result"]
            ["value"],
        json!([640, 480, 2])
    );
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        expected
    );
    assert_eq!(h.page("Emulation.clearDeviceMetricsOverride", json!({}))["result"], json!({}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        baseline
    );
    assert_eq!(
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":0,"height":0,"deviceScaleFactor":0,"mobile":false})
        )["result"],
        json!({})
    );
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"],
        baseline
    );
}

#[test]
fn device_metrics_reject_invalid_and_unsupported_fields_atomically() {
    let mut h = Harness::new();
    let valid = json!({"width":800,"height":600,"deviceScaleFactor":1.5,"mobile":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", valid.clone())["result"], json!({}));
    for (key, value) in [
        ("width", json!(-1)),
        ("height", json!(10000001)),
        ("width", json!(800.5)),
        ("height", json!("600")),
        ("deviceScaleFactor", json!(-1)),
        ("mobile", json!("no")),
        ("screenWidth", json!(10000001)),
        ("screenHeight", json!(-1)),
        ("screenOrientation", json!({"type":"sideways","angle":0})),
    ] {
        let mut params = valid.clone();
        params[key] = value;
        let result = h.page("Emulation.setDeviceMetricsOverride", params.clone());
        assert_eq!(result["error"]["code"], -32602, "{params}: {result}");
    }
    for (key, value) in [
        ("mobile", json!(true)),
        ("scale", json!(0.5)),
        ("positionX", json!(10)),
        ("viewport", json!({"x":0,"y":0,"width":100,"height":100,"scale":1})),
        ("screenOrientation", json!({"type":"portraitPrimary","angle":90})),
    ] {
        let mut params = valid.clone();
        params[key] = value;
        let result = h.page("Emulation.setDeviceMetricsOverride", params.clone());
        assert_eq!(result["error"]["code"], -32000, "{params}: {result}");
    }
    assert_eq!(
        h.page(
            "Runtime.evaluate",
            json!({"expression":"[innerWidth,innerHeight,devicePixelRatio]","returnByValue":true})
        )["result"]["result"]["value"],
        json!([800, 600, 1.5])
    );
}

#[test]
fn handle_budget_spans_sessions_and_returns_on_release_and_detach() {
    let mut h = Harness::with_handle_limit(4);
    let second = h.attach_second_session();
    let first = h.session.clone();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let r = h.call(Some(&first), "Runtime.evaluate", json!({"expression":"({})"}));
        ids.push(r["result"]["result"]["objectId"].as_str().unwrap().to_owned());
        h.call(Some(&second), "Runtime.evaluate", json!({"expression":"({})"}));
    }
    let r = h.call(Some(&second), "Runtime.evaluate", json!({"expression":"({})"}));
    assert_eq!(r["error"]["message"], "Too many remote objects; release unused handles", "aggregate bounded: {r}");
    h.call(Some(&first), "Runtime.releaseObject", json!({"objectId": ids[0]}));
    let r = h.call(Some(&second), "Runtime.evaluate", json!({"expression":"({})"}));
    assert!(r["result"]["result"]["objectId"].is_string(), "release returns capacity: {r}");
    h.call(None, "Target.detachFromTarget", json!({"sessionId": second}));
    for _ in 0..3 {
        let r = h.call(Some(&first), "Runtime.evaluate", json!({"expression":"({})"}));
        assert!(r["result"]["result"]["objectId"].is_string(), "detach returns the session's handles: {r}");
    }
}
