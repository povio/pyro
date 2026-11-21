//! Fetch `Request`: script-created requests (`new Request`) with complete
//! in-memory bodies, which `fetch` accepts as its input.
use crate::{
    fetch::{Body, ReadKind},
    headers::{Headers, HeadersData},
    response::{BodySource, StreamData},
};
use bytes::Bytes;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value, WeakRealm};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
const HEADERS: &str = "__lpRequestHeaders";
const SIGNAL: &str = "__lpRequestSignal";
const STREAM: &str = "__lpRequestStream";

pub struct RequestData {
    id: IdentityKey,
    realm: WeakRealm,
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) mode: String,
    pub(crate) credentials: String,
    pub(crate) cache: String,
    pub(crate) redirect: String,
    /// "no-referrer", "client" or a URL.
    pub(crate) referrer: String,
    pub(crate) referrer_policy: String,
    pub(crate) integrity: String,
    pub(crate) keepalive: bool,
    /// Set only on a service worker's FetchEvent request; "" otherwise.
    pub(crate) destination: String,
    pub(crate) body: Rc<Body>,
    used: Rc<Cell<bool>>,
    /// `body` was read: its source holds this Request's body consumer.
    source_made: Cell<bool>,
}
impl Drop for RequestData {
    fn drop(&mut self) {
        if !self.used.get() && !self.source_made.get() {
            self.body.release_consumer();
        }
    }
}

impl RequestData {
    /// The body bytes for `fetch(request)`, marking the body used; `None`
    /// for no body. Errors when it was already consumed.
    pub(crate) fn take_body(&self) -> JsResult<Option<Bytes>> {
        if self.body.is_null() {
            return Ok(None);
        }
        if self.used.replace(true) {
            return Err(JsError::Type("Request body has already been consumed".into()));
        }
        Ok(Some(self.body.constructed_bytes()))
    }
    pub(crate) fn header_pairs(&self, s: &Scope<'_>) -> Vec<(String, String)> {
        hidden(s, self, HEADERS).ok().and_then(|h| s.unwrap::<Headers>(h).map(|h| h.pairs())).unwrap_or_default()
    }
    pub(crate) fn signal<'s>(&self, s: &Scope<'s>) -> Option<Value<'s>> {
        hidden(s, self, SIGNAL).ok().filter(|v| !v.is_undefined() && !v.is_null())
    }
}

const METHODS: &[&str] = &["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"];
/// RequestInit `referrer`: `""` is `no-referrer`; `about:client` or a URL of
/// another origin than the document's is `client`; else the parsed URL.
pub(crate) fn parse_referrer(page: &crate::page::PageDom, referrer: &str) -> Result<String, String> {
    if referrer.is_empty() {
        return Ok("no-referrer".into());
    }
    let dom = page.dom.borrow();
    let base = dom.document_base_url(page.document()).ok().and_then(|b| pyro_net::Url::parse(b).ok());
    let parsed = pyro_net::Url::options()
        .base_url(base.as_ref())
        .parse(referrer)
        .map_err(|_| format!("Invalid referrer '{referrer}'"))?;
    drop(dom);
    Ok(
        if (parsed.scheme() == "about" && parsed.path() == "client")
            || page.origin_key() != parsed.origin().ascii_serialization()
        {
            "client".into()
        } else {
            parsed.to_string()
        },
    )
}

/// A RequestInit `referrerPolicy` must be a ReferrerPolicy enum value.
pub(crate) fn check_referrer_policy(policy: &str) -> Result<(), String> {
    if REFERRER_POLICIES.contains(&policy) {
        Ok(())
    } else {
        Err(format!("The provided value '{policy}' is not a valid enum value of type ReferrerPolicy."))
    }
}

const REFERRER_POLICIES: &[&str] = &[
    "",
    "no-referrer",
    "no-referrer-when-downgrade",
    "same-origin",
    "origin",
    "strict-origin",
    "origin-when-cross-origin",
    "strict-origin-when-cross-origin",
    "unsafe-url",
];

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn option<'s>(s: &Scope<'s>, init: Value<'s>, key: &str) -> JsResult<Value<'s>> {
    match init.as_object() {
        Some(o) => o.get(key),
        None if init.is_null() || init.is_undefined() => Ok(s.undefined()),
        _ => Err(JsError::Type("Failed to construct 'Request': init must be an object".into())),
    }
}

