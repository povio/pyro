//! A browser context's network identity (`Target.createBrowserContext
//! {proxyServer, proxyBypassList, userAgent}`) covers what the engine loads
//! for its pages by itself: layout's images, fonts and `@import`ed sheets, and
//! what screenshots and PDFs paint. None of it reaches the origin directly or
//! with another User-Agent, a body loaded for one identity never serves
//! another's page, and a default context's page still loads straight from the
//! origin.

use super::*;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// A 2x2 red PNG.
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==";

const IDENTITY: &str = "LayoutIdentity/1.0 (one proxy)";

/// The sub-resources only layout and the renderer load (the page's own
/// loader does not).
const ENGINE_LOADS: [&str; 3] = ["bg.png", "imported-bg.png", "font.ttf"];

/// What the page's own loader loads: the image (layout does too) and the
/// `<style>`'s `@import`ed sheet, whose body it hands to its layouts.
const PAGE_LOADS: [&str; 2] = ["img.png", "imported.css"];

/// Requests a server saw: the path and the User-Agent.
type Hits = Arc<Mutex<Vec<(String, String)>>>;

/// An HTTP server that records every request and answers it itself: as the
/// origin for origin-form requests and, for absolute-form ones, as a proxy
/// that never forwards (what it answers never reaches the origin).
fn server() -> (String, Hits) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let hits = Hits::default();
    let record = hits.clone();
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
                let target = head.lines().next().unwrap_or_default().split(' ').nth(1).unwrap_or("/").to_owned();
                // Absolute-form (proxied): the path of the URL.
                let path = match target.strip_prefix("http://") {
                    Some(rest) => rest.find('/').map_or("/".to_owned(), |at| rest[at..].to_owned()),
                    None => target,
                };
                let agent = head
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("user-agent")))
                    .map(|(_, v)| v.trim().to_owned())
                    .unwrap_or_default();
                record.lock().unwrap().push((path.clone(), agent));
                let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
                let (kind, body): (&str, Vec<u8>) = if path.ends_with("/page") {
                    ("text/html", page(&dir, "").into_bytes())
                } else if path.ends_with("/pdf-page") {
                    ("text/html", page(&dir, "pdf-").into_bytes())
                } else if path.ends_with(".css") {
                    let prefix = if path.ends_with("pdf-imported.css") { "pdf-" } else { "" };
                    ("text/css", format!("#imp {{ background-image: url({prefix}imported-bg.png) }}").into_bytes())
                } else if path.ends_with(".png") {
                    ("image/png", pyro_cdp::fetch_protocol::base64_decode(PNG).unwrap())
                } else {
                    // Not a font: the request is what counts.
                    ("font/ttf", b"not a font".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            });
        }
    });
    (address, hits)
}

/// A page in `dir` with a CSS background image, an `<img>`, an `@font-face`
/// web font and an `@import`ed sheet with an image of its own; the names of
/// the engine's loads start with `prefix`.
fn page(dir: &str, prefix: &str) -> String {
    format!(
        r#"<!doctype html><html><head><style>
@import url({dir}/{prefix}imported.css);
@font-face {{ font-family: Probe; src: url({dir}/{prefix}font.ttf) format("truetype"); }}
body {{ margin: 0 }}
#bg {{ width: 20px; height: 20px; background-image: url({dir}/{prefix}bg.png) }}
#imp {{ width: 10px; height: 10px }}
#text {{ font-family: Probe, sans-serif }}
</style></head><body><div id=bg></div><div id=imp></div><img id=img src={dir}/img.png><p id=text>text</p></body></html>"#
    )
}

fn paths(hits: &Hits, dir: &str) -> Vec<(String, String)> {
    hits.lock().unwrap().iter().filter(|(p, _)| p.starts_with(dir)).cloned().collect()
}

fn count(hits: &Hits, path: &str) -> usize {
    hits.lock().unwrap().iter().filter(|(p, _)| p == path).count()
}

/// A new page in `context` (`None`: the default context), on about:blank.
fn attach(h: &mut Harness, context: Option<&str>) -> String {
    let mut params = json!({"url": "about:blank"});
    if let Some(context) = context {
        params["browserContextId"] = json!(context);
    }
    let id = h.send(None, "Target.createTarget", params);
    let messages = h.until_response(id);
    let attached = messages.iter().find(|m| m["method"] == "Target.attachedToTarget").expect("attached");
    attached["params"]["sessionId"].as_str().unwrap().to_owned()
}

/// Opens a page in `context` (`None`: the default context) at `url`, loaded.
fn open(h: &mut Harness, context: Option<&str>, url: &str) -> String {
    let session = attach(h, context);
    navigate(h, &session, url);
    session
}

