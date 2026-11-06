//! `WebSocket` (WHATWG WebSockets) on `lp_net`'s RFC 6455 client.
//!
//! The constructor parses and checks the URL and subprotocols, then opens the
//! socket on the context's I/O thread. Each thing the socket reports (open,
//! each message, the close) becomes its own task on the page's event loop,
//! in order: `open`, `message` (`MessageEvent`, data a string, `Blob` or
//! `ArrayBuffer` per `binaryType`), and finally `error` (when the connection
//! failed) and `close` (`CloseEvent`).
//!
//! Lifetime: a socket that is not yet closed is pinned, so its listeners run
//! without a JS reference (as HTML keeps such sockets alive); the pin is
//! released when `close` is delivered. Navigating away or closing the page
//! closes its sockets (1001, going away) without further events.
//!
//! Gaps (explicit): workers have no WebSocket; no extensions are negotiated.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, JsResult, RealmId, Scope, Value, WeakRealm};
use lp_net::{Destination, Request, WebSocketHandle, WsEvent, WsMessage, WsRequest};
use rustc_hash::FxHashMap;

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, pin_target, unpin_target, EventTarget, EventTargetData};
use crate::util::dom_string;
use lp_dom::events::TargetKey;

const CONNECTING: u16 = 0;
const OPEN: u16 = 1;
const CLOSING: u16 = 2;
const CLOSED: u16 = 3;

thread_local! {
    /// Sockets not yet closed, by their registry id. Strong: a socket lives
    /// until it is closed or its page goes.
    static SOCKETS: RefCell<FxHashMap<u64, Rc<SocketInner>>> = RefCell::default();
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

pub(crate) struct SocketInner {
    id: u64,
    target: EventTargetData,
    realm: WeakRealm,
    realm_id: RealmId,
    url: String,
    /// Origin of the socket's URL: `MessageEvent.origin`.
    origin: String,
    ready: Cell<u16>,
    protocol: RefCell<String>,
    extensions: RefCell<String>,
    binary_arraybuffer: Cell<bool>,
    handle: RefCell<Option<WebSocketHandle>>,
    /// Bytes `send` was given after the socket started closing (they are
    /// counted, never sent).
    discarded: Cell<u64>,
    /// `close()` was called while connecting: the connection is failed.
    failed_connecting: Cell<bool>,
}

impl SocketInner {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }

    fn buffered_amount(&self) -> u64 {
        self.handle.borrow().as_ref().map_or(0, |h| h.buffered_amount()) + self.discarded.get()
    }

    fn unpin(&self) {
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                unpin_target(&s, self.page(), self.target.key);
            }
        }
    }

    /// Runs on the page's event loop, one task per socket event.
    fn on_event(self: &Rc<Self>, event: WsEvent) {
        if self.page().is_disposed() || self.ready.get() == CLOSED {
            return;
        }
        match event {
            WsEvent::Open { protocol, extensions } => {
                if self.ready.get() != CONNECTING {
                    return;
                }
                self.ready.set(OPEN);
                *self.protocol.borrow_mut() = protocol;
                *self.extensions.borrow_mut() = extensions;
                self.fire_simple("open");
            }
            WsEvent::Message(message) => {
                if self.ready.get() != OPEN {
                    return;
                }
                let Some(realm) = self.realm.upgrade() else { return };
                let Ok(s) = realm.scope() else { return };
                let data = match message {
                    WsMessage::Text(text) => Ok(s.string(&text)),
                    WsMessage::Binary(bytes) if self.binary_arraybuffer.get() => {
                        crate::encoding::array_buffer(&s, &bytes)
                    }
                    WsMessage::Binary(bytes) => s
                        .wrap::<crate::file::Blob>(
                            crate::file::BlobData { bytes, mime: String::new(), file: None },
                            None,
                        )
                        .map(|o| o.value()),
                };
                let data = match data {
                    Ok(d) => d,
                    Err(e) => {
                        s.report_exception(&e);
                        return;
                    }
                };
                let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, "message", false, false);
                {
                    let mut f = ev.fields.borrow_mut();
                    f.message_data = Some(lp_js::Persistent::new(&s, data));
                    f.origin = self.origin.clone();
                }
                fire(self.page(), self.target.key, &ev);
            }
            WsEvent::Closed { code, reason, clean, error } => {
                self.ready.set(CLOSED);
                self.handle.borrow_mut().take();
                SOCKETS.with(|m| m.borrow_mut().remove(&self.id));
                let failed = self.failed_connecting.get();
                if error.is_some() || failed || !clean {
                    self.fire_simple("error");
                }
                let (code, reason, clean) = if failed { (1006, String::new(), false) } else { (code, reason, clean) };
                let ev = EventCore::trusted(self.page(), EventInterface::CloseEvent, "close", false, false);
                {
                    let mut f = ev.fields.borrow_mut();
                    f.was_clean = clean;
                    f.close_code = code;
                    f.close_reason = reason;
                }
                fire(self.page(), self.target.key, &ev);
                self.unpin();
            }
        }
    }

    fn fire_simple(&self, ty: &str) {
        let ev = EventCore::trusted(self.page(), EventInterface::Event, ty, false, false);
        fire(self.page(), self.target.key, &ev);
    }

    /// Drops the connection without events (page or realm gone).
    fn abandon(&self) {
        self.ready.set(CLOSED);
        self.handle.borrow_mut().take();
    }
}

