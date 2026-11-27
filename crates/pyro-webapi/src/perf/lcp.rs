//! Largest Contentful Paint (lane perf-2). Empty until its lane fills it in: see the interface at the top of
//! `perf/mod.rs` (what is called, with what, and when).

use std::rc::Rc;

use crate::page::PageDom;

use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["largest-contentful-paint"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = false;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(_engine: &pyro_js::Engine) {}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(_page: &Rc<PageDom>, _frame: &FrameInfo) {}

