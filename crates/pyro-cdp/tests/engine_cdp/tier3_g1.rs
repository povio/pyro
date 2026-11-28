//! Client hints (navigator.userAgentData), permissions and the clipboard,
//! touch input and the virtual time policy. Expected values are measured on
//! headless Chrome 156 driven by Playwright 1.59.1.
use super::*;

fn context_id(h: &mut Harness) -> String {
    let target = h.frame_id.clone();
    let info = h.call(None, "Target.getTargetInfo", json!({ "targetId": target }));
    info["result"]["targetInfo"]["browserContextId"].as_str().unwrap_or_else(|| panic!("{info}")).to_owned()
}

/// Evaluates an awaited expression and returns its value (exceptions fail).
fn eval_await(h: &mut Harness, expression: &str) -> Value {
    let r = h.page("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true}));
    assert!(r["result"].get("exceptionDetails").is_none() && r.get("error").is_none(), "{expression}: {r}");
    r["result"]["result"]["value"].clone()
}

// ----- client hints ------------------------------------------------------------

#[test]
fn user_agent_data_follows_override_metadata_like_chrome() {
    let mut h = Harness::new();
    // pyro's own agent names no Chrome version: no brands.
    let shape = eval_value(
        &mut h,
        "(() => { const d = navigator.userAgentData; return [Object.prototype.toString.call(d), Object.keys(d), \
         Object.isFrozen(d.brands), d.brands === d.brands, d.mobile, typeof d.platform, JSON.stringify(d) === JSON.stringify(d.toJSON()),          d === navigator.userAgentData, navigator.clipboard === navigator.clipboard, navigator.permissions === navigator.permissions]; })()",
    );
    assert_eq!(shape, json!(["[object NavigatorUAData]", [], true, false, false, "string", true, true, true, true]));
    let metadata = json!({
        "brands": [{"brand": "X", "version": "1"}],
        "fullVersionList": [{"brand": "X", "version": "1.2.3"}],
        "platform": "Android", "platformVersion": "14", "architecture": "", "model": "Pixel", "mobile": true
    });
    let r = h.page(
        "Emulation.setUserAgentOverride",
        json!({"userAgent": "Mozilla/5.0 Custom", "userAgentMetadata": metadata}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(
        eval_value(&mut h, "JSON.stringify(navigator.userAgentData)"),
        json!(r#"{"brands":[{"brand":"X","version":"1"}],"mobile":true,"platform":"Android"}"#)
    );
    // Sorted keys; left-out optional fields take the browser's values
    // (Chrome: bitness "64", formFactors ["Desktop"]).
    let high = eval_await(
        &mut h,
        "navigator.userAgentData.getHighEntropyValues(['wow64','model','architecture','bitness','formFactors','fullVersionList','platformVersion','bogus'])\
         .then(v => [Object.keys(v), v.model, v.platformVersion, v.bitness, v.formFactors, v.fullVersionList, v.wow64, Object.isFrozen(v.fullVersionList)])",
    );
    assert_eq!(
        high,
        json!([
            ["architecture", "bitness", "brands", "formFactors", "fullVersionList", "mobile", "model", "platform", "platformVersion", "wow64"],
            "Pixel", "14", "64", ["Desktop"], [{"brand": "X", "version": "1.2.3"}], false, false
        ])
    );
    // Kept across navigation, in every world.
    assert!(h.page("Page.navigate", json!({"url": "data:text/html,<p>hints</p>"}))["error"].is_null());
    assert_eq!(eval_value(&mut h, "navigator.userAgentData.platform"), "Android");
    // Missing required members are invalid params and change nothing.
    for bad in [
        json!({"platform": "x"}),
        json!({"brands": [{"brand": "a"}], "platform": "", "platformVersion": "", "architecture": "", "model": "", "mobile": false}),
    ] {
        let r = h.page("Emulation.setUserAgentOverride", json!({"userAgent": "x", "userAgentMetadata": bad}));
        assert_eq!(r["error"]["code"], -32602, "{r}");
    }
    assert_eq!(eval_value(&mut h, "navigator.userAgentData.platform"), "Android");
    // An overridden agent without metadata reports no hints at all, even
    // one naming a Chrome version (Chrome does the same).
    h.page(
        "Emulation.setUserAgentOverride",
        json!({"userAgent": "Mozilla/5.0 (Macintosh) Chrome/120.0.1.2 Safari/537.36"}),
    );
    assert_eq!(
        eval_await(&mut h, "navigator.userAgentData.getHighEntropyValues(['architecture','bitness','formFactors','fullVersionList','uaFullVersion']).then(v => JSON.stringify(v))"),
        json!(r#"{"architecture":"","bitness":"","brands":[],"formFactors":[],"fullVersionList":[],"mobile":false,"platform":"","uaFullVersion":""}"#)
    );
    let errors = eval_await(
        &mut h,
        "Promise.all([navigator.userAgentData.getHighEntropyValues(), navigator.userAgentData.getHighEntropyValues('x')].map(p => p.then(() => 'ok', e => e.name + ': ' + e.message)))",
    );
    assert_eq!(
        errors,
        json!([
            "TypeError: Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': 1 argument required, but only 0 present.",
            "TypeError: Failed to execute 'getHighEntropyValues' on 'NavigatorUAData': The provided value cannot be converted to a sequence."
        ])
    );
    // Clearing the override restores the browser's own (empty) brands.
    h.page("Emulation.setUserAgentOverride", json!({"userAgent": ""}));
    assert_eq!(eval_value(&mut h, "navigator.userAgentData.brands.length"), 0);
}

/// Accepts one HTTP request, replies with a small page and returns the
/// request head, lowercased.
fn serve_capturing() -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
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
            assert!(n > 0);
            request.extend_from_slice(&chunk[..n]);
        }
        let body = "<!doctype html><p>hints</p>";
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        String::from_utf8_lossy(&request).to_ascii_lowercase()
    });
    (address, server)
}

#[test]
fn metadata_overrides_send_low_entropy_client_hints_to_trustworthy_origins() {
    let mut h = Harness::new();
    let metadata = json!({"brands": [{"brand": "X", "version": "1"}], "platform": "Android", "platformVersion": "14", "architecture": "", "model": "", "mobile": true});
    let control =
        json!({"platform": "a\u{1}", "platformVersion": "", "architecture": "", "model": "", "mobile": false});
    for (params, message) in [
        (
            json!({"userAgent": "", "userAgentMetadata": metadata}),
            "Empty userAgent invalid with userAgentMetadata provided",
        ),
        (json!({"userAgent": "x", "userAgentMetadata": control}), "Invalid platform string"),
    ] {
        let r = h.page("Emulation.setUserAgentOverride", params);
        assert_eq!(r["error"]["code"], -32602, "{r}");
        assert_eq!(r["error"]["message"], message, "{r}");
    }
    h.page("Emulation.setUserAgentOverride", json!({"userAgent": "Hinted/1", "userAgentMetadata": metadata}));
    let (addr, server) = serve_capturing();
    assert!(h.page("Page.navigate", json!({"url": format!("http://{addr}/")}))["error"].is_null());
    let head = server.join().unwrap();
    assert!(head.contains("\r\nsec-ch-ua: \"x\";v=\"1\"\r\n"), "{head}");
    assert!(head.contains("\r\nsec-ch-ua-mobile: ?1\r\n"), "{head}");
    assert!(head.contains("\r\nsec-ch-ua-platform: \"android\"\r\n"), "{head}");
    // An agent overridden without metadata sends none (Chrome).
    h.page("Emulation.setUserAgentOverride", json!({"userAgent": "Plain/1"}));
    let (addr, server) = serve_capturing();
    assert!(h.page("Page.navigate", json!({"url": format!("http://{addr}/")}))["error"].is_null());
    let head = server.join().unwrap();
    assert!(!head.contains("sec-ch-ua"), "{head}");
}

// ----- permissions and clipboard --------------------------------------------------

fn permission_states(h: &mut Harness) -> Value {
    eval_await(
        h,
        "Promise.all(['clipboard-read','clipboard-write','geolocation','notifications'].map(n => navigator.permissions.query({name: n}).then(s => s.state)))",
    )
}

#[test]
fn browser_permissions_drive_permissions_query_and_the_clipboard() {
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    // Chrome's defaults: writing granted, reading needs a prompt.
    assert_eq!(permission_states(&mut h), json!(["prompt", "granted", "prompt", "prompt"]));
    let denied =
        eval_await(&mut h, "navigator.clipboard.readText().then(v => 'ok:' + v, e => e.name + ': ' + e.message)");
    assert_eq!(denied, "NotAllowedError: Failed to execute 'readText' on 'Clipboard': Read permission denied.");
    assert_eq!(eval_await(&mut h, "navigator.clipboard.writeText('hello').then(v => String(v))"), "undefined");

    // Chrome keys the table by type: clipboardReadWrite alone reads but
    // denies writing (measured).
    h.call(None, "Browser.grantPermissions", json!({"permissions": ["clipboardReadWrite"], "browserContextId": ctx}));
    assert_eq!(permission_states(&mut h), json!(["granted", "denied", "denied", "denied"]));
    let write = eval_await(&mut h, "navigator.clipboard.writeText('x').then(() => 'ok', e => e.message)");
    assert_eq!(write, "Failed to execute 'writeText' on 'Clipboard': Write permission denied.");
    assert_eq!(eval_await(&mut h, "navigator.clipboard.readText()"), "hello");

    // Playwright's grantPermissions(['clipboard-read', 'clipboard-write']).
    let r = h.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"], "browserContextId": ctx}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(
        permission_states(&mut h),
        json!(["granted", "granted", "denied", "denied"]),
        "unlisted ones are denied"
    );
    assert_eq!(eval_await(&mut h, "navigator.clipboard.readText()"), "hello");
    assert_eq!(
        eval_await(&mut h, "navigator.clipboard.writeText('h\u{e9}llo').then(() => navigator.clipboard.readText())"),
        "h\u{e9}llo"
    );
    let items = eval_await(
        &mut h,
        "navigator.clipboard.write([new ClipboardItem({'text/plain': new Blob(['a'], {type: 'text/plain'}), 'text/html': '<b>x</b>'})])\
         .then(() => navigator.clipboard.read()).then(async items => [items.length, items[0].types, await (await items[0].getType('text/html')).text(), \
         await navigator.clipboard.readText(), Object.prototype.toString.call(items[0]), items[0].presentationStyle === undefined])",
    );
    assert_eq!(items, json!([1, ["text/plain", "text/html"], "<b>x</b>", "a", "[object ClipboardItem]", true]));
    let rejected = eval_await(
        &mut h,
        "navigator.clipboard.write([new ClipboardItem({'application/x-foo': new Blob(['a'])})]).then(() => 'ok', e => e.name + ': ' + e.message)",
    );
    assert_eq!(
        rejected,
        "NotAllowedError: Failed to execute 'write' on 'Clipboard': Type application/x-foo not supported on write."
    );
    let ctor = eval_value(
        &mut h,
        "[() => new ClipboardItem(), () => new ClipboardItem({}), () => new ClipboardItem(1)].map(f => { try { f(); return 'ok' } catch (e) { return e.message } })",
    );
    assert_eq!(
        ctor,
        json!([
            "Failed to construct 'ClipboardItem': 1 argument required, but only 0 present.",
            "Failed to construct 'ClipboardItem': Empty dictionary argument",
            "Failed to construct 'ClipboardItem': Only objects can be converted to record<K,V> types"
        ])
    );

    // A per-origin setting overrides the every-origin grant only there.
    let r = h.call(
        None,
        "Browser.setPermission",
        json!({"permission": {"name": "clipboard-write"}, "setting": "denied", "browserContextId": ctx}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    let write =
        eval_await(&mut h, "navigator.clipboard.writeText('x').then(() => 'ok', e => e.name + ': ' + e.message)");
    assert_eq!(write, "NotAllowedError: Failed to execute 'writeText' on 'Clipboard': Write permission denied.");

    h.call(None, "Browser.resetPermissions", json!({"browserContextId": ctx}));
    assert_eq!(permission_states(&mut h), json!(["prompt", "granted", "prompt", "prompt"]));

    // PermissionStatus as Chrome shapes it.
    let status = eval_await(
        &mut h,
        "navigator.permissions.query({name: 'geolocation'}).then(s => [Object.prototype.toString.call(s), s.name, Object.keys(s), \
         Object.getOwnPropertyNames(Object.getPrototypeOf(s)).sort(), s.onchange, s instanceof EventTarget])",
    );
    assert_eq!(
        status,
        json!([
            "[object PermissionStatus]",
            "geolocation",
            [],
            ["constructor", "name", "onchange", "state"],
            null,
            true
        ])
    );
    let errors = eval_await(
        &mut h,
        "Promise.all([undefined, {}, {name: 'bogus'}, {name: 'push'}].map(d => navigator.permissions.query(d).then(s => s.state, e => e.name + ': ' + e.message)))",
    );
    assert_eq!(
        errors,
        json!([
            "TypeError: Failed to execute 'query' on 'Permissions': parameter 1 is not of type 'object'.",
            "TypeError: Failed to execute 'query' on 'Permissions': Failed to read the 'name' property from 'PermissionDescriptor': Required member is undefined.",
            "TypeError: Failed to execute 'query' on 'Permissions': Failed to read the 'name' property from 'PermissionDescriptor': The provided value 'bogus' is not a valid enum value of type PermissionName.",
            "NotSupportedError: Failed to execute 'query' on 'Permissions': Push Permission without userVisibleOnly:true isn't supported yet."
        ])
    );
}

#[test]
fn browser_permission_commands_validate_like_chrome() {
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    for (method, params, message) in [
        ("Browser.grantPermissions", json!({"permissions": ["bogus"]}), "Unknown permission type: bogus"),
        (
            "Browser.grantPermissions",
            json!({"permissions": ["geolocation"], "origin": "not a url"}),
            "Permission can't be granted to opaque origins.",
        ),
        (
            "Browser.grantPermissions",
            json!({"permissions": ["geolocation"], "origin": "*"}),
            "Permission can't be granted to opaque origins.",
        ),
        (
            "Browser.setPermission",
            json!({"permission": {"name": "bogus"}, "setting": "granted"}),
            "Invalid PermissionDescriptor name: bogus",
        ),
        (
            "Browser.setPermission",
            json!({"permission": {"name": "geolocation"}, "setting": "bogus"}),
            "Unknown permission setting: bogus",
        ),
        (
            "Browser.setPermission",
            json!({"permission": {"name": "push"}, "setting": "granted"}),
            "Push Permission without userVisibleOnly:true isn't supported",
        ),
    ] {
        let mut params = params;
        params["browserContextId"] = json!(ctx);
        let r = h.call(None, method, params.clone());
        assert_eq!(r["error"]["code"], -32602, "{method} {params}: {r}");
        assert_eq!(r["error"]["message"], message, "{method} {params}");
    }
    // Accepted: a URL's origin (not this page's opaque one), types with no
    // queryable name, the default context.
    let r = h.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions": ["geolocation"], "origin": "https://a.example/path?q", "browserContextId": ctx}),
    );
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_await(&mut h, "navigator.permissions.query({name: 'geolocation'}).then(s => s.state)"), "prompt");
    for (method, params) in [
        ("Browser.grantPermissions", json!({"permissions": ["ar"], "browserContextId": ctx})),
        (
            "Browser.setPermission",
            json!({"permission": {"name": "push", "userVisibleOnly": true}, "setting": "granted", "browserContextId": ctx}),
        ),
        ("Browser.resetPermissions", json!({})),
    ] {
        assert_eq!(h.call(None, method, params.clone())["result"], json!({}), "{method} {params}");
    }
    // A grant for every origin listing nothing queryable denies the rest.
    assert_eq!(eval_await(&mut h, "navigator.permissions.query({name: 'geolocation'}).then(s => s.state)"), "denied");
}

