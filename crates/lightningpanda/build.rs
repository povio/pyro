//! Warns when a macOS build will not carry the JIT entitlement.
//!
//! `.cargo/config.toml` sets `scripts/macos/link-and-sign.sh` as the linker,
//! but Cargo reads that file from the current directory upwards, so building
//! from outside the repository (e.g. `cargo build --manifest-path ...`) links
//! without it and the system JavaScriptCore runs interpreter-only.

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC_LINKER");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let linker = std::env::var("RUSTC_LINKER").unwrap_or_default();
    if !linker.ends_with("scripts/macos/link-and-sign.sh") {
        println!(
            "cargo:warning=macOS build without scripts/macos/link-and-sign.sh (linker: {}): the executable will lack \
             com.apple.security.cs.allow-jit and JavaScriptCore will run interpreter-only. Run cargo from the \
             repository so .cargo/config.toml applies, or re-sign: codesign -f -s - --entitlements \
             scripts/macos/jit.entitlements <binary>",
            if linker.is_empty() { "default" } else { &linker }
        );
    }
}
