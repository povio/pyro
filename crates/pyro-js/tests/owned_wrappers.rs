//! Owned wrappers (`IdentityKey::OWNED`): objects JavaScriptCore tracks
//! without a weak handle or a finalizer (`webkit/patches/0029`). Their owner
//! decides how long they live; the collector lists the dead ones and
//! `drain_finalized` drops their payloads.

use pyro_js::*;
use std::cell::Cell;
use std::rc::Rc;

struct Data {
    id: u32,
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
    fn define(def: &mut ClassDef<Self>) {
        def.getter("id", |s, this| Ok(s.number(this.id as f64)));
        def.constructor(0, |s, new_target, _args| {
            s.wrap_new::<Thing>(Data { id: 7, drops: Rc::new(Cell::new(0)) }, new_target, Some(IdentityKey::OWNED))
        });
    }
}

fn setup() -> Option<(Engine, Realm)> {
    let e = Engine::new().expect("engine");
    e.register::<Thing>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    // The system framework cannot make them: their users fall back to the identity map.
    let owned = r.scope().unwrap().owned_wrappers();
    owned.then_some((e, r))
}

fn eval_str(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[inline(never)]
fn overwrite_dead_stack() -> u64 {
    let mut junk = [0u64; 16 * 1024];
    for (i, slot) in junk.iter_mut().enumerate() {
        *slot = std::hint::black_box(i as u64);
    }
    std::hint::black_box(junk.iter().sum())
}

fn collect(e: &Engine) {
    overwrite_dead_stack();
    for _ in 0..4 {
        e.collect_garbage_now();
        e.drain_finalized();
    }
}

/// `n` owned wrappers nothing holds, their references returned.
#[inline(never)]
fn make(r: &Realm, drops: &Rc<Cell<u32>>, n: u32, key: IdentityKey) -> Vec<WrapperRef> {
    let s = r.scope().unwrap();
    (0..n)
        .map(|id| {
            let o = s.wrap::<Thing>(Data { id, drops: drops.clone() }, Some(key)).unwrap();
            s.wrapper_ref(o).expect("an owned wrapper has a reference")
        })
        .collect()
}

/// `globalThis.held = ` the wrapper of `w`, in a frame of its own: none of
/// the caller's stack slots then keeps the object (the collector scans the
/// stack conservatively).
#[inline(never)]
fn hold(r: &Realm, w: WrapperRef) {
    let s = r.scope().unwrap();
    // SAFETY: the caller's payload is alive (nothing collected yet).
    let o = unsafe { s.wrapper(w) }.unwrap();
    s.global().set("held", o.value()).unwrap();
}

#[inline(never)]
fn alive(r: &Realm, refs: &[WrapperRef]) -> usize {
    let s = r.scope().unwrap();
    // SAFETY: the caller passes references whose payloads were not dropped.
    refs.iter().filter(|&&w| unsafe { s.wrapper(w) }.is_some()).count()
}

#[test]
fn an_unkept_wrapper_nothing_reaches_dies_and_its_payload_is_dropped_once() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let refs = make(&r, &drops, 300, IdentityKey::OWNED);
    assert_eq!(refs.len(), 300);
    assert_eq!(drops.get(), 0);
    collect(&e);
    assert_eq!(drops.get(), 300, "every payload dropped exactly once");
    assert_eq!(r.pending_finalizers(), 0);
    collect(&e);
    assert_eq!(drops.get(), 300);
}

#[test]
fn a_dead_wrapper_is_reported_before_its_payload_is_drained() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let refs = make(&r, &drops, 50, IdentityKey::OWNED);
    overwrite_dead_stack();
    for _ in 0..4 {
        e.collect_garbage_now();
    }
    // Collected, not drained: the payloads are intact, the objects gone.
    assert_eq!(drops.get(), 0);
    assert_eq!(r.pending_finalizers(), 50);
    assert_eq!(alive(&r, &refs), 0, "a dead wrapper is not handed out");
    assert_eq!(e.drain_finalized(), 50);
    assert_eq!(drops.get(), 50);
    assert_eq!(r.pending_finalizers(), 0);
}

#[test]
fn a_kept_wrapper_survives_with_its_properties_until_it_is_released() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let refs = make(&r, &drops, 100, IdentityKey::OWNED_KEPT);
    {
        let s = r.scope().unwrap();
        for (i, &w) in refs.iter().enumerate() {
            // SAFETY: the payloads are alive.
            let o = unsafe { s.wrapper(w) }.unwrap();
            assert!(unsafe { w.is_kept() });
            o.set("expando", s.number(i as f64)).unwrap();
            s.set_hidden(o, "hidden", s.string("h")).unwrap();
        }
    }
    collect(&e);
    assert_eq!(drops.get(), 0);
    {
        let s = r.scope().unwrap();
        for (i, &w) in refs.iter().enumerate() {
            // SAFETY: the payloads are alive.
            let o = unsafe { s.wrapper(w) }.expect("a kept wrapper survives collections");
            assert_eq!(o.get("expando").unwrap().to_number().unwrap(), i as f64, "same object, same expando");
            assert_eq!(o.get("id").unwrap().to_number().unwrap(), i as f64);
            assert_eq!(s.get_hidden(o, "hidden").unwrap().unwrap().to_string().unwrap(), "h");
            if i % 2 == 0 {
                unsafe { w.set_kept(false) };
            }
        }
    }
    collect(&e);
    assert_eq!(drops.get(), 50, "the released half died, hidden properties and all");
    let kept: Vec<WrapperRef> = refs.iter().copied().skip(1).step_by(2).collect();
    assert_eq!(alive(&r, &kept), 50);
}

