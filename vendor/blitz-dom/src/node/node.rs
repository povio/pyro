use crate::Document;
use crate::layout::paint_tree::{HoistedPaintChild, StackingContext};
use bitflags::bitflags;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, DomEventData, HitResult, PointerCoords,
};
use blitz_traits::node_id::NodeId;
use blitz_traits::shell::ShellProvider;
use euclid::{Point2D, Rect, Size2D};
use keyboard_types::Modifiers;
use kurbo::{Affine, Rect as KurboRect};
use markup5ever::{LocalName, local_name};
use parley::{BreakReason, Cluster, ClusterSide};
use selectors::matching::ElementSelectorFlags;
use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use style::computed_values::flex_direction::T as FlexDirection;
use style::computed_values::isolation::T as Isolation;
use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::properties::ComputedValues;
use style::properties::generated::longhands::position::computed_value::T as Position;
use style::selector_parser::RestyleDamage;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::SharedRwLock;
use style::stylesheets::UrlExtraData;
use style::values::computed::CSSPixelLength;
use style::values::computed::Display as StyloDisplay;
use style::values::computed::Rotate;
use style::values::generics::transform::{Scale, Translate};
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use style_dom::ElementState;
use style_traits::values::ToCss;
use stylo_taffy::CopyStyleGetters;
use stylo_taffy::WritingModeExt;
use taffy::{Cache, prelude::Layout};
use thin_vec::ThinVec;

use super::stylo_data::{ComputedStyleRef, LayoutStyles, StyloData};
use super::{Attribute, DocumentData, ElementData, LayoutData};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayOuter {
    Block,
    Inline,
    None,
}

bitflags! {
    #[derive(Clone, Copy, PartialEq)]
    pub struct NodeFlags: u32 {
        /// Whether the node is the root node of an Inline Formatting Context
        const IS_INLINE_ROOT = 0b00000001;
        /// Whether the node is the root node of an Table formatting context
        const IS_TABLE_ROOT = 0b00000010;
        /// Whether the node is "in the document" (~= has a parent and isn't a template node)
        const IS_IN_DOCUMENT = 0b00000100;
        /// Whether the node is an inline box whose in-flow content is block-level only
        const IS_INLINE_WITH_ONLY_BLOCK_CHILDREN = 0b00001000;
        /// pyro: the node had damage of its own (a mutation or a restyle), not only its
        /// descendants', when this pass propagated damage. Set by `propagate_damage_flags`, cleared
        /// with the damage; the paint tree reads it (`PAINT_TREE_OWN_DAMAGE`), tables read
        /// `OWN_LAYOUT_DAMAGE`.
        const OWN_DAMAGE = 0b00010000;
        /// The node's `paint_children`, `stacking_context` or
        /// `sc_contribution_cache` may be out of date: some resolve since the
        /// last paint-tree build stored `REBUILD_STACKING_CONTEXT` damage on it
        /// (set by `clear_damage_and_dirty_flags`, removed when
        /// `ensure_paint_tree` rebuilds the node). New nodes start dirty.
        const PAINT_TREE_DIRTY = 0b00100000;
        /// The node itself had damage that reaches the paint tree (`OWN_DAMAGE` with
        /// `REBUILD_STACKING_CONTEXT`) in some resolve since the last paint-tree build: a table
        /// reads it where it used to read `OWN_DAMAGE`, which each resolve clears
        /// (`table_paint_children_kept`). Set and cleared with `PAINT_TREE_DIRTY`.
        const PAINT_TREE_OWN_DAMAGE = 0b01000000;
        /// pyro: `OWN_DAMAGE` whose damage reaches layout (`damage::reaches_layout`:
        /// relayout, construction, reordering), not only paint. Tables read it to tell a cell or
        /// row that is to be collected again from one whose colour, background or opacity
        /// changed. Set and cleared with `OWN_DAMAGE`.
        const OWN_LAYOUT_DAMAGE = 0b10000000;
        /// pyro: its 3D transform hides the element and its content: it shows its back face
        /// with `backface-visibility: hidden`, it lies behind the viewer, or its matrix has no
        /// inverse. Set with the transform (`Node::set_transform`); paint skips the element.
        const TRANSFORM_HIDES = 0b1_00000000;
        /// pyro: the last transform pass gave this node's children a [`TransformContext`] (its
        /// `perspective`, or its `transform-style: preserve-3d`), which their transforms
        /// include: the next pass resolves them again, whatever their own damage
        /// (`BaseDocument::resolve_transforms`).
        const GIVES_3D_CONTEXT = 0b10_00000000;
        /// pyro: its transform is a 3D matrix flattened into the plane of the screen and it is
        /// flat (`transform-style: flat`): Chromium composites such an element, rasterized in its
        /// own plane and then drawn through its transform, which blitz-paint asks the backend to
        /// do (`layers::local_raster_marker`). Set with the transform (`Node::set_transform`).
        const FLATTENS_3D = 0b100_00000000;
        /// pyro: an inline box (`display: inline`, not atomic) whose in-flow content includes
        /// block-level boxes, split around them (CSS 2.1 § 9.2.1.1): it is laid out as a block
        /// box without margins, borders, padding or a size of its own, and paints no box: its
        /// anonymous blocks hold its inline pieces, whose lines paint its fragments
        /// (`layout::inline::split_pieces`). Its relative offset moves the whole of it.
        const IS_SPLIT_INLINE = 1 << 11;
        /// pyro: the box of an initial letter (`initial-letter` on a `::first-letter`, CSS
        /// Inline 3 § 7): an inline root of the letter alone, sized by the ink of its glyphs
        /// (`layout::first_letter`), placed by the first line of the root it starts.
        const IS_INITIAL_LETTER_BOX = 1 << 18;
        /// pyro: an anonymous block whose last child is a split inline (`IS_SPLIT_INLINE`)
        /// standing for its head: the inline content before its first block-level box, with its
        /// start. Set by the construction of the split inline's container.
        const ENDS_WITH_SPLIT_HEAD = 1 << 12;
        /// pyro: an anonymous block whose first child is a split inline standing for its tail:
        /// the inline content after its last block-level box, with its end.
        const STARTS_WITH_SPLIT_TAIL = 1 << 13;
        /// pyro: a white-space text node between two table parts of an inline box (cells, rows,
        /// ... laid out there as atomic inlines), which generates no box (CSS 2.1 § 17.2.1, as if
        /// its parts were in one anonymous inline table). Set and cleared by the construction of
        /// its inline formatting context (`table_fixup::mark_space_between_inline_table_parts`).
        const IS_SPACE_BETWEEN_TABLE_PARTS = 1 << 22;
        /// pyro: during the construction of an inline formatting context, an inline box one of whose
        /// children is a table part (`IS_SPACE_BETWEEN_TABLE_PARTS`); cleared as the box is done.
        const ENCLOSES_TABLE_PARTS = 1 << 24;
        /// pyro: a table cell whose content is laid out lower in its box, by its
        /// `LayoutData::table_baseline_offset` (a cell aligned on its row's baseline; set by its
        /// table's layout, `layout::table::shift_cell_content`).
        const HAS_TABLE_BASELINE_OFFSET = 1 << 23;
        /// pyro: some of its `paint_children` are floats, its own or lifted out of the boxes of
        /// its subtree that paint in its paint phases (`Node::paints_in_ancestor_phases`): it paints
        /// them in the phases of CSS 2.1 Appendix E, after the block backgrounds and before the
        /// inline content of those boxes. Set by the paint-tree build.
        const HAS_FLOAT_PAINT_CHILDREN = 1 << 14;
        /// pyro: it lifted floats out of itself to the box that paints them (it paints in that
        /// box's phases), at the last paint-tree build: whether it still does is checked before
        /// its lists are kept.
        const LIFTS_FLOATS = 1 << 15;
        /// pyro: in-flow block boxes it paints in its phases overlap: a block-level child starts
        /// above the bottom of one before it (a negative margin), or follows one whose content
        /// overflows it, here or in a box of its subtree that paints in its phases
        /// (`Node::paints_in_ancestor_phases`). Painted box by box, the later box's background
        /// would cover the earlier one's inline content: it paints its children in the phases of
        /// CSS 2.1 Appendix E, as with floats. Set by its layout.
        const FLOW_BOXES_OVERLAP = 1 << 16;
        /// pyro: its in-flow content is taller than its box (a `height` or `max-height` shorter
        /// than its lines or blocks). Set by its layout.
        const FLOW_CONTENT_OVERFLOWS = 1 << 17;
        /// pyro: the box's last layout clamped its lines (`layout::line_clamp`): one of
        /// its blocks holds a clamp mark.
        const CLAMPS_LINES = 1 << 25;
        /// pyro: the `<body>` whose `overflow` the viewport takes
        /// (`BaseDocument::body_overflow_goes_to_viewport`): its layout style reads `overflow:
        /// visible`. Set before each layout pass (`BaseDocument::mark_viewport_body`).
        const OVERFLOW_TO_VIEWPORT = 1 << 26;
        /// pyro: a shadow host: its children are its shadow tree's (pyro-dom's render tree
        /// marks it, `crate::shadow`).
        const IS_SHADOW_HOST = 1 << 19;
        /// pyro: a slot of a shadow tree whose children are the nodes assigned to it, light
        /// children of its host (`crate::shadow`).
        const IS_ASSIGNED_SLOT = 1 << 20;
        /// pyro: what the box's cached final layout (`RunMode::PerformLayout`) stands for is no
        /// longer laid out so: a measuring run left its lines elsewhere (an inline root), or those
        /// of an inline root in it. Its next final layout runs again instead of standing on the
        /// cache (`LayoutPassState::mark_final_layout_stale`), and clears the mark.
        const FINAL_LAYOUT_STALE = 1 << 21;
        /// pyro: the box's layout algorithm is running (`LayoutPassState::compute_child_layout_internal`).
        const IN_LAYOUT = 1 << 27;
        /// pyro: an anonymous block wrapping a run of its container's inline-level boxes
        /// (`LayoutChildren::create_anonymous_block`). The container's walk placed the scroll
        /// pseudo-element boxes beside those boxes already (`scroll_markers`), in this block or
        /// beside it: its own construction and inline layout do not place them again.
        const WRAPS_BOX_RUN = 1 << 28;
        /// pyro: a positioned (relative or sticky) inline box that is not atomic, listed in the paint
        /// tree as a positioned entry of its own (`BaseDocument::positioned_inline_spans`): its content
        /// paints there, in tree order with the other positioned boxes (CSS 2.1 Appendix E step 8).
        /// Hit testing passes over it (its inline root hit-tests its text).
        const INLINE_PASS_ENTRY = 1 << 29;
        /// pyro: a box after the clamp point of a line clamp (`layout::line_clamp`): not painted.
        const HIDDEN_BY_CLAMP = 1 << 30;
    }
}

/// pyro: what an element gives the 3D transforms of its children
/// ([`Node::transform_context_for_children`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct TransformContext {
    /// Its perspective matrix (its `perspective`, about its `perspective-origin`), in its border
    /// box's coordinates (CSS px).
    pub perspective: Option<euclid::default::Transform3D<f64>>,
    /// With `transform-style: preserve-3d`, its 3D rendering context, which its children share:
    /// their planes are placed by its accumulated matrix composed with theirs, and so is which
    /// face of theirs shows.
    pub preserve_3d: Option<Context3D>,
    /// Its scroll offset (CSS px): its children move by it, its perspective origin does not.
    pub scroll: (f64, f64),
    /// pyro: its border box size, border widths and paddings (top, right, bottom, left), for the
    /// `offset-path: ray()` of the boxes it contains (`MotionContainingBlock`).
    pub cb_size: (f64, f64),
    pub cb_border: [f64; 4],
    pub cb_padding: [f64; 4],
    pub cb_radii: [kurbo::Vec2; 4],
}

/// pyro: a 3D rendering context as an element of it gives it to its children
/// ([`TransformContext::preserve_3d`]).
#[derive(Clone, Copy, Debug)]
pub struct Context3D {
    /// The element's accumulated 3D matrix: from its coordinates to those of the parent of the
    /// context's root (CSS px).
    pub accumulated: euclid::default::Transform3D<f64>,
    /// The 2D affine that paint and layout queries compose for the element (its locations,
    /// scroll offsets and transforms down from the parent of the context's root, CSS px): its
    /// children's transforms are stored relative to it. Never singular (`regularize`).
    pub base: Affine,
    /// pyro (lane H2): the element that established the context (the root of the 3D
    /// rendering context), and its location in its parent less the parent's scroll (CSS px):
    /// the planes of the context are given relative to its untransformed border box
    /// ([`Plane3D::matrix`]).
    pub root: NodeId,
    pub root_offset: (f64, f64),
    /// pyro (lane H2): where layout queries place the element when not by `base`: the
    /// homography (row-major, CSS px) from its coordinates to the context's that its plane's
    /// [`Plane3D::local`] composes to ([`Plane3D::places_by_local`]). Its children's are given
    /// relative to it.
    pub frame: Option<[f64; 9]>,
}

/// pyro (lane H2): a plane blitz-paint draws through its 4x4 matrix rather than through the
/// 2D transform the node keeps for layout queries (`Node::transform`, the flattened stand-in):
/// an element of a 3D rendering context (sorted by depth with the context's other planes), or a
/// flat element whose 3D transform maps it projectively (a rotation out of the plane under
/// perspective).
#[derive(Clone, Copy, Debug)]
pub struct Plane3D {
    /// The root of its 3D rendering context; for a flat element outside one, the element itself.
    pub root: NodeId,
    /// From the element's border box (CSS px) to the root's untransformed border box (CSS px,
    /// the root at its place in its parent before its own transform), in euclid's row-vector
    /// convention: Z is the depth (towards the viewer), W the perspective divisor.
    pub matrix: euclid::default::Transform3D<f64>,
    /// What the element paints itself is not drawn (`backface-visibility: hidden` on a
    /// `preserve-3d` element showing its back face); the planes of its children still are.
    pub own_hidden: bool,
    /// The plane as layout queries place it in place of the 2D stand-in (`Node::transform`): a
    /// homography (row-major, `(X, Y, W) = H (x, y, 1)`, CSS px) from the border box to the
    /// coordinates the stand-in maps it into (the box's untransformed place in its parent's, the
    /// parent as layout queries place it).
    pub local: [f64; 9],
    /// Layout queries place the box by [`Self::local`] rather than by the stand-in: W varies over
    /// the plane, or the context element it is in is placed by its own, and the plane does not
    /// cross the viewer's plane.
    pub places_by_local: bool,
    /// The plane crosses the viewer's plane: paint draws the part in front of it; layout keeps the
    /// box at its untransformed place (its projection has no bounds), and print, which draws the
    /// stand-ins, leaves it out.
    pub crosses_viewer: bool,
}

impl Plane3D {
    /// Whether W varies over the plane: paint draws it through a perspective.
    pub fn is_projective(&self) -> bool {
        varies_w(&self.matrix)
    }
}

/// pyro (lane H2): whether W varies over a plane under `m` ([`Plane3D::is_projective`]).
fn varies_w(m: &euclid::default::Transform3D<f64>) -> bool {
    m.m14.abs() + m.m24.abs() > 1e-12 * m.m44.abs()
}

/// pyro (lane H2): the homography of a 4x4 matrix's projection onto the plane of the screen
/// (rows and columns 1, 2 and 4), row-major.
fn projection_homography(m: &euclid::default::Transform3D<f64>) -> [f64; 9] {
    [m.m11, m.m21, m.m41, m.m12, m.m22, m.m42, m.m14, m.m24, m.m44]
}

/// pyro (lane H2): the homography of a 2D affine, row-major.
fn affine_homography(a: Affine) -> [f64; 9] {
    let [a11, a21, a12, a22, a13, a23] = a.as_coeffs();
    [a11, a12, a13, a21, a22, a23, 0.0, 0.0, 1.0]
}

