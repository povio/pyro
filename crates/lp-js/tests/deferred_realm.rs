//! Realms made when first entered (`Engine::defer_realm_in_current_thread`).
use lp_js::{Engine, JsError, RealmOptions};
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
