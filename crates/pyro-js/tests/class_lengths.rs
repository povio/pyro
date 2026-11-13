//! Zero-length installation must preserve the same eager interface descriptors.
use pyro_js::*;

struct Base;
impl JsClass for Base {
    const NAME: &'static str = "LengthBase";
    type Data = ();
    type Parent = NoParent;
    fn define(_: &mut ClassDef<Self>) {}
}

struct Zero;
impl JsClass for Zero {
    const NAME: &'static str = "LengthZero";
    type Data = ();
    type Parent = Base;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, target, _| s.wrap_new::<Self>((), target, None));
        def.method("zero", 0, |s, _, _| Ok(s.number(42.0)));
        def.method("two", 2, |s, _, _| Ok(s.number(2.0)));
        def.static_method("staticZero", 0, |s, _| Ok(s.number(7.0)));
        def.static_method("staticTwo", 2, |s, _| Ok(s.number(2.0)));
    }
}

struct Two;
impl JsClass for Two {
    const NAME: &'static str = "LengthTwo";
    type Data = ();
    type Parent = Zero;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(2, |s, target, _| s.wrap_new::<Self>((), target, None));
    }
}

#[test]
fn zero_and_nonzero_lengths_keep_eager_descriptors_and_realm_isolation() {
    let engine = Engine::new().unwrap();
    engine.register::<Two>();
    let first = engine.create_realm(Default::default()).unwrap();
    let second = engine.create_realm(Default::default()).unwrap();
    let check = r#"(() => {
      const desc = Object.getOwnPropertyDescriptor(globalThis, 'LengthZero');
      if (!desc || typeof desc.value !== 'function' || desc.get || !desc.writable || desc.enumerable || !desc.configurable) return false;
      const C = desc.value;
      for (const [fn, length, name] of [
        [C, 0, 'LengthZero'], [LengthBase, 0, 'LengthBase'], [LengthTwo, 2, 'LengthTwo'],
        [C.prototype.zero, 0, 'zero'], [C.prototype.two, 2, 'two'],
        [C.staticZero, 0, 'staticZero'], [C.staticTwo, 2, 'staticTwo']
      ]) {
        const d = Object.getOwnPropertyDescriptor(fn, 'length');
        if (!d || d.value !== length || d.writable || d.enumerable || !d.configurable || fn.name !== name) return false;
      }
      const method = Object.getOwnPropertyDescriptor(C.prototype, 'zero');
      if (!method.writable || !method.enumerable || !method.configurable) return false;
      if (Object.getPrototypeOf(C) !== LengthBase || Object.getPrototypeOf(C.prototype) !== LengthBase.prototype) return false;
      if (new LengthTwo().zero() !== 42 || !(new C() instanceof LengthBase) || C.staticZero() !== 7) return false;
      try { C.prototype.zero.call({}); return false; } catch(e) { if (!(e instanceof TypeError)) return false; }
      class Derived extends C {};
      return new Derived() instanceof C;
    })()"#;
    for realm in [&first, &second] {
        let s = realm.scope().unwrap();
        assert!(s.eval(check, None, 1).unwrap().to_bool());
    }
    {
        let s = first.scope().unwrap();
        s.eval(
            "Object.defineProperty(LengthZero.prototype.zero, 'length', {value:99}); delete LengthZero.staticZero",
            None,
            1,
        )
        .unwrap();
    }
    engine.collect_garbage_now();
    assert!(second.scope().unwrap().eval(check, None, 1).unwrap().to_bool());
    let third = engine.create_realm(Default::default()).unwrap();
    assert!(third.scope().unwrap().eval(check, None, 1).unwrap().to_bool());
}