/// pyro (lane H2): `a * b` (`b` applied first).
fn homography_mul(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    std::array::from_fn(|i| {
        let (r, c) = (i / 3, i % 3);
        (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum()
    })
}

/// pyro (lane H2): the inverse of a homography, `None` for a singular one.
fn homography_inverse(m: &[f64; 9]) -> Option<[f64; 9]> {
    let cofactor =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0 * 3 + c0] * m[r1 * 3 + c1] - m[r0 * 3 + c1] * m[r1 * 3 + c0];
    let adjugate = [
        cofactor(1, 2, 1, 2),
        -cofactor(0, 2, 1, 2),
        cofactor(0, 1, 1, 2),
        -cofactor(1, 2, 0, 2),
        cofactor(0, 2, 0, 2),
        -cofactor(0, 1, 0, 2),
        cofactor(1, 2, 0, 1),
        -cofactor(0, 2, 0, 1),
        cofactor(0, 1, 0, 1),
    ];
    let determinant = m[0] * adjugate[0] + m[1] * adjugate[3] + m[2] * adjugate[6];
    (determinant.is_finite() && determinant != 0.0).then(|| adjugate.map(|v| v / determinant))
}

impl TransformContext {
    /// Whether it gives anything (a perspective or a 3D rendering context).
    pub fn gives(&self) -> bool {
        self.perspective.is_some() || self.preserve_3d.is_some()
    }
}

/// pyro: whether `backface-visibility: hidden` hides an element under the matrix `m`: its back
/// face shows. (A `preserve-3d` element's children are planes of their own, which paint would
/// hide with it: its caller does not ask.)
fn back_face_hidden(
    box_styles: &style::properties::generated::style_structs::Box,
    m: &euclid::default::Transform3D<f64>,
) -> bool {
    box_styles.backface_visibility == style::computed_values::backface_visibility::T::Hidden
        && crate::stylo_to_kurbo::is_back_face_visible(m)
}

/// pyro: `t` with a negligible thickness (1e-6) along a direction it collapses: the flattened
/// transform of an edge-on plane of a 3D rendering context (`rotateX(90deg)`), whose children
/// are placed relative to it ([`Context3D::base`]). What it paints itself stays invisible.
fn regularize(t: Affine) -> Affine {
    const THICKNESS: f64 = 1e-6;
    let [a, b, c, d, e, f] = t.as_coeffs();
    let (x_len, y_len) = (a.hypot(b), c.hypot(d));
    if (a * d - b * c).abs() >= THICKNESS * x_len.max(y_len).max(1.0) {
        return t;
    }
    if x_len == 0.0 && y_len == 0.0 {
        Affine::new([THICKNESS, 0.0, 0.0, THICKNESS, e, f])
    } else if x_len >= y_len {
        // The y axis collapses (onto the x axis, or to a point): across the x axis instead.
        Affine::new([a, b, -b / x_len * THICKNESS, a / x_len * THICKNESS, e, f])
    } else {
        Affine::new([d / y_len * THICKNESS, -c / y_len * THICKNESS, c, d, e, f])
    }
}

/// A transform resolved in CSS pixels in device-pixel space (S * T * S^-1): translation
/// components are scaled, linear components are not.
fn to_device_px(t: Affine, scale: f32) -> Affine {
    let scale = scale as f64;
    let [m11, m12, m21, m22, m41, m42] = t.as_coeffs();
    Affine::new([m11, m12, m21, m22, m41 * scale, m42 * scale])
}

impl NodeFlags {
    #[inline(always)]
    pub fn is_inline_root(&self) -> bool {
        self.contains(Self::IS_INLINE_ROOT)
    }

    #[inline(always)]
    pub fn is_table_root(&self) -> bool {
        self.contains(Self::IS_TABLE_ROOT)
    }

    #[inline(always)]
    pub fn is_in_document(&self) -> bool {
        self.contains(Self::IS_IN_DOCUMENT)
    }

    #[inline(always)]
    pub fn reset_construction_flags(&mut self) {
        self.remove(Self::IS_INLINE_ROOT);
        self.remove(Self::IS_TABLE_ROOT);
        self.remove(Self::IS_INLINE_WITH_ONLY_BLOCK_CHILDREN);
        self.remove(Self::IS_SPLIT_INLINE);
    }
}

pub struct Node {
    // The actual tree we belong to. This is unsafe!!
    tree: *mut crate::NodeTree,

    /// Our Id
    pub id: NodeId,
    /// Our parent's ID
    pub parent: Option<NodeId>,
    // What are our children?
    pub children: ThinVec<NodeId>,
    /// Our parent in the layout hierachy: a separate list that includes anonymous collections of inline elements
    pub layout_parent: Cell<Option<NodeId>>,
    /// A separate child list that includes anonymous collections of inline elements
    pub layout_children: RefCell<Option<ThinVec<NodeId>>>,
    /// pyro: the node whose `layout_children` list holds this node,
    /// kept where those lists are built (`resolve_layout_children`). The node
    /// is a box of the current layout when it equals `layout_parent` (which
    /// inline content of an inline root also has, without being listed).
    pub layout_child_of: Cell<Option<NodeId>>,
    /// Out-of-flow (absolutely/fixed positioned) boxes for which this node is the
    /// containing block. Recorded by Taffy's out-of-flow positioning pass. The
    /// `Layout.location` of these boxes is relative to this node's border box.
    pub hoisted_children: RefCell<ThinVec<NodeId>>,
    /// For an out-of-flow box that was hoisted by Taffy's out-of-flow positioning
    /// pass, the node whose `hoisted_children` list contains it (its containing
    /// block). The box's `Layout.location` is relative to this node rather than
    /// to its `layout_parent`. `None` for in-flow boxes.
    pub oof_containing_block: Cell<Option<NodeId>>,
    /// Anonymous block boxes created for this node during layout construction.
    ///
    /// Anonymous blocks live only in the slab (they are not part of the DOM
    /// `children` list), so we track the ones we own here to be able to
    /// deallocate them when this node is reconstructed.
    pub anonymous_blocks: ThinVec<NodeId>,
    /// The same as layout_children, but sorted by z-index
    pub paint_children: RefCell<Option<ThinVec<NodeId>>>,
    pub stacking_context: Option<Box<StackingContext>>,
    /// The hoisted entries this node's subtree pushed into the enclosing
    /// stacking context on its last (non-skipped) paint-tree build. Replayed
    /// when the subtree is skipped as clean. Empty for stacking-context roots.
    pub sc_contribution_cache: RefCell<ThinVec<HoistedPaintChild>>,

    // Flags
    pub flags: NodeFlags,
    /// Whether the node's box is out of flow (absolutely positioned and not `display: none`).
    /// Written by the style traversal from the node's new primary style, so that layout
    /// passes can test it without reading styles.
    pub out_of_flow: AtomicBool,

    /// Node type (Element, TextNode, etc) specific data.
    ///
    /// For element nodes this holds the [`ElementData`], which stores most of
    /// the per-node style/layout state. For the document node it holds the
    /// [`DocumentData`]. Access the moved fields through the forwarding methods
    /// on [`Node`] (e.g. [`Node::style`], [`Node::final_layout`]).
    pub data: NodeData,
}

unsafe impl Send for Node {}
unsafe impl Sync for Node {}

/// Generates forwarding accessors for fields that live on both [`ElementData`]
/// (element / anonymous block nodes) and [`DocumentData`] (the document node).
macro_rules! universal_accessors {
    ($($(#[$meta:meta])* $field:ident / $field_mut:ident : $ty:ty),* $(,)?) => {
        impl Node {
            $(
                $(#[$meta])*
                #[inline]
                pub fn $field(&self) -> &$ty {
                    match &self.data {
                        NodeData::Element(data) | NodeData::AnonymousBlock(data) => &data.$field,
                        NodeData::Document(data) => &data.$field,
                        _ => panic!(concat!("`", stringify!($field), "` is not available on this node kind")),
                    }
                }

                $(#[$meta])*
                #[inline]
                pub fn $field_mut(&mut self) -> &mut $ty {
                    match &mut self.data {
                        NodeData::Element(data) | NodeData::AnonymousBlock(data) => &mut data.$field,
                        NodeData::Document(data) => &mut data.$field,
                        _ => panic!(concat!("`", stringify!($field), "` is not available on this node kind")),
                    }
                }
            )*
        }
    };
}

universal_accessors! {
    stylo_element_data / stylo_element_data_mut: StyloData,
    transform / transform_mut: Option<Box<Affine>>,
    display_constructed_as / display_constructed_as_mut: StyloDisplay,
    // The document node is styled/snapshotted like an element, so it also
    // carries these:
    element_state / element_state_mut: ElementState,
    snapshot_handled / snapshot_handled_mut: AtomicBool,
    // `apply_selector_flags` deposits `for_parent()` flags on the parent node,
    // and the parent of the root <html> element is the document -- so the
    // document has to be able to hold selector flags too.
    selector_flags / selector_flags_mut: Cell<ElementSelectorFlags>,
}

impl Node {
    /// This node's layout output state, or a shared default if layout has
    /// never written to this node.
    ///
    /// Panics for node kinds which do not participate in layout (text and
    /// comment nodes).
    #[inline]
    pub fn layout_data(&self) -> &LayoutData {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.layout_data(),
            NodeData::Document(data) => data.layout_data(),
            _ => panic!("`layout_data` is not available on this node kind"),
        }
    }

    /// Mutable access to this node's layout output state, allocating it if it
    /// does not yet exist.
    ///
    /// Panics for node kinds which do not participate in layout (text and
    /// comment nodes).
    #[inline]
    pub fn layout_data_mut(&mut self) -> &mut LayoutData {
        match &mut self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.layout_data_mut(),
            NodeData::Document(data) => data.layout_data_mut(),
            _ => panic!("`layout_data` is not available on this node kind"),
        }
    }

    /// Mutable access to this node's layout output state, if it has been
    /// allocated. Never allocates. Returns `None` for node kinds which do not
    /// participate in layout.
    #[inline]
    pub fn try_layout_data_mut(&mut self) -> Option<&mut LayoutData> {
        match &mut self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                data.layout_data.as_deref_mut()
            }
            NodeData::Document(data) => data.layout_data.as_deref_mut(),
            _ => None,
        }
    }

    /// Clear this node's taffy layout cache without allocating `LayoutData`
    /// for nodes that have never been laid out.
    #[inline]
    pub fn clear_layout_cache(&mut self) {
        if let Some(layout_data) = self.try_layout_data_mut() {
            layout_data.cache.clear();
        }
    }

    #[inline]
    pub fn cache(&self) -> &Cache {
        &self.layout_data().cache
    }

    #[inline]
    pub fn cache_mut(&mut self) -> &mut Cache {
        &mut self.layout_data_mut().cache
    }

    #[inline]
    pub fn unrounded_layout(&self) -> &Layout {
        &self.layout_data().unrounded_layout
    }

    /// Writes this node's unrounded layout, the only way to. A change is marked on the node and
    /// its ancestors: their rounding (`RoundTree::rounded_unchanged`) and cached scrollable overflow
    /// read those flags, and a relayout can move a box that no style change touched.
    ///
    /// pyro: a box that only moved keeps its own scrollable overflow and transform (both in
    /// its own coordinates), and its rounding is keyed on its layout and offset: only its ancestors,
    /// whose overflow it is part of, are marked.
    pub fn update_unrounded_layout(&mut self, update: impl FnOnce(&mut Layout)) {
        let layout = &mut self.layout_data_mut().unrounded_layout;
        let before = *layout;
        update(layout);
        let after = *layout;
        if after != before {
            let moved_only = Layout { location: before.location, ..after } == before;
            let from = if moved_only { self.parent.or(Some(self.id)) } else { Some(self.id) };
            self.mark_layout_changed(from);
        }
    }

    #[inline]
    pub fn final_layout(&self) -> &Layout {
        &self.layout_data().final_layout
    }

    #[inline]
    pub fn final_layout_mut(&mut self) -> &mut Layout {
        &mut self.layout_data_mut().final_layout
    }

    #[inline]
    pub fn scroll_offset(&self) -> &crate::Point<f64> {
        &self.layout_data().scroll_offset
    }

    #[inline]
    pub fn scroll_offset_mut(&mut self) -> &mut crate::Point<f64> {
        &mut self.layout_data_mut().scroll_offset
    }

    /// pyro: the offset of a `position: sticky` box from its laid-out place
    /// (`LayoutData::sticky_offset`).
    #[inline]
    pub fn sticky_offset(&self) -> crate::Point<f64> {
        self.layout_data().sticky_offset
    }

    /// pyro: sets [`Self::sticky_offset`].
    #[inline]
    pub fn set_sticky_offset(&mut self, offset: crate::Point<f64>) {
        if self.layout_data().sticky_offset != offset {
            self.layout_data_mut().sticky_offset = offset;
        }
    }

    #[inline]
    pub fn scrollable_overflow(&self) -> &KurboRect {
        &self.layout_data().scrollable_overflow
    }

    #[inline]
    pub fn scrollable_overflow_mut(&mut self) -> &mut KurboRect {
        &mut self.layout_data_mut().scrollable_overflow
    }

    /// pyro: see `LayoutData::hit_overflow`.
    #[inline]
    pub fn hit_overflow(&self) -> &KurboRect {
        &self.layout_data().hit_overflow
    }

    #[inline]
    pub fn hit_overflow_mut(&mut self) -> &mut KurboRect {
        &mut self.layout_data_mut().hit_overflow
    }

    /// Style data from stylo, if this node kind carries it (element or document
    /// nodes). Returns `None` for text/comment nodes.
    #[inline]
    pub fn try_stylo_element_data(&self) -> Option<&StyloData> {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                Some(&data.stylo_element_data)
            }
            NodeData::Document(data) => Some(&data.stylo_element_data),
            _ => None,
        }
    }

    #[inline]
    pub fn try_stylo_element_data_mut(&mut self) -> Option<&mut StyloData> {
        match &mut self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                Some(&mut data.stylo_element_data)
            }
            NodeData::Document(data) => Some(&mut data.stylo_element_data),
            _ => None,
        }
    }

    /// The `dirty_descendants` flag, if this node kind carries it (element or
    /// document nodes). Returns `None` for text/comment nodes.
    #[inline]
    fn dirty_descendants_flag(&self) -> Option<&AtomicBool> {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                Some(&data.dirty_descendants)
            }
            NodeData::Document(data) => Some(&data.dirty_descendants),
            _ => None,
        }
    }

    /// The `damaged_descendants` flag, if this node kind carries it (element or
    /// document nodes). Returns `None` for text/comment nodes.
    #[inline]
    fn damaged_descendants_flag(&self) -> Option<&AtomicBool> {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                Some(&data.damaged_descendants)
            }
            NodeData::Document(data) => Some(&data.damaged_descendants),
            _ => None,
        }
    }

    /// The document's shared style lock. Only available on element and
    /// document nodes.
    #[inline]
    pub fn guard(&self) -> &SharedRwLock {
        let guard = match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.guard.as_ref(),
            NodeData::Document(data) => data.guard.as_ref(),
            _ => None,
        };
        guard.expect("`guard` is not available on this node kind")
    }

    #[inline]
    pub fn has_snapshot(&self) -> bool {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.has_snapshot,
            NodeData::Document(data) => data.has_snapshot,
            _ => false,
        }
    }

    #[inline]
    pub fn set_has_snapshot(&mut self, value: bool) {
        match &mut self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.has_snapshot = value,
            NodeData::Document(data) => data.has_snapshot = value,
            _ => {}
        }
    }

    #[inline]
    pub fn before(&self) -> Option<NodeId> {
        self.element_data().and_then(|data| data.before)
    }

    #[inline]
    pub fn after(&self) -> Option<NodeId> {
        self.element_data().and_then(|data| data.after)
    }
}

