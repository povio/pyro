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
            host.set_handle_limit(limit).expect("limit set before any session");
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
        self.browser.adopt_popups(&mut sink);
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
            let mut sink = self.out.clone();
            self.browser.adopt_popups(&mut sink);
        }
    }

    /// Runs the engine until a message matching `pred` arrives.
    fn until_event(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        loop {
            seen.extend(self.drain());
            if let Some(found) = seen.iter().find(|m| pred(m)) {
                return found.clone();
            }
            assert!(Instant::now() < deadline, "event not seen; saw {seen:#?}");
            let host = self.browser.host_mut();
            host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
            host.flush();
            let mut sink = self.out.clone();
            self.browser.adopt_popups(&mut sink);
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

    let names = |messages: &[Value]| -> Vec<String> {
        messages
            .iter()
            .filter(|m| m["method"] == "Page.lifecycleEvent")
            .map(|m| m["params"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    // The committed about:blank document is replayed up to `load`; the network
    // quiet milestones (500 ms after load with nothing in flight) are not
    // reached yet, so they arrive live, once each, rather than replayed early.
    let lifecycle = h.page_with_events("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    assert_eq!(names(&lifecycle), ["DOMContentLoaded", "load"]);
    assert_eq!(names(&h.run_for(800)), ["networkAlmostIdle", "networkIdle"]);
    assert!(names(&h.run_for(300)).is_empty(), "each milestone is reported once");
    // Once reached, enabling again replays all four.
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled":false}));
    let lifecycle = h.page_with_events("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    assert_eq!(names(&lifecycle), ["DOMContentLoaded", "load", "networkAlmostIdle", "networkIdle"]);
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
fn input_mouse_wheel_scrolls_container_then_viewport_unless_canceled() {
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<div id="box" style="height:100px;overflow:auto"><div id="inner" style="height:500px"></div></div><div style="height:10000px"></div>';
      globalThis.box=document.getElementById('box'); globalThis.inner=document.getElementById('inner');
      globalThis.wheels=[]; globalThis.scrolls=[];
      box.addEventListener('wheel', e => wheels.push([e.isTrusted, e.cancelable, e.deltaX, e.deltaY, e.target.id]));
      box.addEventListener('scroll', () => scrolls.push('box'));
      document.addEventListener('scroll', () => scrolls.push('document'));
      // Default-passive: this cannot stop scrolling (as in Chrome).
      document.addEventListener('wheel', e => e.preventDefault());
      const r = inner.getBoundingClientRect(); globalThis.point = [r.x + r.width / 2, r.y + 5]; 1
    "#,
    );
    let point = eval_value(&mut h, "point");
    let (x, y) = (point[0].as_f64().unwrap(), point[1].as_f64().unwrap());
    // Playwright 1.59 crInput.wheel sends exactly these fields.
    let wheel = |h: &mut Harness, dy: f64| {
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseWheel","x":x,"y":y,"modifiers":0,"deltaX":0,"deltaY":dy}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
    };
    wheel(&mut h, 300.0);
    // Scroll events arrive at the next frame (the harness pumps coarsely).
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "[box.scrollTop, scrollY, scrolls.join()]"), json!([300, 0, "box"]));
    wheel(&mut h, 300.0);
    wheel(&mut h, 50.0);
    h.run_for(200);
    // The box stops at its extent; the next delta goes to the viewport.
    assert_eq!(eval_value(&mut h, "[box.scrollTop, scrollY, scrolls.join()]"), json!([400, 50, "box,box,document"]));
    assert_eq!(
        eval_value(&mut h, "wheels"),
        json!([[true, true, 0, 300, "inner"], [true, true, 0, 300, "inner"], [true, true, 0, 50, "inner"]])
    );
    // A non-passive listener that cancels the wheel prevents scrolling.
    eval_value(&mut h, "box.addEventListener('wheel', e => e.preventDefault(), {passive:false}); 1");
    wheel(&mut h, -100.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "[box.scrollTop, scrollY, wheels.length]"), json!([400, 50, 4]));
}

#[test]
fn input_mouse_wheel_over_empty_viewport_space_scrolls_the_viewport() {
    let mut h = Harness::new();
    eval_value(&mut h, "document.body.innerHTML='<div style=\"height:10000px\"></div>'; 1");
    // Playwright's initial mouse position; the synthetic layout has no
    // element there, where Chrome would hit the root element.
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(0, 0) === null"), json!(true));
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mouseWheel","x":0,"y":0,"deltaX":0,"deltaY":70}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "scrollY"), json!(70));
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mouseWheel","x":-1,"y":0,"deltaX":0,"deltaY":70}));
    assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("no element at point")), "{r}");
    eval_value(&mut h, "scrollTo(0, 0); 1");
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":0,"y":0,"button":"left","buttons":1}));
    assert!(
        r["error"]["message"].as_str().is_some_and(|m| m.contains("no element at point")),
        "clicks keep the error: {r}"
    );
}

/// Pages for the drag tests: centers of #src, #dst and #plain, and a capture
/// log of drag/pointer events in Chromium's probe format.
fn drag_page(h: &mut Harness) -> (Value, Value, Value) {
    eval_value(
        h,
        r#"
      document.body.innerHTML='<div id="src" draggable="true" style="width:100px;height:40px">src</div><a id="lnk" href="/doc?id=7" style="display:block;width:80px;height:20px">link <b>x</b></a><div id="dst" style="width:200px;height:80px">dst</div><div id="plain" style="width:100px;height:40px">plain</div>';
      globalThis.log=[];
      const short = e => e.type + '@' + (e.target.id||e.target.nodeName) + (e.dataTransfer ? '[' + [e.dataTransfer.dropEffect, e.dataTransfer.effectAllowed, e.dataTransfer.types.join('+'), e.dataTransfer.getData('text/plain')].join(',') + ']' : '') + ' b' + e.buttons + (e.cancelable ? ' c' : '') + (e.isTrusted ? '' : ' untrusted');
      for (const t of ['dragstart','drag','dragenter','dragover','dragleave','drop','dragend','pointercancel','mouseup','click'])
        document.addEventListener(t, e => log.push(short(e)), true);
      globalThis.moves = 0; document.addEventListener('mousemove', () => moves++);
      const src = document.getElementById('src'), dst = document.getElementById('dst');
      src.addEventListener('dragstart', e => { e.dataTransfer.setData('text/plain', 'payload'); e.dataTransfer.setData('application/x-custom', 'c'); if (globalThis.EA) e.dataTransfer.effectAllowed = EA; });
      dst.addEventListener('dragover', e => { e.dataTransfer.setData('text/plain', 'ignored'); if (!globalThis.NO_ACCEPT) e.preventDefault(); });
      dst.addEventListener('drop', e => { globalThis.dropped = e.dataTransfer; e.preventDefault(); });
      globalThis.center = id => { const r = document.getElementById(id).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; };
      1
    "#,
    );
    (eval_value(h, "center('src')"), eval_value(h, "center('dst')"), eval_value(h, "center('plain')"))
}

fn mouse(h: &mut Harness, kind: &str, at: &Value, dy: f64, buttons: u16) -> Vec<Value> {
    let button = if kind == "mouseMoved" && buttons == 0 { "none" } else { "left" };
    h.page_with_events(
        "Input.dispatchMouseEvent",
        json!({"type":kind,"x":at[0],"y":at[1].as_f64().unwrap() + dy,"button":button,"buttons":buttons,"clickCount":1}),
    )
}

fn drag(h: &mut Harness, kind: &str, at: &Value, data: &Value) -> Value {
    h.page("Input.dispatchDragEvent", json!({"type":kind,"x":at[0],"y":at[1],"data":data,"modifiers":0}))
}

#[test]
fn input_set_intercept_drags_keeps_pointer_drags_on_mouse_events() {
    // Playwright 1.59 wraps every left-button mouse.move in
    // setInterceptDrags(true/false); pointer-driven drags (column resizing,
    // dnd-kit's MouseSensor) must still see plain mouse events.
    let mut h = Harness::new();
    let (_, _, plain) = drag_page(&mut h);
    mouse(&mut h, "mousePressed", &plain, 0.0, 1);
    assert_eq!(h.page("Input.setInterceptDrags", json!({"enabled":true}))["result"], json!({}));
    for dy in [6.0, 12.0, 18.0] {
        let events = mouse(&mut h, "mouseMoved", &plain, dy, 1);
        assert!(!events.iter().any(|e| e["method"] == "Input.dragIntercepted"), "{events:?}");
    }
    assert_eq!(h.page("Input.setInterceptDrags", json!({"enabled":false}))["result"], json!({}));
    mouse(&mut h, "mouseReleased", &plain, 18.0, 0);
    assert_eq!(eval_value(&mut h, "[moves, log.join('|')]"), json!([3, "mouseup@plain b0 c|click@plain b0 c"]));
    assert_eq!(h.page("Input.setInterceptDrags", json!({}))["error"]["code"], -32602);
}

#[test]
fn input_intercepted_native_drag_matches_chromium_event_sequence() {
    let mut h = Harness::new();
    let (src, dst, _) = drag_page(&mut h);
    // Without interception there is no OS drag loop: no dragstart.
    mouse(&mut h, "mousePressed", &src, 0.0, 1);
    mouse(&mut h, "mouseMoved", &src, 10.0, 1);
    mouse(&mut h, "mouseReleased", &src, 10.0, 0);
    assert_eq!(eval_value(&mut h, "log.splice(0).join('|')"), json!("mouseup@src b0 c|click@src b0 c"));

    // A move below the 3px threshold does not start a drag; past it,
    // dragstart fires and Input.dragIntercepted reports the data.
    mouse(&mut h, "mousePressed", &src, 0.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":true}));
    let events = mouse(&mut h, "mouseMoved", &src, 2.0, 1);
    assert!(!events.iter().any(|e| e["method"] == "Input.dragIntercepted"), "{events:?}");
    let events = mouse(&mut h, "mouseMoved", &src, 3.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":false}));
    let intercepted = events.iter().find(|e| e["method"] == "Input.dragIntercepted").expect("dragIntercepted");
    let data = intercepted["params"]["data"].clone();
    assert_eq!(
        data,
        json!({"items":[{"mimeType":"text/plain","data":"payload"},{"mimeType":"application/x-custom","data":"c"}],"dragOperationsMask":-1})
    );
    assert_eq!(drag(&mut h, "dragEnter", &dst, &data)["result"], json!({}));
    assert_eq!(drag(&mut h, "drop", &dst, &data)["result"], json!({}));
    // Mouse release after the drop: no click (the pointer stream was canceled).
    let types = "text/plain+application/x-custom";
    assert_eq!(
        eval_value(&mut h, "log.splice(0)"),
        json!([
            "dragstart@src[none,uninitialized,,] b1 c",
            "pointercancel@src b0",
            format!("drag@src[none,uninitialized,{types},] b0 c"),
            format!("dragenter@dst[none,all,{types},] b0 c"),
            format!("dragover@dst[none,all,{types},] b0 c"),
            format!("drop@dst[copy,all,{types},payload] b0 c"),
            format!("dragend@src[copy,uninitialized,{types},] b0"),
        ])
    );
    // The protected-mode setData in dragover was ignored; the drop store is
    // disabled once its event is over.
    assert_eq!(eval_value(&mut h, "[dropped.types.length, dropped.getData('text/plain')]"), json!([0, ""]));

    // Not accepted (dragover not canceled): drag + dragleave, dragend none.
    eval_value(&mut h, "globalThis.NO_ACCEPT = true; globalThis.EA = 'move'; 1");
    mouse(&mut h, "mousePressed", &src, 0.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":true}));
    let events = mouse(&mut h, "mouseMoved", &src, 5.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":false}));
    let data = events.iter().find(|e| e["method"] == "Input.dragIntercepted").unwrap()["params"]["data"].clone();
    assert_eq!(data["dragOperationsMask"], 16);
    drag(&mut h, "dragEnter", &dst, &data);
    drag(&mut h, "drop", &dst, &data);
    assert_eq!(
        eval_value(&mut h, "log.splice(0).slice(2)"),
        json!([
            format!("drag@src[none,move,{types},] b0 c"),
            format!("dragenter@dst[none,move,{types},] b0 c"),
            format!("dragover@dst[none,move,{types},] b0 c"),
            format!("drag@src[none,move,{types},] b0 c"),
            format!("dragleave@dst[none,move,{types},] b0"),
            format!("dragend@src[none,move,{types},] b0"),
        ])
    );

    // dragCancel (Escape) ends with dragend only; a canceled dragstart starts
    // no drag and the release still clicks.
    eval_value(&mut h, "globalThis.NO_ACCEPT = false; globalThis.EA = undefined; 1");
    mouse(&mut h, "mousePressed", &src, 0.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":true}));
    let events = mouse(&mut h, "mouseMoved", &src, 5.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":false}));
    let data = events.iter().find(|e| e["method"] == "Input.dragIntercepted").unwrap()["params"]["data"].clone();
    drag(&mut h, "dragEnter", &dst, &data);
    assert_eq!(drag(&mut h, "dragCancel", &dst, &json!({"items":[],"dragOperationsMask":65535}))["result"], json!({}));
    assert_eq!(
        eval_value(&mut h, "log.splice(0).slice(-2)"),
        json!([
            format!("dragenter@dst[none,all,{types},] b0 c"),
            format!("dragend@src[none,uninitialized,{types},] b0")
        ])
    );
    eval_value(&mut h, "document.getElementById('src').addEventListener('dragstart', e => e.preventDefault()); 1");
    mouse(&mut h, "mousePressed", &src, 0.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":true}));
    let events = mouse(&mut h, "mouseMoved", &src, 5.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":false}));
    assert!(!events.iter().any(|e| e["method"] == "Input.dragIntercepted"), "{events:?}");
    mouse(&mut h, "mouseReleased", &src, 5.0, 0);
    assert_eq!(
        eval_value(&mut h, "log.splice(0)"),
        json!(["dragstart@src[none,uninitialized,,] b1 c", "mouseup@src b0 c", "click@src b0 c"])
    );
    assert_eq!(drag(&mut h, "drop", &dst, &data)["error"]["message"], "no drag in progress");
    let files = json!({"items":[],"files":["/etc/hosts"],"dragOperationsMask":1});
    assert_eq!(drag(&mut h, "dragEnter", &dst, &files)["error"]["code"], -32000);
}

#[test]
fn input_intercepted_link_drag_carries_chromium_link_data() {
    let mut h = Harness::new();
    drag_page(&mut h);
    let link = eval_value(&mut h, "center('lnk')");
    mouse(&mut h, "mousePressed", &link, 0.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":true}));
    let events = mouse(&mut h, "mouseMoved", &link, 4.0, 1);
    h.page("Input.setInterceptDrags", json!({"enabled":false}));
    let data =
        events.iter().find(|e| e["method"] == "Input.dragIntercepted").expect("link drag")["params"]["data"].clone();
    let url = eval_value(&mut h, "document.getElementById('lnk').href");
    let html = eval_value(&mut h, "document.getElementById('lnk').outerHTML");
    let title = eval_value(&mut h, "document.getElementById('lnk').innerText");
    assert_eq!(
        data,
        json!({"items":[
            {"mimeType":"text/uri-list","data":url,"title":title},
            {"mimeType":"text/plain","data":url},
            {"mimeType":"text/html","data":html,"baseURL":url},
        ],"dragOperationsMask":-1})
    );
    drag(&mut h, "dragCancel", &link, &data);
}

#[test]
fn set_content_document_write_reports_fresh_lifecycle_after_console_tag() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    h.page("Page.enable", json!({}));
    h.page_with_events("Page.setLifecycleEventsEnabled", json!({"enabled":true}));
    h.run_for(50);
    let frame = h.frame_id.clone();
    let utility = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"__pw_utility"}))["result"]
        ["executionContextId"]
        .clone();
    // Playwright 1.59 frames.setContent, evaluated in the utility world.
    let source = r#"(({ html, tag }) => { document.open(); console.debug(tag); document.write(html); document.close(); })({ html: '<!doctype html><title>mail</title><a href="/next">Open conversation</a><script>window.ran = document.readyState<\/script>', tag: '--tag--' })"#;
    let mut events =
        h.page_with_events("Runtime.evaluate", json!({"expression":source,"contextId":utility,"awaitPromise":true}));
    events.extend(h.run_for(100));
    let at = |pred: &dyn Fn(&Value) -> bool| events.iter().position(pred);
    let tag = at(&|e| e["method"] == "Runtime.consoleAPICalled" && e["params"]["args"][0]["value"] == "--tag--")
        .unwrap_or_else(|| panic!("console tag: {events:#?}"));
    let frame = h.frame_id.clone();
    let lifecycle = |name: &str| {
        events.iter().position(|e| {
            e["method"] == "Page.lifecycleEvent" && e["params"]["name"] == name && e["params"]["frameId"] == frame
        })
    };
    let dcl = lifecycle("DOMContentLoaded").unwrap_or_else(|| panic!("DOMContentLoaded: {events:#?}"));
    let load = lifecycle("load").unwrap_or_else(|| panic!("load: {events:#?}"));
    assert!(tag < dcl && dcl < load, "{events:#?}");
    assert!(at(&|e| e["method"] == "Page.loadEventFired").is_some_and(|i| i > tag), "{events:#?}");
    assert_eq!(
        eval_value(
            &mut h,
            "[window.ran, document.title, document.readyState, document.querySelector('a').href.endsWith('/next')]"
        ),
        json!(["loading", "mail", "complete", true])
    );
}

#[test]
fn input_mouse_move_to_empty_viewport_space_leaves_hovered_element_as_chromium() {
    // TOS TS-100 dismisses a hover tooltip with page.mouse.move(0, 0). Chrome
    // hit-tests empty viewport space as the root element; the synthetic
    // layout has no element there. Expected sequences recorded from headless
    // Chromium (Playwright 1.59.1) for the same page.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<header><button id="trigger" style="width:120px;height:30px">MSKU</button></header>';
      globalThis.log=[];
      for (const t of ['pointerover','pointerout','pointerenter','pointerleave','mouseover','mouseout','mouseenter','mouseleave','mousemove'])
        document.addEventListener(t, e => log.push(t + '@' + (e.target.id || e.target.nodeName) + (e.bubbles ? '' : ' nb') + (e.cancelable ? ' c' : '')), true);
      const r = document.getElementById('trigger').getBoundingClientRect(); globalThis.point = [r.x + r.width / 2, r.y + r.height / 2];
      1
    "#,
    );
    let point = eval_value(&mut h, "point");
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(0, 0) === null"), json!(true));
    let mut moved = |x: &Value, y: &Value| {
        let r =
            h.page("Input.dispatchMouseEvent", json!({"type":"mouseMoved","x":x,"y":y,"button":"none","buttons":0}));
        assert_eq!(r["result"], json!({}), "{r}");
        eval_value(&mut h, "log.splice(0).join(' ')")
    };
    assert_eq!(
        moved(&json!(0), &json!(0)),
        json!("pointerover@HTML c pointerenter@#document nb pointerenter@HTML nb mouseover@HTML c mouseenter@#document nb mouseenter@HTML nb mousemove@HTML c")
    );
    assert_eq!(
        moved(&point[0], &point[1]),
        json!("pointerout@HTML c pointerover@trigger c pointerenter@BODY nb pointerenter@HEADER nb pointerenter@trigger nb mouseout@HTML c mouseover@trigger c mouseenter@BODY nb mouseenter@HEADER nb mouseenter@trigger nb mousemove@trigger c")
    );
    assert_eq!(
        moved(&json!(0), &json!(0)),
        json!("pointerout@trigger c pointerleave@trigger nb pointerleave@HEADER nb pointerleave@BODY nb pointerover@HTML c mouseout@trigger c mouseleave@trigger nb mouseleave@HEADER nb mouseleave@BODY nb mouseover@HTML c mousemove@HTML c")
    );
    // Outside the viewport and presses on empty space keep the explicit error.
    let r =
        h.page("Input.dispatchMouseEvent", json!({"type":"mouseMoved","x":-5,"y":2000,"button":"none","buttons":0}));
    assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("no element at point")), "{r}");
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":0,"y":0,"button":"left","buttons":1}));
    assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("no element at point")), "{r}");
}

#[test]
fn input_boundary_events_carry_related_targets_and_follow_a_replaced_hovered_element() {
    // @povio/ui swaps a static input for the interactive one a frame after
    // mouseenter; react-aria's hover state then needs Chrome's boundary
    // events for the element now under the stationary pointer. A removed
    // hovered element gets no out/leave; over/enter name it as relatedTarget
    // and no move events fire, about 100 ms later (Chromium measured 110 ms
    // in the @povio/ui QueryAutocomplete case). Expectations follow Blink's
    // BoundaryEventDispatcher::SendBoundaryEvents; they were not recorded
    // from Chromium.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<div id="other" style="width:200px;height:40px">O</div><div id="cell" style="margin-top:100px"><div id="static" style="width:200px;height:40px">S</div></div>';
      globalThis.log=[];
      for (const t of ['pointerover','pointerout','pointerenter','pointerleave','mouseover','mouseout','mouseenter','mouseleave','mousemove'])
        document.addEventListener(t, e => log.push(t + '@' + (e.target.id || e.target.nodeName) + '<' + (e.relatedTarget?.id || e.relatedTarget?.nodeName || '-')), true);
      const c = (id) => { const r = document.getElementById(id).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; };
      globalThis.points = [c('static'), c('other')];
      1
    "#,
    );
    let points = eval_value(&mut h, "points");
    let mut moved = |p: &Value| {
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":p[0],"y":p[1],"button":"none","buttons":0}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
        eval_value(&mut h, "log.splice(0).join(' ')")
    };
    moved(&points[1]);
    assert_eq!(
        moved(&points[0]),
        json!("pointerout@other<static pointerleave@other<static pointerover@static<other pointerenter@cell<other pointerenter@static<other mouseout@other<static mouseleave@other<static mouseover@static<other mouseenter@cell<other mouseenter@static<other mousemove@static<-")
    );
    eval_value(
        &mut h,
        "const live = document.createElement('div'); live.id = 'live'; live.setAttribute('style', 'width:200px;height:40px'); document.getElementById('cell').replaceChildren(live); 1",
    );
    // Not in the mutating task: Chromium updates hover on its fake-mouse-move
    // timer (~100 ms), after frameworks have cleaned up the removed node.
    h.run_for(30);
    assert_eq!(eval_value(&mut h, "log.splice(0).join(' ')"), json!(""));
    h.run_for(150);
    assert_eq!(
        eval_value(&mut h, "log.splice(0).join(' ')"),
        json!("pointerover@live<static pointerenter@#document<static pointerenter@HTML<static pointerenter@BODY<static pointerenter@cell<static pointerenter@live<static mouseover@live<static mouseenter@#document<static mouseenter@HTML<static mouseenter@BODY<static mouseenter@cell<static mouseenter@live<static")
    );
    assert_eq!(eval_value(&mut h, "document.getElementById('live').matches(':hover')"), json!(true));
}

