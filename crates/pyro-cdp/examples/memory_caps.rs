//! Measures process memory for the count-based CDP caps under realistic and
//! worst-case payloads (real JavaScriptCore). Evidence for
//! docs/automation/OPTIMIZATION.md; not a benchmark of browser memory.
//! Run: `cargo run --release -p pyro-cdp --example memory_caps -- <scenario>`
//! (no argument lists the scenarios). "footprint" is the macOS
//! phys_footprint of the process (proc_pid_rusage) before and after the
//! scenario, with its state still retained.

use std::time::{Duration, Instant};

use pyro_cdp::engine::EngineHost;
use pyro_cdp::outbox::{Outbound, OutboundRx, Outbox};
use pyro_cdp::target::Browser;
use serde_json::{json, Value};

fn rss_mib() -> f64 {
    // SAFETY: getrusage writes into the provided struct.
    let mut usage: Rusage = unsafe { std::mem::zeroed() };
    unsafe { getrusage(0, &mut usage) };
    usage.ru_maxrss as f64 / (1024.0 * 1024.0) // macOS reports bytes
}

#[repr(C)]
struct Rusage {
    ru_utime: [i64; 2],
    ru_stime: [i64; 2],
    ru_maxrss: i64,
    rest: [i64; 13],
}
unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut Rusage) -> i32;
}

fn current_footprint_mib() -> f64 {
    #[repr(C)]
    struct RusageInfoV2 {
        uuid: [u8; 16],
        fields: [u64; 18],
    }
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV2) -> i32;
        fn getpid() -> i32;
    }
    let mut info = RusageInfoV2 { uuid: [0; 16], fields: [0; 18] };
    // SAFETY: buffer has the rusage_info_v2 layout for flavor 2.
    unsafe { proc_pid_rusage(getpid(), 2, &mut info) };
    info.fields[7] as f64 / (1024.0 * 1024.0)
}

struct H {
    b: Browser<EngineHost>,
    out: Outbox,
    rx: OutboundRx,
    id: u64,
    session: String,
}

fn decode(bytes: &[u8]) -> Value {
    let header = match bytes[1] {
        126 => 4,
        127 => 10,
        _ => 2,
    };
    serde_json::from_slice(&bytes[header..]).unwrap_or(Value::Null)
}

impl H {
    fn new() -> H {
        let (out, rx) = Outbox::channel(usize::MAX / 2);
        let host = EngineHost::new(out.clone()).unwrap();
        let mut h = H { b: Browser::new(host), out, rx, id: 0, session: String::new() };
        h.call(false, "Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}));
        let mut sink = h.out.clone();
        h.id += 1;
        h.b.handle_text(
            &json!({"id":h.id,"method":"Target.createTarget","params":{"url":"about:blank"}}).to_string(),
            &mut sink,
        );
        while let Some(Outbound::Frame(f)) = h.rx.try_recv() {
            if let Some(s) = decode(f.as_ref())["params"]["sessionId"].as_str() {
                h.session = s.to_owned();
            }
        }
        h
    }

    fn call(&mut self, in_session: bool, method: &str, params: Value) -> Value {
        self.id += 1;
        let mut m = json!({"id": self.id, "method": method, "params": params});
        if in_session {
            m["sessionId"] = json!(self.session);
        }
        let mut sink = self.out.clone();
        self.b.handle_text(&m.to_string(), &mut sink);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            self.b.host_mut().flush();
            let mut found = None;
            while let Some(item) = self.rx.try_recv() {
                if let Outbound::Frame(f) = item {
                    let v = decode(f.as_ref());
                    if v["id"] == self.id {
                        found = Some(v);
                    }
                }
            }
            if let Some(v) = found {
                return v;
            }
            assert!(Instant::now() < deadline);
            self.b.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(5)), || false);
        }
    }
}

fn report(label: &str, before: f64, after: f64) {
    println!("{label:<62} footprint {before:>7.1} -> {after:>7.1} MiB (+{:.1})", after - before);
}

/// (name, description, run)
type Scenario = (&'static str, &'static str, fn(&mut H));

const SCENARIOS: &[Scenario] = &[
    ("console-primitive", "99,999 console.log(i) in one task (+ queued outbound frames)", |h| {
        h.call(true, "Runtime.evaluate", json!({"expression":"for (let i = 0; i < 99999; i++) console.log(i); 0"}));
    }),
    ("console-object", "99,999 console.log({i}) in one task (rooted objects + frames)", |h| {
        h.call(true, "Runtime.evaluate", json!({"expression":"for (let i = 0; i < 99999; i++) console.log({i}); 0"}));
    }),
    ("console-1k-string", "99,999 console.log(1 KiB string) in one task (+ frames)", |h| {
        h.call(
            true,
            "Runtime.evaluate",
            json!({"expression":"const s = 'x'.repeat(1024); for (let i = 0; i < 99999; i++) console.log(s); 0"}),
        );
    }),
    ("handles", "100,000 retained object handles (default handle budget)", |h| {
        for _ in 0..100_000 {
            h.call(true, "Runtime.evaluate", json!({"expression":"({a: 1})"}));
        }
        let r = h.call(true, "Runtime.evaluate", json!({"expression":"({a: 1})"}));
        assert!(r.get("error").is_some(), "budget reached: {r}");
    }),
    ("pending-replies", "9,999 pending awaitPromise replies", |h| {
        for _ in 0..9_999 {
            let mut sink = h.out.clone();
            h.id += 1;
            let m = json!({"id":h.id,"method":"Runtime.evaluate","params":{"expression":"new Promise(() => {})","awaitPromise":true},"sessionId":h.session});
            h.b.handle_text(&m.to_string(), &mut sink);
        }
    }),
];

/// One scenario per process (`memory_caps <name>`), so the footprint delta is
/// not distorted by memory that earlier scenarios freed and the allocator kept.
fn main() {
    let name = std::env::args().nth(1).unwrap_or_default();
    let Some((_, label, run)) = SCENARIOS.iter().find(|(n, _, _)| *n == name) else {
        let names: Vec<&str> = SCENARIOS.iter().map(|(n, _, _)| *n).collect();
        eprintln!("usage: memory_caps <{}>", names.join("|"));
        std::process::exit(2);
    };
    let mut h = H::new();
    h.call(true, "Runtime.enable", json!({}));
    let before = current_footprint_mib();
    run(&mut h);
    h.b.host_mut().flush();
    report(label, before, current_footprint_mib());
    println!("  peak RSS {:.1} MiB", rss_mib());
}
