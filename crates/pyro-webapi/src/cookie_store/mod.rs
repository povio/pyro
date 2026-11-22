//! Asynchronous Cookie Store operations over the browser context's HTTP jar.
pub(crate) mod events;
pub(crate) mod exposure;
pub(crate) mod subscriptions;

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Persistent, RootedVec, Scope, Value};
use pyro_net::Url;
use pyro_net::cookies::{Access, CookieOptions, SameSite, now_secs};

use crate::page::PageDom;

const MODULE: &str = "lpCookieStore";

/// Store one creation URL per document, before any same-document navigation.
/// No API objects are needed until script actually uses Cookie Store.
pub(crate) fn bind_environment(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<()> {
    page.cookie_creation_url.get_or_init(|| {
        let raw = page.document_url();
        if matches!(raw.split(['#', '?']).next(), Some("about:blank" | "about:srcdoc")) {
            // Initial blank and srcdoc documents inherit their creator's URL
            // for cookie access. A cross-origin base element is never a cookie
            // authority: choose the inherited origin when a base differs.
            if let Some(origin) = crate::storage::tuple_origin(s, page) {
                let dom = page.dom.borrow();
                if let Ok(base) = dom.document_fallback_base_url(page.document()) {
                    if Url::parse(base).ok().is_some_and(|u| u.origin().ascii_serialization() == origin) {
                        return base.to_owned();
                    }
                }
                return origin;
            }
        }
        raw
    });
    Ok(())
}

/// The creation URL of the receiver's relevant environment. A retained
/// object belongs to that environment even when its methods are borrowed.
pub(crate) fn environment(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<Url> {
    let origin = crate::storage::tuple_origin(s, page)
        .filter(|_| !page.has_opaque_origin())
        .ok_or_else(|| JsError::dom("SecurityError", "Cookies are unavailable for an opaque origin."))?;
    bind_environment(s, page)?;
    let url = Url::parse(page.cookie_creation_url.get().expect("creation URL initialized"))
        .map_err(|_| JsError::Type("The cookie URL is invalid.".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.origin().ascii_serialization() != origin {
        return Err(JsError::Type("The environment has no HTTP cookie URL.".into()));
    }
    Ok(url)
}

fn query_url(page: &PageDom, mut creation: Url, option: Option<String>) -> JsResult<Url> {
    if let Some(option) = option {
        let dom = page.dom.borrow();
        let base = dom.document_base_url(page.document()).ok().and_then(|value| Url::parse(value).ok());
        let mut parsed = Url::options()
            .base_url(base.as_ref())
            .parse(&option)
            .map_err(|_| JsError::Type("Invalid CookieStore URL.".into()))?;
        parsed.set_fragment(None);
        creation.set_fragment(None);
        if parsed != creation {
            return Err(JsError::Type("The cookie URL must match this environment's creation URL.".into()));
        }
    }
    Ok(creation)
}

fn optional_string(value: Value<'_>) -> JsResult<Option<String>> {
    if value.is_nullish() { Ok(None) } else { value.to_string().map(Some) }
}

/// Settle through a real task rather than touching the shared HTTP jar during
/// argument conversion or exposing a partially completed operation to script.
fn cookie_task<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    operation: impl for<'a> FnOnce(&Scope<'a>) -> JsResult<Value<'a>> + 'static,
) -> JsResult<Value<'s>> {
    let realm = s.realm().downgrade();
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    page.post_task(Box::new(move || {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(home) = realm.scope() else { return };
        let settled = match operation(&home) {
            Ok(result) => deferred.resolve(&home, result),
            Err(error) => deferred.reject(&home, error.to_value(&home)),
        };
        if let Err(error) = settled {
            home.report_exception(&error);
        }
    }));
    Ok(promise)
}

fn network(page: &PageDom) -> JsResult<pyro_net::NetClient> {
    page.net.borrow().clone().ok_or_else(|| JsError::Type("Cookie storage is unavailable.".into()))
}

struct Write {
    name: String,
    value: String,
    domain: Option<String>,
    expires: Option<f64>,
    max_age: Option<i64>,
    partitioned: bool,
    path: String,
    same_site: SameSite,
}

fn write<'s>(s: &Scope<'s>, data: Write) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let url = environment(s, &page)?;
    let partition = page.cookie_partition_key();
    let site = page.cookie_site_context();
    let net = network(&page)?;
    cookie_task(s, &page, move |home| {
        let cookie = CookieOptions {
            name: &data.name,
            value: &data.value,
            domain: data.domain.as_deref(),
            expires: data.expires,
            max_age: data.max_age,
            partitioned: data.partitioned,
            path: &data.path,
            same_site: data.same_site,
        };
        net.cookies()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .store_script_in(&url, cookie, now_secs(), partition.as_deref(), site)
            .map_err(|msg| JsError::Type(msg.into()))?;
        Ok(home.undefined())
    })
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let ops = s.object();
    ops.set_prototype(s.null());
    ops.set_atom(
        "query",
        s.function("query", |s, _, args| {
            let page = crate::target::page_of(s)?;
            let creation = environment(s, &page)?;
            let name = optional_string(args.get(0))?.map(|s| s.trim_matches([' ', '\t']).to_owned());
            let option = optional_string(args.get(1))?;
            let first = args.get(2).to_bool();
            if first && name.is_none() && option.is_none() {
                return Err(JsError::Type("CookieStore.get requires a cookie name or URL.".into()));
            }
            let url = query_url(&page, creation, option)?;
            let partition = page.cookie_partition_key();
            let site = page.cookie_site_context();
            let net = network(&page)?;
            cookie_task(s, &page, move |home| {
                let items: Vec<_> = {
                    let mut jar = net.cookies().lock().unwrap_or_else(|e| e.into_inner());
                    jar.matching_for(&url, Access::Script, site, now_secs(), partition.as_deref())
                        .into_iter()
                        .filter(|cookie| name.as_ref().is_none_or(|n| cookie.name == *n))
                        .take(if first { 1 } else { usize::MAX })
                        .map(|c| (c.name.clone(), c.value.clone()))
                        .collect()
                };
                let mut rows = RootedVec::new();
                for (key, value) in items {
                    rows.push(home, home.array(&[home.string(&key), home.string(&value)])?.value());
                }
                Ok(home.array(&rows.iter(home).collect::<Vec<_>>())?.value())
            })
        })?
        .value(),
    )?;
    ops.set_atom(
        "write",
        s.function("write", |s, _, args| {
            let expires = args.get(3);
            let expires = if expires.is_null() { None } else { Some(expires.to_number()? / 1000.0) };
            let max_age = optional_string(args.get(4))?
                .map(|v| v.parse::<i64>())
                .transpose()
                .map_err(|_| JsError::Type("Invalid cookie maxAge.".into()))?;
            let same_site = match args.get(7).to_string()?.as_str() {
                "lax" => SameSite::Lax,
                "strict" => SameSite::Strict,
                "none" => SameSite::None,
                _ => return Err(JsError::Type("Invalid SameSite.".into())),
            };
            write(
                s,
                Write {
                    name: args.get(0).to_string()?,
                    value: args.get(1).to_string()?,
                    domain: optional_string(args.get(2))?,
                    expires,
                    max_age,
                    partitioned: args.get(5).to_bool(),
                    path: args.get(6).to_string()?,
                    same_site,
                },
            )
        })?
        .value(),
    )?;
    ops.set_atom(
        "erase",
        s.function("erase", |s, _, args| {
            let name = args.get(0).to_string()?;
            let value = if name.trim_matches([' ', '\t']).is_empty() { "deleted" } else { "" };
            write(
                s,
                Write {
                    name,
                    value: value.into(),
                    domain: optional_string(args.get(1))?,
                    partitioned: args.get(2).to_bool(),
                    path: args.get(3).to_string()?,
                    expires: None,
                    max_age: Some(0),
                    same_site: SameSite::Strict,
                },
            )
        })?
        .value(),
    )?;
    ops.set_atom(
        "handler",
        s.function("handler", |s, _, args| {
            let target = s
                .unwrap_arg::<crate::target::EventTarget>(&args, 0)
                .ok_or_else(|| JsError::Type("Illegal CookieStore receiver.".into()))?;
            if args.len() == 1 {
                crate::target::event_handler(s, target, "change")
            } else {
                crate::target::set_event_handler(s, target, "change", args.get(1))?;
                Ok(s.undefined())
            }
        })?
        .value(),
    )?;
    Ok(ops)
}

pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, MODULE)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let ops = Persistent::new(s, natives(s)?.value());
    let slot = Persistent::new(s, crate::streams::shared_slot(s, page, "CookieStore")?);
    let window = !crate::worker_scope::is_worker(s);
    let factory = s.eval(include_str!("api.js"), Some("pyro://internal/cookie-store.js"), 1)?;
    let result =
        factory.as_object().expect("CookieStore factory").call(None, &[ops.get(s), slot.get(s), s.boolean(window)])?;
    let module = result.as_object().expect("CookieStore module");
    s.set_hidden(holder, MODULE, result)?;
    if window {
        if let Ok(url) = environment(s, page) {
            let store = module.get_atom("cookieStore")?.as_object().expect("CookieStore singleton");
            events::observe(s, page, store, &url, page.cookie_partition_key().as_deref())?;
        }
    }
    Ok(module)
}

/// Lazily materialize only the module needed by this interface.
pub(crate) fn member<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &'static str) -> JsResult<Value<'s>> {
    match name {
        "CookieChangeEvent" | "ExtendableCookieChangeEvent" => {
            events::module(s, page, crate::worker_scope::is_worker(s))?.get_atom(name)
        }
        "CookieStoreManager" => subscriptions::manager_constructor(s, page),
        _ => module(s, page)?.get_atom(name),
    }
}