#[test]
fn hyperlink_downloads_follow_set_download_behavior_like_chromium() {
    // Playwright 1.59 sets allowAndName per context and waits for
    // Browser.downloadWillBegin / downloadProgress on the browser session, then
    // reads downloadPath/guid. Event shapes recorded from headless Chromium.
    static HTML: &str = "<!doctype html><title>dl</title>";
    let (addr, server) = serve_html(2, HTML);
    let mut h = Harness::new();
    let targets = h.call(None, "Target.getTargets", json!({}));
    let frame = h.frame_id.clone();
    let ctx = targets["result"]["targetInfos"].as_array().unwrap().iter().find(|t| t["targetId"] == frame).unwrap()
        ["browserContextId"]
        .clone();
    let dir = std::env::temp_dir().join(format!("lp-downloads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let r = h.call(
        None,
        "Browser.setDownloadBehavior",
        json!({"behavior":"allowAndName","browserContextId":ctx,"downloadPath":dir,"eventsEnabled":true}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({"url": format!("http://{addr}/page")}));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        until_event(&mut h, "Page.loadEventFired");
    }
    let download = |h: &mut Harness, script: &str| -> Vec<Value> {
        eval_value(h, script);
        let mut events = h.run_for(150);
        events.retain(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Browser.download")));
        events
    };
    let check = |events: &[Value], name: &str, url_prefix: &str, body: &[u8]| {
        assert_eq!(events.len(), 3, "{events:#?}");
        let begin = &events[0]["params"];
        assert_eq!(events[0]["method"], "Browser.downloadWillBegin");
        assert!(events.iter().all(|e| e.get("sessionId").is_none()), "browser-session events: {events:#?}");
        assert_eq!(begin["frameId"], frame);
        assert_eq!(begin["suggestedFilename"], name);
        assert!(begin["url"].as_str().unwrap().starts_with(url_prefix), "{begin}");
        let guid = begin["guid"].as_str().unwrap();
        assert_eq!(guid.len(), 36);
        let total = body.len();
        assert_eq!(events[1]["params"], json!({"guid":guid,"totalBytes":total,"receivedBytes":0,"state":"inProgress"}));
        let path = dir.join(guid);
        assert_eq!(
            events[2]["params"],
            json!({"guid":guid,"totalBytes":total,"receivedBytes":total,"state":"completed","filePath":path.to_string_lossy()})
        );
        assert_eq!(std::fs::read(&path).unwrap(), body);
    };
    // blob: link revoked right after click() (TOS file.utils downloadFileFromUrl).
    let events = download(
        &mut h,
        r#"const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob(['hello,world\n'], {type: 'text/csv'}));
           a.download = 'export-1.csv'; document.body.appendChild(a); a.click(); URL.revokeObjectURL(a.href); a.remove(); 1"#,
    );
    let origin = format!("blob:http://{addr}/");
    check(&events, "export-1.csv", &origin, b"hello,world\n");
    // Dispatched (untrusted) click, as TOS downloadFile does; no navigation.
    let events = download(
        &mut h,
        r#"const b = document.createElement('a'); b.href = 'data:text/plain;base64,aGk='; b.download = '';
           b.dispatchEvent(new MouseEvent('click')); 1"#,
    );
    check(&events, "download", "data:text/plain", b"hi");
    // Same-origin HTTP link: fetched, named from the URL.
    let events = download(
        &mut h,
        "const c = document.createElement('a'); c.href = '/reports/q3.txt'; c.download = ''; c.click(); 1",
    );
    check(&events, "q3.txt", &format!("http://{addr}/reports/q3.txt"), HTML.as_bytes());
    assert_eq!(eval_value(&mut h, "location.pathname + '|' + document.title"), json!("/page|dl"));
    server.join().unwrap();
    // Deny delivers nothing.
    h.call(None, "Browser.setDownloadBehavior", json!({"behavior":"deny","browserContextId":ctx}));
    let events = download(
        &mut h,
        "const d = document.createElement('a'); d.href = URL.createObjectURL(new Blob(['x'])); d.download = 'x.txt'; d.click(); 1",
    );
    assert!(events.is_empty(), "{events:#?}");
    let r = h.call(None, "Browser.setDownloadBehavior", json!({"behavior":"allow","browserContextId":ctx}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn error_remote_objects_carry_chrome_style_name_and_message() {
    // console.error(err) text is built from the argument's description. Chrome
    // describes errors as `name: message` plus `    at` frames; JSC's stack
    // alone has only frames, which hid TOS app errors behind bare stacks.
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    h.run_for(20);
    let mut events = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": r#"
          function load() { throw new TypeError("bad id"); }
          try { load(); } catch (e) { console.error(e); }
          const renamed = new Error("custom message"); renamed.name = "AxiosError"; console.error("Query failed:", renamed);
          console.error(new DOMException("aborted", "AbortError"));
          const anon = new Error("only message"); anon.name = ""; console.error(anon);
          1"#}),
    );
    events.extend(h.run_for(50));
    let args: Vec<Value> = events
        .iter()
        .filter(|e| e["method"] == "Runtime.consoleAPICalled")
        .flat_map(|e| e["params"]["args"].as_array().unwrap().clone())
        .filter(|a| a["subtype"] == "error")
        .collect();
    let first_line = |a: &Value| a["description"].as_str().unwrap().lines().next().unwrap().to_owned();
    assert_eq!(
        args.iter().map(first_line).collect::<Vec<_>>(),
        ["TypeError: bad id", "AxiosError: custom message", "AbortError: aborted", "only message"]
    );
    let typed = args[0]["description"].as_str().unwrap();
    assert!(typed.lines().nth(1).is_some_and(|l| l.starts_with("    at load")), "{typed}");
    assert_eq!(args[0]["className"], "TypeError");
}

/// Serves `responses` requests: `/page` as HTML (links to `/file`), anything
/// else with the given extra headers and body.
fn serve_attachment(
    responses: usize,
    headers: &'static str,
    body: &'static [u8],
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            let page = request.starts_with(b"GET /page ");
            let (head, body): (String, &[u8]) = if page {
                let html: &[u8] = b"<!doctype html><title>stay</title><a id=f href=\"/file/report.bin\">f</a>";
                (format!("Content-Type: text/html\r\nContent-Length: {}\r\n", html.len()), html)
            } else {
                (format!("{headers}Content-Length: {}\r\n", body.len()), body)
            };
            stream.write_all(format!("HTTP/1.1 200 OK\r\n{head}Connection: close\r\n\r\n").as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        }
    });
    (address, server)
}

#[test]
fn content_disposition_attachment_navigation_becomes_download() {
    // Recorded from headless Chromium: the navigation starts, is aborted
    // without frameNavigated (Page.navigate gets net::ERR_ABORTED), the page
    // stays, and the response is delivered as a download named by
    // filename* over filename.
    let (addr, server) = serve_attachment(
        3,
        "Content-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"report 2026.xlsx\"; filename*=UTF-8''r%C3%A9port.xlsx\r\n",
        b"\x01\x02\x03",
    );
    let mut h = Harness::new();
    let targets = h.call(None, "Target.getTargets", json!({}));
    let frame = h.frame_id.clone();
    let ctx = targets["result"]["targetInfos"].as_array().unwrap().iter().find(|t| t["targetId"] == frame).unwrap()
        ["browserContextId"]
        .clone();
    let dir = std::env::temp_dir().join(format!("lp-cd-downloads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.call(
        None,
        "Browser.setDownloadBehavior",
        json!({"behavior":"allowAndName","browserContextId":ctx,"downloadPath":dir,"eventsEnabled":true}),
    );
    h.page("Page.enable", json!({}));
    h.page("Network.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({"url": format!("http://{addr}/page")}));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        until_event(&mut h, "Page.loadEventFired");
    }
    // A link click (anchor navigation).
    eval_value(&mut h, "document.getElementById('f').click(); 1");
    let mut events = h.run_for(300);
    let downloads: Vec<&Value> =
        events.iter().filter(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Browser.download"))).collect();
    assert_eq!(downloads.len(), 3, "{events:#?}");
    assert_eq!(downloads[0]["params"]["suggestedFilename"], "réport.xlsx");
    assert_eq!(downloads[0]["params"]["url"], format!("http://{addr}/file/report.bin"));
    let path = downloads[2]["params"]["filePath"].as_str().unwrap().to_owned();
    assert_eq!(std::fs::read(&path).unwrap(), b"\x01\x02\x03");
    assert!(!events.iter().any(|e| e["method"] == "Page.frameNavigated"), "{events:#?}");
    // Playwright resolves the pending navigation from these: the document
    // request fails as aborted (no dataReceived), and the requested
    // navigation is cleared before loading stops.
    let at = |m: &str| events.iter().position(|e| e["method"] == m);
    let failed = events.iter().find(|e| e["method"] == "Network.loadingFailed").expect("loadingFailed");
    assert_eq!(failed["params"]["errorText"], "net::ERR_ABORTED");
    assert_eq!(failed["params"]["canceled"], true);
    assert_eq!(failed["params"]["type"], "Document");
    assert!(!events.iter().any(|e| e["method"] == "Network.dataReceived" || e["method"] == "Network.loadingFinished"));
    let cleared = at("Page.frameClearedScheduledNavigation").expect("cleared");
    assert!(cleared < at("Page.frameStoppedLoading").unwrap(), "{events:#?}");
    assert_eq!(eval_value(&mut h, "document.title + '|' + location.pathname"), json!("stay|/page"));
    // Page.navigate to an attachment URL answers net::ERR_ABORTED.
    events = h.page_with_events("Page.navigate", json!({"url": format!("http://{addr}/file/direct.bin")}));
    events.extend(h.run_for(300));
    let response = events.iter().find(|e| e.get("id").is_some() && e["result"].get("frameId").is_some()).unwrap();
    assert_eq!(response["result"]["errorText"], "net::ERR_ABORTED", "{response}");
    assert!(events.iter().any(|e| e["method"] == "Browser.downloadWillBegin"), "{events:#?}");
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn input_home_end_with_mac_scroll_commands_scroll_the_document() {
    // Playwright sends Home/End with scrollTo{Beginning,End}OfDocument on
    // macOS. Outside editable content Chrome scrolls the document. Scroll
    // events arrive at the next frame, so each key waits past one.
    let mut h = Harness::new();
    eval_value(&mut h, "document.body.innerHTML = '<div style=\"height:5000px\"></div>'; globalThis.scrolls = 0; document.addEventListener('scroll', () => scrolls++); 1");
    for (key, command) in [("End", "scrollToEndOfDocument"), ("Home", "scrollToBeginningOfDocument")] {
        let r = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":"rawKeyDown","key":key,"code":key,"windowsVirtualKeyCode":if key == "End" { 35 } else { 36 },"commands":[command]}),
        );
        assert_eq!(r["result"], json!({}), "{key}: {r}");
        h.run_for(200);
        let y = eval_value(&mut h, "scrollY").as_f64().unwrap();
        if key == "End" {
            assert!(y > 0.0, "End scrolls to the bottom");
        } else {
            assert_eq!(y, 0.0, "Home scrolls to the top");
        }
    }
    assert_eq!(eval_value(&mut h, "scrolls"), json!(2));
}

#[test]
fn file_inputs_report_file_chooser_opened_when_intercepted() {
    // Playwright's waitForEvent('filechooser') enables interception, then
    // expects Page.fileChooserOpened (shapes recorded from headless Chromium).
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"document.body.innerHTML = '<input type="file" id="f" style="width:100px;height:30px"><input type="file" id="m" multiple style="width:100px;height:30px"><label for="f" id="lab" style="display:block;width:100px;height:30px">Up</label>';
           globalThis.clicks = []; document.getElementById('f').addEventListener('click', e => clicks.push(e.isTrusted));
           globalThis.center = id => { const r = document.getElementById(id).getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }; 1"#,
    );
    let click = |h: &mut Harness, id: &str| -> Vec<Value> {
        let at = eval_value(h, &format!("center('{id}')"));
        let mut events = Vec::new();
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            events.extend(h.page_with_events(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":at[0],"y":at[1],"button":"left","buttons":buttons,"clickCount":1}),
            ));
        }
        events.extend(h.run_for(30));
        events.into_iter().filter(|e| e["method"] == "Page.fileChooserOpened").collect()
    };
    assert!(click(&mut h, "f").is_empty(), "no interception, no event");
    assert_eq!(h.page("Page.setInterceptFileChooserDialog", json!({"enabled":true}))["result"], json!({}));
    let frame = h.frame_id.clone();
    let opened = click(&mut h, "f");
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0]["params"]["frameId"], frame);
    assert_eq!(opened[0]["params"]["mode"], "selectSingle");
    let backend = opened[0]["params"]["backendNodeId"].clone();
    let resolved = h.page("DOM.resolveNode", json!({"backendNodeId": backend}));
    let object = resolved["result"]["object"]["objectId"].clone();
    let id = h.page(
        "Runtime.callFunctionOn",
        json!({"objectId": object, "functionDeclaration": "function() { return this.id; }", "returnByValue": true}),
    );
    assert_eq!(id["result"]["result"]["value"], "f");
    let opened = click(&mut h, "m");
    assert_eq!(opened[0]["params"]["mode"], "selectMultiple");
    // A label forwards the user's (trusted) click and opens its control's chooser.
    let opened = click(&mut h, "lab");
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(eval_value(&mut h, "clicks"), json!([true, true, true]));
    h.page("Page.setInterceptFileChooserDialog", json!({"enabled":false}));
    assert!(click(&mut h, "f").is_empty());
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
fn dom_scroll_delivers_each_changed_ancestor_at_the_next_frame() {
    // Scroll events fire in the next rendering update (frame), once per
    // changed scroller: reads wait past one frame (30 ms).
    for primed in [false, true] {
        let mut h = Harness::new();
        let setup = h.page(
            "Runtime.evaluate",
            json!({"expression":r#"
              const rows = n => '<div style="height:20px">row</div>'.repeat(n);
              document.body.innerHTML = rows(180) +
                '<div id="outer" style="height:240px;width:400px;overflow:auto">' + rows(140) +
                '<div id="inner" style="height:120px;width:300px;overflow:auto">' + rows(100) +
                '<button id="target">Target</button>' + rows(100) + '</div>' + rows(140) + '</div>';
              globalThis.scrollEvents = [];
              document.addEventListener('scroll', e => {
                scrollEvents.push([e.target === document ? 'document' : e.target.id, e.bubbles, e.isTrusted]);
              }, true);
              globalThis.scrollState = () => {
                const target = document.querySelector('#target').getBoundingClientRect();
                return ['inner', 'outer'].map(id => {
                  const node = document.querySelector('#' + id), rect = node.getBoundingClientRect();
                  return [node.scrollTop, target.top >= rect.top && target.bottom <= rect.bottom &&
                    target.left >= rect.left && target.right <= rect.right];
                });
              };
              document.querySelector('#target')
            "#}),
        );
        let object = setup["result"]["result"]["objectId"].clone();
        assert!(object.is_string(), "{setup}");
        if primed {
            eval_value(
                &mut h,
                "document.querySelector('#inner').scrollTop=1;document.querySelector('#outer').scrollTop=1;true",
            );
            let r = h.page("Runtime.evaluate", json!({"expression":"new Promise(r=>setTimeout(()=>{scrollEvents=[];r(true)},30))","awaitPromise":true,"returnByValue":true}));
            assert_eq!(r["result"]["result"]["value"], true, "{r}");
        }
        let before = eval_value(&mut h, "scrollState()");
        assert_eq!(before[0][1], false, "inner must initially clip the target: {before}");
        assert_eq!(before[1][1], false, "outer must initially clip the target: {before}");
        assert_eq!(h.page("DOM.scrollIntoViewIfNeeded", json!({"objectId":object}))["result"], json!({}));
        let r = h.page("Runtime.evaluate", json!({"expression":"new Promise(r=>setTimeout(()=>r([scrollState(),scrollEvents]),30))","awaitPromise":true,"returnByValue":true}));
        let value = &r["result"]["result"]["value"];
        for index in 0..2 {
            assert!(value[0][index][0].as_f64().unwrap() > 1.0, "ancestor must scroll: {r}");
            assert_eq!(value[0][index][1], true, "target must be revealed in both ancestors: {r}");
        }
        assert_eq!(value[1], json!([["inner", false, true], ["outer", false, true], ["document", true, true]]));
        eval_value(&mut h, "scrollEvents=[];true");
        assert_eq!(h.page("DOM.scrollIntoViewIfNeeded", json!({"objectId":object}))["result"], json!({}));
        let repeated = h.page("Runtime.evaluate", json!({"expression":"new Promise(r=>setTimeout(()=>r(scrollEvents),30))","awaitPromise":true,"returnByValue":true}));
        assert_eq!(
            repeated["result"]["result"]["value"],
            json!([]),
            "unchanged offsets must not emit scroll: {repeated}"
        );
        assert_eq!(h.page("DOM.focus", json!({"objectId":object}))["result"], json!({}));
        let focused = h.page("Runtime.evaluate", json!({"expression":"new Promise(r=>setTimeout(()=>r([document.activeElement.id,scrollEvents]),30))","awaitPromise":true,"returnByValue":true}));
        assert_eq!(
            focused["result"]["result"]["value"],
            json!(["target", []]),
            "focus after reveal must not trigger a late ancestor scroll: {focused}"
        );
    }
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
        ("screenOrientation", json!({"type":"portraitPrimary","angle":45})),
        ("screenOrientation", json!({"type":"portraitPrimary","angle":-90})),
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

#[test]
fn handle_limit_cannot_change_after_sessions_attach() {
    let mut h = Harness::with_handle_limit(2);
    // A late change must fail without touching the budget the session uses.
    let late = h.browser.host_mut().set_handle_limit(1_000);
    assert!(late.is_err(), "late configuration rejected in every build");
    let session = h.session.clone();
    for _ in 0..2 {
        let r = h.call(Some(&session), "Runtime.evaluate", json!({"expression":"({})"}));
        assert!(r["result"]["result"]["objectId"].is_string(), "{r}");
    }
    let r = h.call(Some(&session), "Runtime.evaluate", json!({"expression":"({})"}));
    assert_eq!(r["error"]["message"], "Too many remote objects; release unused handles", "original capacity kept: {r}");
    // Once no session (and no table) shares the budget, a new limit applies
    // to sessions attached afterwards.
    h.call(None, "Target.detachFromTarget", json!({"sessionId": session}));
    h.browser.host_mut().set_handle_limit(3).expect("no session holds the budget any more");
    let next = h.attach_second_session();
    for _ in 0..3 {
        let r = h.call(Some(&next), "Runtime.evaluate", json!({"expression":"({})"}));
        assert!(r["result"]["result"]["objectId"].is_string(), "{r}");
    }
    let r = h.call(Some(&next), "Runtime.evaluate", json!({"expression":"({})"}));
    assert!(r.get("error").is_some(), "new limit enforced: {r}");
}

/// Playwright 1.59 sends `screenOrientation {angle:0, type:"landscapePrimary"}`
/// for every non-mobile viewport; `page.setViewportSize(390x844)` also makes
/// the screen 390x844. The override is stored and reported by
/// `screen.orientation` regardless of dimensions (Chrome and upstream).
#[test]
fn screen_orientation_override_is_independent_of_dimensions() {
    let mut h = Harness::new();
    let probe = "[innerWidth,innerHeight,screen.width,screen.height,screen.orientation.type,screen.orientation.angle,matchMedia('(orientation: portrait)').matches]";
    let read = |h: &mut Harness| {
        h.page("Runtime.evaluate", json!({"expression":probe,"returnByValue":true}))["result"]["result"]["value"]
            .clone()
    };
    let desktop = json!({"mobile":false,"width":1440,"height":900,"screenWidth":1440,"screenHeight":900,"deviceScaleFactor":1,"screenOrientation":{"angle":0,"type":"landscapePrimary"},"dontSetVisibleSize":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", desktop)["result"], json!({}));
    assert_eq!(read(&mut h), json!([1440, 900, 1440, 900, "landscape-primary", 0, false]));
    // The TOS "mobile" case: page.setViewportSize({width: 390, height: 844}).
    let phone = json!({"mobile":false,"width":390,"height":844,"screenWidth":390,"screenHeight":844,"deviceScaleFactor":1,"screenOrientation":{"angle":0,"type":"landscapePrimary"},"dontSetVisibleSize":false});
    let r = h.page("Emulation.setDeviceMetricsOverride", phone);
    assert_eq!(r["result"], json!({}), "{r}");
    // CSS (orientation) follows the viewport; screen.orientation reports the override.
    assert_eq!(read(&mut h), json!([390, 844, 390, 844, "landscape-primary", 0, true]));
    let rotated = json!({"mobile":false,"width":844,"height":390,"deviceScaleFactor":1,"screenOrientation":{"angle":270,"type":"landscapeSecondary"}});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", rotated)["result"], json!({}));
    assert_eq!(read(&mut h), json!([844, 390, 844, 390, "landscape-secondary", 270, false]));
    // Without an override the type follows the screen dimensions again.
    let plain = json!({"mobile":false,"width":390,"height":844,"deviceScaleFactor":1});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", plain)["result"], json!({}));
    assert_eq!(read(&mut h), json!([390, 844, 390, 844, "portrait-primary", 0, true]));
    let portrait = json!({"mobile":false,"width":390,"height":844,"deviceScaleFactor":1,"screenOrientation":{"angle":90,"type":"portraitSecondary"}});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", portrait)["result"], json!({}));
    assert_eq!(h.page("Emulation.clearDeviceMetricsOverride", json!({}))["result"], json!({}));
    let cleared = read(&mut h);
    assert_eq!(cleared[4], "landscape-primary", "clear removes the override: {cleared}");
    assert_eq!(cleared[5], 0);
}

/// Index of the first message matching `pred`, for ordering assertions.
fn position(messages: &[Value], what: &str, pred: impl Fn(&Value) -> bool) -> usize {
    messages.iter().position(pred).unwrap_or_else(|| panic!("{what} missing in {messages:#?}"))
}

#[test]
fn events_produced_by_an_evaluation_precede_its_reply() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let session = h.session.clone();
    // Synchronous evaluation.
    let id = h.send(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression":"console.log('during'); 42","returnByValue":true}),
    );
    let messages = h.until_response(id);
    let console = position(&messages, "consoleAPICalled", |m| m["method"] == "Runtime.consoleAPICalled");
    let reply = position(&messages, "reply", |m| m["id"] == id);
    assert!(console < reply, "console event must precede the evaluate reply: {messages:#?}");
    assert_eq!(messages[reply]["result"]["result"]["value"], 42);
    // Awaited evaluation: the reply is produced in a later microtask.
    let id = h.send(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression":"Promise.resolve().then(() => { console.log('then'); return 7 })","awaitPromise":true,"returnByValue":true}),
    );
    let messages = h.until_response(id);
    let console = position(&messages, "consoleAPICalled", |m| m["method"] == "Runtime.consoleAPICalled");
    let reply = position(&messages, "reply", |m| m["id"] == id);
    assert!(console < reply, "{messages:#?}");
    assert_eq!(messages[reply]["result"]["result"]["value"], 7);
}

/// One-shot HTTP server for `responses` sequential requests, each answered
/// with the same HTML document.
fn serve_html(responses: usize, html: &'static str) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..responses {
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
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                html.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(html.as_bytes()).unwrap();
        }
    });
    (address, server)
}

/// TOS credit-note shape: after `history.pushState` the client must see
/// `Page.navigatedWithinDocument` before the reply of the evaluation that
/// called it, and the frame URL must already be current (Playwright's
/// `page.url()` reads it right after). Needs the synchronous same-document
/// commit in lp-browser plus queued replies in lp-cdp.
#[test]
fn push_state_event_precedes_the_evaluate_reply() {
    let (address, server) = serve_html(1, "<!doctype html><p>pushstate</p>");
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    let mut messages = h.drain();
    messages.extend(h.run_for(50));
    server.join().unwrap();
    let messages = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression":"history.pushState(null, '', '/pushed-1'); location.pathname","returnByValue":true}),
    );
    let pushed = position(&messages, "navigatedWithinDocument", |m| m["method"] == "Page.navigatedWithinDocument");
    let reply = position(&messages, "evaluate reply", |m| m["result"]["result"]["value"] == "/pushed-1");
    assert!(pushed < reply, "navigatedWithinDocument must precede the evaluate reply: {messages:#?}");
    assert_eq!(messages[pushed]["params"]["url"], format!("http://{address}/pushed-1"));
    assert_eq!(messages[pushed]["params"]["frameId"], h.frame_id.as_str());
    let tree = h.page("Page.getFrameTree", json!({}));
    assert_eq!(tree["result"]["frameTree"]["frame"]["url"], format!("http://{address}/pushed-1"));
}

/// Playwright on macOS sends the editing commands its table derives from the
/// key (crInput `_commandsForCode`), e.g. ArrowRight -> ["moveRight"]. Those are
/// accepted and the engine's own key default actions run (TOS batch 12,
/// docubox-templates `keyboard.press("ArrowRight")`). Commands that are not the
/// key's own encoding still fail explicitly, before any event is delivered.
#[test]
fn input_accepts_mac_key_derived_editing_commands() {
    let mut h = Harness::new();
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":r#"
        document.body.innerHTML = '<input id="field" value="abc"><button id="tree">tree</button>';
        globalThis.field = document.querySelector('#field');
        globalThis.keys = [];
        document.addEventListener('keydown', e => keys.push(e.key));
        field.focus(); field.setSelectionRange(0, 0);
    "#}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let press = |h: &mut Harness, key: &str, code: &str, vk: u32, modifiers: u32, commands: Value| {
        // Exactly the parameters Playwright 1.59 sends on macOS.
        let down = json!({"type":"rawKeyDown","modifiers":modifiers,"windowsVirtualKeyCode":vk,"code":code,"commands":commands,
            "key":key,"text":"","unmodifiedText":"","autoRepeat":false,"location":0,"isKeypad":false});
        let r = h.page("Input.dispatchKeyEvent", down);
        assert_eq!(r["result"], json!({}), "{key}: {r}");
        let up =
            json!({"type":"keyUp","modifiers":modifiers,"key":key,"windowsVirtualKeyCode":vk,"code":code,"location":0});
        assert_eq!(h.page("Input.dispatchKeyEvent", up)["result"], json!({}));
    };
    let selection = |h: &mut Harness| {
        h.page(
            "Runtime.evaluate",
            json!({"expression":"[field.selectionStart, field.selectionEnd]","returnByValue":true}),
        )["result"]["result"]["value"]
            .clone()
    };
    press(&mut h, "ArrowRight", "ArrowRight", 39, 0, json!(["moveRight"]));
    assert_eq!(selection(&mut h), json!([1, 1]));
    press(&mut h, "ArrowRight", "ArrowRight", 39, 8, json!(["moveRightAndModifySelection"]));
    assert_eq!(selection(&mut h), json!([1, 2]));
    press(&mut h, "ArrowLeft", "ArrowLeft", 37, 0, json!(["moveLeft"]));
    assert_eq!(selection(&mut h), json!([1, 1]));
    press(&mut h, "a", "KeyA", 65, 4, json!(["selectAll"]));
    assert_eq!(selection(&mut h), json!([0, 3]));
    // A focused non-editable element (the TOS case) receives the key event too.
    h.page("Runtime.evaluate", json!({"expression":"document.querySelector('#tree').focus()"}));
    press(&mut h, "ArrowRight", "ArrowRight", 39, 0, json!(["moveRight"]));
    // TOS W3-08 quotes-finances: `keyboard.press("Escape")` closes a
    // QuerySelect. Playwright sends ["cancelOperation"]; the events are
    // delivered and nothing is edited.
    press(&mut h, "Escape", "Escape", 27, 0, json!(["cancelOperation"]));
    // TOS W3-08 QuerySelect opener: ArrowUp/ArrowDown move the caret to the
    // start/end of a single-line input (lp-webapi default action) and carry
    // moveUp/moveDown; Shift extends.
    h.page("Runtime.evaluate", json!({"expression":"field.focus(); field.setSelectionRange(1, 1)"}));
    press(&mut h, "ArrowDown", "ArrowDown", 40, 0, json!(["moveDown"]));
    assert_eq!(selection(&mut h), json!([3, 3]));
    press(&mut h, "ArrowUp", "ArrowUp", 38, 8, json!(["moveUpAndModifySelection"]));
    assert_eq!(selection(&mut h), json!([0, 3]));
    press(&mut h, "ArrowUp", "ArrowUp", 38, 0, json!(["moveUp"]));
    assert_eq!(selection(&mut h), json!([0, 0]));
    press(&mut h, "ArrowDown", "ArrowDown", 40, 8, json!(["moveDownAndModifySelection"]));
    assert_eq!(selection(&mut h), json!([0, 3]));
    h.page("Runtime.evaluate", json!({"expression":"document.querySelector('#tree').focus()"}));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"keys","returnByValue":true}))["result"]["result"]["value"],
        json!([
            "ArrowRight",
            "ArrowRight",
            "ArrowLeft",
            "a",
            "ArrowRight",
            "Escape",
            "ArrowDown",
            "ArrowUp",
            "ArrowUp",
            "ArrowDown"
        ])
    );
    // Commands the engine would not carry out (Playwright derives scrolling,
    // clipboard, undo and word movement too), and commands that are not the
    // key's own encoding, are refused without delivering events.
    for (key, code, modifiers, commands) in [
        ("ArrowLeft", "ArrowLeft", 1, json!(["moveWordLeft"])),
        ("z", "KeyZ", 4, json!(["undo"])),
        ("ArrowRight", "ArrowRight", 0, json!(["moveLeft"])),
        ("c", "KeyC", 4, json!(["copy"])),
        ("ArrowRight", "ArrowRight", 0, json!(["moveRight", "selectAll"])),
        ("a", "KeyA", 0, json!(["selectAll"])),
        // Emacs-style aliases in Playwright's table: the engine's deletion and
        // caret defaults are keyed on Backspace/Delete/Arrow keys, not these.
        ("d", "KeyD", 2, json!(["deleteForward"])),
        ("h", "KeyH", 2, json!(["deleteBackward"])),
        ("b", "KeyB", 2, json!(["moveBackward"])),
        ("f", "KeyF", 2, json!(["moveForward"])),
        // Right shortcut and command, but a key the default action is not keyed on.
        ("x", "Backspace", 0, json!(["deleteBackward"])),
        // Escape's command on a key that is not Escape.
        ("q", "Escape", 0, json!(["cancelOperation"])),
        // Vertical commands only with their own key and modifiers.
        ("ArrowDown", "ArrowDown", 0, json!(["moveUp"])),
        ("ArrowDown", "ArrowDown", 4, json!(["moveToEndOfDocument"])),
        ("j", "KeyJ", 0, json!(["moveDown"])),
    ] {
        let r = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":"rawKeyDown","key":key,"code":code,"modifiers":modifiers,"commands":commands}),
        );
        assert_eq!(r["error"]["code"], -32000, "{key} {commands}: {r}");
    }
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"keys.length","returnByValue":true}))["result"]["result"]
            ["value"],
        10,
        "rejected commands deliver no events"
    );
    // The supported deletions keep their behavior with their own commands.
    h.page(
        "Runtime.evaluate",
        json!({"expression":"field.focus(); field.value = 'abcd'; field.setSelectionRange(2, 2)"}),
    );
    press(&mut h, "Backspace", "Backspace", 8, 0, json!(["deleteBackward"]));
    press(&mut h, "Delete", "Delete", 46, 8, json!(["deleteForward"]));
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"[field.value, field.selectionStart]","returnByValue":true}))
            ["result"]["result"]["value"],
        json!(["ad", 1])
    );
}

