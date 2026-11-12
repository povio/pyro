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
mod animations;
mod cache_storage;
pub mod canvas;
mod canvas_path;
mod clipboard;
pub mod collections;
mod compression;
mod computed_properties;
pub mod crypto;
mod crypto_subtle;
pub mod css;
pub mod custom;
pub mod data_transfer;
pub mod dom_parser;
mod edit_commands;
pub mod editing;
mod editing_globals;
pub mod encoding;
pub mod event;
mod event_source;
pub mod fetch;
pub mod file;
pub mod file_reader;
pub mod form_data;
pub mod formdata;
pub mod handlers;
pub mod headers;
pub mod html;
pub mod identity;
mod image_decode;
pub mod indexeddb;
pub mod input;
mod integrity;
mod internals;
pub mod intersection_observer;
pub mod layout_api;
pub(crate) mod lazy_module;
pub mod lifecycle;
pub mod media;
pub(crate) mod media_format;
pub mod media_source;
pub mod message_channel;
pub mod module_script;
pub mod mutation_observer;
pub mod navigator_extras;
pub mod node;
pub mod observable;
pub mod page;
pub mod performance;
mod pointer_capture;
mod popover;
mod popup;
mod port_link;
pub mod range;
pub mod render_blocking;
pub mod render_snapshot;
pub mod report_error;
pub mod request;
pub mod resize_observer;
pub mod resource_timing;
pub mod response;
mod service_worker;
mod service_worker_scope;
pub mod sri;
pub mod storage;
mod storage_manager;
mod streams;
mod structured_clone;
pub mod target;
pub mod thread_state;
pub mod traversal;
pub mod url;
pub mod url_search_params;
pub mod util;
pub mod validity;
pub(crate) mod webaudio;
pub mod websocket;
pub mod window;
pub mod window_message;
pub mod worker;
mod worker_scope;
pub mod xhr;
pub mod xpath;

use std::rc::Rc;

use lp_js::{JsResult, Object, Scope, Value};

pub use event::{EventCore, EventData, EventInterface};
pub use fetch::{FetchLimits, RequestContext};
pub use file_reader::FileReaderLimits;
pub use identity::UserAgentOverride;
pub use navigator_extras::{ContextServices, GeolocationFix, PermissionSetting, UserAgentMetadata};
pub use node::{wrap_node, NodeData};
pub use page::{
    BodyPart, DialogAnswer, DocumentTiming, DownloadRequest, FocusTarget, FocusTransition, FrameViewportSource,
    IframeAttribute, MediaFetchHandle, MediaRequest, MediaResponse, NavigationRequest, PageDom, PageHooks, Partial,
    RemotePoster,
};
pub use popup::{PopupCommand, PopupState, PopupTicket};
pub use render_blocking::RenderBlocker;
pub use service_worker::{navigation_route, subresource_route, LoadRoute};
pub use storage::{StorageNamespace, StorageSeed};
pub use structured_clone::{decode_message as deserialize_message, encode_message as serialize_message, WireMessage};
pub use target::{EventTarget, EventTargetData};
pub use window::{define_members, WindowHost};
pub use worker::{
    hold_for_fork, join_stopped, release_after_fork, take_worker_ended, worker_ended, InspectTask, WorkerEvent,
    WorkerHandle, MAX_WORKERS_PER_DOCUMENT,
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
    internals::register(engine);
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
    engine.register::<cache_storage::CacheHandle>();
    engine.register::<cache_storage::StoredBody>();
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

/// The JavaScript the bindings of a realm capture before page script runs
/// (install.js, with structured_clone.js as its argument): one script, one
/// evaluation per realm.
const INSTALL_JS: &str = concat!("(", include_str!("install.js"), ")(", include_str!("structured_clone.js"), ")");

/// Evaluates install.js in the realm: its pieces by name.
pub(crate) fn install_helpers<'s>(scope: &Scope<'s>) -> JsResult<Object<'s>> {
    let helpers = scope.eval(INSTALL_JS, Some("lightningpanda://internal/install.js"), 1)?;
    helpers.as_object().ok_or_else(|| lp_js::JsError::Type("install.js helpers".into()))
}

thread_local! {
    /// The pages of each `Dom` (by address) that keep node wrappers: the
    /// documents of an iframe share their parent's `Dom`, each page with its
    /// own wrapper table.
    static DOM_PAGES: std::cell::RefCell<rustc_hash::FxHashMap<usize, Rc<std::cell::RefCell<Vec<std::rc::Weak<PageDom>>>>>> =
        Default::default();
}