fn string_option<'s>(s: &Scope<'s>, init: Value<'s>, key: &str) -> JsResult<Option<String>> {
    let v = option(s, init, key)?;
    if v.is_undefined() {
        Ok(None)
    } else {
        crate::util::dom_string(v).map(Some)
    }
}

pub struct Request;
impl JsClass for Request {
    const NAME: &'static str = "Request";
    type Data = RequestData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            a.require(1, "Request")?;
            let v = construct(s, Some(nt), a.get(0), a.get(1))?;
            Ok(v.as_object().expect("Request wrapper"))
        });
        def.getter("method", |s, d| Ok(s.string(&d.method)));
        def.getter("url", |s, d| Ok(s.string(&d.url)));
        def.getter("headers", |s, d| hidden(s, d, HEADERS));
        def.getter("destination", |s, d| Ok(s.string(&d.destination)));
        def.getter("referrer", |s, d| {
            Ok(s.string(match d.referrer.as_str() {
                "no-referrer" => "",
                "client" => "about:client",
                url => url,
            }))
        });
        def.getter("referrerPolicy", |s, d| Ok(s.string(&d.referrer_policy)));
        def.getter("mode", |s, d| Ok(s.string(&d.mode)));
        def.getter("credentials", |s, d| Ok(s.string(&d.credentials)));
        def.getter("cache", |s, d| Ok(s.string(&d.cache)));
        def.getter("redirect", |s, d| Ok(s.string(&d.redirect)));
        def.getter("integrity", |s, d| Ok(s.string(&d.integrity)));
        def.getter("keepalive", |s, d| Ok(s.boolean(d.keepalive)));
        def.getter("isReloadNavigation", |s, _d| Ok(s.boolean(false)));
        def.getter("isHistoryNavigation", |s, _d| Ok(s.boolean(false)));
        def.getter("signal", |s, d| hidden(s, d, SIGNAL));
        def.getter("duplex", |s, _d| Ok(s.string("half")));
        def.getter("body", |s, d| {
            if d.body.is_null() {
                return Ok(s.null());
            }
            let existing = hidden(s, d, STREAM)?;
            if !existing.is_undefined() {
                return Ok(existing);
            }
            let page = crate::target::page_of(s)?;
            let source = s.wrap::<BodySource>(StreamData::new(d.body.clone(), d.used.clone()), None)?;
            d.source_made.set(true);
            let read = source.get_atom("read")?;
            let cancel = source.get_atom("cancel")?;
            let stream = crate::streams::helper(s, &page, "fromBody", &[source.value(), read, cancel])?;
            let realm = d.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let owner = realm.scope()?;
            let wrapper = owner.lookup_identity(d.id).ok_or(JsError::ContextDestroyed)?;
            s.set_hidden(wrapper, STREAM, stream)?;
            Ok(stream)
        });
        def.getter("bodyUsed", |s, d| Ok(s.boolean(d.used.get() || stream_state(s, d)?.1)));
        def.method("text", 0, |s, d, _| consume(s, d, ReadKind::Text));
        def.method("json", 0, |s, d, _| consume(s, d, ReadKind::Json));
        def.method("arrayBuffer", 0, |s, d, _| consume(s, d, ReadKind::ArrayBuffer));
        def.method("bytes", 0, |s, d, _| consume(s, d, ReadKind::Bytes));
        def.method("blob", 0, |s, d, _| consume(s, d, ReadKind::Blob));
        def.method("formData", 0, |s, d, _| consume(s, d, ReadKind::FormData));
        def.method("clone", 0, |s, d, _| {
            let (locked, disturbed) = stream_state(s, d)?;
            if d.used.get() || locked || disturbed {
                return Err(JsError::Type("Request body is already used".into()));
            }
            let bytes = if d.body.is_null() { None } else { Some(d.body.constructed_bytes()) };
            let headers = HeadersData::from_pairs(d.header_pairs(s), false).guard_request(d.mode == "no-cors");
            let signal = d.signal(s);
            wrap(s, None, Fields::copy(d), headers, bytes, d.body.mime.borrow().clone(), signal)
        });
    }
}

