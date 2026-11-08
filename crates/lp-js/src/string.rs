//! Owned JavaScriptCore strings and UTF-8 conversion.

use lp_jsc_sys as sys;
use std::cell::RefCell;
use std::mem::MaybeUninit;

/// An owned, retained `JSStringRef`. Not tied to a context: JSC strings are
/// immutable and usable from any context of the thread that created them.
pub struct JsString(sys::JSStringRef);

impl JsString {
    /// Copies `s` into a new JSC string.
    pub fn new(s: &str) -> JsString {
        // Patched JSC takes Latin-1 with a length: an ASCII string (most
        // names, and script sources) is copied once, with no NUL-terminated
        // copy, scan or decoding first, and text that fits Latin-1 stays
        // 8-bit instead of being widened.
        if let Some(d) = sys::spi::direct_strings() {
            return with_host_units(s, |units| match units {
                // SAFETY: pointer/len describe a live slice; JSC copies it.
                HostUnits::Latin1(b) => JsString(unsafe { (d.create_latin1)(b.as_ptr(), b.len()) }),
                HostUnits::Utf16(u) => Self::from_utf16(u),
            });
        }
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
        // SAFETY: self holds a retain.
        match unsafe { stored_units(self.0) } {
            Some(crate::StringUnits::Latin1(b)) => eq_latin1_str(b, s),
            Some(crate::StringUnits::Utf16(u)) => eq_utf16_str(u, s),
            None => eq_utf16_str(self.utf16(), s),
        }
    }
}

impl std::fmt::Display for JsString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SAFETY: self holds a retain.
        f.write_str(&unsafe { to_rust_string(self.0) })
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

/// A host string's characters in a form JavaScriptCore stores ([`with_host_units`]).
#[derive(Clone, Copy)]
pub(crate) enum HostUnits<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

/// Stack space for transcoding a short non-ASCII string (bytes; half as many
/// UTF-16 units).
const TRANSCODE_STACK: usize = 256;

/// Runs `f` with `s` as JavaScriptCore keeps text: 8-bit (Latin-1) when every
/// scalar fits, as JSC keeps parsed source and JSON, else UTF-16. An ASCII
/// string is passed as it is; other text is transcoded once, on the stack
/// when short.
#[inline]
pub(crate) fn with_host_units<R>(s: &str, f: impl FnOnce(HostUnits<'_>) -> R) -> R {
    if s.is_ascii() {
        return f(HostUnits::Latin1(s.as_bytes()));
    }
    with_transcoded_units(s, f)
}

#[inline(never)]
fn with_transcoded_units<R>(s: &str, f: impl FnOnce(HostUnits<'_>) -> R) -> R {
    let bytes = s.as_bytes();
    // UTF-8 writes U+0080..U+00FF with lead byte 0xC2 or 0xC3 (continuation
    // bytes are 0x80..0xBF, and 0xC0/0xC1 never occur): any byte from 0xC4 up
    // starts a scalar beyond Latin-1.
    if bytes.iter().all(|&b| b < 0xC4) {
        // One Latin-1 byte per scalar: every byte but the continuation bytes.
        let len = bytes.len() - bytes.iter().filter(|&&b| b & 0xC0 == 0x80).count();
        let latin1 = s.chars().map(|c| c as u32 as u8);
        if len <= TRANSCODE_STACK {
            let mut buf = [MaybeUninit::<u8>::uninit(); TRANSCODE_STACK];
            let n = fill(&mut buf, latin1);
            debug_assert_eq!(n, len);
            // SAFETY: the first `n` bytes were written.
            return f(HostUnits::Latin1(unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), n) }));
        }
        let mut v = Vec::with_capacity(len);
        v.extend(latin1);
        return f(HostUnits::Latin1(&v));
    }
    let len = utf16_len(s);
    if len <= TRANSCODE_STACK / 2 {
        let mut buf = [MaybeUninit::<u16>::uninit(); TRANSCODE_STACK / 2];
        let n = fill(&mut buf, s.encode_utf16());
        debug_assert_eq!(n, len);
        // SAFETY: the first `n` units were written.
        return f(HostUnits::Utf16(unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), n) }));
    }
    f(HostUnits::Utf16(&utf16_units(s)))
}

/// Writes `items` to the front of `buf` (as many as fit) and returns how many.
fn fill<T>(buf: &mut [MaybeUninit<T>], items: impl Iterator<Item = T>) -> usize {
    let mut n = 0;
    for (slot, item) in buf.iter_mut().zip(items) {
        slot.write(item);
        n += 1;
    }
    n
}

