//! `navigator.userActivation` (HTML § 6.4.4, the `UserActivation` interface): the window's
//! sticky and transient activation, read live from the page.

use std::rc::Rc;

use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, NoParent, Scope, Value};

use crate::page::PageDom;

pub struct UserActivation;
impl JsClass for UserActivation {
    const NAME: &'static str = "UserActivation";
    type Data = Rc<PageDom>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("hasBeenActive", |s, p| Ok(s.boolean(p.has_sticky_activation())));
        def.getter("isActive", |s, p| Ok(s.boolean(p.has_transient_activation())));
    }
}

/// `navigator.userActivation`, [SameObject].
pub(crate) fn user_activation<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    // Slot 11 of the page's [SameObject] keys (window.rs uses 0, 1 and 3 to 10, indexeddb 2).
    let key = IdentityKey(page.id() | (1 << 57), 11);
    crate::window::same_object(s, page, key, || s.wrap::<UserActivation>(page.clone(), Some(key)))
}
