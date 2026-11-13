//! TextEncoder/TextDecoder, atob/btoa and BufferSource <-> bytes helpers.
//!
//! Typed-array access goes through the JSC C API directly. Every raw pointer
//! obtained from JSC is used immediately, before any call that could run JS
//! or allocate on the JS heap (which might move or detach the buffer).

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use pyro_js::sys;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::util::*;

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// Encodes `input` for a URL query or form submission, writing each character
/// `enc` cannot represent as `prefix`, its decimal code point, then `suffix`
/// (URL spec "percent-encode after encoding"; HTML's `&#NNN;` error mode).
pub(crate) fn encode_with_ncr(enc: &'static encoding_rs::Encoding, input: &str, prefix: &str, suffix: &str) -> Vec<u8> {
    let mut encoder = enc.new_encoder();
    let mut out = Vec::new();
    let mut rest = input;
    loop {
        let needed = encoder.max_buffer_length_from_utf8_without_replacement(rest.len()).unwrap_or(rest.len() * 4 + 8);
        out.reserve(needed);
        let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut out, true);
        rest = &rest[read..];
        match result {
            encoding_rs::EncoderResult::InputEmpty => return out,
            encoding_rs::EncoderResult::OutputFull => {}
            encoding_rs::EncoderResult::Unmappable(c) => {
                out.extend_from_slice(format!("{prefix}{}{suffix}", c as u32).as_bytes());
            }
        }
    }
}

fn check_exc(s: &Scope<'_>, exc: sys::JSValueRef) -> JsResult<()> {
    if exc.is_null() {
        return Ok(());
    }
    // SAFETY: `exc` is a live exception value JSC just returned on this thread
    // for this context; wrapping it roots it for the scope's lifetime.
    let v = unsafe { Value::from_raw(s.ctx(), exc) };
    Err(JsError::thrown(pyro_js::Persistent::new(s, v)))
}

/// Copies the bytes viewed by an ArrayBuffer, TypedArray or DataView.
/// Returns `None` when `v` is not a BufferSource.
pub fn buffer_source_bytes(s: &Scope<'_>, v: Value<'_>) -> JsResult<Option<Vec<u8>>> {
    let Some(obj) = v.as_object() else { return Ok(None) };
    let ctx = s.ctx();
    let mut exc = std::ptr::null();
    // SAFETY: `obj` is a live object rooted by the caller's stack frame.
    let ty = unsafe { sys::JSValueGetTypedArrayType(ctx, obj.as_raw() as sys::JSValueRef, &mut exc) };
    check_exc(s, exc)?;
    match ty {
        sys::kJSTypedArrayTypeArrayBuffer => {
            let mut exc = std::ptr::null();
            // SAFETY: obj is an ArrayBuffer; pointer/length are read and the
            // bytes copied before any further JS execution.
            unsafe {
                let len = sys::JSObjectGetArrayBufferByteLength(ctx, obj.as_raw(), &mut exc);
                check_exc(s, exc)?;
                let ptr = sys::JSObjectGetArrayBufferBytesPtr(ctx, obj.as_raw(), &mut exc) as *const u8;
                check_exc(s, exc)?;
                if ptr.is_null() || len == 0 {
                    return Ok(Some(Vec::new()));
                }
                Ok(Some(std::slice::from_raw_parts(ptr, len).to_vec()))
            }
        }
        sys::kJSTypedArrayTypeNone => {
            // Captured native getters perform the DataView brand check and
            // ignore spoofable constructor/name/buffer properties. Never recurse
            // through an arbitrary user-provided "buffer" object.
            let page = crate::target::page_of(s)?;
            let reader = page
                .data_view_readers
                .borrow()
                .get(&s.realm_id())
                .cloned()
                .ok_or_else(|| pyro_js::JsError::Type("BufferSource bindings are not installed".into()))?;
            let view = reader.get(s).as_object().expect("DataView reader").call(None, &[v])?;
            if view.is_null() {
                return Ok(None);
            }
            buffer_source_bytes(s, view)
        }
        _ => {
            let mut exc = std::ptr::null();
            // SAFETY: obj is a typed array; JSObjectGetTypedArrayBytesPtr
            // returns the backing store start; add the view byteOffset.
            // Copied immediately after obtaining the pointer.
            unsafe {
                let len = sys::JSObjectGetTypedArrayByteLength(ctx, obj.as_raw(), &mut exc);
                check_exc(s, exc)?;
                let offset = sys::JSObjectGetTypedArrayByteOffset(ctx, obj.as_raw(), &mut exc);
                check_exc(s, exc)?;
                let ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj.as_raw(), &mut exc) as *const u8;
                check_exc(s, exc)?;
                if ptr.is_null() || len == 0 {
                    return Ok(Some(Vec::new()));
                }
                Ok(Some(std::slice::from_raw_parts(ptr.add(offset), len).to_vec()))
            }
        }
    }
}

