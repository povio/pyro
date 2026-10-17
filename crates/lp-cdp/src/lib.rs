//! Chrome DevTools Protocol server for lightningpanda.
//!
//! Protocol behavior follows Lightpanda (lightpanda-io/browser at
//! 2ad38773fd674d3c9cea26a263b814ee46f6ba84, AGPL-3.0-only) and the CDP
//! surface Playwright 1.59 drives; see `docs/automation/` for coverage.

pub mod engine;
pub mod fetch_protocol;
mod handshake_crypto;
pub mod http;
pub mod msg;
pub mod outbox;
pub mod runtime_protocol;
pub mod serve;
pub mod server;
pub mod target;
pub mod ws;

pub use serve::serve_main;
