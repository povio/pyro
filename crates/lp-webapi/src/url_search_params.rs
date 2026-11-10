//! Ordered form-url-encoded name/value pairs. Conversion happens before native
//! borrows; iteration reads one current pair at a time rather than snapshotting.
use crate::{target::page_of, util::dom_string};
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
pub(crate) struct ParamsState {
    pub(crate) pairs: RefCell<Vec<(String, String)>>,
    pub(crate) url: RefCell<Option<url::Url>>,
}
#[derive(Clone, Default)]
pub struct ParamsData(pub(crate) Rc<ParamsState>);
impl ParamsData {
    fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self(Rc::new(ParamsState { pairs: RefCell::new(pairs), url: RefCell::new(None) }))
    }
    pub(crate) fn from_url(url: url::Url) -> Self {
        let pairs = url.query_pairs().into_owned().collect();
        Self(Rc::new(ParamsState { pairs: RefCell::new(pairs), url: RefCell::new(Some(url)) }))
    }
    pub(crate) fn refresh_pairs(&self) {
        let pairs = self.0.url.borrow().as_ref().unwrap().query_pairs().into_owned().collect();
        *self.0.pairs.borrow_mut() = pairs;
    }
    fn update_url(&self) {
        if self.0.url.borrow().is_none() {
            return;
        }
        let query = self.serialize();
        self.0.url.borrow_mut().as_mut().unwrap().set_query((!query.is_empty()).then_some(&query));
    }
    pub(crate) fn serialize(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new()).extend_pairs(self.0.pairs.borrow().iter()).finish()
    }
}
pub struct URLSearchParams;
impl JsClass for URLSearchParams {
    const NAME: &'static str = "URLSearchParams";
    type Data = ParamsData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, args| {
            let converted = helper(s, "fill")?.as_object().unwrap().call(None, &[args.get(0)])?;
            let converted_root = Persistent::new(s, converted);
            let converted = converted_root.get(s);
            let pairs = if let Some(array) = converted.as_object() {
                let len = array.get_atom("length")?.to_number()? as u32;
                let mut pairs = Vec::with_capacity(len as usize);
                for i in 0..len {
                    let pair = array.get_index(i)?.as_object().unwrap();
                    pairs.push((pair.get_index(0)?.to_string()?, pair.get_index(1)?.to_string()?));
                }
                pairs
            } else {
                let text = converted.to_string()?;
                url::form_urlencoded::parse(text.strip_prefix('?').unwrap_or(&text).as_bytes()).into_owned().collect()
            };
            s.wrap_new::<Self>(ParamsData::from_pairs(pairs), nt, None)
        });
        def.getter("size", |s, data| Ok(s.number(data.0.pairs.borrow().len() as f64)));
        def.method("append", 2, |s, data, args| {
            args.require(2, "URLSearchParams.append")?;
            let name = dom_string(args.get(0))?;
            let value = dom_string(args.get(1))?;
            data.0.pairs.borrow_mut().push((name, value));
            data.update_url();
            Ok(s.undefined())
        });
        def.method("set", 2, |s, data, args| {
            args.require(2, "URLSearchParams.set")?;
            let name = dom_string(args.get(0))?;
            let value = dom_string(args.get(1))?;
            let mut value = Some(value);
            let mut pairs = data.0.pairs.borrow_mut();
            pairs.retain_mut(|(n, v)| {
                if *n != name {
                    return true;
                }
                if let Some(value) = value.take() {
                    *v = value;
                    true
                } else {
                    false
                }
            });
            if let Some(value) = value {
                pairs.push((name, value));
            }
            drop(pairs);
            data.update_url();
            Ok(s.undefined())
        });
        def.method("delete", 1, |s, data, args| {
            args.require(1, "URLSearchParams.delete")?;
            let name = dom_string(args.get(0))?;
            let value = (!args.get(1).is_undefined()).then(|| dom_string(args.get(1))).transpose()?;
            data.0.pairs.borrow_mut().retain(|(n, v)| *n != name || value.as_ref().is_some_and(|value| value != v));
            data.update_url();
            Ok(s.undefined())
        });
        def.pure_method("has", 1, |s, data, args| {
            args.require(1, "URLSearchParams.has")?;
            let name = dom_string(args.get(0))?;
            let value = (!args.get(1).is_undefined()).then(|| dom_string(args.get(1))).transpose()?;
            Ok(s.boolean(
                data.0.pairs.borrow().iter().any(|(n, v)| *n == name && value.as_ref().is_none_or(|value| value == v)),
            ))
        });
        def.pure_method("get", 1, |s, data, args| {
            args.require(1, "URLSearchParams.get")?;
            let name = dom_string(args.get(0))?;
            Ok(data.0.pairs.borrow().iter().find(|(n, _)| *n == name).map_or(s.null(), |(_, v)| s.string(v)))
        });
        def.pure_method("getAll", 1, |s, data, args| {
            args.require(1, "URLSearchParams.getAll")?;
            let name = dom_string(args.get(0))?;
            let mut values = lp_js::RootedVec::new();
            for (_, value) in data.0.pairs.borrow().iter().filter(|(n, _)| *n == name) {
                values.push(s, s.string(value));
            }
            Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("sort", 0, |s, data, _| {
            // Stable order by UTF-16 code units, including supplementary names.
            data.0.pairs.borrow_mut().sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            data.update_url();
            Ok(s.undefined())
        });
        def.pure_method("toString", 0, |s, data, _| Ok(s.string(&data.serialize())));
        def.method("entries", 0, |s, data, _| iterator(s, data, IterKind::Entries));
        def.method("keys", 0, |s, data, _| iterator(s, data, IterKind::Keys));
        def.method("values", 0, |s, data, _| iterator(s, data, IterKind::Values));
    }
}
enum IterKind {
    Entries,
    Keys,
    Values,
}
pub struct IteratorData {
    list: Rc<ParamsState>,
    index: Cell<usize>,
    done: Cell<bool>,
    kind: IterKind,
}
pub(crate) struct ParamsIterator;
impl JsClass for ParamsIterator {
    const NAME: &'static str = "URLSearchParams Iterator";
    type Data = IteratorData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        // No global name; `install` gives the prototype its iterator shape.
        def.not_exposed();
        def.method("next", 0, |s, data, _| {
            let pair = (!data.done.get()).then(|| data.list.pairs.borrow().get(data.index.get()).cloned()).flatten();
            let value = match pair {
                Some((name, value)) => {
                    data.index.set(data.index.get() + 1);
                    match data.kind {
                        IterKind::Keys => s.string(&name),
                        IterKind::Values => s.string(&value),
                        IterKind::Entries => {
                            let name = Persistent::new(s, s.string(&name));
                            let value = s.string(&value);
                            s.array(&[name.get(s), value])?.value()
                        }
                    }
                }
                None => {
                    data.done.set(true);
                    s.undefined()
                }
            };
            let value = Persistent::new(s, value);
            helper(s, "result")?.as_object().unwrap().call(None, &[value.get(s), s.boolean(data.done.get())])
        });
    }
}
fn iterator<'s>(s: &Scope<'s>, data: &ParamsData, kind: IterKind) -> JsResult<Value<'s>> {
    Ok(s.wrap::<ParamsIterator>(
        IteratorData { list: data.0.clone(), index: Cell::new(0), done: Cell::new(false), kind },
        None,
    )?
    .value())
}
fn helper<'s>(s: &Scope<'s>, name: &str) -> JsResult<Value<'s>> {
    let page = page_of(s)?;
    let helpers = page.url_params_helpers.borrow().get(&s.realm_id()).cloned().ok_or(JsError::ContextDestroyed)?;
    helpers.get(s).as_object().unwrap().get(name)
}
/// `factory` is install.js `urlSearchParams`: given the native USVString
/// conversion, [helpers, the steps completing URLSearchParams.prototype and
/// the iterator prototype once their members are installed].
pub(crate) fn install(s: &Scope<'_>, factory: lp_js::Value<'_>) -> JsResult<()> {
    let convert = s.function("usvString", |s, _, args| Ok(s.string(&dom_string(args.get(0))?)))?;
    let convert = Persistent::new(s, convert.value());
    let parts = factory.as_object().unwrap().call(None, &[convert.get(s)])?;
    let parts = parts.as_object().expect("URLSearchParams helpers");
    let helper = parts.get_index(0)?;
    page_of(s)?.url_params_helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helper));
    let hook = |i: u32| parts.get_index(i).map(|v| v.as_object().expect("URLSearchParams hook"));
    s.after_populate::<URLSearchParams>(hook(1)?)?;
    s.after_populate::<ParamsIterator>(hook(2)?)
}
