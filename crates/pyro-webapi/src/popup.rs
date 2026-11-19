//! The value `window.open` returns: the opener's stand-in for the popup's
//! WindowProxy.
//!
//! Each page has its own JS context and DOM, and the popup page does not
//! exist when `window.open` returns: the embedder creates it from the
//! request, later. [`PopupState`] is what the two sides share, and
//! [`PopupTicket`] is held by whoever is responsible for the page, the
//! pending request first and then the embedder's record of the page. A
//! dropped ticket (request refused, or page gone) reads as `closed`. The
//! embedder attaches the popup's current document to the ticket
//! (`attach_page`), from which point the stand-in reaches the real page.
//!
//! What the stand-in gives the opener:
//! - always: `closed`, `close()`, `focus()`/`blur()`, `location` (`href`,
//!   `assign`, `replace`, `toString`, or assigning `location` itself),
//!   `window`/`self`/`frames`/`top`/`parent` (itself), `opener` (the opener's
//!   global), `postMessage` (delivered to the popup's document when it
//!   exists), `fetch`, `localStorage` and `sessionStorage`;
//! - once the popup page exists and is same-origin: every other Window
//!   member reads through to the popup's real global (`document`,
//!   `addEventListener`, ...), and assignments to other names set them on
//!   that global. Cross-origin reads throw SecurityError, as browsers do;
//!   before the page exists they are undefined.
//!
//! Before the page exists, `fetch` runs in the opener's realm (the initial
//! about:blank document shares the opener's origin and base URL, so the
//! result is the same apart from the Response's realm) and `sessionStorage`
//! is the copy the popup will adopt, taken when `window.open` ran. All JS
//! contexts of a browser thread share one VM, so values cross realms as they
//! are (the same way same-origin frames reach each other).
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::page::PageDom;
use crate::storage::StorageNamespace;

/// The opener's view of a popup.
pub struct PopupState {
    closed: Cell<bool>,
    /// The popup's current URL; about:blank until its first commit.
    url: RefCell<String>,
    /// Browsing context name (`window.open`'s second argument, "" for `_blank`).
    name: String,
    /// The popup's session storage: a copy of the opener's taken when
    /// `window.open` ran (HTML "create a new top-level traversable" copies
    /// the session storage of the opener's top-level traversable).
    session: StorageNamespace,
    /// The popup's current document, once the embedder created the page.
    page: RefCell<Weak<PageDom>>,
    /// The opener set the popup's `opener` to null (`popup.opener = null`,
    /// before the popup's page existed or since): it reads null on both sides.
    opener_cleared: Cell<bool>,
    /// The popup's initial about:blank document, made by the embedder when
    /// `window.open` ran (HTML creates it synchronously), until the embedder
    /// turns it into the popup page (`PopupTicket::take_prebuilt`).
    prebuilt: RefCell<Option<Box<dyn std::any::Any>>>,
}

impl std::fmt::Debug for PopupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopupState")
            .field("closed", &self.closed.get())
            .field("url", &self.url.borrow())
            .field("name", &self.name)
            .finish()
    }
}

impl PopupState {
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// The popup's live document, if the page exists.
    fn page(&self) -> Option<Rc<PageDom>> {
        self.page.borrow().upgrade().filter(|p| !p.is_disposed())
    }
}

/// Ownership of a popup page; see the module docs.
#[derive(Debug)]
pub struct PopupTicket(Rc<PopupState>);

impl PopupTicket {
    pub(crate) fn new(name: String, session: StorageNamespace) -> (Rc<Self>, Rc<PopupState>) {
        let state = Rc::new(PopupState {
            closed: Cell::new(false),
            url: RefCell::new("about:blank".to_owned()),
            name,
            session,
            page: RefCell::new(Weak::new()),
            opener_cleared: Cell::new(false),
            prebuilt: RefCell::new(None),
        });
        (Rc::new(Self(state.clone())), state)
    }

    pub fn is_for(&self, popup: &Rc<PopupState>) -> bool {
        Rc::ptr_eq(&self.0, popup)
    }

    /// The popup committed `url`.
    pub fn set_url(&self, url: &str) {
        *self.0.url.borrow_mut() = url.to_owned();
    }

    /// The browsing context name the popup was opened with ("" for `_blank`).
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The session storage namespace the popup page must use.
    pub fn session_storage(&self) -> StorageNamespace {
        self.0.session.clone()
    }

