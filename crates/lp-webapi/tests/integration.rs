//! Integration tests, linked into one binary: each test executable links the
//! whole engine (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod activation_js;
mod attr_nodes_js;
mod canvas_js;
mod collections_js;
mod crypto_js;
mod custom_elements_js;
mod document_root_element_js;
mod dom_js;
mod editing_js;
mod error_events_js;
mod event_source_js;
mod events_misc_js;
mod fetch_js;
mod focus_shadow_js;
mod form_data_js;
mod forms_js;
mod identity_js;
mod image_input_js;
mod indexeddb_js;
mod layout_text_js;
mod media_source_js;
mod messaging_js;
mod move_before_js;
mod observable_js;
mod popover_dialog_js;
mod storage_js;
mod support;
mod traversal_js;
mod uievents_js;
mod url_params_js;
mod webidl_js;
mod websocket_js;
mod xhr_js;
mod xhr_worker_js;

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