#[test]
fn origin_grants_apply_to_that_origin_only() {
    let (addr, server) = serve_html(1, "<!doctype html><p>origin</p>");
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    assert!(h.page("Page.navigate", json!({"url": format!("http://{addr}/")}))["error"].is_null());
    let origin = format!("http://{addr}");
    h.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions": ["geolocation"], "origin": origin, "browserContextId": ctx}),
    );
    h.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions": ["notifications"], "origin": "https://other.example", "browserContextId": ctx}),
    );
    assert_eq!(permission_states(&mut h), json!(["denied", "denied", "granted", "denied"]));
    server.join().unwrap();
}

/// testdriver's set_permission sends Browser.setPermission on the page's session (no
/// browserContextId): it applies to the page's browser context, and the reply comes on that session.
#[test]
fn permission_commands_on_a_page_session_apply_to_its_context() {
    let mut h = Harness::new();
    let r = h.page("Browser.setPermission", json!({"permission": {"name": "notifications"}, "setting": "granted"}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_await(&mut h, "navigator.permissions.query({name: 'notifications'}).then(s => s.state)"), json!("granted"));
    let r = h.page("Browser.resetPermissions", json!({}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_await(&mut h, "navigator.permissions.query({name: 'notifications'}).then(s => s.state)"), json!("prompt"));
}

#[test]
fn permission_status_fires_change_when_its_state_flips() {
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    eval_await(
        &mut h,
        "(async () => { window.changes = []; const s = await navigator.permissions.query({name: 'geolocation'}); \
         s.onchange = e => changes.push('on:' + s.state + ':' + e.type); \
         (await navigator.permissions.query({name: 'geolocation'})).addEventListener('change', e => changes.push('listener:' + e.target.state)); })()",
    );
    // The page keeps no reference to the statuses: they are still notified.
    let settle = "new Promise(r => setTimeout(r, 0)).then(() => changes.splice(0))";
    h.call(None, "Browser.grantPermissions", json!({"permissions": ["clipboardReadWrite"], "browserContextId": ctx}));
    assert_eq!(eval_await(&mut h, settle), json!(["on:denied:change", "listener:denied"]));
    // Still denied: no event.
    h.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions": ["clipboardSanitizedWrite"], "browserContextId": ctx}),
    );
    assert_eq!(eval_await(&mut h, settle), json!([]));
    h.call(None, "Browser.grantPermissions", json!({"permissions": ["geolocation"], "browserContextId": ctx}));
    assert_eq!(eval_await(&mut h, settle), json!(["on:granted:change", "listener:granted"]));
    h.call(None, "Browser.resetPermissions", json!({"browserContextId": ctx}));
    assert_eq!(eval_await(&mut h, settle), json!(["on:prompt:change", "listener:prompt"]));
    assert_eq!(
        eval_await(&mut h, "navigator.permissions.query().then(() => 'ok', e => e.message)"),
        "Failed to execute 'query' on 'Permissions': 1 argument required, but only 0 present."
    );
}

#[test]
fn notification_follows_the_notifications_permission() {
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    let probe = "(async () => { const n = new Notification('hi', {body: 'b', tag: 't', data: {x: 1}}); \
         const ev = await new Promise(r => { n.onshow = () => r('show'); n.onerror = () => r('error'); }); \
         return [Notification.permission, await Notification.requestPermission(), ev, n.title, n.body, n.tag, n.data.x, \
           n.dir, n.silent, n instanceof EventTarget, Object.prototype.toString.call(n), Notification.maxActions]; })()";
    assert_eq!(
        eval_await(&mut h, probe),
        json!(["default", "default", "error", "hi", "b", "t", 1, "auto", null, true, "[object Notification]", 2])
    );
    h.call(None, "Browser.grantPermissions", json!({"permissions": ["notifications"], "browserContextId": ctx}));
    let granted = eval_await(&mut h, probe);
    assert_eq!(json!([granted[0], granted[1], granted[2]]), json!(["granted", "granted", "show"]));
    // A shown notification reports close.
    assert_eq!(
        eval_await(
            &mut h,
            "new Promise(r => { const n = new Notification('x'); n.onshow = () => n.close(); n.onclose = () => r('closed'); })"
        ),
        "closed"
    );
    h.call(None, "Browser.resetPermissions", json!({"browserContextId": ctx}));
    h.call(
        None,
        "Browser.setPermission",
        json!({"permission": {"name": "notifications"}, "setting": "denied", "browserContextId": ctx}),
    );
    assert_eq!(eval_await(&mut h, "Notification.permission"), "denied");
    assert_eq!(
        eval_await(
            &mut h,
            "[() => new Notification(), () => Notification('x'), () => new Notification('x', {actions: [{action: 'a', title: 'a'}]}), \
              () => new Notification('x', {silent: true, vibrate: [1]}), () => new Notification('x', {dir: 'up'})] \
              .map(f => { try { f(); return 'ok' } catch (e) { return e.name } })"
        ),
        json!(["TypeError", "TypeError", "TypeError", "TypeError", "TypeError"])
    );
}

// ----- touch ---------------------------------------------------------------------

// (The page's listeners are on the document, where touchstart/touchmove listeners are passive by
// default as in Chrome, so Chrome sends those two events non-cancelable: `cancelable` is false.)
const TOUCH_PAGE: &str = r#"(() => {
  document.body.style.margin = '0';
  document.body.innerHTML = '<div id=a style="width:100px;height:100px"></div><div id=b style="width:100px;height:100px"></div>';
  window.log = [];
  const types = ['pointerover','pointerenter','pointerdown','pointermove','pointerup','pointercancel','pointerout','pointerleave',
    'gotpointercapture','lostpointercapture','touchstart','touchmove','touchend','touchcancel','mouseover','mouseenter','mousedown',
    'mousemove','mouseup','mouseout','mouseleave','click'];
  for (const t of types) document.addEventListener(t, e => {
    const id = e.target.id || e.target.nodeName;
    let s = t + '@' + id;
    if (e instanceof PointerEvent && e.pointerType === 'touch') s += ' ' + [e.pointerId > 1 ? 'T' : e.pointerId, e.isPrimary, e.width, e.height, e.pressure, e.button, e.buttons, e.clientX, e.clientY].join();
    if (e instanceof TouchEvent) s += ' ' + [e.touches.length, e.targetTouches.length, e.changedTouches.length, e.cancelable, e.isTrusted, e.composed].join() + ' ' + JSON.stringify([...e.changedTouches].map(t => [t.identifier, t.clientX, t.clientY, t.radiusX, t.force, t.target.id]));
    if (t === 'click' || t === 'mousedown') s += ' ' + e.detail;
    log.push(s);
  }, true);
})()"#;

