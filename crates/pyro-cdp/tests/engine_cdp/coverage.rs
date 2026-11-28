//! `page.coverage` over CDP: Profiler precise coverage with the Debugger
//! script events Playwright reads it with, and CSS rule usage tracking.
use super::*;

/// Runs the engine until a message matching `pred`; every message seen.
fn collect_until(h: &mut Harness, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        seen.extend(h.drain());
        if seen.iter().any(&pred) {
            return seen;
        }
        assert!(Instant::now() < deadline, "not seen; saw {seen:#?}");
        let host = h.browser.host_mut();
        host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        host.flush();
    }
}

/// The text of a V8 range of `source` (UTF-16 offsets; ASCII here).
fn text(source: &str, range: &Value) -> String {
    let (s, e) = (range["startOffset"].as_u64().unwrap() as usize, range["endOffset"].as_u64().unwrap() as usize);
    source[s..e].to_owned()
}

#[test]
fn precise_coverage_reports_executed_and_unexecuted_blocks() {
    if !pyro_js::Engine::has_coverage() {
        eprintln!("skipped: JavaScriptCore without block coverage (patch 0019)");
        return;
    }
    let mut h = Harness::new();
    let mut seen = Vec::new();
    for (method, params) in [
        ("Profiler.enable", json!({})),
        ("Profiler.startPreciseCoverage", json!({"callCount": true, "detailed": true})),
        ("Debugger.enable", json!({})),
        ("Debugger.setSkipAllPauses", json!({"skip": true})),
    ] {
        let session = h.session.clone();
        let id = h.send(Some(&session), method, params);
        let messages = h.until_response(id);
        let reply = messages.iter().find(|m| m["id"] == id).unwrap();
        assert!(reply["error"].is_null(), "{method}: {reply}");
        seen.extend(messages);
    }
    h.page("Page.enable", json!({}));
    let html = r#"<!doctype html><body><script>
function used(x) { if (x > 0) { return 1; } else { return 2; } }
function unused() { return 3; }
used(1); used(2);
</script><script type="module">
function fromModule() { return 1; }
function notFromModule() { return 2; }
fromModule();
</script>"#;
    let id = h.send(Some(&h.session.clone()), "Page.navigate", json!({ "url": html_url(html) }));
    seen.extend(h.until_response(id));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired") {
        seen.extend(collect_until(&mut h, |m| m["method"] == "Page.loadEventFired"));
    }
    // The evaluation of a CDP client is not the page's.
    let r = h.page("Runtime.evaluate", json!({"expression": "used(5)"}));
    assert_eq!(r["result"]["result"]["value"], 1);

    let session = h.session.clone();
    let id = h.send(Some(&session), "Profiler.takePreciseCoverage", json!({}));
    let messages = h.until_response(id);
    seen.extend(messages.iter().cloned());
    let take = messages.into_iter().find(|m| m["id"] == id).unwrap();
    let parsed: Vec<&Value> = seen.iter().filter(|m| m["method"] == "Debugger.scriptParsed").collect();
    assert_eq!(parsed.len(), 2, "the page's two scripts: {parsed:#?}");
    assert_eq!(parsed[1]["params"]["isModule"], true);
    let result = take["result"]["result"].as_array().expect("result").clone();
    assert_eq!(result.len(), 2, "{take:#}");

    let classic = &result[0];
    assert_eq!(classic["scriptId"], parsed[0]["params"]["scriptId"]);
    assert!(classic["url"].as_str().unwrap().starts_with("data:text/html,"));
    let source = h.page("Debugger.getScriptSource", json!({"scriptId": classic["scriptId"]}));
    let source = source["result"]["scriptSource"].as_str().unwrap().to_owned();
    assert!(source.contains("function unused"));
    let functions = classic["functions"].as_array().unwrap();
    // The script as a whole first, as V8 does.
    assert_eq!(functions[0]["functionName"], "");
    assert_eq!(functions[0]["ranges"][0]["startOffset"], 0);
    assert_eq!(functions[0]["ranges"][0]["endOffset"], source.encode_utf16().count());
    let by_name = |name: &str| functions.iter().find(|f| f["functionName"] == name).unwrap_or_else(|| panic!("{name}"));
    let used = by_name("used");
    assert_eq!(used["isBlockCoverage"], true);
    // Two calls from the page and one from Runtime.evaluate.
    assert_eq!(used["ranges"][0]["count"], 3, "{used}");
    assert!(text(&source, &used["ranges"][0]).starts_with("function used"));
    let unexecuted: Vec<String> = used["ranges"].as_array().unwrap()[1..].iter().map(|r| text(&source, r)).collect();
    assert!(unexecuted.iter().any(|t| t.contains("return 2")), "{unexecuted:?}");
    assert!(!unexecuted.iter().any(|t| t.contains("return 1")), "{unexecuted:?}");
    let unused = by_name("unused");
    assert_eq!(unused["ranges"].as_array().unwrap().len(), 1);
    assert_eq!(unused["ranges"][0]["count"], 0);
    assert_eq!(text(&source, &unused["ranges"][0]), "function unused() { return 3; }");

    let module = &result[1];
    // The page's text, offsets into it (not the loader's `import.meta` line). An inline module's
    // text is placed at its document line (c40b7ef84: the engine numbers a module's lines from 1, so
    // error lines are the document's): the script starts on line 5, four blank lines come first.
    let source = h.page("Debugger.getScriptSource", json!({"scriptId": module["scriptId"]}));
    let source = source["result"]["scriptSource"].as_str().unwrap().to_owned();
    assert!(source.starts_with("\n\n\n\n\nfunction fromModule"), "{source:?}");
    let functions = module["functions"].as_array().unwrap();
    let not_from = functions.iter().find(|f| f["functionName"] == "notFromModule").unwrap();
    assert_eq!(text(&source, &not_from["ranges"][0]), "function notFromModule() { return 2; }");
    let count =
        |name: &str| functions.iter().find(|f| f["functionName"] == name).unwrap()["ranges"][0]["count"].clone();
    assert_eq!(count("fromModule"), 1);
    assert_eq!(count("notFromModule"), 0);

    for method in ["Profiler.stopPreciseCoverage", "Profiler.disable", "Debugger.disable"] {
        assert!(h.page(method, json!({}))["error"].is_null(), "{method}");
    }
    // Off again: code runs (recompiled) without counters, nothing is reported.
    let r = h.page("Runtime.evaluate", json!({"expression": "used(-1)"}));
    assert_eq!(r["result"]["result"]["value"], 2);
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    assert_eq!(take["result"]["result"], json!([]));
}

