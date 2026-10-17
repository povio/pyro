//! Lazy member installation must be unobservable: interface objects, their
//! prototypes and members have exactly the shape eager installation produced,
//! whatever touches a class first.

use lp_js::*;
use std::rc::Rc;

struct Data {
    id: u32,
}

struct Base;
impl JsClass for Base {
    const NAME: &'static str = "LzBase";
    type Data = Rc<Data>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constant("BASE_CONST", 7.0);
        def.getter("id", |s, d| Ok(s.number(d.id as f64)));
        def.accessor("label", |s, _d| Ok(s.string("base")), |_s, _d, _v| Ok(()));
        def.method("ping", 0, |s, _d, _a| Ok(s.string("pong")));
        def.method("twoArgs", 2, |s, _d, a| Ok(s.number(a.len() as f64)));
        def.static_method("create", 1, |s, _a| Ok(s.string("static")));
    }
}

struct Mid;
impl JsClass for Mid {
    const NAME: &'static str = "LzMid";
    type Data = Rc<Data>;
    type Parent = Base;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, _d| Ok(s.number(2.0)));
        def.indexed(|s, _d, i| Ok((i < 2).then(|| s.number(i as f64 * 10.0))));
        def.indexed_iterable();
        def.method("midOnly", 0, |s, _d, _a| Ok(s.boolean(true)));
    }
}

struct Leaf;
impl JsClass for Leaf {
    const NAME: &'static str = "LzLeaf";
    type Data = Rc<Data>;
    type Parent = Mid;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, _a| s.wrap_new::<Leaf>(Rc::new(Data { id: 99 }), nt, None));
        def.method("leafOnly", 0, |s, _d, _a| Ok(s.string("leaf")));
    }
}

struct Hidden;
impl JsClass for Hidden {
    const NAME: &'static str = "LzHidden";
    type Data = Rc<Data>;
    type Parent = Base;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.method("hiddenOnly", 0, |s, _d, _a| Ok(s.number(1.0)));
    }
}

