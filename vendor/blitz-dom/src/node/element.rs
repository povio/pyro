use blitz_traits::node_id::NodeId;
use cssparser::Parser;
use kurbo::{Affine, Rect as KurboRect};
use linebender_resource_handle::Blob;
use markup5ever::{LocalName, QualName, local_name};
use selectors::matching::{ElementSelectorFlags, QuirksMode};
use std::cell::Cell;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use style::Atom;
use style::parser::ParserContext;
use style::properties::{Importance, PropertyDeclaration, PropertyId, SourcePropertyDeclaration};
use style::stylesheets::{DocumentStyleSheet, Origin, UrlExtraData};
use style::values::computed::Display as StyloDisplay;
use style::{
    properties::{PropertyDeclarationBlock, parse_style_attribute},
    servo_arc::Arc as ServoArc,
    shared_lock::{Locked, SharedRwLock},
    stylesheets::CssRuleType,
};
use style_dom::ElementState;
use style_traits::ParsingMode;
use taffy::{Cache, prelude::Layout};
use thin_vec::ThinVec;
use url::Url;

use super::stylo_data::StyloData;
#[cfg(feature = "svg")]
use super::svg::SvgImageData;
use super::{Attribute, Attributes};
use crate::Document;
use crate::layout::table::TableContext;
use crate::node::{TextBrush, TextInputData, TextLayout};

#[cfg(feature = "custom-widget")]
use super::custom_widget::CustomWidgetData;

macro_rules! local_names {
    ($($name:tt),+) => {
        [$(local_name!($name),)+]
    };
}

pub(crate) fn parse_classes(value: &str) -> ThinVec<Atom> {
    value.split_ascii_whitespace().map(Atom::from).collect()
}

/// pyro: sampled values at the animations and transitions levels.
pub type SampledBlocks = (
    Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
);

/// pyro: what a MathML radical (`msqrt`, `mroot`) paints beside its content, as its last layout
/// placed it (`layout::math::radical`; drawn by blitz-paint). Lengths are CSS px; the horizontal
/// ones from the content box's start edge (its end edge when `rtl`), the vertical ones from the
/// border box's top.
#[derive(Clone)]
pub struct MathRadical {
    pub rtl: bool,
    /// The overbar over the radicand: where it starts, its width, top and thickness.
    pub bar_start: f32,
    pub bar_width: f32,
    pub bar_top: f32,
    pub bar_thickness: f32,
    /// The radical sign, when the box's font has the glyph.
    pub sign: Option<MathRadicalSign>,
}

/// pyro: the glyphs of a stretched or large MathML operator: a size variant of its character or the
/// parts of a glyph assembly, painted in place of its text (`layout::math::operator`, drawn by
/// blitz-paint). Lengths are CSS px.
#[derive(Clone)]
pub struct MathStretch {
    pub font: linebender_resource_handle::FontData,
    /// The font size.
    pub size: f32,
    /// Each glyph, its id, the offset of its origin from the content box's start edge, and the
    /// offset of its baseline from the border box's top.
    pub glyphs: Vec<MathStretchGlyph>,
}

/// A glyph of a [`MathStretch`].
#[derive(Clone, Copy)]
pub struct MathStretchGlyph {
    pub glyph: u16,
    pub x: f32,
    pub baseline: f32,
}

/// The glyph of a [`MathRadical`]'s sign.
#[derive(Clone)]
pub struct MathRadicalSign {
    pub font: linebender_resource_handle::FontData,
    pub glyph: u16,
    /// The font size.
    pub size: f32,
    /// Where the glyph's origin is: its start edge's offset, and the baseline's.
    pub start: f32,
    /// The glyph's advance width.
    pub width: f32,
    pub baseline: f32,
}

pub struct ElementData {
    /// The elements tag name, namespace and prefix
    pub name: QualName,

    /// The elements id attribute parsed as an atom (if it has one)
    pub id: Option<Atom>,

    /// The elements class attribute split into atoms (empty if it has none).
    /// Kept in sync with `attrs` by `ElementData::new` and the mutator's
    /// `set_attribute`/`remove_attribute`, so selector matching can compare
    /// atoms instead of re-splitting and re-interning the attribute string.
    pub classes: ThinVec<Atom>,

    /// The element's attributes
    pub attrs: Attributes,

    /// Whether the element is focussable
    pub is_focussable: bool,

    /// The element's parsed style attribute (used by stylo)
    pub style_attribute: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,

    /// Heterogeneous data that depends on the element's type.
    /// For example:
    ///   - The image data for \<img\> elements.
    ///   - The parley Layout for inline roots.
    ///   - The text editor for input/textarea elements
    pub special_data: SpecialElementData,

    pub background_images: ThinVec<Option<ImageResourceData>>,

    pub mask_images: ThinVec<Option<ImageResourceData>>,

    /// pyro: the `border-image-source` image (at most one entry).
    pub border_images: ThinVec<Option<ImageResourceData>>,
    /// pyro: the SVG documents this element references elements of by `url(document.svg#id)`:
    /// its computed `clip-path` and `filter` functions, then (an SVG element's) `filter` and
    /// `mask` attributes, one entry each in that order ([`super::svg::reference_candidates`]).
    /// A reference into this document has an entry without an image. `mask-image` documents
    /// are in `mask_images`.
    pub reference_images: ThinVec<Option<ImageResourceData>>,
    /// pyro: the `list-style-image` image of a list item (a list of one), its marker once loaded.
    pub list_style_images: ThinVec<Option<ImageResourceData>>,
    /// pyro: the `shape-outside` image of a float (a list of one), its float area once loaded.
    pub shape_images: ThinVec<Option<ImageResourceData>>,
    /// pyro: the image this box shows in place of content and its pixels per CSS px: the URL
    /// of a generated `img` box (`content: url()` of `::before`/`::after`), or of an element
    /// whose own `content` is one image (it is laid out as an `<img>` of it).
    pub content_image: Option<(String, f32)>,
    pub(crate) source_image_request: Option<Box<(String, blitz_traits::net::FetchOptions)>>,
    /// pyro: an `<img>` whose image failed to load (or has no source) and that has non-empty
    /// `alt` text: it represents its alt text, laid out as a non-replaced box holding it
    /// (HTML rendering § images; `BaseDocument::show_image_alt_text`).
    pub image_broken: bool,
    /// pyro: an `<img>` whose image failed to load (whatever its `alt`): painted with Chromium's
    /// broken-image icon (and, when it has no alt text, a border round its box).
    pub image_failed: bool,
    /// pyro: this box (an element or a `::before`/`::after`) shows the gradient its `content` is
    /// in place of content, as an image without a natural size (`util::content_gradient`).
    pub content_gradient: bool,
    /// pyro: an ordinary MathML fraction's bar, as its last layout placed it: its top (from the
    /// border box's top) and thickness, CSS px (`layout::math`). `None` without one.
    pub math_fraction_bar: Option<[f32; 2]>,
    /// pyro: a MathML radical's bar and sign, as its last layout placed them (`layout::math::radical`).
    pub math_radical: Option<Box<MathRadical>>,
    /// pyro: the glyphs a stretched or large MathML operator paints in place of its text, as its
    /// last layout placed them (`layout::math::operator`). `None` for a text operator.
    pub math_stretch: Option<MathStretch>,
    /// pyro: the class of a MathML box built with a layout of its own (`layout::math`, set with
    /// `NodeFlags::IS_MATH_BOX` by its construction); `None` for any other element.
    pub(crate) math_kind: Option<crate::layout::math::MathKind>,
    /// pyro: a MathML box's italic correction as its last layout set it, CSS px (`layout::math`).
    pub(crate) math_italic_correction: f32,

