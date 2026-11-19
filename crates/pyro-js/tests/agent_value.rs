//! `Scope::agent_value`: one value per engine, shared by its realms, that keeps no realm alive.
use pyro_js::{Engine, RealmOptions};

const MAP: &str = "new WeakMap()";

#[test]
fn agent_value_is_shared_by_the_engines_realms() {
    let engine = Engine::new().unwrap();
    let a = engine.create_realm(RealmOptions::default()).unwrap();
    let b = engine.create_realm(RealmOptions::default()).unwrap();
    let (sa, sb) = (a.scope().unwrap(), b.scope().unwrap());
    let ma = sa.agent_value("test.map", MAP).unwrap();
    let mb = sb.agent_value("test.map", MAP).unwrap();
    sa.global().set("m", ma).unwrap();
    sb.global().set("m", mb).unwrap();
    // A key of realm `a` stored through realm `b`'s view of the map is found from `a`.
    let key = sa.eval("globalThis.k = {}; k", None, 1).unwrap();
    sb.global().set("k", key).unwrap();
    sb.eval("WeakMap.prototype.set.call(m, k, 42)", None, 1).unwrap();
    assert_eq!(sa.eval("WeakMap.prototype.get.call(m, k)", None, 1).unwrap().to_number().unwrap(), 42.0);
    // Made in neither realm: its prototype is the root context's.
    assert!(sa.eval("Object.getPrototypeOf(m) !== WeakMap.prototype", None, 1).unwrap().to_bool());
    assert!(sb.eval("Object.getPrototypeOf(m) !== WeakMap.prototype", None, 1).unwrap().to_bool());
    // Another key, another value.
    let other = sa.agent_value("test.other", MAP).unwrap();
    sa.global().set("o", other).unwrap();
    assert!(sa.eval("m !== o", None, 1).unwrap().to_bool());
}

#[test]
fn agent_value_keeps_no_realm_alive() {
    let engine = Engine::new().unwrap();
    let a = engine.create_realm(RealmOptions::default()).unwrap();
    {
        let s = a.scope().unwrap();
        s.agent_value("test.map", MAP).unwrap();
    }
    let weak = a.downgrade();
    a.dispose();
    drop(a);
    assert!(weak.upgrade().is_none());
    let b = engine.create_realm(RealmOptions::default()).unwrap();
    let s = b.scope().unwrap();
    let m = s.agent_value("test.map", MAP).unwrap();
    s.global().set("m", m).unwrap();
    assert!(s.eval("m instanceof Object.getPrototypeOf(m).constructor", None, 1).unwrap().to_bool());
}

#[test]
fn agent_value_reports_a_throwing_source() {
    let engine = Engine::new().unwrap();
    let a = engine.create_realm(RealmOptions::default()).unwrap();
    let s = a.scope().unwrap();
    assert!(s.agent_value("test.throws", "throw new Error('no')").is_err());
}