/// JavaScript that records the observable shape of the test interfaces.
const DUMP: &str = r#"(() => {
  const d = (o, k) => {
    const x = Object.getOwnPropertyDescriptor(o, k);
    if (!x) return null;
    const out = { e: x.enumerable, c: x.configurable };
    if ('value' in x) { out.w = x.writable; out.t = typeof x.value; if (typeof x.value !== 'object' && typeof x.value !== 'function') out.v = x.value; if (typeof x.value === 'function') { out.n = x.value.name; out.l = x.value.length; } }
    else { out.g = x.get && [x.get.name, x.get.length]; out.s = x.set && [x.set.name, x.set.length]; }
    return out;
  };
  const shape = o => Object.fromEntries(Reflect.ownKeys(o).map(k => [String(k), d(o, k)]).sort((a, b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const res = {};
  for (const name of ['LzBase', 'LzMid', 'LzLeaf', 'LzHidden']) {
    const g = d(globalThis, name);
    const C = globalThis[name];
    res[name] = { global: g, type: typeof C };
    if (typeof C !== 'function') continue;
    res[name].ctor = shape(C);
    res[name].proto = shape(C.prototype);
    res[name].protoCtorIdentity = C.prototype.constructor === C;
    res[name].parent = Object.getPrototypeOf(C) === Function.prototype ? 'Function.prototype' : Object.getPrototypeOf(C).name;
    res[name].protoParent = Object.getPrototypeOf(C.prototype) === Object.prototype ? 'Object.prototype' : Object.getPrototypeOf(C.prototype).constructor.name;
    res[name].tag = Object.prototype.toString.call(C.prototype);
  }
  return JSON.stringify(res);
})()"#;

fn realm() -> (Engine, Realm) {
    let e = Engine::new().unwrap();
    e.register::<Leaf>();
    e.register::<Hidden>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    (e, r)
}

fn eval(r: &Realm, src: &str) -> String {
    let s = r.scope().unwrap();
    match s.eval(src, None, 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

/// Shape produced by eager installation (origin/main 4168ac9, before lazy
/// members), recorded with the same DUMP.
const EAGER_SHAPE: &str = include_str!("lazy_interfaces_eager_shape.json");

#[test]
fn shape_matches_eager_installation_when_dumped_first() {
    let (_e, r) = realm();
    let shape = eval(&r, DUMP);
    if std::env::var_os("LP_PRINT_SHAPE").is_some() {
        println!("SHAPE {shape}");
    }
    assert_eq!(shape, EAGER_SHAPE.trim());
}

#[test]
fn shape_is_independent_of_what_touches_a_class_first() {
    // Each scenario uses a fresh realm (one engine per thread).
    let (e, r) = realm();
    let fresh = || e.create_realm(RealmOptions::default()).unwrap();
    // 1. A wrap of the leaf class before any global lookup.
    {
        let s = r.scope().unwrap();
        let o = s.wrap::<Leaf>(Rc::new(Data { id: 5 }), None).unwrap();
        s.global().set("leaf", o.value()).unwrap();
    }
    assert_eq!(
        eval(&r, "[leaf.id, leaf.ping(), leaf.midOnly(), leaf.leafOnly(), leaf[1], [...leaf].join('+'), leaf.BASE_CONST].join()"),
        "5,pong,true,leaf,10,0+10,7"
    );
    assert_eq!(
        eval(&r, "Object.getPrototypeOf(leaf) === LzLeaf.prototype && leaf instanceof LzBase && leaf instanceof LzMid"),
        "true"
    );
    assert_eq!(eval(&r, DUMP), EAGER_SHAPE.trim());

    // 2. Reaching a parent prototype only through a child's prototype chain.
    let r2 = fresh();
    assert_eq!(
        eval(&r2, "const bp = Object.getPrototypeOf(Object.getPrototypeOf(LzLeaf.prototype)); [typeof Object.getOwnPropertyDescriptor(bp, 'id').get, bp.ping.name, Object.keys(bp).sort().join()].join('|')"),
        "function|ping|BASE_CONST,id,label,ping,twoArgs"
    );
    assert_eq!(eval(&r2, DUMP), EAGER_SHAPE.trim());

    // 3. Subclassing and construction before any other use.
    let r3 = fresh();
    assert_eq!(
        eval(&r3, "class X extends LzLeaf { constructor() { super(); this.x = 1 } } const x = new X(); [x.id, x.x, x.leafOnly(), x.ping(), x instanceof LzBase, Object.getPrototypeOf(X) === LzLeaf].join()"),
        "99,1,leaf,pong,true,true"
    );
    assert_eq!(eval(&r3, DUMP), EAGER_SHAPE.trim());

    // 4. Static call and enumeration of the interface object first.
    let r4 = fresh();
    assert_eq!(
        eval(&r4, "LzBase.create() + ':' + Object.getOwnPropertyNames(LzMid.prototype).sort().join()"),
        "static:constructor,entries,forEach,keys,length,midOnly,values"
    );
    assert_eq!(eval(&r4, DUMP), EAGER_SHAPE.trim());
}

#[test]
fn hidden_classes_install_on_wrap_and_globals_stay_data_properties() {
    let (_e, r) = realm();
    assert_eq!(eval(&r, "typeof LzHidden"), "undefined");
    let desc = "JSON.stringify(Object.getOwnPropertyDescriptor(globalThis, 'LzBase'), (k, v) => typeof v === 'function' ? 'fn' : v)";
    assert_eq!(eval(&r, desc), r#"{"value":"fn","writable":true,"enumerable":false,"configurable":true}"#);
    {
        let s = r.scope().unwrap();
        let o = s.wrap::<Hidden>(Rc::new(Data { id: 1 }), None).unwrap();
        s.global().set("h", o.value()).unwrap();
    }
    assert_eq!(eval(&r, "[h.hiddenOnly(), h.ping(), h.id, h instanceof LzBase].join()"), "1,pong,1,true");
    // Page changes to the interface bindings behave like data properties.
    assert_eq!(
        eval(&r, "LzMid = 5; const k = delete globalThis.LzLeaf; [LzMid, k, typeof globalThis.LzLeaf].join()"),
        "5,true,undefined"
    );
}

#[test]
fn realms_populate_independently_and_reentrantly() {
    let e = Engine::new().unwrap();
    e.register::<Leaf>();
    let a = e.create_realm(RealmOptions::default()).unwrap();
    let b = e.create_realm(RealmOptions { name: "world".into(), ..Default::default() }).unwrap();
    assert_eq!(eval(&a, "LzBase.prototype.ping.name"), "ping");
    // Mutations in one realm never leak into the other.
    assert_eq!(eval(&a, "LzBase.prototype.ping = 1; LzBase.extra = 2; 'ok'"), "ok");
    assert_eq!(
        eval(&b, "[typeof LzBase.prototype.ping, typeof LzBase.extra, LzBase === undefined].join()"),
        "function,undefined,false"
    );
    // A proxy trap fired while another class populates from a getter (reentrancy).
    assert_eq!(
        eval(&b, "const seen = []; Object.defineProperty(globalThis, 'probe', { get() { seen.push(Object.keys(LzMid.prototype).length); return 1 } }); [probe, Reflect.ownKeys(LzLeaf).length > 0, seen[0] > 0].join()"),
        "1,true,true"
    );
}

#[test]
fn disposed_realm_interfaces_fail_consistently_whether_used_or_not() {
    let e = Engine::new().unwrap();
    e.register::<Leaf>();
    let a = e.create_realm(RealmOptions::default()).unwrap();
    let b = e.create_realm(RealmOptions { name: "other".into(), ..Default::default() }).unwrap();
    {
        let sa = a.scope().unwrap();
        // LzBase is used (members installed) before disposal, LzLeaf is not.
        sa.eval("LzBase.prototype.ping; globalThis.leafObj = new LzLeaf()", None, 1).unwrap();
        let sb = b.scope().unwrap();
        for name in ["LzBase", "LzLeaf", "leafObj"] {
            sb.global().set(&format!("foreign{name}"), sa.global().get(name).unwrap()).unwrap();
        }
    }
    a.dispose();
    let probe = |expr: &str| eval(&b, &format!("try {{ {expr}; 'ok' }} catch (e) {{ e.message }}"));
    for iface in ["foreignLzBase", "foreignLzLeaf"] {
        for op in [
            format!("{iface}.prototype"),
            format!("{iface}.name"),
            format!("Reflect.ownKeys({iface})"),
            format!("'x' in {iface}"),
            format!("({{}}) instanceof {iface}"),
            format!("{iface}.y = 1"),
            format!("Object.getOwnPropertyDescriptor({iface}, 'prototype')"),
            format!("Object.preventExtensions({iface})"),
        ] {
            assert_eq!(probe(&op), "Execution context was destroyed.", "{op}");
        }
        // Operations that never touch members are plain forwards.
        assert_eq!(probe(&format!("Object.getPrototypeOf({iface})")), "ok");
        assert!(probe(&format!("new {iface}()")).contains("Execution context was destroyed"));
    }
    // Instances made before disposal keep their (installed) members; calling
    // native members throws like every other native call into a disposed realm.
    assert_eq!(probe("typeof foreignleafObj.leafOnly"), "ok");
    assert_eq!(probe("foreignleafObj.leafOnly()"), "Execution context was destroyed.");
    drop(b);
    drop(a);
    drop(e);
    let _again = Engine::new().expect("engine slot free after teardown");
}

#[test]
fn proxy_traps_preserve_reflection_extensibility_mutation_and_instanceof() {
    let e = Engine::new().unwrap();
    e.register::<Leaf>();
    let fresh = || e.create_realm(RealmOptions::default()).unwrap();
    // preventExtensions / freeze before any other use: members exist first.
    let r = fresh();
    assert_eq!(
        eval(&r, "Object.preventExtensions(LzBase); [Object.isExtensible(LzBase), typeof LzBase.create, LzBase.BASE_CONST, typeof LzBase.prototype.ping].join()"),
        "false,function,7,function"
    );
    let r = fresh();
    assert_eq!(
        eval(&r, "Object.freeze(LzMid); [Object.isFrozen(LzMid), Object.isFrozen(LzMid.prototype), typeof LzMid.prototype.midOnly, Object.getOwnPropertyDescriptor(LzMid, 'prototype').writable].join()"),
        "true,false,function,false"
    );
    // isExtensible / getPrototypeOf do not need members and do not install them;
    // later use still sees the full API.
    let r = fresh();
    assert_eq!(
        eval(&r, "[Object.isExtensible(LzLeaf), Reflect.getPrototypeOf(LzLeaf) === LzMid, typeof LzLeaf.prototype.leafOnly, typeof LzLeaf.prototype.ping].join()"),
        "true,true,function,function"
    );
    // Mutation and deletion behave as on ordinary data properties.
    let r = fresh();
    assert_eq!(
        eval(&r, "LzBase.create = 1; const a = LzBase.create; const d1 = delete LzBase.create; const d2 = delete LzBase.BASE_CONST; [a, d1, 'create' in LzBase, d2, LzBase.BASE_CONST].join()"),
        "1,true,false,false,7"
    );
    assert_eq!(
        eval(&r, "(() => { 'use strict'; try { delete LzBase.BASE_CONST; return 'no' } catch (e) { return e.constructor.name } })()"),
        "TypeError"
    );
    let r = fresh();
    assert_eq!(
        eval(&r, "Object.defineProperty(LzLeaf, 'x', { value: 1 }); [LzLeaf.x, typeof LzLeaf.prototype.leafOnly, Object.keys(LzBase).sort().join('+')].join()"),
        "1,function,BASE_CONST+create"
    );
    // instanceof, including a page-defined Symbol.hasInstance and subclasses.
    let r = fresh();
    assert_eq!(
        eval(&r, "const l = new LzLeaf(); class S extends LzMid {}; [l instanceof LzLeaf, l instanceof LzBase, ({}) instanceof LzBase, Object.create(LzMid.prototype) instanceof LzMid, typeof S.prototype.midOnly].join()"),
        "true,true,false,true,function"
    );
    assert_eq!(
        eval(&r, "Object.defineProperty(LzBase, Symbol.hasInstance, { value: () => true }); [({}) instanceof LzBase, 1 instanceof LzBase].join()"),
        "true,true"
    );
    // Calling an interface object without `new` still fails as before.
    assert!(eval(&r, "try { LzLeaf() } catch (e) { e.constructor.name }").contains("TypeError"));
}

#[test]
fn interface_objects_report_native_source() {
    let (_e, r) = realm();
    assert_eq!(eval(&r, "Function.prototype.toString.call(LzBase).includes('[native code]')"), "true");
    assert_eq!(
        eval(&r, "[typeof LzLeaf, LzLeaf.name, LzLeaf.length, Object.isExtensible(LzLeaf)].join()"),
        "function,LzLeaf,1,true"
    );
}
