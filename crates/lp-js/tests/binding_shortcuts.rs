//! Binding APIs that skip a lookup: receiver-aware members, constant keys
//! (atoms), registry-free unwraps and cross-realm rehoming.

use lp_js::*;
use std::cell::Cell;

struct BaseData {
    id: u32,
    calls: Cell<u32>,
}

struct Base;
impl JsClass for Base {
    const NAME: &'static str = "Base";
    type Data = BaseData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        // The receiver is the wrapper itself: store state on it without
        // finding it again.
        def.method_this("remember", 1, |s, this, me, args| {
            this.calls.set(this.calls.get() + 1);
            s.set_hidden(me, "remembered", args.get(0))?;
            Ok(me.value())
        });
        def.getter_this("recall", |s, _this, me| Ok(s.get_hidden(me, "remembered")?.unwrap_or_else(|| s.null())));
        def.pure_method_this("me", 0, |_s, _this, me, _args| Ok(me.value()));
        def.pure_getter_this_typed("self", GetterResult::ObjectOrNull, |_s, _this, me| Ok(me.value()));
        def.getter("id", |s, this| Ok(s.number(f64::from(this.id))));
    }
}

struct Derived;
impl JsClass for Derived {
    const NAME: &'static str = "Derived";
    type Data = BaseData;
    type Parent = Base;
    fn define(_def: &mut ClassDef<Self>) {}
}

fn data(id: u32) -> BaseData {
    BaseData { id, calls: Cell::new(0) }
}

#[test]
fn receiver_aware_members_get_their_own_wrapper() {
    let engine = Engine::new().unwrap();
    engine.register::<Derived>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let a = s.wrap::<Base>(data(1), None).unwrap();
    let b = s.wrap::<Derived>(data(2), None).unwrap();
    s.global().set_atom("a", a.value()).unwrap();
    s.global().set_atom("b", b.value()).unwrap();
    // Enough iterations for the JIT tiers, which call pure getters through
    // their own entry point.
    let ok = s
        .eval(
            "let ok = true;
             for (let i = 0; i < 20000; i++) {
               ok = ok && a.me() === a && a.self === a && b.me() === b && b.self === b;
             }
             ok && a.remember('x') === a && b.remember(7) === b && a.recall === 'x' && b.recall === 7
               && Object.getOwnPropertyDescriptor(Base.prototype, 'recall').get.call(b) === 7",
            None,
            1,
        )
        .unwrap();
    assert!(ok.to_bool());
    assert_eq!(s.unwrap::<Base>(a.value()).unwrap().calls.get(), 1);
    // A brand mismatch still throws.
    let e = s.eval("try { Base.prototype.me.call({}); 'no' } catch (e) { e.constructor.name }", None, 1).unwrap();
    assert_eq!(e.to_string().unwrap(), "TypeError");
}

#[test]
fn receiver_aware_members_run_in_the_receivers_realm() {
    let engine = Engine::new().unwrap();
    engine.register::<Derived>();
    let main = engine.create_realm(RealmOptions::default()).unwrap();
    let other = engine.create_realm(RealmOptions { name: "other".into(), ..Default::default() }).unwrap();
    let s = main.scope().unwrap();
    let o = other.scope().unwrap();
    let foreign = o.wrap::<Base>(data(3), None).unwrap();
    // A value of another realm of the engine, entered here without a Persistent.
    let foreign = s.rehome_object(foreign);
    assert_eq!(foreign.value().ctx(), s.ctx());
    s.global().set_atom("foreign", foreign.value()).unwrap();
    let r = s
        .eval(
            "const d = Object.getOwnPropertyDescriptor(Base.prototype, 'recall');
             Base.prototype.remember.call(foreign, 'f') === foreign && d.get.call(foreign) === 'f'
               && Base.prototype.me.call(foreign) === foreign",
            None,
            1,
        )
        .unwrap();
    assert!(r.to_bool());
    // The hidden property went onto the foreign wrapper itself.
    let back = o.rehome(s.global().get_atom("foreign").unwrap());
    let hidden = o.get_hidden(back.as_object().unwrap(), "remembered").unwrap().unwrap();
    assert_eq!(hidden.to_string().unwrap(), "f");
}

