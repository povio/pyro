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
mod animation_events;
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
pub mod media_source;
pub mod message_channel;
pub mod mutation_observer;
pub mod navigator_extras;
pub mod node;
pub mod page;
mod popup;
pub mod range;
pub mod render_blocking;
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
pub use render_blocking::RenderBlocker;
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
    media_source::register(engine);
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
    page.register_realm(&realm);
    // Document named properties (`document.formName`): the engine drops its
    // caches of a name when an element that may be named it appears.
    let names = scope.impure_names();
    page.dom.borrow_mut().set_named_item_hook(Some(Rc::new(move |name: &str| names.announce(name))));
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
    if let Some(define) = scope.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()? {
        page.define_properties.borrow_mut().insert(scope.realm_id(), lp_js::Persistent::new(scope, define));
    }
    if page.max_touch_points() > 0 {
        handlers::define_touch_handlers(scope)?;
    }
    let global = scope.global();
    // A window's `document` and `window` ([LegacyUnforgeable]) never change:
    // they are the global's own constant attributes (not members of
    // Window.prototype, see `window::has_window_constants`), which read with
    // no callback and which compiled code holds as constants,
    // `window.document` included. The realm releases them when it is
    // disposed. `document` is made on its first read: its wrapper installs
    // the Document, Node and EventTarget interfaces, which a realm whose
    // script never reads it (an initial about:blank document, a blank
    // iframe, an isolated world) does not need. It is the document the
    // realm was installed for, whatever the page displays by then. Without
    // the patched JavaScriptCore the interface's getters serve them.
    let document = page.document();
    let make: lp_js::LazyGlobalConstant = Rc::new(move |s: &Scope<'_>| window_document(s, document));
    if scope.define_lazy_global_constant("document", make) {
        scope.define_global_constant("window", global.value());
    }
    // [Unscopable] members of ChildNode and ParentNode, defined with each
    // interface's members: reading `Element.prototype` here would install
    // Element, Node, Document and the rest in every new realm.
    let unscopables = scope.eval(
        r#"(() => {
        const create = Object.create, define = Object.defineProperty, symbol = Symbol.unscopables;
        // Runs when a page first uses the interface: no iteration, no
        // descriptor that could inherit from a changed Object.prototype.
        const u = (names) => (proto) => { const o = create(null); for (let i = 0; i < names.length; i++) o[names[i]] = true;
            define(proto, symbol, { __proto__: null, value: o, configurable: true }); };
        const child = ['before', 'after', 'replaceWith', 'remove'], parent = ['prepend', 'append', 'replaceChildren'];
        return [u([...child, ...parent]), u(child), u(child), u(parent), u(parent)];
    })()"#,
        Some("lightningpanda://internal/unscopables.js"),
        1,
    )?;
    let unscopables = unscopables.as_object().expect("unscopables hooks");
    let hook = |i: u32| -> JsResult<lp_js::Object<'_>> {
        unscopables.get_index(i)?.as_object().ok_or_else(|| lp_js::JsError::Type("unscopables hook".into()))
    };
    scope.after_populate::<node::Element>(hook(0)?)?;
    scope.after_populate::<node::CharacterData>(hook(1)?)?;
    scope.after_populate::<node::DocumentType>(hook(2)?)?;
    scope.after_populate::<node::Document>(hook(3)?)?;
    scope.after_populate::<node::DocumentFragment>(hook(4)?)?;
    Ok(())
}

/// The value of a window's `document` constant, made on its first read: the
/// wrapper of `document`, the page's document when the realm was installed.
fn window_document<'s>(scope: &Scope<'s>, document: lp_dom::NodeId) -> JsResult<Value<'s>> {
    let page = target::page_of(scope)?;
    Ok(node::wrap_node(scope, &page, document)?.value())
}

/// Resolves a JS value to its native node (CDP DOM.describeNode etc.).
pub fn node_from_value(scope: &Scope<'_>, value: Value<'_>) -> Option<(Rc<PageDom>, lp_dom::NodeId)> {
    let n = scope.unwrap::<node::Node>(value)?;
    Some((n.page().clone(), n.id))
}
