//! Fetch `Response`: network responses created by `fetch`, and script-created
//! responses (`new Response`, `Response.error/redirect/json`) with complete
//! in-memory bodies. `response.body` is a byte ReadableStream (streams.js)
//! over the fetch body, delivering bytes as they arrive (see [`BodySource`]).
//! A body given as a ReadableStream stays one; consuming it drains it in JS.
use crate::{
    fetch::{Body, ReadKind},
    headers::{Headers, HeadersData},
};
use bytes::Bytes;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value, WeakRealm};
use pyro_net::{Credentials, ResponseHead, ResponseKind};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RESPONSE: AtomicU64 = AtomicU64::new(1);
const HEADERS: &str = "__lpResponseHeaders";
const SIGNAL: &str = "__lpResponseSignal";
const STREAM: &str = "__lpResponseStream";
/// Where a Response came from; drives `type`/`url`/`status` getters.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Origin {
    Network,
    /// `new Response()`, `Response.json`, `Response.redirect`: type "default".
    Script,
    /// `Response.error()`: type "error", status 0.
    Error,
}

pub struct ResponseData {
    origin: Origin,
    id: IdentityKey,
    realm: WeakRealm,
    body: Rc<Body>,
    used: Rc<Cell<bool>>,
    head: ResponseHead,
    credentials: Credentials,
    /// The body is a script's ReadableStream, kept in the STREAM property.
    script_stream: Cell<bool>,
    /// `body` was read: its source holds this Response's body consumer, since
    /// the stream can outlive the Response.
    source_made: Cell<bool>,
}
impl Drop for ResponseData {
    fn drop(&mut self) {
        if !self.used.get() && !self.source_made.get() {
            self.body.release_consumer();
        }
    }
}
pub struct Response;
impl JsClass for Response {
    const NAME: &'static str = "Response";
    type Data = ResponseData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let init = a.get(1);
            let (status, status_text, headers) = parse_init(s, init)?;
            let body = a.get(0);
            let body = if body.is_null() || body.is_undefined() { None } else { Some(body) };
            if body.is_some() && is_null_body_status(status) {
                return Err(JsError::Type(format!("Response with null body status {status} cannot have a body").into()));
            }
            if let Some(stream) = body.filter(|b| crate::streams::is_stream(s, *b).unwrap_or(false)) {
                let page = crate::target::page_of(s)?;
                if crate::streams::helper(s, &page, "isLocked", &[stream])?.to_bool()
                    || crate::streams::helper(s, &page, "isDisturbed", &[stream])?.to_bool()
                {
                    return Err(JsError::Type("Response body object should not be disturbed or locked".into()));
                }
                let v = construct(s, Some(nt), Origin::Script, status, status_text, headers, Some(Bytes::new()))?;
                return adopt_stream(s, v, stream);
            }
            let (bytes, mime) = match body {
                Some(b) => {
                    let (bytes, mime) = crate::fetch::extract_body(s, b)?;
                    (Some(bytes), mime)
                }
                None => (None, String::new()),
            };
            if !mime.is_empty() && !headers.contains("content-type") {
                headers.append_raw("content-type", &mime);
            }
            let v = construct(s, Some(nt), Origin::Script, status, status_text, headers, bytes)?;
            Ok(v.as_object().expect("wrapper object"))
        });
        def.static_method("error", 0, |s, _a| {
            let headers = HeadersData::from_pairs(std::iter::empty(), true);
            construct(s, None, Origin::Error, 0, String::new(), headers, None)
        });
        def.static_method("redirect", 1, |s, a| {
            a.require(1, "Response.redirect")?;
            let target = crate::util::dom_string(a.get(0))?;
            let url = resolve(s, &target)?;
            let status = if a.get(1).is_undefined() { 302 } else { crate::util::to_u32(a.get(1))? as u16 };
            if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                return Err(JsError::Range(format!("Invalid redirect status {status}").into()));
            }
            let headers = HeadersData::from_pairs([("location".to_owned(), url)], true);
            construct(s, None, Origin::Script, status, String::new(), headers, None)
        });
        def.static_method("json", 1, |s, a| {
            let json = a.get(0).to_json(0)?.ok_or_else(|| JsError::Type("Value is not JSON serializable".into()))?;
            let (status, status_text, headers) = parse_init(s, a.get(1))?;
            if is_null_body_status(status) {
                return Err(JsError::Type(format!("Response with null body status {status} cannot have a body").into()));
            }
            if !headers.contains("content-type") {
                headers.append_raw("content-type", "application/json");
            }
            construct(s, None, Origin::Script, status, status_text, headers, Some(Bytes::from(json)))
        });
        def.getter("url", |s, d| {
            // An opaque filtered response has an empty URL list; an opaque-redirect one keeps it.
            let empty = d.origin != Origin::Network || d.head.kind == ResponseKind::Opaque;
            Ok(s.string(if empty { "" } else { d.head.url.as_str() }))
        });
        def.getter("status", |s, d| Ok(s.number(f64::from(status_of(d)))));
        def.getter("statusText", |s, d| {
            Ok(s.string(if d.origin == Origin::Error || opaque(d.head.kind) { "" } else { &d.head.status_text }))
        });
        def.getter("ok", |s, d| Ok(s.boolean((200..300).contains(&status_of(d)))));
        def.getter("redirected", |s, d| {
            Ok(s.boolean(d.origin == Origin::Network && !opaque(d.head.kind) && d.head.redirected))
        });
        def.getter("type", |s, d| {
            Ok(s.string(match (d.origin, d.head.kind) {
                (Origin::Script, _) => "default",
                (Origin::Error, _) => "error",
                (Origin::Network, ResponseKind::Basic) => "basic",
                (Origin::Network, ResponseKind::Cors) => "cors",
                (Origin::Network, ResponseKind::Opaque) => "opaque",
                (Origin::Network, ResponseKind::OpaqueRedirect) => "opaqueredirect",
            }))
        });
        def.getter("body", |s, d| body_stream(s, d));
        def.getter("headers", |s, d| hidden(s, d, HEADERS));
        def.getter("bodyUsed", |s, d| Ok(s.boolean(d.used.get() || stream_state(s, d)?.1)));
        def.promise_method("text", 0, |s, d, _| consume(s, d, ReadKind::Text));
        def.promise_method("json", 0, |s, d, _| consume(s, d, ReadKind::Json));
        def.promise_method("arrayBuffer", 0, |s, d, _| consume(s, d, ReadKind::ArrayBuffer));
        def.promise_method("bytes", 0, |s, d, _| consume(s, d, ReadKind::Bytes));
        def.promise_method("blob", 0, |s, d, _| consume(s, d, ReadKind::Blob));
        def.promise_method("formData", 0, |s, d, _| consume(s, d, ReadKind::FormData));
        def.method("textStream", 0, |s, d, _| {
            let (locked, disturbed) = stream_state(s, d)?;
            if d.used.get() || locked || disturbed {
                return Err(JsError::Type("Response body has already been consumed".into()));
            }
            let stream = body_stream(s, d)?;
            crate::fetch::text_stream(s, stream)
        });
        def.method("clone", 0, |s, d, _| {
            let (locked, disturbed) = stream_state(s, d)?;
            if d.used.get() || locked || disturbed {
                return Err(JsError::Type("Response body has already been consumed".into()));
            }
            if d.script_stream.get() {
                let page = crate::target::page_of(s)?;
                let clone_chunk = crate::structured_clone::realm_clone(s)?;
                let pair = crate::streams::helper(s, &page, "cloneBody", &[hidden(s, d, STREAM)?, clone_chunk.value()])?;
                let pair = pair.as_object().ok_or_else(|| JsError::Type("tee".into()))?;
                let realm = d.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
                let owner = realm.scope()?;
                let wrapper = owner.lookup_identity(d.id).ok_or(JsError::ContextDestroyed)?;
                s.set_hidden(wrapper, STREAM, pair.get_index(0)?)?;
                let current = hidden(s, d, HEADERS)?;
                let pairs = s.unwrap::<Headers>(current).map(|h| h.pairs()).unwrap_or_default();
                let headers = HeadersData::from_pairs(pairs, false);
                let clone = wrap_script(s, None, d.origin, d.head.clone(), d.body.clone(), headers)?;
                return adopt_stream(s, clone, pair.get_index(1)?).map(|o| o.value());
            }
            if d.origin != Origin::Network {
                // Script responses own mutable headers: the clone copies them.
                let current = hidden(s, d, HEADERS)?;
                let pairs = s.unwrap::<Headers>(current).map(|h| h.pairs()).unwrap_or_default();
                let headers = HeadersData::from_pairs(pairs, d.origin == Origin::Error);
                // Its own copy of the bytes: the two bodies' streams read
                // independently (one native body has one stream cursor).
                let body = if d.body.is_null() {
                    d.body.clone()
                } else {
                    Body::constructed(s, Some(d.body.constructed_bytes()), d.body.mime.borrow().clone())
                };
                return wrap_script(s, None, d.origin, d.head.clone(), body, headers);
            }
            let signal = hidden(s, d, SIGNAL)?;
            wrap(s, &d.head, d.body.clone(), d.credentials, Some(signal))
        });
    }
}