/// The non-body fields of a request, as the constructor resolves them.
struct Fields {
    method: String,
    url: String,
    mode: String,
    credentials: String,
    cache: String,
    redirect: String,
    referrer: String,
    referrer_policy: String,
    integrity: String,
    keepalive: bool,
    destination: String,
}
impl Fields {
    fn copy(d: &RequestData) -> Fields {
        Fields {
            method: d.method.clone(),
            url: d.url.clone(),
            mode: d.mode.clone(),
            credentials: d.credentials.clone(),
            cache: d.cache.clone(),
            redirect: d.redirect.clone(),
            referrer: d.referrer.clone(),
            referrer_policy: d.referrer_policy.clone(),
            integrity: d.integrity.clone(),
            keepalive: d.keepalive,
            destination: d.destination.clone(),
        }
    }
}

/// `new Request(input, init)`.
fn construct<'s>(
    s: &Scope<'s>,
    new_target: Option<Object<'s>>,
    input: Value<'s>,
    init: Value<'s>,
) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let input_request = s.unwrap::<Request>(input);
    let has_init = init.as_object().is_some_and(|o| {
        [
            "method",
            "headers",
            "body",
            "referrer",
            "referrerPolicy",
            "mode",
            "credentials",
            "cache",
            "redirect",
            "integrity",
            "keepalive",
            "signal",
            "duplex",
            "priority",
            "window",
        ]
        .iter()
        .any(|k| o.get(k).is_ok_and(|v| !v.is_undefined()))
    });
    let (mut fields, mut headers, mut body_bytes, mut body_mime, mut signal) = match &input_request {
        Some(r) => (Fields::copy(r), r.header_pairs(s), None::<Bytes>, r.body.mime.borrow().clone(), r.signal(s)),
        None => {
            let input = crate::util::dom_string(input)?;
            let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), &input);
            let mut url = pyro_net::Url::parse(&resolved)
                .map_err(|_| JsError::Type(format!("Failed to construct 'Request': Invalid URL '{input}'").into()))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err(JsError::Type("Failed to construct 'Request': Request cannot be constructed from a URL that includes credentials.".into()));
            }
            url.set_fragment(None);
            (
                Fields {
                    method: "GET".into(),
                    url: url.to_string(),
                    mode: "cors".into(),
                    credentials: "same-origin".into(),
                    cache: "default".into(),
                    redirect: "follow".into(),
                    referrer: "client".into(),
                    referrer_policy: String::new(),
                    integrity: String::new(),
                    keepalive: false,
                    destination: String::new(),
                },
                Vec::new(),
                None,
                String::new(),
                None,
            )
        }
    };
    let window_option = option(s, init, "window")?;
    if !window_option.is_undefined() && !window_option.is_null() {
        return Err(JsError::Type("Failed to construct 'Request': 'window' member must be null".into()));
    }
    if has_init && fields.mode == "navigate" {
        fields.mode = "same-origin".into();
    }
    if let Some(referrer) = string_option(s, init, "referrer")? {
        fields.referrer = parse_referrer(&page, &referrer)
            .map_err(|e| JsError::Type(format!("Failed to construct 'Request': {e}").into()))?;
    }
    if let Some(policy) = string_option(s, init, "referrerPolicy")? {
        check_referrer_policy(&policy).map_err(|e| JsError::Type(format!("Failed to construct 'Request': {e}").into()))?;
        fields.referrer_policy = policy;
    }
    if let Some(mode) = string_option(s, init, "mode")? {
        if mode == "navigate" {
            return Err(JsError::Type("Failed to construct 'Request': Cannot construct a Request with a RequestInit whose mode member is set as 'navigate'.".into()));
        }
        if !matches!(mode.as_str(), "same-origin" | "no-cors" | "cors") {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': The provided value '{mode}' is not a valid enum value of type RequestMode."
            ).into()));
        }
        fields.mode = mode;
    }
    if let Some(credentials) = string_option(s, init, "credentials")? {
        if !matches!(credentials.as_str(), "omit" | "same-origin" | "include") {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': The provided value '{credentials}' is not a valid enum value of type RequestCredentials."
            ).into()));
        }
        fields.credentials = credentials;
    }
    if let Some(cache) = string_option(s, init, "cache")? {
        if !matches!(cache.as_str(), "default" | "no-store" | "reload" | "no-cache" | "force-cache" | "only-if-cached")
        {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': The provided value '{cache}' is not a valid enum value of type RequestCache."
            ).into()));
        }
        fields.cache = cache;
    }
    if fields.cache == "only-if-cached" && fields.mode != "same-origin" {
        return Err(JsError::Type(
            "Failed to construct 'Request': 'only-if-cached' can be set only with 'same-origin' mode".into(),
        ));
    }
    if let Some(redirect) = string_option(s, init, "redirect")? {
        if !matches!(redirect.as_str(), "follow" | "error" | "manual") {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': The provided value '{redirect}' is not a valid enum value of type RequestRedirect."
            ).into()));
        }
        fields.redirect = redirect;
    }
    if let Some(integrity) = string_option(s, init, "integrity")? {
        fields.integrity = integrity;
    }
    let keepalive = option(s, init, "keepalive")?;
    if !keepalive.is_undefined() {
        fields.keepalive = keepalive.to_bool();
    }
    if let Some(method) = string_option(s, init, "method")? {
        if !is_token(&method) {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': '{method}' is not a valid HTTP method."
            ).into()));
        }
        let upper = method.to_ascii_uppercase();
        if matches!(upper.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Err(JsError::Type(format!(
                "Failed to construct 'Request': '{method}' HTTP method is unsupported."
            ).into()));
        }
        fields.method = if METHODS.contains(&upper.as_str()) { upper } else { method };
    }
    let signal_option = option(s, init, "signal")?;
    if !signal_option.is_undefined() {
        if signal_option.is_null() {
            signal = None;
        } else {
            if s.unwrap::<crate::abort::AbortSignal>(signal_option).is_none() {
                return Err(JsError::Type(
                    "Failed to construct 'Request': member signal is not of type AbortSignal.".into(),
                ));
            }
            signal = Some(signal_option);
        }
    }
    let headers_option = option(s, init, "headers")?;
    if !headers_option.is_undefined() {
        headers = crate::headers::from_init(s, headers_option)?.pairs();
    }
    // The "request" guard: forbidden request headers are dropped; "no-cors"
    // keeps only CORS-safelisted ones.
    headers.retain(|(name, value)| !crate::fetch::forbidden_request_header(name, value));
    if fields.mode == "no-cors" {
        headers.retain(|(name, value)| crate::headers::cors_safelisted(name, value));
    }
    let body_option = option(s, init, "body")?;
    if !body_option.is_undefined() && !body_option.is_null() {
        if matches!(fields.method.as_str(), "GET" | "HEAD") {
            return Err(JsError::Type(
                "Failed to construct 'Request': Request with GET/HEAD method cannot have body.".into(),
            ));
        }
        if crate::streams::is_stream(s, body_option)? {
            return Err(JsError::Type(
                "Failed to construct 'Request': ReadableStream bodies are not supported.".into(),
            ));
        }
        let (bytes, mime) = crate::fetch::extract_body(s, body_option)?;
        if !mime.is_empty() && !headers.iter().any(|(n, _)| n == "content-type") {
            headers.push(("content-type".into(), mime.clone()));
        }
        body_bytes = Some(bytes);
        body_mime = mime;
    } else if let Some(r) = &input_request {
        // The input's body moves to the new request.
        if !r.body.is_null() {
            if matches!(fields.method.as_str(), "GET" | "HEAD") {
                return Err(JsError::Type(
                    "Failed to construct 'Request': Request with GET/HEAD method cannot have body.".into(),
                ));
            }
            body_bytes = Some(r.take_body()?.unwrap_or_default());
        }
    }
    // The body's MIME type is the request's Content-Type header.
    if let Some((_, ct)) = headers.iter().find(|(n, _)| n == "content-type") {
        body_mime = ct.clone();
    }
    let headers = HeadersData::from_pairs(headers, false).guard_request(fields.mode == "no-cors");
    wrap(s, new_target, fields, headers, body_bytes, body_mime, signal)
}