#[test]
fn css_rule_usage_marks_rules_that_match() {
    let mut h = Harness::new();
    let mut seen = Vec::new();
    for method in ["DOM.enable", "CSS.enable", "CSS.startRuleUsageTracking"] {
        let session = h.session.clone();
        let id = h.send(Some(&session), method, json!({}));
        seen.extend(h.until_response(id));
    }
    h.page("Page.enable", json!({}));
    let css = ".a { color: red } .b, .c::before { color: blue } @media (max-width: 1px) { .a { color: green } } .gone { x: 1 } .brief { color: teal }";
    let html = format!("<!doctype html><style>{css}</style><div class=a></div><div class=c></div><script>document.body.appendChild(document.createElement('p')).className = 'gone'; setTimeout(() => document.querySelector('.gone').remove(), 300); setTimeout(() => {{ const el = document.body.appendChild(document.createElement('i')); el.className = 'brief'; getComputedStyle(el).color; el.remove(); }}, 50)</script>");
    let id = h.send(Some(&h.session.clone()), "Page.navigate", json!({ "url": html_url(&html) }));
    seen.extend(h.until_response(id));
    seen.extend(collect_until(&mut h, |m| m["method"] == "Page.loadEventFired"));
    if !seen.iter().any(|m| m["method"] == "CSS.styleSheetAdded") {
        seen.extend(collect_until(&mut h, |m| m["method"] == "CSS.styleSheetAdded"));
    }
    // The rule for `.gone` matched, then the element went.
    let deadline = Instant::now() + Duration::from_secs(5);
    while h.page("Runtime.evaluate", json!({"expression": "!!document.querySelector('.gone')"}))["result"]["result"]
        ["value"]
        == true
    {
        assert!(Instant::now() < deadline, ".gone stays");
        let host = h.browser.host_mut();
        host.run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        host.flush();
    }
    seen.extend(h.drain());
    let added: Vec<&Value> = seen.iter().filter(|m| m["method"] == "CSS.styleSheetAdded").collect();
    assert_eq!(added.len(), 1, "{added:#?}");
    let header = &added[0]["params"]["header"];
    assert_eq!(header["isInline"], true);
    assert!(header["sourceURL"].as_str().unwrap().starts_with("data:text/html,"));
    let id = header["styleSheetId"].clone();
    let sheet = h.page("CSS.getStyleSheetText", json!({"styleSheetId": id}));
    let text = sheet["result"]["text"].as_str().unwrap().to_owned();
    assert_eq!(text, css);

    let stop = h.page("CSS.stopRuleUsageTracking", json!({}));
    let usage = stop["result"]["ruleUsage"].as_array().expect("ruleUsage").clone();
    let rules: Vec<(String, bool)> = usage
        .iter()
        .map(|u| {
            let (s, e) = (u["startOffset"].as_u64().unwrap() as usize, u["endOffset"].as_u64().unwrap() as usize);
            (text[s..e].to_owned(), u["used"].as_bool().unwrap())
        })
        .collect();
    assert_eq!(
        rules,
        vec![
            (".a { color: red }".to_owned(), true),
            (".b, .c::before { color: blue }".to_owned(), true),
            (".a { color: green }".to_owned(), false),
            // Matched while tracking, gone by the end.
            (".gone { x: 1 }".to_owned(), true),
            // In the document only while a style query recalculated it.
            (".brief { color: teal }".to_owned(), true),
        ],
        "{usage:#?}"
    );
}

