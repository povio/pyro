//! macOS RSS sampler: `proc_pid_rusage(RUSAGE_INFO_V4)` every N ms.
//!
//! Usage: `rss-sampler <pid> <outfile|-> [interval_ms=20|--once] [--instructions]`
//! Output: epoch_ms resident_kib footprint_kib user_ns system_ns monotonic_ns, and with
//! `--instructions` instructions retired as a seventh column;
//! the first three columns remain compatible with existing readers. Stops when
//! the pid disappears (or on SIGTERM).
//!
//! Timing uses a periodic kqueue `EVFILT_TIMER` with `NOTE_CRITICAL`: on the
//! benchmark host plain `nanosleep(20ms)` was measured to wake after 40-170 ms
//! (timer coalescing), `NOTE_CRITICAL` gives ~20.0 ms. Each line is one
//! `write(2)` so a SIGTERM never leaves a partial line.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// `struct rusage_info_v4` from `<sys/resource.h>`: a 16-byte UUID followed by
/// 35 `uint64_t` fields (`ri_user_time` .. `ri_runnable_time`).
#[repr(C)]
struct RusageInfoV4 {
    uuid: [u8; 16],
    fields: [u64; 35],
}
const RI_RESIDENT_SIZE: usize = 6;
const RI_PHYS_FOOTPRINT: usize = 7;
const RI_USER_TIME: usize = 0;
const RI_SYSTEM_TIME: usize = 1;
const RI_INSTRUCTIONS: usize = 29;
const RUSAGE_INFO_V4: i32 = 4;

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

fn nanoseconds(ticks: u64, timebase: &Timebase) -> u128 {
    u128::from(ticks) * u128::from(timebase.numer) / u128::from(timebase.denom)
}

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
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV4) -> i32;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
    fn mach_absolute_time() -> u64;
    fn kqueue() -> i32;
    fn kevent(
        kq: i32,
        changes: *const Kevent,
        nchanges: i32,
        events: *mut Kevent,
        nevents: i32,
        timeout: *const u8,
    ) -> i32;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: rss-sampler <pid> <outfile|-> [interval_ms=20|--once]");
        std::process::exit(2);
    }
    let pid: i32 = args[1].parse().expect("pid must be an integer");
    assert!(pid > 0, "pid must be positive");
    let once = args.get(3).is_some_and(|s| s == "--once");
    let with_instructions = args.iter().any(|s| s == "--instructions");
    let interval_ms: isize =
        if once { 20 } else { args.get(3).map_or(20, |s| s.parse().expect("interval must be an integer")) };
    assert!(interval_ms > 0, "interval must be positive");
    let mut out: Box<dyn Write> = if args[2] == "-" {
        Box::new(std::io::stdout())
    } else {
        Box::new(std::fs::File::create(&args[2]).expect("create output file"))
    };
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // XNU fill_task_rusage uses task_power_info_locked's Mach-time counters.
    // Convert explicitly: Apple Silicon's timebase is not necessarily 1:1.
    // SAFETY: writable mach_timebase_info_data_t-compatible buffer.
    assert_eq!(unsafe { mach_timebase_info(&mut timebase) }, 0);
    assert!(timebase.denom > 0);

    // SAFETY: plain syscall without pointer arguments.
    let kq = unsafe { kqueue() };
    assert!(kq >= 0, "kqueue failed: {}", std::io::Error::last_os_error());
    let timer = Kevent {
        ident: 1,
        filter: EVFILT_TIMER,
        flags: EV_ADD | EV_ENABLE,
        fflags: NOTE_CRITICAL,
        data: interval_ms,
        udata: 0,
    };
    // SAFETY: `timer` is a valid kevent for the duration of the call; no event buffer.
    let rc = unsafe { kevent(kq, &timer, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    assert!(rc >= 0, "kevent(register) failed: {}", std::io::Error::last_os_error());

    loop {
        let mut info = RusageInfoV4 { uuid: [0; 16], fields: [0; 35] };
        // SAFETY: `info` is a writable rusage_info_v4-sized buffer, as RUSAGE_INFO_V4 requires.
        if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V4, &mut info) } != 0 {
            if once {
                panic!("proc_pid_rusage failed: {}", std::io::Error::last_os_error());
            }
            break; // process gone
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock after epoch").as_millis();
        // SAFETY: monotonic clock read without pointer arguments.
        let monotonic = nanoseconds(unsafe { mach_absolute_time() }, &timebase);
        let line = format!(
            "{now} {} {} {} {} {monotonic}{}\n",
            info.fields[RI_RESIDENT_SIZE] / 1024,
            info.fields[RI_PHYS_FOOTPRINT] / 1024,
            nanoseconds(info.fields[RI_USER_TIME], &timebase),
            nanoseconds(info.fields[RI_SYSTEM_TIME], &timebase),
            if with_instructions { format!(" {}", info.fields[RI_INSTRUCTIONS]) } else { String::new() }
        );
        if out.write_all(line.as_bytes()).is_err() {
            break;
        }
        if once {
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
