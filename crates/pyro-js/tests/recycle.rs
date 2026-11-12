//! `Engine::recycle`: an engine goes to an unrelated owner only when it is
//! proven to hold nothing of the last one.
use pyro_js::*;

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

/// An engine and what its heap holds before anything used it; `None`
/// without the statistics SPI (the system framework), where a used engine
/// is never accepted.
fn engine() -> (Engine, Option<HeapCounts>) {
    let engine = Engine::new().unwrap();
    let clean = engine.heap_counts();
    (engine, clean)
}

/// Overwrites the part of the stack the calls before used: the collector
/// scans the stack conservatively, and a pointer a returned call left in a
/// slot nothing has written since keeps its object, and that object's
/// realm, alive. (Such a leftover makes `recycle` refuse, never accept.)
#[inline(never)]
fn clear_stack() {
    let zeroes = [0u8; 256 * 1024];
    std::hint::black_box(&zeroes);
}

#[inline(never)]
fn use_and_leave(engine: &Engine) {
    let realm = engine.create_realm(Default::default()).unwrap();
    eval(
        &realm,
        "globalThis.leaked = { big: Array.from({ length: 5000 }, (_, i) => ({ i })) };
         Object.prototype.leakedProto = 1;
         globalThis.pending = new Promise(() => {}).then(() => leaked);
         Promise.resolve().then(() => { globalThis.ran = 1 });
         function hot(n) { let s = 0; for (let i = 0; i < n; i++) s += i; return s }
         for (let i = 0; i < 200; i++) hot(1000);
         'done'",
    );
    realm.dispose();
}

#[test]
fn an_engine_nothing_used_is_accepted_without_a_baseline() {
    let (engine, _) = engine();
    assert!(engine.untouched());
    assert_eq!(engine.recycle(None), Ok(()));
    assert!(engine.untouched());
}

#[test]
fn a_used_engine_is_accepted_once_its_realms_are_gone_and_starts_over() {
    let (engine, Some(clean)) = engine() else { return };
    assert_eq!(clean.globals, 1, "the root context only: {clean:?}");
    for _ in 0..3 {
        use_and_leave(&engine);
        assert!(!engine.untouched());
        assert_eq!(engine.recycle(Some(clean)), Ok(()));
        assert!(engine.untouched());
        let after = engine.heap_counts().unwrap();
        assert_eq!(
            (after.globals, after.protected, after.protected_globals),
            (clean.globals, clean.protected, clean.protected_globals)
        );
        assert!(after.heap_size <= clean.heap_size + RECYCLE_HEAP_SLACK, "{after:?} vs {clean:?}");
    }
    // The next owner's first realm is a first realm.
    let realm = engine.create_realm(Default::default()).unwrap();
    assert_eq!(
        eval(&realm, "typeof leaked + ' ' + typeof ran + ' ' + ({}).leakedProto"),
        "undefined undefined undefined"
    );
    let first = std::thread::spawn(|| Engine::new().unwrap().create_realm(Default::default()).unwrap().id());
    assert_eq!(realm.id(), first.join().unwrap());
}

#[test]
fn a_used_engine_is_refused_without_a_baseline() {
    let (engine, _) = engine();
    use_and_leave(&engine);
    assert_eq!(engine.recycle(None), Err("no clean heap to compare with"));
}

#[test]
fn a_realm_still_alive_refuses_the_engine() {
    let (engine, Some(clean)) = engine() else { return };
    #[inline(never)]
    fn realm_in(engine: &Engine) -> Realm {
        let realm = engine.create_realm(Default::default()).unwrap();
        eval(&realm, "globalThis.leaked = 1");
        realm
    }
    let realm = realm_in(&engine);
    assert!(engine.recycle(Some(clean)).is_err(), "a live realm");
    realm.dispose();
    assert!(engine.recycle(Some(clean)).is_err(), "a disposed realm something still holds");
    drop(realm);
    clear_stack();
    assert_eq!(engine.recycle(Some(clean)), Ok(()));
}

#[test]
fn a_handle_that_outlives_its_realm_refuses_the_engine() {
    let (engine, Some(clean)) = engine() else { return };
    // Boxed: the handle's pointer to its object is then not on the stack.
    #[inline(never)]
    fn handle_in(engine: &Engine) -> Box<Persistent> {
        let realm = engine.create_realm(Default::default()).unwrap();
        let kept = {
            let scope = realm.scope().unwrap();
            let value = scope.eval("({ secret: 'of the first owner' })", None, 1).unwrap();
            Box::new(Persistent::new(&scope, value))
        };
        realm.dispose();
        kept
    }
    let kept = handle_in(&engine);
    assert!(engine.recycle(Some(clean)).is_err());
    drop(kept);
    clear_stack();
    assert_eq!(engine.recycle(Some(clean)), Ok(()));
}

#[test]
fn an_engine_a_script_was_terminated_in_is_refused_for_good() {
    let (engine, clean) = engine();
    let Ok(handle) = engine.terminate_handle() else { return };
    let realm = engine.create_realm(Default::default()).unwrap();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        handle.terminate();
    });
    {
        let scope = realm.scope().unwrap();
        assert!(scope.eval("for (;;) {}", None, 1).is_err());
        assert_eq!(scope.eval("1 + 1", None, 1).unwrap().to_number().unwrap(), 2.0);
    }
    stopper.join().unwrap();
    realm.dispose();
    drop(realm);
    assert_eq!(engine.recycle(clean), Err("a script was terminated"));
    assert_eq!(engine.recycle(clean), Err("a script was terminated"));
}

#[test]
fn a_termination_requested_while_nothing_ran_refuses_the_engine_too() {
    let (engine, clean) = engine();
    let Ok(handle) = engine.terminate_handle() else { return };
    handle.terminate();
    assert_eq!(engine.recycle(clean), Err("a script was terminated"));
}

#[test]
fn a_time_zone_override_goes_back_to_the_hosts() {
    let (engine, Some(clean)) = engine() else { return };
    let zone = |engine: &Engine| {
        let realm = engine.create_realm(Default::default()).unwrap();
        let zone = eval(&realm, "Intl.DateTimeFormat().resolvedOptions().timeZone");
        realm.dispose();
        zone
    };
    let host = zone(&engine);
    assert_eq!(engine.recycle(Some(clean)), Ok(()));
    let other = if host == "Asia/Tokyo" { "America/Lima" } else { "Asia/Tokyo" };
    if !engine.set_time_zone(Some(other)).unwrap_or(false) {
        return;
    }
    assert!(!engine.untouched());
    assert_eq!(zone(&engine), other);
    assert_eq!(engine.recycle(Some(clean)), Ok(()));
    assert_eq!(zone(&engine), host);
}
