//! Warns when a macOS build will not carry the JIT entitlement.
//!
//! `.cargo/config.toml` passes `-C link-arg=-Bscripts/macos/linker` so that
//! linking signs executables, but Cargo reads that file from the current
//! directory upwards, so building from outside the repository (e.g.
//! `cargo build --manifest-path ...`) links without it, and RUSTFLAGS in the
//! environment replaces it. Either way the system JavaScriptCore runs
//! interpreter-only.

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    if !flags.split('\x1f').any(|flag| flag.ends_with("link-arg=-Bscripts/macos/linker")) {
        println!(
            "cargo:warning=macOS build without `-C link-arg=-Bscripts/macos/linker` (rustflags: {:?}): the executable \
             will lack com.apple.security.cs.allow-jit and JavaScriptCore will run interpreter-only. Run cargo from \
             the repository so .cargo/config.toml applies, add the flag to RUSTFLAGS if you set it, or re-sign: \
             codesign -f -s - --entitlements scripts/macos/jit.entitlements <binary>",
            flags.replace('\x1f', " ")
        );
    }
}
