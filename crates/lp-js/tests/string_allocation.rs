//! `JsString::new` converts large strings (script sources, response text) to
//! UTF-16 in one Rust allocation of the final size, never regrown. This binary
//! counts the Rust heap operations of the conversion on the test thread; JSC's
//! own copy does not go through the Rust allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use lp_js::JsString;

struct Counting;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static REALLOCS: Cell<usize> = const { Cell::new(0) };
}

fn bump(counter: &'static std::thread::LocalKey<Cell<usize>>) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = counter.try_with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: forwards to the system allocator; the counters never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(&REALLOCS);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// (allocations, reallocations) made by `JsString::new(s)` on this thread.
fn heap_ops(s: &str) -> (usize, usize) {
    ALLOCS.with(|c| c.set(0));
    REALLOCS.with(|c| c.set(0));
    TRACK.with(|t| t.set(true));
    let j = JsString::new(s);
    TRACK.with(|t| t.set(false));
    drop(j);
    (ALLOCS.with(Cell::get), REALLOCS.with(Cell::get))
}

#[test]
fn large_strings_convert_without_regrowing_the_buffer() {
    let ascii = "x".repeat(3 << 20);
    let mixed = "žabe 🐸 €\0".repeat((3 << 20) / 15);
    for s in [&ascii, &mixed] {
        assert_eq!(heap_ops(s), (1, 0), "one UTF-16 buffer of the final size, never regrown");
    }
    // Short NUL-free strings use the stack buffer path.
    assert_eq!(heap_ops("short"), (0, 0));
}
