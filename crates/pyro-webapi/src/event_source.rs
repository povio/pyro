//! HTML server-sent events: `EventSource`, made in JS (event_source.js) on a
//! realm's first use of the name, in windows and dedicated workers. The
//! script parses the stream, dispatches the events and reconnects; it gets
//! one native transport (`natives`): a streaming GET through the page's
//! context network client whose UTF-8 text arrives in chunks as it is
//! received. Realms that never name `EventSource` pay one accessor pair.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value, WeakRealm};
use pyro_net::http::{header, HeaderValue};
use pyro_net::{Credentials, Destination, NetError, Request, RequestId, ResponseEvent};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::window::WindowHost;

/// Adds the lazy `EventSource` name to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.global_value_accessor(
        "EventSource",
        |s, w| global(s, w.page(), "EventSource"),
        |s, w, value| replace(s, w.page(), "EventSource", value),
    );
}

/// The accessor's getter: makes the module, then shadows the accessor with
/// a plain data property (as the interface object would be).
pub(crate) fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

/// Defines `name` on the global as a writable, non-enumerable data property.
pub(crate) fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = match page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s)) {
        Some(define) => define,
        // A worker realm, which records no original.
        None => s.global().get_atom("Object")?.as_object().ok_or_else(unavailable)?.get_atom("defineProperty")?,
    };
    let define = define.as_object().ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("EventSource is unavailable".into())
}