fn touch(h: &mut Harness, kind: &str, points: Value) -> Value {
    h.page("Input.dispatchTouchEvent", json!({"type": kind, "touchPoints": points}))
}

fn take_log(h: &mut Harness) -> Vec<String> {
    serde_json::from_value(eval_value(h, "log.splice(0)")).unwrap()
}

#[test]
fn touch_emulation_sets_max_touch_points_and_coarse_pointer() {
    let mut h = Harness::new();
    let probe =
        "[navigator.maxTouchPoints, matchMedia('(pointer: coarse)').matches, matchMedia('(hover: none)').matches]";
    assert_eq!(eval_value(&mut h, probe), json!([0, false, false]));
    assert_eq!(h.page("Emulation.setTouchEmulationEnabled", json!({"enabled": true}))["result"], json!({}));
    assert_eq!(eval_value(&mut h, probe), json!([1, true, true]));
    for points in [0, 17] {
        let r = h.page("Emulation.setTouchEmulationEnabled", json!({"enabled": true, "maxTouchPoints": points}));
        assert_eq!(r["error"]["message"], "Touch points must be between 1 and 16", "{r}");
    }
    h.page("Emulation.setTouchEmulationEnabled", json!({"enabled": true, "maxTouchPoints": 5}));
    assert!(h.page("Page.navigate", json!({"url": "data:text/html,<p>touch</p>"}))["error"].is_null());
    assert_eq!(eval_value(&mut h, probe), json!([5, true, true]), "kept across navigation");
    h.page("Emulation.setTouchEmulationEnabled", json!({"enabled": false}));
    assert_eq!(eval_value(&mut h, probe), json!([0, false, false]));
}

