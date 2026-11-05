//! Generates the custom accessor trampolines (`src/accessors.rs`): JavaScriptCore
//! calls a custom accessor through a bare function pointer, with no data of its
//! own, so each (interface, attribute) pair needs its own entry point. The
//! trampolines are tiny: each forwards its slot number to one dispatcher.
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Custom accessor slots. lp-webapi defines about 650 attributes; a registry
/// that needs more installs the rest as ordinary accessors.
const SLOTS: usize = 1024;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut out = String::new();
    writeln!(out, "pub(crate) const SLOTS: usize = {SLOTS};").unwrap();
    for i in 0..SLOTS {
        writeln!(
            out,
            "unsafe extern \"C\" fn g{i}(ctx: sys::JSContextRef, this: sys::JSValueRef, uid: *const c_void) -> sys::JSValueRef {{ unsafe {{ custom_getter_dispatch({i}, ctx, this, uid) }} }}"
        )
        .unwrap();
        writeln!(
            out,
            "unsafe extern \"C\" fn s{i}(ctx: sys::JSContextRef, this: sys::JSValueRef, value: sys::JSValueRef, uid: *const c_void) -> bool {{ unsafe {{ custom_setter_dispatch({i}, ctx, this, value, uid) }} }}"
        )
        .unwrap();
    }
    let getters: Vec<String> = (0..SLOTS).map(|i| format!("g{i}")).collect();
    let setters: Vec<String> = (0..SLOTS).map(|i| format!("s{i}")).collect();
    writeln!(out, "pub(crate) static GETTERS: [sys::spi::JSCustomGetterCallback; SLOTS] = [{}];", getters.join(", "))
        .unwrap();
    writeln!(out, "pub(crate) static SETTERS: [sys::spi::JSCustomSetterCallback; SLOTS] = [{}];", setters.join(", "))
        .unwrap();
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("accessor_trampolines.rs");
    fs::write(path, out).unwrap();
}
