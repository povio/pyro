//! Named-property hooks: WebIDL `[[Set]]` and `[[Delete]]` semantics on the
//! receiver, symbol-keyed expandos, and names kept as UTF-16 (lone surrogates).

use lp_js::*;
use std::cell::RefCell;

type Entries = RefCell<Vec<(Vec<u16>, Vec<u16>)>>;

fn utf16<'s>(s: &Scope<'s>, units: &[u16]) -> Value<'s> {
    s.js_string(&JsString::from_utf16(units))
}

/// A Storage-like class on the UTF-16 hook variants.
struct Bag;
impl JsClass for Bag {
    const NAME: &'static str = "Bag";
    type Data = Entries;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("size", |s, d| Ok(s.number(d.borrow().len() as f64)));
        def.method("item", 1, |s, d, args| {
            let key = args.get(0).to_js_string()?;
            Ok(d.borrow().iter().find(|(k, _)| k == key.utf16()).map_or(s.null(), |(_, v)| utf16(s, v)))
        });
        def.named_utf16(false, |s, d, name| Ok(d.borrow().iter().find(|(k, _)| k == name).map(|(_, v)| utf16(s, v))));
        def.named_setter_utf16(|_s, d, name, v| {
            let v = v.to_js_string()?.utf16().to_vec();
            let mut d = d.borrow_mut();
            match d.iter_mut().find(|(k, _)| k == name) {
                Some(entry) => entry.1 = v,
                None => d.push((name.to_vec(), v)),
            }
            Ok(true)
        });
        def.named_setter_for_all_names();
        def.named_deleter_utf16(|_s, d, name| {
            let mut d = d.borrow_mut();
            let Some(i) = d.iter().position(|(k, _)| k == name) else { return Ok(false) };
            d.remove(i);
            Ok(true)
        });
        def.property_names_utf16(|_s, d| Ok(d.borrow().iter().map(|(k, _)| k.clone()).collect()));
    }
}

/// The `&str` hook variants: names arrive converted lossily.
struct Lossy;
impl JsClass for Lossy {
    const NAME: &'static str = "Lossy";
    type Data = RefCell<Vec<String>>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.named(false, |s, _d, name| Ok(Some(s.string(name))));
        def.named_setter(|_s, d, name, _v| {
            d.borrow_mut().push(name.to_owned());
            Ok(true)
        });
        def.named_deleter(|_s, d, name| Ok(d.borrow_mut().iter().any(|n| n == name)));
        def.property_names(|_s, d| Ok(d.borrow().clone()));
    }
}

fn setup() -> (Engine, Realm) {
    let e = Engine::new().expect("engine");
    e.register::<Bag>();
    e.register::<Lossy>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    {
        let s = r.scope().unwrap();
        for name in ["bag", "fresh"] {
            let o = s.wrap::<Bag>(RefCell::new(Vec::new()), None).unwrap();
            s.global().set(name, o.value()).unwrap();
        }
        let o = s.wrap::<Lossy>(RefCell::new(Vec::new()), None).unwrap();
        s.global().set("lossy", o.value()).unwrap();
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
fn set_runs_the_named_setter_for_every_string_name_on_the_receiver() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            // Prototype members are never shadowed, but the setter still stores.
            ("bag.size = 'v'; [typeof bag.size, bag.size, bag.item('size')].join('|')", "number|1|v"),
            ("bag.item = 'i'; [typeof bag.item, bag.item('item'), bag.size].join('|')", "function|i|2"),
            ("Object.getOwnPropertyDescriptor(bag, 'size') === undefined", "true"),
            ("bag.hasOwnProperty('size')", "false"),
            (
                "bag.plain = 'p'; [bag.plain, bag.hasOwnProperty('plain'), Object.keys(bag).join(',')].join('|')",
                "p|true|size,item,plain",
            ),
            ("'use strict'; bag.toString = 't'; typeof bag.toString + '|' + bag.item('toString')", "function|t"),
        ],
    );
}

#[test]
fn without_the_opt_in_prototype_names_bypass_the_named_setter() {
    let (_e, r) = setup();
    // `Lossy` emulates per-name attributes: a prototype accessor keeps its
    // setter, so the named setter sees only names the prototype lacks.
    check(
        &r,
        &[
            ("Object.defineProperty(Object.getPrototypeOf(lossy), 'acc', { set(v) { this_set = v; }, get() { return 'g'; }, configurable: true });               lossy.acc = 'v'; lossy.other = 'o'; [typeof this_set, lossy.acc, Object.keys(lossy).join(',')].join('|')", "string|g|other"),
        ],
    );
}

#[test]
fn delete_of_prototype_names_and_unsupported_names_is_the_ordinary_delete() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            ("bag.size = 'v'; bag.x = 'y'; [delete bag.size, bag.item('size'), delete bag.x, bag.item('x'), delete bag.x, delete bag.nothing].join('|')", "true|v|true||true|true"),
            // A symbol is never a named property: the own property it defines keeps its attributes.
            (
                "(() => { const k = Symbol(); Object.defineProperty(bag, k, { value: 't', configurable: false }); \
                 return [Object.getOwnPropertyDescriptor(bag, k).configurable, delete bag[k], bag[k], Object.getOwnPropertySymbols(bag).length].join('|') })()",
                "false|false|t|1",
            ),
            ("(() => { const k = Symbol(); return [delete bag[k], bag[k]].map(String).join('|') })()", "true|undefined"),
        ],
    );
}

#[test]
fn utf16_hooks_keep_lone_surrogates_and_str_hooks_convert_lossily() {
    let (_e, r) = setup();
    check(
        &r,
        &[
            (
                "fresh['\\uD800'] = '\\uDC00'; const hex = s => s.split('').map(c => c.charCodeAt(0).toString(16)).join(','); \
                 [hex(fresh['\\uD800']), hex(Object.keys(fresh)[0]), hex(fresh.item('\\uD800')), '\\uD800' in fresh, '\\uFFFD' in fresh, delete fresh['\\uFFFD'], delete fresh['\\uD800'], fresh.size].join('|')",
                "dc00|d800|dc00|true|false|true|true|0",
            ),
            ("hex(lossy['\\uD800']) + '|' + hex(lossy['a\\uDF4D'])", "fffd|61,fffd"),
            ("lossy['\\uD800'] = 1; [hex(Object.keys(lossy)[0]), delete lossy['\\uD800']].join('|')", "fffd|true"),
        ],
    );
}
