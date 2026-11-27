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
//! The opener gets a WindowProxy (`wrap_popup`): a proxy whose internal
//! methods follow HTML. Same origin, every property operation (get, set,
//! has, delete, define, describe, list, prototype) goes to the popup's
//! global, and functions read through it run with that global as `this`;
//! cross origin, only CrossOriginProperties are there (`window`, `self`,
//! `frames`, `top`, `parent`, `location` (settable), `opener`, `closed`,
//! `length`, `close()`, `focus()`, `blur()`, `postMessage()`) and anything
//! else, writes and definitions included, throws SecurityError. Behind it
//! the stand-in (`PopupWindow`) answers what holds whatever the popup's
//! page: `closed`, `close()`, `focus()`/`blur()`, `location` (`href`,
//! `assign`, `replace`, `toString`, or assigning `location` itself),
//! `opener`, `postMessage` (delivered to the popup's document when it
//! exists), `fetch`, `localStorage` and `sessionStorage`, and, while the
//! popup has no page, every read (other names undefined; expandos stay on
//! the proxy).
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

thread_local! {
    /// How many top-level documents of this thread name a COOP reporting endpoint
    /// (`Cross-Origin-Opener-Policy[-Report-Only]: ...; report-to="x"`): while none does, a
    /// cross-window property access costs this read and nothing else.
    static COOP_REPORTING_DOCUMENTS: Cell<usize> = const { Cell::new(0) };
}

/// The embedder's count of documents naming a COOP reporting endpoint changed by `delta` (see
/// [`note_window_access`]).
pub fn coop_reporting_documents(delta: isize) {
    let active = COOP_REPORTING_DOCUMENTS.with(|c| {
        c.set(c.get().saturating_add_signed(delta));
        c.get() != 0
    });
    // The WindowProxy helpers of every realm read the engine's flag, not this count.
    pyro_js::set_coop_reporting(active);
}

/// Whether some document names a COOP reporting endpoint.
#[inline]
pub fn coop_reporting_active() -> bool {
    COOP_REPORTING_DOCUMENTS.with(|c| c.get() != 0)
}

/// The reserved property name through which helpers.js tells a stand-in which property of its window
/// the script reached (`windowProxy`, `crossOriginWindow`): the rest of the name is the property
/// (`blur`, `indexed`, `named`, `named:<name>`). Never a name a page can reach: a stand-in is not
/// handed to page code.
pub const ACCESS_NOTE: &str = "\0coop:";

