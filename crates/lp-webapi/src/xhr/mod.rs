//! Asynchronous XMLHttpRequest. Native state is browser-thread confined;
//! delivery buffers are bounded and I/O callbacks queue only routing IDs.
mod body;
mod delivery;
use crate::{
    event::{EventCore, EventInterface},
    page::{PageDom, RemotePoster},
    target::{page_of, EventTarget, EventTargetData},
    util::{dom_string, to_u32},
};
use bytes::Bytes;
use delivery::Delivery;
use lp_dom::events::TargetKey;
use lp_js::{
    Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, Object, Persistent, RealmId, Scope, Value, WeakRealm,
};
use lp_net::http::{header, HeaderName, HeaderValue, Method};
use lp_net::{Credentials, Destination, NetError, Request, RequestId, ResponseEvent, ResponseHead};
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

/// Independent XHR limits, with the same shape/defaults as fetch limits.
pub type XhrLimits = crate::fetch::FetchLimits;
const RESPONSE: &str = "__lpXhrResponse";

pub(crate) struct XhrState {
    limits: Cell<XhrLimits>,
    budget: Arc<AtomicUsize>,
    jobs: RefCell<FxHashMap<RequestId, Rc<Pending>>>,
    instances: RefCell<FxHashMap<u64, Weak<Xhr>>>,
    timers: RefCell<FxHashMap<RealmId, Persistent>>,
}
impl Default for XhrState {
    fn default() -> Self {
        Self {
            limits: Cell::new(XhrLimits::default()),
            budget: Arc::new(AtomicUsize::new(0)),
            jobs: RefCell::default(),
            instances: RefCell::default(),
            timers: RefCell::default(),
        }
    }
}
struct Pending {
    xhr: Rc<Xhr>,
    _wrapper: Persistent,
}
thread_local! {static PAGES:RefCell<FxHashMap<u64,Weak<PageDom>>>=RefCell::default();}
impl PageDom {
    pub fn set_xhr_limits(&self, limits: XhrLimits) -> bool {
        if !self.xhr_state.jobs.borrow().is_empty()
            || self.xhr_state.instances.borrow().values().filter_map(Weak::upgrade).any(|x| x.wire.borrow().is_some())
        {
            return false;
        }
        self.xhr_state.limits.set(limits);
        true
    }
    /// Pending browser delivery jobs and retained logical response bytes.
    pub fn xhr_usage(&self) -> (usize, usize) {
        (self.xhr_state.jobs.borrow().len(), self.xhr_state.budget.load(Ordering::Acquire))
    }
}

pub struct XhrData {
    target: EventTargetData,
    state: Rc<Xhr>,
}
struct Xhr {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: RealmId,
    key: TargetKey,
    identity: IdentityKey,
    instance: u64,
    ready: Cell<u8>,
    sent: Cell<bool>,
    generation: Cell<u64>,
    active: Cell<Option<RequestId>>,
    request: RefCell<Option<Request>>,
    head: RefCell<Option<ResponseHead>>,
    wire: RefCell<Option<Arc<Mutex<Delivery>>>>,
    response_type: RefCell<String>,
    with_credentials: Cell<bool>,
    timeout: Cell<u32>,
    timer: Cell<Option<f64>>,
    started: Cell<Option<Instant>>,
    override_mime: RefCell<Option<String>>,
    text: RefCell<Option<body::Text>>,
    reported: Cell<usize>,
    disposed: Cell<bool>,
}
impl Drop for Xhr {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            page.xhr_state.instances.borrow_mut().remove(&self.instance);
        }
    }
}
fn invalid(message: &str) -> JsError {
    JsError::dom("InvalidStateError", message)
}
fn unsupported(message: &str) -> JsError {
    JsError::dom("NotSupportedError", message)
}