#[test]
fn script_execution_disabled_applies_to_the_next_document_and_its_frames() {
    let mut h = Harness::new();
    let page = r#"<!doctype html><body onload="window.loaded = 1"><script>window.ran = 1</script>
<noscript><p id="n">no script</p></noscript>
<iframe srcdoc="<script>parent.inner = 1</script><noscript><b>inner noscript</b></noscript>"></iframe>"#;
    let probe = "[typeof ran, typeof loaded, !!document.getElementById('n'), typeof inner, \
                 !!document.querySelector('iframe').contentDocument.querySelector('b')]";
    let r = h.page("Emulation.setScriptExecutionDisabled", json!({"value": true}));
    assert_eq!(r["result"], json!({}), "{r}");
    load_html(&mut h, page);
    // Protocol evaluation still runs; the page's own script does not.
    assert_eq!(eval_value(&mut h, probe), json!(["undefined", "undefined", true, "undefined", true]));
    let r = h.page("Emulation.setScriptExecutionDisabled", json!({"value": false}));
    assert_eq!(r["result"], json!({}), "{r}");
    load_html(&mut h, page);
    assert_eq!(eval_value(&mut h, probe), json!(["number", "number", false, "number", false]));
    let r = h.page("Emulation.setScriptExecutionDisabled", json!({}));
    assert!(r["error"]["message"].as_str().is_some(), "value is required: {r}");
}

