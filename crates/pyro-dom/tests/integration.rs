//! Integration tests, linked into one binary: each test executable links the
//! whole engine (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod animation_events;
mod adoption;
mod attr_nodes;
mod css_display;
mod css_math;
mod custom_reactions;
mod detached_changes;
mod dispatch_targets;
mod forms_values;
mod fragment_context;
mod html_selectedcontent_parsing;
mod id_index;
mod labels;
mod listener_counts;
mod labels_review;
mod loader;
mod move_before;
mod post_connection;
mod render_html;
mod render_delta_review;
mod select_fast;
mod select_forms;
mod select_query;
mod style_text_revision;
mod svg_script_parsing;
mod stylo_select;
mod text_content;
mod text_content_review;
mod top_layer;
mod tree;
mod tree_scope;
mod walk_summary;
mod html_processing_instructions;
mod xml;
mod xml_navigation;
mod xml_template;
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