    /// Parley text layout (elements with inline inner display mode only)
    pub inline_layout_data: Option<Box<TextLayout>>,

    /// Data associated with display: list-item. Note that this display mode
    /// does not exclude inline_layout_data
    pub list_item_data: Option<Box<ListItemLayout>>,

    /// The element's template contents (\<template\> elements only)
    pub template_contents: Option<NodeId>,
    // /// Whether the node is a [HTML integration point] (https://html.spec.whatwg.org/multipage/#html-integration-point)
    // pub mathml_annotation_xml_integration_point: bool,

    // ---------------------------------------------------------------------
    // Fields moved from `Node`. These live on the element data so that the
    // `Node` struct itself only carries tree-structure information.
    // ---------------------------------------------------------------------
    /// Style data from stylo, plus a lock guard that allows access to it.
    pub stylo_element_data: StyloData,
    pub selector_flags: Cell<ElementSelectorFlags>,
    /// A clone of the document's shared style lock. Set when the owning
    /// [`Node`](super::Node) is constructed.
    pub guard: Option<SharedRwLock>,
    pub element_state: ElementState,
    pub has_snapshot: bool,
    pub snapshot_handled: AtomicBool,
    /// Whether any descendant of this node needs restyling.
    /// Used by Stylo's incremental style traversal to skip unchanged subtrees.
    pub dirty_descendants: AtomicBool,
    /// Whether this node or any of its descendants may carry `RestyleDamage`.
    /// Used by the damage propagation pass to skip unchanged subtrees.
    pub damaged_descendants: AtomicBool,
    /// Whether an animation or transition set has ever been stored for this
    /// element (or its pseudo-elements). Lets Stylo skip the animation lookups
    /// for elements that have never animated.
    pub may_have_animations: AtomicBool,
    /// pyro: the animated values the embedder samples into this element's
    /// cascade at the animations and transitions levels (`animation.rs`).
    pub sampled_animations: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    /// pyro: the sampled values of the animations on the element's `::marker`
    /// (`ElementEffects::marker`): cascaded onto the marker's style (`layout::list`).
    pub sampled_marker: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    /// pyro: the values of the animations in effect on the element's `::backdrop` (sampled by
    /// `BaseDocument::sample_animations`), which paint cascades on the backdrop's style.
    pub sampled_backdrop: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    /// pyro: the sampled values of the effects on the element's `::before` (`[0]`) and `::after`
    /// (`[1]`): at the animations level (`.0`) and the transitions level (`.1`) of the pseudo's
    /// cascade (`animation::cascade_sampled_pseudos`).
    pub sampled_pseudos: [SampledBlocks; 2],
    pub sampled_transitions: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
    /// pyro: an animation of this element that is current or in effect animates a property that
    /// makes a stacking context, which makes the element one (`animation.rs`).
    pub animation_stacking_context: bool,

    // Pseudo element nodes
    pub before: Option<NodeId>,
    pub after: Option<NodeId>,
    /// pyro: the node holding this element's `::first-letter` style, the brush of its first
    /// letter in the inline layout that holds it (`layout::first_letter`). Not in the tree: no
    /// children, no damage, its parent is the letter's text's parent.
    pub first_letter: Option<NodeId>,
    /// pyro: the node holding this element's `::first-line` style, whose parent is the inline
    /// root holding the first line it styles (`layout::first_line`). Not in the tree.
    pub first_line: Option<NodeId>,
    /// pyro: the node holding this list item's `::marker` style, the brush of its marker's text
    /// (`layout::list`). Not in the tree; its parent is the item.
    pub marker_style: Option<NodeId>,
    /// pyro: the box of this list item's inside marker when its `::marker` style combines it in a
    /// vertical line (`text-combine-upright: all`, `layout::combined_text`). Not in the tree.
    pub marker_combined_box: Option<NodeId>,
    /// pyro: the element's scroll pseudo-element boxes (its `::scroll-marker-group`,
    /// `::scroll-button()`s and `::scroll-marker`), or what pseudo-element box this node is
    /// (`layout::scroll_markers`).
    pub scroll: Option<Box<crate::layout::scroll_markers::ScrollPseudos>>,

    /// Detailed layout information from the most recent layout (currently
    /// grid track sizing information for grid containers only). Used by
    /// devtools grid inspection and out-of-flow grid-area positioning.
    pub detailed_layout_info: taffy::DetailedLayoutInfo<Atom>,

    // Taffy layout data:
    pub display_constructed_as: StyloDisplay,
    /// Layout output state (`None` until layout first writes to this node).
    pub layout_data: Option<Box<LayoutData>>,
    pub transform: Option<Box<Affine>>,
    /// pyro (lane H2): the plane paint draws the element in through its 4x4 matrix, when it is
    /// one (`Node::set_transform`).
    pub plane_3d: Option<Box<super::Plane3D>>,
}

/// Taffy layout output state (cache, layouts, scroll offset and overflow).
///
/// Lazily boxed on [`ElementData`] / [`DocumentData`]: inline-level elements
/// are positioned by the inline (parley) layout rather than by taffy, so most
/// nodes on text-heavy pages never allocate one.
#[derive(Debug, Clone)]
pub struct LayoutData {
    pub cache: Cache,
    pub(crate) calc_size_inputs: [Option<(Option<f32>, Option<f32>, f32, Option<f32>)>; 2],
    pub unrounded_layout: Layout,
    pub final_layout: Layout,
    pub(crate) scrollbar_size_limit: Option<taffy::Size<f32>>,
    pub scroll_offset: crate::Point<f64>,
    /// pyro: how far a `position: sticky` box is moved from where it is laid out (CSS px), with
    /// its descendants: the layout places it as if static, its embedder knows the scroll
    /// positions and sets this (`Node::set_sticky_offset`). Painting applies it.
    pub sticky_offset: crate::Point<f64>,
    pub scrollable_overflow: KurboRect,
    /// pyro: a bound of what hit testing finds in this box's subtree
    /// (device px, its own coordinates, like `scrollable_overflow`): the
    /// scrollable overflow with the boxes and lines of no area that it drops
    /// (a `line-height: 0` line, a box of no width), and the glyph runs' content
    /// areas reaching past the line boxes (`BaseDocument::resolve_hit_overflow`).
    /// Brought up to date by the first hit test after a resolve
    /// (`BaseDocument::ensure_hit_overflow`).
    pub hit_overflow: KurboRect,
    /// pyro: a transform pass computed this box again since its
    /// `hit_overflow` was (`BaseDocument::resolve_transforms`).
    pub hit_stale: bool,
    /// The unrounded layout and the fractional part of the parent's absolute position the final
    /// layout was last rounded from (incremental rounding: `RoundTree::rounded_unchanged`).
    pub rounded_from: Option<(Layout, f32, f32)>,
    /// pyro: the half collapsed borders of a cell in a table without uniform ones (top,
    /// right, bottom, left; see `CollapsedEdges`).
    pub collapsed_border: Option<[f32; 4]>,
    /// pyro: a table cell's index in its table's `TableContext::cells` (`u32::MAX`: none).
    pub table_cell_index: u32,
    /// pyro: how far down a table cell's content is laid out in its box: its row's baseline less its
    /// own, for a cell aligned on its row's baseline (`NodeFlags::HAS_TABLE_BASELINE_OFFSET`).
    pub table_baseline_offset: f32,
    /// pyro: a multi-column container's columns, from its last layout (`layout::multicol`). Read
    /// through `Node::column_fragments`, which ignores them once the box's style makes it no
    /// multi-column container.
    pub columns: Option<Box<crate::layout::multicol::ColumnFragments>>,
    /// pyro: what this node's parent grid last gave it as a subgrid (css-grid-2 §9; Taffy's
    /// `LayoutGridContainer::set_subgrid_context`): its tracks there, their line names and sizes.
    pub subgrid: Option<Box<taffy::SubgridContext<style::Atom>>>,
    /// pyro: the last remembered size of its content box (CSS px), logical and per axis
    /// (`[inline, block]`): the size it had at the last rendering update (before its
    /// ResizeObserver callbacks) in each axis whose `contain-intrinsic-size` has `auto`, while it
    /// did not skip its contents. What that `auto` sizes it as while its contents are skipped
    /// (CSS Sizing 4 § 5.1, `BaseDocument::record_remembered_sizes`).
    pub remembered_size: [Option<f32>; 2],
    /// pyro: the border-box size the scope of an element-scoped view transition keeps until its
    /// update callback is done (physical and unrounded, CSS px):
    /// its size from before the transition (pyro-dom's
    /// `view-transition-contain` state token; css-view-transitions-2 size containment with the
    /// last remembered size, which its computed style does not show).
    pub view_transition_size: Option<taffy::Size<f32>>,
    /// The writing mode this node's layout algorithm ran in (the root takes it from `<body>`),
    /// recorded by the layout pass for CSSOM geometry lookups.
    #[cfg(feature = "writing-mode")]
    pub writing_mode: stylo_taffy::WritingMode,
}

