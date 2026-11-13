//! Measures realm creation cost with a WebIDL-sized class registry.
//! Run: cargo run --release -p pyro-js --features bench --example realm_bench
use pyro_js::*;
use std::time::Instant;

struct D;
struct C0;
impl JsClass for C0 {
    const NAME: &'static str = "C0";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m0_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m0_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a0_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a0_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a0_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a0_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a0_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a0_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a0_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a0_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a0_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a0_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a0_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a0_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a0_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a0_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a0_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C1;
impl JsClass for C1 {
    const NAME: &'static str = "C1";
    type Data = ();
    type Parent = C0;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m1_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m1_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a1_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a1_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a1_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a1_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a1_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a1_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a1_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a1_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a1_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a1_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a1_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a1_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a1_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a1_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a1_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C2;
impl JsClass for C2 {
    const NAME: &'static str = "C2";
    type Data = ();
    type Parent = C1;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m2_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m2_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a2_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a2_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a2_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a2_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a2_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a2_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a2_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a2_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a2_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a2_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a2_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a2_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a2_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a2_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a2_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C3;
impl JsClass for C3 {
    const NAME: &'static str = "C3";
    type Data = ();
    type Parent = C2;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m3_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m3_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a3_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a3_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a3_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a3_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a3_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a3_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a3_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a3_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a3_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a3_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a3_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a3_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a3_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a3_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a3_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C4;
impl JsClass for C4 {
    const NAME: &'static str = "C4";
    type Data = ();
    type Parent = C3;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m4_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m4_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a4_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a4_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a4_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a4_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a4_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a4_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a4_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a4_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a4_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a4_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a4_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a4_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a4_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a4_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a4_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C5;
impl JsClass for C5 {
    const NAME: &'static str = "C5";
    type Data = ();
    type Parent = C4;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m5_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m5_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a5_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a5_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a5_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a5_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a5_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a5_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a5_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a5_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a5_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a5_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a5_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a5_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a5_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a5_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a5_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C6;
impl JsClass for C6 {
    const NAME: &'static str = "C6";
    type Data = ();
    type Parent = C5;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m6_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m6_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a6_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a6_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a6_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a6_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a6_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a6_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a6_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a6_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a6_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a6_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a6_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a6_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a6_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a6_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a6_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C7;
impl JsClass for C7 {
    const NAME: &'static str = "C7";
    type Data = ();
    type Parent = C6;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m7_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m7_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a7_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a7_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a7_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a7_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a7_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a7_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a7_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a7_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a7_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a7_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a7_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a7_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a7_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a7_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a7_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C8;
impl JsClass for C8 {
    const NAME: &'static str = "C8";
    type Data = ();
    type Parent = C7;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m8_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m8_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a8_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a8_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a8_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a8_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a8_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a8_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a8_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a8_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a8_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a8_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a8_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a8_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a8_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a8_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a8_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C9;
impl JsClass for C9 {
    const NAME: &'static str = "C9";
    type Data = ();
    type Parent = C8;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m9_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m9_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a9_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a9_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a9_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a9_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a9_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a9_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a9_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a9_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a9_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a9_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a9_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a9_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a9_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a9_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a9_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C10;
impl JsClass for C10 {
    const NAME: &'static str = "C10";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m10_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m10_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a10_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a10_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a10_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a10_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a10_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a10_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a10_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a10_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a10_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a10_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a10_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a10_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a10_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a10_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a10_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C11;
impl JsClass for C11 {
    const NAME: &'static str = "C11";
    type Data = ();
    type Parent = C10;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m11_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m11_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a11_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a11_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a11_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a11_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a11_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a11_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a11_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a11_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a11_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a11_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a11_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a11_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a11_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a11_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a11_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C12;
impl JsClass for C12 {
    const NAME: &'static str = "C12";
    type Data = ();
    type Parent = C11;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m12_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m12_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a12_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a12_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a12_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a12_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a12_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a12_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a12_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a12_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a12_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a12_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a12_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a12_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a12_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a12_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a12_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C13;
impl JsClass for C13 {
    const NAME: &'static str = "C13";
    type Data = ();
    type Parent = C12;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m13_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m13_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a13_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a13_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a13_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a13_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a13_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a13_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a13_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a13_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a13_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a13_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a13_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a13_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a13_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a13_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a13_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C14;
impl JsClass for C14 {
    const NAME: &'static str = "C14";
    type Data = ();
    type Parent = C13;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m14_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m14_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a14_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a14_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a14_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a14_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a14_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a14_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a14_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a14_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a14_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a14_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a14_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a14_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a14_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a14_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a14_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C15;
impl JsClass for C15 {
    const NAME: &'static str = "C15";
    type Data = ();
    type Parent = C14;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m15_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m15_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a15_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a15_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a15_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a15_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a15_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a15_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a15_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a15_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a15_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a15_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a15_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a15_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a15_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a15_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a15_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C16;
impl JsClass for C16 {
    const NAME: &'static str = "C16";
    type Data = ();
    type Parent = C15;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m16_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m16_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a16_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a16_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a16_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a16_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a16_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a16_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a16_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a16_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a16_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a16_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a16_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a16_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a16_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a16_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a16_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C17;
impl JsClass for C17 {
    const NAME: &'static str = "C17";
    type Data = ();
    type Parent = C16;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m17_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m17_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a17_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a17_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a17_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a17_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a17_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a17_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a17_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a17_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a17_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a17_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a17_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a17_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a17_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a17_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a17_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C18;
impl JsClass for C18 {
    const NAME: &'static str = "C18";
    type Data = ();
    type Parent = C17;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m18_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m18_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a18_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a18_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a18_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a18_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a18_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a18_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a18_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a18_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a18_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a18_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a18_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a18_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a18_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a18_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a18_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C19;
impl JsClass for C19 {
    const NAME: &'static str = "C19";
    type Data = ();
    type Parent = C18;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m19_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m19_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a19_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a19_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a19_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a19_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a19_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a19_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a19_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a19_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a19_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a19_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a19_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a19_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a19_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a19_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a19_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C20;
impl JsClass for C20 {
    const NAME: &'static str = "C20";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m20_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m20_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a20_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a20_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a20_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a20_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a20_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a20_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a20_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a20_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a20_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a20_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a20_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a20_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a20_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a20_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a20_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C21;
impl JsClass for C21 {
    const NAME: &'static str = "C21";
    type Data = ();
    type Parent = C20;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m21_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m21_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a21_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a21_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a21_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a21_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a21_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a21_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a21_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a21_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a21_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a21_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a21_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a21_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a21_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a21_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a21_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C22;
impl JsClass for C22 {
    const NAME: &'static str = "C22";
    type Data = ();
    type Parent = C21;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m22_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m22_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a22_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a22_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a22_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a22_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a22_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a22_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a22_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a22_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a22_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a22_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a22_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a22_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a22_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a22_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a22_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C23;
impl JsClass for C23 {
    const NAME: &'static str = "C23";
    type Data = ();
    type Parent = C22;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m23_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m23_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a23_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a23_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a23_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a23_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a23_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a23_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a23_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a23_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a23_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a23_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a23_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a23_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a23_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a23_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a23_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C24;
impl JsClass for C24 {
    const NAME: &'static str = "C24";
    type Data = ();
    type Parent = C23;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m24_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m24_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a24_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a24_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a24_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a24_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a24_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a24_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a24_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a24_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a24_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a24_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a24_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a24_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a24_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a24_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a24_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C25;
impl JsClass for C25 {
    const NAME: &'static str = "C25";
    type Data = ();
    type Parent = C24;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m25_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m25_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a25_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a25_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a25_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a25_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a25_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a25_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a25_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a25_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a25_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a25_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a25_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a25_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a25_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a25_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a25_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C26;
impl JsClass for C26 {
    const NAME: &'static str = "C26";
    type Data = ();
    type Parent = C25;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m26_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m26_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a26_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a26_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a26_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a26_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a26_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a26_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a26_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a26_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a26_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a26_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a26_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a26_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a26_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a26_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a26_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C27;
impl JsClass for C27 {
    const NAME: &'static str = "C27";
    type Data = ();
    type Parent = C26;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m27_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m27_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a27_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a27_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a27_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a27_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a27_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a27_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a27_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a27_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a27_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a27_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a27_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a27_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a27_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a27_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a27_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C28;
impl JsClass for C28 {
    const NAME: &'static str = "C28";
    type Data = ();
    type Parent = C27;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m28_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m28_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a28_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a28_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a28_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a28_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a28_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a28_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a28_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a28_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a28_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a28_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a28_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a28_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a28_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a28_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a28_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C29;
impl JsClass for C29 {
    const NAME: &'static str = "C29";
    type Data = ();
    type Parent = C28;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m29_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m29_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a29_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a29_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a29_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a29_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a29_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a29_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a29_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a29_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a29_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a29_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a29_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a29_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a29_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a29_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a29_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C30;
impl JsClass for C30 {
    const NAME: &'static str = "C30";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m30_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m30_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a30_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a30_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a30_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a30_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a30_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a30_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a30_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a30_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a30_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a30_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a30_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a30_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a30_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a30_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a30_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C31;
impl JsClass for C31 {
    const NAME: &'static str = "C31";
    type Data = ();
    type Parent = C30;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m31_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m31_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a31_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a31_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a31_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a31_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a31_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a31_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a31_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a31_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a31_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a31_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a31_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a31_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a31_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a31_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a31_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C32;
impl JsClass for C32 {
    const NAME: &'static str = "C32";
    type Data = ();
    type Parent = C31;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m32_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m32_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a32_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a32_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a32_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a32_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a32_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a32_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a32_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a32_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a32_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a32_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a32_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a32_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a32_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a32_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a32_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C33;
impl JsClass for C33 {
    const NAME: &'static str = "C33";
    type Data = ();
    type Parent = C32;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m33_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m33_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a33_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a33_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a33_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a33_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a33_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a33_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a33_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a33_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a33_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a33_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a33_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a33_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a33_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a33_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a33_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C34;
impl JsClass for C34 {
    const NAME: &'static str = "C34";
    type Data = ();
    type Parent = C33;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m34_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m34_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a34_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a34_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a34_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a34_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a34_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a34_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a34_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a34_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a34_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a34_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a34_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a34_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a34_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a34_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a34_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C35;
impl JsClass for C35 {
    const NAME: &'static str = "C35";
    type Data = ();
    type Parent = C34;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m35_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m35_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a35_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a35_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a35_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a35_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a35_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a35_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a35_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a35_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a35_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a35_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a35_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a35_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a35_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a35_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a35_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C36;
impl JsClass for C36 {
    const NAME: &'static str = "C36";
    type Data = ();
    type Parent = C35;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m36_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m36_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a36_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a36_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a36_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a36_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a36_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a36_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a36_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a36_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a36_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a36_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a36_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a36_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a36_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a36_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a36_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C37;
impl JsClass for C37 {
    const NAME: &'static str = "C37";
    type Data = ();
    type Parent = C36;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m37_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m37_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a37_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a37_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a37_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a37_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a37_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a37_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a37_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a37_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a37_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a37_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a37_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a37_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a37_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a37_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a37_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C38;
impl JsClass for C38 {
    const NAME: &'static str = "C38";
    type Data = ();
    type Parent = C37;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m38_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m38_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a38_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a38_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a38_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a38_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a38_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a38_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a38_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a38_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a38_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a38_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a38_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a38_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a38_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a38_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a38_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C39;
impl JsClass for C39 {
    const NAME: &'static str = "C39";
    type Data = ();
    type Parent = C38;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m39_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m39_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a39_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a39_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a39_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a39_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a39_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a39_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a39_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a39_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a39_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a39_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a39_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a39_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a39_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a39_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a39_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C40;
impl JsClass for C40 {
    const NAME: &'static str = "C40";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m40_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m40_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a40_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a40_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a40_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a40_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a40_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a40_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a40_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a40_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a40_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a40_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a40_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a40_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a40_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a40_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a40_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C41;
impl JsClass for C41 {
    const NAME: &'static str = "C41";
    type Data = ();
    type Parent = C40;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m41_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m41_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a41_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a41_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a41_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a41_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a41_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a41_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a41_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a41_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a41_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a41_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a41_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a41_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a41_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a41_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a41_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C42;
impl JsClass for C42 {
    const NAME: &'static str = "C42";
    type Data = ();
    type Parent = C41;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m42_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m42_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a42_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a42_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a42_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a42_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a42_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a42_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a42_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a42_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a42_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a42_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a42_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a42_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a42_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a42_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a42_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C43;
impl JsClass for C43 {
    const NAME: &'static str = "C43";
    type Data = ();
    type Parent = C42;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m43_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m43_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a43_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a43_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a43_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a43_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a43_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a43_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a43_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a43_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a43_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a43_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a43_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a43_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a43_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a43_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a43_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C44;
impl JsClass for C44 {
    const NAME: &'static str = "C44";
    type Data = ();
    type Parent = C43;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m44_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m44_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a44_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a44_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a44_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a44_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a44_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a44_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a44_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a44_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a44_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a44_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a44_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a44_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a44_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a44_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a44_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C45;
impl JsClass for C45 {
    const NAME: &'static str = "C45";
    type Data = ();
    type Parent = C44;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m45_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m45_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a45_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a45_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a45_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a45_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a45_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a45_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a45_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a45_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a45_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a45_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a45_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a45_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a45_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a45_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a45_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C46;
impl JsClass for C46 {
    const NAME: &'static str = "C46";
    type Data = ();
    type Parent = C45;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m46_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m46_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a46_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a46_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a46_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a46_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a46_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a46_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a46_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a46_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a46_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a46_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a46_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a46_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a46_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a46_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a46_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C47;
impl JsClass for C47 {
    const NAME: &'static str = "C47";
    type Data = ();
    type Parent = C46;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m47_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m47_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a47_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a47_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a47_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a47_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a47_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a47_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a47_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a47_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a47_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a47_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a47_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a47_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a47_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a47_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a47_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C48;
impl JsClass for C48 {
    const NAME: &'static str = "C48";
    type Data = ();
    type Parent = C47;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m48_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m48_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a48_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a48_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a48_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a48_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a48_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a48_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a48_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a48_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a48_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a48_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a48_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a48_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a48_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a48_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a48_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C49;
impl JsClass for C49 {
    const NAME: &'static str = "C49";
    type Data = ();
    type Parent = C48;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m49_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m49_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a49_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a49_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a49_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a49_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a49_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a49_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a49_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a49_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a49_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a49_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a49_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a49_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a49_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a49_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a49_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C50;
impl JsClass for C50 {
    const NAME: &'static str = "C50";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m50_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m50_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a50_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a50_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a50_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a50_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a50_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a50_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a50_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a50_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a50_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a50_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a50_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a50_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a50_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a50_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a50_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C51;
impl JsClass for C51 {
    const NAME: &'static str = "C51";
    type Data = ();
    type Parent = C50;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m51_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m51_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a51_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a51_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a51_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a51_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a51_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a51_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a51_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a51_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a51_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a51_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a51_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a51_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a51_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a51_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a51_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C52;
impl JsClass for C52 {
    const NAME: &'static str = "C52";
    type Data = ();
    type Parent = C51;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m52_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m52_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a52_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a52_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a52_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a52_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a52_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a52_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a52_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a52_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a52_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a52_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a52_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a52_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a52_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a52_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a52_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C53;
impl JsClass for C53 {
    const NAME: &'static str = "C53";
    type Data = ();
    type Parent = C52;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m53_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m53_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a53_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a53_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a53_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a53_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a53_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a53_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a53_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a53_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a53_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a53_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a53_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a53_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a53_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a53_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a53_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C54;
impl JsClass for C54 {
    const NAME: &'static str = "C54";
    type Data = ();
    type Parent = C53;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m54_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m54_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a54_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a54_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a54_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a54_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a54_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a54_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a54_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a54_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a54_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a54_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a54_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a54_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a54_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a54_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a54_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C55;
impl JsClass for C55 {
    const NAME: &'static str = "C55";
    type Data = ();
    type Parent = C54;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m55_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m55_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a55_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a55_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a55_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a55_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a55_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a55_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a55_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a55_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a55_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a55_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a55_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a55_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a55_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a55_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a55_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C56;
impl JsClass for C56 {
    const NAME: &'static str = "C56";
    type Data = ();
    type Parent = C55;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m56_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m56_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a56_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a56_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a56_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a56_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a56_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a56_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a56_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a56_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a56_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a56_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a56_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a56_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a56_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a56_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a56_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C57;
impl JsClass for C57 {
    const NAME: &'static str = "C57";
    type Data = ();
    type Parent = C56;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m57_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m57_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a57_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a57_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a57_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a57_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a57_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a57_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a57_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a57_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a57_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a57_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a57_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a57_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a57_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a57_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a57_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C58;
impl JsClass for C58 {
    const NAME: &'static str = "C58";
    type Data = ();
    type Parent = C57;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m58_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m58_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a58_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a58_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a58_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a58_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a58_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a58_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a58_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a58_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a58_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a58_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a58_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a58_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a58_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a58_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a58_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C59;
impl JsClass for C59 {
    const NAME: &'static str = "C59";
    type Data = ();
    type Parent = C58;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m59_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m59_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a59_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a59_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a59_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a59_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a59_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a59_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a59_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a59_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a59_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a59_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a59_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a59_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a59_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a59_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a59_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C60;
impl JsClass for C60 {
    const NAME: &'static str = "C60";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m60_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m60_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a60_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a60_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a60_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a60_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a60_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a60_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a60_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a60_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a60_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a60_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a60_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a60_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a60_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a60_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a60_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C61;
impl JsClass for C61 {
    const NAME: &'static str = "C61";
    type Data = ();
    type Parent = C60;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m61_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m61_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a61_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a61_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a61_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a61_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a61_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a61_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a61_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a61_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a61_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a61_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a61_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a61_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a61_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a61_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a61_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C62;
impl JsClass for C62 {
    const NAME: &'static str = "C62";
    type Data = ();
    type Parent = C61;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m62_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m62_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a62_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a62_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a62_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a62_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a62_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a62_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a62_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a62_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a62_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a62_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a62_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a62_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a62_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a62_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a62_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C63;
impl JsClass for C63 {
    const NAME: &'static str = "C63";
    type Data = ();
    type Parent = C62;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m63_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m63_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a63_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a63_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a63_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a63_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a63_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a63_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a63_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a63_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a63_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a63_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a63_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a63_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a63_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a63_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a63_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C64;
impl JsClass for C64 {
    const NAME: &'static str = "C64";
    type Data = ();
    type Parent = C63;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m64_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m64_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a64_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a64_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a64_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a64_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a64_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a64_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a64_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a64_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a64_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a64_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a64_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a64_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a64_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a64_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a64_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C65;
impl JsClass for C65 {
    const NAME: &'static str = "C65";
    type Data = ();
    type Parent = C64;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m65_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m65_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a65_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a65_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a65_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a65_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a65_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a65_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a65_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a65_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a65_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a65_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a65_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a65_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a65_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a65_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a65_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C66;
impl JsClass for C66 {
    const NAME: &'static str = "C66";
    type Data = ();
    type Parent = C65;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m66_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m66_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a66_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a66_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a66_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a66_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a66_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a66_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a66_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a66_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a66_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a66_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a66_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a66_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a66_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a66_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a66_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C67;
impl JsClass for C67 {
    const NAME: &'static str = "C67";
    type Data = ();
    type Parent = C66;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m67_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m67_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a67_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a67_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a67_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a67_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a67_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a67_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a67_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a67_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a67_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a67_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a67_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a67_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a67_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a67_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a67_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C68;
impl JsClass for C68 {
    const NAME: &'static str = "C68";
    type Data = ();
    type Parent = C67;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m68_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m68_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a68_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a68_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a68_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a68_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a68_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a68_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a68_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a68_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a68_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a68_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a68_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a68_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a68_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a68_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a68_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C69;
impl JsClass for C69 {
    const NAME: &'static str = "C69";
    type Data = ();
    type Parent = C68;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m69_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m69_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a69_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a69_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a69_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a69_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a69_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a69_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a69_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a69_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a69_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a69_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a69_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a69_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a69_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a69_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a69_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C70;
impl JsClass for C70 {
    const NAME: &'static str = "C70";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m70_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m70_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a70_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a70_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a70_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a70_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a70_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a70_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a70_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a70_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a70_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a70_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a70_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a70_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a70_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a70_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a70_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C71;
impl JsClass for C71 {
    const NAME: &'static str = "C71";
    type Data = ();
    type Parent = C70;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m71_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m71_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a71_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a71_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a71_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a71_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a71_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a71_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a71_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a71_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a71_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a71_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a71_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a71_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a71_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a71_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a71_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C72;
impl JsClass for C72 {
    const NAME: &'static str = "C72";
    type Data = ();
    type Parent = C71;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m72_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m72_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a72_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a72_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a72_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a72_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a72_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a72_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a72_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a72_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a72_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a72_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a72_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a72_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a72_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a72_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a72_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C73;
impl JsClass for C73 {
    const NAME: &'static str = "C73";
    type Data = ();
    type Parent = C72;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m73_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m73_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a73_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a73_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a73_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a73_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a73_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a73_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a73_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a73_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a73_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a73_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a73_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a73_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a73_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a73_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a73_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C74;
impl JsClass for C74 {
    const NAME: &'static str = "C74";
    type Data = ();
    type Parent = C73;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m74_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m74_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a74_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a74_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a74_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a74_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a74_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a74_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a74_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a74_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a74_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a74_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a74_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a74_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a74_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a74_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a74_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C75;
impl JsClass for C75 {
    const NAME: &'static str = "C75";
    type Data = ();
    type Parent = C74;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m75_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m75_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a75_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a75_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a75_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a75_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a75_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a75_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a75_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a75_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a75_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a75_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a75_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a75_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a75_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a75_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a75_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C76;
impl JsClass for C76 {
    const NAME: &'static str = "C76";
    type Data = ();
    type Parent = C75;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m76_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m76_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a76_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a76_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a76_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a76_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a76_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a76_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a76_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a76_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a76_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a76_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a76_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a76_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a76_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a76_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a76_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C77;
impl JsClass for C77 {
    const NAME: &'static str = "C77";
    type Data = ();
    type Parent = C76;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m77_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m77_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a77_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a77_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a77_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a77_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a77_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a77_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a77_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a77_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a77_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a77_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a77_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a77_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a77_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a77_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a77_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C78;
impl JsClass for C78 {
    const NAME: &'static str = "C78";
    type Data = ();
    type Parent = C77;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m78_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m78_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a78_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a78_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a78_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a78_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a78_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a78_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a78_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a78_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a78_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a78_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a78_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a78_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a78_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a78_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a78_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C79;
impl JsClass for C79 {
    const NAME: &'static str = "C79";
    type Data = ();
    type Parent = C78;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m79_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m79_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a79_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a79_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a79_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a79_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a79_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a79_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a79_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a79_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a79_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a79_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a79_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a79_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a79_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a79_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a79_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C80;
impl JsClass for C80 {
    const NAME: &'static str = "C80";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m80_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m80_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a80_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a80_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a80_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a80_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a80_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a80_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a80_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a80_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a80_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a80_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a80_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a80_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a80_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a80_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a80_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C81;
impl JsClass for C81 {
    const NAME: &'static str = "C81";
    type Data = ();
    type Parent = C80;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m81_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m81_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a81_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a81_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a81_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a81_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a81_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a81_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a81_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a81_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a81_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a81_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a81_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a81_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a81_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a81_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a81_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C82;
impl JsClass for C82 {
    const NAME: &'static str = "C82";
    type Data = ();
    type Parent = C81;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m82_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m82_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a82_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a82_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a82_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a82_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a82_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a82_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a82_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a82_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a82_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a82_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a82_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a82_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a82_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a82_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a82_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C83;
impl JsClass for C83 {
    const NAME: &'static str = "C83";
    type Data = ();
    type Parent = C82;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m83_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m83_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a83_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a83_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a83_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a83_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a83_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a83_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a83_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a83_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a83_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a83_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a83_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a83_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a83_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a83_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a83_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C84;
impl JsClass for C84 {
    const NAME: &'static str = "C84";
    type Data = ();
    type Parent = C83;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m84_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m84_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a84_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a84_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a84_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a84_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a84_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a84_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a84_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a84_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a84_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a84_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a84_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a84_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a84_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a84_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a84_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C85;
impl JsClass for C85 {
    const NAME: &'static str = "C85";
    type Data = ();
    type Parent = C84;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m85_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m85_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a85_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a85_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a85_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a85_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a85_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a85_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a85_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a85_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a85_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a85_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a85_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a85_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a85_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a85_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a85_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C86;
impl JsClass for C86 {
    const NAME: &'static str = "C86";
    type Data = ();
    type Parent = C85;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m86_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m86_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a86_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a86_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a86_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a86_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a86_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a86_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a86_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a86_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a86_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a86_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a86_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a86_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a86_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a86_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a86_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C87;
impl JsClass for C87 {
    const NAME: &'static str = "C87";
    type Data = ();
    type Parent = C86;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m87_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m87_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a87_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a87_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a87_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a87_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a87_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a87_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a87_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a87_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a87_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a87_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a87_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a87_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a87_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a87_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a87_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C88;
impl JsClass for C88 {
    const NAME: &'static str = "C88";
    type Data = ();
    type Parent = C87;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m88_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m88_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a88_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a88_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a88_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a88_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a88_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a88_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a88_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a88_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a88_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a88_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a88_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a88_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a88_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a88_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a88_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C89;
impl JsClass for C89 {
    const NAME: &'static str = "C89";
    type Data = ();
    type Parent = C88;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m89_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m89_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a89_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a89_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a89_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a89_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a89_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a89_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a89_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a89_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a89_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a89_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a89_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a89_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a89_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a89_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a89_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C90;
impl JsClass for C90 {
    const NAME: &'static str = "C90";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m90_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m90_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a90_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a90_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a90_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a90_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a90_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a90_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a90_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a90_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a90_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a90_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a90_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a90_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a90_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a90_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a90_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C91;
impl JsClass for C91 {
    const NAME: &'static str = "C91";
    type Data = ();
    type Parent = C90;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m91_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m91_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a91_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a91_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a91_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a91_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a91_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a91_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a91_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a91_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a91_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a91_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a91_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a91_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a91_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a91_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a91_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C92;
impl JsClass for C92 {
    const NAME: &'static str = "C92";
    type Data = ();
    type Parent = C91;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m92_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m92_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a92_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a92_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a92_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a92_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a92_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a92_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a92_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a92_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a92_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a92_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a92_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a92_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a92_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a92_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a92_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C93;
impl JsClass for C93 {
    const NAME: &'static str = "C93";
    type Data = ();
    type Parent = C92;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m93_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m93_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a93_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a93_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a93_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a93_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a93_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a93_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a93_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a93_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a93_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a93_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a93_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a93_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a93_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a93_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a93_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C94;
impl JsClass for C94 {
    const NAME: &'static str = "C94";
    type Data = ();
    type Parent = C93;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m94_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m94_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a94_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a94_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a94_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a94_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a94_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a94_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a94_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a94_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a94_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a94_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a94_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a94_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a94_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a94_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a94_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C95;
impl JsClass for C95 {
    const NAME: &'static str = "C95";
    type Data = ();
    type Parent = C94;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m95_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m95_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a95_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a95_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a95_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a95_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a95_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a95_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a95_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a95_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a95_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a95_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a95_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a95_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a95_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a95_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a95_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C96;
impl JsClass for C96 {
    const NAME: &'static str = "C96";
    type Data = ();
    type Parent = C95;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m96_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m96_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a96_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a96_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a96_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a96_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a96_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a96_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a96_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a96_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a96_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a96_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a96_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a96_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a96_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a96_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a96_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C97;
impl JsClass for C97 {
    const NAME: &'static str = "C97";
    type Data = ();
    type Parent = C96;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m97_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m97_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a97_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a97_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a97_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a97_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a97_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a97_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a97_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a97_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a97_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a97_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a97_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a97_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a97_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a97_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a97_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C98;
impl JsClass for C98 {
    const NAME: &'static str = "C98";
    type Data = ();
    type Parent = C97;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m98_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m98_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a98_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a98_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a98_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a98_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a98_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a98_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a98_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a98_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a98_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a98_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a98_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a98_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a98_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a98_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a98_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C99;
impl JsClass for C99 {
    const NAME: &'static str = "C99";
    type Data = ();
    type Parent = C98;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m99_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m99_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a99_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a99_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a99_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a99_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a99_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a99_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a99_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a99_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a99_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a99_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a99_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a99_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a99_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a99_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a99_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C100;
impl JsClass for C100 {
    const NAME: &'static str = "C100";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m100_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m100_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a100_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a100_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a100_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a100_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a100_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a100_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a100_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a100_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a100_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a100_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a100_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a100_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a100_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a100_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a100_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C101;
impl JsClass for C101 {
    const NAME: &'static str = "C101";
    type Data = ();
    type Parent = C100;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m101_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m101_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a101_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a101_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a101_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a101_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a101_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a101_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a101_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a101_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a101_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a101_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a101_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a101_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a101_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a101_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a101_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C102;
impl JsClass for C102 {
    const NAME: &'static str = "C102";
    type Data = ();
    type Parent = C101;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m102_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m102_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a102_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a102_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a102_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a102_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a102_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a102_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a102_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a102_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a102_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a102_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a102_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a102_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a102_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a102_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a102_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C103;
impl JsClass for C103 {
    const NAME: &'static str = "C103";
    type Data = ();
    type Parent = C102;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m103_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m103_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a103_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a103_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a103_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a103_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a103_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a103_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a103_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a103_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a103_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a103_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a103_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a103_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a103_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a103_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a103_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C104;
impl JsClass for C104 {
    const NAME: &'static str = "C104";
    type Data = ();
    type Parent = C103;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m104_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m104_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a104_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a104_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a104_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a104_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a104_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a104_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a104_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a104_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a104_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a104_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a104_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a104_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a104_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a104_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a104_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C105;
impl JsClass for C105 {
    const NAME: &'static str = "C105";
    type Data = ();
    type Parent = C104;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m105_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m105_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a105_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a105_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a105_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a105_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a105_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a105_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a105_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a105_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a105_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a105_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a105_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a105_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a105_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a105_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a105_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C106;
impl JsClass for C106 {
    const NAME: &'static str = "C106";
    type Data = ();
    type Parent = C105;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m106_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m106_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a106_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a106_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a106_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a106_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a106_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a106_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a106_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a106_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a106_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a106_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a106_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a106_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a106_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a106_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a106_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C107;
impl JsClass for C107 {
    const NAME: &'static str = "C107";
    type Data = ();
    type Parent = C106;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m107_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m107_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a107_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a107_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a107_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a107_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a107_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a107_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a107_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a107_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a107_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a107_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a107_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a107_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a107_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a107_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a107_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C108;
impl JsClass for C108 {
    const NAME: &'static str = "C108";
    type Data = ();
    type Parent = C107;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m108_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m108_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a108_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a108_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a108_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a108_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a108_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a108_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a108_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a108_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a108_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a108_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a108_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a108_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a108_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a108_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a108_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C109;
impl JsClass for C109 {
    const NAME: &'static str = "C109";
    type Data = ();
    type Parent = C108;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m109_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m109_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a109_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a109_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a109_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a109_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a109_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a109_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a109_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a109_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a109_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a109_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a109_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a109_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a109_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a109_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a109_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C110;
impl JsClass for C110 {
    const NAME: &'static str = "C110";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m110_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m110_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a110_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a110_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a110_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a110_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a110_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a110_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a110_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a110_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a110_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a110_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a110_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a110_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a110_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a110_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a110_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C111;
impl JsClass for C111 {
    const NAME: &'static str = "C111";
    type Data = ();
    type Parent = C110;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m111_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m111_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a111_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a111_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a111_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a111_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a111_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a111_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a111_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a111_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a111_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a111_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a111_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a111_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a111_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a111_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a111_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C112;
impl JsClass for C112 {
    const NAME: &'static str = "C112";
    type Data = ();
    type Parent = C111;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m112_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m112_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a112_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a112_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a112_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a112_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a112_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a112_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a112_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a112_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a112_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a112_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a112_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a112_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a112_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a112_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a112_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C113;
impl JsClass for C113 {
    const NAME: &'static str = "C113";
    type Data = ();
    type Parent = C112;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m113_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m113_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a113_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a113_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a113_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a113_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a113_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a113_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a113_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a113_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a113_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a113_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a113_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a113_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a113_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a113_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a113_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C114;
impl JsClass for C114 {
    const NAME: &'static str = "C114";
    type Data = ();
    type Parent = C113;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m114_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m114_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a114_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a114_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a114_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a114_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a114_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a114_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a114_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a114_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a114_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a114_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a114_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a114_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a114_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a114_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a114_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C115;
impl JsClass for C115 {
    const NAME: &'static str = "C115";
    type Data = ();
    type Parent = C114;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m115_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m115_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a115_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a115_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a115_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a115_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a115_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a115_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a115_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a115_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a115_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a115_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a115_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a115_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a115_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a115_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a115_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C116;
impl JsClass for C116 {
    const NAME: &'static str = "C116";
    type Data = ();
    type Parent = C115;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m116_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m116_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a116_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a116_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a116_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a116_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a116_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a116_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a116_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a116_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a116_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a116_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a116_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a116_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a116_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a116_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a116_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C117;
impl JsClass for C117 {
    const NAME: &'static str = "C117";
    type Data = ();
    type Parent = C116;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m117_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m117_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a117_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a117_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a117_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a117_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a117_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a117_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a117_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a117_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a117_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a117_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a117_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a117_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a117_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a117_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a117_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C118;
impl JsClass for C118 {
    const NAME: &'static str = "C118";
    type Data = ();
    type Parent = C117;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m118_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m118_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a118_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a118_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a118_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a118_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a118_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a118_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a118_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a118_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a118_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a118_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a118_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a118_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a118_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a118_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a118_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C119;
impl JsClass for C119 {
    const NAME: &'static str = "C119";
    type Data = ();
    type Parent = C118;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m119_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m119_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a119_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a119_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a119_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a119_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a119_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a119_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a119_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a119_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a119_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a119_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a119_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a119_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a119_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a119_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a119_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C120;
impl JsClass for C120 {
    const NAME: &'static str = "C120";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m120_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m120_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a120_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a120_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a120_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a120_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a120_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a120_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a120_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a120_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a120_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a120_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a120_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a120_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a120_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a120_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a120_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C121;
impl JsClass for C121 {
    const NAME: &'static str = "C121";
    type Data = ();
    type Parent = C120;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m121_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m121_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a121_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a121_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a121_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a121_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a121_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a121_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a121_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a121_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a121_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a121_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a121_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a121_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a121_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a121_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a121_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C122;
impl JsClass for C122 {
    const NAME: &'static str = "C122";
    type Data = ();
    type Parent = C121;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m122_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m122_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a122_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a122_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a122_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a122_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a122_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a122_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a122_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a122_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a122_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a122_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a122_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a122_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a122_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a122_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a122_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C123;
impl JsClass for C123 {
    const NAME: &'static str = "C123";
    type Data = ();
    type Parent = C122;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m123_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m123_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a123_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a123_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a123_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a123_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a123_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a123_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a123_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a123_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a123_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a123_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a123_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a123_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a123_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a123_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a123_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C124;
impl JsClass for C124 {
    const NAME: &'static str = "C124";
    type Data = ();
    type Parent = C123;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m124_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m124_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a124_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a124_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a124_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a124_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a124_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a124_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a124_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a124_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a124_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a124_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a124_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a124_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a124_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a124_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a124_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C125;
impl JsClass for C125 {
    const NAME: &'static str = "C125";
    type Data = ();
    type Parent = C124;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m125_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m125_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a125_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a125_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a125_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a125_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a125_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a125_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a125_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a125_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a125_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a125_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a125_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a125_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a125_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a125_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a125_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C126;
impl JsClass for C126 {
    const NAME: &'static str = "C126";
    type Data = ();
    type Parent = C125;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m126_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m126_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a126_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a126_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a126_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a126_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a126_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a126_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a126_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a126_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a126_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a126_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a126_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a126_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a126_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a126_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a126_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C127;
impl JsClass for C127 {
    const NAME: &'static str = "C127";
    type Data = ();
    type Parent = C126;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m127_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m127_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a127_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a127_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a127_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a127_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a127_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a127_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a127_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a127_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a127_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a127_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a127_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a127_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a127_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a127_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a127_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C128;
impl JsClass for C128 {
    const NAME: &'static str = "C128";
    type Data = ();
    type Parent = C127;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m128_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m128_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a128_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a128_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a128_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a128_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a128_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a128_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a128_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a128_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a128_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a128_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a128_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a128_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a128_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a128_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a128_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C129;
impl JsClass for C129 {
    const NAME: &'static str = "C129";
    type Data = ();
    type Parent = C128;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m129_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m129_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a129_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a129_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a129_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a129_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a129_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a129_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a129_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a129_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a129_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a129_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a129_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a129_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a129_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a129_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a129_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C130;
impl JsClass for C130 {
    const NAME: &'static str = "C130";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m130_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m130_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a130_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a130_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a130_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a130_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a130_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a130_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a130_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a130_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a130_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a130_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a130_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a130_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a130_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a130_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a130_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C131;
impl JsClass for C131 {
    const NAME: &'static str = "C131";
    type Data = ();
    type Parent = C130;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m131_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m131_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a131_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a131_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a131_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a131_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a131_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a131_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a131_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a131_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a131_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a131_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a131_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a131_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a131_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a131_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a131_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C132;
impl JsClass for C132 {
    const NAME: &'static str = "C132";
    type Data = ();
    type Parent = C131;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m132_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m132_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a132_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a132_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a132_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a132_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a132_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a132_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a132_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a132_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a132_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a132_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a132_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a132_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a132_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a132_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a132_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C133;
impl JsClass for C133 {
    const NAME: &'static str = "C133";
    type Data = ();
    type Parent = C132;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m133_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m133_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a133_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a133_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a133_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a133_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a133_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a133_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a133_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a133_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a133_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a133_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a133_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a133_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a133_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a133_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a133_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C134;
impl JsClass for C134 {
    const NAME: &'static str = "C134";
    type Data = ();
    type Parent = C133;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m134_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m134_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a134_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a134_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a134_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a134_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a134_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a134_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a134_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a134_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a134_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a134_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a134_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a134_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a134_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a134_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a134_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C135;
impl JsClass for C135 {
    const NAME: &'static str = "C135";
    type Data = ();
    type Parent = C134;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m135_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m135_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a135_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a135_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a135_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a135_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a135_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a135_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a135_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a135_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a135_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a135_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a135_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a135_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a135_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a135_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a135_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C136;
impl JsClass for C136 {
    const NAME: &'static str = "C136";
    type Data = ();
    type Parent = C135;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m136_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m136_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a136_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a136_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a136_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a136_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a136_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a136_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a136_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a136_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a136_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a136_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a136_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a136_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a136_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a136_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a136_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C137;
impl JsClass for C137 {
    const NAME: &'static str = "C137";
    type Data = ();
    type Parent = C136;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m137_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m137_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a137_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a137_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a137_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a137_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a137_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a137_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a137_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a137_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a137_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a137_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a137_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a137_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a137_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a137_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a137_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C138;
impl JsClass for C138 {
    const NAME: &'static str = "C138";
    type Data = ();
    type Parent = C137;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m138_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m138_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a138_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a138_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a138_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a138_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a138_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a138_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a138_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a138_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a138_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a138_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a138_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a138_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a138_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a138_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a138_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C139;
impl JsClass for C139 {
    const NAME: &'static str = "C139";
    type Data = ();
    type Parent = C138;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m139_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m139_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a139_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a139_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a139_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a139_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a139_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a139_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a139_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a139_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a139_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a139_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a139_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a139_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a139_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a139_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a139_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C140;
impl JsClass for C140 {
    const NAME: &'static str = "C140";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m140_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m140_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a140_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a140_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a140_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a140_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a140_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a140_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a140_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a140_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a140_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a140_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a140_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a140_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a140_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a140_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a140_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C141;
impl JsClass for C141 {
    const NAME: &'static str = "C141";
    type Data = ();
    type Parent = C140;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m141_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m141_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a141_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a141_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a141_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a141_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a141_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a141_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a141_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a141_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a141_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a141_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a141_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a141_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a141_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a141_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a141_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C142;
impl JsClass for C142 {
    const NAME: &'static str = "C142";
    type Data = ();
    type Parent = C141;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m142_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m142_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a142_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a142_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a142_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a142_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a142_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a142_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a142_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a142_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a142_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a142_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a142_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a142_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a142_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a142_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a142_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C143;
impl JsClass for C143 {
    const NAME: &'static str = "C143";
    type Data = ();
    type Parent = C142;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m143_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m143_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a143_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a143_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a143_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a143_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a143_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a143_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a143_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a143_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a143_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a143_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a143_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a143_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a143_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a143_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a143_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C144;
impl JsClass for C144 {
    const NAME: &'static str = "C144";
    type Data = ();
    type Parent = C143;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m144_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m144_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a144_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a144_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a144_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a144_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a144_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a144_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a144_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a144_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a144_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a144_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a144_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a144_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a144_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a144_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a144_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C145;
impl JsClass for C145 {
    const NAME: &'static str = "C145";
    type Data = ();
    type Parent = C144;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m145_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m145_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a145_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a145_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a145_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a145_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a145_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a145_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a145_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a145_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a145_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a145_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a145_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a145_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a145_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a145_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a145_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C146;
impl JsClass for C146 {
    const NAME: &'static str = "C146";
    type Data = ();
    type Parent = C145;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m146_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m146_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a146_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a146_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a146_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a146_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a146_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a146_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a146_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a146_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a146_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a146_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a146_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a146_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a146_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a146_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a146_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C147;
impl JsClass for C147 {
    const NAME: &'static str = "C147";
    type Data = ();
    type Parent = C146;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m147_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m147_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a147_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a147_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a147_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a147_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a147_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a147_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a147_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a147_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a147_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a147_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a147_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a147_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a147_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a147_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a147_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C148;
impl JsClass for C148 {
    const NAME: &'static str = "C148";
    type Data = ();
    type Parent = C147;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m148_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m148_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a148_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a148_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a148_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a148_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a148_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a148_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a148_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a148_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a148_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a148_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a148_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a148_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a148_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a148_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a148_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C149;
impl JsClass for C149 {
    const NAME: &'static str = "C149";
    type Data = ();
    type Parent = C148;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m149_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m149_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a149_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a149_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a149_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a149_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a149_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a149_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a149_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a149_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a149_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a149_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a149_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a149_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a149_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a149_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a149_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C150;
impl JsClass for C150 {
    const NAME: &'static str = "C150";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m150_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m150_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a150_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a150_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a150_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a150_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a150_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a150_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a150_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a150_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a150_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a150_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a150_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a150_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a150_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a150_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a150_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C151;
impl JsClass for C151 {
    const NAME: &'static str = "C151";
    type Data = ();
    type Parent = C150;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m151_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m151_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a151_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a151_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a151_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a151_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a151_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a151_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a151_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a151_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a151_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a151_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a151_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a151_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a151_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a151_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a151_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C152;
impl JsClass for C152 {
    const NAME: &'static str = "C152";
    type Data = ();
    type Parent = C151;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m152_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m152_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a152_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a152_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a152_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a152_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a152_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a152_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a152_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a152_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a152_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a152_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a152_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a152_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a152_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a152_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a152_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C153;
impl JsClass for C153 {
    const NAME: &'static str = "C153";
    type Data = ();
    type Parent = C152;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m153_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m153_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a153_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a153_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a153_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a153_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a153_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a153_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a153_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a153_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a153_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a153_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a153_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a153_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a153_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a153_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a153_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C154;
impl JsClass for C154 {
    const NAME: &'static str = "C154";
    type Data = ();
    type Parent = C153;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m154_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m154_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a154_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a154_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a154_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a154_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a154_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a154_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a154_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a154_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a154_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a154_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a154_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a154_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a154_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a154_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a154_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C155;
impl JsClass for C155 {
    const NAME: &'static str = "C155";
    type Data = ();
    type Parent = C154;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m155_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m155_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a155_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a155_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a155_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a155_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a155_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a155_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a155_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a155_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a155_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a155_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a155_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a155_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a155_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a155_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a155_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C156;
impl JsClass for C156 {
    const NAME: &'static str = "C156";
    type Data = ();
    type Parent = C155;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m156_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m156_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a156_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a156_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a156_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a156_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a156_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a156_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a156_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a156_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a156_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a156_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a156_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a156_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a156_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a156_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a156_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C157;
impl JsClass for C157 {
    const NAME: &'static str = "C157";
    type Data = ();
    type Parent = C156;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m157_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m157_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a157_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a157_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a157_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a157_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a157_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a157_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a157_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a157_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a157_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a157_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a157_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a157_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a157_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a157_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a157_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C158;
impl JsClass for C158 {
    const NAME: &'static str = "C158";
    type Data = ();
    type Parent = C157;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m158_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m158_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a158_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a158_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a158_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a158_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a158_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a158_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a158_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a158_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a158_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a158_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a158_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a158_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a158_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a158_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a158_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C159;
impl JsClass for C159 {
    const NAME: &'static str = "C159";
    type Data = ();
    type Parent = C158;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m159_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m159_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a159_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a159_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a159_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a159_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a159_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a159_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a159_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a159_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a159_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a159_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a159_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a159_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a159_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a159_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a159_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C160;
impl JsClass for C160 {
    const NAME: &'static str = "C160";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m160_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m160_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a160_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a160_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a160_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a160_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a160_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a160_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a160_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a160_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a160_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a160_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a160_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a160_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a160_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a160_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a160_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C161;
impl JsClass for C161 {
    const NAME: &'static str = "C161";
    type Data = ();
    type Parent = C160;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m161_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m161_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a161_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a161_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a161_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a161_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a161_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a161_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a161_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a161_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a161_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a161_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a161_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a161_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a161_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a161_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a161_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C162;
impl JsClass for C162 {
    const NAME: &'static str = "C162";
    type Data = ();
    type Parent = C161;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m162_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m162_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a162_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a162_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a162_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a162_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a162_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a162_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a162_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a162_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a162_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a162_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a162_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a162_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a162_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a162_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a162_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C163;
impl JsClass for C163 {
    const NAME: &'static str = "C163";
    type Data = ();
    type Parent = C162;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m163_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m163_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a163_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a163_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a163_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a163_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a163_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a163_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a163_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a163_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a163_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a163_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a163_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a163_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a163_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a163_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a163_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C164;
impl JsClass for C164 {
    const NAME: &'static str = "C164";
    type Data = ();
    type Parent = C163;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m164_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m164_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a164_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a164_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a164_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a164_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a164_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a164_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a164_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a164_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a164_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a164_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a164_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a164_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a164_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a164_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a164_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C165;
impl JsClass for C165 {
    const NAME: &'static str = "C165";
    type Data = ();
    type Parent = C164;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m165_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m165_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a165_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a165_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a165_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a165_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a165_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a165_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a165_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a165_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a165_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a165_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a165_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a165_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a165_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a165_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a165_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C166;
impl JsClass for C166 {
    const NAME: &'static str = "C166";
    type Data = ();
    type Parent = C165;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m166_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m166_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a166_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a166_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a166_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a166_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a166_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a166_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a166_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a166_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a166_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a166_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a166_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a166_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a166_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a166_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a166_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C167;
impl JsClass for C167 {
    const NAME: &'static str = "C167";
    type Data = ();
    type Parent = C166;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m167_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m167_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a167_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a167_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a167_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a167_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a167_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a167_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a167_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a167_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a167_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a167_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a167_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a167_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a167_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a167_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a167_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C168;
impl JsClass for C168 {
    const NAME: &'static str = "C168";
    type Data = ();
    type Parent = C167;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m168_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m168_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a168_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a168_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a168_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a168_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a168_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a168_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a168_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a168_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a168_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a168_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a168_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a168_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a168_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a168_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a168_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C169;
impl JsClass for C169 {
    const NAME: &'static str = "C169";
    type Data = ();
    type Parent = C168;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m169_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m169_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a169_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a169_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a169_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a169_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a169_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a169_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a169_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a169_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a169_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a169_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a169_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a169_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a169_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a169_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a169_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C170;
impl JsClass for C170 {
    const NAME: &'static str = "C170";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m170_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m170_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a170_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a170_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a170_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a170_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a170_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a170_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a170_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a170_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a170_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a170_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a170_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a170_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a170_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a170_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a170_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C171;
impl JsClass for C171 {
    const NAME: &'static str = "C171";
    type Data = ();
    type Parent = C170;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m171_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m171_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a171_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a171_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a171_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a171_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a171_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a171_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a171_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a171_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a171_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a171_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a171_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a171_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a171_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a171_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a171_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C172;
impl JsClass for C172 {
    const NAME: &'static str = "C172";
    type Data = ();
    type Parent = C171;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m172_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m172_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a172_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a172_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a172_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a172_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a172_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a172_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a172_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a172_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a172_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a172_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a172_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a172_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a172_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a172_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a172_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C173;
impl JsClass for C173 {
    const NAME: &'static str = "C173";
    type Data = ();
    type Parent = C172;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m173_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m173_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a173_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a173_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a173_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a173_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a173_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a173_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a173_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a173_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a173_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a173_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a173_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a173_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a173_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a173_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a173_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C174;
impl JsClass for C174 {
    const NAME: &'static str = "C174";
    type Data = ();
    type Parent = C173;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m174_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m174_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a174_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a174_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a174_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a174_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a174_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a174_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a174_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a174_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a174_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a174_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a174_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a174_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a174_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a174_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a174_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C175;
impl JsClass for C175 {
    const NAME: &'static str = "C175";
    type Data = ();
    type Parent = C174;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m175_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m175_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a175_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a175_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a175_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a175_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a175_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a175_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a175_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a175_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a175_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a175_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a175_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a175_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a175_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a175_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a175_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C176;
impl JsClass for C176 {
    const NAME: &'static str = "C176";
    type Data = ();
    type Parent = C175;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m176_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m176_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a176_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a176_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a176_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a176_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a176_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a176_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a176_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a176_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a176_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a176_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a176_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a176_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a176_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a176_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a176_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C177;
impl JsClass for C177 {
    const NAME: &'static str = "C177";
    type Data = ();
    type Parent = C176;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m177_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m177_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a177_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a177_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a177_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a177_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a177_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a177_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a177_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a177_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a177_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a177_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a177_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a177_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a177_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a177_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a177_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C178;
impl JsClass for C178 {
    const NAME: &'static str = "C178";
    type Data = ();
    type Parent = C177;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m178_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m178_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a178_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a178_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a178_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a178_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a178_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a178_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a178_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a178_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a178_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a178_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a178_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a178_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a178_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a178_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a178_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C179;
impl JsClass for C179 {
    const NAME: &'static str = "C179";
    type Data = ();
    type Parent = C178;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m179_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m179_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a179_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a179_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a179_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a179_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a179_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a179_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a179_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a179_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a179_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a179_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a179_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a179_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a179_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a179_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a179_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C180;
impl JsClass for C180 {
    const NAME: &'static str = "C180";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m180_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m180_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a180_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a180_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a180_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a180_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a180_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a180_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a180_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a180_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a180_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a180_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a180_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a180_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a180_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a180_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a180_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C181;
impl JsClass for C181 {
    const NAME: &'static str = "C181";
    type Data = ();
    type Parent = C180;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m181_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m181_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a181_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a181_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a181_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a181_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a181_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a181_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a181_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a181_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a181_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a181_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a181_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a181_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a181_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a181_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a181_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C182;
impl JsClass for C182 {
    const NAME: &'static str = "C182";
    type Data = ();
    type Parent = C181;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m182_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m182_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a182_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a182_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a182_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a182_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a182_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a182_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a182_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a182_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a182_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a182_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a182_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a182_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a182_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a182_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a182_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C183;
impl JsClass for C183 {
    const NAME: &'static str = "C183";
    type Data = ();
    type Parent = C182;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m183_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m183_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a183_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a183_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a183_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a183_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a183_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a183_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a183_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a183_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a183_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a183_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a183_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a183_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a183_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a183_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a183_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C184;
impl JsClass for C184 {
    const NAME: &'static str = "C184";
    type Data = ();
    type Parent = C183;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m184_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m184_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a184_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a184_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a184_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a184_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a184_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a184_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a184_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a184_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a184_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a184_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a184_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a184_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a184_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a184_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a184_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C185;
impl JsClass for C185 {
    const NAME: &'static str = "C185";
    type Data = ();
    type Parent = C184;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m185_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m185_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a185_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a185_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a185_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a185_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a185_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a185_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a185_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a185_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a185_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a185_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a185_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a185_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a185_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a185_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a185_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C186;
impl JsClass for C186 {
    const NAME: &'static str = "C186";
    type Data = ();
    type Parent = C185;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m186_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m186_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a186_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a186_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a186_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a186_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a186_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a186_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a186_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a186_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a186_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a186_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a186_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a186_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a186_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a186_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a186_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C187;
impl JsClass for C187 {
    const NAME: &'static str = "C187";
    type Data = ();
    type Parent = C186;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m187_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m187_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a187_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a187_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a187_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a187_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a187_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a187_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a187_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a187_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a187_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a187_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a187_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a187_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a187_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a187_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a187_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C188;
impl JsClass for C188 {
    const NAME: &'static str = "C188";
    type Data = ();
    type Parent = C187;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m188_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m188_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a188_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a188_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a188_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a188_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a188_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a188_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a188_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a188_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a188_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a188_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a188_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a188_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a188_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a188_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a188_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C189;
impl JsClass for C189 {
    const NAME: &'static str = "C189";
    type Data = ();
    type Parent = C188;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m189_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m189_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a189_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a189_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a189_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a189_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a189_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a189_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a189_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a189_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a189_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a189_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a189_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a189_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a189_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a189_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a189_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C190;
impl JsClass for C190 {
    const NAME: &'static str = "C190";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m190_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m190_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a190_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a190_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a190_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a190_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a190_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a190_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a190_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a190_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a190_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a190_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a190_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a190_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a190_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a190_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a190_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C191;
impl JsClass for C191 {
    const NAME: &'static str = "C191";
    type Data = ();
    type Parent = C190;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m191_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m191_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a191_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a191_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a191_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a191_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a191_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a191_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a191_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a191_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a191_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a191_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a191_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a191_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a191_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a191_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a191_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C192;
impl JsClass for C192 {
    const NAME: &'static str = "C192";
    type Data = ();
    type Parent = C191;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m192_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m192_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a192_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a192_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a192_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a192_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a192_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a192_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a192_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a192_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a192_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a192_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a192_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a192_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a192_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a192_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a192_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C193;
impl JsClass for C193 {
    const NAME: &'static str = "C193";
    type Data = ();
    type Parent = C192;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m193_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m193_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a193_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a193_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a193_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a193_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a193_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a193_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a193_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a193_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a193_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a193_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a193_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a193_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a193_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a193_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a193_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C194;
impl JsClass for C194 {
    const NAME: &'static str = "C194";
    type Data = ();
    type Parent = C193;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m194_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m194_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a194_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a194_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a194_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a194_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a194_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a194_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a194_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a194_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a194_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a194_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a194_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a194_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a194_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a194_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a194_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C195;
impl JsClass for C195 {
    const NAME: &'static str = "C195";
    type Data = ();
    type Parent = C194;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m195_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m195_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a195_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a195_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a195_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a195_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a195_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a195_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a195_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a195_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a195_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a195_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a195_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a195_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a195_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a195_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a195_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C196;
impl JsClass for C196 {
    const NAME: &'static str = "C196";
    type Data = ();
    type Parent = C195;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m196_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m196_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a196_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a196_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a196_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a196_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a196_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a196_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a196_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a196_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a196_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a196_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a196_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a196_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a196_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a196_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a196_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C197;
impl JsClass for C197 {
    const NAME: &'static str = "C197";
    type Data = ();
    type Parent = C196;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m197_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m197_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a197_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a197_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a197_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a197_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a197_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a197_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a197_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a197_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a197_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a197_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a197_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a197_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a197_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a197_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a197_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C198;
impl JsClass for C198 {
    const NAME: &'static str = "C198";
    type Data = ();
    type Parent = C197;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m198_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m198_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a198_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a198_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a198_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a198_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a198_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a198_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a198_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a198_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a198_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a198_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a198_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a198_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a198_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a198_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a198_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C199;
impl JsClass for C199 {
    const NAME: &'static str = "C199";
    type Data = ();
    type Parent = C198;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m199_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m199_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a199_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a199_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a199_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a199_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a199_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a199_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a199_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a199_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a199_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a199_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a199_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a199_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a199_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a199_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a199_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C200;
impl JsClass for C200 {
    const NAME: &'static str = "C200";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m200_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m200_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a200_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a200_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a200_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a200_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a200_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a200_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a200_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a200_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a200_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a200_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a200_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a200_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a200_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a200_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a200_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C201;
impl JsClass for C201 {
    const NAME: &'static str = "C201";
    type Data = ();
    type Parent = C200;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m201_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m201_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a201_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a201_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a201_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a201_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a201_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a201_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a201_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a201_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a201_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a201_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a201_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a201_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a201_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a201_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a201_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C202;
impl JsClass for C202 {
    const NAME: &'static str = "C202";
    type Data = ();
    type Parent = C201;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m202_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m202_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a202_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a202_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a202_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a202_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a202_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a202_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a202_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a202_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a202_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a202_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a202_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a202_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a202_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a202_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a202_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C203;
impl JsClass for C203 {
    const NAME: &'static str = "C203";
    type Data = ();
    type Parent = C202;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m203_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m203_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a203_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a203_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a203_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a203_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a203_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a203_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a203_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a203_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a203_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a203_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a203_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a203_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a203_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a203_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a203_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C204;
impl JsClass for C204 {
    const NAME: &'static str = "C204";
    type Data = ();
    type Parent = C203;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m204_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m204_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a204_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a204_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a204_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a204_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a204_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a204_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a204_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a204_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a204_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a204_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a204_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a204_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a204_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a204_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a204_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C205;
impl JsClass for C205 {
    const NAME: &'static str = "C205";
    type Data = ();
    type Parent = C204;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m205_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m205_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a205_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a205_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a205_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a205_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a205_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a205_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a205_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a205_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a205_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a205_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a205_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a205_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a205_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a205_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a205_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C206;
impl JsClass for C206 {
    const NAME: &'static str = "C206";
    type Data = ();
    type Parent = C205;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m206_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m206_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a206_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a206_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a206_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a206_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a206_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a206_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a206_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a206_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a206_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a206_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a206_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a206_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a206_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a206_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a206_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C207;
impl JsClass for C207 {
    const NAME: &'static str = "C207";
    type Data = ();
    type Parent = C206;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m207_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m207_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a207_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a207_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a207_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a207_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a207_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a207_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a207_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a207_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a207_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a207_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a207_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a207_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a207_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a207_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a207_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C208;
impl JsClass for C208 {
    const NAME: &'static str = "C208";
    type Data = ();
    type Parent = C207;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m208_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m208_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a208_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a208_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a208_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a208_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a208_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a208_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a208_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a208_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a208_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a208_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a208_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a208_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a208_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a208_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a208_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C209;
impl JsClass for C209 {
    const NAME: &'static str = "C209";
    type Data = ();
    type Parent = C208;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m209_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m209_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a209_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a209_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a209_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a209_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a209_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a209_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a209_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a209_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a209_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a209_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a209_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a209_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a209_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a209_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a209_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C210;
impl JsClass for C210 {
    const NAME: &'static str = "C210";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m210_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m210_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a210_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a210_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a210_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a210_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a210_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a210_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a210_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a210_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a210_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a210_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a210_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a210_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a210_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a210_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a210_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C211;
impl JsClass for C211 {
    const NAME: &'static str = "C211";
    type Data = ();
    type Parent = C210;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m211_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m211_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a211_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a211_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a211_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a211_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a211_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a211_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a211_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a211_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a211_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a211_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a211_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a211_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a211_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a211_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a211_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C212;
impl JsClass for C212 {
    const NAME: &'static str = "C212";
    type Data = ();
    type Parent = C211;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m212_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m212_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a212_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a212_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a212_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a212_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a212_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a212_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a212_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a212_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a212_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a212_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a212_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a212_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a212_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a212_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a212_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C213;
impl JsClass for C213 {
    const NAME: &'static str = "C213";
    type Data = ();
    type Parent = C212;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m213_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m213_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a213_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a213_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a213_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a213_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a213_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a213_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a213_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a213_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a213_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a213_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a213_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a213_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a213_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a213_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a213_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C214;
impl JsClass for C214 {
    const NAME: &'static str = "C214";
    type Data = ();
    type Parent = C213;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m214_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m214_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a214_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a214_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a214_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a214_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a214_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a214_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a214_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a214_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a214_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a214_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a214_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a214_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a214_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a214_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a214_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C215;
impl JsClass for C215 {
    const NAME: &'static str = "C215";
    type Data = ();
    type Parent = C214;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m215_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m215_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a215_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a215_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a215_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a215_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a215_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a215_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a215_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a215_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a215_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a215_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a215_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a215_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a215_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a215_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a215_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C216;
impl JsClass for C216 {
    const NAME: &'static str = "C216";
    type Data = ();
    type Parent = C215;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m216_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m216_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a216_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a216_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a216_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a216_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a216_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a216_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a216_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a216_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a216_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a216_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a216_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a216_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a216_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a216_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a216_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C217;
impl JsClass for C217 {
    const NAME: &'static str = "C217";
    type Data = ();
    type Parent = C216;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m217_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m217_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a217_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a217_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a217_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a217_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a217_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a217_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a217_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a217_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a217_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a217_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a217_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a217_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a217_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a217_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a217_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C218;
impl JsClass for C218 {
    const NAME: &'static str = "C218";
    type Data = ();
    type Parent = C217;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m218_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m218_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a218_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a218_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a218_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a218_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a218_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a218_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a218_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a218_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a218_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a218_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a218_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a218_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a218_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a218_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a218_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C219;
impl JsClass for C219 {
    const NAME: &'static str = "C219";
    type Data = ();
    type Parent = C218;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m219_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m219_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a219_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a219_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a219_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a219_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a219_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a219_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a219_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a219_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a219_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a219_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a219_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a219_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a219_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a219_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a219_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C220;
impl JsClass for C220 {
    const NAME: &'static str = "C220";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m220_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m220_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a220_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a220_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a220_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a220_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a220_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a220_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a220_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a220_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a220_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a220_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a220_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a220_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a220_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a220_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a220_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C221;
impl JsClass for C221 {
    const NAME: &'static str = "C221";
    type Data = ();
    type Parent = C220;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m221_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m221_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a221_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a221_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a221_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a221_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a221_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a221_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a221_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a221_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a221_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a221_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a221_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a221_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a221_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a221_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a221_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C222;
impl JsClass for C222 {
    const NAME: &'static str = "C222";
    type Data = ();
    type Parent = C221;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m222_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m222_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a222_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a222_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a222_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a222_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a222_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a222_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a222_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a222_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a222_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a222_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a222_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a222_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a222_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a222_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a222_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C223;
impl JsClass for C223 {
    const NAME: &'static str = "C223";
    type Data = ();
    type Parent = C222;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m223_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m223_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a223_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a223_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a223_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a223_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a223_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a223_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a223_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a223_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a223_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a223_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a223_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a223_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a223_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a223_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a223_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C224;
impl JsClass for C224 {
    const NAME: &'static str = "C224";
    type Data = ();
    type Parent = C223;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m224_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m224_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a224_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a224_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a224_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a224_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a224_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a224_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a224_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a224_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a224_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a224_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a224_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a224_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a224_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a224_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a224_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C225;
impl JsClass for C225 {
    const NAME: &'static str = "C225";
    type Data = ();
    type Parent = C224;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m225_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m225_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a225_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a225_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a225_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a225_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a225_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a225_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a225_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a225_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a225_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a225_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a225_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a225_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a225_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a225_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a225_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C226;
impl JsClass for C226 {
    const NAME: &'static str = "C226";
    type Data = ();
    type Parent = C225;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m226_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m226_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a226_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a226_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a226_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a226_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a226_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a226_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a226_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a226_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a226_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a226_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a226_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a226_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a226_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a226_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a226_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C227;
impl JsClass for C227 {
    const NAME: &'static str = "C227";
    type Data = ();
    type Parent = C226;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m227_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m227_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a227_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a227_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a227_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a227_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a227_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a227_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a227_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a227_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a227_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a227_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a227_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a227_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a227_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a227_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a227_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C228;
impl JsClass for C228 {
    const NAME: &'static str = "C228";
    type Data = ();
    type Parent = C227;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m228_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m228_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a228_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a228_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a228_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a228_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a228_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a228_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a228_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a228_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a228_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a228_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a228_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a228_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a228_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a228_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a228_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C229;
impl JsClass for C229 {
    const NAME: &'static str = "C229";
    type Data = ();
    type Parent = C228;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m229_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m229_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a229_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a229_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a229_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a229_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a229_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a229_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a229_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a229_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a229_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a229_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a229_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a229_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a229_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a229_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a229_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C230;
impl JsClass for C230 {
    const NAME: &'static str = "C230";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m230_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m230_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a230_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a230_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a230_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a230_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a230_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a230_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a230_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a230_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a230_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a230_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a230_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a230_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a230_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a230_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a230_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C231;
impl JsClass for C231 {
    const NAME: &'static str = "C231";
    type Data = ();
    type Parent = C230;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m231_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m231_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a231_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a231_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a231_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a231_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a231_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a231_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a231_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a231_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a231_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a231_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a231_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a231_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a231_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a231_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a231_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C232;
impl JsClass for C232 {
    const NAME: &'static str = "C232";
    type Data = ();
    type Parent = C231;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m232_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m232_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a232_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a232_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a232_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a232_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a232_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a232_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a232_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a232_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a232_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a232_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a232_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a232_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a232_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a232_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a232_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C233;
impl JsClass for C233 {
    const NAME: &'static str = "C233";
    type Data = ();
    type Parent = C232;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m233_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m233_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a233_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a233_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a233_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a233_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a233_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a233_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a233_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a233_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a233_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a233_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a233_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a233_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a233_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a233_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a233_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C234;
impl JsClass for C234 {
    const NAME: &'static str = "C234";
    type Data = ();
    type Parent = C233;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m234_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m234_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a234_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a234_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a234_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a234_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a234_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a234_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a234_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a234_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a234_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a234_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a234_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a234_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a234_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a234_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a234_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C235;
impl JsClass for C235 {
    const NAME: &'static str = "C235";
    type Data = ();
    type Parent = C234;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m235_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m235_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a235_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a235_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a235_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a235_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a235_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a235_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a235_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a235_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a235_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a235_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a235_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a235_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a235_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a235_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a235_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C236;
impl JsClass for C236 {
    const NAME: &'static str = "C236";
    type Data = ();
    type Parent = C235;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m236_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m236_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a236_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a236_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a236_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a236_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a236_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a236_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a236_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a236_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a236_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a236_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a236_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a236_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a236_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a236_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a236_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C237;
impl JsClass for C237 {
    const NAME: &'static str = "C237";
    type Data = ();
    type Parent = C236;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m237_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m237_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a237_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a237_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a237_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a237_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a237_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a237_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a237_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a237_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a237_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a237_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a237_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a237_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a237_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a237_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a237_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C238;
impl JsClass for C238 {
    const NAME: &'static str = "C238";
    type Data = ();
    type Parent = C237;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m238_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m238_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a238_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a238_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a238_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a238_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a238_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a238_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a238_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a238_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a238_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a238_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a238_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a238_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a238_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a238_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a238_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C239;
impl JsClass for C239 {
    const NAME: &'static str = "C239";
    type Data = ();
    type Parent = C238;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m239_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m239_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a239_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a239_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a239_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a239_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a239_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a239_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a239_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a239_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a239_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a239_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a239_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a239_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a239_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a239_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a239_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C240;
impl JsClass for C240 {
    const NAME: &'static str = "C240";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m240_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m240_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a240_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a240_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a240_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a240_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a240_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a240_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a240_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a240_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a240_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a240_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a240_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a240_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a240_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a240_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a240_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C241;
impl JsClass for C241 {
    const NAME: &'static str = "C241";
    type Data = ();
    type Parent = C240;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m241_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m241_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a241_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a241_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a241_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a241_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a241_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a241_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a241_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a241_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a241_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a241_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a241_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a241_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a241_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a241_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a241_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C242;
impl JsClass for C242 {
    const NAME: &'static str = "C242";
    type Data = ();
    type Parent = C241;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m242_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m242_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a242_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a242_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a242_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a242_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a242_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a242_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a242_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a242_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a242_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a242_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a242_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a242_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a242_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a242_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a242_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C243;
impl JsClass for C243 {
    const NAME: &'static str = "C243";
    type Data = ();
    type Parent = C242;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m243_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m243_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a243_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a243_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a243_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a243_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a243_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a243_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a243_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a243_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a243_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a243_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a243_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a243_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a243_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a243_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a243_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C244;
impl JsClass for C244 {
    const NAME: &'static str = "C244";
    type Data = ();
    type Parent = C243;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m244_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m244_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a244_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a244_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a244_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a244_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a244_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a244_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a244_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a244_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a244_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a244_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a244_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a244_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a244_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a244_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a244_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C245;
impl JsClass for C245 {
    const NAME: &'static str = "C245";
    type Data = ();
    type Parent = C244;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m245_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m245_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a245_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a245_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a245_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a245_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a245_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a245_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a245_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a245_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a245_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a245_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a245_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a245_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a245_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a245_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a245_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C246;
impl JsClass for C246 {
    const NAME: &'static str = "C246";
    type Data = ();
    type Parent = C245;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m246_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m246_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a246_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a246_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a246_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a246_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a246_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a246_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a246_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a246_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a246_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a246_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a246_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a246_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a246_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a246_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a246_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C247;
impl JsClass for C247 {
    const NAME: &'static str = "C247";
    type Data = ();
    type Parent = C246;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m247_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m247_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a247_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a247_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a247_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a247_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a247_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a247_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a247_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a247_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a247_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a247_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a247_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a247_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a247_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a247_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a247_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C248;
impl JsClass for C248 {
    const NAME: &'static str = "C248";
    type Data = ();
    type Parent = C247;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m248_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m248_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a248_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a248_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a248_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a248_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a248_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a248_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a248_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a248_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a248_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a248_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a248_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a248_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a248_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a248_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a248_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C249;
impl JsClass for C249 {
    const NAME: &'static str = "C249";
    type Data = ();
    type Parent = C248;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m249_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m249_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a249_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a249_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a249_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a249_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a249_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a249_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a249_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a249_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a249_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a249_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a249_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a249_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a249_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a249_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a249_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C250;
impl JsClass for C250 {
    const NAME: &'static str = "C250";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m250_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m250_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a250_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a250_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a250_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a250_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a250_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a250_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a250_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a250_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a250_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a250_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a250_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a250_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a250_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a250_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a250_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C251;
impl JsClass for C251 {
    const NAME: &'static str = "C251";
    type Data = ();
    type Parent = C250;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m251_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m251_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a251_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a251_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a251_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a251_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a251_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a251_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a251_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a251_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a251_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a251_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a251_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a251_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a251_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a251_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a251_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C252;
impl JsClass for C252 {
    const NAME: &'static str = "C252";
    type Data = ();
    type Parent = C251;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m252_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m252_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a252_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a252_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a252_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a252_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a252_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a252_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a252_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a252_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a252_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a252_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a252_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a252_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a252_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a252_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a252_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C253;
impl JsClass for C253 {
    const NAME: &'static str = "C253";
    type Data = ();
    type Parent = C252;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m253_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m253_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a253_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a253_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a253_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a253_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a253_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a253_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a253_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a253_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a253_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a253_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a253_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a253_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a253_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a253_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a253_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C254;
impl JsClass for C254 {
    const NAME: &'static str = "C254";
    type Data = ();
    type Parent = C253;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m254_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m254_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a254_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a254_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a254_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a254_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a254_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a254_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a254_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a254_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a254_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a254_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a254_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a254_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a254_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a254_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a254_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C255;
impl JsClass for C255 {
    const NAME: &'static str = "C255";
    type Data = ();
    type Parent = C254;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m255_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m255_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a255_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a255_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a255_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a255_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a255_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a255_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a255_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a255_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a255_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a255_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a255_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a255_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a255_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a255_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a255_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C256;
impl JsClass for C256 {
    const NAME: &'static str = "C256";
    type Data = ();
    type Parent = C255;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m256_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m256_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a256_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a256_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a256_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a256_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a256_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a256_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a256_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a256_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a256_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a256_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a256_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a256_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a256_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a256_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a256_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C257;
impl JsClass for C257 {
    const NAME: &'static str = "C257";
    type Data = ();
    type Parent = C256;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m257_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m257_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a257_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a257_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a257_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a257_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a257_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a257_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a257_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a257_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a257_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a257_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a257_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a257_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a257_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a257_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a257_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C258;
impl JsClass for C258 {
    const NAME: &'static str = "C258";
    type Data = ();
    type Parent = C257;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m258_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m258_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a258_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a258_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a258_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a258_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a258_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a258_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a258_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a258_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a258_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a258_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a258_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a258_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a258_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a258_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a258_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C259;
impl JsClass for C259 {
    const NAME: &'static str = "C259";
    type Data = ();
    type Parent = C258;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m259_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m259_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a259_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a259_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a259_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a259_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a259_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a259_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a259_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a259_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a259_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a259_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a259_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a259_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a259_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a259_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a259_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C260;
impl JsClass for C260 {
    const NAME: &'static str = "C260";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m260_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m260_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a260_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a260_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a260_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a260_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a260_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a260_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a260_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a260_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a260_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a260_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a260_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a260_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a260_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a260_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a260_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C261;
impl JsClass for C261 {
    const NAME: &'static str = "C261";
    type Data = ();
    type Parent = C260;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m261_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m261_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a261_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a261_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a261_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a261_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a261_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a261_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a261_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a261_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a261_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a261_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a261_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a261_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a261_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a261_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a261_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C262;
impl JsClass for C262 {
    const NAME: &'static str = "C262";
    type Data = ();
    type Parent = C261;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m262_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m262_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a262_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a262_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a262_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a262_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a262_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a262_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a262_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a262_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a262_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a262_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a262_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a262_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a262_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a262_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a262_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C263;
impl JsClass for C263 {
    const NAME: &'static str = "C263";
    type Data = ();
    type Parent = C262;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m263_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m263_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a263_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a263_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a263_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a263_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a263_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a263_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a263_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a263_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a263_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a263_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a263_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a263_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a263_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a263_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a263_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C264;
impl JsClass for C264 {
    const NAME: &'static str = "C264";
    type Data = ();
    type Parent = C263;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m264_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m264_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a264_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a264_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a264_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a264_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a264_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a264_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a264_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a264_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a264_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a264_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a264_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a264_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a264_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a264_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a264_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C265;
impl JsClass for C265 {
    const NAME: &'static str = "C265";
    type Data = ();
    type Parent = C264;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m265_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m265_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a265_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a265_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a265_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a265_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a265_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a265_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a265_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a265_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a265_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a265_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a265_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a265_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a265_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a265_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a265_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C266;
impl JsClass for C266 {
    const NAME: &'static str = "C266";
    type Data = ();
    type Parent = C265;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m266_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m266_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a266_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a266_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a266_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a266_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a266_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a266_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a266_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a266_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a266_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a266_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a266_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a266_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a266_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a266_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a266_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C267;
impl JsClass for C267 {
    const NAME: &'static str = "C267";
    type Data = ();
    type Parent = C266;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m267_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m267_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a267_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a267_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a267_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a267_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a267_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a267_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a267_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a267_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a267_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a267_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a267_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a267_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a267_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a267_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a267_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C268;
impl JsClass for C268 {
    const NAME: &'static str = "C268";
    type Data = ();
    type Parent = C267;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m268_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m268_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a268_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a268_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a268_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a268_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a268_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a268_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a268_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a268_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a268_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a268_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a268_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a268_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a268_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a268_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a268_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C269;
impl JsClass for C269 {
    const NAME: &'static str = "C269";
    type Data = ();
    type Parent = C268;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m269_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m269_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a269_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a269_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a269_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a269_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a269_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a269_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a269_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a269_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a269_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a269_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a269_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a269_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a269_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a269_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a269_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C270;
impl JsClass for C270 {
    const NAME: &'static str = "C270";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m270_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m270_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a270_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a270_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a270_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a270_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a270_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a270_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a270_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a270_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a270_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a270_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a270_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a270_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a270_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a270_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a270_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C271;
impl JsClass for C271 {
    const NAME: &'static str = "C271";
    type Data = ();
    type Parent = C270;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m271_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m271_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a271_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a271_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a271_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a271_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a271_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a271_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a271_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a271_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a271_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a271_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a271_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a271_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a271_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a271_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a271_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C272;
impl JsClass for C272 {
    const NAME: &'static str = "C272";
    type Data = ();
    type Parent = C271;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m272_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m272_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a272_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a272_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a272_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a272_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a272_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a272_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a272_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a272_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a272_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a272_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a272_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a272_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a272_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a272_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a272_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C273;
impl JsClass for C273 {
    const NAME: &'static str = "C273";
    type Data = ();
    type Parent = C272;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m273_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m273_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a273_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a273_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a273_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a273_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a273_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a273_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a273_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a273_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a273_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a273_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a273_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a273_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a273_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a273_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a273_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C274;
impl JsClass for C274 {
    const NAME: &'static str = "C274";
    type Data = ();
    type Parent = C273;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m274_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m274_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a274_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a274_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a274_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a274_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a274_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a274_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a274_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a274_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a274_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a274_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a274_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a274_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a274_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a274_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a274_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C275;
impl JsClass for C275 {
    const NAME: &'static str = "C275";
    type Data = ();
    type Parent = C274;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m275_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m275_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a275_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a275_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a275_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a275_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a275_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a275_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a275_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a275_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a275_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a275_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a275_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a275_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a275_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a275_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a275_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C276;
impl JsClass for C276 {
    const NAME: &'static str = "C276";
    type Data = ();
    type Parent = C275;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m276_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m276_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a276_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a276_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a276_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a276_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a276_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a276_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a276_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a276_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a276_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a276_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a276_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a276_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a276_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a276_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a276_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C277;
impl JsClass for C277 {
    const NAME: &'static str = "C277";
    type Data = ();
    type Parent = C276;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m277_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m277_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a277_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a277_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a277_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a277_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a277_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a277_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a277_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a277_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a277_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a277_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a277_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a277_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a277_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a277_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a277_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C278;
impl JsClass for C278 {
    const NAME: &'static str = "C278";
    type Data = ();
    type Parent = C277;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m278_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m278_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a278_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a278_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a278_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a278_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a278_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a278_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a278_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a278_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a278_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a278_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a278_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a278_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a278_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a278_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a278_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C279;
impl JsClass for C279 {
    const NAME: &'static str = "C279";
    type Data = ();
    type Parent = C278;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m279_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m279_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a279_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a279_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a279_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a279_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a279_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a279_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a279_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a279_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a279_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a279_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a279_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a279_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a279_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a279_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a279_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C280;
impl JsClass for C280 {
    const NAME: &'static str = "C280";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m280_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m280_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a280_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a280_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a280_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a280_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a280_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a280_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a280_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a280_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a280_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a280_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a280_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a280_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a280_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a280_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a280_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C281;
impl JsClass for C281 {
    const NAME: &'static str = "C281";
    type Data = ();
    type Parent = C280;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m281_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m281_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a281_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a281_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a281_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a281_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a281_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a281_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a281_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a281_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a281_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a281_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a281_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a281_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a281_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a281_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a281_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C282;
impl JsClass for C282 {
    const NAME: &'static str = "C282";
    type Data = ();
    type Parent = C281;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m282_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m282_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a282_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a282_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a282_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a282_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a282_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a282_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a282_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a282_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a282_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a282_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a282_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a282_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a282_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a282_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a282_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C283;
impl JsClass for C283 {
    const NAME: &'static str = "C283";
    type Data = ();
    type Parent = C282;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m283_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m283_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a283_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a283_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a283_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a283_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a283_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a283_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a283_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a283_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a283_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a283_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a283_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a283_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a283_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a283_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a283_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C284;
impl JsClass for C284 {
    const NAME: &'static str = "C284";
    type Data = ();
    type Parent = C283;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m284_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m284_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a284_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a284_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a284_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a284_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a284_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a284_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a284_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a284_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a284_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a284_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a284_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a284_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a284_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a284_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a284_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C285;
impl JsClass for C285 {
    const NAME: &'static str = "C285";
    type Data = ();
    type Parent = C284;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m285_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m285_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a285_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a285_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a285_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a285_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a285_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a285_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a285_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a285_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a285_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a285_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a285_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a285_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a285_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a285_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a285_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C286;
impl JsClass for C286 {
    const NAME: &'static str = "C286";
    type Data = ();
    type Parent = C285;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m286_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m286_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a286_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a286_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a286_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a286_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a286_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a286_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a286_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a286_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a286_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a286_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a286_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a286_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a286_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a286_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a286_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C287;
impl JsClass for C287 {
    const NAME: &'static str = "C287";
    type Data = ();
    type Parent = C286;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m287_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m287_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a287_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a287_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a287_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a287_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a287_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a287_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a287_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a287_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a287_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a287_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a287_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a287_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a287_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a287_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a287_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C288;
impl JsClass for C288 {
    const NAME: &'static str = "C288";
    type Data = ();
    type Parent = C287;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m288_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m288_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a288_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a288_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a288_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a288_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a288_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a288_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a288_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a288_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a288_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a288_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a288_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a288_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a288_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a288_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a288_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C289;
impl JsClass for C289 {
    const NAME: &'static str = "C289";
    type Data = ();
    type Parent = C288;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m289_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m289_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a289_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a289_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a289_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a289_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a289_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a289_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a289_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a289_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a289_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a289_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a289_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a289_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a289_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a289_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a289_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C290;
impl JsClass for C290 {
    const NAME: &'static str = "C290";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m290_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m290_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a290_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a290_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a290_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a290_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a290_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a290_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a290_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a290_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a290_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a290_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a290_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a290_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a290_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a290_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a290_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C291;
impl JsClass for C291 {
    const NAME: &'static str = "C291";
    type Data = ();
    type Parent = C290;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m291_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m291_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a291_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a291_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a291_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a291_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a291_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a291_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a291_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a291_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a291_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a291_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a291_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a291_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a291_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a291_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a291_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C292;
impl JsClass for C292 {
    const NAME: &'static str = "C292";
    type Data = ();
    type Parent = C291;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m292_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m292_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a292_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a292_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a292_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a292_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a292_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a292_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a292_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a292_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a292_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a292_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a292_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a292_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a292_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a292_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a292_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C293;
impl JsClass for C293 {
    const NAME: &'static str = "C293";
    type Data = ();
    type Parent = C292;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m293_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m293_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a293_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a293_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a293_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a293_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a293_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a293_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a293_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a293_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a293_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a293_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a293_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a293_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a293_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a293_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a293_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C294;
impl JsClass for C294 {
    const NAME: &'static str = "C294";
    type Data = ();
    type Parent = C293;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m294_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m294_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a294_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a294_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a294_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a294_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a294_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a294_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a294_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a294_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a294_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a294_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a294_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a294_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a294_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a294_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a294_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C295;
impl JsClass for C295 {
    const NAME: &'static str = "C295";
    type Data = ();
    type Parent = C294;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m295_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m295_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a295_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a295_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a295_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a295_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a295_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a295_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a295_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a295_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a295_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a295_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a295_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a295_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a295_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a295_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a295_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C296;
impl JsClass for C296 {
    const NAME: &'static str = "C296";
    type Data = ();
    type Parent = C295;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m296_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m296_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a296_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a296_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a296_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a296_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a296_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a296_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a296_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a296_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a296_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a296_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a296_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a296_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a296_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a296_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a296_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C297;
impl JsClass for C297 {
    const NAME: &'static str = "C297";
    type Data = ();
    type Parent = C296;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m297_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m297_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a297_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a297_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a297_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a297_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a297_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a297_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a297_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a297_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a297_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a297_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a297_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a297_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a297_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a297_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a297_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C298;
impl JsClass for C298 {
    const NAME: &'static str = "C298";
    type Data = ();
    type Parent = C297;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m298_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m298_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a298_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a298_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a298_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a298_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a298_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a298_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a298_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a298_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a298_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a298_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a298_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a298_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a298_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a298_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a298_14", |s, _d| Ok(s.number(14.0)));
    }
}
struct C299;
impl JsClass for C299 {
    const NAME: &'static str = "C299";
    type Data = ();
    type Parent = C298;
    fn define(def: &mut ClassDef<Self>) {
        def.method("m299_0", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_1", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_2", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_3", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_4", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_5", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_6", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_7", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_8", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_9", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_10", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_11", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_12", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_13", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("m299_14", 0, |s, _d, _a| Ok(s.undefined()));
        def.getter("a299_0", |s, _d| Ok(s.number(0.0)));
        def.getter("a299_1", |s, _d| Ok(s.number(1.0)));
        def.getter("a299_2", |s, _d| Ok(s.number(2.0)));
        def.getter("a299_3", |s, _d| Ok(s.number(3.0)));
        def.getter("a299_4", |s, _d| Ok(s.number(4.0)));
        def.getter("a299_5", |s, _d| Ok(s.number(5.0)));
        def.getter("a299_6", |s, _d| Ok(s.number(6.0)));
        def.getter("a299_7", |s, _d| Ok(s.number(7.0)));
        def.getter("a299_8", |s, _d| Ok(s.number(8.0)));
        def.getter("a299_9", |s, _d| Ok(s.number(9.0)));
        def.getter("a299_10", |s, _d| Ok(s.number(10.0)));
        def.getter("a299_11", |s, _d| Ok(s.number(11.0)));
        def.getter("a299_12", |s, _d| Ok(s.number(12.0)));
        def.getter("a299_13", |s, _d| Ok(s.number(13.0)));
        def.getter("a299_14", |s, _d| Ok(s.number(14.0)));
    }
}
fn main() {
    let e = Engine::new().unwrap();
    let t = Instant::now();
    e.register::<C0>();
    e.register::<C1>();
    e.register::<C2>();
    e.register::<C3>();
    e.register::<C4>();
    e.register::<C5>();
    e.register::<C6>();
    e.register::<C7>();
    e.register::<C8>();
    e.register::<C9>();
    e.register::<C10>();
    e.register::<C11>();
    e.register::<C12>();
    e.register::<C13>();
    e.register::<C14>();
    e.register::<C15>();
    e.register::<C16>();
    e.register::<C17>();
    e.register::<C18>();
    e.register::<C19>();
    e.register::<C20>();
    e.register::<C21>();
    e.register::<C22>();
    e.register::<C23>();
    e.register::<C24>();
    e.register::<C25>();
    e.register::<C26>();
    e.register::<C27>();
    e.register::<C28>();
    e.register::<C29>();
    e.register::<C30>();
    e.register::<C31>();
    e.register::<C32>();
    e.register::<C33>();
    e.register::<C34>();
    e.register::<C35>();
    e.register::<C36>();
    e.register::<C37>();
    e.register::<C38>();
    e.register::<C39>();
    e.register::<C40>();
    e.register::<C41>();
    e.register::<C42>();
    e.register::<C43>();
    e.register::<C44>();
    e.register::<C45>();
    e.register::<C46>();
    e.register::<C47>();
    e.register::<C48>();
    e.register::<C49>();
    e.register::<C50>();
    e.register::<C51>();
    e.register::<C52>();
    e.register::<C53>();
    e.register::<C54>();
    e.register::<C55>();
    e.register::<C56>();
    e.register::<C57>();
    e.register::<C58>();
    e.register::<C59>();
    e.register::<C60>();
    e.register::<C61>();
    e.register::<C62>();
    e.register::<C63>();
    e.register::<C64>();
    e.register::<C65>();
    e.register::<C66>();
    e.register::<C67>();
    e.register::<C68>();
    e.register::<C69>();
    e.register::<C70>();
    e.register::<C71>();
    e.register::<C72>();
    e.register::<C73>();
    e.register::<C74>();
    e.register::<C75>();
    e.register::<C76>();
    e.register::<C77>();
    e.register::<C78>();
    e.register::<C79>();
    e.register::<C80>();
    e.register::<C81>();
    e.register::<C82>();
    e.register::<C83>();
    e.register::<C84>();
    e.register::<C85>();
    e.register::<C86>();
    e.register::<C87>();
    e.register::<C88>();
    e.register::<C89>();
    e.register::<C90>();
    e.register::<C91>();
    e.register::<C92>();
    e.register::<C93>();
    e.register::<C94>();
    e.register::<C95>();
    e.register::<C96>();
    e.register::<C97>();
    e.register::<C98>();
    e.register::<C99>();
    e.register::<C100>();
    e.register::<C101>();
    e.register::<C102>();
    e.register::<C103>();
    e.register::<C104>();
    e.register::<C105>();
    e.register::<C106>();
    e.register::<C107>();
    e.register::<C108>();
    e.register::<C109>();
    e.register::<C110>();
    e.register::<C111>();
    e.register::<C112>();
    e.register::<C113>();
    e.register::<C114>();
    e.register::<C115>();
    e.register::<C116>();
    e.register::<C117>();
    e.register::<C118>();
    e.register::<C119>();
    e.register::<C120>();
    e.register::<C121>();
    e.register::<C122>();
    e.register::<C123>();
    e.register::<C124>();
    e.register::<C125>();
    e.register::<C126>();
    e.register::<C127>();
    e.register::<C128>();
    e.register::<C129>();
    e.register::<C130>();
    e.register::<C131>();
    e.register::<C132>();
    e.register::<C133>();
    e.register::<C134>();
    e.register::<C135>();
    e.register::<C136>();
    e.register::<C137>();
    e.register::<C138>();
    e.register::<C139>();
    e.register::<C140>();
    e.register::<C141>();
    e.register::<C142>();
    e.register::<C143>();
    e.register::<C144>();
    e.register::<C145>();
    e.register::<C146>();
    e.register::<C147>();
    e.register::<C148>();
    e.register::<C149>();
    e.register::<C150>();
    e.register::<C151>();
    e.register::<C152>();
    e.register::<C153>();
    e.register::<C154>();
    e.register::<C155>();
    e.register::<C156>();
    e.register::<C157>();
    e.register::<C158>();
    e.register::<C159>();
    e.register::<C160>();
    e.register::<C161>();
    e.register::<C162>();
    e.register::<C163>();
    e.register::<C164>();
    e.register::<C165>();
    e.register::<C166>();
    e.register::<C167>();
    e.register::<C168>();
    e.register::<C169>();
    e.register::<C170>();
    e.register::<C171>();
    e.register::<C172>();
    e.register::<C173>();
    e.register::<C174>();
    e.register::<C175>();
    e.register::<C176>();
    e.register::<C177>();
    e.register::<C178>();
    e.register::<C179>();
    e.register::<C180>();
    e.register::<C181>();
    e.register::<C182>();
    e.register::<C183>();
    e.register::<C184>();
    e.register::<C185>();
    e.register::<C186>();
    e.register::<C187>();
    e.register::<C188>();
    e.register::<C189>();
    e.register::<C190>();
    e.register::<C191>();
    e.register::<C192>();
    e.register::<C193>();
    e.register::<C194>();
    e.register::<C195>();
    e.register::<C196>();
    e.register::<C197>();
    e.register::<C198>();
    e.register::<C199>();
    e.register::<C200>();
    e.register::<C201>();
    e.register::<C202>();
    e.register::<C203>();
    e.register::<C204>();
    e.register::<C205>();
    e.register::<C206>();
    e.register::<C207>();
    e.register::<C208>();
    e.register::<C209>();
    e.register::<C210>();
    e.register::<C211>();
    e.register::<C212>();
    e.register::<C213>();
    e.register::<C214>();
    e.register::<C215>();
    e.register::<C216>();
    e.register::<C217>();
    e.register::<C218>();
    e.register::<C219>();
    e.register::<C220>();
    e.register::<C221>();
    e.register::<C222>();
    e.register::<C223>();
    e.register::<C224>();
    e.register::<C225>();
    e.register::<C226>();
    e.register::<C227>();
    e.register::<C228>();
    e.register::<C229>();
    e.register::<C230>();
    e.register::<C231>();
    e.register::<C232>();
    e.register::<C233>();
    e.register::<C234>();
    e.register::<C235>();
    e.register::<C236>();
    e.register::<C237>();
    e.register::<C238>();
    e.register::<C239>();
    e.register::<C240>();
    e.register::<C241>();
    e.register::<C242>();
    e.register::<C243>();
    e.register::<C244>();
    e.register::<C245>();
    e.register::<C246>();
    e.register::<C247>();
    e.register::<C248>();
    e.register::<C249>();
    e.register::<C250>();
    e.register::<C251>();
    e.register::<C252>();
    e.register::<C253>();
    e.register::<C254>();
    e.register::<C255>();
    e.register::<C256>();
    e.register::<C257>();
    e.register::<C258>();
    e.register::<C259>();
    e.register::<C260>();
    e.register::<C261>();
    e.register::<C262>();
    e.register::<C263>();
    e.register::<C264>();
    e.register::<C265>();
    e.register::<C266>();
    e.register::<C267>();
    e.register::<C268>();
    e.register::<C269>();
    e.register::<C270>();
    e.register::<C271>();
    e.register::<C272>();
    e.register::<C273>();
    e.register::<C274>();
    e.register::<C275>();
    e.register::<C276>();
    e.register::<C277>();
    e.register::<C278>();
    e.register::<C279>();
    e.register::<C280>();
    e.register::<C281>();
    e.register::<C282>();
    e.register::<C283>();
    e.register::<C284>();
    e.register::<C285>();
    e.register::<C286>();
    e.register::<C287>();
    e.register::<C288>();
    e.register::<C289>();
    e.register::<C290>();
    e.register::<C291>();
    e.register::<C292>();
    e.register::<C293>();
    e.register::<C294>();
    e.register::<C295>();
    e.register::<C296>();
    e.register::<C297>();
    e.register::<C298>();
    e.register::<C299>();
    println!("register {} classes: {:?}", 300, t.elapsed());
    let mut realms = Vec::new();
    let t = Instant::now();
    for _ in 0..20 {
        realms.push(e.create_realm(RealmOptions::default()).unwrap());
    }
    println!("create_realm avg over 20: {:?}", t.elapsed() / 20);
    let t = Instant::now();
    for r in &realms {
        r.dispose();
    }
    e.collect_garbage_now();
    println!("dispose 20 + gc: {:?}", t.elapsed());
    let empty = Engine::new();
    drop(empty);
}
