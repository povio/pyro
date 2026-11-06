//! `WebSocket` and `CloseEvent` in a window and a dedicated worker, against a
//! loopback RFC 6455 server of this file (which also serves `/worker.js`).
use crate::support;
use base64::Engine;
use sha1::{Digest, Sha1};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use support::network::*;

/// `/echo` echoes data messages and answers a Close with its payload;
/// `/server-close` closes with 4001 "bye"; `/set-cookie` sets `ws=1`;
/// `/cookie` sends the handshake's Cookie header as its first message;
/// `/stall` never answers the handshake; `/worker.js` (plain HTTP) is the
/// script the test gives.
struct WsServer {
    base: String,
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

impl WsServer {
    fn new(script: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let (log2, script) = (log.clone(), script.to_owned());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (log, script) = (log2.clone(), script.clone());
                thread::spawn(move || serve(stream, &log, &script));
            }
        });
        WsServer { base: format!("http://127.0.0.1:{port}"), port, log }
    }
}

fn read_frame(reader: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    reader.read_exact(&mut head).ok()?;
    let mut len = (head[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        reader.read_exact(&mut b).ok()?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        reader.read_exact(&mut b).ok()?;
        len = u64::from_be_bytes(b);
    }
    let mut key = [0u8; 4];
    reader.read_exact(&mut key).ok()?;
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).ok()?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= key[i % 4];
    }
    Some((head[0] & 0x0f, payload))
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) {
    let mut out = vec![0x80 | opcode];
    match payload.len() {
        n @ 0..=125 => out.push(n as u8),
        n @ 126..=0xffff => {
            out.push(126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    let _ = stream.write_all(&out);
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>, script: &str) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 {
            return;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let hv = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    if hv("upgrade").is_empty() {
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{script}",
            script.len()
        );
        return;
    }
    log.lock().unwrap().push(format!("{path} origin={} cookie={}", hv("origin"), hv("cookie")));
    if path == "/stall" {
        thread::sleep(Duration::from_secs(10));
        return;
    }
    let digest = Sha1::digest(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", hv("sec-websocket-key")));
    let accept = base64::engine::general_purpose::STANDARD.encode(digest);
    let mut response =
        format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n");
    if let Some(first) = hv("sec-websocket-protocol").split(',').next().filter(|p| !p.is_empty()) {
        response.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", first.trim()));
    }
    if path == "/set-cookie" {
        response.push_str("Set-Cookie: ws=1; Path=/\r\n");
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    if path == "/server-close" {
        let mut payload = 4001u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"bye");
        write_frame(&mut stream, 0x8, &payload);
    }
    if path == "/cookie" {
        write_frame(&mut stream, 0x1, hv("cookie").as_bytes());
    }
    while let Some((opcode, payload)) = read_frame(&mut reader) {
        match opcode {
            0x8 => {
                if path != "/server-close" {
                    write_frame(&mut stream, 0x8, &payload);
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            0x1 | 0x2 => write_frame(&mut stream, opcode, &payload),
            _ => {}
        }
    }
}

#[test]
fn constructor_validates_url_and_protocols() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/dir/page", server.base));
    env.eval(&format!("globalThis.origin_ = '{}'", server.base));
    let names = env.eval(
        r#"const out = [];
        const code = f => { try { f(); return 'none'; } catch (e) { return e.name; } };
        for (const url of ['ws://foo bar/', 'ftp://x/', 'about:blank', origin_ + '/#', '#test'])
            out.push(code(() => new WebSocket(url)));
        out.push(code(() => new WebSocket('ws://127.0.0.1/', ['echo', 'eCho'])));
        out.push(code(() => new WebSocket('ws://127.0.0.1/', '/echo')));
        out.push(code(() => new WebSocket('ws://127.0.0.1/', 'a b')));
        out.push(code(() => new WebSocket()));
        out.join()"#,
    );
    assert_eq!(
        names,
        "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,TypeError"
    );
    let urls = env.eval(
        r#"const list = [];
        for (const url of [origin_ + '/x', 'https://127.0.0.1:1/', 'rel?q', '']) {
            const ws = new WebSocket(url); list.push(ws.url); ws.close();
        }
        list.join(' ')"#,
    );
    assert_eq!(
        urls,
        format!(
            "ws://127.0.0.1:{p}/x wss://127.0.0.1:1/ ws://127.0.0.1:{p}/dir/rel?q ws://127.0.0.1:{p}/dir/page",
            p = server.port
        )
    );
    let shape = env.eval(
        r#"const ws = new WebSocket(origin_ + '/stall', {protocols: ['a', 'b']});
        const r = [WebSocket.length, WebSocket.CONNECTING, WebSocket.prototype.CLOSED, ws.OPEN, ws.readyState,
            ws.protocol === '', ws.extensions === '', ws.bufferedAmount, ws.binaryType,
            Object.prototype.toString.call(ws), ws instanceof EventTarget, ws.onopen];
        ws.binaryType = 'nope'; r.push(ws.binaryType);
        ws.binaryType = 'arraybuffer'; r.push(ws.binaryType);
        ws.onmessage = () => {}; ws.onmessage = 2; r.push(ws.onmessage);
        r.push(Object.getOwnPropertyDescriptor(WebSocket.prototype, 'url').enumerable);
        const code2 = f => { try { f(); return 'none'; } catch (e) { return e.name; } };
        r.push(code2(() => ws.send('x')));
        for (const c of [0, 500, NaN, 'string', null, 65536 + 1000]) r.push(code2(() => ws.close(c)));
        r.push(code2(() => ws.close(1000, 'x'.repeat(124))));
        r.push(code2(() => Object.getOwnPropertyDescriptor(WebSocket.prototype, 'url').get.call({})));
        ws.close(); r.push(ws.readyState);
        r.join()"#,
    );
    assert_eq!(
        shape,
        "1,0,3,1,0,true,true,0,blob,[object WebSocket],true,,blob,arraybuffer,,true,InvalidStateError,\
         InvalidAccessError,InvalidAccessError,InvalidAccessError,InvalidAccessError,InvalidAccessError,\
         InvalidAccessError,SyntaxError,TypeError,2"
    );
}

#[test]
fn messages_round_trip_with_events_in_order() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        r#"globalThis.log = []; globalThis.done = false;
        const ws = new WebSocket('ws://127.0.0.1:{}/echo', ['chat', 'other']);
        globalThis.ws = ws;
        ws.binaryType = 'arraybuffer';
        ws.onopen = e => {{
            log.push('open:' + ws.protocol + ':' + ws.readyState + ':' + e.isTrusted);
            ws.send('héllo');
            log.push('buffered:' + ws.bufferedAmount);
            ws.send(new Uint8Array([1, 2, 3, 4]).subarray(1, 3));
            ws.send(new Uint8Array([9, 8]).buffer);
            ws.send(new Blob(['blob!']));
        }};
        let count = 0;
        ws.addEventListener('message', e => {{
            const d = e.data;
            log.push(typeof d === 'string' ? 'text:' + d : 'bytes:' + Array.from(new Uint8Array(d)).join('.'));
            log.push('origin:' + (e.origin === 'ws://127.0.0.1:{}') + ':' + (e instanceof MessageEvent) + ':' + e.isTrusted);
            if (++count === 4) {{ ws.close(1000, 'bye'); log.push('closing:' + ws.readyState); }}
        }});
        ws.onerror = () => log.push('error');
        ws.onclose = e => {{
            log.push('close:' + e.wasClean + ':' + e.code + ':' + e.reason + ':' + ws.readyState + ':'
                + (e instanceof CloseEvent) + ':' + ws.bufferedAmount);
            done = true;
        }};"#,
        server.port, server.port
    ));
    env.until("done");
    assert_eq!(
        env.eval("log.join('|')"),
        "open:chat:1:true|buffered:6|text:héllo|origin:true:true:true|bytes:2.3|origin:true:true:true|\
         bytes:9.8|origin:true:true:true|bytes:98.108.111.98.33|origin:true:true:true|closing:2|\
         close:true:1000:bye:3:true:0"
    );
    assert_eq!(lp_webapi::websocket::open_websockets(), 0);
}

#[test]
fn blobs_server_close_and_connecting_close() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        r#"globalThis.log = []; globalThis.pending = 3;
        const echo = new WebSocket('ws://127.0.0.1:{p}/echo');
        echo.onopen = () => echo.send(new Uint8Array([65, 66]));
        echo.onmessage = async e => {{
            log.push('blob:' + (e.data instanceof Blob) + ':' + await e.data.text());
            echo.close();
        }};
        echo.onclose = e => {{ log.push('echo:' + e.code + ':' + e.wasClean); pending--; }};
        const server = new WebSocket('ws://127.0.0.1:{p}/server-close');
        server.onclose = e => {{ log.push('server:' + e.code + ':' + e.reason + ':' + e.wasClean); pending--; }};
        const early = new WebSocket('ws://127.0.0.1:{p}/stall');
        early.onerror = () => log.push('early-error:' + early.readyState);
        early.onclose = e => {{ log.push('early-close:' + e.code + ':' + e.wasClean); pending--; }};
        early.close();
        log.push('early:' + early.readyState);"#,
        p = server.port
    ));
    env.until("pending === 0");
    let log = env.eval("log.sort().join('|')");
    assert_eq!(log, "blob:true:AB|early-close:1006:false|early-error:3|early:2|echo:1005:true|server:4001:bye:true");
}

