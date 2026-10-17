//! Protocol sequencing for Target/Browser commands against a recording host.
//! These check message order and id bookkeeping only; browser behavior is
//! covered by the engine integration tests.

use lp_cdp::msg::{self, CdpResult, Command};
use lp_cdp::target::{Browser, ContextKey, Host, PageCommand, PageKey, Sink};
use lp_cdp::ws::OutFrame;
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder {
    calls: Vec<String>,
    next: u64,
}

impl Host for Recorder {
    fn create_context(&mut self) -> CdpResult<ContextKey> {
        self.next += 1;
        self.calls.push(format!("create_context -> {}", self.next));
        Ok(self.next)
    }
    fn dispose_context(&mut self, context: ContextKey) {
        self.calls.push(format!("dispose_context {context}"));
    }
    fn create_page(&mut self, context: ContextKey, _target_id: &str) -> CdpResult<PageKey> {
        self.next += 1;
        self.calls.push(format!("create_page {context} -> {}", self.next));
        Ok(self.next)
    }
    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()> {
        self.calls.push(format!("navigate {page} {url}"));
        Ok(())
    }
    fn close_page(&mut self, page: PageKey) {
        self.calls.push(format!("close_page {page}"));
    }
    fn page_url(&self, _page: PageKey) -> String {
        "about:blank".into()
    }
    fn page_title(&self, _page: PageKey) -> String {
        String::new()
    }
    fn session_attached(&mut self, page: PageKey, _session_id: &str) {
        self.calls.push(format!("session_attached {page}"));
    }
    fn session_detached(&mut self, page: PageKey, _session_id: &str) {
        self.calls.push(format!("session_detached {page}"));
    }
    fn page_command(&mut self, cmd: PageCommand<'_, '_>, out: &mut dyn Sink) {
        self.calls.push(format!("page_command {} {}", cmd.page, cmd.command.method));
        out.send(msg::result_frame(cmd.command.id, &json!({}), Some(cmd.session_id)));
    }
    fn browser_command(&mut self, command: &Command<'_>, context: Option<ContextKey>, out: &mut dyn Sink) {
        self.calls.push(format!("browser_command {} {context:?}", command.method));
        out.send(msg::error_frame(Some(command.id), &msg::CdpError::unsupported(&command.method), None));
    }
}

fn decode(frames: Vec<OutFrame>) -> Vec<Value> {
    frames
        .into_iter()
        .map(|f| {
            let bytes = f.finish_text();
            let wire = bytes.as_ref();
            let header = match wire[1] {
                126 => 4,
                127 => 10,
                _ => 2,
            };
            serde_json::from_slice(&wire[header..]).unwrap()
        })
        .collect()
}

fn send(browser: &mut Browser<Recorder>, message: Value) -> Vec<Value> {
    let mut out: Vec<OutFrame> = Vec::new();
    browser.handle_text(&message.to_string(), &mut out);
    decode(out)
}