impl LayoutData {
    pub const fn new() -> Self {
        Self {
            cache: Cache::new(),
            calc_size_inputs: [None; 2],
            unrounded_layout: Layout::new(),
            scrollbar_size_limit: None,
            final_layout: Layout::new(),
            scroll_offset: crate::Point::ZERO,
            sticky_offset: crate::Point::ZERO,
            scrollable_overflow: KurboRect::ZERO,
            hit_overflow: KurboRect::ZERO,
            hit_stale: true,
            rounded_from: None,
            collapsed_border: None,
            table_cell_index: u32::MAX,
            table_baseline_offset: 0.0,
            columns: None,
            subgrid: None,
            remembered_size: [None; 2],
            view_transition_size: None,
            #[cfg(feature = "writing-mode")]
            writing_mode: stylo_taffy::WritingMode::empty(),
        }
    }
}

impl Default for LayoutData {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared read-only default returned by accessors for nodes whose
/// [`LayoutData`] has never been allocated.
pub(crate) static DEFAULT_LAYOUT_DATA: LayoutData = LayoutData::new();

impl ElementData {
    /// This element's layout output state, or a shared default if layout has
    /// never written to this element.
    #[inline]
    pub fn layout_data(&self) -> &LayoutData {
        self.layout_data.as_deref().unwrap_or(&DEFAULT_LAYOUT_DATA)
    }

    /// Mutable access to this element's layout output state, allocating it if
    /// it does not yet exist.
    #[inline]
    pub fn layout_data_mut(&mut self) -> &mut LayoutData {
        self.layout_data.get_or_insert_default()
    }
}

impl DocumentData {
    /// The document node's layout output state, or a shared default if layout
    /// has never written to it.
    #[inline]
    pub fn layout_data(&self) -> &LayoutData {
        self.layout_data.as_deref().unwrap_or(&DEFAULT_LAYOUT_DATA)
    }

    /// Mutable access to the document node's layout output state, allocating
    /// it if it does not yet exist.
    #[inline]
    pub fn layout_data_mut(&mut self) -> &mut LayoutData {
        self.layout_data.get_or_insert_default()
    }
}

/// Data specific to the [`Document`](super::super::Document) root node.
///
/// The document node participates in layout and styling like an element, so it
/// carries the same style/layout fields that were previously stored directly on
/// [`Node`](super::Node).
pub struct DocumentData {
    pub stylo_element_data: StyloData,
    /// Selector flags deposited here by `apply_selector_flags` when a
    /// `for_parent()` flag is applied while matching the root `<html>` element,
    /// whose parent node is the document.
    pub selector_flags: Cell<ElementSelectorFlags>,
    /// A clone of the document's shared style lock. Set when the owning
    /// [`Node`](super::Node) is constructed.
    pub guard: Option<SharedRwLock>,
    pub dirty_descendants: AtomicBool,
    pub damaged_descendants: AtomicBool,
    pub element_state: ElementState,
    pub has_snapshot: bool,
    pub snapshot_handled: AtomicBool,
    pub display_constructed_as: StyloDisplay,
    /// Layout output state (`None` until layout first writes to this node).
    pub layout_data: Option<Box<LayoutData>>,
    pub transform: Option<Box<Affine>>,
}

// Hand-written like `ElementData`'s, because `ElementSelectorFlags` does not
// implement `Debug`. Every other field is still reported.
impl std::fmt::Debug for DocumentData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentData")
            .field("stylo_element_data", &self.stylo_element_data)
            .field("guard", &self.guard)
            .field("dirty_descendants", &self.dirty_descendants)
            .field("damaged_descendants", &self.damaged_descendants)
            .field("element_state", &self.element_state)
            .field("has_snapshot", &self.has_snapshot)
            .field("snapshot_handled", &self.snapshot_handled)
            .field("display_constructed_as", &self.display_constructed_as)
            .field("layout_data", &self.layout_data)
            .field("transform", &self.transform)
            .finish_non_exhaustive()
    }
}

impl DocumentData {
    pub fn new() -> Self {
        Self {
            stylo_element_data: Default::default(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            guard: None,
            dirty_descendants: AtomicBool::new(true),
            damaged_descendants: AtomicBool::new(true),
            element_state: ElementState::empty(),
            has_snapshot: false,
            snapshot_handled: AtomicBool::new(false),
            display_constructed_as: StyloDisplay::Block,
            layout_data: None,
            transform: None,
        }
    }
}

impl Default for DocumentData {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for DocumentData {
    fn clone(&self) -> Self {
        // Runtime style/layout state is reset (the document node is not
        // meaningfully cloneable), matching `ElementData`'s clone semantics.
        Self {
            guard: self.guard.clone(),
            ..Self::new()
        }
    }
}

impl std::fmt::Debug for ElementData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementData")
            .field("name", &self.name)
            .field("id", &self.id)
            .field("attrs", &self.attrs)
            .field("is_focussable", &self.is_focussable)
            .field("style_attribute", &self.style_attribute)
            .field("special_data", &self.special_data)
            .field("background_images", &self.background_images)
            .field("mask_images", &self.mask_images)
            .field("border_images", &self.border_images)
            .field("list_style_images", &self.list_style_images)
            .field("shape_images", &self.shape_images)
            .field("reference_images", &self.reference_images)
            .field("inline_layout_data", &self.inline_layout_data)
            .field("list_item_data", &self.list_item_data)
            .field("template_contents", &self.template_contents)
            .field("element_state", &self.element_state)
            .field("display_constructed_as", &self.display_constructed_as)
            .finish_non_exhaustive()
    }
}

