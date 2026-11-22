//! WebSocket, CloseEvent and the stream-based socket API in windows and workers.
//!
//! The interfaces are made in JS (websocket.js) on a realm's first use of
//! either name, like streams: a realm that never touches them pays one global
//! accessor pair per name and nothing else. The connection itself is pyro-net's
//! ([`pyro_net::NetClient::websocket`]), on the context's I/O thread with its
//! cookies, proxy, TLS and CDP observation. Its events come back as tasks
//! posted to the owner's event loop through the page's remote poster (one
//! task per event; `bufferedAmount` updates are coalesced), so nothing polls
//! and a page without sockets has nothing on its paths. Sockets are closed
//! (aborted) when their realm or document goes away.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, RealmId, Scope, Value, WeakRealm};
use pyro_net::{Destination, Request, WebSocketHandle, WebSocketRequest, WsEvent};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::target::{page_of, EventTarget};
use crate::window::WindowHost;

/// Kept on the window or worker realm's native navigator.
const KEY: &str = "lpWebSocket";

macro_rules! lazy_globals {
    ($def:ident) => {
        $def.global_value_accessor("WebSocket", |s, _| global(s, "WebSocket"), |s, _, value| replace(s, "WebSocket", value));
        $def.global_value_accessor("CloseEvent", |s, _| global(s, "CloseEvent"), |s, _, value| replace(s, "CloseEvent", value));
        $def.global_value_accessor("WebSocketStream", |s, _| global(s, "WebSocketStream"), |s, _, value| replace(s, "WebSocketStream", value));
        $def.global_value_accessor("WebSocketError", |s, _| global(s, "WebSocketError"), |s, _, value| replace(s, "WebSocketError", value));
    };
}

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(def);
}

/// Adds them to a worker's global scope class.
pub(crate) fn define_worker_globals<W: JsClass>(def: &mut ClassDef<W>) {
    lazy_globals!(def);
}

fn global<'s>(s: &Scope<'s>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s)?.get(name)?;
    replace(s, name, value)?;
    Ok(value)
}

/// Shadows the accessor with the interface object's usual data property; also
/// what assigning the name does, so page polyfills keep working.
fn replace(s: &Scope<'_>, name: &str, value: Value<'_>) -> JsResult<()> {
    let page = page_of(s)?;
    // A window's `Object.defineProperty`, captured before page script ran.
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let target = [s.global().value(), s.string(name), value];
    match define.and_then(|v| v.as_object()) {
        Some(define) => {
            let desc = s.object();
            desc.set_atom("value", value)?;
            desc.set_atom("writable", s.boolean(true))?;
            desc.set_atom("enumerable", s.boolean(false))?;
            desc.set_atom("configurable", s.boolean(true))?;
            define.call(None, &[target[0], target[1], desc.value()])?;
        }
        // A worker: the module's, captured when it was made.
        None => {
            module(s)?.get_atom("define")?.as_object().ok_or_else(unavailable)?.call(None, &target)?;
        }
    }
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("WebSocket is unavailable".into())
}

