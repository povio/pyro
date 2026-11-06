//! EventSource (server-sent events): parsing, event dispatch, reconnection
//! with Last-Event-ID, failure, close, and the worker global.
use crate::support;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;
use support::network::*;

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|l| {
        let (n, v) = l.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

#[test]
fn event_source_streams_events_as_they_arrive() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        "globalThis.seen=[];globalThis.es=new EventSource('/sse');\
         es.onopen=()=>seen.push('open:'+es.readyState);\
         es.addEventListener('message.created',e=>seen.push(e.type+':'+e.data+':'+e.lastEventId+':'+(e instanceof MessageEvent)));\
         es.onmessage=e=>seen.push('message:'+e.data+':'+e.lastEventId+':'+(e.origin===new URL(document.URL).origin));\
         [es.readyState,EventSource.CONNECTING,es.OPEN,EventSource.CLOSED,es.url===new URL('/sse',document.URL).href,es.withCredentials].join()",
    );
    assert_eq!(env.eval("String([es.readyState,EventSource.CONNECTING,es.OPEN,EventSource.CLOSED])"), "0,0,1,2");
    env.until("seen.length===2");
    assert_eq!(env.eval("seen.join('|')"), "open:1|message.created:{\"n\":1}:1:true");
    server.release.send(()).unwrap();
    env.until("seen.length===3");
    assert_eq!(env.eval("seen[2]"), "message:{\"n\":2}:2:true");
    let (_, headers, _) = server.requests.lock().unwrap().iter().find(|(f, _, _)| f.contains("/sse")).cloned().unwrap();
    assert_eq!(header(&headers, "accept"), Some("text/event-stream"));
    assert_eq!(header(&headers, "cache-control"), Some("no-cache"));
    env.eval("es.close()");
    assert_eq!(env.eval("es.readyState"), "2");
    server.release.send(()).unwrap();
}

/// BOM, CR/CRLF line ends, comments, `retry`, `id`, empty `data` lines and
/// named events; at the end of the stream it reconnects after the retry
/// time with `Last-Event-ID`.
#[test]
fn event_source_parses_fields_and_reconnects_with_the_last_id() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval(
        "globalThis.seen=[];globalThis.es=new EventSource('/sse-retry');\
         es.onmessage=e=>seen.push('m:'+JSON.stringify(e.data)+':'+e.lastEventId);\
         es.addEventListener('ping',e=>seen.push('ping:'+e.data+':'+e.lastEventId));\
         es.onerror=()=>{seen.push('error:'+es.readyState); if (seen.filter(s=>s.startsWith('error')).length===2) es.close();};",
    );
    env.until("seen.length>=6");
    assert_eq!(env.eval("seen.join('|')"), "m:\"a\\n\":7|ping:b:7|error:0|m:\"a\\n\":7|ping:b:7|error:0");
    let requests: Vec<_> =
        server.requests.lock().unwrap().iter().filter(|(f, _, _)| f.contains("/sse-retry")).cloned().collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(header(&requests[0].1, "last-event-id"), None);
    assert_eq!(header(&requests[1].1, "last-event-id"), Some("7"));
    assert_eq!(env.eval("es.readyState"), "2");
    let _ = server.release.send(());
}

/// A response that is not `text/event-stream` (or not 200) fails the
/// connection for good; an invalid URL throws SyntaxError.
#[test]
fn event_source_fails_on_a_wrong_type_and_rejects_bad_urls() {
    let server = Server::new();
    let env = Env::new(&server.url);
    env.eval("globalThis.seen=[];const es=new EventSource('/json');es.onerror=()=>seen.push('error:'+es.readyState);es.onopen=()=>seen.push('open')");
    env.until("seen.length===1");
    assert_eq!(env.eval("seen.join()"), "error:2");
    assert_eq!(env.eval("try{new EventSource('http://[bad');'no'}catch(e){e.name}"), "SyntaxError");
    assert_eq!(env.eval("Object.prototype.toString.call(EventSource.prototype)"), "[object EventSource]");
    assert_eq!(env.eval("Object.getOwnPropertyDescriptor(globalThis,'EventSource').enumerable"), "false");
    let _ = server.release.send(());
}

/// A dedicated worker has EventSource too, resolving against its script.
#[test]
fn event_source_in_a_worker() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            std::thread::spawn(move || {
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
                    "/worker.js" => (
                        "application/javascript",
                        "const es = new EventSource('events'); const out = [typeof EventSource, es.url.endsWith('/events')];\
                         es.onmessage = e => { out.push(e.data); es.close(); postMessage(out.join()); };"
                            .to_owned(),
                    ),
                    "/events" => ("text/event-stream", "data: from-worker\n\n".to_owned()),
                    _ => ("text/plain", String::new()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    let env = Env::new(&format!("{base}/page"));
    env.eval(&format!(
        "globalThis.msgs=[];globalThis.w=new Worker('{base}/worker.js');w.onmessage=e=>msgs.push(e.data);w.onerror=e=>msgs.push('error:'+e.message);"
    ));
    env.until("msgs.length>=1");
    assert_eq!(env.eval("msgs[0]"), "function,true,from-worker");
    env.eval("w.terminate()");
    std::thread::sleep(Duration::from_millis(50));
}
