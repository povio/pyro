//! Fetch `Response`: network responses created by `fetch`, and script-created
//! responses (`new Response`, `Response.error/redirect/json`) with complete
//! in-memory bodies. `response.body` streams are not implemented.
use crate::{
    fetch::{Body, ReadKind},
    headers::{Headers, HeadersData},
};
use bytes::Bytes;
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Scope, Value, WeakRealm};
use lp_net::{Credentials, ResponseHead, ResponseKind};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RESPONSE: AtomicU64 = AtomicU64::new(1);
const HEADERS: &str = "__lpResponseHeaders";
const SIGNAL: &str = "__lpResponseSignal";
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
    used: Cell<bool>,
    head: ResponseHead,
    credentials: Credentials,
}
impl Drop for ResponseData {
    fn drop(&mut self) {
        if !self.used.get() {
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
                return Err(JsError::Type(format!("Response with null body status {status} cannot have a body")));
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
                return Err(JsError::Range(format!("Invalid redirect status {status}")));
            }
            let headers = HeadersData::from_pairs([("location".to_owned(), url)], true);
            construct(s, None, Origin::Script, status, String::new(), headers, None)
        });
        def.static_method("json", 1, |s, a| {
            let json = a.get(0).to_json(0)?.ok_or_else(|| JsError::Type("Value is not JSON serializable".into()))?;
            let (status, status_text, headers) = parse_init(s, a.get(1))?;
            if is_null_body_status(status) {
                return Err(JsError::Type(format!("Response with null body status {status} cannot have a body")));
            }
            if !headers.contains("content-type") {
                headers.append_raw("content-type", "application/json");
            }
            construct(s, None, Origin::Script, status, status_text, headers, Some(Bytes::from(json)))
        });
        def.getter("url", |s, d| {
            Ok(s.string(if d.origin != Origin::Network || opaque(d.head.kind) { "" } else { d.head.url.as_str() }))
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
        def.getter("body", |s, d| {
            if d.body.is_null() {
                Ok(s.null())
            } else {
                Err(JsError::Type(
                    "Response.body streams are not implemented; use text()/json()/arrayBuffer()/blob()".into(),
                ))
            }
        });
        def.getter("headers", |s, d| hidden(s, d, HEADERS));
        def.getter("bodyUsed", |s, d| Ok(s.boolean(d.used.get())));
        def.method("text", 0, |s, d, _| consume(s, d, ReadKind::Text));
        def.method("json", 0, |s, d, _| consume(s, d, ReadKind::Json));
        def.method("arrayBuffer", 0, |s, d, _| consume(s, d, ReadKind::ArrayBuffer));
        def.method("bytes", 0, |s, d, _| consume(s, d, ReadKind::Bytes));
        def.method("blob", 0, |s, d, _| consume(s, d, ReadKind::Blob));
        def.method("clone", 0, |s, d, _| {
            if d.used.get() {
                return Err(JsError::Type("Response body has already been consumed".into()));
            }
            if d.origin != Origin::Network {
                // Script responses own mutable headers: the clone copies them.
                let current = hidden(s, d, HEADERS)?;
                let pairs = s.unwrap::<Headers>(current).map(|h| h.pairs()).unwrap_or_default();
                let headers = HeadersData::from_pairs(pairs, d.origin == Origin::Error);
                return wrap_script(s, None, d.origin, d.head.clone(), d.body.clone(), headers);
            }
            let signal = hidden(s, d, SIGNAL)?;
            wrap(s, &d.head, d.body.clone(), d.credentials, Some(signal))
        });
    }
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
/// "response" guard forbids them); later `headers.set('set-cookie')` on a
/// script response is not filtered (documented gap).
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
        )));
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
    let parsed = match base.as_deref().and_then(|b| lp_net::Url::parse(b).ok()) {
        Some(b) => b.join(target),
        None => lp_net::Url::parse(target),
    };
    parsed
        .map(|u| u.to_string())
        .map_err(|_| JsError::Type(format!("Failed to execute 'redirect' on 'Response': Invalid URL '{target}'")))
}

fn construct<'s>(
    s: &Scope<'s>,
    new_target: Option<lp_js::Object<'s>>,
    origin: Origin,
    status: u16,
    status_text: String,
    headers: HeadersData,
    bytes: Option<Bytes>,
) -> JsResult<Value<'s>> {
    let mime = headers.pairs().into_iter().find(|(n, _)| n == "content-type").map(|(_, v)| v).unwrap_or_default();
    let head = ResponseHead {
        url: lp_net::Url::parse("about:blank").expect("static URL"),
        // Error responses keep 0 via `Origin::Error`; StatusCode needs a valid code.
        status: lp_net::http::StatusCode::from_u16(status.max(200))
            .map_err(|_| JsError::Range("Invalid status".into()))?,
        status_text,
        headers: Default::default(),
        mime: mime.clone(),
        charset: None,
        redirected: false,
        from_cache: false,
        kind: ResponseKind::Basic,
        remote_addr: None,
    };
    let body = crate::fetch::Body::constructed(s, bytes, mime);
    wrap_script(s, new_target, origin, head, body, headers)
}

fn wrap_script<'s>(
    s: &Scope<'s>,
    new_target: Option<lp_js::Object<'s>>,
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
        used: Cell::new(false),
        head,
        credentials: Credentials::SameOrigin,
    };
    let wrapper = match new_target {
        Some(nt) => s.wrap_new::<Response>(data, nt, Some(id))?,
        None => s.wrap::<Response>(data, Some(id))?,
    };
    let headers = s.wrap::<Headers>(headers, None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    Ok(wrapper.value())
}

fn opaque(kind: ResponseKind) -> bool {
    matches!(kind, ResponseKind::Opaque | ResponseKind::OpaqueRedirect)
}
fn hidden<'s>(s: &Scope<'s>, data: &ResponseData, key: &str) -> JsResult<Value<'s>> {
    // A method borrowed from another same-VM realm still addresses the original
    // wrapper's traced properties. Do not root the wrapper in its own payload.
    let realm = data.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let wrapper = owner.lookup_identity(data.id).ok_or(JsError::ContextDestroyed)?;
    Ok(s.get_hidden(wrapper, key)?.unwrap_or(s.undefined()))
}
fn consume<'s>(s: &Scope<'s>, data: &ResponseData, kind: ReadKind) -> JsResult<Value<'s>> {
    let used = if data.body.is_null() { false } else { data.used.replace(true) };
    data.body.read(s, kind, used)
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
        used: Cell::new(false),
        head: head.clone(),
        credentials,
    };
    let wrapper = s.wrap::<Response>(data, Some(id))?;
    let headers = s.wrap::<Headers>(filtered_headers(head, credentials), None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    if let Some(signal) = signal {
        s.set_hidden(wrapper, SIGNAL, signal)?;
    }
    Ok(wrapper.value())
}

pub(crate) fn filtered_headers(head: &ResponseHead, credentials: Credentials) -> HeadersData {
    let exposed = head
        .headers
        .get_all(lp_net::http::header::ACCESS_CONTROL_EXPOSE_HEADERS)
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