impl Clone for ElementData {
    /// Clones the *content* of the element (name, attributes, style attribute,
    /// special data, etc.). Runtime style/layout state (stylo data, taffy
    /// layout, caches, pseudo-element ids, ...) is reset to its default so that
    /// the clone behaves like a freshly-created element that has not yet been
    /// styled or laid out.
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            id: self.id.clone(),
            classes: self.classes.clone(),
            attrs: self.attrs.clone(),
            is_focussable: self.is_focussable,
            style_attribute: self.style_attribute.clone(),
            special_data: self.special_data.clone(),
            background_images: self.background_images.clone(),
            mask_images: self.mask_images.clone(),
            border_images: self.border_images.clone(),
            list_style_images: self.list_style_images.clone(),
            shape_images: self.shape_images.clone(),
            content_image: self.content_image.clone(),
            source_image_request: self.source_image_request.clone(),
            image_broken: self.image_broken,
            image_failed: self.image_failed,
            content_gradient: self.content_gradient,
            math_fraction_bar: None,
            math_radical: None,
            math_stretch: None,
            math_kind: None,
            math_italic_correction: 0.0,
            reference_images: self.reference_images.clone(),
            inline_layout_data: self.inline_layout_data.clone(),
            list_item_data: self.list_item_data.clone(),
            template_contents: self.template_contents,

            // Runtime state: reset to defaults.
            stylo_element_data: Default::default(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            guard: self.guard.clone(),
            element_state: self.element_state,
            has_snapshot: false,
            snapshot_handled: AtomicBool::new(false),
            dirty_descendants: AtomicBool::new(true),
            damaged_descendants: AtomicBool::new(true),
            may_have_animations: AtomicBool::new(false),
            sampled_animations: None,
            sampled_marker: None,
            sampled_backdrop: None,
            marker_combined_box: None,
            sampled_pseudos: Default::default(),
            sampled_transitions: None,
            animation_stacking_context: false,
            before: None,
            after: None,
            first_letter: None,
            first_line: None,
            marker_style: None,
            scroll: None,
            detailed_layout_info: taffy::DetailedLayoutInfo::None,
            display_constructed_as: StyloDisplay::Block,
            layout_data: None,
            transform: None,
            plane_3d: None,
        }
    }
}

#[derive(Copy, Clone, Default)]
#[non_exhaustive]
pub enum SpecialElementType {
    Stylesheet,
    Image,
    Canvas,
    TableRoot,
    TextInput,
    CheckboxInput,
    #[cfg(feature = "file-input")]
    FileInput,
    #[default]
    None,
}

/// Heterogeneous data that depends on the element's type.
#[derive(Default)]
pub enum SpecialElementData {
    /// A sub-document such an \<iframe\> or \<web-view\> element
    SubDocument(Box<dyn Document>),
    /// A custom widget
    #[cfg(feature = "custom-widget")]
    CustomWidget(CustomWidgetData),
    /// A stylesheet
    Stylesheet(DocumentStyleSheet),
    /// An \<img\> element's image data
    Image(Box<ImageData>),
    /// A \<canvas\> element's custom paint source
    Canvas(CanvasData),
    /// Pre-computed table layout data
    TableRoot(Arc<TableContext>),
    /// Parley text editor (text inputs)
    TextInput(TextInputData),
    /// Checkbox checked state
    CheckboxInput(bool),
    /// Selected files
    #[cfg(feature = "file-input")]
    FileInput(FileData),
    /// No data (for nodes that don't need any node-specific data)
    #[default]
    None,
}

impl Clone for SpecialElementData {
    fn clone(&self) -> Self {
        match self {
            Self::SubDocument(_) => Self::None, // TODO
            #[cfg(feature = "custom-widget")]
            Self::CustomWidget(_) => Self::None, // TODO
            Self::Stylesheet(data) => Self::Stylesheet(data.clone()),
            Self::Image(data) => Self::Image(data.clone()),
            Self::Canvas(data) => Self::Canvas(data.clone()),
            Self::TableRoot(data) => Self::TableRoot(data.clone()),
            Self::TextInput(data) => Self::TextInput(data.clone()),
            Self::CheckboxInput(data) => Self::CheckboxInput(*data),
            #[cfg(feature = "file-input")]
            Self::FileInput(data) => Self::FileInput(data.clone()),
            Self::None => Self::None,
        }
    }
}

