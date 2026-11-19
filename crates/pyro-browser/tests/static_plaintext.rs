use pyro_browser::{BrowserThread, NavigateOpts, PageId};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TEXT: &str = "\nTP-3973 original attachment bytes München\n<script>globalThis.plainExecuted=true</script>&amp;<pre>literal</pre>\n";
const SNIFF_TEXT: &str = "<meta charset=windows-1252>München &lt;literal&gt;";

struct Server {
    origin: String,
    seen: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                }
                if path == "/stop" {
                    break;
                }
                log.lock().unwrap().push(path.clone());
                let (mime, body): (&str, &[u8]) = if path == "/sniff" {
                    ("text/plain", SNIFF_TEXT.as_bytes())
                } else if path == "/latin" {
                    ("text/plain; charset=windows-1252", b"caf\xe9 <b>literal</b>")
                } else {
                    ("text/plain; charset=utf-8", TEXT.as_bytes())
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Disposition: inline; filename=\"proof.txt\"\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        Self { origin, seen, thread: Some(thread) }
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        let _ = self.thread.take().unwrap().join();
    }
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pyro-static-plaintext-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>Mounted SPA</title><p>SPA index</p>").unwrap();
        std::fs::write(dir.join("asset.txt"), "exact static file <b>literal</b>").unwrap();
        Self(dir)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn eval(thread: &BrowserThread, page: PageId, source: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn navigate(thread: &mut BrowserThread, page: PageId, url: &str) {
    thread.browser.navigate(page, NavigateOpts::address_bar(url)).unwrap();
    let condition = format!("location.href === {url:?} && document.readyState === 'complete'");
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |browser| {
        let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(&condition, None, 1).unwrap().to_bool()
    });
    assert_eq!(result, pyro_runtime::RunOutcome::Done);
}

#[test]
fn static_network_fallback_keeps_assets_and_fetches_missing_documents_at_exact_urls() {
    let server = Server::new();
    let files = Files::new();
    for direct in [true, false] {
        let mut thread = crate::browser_thread().unwrap();
        let static_files = pyro_net::StaticFiles::new(vec![(server.origin.clone(), files.0.clone())])
            .unwrap()
            .with_spa_fallback(false);
        thread
            .enable_network(pyro_net::NetConfig { static_files, static_direct: direct, ..Default::default() })
            .unwrap();
        let page = thread.browser.create_page().unwrap();
        navigate(&mut thread, page, &format!("{}/asset.txt", server.origin));
        assert_eq!(eval(&thread, page, "document.body.textContent"), "exact static file <b>literal</b>");
        assert!(server.take().is_empty());
        let path = "/api/media/proof.txt?signature=signed%2Bvalue&disposition=inline";
        let url = format!("{}{path}", server.origin);
        navigate(&mut thread, page, &url);
        assert_eq!(server.take(), [path]);
        assert_eq!(eval(&thread, page, "location.href"), url);
        assert_eq!(eval(&thread, page, "document.contentType"), "text/plain");
        assert_eq!(eval(&thread, page, "document.body.textContent"), TEXT);
        assert_eq!(eval(&thread, page, "document.body.firstElementChild.tagName"), "PRE");
        assert_eq!(eval(&thread, page, "document.querySelectorAll('script,b,meta').length"), "0");
        assert_eq!(eval(&thread, page, "typeof globalThis.plainExecuted"), "undefined");
    }
}

#[test]
fn default_static_spa_fallback_is_unchanged() {
    let server = Server::new();
    let files = Files::new();
    for direct in [true, false] {
        let mut thread = crate::browser_thread().unwrap();
        let static_files = pyro_net::StaticFiles::new(vec![(server.origin.clone(), files.0.clone())]).unwrap();
        thread
            .enable_network(pyro_net::NetConfig { static_files, static_direct: direct, ..Default::default() })
            .unwrap();
        let page = thread.browser.create_page().unwrap();
        navigate(&mut thread, page, &format!("{}/deep/missing?query=retained", server.origin));
        assert_eq!(eval(&thread, page, "document.title"), "Mounted SPA");
        assert_eq!(eval(&thread, page, "document.contentType"), "text/html");
        assert!(server.take().is_empty());
    }
}

#[test]
fn plaintext_decoding_honors_transport_charset_but_not_literal_meta_markup() {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(pyro_net::NetConfig::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    navigate(&mut thread, page, &format!("{}/latin", server.origin));
    assert_eq!(eval(&thread, page, "document.body.textContent"), "café <b>literal</b>");
    assert_eq!(eval(&thread, page, "document.characterSet"), "windows-1252");
    navigate(&mut thread, page, &format!("{}/sniff", server.origin));
    assert_eq!(eval(&thread, page, "document.body.textContent"), SNIFF_TEXT);
    assert_eq!(eval(&thread, page, "document.characterSet"), "UTF-8");
}
