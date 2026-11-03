//! Tier 3 (G2): transformed box models and line fragments, Runtime previews
//! and internal properties, console stack traces,
//! terminateExecution, blocking dialogs, Page.setDocumentContent and
//! Page.close with beforeunload. Expected values were measured in Chrome.
use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use lp_cdp::engine::PumpStep;

fn close(a: &Value, b: &[f64], tolerance: f64) -> bool {
    let a: Vec<f64> =
        a.as_array().unwrap_or_else(|| panic!("not an array: {a}")).iter().map(|v| v.as_f64().unwrap()).collect();
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
}

fn node_object(h: &mut Harness, selector: &str) -> Value {
    let r = h.page("Runtime.evaluate", json!({"expression": format!("document.querySelector({})", json!(selector))}));
    r["result"]["result"]["objectId"].clone()
}

const BOXES: &str = r#"<!doctype html><style>body{margin:0;font:16px/20px sans-serif} div.b{position:absolute;width:100px;height:50px;background:red}</style>
<div class=b id=t style="left:10px;top:10px;transform:translate(5px,7px)"></div>
<div class=b id=r style="left:200px;top:10px;transform:rotate(45deg)"></div>
<div class=b id=s style="left:10px;top:200px;transform:scale(2)"></div>
<div class=b id=p style="left:300px;top:200px;padding:3px 4px 5px 6px;border:2px solid;border-left-width:7px;margin:8px 9px 10px 11px"></div>
<div class=b id=rp style="left:500px;top:300px;padding:3px;border:2px solid;margin:4px;transform:rotate(30deg)"></div>
<p style="position:absolute;left:0;top:400px;width:150px;margin:0">aaaa bbbb <span id=w style="padding:0 2px;border:1px solid;margin:0 3px">cccc dddd eeee ffff</span> gg</p>"#;

#[cfg(feature = "render")]
#[test]
fn box_models_follow_transforms_and_report_each_box() {
    let mut h = Harness::new();
    load_html(&mut h, BOXES);
    let model = |h: &mut Harness, sel: &str| {
        let id = node_object(h, sel);
        let r = h.page("DOM.getBoxModel", json!({"objectId": id}));
        let q = h.page("DOM.getContentQuads", json!({"objectId": id}));
        assert!(r.get("error").is_none() && q.get("error").is_none(), "{sel}: {r} {q}");
        (r["result"]["model"].clone(), q["result"]["quads"].clone())
    };
    // translate: an axis-aligned box moved.
    let (m, q) = model(&mut h, "#t");
    assert!(close(&m["border"], &[15.0, 17.0, 115.0, 17.0, 115.0, 67.0, 15.0, 67.0], 0.01), "{m}");
    assert_eq!(q, json!([[15.0, 17.0, 115.0, 17.0, 115.0, 67.0, 15.0, 67.0]]));
    // rotate(45deg): the quad's points, not its bounding box.
    let (m, q) = model(&mut h, "#r");
    let rotated = [232.322, -18.033, 303.033, 52.678, 267.678, 88.033, 196.967, 17.322];
    assert!(close(&m["border"], &rotated, 0.01), "{m}");
    assert!(close(&q[0], &rotated, 0.01), "{q}");
    assert_eq!((m["width"].clone(), m["height"].clone()), (json!(100), json!(50)), "untransformed size");
    // scale(2) about the center.
    let (m, _) = model(&mut h, "#s");
    assert!(close(&m["content"], &[-40.0, 175.0, 160.0, 175.0, 160.0, 275.0, -40.0, 275.0], 0.01), "{m}");
    // Separate content, padding, border and margin boxes.
    let (m, q) = model(&mut h, "#p");
    assert!(close(&m["content"], &[324.0, 213.0, 424.0, 213.0, 424.0, 263.0, 324.0, 263.0], 0.01), "{m}");
    assert!(close(&m["padding"], &[318.0, 210.0, 428.0, 210.0, 428.0, 268.0, 318.0, 268.0], 0.01), "{m}");
    assert!(close(&m["border"], &[311.0, 208.0, 430.0, 208.0, 430.0, 270.0, 311.0, 270.0], 0.01), "{m}");
    assert!(close(&m["margin"], &[300.0, 200.0, 439.0, 200.0, 439.0, 280.0, 300.0, 280.0], 0.01), "{m}");
    assert_eq!((m["width"].clone(), m["height"].clone()), (json!(119), json!(62)));
    assert!(close(&q[0], &[311.0, 208.0, 430.0, 208.0, 430.0, 270.0, 311.0, 270.0], 0.01), "{q}");
    // Rotated with padding, border and margin: every box rotates.
    let (m, _) = model(&mut h, "#rp");
    assert!(
        close(&m["content"], &[528.199, 287.349, 614.801, 337.349, 589.801, 380.651, 503.199, 330.651], 0.01),
        "{m}"
    );
    assert!(
        close(&m["margin"], &[524.904, 275.055, 627.096, 334.055, 593.096, 392.945, 490.905, 333.945], 0.01),
        "{m}"
    );
    // An inline box over two lines: one content quad per line, the first
    // starting after its start border and padding, the last ending after
    // its end ones (Chrome: x 86.08..118.08 then 0..97.02).
    let (m, q) = model(&mut h, "#w");
    let quads = q.as_array().unwrap();
    assert_eq!(quads.len(), 2, "{q}");
    let (first, second) = (&quads[0], &quads[1]);
    assert!(first[1].as_f64().unwrap() < second[1].as_f64().unwrap(), "lines in order: {q}");
    assert!((first[2].as_f64().unwrap() - 118.0).abs() < 8.0, "{q}");
    assert!(second[0].as_f64().unwrap().abs() < 0.01, "{q}");
    let border = m["border"].as_array().unwrap();
    let padding = m["padding"].as_array().unwrap();
    assert!((padding[0].as_f64().unwrap() - border[0].as_f64().unwrap() - 1.0).abs() < 0.01, "{m}");
    let margin = m["margin"].as_array().unwrap();
    assert!((border[0].as_f64().unwrap() - margin[0].as_f64().unwrap() - 3.0).abs() < 0.01, "{m}");
}

