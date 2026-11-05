//! Owned JavaScriptCore strings and UTF-8 conversion.

use lp_jsc_sys as sys;
use std::cell::RefCell;
use std::collections::HashMap;

/// An owned, retained `JSStringRef`. Not tied to a context: JSC strings are
/// immutable and usable from any context of the thread that created them.
pub struct JsString(sys::JSStringRef);

impl JsString {
    /// Copies `s` into a new JSC string.
    pub fn new(s: &str) -> JsString {
        // NUL-free strings go through the UTF-8 C-string entry point: short ones
        // from a stack buffer, ASCII ones of any length from a heap copy. JSC
        // keeps an all-ASCII UTF-8 string as 8-bit characters, half the memory
        // of the UTF-16 form and faster to lex, which matters for script
        // sources (a bundle is megabytes). Everything else is transcoded to
        // UTF-16 once.
        const STACK: usize = 128;
        let bytes = s.as_bytes();
        if !bytes.contains(&0) {
            if s.len() < STACK {
                let mut buf = [0u8; STACK];
                buf[..s.len()].copy_from_slice(bytes);
                // SAFETY: buf is NUL-terminated (zeroed tail) and valid UTF-8.
                return JsString(unsafe { sys::JSStringCreateWithUTF8CString(buf.as_ptr().cast()) });
            }
            if bytes.is_ascii() {
                let mut buf = Vec::with_capacity(s.len() + 1);
                buf.extend_from_slice(bytes);
                buf.push(0);
                // SAFETY: buf is NUL-terminated and valid UTF-8.
                return JsString(unsafe { sys::JSStringCreateWithUTF8CString(buf.as_ptr().cast()) });
            }
        }
        Self::from_utf16(&utf16_units(s))
    }

    pub fn from_utf16(chars: &[u16]) -> JsString {
        // SAFETY: pointer/len describe a live slice; JSC copies it.
        JsString(unsafe { sys::JSStringCreateWithCharacters(chars.as_ptr(), chars.len()) })
    }

    /// Takes ownership of a +1 retained string returned by JSC.
    ///
    /// # Safety
    /// `raw` must be a non-null string whose reference the caller owns.
    pub unsafe fn from_raw_owned(raw: sys::JSStringRef) -> JsString {
        debug_assert!(!raw.is_null());
        JsString(raw)
    }

    /// Retains a borrowed string (e.g. a property name passed to a callback).
    ///
    /// # Safety
    /// `raw` must be a live, non-null string.
    pub unsafe fn from_raw_borrowed(raw: sys::JSStringRef) -> JsString {
        JsString(unsafe { sys::JSStringRetain(raw) })
    }

    pub fn as_raw(&self) -> sys::JSStringRef {
        self.0
    }

    pub fn utf16(&self) -> &[u16] {
        // SAFETY: the characters pointer is valid while self holds a retain.
        unsafe { utf16_of(self.0) }
    }

    pub fn len_utf16(&self) -> usize {
        // SAFETY: live string.
        unsafe { sys::JSStringGetLength(self.0) }
    }

    pub fn eq_str(&self, s: &str) -> bool {
        eq_utf16_str(self.utf16(), s)
    }
}

impl std::fmt::Display for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&String::from_utf16_lossy(self.utf16()))
    }
}

impl std::fmt::Debug for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string())
    }
}

impl Clone for JsString {
    fn clone(&self) -> Self {
        // SAFETY: live string.
        JsString(unsafe { sys::JSStringRetain(self.0) })
    }
}

impl Drop for JsString {
    fn drop(&mut self) {
        // SAFETY: we own one retain.
        unsafe { sys::JSStringRelease(self.0) }
    }
}

/// UTF-16 code units of `s` in one allocation of exactly the needed size;
/// `collect()` regrows the buffer for large strings (see
/// `tests/string_allocation.rs`).
pub(crate) fn utf16_units(s: &str) -> Vec<u16> {
    let len = utf16_len(s);
    let mut units = Vec::with_capacity(len);
    units.extend(s.encode_utf16());
    debug_assert_eq!(units.len(), len);
    units
}