/// Turns precise coverage on for the harness's current page session; the
/// messages seen meanwhile.
fn start_precise(h: &mut Harness) -> Vec<Value> {
    let mut seen = Vec::new();
    for (method, params) in [
        ("Profiler.enable", json!({})),
        ("Profiler.startPreciseCoverage", json!({"callCount": true, "detailed": true})),
        ("Debugger.enable", json!({})),
    ] {
        let session = h.session.clone();
        let id = h.send(Some(&session), method, params);
        let messages = h.until_response(id);
        let reply = messages.iter().find(|m| m["id"] == id).unwrap();
        assert!(reply["error"].is_null(), "{method}: {reply}");
        seen.extend(messages);
    }
    seen
}

#[test]
fn precise_coverage_is_per_page_when_pages_share_the_vm() {
    if !pyro_js::Engine::has_coverage() {
        eprintln!("skipped: JavaScriptCore without block coverage (patch 0019)");
        return;
    }
    let mut h = Harness::new();
    let first = h.session.clone();
    start_precise(&mut h);

    // A second page runs a script while only the first has coverage on.
    let id = h.send(None, "Target.createTarget", json!({"url": "about:blank"}));
    let attached = h.until_response(id).into_iter().find(|m| m["method"] == "Target.attachedToTarget").unwrap();
    let second = attached["params"]["sessionId"].as_str().unwrap().to_owned();
    h.session = second.clone();
    h.page("Runtime.runIfWaitingForDebugger", json!({}));
    h.page("Page.enable", json!({}));
    // A relative `//# sourceURL=` names a page script.
    let html = "<!doctype html><script>function late(x) { return x ? 1 : 2; } late(1);\n//# sourceURL=late.js</script>";
    let mut seen = h.page_with_events("Page.navigate", json!({ "url": html_url(html) }));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired" && m["sessionId"] == second) {
        seen.push(h.until_event(|m| m["method"] == "Page.loadEventFired" && m["sessionId"] == second));
    }

    // It starts coverage later: the script is its own, counted from now.
    let seen = start_precise(&mut h);
    let parsed: Vec<&Value> =
        seen.iter().filter(|m| m["method"] == "Debugger.scriptParsed" && m["sessionId"] == second).collect();
    assert!(parsed.iter().any(|m| m["params"]["url"] == "late.js"), "{parsed:#?}");
    let r = h.page("Runtime.evaluate", json!({"expression": "late(0)"}));
    assert_eq!(r["result"]["result"]["value"], 2);
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    let result = take["result"]["result"].as_array().expect("result").clone();
    let late = result.iter().find(|s| s["url"] == "late.js").unwrap_or_else(|| panic!("{take:#}"));
    let f = late["functions"].as_array().unwrap().iter().find(|f| f["functionName"] == "late").unwrap();
    assert_eq!(f["ranges"][0]["count"], 1, "the call before the start is not counted: {f}");
    // A take resets the counts, as V8's: nothing of it ran since, so V8
    // leaves the script out.
    let r = h.page("Runtime.evaluate", json!({"expression": "late(1)"}));
    assert_eq!(r["result"]["result"]["value"], 1);
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    let result = take["result"]["result"].as_array().expect("result").clone();
    let late = result.iter().find(|s| s["url"] == "late.js").unwrap_or_else(|| panic!("{take:#}"));
    let f = late["functions"].as_array().unwrap().iter().find(|f| f["functionName"] == "late").unwrap();
    assert_eq!(f["ranges"][0]["count"], 1, "{f}");
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    let result = take["result"]["result"].as_array().expect("result").clone();
    assert!(!result.iter().any(|s| s["url"] == "late.js"), "{take:#}");

    // The first page has none of it.
    h.session = first;
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    let result = take["result"]["result"].as_array().expect("result");
    assert!(!result.iter().any(|s| s["url"] == "late.js"), "{take:#}");
}

