//! Lazy member installation: interface objects, their prototypes and members
//! must have exactly the descriptor shape eager installation produced,
//! whatever touches a class first. Documented differences that remain
//! (Function.prototype.toString of interface objects, proxy-trap cost) are
//! listed in docs/COMPATIBILITY.md and pinned below where observable.

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

/// Mirrors foreign interfaces into the probing realm under the test names (as
/// non-enumerable data properties, like real interface bindings).
const IMPORT: &str = r#"(src) => { for (const n of ['LzBase', 'LzMid', 'LzLeaf']) Object.defineProperty(globalThis, n, { value: src[n], writable: true, enumerable: false, configurable: true }); }"#;

fn export(r: &Realm) -> String {
    eval(r, "globalThis.__exported = { LzBase, LzMid, LzLeaf }; 'ok'")
}

#[test]
fn disposed_realm_interfaces_keep_the_eager_api_whether_used_or_not() {
    let e = Engine::new().unwrap();
    e.register::<Leaf>();
    // Expected: interfaces of a live realm, seen from another realm.
    let live = e.create_realm(RealmOptions::default()).unwrap();
    let probe_live = e.create_realm(RealmOptions { name: "probe-live".into(), ..Default::default() }).unwrap();
    let a = e.create_realm(RealmOptions::default()).unwrap();
    let probe = e.create_realm(RealmOptions { name: "probe".into(), ..Default::default() }).unwrap();
    for (src, dst) in [(&live, &probe_live), (&a, &probe)] {
        export(src);
        let so = src.scope().unwrap();
        let exported = so.global().get("__exported").unwrap();
        let sd = dst.scope().unwrap();
        let import = sd.eval(IMPORT, None, 1).unwrap().as_object().unwrap();
        import.call(None, &[exported]).unwrap();
    }
    // Realm `a`: only LzBase is touched before disposal (members installed);
    // LzMid and LzLeaf are never touched, so their members are never
    // installed before disposal (exporting reads the global bindings only,
    // which does not go through the interface proxies).
    {
        let sa = a.scope().unwrap();
        sa.eval("LzBase.prototype.ping", None, 1).unwrap();
    }
    assert_eq!(eval(&probe, "globalThis.pingBefore = LzBase.prototype.ping; 'ok'"), "ok");
    // A separate realm provides an instance made before its disposal (a
    // construction populates the whole chain, so it is kept out of `a`).
    let a2 = e.create_realm(RealmOptions { name: "with-instance".into(), ..Default::default() }).unwrap();
    {
        let s2 = a2.scope().unwrap();
        let inst = s2.eval("new LzLeaf()", None, 1).unwrap();
        probe.scope().unwrap().global().set("foreignInst", inst).unwrap();
    }
    a.dispose();
    a2.dispose();
    // The never-populated LzMid/LzLeaf and the populated LzBase all expose the
    // same API as live interfaces.
    assert_eq!(eval(&probe, DUMP), eval(&probe_live, DUMP));
    // Members installed before disposal keep their identity.
    assert_eq!(eval(&probe, "LzBase.prototype.ping === pingBefore"), "true");
    // Constants are plain data; native members throw like any native call
    // into a disposed realm, as with eager installation.
    assert_eq!(
        eval(&probe, "[LzBase.BASE_CONST, LzMid.prototype.BASE_CONST, typeof LzLeaf.prototype.leafOnly].join()"),
        "7,7,function"
    );
    assert_eq!(
        eval(&probe, "try { LzLeaf.prototype.leafOnly.call(foreignInst) } catch (e) { e.message }"),
        "Execution context was destroyed."
    );
    assert_eq!(eval(&probe, "try { LzBase.create() } catch (e) { e.message }"), "Execution context was destroyed.");
    assert!(eval(&probe, "try { new LzLeaf(); 'constructed' } catch (e) { e.message }")
        .contains("Execution context was destroyed"));
    // Instances made before disposal: members readable, native calls throw.
    assert_eq!(eval(&probe, "[typeof foreignInst.midOnly, typeof foreignInst.ping].join()"), "function,function");
    assert_eq!(
        eval(&probe, "try { foreignInst.midOnly() } catch (e) { e.message }"),
        "Execution context was destroyed."
    );
    drop(a2);
    drop(probe);
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

/// Index keys reach the indexed getter however script spells them (the
/// patched JSC takes them to a numeric callback; the stock framework parses
/// the name), and the named lookups around them stay ordinary.
#[test]
fn indexed_properties_answer_every_index_key_form() {
    let (_e, r) = realm();
    {
        let s = r.scope().unwrap();
        let o = s.wrap::<Leaf>(Rc::new(Data { id: 5 }), None).unwrap();
        s.global().set("leaf", o.value()).unwrap();
    }
    let checks = [
        ("leaf[0] + ',' + leaf['1'] + ',' + leaf[2] + ',' + leaf['01'] + ',' + leaf[-1]", "0,10,undefined,undefined,undefined"),
        ("[('0' in leaf), (1 in leaf), ('2' in leaf), leaf.hasOwnProperty('1'), leaf.hasOwnProperty(2)].join()", "true,true,false,true,false"),
        ("JSON.stringify(Object.getOwnPropertyDescriptor(leaf, '1'))", r#"{"value":10,"writable":false,"enumerable":false,"configurable":true}"#),
        ("Object.getOwnPropertyDescriptor(leaf, '5')", "undefined"),
        ("Object.keys(leaf).join() + '|' + Object.getOwnPropertyNames(leaf).join()", "0,1|0,1"),
        ("[...leaf].join('+') + '|' + Array.from(leaf).length + '|' + leaf.length", "0+10|2|2"),
        // Indexed properties without a setter ignore sloppy writes and take no own property.
        ("leaf[0] = 7; leaf['1'] = 8; [leaf[0], leaf[1], leaf.hasOwnProperty('0')].join()", "0,10,true"),
        ("(() => { 'use strict'; try { leaf[0] = 7; return 'no throw'; } catch (e) { return e.constructor.name; } })()", "no throw"),
        // Expandos and named members are unaffected.
        ("leaf.foo = 3; leaf.foo + ',' + typeof leaf.midOnly + ',' + leaf.midOnly()", "3,function,true"),
        ("Object.getOwnPropertyDescriptor(LzMid.prototype, 'length').get.call(leaf)", "2"),
        ("Reflect.get(LzMid.prototype, '1', leaf)", "undefined"),
    ];
    for (src, want) in checks {
        assert_eq!(eval(&r, src), want, "{src}");
    }
}

/// `Scope::after_populate`: the embedder's step for a class runs once, when
/// the class is first populated however that happens (a wrap, a trap of its
/// interface object, a subclass), and at once for a class already populated.
#[test]
fn after_populate_runs_once_with_the_population() {
    let (_e, r) = realm();
    let s = r.scope().unwrap();
    let hook = |tag: &str| {
        let src = format!("(proto, ctor) => {{ globalThis.log = (globalThis.log || '') + '{tag}:' + ctor.name + ':' + (proto === ctor.prototype) + ';'; }}");
        s.eval(&src, None, 1).unwrap().as_object().unwrap()
    };
    s.after_populate::<Base>(hook("base")).unwrap();
    s.after_populate::<Hidden>(hook("hidden")).unwrap();
    assert_eq!(eval(&r, "String(globalThis.log)"), "undefined", "nothing populated yet");
    // A trap of a subclass's interface object populates the chain, root first.
    assert_eq!(eval(&r, "typeof LzMid.prototype.midOnly"), "function");
    assert_eq!(eval(&r, "globalThis.log"), "base:LzBase:true;");
    // A wrap of a hidden class (no global name) populates it.
    let o = s.wrap::<Hidden>(Rc::new(Data { id: 1 }), None).unwrap();
    s.global().set("hidden", o.value()).unwrap();
    assert_eq!(eval(&r, "globalThis.log"), "base:LzBase:true;hidden:LzHidden:true;");
    assert_eq!(eval(&r, "Object.getPrototypeOf(hidden) === Object.getPrototypeOf(hidden).constructor.prototype"), "true");
    // Populated already: runs at once.
    s.after_populate::<Mid>(hook("mid")).unwrap();
    assert_eq!(eval(&r, "globalThis.log"), "base:LzBase:true;hidden:LzHidden:true;mid:LzMid:true;");
    assert_eq!(eval(&r, "new LzLeaf(); globalThis.log.split(';').length"), "4");
}

/// Interfaces are populated (their prototypes made, members installed) when a
/// page first uses them, after its scripts ran: a page that put `get`/`value`
/// on Object.prototype or replaced Array iteration must not break that.
#[test]
fn population_after_page_changed_intrinsics_installs_the_eager_shape() {
    let (e, r) = realm();
    // The same engine's untouched installation (a realm of the stock
    // framework has its own setter lengths, see the eager shape tests).
    let clean = e.create_realm(RealmOptions::default()).unwrap();
    eval(
        &r,
        "const d = Object.defineProperty;
         d(Object.prototype, 'get', { __proto__: null, value: function () { return 'polluted'; }, configurable: true, writable: true });
         d(Object.prototype, 'value', { __proto__: null, value: 'polluted', configurable: true, writable: true });
         globalThis.realIterator = Array.prototype[Symbol.iterator];
         d(Array.prototype, Symbol.iterator, { __proto__: null, value: function () { throw new Error('iterated'); }, configurable: true, writable: true });
         'ok'",
    );
    // First use of every interface happens here, with the intrinsics changed.
    assert_eq!(eval(&r, "typeof LzLeaf.prototype.leafOnly + typeof LzMid.prototype.midOnly"), "functionfunction");
    eval(
        &r,
        "delete Object.prototype.get; delete Object.prototype.value;
         Object.defineProperty(Array.prototype, Symbol.iterator, { value: realIterator, configurable: true, writable: true }); 'ok'",
    );
    assert_eq!(eval(&r, DUMP), eval(&clean, DUMP));
}

/// Population runs after page script, so nothing it does may reach code a
/// page put on Array.prototype (indexed accessors over the helper's tables)
/// or Object.prototype (accessors named like members, which an assignment
/// would call; proxy traps the interface objects do not define).
#[test]
fn population_after_page_changed_prototypes_runs_no_page_code() {
    let (e, r) = realm();
    let clean = e.create_realm(RealmOptions::default()).unwrap();
    eval(
        &r,
        "globalThis.seen = [];
         const d = Object.defineProperty;
         for (let i = 0; i < 2000; i++) d(Array.prototype, i, { configurable: true,
           get() { seen.push('read ' + i); return function () { seen.push('called'); }; },
           set(v) { seen.push('write ' + i); } });
         for (const k of ['ping', 'twoArgs', 'midOnly', 'leafOnly', 'create']) d(Object.prototype, k, { configurable: true,
           get() { return undefined; }, set(v) { seen.push('set ' + k); } });
         Object.prototype.apply = function (t) { seen.push('apply ' + t.name); return 'hijacked'; };
         Object.prototype.construct = function (t) { seen.push('construct ' + t.name); return {}; };
         'ok'",
    );
    // First use of every interface happens here.
    assert_eq!(
        eval(&r, "const l = new LzLeaf(); [typeof l.leafOnly, typeof LzMid.prototype.midOnly, Object.getPrototypeOf(l) === LzLeaf.prototype].join()"),
        "function,function,true"
    );
    {
        let s = r.scope().unwrap();
        let o = s.wrap::<Hidden>(Rc::new(Data { id: 1 }), None).unwrap();
        s.global().set("hidden", o.value()).unwrap();
    }
    assert_eq!(eval(&r, "try { LzBase(); 'called' } catch (e) { e.constructor.name }"), "TypeError");
    assert_eq!(eval(&r, "seen.join()"), "");
    eval(
        &r,
        "for (let i = 0; i < 2000; i++) delete Array.prototype[i];
         for (const k of ['ping', 'twoArgs', 'midOnly', 'leafOnly', 'create', 'apply', 'construct']) delete Object.prototype[k];
         'ok'",
    );
    assert_eq!(eval(&r, "[hidden.hiddenOnly(), LzBase.create()].join()"), "1,static");
    assert_eq!(eval(&r, DUMP), eval(&clean, DUMP));
}

/// A first use that fails part way (the stack runs out somewhere in the
/// population, at a different point for each attempt here) is completed by
/// the next use: no prototype is left without its constructor, its tag or a
/// member.
#[test]
fn population_failing_part_way_is_completed_by_the_next_use() {
    let (e, r) = realm();
    let clean = e.create_realm(RealmOptions::default()).unwrap();
    let outcome = eval(
        &r,
        "(() => {
           const names = ['LzBase', 'LzMid', 'LzLeaf'];
           let next = 0, failed = 0;
           function touch() { if (next >= names.length) return; try { void globalThis[names[next]].prototype; next++; } catch (e) { failed++; } }
           function probe(k) { try { Reflect.apply(touch, null, new Array(k).fill(0)); } catch (e) { failed++; } }
           function deeper(n) { try { deeper(n + 1); } catch (e) {} if (next < names.length) for (let k = 0; k < 64; k++) probe(k); }
           for (let round = 0; round < 50 && next < names.length; round++) deeper(0);
           return next + ',' + (failed > 0);
         })()",
    );
    assert_eq!(outcome, "3,true");
    assert_eq!(eval(&r, DUMP), eval(&clean, DUMP));
}