const LAYOUT_QUERY: &str =
    "['bg', 'imp', 'text'].map((id) => document.getElementById(id).getBoundingClientRect().width).join()";

fn navigate(h: &mut Harness, session: &str, url: &str) {
    h.call(Some(session), "Page.enable", json!({}));
    let r = h.call(Some(session), "Page.navigate", json!({"url": url}));
    assert!(r.get("error").is_none(), "{r}");
    h.until_event(|m| m["method"] == "Page.loadEventFired" && m["sessionId"] == session);
}

/// A layout query (which starts what the layout misses loading).
fn lay_out(h: &mut Harness, session: &str) {
    let r = h.call(Some(session), "Runtime.evaluate", json!({"expression": LAYOUT_QUERY, "returnByValue": true}));
    assert!(r.get("error").is_none(), "{r}");
}

/// Lays the page out again until `hits` saw every one of `paths`: a body
/// that arrives makes the next query lay the document out again with it
/// (the imported sheet's image is only asked for then).
fn lay_out_until(h: &mut Harness, session: &str, hits: &Hits, paths: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        lay_out(h, session);
        h.run_for(30);
        let missing: Vec<&String> = paths.iter().filter(|p| count(hits, p) == 0).collect();
        if missing.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "never requested: {missing:?}; saw {:?}", hits.lock().unwrap());
    }
}

fn context(h: &mut Harness, proxy: &str) -> String {
    let r = h.call(
        None,
        "Target.createBrowserContext",
        json!({"proxyServer": proxy, "proxyBypassList": "<-loopback>", "userAgent": IDENTITY}),
    );
    r["result"]["browserContextId"].as_str().unwrap_or_else(|| panic!("{r}")).to_owned()
}

/// Layout's own loads, a screenshot's and a PDF's of a page in a context with
/// a proxy and user agent of its own go through that proxy with that agent.
#[test]
fn layout_and_render_loads_go_through_the_context_proxy_with_its_user_agent() {
    let (origin, direct) = server();
    let (proxy, proxied) = server();
    let mut h = Harness::new();
    let ctx = context(&mut h, &proxy);
    let session = open(&mut h, Some(&ctx), &format!("{origin}/id/page"));
    let want: Vec<String> = ENGINE_LOADS.iter().chain(&PAGE_LOADS).map(|name| format!("/id/{name}")).collect();
    lay_out_until(&mut h, &session, &proxied, &want);

    // A screenshot paints what the document's layout fetched (images, the
    // font, the imported sheet) as it is: it loads none of it again.
    let before: Vec<usize> = ENGINE_LOADS.iter().map(|name| count(&proxied, &format!("/id/{name}"))).collect();
    let r = h.call(Some(&session), "Page.captureScreenshot", json!({}));
    assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
    let after: Vec<usize> = ENGINE_LOADS.iter().map(|name| count(&proxied, &format!("/id/{name}"))).collect();
    assert_eq!(after, before, "the screenshot reused the document's own: {:?}", paths(&proxied, "/id/"));

    // A PDF of a document nothing laid out yet: its layout loads what it
    // prints, and the PDF waits for them.
    navigate(&mut h, &session, &format!("{origin}/id/pdf-page"));
    let r = h.call(Some(&session), "Page.printToPDF", json!({}));
    assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
    for name in ["pdf-bg.png", "pdf-font.ttf", "pdf-imported.css"] {
        assert!(count(&proxied, &format!("/id/{name}")) > 0, "{name}: {:?}", paths(&proxied, "/id/"));
    }

    let seen = paths(&proxied, "/id/");
    assert!(seen.iter().all(|(_, agent)| agent == IDENTITY), "{seen:?}");
    assert_eq!(paths(&direct, "/id/"), vec![], "nothing of the page reached the origin directly");
}

