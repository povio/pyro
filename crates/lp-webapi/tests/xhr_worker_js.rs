//! XMLHttpRequest in workers, `blob:` URLs through XHR and fetch, HEAD of a
//! `data:` URL. The worker tests run against a loopback server of this file
//! because the shared one serves JSON on every unknown path, and a worker
//! script has to parse.
use crate::support;
use lp_webapi::*;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    thread,
    time::Duration,
};
use support::network::*;

/// Serves `/worker.js` (the script `script` builds from this server's origin),
/// `/data.txt`, `/json` and `/stall` (a response after three seconds, for
/// timeouts and terminations to beat), one thread per connection so a stalled
/// one blocks nothing else.
struct ScriptServer {
    base: String,
}
impl ScriptServer {
    fn new(script: impl FnOnce(&str) -> String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let script = script(&base);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let script = script.clone();
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
                    let (kind, body) = match path.as_str() {
                        "/worker.js" => ("application/javascript", script),
                        "/data.txt" => ("text/plain", "bottom".to_owned()),
                        "/stall" => {
                            thread::sleep(Duration::from_secs(3));
                            ("text/plain", "late".to_owned())
                        }
                        _ => ("application/json", "{\"answer\":42}".to_owned()),
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        Self { base }
    }
}

#[test]
fn worker_exposes_xhr_without_response_xml_and_resolves_against_its_script() {
    let worker_host = ScriptServer::new(|base| {
        format!(
            r#"const base = '{base}';
        const out = [];
        out.push(typeof XMLHttpRequest, typeof XMLHttpRequestUpload, 'responseXML' in XMLHttpRequest.prototype,
            XMLHttpRequest.prototype.hasOwnProperty('responseXML'), 'responseText' in XMLHttpRequest.prototype);
        const x = new XMLHttpRequest();
        out.push(x instanceof EventTarget, x.upload instanceof XMLHttpRequestUpload, x.constructor.name);
        x.responseType = 'document'; out.push(x.responseType === '');
        x.responseType = 'json'; out.push(x.responseType);
        // A worker may give a synchronous request a timeout and a responseType.
        x.open('GET', base + '/json', false);
        try {{ x.timeout = 5000; out.push('timeout-ok'); }} catch (e) {{ out.push(e.name); }}
        x.send();
        out.push(x.status, JSON.stringify(x.response));
        // Relative URLs resolve against the worker script's URL.
        const r = new XMLHttpRequest(); r.open('GET', 'data.txt', false); r.send(); out.push(r.responseText, r.responseURL);
        // A synchronous timeout is a TimeoutError exception.
        const t = new XMLHttpRequest(); t.open('GET', base + '/stall', false); t.timeout = 100;
        try {{ t.send(); out.push('no-throw'); }} catch (e) {{ out.push(e.name, t.readyState); }}
        // Asynchronous requests run on the worker's loop, with ready states and progress events.
        const a = new XMLHttpRequest(); const seen = [];
        a.onreadystatechange = () => seen.push(a.readyState);
        a.onload = () => seen.push('load:' + a.responseText);
        a.onloadend = () => {{ out.push(seen.join('/')); postMessage(out.join()); }};
        a.open('GET', 'data.txt'); a.send();"#
        )
    });
    let env = Env::new(&format!("{}/page", worker_host.base));
    env.eval(&format!(
        "globalThis.msgs = []; globalThis.w = new Worker('{}/worker.js');
         w.onmessage = e => msgs.push(e.data); w.onerror = e => msgs.push('error:' + e.message);",
        worker_host.base
    ));
    env.until("msgs.length >= 1");
    assert_eq!(
        env.eval("msgs[0]"),
        format!(
            "function,function,false,false,true,true,true,XMLHttpRequest,true,json,timeout-ok,200,{{\"answer\":42}},bottom,{}/data.txt,TimeoutError,4,1/2/3/4/load:bottom",
            worker_host.base
        )
    );
    env.eval("w.terminate()");
    assert!(join_stopped(Duration::from_secs(5)), "the worker thread ends after terminate()");
}

#[test]
fn terminating_a_worker_ends_its_pending_and_parked_requests() {
    let host = ScriptServer::new(|base| {
        format!(
            r#"const a = new XMLHttpRequest(); a.open('GET', '{base}/stall'); a.responseType = 'blob';
        a.onloadend = () => postMessage('loaded');
        a.send();
        postMessage('sent');
        // Parks this thread; terminate() must still end it, long before the response.
        const s = new XMLHttpRequest(); s.open('GET', '{base}/stall', false);
        try {{ s.send(); postMessage('completed'); }} catch (e) {{ postMessage('threw:' + e.name); }}"#
        )
    });
    let env = Env::new(&format!("{}/page", host.base));
    env.eval(&format!(
        "globalThis.msgs = []; globalThis.w = new Worker('{}/worker.js');
         w.onmessage = e => {{ msgs.push(e.data); if (e.data === 'sent') w.terminate(); }};
         w.onerror = e => msgs.push('error:' + e.message);",
        host.base
    ));
    env.until("msgs.length >= 1");
    assert_eq!(env.eval("msgs.join()"), "sent");
    assert!(join_stopped(Duration::from_secs(5)), "the worker thread ends while its requests are in flight");
    env.drain_for(Duration::from_millis(200));
    assert_eq!(env.eval("msgs.join()"), "sent", "nothing is delivered from a terminated worker");
}

#[test]
fn blob_urls_serve_whole_objects_and_single_byte_ranges() {
    let server = Server::new();
    let env = Env::new(&server.url);
    assert_eq!(
        env.eval(
            r#"globalThis.log = [];
            const blob = new Blob(['A simple Hello, World! example'], {type: 'text/plain'});
            globalThis.url = URL.createObjectURL(blob);
            const x = new XMLHttpRequest(); x.open('GET', url, false); x.send();
            log.push(x.status, x.getResponseHeader('Content-Type'), x.getResponseHeader('Content-Length'),
                String(x.getResponseHeader('Content-Range')), x.responseText, x.responseURL === url);
            const r = new XMLHttpRequest(); r.open('GET', url, false); r.setRequestHeader('Range', 'bytes= \t9-21'); r.send();
            log.push(r.status, r.statusText, r.getResponseHeader('Content-Length'), r.getResponseHeader('Content-Range'),
                r.responseText, /content-range: bytes 9-21\/30/i.test(r.getAllResponseHeaders()));
            const suffix = new XMLHttpRequest(); suffix.open('GET', url, false); suffix.setRequestHeader('Range', 'bytes=-7'); suffix.send();
            log.push(suffix.status, suffix.getResponseHeader('Content-Range'), suffix.responseText);
            const open = new XMLHttpRequest(); open.open('GET', url, false); open.setRequestHeader('Range', 'bytes=23-'); open.send();
            log.push(open.status, open.getResponseHeader('Content-Range'), open.responseText);
            for (const range of ['bytes=100-', 'bytes=30-', 'bytes=10-5', 'bytes=0-5,15-', 'bytes=-', '', 'byte=0-']) {
                const bad = new XMLHttpRequest(); bad.open('GET', url, false); bad.setRequestHeader('Range', range);
                try { bad.send(); log.push('no-throw'); } catch (e) { log.push(e.name + ':' + bad.status); }
            }
            const post = new XMLHttpRequest(); post.open('POST', url, false);
            try { post.send('x'); log.push('no-throw'); } catch (e) { log.push(e.name); }
            const untyped = URL.createObjectURL(new Blob(['no type']));
            const u = new XMLHttpRequest(); u.open('GET', untyped, false); u.send();
            log.push(u.status, JSON.stringify(u.getResponseHeader('Content-Type')), u.responseText);
            log.join('|')"#
        ),
        "200|text/plain|30|null|A simple Hello, World! example|true|206|Partial Content|13|bytes 9-21/30|Hello, World!|true|206|bytes 23-29/30|example|206|bytes 23-29/30|example|NetworkError:0|NetworkError:0|NetworkError:0|NetworkError:0|NetworkError:0|NetworkError:0|NetworkError:0|NetworkError|200|\"\"|no type"
    );
    // Asynchronously, through XHR events and through fetch; a revoked URL is a network error.
    env.eval(
        r#"globalThis.result = 'pending'; globalThis.events = [];
        const a = new XMLHttpRequest(); a.open('GET', url); a.responseType = 'text'; a.setRequestHeader('Range', 'bytes=0-0');
        a.onload = () => events.push('load:' + a.status + ':' + a.responseText);
        a.onerror = () => events.push('error');
        a.onloadend = () => {
            fetch(url, {headers: {Range: 'bytes=11-'}}).then(async res => {
                events.push('fetch:' + res.status + ':' + res.headers.get('content-range') + ':' + await res.text());
                URL.revokeObjectURL(url);
                return fetch(url);
            }).then(() => events.push('revoked-ok'), e => events.push('revoked:' + e.name))
              .then(() => result = events.join());
        };
        a.send();"#,
    );
    env.until("result !== 'pending'");
    assert_eq!(env.eval("result"), "load:206:A,fetch:206:bytes 11-29/30:llo, World! example,revoked:TypeError");
}

#[test]
fn head_of_a_data_url_has_no_body() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        r#"globalThis.result = 'pending';
        const h = new XMLHttpRequest(); h.open('HEAD', 'data:text/plain,Hello, World!');
        h.onloadend = () => result = [h.status, JSON.stringify(h.responseText), h.getResponseHeader('Content-Type'),
            /content-length/i.test(h.getAllResponseHeaders())].join();
        h.send();"#,
    );
    env.until("result !== 'pending'");
    assert_eq!(env.eval("result"), "200,\"\",text/plain,false");
    assert_eq!(
        env.eval(
            r#"const g = new XMLHttpRequest(); g.open('GET', 'data:text/plain,Hello, World!', false); g.send(); g.responseText"#
        ),
        "Hello, World!"
    );
}
