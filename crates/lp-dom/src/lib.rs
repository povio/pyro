//! lp-dom: the DOM tree for lightningpanda, a Rust + JavaScriptCore rewrite
//! of lightpanda-io/browser (AGPL-3.0-only). Pure Rust, no JS engine
//! dependency; JS bindings live in `lp-webapi`.

pub mod dom;
pub mod error;
pub mod events;
pub mod forms;
pub mod mutation;
pub mod node;
pub mod parse;
pub mod script;
pub mod select;
pub mod serialize;

pub use dom::{position, Dom};
pub use error::{DomError, DomErrorKind, DomResult};
pub use node::{Attr, ControlState, DocumentKind, NodeId, NodeKind, NodeType, QuirksMode, ShadowRootMode};

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};
