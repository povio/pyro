//! Protocol sequencing for Target/Browser commands against a recording host.
//! These check message order and id bookkeeping only, not browser behavior
//! (see tests/engine_cdp.rs and tests/e2e for the JSC-backed checks).

use lp_cdp::msg::{self, CdpResult, Command};
use lp_cdp::target::{Browser, ContextKey, Host, PageCommand, PageKey, Sink};
use lp_cdp::ws::OutFrame;
use serde_json::{json, Value};

#[derive(Default)]
struct Recorder {
    calls: Vec<String>,
    next: u64,
    fail_navigation: bool,
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
    fn create_page(&mut self, context: ContextKey) -> CdpResult<(PageKey, String)> {
        self.next += 1;
        self.calls.push(format!("create_page {context} -> {}", self.next));
        Ok((self.next, format!("{:032X}", self.next)))
    }
    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()> {
        self.calls.push(format!("navigate {page} {url}"));
        if self.fail_navigation {
            return Err(msg::CdpError::server("navigation failed"));
        }
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
fn default_context_is_separate_from_created_contexts() {
    let mut b = Browser::new(Recorder::default());
    let created =
        send(&mut b, json!({"id":1,"method":"Target.createBrowserContext"}))[0]["result"]["browserContextId"].clone();
    send(
        &mut b,
        json!({"id":2,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}}),
    );
    let r = send(&mut b, json!({"id":3,"method":"Target.createTarget","params":{"url":""}}));
    let default = r[0]["params"]["targetInfo"]["browserContextId"].clone();
    assert_ne!(default, created, "default target is not placed in a created context");
    let r = send(&mut b, json!({"id":4,"method":"Target.getBrowserContexts"}));
    assert_eq!(r[0]["result"]["browserContextIds"], json!([created]));
    let r = send(&mut b, json!({"id":5,"method":"Target.disposeBrowserContext","params":{"browserContextId":created}}));
    assert_eq!(r, vec![json!({"id":5,"result":{}})], "disposing the created context leaves the default target alone");
    let r = send(&mut b, json!({"id":6,"method":"Target.getTargets"}));
    assert_eq!(r[0]["result"]["targetInfos"].as_array().unwrap().len(), 1);
    let r = send(&mut b, json!({"id":7,"method":"Target.disposeBrowserContext","params":{"browserContextId":default}}));
    assert_eq!(r[0]["error"]["code"], -32000);
}

#[test]
fn failed_initial_navigation_rolls_back_the_target() {
    let mut b = Browser::new(Recorder { fail_navigation: true, ..Recorder::default() });
    send(
        &mut b,
        json!({"id":1,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}}),
    );
    let r = send(&mut b, json!({"id":2,"method":"Target.createTarget","params":{"url":"http://x/"}}));
    let methods: Vec<&str> = r.iter().filter_map(|m| m["method"].as_str()).collect();
    assert_eq!(methods, ["Target.attachedToTarget", "Target.detachedFromTarget"]);
    assert_eq!(r.last().unwrap()["error"]["code"], -32000);
    let r = send(&mut b, json!({"id":3,"method":"Target.getTargets"}));
    assert_eq!(r[0]["result"]["targetInfos"], json!([]));
}

#[test]
fn filters_excluding_pages_are_honored() {
    let mut b = Browser::new(Recorder::default());
    send(
        &mut b,
        json!({"id":1,"method":"Target.setDiscoverTargets","params":{"discover":true,"filter":[{"type":"page","exclude":true}]}}),
    );
    send(
        &mut b,
        json!({"id":2,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true,"filter":[{"type":"page","exclude":true},{}]}}),
    );
    let r = send(&mut b, json!({"id":3,"method":"Target.createTarget","params":{}}));
    assert_eq!(r.len(), 1, "no targetCreated/attachedToTarget for excluded pages: {r:?}");
    let target = r[0]["result"]["targetId"].clone();
    let r = send(&mut b, json!({"id":4,"method":"Target.activateTarget","params":{"targetId":"missing"}}));
    assert_eq!(r[0]["error"]["code"], -32602);
    let r = send(&mut b, json!({"id":5,"method":"Target.activateTarget","params":{"targetId":target}}));
    assert_eq!(r[0]["result"], json!({}));
}

