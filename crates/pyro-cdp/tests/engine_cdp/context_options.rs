//! What Playwright's `bypassCSP`, `serviceWorkers: 'block'` and
//! `acceptDownloads: false` send, against local HTTP. Expected values are
//! measured on Playwright 1.59.1's headless Chromium (147, build 1217).
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type Route = fn(&str) -> (&'static str, &'static str, &'static [u8]);

/// Answers every request with `route(path)` (status, extra header lines,
/// body) until dropped; counts the paths asked for.
struct Site {
    origin: String,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Site {
    fn new(route: Route) -> Site {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (hits, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (seen, stopping) = (hits.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                seen.lock().unwrap().push(path.clone());
                let (status, headers, body) = route(&path);
                let head = format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        Site { origin, hits, stop, thread: Some(thread) }
    }

    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|p| *p == path).count()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.take().unwrap().join();
    }
}

fn context_id(h: &mut Harness) -> String {
    let target = h.frame_id.clone();
    let info = h.call(None, "Target.getTargetInfo", json!({ "targetId": target }));
    info["result"]["targetInfo"]["browserContextId"].as_str().unwrap_or_else(|| panic!("{info}")).to_owned()
}

/// Navigates the harness page and waits for its load event.
fn goto(h: &mut Harness, url: &str) {
    h.page("Page.enable", json!({}));
    let events = h.page_with_events("Page.navigate", json!({ "url": url }));
    if !events.iter().any(|e| e["method"] == "Page.loadEventFired") {
        h.until_event(|m| m["method"] == "Page.loadEventFired");
    }
}

/// A page in a new browser context made with `params`; returns its session.
fn page_in_context(h: &mut Harness, params: Value) -> String {
    let context = h.call(None, "Target.createBrowserContext", params);
    let context = context["result"]["browserContextId"].as_str().unwrap_or_else(|| panic!("{context}")).to_owned();
    let target = h.call(None, "Target.createTarget", json!({"url": "about:blank", "browserContextId": context}));
    let target_id = target["result"]["targetId"].clone();
    let attached = h.call(None, "Target.attachToTarget", json!({"targetId": target_id, "flatten": true}));
    attached["result"]["sessionId"].as_str().unwrap().to_owned()
}

/// Evaluates an awaited expression on `session` and returns its value.
fn eval_on(h: &mut Harness, session: &str, expression: &str) -> Value {
    let r = h.call(
        Some(session),
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
    );
    assert!(r["result"].get("exceptionDetails").is_none() && r.get("error").is_none(), "{expression}: {r}");
    r["result"]["result"]["value"].clone()
}

// ----- Page.setBypassCSP -------------------------------------------------------

/// A document and its frame, both delivered with a policy, whose script
/// inserts an element with a nonce.
const CSP_PAGE: &[u8] = b"<!doctype html><iframe src=\"/frame\"></iframe><script>
const d = document.createElement('div'); d.id = 'inserted'; d.setAttribute('nonce', 'abc'); document.body.append(d);
</script>";
const CSP_FRAME: &[u8] = b"<!doctype html><body><script>
const d = document.createElement('div'); d.id = 'inserted'; d.setAttribute('nonce', 'abc'); document.body.append(d);
</script>";
const CSP_HEADERS: &str = "Content-Type: text/html\r\nContent-Security-Policy: script-src 'unsafe-inline'\r\n";

/// The nonce attribute and property of the element the script of the page
/// and of its frame inserted, and the attribute of one inserted now.
const NONCES: &str = "(() => {
  const read = (doc) => { const d = doc.getElementById('inserted'); return [d.getAttribute('nonce'), d.nonce]; };
  const late = document.createElement('div'); late.setAttribute('nonce', 'late'); document.body.append(late);
  return [read(document), read(document.querySelector('iframe').contentDocument), late.getAttribute('nonce')];
})()";

#[test]
fn bypass_csp_lifts_nonce_hiding_from_the_next_document_in_every_frame() {
    // The engine enforces no directive; a header-delivered policy only turns
    // on nonce hiding, which Chromium also skips with bypassCSP on.
    let site = Site::new(|path| match path {
        "/frame" => ("200 OK", CSP_HEADERS, CSP_FRAME),
        _ => ("200 OK", CSP_HEADERS, CSP_PAGE),
    });
    let mut h = Harness::new();
    let page = format!("{}/page", site.origin);
    goto(&mut h, &page);
    let hidden = json!([["", "abc"], ["", "abc"], ""]);
    assert_eq!(eval_value(&mut h, NONCES), hidden);
    // The document already loaded keeps its policy (Chromium too).
    let r = h.page("Page.setBypassCSP", json!({"enabled": true}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, NONCES), hidden);
    goto(&mut h, &page);
    let kept = json!([["abc", "abc"], ["abc", "abc"], "late"]);
    assert_eq!(eval_value(&mut h, NONCES), kept);
    // Off again: from the next document on, as well.
    let r = h.page("Page.setBypassCSP", json!({"enabled": false}));
    assert_eq!(r["result"], json!({}), "{r}");
    assert_eq!(eval_value(&mut h, NONCES), kept);
    goto(&mut h, &page);
    assert_eq!(eval_value(&mut h, NONCES), hidden);
    let r = h.page("Page.setBypassCSP", json!({}));
    assert_eq!(r["error"]["code"], -32602, "enabled is required: {r}");
}

