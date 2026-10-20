//! Flow layout (docs/dom/FLOW_LAYOUT.md), owned by W1. This file currently holds
//! only the Stage 1 style input contract, verbatim from W1's draft with
//! compile fixes, so the style engine can implement [`FlowStyleSource`]
//! (`style_flow.rs`). The layout algorithm and geometry switch follow in W1's
//! Stage 1 patch.

use crate::dom::Dom;
use crate::node::NodeId;

// ----- style input contract -------------------------------------------------

/// A length: `px` plus `percent` of the containing block's matching axis
/// (so `calc(100% - 8px)` is `{ px: -8, percent: 100 }`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Len {
    pub px: f64,
    pub percent: f64,
}

impl Len {
    pub const ZERO: Len = Len { px: 0.0, percent: 0.0 };
    pub fn px(px: f64) -> Len {
        Len { px, percent: 0.0 }
    }
    pub fn percent(percent: f64) -> Len {
        Len { px: 0.0, percent }
    }
    pub fn resolve(self, basis: f64) -> f64 {
        self.px + self.percent * basis / 100.0
    }
}

/// A size or offset that may be `auto` (or `none` for max sizes).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Dim {
    #[default]
    Auto,
    Len(Len),
}

impl Dim {
    pub fn px(px: f64) -> Dim {
        Dim::Len(Len::px(px))
    }
    pub fn resolve(self, basis: Option<f64>) -> Option<f64> {
        match self {
            Dim::Auto => None,
            Dim::Len(l) if l.percent != 0.0 => basis.map(|b| l.resolve(b)),
            Dim::Len(l) => Some(l.px),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum OuterDisplay {
    #[default]
    Block,
    Inline,
    /// The element generates no box; its children are laid out in its place.
    Contents,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum InnerDisplay {
    #[default]
    Flow,
    /// Establishes a new formatting context (inline-block, flow-root).
    FlowRoot,
    Flex,
    Grid,
    Table,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// top, right, bottom, left.
pub type Sides<T> = [T; 4];
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// Used style values of one rendered element.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowStyle {
    pub outer: OuterDisplay,
    pub inner: InnerDisplay,
    pub position: Position,
    pub inset: Sides<Dim>,
    pub width: Dim,
    pub height: Dim,
    pub min_width: Dim,
    pub max_width: Dim,
    pub min_height: Dim,
    pub max_height: Dim,
    pub margin: Sides<Dim>,
    pub padding: Sides<Len>,
    pub border: Sides<f64>,
    pub border_box: bool,
    pub clips_x: bool,
    pub clips_y: bool,
    pub font_size: f64,
    /// px; None = `normal`.
    pub line_height: Option<f64>,
    pub nowrap: bool,
    /// `<img>`/`<iframe>`-like replaced element: atomic, sized by its own
    /// width/height only.
    pub replaced: bool,
}

impl Default for FlowStyle {
    fn default() -> Self {
        FlowStyle {
            outer: OuterDisplay::Block,
            inner: InnerDisplay::Flow,
            position: Position::Static,
            inset: [Dim::Auto; 4],
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            max_width: Dim::Auto,
            min_height: Dim::Auto,
            max_height: Dim::Auto,
            margin: [Dim::px(0.0); 4],
            padding: [Len::ZERO; 4],
            border: [0.0; 4],
            border_box: false,
            clips_x: false,
            clips_y: false,
            font_size: 16.0,
            line_height: None,
            nowrap: false,
            replaced: false,
        }
    }
}

/// Supplies used style values. `None`: the element is not rendered.
pub trait FlowStyleSource {
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle>;
}
