//! Realms made when first entered (`Engine::defer_realm_in_current_thread`).
use pyro_js::{Engine, JsError, RealmOptions};
use std::cell::Cell;
use std::rc::Rc;

fn options() -> RealmOptions {
    RealmOptions { name: "utility".into(), origin: Some("https://site.test".into()), ..Default::default() }
}

#[test]
fn a_deferred_realm_is_made_by_its_first_scope() {
    let engine = Engine::new().unwrap();
    let _root = engine.create_realm(RealmOptions::default()).unwrap();
    let ran = Rc::new(Cell::new(0));
    let setup_ran = ran.clone();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(move |realm| {
            setup_ran.set(setup_ran.get() + 1);
            realm.scope()?.eval("globalThis.setUp = 7", None, 1)?;
            Ok(())
        }),
    )
    .unwrap();
    // Everything but the context exists at once.
    assert!(realm.is_deferred());
    assert!(!realm.is_disposed());
    assert_eq!(realm.name(), "utility");
    assert_eq!(realm.origin().as_deref(), Some("https://site.test"));
    assert_eq!(ran.get(), 0, "nothing ran");
    let id = realm.id();
    let later = engine.create_realm(RealmOptions::default()).unwrap();
    assert!(later.id() > id, "the id is taken when the realm is deferred");
    {
        let s = realm.scope().unwrap();
        assert_eq!(s.eval("setUp + Array.length", None, 1).unwrap().to_number().unwrap(), 8.0, "set up, interfaces made");
    }
    assert!(!realm.is_deferred());
    assert!(!realm.is_disposed());
    assert_eq!(ran.get(), 1);
    assert_eq!(realm.id(), id);
    realm.scope().unwrap();
    assert_eq!(ran.get(), 1, "set up once");
    realm.dispose();
    assert!(realm.is_disposed());
    assert!(matches!(realm.scope(), Err(JsError::ContextDestroyed)));
}

#[test]
fn disposing_a_deferred_realm_never_makes_it() {
    let engine = Engine::new().unwrap();
    let _root = engine.create_realm(RealmOptions::default()).unwrap();
    let ran = Rc::new(Cell::new(false));
    let setup_ran = ran.clone();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(move |_| {
            setup_ran.set(true);
            Ok(())
        }),
    )
    .unwrap();
    let handle = realm.clone();
    realm.dispose();
    assert!(realm.is_disposed() && handle.is_disposed());
    assert!(!realm.is_deferred());
    assert!(matches!(handle.scope(), Err(JsError::ContextDestroyed)));
    assert!(!ran.get(), "a disposed deferred realm is never set up");
    // Dropping the last handle of one never entered costs nothing either.
    let dropped = Engine::defer_realm_in_current_thread(options(), Box::new(|_| Ok(()))).unwrap();
    let weak = dropped.downgrade();
    drop(dropped);
    assert!(weak.upgrade().is_none());
}

#[test]
fn a_failing_setup_leaves_the_realm_disposed() {
    let engine = Engine::new().unwrap();
    let _root = engine.create_realm(RealmOptions::default()).unwrap();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(|realm| {
            realm.scope()?.eval("globalThis.partial = 1", None, 1)?;
            Err(JsError::Type("setup failed".into()))
        }),
    )
    .unwrap();
    assert!(matches!(realm.scope(), Err(JsError::Type(message)) if message == "setup failed"));
    assert!(realm.is_disposed(), "a realm whose setup failed is disposed, not half made");
    assert!(matches!(realm.scope(), Err(JsError::ContextDestroyed)));
}

// ---- Review (perf/realms) ----

#[test]
fn review_a_deferred_realm_made_from_inside_another_realms_callback() {
    // The first entry can happen with page script of another realm on the
    // stack (a native callback that reaches into the world).
    let engine = Engine::new().unwrap();
    let root = engine.create_realm(RealmOptions::default()).unwrap();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(|realm| {
            realm.scope()?.eval("globalThis.ready = 'set up'", None, 1)?;
            Ok(())
        }),
    )
    .unwrap();
    let world = realm.clone();
    let scope = root.scope().unwrap();
    let enter = scope
        .function("enter", move |s, _, _| {
            let made = {
                let w = world.scope()?;
                w.eval("ready + ':' + typeof Array + ':' + (globalThis.count = (globalThis.count || 0) + 1)", None, 1)?
                    .to_string()?
            };
            Ok(s.string(&made))
        })
        .unwrap();
    scope.global().set("enter", enter.value()).unwrap();
    let r = scope.eval("[enter(), enter(), typeof ready].join()", None, 1).unwrap().to_string().unwrap();
    assert_eq!(r, "set up:function:1,set up:function:2,undefined");
    assert!(!realm.is_deferred());
    // Found by id once made (and not before, see below).
    assert!(pyro_js::Realm::by_id(realm.id()).is_some());
}

#[test]
fn review_a_deferred_realm_is_not_found_by_id_until_made() {
    let engine = Engine::new().unwrap();
    let _root = engine.create_realm(RealmOptions::default()).unwrap();
    let realm = Engine::defer_realm_in_current_thread(options(), Box::new(|_| Ok(()))).unwrap();
    // Documents the lookup's behaviour for a realm that exists but has no context yet.
    assert!(pyro_js::Realm::by_id(realm.id()).is_none());
    realm.scope().unwrap();
    assert!(pyro_js::Realm::by_id(realm.id()).is_some_and(|r| r.ptr_eq(&realm)));
}

#[test]
fn review_a_setup_that_disposes_its_realm_fails_the_entry() {
    let engine = Engine::new().unwrap();
    let _root = engine.create_realm(RealmOptions::default()).unwrap();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(|realm| {
            realm.dispose();
            Ok(())
        }),
    )
    .unwrap();
    assert!(matches!(realm.scope(), Err(JsError::ContextDestroyed)));
    assert!(realm.is_disposed() && !realm.is_deferred());
}

#[test]
fn review_a_deferred_realm_after_the_engine_shut_down() {
    let engine = Engine::new().unwrap();
    let root = engine.create_realm(RealmOptions::default()).unwrap();
    let ran = Rc::new(Cell::new(false));
    let flag = ran.clone();
    let realm = Engine::defer_realm_in_current_thread(
        options(),
        Box::new(move |_| {
            flag.set(true);
            Ok(())
        }),
    )
    .unwrap();
    // The engine shuts down once its owner and every made realm are gone: a
    // deferred realm does not hold it up (it is not in the engine's map).
    drop(engine);
    root.dispose();
    drop(root);
    assert!(realm.scope().is_err());
    assert!(!ran.get(), "never set up once the engine is gone");
    assert!(realm.is_disposed());
}