/// `response.body`: null, or the one stream this Response hands out (made on
/// first use over the native body).
fn body_stream<'s>(s: &Scope<'s>, d: &ResponseData) -> JsResult<Value<'s>> {
    if d.body.is_null() {
        return Ok(s.null());
    }
    let existing = hidden(s, d, STREAM)?;
    if !existing.is_undefined() {
        return Ok(existing);
    }
    let page = crate::target::page_of(s)?;
    let source = s.wrap::<BodySource>(
        StreamData { body: d.body.clone(), used: d.used.clone(), opened: Cell::new(false) },
        None,
    )?;
    d.source_made.set(true);
    let read = source.get_atom("read")?;
    let cancel = source.get_atom("cancel")?;
    let stream = crate::streams::helper(s, &page, "fromBody", &[source.value(), read, cancel])?;
    if d.used.get() {
        // text()/json()/... already read the body: its stream is unusable.
        crate::streams::helper(s, &page, "markConsumed", &[stream])?;
    }
    let realm = d.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let wrapper = owner.lookup_identity(d.id).ok_or(JsError::ContextDestroyed)?;
    s.set_hidden(wrapper, STREAM, stream)?;
    Ok(stream)
}

fn status_of(d: &ResponseData) -> u16 {
    if d.origin == Origin::Error || (d.origin == Origin::Network && opaque(d.head.kind)) {
        0
    } else {
        d.head.status.as_u16()
    }
}

