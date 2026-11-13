//! `ClassDef::legacy_platform_object` and `ClassDef::indexed_iterator`: the
//! WebIDL legacy platform object rules of a class with an indexed getter and
//! a named getter but no setters (`HTMLCollection`).

use pyro_js::*;
use std::cell::RefCell;

#[derive(Default)]
struct Items {
    values: RefCell<Vec<f64>>,
    names: RefCell<Vec<(String, f64)>>,
}

/// An `HTMLCollection`-like class.
struct Coll;
impl JsClass for Coll {
    const NAME: &'static str = "Coll";
    type Data = Items;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, d| Ok(s.number(d.values.borrow().len() as f64)));
        def.method("item", 1, |s, d, a| {
            let i = a.get(0).to_number()? as usize;
            Ok(d.values.borrow().get(i).map_or(s.null(), |v| s.number(*v)))
        });
        def.method("push", 1, |s, d, a| {
            d.values.borrow_mut().push(a.get(0).to_number()?);
            Ok(s.undefined())
        });
        def.method("name", 2, |s, d, a| {
            d.names.borrow_mut().push((a.get(0).to_string()?, a.get(1).to_number()?));
            Ok(s.undefined())
        });
        def.indexed(|s, d, i| Ok(d.values.borrow().get(i as usize).map(|v| s.number(*v))));
        def.named(false, |s, d, name| Ok(d.names.borrow().iter().find(|(n, _)| n == name).map(|(_, v)| s.number(*v))));
        def.unenumerable_property_names(|_s, d| Ok(d.names.borrow().iter().map(|(n, _)| n.clone()).collect()));
        def.indexed_iterator();
        def.legacy_platform_object();
    }
}

/// The same getters without the opt-in.
struct Plain;
impl JsClass for Plain {
    const NAME: &'static str = "Plain";
    type Data = Items;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, d| Ok(s.number(d.values.borrow().len() as f64)));
        def.method("name", 2, |s, d, a| {
            d.names.borrow_mut().push((a.get(0).to_string()?, a.get(1).to_number()?));
            Ok(s.undefined())
        });
        def.indexed(|s, d, i| Ok(d.values.borrow().get(i as usize).map(|v| s.number(*v))));
        def.named(false, |s, d, name| Ok(d.names.borrow().iter().find(|(n, _)| n == name).map(|(_, v)| s.number(*v))));
        def.indexed_iterable();
    }
}

fn setup() -> (Engine, Realm) {
    let e = Engine::new().expect("engine");
    e.register::<Coll>();
    e.register::<Plain>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    {
        let s = r.scope().unwrap();
        let items = || Items { values: RefCell::new(vec![10.0, 20.0]), names: RefCell::new(vec![("id".into(), 7.0)]) };
        let o = s.wrap::<Coll>(items(), None).unwrap();
        s.global().set("coll", o.value()).unwrap();
        let o = s.wrap::<Plain>(items(), None).unwrap();
        s.global().set("plain", o.value()).unwrap();
        s.eval(
            "function kind(f) { try { f(); return 'no throw'; } catch (e) { return e.constructor.name; } }
             function own(o) { return Object.getOwnPropertyNames(o).join(','); }",
            Some("setup.js"),
            1,
        )
        .unwrap();
    }
    (e, r)
}

fn check(realm: &Realm, checks: &[(&str, &str)]) {
    let s = realm.scope().unwrap();
    for (src, expected) in checks {
        let got = match s.eval(src, Some("test.js"), 1) {
            Ok(v) => v.to_string().unwrap(),
            Err(e) => format!("THROW {}", e.details(&s).text()),
        };
        assert_eq!(got, *expected, "{src}");
    }
}

#[test]
fn an_expando_shadows_a_named_property_that_appears_later() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            ("coll.later = 'expando'; coll.name('later', 1); coll.later", "expando"),
            ("coll.hasOwnProperty('later') + '|' + Object.getOwnPropertyDescriptor(coll, 'later').writable", "true|true"),
            ("coll.later = 'again'; coll.later", "again"),
            // Deleting the expando uncovers the named property.
            ("delete coll.later; coll.later", "1"),
            // A non-configurable expando stays for good.
            (
                "Object.defineProperty(coll, 'fixed', { value: 5 }); coll.name('fixed', 2);
                 [coll.fixed, delete coll.fixed, coll.fixed, kind(() => { 'use strict'; delete coll.fixed; })].join('|')",
                "5|false|5|TypeError",
            ),
            // A prototype member is shadowed by an expando as on any object.
            ("coll.item = 'mine'; coll.item + '|' + typeof Object.getPrototypeOf(coll).item", "mine|function"),
        ],
    );
}

