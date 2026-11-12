//! What this crate keeps per thread, for a browser thread that serves one
//! client after another: [`reset_thread`] proves that nothing of the pages
//! the thread ran is left in it.
//!
//! Every `thread_local!` of the crate is listed in [`STATICS`] with what it
//! is; `tests/thread_state.rs` fails for one that is not. Three kinds:
//!
//! - a registry of pages, realms, ports, workers or requests: the module's
//!   `thread_leftover` drops the entries whose owner is gone and names the
//!   static when one stays. Pages are numbered across the process
//!   (`page::NEXT_PAGE`), so a stale entry could never be taken for a later
//!   page; an entry that stays means something of the old pages is alive;
//! - state of a call in progress (a flag, a slot filled and emptied within
//!   one call): must be empty outside of script, and is checked;
//! - a counter of ids handed to script or to the client: starts again at 1;
//! - `event::TIME_ORIGIN`, the thread's first use of an event time stamp,
//!   read only for a page without a time origin of its own (a detached
//!   document): it stays, and tells a page nothing but that the thread is
//!   older than the page;
//! - `worker::ENDED`, a note that a worker's end left something to collect:
//!   forgotten, since the engine it was about is dropped or recycled.

/// The crate's thread-local statics, as `module::NAME`.
pub const STATICS: &[&str] = &[
    "abort::ALGORITHM_EXCEPTION",
    "canvas::HANDED",
    "canvas::PLACEHOLDER_HANDED",
    "custom::RUNNING",
    "event::TIME_ORIGIN",
    "event_source::SOURCES",
    "fetch::PAGES",
    "formdata::CONSTRUCTING",
    "formdata::SELECTED_COORDINATE",
    "indexeddb::REALMS",
    "lib::DOM_PAGES",
    "message_channel::CHANNELS",
    "message_channel::EXPORT_DEPTH",
    "message_channel::MADE",
    "message_channel::REMOTE",
    "node::FOUND",
    "port_link::RELEASE_ACTIVE",
    "port_link::RELEASING",
    "resource_timing::PAGES",
    "service_worker::FINISHERS",
    "service_worker::VERSIONS",
    "service_worker_scope::GLUE",
    "service_worker_scope::NEXT_KEY",
    "service_worker_scope::PENDING",
    "service_worker_scope::QUERIES",
    "structured_clone::CLONE_SOURCE",
    "structured_clone::WIRE_BLOBS",
    "structured_clone::WIRE_CANVASES",
    "target::WATCHED",
    "webaudio::NEXT",
    "webaudio::PENDING",
    "websocket::NEXT",
    "websocket::SOCKETS",
    "worker::ENDED",
    "worker::HELD",
    "worker::LIVE",
    "worker::THREADS",
    "xhr::PAGES",
];

/// After every page of this thread was closed and its worker threads joined
/// ([`crate::join_stopped`]): drops what dead pages left in the crate's
/// per-thread registries and starts its id counters again. `Err` names the
/// first static that still holds something of them; the thread must then not
/// serve another client.
pub fn reset_thread() -> Result<(), &'static str> {
    let checks: [fn() -> Option<&'static str>; 20] = [
        crate::dom_pages_leftover,
        crate::abort::thread_leftover,
        crate::canvas::thread_leftover,
        crate::custom::thread_leftover,
        crate::event_source::thread_leftover,
        crate::fetch::thread_leftover,
        crate::formdata::thread_leftover,
        crate::indexeddb::thread_leftover,
        crate::message_channel::thread_leftover,
        crate::node::thread_leftover,
        crate::port_link::thread_leftover,
        crate::resource_timing::thread_leftover,
        crate::service_worker::thread_leftover,
        crate::service_worker_scope::thread_leftover,
        crate::structured_clone::thread_leftover,
        crate::target::thread_leftover,
        crate::webaudio::thread_leftover,
        crate::websocket::thread_leftover,
        crate::worker::thread_leftover,
        crate::xhr::thread_leftover,
    ];
    if let Some(name) = checks.iter().find_map(|check| check()) {
        return Err(name);
    }
    // The canvas text context holds the web fonts pages registered.
    lp_canvas::text::release_thread();
    Ok(())
}