fn evaluate(h: &mut Harness, expression: &str, extra: Value) -> Value {
    let mut params = json!({"expression": expression});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    let r = h.page("Runtime.evaluate", params);
    assert!(r["result"].get("exceptionDetails").is_none(), "{r}");
    r["result"]["result"].clone()
}

#[test]
fn object_previews_follow_chrome_shapes() {
    let mut h = Harness::new();
    let preview = |h: &mut Harness, e: &str| evaluate(h, e, json!({"generatePreview": true}))["preview"].clone();
    let p = preview(&mut h, r#"({a:1, b:"x", c:{d:2}, e:[1,2], f(){}, g:null})"#);
    assert_eq!(p["type"], "object");
    assert_eq!(p["description"], "Object");
    assert_eq!(p["overflow"], true, "{p}");
    assert_eq!(
        p["properties"],
        json!([
            {"name":"a","type":"number","value":"1"},
            {"name":"b","type":"string","value":"x"},
            {"name":"c","type":"object","value":"Object"},
            {"name":"e","type":"object","value":"Array(2)","subtype":"array"},
            {"name":"f","type":"function","value":""},
        ])
    );
    let p = preview(&mut h, r#"[1,"two",null]"#);
    assert_eq!(p["subtype"], "array");
    assert_eq!(p["description"], "Array(3)");
    assert_eq!(p["overflow"], false);
    assert_eq!(p["properties"][2], json!({"name":"2","type":"object","value":"null","subtype":"null"}));
    let p = preview(&mut h, "Array.from({length:120},(_, i)=>i)");
    assert_eq!((p["overflow"].clone(), p["properties"].as_array().unwrap().len()), (json!(true), 100));
    let p = preview(&mut h, r#"new Map([["k",1],[{o:1},"v"]])"#);
    assert_eq!(p["properties"], json!([{"name":"size","type":"number","value":"2"}]));
    assert_eq!(
        p["entries"][0],
        json!({"key":{"type":"string","description":"k","overflow":false,"properties":[]},
               "value":{"type":"number","description":"1","overflow":false,"properties":[]}})
    );
    assert_eq!(p["entries"][1]["key"]["properties"], json!([{"name":"o","type":"number","value":"1"}]));
    // Chrome's descriptions for sized collections.
    assert_eq!(evaluate(&mut h, "new Map([[1, 2]])", json!({}))["description"], "Map(1)");
    assert_eq!(evaluate(&mut h, "new Uint8Array(3)", json!({}))["description"], "Uint8Array(3)");
    let p = preview(&mut h, r#"new Set([1,"a"])"#);
    assert_eq!(p["entries"][1], json!({"value":{"type":"string","description":"a","overflow":false,"properties":[]}}));
    let p = preview(&mut h, "new Number(5)");
    assert_eq!(p["properties"], json!([{"name":"[[PrimitiveValue]]","type":"number","value":"5"}]));
    let p = preview(&mut h, "Promise.resolve(42)");
    assert_eq!(
        p["properties"],
        json!([{"name":"[[PromiseState]]","type":"string","value":"fulfilled"},
               {"name":"[[PromiseResult]]","type":"number","value":"42"}])
    );
    // Without generatePreview, no preview.
    assert!(evaluate(&mut h, "({a:1})", json!({})).get("preview").is_none());
    // callFunctionOn too.
    let id = evaluate(&mut h, "({})", json!({}))["objectId"].clone();
    let r = h.page(
        "Runtime.callFunctionOn",
        json!({"functionDeclaration": "function () { return {z: 1}; }", "objectId": id, "generatePreview": true}),
    );
    assert_eq!(
        r["result"]["result"]["preview"]["properties"],
        json!([{"name":"z","type":"number","value":"1"}]),
        "{r}"
    );
}

#[test]
fn get_properties_reports_symbols_accessors_and_internal_slots() {
    let mut h = Harness::new();
    let object = |h: &mut Harness, e: &str| evaluate(h, e, json!({}))["objectId"].clone();
    let id = object(&mut h, r#"({a:1, [Symbol("s")]: 3, get z(){return 1}})"#);
    let r = h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": true}));
    let result = r["result"]["result"].as_array().unwrap_or_else(|| panic!("{r}"));
    let sym = result.iter().find(|p| p["name"] == "Symbol(s)").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(sym["symbol"]["type"], "symbol", "{sym}");
    assert_eq!(sym["value"]["value"], 3);
    let z = result.iter().find(|p| p["name"] == "z").unwrap();
    assert_eq!(z["get"]["type"], "function");
    assert_eq!(z["set"], json!({"type":"undefined"}), "the missing setter is reported");
    let internal = r["result"]["internalProperties"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(internal[0]["name"], "[[Prototype]]");
    assert_eq!(internal[0]["value"]["className"], "Object");

    let r =
        h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": true, "accessorPropertiesOnly": true}));
    let names: Vec<_> = r["result"]["result"].as_array().unwrap().iter().map(|p| p["name"].clone()).collect();
    assert_eq!(names, [json!("z")], "{r}");
    assert!(r["result"].get("internalProperties").is_none(), "{r}");

    let internals = |h: &mut Harness, e: &str| {
        let id = object(h, e);
        let r = h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": true}));
        r["result"]["internalProperties"]
            .as_array()
            .unwrap_or_else(|| panic!("{r}"))
            .iter()
            .map(|p| (p["name"].as_str().unwrap().to_owned(), p["value"].clone()))
            .collect::<Vec<_>>()
    };
    let n = internals(&mut h, "new Number(5)");
    assert_eq!(n[1], ("[[PrimitiveValue]]".to_owned(), json!({"type":"number","value":5,"description":"5"})));
    let p = internals(&mut h, "Promise.resolve(7)");
    assert_eq!(p[1].0, "[[PromiseState]]");
    assert_eq!(p[1].1["value"], "fulfilled");
    assert_eq!(p[2].0, "[[PromiseResult]]");
    assert_eq!(p[2].1["value"], 7);
    let p = internals(&mut h, "new Promise(() => {})");
    assert_eq!(p[1].1["value"], "pending");
    assert_eq!(p.len(), 2, "no result while pending: {p:?}");
    let m = internals(&mut h, r#"new Map([["k",1]])"#);
    assert_eq!(m[1].0, "[[Entries]]");
    assert_eq!(m[1].1["subtype"], "array");
    let entries = h.page("Runtime.getProperties", json!({"objectId": m[1].1["objectId"], "ownProperties": true}));
    let first = entries["result"]["result"].as_array().unwrap().iter().find(|p| p["name"] == "0").cloned().unwrap();
    let entry = h.page("Runtime.getProperties", json!({"objectId": first["value"]["objectId"], "ownProperties": true}));
    let key = entry["result"]["result"].as_array().unwrap().iter().find(|p| p["name"] == "key").cloned().unwrap();
    assert_eq!(key["value"]["value"], "k", "{entry}");

    let proto = object(&mut h, "Array.prototype");
    let r = h.page("Runtime.queryObjects", json!({"prototypeObjectId": proto}));
    assert!(r["error"]["message"].as_str().unwrap().contains("heap iteration"), "{r}");
}

#[test]
fn console_calls_carry_previews_and_stacks() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let console = |h: &mut Harness, e: &str| {
        let events = h.page_with_events("Runtime.evaluate", json!({"expression": e}));
        events.into_iter().find(|m| m["method"] == "Runtime.consoleAPICalled").expect("console event")
    };
    let c = console(&mut h, "function f1() { console.log('hi', {a: 1}) } f1()");
    assert_eq!(c["params"]["stackTrace"]["callFrames"][0]["functionName"], "f1", "Runtime enabled is enough: {c}");
    assert_eq!(c["params"]["args"][1]["preview"]["properties"], json!([{"name":"a","type":"number","value":"1"}]));
    assert_eq!(h.page("Runtime.setAsyncCallStackDepth", json!({"maxDepth": 32}))["result"], json!({}));
    let c = console(&mut h, "function f2() { console.log('hi2') }\nf2()");
    let frames = c["params"]["stackTrace"]["callFrames"].as_array().unwrap_or_else(|| panic!("{c}"));
    let names: Vec<_> = frames.iter().map(|f| f["functionName"].clone()).collect();
    assert_eq!(names, [json!("f2"), json!("global code")], "the console method's own frame is left out: {c}");
    // A page script's frames carry its location (evaluated code has none in
    // JavaScriptCore).
    load_html(&mut h, "<script>\nfunction f3() { console.log('hi3') }\n</script>");
    let c = console(&mut h, "f3()");
    let top = &c["params"]["stackTrace"]["callFrames"][0];
    assert_eq!((top["functionName"].clone(), top["lineNumber"].clone()), (json!("f3"), json!(1)), "{c}");
    assert!(top["url"].as_str().unwrap().starts_with("data:"), "{c}");
}

#[test]
fn terminated_evaluations_answer_execution_was_terminated() {
    let mut h = Harness::new();
    // What the connection's watchdog check does when Runtime.terminateExecution arrives.
    let armed = Rc::new(std::cell::Cell::new(true));
    let check = armed.clone();
    h.browser.host().thread.browser.engine.set_interrupt_check(Some(Rc::new(move || check.get())));
    let r = h.page("Runtime.evaluate", json!({"expression": "for (;;) {}"}));
    armed.set(false);
    h.browser.host().thread.browser.engine.set_interrupt_check(None);
    assert_eq!(r["error"]["message"], "Execution was terminated", "{r}");
    assert_eq!(h.page("Runtime.terminateExecution", json!({}))["result"], json!({}));
    assert_eq!(eval_value(&mut h, "1 + 1"), json!(2), "the engine runs again");
}

/// A scripted connection for the dialog's nested read loop: offers each
/// message in turn, recording what the dialog holds for later.
struct ScriptedPump {
    script: Rc<RefCell<VecDeque<String>>>,
    held: Rc<RefCell<Vec<String>>>,
}

impl ScriptedPump {
    fn install(h: &mut Harness) -> ScriptedPump {
        let script: Rc<RefCell<VecDeque<String>>> = Rc::default();
        let held: Rc<RefCell<Vec<String>>> = Rc::default();
        let (s, k) = (script.clone(), held.clone());
        h.browser.host().set_dialog_pump(Some(Rc::new(move |take: &mut dyn FnMut(&str) -> PumpStep| loop {
            let Some(text) = s.borrow_mut().pop_front() else { return false };
            let step = take(&text);
            if step.hold {
                k.borrow_mut().push(text);
            }
            if step.done {
                return true;
            }
        })));
        ScriptedPump { script, held }
    }

    fn push(&self, h: &Harness, id: u64, method: &str, params: Value) {
        let message = json!({"id": id, "method": method, "params": params, "sessionId": h.session});
        self.script.borrow_mut().push_back(message.to_string());
    }
}

#[test]
fn dialogs_block_until_answered_when_page_is_enabled() {
    let mut h = Harness::new();
    let pump = ScriptedPump::install(&mut h);
    // Without Page enabled dialogs still do not block (the pump is unused).
    assert_eq!(eval_value(&mut h, "confirm('x')"), json!(false));
    h.page("Page.enable", json!({}));

    pump.push(&h, 900, "Runtime.evaluate", json!({"expression": "1"}));
    pump.push(&h, 901, "Page.handleJavaScriptDialog", json!({"accept": true, "promptText": "typed"}));
    let messages =
        h.page_with_events("Runtime.evaluate", json!({"expression": "prompt('name', 'def')", "returnByValue": true}));
    let reply = messages.iter().find(|m| m["id"].as_u64() == Some(h.next_id - 1)).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "typed", "{messages:#?}");
    let opening = messages.iter().find(|m| m["method"] == "Page.javascriptDialogOpening").unwrap();
    assert_eq!(opening["params"]["type"], "prompt");
    assert_eq!(opening["params"]["defaultPrompt"], "def");
    assert_eq!(opening["params"]["hasBrowserHandler"], true);
    assert_eq!(opening["params"]["frameId"], h.frame_id.as_str());
    assert!(messages.iter().any(|m| m["id"] == 901 && m["result"] == json!({})), "handled: {messages:#?}");
    let closed = messages.iter().find(|m| m["method"] == "Page.javascriptDialogClosed").unwrap();
    assert_eq!(closed["params"], json!({"frameId": h.frame_id, "result": true, "userInput": "typed"}));
    // The evaluation sent while the dialog showed waited for it.
    assert_eq!(pump.held.borrow().len(), 1);
    assert!(pump.held.borrow()[0].contains("\"id\":900"));

    pump.push(&h, 902, "Page.handleJavaScriptDialog", json!({"accept": false}));
    assert_eq!(eval_value(&mut h, "confirm('sure?')"), json!(false));
    pump.push(&h, 903, "Page.handleJavaScriptDialog", json!({"accept": true}));
    assert_eq!(eval_value(&mut h, "[confirm('sure?'), 'after']"), json!([true, "after"]));
    pump.push(&h, 904, "Page.handleJavaScriptDialog", json!({"accept": true}));
    assert_eq!(eval_value(&mut h, "prompt('q')"), json!(""), "accepted without text: empty, as Chrome");
    // The connection ending dismisses.
    assert_eq!(eval_value(&mut h, "prompt('q')"), Value::Null);
    // No dialog showing: answered as before.
    let r = h.page("Page.handleJavaScriptDialog", json!({"accept": true}));
    assert_eq!(r["error"]["message"], "No dialog is showing", "{r}");
}

#[test]
fn driver_steps_keep_dialogs_non_blocking() {
    let mut h = Harness::new();
    h.browser.host().set_dialog_pump(Some(Rc::new(|_take: &mut dyn FnMut(&str) -> PumpStep| -> bool {
        panic!("an LP.run step must not block on a dialog")
    })));
    h.page("Page.enable", json!({}));
    let r = h.page("LP.run", json!({"steps": [{"eval": {"expression": "String(confirm('x'))", "as": "c"}}]}));
    assert!(r.get("error").is_none(), "{r}");
    assert!(r.to_string().contains("\"false\""), "{r}");
}

#[test]
fn set_document_content_replaces_the_frame_document() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    let frame = h.frame_id.clone();
    let r = h.page(
        "Page.setDocumentContent",
        json!({"frameId": frame, "html": "<title>T</title><p id=x>hello</p><script>window.ran = 1</script>"}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    h.run_for(30);
    assert_eq!(
        eval_value(&mut h, "[document.title, document.getElementById('x').textContent, window.ran, location.href]"),
        json!(["T", "hello", 1, "about:blank"])
    );
    let r = h.page("Page.setDocumentContent", json!({"frameId": "nope", "html": ""}));
    assert_eq!(r["error"]["message"], "No frame for given id found", "{r}");
}

fn closed_target(messages: &[Value], target: &str) -> bool {
    messages.iter().any(|m| m["method"] == "Target.detachedFromTarget" && m["params"]["targetId"] == target)
}

#[test]
fn page_close_runs_beforeunload_and_honours_the_dialog() {
    let mut h = Harness::new();
    let pump = ScriptedPump::install(&mut h);
    h.page("Page.enable", json!({}));
    eval_value(
        &mut h,
        "addEventListener('beforeunload', e => { window.asked = (window.asked || 0) + 1; e.preventDefault(); e.returnValue = 'x'; }); 0",
    );
    pump.push(&h, 950, "Page.handleJavaScriptDialog", json!({"accept": false}));
    let messages = h.page_with_events("Page.close", json!({}));
    let opening = messages.iter().find(|m| m["method"] == "Page.javascriptDialogOpening").expect("dialog");
    assert_eq!(opening["params"]["type"], "beforeunload");
    assert_eq!(opening["params"]["message"], "");
    let target = h.frame_id.clone();
    assert!(!closed_target(&messages, &target), "dismissed: the page stays {messages:#?}");
    assert_eq!(eval_value(&mut h, "window.asked"), json!(1));

    pump.push(&h, 951, "Page.handleJavaScriptDialog", json!({"accept": true}));
    let mut messages = h.page_with_events("Page.close", json!({}));
    messages.extend(h.run_for(20));
    assert!(closed_target(&messages, &target), "accepted: closed {messages:#?}");
}

#[test]
fn page_close_without_a_cancelling_handler_closes() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    eval_value(&mut h, "addEventListener('beforeunload', () => { window.ran = 1 }); 0");
    let target = h.frame_id.clone();
    let mut messages = h.page_with_events("Page.close", json!({}));
    messages.extend(h.run_for(20));
    assert!(closed_target(&messages, &target), "{messages:#?}");
    assert!(!messages.iter().any(|m| m["method"] == "Page.javascriptDialogOpening"));
}

#[cfg(feature = "render")]
#[test]
fn transforms_are_css_px_at_device_scale_two() {
    let mut h = Harness::new();
    let r = h.page(
        "Emulation.setDeviceMetricsOverride",
        json!({"width": 800, "height": 600, "deviceScaleFactor": 2, "mobile": false}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    load_html(
        &mut h,
        "<style>body{margin:0}</style><div id=d style='position:absolute;left:20px;top:5px;width:50px;height:10px;transform:translate(10px,4px) rotate(0.0001deg)'></div><div id=e style='position:absolute;left:20px;top:50px;width:50px;height:10px;transform:translate(10px,4px)'></div>",
    );
    for sel in ["#d", "#e"] {
        let id = node_object(&mut h, sel);
        let q = h.page("DOM.getContentQuads", json!({"objectId": id}));
        let x = q["result"]["quads"][0][0].as_f64().unwrap_or_else(|| panic!("{q}"));
        assert!((x - 30.0).abs() < 0.01, "{sel}: translate is CSS px: {q}");
    }
    assert_eq!(eval_value(&mut h, "document.getElementById('e').getBoundingClientRect().x"), json!(30));
}

#[test]
fn set_document_content_keeps_the_document_and_reports_init_with_its_loader() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Page.setLifecycleEventsEnabled", json!({"enabled": true}));
    h.page("Runtime.enable", json!({}));
    h.page("Page.addScriptToEvaluateOnNewDocument", json!({"source": "window.inits = (window.inits || 0) + 1"}));
    load_html(&mut h, "<p>old</p><script>addEventListener('click', () => window.clicked = 1)</script>");
    let tree = h.page("Page.getFrameTree", json!({}));
    let loader_id = tree["result"]["frameTree"]["frame"]["loaderId"].clone();
    let frame = h.frame_id.clone();
    let mut events =
        h.page_with_events("Page.setDocumentContent", json!({"frameId": frame, "html": "<p id=n>new</p>"}));
    events.extend(h.run_for(100));
    let names = methods(&events);
    for absent in [
        "Page.frameNavigated",
        "Runtime.executionContextsCleared",
        "Runtime.executionContextCreated",
        "Page.frameStartedLoading",
    ] {
        assert!(!names.iter().any(|n| n == absent), "{absent}: {names:?}");
    }
    let lifecycle: Vec<_> = events
        .iter()
        .filter(|m| m["method"] == "Page.lifecycleEvent")
        .map(|m| (m["params"]["name"].as_str().unwrap().to_owned(), m["params"]["loaderId"].clone()))
        .collect();
    assert_eq!(lifecycle.first().map(|l| l.0.as_str()), Some("init"), "{lifecycle:?}");
    assert!(lifecycle.iter().all(|l| l.1 == loader_id), "same loader: {lifecycle:?} vs {loader_id}");
    assert!(lifecycle.iter().any(|l| l.0 == "load"), "{lifecycle:?}");
    assert_eq!(
        eval_value(
            &mut h,
            "document.body.click(); [document.getElementById('n').textContent, window.inits, window.clicked ?? null]"
        ),
        json!(["new", 1, null]),
        "listeners erased, init scripts not rerun"
    );
}

#[test]
fn page_close_answers_before_beforeunload_runs() {
    let mut h = Harness::new();
    h.page("Page.enable", json!({}));
    h.page("Runtime.enable", json!({}));
    eval_value(&mut h, "addEventListener('beforeunload', () => console.log('unloading')); 0");
    let id = h.next_id;
    let mut messages = h.page_with_events("Page.close", json!({}));
    messages.extend(h.run_for(20));
    let reply = messages.iter().position(|m| m["id"] == id).expect("reply");
    let console = messages.iter().position(|m| m["method"] == "Runtime.consoleAPICalled").expect("handler ran");
    assert!(reply < console, "{messages:#?}");
}

#[test]
fn console_stacks_come_with_runtime_enabled_and_page_disable_dismisses_a_dialog() {
    let mut h = Harness::new();
    h.page("Runtime.enable", json!({}));
    let events = h.page_with_events("Runtime.evaluate", json!({"expression": "function g() { console.log(1) } g()"}));
    let c = events.iter().find(|m| m["method"] == "Runtime.consoleAPICalled").unwrap();
    assert_eq!(c["params"]["stackTrace"]["callFrames"][0]["functionName"], "g", "{c}");

    let pump = ScriptedPump::install(&mut h);
    h.page("Page.enable", json!({}));
    pump.push(&h, 960, "Page.disable", json!({}));
    let messages = h.page_with_events("Runtime.evaluate", json!({"expression": "prompt('x')", "returnByValue": true}));
    let closed = messages.iter().find(|m| m["method"] == "Page.javascriptDialogClosed").expect("closed");
    assert_eq!(closed["params"]["result"], false);
    let reply = messages.iter().find(|m| m["id"].as_u64() == Some(h.next_id - 1)).unwrap();
    assert_eq!(reply["result"]["result"]["value"], Value::Null, "{reply}");
    assert!(pump.held.borrow()[0].contains("Page.disable"), "Page.disable runs after the dialog");
    let r = h.page("Page.handleJavaScriptDialog", json!({"accept": true}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
}

#[test]
fn get_properties_flags_and_preview_accessors() {
    let mut h = Harness::new();
    let object = |h: &mut Harness, e: &str| evaluate(h, e, json!({}))["objectId"].clone();
    let id = object(&mut h, "({a: 1})");
    let r = h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": false}));
    assert_eq!(r["result"]["internalProperties"][0]["name"], "[[Prototype]]", "{r}");
    let id = object(&mut h, "Object.create(null)");
    let r = h.page("Runtime.getProperties", json!({"objectId": id, "ownProperties": true}));
    assert_eq!(r["result"]["internalProperties"][0]["value"], json!({"type":"object","subtype":"null","value":null}));
    let id = object(&mut h, "[1, 2]");
    let r = h.page(
        "Runtime.getProperties",
        json!({"objectId": id, "ownProperties": true, "nonIndexedPropertiesOnly": true}),
    );
    let names: Vec<_> = r["result"]["result"].as_array().unwrap().iter().map(|p| p["name"].clone()).collect();
    assert_eq!(names, [json!("length")], "{r}");
    let p =
        evaluate(&mut h, "({get x() { throw 1 }, y: undefined})", json!({"generatePreview": true}))["preview"].clone();
    assert_eq!(
        p["properties"],
        json!([{"name":"x","type":"accessor"},{"name":"y","type":"undefined","value":"undefined"}]),
        "{p}"
    );
    assert!(evaluate(&mut h, "({a: 1})", json!({"generatePreview": true, "returnByValue": true}))
        .get("preview")
        .is_none());
    let r = h.page("Runtime.queryObjects", json!({"prototypeObjectId": "nope"}));
    assert_eq!(r["error"]["message"], "Could not find object with given id", "{r}");
}
