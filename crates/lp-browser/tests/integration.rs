//! Integration tests, linked into one binary: each test executable links the
//! whole engine (and is re-signed on macOS), so one per file made `cargo test`
//! mostly link time. Add new test files as modules here.

mod bindings;
mod cache_disabled;
mod child_frame_network;
mod child_frames;
mod constructable_stylesheets;
mod css_declarations;
mod cssom_and_attributes;
mod custom_elements_parser;
mod detached_frame_script;
mod document_write;
mod error_events;
mod event_realms;
mod extra_headers;
mod fetch;
mod file_urls;
mod frame_focus;
mod has_state;
mod history_rewrite;
mod images;
mod link_fragment;
mod loading;
mod messaging;
mod media;
mod media_decoding;
mod media_http;
mod microtask_species;
mod named_access;
mod navigation_api;
mod navigation_lifecycle;
mod navigation_supersession;
mod navigation_tasks;
mod network_delivery;
mod network_idle;
mod node_realms;
mod performance_timeline;
mod popups;
mod preload_scanner;
mod referrer;
mod remote_ordering;
mod resource_timing;
mod runloop_idle_finalization;
mod script_inserted;
mod selection_pointer_timing;
mod service_worker;
mod static_plaintext;
mod storage_events;
mod stylesheets;
mod stylesheets_http;
mod subresources;
mod user_agent;
mod viewport;
mod web_animations;

/// Loads nothing: the pages' sub-resources come from the loader.
struct NoFetch;
impl lp_render::Fetcher for NoFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: offline")));
    }
}

/// A browser thread whose pages are styled and laid out by Blitz, as the
/// binary's are (lp-cdp installs the same on its threads).
pub(crate) fn browser_thread() -> Result<lp_browser::BrowserThread, lp_js::JsError> {
    lp_layout::install(std::sync::Arc::new(NoFetch));
    lp_browser::BrowserThread::new()
}

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