#[test]
fn layout_metrics_report_viewport_scroll_and_document_size() {
    let mut h = Harness::new();
    let metrics = json!({"width":800,"height":600,"deviceScaleFactor":2,"mobile":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", metrics)["result"], json!({}));
    h.page("Page.enable", json!({}));
    let load = |h: &mut Harness, url: &str| {
        let events = h.page_with_events("Page.navigate", json!({ "url": url }));
        if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
            until_event(h, "Page.loadEventFired");
        }
    };
    // Expected values are what headless Chromium (Playwright 1.59.1's build)
    // reports for the same page, viewport and scroll offsets at DPR 2.
    let expected = |x: u32, y: u32, w: u32, h: u32| {
        let layout = json!({"pageX":x,"pageY":y,"clientWidth":800,"clientHeight":600});
        let visual = json!({"offsetX":0,"offsetY":0,"pageX":x,"pageY":y,"clientWidth":800,"clientHeight":600,"scale":1,"zoom":1});
        let content = json!({"x":0,"y":0,"width":w,"height":h});
        json!({"layoutViewport":layout,"visualViewport":visual,"contentSize":content,
               "cssLayoutViewport":layout,"cssVisualViewport":visual,"cssContentSize":content})
    };
    // Sizes are f64 (`1500.0`); compare them as JSON numbers.
    let metrics = |h: &mut Harness| -> Value {
        let r = h.page("Page.getLayoutMetrics", json!({}));
        serde_json::from_str(&r["result"].to_string().replace(".0,", ",").replace(".0}", "}")).unwrap()
    };
    load(&mut h, "data:text/html,<!doctype html><body style=margin:0><div style=width:1500px;height:2500px></div>");
    assert_eq!(
        eval_value(&mut h, "[document.documentElement.scrollWidth, document.documentElement.scrollHeight]"),
        json!([1500, 2500])
    );
    assert_eq!(metrics(&mut h), expected(0, 0, 1500, 2500));
    assert_eq!(eval_value(&mut h, "scrollTo(100, 300); [scrollX, scrollY]"), json!([100, 300]));
    assert_eq!(metrics(&mut h), expected(100, 300, 1500, 2500));
    // Content smaller than the viewport reports the viewport size.
    load(&mut h, "data:text/html,<!doctype html><body>short");
    assert_eq!(metrics(&mut h), expected(0, 0, 800, 600));
    let r = h.page("Page.getLayoutMetrics", json!({"bogus":1}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

fn eval_value(h: &mut Harness, expression: &str) -> Value {
    let r = h.page("Runtime.evaluate", json!({"expression": expression, "returnByValue": true}));
    assert!(r["result"].get("exceptionDetails").is_none(), "{r}");
    r["result"]["result"]["value"].clone()
}

fn history(h: &mut Harness) -> (u64, Vec<(u64, String)>) {
    let r = h.page("Page.getNavigationHistory", json!({}));
    let entries = r["result"]["entries"].as_array().unwrap_or_else(|| panic!("{r}"));
    for e in entries {
        assert_eq!(e["userTypedURL"], e["url"], "{r}");
        assert_eq!(e["transitionType"], "other", "{r}");
    }
    let entries = entries.iter().map(|e| (e["id"].as_u64().unwrap(), e["url"].as_str().unwrap().to_string())).collect();
    (r["result"]["currentIndex"].as_u64().unwrap(), entries)
}

/// TOS W3-02 email-logs "restores text drafts on Back and Forward": Playwright
/// `page.goBack()`/`goForward()` read `Page.getNavigationHistory` and send
/// `Page.navigateToHistoryEntry`. A same-document traversal (an SPA router's
/// pushState entries) commits as `Page.navigatedWithinDocument` and fires
/// `popstate` with the entry state, which is what routers re-render from.
#[test]
fn navigation_history_traverses_same_document_entries_with_popstate() {
    let (address, server) = serve_html(1, "<!doctype html><p>history</p>");
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.run_for(50);
    server.join().unwrap();
    eval_value(
        &mut h,
        r#"globalThis.seen = [];
        addEventListener('popstate', e => seen.push(['popstate', location.pathname + location.hash, e.state]));
        addEventListener('hashchange', e => seen.push(['hashchange', new URL(e.oldURL).hash, new URL(e.newURL).hash]));
        history.pushState({n: 1}, '', '/one');
        history.pushState({n: 2}, '', '/two');
        location.hash = 'x';"#,
    );
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "seen"), json!([["hashchange", "", "#x"]]));
    let (index, entries) = history(&mut h);
    let urls: Vec<_> = entries.iter().map(|(_, u)| u.as_str()).collect();
    let base = format!("http://{address}");
    // The initial about:blank entry is kept (Chrome replaces it; see nav.rs).
    assert_eq!(
        urls,
        [
            "about:blank".to_string(),
            format!("{base}/start"),
            format!("{base}/one"),
            format!("{base}/two"),
            format!("{base}/two#x")
        ]
    );
    assert_eq!(index, 4);

    let error = h.page("Page.navigateToHistoryEntry", json!({"entryId": 999_999}));
    assert_eq!(error["error"]["code"], -32602, "{error}");

    let one = entries[2].0;
    let messages = h.page_with_events("Page.navigateToHistoryEntry", json!({"entryId": one}));
    assert!(messages.iter().any(|m| m["result"] == json!({})), "{messages:#?}");
    let mut messages = messages;
    messages.extend(h.run_for(50));
    let within = position(&messages, "navigatedWithinDocument", |m| m["method"] == "Page.navigatedWithinDocument");
    assert_eq!(messages[within]["params"]["url"], format!("{base}/one"));
    assert!(!messages.iter().any(|m| m["method"] == "Page.frameNavigated"), "same-document: {messages:#?}");
    assert_eq!(eval_value(&mut h, "seen.slice(1)"), json!([["popstate", "/one", {"n": 1}]]));
    assert_eq!(history(&mut h).0, 2);

    // Forward to the fragment entry of the same pushState entry: popstate (no
    // state), then hashchange.
    let fragment = entries[4].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": fragment}))["result"], json!({}));
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "location.href"), format!("{base}/two#x"));
    assert_eq!(history(&mut h).0, 4);
    let frame = h.page("Page.getFrameTree", json!({}));
    assert_eq!(frame["result"]["frameTree"]["frame"]["url"], format!("{base}/two#x"));
    // /one -> /two#x is not a fragment-only change: no hashchange.
    assert_eq!(eval_value(&mut h, "seen.slice(2)"), json!([["popstate", "/two#x", null]]));
    // history.back() over a fragment entry fires both events.
    eval_value(&mut h, "history.back()");
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "seen.slice(3)"), json!([["popstate", "/two", {"n": 2}], ["hashchange", "#x", ""]]));
}

