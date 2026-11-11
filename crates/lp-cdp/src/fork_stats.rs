//! `LIGHTNINGPANDA_FORK_STATS=<file>`: what each process of a `serve --fork`
//! server costs, as JSON lines appended to `<file>` (a diagnostic; off unless
//! set). Templates log every fork (`"e":"fork"`, with their own use so far)
//! and every child they reap (`"e":"reaped"`, the kernel's account, exit
//! included); forked children log their own use at each step of their life
//! (`"e":"child"`: forked, taken by a connection, served, closed, dropped).

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};

static FILE: OnceLock<Option<std::fs::File>> = OnceLock::new();

fn file() -> Option<&'static std::fs::File> {
    FILE.get_or_init(|| {
        let path = std::env::var_os("LIGHTNINGPANDA_FORK_STATS")?;
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
    })
    .as_ref()
}

/// Opens the file (before the first fork, so that children share it).
pub(crate) fn init() -> bool {
    file().is_some()
}

fn log(line: Value) {
    if let Some(mut f) = file() {
        let mut text = line.to_string();
        text.push('\n');
        // One write: O_APPEND keeps lines of concurrent processes whole.
        let _ = f.write_all(text.as_bytes());
    }
}

/// Resource use of this process so far.
#[derive(Clone, Copy, Default)]
pub(crate) struct Usage {
    user_us: u64,
    sys_us: u64,
    faults: u64,
    cow_faults: u64,
    pageins: u64,
    vcsw: u64,
    ivcsw: u64,
    footprint_kb: u64,
    /// Instructions retired (millions): the work done, whatever the load.
    minstr: u64,
    syscalls: u64,
}

fn tv_us(tv: libc::timeval) -> u64 {
    tv.tv_sec as u64 * 1_000_000 + tv.tv_usec as u64
}

/// CPU time the calling thread has used (the `LP_RUN_STEP_TRACE` wait
/// records: a wait whose CPU time is far below its wall time was idle or
/// had no CPU to run on).
pub(crate) fn thread_cpu() -> std::time::Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid timespec out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    std::time::Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct TaskEventsInfo {
    faults: i32,
    pageins: i32,
    cow_faults: i32,
    messages_sent: i32,
    messages_received: i32,
    syscalls_mach: i32,
    syscalls_unix: i32,
    csw: i32,
}

impl Usage {
    fn now() -> Usage {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut usage = Usage {
            user_us: tv_us(ru.ru_utime),
            sys_us: tv_us(ru.ru_stime),
            faults: ru.ru_minflt as u64 + ru.ru_majflt as u64,
            pageins: ru.ru_majflt as u64,
            vcsw: ru.ru_nvcsw as u64,
            ivcsw: ru.ru_nivcsw as u64,
            ..Usage::default()
        };
        #[cfg(target_os = "macos")]
        unsafe {
            const TASK_EVENTS_INFO: u32 = 2;
            let mut info = TaskEventsInfo::default();
            let mut count = (std::mem::size_of::<TaskEventsInfo>() / 4) as libc::mach_msg_type_number_t;
            #[allow(deprecated)]
            let task = libc::mach_task_self();
            if libc::task_info(task, TASK_EVENTS_INFO, (&mut info as *mut TaskEventsInfo).cast(), &mut count) == 0 {
                usage.faults = info.faults as u64;
                usage.cow_faults = info.cow_faults as u64;
                usage.pageins = info.pageins as u64;
                usage.syscalls = info.syscalls_unix as u64 + info.syscalls_mach as u64;
            }
            let mut ri: libc::rusage_info_v4 = std::mem::zeroed();
            if libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V4,
                (&mut ri as *mut libc::rusage_info_v4).cast(),
            ) == 0
            {
                usage.footprint_kb = ri.ri_phys_footprint / 1024;
                usage.minstr = ri.ri_instructions / 1_000_000;
            }
        }
        usage
    }

    fn json(&self) -> Value {
        json!({
            "user_us": self.user_us,
            "sys_us": self.sys_us,
            "faults": self.faults,
            "cow_faults": self.cow_faults,
            "pageins": self.pageins,
            "vcsw": self.vcsw,
            "ivcsw": self.ivcsw,
            "footprint_kb": self.footprint_kb,
            "minstr": self.minstr,
            "syscalls": self.syscalls,
        })
    }
}

/// This child's steps so far: (name, when, use).
static MARKS: Mutex<Vec<(&'static str, Instant, Usage)>> = Mutex::new(Vec::new());

/// In a template, right after `fork()` returned `child`, which took `fork_us`.
pub(crate) fn forked(kind: &str, child: libc::pid_t, fork_us: u64) {
    if file().is_none() {
        return;
    }
    log(json!({
        "e": "fork",
        "kind": kind,
        "pid": std::process::id(),
        "child": child,
        "fork_us": fork_us,
        "self": Usage::now().json(),
    }));
}

/// In a forked child, right after the fork: its first step.
pub(crate) fn child_started() {
    if file().is_none() {
        return;
    }
    let mut marks = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    marks.clear();
    marks.push(("fork", Instant::now(), Usage::now()));
}

/// In a forked child: a step of its life (nothing in other processes).
pub(crate) fn mark(step: &'static str) {
    if file().is_none() {
        return;
    }
    let mut marks = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    if !marks.is_empty() {
        marks.push((step, Instant::now(), Usage::now()));
    }
}

/// In a forked child about to exit: logs its steps.
pub(crate) fn child_done(kind: &str, profile: Option<usize>) {
    if file().is_none() {
        return;
    }
    let marks = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, start, _)) = marks.first() else { return };
    let steps: Vec<Value> = marks
        .iter()
        .map(|(name, at, usage)| {
            let mut v = usage.json();
            v["at"] = (*name).into();
            v["ms"] = (at.duration_since(*start).as_secs_f64() * 1000.0).into();
            v
        })
        .collect();
    log(json!({
        "e": "child",
        "kind": kind,
        "pid": std::process::id(),
        "ppid": unsafe { libc::getppid() },
        "profile": profile,
        "steps": steps,
    }));
}

/// In a template: a child it reaped, as the kernel accounts it (its exit
/// and its own reaped children included).
pub(crate) fn reaped(pid: libc::pid_t, ru: &libc::rusage) {
    if file().is_none() {
        return;
    }
    // Bytes on macOS, kilobytes elsewhere.
    let maxrss_kb = if cfg!(target_os = "macos") { ru.ru_maxrss as u64 / 1024 } else { ru.ru_maxrss as u64 };
    log(json!({
        "e": "reaped",
        "pid": pid,
        "by": std::process::id(),
        "user_us": tv_us(ru.ru_utime),
        "sys_us": tv_us(ru.ru_stime),
        "faults": ru.ru_minflt as u64 + ru.ru_majflt as u64,
        "pageins": ru.ru_majflt as u64,
        "vcsw": ru.ru_nvcsw as u64,
        "ivcsw": ru.ru_nivcsw as u64,
        "maxrss_kb": maxrss_kb,
    }));
}