pub struct XMLHttpRequest;
impl JsClass for XMLHttpRequest {
    const NAME: &'static str = "XMLHttpRequest";
    type Data = XhrData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &XhrData| &d.target);
        def.constructor(0, |s, nt, _| {
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let target = EventTargetData::new(&page);
            let TargetKey::Other(instance) = target.key else { unreachable!() };
            let identity = page.other_identity(instance);
            let state = Rc::new(Xhr {
                page: Rc::downgrade(&page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                key: target.key,
                identity,
                instance,
                ready: Cell::new(0),
                sent: Cell::new(false),
                generation: Cell::new(0),
                active: Cell::new(None),
                request: RefCell::new(None),
                head: RefCell::new(None),
                wire: RefCell::new(None),
                response_type: RefCell::new(String::new()),
                with_credentials: Cell::new(false),
                timeout: Cell::new(0),
                timer: Cell::new(None),
                started: Cell::new(None),
                override_mime: RefCell::new(None),
                text: RefCell::new(None),
                reported: Cell::new(0),
                disposed: Cell::new(false),
            });
            page.xhr_state.instances.borrow_mut().insert(instance, Rc::downgrade(&state));
            let wrapper = s.wrap_new::<Self>(XhrData { target, state }, nt, Some(identity))?;
            Ok(wrapper)
        });
        for (name, value) in [("UNSENT", 0.), ("OPENED", 1.), ("HEADERS_RECEIVED", 2.), ("LOADING", 3.), ("DONE", 4.)] {
            def.constant(name, value);
        }
        def.getter("readyState", |s, d| Ok(s.number(d.state.ready.get() as f64)));
        def.method("open", 2, |s, d, args| {
            d.state.open(s, args)?;
            Ok(s.undefined())
        });
        def.method("send", 0, |s, d, args| {
            d.state.send(s, args.get(0))?;
            Ok(s.undefined())
        });
        def.method("abort", 0, |s, d, _| {
            d.state.abort();
            Ok(s.undefined())
        });
        def.method("setRequestHeader", 2, |s, d, args| {
            args.require(2, "XMLHttpRequest.setRequestHeader")?;
            let name = crate::headers::byte_string(args.get(0))?;
            let value = crate::headers::byte_string(args.get(1))?;
            d.state.can_send()?;
            let name =
                crate::headers::validate_name(name).map_err(|_| JsError::dom("SyntaxError", "Invalid header name"))?;
            let value = crate::headers::validate_value(value)
                .map_err(|_| JsError::dom("SyntaxError", "Invalid header value"))?;
            if crate::fetch::forbidden_header(&name)
                || (matches!(name.as_str(), "x-http-method" | "x-http-method-override" | "x-method-override")
                    && value
                        .split(',')
                        .any(|v| matches!(v.trim().to_ascii_uppercase().as_str(), "CONNECT" | "TRACE" | "TRACK")))
            {
                return Ok(s.undefined());
            }
            let key =
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| JsError::dom("SyntaxError", "Invalid header"))?;
            let mut request = d.state.request.borrow_mut();
            let request = request.as_mut().expect("opened request");
            let mut bytes = request
                .headers
                .get(&key)
                .map(|v| {
                    let mut b = v.as_bytes().to_vec();
                    b.extend_from_slice(b", ");
                    b
                })
                .unwrap_or_default();
            bytes.extend(value.chars().map(|c| c as u8));
            request.headers.insert(
                key,
                HeaderValue::from_bytes(&bytes).map_err(|_| JsError::dom("SyntaxError", "Invalid header value"))?,
            );
            Ok(s.undefined())
        });
        def.getter("status", |s, d| {
            Ok(s.number(
                d.state
                    .head
                    .borrow()
                    .as_ref()
                    .filter(|_| d.state.ready.get() >= 2)
                    .map_or(0., |h| h.status.as_u16() as f64),
            ))
        });
        def.getter("statusText", |s, d| {
            Ok(s.string(
                &d.state
                    .head
                    .borrow()
                    .as_ref()
                    .filter(|_| d.state.ready.get() >= 2)
                    .map_or(String::new(), |h| h.status_text.clone()),
            ))
        });
        def.getter("responseURL", |s, d| {
            Ok(s.string(
                &d.state
                    .head
                    .borrow()
                    .as_ref()
                    .filter(|_| d.state.ready.get() >= 2)
                    .map_or(String::new(), |h| h.url.to_string()),
            ))
        });
        def.getter("responseText", |s, d| d.state.response_text(s));
        def.getter("response", |s, d| d.state.response(s));
        def.getter("responseXML", |_s, _d| Err(unsupported("XML response parsing is not implemented")));
        def.getter("upload", |_s, _d| Err(unsupported("XHR upload observation is not implemented")));
        def.accessor(
            "responseType",
            |s, d| Ok(s.string(&d.state.response_type.borrow())),
            |_s, d, value| {
                let kind = dom_string(value)?;
                if d.state.ready.get() >= 3 {
                    return Err(invalid("responseType cannot change after loading"));
                }
                if kind == "document" {
                    return Err(unsupported("Document responseType is not implemented"));
                }
                if matches!(kind.as_str(), "" | "text" | "json" | "arraybuffer" | "blob") {
                    *d.state.response_type.borrow_mut() = kind;
                }
                Ok(())
            },
        );
        def.accessor(
            "withCredentials",
            |s, d| Ok(s.boolean(d.state.with_credentials.get())),
            |_s, d, value| {
                if d.state.ready.get() > 1 || d.state.sent.get() {
                    return Err(invalid("withCredentials cannot change during a request"));
                }
                d.state.with_credentials.set(value.to_bool());
                Ok(())
            },
        );
        def.accessor(
            "timeout",
            |s, d| Ok(s.number(d.state.timeout.get() as f64)),
            |s, d, value| {
                let timeout = to_u32(value)?;
                d.state.timeout.set(timeout);
                if d.state.sent.get() {
                    d.state.schedule_timer(s)?;
                }
                Ok(())
            },
        );
        def.method("overrideMimeType", 1, |s, d, args| {
            args.require(1, "XMLHttpRequest.overrideMimeType")?;
            let mime = dom_string(args.get(0))?;
            if d.state.ready.get() >= 3 {
                return Err(invalid("MIME type cannot change after loading"));
            }
            *d.state.override_mime.borrow_mut() = Some(mime);
            Ok(s.undefined())
        });
        def.method("getResponseHeader", 1, |s, d, args| {
            args.require(1, "XMLHttpRequest.getResponseHeader")?;
            let name = crate::headers::byte_string(args.get(0))?.to_ascii_lowercase();
            let value = d.state.headers().into_iter().find(|(n, _)| n == &name).map(|(_, v)| v);
            Ok(value.map_or(s.null(), |v| s.string(&v)))
        });
        def.method("getAllResponseHeaders", 0, |s, d, _| {
            let mut out = String::new();
            for (name, value) in d.state.headers() {
                out.push_str(&name);
                out.push_str(": ");
                out.push_str(&value);
                out.push_str("\r\n");
            }
            Ok(s.string(&out))
        });
        macro_rules! handler {
            ($name:literal, $ty:literal) => {
                def.accessor(
                    $name,
                    |s, d| crate::target::event_handler(s, &d.target, $ty),
                    |s, d, value| crate::target::set_event_handler(s, &d.target, $ty, value),
                );
            };
        }
        handler!("onreadystatechange", "readystatechange");
        handler!("onloadstart", "loadstart");
        handler!("onprogress", "progress");
        handler!("onabort", "abort");
        handler!("onerror", "error");
        handler!("ontimeout", "timeout");
        handler!("onload", "load");
        handler!("onloadend", "loadend");
    }
}

