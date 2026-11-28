//! Resource Timing and Navigation Timing over real HTTP: entries for every
//! kind of subresource, their phases from pyro-net, Timing-Allow-Origin, the
//! buffer and PerformanceObserver delivery.
use pyro_browser::{BrowserThread, NavigateOpts, PageId};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A server answering each request (one per connection) with `route(path)`:
/// (status, extra headers, content type, body).
type Route = fn(&str, &str) -> (u16, String, &'static str, Vec<u8>);

struct Server {
    origin: String,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new(route: Route) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let me = Arc::new(origin.clone());
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                if length > 0 {
                    let mut body = vec![0; length];
                    let _ = std::io::Read::read_exact(&mut reader, &mut body);
                }
                if path == "/stop" {
                    break;
                }
                let me = me.clone();
                thread::spawn(move || {
                    let (status, headers, mime, body) = route(&path, &me);
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n{headers}Connection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        Self { origin, thread: Some(thread) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        self.thread.take().unwrap().join().unwrap();
    }
}

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn until(thread: &mut BrowserThread, page: PageId, condition: &str) {
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |browser| {
        let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(condition, None, 1).is_ok_and(|v| v.to_bool())
    });
    assert_eq!(result, pyro_runtime::RunOutcome::Done, "waiting for {condition}");
}

fn page_route(path: &str, _me: &str) -> (u16, String, &'static str, Vec<u8>) {
    let (mime, body): (&str, &[u8]) = match path {
        "/" => ("text/html", include_bytes!("resource_timing_page.html")),
        "/style.css" => ("text/css", b"p { color: red }"),
        "/script.js" => ("text/javascript", b"globalThis.scriptRan = true"),
        "/frame" => ("text/html", b"<p>frame</p>"),
        "/worker.js" => ("text/javascript", WORKER_SCRIPT.as_bytes()),
        "/missing" => return (404, String::new(), "text/plain", b"gone".to_vec()),
        _ => ("application/json", b"{\"answer\":42}"),
    };
    (200, String::new(), mime, body.to_vec())
}

/// A worker that fetches what the page names in its message, then answers
/// with what its own timeline says (`urls`), or runs `code` and answers with
/// its result.
const WORKER_SCRIPT: &str = "onmessage = async (e) => { \
    if (e.data.code) { postMessage(await (0, eval)(e.data.code)); return; } \
    for (const url of e.data.urls) await fetch(url).then(r => r.text()).catch(() => {}); \
    postMessage(performance.getEntriesByType('resource').map(e => e.name.split('/').pop() + ':' + e.initiatorType)); }";

fn other_route(path: &str, _me: &str) -> (u16, String, &'static str, Vec<u8>) {
    let headers = if path.starts_with("/tao") { "Timing-Allow-Origin: *\r\n".to_string() } else { String::new() };
    (200, headers, "text/plain", b"cross".to_vec())
}

fn open(server: &Server, other: &Server) -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    until(&mut thread, page, "document.readyState == 'complete' && globalThis.scriptRan === true");
    eval(&thread, page, &format!("globalThis.other = '{}'", other.origin));
    (thread, page)
}

#[test]
fn every_kind_of_subresource_has_an_entry_with_network_phases() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (mut thread, page) = open(&server, &other);
    eval(
        &thread,
        page,
        "globalThis.done = 0; \
         fetch('/data').then(r => r.text()).then(() => done++); \
         const x = new XMLHttpRequest(); x.open('GET', '/xhr'); x.onloadend = () => done++; x.send(); \
         navigator.sendBeacon('/beacon', 'b'); \
         fetch('/missing').then(r => r.text()).then(() => done++); \
         fetch(other + '/plain').then(r => r.text()).then(() => done++); \
         fetch(other + '/tao').then(r => r.text()).then(() => done++);",
    );
    until(&mut thread, page, "done == 5 && performance.getEntriesByName(location.origin + '/beacon').length == 1");
    let o = &server.origin;
    // Every kind, named after its URL, with its initiator.
    assert_eq!(
        eval(
            &thread,
            page,
            "performance.getEntriesByType('resource') \
               .map(e => e.name.replace(location.origin, '').replace(other, 'X') + '=' + e.initiatorType).sort().join()"
        ),
        "/beacon=beacon,/data=fetch,/frame=iframe,/img.png=img,/missing=fetch,/script.js=script,/style.css=link,/xhr=xmlhttprequest,X/plain=fetch,X/tao=fetch"
    );
    // A same-origin fetch: every phase, in order, and its sizes.
    assert_eq!(
        eval(
            &thread,
            page,
            &format!(
                "const e = performance.getEntriesByName('{o}/data')[0]; \
                 const order = ['startTime', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', \
                   'connectEnd', 'requestStart', 'responseStart', 'responseEnd']; \
                 [e instanceof PerformanceResourceTiming, e.entryType, e.startTime > 0, e.fetchStart == e.startTime, \
                  order.every((k, i) => i == 0 || e[order[i - 1]] <= e[k]), e.responseEnd > e.startTime, \
                  e.duration == e.responseEnd - e.startTime, e.secureConnectionStart, e.redirectStart, e.workerStart, \
                  e.encodedBodySize, e.decodedBodySize, e.transferSize, e.nextHopProtocol, e.responseStatus, \
                  e.contentType, e.deliveryType, e.renderBlockingStatus, e.toJSON().initiatorType, e.toJSON().transferSize, \
                  performance.getEntriesByName('{o}/data')[0] === e].join()"
            )
        ),
        "true,resource,true,true,true,true,true,0,0,0,13,13,313,http/1.1,200,application/json,,non-blocking,fetch,313,true"
    );
    // A stylesheet blocks rendering; a 404 is still an entry with its status.
    assert_eq!(
        eval(
            &thread,
            page,
            &format!(
                "[performance.getEntriesByName('{o}/style.css')[0].renderBlockingStatus, \
                  performance.getEntriesByName('{o}/missing')[0].responseStatus].join()"
            )
        ),
        "blocking,404"
    );
    // Cross-origin without Timing-Allow-Origin: only start, fetchStart and
    // the end; with `*`, everything.
    assert_eq!(
        eval(
            &thread,
            page,
            "const plain = performance.getEntriesByName(other + '/plain')[0]; \
             const tao = performance.getEntriesByName(other + '/tao')[0]; \
             [plain.fetchStart > 0, plain.responseEnd >= plain.fetchStart, plain.domainLookupStart, plain.connectStart, \
              plain.requestStart, plain.responseStart, plain.transferSize, plain.encodedBodySize, plain.nextHopProtocol, \
              tao.requestStart >= tao.fetchStart && tao.requestStart > 0, tao.responseStart >= tao.requestStart, \
              tao.encodedBodySize].join()"
        ),
        "true,true,0,0,0,0,0,0,,true,true,5"
    );
    // An iframe's entry is the parent's: its own origin's shows status and
    // type, another origin's neither (nor, without TAO, its phases).
    eval(
        &thread,
        page,
        "const f = document.createElement('iframe'); f.src = other + '/frame'; document.body.append(f);",
    );
    until(&mut thread, page, "performance.getEntriesByName(other + '/frame').length == 1");
    assert_eq!(
        eval(
            &thread,
            page,
            &format!(
                "const same = performance.getEntriesByName('{o}/frame')[0]; \
                 const cross = performance.getEntriesByName(other + '/frame')[0]; \
                 [same.responseStatus, same.contentType, same.requestStart > 0, \
                  cross.initiatorType, cross.responseStatus, cross.contentType, cross.requestStart, \
                  cross.responseEnd > 0].join()"
            )
        ),
        "200,text/html,true,iframe,0,,0,true"
    );
    // A synchronous XHR's entry is there when `send()` returns.
    assert_eq!(
        eval(
            &thread,
            page,
            "const s = new XMLHttpRequest(); s.open('GET', '/sync', false); s.send(); \
             performance.getEntriesByName(location.origin + '/sync').map(e => e.initiatorType + ':' + e.responseStatus).join()"
        ),
        "xmlhttprequest:200"
    );
}

