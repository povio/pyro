//! `serve --fork`: every connection is a fork of a template process that
//! booted the prepared pages once (patched JavaScriptCore only; skipped
//! otherwise).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const APP: &str = "<!doctype html><title>app</title><div id=root>loading</div><script src=/app.js></script>";
const APP_JS: &str = "window.__boots = (window.__boots || 0) + 1; window.__store = { n: 0 }; \
                      document.getElementById('root').textContent = 'ready';";

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn connect(port: u16) -> TcpStream {
    connect_to(port, "/")
}

fn connect_to(port: u16, path: &str) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut s = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("server did not start: {e}"),
        }
    };
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(
        format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    s
}

fn call(s: &mut TcpStream, id: u64, method: &str, params: Value, session: Option<&str>) -> Value {
    let mut message = json!({ "id": id, "method": method, "params": params });
    if let Some(session) = session {
        message["sessionId"] = json!(session);
    }
    let payload = message.to_string().into_bytes();
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x81];
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&frame).unwrap();
    loop {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        let len = match h[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).unwrap();
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                s.read_exact(&mut b).unwrap();
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut p = vec![0u8; len];
        s.read_exact(&mut p).unwrap();
        let Ok(m) = serde_json::from_slice::<Value>(&p) else { continue };
        if m["id"] == json!(id) {
            assert!(m.get("error").is_none(), "{method}: {m}");
            return m["result"].clone();
        }
    }
}

/// Acquires the prepared page at `url` on a new connection and evaluates `expression` in it.
fn eval_in_prepared(port: u16, url: &str, expression: &str) -> Value {
    let mut c = connect(port);
    let page = call(&mut c, 1, "Lightningpanda.acquirePreparedPage", json!({ "url": url, "attach": true }), None);
    assert_eq!(page["ready"], json!(true));
    let session = page["sessionId"].as_str().unwrap().to_owned();
    let r =
        call(&mut c, 2, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }), Some(&session));
    r["result"]["value"].clone()
}

/// `eval_in_prepared` for a connection to `?profile=N`, acquiring that profile's page.
fn eval_in_profile(port: u16, profile: usize, expression: &str) -> Value {
    let mut c = connect_to(port, &format!("/?profile={profile}"));
    let page =
        call(&mut c, 1, "Lightningpanda.acquirePreparedPage", json!({ "profileId": profile, "attach": true }), None);
    assert_eq!(page["ready"], json!(true));
    let session = page["sessionId"].as_str().unwrap().to_owned();
    let r =
        call(&mut c, 2, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }), Some(&session));
    r["result"]["value"].clone()
}