/// Cross-document traversal re-fetches the entry (no bfcache) and commits as an
/// ordinary navigation (`Page.frameNavigated`) without adding an entry.
#[test]
fn navigation_history_traverses_across_documents() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    for body in ["first", "second"] {
        let r = h.page("Page.navigate", json!({"url": format!("data:text/html,<p>{body}</p>")}));
        assert!(r["result"].get("errorText").is_none(), "{r}");
    }
    let (index, entries) = history(&mut h);
    assert_eq!(index, 2);
    assert_eq!(entries.len(), 3);
    let first = entries[1].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": first}))["result"], json!({}));
    let messages = h.run_for(100);
    let navigated = position(&messages, "frameNavigated", |m| m["method"] == "Page.frameNavigated");
    assert_eq!(messages[navigated]["params"]["frame"]["url"], "data:text/html,<p>first</p>");
    assert_eq!(eval_value(&mut h, "document.body.textContent"), "first");
    let (index, after) = history(&mut h);
    assert_eq!(index, 1);
    assert_eq!(after, entries, "traversal keeps the entry list and ids");
    // Forward again.
    let second = entries[2].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": second}))["result"], json!({}));
    h.run_for(100);
    assert_eq!(eval_value(&mut h, "document.body.textContent"), "second");
    assert_eq!(history(&mut h).0, 2);

    // Page.reload keeps the entry (id and state); Page.navigate to the same
    // URL replaces it with a new entry.
    eval_value(&mut h, "history.replaceState({k: 1}, '')");
    let (_, before) = history(&mut h);
    assert!(h.page("Page.reload", json!({}))["result"].get("errorText").is_none());
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "JSON.stringify(history.state)"), r#"{"k":1}"#);
    assert_eq!(history(&mut h), (2, before.clone()));
    let r = h.page("Page.navigate", json!({"url": "data:text/html,<p>second</p>"}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "history.state"), Value::Null);
    let (index, after) = history(&mut h);
    assert_eq!((index, &after[..2], after[2].1.as_str()), (2, &before[..2], before[2].1.as_str()));
    assert_ne!(after[2].0, before[2].0, "a same-URL navigation is a new (replacing) entry");
}

