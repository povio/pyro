//! Bounded in-realm value cloning. JS owns temporary graph roots; native checks
//! reject proxies and host wrappers without consulting user properties.
use lp_js::{JsError, JsResult, Persistent, Scope};

pub(crate) fn install(s: &Scope<'_>, page: &crate::page::PageDom) -> JsResult<()> {
    // One native function serves the JS clone: `guard(value)` says whether
    // `value` is an ordinary object (no proxy, no host wrapper); with a mode
    // it is structured_clone.js's `host` (serializable/transferable hosts).
    let guard = s.function("cloneObjectGuard", |s, _, args| {
        let value = args.get(0);
        if args.len() > 1 {
            return match args.get(1).to_number()? as u32 {
                0 => Ok(crate::file::clone_host(s, value)?.unwrap_or_else(|| s.undefined())),
                1 => Ok(crate::message_channel::prepare_transfer(s, value)?.unwrap_or_else(|| s.undefined())),
                // A platform object the script does not know (CryptoKey), or
                // undefined; asked only for a value it would otherwise reject.
                3 => crate::crypto::clone_platform_object(s, value),
                _ => crate::message_channel::complete_transfer(s, value, args.get(2)).map(|()| s.undefined()),
            };
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
    let dom_exception = s.dom_exception().ok_or(JsError::ContextDestroyed)?;
    let factory =
        s.eval(include_str!("structured_clone.js"), Some("lightningpanda://internal/structured-clone.js"), 1)?;
    let pair = factory
        .as_object()
        .expect("clone factory")
        .call(None, &[guard_root.get(s), fail_root.get(s), dom_exception.value()])?;
    let pair = pair.as_object().expect("clone pair");
    let (public, internal) = (pair.get_index(0)?, pair.get_index(1)?);
    // Native users (MessagePort, postMessage) keep the original, unaffected
    // by page reassignment of `structuredClone`; released with the realm/page.
    page.structured_clones.borrow_mut().insert(s.realm_id(), Persistent::new(s, internal));
    s.global().set_atom("structuredClone", public)
}

fn realm_clone<'s>(s: &Scope<'s>) -> JsResult<lp_js::Object<'s>> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    clone
        .and_then(|v| v.as_object())
        .ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable in this realm"))
}

/// StructuredSerializeWithTransfer + deserialize into `s`'s realm, with
/// `options` the `structuredClone` options argument (`{transfer}`). `value`
/// may come from another realm of the VM (a frame or popup posting here).
pub(crate) fn clone_value<'s>(
    s: &Scope<'s>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<lp_js::Value<'s>> {
    realm_clone(s)?.call(None, &[value, options])
}

/// [`clone_value`] that also returns the new ports of the transferred
/// MessagePorts, in transfer-list order (a `MessageEvent`'s `ports`).
pub(crate) fn clone_with_ports<'s>(
    s: &Scope<'s>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<(lp_js::Value<'s>, lp_js::Object<'s>)> {
    let ports = s.array(&[])?;
    let value = realm_clone(s)?.call(None, &[value, options, ports.value()])?;
    Ok((value, ports))
}

/// [`clone_with_ports`] of a message posted from `s`'s realm into `rs`'s
/// (a window or MessagePort of another realm). Whatever the clone throws at
/// the poster (a getter's exception, a transfer-list TypeError, a
/// DataCloneError) must belong to the poster's realm, never the receiver's:
/// a cross-origin receiver's error objects would hand the poster its
/// `Function`. So across realms the message is first cloned in `s` (user
/// code and errors stay there), and that copy, which holds only clonable
/// values, is cloned into `rs`, transferring the ports the first pass made;
/// an error of that second pass is reported as a realm-less DataCloneError.
/// Within one realm this is a single [`clone_with_ports`].
pub(crate) fn clone_into<'s, 'r>(
    s: &Scope<'s>,
    rs: &Scope<'r>,
    value: lp_js::Value<'s>,
    options: lp_js::Value<'s>,
) -> JsResult<(lp_js::Value<'r>, lp_js::Object<'r>)> {
    if s.realm_id() == rs.realm_id() {
        return clone_with_ports(rs, rs.rehome(value), rs.rehome(options));
    }
    let second = |value: lp_js::Value<'r>, options: lp_js::Value<'r>| {
        clone_with_ports(rs, value, options).map_err(|error| match error {
            JsError::Terminated | JsError::ContextDestroyed => error,
            JsError::Dom { name, message } if name == "DataCloneError" => JsError::Dom { name, message },
            _ => JsError::dom("DataCloneError", "The message could not be cloned into the receiving realm."),
        })
    };
    if realm_clone(s).is_err() {
        // No clone function in the poster's realm (an internal one): clone
        // straight into the receiver, still without leaking its errors.
        return second(rs.rehome(value), rs.rehome(options));
    }
    let (copy, ports) = clone_with_ports(s, value, options)?;
    let options = if ports.get_atom("length")?.to_number()? > 0.0 {
        let o = rs.object();
        o.set_atom("transfer", rs.rehome(ports.value()))?;
        o.value()
    } else {
        rs.undefined()
    };
    second(rs.rehome(copy), options)
}

