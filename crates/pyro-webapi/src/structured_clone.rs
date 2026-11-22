//! Bounded in-realm value cloning. JS owns temporary graph roots; native checks
//! reject proxies and host wrappers without consulting user properties.
use pyro_js::{JsError, JsResult, Persistent, Scope};

/// `factory` is structured_clone.js (install.js `structuredClone`).
pub(crate) fn install(s: &Scope<'_>, page: &crate::page::PageDom, factory: pyro_js::Value<'_>) -> JsResult<()> {
    // One native function serves the JS clone: `guard(value)` says whether
    // `value` is an ordinary object (no proxy, no host wrapper); with a mode
    // it is structured_clone.js's `host` (serializable/transferable hosts).
    let guard = s.function("cloneObjectGuard", |s, _, args| {
        let value = args.get(0);
        if args.len() > 1 {
            return match args.get(1).to_number()? as u32 {
                0 => {
                    if let Some(resource) = CodecResource::parts(s, value)? {
                        if args.get(2).to_bool() {
                            return Err(JsError::dom("DataCloneError", "WebCodecs resources cannot be serialized for storage."));
                        }
                        if let Some(marker) = args.get(3).as_object() {
                            marker.set_index(0, s.boolean(true))?;
                        }
                        return resource.wrap(s);
                    }
                    Ok(crate::file::clone_host(s, value)?.unwrap_or_else(|| s.undefined()))
                }
                1 => Ok(crate::message_channel::prepare_transfer(s, value)?.unwrap_or_else(|| s.undefined())),
                // A platform object the script does not know (CryptoKey,
                // ImageBitmap), or undefined; asked only for a value it would
                // otherwise reject.
                3 => {
                    let copy = crate::crypto::clone_platform_object(s, value)?;
                    if !copy.is_undefined() {
                        return Ok(copy);
                    }
                    match canvas_object(s, value, false)? {
                        Some(object) => crate::canvas::make_canvas_object(s, &object),
                        None => Ok(s.undefined()),
                    }
                }
                // A transferable ImageBitmap or OffscreenCanvas: its copy
                // here (the original is detached by mode 9), else undefined.
                10 => geometry_parts(s, value),
                11 => crate::canvas::MODULE.helper(s, &crate::target::page_of(s)?, "geometryFromParts", &[value, args.get(2)]),
                8 => match canvas_object(s, value, true)? {
                    Some(object) => crate::canvas::make_canvas_object(s, &object),
                    None => Ok(s.undefined()),
                },
                9 => detach_canvas_object(s, value).map(|()| s.undefined()),
                // Snapshot the AudioData transfer now, but only close the
                // sender once graph traversal and validation have succeeded.
                12 => match crate::audio_data::parts(s, value)? {
                    Some(frame) => {
                        if let Some(marker) = args.get(2).as_object() {
                            marker.set_index(0, s.boolean(true))?;
                        }
                        crate::audio_data::wrap(s, &frame)
                    }
                    None => Ok(s.undefined()),
                },
                13 => Ok(s.boolean(crate::audio_data::parts(s, value)?.is_some())),
                14 => {
                    crate::audio_data::detach(s, value);
                    Ok(s.undefined())
                }
                5 => prepare_stream_transfer(s, value),
                6 => complete_stream_transfer(s, value, args.get(2)).map(|()| s.undefined()),
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

pub(crate) fn realm_clone<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
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
    value: pyro_js::Value<'s>,
    options: pyro_js::Value<'s>,
) -> JsResult<pyro_js::Value<'s>> {
    realm_clone(s)?.call(None, &[value, options])
}

/// Clone a value for persistent storage, observing member access in graph order.
pub(crate) fn clone_for_storage<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>) -> JsResult<pyro_js::Value<'s>> {
    let unused = s.undefined();
    realm_clone(s)?.call(None, &[value, unused, unused, unused, unused, unused, s.boolean(true)])
}

/// [`clone_value`] that also returns the new ports of the transferred
/// MessagePorts, in transfer-list order (a `MessageEvent`'s `ports`).
pub(crate) fn clone_with_ports<'s>(
    s: &Scope<'s>,
    value: pyro_js::Value<'s>,
    options: pyro_js::Value<'s>,
) -> JsResult<(pyro_js::Value<'s>, pyro_js::Object<'s>)> {
    let ports = s.array(&[])?;
    let value = realm_clone(s)?.call(None, &[value, options, ports.value()])?;
    Ok((value, ports))
}