/// Makes the module. Only the global's accessor asks, once per realm: the
/// accessor is then shadowed by a data property, so nothing keeps the module.
fn module<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let make = s.eval(include_str!("event_source.js"), Some("pyro://internal/event_source.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[natives(s)?.value()])?;
    module.as_object().ok_or_else(unavailable)
}

/// What the transport reports to the script, in order.
enum Report {
    /// The response head: status, MIME essence, the final URL's origin.
    Open(u16, String, String),
    Text(String),
    End,
    /// A network error; `true` when the connection must not be retried (a
    /// CORS failure, or a URL that is not http(s)).
    Failed(bool),
}

#[derive(Default)]
struct Queue {
    reports: Vec<Report>,
    /// A task to drain the queue is posted and has not run.
    scheduled: bool,
}

/// An open connection: owned by the page's thread, reached by request id.
struct Source {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: pyro_js::RealmId,
    callback: Persistent,
    queue: Arc<Mutex<Queue>>,
}

thread_local! {
    // Only ids cross to the I/O thread; JS values stay here.
    static SOURCES: RefCell<FxHashMap<RequestId, Source>> = RefCell::new(FxHashMap::default());
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // The absolute URL of `url` against the document (or worker) base, or
    // null when it does not parse.
    let resolve = s.function("resolve", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let input = crate::util::dom_string(a.get(0))?;
        let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), &input);
        Ok(match pyro_net::Url::parse(&resolved) {
            Ok(url) => s.string(url.as_str()),
            Err(_) => s.null(),
        })
    })?;
    natives.set_atom("resolve", resolve.value())?;
    // connect(url, withCredentials, lastEventId, report) -> id: starts the
    // GET; `report(kind, ...)` is called with "open" (status, type, origin),
    // "text" (chunk), "end" or "error" (fatal).
    let connect = s.function("connect", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        if page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        let url = pyro_net::Url::parse(&crate::util::dom_string(a.get(0))?)
            .map_err(|_| JsError::dom("SyntaxError", "Invalid EventSource URL"))?;
        let callback = Persistent::new(s, a.get(3));
        let id = start(s, &page, url, a.get(1).to_bool(), &crate::util::dom_string(a.get(2))?, callback)?;
        Ok(s.number(id.0 as f64))
    })?;
    natives.set_atom("connect", connect.value())?;
    let cancel = s.function("cancel", |s, _this, a| {
        let id = RequestId(a.get(0).to_number()? as u64);
        if let Some(source) = SOURCES.with(|m| m.borrow_mut().remove(&id)) {
            if let Some(net) = source.page.upgrade().and_then(|p| p.net.borrow().clone()) {
                net.cancel(id);
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("cancel", cancel.value())?;
    // HTML initializes the MessageEvent with the final response URL's
    // origin. Build a native event rather than consulting the realm's
    // author-replaceable MessageEvent constructor.
    let message = s.function("message", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let target = s.unwrap::<crate::target::EventTarget>(a.get(0)).ok_or(JsError::ContextDestroyed)?.key;
        let event = crate::event::EventCore::trusted(
            &page, crate::event::EventInterface::MessageEvent, &a.get(1).to_string()?, false, false,
        );
        {
            let mut fields = event.fields.borrow_mut();
            fields.message_data = Some(Persistent::new(s, a.get(2)));
            fields.set_sender_origin(a.get(3).to_string()?);
            fields.last_event_id = Some(a.get(4).to_string()?.into_boxed_str());
        }
        crate::target::fire(&page, target, &event);
        Ok(s.undefined())
    })?;
    natives.set_atom("message", message.value())?;
    Ok(natives)
}

fn start(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    url: pyro_net::Url,
    with_credentials: bool,
    last_event_id: &str,
    callback: Persistent,
) -> JsResult<RequestId> {
    let net = page.net.borrow().clone().ok_or_else(|| JsError::Type("Page networking is not installed".into()))?;
    let poster = page.remote_poster().ok_or_else(|| JsError::Type("Browser task poster is not installed".into()))?;
    // A scheme that cannot serve an event stream fails the connection for
    // good instead of reconnecting (Chrome).
    let fatal_scheme = !matches!(url.scheme(), "http" | "https");
    let mut request = Request::get(url, Destination::EventSource);
    request.credentials = if with_credentials { Credentials::Include } else { Credentials::SameOrigin };
    request.cache = pyro_net::CacheMode::NoStore;
    request.headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if !last_event_id.is_empty() {
        // The id is UTF-8 (it never holds NUL, CR or LF).
        if let Ok(v) = HeaderValue::from_bytes(last_event_id.as_bytes()) {
            request.headers.insert(header::HeaderName::from_static("last-event-id"), v);
        }
    }
    let document_url = page.document_url_parsed();
    request.origin = Some(s.realm().origin().unwrap_or_else(|| {
        document_url.as_ref().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into())
    }));
    request.referrer = document_url;
    request.referrer_policy = page.referrer_policy();
    let context = page.request_context();
    request.frame_id = context.frame_id;
    request.loader_id = context.loader_id;
    // The stream is not buffered: no body bound.
    request.max_body = Some(usize::MAX);
    page.apply_request_defaults(&mut request);
    let id = net.next_request_id();
    let queue = Arc::new(Mutex::new(Queue::default()));
    SOURCES.with(|m| {
        m.borrow_mut().insert(
            id,
            Source {
                page: Rc::downgrade(page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                callback,
                queue: queue.clone(),
            },
        )
    });
    let mut decoder = encoding_rs::UTF_8.new_decoder_with_bom_removal();
    let url = request.url.clone();
    let sink: pyro_net::ResponseSink = Box::new(move |event| {
        let report = match event {
            ResponseEvent::Redirect { .. } => return,
            ResponseEvent::Response(head) => {
                Report::Open(head.status.as_u16(), head.mime.clone(), head.url.origin().ascii_serialization())
            }
            ResponseEvent::Data(bytes) => {
                let mut text = String::with_capacity(bytes.len() + 4);
                let _ = decoder.decode_to_string(&bytes, &mut text, false);
                if text.is_empty() {
                    return;
                }
                Report::Text(text)
            }
            ResponseEvent::Finished { .. } => {
                let mut text = String::with_capacity(4);
                let _ = decoder.decode_to_string(&[], &mut text, true);
                if !text.is_empty() {
                    queue.lock().unwrap().reports.push(Report::Text(text));
                }
                Report::End
            }
            ResponseEvent::Failed(error) => Report::Failed(fatal_scheme || matches!(error, NetError::Cors(_))),
        };
        let post = {
            let mut q = queue.lock().unwrap();
            q.reports.push(report);
            !std::mem::replace(&mut q.scheduled, true)
        };
        if post {
            poster(Box::new(move || pump(id)));
        }
    });
    let sink = crate::resource_timing::timed_sink(page, crate::resource_timing::Initiator::Other, &url, false, sink);
    net.fetch_with_id(id, request, sink);
    Ok(id)
}

/// Delivers a connection's queued reports to its script.
fn pump(id: RequestId) {
    let Some((realm, callback, queue)) = SOURCES.with(|m| {
        m.borrow().get(&id).map(|source| (source.realm.clone(), source.callback.clone(), source.queue.clone()))
    }) else {
        return;
    };
    let reports = {
        let mut q = queue.lock().unwrap();
        q.scheduled = false;
        std::mem::take(&mut q.reports)
    };
    let Some(realm) = realm.upgrade() else {
        forget(id);
        return;
    };
    let Ok(s) = realm.scope() else {
        forget(id);
        return;
    };
    let Some(f) = callback.get(&s).as_object() else { return };
    for report in reports {
        // The script may close the connection while it handles a report.
        if !SOURCES.with(|m| m.borrow().contains_key(&id)) {
            return;
        }
        let args: Vec<Value<'_>> = match &report {
            Report::Open(status, mime, origin) => {
                vec![s.string("open"), s.number(f64::from(*status)), s.string(mime), s.string(origin)]
            }
            Report::Text(text) => vec![s.string("text"), s.string(text)],
            Report::End => vec![s.string("end")],
            Report::Failed(fatal) => vec![s.string("error"), s.boolean(*fatal)],
        };
        if matches!(report, Report::End | Report::Failed(_)) {
            forget(id);
        }
        if let Err(e) = f.call(None, &args) {
            s.report_exception(&e);
        }
    }
}

fn forget(id: RequestId) {
    SOURCES.with(|m| m.borrow_mut().remove(&id));
}

/// Stops the connections of a document that goes away (`fetch::dispose`).
pub(crate) fn dispose(page: &PageDom) {
    cancel_where(page, |_| true);
}

/// Stops the connections of a realm that goes away.
pub(crate) fn cancel_realm(page: &PageDom, realm: pyro_js::RealmId) {
    cancel_where(page, |source| source.realm_id == realm);
}

fn cancel_where(page: &PageDom, pick: impl Fn(&Source) -> bool) {
    let ids: Vec<RequestId> = SOURCES.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_empty() {
            return Vec::new();
        }
        let ids: Vec<RequestId> = m
            .iter()
            .filter(|(_, source)| std::ptr::eq(source.page.as_ptr(), page) && pick(source))
            .map(|(id, _)| *id)
            .collect();
        for id in &ids {
            m.remove(id);
        }
        ids
    });
    if let Some(net) = page.net.borrow().as_ref() {
        for id in ids {
            net.cancel(id);
        }
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    SOURCES.with(|s| !s.borrow().is_empty()).then_some("event_source::SOURCES")
}
