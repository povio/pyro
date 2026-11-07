//! lp-dom: the DOM tree for lightningpanda, a Rust + JavaScriptCore rewrite
//! of lightpanda-io/browser (AGPL-3.0-only). Pure Rust, no JS engine
//! dependency; JS bindings live in `lp-webapi`.

mod attr_node;
pub mod computed;
pub mod direction;
pub mod dom;
pub mod error;
pub mod events;
mod flow_font;
mod focus_nav;
pub mod forms;
pub(crate) mod gecko_only;
pub mod geometry;
pub mod iframe;
mod window_names;
pub mod image;
pub mod cssom;
pub mod custom;
mod internals;
mod moving;
mod nonce;
pub mod picture;
mod details;
mod top_layer;
pub mod inner_text;
pub mod layout;
pub mod live_iterator;
pub mod live_range;
pub mod media;
pub mod media_element;
pub mod mutation;
pub mod node;
pub mod parse;
pub mod script;
pub mod select;
mod select_query;
pub mod serialize;
mod slots;
pub mod snapshot;
pub mod style;
pub mod stylo;
pub mod svg_geometry;
pub mod text_metrics;
pub mod upgrade;
pub mod timing;
mod tree_scope;
pub mod xml;
pub mod xpath;

pub use dom::{attr_name_hash, position, CollectionWatch, Dom};
pub use top_layer::PopoverStack;
pub use window_names::window_name_hash;
pub use error::{DomError, DomErrorKind, DomResult};
pub use node::{
    shadow_flags, Atom, Attr, AttrNodeData, ControlState, DocumentKind, NodeId, NodeKind, NodeType, QuirksMode, ShadowRootMode,
};

pub use markup5ever::{LocalName, Namespace, Prefix, QualName};