thread_local! {
    /// The poster's realm while [`clone_into`] clones its first copy into
    /// the receiver: the streams that copy holds are the poster realm's, so
    /// that realm's streams module transfers them.
    static CLONE_SOURCE: std::cell::RefCell<Option<pyro_js::WeakRealm>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with the scope of the realm whose streams module knows `value`
/// (`s`'s, or the poster's during [`clone_into`]'s second pass) and that
/// module's transfer kind, or returns `None`.
fn with_stream_realm<'s, T>(
    s: &Scope<'s>,
    value: pyro_js::Value<'s>,
    f: impl for<'x> FnOnce(&Scope<'x>, pyro_js::Value<'x>, String) -> JsResult<T>,
) -> JsResult<Option<T>> {
    if let Some(kind) = crate::streams::transfer_kind(s, value)? {
        let kind = kind.to_string()?;
        return f(s, value, kind).map(Some);
    }
    let source = CLONE_SOURCE.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()));
    let Some(source) = source.filter(|r| r.id() != s.realm_id()) else { return Ok(None) };
    let ss = source.scope()?;
    let value = ss.rehome(value);
    match crate::streams::transfer_kind(&ss, value)? {
        Some(kind) => {
            let kind = kind.to_string()?;
            f(&ss, value, kind).map(Some)
        }
        None => Ok(None),
    }
}

/// Read geometry slots in the source realm and construct copies in the receiving realm.
fn geometry_parts<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>) -> JsResult<pyro_js::Value<'s>> {
    let Some(realm) = value.as_object().and_then(|object| s.object_realm(object)) else { return Ok(s.undefined()) };
    let scope = realm.scope()?;
    let page = crate::target::page_of(&scope)?;
    let Some(module) = crate::canvas::MODULE.existing(&scope, &page)? else { return Ok(s.undefined()) };
    let describe = module.get_atom("geometryParts")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let result = describe.call(None, &[scope.rehome(value)])?;
    Ok(Persistent::new(&scope, result).get(s))
}

/// The ImageBitmap or OffscreenCanvas `value` as a [`crate::canvas::CanvasObject`]:
/// of `s`'s realm, or of the poster's during [`clone_into`]'s second pass.
fn canvas_object(
    s: &Scope<'_>,
    value: pyro_js::Value<'_>,
    transfer: bool,
) -> JsResult<Option<crate::canvas::CanvasObject>> {
    if let Some(object) = crate::canvas::canvas_object_of(s, value, transfer)? {
        return Ok(Some(object));
    }
    let source = CLONE_SOURCE.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()));
    let Some(source) = source.filter(|r| r.id() != s.realm_id()) else { return Ok(None) };
    let ss = source.scope()?;
    crate::canvas::canvas_object_of(&ss, ss.rehome(value), transfer)
}

/// Clone mode 9: detaches a transferred ImageBitmap or OffscreenCanvas in
/// its realm.
fn detach_canvas_object(s: &Scope<'_>, value: pyro_js::Value<'_>) -> JsResult<()> {
    if crate::canvas::detach_canvas_object(s, value)? {
        return Ok(());
    }
    let source = CLONE_SOURCE.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()));
    if let Some(source) = source.filter(|r| r.id() != s.realm_id()) {
        let ss = source.scope()?;
        crate::canvas::detach_canvas_object(&ss, ss.rehome(value))?;
    }
    Ok(())
}

