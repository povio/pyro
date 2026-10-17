//! Real JSC + isolated loopback HTTP, with an explicit browser task queue.
use lp_js::{ClassDef, Engine, JsClass, Realm, RealmOptions};
use lp_webapi::*;
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

struct WindowData {
    page: Rc<PageDom>,
    target: EventTargetData,
}
impl WindowHost for WindowData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}
struct Window;
impl JsClass for Window {
    const NAME: &'static str = "Window";
    type Data = WindowData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &WindowData| &d.target);
        define_members::<Window>(def);
    }
}
type Task = Box<dyn FnOnce() + Send>;
struct Hooks {
    tx: mpsc::Sender<Task>,
    posts: Arc<AtomicUsize>,
}
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        task();
    }
    fn navigate(&self, _: NavigationRequest) {}
    fn remote_poster(&self) -> Option<RemotePoster> {
        let tx = self.tx.clone();
        let posts = self.posts.clone();
        Some(Arc::new(move |task| {
            posts.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(task);
        }))
    }
}
struct Env {
    engine: Rc<Engine>,
    realm: Realm,
    page: Rc<PageDom>,
    tasks: mpsc::Receiver<Task>,
    posts: Arc<AtomicUsize>,
}
impl Env {
    fn new(url: &str) -> Self {
        Self::with_engine(url, Rc::new(Engine::new().unwrap()))
    }
    fn with_engine(url: &str, engine: Rc<Engine>) -> Self {
        register(&engine);
        let class = engine.register::<Window>();
        let dom = Rc::new(RefCell::new(lp_dom::Dom::new()));
        let doc = lp_dom::parse::parse_document(&dom, url, "<body></body>");
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let page = PageDom::new(dom, doc);
        *page.net.borrow_mut() = Some(lp_net::NetClient::new(Default::default()).unwrap());
        let (tx, tasks) = mpsc::channel();
        let posts = Arc::new(AtomicUsize::new(0));
        page.set_hooks(Some(Rc::new(Hooks { tx, posts: posts.clone() })));
        let realm = engine.create_realm(RealmOptions { global_class: Some(class), ..Default::default() }).unwrap();
        let key = page.create_window_target(doc);
        realm
            .set_global_data::<Window>(WindowData { page: page.clone(), target: EventTargetData::window(&page, key) })
            .unwrap();
        install(&realm.scope().unwrap(), page.clone()).unwrap();
        Self { engine, realm, page, tasks, posts }
    }
    fn eval(&self, source: &str) -> String {
        let s = self.realm.scope().unwrap();
        match s.eval(source, Some("fetch-test.js"), 1) {
            Ok(value) => value.to_string().unwrap(),
            Err(error) => panic!("{}\n{source}", error.details(&s).text()),
        }
    }
    fn until(&self, condition: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.eval(condition) != "true" {
            let wait = deadline.saturating_duration_since(Instant::now());
            assert!(!wait.is_zero(), "timeout waiting for {condition}");
            let task = self
                .tasks
                .recv_timeout(wait)
                .unwrap_or_else(|_| panic!("no task for {condition}; result={}", self.eval("globalThis.result")));
            task();
            self.page.after_task();
        }
    }
    fn finish_network_tasks(&self) {
        while self.page.fetch_usage().0 != 0 {
            self.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
            self.page.after_task();
        }
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        self.page.dispose();
        self.realm.dispose();
    }
}

