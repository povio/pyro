//! `Engine` teardown while the collector works. Its own test binary: JSC
//! reads `JSC_collectContinuously` once per process, and with it a
//! collection is under way (or ends) at nearly every point of the teardown,
//! which no other test wants.

use pyro_js::*;
use std::cell::Cell;
use std::rc::Rc;

struct Data {
    drops: Rc<Cell<u32>>,
}

impl Drop for Data {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

struct Thing;
impl JsClass for Thing {
    const NAME: &'static str = "Thing";
    type Data = Data;
    type Parent = NoParent;
    fn define(_def: &mut ClassDef<Self>) {}
}

/// Dropping the engine drops every owned wrapper's payload, whatever the
/// collector did meanwhile:
///
/// - a collection that ends between the last `drain_finalized` and the
///   teardown's take-all lists wrappers as dead (the realms are disposed,
///   nothing keeps them); the teardown takes that list too. Without that
///   their payloads leaked, and a node's payload holds its page's DOM.
/// - a collection still marking visits the live wrappers' hidden properties
///   on its own threads; the take-all frees them only once it has ended
///   (`webkit/patches/0029`, `PreventCollectionScope`).
#[test]
fn dropping_the_engine_while_the_collector_runs_drops_every_payload_once() {
    // Before the first VM exists: JSC reads its options then. One test in
    // this binary, so no other thread reads the environment meanwhile.
    std::env::set_var("JSC_collectContinuously", "1");
    let rounds: u32 = std::env::var("PYRO_TEST_TEARDOWN_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    for _ in 0..rounds {
        let e = Engine::new().expect("engine");
        e.register::<Thing>();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        // The system framework cannot make them.
        if !r.scope().unwrap().owned_wrappers() {
            return;
        }
        let drops = Rc::new(Cell::new(0));
        let n = 20_000;
        {
            let s = r.scope().unwrap();
            for _ in 0..n {
                let o = s.wrap::<Thing>(Data { drops: drops.clone() }, Some(IdentityKey::OWNED_KEPT)).unwrap();
                s.set_hidden(o, "hidden", s.string("h")).unwrap();
                s.set_hidden(o, "other", o.value()).unwrap();
            }
            // A large live heap and steady allocation: marking takes a while.
            s.eval(
                "var live = []; for (let i = 0; i < 300000; i++) live.push({ i, s: 'x' + i }); \
                 for (let k = 0; k < 6; k++) { let junk = []; for (let i = 0; i < 100000; i++) junk.push([i]); }",
                Some("test.js"),
                1,
            )
            .unwrap();
        }
        drop(r);
        drop(e);
        assert_eq!(drops.get(), n);
    }
}