/// Clone mode 5: for a transferable stream, `[new stream, ...its ports]` in
/// `s`'s realm, the ports not yet entangled; undefined for anything else.
fn prepare_stream_transfer<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>) -> JsResult<pyro_js::Value<'s>> {
    let Some(kind) = with_stream_realm(s, value, |_, _, kind| Ok(kind))? else { return Ok(s.undefined()) };
    let info = s.array(&[])?;
    let ports = s.array(&[])?;
    for i in 0..if kind == "T" { 2 } else { 1 } {
        let port = crate::message_channel::fresh_port(s)?;
        ports.set_index(i, port)?;
        info.set_index(i + 1, port)?;
    }
    info.set_index(0, crate::streams::transfer_in(s, s.string(&kind), ports.value())?)?;
    Ok(info.value())
}

/// Clone mode 6: pipes `value` through new channels in its realm, whose
/// other ports move into the ports `info` (from mode 5) holds.
fn complete_stream_transfer<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>, info: pyro_js::Value<'s>) -> JsResult<()> {
    let info = info.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Invalid stream transfer"))?;
    let done = with_stream_realm(s, value, |ss, value, _| {
        let out = crate::streams::transfer_out(ss, value)?;
        let count = out.get_atom("length")?.to_number()? as u32;
        for i in 0..count {
            let from = s.rehome(out.get_index(i)?);
            crate::message_channel::complete_transfer(s, from, info.get_index(i + 1)?)?;
        }
        Ok(())
    })?;
    done.ok_or_else(|| JsError::dom("DataCloneError", "Value is not transferable"))
}

/// Local messages normally keep the receiver-realm clone. A graph containing
/// cluster-bound data stays serialized until delivery, since a MessagePort
/// (and its queue) can move to a different realm before it is started.
pub(crate) enum MessagePayload {
    Local(Persistent, Option<Persistent>),
    Serialized(WireMessage),
}

impl MessagePayload {
    pub(crate) fn read(self, receiver: &Scope<'_>) -> JsResult<(Persistent, Option<Persistent>)> {
        match self {
            Self::Local(value, ports) => Ok((value, ports)),
            Self::Serialized(message) => {
                let (value, ports) = decode_with_ports(receiver, &message)?;
                Ok((Persistent::new(receiver, value), ports.map(|p| Persistent::new(receiver, p.value()))))
            }
        }
    }
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
    value: pyro_js::Value<'s>,
    options: pyro_js::Value<'s>,
) -> JsResult<MessagePayload> {
    let local = |value, ports: pyro_js::Object<'r>| {
        let has_ports = ports.get_atom("length")?.to_number()? > 0.;
        Ok(MessagePayload::Local(Persistent::new(rs, value), has_ports.then(|| Persistent::new(rs, ports.value()))))
    };
    let second = |value: pyro_js::Value<'r>, options: pyro_js::Value<'r>| {
        clone_with_ports(rs, value, options).map_err(|error| match error {
            JsError::Terminated | JsError::ContextDestroyed => error,
            JsError::Dom(e) if e.name == "DataCloneError" => JsError::Dom(e),
            _ => JsError::dom("DataCloneError", "The message could not be cloned into the receiving realm."),
        })
    };
    let from_source = |value: pyro_js::Value<'r>, options: pyro_js::Value<'r>| {
        let saved = CLONE_SOURCE.with(|c| c.replace(Some(s.realm().downgrade())));
        let result = second(value, options);
        CLONE_SOURCE.with(|c| *c.borrow_mut() = saved);
        result
    };
    let Ok(clone) = realm_clone(s) else {
        // No clone function in the poster's realm (an internal one): clone
        // straight into the receiver, still without leaking its errors.
        let (copy, ports) = from_source(rs.rehome(value), rs.rehome(options))?;
        return local(copy, ports);
    };
    let (ports, streams, resources) = (s.array(&[])?, s.array(&[])?, s.array(&[])?);
    let cluster_bound = s.array(&[s.boolean(false)])?;
    let copy = clone.call(None, &[
        value, options, ports.value(), s.undefined(), streams.value(), resources.value(), s.boolean(false), cluster_bound.value(),
    ])?;
    let must_serialize = cluster_bound.get_index(0)?.to_bool();
    if !must_serialize && s.realm_id() == rs.realm_id() {
        return local(rs.rehome(copy), rs.rehome_object(ports));
    }
    // The second pass transfers what the first one made: ports, streams,
    // then transferred canvas objects and AudioData frames.
    let port_count = ports.get_atom("length")?.to_number()? as u32;
    let stream_count = streams.get_atom("length")?.to_number()? as u32;
    let resource_count = resources.get_atom("length")?.to_number()? as u32;
    let options = if port_count + stream_count + resource_count > 0 {
        for i in 0..stream_count {
            ports.set_index(port_count + i, streams.get_index(i)?)?;
        }
        for i in 0..resource_count {
            ports.set_index(port_count + stream_count + i, resources.get_index(i)?)?;
        }
        let o = s.object();
        o.set_atom("transfer", ports.value())?;
        o.value()
    } else {
        s.undefined()
    };
    if must_serialize {
        return encode_message(s, copy, options).map(MessagePayload::Serialized);
    }
    let (copy, ports) = from_source(rs.rehome(copy), rs.rehome(options))?;
    local(copy, ports)
}