/// [`thread_state::reset_thread`]'s part for `DOM_PAGES`.
fn dom_pages_leftover() -> Option<&'static str> {
    DOM_PAGES.with(|map| {
        let mut map = map.borrow_mut();
        map.retain(|_, pages| pages.borrow().iter().any(|p| p.strong_count() > 0));
        (!map.is_empty()).then_some("lib::DOM_PAGES")
    })
}

/// A node holding listeners keeps its wrapper (and so the listeners) from the
/// moment an insertion connects it (`Dom::set_listener_connected_hook`), in
/// every page sharing the `Dom`.
fn install_listener_connected_hook(page: &Rc<PageDom>) {
    let key = Rc::as_ptr(&page.dom) as usize;
    let (pages, new) = DOM_PAGES.with(|map| {
        let mut map = map.borrow_mut();
        map.retain(|_, pages| pages.borrow().iter().any(|p| p.strong_count() > 0));
        match map.get(&key) {
            Some(pages) => (pages.clone(), false),
            None => {
                let pages = Rc::new(std::cell::RefCell::new(Vec::new()));
                map.insert(key, pages.clone());
                (pages, true)
            }
        }
    });
    {
        let mut list = pages.borrow_mut();
        list.retain(|p| p.strong_count() > 0);
        if !list.iter().any(|p| std::ptr::eq(p.as_ptr(), Rc::as_ptr(page))) {
            list.push(Rc::downgrade(page));
        }
    }
    let mut dom = page.dom.borrow_mut();
    if new || !dom.has_listener_connected_hook() {
        dom.set_listener_connected_hook(Some(Rc::new(move |idx, all, callbacks| {
            let Ok(list) = pages.try_borrow() else { return };
            for page in list.iter().filter_map(std::rc::Weak::upgrade) {
                page.keep_connected_listener_node(idx, all, callbacks);
            }
        })));
    }
}

/// Installs the page's bindings into a freshly created realm.
pub fn install(scope: &Scope<'_>, page: Rc<PageDom>) -> JsResult<()> {
    install_with_helpers(scope, page).map(drop)
}

/// [`install`], returning the realm's install.js helpers for the embedder's
/// own setup (`accessor`: an enumerable, configurable getter on the global).
pub fn install_with_helpers<'s>(scope: &Scope<'s>, page: Rc<PageDom>) -> JsResult<Object<'s>> {
    if page.is_disposed() {
        return Err(lp_js::JsError::ContextDestroyed);
    }
    let realm = scope.realm();
    page.register_realm(&realm);
    // Document named properties (`document.formName`): the engine drops its
    // caches of a name when an element that may be named it appears.
    let names = scope.impure_names();
    page.dom.borrow_mut().set_named_item_hook(Some(Rc::new(move |name: &str| names.announce(name))));
    install_listener_connected_hook(&page);
    xhr::install(scope, &page)?;
    realm.set_embedder(page.clone());
    realm.set_named_properties(Rc::new(window::WindowNamedProperties(page.clone())));
    realm.set_unhandled_rejection_handler(event::dispatch_unhandled_rejection);
    let helpers = install_helpers(scope)?;
    mutation_observer::install(scope, &page, helpers.get_atom("mutationOptions")?)?;
    intersection_observer::install(scope, &page, helpers.get_atom("intersection")?)?;
    encoding::install(scope, helpers.get_atom("dataView")?)?;
    headers::install(scope, helpers.get_atom("headers")?)?;
    url_search_params::install(scope, helpers.get_atom("urlSearchParams")?)?;
    form_data::install(scope, helpers.get_atom("formData")?)?;
    fetch::install(scope)?;
    structured_clone::install(scope, &page, helpers.get_atom("structuredClone")?)?;
    if let Some(define) =
        scope.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()?
    {
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
    // interface's members (install.js `unscopables`): reading
    // `Element.prototype` here would install Element, Node, Document and the
    // rest in every new realm.
    let unscopables = helpers.get_atom("unscopables")?;
    let unscopables = unscopables.as_object().expect("unscopables hooks");
    let hook = |i: u32| -> JsResult<lp_js::Object<'_>> {
        unscopables.get_index(i)?.as_object().ok_or_else(|| lp_js::JsError::Type("unscopables hook".into()))
    };
    scope.after_populate::<node::Element>(hook(0)?)?;
    scope.after_populate::<node::CharacterData>(hook(1)?)?;
    scope.after_populate::<node::DocumentType>(hook(2)?)?;
    scope.after_populate::<node::Document>(hook(3)?)?;
    scope.after_populate::<node::DocumentFragment>(hook(4)?)?;
    Ok(helpers)
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