    /// Whether the opener set `popup.opener = null`: the popup's own
    /// `window.opener` reads null too.
    pub fn opener_cleared(&self) -> bool {
        self.0.opener_cleared.get()
    }

    /// The popup page's current document (each navigation attaches the new
    /// one), through which the opener's stand-in reaches the popup.
    pub fn attach_page(&self, page: &Rc<PageDom>) {
        *self.0.page.borrow_mut() = Rc::downgrade(page);
    }

    /// Keeps the embedder's popup page made ahead of its creation (its
    /// initial about:blank document, already attached with `attach_page`).
    pub fn set_prebuilt(&self, prebuilt: Box<dyn std::any::Any>) {
        *self.0.prebuilt.borrow_mut() = Some(prebuilt);
    }

    /// The popup page made ahead, once.
    pub fn take_prebuilt(&self) -> Option<Box<dyn std::any::Any>> {
        self.0.prebuilt.borrow_mut().take()
    }
}

impl PopupTicket {
    /// The popup's WindowProxy stand-in as `opener` (the opener's document)
    /// sees it in `s`'s realm: the object its `window.open` returned.
    pub fn window_in<'s>(&self, s: &Scope<'s>, opener: &Rc<PageDom>) -> JsResult<Value<'s>> {
        wrap_popup(s, opener, &self.0)
    }
}

impl Drop for PopupTicket {
    fn drop(&mut self) {
        self.0.closed.set(true);
    }
}

impl PartialEq for PopupTicket {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for PopupTicket {}

/// What the opener asked of its popup through the returned object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupCommand {
    Close,
    Navigate(String),
}

#[derive(Clone)]
pub struct PopupData {
    /// The opener's document: URLs resolve against it and origins compare to it.
    page: Rc<PageDom>,
    popup: Rc<PopupState>,
}

impl PopupData {
    pub(crate) fn new(page: &Rc<PageDom>, popup: Rc<PopupState>) -> Self {
        Self { page: page.clone(), popup }
    }

    fn navigate(&self, url: &str) -> JsResult<()> {
        let resolved = {
            let dom = self.page.dom.borrow();
            crate::html::resolve_url(&dom, self.page.document(), url)
        };
        let parsed = pyro_net::Url::parse(&resolved)
            .map_err(|_| JsError::dom("SyntaxError", format!("'{url}' is not a valid URL.")))?;
        // A javascript: URL would run the opener's script in the popup's
        // document, which the popup's origin may not allow.
        if parsed.scheme() != "javascript" && !self.popup.closed.get() {
            self.page.popup_command(self.popup.clone(), PopupCommand::Navigate(resolved));
        }
        Ok(())
    }

    /// The opener's tuple origin, if it has one.
    fn opener_origin(&self) -> Option<String> {
        crate::storage::document_origin(&self.page)
    }

    /// Whether the opener may reach into the popup: its document is the
    /// initial about:blank (which inherits the opener's origin) or shares
    /// the opener's tuple origin.
    fn same_origin(&self) -> bool {
        if let Some(page) = self.popup.page() {
            return match crate::storage::document_origin(&page) {
                Some(origin) => self.opener_origin() == Some(origin),
                None => {
                    let url = page.dom.borrow().document_data(page.document()).ok().map(|d| d.url.to_string());
                    url.as_deref() == Some("about:blank")
                }
            };
        }
        let url = self.popup.url.borrow();
        url.as_str() == "about:blank"
            || pyro_net::Url::parse(&url).is_ok_and(|u| {
                let origin = u.origin();
                origin.is_tuple() && self.opener_origin() == Some(origin.ascii_serialization())
            })
    }

    /// The popup's current URL: its document's once the page exists,
    /// otherwise what the embedder reported committed (about:blank at first).
    fn url(&self) -> String {
        if let Some(page) = self.popup.page() {
            if let Ok(data) = page.dom.borrow().document_data(page.document()) {
                return data.url.to_string();
            }
        }
        self.popup.url.borrow().clone()
    }

    fn href(&self) -> JsResult<String> {
        if self.same_origin() {
            Ok(self.url())
        } else {
            Err(JsError::dom("SecurityError", "Blocked a read of a cross-origin popup's location."))
        }
    }