/// `postMessage(message, transfer)` / `postMessage(message, options)` as the
/// `structuredClone` options argument: a sequence is the transfer list.
pub(crate) fn transfer_options<'s>(s: &Scope<'s>, arg: Option<pyro_js::Value<'s>>) -> JsResult<pyro_js::Value<'s>> {
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

/// WebCodecs serialization retains the media resource and its source agent
/// cluster. Neither the JavaScript wrapper nor its detached state is shared.
#[derive(Clone)]
enum CodecResource {
    Chunk(crate::encoded_chunks::EncodedChunk),
    Audio(crate::audio_data::AudioResource),
}

impl CodecResource {
    fn parts(s: &Scope<'_>, value: pyro_js::Value<'_>) -> JsResult<Option<Self>> {
        if let Some(chunk) = crate::encoded_chunks::parts(s, value) {
            return Ok(Some(Self::Chunk(chunk)));
        }
        Ok(crate::audio_data::parts(s, value)?.map(Self::Audio))
    }

    fn wrap<'s>(&self, s: &Scope<'s>) -> JsResult<pyro_js::Value<'s>> {
        match self {
            Self::Chunk(chunk) => crate::encoded_chunks::wrap(s, chunk),
            Self::Audio(frame) => crate::audio_data::wrap(s, frame),
        }
    }

    fn readable_in(&self, page: &crate::PageDom) -> bool {
        match self {
            Self::Chunk(chunk) => chunk.readable_in(page),
            Self::Audio(frame) => frame.readable_in(page),
        }
    }

    fn byte_length(&self) -> usize {
        match self {
            Self::Chunk(chunk) => chunk.byte_length(),
            Self::Audio(frame) => frame.byte_length(),
        }
    }
}

/// A serialized message, independent of any realm (`worker_wire.js`): on
/// its way to another VM, or kept by session history.
pub struct WireMessage {
    text: String,
    buffers: Vec<Vec<u8>>,
    /// Bytes of the Blobs and Files it holds (shared, immutable).
    blobs: Vec<bytes::Bytes>,
    /// The MessagePorts it transfers, in transfer-list order: each is
    /// claimed by the deserialization that makes its new port, and closed
    /// if the message is dropped first.
    ports: Vec<crate::port_link::TransitEnd>,
    /// How many of `ports` the sender listed (the event's `ports`); the
    /// rest carry the transferred streams.
    listed_ports: usize,
    /// The ImageBitmaps (pixels shared, not copied) and transferred
    /// OffscreenCanvases it holds.
    canvases: Vec<crate::canvas::CanvasObject>,
    /// Encoded chunks and AudioData frames share immutable byte storage.
    codecs: Vec<CodecResource>,
}