#[test]
fn precise_coverage_started_after_the_page_ran_reports_what_runs_since() {
    if !pyro_js::Engine::has_coverage() {
        eprintln!("skipped: JavaScriptCore without block coverage (patch 0019)");
        return;
    }
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let html = "<!doctype html><script>\nfunction used(x) { if (x > 0) { return 1; } else { return 2; } }\nfunction unused() { return 3; }\nfunction outer() { function inner() { return 4; } return 5; }\nused(1);\n//# sourceURL=app.js</script><script>function other() { return 1; } other();\n//# sourceURL=other.js</script>";
    let mut seen = h.page_with_events("Page.navigate", json!({ "url": html_url(html) }));
    if !seen.iter().any(|m| m["method"] == "Page.loadEventFired") {
        seen.push(h.until_event(|m| m["method"] == "Page.loadEventFired"));
    }

    // As Chrome when coverage starts after the page loaded: counts start at
    // zero, and only what runs again is reported (with the functions it
    // declares); the top level and scripts none of whose code ran since are
    // left out.
    start_precise(&mut h);
    let r = h.page("Runtime.evaluate", json!({"expression": "used(-1) + used(-2) + outer()"}));
    assert_eq!(r["result"]["result"]["value"], 9);
    let take = h.page("Profiler.takePreciseCoverage", json!({}));
    let result = take["result"]["result"].as_array().expect("result").clone();
    assert!(!result.iter().any(|s| s["url"] == "other.js"), "{take:#}");
    let app = result.iter().find(|s| s["url"] == "app.js").unwrap_or_else(|| panic!("{take:#}"));
    let functions = app["functions"].as_array().unwrap();
    let names: Vec<&str> = functions.iter().map(|f| f["functionName"].as_str().unwrap()).collect();
    assert_eq!(names, ["used", "outer", "inner"], "{app:#}");
    let used = &functions[0];
    assert_eq!(used["ranges"][0]["count"], 2, "{used}");
    let source = html.split("<script>").nth(1).unwrap().split("</script>").next().unwrap();
    let zero: Vec<String> = used["ranges"].as_array().unwrap()[1..].iter().map(|r| text(source, r)).collect();
    assert!(zero.iter().any(|t| t.contains("return 1")), "{zero:?}");
    assert!(!zero.iter().any(|t| t.contains("return 2")), "{zero:?}");
    assert_eq!(functions[1]["ranges"][0]["count"], 1);
    assert_eq!(functions[2]["ranges"][0]["count"], 0);
}
