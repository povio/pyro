//! `JsString::new` converts large strings (e.g. classic script sources) in at
//! most one Rust allocation of exactly the final size, never regrown. With
//! patched JSC (`webkit/patches/0016`) an ASCII string needs none (JSC copies
//! it as Latin-1), other text that fits Latin-1 one byte per scalar, the rest
//! UTF-16; the system framework takes a NUL-terminated copy for ASCII instead.
//! This binary counts the Rust heap operations of the conversion on the test
//! thread; JSC's own copy does not go through the Rust allocator.

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
    let direct = lp_js::sys::spi::direct_strings().is_some();
    let ascii = "x".repeat(3 << 20);
    let mixed = "žabe 🐸 €\0".repeat((3 << 20) / 15);
    // 3-byte UTF-8 scalars: one UTF-16 unit for every three bytes.
    let cjk = "漢字テキスト".repeat((3 << 20) / 18);
    let latin1 = "café über ".repeat((3 << 20) / 12);
    for s in [&ascii, &mixed, &cjk, &latin1] {
        let expected = match (direct, s.is_ascii(), s.chars().all(|c| u32::from(c) < 0x100)) {
            (true, true, _) => (0, 0, 0),
            (true, false, true) => (1, 0, s.chars().count()),
            (false, true, _) => (1, 0, s.len() + 1),
            _ => (1, 0, 2 * s.encode_utf16().count()),
        };
        assert_eq!(heap_ops(s), expected, "at most one buffer of exactly the final size, never regrown");
    }
    // Short strings use a stack buffer (or none).
    assert_eq!(heap_ops("short"), (0, 0, 0));
    assert_eq!(heap_ops("žabe 🐸"), (0, 0, 0));
    if direct {
        assert_eq!(heap_ops("café"), (0, 0, 0));
    }
}
