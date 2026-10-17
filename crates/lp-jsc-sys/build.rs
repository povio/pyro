// Links the platform JavaScriptCore. macOS: the system framework (public C API
// plus the ObjC module-loader shim in `shim/modules.m`). Other platforms are not
// supported yet: WebKitGTK's libjavascriptcoregtk exposes the same C API but no
// module loader, see docs/ARCHITECTURE.md "Platform support".
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    println!("cargo:rerun-if-changed=shim/modules.m");
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=JavaScriptCore");
        println!("cargo:rustc-link-lib=framework=Foundation");
        cc::Build::new().file("shim/modules.m").flag("-fobjc-arc").flag("-Wall").flag("-Werror").compile("lp_jsc_shim");
    } else {
        panic!("lp-jsc-sys: only macOS (system JavaScriptCore.framework) is supported so far");
    }
}