#[test]
fn navigation_timing_has_the_document_fetch() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (thread, page) = open(&server, &other);
    let size = include_bytes!("resource_timing_page.html").len();
    assert_eq!(
        eval(
            &thread,
            page,
            "const n = performance.getEntriesByType('navigation')[0]; \
             const order = ['startTime', 'fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', \
               'connectEnd', 'requestStart', 'responseStart', 'responseEnd', 'domInteractive', \
               'domContentLoadedEventStart', 'domComplete', 'loadEventEnd']; \
             const t = performance.timing; \
             [n.startTime, n.fetchStart, order.every((k, i) => i == 0 || n[order[i - 1]] <= n[k]), n.responseEnd > 0, \
              n.encodedBodySize, n.decodedBodySize, n.transferSize == n.encodedBodySize + 300, n.nextHopProtocol, \
              n.responseStatus, n.contentType, n.redirectCount, \
              t.requestStart >= t.fetchStart && t.responseEnd >= t.requestStart && t.domInteractive >= t.responseEnd, \
              t.fetchStart == t.navigationStart, \
              performance.measure('m', 'requestStart', 'responseEnd').duration == n.responseEnd - n.requestStart].join()"
        ),
        format!("0,0,true,true,{size},{size},true,http/1.1,200,text/html,0,true,true,true")
    );
}

