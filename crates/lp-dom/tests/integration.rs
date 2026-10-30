//! Integration tests, linked into one binary: each test executable links the
//! whole engine (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod flow;
mod flow_bench;
mod flow_snapshot;
mod id_index;
mod inner_text;
mod layout;
mod loader;
mod record_scan_bench;
mod select_forms;
mod stylo_cascade;
mod tree;
mod typed_flow;
mod xml;
mod xpath;

/// `autotests = false` means a file in tests/ that is neither a module here nor
/// its own `[[test]]` target would silently never run.
#[test]
fn every_test_file_is_built() {
    let root = include_str!("integration.rs");
    let manifest = include_str!("../Cargo.toml");
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let Some(module) = name.strip_suffix(".rs") else { continue };
        assert!(
            name == "integration.rs"
                || root.contains(&format!("mod {module};"))
                || manifest.contains(&format!("path = \"tests/{name}\"")),
            "tests/{name} is not built: add `mod {module};` to tests/integration.rs"
        );
    }
}
