//! Sampling profile of the live Rust heap (feature `heap-profile`; not in
//! shipped builds). Answers "who owns the Rust heap right now" for a running
//! `serve`, fork children included, without slowing it enough to change what
//! it does.
//!
//! `PYRO_HEAP_PROFILE=<path prefix>` turns it on. Every
//! `PYRO_HEAP_PROFILE_INTERVAL` bytes a thread allocates (4096 by
//! default) the allocation that crosses the mark is recorded with its call
//! stack and stands for the bytes since the last one; freeing it removes
//! them. `SIGUSR2` asks for a dump, written by the next allocation of any
//! thread to `<prefix>.<pid>.<n>.heap`:
//!
//! ```text
//! load 0x<address of the executable's Mach-O header> interval <bytes>
//! <live bytes> <live samples> <return address> <return address> ...
//! ```
//!
//! one line per call stack, innermost frame first. `bench/scripts/heap-profile.mjs`
//! turns the addresses into symbols (`atos`) and sums by owner.
//!
//! Allocations made before `main` and by the profiler itself are not counted;
//! JavaScriptCore's own heap is not Rust's and is not seen at all.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// The global allocator of a `heap-profile` build: mimalloc, watched.
pub struct Profiled(pub mimalloc::MiMalloc);

static ENABLED: AtomicBool = AtomicBool::new(false);
static DUMP_REQUESTED: AtomicBool = AtomicBool::new(false);
static INTERVAL: AtomicUsize = AtomicUsize::new(4096);

thread_local! {
    /// Set while the profiler itself runs: what it allocates is not sampled.
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// Bytes this thread allocated since its last sample.
    static SINCE: Cell<usize> = const { Cell::new(0) };
}

/// Frames kept per sample, after the profiler's own.
const DEPTH: usize = 40;

struct State {
    prefix: String,
    dumps: u32,
    /// Sampled live block → (call stack, bytes it stands for).
    live: HashMap<usize, (u32, u32)>,
    stacks: HashMap<Box<[usize]>, u32>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Reads the environment; call first thing in `main`.
pub fn init() {
    let Ok(prefix) = std::env::var("PYRO_HEAP_PROFILE") else { return };
    if let Some(interval) = std::env::var("PYRO_HEAP_PROFILE_INTERVAL").ok().and_then(|v| v.parse().ok()) {
        INTERVAL.store(interval, Ordering::Relaxed);
    }
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(State { prefix, dumps: 0, live: HashMap::new(), stacks: HashMap::new() });
    extern "C" fn request_dump(_signal: libc::c_int) {
        DUMP_REQUESTED.store(true, Ordering::Relaxed);
    }
    // SAFETY: the handler only stores to an atomic.
    unsafe { libc::signal(libc::SIGUSR2, request_dump as *const () as libc::sighandler_t) };
    ENABLED.store(true, Ordering::Relaxed);
}

fn guarded(f: impl FnOnce()) {
    let _ = BUSY.try_with(|busy| {
        if !busy.replace(true) {
            f();
            busy.set(false);
        }
    });
}

#[inline]
fn allocated(ptr: *mut u8, size: usize) {
    if !ENABLED.load(Ordering::Relaxed) || ptr.is_null() {
        return;
    }
    let Ok(since) = SINCE.try_with(|since| {
        let bytes = since.get() + size;
        since.set(bytes);
        bytes
    }) else {
        return;
    };
    let sample = since >= INTERVAL.load(Ordering::Relaxed);
    if !sample && !DUMP_REQUESTED.load(Ordering::Relaxed) {
        return;
    }
    guarded(|| {
        if sample {
            let _ = SINCE.try_with(|since| since.set(0));
            let mut frames = [std::ptr::null_mut::<libc::c_void>(); DEPTH + 3];
            // SAFETY: `backtrace` writes at most the given number of frames.
            let count = unsafe { libc::backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int) } as usize;
            // The first three frames are this function's callers inside the allocator.
            let stack: Box<[usize]> = frames[3.min(count)..count].iter().map(|f| *f as usize).collect();
            let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = state.as_mut() {
                let next = state.stacks.len() as u32;
                let id = *state.stacks.entry(stack).or_insert(next);
                state.live.insert(ptr as usize, (id, since.min(u32::MAX as usize) as u32));
            }
        }
        if DUMP_REQUESTED.swap(false, Ordering::Relaxed) {
            dump();
        }
    });
}

#[inline]
fn freed(ptr: *mut u8) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    guarded(|| {
        if let Some(state) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            state.live.remove(&(ptr as usize));
        }
    });
}

fn load_address() -> usize {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn _dyld_get_image_header(index: u32) -> *const libc::c_void;
        }
        // SAFETY: image 0 is the executable and always exists.
        unsafe { _dyld_get_image_header(0) as usize }
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

fn dump() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(state) = state.as_mut() else { return };
    let mut by_stack: HashMap<u32, (u64, u64)> = HashMap::new();
    for (id, bytes) in state.live.values() {
        let entry = by_stack.entry(*id).or_default();
        entry.0 += u64::from(*bytes);
        entry.1 += 1;
    }
    let path = format!("{}.{}.{}.heap", state.prefix, std::process::id(), state.dumps);
    state.dumps += 1;
    let Ok(file) = std::fs::File::create(&path) else { return };
    let mut out = std::io::BufWriter::new(file);
    let _ = writeln!(out, "load {:#x} interval {}", load_address(), INTERVAL.load(Ordering::Relaxed));
    for (stack, id) in &state.stacks {
        let Some((bytes, samples)) = by_stack.get(id) else { continue };
        let _ = write!(out, "{bytes} {samples}");
        for frame in stack.iter() {
            let _ = write!(out, " {frame:#x}");
        }
        let _ = writeln!(out);
    }
}

// SAFETY: every call is forwarded to mimalloc unchanged; the bookkeeping
// around it never touches the block.
unsafe impl GlobalAlloc for Profiled {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc(layout) };
        allocated(ptr, layout.size());
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.0.alloc_zeroed(layout) };
        allocated(ptr, layout.size());
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        freed(ptr);
        unsafe { self.0.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        freed(ptr);
        let new = unsafe { self.0.realloc(ptr, layout, new_size) };
        allocated(new, new_size);
        new
    }
}
