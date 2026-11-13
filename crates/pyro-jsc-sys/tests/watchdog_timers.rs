//! `VMTraps::queue()` is one thread per process, and every watchdog of every VM
//! arms its one-shot timers on it. A timer's `ScheduledTask` used to keep
//! `CheckedPtr`s to its former tree children after it left the tree
//! (`webkit/patches/0007`). `dispatchAfter` drops the caller's timer reference
//! only after arming it, so when the caller stalls right there the timer can
//! fire, and a former child of it can fire and die, first. WTF then scribbles and
//! leaks the child, and the caller's drop crashes on the scribbled count:
//! EXC_BREAKPOINT in `~TimerBase` on the thread running JavaScript.
//!
//! The stall cannot be forced through the C API, so this is a stress: VMs on
//! low QoS threads, which lose the CPU to the timer thread, arm a nearly
//! expired timer on every entry. Without the patch it usually crashes this test
//! binary within seconds, but a run can survive. Passing is evidence, not proof.
//! It is ignored because it saturates the machine for its whole time budget:
//!
//!     cargo test -p pyro-jsc-sys --test watchdog_timers -- --ignored
//!
//! `PYRO_WATCHDOG_STRESS_SECS` sets the budget (default 10).

use pyro_jsc_sys::*;
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
}
const QOS_CLASS_UTILITY: u32 = 0x11;

unsafe extern "C" fn keep_running(_: JSContextRef, _: *mut core::ffi::c_void) -> bool {
    false
}

#[test]
#[ignore = "a stress of the timer race, see the head of this file"]
fn watchdog_timers_of_many_vms_survive_a_stalled_caller() {
    let Some(set_limit) = spi::get().set_execution_time_limit else {
        eprintln!("skipped: this JavaScriptCore has no execution time limit");
        return;
    };
    let budget = std::env::var("PYRO_WATCHDOG_STRESS_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    let deadline = Instant::now() + Duration::from_secs(budget);
    let cores = std::thread::available_parallelism().map_or(8, usize::from);
    static ENTRIES: AtomicU64 = AtomicU64::new(0);

    let threads: Vec<_> = (0..cores * 4)
        .map(|n| {
            std::thread::spawn(move || unsafe {
                // Most callers run at a QoS that loses the CPU to the timer thread.
                if n % 8 != 0 {
                    pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
                }
                let group = JSContextGroupCreate();
                let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null_mut());
                let src = CString::new("0").unwrap();
                let script = JSStringCreateWithUTF8CString(src.as_ptr());
                // Different limits so that timers interleave in the tree.
                set_limit(group, 1e-6 * (1 + n % 16) as f64, Some(keep_running), core::ptr::null_mut());
                while Instant::now() < deadline {
                    JSEvaluateScript(
                        ctx,
                        script,
                        core::ptr::null_mut(),
                        core::ptr::null_mut(),
                        1,
                        core::ptr::null_mut(),
                    );
                    ENTRIES.fetch_add(1, Ordering::Relaxed);
                }
                JSStringRelease(script);
                JSGlobalContextRelease(ctx);
                JSContextGroupRelease(group);
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    eprintln!("{} VM entries in {budget} s", ENTRIES.load(Ordering::Relaxed));
}
