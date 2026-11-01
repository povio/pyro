//! lp-dom: the DOM tree for lightningpanda, a Rust + JavaScriptCore rewrite
//! of lightpanda-io/browser (AGPL-3.0-only). Pure Rust, no JS engine
//! dependency; JS bindings live in `lp-webapi`.

pub mod computed;
pub mod dom;
pub mod error;
pub mod events;
mod flow_font;
pub mod forms;
pub mod geometry;
pub mod iframe;
pub mod image;
pub mod inner_text;
pub mod layout;
pub mod media;
pub mod mutation;
pub mod node;
pub mod parse;
pub mod script;
pub mod select;
pub mod serialize;
pub mod snapshot;
pub mod style;
pub mod stylo;
pub mod svg_geometry;
pub mod text_metrics;
pub mod xml;
pub mod xpath;

pub use dom::{attr_name_hash, position, CollectionWatch, Dom};
pub use error::{DomError, DomErrorKind, DomResult};
pub use node::{Attr, ControlState, DocumentKind, NodeId, NodeKind, NodeType, QuirksMode, ShadowRootMode};

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};
