//! Chrome DevTools Protocol server for lightningpanda.
//!
//! Protocol behavior follows Lightpanda (lightpanda-io/browser at
//! 2ad38773fd674d3c9cea26a263b814ee46f6ba84, AGPL-3.0-only) and the CDP
//! surface Playwright 1.59 drives; see `docs/automation/` for coverage.
//!
//! The engine-independent protocol lives in `lp-cdp-protocol` and is
//! re-exported here under the same module paths.

mod conn;
pub mod engine;
pub mod fork;
pub mod lifetime;
pub mod serve;

pub use lp_cdp_protocol::{
    fetch_protocol, http, msg, network_protocol, outbox, perf, runtime_protocol, server, target, ws,
};
pub use serve::serve_main;
