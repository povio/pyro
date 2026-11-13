//! Scheduling at the native return boundary, with GC-safe results and no recursive hooks.
use pyro_js::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

#[test]
fn hook_runs_after_body_borrows_before_later_promise_reactions() {
    let engine = Engine::new().unwrap();
    let realm = engine.create_realm(Default::default()).unwrap();
    let records = Rc::new(RefCell::new(Vec::new()));
    let hook_calls = Rc::new(Cell::new(0));
    let helper_calls = Rc::new(Cell::new(0));
    {
        let s = realm.scope().unwrap();
        let records = records.clone();
        let mutate = s
            .function("mutate", move |s, _, _| {
                records.borrow_mut().push(1);
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("mutate", mutate.value()).unwrap();
        let helper_calls = helper_calls.clone();
        let helper = s
            .function("helper", move |s, _, _| {
                helper_calls.set(helper_calls.get() + 1);
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("helper", helper.value()).unwrap();
        let outer = s
            .function("outer", |s, _, args| {
                args.get(0).as_object().unwrap().call(None, &[])?;
                Ok(s.undefined())
            })
            .unwrap();
        s.global().set("outer", outer.value()).unwrap();
    }
    let calls = hook_calls.clone();
    realm
        .set_native_callback_hook(Some(Rc::new(move |s| {
            calls.set(calls.get() + 1);
            let pending = !records.try_borrow_mut().expect("native body still borrowed records").is_empty();
            if pending {
                records.borrow_mut().clear();
                // This invokes another native callback, which must not recursively run the hook.
                s.eval("helper(); Promise.resolve().then(() => order.push('observer'))", None, 1).unwrap();
            }
        })))
        .unwrap();
    eval(&realm, "globalThis.order=[]; Promise.resolve().then(()=>order.push('before')); mutate(); Promise.resolve().then(()=>order.push('after')); order.push('sync')");
    assert_eq!(eval(&realm, "order.join(',')"), "sync,before,observer,after");
    assert_eq!(hook_calls.get(), 1);
    assert_eq!(helper_calls.get(), 1);
    // Nested ordinary native callbacks must still schedule before JS continues inside outer().
    eval(
        &realm,
        "order=[]; outer(()=>{mutate(); Promise.resolve().then(()=>order.push('after-inner'))}); order.push('sync')",
    );
    assert_eq!(eval(&realm, "order.join(',')"), "sync,observer,after-inner");
    assert_eq!(hook_calls.get(), 3);
    assert_eq!(helper_calls.get(), 2);
    realm.set_native_callback_hook(None).unwrap();
    eval(&realm, "mutate()");
    assert_eq!(hook_calls.get(), 3);
}

struct Payload(Rc<Cell<usize>>);
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct Token;
impl JsClass for Token {
    const NAME: &'static str = "Token";
    type Data = Payload;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("answer", |s, _| Ok(s.number(42.0)));
    }
}

#[test]
fn return_and_thrown_values_survive_hook_gc_and_payloads_are_released() {
    let engine = Rc::new(Engine::new().unwrap());
    engine.register::<Token>();
    let realm = engine.create_realm(Default::default()).unwrap();
    let drops = Rc::new(Cell::new(0));
    {
        let scope = realm.scope().unwrap();
        for throwing in [false, true] {
            let drops = drops.clone();
            let f = scope
                .function("make", move |s, _, _| {
                    let v = s.wrap::<Token>(Payload(drops.clone()), None)?.value();
                    if throwing {
                        Err(JsError::thrown(Persistent::new(s, v)))
                    } else {
                        Ok(v)
                    }
                })
                .unwrap();
            scope.global().set(if throwing { "fail" } else { "make" }, f.value()).unwrap();
        }
    }
    let weak = Rc::downgrade(&engine);
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    realm
        .set_native_callback_hook(Some(Rc::new(move |_| {
            count.set(count.get() + 1);
            weak.upgrade().unwrap().collect_garbage_now();
        })))
        .unwrap();
    assert_eq!(
        eval(&realm, "let sum=0; for(let i=0;i<32;i++){ sum+=make().answer;try{fail()}catch(e){sum+=e.answer}} sum"),
        "2688"
    );
    assert_eq!(calls.get(), 128, "hook omitted a return, throw or getter");
    realm.dispose();
    drop(realm);
    drop(engine);
    assert_eq!(drops.get(), 64, "native return/exception roots leaked payloads");
}

#[test]
fn panicking_hook_resets_reentrancy_and_disposal_releases_captures() {
    let engine = Engine::new().unwrap();
    let realm = engine.create_realm(Default::default()).unwrap();
    {
        let s = realm.scope().unwrap();
        let f = s.function("native", |s, _, _| Ok(s.number(7.0))).unwrap();
        s.global().set("native", f.value()).unwrap();
    }
    let calls = Rc::new(Cell::new(0));
    let weak = Rc::downgrade(&calls);
    realm
        .set_native_callback_hook(Some(Rc::new(move |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                panic!("hook test panic");
            }
        })))
        .unwrap();
    assert_eq!(eval(&realm, "try { native() } catch(e) { e.message }"), "Internal error: hook test panic");
    assert_eq!(eval(&realm, "native()"), "7");
    assert_eq!(weak.upgrade().unwrap().get(), 2);
    realm.dispose();
    assert!(weak.upgrade().is_none());
    assert!(realm.set_native_callback_hook(None).is_err());
}

#[test]
fn termination_skips_the_scheduling_hook() {
    let engine = Engine::new().unwrap();
    let realm = engine.create_realm(Default::default()).unwrap();
    let calls = Rc::new(Cell::new(0));
    {
        let s = realm.scope().unwrap();
        let f = s.function("stop", |_, _, _| Err(JsError::Terminated)).unwrap();
        s.global().set("stop", f.value()).unwrap();
    }
    let count = calls.clone();
    realm.set_native_callback_hook(Some(Rc::new(move |_| count.set(count.get() + 1)))).unwrap();
    assert!(realm.scope().unwrap().eval("stop()", None, 1).is_err());
    assert_eq!(calls.get(), 0);
}
