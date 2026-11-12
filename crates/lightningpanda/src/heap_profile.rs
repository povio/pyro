//! Sampling profile of the live Rust heap (feature `heap-profile`; not in
//! shipped builds). Answers "who owns the Rust heap right now" for a running
//! `serve`, fork children included, without slowing it enough to change what
//! it does.
//!
//! `LIGHTNINGPANDA_HEAP_PROFILE=<path prefix>` turns it on. Every
//! `LIGHTNINGPANDA_HEAP_PROFILE_INTERVAL` bytes a thread allocates (4096 by
//! default, on average: each gap is drawn between half and one and a half
//! times that, so a loop that allocates a fixed number of bytes per turn does
//! not hand every sample to the same one of its allocations) the allocation
//! that crosses the mark is recorded with its call stack and stands for the
//! bytes since the last one; freeing it removes them. `SIGUSR2` asks for a
//! dump, written by the next allocation of any thread to
//! `<prefix>.<pid>.<n>.heap`:
//!
//! ```text
//! load 0x<address of the executable's Mach-O header> interval <bytes>
//! <live bytes> <live samples> <return address> <return address> ...
//! ```
//!
//! one line per call stack, innermost frame first. `bench/scripts/heap-profile.mjs`
//! turns the addresses into symbols (`atos`) and sums by owner.
//!
//! Two more files say what the allocator holds beyond the live bytes:
//! `<prefix>.<pid>.<n>.areas`, one line per allocator page (`dump_areas`), and
//! `<prefix>.<pid>.<n>.sparse.heap`, the same profile restricted to the
//! samples in pages of small and medium blocks that are less than half in
//! use: the owners whose few live blocks keep mostly free pages.
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
    /// The gap to this thread's next sample (0: not drawn yet) and the state
    /// of the generator that draws it.
    static GAP: Cell<(usize, u64)> = const { Cell::new((0, 0)) };
}