impl SpecialElementData {
    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

impl ElementData {
    pub fn new(name: QualName, attrs: Vec<Attribute>) -> Self {
        let id_attr_atom = attrs
            .iter()
            .find(|attr| &attr.name.local == "id" && attr.name.ns == markup5ever::ns!())
            .map(|attr| attr.value.as_ref())
            .map(|value: &str| Atom::from(value));
        let classes = attrs
            .iter()
            // pyro: only the `class` attribute in no namespace gives classes (an XHTML-namespaced
            // `xhtml:class` does not; WPT css/selectors/old-tests `css3-modsel-171`).
            .find(|attr| &attr.name.local == "class" && attr.name.ns == markup5ever::ns!())
            .map(|attr| parse_classes(&attr.value))
            .unwrap_or_default();

        let mut data = ElementData {
            name,
            id: id_attr_atom,
            classes,
            attrs: Attributes::new(attrs),
            is_focussable: false,
            style_attribute: Default::default(),
            inline_layout_data: None,
            list_item_data: None,
            special_data: SpecialElementData::None,
            template_contents: None,
            background_images: ThinVec::new(),
            mask_images: ThinVec::new(),
            border_images: ThinVec::new(),
            list_style_images: ThinVec::new(),
            shape_images: ThinVec::new(),
            content_image: None,
            source_image_request: None,
            image_broken: false,
            image_failed: false,
            content_gradient: false,
            math_fraction_bar: None,
            math_radical: None,
            math_stretch: None,
            math_kind: None,
            math_italic_correction: 0.0,
            reference_images: ThinVec::new(),

            stylo_element_data: Default::default(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            guard: None,
            element_state: ElementState::empty(),
            has_snapshot: false,
            snapshot_handled: AtomicBool::new(false),
            dirty_descendants: AtomicBool::new(true),
            damaged_descendants: AtomicBool::new(true),
            may_have_animations: AtomicBool::new(false),
            sampled_animations: None,
            sampled_marker: None,
            sampled_backdrop: None,
            marker_combined_box: None,
            sampled_pseudos: Default::default(),
            sampled_transitions: None,
            animation_stacking_context: false,
            before: None,
            after: None,
            first_letter: None,
            first_line: None,
            marker_style: None,
            scroll: None,
            detailed_layout_info: taffy::DetailedLayoutInfo::None,
            display_constructed_as: StyloDisplay::Block,
            layout_data: None,
            transform: None,
            plane_3d: None,
        };
        data.flush_is_focussable();
        data.flush_link_state();

        // Mirror the `checked` attribute into the element state so that `:checked`
        // selectors can be matched (and invalidated) from `ElementState`.
        if data.name.local == local_name!("input")
            && matches!(
                data.attr(local_name!("type")),
                Some("checkbox") | Some("radio")
            )
            && data.has_attr(local_name!("checked"))
        {
            data.element_state.insert(ElementState::CHECKED);
        }
        // pyro: a selected `<option>` is `:checked` (pyro-dom's render
        // tree writes `selected` for the current selectedness), and the form
        // states pyro-dom mirrors as `urn:x-pyro:state` tokens are
        // element states, so Stylo's state invalidation (and `:has()`'s)
        // follows them.
        data.sync_lp_attribute_state();

        // The element state needs to be modified if the element can be disabled.
        if data.can_be_disabled() {
            data.element_state
                .insert(match data.has_attr(local_name!("disabled")) {
                    true => ElementState::DISABLED,
                    false => ElementState::ENABLED,
                });
        }

        data.flush_open_state();

        data
    }

    pub fn attrs(&self) -> &[Attribute] {
        &self.attrs
    }

    pub fn attr(&self, name: impl PartialEq<LocalName>) -> Option<&str> {
        let attr = self.attrs.iter().find(|attr| name == attr.name.local)?;
        Some(&attr.value)
    }

    /// pyro: the engine's reserved attribute `local` (`data-pyro-*`), which no author-visible path
    /// sees ([`crate::node::Attributes::reserved`]). Engine readers check the snapshot's nonce
    /// (`BaseDocument::engine_attr`).
    pub fn reserved_attr(&self, local: &str) -> Option<&str> {
        self.attrs.reserved(local)
    }

    pub fn attr_parsed<T: FromStr>(&self, name: impl PartialEq<LocalName>) -> Option<T> {
        let attr = self.attrs.iter().find(|attr| name == attr.name.local)?;
        attr.value.parse::<T>().ok()
    }

    /// Detects the presence of the attribute, treating *any* value as truthy.
    pub fn has_attr(&self, name: impl PartialEq<LocalName>) -> bool {
        self.attrs.iter().any(|attr| name == attr.name.local)
    }

    /// pyro: whether pyro-dom's render tree gave the element its [`crate::PYRO_STATE_NS`] `state`
    /// tokens (which then carry `:enabled`/`:disabled`).
    pub(crate) fn has_pyro_state(&self) -> bool {
        self.attrs.state("state").is_some()
    }

    pub fn can_be_disabled(&self) -> bool {
        local_names!("button", "input", "select", "textarea").contains(&self.name.local)
    }

    /// Whether this element is a link (an `<a>` or `<area>` element with an `href` attribute)
    pub fn is_link(&self) -> bool {
        (self.name.local == local_name!("a") || self.name.local == local_name!("area"))
            && self.has_attr(local_name!("href"))
    }

    /// Sync the visitedness bits of `element_state` with the element's link-ness.
    /// Blitz does not track browsing history, so all links are unvisited.
    ///
    /// Stylo's snapshot invalidation (`ElementWrapper::is_link`) determines link-ness
    /// from these state bits, so they must be kept accurate for `:link`/`:any-link`
    /// selectors to be correctly invalidated. Must be called whenever the `href`
    /// attribute is added or removed.
    pub fn flush_link_state(&mut self) {
        self.element_state
            .remove(ElementState::VISITED_OR_UNVISITED);
        if self.is_link() {
            self.element_state.insert(ElementState::UNVISITED);
        }
    }

    /// pyro: a link whose `href` resolves (against `base`) to `document` itself is visited, as
    /// the page a browser shows is in its history (WPT `css-color/currentcolor-004`, `href=""`):
    /// `:visited` matches it instead of `:link`. Other links stay unvisited: there is no
    /// history. Call after [`Self::flush_link_state`].
    pub fn flush_link_visited(&mut self, base: &url::Url, document: &url::Url) {
        if !self.element_state.contains(ElementState::UNVISITED) {
            return;
        }
        let Some(href) = self.attr(local_name!("href")) else {
            return;
        };
        if base.join(href.trim()).is_ok_and(|url| url == *document) {
            self.element_state.remove(ElementState::UNVISITED);
            self.element_state.insert(ElementState::VISITED);
        }
    }

    pub fn flush_open_state(&mut self) {
        let is_open = matches!(
            self.name.local,
            local_name!("details") | local_name!("dialog")
        ) && self.has_attr(local_name!("open"));
        self.element_state.set(ElementState::OPEN, is_open);
    }

    pub fn image_data(&self) -> Option<&ImageData> {
        match &self.special_data {
            SpecialElementData::Image(data) => Some(&**data),
            _ => None,
        }
    }

    pub fn image_data_mut(&mut self) -> Option<&mut ImageData> {
        match self.special_data {
            SpecialElementData::Image(ref mut data) => Some(&mut **data),
            _ => None,
        }
    }

    pub fn raster_image_data(&self) -> Option<&RasterImageData> {
        match self.image_data()? {
            ImageData::Raster(data) => Some(data),
            _ => None,
        }
    }

    pub fn raster_image_data_mut(&mut self) -> Option<&mut RasterImageData> {
        match self.image_data_mut()? {
            ImageData::Raster(data) => Some(data),
            _ => None,
        }
    }

    /// pyro: an HTML-in-Canvas canvas: a `<canvas>` whose `content` attribute is `drawable`
    /// (ASCII case-insensitive). Its children are laid out (`BaseDocument::layout_drawable_canvas_children`)
    /// but not painted in place (blitz-paint `render_element_clipped`).
    pub fn is_drawable_canvas(&self) -> bool {
        self.name.local == local_name!("canvas")
            && self.attr(local_name!("content")).is_some_and(|value| value.eq_ignore_ascii_case("drawable"))
    }

    pub fn canvas_data(&self) -> Option<&CanvasData> {
        match &self.special_data {
            SpecialElementData::Canvas(data) => Some(data),
            _ => None,
        }
    }

    pub fn sub_doc_data(&self) -> Option<&dyn Document> {
        match &self.special_data {
            SpecialElementData::SubDocument(data) => Some(data.as_ref()),
            _ => None,
        }
    }

    pub fn sub_doc_data_mut(&mut self) -> Option<&mut dyn Document> {
        match &mut self.special_data {
            SpecialElementData::SubDocument(data) => Some(data.as_mut()),
            _ => None,
        }
    }

    #[cfg(feature = "svg")]
    pub fn svg_data(&self) -> Option<&usvg::Tree> {
        match self.image_data()? {
            ImageData::Svg(data) => Some(&data.tree),
            _ => None,
        }
    }

    pub fn text_input_data(&self) -> Option<&TextInputData> {
        match &self.special_data {
            SpecialElementData::TextInput(data) => Some(data),
            _ => None,
        }
    }

    pub fn text_input_data_mut(&mut self) -> Option<&mut TextInputData> {
        match &mut self.special_data {
            SpecialElementData::TextInput(data) => Some(data),
            _ => None,
        }
    }

    #[cfg(feature = "custom-widget")]
    pub fn custom_widget_data(&self) -> Option<&CustomWidgetData> {
        match &self.special_data {
            SpecialElementData::CustomWidget(data) => Some(data),
            _ => None,
        }
    }

    #[cfg(feature = "custom-widget")]
    pub fn custom_widget_data_mut(&mut self) -> Option<&mut CustomWidgetData> {
        match &mut self.special_data {
            SpecialElementData::CustomWidget(data) => Some(data),
            _ => None,
        }
    }

    pub fn checkbox_input_checked(&self) -> Option<bool> {
        match self.special_data {
            SpecialElementData::CheckboxInput(checked) => Some(checked),
            _ => None,
        }
    }

    pub fn checkbox_input_checked_mut(&mut self) -> Option<&mut bool> {
        match self.special_data {
            SpecialElementData::CheckboxInput(ref mut checked) => Some(checked),
            _ => None,
        }
    }

    /// Set the checked state of a checkbox/radio input, keeping the
    /// `ElementState::CHECKED` bit (used for `:checked` selector matching and
    /// invalidation) in sync with the special data.
    pub fn set_checkbox_input_checked(&mut self, checked: bool) {
        if let Some(is_checked) = self.checkbox_input_checked_mut() {
            *is_checked = checked;
        }
        self.element_state.set(ElementState::CHECKED, checked);
    }

    /// pyro: brings the element states that mirror attributes
    /// pyro-dom's render tree writes up to the attributes: `:checked` of a
    /// selected `<option>`, and the [`crate::PYRO_STATE_NS`] `state` tokens.
    pub(crate) fn sync_lp_attribute_state(&mut self) {
        // (The engine's store: `Attributes::state`.)
        let lp = self.attrs.state("state");
        let bits = crate::has_invalidation::pyro_state_bits(lp);
        self.element_state.remove(crate::has_invalidation::PYRO_STATES);
        // pyro: `:enabled`/`:disabled` come from the tokens when they say either (pyro-dom knows
        // options, optgroups, fieldsets and what a disabled fieldset disables); otherwise Blitz's
        // own `disabled` attribute handling stands.
        if bits.intersects(ElementState::ENABLED | ElementState::DISABLED) {
            self.element_state.remove(ElementState::ENABLED | ElementState::DISABLED);
        }
        self.element_state.insert(bits);
        if self.name.local == local_name!("option") {
            let selected = self.has_attr(local_name!("selected"));
            self.element_state.set(ElementState::CHECKED, selected);
        }
    }

    #[cfg(feature = "file-input")]
    pub fn file_data(&self) -> Option<&FileData> {
        match &self.special_data {
            SpecialElementData::FileInput(data) => Some(data),
            _ => None,
        }
    }

    #[cfg(feature = "file-input")]
    pub fn file_data_mut(&mut self) -> Option<&mut FileData> {
        match &mut self.special_data {
            SpecialElementData::FileInput(data) => Some(data),
            _ => None,
        }
    }

    pub fn flush_is_focussable(&mut self) {
        let disabled: bool = self.attr_parsed(local_name!("disabled")).unwrap_or(false);
        let tabindex: Option<i32> = self.attr_parsed(local_name!("tabindex"));
        let contains_sub_document: bool = self.sub_doc_data().is_some();

        self.is_focussable = contains_sub_document
            || (!disabled
                && match tabindex {
                    Some(index) => index >= 0,
                    None => {
                        // Some focusable HTML elements have a default tabindex value of 0 set under the hood by the user agent.
                        // These elements are:
                        //   - <a> or <area> with href attribute
                        //   - <button>, <frame>, <iframe>, <input>, <object>, <select>, <textarea>, and SVG <a> element
                        //   - <summary> element that provides summary for a <details> element.

                        if [local_name!("a"), local_name!("area")].contains(&self.name.local) {
                            self.attr(local_name!("href")).is_some()
                        } else {
                            const DEFAULT_FOCUSSABLE_ELEMENTS: [LocalName; 7] = [
                                local_name!("button"),
                                local_name!("input"),
                                local_name!("select"),
                                local_name!("textarea"),
                                local_name!("frame"),
                                local_name!("iframe"),
                                local_name!("summary"),
                            ];
                            DEFAULT_FOCUSSABLE_ELEMENTS.contains(&self.name.local)
                        }
                    }
                })
    }

    /// Inline CSS for the cascade. The ordinary attribute keeps its CSSOM serialization.
    pub(crate) fn inline_declarations(&self) -> Option<&str> {
        let reflected = self.attr(local_name!("style"))?;
        Some(self.attrs.state("inline-declarations").unwrap_or(reflected))
    }

    pub fn flush_style_attribute(&mut self, guard: &SharedRwLock, url_extra_data: &UrlExtraData) {
        self.style_attribute = self.inline_declarations().map(|style_str| {
            ServoArc::new(guard.wrap(parse_style_attribute(
                style_str,
                url_extra_data,
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            )))
        });
    }

    pub fn set_style_property(
        &mut self,
        name: &str,
        value: &str,
        guard: &SharedRwLock,
        url_extra_data: UrlExtraData,
    ) -> bool {
        let mut context = ParserContext::new(
            Origin::Author,
            &url_extra_data,
            Some(CssRuleType::Style),
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            /* namespaces = */ Default::default(),
            None,
            None,
            /* attr_taint = */ Default::default(),
        );

        let Ok(property_id) = PropertyId::parse(name, &context) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(property = name, "Unsupported property");
            return false;
        };
        let mut source_property_declaration = SourcePropertyDeclaration::default();
        let mut parser = Parser::new(value);
        let Ok(_) = PropertyDeclaration::parse_into(
            &mut source_property_declaration,
            property_id,
            &mut context,
            &mut parser,
        ) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(property = name, value, "Invalid property value");
            return false;
        };

