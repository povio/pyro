//! Network-created Responses. Body streams and the public Response constructor
//! are deliberately not exposed until their semantics are implemented.
use crate::{
    fetch::{Body, ReadKind},
    headers::{Headers, HeadersData},
};
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Scope, Value, WeakRealm};
use lp_net::{Credentials, ResponseHead, ResponseKind};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RESPONSE: AtomicU64 = AtomicU64::new(1);
const HEADERS: &str = "__lpResponseHeaders";
const SIGNAL: &str = "__lpResponseSignal";
pub struct ResponseData {
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
        def.getter("url", |s, d| Ok(s.string(if opaque(d.head.kind) { "" } else { d.head.url.as_str() })));
        def.getter("status", |s, d| {
            Ok(s.number(if opaque(d.head.kind) { 0.0 } else { f64::from(d.head.status.as_u16()) }))
        });
        def.getter("statusText", |s, d| Ok(s.string(if opaque(d.head.kind) { "" } else { &d.head.status_text })));
        def.getter("ok", |s, d| Ok(s.boolean(!opaque(d.head.kind) && d.head.status.is_success())));
        def.getter("redirected", |s, d| Ok(s.boolean(!opaque(d.head.kind) && d.head.redirected)));
        def.getter("type", |s, d| {
            Ok(s.string(match d.head.kind {
                ResponseKind::Basic => "basic",
                ResponseKind::Cors => "cors",
                ResponseKind::Opaque => "opaque",
                ResponseKind::OpaqueRedirect => "opaqueredirect",
            }))
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
            let signal = hidden(s, d, SIGNAL)?;
            wrap(s, &d.head, d.body.clone(), d.credentials, Some(signal))
        });
    }
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
        id,
        realm: s.realm().downgrade(),
        body,
        used: Cell::new(false),
        head: head.clone(),
        credentials,
    };
    let wrapper = s.wrap::<Response>(data, Some(id))?;
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
    let headers = s.wrap::<Headers>(HeadersData::from_pairs(pairs, true), None)?;
    s.set_hidden(wrapper, HEADERS, headers.value())?;
    if let Some(signal) = signal {
        s.set_hidden(wrapper, SIGNAL, signal)?;
    }
    Ok(wrapper.value())
}
