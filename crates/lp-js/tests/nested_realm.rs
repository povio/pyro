//! Child-frame creation must work while a parent native callback is on the stack.
use lp_js::{Engine, JsError, Persistent, Realm, RealmOptions};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn current_engine_realm_creation_is_synchronous_isolated_and_collectable() {
    let engine = Engine::new().unwrap();
    let parent = engine.create_realm(RealmOptions::default()).unwrap();
    let children = Rc::new(RefCell::new(Vec::<Realm>::new()));
    let keep = children.clone();
    let scope = parent.scope().unwrap();
    let make = scope.function("makeChild", move |s, _, _| {
        let child = Engine::create_realm_in_current_thread(RealmOptions {
            name: "child".into(),
            origin: Some("https://example.test".into()),
            ..Default::default()
        })?;
        let root = {
            let c = child.scope()?;
            c.eval("globalThis.answer = 42", None, 1)?;
            Persistent::new(&c, c.global().value())
        };
        keep.borrow_mut().push(child);
        Ok(root.get(s))
    });
    scope.global().set("makeChild", make.unwrap().value()).unwrap();
    assert!(scope
        .eval(
            "const child = makeChild(); child.answer === 42 && child.Array !== Array && typeof answer === 'undefined'",
            None,
            1
        )
        .unwrap()
        .to_bool());
    engine.collect_garbage_now();
    assert_eq!(scope.eval("child.answer", None, 1).unwrap().to_number().unwrap(), 42.0);
    let child = children.borrow_mut().pop().unwrap();
    let weak = child.downgrade();
    child.dispose();
    drop(child);
    assert!(weak.upgrade().is_none());
    assert_eq!(scope.eval("6 * 7", None, 1).unwrap().to_number().unwrap(), 42.0);
}

#[test]
fn current_engine_creation_rejects_absent_or_dropped_owner() {
    assert!(matches!(Engine::create_realm_in_current_thread(RealmOptions::default()), Err(JsError::ContextDestroyed)));
    let engine = Engine::new().unwrap();
    let parent = engine.create_realm(RealmOptions::default()).unwrap();
    drop(engine);
    assert!(matches!(Engine::create_realm_in_current_thread(RealmOptions::default()), Err(JsError::ContextDestroyed)));
    drop(parent);
}
