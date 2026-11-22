//! The HTML Origin value interface. These identities are comparisons for
//! script; existing navigation, message delivery and access checks remain the
//! authority for which platform objects a realm can reach.
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};
use url::{Host, Origin as UrlOrigin, Url};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum OriginValue {
    Url(UrlOrigin),
    /// The embedder's private token for a particular opaque environment.
    /// The public "null" serialization cannot recreate such an identity.
    Environment(String),
}

impl OriginValue {
    fn fresh() -> Self {
        Self::Url(UrlOrigin::new_opaque())
    }

    fn from_key(key: String) -> Option<Self> {
        if key.starts_with("opaque:") || key.starts_with("opaque-document:") {
            return Some(Self::Environment(key));
        }
        let origin = Url::parse(&key).ok()?.origin();
        origin.is_tuple().then_some(Self::Url(origin))
    }

    fn from_url(url: Url, blob_creator: Option<String>) -> Self {
        if url.scheme() != "blob" {
            return Self::Url(url.origin());
        }
        if let Some(origin) = blob_creator.and_then(Self::from_key) {
            return origin;
        }
        match Url::parse(url.path()) {
            Ok(inner) if matches!(inner.scheme(), "http" | "https" | "file") => Self::Url(inner.origin()),
            _ => Self::fresh(),
        }
    }

    fn opaque(&self) -> bool {
        !matches!(self, Self::Url(UrlOrigin::Tuple(..)))
    }

    fn same_site(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Url(UrlOrigin::Tuple(a, x, _)), Self::Url(UrlOrigin::Tuple(b, y, _))) if a == b => match (x, y) {
                (Host::Domain(x), Host::Domain(y)) => pyro_net::cookies::site_of(x) == pyro_net::cookies::site_of(y),
                _ => x == y,
            },
            _ => self == other,
        }
    }
}

pub(crate) struct Origin;

fn extract(s: &Scope<'_>, value: Value<'_>) -> JsResult<OriginValue> {
    if let Some(text) = value.as_string() {
        let url = Url::parse(&text).map_err(|_| JsError::Type("Invalid absolute URL.".into()))?;
        // HTML calls the basic URL parser here. Its string input does not
        // perform the full URL parser's blob-store entry lookup.
        return Ok(OriginValue::from_url(url, None));
    }
    if let Some(origin) = s.unwrap::<Origin>(value) {
        return Ok((*origin).clone());
    }
    if let Some((url, creator)) = crate::origin_urls::object_url(s, value) {
        return Ok(OriginValue::from_url(url, creator));
    }
    crate::origin_globals::global_origin_key(s, value)
        .or_else(|| crate::event::message_origin_key(s, value))
        .and_then(OriginValue::from_key)
        .ok_or_else(|| JsError::Type("The value has no extractable origin.".into()))
}

impl JsClass for Origin {
    const NAME: &'static str = "Origin";
    type Data = OriginValue;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, target, _| s.wrap_new::<Origin>(OriginValue::fresh(), target, None));
        def.static_method("from", 1, |s, args| {
            args.require(1, "Origin.from")?;
            Ok(s.wrap::<Origin>(extract(s, args.get(0))?, None)?.value())
        });
        def.getter("opaque", |s, origin| Ok(s.boolean(origin.opaque())));
        def.method("isSameOrigin", 1, |s, origin, args| {
            args.require(1, "Origin.isSameOrigin")?;
            let other = s.unwrap_arg::<Origin>(&args, 0).ok_or_else(|| JsError::Type("Expected an Origin.".into()))?;
            Ok(s.boolean(origin == other))
        });
        def.method("isSameSite", 1, |s, origin, args| {
            args.require(1, "Origin.isSameSite")?;
            let other = s.unwrap_arg::<Origin>(&args, 0).ok_or_else(|| JsError::Type("Expected an Origin.".into()))?;
            Ok(s.boolean(origin.same_site(other)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> OriginValue {
        OriginValue::from_url(Url::parse(text).unwrap(), None)
    }

    #[test]
    fn tuple_comparisons_preserve_scheme_and_ignore_port_only_for_sites() {
        let a = parsed("https://example.org:443/path");
        assert!(a == parsed("https://example.org/another"));
        assert!(a != parsed("https://example.org:444/"));
        assert!(a.same_site(&parsed("https://sub.example.org:444/")));
        assert!(!a.same_site(&parsed("http://example.org/")));
        assert!(!parsed("wss://example.org").same_site(&a));
        assert!(!parsed("https://one.github.io").same_site(&parsed("https://two.github.io")));
        assert!(parsed("https://[::1]:81").same_site(&parsed("https://[::1]:82")));
    }

    #[test]
    fn opaque_copies_and_registered_blob_origins_retain_identity() {
        let a = OriginValue::fresh();
        assert!(a.opaque() && a == a.clone() && !a.same_site(&OriginValue::fresh()));
        let key = "opaque:42".to_owned();
        let url = Url::parse("blob:null/example").unwrap();
        assert!(OriginValue::from_url(url.clone(), Some(key.clone())) == OriginValue::from_key(key).unwrap());
        assert!(OriginValue::from_url(url.clone(), None) != OriginValue::from_url(url, None));
        assert!(parsed("blob:https://example.org/object") == parsed("https://example.org"));
        assert!(parsed("blob:ftp://example.org/object").opaque());
        assert!(parsed("blob:blob:https://example.org/object").opaque());
        assert!(OriginValue::from_key("null".into()).is_none());
    }
}
