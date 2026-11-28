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
//! - `popup::COOP_REPORTING_DOCUMENTS`, the number of documents naming a COOP reporting endpoint:
//!   the embedder keeps it balanced as documents come and go, so the crate does not reset it;
//! - `performance::WRAP_NAVIGATION_ID`: the `navigationId` of the entry being wrapped, set and
//!   restored around one wrap call;
//! - `custom::REACTION_STACK_BASE`, `perf::NESTED_MS` and `element::LAST`: scratch values of a call
//!   or a cache keyed by the process-unique page id, rewritten before they are read (the latter two
//!   are cleared anyway);
//! - `worker::ENDED`, a note that a worker's end left something to collect:
//!   forgotten, since the engine it was about is dropped or recycled;
//! - `paint_worklet::FORCE_GRAPH_FALLBACK`, a test hook compiled only with the
//!   `test-hooks` feature (test builds, never the browser's): the test that sets
//!   it clears it. Listed in every configuration, as the source scan of
//!   `tests/thread_state.rs` sees it in every one.

/// The crate's thread-local statics, as `module::NAME`.
pub const STATICS: &[&str] = &[
    "abort::ALGORITHM_EXCEPTION",
    "association::OWNERS",
    "canvas::HANDED",
    "canvas::PLACEHOLDER_HANDED",
    "coep_report::PAGES",
    "csp::HANDLER_LOCATION",
    "custom::CONSTRUCTIONS",
    "custom::FRAMES",
    "custom::REACTION_STACK_BASE",
    "custom::RUNNING",
    "editing::SCRIPT_COMMANDS",
    "element::LAST",
    "eme::STORES",
    "event::TIME_ORIGIN",
    "event_source::SOURCES",
    "fetch::PAGES",
    "formdata::CONSTRUCTING",
    "formdata::SELECTED_COORDINATE",
    "indexeddb::REALMS",
    "input::FOCUS_VISIBLE_OPTION",
    "input::LAST_PRESS",
    "input::TRUSTED_ACTIVATION",
    "lcp::STATES",
    "lib::DOM_PAGES",
    "message_channel::CHANNELS",
    "message_channel::EXPORT_DEPTH",
    "message_channel::MADE",
    "message_channel::REMOTE",
    "node::FOUND",
    "page::PENDING_FRAME_RESIZES",
    "paint_worklet::FORCE_GRAPH_FALLBACK",
    "perf::NESTED_MS",
    "performance::WRAP_NAVIGATION_ID",
    "pointer_capture::TOUCH_CAPTURE",
    "popover::CANCELING",
    "popup::COOP_REPORTING_DOCUMENTS",
    "port_link::RELEASE_ACTIVE",
    "port_link::RELEASING",
    "public_key::PENDING",
    "public_key::QUEUE",
    "resource_timing::PAGES",
    "sensors::PUMPS",
    "sensors::WATCHERS",
    "service_worker::FINISHERS",
    "service_worker::VERSIONS",
    "service_worker_scope::GLUE",
    "service_worker_scope::NEXT_KEY",
    "service_worker_scope::PENDING",
    "service_worker_scope::PRELOADS",
    "service_worker_scope::QUERIES",
    "shared_worker::BY_ID",
    "shared_worker::WORKERS",
    "shift::STATES",
    "smooth_scroll::RUNNING",
    "softnav::STATES",
    "structured_clone::CLONE_SOURCE",
    "structured_clone::WIRE_BLOBS",
    "structured_clone::WIRE_CANVASES",
    "structured_clone::WIRE_CODECS",
    "structured_clone::WIRE_HANDLES",
    "target::WATCHED",
    "web_locks::CLIENTS",
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
    let checks: [fn() -> Option<&'static str>; 34] = [
        crate::dom_pages_leftover,
        crate::abort::thread_leftover,
        crate::coep_report::thread_leftover,
        crate::eme::thread_leftover,
        crate::perf::thread_leftover,
        crate::pointer_capture::thread_leftover,
        crate::sensors::thread_leftover,
        crate::canvas::thread_leftover,
        crate::csp::thread_leftover,
        crate::custom::thread_leftover,
        crate::editing::thread_leftover,
        crate::event_source::thread_leftover,
        crate::fetch::thread_leftover,
        crate::formdata::thread_leftover,
        crate::indexeddb::thread_leftover,
        crate::input::thread_leftover,
        crate::message_channel::thread_leftover,
        crate::web_locks::thread_leftover,
        crate::node::thread_leftover,
        crate::page::thread_leftover,
        crate::popover::thread_leftover,
        crate::port_link::thread_leftover,
        crate::crypto_subtle::public_key::thread_leftover,
        crate::resource_timing::thread_leftover,
        crate::service_worker::thread_leftover,
        crate::service_worker_scope::thread_leftover,
        crate::shared_worker::thread_leftover,
        crate::smooth_scroll::thread_leftover,
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
    pyro_canvas::text::release_thread();
    Ok(())
}
