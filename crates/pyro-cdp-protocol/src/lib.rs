//! The CDP wire protocol behind `pyro-cdp`: WebSocket framing, HTTP discovery,
//! the socket server, message envelopes, the `Target` domain and the
//! engine-independent parts of `Network`, `Fetch` and `Runtime`.
//!
//! Kept apart from `pyro-cdp` so it does not wait for pyro-browser, pyro-dom and
//! pyro-webapi to compile. `pyro-cdp` re-exports every module here; use it from
//! there, not this crate.

pub mod fetch_protocol;
mod handshake_crypto;
pub mod http;
pub mod msg;
pub mod network_protocol;
pub mod outbox;
pub mod perf;
pub mod runtime_protocol;
pub mod server;
pub mod target;
pub mod ws;