/// A service worker's view of an intercepted request (FetchEvent `request`):
/// its fields as the client's fetch resolved them, `navigate` mode and a
/// cross-origin referrer included, which `new Request` would refuse.
pub(crate) struct Intercepted {
    pub method: String,
    pub url: String,
    pub mode: &'static str,
    pub credentials: &'static str,
    pub cache: &'static str,
    pub redirect: &'static str,
    pub referrer: String,
    pub referrer_policy: String,
    pub integrity: String,
    pub keepalive: bool,
    pub destination: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

pub(crate) fn intercepted<'s>(s: &Scope<'s>, r: Intercepted) -> JsResult<Value<'s>> {
    let mime = r.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone());
    let headers = HeadersData::from_pairs(r.headers, false).guard_request(r.mode == "no-cors");
    let fields = Fields {
        method: r.method,
        url: r.url,
        mode: r.mode.into(),
        credentials: r.credentials.into(),
        cache: r.cache.into(),
        redirect: r.redirect.into(),
        referrer: r.referrer,
        referrer_policy: r.referrer_policy,
        integrity: r.integrity,
        keepalive: r.keepalive,
        destination: r.destination.into(),
    };
    wrap(s, None, fields, headers, r.body, mime.unwrap_or_default(), None)
}

fn wrap<'s>(
    s: &Scope<'s>,
    new_target: Option<Object<'s>>,
    fields: Fields,
    headers: HeadersData,
    bytes: Option<Bytes>,
    mime: String,
    signal: Option<Value<'s>>,
) -> JsResult<Value<'s>> {
    let id = IdentityKey(1 << 58, NEXT_REQUEST.fetch_add(1, Ordering::Relaxed));
    let body = Body::constructed(s, bytes, mime);
    body.add_consumer();
    let data = RequestData {
        id,
        realm: s.realm().downgrade(),
        method: fields.method,
        url: fields.url,
        mode: fields.mode,
        credentials: fields.credentials,
        cache: fields.cache,
        redirect: fields.redirect,
        referrer: fields.referrer,
        referrer_policy: fields.referrer_policy,
        integrity: fields.integrity,
        keepalive: fields.keepalive,
        destination: fields.destination,
        body,
        used: Rc::new(Cell::new(false)),
        source_made: Cell::new(false),
    };
    let wrapper = match new_target {
        Some(nt) => s.wrap_new::<Request>(data, nt, Some(id))?,
        None => s.wrap::<Request>(data, Some(id))?,
    };
    let headers = s.wrap::<Headers>(headers, None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    let signal = match signal {
        Some(v) => v,
        None => crate::abort::new_signal(s)?.value(),
    };
    s.set_hidden(wrapper, SIGNAL, signal)?;
    Ok(wrapper.value())
}

