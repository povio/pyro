//! fork() support of the patched JavaScriptCore (`webkit/patches/0002`, `0026`):
//! a child keeps using the parent's VM, with its state, after JSPrepareForFork.
//! Skipped on a JavaScriptCore without it.

use pyro_jsc_sys::*;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Barrier, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
    fn alarm(seconds: u32) -> u32;
    fn signal(signum: i32, handler: unsafe extern "C" fn(i32)) -> usize;
}
const SIGALRM: i32 = 14;

/// The tests of this binary share a process, and a child must not inherit the
/// locks of a second thread that was inside JavaScriptCore at the fork: one
/// test at a time. Also selects the options pyro runs with (pyro-js
/// `configure_process`, `prepare_process_for_fork`), before JavaScriptCore
/// reads them: polling traps, and no Mach exception port, which a child does
/// not inherit (a thread that enters a VM there for the first time would crash
/// registering with it).
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("JSC_usePollingTraps", "true");
        std::env::set_var("JSC_useMachForExceptions", "false");
    });
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe fn eval(ctx: JSGlobalContextRef, src: &str) -> String {
    unsafe {
        let src = CString::new(src).unwrap();
        let script = JSStringCreateWithUTF8CString(src.as_ptr());
        let mut exc: JSValueRef = core::ptr::null();
        let v = JSEvaluateScript(ctx, script, core::ptr::null_mut(), core::ptr::null_mut(), 1, &mut exc);
        JSStringRelease(script);
        let s = JSValueToStringCopy(ctx, if exc.is_null() { v } else { exc }, core::ptr::null_mut());
        let mut buf = vec![0u8; JSStringGetMaximumUTF8CStringSize(s)];
        let n = JSStringGetUTF8CString(s, buf.as_mut_ptr().cast(), buf.len());
        JSStringRelease(s);
        String::from_utf8_lossy(&buf[..n.saturating_sub(1)]).into_owned()
    }
}

unsafe fn define(ctx: JSGlobalContextRef, name: &str, function: JSObjectCallAsFunctionCallbackRaw) {
    unsafe {
        let name = CString::new(name).unwrap();
        let name = JSStringCreateWithUTF8CString(name.as_ptr());
        let function = JSObjectMakeFunctionWithCallback(ctx, name, Some(function));
        let global = JSContextGetGlobalObject(ctx);
        JSObjectSetProperty(ctx, global, name, function, kJSPropertyAttributeNone, core::ptr::null_mut());
        JSStringRelease(name);
    }
}

#[test]
fn child_continues_from_the_parents_heap() {
    let _serial = serial();
    let Some((prepare, did_fork, reseed)) = spi::get().fork() else {
        eprintln!("skipped: this JavaScriptCore cannot fork");
        return;
    };
    unsafe {
        let group = JSContextGroupCreate();
        let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null_mut());
        // Enough code and garbage that the JIT and the GC threads have run.
        eval(
            ctx,
            "globalThis.app = { fns: [] }; \
             for (let m = 0; m < 2000; m++) app.fns.push(new Function('x', 'return x + ' + m)); \
             function hot(n) { let s = 0; for (let i = 0; i < n; i++) s = (s + app.fns[i % 2000](i)) | 0; return s; } \
             hot(2e6); Math.random()",
        );
        let random_file = std::env::temp_dir().join(format!("pyro-jsc-fork-{}", std::process::id()));
        for _ in 0..2 {
            assert!(prepare(group, 5.0), "helper threads did not stop");
            let pid = fork();
            if pid == 0 {
                did_fork(true);
                reseed(ctx);
                let ok = eval(
                    ctx,
                    "let g = []; for (let i = 0; i < 2e5; i++) g.push({ i }); g = null; \
                     app.fns.push(1); [app.fns.length, typeof hot(3e6)].join()",
                ) == "2001,number";
                std::fs::write(&random_file, eval(ctx, "Math.random()")).unwrap();
                JSGarbageCollect(ctx);
                _exit(if ok { 0 } else { 1 });
            }
            did_fork(false);
            let mut status = -1;
            waitpid(pid, &mut status, 0);
            assert_eq!(status, 0, "child failed (status {status})");
            assert_eq!(eval(ctx, "app.fns.length"), "2000", "the child's writes stay in the child");
            // Without the reseed the child would draw exactly the parent's next number.
            assert_ne!(std::fs::read_to_string(&random_file).unwrap(), eval(ctx, "Math.random()"));
        }
        let _ = std::fs::remove_file(&random_file);
        JSGlobalContextRelease(ctx);
        JSContextGroupRelease(group);
    }
}

