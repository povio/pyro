//! JS to Rust to JS re-entry while the JS lock stays held across native callbacks.
use lp_js::*;

fn realm_with_native(engine: &Engine) -> Realm {
    let realm = engine.create_realm(Default::default()).unwrap();
    {
        let s = realm.scope().unwrap();
        let native = s
            .function("native", |s, _, args| {
                args.get(0).as_object().unwrap().call(None, &[])?;
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("native", native.value()).unwrap();
    }
    realm
}

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

#[test]
fn runaway_recursion_through_native_ends_in_range_error() {
    let engine = Engine::new().unwrap();
    let realm = realm_with_native(&engine);
    let result = eval(
        &realm,
        "let depth = 0;
         function r() { depth++; native(r); }
         let kind = 'none';
         try { r(); } catch (e) { kind = e.constructor.name; }
         kind + ':' + (depth > 100)",
    );
    assert_eq!(result, "RangeError:true");
    // The VM still works after unwinding.
    assert_eq!(eval(&realm, "native(() => { globalThis.ok = 1 + 1; }); ok"), "2");
}

#[test]
fn microtasks_queued_inside_a_callback_wait_for_the_outer_script() {
    let engine = Engine::new().unwrap();
    let realm = realm_with_native(&engine);
    assert_eq!(
        eval(
            &realm,
            "globalThis.log = [];
             native(() => {
               Promise.resolve().then(() => log.push('micro'));
               native(() => log.push('nested'));
               log.push('inner');
             });
             log.push('after');
             log.join()"
        ),
        "nested,inner,after"
    );
    assert_eq!(eval(&realm, "log.join()"), "nested,inner,after,micro");
}
