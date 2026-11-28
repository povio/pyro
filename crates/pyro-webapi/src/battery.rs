//! Battery Status (https://w3c.github.io/battery/): `navigator.getBattery()` and `BatteryManager` on a window,
//! made in JS (battery.js) on a realm's first use and secure-context only. Chrome enforces no `battery` permissions
//! policy (its run of battery-disabled-by-permissions-policy and battery-disallowed-in-cross-origin-iframe
//! fails), so none is applied here either.

use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsResult, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

static MODULE: LazyModule = LazyModule {
    key: "lpBattery",
    source: include_str!("battery.js"),
    url: "pyro://internal/battery.js",
    natives: None,
};

/// `BatteryManager` on a window global (`[SecureContext]`: absent elsewhere).
pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: crate::window::WindowHost,
{
    def.global_value_accessor(
        "BatteryManager",
        |s, w| {
            let page = crate::window::WindowHost::page(w);
            if !crate::secure_context::secure(s, page) {
                return Ok(s.undefined());
            }
            MODULE.global(s, page, "BatteryManager")
        },
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "BatteryManager", value),
    );
}

/// `navigator.getBattery()`: the realm's one promise. Chrome returns `undefined` rather than a promise from a
/// navigator whose window is closed or whose frame was removed (WPT no-leak-on-detached-use).
pub(crate) fn get_battery<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() || !page.is_fully_active() {
        return Ok(s.undefined());
    }
    MODULE.helper(s, page, "getBattery", &[])
}