/// Number of UTF-16 code units in `s`: one per scalar value (every byte that
/// is not a UTF-8 continuation byte `10xxxxxx`), plus one more for each 4-byte
/// scalar (lead byte `11110xxx`), which needs a surrogate pair.
pub(crate) fn utf16_len(s: &str) -> usize {
    let bytes = s.as_bytes();
    if bytes.is_ascii() {
        return bytes.len();
    }
    // A byte adds at most 2, so a 127-byte chunk's count fits in a `u8`; the
    // narrow accumulator lets the compiler vectorize the scan.
    bytes
        .chunks(127)
        .map(|chunk| {
            let n = chunk.iter().fold(0u8, |n, &b| n + u8::from(b & 0xC0 != 0x80) + u8::from(b >= 0xF0));
            usize::from(n)
        })
        .sum()
}

/// # Safety
/// `s` must be live for the returned lifetime.
pub(crate) unsafe fn utf16_of<'a>(s: sys::JSStringRef) -> &'a [u16] {
    unsafe {
        let len = sys::JSStringGetLength(s);
        if len == 0 {
            return &[];
        }
        std::slice::from_raw_parts(sys::JSStringGetCharactersPtr(s), len)
    }
}

/// Converts a borrowed JSC string to UTF-8 (lone surrogates become U+FFFD).
///
/// # Safety
/// `s` must be a live string.
pub(crate) unsafe fn to_rust_string(s: sys::JSStringRef) -> String {
    String::from_utf16_lossy(unsafe { utf16_of(s) })
}

pub(crate) fn eq_utf16_str(a: &[u16], b: &str) -> bool {
    // Fast path for ASCII: compare unit by byte.
    if b.is_ascii() {
        return a.len() == b.len() && a.iter().zip(b.bytes()).all(|(x, y)| *x == y as u16);
    }
    a.iter().copied().eq(b.encode_utf16())
}

thread_local! {
    static ATOMS: RefCell<HashMap<&'static str, JsString>> = RefCell::new(HashMap::new());
}

/// Returns a cached JSC string for a static name (property names, event types).
/// Cached per thread for the thread's lifetime.
pub fn atom(name: &'static str) -> JsString {
    ATOMS.with(|a| a.borrow_mut().entry(name).or_insert_with(|| JsString::new(name)).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_utf8_utf16_and_nul() {
        for s in ["", "abc", "žabe 🐸", "a\0b", &"x".repeat(1000)] {
            let j = JsString::new(s);
            assert_eq!(j.to_string(), s);
            assert!(j.eq_str(s));
        }
        assert!(!JsString::new("ab").eq_str("abc"));
    }

    #[test]
    fn utf16_units_match_encode_utf16_without_growth() {
        // 1-, 2-, 3- and 4-byte UTF-8 (the last needs a surrogate pair), NUL,
        // and the extremes of each encoding length.
        let mixed = "a\0ž€🐸".repeat(300);
        let edges = "\u{7f}\u{80}\u{7ff}\u{800}\u{ffff}\u{10000}\u{10ffff}";
        let cjk = "漢字テキスト".repeat(1000);
        for s in ["", "a", "ž", "€", "🐸", "a\0b", edges, mixed.as_str(), cjk.as_str()] {
            let expected: Vec<u16> = s.encode_utf16().collect();
            assert_eq!(utf16_len(s), expected.len(), "{s:?}");
            let units = utf16_units(s);
            assert_eq!(units, expected, "{s:?}");
            assert!(units.capacity() >= units.len(), "{s:?}");
        }
    }

    #[test]
    fn large_multibyte_strings_roundtrip() {
        for unit in ["x", "žabe 🐸 ", "€\0", "漢字テキスト"] {
            let s = unit.repeat((5 << 20) / unit.len());
            let j = JsString::new(&s);
            assert_eq!(j.len_utf16(), s.encode_utf16().count());
            assert!(j.eq_str(&s));
            assert_eq!(j.to_string(), s);
        }
    }
}