    /// The popup's live, same-origin document and its main-world realm.
    fn reachable_page(&self) -> JsResult<Option<(Rc<PageDom>, pyro_js::Realm)>> {
        let Some(page) = self.popup.page() else { return Ok(None) };
        if !self.same_origin() {
            return Err(JsError::dom("SecurityError", "Blocked a frame from accessing a cross-origin frame."));
        }
        Ok(page.main_world_realm().map(|realm| (page, realm)))
    }

    /// A Window member of the popup's real global, brought into `s`'s realm.
    fn forward<'s>(&self, s: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>> {
        let Some((_, realm)) = self.reachable_page()? else { return Ok(None) };
        let scope = realm.scope()?;
        let value = scope.global().get(name)?;
        Ok(Some(s.rehome(value)))
    }

    /// `popup.fetch(...)`: the popup's own `fetch` once its page exists,
    /// otherwise the opener's (see the module docs).
    fn fetch<'s>(&self, s: &Scope<'s>, a: &Args<'s>) -> JsResult<Value<'s>> {
        let args: Vec<Value<'s>> = a.iter().collect();
        match self.reachable_page()? {
            Some((_, realm)) => {
                let scope = realm.scope()?;
                let fetch = scope.global().get_atom("fetch")?;
                let fetch = fetch.as_object().filter(|f| f.is_function()).ok_or(JsError::ContextDestroyed)?;
                let result = fetch.call(None, &args)?;
                Ok(s.rehome(result))
            }
            None => {
                let fetch = s.global().get_atom("fetch")?;
                let fetch = fetch.as_object().filter(|f| f.is_function()).ok_or(JsError::ContextDestroyed)?;
                fetch.call(None, &args)
            }
        }
    }

    /// `popup.localStorage` / `popup.sessionStorage`.
    fn storage<'s>(&self, s: &Scope<'s>, session: bool) -> JsResult<Value<'s>> {
        if let Some((page, realm)) = self.reachable_page()? {
            let scope = realm.scope()?;
            let value = crate::storage::window_storage(&scope, &page, session)?;
            return Ok(s.rehome(value));
        }
        if !self.same_origin() {
            return Err(JsError::dom("SecurityError", "Blocked a frame from accessing a cross-origin frame."));
        }
        if !session {
            // The same browser context's namespace and the same origin: the
            // very area the popup will use.
            return crate::storage::window_storage(s, &self.page, false);
        }
        // The popup's session area is in the namespace copied at `window.open`.
        // `Storage` wrappers are built for a page's current namespace, so the
        // opener's is swapped for the copy while this one is made: the area
        // (and so the wrapper, keyed by the area) is the popup's own, shared
        // with the page once it exists. A change through this wrapper is
        // attributed to the opener page, so `storage` events reach the
        // opener's other same-origin documents instead of the popup's.
        let previous = self.page.session_storage.replace(self.popup.session.clone());
        let result = crate::storage::window_storage(s, &self.page, true);
        *self.page.session_storage.borrow_mut() = previous;
        result
    }

    /// `popup.postMessage(message, targetOrigin, transfer)`: structured
    /// clone in the opener, delivered as a task in the popup's document.
    fn post_message<'s>(&self, s: &Scope<'s>, a: &Args<'s>) -> JsResult<()> {
        let sender = self.opener_origin().unwrap_or_else(|| "null".into());
        let (message, wanted, options) = crate::window_message::post_message_args(s, a, &sender)?;
        // Clone even when the message will be dropped (errors are reported
        // regardless of the target).
        let message = crate::structured_clone::encode_message(s, message, options)?;
        let Some(page) = self.popup.page() else { return Ok(()) };
        if !wanted.allows(crate::storage::document_origin(&page).as_deref().unwrap_or("null")) {
            return Ok(());
        }
        deliver_message(&page, message, sender);
        Ok(())
    }
}

/// Fires a `message` event with `message` at `page`'s window, as a task.
pub(crate) fn deliver_message(page: &Rc<PageDom>, message: crate::structured_clone::WireMessage, origin: String) {
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() {
            return;
        }
        let (Some(window), Some(realm)) = (page.window_target(), page.main_world_realm()) else { return };
        let Ok(scope) = realm.scope() else { return };
        crate::message_channel::fire_wire_message(&scope, &page, window, &message, origin);
    }));
}