#[test]
fn a_write_to_a_supported_property_fails_like_a_read_only_one() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            // Sloppy mode code: ignored.
            ("coll[0] = 'x'; coll[5] = 'y'; coll.id = 'z'; [coll[0], coll[5], coll.id, own(coll)].join('|')", "10||7|0,1,id"),
            // Strict mode code: a TypeError, for a present index, one past the end and a visible name.
            ("kind(() => { 'use strict'; coll[0] = 'x'; })", "TypeError"),
            ("kind(() => { 'use strict'; coll[5] = 'x'; })", "TypeError"),
            ("kind(() => { 'use strict'; coll['1'] = 'x'; })", "TypeError"),
            ("kind(() => { 'use strict'; coll.id = 'x'; })", "TypeError"),
            ("[Reflect.set(coll, 0, 'x'), Reflect.set(coll, 'id', 'x'), Reflect.set(coll, 'free', 'x')].join('|')", "false|false|true"),
            // Nothing is left on the wrapper, whatever is asked first.
            ("kind(() => { 'use strict'; coll[1] = 'x'; }) + '|' + coll[1] + '|' + own(coll)", "TypeError|20|0,1,free,id"),
            ("kind(() => { 'use strict'; coll.id = 'x'; }) + '|' + own(coll)", "TypeError|0,1,free,id"),
            ("kind(() => { 'use strict'; coll[1] = 'x'; }) + '|' + Object.keys(coll)", "TypeError|0,1,free"),
            ("kind(() => { 'use strict'; coll[5] = 'x'; }) + '|' + (5 in coll) + '|' + own(coll)", "TypeError|false|0,1,free,id"),
            ("kind(() => { 'use strict'; coll[5] = 'x'; }) + '|' + Object.getOwnPropertyDescriptor(coll, 5)", "TypeError|undefined"),
            ("kind(() => { 'use strict'; coll.id = 'x'; }) + '|' + JSON.stringify(Object.getOwnPropertyDescriptor(coll, 'id'))",
             r#"TypeError|{"value":7,"writable":false,"enumerable":false,"configurable":true}"#),
            ("kind(() => { 'use strict'; coll[5] = 'x'; }); coll.push(1); coll.push(2); coll.push(3); coll.push(4); coll[5]", "4"),
            ("coll[0] + '|' + coll.id + '|' + coll.length", "10|7|6"),
            // A name that is not supported becomes an expando in either mode.
            ("kind(() => { 'use strict'; coll.other = 'o'; }) + '|' + coll.other", "no throw|o"),
        ],
    );
}

#[test]
fn unenumerable_names_are_own_keys_that_enumeration_skips() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            ("own(coll)", "0,1,id"),
            ("Reflect.ownKeys(coll).join(',')", "0,1,id"),
            ("Object.keys(coll).join(',')", "0,1"),
            ("(() => { const seen = []; for (const k in coll) if (coll.hasOwnProperty(k)) seen.push(k); return seen.join(','); })()", "0,1"),

            // Listing twice, and after the names changed, is consistent.
            ("coll.name('second', 2); coll.name('id', 9); own(coll) + '|' + own(coll)", "0,1,id,second|0,1,id,second"),
            ("JSON.stringify(Object.getOwnPropertyDescriptor(coll, 'second'))",
             r#"{"value":2,"writable":false,"enumerable":false,"configurable":true}"#),
            // Not visible: a name the prototype chain has, an array index, an expando's name.
            ("coll.name('item', 1); coll.name('7', 1); coll.expando = 1; coll.name('expando', 2); own(coll)", "0,1,expando,id,second"),
            ("coll.expando + '|' + typeof coll.item + '|' + coll[7]", "1|function|undefined"),
            // The class without the hook lists indices only.
            ("own(plain)", "0,1"),
        ],
    );
}

#[test]
fn refused_writes_survive_collection_of_the_wrapper() {

    let (e, r) = setup();
    check(&r, &[("kind(() => { 'use strict'; coll[9] = 'x'; }); coll = null; 'dropped'", "dropped")]);
    e.collect_garbage_now();
    check(&r, &[("kind(() => { 'use strict'; plain.length; }) + '|' + own(plain)", "no throw|0,1")]);
    e.collect_garbage_now();
}

#[test]
fn without_the_opt_in_writes_are_ignored_and_names_win() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            ("kind(() => { 'use strict'; plain[0] = 'x'; }) + '|' + plain[0]", "no throw|10"),
            ("plain.later = 'expando'; plain.name('later', 1); plain.later", "1"),
        ],
    );
}

#[test]
fn indexed_iterator_installs_only_symbol_iterator() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            ("[...coll].join(',')", "10,20"),
            ("Object.getPrototypeOf(coll)[Symbol.iterator] === Array.prototype.values", "true"),
            ("['entries', 'keys', 'values', 'forEach'].filter(k => k in coll).join(',')", ""),
            ("['entries', 'keys', 'values', 'forEach'].filter(k => k in plain).join(',')", "entries,keys,values,forEach"),
        ],
    );
}