/// How often the watchdog callback ran, in this process.
static FIRED: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn count_firing(_: JSContextRef, _: *mut core::ffi::c_void) -> bool {
    FIRED.fetch_add(1, Ordering::Relaxed);
    false
}

/// `fired()`: whether the watchdog callback has run.
unsafe extern "C" fn fired(
    ctx: JSContextRef,
    _: JSObjectRef,
    _: JSObjectRef,
    _: usize,
    _: *const JSValueRef,
    _: *mut JSValueRef,
) -> JSValueRef {
    unsafe { JSValueMakeBoolean(ctx, FIRED.load(Ordering::Relaxed) > 0) }
}

/// Runs script until the watchdog callback has run, for 2 s at most.
const WAIT_FOR_WATCHDOG: &str =
    "(() => { const t = Date.now(); while (!fired() && Date.now() - t < 2000) {} return fired(); })()";

/// A watchdog arms no timer while one that fires early enough is pending. The
/// pending one of a child is on the parent's timer thread, which the child does
/// not have: its first VM entries, for as long as the parent's time limit had
/// left, ran without a watchdog.
#[test]
fn child_arms_its_own_watchdog_timer() {
    let _serial = serial();
    let (Some((prepare, did_fork, reseed)), Some(set_limit)) = (spi::get().fork(), spi::get().set_execution_time_limit)
    else {
        eprintln!("skipped: this JavaScriptCore cannot fork");
        return;
    };
    unsafe {
        let group = JSContextGroupCreate();
        let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null_mut());
        define(ctx, "fired", fired);
        // Entering the VM arms a timer for the limit.
        set_limit(group, 0.25, Some(count_firing), core::ptr::null_mut());
        eval(ctx, "0");
        assert!(prepare(group, 5.0), "helper threads did not stop");
        let pid = fork();
        if pid == 0 {
            did_fork(true);
            reseed(ctx);
            FIRED.store(0, Ordering::Relaxed);
            _exit(if eval(ctx, WAIT_FOR_WATCHDOG) == "true" { 0 } else { 1 });
        }
        did_fork(false);
        let mut status = -1;
        waitpid(pid, &mut status, 0);
        assert_eq!(status, 0, "the watchdog did not fire in the child (status {status})");
        JSGlobalContextRelease(ctx);
        JSContextGroupRelease(group);
    }
}

/// Timers of one round of the stress, due `STEP` apart.
const TIMERS: u32 = 20_000;
const STEP: Duration = Duration::from_micros(1);
/// Start of the round and when its first timer is due after it.
static ROUND: Mutex<Option<(Instant, Duration)>> = Mutex::new(None);

/// `armTimers()`: arms the round's timers. A time limit set inside the VM arms
/// a timer when its deadline is before the pending one's, so the last comes
/// first.
unsafe extern "C" fn arm_timers(
    ctx: JSContextRef,
    _: JSObjectRef,
    _: JSObjectRef,
    _: usize,
    _: *const JSValueRef,
    _: *mut JSValueRef,
) -> JSValueRef {
    let set_limit = spi::get().set_execution_time_limit.unwrap();
    let (start, first) = ROUND.lock().unwrap().unwrap();
    unsafe {
        let group = JSContextGetGroup(ctx);
        for i in (0..TIMERS).rev() {
            let limit = (first + STEP * i).saturating_sub(start.elapsed()).as_secs_f64().max(1e-6);
            set_limit(group, limit, Some(count_firing), core::ptr::null_mut());
        }
        JSValueMakeUndefined(ctx)
    }
}