fn is_null_body_status(status: u16) -> bool {
    matches!(status, 101 | 103 | 204 | 205 | 304)
}

/// `ResponseInit`: status (200..=599, default 200), statusText (reason-phrase
/// characters only), headers. `set-cookie`/`set-cookie2` are dropped (the
/// "response" guard forbids them, as it does later writes: `wrap_script`).
fn parse_init(s: &Scope<'_>, init: Value<'_>) -> JsResult<(u16, String, HeadersData)> {
    let get = |key: &str| -> JsResult<Value<'_>> {
        match init.as_object() {
            Some(o) => o.get(key),
            None if init.is_nullish() => Ok(s.undefined()),
            None => Err(JsError::Type("ResponseInit must be an object".into())),
        }
    };
    let status = get("status")?;
    let status = if status.is_undefined() { 200 } else { crate::util::to_u32(status)? };
    if !(200..=599).contains(&status) {
        return Err(JsError::Range(format!(
            "Failed to construct 'Response': The status provided ({status}) is outside the range [200, 599]."
        ).into()));
    }
    let text = get("statusText")?;
    let status_text = if text.is_undefined() { String::new() } else { crate::headers::byte_string(text)? };
    if status_text.chars().any(|c| c == '\r' || c == '\n' || (c < ' ' && c != '\t') || c == '\u{7f}') {
        return Err(JsError::Type("Failed to construct 'Response': Invalid statusText".into()));
    }
    let headers_init = get("headers")?;
    let headers = crate::headers::from_init(s, headers_init)?;
    headers.remove("set-cookie");
    headers.remove("set-cookie2");
    Ok((status as u16, status_text, headers))
}