#[test]
fn the_buffer_fills_fires_its_event_and_observers_see_every_entry() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (mut thread, page) = open(&server, &other);
    eval(
        &thread,
        page,
        "globalThis.seen = []; globalThis.full = 0; globalThis.dropped = []; globalThis.buffered = null; \
         new PerformanceObserver((list, obs, options) => { \
           seen.push(...list.getEntries().map(e => e.name.split('/').pop())); dropped.push(options.droppedEntriesCount); \
         }).observe({ type: 'resource' }); \
         new PerformanceObserver(list => { buffered ??= list.getEntries().length; }).observe({ type: 'resource', buffered: true }); \
         performance.clearResourceTimings(); \
         performance.setResourceTimingBufferSize(2); \
         performance.onresourcetimingbufferfull = e => { full++; globalThis.fullEvent = [e.type, e.bubbles, e.target === performance].join(); }; \
         globalThis.done = 0; \
         for (const n of [1, 2, 3, 4]) fetch('/n' + n).then(r => r.text()).then(() => done++);",
    );
    until(&mut thread, page, "done == 4 && seen.length == 4 && full > 0");
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    // Entries that did not fit are dropped and counted for observers.
    eval(
        &thread,
        page,
        "globalThis.late = null; new PerformanceObserver((list, obs, options) => { late ??= options.droppedEntriesCount; }) \
           .observe({ type: 'resource', buffered: true });",
    );
    until(&mut thread, page, "late !== null");
    // The buffer keeps the first two entries to complete: the four fetches run in parallel, so
    // which two is up to the network (entries are added as responses end).
    assert_eq!(
        eval(
            &thread,
            page,
            "globalThis.kept = performance.getEntriesByType('resource').map(e => e.name.split('/').pop()); \
             [kept.length == 2 && new Set(kept).size == 2 && kept.every(n => seen.includes(n)), full > 0, fullEvent, \
              seen.sort().join(' '), late, buffered >= 4, PerformanceObserver.supportedEntryTypes.join(' ')].join()"
        ),
        "true,true,resourcetimingbufferfull,false,true,n1 n2 n3 n4,2,true,element event first-input largest-contentful-paint layout-shift long-animation-frame longtask mark measure navigation paint resource soft-navigation"
    );
    // A handler that makes room keeps the waiting entries.
    eval(
        &thread,
        page,
        "performance.clearResourceTimings(); performance.setResourceTimingBufferSize(1); \
         performance.onresourcetimingbufferfull = () => performance.setResourceTimingBufferSize(10); \
         done = 0; for (const n of [5, 6, 7]) fetch('/n' + n).then(r => r.text()).then(() => done++);",
    );
    until(&mut thread, page, "done == 3");
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    assert_eq!(
        eval(&thread, page, "performance.getEntriesByType('resource').map(e => e.name.split('/').pop()).sort().join()"),
        "n5,n6,n7"
    );
}

#[test]
fn only_the_containers_navigation_of_an_iframe_is_the_parents() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (mut thread, page) = open(&server, &other);
    eval(
        &thread,
        page,
        "globalThis.loads = 0; globalThis.f = document.createElement('iframe'); f.onload = () => loads++; \
         f.src = '/frame?src'; document.body.append(f);",
    );
    until(&mut thread, page, "loads == 1");
    // The frame's own navigation (a script in it, a link, a form): its URL
    // is not the parent's to read.
    eval(&thread, page, "f.contentWindow.location.href = '/frame?own';");
    until(&mut thread, page, "loads == 2");
    // The container's again (`src` set by the parent): the parent's.
    eval(&thread, page, "f.src = '/frame?again';");
    until(&mut thread, page, "loads == 3");
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(
        eval(
            &thread,
            page,
            "performance.getEntriesByType('resource').filter(e => e.name.includes('/frame?')) \
               .map(e => e.initiatorType + ':' + e.name.split('?').pop()).join()"
        ),
        "iframe:src,iframe:again"
    );
}