impl WireMessage {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        WireMessage {
            text: String::new(),
            buffers: Vec::new(),
            blobs: Vec::new(),
            ports: Vec::new(),
            listed_ports: 0,
            canvases: Vec::new(),
            codecs: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn carrying(port: crate::port_link::TransitEnd) -> Self {
        WireMessage { ports: vec![port], listed_ports: 1, ..Self::empty() }
    }
    /// What the message holds in memory while queued.
    pub(crate) fn size(&self) -> usize {
        self.text.len()
            + self.buffers.iter().map(Vec::len).sum::<usize>()
            + self.blobs.iter().map(bytes::Bytes::len).sum::<usize>()
            + self.codecs.iter().map(CodecResource::byte_length).sum::<usize>()
            + 64 * (1 + self.ports.len() + self.canvases.len() + self.codecs.len())
    }
    /// The pipe sides it carries.
    pub(crate) fn transit_ends(&self) -> &[crate::port_link::TransitEnd] {
        &self.ports
    }
    #[cfg(test)]
    pub(crate) fn sized(bytes: usize) -> Self {
        WireMessage { text: "x".repeat(bytes), ..Self::empty() }
    }
    /// Whether it transfers the port on the other side of `link` (posting
    /// a port's entangled peer through it loses the message, as in HTML).
    pub(crate) fn carries_peer_of(&self, link: &crate::port_link::Link) -> bool {
        self.ports.iter().any(|p| p.is_peer_of(link))
    }
}

thread_local! {
    /// Blob bytes of the message being encoded or decoded on this thread
    /// (saved and restored around each, so nested messages keep their own).
    static WIRE_BLOBS: std::cell::RefCell<Vec<bytes::Bytes>> = const { std::cell::RefCell::new(Vec::new()) };
    /// As `WIRE_BLOBS`, for ImageBitmaps and OffscreenCanvases.
    static WIRE_CANVASES: std::cell::RefCell<Vec<crate::canvas::CanvasObject>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static WIRE_CODECS: std::cell::RefCell<Vec<CodecResource>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `host` of worker_wire.js.
fn wire_host<'s>(s: &Scope<'s>, args: &pyro_js::Args<'s>) -> JsResult<pyro_js::Value<'s>> {
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
            if let Some(parts) = crate::crypto::serialize_key(s, value)? {
                return Ok(s.array(&[s.string("K"), parts])?.value());
            }
            // An ImageBitmap, or an OffscreenCanvas (the copy holds one only
            // when it is transferred).
            if let Some(object) = crate::canvas::canvas_object_of(s, value, true)? {
                let index = WIRE_CANVASES.with(|c| {
                    let mut c = c.borrow_mut();
                    c.push(object);
                    c.len() - 1
                });
                return Ok(s.array(&[s.string("C"), s.number(index as f64)])?.value());
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
        2 => {
            let files = value.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            crate::file::make_file_list(s, files)
        }
        3 => crate::crypto::deserialize_key(s, value),
        4 => Ok(s.boolean(crate::message_channel::is_transferable_port(s, value)?)),
        5 => Ok(crate::streams::transfer_kind(s, value)?.unwrap_or_else(|| s.undefined())),
        6 => Ok(crate::streams::transfer_out(s, value)?.value()),
        8 => {
            if crate::audio_data::parts(s, value)?.is_some() {
                return Ok(s.string("A"));
            }
            Ok(match crate::canvas::canvas_object_of(s, value, true)? {
                Some(_) => s.string("C"),
                None => s.undefined(),
            })
        }
        9 => {
            if !crate::audio_data::detach(s, value) {
                crate::canvas::detach_canvas_object(s, value)?;
            }
            Ok(s.undefined())
        }
        10 => {
            let index = value.to_number()? as usize;
            let object = WIRE_CANVASES.with(|c| c.borrow().get(index).cloned());
            let object = object.ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            crate::canvas::make_canvas_object(s, &object)
        }
        11 => {
            let Some(resource) = CodecResource::parts(s, value)? else { return Ok(s.undefined()) };
            let index = WIRE_CODECS.with_borrow_mut(|codecs| {
                let index = codecs.len();
                codecs.push(resource);
                index
            });
            Ok(s.number(index as f64))
        }
        12 => {
            let index = value.to_number()? as usize;
            let resource = WIRE_CODECS.with_borrow(|codecs| codecs.get(index).cloned())
                .ok_or_else(|| JsError::dom("DataCloneError", "Malformed WebCodecs resource"))?;
            resource.wrap(s)
        }
        15 => geometry_parts(s, value),
        16 => crate::canvas::MODULE.helper(s, &crate::target::page_of(s)?, "geometryFromParts", &[value, args.get(2)]),
        13 => crate::canvas::MODULE.helper(s, &crate::target::page_of(s)?, "imageInfo", &[value]),
        14 => {
            let tree = value.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed ImageData"))?;
            crate::canvas::MODULE.helper(s, &crate::target::page_of(s)?, "imageDataOf", &[
                args.get(2), tree.get_index(1)?, tree.get_index(2)?, tree.get_index(3)?,
            ])
        }
        _ => {
            let parts = value.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
            crate::streams::transfer_in(s, parts.get_index(0)?, parts.get_index(1)?)
        }
    }
}

/// The realm's `[encode, decode]`, compiled on its first use.
fn wire_functions<'s>(s: &Scope<'s>) -> JsResult<(pyro_js::Object<'s>, pyro_js::Object<'s>)> {
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
                s.eval(include_str!("worker_wire.js"), Some("pyro://internal/worker-wire.js"), 1)?;
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
pub fn encode_message<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>, options: pyro_js::Value<'s>) -> JsResult<WireMessage> {
    encode(s, value, options, false)
}

/// StructuredSerializeForStorage for persistent state such as history entries.
pub fn encode_for_storage<'s>(s: &Scope<'s>, value: pyro_js::Value<'s>) -> JsResult<WireMessage> {
    encode(s, value, s.undefined(), true)
}

