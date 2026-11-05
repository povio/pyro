//! A class that defines an attribute twice (shared members first, then its
//! own, as lp-browser's Window does over lp-webapi's) gets the later one,
//! whether the attribute is a custom accessor or an ordinary one.

use lp_js::*;

struct Twice;
impl JsClass for Twice {
    const NAME: &'static str = "Twice";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("value", |s, _d| Ok(s.string("first")));
        def.accessor("both", |s, _d| Ok(s.string("first")), |_s, _d, _v| Ok(()));
        def.getter("value", |s, _d| Ok(s.string("second")));
        def.getter("both", |s, _d| Ok(s.string("second")));
    }
}

#[test]
fn the_later_definition_of_an_attribute_wins() {
    let e = Engine::new().expect("engine");
    e.register::<Twice>();
    let r = e.create_realm(RealmOptions::default()).unwrap();
    let s = r.scope().unwrap();
    let o = s.wrap::<Twice>((), None).unwrap();
    s.global().set("o", o.value()).unwrap();
    let got = s
        .eval(
            "const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(o), 'both');
             [o.value, o.both, typeof d.set].join()",
            None,
            1,
        )
        .unwrap()
        .to_string()
        .unwrap();
    assert_eq!(got, "second,second,undefined");
}
