//! URL component parsing uses rust-url's browser-compatible quirks API.
//! URL/searchParams share native state and traced JS links, without self-roots.
use crate::{
    target::page_of,
    url_search_params::{ParamsData, URLSearchParams},
    util::dom_string,
};
use ::url::{quirks, Url};
use lp_js::{
    Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value, WeakRealm,
};

pub struct UrlData {
    params: ParamsData,
    realm: WeakRealm,
    identity: IdentityKey,
}
pub struct URL;
impl JsClass for URL {
    const NAME: &'static str = "URL";
    type Data = UrlData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        macro_rules! component {
            ($name:literal, $get:ident, $set:ident) => {
                def.accessor(
                    $name,
                    |s, data| Ok(s.string(quirks::$get(data.params.0.url.borrow().as_ref().unwrap()))),
                    |_, data, value| {
                        let value = dom_string(value)?;
                        // Component parser failure is a no-op; href assignment throws.
                        let _ = quirks::$set(data.params.0.url.borrow_mut().as_mut().unwrap(), &value);
                        Ok(())
                    },
                );
            };
        }

        def.constructor(1, |s, nt, args| {
            let url = parse(&args)?.ok_or_else(|| JsError::Type("Invalid URL".into()))?;
            wrap(s, url, Some(nt))
        });
        def.static_method("canParse", 1, |s, args| Ok(s.boolean(parse(&args)?.is_some())));
        def.static_method("parse", 1, |s, args| match parse(&args)? {
            Some(url) => Ok(wrap(s, url, None)?.value()),
            None => Ok(s.null()),
        });
        def.static_method("createObjectURL", 1, |s, args| {
            args.require(1, "URL.createObjectURL")?;
            let page = page_of(s)?;
            if s.unwrap::<crate::media_source::MediaSource>(args.get(0)).is_some() {
                return Ok(s.string(&crate::media_source::object_url(s, &page)?));
            }
            let blob = s.unwrap::<crate::file::Blob>(args.get(0)).ok_or_else(|| {
                JsError::Type("Failed to execute 'createObjectURL' on 'URL': parameter 1 is not of type 'Blob' or 'MediaSource'.".into())
            })?;
            let origin = crate::storage::tuple_origin(s, &page).unwrap_or_else(|| "null".into());
            let url = format!("blob:{origin}/{}", crate::crypto::random_uuid()?);
            page.blob_urls.borrow_mut().insert(url.clone(), (blob.bytes.clone(), blob.mime.clone()));
            Ok(s.string(&url))
        });
        def.static_method("revokeObjectURL", 1, |s, args| {
            args.require(1, "URL.revokeObjectURL")?;
            let url = dom_string(args.get(0))?;
            let page = page_of(s)?;
            // Only the creating origin may revoke; other attempts fail silently.
            let origin = crate::storage::tuple_origin(s, &page).unwrap_or_else(|| "null".into());
            if url
                .strip_prefix("blob:")
                .and_then(|rest| rest.strip_prefix(origin.as_str()))
                .is_some_and(|r| r.starts_with('/'))
            {
                page.blob_urls.borrow_mut().remove(&url);
            }
            Ok(s.undefined())
        });
        def.getter("origin", |s, data| Ok(s.string(&quirks::origin(data.params.0.url.borrow().as_ref().unwrap()))));
        def.accessor(
            "href",
            |s, data| Ok(s.string(data.params.0.url.borrow().as_ref().unwrap().as_str())),
            |_, data, value| {
                let value = dom_string(value)?;
                quirks::set_href(data.params.0.url.borrow_mut().as_mut().unwrap(), &value)
                    .map_err(|_| JsError::Type("Invalid URL".into()))?;
                data.params.refresh_pairs();
                Ok(())
            },
        );
        component!("protocol", protocol, set_protocol);
        component!("username", username, set_username);
        component!("password", password, set_password);
        component!("host", host, set_host);
        component!("hostname", hostname, set_hostname);
        component!("port", port, set_port);
        def.accessor(
            "pathname",
            |s, data| Ok(s.string(quirks::pathname(data.params.0.url.borrow().as_ref().unwrap()))),
            |_, data, value| {
                let value = dom_string(value)?;
                quirks::set_pathname(data.params.0.url.borrow_mut().as_mut().unwrap(), &value);
                Ok(())
            },
        );
        def.accessor(
            "search",
            |s, data| Ok(s.string(quirks::search(data.params.0.url.borrow().as_ref().unwrap()))),
            |_, data, value| {
                let value = dom_string(value)?;
                quirks::set_search(data.params.0.url.borrow_mut().as_mut().unwrap(), &value);
                data.params.refresh_pairs();
                Ok(())
            },
        );
        def.accessor(
            "hash",
            |s, data| Ok(s.string(quirks::hash(data.params.0.url.borrow().as_ref().unwrap()))),
            |_, data, value| {
                let value = dom_string(value)?;
                quirks::set_hash(data.params.0.url.borrow_mut().as_mut().unwrap(), &value);
                Ok(())
            },
        );
        def.getter("searchParams", |s, data| {
            let realm = data.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let owner = realm.scope()?;
            let object = owner.lookup_identity(data.identity).ok_or(JsError::ContextDestroyed)?;
            let value = owner.get_hidden(object, "params")?.ok_or(JsError::ContextDestroyed)?;
            // SAFETY: both realms share the VM; the receiver's traced property
            // roots the cached params wrapper throughout this getter.
            Ok(unsafe { Value::from_raw(s.ctx(), value.as_raw()) })
        });
        for name in ["toString", "toJSON"] {
            def.method(name, 0, |s, data, _| Ok(s.string(data.params.0.url.borrow().as_ref().unwrap().as_str())));
        }
    }
}
fn parse(args: &Args<'_>) -> JsResult<Option<Url>> {
    args.require(1, "URL")?;
    let input = dom_string(args.get(0))?;
    let base = (!args.get(1).is_undefined()).then(|| dom_string(args.get(1))).transpose()?;
    let base = match base {
        Some(base) => match Url::parse(&base) {
            Ok(base) => Some(base),
            Err(_) => return Ok(None),
        },
        None => None,
    };
    Ok(Url::options().base_url(base.as_ref()).parse(&input).ok())
}
fn wrap<'s>(s: &Scope<'s>, url: Url, new_target: Option<Object<'s>>) -> JsResult<Object<'s>> {
    let page = page_of(s)?;
    let identity = page.event_identity(page.next_event_id());
    let params = ParamsData::from_url(url);
    let data = UrlData { params: params.clone(), realm: s.realm().downgrade(), identity };
    let object = match new_target {
        Some(nt) => s.wrap_new::<URL>(data, nt, Some(identity))?,
        None => s.wrap::<URL>(data, Some(identity))?,
    };
    let root = Persistent::new(s, object.value());
    let params = s.wrap::<URLSearchParams>(params, None)?;
    let params_root = Persistent::new(s, params.value());
    s.set_hidden(object, "params", params.value())?;
    s.set_hidden(params, "url", root.get(s))?;
    drop(params_root);
    Ok(object)
}