fn resolve(s: &Scope<'_>, target: &str) -> JsResult<String> {
    let base = crate::target::page_of(s)
        .ok()
        .and_then(|p| p.dom.borrow().document_base_url(p.document()).ok().map(str::to_owned));
    let parsed = match base.as_deref().and_then(|b| pyro_net::Url::parse(b).ok()) {
        Some(b) => b.join(target),
        None => pyro_net::Url::parse(target),
    };
    parsed
        .map(|u| u.to_string())
        .map_err(|_| JsError::Type(format!("Failed to execute 'redirect' on 'Response': Invalid URL '{target}'").into()))
}

fn construct<'s>(
    s: &Scope<'s>,
    new_target: Option<pyro_js::Object<'s>>,
    origin: Origin,
    status: u16,
    status_text: String,
    headers: HeadersData,
    bytes: Option<Bytes>,
) -> JsResult<Value<'s>> {
    let mime = headers.pairs().into_iter().find(|(n, _)| n == "content-type").map(|(_, v)| v).unwrap_or_default();
    let head = ResponseHead {
        url: pyro_net::Url::parse("about:blank").expect("static URL"),
        // Error responses keep 0 via `Origin::Error`; StatusCode needs a valid code.
        status: pyro_net::http::StatusCode::from_u16(status.max(200))
            .map_err(|_| JsError::Range("Invalid status".into()))?,
        status_text,
        headers: Default::default(),
        mime: mime.clone(),
        charset: None,
        redirected: false,
        from_cache: false,
        from_prefetch_cache: false,
        kind: ResponseKind::Basic,
        remote_addr: None,
        protocol: pyro_net::http::Version::HTTP_11,
        timing: Default::default(),
    };
    let body = crate::fetch::Body::constructed(s, bytes, mime);
    wrap_script(s, new_target, origin, head, body, headers)
}

fn wrap_script<'s>(
    s: &Scope<'s>,
    new_target: Option<pyro_js::Object<'s>>,
    origin: Origin,
    head: ResponseHead,
    body: Rc<Body>,
    headers: HeadersData,
) -> JsResult<Value<'s>> {
    let id = IdentityKey(1 << 57, NEXT_RESPONSE.fetch_add(1, Ordering::Relaxed));
    body.add_consumer();
    let data = ResponseData {
        origin,
        id,
        realm: s.realm().downgrade(),
        body,
        used: Rc::new(Cell::new(false)),
        head,
        credentials: Credentials::SameOrigin,
        script_stream: Cell::new(false),
        source_made: Cell::new(false),
    };
    let wrapper = match new_target {
        Some(nt) => s.wrap_new::<Response>(data, nt, Some(id))?,
        None => s.wrap::<Response>(data, Some(id))?,
    };
    let headers = if origin == Origin::Script { headers.guard_response() } else { headers };
    let headers = s.wrap::<Headers>(headers, None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    Ok(wrapper.value())
}

/// The status and headers of an opaque-redirect Response as the network gave them (script sees
/// status 0 and no headers): a `respondWith` of one answers a navigation, which follows the
/// redirect it names. `None` for any other Response.
pub(crate) fn internal_head(s: &Scope<'_>, response: Value<'_>) -> Option<(u16, Vec<(String, String)>)> {
    let data = s.unwrap::<Response>(response)?;
    if data.origin != Origin::Network || data.head.kind != ResponseKind::OpaqueRedirect {
        return None;
    }
    let headers = data
        .head
        .headers
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    Some((data.head.status.as_u16(), headers))
}

