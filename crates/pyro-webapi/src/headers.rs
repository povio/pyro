//! Fetch Headers storage and validation. JS adapters provide iterator protocol
//! mechanics; header state stays native and borrows never cross JS execution.
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::{target::page_of, util::dom_string};
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};

type Entries = Rc<Vec<(String, String)>>;
#[derive(Clone, Default)]
pub struct HeaderList {
    values: BTreeMap<String, Vec<String>>,
    sorted: Option<Entries>,
    immutable: bool,
    /// Fetch's "request" (`Some(false)`) or "request-no-cors" (`Some(true)`)
    /// guard: forbidden request headers are silently not set.
    request_guard: Option<bool>,
}
impl HeaderList {
    fn append(&mut self, name: String, value: String) {
        self.values.entry(name).or_default().push(value);
        self.sorted = None;
    }
    fn entries(&mut self) -> Entries {
        self.sorted
            .get_or_insert_with(|| {
                Rc::new(
                    self.values
                        .iter()
                        .flat_map(|(name, values)| {
                            if name == "set-cookie" {
                                values.iter().map(|value| (name.clone(), value.clone())).collect::<Vec<_>>()
                            } else {
                                vec![(name.clone(), values.join(", "))]
                            }
                        })
                        .collect(),
                )
            })
            .clone()
    }
}

#[derive(Clone)]
pub struct HeadersData(Rc<RefCell<HeaderList>>);
impl HeadersData {
    pub(crate) fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>, immutable: bool) -> Self {
        let mut list = HeaderList::default();
        for (name, value) in pairs {
            list.append(name, value);
        }
        list.immutable = immutable;
        Self(Rc::new(RefCell::new(list)))
    }
    /// The headers of a Request: its guard is "request", or
    /// "request-no-cors" for a `no-cors` request.
    pub(crate) fn guard_request(self, no_cors: bool) -> Self {
        self.0.borrow_mut().request_guard = Some(no_cors);
        self
    }
    /// Whether the guard lets script write `name: value` (`value` is the
    /// value the header would have, `None` for a removal).
    fn guard_allows(&self, name: &str, value: Option<&str>) -> bool {
        let list = self.0.borrow();
        let Some(no_cors) = list.request_guard else { return true };
        if crate::fetch::forbidden_request_header(name, value.unwrap_or("")) {
            return false;
        }
        if !no_cors {
            return true;
        }
        match value {
            Some(value) => cors_safelisted(name, value),
            None => matches!(name, "accept" | "accept-language" | "content-language" | "content-type"),
        }
    }
    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        self.0.borrow().values.iter().map(|(n, values)| (n.clone(), values.join(", "))).collect()
    }
    pub(crate) fn mime_type(&self) -> Option<data_url::mime::Mime> {
        let list = self.0.borrow();
        let values = list.values.get("content-type")?;
        match values.as_slice() {
            [value] => crate::fetch::extract_mime(value),
            _ => crate::fetch::extract_mime(&values.join(", ")),
        }
    }
    /// Whether a (lowercase) header name is present.
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.borrow().values.contains_key(name)
    }
    /// Appends a validated (lowercase name, byte-string value) pair.
    pub(crate) fn append_raw(&self, name: &str, value: &str) {
        self.0.borrow_mut().append(name.to_owned(), value.to_owned());
    }
    /// Removes a (lowercase) header name.
    pub(crate) fn remove(&self, name: &str) {
        let mut list = self.0.borrow_mut();
        if list.values.remove(name).is_some() {
            list.sorted = None;
        }
    }
    fn check_mutable(&self) -> JsResult<()> {
        if self.0.borrow().immutable {
            Err(JsError::Type("Headers are immutable".into()))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn from_init(s: &Scope<'_>, init: Value<'_>) -> JsResult<HeadersData> {
    let list = Rc::new(RefCell::new(HeaderList::default()));
    if !init.is_undefined() {
        let convert = s.function("byteString", |s, _this, args| Ok(s.string(&byte_string(args.get(0))?)))?;
        let converted = helper(s, "fill")?.as_object().unwrap().call(None, &[init, convert.value()])?;
        let entries = converted.as_object().expect("converted sequence");
        let len = entries.get_atom("length")?.to_number()? as u32;
        for i in 0..len {
            let entry = entries.get_index(i)?.as_object().expect("converted entry");
            if entry.get_atom("length")?.to_number()? != 2.0 {
                return Err(JsError::Type("Header entry must have two items".into()));
            }
            let name = header_name(entry.get_index(0)?)?;
            let value = validate_value(byte_string(entry.get_index(1)?)?)?;
            list.borrow_mut().append(name, value);
        }
    }
    Ok(HeadersData(list))
}

pub struct Headers;
impl JsClass for Headers {
    const NAME: &'static str = "Headers";
    type Data = HeadersData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, args| s.wrap_new::<Headers>(from_init(s, args.get(0))?, nt, None));
        def.method("append", 2, |s, data, args| {
            args.require(2, "Headers.append")?;
            let name = byte_string(args.get(0))?;
            let value = byte_string(args.get(1))?;
            let name = validate_name(name)?;
            let value = validate_value(value)?;
            data.check_mutable()?;
            // A no-cors guard judges the combined value.
            let combined = match data.0.borrow().values.get(&name) {
                Some(existing) if !existing.is_empty() => format!("{}, {value}", existing.join(", ")),
                _ => value.clone(),
            };
            if data.guard_allows(&name, Some(&combined)) {
                data.0.borrow_mut().append(name, value);
            }
            Ok(s.undefined())
        });
        def.method("set", 2, |s, data, args| {
            args.require(2, "Headers.set")?;
            let name = byte_string(args.get(0))?;
            let value = byte_string(args.get(1))?;
            let name = validate_name(name)?;
            let value = validate_value(value)?;
            data.check_mutable()?;
            if !data.guard_allows(&name, Some(&value)) {
                return Ok(s.undefined());
            }
            let mut list = data.0.borrow_mut();
            list.values.insert(name, vec![value]);
            list.sorted = None;
            Ok(s.undefined())
        });
        def.method("delete", 1, |s, data, args| {
            args.require(1, "Headers.delete")?;
            let name = header_name(args.get(0))?;
            data.check_mutable()?;
            if !data.guard_allows(&name, None) {
                return Ok(s.undefined());
            }
            let mut list = data.0.borrow_mut();
            list.values.remove(&name);
            list.sorted = None;
            Ok(s.undefined())
        });
        def.pure_method("has", 1, |s, data, args| {
            args.require(1, "Headers.has")?;
            let name = header_name(args.get(0))?;
            Ok(s.boolean(data.0.borrow().values.contains_key(&name)))
        });
        def.pure_method("get", 1, |s, data, args| {
            args.require(1, "Headers.get")?;
            let name = header_name(args.get(0))?;
            Ok(data.0.borrow().values.get(&name).map_or(s.null(), |values| s.string(&values.join(", "))))
        });
        def.pure_method("getSetCookie", 0, |s, data, _args| {
            let mut values = pyro_js::RootedVec::new();
            if let Some(cookies) = data.0.borrow().values.get("set-cookie") {
                for cookie in cookies {
                    values.push(s, s.string(cookie));
                }
            }
            Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("entries", 0, |s, data, _args| iterator(s, data, IterKind::Entries));
        def.method("keys", 0, |s, data, _args| iterator(s, data, IterKind::Keys));
        def.method("values", 0, |s, data, _args| iterator(s, data, IterKind::Values));
    }
}

pub(crate) fn byte_string(value: Value<'_>) -> JsResult<String> {
    let value = dom_string(value)?;
    if value.chars().any(|c| u32::from(c) > 255) {
        return Err(JsError::Type("Header name or value is not a ByteString".into()));
    }
    Ok(value)
}
fn header_name(value: Value<'_>) -> JsResult<String> {
    validate_name(byte_string(value)?)
}
pub(crate) fn validate_name(name: String) -> JsResult<String> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
        return Err(JsError::Type("Invalid HTTP header name".into()));
    }
    Ok(name.to_ascii_lowercase())
}
pub(crate) fn validate_value(value: String) -> JsResult<String> {
    let value = value.trim_matches([' ', '\t', '\r', '\n']);
    if value.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n')) {
        return Err(JsError::Type("Invalid HTTP header value".into()));
    }
    Ok(value.to_owned())
}

