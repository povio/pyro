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