/// Script of the running (incumbent) window reached `property` of the window whose document is
/// `target` (a WindowProxy property of HTML's CrossOriginProperties, `indexed` or `named`): COOP
/// access reporting (`PageHooks::coop_access`). Nothing happens, at the cost of one thread-local
/// read, while no document names a COOP reporting endpoint, and when `target` is the running window's
/// own document.
pub fn note_window_access(target: &Rc<PageDom>, property: &str) {
    if !coop_reporting_active() {
        return;
    }
    let Some(realm) = pyro_js::entry_realm() else { return };
    let Ok(entry) = realm.scope() else { return };
    let Ok(accessor) = crate::target::page_of(&entry) else { return };
    if Rc::ptr_eq(&accessor, target) {
        return;
    }
    let (Some(accessor_hooks), Some(target_side)) = (accessor.hooks(), target.hooks().and_then(|h| h.coop_side())) else {
        return;
    };
    accessor_hooks.coop_access(&entry, target, &*target_side, property);
}

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
    /// The embedder attached a document to the popup at some point (`PopupTicket::attach_page`).
    attached: Cell<bool>,
    /// The opener set the popup's `opener` to null (`popup.opener = null`,
    /// before the popup's page existed or since): it reads null on both sides.
    opener_cleared: Cell<bool>,
    /// A navigation of the popup's top-level frame left its browsing context group (HTML
    /// "check if COOP values require a browsing context group switch", `PopupTicket::sever`): the
    /// opener sees a closed window (`closed`, no `length`, no `name`, no document, no messages)
    /// and the popup's `opener` is null.
    severed: Cell<bool>,
    /// The popup's initial about:blank document, made by the embedder when
    /// `window.open` ran (HTML creates it synchronously), until the embedder
    /// turns it into the popup page (`PopupTicket::take_prebuilt`).
    prebuilt: RefCell<Option<Box<dyn std::any::Any>>>,
    /// The sandboxing flags the popup's documents take from its creator (HTML "popup sandboxing
    /// flag set"), as the embedder encodes them; 0 for none.
    sandbox: Cell<u32>,
    /// The embedder's id of the child frame whose document opened the popup by following a link
    /// or submitting a form (None: the top-level frame, or `window.open`, which records it itself).
    opener_child: Cell<Option<u64>>,
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
    /// Closed, or no longer in the opener's browsing context group: a closed window to the opener.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get() || self.severed.get()
    }

    /// HTML "discard a browsing context" has run: the popup was closed and its document is gone.
    /// Until then (`window.close()` only queues the discard) the window still has its opener,
    /// parent and top; afterwards `opener`, `parent` and `top` read null.
    pub(crate) fn is_discarded(&self) -> bool {
        self.is_closed() && self.attached.get() && self.page().is_none()
    }

    /// The popup's live document, if the page exists (and the opener may still reach it).
    fn page(&self) -> Option<Rc<PageDom>> {
        if self.severed.get() {
            return None;
        }
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
            attached: Cell::new(false),
            opener_cleared: Cell::new(false),
            severed: Cell::new(false),
            prebuilt: RefCell::new(None),
            sandbox: Cell::new(0),
            opener_child: Cell::new(None),
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

    /// The popup sandboxing flag set the embedder recorded for the popup (0: none).
    pub fn sandbox_bits(&self) -> u32 {
        self.0.sandbox.get()
    }
    pub fn set_sandbox_bits(&self, bits: u32) {
        self.0.sandbox.set(bits);
    }

    /// The child frame of the opener page that opened the popup (`PopupState::opener_child`).
    pub fn opener_child(&self) -> Option<u64> {
        self.0.opener_child.get()
    }
    pub fn set_opener_child(&self, child: Option<u64>) {
        self.0.opener_child.set(child);
    }

    /// The session storage namespace the popup page must use.
    pub fn session_storage(&self) -> StorageNamespace {
        self.0.session.clone()
    }

    /// Whether the popup's `window.opener` reads null: the opener set `popup.opener = null`, or
    /// the popup left the opener's browsing context group (`sever`).
    pub fn opener_cleared(&self) -> bool {
        self.0.opener_cleared.get() || self.0.severed.get()
    }

    /// The popup's top-level frame navigated to a document in a new browsing context group
    /// (Cross-Origin-Opener-Policy): the opener's WindowProxy for it reports a closed window from
    /// now on, and the popup has no opener.
    pub fn sever(&self) {
        self.0.severed.set(true);
    }

    /// Whether the popup left its opener's browsing context group (`sever`).
    pub fn is_severed(&self) -> bool {
        self.0.severed.get()
    }

    /// The popup page's current document (each navigation attaches the new
    /// one), through which the opener's stand-in reaches the popup.
    pub fn attach_page(&self, page: &Rc<PageDom>) {
        *self.0.page.borrow_mut() = Rc::downgrade(page);
        self.0.attached.set(true);
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
    /// A named-window navigation initiated by window.open, with its caller's
    /// referrer source and policy captured before the navigation task runs.
    OpenNamed { url: String, referrer: Option<String>, policy: pyro_net::ReferrerPolicy },
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
        if parsed.scheme() != "javascript" && !self.popup.is_closed() {
            self.page.popup_command(self.popup.clone(), PopupCommand::Navigate(resolved));
        }
        Ok(())
    }

    /// The opener's tuple origin, if it has one.
    fn opener_origin(&self) -> Option<String> {
        crate::storage::document_origin(&self.page)
    }

    /// Whether the opener may reach into the popup: their documents have the same origin, compared
    /// by identity (`PageDom::origin_key`: an opaque origin equals only itself, which an about:blank
    /// document shares with its creator), as postMessage's "/" compares them.
    fn same_origin(&self) -> bool {
        if let Some(page) = self.popup.page() {
            return page.origin_key() == self.page.origin_key();
        }
        // No document of its own (yet): only a tuple origin can be compared.
        let url = self.popup.url.borrow();
        pyro_net::Url::parse(&url).is_ok_and(|u| {
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

    /// A member of the popup's global read whatever its origin, for the
    /// cross-origin readable ones whose value the caller reduces to what
    /// may cross (a number, null or not): never handed out as it is.
    fn forward_any_origin<'s>(&self, s: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>> {
        let Some(page) = self.popup.page() else { return Ok(None) };
        let Some(realm) = page.main_world_realm() else { return Ok(None) };
        let scope = realm.scope()?;
        let value = scope.global().get(name)?;
        Ok(Some(s.rehome(value)))
    }

    /// A Window member of the popup's real global, brought into `s`'s realm.
    fn forward<'s>(&self, s: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>> {
        let Some((_, realm)) = self.reachable_page()? else { return Ok(None) };
        let scope = realm.scope()?;
        let value = scope.global().get(name)?;
        Ok(Some(s.rehome(value)))
    }

    /// helpers.js reports that `property` of the popup's window was reached (`ACCESS_NOTE`).
    fn note_access(&self, property: &str) {
        if !coop_reporting_active() {
            return;
        }
        if let Some(page) = self.popup.page() {
            note_window_access(&page, property);
        }
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
        let sender = self.page.origin_key();
        // Matched by origin key: an opaque opener's "/" reaches only a popup of that same opaque
        // origin (its initial about:blank), not one that navigated to another opaque origin.
        let (message, wanted, options) = crate::window_message::post_message_args(s, a, &sender)?;
        // Clone even when the message will be dropped (errors are reported
        // regardless of the target).
        let message = crate::structured_clone::encode_message(s, message, options)?;
        let Some(page) = self.popup.page() else { return Ok(()) };
        if !wanted.allows(&page.origin_key()) {
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
        // HTML "window post message steps": `source` is the poster's WindowProxy as the popup sees
        // it, i.e. the popup's `window.opener` (null once the opener is gone or was cleared).
        let popup = Rc::downgrade(&page);
        let source: crate::event::MessageSource =
            Rc::new(move |s: &Scope<'_>| match popup.upgrade().and_then(|p| p.hooks()) {
                Some(hooks) => hooks.opener_window(s),
                None => Ok(s.null()),
            });
        crate::message_channel::fire_wire_message_from(&scope, &page, window, &message, Some(origin), Some(source));
    }));
}

