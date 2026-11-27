//! Long Tasks (lane perf-6). Empty until its lane fills it in: see the interface at the top of
//! `perf/mod.rs` (what is called, with what, and when).

use std::rc::Rc;

use crate::page::PageDom;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["longtask"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = false;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(_engine: &pyro_js::Engine) {}

/// A task of the event loop ended; called when ENABLED, for every document.
pub fn on_task(_page: &Rc<PageDom>, _task: &super::TaskInfo) {}