/// The realm's module when it was made.
fn existing<'s>(s: &Scope<'s>) -> JsResult<Option<Object<'s>>> {
    let holder = crate::worker_scope::module_holder(s, &page_of(s)?)?;
    Ok(s.get_hidden(holder, KEY)?.and_then(|v| v.as_object()))
}

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    if let Some(module) = existing(s)? {
        return Ok(module);
    }
    let make = s.eval(include_str!("websocket.js"), Some("pyro://internal/websocket.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[natives(s)?.value()])?;
    let holder = crate::worker_scope::module_holder(s, &page_of(s)?)?;
    s.set_hidden(holder, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// One connection, owned by its realm's thread.
struct Socket {
    realm: WeakRealm,
    realm_id: RealmId,
    page: Weak<PageDom>,
    /// The WebSocket or WebSocketStream, kept alive until the connection closes.
    object: Persistent,
    url: pyro_net::Url,
    top_level_site: Option<String>,
    cookie_site: pyro_net::cookies::SiteContext,
    handle: RefCell<Option<WebSocketHandle>>,
    stream: bool,
    closing: Cell<bool>,
    /// A stream observes its constructor signal only until the handshake ends.
    handshake_abort: RefCell<Option<(crate::abort::AbortRegistration, Persistent)>>,
    /// Bytes written since the last `sent` delivery, and whether one is posted.
    sent: Arc<AtomicUsize>,
    sent_posted: Arc<AtomicBool>,
}

thread_local! {
    /// The thread's open connections by key. Empty unless script opened one.
    static SOCKETS: RefCell<FxHashMap<u64, Rc<Socket>>> = RefCell::new(FxHashMap::default());
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

fn socket(key: u64) -> Option<Rc<Socket>> {
    SOCKETS.with(|sockets| sockets.borrow().get(&key).cloned())
}

fn write_stream(key: u64, write: impl FnOnce(&WebSocketHandle)) -> JsResult<()> {
    let closed = || JsError::dom("InvalidStateError", "The WebSocket connection is closing.");
    let socket = socket(key).filter(|socket| socket.stream && !socket.closing.get()).ok_or_else(closed)?;
    let handle = socket.handle.borrow();
    write(handle.as_ref().ok_or_else(closed)?);
    Ok(())
}

/// Whether `name` is an RFC 7230 token (a subprotocol name).
fn is_token(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| (0x21..0x7f).contains(&b) && !b"()<>@,;:\\\"/[]?={}".contains(&b))
}

fn syntax_error(message: String) -> JsError {
    JsError::dom("SyntaxError", format!("Failed to construct 'WebSocket': {message}"))
}

/// The URL steps of `new WebSocket(url)`: the resolved `ws:`/`wss:` URL.
fn parse_url(page: &PageDom, input: &str) -> JsResult<pyro_net::Url> {
    let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), input);
    let mut url = pyro_net::Url::parse(&resolved).map_err(|_| syntax_error(format!("The URL '{input}' is invalid.")))?;
    match url.scheme() {
        "ws" | "wss" => {}
        "http" => {
            let _ = url.set_scheme("ws");
        }
        "https" => {
            let _ = url.set_scheme("wss");
        }
        other => {
            return Err(syntax_error(format!(
                "The URL's scheme must be either 'http', 'https', 'ws', or 'wss'. '{other}' is not allowed."
            )))
        }
    }
    if let Some(fragment) = url.fragment() {
        return Err(syntax_error(format!(
            "The URL contains a fragment identifier ('{fragment}'). Fragment identifiers are not allowed in WebSocket URLs."
        )));
    }
    Ok(url)
}

fn validate_protocols(protocols: &[String]) -> JsResult<()> {
    for (i, protocol) in protocols.iter().enumerate() {
        if !is_token(protocol) {
            return Err(syntax_error(format!("The subprotocol '{protocol}' is invalid.")));
        }
        if protocols[..i].iter().any(|p| p.eq_ignore_ascii_case(protocol)) {
            return Err(syntax_error(format!("The subprotocol '{protocol}' is duplicated.")));
        }
    }
    Ok(())
}

/// Posts `event` for socket `key` to the owner's loop. Runs on the I/O thread.
fn sink(
    key: u64,
    poster: crate::page::RemotePoster,
    sent: Arc<AtomicUsize>,
    sent_posted: Arc<AtomicBool>,
    stream: bool,
) -> pyro_net::WsSink {
    Box::new(move |event| match event {
        WsEvent::Sent(n) if !stream => {
            sent.fetch_add(n, Ordering::AcqRel);
            if !sent_posted.swap(true, Ordering::AcqRel) {
                poster(Box::new(move || deliver_sent(key)));
            }
        }
        event => poster(Box::new(move || deliver(key, event))),
    })
}

/// Calls the module's `deliver(ws, kind, ...)` in the socket's realm.
fn call_deliver(socket: &Socket, kind: &str, args: impl for<'s> FnOnce(&Scope<'s>) -> JsResult<[Value<'s>; 4]>) {
    let Some(page) = socket.page.upgrade() else { return };
    if page.is_disposed() {
        return;
    }
    let Some(realm) = socket.realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let run = || -> JsResult<()> {
        let module = existing(&s)?.ok_or_else(unavailable)?;
        let deliver = module.get_atom("deliver")?.as_object().ok_or_else(unavailable)?;
        let [a, b, c, d] = args(&s)?;
        deliver.call(None, &[socket.object.get(&s), s.string(kind), a, b, c, d])?;
        Ok(())
    };
    if let Err(error) = run() {
        s.report_exception(&error);
    }
}

fn deliver_sent(key: u64) {
    let Some(socket) = socket(key) else { return };
    socket.sent_posted.store(false, Ordering::Release);
    let n = socket.sent.swap(0, Ordering::AcqRel);
    call_deliver(&socket, "sent", |s| Ok([s.number(n as f64), s.undefined(), s.undefined(), s.undefined()]));
}

fn deliver(key: u64, event: WsEvent) {
    let Some(socket) = socket(key) else { return };
    match event {
        WsEvent::Open { protocol, extensions, headers } => {
            drop(socket.handshake_abort.borrow_mut().take());
            // The handshake's cookies become visible to script now.
            if let Some(page) = socket.page.upgrade() {
                if let Some(net) = page.net.borrow().as_ref() {
                    net.store_websocket_cookies_for(&socket.url, &headers, socket.top_level_site.as_deref(), socket.cookie_site);
                }
            }
            call_deliver(&socket, "open", |s| {
                Ok([s.string(&protocol), s.string(&extensions), s.undefined(), s.undefined()])
            });
        }
        WsEvent::Text(text) => {
            call_deliver(&socket, "message", |s| Ok([s.string(&text), s.undefined(), s.undefined(), s.undefined()]))
        }
        WsEvent::Binary(bytes) => call_deliver(&socket, "message", |s| {
            let chunk = if socket.stream {
                crate::encoding::uint8_array(s, &bytes)?
            } else {
                crate::encoding::array_buffer(s, &bytes)?
            };
            Ok([chunk, s.undefined(), s.undefined(), s.undefined()])
        }),
        // Stream writes are acknowledged individually, including empty messages.
        WsEvent::Sent(n) => call_deliver(&socket, "sent", |s| {
            Ok([s.number(n as f64), s.undefined(), s.undefined(), s.undefined()])
        }),
        WsEvent::Closed { clean, code, reason, failed } => {
            SOCKETS.with(|sockets| sockets.borrow_mut().remove(&key));
            socket.handle.borrow_mut().take();
            drop(socket.handshake_abort.borrow_mut().take());
            call_deliver(&socket, "close", |s| {
                Ok([s.boolean(clean), s.number(code as f64), s.string(&reason), s.boolean(failed)])
            });
        }
    }
}

/// "Fail the WebSocket connection" without a network connection.
fn fail_soon(page: &PageDom, key: u64) {
    let closed = WsEvent::Closed { clean: false, code: 1006, reason: String::new(), failed: true };
    match page.remote_poster() {
        Some(poster) => poster(Box::new(move || deliver(key, closed))),
        None => {
            SOCKETS.with(|sockets| sockets.borrow_mut().remove(&key));
        }
    }
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    let page = page_of(s)?;
    natives.set_atom("streamSlot", crate::streams::shared_slot(s, &page, "WebSocketStream.state")?)?;
    natives.set_atom("errorSlot", crate::streams::shared_slot(s, &page, "WebSocketError.closeInfo")?)?;
    let constructors = s.function("streamConstructors", |s, _, _| {
        let streams = crate::streams::module(s, &page_of(s)?)?;
        Ok(s.array(&[streams.get_atom("ReadableStream")?, streams.get_atom("WritableStream")?])?.value())
    })?;
    natives.set_atom("streamConstructors", constructors.value())?;
    // connect(object, url, protocols, stream?, signal?) -> [key, URL, origin].
    // Stream key 0 means already aborted; -1 means no owner task route.
    let connect = s.function("connect", |s, _this, a| {
        let page = page_of(s)?;
        if page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        // Finish Web IDL dictionary conversion before validating the URL.
        let stream = a.get(3).to_bool();
        let signal_value = a.get(4);
        let signal = if stream && !signal_value.is_undefined() {
            Some(s.unwrap::<crate::abort::AbortSignal>(signal_value)
                .ok_or_else(|| JsError::Type("Expected an AbortSignal.".into()))?)
        } else {
            None
        };
        let input = a.get(1).to_string()?;
        let url = parse_url(&page, &input)?;
        let list = a.get(2).as_object().ok_or_else(unavailable)?;
        let len = list.get_atom("length")?.to_number()? as u32;
        let mut protocols = Vec::with_capacity(len as usize);
        for i in 0..len {
            protocols.push(list.get_index(i)?.to_string()?);
        }
        validate_protocols(&protocols)?;
        let result = |id: f64| -> JsResult<Value<'_>> {
            Ok(s.array(&[s.number(id), s.string(url.as_str()), s.string(&url.origin().ascii_serialization())])?.value())
        };
        if signal.as_ref().is_some_and(|s| s.aborted()) {
            return result(0.0);
        }
        let poster = page.remote_poster();
        if stream && poster.is_none() {
            return result(-1.0);
        }
        let key = NEXT.with(|n| n.replace(n.get() + 1));
        let top_level_site = page.cookie_partition_key();
        let same_site = page.cookie_site_context() == pyro_net::cookies::SiteContext::SameSite
            && top_level_site.is_some() && pyro_net::cookies::partition_key(&url) == top_level_site;
        let socket = Rc::new(Socket {
            realm: s.realm().downgrade(),
            realm_id: s.realm_id(),
            page: Rc::downgrade(&page),
            object: Persistent::new(s, a.get(0)),
            url: url.clone(),
            top_level_site,
            cookie_site: if same_site { pyro_net::cookies::SiteContext::SameSite } else { pyro_net::cookies::SiteContext::CrossSite },
            handle: RefCell::new(None),
            stream,
            closing: Cell::new(false),
            handshake_abort: RefCell::default(),
            sent: Arc::default(),
            sent_posted: Arc::default(),
        });
        SOCKETS.with(|sockets| sockets.borrow_mut().insert(key, socket.clone()));
        if let Some(signal) = signal {
            let registration = signal.register_native(Box::new(move |_| {
                // Drop the transport and remove the key before script can re-enter.
                let socket = SOCKETS.with(|sockets| sockets.borrow_mut().remove(&key));
                if let Some(socket) = socket {
                    drop(socket.handle.borrow_mut().take());
                    call_deliver(&socket, "abort", |s| Ok([s.undefined(); 4]));
                }
            }));
            *socket.handshake_abort.borrow_mut() = registration.map(|r| (r, Persistent::new(s, signal_value)));
        }
        // The realm's origin, else its document's (as fetch does).
        let origin = s.realm().origin().unwrap_or_else(|| {
            let document = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).ok();
            document
                .and_then(|u| pyro_net::Url::parse(&u).ok())
                .map_or_else(|| "null".into(), |u| u.origin().ascii_serialization())
        });
        let mut http_url = url.clone();
        let _ = http_url.set_scheme(if url.scheme() == "wss" { "https" } else { "http" });
        // Mixed content: a secure context only opens secure connections.
        let insecure = origin.starts_with("https:") && !pyro_net::cookies::is_trustworthy(&http_url);
        let net = page.net.borrow().clone();
        match (net, poster) {
            (Some(net), Some(poster)) if !insecure => {
                let mut defaults = Request::get(http_url, Destination::WebSocket);
                page.apply_request_defaults(&mut defaults);
                let request = WebSocketRequest {
                    url: url.clone(),
                    protocols,
                    origin,
                    top_level_site: socket.top_level_site.clone(),
                    cross_site_ancestor: page.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite,
                    frame_id: page.request_context().frame_id,
                    defaults: defaults.defaults,
                };
                let sink = sink(key, poster, socket.sent.clone(), socket.sent_posted.clone(), stream);
                let (_, handle) = if stream { net.websocket_stream(request, sink) } else { net.websocket(request, sink) };
                *socket.handle.borrow_mut() = Some(handle);
            }
            _ => fail_soon(&page, key),
        }
        result(key as f64)
    })?;
    natives.set_atom("connect", connect.value())?;
    // `send(key, data, live)`: the message's byte length; sent when `live`.
    let send = s.function("send", |s, _this, a| {
        let key = a.get(0).to_number()? as u64;
        let data = a.get(1);
        let live = a.get(2).to_bool();
        let handle = |f: &dyn Fn(&WebSocketHandle)| {
            if live {
                if let Some(socket) = socket(key) {
                    if let Some(handle) = socket.handle.borrow().as_ref() {
                        f(handle);
                    }
                }
            }
        };
        if !data.is_string() {
            let bytes = match s.unwrap::<crate::file::Blob>(data) {
                Some(blob) => Some(blob.bytes.clone()),
                None => crate::encoding::buffer_source_bytes(s, data)?.map(bytes::Bytes::from),
            };
            if let Some(bytes) = bytes {
                let len = bytes.len();
                handle(&|h| h.send_binary(bytes.clone()));
                return Ok(s.number(len as f64));
            }
        }
        // USVString: lone surrogates become U+FFFD.
        let text = data.to_string()?;
        let len = text.len();
        handle(&|h| h.send_text(text.clone()));
        Ok(s.number(len as f64))
    })?;
    natives.set_atom("send", send.value())?;
    let pull = s.function("pull", |s, _, a| {
        if let Some(socket) = socket(a.get(0).to_number()? as u64) {
            if let Some(handle) = socket.handle.borrow().as_ref() {
                handle.pull();
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("pull", pull.value())?;
    // Conversion can run script; recheck the connection after conversion.
    let write = s.function("writeStream", |s, _, a| {
        let key = a.get(0).to_number()? as u64;
        let value = a.get(1);
        match crate::encoding::buffer_source_bytes(s, value)? {
            Some(bytes) => write_stream(key, |handle| handle.send_binary(bytes.into()))?,
            None => {
                let text = value.to_string()?;
                write_stream(key, |handle| handle.send_text(text))?;
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("writeStream", write.value())?;
    // `close(key, code or -1, reason)`: the closing handshake.
    let close = s.function("close", |s, _this, a| {
        let key = a.get(0).to_number()? as u64;
        let code = a.get(1).to_number()?;
        let reason = a.get(2).to_string()?;
        if let Some(socket) = socket(key) {
            socket.closing.set(true);
            if let Some(handle) = socket.handle.borrow().as_ref() {
                handle.close((code >= 0.0).then_some(code as u16), reason);
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("close", close.value())?;
    // `abort(key)`: close() while connecting fails the connection.
    let abort = s.function("abort", |s, _this, a| {
        let key = a.get(0).to_number()? as u64;
        if let Some(socket) = socket(key) {
            drop(socket.handle.borrow_mut().take());
            drop(socket.handshake_abort.borrow_mut().take());
            if socket.stream {
                SOCKETS.with(|sockets| sockets.borrow_mut().remove(&key));
                if let Some(page) = socket.page.upgrade() {
                    page.post_task(Box::new(move || call_deliver(&socket, "close", |s| {
                        Ok([s.boolean(false), s.number(1006.0), s.string(""), s.boolean(true)])
                    })));
                }
            } else if let Some(page) = socket.page.upgrade() {
                fail_soon(&page, key);
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("abort", abort.value())?;
    // The reason as a USVString of at most 123 UTF-8 bytes.
    let reason = s.function("closeReason", |s, _this, a| {
        let reason = a.get(0).to_string()?;
        if reason.len() > 123 {
            return Err(JsError::dom(
                "SyntaxError",
                "Failed to execute 'close' on 'WebSocket': The close reason must not be greater than 123 UTF-8 bytes.",
            ));
        }
        Ok(s.string(&reason))
    })?;
    natives.set_atom("closeReason", reason.value())?;
    // `dispatch(target, event)`: fires `event` as the user agent does (trusted).
    let dispatch = s.function("dispatch", |s, _this, a| {
        let target = s.unwrap::<EventTarget>(a.get(0)).ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
        let event = s.unwrap::<crate::event::Event>(a.get(1)).ok_or_else(unavailable)?.clone();
        event.state.borrow_mut().is_trusted = true;
        crate::target::fire(&target.page, target.key, &event);
        Ok(s.undefined())
    })?;
    natives.set_atom("dispatch", dispatch.value())?;
    let get = s.function("getHandler", |s, _this, a| {
        let target = s.unwrap::<EventTarget>(a.get(0)).ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
        crate::target::event_handler(s, &target, handler_type(&a.get(1).to_string()?)?)
    })?;
    natives.set_atom("getHandler", get.value())?;
    let set = s.function("setHandler", |s, _this, a| {
        let target = s.unwrap::<EventTarget>(a.get(0)).ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
        crate::target::set_event_handler(s, &target, handler_type(&a.get(1).to_string()?)?, a.get(2))?;
        Ok(s.undefined())
    })?;
    natives.set_atom("setHandler", set.value())?;
    Ok(natives)
}

fn handler_type(name: &str) -> JsResult<&'static str> {
    Ok(match name {
        "open" => "open",
        "message" => "message",
        "error" => "error",
        "close" => "close",
        _ => return Err(unavailable()),
    })
}

/// Aborts the connections of `realm` (navigation, world removal).
pub(crate) fn cancel_realm(realm: RealmId) {
    let closed: Vec<Rc<Socket>> = SOCKETS.with(|sockets| {
        let mut sockets = sockets.borrow_mut();
        if sockets.is_empty() {
            return Vec::new();
        }
        let keys: Vec<u64> = sockets.iter().filter(|(_, s)| s.realm_id == realm).map(|(k, _)| *k).collect();
        keys.into_iter().filter_map(|k| sockets.remove(&k)).collect()
    });
    for socket in closed {
        drop(socket.handle.borrow_mut().take());
    }
}

/// Aborts the connections of `page`'s realms (document teardown).
pub(crate) fn dispose(page: &PageDom) {
    let closed: Vec<Rc<Socket>> = SOCKETS.with(|sockets| {
        let mut sockets = sockets.borrow_mut();
        if sockets.is_empty() {
            return Vec::new();
        }
        let keys: Vec<u64> = sockets
            .iter()
            .filter(|(_, s)| s.page.upgrade().is_none_or(|p| std::ptr::eq(&*p, page)))
            .map(|(k, _)| *k)
            .collect();
        keys.into_iter().filter_map(|k| sockets.remove(&k)).collect()
    });
    for socket in closed {
        drop(socket.handle.borrow_mut().take());
    }
}

/// Open connections of this thread (diagnostics, tests).
pub fn open_websockets() -> usize {
    SOCKETS.with(|sockets| sockets.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprotocols_are_distinct_tokens() {
        assert!(validate_protocols(&["chat".into(), "superchat".into()]).is_ok());
        assert!(validate_protocols(&["echo".into(), "eCho".into()]).is_err());
        assert!(validate_protocols(&["/echo".into()]).is_err());
        assert!(validate_protocols(&["a b".into()]).is_err());
        assert!(validate_protocols(&["".into()]).is_err());
        assert!(validate_protocols(&["\u{e9}".into()]).is_err());
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if SOCKETS.with(|s| !s.borrow().is_empty()) {
        return Some("websocket::SOCKETS");
    }
    NEXT.set(1);
    None
}