pub struct WebSocketData(Rc<SocketInner>);

/// Parses `url` against the document (WHATWG WebSockets "new WebSocket" steps
/// 1-7): http(s) become ws(s), anything else but ws(s) and fragments fail.
fn parse_url(page: &PageDom, input: &str) -> JsResult<lp_net::Url> {
    let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), input);
    let mut url = lp_net::Url::parse(&resolved).map_err(|_| {
        JsError::dom("SyntaxError", format!("Failed to construct 'WebSocket': The URL '{input}' is invalid."))
    })?;
    match url.scheme() {
        "http" => {
            let _ = url.set_scheme("ws");
        }
        "https" => {
            let _ = url.set_scheme("wss");
        }
        "ws" | "wss" => {}
        other => {
            return Err(JsError::dom(
                "SyntaxError",
                format!(
                    "Failed to construct 'WebSocket': The URL's scheme must be either 'http', 'https', 'ws', or 'wss'. '{other}' is not allowed."
                ),
            ))
        }
    }
    if url.fragment().is_some() {
        return Err(JsError::dom(
            "SyntaxError",
            format!("Failed to construct 'WebSocket': The URL contains a fragment identifier ('{}'). Fragment identifiers are not allowed in WebSocket URLs.", url.fragment().unwrap_or("")),
        ));
    }
    Ok(url)
}

/// A subprotocol is an HTTP token (RFC 7230 §3.2.6).
fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn protocols(s: &Scope<'_>, value: Value<'_>) -> JsResult<Vec<String>> {
    if value.is_undefined() {
        return Ok(Vec::new());
    }
    // `(DOMString or sequence<DOMString>)`: an object is a sequence.
    let list = if value.as_object().is_some() {
        let seq = s.sequence(value, "Failed to construct 'WebSocket'")?;
        let len = seq.get("length")?.to_number()? as u32;
        (0..len).map(|i| seq.get_index(i).and_then(dom_string)).collect::<JsResult<Vec<_>>>()?
    } else {
        vec![dom_string(value)?]
    };
    for (i, p) in list.iter().enumerate() {
        if !is_token(p) {
            return Err(JsError::dom(
                "SyntaxError",
                format!("Failed to construct 'WebSocket': The subprotocol '{p}' is invalid."),
            ));
        }
        if list[..i].contains(p) {
            return Err(JsError::dom(
                "SyntaxError",
                format!("Failed to construct 'WebSocket': The subprotocol '{p}' is duplicated."),
            ));
        }
    }
    Ok(list)
}

