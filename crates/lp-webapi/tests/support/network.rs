#![allow(dead_code)]
//! Real JSC + isolated loopback HTTP, with an explicit browser task queue.
use lp_js::{ClassDef, Engine, JsClass, Persistent, Realm, RealmOptions};
use lp_webapi::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct WindowData {
    pub page: Rc<PageDom>,
    pub target: EventTargetData,
}
impl WindowHost for WindowData {
    fn page(&self) -> &Rc<PageDom> {
        &self.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}
pub struct Window;
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
/// Shared, ordered record of delivery frontiers (from the hook) and JS markers
/// (from `__log`), so tests can prove the hook runs before dependent JS.
pub type Log = Rc<RefCell<Vec<String>>>;
struct Hooks {
    tx: mpsc::Sender<Task>,
    pub posts: Arc<AtomicUsize>,
    log: Log,
}
impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        task();
    }
    fn navigate(&self, _: NavigationRequest) {}
    fn network_delivery(&self, id: lp_net::RequestId, d: lp_net::NetworkDelivery) {
        self.log.borrow_mut().push(format!("delivery {} r={} b={} t={}", id.0, d.response, d.data_bytes, d.terminal));
    }
    fn remote_poster(&self) -> Option<RemotePoster> {
        let tx = self.tx.clone();
        let posts = self.posts.clone();
        Some(Arc::new(move |task| {
            posts.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(task);
        }))
    }
}
type Timers = Rc<RefCell<BTreeMap<u32, (Instant, Persistent)>>>;
pub struct Env {
    timers: Timers,
    pub engine: Rc<Engine>,
    pub realm: Realm,
    pub page: Rc<PageDom>,
    pub tasks: mpsc::Receiver<Task>,
    pub posts: Arc<AtomicUsize>,
    pub log: Log,
}
impl Env {
    /// Index of the first log entry satisfying `pred`.
    pub fn log_index(&self, pred: impl Fn(&str) -> bool) -> Option<usize> {
        self.log.borrow().iter().position(|l| pred(l))
    }
    /// Parsed `(response, data_bytes, terminal)` frontiers, in hook order.
    /// Asserts they are cumulative: never regress, and nothing after a terminal.
    pub fn frontiers(&self) -> Vec<(bool, u64, bool)> {
        let parsed: Vec<(bool, u64, bool)> = self
            .log
            .borrow()
            .iter()
            .filter_map(|l| l.strip_prefix("delivery "))
            .map(|l| {
                let f: Vec<&str> = l.split(' ').collect();
                let v = |k: &str| f.iter().find_map(|x| x.strip_prefix(k)).unwrap().to_string();
                (v("r=") == "true", v("b=").parse().unwrap(), v("t=") == "true")
            })
            .collect();
        for w in parsed.windows(2) {
            let ((r0, b0, t0), (r1, b1, _)) = (w[0], w[1]);
            assert!(!t0, "no frontier may follow a terminal one: {parsed:?}");
            assert!(r1 >= r0 && b1 >= b0, "frontiers must be cumulative: {parsed:?}");
        }
        parsed
    }
    /// Runs queued browser tasks for a bounded time (no product timing involved).
    pub fn drain_for(&self, total: Duration) {
        let deadline = Instant::now() + total;
        while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            match self.tasks.recv_timeout(wait) {
                Ok(task) => task(),
                Err(_) => break,
            }
        }
    }
    pub fn new(url: &str) -> Self {
        Self::with_engine(url, Rc::new(Engine::new().unwrap()))
    }
    pub fn with_engine(url: &str, engine: Rc<Engine>) -> Self {
        register(&engine);
        let class = engine.register::<Window>();
        let dom = Rc::new(RefCell::new(lp_dom::Dom::new()));
        let doc = lp_dom::parse::parse_document(&dom, url, "<body></body>");
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let page = PageDom::new(dom, doc);
        *page.net.borrow_mut() = Some(lp_net::NetClient::new(Default::default()).unwrap());
        let (tx, tasks) = mpsc::channel();
        let posts = Arc::new(AtomicUsize::new(0));
        let log: Log = Rc::default();
        page.set_hooks(Some(Rc::new(Hooks { tx, posts: posts.clone(), log: log.clone() })));
        let realm = engine.create_realm(RealmOptions { global_class: Some(class), ..Default::default() }).unwrap();
        let key = page.create_window_target(doc);
        realm
            .set_global_data::<Window>(WindowData { page: page.clone(), target: EventTargetData::window(&page, key) })
            .unwrap();
        let timers: Timers = Rc::new(RefCell::new(BTreeMap::new()));
        {
            let s = realm.scope().unwrap();
            let scheduled = timers.clone();
            let ids = Cell::new(0u32);
            let set = s
                .function("setTimeout", move |s, _, args| {
                    let ms = args.get(1).to_number()?.max(0.);
                    let callback = Persistent::new(s, args.get(0));
                    let id = ids.get() + 1;
                    ids.set(id);
                    scheduled.borrow_mut().insert(id, (Instant::now() + Duration::from_secs_f64(ms / 1000.), callback));
                    Ok(s.number(id as f64))
                })
                .unwrap();
            let scheduled = timers.clone();
            let clear = s
                .function("clearTimeout", move |s, _, args| {
                    let id = args.get(0).to_number()? as u32;
                    let removed = scheduled.borrow_mut().remove(&id);
                    drop(removed);
                    Ok(s.undefined())
                })
                .unwrap();
            let marks = log.clone();
            let mark = s
                .function("__log", move |s, _, args| {
                    marks.borrow_mut().push(args.get(0).to_string()?);
                    Ok(s.undefined())
                })
                .unwrap();
            s.global().set("__log", mark.value()).unwrap();
            s.global().set("setTimeout", set.value()).unwrap();
            s.global().set("clearTimeout", clear.value()).unwrap();
            install(&s, page.clone()).unwrap();
        }
        Self { engine, realm, page, tasks, posts, timers, log }
    }
    pub fn eval(&self, source: &str) -> String {
        let s = self.realm.scope().unwrap();
        match s.eval(source, Some("fetch-test.js"), 1) {
            Ok(value) => value.to_string().unwrap(),
            Err(error) => panic!("{}\n{source}", error.details(&s).text()),
        }
    }
    pub fn until(&self, condition: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.eval(condition) != "true" {
            let wait = deadline.saturating_duration_since(Instant::now());
            assert!(!wait.is_zero(), "timeout waiting for {condition}");
            let next = self.timers.borrow().values().map(|(when, _)| *when).min();
            let wait = next.map_or(wait, |when| wait.min(when.saturating_duration_since(Instant::now())));
            match self.tasks.recv_timeout(wait) {
                Ok(task) => task(),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let due: Vec<_> = self
                        .timers
                        .borrow()
                        .iter()
                        .filter(|(_, (when, _))| *when <= Instant::now())
                        .map(|(id, _)| *id)
                        .collect();
                    for id in due {
                        let callback = self.timers.borrow_mut().remove(&id);
                        if let Some((_, callback)) = callback {
                            let s = self.realm.scope().unwrap();
                            callback.get(&s).as_object().unwrap().call(None, &[]).unwrap();
                        }
                    }
                }
                Err(error) => panic!("task queue closed: {error}"),
            }
            self.page.after_task();
        }
    }
    pub fn finish_network_tasks(&self) {
        while self.page.fetch_usage().0 != 0 {
            self.tasks.recv_timeout(Duration::from_secs(5)).unwrap()();
            self.page.after_task();
        }
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        self.page.dispose();
        self.timers.borrow_mut().clear();
        self.realm.dispose();
    }
}

type RecordedRequest = (String, String, Vec<u8>);
pub struct Server {
    pub url: String,
    stop: Arc<AtomicBool>,
    pub release: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
}
impl Server {
    pub fn new() -> Self {
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
                    "/gated-failure" | "/gated-text" => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 10\r\n\r\nfirst");
                        let _ = stream.flush();
                        let _ = gate.recv_timeout(Duration::from_secs(5));
                        if path == "/gated-text" { let _ = stream.write_all(b"later"); }
                        continue;
                    }
                    "/gated" => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 6\r\n\r\n");
                        let _ = stream.flush(); let _ = gate.recv_timeout(Duration::from_secs(5));
                        let _ = stream.write_all(b"abcdef"); continue;
                    }
                    "/split-utf8" => {
                        let _=stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\n\r\n\xe2\x82");
                        let _=stream.flush();let _=gate.recv_timeout(Duration::from_secs(5));let _=stream.write_all(b"\xac");continue;
                    }
                    "/latin1" => ("200 OK", "Content-Type: text/plain; charset=windows-1252\r\n".into(), vec![0xe9]),
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