/// What a child of the stress is doing, for the report of one that hangs.
static STAGE: AtomicI32 = AtomicI32::new(0);
const STAGES: [&str; 6] =
    ["in JSDidFork", "reseeding", "waiting for its watchdog", "collecting", "running threads", "collecting at the end"];

unsafe extern "C" fn give_up(_: i32) {
    unsafe { _exit(100 + STAGE.load(Ordering::Relaxed)) }
}

fn outcome(status: i32) -> Option<String> {
    match (status & 0x7f, (status >> 8) & 0xff) {
        (0, 0) => None,
        (0, 3) => Some("its watchdog never fired".into()),
        (0, code @ 100..=105) => Some(format!("hung {}", STAGES[code as usize - 100])),
        (0, code) => Some(format!("exit code {code}")),
        (signal, _) => Some(format!("killed by signal {signal}")),
    }
}

/// One thread outlives JSPrepareForFork: the queue thread that fires watchdog
/// timers ("JSC VMTraps Signal Sender"). It used to keep running through the
/// fork, and a lock it held at that instant stayed locked in the child for
/// good: the watchdog's (the child's watchdog never fires), a VM's trap lock
/// (the child hangs the first time it handles a trap), a ParkingLot bucket (it
/// hangs whenever a thread next parks on that bucket or the table grows) or an
/// allocator lock (the child aborts on it). JSPrepareForFork now holds that
/// thread at a point where it has none.
///
/// The window is some hundred nanoseconds per timer, so this is a stress: the
/// parent arms a burst of timers and forks while they fire, and each child uses
/// what the timer thread could have held. Without the hold 30 of 360 children
/// failed in 30 s on an M1 Max (20 aborted in the allocator, 9 had no watchdog,
/// 1 hung); with it none of 3,314 did. It is ignored because it takes its whole
/// time budget:
///
///     cargo test -p pyro-jsc-sys --test fork -- --ignored
///
/// `PYRO_FORK_STRESS_SECS` sets the budget (default 10), `PYRO_FORK_STRESS_IDLE=1`
/// leaves the timer thread asleep on one far timer (the control: no child fails
/// on any build).
#[test]
#[ignore = "a stress of the fork race, see its comment"]
fn children_of_a_fork_under_firing_timers_run() {
    let _serial = serial();
    let (Some((prepare, did_fork, reseed)), Some(set_limit)) = (spi::get().fork(), spi::get().set_execution_time_limit)
    else {
        eprintln!("skipped: this JavaScriptCore cannot fork");
        return;
    };
    let env = |name: &str| std::env::var(name).ok().and_then(|s| s.parse::<u64>().ok());
    let budget = Duration::from_secs(env("PYRO_FORK_STRESS_SECS").unwrap_or(10));
    let idle = env("PYRO_FORK_STRESS_IDLE") == Some(1);
    const FORKS: usize = 8;
    const THREADS: usize = 40;
    let mut random = std::process::id() as u64 | 1;
    let mut next_random = move || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    unsafe {
        let group = JSContextGroupCreate();
        let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null_mut());
        define(ctx, "fired", fired);
        define(ctx, "armTimers", arm_timers);
        eval(ctx, "function sum(n) { let s = 0; for (let i = 0; i < n; i++) s += i; return s; } sum(1e6)");

        let child = |ctx: JSGlobalContextRef| -> ! {
            signal(SIGALRM, give_up);
            alarm(5);
            did_fork(true);
            STAGE.store(1, Ordering::Relaxed);
            reseed(ctx);
            let run = std::panic::AssertUnwindSafe(|| {
                STAGE.store(2, Ordering::Relaxed);
                FIRED.store(0, Ordering::Relaxed);
                set_limit(group, 0.002, Some(count_firing), core::ptr::null_mut());
                if eval(ctx, WAIT_FOR_WATCHDOG) != "true" {
                    return 3;
                }
                STAGE.store(3, Ordering::Relaxed);
                eval(ctx, "{ let g = []; for (let i = 0; i < 1e5; i++) g.push({ i }); }");
                JSGarbageCollect(ctx);
                // Threads that wait for the VM park, each with a new entry in
                // the ParkingLot, whose table grows: that takes every bucket.
                // Each enters the VM alone first: a thread whose first use of
                // JavaScriptCore is a wait for a VM's lock can pass for the
                // lock's owner (JSLock reads the owner, then creates the
                // thread's own object, which can land where the object of an
                // owner that has finished meanwhile was).
                STAGE.store(4, Ordering::Relaxed);
                let shared = ctx as usize;
                let (alone, all_entered) = (Mutex::new(()), Barrier::new(THREADS));
                let ran = std::thread::scope(|scope| {
                    let threads: Vec<_> = (0..THREADS)
                        .map(|_| {
                            scope.spawn(|| {
                                let ctx = shared as JSGlobalContextRef;
                                let entered = {
                                    let _alone = alone.lock();
                                    eval(ctx, "0") == "0"
                                };
                                all_entered.wait();
                                entered && (0..3).all(|_| eval(ctx, "sum(20000)") == "199990000")
                            })
                        })
                        .collect();
                    threads.into_iter().all(|t| t.join().unwrap_or(false))
                });
                if !ran {
                    return 4;
                }
                STAGE.store(5, Ordering::Relaxed);
                JSGarbageCollect(ctx);
                0
            });
            _exit(std::panic::catch_unwind(run).unwrap_or(2))
        };

        if idle {
            // The timer thread exists once a VM entry has armed a timer.
            set_limit(group, 3600.0, Some(count_firing), core::ptr::null_mut());
            eval(ctx, "0");
        }
        let deadline = Instant::now() + budget;
        let (mut rounds, mut forks, mut not_stopped) = (0u32, 0u32, 0u32);
        let mut prepare_us = Vec::new();
        let mut failures: BTreeMap<String, u32> = BTreeMap::new();
        // Arming takes a while: the first timer is due once that is over.
        let mut first = Duration::from_millis(60);
        while Instant::now() < deadline {
            let window = STEP * TIMERS;
            let start = Instant::now();
            if !idle {
                // A limit past the burst, so that entering the VM arms nothing before it.
                set_limit(group, (first + window).as_secs_f64() + 1.0, Some(count_firing), core::ptr::null_mut());
                *ROUND.lock().unwrap() = Some((start, first));
                eval(ctx, "armTimers()");
            }
            let armed = start.elapsed();
            while start.elapsed() < first {
                std::hint::spin_loop();
            }
            let mut children = Vec::new();
            while children.len() < FORKS && start.elapsed() < first + window {
                let preparing = Instant::now();
                if !prepare(group, 5.0) {
                    did_fork(false);
                    not_stopped += 1;
                    continue;
                }
                prepare_us.push(preparing.elapsed().as_micros());
                let pid = fork();
                if pid == 0 {
                    child(ctx);
                }
                did_fork(false);
                assert!(pid > 0, "fork failed");
                children.push(pid);
                // Spread the forks over the burst.
                let gap = Duration::from_micros(next_random() % 2000);
                let forked = Instant::now();
                while forked.elapsed() < gap {
                    std::hint::spin_loop();
                }
            }
            for pid in children {
                let mut status = -1;
                waitpid(pid, &mut status, 0);
                forks += 1;
                if let Some(failure) = outcome(status) {
                    *failures.entry(failure).or_default() += 1;
                }
            }
            rounds += 1;
            if !idle {
                first = (armed + armed / 4 + Duration::from_millis(2)).max(Duration::from_millis(5));
            }
        }
        prepare_us.sort_unstable();
        let prepare_us = prepare_us.get(prepare_us.len() / 2).copied().unwrap_or(0);
        eprintln!(
            "{forks} forks in {rounds} rounds, {not_stopped} skipped, JSPrepareForFork {prepare_us} us (median): \
             failures {failures:?}"
        );
        assert!(failures.is_empty(), "children failed: {failures:?} of {forks}");
        JSGlobalContextRelease(ctx);
        JSContextGroupRelease(group);
    }
}