fn opaque(kind: ResponseKind) -> bool {
    matches!(kind, ResponseKind::Opaque | ResponseKind::OpaqueRedirect)
}
fn hidden<'s>(s: &Scope<'s>, data: &ResponseData, key: &'static str) -> JsResult<Value<'s>> {
    // A method borrowed from another same-VM realm still addresses the original
    // wrapper's traced properties. Do not root the wrapper in its own payload.
    let realm = data.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let wrapper = owner.lookup_identity(data.id).ok_or(JsError::ContextDestroyed)?;
    Ok(s.get_hidden(wrapper, key)?.unwrap_or(s.undefined()))
}
/// Marks a just constructed Response as having the script stream as its body.
fn adopt_stream<'s>(s: &Scope<'s>, response: Value<'s>, stream: Value<'s>) -> JsResult<Object<'s>> {
    let wrapper = response.as_object().expect("wrapper object");
    let data = s.unwrap::<Response>(response).expect("Response wrapper");
    data.script_stream.set(true);
    s.set_hidden(wrapper, STREAM, stream)?;
    Ok(wrapper)
}
/// (locked, disturbed) of the body stream a script has seen, if any.
fn stream_state(s: &Scope<'_>, data: &ResponseData) -> JsResult<(bool, bool)> {
    // A Response of a closed realm has no wrapper to ask; its native reads
    // report that themselves.
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
fn consume<'s>(s: &Scope<'s>, data: &ResponseData, kind: ReadKind) -> JsResult<Value<'s>> {
    let (locked, disturbed) = stream_state(s, data)?;
    if locked || disturbed {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        deferred.reject(s, JsError::Type("Response body has already been consumed".into()).to_value(s))?;
        return Ok(promise);
    }
    let headers = if matches!(kind, ReadKind::Blob | ReadKind::FormData) {
        // As they are now: a Blob's type and FormData's MIME type come from the headers when the
        // method is called (Chrome), not from changes made while the body is read.
        hidden(s, data, HEADERS).ok().and_then(|v| s.unwrap::<Headers>(v).map(|h| HeadersData::from_pairs(h.pairs(), true)))
    } else {
        None
    };
    if data.script_stream.get() {
        let page = crate::target::page_of(s)?;
        let name = match kind {
            ReadKind::FormData | ReadKind::Blob => "package",
            ReadKind::Text => "text",
            ReadKind::Json => "json",
            // A script's stream has no pieces to share: its bytes, in a buffer.
            ReadKind::ArrayBuffer | ReadKind::Chunks => "arrayBuffer",
            ReadKind::Bytes => "bytes",
        };
        data.used.set(true);
        let package = if name == "package" {
            s.function("packageBody", move |s, _, args| {
                let bytes = crate::encoding::buffer_source_bytes(s, args.get(0))?
                    .ok_or_else(|| JsError::Type("Expected body bytes".into()))?;
                crate::fetch::package_body(s, kind, bytes, headers.as_ref())
            })?
            .value()
        } else {
            s.undefined()
        };
        let args = [hidden(s, data, STREAM)?, s.string(name), package];
        return crate::streams::helper(s, &page, "consume", &args);
    }
    let used = if data.body.is_null() { false } else { data.used.replace(true) };
    data.body.read(s, kind, used, headers)
}
pub(crate) fn wrap<'s>(
    s: &Scope<'s>,
    head: &ResponseHead,
    body: Rc<Body>,
    credentials: Credentials,
    signal: Option<Value<'s>>,
) -> JsResult<Value<'s>> {
    let id = IdentityKey(1 << 57, NEXT_RESPONSE.fetch_add(1, Ordering::Relaxed));
    body.add_consumer();
    let data = ResponseData {
        origin: Origin::Network,
        id,
        realm: s.realm().downgrade(),
        body,
        used: Rc::new(Cell::new(false)),
        head: head.clone(),
        credentials,
        script_stream: Cell::new(false),
        source_made: Cell::new(false),
    };
    let wrapper = s.wrap::<Response>(data, Some(id))?;
    let headers = s.wrap::<Headers>(filtered_headers(head, credentials), None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    if let Some(signal) = signal {
        s.set_hidden(wrapper, SIGNAL, signal)?;
    }
    Ok(wrapper.value())
}

