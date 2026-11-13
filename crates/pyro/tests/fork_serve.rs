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
    let m = call_message(s, id, method, params, session);
    assert!(m.get("error").is_none(), "{method}: {m}");
    m["result"].clone()
}

/// The response to a command, an error response included.
fn call_message(s: &mut TcpStream, id: u64, method: &str, params: Value, session: Option<&str>) -> Value {
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
            return m;
        }
    }
}

/// Acquires the prepared page at `url` on a new connection and evaluates `expression` in it.
fn eval_in_prepared(port: u16, url: &str, expression: &str) -> Value {
    let mut c = connect(port);
    let page = call(&mut c, 1, "Pyro.acquirePreparedPage", json!({ "url": url, "attach": true }), None);
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
        call(&mut c, 1, "Pyro.acquirePreparedPage", json!({ "profileId": profile, "attach": true }), None);
    assert_eq!(page["ready"], json!(true));
    let session = page["sessionId"].as_str().unwrap().to_owned();
    let r =
        call(&mut c, 2, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true }), Some(&session));
    r["result"]["value"].clone()
}

#[test]
fn connections_are_forks_of_the_booted_app() {
    if !pyro_js::Capabilities::detect().fork {
        eprintln!("skipped: this JavaScriptCore cannot fork (scripts/jsc/build.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pyro-fork-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), APP).unwrap();
    std::fs::write(dir.join("other.html"), APP).unwrap();
    std::fs::write(dir.join("setup.html"), APP).unwrap();
    std::fs::write(dir.join("app.js"), APP_JS).unwrap();
    let port = free_port();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_pyro"))
            .args(["serve", "--port", &port.to_string(), "--fork", "--log-level", "warn"])
            .arg("--static")
            .arg(format!("http://app.test={}", dir.display()))
            .args(["--prepare", "http://app.test/"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let mut main = connect(port);
    assert_eq!(call(&mut main, 1, "Pyro.capabilities", json!({}), None)["fork"], json!(true));

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
        "Pyro.registerPreparedPage",
        json!({ "url": "http://app.test/other.html", "initScripts": ["window.__seeded = 42"] }),
        None,
    );
    assert_eq!(registered["profileId"], json!(1));
    let again = call(
        &mut main,
        3,
        "Pyro.registerPreparedPage",
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
        "Pyro.registerPreparedPage",
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
        "Pyro.registerPreparedPage",
        json!({ "url": "http://app.test/setup.html", "base": 2, "setup": [{ "eval": {
            "expression": "window.__store.n += 100; window.__derived = (window.__derived || 0) + 1"
        } }] }),
        None,
    );
    assert_eq!(derived["profileId"], json!(3));
    let deeper = call(
        &mut main,
        6,
        "Pyro.registerPreparedPage",
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
    if !pyro_js::Capabilities::detect().fork {
        eprintln!("skipped: this JavaScriptCore cannot fork (scripts/jsc/build.sh)");
        return;
    }
    let options = |growth: Option<&str>| {
        let port = free_port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pyro"));
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
        assert_eq!(call(&mut main, 1, "Pyro.capabilities", json!({}), None)["fork"], json!(true));
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

/// The root template parses and compiles the static roots' module scripts
/// before its first page. Its forks load them without parsing: after a
/// navigation of their own, and a module no process had loaded before.
#[test]
fn forks_load_the_modules_the_template_compiled() {
    if !pyro_js::Capabilities::detect().fork || !pyro_js::Engine::has_module_precompile() {
        eprintln!("skipped: this JavaScriptCore cannot fork or precompile modules (scripts/jsc/build.sh)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pyro-fork-modules-{}", std::process::id()));
    let stats = std::env::temp_dir().join(format!("pyro-fork-modules-{}.stats", std::process::id()));
    let _ = std::fs::remove_file(&stats);
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>modules</title><div id=root>loading</div>\
         <script type=module src=/assets/main.js></script>",
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/main.js"),
        "import { greet } from './dep.js';\n\
         document.getElementById('root').textContent = greet(location.search || 'main');\n\
         window.lazy = () => import('./lazy.js').then((m) => m.default(import.meta.url));\n",
    )
    .unwrap();
    std::fs::write(dir.join("assets/dep.js"), "export function greet(who) { return `hello ${who}`; }\n").unwrap();
    std::fs::write(
        dir.join("assets/lazy.js"),
        "const made = await Promise.resolve('lazy');\n\
         export default (from) => `${made} from ${new URL(from).pathname}`;\n",
    )
    .unwrap();
    // A classic script that is no module: tried, not kept.
    std::fs::write(dir.join("assets/legacy.js"), "with (Math) { var p = PI }\n").unwrap();
    let port = free_port();
    let server = Server(
        Command::new(env!("CARGO_BIN_EXE_pyro"))
            .args(["serve", "--port", &port.to_string(), "--fork", "--log-level", "warn"])
            .arg("--static")
            .arg(format!("http://app.test={}", dir.display()))
            .args(["--prepare", "http://app.test/"])
            .env("PYRO_FORK_STATS", &stats)
            .env_remove("PYRO_FORK_PRECOMPILE")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let mut c = connect(port);
    let page = call(
        &mut c,
        1,
        "Pyro.acquirePreparedPage",
        json!({ "url": "http://app.test/", "attach": true }),
        None,
    );
    assert_eq!(page["ready"], json!(true));
    let session = page["sessionId"].as_str().unwrap().to_owned();
    let mut id = 1;
    let mut eval = |c: &mut TcpStream, expression: &str| {
        id += 1;
        let params = json!({ "expression": expression, "returnByValue": true, "awaitPromise": true });
        call(c, id, "Runtime.evaluate", params, Some(&session))["result"]["value"].clone()
    };
    assert_eq!(eval(&mut c, "document.getElementById('root').textContent"), json!("hello main"));
    // The template never imported this one.
    assert_eq!(eval(&mut c, "window.lazy()"), json!("lazy from /assets/main.js"));
    // A document of the fork's own loads the page's modules again.
    call(&mut c, 100, "Page.navigate", json!({ "url": "http://app.test/?again" }), Some(&session));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut id = 100;
    loop {
        id += 1;
        // An error while the new document replaces the old one's context.
        let params = json!({ "expression": "document.getElementById('root')?.textContent", "returnByValue": true });
        let shown = call_message(&mut c, id, "Runtime.evaluate", params, Some(&session));
        if shown["result"]["result"]["value"] == json!("hello ?again") {
            break;
        }
        assert!(Instant::now() < deadline, "the second document did not load: {shown}");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(c);

    // The fork logs its counters when its connection has closed.
    let deadline = Instant::now() + Duration::from_secs(20);
    let (precompile, child) = loop {
        let text = std::fs::read_to_string(&stats).unwrap_or_default();
        let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let precompile = lines.iter().find(|l| l["e"] == json!("precompile")).cloned();
        let child = lines.iter().find(|l| l["e"] == json!("child") && l["modules"].is_object()).cloned();
        match (precompile, child) {
            (Some(precompile), Some(child)) => break (precompile, child),
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => panic!("no precompile and child record in the fork stats: {text}"),
        }
    };
    assert_eq!((&precompile["tried"], &precompile["kept"]), (&json!(4), &json!(3)), "{precompile}");
    // The default keeps the modules; a function is compiled where it is first called.
    assert_eq!(precompile["functions"], json!(0), "{precompile}");
    // lazy.js, then main.js and dep.js of the second document.
    let modules = &child["modules"];
    assert_eq!(
        (&modules["precompiled"], &modules["record_hits"], &modules["record_misses"], &modules["code_hits"]),
        (&json!(3), &json!(3), &json!(0), &json!(3)),
        "{child}"
    );

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&stats);
}