/// The opener realm's wrapper of `popup` (one per realm, as a WindowProxy).
/// The opener's WindowProxy for `popup`: one object per popup and realm, a
/// proxy (`Scope::window_proxy`) over the stand-in (`PopupWindow`, never
/// handed to page code) that follows HTML's WindowProxy internal methods:
/// same origin, every property operation goes to the popup's global;
/// cross origin, only CrossOriginProperties are there and anything else
/// throws SecurityError; while the popup has no page, the stand-in's
/// members and the proxy's own expandos answer.
pub(crate) fn wrap_popup<'s>(s: &Scope<'s>, page: &Rc<PageDom>, popup: &Rc<PopupState>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 54), Rc::as_ptr(popup) as u64);
    let stand_in = match s.lookup_identity(key) {
        Some(o) => o,
        None => s.wrap::<PopupWindow>(PopupData::new(page, popup.clone()), Some(key))?,
    };
    if let Some(proxy) = s.get_hidden(stand_in, PROXY)? {
        return Ok(proxy);
    }
    let data = PopupData::new(page, popup.clone());
    let target = s.function("", move |s, _, _| match data.reachable_page() {
        Ok(Some((_, realm))) => {
            let scope = realm.scope()?;
            Ok(s.rehome(scope.global().value()))
        }
        Ok(None) => Ok(s.null()),
        // Cross origin: the popup's origin, which what is handed out is kept by.
        Err(_) => {
            // By origin key: two different opaque origins keep different objects.
            let origin = data.popup.page().map(|p| p.origin_key());
            Ok(s.string(origin.as_deref().unwrap_or("null")))
        }
    })?;
    let children = PopupData::new(page, popup.clone());
    let child = s.function("", move |s, _, a| {
        let key = a.get(0).to_string()?;
        let Some(page) = children.popup.page() else { return Ok(s.undefined()) };
        Ok(page.hooks().map(|h| h.foreign_child_window(s, &key)).transpose()?.flatten().unwrap_or_else(|| s.undefined()))
    })?;
    let deny = s.function("", |_s, _, a| {
        let name = a.get(0).to_string().unwrap_or_default();
        Err(JsError::dom(
            "SecurityError",
            format!("Blocked a frame from accessing property \"{name}\" of a cross-origin window."),
        ))
    })?;
    let proxy = s.window_proxy(stand_in, target, child, deny)?;
    // The stand-in, kept alive by the proxy's handler, keeps the proxy: the
    // same object for every `window.open` of this popup.
    s.set_hidden(stand_in, PROXY, proxy.value())?;
    Ok(proxy.value())
}

/// The WindowProxy `opener`'s realm `s` has for the popup whose current
/// document is `popup_page` (a child window's `parent` or `top`, seen from
/// the opener), if `opener` opened it.
pub fn popup_window_proxy<'s>(s: &Scope<'s>, opener: &Rc<PageDom>, popup_page: &Rc<PageDom>) -> JsResult<Option<Value<'s>>> {
    let popup = opener.popups.borrow().iter().find(|(_, p)| p.page().is_some_and(|q| Rc::ptr_eq(&q, popup_page))).map(|(_, p)| p.clone());
    popup.map(|popup| wrap_popup(s, opener, &popup)).transpose()
}