/// Network and page events of one page as "method url" in emission order.
fn page_and_network_order(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| {
            let method = m["method"].as_str()?;
            let p = &m["params"];
            let url = p["request"]["url"]
                .as_str()
                .or(p["response"]["url"].as_str())
                .or(p["url"].as_str())
                .or(p["frame"]["url"].as_str())
                .unwrap_or("");
            (method.starts_with("Network.") || method.starts_with("Page.")).then(|| format!("{method} {url}"))
        })
        .collect()
}

fn index_of(order: &[String], prefix: &str, suffix: &str) -> usize {
    order
        .iter()
        .position(|m| m.starts_with(prefix) && m.ends_with(suffix))
        .unwrap_or_else(|| panic!("{prefix} …{suffix} missing in {order:#?}"))
}

/// Loads `/start` with Page and Network enabled, then evaluates `expression`
/// (which may keep the browser thread busy) and collects what follows.
fn events_around(expression: &str, responses: usize, settle_ms: u64) -> Vec<String> {
    let (address, server) = serve_html(responses, "<!doctype html><p>x</p>");
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Network.enable", json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.run_for(100);
    let mut messages = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
    );
    messages.extend(h.run_for(settle_ms));
    server.join().unwrap();
    page_and_network_order(&messages)
}

const BUSY_300MS: &str = "const t = Date.now(); while (Date.now() - t < 300) {}";

/// B7 credit-note shape (W1 repro): a request is in flight, the page calls
/// pushState, and the response arrives while the browser thread is still
/// busy. Events keep occurrence order: navigatedWithinDocument, queued at
/// pushState, precedes the response published after it.
#[test]
fn push_state_precedes_network_events_published_after_it() {
    let order =
        events_around(&format!("fetch('/data'); history.pushState(null, '', '/pushed'); {BUSY_300MS} 1"), 2, 200);
    let pushed = index_of(&order, "Page.navigatedWithinDocument", "/pushed");
    assert!(pushed < index_of(&order, "Network.responseReceived", "/data"), "{order:#?}");
}

/// Responses received by the I/O thread during author JavaScript are not
/// delivered until a browser task can run. A pushState at the end of that
/// JavaScript therefore precedes response/data/terminal notifications, even
/// when the server has already replied. The microtask variants keep the same
/// boundary: promise/queueMicrotask work is part of the current task.
fn response_delivery_after_busy_push(start: &str, microtasks: bool) {
    let push = "history.pushState(null, '', '/pushed')";
    let expression = if microtasks {
        format!("{start}; queueMicrotask(() => {{ {BUSY_300MS} }}); queueMicrotask(() => {{ {push}; }}); 1")
    } else {
        format!("{start}; {BUSY_300MS} {push}; 1")
    };
    let order = events_around(&expression, 2, 200);
    let pushed = index_of(&order, "Page.navigatedWithinDocument", "/pushed");
    for method in ["Network.responseReceived", "Network.dataReceived", "Network.loadingFinished"] {
        let suffix = if method == "Network.responseReceived" { "/data" } else { "" };
        assert!(pushed < index_of(&order, method, suffix), "{order:#?}");
    }
}

#[test]
fn response_delivery_follows_synchronous_fetch_push() {
    response_delivery_after_busy_push("fetch('/data')", false);
}

#[test]
fn response_delivery_follows_microtask_fetch_push() {
    response_delivery_after_busy_push("fetch('/data')", true);
}

#[test]
fn response_delivery_follows_synchronous_xhr_push() {
    response_delivery_after_busy_push("const xhr = new XMLHttpRequest(); xhr.open('GET', '/data'); xhr.send()", false);
}

#[test]
fn response_delivery_follows_microtask_xhr_push() {
    response_delivery_after_busy_push("const xhr = new XMLHttpRequest(); xhr.open('GET', '/data'); xhr.send()", true);
}

/// Causal variant (W1 repro): a request issued after pushState is announced
/// after the navigation event.
#[test]
fn request_issued_after_push_state_follows_it() {
    let order =
        events_around(&format!("history.pushState(null, '', '/pushed'); fetch('/data'); {BUSY_300MS} 1"), 2, 200);
    let pushed = index_of(&order, "Page.navigatedWithinDocument", "/pushed");
    assert!(pushed < index_of(&order, "Network.requestWillBeSent", "/data"), "{order:#?}");
}

/// The other direction: a response that completed before pushState is
/// emitted before the navigation event, not held back behind it.
#[test]
fn network_events_published_before_push_state_precede_it() {
    let order = events_around(
        "fetch('/data').then(r => r.text()).then(() => { history.pushState(null, '', '/after'); return 1; })",
        2,
        100,
    );
    let pushed = index_of(&order, "Page.navigatedWithinDocument", "/after");
    assert!(index_of(&order, "Network.responseReceived", "/data") < pushed, "{order:#?}");
}