#[test]
fn connections_are_forks_of_the_booted_app() {
    if !lp_js::Capabilities::detect().fork {
        eprintln!("skipped: this JavaScriptCore cannot fork (scripts/jsc/build.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lp-fork-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), APP).unwrap();
    std::fs::write(dir.join("other.html"), APP).unwrap();
    std::fs::write(dir.join("setup.html"), APP).unwrap();
    std::fs::write(dir.join("app.js"), APP_JS).unwrap();
    let port = free_port();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_lightningpanda"))
            .args(["serve", "--port", &port.to_string(), "--fork", "--log-level", "warn"])
            .arg("--static")
            .arg(format!("http://app.test={}", dir.display()))
            .args(["--prepare", "http://app.test/"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let mut main = connect(port);
    assert_eq!(call(&mut main, 1, "Lightningpanda.capabilities", json!({}), None)["fork"], json!(true));

    // Each connection starts from the booted page: its writes stay its own,
    // and Math.random() does not repeat a sibling's sequence.
    let expression = "JSON.stringify([document.getElementById('root').textContent, window.__boots, ++window.__store.n, Math.random()])";
    let runs: Vec<Value> = (0..3)
        .map(|_| {
            serde_json::from_str(eval_in_prepared(port, "http://app.test/", expression).as_str().unwrap()).unwrap()
        })
        .collect();
    for r in &runs {
        assert_eq!(r[0], json!("ready"));
        assert_eq!(r[1], json!(1), "booted once, in the template");
        assert_eq!(r[2], json!(1), "a fork does not see another fork's writes");
    }
    assert_ne!(runs[0][3], runs[1][3]);
    assert_ne!(runs[1][3], runs[2][3]);

    // A profile registered at run time boots in the template; later connections have it.
    let registered = call(
        &mut main,
        2,
        "Lightningpanda.registerPreparedPage",
        json!({ "url": "http://app.test/other.html", "initScripts": ["window.__seeded = 42"] }),
        None,
    );
    assert_eq!(registered["profileId"], json!(1));
    let again = call(
        &mut main,
        3,
        "Lightningpanda.registerPreparedPage",
        json!({ "url": "http://app.test/other.html", "initScripts": ["window.__seeded = 42"] }),
        None,
    );
    assert_eq!(again["profileId"], json!(1), "the same profile is booted once");
    let seeded = eval_in_prepared(port, "http://app.test/other.html", "[window.__seeded, location.pathname].join()");
    assert_eq!(seeded, json!("42,/other.html"));

    // Setup steps run once in the template; every fork starts from their result.
    let with_setup = call(
        &mut main,
        4,
        "Lightningpanda.registerPreparedPage",
        json!({ "url": "http://app.test/setup.html", "setup": [{ "eval": {
            "expression": "window.__store.n += 10; window.__setups = (window.__setups || 0) + 1; \
                           document.getElementById('root').textContent = 'set up'"
        } }] }),
        None,
    );
    assert_eq!(with_setup["profileId"], json!(2));
    for _ in 0..2 {
        let state = eval_in_prepared(
            port,
            "http://app.test/setup.html",
            "[document.getElementById('root').textContent, window.__setups, ++window.__store.n].join()",
        );
        assert_eq!(state, json!("set up,1,11"));
    }

    // A snapshot built on that one: forked from it, runs only its own steps,
    // and connections ask for it by profile in the URL.
    let derived = call(
        &mut main,
        5,
        "Lightningpanda.registerPreparedPage",
        json!({ "url": "http://app.test/setup.html", "base": 2, "setup": [{ "eval": {
            "expression": "window.__store.n += 100; window.__derived = (window.__derived || 0) + 1"
        } }] }),
        None,
    );
    assert_eq!(derived["profileId"], json!(3));
    let deeper = call(
        &mut main,
        6,
        "Lightningpanda.registerPreparedPage",
        json!({ "url": "http://app.test/setup.html", "base": 3, "setup": [{ "eval": {
            "expression": "window.__store.n += 1000"
        } }] }),
        None,
    );
    assert_eq!(deeper["profileId"], json!(4));
    for (profile, expected) in [(3, "set up,1,1,111"), (3, "set up,1,1,111"), (4, "set up,1,1,1111")] {
        let state = eval_in_profile(
            port,
            profile,
            "[document.getElementById('root').textContent, window.__setups, window.__derived, ++window.__store.n].join()",
        );
        assert_eq!(state, json!(expected), "profile {profile}");
    }
    // The base snapshot is unchanged by what was built on it.
    let base = eval_in_prepared(port, "http://app.test/setup.html", "[window.__derived, window.__store.n].join()");
    assert_eq!(base, json!(",10"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The template starts with fork mode's JavaScriptCore defaults (a 32 MB JIT
/// pool, JavaScriptCore's own heap growth) in place of the server's own
/// defaults, and a value the user set wins.
#[test]
fn the_template_takes_the_fork_defaults_unless_the_user_set_them() {
    if !lp_js::Capabilities::detect().fork {
        eprintln!("skipped: this JavaScriptCore cannot fork (scripts/jsc/build.sh)");
        return;
    }
    let options = |growth: Option<&str>| {
        let port = free_port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_lightningpanda"));
        command
            .args(["serve", "--port", &port.to_string(), "--fork", "--log-level", "warn"])
            .env("JSC_dumpOptions", "2")
            .env_remove("JSC_heapGrowthMaxIncrease")
            .env_remove("JSC_jitMemoryReservationSize")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(growth) = growth {
            command.env("JSC_heapGrowthMaxIncrease", growth);
        }
        let mut server = Server(command.spawn().unwrap());
        let mut main = connect(port);
        // Served by a fork: the template is up, its options dumped.
        assert_eq!(call(&mut main, 1, "Lightningpanda.capabilities", json!({}), None)["fork"], json!(true));
        let mut stderr = server.0.stderr.take().unwrap();
        drop(main);
        drop(server); // the template, then its forks, end with the server
        let mut text = String::new();
        stderr.read_to_string(&mut text).unwrap();
        let values = |name: &str| -> Vec<String> {
            text.split_whitespace().filter_map(|w| w.strip_prefix(name)).map(str::to_owned).collect()
        };
        (values("heapGrowthMaxIncrease="), values("jitMemoryReservationSize="))
    };
    let (growth, pool) = options(None);
    assert!(growth.contains(&"3".to_owned()), "template heap growth: {growth:?}");
    assert!(pool.contains(&"33554432".to_owned()), "template JIT pool: {pool:?}");
    let (growth, _) = options(Some("2"));
    assert!(growth.contains(&"2".to_owned()) && !growth.contains(&"3".to_owned()), "user's heap growth: {growth:?}");
}