pub struct WebSocket;
impl JsClass for WebSocket {
    const NAME: &'static str = "WebSocket";
    type Data = WebSocketData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &WebSocketData| &d.0.target);
        def.constructor(1, |s, nt, a| {
            a.require(1, "WebSocket")?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let input = dom_string(a.get(0))?;
            let url = parse_url(&page, &input)?;
            let protocols = protocols(s, a.get(1))?;
            let net =
                page.net.borrow().clone().ok_or_else(|| JsError::Type("Page networking is not installed".into()))?;
            let poster =
                page.remote_poster().ok_or_else(|| JsError::Type("Browser task poster is not installed".into()))?;
            let document_url =
                page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            let mut request = WsRequest::new(url.clone());
            request.protocols = protocols;
            request.origin = Some(crate::storage::tuple_origin(s, &page).unwrap_or_else(|| "null".into()));
            request.document_url = lp_net::Url::parse(&document_url).ok();
            {
                let mut defaults =
                    Request::get(lp_net::Url::parse("http://localhost/").expect("static URL"), Destination::WebSocket);
                page.apply_request_defaults(&mut defaults);
                request.defaults = defaults.defaults;
            }
            let context = page.request_context();
            request.frame_id = context.frame_id;
            request.loader_id = context.loader_id;

            let target = EventTargetData::new(&page);
            let TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            let id = NEXT_ID.with(|n| n.replace(n.get() + 1));
            let origin = url.origin().ascii_serialization();
            let inner = Rc::new(SocketInner {
                id,
                target,
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                url: url.to_string(),
                origin,
                ready: Cell::new(CONNECTING),
                protocol: RefCell::new(String::new()),
                extensions: RefCell::new(String::new()),
                binary_arraybuffer: Cell::new(false),
                handle: RefCell::new(None),
                discarded: Cell::new(0),
                failed_connecting: Cell::new(false),
            });
            let object = s.wrap_new::<WebSocket>(WebSocketData(inner.clone()), nt, Some(key))?;
            pin_target(s, &page, inner.target.key, object);
            SOCKETS.with(|m| m.borrow_mut().insert(id, inner.clone()));
            let handle = net.websocket(
                request,
                Box::new(move |event| {
                    poster(Box::new(move || {
                        let socket = SOCKETS.with(|m| m.borrow().get(&id).cloned());
                        if let Some(socket) = socket {
                            socket.on_event(event);
                        }
                    }))
                }),
            );
            *inner.handle.borrow_mut() = Some(handle);
            Ok(object)
        });
        for (name, value) in [("CONNECTING", CONNECTING), ("OPEN", OPEN), ("CLOSING", CLOSING), ("CLOSED", CLOSED)] {
            def.constant(name, value as f64);
        }
        def.getter("url", |s, d| Ok(s.string(&d.0.url)));
        def.getter("readyState", |s, d| Ok(s.number(d.0.ready.get() as f64)));
        def.getter("bufferedAmount", |s, d| Ok(s.number(d.0.buffered_amount() as f64)));
        def.getter("extensions", |s, d| Ok(s.string(&d.0.extensions.borrow())));
        def.getter("protocol", |s, d| Ok(s.string(&d.0.protocol.borrow())));
        def.accessor(
            "binaryType",
            |s, d| Ok(s.string(if d.0.binary_arraybuffer.get() { "arraybuffer" } else { "blob" })),
            |_s, d, value| {
                // An enumeration attribute ignores values outside the enum.
                match dom_string(value)?.as_str() {
                    "arraybuffer" => d.0.binary_arraybuffer.set(true),
                    "blob" => d.0.binary_arraybuffer.set(false),
                    _ => {}
                }
                Ok(())
            },
        );
        def.method("send", 1, |s, d, a| {
            a.require(1, "WebSocket.send")?;
            let data = a.get(0);
            let message = if let Some(blob) = s.unwrap::<crate::file::Blob>(data) {
                WsMessage::Binary(blob.bytes.clone())
            } else if let Some(bytes) = crate::encoding::buffer_source_bytes(s, data)? {
                WsMessage::Binary(bytes.into())
            } else {
                WsMessage::Text(dom_string(data)?)
            };
            match d.0.ready.get() {
                CONNECTING => {
                    return Err(JsError::dom(
                        "InvalidStateError",
                        "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
                    ))
                }
                OPEN => {
                    if let Some(handle) = d.0.handle.borrow().as_ref() {
                        handle.send(message);
                    }
                }
                _ => d.0.discarded.set(d.0.discarded.get() + message.len() as u64),
            }
            Ok(s.undefined())
        });
        def.method("close", 0, |s, d, a| {
            let code = match a.opt(0).filter(|v| !v.is_undefined()) {
                Some(v) => {
                    let n = v.to_number()?;
                    // `[Clamp] unsigned short`.
                    let n = if n.is_nan() { 0.0 } else { n.clamp(0.0, 65535.0).round_ties_even() };
                    let code = n as u16;
                    if code != 1000 && !(3000..=4999).contains(&code) {
                        return Err(JsError::dom(
                            "InvalidAccessError",
                            format!("Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. {code} is neither."),
                        ));
                    }
                    Some(code)
                }
                None => None,
            };
            let reason = match a.opt(1).filter(|v| !v.is_undefined()) {
                Some(v) => dom_string(v)?,
                None => String::new(),
            };
            if reason.len() > 123 {
                return Err(JsError::dom(
                    "SyntaxError",
                    "Failed to execute 'close' on 'WebSocket': The close reason must not be greater than 123 UTF-8 bytes.",
                ));
            }
            match d.0.ready.get() {
                CLOSING | CLOSED => {}
                CONNECTING => {
                    d.0.ready.set(CLOSING);
                    d.0.failed_connecting.set(true);
                    if let Some(handle) = d.0.handle.borrow().as_ref() {
                        handle.close(code.or(Some(1000)), reason);
                    }
                }
                _ => {
                    d.0.ready.set(CLOSING);
                    if let Some(handle) = d.0.handle.borrow().as_ref() {
                        // A reason needs a code; close() without one sends none.
                        handle.close(code.or(if reason.is_empty() { None } else { Some(1000) }), reason);
                    }
                }
            }
            Ok(s.undefined())
        });
        macro_rules! handler {
            ($name:literal, $ty:literal) => {
                def.accessor(
                    $name,
                    |s, d| crate::target::event_handler(s, &d.0.target, $ty),
                    |s, d, value| crate::target::set_event_handler(s, &d.0.target, $ty, value),
                );
            };
        }
        handler!("onopen", "open");
        handler!("onmessage", "message");
        handler!("onerror", "error");
        handler!("onclose", "close");
    }
}

/// The page's sockets go away with it.
pub(crate) fn dispose(page: &PageDom) {
    let gone: Vec<Rc<SocketInner>> = SOCKETS.with(|m| {
        let mut m = m.borrow_mut();
        let ids: Vec<u64> =
            m.values().filter(|sock| std::ptr::eq(Rc::as_ptr(sock.page()), page)).map(|sock| sock.id).collect();
        ids.into_iter().filter_map(|id| m.remove(&id)).collect()
    });
    for socket in gone {
        socket.abandon();
    }
}

/// A disposed realm's sockets close.
pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    let gone: Vec<Rc<SocketInner>> = SOCKETS.with(|m| {
        let mut m = m.borrow_mut();
        let ids: Vec<u64> = m
            .values()
            .filter(|sock| sock.realm_id == realm && std::ptr::eq(Rc::as_ptr(sock.page()), page))
            .map(|sock| sock.id)
            .collect();
        ids.into_iter().filter_map(|id| m.remove(&id)).collect()
    });
    for socket in gone {
        socket.abandon();
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<WebSocket>();
}
