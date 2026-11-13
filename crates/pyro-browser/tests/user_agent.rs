//! Per-page identity survives document replacement without mutating the context client.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId};
use pyro_webapi::UserAgentOverride;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn identity(ua: &str, language: &str, platform: &str) -> UserAgentOverride {
    UserAgentOverride {
        user_agent: ua.into(),
        accept_language: Some(language.into()),
        platform: Some(platform.into()),
        metadata: None,
    }
}
fn eval(t: &BrowserThread, page: PageId, script: &str) -> String {
    let scope = t.browser.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
const SNAPSHOT: &str = "[navigator.userAgent,navigator.language,navigator.platform].join('|')";

#[test]
fn identity_is_live_in_all_worlds_and_precedes_navigation_init_scripts() {
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(pyro_net::NetConfig { user_agent: "Context-UA".into(), ..Default::default() }).unwrap();
    let page = t.browser.create_page().unwrap();
    let sibling = t.browser.create_page().unwrap();
    let original = eval(&t, page, SNAPSHOT);
    let isolated = t.browser.isolated_world(page, "before").unwrap();
    t.browser.set_user_agent_override(page, Some(identity("Page-UA", "sl-SI,en;q=0.5", "TestOS"))).unwrap();
    assert_eq!(eval(&t, page, SNAPSHOT), "Page-UA|sl-SI|TestOS");
    assert_eq!(isolated.scope().unwrap().eval(SNAPSHOT, None, 1).unwrap().to_string().unwrap(), "Page-UA|sl-SI|TestOS");
    assert_eq!(eval(&t, sibling, SNAPSHOT), original);
    for world in [None, Some("after".into())] {
        t.browser.add_init_script(
            page,
            InitScript { id: format!("{world:?}"), world, source: format!("globalThis.atInit = {SNAPSHOT}") },
        );
    }
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    assert!(isolated.scope().is_err());
    assert_eq!(eval(&t, page, "atInit"), "Page-UA|sl-SI|TestOS");
    let after = t.browser.isolated_world(page, "after").unwrap();
    assert_eq!(after.scope().unwrap().eval("atInit", None, 1).unwrap().to_string().unwrap(), "Page-UA|sl-SI|TestOS");
    t.browser.set_user_agent_override(page, Some(identity("", "fr-FR", "LocaleOS"))).unwrap();
    assert_eq!(eval(&t, page, SNAPSHOT), "Context-UA|fr-FR|LocaleOS");
    t.browser.set_user_agent_override(page, None).unwrap();
    assert_eq!(eval(&t, page, SNAPSHOT), original);
    assert_eq!(after.scope().unwrap().eval(SNAPSHOT, None, 1).unwrap().to_string().unwrap(), original);
    t.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    assert_eq!(eval(&t, page, "atInit"), original);
    assert!(t.browser.set_user_agent_override(PageId(u32::MAX), None).is_err());
}

#[derive(Clone, Debug)]
struct Request {
    path: String,
    ua: String,
    language: String,
}
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let worker = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap().to_string();
                let mut request = Request { path: path.clone(), ua: String::new(), language: String::new() };
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        match name.to_ascii_lowercase().as_str() {
                            "user-agent" => request.ua = value.trim().into(),
                            "accept-language" => request.language = value.trim().into(),
                            _ => {}
                        }
                    }
                }
                if path == "/stop" {
                    break;
                }
                recorded.lock().unwrap().push(request);
                let tag = path.split_once('?').map_or("", |(_, tag)| tag);
                if path.starts_with("/redirect?") {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nLocation: /doc?{tag}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    continue;
                }
                let (mime, body) = if path.starts_with("/doc?") {
                    (
                        "text/html",
                        format!(
                            r#"<link rel=stylesheet href='/style?{tag}'>
<script src='/script?{tag}'></script><script type=module src='/module?{tag}'></script>
<script>fetch('/fetch?{tag}').then(r=>r.text()).then(()=>globalThis.fetchDone=true);
const x=new XMLHttpRequest();x.open('GET','/xhr?{tag}');x.onload=()=>globalThis.xhrDone=true;x.send();</script><p>identity</p>"#
                        ),
                    )
                } else if path.starts_with("/script?") {
                    ("text/javascript", "globalThis.scriptDone=true".into())
                } else if path.starts_with("/module?") {
                    ("text/javascript", "globalThis.moduleDone=true".into())
                } else if path.starts_with("/style?") {
                    ("text/css", "p {display:none}".into())
                } else {
                    ("text/plain", "ok".into())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { origin, requests, worker: Some(worker) }
    }
    fn navigate(&self, t: &mut BrowserThread, page: PageId, tag: &str, ua: &str, language: &str) {
        t.browser.navigate(page, NavigateOpts::address_bar(format!("{}/redirect?{tag}", self.origin))).unwrap();
        let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
            let scope = b.page(page).unwrap().main_frame.main_realm().unwrap().scope().unwrap();
            scope.eval(&format!("document.URL.endsWith('?{tag}') && document.readyState==='complete' && globalThis.fetchDone && globalThis.xhrDone && globalThis.scriptDone && globalThis.moduleDone"), None, 1).unwrap().to_bool()
        });
        assert_eq!(outcome, pyro_runtime::RunOutcome::Done, "page did not complete");
        let requests = self.requests.lock().unwrap().clone();
        let matching: Vec<_> = requests.iter().filter(|r| r.path.ends_with(&format!("?{tag}"))).collect();
        assert_eq!(matching.len(), 7, "{matching:?}");
        for request in matching {
            assert_eq!(request.ua, ua, "{request:?}");
            assert_eq!(request.language, language, "{request:?}");
        }
        assert_eq!(eval(t, page, "navigator.userAgent"), ua);
        assert_eq!(eval(t, page, "getComputedStyle(document.querySelector('p')).display"), "none");
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn request_defaults_cover_all_loader_and_binding_requests_without_sibling_bleed() {
    let server = Server::new();
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(pyro_net::NetConfig {
        user_agent: "Context-UA".into(),
        accept_language: "en-GB".into(),
        ..Default::default()
    })
    .unwrap();
    let a = t.browser.create_page().unwrap();
    let b = t.browser.create_page().unwrap();
    t.browser.set_user_agent_override(a, Some(identity("Page-A", "sl-SI", "A"))).unwrap();
    t.browser.set_user_agent_override(b, Some(identity("Page-B", "de-DE", "B"))).unwrap();
    server.navigate(&mut t, a, "first-a", "Page-A", "sl-SI");
    server.navigate(&mut t, b, "first-b", "Page-B", "de-DE");
    // Invalid header updates must leave current bindings AND future native defaults untouched.
    for bad in [identity("bad\r\nInjected: yes", "en", "Bad"), identity("Changed", "bad\nvalue", "Bad")] {
        assert!(t.browser.set_user_agent_override(a, Some(bad)).is_err());
        assert_eq!(eval(&t, a, SNAPSHOT), "Page-A|sl-SI|A");
    }
    server.navigate(&mut t, a, "invalid-a", "Page-A", "sl-SI");
    t.browser.set_user_agent_override(a, Some(identity("", "fr-FR", "LocaleOS"))).unwrap();
    server.navigate(&mut t, a, "locale-a", "Context-UA", "fr-FR");
    t.browser.set_user_agent_override(a, None).unwrap();
    server.navigate(&mut t, a, "reset-a", "Context-UA", "en-GB");
    server.navigate(&mut t, b, "unchanged-b", "Page-B", "de-DE");
}