/// Keeps the realm's DataView reader (install.js `dataView`, made before
/// page scripts can replace the builtins it captures): it accepts only a
/// native DataView and produces a genuine Uint8Array over the same view.
pub(crate) fn install(s: &Scope<'_>, reader: pyro_js::Value<'_>) -> JsResult<()> {
    crate::target::page_of(s)?.data_view_readers.borrow_mut().insert(s.realm_id(), pyro_js::Persistent::new(s, reader));
    Ok(())
}

/// New `Uint8Array` holding a copy of `bytes`.
pub fn uint8_array<'s>(s: &Scope<'s>, bytes: &[u8]) -> JsResult<Value<'s>> {
    let ctx = s.ctx();
    let mut exc = std::ptr::null();
    // SAFETY: JSObjectMakeTypedArray allocates a zeroed array of `len` bytes
    // owned by JSC; we copy into it before returning control to JS.
    unsafe {
        let obj = sys::JSObjectMakeTypedArray(ctx, sys::kJSTypedArrayTypeUint8Array, bytes.len(), &mut exc);
        check_exc(s, exc)?;
        if !bytes.is_empty() {
            let mut exc = std::ptr::null();
            let ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj, &mut exc) as *mut u8;
            check_exc(s, exc)?;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            }
        }
        Ok(Value::from_raw(ctx, obj as sys::JSValueRef))
    }
}

/// New `ArrayBuffer` holding a copy of `bytes`.
pub fn array_buffer<'s>(s: &Scope<'s>, bytes: &[u8]) -> JsResult<Value<'s>> {
    let arr = uint8_array(s, bytes)?;
    arr.as_object().expect("typed array").get_atom("buffer")
}

pub struct TextEncoder;
impl JsClass for TextEncoder {
    const NAME: &'static str = "TextEncoder";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| s.wrap_new::<TextEncoder>((), nt, None));
        def.getter("encoding", |s, _d| Ok(s.string("utf-8")));
        def.pure_method("encode", 0, |s, _d, a| {
            let text = if a.get(0).is_undefined() { String::new() } else { dom_string(a.get(0))? };
            uint8_array(s, text.as_bytes())
        });
        def.method("encodeInto", 2, |s, _d, a| {
            let text = dom_string(a.get(0))?;
            let dest =
                a.get(1).as_object().ok_or_else(|| JsError::Type("parameter 2 is not of type 'Uint8Array'.".into()))?;
            let cap = dest.get_atom("length")?.to_number()? as usize;
            // Largest prefix of whole code points that fits.
            let mut end = 0;
            let mut read16 = 0;
            for (i, c) in text.char_indices() {
                if i + c.len_utf8() > cap {
                    break;
                }
                end = i + c.len_utf8();
                read16 += c.len_utf16();
            }
            for (i, b) in text.as_bytes()[..end].iter().enumerate() {
                dest.set_index(i as u32, s.number(*b as f64))?;
            }
            let r = s.object();
            r.set_atom("read", s.number(read16 as f64))?;
            r.set_atom("written", s.number(end as f64))?;
            Ok(r.value())
        });
    }
}

pub struct DecoderData {
    encoding: &'static encoding_rs::Encoding,
    fatal: bool,
    ignore_bom: bool,
    /// Held between `decode(..., {stream: true})` calls; None starts afresh.
    decoder: std::cell::RefCell<Option<encoding_rs::Decoder>>,
}