/// Network events moved in front of queued items are charged with lp-net's
/// `NetEvent::retained_bytes` and released when handled; beyond the limit the
/// connection closes rather than dropping events.
#[test]
fn queued_network_events_are_bounded_and_released() {
    for limit in [None, Some(1)] {
        let (address, server) = serve_html(2, "<!doctype html><p>x</p>");
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Network.enable", json!({}));
        let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
        assert!(r["result"].get("errorText").is_none(), "{r}");
        h.run_for(100);
        if let Some(limit) = limit {
            h.browser.host_mut().set_queued_network_byte_limit(limit);
        }
        // The response is published while the evaluation is busy, so it is
        // moved into the queue in front of the pushState event.
        let session = h.session.clone();
        h.send(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression": format!("fetch('/data'); {BUSY_300MS} history.pushState(null, '', '/pushed'); 1")}),
        );
        // Run the engine without reading frames: an exceeded connection has
        // no frames left to read.
        let deadline = Instant::now() + Duration::from_millis(600);
        while Instant::now() < deadline {
            let host = h.browser.host_mut();
            host.run_until_work(Some(Instant::now() + Duration::from_millis(10)), || false);
            host.flush();
        }
        server.join().unwrap();
        match limit {
            None => {
                assert!(!h.out.is_exceeded());
                assert_eq!(h.browser.host_mut().queued_network_bytes(), 0, "handled events release their charge");
            }
            Some(_) => assert!(h.out.is_exceeded(), "a queue beyond the byte limit closes the connection"),
        }
    }
}

/// Navigation start: the old document's network events published before the
/// navigation are emitted before it, and the new document's request after it
/// (previously the old document's events were held until after the navigation
/// cleared its captures).
#[test]
fn navigation_start_separates_old_and_new_document_network_events() {
    let order = events_around(&format!("fetch('/data'); {BUSY_300MS} location.href = '/next'; 1"), 3, 300);
    let navigating = index_of(&order, "Page.frameRequestedNavigation", "/next");
    assert!(index_of(&order, "Network.responseReceived", "/data") < navigating, "{order:#?}");
    assert!(navigating < index_of(&order, "Network.requestWillBeSent", "/next"), "{order:#?}");
}

/// Serves `/start` (HTML), then one more request whose response headers are
/// sent immediately and whose body follows after `gap_ms`.
fn serve_headers_then_body(gap_ms: u64) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            let body = if index == 0 { "<!doctype html><p>start</p>" } else { "slow body" };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.flush().unwrap();
            if index == 1 {
                std::thread::sleep(Duration::from_millis(gap_ms));
            }
            stream.write_all(body.as_bytes()).unwrap();
        }
    });
    (address, server)
}

/// Response events reach the client when the page's consumer exposes them
/// (the lp-webapi/lp-browser delivery frontier), in order with what the
/// page's script then does: headers before the script that sees the
/// Response, body and terminal before the script that sees the text.
fn frontier_order(expression: &str) {
    let (address, server) = serve_headers_then_body(300);
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Network.enable", json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.run_for(100);
    let mut messages = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
    );
    messages.extend(h.run_for(100));
    server.join().unwrap();
    let order = page_and_network_order(&messages);
    let response = index_of(&order, "Network.responseReceived", "/slow");
    let headers = index_of(&order, "Page.navigatedWithinDocument", "/headers");
    let data = index_of(&order, "Network.dataReceived", "");
    let finished = index_of(&order, "Network.loadingFinished", "");
    let body = index_of(&order, "Page.navigatedWithinDocument", "/body");
    assert!(response < headers, "{order:#?}");
    assert!(headers < data && data < body, "{order:#?}");
    assert!(headers < finished && finished < body, "{order:#?}");
}

#[test]
fn fetch_frontier_orders_headers_and_body_with_script() {
    frontier_order(
        "fetch('/slow').then(r => { history.pushState(null, '', '/headers'); return r.text(); })
            .then(() => { history.pushState(null, '', '/body'); return 1; })",
    );
}

#[test]
fn xhr_frontier_orders_headers_and_body_with_script() {
    frontier_order(
        "new Promise(resolve => {
            const xhr = new XMLHttpRequest();
            xhr.open('GET', '/slow');
            xhr.onreadystatechange = () => {
                if (xhr.readyState === 2) history.pushState(null, '', '/headers');
            };
            xhr.onload = () => { history.pushState(null, '', '/body'); resolve(1); };
            xhr.send();
        })",
    );
}

/// Serves `responses` requests and records each request head (lowercased).
fn record_requests(
    responses: usize,
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            log.lock().unwrap().push(String::from_utf8_lossy(&request).to_ascii_lowercase());
            let body = "<!doctype html><p>ok</p>";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        }
    });
    (address, server, seen)
}