#[test]
fn touch_tap_fires_chromium_pointer_touch_and_compat_mouse_sequence() {
    let mut h = Harness::new();
    h.page("Emulation.setTouchEmulationEnabled", json!({"enabled": true}));
    eval_value(&mut h, TOUCH_PAGE);
    // Playwright's tap: touchStart [{x, y}] then touchEnd [].
    assert_eq!(touch(&mut h, "touchStart", json!([{"x": 50, "y": 50}]))["result"], json!({}));
    assert_eq!(touch(&mut h, "touchEnd", json!([]))["result"], json!({}));
    let pressed = "T,true,2,2,1,0,1,50,50";
    let released = "T,true,1,1,0,0,0,50,50";
    let point = r#"[[0,50,50,1,1,"a"]]"#;
    let expected: Vec<String> = [
        format!("pointerover@a {pressed}"),
        format!("pointerenter@#document {pressed}"),
        format!("pointerenter@HTML {pressed}"),
        format!("pointerenter@BODY {pressed}"),
        format!("pointerenter@a {pressed}"),
        format!("pointerdown@a {pressed}"),
        format!("touchstart@a 1,1,1,false,true,true {point}"),
        format!("gotpointercapture@a {released}"),
        format!("pointerup@a {released}"),
        format!("lostpointercapture@a {released}"),
        format!("pointerout@a {released}"),
        format!("pointerleave@a {released}"),
        format!("pointerleave@BODY {released}"),
        format!("pointerleave@HTML {released}"),
        format!("pointerleave@#document {released}"),
        format!("touchend@a 0,0,1,true,true,true {point}"),
        "mouseover@a".into(),
        "mouseenter@#document".into(),
        "mouseenter@HTML".into(),
        "mouseenter@BODY".into(),
        "mouseenter@a".into(),
        "mousemove@a".into(),
        "mousedown@a 1".into(),
        "mouseup@a".into(),
        "click@a T,false,1,1,0,0,0,50,50 1".into(),
    ]
    .into();
    assert_eq!(take_log(&mut h), expected);
}

