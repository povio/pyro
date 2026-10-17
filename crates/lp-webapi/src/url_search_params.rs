//! Ordered form-url-encoded name/value pairs. Conversion happens before native
//! borrows; iteration reads one current pair at a time rather than snapshotting.
use crate::{target::page_of, util::dom_string};
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Default)]
pub struct ParamsData(Rc<RefCell<Vec<(String, String)>>>);
impl ParamsData {
    pub(crate) fn serialize(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new()).extend_pairs(self.0.borrow().iter()).finish()
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
                let len = array.get("length")?.to_number()? as u32;
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
            s.wrap_new::<Self>(ParamsData(Rc::new(RefCell::new(pairs))), nt, None)
        });
        def.getter("size", |s, data| Ok(s.number(data.0.borrow().len() as f64)));
        def.method("append", 2, |s, data, args| {
            args.require(2, "URLSearchParams.append")?;
            let name = dom_string(args.get(0))?;
            let value = dom_string(args.get(1))?;
            data.0.borrow_mut().push((name, value));
            Ok(s.undefined())
        });
        def.method("set", 2, |s, data, args| {
            args.require(2, "URLSearchParams.set")?;
            let name = dom_string(args.get(0))?;
            let value = dom_string(args.get(1))?;
            let mut value = Some(value);
            let mut pairs = data.0.borrow_mut();
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
            Ok(s.undefined())
        });
        def.method("delete", 1, |s, data, args| {
            args.require(1, "URLSearchParams.delete")?;
            let name = dom_string(args.get(0))?;
            let value = (!args.get(1).is_undefined()).then(|| dom_string(args.get(1))).transpose()?;
            data.0.borrow_mut().retain(|(n, v)| *n != name || value.as_ref().is_some_and(|value| value != v));
            Ok(s.undefined())
        });
        def.method("has", 1, |s, data, args| {
            args.require(1, "URLSearchParams.has")?;
            let name = dom_string(args.get(0))?;
            let value = (!args.get(1).is_undefined()).then(|| dom_string(args.get(1))).transpose()?;
            Ok(s.boolean(
                data.0.borrow().iter().any(|(n, v)| *n == name && value.as_ref().is_none_or(|value| value == v)),
            ))
        });
        def.method("get", 1, |s, data, args| {
            args.require(1, "URLSearchParams.get")?;
            let name = dom_string(args.get(0))?;
            Ok(data.0.borrow().iter().find(|(n, _)| *n == name).map_or(s.null(), |(_, v)| s.string(v)))
        });
        def.method("getAll", 1, |s, data, args| {
            args.require(1, "URLSearchParams.getAll")?;
            let name = dom_string(args.get(0))?;
            let mut values = lp_js::RootedVec::new();
            for (_, value) in data.0.borrow().iter().filter(|(n, _)| *n == name) {
                values.push(s, s.string(value));
            }
            Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("sort", 0, |s, data, _| {
            // Stable order by UTF-16 code units, including supplementary names.
            data.0.borrow_mut().sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            Ok(s.undefined())
        });
        def.method("toString", 0, |s, data, _| Ok(s.string(&data.serialize())));
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
    list: Rc<RefCell<Vec<(String, String)>>>,
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
        def.method("next", 0, |s, data, _| {
            let pair = (!data.done.get()).then(|| data.list.borrow().get(data.index.get()).cloned()).flatten();
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
pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let convert = s.function("usvString", |s, _, args| Ok(s.string(&dom_string(args.get(0))?)))?;
    let convert = Persistent::new(s, convert.value());
    let factory=s.eval(r#"(convert=>{
        const symbol=Symbol.iterator,tag=Symbol.toStringTag,keys=Reflect.ownKeys;
        const descriptor=Object.getOwnPropertyDescriptor,define=Object.defineProperty,create=Object.create;
        const apply=Reflect.apply,entries=URLSearchParams.prototype.entries,TypeError_=TypeError;
        const iteratorPrototype=globalThis['URLSearchParams Iterator'].prototype;
        delete iteratorPrototype.constructor;
        Object.setPrototypeOf(iteratorPrototype,Object.getPrototypeOf(Object.getPrototypeOf([][symbol]())));
        define(iteratorPrototype,tag,{value:'URLSearchParams Iterator',configurable:true});
        delete globalThis['URLSearchParams Iterator'];
        const append=(array,value)=>define(array,array.length,{value,writable:true,enumerable:true,configurable:true});
        define(URLSearchParams.prototype,symbol,{value:entries,writable:true,configurable:true});
        define(URLSearchParams.prototype,'forEach',{value:function(callback,thisArg){
            const iterator=apply(entries,this,[]);
            if(typeof callback!=='function')throw new TypeError_('callback must be callable');
            for(const pair of iterator)apply(callback,thisArg,[pair[1],pair[0],this]);
        },writable:true,enumerable:true,configurable:true});
        return {
            result(value,done){return {value,done};},
            fill(init){
                if(init===undefined)return '';
                if(init===null||(typeof init!=='object'&&typeof init!=='function'))return convert(init);
                const method=init[symbol],result=[];
                if(method!==undefined&&method!==null){
                    if(typeof method!=='function')throw new TypeError_('Invalid iterator');
                    for(const pair of {[symbol](){return apply(method,init,[]);}}){
                        if(pair===null||(typeof pair!=='object'&&typeof pair!=='function'))throw new TypeError_('Expected iterable pair');
                        const values=[];
                        for(const value of pair)append(values,convert(value));
                        if(values.length!==2)throw new TypeError_('URLSearchParams entry must have two items');
                        append(result,values);
                    }
                }else{
                    const positions=create(null);
                    for(const key of keys(init)){
                        if(descriptor(init,key)?.enumerable){
                            const name=convert(key),value=convert(init[key]);
                            if(positions[name]===undefined){positions[name]=result.length;append(result,[name,value]);}
                            else result[positions[name]][1]=value;
                        }
                    }
                }
                return result;
            }
        };
    })"#,Some("lp-url-search-params.js"),1)?;
    let helper = factory.as_object().unwrap().call(None, &[convert.get(s)])?;
    page_of(s)?.url_params_helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helper));
    Ok(())
}