        self.mutable_style_attribute(guard)
            .write_with(&mut guard.write())
            .extend(source_property_declaration.drain(), Importance::Normal);

        true
    }

    // Copy-on-write: once Stylo has put this block in the rule tree it may be
    // shared with other elements' styles (mirrors Gecko's
    // nsDOMCSSDeclaration::EnsureBlockMutable).
    fn mutable_style_attribute(
        &mut self,
        guard: &SharedRwLock,
    ) -> &ServoArc<Locked<PropertyDeclarationBlock>> {
        let cloned = match &self.style_attribute {
            None => Some(PropertyDeclarationBlock::new()),
            Some(arc) => {
                let read = guard.read();
                let block = arc.read_with(&read);
                block
                    .immutable
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .then(|| block.clone())
            }
        };
        if let Some(block) = cloned {
            self.style_attribute = Some(ServoArc::new(guard.wrap(block)));
        }
        self.style_attribute.as_ref().unwrap()
    }

    pub fn remove_style_property(
        &mut self,
        name: &str,
        guard: &SharedRwLock,
        url_extra_data: UrlExtraData,
    ) -> bool {
        let context = ParserContext::new(
            Origin::Author,
            &url_extra_data,
            Some(CssRuleType::Style),
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            /* namespaces = */ Default::default(),
            None,
            None,
            /* attr_taint = */ Default::default(),
        );
        let Ok(property_id) = PropertyId::parse(name, &context) else {
            #[cfg(feature = "tracing")]
            tracing::warn!(property = name, "Unsupported property");
            return false;
        };

        if self.style_attribute.is_none() {
            return false;
        }
        let style = self.mutable_style_attribute(guard);
        let mut guard = guard.write();
        let style = style.write_with(&mut guard);
        if let Some(index) = style.first_declaration_to_remove(&property_id) {
            style.remove_property(&property_id, index);
            return true;
        }

        false
    }

    pub fn set_sub_document(&mut self, sub_document: Box<dyn Document>) {
        self.special_data = SpecialElementData::SubDocument(sub_document);
    }