fn hidden<'s>(s: &Scope<'s>, data: &RequestData, key: &'static str) -> JsResult<Value<'s>> {
    let realm = data.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let wrapper = owner.lookup_identity(data.id).ok_or(JsError::ContextDestroyed)?;
    Ok(s.get_hidden(wrapper, key)?.unwrap_or(s.undefined()))
}

fn stream_state(s: &Scope<'_>, data: &RequestData) -> JsResult<(bool, bool)> {
    let Ok(stream) = hidden(s, data, STREAM) else { return Ok((false, false)) };
    if stream.is_undefined() {
        return Ok((false, false));
    }
    let page = crate::target::page_of(s)?;
    Ok((
        crate::streams::helper(s, &page, "isLocked", &[stream])?.to_bool(),
        crate::streams::helper(s, &page, "isDisturbed", &[stream])?.to_bool(),
    ))
}

fn consume<'s>(s: &Scope<'s>, data: &RequestData, kind: ReadKind) -> JsResult<Value<'s>> {
    let (locked, disturbed) = stream_state(s, data)?;
    if locked || disturbed {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        deferred.reject(s, JsError::Type("Request body has already been consumed".into()).to_value(s))?;
        return Ok(promise);
    }
    let used = if data.body.is_null() { false } else { data.used.replace(true) };
    let headers = if matches!(kind, ReadKind::Blob | ReadKind::FormData) {
        hidden(s, data, HEADERS).ok().and_then(|v| s.unwrap::<Headers>(v).map(|h| h.clone()))
    } else {
        None
    };
    data.body.read(s, kind, used, headers)
}
