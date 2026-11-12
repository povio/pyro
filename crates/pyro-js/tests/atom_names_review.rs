//! Review tests for the thread's static name strings (`pyro_js::string::atom_raw`)
//! across engines, written for patch 0025's remembered atoms (since dropped)
//! and kept for its lock skip: a VM's atom table dies with it; the next
//! engine on the thread must still find its properties by those names, and
//! two live engines on one thread must each see their own.

use pyro_js::*;

struct Probe;
impl JsClass for Probe {
    const NAME: &'static str = "AtomReviewProbe";
    type Data = u32;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method_this("remember", 1, |s, _this, me, args| {
            s.set_hidden(me, HIDDEN, args.get(0))?;
            Ok(me.value())
        });
        def.getter_this("recall", |s, _this, me| Ok(s.get_hidden(me, HIDDEN)?.unwrap_or_else(|| s.null())));
        def.getter("lpReviewAccessorName", |s, this| Ok(s.number(f64::from(*this))));
    }
}

fn engine() -> Engine {
    let e = Engine::new().expect("engine");
    e.register::<Probe>();
    e
}

const NAME: &str = "lpReviewAtomName";
const HIDDEN: &str = "lpReviewHiddenName";

/// Writes and reads by the static names in `r`, and checks JavaScript sees
/// the same property (same atom) both ways.
fn exercise_plain(r: &Realm, tag: f64) {
    let s = r.scope().unwrap();
    let g = s.global();
    let v = s.eval(&format!("{tag}"), None, 1).unwrap();
    g.set_atom(NAME, v).unwrap();
    assert_eq!(s.eval(&format!("globalThis.{NAME}"), None, 1).unwrap().to_number().unwrap(), tag, "JS sees the API's write");
    s.eval(&format!("globalThis.{NAME} = {tag} + 1"), None, 1).unwrap();
    assert_eq!(g.get_atom(NAME).unwrap().to_number().unwrap(), tag + 1.0, "the API sees JS's write");
    assert!(g.has_atom(NAME));
}

/// [`exercise_plain`], plus hidden values and an accessor of a host class.
fn exercise(r: &Realm, tag: f64) {
    exercise_plain(r, tag);
    let s = r.scope().unwrap();
    let g = s.global();
    // Hidden (private) properties by name, from Rust and from callbacks,
    // and an accessor installed by its static name.
    let w = s.wrap::<Probe>(tag as u32, None).unwrap();
    let h = s.eval(&format!("({{ t: {tag} }})"), None, 1).unwrap();
    s.set_hidden(w, HIDDEN, h).unwrap();
    assert_eq!(s.get_hidden(w, HIDDEN).unwrap().expect("hidden value").as_raw(), h.as_raw());
    g.set("w", w.value()).unwrap();
    let r = s
        .eval("var o = { k: 1 }; w.remember(o); [w.recall === o, w.lpReviewAccessorName, Object.getOwnPropertyNames(Object.getPrototypeOf(w)).includes('lpReviewAccessorName')].join()", None, 1)
        .unwrap()
        .to_string()
        .unwrap();
    assert_eq!(r, format!("true,{},true", tag as u32));
}

#[test]
fn static_names_work_across_engines_made_one_after_another() {
    for i in 0..6 {
        let e = engine();
        let r = e.create_realm(RealmOptions::default()).unwrap();
        exercise(&r, i as f64);
        // A second realm of the same engine shares its VM and table.
        let r2 = e.create_realm(RealmOptions::default()).unwrap();
        exercise(&r2, 100.0 + i as f64);
        drop(r2);
        drop(r);
        drop(e);
    }
}

#[test]
fn static_names_work_with_two_live_engines_on_one_thread() {
    // Engine A's owner goes, but a realm keeps its VM (and atom table) alive
    // while engine B is made on the same thread.
    let a = engine();
    let ra = a.create_realm(RealmOptions::default()).unwrap();
    exercise(&ra, 1.0);
    drop(a);
    let b = engine();
    let rb = b.create_realm(RealmOptions::default()).unwrap();
    for i in 0..4 {
        exercise(&rb, 10.0 + i as f64);
        // A is no longer the thread's engine: its callbacks refuse to run,
        // but plain property calls by static name still reach its VM.
        exercise_plain(&ra, 20.0 + i as f64);
    }
    // A's VM and table go now; B must keep finding its properties, and a
    // third engine after B as well.
    drop(ra);
    for i in 0..3 {
        exercise(&rb, 30.0 + i as f64);
    }
    drop(rb);
    drop(b);
    let c = engine();
    let rc = c.create_realm(RealmOptions::default()).unwrap();
    exercise(&rc, 40.0);
}

#[test]
fn static_names_work_on_many_threads() {
    let threads: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                for i in 0..3 {
                    let e = engine();
                    let r = e.create_realm(RealmOptions::default()).unwrap();
                    exercise(&r, (t * 10 + i) as f64);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().expect("thread");
    }
}