/// Reads `response`'s body for `cache.put` (`cache_storage`): a promise of a
/// `StoredBody` holding the pieces the network delivered (an ArrayBuffer for a
/// script's stream), or null for a null body. Marks the body used, as
/// `arrayBuffer()` does.
pub(crate) fn read_for_cache<'s>(s: &Scope<'s>, response: Value<'_>) -> JsResult<Value<'s>> {
    let data = s.unwrap::<Response>(response).ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    if !data.script_stream.get() && data.body.is_null() {
        return Ok(s.null());
    }
    consume(s, &data, ReadKind::Chunks)
}

/// The checks WebAssembly's "compile a potential WebAssembly response" makes of `response`
/// before its bytes compile (WebAssembly Web API, for `compileStreaming` and
/// `instantiateStreaming`, named by `method`): a Response, whose `Content-Type` is
/// `application/wasm`, CORS-same-origin and of an ok status, else a TypeError. A promise of its
/// body as an ArrayBuffer, as `arrayBuffer()` gives it (rejected when the body was used).
pub(crate) fn read_for_wasm<'s>(s: &Scope<'s>, response: Value<'_>, method: Value<'_>) -> JsResult<Value<'s>> {
    let method = method.to_string()?;
    let fail = |reason: &str| JsError::Type(format!("Failed to execute '{method}' on 'WebAssembly': {reason}").into());
    let data = s.unwrap::<Response>(response).ok_or_else(|| fail("Argument 0 must be a Response or a promise of one"))?;
    let headers = hidden(s, &data, HEADERS).ok().and_then(|v| s.unwrap::<Headers>(v).map(|h| h.clone()));
    let content_type = headers
        .and_then(|h| h.pairs().into_iter().find(|(name, _)| name == "content-type"))
        .map(|(_, value)| value);
    // The header value with HTTP tabs and spaces trimmed, no parameters allowed.
    let wasm = content_type
        .as_deref()
        .is_some_and(|v| v.trim_matches(|c| c == ' ' || c == '\t').eq_ignore_ascii_case("application/wasm"));
    if !wasm {
        return Err(fail("Incorrect response MIME type. Expected 'application/wasm'."));
    }
    if data.origin == Origin::Network && opaque(data.head.kind) {
        return Err(fail("The response is not CORS-same-origin"));
    }
    if !(200..300).contains(&status_of(&data)) {
        return Err(fail("HTTP status code is not ok"));
    }
    consume(s, &data, ReadKind::ArrayBuffer)
}

/// A response read back from a cache (`cache_storage`): its stored type
/// ("default" for a script's, "error", or a network one's), URL, status,
/// status text, headers (as script saw them) and body, in a new object.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stored<'s>(
    s: &Scope<'s>,
    corp: Option<&str>,
    kind: &str,
    url: &str,
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    redirected: bool,
    body: Option<Vec<Bytes>>,
) -> JsResult<Value<'s>> {
    let (origin, kind) = match kind {
        "default" => (Origin::Script, ResponseKind::Basic),
        "error" => (Origin::Error, ResponseKind::Basic),
        "cors" => (Origin::Network, ResponseKind::Cors),
        "opaque" => (Origin::Network, ResponseKind::Opaque),
        "opaqueredirect" => (Origin::Network, ResponseKind::OpaqueRedirect),
        _ => (Origin::Network, ResponseKind::Basic),
    };
    let mime = headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone());
    let mut head = ResponseHead {
        url: pyro_net::Url::parse(url).unwrap_or_else(|_| pyro_net::Url::parse("about:blank").expect("static URL")),
        // Opaque and error responses read 0 through their kind and origin.
        status: pyro_net::http::StatusCode::from_u16(status).unwrap_or(pyro_net::http::StatusCode::OK),
        status_text,
        headers: Default::default(),
        mime: mime.clone().unwrap_or_default(),
        charset: None,
        redirected,
        from_cache: false,
        from_prefetch_cache: false,
        kind,
        remote_addr: None,
        protocol: pyro_net::http::Version::HTTP_11,
        timing: Default::default(),
    };
    // A filtered response hides its `Cross-Origin-Resource-Policy` from script, but the cache keeps it
    // for the embedder policy check of a later `match` (and of a `put` of the response again).
    if let Some(corp) = corp.and_then(|c| pyro_net::http::HeaderValue::from_str(c).ok()) {
        head.headers.insert(pyro_net::http::HeaderName::from_static("cross-origin-resource-policy"), corp);
    }
    let body = crate::fetch::Body::constructed_chunks(s, body, mime.unwrap_or_default());
    let headers = HeadersData::from_pairs(headers, origin != Origin::Script);
    wrap_script(s, None, origin, head, body, headers)
}