enum IterKind {
    Entries,
    Keys,
    Values,
}
fn iterator<'s>(s: &Scope<'s>, data: &HeadersData, kind: IterKind) -> JsResult<Value<'s>> {
    let list = data.0.clone();
    let index = Cell::new(0usize);
    let done = Cell::new(false);
    let next = s.function("next", move |s, _this, _args| {
        let entries = list.borrow_mut().entries();
        let entry = (!done.get()).then(|| entries.get(index.get())).flatten();
        let out = s.object();
        out.set_atom("done", s.boolean(entry.is_none()))?;
        let value = match entry {
            Some((key, value)) => {
                index.set(index.get() + 1);
                match kind {
                    IterKind::Keys => s.string(key),
                    IterKind::Values => s.string(value),
                    IterKind::Entries => {
                        let key = s.string(key);
                        let value = s.string(value);
                        s.array(&[key, value])?.value()
                    }
                }
            }
            None => {
                done.set(true);
                s.undefined()
            }
        };
        out.set_atom("value", value)?;
        Ok(out.value())
    })?;
    helper(s, "iterator")?.as_object().unwrap().call(None, &[next.value()])
}

fn helper<'s>(s: &Scope<'s>, name: &str) -> JsResult<Value<'s>> {
    let page = page_of(s)?;
    let helpers = page
        .header_helpers
        .borrow()
        .get(&s.realm_id())
        .cloned()
        .ok_or_else(|| JsError::Type("Headers bindings are not installed".into()))?;
    helpers.get(s).as_object().unwrap().get(name)
}

/// Whether (`name`, `value`) is a CORS-safelisted request header.
pub(crate) fn cors_safelisted(name: &str, value: &str) -> bool {
    pyro_net::http::HeaderValue::from_str(value).is_ok_and(|v| pyro_net::cors::is_safelisted_header(name, &v))
}

/// `pair` is install.js `headers`: [helpers, the step that completes
/// Headers.prototype once its members are installed (`after_populate`)].
pub(crate) fn install(s: &Scope<'_>, pair: pyro_js::Value<'_>) -> JsResult<()> {
    let pair = pair.as_object().expect("headers helpers");
    let helpers = pair.get_index(0)?;
    page_of(s)?.header_helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helpers));
    s.after_populate::<Headers>(pair.get_index(1)?.as_object().expect("headers hook"))
}
