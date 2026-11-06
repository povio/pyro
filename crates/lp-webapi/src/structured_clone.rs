//! Bounded in-realm value cloning. JS owns temporary graph roots; native checks
//! reject proxies and host wrappers without consulting user properties.
use lp_js::{JsError, JsResult, Persistent, Scope};

pub(crate) fn install(s: &Scope<'_>, page: &crate::page::PageDom) -> JsResult<()> {
    let guard = s.function("cloneObjectGuard", |s, _, args| {
        let value = args.get(0);
        // `guard(value, true)`: the copy of a platform object the script does
        // not know (CryptoKey), or undefined; asked only for a value the
        // script would otherwise reject.
        if args.len() > 1 {
            return crate::crypto::clone_platform_object(s, value);
        }
        let check: JsResult<bool> = (|| Ok(s.is_host_object(value)? || value.has_proxy_target()?))();
        match check {
            Ok(reject) => Ok(s.boolean(!reject)),
            Err(JsError::Unsupported(_)) => {
                Err(JsError::dom("NotSupportedError", "Native clone classification is unavailable"))
            }
            Err(error) => Err(error),
        }
    })?;
    let guard_root = Persistent::new(s, guard.value());
    let fail = s.function("cloneFailure", |_, _, args| {
        let name = if args.get(1).to_bool() { "NotSupportedError" } else { "DataCloneError" };
        Err(JsError::dom(name, args.get(0).to_string()?))
    })?;
    let fail_root = Persistent::new(s, fail.value());
    let factory =
        s.eval(include_str!("structured_clone.js"), Some("lightningpanda://internal/structured-clone.js"), 1)?;
    let clone = factory.as_object().expect("clone factory").call(None, &[guard_root.get(s), fail_root.get(s)])?;
    // Native users (MessagePort) keep the original, unaffected by page
    // reassignment of `structuredClone`; released with the realm/page.
    page.structured_clones.borrow_mut().insert(s.realm_id(), Persistent::new(s, clone));
    s.global().set_atom("structuredClone", clone)
}

/// StructuredSerialize + deserialize into the same realm, with `options`
/// passed as the `structuredClone` options argument. Transfer lists are
/// rejected with NotSupportedError by the clone itself.
pub(crate) fn clone_value<'s>(
    s: &Scope<'s>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<lp_js::Value<'s>> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let clone = clone
        .and_then(|v| v.as_object())
        .ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable in this realm"))?;
    clone.call(None, &[value, options])
}

/// A message on its way to another VM (`worker_wire.js`).
pub(crate) struct WireMessage {
    text: String,
    buffers: Vec<Vec<u8>>,
}

impl WireMessage {
    /// What the message holds in memory while queued.
    pub(crate) fn size(&self) -> usize {
        self.text.len() + self.buffers.iter().map(Vec::len).sum::<usize>() + 64
    }
}

/// The realm's `[encode, decode]`, compiled on its first use.
fn wire_functions<'s>(s: &Scope<'s>) -> JsResult<(lp_js::Object<'s>, lp_js::Object<'s>)> {
    let page = crate::target::page_of(s)?;
    let cached = page.worker_wire.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let pair = match cached {
        Some(pair) => pair,
        None => {
            let fail = s.function("wireFailure", |_, _, args| {
                let name = if args.get(1).to_bool() { "NotSupportedError" } else { "DataCloneError" };
                Err(JsError::dom(name, args.get(0).to_string()?))
            })?;
            let fail_root = Persistent::new(s, fail.value());
            let factory =
                s.eval(include_str!("worker_wire.js"), Some("lightningpanda://internal/worker-wire.js"), 1)?;
            let pair = factory.as_object().expect("wire factory").call(None, &[fail_root.get(s)])?;
            page.worker_wire.borrow_mut().insert(s.realm_id(), Persistent::new(s, pair));
            pair
        }
    };
    let pair = pair.as_object().expect("wire pair");
    Ok((pair.get_index(0)?.as_object().expect("encode"), pair.get_index(1)?.as_object().expect("decode")))
}

/// Compiles the realm's message functions now, if it has none yet.
pub(crate) fn prepare_wire(s: &Scope<'_>) -> JsResult<()> {
    wire_functions(s).map(|_| ())
}

/// StructuredSerialize for another VM. `options` is `postMessage`'s second
/// argument (a transfer list or `{transfer}`); listed ArrayBuffers are detached.
pub(crate) fn encode_message<'s>(
    s: &Scope<'s>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<WireMessage> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let clone = clone.ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable"))?;
    let (encode, _) = wire_functions(s)?;
    let pair = encode.call(None, &[clone, value, options])?;
    let pair = pair.as_object().expect("encoded pair");
    let text = pair.get_index(0)?.to_string()?;
    let list = pair.get_index(1)?;
    let list = list.as_object().expect("buffer list");
    let count = list.get_atom("length")?.to_number()? as u32;
    let mut buffers = Vec::with_capacity(count as usize);
    for i in 0..count {
        let bytes = crate::encoding::buffer_source_bytes(s, list.get_index(i)?)?;
        buffers.push(bytes.ok_or_else(|| JsError::dom("DataCloneError", "Detached ArrayBuffer"))?);
    }
    Ok(WireMessage { text, buffers })
}

/// StructuredDeserialize into `s`'s realm.
pub(crate) fn decode_message<'s>(s: &Scope<'s>, message: &WireMessage) -> JsResult<lp_js::Value<'s>> {
    let (_, decode) = wire_functions(s)?;
    let list = s.array(&[])?;
    for (i, bytes) in message.buffers.iter().enumerate() {
        list.set_index(i as u32, crate::encoding::array_buffer(s, bytes)?)?;
    }
    decode.call(None, &[s.string(&message.text), list.value()])
}