impl Node {
    pub(crate) fn new(
        tree: *mut crate::NodeTree,
        id: NodeId,
        guard: SharedRwLock,
        mut data: NodeData,
    ) -> Self {
        // Store a handle to the document's shared style lock on the node data.
        // Both element and document nodes are styled by stylo and so need it.
        match &mut data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => {
                data.guard = Some(guard);
            }
            NodeData::Document(data) => data.guard = Some(guard),
            _ => {}
        }

        Self {
            tree,

            id,
            parent: None,
            children: ThinVec::new(),
            layout_parent: Cell::new(None),
            layout_child_of: Cell::new(None),
            oof_containing_block: Cell::new(None),
            layout_children: RefCell::new(None),
            hoisted_children: RefCell::new(ThinVec::new()),
            anonymous_blocks: ThinVec::new(),
            paint_children: RefCell::new(None),
            stacking_context: None,
            sc_contribution_cache: RefCell::new(ThinVec::new()),

            flags: NodeFlags::PAINT_TREE_DIRTY,
            out_of_flow: AtomicBool::new(false),
            data,
        }
    }

    /// pyro: whether a `transform`, `translate`, `rotate` or `scale` style applies,
    /// which [`Self::set_transform`] resolves against the rounded size. A transform that
    /// resolves to the identity at one size (`translate: 50%` at width 0) is stored as
    /// `None`, so `transform().is_some()` alone does not tell whether a new size changes it.
    pub(crate) fn has_transform_style(&self) -> bool {
        use style::values::computed::Rotate;
        use style::values::generics::transform::{Scale, Translate};
        self.primary_styles().is_some_and(|s| {
            let b = s.get_box();
            !b.transform.0.is_empty()
                || !matches!(b.translate, Translate::None)
                || !matches!(b.rotate, Rotate::None)
                || !matches!(b.scale, Scale::None)
        })
    }

    /// pyro: call after changing the scroll offset (`scroll_offset_mut`). The 3D transforms of
    /// the children of an element with a perspective depend on its scroll offset (they move
    /// under a perspective origin that does not), so a scrolled one is resolved again by the
    /// next transform pass (`BaseDocument::resolve_transforms`).
    pub fn scroll_offset_changed(&self) {
        if self.flags.contains(NodeFlags::GIVES_3D_CONTEXT) {
            self.mark_layout_changed(Some(self.id));
        }
    }

    /// pyro: the reference box of the element's `transform`, `transform-origin` and
    /// `perspective-origin` (CSS Transforms 1 `transform-box`), in its border box's coordinates
    /// (CSS px): a CSS box's `content-box` and `fill-box` are its content box, the others its
    /// border box.
    fn transform_reference_box(
        &self,
        transform_box: style::values::computed::TransformBox,
    ) -> euclid::default::Rect<CSSPixelLength> {
        use style::values::computed::TransformBox;
        let layout = self.final_layout();
        let (origin, size) = match transform_box {
            TransformBox::ContentBox | TransformBox::FillBox => (
                (
                    layout.border.left + layout.padding.left,
                    layout.border.top + layout.padding.top,
                ),
                (layout.content_box_width(), layout.content_box_height()),
            ),
            TransformBox::BorderBox | TransformBox::StrokeBox | TransformBox::ViewBox => {
                ((0.0, 0.0), (layout.size.width, layout.size.height))
            }
        };
        Rect::new(
            Point2D::new(CSSPixelLength::new(origin.0), CSSPixelLength::new(origin.1)),
            Size2D::new(CSSPixelLength::new(size.0), CSSPixelLength::new(size.1)),
        )
    }

    /// pyro (lane H2): the plane paint draws the element in through its 4x4 matrix
    /// ([`Plane3D`]), if it is one.
    #[inline]
    pub fn plane_3d(&self) -> Option<&Plane3D> {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.plane_3d.as_deref(),
            _ => None,
        }
    }

    /// pyro (lane H2): `hit` (device px, the node's coordinates; a hit-test bound) in its
    /// parent's coordinates, the node at `location` (device px): through its 2D transform, and for
    /// a plane paint draws through a perspective (`Plane3D::local`) through its homography as
    /// well. Hit testing culls a subtree by this bound: it holds both where the plane is drawn and
    /// where the stand-in puts what is placed by it (an inline element's fragments, a table row).
    pub fn hit_bound_in_parent(&self, location: (f64, f64), scale: f64, hit: KurboRect) -> KurboRect {
        let placed = match self.transform().as_deref() {
            Some(t) => (Affine::translate(location) * *t).transform_rect_bbox(hit),
            None => hit + kurbo::Vec2::new(location.0, location.1),
        };
        let Some(plane) = self.plane_3d().filter(|plane| plane.places_by_local) else {
            return placed;
        };
        const MIN_W: f64 = 1e-6;
        const LIMIT: f64 = 1e7;
        let h = &plane.local;
        let corners = [(hit.x0, hit.y0), (hit.x1, hit.y0), (hit.x1, hit.y1), (hit.x0, hit.y1)].map(|(x, y)| {
            let (x, y) = (x / scale, y / scale);
            [h[0] * x + h[1] * y + h[2], h[3] * x + h[4] * y + h[5], h[6] * x + h[7] * y + h[8]]
        });
        let mut bounds = placed;
        let mut add = |x: f64, y: f64| {
            let (x, y) =
                ((x * scale).clamp(-LIMIT, LIMIT) + location.0, (y * scale).clamp(-LIMIT, LIMIT) + location.1);
            bounds = bounds.union(KurboRect::new(x, y, x, y));
        };
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            if a[2] >= MIN_W {
                add(a[0] / a[2], a[1] / a[2]);
            }
            if (a[2] >= MIN_W) != (b[2] >= MIN_W) {
                let t = (MIN_W - a[2]) / (b[2] - a[2]);
                add((a[0] + (b[0] - a[0]) * t) / MIN_W, (a[1] + (b[1] - a[1]) * t) / MIN_W);
            }
        }
        bounds
    }

    /// pyro: whether the element has `transform-style: preserve-3d`.
    ///
    /// The used value: a grouping property (CSS Transforms 2: `overflow` other than `visible`
    /// and `clip`, `opacity` below 1, a `filter`, `clip-path` or mask, `isolation: isolate`,
    /// a `mix-blend-mode`, paint containment) flattens the element's content first, so its
    /// `transform-style` is `flat`.
    pub(crate) fn is_preserve_3d(&self) -> bool {
        use style::values::computed::Overflow;
        use style::values::specified::box_::Contain;
        let Some(s) = self.primary_styles() else {
            return false;
        };
        let b = s.get_box();
        let groups = |o: Overflow| !matches!(o, Overflow::Visible | Overflow::Clip);
        b.transform_style == style::computed_values::transform_style::T::Preserve3d
            && !groups(b.overflow_x)
            && !groups(b.overflow_y)
            && b.isolation != style::computed_values::isolation::T::Isolate
            && s.get_effects().mix_blend_mode == style::computed_values::mix_blend_mode::T::Normal
            && !b.slow_clone_contain().contains(Contain::PAINT)
            && !self.applies_atomic_paint_effect()
    }

    /// pyro: what this element gives the 3D transforms of its children: its perspective, and
    /// with `transform-style: preserve-3d` its 3D rendering context (`accumulated`, from
    /// [`Self::set_transform`]).
    pub fn transform_context_for_children(
        &self,
        accumulated: Option<Context3D>,
    ) -> TransformContext {
        let scroll = self.scroll_offset();
        let layout = self.final_layout();
        let edges = |r: taffy::Rect<f32>| [r.top, r.right, r.bottom, r.left].map(f64::from);
        let mut context = TransformContext {
            scroll: (scroll.x, scroll.y),
            cb_size: (layout.size.width as f64, layout.size.height as f64),
            cb_border: edges(layout.border),
            cb_padding: edges(layout.padding),
            ..Default::default()
        };
        let Some(s) = self.primary_styles() else {
            return context;
        };
        {
            let b = s.get_border();
            let (w, h) = context.cb_size;
            let r = |c: &style::values::computed::BorderCornerRadius| {
                kurbo::Vec2::new(
                    c.0.width.0.resolve(style::values::computed::CSSPixelLength::new(w as f32)).px() as f64,
                    c.0.height.0.resolve(style::values::computed::CSSPixelLength::new(h as f32)).px() as f64,
                )
            };
            context.cb_radii = [
                r(&b.border_top_left_radius),
                r(&b.border_top_right_radius),
                r(&b.border_bottom_right_radius),
                r(&b.border_bottom_left_radius),
            ];
        }
        let box_styles = s.get_box();
        // pyro: an anonymous box (a table wrapper, row group or row around a cell) passes on the
        // 3D rendering context it is in, as Chromium's do (`accumulated` is its parent's there).
        if self.is_preserve_3d() || self.is_anonymous() {
            context.preserve_3d = accumulated;
        }
        if !matches!(
            box_styles.perspective,
            style::values::generics::box_::GenericPerspective::None
        ) {
            let reference_box = self.transform_reference_box(box_styles.transform_box);
            context.perspective =
                crate::stylo_to_kurbo::resolve_perspective_matrix(box_styles, reference_box);
        }
        context
    }

    /// pyro: `parent`: what the element's parent gives its 3D transform
    /// ([`Self::transform_context_for_children`]). Returns the transform, and the 3D rendering
    /// context it gives its children ([`TransformContext::preserve_3d`]) when the element has
    /// `transform-style: preserve-3d`.
    pub fn set_transform(
        &mut self,
        scale: f32,
        parent: &TransformContext,
    ) -> (Option<Affine>, Option<Context3D>) {
        use crate::stylo_to_kurbo::{Flattened, ResolvedTransform};
        use euclid::default::Transform3D;
        let mut hides = false;
        let mut context = None;
        let mut flattens = false;
        let mut plane: Option<Plane3D> = None;
        let id = self.id;
        let transform = self.primary_styles().and_then(|s| {
            let layout = self.final_layout();
            let size = (layout.size.width as f64, layout.size.height as f64);
            let box_styles = s.get_box();
            let reference_box = self.transform_reference_box(box_styles.transform_box);
            // pyro: the containing block's geometry for `offset-path: ray()`, around the box.
            let motion = (!matches!(box_styles.offset_path, style::values::generics::motion::GenericOffsetPath::None))
                .then(|| crate::stylo_to_kurbo::MotionContainingBlock {
                    border_box: kurbo::Rect::new(
                        -layout.location.x as f64,
                        -layout.location.y as f64,
                        parent.cb_size.0 - layout.location.x as f64,
                        parent.cb_size.1 - layout.location.y as f64,
                    ),
                    border: parent.cb_border,
                    padding: parent.cb_padding,
                    radii: parent.cb_radii,
                });
            let previous = crate::stylo_to_kurbo::MOTION_CONTAINING_BLOCK.with(|c| c.replace(motion));
            let resolved = crate::stylo_to_kurbo::resolve_transform(box_styles, reference_box);
            crate::stylo_to_kurbo::MOTION_CONTAINING_BLOCK.with(|c| c.set(previous));
            // pyro: an anonymous box has no `transform-style` of its own: it is in its parent's
            // 3D rendering context, if any, and passes it on (CSS Transforms 2 WPT
            // transform-table-011: anonymous table boxes around a preserve-3d cell).
            let preserve_3d = self.is_preserve_3d() || (self.is_anonymous() && parent.preserve_3d.is_some());
            flattens = matches!(resolved, ResolvedTransform::Spatial(_)) && !preserve_3d;
            // A 2D transform of a flat element outside a 3D rendering context resolves as it
            // always has.
            if let ResolvedTransform::Flat(flat) = resolved
                && parent.preserve_3d.is_none()
                && !preserve_3d
            {
                return flat.map(|t| to_device_px(t, scale));
            }
            // The matrix from the element's coordinates to its parent's, under the parent's
            // perspective (CSS Transforms 2: the accumulated 3D transformation matrix). The
            // perspective is in the parent's coordinates, which the element's are offset from by
            // its location less the parent's scroll (the perspective origin does not scroll).
            let own = match &resolved {
                ResolvedTransform::Spatial(m) => *m,
                ResolvedTransform::Flat(Some(t)) => {
                    let [a, b, c, d, e, f] = t.as_coeffs();
                    Transform3D::new(
                        a, b, 0.0, 0.0, c, d, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, e, f, 0.0, 1.0,
                    )
                }
                ResolvedTransform::Flat(None) => Transform3D::identity(),
            };
            let (x, y) = (
                layout.location.x as f64 - parent.scroll.0,
                layout.location.y as f64 - parent.scroll.1,
            );
            let mut to_parent = own.then(&Transform3D::translation(x, y, 0.0));
            if let Some(perspective) = &parent.perspective {
                to_parent = to_parent.then(perspective);
            }
            if let ResolvedTransform::Spatial(m) = &resolved {
                // An element whose matrix has no inverse is not rendered (`scale: 1 1 0`
                // flattens to the identity).
                hides = m.determinant() == 0.0;
            }
            let spatial = matches!(resolved, ResolvedTransform::Spatial(_));
            let flat = match &parent.preserve_3d {
                // In its parent's 3D rendering context the element's plane is the projection of
                // its accumulated matrix, which the parent's matrix enters (a child turned 45deg
                // in a parent turned 45deg is turned 90deg), stored relative to the parent's
                // place. Which face shows is the accumulated matrix's too.
                Some(shared) => {
                    let accumulated = to_parent.then(&shared.accumulated);
                    hides |= !preserve_3d && back_face_hidden(box_styles, &accumulated);
                    // pyro (lane H2): an element of the context that Chromium gives a transform
                    // node of its own (a transform, `will-change: transform`, `preserve-3d`,
                    // `backface-visibility: hidden`) is a plane of it, sorted by depth with the
                    // others, and shows or hides by its own face (WPT
                    // `composited-under-rotateY-180deg-preserve-3d`). The rest paints in the plane
                    // of the element it is in, in paint order.
                    let makes_plane = !box_styles.transform.0.is_empty()
                        || box_styles
                            .will_change
                            .bits
                            .intersects(style::values::specified::box_::WillChangeBits::TRANSFORM)
                        || !matches!(box_styles.translate, style::values::computed::Translate::None)
                        || !matches!(box_styles.rotate, style::values::computed::Rotate::None)
                        || !matches!(box_styles.scale, style::values::computed::Scale::None)
                        || preserve_3d
                        || box_styles.backface_visibility
                            == style::computed_values::backface_visibility::T::Hidden;
                    let at = Affine::translate((x, y));
                    // pyro (lane H2): where layout queries put the parent (its frame: from its
                    // coordinates to the context's, those of the root's parent), less the box's
                    // location: a box at its untransformed place in the parent is there.
                    let parent_frame = shared.frame.unwrap_or_else(|| affine_homography(shared.base));
                    let to_parent_place = homography_inverse(&parent_frame)
                        .map(|inverse| homography_mul(&affine_homography(at.inverse()), &inverse));
                    let mut crosses_viewer = false;
                    let mut by_local = false;
                    let mut frame = None;
                    let flat = match crate::stylo_to_kurbo::flatten_transform(&accumulated, size) {
                        Flattened::Hidden => {
                            hides = true;
                            None
                        }
                        // pyro (lane H2): a box crossing the viewer's plane is drawn through its
                        // matrix (paint cuts it there); its projection has no bounds, so layout
                        // keeps it at its untransformed place in its parent, as before it was drawn.
                        Flattened::Crossing => {
                            crosses_viewer = true;
                            if preserve_3d {
                                context = Some(Context3D {
                                    accumulated,
                                    base: shared.base * at,
                                    frame: shared.frame.map(|f| homography_mul(&f, &affine_homography(at))),
                                    root: shared.root,
                                    root_offset: shared.root_offset,
                                });
                            }
                            None
                        }
                        // An element that is no plane paints in its parent's plane at its place
                        // there (paint draws that plane through its matrix): no transform.
                        Flattened::Affine(_) if !makes_plane => None,
                        Flattened::Affine(place) => {
                            let place = if preserve_3d {
                                regularize(place)
                            } else {
                                place
                            };
                            // A plane of a context its parent's frame is a homography of, or one W
                            // varies over, is placed by its homography for layout queries
                            // (`Plane3D::local`); its frame is that homography. The 2D transform
                            // stays the stand-in paint records it with.
                            by_local = !hides
                                && to_parent_place.is_some()
                                && (shared.frame.is_some() || varies_w(&accumulated));
                            if by_local {
                                frame = Some(if varies_w(&accumulated) {
                                    projection_homography(&accumulated)
                                } else {
                                    affine_homography(place)
                                });
                            }
                            if preserve_3d {
                                context = Some(Context3D {
                                    accumulated,
                                    base: place,
                                    frame,
                                    root: shared.root,
                                    root_offset: shared.root_offset,
                                });
                            }
                            let relative =
                                Affine::translate((-x, -y)) * shared.base.inverse() * place;
                            let relative = crate::stylo_to_kurbo::snap(relative);
                            (relative != Affine::IDENTITY).then_some(relative)
                        }
                    };
                    if makes_plane && !hides {
                        let (rx, ry) = shared.root_offset;
                        // Into the parent's coordinates as layout queries place the parent.
                        let own_frame = frame.unwrap_or_else(|| projection_homography(&accumulated));
                        let local = homography_mul(
                            &to_parent_place.unwrap_or_else(|| {
                                affine_homography(Affine::translate((-x, -y)) * shared.base.inverse())
                            }),
                            &own_frame,
                        );
                        plane = Some(Plane3D {
                            root: shared.root,
                            matrix: accumulated.then(&Transform3D::translation(-rx, -ry, 0.0)),
                            own_hidden: preserve_3d && back_face_hidden(box_styles, &accumulated),
                            local,
                            places_by_local: by_local,
                            crosses_viewer,
                        });
                    }
                    flat
                }
                // pyro: a 3D transform is flattened into the plane of the screen, as browsers
                // composite an element whose `transform-style` is `flat`, under its parent's
                // perspective (seen from the front without one).
                None => {
                    hides |= !preserve_3d && back_face_hidden(box_styles, &to_parent);
                    let local = to_parent.then(&Transform3D::translation(-x, -y, 0.0));
                    let mut crosses_viewer = false;
                    let flat = match resolved {
                        ResolvedTransform::Flat(flat) => flat,
                        ResolvedTransform::Spatial(_) => {
                            match crate::stylo_to_kurbo::flatten_transform(&local, size) {
                                Flattened::Affine(affine) => {
                                    (affine != Affine::IDENTITY).then_some(affine)
                                }
                                Flattened::Hidden => {
                                    hides = true;
                                    None
                                }
                                // pyro (lane H2): drawn through its matrix; layout keeps the
                                // untransformed box (see the context case above).
                                Flattened::Crossing => {
                                    crosses_viewer = true;
                                    None
                                }
                            }
                        }
                    };
                    let by_local = !crosses_viewer && varies_w(&local);
                    // The root of a 3D rendering context: its children are placed relative to it.
                    if preserve_3d && !hides {
                        let own = regularize(flat.unwrap_or(Affine::IDENTITY));
                        context = Some(Context3D {
                            accumulated: to_parent,
                            base: Affine::translate((x, y)) * own,
                            frame: by_local.then(|| projection_homography(&to_parent)),
                            root: id,
                            root_offset: (x, y),
                        });
                        plane = Some(Plane3D {
                            root: id,
                            matrix: local,
                            own_hidden: back_face_hidden(box_styles, &to_parent),
                            local: projection_homography(&local),
                            places_by_local: by_local,
                            crosses_viewer,
                        });
                        (own != Affine::IDENTITY).then_some(own)
                    } else {
                        // pyro (lane H2): a flat element its 3D transform maps projectively is
                        // drawn through its matrix; `flat` stands in for layout queries.
                        if spatial && !hides && crate::stylo_to_kurbo::is_projective(&local, size) {
                            plane = Some(Plane3D {
                                root: id,
                                matrix: local,
                                own_hidden: false,
                                local: projection_homography(&local),
                                places_by_local: by_local,
                                crosses_viewer,
                            });
                        }
                        flat
                    }
                }
            };
            flat.map(|t| to_device_px(t, scale))
        });
        self.flags.set(NodeFlags::TRANSFORM_HIDES, hides);
        self.flags.set(NodeFlags::FLATTENS_3D, flattens && !hides && transform.is_some());
        if let Some(data) = self.element_data_mut() {
            match (data.plane_3d.as_deref_mut(), plane) {
                (Some(existing), Some(new)) => *existing = new,
                (None, Some(new)) => data.plane_3d = Some(Box::new(new)),
                (_, None) => data.plane_3d = None,
            }
        }

        let slot = self.transform_mut();
        match (slot.as_deref_mut(), transform) {
            (Some(existing), Some(new)) => *existing = new,
            (None, Some(new)) => *slot = Some(Box::new(new)),
            (_, None) => *slot = None,
        }
        (transform, context)
    }

    pub fn pe_by_index(&self, index: usize) -> Option<NodeId> {
        match index {
            0 => self.after(),
            1 => self.before(),
            _ => panic!("Invalid pseudo element index"),
        }
    }

    pub fn set_pe_by_index(&mut self, index: usize, value: Option<NodeId>) {
        let Some(data) = self.element_data_mut() else {
            return;
        };
        match index {
            0 => data.after = value,
            1 => data.before = value,
            _ => panic!("Invalid pseudo element index"),
        }
    }

    pub(crate) fn display_style(&self) -> Option<StyloDisplay> {
        let display = self.primary_styles().as_ref()?.clone_display();
        Some(
            if self.element_data().is_some_and(|el| el.is_button_input()) {
                stylo_taffy::convert::button_input_display(display)
            } else if self.is_fieldset() {
                stylo_taffy::convert::fieldset_display(display)
            } else {
                display
            },
        )
    }

    /// Whether this is a `<br>` that generates a line break. Like Chrome, a `<br>` is a piece of
    /// inline text in any container: only `display: none` and `display: contents` have an effect
    /// on it (no break). Its other display values (blockified as a flex or grid item, too), its
    /// position, float, box and font properties are all ignored.
    pub fn is_line_break(&self) -> bool {
        self.data.is_element_with_tag_name(&local_name!("br"))
            && !self
                .display_style()
                .is_some_and(|d| matches!(d.inside(), DisplayInside::None | DisplayInside::Contents))
    }

    /// pyro: whether the element's `::before` or `::after` generates an in-flow block-level box
    /// (`display: block` and the like, with content, neither floated nor absolutely positioned).
    fn has_block_level_pseudo_element(&self) -> bool {
        use style::values::generics::counters::Content;
        let Some(data) = self.try_stylo_element_data().and_then(|data| data.get()) else {
            return false;
        };
        if data.styles.pseudos.is_empty() {
            return false;
        }
        data.styles.pseudos.as_array()[..2].iter().flatten().any(|style| {
            !matches!(style.get_counters().content, Content::Normal | Content::None)
                && style.clone_display().outside() == DisplayOutside::Block
                && !style.clone_float().is_floating()
                && !style.clone_position().is_absolutely_positioned()
        })
    }

    pub fn is_or_contains_block(&self) -> bool {
        if self.is_line_break() {
            return false;
        }
        let style = self.primary_styles();
        let style = style.as_ref();

        // Ignore out-of-flow items
        let position = style
            .map(|s| s.clone_position())
            .unwrap_or(Position::Relative);
        let is_in_flow = matches!(
            position,
            Position::Static | Position::Relative | Position::Sticky
        );
        if !is_in_flow {
            return false;
        }
        // Floated boxes do not break up the inline flow: they participate in the
        // inline formatting context as out-of-flow inline boxes
        let is_floating = style
            .map(|s| s.clone_float().is_floating())
            .unwrap_or(false);
        if is_floating {
            return false;
        }
        let display = self.display_style().unwrap_or(StyloDisplay::inline());
        match display.outside() {
            DisplayOutside::None => false,
            DisplayOutside::Block => true,
            _ => {
                // pyro: an atomic inline box (a replaced element, an `<object>` showing an
                // image) contains no block for its container: its children are its own
                // content (`<svg>`) or are not rendered (an object's fallback).
                if self.element_data().is_some_and(crate::layout::replaced::is_inline_box) {
                    return false;
                }
                if display.inside() == DisplayInside::Flow {
                    // pyro: so is a block-level `::before` or `::after` (read from its style: its
                    // box is made when this inline box is built, after its container asks).
                    self.has_block_level_pseudo_element()
                        || self
                            .children
                            .iter()
                            .copied()
                            .any(|child_id| crate::util::ensure_stack(|| self.tree()[child_id].is_or_contains_block()))
                } else {
                    false
                }
            }
        }
    }

    /// Whether box-tree construction found this to be an inline box with block-level in-flow content only.
    /// It is laid out as a block box, so its own background and borders must not be painted
    /// over that content. Inline text or inline children keep the painting.
    pub fn is_inline_with_only_block_children(&self) -> bool {
        self.flags
            .contains(NodeFlags::IS_INLINE_WITH_ONLY_BLOCK_CHILDREN)
    }

    /// pyro: whether box-tree construction split this inline box around block-level content
    /// (`NodeFlags::IS_SPLIT_INLINE`): the anonymous blocks around and inside it paint its
    /// fragments, so it paints no box of its own.
    pub fn is_split_inline(&self) -> bool {
        self.flags.contains(NodeFlags::IS_SPLIT_INLINE)
    }

    /// pyro: whether the child at `index` of this anonymous block (of `len` children) stands for a
    /// split inline's head or tail (`NodeFlags::ENDS_WITH_SPLIT_HEAD`, `STARTS_WITH_SPLIT_TAIL`):
    /// `Some(true)` for a tail, `Some(false)` for a head.
    pub(crate) fn split_ref_at(&self, index: usize, len: usize) -> Option<bool> {
        if index == 0 && self.flags.contains(NodeFlags::STARTS_WITH_SPLIT_TAIL) {
            Some(true)
        } else if index + 1 == len && self.flags.contains(NodeFlags::ENDS_WITH_SPLIT_HEAD) {
            Some(false)
        } else {
            None
        }
    }

    pub fn is_whitespace_node(&self) -> bool {
        match &self.data {
            NodeData::Text(data) => data.content.chars().all(|c| c.is_ascii_whitespace()),
            _ => false,
        }
    }

    /// Whether this is an empty or whitespace-only text node whose whitespace may be collapsed
    /// away under the `white-space-collapse` it inherits from its parent.
    pub fn is_collapsible_whitespace_node(&self) -> bool {
        let NodeData::Text(data) = &self.data else {
            return false;
        };
        if data.content.is_empty() {
            return true;
        }
        if !data.content.chars().all(|c| c.is_ascii_whitespace()) {
            return false;
        }
        let white_space_collapse = self
            .parent
            .and_then(|parent_id| self.with(parent_id).primary_styles())
            .map(|style| style.clone_white_space_collapse());
        match white_space_collapse {
            Some(
                WhiteSpaceCollapse::Preserve
                | WhiteSpaceCollapse::BreakSpaces
                | WhiteSpaceCollapse::PreserveSpaces,
            ) => false,
            Some(WhiteSpaceCollapse::PreserveBreaks) => !data.content.contains('\n'),
            Some(WhiteSpaceCollapse::Collapse) | None => true,
        }
    }

    pub fn is_focussable(&self) -> bool {
        self.data
            .downcast_element()
            .map(|el| el.is_focussable)
            .unwrap_or(false)
    }

    pub fn set_restyle_hint(&mut self, hint: RestyleHint) {
        if let Some(stylo_element_data) = self.try_stylo_element_data_mut() {
            if let Some(mut element_data) = stylo_element_data.get_mut() {
                element_data.hint.insert(hint);
            }
        }
        // Mark all ancestors as having dirty descendants so the style traversal
        // will visit this node's subtree
        self.mark_ancestors_dirty();
    }

    /// Returns whether this node has any descendants that need restyling.
    pub fn has_dirty_descendants(&self) -> bool {
        self.dirty_descendants_flag()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Sets the dirty_descendants flag on this node.
    pub fn set_dirty_descendants(&self) {
        if let Some(flag) = self.dirty_descendants_flag() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Clears the dirty_descendants flag on this node.
    pub fn unset_dirty_descendants(&self) {
        if let Some(flag) = self.dirty_descendants_flag() {
            flag.store(false, Ordering::Relaxed);
        }
    }

    /// Marks all ancestors of this node as having dirty descendants.
    /// This propagates the dirty flag up the tree so that the style traversal
    /// knows to visit the subtree containing this node.
    pub fn mark_ancestors_dirty(&self) {
        let mut current_id = self.parent;
        while let Some(parent_id) = current_id {
            let parent = &self.tree()[parent_id];
            // If this ancestor already has dirty_descendants set, we can stop
            // because all further ancestors must also have it set
            if let Some(flag) = parent.dirty_descendants_flag() {
                if flag.swap(true, Ordering::Relaxed) {
                    break;
                }
            }
            current_id = parent.parent;
        }
    }

    /// Returns whether this node or any of its descendants may carry damage.
    pub fn has_damaged_descendants(&self) -> bool {
        self.damaged_descendants_flag()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// pyro: whether a pass that follows damage down the layout tree has nothing to do
    /// in this node's subtree: an element (not an anonymous box, whose flag is not maintained)
    /// whose `damaged_descendants` flag is clear, still listed by `layout_parent`. By the invariant
    /// [`Self::mark_damaged`] keeps, such a node carries no damage, nor does anything below it.
    /// The pass skips it from its parent's loop instead of entering it to return: a cell edit in a
    /// long table visits that cell, not its thousands of siblings.
    ///
    /// (A text node is not one: `resolve_layout_children` takes a node with no style data as
    /// damaged.)
    #[inline]
    pub(crate) fn is_clean_layout_child_of(&self, layout_parent: NodeId) -> bool {
        let clean = matches!(self.data, NodeData::Element(_))
            && !self.has_damaged_descendants()
            && self.layout_parent.get() == Some(layout_parent);
        #[cfg(debug_assertions)]
        if clean {
            debug_assert!(
                self.damage().is_some_and(|damage| damage.is_empty()),
                "{:?} ({:?}) is an unflagged layout child with damage {:?}",
                self.id,
                self.data.downcast_element().map(|el| &el.name.local),
                self.damage(),
            );
        }
        clean
    }

    /// pyro: whether the DOM walks that follow damage (`propagate_damage_flags`,
    /// `clear_damage_and_dirty_flags`) have nothing to do in this node's subtree: it is not an
    /// anonymous box (whose flag is not maintained) and its `damaged_descendants` flag is clear
    /// (a text or comment node has none, nor damage). By the invariant [`Self::mark_damaged`]
    /// keeps, it carries no damage, nor does anything below it, and the walk skips it from its
    /// parent's loop instead of entering it to return: a cell edit in a long table enters that
    /// row, not its thousands of siblings.
    #[inline]
    pub(crate) fn has_no_damage_below(&self) -> bool {
        let clean = !self.is_anonymous() && !self.has_damaged_descendants();
        #[cfg(debug_assertions)]
        if clean {
            debug_assert!(
                !self.damage().is_some_and(|damage| !damage.is_empty()),
                "{:?} ({:?}) carries damage {:?} without its damaged_descendants flag",
                self.id,
                self.data.downcast_element().map(|el| &el.name.local),
                self.damage(),
            );
        }
        clean
    }

    /// Clears the damaged_descendants flag on this node.
    pub fn unset_damaged_descendants(&self) {
        if let Some(flag) = self.damaged_descendants_flag() {
            flag.store(false, Ordering::Relaxed);
        }
    }

    /// Marks this node and all of its ancestors as (potentially) carrying
    /// damage, so that the damage propagation pass visits this node's subtree.
    ///
    /// The invariant is: if a node carries damage (or needs damage-phase
    /// processing such as pseudo-element style syncing), then it and all of
    /// its ancestors have `damaged_descendants` set.
    pub fn mark_damaged(&self) {
        if let Some(flag) = self.damaged_descendants_flag() {
            if flag.swap(true, Ordering::Relaxed) {
                #[cfg(debug_assertions)]
                self.debug_assert_ancestors_flagged(self);
                return;
            }
        }
        let mut current_id = self.parent;
        while let Some(parent_id) = current_id {
            let parent = &self.tree()[parent_id];
            // If this ancestor already has damaged_descendants set, we can stop
            // because all further ancestors must also have it set
            if let Some(flag) = parent.damaged_descendants_flag() {
                if flag.swap(true, Ordering::Relaxed) {
                    #[cfg(debug_assertions)]
                    self.debug_assert_ancestors_flagged(parent);
                    break;
                }
            }
            current_id = parent.parent;
        }
    }

    /// [`Self::mark_damaged`], from `from`: marks it and its ancestors as having damaged
    /// descendants (a layout change: `update_unrounded_layout`, a rounded size a transform
    /// resolves against).
    ///
    /// pyro: the walk stops at a node already flagged, as `mark_damaged` does. A flag is
    /// only set together with those of every ancestor (here, in `mark_damaged`, and a node joining
    /// the tree flags its new parent: `add_children_to_parent`), and
    /// `clear_damage_and_dirty_flags` clears a node's flag after those below it, so a flagged
    /// node's ancestors are flagged. The rows below a cell that grew used to walk to the document
    /// each: thousands of walks per edit of a long table. An anonymous box's flag is not maintained
    /// (`rounded_unchanged`): the walk goes on past it.
    pub(crate) fn mark_layout_changed(&self, from: Option<NodeId>) {
        let mut current = from;
        while let Some(id) = current {
            let node = &self.tree()[id];
            if let Some(flag) = node.damaged_descendants_flag() {
                if flag.swap(true, Ordering::Relaxed) && !node.is_anonymous() {
                    #[cfg(debug_assertions)]
                    self.debug_assert_ancestors_flagged(node);
                    return;
                }
            }
            current = node.parent;
        }
    }

    /// The invariant the early exits of [`Self::mark_damaged`] and [`Self::mark_layout_changed`]
    /// rely on: a flagged node's ancestors are flagged (anonymous boxes' flags aside, which are
    /// not maintained). A violation leaves a flag, and the damage below it, out of reach of
    /// `clear_damage_and_dirty_flags` and lets rounding skip a subtree whose layout changed.
    #[cfg(debug_assertions)]
    fn debug_assert_ancestors_flagged(&self, flagged: &Node) {
        let mut current = flagged.parent;
        while let Some(id) = current {
            let ancestor = &self.tree()[id];
            if let Some(flag) = ancestor.damaged_descendants_flag() {
                debug_assert!(
                    flag.load(Ordering::Relaxed) || ancestor.is_anonymous(),
                    "{:?} ({:?}) has damaged_descendants set while its ancestor {:?} ({:?}) has not",
                    flagged.id,
                    flagged.data.downcast_element().map(|el| &el.name.local),
                    ancestor.id,
                    ancestor.data.downcast_element().map(|el| &el.name.local),
                );
            }
            current = ancestor.parent;
        }
    }

    // pub fn damage_mut(&mut self) -> Option<&mut RestyleDamage> {
    //     self.stylo_element_data
    //         .get_mut()
    //         .map(|mut data: ElementDataMut<'a>| &'a mut data.damage)
    // }

    pub fn damage(&self) -> Option<RestyleDamage> {
        self.try_stylo_element_data()
            .and_then(|stylo| stylo.get().map(|data| data.damage))
    }

    pub fn set_damage(&mut self, damage: RestyleDamage) {
        if let Some(stylo) = self.try_stylo_element_data_mut() {
            if let Some(mut data) = stylo.get_mut() {
                data.damage = damage;
            }
        }
    }

    pub fn insert_damage(&mut self, damage: RestyleDamage) {
        if let Some(stylo) = self.try_stylo_element_data_mut() {
            if let Some(mut data) = stylo.get_mut() {
                data.damage |= damage;
            }
        }
        if !damage.is_empty() {
            self.mark_damaged();
        }
    }

    pub fn remove_damage(&mut self, damage: RestyleDamage) {
        if let Some(stylo) = self.try_stylo_element_data_mut() {
            if let Some(mut data) = stylo.get_mut() {
                data.damage.remove(damage);
            }
        }
    }

    pub fn clear_damage_mut(&mut self) {
        if let Some(stylo) = self.try_stylo_element_data_mut() {
            if let Some(mut data) = stylo.get_mut() {
                data.damage = RestyleDamage::empty();
            }
        }
    }

    // State changes (hover/focus/active/disabled) do not set a restyle hint.
    // Invalidation is driven by element snapshots: the style traversal diffs the
    // snapshotted (pre-change) state against the current state and invalidates
    // only the elements matched by selectors that depend on the changed state
    // bits. Ancestors are marked dirty so the traversal reaches this node.
    pub fn hover(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.insert(ElementState::HOVER);
        }
        self.mark_ancestors_dirty();
    }

    pub fn unhover(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.remove(ElementState::HOVER);
        }
        self.mark_ancestors_dirty();
    }

    pub fn is_hovered(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.element_state.contains(ElementState::HOVER))
    }

    /// Mark this node as containing focus (`:focus-within`). Set on the
    /// focused element and all of its DOM ancestors.
    pub fn add_focus_within(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.insert(ElementState::FOCUS_WITHIN);
        }
        self.mark_ancestors_dirty();
    }

    /// Mark this node as no longer containing focus (`:focus-within`).
    pub fn remove_focus_within(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.remove(ElementState::FOCUS_WITHIN);
        }
        self.mark_ancestors_dirty();
    }

    /// Whether this node is the focused node or one of its DOM ancestors (`:focus-within`).
    pub fn contains_focus(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.element_state.contains(ElementState::FOCUS_WITHIN))
    }

    pub fn focus(&mut self, shell_provider: Arc<dyn ShellProvider>) {
        if let Some(data) = self.element_data_mut() {
            // FOCUSRING (`:focus-visible`) is the document's call.
            data.element_state.insert(ElementState::FOCUS);
        }
        self.mark_ancestors_dirty();

        // If focussing a text input, enable IME and set IME area
        if self
            .element_data()
            .and_then(|elem| elem.text_input_data())
            .is_some()
        {
            shell_provider.set_ime_enabled(true);
            let mut pos = self.absolute_position(0.0, 0.0);
            pos.x += self.final_layout().content_box_x();
            pos.y += self.final_layout().content_box_y();
            let width = self.final_layout().content_box_width();
            let height = self.final_layout().content_box_height();
            shell_provider.set_ime_cursor_area(pos.x, pos.y, width, height);
        }
    }

    pub fn blur(&mut self, shell_provider: Arc<dyn ShellProvider>) {
        if let Some(data) = self.element_data_mut() {
            data.element_state
                .remove(ElementState::FOCUS | ElementState::FOCUSRING);
        }
        self.mark_ancestors_dirty();

        // If blurring a text input, disable IME
        if self
            .element_data()
            .and_then(|elem| elem.text_input_data())
            .is_some()
        {
            shell_provider.set_ime_enabled(false);
        }
    }

    pub fn is_focussed(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.element_state.contains(ElementState::FOCUS))
    }

    pub fn active(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.insert(ElementState::ACTIVE);
        }
        self.mark_ancestors_dirty();
    }

    pub fn unactive(&mut self) {
        if let Some(data) = self.element_data_mut() {
            data.element_state.remove(ElementState::ACTIVE);
        }
        self.mark_ancestors_dirty();
    }

    pub fn is_active(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.element_state.contains(ElementState::ACTIVE))
    }

    // Marks the node as disabled if it can be.
    // It does not disable any children which should be disabled as well (relevant for the `select` element).
    pub fn disable(&mut self) {
        if let Some(data) = self.element_data_mut() {
            if data.can_be_disabled() {
                data.element_state.insert(ElementState::DISABLED);
                data.element_state.remove(ElementState::ENABLED);
            }
        }
        self.mark_ancestors_dirty();
    }

    // Marks the node as enabled if it can be.
    // It does not enable any children which should be enabled as well (relevant for the `select` element).
    pub fn enable(&mut self) {
        if let Some(data) = self.element_data_mut() {
            if data.can_be_disabled() {
                data.element_state.insert(ElementState::ENABLED);
                data.element_state.remove(ElementState::DISABLED);
            }
        }
        self.mark_ancestors_dirty();
    }

    pub fn subdoc(&self) -> Option<&dyn Document> {
        self.element_data().and_then(|el| el.sub_doc_data())
    }

    pub fn subdoc_mut(&mut self) -> Option<&mut dyn Document> {
        self.element_data_mut().and_then(|el| el.sub_doc_data_mut())
    }

    pub fn text_input_v_centering_offset(&self, scale: f64) -> f64 {
        // For single-line inputs, add an offset to vertically center the text input layout
        // within the content box of it's node.
        if let Some(input_data) = self
            .data
            .downcast_element()
            .and_then(|el| el.text_input_data())
        {
            if !input_data.is_multiline {
                // pyro: in the unrounded box, as Chromium centres in layout units: a 33.5px
                // input's line is 8.25px down (painted from 8px), not 8.5px (9px).
                let content_box_height = self.unrounded_layout().content_box_height();
                let input_height = input_data.editor.try_layout().unwrap().height() / scale as f32;
                // pyro: a line taller than the content box is centred too, overflowing both
                // edges (Blink; WPT css-overflow `input-scrollable-region-001`)
                let y_offset = (content_box_height - input_height) / 2.0;

                return y_offset as f64;
            }
        }

        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeKind {
    Document,
    Element,
    AnonymousBlock,
    Text,
    Comment,
}

/// The different kinds of nodes in the DOM.
#[derive(Debug, Clone)]
pub enum NodeData {
    /// The `Document` itself - the root node of a HTML document.
    Document(Box<DocumentData>),

    /// An element with attributes.
    Element(Box<ElementData>),

    /// An anonymous block box
    AnonymousBlock(Box<ElementData>),

    /// A text node.
    Text(TextNodeData),

    /// A comment.
    Comment {
        /// The textual content of the comment
        contents: String,
    },
    // /// A `DOCTYPE` with name, public id, and system id. See
    // /// [document type declaration on wikipedia][https://en.wikipedia.org/wiki/Document_type_declaration]
    // Doctype { name: String, public_id: String, system_id: String },

    // /// A Processing instruction.
    // ProcessingInstruction { target: String, contents: String },
}

impl NodeData {
    pub fn downcast_element(&self) -> Option<&ElementData> {
        match self {
            Self::Element(data) => Some(data),
            Self::AnonymousBlock(data) => Some(data),
            _ => None,
        }
    }

    pub fn downcast_element_mut(&mut self) -> Option<&mut ElementData> {
        match self {
            Self::Element(data) => Some(data),
            Self::AnonymousBlock(data) => Some(data),
            _ => None,
        }
    }

    pub fn is_element_with_tag_name(&self, name: &impl PartialEq<LocalName>) -> bool {
        let Some(elem) = self.downcast_element() else {
            return false;
        };
        *name == elem.name.local
    }

    pub fn attrs(&self) -> Option<&[Attribute]> {
        Some(&self.downcast_element()?.attrs)
    }

    pub fn attr(&self, name: impl PartialEq<LocalName>) -> Option<&str> {
        self.downcast_element()?.attr(name)
    }

    pub fn has_attr(&self, name: impl PartialEq<LocalName>) -> bool {
        self.downcast_element()
            .is_some_and(|elem| elem.has_attr(name))
    }

    pub fn kind(&self) -> NodeKind {
        match self {
            NodeData::Document(_) => NodeKind::Document,
            NodeData::Element(_) => NodeKind::Element,
            NodeData::AnonymousBlock(_) => NodeKind::AnonymousBlock,
            NodeData::Text(_) => NodeKind::Text,
            NodeData::Comment { .. } => NodeKind::Comment,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextNodeData {
    /// The textual content of the text node
    pub content: String,
}

impl TextNodeData {
    pub fn new(content: String) -> Self {
        Self { content }
    }
}

/*
-> Computed styles
-> Layout
-----> Needs to happen only when styles are computed
*/

// type DomRefCell<T> = RefCell<T>;

// pub struct DomData {
//     // ... we can probs just get away with using the html5ever types directly. basically just using the servo dom, but without the bindings
//     local_name: html5ever::LocalName,
//     tag_name: html5ever::QualName,
//     namespace: html5ever::Namespace,
//     prefix: DomRefCell<Option<html5ever::Prefix>>,
//     attrs: DomRefCell<Vec<Attr>>,
//     // attrs: DomRefCell<Vec<Dom<Attr>>>,
//     id_attribute: DomRefCell<Option<Atom>>,
//     is: DomRefCell<Option<LocalName>>,
//     // style_attribute: DomRefCell<Option<Arc<Locked<PropertyDeclarationBlock>>>>,
//     // attr_list: MutNullableDom<NamedNodeMap>,
//     // class_list: MutNullableDom<DOMTokenList>,
//     state: Cell<ElementState>,
// }

impl Node {
    pub fn tree(&self) -> &crate::NodeTree {
        unsafe { &*self.tree }
    }

    #[track_caller]
    pub fn with(&self, id: NodeId) -> &Node {
        self.tree().get(id).unwrap()
    }

    pub fn print_tree(&self, level: usize) {
        println!(
            "{} {} {:?} {} {:?}",
            "  ".repeat(level),
            self.id,
            self.parent,
            self.node_debug_str().replace('\n', ""),
            self.children
        );
        // println!("{} {:?}", "  ".repeat(level), self.children);
        for child_id in self.children.iter() {
            let child = self.with(*child_id);
            child.print_tree(level + 1)
        }
    }

    // Get the index of the current node in the parents child list
    pub fn index_of_child(&self, child_id: NodeId) -> Option<usize> {
        self.children.iter().position(|id| *id == child_id)
    }

    // Get the index of the current node in the parents child list
    pub fn child_index(&self) -> Option<usize> {
        self.tree()[self.parent?]
            .children
            .iter()
            .position(|id| *id == self.id)
    }

    // Get the nth node in the parents child list
    pub fn forward(&self, n: usize) -> Option<&Node> {
        let child_idx = self.child_index().unwrap_or(0);
        self.tree()[self.parent?]
            .children
            .get(child_idx + n)
            .map(|id| self.with(*id))
    }

    pub fn backward(&self, n: usize) -> Option<&Node> {
        let child_idx = self.child_index().unwrap_or(0);
        if child_idx < n {
            return None;
        }

        self.tree()[self.parent?]
            .children
            .get(child_idx - n)
            .map(|id| self.with(*id))
    }

    pub fn is_element(&self) -> bool {
        matches!(self.data, NodeData::Element { .. })
    }

    /// The HTML root has the document node as its containing block.
    pub fn is_root_element(&self) -> bool {
        self.is_element() && self.parent.is_some_and(|parent| matches!(self.with(parent).data, NodeData::Document(_)))
    }

    pub fn is_anonymous(&self) -> bool {
        matches!(self.data, NodeData::AnonymousBlock { .. })
    }

    pub fn is_text_node(&self) -> bool {
        matches!(self.data, NodeData::Text { .. })
    }

    pub fn element_data(&self) -> Option<&ElementData> {
        match self.data {
            NodeData::Element(ref data) => Some(data),
            NodeData::AnonymousBlock(ref data) => Some(data),
            _ => None,
        }
    }

    pub fn element_data_mut(&mut self) -> Option<&mut ElementData> {
        match self.data {
            NodeData::Element(ref mut data) => Some(data),
            NodeData::AnonymousBlock(ref mut data) => Some(data),
            _ => None,
        }
    }

    pub fn text_data(&self) -> Option<&TextNodeData> {
        match self.data {
            NodeData::Text(ref data) => Some(data),
            _ => None,
        }
    }

    pub fn text_data_mut(&mut self) -> Option<&mut TextNodeData> {
        match self.data {
            NodeData::Text(ref mut data) => Some(data),
            _ => None,
        }
    }

    pub fn node_debug_str(&self) -> String {
        let mut s = String::new();

        match &self.data {
            NodeData::Document(_) => write!(s, "DOCUMENT"),
            // NodeData::Doctype { name, .. } => write!(s, "DOCTYPE {name}"),
            NodeData::Text(data) => {
                let bytes = data.content.as_bytes();
                write!(
                    s,
                    "TEXT {}",
                    std::str::from_utf8(bytes.split_at(10.min(bytes.len())).0)
                        .unwrap_or("INVALID UTF8")
                )
            }
            NodeData::Comment { .. } => write!(s, "COMMENT"),
            NodeData::AnonymousBlock(_) => write!(s, "AnonymousBlock"),
            NodeData::Element(data) => {
                let name = &data.name;
                let class = self.attr(local_name!("class")).unwrap_or("");
                let id = self.attr(local_name!("id")).unwrap_or("");
                let display = self.display_constructed_as().to_css_string();
                write!(s, "<{}", name.local).unwrap();
                if !id.is_empty() {
                    write!(s, " #{id}").unwrap();
                }
                if !class.is_empty() {
                    if class.contains(' ') {
                        write!(s, " class=\"{class}\"").unwrap()
                    } else {
                        write!(s, " .{class}").unwrap()
                    }
                }
                write!(s, "> ({display})")
            } // NodeData::ProcessingInstruction { .. } => write!(s, "ProcessingInstruction"),
        }
        .unwrap();
        s
    }

    pub fn attrs(&self) -> Option<&[Attribute]> {
        Some(&self.element_data()?.attrs)
    }

    pub fn attr(&self, name: LocalName) -> Option<&str> {
        let attr = self.attrs()?.iter().find(|id| id.name.local == name)?;
        Some(&attr.value)
    }

    pub fn primary_styles(&self) -> Option<impl Deref<Target = ServoArc<ComputedValues>>> {
        self.try_stylo_element_data()
            .and_then(|stylo| stylo.primary_styles())
    }

    /// A lazy Taffy style backed by this node's primary stylo style.
    ///
    /// Panics if the node has no computed styles: layout always runs after
    /// styling, and the only unstyled nodes (text, comments, descendants of
    /// `display: none`) are never queried by Taffy.
    pub fn layout_style(&self) -> stylo_taffy::TaffyStyloStyle<ComputedStyleRef<'_>> {
        let styles = self
            .try_stylo_element_data()
            .and_then(|stylo| stylo.layout_computed_styles())
            .expect("layout_style() called on a node without computed styles");
        #[cfg(debug_assertions)]
        self.assert_layout_styles_are_current();

        let mut flags = stylo_taffy::StyleFlags::empty();
        let mut natural_ratio = None;
        if let Some(el) = self.data.downcast_element() {
            if crate::layout::replaced::is_replaced(el) {
                flags |= stylo_taffy::StyleFlags::IS_REPLACED;
                if crate::layout::replaced::is_content_image(el) {
                    flags |= stylo_taffy::StyleFlags::IS_CONTENT_REPLACED;
                }
                // pyro
                if styles.get_position().aspect_ratio.auto && crate::layout::replaced::has_natural_ratio(el) {
                    flags |= stylo_taffy::StyleFlags::HAS_NATURAL_RATIO;
                    natural_ratio = crate::layout::replaced::natural_ratio(el);
                }
            }
            if el.name.local == local_name!("button") {
                flags |= stylo_taffy::StyleFlags::IS_BUTTON;
            }
            if el.is_button_input() {
                flags |= stylo_taffy::StyleFlags::IS_BUTTON_INPUT;
            }
            if el.name.local == local_name!("fieldset") {
                flags |= stylo_taffy::StyleFlags::IS_FIELDSET;
            }
            // pyro
            if el.name.local == local_name!("input") {
                flags |= stylo_taffy::StyleFlags::IS_INPUT;
            }
            if let Some(inline_layout) = el.inline_layout_data.as_deref() {
                flags |= inline_layout.span_cb_flags;
            }
        }
        if self.flags.contains(NodeFlags::IS_SPLIT_INLINE) {
            flags |= stylo_taffy::StyleFlags::IS_SPLIT_INLINE;
        }
        // pyro
        if self.flags.contains(NodeFlags::OVERFLOW_TO_VIEWPORT) {
            flags |= stylo_taffy::StyleFlags::OVERFLOW_TO_VIEWPORT;
        }

        #[cfg(feature = "writing-mode")]
        {
            // Own-layout_wm by default: the node's own algorithm reads its style in its own axes
            let layout_wm = styles.writing_mode;
            let mut style = stylo_taffy::TaffyStyloStyle::new_in(styles, flags, layout_wm);
            style.natural_ratio = natural_ratio;
            style
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            let mut style = stylo_taffy::TaffyStyloStyle::new(styles, flags);
            style.natural_ratio = natural_ratio;
            style
        }
    }

    /// The half collapsed borders of a cell in a table without uniform ones.
    pub(crate) fn collapsed_border(&self) -> Option<[f32; 4]> {
        match &self.data {
            NodeData::Element(data) | NodeData::AnonymousBlock(data) => data.layout_data().collapsed_border,
            _ => None,
        }
    }

    /// pyro: rewrite the layout styles derived from the node's primary style
    /// ([`LayoutStyles`]). Called wherever the primary style is replaced outside the style
    /// traversal, and when a cell's collapsed borders change.
    pub(crate) fn update_layout_styles(&mut self) {
        let collapsed_border = self.collapsed_border();
        let fieldset = self.is_fieldset();
        let Some(stylo) = self.try_stylo_element_data_mut() else {
            return;
        };
        let derived = match stylo.get().as_ref().and_then(|data| data.styles.get_primary()) {
            Some(style) => LayoutStyles::derive(style, collapsed_border, fieldset),
            None => LayoutStyles::default(),
        };
        *stylo.layout_styles_mut() = derived;
    }

    /// Set a table cell's half collapsed borders, and its layout style with them. The cell's
    /// table-part Taffy style does not depend on them and stays.
    ///
    /// pyro: the cell's own layout reads its layout style, so a change clears what its
    /// last layout measured (`clear_layout_cache`), whichever path changed it: a table built
    /// again (`build_table_context`) leaves its undamaged cells' layouts in place otherwise.
    pub(crate) fn set_collapsed_border(&mut self, collapsed_border: Option<[f32; 4]>) {
        if self.collapsed_border() == collapsed_border {
            return;
        }
        self.layout_data_mut().collapsed_border = collapsed_border;
        self.clear_layout_cache();
        let Some(stylo) = self.try_stylo_element_data_mut() else {
            return;
        };
        let cell = match stylo.get().as_ref().and_then(|data| data.styles.get_primary()) {
            Some(style) => super::stylo_data::cell_layout_style(style, collapsed_border),
            None => None,
        };
        stylo.layout_styles_mut().cell = cell;
    }

    /// The physical Taffy style of a table, table row, column or cell, converted when its style
    /// was written.
    pub(crate) fn table_part_taffy_style(&self) -> Option<&taffy::Style<style::Atom>> {
        let style = self.try_stylo_element_data()?.layout_styles().table_part.as_deref();
        #[cfg(debug_assertions)]
        self.assert_layout_styles_are_current();
        style
    }

    /// Debug builds: the stored layout styles are those the primary style gives now.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_layout_styles_are_current(&self) {
        let Some(stylo) = self.try_stylo_element_data() else {
            return;
        };
        let Some(primary) = stylo.primary_styles() else {
            return;
        };
        stylo
            .layout_styles()
            .assert_current(&primary, self.collapsed_border(), self.is_fieldset(), self.id);
    }

    /// pyro: whether this is a `<fieldset>` element (laid out by its own rules whatever its
    /// `display`: `stylo_taffy::convert::fieldset_display`).
    #[inline]
    pub(crate) fn is_fieldset(&self) -> bool {
        self.data.downcast_element().is_some_and(|el| el.name.local == local_name!("fieldset"))
    }

    /// The node's style for Taffy, expressed in the axes of the writing mode `layout_wm` (the writing
    /// mode of the box laying this node out). Used for the child-style getters.
    #[cfg(feature = "writing-mode")]
    pub fn layout_style_in(
        &self,
        layout_wm: stylo_taffy::WritingMode,
    ) -> stylo_taffy::TaffyStyloStyle<ComputedStyleRef<'_>> {
        let mut style = self.layout_style();
        style.layout_wm = layout_wm;
        style
    }

    /// Whether the node has any `contain` value, which blocks `writing-mode` propagation from
    /// `<body>` to the root (css-writing-modes-3 §8).
    #[cfg(feature = "writing-mode")]
    pub(crate) fn has_containment(&self) -> bool {
        self.primary_styles()
            .is_some_and(|style| !style.clone_contain().is_empty())
    }

    /// Whether the node is a flex container with a column `flex-direction`, i.e. aligns its
    /// children's `align-self` in its inline axis.
    #[cfg(feature = "writing-mode")]
    pub(crate) fn is_column_flex_container(&self) -> bool {
        self.primary_styles().is_some_and(|s| {
            s.clone_display().inside() == DisplayInside::Flex
                && !self.element_data().is_some_and(|el| el.is_button_input())
                && matches!(
                    s.get_position().flex_direction,
                    FlexDirection::Column | FlexDirection::ColumnReverse
                )
        })
    }

    /// The node's computed `writing-mode` (horizontal-tb for nodes without styles)
    #[cfg(feature = "writing-mode")]
    pub fn writing_mode(&self) -> stylo_taffy::WritingMode {
        self.primary_styles()
            .map(|s| s.writing_mode)
            .unwrap_or(stylo_taffy::WritingMode::empty())
    }

    /// Whether the node's layout algorithm ran in a vertical writing mode: `None` if not, else whether
    /// its block axis runs left to right.
    #[inline]
    pub fn vertical_block_axis(&self) -> Option<bool> {
        #[cfg(feature = "writing-mode")]
        {
            let wm = self.layout_data().writing_mode;
            wm.is_vertical().then(|| wm.is_vertical_lr())
        }
        #[cfg(not(feature = "writing-mode"))]
        None
    }

    /// The node's `display` as a [`taffy::Display`]. Returns [`taffy::Display::Block`]
    /// for nodes without computed styles (e.g. text nodes).
    pub fn taffy_display(&self) -> taffy::Display {
        self.primary_styles()
            .map(|style| {
                let display = style.clone_display();
                if self.element_data().is_some_and(|el| el.is_button_input())
                    && !matches!(
                        display.inside(),
                        DisplayInside::None | DisplayInside::Contents
                    )
                {
                    taffy::Display::FlowRoot
                } else {
                    stylo_taffy::convert::display(display)
                }
            })
            .unwrap_or(taffy::Display::Block)
    }

    /// The node's `position` as a [`taffy::Position`]. Returns [`taffy::Position::Static`]
    /// for nodes without computed styles (e.g. text nodes).
    pub fn taffy_position(&self) -> taffy::Position {
        self.primary_styles()
            .map(|s| stylo_taffy::convert::position(s.get_box().position))
            .unwrap_or(taffy::Position::Static)
    }

    /// Whether the node is an out-of-flow box that Taffy positions from its containing
    /// block's hoisted child list rather than from its parent (`display: none` boxes
    /// generate no box and are never hoisted).
    pub fn is_out_of_flow(&self) -> bool {
        self.primary_styles()
            .is_some_and(|s| style_is_out_of_flow(&s))
    }

    /// Record in [`Node::out_of_flow`] whether `style` (the node's new primary style)
    /// makes its box out of flow.
    pub(crate) fn record_out_of_flow(&self, style: &ComputedValues) {
        self.out_of_flow
            .store(style_is_out_of_flow(style), Ordering::Relaxed);
    }

    pub fn text_content(&self) -> String {
        let mut out = String::new();
        self.write_text_content(&mut out);
        out
    }

    fn write_text_content(&self, out: &mut String) {
        match &self.data {
            NodeData::Text(data) => {
                out.push_str(&data.content);
            }
            NodeData::Element(..) | NodeData::AnonymousBlock(..) => {
                for child_id in self.children.iter() {
                    crate::util::ensure_stack(|| self.with(*child_id).write_text_content(out));
                }
            }
            _ => {}
        }
    }

    pub fn flush_style_attribute(&mut self, url_extra_data: &UrlExtraData) {
        if let NodeData::Element(ref mut elem_data) = self.data {
            if let Some(guard) = elem_data.guard.clone() {
                elem_data.flush_style_attribute(&guard, url_extra_data);
            }
        }
    }

    /// The `order` sort key of this node within a flex/grid container's
    /// `layout_children`. The list is stable-sorted by this key when the
    /// container's children are constructed (`collect_layout_children`) and
    /// re-collected when a child's `order` changes (`REORDER_CHILDREN` damage).
    ///
    /// ::before/::after pseudos are flex/grid items and honor `order`. They
    /// sit first/last in the constructed list, and the sort is stable, so
    /// ties keep ::before first and ::after last. Out-of-flow children are
    /// not flex/grid items, so they key as 0 and keep source order.
    pub fn order(&self) -> i32 {
        self.primary_styles()
            .filter(|s| !matches!(s.get_box().position, Position::Absolute | Position::Fixed))
            .map(|s| s.clone_order())
            .unwrap_or(0)
    }

    pub fn z_index(&self) -> i32 {
        self.primary_styles()
            .map(|s| s.clone_z_index().integer_or(0))
            .unwrap_or(0)
    }

    // https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_positioned_layout/Stacking_context#features_creating_stacking_contexts
    pub fn is_stacking_context_root(&self, is_flex_or_grid_item: bool) -> bool {
        let Some(style) = self.primary_styles() else {
            return false;
        };

        let position = style.clone_position();
        let has_z_index = !style.clone_z_index().is_auto();

        if style.clone_opacity() != 1.0 {
            return true;
        }

        let position_based = match position {
            Position::Fixed | Position::Sticky => true,
            Position::Relative | Position::Absolute => has_z_index,
            Position::Static => has_z_index && is_flex_or_grid_item,
        };
        if position_based {
            return true;
        }

        let box_style = style.get_box();
        if !box_style.transform.0.is_empty()
            || !matches!(box_style.rotate, Rotate::None)
            || !matches!(box_style.scale, Scale::None)
            || !matches!(box_style.translate, Translate::None)
        {
            return true;
        }

        // pyro: `perspective` other than `none` and `transform-style: preserve-3d` make a
        // stacking context too (CSS Transforms 2), as they make a containing block.
        if !matches!(
            box_style.perspective,
            style::values::generics::box_::GenericPerspective::None
        ) || box_style.transform_style == style::computed_values::transform_style::T::Preserve3d
        {
            return true;
        }

        if self.applies_atomic_paint_effect() {
            return true;
        }

        // pyro: an animation, current or in effect, of a property that makes a stacking context
        // (the `will-change` side effect of Web Animations 1; WPT
        // `individual-transform/stacking-context-001`: in its delay).
        if self.element_data().is_some_and(|data| data.animation_stacking_context) {
            return true;
        }

        if style.get_box().isolation == Isolation::Isolate {
            return true;
        }

        if style.get_effects().mix_blend_mode != style::computed_values::mix_blend_mode::T::Normal {
            return true;
        }
        // pyro: `will-change` of a property that would make a stacking context makes one (its
        // positioned descendants paint inside it, its backdrop-root group among them), as in
        // Gecko: the unconditional ones, `opacity`, the transforms and `perspective`.
        {
            use style::values::specified::box_::WillChangeBits;
            if box_style.will_change.bits.intersects(
                WillChangeBits::STACKING_CONTEXT_UNCONDITIONAL
                    | WillChangeBits::OPACITY
                    | WillChangeBits::TRANSFORM
                    | WillChangeBits::PERSPECTIVE
                    // pyro: `contain` too (some of its values make one; WPT css-contain
                    // `contain-paint-stacking-context-001b`).
                    | WillChangeBits::CONTAIN,
            ) {
                return true;
            }
        }
        // pyro: layout and paint containment make a stacking context (CSS Contain 2 § 3.2, § 3.4),
        // on a box they apply to: not a non-atomic inline box, nor an internal table box other
        // than a cell (WPT css-contain `contain-paint-stacking-context-*`).
        {
            use style::values::specified::box_::{Contain, DisplayInside, DisplayOutside};
            if box_style.slow_clone_contain().intersects(Contain::LAYOUT | Contain::PAINT) {
                let display = box_style.display;
                let inline_box = display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow;
                let table_internal =
                    display.outside() == DisplayOutside::InternalTable && display.inside() != DisplayInside::TableCell;
                if !inline_box && !table_internal && !display.is_contents() && !display.is_none() {
                    return true;
                }
            }
        }

        false
    }

    /// Whether this node's styles apply an atomic paint effect (opacity, filter,
    /// clip-path, mask) to its subtree.
    pub(crate) fn applies_atomic_paint_effect(&self) -> bool {
        use style::values::computed::basic_shape::ClipPath;
        use style::values::generics::image::GenericImage;

        let Some(style) = self.primary_styles() else {
            return false;
        };

        if style.clone_opacity() != 1.0 {
            return true;
        }

        let effects = style.get_effects();
        if !effects.filter.0.is_empty() {
            return true;
        }

        if !matches!(style.get_clip_path(), ClipPath::None) {
            return true;
        }

        // pyro: a `mask-border` masks as a `mask-image` does.
        !matches!(style.get_svg().mask_border_source, GenericImage::None)
            || style
                .get_svg()
                .mask_image
                .0
                .iter()
                .any(|image| !matches!(image, GenericImage::None))
    }

    /// Takes an (x, y) position (relative to the *parent's* top-left corner) and returns:
    ///    - None if the position is outside of this node's bounds
    ///    - Some(HitResult) if the position is within the node but doesn't match any children
    ///    - The result of recursively calling child.hit() on the the child element that is
    ///      positioned at that position if there is one.
    ///
    /// Hit testing may run between a DOM mutation and the next `resolve`, so the
    /// paint lists and inline layouts it walks can still name nodes the mutation
    /// dropped; those are skipped.
    ///
    /// TODO: z-index
    /// (If multiple children are positioned at the position then a random one will be recursed into)
    pub fn hit(&self, x: f32, y: f32, scale: f64) -> Option<HitResult> {
        self.hit_inner(x, y, scale, &mut None)
    }

    /// [`hit`](Self::hit), also resolving the innermost overlay scrollbar
    /// thumb under the point into `scrollbar` during the same descent (so
    /// thumb hit-testing shares the exact coordinate handling — transforms
    /// included — of every other hit test).
    pub(crate) fn hit_inner(
        &self,
        x: f32,
        y: f32,
        scale: f64,
        scrollbar: &mut Option<crate::node::ScrollbarRef>,
    ) -> Option<HitResult> {
        use style::computed_values::pointer_events::T as PointerEvents;
        use style::computed_values::visibility::T as Visibility;

        // pyro: a positioned inline box listed in the paint tree for its paint order alone: its
        // inline root hit-tests its text (`NodeFlags::INLINE_PASS_ENTRY`).
        if self.flags.contains(NodeFlags::INLINE_PASS_ENTRY) {
            return None;
        }

        // Don't hit on visbility:hidden elements
        if let Some(style) = self.primary_styles() {
            if matches!(
                style.clone_visibility(),
                Visibility::Hidden | Visibility::Collapse
            ) {
                return None;
            }
        }

        // pointer-events:none makes this element transparent to hits, but its
        // descendants are still tested (one may restore pointer-events:auto).
        let pointer_events_none = self
            .primary_styles()
            .is_some_and(|style| style.clone_pointer_events() == PointerEvents::None);

        let mut x = x - self.final_layout().location.x + self.scroll_offset().x as f32;
        let mut y = y - self.final_layout().location.y + self.scroll_offset().y as f32;

        if let Some(t) = self.transform().as_deref() {
            let p = t.inverse() * kurbo::Point::new(x as f64 * scale, y as f64 * scale);
            x = (p.x / scale) as f32;
            y = (p.y / scale) as f32;
        }

        let size = self.final_layout().size;
        let matches_self = !(x < self.scroll_offset().x as f32
            || x > size.width + self.scroll_offset().x as f32
            || y < self.scroll_offset().y as f32
            || y > size.height + self.scroll_offset().y as f32);

        let layout = self.final_layout();
        let overflow_rect = layout.scrollable_overflow_rect;
        // Not `layout_style()`: hit testing also reaches elements that have no computed styles
        // yet (a pointer event before the first resolve). Those count as left-to-right.
        let is_rtl = self.primary_styles().is_some_and(|style| {
            style.clone_direction() == style::computed_values::direction::T::Rtl
        });
        let (gutter_left, gutter_top) = (is_rtl, false);
        #[cfg(feature = "writing-mode")]
        let (gutter_left, gutter_top) = {
            let wm = self.layout_data().writing_mode;
            if wm.swaps_axes() {
                (wm.block_left_is_end(), wm.direction().is_rtl())
            } else {
                (gutter_left, gutter_top)
            }
        };
        let client_x = layout.border.left
            + if gutter_left {
                layout.scrollbar_size.width
            } else {
                0.0
            };
        let client_y = layout.border.top
            + if gutter_top {
                layout.scrollbar_size.height
            } else {
                0.0
            };
        let matches_content = !(x < overflow_rect.left() + client_x
            || x > overflow_rect.right() + client_x
            || y < overflow_rect.top() + client_y
            || y > overflow_rect.bottom() + client_y);

        // pyro: a multi-column container's content lies in its strip (`layout::multicol`): its
        // boxes and lines are hit at the point the column under it shows.
        let (strip_x, strip_y) = self.columns_to_strip(x, y);
        let matches_hoisted_content = match &self.stacking_context {
            Some(sc) => {
                // Both the point (after the scroll adjustment above) and the
                // bbox are in this node's unscrolled content coordinates.
                let bbox = sc.hoisted_content_bbox(self.tree(), self.id, scale);
                let (x, y) = (strip_x, strip_y);
                x >= bbox.left && x <= bbox.right && y >= bbox.top && y <= bbox.bottom
            }
            None => false,
        };

        // `scrollable_overflow` is stored in device (scaled) pixels, whereas the
        // coordinates here are in CSS pixels, so unscale it before comparing.
        let overflow = *self.scrollable_overflow();

        let matches_overflow = x >= (overflow.x0 / scale) as f32
            && x <= (overflow.x1 / scale) as f32
            && y >= (overflow.y0 / scale) as f32
            && y <= (overflow.y1 / scale) as f32;

        if !matches_self && !matches_content && !matches_hoisted_content && !matches_overflow {
            return None;
        }

        // Descendants overwrite, so the innermost scroll container's thumb
        // wins. Thumb coords are border-box relative (unscrolled).
        if matches_self
            && let Some(sb) = self.scrollbar_at_local(
                (x - self.scroll_offset().x as f32) as f64,
                (y - self.scroll_offset().y as f32) as f64,
            )
        {
            *scrollbar = Some(sb);
        }
        let (x, y) = (strip_x, strip_y);

        // Text clusters and callers of the returned coordinates work in the
        // inline root's content box, but embedded boxes store border-box
        // relative locations like every other child; apply the offset only
        // where content-box coordinates are wanted, or atomic inlines in a
        // padded inline root are hit one padding down and right of where
        // they are painted.
        let content_box_offset = if self.flags.is_inline_root() {
            taffy::Point {
                x: self.final_layout().padding.left + self.final_layout().border.left,
                y: self.final_layout().padding.top + self.final_layout().border.top,
            }
        } else {
            taffy::Point { x: 0.0, y: 0.0 }
        };

        // Positive z_index hoisted children
        if matches_hoisted_content {
            if let Some(hoisted) = &self.stacking_context {
                for hoisted_child in hoisted.pos_z_hoisted_children().rev() {
                    let Some(child) = self.tree().get(hoisted_child.node_id) else {
                        continue;
                    };
                    let pos = hoisted_child.position(self.tree(), self.id);
                    let x = x - pos.x;
                    let y = y - pos.y;
                    if let Some(hit) = crate::util::ensure_stack(|| child.hit_inner(x, y, scale, scrollbar)) {
                        return Some(hit);
                    }
                }
            }
        }

        // Call `.hit()` on each child in turn. If any return `Some` then return that value. Else return `Some(self.id).
        for child_id in self.paint_children.borrow().iter().flatten().rev() {
            let Some(child) = self.tree().get(*child_id) else {
                continue;
            };
            let child_position = child.taffy_position();
            let mut child_x = x;
            let mut child_y = y;
            // pyro: an out-of-flow box painted here but laid out by a containing block elsewhere
            // (a fixed box at its stacking context's positioned level) is placed as painted.
            let lifted = child_position.is_out_of_flow().then(|| child.lifted_paint_offset(self.id)).flatten();
            if let Some(offset) = lifted {
                child_x -= offset.x;
                child_y -= offset.y;
            } else if child_position.is_out_of_flow() {
                // Fixed-position children of the root element are positioned
                // against the viewport and do not scroll with it. (A fixed box
                // whose containing block is a transformed ancestor scrolls with
                // that ancestor like any other out-of-flow box.)
                if child_position == taffy::Position::Fixed && self.is_root_element() {
                    child_x -= self.scroll_offset().x as f32;
                    child_y -= self.scroll_offset().y as f32;
                    let viewport_scroll = self.tree().viewport_scroll();
                    child_x -= viewport_scroll.x as f32;
                    child_y -= viewport_scroll.y as f32;
                }
            } else if let Some(offset) = child.lifted_paint_offset(self.id) {
                child_x -= offset.x;
                child_y -= offset.y;
            }
            if let Some(hit) = crate::util::ensure_stack(|| child.hit_inner(child_x, child_y, scale, scrollbar)) {
                return Some(hit);
            }
        }

        // Negative z_index hoisted children
        if matches_hoisted_content {
            if let Some(hoisted) = &self.stacking_context {
                for hoisted_child in hoisted.neg_z_hoisted_children().rev() {
                    let Some(child) = self.tree().get(hoisted_child.node_id) else {
                        continue;
                    };
                    let pos = hoisted_child.position(self.tree(), self.id);
                    let x = x - pos.x;
                    let y = y - pos.y;
                    if let Some(hit) = crate::util::ensure_stack(|| child.hit_inner(x, y, scale, scrollbar)) {
                        return Some(hit);
                    }
                }
            }
        }

        // Inline children
        if self.flags.is_inline_root() {
            let x = x - content_box_offset.x;
            let y = y - content_box_offset.y;
            let element_data = &self.element_data().unwrap();
            if let Some(ild) = element_data.inline_layout_data.as_ref() {
                let layout = &ild.layout;
                let scale = layout.scale();
                let y = y - ild.block_offset;

                if let Some((cluster, _side)) =
                    Cluster::from_point_exact(layout, x * scale, y * scale)
                {
                    let node_id = cluster.style().brush.id;
                    if let Some(text_node) = self.tree().get(node_id)
                        && !text_node.primary_styles().is_some_and(|style| {
                            style.clone_pointer_events() == PointerEvents::None
                        })
                    {
                        return Some(HitResult {
                            node_id,
                            x,
                            y,
                            is_text: true,
                        });
                    }
                }
            }
        }

        // Self (this node)
        if matches_self && !pointer_events_none {
            return Some(HitResult {
                node_id: self.id,
                x: x - content_box_offset.x,
                y: y - content_box_offset.y,
                is_text: false,
            });
        }

        None
    }

    /// Find the inline root ancestor of this node (or self if this is an inline root).
    /// Returns None if no inline root ancestor exists.
    pub fn inline_root_ancestor(&self) -> Option<&Node> {
        let mut node = self;
        loop {
            if node.flags.is_inline_root() {
                return Some(node);
            }
            let id = node.layout_parent.get()?;
            node = self.with(id);
        }
    }

    /// Get the text byte offset at a given point, using coordinates already transformed
    /// to be relative to this inline root's content box.
    /// Returns Some(byte_offset) if the point hits text, None otherwise.
    pub fn text_offset_at_point(&self, x: f32, y: f32) -> Option<usize> {
        if !self.flags.is_inline_root() {
            return None;
        }

        let element_data = self.element_data()?;
        let inline_layout = element_data.inline_layout_data.as_ref()?;
        let layout = &inline_layout.layout;
        let scale = layout.scale();
        let y = y - inline_layout.block_offset;

        // Use Parley's cluster hit testing (from_point is more forgiving than from_point_exact)
        let (cluster, side) = Cluster::from_point(layout, x * scale, y * scale)?;

        // Determine byte offset based on which side of the cluster was clicked
        // For LTR text: left side = start of cluster, right side = end of cluster
        // For RTL text: left side = end of cluster, right side = start of cluster
        // Also, explicit line breaks should always use start to avoid cursor appearing on next line
        let is_leading = side == ClusterSide::Left;
        let offset = if cluster.is_rtl() {
            if is_leading {
                cluster.text_range().end
            } else {
                cluster.text_range().start
            }
        } else {
            // LTR text
            if is_leading || cluster.is_line_break() == Some(BreakReason::Explicit) {
                cluster.text_range().start
            } else {
                cluster.text_range().end
            }
        };

        Some(offset)
    }

    /// Whether this node is a non-atomic inline element: one that has no layout box of its
    /// own because it is flattened into the containing inline root's text layout as a style
    /// span.
    pub fn is_non_atomic_inline(&self) -> bool {
        let Some(element) = self.element_data() else {
            return false;
        };
        if self.flags.is_inline_root()
            || crate::layout::replaced::is_inline_box(element)
        {
            return false;
        }
        self.primary_styles().is_some_and(|styles| {
            let display = styles.clone_display();
            display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow
        })
    }

    /// The node whose box this node's `Layout.location` is relative to: the
    /// `oof_containing_block` for a hoisted out-of-flow box, otherwise the
    /// `layout_parent`.
    pub fn containing_block(&self) -> Option<NodeId> {
        self.oof_containing_block
            .get()
            .or_else(|| self.layout_parent.get())
    }

    /// Computes the Document-relative coordinates of the `Node`
    pub fn absolute_position(&self, x: f32, y: f32) -> crate::util::Point<f32> {
        let x = x + self.final_layout().location.x - self.scroll_offset().x as f32;
        let y = y + self.final_layout().location.y - self.scroll_offset().y as f32;

        // Recurse up the positioning hierarchy
        // pyro: out of a multi-column container's strip into its columns (`layout::multicol`).
        self.containing_block()
            .map(|i| {
                let containing = self.with(i);
                let p = containing.strip_to_columns(crate::util::Point { x, y });
                containing.absolute_position(p.x, p.y)
            })
            .unwrap_or(crate::util::Point { x, y })
    }

    /// Whether this node can act as an [`offset_parent`](Self::offset_parent): a positioned
    /// element, or one of the elements that always qualify (`body`, `td`, `th`).
    pub(crate) fn is_offset_parent(&self) -> bool {
        let Some(styles) = self.primary_styles() else {
            return false;
        };
        if styles.get_box().position != Position::Static {
            return true;
        }
        self.data.is_element_with_tag_name(&local_name!("body"))
            || self.data.is_element_with_tag_name(&local_name!("td"))
            || self.data.is_element_with_tag_name(&local_name!("th"))
    }

    /// Whether this node is a non-positioned `body` element. When such an element is the
    /// `offsetParent`, `offsetLeft`/`offsetTop` are measured from the initial containing
    /// block origin rather than from the `body`'s padding edge.
    pub(crate) fn is_static_body(&self) -> bool {
        self.data.is_element_with_tag_name(&local_name!("body"))
            && self
                .primary_styles()
                .is_some_and(|styles| styles.get_box().position == Position::Static)
    }

    /// The nearest layout ancestor that [is an offset parent](Self::is_offset_parent), as in
    /// CSSOM View's `offsetParent`.
    pub fn offset_parent(&self) -> Option<&Node> {
        let mut node = self;
        loop {
            node = self.with(node.containing_block()?);
            if node.is_offset_parent() {
                return Some(node);
            }
        }
    }

    /// The per-line-box fragment boxes of a non-atomic inline element, in CSS pixels relative
    /// to the border box of its inline root.
    ///
    /// Returns `None` for nodes that have their own layout box.
    pub fn inline_fragment_boxes(&self) -> Option<impl Iterator<Item = taffy::Rect<f32>> + '_> {
        use parley::PositionedLayoutItem;

        if !self.is_non_atomic_inline() {
            return None;
        }

        let inline_root = self.inline_root_ancestor()?;
        let inline_layout = inline_root.element_data()?.inline_layout_data.as_ref()?;
        let layout = &inline_layout.layout;
        let scale = layout.scale();

        // Walk up the DOM parent chain from `id` to check whether it is (or is
        // inside) the target node, stopping at the inline root.
        let inline_root_id = inline_root.id;
        let is_in_target = move |mut id: NodeId| -> bool {
            loop {
                if id == self.id {
                    return true;
                }
                if id == inline_root_id {
                    return false;
                }
                match self.with(id).parent {
                    Some(parent) => id = parent,
                    None => return false,
                }
            }
        };

        let root_layout = inline_root.unrounded_layout();
        let content_box_inset = root_layout.padding + root_layout.border;
        let vertical = inline_root.vertical_block_axis();
        // pyro: `sideways-lr` lines run bottom to top from the content box's bottom (its
        // line-left side), their line-over side on the left: the content height they run up from.
        let line_left_is_bottom = vertical.is_some() && inline_root.layout_data().writing_mode.line_left_is_bottom();
        let inline_extent = line_left_is_bottom.then(|| {
            let layout = inline_root.final_layout();
            layout.size.height - layout.padding.top - layout.padding.bottom - layout.border.top - layout.border.bottom
        });
        let (origin_x, origin_y) = match vertical {
            None => (content_box_inset.left, content_box_inset.top + inline_layout.block_offset),
            Some(block_lr) => {
                // Physical: `unrounded_layout` is in the writing mode of the box that placed the root.
                let root_layout = inline_root.final_layout();
                let inset = root_layout.padding + root_layout.border;
                let block_start = if block_lr {
                    inset.left + inline_layout.block_offset
                } else {
                    root_layout.size.width - inset.right - inline_layout.block_offset
                };
                (block_start, inset.top)
            }
        };

        fn union(acc: &mut Option<taffy::Rect<f32>>, left: f32, top: f32, right: f32, bottom: f32) {
            *acc = Some(match *acc {
                Some(rect) => taffy::Rect {
                    left: rect.left.min(left),
                    top: rect.top.min(top),
                    right: rect.right.max(right),
                    bottom: rect.bottom.max(bottom),
                },
                None => taffy::Rect {
                    left,
                    top,
                    right,
                    bottom,
                },
            });
        }

        // One rect per line box: the union of all of the target's fragments on that line
        Some(layout.lines().filter_map(move |line| {
            let line_metrics = line.metrics();
            let mut line_rect: Option<taffy::Rect<f32>> = None;

            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => {
                        if !is_in_target(glyph_run.style().brush.id) {
                            continue;
                        }
                        let x0 = glyph_run.offset();
                        let x1 = x0 + glyph_run.advance();
                        // Use the line box's block extent rather than the
                        // run's font ascent/descent: fonts with small
                        // typographic metrics would otherwise produce rects
                        // that clip the rendered glyphs. This matches the
                        // geometry used for text selection highlights.
                        let y0 = line_metrics.block_min_coord;
                        let y1 = line_metrics.block_max_coord;
                        union(&mut line_rect, x0, y0, x1, y1);
                    }
                    PositionedLayoutItem::InlineBox(inline_box) => {
                        if !is_in_target(NodeId::from_u64(inline_box.id)) {
                            continue;
                        }
                        let x0 = inline_box.x;
                        let y0 = inline_box.y;
                        union(
                            &mut line_rect,
                            x0,
                            y0,
                            x0 + inline_box.width,
                            y0 + inline_box.height,
                        );
                    }
                }
            }

            line_rect.map(|rect| {
                // The lines of a vertical writing mode are logical: x inline, y block.
                if let Some(block_lr) = vertical {
                    let (min, max) = (line_metrics.block_min_coord, line_metrics.block_max_coord);
                    if let Some(extent) = inline_extent {
                        return taffy::Rect {
                            left: origin_x + rect.top / scale,
                            top: origin_y + extent - rect.right / scale,
                            right: origin_x + rect.bottom / scale,
                            bottom: origin_y + extent - rect.left / scale,
                        };
                    }
                    let (b0, b1) = if block_lr {
                        (min + max - rect.bottom, min + max - rect.top)
                    } else {
                        (-rect.bottom, -rect.top)
                    };
                    return taffy::Rect {
                        left: origin_x + b0 / scale,
                        top: origin_y + rect.left / scale,
                        right: origin_x + b1 / scale,
                        bottom: origin_y + rect.right / scale,
                    };
                }
                taffy::Rect {
                    left: origin_x + rect.left / scale,
                    top: origin_y + rect.top / scale,
                    right: origin_x + rect.right / scale,
                    bottom: origin_y + rect.bottom / scale,
                }
            })
        }))
    }

    /// CSSOM View's `offsetLeft`/`offsetTop`: the offset of this node's border box from the
    /// padding edge of its [`offset_parent`](Self::offset_parent).
    pub fn offset_top_left(&self) -> crate::util::Point<f32> {
        let mut x = 0.0;
        let mut y = 0.0;
        let mut current = self;

        // Non-atomic inlines have no layout box of their own: start from the first
        // fragment box (relative to the inline root's border box) and continue the
        // walk from the inline root.
        if let Some(mut fragments) = self.inline_fragment_boxes() {
            if let Some(first) = fragments.next() {
                x += first.left;
                y += first.top;
            }
            let Some(inline_root) = self.inline_root_ancestor() else {
                return crate::util::Point { x, y };
            };
            if inline_root.is_offset_parent() && !inline_root.is_static_body() {
                let border = inline_root.final_layout().border;
                return crate::util::Point {
                    x: x - border.left,
                    y: y - border.top,
                };
            }
            current = inline_root;
        }

        loop {
            let layout = current.final_layout();
            x += layout.location.x;
            y += layout.location.y;

            let Some(parent_id) = current.containing_block() else {
                break;
            };
            let parent = self.with(parent_id);
            if parent.is_offset_parent() && !parent.is_static_body() {
                let border = parent.final_layout().border;
                x -= border.left;
                y -= border.top;
                break;
            }
            current = parent;
        }
        crate::util::Point { x, y }
    }

    /// CSSOM View's `clientWidth`: the width of the padding box (border box minus
    /// borders and scrollbar)
    pub fn client_width(&self) -> f32 {
        let layout = self.final_layout();
        layout.size.width - layout.border.left - layout.border.right - layout.scrollbar_size.width
    }

    /// CSSOM View's `clientHeight`: the height of the padding box (border box minus
    /// borders and scrollbar)
    pub fn client_height(&self) -> f32 {
        let layout = self.final_layout();
        layout.size.height - layout.border.top - layout.border.bottom - layout.scrollbar_size.height
    }

    /// CSSOM View's `scrollWidth`: the width of the node's content, including
    /// content not visible due to overflow
    pub fn scroll_width(&self) -> f32 {
        self.cssom_scroll_size().width
    }

    /// CSSOM View's `scrollHeight`: the height of the node's content, including
    /// content not visible due to overflow
    pub fn scroll_height(&self) -> f32 {
        self.cssom_scroll_size().height
    }

    fn cssom_scroll_size(&self) -> taffy::Size<f32> {
        let layout = self.final_layout();
        if self.primary_styles().is_none() {
            return taffy::Size {
                width: self.client_width() + layout.scroll_width(),
                height: self.client_height() + layout.scroll_height(),
            };
        }
        let overflow = taffy::CoreStyle::overflow(&self.layout_style());
        if overflow.x.is_scroll_container() || overflow.y.is_scroll_container() {
            return taffy::Size {
                width: self.client_width() + layout.scroll_width(),
                height: self.client_height() + layout.scroll_height(),
            };
        }

        // Non-scroll containers retain both sides for propagation and hit testing, but do not
        // carry scroll-origin flags. CSSOM still measures their scrolling area from its origin.
        let mut inline = false;
        let mut block = false;
        let mut vertical = false;
        if let Some(style) = self.primary_styles() {
            let wm = style.writing_mode;
            inline = wm.direction().is_rtl();
            block = wm.block_left_is_end();
            vertical = wm.swaps_axes();
            if self.taffy_display() == taffy::Display::Flex {
                let position = style.get_position();
                let row = matches!(
                    position.flex_direction,
                    FlexDirection::Row | FlexDirection::RowReverse
                );
                let reverse = matches!(
                    position.flex_direction,
                    FlexDirection::RowReverse | FlexDirection::ColumnReverse
                );
                let wrap_reverse = position
                    .flex_wrap
                    .contains(style::values::computed::FlexWrap::WRAP_REVERSE);
                inline ^= if row { reverse } else { wrap_reverse };
                block ^= if row { wrap_reverse } else { reverse };
            }
        }
        let (reverse_x, reverse_y) = if vertical {
            (block, inline)
        } else {
            (inline, block)
        };
        let overflow = layout.scrollable_overflow_rect;
        taffy::Size {
            width: if reverse_x {
                self.client_width() - overflow.left()
            } else {
                self.client_width().max(overflow.right())
            },
            height: if reverse_y {
                self.client_height() - overflow.top()
            } else {
                self.client_height().max(overflow.bottom())
            },
        }
    }

    /// Signed CSSOM scroll offsets, with zero at the initial scroll position.
    pub(crate) fn scroll_range(&self) -> (crate::Point<f64>, crate::Point<f64>) {
        let layout = self.final_layout();
        let reversed = layout.scrollable_overflow_rect.is_reversed();
        let width = layout.scroll_width() as f64;
        let height = layout.scroll_height() as f64;
        (
            crate::Point {
                x: if reversed.x { -width } else { 0.0 },
                y: if reversed.y { -height } else { 0.0 },
            },
            crate::Point {
                x: if reversed.x { 0.0 } else { width },
                y: if reversed.y { 0.0 } else { height },
            },
        )
    }

    /// Does the node generate any boxes? (e.g. `getClientRects()` returns an empty
    /// list for boxless nodes, such as `display: none`, `display: contents`, or
    /// detached elements)
    pub fn has_boxes(&self) -> bool {
        self.flags.is_in_document()
            && !self.display_style().is_some_and(|display| {
                matches!(
                    display.inside(),
                    style::values::specified::box_::DisplayInside::None
                        | style::values::specified::box_::DisplayInside::Contents
                )
            })
    }

    /// Creates a synthetic click event
    pub fn synthetic_click_event(&self, mods: Modifiers) -> DomEventData {
        DomEventData::Click(self.synthetic_click_event_data(mods))
    }

    pub fn synthetic_click_event_data(&self, mods: Modifiers) -> BlitzPointerEvent {
        let absolute_position = self.absolute_position(0.0, 0.0);
        let x = absolute_position.x + (self.final_layout().size.width / 2.0);
        let y = absolute_position.y + (self.final_layout().size.height / 2.0);

        BlitzPointerEvent {
            id: BlitzPointerId::Mouse,
            is_primary: true,
            coords: PointerCoords {
                page_x: x,
                page_y: y,

                // TODO: should these be different?
                screen_x: x,
                screen_y: y,
                client_x: x,
                client_y: y,
            },
            mods,
            button: Default::default(),
            buttons: Default::default(),
            details: Default::default(),
            element: Default::default(),
            active_pointers: Default::default(),
        }
    }
}

