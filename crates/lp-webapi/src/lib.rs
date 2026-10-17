//! lp-webapi: JavaScript bindings for the DOM and web platform APIs of
//! lightningpanda, over lp-js (JavaScriptCore), lp-dom and lp-net.
//!
//! Embedding (lp-browser):
//! 1. `register(&engine)` once, before creating realms.
//! 2. Per page: `PageDom::new(dom, document)`; `page.set_hooks(..)`;
//!    `dom.set_resource_tracking(true)`.
//! 3. Per realm (main world and isolated worlds), right after creation and
//!    before init scripts: `install(&scope, page.clone())`.
//! 4. lp-browser's `Window` class: `type Parent = EventTarget`,
//!    `def.upcast(|w| w.event_target())` and `define_members::<Window>(def)`.
//! 5. After every task: `page.after_task()`; drain
//!    `dom.take_resource_events()` and pass style events to
//!    `page.apply_style_event`.
//! 6. Before disposing a document's realms on navigation/page teardown:
//!    `page.dispose()`. Per-world removal can use `lifecycle::realm_disposed`.

pub mod abort;
pub mod collections;
pub mod css;
pub mod custom;
pub mod encoding;
pub mod event;
pub mod fetch;
pub mod file;
pub mod formdata;
pub mod headers;
pub mod html;
pub mod identity;
pub mod indexeddb;
pub mod input;
pub mod layout_api;
pub mod lifecycle;
pub mod mutation_observer;
pub mod node;
pub mod page;
pub mod range;
pub mod response;
pub mod storage;
pub mod target;
pub mod traversal;
pub mod url_search_params;
pub mod util;
pub mod validity;
pub mod window;
pub mod xhr;

use std::rc::Rc;

use lp_js::{JsResult, Scope, Value};

pub use event::{EventCore, EventData, EventInterface};
pub use fetch::{FetchLimits, RequestContext};
pub use identity::UserAgentOverride;
pub use node::{wrap_node, NodeData};
pub use page::{NavigationRequest, PageDom, PageHooks, RemotePoster};
pub use storage::StorageNamespace;
pub use target::{EventTarget, EventTargetData};
pub use window::{define_members, WindowHost};
pub use xhr::XhrLimits;

/// Registers every host class with the engine (call before creating realms).
pub fn register(engine: &lp_js::Engine) {
    engine.register::<target::EventTarget>();
    event::register(engine);
    node::register(engine);
    html::register(engine);
    collections::register(engine);
    css::register(engine);
    layout_api::register(engine);
    traversal::register(engine);
    range::register(engine);
    abort::register(engine);
    custom::register(engine);
    file::register(engine);
    encoding::register(engine);
    validity::register(engine);
    window::register(engine);
    mutation_observer::register(engine);
    engine.register::<storage::Storage>();
    engine.register::<indexeddb::IdbFactory>();
    engine.register::<headers::Headers>();
    engine.register::<url_search_params::URLSearchParams>();
    engine.register::<url_search_params::ParamsIterator>();
    engine.register::<response::Response>();
    engine.register::<xhr::XMLHttpRequest>();
}

/// Installs the page's bindings into a freshly created realm.
pub fn install(scope: &Scope<'_>, page: Rc<PageDom>) -> JsResult<()> {
    if page.is_disposed() {
        return Err(lp_js::JsError::ContextDestroyed);
    }
    let realm = scope.realm();
    page.realms.borrow_mut().insert(realm.id(), realm.downgrade());
    xhr::install(scope, &page)?;
    realm.set_embedder(page.clone());
    mutation_observer::install(scope, &page)?;
    encoding::install(scope)?;
    headers::install(scope)?;
    url_search_params::install(scope)?;
    fetch::install(scope)?;
    let global = scope.global();
    global.set("CSS", window::css_namespace(scope)?)?;
    Ok(())
}

/// Resolves a JS value to its native node (CDP DOM.describeNode etc.).
pub fn node_from_value(scope: &Scope<'_>, value: Value<'_>) -> Option<(Rc<PageDom>, lp_dom::NodeId)> {
    let n = scope.unwrap::<node::Node>(value)?;
    Some((n.page().clone(), n.id))
}