pub struct TextDecoder;
impl JsClass for TextDecoder {
    const NAME: &'static str = "TextDecoder";
    type Data = DecoderData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let label = if a.get(0).is_undefined() { "utf-8".to_owned() } else { dom_string(a.get(0))? };
            let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
                .ok_or_else(|| JsError::Range(format!("The encoding label provided ('{label}') is invalid.").into()))?;
            let (fatal, ignore_bom) = match a.get(1).as_object() {
                Some(o) => (o.get_atom("fatal")?.to_bool(), o.get_atom("ignoreBOM")?.to_bool()),
                None => (false, false),
            };
            s.wrap_new::<TextDecoder>(
                DecoderData { encoding, fatal, ignore_bom, decoder: Default::default() },
                nt,
                None,
            )
        });
        def.getter("encoding", |s, d| Ok(s.string(&d.encoding.name().to_ascii_lowercase())));
        def.getter("fatal", |s, d| Ok(s.boolean(d.fatal)));
        def.getter("ignoreBOM", |s, d| Ok(s.boolean(d.ignore_bom)));
        def.method("decode", 0, |s, d, a| {
            let bytes = if a.get(0).is_undefined() {
                Vec::new()
            } else {
                buffer_source_bytes(s, a.get(0))?.ok_or_else(|| {
                    JsError::Type("The provided value is not of type '(ArrayBuffer or ArrayBufferView)'.".into())
                })?
            };
            let last = match a.get(1).as_object() {
                Some(o) => !o.get_atom("stream")?.to_bool(),
                None => true,
            };
            let mut slot = d.decoder.borrow_mut();
            let decoder = slot.get_or_insert_with(|| {
                if d.ignore_bom {
                    d.encoding.new_decoder_without_bom_handling()
                } else {
                    // Only the encoding's own BOM is dropped; it never switches encoding.
                    d.encoding.new_decoder_with_bom_removal()
                }
            });
            let capacity = decoder.max_utf8_buffer_length(bytes.len()).unwrap_or(bytes.len());
            let mut text = String::with_capacity(capacity);
            let malformed = if d.fatal {
                let (result, _) = decoder.decode_to_string_without_replacement(&bytes, &mut text, last);
                matches!(result, encoding_rs::DecoderResult::Malformed(..))
            } else {
                let _ = decoder.decode_to_string(&bytes, &mut text, last);
                false
            };
            if last || malformed {
                *slot = None;
            }
            if malformed {
                return Err(JsError::Type("The encoded data was not valid.".into()));
            }
            Ok(s.string(&text))
        });
    }
}

/// `btoa(data)`: Latin-1 string -> base64.
pub fn btoa(input: &str) -> JsResult<String> {
    let mut bytes = Vec::with_capacity(input.len());
    for c in input.chars() {
        let cp = c as u32;
        if cp > 0xFF {
            return Err(JsError::dom(
                "InvalidCharacterError",
                "The string to be encoded contains characters outside of the Latin1 range.",
            ));
        }
        bytes.push(cp as u8);
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `atob(data)`: forgiving-base64 decode -> Latin-1 string.
pub fn atob(input: &str) -> JsResult<String> {
    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut cleaned = cleaned;
    if cleaned.len().is_multiple_of(4) {
        while cleaned.last() == Some(&b'=') && cleaned.len() % 4 != 1 {
            cleaned.pop();
            if cleaned.len() % 4 == 2 {
                break;
            }
        }
        while cleaned.last() == Some(&b'=') {
            cleaned.pop();
        }
    }
    if cleaned.len() % 4 == 1 || cleaned.iter().any(|b| !(b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/')) {
        return Err(JsError::dom("InvalidCharacterError", "The string to be decoded is not correctly encoded."));
    }
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(&cleaned)
        .map_err(|_| JsError::dom("InvalidCharacterError", "The string to be decoded is not correctly encoded."))?;
    Ok(bytes.into_iter().map(|b| b as char).collect())
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<TextEncoder>();
    engine.register::<TextDecoder>();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_roundtrip() {
        assert_eq!(btoa("hello").unwrap(), "aGVsbG8=");
        assert_eq!(atob("aGVsbG8=").unwrap(), "hello");
        assert_eq!(atob(" aGVs bG8 ").unwrap(), "hello");
        assert!(atob("a").is_err());
        assert!(btoa("ž").is_err());
        assert_eq!(atob(&btoa("\u{ff}\u{0}").unwrap()).unwrap(), "\u{ff}\u{0}");
    }
}