/// It might be wrong to expose this since what does *equality* mean outside the dom?
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Node {}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // FIXME: update to reflect changes to fields
        f.debug_struct("NodeData")
            .field("parent", &self.parent)
            .field("id", &self.id)
            .field("is_inline_root", &self.flags.is_inline_root())
            .field("children", &self.children)
            .field("layout_children", &self.layout_children.borrow())
            // .field("style", &self.style)
            .field("node", &self.data)
            .field("stylo_element_data", &self.try_stylo_element_data())
            // .field("unrounded_layout", &self.unrounded_layout)
            // .field("final_layout", &self.final_layout)
            .finish()
    }
}

/// Whether a box with `style` is out of flow: Taffy positions it from its containing
/// block's hoisted child list rather than from its parent (`display: none` boxes generate
/// no box and are never hoisted).
pub(crate) fn style_is_out_of_flow(style: &ComputedValues) -> bool {
    let box_style = style.get_box();
    matches!(box_style.position, Position::Absolute | Position::Fixed)
        && !box_style.display.is_none()
}

#[cfg(test)]
mod test {
    use style_dom::ElementState;

    use crate::{Attribute, BaseDocument, DocumentConfig, ElementData, NodeData, qual_name};

    #[test]
    fn create_node_with_disabled_attr() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let node = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("button"),
            vec![Attribute {
                name: qual_name!("disabled"),
                value: "".into(),
            }],
        ))));
        let node = document.get_node(node).unwrap();

        assert!(
            node.element_state().contains(ElementState::DISABLED),
            "form node is disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is not enabled"
        );
    }

    #[test]
    fn ignore_disabled_attr_content() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let node = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("button"),
            vec![Attribute {
                name: qual_name!("disabled"),
                value: "false".into(),
            }],
        ))));
        let node = document.get_node(node).unwrap();

        assert!(
            node.element_state().contains(ElementState::DISABLED),
            "form node is disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is not enabled"
        );
    }

    #[test]
    fn create_node_with_ignored_disable() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let node = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("a"),
            vec![Attribute {
                name: qual_name!("disabled"),
                value: "".into(),
            }],
        ))));
        let node = document.get_node(node).unwrap();

        assert!(
            !node.element_state().contains(ElementState::DISABLED),
            "Non form node cannot be disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "Non form node cannot be enabled"
        );
    }

    #[test]
    fn create_empty_enabled_node() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let node = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("button"),
            vec![],
        ))));
        let node = document.get_node(node).unwrap();

        assert!(
            node.element_state().contains(ElementState::ENABLED),
            "Button should be enabled by default"
        );
    }
}
