//! Event Timing (lane perf-5). Empty until its lane fills it in: see the interface at the top of
//! `perf/mod.rs` (what is called, with what, and when).

use std::rc::Rc;

use crate::page::PageDom;

use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["event", "first-input"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = false;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(_engine: &pyro_js::Engine) {}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(_page: &Rc<PageDom>, _frame: &FrameInfo) {}

/// What [`dispatch_begin`] hands [`dispatch_end`].
pub struct DispatchToken;

/// A trusted event is about to run its listeners; called when ENABLED.
pub fn dispatch_begin(_page: &PageDom, _event: &crate::event::EventData) -> Option<DispatchToken> {
    None
}

/// The listeners of the event ran; called when ENABLED.
pub fn dispatch_end(_page: &PageDom, _event: &crate::event::EventData, _token: DispatchToken) {}

/// A trusted input event nobody listens for was not made (`perf::note_unheard`); called when
/// ENABLED. It counts like a dispatched one (`eventCounts`, entries, `interactionId`).
pub fn note_unheard(_page: &PageDom, _event: &super::UnheardEvent<'_>) {}

/// `performance.eventCounts` (an `EventCounts` object of `s`'s realm); read when ENABLED.
pub fn event_counts<'s>(s: &pyro_js::Scope<'s>, _page: &PageDom) -> pyro_js::JsResult<pyro_js::Value<'s>> {
    Ok(s.undefined())
}

/// `performance.interactionCount`; read when ENABLED.
pub fn interaction_count(_page: &PageDom) -> u32 {
    0
}