fn encode<'s>(
    s: &Scope<'s>,
    value: pyro_js::Value<'s>,
    options: pyro_js::Value<'s>,
    for_storage: bool,
) -> JsResult<WireMessage> {
    let page = crate::target::page_of(s)?;
    let clone = page.structured_clones.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let clone = clone.ok_or_else(|| JsError::dom("NotSupportedError", "structured clone is unavailable"))?;
    let (encode, _) = wire_functions(s)?;
    let saved = WIRE_BLOBS.with(|b| b.take());
    let saved_canvases = WIRE_CANVASES.with(|c| c.take());
    let saved_codecs = WIRE_CODECS.with(|c| c.take());
    let pair = encode.call(None, &[clone, value, options, s.boolean(for_storage)]);
    let codecs = WIRE_CODECS.with(|c| c.replace(saved_codecs));
    let blobs = WIRE_BLOBS.with(|b| b.replace(saved));
    let canvases = WIRE_CANVASES.with(|c| c.replace(saved_canvases));
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
    // The whole message serialized: only now are its ports detached. All
    // are checked again first (user code ran during the walk), so none is
    // detached unless all can be.
    let list = pair.get_index(2)?;
    let list = list.as_object().expect("port list");
    let count = list.get_atom("length")?.to_number()? as u32;
    let mut sent = Vec::with_capacity(count as usize);
    for i in 0..count {
        let port = list.get_index(i)?;
        if !crate::message_channel::is_transferable_port(s, port)? {
            return Err(JsError::dom("DataCloneError", "Invalid MessagePort transfer"));
        }
        sent.push(port);
    }
    let mut ports = Vec::with_capacity(sent.len());
    for port in sent {
        ports.push(crate::message_channel::export_port(s, port)?);
    }
    let listed_ports = (pair.get_index(3)?.to_number()? as usize).min(ports.len());
    Ok(WireMessage { text, buffers, blobs, ports, listed_ports, canvases, codecs })
}