// ----- Target.createBrowserContext serviceWorkers ----------------------------

const SW_SCRIPT: &[u8] = b"self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(clients.claim()));
self.addEventListener('fetch', (e) => { if (new URL(e.request.url).pathname === '/hello') e.respondWith(new Response('from sw')); });";

fn sw_site(path: &str) -> (&'static str, &'static str, &'static [u8]) {
    match path {
        "/sw.js" => ("200 OK", "Content-Type: text/javascript\r\n", SW_SCRIPT),
        "/hello" => ("200 OK", "Content-Type: text/plain\r\n", b"from network"),
        _ => ("200 OK", "Content-Type: text/html\r\n", b"<!doctype html><p>sw</p>"),
    }
}

#[test]
fn a_context_made_with_service_workers_block_registers_none() {
    let site = Site::new(sw_site);
    let mut h = Harness::new();
    let r = h.call(None, "Target.createBrowserContext", json!({"serviceWorkers": "sometimes"}));
    assert_eq!(r["error"]["code"], -32602, "{r}");
    let blocked = page_in_context(&mut h, json!({"serviceWorkers": "block"}));
    h.call(Some(&blocked), "Runtime.enable", json!({}));
    h.call(Some(&blocked), "Page.enable", json!({}));
    let r = h.call(Some(&blocked), "Page.navigate", json!({"url": format!("{}/page", site.origin)}));
    assert!(r["result"].get("errorText").is_none(), "{r}");
    h.until_event(|m| m["method"] == "Page.loadEventFired");
    // Chromium with Playwright's init script: undefined after a warning,
    // no registration, and the page's requests reach the network.
    let id = h.send(
        Some(&blocked),
        "Runtime.evaluate",
        json!({"expression": "navigator.serviceWorker.register('/sw.js').then(String)", "awaitPromise": true, "returnByValue": true}),
    );
    let messages = h.until_response(id);
    let reply = messages.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(reply["result"]["result"]["value"], "undefined", "{reply}");
    let warning = messages.iter().find(|m| m["method"] == "Runtime.consoleAPICalled").expect("console warning");
    assert_eq!(warning["params"]["type"], "warning");
    assert_eq!(warning["params"]["args"][0]["value"], "Service Worker registration blocked by Playwright");
    // Unlike Playwright's script, the block is the context's: the
    // prototype's method registers nothing either.
    let probe = "ServiceWorkerContainer.prototype.register.call(navigator.serviceWorker, '/sw.js')
      .then(async (r) => [String(r), (await navigator.serviceWorker.getRegistrations()).length,
        navigator.serviceWorker.controller, await fetch('/hello').then((r) => r.text())])";
    assert_eq!(eval_on(&mut h, &blocked, probe), json!(["undefined", 0, null, "from network"]));
    assert_eq!(site.hits("/sw.js"), 0);
    // 'allow' (and no parameter) keeps registration.
    let allowed = page_in_context(&mut h, json!({"serviceWorkers": "allow"}));
    h.call(Some(&allowed), "Page.enable", json!({}));
    h.call(Some(&allowed), "Page.navigate", json!({"url": format!("{}/page", site.origin)}));
    h.until_event(|m| m["method"] == "Page.loadEventFired");
    let registered = "navigator.serviceWorker.register('/sw.js').then((r) => r.scope)";
    assert_eq!(eval_on(&mut h, &allowed, registered), json!(format!("{}/", site.origin)));
    assert_eq!(site.hits("/sw.js"), 1);
}

// ----- Browser.setDownloadBehavior deny, Browser.cancelDownload -------------