impl Xhr {
    fn page(&self) -> JsResult<Rc<PageDom>> {
        let page = self.page.upgrade().ok_or(JsError::ContextDestroyed)?;
        if self.disposed.get() || page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        Ok(page)
    }
    fn can_send(&self) -> JsResult<()> {
        self.page()?;
        if self.ready.get() != 1 || self.sent.get() {
            Err(invalid("XHR must be opened and not sending"))
        } else {
            Ok(())
        }
    }
    fn wrapper<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let owner = realm.scope()?;
        let wrapper = owner.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
        // SAFETY: same Engine/VM; receiver or active Pending roots this wrapper.
        Ok(unsafe { Object::from_raw(s.ctx(), wrapper.as_raw()) })
    }
    fn headers(&self) -> Vec<(String, String)> {
        if self.ready.get() < 2 {
            return Vec::new();
        }
        self.head
            .borrow()
            .as_ref()
            .map(|head| {
                crate::response::filtered_headers(
                    head,
                    if self.with_credentials.get() { Credentials::Include } else { Credentials::SameOrigin },
                )
                .pairs()
            })
            .unwrap_or_default()
    }
    fn open(&self, s: &Scope<'_>, args: Args<'_>) -> JsResult<()> {
        args.require(2, "XMLHttpRequest.open")?;
        let method = crate::headers::byte_string(args.get(0))?;
        let url = dom_string(args.get(1))?;
        let asynchronous = args.len() < 3 || args.get(2).to_bool();
        let username =
            if args.get(3).is_null() || args.get(3).is_undefined() { String::new() } else { dom_string(args.get(3))? };
        let password =
            if args.get(4).is_null() || args.get(4).is_undefined() { String::new() } else { dom_string(args.get(4))? };
        let page = self.page()?;
        let upper = method.to_ascii_uppercase();
        let method =
            Method::from_bytes(if matches!(upper.as_str(), "DELETE" | "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT") {
                upper.as_bytes()
            } else {
                method.as_bytes()
            })
            .map_err(|_| JsError::dom("SyntaxError", "Invalid HTTP method"))?;
        if matches!(upper.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err(JsError::dom("SecurityError", "Forbidden HTTP method"));
        }
        let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &url);
        let mut url = lp_net::Url::parse(&resolved).map_err(|_| JsError::dom("SyntaxError", "Invalid URL"))?;
        url.set_fragment(None);
        if !asynchronous {
            return Err(unsupported("Synchronous XHR is not implemented"));
        }
        if !username.is_empty() || !password.is_empty() || !url.username().is_empty() || url.password().is_some() {
            return Err(unsupported("XHR URL credentials are not implemented"));
        }
        self.generation.set(self.generation.get().checked_add(1).expect("XHR generation exhausted"));
        self.stop();
        self.clear_response(s)?;
        let mut request = Request::get(url, Destination::Xhr);
        request.method = method;
        *self.request.borrow_mut() = Some(request);
        if self.ready.replace(1) != 1 {
            self.fire("readystatechange", false, 0);
        }
        Ok(())
    }
    fn clear_response(&self, s: &Scope<'_>) -> JsResult<()> {
        if let Some(wire) = self.wire.take() {
            wire.lock().unwrap().close();
        }
        self.head.take();
        self.text.take();
        self.reported.set(0);
        s.set_hidden(self.wrapper(s)?, RESPONSE, s.undefined())
    }
    fn current(&self, generation: u64, id: RequestId) -> bool {
        self.generation.get() == generation && self.active.get() == Some(id) && !self.disposed.get()
    }
    fn send(self: &Rc<Self>, s: &Scope<'_>, body: Value<'_>) -> JsResult<()> {
        self.can_send()?;
        let generation = self.generation.get();
        let mut request = self.request.borrow().as_ref().unwrap().clone();
        if !matches!(request.method, Method::GET | Method::HEAD) && !body.is_null() && !body.is_undefined() {
            let (bytes, mime) = if let Some(blob) = s.unwrap::<crate::file::Blob>(body) {
                (blob.bytes.clone(), blob.mime.clone())
            } else if let Some(params) = s.unwrap::<crate::url_search_params::URLSearchParams>(body) {
                (Bytes::from(params.serialize()), "application/x-www-form-urlencoded;charset=UTF-8".into())
            } else if let Some(bytes) = crate::encoding::buffer_source_bytes(s, body)? {
                (Bytes::from(bytes), String::new())
            } else {
                (Bytes::from(dom_string(body)?), "text/plain;charset=UTF-8".into())
            };
            if !mime.is_empty() && !request.headers.contains_key(header::CONTENT_TYPE) {
                request.headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(&mime).map_err(|_| JsError::Type("Invalid body MIME type".into()))?,
                );
            }
            request.body = Some(bytes);
        }
        self.can_send()?;
        if self.generation.get() != generation {
            return Err(invalid("XHR was reopened during body conversion"));
        }
        let page = self.page()?;
        let limits = page.xhr_state.limits.get();
        if page.xhr_state.jobs.borrow().len() >= limits.max_requests {
            return Err(JsError::dom("QuotaExceededError", "Too many pending XHR requests"));
        }
        let net = page.net.borrow().clone().ok_or_else(|| unsupported("Page networking is not installed"))?;
        let poster = page.remote_poster().ok_or_else(|| unsupported("Browser task poster is not installed"))?;
        if self.timeout.get() != 0 && !page.xhr_state.timers.borrow().contains_key(&self.realm_id) {
            return Err(unsupported("Browser timers are not installed"));
        }
        request.credentials = if self.with_credentials.get() { Credentials::Include } else { Credentials::SameOrigin };
        let doc_url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
        request.origin = Some(s.realm().origin().map(str::to_owned).unwrap_or_else(|| {
            lp_net::Url::parse(&doc_url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into())
        }));
        let context = page.request_context();
        request.frame_id = context.frame_id;
        request.loader_id = context.loader_id;
        page.apply_request_defaults(&mut request);
        request.max_body = Some(limits.max_body_bytes);
        let id = net.next_request_id();
        let wire = Arc::new(Mutex::new(Delivery::new(page.xhr_state.budget.clone())));
        *self.wire.borrow_mut() = Some(wire.clone());
        self.active.set(Some(id));
        self.sent.set(true);
        self.started.set(Some(Instant::now()));
        page.xhr_state
            .jobs
            .borrow_mut()
            .insert(id, Rc::new(Pending { xhr: self.clone(), _wrapper: Persistent::new(s, self.wrapper(s)?.value()) }));
        PAGES.with(|p| p.borrow_mut().insert(page.id(), Rc::downgrade(&page)));
        if let Err(error) = self.schedule_timer(s) {
            self.stop();
            return Err(error);
        }
        self.fire("loadstart", true, 0);
        if !self.current(generation, id) {
            return Ok(());
        }
        let page_id = page.id();
        net.fetch_with_id(
            id,
            request,
            Box::new(move |event| delivery::receive(&wire, page_id, id, &poster, limits, event)),
        );
        Ok(())
    }
    fn fire(&self, ty: &str, progress: bool, loaded: usize) {
        let Ok(page) = self.page() else { return };
        let event = EventCore::trusted(
            &page,
            if progress { EventInterface::ProgressEvent } else { EventInterface::Event },
            ty,
            false,
            false,
        );
        if progress {
            let total = self
                .head
                .borrow()
                .as_ref()
                .and_then(|h| h.headers.get(header::CONTENT_LENGTH))
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let mut fields = event.fields.borrow_mut();
            fields.loaded = loaded as f64;
            fields.total = total as f64;
            fields.length_computable = total > 0;
        }
        crate::target::fire(&page, self.key, &event);
    }
    fn stop(&self) -> Option<Rc<Pending>> {
        let mut keep = None;
        self.sent.set(false);
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                self.clear_timer(&s);
            }
        }
        if let Some(id) = self.active.take() {
            if let Some(page) = self.page.upgrade() {
                if let Some(net) = page.net.borrow().as_ref() {
                    net.cancel(id);
                }
                keep = page.xhr_state.jobs.borrow_mut().remove(&id);
            }
        }
        if let Some(wire) = self.wire.borrow().as_ref() {
            wire.lock().unwrap().close();
        }
        keep
    }
    fn fail(&self, kind: &str) {
        if self.active.get().is_none() {
            return;
        }
        let generation = self.generation.get();
        let _keep = self.stop();
        self.head.take();
        self.text.take();
        self.ready.set(4);
        self.fire("readystatechange", false, 0);
        if self.generation.get() != generation {
            return;
        }
        self.fire(kind, true, 0);
        if self.generation.get() != generation {
            return;
        }
        self.fire("loadend", true, 0);
    }
    fn abort(&self) {
        let generation = self.generation.get();
        if self.sent.get() {
            self.fail("abort");
        }
        if self.generation.get() == generation && self.ready.get() == 4 {
            self.ready.set(0);
            self.head.take();
            if let Some(w) = self.wire.take() {
                w.lock().unwrap().close();
            }
            self.text.take();
            self.release_cached_response();
        }
    }
    fn release_cached_response(&self) {
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                if let Ok(wrapper) = self.wrapper(&s) {
                    let _ = s.set_hidden(wrapper, RESPONSE, s.undefined());
                }
            }
        }
    }
    fn timer_functions<'s>(&self, s: &Scope<'s>) -> Option<Object<'s>> {
        self.page.upgrade()?.xhr_state.timers.borrow().get(&self.realm_id)?.get(s).as_object()
    }
    fn clear_timer(&self, s: &Scope<'_>) {
        if let Some(id) = self.timer.take() {
            if let Some(functions) = self.timer_functions(s) {
                if let Ok(Some(clear)) = functions.get("clear").map(|v| v.as_object()) {
                    let _ = clear.call(Some(s.global()), &[s.number(id)]);
                }
            }
        }
    }
    fn schedule_timer(self: &Rc<Self>, s: &Scope<'_>) -> JsResult<()> {
        self.clear_timer(s);
        if !self.sent.get() || self.timeout.get() == 0 {
            return Ok(());
        }
        let funcs = self.timer_functions(s).ok_or_else(|| unsupported("Browser timers are not installed"))?;
        let remaining = Duration::from_millis(self.timeout.get() as u64)
            .saturating_sub(self.started.get().map_or(Duration::ZERO, |t| t.elapsed()));
        let weak = Rc::downgrade(self);
        let generation = self.generation.get();
        let id = self.active.get();
        let callback = s.function("xhrTimeout", move |s, _, _| {
            if let Some(xhr) = weak.upgrade() {
                if xhr.generation.get() == generation && xhr.active.get() == id && xhr.sent.get() {
                    xhr.fail("timeout");
                }
            }
            Ok(s.undefined())
        })?;
        let timer = funcs
            .get("set")?
            .as_object()
            .unwrap()
            .call(Some(s.global()), &[callback.value(), s.number(remaining.as_secs_f64() * 1000.)])?
            .to_number()?;
        self.timer.set(Some(timer));
        Ok(())
    }
    fn dispose(&self) {
        self.stop();
        self.disposed.set(true);
        self.ready.set(0);
        self.head.take();
        self.text.take();
        self.wire.take();
        self.release_cached_response();
    }
}

pub(crate) fn install(s: &Scope<'_>, page: &PageDom) -> JsResult<()> {
    let set = s.global().get("setTimeout")?;
    let clear = s.global().get("clearTimeout")?;
    if set.as_object().is_some_and(|o| o.is_function()) && clear.as_object().is_some_and(|o| o.is_function()) {
        let object = s.object();
        object.set("set", set)?;
        object.set("clear", clear)?;
        page.xhr_state.timers.borrow_mut().insert(s.realm_id(), Persistent::new(s, object.value()));
    }
    Ok(())
}
pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    let instances: Vec<_> =
        page.xhr_state.instances.borrow().values().filter_map(Weak::upgrade).filter(|x| x.realm_id == realm).collect();
    for xhr in instances {
        xhr.dispose();
    }
    page.xhr_state.timers.borrow_mut().remove(&realm);
}
pub(crate) fn dispose(page: &PageDom) {
    PAGES.with(|p| p.borrow_mut().remove(&page.id()));
    let instances: Vec<_> = page.xhr_state.instances.borrow().values().filter_map(Weak::upgrade).collect();
    for xhr in instances {
        xhr.dispose();
    }
    page.xhr_state.timers.borrow_mut().clear();
}