type RecordedRequest = (String, String, Vec<u8>);
struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    release: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (release, gate) = mpsc::channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let record = requests.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                let mut headers = String::new();
                let mut length = 0;
                let mut origin = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                        if name.eq_ignore_ascii_case("origin") {
                            origin = value.trim().into();
                        }
                    }
                    headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                record.lock().unwrap().push((first.clone(), headers.clone(), body.clone()));
                let (status, extra, response) = match path.as_str() {
                    "/redirect" => ("302 Found", "Location: /json\r\n".into(), vec![]),
                    "/echo" => ("200 OK", "Content-Type: text/plain\r\n".into(), body),
                    "/cors" => ("200 OK", format!("Access-Control-Allow-Origin: {origin}\r\nAccess-Control-Expose-Headers: X-Visible\r\nAccess-Control-Expose-Headers: X-Second\r\nX-Second: second\r\nX-Visible: yes\r\nX-Hidden: no\r\nSet-Cookie: hidden=1\r\n"), b"cors".to_vec()),
                    "/empty" => ("204 No Content", String::new(), vec![]),
                    "/large" => ("200 OK", String::new(), vec![b'x'; 8192]),
                    "/gated" => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 6\r\n\r\n");
                        let _ = stream.flush(); let _ = gate.recv_timeout(Duration::from_secs(5));
                        let _ = stream.write_all(b"abcdef"); continue;
                    }
                    "/bad-json" => ("200 OK", String::new(), b"not-json".to_vec()),
                    "/missing" => ("404 Not Found", String::new(), b"missing".to_vec()),
                    _ => ("200 OK", "Content-Type: application/json\r\nSet-Cookie: hidden=1\r\n".into(), b"{\"answer\":42}".to_vec()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{extra}Connection: close\r\nContent-Length: {}\r\n\r\n",
                    response.len()
                );
                let _ = stream.write_all(&response);
            }
        });
        Self { url: format!("http://{address}/page"), stop, release, thread: Some(thread), requests }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.release.send(());
        let url = lp_net::Url::parse(&self.url).unwrap();
        let _ = TcpStream::connect(("127.0.0.1", url.port().unwrap()));
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn fetch_json_headers_clone_consumption_and_correlation() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.page
        .set_request_context(RequestContext { frame_id: Some("frame-7".into()), loader_id: Some("loader-9".into()) });
    let events = env.page.net.borrow().as_ref().unwrap().subscribe();
    env.eval(
        r#"globalThis.result='pending'; fetch('/json').then(async r => {
        if (!(r instanceof Response) || !r.ok || r.status!==200 || r.type!=='basic') throw Error('head');
        if (r.headers!==r.headers || r.headers.has('set-cookie')) throw Error('headers');
        try {r.headers.set('x','y');throw Error('mutable')} catch(e) {if (!(e instanceof TypeError)) throw e;}
        const clone=r.clone(); const data=await r.json();
        if (data.answer!==42 || !r.bodyUsed) throw Error('body');
        try {await r.text();throw Error('reused')} catch(e) {if (!(e instanceof TypeError)) throw e;}
        result=await clone.text();
    }).catch(e=>result='ERROR '+e);"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    let mut starts = 0;
    while let Ok(event) = events.try_recv() {
        if let lp_net::NetEvent::RequestWillBeSent { request, .. } = event {
            starts += 1;
            assert_eq!(request.frame_id.as_deref(), Some("frame-7"));
            assert_eq!(request.loader_id.as_deref(), Some("loader-9"));
            assert_eq!(request.destination, lp_net::Destination::Fetch);
        }
    }
    assert_eq!(starts, 1);
    assert!(env.posts.load(Ordering::Relaxed) <= 2);
}

#[test]
fn fetch_delivers_headers_before_body_and_abort_preserves_reason() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.ac=new AbortController();globalThis.reason={id:5};globalThis.result='pending';fetch('/gated',{signal:ac.signal}).then(r=>{globalThis.response=r;result='headers';r.text().then(()=>result='unexpected', e=>result=e===reason?'aborted':'wrong reason')},e=>result='ERROR '+e)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "headers");
    env.eval("ac.abort(reason)");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let _ = server.release.send(());
}