#[test]
fn denied_downloads_are_announced_and_canceled_like_chromium() {
    let site = Site::new(|_| ("200 OK", "Content-Type: text/html\r\n", b"<!doctype html><title>dl</title>"));
    let mut h = Harness::new();
    let ctx = context_id(&mut h);
    let frame = h.frame_id.clone();
    goto(&mut h, &format!("{}/page", site.origin));
    let download = |h: &mut Harness| -> Vec<Value> {
        eval_value(
            h,
            "{ const d = document.createElement('a'); d.href = URL.createObjectURL(new Blob(['x'])); d.download = 'x.txt'; d.click(); } 1",
        );
        let mut events = h.run_for(150);
        events.retain(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Browser.download")));
        events
    };
    // Playwright's `acceptDownloads: false` sends deny with events; headless
    // Chromium's default behaves the same.
    let mut guids = Vec::new();
    for behavior in ["deny", "default"] {
        let r = h.call(
            None,
            "Browser.setDownloadBehavior",
            json!({"behavior": behavior, "browserContextId": ctx, "eventsEnabled": true}),
        );
        assert_eq!(r["result"], json!({}), "{r}");
        let events = download(&mut h);
        assert_eq!(events.len(), 3, "{behavior}: {events:#?}");
        let begin = &events[0]["params"];
        assert_eq!(events[0]["method"], "Browser.downloadWillBegin");
        assert_eq!((&begin["frameId"], &begin["suggestedFilename"]), (&json!(frame), &json!("x.txt")));
        let guid = begin["guid"].as_str().unwrap().to_owned();
        assert_eq!(
            events[1..].iter().map(|e| e["params"].clone()).collect::<Vec<_>>(),
            [
                json!({"guid": guid, "totalBytes": 0, "receivedBytes": 0, "state": "inProgress"}),
                json!({"guid": guid, "totalBytes": 0, "receivedBytes": 0, "state": "canceled"}),
            ],
            "{behavior}"
        );
        guids.push(guid);
    }
    // Chromium answers a finished download's cancel with success, and an
    // unknown guid (or one of another context) with invalid params.
    for guid in &guids {
        let r = h.call(None, "Browser.cancelDownload", json!({"guid": guid, "browserContextId": ctx}));
        assert_eq!(r["result"], json!({}), "{r}");
        let r = h.call(None, "Browser.cancelDownload", json!({"guid": guid}));
        assert_eq!(r["error"]["message"], "No download item found for the given GUID", "{r}");
    }
    let r = h.call(
        None,
        "Browser.cancelDownload",
        json!({"guid": "00000000-0000-4000-8000-000000000000", "browserContextId": ctx}),
    );
    assert_eq!(
        (&r["error"]["code"], &r["error"]["message"]),
        (&json!(-32602), &json!("No download item found for the given GUID"))
    );
    // A completed download too.
    let dir = std::env::temp_dir().join(format!("pyro-cancel-downloads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.call(
        None,
        "Browser.setDownloadBehavior",
        json!({"behavior": "allowAndName", "browserContextId": ctx, "downloadPath": dir, "eventsEnabled": true}),
    );
    let events = download(&mut h);
    assert_eq!(events.last().unwrap()["params"]["state"], "completed", "{events:#?}");
    let guid = events[0]["params"]["guid"].clone();
    let r = h.call(None, "Browser.cancelDownload", json!({"guid": guid, "browserContextId": ctx}));
    assert_eq!(r["result"], json!({}), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Page.generateTestReport -------------------------------------------------

const REPORT_PAGE: &[u8] = b"<!doctype html><script>
window.seen = [];
new ReportingObserver((reports) => { for (const r of reports) seen.push(JSON.stringify(r)); }).observe();
</script>";

#[test]
fn generate_test_report_reaches_observers_and_the_endpoint_in_one_request() {
    let site = Site::new(|path| match path {
        "/report" => ("200 OK", "Content-Type: text/plain\r\n", b"ok"),
        _ => ("200 OK", "Content-Type: text/html\r\nReporting-Endpoints: default=\"/report\", other=\"/other\"\r\n", REPORT_PAGE),
    });
    let mut h = Harness::new();
    let page = format!("{}/page", site.origin);
    goto(&mut h, &page);
    let r = h.page("Page.generateTestReport", json!({"message": "one"}));
    assert_eq!(r["result"], json!({}), "{r}");
    let r = h.page("Page.generateTestReport", json!({"message": "two", "group": "other"}));
    assert_eq!(r["result"], json!({}), "{r}");
    let r = h.page("Page.generateTestReport", json!({}));
    assert_eq!(r["error"]["code"], -32602, "message is required: {r}");
    let r = h.page(
        "Runtime.evaluate",
        json!({"expression": "new Promise((resolve) => setTimeout(() => resolve(seen), 100))", "returnByValue": true, "awaitPromise": true}),
    );
    let seen = r["result"]["result"]["value"].clone();
    assert_eq!(
        seen,
        json!([
            format!("{{\"type\":\"test\",\"url\":\"{page}\",\"body\":{{\"message\":\"one\"}}}}"),
            format!("{{\"type\":\"test\",\"url\":\"{page}\",\"body\":{{\"message\":\"two\"}}}}"),
        ])
    );
    // One request per endpoint, each after its task.
    for _ in 0..50 {
        if site.hits("/report") == 1 && site.hits("/other") == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!((site.hits("/report"), site.hits("/other")), (1, 1));
}