    pub fn remove_sub_document(&mut self) {
        self.special_data = SpecialElementData::None;
    }

    #[cfg(feature = "custom-widget")]
    pub fn set_custom_widget(&mut self, mut widget: Box<dyn crate::Widget>) {
        use crate::node::custom_widget::CustomWidgetData;
        for attr in self.attrs.iter() {
            widget.attribute_changed(&attr.name.local, None, Some(&attr.value));
        }
        self.special_data = SpecialElementData::CustomWidget(CustomWidgetData::new(widget));
    }

    #[cfg(feature = "custom-widget")]
    pub fn remove_custom_widget(&mut self) -> Vec<anyrender::ResourceId> {
        let resource_ids = self
            .custom_widget_data_mut()
            .map(|widget_data| widget_data.take_resource_ids())
            .unwrap_or_default();
        self.special_data = SpecialElementData::None;
        resource_ids
    }

    pub fn take_inline_layout(&mut self) -> Option<Box<TextLayout>> {
        std::mem::take(&mut self.inline_layout_data)
    }

    pub(crate) fn is_button_input(&self) -> bool {
        self.name.local == local_name!("input")
            && matches!(
                self.attr(local_name!("type")),
                Some("button" | "submit" | "reset")
            )
    }

    pub fn is_submit_button(&self) -> bool {
        if self.name.local != local_name!("button") {
            return false;
        }
        let type_attr = self.attr(local_name!("type"));
        let is_submit = type_attr == Some("submit");
        let is_auto_submit = type_attr.is_none()
            && self.attr(LocalName::from("command")).is_none()
            && self.attr(LocalName::from("commandfor")).is_none();
        is_submit || is_auto_submit
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterImageData {
    /// The width of the image
    pub width: u32,
    /// The height of the image
    pub height: u32,
    /// The raw image data in RGBA8 format
    pub data: Blob<u8>,
    /// pyro: the EXIF orientation (1 to 8; 1 is none) the pixels were turned by when decoded
    /// (`image-orientation: from-image`). `image-orientation: none` shows them turned back.
    pub orientation: u8,
    /// pyro: a decoded `<video>` frame whose pixels a page must not read back (cross-origin media
    /// without CORS): HTML-in-Canvas privacy paints it but taints the canvas (blitz-paint
    /// `Privacy::admit`). False for every image from a response (those are classified by
    /// `net::is_opaque_image`).
    pub tainting: bool,
}
impl RasterImageData {
    pub fn new(width: u32, height: u32, data: Arc<Vec<u8>>) -> Self {
        Self {
            width,
            height,
            data: Blob::new(data),
            orientation: 1,
            tainting: false,
        }
    }

    /// pyro: for `image-orientation: none`: the affine coefficients (as kurbo's `Affine::new`)
    /// that take the decoded pixels' space to the image's as stored, which is `width` x
    /// `height` of `shown_size(true)`.
    pub fn unturn(&self) -> [f64; 6] {
        let (w, h) = self.shown_size(true);
        let (w, h) = (w as f64, h as f64);
        match self.orientation {
            2 => [-1.0, 0.0, 0.0, 1.0, w, 0.0],
            3 => [-1.0, 0.0, 0.0, -1.0, w, h],
            4 => [1.0, 0.0, 0.0, -1.0, 0.0, h],
            5 => [0.0, 1.0, 1.0, 0.0, 0.0, 0.0],
            6 => [0.0, -1.0, 1.0, 0.0, 0.0, h],
            7 => [0.0, -1.0, -1.0, 0.0, w, h],
            8 => [0.0, 1.0, -1.0, 0.0, w, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }

    /// pyro: the image's size as `image-orientation: none` shows it (`unoriented`), else as
    /// decoded.
    pub fn shown_size(&self, unoriented: bool) -> (u32, u32) {
        if unoriented && (5..=8).contains(&self.orientation) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[derive(Debug, Clone)]
pub enum ImageData {
    Raster(RasterImageData),
    #[cfg(feature = "svg")]
    Svg(SvgImageData),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Ok,
    Error,
    Loading,
}

#[derive(Debug, Clone)]
pub struct ImageResourceData {
    /// The url of the background image
    pub url: ServoArc<Url>,
    /// The loading status of the background image
    pub status: Status,
    /// The image data
    pub image: ImageData,
    /// pyro: how the image was requested (a `url()`'s request URL modifiers, the CORS mode of a
    /// mask or shape, the frame document it is for): with `url`, the identity of the request
    /// the slot holds the response of. A response to another request never goes in it.
    pub options: blitz_traits::net::FetchOptions,
}

impl ImageResourceData {
    pub fn new(url: ServoArc<Url>) -> Self {
        Self {
            url,
            status: Status::Loading,
            image: ImageData::None,
            options: Default::default(),
        }
    }

    /// pyro: this slot for the request made with `options`.
    pub fn with_options(mut self, options: blitz_traits::net::FetchOptions) -> Self {
        self.options = options;
        self
    }

    /// pyro: whether this slot holds the response to the request for `url` made with `options`.
    pub fn is_request(&self, url: &Url, options: &blitz_traits::net::FetchOptions) -> bool {
        *self.url == *url && self.options == *options
    }

    /// pyro: the key the response to this slot's request is cached and delivered under.
    pub fn request_key(&self) -> String {
        self.options.cache_key(self.url.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct CanvasData {
    pub custom_paint_source_id: u64,
}

impl std::fmt::Debug for SpecialElementData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecialElementData::SubDocument(_) => f.write_str("NodeSpecificData::SubDocument"),
            #[cfg(feature = "custom-widget")]
            SpecialElementData::CustomWidget(_) => f.write_str("NodeSpecificData::CustomWidget"),
            SpecialElementData::Stylesheet(_) => f.write_str("NodeSpecificData::Stylesheet"),
            SpecialElementData::Image(data) => match **data {
                ImageData::Raster(_) => f.write_str("NodeSpecificData::Image(Raster)"),
                #[cfg(feature = "svg")]
                ImageData::Svg(_) => f.write_str("NodeSpecificData::Image(Svg)"),
                ImageData::None => f.write_str("NodeSpecificData::Image(None)"),
            },
            SpecialElementData::Canvas(_) => f.write_str("NodeSpecificData::Canvas"),
            SpecialElementData::TableRoot(_) => f.write_str("NodeSpecificData::TableRoot"),
            SpecialElementData::TextInput(_) => f.write_str("NodeSpecificData::TextInput"),
            SpecialElementData::CheckboxInput(_) => f.write_str("NodeSpecificData::CheckboxInput"),
            #[cfg(feature = "file-input")]
            SpecialElementData::FileInput(_) => f.write_str("NodeSpecificData::FileInput"),
            SpecialElementData::None => f.write_str("NodeSpecificData::None"),
        }
    }
}

#[derive(Clone)]
pub struct ListItemLayout {
    pub marker: Marker,
    /// pyro: the node holding the item's `::marker` style (`ElementData::marker_style`), when a
    /// rule styles the marker: its text is that node's (colour, font, shadows).
    pub style_node: Option<NodeId>,
    pub position: ListItemLayoutPosition,
    /// pyro: the marker's digits are tabular (Blink's UA sheet: `::marker { font-variant-numeric:
    /// tabular-nums }`), unless a `::marker` rule declares `font-variant-numeric`.
    pub tabular_nums: bool,
    /// pyro: a string marker inside the item is bidi-isolated in the item's paragraph (Blink's UA
    /// sheet: `::marker { unicode-bidi: isolate }`), unless a `::marker` rule declares
    /// `unicode-bidi: normal`.
    pub isolate: bool,
    /// pyro: the room after an image marker: Blink's 7px after a `list-style-image`, none after
    /// a `::marker`'s `content` image (inline content of the marker).
    pub image_gap: f32,
    /// Offset (in physical CSS px) of the unrounded content box origin from the rounded one in
    /// `final_layout`. Paint draws text from the unrounded origin so that it keeps its sub-pixel
    /// position while box edges stay snapped. Set by the `writing-mode` layout pass; zero otherwise.
    pub content_box_subpixel_offset: taffy::Point<f32>,
    /// pyro: an outside marker's emphasis marks (`text-emphasis`), drawn as over inline text.
    pub annotations: Vec<crate::node::TextAnnotation>,
}

//We seperate chars from strings in order to optimise rendering - ie not needing to
//construct a whole parley layout for simple char markers
#[derive(Debug, PartialEq, Clone)]
pub enum Marker {
    Char(char),
    String(String),
    /// pyro: the item's loaded `list-style-image` (`ElementData::list_style_images`), its
    /// natural size in CSS px.
    Image { width: f32, height: f32 },
}

//Value depends on list-style-position, determining whether a seperate layout is created for it
#[derive(Clone)]
pub enum ListItemLayoutPosition {
    Inside,
    Outside(Box<parley::Layout<TextBrush>>),
}

impl std::fmt::Debug for ListItemLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ListItemLayout - marker {:?}", self.marker)
    }
}

#[cfg(feature = "file-input")]
mod file_data {
    use std::ops::{Deref, DerefMut};
    use std::path::PathBuf;

    #[derive(Clone, Debug)]
    pub struct FileData(pub Vec<PathBuf>);
    impl Deref for FileData {
        type Target = Vec<PathBuf>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl DerefMut for FileData {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl From<Vec<PathBuf>> for FileData {
        fn from(files: Vec<PathBuf>) -> Self {
            Self(files)
        }
    }
}
#[cfg(feature = "file-input")]
pub use file_data::FileData;

#[cfg(test)]
mod tests {
    use super::TextInputData;
    use parley::{FontContext, LayoutContext};

    /// Build a [`TextInputData`] with the given text laid out at scale 1.0.
    fn make_input(is_multiline: bool, text: &str) -> TextInputData {
        let mut font_ctx = FontContext::new();
        let mut layout_ctx = LayoutContext::new();
        let mut data = TextInputData::new(is_multiline);
        data.editor.set_scale(1.0);
        data.editor.set_text(text);
        data.editor
            .driver(&mut font_ctx, &mut layout_ctx)
            .refresh_layout();
        data
    }

    #[test]
    fn short_text_does_not_scroll() {
        let mut data = make_input(false, "hi");
        // A wide content box that comfortably fits the text.
        data.clamp_scroll_offset(1000.0, 100.0);
        assert_eq!(data.scroll_offset, 0.0);
    }

    #[test]
    fn single_line_scrolls_to_follow_caret() {
        let text = "the quick brown fox jumps over the lazy dog repeatedly and at length";
        let mut data = make_input(false, text);
        let content_box_width = 40.0;
        let content_box_height = 20.0;

        // Caret at the end of a string that overflows a narrow input should scroll right.
        data.editor
            .driver(&mut FontContext::new(), &mut LayoutContext::new())
            .move_to_text_end();
        data.clamp_scroll_offset(content_box_width, content_box_height);

        let layout_width = data.editor.try_layout().unwrap().full_width();
        if layout_width > content_box_width {
            assert!(
                data.scroll_offset > 0.0,
                "expected horizontal scroll for overflowing single-line input"
            );
            // The caret must be within the visible region after scrolling (pyro: its start edge:
            // its width may hang past text that ends at the edge, which is not scrolled for it).
            let caret = data.editor.cursor_geometry(1.5).unwrap();
            assert!(caret.x0 as f32 <= data.scroll_offset + content_box_width + 0.5);
            assert!(caret.x0 as f32 >= data.scroll_offset - 0.5);
        }

        // Moving the caret back to the start should reset the scroll offset.
        data.editor
            .driver(&mut FontContext::new(), &mut LayoutContext::new())
            .move_to_text_start();
        data.clamp_scroll_offset(content_box_width, content_box_height);
        assert_eq!(data.scroll_offset, 0.0);
    }

    #[test]
    fn multiline_scrolls_vertically_not_horizontally() {
        let text = (0..40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut data = make_input(true, &text);
        // Constrain the width so wrapping is well-defined.
        data.editor.set_width(Some(200.0));
        data.editor
            .driver(&mut FontContext::new(), &mut LayoutContext::new())
            .refresh_layout();

        let content_box_width = 200.0;
        let content_box_height = 30.0;

        data.editor
            .driver(&mut FontContext::new(), &mut LayoutContext::new())
            .move_to_text_end();
        data.clamp_scroll_offset(content_box_width, content_box_height);

        let layout_height = data.editor.try_layout().unwrap().height();
        if layout_height > content_box_height {
            assert!(
                data.scroll_offset > 0.0,
                "expected vertical scroll for overflowing multi-line input"
            );
        }
    }

    #[test]
    fn scroll_by_clamps_and_bubbles() {
        let text = (0..40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut data = make_input(true, &text);
        data.editor.set_width(Some(200.0));
        data.editor
            .driver(&mut FontContext::new(), &mut LayoutContext::new())
            .refresh_layout();

        let content_box_width = 200.0;
        let content_box_height = 30.0;
        let max = data.max_scroll_offset(content_box_width, content_box_height);
        assert!(max > 0.0, "test text should overflow the content box");

        // Scrolling up (positive delta decreases offset) while already at the top is a no-op and
        // the whole delta bubbles.
        assert_eq!(data.scroll_offset, 0.0);
        let bubbled = data.scroll_by(15.0, content_box_width, content_box_height);
        assert_eq!(data.scroll_offset, 0.0);
        assert_eq!(bubbled, 15.0);

        // Scrolling down moves the offset and consumes the delta.
        let bubbled = data.scroll_by(-10.0, content_box_width, content_box_height);
        assert_eq!(data.scroll_offset, 10.0);
        assert_eq!(bubbled, 0.0);

        // Scrolling past the end clamps to the maximum and bubbles the remainder. Starting at
        // offset 10 with max headroom of `max - 10`, a delta of `-(max + 100)` consumes
        // `max - 10` and bubbles the rest (`-110`).
        let bubbled = data.scroll_by(-(max + 100.0), content_box_width, content_box_height);
        assert_eq!(data.scroll_offset, max);
        assert!((bubbled - (-110.0)).abs() < 1e-3);
    }

    #[test]
    fn single_line_does_not_scroll_when_text_fits() {
        let mut data = make_input(false, "hi");
        // Wide content box; nothing to scroll, so all delta bubbles.
        let bubbled = data.scroll_by(-50.0, 1000.0, 100.0);
        assert_eq!(data.scroll_offset, 0.0);
        assert_eq!(bubbled, -50.0);
    }
}