#[test]
fn a_wrapper_script_holds_survives_unkept_and_is_the_same_object() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let refs = make(&r, &drops, 1, IdentityKey::OWNED);
    hold(&r, refs[0]);
    eval_str(&r, "held.mark = 'm'");
    collect(&e);
    assert_eq!(drops.get(), 0);
    #[inline(never)]
    fn same(r: &Realm, w: WrapperRef) {
        let s = r.scope().unwrap();
        // SAFETY: the payload is alive: script holds the object.
        let o = unsafe { s.wrapper(w) }.expect("reachable from script");
        assert!(o.value().strict_equals(s.global().get("held").unwrap()));
        assert_eq!(o.get("mark").unwrap().to_string().unwrap(), "m");
    }
    same(&r, refs[0]);
    eval_str(&r, "held = null");
    collect(&e);
    assert_eq!(drops.get(), 1);
}

#[test]
fn a_wrapper_kept_again_while_reachable_outlives_its_last_script_reference() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let refs = make(&r, &drops, 1, IdentityKey::OWNED);
    hold(&r, refs[0]);
    // Old by now: an eden collection would not look at it again.
    collect(&e);
    unsafe { refs[0].set_kept(true) };
    eval_str(&r, "held = null");
    collect(&e);
    assert_eq!(drops.get(), 0);
    assert_eq!(alive(&r, &refs), 1);
    unsafe { refs[0].set_kept(false) };
    collect(&e);
    assert_eq!(drops.get(), 1);
}

#[test]
fn young_wrappers_die_in_eden_collections_while_old_ones_stay() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    let old = make(&r, &drops, 64, IdentityKey::OWNED_KEPT);
    collect(&e);
    // Allocation pressure alone (eden collections) must end the young ones.
    for round in 0..40 {
        make(&r, &drops, 2000, IdentityKey::OWNED);
        eval_str(&r, "var junk = []; for (let i = 0; i < 20000; i++) junk.push({ i }); junk.length");
        e.drain_finalized();
        if drops.get() > 0 && round > 4 {
            break;
        }
    }
    assert!(drops.get() > 0, "no explicit full collection was needed");
    assert_eq!(alive(&r, &old), 64);
    collect(&e);
    assert_eq!(alive(&r, &old), 64);
}

#[test]
fn a_constructed_wrapper_with_another_prototype_is_tracked_too() {
    let Some((e, r)) = setup() else { return };
    assert_eq!(
        eval_str(
            &r,
            "class Sub extends Thing { extra() { return 'x' + this.id; } }; var t = new Sub(); t.own = 1; \
             [t instanceof Thing, t instanceof Sub, t.extra(), t.id, Object.getPrototypeOf(t) === Sub.prototype].join()"
        ),
        "true,true,x7,7,true"
    );
    collect(&e);
    assert_eq!(eval_str(&r, "[t.own, t.extra()].join()"), "1,x7");
    eval_str(&r, "t = null");
    collect(&e);
}

#[test]
fn disposing_a_realm_stops_keeping_its_wrappers_only() {
    let Some((e, r)) = setup() else { return };
    let other = e.create_realm(RealmOptions::default()).unwrap();
    let (drops, other_drops) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let refs = make(&r, &drops, 40, IdentityKey::OWNED_KEPT);
    let _gone = make(&other, &other_drops, 40, IdentityKey::OWNED_KEPT);
    collect(&e);
    assert_eq!((drops.get(), other_drops.get()), (0, 0));
    other.dispose();
    drop(other);
    collect(&e);
    assert_eq!(other_drops.get(), 40, "the disposed realm's wrappers died");
    assert_eq!(drops.get(), 0);
    assert_eq!(alive(&r, &refs), 40);
}

#[test]
fn dropping_the_engine_drops_live_and_dead_payloads_once() {
    let Some((e, r)) = setup() else { return };
    let drops = Rc::new(Cell::new(0));
    make(&r, &drops, 30, IdentityKey::OWNED_KEPT);
    make(&r, &drops, 30, IdentityKey::OWNED);
    {
        let s = r.scope().unwrap();
        let o = s.wrap::<Thing>(Data { id: 1, drops: drops.clone() }, Some(IdentityKey::OWNED_KEPT)).unwrap();
        s.set_hidden(o, "hidden", s.string("kept to the end")).unwrap();
    }
    overwrite_dead_stack();
    // Some are dead and listed, none drained.
    e.collect_garbage_now();
    drop(r);
    drop(e);
    assert_eq!(drops.get(), 61);
}