#[test]
fn handshake_carries_cookies_and_set_cookie_is_seen_after_open() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        r#"globalThis.log = []; globalThis.done = false;
        document.cookie = 'sid=42; Path=/';
        const ws = new WebSocket('ws://127.0.0.1:{p}/set-cookie');
        log.push('before:' + document.cookie.includes('ws=1'));
        ws.onopen = () => {{
            log.push('open:' + document.cookie.includes('ws=1'));
            const second = new WebSocket('ws://127.0.0.1:{p}/cookie');
            second.onmessage = e => {{ log.push('sent:' + e.data); second.close(); ws.close(); done = true; }};
        }};"#,
        p = server.port
    ));
    env.until("done");
    let handshakes = server.log.lock().unwrap().clone();
    assert_eq!(env.eval("log.join('|')"), "before:false|open:true|sent:sid=42; ws=1", "{handshakes:?}");
    assert!(handshakes[0].contains(&format!("origin=http://127.0.0.1:{}", server.port)), "{handshakes:?}");
}

#[test]
fn close_event_constructor_and_lazy_globals() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/page", server.base));
    assert_eq!(
        env.eval(
            r#"const kind = typeof WebSocket;
            const d = Object.getOwnPropertyDescriptor(globalThis, 'WebSocket');
            const e = new CloseEvent('close', {code: 70000, reason: 7, wasClean: 1, bubbles: true});
            const plain = new CloseEvent('x');
            [kind, typeof CloseEvent, d && d.enumerable, e.code, e.reason, e.wasClean, e.bubbles,
             e.isTrusted, plain.code, plain.reason, plain.wasClean, CloseEvent.length, e instanceof Event].join()"#
        ),
        "function,function,false,4464,7,true,true,false,0,,false,1,true"
    );
    // Assigning the name replaces the interface, as for any global.
    assert_eq!(env.eval("WebSocket = 5; WebSocket"), "5");
}

