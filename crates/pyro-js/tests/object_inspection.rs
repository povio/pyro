//! Native inspection must not invoke page getters/traps or depend on prototypes.
use pyro_js::*;

struct Host;
impl JsClass for Host {
    const NAME: &'static str = "InspectionHost";
    type Data = ();
    type Parent = NoParent;
    fn define(_: &mut ClassDef<Self>) {}
}

#[test]
fn live_proxy_inspection_does_not_invoke_traps_and_documents_revoked_boundary() {
    let engine = Engine::new().unwrap();
    let realm = engine.create_realm(Default::default()).unwrap();
    let s = realm.scope().unwrap();
    s.eval(
        r#"globalThis.traps = 0;
        const hostile = new Proxy({}, {get() { traps++; throw new Error('trap'); }});
        globalThis.p = new Proxy({}, hostile);
        globalThis.nested = new Proxy(p, hostile);
        globalThis.revoked = Proxy.revocable([], hostile);
        revoked.revoke();
        globalThis.tagged = {get [Symbol.toStringTag]() { traps++; throw new Error('tag'); }};
        "#,
        None,
        1,
    )
    .unwrap();
    for source in ["p", "nested", "new Proxy(function(){}, {})"] {
        let value = s.eval(source, None, 1).unwrap();
        if engine.capabilities().proxy_targets {
            assert!(value.has_proxy_target().unwrap(), "{source}");
        } else {
            assert!(matches!(value.has_proxy_target(), Err(JsError::Unsupported(_))));
        }
    }
    if engine.capabilities().proxy_targets {
        for source in ["tagged", "[]", "({})", "function f(){};f"] {
            assert!(!s.eval(source, None, 1).unwrap().has_proxy_target().unwrap(), "{source}");
        }
    }
    for source in ["undefined", "null", "1", "true", "'x'", "1n", "Symbol()"] {
        assert!(!s.eval(source, None, 1).unwrap().has_proxy_target().unwrap());
    }
    assert_eq!(s.eval("traps", None, 1).unwrap().to_number().unwrap(), 0.0);
    // The installed JSC may retain or clear a revoked proxy target. Inspecting
    // it must not execute traps; callers also need the captured Array.isArray
    // rejection path rather than interpreting false as cloneability.
    if engine.capabilities().proxy_targets {
        let _ = s.eval("revoked.proxy", None, 1).unwrap().has_proxy_target().unwrap();
        assert_eq!(s.eval("traps", None, 1).unwrap().to_number().unwrap(), 0.0);
    }
    assert_eq!(
        s.eval("try { Array.isArray(revoked.proxy) } catch(e) { e.name }", None, 1).unwrap().to_string().unwrap(),
        "TypeError"
    );
}

#[test]
fn host_inspection_survives_prototype_changes_and_realm_disposal_without_js() {
    let engine = Engine::new().unwrap();
    engine.register::<Host>();
    let first = engine.create_realm(Default::default()).unwrap();
    let second = engine.create_realm(Default::default()).unwrap();
    let (foreign_global, wrapper, closure) = {
        let s = first.scope().unwrap();
        let wrapper = s.wrap::<Host>((), None).unwrap();
        wrapper.set_prototype(s.null());
        let closure = s.function("native", |s, _, _| Ok(s.undefined())).unwrap();
        (
            Persistent::new(&s, s.global().value()),
            Persistent::new(&s, wrapper.value()),
            Persistent::new(&s, closure.value()),
        )
    };
    let s = second.scope().unwrap();
    for value in [wrapper.get(&s), closure.get(&s)] {
        assert!(s.is_host_object(value).unwrap());
    }
    if engine.capabilities().object_global_context {
        assert!(s.is_host_object(s.global().value()).unwrap());
        assert!(s.is_host_object(foreign_global.get(&s)).unwrap());
    } else {
        assert!(matches!(s.is_host_object(s.global().value()), Err(JsError::Unsupported(_))));
    }
    first.dispose();
    drop(first);
    engine.collect_garbage_now();
    assert!(s.is_host_object(wrapper.get(&s)).unwrap());
    if engine.capabilities().object_global_context {
        assert!(s.is_host_object(foreign_global.get(&s)).unwrap());
        s.eval("globalThis.traps = 0", None, 1).unwrap();
        for source in [
            "({get [Symbol.toStringTag]() { traps++; throw new Error('tag'); }})",
            "new Proxy({}, {getPrototypeOf(){traps++;throw new Error('trap')}})",
            "[]",
            "new Date()",
            "Promise.resolve()",
            "function f(){};f",
        ] {
            assert!(!s.is_host_object(s.eval(source, None, 1).unwrap()).unwrap(), "{source}");
        }
        assert_eq!(s.eval("traps", None, 1).unwrap().to_number().unwrap(), 0.0);
    }
    assert!(!s.is_host_object(s.null()).unwrap());
    assert!(!s.is_host_object(s.number(3.0)).unwrap());
}

#[test]
fn host_inspection_rejects_values_from_another_engine() {
    let first = Engine::new().unwrap();
    let first_realm = first.create_realm(Default::default()).unwrap();
    drop(first);
    let second = Engine::new().unwrap();
    let second_realm = second.create_realm(Default::default()).unwrap();
    let a = first_realm.scope().unwrap();
    let b = second_realm.scope().unwrap();
    assert!(matches!(b.is_host_object(a.global().value()), Err(JsError::Type(_))));
}