/// The bytes until a thread's next sample: `interval` on average, uniform
/// between half and one and a half times it.
fn next_gap(interval: usize, seed: u64) -> (usize, u64) {
    // xorshift64*, seeded per thread from the address of its counter.
    let mut x = if seed == 0 { 0x9e37_79b9_7f4a_7c15 ^ GAP.with(|g| g as *const _ as u64) } else { seed };
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    let r = x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33;
    (interval / 2 + (r as usize) % interval.max(1), x)
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
    let Ok(prefix) = std::env::var("LIGHTNINGPANDA_HEAP_PROFILE") else { return };
    if let Some(interval) = std::env::var("LIGHTNINGPANDA_HEAP_PROFILE_INTERVAL").ok().and_then(|v| v.parse().ok()) {
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
    let Ok(gap) = GAP.try_with(|gap| {
        let (bytes, seed) = gap.get();
        if bytes == 0 {
            gap.set(next_gap(INTERVAL.load(Ordering::Relaxed), seed));
        }
        gap.get().0
    }) else {
        return;
    };
    let sample = since >= gap;
    if !sample && !DUMP_REQUESTED.load(Ordering::Relaxed) {
        return;
    }
    guarded(|| {
        if sample {
            let _ = SINCE.try_with(|since| since.set(0));
            let _ = GAP.try_with(|gap| gap.set(next_gap(INTERVAL.load(Ordering::Relaxed), gap.get().1)));
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
    // What the profiler itself holds in the same heap: every call stack
    // seen so far, and one entry per live sample.
    let own = state.stacks.keys().map(|s| s.len() * 8).sum::<usize>()
        + state.stacks.capacity() * 21
        + state.live.capacity() * 17;
    let mut pages = dump_areas(&format!("{}.{}.{}.areas", state.prefix, std::process::id(), state.dumps), own);
    // The samples that sit in a page of small or medium blocks with less
    // than half of its committed bytes in use: who keeps the allocator's
    // mostly free pages. A second profile in the same format.
    pages.sort_unstable_by_key(|row| row[5]);
    let mut sparse: HashMap<u32, (u64, u64)> = HashMap::new();
    for (at, (id, bytes)) in &state.live {
        let next = pages.partition_point(|row| row[5] <= *at);
        let Some(&[size, reserved, committed, used, _, start]) = next.checked_sub(1).map(|i| &pages[i]) else { continue };
        if *at < start + reserved && size <= 65536 && used * 2 < committed {
            let entry = sparse.entry(*id).or_default();
            entry.0 += u64::from(*bytes);
            entry.1 += 1;
        }
    }
    let sparse_path = format!("{}.{}.{}.sparse.heap", state.prefix, std::process::id(), state.dumps);
    state.dumps += 1;
    for (path, by_stack) in [(path, by_stack), (sparse_path, sparse)] {
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
}

/// What the allocator holds against what is in use (returned as rows of
/// block size, reserved, committed, in use, thread, first block): one line
/// `<block size> <bytes reserved> <bytes committed> <bytes in use> <thread>`
/// per allocator page, after `# profiler <bytes>` (what the profiler's own
/// tables hold) and `# thread <id>` (the dumping thread). `<thread>` is the
/// page's owner as mimalloc numbers threads, 0 or 4 for a page whose thread
/// ended. The difference between the heap's footprint and the live bytes of
/// the profile is here: pages kept for a few live blocks, and pages of
/// threads that have not collected. Read while other threads allocate, so
/// the counts are approximate.
fn dump_areas(path: &str, profiler: usize) -> Vec<[usize; 6]> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Area {
        blocks: *mut c_void,
        reserved: usize,
        committed: usize,
        used: usize,
        block_size: usize,
        full_block_size: usize,
        /// The `mi_page_t`, whose first word is its thread's id and two flag
        /// bits (mimalloc v3's layout: this is a diagnostic build).
        page: *const usize,
    }
    type Visitor = extern "C" fn(*const c_void, *const Area, *mut c_void, usize, *mut c_void) -> bool;
    unsafe extern "C" {
        fn mi_heap_main() -> *mut c_void;
        fn mi_heap_visit_blocks(heap: *mut c_void, visit_blocks: bool, visitor: Visitor, arg: *mut c_void) -> bool;
    }
    /// (block size, reserved, committed, in use, thread, first block).
    type Row = [usize; 6];
    extern "C" fn visit(_heap: *const c_void, area: *const Area, _block: *mut c_void, _size: usize, arg: *mut c_void) -> bool {
        // SAFETY: mimalloc passes a valid area and the vector given below.
        let (area, rows) = unsafe { (&*area, &mut *(arg as *mut Vec<Row>)) };
        // No allocation in here: the visitor runs inside the allocator.
        if rows.len() < rows.capacity() {
            // SAFETY: the page outlives the visit of its area.
            let thread = unsafe { area.page.read_volatile() } & !3;
            rows.push([
                area.full_block_size,
                area.reserved,
                area.committed,
                area.used * area.full_block_size,
                thread,
                area.blocks as usize,
            ]);
        }
        true
    }
    let mut rows: Vec<Row> = Vec::with_capacity(16384);
    // A block of this thread's, to name the thread as mimalloc does.
    let probe = Box::new(0u64);
    // SAFETY: mimalloc's public API; the visitor only reads the area.
    unsafe { mi_heap_visit_blocks(mi_heap_main(), false, visit, (&raw mut rows).cast()) };
    let at = &raw const *probe as usize;
    let me = rows.iter().find(|r| r[0] <= 64 && (r[5]..r[5] + r[1]).contains(&at)).map_or(0, |r| r[4]);
    let Ok(file) = std::fs::File::create(path) else { return rows };
    let mut out = std::io::BufWriter::new(file);
    rows.sort_unstable();
    let _ = writeln!(out, "# profiler {profiler}");
    let _ = writeln!(out, "# thread {me}");
    for [size, reserved, committed, used, thread, _] in &rows {
        let _ = writeln!(out, "{size} {reserved} {committed} {used} {thread}");
    }
    rows
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