#[test]
fn touch_moves_cancel_the_tap_scroll_and_follow_ids() {
    let mut h = Harness::new();
    eval_value(&mut h, TOUCH_PAGE);
    let short = |log: Vec<String>| -> Vec<String> {
        log.into_iter()
            .filter(|l| {
                !l.starts_with("pointerenter") && !l.starts_with("pointerleave") && !l.starts_with("pointerover")
            })
            .map(|l| l.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    // Within the 15px slop: pointermoves but no touchmove, still a tap.
    touch(&mut h, "touchStart", json!([{"x": 10, "y": 10}]));
    touch(&mut h, "touchMove", json!([{"x": 18, "y": 10}]));
    touch(&mut h, "touchMove", json!([{"x": 23, "y": 10}]));
    touch(&mut h, "touchEnd", json!([]));
    let log = short(take_log(&mut h));
    assert!(!log.iter().any(|l| l.starts_with("touchmove")), "{log:?}");
    assert!(log.iter().any(|l| l.starts_with("click@a")), "{log:?}");
    // Past it: touchmove, then the scroll cancels the pointer; no click and
    // the remaining touch events are not cancelable.
    touch(&mut h, "touchStart", json!([{"x": 10, "y": 10, "radiusX": 3, "radiusY": 4, "force": 0.5, "id": 5}]));
    touch(&mut h, "touchMove", json!([{"x": 10, "y": 150, "id": 5}]));
    touch(&mut h, "touchMove", json!([{"x": 10, "y": 160, "id": 5}]));
    touch(&mut h, "touchEnd", json!([]));
    let log = take_log(&mut h);
    let types: Vec<&str> = log.iter().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(
        types,
        [
            "pointerover@a",
            "pointerenter@#document",
            "pointerenter@HTML",
            "pointerenter@BODY",
            "pointerenter@a",
            "pointerdown@a",
            "touchstart@a",
            "gotpointercapture@a",
            "pointermove@a",
            "touchmove@a",
            "pointercancel@a",
            "lostpointercapture@a",
            "pointerout@a",
            "pointerleave@a",
            "pointerleave@BODY",
            "pointerleave@HTML",
            "pointerleave@#document",
            "touchmove@a",
            "touchend@a"
        ],
        "{log:#?}"
    );
    assert!(log[5].ends_with("T,true,6,8,0.5,0,1,10,10"), "{}", log[5]);
    assert!(log[6].contains(r#"[[5,10,10,3,0.5,"a"]]"#), "{}", log[6]);
    assert!(log[10].ends_with("T,true,1,1,0,0,0,0,0"), "scroll cancel: {}", log[10]);
    assert!(log[17].contains(" 1,1,1,false,"), "touchmove during the scroll: {}", log[17]);
    assert!(log[18].contains(" 0,0,1,false,"), "{}", log[18]);
    // Two touches: the second is not primary; touchEnd lists the released
    // ids; no tap.
    touch(&mut h, "touchStart", json!([{"x": 10, "y": 10, "id": 7}, {"x": 10, "y": 150, "id": 8}]));
    touch(&mut h, "touchMove", json!([{"x": 12, "y": 10, "id": 7}, {"x": 12, "y": 150, "id": 8}]));
    touch(&mut h, "touchEnd", json!([{"x": 12, "y": 150, "id": 8}]));
    touch(&mut h, "touchEnd", json!([]));
    let log = take_log(&mut h);
    let touches: Vec<&String> = log.iter().filter(|l| l.starts_with("touch")).collect();
    assert_eq!(touches.len(), 6, "{log:#?}");
    assert!(touches[0].starts_with("touchstart@a 1,1,1,false"), "{touches:?}");
    assert!(touches[1].starts_with("touchstart@b 2,1,1,false") && touches[1].contains("[[8,"), "{touches:?}");
    assert!(touches[2].starts_with("touchmove@a 2,1,2,false"), "{touches:?}");
    assert!(touches[3].starts_with("touchmove@b 2,1,2,false"), "{touches:?}");
    assert!(touches[4].starts_with("touchend@b 1,0,1,true") && touches[4].contains("[[8,"), "{touches:?}");
    assert!(touches[5].starts_with("touchend@a 0,0,1,true") && touches[5].contains("[[7,"), "{touches:?}");
    assert!(log.iter().any(|l| l.starts_with("pointerdown@b T,false")), "{log:#?}");
    assert!(!log.iter().any(|l| l.starts_with("click")), "{log:#?}");
    // touchCancel cancels every touch.
    touch(&mut h, "touchStart", json!([{"x": 10, "y": 10}]));
    touch(&mut h, "touchCancel", json!([]));
    let log = short(take_log(&mut h));
    assert_eq!(
        log,
        [
            "pointerdown@a T,true,2,2,1,0,1,10,10",
            "touchstart@a 1,1,1,true,true,true",
            "gotpointercapture@a T,true,2,2,0,-1,0,10,10",
            "pointercancel@a T,true,2,2,0,-1,0,10,10",
            "lostpointercapture@a T,true,2,2,0,-1,0,10,10",
            "pointerout@a T,true,2,2,0,-1,0,10,10",
            "touchcancel@a 0,0,1,false,true,true"
        ]
    );
}

#[test]
fn canceled_touchstart_prevents_the_tap_and_touch_input_validates_like_chrome() {
    let mut h = Harness::new();
    eval_value(&mut h, TOUCH_PAGE);
    eval_value(
        &mut h,
        "document.getElementById('a').addEventListener('touchstart', e => e.preventDefault(), {once: true})",
    );
    touch(&mut h, "touchStart", json!([{"x": 10, "y": 10}]));
    touch(&mut h, "touchEnd", json!([]));
    let log = take_log(&mut h);
    assert!(log.last().unwrap().starts_with("touchend@a"), "no compat mouse events or click: {log:#?}");
    for (kind, points, message) in [
        ("bogus", json!([{"x": 1, "y": 1}]), "Unexpected event type 'bogus'"),
        ("touchStart", json!([]), "TouchStart and TouchMove must have at least one touch point."),
        ("touchMove", json!([]), "TouchStart and TouchMove must have at least one touch point."),
        ("touchEnd", json!([{"x": 1, "y": 1}]), "Must send a TouchStart first to start a new touch."),
        ("touchEnd", json!([]), "Must send a TouchStart first to start a new touch."),
        ("touchCancel", json!([]), "Must send a TouchStart first to start a new touch."),
        (
            "touchStart",
            json!([{"x": 1, "y": 1, "id": 1}, {"x": 2, "y": 2}]),
            "All or none of the provided TouchPoints must supply ids.",
        ),
        ("touchCancel", json!([{"x": 1, "y": 1}]), "TouchCancel must not have any touch points."),
        ("touchMove", json!([{"x": 1, "y": 1}]), "Must send a TouchStart first to start a new touch."),
        ("touchStart", json!([{"x": 1, "y": 1, "id": -1}]), "TouchPoint id '-1' is out of valid range."),
        ("touchStart", json!([{"x": 1, "y": 1, "force": 2}]), "'force' should be in the range of [0,1]"),
        ("touchStart", json!([{"x": 1, "y": 1, "twist": 400}]), "'twist' should be in the range of [0,359]"),
    ] {
        let r = touch(&mut h, kind, points.clone());
        assert_eq!(r["error"]["code"], -32602, "{kind} {points}: {r}");
        assert_eq!(r["error"]["message"], message, "{kind} {points}");
    }
    assert!(take_log(&mut h).is_empty(), "invalid commands dispatch nothing");
}

#[test]
fn touch_sequences_go_to_the_child_frame_under_the_first_touch() {
    let mut h = Harness::new();
    eval_value(
        &mut h,
        r#"(() => {
          document.body.style.margin = '0';
          window.log = [];
          const f = document.createElement('iframe');
          f.style.cssText = 'border:0;width:300px;height:200px;display:block';
          f.srcdoc = '<body style="margin:0"><div id=inner style="width:100px;height:100px"></div><script>for (const t of ["touchstart","touchend","click"]) document.addEventListener(t, e => parent.log.push(t + "@" + e.target.id + ":" + (e.changedTouches ? e.changedTouches[0].clientX : e.clientX)), true)</script>';
          document.body.appendChild(f);
          for (const t of ['touchstart','touchend','click']) document.addEventListener(t, e => log.push('top:' + t), true);
        })()"#,
    );
    h.run_for(200);
    touch(&mut h, "touchStart", json!([{"x": 20, "y": 30}]));
    touch(&mut h, "touchEnd", json!([]));
    assert_eq!(
        eval_value(&mut h, "log.splice(0)"),
        json!(["touchstart@inner:20", "touchend@inner:20", "click@inner:20"])
    );
}

// ----- virtual time ---------------------------------------------------------------

#[test]
fn virtual_time_pause_holds_the_clock_and_timers_until_a_budget_advances_them() {
    let mut h = Harness::new();
    eval_value(&mut h, "window.fired = []; setTimeout(() => fired.push(['t1000', performance.now()]), 1000); setTimeout(() => fired.push(['t5000', performance.now()]), 5000); 0");
    let r = h.page("Emulation.setVirtualTimePolicy", json!({"policy": "pause"}));
    let base = r["result"]["virtualTimeTicksBase"].as_f64().unwrap_or_else(|| panic!("{r}"));
    let paused = eval_value(&mut h, "[Date.now(), performance.now()]");
    h.run_for(150);
    assert_eq!(eval_value(&mut h, "[Date.now(), performance.now()]"), paused, "the page clock is frozen");
    // Zero-delay timers still run while paused, as in Chrome.
    assert_eq!(eval_await(&mut h, "new Promise(r => setTimeout(() => r(fired.length), 0))"), 0);

    let id = h.send(
        Some(&h.session.clone()),
        "Emulation.setVirtualTimePolicy",
        json!({"policy": "advance", "budget": 2000}),
    );
    let messages = h.until_response(id);
    let reply = messages.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(reply["result"]["virtualTimeTicksBase"].as_f64(), Some(base), "the base stays");
    h.until_event(|m| m["method"] == "Emulation.virtualTimeBudgetExpired");
    let now = eval_value(&mut h, "[performance.now(), fired.map(f => [f[0], Math.round(f[1])])]");
    let start = paused[1].as_f64().unwrap();
    assert!((now[0].as_f64().unwrap() - (start + 2000.0)).abs() < 1e-6, "exactly the budget passed: {now}");
    assert_eq!(now[1][0][0], "t1000", "{now}");
    assert_eq!(now[1][0][1].as_f64(), Some((start + 1000.0).round()), "the timer ran at its virtual time: {now}");
    assert_eq!(now[1].as_array().map(Vec::len), Some(1), "{now}");
    h.run_for(100);
    assert!(
        (eval_value(&mut h, "performance.now()").as_f64().unwrap() - (start + 2000.0)).abs() < 1e-6,
        "paused again after the budget"
    );

    h.page("Emulation.setVirtualTimePolicy", json!({"policy": "advance", "budget": 5000}));
    h.until_event(|m| m["method"] == "Emulation.virtualTimeBudgetExpired");
    assert_eq!(eval_value(&mut h, "fired.map(f => f[0])"), json!(["t1000", "t5000"]));
    assert!((eval_value(&mut h, "performance.now()").as_f64().unwrap() - (start + 7000.0)).abs() < 1e-6);

    for (params, message) in [
        (json!({"policy": "pause", "budget": 10}), "Can only specify budget for non-Pause policy"),
        (json!({"policy": "bogus", "budget": 10}), "Can only specify budget for non-Pause policy"),
    ] {
        let r = h.page("Emulation.setVirtualTimePolicy", params.clone());
        assert_eq!(r["error"]["message"], message, "{params}: {r}");
    }
    assert_eq!(h.page("Emulation.setVirtualTimePolicy", json!({}))["error"]["code"], -32602);
    assert_eq!(
        h.page(
            "Emulation.setVirtualTimePolicy",
            json!({"policy": "pauseIfNetworkFetchesPending", "budget": 10, "maxVirtualTimeTaskStarvationCount": 5})
        )["result"]["virtualTimeTicksBase"]
            .as_f64(),
        Some(base)
    );
}

#[test]
fn virtual_time_belongs_to_one_page_per_browser_thread() {
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    let r =
        h.page("Emulation.setVirtualTimePolicy", json!({"policy": "pause", "maxVirtualTimeTaskStarvationCount": 5}));
    assert_eq!(r["error"]["message"], "Can only specify starvation count for non-Pause policy", "{r}");
    assert!(h.page("Emulation.setVirtualTimePolicy", json!({"policy": "pause"}))["result"].is_object());
    let id = h.send(None, "Target.createTarget", json!({"url": "about:blank", "browserContextId": ctx}));
    let messages = h.until_response(id);
    let other = messages
        .iter()
        .rev()
        .find(|m| m["method"] == "Target.attachedToTarget")
        .and_then(|m| m["params"]["sessionId"].as_str())
        .expect("second page attached")
        .to_owned();
    let r = h.call(Some(&other), "Emulation.setVirtualTimePolicy", json!({"policy": "pause"}));
    assert_eq!(r["error"]["message"], "virtual time is already enabled for another page on this browser thread", "{r}");
}

#[test]
fn virtual_time_initial_time_sets_date_and_survives_navigation() {
    let mut h = Harness::new();
    let r = h.page("Emulation.setVirtualTimePolicy", json!({"policy": "pause", "initialVirtualTime": 1_000_000}));
    assert!(r["result"]["virtualTimeTicksBase"].is_number(), "{r}");
    // Only the first call sets it; documents created from then on start at it.
    h.page("Emulation.setVirtualTimePolicy", json!({"policy": "pause", "initialVirtualTime": 5}));
    assert!(h.page("Page.navigate", json!({"url": "data:text/html,<p>virtual</p>"}))["error"].is_null());
    assert_eq!(eval_value(&mut h, "[Date.now(), new Date().getTime()]"), json!([1_000_000_000, 1_000_000_000]));
    h.page("Emulation.setVirtualTimePolicy", json!({"policy": "advance", "budget": 250}));
    h.until_event(|m| m["method"] == "Emulation.virtualTimeBudgetExpired");
    assert_eq!(eval_value(&mut h, "Date.now()"), json!(1_000_000_250));
}
