//! Resource Timing data on response heads (`ResponseHead::timing`) against a
//! keep-alive HTTP/1.1 server: a new connection's phases, a reused one,
//! redirects, the Timing-Allow-Origin check and cache hits.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lp_net::*;

/// Serves every request of a connection until it closes.
fn start() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    let (status, extra) = match path.as_str() {
                        "/redirect" => ("302 Found", "Location: /final\r\n"),
                        "/tao" => ("200 OK", "Timing-Allow-Origin: https://other.test\r\n"),
                        "/cached" => ("200 OK", "Cache-Control: max-age=600\r\n"),
                        _ => ("200 OK", ""),
                    };
                    let body = "body";
                    let out = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len());
                    if stream.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

fn head(client: &NetClient, req: Request) -> ResponseHead {
    let (tx, rx) = mpsc::channel();
    client.fetch(req, Box::new(move |ev| tx.send(ev).unwrap()));
    let mut head = None;
    loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            ResponseEvent::Response(h) => head = Some(h),
            ResponseEvent::Finished { .. } => return head.expect("response"),
            ResponseEvent::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
}

fn get(port: u16, path: &str) -> Request {
    Request::get(Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap(), Destination::Script)
}

#[test]
fn a_new_connection_reports_its_phases_and_a_reused_one_none() {
    let port = start();
    let client = NetClient::new(NetConfig { shared_asset_cache: None, ..NetConfig::default() }).unwrap();
    let t = head(&client, get(port, "/a")).timing;
    let (start, c) = (t.start.unwrap(), t.connection.expect("a new connection"));
    assert_eq!(t.fetch_start, Some(start));
    assert!(c.dns.is_none(), "an IP literal needs no lookup");
    assert!(!c.secure);
    assert!(start <= c.start && c.start <= c.tcp_end && c.tcp_end <= c.end);
    assert!(t.request_start.unwrap() >= c.end && t.response_start.unwrap() >= t.request_start.unwrap());
    assert!(!t.timing_allow_failed && t.redirect_end.is_none() && t.redirect_count == 0);
    // Keep-alive: the next request reuses the connection.
    let t = head(&client, get(port, "/b")).timing;
    assert!(t.connection.is_none());
    assert!(t.request_start.unwrap() >= t.start.unwrap());
}

#[test]
fn redirects_and_the_timing_allow_origin_check() {
    let port = start();
    let client = NetClient::new(NetConfig { shared_asset_cache: None, ..NetConfig::default() }).unwrap();
    let t = head(&client, get(port, "/redirect")).timing;
    assert_eq!((t.redirect_count, t.cross_origin_redirect), (1, false));
    assert!(t.redirect_end.unwrap() >= t.start.unwrap() && t.fetch_start.unwrap() >= t.redirect_end.unwrap());
    // A document of another origin sees the details only with its origin
    // (or `*`) in Timing-Allow-Origin.
    let mut req = get(port, "/a");
    req.origin = Some("https://other.test".into());
    assert!(head(&client, req).timing.timing_allow_failed);
    let mut req = get(port, "/tao");
    req.origin = Some("https://other.test".into());
    assert!(!head(&client, req).timing.timing_allow_failed);
    let mut req = get(port, "/a");
    req.origin = Some(format!("http://127.0.0.1:{port}"));
    assert!(!head(&client, req).timing.timing_allow_failed);
}

#[test]
fn a_cache_hit_has_no_network_phases() {
    let port = start();
    let client = NetClient::new(NetConfig { shared_asset_cache: None, ..NetConfig::default() }).unwrap();
    assert!(!head(&client, get(port, "/cached")).from_cache);
    let hit = head(&client, get(port, "/cached"));
    assert!(hit.from_cache);
    let t = hit.timing;
    assert!(t.connection.is_none() && t.start.is_some());
    assert!(t.request_start.unwrap() >= t.fetch_start.unwrap());
}