/// The `Cross-Origin-Resource-Policy` header `response` arrived with, which a filtered (opaque)
/// response hides from script: its value when it came once, else none (a repeated header is no
/// valid policy). For `cache.put`, which keeps it.
pub(crate) fn corp_header(response: Value<'_>, s: &Scope<'_>) -> Option<String> {
    let data = s.unwrap::<Response>(response)?;
    let mut values = data.head.headers.get_all("cross-origin-resource-policy").iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.to_str().ok().map(str::to_owned),
        _ => None,
    }
}

/// Where a response that came from the network really came from, and the
/// `Cross-Origin-Resource-Policy` it carried (see [`corp_header`]): an opaque response hides both
/// from script. `None` for a response a script made.
pub(crate) fn network_source(response: Value<'_>, s: &Scope<'_>) -> Option<(Option<String>, String)> {
    let data = s.unwrap::<Response>(response)?;
    (data.origin == Origin::Network).then(|| (corp_header(response, s), data.head.url.to_string()))
}

pub(crate) fn filtered_headers(head: &ResponseHead, credentials: Credentials) -> HeadersData {
    let exposed = head
        .headers
        .get_all(pyro_net::http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let wildcard = credentials != Credentials::Include && exposed.iter().any(|n| n == "*");
    let pairs = head
        .headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            if opaque(head.kind) || matches!(name, "set-cookie" | "set-cookie2") {
                return false;
            }
            head.kind != ResponseKind::Cors
                || wildcard
                || matches!(
                    name,
                    "cache-control"
                        | "content-language"
                        | "content-length"
                        | "content-type"
                        | "expires"
                        | "last-modified"
                        | "pragma"
                )
                || exposed.iter().any(|n| n == name)
        })
        .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().iter().map(|b| char::from(*b)).collect()));
    HeadersData::from_pairs(pairs, true)
}

/// The native side of `response.body`: streams.js wraps it in a byte
/// ReadableStream, pulling on each read. Never exposed to pages. The body
/// counts as used from the first read or cancel on.
pub struct StreamData {
    body: Rc<Body>,
    used: Rc<Cell<bool>>,
    /// The stream owns the body consumer from the first read on.
    opened: Cell<bool>,
}
impl StreamData {
    pub(crate) fn new(body: Rc<Body>, used: Rc<Cell<bool>>) -> StreamData {
        StreamData { body, used, opened: Cell::new(false) }
    }
    /// False when `text()` and the like already took the body.
    fn open(&self) -> bool {
        if self.opened.get() {
            return true;
        }
        if self.used.get() {
            return false;
        }
        self.opened.set(true);
        self.used.set(true);
        self.body.open_stream();
        true
    }
}
impl Drop for StreamData {
    fn drop(&mut self) {
        // Never read nor consumed: nothing else releases the consumer.
        if !self.used.get() {
            self.body.release_consumer();
        }
    }
}
pub struct BodySource;
impl JsClass for BodySource {
    const NAME: &'static str = "ResponseBodySource";
    type Data = StreamData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.method("read", 0, |s, d, _a| {
            d.open();
            d.body.stream_read(s)
        });
        def.method("cancel", 0, |s, d, _a| {
            if d.open() {
                d.body.cancel_consumer_stream();
            }
            let deferred = s.deferred()?;
            deferred.resolve(s, s.undefined())?;
            Ok(deferred.promise.get(s))
        });
    }
}