/// A Latin-1 string as UTF-8: an ASCII one copied as it is.
pub(crate) fn latin1_to_string(b: &[u8]) -> String {
    if b.is_ascii() {
        // SAFETY: ASCII is UTF-8.
        return unsafe { std::str::from_utf8_unchecked(b) }.to_owned();
    }
    let mut out = String::with_capacity(b.len() + b.iter().filter(|&&c| c >= 0x80).count());
    out.extend(b.iter().map(|&c| char::from(c)));
    out
}

/// UTF-16 as UTF-8, unpaired surrogates as U+FFFD; the buffer is sized for
/// the common case (one byte per unit) up front.
pub(crate) fn utf16_to_string(u: &[u16]) -> String {
    let mut out = String::with_capacity(u.len());
    out.extend(char::decode_utf16(u.iter().copied()).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)));
    out
}

/// The characters `s` stores (patched JSC), or `None` without the entry point.
///
/// # Safety
/// `s` must be live for the returned lifetime.
unsafe fn stored_units<'a>(s: sys::JSStringRef) -> Option<crate::StringUnits<'a>> {
    let d = sys::spi::direct_strings()?;
    let (mut len, mut is_8bit) = (0usize, false);
    // SAFETY: a live string; the characters are its own, valid while it is.
    let p = unsafe { (d.stored_characters)(s, &mut len, &mut is_8bit) };
    Some(if len == 0 {
        crate::StringUnits::Latin1(&[])
    } else if is_8bit {
        crate::StringUnits::Latin1(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), len) })
    } else {
        crate::StringUnits::Utf16(unsafe { std::slice::from_raw_parts(p.cast::<u16>(), len) })
    })
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
    // The stored characters, not `utf16_of`: that widens an 8-bit string
    // (most JSON and names) into a second copy the string keeps.
    match unsafe { stored_units(s) } {
        Some(units) => units.to_string_lossy(),
        None => String::from_utf16_lossy(unsafe { utf16_of(s) }),
    }
}

pub(crate) fn eq_latin1_str(a: &[u8], b: &str) -> bool {
    if b.is_ascii() {
        return a == b.as_bytes();
    }
    a.iter().map(|&c| char::from(c)).eq(b.chars())
}

pub(crate) fn eq_utf16_str(a: &[u16], b: &str) -> bool {
    // Fast path for ASCII: compare unit by byte.
    if b.is_ascii() {
        return a.len() == b.len() && a.iter().zip(b.bytes()).all(|(x, y)| *x == y as u16);
    }
    a.iter().copied().eq(b.encode_utf16())
}

thread_local! {
    /// One JSC string per static name, keyed by the name's address and
    /// length: no hashing of the characters, and literals are deduplicated
    /// by the linker, so a name used at many sites is usually one entry (a
    /// duplicate is merely a second entry with equal contents). Entries are
    /// never removed, so a raw `JSStringRef` taken from the table stays valid
    /// for the thread's lifetime.
    static ATOMS: RefCell<rustc_hash::FxHashMap<(usize, usize), JsString>> =
        RefCell::new(rustc_hash::FxHashMap::default());
}

/// The thread's JSC string for the static name `name`, made on first use:
/// constant property keys ([`crate::Scope::get_hidden`],
/// [`crate::Object::get_atom`]) need no string creation and release per
/// access. The reference is borrowed from the thread's table, which keeps it
/// for the thread's lifetime. The table borrow ends before this returns, so
/// callers may enter JSC (which may call back into code that asks for atoms)
/// while they use it.
#[inline]
pub(crate) fn atom_raw(name: &'static str) -> sys::JSStringRef {
    let key = (name.as_ptr() as usize, name.len());
    ATOMS.with(|a| {
        if let Some(s) = a.borrow().get(&key) {
            return s.as_raw();
        }
        let s = JsString::new(name);
        let raw = s.as_raw();
        a.borrow_mut().insert(key, s);
        raw
    })
}

/// Returns a cached JSC string for a static name (property names, event types).
/// Cached per thread for the thread's lifetime.
pub fn atom(name: &'static str) -> JsString {
    // SAFETY: a live string owned by the thread's table; retained here.
    unsafe { JsString::from_raw_borrowed(atom_raw(name)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_are_made_once_per_name() {
        const NAME: &str = "listeners";
        let a = atom_raw(NAME);
        assert_eq!(atom_raw(NAME), a);
        assert!(atom(NAME).eq_str("listeners"));
        // A name at another address is another entry with equal contents.
        let other: &'static str = Box::leak(String::from("listeners").into_boxed_str());
        let b = atom_raw(other);
        assert_ne!(a, b);
        assert_eq!(unsafe { utf16_of(a) }, unsafe { utf16_of(b) });
    }

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