#[test]
fn fetch_post_bytes_headers_redirects_and_http_errors() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const bytes=new Uint8Array([99,65,66,88]);
        const r=await fetch('/echo',{method:'post',body:bytes.subarray(1,3),headers:[['X-Test','a'],['x-test','b'],['Host','wrong'],['Cookie','wrong=1']]});
        if(await r.text()!=='AB') throw Error('subview');
        const text=await fetch('/echo',{method:'POST',body:'hello'});
        if(await text.text()!=='hello') throw Error('string');
        const blob=await fetch('/echo',{method:'POST',body:new Blob(['blob'],{type:'text/plain'})});
        if(await blob.text()!=='blob') throw Error('blob');
        const missing=await fetch('/missing');
        if(missing.status!==404 || missing.ok || await missing.text()!=='missing') throw Error('404');
        const redirect=await fetch('/redirect');
        if(!redirect.redirected || !redirect.url.endsWith('/json') || (await redirect.json()).answer!==42) throw Error('redirect');
        const manual=await fetch('/redirect',{redirect:'manual'});
        if(manual.type!=='opaqueredirect' || manual.status!==0 || manual.url!=='' || [...manual.headers].length || await manual.text()!=='') throw Error('manual');
        try { await fetch('/redirect',{redirect:'error'}); throw Error('redirect accepted') } catch(e){if(!(e instanceof TypeError))throw e;}
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[0].2, b"AB");
    assert!(requests[0].1.to_ascii_lowercase().contains("x-test: a, b\r\n"));
    assert!(!requests[0].1.contains("wrong"));
    assert!(requests[1].1.to_ascii_lowercase().contains("content-type: text/plain;charset=utf-8"));
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_cors_opaque_guards_and_null_body() {
    let server = Server::new();
    let other = Server::new();
    let env = Env::new(&server.url);
    let remote = other.url.replace("/page", "");
    env.eval(&format!("globalThis.remote='{remote}'"));
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const r=await fetch(remote+'/cors');
        if(r.type!=='cors' || r.headers.get('x-visible')!=='yes' || r.headers.get('x-second')!=='second' || r.headers.has('x-hidden') || r.headers.has('set-cookie') || await r.text()!=='cors') throw Error('CORS filter');
        try {await fetch(remote+'/json');throw Error('CORS accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        try {await fetch(remote+'/cors',{mode:'same-origin'});throw Error('same-origin accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        const o=await fetch(remote+'/json',{mode:'no-cors',headers:{'X-Unsafe':'x'}});
        if(o.type!=='opaque' || o.status!==0 || o.url!=='' || o.ok || [...o.headers].length || await o.text()!=='' || o.bodyUsed || (await o.blob()).type!=='') throw Error('opaque');
        const n=await fetch('/empty');
        if(await n.text()!=='' || await n.text()!=='' || n.bodyUsed) throw Error('null body');
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    assert!(!other.requests.lock().unwrap().last().unwrap().1.to_ascii_lowercase().contains("x-unsafe"));
}

#[test]
fn fetch_binary_json_errors_and_body_budget_release() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending'; (async()=>{
        const r=await fetch('/json'); globalThis.held=r;
        const a=r.clone(),b=r.clone();
        const bytes=await a.bytes();
        if(!(bytes instanceof Uint8Array) || bytes[0]!==123) throw Error('bytes');
        const buffer=await b.arrayBuffer();
        if(!(buffer instanceof ArrayBuffer) || buffer.byteLength!==13) throw Error('arrayBuffer');
        const blob=await r.blob();
        if(blob.type!=='application/json' || blob.size!==13) throw Error('blob');
        try {await (await fetch('/bad-json')).json();throw Error('JSON accepted')} catch(e){if(!(e instanceof SyntaxError))throw e;}
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_abort_before_headers_and_after_completed_body() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result='pending';globalThis.reason={id:42};globalThis.ac=new AbortController();
        const p=fetch('/gated',{signal:ac.signal});ac.abort(reason);
        p.then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');"#,
    );
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
    env.eval(r#"result='pending';globalThis.ac2=new AbortController();fetch('/json',{signal:ac2.signal}).then(r=>{globalThis.unread=r;result='ready'},e=>result='ERROR '+e)"#);
    env.until("result!=='pending'");
    // Headers can precede completion; pump terminal delivery before aborting.
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    assert_eq!(env.page.fetch_usage(), (0, 13));
    env.eval("result='pending';ac2.abort(reason);unread.text().then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    let count = server.requests.lock().unwrap().len();
    env.eval("result='pending';fetch('/json',{signal:ac2.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(server.requests.lock().unwrap().len(), count);
}

#[test]
fn fetch_limits_reject_overflow_and_refund_retained_bytes() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_fetch_limits(FetchLimits {
        max_body_bytes: 32,
        max_total_body_bytes: 20,
        ..Default::default()
    }));
    env.eval(r#"globalThis.result='pending';(async()=>{
        globalThis.held=await fetch('/json');
        await held.clone().text();
        try {await (await fetch('/json')).text();throw Error('aggregate accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        if(await held.text()!=='{"answer":42}') throw Error('original damaged');
        try {await (await fetch('/large')).text();throw Error('large accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        if((await (await fetch('/json')).json()).answer!==42) throw Error('budget not refunded');
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_disposal_cancels_jobs_and_queued_tasks_are_harmless() {
    for whole_page in [false, true] {
        let server = Server::new();
        let env = Env::new(&server.url);
        env.eval("globalThis.result='pending';fetch('/gated').then(r=>{result='headers';r.text().then(()=>result='stale',()=>result='stale')})");
        env.until("result!=='pending'");
        assert_eq!(env.eval("result"), "headers");
        assert_eq!(env.page.fetch_usage().0, 1);
        if whole_page {
            env.page.dispose();
        } else {
            lifecycle::realm_disposed(&env.page, env.realm.id());
        }
        env.finish_network_tasks();
        assert_eq!(env.page.fetch_usage(), (0, 0));
        server.release.send(()).unwrap();
        while let Ok(task) = env.tasks.try_recv() {
            task();
        }
        assert_eq!(env.eval("result"), "headers");
        env.realm.dispose();
        env.engine.collect_garbage_now();
    }
}

#[test]
fn fetch_completed_unread_body_released_after_gc() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';fetch('/json').then(r=>{globalThis.response=r;result='ready'})");
    env.until("result!=='pending'");
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    assert_eq!(env.page.fetch_usage(), (0, 13));
    env.eval("response=undefined");
    for _ in 0..6 {
        env.eval("Array.from({length:2000}, (_,i)=>({i}))");
        env.engine.collect_garbage_now();
        env.engine.drain_finalized();
        env.page.after_task();
    }
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    assert!(env.page.set_fetch_limits(FetchLimits::default()));
}

#[test]
fn fetch_pending_limit_and_conversion_failures_do_not_start_requests() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert!(env.page.set_fetch_limits(FetchLimits { max_requests: 1, ..Default::default() }));
    env.eval("globalThis.result='pending';globalThis.ac=new AbortController();fetch('/gated',{signal:ac.signal}).then(r=>{globalThis.response=r;result='headers'})");
    env.until("result!=='pending'");
    let count = server.requests.lock().unwrap().len();
    env.eval(r#"result='pending'; (async()=>{
        for (const init of [{}, {method:'TRACE'}, {body:'x'}, {headers:{'bad name':'x'}}, {signal:{}}, {mode:'wrong'}, {get method(){throw new TypeError('getter')}}]) {
            try {await fetch('/json',init);throw Error('accepted')} catch(e){if(!(e instanceof TypeError))throw e;}
        }
        result='ok';
    })().catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
    assert_eq!(server.requests.lock().unwrap().len(), count);
    env.eval("ac.abort()");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn fetch_null_body_remains_readable_after_abort() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(r#"globalThis.result='pending';globalThis.ac=new AbortController();fetch('/empty',{signal:ac.signal}).then(async r=>{
        ac.abort();
        if(await r.text()!=='' || r.bodyUsed || await r.text()!=='') throw Error('null body');
        result='ok';
    }).catch(e=>result='ERROR '+e);"#);
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "ok");
}

#[test]
fn response_borrowed_methods_work_across_realms_without_self_roots() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';fetch('/json').then(r=>{globalThis.response=r;result='ready'})");
    env.until("result!=='pending'");
    while env.page.fetch_usage().0 != 0 {
        env.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
    }
    let class = env.engine.register::<Window>();
    let other = env.engine.create_realm(RealmOptions { global_class: Some(class), ..Default::default() }).unwrap();
    let key = env.page.create_window_target(env.page.document());
    other
        .set_global_data::<Window>(WindowData {
            page: env.page.clone(),
            target: EventTargetData::window(&env.page, key),
        })
        .unwrap();
    {
        let s = other.scope().unwrap();
        install(&s, env.page.clone()).unwrap();
        let main = env.realm.scope().unwrap();
        s.global().set("foreign", main.global().get("response").unwrap()).unwrap();
        assert_eq!(
            s.eval(
                "Object.getOwnPropertyDescriptor(Response.prototype,'headers').get.call(foreign)===foreign.headers",
                None,
                1
            )
            .unwrap()
            .to_string()
            .unwrap(),
            "true"
        );
        s.eval(
            "globalThis.result='pending';Response.prototype.clone.call(foreign).json().then(v=>result=v.answer)",
            None,
            1,
        )
        .unwrap();
        assert_eq!(s.eval("result", None, 1).unwrap().to_string().unwrap(), "42");
    }
    lifecycle::realm_disposed(&env.page, other.id());
    other.dispose();
    env.eval("result='pending';response.text().then(v=>result=v)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "{\"answer\":42}");
    env.finish_network_tasks();
    assert_eq!(env.page.fetch_usage(), (0, 0));
}

#[test]
fn fetch_disposal_before_queued_headers_preserves_shared_context_network() {
    let server = Server::new();
    let first = Env::new(&server.url);
    let second = Env::with_engine(&server.url, first.engine.clone());
    *second.page.net.borrow_mut() = first.page.net.borrow().clone();
    first.eval("globalThis.result='pending';fetch('/gated').then(()=>result='stale',()=>result='stale')");
    let stale = first.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
    first.page.dispose();
    stale();
    assert_eq!(first.eval("result"), "pending");
    assert_eq!(first.page.fetch_usage(), (0, 0));
    let weak = Rc::downgrade(&first.page);
    drop(first);
    for _ in 0..6 {
        second.engine.collect_garbage_now();
        second.engine.drain_finalized();
    }
    assert!(weak.upgrade().is_none(), "queued fetch state retained disposed page");
    server.release.send(()).unwrap();
    second.eval(
        "globalThis.result='pending';fetch('/json').then(r=>r.json()).then(v=>result=v.answer,e=>result='ERROR '+e)",
    );
    second.until("result!=='pending'");
    assert_eq!(second.eval("result"), "42");
}

#[test]
fn fetch_head_abort_before_headers_rejects_without_panicking() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';globalThis.reason={head:true};globalThis.ac=new AbortController();fetch('/gated',{method:'HEAD',signal:ac.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason');ac.abort(reason)");
    env.until("result!=='pending'");
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.page.fetch_usage(), (0, 0));
    server.release.send(()).unwrap();
}

#[test]
fn fetch_abort_before_queued_null_headers_rejects_with_original_reason() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.result='pending';globalThis.reason={empty:true};globalThis.ac=new AbortController();fetch('/empty',{signal:ac.signal}).then(()=>result='unexpected',e=>result=e===reason?'aborted':'wrong reason')");
    let queued_headers = env.tasks.recv_timeout(Duration::from_secs(5)).unwrap();
    env.eval("ac.abort(reason)");
    assert_eq!(env.eval("result"), "aborted");
    queued_headers();
    assert_eq!(env.eval("result"), "aborted");
    assert_eq!(env.page.fetch_usage(), (0, 0));
}