/// StructuredDeserialize into `s`'s realm.
pub fn decode_message<'s>(s: &Scope<'s>, message: &WireMessage) -> JsResult<pyro_js::Value<'s>> {
    decode_with_ports(s, message).map(|(value, _)| value)
}

/// [`decode_message`] that also returns the new ports of the MessagePorts
/// the message transfers, in transfer-list order (a `MessageEvent`'s
/// `ports`), or `None` when it transfers none. A message's ports are
/// claimed by its first deserialization: a second one fails.
pub(crate) fn decode_with_ports<'s>(
    s: &Scope<'s>,
    message: &WireMessage,
) -> JsResult<(pyro_js::Value<'s>, Option<pyro_js::Object<'s>>)> {
    // Reject an incompatible recipient before claiming any transferred ports.
    if !message.codecs.is_empty() {
        let page = crate::target::page_of(s)?;
        if !crate::encoded_chunks::exposed_in(s) || message.codecs.iter().any(|resource| !resource.readable_in(&page)) {
            return Err(JsError::dom("DataCloneError", "WebCodecs resources are not available to the receiving agent cluster"));
        }
    }
    let (_, decode) = wire_functions(s)?;
    let list = s.array(&[])?;
    for (i, bytes) in message.buffers.iter().enumerate() {
        list.set_index(i as u32, crate::encoding::array_buffer(s, bytes)?)?;
    }
    let run = |ports: pyro_js::Value<'s>| {
        let saved = WIRE_BLOBS.with(|b| b.replace(message.blobs.clone()));
        let saved_canvases = WIRE_CANVASES.with(|c| c.replace(message.canvases.clone()));
        let saved_codecs = WIRE_CODECS.with(|c| c.replace(message.codecs.clone()));
        let result = decode.call(None, &[s.string(&message.text), list.value(), ports]);
        WIRE_CODECS.with(|c| *c.borrow_mut() = saved_codecs);
        WIRE_BLOBS.with(|b| *b.borrow_mut() = saved);
        WIRE_CANVASES.with(|c| *c.borrow_mut() = saved_canvases);
        result
    };
    if message.ports.is_empty() {
        // Most messages transfer nothing: no ports array to make.
        return run(s.undefined()).map(|value| (value, None));
    }
    let ports = s.array(&[])?;
    let mut made = Vec::with_capacity(message.ports.len());
    let mut result = Ok(());
    for (i, transit) in message.ports.iter().enumerate() {
        match crate::message_channel::import_port(s, transit) {
            Ok((port, inner)) => {
                made.push(inner);
                if let Err(e) = ports.set_index(i as u32, port) {
                    result = Err(e);
                    break;
                }
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let result = result.and_then(|()| run(ports.value()));
    match result {
        Ok(value) if message.listed_ports == 0 => Ok((value, None)),
        Ok(value) if message.listed_ports == made.len() => Ok((value, Some(ports))),
        Ok(value) => {
            // The event's `ports` are the listed ones, not the streams' ports.
            let listed = s.array(&[])?;
            for i in 0..message.listed_ports {
                listed.set_index(i as u32, ports.get_index(i as u32)?)?;
            }
            Ok((value, Some(listed)))
        }
        Err(error) => {
            // No one can reach the ports made so far: let their partners go.
            for port in made {
                crate::message_channel::close_inner(&port);
            }
            Err(error)
        }
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if CLONE_SOURCE.with(|c| c.borrow().is_some()) {
        return Some("structured_clone::CLONE_SOURCE");
    }
    if WIRE_BLOBS.with(|b| !b.borrow().is_empty()) {
        return Some("structured_clone::WIRE_BLOBS");
    }
    if WIRE_CANVASES.with(|c| !c.borrow().is_empty()) {
        return Some("structured_clone::WIRE_CANVASES");
    }
    WIRE_CODECS.with_borrow(|codecs| !codecs.is_empty()).then_some("structured_clone::WIRE_CODECS")
}