/// TOS W3-19 positions-overview: `page.setExtraHTTPHeaders`. Behaviour
/// matches Chrome 147 (W3 probes): the whole map is replaced on each call, {}
/// clears it, headers apply to navigations and fetches only while the session
/// has Network enabled (they are kept while it is disabled), and invalid input
/// is rejected with Chrome's messages.
#[test]
fn extra_http_headers_replace_clear_and_follow_network_enable() {
    let (address, server, seen) = record_requests(6);
    let mut h = Harness::new();
    let base = format!("http://{address}");
    let fetch = |h: &mut Harness, path: &str| {
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": format!("fetch('{path}').then(r => r.status)"), "awaitPromise": true, "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"], 200, "{r}");
    };
    // Kept while Network is disabled, but not applied.
    let set = |h: &mut Harness, headers: Value| h.page("Network.setExtraHTTPHeaders", json!({"headers": headers}));
    assert_eq!(set(&mut h, json!({"X-A": "1", "Cache-Control": "no-cache"}))["result"], json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("{base}/nav-disabled")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.page("Network.enable", json!({}));
    let r = h.page("Page.navigate", json!({"url": format!("{base}/nav-enabled")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    fetch(&mut h, "/fetch-enabled");
    assert_eq!(set(&mut h, json!({"X-B": "2"}))["result"], json!({}));
    fetch(&mut h, "/fetch-replaced");
    assert_eq!(set(&mut h, json!({}))["result"], json!({}));
    fetch(&mut h, "/fetch-cleared");
    assert_eq!(set(&mut h, json!({"X-A": "1"}))["result"], json!({}));
    h.page("Network.disable", json!({}));
    fetch(&mut h, "/fetch-disabled-again");
    server.join().unwrap();
    let seen = seen.lock().unwrap();
    let has = |path: &str, header: &str| {
        let request =
            seen.iter().find(|r| r.starts_with(&format!("get {path} "))).unwrap_or_else(|| panic!("{path}: {seen:#?}"));
        request.contains(&format!("\r\n{header}\r\n"))
    };
    assert!(!has("/nav-disabled", "x-a: 1"));
    assert!(has("/nav-enabled", "x-a: 1") && has("/nav-enabled", "cache-control: no-cache"));
    assert!(has("/fetch-enabled", "x-a: 1"));
    assert!(has("/fetch-replaced", "x-b: 2") && !has("/fetch-replaced", "x-a: 1"));
    assert!(!has("/fetch-replaced", "cache-control: no-cache"));
    assert!(!has("/fetch-cleared", "x-b: 2"));
    assert!(!has("/fetch-disabled-again", "x-a: 1"));

    for (headers, message) in [
        (json!({"Bad Name": "x"}), "Invalid header name"),
        (json!({"X-V": "a\nb"}), "Invalid header value"),
        (json!({"X-N": 1}), "Invalid header value, string expected"),
        (json!({"X-N": null}), "Invalid header value, string expected"),
    ] {
        let r = h.page("Network.setExtraHTTPHeaders", json!({"headers": headers}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
        assert_eq!(r["error"]["message"], message, "{r}");
    }
}

/// Serves `responses` requests: `/asset*` as a cacheable script
/// (`public, max-age=3600`), anything else as HTML; records request heads.
fn serve_cacheable(
    responses: usize,
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let asset = request.starts_with("get /asset");
            log.lock().unwrap().push(request);
            let (kind, cache, body) = if asset {
                ("text/javascript", "Cache-Control: public, max-age=3600\r\n", "1;")
            } else {
                ("text/html", "", "<!doctype html><p>cache</p>")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{cache}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        }
    });
    (address, server, seen)
}

/// Network.setCacheDisabled / clearBrowserCache and cache-hit reporting, as
/// observed in Chrome 147 (W3 probes cache-controls.mjs,
/// cache-disabled-enable.mjs): a hit reports fromDiskCache with
/// encodedDataLength 0; disabling bypasses the cache (with no-cache request
/// headers) only while the session has Network enabled; clearing empties the
/// context's cache.
#[test]
fn cache_disabled_and_clear_browser_cache_follow_chrome() {
    let (address, server, seen) = serve_cacheable(4);
    let mut h = Harness::new();
    let r = h.page("Page.navigate", json!({"url": format!("http://{address}/start")}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.page("Network.enable", json!({}));
    let fetch = |h: &mut Harness| {
        let session = h.session.clone();
        let id = h.send(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression": "fetch('/asset').then(r => r.text())", "awaitPromise": true, "returnByValue": true}),
        );
        let mut messages = h.until_response(id);
        messages.extend(h.run_for(50));
        let reply = messages.iter().find(|m| m["id"] == id).unwrap();
        assert_eq!(reply["result"]["result"]["value"], "1;", "{reply}");
        let response = messages.iter().find(|m| m["method"] == "Network.responseReceived").cloned();
        let finished = messages.iter().find(|m| m["method"] == "Network.loadingFinished").cloned();
        (response, finished)
    };
    let requests = |seen: &std::sync::Arc<std::sync::Mutex<Vec<String>>>| seen.lock().unwrap().len();
    fetch(&mut h); // primes the cache
    assert_eq!(requests(&seen), 2);
    let (response, finished) = fetch(&mut h);
    assert_eq!(requests(&seen), 2, "hit");
    let (response, finished) = (response.expect("responseReceived"), finished.expect("loadingFinished"));
    assert_eq!(response["params"]["response"]["fromDiskCache"], true, "{response}");
    assert_eq!(response["params"]["response"]["encodedDataLength"], 0, "{response}");
    assert_eq!(finished["params"]["encodedDataLength"], 0, "{finished}");

    assert_eq!(h.page("Network.setCacheDisabled", json!({"cacheDisabled": true}))["result"], json!({}));
    fetch(&mut h);
    assert_eq!(requests(&seen), 3, "bypassed");
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert!(last.contains("\r\ncache-control: no-cache\r\n") && last.contains("\r\npragma: no-cache\r\n"), "{last}");
    h.page("Network.disable", json!({}));
    fetch(&mut h);
    assert_eq!(requests(&seen), 3, "kept but not applied while Network is disabled");
    h.page("Network.enable", json!({}));
    assert_eq!(h.page("Network.setCacheDisabled", json!({"cacheDisabled": false}))["result"], json!({}));
    fetch(&mut h);
    assert_eq!(requests(&seen), 3, "hit again");

    assert_eq!(h.page("Network.clearBrowserCache", json!({}))["result"], json!({}));
    fetch(&mut h);
    assert_eq!(requests(&seen), 4, "miss after clearing");
    server.join().unwrap();
}

/// Serves a document whose inline script fetches `/slow`, then `/slow` with
/// its body `delay_ms` after the headers.
fn serve_document_with_slow_fetch(delay_ms: u64) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            let (kind, body) = if index == 0 {
                ("text/html", "<!doctype html><p>idle</p><script>fetch('/slow').then(r => r.text())</script>")
            } else {
                ("text/plain", "slow")
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.flush().unwrap();
            if index == 1 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            stream.write_all(body.as_bytes()).unwrap();
        }
    });
    (address, server)
}

/// networkIdle follows Chrome (W3 probe network-idle.mjs): never while a
/// request of the document is in flight; almost-idle allows up to two. With
/// lp-browser's detector (PR #110) a fetch started by the page keeps
/// networkIdle back until it finishes plus the quiet window, though `load`
/// came long before.
#[test]
fn network_idle_waits_for_in_flight_requests() {
    let (address, server) = serve_document_with_slow_fetch(800);
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Network.enable", json!({}));
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled": true}));
    h.run_for(800); // about:blank's own milestones
    let mut messages = h.page_with_events("Page.navigate", json!({"url": format!("http://{address}/start")}));
    messages.extend(h.run_for(2000));
    server.join().unwrap();
    let order: Vec<String> = messages
        .iter()
        .filter_map(|m| match m["method"].as_str()? {
            "Page.lifecycleEvent" => Some(m["params"]["name"].as_str()?.to_owned()),
            "Network.loadingFinished" => Some("loadingFinished".to_owned()),
            _ => None,
        })
        .filter(|n| n != "init" && n != "commit")
        .collect();
    let at = |name: &str| order.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name}: {order:?}"));
    let slow_finished = order.iter().rposition(|n| n == "loadingFinished").expect("slow fetch finished");
    assert!(at("load") < slow_finished, "{order:?}");
    assert!(at("networkAlmostIdle") < slow_finished, "one request in flight is almost idle: {order:?}");
    assert!(slow_finished < at("networkIdle"), "{order:?}");
    assert_eq!(order.iter().filter(|n| *n == "networkIdle").count(), 1, "{order:?}");
}

#[path = "engine_cdp/frames.rs"]
mod frames;

#[test]
fn links_and_window_open_targeting_blank_open_paused_popups() {
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    h.page("Page.enable", json!({}));
    let page = "data:text/html,<a id=a href=\"data:text/html,<title>popped</title>\" target=_blank>a</a>";
    let seen = h.page_with_events("Page.navigate", json!({ "url": page }));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    h.page("Runtime.evaluate", json!({"expression":"document.getElementById('a').click()"}));

    // Chrome: a paused popup target in the opener's context, announced with its opener.
    let attached =
        h.until_event(|m| m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != opener);
    let info = &attached["params"]["targetInfo"];
    assert_eq!(info["openerId"], json!(opener));
    assert_eq!(info["canAccessOpener"], json!(false), "target=_blank links are noopener");
    assert_eq!(attached["params"]["waitingForDebugger"], json!(true));
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();

    // Uncommitted initial document until the client resumes the popup.
    let tree = h.call(Some(&popup), "Page.getFrameTree", json!({}));
    assert_eq!(tree["result"]["frameTree"]["frame"]["url"], json!(":"));
    h.call(Some(&popup), "Page.enable", json!({}));
    // Resuming starts the held navigation; its events may share the response's batch.
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let mut seen = h.until_response(id);
    let is_navigated = |m: &Value| m["method"] == "Page.frameNavigated" && m["sessionId"] == json!(popup);
    let navigated = match seen.iter().position(is_navigated) {
        Some(i) => seen.swap_remove(i),
        None => h.until_event(is_navigated),
    };
    assert_eq!(navigated["params"]["frame"]["url"], json!("data:text/html,<title>popped</title>"));

    // window.open with noopener opens a popup and returns null; opener access is refused explicitly.
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"String(window.open('data:text/html,second', '_blank', 'noopener'))","returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!("null"));
    let second = h.until_event(|m| {
        m["method"] == "Target.attachedToTarget"
            && m["params"]["targetInfo"]["targetId"] != opener
            && m["params"]["sessionId"] != json!(popup)
    });
    assert_eq!(second["params"]["targetInfo"]["openerId"], json!(opener));
    let r = h.page("Runtime.evaluate", json!({"expression":"window.open('data:text/html,x')"}));
    assert!(r["result"]["exceptionDetails"].is_object(), "scriptable popups are unsupported: {r}");
}

#[test]
fn tos_new_tab_activations_open_popups_with_their_opener() {
    // TOS opens new tabs three ways (TP-4231, TP-4217, TS-106): react-aria
    // onPress calling window.open(url, "_blank", "noopener,noreferrer") from
    // synthetic (untrusted) pointer/mouse events (DOM activation), a click on
    // a target=_blank link, and Enter on a focused target=_blank link
    // (Playwright focus() + press("Enter")). Playwright's `popup` and
    // context `page` events need a new target announced with openerId, as
    // Chrome does for noopener popups.
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    h.page("Page.enable", json!({}));
    let page = "data:text/html,<button id=b>View Position</button><a id=a href=\"data:text/html,partner\" target=_blank>Open in new tab</a>\
        <script>document.getElementById('b').addEventListener('click', () => { window.__r = String(window.open('data:text/html,master', '_blank', 'noopener,noreferrer')); });</script>";
    let seen = h.page_with_events("Page.navigate", json!({ "url": page }));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let mut popups: Vec<String> = Vec::new();
    let next_popup = |h: &mut Harness, popups: &mut Vec<String>, what: &str| {
        let known = popups.clone();
        let attached = h.until_event(|m| {
            m["method"] == "Target.attachedToTarget"
                && m["params"]["targetInfo"]["targetId"] != opener
                && !known.iter().any(|s| m["params"]["sessionId"] == json!(s))
        });
        assert_eq!(attached["params"]["targetInfo"]["openerId"], json!(opener), "{what}");
        assert_eq!(attached["params"]["targetInfo"]["canAccessOpener"], json!(false), "{what}");
        popups.push(attached["params"]["sessionId"].as_str().unwrap().to_owned());
    };

    // DOM activation: untrusted events, as TOS's E2E_DOM_ACTIVATION lane sends.
    h.page(
        "Runtime.evaluate",
        json!({"expression":"document.getElementById('b').dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true}))"}),
    );
    next_popup(&mut h, &mut popups, "window.open noopener,noreferrer from an untrusted click");
    let r = h.page("Runtime.evaluate", json!({"expression":"window.__r","returnByValue":true}));
    assert_eq!(r["result"]["result"]["value"], json!("null"), "noopener window.open returns null");

    // Enter on a focused target=_blank link.
    h.page("Runtime.evaluate", json!({"expression":"document.getElementById('a').focus()"}));
    h.page(
        "Input.dispatchKeyEvent",
        json!({"type":"keyDown","key":"Enter","code":"Enter","windowsVirtualKeyCode":13,"text":"\r","unmodifiedText":"\r"}),
    );
    h.page("Input.dispatchKeyEvent", json!({"type":"keyUp","key":"Enter","code":"Enter","windowsVirtualKeyCode":13}));
    next_popup(&mut h, &mut popups, "Enter on a target=_blank link");

    // The opener stays where it was.
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"location.href.startsWith('data:text/html,<button')","returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!(true));
}

#[test]
fn scroll_into_view_if_needed_reveals_a_node_relative_rect() {
    let mut h = Harness::new();
    let setup = h.page(
        "Runtime.evaluate",
        json!({"expression": "document.body.innerHTML = '<div id=t style=\"height:3000px;width:100px\">t</div>'; document.querySelector('#t')"}),
    );
    let object = setup["result"]["result"]["objectId"].clone();
    // Playwright sends `rect` for positional clicks; the area near the node's bottom must end up in view.
    let r = h.page(
        "DOM.scrollIntoViewIfNeeded",
        json!({"objectId": object, "rect": {"x": 10, "y": 2900, "width": 1, "height": 1}}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let visible = eval_value(
        &mut h,
        "(() => { const top = document.querySelector('#t').getBoundingClientRect().top + 2900; return top >= 0 && top + 1 <= innerHeight; })()",
    );
    assert_eq!(visible, json!(true));
}

#[test]
fn scroll_into_view_if_needed_leaves_a_visible_node_alone() {
    // Chromium's DOM.scrollIntoViewIfNeeded does not scroll a node that is
    // already in view. Playwright sends it before every click; start-aligning
    // the window instead fired a scroll that react-aria's close-on-scroll
    // treats as moving the trigger, so a combobox popover on any page taller
    // than the viewport never rendered.
    let mut h = Harness::new();
    // Flow geometry (TOS runs it): approximate client rects cannot tell what is in view.
    let page = h.browser.host().thread.browser.pages[0].main_frame.page_dom.as_ref().unwrap().clone();
    page.layout.borrow_mut().set_geometry(lp_dom::layout::Geometry::Flow);
    let setup = h.page(
        "Runtime.evaluate",
        json!({"expression": "document.body.innerHTML = '<div style=\"height:150px\"></div><button id=b>b</button><div style=\"height:3000px\"></div>'; \
            globalThis.scrolls = []; addEventListener('scroll', e => scrolls.push(e.target === document ? 'document' : e.target.id), true); \
            document.querySelector('#b')"}),
    );
    let object = setup["result"]["result"]["objectId"].clone();
    assert_eq!(eval_value(&mut h, "(() => { const r = document.querySelector('#b').getBoundingClientRect(); return r.top > 0 && r.bottom < innerHeight; })()"), json!(true));
    let r = h.page("DOM.scrollIntoViewIfNeeded", json!({ "objectId": object }));
    assert_eq!(r["result"], json!({}), "{r}");
    h.run_for(60);
    assert_eq!(eval_value(&mut h, "[scrollY, scrolls.join()]"), json!([0, ""]));
}

#[test]
fn scroll_events_from_scroll_into_view_wait_for_the_next_frame() {
    // Chromium fires the scroll event of DOM.scrollIntoViewIfNeeded (and of
    // scripted scrolls) in a later rendering update, after the commands that
    // follow it: Playwright's click scrolls the option into view and clicks
    // at once. TOS's @povio/ui autocomplete (react-aria) closes its popover on
    // scroll; delivered as an immediate task, the event closed it before the
    // click and left aria-expanded and ariaHideOutside behind (Customer
    // heading aria-hidden). Each scrolled target fires once per frame.
    let mut h = Harness::new();
    let setup = h.page(
        "Runtime.evaluate",
        json!({"expression": "document.body.innerHTML = '<div style=\"height:3000px\"></div><button id=b>b</button>'; \
            globalThis.scrolls = []; addEventListener('scroll', e => scrolls.push(e.target === document ? 'document' : e.target.id), true); \
            document.querySelector('#b')"}),
    );
    let object = setup["result"]["result"]["objectId"].clone();
    let r = h.page("DOM.scrollIntoViewIfNeeded", json!({ "objectId": object }));
    assert_eq!(r["result"], json!({}), "{r}");
    // The scroll itself is applied at once; its event is not delivered yet,
    // even after the few milliseconds before Playwright's next command.
    h.run_for(5);
    assert_eq!(eval_value(&mut h, "scrollY > 0"), json!(true));
    assert_eq!(eval_value(&mut h, "scrolls.join()"), json!(""), "no scroll event before the next command");
    // A second scroll of the same target before the frame: still one event.
    eval_value(&mut h, "scrollBy(0, -10); 1");
    h.run_for(60);
    assert_eq!(eval_value(&mut h, "scrolls.join()"), json!("document"), "one event at the next frame");
}

#[test]
fn input_hover_after_scroll_carries_the_previous_element_like_chromium() {
    // TOS HS field, measured in Chromium (W1 trace): with a span previously
    // hovered, scrolling an off-screen field into view and moving onto it
    // gives out on the span (relatedTarget = the field input) and over/enter
    // on the input (relatedTarget = the span); nothing fires for the scroll.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<span id="span" style="display:block;width:200px;height:20px">S</span><div style="height:2000px"></div><div id="field"><input id="hs" tabindex="-1" style="display:block;width:190px;height:24px"></div>';
      globalThis.log=[];
      for (const t of ['pointerover','pointerout','pointerenter','pointerleave','mouseover','mouseout','mouseenter','mouseleave'])
        document.addEventListener(t, e => { if (e.target.id) log.push(t + '@' + e.target.id + '<' + (e.relatedTarget?.id || e.relatedTarget?.nodeName || '-')) }, true);
      1
    "#,
    );
    let moved = |h: &mut Harness, id: &str| {
        let point = eval_value(h, &format!("(() => {{ const r = document.getElementById('{id}').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()"));
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":point[0],"y":point[1],"button":"none","buttons":0}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
    };
    moved(&mut h, "span");
    eval_value(&mut h, "log.length = 0; document.getElementById('hs').scrollIntoView(); 1");
    assert_eq!(eval_value(&mut h, "log.join(' ')"), json!(""), "scrolling alone dispatches nothing");
    moved(&mut h, "hs");
    assert_eq!(
        eval_value(&mut h, "log.join(' ')"),
        json!("pointerout@span<hs pointerleave@span<hs pointerover@hs<span pointerenter@field<span pointerenter@hs<span mouseout@span<hs mouseleave@span<hs mouseover@hs<span mouseenter@field<span mouseenter@hs<span")
    );
}

#[test]
fn select_then_insert_text_replaces_a_number_inputs_value_like_chromium() {
    // Playwright's fill() on input[type=number] is select(), focus(), then
    // Input.insertText. select() did nothing for number inputs (they have no
    // selection API), so the text was appended: TOS Projects split read
    // "10.75" for fill("0.75") over "1" and the Create Position button
    // stayed disabled. Chromium replaces the value (its queued `select`
    // event timing varies, so it is not asserted); the selection API still
    // reports null and throws.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        "document.body.innerHTML = '<input id=n type=number max=2 step=any value=1><input id=n2 type=number value=1.26>'; \
         globalThis.ev = []; for (const id of ['n', 'n2']) for (const type of ['select', 'beforeinput', 'input']) \
           document.getElementById(id).addEventListener(type, e => ev.push(id + ':' + type + '=' + e.target.value)); 1",
    );
    let fill = |h: &mut Harness, id: &str, text: &str| {
        eval_value(
            h,
            &format!("(() => {{ const e = document.getElementById('{id}'); e.select(); e.focus(); return 1; }})()"),
        );
        assert_eq!(h.page("Input.insertText", json!({ "text": text }))["result"], json!({}));
    };
    fill(&mut h, "n", "0.75");
    fill(&mut h, "n2", "1.25");
    assert_eq!(
        eval_value(
            &mut h,
            "(() => { const n = document.getElementById('n'), n2 = document.getElementById('n2'); \
             return [n.value, n2.value, n.validity.valid, n.selectionStart, ev.filter(e => !e.includes(':select')).join(' ')]; })()"
        ),
        json!([
            "0.75",
            "1.25",
            true,
            null,
            "n:beforeinput=1 n:input=0.75 n2:beforeinput=1.26 n2:input=1.25"
        ])
    );
    // The selection API stays unavailable on number inputs.
    assert_eq!(
        eval_value(
            &mut h,
            "(() => { try { document.getElementById('n').setSelectionRange(0, 1); return 'ok'; } catch (e) { return e.name; } })()"
        ),
        json!("InvalidStateError")
    );
}

#[test]
fn disabling_the_focused_control_blurs_it_like_chromium() {
    // Chromium blurs a focused control the moment it becomes disabled (by
    // attribute, property or a disabled fieldset): trusted blur and focusout
    // with no related target and the body active, synchronously, before the
    // change's MutationObserver callback. The engine kept the disabled input
    // focused, so react-aria synthesized its own blur and the TOS Quotes
    // Document drawer's FocusScope saw two blurs, reopening the Customer
    // combobox (heading left aria-hidden).
    let mut h = Harness::new();
    let probe = |mode: &str| {
        format!(
            "new Promise(res => {{ \
               document.body.innerHTML = '<fieldset id=fs><input id=i></fieldset>'; \
               const i = document.getElementById('i'), fs = document.getElementById('fs'), log = []; \
               const d = n => n ? (n.id || n.nodeName) : null; \
               for (const t of ['blur', 'focusout', 'focus', 'focusin']) document.addEventListener(t, e => \
                 log.push(`${{t}}@${{d(e.target)}} trusted=${{e.isTrusted}} rel=${{d(e.relatedTarget)}} active=${{d(document.activeElement)}}`), true); \
               i.focus(); log.length = 0; \
               new MutationObserver(() => log.push(`MO active=${{d(document.activeElement)}}`)).observe('{mode}' === 'fieldset' ? fs : i, {{ attributes: true }}); \
               if ('{mode}' === 'attribute') i.setAttribute('disabled', ''); \
               else if ('{mode}' === 'property') i.disabled = true; \
               else fs.disabled = true; \
               log.push(`sync active=${{d(document.activeElement)}}`); \
               setTimeout(() => {{ log.push(`task active=${{d(document.activeElement)}}`); res(log.join(' | ')); }}, 0); \
             }})"
        )
    };
    for mode in ["attribute", "property", "fieldset"] {
        let r =
            h.page("Runtime.evaluate", json!({"expression": probe(mode), "awaitPromise": true, "returnByValue": true}));
        assert_eq!(
            r["result"]["result"]["value"],
            "blur@i trusted=true rel=null active=BODY | focusout@i trusted=true rel=null active=BODY | sync active=BODY | MO active=BODY | task active=BODY",
            "{mode}: {r}"
        );
    }
    // Disabling an unfocused control, or re-enabling, moves no focus.
    assert_eq!(
        eval_value(
            &mut h,
            "(() => { document.body.innerHTML = '<input id=a><input id=b>'; const a = document.getElementById('a'), b = document.getElementById('b'); \
             a.focus(); b.disabled = true; b.disabled = false; return document.activeElement.id; })()"
        ),
        json!("a")
    );
}

#[test]
fn disabled_writes_elsewhere_move_no_focus_like_chromium() {
    // Chromium blurs only the focused control that a `disabled` write
    // disables. A write on another control or on a fieldset that does not
    // contain the focused control fires nothing, and neither does any write
    // after the focused control was removed. The engine blurred whatever
    // focused element was not focusable after any such write, so a detached
    // one silently lost focus at the next unrelated write (TOS Quote Finances
    // kept Select all checked after a tab round trip with that change).
    let mut h = Harness::new();
    let probe = |html: &str, action: &str| {
        format!(
            "new Promise(res => {{ \
               document.body.innerHTML = '{html}'; \
               const $ = id => document.getElementById(id), i = $('i'), other = $('other'), wrap = $('wrap'), fs2 = $('fs2'), log = []; \
               const d = n => n ? (n.id || n.nodeName) : null; \
               const mark = what => log.push(`${{what}} active=${{d(document.activeElement)}}`); \
               for (const t of ['blur', 'focusout', 'focus', 'focusin']) document.addEventListener(t, e => \
                 log.push(`${{t}}@${{d(e.target)}} trusted=${{e.isTrusted}}`), true); \
               i.focus(); log.length = 0; \
               new MutationObserver(() => mark('MO')).observe(document, {{ attributes: true, childList: true, subtree: true }}); \
               {action}; \
               mark('sync'); \
               setTimeout(() => {{ mark('task'); res(log.join(' | ')); }}, 0); \
             }})"
        )
    };
    let unrelated = "<input id=i><button id=other>o</button>";
    let fieldsets = "<fieldset id=fs1><input id=i></fieldset><fieldset id=fs2><button id=other>o</button></fieldset>";
    for (name, html, action) in [
        ("property", unrelated, "other.disabled = true"),
        ("attribute", unrelated, "other.setAttribute('disabled', '')"),
        ("toggle", unrelated, "other.toggleAttribute('disabled', true)"),
        ("fieldset", fieldsets, "fs2.disabled = true"),
    ] {
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": probe(html, action), "awaitPromise": true, "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"], "sync active=i | MO active=i | task active=i", "{name}: {r}");
    }
    // Chromium blurs a removed focused control at removal (the engine does
    // not yet); either way a later `disabled` write, unrelated or on the
    // detached control, fires nothing and keeps the active element.
    for (name, html, action) in [
        ("unrelated", "<div id=wrap><input id=i></div><button id=other>o</button>", "other.disabled = true"),
        ("detached", "<div id=wrap><input id=i></div>", "i.disabled = true"),
    ] {
        let action = format!(
            "wrap.remove(); const removed = document.activeElement; log.length = 0; {action}; \
             log.push(`same=${{document.activeElement === removed}}`)"
        );
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": probe(html, &action), "awaitPromise": true, "returnByValue": true}),
        );
        let v = r["result"]["result"]["value"].as_str().unwrap_or_default().to_owned();
        assert!(v.starts_with("same=true | sync active="), "{name}: {r}");
    }
}

/// Loads `body` (flow geometry, 20px/30px text), logs mouse/selection events
/// and returns the page.
fn selection_page(h: &mut Harness, body: &str, extra: &str) {
    let page = h.browser.host().thread.browser.pages[0].main_frame.page_dom.as_ref().unwrap().clone();
    page.layout.borrow_mut().set_geometry(lp_dom::layout::Geometry::Flow);
    let setup = format!(
        "(() => {{ document.head.innerHTML = '<style>body{{margin:0;font-size:20px;line-height:30px}} td{{padding:0 20px 0 0}}</style>'; \
           document.body.innerHTML = {body:?}; globalThis.log = []; globalThis.cancel = {{}}; \
           if (!globalThis.d) {{ \
             globalThis.d = n => n ? (n.nodeType === 3 ? `#text(${{n.parentNode.id || n.parentNode.nodeName}})` : (n.id || n.nodeName)) : null; \
             for (const t of ['mousedown', 'selectstart', 'mouseup', 'click']) document.addEventListener(t, e => {{ log.push(`${{t}}@${{d(e.target)}}`); if (cancel[t]) e.preventDefault(); }}, true); \
             document.addEventListener('selectionchange', () => {{ if (log[log.length - 1] !== 'selectionchange') log.push('selectionchange'); }}); \
           }} \
           {extra} }})()"
    );
    eval_value(h, &setup);
}

/// A point at the vertical middle of `sel`'s text, `dx` px from its left
/// edge (negative: from its right edge).
fn text_point(h: &mut Harness, sel: &str, dx: f64) -> Value {
    eval_value(
        h,
        &format!(
            "(() => {{ const el = document.querySelector({sel:?}); const t = [...el.childNodes].find(n => n.nodeType === 3) || el; \
               const r = document.createRange(); r.selectNodeContents(t); const b = r.getBoundingClientRect(); \
               return [{dx} < 0 ? b.right + {dx} : b.left + {dx}, b.top + b.height / 2]; }})()"
        ),
    )
}

fn drag_select(h: &mut Harness, from: &Value, to: &Value) -> String {
    mouse(h, "mousePressed", from, 0.0, 1);
    for k in 1..=12 {
        let f = k as f64 / 12.0;
        let at = json!([
            from[0].as_f64().unwrap() + (to[0].as_f64().unwrap() - from[0].as_f64().unwrap()) * f,
            from[1].as_f64().unwrap() + (to[1].as_f64().unwrap() - from[1].as_f64().unwrap()) * f
        ]);
        mouse(h, "mouseMoved", &at, 0.0, 1);
    }
    mouse(h, "mouseReleased", to, 0.0, 0);
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": "new Promise(res => setTimeout(() => { const s = getSelection(); \
            res(`str=${JSON.stringify(s.toString())} type=${s.type} anchor=${d(s.anchorNode)}:${s.anchorOffset} focus=${d(s.focusNode)}:${s.focusOffset} active=${d(document.activeElement)} changed=${log.includes('selectionchange')} log=${log.filter(e => e !== 'selectionchange').join(',')}`); }, 20))",
            "awaitPromise": true, "returnByValue": true}),
    );
    r["result"]["result"]["value"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn native_drag_selection_follows_chromium_rules() {
    // Chromium selects text under a primary-button drag. The engine never
    // did, so TOS table-viewport's native cross-row selection stayed empty.
    // Chromium (800x600, measured): a press on text fires selectstart at the
    // text node and collapses the selection there; moves extend it (anchor
    // at the press, focus under the pointer, backwards too). A canceled
    // mousedown or selectstart (refired on every move) selects nothing;
    // user-select: none on the element or an ancestor selects nothing
    // (user-select: text inside it does); a press on a button focuses it and
    // selects nothing; a drag into user-select: none content stops at the end
    // of the selectable text. The engine measures text with its own advance
    // table, so points here are at text edges. (This harness runs the queued
    // selectionchange task after the whole gesture; over CDP it comes after
    // selectstart, as in Chromium, so only its firing is asserted.)
    let mut h = Harness::new();
    let p = "<p id=p style=\"margin:0\">abcdefghij</p><p id=q style=\"margin:0\">klmnopqrst</p>";
    let whole = "str=\"abcdefghij\" type=Range anchor=#text(p):0 focus=#text(p):10 active=BODY changed=true log=mousedown@p,selectstart@#text(p),mouseup@p,click@p";
    let run = |h: &mut Harness, body: &str, extra: &str, from: (&str, f64), to: (&str, f64)| {
        selection_page(h, body, extra);
        let (a, b) = (text_point(h, from.0, from.1), text_point(h, to.0, to.1));
        drag_select(h, &a, &b)
    };
    assert_eq!(run(&mut h, p, "", ("#p", 1.0), ("#p", -0.5)), whole, "whole line");
    assert_eq!(
        run(&mut h, p, "", ("#p", -1.0), ("#p", 1.0)),
        "str=\"abcdefghij\" type=Range anchor=#text(p):10 focus=#text(p):0 active=BODY changed=true log=mousedown@p,selectstart@#text(p),mouseup@p,click@p",
        "backwards"
    );
    let r = run(&mut h, p, "", ("#p", 1.0), ("#q", -1.0));
    assert!(
        r.contains("anchor=#text(p):0 focus=#text(q):10")
            && r.contains("log=mousedown@p,selectstart@#text(p),mouseup@q"),
        "across lines: {r}"
    );
    let none =
        "str=\"\" type=None anchor=null:0 focus=null:0 active=BODY changed=false log=mousedown@p,mouseup@p,click@p";
    assert_eq!(run(&mut h, p, "cancel.mousedown = true", ("#p", 1.0), ("#p", -1.0)), none, "mousedown canceled");
    let r = run(&mut h, p, "cancel.selectstart = true", ("#p", 1.0), ("#p", -1.0));
    assert!(r.starts_with("str=\"\" type=None") && r.matches("selectstart@").count() > 1, "selectstart canceled: {r}");
    let us = "<p id=p style=\"margin:0;user-select:none\">abcdefghij</p>";
    assert_eq!(run(&mut h, us, "", ("#p", 1.0), ("#p", -1.0)), none, "user-select none");
    let anc = "<div style=\"user-select:none\"><p id=p style=\"margin:0\">abcdefghij</p></div>";
    assert_eq!(run(&mut h, anc, "", ("#p", 1.0), ("#p", -1.0)), none, "user-select none ancestor");
    let text = "<div style=\"user-select:none\"><p id=p style=\"margin:0;user-select:text\">abcdefghij</p></div>";
    assert_eq!(run(&mut h, text, "", ("#p", 1.0), ("#p", -1.0)), whole, "user-select text inside none");
    let btn = "<button id=btn style=\"font:inherit\">button text</button><p id=p style=\"margin:0\">abcdefghij</p>";
    let r = run(&mut h, btn, "", ("#btn", 1.0), ("#p", -1.0));
    assert!(
        r.starts_with("str=\"\" type=None") && r.contains("active=btn") && !r.contains("selectstart"),
        "drag on button: {r}"
    );
    let into = "<p id=p style=\"margin:0\">abcdefghij</p><p id=q style=\"margin:0;user-select:none\">klmnopqrst</p>";
    let r = run(&mut h, into, "", ("#p", 1.0), ("#q", -1.0));
    assert!(
        r.starts_with("str=\"abcdefghij\" type=Range anchor=#text(p):0 focus=#text(p):10"),
        "into none region: {r}"
    );
    let table = "<table id=t style=\"border-spacing:0\"><tr><td id=a0>Alpha one</td><td id=b0>Beta one</td></tr><tr><td id=a1>Alpha two</td><td id=b1>Beta two</td></tr></table>";
    let r = run(&mut h, table, "", ("#a0", 1.0), ("#b1", -1.0));
    assert!(
        r.contains("Alpha one") && r.contains("Alpha two") && r.contains("anchor=#text(a0):0"),
        "table cross-row: {r}"
    );
    // A plain click collapses the selection there (the Selection API's own
    // selection included), firing selectstart.
    selection_page(
        &mut h,
        p,
        "const t = document.getElementById('p').firstChild; getSelection().setBaseAndExtent(t, 1, t, 4);",
    );
    let at = text_point(&mut h, "#q", 1.0);
    mouse(&mut h, "mousePressed", &at, 0.0, 1);
    mouse(&mut h, "mouseReleased", &at, 0.0, 0);
    let r = eval_value(&mut h, "(() => { const s = getSelection(); return `${s.type} ${d(s.anchorNode)}:${s.anchorOffset} ${log.filter(e => e !== 'selectionchange').join(',')}`; })()");
    assert_eq!(r, json!("Caret #text(q):0 mousedown@q,selectstart@#text(q),mouseup@q,click@q"), "click collapses");
}