#[test]
fn constant_keys_and_registry_free_unwraps() {
    let engine = Engine::new().unwrap();
    engine.register::<Derived>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let o = s.object();
    o.set_atom("answer", s.number(42.0)).unwrap();
    assert!(o.has_atom("answer"));
    assert!(!o.has_atom("question"));
    assert_eq!(o.get_atom("answer").unwrap().to_number().unwrap(), 42.0);
    assert_eq!(o.get("answer").unwrap().to_number().unwrap(), 42.0);
    o.set_atom_with("hidden", s.boolean(true), sys::kJSPropertyAttributeDontEnum).unwrap();
    assert_eq!(o.keys(), vec!["answer".to_string()]);

    let w = s.wrap::<Derived>(data(9), None).unwrap();
    // A subclass instance unwraps as itself and as its parent; others do not.
    assert_eq!(s.unwrap::<Derived>(w.value()).map(|d| d.id), Some(9));
    assert_eq!(s.unwrap::<Base>(w.value()).map(|d| d.id), Some(9));
    let base = s.wrap::<Base>(data(10), None).unwrap();
    assert!(s.unwrap::<Derived>(base.value()).is_none());
    assert!(s.unwrap::<Base>(o.value()).is_none());
    assert!(s.unwrap::<Base>(s.number(1.0)).is_none());
    // SAFETY: `w` is held in a local for the whole borrow.
    assert_eq!(unsafe { s.unwrap_rooted::<Base>(w.value()) }.map(|d| d.id), Some(9));
    // Hidden values under a constant key and under a computed one are the same property.
    s.set_hidden(w, "slot", s.number(5.0)).unwrap();
    let key = String::from("slot");
    assert_eq!(s.get_hidden_str(w, &key).unwrap().unwrap().to_number().unwrap(), 5.0);
    assert!(s.get_hidden(w, "other").unwrap().is_none());
}

#[test]
#[should_panic(expected = "another engine")]
fn rehome_refuses_values_of_another_engine() {
    let engine = Engine::new().unwrap();
    let a = engine.create_realm(RealmOptions::default()).unwrap();
    let sa = a.scope().unwrap();
    // A second engine on another thread could not hand us values; a context
    // group of our own, made through the raw API, stands in for one.
    unsafe {
        let group = sys::JSContextGroupCreate();
        let ctx = sys::JSGlobalContextCreateInGroup(group, std::ptr::null());
        let v = Value::from_raw(ctx, sys::JSValueMakeNumber(ctx, 1.0));
        let _ = sa.rehome(v);
    }
}

/// A class whose data embeds its parent's: members of `Base` reach it
/// through an upcast, members of `Wide` directly.
struct WideData {
    base: BaseData,
    extra: u32,
}

struct Wide;
impl JsClass for Wide {
    const NAME: &'static str = "Wide";
    type Data = WideData;
    type Parent = Derived;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w: &WideData| &w.base);
        def.pure_getter_typed("extra", GetterResult::Number, |s, w| Ok(s.number(f64::from(w.extra))));
    }
}

/// Lean members of every kind: a pure getter, a pure method and an indexed
/// getter.
struct Lean;
impl JsClass for Lean {
    const NAME: &'static str = "Lean";
    type Data = BaseData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_getter_typed("leanId", GetterResult::Number, |s, d| Ok(s.number(f64::from(d.id))));
        def.pure_method("twice", 0, |s, d, _a| Ok(s.number(f64::from(d.id * 2))));
        def.indexed(|s, d, i| Ok((i < 2).then(|| s.number(f64::from(d.id + i)))));
    }
}

#[test]
fn brands_hold_through_depth_and_upcasts() {
    let engine = Engine::new().unwrap();
    engine.register::<Wide>();
    engine.register::<Lean>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let w = s.wrap::<Wide>(WideData { base: data(4), extra: 11 }, None).unwrap();
    let d = s.wrap::<Derived>(data(5), None).unwrap();
    let l = s.wrap::<Lean>(data(6), None).unwrap();
    s.global().set_atom("w", w.value()).unwrap();
    s.global().set_atom("d", d.value()).unwrap();
    s.global().set_atom("l", l.value()).unwrap();
    // Through the JIT tiers too: members of the root class on a grandchild
    // whose data is a projection, members of the class itself, and refusals
    // of an unrelated class at the same depth and of a shallower one.
    let r = s
        .eval(
            "const extra = Object.getOwnPropertyDescriptor(Wide.prototype, 'extra').get;
             const id = Object.getOwnPropertyDescriptor(Base.prototype, 'id').get;
             const leanId = Object.getOwnPropertyDescriptor(Lean.prototype, 'leanId').get;
             let ok = true;
             for (let i = 0; i < 20000; i++) {
               ok = ok && w.id === 4 && w.extra === 11 && w.me() === w && d.id === 5 && l.leanId === 6;
             }
             const refused = f => { try { f(); return false } catch (e) { return e instanceof TypeError } };
             ok && refused(() => extra.call(d)) && refused(() => leanId.call(w)) && refused(() => id.call(l))
               && refused(() => Lean.prototype.twice.call(d))",
            None,
            1,
        )
        .unwrap();
    assert!(r.to_bool());
    assert_eq!(s.unwrap::<Base>(w.value()).map(|d| d.id), Some(4));
    assert_eq!(s.unwrap::<Wide>(w.value()).map(|w| w.extra), Some(11));
    assert!(s.unwrap::<Wide>(d.value()).is_none());
    assert!(s.unwrap::<Lean>(w.value()).is_none());
}