#[test]
fn session_budget_is_checked_before_creating_targets() {
    let mut b = Browser::new(Recorder::default());
    let target =
        send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{}}))[0]["result"]["targetId"].clone();
    for i in 0..lp_cdp::target::MAX_SESSIONS {
        let r = send(
            &mut b,
            json!({"id": 10 + i, "method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}),
        );
        assert!(r.last().unwrap().get("result").is_some(), "attach {i}");
    }
    send(
        &mut b,
        json!({"id":2,"method":"Target.setAutoAttach","params":{"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}}),
    );
    let r = send(&mut b, json!({"id":3,"method":"Target.createTarget","params":{}}));
    assert_eq!(r.last().unwrap()["error"]["message"], "Too many sessions");
    let r = send(&mut b, json!({"id":4,"method":"Target.getTargets"}));
    assert_eq!(r[0]["result"]["targetInfos"].as_array().unwrap().len(), 1, "no target leaked");
}

#[test]
fn get_targets_honors_filters() {
    let mut b = Browser::new(Recorder::default());
    send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{}}));
    let r = send(
        &mut b,
        json!({"id":2,"method":"Target.getTargets","params":{"filter":[{"type":"page","exclude":true},{}]}}),
    );
    assert_eq!(r[0]["result"]["targetInfos"], json!([]));
    let r = send(&mut b, json!({"id":3,"method":"Target.getTargets","params":{"filter":[{}]}}));
    assert_eq!(r[0]["result"]["targetInfos"].as_array().unwrap().len(), 1);
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

#[test]
fn browser_session_routes_children_and_detaches_without_closing_pages() {
    let mut b = Browser::new(Recorder::default());
    let target = send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}))[0]
        ["result"]["targetId"]
        .clone();
    let parent =
        send(&mut b, json!({"id":2,"method":"Target.attachToBrowserTarget"}))[0]["result"]["sessionId"].clone();
    let other = send(&mut b, json!({"id":3,"method":"Target.attachToBrowserTarget"}))[0]["result"]["sessionId"].clone();
    let r = send(&mut b, json!({"id":4,"sessionId":parent,"method":"Browser.getVersion"}));
    assert_eq!(r[0]["sessionId"], parent);
    assert!(r[0]["result"]["product"].is_string());
    let r = send(
        &mut b,
        json!({"id":5,"sessionId":parent,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}),
    );
    assert_eq!(r[0]["method"], "Target.attachedToTarget");
    assert_eq!(r[0]["sessionId"], parent);
    assert_eq!(r[1]["sessionId"], parent);
    let child = r[1]["result"]["sessionId"].clone();
    let root =
        send(&mut b, json!({"id":6,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))[1]
            ["result"]["sessionId"]
            .clone();
    let r =
        send(&mut b, json!({"id":7,"sessionId":other,"method":"Target.detachFromTarget","params":{"sessionId":child}}));
    assert_eq!(r[0]["error"]["code"], -32001);
    assert_eq!(r[0]["sessionId"], other);
    let r = send(&mut b, json!({"id":8,"method":"Target.detachFromTarget","params":{"sessionId":parent}}));
    assert_eq!(r.len(), 3);
    assert_eq!(r[0]["params"]["sessionId"], child);
    assert_eq!(r[0]["sessionId"], parent);
    assert_eq!(r[1]["params"]["sessionId"], parent);
    assert!(r[1].get("sessionId").is_none());
    assert_eq!(send(&mut b, json!({"id":9,"sessionId":child,"method":"Page.enable"}))[0]["error"]["code"], -32001);
    assert_eq!(
        send(&mut b, json!({"id":10,"sessionId":parent,"method":"Browser.getVersion"}))[0]["error"]["code"],
        -32001
    );
    assert!(send(&mut b, json!({"id":11,"sessionId":root,"method":"Page.enable"}))[0].get("result").is_some());
    assert!(!b.host().calls.iter().any(|c| c.starts_with("close_page")));
}

#[test]
fn close_target_routes_detach_to_browser_parent() {
    let mut b = Browser::new(Recorder::default());
    let parent =
        send(&mut b, json!({"id":1,"method":"Target.attachToBrowserTarget"}))[0]["result"]["sessionId"].clone();
    let r =
        send(&mut b, json!({"id":2,"sessionId":parent,"method":"Target.createTarget","params":{"url":"about:blank"}}));
    assert_eq!(r[0]["sessionId"], parent);
    let target = r[0]["result"]["targetId"].clone();
    let child = send(
        &mut b,
        json!({"id":3,"sessionId":parent,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}),
    )[1]["result"]["sessionId"]
        .clone();
    let r = send(&mut b, json!({"id":4,"sessionId":parent,"method":"Target.closeTarget","params":{"targetId":target}}));
    assert_eq!(r[0]["method"], "Target.detachedFromTarget");
    assert_eq!(r[0]["sessionId"], parent);
    assert_eq!(r[0]["params"]["sessionId"], child);
    assert_eq!(r[1], json!({"id":4,"sessionId":parent,"result":{"success":true}}));
}

