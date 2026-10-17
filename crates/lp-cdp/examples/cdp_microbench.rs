//! In-process CDP microbenchmark on the real JSC engine (no sockets).
//! Measures costs that decide which optimizations are worth doing; it is not
//! a browser benchmark. Run: `cargo run --release -p lp-cdp --example cdp_microbench`.

use std::time::{Duration, Instant};

use lp_cdp::engine::EngineHost;
use lp_cdp::outbox::{Outbound, OutboundRx, Outbox, DEFAULT_OUTBOUND_BUDGET};
use lp_cdp::target::Browser;
use serde_json::{json, Value};

fn stats(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let median = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!("{name:<44} n={:<5} median={:>9.1}us p95={:>9.1}us", samples.len(), us(median), us(p95));
}

fn decode(bytes: &[u8]) -> Value {
    let header = match bytes[1] {
        126 => 4,
        127 => 10,
        _ => 2,
    };
    serde_json::from_slice(&bytes[header..]).expect("JSON frame")
}

struct H {
    b: Browser<EngineHost>,
    out: Outbox,
    rx: OutboundRx,
    id: u64,
}

impl H {
    fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.id += 1;
        let mut m = json!({"id": self.id, "method": method, "params": params});
        if let Some(s) = session {
            m["sessionId"] = json!(s);
        }
        let mut sink = self.out.clone();
        self.b.handle_text(&m.to_string(), &mut sink);
        loop {
            self.b.host_mut().flush();
            while let Some(Outbound::Frame(f)) = self.rx.try_recv() {
                let v = decode(f.as_ref());
                if v["id"] == self.id {
                    assert!(v.get("error").is_none(), "{method} failed: {v}");
                    return v;
                }
            }
            self.b.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(5)), || false);
        }
    }
}

fn main() {
    let t = Instant::now();
    let (out, rx) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let host = EngineHost::new(out.clone()).expect("engine");
    let engine_new = t.elapsed();
    let mut h = H { b: Browser::new(host), out, rx, id: 0 };
    println!("EngineHost::new (JSC VM + browser thread state): {:.1}us", engine_new.as_secs_f64() * 1e6);
    h.call(None, "Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}));

    // Page creation + Playwright-style init, per page (context + target + init commands).
    let mut page_setup = Vec::new();
    let mut session = String::new();
    for _ in 0..50 {
        let t = Instant::now();
        let ctx = h.call(None, "Target.createBrowserContext", json!({}))["result"]["browserContextId"].clone();
        h.id += 1;
        let id = h.id;
        let mut sink = h.out.clone();
        h.b.handle_text(&json!({"id":id,"method":"Target.createTarget","params":{"url":"about:blank","browserContextId":ctx}}).to_string(), &mut sink);
        h.b.host_mut().flush();
        while let Some(Outbound::Frame(f)) = h.rx.try_recv() {
            let v = decode(f.as_ref());
            if let Some(s) = v["params"]["sessionId"].as_str() {
                session = s.to_owned();
            }
        }
        for m in ["Page.enable", "Runtime.enable", "Page.getFrameTree"] {
            h.call(Some(&session), m, json!({}));
        }
        assert!(!session.is_empty(), "attached session");
        page_setup.push(t.elapsed());
    }
    stats("context+target+Page/Runtime.enable", page_setup);

    let mut eval = Vec::new();
    for _ in 0..500 {
        let t = Instant::now();
        h.call(Some(&session), "Runtime.evaluate", json!({"expression":"1+1","returnByValue":true}));
        eval.push(t.elapsed());
    }
    stats("Runtime.evaluate 1+1 (by value)", eval);

    // Playwright re-sends large utility functions on every callFunctionOn;
    // lp-js call_function re-evaluates the source each time.
    let body: String = (0..2000).map(|i| format!("const v{i} = {i};\n")).collect();
    let big = format!("function(a) {{ {body} return a + 1; }}");
    let ctx_id = h.call(Some(&session), "Runtime.evaluate", json!({"expression":"1"}));
    let _ = ctx_id;
    let realm = h.call(Some(&session), "Runtime.evaluate", json!({"expression":"globalThis"}))["result"]["result"]["objectId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (label, source) in [("callFunctionOn small fn", "function(a){return a+1}".to_owned()), ("callFunctionOn ~38KB fn", big)] {
        let mut samples = Vec::new();
        for _ in 0..300 {
            let t = Instant::now();
            let r = h.call(
                Some(&session),
                "Runtime.callFunctionOn",
                json!({"functionDeclaration": source, "objectId": realm, "arguments":[{"value":1}], "returnByValue": true}),
            );
            assert_eq!(r["result"]["result"]["value"], 2);
            samples.push(t.elapsed());
        }
        stats(label, samples);
    }

    let mut handles = Vec::new();
    for _ in 0..500 {
        let t = Instant::now();
        let r = h.call(Some(&session), "Runtime.evaluate", json!({"expression":"({a:1})"}));
        let id = r["result"]["result"]["objectId"].as_str().unwrap().to_owned();
        h.call(Some(&session), "Runtime.releaseObject", json!({"objectId": id}));
        handles.push(t.elapsed());
    }
    stats("evaluate handle + releaseObject", handles);

    let mut nav = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        h.call(Some(&session), "Page.navigate", json!({"url":"about:blank"}));
        nav.push(t.elapsed());
    }
    stats("Page.navigate about:blank (realm rebuild)", nav);
}
