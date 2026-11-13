//! The macOS build path (scripts/macos/linker/ld via .cargo/config.toml)
//! must sign executables with the JIT entitlement; without it the system
//! JavaScriptCore silently runs interpreter-only.
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn entitlements(path: &Path) -> String {
    let out = Command::new("codesign").args(["-d", "--entitlements", "-"]).arg(path).output().expect("run codesign");
    assert!(out.status.success(), "codesign failed for {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn cli_binary_has_only_the_jit_entitlement() {
    let text = entitlements(Path::new(env!("CARGO_BIN_EXE_pyro")));
    assert!(text.contains("com.apple.security.cs.allow-jit"), "{text}");
    // Nothing beyond allow-jit is granted.
    assert_eq!(text.matches("com.apple.security.").count(), 1, "{text}");
}

#[test]
fn test_binaries_are_signed_the_same_way() {
    let exe = std::env::current_exe().expect("test executable");
    assert!(entitlements(&exe).contains("com.apple.security.cs.allow-jit"));
}

/// With the JIT active, JSC's default signal-based traps never stop a JIT-compiled
/// loop; `pyro_js::configure_process` selects polling traps so the timeout works.
#[test]
fn timeout_terminates_a_jit_compiled_loop() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pyro"))
        .args(["eval", "--timeout-ms", "300", "-e", "for (;;) {}"])
        .env_remove("JSC_usePollingTraps")
        .spawn()
        .expect("spawn pyro");
    // Hang guard only: termination is expected within about one second.
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("eval --timeout-ms 300 did not terminate the loop");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(124));
}

/// `pyro_js::configure_process` lowers JSC's heap growth, and a value the user set wins.
#[test]
fn heap_growth_default_yields_to_the_environment() {
    let growth = |value: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pyro"));
        cmd.args(["eval", "-e", "1"]).env("JSC_dumpOptions", "2").env_remove("JSC_heapGrowthMaxIncrease");
        if let Some(value) = value {
            cmd.env("JSC_heapGrowthMaxIncrease", value);
        }
        let out = cmd.output().expect("run pyro");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let option = text.split_whitespace().find(|word| word.starts_with("heapGrowthMaxIncrease="));
        option.expect("option in the dump").to_owned()
    };
    assert_eq!(growth(None), "heapGrowthMaxIncrease=1");
    assert_eq!(growth(Some("3")), "heapGrowthMaxIncrease=3");
}

/// `pyro_js::configure_process` lets the heap grow 4 MB between collections
/// (the C API's default is 1 MB), and a value the user set wins.
#[test]
fn young_generation_default_yields_to_the_environment() {
    let size = |value: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pyro"));
        cmd.args(["eval", "-e", "1"]).env("JSC_dumpOptions", "2").env_remove("JSC_smallHeapSize");
        if let Some(value) = value {
            cmd.env("JSC_smallHeapSize", value);
        }
        let out = cmd.output().expect("run pyro");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let option = text.split_whitespace().find(|word| word.starts_with("smallHeapSize="));
        option.expect("option in the dump").to_owned()
    };
    assert_eq!(size(None), "smallHeapSize=4194304");
    assert_eq!(size(Some("1048576")), "smallHeapSize=1048576");
}