#[test]
fn lean_members_of_a_disposed_or_dropped_realm_refuse_safely() {
    let engine = Engine::new().unwrap();
    engine.register::<Lean>();
    let main = engine.create_realm(RealmOptions::default()).unwrap();
    let s = main.scope().unwrap();
    let read = "(() => { try { return [o.leanId, o.twice(), o[1]].join() } catch (e) { return e.message } })()";
    for round in 0..2 {
        let other = engine.create_realm(RealmOptions { name: "other".into(), ..Default::default() }).unwrap();
        {
            let o = other.scope().unwrap();
            let w = o.wrap::<Lean>(data(20), None).unwrap();
            s.global().set_atom("o", s.rehome_object(w).value()).unwrap();
        }
        // Warm every lean entry (inline caches and JIT) while the realm lives.
        let warm = s
            .eval(&format!("var r; for (let i = 0; i < 20000; i++) r = {read}; r"), None, 1)
            .unwrap();
        assert_eq!(warm.to_string().unwrap(), "20,40,21");
        if round == 0 {
            other.dispose();
        } else {
            // The last handle: the realm itself is freed, its wrapper lives on.
            drop(other);
        }
        engine.collect_garbage_now();
        engine.drain_finalized();
        for _ in 0..3 {
            let after = s.eval(&format!("var r2; for (let i = 0; i < 2000; i++) r2 = {read}; r2"), None, 1).unwrap();
            assert_ne!(after.to_string().unwrap(), "20,40,21", "round {round}: a disposed realm's member answered");
        }
    }
}

// ---- review/bind: adversarial cases -------------------------------------

thread_local! {
    static DOOMED: std::cell::RefCell<Option<Realm>> = const { std::cell::RefCell::new(None) };
    static CONVERSIONS: Cell<u32> = const { Cell::new(0) };
}

/// A pure method whose argument conversion can run page script (as
/// `dom_string` does on an object argument), then uses its scope's realm
/// (as every member returning a new wrapper does).
struct Coerce;
impl JsClass for Coerce {
    const NAME: &'static str = "Coerce";
    type Data = BaseData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_method("make", 1, |s, d, a| {
            let n = a.get(0).to_string()?;
            // Uses the scope's realm after the conversion: a clone of its Rc,
            // and a new wrapper in it.
            let r = s.realm();
            drop(r);
            let w = s.wrap::<Lean>(data(d.id + n.len() as u32), None)?;
            Ok(w.value())
        });
        def.pure_method("echo", 1, |s, _d, a| {
            let n = a.get(0).to_string()?;
            Ok(s.string(&n))
        });
        def.method("echoFull", 1, |s, _d, a| {
            let n = a.get(0).to_string()?;
            Ok(s.string(&n))
        });
        def.pure_method("fail", 1, |_s, d, a| {
            d.calls.set(d.calls.get() + 1);
            let _ = a.get(0).to_number()?;
            Err(JsError::dom_named("IndexSizeError", "offset out of range"))
        });
        def.method("failFull", 1, |_s, d, a| {
            d.calls.set(d.calls.get() + 1);
            let _ = a.get(0).to_number()?;
            Err(JsError::dom_named("IndexSizeError", "offset out of range"))
        });
        def.method("calls", 0, |s, d, _a| Ok(s.number(f64::from(d.calls.get()))));
        def.pure_method("num", 1, |s, _d, a| Ok(s.number(a.get(0).to_number()?)));
        def.method("numFull", 1, |s, _d, a| Ok(s.number(a.get(0).to_number()?)));
    }
}