/// The opener realm's wrapper of `popup` (one per realm, as a WindowProxy).
pub(crate) fn wrap_popup<'s>(s: &Scope<'s>, page: &Rc<PageDom>, popup: &Rc<PopupState>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 54), Rc::as_ptr(popup) as u64);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<PopupWindow>(PopupData::new(page, popup.clone()), Some(key))?.value())
}

/// The object `window.open` returns. Not exposed on the global.
pub struct PopupWindow;
impl JsClass for PopupWindow {
    const NAME: &'static str = "PopupWindow";
    type Data = PopupData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.getter("closed", |s, d| Ok(s.boolean(d.popup.closed.get())));
        def.method("close", 0, |s, d, _a| {
            // `closed` reads true from here on, before the page is gone.
            if !d.popup.closed.replace(true) {
                d.page.popup_command(d.popup.clone(), PopupCommand::Close);
            }
            Ok(s.undefined())
        });
        // Headless pages have no window focus to move between them.
        def.method("focus", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("blur", 0, |s, _d, _a| Ok(s.undefined()));
        def.accessor(
            "location",
            |s, d| {
                // Keyed by the shared state, which the wrapper keeps alive.
                let key = pyro_js::IdentityKey(d.page.id() | (1 << 53), Rc::as_ptr(&d.popup) as u64);
                Ok(s.wrap::<PopupLocation>(d.clone(), Some(key))?.value())
            },
            |_s, d, v| d.navigate(&v.to_string()?),
        );
        for name in ["window", "self", "frames", "top", "parent"] {
            def.getter(name, |s, d| wrap_popup(s, &d.page, &d.popup));
        }
        // [Replaceable]: `popup.opener = null` (a common way to cut the
        // popup off from its opener before navigating it) must not throw in
        // strict code, and the popup's `opener` reads null from then on.
        def.accessor(
            "opener",
            |s, d| {
                if d.popup.opener_cleared.get() {
                    return Ok(s.null());
                }
                Ok(d.forward(s, "opener")?.unwrap_or_else(|| s.global().value()))
            },
            |_s, d, v| {
                if v.is_null() {
                    d.popup.opener_cleared.set(true);
                }
                if let Some((_, realm)) = d.reachable_page()? {
                    let scope = realm.scope()?;
                    scope.global().set("opener", scope.rehome(v))?;
                }
                Ok(())
            },
        );
        def.getter("length", |s, d| Ok(d.forward(s, "length")?.unwrap_or_else(|| s.number(0.0))));
        def.getter("name", |s, d| Ok(d.forward(s, "name")?.unwrap_or_else(|| s.string(&d.popup.name))));
        def.getter("document", |s, d| Ok(d.forward(s, "document")?.unwrap_or_else(|| s.undefined())));
        def.getter("localStorage", |s, d| d.storage(s, false));
        def.getter("sessionStorage", |s, d| d.storage(s, true));
        def.method("fetch", 1, |s, d, a| d.fetch(s, &a));
        def.method("postMessage", 1, |s, d, a| {
            d.post_message(s, &a)?;
            Ok(s.undefined())
        });
        // Everything else reads through to the popup's global once it exists,
        // and assigning it (`popup.x = v`) sets it there, as a WindowProxy's
        // [[Set]] does; before the page exists it stays on the stand-in.
        def.named(false, |s, d, name| d.forward(s, name));
        def.named_setter(|_s, d, name, v| {
            let Some((_, realm)) = d.reachable_page()? else { return Ok(false) };
            let scope = realm.scope()?;
            scope.global().set(name, scope.rehome(v))?;
            Ok(true)
        });
    }
}

/// `popup.location`.
pub struct PopupLocation;
impl JsClass for PopupLocation {
    const NAME: &'static str = "PopupLocation";
    type Data = PopupData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.accessor("href", |s, d| Ok(s.string(&d.href()?)), |_s, d, v| d.navigate(&v.to_string()?));
        def.method("assign", 1, |s, d, a| navigate_arg(s, d, a.get(0)));
        def.method("replace", 1, |s, d, a| navigate_arg(s, d, a.get(0)));
        def.method("toString", 0, |s, d, _a| Ok(s.string(&d.href()?)));
    }
}

fn navigate_arg<'s>(s: &Scope<'s>, d: &PopupData, url: Value<'s>) -> JsResult<Value<'s>> {
    d.navigate(&url.to_string()?)?;
    Ok(s.undefined())
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<PopupWindow>();
    engine.register::<PopupLocation>();
}