#[test]
fn browser_and_page_sessions_share_budget_and_release_slots() {
    let mut b = Browser::new(Recorder::default());
    let target = send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}))[0]
        ["result"]["targetId"]
        .clone();
    let page =
        send(&mut b, json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))[1]
            ["result"]["sessionId"]
            .clone();
    for id in 3..514 {
        assert!(send(&mut b, json!({"id":id,"method":"Target.attachToBrowserTarget"}))[0].get("result").is_some());
    }
    assert!(send(&mut b, json!({"id":514,"method":"Target.attachToBrowserTarget"}))[0].get("error").is_some());
    assert!(send(
        &mut b,
        json!({"id":515,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}})
    )[0]
    .get("error")
    .is_some());
    send(&mut b, json!({"id":516,"method":"Target.detachFromTarget","params":{"sessionId":page}}));
    assert!(send(&mut b, json!({"id":517,"method":"Target.attachToBrowserTarget"}))[0].get("result").is_some());
}

#[test]
fn disabling_auto_attach_preserves_manual_sessions() {
    let mut b = Browser::new(Recorder::default());
    send(&mut b, json!({"id":1,"method":"Target.setAutoAttach","params":{"autoAttach":true,"flatten":true}}));
    let r = send(&mut b, json!({"id":2,"method":"Target.createTarget","params":{"url":"about:blank"}}));
    let auto = r[0]["params"]["sessionId"].clone();
    let target = r[1]["result"]["targetId"].clone();
    let manual =
        send(&mut b, json!({"id":3,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))[1]
            ["result"]["sessionId"]
            .clone();
    let r = send(&mut b, json!({"id":4,"method":"Target.setAutoAttach","params":{"autoAttach":false,"flatten":true}}));
    assert_eq!(r.len(), 2);
    assert_eq!(r[1]["params"]["sessionId"], auto);
    assert!(send(&mut b, json!({"id":5,"sessionId":manual,"method":"Page.enable"}))[0].get("result").is_some());
}

#[test]
fn auto_attach_capacity_failure_is_atomic_and_has_one_reply() {
    let mut b = Browser::new(Recorder::default());
    let first = send(&mut b, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}))[0]
        ["result"]["targetId"]
        .clone();
    let second = send(&mut b, json!({"id":2,"method":"Target.createTarget","params":{"url":"about:blank"}}))[0]
        ["result"]["targetId"]
        .clone();
    let mut parents = Vec::new();
    for id in 3..514 {
        parents.push(
            send(&mut b, json!({"id":id,"method":"Target.attachToBrowserTarget"}))[0]["result"]["sessionId"].clone(),
        );
    }
    // Only one slot remains, but enabling needs two. No partial attach or
    // early success may escape before reporting the capacity error.
    let r = send(&mut b, json!({"id":514,"method":"Target.setAutoAttach","params":{"autoAttach":true,"flatten":true}}));
    assert_eq!(r.len(), 1, "one error, without a success or partial attachment: {r:?}");
    assert!(r[0].get("error").is_some());
    assert!(!b.host().calls.iter().any(|c| c.starts_with("session_attached")));
    // The failed configuration must leave auto-attach disabled.
    let r = send(&mut b, json!({"id":515,"method":"Target.createTarget","params":{"url":"about:blank"}}));
    assert_eq!(r.len(), 1);
    assert!(r[0].get("result").is_some());
    let third = r[0]["result"]["targetId"].clone();
    for (id, parent) in parents.iter().take(2).enumerate() {
        send(&mut b, json!({"id":516+id,"method":"Target.detachFromTarget","params":{"sessionId":parent}}));
    }
    let r = send(&mut b, json!({"id":518,"method":"Target.setAutoAttach","params":{"autoAttach":true,"flatten":true}}));
    assert_eq!(r.len(), 4);
    assert_eq!(r[0], json!({"id":518,"result":{}}));
    let targets: Vec<_> = r[1..].iter().map(|event| event["params"]["targetInfo"]["targetId"].clone()).collect();
    assert_eq!(targets, [first, second, third]);
}