#[test]
fn page_dispose_aborts_open_connections() {
    let server = WsServer::new("");
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        "globalThis.opened = false; globalThis.ws = new WebSocket('ws://127.0.0.1:{}/echo'); ws.onopen = () => opened = true;",
        server.port
    ));
    env.until("opened");
    assert_eq!(lp_webapi::websocket::open_websockets(), 1);
    env.page.dispose();
    assert_eq!(lp_webapi::websocket::open_websockets(), 0);
}

#[test]
fn worker_opens_and_echoes() {
    let script = r#"const ws = new WebSocket(location.origin.replace('http', 'ws') + '/echo');
        ws.binaryType = 'arraybuffer';
        const out = [typeof CloseEvent, ws.readyState];
        ws.onopen = () => ws.send('from worker');
        ws.onmessage = e => { out.push(e.data); ws.close(3000, 'w'); };
        ws.onclose = e => { out.push(e.code, e.reason, e.wasClean); postMessage(out.join()); };"#;
    let server = WsServer::new(script);
    let env = Env::new(&format!("{}/page", server.base));
    env.eval(&format!(
        "globalThis.msgs = []; globalThis.w = new Worker('{}/worker.js'); w.onmessage = e => msgs.push(e.data);
         w.onerror = e => msgs.push('error:' + e.message);",
        server.base
    ));
    env.until("msgs.length >= 1");
    assert_eq!(env.eval("msgs[0]"), "function,0,from worker,3000,w,true");
    env.eval("w.terminate()");
    assert!(lp_webapi::worker::join_stopped(Duration::from_secs(5)));
}