/// `postMessage(message, transfer)` / `postMessage(message, options)` as the
/// `structuredClone` options argument: a sequence is the transfer list.
pub(crate) fn transfer_options<'s>(s: &Scope<'s>, arg: Option<lp_js::Value<'s>>) -> JsResult<lp_js::Value<'s>> {
    Ok(match arg {
        Some(v) if v.is_array() => {
            let o = s.object();
            o.set_atom("transfer", v)?;
            o.value()
        }
        Some(v) => v,
        None => s.undefined(),
    })
}

/// A serialized message, independent of any realm (`worker_wire.js`): on
/// its way to another VM, or kept by session history.
pub struct WireMessage {
    text: String,
    buffers: Vec<Vec<u8>>,
    /// Bytes of the Blobs and Files it holds (shared, immutable).
    blobs: Vec<bytes::Bytes>,
}

impl WireMessage {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        WireMessage { text: String::new(), buffers: Vec::new(), blobs: Vec::new() }
    }
    /// What the message holds in memory while queued.
    pub(crate) fn size(&self) -> usize {
        self.text.len()
            + self.buffers.iter().map(Vec::len).sum::<usize>()
            + self.blobs.iter().map(bytes::Bytes::len).sum::<usize>()
            + 64
    }
}

thread_local! {
    /// Blob bytes of the message being encoded or decoded on this thread
    /// (saved and restored around each, so nested messages keep their own).
    static WIRE_BLOBS: std::cell::RefCell<Vec<bytes::Bytes>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `host` of worker_wire.js.
fn wire_host<'s>(s: &Scope<'s>, args: &lp_js::Args<'s>) -> JsResult<lp_js::Value<'s>> {
    let value = args.get(0);
    match args.get(1).to_number()? as u32 {
        0 => {
            if let Some((bytes, mime, file)) = crate::file::blob_parts_of(s, value) {
                let index = WIRE_BLOBS.with(|b| {
                    let mut b = b.borrow_mut();
                    b.push(bytes);
                    b.len() - 1
                });
                let (name, modified) = match file {
                    Some((name, modified)) => (s.string(&name), s.number(modified)),
                    None => (s.null(), s.number(0.0)),
                };
                let parts = [s.string("F"), s.number(index as f64), s.string(&mime), name, modified];
                return Ok(s.array(&parts)?.value());
            }
            if crate::file::is_file_list(s, value) {
                let copy = crate::file::clone_host(s, value)?.and_then(|v| v.as_object());
                let files = s.array(&[s.string("L")])?;
                if let Some(copy) = copy {
                    let count = crate::file::file_list_len(s, copy.value());
                    for i in 0..count {
                        files.set_index(i as u32 + 1, copy.get_index(i as u32)?)?;
                    }
                }
                return Ok(files.value());
            }
            Ok(s.undefined())
        }
        1 => {
            let tree = value.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            let index = tree.get_index(1)?.to_number()? as usize;
            let bytes = WIRE_BLOBS.with(|b| b.borrow().get(index).cloned());
            let bytes = bytes.ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            let mime = tree.get_index(2)?.to_string()?;
            let name = tree.get_index(3)?;
            let file = if name.is_null() { None } else { Some((name.to_string()?, tree.get_index(4)?.to_number()?)) };
            crate::file::make_blob(s, bytes, mime, file)
        }
        _ => {
            let files = value.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            crate::file::make_file_list(s, files)
        }
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
            let host = s.function("wireHost", |s, _, args| wire_host(s, &args))?;
            let host_root = Persistent::new(s, host.value());
            let dom_exception = s.dom_exception().ok_or(JsError::ContextDestroyed)?;
            let factory =
                s.eval(include_str!("worker_wire.js"), Some("lightningpanda://internal/worker-wire.js"), 1)?;
            let pair = factory
                .as_object()
                .expect("wire factory")
                .call(None, &[fail_root.get(s), host_root.get(s), dom_exception.value()])?;
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
pub fn encode_message<'s>(s: &Scope<'s>, value: lp_js::Value<'s>, options: lp_js::Value<'s>) -> JsResult<WireMessage> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let clone = clone.ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable"))?;
    let (encode, _) = wire_functions(s)?;
    let saved = WIRE_BLOBS.with(|b| b.take());
    let pair = encode.call(None, &[clone, value, options]);
    let blobs = WIRE_BLOBS.with(|b| b.replace(saved));
    let pair = pair?;
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
    Ok(WireMessage { text, buffers, blobs })
}

/// StructuredDeserialize into `s`'s realm.
pub fn decode_message<'s>(s: &Scope<'s>, message: &WireMessage) -> JsResult<lp_js::Value<'s>> {
    let (_, decode) = wire_functions(s)?;
    let list = s.array(&[])?;
    for (i, bytes) in message.buffers.iter().enumerate() {
        list.set_index(i as u32, crate::encoding::array_buffer(s, bytes)?)?;
    }
    let saved = WIRE_BLOBS.with(|b| b.replace(message.blobs.clone()));
    let result = decode.call(None, &[s.string(&message.text), list.value()]);
    WIRE_BLOBS.with(|b| *b.borrow_mut() = saved);
    result
}
