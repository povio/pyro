//! macOS RSS sampler: `proc_pid_rusage(RUSAGE_INFO_V2)` every N ms.
//!
//! Usage: `rss-sampler <pid> <outfile> [interval_ms=20]`
//! Output lines: `<epoch_ms> <resident_kib> <phys_footprint_kib>`; stops when
//! the pid disappears (or on SIGTERM).
//!
//! Timing uses a periodic kqueue `EVFILT_TIMER` with `NOTE_CRITICAL`: on the
//! benchmark host plain `nanosleep(20ms)` was measured to wake after 40-170 ms
//! (timer coalescing), `NOTE_CRITICAL` gives ~20.0 ms. Each line is one
//! `write(2)` so a SIGTERM never leaves a partial line.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// `struct rusage_info_v2` from `<sys/resource.h>`: a 16-byte UUID followed by
/// 18 `uint64_t` fields (`ri_user_time` .. `ri_diskio_byteswritten`).
#[repr(C)]
struct RusageInfoV2 {
    uuid: [u8; 16],
    fields: [u64; 18],
}
const RI_RESIDENT_SIZE: usize = 6;
const RI_PHYS_FOOTPRINT: usize = 7;
const RUSAGE_INFO_V2: i32 = 2;

/// `struct kevent` from `<sys/event.h>`.
#[repr(C)]
struct Kevent {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: usize,
}
const EVFILT_TIMER: i16 = -7;
const EV_ADD: u16 = 0x1;
const EV_ENABLE: u16 = 0x4;
const NOTE_CRITICAL: u32 = 0x20;

unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV2) -> i32;
    fn kqueue() -> i32;
    fn kevent(kq: i32, changes: *const Kevent, nchanges: i32, events: *mut Kevent, nevents: i32, timeout: *const u8) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: rss-sampler <pid> <outfile> [interval_ms=20]");
        std::process::exit(2);
    }
    let pid: i32 = args[1].parse().expect("pid must be an integer");
    let interval_ms: isize = args.get(3).map_or(20, |s| s.parse().expect("interval must be an integer"));
    let mut out = std::fs::File::create(&args[2]).expect("create output file");

    // SAFETY: plain syscall without pointer arguments.
    let kq = unsafe { kqueue() };
    assert!(kq >= 0, "kqueue failed: {}", std::io::Error::last_os_error());
    let timer = Kevent { ident: 1, filter: EVFILT_TIMER, flags: EV_ADD | EV_ENABLE, fflags: NOTE_CRITICAL, data: interval_ms, udata: 0 };
    // SAFETY: `timer` is a valid kevent for the duration of the call; no event buffer.
    let rc = unsafe { kevent(kq, &timer, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    assert!(rc >= 0, "kevent(register) failed: {}", std::io::Error::last_os_error());

    loop {
        let mut info = RusageInfoV2 { uuid: [0; 16], fields: [0; 18] };
        // SAFETY: `info` is a writable rusage_info_v2-sized buffer, as RUSAGE_INFO_V2 requires.
        if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, &mut info) } != 0 {
            break; // process gone
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after epoch").as_millis();
        let line = format!("{now} {} {}\n", info.fields[RI_RESIDENT_SIZE] / 1024, info.fields[RI_PHYS_FOOTPRINT] / 1024);
        if out.write_all(line.as_bytes()).is_err() {
            break;
        }
        let mut event = Kevent { ident: 0, filter: 0, flags: 0, fflags: 0, data: 0, udata: 0 };
        // SAFETY: `event` is a writable buffer for exactly one kevent; null timeout blocks until the timer fires.
        let rc = unsafe { kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null()) };
        if rc < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break;
        }
    }
}
