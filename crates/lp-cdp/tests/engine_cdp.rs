//! CDP over the real engine (lp-browser + lp-js on JavaScriptCore), driven
//! in-process with the message sequence Playwright 1.59 uses. Only
//! about:blank can load until document loading is integrated.

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
        let (out, rx) = Outbox::channel(lp_cdp::outbox::DEFAULT_OUTBOUND_BUDGET);
        let host = EngineHost::new(out.clone()).expect("JavaScriptCore engine");
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
    // Real network (integration): nothing listens on port 1.
    assert!(r["result"]["errorText"].as_str().unwrap().contains("ERR_CONNECTION_REFUSED"), "explicit failure: {r}");
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"location === undefined ? 'no-location' : 'x'", "returnByValue": true}),
    );
    assert!(r.get("result").is_some(), "page still usable after a failed navigation: {r}");
}

#[test]
fn unsupported_commands_fail_explicitly() {
    let mut h = Harness::new();
    for method in
        ["DOM.getDocument", "Input.dispatchMouseEvent", "Fetch.enable", "Page.captureScreenshot", "Runtime.addBinding"]
    {
        let r = h.page(method, json!({}));
        assert_eq!(r["error"]["code"], -32000, "{method}: {r}");
    }
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
