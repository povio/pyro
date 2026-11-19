//! JSON and CSS module scripts in module workers (patched JavaScriptCore): a
//! JSON import loads, a CSS one fails without a fetch (workers have no
//! CSSStyleSheet), and every module response's MIME type must match its
//! module type. The window side is in pyro-browser's tests/loading.rs.
use crate::support;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};
use support::network::*;

/// Serves fixed (content type, body) pairs by path and records the paths
/// requested.
struct Server {
    base: String,
    requested: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn new(files: &[(&str, &str, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<HashMap<String, (String, String)>> =
            Arc::new(files.iter().map(|(p, t, b)| ((*p).to_owned(), ((*t).to_owned(), (*b).to_owned()))).collect());
        let requested = Arc::new(Mutex::new(Vec::new()));
        let seen = requested.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let (files, seen) = (files.clone(), seen.clone());
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    if reader.read_line(&mut first).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    seen.lock().unwrap().push(path.clone());
                    let response = match files.get(&path) {
                        Some((kind, body)) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nAccess-Control-Allow-Origin: *\r\n\
                             Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        ),
                        None => "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n".to_owned(),
                    };
                    let _ = stream.write_all(response.as_bytes());
                });
            }
        });
        Self { base, requested }
    }
}

fn typed_modules() -> bool {
    unsafe { pyro_js::sys::modules::pyro_jsc_typed_modules_available() == 1 }
}

#[test]
fn module_workers_load_json_modules_and_refuse_css_ones() {
    if !typed_modules() {
        eprintln!("import attributes need the patched JavaScriptCore; skipping");
        return;
    }
    let server = Server::new(&[
        (
            "/worker.js",
            "text/javascript",
            "import data from './data.json' with { type: 'json' };
             const out = [data.n];
             const outcome = p => p.then(m => 'ok:' + JSON.stringify(m.default), e => e.constructor.name);
             out.push(await outcome(import('./style.css', { with: { type: 'css' } })));
             out.push(await outcome(import('./data.json')));
             out.push(await outcome(import('./text.json', { with: { type: 'json' } })));
             out.push(await outcome(import('./code.js', { with: { type: 'json' } })));
             out.push(await outcome(import('./ld.json', { with: { type: 'json' } })));
             postMessage(out.join());",
        ),
        ("/data.json", "application/json", r#"{"n": 7}"#),
        ("/ld.json", "application/ld+json; charset=utf-8", r#"[1]"#),
        ("/text.json", "text/plain", r#"{"n": 1}"#),
        ("/code.js", "text/javascript", "export default 1"),
        ("/style.css", "text/css", "p { color: red }"),
    ]);
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        "globalThis.msgs = []; globalThis.w = new Worker('{}/worker.js', {{ type: 'module' }});
         w.onmessage = e => msgs.push(e.data); w.onerror = e => msgs.push('error:' + e.message);",
        server.base
    ));
    env.until("msgs.length >= 1");
    assert_eq!(env.eval("msgs[0]"), "7,TypeError,TypeError,TypeError,TypeError,ok:[1]");
    let requested = server.requested.lock().unwrap().clone();
    assert!(!requested.iter().any(|p| p == "/style.css"), "a worker never fetches a CSS module: {requested:?}");
    // One fetch per (URL, type): the typed and untyped data.json are two.
    assert_eq!(requested.iter().filter(|p| *p == "/data.json").count(), 2, "{requested:?}");
    env.eval("w.terminate()");
}

#[test]
fn a_module_worker_script_needs_a_javascript_mime_type() {
    if !unsafe { pyro_js::sys::modules::pyro_jsc_modules_available() == 1 } {
        eprintln!("module SPI unavailable; skipping");
        return;
    }
    let server = Server::new(&[
        ("/sheet.css", "text/css", "p { color: red }"),
        ("/ok.js", "application/javascript", "postMessage('ran')"),
    ]);
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        "globalThis.msgs = [];
         for (const name of ['sheet.css', 'ok.js']) {{
           const w = new Worker('{}/' + name, {{ type: 'module' }});
           w.onmessage = e => msgs.push(name + ':' + e.data); w.onerror = () => msgs.push(name + ':error');
         }}",
        server.base
    ));
    env.until("msgs.length >= 2");
    assert_eq!(env.eval("msgs.sort().join()"), "ok.js:ran,sheet.css:error");
}

/// A message posted to a module worker before its graph has loaded waits for
/// the script: the worker's message queue is enabled once it ran (HTML "run
/// a worker"), so a handler the module sets after its imports gets it.
#[test]
fn a_module_worker_gets_the_messages_posted_while_its_imports_load() {
    if !unsafe { pyro_js::sys::modules::pyro_jsc_modules_available() == 1 } {
        eprintln!("module SPI unavailable; skipping");
        return;
    }
    let server = Server::new(&[
        (
            "/worker.js",
            "text/javascript",
            "import { tag } from './dep.js';
             self.onmessage = e => postMessage(tag + ':' + e.data);",
        ),
        ("/dep.js", "text/javascript", "export const tag = 'dep';"),
        ("/plain.js", "text/javascript", "self.onmessage = e => postMessage('plain:' + e.data);"),
    ]);
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        "globalThis.msgs = [];
         for (const name of ['worker.js', 'plain.js']) {{
           const w = new Worker('{}/' + name, {{ type: 'module' }});
           w.onmessage = e => msgs.push(e.data); w.onerror = e => msgs.push('error:' + e.message);
           w.postMessage('first'); w.postMessage('second');
         }}",
        server.base
    ));
    env.until("msgs.length >= 4");
    assert_eq!(env.eval("msgs.sort().join()"), "dep:first,dep:second,plain:first,plain:second");
}