#[test]
fn an_aborted_fetchs_report_does_not_hold_back_later_entries() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (mut thread, page) = open(&server, &other);
    // The fetch ends on the I/O thread while this task runs, and the abort
    // removes it before its task does: that task still adds the report, so
    // the next one (the image's) is announced to observers.
    eval(
        &thread,
        page,
        "globalThis.seen = []; \
         new PerformanceObserver(list => seen.push(...list.getEntries().map(e => e.name.split('/').pop()))) \
           .observe({ type: 'resource' }); \
         const c = new AbortController(); fetch('/aborted', { signal: c.signal }).catch(() => {}); \
         const t = Date.now(); while (Date.now() - t < 300); \
         c.abort();",
    );
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    eval(&thread, page, "const i = new Image(); i.src = '/img.png?late';");
    until(&mut thread, page, "seen.includes('img.png?late')");
}

/// Asks the page's worker `w` to run `code` and waits for its answer.
fn in_worker(thread: &mut BrowserThread, page: PageId, code: &str) -> String {
    eval(
        thread,
        page,
        &format!(
            "globalThis.answer = undefined; w.onmessage = e => answer = e.data; w.postMessage({{code: {code:?}}});"
        ),
    );
    until(thread, page, "answer !== undefined");
    eval(thread, page, "String(answer)")
}

#[test]
fn a_workers_fetches_are_in_its_own_timeline() {
    let (server, other) = (Server::new(page_route), Server::new(other_route));
    let (mut thread, page) = open(&server, &other);
    eval(
        &thread,
        page,
        "globalThis.w = new Worker('/worker.js'); globalThis.answer = undefined; w.onmessage = e => answer = e.data; \
         w.postMessage({urls: ['/w1', '/w2', other + '/plain', other + '/tao']});",
    );
    until(&mut thread, page, "answer !== undefined");
    assert_eq!(eval(&thread, page, "answer.join()"), "w1:fetch,w2:fetch,plain:fetch,tao:fetch");
    // The page's timeline has none of the worker's fetches.
    assert_eq!(
        eval(
            &thread,
            page,
            "performance.getEntriesByType('resource').filter(e => /w1|w2|plain|tao/.test(e.name)).length"
        ),
        "0"
    );
    // Phases on the worker's clock, its interfaces, and Timing-Allow-Origin
    // against the worker's origin.
    assert_eq!(
        in_worker(
            &mut thread,
            page,
            "const e = performance.getEntriesByName(location.origin + '/w1')[0]; \
             const plain = performance.getEntriesByType('resource').find(e => e.name.endsWith('/plain')); \
             const tao = performance.getEntriesByType('resource').find(e => e.name.endsWith('/tao')); \
             [e instanceof PerformanceResourceTiming, e instanceof PerformanceEntry, self.performance instanceof Performance, \
              e.startTime > 0, e.responseEnd <= performance.now(), e.requestStart >= e.fetchStart, e.encodedBodySize, \
              e.responseStatus, plain.requestStart, plain.transferSize, tao.requestStart > 0, tao.encodedBodySize, \
              performance === self.performance, typeof performance.timing, \
              PerformanceObserver.supportedEntryTypes.join(' ')].join()"
        ),
        "true,true,true,true,true,true,13,200,0,0,true,5,true,undefined,mark measure resource"
    );
    // User Timing, an observer, and the buffer with its full event.
    assert_eq!(
        in_worker(
            &mut thread,
            page,
            "(async () => { \
               const seen = []; let full = 0; \
               new PerformanceObserver(list => seen.push(...list.getEntries().map(e => e.entryType + ':' + e.name.split('/').pop()))) \
                 .observe({entryTypes: ['resource', 'mark']}); \
               performance.mark('m'); \
               performance.clearResourceTimings(); performance.setResourceTimingBufferSize(1); \
               performance.onresourcetimingbufferfull = e => { full++; }; \
               for (const n of ['a', 'b', 'c']) await fetch('/' + n).then(r => r.text()); \
               await new Promise(r => setTimeout(r, 50)); \
               const m = performance.measure('since', 'm'); \
               return [seen.sort().join(' '), full > 0, \
                 performance.getEntriesByType('resource').map(e => e.name.split('/').pop()).join(' '), \
                 m.duration >= 0, performance.getEntriesByType('mark').length].join(); })()"
        ),
        "mark:m resource:a resource:b resource:c,true,a,true,1"
    );
    assert_eq!(
        in_worker(
            &mut thread,
            page,
            "(() => { try { performance.measure('x', 'requestStart'); } catch (e) { return e.name; } })()"
        ),
        "TypeError"
    );
    // An imported script is the worker's.
    assert_eq!(
        in_worker(
            &mut thread,
            page,
            "performance.setResourceTimingBufferSize(10); importScripts('/script.js'); \
             performance.getEntriesByType('resource').filter(e => e.name.endsWith('/script.js')).map(e => e.initiatorType).join()"
        ),
        "other"
    );
}
