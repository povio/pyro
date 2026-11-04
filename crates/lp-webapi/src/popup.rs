//! The value `window.open` returns: a minimal stand-in for the popup's
//! WindowProxy.
//!
//! Each page has its own JS context and DOM, so the opener cannot reach into
//! the popup. It gets `closed`, `close()`, `focus()`/`blur()` and `location`
//! (`href`, `assign`, `replace`, `toString`, or assigning `location` itself).
//! Every other Window member is undefined rather than faked, so
//! `popup.document` fails where it is used. `location.href` reads throw
//! SecurityError when the popup's origin differs, as browsers do.
//!
//! The popup page does not exist when `window.open` returns: the embedder
//! creates it from the request, later. [`PopupState`] is what the two sides
//! share, and [`PopupTicket`] is held by whoever is responsible for the page,
//! the pending request first and then the embedder's record of the page. A
//! dropped ticket (request refused, or page gone) reads as `closed`.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::page::PageDom;

/// The opener's view of a popup.
#[derive(Debug)]
pub struct PopupState {
    closed: Cell<bool>,
    /// The popup's current URL; about:blank until its first commit.
    url: RefCell<String>,
}

/// Ownership of a popup page; see the module docs.
#[derive(Debug)]
pub struct PopupTicket(Rc<PopupState>);

impl PopupTicket {
    pub(crate) fn new() -> (Rc<Self>, Rc<PopupState>) {
        let state = Rc::new(PopupState { closed: Cell::new(false), url: RefCell::new("about:blank".to_owned()) });
        (Rc::new(Self(state.clone())), state)
    }

    pub fn is_for(&self, popup: &Rc<PopupState>) -> bool {
        Rc::ptr_eq(&self.0, popup)
    }

    /// The popup committed `url`.
    pub fn set_url(&self, url: &str) {
        *self.0.url.borrow_mut() = url.to_owned();
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
        let parsed = lp_net::Url::parse(&resolved)
            .map_err(|_| JsError::dom("SyntaxError", format!("'{url}' is not a valid URL.")))?;
        // A javascript: URL would run the opener's script in the popup's
        // document, which the popup's origin may not allow.
        if parsed.scheme() != "javascript" && !self.popup.closed.get() {
            self.page.popup_command(self.popup.clone(), PopupCommand::Navigate(resolved));
        }
        Ok(())
    }

    fn href(&self) -> JsResult<String> {
        let url = self.popup.url.borrow();
        let visible = url.as_str() == "about:blank"
            || lp_net::Url::parse(&url).is_ok_and(|u| {
                let origin = u.origin();
                origin.is_tuple() && crate::storage::document_origin(&self.page) == Some(origin.ascii_serialization())
            });
        if visible {
            Ok(url.clone())
        } else {
            Err(JsError::dom("SecurityError", "Blocked a read of a cross-origin popup's location."))
        }
    }
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
                let key = lp_js::IdentityKey(d.page.id() | (1 << 53), Rc::as_ptr(&d.popup) as u64);
                Ok(s.wrap::<PopupLocation>(d.clone(), Some(key))?.value())
            },
            |_s, d, v| d.navigate(&v.to_string()?),
        );
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

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<PopupWindow>();
    engine.register::<PopupLocation>();
}