/// B2 x C1: the lean dispatch borrows the receiver's realm without a
/// count, on the premise that a pure member runs no script. Argument
/// conversion does run script; when that script drops the receiver realm's
/// last handle, the member continues on a freed realm (base held a strong
/// count for the whole call).
#[test]
fn review_realm_dropped_by_argument_conversion_inside_a_pure_call() {
    let engine = Engine::new().unwrap();
    engine.register::<Lean>();
    engine.register::<Coerce>();
    let main = engine.create_realm(RealmOptions::default()).unwrap();
    let s = main.scope().unwrap();
    let kill = s
        .function("killOther", |s, _, _| {
            DOOMED.with(|d| d.borrow_mut().take());
            Ok(s.undefined())
        })
        .unwrap();
    s.global().set_atom("killOther", kill.value()).unwrap();
    for round in 0..4 {
        let other = engine.create_realm(RealmOptions { name: format!("other{round}"), ..Default::default() }).unwrap();
        {
            let o = other.scope().unwrap();
            let w = o.wrap::<Coerce>(data(1), None).unwrap();
            s.global().set_atom("c", s.rehome_object(w).value()).unwrap();
        }
        DOOMED.with(|d| *d.borrow_mut() = Some(other));
        // Warm (JIT) with a harmless argument, then drop the realm from
        // inside the conversion.
        let r = s
            .eval(
                "(() => { let k = 0; for (let i = 0; i < 20000; i++) k += c.make('ab') ? 1 : 0;
                 let out;
                 try { out = c.make({ toString() { killOther(); return 'xyz' } }); out = typeof out }
                 catch (e) { out = 'threw ' + e.message }
                 return k + ':' + out })()",
                None,
                1,
            )
            .unwrap();
        // Base keeps the realm alive for the call (a strong count) and
        // answers; the branch must not touch the freed realm.
        assert_eq!(r.to_string().unwrap(), "20000:object", "round {round}");
        engine.collect_garbage_now();
        engine.drain_finalized();
    }
}

/// C1 "double toString": a conversion that throws is rerun by the full
/// dispatch. Count how many times page script ran for one call.
#[test]
fn review_throwing_conversion_runs_once() {
    let engine = Engine::new().unwrap();
    engine.register::<Coerce>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let w = s.wrap::<Coerce>(data(1), None).unwrap();
    s.global().set_atom("c", w.value()).unwrap();
    let r = s
        .eval(
            "let n = 0, m = 0;
             try { c.echo({ toString() { n++; throw new Error('boom') } }) } catch (e) {}
             const v = c.echo({ toString() { m++; return 'ok' } });
             n + ',' + m + ',' + v",
            None,
            1,
        )
        .unwrap();
    assert_eq!(r.to_string().unwrap(), "1,1,ok", "toString calls for one throwing / one plain call");
}

/// Primitive arguments stay on the lean path: their conversion runs no
/// script. A Symbol (which cannot become a string) throws the same TypeError
/// as the full dispatch, and a BigInt converts to a number the same way on
/// both paths, before and after the JIT tiers take over.
#[test]
fn unconvertible_primitive_arguments_throw_as_the_full_dispatch_does() {
    let engine = Engine::new().unwrap();
    engine.register::<Coerce>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let w = s.wrap::<Coerce>(data(1), None).unwrap();
    s.global().set_atom("c", w.value()).unwrap();
    let r = s
        .eval(
            "(() => {
               const err = f => { try { f(); return 'no throw' } catch (e) { return e.constructor.name + ': ' + e.message } };
               const out = [];
               for (let i = 0; i < 20000; i++) {
                 if (c.echo(i & 7) !== String(i & 7) || c.num(i & 7) !== (i & 7)) return 'bad ' + i;
                 if (i % 4999 === 0) {
                   const sym = Symbol('x');
                   const a = err(() => c.echo(sym)), b = err(() => c.echoFull(sym));
                   const x = err(() => c.num(5n)), y = err(() => c.numFull(5n));
                   if (a !== b || x !== y) return a + ' | ' + b + ' | ' + x + ' | ' + y;
                   out.push(a, x);
                 }
               }
               return out.slice(0, 2).join(' / ');
             })()",
            None,
            1,
        )
        .unwrap()
        .to_string()
        .unwrap();
    assert_eq!(r, "TypeError: Cannot convert a symbol to a string / no throw");
}

/// A pure operation that throws runs once: the lean dispatch throws its own
/// error (the full dispatch would run the same body in the same realm), and
/// the error is the one the full dispatch throws.
#[test]
fn a_throwing_pure_operation_runs_once_and_throws_as_the_full_dispatch_does() {
    let engine = Engine::new().unwrap();
    engine.register::<Coerce>();
    let realm = engine.create_realm(RealmOptions::default()).unwrap();
    let s = realm.scope().unwrap();
    let w = s.wrap::<Coerce>(data(1), None).unwrap();
    s.global().set_atom("c", w.value()).unwrap();
    let r = s
        .eval(
            "(() => {
               const err = f => { try { f(); return 'no throw' } catch (e) { return e.constructor.name + '/' + e.name + ': ' + e.message } };
               for (let i = 0; i < 20000; i++) {
                 const before = c.calls();
                 const a = err(() => c.fail(i));
                 const mid = c.calls();
                 const b = err(() => c.failFull(i));
                 const after = c.calls();
                 if (a !== b || mid - before !== 1 || after - mid !== 1) return i + ': ' + a + ' | ' + b + ' | ' + (mid - before);
               }
               return err(() => c.fail(0));
             })()",
            None,
            1,
        )
        .unwrap()
        .to_string()
        .unwrap();
    assert!(r.contains("IndexSizeError: offset out of range"), "{r}");
}
