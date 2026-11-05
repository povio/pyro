//! `JsString::new` converts large strings (e.g. classic script sources) in one
//! Rust allocation of exactly the final size, never regrown: a NUL-terminated
//! copy for ASCII (JSC keeps it as 8-bit characters), UTF-16 otherwise. This
//! binary counts the Rust heap operations of the conversion on the test thread;
//! JSC's own copy does not go through the Rust allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use lp_js::JsString;

struct Counting;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static REALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

fn bump(counter: &'static std::thread::LocalKey<Cell<usize>>, bytes: usize) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = counter.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + bytes));
    }
}

// SAFETY: forwards to the system allocator; the counters never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(&ALLOCS, layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(&REALLOCS, new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// (allocations, reallocations, requested bytes) of `JsString::new(s)` on
/// this thread.
fn heap_ops(s: &str) -> (usize, usize, usize) {
    ALLOCS.with(|c| c.set(0));
    REALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    TRACK.with(|t| t.set(true));
    let j = JsString::new(s);
    TRACK.with(|t| t.set(false));
    drop(j);
    (ALLOCS.with(Cell::get), REALLOCS.with(Cell::get), BYTES.with(Cell::get))
}

#[test]
fn large_strings_convert_without_regrowing_the_buffer() {
    let ascii = "x".repeat(3 << 20);
    let mixed = "žabe 🐸 €\0".repeat((3 << 20) / 15);
    // 3-byte UTF-8 scalars: one UTF-16 unit for every three bytes.
    let cjk = "漢字テキスト".repeat((3 << 20) / 18);
    for s in [&ascii, &mixed, &cjk] {
        let exact = if s.is_ascii() { s.len() + 1 } else { 2 * s.encode_utf16().count() };
        assert_eq!(heap_ops(s), (1, 0, exact), "one buffer of exactly the final size, never regrown");
    }
    // Short NUL-free strings use the stack buffer path.
    assert_eq!(heap_ops("short"), (0, 0, 0));
}
