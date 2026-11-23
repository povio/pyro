//! CDP over the real engine (pyro-browser + pyro-js on JavaScriptCore), driven
//! in-process with the message sequence Playwright 1.59 uses, including
//! HTTP/data navigation, Network/Fetch and per-context storage.

use std::time::{Duration, Instant};

use pyro_cdp::engine::EngineHost;
use pyro_cdp::outbox::{Outbound, OutboundRx, Outbox};
use pyro_cdp::target::Browser;
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

    fn with_budget(budget: Option<pyro_cdp::network_protocol::BodyBudget>) -> Harness {
        Self::build(budget, None)
    }

    fn with_handle_limit(limit: usize) -> Harness {
        Self::build(None, Some(limit))
    }

    fn build(budget: Option<pyro_cdp::network_protocol::BodyBudget>, handle_limit: Option<usize>) -> Harness {
        let (out, rx) = Outbox::channel(pyro_cdp::outbox::DEFAULT_OUTBOUND_BUDGET);
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
fn by_value_results_keep_key_order_as_on_the_wire() {
    let mut h = Harness::new();
    let session = h.session.clone();
    h.drain();
    let id = h.send(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression":"({b: 1, a: [3, 1], c: {z: 1e21, y: 'q', x: null}, \"\": true})","returnByValue":true}),
    );
    let mut texts = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !texts.iter().any(|t: &String| t.contains(&format!("\"id\":{id},"))) {
        assert!(Instant::now() < deadline, "no response; saw {texts:?}");
        while let Some(Outbound::Frame(f)) = h.rx.try_recv() {
            let bytes = f.as_ref();
            let header = match bytes[1] {
                126 => 4,
                127 => 10,
                _ => 2,
            };
            texts.push(String::from_utf8(bytes[header..].to_vec()).unwrap());
        }
        h.browser.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        h.browser.host_mut().flush();
    }
    // Insertion order, like Chrome (JSON.stringify's text, numbers included).
    let reply = texts.iter().find(|t| t.contains(&format!("\"id\":{id},"))).unwrap();
    assert!(reply.contains(r#""value":{"b":1,"a":[3,1],"c":{"z":1e+21,"y":"q","x":null},"":true}"#), "{reply}");
}

/// A double crosses CDP unchanged, as with Chrome, in results
/// (`Runtime.evaluate` by value, `Pyro.run` `eval` reads) and in arguments
/// (`Runtime.callFunctionOn`, `Pyro.run` `eval` `arg`), checked against
/// `String(x)` in the page. JSON numbers the engine parses must round
/// correctly (serde_json's `float_roundtrip`, set in the workspace
/// manifest): without it 0.9591810769706007 read as 0.9591810769706008,
/// and about one random double in ten was one ulp off.
#[test]
fn doubles_cross_cdp_unchanged() {
    let mut h = Harness::new();
    let setup = "globalThis.doubles = [0.9591810769706007, 0.011425316439999667, 0.11370447987098531, \
        0.9333951639905637, 123456.78901234567, -0.1, 1e21, 1e-7, 5e-324, 2.2250738585072014e-308, \
        1.7976931348623157e308].concat(Array.from({length: 1000}, Math.random)); doubles.map(String)";
    let r = h.page("Runtime.evaluate", json!({"expression": setup, "returnByValue": true}));
    let texts: Vec<String> = r["result"]["result"]["value"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .iter()
        .map(|t| t.as_str().unwrap().to_owned())
        .collect();
    // The double each text names (`str::parse` rounds correctly).
    let doubles: Vec<f64> = texts.iter().map(|t| t.parse().unwrap()).collect();
    let same = |what: &str, got: &[Value]| {
        assert_eq!(got.len(), texts.len(), "{what}");
        for ((got, want), text) in got.iter().zip(&doubles).zip(&texts) {
            assert_eq!(got.as_f64(), Some(*want), "{what}: {got} for the page's {text}");
        }
    };

    let scalars: Vec<Value> = (0..texts.len())
        .map(|i| {
            let r = h.page("Runtime.evaluate", json!({"expression": format!("doubles[{i}]"), "returnByValue": true}));
            r["result"]["result"]["value"].clone()
        })
        .collect();
    same("Runtime.evaluate number", &scalars);
    let r = h.page("Runtime.evaluate", json!({"expression": "doubles", "returnByValue": true}));
    same("Runtime.evaluate array", r["result"]["result"]["value"].as_array().unwrap_or_else(|| panic!("{r}")));

    let mut steps: Vec<Value> =
        (0..texts.len()).map(|i| json!({"eval": {"expression": format!("doubles[{i}]")}})).collect();
    steps.push(json!({"eval": {"expression": "doubles", "as": "all"}}));
    steps.push(json!({"eval": {"expression": "(xs) => xs.map(String)", "call": true, "arg": doubles, "as": "texts"}}));
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    same("Pyro.run eval number", &results[..texts.len()]);
    same("Pyro.run eval array", r["result"]["reads"]["all"].as_array().unwrap_or_else(|| panic!("{r}")));
    assert_eq!(r["result"]["reads"]["texts"], json!(texts), "Pyro.run eval arg");

    let global = h.page("Runtime.evaluate", json!({"expression": "globalThis"}));
    let global = global["result"]["result"]["objectId"].as_str().unwrap().to_owned();
    for (want, text) in doubles.iter().zip(&texts) {
        let r = h.page(
            "Runtime.callFunctionOn",
            json!({"functionDeclaration": "function(x) { return String(x); }", "objectId": global,
                "arguments": [{"value": want}], "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"], text.as_str(), "Runtime.callFunctionOn argument");
    }
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
        // With the `render` feature it renders (see `mod screenshot`).
        #[cfg(not(feature = "render"))]
        "Page.captureScreenshot",
    ] {
        let r = h.page(method, json!({}));
        assert_eq!(r["error"]["code"], -32000, "{method}: {r}");
    }
}

#[test]
fn locale_override_sets_languages() {
    let mut h = Harness::new();
    let langs = |h: &mut Harness| {
        let r = h.page("Runtime.evaluate", json!({"expression":"navigator.languages.join()","returnByValue":true}));
        r["result"]["result"]["value"].as_str().unwrap().to_owned()
    };
    let default = langs(&mut h);
    let r = h.page("Emulation.setLocaleOverride", json!({"locale":"de-DE"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(langs(&mut h), "de-DE");
    // An explicit acceptLanguage wins over the locale.
    h.page("Emulation.setUserAgentOverride", json!({"userAgent":"", "acceptLanguage":"fr-FR,fr"}));
    assert_eq!(langs(&mut h), "fr-FR,fr");
    h.page("Emulation.setUserAgentOverride", json!({"userAgent":""}));
    assert_eq!(langs(&mut h), "de-DE", "locale applies again once the explicit override is cleared");
    let r = h.page("Emulation.setLocaleOverride", json!({}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(langs(&mut h), default, "absent locale resets");
    let r = h.page("Emulation.setLocaleOverride", json!({"locale":"de DE"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

#[test]
fn timezone_override_applies_to_date_intl_and_the_whole_connection() {
    let mut h = Harness::new();
    if !pyro_js::sys::SOURCE_BUILD {
        // The system JavaScriptCore has no per-VM zone: refused, not faked.
        let r = h.page("Emulation.setTimezoneOverride", json!({"timezoneId":"Asia/Tokyo"}));
        assert_eq!(r["error"]["code"], -32000, "{r}");
        assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":""}))["result"], json!({}));
        return;
    }
    let zone = |h: &mut Harness| eval_value(h, "Intl.DateTimeFormat().resolvedOptions().timeZone");
    let offset = |h: &mut Harness, when: &str| eval_value(h, &format!("new Date('{when}').getTimezoneOffset()"));
    let host = zone(&mut h);

    // An old Date object, with its local time and the default formats already cached.
    eval_value(&mut h, "globalThis.old = new Date(Date.UTC(2024, 0, 15, 12)); old.getHours(); old.toLocaleString(); 0");
    for (id, winter, summer, hours) in [
        ("Asia/Tokyo", -540, -540, 21),
        ("Europe/Ljubljana", -60, -120, 13),
        ("America/New_York", 300, 240, 7),
        ("UTC", 0, 0, 12),
    ] {
        let r = h.page("Emulation.setTimezoneOverride", json!({"timezoneId": id}));
        assert_eq!(r["result"], json!({}), "{id}: {r}");
        assert_eq!(zone(&mut h), json!(id));
        assert_eq!(offset(&mut h, "2024-01-15T12:00:00Z"), json!(winter), "{id}");
        assert_eq!(offset(&mut h, "2024-07-15T12:00:00Z"), json!(summer), "{id}");
        assert_eq!(eval_value(&mut h, "old.getHours()"), json!(hours), "{id}: a cached local time goes stale");
        assert_eq!(
            eval_value(&mut h, "old.toLocaleString() === new Intl.DateTimeFormat(undefined, {year:'numeric', month:'numeric', day:'numeric', hour:'numeric', minute:'numeric', second:'numeric'}).format(old)"),
            json!(true),
            "{id}: the default format follows"
        );
        assert_eq!(
            eval_value(&mut h, "old.toLocaleTimeString('en-US', {hour:'numeric', hour12:false})"),
            json!(format!("{hours:02}")),
            "{id}"
        );
    }

    // IDs are checked against the zones Intl knows, and a bad one changes nothing.
    for id in ["Europe/Atlantis", "Europe//Paris", "../etc", "Europe/", " UTC", "UTC\u{0}x", "Ljubljana"] {
        let r = h.page("Emulation.setTimezoneOverride", json!({"timezoneId": id}));
        assert_eq!(r["error"]["code"], -32602, "{id}: {r}");
        assert!(r["error"]["message"].as_str().unwrap().starts_with("Invalid timezone id"), "{id}: {r}");
        assert_eq!(zone(&mut h), json!("UTC"));
    }

    // Reset; then the zone holds across a new page and against a different one.
    assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":""}))["result"], json!({}));
    assert_eq!(zone(&mut h), host);
    assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":"Asia/Tokyo"}))["result"], json!({}));
    let first = h.session.clone();
    let id = h.send(None, "Target.createTarget", json!({"url": "about:blank"}));
    let attached = h.until_response(id).into_iter().find(|m| m["method"] == "Target.attachedToTarget").unwrap();
    let target = attached["params"]["targetInfo"]["targetId"].as_str().unwrap().to_owned();
    h.session = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(zone(&mut h), json!("Asia/Tokyo"), "one VM, one zone");
    let r = h.page("Emulation.setTimezoneOverride", json!({"timezoneId":"Europe/Paris"}));
    assert_eq!(r["error"]["code"], -32000, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().starts_with("Cannot set timezone Europe/Paris"), "{r}");
    assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":"Asia/Tokyo"}))["result"], json!({}));

    // The zone stays while one page holds it, and goes with the last.
    h.session = first;
    assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":""}))["result"], json!({}));
    assert_eq!(zone(&mut h), json!("Asia/Tokyo"));
    h.call(None, "Target.closeTarget", json!({"targetId": target}));
    assert_eq!(zone(&mut h), host);
    assert_eq!(h.page("Emulation.setTimezoneOverride", json!({"timezoneId":"Europe/Paris"}))["result"], json!({}));
    assert_eq!(zone(&mut h), json!("Europe/Paris"));
}

#[test]
fn security_ignores_certificate_errors_on_page_and_browser_sessions() {
    let mut h = Harness::new();
    for (method, params) in [
        ("Security.enable", json!({})),
        ("Security.setIgnoreCertificateErrors", json!({"ignore": true})),
        ("Security.setIgnoreCertificateErrors", json!({"ignore": false})),
        ("Security.disable", json!({})),
    ] {
        let r = h.page(method, params.clone());
        assert_eq!(r["result"], json!({}), "page {method}: {r}");
        let r = h.call(None, method, params);
        assert_eq!(r["result"], json!({}), "browser {method}: {r}");
    }
    let r = h.call(None, "Security.setIgnoreCertificateErrors", json!({}));
    assert_eq!(r["error"]["code"], -32602, "ignore is required: {r}");
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
                let gc = pyro_js::sys::spi::get().synchronous_gc_for_debugging.expect("GC SPI");
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

    /// Runs until the reply to `id` arrives or `ms` elapse; returns everything seen.
    /// A condition-based, explicitly bounded wait: callers assert the reply's exact payload.
    fn until_reply(&mut self, id: u64, ms: u64) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let host = self.browser.host_mut();
            host.run_until_work(Some(Instant::now() + Duration::from_millis(5)), || false);
            host.flush();
            seen.extend(self.drain());
            if seen.iter().any(|m| m["id"] == id) {
                break;
            }
        }
        seen
    }

    fn pending_replies(&self) -> usize {
        self.browser.host().pending_awaited_replies()
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

const CONTEXT_DESTROYED: &str = "Execution context was destroyed.";

/// A never-settling promise nobody references is garbage; collecting it must
/// not lose the evaluation's reply: navigation still answers it, exactly once.
#[test]
fn never_settling_awaited_evaluation_is_answered_after_gc_and_navigation() {
    let mut h = Harness::new();
    h.install_gc();
    let session = h.session.clone();
    let id =
        h.send(Some(&session), "Runtime.evaluate", json!({"expression":"new Promise(() => {})","awaitPromise":true}));
    h.drain();
    assert_eq!(h.pending_replies(), 1);
    for _ in 0..3 {
        let r = h.page("Runtime.evaluate", json!({"expression":"reviewGc(); reviewGc(); 1","returnByValue":true}));
        assert_eq!(r["result"]["result"]["value"], 1, "{r}");
    }
    assert_eq!(h.pending_replies(), 1, "the pending reply must survive collection of its promise");
    let mut seen = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    seen.extend(h.until_reply(id, 2000));
    let replies: Vec<&Value> = seen.iter().filter(|m| m["id"] == id).collect();
    assert_eq!(replies.len(), 1, "exactly one reply: {:?}", methods(&seen));
    assert_eq!(replies[0]["error"]["message"], CONTEXT_DESTROYED);
    assert_eq!(replies[0]["error"]["code"], -32000);
    assert_eq!(replies[0]["sessionId"], session.as_str());
    assert_eq!(h.pending_replies(), 0);
    // Quiet after navigation: nothing else answers it later.
    let later = h.run_for(50);
    assert!(later.iter().all(|m| m["id"] != id), "{later:?}");
}

/// Settled awaited evaluations reply with their value and leave the table.
#[test]
fn settled_awaited_replies_leave_the_pending_table() {
    let mut h = Harness::new();
    for i in 0..5u32 {
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": format!("Promise.resolve({i})"), "awaitPromise": true, "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"], i, "{r}");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"Promise.reject(new Error('nope'))","awaitPromise":true,"returnByValue":true}),
    );
    assert_eq!(
        r["result"]["exceptionDetails"]["exception"]["description"].as_str().map(|d| d.starts_with("Error: nope")),
        Some(true),
        "{r}"
    );
    assert_eq!(h.pending_replies(), 0);
}

/// The table is bounded; context destruction empties it and new evaluations register again.
#[test]
fn pending_awaited_replies_are_bounded_and_freed_by_context_destruction() {
    let mut h = Harness::new();
    h.browser.host().set_pending_reply_limit(3);
    let session = h.session.clone();
    let ids: Vec<u64> = (0..3)
        .map(|_| {
            h.send(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression":"new Promise(() => {})","awaitPromise":true}),
            )
        })
        .collect();
    h.drain();
    assert_eq!(h.pending_replies(), 3);
    let r = h.page("Runtime.evaluate", json!({"expression":"new Promise(() => {})","awaitPromise":true}));
    assert_eq!(r["error"]["message"], "Too many pending awaitPromise evaluations", "{r}");
    assert_eq!(h.pending_replies(), 3);
    let mut seen = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    for id in &ids {
        if !seen.iter().any(|m| m["id"] == *id) {
            seen.extend(h.until_reply(*id, 2000));
        }
    }
    for id in &ids {
        let replies: Vec<&Value> = seen.iter().filter(|m| m["id"] == *id).collect();
        assert_eq!(replies.len(), 1, "reply {id}: {:?}", methods(&seen));
        assert_eq!(replies[0]["error"]["message"], CONTEXT_DESTROYED);
    }
    assert_eq!(h.pending_replies(), 0);
    let r =
        h.page("Runtime.evaluate", json!({"expression":"Promise.resolve(7)","awaitPromise":true,"returnByValue":true}));
    assert_eq!(r["result"]["result"]["value"], 7, "{r}");
}

/// Detaching a session cancels its pending replies silently and frees the table.
#[test]
fn detaching_a_session_cancels_its_pending_replies_and_frees_the_table() {
    let mut h = Harness::new();
    let a = h.attach_second_session();
    let id = h.send(Some(&a), "Runtime.evaluate", json!({"expression":"new Promise(() => {})","awaitPromise":true}));
    h.drain();
    assert_eq!(h.pending_replies(), 1);
    h.call(None, "Target.detachFromTarget", json!({"sessionId": a}));
    assert_eq!(h.pending_replies(), 0);
    let later = h.run_for(50);
    assert!(later.iter().all(|m| m["id"] != id), "reply sent to detached session: {later:?}");
    let seen = h.page_with_events("Page.navigate", json!({"url":"about:blank"}));
    assert!(seen.iter().all(|m| m["id"] != id), "cancelled reply answered after navigation: {seen:?}");
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
fn playwright_utility_world_is_announced_at_commit_and_made_by_its_first_evaluation() {
    // Playwright: an init script with no source names the utility world for
    // every document, `Page.createIsolatedWorld` makes it for the first one,
    // and after each navigation the world's `executionContextCreated` is
    // what its actions wait for. The realm behind the announced id is made
    // when first evaluated in.
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    h.page("Page.enable", json!({}));
    h.page("Page.addScriptToEvaluateOnNewDocument", json!({"source":"","worldName":"__pw_utility"}));
    let frame = h.frame_id.clone();
    let first = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"__pw_utility"}))["result"]
        ["executionContextId"]
        .clone();
    let events = h.page_with_events("Page.navigate", json!({ "url": html_url("<p>committed</p>") }));
    let mut events = events;
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        events.extend(h.run_for(100));
    }
    let created: Vec<&Value> =
        events.iter().filter(|m| m["method"] == "Runtime.executionContextCreated").collect();
    let utility = created
        .iter()
        .find(|m| m["params"]["context"]["name"] == "__pw_utility")
        .unwrap_or_else(|| panic!("utility world announced for the new document: {events:#?}"));
    let context = utility["params"]["context"]["id"].clone();
    assert_ne!(context, first, "a new context per document");
    assert_eq!(utility["params"]["context"]["auxData"]["isDefault"], false);
    assert!(created.iter().any(|m| m["params"]["context"]["auxData"]["isDefault"] == true), "{events:#?}");
    let destroyed = events.iter().position(|m| m["method"] == "Runtime.executionContextsCleared").unwrap();
    let announced = events
        .iter()
        .position(|m| m["method"] == "Runtime.executionContextCreated" && m["params"]["context"]["id"] == context)
        .unwrap();
    assert!(announced > destroyed, "{events:#?}");
    let frame = h.frame_id.clone();
    let r = h.page("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"__pw_utility"}));
    assert_eq!(r["result"]["executionContextId"], context, "the same world for this document");
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression":"document.querySelector('p').textContent + ':' + (globalThis.mark = 1)","contextId": context,"returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], "committed:1", "{r}");
    let r = h.page("Runtime.evaluate", json!({"expression":"typeof mark","returnByValue":true}));
    assert_eq!(r["result"]["result"]["value"], "undefined", "main world untouched");
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration":"function() { return mark + 1 }","executionContextId": context,"returnByValue":true}),
    );
    assert_eq!(r["result"]["result"]["value"], 2, "{r}");
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
    use pyro_cdp::network_protocol::{BodyBudget, BodyLimits};
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
    use pyro_cdp::network_protocol::{BodyBudget, BodyLimits};
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
    use pyro_cdp::target::Host;
    let (out, _) = Outbox::channel(1024 * 1024);
    let mut host = EngineHost::new(out).unwrap();
    let a = host.create_context().unwrap();
    let b = host.create_context().unwrap();
    let (pa, _) = host.create_page(a).unwrap();
    let (pb, _) = host.create_page(b).unwrap();
    let ca = host.thread.browser.page(pyro_browser::PageId(pa as u32)).unwrap().context;
    let cb = host.thread.browser.page(pyro_browser::PageId(pb as u32)).unwrap().context;
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

/// A document fetched over HTTP reports its whole network lifecycle before
/// the page events its commit causes, as Chrome does: the I/O thread wakes
/// the engine for the observations before it posts the completion that
/// commits the document, so they are owner-published when `frameNavigated`
/// is queued behind them.
#[test]
fn document_network_events_precede_its_frame_navigated() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0 && request.len() + n <= 8192);
            request.extend_from_slice(&chunk[..n]);
        }
        let body = b"<!doctype html><title>net order</title><p>hello";
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
    });
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Network.enable", json!({}));
    let session = h.session.clone();
    h.send(Some(&session), "Page.navigate", json!({"url":format!("http://{address}/doc")}));
    let events = until_event(&mut h, "Page.loadEventFired");
    server.join().unwrap();
    let methods: Vec<&str> = events.iter().filter_map(|e| e["method"].as_str()).collect();
    let at = |method: &str| methods.iter().position(|m| *m == method).unwrap_or_else(|| panic!("missing {method}: {methods:?}"));
    assert!(at("Network.requestWillBeSent") < at("Network.responseReceived"), "{methods:?}");
    assert!(at("Network.responseReceived") < at("Network.dataReceived"), "{methods:?}");
    assert!(at("Network.dataReceived") < at("Network.loadingFinished"), "{methods:?}");
    assert!(at("Network.loadingFinished") < at("Page.frameNavigated"), "{methods:?}");
    assert!(at("Page.frameNavigated") < at("Page.loadEventFired"), "{methods:?}");
    let request = events.iter().find(|e| e["method"] == "Network.requestWillBeSent").unwrap();
    let navigated = events.iter().find(|e| e["method"] == "Page.frameNavigated").unwrap();
    assert_eq!(request["params"]["loaderId"], navigated["params"]["frame"]["loaderId"]);
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
fn a_scroller_whose_content_ends_a_layout_unit_short_of_a_pixel_scrolls_to_that_pixel() {
    let mut h = Harness::new();
    // Measured in Chrome 156: content of 217.984375px (a layout unit short of
    // 218) in a 100px scroller has a scrollHeight of 218 and scrolls to 118,
    // the pixel-snapped sizes' difference; so does 217.5px, and 217.4px
    // scrolls to 117. The offset was 117.98 (117.5) cut to 117. The window
    // clamps alike, also where the document's size is its overflow (html and
    // body as tall as the viewport, as on ToS's settings pages).
    let value = eval_value(
        &mut h,
        r#"
      document.body.innerHTML = ['217.984375px', '217.5px', '217.4px']
        .map((height) => `<div class="box" style="height:100px;overflow:auto"><div style="height:${height}"></div></div>`)
        .join('');
      [...document.querySelectorAll('.box')].map((box) => {
        box.scrollTop = box.scrollHeight;
        return [box.scrollHeight, box.clientHeight, box.scrollTop];
      })
    "#,
    );
    assert_eq!(value, json!([[218, 100, 118], [218, 100, 118], [217, 100, 117]]));
    let value = eval_value(
        &mut h,
        r#"
      document.documentElement.style.height = '100%';
      document.body.style.cssText = 'margin:0;height:100%';
      document.body.innerHTML = '<div style="height:100%"></div><div id="rest"></div>';
      const root = document.scrollingElement;
      ['617.984375px', '617.5px', '617.4px'].flatMap((height) => {
        scrollTo(0, 0);
        document.getElementById('rest').style.height = height;
        root.scrollTop = root.scrollHeight;
        return [root.scrollHeight - innerHeight, scrollY];
      })
    "#,
    );
    assert_eq!(value, json!([618, 618, 618, 618, 617, 617]));
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

/// A left click at (`x`, `y`): press and release.
fn click_at(h: &mut Harness, x: f64, y: f64) {
    for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
        let r = h.page(
            "Input.dispatchMouseEvent",
            json!({"type":kind,"x":x,"y":y,"button":"left","buttons":buttons,"clickCount":1}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
    }
}

#[test]
fn a_click_on_a_scroll_button_scrolls_its_scroller_by_a_page() {
    // WPT css-overflow scroll-markers/scroll-buttons-activation and scroll-buttons-scroll-distance:
    // a click on an enabled ::scroll-button() scrolls its originating element by a page in its
    // direction (Chromium's page step, 87.5% of the scrollport), up to its scroll range; a
    // canceled click does not.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<style>body{margin:0}#sc{width:400px;height:200px;overflow:auto;white-space:nowrap}#sc>div{display:inline-block;width:400px;height:150px}#sc::scroll-button(inline-end){content:"";position:absolute;top:0;left:0;width:20px;height:20px}#sc::scroll-button(up){content:"";position:absolute;top:0;left:40px;width:20px;height:20px}</style><div id="sc"><div></div><div></div></div>';
      globalThis.sc=document.getElementById('sc'); globalThis.clicks=[];
      sc.addEventListener('click', e => clicks.push(e.target.id)); 1
    "#,
    );
    // The click goes to the originating element.
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(5, 5) === sc"), json!(true));
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "[sc.scrollLeft, clicks.join()]"), json!([350, "sc"]));
    // The second page stops at the end of the scroll range (404: the space between the blocks).
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    let end = eval_value(&mut h, "sc.scrollWidth - sc.clientWidth");
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), end);
    // Up is disabled (no vertical overflow): nothing scrolls.
    click_at(&mut h, 45.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "[sc.scrollLeft, sc.scrollTop]"), json!([end, 0]));
    // A canceled click does not scroll.
    eval_value(&mut h, "sc.scrollLeft = 0; sc.addEventListener('click', e => e.preventDefault()); 1");
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), json!(0));
}

#[test]
fn a_click_on_the_root_scroll_button_scrolls_the_viewport() {
    // WPT css-overflow scroll-markers/root-scroll-button-activation.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<style>body{margin:0}:root::scroll-button(block-end){content:"down";position:absolute;top:0;width:30px;height:30px}</style><div style="height:5000px"></div>'; 1
    "#,
    );
    let height = eval_value(&mut h, "innerHeight").as_f64().unwrap();
    click_at(&mut h, 10.0, 10.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "scrollY"), json!((height * 0.875).round() as i64));
}

#[test]
fn a_scroll_button_on_a_snapping_scroller_advances_to_the_next_snap_position() {
    // A page (350px of a 400px scrollport) lands nearer the current snap position (0) than the
    // next (1000): the button's scroll is directional, so it lands on the next one ahead, both
    // ways, as Chromium's directional snapping for scroll buttons. (The snap areas are small
    // children: a 1000px area wider than the snapport would let the page land inside it.)
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<style>body{margin:0}#sc{width:400px;height:200px;overflow:auto;white-space:nowrap;font-size:0;scroll-snap-type:x mandatory}#sc>div{display:inline-block;width:1000px;height:150px}#sc>div>i{display:block;width:10px;height:10px;scroll-snap-align:start}#sc::scroll-button(inline-end){content:"";position:absolute;top:0;left:0;width:20px;height:20px}#sc::scroll-button(inline-start){content:"";position:absolute;top:0;left:40px;width:20px;height:20px}</style><div id="sc"><div><i></i></div><div><i></i></div><div><i></i></div></div>';
      globalThis.sc=document.getElementById('sc'); 1
    "#,
    );
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), json!(1000));
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), json!(2000));
    click_at(&mut h, 45.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), json!(1000));
}

#[test]
fn a_scroll_button_under_another_element_is_not_activated() {
    // The painted hit decides: an element painted over the button takes the click, and the
    // scroller does not scroll; a `pointer-events: none` button lets the click through to the
    // link under it, which is followed.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r##"
      document.body.innerHTML='<style>body{margin:0}#sc{width:400px;height:200px;overflow:auto;white-space:nowrap}#sc>div{display:inline-block;width:400px;height:150px}#sc::scroll-button(inline-end){content:"";position:absolute;top:0;left:0;width:20px;height:20px}#cover{position:absolute;top:0;left:0;width:20px;height:20px;z-index:1}#pe{display:block;margin-top:50px;width:400px;height:30px;overflow:auto;white-space:nowrap}#pe span{display:inline-block;width:900px;height:10px}#pe::scroll-button(inline-end){content:"";position:absolute;top:250px;left:0;width:20px;height:20px;pointer-events:none}</style><div id="sc"><div></div><div></div></div><div id="cover"></div><a id="pe" href="#through"><span></span></a>';
      globalThis.sc=document.getElementById('sc'); globalThis.pe=document.getElementById('pe'); 1
    "##,
    );
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(5, 5).id"), json!("cover"));
    click_at(&mut h, 5.0, 5.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "sc.scrollLeft"), json!(0));
    // The pointer-events: none button over the link at (5, 255): the link's box is at 250..280.
    click_at(&mut h, 5.0, 255.0);
    h.run_for(200);
    assert_eq!(eval_value(&mut h, "[pe.scrollLeft, location.hash]"), json!([0, "#through"]));
}

#[test]
fn a_scroll_button_that_scrolls_handles_the_click_and_a_link_is_not_followed() {
    // WPT css-overflow scroll-markers/scroll-button-link-activation: the ::scroll-button() of an
    // element that is no scroll container does nothing, so the link it is in is followed; the
    // ::scroll-button() of a scroll container scrolls it instead.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"
      document.body.innerHTML='<style>body{margin:0}#c{margin-top:100px}a{display:block}.o{width:100px;height:50px}.s{overflow:auto;white-space:nowrap}.s span{display:inline-block;width:300px;height:10px}.o::scroll-button(inline-end){content:"";position:absolute;top:0;left:0;width:20px;height:20px}</style><div id="c"></div>'; 1
    "#,
    );
    for (markup, hash) in [
        (r##"<a href="#nav" class="o"></a>"##, "#nav"),
        (r##"<a href="#nav"><div class="o"></div></a>"##, "#nav"),
        (r##"<a href="#nav" class="o s"><span></span></a>"##, "#start"),
        (r##"<a href="#nav"><div class="o s"><span></span></div></a>"##, "#start"),
    ] {
        let set = format!("location.hash = '#start'; document.getElementById('c').innerHTML = {}; 1", json!(markup));
        eval_value(&mut h, &set);
        h.run_for(200);
        let hit = "document.elementFromPoint(10, 10) === document.querySelector('#c .o')";
        assert_eq!(eval_value(&mut h, hit), json!(true), "{markup}");
        click_at(&mut h, 10.0, 10.0);
        h.run_for(200);
        assert_eq!(eval_value(&mut h, "location.hash"), json!(hash), "{markup}");
        let scrolled = eval_value(&mut h, "document.querySelector('#c .o').scrollLeft > 0");
        assert_eq!(scrolled, json!(hash == "#start"), "{markup}");
    }
}

#[test]
fn input_mouse_wheel_over_empty_viewport_space_scrolls_the_viewport() {
    let mut h = Harness::new();
    eval_value(&mut h, "document.body.innerHTML='<div style=\"height:10000px\"></div>'; 1");
    // Playwright's initial mouse position: Chrome hits the root element there
    // (the body's margin).
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(0, 0) === document.documentElement"), json!(true));
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mouseWheel","x":0,"y":0,"deltaX":0,"deltaY":70}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "scrollY"), json!(70));
    let r = h.page("Input.dispatchMouseEvent", json!({"type":"mouseWheel","x":-1,"y":0,"deltaX":0,"deltaY":70}));
    assert!(r["error"]["message"].as_str().is_some_and(|m| m.contains("no element at point")), "{r}");
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
    // hit-tests empty viewport space as the root element. Expected sequences
    // recorded from headless Chromium (Playwright 1.59.1) for the same page.
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
    assert_eq!(eval_value(&mut h, "document.elementFromPoint(0, 0) === document.documentElement"), json!(true));
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
    // Outside the viewport keeps the explicit error.
    let r =
        h.page("Input.dispatchMouseEvent", json!({"type":"mouseMoved","x":-5,"y":2000,"button":"none","buttons":0}));
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
        json!(
            "pointerout@other<static pointerleave@other<static pointerover@static<other pointerenter@cell<other pointerenter@static<other mouseout@other<static mouseleave@other<static mouseover@static<other mouseenter@cell<other mouseenter@static<other mousemove@static<-"
        )
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
        json!(
            "pointerover@live<static pointerenter@#document<static pointerenter@HTML<static pointerenter@BODY<static pointerenter@cell<static pointerenter@live<static mouseover@live<static mouseenter@#document<static mouseenter@HTML<static mouseenter@BODY<static mouseenter@cell<static mouseenter@live<static"
        )
    );
    // (`#live:hover`, not a bare `:hover`: about:blank is in quirks mode since 6efaefdaf, as in
    // Chrome, and there a bare `:hover` matches links only.)
    assert_eq!(eval_value(&mut h, "document.getElementById('live').matches('#live:hover')"), json!(true));
}

#[test]
fn input_press_and_release_reconcile_hover_after_an_overlay_covers_the_pointer() {
    for open_at in ["before", "press", "click"] {
        let mut h = Harness::new();
        eval_value(
            &mut h,
            r#"
            document.body.innerHTML='<button id="target" style="position:absolute;left:20px;top:20px;width:150px;height:50px">Open</button>';
            globalThis.log=[];
            for(const type of ['pointerleave','pointerenter','mouseleave','mouseenter','mousemove'])
                document.addEventListener(type,e=>log.push([type,e.target.id,e.relatedTarget?.id]),true);
            globalThis.cover=()=>{const c=document.createElement('div');c.id='cover';c.style.cssText='position:fixed;inset:0;background:white';document.body.append(c)};0
        "#,
        );
        h.page(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseMoved","x":50,"y":40}),
        );
        eval_value(
            &mut h,
            match open_at {
                "press" => "log=[];document.getElementById('target').onpointerdown=cover;0",
                "click" => "log=[];document.getElementById('target').onclick=cover;0",
                _ => "log=[];cover();0",
            },
        );
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            let result = h.page(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":50,"y":40,"button":"left","buttons":buttons,"clickCount":1}),
            );
            assert_eq!(result["result"], json!({}), "{result}");
        }
        if open_at == "click" {
            h.run_for(30);
            assert_eq!(
                eval_value(&mut h, "log"),
                json!([]),
                "not in the mutating task"
            );
            h.run_for(150);
        }
        // Observed in Chromium: boundary events, without a synthetic move.
        assert_eq!(
            eval_value(&mut h, "log"),
            json!([
                ["pointerleave", "target", "cover"],
                ["pointerenter", "cover", "target"],
                ["mouseleave", "target", "cover"],
                ["mouseenter", "cover", "target"]
            ]),
            "open_at={open_at}"
        );
    }
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
    let dir = std::env::temp_dir().join(format!("pyro-downloads-{}", std::process::id()));
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

/// A CSP-sandboxed page (`sandbox allow-scripts`) that navigates itself to a `blob:` or `data:`
/// document it made keeps its policies and sandbox there (the document has the policy container
/// of the one that navigated to it): its origin stays opaque and, without allow-popups,
/// window.open returns null.
#[test]
fn a_csp_sandboxed_page_keeps_its_sandbox_in_local_scheme_documents() {
    for kind in ["blob", "data"] {
        let body: &'static [u8] = Box::leak(
            format!(
                r#"<!doctype html><script>
const html = '<script>document.title = [self.origin, String(window.open("about:blank"))].join()<\/script>';
location = {} ;
</script>"#,
                if kind == "blob" {
                    "URL.createObjectURL(new Blob([html], { type: 'text/html' }))"
                } else {
                    "'data:text/html,' + encodeURIComponent(html)"
                }
            )
            .into_bytes()
            .into_boxed_slice(),
        );
        let (addr, server) =
            serve_attachment(1, "Content-Type: text/html\r\nContent-Security-Policy: sandbox allow-scripts\r\n", body);
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/sandboxed") }));
        let scheme = format!("{kind}:");
        h.until_event(move |m| {
            m["method"] == "Page.frameNavigated"
                && m["params"]["frame"]["url"].as_str().is_some_and(|u| u.starts_with(scheme.as_str()))
        });
        h.run_for(100);
        assert_eq!(eval_value(&mut h, "document.title"), json!("null,null"), "{kind}");
        // The embedder's navigation (Page.navigate) is not the sandboxed page's: no sandbox.
        let fresh = "data:text/html,<script>document.title = String(window.open('about:blank') !== null)</script>";
        h.page_with_events("Page.navigate", json!({ "url": fresh }));
        h.run_for(100);
        assert_eq!(eval_value(&mut h, "document.title"), json!("true"), "{kind}: Page.navigate");
        server.join().unwrap();
    }
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
    let dir = std::env::temp_dir().join(format!("pyro-cd-downloads-{}", std::process::id()));
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
fn a_download_a_popup_starts_is_its_openers_and_closes_the_popup() {
    // Recorded from Chromium 147 (Playwright 1.59's full build): a
    // target=_blank link answered with an attachment opens a paused popup;
    // once resumed, its navigation becomes the download, announced with the
    // popup's frame id, and Chrome closes the tab it opened for it
    // (Target.detachedFromTarget). Playwright reports the download on the
    // opener; so do `Pyro.run` download steps.
    let (addr, server) = serve_attachment(
        2,
        "Content-Type: application/zip\r\nContent-Disposition: attachment; filename=report.zip\r\n",
        b"zipbytes",
    );
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    let targets = h.call(None, "Target.getTargets", json!({}));
    let ctx = targets["result"]["targetInfos"].as_array().unwrap().iter().find(|t| t["targetId"] == opener).unwrap()
        ["browserContextId"]
        .clone();
    let dir = std::env::temp_dir().join(format!("pyro-popup-downloads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.call(
        None,
        "Browser.setDownloadBehavior",
        json!({"behavior":"allowAndName","browserContextId":ctx,"downloadPath":dir,"eventsEnabled":true}),
    );
    h.call(None, "Target.setDiscoverTargets", json!({"discover":true}));
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({"url": format!("http://{addr}/page")}));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        until_event(&mut h, "Page.loadEventFired");
    }
    let session = h.session.clone();
    let run = h.send(
        Some(&session),
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "const a = document.getElementById('f'); a.target = '_blank'; a.click(); null"}},
            {"download": {"as": "file"}},
        ]}),
    );
    let attached =
        h.until_event(|m| m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != opener);
    assert_eq!(attached["params"]["targetInfo"]["openerId"], json!(opener));
    let popup_target = attached["params"]["targetInfo"]["targetId"].as_str().unwrap().to_owned();
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    h.call(Some(&popup), "Page.enable", json!({}));
    h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let mut events = h.until_response(run);
    // The opener's run got the download.
    let reply = events.iter().find(|e| e["id"] == run).unwrap().clone();
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["reads"]["file"]["filename"], "report.zip", "{reply}");
    assert_eq!(reply["result"]["reads"]["file"]["text"], "zipbytes", "{reply}");
    // Settled: the close request goes out with the next message handled.
    events.extend(h.run_for(100));
    h.call(None, "Target.getTargets", json!({}));
    events.extend(h.drain());
    let begin = events.iter().find(|e| e["method"] == "Browser.downloadWillBegin").expect("downloadWillBegin");
    assert_eq!(begin["params"]["frameId"], json!(popup_target), "the popup's frame, as in Chrome");
    let completed = events
        .iter()
        .find(|e| e["method"] == "Browser.downloadProgress" && e["params"]["state"] == "completed")
        .expect("completed");
    assert_eq!(std::fs::read(completed["params"]["filePath"].as_str().unwrap()).unwrap(), b"zipbytes");
    // No document committed in the popup, then it closed.
    assert!(
        !events.iter().any(|e| e["method"] == "Page.frameNavigated" && e["sessionId"] == json!(popup)),
        "{events:#?}"
    );
    let at = |pred: &dyn Fn(&Value) -> bool| events.iter().position(pred);
    let detached = at(&|e| e["method"] == "Target.detachedFromTarget" && e["params"]["sessionId"] == json!(popup))
        .unwrap_or_else(|| panic!("popup not closed: {events:#?}"));
    let destroyed = at(&|e| e["method"] == "Target.targetDestroyed" && e["params"]["targetId"] == json!(popup_target))
        .expect("targetDestroyed");
    assert!(at(&|e| e["method"] == "Browser.downloadWillBegin").unwrap() < detached && detached < destroyed);
    let targets = h.call(None, "Target.getTargets", json!({}));
    let ids: Vec<&Value> =
        targets["result"]["targetInfos"].as_array().unwrap().iter().map(|t| &t["targetId"]).collect();
    assert!(!ids.contains(&&json!(popup_target)), "{targets}");
    assert!(ids.contains(&&json!(opener)), "{targets}");
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn input_home_end_with_mac_scroll_commands_scroll_the_document() {
    // Playwright sends Home/End with scrollTo{Beginning,End}OfDocument on
    // macOS. Outside editable content Chrome scrolls the document. Scroll
    // events arrive at the next frame, so each key waits past one.
    let mut h = Harness::new();
    eval_value(
        &mut h,
        "document.body.innerHTML = '<div style=\"height:5000px\"></div>'; globalThis.scrolls = 0; document.addEventListener('scroll', () => scrolls++); 1",
    );
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
    // The border box: 40x30 plus the UA input padding and border.
    let (bw, bh) = (rect["width"].as_f64().unwrap(), rect["height"].as_f64().unwrap());
    assert_eq!((bw, bh), (48.0, 36.0));
    let quad = json!([x, y, x + bw, y, x + bw, y + bh, x, y + bh]);
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
            let doc = dom.create_document(pyro_dom::DocumentKind::Html, "about:blank");
            let node = dom.create_element(doc, "input").unwrap();
            dom.append_child(doc, node).unwrap();
            node
        };
        let scope = realm.scope().unwrap();
        let wrapper = pyro_webapi::wrap_node(&scope, &page, node).unwrap();
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
    assert_eq!(metadata["error"]["code"], -32602, "client hints without required members: {metadata}");
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
/// commit in pyro-browser plus queued replies in pyro-cdp.
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
/// accepted and run in place of the key's default action (TOS batch 12,
/// docubox-templates `keyboard.press("ArrowRight")`).
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
    // start/end of a single-line input (pyro-webapi default action) and carry
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
    // Other commands (scrolling, clipboard, undo, word movement, Emacs-style
    // aliases): the key events are delivered; handled commands act on the
    // focused element (here a button: nothing to edit), the others leave the
    // key's own default action.
    let advisory = [
        ("ArrowLeft", "ArrowLeft", 1, json!(["moveWordLeft"])),
        ("z", "KeyZ", 4, json!(["undo"])),
        ("c", "KeyC", 4, json!(["copy"])),
        ("d", "KeyD", 2, json!(["deleteForward"])),
        ("PageDown", "PageDown", 0, json!(["scrollPageDown"])),
        ("j", "KeyJ", 0, json!(["moveDown"])),
    ];
    for (key, code, modifiers, commands) in &advisory {
        let r = h.page(
            "Input.dispatchKeyEvent",
            json!({"type":"rawKeyDown","key":key,"code":code,"modifiers":modifiers,"commands":commands}),
        );
        assert_eq!(r["result"], json!({}), "{key} {commands}: {r}");
        h.page("Input.dispatchKeyEvent", json!({"type":"keyUp","key":key,"code":code,"modifiers":modifiers}));
    }
    assert_eq!(
        h.page("Runtime.evaluate", json!({"expression":"keys.slice(10)","returnByValue":true}))["result"]["result"]
            ["value"],
        json!(advisory.iter().map(|a| a.0).collect::<Vec<_>>()),
        "advisory commands deliver the key events"
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

/// `Emulation.setEmulatedMedia` drives `prefers-color-scheme`,
/// `prefers-reduced-motion` and the `print` media type: `matchMedia` and
/// `@media` rules follow at once, `change` fires (at the next safe point) on
/// the lists it flipped, and
/// each call replaces the previous emulation (as Chrome). A size change from
/// `setDeviceMetricsOverride` after load fires `resize` and relayouts.
#[test]
fn emulated_media_flips_match_media_and_fires_change() {
    let mut h = Harness::new();
    load_html(&mut h, "<style>p{color:red}@media (prefers-color-scheme: dark){p{color:blue}}@media (prefers-reduced-motion: reduce){p{opacity:.5}}</style><p id=p>x</p>");
    eval_value(
        &mut h,
        "globalThis.log = []; globalThis.p = document.getElementById('p'); matchMedia('(prefers-color-scheme: dark)').addEventListener('change', e => log.push('dark:' + e.matches)); \
         addEventListener('resize', () => log.push('resize:' + innerWidth + ':' + p.getBoundingClientRect().width)); 0",
    );
    let probe =
        "[matchMedia('(prefers-color-scheme: dark)').matches, matchMedia('(prefers-reduced-motion: reduce)').matches, \
        matchMedia('print').matches, getComputedStyle(p).color]";
    let features =
        json!([{"name":"prefers-color-scheme","value":"dark"},{"name":"prefers-reduced-motion","value":"reduce"}]);
    assert_eq!(h.page("Emulation.setEmulatedMedia", json!({"features": features}))["result"], json!({}));
    assert_eq!(eval_value(&mut h, probe), json!([true, true, false, "rgb(0, 0, 255)"]));
    assert_eq!(eval_value(&mut h, "getComputedStyle(p).opacity"), json!("0.5"));
    // `change` fires at the next safe point, after `matches` already flipped.
    h.run_for(50);
    assert_eq!(h.page("Emulation.setEmulatedMedia", json!({"media": "print"}))["result"], json!({}));
    assert_eq!(eval_value(&mut h, probe), json!([false, false, true, "rgb(255, 0, 0)"]));
    assert_eq!(eval_value(&mut h, "getComputedStyle(p).opacity"), json!("1"));
    let metrics = json!({"width":400,"height":300,"deviceScaleFactor":1,"mobile":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", metrics)["result"], json!({}));
    h.run_for(50);
    // Both changes are delivered at one safe point, `resize` before media
    // query `change` (the HTML "update the rendering" order).
    assert_eq!(eval_value(&mut h, "log.join()"), json!("dark:true,resize:400:384,dark:false"));
    let r = h.page("Emulation.setEmulatedMedia", json!({"media": "tv"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

/// What makes a document scroll, against Chrome 156 on the 800 x 600 viewport: `overflow` on `body` is the viewport's
/// (its content counts), a box fixed to the viewport and an empty box do not count.
#[test]
fn scrollable_overflow_of_body_overflow_fixed_and_empty_boxes() {
    let mut h = Harness::new();
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":800,"height":600,"deviceScaleFactor":1,"mobile":false}),
    );
    let wide = r#"<div style="width:2000px;height:50px"></div>"#;
    let tall = r#"<div style="width:50px;height:1000px"></div>"#;
    let far = "left:1900px;top:2000px";
    for (doc, chrome) in [
        // Goes to the viewport, so what is in `body` is scrollable overflow.
        (format!(r#"<body style="margin:0;overflow:hidden;width:300px">{wide}"#), [2000, 600]),
        (format!(r#"<body style="margin:0;overflow:hidden;height:100px">{tall}"#), [800, 1000]),
        // Stays on `body`: the root has its own.
        (
            format!(r#"<html style="overflow:hidden"><body style="margin:0;overflow:hidden;width:300px">{wide}"#),
            [800, 600],
        ),
        (
            format!(r#"<html style="overflow:hidden"><body style="margin:0;overflow:hidden;height:100px">{tall}"#),
            [800, 600],
        ),
        // Empty boxes count for nothing, wherever they are.
        (r#"<body style="margin:0"><div style="width:2000px;height:0"></div>"#.into(), [800, 600]),
        (
            format!(r#"<body style="margin:0"><div style="position:absolute;{far};width:0;height:10px"></div>"#),
            [800, 600],
        ),
        // Fixed does not count, absolute does.
        (
            format!(r#"<body style="margin:0"><div style="position:fixed;{far};width:50px;height:10px"></div>"#),
            [800, 600],
        ),
        (
            format!(r#"<body style="margin:0"><div style="position:absolute;{far};width:50px;height:10px"></div>"#),
            [1950, 2010],
        ),
    ] {
        load_html(&mut h, &format!("<!doctype html>{doc}"));
        let scroll =
            eval_value(&mut h, "[document.documentElement.scrollWidth, document.documentElement.scrollHeight]");
        assert_eq!(scroll, json!(chrome), "{doc}");
        let metrics = h.page("Page.getLayoutMetrics", json!({}));
        let content = &metrics["result"]["cssContentSize"];
        assert_eq!(
            [content["width"].as_f64().unwrap(), content["height"].as_f64().unwrap()],
            chrome.map(f64::from),
            "{doc}"
        );
    }
    // `body` keeps its own scroll sizes, 2000 px wide inside 300 px, either way.
    load_html(&mut h, &format!(r#"<!doctype html><body style="margin:0;overflow:hidden;width:300px">{wide}"#));
    assert_eq!(eval_value(&mut h, "[document.body.scrollWidth, document.body.clientWidth]"), json!([2000, 300]));
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
    // A fragment navigation fires popstate (state null) before hashchange, as Chrome does.
    assert_eq!(eval_value(&mut h, "seen"), json!([["popstate", "/two#x", null], ["hashchange", "", "#x"]]));
    let (index, entries) = history(&mut h);
    let urls: Vec<_> = entries.iter().map(|(_, u)| u.as_str()).collect();
    let base = format!("http://{address}");
    // The first navigation replaced the initial about:blank entry (Chrome).
    assert_eq!(urls, [format!("{base}/start"), format!("{base}/one"), format!("{base}/two"), format!("{base}/two#x")]);
    assert_eq!(index, 3);

    let error = h.page("Page.navigateToHistoryEntry", json!({"entryId": 999_999}));
    assert_eq!(error["error"]["code"], -32602, "{error}");

    let one = entries[1].0;
    let messages = h.page_with_events("Page.navigateToHistoryEntry", json!({"entryId": one}));
    assert!(messages.iter().any(|m| m["result"] == json!({})), "{messages:#?}");
    let mut messages = messages;
    messages.extend(h.run_for(50));
    let within = position(&messages, "navigatedWithinDocument", |m| m["method"] == "Page.navigatedWithinDocument");
    assert_eq!(messages[within]["params"]["url"], format!("{base}/one"));
    assert!(!messages.iter().any(|m| m["method"] == "Page.frameNavigated"), "same-document: {messages:#?}");
    assert_eq!(eval_value(&mut h, "seen.slice(2)"), json!([["popstate", "/one", {"n": 1}]]));
    assert_eq!(history(&mut h).0, 1);

    // Forward to the fragment entry of the same pushState entry: popstate (no
    // state), then hashchange.
    let fragment = entries[3].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": fragment}))["result"], json!({}));
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "location.href"), format!("{base}/two#x"));
    assert_eq!(history(&mut h).0, 3);
    let frame = h.page("Page.getFrameTree", json!({}));
    assert_eq!(frame["result"]["frameTree"]["frame"]["url"], format!("{base}/two#x"));
    // /one -> /two#x is not a fragment-only change: no hashchange.
    assert_eq!(eval_value(&mut h, "seen.slice(3)"), json!([["popstate", "/two#x", null]]));
    // history.back() over a fragment entry fires both events.
    eval_value(&mut h, "history.back()");
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "seen.slice(4)"), json!([["popstate", "/two", {"n": 2}], ["hashchange", "#x", ""]]));
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
    assert_eq!(index, 1);
    assert_eq!(entries.len(), 2, "the first navigation replaced the initial about:blank entry");
    let first = entries[0].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": first}))["result"], json!({}));
    let messages = h.run_for(100);
    let navigated = position(&messages, "frameNavigated", |m| m["method"] == "Page.frameNavigated");
    assert_eq!(messages[navigated]["params"]["frame"]["url"], "data:text/html,<p>first</p>");
    assert_eq!(eval_value(&mut h, "document.body.textContent"), "first");
    let (index, after) = history(&mut h);
    assert_eq!(index, 0);
    assert_eq!(after, entries, "traversal keeps the entry list and ids");
    // Forward again.
    let second = entries[1].0;
    assert_eq!(h.page("Page.navigateToHistoryEntry", json!({"entryId": second}))["result"], json!({}));
    h.run_for(100);
    assert_eq!(eval_value(&mut h, "document.body.textContent"), "second");
    assert_eq!(history(&mut h).0, 1);

    // Page.reload keeps the entry (id and state); Page.navigate to the same
    // URL replaces it with a new entry.
    eval_value(&mut h, "history.replaceState({k: 1}, '')");
    let (_, before) = history(&mut h);
    assert!(h.page("Page.reload", json!({}))["result"].get("errorText").is_none());
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "JSON.stringify(history.state)"), r#"{"k":1}"#);
    assert_eq!(history(&mut h), (1, before.clone()));
    let r = h.page("Page.navigate", json!({"url": "data:text/html,<p>second</p>"}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.run_for(50);
    assert_eq!(eval_value(&mut h, "history.state"), Value::Null);
    let (index, after) = history(&mut h);
    assert_eq!((index, &after[..1], after[1].1.as_str()), (1, &before[..1], before[1].1.as_str()));
    assert_ne!(after[1].0, before[1].0, "a same-URL navigation is a new (replacing) entry");
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

/// Network events moved in front of queued items are charged with pyro-net's
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
/// (the pyro-webapi/pyro-browser delivery frontier), in order with what the
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
/// pyro-browser's detector (PR #110) a fetch started by the page keeps
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

#[path = "engine_cdp/accessibility.rs"]
mod accessibility;

#[path = "engine_cdp/frames.rs"]
mod frames;

#[path = "engine_cdp/layout_text.rs"]
mod layout_text;

#[path = "engine_cdp/tier3_g2.rs"]
mod tier3_g2;

#[path = "engine_cdp/tier3_g1.rs"]
mod tier3_g1;

#[path = "engine_cdp/websocket.rs"]
mod websocket;

#[path = "engine_cdp/aria_snapshot.rs"]
mod aria_snapshot;

#[path = "engine_cdp/coverage.rs"]
mod coverage;

#[cfg(feature = "render")]
#[path = "engine_cdp/context_identity.rs"]
mod context_identity;
#[path = "engine_cdp/context_options.rs"]
mod context_options;

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

    // window.open with noopener opens a popup and returns null.
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
}

#[test]
fn a_new_windows_failed_navigation_commits_chromes_error_document() {
    // Recorded from Chromium 147 (Playwright 1.59's headless shell): a popup
    // whose navigation fails (here a refused connection) commits the error
    // document: Network.loadingFailed, then frameNavigated at
    // chrome-error://chromewebdata/ with the URL as unreachableUrl (opaque
    // origin "://"), its execution context after it, DOMContentLoaded and
    // load, and only then frameStoppedLoading. Playwright's `popup` event
    // waits for that commit.
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    h.page("Page.enable", json!({}));
    let refused = "http://127.0.0.1:1/refused";
    h.page("Runtime.evaluate", json!({ "expression": format!("window.open('{refused}'); 1") }));
    let attached =
        h.until_event(|m| m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != opener);
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    let popup_frame = attached["params"]["targetInfo"]["targetId"].clone();
    for method in ["Page.enable", "Runtime.enable", "Network.enable"] {
        h.call(Some(&popup), method, json!({}));
    }
    h.call(Some(&popup), "Page.setLifecycleEventsEnabled", json!({"enabled": true}));
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let mut events = h.until_response(id);
    let is_stopped = |m: &Value| m["method"] == "Page.frameStoppedLoading" && m["sessionId"] == json!(popup);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !events.iter().any(is_stopped) {
        assert!(Instant::now() < deadline, "{events:#?}");
        events.extend(h.run_for(20));
    }
    let mine: Vec<&Value> = events.iter().filter(|m| m["sessionId"] == json!(popup)).collect();
    let at = |name: &str| mine.iter().position(|m| m["method"] == name || m["params"]["name"] == name);
    let navigated = mine.iter().find(|m| m["method"] == "Page.frameNavigated").unwrap_or_else(|| panic!("{mine:#?}"));
    let frame = &navigated["params"]["frame"];
    assert_eq!(frame["id"], popup_frame);
    assert_eq!(frame["url"], "chrome-error://chromewebdata/");
    assert_eq!(frame["unreachableUrl"], refused);
    assert_eq!(frame["securityOrigin"], "://");
    assert_eq!(frame["secureContextType"], "InsecureScheme");
    let failed = mine.iter().find(|m| m["method"] == "Network.loadingFailed").expect("loadingFailed");
    assert_eq!(failed["params"]["errorText"], "net::ERR_CONNECTION_REFUSED");
    let order: Vec<usize> = [
        "Network.loadingFailed",
        "Page.frameNavigated",
        "Runtime.executionContextCreated",
        "DOMContentLoaded",
        "load",
        "Page.frameStoppedLoading",
    ]
    .iter()
    .map(|name| at(name).unwrap_or_else(|| panic!("{name} missing: {mine:#?}")))
    .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}: {mine:#?}");
    assert_eq!(mine.iter().filter(|m| m["method"] == "Page.frameStoppedLoading").count(), 1, "{mine:#?}");
    let tree = h.call(Some(&popup), "Page.getFrameTree", json!({}));
    assert_eq!(tree["result"]["frameTree"]["frame"]["unreachableUrl"], refused, "{tree}");
    let r = h.call(
        Some(&popup),
        "Runtime.evaluate",
        json!({"expression": "[location.href, location.origin].join()", "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "chrome-error://chromewebdata/,null", "{r}");

    // A new page navigated with Page.navigate: the error first, then the commit.
    drop(h);
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let mut events = h.page_with_events("Page.navigate", json!({"url": refused}));
    let reply =
        events.iter().position(|m| m["result"]["errorText"] == "net::ERR_CONNECTION_REFUSED").expect("errorText");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !events.iter().any(|m| m["method"] == "Page.frameNavigated") {
        assert!(Instant::now() < deadline, "{events:#?}");
        events.extend(h.run_for(20));
    }
    let navigated = events.iter().position(|m| m["method"] == "Page.frameNavigated").unwrap();
    assert!(reply < navigated, "{events:#?}");
    assert_eq!(events[navigated]["params"]["frame"]["unreachableUrl"], refused);
    // A page with a document of its own keeps it, as before.
    let events = h.page_with_events("Page.navigate", json!({"url": "http://127.0.0.1:1/again"}));
    assert!(events.iter().any(|m| m["result"]["errorText"] == "net::ERR_CONNECTION_REFUSED"), "{events:#?}");
    let later = h.run_for(100);
    assert!(!later.iter().chain(&events).any(|m| m["method"] == "Page.frameNavigated"), "{later:#?}");
    assert_eq!(eval_value(&mut h, "location.href"), "chrome-error://chromewebdata/");
}

#[test]
fn window_open_returns_a_minimal_popup_object() {
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    let attached_popup = |h: &mut Harness, seen: &mut Vec<String>| {
        let known = seen.clone();
        let opener = opener.clone();
        let attached = h.until_event(move |m| {
            m["method"] == "Target.attachedToTarget"
                && m["params"]["targetInfo"]["targetId"] != json!(opener)
                && !known.iter().any(|s| m["params"]["sessionId"] == json!(s))
        });
        seen.push(attached["params"]["sessionId"].as_str().unwrap().to_owned());
        attached
    };
    let mut seen = Vec::new();

    let expr = "globalThis.w = window.open('data:text/html,<title>first</title>'); [typeof w, w.closed, typeof w.document, typeof w.opener, w.location.href, typeof PopupWindow].join()";
    // The initial about:blank document exists as soon as window.open returns (HTML).
    assert_eq!(value(&mut h, expr), json!("object,false,object,object,about:blank,undefined"));
    let attached = attached_popup(&mut h, &mut seen);
    assert_eq!(attached["params"]["targetInfo"]["openerId"], json!(opener));
    assert_eq!(attached["params"]["targetInfo"]["canAccessOpener"], json!(true));
    assert_eq!(attached["params"]["waitingForDebugger"], json!(true));
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    let target = attached["params"]["targetInfo"]["targetId"].clone();

    // Navigating a popup still paused replaces what it loads when resumed.
    assert_eq!(value(&mut h, "w.location === w.location && (w.location.href = 'data:text/html,<title>second</title>', w.focus(), w.blur(), 1)"), json!(1));
    h.call(Some(&popup), "Page.enable", json!({}));
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let mut events = h.until_response(id);
    let is_navigated = |m: &Value| m["method"] == "Page.frameNavigated" && m["sessionId"] == json!(popup);
    let navigated = match events.iter().position(is_navigated) {
        Some(i) => events.swap_remove(i),
        None => h.until_event(is_navigated),
    };
    assert_eq!(navigated["params"]["frame"]["url"], json!("data:text/html,<title>second</title>"));

    // An unparseable URL throws and opens nothing.
    let invalid = "(() => { try { window.open('http://[') } catch (e) { return e.name } })()";
    assert_eq!(value(&mut h, invalid), json!("SyntaxError"));

    // Its document is now cross-origin: href reads throw, writes still navigate.
    let thrown = "(() => { try { return w.location.href } catch (e) { return e.name } })()";
    assert_eq!(value(&mut h, thrown), json!("SecurityError"));
    assert_eq!(
        value(&mut h, "(() => { try { w.location = 'http://[' } catch (e) { return e.name } })()"),
        json!("SyntaxError")
    );
    // Of a cross-origin Location only the href setter and replace() are reachable (HTML
    // CrossOriginProperties, Chrome): assign() throws, replace() navigates.
    assert_eq!(
        value(&mut h, "(() => { try { w.location.assign('data:text/html,<title>x</title>') } catch (e) { return e.name } })()"),
        json!("SecurityError")
    );
    value(&mut h, "w.location.replace('data:text/html,<title>third</title>'); 1");
    h.until_event(|m| {
        m["method"] == "Page.frameNavigated"
            && m["params"]["frame"]["url"] == json!("data:text/html,<title>third</title>")
    });

    // close() closes the target; closed reads true at once.
    assert_eq!(value(&mut h, "w.close(); w.closed"), json!(true));
    h.until_event(|m| m["method"] == "Target.detachedFromTarget" && m["params"]["targetId"] == target);

    // The client closing the popup reads as closed too.
    assert_eq!(value(&mut h, "globalThis.x = window.open(); x.closed"), json!(false));
    // Its initial about:blank document is there at once and is the one the popup keeps.
    assert_eq!(
        value(&mut h, "x.document.body.innerHTML = '<p id=kept>kept</p>'; [x.document.URL, typeof x.ReadableStream, x.document.readyState].join()"),
        json!("about:blank,function,complete")
    );
    let attached = attached_popup(&mut h, &mut seen);
    let target = attached["params"]["targetInfo"]["targetId"].clone();
    // A popup with no URL still commits about:blank when resumed; Playwright's
    // `popup` event waits for that commit.
    let blank = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    h.call(Some(&blank), "Page.enable", json!({}));
    let id = h.send(Some(&blank), "Runtime.runIfWaitingForDebugger", json!({}));
    let mut events = h.until_response(id);
    let is_navigated = |m: &Value| m["method"] == "Page.frameNavigated" && m["sessionId"] == json!(blank);
    let navigated = match events.iter().position(is_navigated) {
        Some(i) => events.swap_remove(i),
        None => h.until_event(is_navigated),
    };
    assert_eq!(navigated["params"]["frame"]["url"], json!("about:blank"));
    let r = h.call(Some(&blank), "Runtime.evaluate", json!({ "expression": "document.getElementById('kept').textContent + ':' + (window.opener !== null)", "returnByValue": true }));
    assert_eq!(
        r["result"]["result"]["value"],
        json!("kept:true")
    );
    assert_eq!(value(&mut h, "x.document.getElementById('kept').textContent"), json!("kept"));
    h.call(None, "Target.closeTarget", json!({ "targetId": target }));
    assert_eq!(value(&mut h, "x.closed"), json!(true));

    // close() in the same task as open(): the popup opens and closes in order.
    assert_eq!(value(&mut h, "globalThis.y = window.open(); y.close(); y.closed"), json!(true));
    let attached = attached_popup(&mut h, &mut seen);
    let target = attached["params"]["targetInfo"]["targetId"].clone();
    h.until_event(|m| m["method"] == "Target.detachedFromTarget" && m["params"]["targetId"] == target);
}

#[test]
fn child_frames_open_popups_unless_sandboxed() {
    let mut h = Harness::new();
    let opener = h.frame_id.clone();
    h.page("Page.enable", json!({}));
    let page = "data:text/html,<iframe id=f></iframe><iframe id=s sandbox='allow-same-origin allow-scripts'></iframe>";
    let seen = h.page_with_events("Page.navigate", json!({ "url": page }));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    // Without allow-popups no window opens: window.open returns null (HTML "the rules for choosing
    // a navigable", WPT html/browsers/sandboxing/sandbox-disallow-popups).
    let expr = "globalThis.blocked = document.getElementById('s').contentWindow.open('about:blank'); new Promise(r => setTimeout(r, 0)).then(() => blocked === null)";
    let r = h.page("Runtime.evaluate", json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }));
    assert_eq!(r["result"]["result"]["value"], json!(true), "no allow-popups: {r}");
    h.page(
        "Runtime.evaluate",
        json!({ "expression": "globalThis.open1 = document.getElementById('f').contentWindow.open('about:blank'); 1" }),
    );
    let attached =
        h.until_event(|m| m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != opener);
    assert_eq!(attached["params"]["targetInfo"]["openerId"], json!(opener));
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
    let setup = h.page(
        "Runtime.evaluate",
        json!({"expression": "document.body.innerHTML = '<div style=\"height:150px\"></div><button id=b>b</button><div style=\"height:3000px\"></div>'; \
            globalThis.scrolls = []; addEventListener('scroll', e => scrolls.push(e.target === document ? 'document' : e.target.id), true); \
            document.querySelector('#b')"}),
    );
    let object = setup["result"]["result"]["objectId"].clone();
    assert_eq!(
        eval_value(
            &mut h,
            "(() => { const r = document.querySelector('#b').getBoundingClientRect(); return r.top > 0 && r.bottom < innerHeight; })()"
        ),
        json!(true)
    );
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
        let point = eval_value(
            h,
            &format!(
                "(() => {{ const r = document.getElementById('{id}').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()"
            ),
        );
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
        json!(
            "pointerout@span<hs pointerleave@span<hs pointerover@hs<span pointerenter@field<span pointerenter@hs<span mouseout@span<hs mouseleave@span<hs mouseover@hs<span mouseenter@field<span mouseenter@hs<span"
        )
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
        json!(["0.75", "1.25", true, null, "n:beforeinput=1 n:input=0.75 n2:beforeinput=1.26 n2:input=1.25"])
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
    let setup = format!(
        "(() => {{ document.head.innerHTML = '<style>body{{margin:0;font-size:20px;line-height:30px}} td{{padding:0 20px 0 0}}</style>'; \
           document.body.innerHTML = {body:?}; globalThis.log = []; globalThis.cancel = {{}}; globalThis.hooks = {{}}; \
           if (!globalThis.d) {{ \
             globalThis.d = n => n ? (n.nodeType === 3 ? `#text(${{n.parentNode ? (n.parentNode.id || n.parentNode.nodeName) : 'detached'}})` : (n.id || n.nodeName)) : null; \
             for (const t of ['mousedown', 'selectstart', 'mouseup', 'click']) document.addEventListener(t, e => {{ log.push(`${{t}}@${{d(e.target)}}`); if (cancel[t]) e.preventDefault(); if (hooks[t]) hooks[t](e); }}, true); \
             document.addEventListener('mousemove', e => {{ if (hooks.mousemove) hooks.mousemove(e); }}); \
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
    let r = eval_value(
        &mut h,
        "(() => { const s = getSelection(); return `${s.type} ${d(s.anchorNode)}:${s.anchorOffset} ${log.filter(e => e !== 'selectionchange').join(',')}`; })()",
    );
    assert_eq!(r, json!("Caret #text(q):0 mousedown@q,selectstart@#text(q),mouseup@q,click@q"), "click collapses");
}

#[test]
fn mouse_presses_over_empty_viewport_space_target_the_root_element() {
    // Chromium hit-tests empty viewport space (below short content) as the
    // root element: a click there fires pointerdown, mousedown, pointerup,
    // mouseup and click at <html>. The engine rejected presses and releases
    // there ("no element at point"), e.g. a drag released where content had
    // been removed under the pointer.
    // The harness page is about:blank, in quirks mode (6efaefdaf, as in Chrome): there the body
    // fills the viewport (the "body fills the html" quirk), so the press is at <body>, as in
    // Chrome. A doctype document written over it (document.open) is in no-quirks mode, and its
    // layout follows: the space below the paragraph is the root's.
    let mut h = Harness::new();
    let press = |h: &mut Harness| {
        eval_value(
            h,
            "(() => { globalThis.log = []; for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) \
              document.addEventListener(t, e => log.push(`${t}@${e.target.id || e.target.nodeName}`), true); })()",
        );
        let at = json!([200.0, 400.0]);
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            let r = h.page(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":at[0],"y":at[1],"button":"left","buttons":buttons,"clickCount":1}),
            );
            assert!(r.get("error").is_none(), "{kind}: {r}");
        }
        eval_value(h, "log.join(',')")
    };
    eval_value(
        &mut h,
        "(() => { document.body.style.margin = '0'; document.body.innerHTML = '<p id=p style=\"margin:0;font-size:20px;line-height:30px\">abc</p>'; })()",
    );
    assert_eq!(eval_value(&mut h, "document.compatMode"), json!("BackCompat"));
    assert_eq!(press(&mut h), json!("pointerdown@BODY,mousedown@BODY,pointerup@BODY,mouseup@BODY,click@BODY"));
    eval_value(
        &mut h,
        "document.open(); document.write('<!doctype html><body style=margin:0><p id=p style=\"margin:0;font-size:20px;line-height:30px\">abc</p>'); document.close(); 1",
    );
    assert_eq!(eval_value(&mut h, "[document.compatMode, document.body.offsetHeight].join()"), json!("CSS1Compat,30"));
    assert_eq!(press(&mut h), json!("pointerdown@HTML,mousedown@HTML,pointerup@HTML,mouseup@HTML,click@HTML"));
}

/// Like `drag_select`, but reports the Range text instead of Selection's.
fn drag_select_range(h: &mut Harness, from: &Value, to: &Value) -> String {
    drag_select(h, from, to);
    eval_value(
        h,
        "(() => { const s = getSelection(); return `range=${JSON.stringify(s.rangeCount ? s.getRangeAt(0).toString() : '-')} type=${s.type} anchor=${d(s.anchorNode)}:${s.anchorOffset} focus=${d(s.focusNode)}:${s.focusOffset} log=${log.filter(e => e !== 'selectionchange').join(',')}`; })()",
    )
    .as_str()
    .unwrap_or_default()
    .to_owned()
}

#[test]
fn native_drag_selection_resolves_hit_targets_and_removed_nodes_like_chromium() {
    // Chromium (800x600, measured) resolves the caret within the element
    // under the pointer: over empty space below a clipped container the
    // caret is that element at offset 0 (not the clipped line nearer the
    // point), on text covered by an opaque positioned element it is that
    // element, and at the top of a scrolled container it is in the visible
    // line; selectstart targets the text when the point is on it, else the
    // element. A selectstart handler removing the text leaves no selection;
    // an anchor removed or replaced mid-drag continues from where it was; a
    // drag across user-select: none content keeps it inside the Range.
    let mut h = Harness::new();
    let lines: String = (0..8).map(|i| format!("<p id=l{i} style=\"margin:0\">line{i}text</p>")).collect();
    let clipped = format!(
        "<div id=sc style=\"height:90px;overflow:hidden\">{lines}</div><div style=\"height:100px\"></div><p id=v>visible text</p>"
    );
    let below = |h: &mut Harness, dx: f64| {
        eval_value(
            h,
            &format!(
                "(() => {{ const r = document.getElementById('sc').getBoundingClientRect(); return [{dx}, r.bottom + 15]; }})()"
            ),
        )
    };
    let click = |h: &mut Harness, at: &Value| {
        mouse(h, "mousePressed", at, 0.0, 1);
        mouse(h, "mouseReleased", at, 0.0, 0);
        eval_value(
            h,
            "(() => { const s = getSelection(); return `type=${s.type} anchor=${d(s.anchorNode)}:${s.anchorOffset} log=${log.filter(e => e !== 'selectionchange').join(',')}`; })()",
        )
    };
    selection_page(&mut h, &clipped, "document.getElementById('v').style.margin = '0'");
    let at = below(&mut h, 20.0);
    assert_eq!(
        click(&mut h, &at),
        json!("type=Caret anchor=DIV:0 log=mousedown@DIV,selectstart@DIV,mouseup@DIV,click@DIV"),
        "click over a clipped line"
    );
    selection_page(&mut h, &clipped, "document.getElementById('v').style.margin = '0'");
    let (a, z) = (text_point(&mut h, "#l0", 1.0), below(&mut h, 60.0));
    assert_eq!(
        drag_select_range(&mut h, &a, &z),
        "range=\"line0textline1textline2textline3textline4textline5textline6textline7text\" type=Range anchor=#text(l0):0 focus=DIV:0 log=mousedown@l0,selectstart@#text(l0),mouseup@DIV,click@BODY",
        "drag ending over a clipped line"
    );
    let scrolled = format!("<div id=sc style=\"height:90px;overflow:auto\">{lines}</div><p id=v>visible text</p>");
    selection_page(&mut h, &scrolled, "document.getElementById('v').style.margin = '0'");
    eval_value(&mut h, "document.getElementById('sc').scrollTop = 60");
    let top = eval_value(
        &mut h,
        "(() => { const r = document.getElementById('sc').getBoundingClientRect(); return [20, r.top + 1]; })()",
    );
    let r = click(&mut h, &top);
    assert!(
        r.as_str().unwrap().starts_with("type=Caret anchor=#text(l2):")
            && r.as_str().unwrap().contains("selectstart@l2"),
        "top of a scrolled container: {r}"
    );
    let covered = "<p id=p style=\"margin:0\">abcdefghij</p><div id=ov style=\"position:absolute;left:0;top:0;width:400px;height:30px;background:white\"></div><p id=q style=\"margin:0\">klmnopqrst</p>";
    selection_page(&mut h, covered, "");
    let on_p = text_point(&mut h, "#p", 30.0);
    assert_eq!(
        click(&mut h, &on_p),
        json!("type=Caret anchor=ov:0 log=mousedown@ov,selectstart@ov,mouseup@ov,click@ov"),
        "click on covered text"
    );
    selection_page(&mut h, covered, "");
    let (a, z) = (text_point(&mut h, "#q", -5.0), text_point(&mut h, "#p", 30.0));
    let r = drag_select_range(&mut h, &a, &z);
    assert!(
        r.contains("anchor=#text(q):")
            && r.contains("focus=ov:0")
            && r.contains("log=mousedown@q,selectstart@#text(q),mouseup@ov"),
        "drag ending on covered text: {r}"
    );
    let two = "<p id=p style=\"margin:0\">abcdefghij</p><p id=q style=\"margin:0\">klmnopqrst</p>";
    selection_page(
        &mut h,
        two,
        "hooks.selectstart = e => { if (e.target.nodeType === 3 && e.target.parentNode && e.target.parentNode.id === 'p') e.target.remove(); };",
    );
    let (a, z) = (text_point(&mut h, "#p", 1.0), text_point(&mut h, "#q", -1.0));
    let r = drag_select_range(&mut h, &a, &z);
    assert!(
        r.starts_with("range=\"-\" type=None") && r.contains("selectstart@#text(p)"),
        "selectstart removes its text: {r}"
    );
    selection_page(
        &mut h,
        two,
        "let n = 0; hooks.mousemove = () => { if (++n === 4) { const t = document.getElementById('p').firstChild; if (t) t.remove(); } };",
    );
    let (a, z) = (text_point(&mut h, "#p", 1.0), text_point(&mut h, "#q", -1.0));
    let r = drag_select_range(&mut h, &a, &z);
    assert!(
        r.starts_with("range=\"klmnopqrst\" type=Range") && r.contains("focus=#text(q):10"),
        "anchor removed mid-drag: {r}"
    );
    selection_page(
        &mut h,
        two,
        "let n = 0; hooks.mousemove = () => { if (++n === 4) document.getElementById('p').textContent = 'ABCDEFGHIJ'; };",
    );
    let (a, z) = (text_point(&mut h, "#p", 1.0), text_point(&mut h, "#q", -1.0));
    let r = drag_select_range(&mut h, &a, &z);
    assert!(
        r.starts_with("range=\"ABCDEFGHIJklmnopqrst\" type=Range anchor=#text(p):0 focus=#text(q):10"),
        "anchor text replaced mid-drag: {r}"
    );
    let across = "<p id=p style=\"margin:0\">abcdefghij</p><p id=q style=\"margin:0;user-select:none\">klmnopqrst</p><p id=r style=\"margin:0\">uvwxyz</p>";
    selection_page(&mut h, across, "");
    let (a, z) = (text_point(&mut h, "#p", 1.0), text_point(&mut h, "#r", -1.0));
    let r = drag_select_range(&mut h, &a, &z);
    assert!(
        r.starts_with("range=\"abcdefghijklmnopqrstuvwxyz\" type=Range anchor=#text(p):0 focus=#text(r):6"),
        "across a none region: {r}"
    );
}

#[test]
fn reported_geometry_is_in_layout_units_like_chromium() {
    // Chromium reports untransformed layout geometry in layout units: x, y,
    // width and height each truncated toward zero to 1/64 px, so a quad's
    // right edge minus its left edge is the width again. The engine reported
    // full-precision coordinates: TOS bookkeeping's 150px header at
    // x = 942.8340611353711 gave Playwright's boundingBox (quad maxX - minX)
    // 149.9999999999999, below the column's 150px minimum. Chromium (800x600,
    // measured): getBoundingClientRect and the box-model quad agree.
    let mut h = Harness::new();
    for (style, expected) in [
        ("left:100.7px;top:10.3px;width:150px;height:20px", [100.6875, 10.296875, 150.0, 20.0]),
        ("left:-10.7px;top:-3.3px;width:150px;height:20px", [-10.6875, -3.296875, 150.0, 20.0]),
        ("left:100px;top:10px;width:150.7px;height:20.3px", [100.0, 10.0, 150.6875, 20.296875]),
        ("left:100.3px;top:10px;width:33.33px;height:20px", [100.296875, 10.0, 33.328125, 20.0]),
        ("left:942.8340611353711px;top:10px;width:150px;height:20px", [942.828125, 10.0, 150.0, 20.0]),
    ] {
        let setup = h.page(
            "Runtime.evaluate",
            json!({"expression": format!("document.body.style.margin = '0'; document.body.innerHTML = '<div id=e style=\"position:absolute;{style}\"></div>'; document.getElementById('e')")}),
        );
        let object = setup["result"]["result"]["objectId"].clone();
        let rect = eval_value(
            &mut h,
            "(() => { const b = document.getElementById('e').getBoundingClientRect(); return [b.x, b.y, b.width, b.height]; })()",
        );
        let rect: Vec<f64> = rect.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(rect, expected, "{style}: DOMRect");
        let model = h.page("DOM.getBoxModel", json!({ "objectId": object }));
        let q = &model["result"]["model"]["border"];
        let (x0, x1, y0, y2) =
            (q[0].as_f64().unwrap(), q[2].as_f64().unwrap(), q[1].as_f64().unwrap(), q[5].as_f64().unwrap());
        assert_eq!(
            [x0, y0, x1 - x0, y2 - y0],
            expected,
            "{style}: box-model quad (and its width/height as Playwright derives them)"
        );
    }
}

/// Host lifetime on one thread: create, use, close everything, drop, repeat
/// (each host's loop services the thread's run loop while its VM exists and
/// stops with it); deterministic, no GC timing involved.
#[test]
fn host_create_close_all_drop_repeats_on_one_thread() {
    for round in 0..10 {
        let mut h = Harness::new();
        let session = h.session.clone();
        let r = h.call(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression": format!("{round} + 1"), "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"], json!(round + 1), "{r}");
        h.browser.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        h.browser.host_mut().close_all();
        drop(h);
    }
}

/// `data:` URL of an HTML document (every byte outside a safe set escaped).
fn html_url(html: &str) -> String {
    let mut url = String::from("data:text/html,");
    for b in html.bytes() {
        if b.is_ascii_alphanumeric() || b"<>=/-_.:;'\"(){}!,+".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

fn load_html(h: &mut Harness, html: &str) {
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": html_url(html) }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
}

const DRIVER_FORM: &str = r#"<!doctype html><body>
<label for="name">Your name</label> <input id="name">
<button id="go">Send</button> <button id="off" disabled>Off</button>
<div id="out"></div>
<div data-testid="row">one</div><div data-testid="row">two</div>
<a href="/x" style="display:none">Hidden link</a>
<script>
  document.getElementById('go').addEventListener('click', () => {
    const name = document.getElementById('name').value;
    setTimeout(() => { document.getElementById('out').textContent = 'Hello, ' + name; }, 30);
  });
</script>"#;

#[test]
fn driver_fills_clicks_and_waits_for_a_later_change() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_FORM);
    let r = h.page("Pyro.act", json!({"locator":{"label":"your name"},"action":"fill","value":"Ada"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(h.page("Pyro.query", json!({"locator":{"css":"input"},"read":"value"}))["result"]["value"], "Ada");
    // Filling again replaces the value.
    h.page("Pyro.act", json!({"locator":{"css":"input"},"action":"fill","value":"Grace"}));
    let r = h.page("Pyro.act", json!({"locator":{"role":"button","name":"Send"},"action":"click"}));
    assert_eq!(r["result"], json!({}), "{r}");
    // The text appears 30 ms later, from a timer: the expect is answered
    // once that task changed the DOM, with no further command.
    let start = Instant::now();
    let r = h.page("Pyro.expect", json!({"locator":{"css":"div[id=out]"},"assert":"text","expected":"Hello, Grace"}));
    assert_eq!(r["result"]["actual"], "Hello, Grace", "{r}");
    assert!(start.elapsed() >= Duration::from_millis(20) && start.elapsed() < Duration::from_secs(2));
    let r = h.page("Pyro.expect", json!({"locator":{"text":"hello, grace"},"assert":"visible"}));
    assert_eq!(r["result"]["actual"], true, "{r}");
}

#[test]
fn driver_locators_are_strict_and_report_why_they_wait() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_FORM);
    let r = h.page("Pyro.act", json!({"locator":{"testId":"row"},"action":"click"}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("strict mode violation") && message.contains("2 elements"), "{r}");
    let r = h.page("Pyro.query", json!({"locator":{"testId":"row","nth":-1},"read":"text"}));
    assert_eq!(r["result"]["value"], "two", "{r}");
    let r = h.page("Pyro.expect", json!({"locator":{"testId":"row"},"assert":"count","expected":2}));
    assert_eq!(r["result"]["actual"], 2, "{r}");
    // Hidden elements have no role.
    let r = h.page("Pyro.query", json!({"locator":{"role":"link"},"read":"count"}));
    assert_eq!(r["result"]["value"], 0, "{r}");
    let r = h.page("Pyro.expect", json!({"locator":{"css":".missing"},"assert":"hidden"}));
    assert_eq!(r["result"]["actual"], false, "{r}");
    let r = h.page("Pyro.act", json!({"locator":{"role":"button","name":"Off"},"action":"click","timeout":50}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("timed out after 50 ms") && message.contains("disabled"), "{r}");
    let r = h.page("Pyro.act", json!({"locator":{"css":"p","text":"x"},"action":"click"}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("exactly one"), "{r}");
}

/// Serves `/form` (a form whose button fetches `/api?name=...`, answered after
/// 100 ms) until the test ends.
fn driver_site() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or_default().to_owned();
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                let (ty, body) = if let Some(name) = path.strip_prefix("/api?name=") {
                    std::thread::sleep(Duration::from_millis(100));
                    ("application/json", format!(r#"{{"greeting":"Hello, {name}"}}"#))
                } else if path == "/eager-popup" {
                    ("text/html", "<!doctype html><body><p id=ready>Loading</p><script>fetch('/api?name=PopupStartup').then(r=>r.json()).then(data=>{document.getElementById('ready').textContent=data.greeting;document.body.dataset.ready='yes';});</script>".to_owned())
                } else if path == "/quiet" {
                    ("text/html", DRIVER_SITE_QUIET.to_owned())
                } else {
                    ("text/html", DRIVER_SITE_FORM.to_owned())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            });
        }
    });
    address
}

const DRIVER_SITE_FORM: &str = r#"<!doctype html><body>
<input id="name" aria-label="Name"> <input id="city" placeholder="City">
<button id="go">Send</button> <div id="out"></div>
<ul><li>Alpha <button>Edit</button></li><li>Beta <button>Edit</button></li></ul>
<div id="picked"></div> <div id="path"></div>
<button id="settings">Settings</button>
<div id="menu" style="display:none"><button>Close Position</button></div>
<script>
  window.addEventListener('popstate', async () => {
    document.getElementById('path').textContent = location.pathname;
    const r = await fetch('/api?name=' + location.pathname.slice(1));
    document.getElementById('out').textContent = (await r.json()).greeting;
  });
  document.getElementById('settings').addEventListener('click', () => {
    document.getElementById('menu').style.display = 'block';
  });
  document.getElementById('go').addEventListener('click', async () => {
    const name = document.getElementById('name').value;
    const r = await fetch('/api?name=' + encodeURIComponent(name));
    document.getElementById('out').textContent = (await r.json()).greeting;
  });
  for (const li of document.querySelectorAll('li')) {
    li.querySelector('button').addEventListener('click', () => {
      document.getElementById('picked').textContent = li.firstChild.textContent.trim();
    });
  }
</script>"#;

const DRIVER_SITE_QUIET: &str = r#"<!doctype html><body>
<button id="go">Go</button> <button id="poll">Poll</button> <div id="out"></div>
<script>
  // Background work that is not pending: a perpetual frame loop, an
  // interval and a far timeout.
  let spins = 0;
  const spin = () => { spins++; requestAnimationFrame(spin); };
  requestAnimationFrame(spin);
  setInterval(() => {}, 10);
  setTimeout(() => {}, 5000);
  // A click's follow-up: a short timeout, two frames, then a fetch.
  document.getElementById('go').addEventListener('click', () => {
    setTimeout(() => requestAnimationFrame(() => requestAnimationFrame(async () => {
      const r = await fetch('/api?name=quiet');
      document.getElementById('out').textContent = (await r.json()).greeting;
    })), 30);
  });
  // Polling with short timeouts never goes quiet.
  document.getElementById('poll').addEventListener('click', () => {
    const tick = () => setTimeout(tick, 20);
    tick();
  });
</script>"#;

/// Settle waits for everything an action set off (a short timeout, frames
/// chained from it, the fetch they start) but not for the page's background
/// work (an animation loop, an interval, a far timeout); `idle` waits the
/// same way and reports what kept the page busy when it gives up.
#[test]
fn driver_settles_when_the_page_goes_quiet() {
    let address = driver_site();
    let mut h = Harness::new();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/quiet")}},
        {"idle": {}},
        {"act": {"locator": {"css": "#go"}, "action": "click"}},
        {"read": {"locator": {"css": "#out"}, "read": "text", "as": "out"}},
        {"act": {"locator": {"css": "#poll"}, "action": "click", "settle": false}},
        {"idle": {"timeout": 200}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let result = &r["result"];
    // Read right after the click: the fetch it set off already landed.
    assert_eq!(result["reads"]["out"], "Hello, quiet", "{r}");
    let results = result["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert!(results[1].get("settled").is_none(), "background work kept the page busy: {r}");
    assert!(results[1]["settleMs"].as_f64().unwrap() < 100.0, "{r}");
    assert!(results[2].get("settled").is_none(), "{r}");
    // 30 ms timeout + 100 ms response, well short of the 1 s cap.
    let settle = results[2]["settleMs"].as_f64().unwrap();
    assert!((120.0..900.0).contains(&settle), "{r}");
    assert_eq!(results[5]["settled"], false, "{r}");
    assert_eq!(results[5]["pending"]["timers"], 1, "{r}");

    // `Pyro.idle` is the same wait as one command.
    let r = h.page("Pyro.idle", json!({ "timeout": 100 }));
    assert_eq!(r["result"]["settled"], false, "{r}");
    assert_eq!(r["result"]["pending"]["timers"], 1, "{r}");
}

/// `Pyro.run`: navigation, actions, checks and reads in one command. A click's
/// settle waits for the fetch it started, so the read right after it already
/// sees the response's effect; chained locators pick a button inside a row.
#[test]
fn driver_run_does_a_whole_scenario_in_one_reply() {
    let address = driver_site();
    let mut h = Harness::new();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"act": {"locator": {"label": "Name"}, "action": "fill", "value": "Ada"}},
        {"act": {"locator": {"placeholder": "city"}, "action": "fill", "value": "Ljubljana"}},
        {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click"}},
        {"read": {"locator": {"css": "#out"}, "read": "text", "as": "greeting"}},
        {"act": {"locator": {"role": "button", "name": "Edit",
                             "within": {"css": "li", "filter": {"hasText": "beta"}}}, "action": "click"}},
        {"check": {"locator": {"css": "#picked"}, "assert": "text", "expected": "Beta"}},
        {"read": {"locator": {"css": "li", "filter": {"has": {"role": "button"}, "hasNotText": "alpha"}},
                  "read": "text"}},
        {"read": {"locator": {"role": "button", "name": "Edit"}, "read": "count"}},
        {"read": {"locator": {"css": "#city"}, "read": "value"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let result = &r["result"];
    assert_eq!(result["reads"]["greeting"], "Hello, Ada", "{r}");
    let results = result["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(results.len(), 10, "{r}");
    // The click waited for the 100 ms response.
    assert!(results[3]["settleMs"].as_f64().unwrap() >= 80.0, "{r}");
    assert!(results[3].get("settled").is_none(), "{r}");
    assert_eq!(results[6]["actual"], "Beta", "{r}");
    assert_eq!(results[7], "Beta Edit", "{r}");
    assert_eq!(results[8], 2, "{r}");
    assert_eq!(results[9], "Ljubljana", "{r}");

    // The first failing step ends the run, named in the error.
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 50, "steps": [
            {"check": {"locator": {"css": "#picked"}, "assert": "text", "expected": "Beta"}},
            {"act": {"locator": {"role": "button", "name": "Edit"}, "action": "click"}},
            {"act": {"locator": {"css": "#missing"}, "action": "click"}},
        ]}),
    );
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 1 (Click") && message.contains("strict mode violation"), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 50, "steps": [{"act": {"locator": {"css": "#missing"}, "action": "click"}}]}),
    );
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 0 (Click") && message.contains("timed out after 50 ms"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"jump": {}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().starts_with("step 0"), "{r}");
}

/// `Pyro.run`'s same-document `route` settles on the fetch the app starts for
/// it; `press` activates a focused button; `eval` and regex locators read
/// values; `request` steps check the run's request log, which the reply
/// carries too.
#[test]
fn driver_run_routes_presses_and_checks_requests() {
    let address = driver_site();
    let mut h = Harness::new();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"route": {"url": "/positions"}},
        {"read": {"locator": {"css": "#out"}, "read": "text", "as": "out"}},
        {"check": {"locator": {"css": "#path"}, "assert": "text", "expected": "/positions"}},
        {"request": {"method": "GET", "path": "/api", "status": 200}},
        {"act": {"locator": {"role": "button", "name": "Settings", "exact": true}, "action": "press", "value": "Enter"}},
        {"check": {"locator": {"role": "button", "name": "Close Position"}, "assert": "visible"}},
        {"eval": {"expression": "location.pathname", "as": "where"}},
        {"read": {"locator": {"text": {"regex": "^hello, pos.*$", "flags": "i"}}, "read": "text", "as": "re"}},
        {"request": {"path": "/api", "count": 1}},
        {"request": {"path": "/nothing", "count": 0}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let result = &r["result"];
    assert_eq!(result["reads"]["out"], "Hello, positions", "{r}");
    assert_eq!(result["reads"]["where"], "/positions", "{r}");
    assert_eq!(result["reads"]["re"], "Hello, positions", "{r}");
    assert_eq!(result["results"][4]["status"], 200, "{r}");
    let requests = result["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
    let urls: Vec<&str> = requests.iter().filter_map(|q| q["url"].as_str()).collect();
    assert!(
        urls.iter().any(|u| u.ends_with("/form")) && urls.iter().any(|u| u.ends_with("/api?name=positions")),
        "{r}"
    );

    let r = h.page("Pyro.run", json!({"timeout": 50, "steps": [{"request": {"method": "POST", "path": "/api"}}]}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 0 (request") && message.contains("no matching request yet"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "missing.call()"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("threw"), "{r}");
}

/// `eval` with `call`: the expression is a function, called with the JSON
/// `arg`; with a `locator` it gets the element (waiting for exactly one) or,
/// with `all`, the array of matches. Playwright's `evaluate(fn, arg)` and
/// `locator.evaluate(fn, arg)`.
#[test]
fn driver_run_eval_calls_functions_with_elements() {
    let address = driver_site();
    let mut h = Harness::new();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"eval": {"expression": "(n) => n * 2", "call": true, "arg": 21, "as": "doubled"}},
        {"eval": {"expression": "() => document.title.length >= 0", "call": true, "as": "noArg"}},
        {"eval": {"expression": "(el, suffix) => el.id + suffix", "locator": {"role": "button", "name": "Settings", "exact": true}, "arg": "!", "as": "one"}},
        {"eval": {"expression": "(els) => els.map((e) => e.tagName)", "locator": {"css": "button"}, "all": true, "as": "all"}},
        {"eval": {"expression": "(els) => els.length", "locator": {"css": "#missing"}, "all": true, "as": "none"}},
        {"eval": {"expression": "async (el) => { await new Promise((r) => setTimeout(r, 5)); return el.tagName; }", "locator": {"css": "#out"}, "as": "later"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let reads = &r["result"]["reads"];
    assert_eq!(reads["doubled"], 42, "{r}");
    assert_eq!(reads["noArg"], true, "{r}");
    assert_eq!(reads["one"], "settings!", "{r}");
    assert!(reads["all"].as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|t| t == "BUTTON")), "{r}");
    assert_eq!(reads["none"], 0, "{r}");
    assert_eq!(reads["later"], "DIV", "{r}");

    // Waits for the element, then times out naming the locator.
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"eval": {"expression": "(el) => el.id", "locator": {"css": "#missing"}, "timeout": 50}}]}),
    );
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("timed out after 50 ms") && message.contains("matched no element"), "{r}");
    let r =
        h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "(el) => el.id", "locator": {"css": "button"}}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("strict mode violation"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "42", "call": true}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("not a function"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "(x) => x.y.z", "call": true, "arg": {}}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("threw"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "(e) => e", "all": true}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("needs a locator"), "{r}");
}

/// Locator kinds for Playwright's API: `xpath`, `of` (filter or nth after
/// an nth), `or` (union in document order), `and` (intersection) and the
/// `getByRole` state options.
#[test]
fn driver_locators_xpath_of_or_and_role_states() {
    let address = driver_site();
    let mut h = Harness::new();
    let html = r#"<h1>Top</h1><h2>Sub</h2><div aria-level="3" role="heading">Aria</div>
<ul><li>Apple <b>red</b></li><li>Banana</li><li>Cherry <b>red</b></li></ul>
<input type="checkbox" id="a" checked aria-label="A"><input type="checkbox" id="b" aria-label="B">
<button aria-pressed="true">Bold</button><button aria-pressed="false">Italic</button><button disabled>Off</button>
<button aria-expanded="true">Menu</button><button hidden>Ghost</button>
<select aria-label="S"><option>x</option><option selected>y</option></select>"#;
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"eval": {"expression": "(h) => { document.body.innerHTML = h; }", "arg": html}},
        {"read": {"locator": {"xpath": "//li[b]"}, "read": "count", "as": "xpath"}},
        {"read": {"locator": {"of": {"css": "li", "nth": 0}, "filter": {"hasText": "Apple"}}, "read": "count", "as": "ofKeep"}},
        {"read": {"locator": {"of": {"css": "li", "nth": 0}, "filter": {"hasText": "Banana"}}, "read": "count", "as": "ofDrop"}},
        {"read": {"locator": {"of": {"css": "h2"}, "or": [{"css": "h1"}], "nth": 0}, "read": "text", "as": "orFirst"}},
        {"read": {"locator": {"of": {"css": "li"}, "and": [{"xpath": "//li[b]"}]}, "read": "count", "as": "and"}},
        {"read": {"locator": {"of": {"css": "li"}, "and": [{"text": "red"}]}, "read": "count", "as": "andText"}},
        {"read": {"locator": {"role": "heading", "level": 2}, "read": "text", "as": "level2"}},
        {"read": {"locator": {"role": "heading", "level": 3}, "read": "text", "as": "level3"}},
        {"read": {"locator": {"role": "checkbox", "checked": true}, "read": "name", "as": "checked"}},
        {"read": {"locator": {"role": "button", "pressed": true}, "read": "name", "as": "pressed"}},
        {"read": {"locator": {"role": "button", "disabled": true}, "read": "name", "as": "disabled"}},
        {"read": {"locator": {"role": "button", "expanded": true}, "read": "name", "as": "expanded"}},
        {"read": {"locator": {"role": "option"}, "read": "count", "as": "options"}},
        {"read": {"locator": {"role": "option", "selected": true}, "read": "text", "as": "selected"}},
        {"read": {"locator": {"role": "button", "includeHidden": true}, "read": "count", "as": "hidden"}},
        {"read": {"locator": {"role": "button"}, "read": "count", "as": "notHidden"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let reads = &r["result"]["reads"];
    assert_eq!(reads["xpath"], 2, "{r}");
    assert_eq!(reads["ofKeep"], 1, "{r}");
    assert_eq!(reads["ofDrop"], 0, "{r}");
    assert_eq!(reads["orFirst"], "Top", "document order: {r}");
    assert_eq!(reads["and"], 2, "{r}");
    // As Playwright: the text locator names the <b>, not the <li>.
    assert_eq!(reads["andText"], 0, "{r}");
    assert_eq!(reads["options"], 2, "options of a visible select: {r}");
    assert_eq!(reads["level2"], "Sub", "{r}");
    assert_eq!(reads["level3"], "Aria", "{r}");
    assert_eq!(reads["checked"], "A", "{r}");
    assert_eq!(reads["pressed"], "Bold", "{r}");
    assert_eq!(reads["disabled"], "Off", "{r}");
    assert_eq!(reads["expanded"], "Menu", "{r}");
    assert_eq!(reads["selected"], "y", "{r}");
    assert_eq!(reads["hidden"], 5, "{r}");
    assert_eq!(reads["notHidden"], 4, "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"read": {"locator": {"css": "li", "level": 1}, "read": "count"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("need a role"), "{r}");
}

/// `modifiers` on pointer actions set the events' modifier flags (Playwright's
/// `click({ modifiers })`).
#[test]
fn driver_click_modifiers_reach_the_events() {
    let address = driver_site();
    let mut h = Harness::new();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"eval": {"expression": "() => { document.body.innerHTML = '<button id=m>M</button>'; window.seen = []; m.addEventListener('click', (e) => seen.push([e.shiftKey, e.ctrlKey, e.altKey, e.metaKey].join())); }", "call": true}},
        {"act": {"locator": {"css": "#m"}, "action": "click", "modifiers": ["Shift", "Alt"]}},
        {"act": {"locator": {"css": "#m"}, "action": "click"}},
        {"mouse": {"action": "click", "x": 5, "y": 5, "modifiers": ["Control"]}},
        {"eval": {"expression": "seen", "as": "seen"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let seen = r["result"]["reads"]["seen"].as_array().cloned().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(seen[0], "true,false,true,false", "{r}");
    assert_eq!(seen[1], "false,false,false,false", "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"act": {"locator": {"css": "#m"}, "action": "click", "modifiers": ["Hyper"]}}]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("unknown modifier"), "{r}");
}

const DRIVER_STEPS_PAGE: &str = r#"<!doctype html><body>
<style>#tip{display:none} #cell:hover + #tip{display:block} #box{color:rgb(1, 2, 3);width:120px;height:40px}</style>
<div id="cell">Cell</div><div id="tip">Tooltip</div>
<div id="box"></div>
<input id="t" aria-label="Title">
<button id="dbl">Twice</button> <div id="dbl-out"></div>
<div id="blur-out"></div>
<button id="load" aria-label="Load items">Load</button> <div id="status"></div> <ul id="items"></ul>
<script>
  const t = document.getElementById('t');
  t.addEventListener('blur', () => { document.getElementById('blur-out').textContent = 'blurred:' + t.value; });
  document.getElementById('dbl').addEventListener('dblclick', (e) => { document.getElementById('dbl-out').textContent = 'dbl:' + e.detail; });
  document.getElementById('cell').addEventListener('mouseover', () => { document.getElementById('cell').dataset.hovered = '1'; });
  document.getElementById('load').addEventListener('click', async () => {
    const status = document.getElementById('status'); status.textContent = 'loading';
    try {
      const r = await fetch('/api/items?page=1');
      if (!r.ok) { status.textContent = 'error ' + r.status; return; }
      const items = await r.json();
      document.getElementById('items').innerHTML = items.map((i) => '<li>' + i.name + '</li>').join('');
      status.textContent = 'done';
    } catch (e) { status.textContent = 'failed'; }
  });
</script>"#;

/// `Pyro.run`'s `page` step drives a popup in the same run: steps after it
/// act on the popup (its fetch settles, `request` matches its log), `main`
/// returns to the page the run started on with its state intact, and the
/// reply's requests are the starting page's. An unknown target is a step
/// error after its timeout.
#[test]
fn driver_run_drives_a_popup_then_returns_to_the_opener() {
    let address = driver_site();
    let mut h = Harness::new();
    // As `pyro`: no auto-attach, so a popup navigates at once
    // (paused popups wait for the client's `Runtime.runIfWaitingForDebugger`).
    // Turning auto-attach off detaches the page: attach again explicitly.
    h.call(
        None,
        "Target.setAutoAttach",
        json!({"autoAttach": false, "waitForDebuggerOnStart": false, "flatten": true}),
    );
    let targets = h.call(None, "Target.getTargets", json!({}));
    let target = targets["result"]["targetInfos"][0]["targetId"].clone();
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true}));
    h.session = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"act": {"locator": {"label": "Name"}, "action": "fill", "value": "Opener"}},
        {"eval": {"expression": "document.getElementById('settings').onclick = () => window.open('/form?popup', '_blank', 'noopener'); null"}},
        {"act": {"locator": {"css": "#settings"}, "action": "click", "settle": false}},
        {"popup": {"as": "tab"}},
        {"page": {"target": "tab"}},
        {"url": {"url": "/form?popup"}},
        {"loaded": {}},
        {"act": {"locator": {"label": "Name"}, "action": "fill", "value": "Popup"}},
        {"act": {"locator": {"css": "#go"}, "action": "click"}},
        {"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "Hello, Popup"}},
        {"request": {"path": "/api"}},
        {"eval": {"expression": "[location.search, !!window.opener, document.getElementById('name').value].join()", "as": "popup"}},
        {"page": {"target": "main"}},
        {"eval": {"expression": "[location.search, document.getElementById('name').value, document.getElementById('out').textContent].join()", "as": "main"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["popup"], "?popup,false,Popup", "{r}");
    assert_eq!(reads["main"], ",Opener,", "{r}");
    let results = r["result"]["results"].as_array().unwrap();
    assert_eq!(results[5]["targetId"], reads["tab"]["targetId"], "{r}");
    let requests = r["result"]["requests"].as_array().unwrap();
    assert!(requests.iter().all(|q| !q["url"].as_str().unwrap_or_default().contains("/api")), "{r}");
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"page": {"target": "nope"}}]}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 0") && message.contains("no page nope"), "{r}");
}

/// `Pyro.run`'s `viewport` step resizes mid-run (`innerWidth`, relayout of the
/// body inside its 8px margins, a `resize` listener, a width media query)
/// and `media` emulates
/// `prefers-color-scheme` / `prefers-reduced-motion` / `print`: `matchMedia`
/// flips and its `change` listeners fire (also one added without keeping
/// the list), before the next step runs. A query without a media type is
/// `all`: `print` flips `screen and (max-width)`, not `(max-width)`. Invalid
/// values are step errors.
#[test]
fn driver_run_resizes_the_viewport_and_emulates_media() {
    let address = driver_site();
    let mut h = Harness::new();
    let setup = "globalThis.log = []; \
        addEventListener('resize', () => log.push('resize:' + innerWidth + 'x' + innerHeight + ':' + document.body.getBoundingClientRect().width)); \
        globalThis.dark = matchMedia('(prefers-color-scheme: dark)'); \
        dark.addEventListener('change', e => log.push('dark:' + e.matches + ':' + (e instanceof MediaQueryListEvent) + ':' + e.media)); \
        matchMedia('(prefers-reduced-motion: reduce)').addListener(e => log.push('motion:' + e.matches)); \
        matchMedia('print').onchange = e => log.push('print:' + e.matches); \
        matchMedia('(max-width: 500px)').addEventListener('change', e => log.push('narrow:' + e.matches)); \
        null";
    let probe = "[innerWidth, innerHeight, dark.matches, matchMedia('(prefers-color-scheme: light)').matches, \
        matchMedia('(prefers-reduced-motion: reduce)').matches, matchMedia('print').matches, \
        matchMedia('screen and (max-width: 500px)').matches].join()";
    let steps = json!([
        {"goto": {"url": format!("http://{address}/form")}},
        {"eval": {"expression": setup}},
        {"viewport": {"width": 390, "height": 844}},
        {"eval": {"expression": probe, "as": "narrow"}},
        {"media": {"colorScheme": "dark", "reducedMotion": "reduce"}},
        {"eval": {"expression": probe, "as": "dark"}},
        {"media": {"media": "print"}},
        {"eval": {"expression": probe, "as": "print"}},
        {"media": {}},
        {"eval": {"expression": probe, "as": "reset"}},
        {"viewport": {"width": 1280, "height": 720}},
        {"eval": {"expression": "log.join()", "as": "log"}},
    ]);
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    let reads = &r["result"]["reads"];
    assert_eq!(reads["narrow"], "390,844,false,true,false,false,true", "{r}");
    assert_eq!(reads["dark"], "390,844,true,false,true,false,true", "{r}");
    assert_eq!(reads["print"], "390,844,false,true,false,true,false", "{r}");
    assert_eq!(reads["reset"], "390,844,false,true,false,false,true", "{r}");
    assert_eq!(
        reads["log"],
        "resize:390x844:374,narrow:true,dark:true:true:(prefers-color-scheme: dark),motion:true,dark:false:true:(prefers-color-scheme: dark),motion:false,print:true,print:false,resize:1280x720:1264,narrow:false",
        "{r}"
    );
    // The step settles (listeners ran) and reports the size.
    assert_eq!(r["result"]["results"][2]["width"], 390, "{r}");
    assert!(r["result"]["results"][2]["settleMs"].is_number(), "{r}");
    for bad in [json!({"viewport": {"width": 0, "height": 10}}), json!({"media": {"colorScheme": "sepia"}})] {
        let r = h.page("Pyro.run", json!({ "steps": [bad] }));
        assert!(r["error"]["message"].as_str().unwrap_or_default().starts_with("step 0"), "{r}");
    }
}

/// `read css` and `check css` take the property as `getComputedStyle(el)[name]` does: the CSS spelling
/// (`scroll-margin-bottom`) or the camel-cased CSSOM attribute (`scrollMarginBottom`, `backgroundColor`,
/// `cssFloat`, `webkitTransform`). SloSki's club roster reads its row action's `scrollMarginBottom`
/// (`[&_button]:scroll-mb-14`, 3.5rem) and expects `56px`; the camel-cased name used to reach the style
/// engine as it was and read `""`. A custom property keeps its own name.
#[test]
fn driver_css_read_and_check_accept_cssom_property_names() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
        :root { --gap: 12px; --mixedCase: 17px }
        .list button { scroll-margin-bottom: 3.5rem; background-color: rgb(1, 2, 3); float: left }
        #plain { scroll-margin-bottom: 0 }
        </style><body><div class=list><button id=b>Act</button></div><button id=plain>Plain</button></body>"#,
    );
    let read = |h: &mut Harness, selector: &str, property: &str| -> serde_json::Value {
        let r = h.page("Pyro.query", json!({"locator": {"css": selector}, "read": "css", "property": property}));
        assert!(r.get("error").is_none(), "{r}");
        r["result"]["value"].clone()
    };
    for (property, expected) in [
        ("scrollMarginBottom", "56px"),
        ("scroll-margin-bottom", "56px"),
        ("scrollMarginTop", "0px"),
        ("scrollMarginBlockEnd", "56px"),
        ("scrollMargin", "0px 0px 56px"),
        ("backgroundColor", "rgb(1, 2, 3)"),
        ("background-color", "rgb(1, 2, 3)"),
        ("cssFloat", "left"),
        ("float", "left"),
        ("--gap", "12px"),
        ("--mixedCase", "17px"),
        ("--mixedcase", ""),
        ("notAProperty", ""),
    ] {
        assert_eq!(read(&mut h, "#b", property), expected, "read css {property}");
    }
    assert_eq!(read(&mut h, "#plain", "scrollMarginBottom"), "0px", "an id beats the descendant rule");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"check": {"locator": {"css": "#b"}, "assert": "css", "property": "scrollMarginBottom", "expected": "56px"}},
            {"check": {"locator": {"css": "#b"}, "assert": "css", "property": "scroll-margin-bottom", "expected": "56px"}},
            {"read": {"locator": {"css": "#b"}, "read": "css", "property": "scrollMarginBottom", "as": "margin"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["margin"], "56px", "{r}");
}

/// `Pyro.run`: mocks answer requests from the context (json/body, delay for a
/// loading state, abort for a failure, `times`), `url` waits for a route,
/// `reload` reloads, and the element steps hover, dblclick, blur, type,
/// css/focused/name checks and reads.
#[test]
fn driver_run_mocks_requests_and_covers_the_new_element_steps() {
    let address = driver_site();
    let mut h = Harness::new();
    // Element steps on a data: document.
    load_html(&mut h, DRIVER_STEPS_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#cell"}, "action": "hover"}},
            {"check": {"locator": {"css": "#tip"}, "assert": "visible"}},
            {"read": {"locator": {"css": "#cell"}, "read": "css", "property": "display", "as": "display"}},
            {"check": {"locator": {"css": "#box"}, "assert": "css", "property": "color", "expected": "rgb(1, 2, 3)"}},
            {"check": {"locator": {"css": "#box"}, "assert": "css", "property": "width", "expected": "120px"}},
            {"act": {"locator": {"label": "Title"}, "action": "type", "value": "Hi 2"}},
            {"check": {"locator": {"label": "Title"}, "assert": "focused"}},
            {"check": {"locator": {"label": "Title"}, "assert": "value", "expected": "Hi 2"}},
            {"check": {"locator": {"label": "Title"}, "assert": "name", "expected": "Title"}},
            {"read": {"locator": {"role": "button", "name": "Load items"}, "read": "name", "as": "name"}},
            {"act": {"locator": {"label": "Title"}, "action": "blur"}},
            {"check": {"locator": {"label": "Title"}, "assert": "focused", "expected": false}},
            {"check": {"locator": {"css": "#blur-out"}, "assert": "text", "expected": "blurred:Hi 2"}},
            {"act": {"locator": {"css": "#dbl"}, "action": "dblclick"}},
            {"check": {"locator": {"css": "#dbl-out"}, "assert": "text", "expected": "dbl:2"}},
            {"read": {"locator": {"css": "#cell"}, "read": "focused"}},
        ]}),
    );
    let result = &r["result"];
    assert_eq!(result["reads"]["display"], "block", "{r}");
    assert_eq!(result["reads"]["name"], "Load items", "{r}");
    let results = result["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(results.len(), 16, "{r}");
    assert_eq!(results[15], false, "{r}");
    let r = h.page("Pyro.query", json!({"locator": {"css": "#cell"}, "read": "css", "property": "display"}));
    assert_eq!(r["result"]["value"], "block", "{r}");
    let r =
        h.page("Pyro.run", json!({"steps": [{"check": {"locator": {"css": "#box"}, "assert": "css", "expected": "x"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("css needs a property"), "{r}");

    // Mocks on the site: a json mock, then a delayed one (loading state
    // visible), then an abort (error state), then `times` and `unmock`.
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/form")}},
            {"url": {"url": "/form"}},
            {"route": {"url": "/positions/7"}},
            {"url": {"url": {"regex": "/positions/\\d+$"}}},
            {"reload": {}},
            {"check": {"locator": {"css": "#path"}, "assert": "text", "expected": ""}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert!(results[3]["url"].as_str().unwrap().ends_with("/positions/7"), "{r}");
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"url": {"url": "/nowhere"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("url is"), "{r}");

    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"mock": {"url": "/api?name=", "json": {"greeting": "Mocked"}, "headers": {"x-mock": "1"}}},
            {"act": {"locator": {"label": "Name"}, "action": "fill", "value": "Ada"}},
            {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click"}},
            {"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "Mocked"}},
            {"request": {"path": "/api", "status": [200, 304]}},
            {"unmock": {"url": "/api?name="}},
            {"mock": {"url": "/api?name=", "status": 503, "body": "down", "times": 1}},
            {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click"}},
            {"request": {"path": "/api", "status": 503}},
            {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click"}},
            {"request": {"path": "/api", "count": 3}},
            {"mock": {"url": "/api?name=", "abort": "ConnectionRefused"}},
            {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click"}},
            {"mock": {"url": "/api?name=", "json": {"greeting": "Slow"}, "delay": 300}},
            {"act": {"locator": {"role": "button", "name": "Send"}, "action": "click", "settle": false}},
            {"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "Slow"}},
            {"unmock": {}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(results[0]["mocks"], 1, "{r}");
    assert_eq!(results[4]["status"], 200, "{r}");
    assert_eq!(results[5]["removed"], 1, "{r}");
    assert_eq!(results[8]["status"], 503, "{r}");
    assert_eq!(results[10]["count"], 3, "{r}");
    assert!(results[15]["actual"] == "Slow", "{r}");
    let requests = r["result"]["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
    let api: Vec<_> = requests.iter().filter(|q| q["url"].as_str().is_some_and(|u| u.contains("/api?name="))).collect();
    assert!(api.len() >= 5, "{r}");
    assert!(
        api.iter()
            .any(|q| q["error"].as_str().is_some_and(|e| e.contains("CONNECTION_REFUSED")) || q["failed"] == true),
        "{r}"
    );
    assert_eq!(results[16]["removed"], 2, "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"mock": {"url": "/x", "body": "a", "json": 1}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("body or json"), "{r}");
}

/// Serves `/` (a page whose `call(url, init)` fetches and shows the response
/// text) and `/api` (any method: a JSON reply echoing the method, the
/// request body and the `x-idempotency-key` header; `?slow` answers after
/// 300 ms) until the test ends.
fn driver_api_site() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                let head_end = loop {
                    if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
                let mut lines = head.lines();
                let line = lines.next().unwrap_or_default().to_owned();
                let header = |name: &str| {
                    lines.clone().find_map(|l| {
                        let (n, v) = l.split_once(':')?;
                        n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                    })
                };
                let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                while request.len() < head_end + length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let body = String::from_utf8_lossy(&request[head_end..head_end + length]).into_owned();
                let mut parts = line.split(' ');
                let method = parts.next().unwrap_or("GET").to_owned();
                let path = parts.next().unwrap_or("/").to_owned();
                let key = header("x-idempotency-key").unwrap_or_default();
                let (ty, reply) = if path.starts_with("/api") {
                    if path.contains("slow") {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    (
                        "application/json",
                        json!({"total": 1, "items": ["a"], "method": method, "body": body}).to_string(),
                    )
                } else {
                    ("text/html", DRIVER_API_PAGE.to_owned())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nX-Echo-Key: {key}\r\nConnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(reply.as_bytes());
            });
        }
    });
    address
}

const DRIVER_API_PAGE: &str = r#"<!doctype html><body><p id="out"></p>
<script>
  window.call = (url, init) => fetch(url, init).then((r) => r.text()).then((t) => {
    document.getElementById('out').textContent = t;
    return t;
  }).catch((e) => { document.getElementById('out').textContent = 'failed: ' + e; });
</script>"#;

const PATCH_CALL: &str = "call('/api', {method: 'PATCH', headers: {'x-idempotency-key': 'k1', \
                          'content-type': 'application/json'}, body: JSON.stringify({remarks: 'x'})}); 1";

/// `Pyro.run`'s request log carries each request's headers and body, and
/// `request` steps match on them (`body` pattern, `bodyJson`, `bodyKeys`,
/// `headers`).
#[test]
fn driver_run_requests_carry_headers_and_bodies() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"eval": {"expression": PATCH_CALL}},
            {"request": {"method": "PATCH", "path": "/api", "bodyKeys": ["remarks"]}},
            {"request": {"method": "PATCH", "path": "/api", "body": {"regex": "\"remarks\""}}},
            {"request": {"method": "PATCH", "path": "/api", "body": "remarks"}},
            {"request": {"method": "PATCH", "path": "/api", "bodyJson": {"remarks": "x"}}},
            {"request": {"method": "PATCH", "path": "/api", "headers": {"X-Idempotency-Key": "k1"}}},
            {"request": {"method": "PATCH", "path": "/api", "headers": {"x-idempotency-key": {"regex": "^k\\d$"}}}},
            {"request": {"path": "/api", "bodyKeys": ["other"], "count": 0}},
            {"request": {"path": "/api", "bodyJson": {"remarks": "y"}, "count": 0}},
            {"request": {"path": "/api", "headers": {"x-idempotency-key": "k2"}, "count": 0}},
            {"request": {"path": "/api", "body": "nowhere", "count": 0}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    let req = &results[2];
    assert_eq!(req["status"], 200, "{r}");
    assert_eq!(req["postData"], "{\"remarks\":\"x\"}", "{r}");
    assert_eq!(req["requestHeaders"]["x-idempotency-key"], "k1", "{r}");
    assert_eq!(req["requestHeaders"]["content-type"], "application/json", "{r}");
    assert_eq!(req["responseHeaders"]["x-echo-key"], "k1", "{r}");
    assert_eq!(req["responseHeaders"]["content-type"], "application/json", "{r}");
    assert!(req.get("postDataBase64").is_none() && req.get("postDataTruncated").is_none(), "{r}");
    let requests = r["result"]["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
    let api = requests.iter().find(|q| q["method"] == "PATCH").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(api["postData"], "{\"remarks\":\"x\"}", "{r}");
    assert_eq!(api["requestHeaders"]["x-idempotency-key"], "k1", "{r}");
    let document = requests.iter().find(|q| q["method"] == "GET").unwrap_or_else(|| panic!("{r}"));
    assert!(document.get("postData").is_none(), "{r}");
    assert!(document["requestHeaders"]["user-agent"].is_string(), "{r}");

    let r = h.page(
        "Pyro.run",
        json!({"timeout": 100, "steps": [{"request": {"method": "PATCH", "path": "/api", "bodyKeys": ["other"]}}]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("no matching request yet"), "{r}");
}

/// `request` with `state: "started"` counts requests as soon as they are
/// sent: a request a delayed mock holds is counted once (and only once,
/// before and after it finishes), while the default waits for it to finish.
#[test]
fn driver_run_counts_started_requests() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let start = Instant::now();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/api", "json": {"total": 5}, "delay": 400}},
            {"eval": {"expression": "call('/api?x'); 1"}},
            {"request": {"path": "/api", "count": 1, "state": "started"}},
            {"request": {"path": "/api", "state": "started"}},
            {"request": {"path": "/api", "count": 1}},
            {"request": {"path": "/api", "count": 1, "state": "started"}},
            {"request": {"path": "/api", "count": 1, "state": "finished"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "5"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert!(start.elapsed() >= Duration::from_millis(400), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(results[3]["count"], 1, "{r}");
    assert!(results[4]["status"].is_null() && results[4]["url"].as_str().unwrap().ends_with("/api?x"), "{r}");
    assert_eq!(results[5]["count"], 1, "{r}");
    let requests = r["result"]["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(requests.iter().filter(|q| q["url"].as_str().is_some_and(|u| u.contains("/api"))).count(), 1, "{r}");

    // A started count stays unmet (not over-met) when nothing was sent.
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 100, "steps": [{"request": {"path": "/api", "count": 1, "state": "started"}}]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("0 matching requests started"), "{r}");
}

const OUT_TEXT: &str = "document.getElementById('out').textContent";

/// A `passthrough` mock lets the request reach the real backend and
/// rewrites its response with `patch` (status, headers, a JSON merge patch;
/// `patch` alone implies passthrough).
#[test]
fn driver_run_passthrough_mock_patches_the_real_response() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/api", "passthrough": true, "times": 1,
                      "patch": {"status": 201, "headers": {"x-patched": "yes"}, "json": {"total": 999, "items": null}}}},
            {"eval": {"expression": "call('/api?a'); 1"}},
            {"request": {"path": "/api"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "999"}},
            {"eval": {"expression": OUT_TEXT}},
            // Used up: the next request gets the real response.
            {"eval": {"expression": "call('/api?b'); 1"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "\"total\":1"}},
            {"eval": {"expression": OUT_TEXT}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(results[3]["status"], 201, "{r}");
    assert_eq!(results[3]["responseHeaders"]["x-patched"], "yes", "{r}");
    let patched: Value = serde_json::from_str(results[5].as_str().unwrap_or_else(|| panic!("{r}"))).unwrap();
    // Merged into the real body: `total` replaced, `items` removed, the rest kept.
    assert_eq!(patched, json!({"total": 999, "method": "GET", "body": ""}), "{r}");
    let real: Value = serde_json::from_str(results[8].as_str().unwrap_or_else(|| panic!("{r}"))).unwrap();
    assert_eq!(real["items"], json!(["a"]), "{r}");

    // A plain passthrough answers like the backend; `patch.body` replaces it.
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            // `patch` implies `passthrough`.
            {"mock": {"url": "/api", "patch": {"body": "replaced"}}},
            {"mock": {"url": "/api?plain", "passthrough": true}},
            {"eval": {"expression": "call('/api?plain'); 1"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "\"total\":1"}},
            {"eval": {"expression": "call('/api?other'); 1"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "replaced"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"mock": {"url": "/api", "json": {}, "patch": {"status": 500}}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("not a response"), "{r}");
}

/// `hold` keeps a request (or, with `passthrough`, its real response) until
/// a `release` step names the hold.
#[test]
fn driver_run_held_mock_finishes_only_after_release() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let still_held = |h: &mut Harness, expected: &str| {
        let r = h.page(
            "Pyro.run",
            json!({"timeout": 300, "steps": [
                {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": expected}},
            ]}),
        );
        assert!(r.get("error").is_some(), "should still be held: {r}");
    };
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/api", "json": {"total": 7}, "hold": "save"}},
            {"eval": {"expression": "call('/api?held'); 1"}},
            {"request": {"path": "/api", "count": 1, "state": "started"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    still_held(&mut h, "7");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"release": {"hold": "save"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "7"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["results"][0]["released"], 1, "{r}");

    // A held real response: the page gets the backend's answer on release.
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"unmock": {}},
            {"mock": {"url": "/api", "passthrough": true, "hold": "resp"}},
            {"eval": {"expression": "call('/api?real'); 1"}},
            {"request": {"path": "/api", "count": 1, "state": "started"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    still_held(&mut h, "\"total\":1");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"release": {"hold": "resp"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "\"total\":1"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");

    // Nothing held: release waits, then times out.
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"release": {"hold": "none"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("nothing is held"), "{r}");
}

/// URL and request patterns are ECMAScript regexes: look-ahead, negative
/// look-ahead and backreferences work as in a JS `RegExp`.
#[test]
fn driver_run_patterns_are_javascript_regexes() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": {"regex": "/api(?!\\?real)"}, "json": {"total": 42}}},
            {"eval": {"expression": "call('/api?mocked'); 1"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "42"}},
            {"eval": {"expression": "call('/api?real'); 1"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "\"total\":1"}},
            {"eval": {"expression": "call('/api', {method: 'POST', body: 'ab-ab'}); 1"}},
            {"request": {"url": {"regex": "/api(?=\\?real$)"}, "count": 1}},
            {"request": {"url": {"regex": "/API(?!\\?)", "flags": "i"}, "count": 1}},
            {"request": {"body": {"regex": "^(\\w+)-\\1$"}, "count": 1}},
            {"url": {"url": {"regex": "^http://(?<host>[^/]+)/$"}}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"request": {"url": {"regex": "a(?<=", "flags": ""}}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("invalid regex"), "{r}");
}

const DRIVER_BATCH2_PAGE: &str = r#"<!doctype html><body>
<iframe id="fr" srcdoc="<button id='inner'>Inner</button><p id='inner-out'></p><script>document.getElementById('inner').addEventListener('click', () => { document.getElementById('inner-out').textContent = 'inner clicked'; });</script>"></iframe>
<input id="file" type="file" multiple><p id="file-out"></p>
<div id="src" draggable="true">Drag me</div><div id="dst">Drop here</div><p id="drag-out"></p>
<div id="psrc">Pointer</div><div id="pdst">Pointer target</div><p id="pointer-out"></p>
<a id="dl" href="/export.csv" download="report.csv">Export</a>
<button id="open">Open</button>
<script>
  const f = document.getElementById('file');
  f.addEventListener('change', () => { document.getElementById('file-out').textContent = [...f.files].map((x) => x.name + ':' + x.type + ':' + x.size).join(','); });
  const src = document.getElementById('src'), dst = document.getElementById('dst');
  src.addEventListener('dragstart', (e) => { e.dataTransfer.setData('text/plain', 'payload'); });
  dst.addEventListener('dragover', (e) => e.preventDefault());
  dst.addEventListener('drop', (e) => { e.preventDefault(); document.getElementById('drag-out').textContent = 'dropped:' + e.dataTransfer.getData('text/plain'); });
  const psrc = document.getElementById('psrc'); let down = false;
  psrc.addEventListener('mousedown', () => { down = true; });
  document.addEventListener('mouseup', (e) => { if (down) { down = false; document.getElementById('pointer-out').textContent = 'released on ' + e.target.id; } });
  document.getElementById('open').addEventListener('click', () => { window.open('/popup', '_blank', 'noopener'); });
</script>"#;

/// `Pyro.run` batch 2: locators scoped to an iframe's document, `upload`
/// (inline files and a path), `drag` (native drag and drop, and pointer
/// based), `download` (the `<a download>` body, saved on request), and
/// `popup` (the target id of a window the page opened).
#[test]
fn driver_run_frames_upload_drag_download_and_popup() {
    let address = driver_site();
    let mut h = Harness::new();
    let dir = std::env::temp_dir().join(format!("pyro-driver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();
    load_html(&mut h, DRIVER_BATCH2_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#inner", "frame": {"css": "#fr"}}, "action": "click"}},
            {"check": {"locator": {"css": "#inner-out", "frame": {"css": "#fr"}}, "assert": "text", "expected": "inner clicked"}},
            {"read": {"locator": {"role": "button", "frame": {"css": "#fr"}}, "read": "count", "as": "innerButtons"}},
            {"act": {"locator": {"css": "#file"}, "action": "upload",
                     "files": [{"name": "a.csv", "text": "x,y"}, {"name": "b.bin", "base64": "AAEC", "mimeType": "application/octet-stream"}]}},
            {"check": {"locator": {"css": "#file-out"}, "assert": "text", "expected": "a.csv:text/csv:3,b.bin:application/octet-stream:3"}},
            {"act": {"locator": {"css": "#file"}, "action": "upload", "value": dir.join("notes.txt").to_string_lossy()}},
            {"check": {"locator": {"css": "#file-out"}, "assert": "text", "expected": "notes.txt:text/plain:5"}},
            {"act": {"locator": {"css": "#src"}, "action": "drag", "to": {"css": "#dst"}}},
            {"check": {"locator": {"css": "#drag-out"}, "assert": "text", "expected": "dropped:payload"}},
            {"act": {"locator": {"css": "#psrc"}, "action": "drag", "to": {"css": "#pdst"}}},
            {"check": {"locator": {"css": "#pointer-out"}, "assert": "text", "expected": "released on pdst"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(r["result"]["reads"]["innerButtons"], 1, "{r}");
    assert_eq!(results[3]["files"], 2, "{r}");
    assert_eq!(results[7]["native"], true, "{r}");
    assert_eq!(results[9]["native"], false, "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"act": {"locator": {"css": "#file"}, "action": "upload"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("upload needs"), "{r}");
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"act": {"locator": {"css": "#inner", "frame": {"css": "#missing"}}, "action": "click"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("matched no frame"), "{r}");

    // Download and popup on the site (http URLs).
    let r = h.page("Pyro.run", json!({"steps": [{"goto": {"url": format!("http://{address}/batch2")}}]}));
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page("Page.navigate", json!({"url": html_url(DRIVER_BATCH2_PAGE)}));
    assert!(r.get("error").is_none(), "{r}");
    let _ = h.page("Pyro.run", json!({"steps": [{"loaded": {}}]}));
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"role": "link", "name": "Export"}, "action": "click", "settle": false}},
            {"download": {"saveTo": dir.to_string_lossy(), "as": "file"}},
            {"act": {"locator": {"role": "button", "name": "Open"}, "action": "click", "settle": false}},
            {"popup": {"as": "popup"}},
        ]}),
    );
    // A data: document cannot download from a relative URL; the site can.
    let message = r["error"]["message"].as_str().unwrap_or_default().to_owned();
    if !message.is_empty() {
        assert!(message.contains("download"), "{r}");
    }
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/batch2")}},
            {"eval": {"expression": "document.body.innerHTML = `<a id=dl href='/export.csv' download='report.csv'>Export</a><button id=open>Open</button>`; document.getElementById('open').addEventListener('click', () => { window.open('/popup', '_blank', 'noopener'); }); null"}},
            {"act": {"locator": {"role": "link", "name": "Export"}, "action": "click", "settle": false}},
            {"download": {"saveTo": dir.to_string_lossy(), "as": "file"}},
            {"act": {"locator": {"role": "button", "name": "Open"}, "action": "click", "settle": false}},
            {"popup": {"as": "popup"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let file = &r["result"]["reads"]["file"];
    assert_eq!(file["filename"], "report.csv", "{r}");
    assert!(file["url"].as_str().unwrap().ends_with("/export.csv"), "{r}");
    assert_eq!(file["path"], dir.join("report.csv").to_string_lossy().as_ref(), "{r}");
    assert!(std::fs::read_to_string(dir.join("report.csv")).unwrap().contains("<!doctype html>"), "{r}");
    let popup = &r["result"]["reads"]["popup"];
    assert!(popup["targetId"].as_str().is_some_and(|t| t.len() == 32), "{r}");
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"popup": {}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("no popup yet"), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs the engine until every response in `ids` arrived; the replies by id.
fn until_responses(h: &mut Harness, ids: &[u64]) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        seen.extend(h.drain());
        if ids.iter().all(|id| seen.iter().any(|m| m["id"] == *id)) {
            return ids.iter().map(|id| seen.iter().find(|m| m["id"] == *id).unwrap().clone()).collect();
        }
        assert!(Instant::now() < deadline, "responses missing; saw {seen:#?}");
        let host = h.browser.host_mut();
        host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        host.flush();
        let mut sink = h.out.clone();
        h.browser.adopt_popups(&mut sink);
    }
}

/// A prepared page taken the moment its profile is registered (`ready:
/// false`) is still fetching its document: `loaded` waits for that
/// navigation rather than for the `about:blank` it starts on, so the
/// `route` after it lands on the profile's origin. Several profiles taken
/// at once all get there. A `route` on a page that never navigated fails
/// with a clear message instead of `pushState` throwing.
#[test]
fn driver_run_loaded_waits_for_a_prepared_pages_navigation() {
    let address = driver_site();
    let mut h = Harness::new();
    let mut sessions = Vec::new();
    for i in 0..5 {
        let r = h.call(
            None,
            "Pyro.registerPreparedPage",
            json!({"url": format!("http://{address}/form?p={i}"), "pool": 1}),
        );
        let profile = r["result"]["profileId"].as_u64().unwrap_or_else(|| panic!("{r}"));
        let r = h.call(None, "Pyro.acquirePreparedPage", json!({"profileId": profile, "attach": true}));
        assert_eq!(r["result"]["ready"], false, "{r}");
        sessions.push(r["result"]["sessionId"].as_str().unwrap_or_else(|| panic!("{r}")).to_owned());
    }
    let ids: Vec<u64> = sessions
        .iter()
        .enumerate()
        .map(|(i, session)| {
            h.send(
                Some(session),
                "Pyro.run",
                json!({"steps": [
                    {"loaded": {}},
                    {"route": {"url": format!("/positions/{i}")}},
                    {"check": {"locator": {"css": "#path"}, "assert": "text", "expected": format!("/positions/{i}")}},
                    {"url": {"url": format!("/positions/{i}")}},
                ]}),
            )
        })
        .collect();
    for (i, r) in until_responses(&mut h, &ids).iter().enumerate() {
        assert!(r.get("error").is_none(), "{r}");
        let url = r["result"]["results"][3]["url"].as_str().unwrap_or_else(|| panic!("{r}"));
        assert_eq!(url, format!("http://{address}/positions/{i}"), "{r}");
    }
    // The harness page never navigated: no document to route in.
    let r = h.page("Pyro.run", json!({"steps": [{"route": {"url": "/x"}}]}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 0 (route /x)") && message.contains("about:blank"), "{r}");
}

/// `eval` awaits a returned promise (as `page.evaluate` does): the settled
/// value is the step's result and read; a rejection fails the step with
/// its message; a promise that never settles times out with the step's
/// own timeout. `route` with `settle: false` returns while the request it
/// started is still pending; `check` `enabled` reads the disabled state.
#[test]
fn driver_run_eval_awaits_promises_and_route_can_skip_settling() {
    let address = driver_site();
    let mut h = Harness::new();
    let start = Instant::now();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/form")}},
            {"eval": {"expression": "new Promise(r => setTimeout(() => r({n: 1}), 150))", "as": "x"}},
            {"eval": {"expression": "Promise.resolve('now')", "as": "y"}},
            {"eval": {"expression": "7", "as": "z"}},
            {"check": {"locator": {"role": "button", "name": "Send"}, "assert": "enabled"}},
            {"mock": {"url": "/api?name=", "json": {"greeting": "Slow"}, "delay": 400}},
            {"route": {"url": "/positions/9", "settle": false, "timeout": 1000}},
            {"check": {"locator": {"css": "#path"}, "assert": "text", "expected": "/positions/9"}},
            {"read": {"locator": {"css": "#out"}, "read": "text", "as": "out"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let result = &r["result"];
    assert_eq!(result["reads"]["x"], json!({"n": 1}), "{r}");
    assert_eq!(result["reads"]["y"], "now", "{r}");
    assert_eq!(result["reads"]["z"], 7, "{r}");
    assert_eq!(result["results"][1], json!({"n": 1}), "{r}");
    assert_eq!(result["results"][4]["actual"], true, "{r}");
    assert!(result["results"][6].get("settleMs").is_none(), "route did not settle: {r}");
    assert_eq!(result["reads"]["out"], "", "the mocked response is still pending: {r}");
    assert!(start.elapsed() < Duration::from_millis(400), "{:?}", start.elapsed());
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "Slow"}}]}),
    );
    assert!(r.get("error").is_none(), "{r}");

    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "Promise.reject(new Error('nope'))"}}]}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("step 0 (eval)") && message.contains("rejected: nope"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "new Promise(() => {})", "timeout": 60}}]}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("timed out after 60 ms"), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"check": {"locator": {"role": "button", "name": "Send"}, "assert": "enabled", "expected": false, "timeout": 50}}]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("expected Enabled false, got true"), "{r}");
}

/// `text` locators follow Playwright's text engine: the scope element
/// itself is a candidate (`css('td').text('Domestic', exact)` matches
/// `<td>Domestic</td>`). With `legacyText` (Playwright's `text="…"`) an
/// exact string also matches an element one of whose own text-node runs
/// equals it, child elements aside (`<span><span>|</span>TS-106-POSITION</span>`);
/// `getByText` exact compares the whole text only.
#[test]
fn driver_text_locators_match_the_scope_root_and_own_text_nodes() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<table><tr><td>Domestic</td><td>Domestic flights</td></tr></table>
<span id="pos"><span>|</span>TS-106-POSITION</span>
<p><b>bold</b> and plain</p>
<div id="enabled"><button>Go</button><button disabled>Stop</button></div>"#,
    );
    let q = |h: &mut Harness, locator: Value| h.page("Pyro.query", json!({"locator": locator, "read": "text"}));
    let r = q(&mut h, json!({"text": "Domestic", "exact": true, "within": {"css": "td"}}));
    assert_eq!(r["result"]["value"], "Domestic", "{r}");
    let r = q(&mut h, json!({"text": "Domestic", "exact": true}));
    assert_eq!(r["result"]["value"], "Domestic", "{r}");
    let r = h.page("Pyro.query", json!({"locator": {"text": "Domestic"}, "read": "count"}));
    assert_eq!(r["result"]["value"], 2, "{r}");
    let r = q(&mut h, json!({"text": "TS-106-POSITION", "exact": true, "legacyText": true}));
    assert_eq!(r["result"]["value"], "|TS-106-POSITION", "{r}");
    let r = q(&mut h, json!({"text": "TS-106-POSITION", "exact": true, "legacyText": true, "within": {"css": "#pos"}}));
    assert_eq!(r["result"]["value"], "|TS-106-POSITION", "{r}");
    let r = h.page("Pyro.query", json!({"locator": {"text": "TS-106-POSITION", "exact": true}, "read": "count"}));
    assert_eq!(r["result"]["value"], 0, "{r}");
    // The whole text still wins, and own text nodes do not match substrings.
    let r = q(&mut h, json!({"text": "bold and plain", "exact": true}));
    assert_eq!(r["result"]["value"], "bold and plain", "{r}");
    let r = h.page("Pyro.query", json!({"locator": {"text": "and", "exact": true}, "read": "count"}));
    assert_eq!(r["result"]["value"], 0, "{r}");
    let r = h.page("Pyro.query", json!({"locator": {"role": "button", "name": "Stop"}, "read": "enabled"}));
    assert_eq!(r["result"]["value"], false, "{r}");
    let r = h.page("Pyro.expect", json!({"locator": {"role": "button", "name": "Go"}, "assert": "enabled"}));
    assert_eq!(r["result"]["actual"], true, "{r}");
}

/// Runs one `read` step per `(name, read, locator)` and returns the reads.
fn run_reads(h: &mut Harness, reads: &[(&str, &str, serde_json::Value)]) -> serde_json::Value {
    let steps: Vec<serde_json::Value> = reads
        .iter()
        .map(|(name, read, locator)| json!({"read": {"locator": locator, "read": read, "as": name}}))
        .collect();
    let r = h.page("Pyro.run", json!({"steps": steps}));
    assert!(r.get("error").is_none(), "{r}");
    r["result"]["reads"].clone()
}

/// Implicit roles follow Playwright's `roleUtils.ts`: `<section>` and
/// `<form>` need an aria name, `<header>`/`<footer>` are landmarks only
/// outside sectioning content, `<img alt="">` is presentational unless
/// titled or aria-attributed, `<th scope>` and `<td>` in a grid pick their
/// header/cell roles, `<select>` is a listbox when multiple or sized, a
/// search input with a list is a combobox, and the static tag roles exist.
#[test]
fn driver_implicit_roles_follow_playwright() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<section aria-label="ProbeA"><p>a</p></section><section><p>plain</p></section>
<div role="region" aria-label="ProbeB"></div><section aria-labelledby="t"><h2 id="t">Titled</h2></section>
<header>Top</header><footer>Bottom</footer>
<article><header>Inner</header><footer>Inner foot</footer></article>
<form aria-label="Login"></form><form></form>
<img alt="" src="x"><img alt="" title="Pic" src="y"><img alt="Photo" src="z"><img alt="" aria-label="Icon" src="w">
<a href="/a">Link</a><a>Not a link</a>
<table><tr><th>Col</th><th scope="row">Row</th><td>cell</td></tr></table>
<table role="grid"><tr><td>g</td></tr></table>
<select><option>a</option></select><select multiple><option>b</option></select><select size="3"><option>c</option></select>
<input type="search"><input type="search" list="dl"><input list="dl"><datalist id="dl"></datalist>
<hr><fieldset><legend>Legend</legend></fieldset><p>para</p>
"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("regions", "count", json!({"role": "region"})),
            ("probeA", "count", json!({"role": "region", "name": "ProbeA"})),
            ("banner", "count", json!({"role": "banner"})),
            ("contentinfo", "count", json!({"role": "contentinfo"})),
            ("form", "count", json!({"role": "form"})),
            ("presentation", "count", json!({"role": "presentation"})),
            ("img", "count", json!({"role": "img"})),
            ("link", "count", json!({"role": "link"})),
            ("columnheader", "count", json!({"role": "columnheader"})),
            ("rowheader", "count", json!({"role": "rowheader"})),
            ("cell", "count", json!({"role": "cell"})),
            ("gridcell", "count", json!({"role": "gridcell"})),
            ("listbox", "count", json!({"role": "listbox"})),
            ("combobox", "count", json!({"role": "combobox"})),
            ("searchbox", "count", json!({"role": "searchbox"})),
            ("textbox", "count", json!({"role": "textbox"})),
            ("separator", "count", json!({"role": "separator"})),
            ("group", "count", json!({"role": "group"})),
            ("paragraph", "count", json!({"role": "paragraph"})),
        ],
    );
    let want = json!({
        "regions": 3, "probeA": 1, "banner": 1, "contentinfo": 1, "form": 1, "presentation": 1, "img": 3,
        "link": 1, "columnheader": 1, "rowheader": 1, "cell": 1, "gridcell": 1, "listbox": 2,
        "combobox": 3, "searchbox": 1, "textbox": 0, "separator": 1, "group": 1, "paragraph": 3,
    });
    assert_eq!(reads, want);
}

/// Names from content put a space around block-level (and flex-item,
/// `<br>`) children, as Chromium does, and none around inline ones.
#[test]
fn driver_accessible_name_spaces_block_children() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<label style="display:flex" for="a">Flex<span>*</span></label><input id="a">
<label for="b">Block<div>*</div></label><input id="b">
<label for="c">Inline<span>*</span></label><input id="c">
<button id="br">Line<br>break</button>
<button id="deep"><span>Save</span><span style="display:inline-block">now</span></button>
"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("flex", "name", json!({"css": "#a"})),
            ("block", "name", json!({"css": "#b"})),
            ("inline", "name", json!({"css": "#c"})),
            ("br", "name", json!({"css": "#br"})),
            ("deep", "name", json!({"css": "#deep"})),
            ("byName", "count", json!({"role": "textbox", "name": "Flex *", "exact": true})),
        ],
    );
    let want = json!({"flex": "Flex *", "block": "Block *", "inline": "Inline*", "br": "Line break", "deep": "Save now", "byName": 1});
    assert_eq!(reads, want);
}

/// `aria-labelledby`: a referenced node (even the element itself) gives its
/// `aria-label` before its contents (accname 2C/2D), duplicate IDREFs count
/// once, an embedded control contributes its value, and hidden references
/// are still read.
#[test]
fn driver_accessible_name_labelledby_uses_aria_label_and_dedupes() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<div role="spinbutton" id="a" aria-label="day, " aria-labelledby="a b">15</div><span id="b">X</span>
<button id="bl" aria-label="Btn label">x</button><section role="dialog" aria-labelledby="bl"></section>
<span id="l">All Charges</span><div role="radiogroup" aria-labelledby="l l"></div>
<div id="h" style="display:none">Hidden label</div><div role="group" id="g" aria-labelledby="h"></div>
<div id="v">Amount <input id="n" value="5"></div><div role="slider" id="s" aria-labelledby="v"></div>
"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("spin", "name", json!({"css": "#a"})),
            ("dialog", "name", json!({"role": "dialog"})),
            ("radiogroup", "name", json!({"role": "radiogroup"})),
            ("hidden", "name", json!({"css": "#g"})),
            ("embedded", "name", json!({"css": "#s"})),
            ("exact", "count", json!({"role": "radiogroup", "name": "All Charges", "exact": true})),
        ],
    );
    let want = json!({"spin": "day, X", "dialog": "Btn label", "radiogroup": "All Charges", "hidden": "Hidden label", "embedded": "Amount 5", "exact": 1});
    assert_eq!(reads, want);
}

/// Elements in an `inert` or `aria-hidden` subtree are hidden for ARIA: role
/// locators skip them and names from content do not read them.
#[test]
fn driver_role_locators_skip_aria_hidden_but_not_inert_subtrees() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<div inert><table><thead><tr><th>Name</th></tr></thead></table><button>Go</button></div>
<div aria-hidden="true"><button>Go</button></div>
<div aria-hidden="TRUE"><button>Go</button></div>
<table><thead><tr><th>Name</th></tr></thead></table><button>Go <span aria-hidden="true">(x)</span><span inert>(y)</span></button>
"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("headers", "count", json!({"role": "columnheader", "name": "Name", "exact": true})),
            ("buttons", "count", json!({"role": "button"})),
        ],
    );
    // Playwright's isElementHiddenForAria has no inert rule (and
    // checkVisibility() ignores inert): inert content keeps its role and
    // its text stays in names; aria-hidden content does not.
    assert_eq!(reads["headers"], 2, "{reads}");
    assert_eq!(reads["buttons"], 2, "{reads}");
    let r = h.page("Pyro.query", json!({"locator": {"role": "button", "name": "Go (y)", "exact": true}, "read": "name"}));
    assert_eq!(r["result"]["value"], "Go (y)", "{r}");
}

/// `checked` honours `aria-checked` on checkbox-like roles (and `mixed`);
/// `check`/`uncheck` click such elements only when the state differs, and
/// both refuse elements that are not checkable.
#[test]
fn driver_checked_honours_aria_checked_roles() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<button id="r1" role="radio" aria-checked="true">With positions</button>
<button id="r2" role="radio" aria-checked="false">Without</button>
<div id="sw" role="switch" aria-checked="mixed" tabindex="0">Mixed</div>
<input id="cb" type="checkbox" checked>
<input id="ind" type="checkbox">
<button id="plain">Plain</button>
<script>
  document.getElementById('ind').indeterminate = true;
  for (const b of document.querySelectorAll('[role=radio]')) b.addEventListener('click', () => {
    for (const o of document.querySelectorAll('[role=radio]')) o.setAttribute('aria-checked', o === b ? 'true' : 'false');
  });
</script>
"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"check": {"locator": {"role": "radio", "name": "With positions"}, "assert": "checked"}},
            {"check": {"locator": {"role": "radio", "name": "Without"}, "assert": "checked", "expected": false}},
            {"check": {"locator": {"role": "switch"}, "assert": "checked", "expected": "mixed"}},
            {"check": {"locator": {"role": "switch"}, "assert": "checked", "expected": true, "not": true}},
            {"check": {"locator": {"css": "#cb"}, "assert": "checked"}},
            {"check": {"locator": {"css": "#ind"}, "assert": "checked", "expected": "mixed"}},
            {"act": {"locator": {"role": "radio", "name": "With positions"}, "action": "check"}},
            {"act": {"locator": {"role": "radio", "name": "Without"}, "action": "check"}},
            {"check": {"locator": {"css": "#r2"}, "assert": "checked"}},
            {"check": {"locator": {"css": "#r1"}, "assert": "checked", "expected": false}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page("Pyro.run", json!({"timeout": 100, "steps": [{"check": {"locator": {"css": "#plain"}, "assert": "checked", "expected": false}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("not a checkbox"), "{r}");
    let r = h
        .page("Pyro.run", json!({"timeout": 100, "steps": [{"act": {"locator": {"css": "#plain"}, "action": "check"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("not a checkbox"), "{r}");
}

/// A locator inside a `within` row of an iframe document stays in that row.
#[test]
fn driver_within_row_in_frame_scopes_inner_locator() {
    let mut h = Harness::new();
    let inner = "<table><tr><td>first<svg id=s1></svg></td></tr><tr><td>second<svg id=s2></svg><svg id=s3></svg></td></tr></table>";
    load_html(&mut h, &format!(r#"<!doctype html><body><iframe id="fr" srcdoc="{inner}"></iframe>"#));
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"read": {"locator": {"css": "svg", "frame": {"css": "#fr"}, "within": {"role": "row", "filter": {"hasText": "second"}}}, "read": "count", "as": "second"}},
            {"read": {"locator": {"css": "svg", "frame": {"css": "#fr"}, "within": {"role": "row", "filter": {"hasText": "first"}}}, "read": "count", "as": "first"}},
            {"read": {"locator": {"css": "svg", "frame": {"css": "#fr"}, "within": {"role": "row", "filter": {"has": {"text": "second"}}}}, "read": "count", "as": "has"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"], json!({"second": 2, "first": 1, "has": 2}), "{r}");
}

/// Danex 6: exact `text` is Playwright's `getByText(…, { exact: true })`:
/// the element's whole text, and only where no child element's text
/// matches too. `<span>Charges difference <span>+37.86 USD</span></span>`
/// does not match "Charges difference"; the `<dt>`'s inner span does.
/// Script and style text does not count; `<input type=submit>` counts its value.
#[test]
fn driver_exact_text_compares_the_whole_text() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body>
<span id="a">Charges difference <span>+37.86 USD</span></span>
<dl><dt><span id="b">Charges difference</span></dt></dl>
<p id="c">Total<script>var x = 1;</script></p>
<div id="d"><input type="submit" value="Go"></div>
<div id="e">X<p>Z<b>X</b></p></div>"#,
    );
    let count = |h: &mut Harness, locator: Value| {
        h.page("Pyro.query", json!({"locator": locator, "read": "count"}))["result"]["value"].clone()
    };
    assert_eq!(count(&mut h, json!({"text": "Charges difference", "exact": true})), 1);
    let r = h.page("Pyro.query", json!({"locator": {"text": "Charges difference", "exact": true}, "read": "box"}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(count(&mut h, json!({"text": "Charges difference", "exact": true, "within": {"css": "dt"}})), 1);
    assert_eq!(count(&mut h, json!({"text": "Charges difference"})), 2);
    assert_eq!(count(&mut h, json!({"text": "Total", "exact": true})), 1);
    assert_eq!(count(&mut h, json!({"text": "Go", "exact": true})), 1);
    // A regex: `<div id=e>` ("XZX") matches /^X/ while its child <p> ("ZX")
    // does not, so it is a match, and so is the <b>.
    assert_eq!(count(&mut h, json!({"text": {"regex": "^X"}, "within": {"css": "#e"}})), 2);
}

/// Danex 7: a second `hasText` narrows the first one's matches (as the SDK
/// sends it: the first filter's locator as the base set, `of`).
#[test]
fn driver_chained_filters_narrow() {
    let mut h = Harness::new();
    load_html(&mut h, r#"<!doctype html><body><ul><li>apple red</li><li>apple green</li><li>pear red</li></ul>"#);
    let count = |h: &mut Harness, locator: Value| {
        h.page("Pyro.query", json!({"locator": locator, "read": "count"}))["result"]["value"].clone()
    };
    let first = json!({"css": "li", "filter": {"hasText": "apple"}});
    assert_eq!(count(&mut h, json!({"of": first, "filter": {"hasText": "red"}})), 1);
    assert_eq!(count(&mut h, json!({"of": {"css": "li", "nth": 0}, "filter": {"hasText": "green"}})), 0);
}

/// Danex 2: `press` and `type` do not check that the element is enabled
/// (Playwright focuses it and sends the keys): an `aria-disabled` button
/// gets the keydown. `fill` and `click` still wait for an enabled element.
#[test]
fn driver_press_ignores_aria_disabled() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><title>start</title><button aria-disabled="true" onkeydown="document.title = 'pressed ' + event.key">Settled</button>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 2000, "steps": [
            {"act": {"locator": {"role": "button", "name": "Settled"}, "action": "press", "value": "Enter"}},
            {"eval": {"expression": "document.title", "as": "title"}},
            {"eval": {"expression": "document.activeElement.textContent", "as": "focused"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["title"], "pressed Enter", "{r}");
    assert_eq!(r["result"]["reads"]["focused"], "Settled", "{r}");
}

/// Danex 8: Escape in a filled `<input type=search>` empties it and fires
/// `input`, then `search` (Chromium's SearchInputType); an empty one lets
/// the key through.
#[test]
fn driver_escape_clears_a_search_field() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><input type="search" id="s"><div id="log"></div>
<script>
  const s = document.getElementById('s'), log = [];
  for (const t of ['input', 'search', 'change']) s.addEventListener(t, () => { log.push(t + ':' + s.value); document.getElementById('log').textContent = log.join(','); });
</script>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#s"}, "action": "fill", "value": "abc"}},
            {"act": {"locator": {"css": "#s"}, "action": "press", "value": "Escape"}},
            {"eval": {"expression": "[document.getElementById('s').value, document.getElementById('log').textContent]", "as": "after"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["after"], json!(["", "input:abc,input:,search:"]), "{r}");
}

/// Danex 9: `click({ force: true })` on a visually hidden checkbox (react-aria:
/// absolutely positioned, 1px, `clip: rect(0 0 0 0)`, `clip-path: inset(50%)`)
/// clicks the center of its box, as Playwright's force does.
#[test]
fn driver_force_click_on_a_visually_hidden_checkbox() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><label id=l style="display:inline-flex;gap:8px;margin:40px"><span style="position:absolute;overflow:hidden;clip:rect(0 0 0 0);clip-path:inset(50%);width:1px;height:1px;white-space:nowrap"><input type=checkbox id=c aria-label="Grant"></span><span>Grant</span></label>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 2000, "steps": [
            {"act": {"locator": {"css": "#c"}, "action": "click", "force": true}},
            {"check": {"locator": {"css": "#c"}, "assert": "checked"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
}

/// Native API gaps: RegExp expectations for text/containsText/value, the
/// attribute check, the page-level `keyboard` step, `:scope` in a scoped
/// CSS locator, heading `level`.
#[test]
fn driver_regex_checks_keyboard_step_scope_and_level() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><h1>Top</h1><h2>Sub</h2><p id="t">Order   #123 placed</p><input id="v" value="ID-42" data-state="open">
<ul id="list"><li>a<ul><li>nested</li></ul></li><li>b</li></ul><div id="keys"></div>
<script>document.addEventListener('keydown', (e) => { document.getElementById('keys').textContent += e.key + '@' + (document.activeElement && document.activeElement.id) + ' '; });</script>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 2000, "steps": [
            {"check": {"locator": {"css": "#t"}, "assert": "text", "expected": {"regex": "^Order #\\d+ placed$"}}},
            {"check": {"locator": {"css": "#t"}, "assert": "containsText", "expected": {"regex": "#1\\d3", "flags": "i"}}},
            {"check": {"locator": {"css": "#v"}, "assert": "value", "expected": {"regex": "^ID-\\d+$"}}},
            {"check": {"locator": {"css": "#v"}, "assert": "value", "expected": {"regex": "^\\d"}, "not": true}},
            {"check": {"locator": {"css": "#v"}, "assert": "attribute", "property": "data-state", "expected": "open"}},
            {"check": {"locator": {"css": "#v"}, "assert": "attribute", "property": "data-state", "expected": {"regex": "^op"}}},
            {"check": {"locator": {"css": "#v"}, "assert": "attribute", "property": "data-state", "expected": null, "not": true}},
            {"check": {"locator": {"css": "#v"}, "assert": "attribute", "property": "data-gone", "expected": null}},
            {"check": {"locator": {"role": "heading", "level": 2}, "assert": "text", "expected": "Sub"}},
            {"check": {"locator": {"css": ":scope > li", "within": {"css": "#list"}}, "assert": "count", "expected": 2}},
            {"act": {"locator": {"css": "#v"}, "action": "focus"}},
            {"keyboard": {"press": "Shift+x"}},
            {"act": {"locator": {"css": "#v"}, "action": "blur"}},
            {"keyboard": {"type": "ab"}},
            {"eval": {"expression": "document.getElementById('keys').textContent", "as": "keys"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["keys"], "Shift@v x@v a@ b@ ", "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"keyboard": {"press": "NoSuchKey"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("Unknown key"), "{r}");
}

const DRIVER_POINTER_PAGE: &str = r#"<!doctype html><body>
<style>#scroller{overflow:auto;width:1000px} #wide{width:2500px;height:26px} #tall{height:300px;width:200px;overflow:auto}</style>
<div id="target">Target</div><div id="target-out"></div>
<input id="dis" disabled><div id="dis-out"></div>
<div id="mi" role="menuitem" aria-disabled="true">Item</div>
<div id="pad" style="width:200px;height:100px"></div><div id="pad-out"></div>
<div style="position:relative;width:200px;height:50px"><button id="under">Under</button><div id="over" style="position:absolute;left:0;top:0;width:200px;height:50px"></div></div><div id="over-out"></div>
<div id="scroller"><div id="wide" ondblclick="this.textContent='ok'">row</div></div><div id="wide-out"></div>
<div id="tall"><div id="tall-inner" style="height:2000px">tall</div></div><div id="wheel-out"></div>
<div id="moves-out"></div>
<script>
  const t = document.getElementById('target'); const seen = [];
  for (const n of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'contextmenu', 'auxclick', 'dblclick']) {
    t.addEventListener(n, (e) => { seen.push(n + ':' + e.button); document.getElementById('target-out').textContent = seen.join(','); });
  }
  t.addEventListener('contextmenu', (e) => e.preventDefault());
  document.addEventListener('contextmenu', (e) => { document.body.dataset.cm = [e.cancelable, e.bubbles, e.composed, e.buttons, e instanceof PointerEvent, e.defaultPrevented].join(); });
  document.getElementById('dis').addEventListener('mouseover', () => { document.getElementById('dis-out').textContent = 'hovered'; });
  document.getElementById('mi').addEventListener('mouseover', () => { document.getElementById('mi').dataset.hovered = '1'; });
  const pad = document.getElementById('pad');
  pad.addEventListener('click', (e) => { const r = pad.getBoundingClientRect(); document.getElementById('pad-out').textContent = Math.round(e.clientX - r.left) + ',' + Math.round(e.clientY - r.top); });
  document.getElementById('over').addEventListener('click', () => { document.getElementById('over-out').textContent = 'over'; });
  document.getElementById('tall').addEventListener('wheel', (e) => { document.getElementById('wheel-out').textContent = 'wheel:' + e.deltaX + ',' + e.deltaY + ':' + e.target.id; });
  let moves = 0; document.addEventListener('mousemove', (e) => { moves += 1; document.body.dataset.mb = e.buttons; document.getElementById('moves-out').textContent = moves + '@' + e.clientX + ',' + e.clientY; });
  document.addEventListener('dblclick', (e) => { document.getElementById('wide-out').textContent = e.target.id + '@' + e.clientX; });
  document.getElementById('wide').addEventListener('dblclick', function () { this.textContent = 'ok'; });
</script>"#;

/// Gap 1: `click` with `button: "right"` presses and releases button 2,
/// fires `contextmenu` after the press (Chromium on Linux and macOS) and
/// `auxclick` after the release, never `click`; `middle` fires `auxclick`;
/// `dblclick` takes the button too.
#[test]
fn driver_run_right_click_fires_contextmenu() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_POINTER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#target"}, "action": "click", "button": "right"}},
            {"check": {"locator": {"css": "#target-out"}, "assert": "text",
                       "expected": "pointerdown:2,mousedown:2,contextmenu:2,pointerup:2,mouseup:2,auxclick:2"}},
            {"eval": {"expression": "document.body.dataset.cm", "as": "cm"}},
            {"act": {"locator": {"css": "#target"}, "action": "click", "button": "middle"}},
            {"check": {"locator": {"css": "#target-out"}, "assert": "containsText",
                       "expected": "auxclick:2,pointerdown:1,mousedown:1,pointerup:1,mouseup:1,auxclick:1"}},
            {"act": {"locator": {"css": "#target"}, "action": "dblclick", "button": "right"}},
            {"check": {"locator": {"css": "#target-out"}, "assert": "containsText",
                       "expected": "auxclick:1,pointerdown:2,mousedown:2,contextmenu:2,pointerup:2,mouseup:2,auxclick:2,pointerdown:2,mousedown:2,contextmenu:2,pointerup:2,mouseup:2,auxclick:2"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    // Cancelable, bubbling, composed, button 2 held, a PointerEvent; the
    // target's handler canceled it.
    assert_eq!(r["result"]["reads"]["cm"], "true,true,true,2,true,true", "{r}");
    let r = h
        .page("Pyro.run", json!({"steps": [{"act": {"locator": {"css": "#target"}, "action": "click", "button": "x"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("button must be"), "{r}");
}

/// Gap 18: `hover` does not need an enabled element (a disabled input, an
/// `aria-disabled` menu item); `click` still does.
#[test]
fn driver_run_hover_works_on_disabled_elements() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_POINTER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#dis"}, "action": "hover"}},
            {"check": {"locator": {"css": "#dis-out"}, "assert": "text", "expected": "hovered"}},
            {"act": {"locator": {"role": "menuitem", "name": "Item"}, "action": "hover"}},
            {"eval": {"expression": "document.getElementById('mi').dataset.hovered", "as": "mi"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["mi"], "1", "{r}");
    let r =
        h.page("Pyro.run", json!({"timeout": 50, "steps": [{"act": {"locator": {"css": "#dis"}, "action": "click"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("element is disabled"), "{r}");
}

/// Gap 11: the `mouse` step acts at viewport coordinates (`move` with
/// `steps` sends intermediate moves; `down`/`up`/`wheel` default to the
/// current position); locator actions take `position` (an offset from the
/// element's top-left) and `force` (no hit test).
#[test]
fn driver_run_mouse_step_at_coordinates_position_and_force() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_POINTER_PAGE);
    let r = h.page("Pyro.query", json!({"locator": {"css": "#pad"}, "read": "box"}));
    let (x, y) = (r["result"]["value"]["x"].as_f64().unwrap(), r["result"]["value"]["y"].as_f64().unwrap());
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"mouse": {"action": "click", "x": x + 10.0, "y": y + 20.0}},
            {"check": {"locator": {"css": "#pad-out"}, "assert": "text", "expected": "10,20"}},
            {"act": {"locator": {"css": "#pad"}, "action": "click", "position": {"x": 5, "y": 7}}},
            {"check": {"locator": {"css": "#pad-out"}, "assert": "text", "expected": "5,7"}},
            {"mouse": {"action": "move", "x": x + 30.0, "y": y + 30.0, "steps": 4}},
            {"mouse": {"action": "down"}},
            {"mouse": {"action": "move", "x": x + 30.0, "y": y + 30.0}},
            {"eval": {"expression": "document.body.dataset.mb", "as": "held"}},
            {"mouse": {"action": "up"}},
            {"check": {"locator": {"css": "#pad-out"}, "assert": "text", "expected": "30,30"}},
            {"read": {"locator": {"css": "#moves-out"}, "read": "text", "as": "moves"}},
            {"mouse": {"action": "dblclick", "x": x + 1.0, "y": y + 1.0, "button": "right"}},
            {"act": {"locator": {"css": "#under"}, "action": "click", "force": true}},
            {"check": {"locator": {"css": "#over-out"}, "assert": "text", "expected": "over"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let moves = r["result"]["reads"]["moves"].as_str().unwrap_or_default();
    // One move per click so far (2), the 4 steps, then the move with the
    // button held, ending at the target.
    assert_eq!(moves, format!("7@{},{}", x + 30.0, y + 30.0), "{r}");
    // A move between `down` and `up` reports the held left button.
    assert_eq!(r["result"]["reads"]["held"], "1", "{r}");
    let r =
        h.page("Pyro.run", json!({"timeout": 50, "steps": [{"act": {"locator": {"css": "#under"}, "action": "click"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("intercepts pointer events"), "{r}");
    let r = h.page("Pyro.run", json!({"steps": [{"mouse": {"action": "click"}}]}));
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("needs x and y"), "{r}");
}

/// Gap 10: `wheel` fires a `wheel` event on the element under the pointer
/// and scrolls its container; the locator form hovers the element first,
/// the `mouse` form uses the current pointer position unless given `x`/`y`.
#[test]
fn driver_run_wheel_scrolls_the_container_under_the_pointer() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_POINTER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "#tall"}, "action": "wheel", "deltaY": 100}},
            {"check": {"locator": {"css": "#wheel-out"}, "assert": "text", "expected": "wheel:0,100:tall-inner"}},
            {"eval": {"expression": "document.getElementById('tall').scrollTop", "as": "top1"}},
            {"mouse": {"action": "wheel", "deltaY": 50}},
            {"eval": {"expression": "document.getElementById('tall').scrollTop", "as": "top2"}},
            {"mouse": {"action": "wheel", "x": 5, "y": 5, "deltaY": 50}},
            {"eval": {"expression": "document.getElementById('tall').scrollTop", "as": "top3"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["top1"], 100, "{r}");
    assert_eq!(r["result"]["reads"]["top2"], 150, "{r}");
    assert_eq!(r["result"]["reads"]["top3"], 150, "{r}");
}

/// Gap 6: a pointer action aims at the center of the element's part inside
/// the viewport and its overflow-clipping ancestors (a 2500 px row in a
/// 1000 px scroller is clicked at x 500, not 1250).
#[test]
fn driver_run_clicks_the_visible_part_of_a_clipped_element() {
    let mut h = Harness::new();
    let desktop = json!({"mobile":false,"width":1440,"height":900,"screenWidth":1440,"screenHeight":900,"deviceScaleFactor":1,"screenOrientation":{"angle":0,"type":"landscapePrimary"},"dontSetVisibleSize":false});
    assert_eq!(h.page("Emulation.setDeviceMetricsOverride", desktop)["result"], json!({}));
    load_html(&mut h, DRIVER_POINTER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"act": {"locator": {"css": "div[ondblclick]"}, "action": "dblclick"}},
            {"check": {"locator": {"css": "#wide-out"}, "assert": "containsText", "expected": "wide@"}},
            {"read": {"locator": {"css": "#wide-out"}, "read": "text", "as": "at"}},
            {"check": {"locator": {"css": "#wide"}, "assert": "text", "expected": "ok"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let at = r["result"]["reads"]["at"].as_str().unwrap_or_default();
    let x: f64 = at.trim_start_matches("wide@").parse().unwrap_or_else(|_| panic!("{r}"));
    assert!(x > 0.0 && x < 1000.0, "clicked at {x}: {r}");
}

const DRIVER_SWAP_PAGE: &str = r#"<!doctype html><body>
<style>.box{width:120px;height:40px;display:block} .box input{width:120px;height:40px;display:block;margin:0;box-sizing:border-box}</style>
<div id="sync" class="box"><span class="frame">sync</span></div>
<div id="later" class="box"><span class="frame">later</span></div>
<div id="micro" class="box"><span class="frame">micro</span></div>
<div id="gone" class="box"><span class="frame">gone</span></div>
<div id="reveal" class="box"></div>
<script>
  const log = [];
  const swap = (wrap) => {
    const input = document.createElement('input');
    input.id = wrap.id + '-input';
    for (const t of ['mousedown', 'mouseup', 'click', 'focus']) input.addEventListener(t, () => log.push(wrap.id + ':' + t));
    wrap.firstElementChild.replaceWith(input);
  };
  document.getElementById('sync').addEventListener('mouseenter', (e) => swap(e.currentTarget), { once: true });
  document.getElementById('later').addEventListener('mouseover', (e) => { const w = e.currentTarget; setTimeout(() => swap(w), 0); }, { once: true });
  document.getElementById('micro').addEventListener('mouseover', (e) => { const w = e.currentTarget; queueMicrotask(() => swap(w)); }, { once: true });
  document.getElementById('gone').addEventListener('mouseover', (e) => { const w = e.currentTarget; setTimeout(() => swap(w), 0); }, { once: true });
  const reveal = document.getElementById('reveal');
  reveal.addEventListener('mouseenter', () => {
    reveal.innerHTML = '<a id="ext" href="/ext" target="_blank"><svg width="24" height="24"><path d="M0 0h24v24H0z"/></svg></a>';
    const a = document.getElementById('ext');
    for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) a.addEventListener(t, (e) => { if (t === 'click') e.preventDefault(); log.push('ext:' + t); });
  }, { once: true });
  window.getLog = () => log.join(',');
</script>"#;

/// A click on an element that a hover handler replaces (synchronously, from
/// a timer, or from a microtask) presses on the control that is there after
/// the move: it gets mousedown, mouseup and click, and focus. That holds
/// when the located element itself left the document (`#gone .frame`): the
/// sequence goes on against the element under the pointer. A link revealed
/// on hover and hit through its `<svg>` child gets the whole sequence.
#[test]
fn driver_run_click_presses_the_element_a_hover_swapped_in() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_SWAP_PAGE);
    for id in ["sync", "later", "micro"] {
        let r = h.page(
            "Pyro.run",
            json!({"timeout": 1000, "steps": [
                {"act": {"locator": {"css": format!("#{id}")}, "action": "click"}},
                {"check": {"locator": {"css": format!("#{id}-input")}, "assert": "focused"}},
            ]}),
        );
        assert!(r.get("error").is_none(), "{id}: {r}");
    }
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"act": {"locator": {"css": "#gone .frame"}, "action": "click"}},
            {"check": {"locator": {"css": "#gone-input"}, "assert": "focused"}},
            {"act": {"locator": {"css": "#reveal"}, "action": "hover"}},
            {"act": {"locator": {"css": "#ext svg"}, "action": "click"}},
            {"eval": {"expression": "getLog()", "as": "log"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let log = r["result"]["reads"]["log"].as_str().unwrap_or_default();
    for id in ["sync", "later", "micro", "gone"] {
        for t in ["focus", "mousedown", "mouseup", "click"] {
            assert!(log.contains(&format!("{id}:{t}")), "{id}:{t} missing in {log}");
        }
    }
    assert!(log.ends_with("ext:pointerdown,ext:mousedown,ext:pointerup,ext:mouseup,ext:click"), "{log}");
}

const DRIVER_TURNS_PAGE: &str = r#"<!doctype html><body>
<div id="handle" style="width:40px;height:40px">H</div>
<div style="height:100px"></div>
<div id="target" style="width:40px;height:40px">T</div>
<input id="a"><input id="b">
<script>
  const log = [];
  document.addEventListener('mousedown', () => {
    setTimeout(() => log.push('tick'), 0);
    requestAnimationFrame(() => log.push('raf'));
  }, true);
  for (const t of ['mousedown', 'mousemove', 'mouseup']) document.addEventListener(t, () => log.push(t), true);
  const saves = [];
  for (const id of ['a', 'b']) {
    const el = document.getElementById(id);
    el.addEventListener('focusout', () => setTimeout(() => saves.push(id + '=' + el.value), 0));
  }
  window.getLog = () => log.join(',');
  window.getSaves = () => saves.join(',');
</script>"#;

/// Each pointer event of a drag is its own task: a timer and an animation
/// frame requested by the mousedown handler run before the mouseup, as in
/// Chromium where every input event is a task. Every step, `settle: false`
/// ones too, is followed by an event-loop turn: a `setTimeout(save, 0)` from
/// one field's focusout runs before the next field is filled.
#[test]
fn driver_run_turns_the_event_loop_between_input_events_and_steps() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_TURNS_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"act": {"locator": {"css": "#handle"}, "action": "drag", "to": {"css": "#target"}}},
            {"eval": {"expression": "getLog()", "as": "log"}},
            {"act": {"locator": {"css": "#a"}, "action": "fill", "value": "1", "settle": false}},
            {"act": {"locator": {"css": "#a"}, "action": "blur", "settle": false}},
            {"act": {"locator": {"css": "#b"}, "action": "fill", "value": "2", "settle": false}},
            {"act": {"locator": {"css": "#b"}, "action": "blur", "settle": false}},
            {"eval": {"expression": "getSaves()", "as": "saves"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let log = r["result"]["reads"]["log"].as_str().unwrap_or_default();
    let events: Vec<&str> = log.split(',').collect();
    let at = |what: &str| events.iter().position(|e| *e == what).unwrap_or_else(|| panic!("{what} missing in {log}"));
    assert!(at("tick") > at("mousedown") && at("tick") < at("mouseup"), "{log}");
    assert!(at("raf") > at("mousedown") && at("raf") < at("mouseup"), "{log}");
    assert_eq!(events.iter().filter(|e| **e == "mousemove").count(), 4, "{log}");
    assert_eq!(r["result"]["reads"]["saves"], "a=1,b=2", "{r}");
}

const SETTLE_OBSERVER_PAGE: &str = r#"<!doctype html><body style="margin:0">
<div id="box" style="width:100px;height:20px"></div>
<button id="grow">Grow</button> <button id="loop">Loop</button>
<script>
  const box = document.getElementById('box');
  window.seen = [];
  let rounds = 0;
  new ResizeObserver((entries) => {
    const width = entries[0].contentRect.width;
    seen.push(width);
    if (rounds > 0) { rounds--; box.style.width = (width + 10) + 'px'; }
  }).observe(box);
  document.getElementById('grow').onclick = () => { rounds = 3; box.style.width = '150px'; };
  document.getElementById('loop').onclick = () => { rounds = Infinity; box.style.width = '300px'; };
</script>"#;

/// A settle that only a ResizeObserver's pacing holds up has the observers
/// gather at once: a click whose resize sets off three more rounds of
/// callbacks (each resizing the box again) settles in the time the callbacks
/// take, not in four pacing intervals of 16 ms, and every size was delivered.
#[test]
fn settle_brings_paced_resize_observer_gathers_forward() {
    let mut h = Harness::new();
    load_html(&mut h, SETTLE_OBSERVER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"idle": {}},
            {"act": {"locator": {"css": "#grow"}, "action": "click"}},
            {"eval": {"expression": "seen.join()", "as": "seen"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["seen"], "100,150,160,170,180", "{r}");
    let click = &r["result"]["results"][1];
    assert!(click.get("settled").is_none(), "{r}");
    assert!(click["settleMs"].as_f64().unwrap() < 40.0, "{r}");
}

/// A ResizeObserver that resizes its target in every round (Chromium
/// delivers it once per frame, and input does not wait for it) does not hold
/// a click: the press waits for one delivered round, not for the loop to end.
#[test]
fn a_resize_observer_loop_does_not_hold_a_click() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><div id="t" style="width:100px;height:20px"></div>
<button id="b" onclick="document.title = 'clicked'">Go</button>
<script>
  const t = document.getElementById('t');
  new ResizeObserver(() => { t.style.width = t.offsetWidth === 100 ? '101px' : '100px'; }).observe(t);
</script>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 3000, "steps": [
            {"act": {"locator": {"css": "#b"}, "action": "click", "settle": false}},
            {"eval": {"expression": "document.title", "as": "title"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["title"], "clicked", "{r}");
}

/// Playwright's visibility: `visibility: hidden` or `collapse` (own or
/// inherited) is not visible to the `visible` filter, `read: visible` and the
/// visible/hidden checks, and an action waits on it.
#[test]
fn visibility_hidden_is_not_visible_to_the_driver() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<div style="visibility:hidden"><span id="inherited">a</span><button id="b">B</button></div>
<span id="own" style="visibility:hidden">b</span><span id="col" style="visibility:collapse">c</span><span id="shown">d</span>"#,
    );
    for (id, visible) in [("inherited", false), ("own", false), ("col", false), ("shown", true)] {
        let r = h.page("Pyro.query", json!({"locator":{"css":format!("#{id}")},"read":"visible"}));
        assert_eq!(r["result"]["value"], visible, "{id}: {r}");
        let r =
            h.page("Pyro.query", json!({"locator":{"css":format!("#{id}"),"filter":{"visible":true}},"read":"count"}));
        assert_eq!(r["result"]["value"], u64::from(visible), "{id}: {r}");
    }
    let r = h.page("Pyro.expect", json!({"locator":{"css":"#inherited"},"assert":"hidden"}));
    assert_eq!(r["result"]["actual"], false, "{r}");
    let r = h.page("Pyro.act", json!({"locator":{"css":"#b"},"action":"click","timeout":50}));
    let message = r["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("not visible"), "{r}");
}

/// `Range.getBoundingClientRect()` over an element's children is the union
/// of their boxes (Chromium): a `display: contents` element in a flex parent
/// has no box of its own, and its range reports its child's.
#[test]
fn a_range_over_display_contents_children_unites_their_boxes() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<body style="margin:0"><div style="margin:40px"><span style="display:inline-flex"><span id="c" style="display:contents"><span id="in" style="padding:0 3px">Ana Novak</span></span></span></div>
<div style="display:flex;margin:40px"><span id="m" style="display:contents"><b id="x">x</b> text <i id="y">y</i></span></div></body>"#,
    );
    let r = eval_value(
        &mut h,
        "(() => { const rect = (b) => [b.x, b.y, b.width, b.height].map(Math.round).join();            const range = (id) => { const r = document.createRange(); r.selectNodeContents(document.getElementById(id)); return rect(r.getBoundingClientRect()); };            const x = x_ => document.getElementById(x_).getBoundingClientRect();            const [a, b] = [x('x'), x('y')];            return [range('c') === rect(x('in')), rect(x('in')) !== '0,0,0,0',                    range('m') === [a.x, a.y, b.right - a.x, Math.max(a.bottom, b.bottom) - a.y].map(Math.round).join()].join(); })()",
    );
    assert_eq!(r, json!("true,true,true"));
}

/// Observers that keep resizing what they observe do not turn the settle
/// into a busy loop: it brings a few gathers forward, then the pacing
/// applies again until it gives up and reports the observers as pending.
#[test]
fn settle_leaves_a_resize_observer_loop_to_its_pacing() {
    let mut h = Harness::new();
    load_html(&mut h, SETTLE_OBSERVER_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"idle": {}},
            {"act": {"locator": {"css": "#loop"}, "action": "click", "settle": false}},
            {"idle": {"timeout": 200}},
            {"eval": {"expression": "seen.length", "as": "rounds"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let idle = &r["result"]["results"][2];
    assert_eq!(idle["settled"], false, "{r}");
    assert_eq!(idle["pending"]["observers"], 1, "{r}");
    // The first size, then at most a round per 16 ms of the 200 ms (and of
    // the turn before them), and the few brought forward.
    let rounds = r["result"]["reads"]["rounds"].as_f64().unwrap();
    assert!((6.0..=24.0).contains(&rounds), "{r}");
}

/// A scrollable pane whose `scroll` listener scrolls it again (a pixel to
/// and fro, so that what is in view stays there) while `rounds` are left.
/// `#go` scrolls it from its click handler, `#far` has to be scrolled to.
const SETTLE_SCROLL_PAGE: &str = r#"<!doctype html><body style="margin:0">
<div id="pane" style="height:100px;overflow:auto">
  <div style="height:400px"></div>
  <button id="far">Far</button>
  <div style="height:400px"></div>
</div>
<button id="go">Go</button> <button id="loop">Loop</button>
<script>
  const pane = document.getElementById('pane');
  window.seen = [];
  window.rounds = 0;
  let by = 1;
  pane.addEventListener('scroll', () => {
    seen.push('scroll');
    if (rounds > 0) { rounds--; pane.scrollTop += by; by = -by; }
  });
  document.getElementById('far').onclick = () => seen.push('far');
  document.getElementById('go').onclick = () => { seen.push('go'); rounds = 3; pane.scrollTop += 10; };
  document.getElementById('loop').onclick = () => { rounds = Infinity; pane.scrollTop += 10; };
</script>"#;

/// The `scroll` events an action leaves queued are part of what it settles
/// on, and a settle that only they hold up runs them at once instead of a
/// frame interval (16 ms) apart: the step after a click that scrolled (and
/// whose `scroll` handlers scrolled three more times) finds all four events
/// delivered. So does the step after a click the driver had to scroll to.
#[test]
fn settle_delivers_the_scroll_events_an_action_queued() {
    let mut h = Harness::new();
    load_html(&mut h, SETTLE_SCROLL_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"idle": {}},
            {"act": {"locator": {"css": "#go"}, "action": "click"}},
            {"eval": {"expression": "seen.splice(0).join()", "as": "scrolled"}},
            {"eval": {"expression": "rounds = 3; null"}},
            {"act": {"locator": {"css": "#far"}, "action": "click"}},
            {"eval": {"expression": "seen.join()", "as": "revealed"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["scrolled"], "go,scroll,scroll,scroll,scroll", "{r}");
    let click = &r["result"]["results"][1];
    assert!(click.get("settled").is_none(), "{r}");
    assert!(click["settleMs"].as_f64().unwrap() < 40.0, "{r}");
    // The scroll into view and what it set off: delivered around the click
    // (a slow click's own turns can take a frame interval) or by its settle.
    let revealed = r["result"]["reads"]["revealed"].as_str().unwrap_or_default();
    assert_eq!(revealed.split(',').filter(|e| *e == "scroll").count(), 4, "{r}");
    assert_eq!(revealed.split(',').filter(|e| *e == "far").count(), 1, "{r}");
    assert!(r["result"]["results"][4].get("settled").is_none(), "{r}");
}

/// A `scroll` handler that keeps scrolling does not turn the settle into a
/// busy loop: a few events are brought forward, then they come a frame
/// interval apart until the wait gives up and reports them as pending.
#[test]
fn settle_leaves_a_scroll_loop_to_its_pacing() {
    let mut h = Harness::new();
    load_html(&mut h, SETTLE_SCROLL_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"idle": {}},
            {"act": {"locator": {"css": "#loop"}, "action": "click", "settle": false}},
            {"idle": {"timeout": 200}},
            {"eval": {"expression": "seen.length", "as": "rounds"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let idle = &r["result"]["results"][2];
    assert_eq!(idle["settled"], false, "{r}");
    assert_eq!(idle["pending"]["scrolls"], 1, "{r}");
    // The few brought forward, then at most one per 16 ms of the 200 ms (and
    // of the turn before them).
    let rounds = r["result"]["reads"]["rounds"].as_f64().unwrap();
    assert!((5.0..=24.0).contains(&rounds), "{r}");
}

/// An action that does not settle (`focus` never does) still has its own
/// `scroll` event delivered in the turn that follows it: a focus that had to
/// reveal its element does not leave the event for the frame interval, where
/// it would land inside the next step, after that step's key opened a popover
/// that closes when its trigger's container scrolls.
#[test]
fn the_turn_after_an_action_that_does_not_settle_delivers_its_scroll_event() {
    let mut h = Harness::new();
    load_html(&mut h, SETTLE_SCROLL_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"idle": {}},
            {"eval": {"expression": "document.getElementById('far').addEventListener('keydown', () => seen.push('open')); null"}},
            {"act": {"locator": {"css": "#far"}, "action": "focus", "settle": false}},
            {"eval": {"expression": "seen.join() + ':' + (pane.scrollTop > 0)", "as": "focused"}},
            {"act": {"locator": {"css": "#far"}, "action": "press", "value": "ArrowDown"}},
            {"eval": {"expression": "seen.join()", "as": "pressed"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["focused"], "scroll:true", "{r}");
    assert_eq!(r["result"]["reads"]["pressed"], "scroll,open", "{r}");
}

/// A settle waits for the requests its own action started, not for as many
/// requests as were in flight before it: a click made while an earlier
/// request (100 ms) is half-way does not settle when that one ends but when
/// the click's own response landed. The earlier request holds up no settle:
/// the fill before the click does not wait for it.
#[test]
fn settle_waits_for_the_requests_its_action_started() {
    let address = driver_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/form")}},
            {"eval": {"expression": "fetch('/api?name=earlier'); 1"}},
            {"act": {"locator": {"css": "#name"}, "action": "fill", "value": "mine"}},
            {"eval": {"expression": "new Promise((done) => setTimeout(done, 50))"}},
            {"act": {"locator": {"css": "#go"}, "action": "click"}},
            {"read": {"locator": {"css": "#out"}, "read": "text", "as": "out"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["out"], "Hello, mine", "{r}");
    let results = r["result"]["results"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert!(results[2]["settleMs"].as_f64().unwrap() < 80.0, "the fill waited for the earlier request: {r}");
    assert!(results[4].get("settled").is_none(), "{r}");
    // The click's request takes 100 ms; the earlier one ended 50 ms into it.
    assert!(results[4]["settleMs"].as_f64().unwrap() >= 80.0, "{r}");
}

/// The step after a `release` finds the page with the released reply
/// handled: the answer of a held mocked request and a held real response are
/// delivered, and the page's handlers for them have run, before the step
/// ends. A held request that goes on to the network is not waited for.
#[test]
fn driver_run_release_delivers_before_the_next_step() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let out = "document.getElementById('out').textContent";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/api", "json": {"total": 7}, "hold": "answer"}},
            {"eval": {"expression": "call('/api?held'); 1"}},
            {"release": {"hold": "answer"}},
            {"eval": {"expression": out, "as": "answered"}},
            {"unmock": {}},
            {"mock": {"url": "/api", "passthrough": true, "hold": "real"}},
            {"eval": {"expression": "call('/api?real'); 1"}},
            {"release": {"hold": "real"}},
            {"eval": {"expression": out, "as": "passed"}},
            {"unmock": {}},
            {"mock": {"url": "/api", "hold": "unsent"}},
            {"eval": {"expression": "document.getElementById('out').textContent = ''; call('/api?slow'); 1"}},
            {"release": {"hold": "unsent"}},
            {"eval": {"expression": out, "as": "sent"}},
            {"check": {"locator": {"css": "#out"}, "assert": "containsText", "expected": "\"total\":1"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let (results, reads) = (&r["result"]["results"], &r["result"]["reads"]);
    for release in [3, 8, 13] {
        assert_eq!(results[release]["released"], 1, "{r}");
    }
    assert_eq!(reads["answered"], "{\"total\":7}", "{r}");
    assert!(reads["passed"].as_str().unwrap_or_default().contains("\"total\":1"), "{r}");
    // Sent on release: the backend's answer (300 ms) is not there yet.
    assert_eq!(reads["sent"], "", "{r}");
}

/// A run's `requests` has the requests the net thread announced while it ran,
/// whether or not the loop has read the announcement. The page task that
/// sends the request here stays busy until the backend has answered: the
/// `held` step, served right after that task, reads the parked response
/// before the loop reads the request's announcement. A request announced
/// between two runs (the loop runs without reading it) is not one of the next
/// run's own: that run reports it once, with `previousRun: true`.
#[test]
fn driver_run_lists_the_requests_announced_while_it_ran() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let busy = |path: &str| {
        format!(
            "setTimeout(() => {{ fetch('{path}'); \
             const end = Date.now() + 300; for (let i = 0; i < 2e9 && Date.now() < end; i++) {{}} }}, 40); null"
        )
    };
    let listed = |r: &Value, path: &str| {
        let requests = r["result"]["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
        requests.iter().filter(|q| q["url"].as_str().is_some_and(|u| u.ends_with(path))).count()
    };
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/api", "passthrough": true, "hold": "poll"}},
            {"eval": {"expression": busy("/api?first")}},
            {"held": {"hold": "poll", "count": 1, "status": 200}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(listed(&r, "/api?first"), 1, "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": busy("/api?second")}},
            {"held": {"hold": "poll", "count": 2, "status": 200}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!((listed(&r, "/api?first"), listed(&r, "/api?second")), (0, 1), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"eval": {"expression": "setTimeout(() => fetch('/api?between'), 40); null"}}]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(listed(&r, "/api?between"), 0, "{r}");
    h.browser.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(400)), || false);
    let r = h.page("Pyro.run", json!({"steps": [{"held": {"hold": "poll", "count": 3, "status": 200}}]}));
    assert!(r.get("error").is_none(), "{r}");
    let between: Vec<&Value> = r["result"]["requests"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .iter()
        .filter(|q| q["url"].as_str().is_some_and(|u| u.ends_with("/api?between")))
        .collect();
    assert_eq!(between.len(), 1, "{r}");
    assert_eq!(between[0]["previousRun"], true, "{r}");
    assert_eq!((listed(&r, "/api?first"), listed(&r, "/api?second")), (0, 0), "{r}");
}

/// A chain of tasks that counts them (`n`): the count moves between two
/// steps only when the event loop ran between them. `other` is a second
/// channel with a listener, for a script that posts a message.
const TASK_CHAIN: &str = "window.n = 0; window.tick = new MessageChannel(); \
    tick.port1.onmessage = () => { n++; tick.port2.postMessage(0); }; tick.port2.postMessage(0); \
    window.other = new MessageChannel(); other.port1.onmessage = () => {}; null";

/// An `eval` whose script left the page nothing to run (it read, or changed
/// only script state) goes straight on to the next step; one that queued
/// work (a timer, a frame, a message, a request), changed the document or
/// moved the focus gets its event-loop turn first.
#[test]
fn driver_run_eval_gets_a_turn_only_when_it_left_the_page_work() {
    let address = driver_site();
    let mut h = Harness::new();
    let cases: &[(&str, bool)] = &[
        ("document.title", false),
        ("window.x = 1", false),
        ("document.body.getBoundingClientRect().width", false),
        ("getComputedStyle(document.body).color", false),
        ("document.querySelectorAll('button').length", false),
        ("document.dispatchEvent(new Event('nothing'))", false),
        ("Promise.resolve().then(() => { window.micro = n })", false),
        ("localStorage.setItem('k', 'v')", false),
        ("history.pushState({}, '', '/form?pushed')", false),
        ("setTimeout(() => {}, 0)", true),
        ("setTimeout(() => {}, 60000)", true),
        ("requestAnimationFrame(() => {})", true),
        ("other.port2.postMessage(0)", true),
        ("document.body.appendChild(document.createElement('i'))", true),
        ("document.body.style.color = 'red'", true),
        ("document.getElementById('name').focus()", true),
        ("new ResizeObserver(() => {}).observe(document.body)", true),
        ("fetch('/api?name=turn')", true),
        ("document.getElementById('go').click()", true),
    ];
    let mut steps =
        vec![json!({"goto": {"url": format!("http://{address}/form")}}), json!({"eval": {"expression": TASK_CHAIN}})];
    for (i, (script, _)) in cases.iter().enumerate() {
        steps.push(json!({"eval": {"expression": format!("{script}; n"), "as": format!("a{i}")}}));
        steps.push(json!({"eval": {"expression": "n", "as": format!("b{i}")}}));
    }
    // The same for a called function (`evaluate(fn)`).
    steps.push(json!({"eval": {"expression": "() => n", "call": true, "as": "readA"}}));
    steps.push(json!({"eval": {"expression": "() => n", "call": true, "as": "readB"}}));
    steps.push(
        json!({"eval": {"expression": "() => { setTimeout(() => {}, 0); return n; }", "call": true, "as": "workA"}}),
    );
    steps.push(json!({"eval": {"expression": "() => [n, typeof micro]", "call": true, "as": "workB"}}));
    let r = h.page("Pyro.run", json!({ "steps": steps }));
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    let count = |key: &str| reads[key].as_f64().unwrap_or_else(|| panic!("{key} in {r}"));
    for (i, (script, turn)) in cases.iter().enumerate() {
        let (a, b) = (count(&format!("a{i}")), count(&format!("b{i}")));
        assert_eq!(b > a, *turn, "`{script}`: {a} tasks before the next step, {b} at it");
    }
    assert_eq!(reads["readA"], reads["readB"], "{r}");
    assert!(reads["workB"][0].as_f64() > reads["workA"].as_f64(), "{r}");
    // The microtask a script queued ran before the step ended, turn or not.
    assert_eq!(reads["workB"][1], "number", "{r}");
}

const DRIVER_SHIFT_PAGE: &str = r#"<!doctype html><body style="margin:0">
<div id="list"></div>
<button id="archive" style="width:160px;height:30px"><span>Archive Department</span></button>
<div id="section" style="height:400px">You will not be able to undo this</div>
<script>
  const log = [];
  document.getElementById('archive').addEventListener('pointerdown', () => setTimeout(() => {
    document.getElementById('list').innerHTML = '<div style="height:200px">rows that loaded late</div>';
    log.push('shifted');
  }, 0));
  for (const t of ['pointerdown', 'pointerup', 'click']) {
    document.addEventListener(t, (e) => log.push(t + ':' + (e.target.closest('[id]')?.id ?? e.target.tagName)), true);
  }
  window.getLog = () => log.join(',');
</script>"#;

/// Page work that runs between a click's press and release (an async list
/// above the button finished loading) can move the pressed element: the
/// release follows it, so the click lands on what was pressed instead of on
/// whatever moved under the pointer (react-aria cancels a press released
/// elsewhere).
#[test]
fn driver_click_release_follows_an_element_that_moved_after_the_press() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_SHIFT_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"act": {"locator": {"css": "#archive"}, "action": "click"}},
            {"eval": {"expression": "getLog()", "as": "log"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["log"], "pointerdown:archive,shifted,pointerup:archive,click:archive", "{r}");
}

const DRIVER_BLUR_PAGE: &str = r#"<!doctype html><body>
<input id="f"><input id="g">
<script>
  const log = [];
  const f = document.getElementById('f');
  for (const t of ['focus', 'blur', 'focusout']) f.addEventListener(t, (e) => log.push(t + ':' + (e.relatedTarget ? e.relatedTarget.id : 'null')));
  const gone = [];
  document.addEventListener('focusout', (e) => { if (e.target.id === 'g') gone.push('focusout:' + e.target.isConnected); });
  window.getLog = () => log.join(',');
  window.getGone = () => gone.join(',');
</script>"#;

/// The `blur` action fires `blur` and `focusout` (no related target) on the
/// element and leaves the body active. Removing the focused element fires
/// `focusout` first, while it is connected, as Chromium does: a listener on
/// the document (React's `onBlur`) sees it.
#[test]
fn driver_run_blur_and_removal_fire_blur_and_focusout() {
    let mut h = Harness::new();
    load_html(&mut h, DRIVER_BLUR_PAGE);
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"act": {"locator": {"css": "#f"}, "action": "focus"}},
            {"check": {"locator": {"css": "#f"}, "assert": "focused"}},
            {"act": {"locator": {"css": "#f"}, "action": "blur"}},
            {"check": {"locator": {"css": "#f"}, "assert": "focused", "expected": false}},
            {"eval": {"expression": "document.activeElement.tagName", "as": "active"}},
            {"eval": {"expression": "getLog()", "as": "log"}},
            {"act": {"locator": {"css": "#g"}, "action": "click"}},
            {"eval": {"expression": "document.getElementById('g').remove(); getGone() + ':' + document.activeElement.tagName", "as": "gone"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["active"], "BODY", "{r}");
    assert_eq!(r["result"]["reads"]["gone"], "focusout:true:BODY", "{r}");
    assert_eq!(r["result"]["reads"]["log"], "focus:null,blur:null,focusout:null", "{r}");
}

/// A client that always has a command waiting (pipelined, or several in
/// flight) still lets the page's timers run: each pass of the serve loop
/// runs a ready task before it looks at the inbox, so a `setTimeout(f, 0)`
/// fires between two commands instead of never.
#[test]
fn timers_run_between_commands_of_a_client_that_never_waits() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><body>");
    let session = h.session.clone();
    h.send(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression": "window.f = 0; setTimeout(() => { f = 1; }, 0); 0"}),
    );
    let mut seen = Vec::new();
    for _ in 0..20 {
        // As the serve loop does when the next command is already decoded.
        h.browser.host_mut().run_until_work(None, || true);
        h.browser.host_mut().flush();
        let id = h.send(Some(&session), "Runtime.evaluate", json!({"expression": "String(f)", "returnByValue": true}));
        let reply = h.drain().into_iter().find(|m| m["id"] == id).expect("evaluate replies at once");
        seen.push(reply["result"]["result"]["value"].as_str().unwrap_or_default().to_owned());
        if seen.last().is_some_and(|v| v == "1") {
            return;
        }
    }
    panic!("the 0 ms timer never ran: {seen:?}");
}

/// Screenshots rendered with pyro-render (`render` feature).
#[cfg(feature = "render")]
mod screenshot {
    use super::*;

    const CARD: &str = r#"<!doctype html><html><head><style>
      body { margin: 0; background: #f3f4f6; font-family: sans-serif; }
      .card { width: 300px; height: 120px; margin: 40px; background: #6d28d9; border-radius: 12px; }
      .tall { height: 2000px; }
    </style></head><body><div class="card" id="card"></div><input id="name"><div class="tall"></div></body></html>"#;

    fn png_of(b64: &Value) -> pyro_render::RgbaImage {
        let bytes = pyro_cdp::fetch_protocol::base64_decode(b64.as_str().expect("base64 string")).unwrap();
        pyro_render::decode_png(&bytes).unwrap()
    }

    fn pixel(img: &pyro_render::RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * img.width as usize + x as usize) * 4;
        img.data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn print_to_pdf_validates_and_applies_margins_scale_and_ranges() {
        let mut h = Harness::new();
        load_html(&mut h, "<!doctype html><body style=margin:0><div style=height:3000px></div>");
        let pages = |h: &mut Harness, params: Value| {
            let r = h.page("Page.printToPDF", params);
            r["result"]["pyro"]["pages"].as_array().map(Vec::len).unwrap_or_else(|| panic!("{r}"))
        };
        // Letter, no margins: 3000 / 1056 px.
        let none = json!({"marginTop": 0, "marginBottom": 0, "marginLeft": 0, "marginRight": 0});
        assert_eq!(pages(&mut h, none.clone()), 3);
        assert_eq!(pages(&mut h, json!({})), 3000usize.div_ceil(1056 - 77), "Chrome's 0.4in default margins");
        let mut half = none.clone();
        half["scale"] = json!(0.5);
        assert_eq!(pages(&mut h, half), 2);
        let mut ranges = none.clone();
        ranges["pageRanges"] = json!("2-");
        assert_eq!(pages(&mut h, ranges), 2);
        for (params, message) in [
            (json!({"scale": 3}), "scale is outside [0.1 - 2] range"),
            (json!({"pageRanges": "2-1"}), "Page range syntax error"),
            (json!({"marginTop": -1}), "marginTop is negative"),
            (json!({"marginLeft": 5, "marginRight": 5}), "invalid print parameters: content area is empty"),
            (json!({"paperWidth": 0}), "paperWidth is zero or negative"),
        ] {
            let r = h.page("Page.printToPDF", params.clone());
            assert_eq!(r["error"]["message"], message, "{params}: {r}");
        }
        let r = h.page("Page.printToPDF", json!({"pageRanges": "9"}));
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("Page range exceeds page count"), "{r}");
    }

    /// Paper-cut PDFs carry link annotations and (by default, as Chrome)
    /// a tag tree; printing at the paper's width leaves the page's layout
    /// as it was.
    #[test]
    fn print_to_pdf_links_tags_and_keeps_the_screen_layout() {
        let mut h = Harness::new();
        load_html(
            &mut h,
            r#"<!doctype html><body style="margin:0"><h1>Title</h1><p>See <a href="https://example.com/x">x</a></p><div id="w" style="height:10px"></div>"#,
        );
        let width = "document.getElementById('w').getBoundingClientRect().width";
        let before = eval_value(&mut h, width);
        let print = |h: &mut Harness, params: Value| {
            let r = h.page("Page.printToPDF", params);
            let pdf =
                pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap_or_else(|| panic!("{r}")))
                    .unwrap();
            (String::from_utf8_lossy(&pdf).into_owned(), r["result"]["pyro"]["stats"].clone())
        };
        let (pdf, stats) = print(&mut h, json!({}));
        assert_eq!(stats["links"], 1, "{stats}");
        assert!(pdf.contains("/URI(https://example.com/x)"), "the link's URI");
        assert!(pdf.contains("/StructTreeRoot") && pdf.contains("/S/H1"), "tagged by default");
        let (pdf, _) = print(&mut h, json!({"generateTaggedPDF": false}));
        assert!(!pdf.contains("/StructTreeRoot"), "generateTaggedPDF: false");
        // Letter with 0.4in margins is 741.33 px of content, laid out at that
        // width for printing only.
        assert_eq!(eval_value(&mut h, width), before, "the screen layout is restored");
        let r = h.page("Page.printToPDF", json!({"pageRanges": "9"}));
        assert!(r.get("error").is_some(), "{r}");
        assert_eq!(eval_value(&mut h, width), before, "restored after a failed print too");
    }

    /// Page size and link rects measured on Chrome 156: paper rounded up and
    /// margins down to whole points, the origin in whole px.
    #[test]
    fn print_to_pdf_page_geometry_matches_chrome() {
        let mut h = Harness::new();
        load_html(
            &mut h,
            r#"<!doctype html><body style="margin:0"><div style="height:70px"></div><a href="https://example.com/b" style="display:block;margin-left:40px;width:200px;height:50px"></a>"#,
        );
        let (letter, a4) = ((8.5, 11.0, "612 792"), (8.27, 11.69, "595.92 841.92"));
        for ((w, height, media), margin, scale, want) in [
            (letter, 0.4, 1.0, [57.75, 674.25, 207.75, 711.75]),
            (letter, 0.4, 0.8, [51.75, 692.25, 171.75, 722.25]),
            (letter, 0.4, 1.5, [72.75, 629.25, 297.75, 685.5]),
            (letter, 0.5, 1.0, [66.0, 666.0, 216.0, 703.5]),
            (a4, 1.0 / 2.54, 1.0, [57.75, 724.17, 207.75, 761.67]),
            (a4, 0.5, 0.8, [60.0, 733.92, 180.0, 763.92]),
            (a4, 0.0, 1.0, [30.0, 751.92, 180.0, 789.42]),
        ] {
            let r = h.page(
                "Page.printToPDF",
                json!({"paperWidth": w, "paperHeight": height, "marginTop": margin, "marginBottom": margin,
                       "marginLeft": margin, "marginRight": margin, "scale": scale}),
            );
            let pdf =
                pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap_or_else(|| panic!("{r}")))
                    .unwrap();
            let pdf = String::from_utf8_lossy(&pdf).into_owned();
            assert!(pdf.contains(&format!("/MediaBox[0 0 {media}]")), "{w}x{height}");
            let got: Vec<f64> = pdf
                .split("/Rect[")
                .nth(1)
                .and_then(|r| r.split(']').next())
                .map(|r| r.split_whitespace().filter_map(|v| v.parse().ok()).collect())
                .unwrap_or_default();
            let matches = got.len() == 4 && got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01);
            assert!(matches, "{w}x{height} margin {margin} scale {scale}: {got:?}, Chrome {want:?}");
        }
        // A fractional print width (741.33 px) restores the screen layout, which is whole px.
        let width = "document.querySelector('a').parentElement.getBoundingClientRect().width";
        let before = eval_value(&mut h, width);
        assert!(h.page("Page.printToPDF", json!({})).get("error").is_none());
        assert_eq!(eval_value(&mut h, width), before, "the screen layout is restored");
        // Whole points decide whether content is left, as in Chrome.
        let thin = |m: f64| json!({"paperWidth": 1, "paperHeight": 1, "marginTop": m, "marginBottom": m, "marginLeft": 0, "marginRight": 0});
        assert!(h.page("Page.printToPDF", thin(0.497)).get("error").is_none());
        let r = h.page("Page.printToPDF", thin(0.5));
        assert_eq!(r["error"]["message"], "invalid print parameters: content area is empty", "{r}");
    }

    /// Pagination of one document mixing what moves cuts: the page count and every link area are Chrome 156's on A4
    /// without margins. Its 900 px width shrinks it, avoided boxes move (the 1200 px one, taller than a page, too),
    /// `widows` holds lines back and the forced break keeps its margin.
    #[test]
    fn print_to_pdf_paginates_a_mixed_document_as_chrome() {
        let mut h = Harness::new();
        let link = |id: String| format!(r#"<a href="https://example.com/{id}">x</a>"#);
        let lines = |p: &str, n: usize| (0..n).map(|i| link(format!("{p}{i}"))).collect::<Vec<_>>().join("<br>");
        let block = |id: &str, h: u32, style: &str| {
            format!(r#"<a href="https://example.com/{id}" style="display:block;height:{h}px;{style}"></a>"#)
        };
        let html = format!(
            r#"<!doctype html><body style="margin:0;font:16px/20px Arial"><div style="height:900px"></div><div>{}</div><div style="height:10px"></div>{}<div style="height:600px"></div>{}<div style="widows:3">{}</div><div style="break-before:page;margin-top:30px">{}</div><div style="width:900px">{}</div>"#,
            lines("a", 3),
            block("b", 300, "break-inside:avoid"),
            block("c", 1200, "break-inside:avoid;margin-top:20px"),
            lines("d", 4),
            lines("e", 1),
            lines("f", 2),
        );
        load_html(&mut h, &html);
        let r = h.page(
            "Page.printToPDF",
            json!({"paperWidth": 8.27, "paperHeight": 11.69, "marginTop": 0, "marginBottom": 0, "marginLeft": 0, "marginRight": 0}),
        );
        let pdf = pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap_or_else(|| panic!("{r}")))
            .unwrap();
        let pdf = String::from_utf8_lossy(&pdf).into_owned();
        assert_eq!(pdf.matches("/MediaBox[").count(), 5, "pages");
        let mut got: Vec<Vec<f64>> = pdf
            .split("/Rect[")
            .skip(1)
            .filter_map(|r| r.split(']').next())
            .map(|r| r.split_whitespace().filter_map(|v| v.parse().ok()).collect())
            .collect();
        got.sort_by(|a, b| a[1].total_cmp(&b[1]));
        // Bottom edges and heights (pt), sorted: page by page they say which line is where.
        let chrome = [
            (0.54, 198.75),
            (21.74, 11.27),
            (34.99, 11.27),
            (46.92, 795.0),
            (207.25, 11.26),
            (220.49, 11.27),
            (233.75, 11.26),
            (783.62, 11.26),
            (796.87, 11.26),
            (810.12, 11.26),
            (816.74, 11.27),
            (830.0, 11.26),
        ];
        assert_eq!(got.len(), chrome.len(), "{got:?}");
        for (rect, (bottom, height)) in got.iter().zip(chrome) {
            let close = (rect[1] - bottom).abs() < 0.05 && ((rect[3] - rect[1]) - height).abs() < 0.05;
            assert!(close, "{rect:?}, Chrome bottom {bottom} height {height}: {got:?}");
        }
    }

    /// A server for load gate tests: `/` answers `page`; a request `stall` picks (by its whole
    /// head) waits that long, then answers 404 (images) or `font` bytes; others 404 at once.
    fn gate_server(page: &'static str, stall: fn(&str) -> Option<u64>) -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let (status, kind, body) = if head.starts_with("GET / ") {
                        ("200 OK", "text/html", page)
                    } else {
                        if let Some(ms) = stall(&head) {
                            std::thread::sleep(Duration::from_millis(ms));
                        }
                        ("404 Not Found", "text/plain", "")
                    };
                    let out = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(out.as_bytes());
                });
            }
        });
        address
    }

    #[test]
    fn animation_frame_starts_a_changed_content_image_without_a_geometry_read() {
        let address = gate_server("<!doctype html><div id='target'></div>", |_| None);
        let mut harness = Harness::new();
        harness.page("Page.enable", json!({}));
        harness.page("Network.enable", json!({}));
        harness.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        harness.until_event(|event| event["method"] == "Page.loadEventFired");
        harness.page("Runtime.evaluate", json!({"expression":
            "requestAnimationFrame(() => document.getElementById('target').style.content = 'url(/replacement.svg)')"}));
        let url = format!("http://{address}/replacement.svg");
        harness.until_event(|event| event["method"] == "Network.requestWillBeSent" && event["params"]["request"]["url"] == url);
    }

    /// Author CSS cannot see the engine's reserved attributes in the geometry mirror: attribute
    /// selectors on each reserved name and on every one-character prefix or suffix of the
    /// snapshot nonce, inside `:is()`/`:has()` too, match nothing, and `attr()` of them is empty,
    /// so no geometry a page script reads depends on the nonce (security review cycle 7, 1).
    #[test]
    fn author_css_cannot_see_the_engines_attributes() {
        let mut rules = String::from(
            "iframe { border: 0; display: block; width: 50px; height: 20px } link { display: inline-block }\
             link::before { content: attr(data-pyro-sheet) attr(data-pyro-n) attr(data-pyro-frame) }\
             iframe[data-pyro-frame] { height: 31px } iframe[data-pyro-n] { height: 32px }\
             :is(iframe[data-pyro-n], iframe[data-pyro-frame]) { height: 33px } body:has([data-pyro-n]) iframe { margin-left: 9px }\
             link[data-pyro-sheet], link[data-pyro-n], link[data-pyro-sheet-referrer-policy] { width: 40px }",
        );
        for (i, c) in "0123456789abcdef".chars().enumerate() {
            rules.push_str(&format!(
                "iframe[data-pyro-n^=\"{c}\"] {{ width: {w}px }} iframe[data-pyro-n$=\"{c}\"] {{ height: {w}px }} \
                 iframe[data-pyro-n*=\"{c}{c}\"] {{ padding-left: {w}px }} iframe[data-pyro-frame^=\"{c}\"] {{ margin-top: {w}px }} \
                 iframe[data-pyro-n|=\"{c}\"], iframe[data-pyro-n~=\"{c}\"] {{ padding-top: {w}px }}",
                w = i + 1
            ));
        }
        let page = format!(
            "<!doctype html><style>{rules}</style><link id=l rel=stylesheet href=/s.css><iframe id=f srcdoc=\"<p>x</p>\"></iframe>"
        );
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let page = page.clone();
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let (kind, body) = if head.starts_with("GET /s.css") { ("text/css", "q { }".to_owned()) } else { ("text/html", page) };
                    let out = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(out.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        h.run_for(100);
        // (A capture gives the frame its document, whose marker the mirror then carries.)
        let shot = h.page("Page.captureScreenshot", json!({}));
        assert!(shot["result"]["data"].is_string(), "{shot}");
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": "const f = document.getElementById('f').getBoundingClientRect(), l = document.getElementById('l').getBoundingClientRect(); [f.left, f.top, f.width, f.height, l.width, getComputedStyle(document.getElementById('f')).paddingLeft].join()", "returnByValue": true}),
        );
        let top = |r: &Value| r["result"]["result"]["value"].as_str().unwrap_or_default().to_owned();
        let got = top(&r);
        let fields: Vec<&str> = got.split(',').collect();
        assert_eq!((fields.get(2).copied(), fields.get(3).copied(), fields.get(4).copied(), fields.get(5).copied()), (Some("50"), Some("20"), Some("0"), Some("0px")), "{got}");
        assert_eq!(fields.first().copied(), Some("8"), "{got}");
    }

    /// The page's own `data-pyro-*` attributes are ordinary attributes in the geometry mirror: an
    /// author `data-pyro-theme` styles by selector and by `attr()`, and an author-forged
    /// `data-pyro-n`/`data-pyro-frame` (no nonce) styles like any attribute while the engine ignores
    /// it (security review cycle 8, 2; the engine's own travel under nonce names).
    #[test]
    fn author_data_pyro_attributes_style_as_ordinary_attributes() {
        let page = "<!doctype html><style>div, iframe { display: block; border: 0; width: 10px; height: 10px }\
             [data-pyro-theme=dark] { width: 33px } span { display: inline-block } span::before { content: attr(data-pyro-theme) }\
             iframe[data-pyro-frame] { height: 44px } iframe[data-pyro-n^=\"0\"] { width: 55px }</style>\
             <div id=t data-pyro-theme=dark></div><span id=s data-pyro-theme=dark></span>\
             <iframe id=f data-pyro-n=0123456789abcdef0123456789abcdef data-pyro-frame=\"0123456789abcdef0123456789abcdef:1:0000000000000000\"></iframe>";
        let address = gate_server(page, |_| None);
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        h.run_for(50);
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": "const r = id => document.getElementById(id).getBoundingClientRect(); [r('t').width, r('s').width > 0, r('f').width, r('f').height].join()", "returnByValue": true}),
        );
        assert_eq!(r["result"]["result"]["value"].as_str(), Some("33,true,55,44"), "{r}");
    }

    /// The engine's private state (`urn:x-pyro:state`: pseudo-class tokens, top layer, markers) is
    /// invisible to author CSS in the geometry mirror: `[*|state]` and value selectors on it match
    /// nothing, while the pseudo-classes it feeds still match; a page's own `state` attribute, in
    /// no namespace or in `urn:x-pyro:state` itself, is an ordinary attribute (security review
    /// cycle 9, 2).
    #[test]
    fn engine_state_is_invisible_to_author_css() {
        let page = "<!doctype html><style>input { appearance: none; display: block; margin: 0; border: 0; padding: 0; width: 10px; height: 10px }\
             input:checked { width: 44px } [*|state] { height: 33px } input[*|state*=\"c\"] { margin-left: 22px }\
             div { width: 10px; height: 10px } div[*|state=mine] { width: 55px } div[state=plain] { height: 66px }\
             span { display: inline-block } span::before { content: attr(state) }</style>\
             <input id=c type=checkbox><div id=a></div><div id=b state=plain></div><span id=s></span>\
             <script>c.checked = true; a.setAttributeNS('urn:x-pyro:state', 'state', 'mine');</script>";
        let address = gate_server(page, |_| None);
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        h.run_for(50);
        let r = h.page(
            "Runtime.evaluate",
            json!({"expression": "const r = id => document.getElementById(id).getBoundingClientRect(); [r('c').width, r('c').height, r('c').left, r('a').width, r('a').height, r('b').height, r('s').width].join()", "returnByValue": true}),
        );
        // (The page's own `state` attributes match `[*|state]` too: 33px high, 66px by the more
        // specific `div[state=plain]`.)
        assert_eq!(r["result"]["result"]["value"].as_str(), Some("44,10,8,55,33,66,0"), "{r}");
    }

    /// How long `load` took from navigating to the gate server.
    fn load_time(address: std::net::SocketAddr) -> Duration {
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        let started = Instant::now();
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        started.elapsed()
    }

    /// A face first made applicable by a style change before load (its fetch starts with the
    /// gate's own layout pass) delays load: fonts are checked again after that pass (security
    /// review cycle 4, S3a). The face's request is the document's (by key), recorded as loading
    /// from the moment its layout asks, never confused with a failed one (cycle 5, 6).
    #[test]
    fn a_face_the_gates_layout_starts_delays_load() {
        let address = gate_server(
            // (A family no other test registers in the thread's font context.)
            "<!doctype html><style>@font-face { font-family: GateRecheckFace; src: url(/slow.woff2) }</style>\
             <p id=p>text</p><script>addEventListener('DOMContentLoaded', () => document.getElementById('p').style.fontFamily = 'GateRecheckFace')</script>",
            |head| head.starts_with("GET /slow.woff2").then_some(1500),
        );
        let waited = load_time(address);
        assert!(waited >= Duration::from_millis(1200), "load did not wait for the face: {waited:?}");
    }

    /// `replaceSync()` on an adopted sheet that drops the last use of a stalled image releases
    /// load (the gate looked at the DOM version only: S3b).
    #[test]
    fn an_adopted_sheet_change_releases_load() {
        let address = gate_server(
            "<!doctype html><div style='width:20px;height:20px'></div><script>\
             const sheet = new CSSStyleSheet(); sheet.replaceSync('div { background-image: url(/stall.png) }');\
             document.adoptedStyleSheets = [sheet]; setTimeout(() => sheet.replaceSync(''), 300);</script>",
            |head| head.starts_with("GET /stall.png").then_some(4000),
        );
        let waited = load_time(address);
        assert!(waited < Duration::from_millis(2500), "load waited for an image nothing uses: {waited:?}");
    }

    /// The gate waits by request, not URL: removing the stalled CORS variant of a URL releases
    /// load though the plain variant of that URL, which answered, is still used (S3c).
    #[test]
    fn the_gate_waits_by_request_not_url() {
        let address = gate_server(
            "<!doctype html><div style='width:20px;height:20px;background-image:url(/v.png)'></div>\
             <div id=c style='width:20px;height:20px;mask-image:url(/v.png)'></div>\
             <script>setTimeout(() => document.getElementById('c').remove(), 300)</script>",
            // (A mask's request is a CORS one.)
            |head| (head.starts_with("GET /v.png") && head.to_ascii_lowercase().contains("sec-fetch-mode: cors")).then_some(4000),
        );
        let waited = load_time(address);
        assert!(waited < Duration::from_millis(2500), "load waited for a variant nothing uses: {waited:?}");
    }

    /// A PDF waits for a frame's fetch that the frame's own geometry has in flight (the page's
    /// geometry, meeting it pending, did not count it, and the PDF painted before it arrived:
    /// security review cycle 5, 4).
    #[test]
    fn a_pdf_waits_for_a_frames_fetch_another_geometry_made() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let (status, body) = if head.starts_with("GET /slow.png") {
                        std::thread::sleep(Duration::from_millis(1200));
                        ("404 Not Found", "")
                    } else if head.starts_with("GET /child") {
                        // After load, the child's own layout (a geometry read) asks for the image.
                        ("200 OK", "<!doctype html><div id=d style='width:20px;height:20px'></div><script>\
                         addEventListener('load', () => setTimeout(() => { const d = document.getElementById('d');\
                         d.style.backgroundImage = 'url(/slow.png)'; d.getBoundingClientRect(); }, 0))</script>")
                    } else {
                        ("200 OK", "<!doctype html><iframe src=/child style='width:50px;height:50px;border:0'></iframe>")
                    };
                    let out = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(out.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        h.run_for(150);
        let session = h.session.clone();
        let started = Instant::now();
        let r = h.call(Some(&session), "Page.printToPDF", json!({}));
        assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
        assert!(started.elapsed() >= Duration::from_millis(600), "the PDF did not wait: {:?}", started.elapsed());
        // It parked on the pending fetch, woken when a fetch ended, not again and again while only
        // the frame's fetch was pending (security review cycle 6, 5: thousands of parks). (The end
        // of any layout fetch in the process wakes every waiting PDF, other tests' too: a few.)
        let parks = h.browser.host().pdf_parks();
        assert!((1..=200).contains(&parks), "the PDF wait parked {parks} times");
    }

    /// One failure state: a CSS image the document's layout failed to fetch is a failure for its
    /// capture too, which neither fetches it again nor paints what the document never got
    /// (geometry and capture kept apart failure sets: security review cycle 4, 3c).
    #[test]
    fn a_capture_takes_the_layouts_failure_as_its_own() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let counter = counter.clone();
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let (status, body) = if request.starts_with(b"GET /missing.png") {
                        counter.fetch_add(1, Ordering::SeqCst);
                        // (Answered late: a capture taken meanwhile meets the layout's fetch in flight.)
                        std::thread::sleep(Duration::from_millis(600));
                        ("404 Not Found", "")
                    } else {
                        ("200 OK", "<!doctype html><div style='width:20px;height:20px;background-image:url(/missing.png)'></div>")
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        let session = h.session.clone();
        // The layout asks (its load gate's pass); a capture taken while that fetch is in flight
        // waits for it instead of sending the request again.
        let deadline = Instant::now() + Duration::from_secs(10);
        while hits.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the layout never asked");
            h.run_for(10);
        }
        let r = h.call(Some(&session), "Page.captureScreenshot", json!({}));
        assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the capture met the layout's fetch in flight");
        // Once it failed (the 404 answered), a capture takes the failure as its own.
        h.run_for(800);
        let r = h.call(Some(&session), "Page.captureScreenshot", json!({}));
        assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the capture took the layout's failure");
    }

    /// The load event waits for the CSS images the document still references: once the only
    /// element using a stalled image is removed, load fires without it (it waited out the image or
    /// the 10 s cap: security review cycle 3, D).
    #[test]
    fn removing_the_only_user_of_a_stalled_css_image_releases_load() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let (kind, body) = if request.starts_with(b"GET /slow.png") {
                        std::thread::sleep(Duration::from_secs(4));
                        ("image/png", "")
                    } else {
                        (
                            "text/html",
                            "<!doctype html><div id=d style='width:20px;height:20px;background-image:url(/slow.png)'></div>\
                             <script>setTimeout(() => document.getElementById('d').remove(), 300)</script>",
                        )
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        let started = Instant::now();
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        let waited = started.elapsed();
        assert!(waited < Duration::from_millis(2500), "load waited for an image nothing uses: {waited:?}");
    }

    /// A layout sub-resource still downloading delays the PDF, not the
    /// browser thread: commands sent after printToPDF answer first.
    #[test]
    fn print_to_pdf_waits_for_layout_fetches_without_blocking() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let (kind, body) = if request.starts_with(b"GET /slow.svg") || request.starts_with(b"GET /later.svg") {
                        std::thread::sleep(Duration::from_millis(400));
                        (
                            "image/svg+xml",
                            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#,
                        )
                    } else {
                        (
                            "text/html",
                            r#"<!doctype html><body style=margin:0><img src="/slow.svg" style="width:50px;height:50px">"#,
                        )
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        let session = h.session.clone();
        // pyro: a CSS image asked for after load (the page's own image was fetched by layout
        // while it loaded): the PDF's layout starts its fetch and the PDF waits for it.
        h.call(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression": "document.body.style.background = 'url(/later.svg)'; 1", "returnByValue": true}),
        );
        let started = Instant::now();
        let pdf = h.send(Some(&session), "Page.printToPDF", json!({}));
        let eval = h.send(Some(&session), "Runtime.evaluate", json!({"expression": "1 + 1", "returnByValue": true}));
        let messages = h.until_response(pdf);
        let at = |id: u64| messages.iter().position(|m| m["id"] == id);
        let pdf_reply = &messages[at(pdf).unwrap()];
        assert!(pdf_reply["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{pdf_reply}");
        let waited = started.elapsed();
        assert!(at(eval).unwrap() < at(pdf).unwrap(), "the evaluate answered while the PDF waited");
        assert!(waited >= Duration::from_millis(300) && waited < Duration::from_secs(5), "{waited:?}");
    }

    #[test]
    fn emulated_print_media_reaches_screenshots() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 100, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0;background:#00f}@media print{body{background:#f00}}</style>",
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!(pixel(&shot(&mut h), 5, 5), [0, 0, 255, 255]);
        h.page("Emulation.setEmulatedMedia", json!({"media": "print"}));
        assert_eq!(pixel(&shot(&mut h), 5, 5), [255, 0, 0, 255]);
        h.page("Emulation.setEmulatedMedia", json!({"media": ""}));
        assert_eq!(pixel(&shot(&mut h), 5, 5), [0, 0, 255, 255]);
    }

    /// Modal dialogs and showing popovers paint above everything, in
    /// top-layer order, over their `::backdrop`, whatever their ancestors'
    /// z-index, opacity or overflow clip; hit testing and clicks follow.
    #[test]
    fn top_layer_paints_above_everything_over_its_backdrop() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            r#"<!doctype html><style>
              body { margin: 0; background: #fff; }
              #cover { position: fixed; inset: 0; z-index: 2147483647; background: rgb(0, 0, 255); }
              #clip { position: relative; z-index: 1; opacity: 0.5; overflow: hidden; width: 10px; height: 10px; }
              #d { width: 100px; height: 60px; padding: 0; border: 0; background: rgb(255, 0, 0); }
              #d button { display: block; width: 100px; height: 60px; padding: 0; border: 0; background: transparent; }
              #p { width: 40px; height: 40px; padding: 0; border: 0; background: rgb(255, 255, 0); }
            </style>
            <div id="clip"><dialog id="d"><button id="in">in</button></dialog><div id="p" popover></div></div>
            <div id="cover"></div>"#,
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        let click = |h: &mut Harness, x: u32, y: u32| {
            for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
                let r = h.page(
                    "Input.dispatchMouseEvent",
                    json!({"type": kind, "x": x, "y": y, "button": "left", "buttons": buttons, "clickCount": 1}),
                );
                assert_eq!(r["result"], json!({}), "{r}");
            }
        };
        eval_value(
            &mut h,
            "globalThis.clicks = []; for (const id of ['in', 'cover', 'd']) document.getElementById(id).addEventListener('click', e => clicks.push(id + ':' + e.target.id))",
        );
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 200, 150), [0, 0, 255, 255], "closed: the cover");
        assert_eq!(eval_value(&mut h, "document.elementFromPoint(200, 150).id"), "cover");

        // The modal dialog is centered in the viewport above the cover, at
        // full opacity and unclipped; the rest is under its backdrop
        // (rgba(0, 0, 0, 0.1) over blue).
        eval_value(&mut h, "d.showModal()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 165, 170), [255, 0, 0, 255], "dialog (away from its text)");
        assert_eq!(pixel(&img, 248, 178), [255, 0, 0, 255], "dialog bottom-right corner");
        assert_eq!(pixel(&img, 151, 121), [255, 0, 0, 255], "dialog top-left corner");
        assert_eq!(pixel(&img, 148, 150), [0, 0, 229, 255], "backdrop left of the dialog");
        assert_eq!(pixel(&img, 10, 10), [0, 0, 229, 255], "backdrop");
        assert_eq!(
            eval_value(
                &mut h,
                "[document.elementFromPoint(200, 150).id, document.elementFromPoint(10, 10).id, JSON.stringify(d.getBoundingClientRect())]"
            ),
            json!(["in", "d", r#"{"x":150,"y":120,"width":100,"height":60,"top":120,"right":250,"bottom":180,"left":150}"#])
        );
        // A click reaches the dialog's button; one outside lands on the
        // backdrop (the dialog), never on the inert cover.
        click(&mut h, 200, 150);
        click(&mut h, 10, 10);
        assert_eq!(eval_value(&mut h, "clicks.join()"), "in:in,d:in,d:d");

        // An author `::backdrop`.
        eval_value(&mut h, "document.head.insertAdjacentHTML('beforeend', '<style>#d::backdrop { background: rgb(0, 255, 0) }</style>')");
        assert_eq!(pixel(&shot(&mut h), 10, 10), [0, 255, 0, 255], "author backdrop");

        // A popover shown later is above the dialog; its backdrop is
        // transparent.
        eval_value(&mut h, "p.showPopover()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 200, 150), [255, 255, 0, 255], "popover above the dialog");
        assert_eq!(pixel(&img, 170, 150), [255, 0, 0, 255], "dialog around the popover");
        assert_eq!(pixel(&img, 10, 10), [0, 255, 0, 255], "the popover's backdrop is transparent");
        assert_eq!(eval_value(&mut h, "document.elementFromPoint(170, 150).id"), "in");

        // Hiding and closing put everything back where it was.
        eval_value(&mut h, "p.hidePopover(); d.close()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 200, 150), [0, 0, 255, 255], "closed again: the cover");
        assert_eq!(pixel(&img, 10, 10), [0, 0, 255, 255], "no backdrop");

        // A popover alone: above the cover, no backdrop color.
        eval_value(&mut h, "p.showPopover()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 200, 150), [255, 255, 0, 255], "popover above the cover");
        assert_eq!(pixel(&img, 10, 10), [0, 0, 255, 255], "the cover, not dimmed");
        assert_eq!(eval_value(&mut h, "document.elementFromPoint(200, 150).id"), "p");
    }

    /// On a scrolled page the top layer stays fixed to the viewport, and a
    /// popover shown inside a modal dialog paints above it by its own rank.
    #[test]
    fn top_layer_is_fixed_to_the_viewport_and_nests() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            r#"<!doctype html><style>
              body { margin: 0; background: rgb(0, 0, 255); }
              #d { width: 100px; height: 60px; padding: 0; border: 0; background: rgb(255, 0, 0); }
              #q { width: 20px; height: 20px; padding: 0; border: 0; margin: 0; top: 0; left: 0; background: rgb(0, 255, 0); }
              #d::backdrop { background: rgb(255, 255, 255); }
            </style>
            <div style="height: 3000px"></div><dialog id="d"><div id="q" popover></div></dialog>"#,
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        eval_value(&mut h, "scrollTo(0, 1000); d.showModal(); q.showPopover()");
        assert_eq!(eval_value(&mut h, "scrollY"), json!(1000));
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 165, 170), [255, 0, 0, 255], "dialog centered in the scrolled viewport");
        assert_eq!(pixel(&img, 10, 290), [255, 255, 255, 255], "backdrop covers the viewport");
        assert_eq!(pixel(&img, 5, 5), [0, 255, 0, 255], "the nested popover, above its dialog's backdrop");
        assert_eq!(eval_value(&mut h, "document.elementFromPoint(5, 5).id"), "q");
        // Hidden, the popover is not painted where its dialog would put it.
        eval_value(&mut h, "q.hidePopover()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 5, 5), [255, 255, 255, 255], "backdrop where the popover was");
        // A full-page capture keeps the dialog on the viewport it was laid out in.
        let r = h.page("Page.captureScreenshot", json!({"captureBeyondViewport": true}));
        let full = png_of(&r["result"]["data"]);
        assert!(full.height >= 3000, "{}", full.height);
    }

    /// A top-layer element is a stacking context whatever its style: an
    /// author `position: absolute` popover (no z-index, opacity or
    /// transform) keeps its z-indexed and fixed descendants, which paint
    /// with it above the document, not in place under it.
    #[test]
    fn top_layer_element_keeps_its_z_indexed_and_fixed_descendants() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            r#"<!doctype html><style>
              body { margin: 0; background: #fff; }
              #cover { position: fixed; inset: 0; z-index: 2147483647; background: rgb(0, 0, 255); }
              #p { position: absolute; inset: auto; left: 50px; top: 50px; width: 100px; height: 100px;
                   margin: 0; padding: 0; border: 0; overflow: visible; background: rgb(255, 0, 0); }
              #z { position: relative; z-index: 1; width: 40px; height: 40px; background: rgb(0, 255, 0); }
              #f { position: fixed; left: 300px; top: 200px; width: 20px; height: 20px; background: rgb(255, 255, 0); }
            </style>
            <div id="p" popover="manual"><div id="z"></div><div id="f"></div></div>
            <div id="cover"></div>"#,
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        eval_value(&mut h, "p.showPopover()");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 120, 120), [255, 0, 0, 255], "the popover above the cover");
        assert_eq!(pixel(&img, 70, 70), [0, 255, 0, 255], "its z-indexed child above it");
        assert_eq!(pixel(&img, 310, 210), [255, 255, 0, 255], "its fixed child above the cover");
        assert_eq!(pixel(&img, 10, 10), [0, 0, 255, 255], "the cover elsewhere");
    }

    /// The root element in the top layer (`<html popover>`) paints with its
    /// own transform applied once, as in the document pass.
    #[test]
    fn root_element_in_the_top_layer_paints_its_transform_once() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            r#"<!doctype html><html popover="manual"><style>
              html, html:popover-open { position: static; margin: 0; padding: 0; border: 0; width: auto; height: auto;
                overflow: visible; inset: auto; background: rgb(0, 0, 255); transform: translateX(100px); }
              body { margin: 0; }
              #b { width: 50px; height: 50px; background: rgb(255, 0, 0); }
            </style><div id="b"></div></html>"#,
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        // (Hidden, the root popover is `display: none`.)
        eval_value(&mut h, "document.documentElement.showPopover()");
        assert_eq!(eval_value(&mut h, "document.documentElement.matches(':popover-open')"), json!(true));
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 125, 25), [255, 0, 0, 255], "shown: still translated once");
        assert_ne!(pixel(&img, 225, 25), [255, 0, 0, 255], "not translated twice");
    }

    #[test]
    fn default_background_override_paints_under_the_document() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0}div{width:50px;height:50px;background:#ff0000}</style><div></div>",
        );
        let shot = |h: &mut Harness| {
            let r = h.page("Page.captureScreenshot", json!({}));
            png_of(&r["result"]["data"])
        };
        assert_eq!(pixel(&shot(&mut h), 150, 80), [255, 255, 255, 255], "white by default");
        let r =
            h.page("Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": 0, "g": 0, "b": 0, "a": 0}}));
        assert_eq!(r["result"], json!({}), "{r}");
        let img = shot(&mut h);
        assert_eq!(pixel(&img, 150, 80)[3], 0, "transparent where the document has no background");
        assert_eq!(pixel(&img, 10, 10), [255, 0, 0, 255], "the document still paints");
        h.page("Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": 0, "g": 0, "b": 255, "a": 0.5}}));
        let p = pixel(&shot(&mut h), 150, 80);
        assert!(p[2] >= 254 && p[0] == 0 && (127..=128).contains(&p[3]), "straight alpha: {p:?}");
        h.page("Emulation.setDefaultBackgroundColorOverride", json!({}));
        assert_eq!(pixel(&shot(&mut h), 150, 80), [255, 255, 255, 255], "absent color resets");
    }

    #[test]
    fn print_to_pdf_streams_through_io_and_validates_the_paper() {
        let mut h = Harness::new();
        load_html(&mut h, "<!doctype html><body style=margin:0><div style=height:2500px>pdf</div>");
        let inline = h.page("Page.printToPDF", json!({}));
        let inline = pyro_cdp::fetch_protocol::base64_decode(inline["result"]["data"].as_str().unwrap()).unwrap();
        assert!(inline.starts_with(b"%PDF"));
        let r = h.page("Page.printToPDF", json!({"transferMode": "ReturnAsStream"}));
        assert_eq!(r["result"]["data"], "", "{r}");
        let handle = r["result"]["stream"].as_str().unwrap().to_owned();
        let mut streamed = Vec::new();
        loop {
            let r = h.page("IO.read", json!({"handle": handle, "size": 4096}));
            assert_eq!(r["result"]["base64Encoded"], true, "{r}");
            streamed.extend(pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap()).unwrap());
            if r["result"]["eof"] == true {
                break;
            }
        }
        assert_eq!(streamed, inline, "the stream holds the same document");
        assert_eq!(h.page("IO.close", json!({"handle": handle}))["result"], json!({}));
        let r = h.page("IO.read", json!({"handle": handle}));
        assert_eq!(r["error"]["code"], -32602, "closed: {r}");
        for (w, hh) in [(0.0, 11.0), (8.5, -1.0)] {
            let r = h.page("Page.printToPDF", json!({"paperWidth": w, "paperHeight": hh}));
            assert_eq!(r["error"]["code"], -32602, "{w}x{hh}: {r}");
        }
    }

    #[test]
    fn screenshots_show_hover_styles() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 200, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0}#cell{width:100px;height:50px;background:#00f}\
             #tip{display:none;width:100px;height:50px;background:#f00}#cell:hover + #tip{display:block}</style>\
             <div id=cell></div><div id=tip></div>",
        );
        let shot = |h: &mut Harness| {
            let r = h.page("Page.captureScreenshot", json!({}));
            png_of(&r["result"]["data"])
        };
        assert_eq!(pixel(&shot(&mut h), 50, 75), [255, 255, 255, 255], "tip hidden");
        h.page("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": 50, "y": 25}));
        assert_eq!(pixel(&shot(&mut h), 50, 75), [255, 0, 0, 255], "hovering the cell shows the tip");
        h.page("Input.dispatchMouseEvent", json!({"type": "mouseMoved", "x": 150, "y": 150}));
        assert_eq!(pixel(&shot(&mut h), 50, 75), [255, 255, 255, 255], "tip hidden again");
    }

    #[test]
    fn screenshots_show_scrolled_containers() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 200, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0}#box{width:100px;height:100px;overflow:auto}\
             .a{height:100px;background:#f00}.b{height:100px;background:#00f}</style>\
             <div id=box><div class=a></div><div class=b></div></div>",
        );
        let shot = |h: &mut Harness| png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!(pixel(&shot(&mut h), 20, 50), [255, 0, 0, 255], "top of the content");
        h.page("Runtime.evaluate", json!({"expression": "document.getElementById('box').scrollTop = 100"}));
        assert_eq!(pixel(&shot(&mut h), 20, 50), [0, 0, 255, 255], "scrolled to the second block");
        h.page("Runtime.evaluate", json!({"expression": "document.getElementById('box').scrollTop = 0"}));
        assert_eq!(pixel(&shot(&mut h), 20, 50), [255, 0, 0, 255], "back at the origin");
    }

    #[test]
    fn screenshots_draw_external_svg_sprites() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut chunk) {
                            Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let (kind, body) = if request.starts_with(b"GET /sprite.svg") {
                        (
                            "image/svg+xml",
                            r#"<svg xmlns="http://www.w3.org/2000/svg"><symbol id="icon" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></symbol></svg>"#,
                        )
                    } else {
                        (
                            "text/html",
                            r#"<!doctype html><body style=margin:0><svg width="50" height="50"><use href="/sprite.svg#icon"/></svg>"#,
                        )
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body.as_bytes());
                });
            }
        });
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 100, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        h.page("Page.enable", json!({}));
        h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
        h.until_event(|m| m["method"] == "Page.loadEventFired");
        let img = png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!(pixel(&img, 25, 25), [255, 0, 0, 255], "the sprite's symbol is drawn");
        assert_eq!(pixel(&img, 75, 75), [255, 255, 255, 255]);
        // A second capture reuses the resolved sprite.
        let again = png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!(pixel(&again, 25, 25), [255, 0, 0, 255]);
    }

    /// `mobile: true`, as Playwright sends it for `devices['iPhone 13']`: a
    /// page without `<meta name="viewport">` lays out 980 px wide, zoomed out
    /// to the device's width. Expected: Chrome 147 (Playwright 1.59.1's
    /// Chromium) on the same page.
    #[test]
    fn mobile_metrics_layout_metrics_and_screenshots_follow_the_meta_viewport() {
        let mut h = Harness::new();
        let iphone = json!({"width":390,"height":664,"deviceScaleFactor":3,"mobile":true,"screenWidth":390,"screenHeight":844,"screenOrientation":{"angle":0,"type":"portraitPrimary"}});
        assert_eq!(h.page("Emulation.setDeviceMetricsOverride", iphone.clone())["result"], json!({}));
        load_html(
            &mut h,
            "<!doctype html><body style=margin:0><div style='height:200px;background:linear-gradient(90deg,red,blue)'></div><div style='width:50%;height:300px;background:lime'></div>",
        );
        let probe = "[innerWidth, innerHeight, document.documentElement.clientWidth, document.documentElement.clientHeight, visualViewport.scale, outerWidth, screen.height]";
        assert_eq!(eval_value(&mut h, probe), json!([980, 1669, 980, 1668, 0.3979591727256775, 390, 844]));
        let m = h.page("Page.getLayoutMetrics", json!({}))["result"].clone();
        let layout = json!({"pageX":0,"pageY":0,"clientWidth":980,"clientHeight":1669});
        let visual = json!({"offsetX":0,"offsetY":0,"pageX":0,"pageY":0,"clientWidth":980.0,"clientHeight":1668.5128173828125,"scale":0.3979591727256775,"zoom":1});
        let content = json!({"x":0,"y":0,"width":980.0,"height":1669.0});
        assert_eq!(
            m,
            json!({"layoutViewport":layout,"visualViewport":visual,"contentSize":content,
                   "cssLayoutViewport":layout,"cssVisualViewport":visual,"cssContentSize":content})
        );
        // Without a clip: the visual viewport in device pixels (Chrome: 1170 x 1992), the
        // band's red start and blue end in view, the lime block over the left half.
        let shows_the_layout = |img: &pyro_render::RgbaImage| {
            let [r, _, b, _] = pixel(img, 10, 100);
            assert!(r > 200 && b < 60, "red start: {:?}", pixel(img, 10, 100));
            let [r, _, b, _] = pixel(img, 1160, 100);
            assert!(b > 200 && r < 60, "blue end: {:?}", pixel(img, 1160, 100));
            assert_eq!(pixel(img, 290, 400), [0, 255, 0, 255]);
            assert_eq!(pixel(img, 570, 400), [0, 255, 0, 255]);
            assert_eq!(pixel(img, 600, 400), [255, 255, 255, 255]);
        };
        let img = png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!((img.width, img.height), (1170, 1992));
        shows_the_layout(&img);
        // Playwright's clip, the visual viewport in whole CSS px at its scale (Chrome: 1170 x 1991).
        let clip = json!({"clip": {"x":0,"y":0,"width":980,"height":1668,"scale":0.3979591727256775}});
        let img = png_of(&h.page("Page.captureScreenshot", clip)["result"]["data"]);
        assert_eq!((img.width, img.height), (1170, 1991));
        shows_the_layout(&img);
        // `mobile: false` lays the page out at the device's width again.
        let mut desktop = iphone;
        desktop["mobile"] = json!(false);
        assert_eq!(h.page("Emulation.setDeviceMetricsOverride", desktop)["result"], json!({}));
        assert_eq!(eval_value(&mut h, probe), json!([390, 664, 390, 664, 1, 390, 844]));
        let img = png_of(&h.page("Page.captureScreenshot", json!({}))["result"]["data"]);
        assert_eq!((img.width, img.height), (1170, 1992));
        // At scale 1 the band is 600 device px tall and the lime block half of 390 px.
        assert_eq!(pixel(&img, 290, 1000), [0, 255, 0, 255]);
        assert_eq!(pixel(&img, 600, 1000), [255, 255, 255, 255]);
    }

    #[test]
    fn capture_screenshot_renders_the_viewport_clip_and_full_page() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 640, "height": 480, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(&mut h, CARD);
        let r = h.page("Page.captureScreenshot", json!({}));
        let img = png_of(&r["result"]["data"]);
        assert_eq!((img.width, img.height), (640, 480), "{r}");
        assert_eq!(pixel(&img, 100, 100), [0x6d, 0x28, 0xd9, 255]);
        assert_eq!(pixel(&img, 5, 5), [0xf3, 0xf4, 0xf6, 255]);

        let r = h.page(
            "Page.captureScreenshot",
            json!({"clip": {"x": 40, "y": 40, "width": 300, "height": 120, "scale": 1}}),
        );
        let img = png_of(&r["result"]["data"]);
        assert_eq!((img.width, img.height), (300, 120), "{r}");
        assert_eq!(pixel(&img, 150, 60), [0x6d, 0x28, 0xd9, 255]);

        let r = h.page("Page.captureScreenshot", json!({"captureBeyondViewport": true}));
        let img = png_of(&r["result"]["data"]);
        assert_eq!(img.width, 640, "{r}");
        assert!(img.height > 2000, "full page is {}x{}", img.width, img.height);

        let r = h.page("Page.captureScreenshot", json!({"format": "jpeg", "quality": 50}));
        let bytes = pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8], "JPEG magic");
        let r = h.page("Page.captureScreenshot", json!({"format": "webp"}));
        assert!(r["error"]["message"].as_str().unwrap().contains("webp"), "{r}");
    }

    #[test]
    fn match_screenshot_records_matches_hits_the_hash_and_fails_on_a_change() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(&mut h, CARD);

        // No baseline: the render comes back.
        let r = h.page("Pyro.run", json!({"steps": [{"matchScreenshot": {"name": "card", "as": "shot"}}]}));
        let shot = &r["result"]["reads"]["shot"];
        assert_eq!(shot["recorded"], true, "{r}");
        let baseline = shot["actual"].as_str().unwrap().to_owned();
        assert_eq!(png_of(&shot["actual"]).width, 400);

        // Same page against it: compared, then a hash hit (cached per connection).
        let step = json!({"matchScreenshot": {"name": "card", "baseline": baseline, "as": "shot"}});
        let r = h.page("Pyro.run", json!({"steps": [step]}));
        let shot = &r["result"]["reads"]["shot"];
        assert_eq!((shot["pass"].clone(), shot["hashHit"].clone()), (json!(true), json!(false)), "{r}");
        let r = h.page("Pyro.run", json!({"steps": [{"matchScreenshot": {"name": "card", "as": "shot"}}]}));
        let shot = &r["result"]["reads"]["shot"];
        assert_eq!((shot["pass"].clone(), shot["hashHit"].clone()), (json!(true), json!(true)), "{r}");
        assert_eq!(shot["ms"]["raster"], 0.0, "{r}");
        let hash = shot["sceneHash"].as_str().unwrap().to_owned();

        // One CSS color, from a sheet script added: a different scene, failing pixels, images in error.data.
        let r = h.page(
            "Pyro.run",
            json!({"steps": [
                {"eval": {"expression": "const s = document.createElement('style'); s.textContent = '.card { background: #6d28da }'; document.head.append(s); 1"}},
                {"matchScreenshot": {"name": "card", "baseline": baseline}},
            ]}),
        );
        let error = &r["error"];
        assert!(error["message"].as_str().unwrap().contains("pixels differ"), "{r}");
        let data = &error["data"];
        assert_eq!(data["pass"], false, "{r}");
        assert_ne!(data["sceneHash"], json!(hash), "{r}");
        // Only the card changed (less the background showing in its rounded corners).
        let diff_pixels = data["diffPixels"].as_u64().unwrap();
        assert!((35_000..=36_000).contains(&diff_pixels), "{diff_pixels}");
        assert_eq!(png_of(&data["actual"]).width, 400);
        let diff = png_of(&data["diff"]);
        assert_eq!(pixel(&diff, 100, 100), [255, 0, 0, 255]);

        // A tolerance absorbs a one-unit change.
        let step = json!({"matchScreenshot": {"name": "card", "baseline": baseline, "threshold": 0.01}});
        let r = h.page("Pyro.run", json!({"steps": [step]}));
        assert!(r["result"]["results"][0]["pass"] == true, "{r}");
    }

    fn mirror_of(r: &Value) -> Value {
        r["result"]["pyro"]["ms"]["mirror"].clone()
    }

    #[test]
    fn repeat_screenshots_update_the_persistent_document() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(&mut h, CARD);
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(
            mirror_of(&r),
            json!({"rebuilt": "first", "mutations": mirror_of(&r)["mutations"], "pages": 1}),
            "{r}"
        );
        // Nothing changed: the same tree, not even diffed.
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r), json!({"rebuilt": null, "mutations": 0, "pages": 1}), "{r}");
        let before = png_of(&r["result"]["data"]);
        // A script changes one class and one text: applied in place.
        h.page(
            "Runtime.evaluate",
            json!({"expression": "document.getElementById('card').style.background = '#0000ff'; document.body.append('x'); 1"}),
        );
        let r = h.page("Page.captureScreenshot", json!({}));
        let m = mirror_of(&r);
        assert_eq!(m["rebuilt"], Value::Null, "{r}");
        assert!((1..10).contains(&m["mutations"].as_u64().unwrap()), "{r}");
        let after = png_of(&r["result"]["data"]);
        assert_eq!(pixel(&before, 100, 100), [0x6d, 0x28, 0xd9, 255]);
        assert_eq!(pixel(&after, 100, 100), [0, 0, 255, 255]);
        // Typed text (form state) is applied too.
        h.page("Pyro.run", json!({"steps": [{"act": {"locator": {"css": "#name"}, "action": "fill", "value": "Ada"}}]}));
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r)["rebuilt"], Value::Null, "{r}");
        assert_ne!(png_of(&r["result"]["data"]), after, "typed text is rendered");
        // A navigation is another document.
        load_html(&mut h, CARD);
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r)["rebuilt"], "document", "{r}");
        assert_eq!(png_of(&r["result"]["data"]), before);
    }

    #[test]
    fn only_pages_that_take_screenshots_keep_a_document_until_they_close() {
        let mut h = Harness::new();
        load_html(&mut h, CARD);
        let first = (h.session.clone(), h.frame_id.clone());
        // A second page that never takes a screenshot.
        let id = h.send(None, "Target.createTarget", json!({"url": "about:blank"}));
        let attached = h.until_response(id).into_iter().find(|m| m["method"] == "Target.attachedToTarget").unwrap();
        let second = (
            attached["params"]["sessionId"].as_str().unwrap().to_owned(),
            attached["params"]["targetInfo"]["targetId"].as_str().unwrap().to_owned(),
        );
        h.session = second.0.clone();
        load_html(&mut h, CARD);
        h.page("Runtime.evaluate", json!({"expression": "for (let i = 0; i < 100; i++) document.body.append('x'); 1"}));
        h.session = first.0.clone();
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r)["pages"], 1, "{r}");
        // Both pages: two documents; the second one closes: one again.
        h.session = second.0.clone();
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r)["pages"], 2, "{r}");
        h.call(None, "Target.closeTarget", json!({"targetId": second.1}));
        h.session = first.0.clone();
        h.page("Runtime.evaluate", json!({"expression": "document.body.append('y'); 1"}));
        let r = h.page("Page.captureScreenshot", json!({}));
        assert_eq!(mirror_of(&r)["pages"], 1, "{r}");
        assert_eq!(mirror_of(&r)["rebuilt"], Value::Null, "{r}");
    }

    #[test]
    fn match_screenshot_captures_form_state_and_elements() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 400, "height": 300, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(&mut h, CARD);
        let r = h.page("Pyro.run", json!({"steps": [{"matchScreenshot": {"name": "form", "as": "s"}}]}));
        let empty = r["result"]["reads"]["s"]["actual"].as_str().unwrap().to_owned();
        let r = h.page(
            "Pyro.run",
            json!({"steps": [
                {"act": {"locator": {"css": "#name"}, "action": "fill", "value": "Ada Lovelace"}},
                {"matchScreenshot": {"name": "form", "baseline": empty}},
            ]}),
        );
        assert!(r["error"]["data"]["diffPixels"].as_u64().unwrap() > 0, "typed text is rendered: {r}");

        let r = h.page(
            "Pyro.run",
            json!({"steps": [{"matchScreenshot": {"name": "card-only", "locator": {"css": "#card"}, "as": "s"}}]}),
        );
        let s = &r["result"]["reads"]["s"];
        assert_eq!((s["width"].clone(), s["height"].clone()), (json!(300), json!(120)), "{r}");
        let img = png_of(&s["actual"]);
        // Rounded corners show the page behind them.
        assert_eq!(pixel(&img, 150, 60), [0x6d, 0x28, 0xd9, 255]);
        assert_eq!(pixel(&img, 0, 0), [0xf3, 0xf4, 0xf6, 255]);

        let r = h.page(
            "Pyro.run",
            json!({"steps": [{"matchScreenshot": {"name": "x", "locator": {"css": "#missing"}, "timeout": 100}}]}),
        );
        assert!(r["error"]["message"].as_str().unwrap().contains("matches no element"), "{r}");
    }

    fn frames_in(messages: &[Value]) -> Vec<Value> {
        messages.iter().filter(|m| m["method"] == "Page.screencastFrame").cloned().collect()
    }

    /// Sends a page command, then runs the loop until a screencast frame
    /// comes (it may come with the reply).
    fn frame_after(h: &mut Harness, method: &str, params: Value) -> Value {
        let seen = h.page_with_events(method, params);
        match frames_in(&seen).into_iter().next() {
            Some(frame) => frame,
            None => h.until_event(|m| m["method"] == "Page.screencastFrame"),
        }
    }

    /// Sends a page command and runs the loop for `ms`: the frames sent.
    fn frames_after(h: &mut Harness, method: &str, params: Value, ms: u64) -> Vec<Value> {
        let mut seen = h.page_with_events(method, params);
        seen.extend(h.run_for(ms));
        frames_in(&seen)
    }

    fn eval_params(expression: &str) -> Value {
        json!({"expression": expression, "returnByValue": true})
    }

    /// `Page.startScreencast` as Chromium sends it (Playwright's video, and
    /// Puppeteer's `page.screencast()` waits for the first frame): a frame at
    /// once, then one per change that shows, none while nothing changes or the
    /// last frame is unacked, none after `stopScreencast`.
    #[test]
    fn screencast_sends_a_frame_per_change_after_each_ack() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 800, "height": 600, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><body style='margin:0;background:#fff'><div id=box style='width:100px;height:100px;background:#00f'></div>",
        );
        let first =
            frame_after(&mut h, "Page.startScreencast", json!({"format": "png", "maxWidth": 400, "maxHeight": 400}));
        let p = &first["params"];
        let img = png_of(&p["data"]);
        assert_eq!((img.width, img.height), (400, 300), "scaled down to fit maxWidth x maxHeight");
        assert_eq!(pixel(&img, 10, 10), [0, 0, 255, 255]);
        assert_eq!(pixel(&img, 200, 200), [255, 255, 255, 255]);
        let m = &p["metadata"];
        assert_eq!((m["deviceWidth"].as_u64(), m["deviceHeight"].as_u64()), (Some(800), Some(600)), "{m}");
        assert_eq!((m["offsetTop"].as_f64(), m["pageScaleFactor"].as_f64()), (Some(0.0), Some(1.0)), "{m}");
        assert_eq!((m["scrollOffsetX"].as_f64(), m["scrollOffsetY"].as_f64()), (Some(0.0), Some(0.0)), "{m}");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        assert!((now - m["timestamp"].as_f64().unwrap()).abs() < 60.0, "seconds since the epoch: {m}");
        let id = p["sessionId"].as_u64().expect("sessionId");

        // Unacked: the change waits for the ack, then comes.
        let set = "document.getElementById('box').style.background = '#f00'";
        assert!(frames_after(&mut h, "Runtime.evaluate", eval_params(set), 300).is_empty(), "unacked");
        let second = frame_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}));
        assert_eq!(second["params"]["sessionId"], id, "one session id per screencast");
        assert_eq!(pixel(&png_of(&second["params"]["data"]), 10, 10), [255, 0, 0, 255], "the change made meanwhile");
        let t1 = second["params"]["metadata"]["timestamp"].as_f64().unwrap();
        assert!(t1 >= m["timestamp"].as_f64().unwrap() + 0.25, "taken when sent for");

        // Acked, nothing changes: no frame. A change nothing shows: rendered, not sent.
        assert!(frames_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}), 300).is_empty());
        let hidden = "document.body.dataset.x = '1'";
        assert!(frames_after(&mut h, "Runtime.evaluate", eval_params(hidden), 300).is_empty(), "same pixels");
        let wider = "document.getElementById('box').style.width = '200px'";
        let third = frame_after(&mut h, "Runtime.evaluate", eval_params(wider));
        assert_eq!(pixel(&png_of(&third["params"]["data"]), 75, 10), [255, 0, 0, 255]);

        // Scrolling is a change, reported in the metadata.
        assert!(frames_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}), 100).is_empty());
        let scroll = "document.body.style.height = '3000px'; scrollTo(0, 500)";
        let scrolled = frame_after(&mut h, "Runtime.evaluate", eval_params(scroll));
        assert_eq!(scrolled["params"]["metadata"]["scrollOffsetY"], 500.0, "{}", scrolled["params"]["metadata"]);
        assert_eq!(pixel(&png_of(&scrolled["params"]["data"]), 10, 10), [255, 255, 255, 255], "the box scrolled away");

        // Stopped: no more frames. Started again: a frame at once, a new session id, JPEG.
        assert!(frames_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}), 100).is_empty());
        h.page("Page.stopScreencast", json!({}));
        let gone = "document.getElementById('box').style.width = '300px'";
        assert!(frames_after(&mut h, "Runtime.evaluate", eval_params(gone), 300).is_empty(), "stopped");
        let jpeg = frame_after(&mut h, "Page.startScreencast", json!({"format": "jpeg", "quality": 60}));
        assert_ne!(jpeg["params"]["sessionId"], id);
        let bytes = pyro_cdp::fetch_protocol::base64_decode(jpeg["params"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(&bytes[..3], &[0xff, 0xd8, 0xff], "JPEG");
        let r = h.page("Page.startScreencast", json!({"format": "webp"}));
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("webp"), "{r}");
    }

    /// Frames are capped: a page that changes every few milliseconds gets at
    /// most one frame per 40 ms (`everyNthFrame` n: per n × 40 ms) however
    /// fast the client acks.
    #[test]
    fn screencast_frames_are_capped_at_25_per_second() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><body style=margin:0><div id=n>0</div><script>let i = 0; setInterval(() => n.textContent = ++i, 4)</script>",
        );
        let count = |h: &mut Harness, params: Value| {
            let mut unacked = vec![frame_after(h, "Page.startScreencast", params)];
            let started = Instant::now();
            let mut frames = 0;
            while started.elapsed() < Duration::from_millis(1000) {
                for f in std::mem::take(&mut unacked) {
                    frames += 1;
                    let ack = json!({"sessionId": f["params"]["sessionId"]});
                    unacked.extend(frames_in(&h.page_with_events("Page.screencastFrameAck", ack)));
                }
                unacked.extend(frames_in(&h.run_for(5)));
            }
            h.page("Page.stopScreencast", json!({}));
            h.run_for(50);
            frames
        };
        let all = count(&mut h, json!({}));
        assert!((5..=27).contains(&all), "{all} frames in a second");
        let fifth = count(&mut h, json!({"everyNthFrame": 5}));
        assert!((2..=6).contains(&fifth), "{fifth} frames in a second, every 5th");
    }

    /// A screenshot shows animations where they are when it is taken: a CSS
    /// animation held halfway (`paused`, a negative delay), a transition a
    /// style change just started (a capture is a rendering opportunity, so it
    /// starts the transition first), a script animation paused at a time, and
    /// the base value once they are gone.
    #[test]
    fn capture_screenshot_shows_animations_where_they_are() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 300, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0;background:#fff} div{position:absolute;top:0;width:100px;height:100px}\
             @keyframes fade{from{background-color:rgb(0,200,0)}to{background-color:rgb(200,0,0)}}\
             #css{left:0;animation:fade 10s linear -5s paused}\
             #tr{left:100px;background-color:rgb(0,0,200);transition:background-color 1000s cubic-bezier(0,1,1,0) -500s}\
             #js{left:200px;background-color:rgb(0,0,0)}</style><div id=css></div><div id=tr></div><div id=js></div>",
        );
        h.run_for(50);
        let set = "document.getElementById('tr').style.backgroundColor = 'rgb(200, 0, 200)';
                   const a = document.getElementById('js').animate(
                     [{ backgroundColor: 'rgb(0, 0, 0)' }, { backgroundColor: 'rgb(200, 200, 200)' }], 1000);
                   a.pause(); a.currentTime = 250; 0";
        h.page("Runtime.evaluate", eval_params(set));
        let shot = |h: &mut Harness| {
            let r = h.page("Page.captureScreenshot", json!({"format": "rgba"}));
            let data = pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap()).unwrap();
            move |x: usize| -> [u8; 3] {
                [data[(50 * 300 + x) * 4], data[(50 * 300 + x) * 4 + 1], data[(50 * 300 + x) * 4 + 2]]
            }
        };
        let near = |got: [u8; 3], want: [u8; 3], what: &str| {
            assert!(got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 2), "{what}: {got:?}, want {want:?}");
        };
        let at = shot(&mut h);
        near(at(50), [100, 100, 0], "a paused CSS animation, halfway");
        near(at(150), [100, 0, 200], "a transition, halfway (cubic-bezier(0, 1, 1, 0) flat at 0.5)");
        near(at(250), [50, 50, 50], "a script animation paused at a quarter");
        let gone = "document.getAnimations().forEach(a => a.cancel());
                    document.getElementById('css').style.animation = 'none'; 0";
        h.page("Runtime.evaluate", eval_params(gone));
        h.run_for(50);
        let at = shot(&mut h);
        near(at(50), [255, 255, 255], "no animation: no background");
        near(at(250), [0, 0, 0], "the script animation canceled");
    }

    /// A `matchScreenshot` step leaves animations as Playwright's screenshot
    /// assertions do (`animations: "disabled"`, the default): a finite one
    /// finished (filled forwards at its end), an infinite one canceled, the
    /// same pixels whenever the step runs; `allow` shows them where they are.
    #[test]
    fn match_screenshot_settles_animations_unless_allowed() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0;background:#fff}\
             @keyframes grow{from{width:10px}to{width:100px}} @keyframes tint{to{background-color:rgb(255,0,0)}}\
             #a{height:50px;background:rgb(0,0,255);animation:grow 10s linear forwards}\
             #b{height:50px;width:50px;background-color:rgb(0,128,0);animation:tint 1s linear infinite}</style>\
             <div id=a></div><div id=b></div>",
        );
        h.run_for(50);
        let shot = |h: &mut Harness, name: &str, animations: Option<&str>| {
            let mut step = json!({"name": name, "as": "s"});
            if let Some(a) = animations {
                step["animations"] = json!(a);
            }
            let r = h.page("Pyro.run", json!({"steps": [{"matchScreenshot": step}]}));
            png_of(&r["result"]["reads"]["s"]["actual"])
        };
        let settled = shot(&mut h, "settled", None);
        assert_eq!(pixel(&settled, 90, 25), [0, 0, 255, 255], "finished: filled forwards at 100px");
        assert_eq!(pixel(&settled, 25, 75), [0, 128, 0, 255], "the infinite one canceled");
        h.run_for(150);
        let again = shot(&mut h, "settled-again", Some("disabled"));
        assert!(settled.data == again.data, "the same pixels whenever it is taken");
        let live = shot(&mut h, "live", Some("allow"));
        assert_eq!(pixel(&live, 90, 25), [255, 255, 255, 255], "allowed: a few percent grown");
        let r = h.page("Pyro.run", json!({"steps": [{"matchScreenshot": {"name": "x", "animations": "paused"}}]}));
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("animations"), "{r}");
    }

    /// A screencast follows a running animation: frames keep coming while it
    /// moves although the DOM does not change, and stop once it is still.
    #[test]
    fn screencast_frames_follow_a_running_animation() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 100, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><style>body{margin:0}@keyframes fade{from{background-color:rgb(0,0,0)}to{background-color:rgb(250,250,250)}}\
             div{width:100px;height:100px;animation:fade 400ms linear forwards}</style><div></div>",
        );
        let first = frame_after(&mut h, "Page.startScreencast", json!({"format": "png"}));
        let mut shades = vec![pixel(&png_of(&first["params"]["data"]), 50, 50)[0]];
        let mut id = first["params"]["sessionId"].clone();
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(1200) {
            let frames = frames_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}), 60);
            for f in frames {
                shades.push(pixel(&png_of(&f["params"]["data"]), 50, 50)[0]);
                id = f["params"]["sessionId"].clone();
            }
        }
        assert!(shades.len() >= 4, "a frame per interval while it runs: {shades:?}");
        assert!(shades.windows(2).all(|w| w[0] <= w[1]), "lighter frame by frame: {shades:?}");
        assert_eq!(*shades.last().unwrap(), 250, "it ends filled forwards: {shades:?}");
        assert!(
            frames_after(&mut h, "Page.screencastFrameAck", json!({"sessionId": id}), 200).is_empty(),
            "still: no frame"
        );
    }

    /// pyro's raw capture formats (frames for ffmpeg, nothing encoded): `rgba` is the render
    /// as it is, `rgb24` drops alpha, `yuv420p` is I420. With an absolute `path` the frame is
    /// appended to that file and the reply's data is empty; the reply gives the frame's size,
    /// the write's time and the process's CPU time. A FIFO nobody reads fails at once.
    #[test]
    fn capture_screenshot_raw_formats_and_path() {
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 40, "height": 20, "deviceScaleFactor": 2, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><body style='margin:0;background:#fff'><div style='width:20px;height:20px;background:#f00'></div>",
        );
        let capture = |h: &mut Harness, params: Value| {
            let r = h.page("Page.captureScreenshot", params);
            let report = r["result"]["pyro"].clone();
            let data =
                pyro_cdp::fetch_protocol::base64_decode(r["result"]["data"].as_str().unwrap_or_else(|| panic!("{r}")))
                    .unwrap();
            (report, data)
        };
        let (report, rgba) = capture(&mut h, json!({"format": "rgba"}));
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64(), report["bytes"].as_u64()),
            (Some(80), Some(40), Some(12800))
        );
        assert_eq!(rgba.len(), 80 * 40 * 4);
        let at = |x: usize, y: usize| &rgba[(y * 80 + x) * 4..(y * 80 + x) * 4 + 4];
        assert_eq!((at(10, 10), at(70, 10)), (&[255, 0, 0, 255][..], &[255, 255, 255, 255][..]));
        assert!(report["ms"]["cpu"].as_f64().unwrap() > 0.0, "{report}");
        let (report, rgb) = capture(&mut h, json!({"format": "rgb24"}));
        assert_eq!((report["bytes"].as_u64(), rgb.len(), &rgb[..3]), (Some(9600), 9600, &[255, 0, 0][..]));
        let (report, yuv) = capture(&mut h, json!({"format": "yuv420p"}));
        assert_eq!((report["bytes"].as_u64(), yuv.len()), (Some(4800), 4800));
        assert_eq!((yuv[10 * 80 + 10], yuv[10 * 80 + 70]), (82, 235), "red and white luma");

        let dir = std::env::temp_dir().join(format!("pyro-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("frames.rgb");
        let path = file.to_str().unwrap();
        for _ in 0..2 {
            let (report, data) = capture(&mut h, json!({"format": "rgb24", "path": path}));
            assert!(data.is_empty(), "written, not returned");
            assert_eq!(report["bytes"].as_u64(), Some(9600));
            assert!(report["ms"]["write"].as_f64().is_some(), "{report}");
        }
        let written = std::fs::read(&file).unwrap();
        assert_eq!(written.len(), 2 * 9600, "appended");
        assert_eq!(&written[9600..], &rgb[..], "the same frame twice");
        let r = h.page("Page.captureScreenshot", json!({"format": "rgba", "path": "frames.rgba"}));
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("not absolute"), "{r}");
        let fifo = dir.join("fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let r = h.page("Page.captureScreenshot", json!({"format": "rgba", "path": fifo.to_str().unwrap()}));
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("cannot write"), "no reader: {r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs the loop until a whole raw frame came on `socket`: its header (width, height,
    /// timestamp) and planes.
    fn raw_frame(h: &mut Harness, socket: &mut std::os::unix::net::UnixStream) -> (u32, u32, f64, Vec<u8>) {
        use std::io::Read;
        socket.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = Vec::new();
        loop {
            let mut chunk = [0u8; 65536];
            match socket.read(&mut chunk) {
                Ok(0) => panic!("the socket closed"),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(h.run_for(5).iter().all(|m| m["method"] != "Page.screencastFrame"), "no events");
                }
                Err(e) => panic!("{e}"),
            }
            if buf.len() >= 20 {
                assert_eq!(&buf[..4], b"PYF1");
                let w = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                let ht = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                let t = f64::from_le_bytes(buf[12..20].try_into().unwrap());
                let len = (w * ht + 2 * w.div_ceil(2) * ht.div_ceil(2)) as usize;
                if buf.len() >= 20 + len {
                    assert_eq!(buf.len(), 20 + len, "nothing past the frame before its ack");
                    return (w, ht, t, buf[20..].to_vec());
                }
            }
            assert!(Instant::now() < deadline, "no frame; {} bytes", buf.len());
        }
    }

    /// pyro's `yuv420p` screencast (wright's `recordVideo`): frames of exactly maxWidth x
    /// maxHeight go raw to the client's Unix socket, the page scaled down to fit at the top
    /// left and gray around it; the next frame waits for the client's one-byte ack; no
    /// events. Without a socket or a size, or with a socket nobody listens on, it fails.
    #[test]
    fn screencast_yuv420p_frames_go_raw_to_a_socket() {
        use std::io::Write;
        let mut h = Harness::new();
        h.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": 200, "height": 100, "deviceScaleFactor": 1, "mobile": false}),
        );
        load_html(
            &mut h,
            "<!doctype html><body style='margin:0;background:#fff'><div id=box style='width:100px;height:100px;background:#000'></div>",
        );
        let dir = std::env::temp_dir().join(format!("pyro-cast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s");
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let socket_path = path.to_str().unwrap();
        let started = json!({"format": "yuv420p", "socket": socket_path, "maxWidth": 100, "maxHeight": 80});
        assert_eq!(h.page("Page.startScreencast", started.clone())["result"], json!({}));
        let (mut socket, _) = listener.accept().unwrap();
        let (w, ht, t, frame) = raw_frame(&mut h, &mut socket);
        assert_eq!((w, ht), (100, 80), "maxWidth x maxHeight exactly");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        assert!((now - t).abs() < 60.0, "seconds since the epoch: {t}");
        let y = |x: usize, row: usize| frame[row * 100 + x];
        // 200x100 scaled by 0.5 into 100x50: the black box on the left half, white on the right,
        // gray (luma 126) under it; chroma neutral everywhere.
        assert_eq!((y(10, 10), y(80, 10), y(10, 70)), (16, 235, 126), "black, white, gray padding");
        assert!(frame[100 * 80..].iter().all(|c| *c == 128), "no chroma in black, white and gray");

        // Unacked: a change waits. Acked: it comes.
        let set = "document.getElementById('box').style.background = '#fff'";
        assert!(frames_after(&mut h, "Runtime.evaluate", eval_params(set), 300).is_empty());
        socket.set_nonblocking(true).unwrap();
        let mut peek = [0u8; 1];
        assert!(std::io::Read::read(&mut socket, &mut peek).is_err(), "nothing before the ack");
        socket.set_nonblocking(false).unwrap();
        socket.write_all(&[1]).unwrap();
        let (_, _, t2, second) = raw_frame(&mut h, &mut socket);
        assert!(t2 > t, "a later snapshot");
        assert_eq!(second[100 * 10 + 10], 235, "the box turned white");

        // Stopped: the socket ends (macOS refuses a read timeout on a socket its peer shut down).
        socket.write_all(&[1]).unwrap();
        h.page("Page.stopScreencast", json!({}));
        socket.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match std::io::Read::read(&mut socket, &mut peek) {
                Ok(0) => break,
                Ok(_) => panic!("a frame after the stop"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    h.run_for(10);
                }
                Err(e) => panic!("not closed: {e}"),
            }
        }

        // Mistakes.
        let error = |h: &mut Harness, params: Value| {
            h.page("Page.startScreencast", params)["error"]["message"].as_str().unwrap_or_default().to_owned()
        };
        assert!(error(&mut h, json!({"format": "yuv420p", "maxWidth": 100, "maxHeight": 80})).contains("socket"));
        assert!(
            error(&mut h, json!({"format": "yuv420p", "socket": socket_path, "maxWidth": 100})).contains("maxHeight")
        );
        drop(listener);
        let _ = std::fs::remove_file(&path);
        assert!(error(&mut h, started).contains("cannot write frames to socket"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn driver_attribute_and_description_checks_retry_and_reject_mismatches() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<input id='field' aria-describedby='help'><span id='help'>Must be greater than 0</span><script>setTimeout(()=>document.querySelector('#field').setAttribute('aria-invalid','true'),20)</script>",
    );
    let result = h.page(
        "Pyro.run",
        json!({"steps":[
            {"check":{"locator":{"css":"#field"},"assert":"attribute","property":"aria-invalid","expected":"true"}},
            {"check":{"locator":{"css":"#field"},"assert":"description","expected":{"regex":"greater than 0"}}},
            {"check":{"locator":{"css":"#field"},"assert":"attribute","property":"missing","expected":null}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    let failed = h.page("Pyro.run",json!({"steps":[{"check":{"locator":{"css":"#field"},"assert":"attribute","property":"aria-invalid","expected":"false","timeout":50}}]}));
    assert!(failed.get("error").is_some(), "{failed}");
}

#[test]
fn driver_description_unresolved_references_fall_back_but_empty_targets_win() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<input id="aria" aria-describedby="missing" aria-description="ARIA description" title="Title description">
        <input id="title" aria-describedby="missing" title="Title description">
        <input id="empty" aria-describedby="empty-help" aria-description="ARIA description" title="Title description">
        <span id="empty-help"></span>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"steps":[
            {"check":{"locator":{"css":"#aria"},"assert":"description","expected":"ARIA description"}},
            {"check":{"locator":{"css":"#title"},"assert":"description","expected":"Title description"}},
            {"check":{"locator":{"css":"#empty"},"assert":"description","expected":""}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
}

#[test]
fn driver_svg_hover_ignores_fully_inset_clipped_label_and_descendants() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<style>
        button{display:inline-flex;align-items:center;justify-content:center;position:relative;width:20px;height:20px;padding:0;border:0}
        .sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
        </style><button aria-label="Dunning Blocked"><svg width="20" height="20"><circle cx="10" cy="10" r="9"/></svg><span class="sr-only"><span>Dunning Blocked</span></span></button>
        <script>window.hovered=false;document.querySelector('button').addEventListener('mouseover',()=>window.hovered=true)</script>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"steps":[
            {"act":{"locator":{"css":"svg"},"action":"hover","timeout":200}},
            {"eval":{"expression":"window.hovered","as":"hovered"}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["hovered"], true, "{result}");
}

#[test]
fn driver_svg_hover_still_rejects_unclipped_label_and_overlay() {
    for cover in [
        "<span style='position:absolute;width:1px;height:1px;margin:-1px;overflow:hidden'>Label</span>",
        "<span style='position:absolute;inset:0;background:red'>Overlay</span>",
    ] {
        let mut h = Harness::new();
        load_html(
            &mut h,
            &format!(
                r#"<style>button{{display:inline-flex;align-items:center;justify-content:center;position:relative;width:20px;height:20px;padding:0;border:0}}</style><button><svg width="20" height="20"><circle cx="10" cy="10" r="9"/></svg>{cover}</button>"#
            ),
        );
        let result = h.page(
            "Pyro.run",
            json!({"steps":[
                {"act":{"locator":{"css":"svg"},"action":"hover","timeout":100}}
            ]}),
        );
        assert!(result["error"]["message"].as_str().unwrap().contains("intercepts pointer events"), "{result}");
    }
}

#[test]
fn driver_right_click_preserves_context_menu_autofocus() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<button id='row'>Row</button><div id='menu' role='menu' tabindex='-1'>Menu</div><script>document.querySelector('#row').addEventListener('contextmenu',e=>{e.preventDefault();document.querySelector('#menu').focus()})</script>",
    );
    let result = h.page(
        "Pyro.run",
        json!({"steps":[
            {"act":{"locator":{"css":"#row"},"action":"click","button":"right"}},
            {"check":{"locator":{"css":"#menu"},"assert":"focused","expected":true}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
}

#[test]
fn prepared_profile_init_scripts_capture_popup_errors_and_survive_navigation() {
    let address = driver_site();
    let mut h = Harness::new();
    h.call(
        None,
        "Target.setAutoAttach",
        json!({"autoAttach": false, "waitForDebuggerOnStart": false, "flatten": true}),
    );
    let r = h.call(None, "Pyro.registerPreparedPage", json!({
        "url": format!("http://{address}/form"), "pool": 1,
        "initScripts": ["window.__errors=[];console.error=(...args)=>window.__errors.push(args.join(' '));window.__profileRuns=(window.__profileRuns||0)+1;"]
    }));
    let profile = r["result"]["profileId"].clone();
    let r = h.call(None, "Pyro.acquirePreparedPage", json!({"profileId": profile, "attach": true}));
    let session = r["result"]["sessionId"].as_str().unwrap().to_owned();
    let r = h.call(Some(&session), "Pyro.run", json!({"steps": [
        {"loaded": {}},
        {"eval": {"expression": "window.open('data:text/html,<script>console.error(%22popup-startup%22)</script>', '_blank', 'noopener');null"}},
        {"popup": {"as": "child"}},
        {"page": {"target": "child"}},
        {"loaded": {}},
        {"eval": {"expression": "[window.__profileRuns, ...window.__errors]", "as": "startup"}},
        {"goto": {"url": "data:text/html,<script>console.error('popup-next')</script>"}},
        {"eval": {"expression": "[window.__profileRuns, ...window.__errors]", "as": "next"}},
        {"page": {"target": "main"}},
        {"eval": {"expression": "[window.__profileRuns, ...window.__errors]", "as": "main"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["startup"], json!([1, "popup-startup"]), "{r}");
    assert_eq!(r["result"]["reads"]["next"], json!([1, "popup-next"]), "{r}");
    assert_eq!(r["result"]["reads"]["main"], json!([1]), "{r}");
    let context = h.call(None, "Target.createBrowserContext", json!({}));
    let target = h.call(
        None,
        "Target.createTarget",
        json!({"url": "about:blank", "browserContextId": context["result"]["browserContextId"]}),
    );
    let attached =
        h.call(None, "Target.attachToTarget", json!({"targetId": target["result"]["targetId"], "flatten": true}));
    let session = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    let r = h.call(
        Some(&session),
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": "data:text/html,<p>isolated</p>"}},
            {"eval": {"expression": "typeof window.__errors", "as": "isolated"}}
        ]}),
    );
    assert_eq!(r["result"]["reads"]["isolated"], "undefined", "{r}");
}

/// Requests a test server answered, as (request line, User-Agent), in order.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// An HTTP server that records every request and answers a document for
/// paths ending in `/page`, `ok <path>` for the rest. It takes absolute-form
/// requests too, as an HTTP proxy would (and answers them itself).
fn recording_site() -> (std::net::SocketAddr, Seen) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Seen::default();
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let record = record.clone();
            std::thread::spawn(move || {
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&request).into_owned();
                let line = head.lines().next().unwrap_or_default().to_owned();
                let agent = head
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("user-agent")))
                    .map(|(_, v)| v.trim().to_owned())
                    .unwrap_or_default();
                let path = line.split(' ').nth(1).unwrap_or("/").to_owned();
                record.lock().unwrap().push((line, agent));
                let (ty, body) = if path.ends_with("/page") {
                    ("text/html", "<!doctype html><p id=ready>ready</p>".to_owned())
                } else {
                    ("text/plain", format!("ok {path}"))
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            });
        }
    });
    (address, seen)
}

/// A prepared profile's `userAgent` and proxy belong to its pages' browser
/// context: the document request, fetches and `navigator.userAgent` of its
/// pages have them; pages of other contexts keep the browser's.
#[test]
fn prepared_profiles_send_their_user_agent_and_go_through_their_proxy() {
    let (site, seen) = recording_site();
    let mut h = Harness::new();
    let take = |h: &mut Harness, params: Value| {
        let r = h.call(None, "Pyro.registerPreparedPage", params);
        let profile = r["result"]["profileId"].as_u64().unwrap_or_else(|| panic!("{r}"));
        let r = h.call(None, "Pyro.acquirePreparedPage", json!({"profileId": profile, "attach": true}));
        let session = r["result"]["sessionId"].as_str().unwrap_or_else(|| panic!("{r}")).to_owned();
        h.call(
            Some(&session),
            "Pyro.run",
            json!({"steps": [
                {"loaded": {}},
                {"eval": {"expression": "navigator.userAgent", "as": "agent"}},
                {"eval": {"expression": "fetch('/api').then((r) => r.text())", "as": "api"}},
            ]}),
        )
    };

    let agent = "ScraperBot/1.0 (identity one)";
    let r = take(&mut h, json!({"url": format!("http://{site}/direct/page"), "pool": 1, "userAgent": agent}));
    assert_eq!(r["result"]["reads"], json!({"agent": agent, "api": "ok /api"}), "{r}");
    // Through the proxy: `proxied.test` resolves nowhere, only the proxy answers for it.
    let r = take(
        &mut h,
        json!({"url": "http://proxied.test/page", "pool": 1, "userAgent": "Proxied/2.0", "proxyServer": format!("http://{site}")}),
    );
    assert_eq!(r["result"]["reads"], json!({"agent": "Proxied/2.0", "api": "ok http://proxied.test/api"}), "{r}");
    let seen = seen.lock().unwrap().clone();
    let request = |line: &str| seen.iter().find(|(l, _)| l == line).map(|(_, a)| a.clone());
    assert_eq!(request("GET /direct/page HTTP/1.1").as_deref(), Some(agent), "{seen:?}");
    assert_eq!(request("GET /api HTTP/1.1").as_deref(), Some(agent), "{seen:?}");
    assert_eq!(request("GET http://proxied.test/page HTTP/1.1").as_deref(), Some("Proxied/2.0"), "{seen:?}");
    assert_eq!(request("GET http://proxied.test/api HTTP/1.1").as_deref(), Some("Proxied/2.0"), "{seen:?}");

    // The harness's own page (another context) keeps the browser's agent.
    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "navigator.userAgent", "as": "agent"}}]}));
    let own = r["result"]["reads"]["agent"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert!(own.starts_with("Mozilla/5.0 (compatible; pyro/"), "{r}");
}

#[test]
fn driver_clicks_through_display_contents_overflow_without_ignoring_real_clips() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body style="margin:0">
<button id=button style="display:flex;width:255px;height:28px;overflow:hidden;margin-left:740px;margin-top:200px"><span style="display:contents;overflow:hidden"><span id=label style="display:block;width:203px;height:20px;overflow:hidden">Conversation label</span></span></button><output id=out></output>
<div style="width:0;height:0;overflow:hidden"><span id=hidden style="display:block;width:100px;height:20px">Actually clipped</span></div>
"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 100, "steps": [
            {"eval": {"expression": "document.getElementById('button').addEventListener('click', () => document.getElementById('out').textContent='clicked');null"}},
            {"act": {"locator": {"css": "#label"}, "action": "click"}},
            {"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "clicked"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 100, "steps": [
            {"act": {"locator": {"css": "#hidden"}, "action": "click"}}
        ]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("outside the viewport"), "{r}");
}

#[test]
fn driver_fills_time_controls_with_input_change_and_rejects_invalid_or_readonly_values() {
    let mut h = Harness::new();
    load_html(&mut h, "<input id=time type=time value='09:00'><input id=readonly type=time readonly value='08:00'>");
    let r = h.page("Pyro.run", json!({"steps": [
        {"eval": {"expression": "window.events=[];for(const type of ['input','change'])document.querySelector('#time').addEventListener(type,e=>events.push(type+':'+e.target.value));null"}},
        {"act": {"locator": {"css": "#time"}, "action": "fill", "value": "13:45"}},
        {"check": {"locator": {"css": "#time"}, "assert": "value", "expected": "13:45"}},
        {"act": {"locator": {"css": "#time"}, "action": "fill", "value": ""}},
        {"eval": {"expression": "events", "as": "events"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["events"], json!(["input:13:45", "change:13:45", "input:", "change:"]), "{r}");
    for (selector, value, reason) in [("#time", "24:00", "malformed time"), ("#readonly", "10:00", "not editable")] {
        let r = h.page(
            "Pyro.run",
            json!({"timeout": 50, "steps": [
                {"act": {"locator": {"css": selector}, "action": "fill", "value": value}}
            ]}),
        );
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains(reason), "{r}");
    }
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"check": {"locator": {"css": "#time"}, "assert": "value", "expected": ""}},
            {"check": {"locator": {"css": "#readonly"}, "assert": "value", "expected": "08:00"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
}

#[test]
fn driver_clicks_fixed_dialog_content_outside_overflow_ancestor() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><div style="width:20px;height:20px;overflow:hidden"><div style="position:fixed;left:100px;top:100px;width:200px;height:100px"><button id=inside>Dialog action</button></div></div><output id=out></output>"#,
    );
    let r = h.page("Pyro.run", json!({"timeout":3000,"steps":[
        {"eval":{"expression":"document.querySelector('#inside').addEventListener('click',()=>document.querySelector('#out').textContent='clicked');null"}},
        {"act":{"locator":{"css":"#inside"},"action":"click"}},
        {"check":{"locator":{"css":"#out"},"assert":"text","expected":"clicked"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
}

#[test]
fn driver_release_replaces_held_real_response_body_preserving_status_and_headers() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page("Pyro.run", json!({"steps":[
        {"goto":{"url":format!("http://{address}/")}},
        {"mock":{"url":"/api","method":"PATCH","passthrough":true,"hold":"save","times":1}},
        {"eval":{"expression":"window.held=null;fetch('/api',{method:'PATCH',headers:{'x-idempotency-key':'preserved'},body:'persisted'}).then(async r=>{window.held={status:r.status,key:r.headers.get('x-echo-key'),type:r.headers.get('content-type'),body:await r.json()};document.querySelector('#out').textContent='released'});null"}},
        {"request":{"path":"/api","method":"PATCH","state":"started","count":1}},
        {"held":{"hold":"save","count":1,"status":200,"as":"parked"}},
        {"advanceTime":{"ms":120001}},
        {"eval":{"expression":"fetch('/api?fresh').then(r=>r.json())","as":"fresh"}},
        {"release":{"hold":"save","body":"{\"name\":\"fresh\",\"width\":77}"}},
        {"check":{"locator":{"css":"#out"},"assert":"text","expected":"released"}},
        {"eval":{"expression":"held","as":"held"}},
        {"request":{"path":"/api","method":"PATCH","status":200,"count":1}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(
        r["result"]["reads"]["held"],
        json!({"status":200,"key":"preserved","type":"application/json","body":{"name":"fresh","width":77}}),
        "{r}"
    );
    assert_eq!(r["result"]["reads"]["fresh"]["method"], "GET", "{r}");
    assert_eq!(r["result"]["reads"]["parked"]["responses"][0]["json"]["method"], "PATCH", "{r}");
    assert_eq!(r["result"]["reads"]["parked"]["responses"][0]["json"]["body"], "persisted", "{r}");
}

#[test]
fn driver_request_count_does_not_ignore_explicit_status() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"timeout":100,"steps":[
            {"goto":{"url":format!("http://{address}/")}},
            {"mock":{"url":"/api","status":500,"body":"failure"}},
            {"eval":{"expression":"call('/api');null"}},
            {"request":{"path":"/api","count":1,"status":200}}
        ]}),
    );
    assert!(r["error"]["message"].as_str().unwrap_or_default().contains("status differs"), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"steps":[
            {"eval":{"expression":"call('/api');null"}},
            {"request":{"path":"/api","count":1,"status":500}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
}

#[test]
fn driver_request_diagnostics_redact_credentials_without_changing_matching() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let setup = json!([
        {"goto":{"url":format!("http://{address}/")}},
        {"mock":{"url":"/api","status":200,"headers":{"Set-Cookie":"session=private-cookie","x-test":"public"},"body":"ok"}},
        {"eval":{"expression":"fetch('/api',{headers:{Authorization:'Bearer private-token','X-Api-Key':'private-api-key','X-Idempotency-Key':'public-id'}});null"}},
        {"request":{"path":"/api","headers":{"Authorization":"Bearer private-token","X-Api-Key":"private-api-key"},"status":200}}
    ]);
    let r = h.page("Pyro.run", json!({"steps":setup}));
    assert!(r.get("error").is_none(), "{r}");
    let output = r.to_string();
    for secret in ["private-token", "private-api-key", "private-cookie"] {
        assert!(!output.contains(secret), "credential present in successful run output");
    }
    let request = &r["result"]["results"][3];
    assert_eq!(request["requestHeaders"]["authorization"], "[REDACTED]");
    assert_eq!(request["requestHeaders"]["x-api-key"], "[REDACTED]");
    assert_eq!(request["requestHeaders"]["x-idempotency-key"], "public-id");
    assert_eq!(request["responseHeaders"]["set-cookie"], "[REDACTED]");
    for count in [Value::Null, json!(1)] {
        let r = h.page("Pyro.run", json!({"timeout":75,"steps":[
            {"eval":{"expression":"fetch('/api',{headers:{Authorization:'Bearer private-token'}});null"}},
            {"request":{"path":"/api","headers":{"Authorization":"Bearer private-token"},"status":201,"count":count}}
        ]}));
        assert!(r.get("error").is_some(), "must reject wrong status");
        assert!(!r.to_string().contains("private-token"), "credential present in failed run diagnostic");
        assert!(r["error"]["message"].as_str().unwrap_or_default().contains("[REDACTED]"));
    }
}

#[test]
fn driver_clicks_rowspanning_cell_content_above_later_row_background() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body style="margin:0"><table style="border-collapse:collapse;width:240px"><tbody><tr><td style="height:20px;width:120px;padding:0">First row</td><td rowspan=2 style="width:120px;padding:0;vertical-align:middle"><button id=action style="height:20px">action</button></td></tr><tr><td style="height:60px;padding:0">Second row</td></tr></tbody></table><output id=out></output>"#,
    );
    let r = h.page("Pyro.run",json!({"timeout":3000,"steps":[
        {"eval":{"expression":"document.querySelector('#action').addEventListener('click',()=>document.querySelector('#out').textContent='clicked');null"}},
        {"act":{"locator":{"css":"#action"},"action":"click"}},
        {"check":{"locator":{"css":"#out"},"assert":"text","expected":"clicked"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
}

#[test]
fn driver_tooltip_name_comes_from_visible_contents_and_respects_hidden_ancestors() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body><button aria-describedby=tip>Action</button><div role=tooltip id=tip><span>Add reaction</span><svg aria-hidden=true width=10 height=10><path d="M0 0L10 10"/></svg></div><div aria-hidden=true><div role=tooltip>Hidden tooltip</div></div><div role=tooltip aria-label="Explicit tooltip">Different visible text</div></body>"#,
    );
    let reads = run_reads(
        &mut h,
        &[
            ("visible", "name", json!({"css":"#tip"})),
            ("matched", "count", json!({"role":"tooltip","name":"Add reaction","exact":true})),
            ("hidden", "count", json!({"role":"tooltip","name":"Hidden tooltip","exact":true})),
            ("explicit", "count", json!({"role":"tooltip","name":"Explicit tooltip","exact":true})),
        ],
    );
    assert_eq!(reads, json!({"visible":"Add reaction","matched":1,"hidden":0,"explicit":1}));
}

#[test]
fn driver_context_request_scope_observes_prior_batch_completion_without_weakening_matches() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page("Pyro.run",json!({"steps":[
        {"goto":{"url":format!("http://{address}/")}},
        {"mock":{"url":"/api","method":"PATCH","passthrough":true,"hold":"save","times":1}},
        {"eval":{"expression":"fetch('/api',{method:'PATCH',headers:{Authorization:'Bearer secret-cross-run','content-type':'application/json'},body:JSON.stringify({language:'hu'})}).then(()=>document.querySelector('#out').textContent='done');null"}},
        {"request":{"path":"/api","method":"PATCH","state":"started","count":1}},
        {"held":{"hold":"save","count":1,"status":200}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    let r = h.page("Pyro.run",json!({"steps":[
        {"request":{"path":"/api","method":"PATCH","state":"started","count":0}},
        {"release":{"hold":"save"}},
        {"request":{"path":"/api","method":"PATCH","scope":"context","status":200,"bodyJson":{"language":"hu"},"bodyKeys":["language"],"count":1}},
        {"request":{"path":"/api","method":"PATCH","scope":"context","headers":{"authorization":"Bearer secret-cross-run"},"status":200}},
        {"request":{"path":"/api","method":"PATCH","count":0}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert!(!r.to_string().contains("secret-cross-run"));
    assert_eq!(r["result"]["results"][3]["requestHeaders"]["authorization"], "[REDACTED]");
    for mismatch in [
        json!({"status":201,"count":1}),
        json!({"status":200,"count":2}),
        json!({"status":200,"bodyJson":{"language":"en"},"count":1}),
    ] {
        let mut request = json!({"path":"/api","method":"PATCH","scope":"context"});
        request.as_object_mut().unwrap().extend(mismatch.as_object().unwrap().clone());
        let r = h.page("Pyro.run", json!({"timeout":75,"steps":[{"request":request}]}));
        assert!(r.get("error").is_some(), "mismatched request must fail: {r}");
        assert!(!r.to_string().contains("secret-cross-run"));
    }
    let target = h.call(None, "Target.getTargetInfo", json!({"targetId":h.frame_id}));
    let context = target["result"]["targetInfo"]["browserContextId"].clone();
    let target = h.call(None, "Target.createTarget", json!({"url":"about:blank","browserContextId":context}));
    let attached =
        h.call(None, "Target.attachToTarget", json!({"targetId":target["result"]["targetId"],"flatten":true}));
    let session = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    let r = h.call(
        Some(&session),
        "Pyro.run",
        json!({"steps":[{"request":{"path":"/api","method":"PATCH","scope":"context","count":0}}]}),
    );
    assert!(r.get("error").is_none(), "sibling page must not see another page's history: {r}");
}

#[test]
fn native_wheel_scroll_steps_run_before_animation_callbacks_and_are_not_duplicated() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<!doctype html><div id=scroller style='height:100px;width:200px;overflow:auto'><div style='height:2000px'>content</div></div>",
    );
    let r=h.page("Pyro.run",json!({"steps":[
        {"eval":{"expression":"window.events=[];document.querySelector('#scroller').addEventListener('scroll',e=>events.push('scroll:'+e.target.scrollTop));null"}},
        {"act":{"locator":{"css":"#scroller"},"action":"wheel","deltaY":450,"settle":false}},
        {"eval":{"expression":"new Promise(resolve=>requestAnimationFrame(()=>{events.push('raf1');requestAnimationFrame(()=>{events.push('raf2');resolve(events.slice())})}))","as":"events"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["events"], json!(["scroll:450", "raf1", "raf2"]), "{r}");
    h.run_for(40);
    assert_eq!(eval_value(&mut h, "events"), json!(["scroll:450", "raf1", "raf2"]));
}

#[test]
fn fill_replaces_controlled_value_when_beforeinput_reassigns_unchanged_value() {
    let mut h = Harness::new();
    load_html(&mut h, "<input id=search value=Existing><textarea id=area>Existing area</textarea>");
    let r=h.page("Pyro.run",json!({"steps":[
        {"eval":{"expression":"window.values=[];for(const el of document.querySelectorAll('input,textarea')){el.addEventListener('beforeinput',()=>{el.value=el.value});el.addEventListener('input',()=>{values.push(el.value);el.value=el.value});el.addEventListener('focus',()=>{el.value=el.value});}null"}},
        {"act":{"locator":{"css":"#search"},"action":"fill","value":"First replacement"}},
        {"check":{"locator":{"css":"#search"},"assert":"value","expected":"First replacement"}},
        {"act":{"locator":{"css":"#search"},"action":"fill","value":"Second replacement"}},
        {"check":{"locator":{"css":"#search"},"assert":"value","expected":"Second replacement"}},
        {"act":{"locator":{"css":"#area"},"action":"fill","value":"New area"}},
        {"check":{"locator":{"css":"#area"},"assert":"value","expected":"New area"}},
        {"eval":{"expression":"values","as":"values"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["values"], json!(["First replacement", "Second replacement", "New area"]));
}

#[test]
fn fill_replaces_value_after_focus_handler_replaces_the_control() {
    let mut h = Harness::new();
    load_html(&mut h, "<input id=search value=Existing>");
    let r=h.page("Pyro.run",json!({"timeout":150,"steps":[
        {"eval":{"expression":"const old=document.querySelector('#search');old.addEventListener('focus',()=>{const next=old.cloneNode(true);old.replaceWith(next);next.focus()},{once:true});null"}},
        {"act":{"locator":{"css":"#search"},"action":"fill","value":"First replacement"}},
        {"check":{"locator":{"css":"#search"},"assert":"value","expected":"First replacement"}},
        {"act":{"locator":{"css":"#search"},"action":"fill","value":"Second replacement"}},
        {"check":{"locator":{"css":"#search"},"assert":"value","expected":"Second replacement"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
}

/// The focus of `focus`, `fill`, `press` and `type`, and the `blur` action,
/// run as a script's `focus()` and `blur()` do: the listeners of both events
/// run before the microtasks they queued. A click's focus is the browser's:
/// each listener has its own microtask checkpoint.
#[test]
fn driver_focus_and_blur_run_listeners_before_microtasks_like_script_calls() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><input id=field><button id=other>Other</button>");
    // Each step is its own evaluation: the microtasks of one ran before the next.
    let eval = |expression: &str| json!({"eval": {"expression": expression}});
    let read = |name: &str| json!({"eval": {"expression": "log.join()", "as": name}});
    let act = |action: &str, value: Option<&str>| {
        let mut act = json!({"locator": {"css": "#field"}, "action": action});
        if let Some(value) = value {
            act["value"] = json!(value);
        }
        json!({ "act": act })
    };
    let (unfocus, clear) = (eval("document.activeElement.blur();null"), eval("log.length=0;null"));
    let mut steps = vec![eval(
        "window.log=[];const f=document.querySelector('#field');for(const t of ['focus','focusin','blur','focusout'])f.addEventListener(t,()=>{log.push(t);queueMicrotask(()=>log.push('after '+t))});null",
    )];
    for (action, value) in
        [("focus", None), ("fill", Some("a")), ("press", Some("Shift")), ("type", Some("b")), ("click", None)]
    {
        steps.extend([unfocus.clone(), clear.clone(), act(action, value), read(action)]);
    }
    steps.extend([clear, act("blur", None), read("blur")]);
    let r = h.page("Pyro.run", json!({"timeout": 1000, "steps": steps}));
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    for action in ["focus", "fill", "press", "type"] {
        assert_eq!(reads[action], "focus,focusin,after focus,after focusin", "{action}: {r}");
    }
    assert_eq!(reads["blur"], "blur,focusout,after blur,after focusout", "{r}");
    assert_eq!(reads["click"], "focus,after focus,focusin,after focusin", "{r}");
}

/// A page that renders in a microtask and replaces the control on focus
/// (React): every `focusin` listener of the driver's focus sees the control
/// connected, the replacement happens after them, and the fill lands in the
/// replacement.
#[test]
fn fill_focus_listeners_see_the_control_before_a_microtask_replaces_it() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><input id=search>");
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 1000, "steps": [
            {"eval": {"expression": "window.log=[];const first=document.querySelector('#search');document.body.addEventListener('focusin',e=>{if(e.target!==first)return;log.push('capture');queueMicrotask(()=>{log.push('render');const next=first.cloneNode(true);first.replaceWith(next);next.focus()})},true);document.addEventListener('focusin',e=>{if(e.target===first)log.push('bubble:'+e.target.isConnected)});null"}},
            {"act": {"locator": {"css": "#search"}, "action": "fill", "value": "Text"}},
            {"check": {"locator": {"css": "#search"}, "assert": "value", "expected": "Text"}},
            {"eval": {"expression": "log.join()+':'+(document.activeElement!==first&&document.activeElement.id)", "as": "log"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["log"], "capture,bubble:true,render:search", "{r}");
}

#[test]
fn native_click_waits_for_hover_triggered_render_updates_before_pressing() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<!doctype html><button id=target style='position:absolute;left:100px;top:900px;width:140px;height:30px'>Choose</button><output id=out></output>",
    );
    let r=h.page("Pyro.run",json!({"timeout":250,"steps":[
        {"eval":{"expression":"window.ready=false;let hovered=false;let frames=0;let warm=0;const b=document.querySelector('#target');b.addEventListener('mouseenter',()=>hovered=true,{once:true});b.addEventListener('click',()=>document.querySelector('#out').textContent=ready?'stable':'early');new Promise(resolve=>{const frame=()=>{if(hovered&&!ready&&++frames===2){b.style.top='200px';ready=true}requestAnimationFrame(frame);if(++warm===6)resolve(null)};requestAnimationFrame(frame)})"}},
        {"act":{"locator":{"css":"#target"},"action":"click"}},
        {"check":{"locator":{"css":"#out"},"assert":"text","expected":"stable"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
}

/// Within one task the layout samples a running animation once (as Chromium
/// samples its timelines once per frame): a box read twice 60 ms apart in
/// the same task is in the same place. Sampled at each query, a task asking
/// for geometry over and over re-synced the layout every 16 ms (Danex's
/// dev-mode targets ran for minutes over a large view).
#[test]
fn a_running_animation_is_sampled_once_per_task() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<!doctype html><style>@keyframes move{from{transform:translateX(0)}to{transform:translateX(1000px)}}#box{width:10px;height:10px;animation:move 1s linear infinite}</style><div id=box></div>",
    );
    let r = h.page("Pyro.run", json!({"timeout": 2000, "steps": [
        {"eval": {"expression": "new Promise(resolve => setTimeout(() => { const b = document.querySelector('#box'); const a = b.getBoundingClientRect().x; const end = performance.now() + 60; while (performance.now() < end) {} const c = b.getBoundingClientRect().x; requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(() => resolve([a, c, b.getBoundingClientRect().x]), 60))); }, 20))", "as": "xs"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    let xs = r["result"]["reads"]["xs"].as_array().cloned().unwrap_or_default();
    assert_eq!(xs.len(), 3, "{r}");
    assert_eq!(xs[0], xs[1], "same task, same sample: {r}");
    assert_ne!(xs[0], xs[2], "a later task samples again: {r}");
}

#[test]
fn click_waits_for_pending_resize_observer_popover_position() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        "<!doctype html><button id=open>Open</button><div id=popover style='position:absolute;left:100px;top:900px;width:200px;height:10px'></div><output id=out></output>",
    );
    let r=h.page("Pyro.run",json!({"timeout":250,"steps":[
        {"eval":{"expression":"window.ready=false;const p=document.querySelector('#popover');new Promise(resolve=>{let initial=true;new ResizeObserver(()=>{if(initial){initial=false;resolve(null)}else{p.style.top='200px';ready=true}}).observe(p);document.querySelector('#open').onclick=()=>{p.style.height='40px';p.innerHTML='<button id=target>Choose</button>';document.querySelector('#target').onclick=()=>document.querySelector('#out').textContent=ready?'stable':'early'}})"}},
        {"act":{"locator":{"css":"#open"},"action":"click"}},
        {"act":{"locator":{"css":"#target"},"action":"click"}},
        {"check":{"locator":{"css":"#out"},"assert":"text","expected":"stable"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
}

#[test]
fn driver_external_file_drop_preserves_bytes_names_and_acceptance() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<div id="accept" style="width:200px;height:100px">accept</div>
<div id="reject" style="width:200px;height:100px">reject</div><output id="out"></output>
<script>
window.events=[];window.rejected=0;
const accept=document.getElementById('accept'),reject=document.getElementById('reject'),out=document.getElementById('out');
for(const type of ['dragenter','dragover','drop']) accept.addEventListener(type, async event=>{
  events.push([type,event.dataTransfer.types.join(','),event.dataTransfer.files.length,event.dataTransfer.items[0].kind]);
  event.preventDefault();
  if(type==='drop') {
    const file=event.dataTransfer.files[0];
    const itemFile=event.dataTransfer.items[0].getAsFile();
    const bytes=Array.from(new Uint8Array(await file.arrayBuffer()));
    out.textContent=JSON.stringify({name:file.name,type:file.type,bytes,itemName:itemFile.name});
  }
});
reject.addEventListener('drop',()=>rejected++);
</script>"#,
    );
    let file = json!({"name":"invalid/drop-name.pdf","mimeType":"application/pdf","base64":"AAEC/w=="});
    let result=h.page("Pyro.run",json!({"steps":[
      {"act":{"locator":{"css":"#accept"},"action":"dropFiles","files":[file.clone()]}},
      {"check":{"locator":{"css":"#out"},"assert":"text","expected":"{\"name\":\"invalid/drop-name.pdf\",\"type\":\"application/pdf\",\"bytes\":[0,1,2,255],\"itemName\":\"invalid/drop-name.pdf\"}"}},
      {"act":{"locator":{"css":"#reject"},"action":"dropFiles","files":[file]}},
      {"eval":{"expression":"JSON.stringify({events,rejected})","as":"events"}}
    ]}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(
        result["result"]["reads"]["events"],
        r#"{"events":[["dragenter","Files",0,"file"],["dragover","Files",0,"file"],["drop","Files",1,"file"]],"rejected":0}"#,
        "{result}"
    );
}

#[test]
fn driver_download_count_is_read_only_and_cross_batch() {
    let address = driver_site();
    let mut h = Harness::new();
    let result = h.page("Pyro.run", json!({"steps":[
      {"goto":{"url":format!("http://{address}/")}},
      {"eval":{"expression":"document.body.innerHTML = `<a id=dl href='/export.csv' download='report.csv'>Export</a>`; null"}},
      {"downloadCount":{"as":"before"}},
      {"act":{"locator":{"css":"#dl"},"action":"click"}},
      {"request":{"path":"/export.csv","method":"GET","status":200}},
      {"downloadCount":{"as":"after"}},
      {"download":{"as":"file"}},
      {"downloadCount":{"as":"afterConsume"}}
    ]}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["before"], 0, "{result}");
    assert_eq!(result["result"]["reads"]["after"], 1, "{result}");
    assert_eq!(result["result"]["reads"]["afterConsume"], 1, "{result}");
    assert_eq!(result["result"]["reads"]["file"]["filename"], "report.csv", "{result}");
    let next = h.page("Pyro.run",json!({"steps":[{"downloadCount":{"as":"before"}},{"eval":{"expression":"document.body.textContent='preview'; null"}},{"downloadCount":{"as":"after"}}]}));
    assert!(next.get("error").is_none(), "{next}");
    assert_eq!(next["result"]["reads"]["before"], 1, "{next}");
    assert_eq!(next["result"]["reads"]["after"], 1, "{next}");
}

#[test]
fn driver_check_uses_visible_labels_for_clipped_native_controls() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
      .clip {position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);clip-path:inset(50%)}
      .clip input {position:absolute;left:4px;top:4px;width:13px;height:13px}
      label {display:block;width:150px;height:25px}
    </style><label id="wrapping"><span class="clip"><input id="wrapped" type="checkbox" checked></span>Wrapping label</label>
    <span class="clip"><input id="external" type="checkbox"></span><label id="forlabel" for="external" style="margin-top:1200px">External label</label>
    <label><span class="clip"><input id="radio" type="radio"></span>Radio label</label>
    <script>window.changes=[];for(const id of ['wrapped','external','radio'])document.getElementById(id).addEventListener('change',()=>changes.push(id));</script>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"timeout":500,"steps":[
          {"act":{"locator":{"css":"#wrapped"},"action":"uncheck"}},
          {"check":{"locator":{"css":"#wrapped"},"assert":"checked","expected":false}},
          {"act":{"locator":{"css":"#wrapped"},"action":"uncheck"}},
          {"act":{"locator":{"css":"#wrapped"},"action":"check"}},
          {"check":{"locator":{"css":"#wrapped"},"assert":"checked"}},
          {"act":{"locator":{"css":"#external"},"action":"check"}},
          {"check":{"locator":{"css":"#external"},"assert":"checked"}},
          {"act":{"locator":{"css":"#external"},"action":"check"}},
          {"act":{"locator":{"css":"#external"},"action":"uncheck"}},
          {"check":{"locator":{"css":"#external"},"assert":"checked","expected":false}},
          {"act":{"locator":{"css":"#radio"},"action":"check"}},
          {"act":{"locator":{"css":"#radio"},"action":"check"}},
          {"check":{"locator":{"css":"#radio"},"assert":"checked"}},
          {"eval":{"expression":"({changes,scrollY})","as":"proof"}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(
        result["result"]["reads"]["proof"]["changes"],
        json!(["wrapped", "wrapped", "external", "external", "radio"]),
        "{result}"
    );
    assert!(result["result"]["reads"]["proof"]["scrollY"].as_f64().unwrap() > 0.0, "{result}");
}

#[test]
fn driver_check_label_fallback_preserves_disabled_and_hit_target_guards() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
      .clip {position:absolute;width:1px;height:1px;overflow:hidden}
      .clip input,.clip [role=checkbox] {position:absolute;left:4px;top:4px;width:13px;height:13px}
      label {position:absolute;left:20px;width:150px;height:25px}
      .cover {position:absolute;left:10px;width:180px;height:30px;z-index:5}
    </style>
    <label style="top:20px"><span class="clip"><input id="disabled" type="checkbox" disabled></span>Disabled label</label>
    <label style="top:60px"><span class="clip"><input id="covered" type="checkbox"></span>Covered label</label><div class="cover" style="top:60px"></div>
    <label id="ariaLabel" style="top:100px"><span class="clip"><div id="aria" role="checkbox" aria-checked="false"></div></span>ARIA label</label>
    <input id="visible" type="checkbox" style="position:absolute;left:30px;top:150px;width:20px;height:20px"><label for="visible" style="left:300px;top:150px">Visible alternate label</label><div class="cover" style="top:150px"></div>
    <script>document.getElementById('ariaLabel').onclick=()=>document.getElementById('aria').setAttribute('aria-checked','true');</script>"#,
    );
    for (id, expected_error) in [
        ("disabled", "disabled"),
        ("covered", "intercepts"),
        ("aria", "outside the viewport"),
        ("visible", "intercepts"),
    ] {
        let result = h.page(
            "Pyro.run",
            json!({"timeout":100,"steps":[{"act":{"locator":{"css":format!("#{id}")},"action":"check"}}]}),
        );
        assert!(result["error"]["message"].as_str().unwrap_or_default().contains(expected_error), "{id}: {result}");
    }
    let result=h.page("Pyro.run",json!({"steps":[{"eval":{"expression":"({disabled:document.getElementById('disabled').checked,covered:document.getElementById('covered').checked,aria:document.getElementById('aria').getAttribute('aria-checked'),visible:document.getElementById('visible').checked})","as":"states"}}]}));
    assert_eq!(
        result["result"]["reads"]["states"],
        json!({"disabled":false,"covered":false,"aria":"false","visible":false}),
        "{result}"
    );
}

#[test]
fn driver_check_visible_native_control_remains_the_direct_pointer_target() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><input id="visible" type="checkbox" style="width:30px;height:30px"><label for="visible">Visible label</label><script>window.targets=[];document.addEventListener('click',e=>targets.push(e.target.id));</script>"#,
    );
    let result=h.page("Pyro.run",json!({"steps":[{"act":{"locator":{"css":"#visible"},"action":"check"}},{"act":{"locator":{"css":"#visible"},"action":"check"}},{"check":{"locator":{"css":"#visible"},"assert":"checked"}},{"eval":{"expression":"targets","as":"targets"}}]}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["targets"], json!(["visible"]), "{result}");
}

#[test]
fn driver_popup_context_log_captures_initial_navigation_before_switch() {
    let address = driver_site();
    let mut h = Harness::new();
    h.call(None, "Target.setAutoAttach", json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}));
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId":h.frame_id,"flatten":true}));
    h.session = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    let opened = h.page(
        "Pyro.run",
        json!({"steps":[
          {"goto":{"url":format!("http://{address}/form")}},
          {"act":{"locator":{"css":"#name"},"action":"fill","value":"Parent"}},
          {"act":{"locator":{"css":"#go"},"action":"click"}},
          {"request":{"method":"GET","path":"/api","status":200,"count":1}},
          {"eval":{"expression":"window.open('/eager-popup','_blank','noopener'); null"}},
          {"popup":{"as":"popup"}}
        ]}),
    );
    assert!(opened.get("error").is_none(), "{opened}");
    let popup_id = opened["result"]["reads"]["popup"]["targetId"].as_str().unwrap().to_owned();
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId":popup_id,"flatten":true}));
    let popup_session = attached["result"]["sessionId"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let ready = h.call(
            Some(&popup_session),
            "Runtime.evaluate",
            json!({"expression":"document.body?.dataset.ready === 'yes'","returnByValue":true}),
        );
        if ready["result"]["result"]["value"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "popup bootstrap did not finish: {ready}");
        let host = h.browser.host_mut();
        host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        host.flush();
    }
    let observed = h.page(
        "Pyro.run",
        json!({"timeout":100,"steps":[
          {"page":{"target":popup_id}},
          {"loaded":{}},
          {"check":{"locator":{"css":"#ready"},"assert":"text","expected":"Hello, PopupStartup"}},
          {"request":{"method":"GET","path":"/api","scope":"context","status":200,"count":1}},
          {"request":{"method":"GET","path":"/eager-popup","scope":"context","status":200,"count":1}},
          {"request":{"method":"GET","path":"/api","count":0}},
          {"page":{"target":"main"}},
          {"request":{"method":"GET","path":"/api","scope":"context","status":200,"count":1}},
          {"check":{"locator":{"css":"#out"},"assert":"text","expected":"Hello, Parent"}}
        ]}),
    );
    assert!(observed.get("error").is_none(), "{observed}");
    assert_eq!(
        observed["result"]["requests"],
        json!([]),
        "current batch reply must remain scoped to its opener: {observed}"
    );
}

#[test]
fn driver_check_reveals_a_row_occluded_by_its_sticky_header() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
      #scroll { width:120px; height:100px; overflow:auto }
      #header { position:sticky; top:0; height:24px; background:white; z-index:2 }
      .row { height:24px } input { display:block; width:24px; height:24px; margin:0 }
    </style><div id=scroll><div id=header><input id=all type=checkbox></div>
      <div class=row><input id=row type=checkbox></div><div class=row><input id=middle type=checkbox></div><div class=row></div><div class=row></div>
    </div><script>document.getElementById('scroll').scrollTop=20;</script>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"timeout":300,"steps":[
          {"act":{"locator":{"css":"#middle"},"action":"check"}},
          {"eval":{"expression":"document.getElementById('scroll').scrollTop","as":"initialTop"}},
          {"act":{"locator":{"css":"#row"},"action":"check"}},
          {"check":{"locator":{"css":"#row"},"assert":"checked"}},
          {"check":{"locator":{"css":"#all"},"assert":"checked","expected":false}},
          {"eval":{"expression":"document.getElementById('scroll').scrollTop","as":"top"}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["initialTop"], 20, "{result}");
    assert_eq!(result["result"]["reads"]["top"], 0, "{result}");
}

#[test]
fn driver_centered_retry_still_rejects_a_covering_overlay() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
      #scroll {width:120px;height:100px;overflow:auto} .row{height:24px}
      input{display:block;width:24px;height:24px;margin:0}
      #cover{position:fixed;inset:0;z-index:100;background:white}
    </style><div id=scroll><div class=row></div><div class=row><input id=row type=checkbox></div><div class=row></div><div class=row></div><div class=row></div></div><div id=cover></div>
    <script>document.getElementById('scroll').scrollTop=20;window.clicks=0;document.addEventListener('click',()=>clicks++);</script>"#,
    );
    let result = h.page("Pyro.act", json!({"locator":{"css":"#row"},"action":"check","timeout":60}));
    assert!(result["error"]["message"].as_str().unwrap_or_default().contains("intercepts pointer events"), "{result}");
    let proof = h.page(
        "Runtime.evaluate",
        json!({"expression":"[document.getElementById('row').checked,clicks]","returnByValue":true}),
    );
    assert_eq!(proof["result"]["result"]["value"], json!([false, 0]), "{proof}");
}

#[test]
fn driver_explicit_clock_advances_dates_and_timers_without_automatic_time() {
    let mut h = Harness::new();
    load_html(&mut h, "<p id=out>waiting</p>");
    let r = h.page("Pyro.run", json!({"steps":[
        {"eval":{"expression":"window.clockBefore=Date.now();window.perfBefore=performance.now();setTimeout(()=>document.querySelector('#out').textContent='due',120000);null"}},
        {"advanceTime":{"ms":120001}},
        {"eval":{"expression":"({date:Date.now()-clockBefore,performance:performance.now()-perfBefore,state:document.querySelector('#out').textContent})","as":"elapsed"}}
    ]}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["elapsed"]["state"], "due", "timers must run before the next step: {r}");
    for key in ["date", "performance"] {
        let elapsed = r["result"]["reads"]["elapsed"][key].as_f64().unwrap();
        assert!(elapsed >= 120001.0 && elapsed < 121001.0, "{key}: {r}");
    }
    assert!(!pyro_browser::clock::virtual_time(), "explicit advance must not enable automatic jumps");
}

#[test]
fn viewport_step_updates_resize_observer_table_measurements() {
    let mut h = Harness::new();
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1920,"height":900,"deviceScaleFactor":1,"mobile":false}),
    );
    load_html(
        &mut h,
        r#"<!doctype html><style>body{margin:0}#container{margin-left:222px}table{table-layout:fixed}th{width:120px;min-width:120px}</style><div id=container><table id=table><tr><th>A</th><th>B</th><th>C</th><th>D</th><th>E</th><th>F</th></tr></table></div><script>const container=document.getElementById('container'),table=document.getElementById('table');window.events=[];function update(){table.style.width=container.clientWidth+'px';events.push(container.clientWidth)}update();new ResizeObserver(update).observe(container);</script>"#,
    );
    let result=h.page("Pyro.run",json!({"steps":[
      {"viewport":{"width":1440,"height":900}},
      {"eval":{"expression":"({width:innerWidth,container:container.clientWidth,inline:table.style.width,events})","as":"m"}}
    ]}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["m"]["width"], 1440, "{result}");
    assert_eq!(result["result"]["reads"]["m"]["container"], 1218, "{result}");
    assert_eq!(result["result"]["reads"]["m"]["inline"], "1218px", "{result}");
}

#[test]
fn driver_checks_transparent_z_index_input_above_later_visual_sibling() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>label{position:relative;display:inline-flex;width:24px;height:24px}input{position:absolute;inset:0;z-index:10;margin:0;width:100%;height:100%;opacity:0}.visual{position:relative;width:16px;height:16px;background:white}</style><label><input id=native type=checkbox><div id=visual class=visual></div></label><script>window.clicks=[];document.addEventListener('click',e=>clicks.push(e.target.id))</script>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"timeout":300,"steps":[
          {"act":{"locator":{"css":"#native"},"action":"check"}},
          {"check":{"locator":{"css":"#native"},"assert":"checked"}},
          {"eval":{"expression":"clicks","as":"targets"}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["targets"], json!(["native"]), "{result}");
}

#[test]
fn driver_checkbox_cannot_escape_an_ancestor_stacking_context() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>#low,#high{position:absolute;left:0;top:0;width:100px;height:100px}#low{z-index:1}#high{z-index:2;background:white}input{position:absolute;left:0;top:0;width:24px;height:24px;z-index:9999;margin:0;opacity:0}</style><div id=low><input id=native type=checkbox></div><div id=high></div><script>window.clicks=0;document.addEventListener('click',()=>clicks++)</script>"#,
    );
    let result = h.page("Pyro.act", json!({"locator":{"css":"#native"},"action":"check","timeout":60}));
    assert!(result["error"]["message"].as_str().unwrap_or_default().contains("intercepts pointer events"), "{result}");
    let proof=h.page("Runtime.evaluate",json!({"expression":"[document.getElementById('native').checked,clicks,document.elementFromPoint(12,12).id]","returnByValue":true}));
    assert_eq!(proof["result"]["result"]["value"], json!([false, 0, "high"]), "{proof}");
}

#[test]
fn driver_click_respects_modal_context_above_a_later_backdrop() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>#modal,#backdrop{position:fixed;inset:0}#modal{z-index:50;pointer-events:none}#dialog{position:relative;width:100%;height:100%;pointer-events:auto}#empty{width:100%;height:80vh;overflow:scroll}#backdrop{z-index:40;background:white}button{position:absolute;left:8px;top:8px;width:60px;height:24px}</style><div id=modal><div id=dialog><button id=close>Close</button><div id=empty>No Cargo</div></div></div><div id=backdrop></div><script>window.didClose=false;document.getElementById('close').onclick=()=>didClose=true;</script>"#,
    );
    let result = h.page(
        "Pyro.run",
        json!({"timeout":300,"steps":[
          {"act":{"locator":{"css":"#close"},"action":"click"}},
          {"eval":{"expression":"didClose","as":"closed"}}
        ]}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["result"]["reads"]["closed"], true, "{result}");
    h.page(
        "Runtime.evaluate",
        json!({"expression":"didClose=false;document.getElementById('backdrop').style.zIndex='60';"}),
    );
    let blocked = h.page("Pyro.act", json!({"locator":{"css":"#close"},"action":"click","timeout":60}));
    assert!(
        blocked["error"]["message"].as_str().unwrap_or_default().contains("intercepts pointer events"),
        "{blocked}"
    );
    let proof = h.page("Runtime.evaluate", json!({"expression":"didClose","returnByValue":true}));
    assert_eq!(proof["result"]["result"]["value"], false, "{proof}");
}

#[test]
fn sticky_direct_child_stays_in_scroll_content_beyond_one_scrollport() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>body{margin:0}#scroller{width:300px;height:200px;overflow:auto}#toolbar{position:sticky;top:0;z-index:20;height:40px;background:white}#target{margin:0 0 0 200px;width:60px;height:30px}#content{height:800px}#field{position:relative}textarea{box-sizing:border-box;width:300px;height:100px}</style><div id=scroller><div id=toolbar><button id=target>Delete</button></div><div id=content><div style=height:160px></div><div id=field><textarea id=under></textarea></div></div></div>"#,
    );
    assert_eq!(eval_value(&mut h, "(() => { document.getElementById('scroller').scrollTop=200; const target=document.getElementById('target'); const r=target.getBoundingClientRect(); return [r.y,document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)===target]; })()"), json!([0,true]), "Chromium keeps the direct sticky child at the scrollport top and hit-testable");
}

#[test]
fn sticky_scrollport_respects_padding_and_border() {
    let mut h = Harness::new();
    for nested in [false, true] {
        let wrapper = if nested { "<section>" } else { "" };
        let end = if nested { "</section>" } else { "" };
        let html=format!("<!doctype html><style>body{{margin:0}}#sc{{width:300px;height:200px;overflow:auto;padding:20px;border:10px solid}}#bar{{position:sticky;top:0;height:40px}}</style><div id=sc>{wrapper}<div id=bar>Toolbar</div><div style=height:800px></div>{end}</div>");
        load_html(&mut h, &html);
        for scroll in [0, 200, 640] {
            assert_eq!(eval_value(&mut h, &format!("document.getElementById('sc').scrollTop={scroll};document.getElementById('bar').getBoundingClientRect().y")), json!(30), "nested={nested}, scroll={scroll}");
        }
    }
}

#[test]
fn sticky_footer_respects_scrollport_padding() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><style>body{margin:0}#sc{width:300px;height:200px;overflow:auto;padding:20px;border:10px solid}#bar{position:sticky;bottom:0;height:40px}</style><div id=sc><section><div style=height:800px></div><div id=bar>Footer</div></section></div>");
    for scroll in [0, 200, 640] {
        assert_eq!(eval_value(&mut h, &format!("document.getElementById('sc').scrollTop={scroll};document.getElementById('bar').getBoundingClientRect().y")), json!(190), "scroll={scroll}");
    }
}

const DRIVER_TRANSIENT_POINTER_POPUP: &str = r#"<!doctype html><style>
  html{overflow:hidden}body{margin:0;height:951px}
  #list{box-sizing:border-box;position:absolute;left:460.65625px;top:526px;width:342.65625px;height:51px;border:1px solid;padding:4px;overflow-x:hidden;overflow-y:auto}
  #inner{position:relative;width:341px;height:41px}#option{width:341px;height:41px}
</style><input id=trigger style="position:absolute;left:460px;top:492px">
<div id=list role=listbox><div id=inner><div id=option role=option>Quote</div></div></div>
<script>
  window.popupState={reactive:false,restored:false,clicks:0,pressPhase:null,viewportScrolls:0};
  (()=>{
    const list=document.getElementById('list'),inner=document.getElementById('inner'),option=document.getElementById('option');
    // Searching a previously scrolled virtual list leaves an explicit zero offset.
    list.scrollLeft=1;list.scrollLeft=0;
    list.addEventListener('scroll',()=>{
      if(popupState.reactive&&list.scrollLeft>0){
        inner.style.pointerEvents='none';
        setTimeout(()=>{inner.style.pointerEvents='auto';popupState.restored=true},300);
      }
    });
    option.addEventListener('pointerdown',()=>popupState.pressPhase=popupState.restored?'restored':'early');
    option.addEventListener('click',()=>popupState.clicks++);
    document.addEventListener('scroll',event=>{
      if(event.target===document){popupState.viewportScrolls++;list.remove()}
    },true);
  })();
</script>"#;

fn transient_pointer_popup_harness() -> Harness {
    let mut h = Harness::new();
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":1440,"height":900,"deviceScaleFactor":1,"mobile":false}),
    );
    load_html(&mut h, DRIVER_TRANSIENT_POINTER_POPUP);
    h
}

#[test]
fn driver_transient_pointer_block_waits_without_centering_visible_popup() {
    let mut h = transient_pointer_popup_harness();
    eval_value(&mut h, "popupState.reactive=true");
    let result = h.page("Pyro.act", json!({"locator":{"css":"#option"},"action":"click","timeout":1000}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(
        eval_value(&mut h, "[popupState.clicks,popupState.restored,popupState.pressPhase,popupState.viewportScrolls,scrollY,document.getElementById('list').isConnected,document.getElementById('list').scrollLeft]"),
        json!([1,true,"restored",0,0,true,4]),
        "the horizontal reveal must wait for pointer recovery without dismissing the popup"
    );
}

#[test]
fn driver_permanent_pointer_block_does_not_click_or_center_visible_popup() {
    let mut h = transient_pointer_popup_harness();
    eval_value(&mut h, "document.getElementById('inner').style.pointerEvents='none'");
    let result = h.page("Pyro.act", json!({"locator":{"css":"#option"},"action":"click","timeout":80}));
    assert!(result["error"]["message"].as_str().unwrap_or_default().contains("intercepts pointer events"), "{result}");
    assert_eq!(
        eval_value(&mut h, "[popupState.clicks,popupState.viewportScrolls,scrollY,document.getElementById('list').isConnected,getComputedStyle(document.getElementById('option')).pointerEvents]"),
        json!([0,0,0,true,"none"]),
        "an unclickable target must stay blocked without moving the viewport"
    );
}

#[test]
fn driver_pointer_auto_child_remains_clickable_in_pointer_none_parent() {
    let mut h = transient_pointer_popup_harness();
    eval_value(&mut h, "document.getElementById('inner').style.pointerEvents='none';document.getElementById('option').style.pointerEvents='auto'");
    let result = h.page("Pyro.act", json!({"locator":{"css":"#option"},"action":"click","timeout":300}));
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(
        eval_value(
            &mut h,
            "[popupState.clicks,popupState.viewportScrolls,scrollY,document.getElementById('list').isConnected]"
        ),
        json!([1, 0, 0, true]),
        "the actual hit child overrides its parent's pointer block"
    );
}

fn driver_trace_child_mode(mode: &str) -> bool {
    std::env::var("PYRO_DRIVER_TRACE_TEST_CHILD").is_ok_and(|value| value == mode)
}

fn driver_trace_scenario(mode: &str) {
    let mut h = Harness::new();
    let mut run_id = h.next_id;
    if mode == "attempt-cap" {
        load_html(&mut h, "<!doctype html><input id=trace-private-selector class=waiting>");
        run_id = h.next_id;
        let session = h.session.clone();
        let pending = h.send(Some(&session), "Pyro.run", json!({"steps":[
            {"check":{"locator":{"css":"#trace-private-selector"},"assert":"attribute","property":"class","expected":"done"}}
        ]}));
        for tick in 0..40 {
            eval_value(
                &mut h,
                &format!("document.querySelector('#trace-private-selector').setAttribute('data-tick','{tick}');null"),
            );
        }
        h.send(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression":"document.querySelector('#trace-private-selector').setAttribute('class','done')"}),
        );
        let messages = h.until_response(pending);
        let reply = messages.iter().find(|message| message["id"] == pending).unwrap();
        assert!(reply.get("error").is_none(), "retry control must eventually pass");
    } else if mode == "error" {
        let result = h.page(
            "Pyro.run",
            json!({"steps":[{"eval":{"expression":"throw new Error('trace-private-error-message')"}}]}),
        );
        assert!(result.get("error").is_some(), "the error control must fail");
    } else if mode == "cancel" || mode == "turn-cancel" {
        let session = h.session.clone();
        // The turn follows a script that left the page something to run.
        let expression = if mode == "turn-cancel" {
            "setTimeout(() => {}); 'trace-private-turn-value'"
        } else {
            "new Promise(() => { const value = 'trace-private-pending-script'; })"
        };
        let id = h.send(Some(&session), "Pyro.run", json!({"steps":[{"eval":{"expression":expression}}]}));
        assert_eq!(id, run_id);
        assert!(h.drain().iter().all(|message| message["id"] != id), "pending work must not have replied");
        // Connection teardown uses this same public close_all path. It must
        // drop the waiter without fabricating successful completion.
        h.browser.host_mut().close_all();
    } else {
        let url = html_url(
            "<!doctype html><input id=trace-private-selector data-url='https://trace.invalid/trace-private-url'>",
        );
        let result = h.page("Pyro.run", json!({"steps":[
            {"goto":{"url":url}},
            {"eval":{"expression":"setTimeout(() => document.querySelector('#trace-private-selector').setAttribute('data-ready','yes'), 15); 'trace-private-script'"}},
            {"check":{"locator":{"css":"#trace-private-selector[data-ready]"},"assert":"count","expected":1}},
            {"act":{"locator":{"css":"#trace-private-selector"},"action":"fill","value":"trace-private-fill-value"}},
            {"check":{"locator":{"css":"#trace-private-selector"},"assert":"value","expected":"trace-private-fill-value"}},
            {"eval":{"expression":"new Promise(resolve => setTimeout(() => resolve('trace-private-promise-value'), 15))","as":"trace-private-read-key"}},
            {"read":{"locator":{"css":"#trace-private-selector"},"read":"value","as":"trace-private-result-key"}},
            {"eval":{"expression":"document.querySelector('#trace-private-selector').value + 'trace-private-read-only'"}},
            {"request":{"path":"/trace-private-request-url","count":0}}
        ]}));
        assert!(result.get("error").is_none(), "the complete control must pass: {result}");
        assert_eq!(result["result"]["reads"]["trace-private-result-key"], "trace-private-fill-value");
    }
    println!("pyro-run-trace-test-id {run_id}");
}

fn driver_trace_subprocess(test: &str, mode: &str, enabled: bool) -> (u32, std::process::Output) {
    use std::process::{Command, Stdio};
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("PYRO_DRIVER_TRACE_TEST_CHILD", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if enabled {
        command.env("PYRO_RUN_STEP_TRACE", "1");
    } else {
        command.env_remove("PYRO_RUN_STEP_TRACE");
    }
    let child = command.spawn().unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "isolated trace scenario failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (pid, output)
}

fn driver_trace_records(pid: u32, output: &std::process::Output) -> Vec<Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let run_id: u64 = stdout
        .split("pyro-run-trace-test-id ")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .unwrap()
        .parse()
        .unwrap();
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| {
            let record = line.strip_prefix("pyro-run-step ")?;
            assert!(
                !record.contains("trace-private") && !record.contains("https://"),
                "private data entered trace: {record}"
            );
            let value: Value = serde_json::from_str(record).unwrap();
            let mut keys: Vec<_> = value.as_object().unwrap().keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["elapsedMs", "event", "kind", "pid", "runId", "step"]);
            assert_eq!(value["pid"], pid);
            assert_eq!(value["runId"], run_id);
            assert!(value["elapsedMs"].as_f64().unwrap().is_finite());
            Some(value)
        })
        .collect()
}

fn driver_stage_records(pid: u32, output: &std::process::Output) -> Vec<Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let run_id: u64 = stdout
        .split("pyro-run-trace-test-id ")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .unwrap()
        .parse()
        .unwrap();
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| {
            let record = line.strip_prefix("pyro-run-stage ")?;
            assert!(
                !record.contains("trace-private") && !record.contains("https://"),
                "private data entered stage record"
            );
            let value: Value = serde_json::from_str(record).unwrap();
            let allowed = [
                "pid",
                "runId",
                "step",
                "stage",
                "event",
                "elapsedMs",
                "durationMs",
                "outcome",
                "attempt",
                "startedAttempts",
                "completedAttempts",
                "attemptMs",
                "recordedAttempts",
                "truncated",
                "pending",
            ];
            assert!(value.as_object().unwrap().keys().all(|key| allowed.contains(&key.as_str())));
            assert_eq!(value["pid"], pid);
            assert_eq!(value["runId"], run_id);
            assert!(["eval_api", "eval_checkpoint", "post_eval_turn", "settle", "check_attempt", "request_attempt"]
                .contains(&value["stage"].as_str().unwrap()));
            assert!(["start", "end", "skipped", "error", "summary", "truncated"]
                .contains(&value["event"].as_str().unwrap()));
            if let Some(outcome) = value.get("outcome") {
                assert!(["done", "pending", "retry", "retrySoon", "continue", "error", "quiet", "gaveUp"]
                    .contains(&outcome.as_str().unwrap()));
            }
            if let Some(duration) = value.get("durationMs") {
                assert!(duration.as_f64().unwrap() >= 0.0);
            }
            Some(value)
        })
        .collect()
}

#[test]
fn driver_step_trace_default_off_preserves_scenario() {
    if driver_trace_child_mode("off") {
        driver_trace_scenario("off");
        return;
    }
    let (pid, output) =
        driver_trace_subprocess("engine_cdp::driver_step_trace_default_off_preserves_scenario", "off", false);
    assert!(driver_trace_records(pid, &output).is_empty());
    assert!(driver_stage_records(pid, &output).is_empty());
}

#[test]
fn driver_step_trace_completion_is_private_and_pairs_after_waits() {
    if driver_trace_child_mode("complete") {
        driver_trace_scenario("complete");
        return;
    }
    let (pid, output) = driver_trace_subprocess(
        "engine_cdp::driver_step_trace_completion_is_private_and_pairs_after_waits",
        "complete",
        true,
    );
    let records = driver_trace_records(pid, &output);
    assert_eq!(records.len(), 18, "one start/end per step, including delayed checks and promises");
    let kinds = ["goto", "eval", "check", "act", "check", "eval", "read", "eval", "request"];
    let mut previous = 0.0;
    for (step, pair) in records.chunks_exact(2).enumerate() {
        assert_eq!(pair[0]["step"], step);
        assert_eq!(pair[1]["step"], step);
        assert_eq!(pair[0]["kind"], kinds[step]);
        assert_eq!(pair[1]["kind"], kinds[step]);
        assert_eq!(pair[0]["event"], "start");
        assert_eq!(pair[1]["event"], "end");
        for record in pair {
            let elapsed = record["elapsedMs"].as_f64().unwrap();
            assert!(elapsed >= previous, "timestamps must follow actual step/wait completion");
            previous = elapsed;
        }
    }
    let stages = driver_stage_records(pid, &output);
    for step in [1, 5] {
        let phases: Vec<_> = stages
            .iter()
            .filter(|r| r["step"] == step)
            .map(|r| (r["stage"].as_str().unwrap(), r["event"].as_str().unwrap()))
            .collect();
        assert_eq!(
            phases,
            [
                ("eval_api", "start"),
                ("eval_api", "end"),
                ("eval_checkpoint", "start"),
                ("eval_checkpoint", "end"),
                ("post_eval_turn", "start"),
                ("post_eval_turn", "end")
            ]
        );
    }
    // The script that only read left nothing for a turn to run.
    let read_only: Vec<_> = stages
        .iter()
        .filter(|r| r["step"] == 7 && r["stage"] == "post_eval_turn")
        .map(|r| r["event"].as_str().unwrap())
        .collect();
    assert_eq!(read_only, ["skipped"]);
    // The fill settled: one record, with how long it took.
    let settles: Vec<_> = stages.iter().filter(|r| r["stage"] == "settle").collect();
    assert_eq!(settles.len(), 1);
    assert_eq!((settles[0]["step"].as_u64(), settles[0]["event"].as_str()), (Some(3), Some("end")));
    assert_eq!(settles[0]["outcome"], "quiet");
    for (step, stage) in [(2, "check_attempt"), (4, "check_attempt"), (8, "request_attempt")] {
        let summary =
            stages.iter().find(|r| r["step"] == step && r["stage"] == stage && r["event"] == "summary").unwrap();
        assert!(summary["completedAttempts"].as_u64().unwrap() >= 1);
        assert_eq!(summary["startedAttempts"], summary["completedAttempts"]);
        assert!(summary["attemptMs"].as_f64().unwrap() >= 0.0);
    }
}

#[test]
fn driver_step_trace_errors_are_terminal_without_error_text() {
    if driver_trace_child_mode("error") {
        driver_trace_scenario("error");
        return;
    }
    let (pid, output) =
        driver_trace_subprocess("engine_cdp::driver_step_trace_errors_are_terminal_without_error_text", "error", true);
    let records = driver_trace_records(pid, &output);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["event"], "start");
    assert_eq!(records[1]["event"], "error");
    assert_eq!(records[1]["kind"], "eval");
    let stages = driver_stage_records(pid, &output);
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["stage"], "eval_api");
    assert_eq!(stages[0]["event"], "start");
    assert_eq!(stages[1]["event"], "error");
    assert_eq!(stages[1]["outcome"], "error");
}

#[test]
fn driver_step_trace_cancellation_leaves_started_work_unfinished() {
    for mode in ["cancel", "turn-cancel"] {
        if driver_trace_child_mode(mode) {
            driver_trace_scenario(mode);
            return;
        }
    }
    let (pid, output) = driver_trace_subprocess(
        "engine_cdp::driver_step_trace_cancellation_leaves_started_work_unfinished",
        "cancel",
        true,
    );
    let records = driver_trace_records(pid, &output);
    assert_eq!(records.len(), 1, "teardown must not fabricate end/error records");
    assert_eq!(records[0]["step"], 0);
    assert_eq!(records[0]["kind"], "eval");
    assert_eq!(records[0]["event"], "start");
    let stages = driver_stage_records(pid, &output);
    assert!(!stages.iter().any(|r| r["stage"] == "post_eval_turn"));
    let (pid, output) = driver_trace_subprocess(
        "engine_cdp::driver_step_trace_cancellation_leaves_started_work_unfinished",
        "turn-cancel",
        true,
    );
    assert_eq!(driver_trace_records(pid, &output).len(), 1);
    let turn: Vec<_> =
        driver_stage_records(pid, &output).into_iter().filter(|r| r["stage"] == "post_eval_turn").collect();
    assert_eq!(turn.len(), 1, "teardown must not fabricate an end for an interrupted turn");
    assert_eq!(turn[0]["event"], "start");
}

#[test]
fn driver_stage_trace_bounds_retries_and_keeps_exact_attempt_totals() {
    if driver_trace_child_mode("attempt-cap") {
        driver_trace_scenario("attempt-cap");
        return;
    }
    let (pid, output) = driver_trace_subprocess(
        "engine_cdp::driver_stage_trace_bounds_retries_and_keeps_exact_attempt_totals",
        "attempt-cap",
        true,
    );
    let steps = driver_trace_records(pid, &output);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1]["event"], "end");
    let stages = driver_stage_records(pid, &output);
    assert_eq!(stages.iter().filter(|r| r["event"] == "start").count(), 8);
    assert_eq!(stages.iter().filter(|r| r["event"] == "end").count(), 8);
    assert_eq!(stages.iter().filter(|r| r["event"] == "truncated").count(), 1);
    assert_eq!(stages.len(), 18, "bounded detail plus one exact terminal summary");
    let summary = stages.iter().find(|r| r["event"] == "summary").unwrap();
    assert!(summary["completedAttempts"].as_u64().unwrap() >= 40);
    assert_eq!(summary["startedAttempts"], summary["completedAttempts"]);
    assert_eq!(summary["recordedAttempts"], 8);
    assert_eq!(summary["truncated"], true);
    let detailed_ms: f64 =
        stages.iter().filter(|r| r["event"] == "end").map(|r| r["durationMs"].as_f64().unwrap()).sum();
    assert!(summary["attemptMs"].as_f64().unwrap() >= detailed_ms);
}

#[test]
fn dialogs_return_dismissed_results_and_are_reported() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let mut messages = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": "JSON.stringify([alert('hi'), confirm('sure?'), prompt('name', 'x')])", "returnByValue": true}),
    );
    messages.extend(h.run_for(50));
    let reply = messages.iter().find(|m| m.get("id").is_some()).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "[null,false,null]", "{reply}");
    let opened: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "Page.javascriptDialogOpening")
        .map(|m| (m["params"]["type"].clone(), m["params"]["message"].clone(), m["params"]["defaultPrompt"].clone()))
        .collect();
    assert_eq!(
        opened,
        [
            (json!("alert"), json!("hi"), json!("")),
            (json!("confirm"), json!("sure?"), json!("")),
            (json!("prompt"), json!("name"), json!("x")),
        ]
    );
    assert_eq!(messages.iter().filter(|m| m["method"] == "Page.javascriptDialogClosed").count(), 3);
    for _ in 0..3 {
        let r = h.page("Page.handleJavaScriptDialog", json!({"accept": false}));
        assert_eq!(r["result"], json!({}), "{r}");
    }
    let r = h.page("Page.handleJavaScriptDialog", json!({"accept": true}));
    assert_eq!(r["error"]["message"], "No dialog is showing", "{r}");
}

#[test]
fn history_and_target_info_report_the_document_title() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><title> Order   42 </title><p>x</p>");
    let r = h.page("Page.getNavigationHistory", json!({}));
    let index = r["result"]["currentIndex"].as_u64().unwrap() as usize;
    assert_eq!(r["result"]["entries"][index]["title"], "Order 42", "{r}");
    assert_eq!(r["result"]["entries"].as_array().unwrap().len(), 1, "the initial about:blank entry was replaced");
    let frame = h.frame_id.clone();
    let info = h.call(None, "Target.getTargetInfo", json!({"targetId": frame}));
    assert_eq!(info["result"]["targetInfo"]["title"], "Order 42", "{info}");
}

#[test]
fn network_offline_emulation_fails_requests_and_flips_on_line() {
    let (address, server) = serve_html(2, "<!doctype html><p>online</p>");
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
    h.until_event(|m| m["method"] == "Page.loadEventFired");
    h.page("Network.enable", json!({}));
    let r = h.page(
        "Network.emulateNetworkConditions",
        json!({"offline": true, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "navigator.onLine"), false);
    let url = format!("http://{address}/");
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": format!("fetch('{url}').then(() => 'ok', e => e.name)"), "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "TypeError", "{r}");
    h.page(
        "Network.emulateNetworkConditions",
        json!({"offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1}),
    );
    assert_eq!(eval_value(&mut h, "navigator.onLine"), true);
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": format!("fetch('{url}').then(r => r.text())"), "awaitPromise": true, "returnByValue": true}),
    );
    assert!(r["result"]["result"]["value"].as_str().unwrap_or_default().contains("online"), "{r}");
    server.join().unwrap();
}

#[test]
fn network_cookie_commands_use_the_page_context_jar() {
    let mut h = Harness::new();
    let r = h.page("Network.setCookie", json!({"name": "a", "value": "1", "url": "http://example.test/path"}));
    assert_eq!(r["result"]["success"], true, "{r}");
    let r = h.page(
        "Network.setCookies",
        json!({"cookies": [{"name": "b", "value": "2", "domain": ".example.test", "path": "/"}, {"name": "c", "value": "3", "url": "http://other.test/"}]}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let names = |h: &mut Harness, params: Value| {
        let r = h.page("Network.getCookies", params);
        let mut n: Vec<String> =
            r["result"]["cookies"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_owned()).collect();
        n.sort();
        n
    };
    assert_eq!(names(&mut h, json!({"urls": ["http://example.test/path/x"]})), ["a", "b"]);
    assert_eq!(names(&mut h, json!({"urls": ["http://other.test/"]})), ["c"]);
    assert!(names(&mut h, json!({})).is_empty(), "about:blank has no cookies");
    let r = h.page("Network.deleteCookies", json!({"name": "a", "url": "http://example.test/"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(names(&mut h, json!({"urls": ["http://example.test/path/x"]})), ["b"]);
    let r = h.page("Network.deleteCookies", json!({"name": "a"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    h.page("Network.clearBrowserCookies", json!({}));
    assert!(names(&mut h, json!({"urls": ["http://example.test/", "http://other.test/"]})).is_empty());
}

#[test]
fn network_get_request_post_data_returns_the_body() {
    let (address, server) = serve_html(1, "ok");
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    let url = format!("http://{address}/submit");
    let mut messages = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": format!("fetch('{url}', {{method: 'POST', body: JSON.stringify({{a: 1}})}}).then(r => r.text())"), "awaitPromise": true}),
    );
    messages.extend(h.run_for(50));
    let sent = messages
        .iter()
        .find(|m| m["method"] == "Network.requestWillBeSent" && m["params"]["request"]["url"] == url)
        .expect("requestWillBeSent");
    let id = sent["params"]["requestId"].clone();
    let r = h.page("Network.getRequestPostData", json!({"requestId": id}));
    assert_eq!(r["result"]["postData"], r#"{"a":1}"#, "{r}");
    let r = h.page("Network.getRequestPostData", json!({"requestId": "nope"}));
    assert_eq!(r["error"]["message"], "No post data available for the request", "{r}");
    server.join().unwrap();
}

/// `Pyro.geometryStats` resets process-wide counters (layouts, layoutMs,
/// resolveMs, ...): tests that call it take this lock, so one test's reset
/// does not land between another's two calls.
#[cfg(feature = "render")]
static GEOMETRY_STATS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "render")]
#[test]
fn performance_get_metrics_reports_chrome_names() {
    let _stats = GEOMETRY_STATS.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><div><p>a</p><p>b</p></div>");
    h.page("Performance.enable", json!({}));
    eval_value(&mut h, "document.querySelector('p').getBoundingClientRect().width");
    let stats = h.page("Pyro.geometryStats", json!({}));
    let r = h.page("Performance.getMetrics", json!({}));
    let metrics: std::collections::HashMap<String, f64> = r["result"]["metrics"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .iter()
        .map(|m| (m["name"].as_str().unwrap().to_owned(), m["value"].as_f64().unwrap()))
        .collect();
    for name in [
        "Timestamp",
        "Documents",
        "Frames",
        "Nodes",
        "LayoutCount",
        "RecalcStyleCount",
        "LayoutDuration",
        "RecalcStyleDuration",
    ] {
        assert!(metrics.contains_key(name), "{name}: {r}");
    }
    assert!(metrics["Nodes"] >= 5.0 && metrics["Frames"] == 1.0, "{r}");
    assert!(metrics["LayoutCount"] >= 1.0, "geometryStats ({stats}) did not reset it: {r}");
}

#[cfg(feature = "render")]
#[test]
fn geometry_stats_time_layouts_once_requested() {
    let _stats = GEOMETRY_STATS.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><div><p>a</p><p>b</p></div>");
    // The first call turns the timers on (and resets the counters).
    h.page("Pyro.geometryStats", json!({}));
    eval_value(
        &mut h,
        "for (const w of [10, 20, 30]) { document.querySelector('p').style.width = w + 'px'; \
         document.querySelector('p').getBoundingClientRect(); } 0",
    );
    let r = h.page("Pyro.geometryStats", json!({}));
    let stats = &r["result"];
    assert!(stats["layouts"].as_u64().unwrap_or(0) >= 3, "{r}");
    assert!(stats["layoutMs"].as_f64().unwrap_or(0.0) > 0.0, "layout time reads zero: {r}");
    // The sync's split: settle may already have resolved (resolveMs then
    // reads ~0), so the three together are what must be non-zero.
    let sync_ms: f64 = ["mirrorMs", "settleMs", "resolveMs"].iter().map(|k| stats[k].as_f64().unwrap_or(0.0)).sum();
    assert!(sync_ms > 0.0, "sync time reads zero: {r}");
    assert!(stats["blitzCumulative"]["phasesMs"]["layout"].as_f64().unwrap_or(0.0) > 0.0, "{r}");
}

#[test]
fn fetch_response_stage_pauses_reads_and_overrides_responses() {
    let (address, server) = serve_html(5, "<!doctype html><p>payload</p>");
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Page.navigate", json!({"url": format!("http://{address}/")}));
    h.until_event(|m| m["method"] == "Page.loadEventFired");
    let session = h.session.clone();
    let r = h.page(
        "Fetch.enable",
        json!({"patterns": [{"urlPattern": "*/resp*", "requestStage": "Response"}, {"urlPattern": "*/req*", "requestStage": "Request"}]}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let fetch = |h: &mut Harness, path: &str| {
        h.send(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression": format!("fetch('/{path}').then(async r => r.status + ':' + r.headers.get('x-added') + ':' + (await r.text()))"), "awaitPromise": true, "returnByValue": true}),
        )
    };
    let paused = |h: &mut Harness| h.until_event(|m| m["method"] == "Fetch.requestPaused");

    // Response stage: status, headers and body, then a new status and header.
    let eval = fetch(&mut h, "resp1");
    let p = paused(&mut h);
    assert_eq!(p["params"]["responseStatusCode"], 200, "{p}");
    assert!(p["params"]["responseHeaders"].as_array().unwrap().iter().any(|h| h["name"] == "content-type"), "{p}");
    let id = p["params"]["requestId"].clone();
    let body = h.page("Fetch.getResponseBody", json!({"requestId": id}));
    assert_eq!(body["result"]["body"], "<!doctype html><p>payload</p>", "{body}");
    assert_eq!(body["result"]["base64Encoded"], false);
    let r = h.page("Fetch.continueRequest", json!({"requestId": id}));
    assert_eq!(r["error"]["code"], -32602, "continueRequest at the response stage: {r}");
    let r = h.page(
        "Fetch.continueResponse",
        json!({"requestId": id, "responseCode": 201, "responseHeaders": [{"name": "x-added", "value": "yes"}]}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let reply = h.until_response(eval).into_iter().find(|m| m["id"] == eval).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "201:yes:<!doctype html><p>payload</p>", "{reply}");

    // Response stage: fulfill replaces the response.
    let eval = fetch(&mut h, "resp2");
    let id = paused(&mut h)["params"]["requestId"].clone();
    let r = h.page(
        "Fetch.fulfillRequest",
        json!({"requestId": id, "responseCode": 200, "body": pyro_cdp::network_protocol::base64_encode(b"mocked")}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let reply = h.until_response(eval).into_iter().find(|m| m["id"] == eval).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "200:null:mocked", "{reply}");

    // Request stage, then the response through interceptResponse.
    let eval = fetch(&mut h, "req3");
    let p = paused(&mut h);
    assert!(p["params"].get("responseStatusCode").is_none(), "{p}");
    let id = p["params"]["requestId"].clone();
    let r = h.page("Fetch.getResponseBody", json!({"requestId": id}));
    assert!(r.get("error").is_some(), "no body at the request stage: {r}");
    assert_eq!(
        h.page("Fetch.continueRequest", json!({"requestId": id, "interceptResponse": true}))["result"],
        json!({})
    );
    let p = paused(&mut h);
    assert_eq!(p["params"]["requestId"], id, "the same interception id at the response stage");
    assert_eq!(p["params"]["responseStatusCode"], 200);
    assert_eq!(h.page("Fetch.failRequest", json!({"requestId": id, "errorReason": "Failed"}))["result"], json!({}));
    let reply = h.until_response(eval).into_iter().find(|m| m["id"] == eval).unwrap();
    assert!(reply["result"].get("exceptionDetails").is_some(), "failed at the response stage: {reply}");

    // Unmatched requests are not paused.
    let eval = fetch(&mut h, "other");
    let reply = h.until_response(eval).into_iter().find(|m| m["id"] == eval).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "200:null:<!doctype html><p>payload</p>", "{reply}");
    h.page("Fetch.disable", json!({}));
    server.join().unwrap();
}

#[test]
fn runtime_bindings_report_calls_and_survive_navigation() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let r = h.page("Runtime.addBinding", json!({"name": "sendToHost"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "typeof sendToHost"), "function", "defined in the live main world");
    assert_eq!(eval_value(&mut h, "Object.keys(window).includes('sendToHost')"), false, "not enumerable");
    let messages = h.page_with_events("Runtime.evaluate", json!({"expression": "sendToHost('hi'); 1"}));
    let called = messages.iter().find(|m| m["method"] == "Runtime.bindingCalled").expect("bindingCalled");
    assert_eq!(called["params"]["name"], "sendToHost");
    assert_eq!(called["params"]["payload"], "hi");
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": "try { sendToHost(1); 'ok' } catch (e) { e.constructor.name }", "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "TypeError", "one string only");

    // Kept across navigation, defined before init scripts run.
    h.page("Page.addScriptToEvaluateOnNewDocument", json!({"source": "window.early = typeof sendToHost"}));
    load_html(&mut h, "<!doctype html><p>next</p>");
    assert_eq!(eval_value(&mut h, "early"), "function");
    let messages = h.page_with_events("Runtime.evaluate", json!({"expression": "sendToHost('after'); 1"}));
    let called =
        messages.iter().find(|m| m["method"] == "Runtime.bindingCalled").expect("bindingCalled after navigation");
    assert_eq!(called["params"]["payload"], "after");

    // A named world only.
    h.page("Runtime.addBinding", json!({"name": "worldOnly", "executionContextName": "utility"}));
    assert_eq!(eval_value(&mut h, "typeof worldOnly"), "undefined", "not in the main world");
    let world = h.page("Page.createIsolatedWorld", json!({"frameId": h.frame_id.clone(), "worldName": "utility"}));
    let id = world["result"]["executionContextId"].clone();
    let r =
        h.page("Runtime.evaluate", json!({"expression": "typeof worldOnly", "contextId": id, "returnByValue": true}));
    assert_eq!(r["result"]["result"]["value"], "function", "{r}");

    let r = h.page("Runtime.removeBinding", json!({"name": "sendToHost"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "typeof sendToHost"), "undefined");
    load_html(&mut h, "<!doctype html><p>again</p>");
    assert_eq!(eval_value(&mut h, "typeof sendToHost"), "undefined", "not re-added on navigation");
}

#[test]
fn runtime_binding_event_carries_the_calling_context() {
    let mut h = Harness::new();
    let created = h.page_with_events("Runtime.enable", json!({}));
    let main = created
        .iter()
        .find(|m| {
            m["method"] == "Runtime.executionContextCreated" && m["params"]["context"]["auxData"]["isDefault"] == true
        })
        .map(|m| m["params"]["context"]["id"].clone())
        .expect("main context");
    h.page("Runtime.addBinding", json!({"name": "b"}));
    let messages = h.page_with_events("Runtime.evaluate", json!({"expression": "b('x'); 1"}));
    let called = messages.iter().find(|m| m["method"] == "Runtime.bindingCalled").expect("bindingCalled");
    assert_eq!(called["params"]["executionContextId"], main);
}

#[test]
fn dom_set_file_input_files_reads_paths_and_fires_input_and_change() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><input type=file id=f multiple><input id=text>");
    let dir = std::env::temp_dir().join(format!("pyro-set-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("notes.txt"), dir.join("data.json"));
    std::fs::write(&a, "hello").unwrap();
    std::fs::write(&b, "{}").unwrap();
    eval_value(
        &mut h,
        "window.f = document.getElementById('f'); window.seen = []; for (const t of ['input', 'change']) f.addEventListener(t, e => seen.push(e.type + ':' + e.bubbles)); 0",
    );
    let object = |h: &mut Harness, id: &str| {
        let r = h.page("Runtime.evaluate", json!({"expression": format!("document.getElementById('{id}')")}));
        r["result"]["result"]["objectId"].as_str().unwrap().to_owned()
    };
    let input = object(&mut h, "f");
    let r = h.page("DOM.setFileInputFiles", json!({"objectId": input, "files": [a, b]}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(
        eval_value(&mut h, "[...f.files].map(f => [f.name, f.size, f.type, f.lastModified > 1e12])"),
        json!([["notes.txt", 5, "text/plain", true], ["data.json", 2, "application/json", true]])
    );
    assert_eq!(eval_value(&mut h, "seen"), json!(["input:true", "change:true"]));
    let text = object(&mut h, "text");
    let r = h.page("DOM.setFileInputFiles", json!({"objectId": text, "files": [a]}));
    assert_eq!(r["error"]["message"], "Node is not a file input element", "{r}");
    let r = h.page("DOM.setFileInputFiles", json!({"objectId": input, "files": [dir.join("missing")]}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let r = h.page("DOM.setFileInputFiles", json!({"objectId": input, "files": []}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, "f.files.length"), 0, "no files clears the selection");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Review (perf/realms): every protocol path as the first entry of a deferred world ----

#[test]
fn review_each_protocol_path_can_be_the_first_entry_of_a_deferred_world() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    load_html(&mut h, "<!doctype html><p id=p>para</p><iframe srcdoc='<span>in</span>'></iframe>");
    let frame = h.frame_id.clone();
    let mut n = 0;
    fn make(h: &mut Harness, frame: &str, n: &mut u32) -> Value {
        *n += 1;
        let r = h.page("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": format!("fresh{n}")}));
        r["result"]["executionContextId"].clone()
    }
    macro_rules! fresh {
        ($h:expr) => {
            make($h, &frame, &mut n)
        };
    }
    // Runtime.callFunctionOn with an executionContextId.
    let id = fresh!(&mut h);
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function() { return document.getElementById('p').textContent + window.length }", "executionContextId": id, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "para1", "{r}");
    // Runtime.evaluate returning the global, then Runtime.getProperties on it.
    let id = fresh!(&mut h);
    let r = h.page("Runtime.evaluate", json!({"expression": "window", "contextId": id}));
    let object = r["result"]["result"]["objectId"].clone();
    assert!(object.is_string(), "{r}");
    let props = h.page("Runtime.getProperties", json!({"objectId": object, "ownProperties": true}));
    let names: Vec<&str> =
        props["result"]["result"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"document") && names.contains(&"Node") && names.contains(&"0"), "{names:?}");
    // DOM.resolveNode into a world that was never entered.
    let object = h.page("Runtime.evaluate", json!({"expression": "document.getElementById('p')"}))["result"]["result"]["objectId"].clone();
    let backend = h.page("DOM.describeNode", json!({"objectId": object}))["result"]["node"]["backendNodeId"].clone();
    let id = fresh!(&mut h);
    let resolved = h.page("DOM.resolveNode", json!({"backendNodeId": backend, "executionContextId": id}));
    let adopted = resolved["result"]["object"]["objectId"].clone();
    assert!(adopted.is_string(), "{resolved}");
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"objectId": adopted, "functionDeclaration": "function() { return this.textContent + (this.ownerDocument === document) }", "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "paratrue", "{r}");
    // Console messages carry the world's context id.
    let id = fresh!(&mut h);
    let events = h.page_with_events("Runtime.evaluate", json!({"expression": "console.log('from-world'); 1", "contextId": id}));
    let mut events = events;
    if !events.iter().any(|m| m["method"] == "Runtime.consoleAPICalled") {
        events.extend(h.run_for(100));
    }
    let console = events.iter().find(|m| m["method"] == "Runtime.consoleAPICalled").expect("consoleAPICalled");
    assert_eq!(console["params"]["executionContextId"], id, "{console}");
    // Runtime.addBinding naming a world that exists but was never entered.
    let id = fresh!(&mut h);
    let name = format!("fresh{n}");
    let r = h.page("Runtime.addBinding", json!({"name": "hostCall", "executionContextName": name}));
    assert_eq!(r["result"], json!({}), "{r}");
    let events = h.page_with_events("Runtime.evaluate", json!({"expression": "hostCall('w'); typeof hostCall", "contextId": id, "returnByValue": true}));
    let mut events = events;
    if !events.iter().any(|m| m["method"] == "Runtime.bindingCalled") {
        events.extend(h.run_for(100));
    }
    let called = events.iter().find(|m| m["method"] == "Runtime.bindingCalled").expect("bindingCalled");
    assert_eq!(called["params"]["executionContextId"], id, "{called}");
    assert_eq!(called["params"]["payload"], "w");
    assert_eq!(eval_value(&mut h, "typeof hostCall"), "undefined", "not in the main world");
    // A second session's Runtime.enable announces the deferred worlds without making them.
    let id = fresh!(&mut h);
    let second = h.attach_second_session();
    let events = {
        let sent = h.send(Some(&second), "Runtime.enable", json!({}));
        h.until_response(sent)
    };
    let announced: Vec<&Value> = events
        .iter()
        .filter(|m| m["method"] == "Runtime.executionContextCreated")
        .map(|m| &m["params"]["context"]["id"])
        .collect();
    assert!(announced.contains(&&id), "{events:#?}");
    let r = h.call(Some(&second), "Runtime.evaluate", json!({"expression": "location.href.startsWith('data:') || location.href", "contextId": id, "returnByValue": true}));
    assert!(r["result"].get("exceptionDetails").is_none(), "{r}");
    // Navigating away destroys never-entered worlds as it does entered ones.
    let id = fresh!(&mut h);
    let events = h.page_with_events("Page.navigate", json!({ "url": html_url("<p>next</p>") }));
    let mut events = events;
    events.extend(h.run_for(100));
    let r = h.page("Runtime.evaluate", json!({"expression": "1", "contextId": id}));
    assert!(r.get("error").is_some(), "the old document's world is gone: {r}");
}

#[test]
fn review_closing_a_target_while_its_deferred_world_waits() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    h.page("Page.addScriptToEvaluateOnNewDocument", json!({"source": "", "worldName": "__pw_utility"}));
    load_html(&mut h, "<!doctype html><p>x</p>");
    let frame = h.frame_id.clone();
    let id = h.page("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": "__pw_utility"}))["result"]["executionContextId"].clone();
    // A pending evaluation in the world (its first entry), then the close.
    let session = h.session.clone();
    let pending = h.send(Some(&session), "Runtime.evaluate", json!({"expression": "new Promise(() => {})", "awaitPromise": true, "contextId": id}));
    h.run_for(50);
    let target = h.frame_id.clone();
    let r = h.call(None, "Target.closeTarget", json!({"targetId": target}));
    assert_eq!(r["result"]["success"], true, "{r}");
    let seen = h.until_reply(pending, 1000);
    // Whether the closed session still answers is the base's behaviour (not
    // changed here); what matters is that nothing waits on the world.
    if let Some(reply) = seen.iter().find(|m| m["id"] == pending) {
        assert!(reply.get("error").is_some() || reply["result"].get("exceptionDetails").is_some(), "{reply}");
    }
    assert_eq!(h.pending_replies(), 0, "no reply is left waiting on the closed page's world");
}

/// Danex E2E (v0.0.40 check): a side sheet that slides in with a keyframe
/// animation a task after the click that opens it. A hover waits until the
/// target's box is stable over a frame before its first move (the page places
/// a marker where the hovered text is then, as Danex's dev-mode targets), and a
/// click whose target is still outside the viewport tries again on a timer: no
/// DOM change brings the sheet in. Before: the hover landed at the animation's
/// start (marker off-screen) and the click waited for a DOM change until its
/// deadline.
#[test]
fn driver_pointer_actions_wait_for_a_sheet_sliding_in() {
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
  #sheet{position:fixed;top:0;right:0;width:300px;height:100vh;background:#fff;transform:translateX(100%)}
  @keyframes slide{from{transform:translateX(100%)}to{transform:none}}
  #sheet.in{transform:none;animation:slide 300ms ease-out}
  #marker{position:fixed;width:12px;height:12px}
</style><button id=show>Open</button><div id=sheet><p style="margin:40px"><span id=who>Blake Baker</span></p><button id=ok style="margin:40px">Approve</button></div><i id=marker></i>
<script>
  window.log = [];
  document.getElementById('show').onclick = () => setTimeout(() => document.getElementById('sheet').classList.add('in'));
  document.getElementById('who').addEventListener('mouseover', () => { const r = document.getElementById('who').getBoundingClientRect(); marker.style.left = r.right + 'px'; marker.style.top = (r.top - 12) + 'px'; });
  for (const t of ['mousemove', 'click']) document.getElementById('ok').addEventListener(t, () => log.push(t));
</script>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 3000, "steps": [
            {"act": {"locator": {"css": "#show"}, "action": "click"}},
            {"act": {"locator": {"css": "#who"}, "action": "hover"}},
            {"eval": {"expression": "Math.round(marker.getBoundingClientRect().left) === Math.round(document.getElementById('who').getBoundingClientRect().right)", "as": "settled"}},
            {"eval": {"expression": "document.getElementById('sheet').classList.remove('in'); void document.body.offsetWidth; document.getElementById('show').click(); 1"}},
            {"act": {"locator": {"css": "#ok"}, "action": "click"}},
            {"eval": {"expression": "log.join(',')", "as": "log"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["settled"], json!(true), "{r}");
    assert_eq!(r["result"]["reads"]["log"], json!("mousemove,click"), "{r}");
}

#[test]
fn animation_frames_advance_moving_geometry_after_client_activity() {
    for scripted in [false, true] {
        let mut h = Harness::new();
        let animation = if scripted {
            "target.animate([{transform:'translateX(0px)'},{transform:'translateX(1000px)'}],
                {duration:10000,iterations:Infinity});"
        } else {
            "document.head.innerHTML = '<style>@keyframes move {from {transform:translateX(0px)} to {transform:translateX(1000px)}} #target {animation:move 10s linear infinite}</style>';"
        };
        let expression = format!("new Promise(resolve => {{
            document.body.innerHTML = '<div id=target>moving</div>';
            const target = document.getElementById('target');
            {animation}
            const xs = [];
            const step = () => {{
                xs.push(target.getBoundingClientRect().x);
                if (xs.length < 6) requestAnimationFrame(step);
                else resolve(xs);
            }};
            requestAnimationFrame(step);
        }})");
        let r = h.page("Runtime.evaluate",
            json!({"expression": expression, "awaitPromise": true, "returnByValue": true}));
        let xs = r["result"]["result"]["value"].as_array().expect("six frame positions");
        assert_eq!(xs.len(), 6, "{r}");
        assert!(xs.windows(2).all(|pair| pair[1].as_f64().unwrap() > pair[0].as_f64().unwrap()),
            "scripted={scripted}: {xs:?}");
    }
}

#[test]
fn scroll_into_view_and_actions_scroll_every_container_inside_a_scaled_layer() {
    // Danex's lab canvas: a scaled world in an overflow: hidden stage, a
    // scroller inside it. Chromium 147 (measured, 800x600): scrollIntoView()
    // scrolls the scroller 683 of its own px (its content moves 683 * 0.6
    // client px) and the window 142, leaving the target at the top; an
    // overflow: hidden stage scrolls too (760 for a box past its edge);
    // offsetWidth ignores the transform; click and hover scroll and hit.
    let mut h = Harness::new();
    h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width": 800, "height": 600, "deviceScaleFactor": 1, "mobile": false}),
    );
    let rows: String = (0..20).map(|_| "<div style=\"height:40px;border-bottom:1px solid\"></div>").collect();
    load_html(
        &mut h,
        &format!(
            r#"<!doctype html><body style="margin:0">
<div id=stage style="position:absolute;left:0;top:0;width:2000px;height:1500px;transform:scale(0.6);transform-origin:0 0">
<div id=sc style="position:absolute;left:100px;top:100px;width:400px;height:300px;overflow:auto">{rows}<button id=target style="height:40px">target</button><div style="height:123px"></div></div>
</div>
<div id=clip style="position:absolute;left:200px;top:0;width:500px;height:300px;overflow:hidden">
<div style="position:absolute;left:0;top:0;transform:translate(40px,30px) scale(0.5);transform-origin:0 0"><div style="position:absolute;left:1600px;top:0;width:400px;height:100px"><button id=far>far</button></div></div>
</div><output id=out></output>"#
        ),
    );
    let read = "[document.getElementById('sc').scrollTop, scrollY, Math.round(document.getElementById('target').getBoundingClientRect().top), document.getElementById('sc').offsetWidth, document.getElementById('sc').getBoundingClientRect().width]";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "document.getElementById('target').scrollIntoView(); null"}},
            {"eval": {"expression": read, "as": "after"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["after"], json!([683, 142, 0, 400, 240]), "{r}");
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 2000, "steps": [
            {"eval": {"expression": "document.getElementById('sc').scrollTop = 0; scrollTo(0, 0); for (const id of ['target', 'far']) document.getElementById(id).addEventListener('click', () => document.getElementById('out').textContent += id + ' '); null"}},
            {"act": {"locator": {"css": "#target"}, "action": "click"}},
            {"act": {"locator": {"css": "#far"}, "action": "click"}},
            {"check": {"locator": {"css": "#out"}, "assert": "text", "expected": "target far "}},
            {"eval": {"expression": "[document.getElementById('sc').scrollTop > 0, document.getElementById('clip').scrollLeft > 0]", "as": "scrolled"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["scrolled"], json!([true, true]), "{r}");
}

#[test]
fn computed_style_inside_display_none_follows_every_change() {
    // Styles resolved on demand inside a display: none subtree are cached
    // until the next sync; each kind of change still shows (Chromium: the
    // same reads).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>.on .item{color:rgb(1, 2, 3)} .menu:has(.picked) .item{background-color:rgb(4, 5, 6)}</style>
<div id=menu class=menu style="display:none"><span id=item class=item>a</span><span id=other>b</span></div>"#,
    );
    let read = "(() => { const s = getComputedStyle(document.getElementById('item')); return s.color + ' ' + s.backgroundColor + ' ' + s.width; })()";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": read, "as": "a"}},
            {"eval": {"expression": read, "as": "a2"}},
            {"eval": {"expression": "document.getElementById('menu').classList.add('on'); null"}},
            {"eval": {"expression": read, "as": "b"}},
            {"eval": {"expression": "document.getElementById('other').classList.add('picked'); null"}},
            {"eval": {"expression": read, "as": "c"}},
            {"eval": {"expression": "document.head.append(Object.assign(document.createElement('style'), {textContent: '#item{color:rgb(9, 9, 9)}'})); null"}},
            {"eval": {"expression": read, "as": "d"}},
            {"eval": {"expression": "document.getElementById('item').style.color = 'rgb(7, 7, 7)'; null"}},
            {"eval": {"expression": read, "as": "e"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["a"], reads["a2"], "{r}");
    assert_eq!(reads["a"], json!("rgb(0, 0, 0) rgba(0, 0, 0, 0) auto"), "{r}");
    assert_eq!(reads["b"], json!("rgb(1, 2, 3) rgba(0, 0, 0, 0) auto"), "{r}");
    assert_eq!(reads["c"], json!("rgb(1, 2, 3) rgb(4, 5, 6) auto"), "{r}");
    assert_eq!(reads["d"], json!("rgb(9, 9, 9) rgb(4, 5, 6) auto"), "{r}");
    assert_eq!(reads["e"], json!("rgb(7, 7, 7) rgb(4, 5, 6) auto"), "{r}");
}

#[test]
fn computed_width_and_height_inside_display_none_are_the_computed_values() {
    // Chromium 147 (measured): an element without a box reports its
    // computed width/height (`7px` from a :has() rule, `50%`, `3em` as
    // `48px`, `auto`), not `auto` for every declaration.
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>.menu:has(.picked) .a{width:7px} .b{width:50%;height:3em} .c{font-size:10px;width:calc(2em + 1px)}</style>
<div class=menu style="display:none"><i class=picked></i><span id=a class=a></span><span id=b class=b></span><span id=c class=c></span><span id=d style="width:3px"></span><span id=e></span></div>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "['a','b','c','d','e'].map(id => { const s = getComputedStyle(document.getElementById(id)); return s.width + ' ' + s.height; })", "as": "sizes"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(
        r["result"]["reads"]["sizes"],
        json!(["7px auto", "50% 48px", "21px auto", "3px auto", "auto auto"]),
        "{r}"
    );
}

#[test]
fn computed_style_inside_display_none_inherits_from_changed_ancestors() {
    // Styles inside a display: none subtree are resolved once per element and
    // inherited down from cached ancestors; a change to a hidden ancestor or a
    // :has() subject still shows on the next read (Chromium: the same reads).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>.warm{color:rgb(200, 0, 0)} section:has(.x) b{text-decoration-line:underline}</style>
<div style="display:none"><section id=s><p id=p><b id=b>t</b></p><i id=i></i></section></div>"#,
    );
    let read = "(() => { const s = getComputedStyle(document.getElementById('b')); return s.color + ' ' + s.textDecorationLine; })()";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "getComputedStyle(document.getElementById('p')).color", "as": "p"}},
            {"eval": {"expression": read, "as": "a"}},
            {"eval": {"expression": "document.getElementById('p').classList.add('warm'); null"}},
            {"eval": {"expression": read, "as": "b"}},
            {"eval": {"expression": "document.getElementById('i').className = 'x'; null"}},
            {"eval": {"expression": read, "as": "c"}},
            {"eval": {"expression": "document.getElementById('i').remove(); null"}},
            {"eval": {"expression": read, "as": "d"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["a"], json!("rgb(0, 0, 0) none"), "{r}");
    assert_eq!(reads["b"], json!("rgb(200, 0, 0) none"), "{r}");
    assert_eq!(reads["c"], json!("rgb(200, 0, 0) underline"), "{r}");
    assert_eq!(reads["d"], json!("rgb(200, 0, 0) none"), "{r}");
}

/// 1,000 write/read pairs on a hidden measuring probe (Danex's textWidth: set
/// font and text, read getBoundingClientRect) on a page with an animation
/// rule, sticky elements (one in seven) and `elements` elements, in ms.
fn probe_pairs_ms(h: &mut Harness, elements: usize) -> f64 {
    let cells: String = (0..elements).map(|i| format!("<span class=c{}>{i}</span>", i % 7)).collect();
    load_html(
        h,
        &format!(
            r#"<!doctype html><style>@keyframes spin{{to{{transform:rotate(1turn)}}}} .c3.on{{animation:spin 1s}} .c5{{position:sticky;top:0}}</style><div id=grid>{cells}</div>"#
        ),
    );
    let r = h.page(
        "Pyro.run",
        json!({"timeout": 120000, "steps": [{"eval": {"as": "ms", "expression": "(() => {
            const probe = document.createElement('span');
            probe.style.cssText = 'position:absolute;visibility:hidden;white-space:pre;left:-9999px;top:0';
            document.body.append(probe);
            probe.getBoundingClientRect();
            const t = performance.now(); let w = 0;
            for (let i = 0; i < 1000; i++) {
                probe.style.fontSize = i % 7 ? '13px' : '12px';
                probe.textContent = 'DX09-' + i + ' 129.951';
                w += probe.getBoundingClientRect().width;
            }
            return w > 0 ? performance.now() - t : -1;
        })()"}}]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    r["result"]["reads"]["ms"].as_f64().unwrap()
}

#[test]
fn measuring_probe_write_read_pairs_stay_cheap() {
    // Each pair used to query every animation rule over the whole document
    // (and walk it for counters), so its cost grew with the page. Compared
    // with a small page rather than against a fixed budget, so that machine
    // load cancels out.
    let mut h = Harness::new();
    probe_pairs_ms(&mut h, 30);
    let small = probe_pairs_ms(&mut h, 30);
    let big = probe_pairs_ms(&mut h, 3000);
    eprintln!("probe pairs: 30 elements {small} ms, 3,000 elements {big} ms");
    assert!(small > 0.0 && big > 0.0, "{small} {big}");
    // Debug build: 13.4x before the journal-driven candidates, 4.8x after (the rest is the layout each read
    // flushes, which still grows with the page).
    assert!(big < small * 8.0 + 100.0, "1,000 probe pairs: {big} ms on 3,000 elements vs {small} ms on 30");
}

#[test]
fn animations_start_after_text_only_changes_from_a_class_or_inline_style() {
    // The candidate cache skips the rule query across text and `style`
    // changes; a class or an inline animation added afterwards still starts.
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>@keyframes spin{to{transform:rotate(1turn)}} .on{animation:spin 1s}</style><p id=p>a</p><i id=i></i><b id=b></b>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "window.started = []; document.addEventListener('animationstart', e => started.push(e.target.id)); for (let k = 0; k < 50; k++) { p.textContent = 'x' + k; p.style.color = k % 2 ? 'red' : 'blue'; p.getBoundingClientRect(); } null"}},
            {"eval": {"expression": "i.className = 'on'; b.style.animation = 'spin 2s'; null"}},
            {"eval": {"expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(started.slice().sort()))))", "as": "started"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["started"], json!(["b", "i"]), "{r}");
}

#[test]
fn animation_events_reach_listeners_added_after_a_scroll_in_the_same_task() {
    // A scroll write reads the box's `scroll-behavior`: that read must not start the animation the
    // task just asked for and plan its events while nobody listens yet (they were dropped), but
    // leave it to the next frame, as without the scroll (WPT css-contain
    // `content-visibility-animation-and-scroll`).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>@keyframes fade{from{opacity:0}} .on{animation:fade 50ms}</style><body style="margin:0"><div id=t style="width:10px;height:10px"></div><div style="height:3000px"></div>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "window.seen = []; t.className = 'on'; window.scrollTo(0, 500); for (const type of ['animationstart', 'animationend']) t.addEventListener(type, e => seen.push(e.type)); scrollY", "as": "y"}},
            {"eval": {"expression": "new Promise(r => { const t0 = performance.now(); const poll = () => seen.length === 2 || performance.now() - t0 > 2000 ? r(seen.slice()) : requestAnimationFrame(poll); requestAnimationFrame(poll); })", "as": "seen"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["y"], json!(500), "{r}");
    assert_eq!(r["result"]["reads"]["seen"], json!(["animationstart", "animationend"]), "{r}");
}

#[test]
fn nothing_scrolls_into_view_in_content_visibility_hidden_contents() {
    // A geometry query lays out the contents `content-visibility: hidden` skips, but they stay
    // unreachable by scrollIntoView and fragment scrolls (Chromium; WPT css-contain
    // `content-visibility-056`, `-083`, `-084`); the hidden element itself is scrolled to.
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body style="margin:0"><div style="height:3000px"></div><div id=c style="content-visibility:hidden;width:150px;height:150px"><div id=t style="width:100px;height:100px"></div></div><div style="height:3000px"></div><div id=v style="height:10px"></div>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "t.getBoundingClientRect(); t.scrollIntoView(); scrollY", "as": "script"}},
            {"eval": {"expression": "location.hash = 't'; null"}},
            {"eval": {"expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(scrollY))))", "as": "fragment"}},
            {"eval": {"expression": "location.hash = 'v'; null"}},
            {"eval": {"expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(scrollY > 3000))))", "as": "control"}},
            {"eval": {"expression": "scrollTo(0, 0); c.scrollIntoView(); scrollY > 0", "as": "container"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["script"], json!(0), "{r}");
    assert_eq!(r["result"]["reads"]["fragment"], json!(0), "{r}");
    assert_eq!(r["result"]["reads"]["control"], json!(true), "a fragment scroll to a visible element moves: {r}");
    assert_eq!(r["result"]["reads"]["container"], json!(true), "{r}");
}

#[test]
fn custom_property_changes_reach_descendants_that_skip_the_cascade() {
    // A custom property set on a container: descendants that use no var() take
    // the new value without cascading again, yet read it, and var() users and
    // children that declare their own custom properties still compute from it
    // (Chromium: the same reads, before and after).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>.w{width:var(--w, 10px)} .own{--o: calc(var(--w, 0px) * 2)} .use-o{width:var(--o, 1px)}</style>
<div id=sc><p id=plain><span id=deep>x</span></p><div id=w class=w></div><div class=own><div id=o class=use-o></div></div><p id=after style="color:rgb(1, 2, 3)"><b id=b>b</b></p></div>"#,
    );
    let read = "(() => { const v = id => getComputedStyle(document.getElementById(id)); return [v('deep').getPropertyValue('--w'), v('w').width, v('o').width, v('b').color, v('b').getPropertyValue('--w')]; })()";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": read, "as": "before"}},
            {"eval": {"expression": "sc.style.setProperty('--w', '30px'); document.getElementById('w').getBoundingClientRect(); null"}},
            {"eval": {"expression": read, "as": "a"}},
            {"eval": {"expression": "sc.style.setProperty('--w', '40px'); null"}},
            {"eval": {"expression": read, "as": "b"}},
            {"eval": {"expression": "sc.style.removeProperty('--w'); null"}},
            {"eval": {"expression": read, "as": "c"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["before"], json!(["", "10px", "0px", "rgb(1, 2, 3)", ""]), "{r}");
    assert_eq!(reads["a"], json!(["30px", "30px", "60px", "rgb(1, 2, 3)", "30px"]), "{r}");
    assert_eq!(reads["b"], json!(["40px", "40px", "80px", "rgb(1, 2, 3)", "40px"]), "{r}");
    assert_eq!(reads["c"], json!(["", "10px", "0px", "rgb(1, 2, 3)", ""]), "{r}");
}

#[test]
fn a_resized_query_container_restyles_its_subtree_and_leaves_the_rest() {
    // Only a resized container's descendants restyle for its size: @container
    // rules and cq units follow, nested containers too, and elements outside it
    // keep their values (Chromium: the same reads).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
.c{container-type:inline-size} #a{width:300px} #b{width:300px}
.k{color:rgb(0, 0, 0)} @container (min-width: 400px){.k{color:rgb(0, 128, 0)}}
.u{width:50cqw;height:10px}
#inner{width:50%} @container (min-width: 300px){#deep{color:rgb(0, 0, 255)}}
</style>
<div id=a class=c><p id=ka class=k>a</p><div id=ua class=u></div><div id=inner class=c><span id=deep>d</span></div></div>
<div id=b class=c><p id=kb class=k>b</p><div id=ub class=u></div></div>"#,
    );
    let read = "(() => { const s = id => getComputedStyle(document.getElementById(id)); return [s('ka').color, document.getElementById('ua').getBoundingClientRect().width, s('deep').color, s('kb').color, document.getElementById('ub').getBoundingClientRect().width]; })()";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": read, "as": "before"}},
            {"eval": {"expression": "document.getElementById('a').style.width = '600px'; null"}},
            {"eval": {"expression": read, "as": "wide"}},
            {"eval": {"expression": "document.getElementById('a').style.width = '300px'; null"}},
            {"eval": {"expression": read, "as": "back"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["before"], json!(["rgb(0, 0, 0)", 150, "rgb(0, 0, 0)", "rgb(0, 0, 0)", 150]), "{r}");
    assert_eq!(reads["wide"], json!(["rgb(0, 128, 0)", 300, "rgb(0, 0, 255)", "rgb(0, 0, 0)", 150]), "{r}");
    assert_eq!(reads["back"], json!(["rgb(0, 0, 0)", 150, "rgb(0, 0, 0)", "rgb(0, 0, 0)", 150]), "{r}");
}

#[test]
fn styling_detached_clones_between_reads_keeps_layout_and_observers_right() {
    // modern-screenshot reads each element's computed style and writes it onto a detached clone.
    // Writes outside the document must not change what reads see, a clone inserted afterwards
    // must lay out with its styles, and two observers of one target both get their records
    // (Chromium: the same reads).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><body style="margin:0"><div id=src style="width:120px;height:30px;color:rgb(1, 2, 3)">a</div><div id=host></div>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "window.recs = []; for (const k of ['one', 'two']) new MutationObserver(l => recs.push(k + ':' + l.length)).observe(host, {childList: true}); null"}},
            {"eval": {"as": "copy", "expression": "(() => { const src = document.getElementById('src'); const clone = src.cloneNode(true); const out = []; for (let i = 0; i < 50; i++) { const cs = getComputedStyle(src); out.push(cs.width + ' ' + cs.color); clone.style.setProperty('width', (200 + i) + 'px'); clone.style.setProperty('color', cs.color); } window.clone = clone; return [out[0], out[49], src.getBoundingClientRect().width]; })()"}},
            {"eval": {"as": "inserted", "expression": "(() => { host.appendChild(clone); const r = clone.getBoundingClientRect(); return [r.width, getComputedStyle(clone).color]; })()"}},
            {"eval": {"as": "recs", "expression": "new Promise(r => setTimeout(() => r(recs.slice().sort()), 0))"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["copy"], json!(["120px rgb(1, 2, 3)", "120px rgb(1, 2, 3)", 120]), "{r}");
    assert_eq!(reads["inserted"], json!([249, "rgb(1, 2, 3)"]), "{r}");
    assert_eq!(reads["recs"], json!(["one:1", "two:1"]), "{r}");
}

#[test]
fn a_clone_given_an_animation_while_detached_animates_once_inserted() {
    // Changes outside the document count for nothing in the animation candidates (a screenshot
    // library copies `animation-*` onto its clones); inserting the clone still starts its
    // animation, and a <style> added while clones change still applies (Chromium: the same).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>@keyframes spin{to{transform:rotate(1turn)}}</style><p id=p>a</p><div id=host></div>"#,
    );
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": "window.started = []; document.addEventListener('animationstart', e => started.push(e.target.id)); const c = p.cloneNode(true); c.id = 'c'; for (let i = 0; i < 20; i++) { getComputedStyle(p).getPropertyValue('animation-name'); c.style.setProperty('animation', i % 2 ? 'spin 1s' : 'none'); } c.style.animation = 'spin 1s'; window.c = c; document.head.append(Object.assign(document.createElement('style'), {textContent: '#p{animation:spin 2s}'})); getComputedStyle(p).getPropertyValue('animation-name'); host.append(c); null"}},
            {"eval": {"expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r([started.slice().sort(), getComputedStyle(p).animationName, getComputedStyle(c).animationName]))))", "as": "got"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(r["result"]["reads"]["got"], json!([["c", "p"], "spin", "spin"]), "{r}");
}

#[test]
fn has_anchors_follow_inserted_subtrees_with_repeated_and_sibling_compounds() {
    // `:has()` insertion invalidation collapses dependencies of one selector that share a
    // compound; inserting and removing subtrees that match through nested and sibling
    // compounds still restyles the anchors (Chromium: the same reads).
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><style>
#one:has(.a .a .a){color:rgb(200, 0, 0)} #two:has(> .x + .x){color:rgb(0, 150, 0)} #three:has(.k){color:rgb(0, 0, 220)}
</style><div id=one><div class=a></div></div><div id=two><i class=x></i></div><div id=three></div>"#,
    );
    let read = "['one','two','three'].map(id => getComputedStyle(document.getElementById(id)).color)";
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"eval": {"expression": read, "as": "before"}},
            {"eval": {"expression": "window.t = document.createElement('div'); t.innerHTML = '<div class=a><div class=a><span class=k></span></div></div>' + '<p class=a></p>'.repeat(20); one.firstElementChild.appendChild(t); window.x = document.createElement('i'); x.className = 'x'; two.appendChild(x); three.appendChild(document.createElement('b')).className = 'k'; null"}},
            {"eval": {"expression": read, "as": "inserted"}},
            {"eval": {"expression": "t.remove(); x.remove(); three.firstElementChild.remove(); null"}},
            {"eval": {"expression": read, "as": "removed"}}
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let reads = &r["result"]["reads"];
    assert_eq!(reads["before"], json!(["rgb(0, 0, 0)", "rgb(0, 0, 0)", "rgb(0, 0, 0)"]), "{r}");
    assert_eq!(reads["inserted"], json!(["rgb(200, 0, 0)", "rgb(0, 150, 0)", "rgb(0, 0, 220)"]), "{r}");
    assert_eq!(reads["removed"], json!(["rgb(0, 0, 0)", "rgb(0, 0, 0)", "rgb(0, 0, 0)"]), "{r}");
}

/// A request still in flight when a `Pyro.run` replies is listed there
/// unfinished; the next run reports how it finished (here a mocked 500 the
/// page shows), once, marked `previousRun`, and the run after that does not
/// list it again.
#[test]
fn run_reports_requests_that_finish_after_it_ended() {
    let address = driver_api_site();
    let mut h = Harness::new();
    let r = h.page(
        "Pyro.run",
        json!({"steps": [
            {"goto": {"url": format!("http://{address}/")}},
            {"mock": {"url": "/late", "status": 500, "body": "down", "delay": 2500}},
            {"eval": {"expression": "fetch('/late').then(r => { const p = document.createElement('p'); p.id = 'late'; p.textContent = 'status ' + r.status; document.body.append(p) }); 1"}},
        ]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let late = |r: &Value| -> Vec<Value> {
        let requests = r["result"]["requests"].as_array().unwrap_or_else(|| panic!("{r}"));
        requests.iter().filter(|q| q["url"].as_str().is_some_and(|u| u.ends_with("/late"))).cloned().collect()
    };
    // Run 1 lists it unfinished, or not at all when the network thread
    // announced it only after the reply.
    let first = late(&r);
    assert!(first.len() <= 1, "{r}");
    for q in &first {
        assert_eq!(q["status"], Value::Null, "still in flight when run 1 replied: {r}");
        assert_eq!(q["error"], Value::Null, "{r}");
        assert!(q.get("previousRun").is_none(), "{r}");
    }

    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"check": {"locator": {"css": "#late"}, "assert": "text", "expected": "status 500"}}]}),
    );
    assert!(r.get("error").is_none(), "{r}");
    let second = late(&r);
    assert_eq!(second.len(), 1, "{r}");
    assert_eq!(second[0]["status"], 500, "{r}");
    assert_eq!(second[0]["previousRun"], true, "{r}");

    let r = h.page("Pyro.run", json!({"steps": [{"eval": {"expression": "1"}}]}));
    assert!(r.get("error").is_none(), "{r}");
    assert!(late(&r).is_empty(), "reported once: {r}");
}

/// DevTools evaluation is exempt from the page's Trusted Types eval/Function
/// checks, as Chrome exempts it from CSP: on a `require-trusted-types-for
/// 'script'` page the page's own `eval` is refused, while `Runtime.evaluate`,
/// `Runtime.callFunctionOn` and `Pyro.run` compile (a driver's `page.evaluate`
/// goes through them). Before, the driver's code was refused and the
/// Playwright WPT runner never read the results of the enforcing pages.
#[test]
fn devtools_evaluation_is_exempt_from_trusted_types() {
    if !pyro_js::compile_strings_hooks_available() {
        eprintln!("skipped: JavaScriptCore without patch 0035");
        return;
    }
    let mut h = Harness::new();
    load_html(
        &mut h,
        r#"<!doctype html><head><meta http-equiv="Content-Security-Policy" content="require-trusted-types-for 'script'"></head>
<body><script>try { eval("1"); window.pageEval = "ran"; } catch (e) { window.pageEval = e.name; }</script>"#,
    );
    let r = h.page("Runtime.evaluate", json!({"expression": "window.pageEval", "returnByValue": true}));
    assert_eq!(r["result"]["result"]["value"], "EvalError", "the page's own eval is refused: {r}");

    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": "eval('6 * 7') + new Function('return 1')()", "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], 43, "Runtime.evaluate: {r}");

    let global = h.page("Runtime.evaluate", json!({"expression": "globalThis"}));
    let global = global["result"]["result"]["objectId"].as_str().unwrap().to_owned();
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function() { return eval('2 + 2'); }", "objectId": global, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], 4, "Runtime.callFunctionOn: {r}");

    let r = h.page(
        "Pyro.run",
        json!({"steps": [{"eval": {"expression": "eval('5')", "as": "a"}},
            {"eval": {"expression": "(x) => new Function('return ' + x)()", "call": true, "arg": 7, "as": "b"}}]}),
    );
    assert_eq!(r["result"]["reads"], json!({"a": 5, "b": 7}), "Pyro.run: {r}");

    // The exemption ends with the call: page code still refuses afterwards.
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": "new Promise(r => setTimeout(() => { try { eval('1'); r('ran'); } catch (e) { r(e.name); } }, 0))",
            "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], "EvalError", "a timer the call scheduled is page code: {r}");
}

/// postMessage's "/" target is the poster's origin, compared as an origin, not as its serialization:
/// an opaque opener (a data: URL page) reaches its popup's initial about:blank (which inherits that
/// very opaque origin), but once the popup is a document of another opaque origin its "/" does not
/// reach the opener, though both serialize as "null"; "*" does.
#[test]
fn post_message_slash_matches_opaque_origins_by_identity() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><title>opener</title><script>window.got = []; onmessage = (e) => got.push(e.data);</script>");
    let opener = h.frame_id.clone();
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    // The initial about:blank popup shares the opener's opaque origin: "/" reaches it.
    assert_eq!(
        value(&mut h, "globalThis.w = window.open(); w.received = []; w.addEventListener('message', (e) => w.received.push(e.data)); w.postMessage('same', '/'); 1"),
        json!(1)
    );
    let attached = h.until_event(move |m| {
        m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != json!(opener)
    });
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    h.run_for(50);
    assert_eq!(value(&mut h, "w.received.join()"), json!("same"));
    // A document of another opaque origin: its "/" does not reach the opener, "*" does.
    let page = "data:text/html,<script>opener.postMessage('slash', '/'); opener.postMessage('star', '*')</script>";
    value(&mut h, &format!("w.location.href = {}; 1", json!(page)));
    h.call(Some(&popup), "Page.enable", json!({}));
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let events = h.until_response(id);
    let popup_session = popup.clone();
    if !events.iter().any(|m| m["method"] == "Page.loadEventFired" && m["sessionId"] == json!(popup_session)) {
        h.until_event(move |m| m["method"] == "Page.loadEventFired" && m["sessionId"] == json!(popup));
    }
    h.run_for(200);
    assert_eq!(value(&mut h, "got.join()"), json!("star"));
}

/// A closed page's document leaves the DOM store it shares with its popups (or opener): closing the
/// opener while its popup lives frees the opener's tree (Performance.getMetrics' `Nodes` counts the
/// shared store's live nodes), after a garbage collection as a removed iframe's is.
#[test]
fn closing_the_opener_frees_its_document_in_the_shared_store() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><script>document.write('<div><span>x</span></div>'.repeat(300))</script>");
    let opener = h.frame_id.clone();
    h.page("Runtime.evaluate", json!({"expression": "window.open(); 1"}));
    let attached = h.until_event(move |m| {
        m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != json!(opener)
    });
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    h.until_response(id);
    let nodes = |h: &mut Harness, popup: &str| {
        let r = h.call(Some(popup), "Performance.getMetrics", json!({}));
        r["result"]["metrics"].as_array().unwrap_or_else(|| panic!("{r}")).iter().find(|m| m["name"] == "Nodes").unwrap()["value"].as_f64().unwrap()
    };
    let before = nodes(&mut h, &popup);
    assert!(before > 600.0, "the opener's nodes are in the shared store: {before}");
    let target = h.frame_id.clone();
    h.call(None, "Target.closeTarget", json!({ "targetId": target }));
    h.run_for(200);
    h.call(Some(&popup), "Pyro.jscMemory", json!({"action": "collect"}));
    h.call(Some(&popup), "Runtime.evaluate", json!({"expression": "1"}));
    h.run_for(100);
    let after = nodes(&mut h, &popup);
    assert!(after < 100.0, "the closed opener's document is freed: before {before}, after {after}");
}

/// The DOM store a popup shares with its opener holds one queue of resource events: one page's
/// checkpoint drains its peers' events too and hands them to their pages. An opener adding a style
/// sheet to its same-origin popup gets the sheet's `load` event and the sheet applied there.
#[test]
fn an_opener_adding_a_style_sheet_to_its_popup_loads_it() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><p>opener</p>");
    let opener = h.frame_id.clone();
    h.page("Runtime.evaluate", json!({"expression": "globalThis.w = window.open(); 1"}));
    // A client resumes the popup (Playwright does), which makes it a page of its own.
    let attached = h.until_event(move |m| {
        m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != json!(opener)
    });
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    h.until_response(id);
    h.run_for(50);
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"new Promise((done) => {
            const link = w.document.createElement('link');
            link.rel = 'stylesheet';
            link.href = 'data:text/css,p{color:rgb(1,2,3)}';
            link.onload = () => done(['load', w.getComputedStyle(w.document.querySelector('p')).color]);
            link.onerror = () => done(['error']);
            w.document.body.innerHTML = '<p>popup</p>';
            w.document.head.append(link);
            setTimeout(() => done(['timeout']), 2000);
          })"#, "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!(["load", "rgb(1, 2, 3)"]), "{r}");
}

/// A server answering a few paths (a thread that stops once the test ends): `/` an empty page,
/// `/worklet.js` a redirect to `/final/worklet.js`, a module that imports `./dep.js` and checks its
/// own URL, and `/final/dep.js`, which registers a painter (`/dep.js`, what the wrong base resolves
/// to, is a 404).
fn serve_worklet_modules() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, ty, body, location) = match path.as_str() {
                "/worklet.js" => ("302 Found", "text/plain", String::new(), Some("/final/worklet.js")),
                // A wrong base fetches `/dep.js`, whose 404 fails the graph's load; a wrong
                // `import.meta.url` throws a `SyntaxError`, which reads as a link error and rejects
                // addModule() (worklet-realm reports never reach CDP, so a reported error would not fail).
                "/final/worklet.js" => (
                    "200 OK",
                    "text/javascript",
                    "import './dep.js'; if (!import.meta.url.endsWith('/final/worklet.js')) throw new SyntaxError('evaluated at ' + import.meta.url);".to_owned(),
                    None,
                ),
                "/final/dep.js" => ("200 OK", "text/javascript", "registerPaint('dep', class { paint() {} });".to_owned(), None),
                "/dep.js" => ("404 Not Found", "text/plain", "wrong base".to_owned(), None),
                _ => ("200 OK", "text/html", "<!doctype html><p>page</p>".to_owned(), None),
            };
            let location = location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nAccess-Control-Allow-Origin: *\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    address
}

/// CSS.paintWorklet.addModule() loads the module's static imports (as the page's module scripts load
/// theirs) and evaluates the module under its response URL, after redirects: `import.meta.url` and
/// relative imports follow it.
#[test]
fn paint_worklet_modules_load_their_imports_at_the_final_url() {
    let addr = serve_worklet_modules();
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let seen = h.page_with_events(
        "Runtime.evaluate",
        json!({"expression": r#"CSS.paintWorklet.addModule('/worklet.js').then(() => 'loaded', (e) => 'rejected: ' + e)"#, "awaitPromise": true, "returnByValue": true}),
    );
    let r = seen.iter().find(|m| m["id"].is_u64() && m.get("result").is_some()).cloned().expect("evaluate response");
    assert_eq!(r["result"]["result"]["value"], json!("loaded"), "{r}");
}

/// CSS.paintWorklet.addModule()'s credentials mode applies to the whole module graph: with "omit",
/// neither the module nor its static imports carry the page's cookies, even a module the page itself
/// imported with them (the worklet graph has a module map of its own).
#[test]
fn paint_worklet_imports_use_the_graph_credentials_mode() {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let served = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_owned();
            let cookie = text
                .lines()
                .find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: ")))
                .unwrap_or("-")
                .to_owned();
            let (ty, body) = match path.as_str() {
                "/m/root.js" => ("text/javascript", "import './dep.js'; import './shared.js';".to_owned()),
                "/m/dep.js" | "/m/shared.js" => ("text/javascript", "export {};".to_owned()),
                "/log" => ("text/plain", { let mut l = served.lock().unwrap().clone(); l.sort(); l.join(" ") }),
                _ => ("text/html", "<!doctype html><p>page</p>".to_owned()),
            };
            if path.starts_with("/m/") {
                served.lock().unwrap().push(format!("{path}:{cookie}"));
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"document.cookie = 'c=1'; import('/m/shared.js').then(() => CSS.paintWorklet.addModule('/m/root.js', { credentials: 'omit' })).then(() => fetch('/log')).then((r) => r.text(), (e) => 'rejected: ' + e)"#, "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(
        r["result"]["result"]["value"],
        json!("/m/dep.js:- /m/root.js:- /m/shared.js:- /m/shared.js:c=1"),
        "{r}"
    );
}

/// The paint worklet's module map is consulted before fetching: addModule() of a URL that already
/// loaded resolves without a new fetch (here the server fails every later request for it).
#[test]
fn paint_worklet_add_module_of_a_loaded_url_does_not_fetch_again() {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let fetched = Arc::new(AtomicUsize::new(0));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = fetched.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_owned();
            let (status, ty, body) = match path.as_str() {
                "/w.js" if count.fetch_add(1, Ordering::SeqCst) == 0 => ("200 OK", "text/javascript", "registerPaint('once', class { paint() {} });"),
                "/w.js" => ("500 Internal Server Error", "text/plain", "gone"),
                _ => ("200 OK", "text/html", "<!doctype html><p>page</p>"),
            };
            let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"(async () => {
            const add = () => CSS.paintWorklet.addModule('/w.js').then(() => 'loaded', (e) => e.name);
            const first = await add();
            const [second, third] = await Promise.all([add(), add()]);
            return [first, second, third].join();
          })()"#, "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(r["result"]["result"]["value"], json!("loaded,loaded,loaded"), "{r}");
    assert_eq!(fetched.load(Ordering::SeqCst), 1);
}

/// The paint worklet's module map holds every module of its graphs, imports included: after a graph
/// whose entry imports B loaded, addModule(B) resolves without fetching B again (here the server fails
/// every later request for it); an import that failed is fetched again when added on its own.
#[test]
fn paint_worklet_add_module_of_a_loaded_import_does_not_fetch_again() {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let fetched = Arc::new(AtomicUsize::new(0));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = fetched.clone();
    let failed_once = Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_owned();
            let (status, ty, body) = match path.as_str() {
                "/g/a.js" => ("200 OK", "text/javascript", "import './b.js';"),
                "/g/c-entry.js" => ("200 OK", "text/javascript", "import './c.js';"),
                // Fails the first time (in c-entry's graph), loads when added on its own.
                "/g/c.js" if !failed_once.swap(true, Ordering::SeqCst) => ("500 Internal Server Error", "text/plain", "busy"),
                "/g/c.js" => ("200 OK", "text/javascript", "registerPaint('c', class { paint() {} });"),
                "/g/b.js" if count.fetch_add(1, Ordering::SeqCst) == 0 => ("200 OK", "text/javascript", "registerPaint('b', class { paint() {} });"),
                "/g/b.js" => ("500 Internal Server Error", "text/plain", "gone"),
                _ => ("200 OK", "text/html", "<!doctype html><p>page</p>"),
            };
            let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"(async () => {
            const add = (u) => CSS.paintWorklet.addModule(u).then(() => 'loaded', (e) => e.name);
            const a = await add('/g/a.js');
            const b = await add('/g/b.js');
            const entry = await add('/g/c-entry.js');
            const c = await add('/g/c.js');
            return [a, b, entry === 'loaded' ? 'loaded' : 'rejected', c].join();
          })()"#, "awaitPromise": true, "returnByValue": true}),
    );
    // A module whose fetch failed in another graph leaves the map: added on its own, it is fetched.
    assert_eq!(r["result"]["result"]["value"], json!("loaded,loaded,rejected,loaded"), "{r}");
    assert_eq!(fetched.load(Ordering::SeqCst), 1);
}

/// Serves a worklet graph a.js -> b.js -> c.js where each request for c.js waits for the test's word
/// (true: serve it, false: fail it with a 500).
fn serve_gated_worklet_graph() -> (std::net::SocketAddr, std::sync::mpsc::Sender<bool>) {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let rx = Arc::new(Mutex::new(rx));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let rx = rx.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_owned();
                let (status, ty, body) = match path.as_str() {
                    "/s/a.js" => ("200 OK", "text/javascript", "import './b.js';"),
                    "/s/b.js" => ("200 OK", "text/javascript", "import './c.js';"),
                    "/s/c.js" => match rx.lock().unwrap().recv_timeout(Duration::from_secs(10)) {
                        Ok(true) => ("200 OK", "text/javascript", "registerPaint('c', class { paint() {} });"),
                        _ => ("500 Internal Server Error", "text/plain", "failed"),
                    },
                    _ => ("200 OK", "text/html", "<!doctype html><p>page</p>"),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            });
        }
    });
    (addr, tx)
}

/// addModule(B), where B is an import of a graph already loading or loaded (A -> B -> C), settles as
/// B's own graph completes (B and its imports fetched and evaluated, or failed), as HTML's "fetch a
/// worklet script graph" does, not when B's source alone has been fetched: added while C is still
/// loading it waits for C; when C fails (during A's load or before) it rejects.
#[test]
fn paint_worklet_add_module_of_an_import_settles_with_its_graph() {
    add_module_of_an_import_settles_with_its_graph();
}

/// The same with an engine that cannot load a registered module's graph (JavaScriptCore without
/// patch 0037; forced here): addModule(B) settles as the graph that imports B does, its rejection
/// included.
#[test]
fn paint_worklet_add_module_of_an_import_settles_with_its_graph_without_graph_phases() {
    pyro_webapi::test_hooks::force_graph_fallback_for_tests(true);
    add_module_of_an_import_settles_with_its_graph();
    pyro_webapi::test_hooks::force_graph_fallback_for_tests(false);
}

fn add_module_of_an_import_settles_with_its_graph() {
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    let open = |addr: std::net::SocketAddr| {
        let mut h = Harness::new();
        h.page("Page.enable", json!({}));
        let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
        if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
            h.until_event(|m| m["method"] == "Page.loadEventFired");
        }
        value(&mut h, "globalThis.st = {}; globalThis.add = (name, u) => { st[name] = 'pending'; CSS.paintWorklet.addModule(u).then(() => st[name] = 'loaded', (e) => st[name] = 'rejected'); }; 1");
        h
    };
    // Added while A's graph waits for C, which then loads: B waits for C.
    for c_loads in [true, false] {
        let (addr, c) = serve_gated_worklet_graph();
        let mut h = open(addr);
        value(&mut h, "add('a', '/s/a.js'); 1");
        h.run_for(300);
        value(&mut h, "add('b', '/s/b.js'); 1");
        h.run_for(300);
        assert_eq!(value(&mut h, "[st.a, st.b].join()"), json!("pending,pending"), "C not served yet (c_loads {c_loads})");
        for _ in 0..3 {
            c.send(c_loads).unwrap();
        }
        h.run_for(500);
        let want = if c_loads { "loaded,loaded" } else { "rejected,rejected" };
        assert_eq!(value(&mut h, "[st.a, st.b].join()"), json!(want), "c_loads {c_loads}");
    }
    // Added after A's graph failed on C: B's graph failed too.
    let (addr, c) = serve_gated_worklet_graph();
    let mut h = open(addr);
    for _ in 0..3 {
        c.send(false).unwrap();
    }
    value(&mut h, "add('a', '/s/a.js'); 1");
    h.run_for(500);
    assert_eq!(value(&mut h, "st.a"), json!("rejected"));
    value(&mut h, "add('b', '/s/b.js'); 1");
    h.run_for(500);
    assert_eq!(value(&mut h, "st.b"), json!("rejected"));
}

/// Serves `/e/` worklet modules for `add_module_settles_by_failure_kind`: modules whose evaluation
/// throws (the WPT security-features payload calls `postMessage`, which a worklet global lacks),
/// graphs that fail to load or link, and a module that loads; anything else under `/e/` is a 404.
fn serve_worklet_failure_kinds() -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let path = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_owned();
            let js = |body: &'static str| ("200 OK", "text/javascript", body);
            let (status, ty, body) = match path.as_str() {
                // common/security-features/subresource/template/worker.js.template.
                "/e/post-message.js" | "/e/post-message-data.js" => js("postMessage({ \"headers\": {} });"),
                "/e/imports-throwing.js" => js("import './throwing.js';"),
                "/e/throwing.js" => js("throw new Error('evaluation threw');"),
                "/e/imports-missing.js" => js("import './missing.js';"),
                "/e/syntax.js" => js("export let = ;"),
                "/e/link.js" => js("import { nope } from './empty.js';"),
                "/e/empty.js" => js("export {};"),
                "/e/ok.js" => js("registerPaint('ok', class { paint() {} });"),
                p if p.starts_with("/e/") => ("404 Not Found", "text/plain", "missing"),
                _ => ("200 OK", "text/html", "<!doctype html><p>page</p>"),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    addr
}

/// HTML's addModule() rejects only when "fetch a worklet script graph" fails: a failed fetch (an
/// import's included), a parse error or a link error (the script's error to rethrow). A module
/// whose evaluation throws ran: the error is reported and the promise fulfills (WPT
/// worklets/resources/import-tests.js "Importing a script which throws should still resolve the
/// given promise"; Chrome 158). The WPT content-security-policy/gen worklet-paint pages rely on it:
/// their module's `postMessage` throws a ReferenceError in the worklet global. This holds for an
/// entry, an entry imported from a `data:` module, an import that threw, and that import added later
/// on its own. A worklet that ran a throwing module still loads the next one.
#[test]
fn paint_worklet_add_module_rejects_only_when_the_graph_fails_to_load() {
    add_module_settles_by_failure_kind();
}

/// The same where addModule() of another graph's import waits for the graph that fetched it (forced:
/// see `force_graph_fallback_for_tests`).
#[test]
fn paint_worklet_add_module_rejects_only_when_the_graph_fails_to_load_without_graph_phases() {
    pyro_webapi::test_hooks::force_graph_fallback_for_tests(true);
    add_module_settles_by_failure_kind();
    pyro_webapi::test_hooks::force_graph_fallback_for_tests(false);
}

fn add_module_settles_by_failure_kind() {
    // Without the engine's two steps one promise covers load and evaluation (paint_worklet.rs).
    if !pyro_js::Scope::has_module_phases() {
        return;
    }
    let addr = serve_worklet_failure_kinds();
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": format!("http://{addr}/") }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": r#"(async () => {
            const add = (u) => CSS.paintWorklet.addModule(u).then((v) => v === undefined ? 'loaded' : 'value', (e) => 'rejected:' + e.name);
            const out = [];
            // Evaluation throws: fulfilled.
            out.push(await add('/e/post-message.js'));
            out.push(await add(`data:text/javascript,import '${location.origin}/e/post-message-data.js';`));
            out.push(await add('/e/imports-throwing.js'));
            out.push(await add('/e/throwing.js'));
            // The graph fails to load or link: rejected.
            out.push((await add('/e/imports-missing.js')).startsWith('rejected:') ? 'rejected' : 'settled');
            out.push(await add('/e/syntax.js'));
            out.push(await add('/e/link.js'));
            // Still loads modules.
            out.push(await add('/e/ok.js'));
            return out.join();
          })()"#, "awaitPromise": true, "returnByValue": true}),
    );
    assert_eq!(
        r["result"]["result"]["value"],
        json!("loaded,loaded,loaded,loaded,rejected,rejected:SyntaxError,rejected:SyntaxError,loaded"),
        "{r}"
    );
}

/// A `<frameset>` is the viewport's size wherever it is and whatever its author size (Chrome 158:
/// 800x513 in an 800x513 viewport, in `<body>`, in a 200px `<div>`, with `height: 50px`), so a click at
/// its centre reaches it (WPT fullscreen/crashtests/frameset-crash clicks a frameset in `<body>`).
#[test]
fn a_frameset_is_the_viewports_size() {
    let mut h = Harness::new();
    // (The parser drops a <frameset> in <body>: script makes them, as the WPT page does.)
    load_html(&mut h, "<!doctype html><body><div style='height:200px'></div><script>const a = document.createElement('frameset'); a.style.height = '50px'; document.querySelector('div').append(a); document.body.append(document.createElement('frameset'));</script>");
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    assert_eq!(
        value(&mut h, "[...document.querySelectorAll('frameset')].map((f) => { const r = f.getBoundingClientRect(); return [r.width === innerWidth, r.height === innerHeight, r.height > 0]; }).flat().join()"),
        json!("true,true,true,true,true,true")
    );
    // The second frameset's centre hits it.
    assert_eq!(
        value(&mut h, "(() => { const f = document.querySelectorAll('frameset')[1]; const r = f.getBoundingClientRect(); return document.elementFromPoint(r.x + r.width / 2, r.y + Math.min(r.height / 2, innerHeight - r.y - 1)) === f; })()"),
        json!(true)
    );
}

/// The opener's WindowProxy for a popup targets the browsing context, not a document: once the popup
/// navigated (here to a document of another origin), the opener's postMessage reaches the popup's
/// current document.
#[test]
fn post_message_reaches_a_navigated_popup() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><title>opener</title><script>window.got = []; onmessage = (e) => got.push(e.data);</script>");
    let opener = h.frame_id.clone();
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    value(&mut h, "globalThis.w = window.open(); 1");
    let attached = h.until_event(move |m| {
        m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != json!(opener)
    });
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    let page = "data:text/html,<script>onmessage = (e) => opener.postMessage('echo:' + e.data, '*')</script>";
    value(&mut h, &format!("w.location.href = {}; 1", json!(page)));
    h.call(Some(&popup), "Page.enable", json!({}));
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    let events = h.until_response(id);
    // Wait for the navigation to the data: document (the initial about:blank reports a load of its own).
    let session = popup.clone();
    let navigated = |m: &Value| {
        m["method"] == "Page.frameNavigated" && m["sessionId"] == json!(session) && m["params"]["frame"]["url"].as_str().is_some_and(|u| u.starts_with("data:"))
    };
    if !events.iter().any(navigated) {
        h.until_event(navigated);
    }
    h.run_for(100);
    value(&mut h, "w.postMessage('one', '*'); 1");
    h.run_for(200);
    assert_eq!(value(&mut h, "got.join()"), json!("echo:one"));
}

/// A popup opened by a sandboxed (opaque) child of an opaque top-level document has the child's
/// origin, not the top-level opener's: its later about:blank documents inherit that same origin, so
/// the child's property access and its postMessage("/") agree about the popup (both reach it). The
/// popup escapes the sandbox (`allow-popups-to-escape-sandbox`): one that takes the child's sandbox
/// (HTML "popup sandboxing flag set") gives its about:blank documents new opaque origins instead.
#[test]
fn a_popup_takes_the_origin_of_the_document_that_opened_it() {
    let mut h = Harness::new();
    let child = r#"<script>
let w;
onmessage = (e) => {
  const say = (v) => parent.postMessage(v, '*');
  if (e.data === 'open') { w = window.open(); say('opened'); }
  else if (e.data === 'blank') { w.location.href = 'about:blank'; say('navigating'); }
  else if (e.data === 'check') {
    let access;
    try { access = typeof w.document.body; w.got = []; w.onmessage = (m) => w.got.push(m.data); } catch (err) { access = err.name; }
    w.postMessage('slash', '/');
    say('access:' + access);
  } else if (e.data === 'got') {
    let got; try { got = w.got.join(); } catch (err) { got = err.name; }
    say('got:' + got);
  }
};
</script>"#;
    let top = format!(
        "<!doctype html><script>window.log = []; onmessage = (e) => log.push(e.data);</script><iframe sandbox='allow-scripts allow-popups allow-popups-to-escape-sandbox' srcdoc=\"{}\"></iframe>",
        child.replace('"', "&quot;")
    );
    load_html(&mut h, &top);
    let opener = h.frame_id.clone();
    let value = |h: &mut Harness, expression: &str| {
        let r = h.page("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }));
        r["result"]["result"]["value"].clone()
    };
    h.run_for(50);
    value(&mut h, "frames[0].postMessage('open', '*'); 1");
    let attached = h.until_event(move |m| {
        m["method"] == "Target.attachedToTarget" && m["params"]["targetInfo"]["targetId"] != json!(opener)
    });
    let popup = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    h.call(Some(&popup), "Page.enable", json!({}));
    let id = h.send(Some(&popup), "Runtime.runIfWaitingForDebugger", json!({}));
    h.until_response(id);
    h.run_for(100);
    value(&mut h, "frames[0].postMessage('blank', '*'); 1");
    let session = popup.clone();
    h.until_event(move |m| m["method"] == "Page.frameNavigated" && m["sessionId"] == json!(session));
    h.run_for(100);
    value(&mut h, "frames[0].postMessage('check', '*'); 1");
    h.run_for(100);
    value(&mut h, "frames[0].postMessage('got', '*'); 1");
    h.run_for(100);
    assert_eq!(value(&mut h, "log.join(' ')"), json!("opened navigating access:object got:slash"));
}

/// BroadcastChannel matches opaque origins by identity: an about:blank iframe inherits its opaque
/// parent's origin and the two talk; a sandboxed frame (an opaque origin of its own) hears neither.
#[test]
fn broadcast_channel_matches_opaque_origins_by_identity() {
    let mut h = Harness::new();
    let sandboxed = "<script>new BroadcastChannel('c').onmessage = (e) => parent.postMessage('sandboxed:' + e.data, '*')</script>";
    load_html(
        &mut h,
        &format!(
            "<!doctype html><script>window.log = []; onmessage = (e) => log.push(e.data); new BroadcastChannel('c').onmessage = (e) => log.push('top:' + e.data);</script><iframe id=blank></iframe><iframe sandbox=allow-scripts srcdoc=\"{sandboxed}\"></iframe>"
        ),
    );
    h.run_for(100);
    let r = h.page(
        "Runtime.evaluate",
        json!({ "expression": "blank.contentWindow.eval(\"new BroadcastChannel('c').postMessage('hi')\"); 1", "returnByValue": true }),
    );
    assert_eq!(r["result"]["result"]["value"], json!(1), "{r}");
    h.run_for(100);
    let r = h.page("Runtime.evaluate", json!({ "expression": "log.join(' ')", "returnByValue": true }));
    assert_eq!(r["result"]["result"]["value"], json!("top:hi"));
}

/// A closed popup's document leaves the DOM store it shares with its opener and is freed, even while
/// the opener keeps its WindowProxy (and a function it read through it): after close the proxy answers
/// `closed` and holds nothing that keeps the popup's realm or DOM alive. Freeing takes two garbage
/// collections (the first frees the popup's realm, whose finalizers release its node wrappers), as the
/// Performance.getMetrics `Nodes` count of the shared store shows.
#[test]
fn closing_a_popup_frees_its_document_while_the_opener_keeps_its_handle() {
    let mut h = Harness::new();
    load_html(&mut h, "<!doctype html><p>opener</p>");
    let nodes = |h: &mut Harness| {
        let r = h.page("Performance.getMetrics", json!({}));
        r["result"]["metrics"].as_array().unwrap_or_else(|| panic!("{r}")).iter().find(|m| m["name"] == "Nodes").unwrap()["value"].as_f64().unwrap()
    };
    h.run_for(50);
    let before = nodes(&mut h);
    h.page(
        "Runtime.evaluate",
        json!({"expression": "globalThis.w = window.open(); w.eval(\"document.body.innerHTML = '<div><span>x</span></div>'.repeat(300)\"); w.document.title; globalThis.f = w.eval; 1"}),
    );
    h.run_for(50);
    assert!(nodes(&mut h) > before + 600.0, "the popup's nodes are in the shared store");
    h.page("Runtime.evaluate", json!({"expression": "w.close(); 1"}));
    h.run_for(300);
    for _ in 0..2 {
        h.page("Pyro.jscMemory", json!({"action": "collect"}));
        for _ in 0..3 {
            h.page("Runtime.evaluate", json!({"expression": "1"}));
            h.run_for(30);
        }
    }
    let after = nodes(&mut h);
    assert!(after < before + 100.0, "the closed popup's document is freed: before {before}, after {after}");
    let r = h.page("Runtime.evaluate", json!({"expression": "[w.closed, typeof w.document].join()", "returnByValue": true}));
    assert_eq!(r["result"]["result"]["value"], json!("true,undefined"), "{r}");
}