/// The stand-in's hidden reference to its WindowProxy.
const PROXY: &str = "lpWindowProxy";

/// The object `window.open` returns. Not exposed on the global.
pub struct PopupWindow;
impl JsClass for PopupWindow {
    const NAME: &'static str = "PopupWindow";
    type Data = PopupData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.getter("closed", |s, d| Ok(s.boolean(d.popup.is_closed())));
        def.method("close", 0, |s, d, _a| {
            // `closed` reads true from here on, before the page is gone.
            // A popup in another browsing context group is a closed window to its former opener.
            if !d.popup.severed.get() && !d.popup.closed.replace(true) {
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
        for name in ["window", "self", "frames", "globalThis"] {
            def.getter(name, |s, d| wrap_popup(s, &d.page, &d.popup));
        }
        // A discarded popup has no browsing context: no parent, no top.
        for name in ["top", "parent"] {
            def.getter(name, |s, d| if d.popup.is_discarded() { Ok(s.null()) } else { wrap_popup(s, &d.page, &d.popup) });
        }
        // [Replaceable]: `popup.opener = null` (a common way to cut the
        // popup off from its opener before navigating it) must not throw in
        // strict code, and the popup's `opener` reads null from then on.
        def.accessor(
            "opener",
            |s, d| {
                if d.popup.opener_cleared.get() || d.popup.severed.get() || d.popup.is_discarded() {
                    return Ok(s.null());
                }
                // Cross origin (readable there): the popup's opener is this
                // window unless the popup cut it off.
                if d.reachable_page().is_ok() {
                    return Ok(d.forward(s, "opener")?.unwrap_or_else(|| s.global().value()));
                }
                let cut_off = d.forward_any_origin(s, "opener")?.is_some_and(|v| v.is_null());
                Ok(if cut_off { s.null() } else { s.global().value() })
            },
            |_s, d, v| {
                // Checked before any state changes (the proxy refuses a
                // cross-origin write before it gets here).
                let reachable = d.reachable_page()?;
                if v.is_null() {
                    d.popup.opener_cleared.set(true);
                }
                if let Some((_, realm)) = reachable {
                    let scope = realm.scope()?;
                    scope.global().set("opener", scope.rehome(v))?;
                }
                Ok(())
            },
        );
        // Cross origin too (CrossOriginProperties): a number of the popup's.
        def.getter("length", |s, d| {
            let length = match d.reachable_page() {
                Ok(_) => d.forward(s, "length")?,
                Err(_) => d.forward_any_origin(s, "length")?.map(|v| s.number(v.to_number().unwrap_or(0.0))),
            };
            Ok(length.unwrap_or_else(|| s.number(0.0)))
        });
        def.getter("name", |s, d| {
            // A popup that left the group lost its browsing context name.
            if d.popup.severed.get() {
                return Ok(s.string(""));
            }
            Ok(d.forward(s, "name")?.unwrap_or_else(|| s.string(&d.popup.name)))
        });
        def.getter("document", |s, d| Ok(d.forward(s, "document")?.unwrap_or_else(|| s.undefined())));
        def.getter("localStorage", |s, d| d.storage(s, false));
        def.getter("sessionStorage", |s, d| d.storage(s, true));
        def.method("fetch", 1, |s, d, a| d.fetch(s, &a));
        def.method("postMessage", 1, |s, d, a| {
            d.post_message(s, &a)?;
            Ok(s.undefined())
        });
        // Everything else reads through to the popup's global once it exists
        // (the WindowProxy, `wrap_popup`, does the rest).
        def.named(false, |s, d, name| {
            if let Some(property) = name.strip_prefix(ACCESS_NOTE) {
                d.note_access(property);
                return Ok(None);
            }
            d.forward(s, name)
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

/// A link or form of `page` targeting window name `name` (HTML "rules for choosing a navigable":
/// an existing navigable of that name), when `page` opened a popup of that name that is still open
/// (`window.open(url, name)`): that popup navigates to `url`, as `window.open` with the same name
/// does. False when there is none, and the caller opens a new window.
pub fn navigate_named_popup(page: &Rc<PageDom>, name: &str, url: &str) -> bool {
    let popup = page.popups.borrow().iter().find(|(n, p)| n.as_str() == name && !p.is_closed()).map(|(_, p)| p.clone());
    let Some(popup) = popup else { return false };
    page.popup_command(
        popup,
        PopupCommand::OpenNamed {
            url: url.to_owned(),
            referrer: page.client_referrer().map(Into::into),
            policy: page.referrer_policy(),
        },
    );
    true
}
