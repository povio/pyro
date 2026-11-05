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
pub mod canvas;
pub mod collections;
pub mod crypto;
pub mod css;
pub mod custom;
pub mod data_transfer;
pub mod dom_parser;
pub mod editing;
pub mod encoding;
pub mod event;
pub mod fetch;
pub mod file;
pub mod file_reader;
pub mod form_data;
pub mod formdata;
pub mod handlers;
pub mod headers;
pub mod html;
pub mod identity;
pub mod indexeddb;
pub mod input;
pub mod intersection_observer;
pub mod layout_api;
pub mod lifecycle;
pub mod message_channel;
pub mod mutation_observer;
pub mod navigator_extras;
pub mod node;
pub mod page;
mod popup;
pub mod range;
pub mod render_snapshot;
pub mod resize_observer;
pub mod request;
pub mod observable;
pub mod response;
pub mod storage;
mod streams;
mod structured_clone;
pub mod target;
pub mod traversal;
pub mod url;
pub mod url_search_params;
pub mod util;
pub mod validity;
pub mod window;
pub mod worker;
mod worker_scope;
pub mod xhr;
pub mod xpath;

use std::rc::Rc;

use lp_js::{JsResult, Scope, Value};

pub use event::{EventCore, EventData, EventInterface};
pub use fetch::{FetchLimits, RequestContext};
pub use file_reader::FileReaderLimits;
pub use identity::UserAgentOverride;
pub use navigator_extras::{ContextServices, GeolocationFix, PermissionSetting, UserAgentMetadata};
pub use node::{wrap_node, NodeData};
pub use page::{
    DialogAnswer, DownloadRequest, FocusTarget, FocusTransition, FrameViewportSource, IframeAttribute,
    NavigationRequest, PageDom, PageHooks, RemotePoster,
};
pub use popup::{PopupCommand, PopupState, PopupTicket};
pub use storage::{StorageNamespace, StorageSeed};
pub use target::{EventTarget, EventTargetData};
pub use window::{define_members, WindowHost};
pub use worker::{
    hold_for_fork, join_stopped, release_after_fork, InspectTask, WorkerEvent, WorkerHandle, MAX_WORKERS_PER_DOCUMENT,
};
pub use worker_scope::{BufferedEvent, WorkerContext, WorkerEvents};
pub use xhr::XhrLimits;

/// Registers every host class with the engine (call before creating realms).
pub fn register(engine: &lp_js::Engine) {
    engine.set_wrapper_property_hook(target::pin_wrapper_with_properties);
    engine.register::<target::EventTarget>();
    event::register(engine);
    popup::register(engine);
    node::register(engine);
    html::register(engine);
    collections::register(engine);
    css::register(engine);
    layout_api::register(engine);
    canvas::register(engine);
    traversal::register(engine);
    range::register(engine);
    abort::register(engine);
    message_channel::register(engine);
    worker::register(engine);
    custom::register(engine);
    file::register(engine);
    engine.register::<file_reader::FileReader>();
    data_transfer::register(engine);
    encoding::register(engine);
    validity::register(engine);
    window::register(engine);
    mutation_observer::register(engine);
    intersection_observer::register(engine);
    crypto::register(engine);
    form_data::register(engine);
    dom_parser::register(engine);
    resize_observer::register(engine);
    xpath::register(engine);
    engine.register::<storage::Storage>();
    engine.register::<indexeddb::IdbFactory>();
    engine.register::<headers::Headers>();
    engine.register::<url::URL>();
    engine.register::<url_search_params::URLSearchParams>();
    engine.register::<url_search_params::ParamsIterator>();
    engine.register::<response::Response>();
    engine.register::<request::Request>();
    engine.register::<response::BodySource>();
    engine.register::<xhr::XMLHttpRequest>();
    engine.register::<xhr::XMLHttpRequestUpload>();
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
    realm.set_named_properties(Rc::new(window::WindowNamedProperties(page.clone())));
    realm.set_unhandled_rejection_handler(event::dispatch_unhandled_rejection);
    mutation_observer::install(scope, &page)?;
    intersection_observer::install(scope, &page)?;
    encoding::install(scope)?;
    headers::install(scope)?;
    url_search_params::install(scope)?;
    form_data::install(scope)?;
    fetch::install(scope)?;
    structured_clone::install(scope, &page)?;
    if let Some(define) = scope.global().get("Object")?.as_object().map(|o| o.get("defineProperty")).transpose()? {
        page.define_properties.borrow_mut().insert(scope.realm_id(), lp_js::Persistent::new(scope, define));
    }
    if page.max_touch_points() > 0 {
        handlers::define_touch_handlers(scope)?;
    }
    let global = scope.global();
    global.set("CSS", window::css_namespace(scope)?)?;
    // [Unscopable] members of ChildNode and ParentNode.
    scope.eval(
        r#"(() => {
        const u = (proto, names) => { const o = Object.create(null); for (const n of names) o[n] = true;
            Object.defineProperty(proto, Symbol.unscopables, { value: o, configurable: true }); };
        const child = ['before', 'after', 'replaceWith', 'remove'], parent = ['prepend', 'append', 'replaceChildren'];
        u(Element.prototype, [...child, ...parent]);
        u(CharacterData.prototype, child);
        u(DocumentType.prototype, child);
        u(Document.prototype, parent);
        u(DocumentFragment.prototype, parent);
    })()"#,
        Some("lightningpanda://internal/unscopables.js"),
        1,
    )?;
    Ok(())
}

/// Resolves a JS value to its native node (CDP DOM.describeNode etc.).
pub fn node_from_value(scope: &Scope<'_>, value: Value<'_>) -> Option<(Rc<PageDom>, lp_dom::NodeId)> {
    let n = scope.unwrap::<node::Node>(value)?;
    Some((n.page().clone(), n.id))
}
