//! Synthetic large-string workload: process memory and CPU across repeated
//! `JsString::new` conversions or realm cycles that each evaluate one large
//! classic script. No DOM, network or CDP; not a browser or TOS benchmark.
//! ES modules do not go through `JsString::new` and are not exercised.
//!
//! Run: `cargo run --release -p lp-js --example large_string_cycles -- <convert|realm> <cycles> <kib> [ascii|cjk]`
//!
//! The source is `kib` KiB of UTF-8: `ascii` is ASCII only; `cjk` puts long
//! string literals of 3-byte scalars on every line, so most bytes are 3-byte
//! UTF-8. `convert` converts the same source to a JSC string each cycle and
//! drops it. `realm` creates a realm, evaluates the source (which keeps its
//! functions on the global), disposes the realm, then runs a synchronous full
//! collection (the run aborts if that SPI is unavailable) and drains finalized
//! payloads. Each cycle prints macOS `phys_footprint` and resident size; the
//! summary adds process CPU seconds (user + system, `getrusage`) and wall
//! seconds. With `LP_PRESSURE_RELIEF=1` it then asks every malloc zone to
//! return free memory (`malloc_zone_pressure_relief`) and prints the footprint
//! again.

use lp_js::{Engine, JsString, RealmOptions};
use std::time::Instant;

/// (phys_footprint, resident) of this process in MiB.
fn memory_mib() -> (f64, f64) {
    // `struct rusage_info_v2` (<libproc.h>): a UUID, then 18 `uint64_t`
    // fields; index 6 is `ri_resident_size`, 7 is `ri_phys_footprint`.
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
    // SAFETY: the buffer has the rusage_info_v2 layout requested by flavor 2.
    let rc = unsafe { proc_pid_rusage(getpid(), 2, &mut info) };
    assert_eq!(rc, 0, "proc_pid_rusage");
    let mib = |v: u64| v as f64 / (1024.0 * 1024.0);
    (mib(info.fields[7]), mib(info.fields[6]))
}

/// Darwin `struct timeval`: `time_t` (64-bit) seconds and `suseconds_t`
/// (32-bit) microseconds, padded to 16 bytes.
#[repr(C)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i32,
}

/// Darwin `struct rusage`: two timevals, then 14 `long` counters.
#[repr(C)]
struct Rusage {
    ru_utime: Timeval,
    ru_stime: Timeval,
    counters: [i64; 14],
}
const _: () = assert!(std::mem::size_of::<Timeval>() == 16 && std::mem::size_of::<Rusage>() == 144);

/// User + system CPU seconds of the whole process.
fn cpu_seconds() -> f64 {
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    // SAFETY: getrusage(RUSAGE_SELF) writes a complete struct rusage.
    let mut u: Rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { getrusage(0, &mut u) };
    assert_eq!(rc, 0, "getrusage");
    let tv = |t: &Timeval| t.tv_sec as f64 + f64::from(t.tv_usec) / 1e6;
    tv(&u.ru_utime) + tv(&u.ru_stime)
}

fn script(kib: usize, cjk: bool) -> String {
    let literal = if cjk { "漢字テキスト".repeat(16) } else { "pad-pad-pad-pad".into() };
    let mut js = String::from("var __app = [];\n");
    let mut i = 0;
    while js.len() < kib * 1024 {
        js.push_str(&format!("__app.push(function f{i}(a) {{ return a + {i} + '{literal}'; }});\n"));
        i += 1;
    }
    js
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "realm".into());
    let cycles: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(30);
    let kib: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(4096);
    let text = args.next().unwrap_or_else(|| "ascii".into());
    assert!(mode == "convert" || mode == "realm", "mode must be convert or realm");
    assert!(text == "ascii" || text == "cjk", "text must be ascii or cjk");
    let engine = Engine::new().expect("engine");
    let src = script(kib, text == "cjk");
    let units = src.encode_utf16().count();
    let (fp0, rss0) = memory_mib();
    let (cpu0, wall0) = (cpu_seconds(), Instant::now());
    println!(
        "mode={mode} cycles={cycles} kib={kib} text={text} source_bytes={} utf16_units={units} \
         start footprint={fp0:.1} rss={rss0:.1} MiB",
        src.len()
    );
    for i in 1..=cycles {
        if mode == "convert" {
            let j = JsString::new(&src);
            assert_eq!(j.len_utf16(), units);
        } else {
            let realm = engine.create_realm(RealmOptions::default()).expect("realm");
            {
                let s = realm.scope().expect("scope");
                s.eval(&src, Some("http://127.0.0.1/app.js"), 1).expect("eval");
            }
            realm.dispose();
            drop(realm);
            assert!(engine.collect_garbage_now(), "synchronous full collection unavailable");
            engine.drain_finalized();
        }
        let (fp, rss) = memory_mib();
        println!("{i:>5}  footprint {fp:>8.1} MiB  rss {rss:>8.1} MiB");
    }
    let (fp, rss) = memory_mib();
    println!(
        "end footprint +{:.1} MiB  rss +{:.1} MiB  cpu {:.3} s  wall {:.3} s",
        fp - fp0,
        rss - rss0,
        cpu_seconds() - cpu0,
        wall0.elapsed().as_secs_f64()
    );
    if std::env::var_os("LP_PRESSURE_RELIEF").is_some() {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means all zones; goal 0 means as much as possible.
        let released = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
        let (fp, rss) = memory_mib();
        println!(
            "after pressure relief ({:.1} MiB released) footprint +{:.1} MiB  rss +{:.1} MiB",
            released as f64 / (1024.0 * 1024.0),
            fp - fp0,
            rss - rss0
        );
    }
}