/// Mirrors Playwright 1.59 connectOverCDP + newContext + newPage ordering.
#[test]
fn playwright_connect_new_context_new_page_sequence() {
    let mut b = Browser::new(Recorder::default());
    let r = send(&mut b, json!({"id":1,"method":"Browser.getVersion"}));
    assert_eq!(r[0]["result"]["product"], "Chrome/124.0.6367.29");
    assert!(!r[0]["result"]["userAgent"].as_str().unwrap().contains("Headless"));

    let r = send(
        &mut b,
        json!({"id":2,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}}),
    );
    assert_eq!(r, vec![json!({"id":2,"result":{}})], "no placeholder page is attached");

    let r = send(&mut b, json!({"id":3,"method":"Target.getTargetInfo"}));
    assert_eq!(r[0]["result"]["targetInfo"]["type"], "browser");

    let r = send(&mut b, json!({"id":4,"method":"Target.createBrowserContext","params":{"disposeOnDetach":true}}));
    let ctx = r[0]["result"]["browserContextId"].as_str().unwrap().to_owned();

    let r = send(
        &mut b,
        json!({"id":5,"method":"Target.createTarget","params":{"url":"about:blank","browserContextId":ctx}}),
    );
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["method"], "Target.attachedToTarget", "attach precedes the createTarget result");
    let session = r[0]["params"]["sessionId"].as_str().unwrap().to_owned();
    let target = r[1]["result"]["targetId"].as_str().unwrap().to_owned();
    assert_eq!(r[0]["params"]["targetInfo"]["targetId"], target.as_str());
    assert_eq!(r[0]["params"]["targetInfo"]["browserContextId"], ctx.as_str());
    assert_eq!(r[0]["params"]["targetInfo"]["type"], "page");
    assert_eq!(r[0]["params"]["waitingForDebugger"], false);

    let r = send(&mut b, json!({"id":6,"method":"Page.enable","sessionId":session}));
    assert_eq!(r, vec![json!({"id":6,"result":{},"sessionId":session})]);
    let r = send(
        &mut b,
        json!({"id":7,"method":"Target.setAutoAttach","sessionId":session,"params":{"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}}),
    );
    assert_eq!(r, vec![json!({"id":7,"result":{},"sessionId":session})]);
    let r = send(&mut b, json!({"id":8,"method":"Runtime.runIfWaitingForDebugger","sessionId":session}));
    assert_eq!(r, vec![json!({"id":8,"result":{},"sessionId":session})]);

    let r = send(&mut b, json!({"id":9,"method":"Target.closeTarget","params":{"targetId":target}}));
    assert_eq!(r[0]["method"], "Target.detachedFromTarget");
    assert_eq!(r[0]["params"]["sessionId"], session.as_str());
    assert_eq!(r[1], json!({"id":9,"result":{"success":true}}));

    let r = send(&mut b, json!({"id":10,"method":"Page.enable","sessionId":session}));
    assert_eq!(r[0]["error"]["code"], -32001);
    assert_eq!(r[0]["sessionId"], session.as_str());

    let r = send(&mut b, json!({"id":11,"method":"Target.disposeBrowserContext","params":{"browserContextId":ctx}}));
    assert_eq!(r, vec![json!({"id":11,"result":{}})]);
    assert_eq!(
        b.host().calls,
        [
            "create_context -> 1",
            "create_page 1 -> 2",
            "session_attached 2",
            "page_command 2 Page.enable",
            "session_detached 2",
            "close_page 2",
            "dispose_context 1",
        ]
    );
}

#[test]
fn dispose_context_closes_its_targets_before_responding() {
    let mut b = Browser::new(Recorder::default());
    send(
        &mut b,
        json!({"id":1,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}}),
    );
    let ctx =
        send(&mut b, json!({"id":2,"method":"Target.createBrowserContext"}))[0]["result"]["browserContextId"].clone();
    send(
        &mut b,
        json!({"id":3,"method":"Target.createTarget","params":{"url":"http://127.0.0.1/","browserContextId":ctx}}),
    );
    let r = send(&mut b, json!({"id":4,"method":"Target.disposeBrowserContext","params":{"browserContextId":ctx}}));
    assert_eq!(r[0]["method"], "Target.detachedFromTarget");
    assert_eq!(r[1], json!({"id":4,"result":{}}));
    assert!(b.host().calls.contains(&"navigate 2 http://127.0.0.1/".to_owned()));

    let r = send(&mut b, json!({"id":5,"method":"Target.disposeBrowserContext","params":{"browserContextId":ctx}}));
    assert_eq!(r[0]["error"]["code"], -32602);
}

#[test]
fn multiple_contexts_are_independent() {
    let mut b = Browser::new(Recorder::default());
    let a =
        send(&mut b, json!({"id":1,"method":"Target.createBrowserContext"}))[0]["result"]["browserContextId"].clone();
    let c =
        send(&mut b, json!({"id":2,"method":"Target.createBrowserContext"}))[0]["result"]["browserContextId"].clone();
    assert_ne!(a, c);
    let r = send(&mut b, json!({"id":3,"method":"Target.getBrowserContexts"}));
    assert_eq!(r[0]["result"]["browserContextIds"], json!([a, c]));
}

#[test]
fn discover_targets_reports_lifecycle() {
    let mut b = Browser::new(Recorder::default());
    let r = send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{"url":""}}));
    assert_eq!(r.len(), 1, "no events without discover/auto-attach");
    let target = r[0]["result"]["targetId"].clone();
    let r = send(&mut b, json!({"id":2,"method":"Target.setDiscoverTargets","params":{"discover":true}}));
    assert_eq!(r[1]["method"], "Target.targetCreated");
    assert_eq!(r[1]["params"]["targetInfo"]["attached"], false);
    let r = send(&mut b, json!({"id":3,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}));
    assert_eq!(r[0]["method"], "Target.attachedToTarget");
    let session = r[1]["result"]["sessionId"].clone();
    assert_eq!(r[0]["params"]["sessionId"], session);
    let r = send(&mut b, json!({"id":4,"method":"Target.detachFromTarget","params":{"sessionId":session}}));
    assert_eq!(r[0]["method"], "Target.detachedFromTarget");
    assert_eq!(r[1], json!({"id":4,"result":{}}));
    let r = send(&mut b, json!({"id":5,"method":"Target.closeTarget","params":{"targetId":target}}));
    assert_eq!(r[0]["method"], "Target.targetDestroyed");
}

#[test]
fn errors_are_reported_with_ids() {
    let mut b = Browser::new(Recorder::default());
    let r = send(&mut b, json!({"id":1,"method":"Nope.nope"}));
    assert_eq!(r[0]["id"], 1);
    assert_eq!(r[0]["error"]["code"], -32000);
    let r = send(&mut b, json!({"id":2,"method":"Target.createBrowserContext","params":{"proxyServer":"http://p"}}));
    assert_eq!(r[0]["error"]["code"], -32000, "proxy is refused, not silently ignored");
    let r = send(&mut b, json!({"id":3,"method":"Target.createTarget","params":{"browserContextId":"missing"}}));
    assert_eq!(r[0]["error"]["code"], -32602);
    let r = send(
        &mut b,
        json!({"id":4,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false}}),
    );
    assert_eq!(r[0]["error"]["message"], "Only flatten mode is supported");
    let r = send(&mut b, json!({"id":5,"method":"Storage.getCookies","params":{"browserContextId":"missing"}}));
    assert_eq!(r[0]["error"]["code"], -32602);
}

#[test]
fn shutdown_disposes_all_contexts() {
    let mut b = Browser::new(Recorder::default());
    send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{}}));
    send(&mut b, json!({"id":2,"method":"Target.createBrowserContext"}));
    b.shutdown();
    let calls = &b.host().calls;
    assert!(calls.contains(&"close_page 2".to_owned()));
    assert!(calls.contains(&"dispose_context 1".to_owned()));
    assert!(calls.contains(&"dispose_context 3".to_owned()));
}
