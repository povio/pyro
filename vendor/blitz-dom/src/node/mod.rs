#![allow(clippy::module_inception)]

mod attributes;
#[cfg(feature = "custom-widget")]
mod custom_widget;
mod element;
mod node;
pub(crate) mod scrollbar;
mod serialize;
mod stylo_data;
#[cfg(feature = "svg")]
mod svg;
#[cfg(feature = "svg")]
pub(crate) mod svg_css;
mod text;

pub use attributes::{Attribute, Attributes, PresentationalHints};
#[cfg(feature = "custom-widget")]
pub use custom_widget::{
    ComputedStyles, CustomWidgetData, CustomWidgetStatus, ProxyRenderContext, Widget,
};
pub(crate) use element::parse_classes;
pub use element::{
    CanvasData, DocumentData, ElementData, ImageData, ImageResourceData, LayoutData,
    ListItemLayout, ListItemLayoutPosition, Marker, MathRadical, MathRadicalSign, MathStretch, MathStretchGlyph, RasterImageData, SpecialElementData,
    SpecialElementType, Status,
};
pub use node::*;
pub use scrollbar::{ClassicScrollbars, ScrollbarColor, ScrollbarRef, ScrollbarWidth};
pub use stylo_data::{ComputedStyleRef, UnvisitedPaint};
pub(crate) use stylo_data::LayoutStyles;
#[cfg(feature = "svg")]
pub use svg::{SvgImageData, SvgIntrinsicDimensions, SvgTree, svg_source_element};
#[cfg(feature = "svg")]
pub(crate) use svg::{ReferenceCandidate, embeddable_svg, reference_candidates, references_out_of_sync};
pub use text::{
    can_take_emphasis_mark, FirstLetterSplit, InitialLetter, GeneratedTextInputEvent, InputPlaceholder, TextAnnotation,
    TextAnnotationKind, TextBrush, TextInputData, TextLayout, LineClamp, LaidOutLines,
};