/// The bodies layout loaded serve only the document that loaded them: a page
/// in a context with its own proxy and agent loads everything through them
/// although a default context's page loaded the same URLs straight from the
/// origin before, which it still does; another page of that context loads its
/// own, through the same proxy (bodies are per document: security review
/// cycle 3, layout body cache).
#[test]
fn layout_bodies_serve_only_their_network_identity() {
    let (origin, direct) = server();
    let (proxy, proxied) = server();
    let mut h = Harness::new();
    let url = format!("{origin}/shared/page");
    let want: Vec<String> = ENGINE_LOADS.iter().map(|name| format!("/shared/{name}")).collect();

    // The default context: straight from the origin, with Pyro's agent.
    let plain = open(&mut h, None, &url);
    lay_out_until(&mut h, &plain, &direct, &want);
    let seen = paths(&direct, "/shared/");
    assert!(seen.iter().all(|(_, agent)| agent.starts_with("Mozilla/5.0 (compatible; pyro/")), "{seen:?}");
    let r = h.call(Some(&plain), "Page.captureScreenshot", json!({}));
    assert!(r["result"]["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
    assert_eq!(paths(&proxied, "/shared/"), vec![], "the default context does not use the proxy");

    // The same page in a context of its own identity: every engine load goes
    // through its proxy, none is served from the default context's bodies.
    let direct_before = paths(&direct, "/shared/").len();
    let ctx = context(&mut h, &proxy);
    let first = open(&mut h, Some(&ctx), &url);
    lay_out_until(&mut h, &first, &proxied, &want);
    assert_eq!(paths(&direct, "/shared/").len(), direct_before, "{:?}", paths(&direct, "/shared/"));
    let seen = paths(&proxied, "/shared/");
    assert!(seen.iter().all(|(_, agent)| agent == IDENTITY), "{seen:?}");

    // Another page of the same context loads its own, through the same proxy.
    let loads: Vec<usize> = want.iter().map(|path| count(&proxied, path)).collect();
    let second = open(&mut h, Some(&ctx), &url);
    let deadline = Instant::now() + Duration::from_secs(10);
    while want.iter().zip(&loads).any(|(path, before)| count(&proxied, path) <= *before) {
        assert!(Instant::now() < deadline, "the second page's own loads: {:?}", paths(&proxied, "/shared/"));
        lay_out(&mut h, &second);
        h.run_for(30);
    }
    assert_eq!(paths(&direct, "/shared/").len(), direct_before, "{:?}", paths(&direct, "/shared/"));
    assert!(paths(&proxied, "/shared/").iter().all(|(_, agent)| agent == IDENTITY));
}

/// A page's Fetch interception pauses its layout's loads too; one it fails
/// is a failure for that document only: another context's page of the same
/// identity (both go out as the browser does) loads it itself.
#[test]
fn a_layout_load_one_page_intercepts_and_fails_is_not_failed_for_others() {
    let (origin, direct) = server();
    let mut h = Harness::new();
    let url = format!("{origin}/abort/page");
    let plain_context = |h: &mut Harness| {
        let r = h.call(None, "Target.createBrowserContext", json!({}));
        r["result"]["browserContextId"].as_str().unwrap_or_else(|| panic!("{r}")).to_owned()
    };
    let paused_background = |m: &Value| {
        m["method"] == "Fetch.requestPaused"
            && m["params"]["request"]["url"].as_str().is_some_and(|u| u.ends_with("/bg.png"))
    };

    let context = plain_context(&mut h);
    let first = attach(&mut h, Some(&context));
    h.call(Some(&first), "Fetch.enable", json!({"patterns": [{"urlPattern": "*/abort/bg.png"}]}));
    h.call(Some(&first), "Page.enable", json!({}));
    h.send(Some(&first), "Page.navigate", json!({"url": url}));
    // Every message is kept: the load may be paused while the page loads.
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let paused = loop {
        seen.extend(h.run_for(30));
        if let Some(paused) = seen.iter().find(|m| paused_background(m) && m["sessionId"] == first) {
            break paused.clone();
        }
        if seen.iter().any(|m| m["method"] == "Page.loadEventFired" && m["sessionId"] == first) {
            h.send(Some(&first), "Runtime.evaluate", json!({"expression": LAYOUT_QUERY}));
        }
        assert!(Instant::now() < deadline, "the layout's load was not paused: {seen:#?}");
    };
    let r = h.call(
        Some(&first),
        "Fetch.failRequest",
        json!({"requestId": paused["params"]["requestId"], "errorReason": "BlockedByClient"}),
    );
    assert!(r.get("error").is_none(), "{r}");
    // Laid out again (its other loads arrive meanwhile): it does not ask again.
    let mut later = Vec::new();
    for _ in 0..5 {
        h.send(Some(&first), "Runtime.evaluate", json!({"expression": LAYOUT_QUERY}));
        later.extend(h.run_for(60));
    }
    assert!(!later.iter().any(paused_background), "asked again: {later:#?}");
    assert_eq!(count(&direct, "/abort/bg.png"), 0, "the failed request never reached the site");

    // Another context without interception loads it, from the site.
    let other = plain_context(&mut h);
    let second = open(&mut h, Some(&other), &url);
    lay_out_until(&mut h, &second, &direct, &["/abort/bg.png".to_owned()]);
    assert_eq!(count(&direct, "/abort/bg.png"), 1, "{:?}", paths(&direct, "/abort/"));
}
