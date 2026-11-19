mod background;
mod border;
mod border_image;
mod border_shape;
mod box_shadow;
mod clip_path;
mod form_controls;
mod mask;
mod multicol;
mod three_d;
pub(crate) use three_d::is_projective_marker;
#[cfg(feature = "svg")]
pub(crate) use clip_path::svg_reference::external_reference;

use blitz_dom::CopyStyleGetters;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::kurbo_css::CssBox;
use crate::blend::convert_blend_mode;
use crate::color::{Color, ToColorColor};
use crate::debug_overlay::render_debug_overlay;
use crate::filters::convert_filters;
use crate::kurbo_css::NonUniformRoundedRectRadii;
use crate::layers::LayerManager;
use crate::sizing::compute_object_fit;
use crate::text::{DrawTextContext, TextPaintOptions};
use crate::CustomWidgetSceneMap;
use anyrender::{PaintScene, Scene};
use blitz_dom::node::{
    ListItemLayout, ListItemLayoutPosition, Marker, NodeData, RasterImageData, TextInputData,
    TextNodeData,
};
use blitz_dom::{BaseDocument, ElementData, Node, NodeId, local_name};
use blitz_traits::devtools::DevtoolSettings;

use style::logical_geometry::WritingMode;
use style::values::computed::{BorderCornerRadius, ColorOrAuto};
use style::{
    dom::TElement,
    properties::{
        ComputedValues, generated::longhands::visibility::computed_value::T as StyloVisibility,
        style_structs::Font,
    },
    values::{
        computed::{CSSPixelLength, Contain, Overflow},
        specified::box_::{DisplayInside, DisplayOutside},
        specified::image::ImageRendering,
    },
};

use kurbo::{self, Affine, Insets, Point, Rect, Shape, Size, Stroke, Vec2};
use peniko::{self, BlendMode, Fill, ImageData, ImageSampler};
use style::computed_values::isolation::T as Isolation;
use taffy::Layout;

/// An opacity just below 1.0, used to force the renderer to allocate an isolated
/// buffer for mask and text-clip isolation layers. Renderers (e.g. vello_cpu) may optimize
/// layers with `Mix::Normal`/`Compose::SrcOver` and an opacity of exactly 1.0 into
/// plain non-isolated clip layers, in which case the `Compose::DestIn` mask
/// compositing would erase the backdrop behind the element rather than just the
/// element's own content.
pub(super) const ALMOST_OPAQUE: f32 = 1.0 - f32::EPSILON;

/// A view of the track positions reported by taffy in physical (left-to-right /
/// top-to-bottom) order. Taffy reports tracks in logical order, which for RTL grid
/// containers is physically reversed.
#[derive(Clone, Copy)]
pub(crate) struct PhysicalTracks<'a> {
    positions: &'a [taffy::Line<f32>],
    reversed: bool,
}

impl<'a> PhysicalTracks<'a> {
    pub(crate) fn from_tracks<S: taffy::CheapCloneStr>(
        tracks: &'a taffy::DetailedGridTracksInfo<S>,
    ) -> Self {
        let positions = tracks.positions.as_slice();
        let reversed = positions
            .first()
            .zip(positions.last())
            .is_some_and(|(first, last)| first.start > last.start);
        Self {
            positions,
            reversed,
        }
    }

    fn get(self, index: usize) -> taffy::Line<f32> {
        if self.reversed {
            self.positions[self.positions.len() - 1 - index]
        } else {
            self.positions[index]
        }
    }

    /// Iterate track positions in physical order
    pub(crate) fn iter(self) -> impl ExactSizeIterator<Item = taffy::Line<f32>> + 'a {
        (0..self.positions.len()).map(move |index| self.get(index))
    }

    /// The physical start position of the first track
    pub(crate) fn origin(self) -> f32 {
        if self.positions.is_empty() {
            0.0
        } else {
            self.get(0).start
        }
    }

    /// Total distance from the start of the first track to the end of the last track
    pub(crate) fn span(self) -> f32 {
        if self.positions.is_empty() {
            0.0
        } else {
            self.get(self.positions.len() - 1).end - self.get(0).start
        }
    }
}

/// pyro: which part of a box of the normal flow is painted (CSS 2.1 Appendix E). A box that
/// paints floats (`NodeFlags::HAS_FLOAT_PAINT_CHILDREN`: its own, or lifted out of the boxes that
/// paint in its phases, `Node::paints_in_ancestor_phases`) paints those boxes twice: their
/// backgrounds and borders first (step 4), then the floats (step 5), then their inline content
/// (step 7). Every other box paints `All` of itself, at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlowPhase {
    /// The whole box and its descendants.
    All,
    /// Its background, borders and shadows, then its block-level descendants' (atomic ones whole).
    Background,
    /// Its inline content (lines, atomic inlines, list marker), then its block-level descendants'.
    Foreground,
    /// Its outline, then its block-level descendants': Chromium paints the outlines of a normal
    /// flow after its foreground, under the positioned boxes (WPT `z-index-020`).
    Outline,
}

/// A short-lived struct which holds a bunch of parameters for rendering a scene so
/// that we don't have to pass them down as parameters
pub struct BlitzDomPainter<'dom, 'a> {
    /// Input parameters (read only) for generating the Scene
    pub(crate) dom: &'dom BaseDocument,
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) initial_x: f64,
    pub(crate) initial_y: f64,
    /// The id of the document's root element (cached to avoid re-resolving it for every element)
    pub(crate) root_element_id: Option<NodeId>,
    /// pyro: the element whose background is the canvas background: the root element, or the
    /// body whose background propagates to it; `None` when the root element is not rendered.
    pub(crate) canvas_background: Option<NodeId>,
    /// pyro: the document's top layer, bottom first (usually
    /// empty). Its elements are skipped where the tree puts them and painted
    /// above everything, each over its `::backdrop`.
    pub(crate) top_layer: Vec<NodeId>,
    /// The top-layer element being painted as such (not skipped).
    pub(crate) top_layer_root: std::cell::Cell<Option<NodeId>>,
    /// Scrollbar hover/drag state, resolved once per scene like the root element
    #[cfg(feature = "scrollbars")]
    pub(crate) hovered_scrollbar: Option<blitz_dom::node::ScrollbarRef>,
    #[cfg(feature = "scrollbars")]
    pub(crate) scrollbar_drag_target: Option<blitz_dom::node::ScrollbarRef>,
    pub(crate) layer_manager: LayerManager,
    /// pyro: the stacking contexts painted as isolated groups for a `mix-blend-mode` in them.
    pub(crate) blend_isolated: HashSet<NodeId>,
    /// pyro: the ancestors of elements with a `backdrop-filter` (`layers::DocumentEffects`).
    pub(crate) backdrop_ancestors: HashSet<NodeId>,
    /// Reusable scratch allocations shared by all text layouts painted for this document.
    pub(crate) draw_text_context: RefCell<DrawTextContext>,
    /// Cached selection ranges for O(1) lookup: node_id -> (start_offset, end_offset)
    pub(crate) selection_ranges: HashMap<NodeId, (usize, usize)>,
    /// pyro: the highlights painted (custom highlights and the target text, then the selection),
    /// bottom first, and each inline root's highlighted bytes: (index in `highlight_layers`,
    /// start, end).
    pub(crate) highlight_layers: Vec<crate::highlight::Layer>,
    pub(crate) highlight_ranges: HashMap<NodeId, Vec<(usize, usize, usize)>>,
    /// pyro: the caret of the focused editing host, when it is not painted as a highlight (a
    /// block caret over the character after it is: `Layer::Caret`).
    pub(crate) caret: Option<crate::caret::CaretPaint>,
    /// pyro: what `paint_inline_box_fragment` paints of an inline box's outline: 0 all of the
    /// fragment with its outline, 1 the fragment without it, 2 only the outline (an inline root
    /// draws its boxes' outlines after its text, CSS 2.1 Appendix E step 10).
    pub(crate) inline_outlines: std::cell::Cell<u8>,
    /// pyro: the content of positioned inline boxes listed in the paint tree
    /// (`NodeFlags::INLINE_PASS_ENTRY`), left by their inline root for their own entry: the span's id ->
    /// (its inline root, the root's transform, the root's content position), painted in the positioned
    /// layer in tree order (CSS 2.1 Appendix E step 8; `draw_inline_pass_entry`). In a multi-column
    /// inline root, one per column that shows a line of the box, with the column's strip range
    /// (CSS px) it is clipped to (`PaintContext::inline_column_range`).
    pub(crate) inline_pass_entries: RefCell<HashMap<NodeId, Vec<(NodeId, Affine, Point, Option<(f32, f32)>)>>>,
    /// pyro: while a multi-column container paints its children (`ElementCx::draw_columns`): the
    /// container, and whether only its unfragmented children are painted (`true`: the pass after
    /// the columns) or all but them (`false`: each column). Its unfragmented children are the
    /// out-of-flow boxes it is the containing block of, placed against it, not in a column.
    pub(crate) multicol_pass: std::cell::Cell<Option<(NodeId, bool)>>,
    /// pyro: the clip (screen space) of the element being painted (`render_element`): dotted and
    /// dashed borders make their marks in it only.
    pub(crate) paint_clip: std::cell::Cell<Rect>,
    /// pyro: while a multi-column container paints one of its columns (not a spanner's row): the
    /// container, whose spanners that pass leaves out (they are painted in their own rows).
    pub(crate) column_pass_container: std::cell::Cell<Option<NodeId>>,
    /// pyro: while a multi-column container paints a column without a block-axis clip: the
    /// container, the column's strip range (CSS px), its transform and its clip rectangle (the
    /// container's space, device px) for the boxes a break goes through (`draw_paint_child`).
    pub(crate) column_range: std::cell::Cell<Option<(NodeId, f32, f32, Affine, Rect)>>,
    /// pyro: while a multi-column container paints a spanner's row: the container, whose boxes
    /// that carry the spanner do not paint their own box there (the row is no part of them).
    pub(crate) spanner_row_container: std::cell::Cell<Option<NodeId>>,
    /// pyro: the spanner that row shows: the row paints it alone (and the boxes it is in, which
    /// paint no box of their own there), what overflows it below too.
    pub(crate) spanner_row_spanner: std::cell::Cell<Option<NodeId>>,
    /// pyro: the spanner a last spanner row's second pass leaves out (that row shows what comes
    /// after it too, clipped at its start).
    pub(crate) spanner_row_except: std::cell::Cell<Option<NodeId>>,
    /// pyro: a box hoisted out of a multi-column container (a stacking context painted by an
    /// ancestor) is being painted in one of the container's columns (`draw_paint_child`).
    pub(crate) hoisted_in_column: std::cell::Cell<bool>,
    /// pyro: while a multi-column inline root paints its lines in a column: the column's strip
    /// range (CSS px), for the positioned boxes left to their paint-tree entries.
    pub(crate) inline_column_range: std::cell::Cell<Option<(f32, f32)>>,
    /// Paint background colors and images (false: printing without them).
    pub backgrounds: bool,
    /// Print: text at its layout-unit x, not its box's whole-pixel x (Chromium keeps the fraction in the
    /// inline axis). Screenshots keep the box's x.
    pub fractional_text_x: bool,
    /// pyro: a flat element with a 3D transform and element children is drawn in a layer the
    /// backend rasterizes in the element's own plane (`layers::local_raster_marker`), as Chromium
    /// composites it. Screenshots only: printed output stays vector.
    pub composite_3d: bool,
    /// pyro: how many local-raster layers (`composite_3d`) the element painted is in.
    pub(crate) local_raster_depth: std::cell::Cell<u32>,
    /// pyro: the paint phase the element painted is painted in (`FlowPhase`).
    pub(crate) flow_phase: std::cell::Cell<FlowPhase>,
    /// pyro: a box painted in the foreground phase had an outline, left for the outline phase.
    pub(crate) deferred_outlines: std::cell::Cell<bool>,
    /// pyro: the clipping stacking-context root whose content is being painted in its clip: the
    /// out-of-flow boxes of its context whose containing block lies outside it are left for
    /// after the clip (`ElementCx::escapes_own_clip`).
    pub(crate) clip_escapes_of: std::cell::Cell<Option<NodeId>>,
    /// pyro: a clipping box that is no stacking context painting its children each in its clip
    /// (`ElementCx::clip_transfer_child`), and the child that takes the clip over instead: (the
    /// box, the child, the clip in device space). The clip is the box's; boxes it does not contain
    /// escape it inside the child's effect group (Chromium clips by the containing-block chain).
    /// (Also the box's own culling rect, uncut by its clip, which the child's effect group takes.)
    pub(crate) handed_clips: RefCell<Option<(NodeId, NodeId, kurbo::BezPath, Rect)>>,
    /// pyro: the clip handed to the element about to paint: (its owner, the clip in device space).
    pub(crate) handed_clip: RefCell<Option<(NodeId, kurbo::BezPath)>>,
    /// pyro: the element painting under a handed clip and the clip's owner, against whose
    /// containing-block chain its boxes escape (`ElementCx::escapes_own_clip`).
    pub(crate) escape_owner: std::cell::Cell<Option<(NodeId, NodeId)>>,
    /// pyro: the element whose negative `z-index` children are painted before its clip, each in
    /// it unless it escapes it (`ElementCx::escapes_own_clip`).
    pub(crate) neg_z_outside_clip_of: std::cell::Cell<Option<NodeId>>,
    /// pyro: the element whose positioned-level children (and those with a positive `z-index`)
    /// are being left for after its outline (`ElementCx::defers_positioned`).
    pub(crate) positioned_after_outline_of: std::cell::Cell<Option<NodeId>>,
    /// pyro (lane H2): 3D rendering contexts being recorded (`render/three_d.rs`).
    pub(crate) three_d: three_d::State,
    /// pyro: where the scene's coordinates land in device pixels, when that is known (a
    /// screenshot's page scene: the capture's transform of it). A clip around a box hoisted to its
    /// stacking context is then left out where it would cut nothing (`draw_hoisted_children`).
    /// `None` (printed output, sub-documents): every such clip is pushed.
    pub device_pixels: Option<Affine>,

    // Pre-computed `Scene`s for each CustomWidget
    pub(crate) custom_widget_scenes: &'a CustomWidgetSceneMap,
}

impl<'dom, 'a> BlitzDomPainter<'dom, 'a> {
    /// Create a new BlitzDomPainter for the given document
    pub fn new(
        dom: &'dom BaseDocument,
        scale: f64,
        width: u32,
        height: u32,
        initial_x: f64,
        initial_y: f64,
        custom_widget_scenes: &'a CustomWidgetSceneMap,
    ) -> Self {
        let selection_ranges: HashMap<NodeId, (usize, usize)> = dom
            .get_text_selection_ranges()
            .into_iter()
            .map(|(node_id, start, end)| (node_id, (start, end)))
            .collect();

        // pyro: at most 31 highlights below the selection (bits of a mask).
        let mut highlight_layers = Vec::new();
        let mut highlight_ranges: HashMap<NodeId, Vec<(usize, usize, usize)>> = HashMap::new();
        for (name, ranges) in dom.highlight_layer_ranges().into_iter().take(31) {
            let layer = highlight_layers.len();
            highlight_layers.push(crate::highlight::Layer::Named(name));
            for (node_id, start, end) in ranges {
                highlight_ranges.entry(node_id).or_default().push((layer, start, end));
            }
        }
        let selection_layer = highlight_layers.len();
        highlight_layers.push(crate::highlight::Layer::Selection);
        for (&node_id, &(start, end)) in &selection_ranges {
            highlight_ranges.entry(node_id).or_default().push((selection_layer, start, end));
        }
        // pyro: the caret, a block one over the character after it as the top highlight.
        let mut caret = crate::caret::caret_paint(dom);
        if let Some(paint) = &caret
            && let Some(end) = paint.covers
        {
            let layer = highlight_layers.len();
            highlight_layers.push(crate::highlight::Layer::Caret(paint.color, paint.text_color));
            highlight_ranges.entry(paint.node).or_default().push((layer, paint.byte, end));
            caret = None;
        }

        let layer_manager = LayerManager::default();
        let effects = crate::layers::document_effects(dom);
        let root_element_id = dom.try_root_element().map(|el| el.id);
        let canvas_background = canvas_background_source(dom);

        Self {
            dom,
            scale,
            width,
            height,
            initial_x,
            initial_y,
            root_element_id,
            canvas_background,
            top_layer: if dom.has_top_layer() {
                dom.top_layer_nodes().collect()
            } else {
                Vec::new()
            },
            top_layer_root: std::cell::Cell::new(None),
            #[cfg(feature = "scrollbars")]
            hovered_scrollbar: dom.hovered_scrollbar(),
            #[cfg(feature = "scrollbars")]
            scrollbar_drag_target: dom.scrollbar_drag_target(),
            layer_manager,
            blend_isolated: effects.blend_isolated,
            backdrop_ancestors: effects.backdrop_ancestors,
            draw_text_context: RefCell::new(DrawTextContext::default()),
            selection_ranges,
            highlight_layers,
            highlight_ranges,
            caret,
            inline_outlines: std::cell::Cell::new(0),
            inline_pass_entries: RefCell::new(HashMap::new()),
            multicol_pass: std::cell::Cell::new(None),
            column_pass_container: std::cell::Cell::new(None),
            column_range: std::cell::Cell::new(None),
            spanner_row_container: std::cell::Cell::new(None),
            spanner_row_spanner: std::cell::Cell::new(None),
            spanner_row_except: std::cell::Cell::new(None),
            hoisted_in_column: std::cell::Cell::new(false),
            inline_column_range: std::cell::Cell::new(None),
            paint_clip: std::cell::Cell::new(Rect::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY)),
            backgrounds: true,
            fractional_text_x: false,
            composite_3d: false,
            local_raster_depth: std::cell::Cell::new(0),
            flow_phase: std::cell::Cell::new(FlowPhase::All),
            deferred_outlines: std::cell::Cell::new(false),
            clip_escapes_of: std::cell::Cell::new(None),
            handed_clips: RefCell::new(None),
            handed_clip: RefCell::new(None),
            escape_owner: std::cell::Cell::new(None),
            neg_z_outside_clip_of: std::cell::Cell::new(None),
            positioned_after_outline_of: std::cell::Cell::new(None),
            three_d: Default::default(),
            device_pixels: None,
            custom_widget_scenes,
        }
    }

    /// Draw the current tree to current render surface
    /// Eventually we'll want the surface itself to be passed into the render function, along with things like the viewport
    ///
    /// This assumes styles are resolved and layout is complete.
    /// Make sure you do those before trying to render
    pub fn paint_scene(&self, scene: &mut impl PaintScene) {
        if self.dom.has_pending_critical_resources() {
            return;
        }

        // Simply render the document (the root element (note that this is not the same as the root node)))
        // scene.reset();
        let viewport_scroll = self.dom.as_ref().viewport_scroll();

        // A document without a root element (e.g. an empty iframe sub-document) has
        // nothing to paint.
        let Some(root_element) = self.dom.as_ref().try_root_element() else {
            return;
        };
        let root_id = root_element.id;

        // pyro: the root element paints the canvas background (`draw_canvas_background`), in
        // its effect group. A root that paints nothing of its own (`visibility: hidden`) leaves
        // it to be painted here, its colour alone, as before.
        if self.backgrounds
            && let Some(source) = self.canvas_background
            && root_element
                .primary_styles()
                .is_some_and(|style| style.get_inherited_box().visibility != StyloVisibility::Visible)
            && let Some(style) = self.dom.get_node(source).and_then(|node| node.primary_styles())
        {
            let bg_color = style
                .get_background_color()
                .resolve_to_absolute(&style.clone_color())
                .as_srgb_color();
            scene.fill(Fill::NonZero, Affine::IDENTITY, bg_color, None, &self.canvas_rect());
        }

        // The root clip rectangle is the viewport (in screen coordinates, with the
        // initial offset already subtracted). Elements outside of this are culled, and
        // scrollports narrow this rectangle further for their descendants.
        let viewport_clip_rect = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);

        // pyro: classic scrollbars: the document is cut to the viewport's scrollport, the
        // viewport less its scrollbar gutters; the canvas background goes under the gutters.
        let viewport_bars = self.dom.as_ref().viewport_scrollbars;
        let scrollport = viewport_bars.any().then(|| {
            let port = viewport_bars.scrollport(self.width as f32 / self.scale as f32, self.height as f32 / self.scale as f32);
            port.scale_from_origin(self.scale) + Vec2::new(self.initial_x, self.initial_y)
        });
        if let Some(port) = scrollport {
            if self.backgrounds
                && let Some(source) = self.canvas_background
                && let Some(style) = self.dom.get_node(source).and_then(|node| node.primary_styles())
            {
                let bg_color =
                    style.get_background_color().resolve_to_absolute(&style.clone_color()).as_srgb_color();
                let canvas = Rect::from_origin_size((self.initial_x, self.initial_y), (self.width as f64, self.height as f64));
                for strip in [
                    Rect::new(canvas.x0, canvas.y0, port.x0, canvas.y1),
                    Rect::new(port.x1, canvas.y0, canvas.x1, canvas.y1),
                    Rect::new(port.x0, canvas.y0, port.x1, port.y0),
                    Rect::new(port.x0, port.y1, port.x1, canvas.y1),
                ] {
                    if strip.width() > 0.0 && strip.height() > 0.0 {
                        scene.fill(Fill::NonZero, Affine::IDENTITY, bg_color, None, &strip);
                    }
                }
            }
            scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &port);
        }

        self.render_element(
            scene,
            root_id,
            Affine::translate(Vec2 {
                x: self.initial_x - (viewport_scroll.x * self.scale),
                y: self.initial_y - (viewport_scroll.y * self.scale),
            }),
            viewport_clip_rect,
        );

        if !self.top_layer.is_empty() {
            self.paint_top_layer(scene, root_id, viewport_scroll, viewport_clip_rect);
        }

        if let Some(port) = scrollport {
            scene.pop_layer();
            self.paint_viewport_scrollbars(scene, port, viewport_bars, viewport_scroll);
        }

        // Render debug overlay
        if self.dom.devtools().highlight_hover {
            if let Some(node_id) = self.dom.as_ref().get_hover_node_id() {
                render_debug_overlay(
                    scene,
                    self.dom,
                    node_id,
                    self.scale,
                    self.initial_x,
                    self.initial_y,
                );
            }
        }
        if let Some(node_id) = self.dom.devtools().highlight_node {
            // Only nodes with element data have a layout to visualise
            if self
                .dom
                .as_ref()
                .get_node(node_id)
                .is_some_and(|node| node.element_data().is_some())
            {
                render_debug_overlay(
                    scene,
                    self.dom,
                    node_id,
                    self.scale,
                    self.initial_x,
                    self.initial_y,
                );
            }
        }
    }

    /// pyro: classic scrollbars: the viewport's scrollbars that show, in its gutters around
    /// `port` (the scrollport, scene px).
    fn paint_viewport_scrollbars(
        &self,
        scene: &mut impl PaintScene,
        port: Rect,
        bars: blitz_dom::ViewportScrollbars,
        scroll: blitz_dom::Point<f64>,
    ) {
        let s = self.scale;
        let thickness = bars.thickness as f64 * s;
        let Some(root) = self.root_element_id.and_then(|id| self.dom.get_node(id)) else { return };
        let colors = ClassicScrollbarColors::of(root);
        // The document's extent (CSS px): the root's margin box and its scrollable overflow.
        let (doc_width, doc_height) = self.root_element_id.and_then(|id| self.dom.get_node(id)).map_or((0.0, 0.0), |root| {
            let l = root.final_layout();
            let overflow = l.scrollable_overflow_rect.rect();
            (
                (l.location.x + l.size.width + l.margin.right).max(l.location.x + overflow.right) as f64,
                (l.location.y + l.size.height + l.margin.bottom).max(l.location.y + overflow.bottom) as f64,
            )
        });
        let origin = Vec2::new(self.initial_x, self.initial_y);
        let (port_w, port_h) = (port.width() / s, port.height() / s);
        let port_origin = ((port.x0 - origin.x) / s, (port.y0 - origin.y) / s);
        if bars.shown.y {
            let track = Rect::new(port.x1, port.y0, port.x1 + thickness, port.y1);
            let content = (doc_height - port_origin.1).max(port_h);
            paint_classic_scrollbar(scene, colors, Affine::IDENTITY, track, true, port_h, content, scroll.y, s);
        }
        if bars.shown.x {
            let track = Rect::new(port.x0, port.y1, port.x1, port.y1 + thickness);
            let content = (doc_width - port_origin.0).max(port_w);
            paint_classic_scrollbar(scene, colors, Affine::IDENTITY, track, false, port_w, content, scroll.x, s);
        }
        if bars.shown.x && bars.shown.y {
            let corner = Rect::new(port.x1, port.y1, port.x1 + thickness, port.y1 + thickness);
            scene.fill(Fill::NonZero, Affine::IDENTITY, colors.corner, None, &corner);
        }
    }

    /// pyro: the canvas, in scene space: the viewport, or the root element's box where that is
    /// larger.
    fn canvas_rect(&self) -> Rect {
        let size = self
            .root_element_id
            .and_then(|id| self.dom.get_node(id))
            .map_or(taffy::Size::ZERO, |root| root.final_layout().size);
        let width = (self.width as f64).max(size.width as f64 * self.scale);
        let height = (self.height as f64).max(size.height as f64 * self.scale);
        Rect::from_origin_size((self.initial_x, self.initial_y), (width, height))
    }

    /// pyro: paints the top layer above the document, bottom
    /// first: each element's `::backdrop` (a viewport-sized box: its
    /// background color, `opacity` and `backdrop-filter`), then the element
    /// in its own root stacking context, positioned as a box hoisted to the
    /// root element, unclipped and unaffected by its ancestors' effects.
    fn paint_top_layer(
        &self,
        scene: &mut impl PaintScene,
        root_id: NodeId,
        viewport_scroll: blitz_dom::Point<f64>,
        clip_rect: Rect,
    ) {
        let tree = self.dom.as_ref().tree();
        let root = &tree[root_id];
        let root_location = root.final_layout().location;
        // The transform `paint_scene` gives the root element.
        let root_transform = Affine::translate(Vec2 {
            x: self.initial_x - (viewport_scroll.x * self.scale),
            y: self.initial_y - (viewport_scroll.y * self.scale),
        });
        // The transform `draw_children` gives the root element's children.
        let root_children = (root_transform * Affine::translate(Vec2::new(
            root_location.x as f64 * self.scale,
            root_location.y as f64 * self.scale,
        )) * root.transform().as_deref().copied().unwrap_or_default())
        .then_translate(Vec2 {
            x: -root.scroll_offset().x * self.scale,
            y: -root.scroll_offset().y * self.scale,
        });
        for &id in &self.top_layer {
            let node = &tree[id];
            // An element not rendered (its own or an ancestor's `display:
            // none`, which the layout gives no box) has no backdrop either.
            if matches!(node.taffy_display(), taffy::Display::None)
                || node.primary_styles().is_none()
                || !self.rendered(id)
            {
                continue;
            }
            self.paint_backdrop(scene, id);
            // The root element paints where the document pass would have
            // put it (`render_element` applies its own location, transform
            // and scroll); anything else is hoisted to the root element.
            let transform = if id == root_id {
                root_transform
            } else {
                let position = self.dom.top_layer_position(id);
                root_children.pre_translate(Vec2::new(
                    position.x as f64 * self.scale,
                    position.y as f64 * self.scale,
                ))
            };
            self.top_layer_root.set(Some(id));
            self.render_element(scene, id, transform, clip_rect);
            self.top_layer_root.set(None);
        }
    }

    /// Whether `id` and its ancestors are styled and not `display: none`
    /// (the style traversal keeps stale styles under a `display: none`
    /// ancestor).
    fn rendered(&self, id: NodeId) -> bool {
        let mut cur = self.dom.get_node(id);
        while let Some(node) = cur {
            if node.is_element() {
                match node.primary_styles() {
                    Some(s) if s.get_box().display.is_none() => return false,
                    Some(_) => {}
                    None => return false,
                }
            }
            cur = node.parent.and_then(|p| self.dom.get_node(p));
        }
        true
    }

    /// The `::backdrop` of top-layer element `id`: a box filling the painted
    /// area below the element (UA style: `position: fixed; inset: 0`).
    fn paint_backdrop(&self, scene: &mut impl PaintScene, id: NodeId) {
        let Some(style) = self.dom.backdrop_style(id) else {
            return;
        };
        if style.get_box().display.is_none()
            || style.get_inherited_box().visibility != StyloVisibility::Visible
        {
            return;
        }
        let effects = style.get_effects();
        let opacity = effects.opacity;
        let color = style
            .slow_clone_background_color()
            .resolve_to_absolute(&style.clone_color());
        let backdrop_filter =
            convert_filters(&effects.backdrop_filter.0, self.scale, &style.clone_color(), self.dom).map(Arc::new);
        let visible_color = color.alpha > 0.0 && self.backgrounds;
        if opacity <= 0.0 || (!visible_color && backdrop_filter.is_none()) {
            return;
        }
        let rect = Rect::new(
            self.initial_x,
            self.initial_y,
            self.initial_x + self.width as f64,
            self.initial_y + self.height as f64,
        );
        self.layer_manager.maybe_with_layer(
            scene,
            opacity < 1.0 || backdrop_filter.is_some(),
            BlendMode::default(),
            opacity,
            Affine::IDENTITY,
            &rect,
            None,
            backdrop_filter,
            |scene| {
                if visible_color {
                    scene.fill(
                        Fill::NonZero,
                        Affine::IDENTITY,
                        color.as_srgb_color(),
                        None,
                        &rect,
                    );
                }
            },
        );
    }

    /// pyro: the effects of the inline boxes a float sits in: the product of their opacities and
    /// the style of the nearest one with a filter. An inline box's opacity and filter apply to all
    /// of its content (CSS Color 4 `opacity`, Filter Effects 1), a float in it too, which its block
    /// lays out and paints apart from the inline's fragments: the float is painted in a group of
    /// its own with them (with a filter of its own, the inline's is left out). WPT
    /// `inline-opacity-float-child`, `filtered-inline-applies-to-float`. The walk stops at the
    /// first ancestor that is no plain inline box (a split inline paints its own group).
    fn enclosing_inline_effects(
        &self,
        node: &Node,
        styles: &ComputedValues,
    ) -> (f32, Option<style::servo_arc::Arc<ComputedValues>>) {
        if styles.clone_float() == style::computed_values::float::T::None {
            return (1.0, None);
        }
        let own_filter = !styles.get_effects().filter.0.is_empty();
        let tree = self.dom.as_ref().tree();
        let mut opacity = 1.0;
        let mut filtered = None;
        let mut parent = node.parent;
        while let Some(id) = parent {
            let ancestor = &tree[id];
            if ancestor.is_split_inline() || ancestor.is_inline_with_only_block_children() {
                break;
            }
            let Some(style) = ancestor.primary_styles() else { break };
            if !style.clone_display().is_inline_flow() {
                break;
            }
            opacity *= style.clone_opacity();
            if !own_filter && filtered.is_none() && !style.get_effects().filter.0.is_empty() {
                filtered = Some((*style).clone());
            }
            parent = ancestor.parent;
        }
        (opacity, filtered)
    }

    fn node_transform(&self, node: &Node, parent: Affine) -> Affine {
        let location = node.final_layout().location;
        // pyro: a `position: sticky` box is painted where its scroll container holds it
        // (`Node::sticky_offset`), with its descendants.
        // The offset is snapped to device pixels: the box keeps the pixel alignment it was laid
        // out with (WPT `position-sticky-fractional-offset`).
        let sticky = node.sticky_offset();
        let sticky = if sticky == blitz_dom::Point::ZERO {
            Vec2::ZERO
        } else {
            // From the box's unrounded place, so that a box laid out at 10.5 (painted at 11) and
            // held 9.5 further is painted at 20.
            let unrounded = node.unrounded_layout().location;
            let snap = |offset: f64, unrounded: f32, rounded: f32| {
                ((unrounded as f64 + offset) * self.scale).round() - rounded as f64 * self.scale
            };
            Vec2::new(snap(sticky.x, unrounded.x, location.x), snap(sticky.y, unrounded.y, location.y))
        };
        parent
            * self.table_part_transform(node)
            * Affine::translate(Vec2::new(location.x as f64, location.y as f64) * self.scale + sticky)
            * node.transform().as_deref().copied().unwrap_or_default()
            * self.combined_text_compression(node)
    }

    /// pyro: `text-combine-upright: all` text wider than its 1em square (an inline-block of
    /// `horizontal-tb` writing in a vertical line, Stylo's `adjust_for_text_combine_upright`) is
    /// compressed horizontally to fit it, from its left edge (CSS Writing Modes 4 § 9.1.1: the UA
    /// may scale the text; Blink scales it).
    fn combined_text_compression(&self, node: &Node) -> Affine {
        let Some(style) = node.primary_styles() else {
            return Affine::IDENTITY;
        };
        // (Stylo's box of combined text, or `layout::combined_text`'s, `all` or `digits N`.)
        if !style.flags.contains(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED)
            || style.writing_mode.is_vertical()
        {
            return Affine::IDENTITY;
        }
        let parent_vertical = node
            .layout_parent
            .get()
            .and_then(|parent| self.dom.get_node(parent))
            .and_then(|parent| parent.primary_styles())
            .is_some_and(|parent| parent.writing_mode.is_vertical() && !parent.writing_mode.is_sideways());
        if !parent_vertical {
            return Affine::IDENTITY;
        }
        let Some(text) = node.element_data().and_then(|el| el.inline_layout_data.as_deref()) else {
            return Affine::IDENTITY;
        };
        let box_width = node.final_layout().size.width as f64;
        let text_width = text.layout.width() as f64 / text.layout.scale().max(f32::EPSILON) as f64;
        if box_width <= 0.0 || text_width <= box_width {
            return Affine::IDENTITY;
        }
        // An overflowing line starts at its start edge: the right one in right-to-left text.
        let scale = box_width / text_width;
        let start = if style.writing_mode.is_bidi_ltr() { 0.0 } else { (box_width - text_width) * self.scale };
        Affine::translate((-start * scale, 0.0)) * Affine::scale_non_uniform(scale, 1.0)
    }

    /// pyro: the transforms of a table cell's row group and row, which are no boxes of their own
    /// (their cells are the table's children): each about its own box, in the table's space.
    ///
    /// An absolutely positioned or fixed box whose containing block is a row or row group (the
    /// table lays it out, `containing_block_below_table`) paints in that part's transform too.
    fn table_part_transform(&self, node: &Node) -> Affine {
        let index = node.layout_data().table_cell_index;
        let position = node.taffy_position();
        let out_of_flow = matches!(position, taffy::Position::Absolute | taffy::Position::Fixed);
        if index == u32::MAX && !out_of_flow {
            return Affine::IDENTITY;
        }
        // (An out-of-flow box is laid out by its containing block: the table for one in a part.)
        let table = if index != u32::MAX { node.layout_parent.get() } else { node.containing_block() };
        let Some(table) = table.and_then(|id| self.dom.get_node(id)) else {
            return Affine::IDENTITY;
        };
        let Some(blitz_dom::node::SpecialElementData::TableRoot(ctx)) = table.element_data().map(|e| &e.special_data) else {
            return Affine::IDENTITY;
        };
        let parts = if index != u32::MAX {
            let Some(cell) = ctx.cells.get(index as usize).filter(|cell| cell.node_id == node.id) else {
                return Affine::IDENTITY;
            };
            let Some(row) = ctx.rows.get(cell.row as usize) else {
                return Affine::IDENTITY;
            };
            [row.group, Some(row.node_id)]
        } else {
            let Some(part) = self.dom.containing_block_below_table(table.id, node.id, position) else {
                return Affine::IDENTITY;
            };
            if let Some(row) = ctx.rows.iter().find(|row| row.node_id == part) {
                [row.group, Some(part)]
            } else if ctx.rows.iter().any(|row| row.group == Some(part)) {
                [Some(part), None]
            } else {
                return Affine::IDENTITY;
            }
        };
        let transformed = |id: NodeId| {
            self.dom.get_node(id).and_then(|node| node.primary_styles()).is_some_and(|style| {
                use style::values::generics::transform::{GenericRotate, GenericScale, GenericTranslate};
                let b = style.get_box();
                !b.transform.0.is_empty()
                    || !matches!(b.rotate, GenericRotate::None)
                    || !matches!(b.translate, GenericTranslate::None)
                    || !matches!(b.scale, GenericScale::None)
            })
        };
        // pyro: the sticky offsets of the cell's row group and row move their cells.
        let mut sticky = Vec2::ZERO;
        for id in parts.iter().flatten() {
            if let Some(part) = self.dom.get_node(*id) {
                let offset = part.sticky_offset();
                sticky += Vec2::new(offset.x, offset.y);
            }
        }
        let sticky = Affine::translate(Vec2::new((sticky.x * self.scale).round(), (sticky.y * self.scale).round()));
        if !parts.iter().flatten().any(|&id| transformed(id)) {
            return sticky;
        }
        let grid = ctx.computed_grid_info.borrow();
        let Some(grid) = grid.as_ref() else { return Affine::IDENTITY };
        let columns: Vec<taffy::Line<f32>> = PhysicalTracks::from_tracks(&grid.columns).iter().collect();
        let rows: Vec<taffy::Line<f32>> = PhysicalTracks::from_tracks(&grid.rows).iter().collect();
        let (Some(first), Some(last)) = (columns.first(), columns.last()) else { return Affine::IDENTITY };
        let top = ctx.grid_box.borrow().map_or(0.0, |(top, _)| top);
        let mut out = Affine::IDENTITY;
        for (at, id) in parts.into_iter().enumerate() {
            let Some(id) = id else { continue };
            if !transformed(id) {
                continue;
            }
            // The rows of the part: the row's own, or its group's.
            let spanned: Vec<&taffy::Line<f32>> = ctx
                .rows
                .iter()
                .zip(&rows)
                .filter(|(other, _)| if at == 0 { other.group == Some(id) } else { other.node_id == id })
                .map(|(_, track)| track)
                .collect();
            let (Some(y0), Some(y1)) = (spanned.first().map(|t| t.start), spanned.last().map(|t| t.end)) else {
                continue;
            };
            let Some(style) = self.dom.get_node(id).and_then(|node| node.primary_styles()) else { continue };
            let Some(t) = blitz_dom::resolve_2d_transform_for_size(style.get_box(), last.end - first.start, y1 - y0)
            else {
                continue;
            };
            let [a, b, c, d, e, f] = t.as_coeffs();
            let t = Affine::new([a, b, c, d, e * self.scale, f * self.scale]);
            let origin = Vec2::new(first.start as f64, (y0 + top) as f64) * self.scale;
            out *= Affine::translate(origin) * t * Affine::translate(-origin);
        }
        sticky * out
    }

    fn scrolled_transform(&self, node: &Node, transform: Affine) -> Affine {
        transform.then_translate(Vec2 {
            x: -node.scroll_offset().x * self.scale,
            y: -node.scroll_offset().y * self.scale,
        })
    }

    fn should_clip_node(&self, node: &Node, styles: &ComputedValues) -> bool {
        let box_style = styles.get_box();
        let contain_paint = box_style.slow_clone_contain().contains(Contain::PAINT) && {
            let display = styles.clone_display();
            let is_internal_table_box_other_than_cell = display.outside()
                == DisplayOutside::InternalTable
                && display.inside() != DisplayInside::TableCell;
            // (A replaced element is an atomic inline: paint containment applies to it. WPT
            // css-overflow `paint-containment-svg`.)
            (!display.is_inline_flow() || node.element_data().is_some_and(|e| e.image_data().is_some()))
                && !is_internal_table_box_other_than_cell
        };
        let element = node.element_data();
        // pyro: an SVG clips its content at its viewport itself, at its content box and only
        // where the content reaches past it (`ElementCx::draw_svg`).
        let svg = element.is_some_and(|e| matches!(e.image_data(), Some(blitz_dom::node::ImageData::Svg(_))));
        // pyro: an image whose author made its `overflow` visible shows its content past its
        // box (the UA sheet's `overflow: clip` is the default; `ElementCx::draw_image`).
        let overflows_visibly = element.is_some_and(|e| replaced_overflow_is_visible(e, styles));
        self.root_element_id != Some(node.id)
            && (element.and_then(|e| e.raster_image_data()).is_some() && !overflows_visibly
                || element.and_then(|e| e.sub_doc_data()).is_some()
                || element.and_then(|e| e.text_input_data()).is_some()
                || contain_paint
                // pyro: `overflow` on `body` is the viewport's, not body's.
                || ((!matches!(box_style.overflow_x, Overflow::Visible)
                    || !matches!(box_style.overflow_y, Overflow::Visible))
                    && !svg
                    // pyro: the UA sheet's `overflow: clip` of an `<img>`, `<video>` or
                    // `<canvas>` cuts its replaced content (`draw_image`), not its box: one
                    // without an image (a canvas, a broken image) is clipped as before it.
                    && !element.is_some_and(is_ua_clipped_replaced)
                    && !self.dom.as_ref().body_overflow_goes_to_viewport(node.id)))
    }

    fn draw_text_mask(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        transform: Affine,
        root: bool,
    ) {
        // pyro: an inline box's mask is its own glyphs, where its inline root draws them.
        if root && crate::text::draw_inline_text_mask(scene, self.dom, &self.draw_text_context, node_id, self.scale) {
            return;
        }
        let node = &self.dom.as_ref().tree()[node_id];
        if matches!(node.taffy_display(), taffy::Display::None) || (!root && node.is_out_of_flow())
        {
            return;
        }
        let Some(styles) = node.primary_styles() else {
            return;
        };
        if styles.get_inherited_box().visibility != StyloVisibility::Visible
            || node.element_data().is_none()
        {
            return;
        }
        let cx = self.element_cx(node, *node.final_layout(), transform, None);
        let clip = if cx.text_input.is_some() {
            cx.frame.text_input_clip_path()
        } else {
            cx.overflow_clip_path(&styles)
        };
        let pushed = self.layer_manager.maybe_push_layer(
            scene,
            self.should_clip_node(node, &styles),
            BlendMode::default(),
            1.0,
            transform,
            &clip,
            None,
            None,
        );
        let transform = self.scrolled_transform(node, transform);
        if node.flags.is_inline_root() {
            if let Some(layout) = &cx.element.inline_layout_data {
                let origin = node.content_box_origin();
                let x = origin.x as f64 - node.scroll_offset().x;
                let y = origin.y as f64 - node.scroll_offset().y
                    + layout.block_offset as f64;
                let text_transform =
                    transform * Affine::translate((x * self.scale, y * self.scale));
                // pyro: the glyphs where `draw_inline_layout` draws them, cut by an ellipsis as
                // there.
                let (text_shift, tops) = cx.text_placement(layout.block_offset as f64);
                let mut context = self.draw_text_context.borrow_mut();
                (context.text_shift, context.line_tops) = (text_shift, tops);
                stroke_inline_text(
                    scene,
                    self.dom,
                    node,
                    layout,
                    text_transform,
                    TextPaintOptions {
                        scale: self.scale,
                        brush_override: Some(Color::BLACK),
                        // A `background-clip: text` mask is the glyphs alone.
                        shadows: false,
                    },
                    &mut context,
                    tops,
                    self.scale,
                );
                (context.text_shift, context.line_tops) = (0.0, None);
                drop(context);
                // pyro: emphasis marks and ruby annotations are text the background shows in.
                let recording = self.draw_text_context.borrow().runs.is_some();
                for line in layout.layout.lines() {
                    crate::text::draw_text_annotations(
                        scene,
                        &line,
                        &layout.annotations,
                        &layout.text,
                        self.dom,
                        text_transform,
                        self.scale,
                        recording,
                        None,
                        Some(Color::BLACK),
                        None,
                    );
                }
            }
        }
        if let Some(children) = &*node.layout_children.borrow() {
            for &child_id in children {
                let child = &self.dom.as_ref().tree()[child_id];
                self.draw_text_mask(
                    scene,
                    child_id,
                    self.node_transform(child, transform),
                    false,
                );
            }
        }
        self.layer_manager.maybe_pop_layer(scene, pushed);
    }

    /// Renders a node, but is guaranteed that the node is an element
    /// This is because the font_size is calculated from layout resolution and all text is rendered directly here, instead
    /// of a separate text stroking phase.
    ///
    /// In Blitz, text styling gets its attributes from its container element/resolved styles
    /// In other libraries, text gets its attributes from a `text` element - this is not how HTML works.
    ///
    /// Approaching rendering this way guarantees we have all the styles we need when rendering text with not having
    /// to traverse back to the parent for its styles, or needing to pass down styles
    fn render_element(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // pyro: the element's clip while it paints (`PaintContext::paint_clip`).
        let outer_clip = self.paint_clip.replace(clip_rect);
        self.render_element_clipped(scene, node_id, parent_style_transform, clip_rect);
        self.paint_clip.set(outer_clip);
    }

    fn render_element_clipped(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let node = &self.dom.as_ref().tree()[node_id];
        // pyro: the phase this element is painted in, set by its painter for it alone
        // (`ElementCx::draw_flow_children`); its own descendants are painted whole unless it says.
        let phase = self.flow_phase.replace(FlowPhase::All);

        // Early return if the element is hidden
        if matches!(node.taffy_display(), taffy::Display::None) {
            return;
        }
        // pyro: an atomic inline that does not fit before a line's ellipsis is hidden whole
        // (Blink's `LineTruncator::HideChild`).
        if hidden_by_ellipsis(self.dom.as_ref(), node) {
            return;
        }
        // pyro: nor is a box after a line clamp's clamp point (blitz-dom `layout::line_clamp`).
        if node.flags.contains(blitz_dom::node::NodeFlags::HIDDEN_BY_CLAMP) {
            return;
        }

        // pyro: a top-layer element paints above the document
        // (`paint_top_layer`), not here.
        if !self.top_layer.is_empty()
            && self.top_layer_root.get() != Some(node_id)
            && self.dom.is_in_top_layer(node_id)
        {
            return;
        }

        // Only draw elements with a style
        let Some(styles) = node.primary_styles() else {
            return;
        };

        // Hide inputs with type=hidden
        // Implemented here rather than using the style engine for performance reasons
        if node.local_name() == "input" && node.attr(local_name!("type")) == Some("hidden") {
            return;
        }

        // pyro: `visibility` other than `visible` hides what the element paints itself (its box,
        // its own text and content); its descendants that are `visible` again still paint (CSS 2.1
        // § 11.2), and so does the region of an SVG filter.
        let hidden = styles.get_inherited_box().visibility != StyloVisibility::Visible;

        // pyro: a 3D transform hides the element and its content: its back face shows with
        // `backface-visibility: hidden`, or it lies behind the viewer (`Node::set_transform`).
        if node
            .flags
            .contains(blitz_dom::node::NodeFlags::TRANSFORM_HIDES)
        {
            return;
        }

        let effects = styles.get_effects();
        // pyro: a float in inline boxes with an opacity or a filter paints with their effects
        // (`enclosing_inline_effects`).
        let (inline_opacity, inline_filter_style) = self.enclosing_inline_effects(node, &styles);
        let opacity = effects.opacity * inline_opacity;
        if opacity == 0.0 {
            return;
        }
        let has_opacity = opacity < 1.0;
        let filter_list = match &inline_filter_style {
            Some(style) => &style.get_effects().filter.0,
            None => &effects.filter.0,
        };

        // pyro (lane H2): a plane of a 3D rendering context, or a plane drawn through a
        // perspective, is recorded and drawn sorted by depth (`render/three_d.rs`).
        if self.paint_plane(scene, node, parent_style_transform) {
            return;
        }

        let should_clip = self.should_clip_node(node, &styles);
        let is_text_input = node
            .element_data()
            .and_then(|el| el.text_input_data())
            .is_some();

        // Apply padding/border offset to inline root
        let taffy::Layout {
            size,
            border,
            padding,
            ..
        } = *node.final_layout();
        let box_size = Size::new(
            size.width as f64 * self.scale,
            size.height as f64 * self.scale,
        );
        let border_box = Rect::from_origin_size(Point::ORIGIN, box_size);
        // pyro: and the classic scrollbar gutters (`Node::gutter_insets`): the content box.
        let scaled_pb = (padding + border + node.gutter_insets()).map(f64::from);
        let content_position = kurbo::Point {
            x: scaled_pb.left,
            y: scaled_pb.top,
        };
        let content_box_size = kurbo::Size {
            width: (size.width as f64 - scaled_pb.left - scaled_pb.right) * self.scale,
            height: (size.height as f64 - scaled_pb.top - scaled_pb.bottom) * self.scale,
        };

        // Don't render things that are out of view
        let overflow = *node.scrollable_overflow();
        let transform = self.node_transform(node, parent_style_transform);
        let screen_transform = Affine::translate(Vec2 {
            x: -self.initial_x,
            y: -self.initial_y,
        }) * transform;
        let screen_bbox = screen_transform.transform_rect_bbox(overflow.union(border_box));

        // Cull elements that fall entirely outside the current clip rectangle. In addition to
        // the viewport, `clip_rect` is narrowed by any ancestor scrollport (see below), so this
        // also culls elements scrolled out of view inside a clipping/scrolling container.
        let outside = |bbox: Rect| {
            bbox.x1 < clip_rect.x0 || bbox.x0 > clip_rect.x1 || bbox.y1 < clip_rect.y0 || bbox.y0 > clip_rect.y1
        };
        let current_color = styles.clone_color();
        // pyro: an SVG filter that is no colour chain makes the list layers of its own
        // (`filters::filter_steps`), and paints its filter region even for an element that paints
        // nothing (empty or hidden).
        #[cfg(feature = "svg")]
        let filter_steps = crate::filters::filter_steps(
            filter_list,
            self.scale,
            &current_color,
            self.dom,
            (size.width, size.height),
            node.element_data().map_or(&[][..], |element| &element.reference_images[..]),
            false,
        );
        #[cfg(not(feature = "svg"))]
        let filter_steps: Option<Vec<()>> = None;
        let filter = match filter_steps {
            Some(_) => None,
            None => {
                convert_filters(filter_list, self.scale, &current_color, self.dom).map(Arc::new)
            }
        };
        // pyro: what a filter moves or spreads into the clip rectangle is in it (a drop shadow's
        // offset, a blur): `layers::filter_reach`. A graph's output lies in its filter region,
        // wherever the content is (a flood, a tile): the regions are in the element's ink
        // (`filters::steps_reach`), and an `feTile` reads its input anywhere in the region.
        #[cfg(feature = "svg")]
        let (steps_reach, filter_regions, tiles) = match &filter_steps {
            Some(steps) => crate::filters::steps_reach(steps, screen_transform, self.scale),
            None => (Rect::ZERO, None, false),
        };
        #[cfg(not(feature = "svg"))]
        let (steps_reach, filter_regions, tiles) = (Rect::ZERO, None::<Rect>, false);
        let filter_reach = match filter.as_deref() {
            Some(filter) => crate::layers::filter_reach(filter, screen_transform),
            None => steps_reach,
        };
        let filter_reach = Some(filter_reach).filter(|reach| *reach != Rect::ZERO);
        let screen_bbox = filter_regions.map_or(screen_bbox, |regions| screen_bbox.union(regions));
        let screen_bbox = match filter_reach {
            Some(reach) => Rect::new(
                screen_bbox.x0 + reach.x0,
                screen_bbox.y0 + reach.y0,
                screen_bbox.x1 + reach.x1,
                screen_bbox.y1 + reach.y1,
            ),
            None => screen_bbox,
        };
        // pyro: an inline root's own text decorations reach outside its box by as much as they
        // are thick (`text-decoration-thickness: 4em`).
        // pyro: nor is one culled whose positioned inline boxes it leaves to their paint-tree entries
        // (`inline_pass_entries`): a relative or sticky offset can move them into view.
        if outside(screen_bbox)
            && !(node.flags.is_inline_root() && {
                let reach = crate::text::decoration_ink_reach(&styles) * self.scale;
                reach > 0.0 && !outside(screen_bbox.inflate(reach, reach))
            })
            && !(node.flags.is_inline_root() && self.dom.inline_root_has_relative_spans(node_id))
            // pyro: nor one painting boxes that escape its clip (`has_clip_escapes`): they lie
            // wherever their containing block puts them (a fixed box under a scroller scrolled out
            // of view; WPT css-position `position-sticky-escape-scroller-002`).
            && !has_clip_escapes(self.dom.as_ref().tree(), node)
        {
            return;
        }

        // Cull elements whose transformed geometry is not representable in f32 (e.g.
        // `transform: scale(99e99)`): the renderer works in f32 and flattening such paths
        // can attempt to allocate an unbounded number of line segments.
        let f32_max = f64::from(f32::MAX);
        if !screen_bbox.is_finite()
            || [
                screen_bbox.x0,
                screen_bbox.y0,
                screen_bbox.x1,
                screen_bbox.y1,
            ]
            .iter()
            .any(|c| c.abs() > f32_max)
        {
            return;
        }

        // Optimise zero-area (/very small area) clips by not rendering at all
        // pyro: but for an SVG filter graph, which paints its filter region (`filter_steps`).
        let clip_area = content_box_size.width * content_box_size.height;
        let overflow_area =
            node.scrollable_overflow().width() * node.scrollable_overflow().height();
        // pyro: a box with borders or padding around its empty content box still paints them (an
        // `<hr>`, `overflow: hidden` in the UA sheet; WPT css-lists `outside-marker-with-hr-content`).
        if should_clip
            && clip_area < 0.01
            && overflow_area < 0.01
            && border_box.area() < 0.01
            && filter_steps.is_none()
            // pyro: what it paints outside its clip still shows (`has_clip_escapes`).
            && !has_clip_escapes(self.dom.as_ref().tree(), node)
            // pyro: and so does its outline, around its zero-width box (an empty button's;
            // WPT css-inline `empty-span-size-001`).
            && !has_visible_outline(&styles)
        {
            return;
        }

        #[cfg(feature = "custom-widget")]
        let custom_widget_scene = self.custom_widget_scenes.get(&(self.dom.id(), node_id));
        #[cfg(not(feature = "custom-widget"))]
        let custom_widget_scene = None;

        // Apply CSS transform property (where transforms are 2d)

        let mut cx = self.element_cx(node, *node.final_layout(), transform, custom_widget_scene);

        // If this element clips its overflow it establishes a scrollport: narrow the clip
        // rectangle passed to descendants to the visible (clipped) region so that content
        // scrolled out of view is culled rather than drawn and clipped away. The box used
        // here matches the clip applied to the content below.
        // pyro: under a filter, content its reach moves into the rectangle is visible.
        let source_rect = match filter_reach {
            Some(reach) => crate::layers::filter_source_area(clip_rect, reach),
            None => clip_rect,
        };
        // pyro: an `feTile` in view reads its input anywhere in its filter region.
        let source_rect = match filter_regions {
            Some(regions) if tiles && !outside(regions) => {
                source_rect.union(crate::layers::filter_source_area(regions, steps_reach))
            }
            _ => source_rect,
        };
        let child_clip_rect = if should_clip {
            let clip_box = if is_text_input {
                cx.frame.text_input_clip_path()
            } else {
                cx.overflow_clip_path(&styles)
            };
            source_rect.intersect(screen_transform.transform_rect_bbox(clip_box.bounding_box()))
        } else {
            source_rect
        };

        // pyro: a flex or grid container painted in its painter's phases (`items_in_phases`): its box
        // with the block backgrounds, its items whole with the inline content, as inline blocks.
        if phase != FlowPhase::All && node.items_paint_in_ancestor_phases() {
            match phase {
                FlowPhase::Background if !hidden => {
                    cx.draw_outset_box_shadow(scene);
                    if cx.context.backgrounds && self.canvas_background != Some(node_id) {
                        cx.draw_background(scene);
                    }
                    cx.draw_inset_box_shadow(scene);
                    cx.draw_border(scene);
                }
                FlowPhase::Foreground => {
                    let children = node.paint_children.borrow().clone().unwrap_or_default();
                    for child_id in children {
                        self.flow_phase.set(FlowPhase::All);
                        cx.draw_paint_child(scene, child_id, cx.transform, child_clip_rect);
                    }
                }
                FlowPhase::Outline if !hidden => cx.draw_outline(scene),
                _ => {}
            }
            return;
        }

        // pyro: a scroll container painted in its painter's phases (`scroller_in_phases`): its own
        // box unclipped, its content clipped by its overflow clip and scrolled, its scrollbars
        // over its inline content.
        if phase != FlowPhase::All && should_clip && node.scroller_paints_in_ancestor_phases() {
            let clip = cx.overflow_clip_path(&styles);
            match phase {
                FlowPhase::Background if !hidden => {
                    cx.draw_outset_box_shadow(scene);
                    if cx.context.backgrounds && self.canvas_background != Some(node_id) {
                        cx.draw_background(scene);
                    }
                    cx.draw_inset_box_shadow(scene);
                    cx.draw_border(scene);
                }
                FlowPhase::Outline => cx.draw_outline(scene),
                _ => {}
            }
            let unscrolled = cx.transform;
            scene.push_clip_layer(Fill::NonZero, unscrolled, &clip);
            cx.transform = self.scrolled_transform(node, unscrolled);
            if phase == FlowPhase::Foreground {
                cx.draw_marker(scene, content_position);
                cx.draw_inline_layout(scene, content_position);
            }
            cx.draw_flow_children(scene, cx.transform, child_clip_rect, phase);
            cx.transform = unscrolled;
            scene.pop_layer();
            #[cfg(feature = "scrollbars")]
            if phase == FlowPhase::Foreground && !hidden {
                cx.draw_scrollbars(scene);
            }
            // pyro: and its classic scrollbars, over its inline content.
            if phase == FlowPhase::Foreground && !hidden {
                cx.draw_classic_scrollbars(scene);
            }
            return;
        }

        // pyro: a box of the normal flow painted in one of its painter's phases (`FlowPhase`). It
        // makes no layer of its own: no effect, no clip (`Node::paints_in_ancestor_phases`).
        if phase != FlowPhase::All {
            match phase {
                FlowPhase::Background => {
                    if !hidden && !node.is_inline_with_only_block_children() && !node.is_split_inline() {
                        cx.draw_outset_box_shadow(scene);
                        if cx.context.backgrounds && self.canvas_background != Some(node_id) {
                            cx.draw_background(scene);
                        }
                        cx.draw_inset_box_shadow(scene);
                        cx.draw_border(scene);
                    }
                    cx.stroke_devtools(scene);
                }
                FlowPhase::Foreground => {
                    // pyro: the marker first: it is at the start of its line, under text that
                    // overlaps it (WPT css-pseudo `outside-marker-paint-order`).
                    cx.draw_marker(scene, content_position);
                    cx.draw_inline_layout(scene, content_position);
                    // pyro: a block-level frame's document, its replaced content
                    // (`block_level_frame_in_phases`).
                    let skipped = cx.style.get_box().slow_clone_content_visibility()
                        == style::values::specified::box_::ContentVisibility::Hidden;
                    if !hidden && !skipped {
                        cx.draw_sub_document(scene);
                    }
                    if cx.outline_line().is_some() {
                        self.deferred_outlines.set(true);
                    }
                }
                FlowPhase::Outline => cx.draw_outline(scene),
                FlowPhase::All => {}
            }
            cx.draw_flow_children(scene, cx.transform, child_clip_rect, phase);
            return;
        }

        // Compute clip-path (if any) and wrap all rendering in a clip layer
        let element_clip = cx.element_clip();
        // pyro: a clip-path makes the element a backdrop root (`layers::backdrop_root`).
        let has_clip_path = element_clip.is_some();

        // CSS 2 `clip` property (absolutely positioned elements only). Applies to the whole of
        // the element's rendering (including outlines and shadows), so it is the outermost layer.
        let css_clip_rect = cx.css_clip_rect();
        if css_clip_rect.is_some_and(|rect| rect.is_zero_area()) {
            return;
        }

        // pyro: a `url()` naming an SVG filter graph (one that is no colour chain) is the backdrop
        // filter as that graph, in the element's user space (CSS px from its border box's corner),
        // which the backdrop markers are pushed in (WPT filter-effects
        // `backdrop-filter-svg-displacement`, `backdrop-filter-reference-filter-mutated`).
        #[cfg(feature = "svg")]
        let backdrop_graph = match crate::filters::filter_steps(
            &effects.backdrop_filter.0,
            self.scale,
            &current_color,
            self.dom,
            ((cx.frame.border_box.width() / self.scale) as f32, (cx.frame.border_box.height() / self.scale) as f32),
            node.element_data().map_or(&[][..], |element| &element.reference_images[..]),
            true,
        )
        .as_deref()
        {
            Some([crate::filters::FilterStep::Graph(graph)]) => Some(graph.filter.clone()),
            _ => None,
        };
        #[cfg(not(feature = "svg"))]
        let backdrop_graph: Option<Arc<anyrender::Filter>> = None;
        let backdrop_space = match backdrop_graph {
            Some(_) => Affine::translate(cx.frame.border_box.origin().to_vec2()) * Affine::scale(self.scale),
            None => Affine::IDENTITY,
        };
        let backdrop_filter = backdrop_graph.or_else(|| {
            convert_filters(&effects.backdrop_filter.0, self.scale, &current_color, self.dom).map(Arc::new)
        });
        // pyro: `backdrop-filter`: the backdrop is captured here, before the element's own layers
        // (its clip-path, mask and blend mode are no backdrop root for it), and drawn first in its
        // effect group (`paint_effect_group`). See anyrender_vello_cpu's `backdrop`.
        // (The clip in the filter's space, which the markers' transform maps on: `backdrop_space`.)
        let backdrop_clip = backdrop_filter.as_ref().map(|_| (backdrop_space.inverse() * cx.frame.border_box_path(), backdrop_space));
        if let (Some(backdrop), Some((clip, space))) = (&backdrop_filter, &backdrop_clip) {
            scene.push_layer(Fill::NonZero, BlendMode::default(), 0.0, cx.transform * *space, clip, None, Some(backdrop.clone()));
            scene.pop_layer();
        }
        let css_clip_layer_pushed = self.layer_manager.maybe_push_layer(
            scene,
            css_clip_rect.is_some(),
            BlendMode::default(),
            1.0,
            cx.transform,
            &css_clip_rect.unwrap_or(Rect::ZERO),
            None,
            None,
        );

        // Use the viewport in scene coordinates so outlines, shadows and overflowing
        // descendants are not clipped to the element's border box.
        let blend = convert_blend_mode(effects.mix_blend_mode);
        let isolate = styles.get_box().isolation == Isolation::Isolate
            || self.blend_isolated.contains(&node_id);
        let viewport = Rect::new(
            self.initial_x,
            self.initial_y,
            self.initial_x + self.width as f64,
            self.initial_y + self.height as f64,
        );
        // pyro: in a local-raster layer the plane is rasterized a pixel or two past the frame, for
        // the pixels at the frame's edges to sample; a group clipped to the viewport there cut
        // through them, and its antialiased edge came out a fifth lighter along the edge of the
        // screenshot. Past the frame the margin shows nothing.
        let viewport = if self.local_raster_depth.get() > 0 { viewport.inflate(8.0, 8.0) } else { viewport };
        // pyro (lane H2): in a plane's recording the element's stand-in transform is not where the
        // plane is drawn: a group clipped to the viewport keeps all of the plane.
        let viewport = if self.three_d.depth.get() > 0 { Rect::new(-1e7, -1e7, 1e7, 1e7) } else { viewport };
        let blend_layer_pushed = self.layer_manager.maybe_push_layer(
            scene,
            isolate || blend != BlendMode::default(),
            blend,
            if isolate { 1.0 - f32::EPSILON } else { 1.0 },
            Affine::IDENTITY,
            &viewport,
            None,
            None,
        );
        // pyro: a backdrop root none of the element's layers marks (`layers::backdrop_root`).
        let backdrop_root_pushed = self.layer_manager.maybe_push_layer(
            scene,
            self.backdrop_ancestors.contains(&node_id)
                && (crate::layers::backdrop_root(has_clip_path, cx.has_css_mask() || cx.has_mask_border(), &styles.get_box().will_change)
                    || self.dom.animates_backdrop_root(node_id)),
            BlendMode::default(),
            1.0,
            Affine::IDENTITY,
            &viewport,
            Some(crate::layers::backdrop_root_marker()),
            None,
        );

        // pyro: a flat element with a 3D transform and element children is rasterized in its own
        // plane and drawn through its transform (`layers::local_raster_marker`), as Chromium
        // composites it: edges its boxes share resolve in that plane, where its layout puts them
        // on whole pixels. Drawn each through the transform, a child's edge over the element's
        // took in what the element's edge let through, and a child blending there came out up to
        // 6 levels off the same colours painted without a blend (WPT
        // `mix-blend-mode-parent-with-3D-transform`). A box with no element in it shares no edge
        // with another: it keeps its own edges, with none of a resampled image's rounding.
        // Its own blend mode applies to that image; its effects, inside, in its plane.
        // pyro: so is a box turned by a 2D transform (a rotation, a skew) holding a box that blends
        // (`mix-blend-mode`): Chromium draws it as a surface in its own unturned space, where its
        // background and its overflow clip are whole pixels, and turns the surface: the blend sees
        // an opaque backdrop up to the clip's edge, which is anti-aliased once, when the surface is
        // drawn (WPT compositing `mix-blend-mode-rotated-clip`).
        let turned_blending = || {
            node.transform().as_deref().is_some_and(|t| {
                let [_, b, c, _, _, _] = t.as_coeffs();
                b.abs() > 1e-9 || c.abs() > 1e-9
            }) && self.has_blending_descendant(node)
        };
        let local_raster_pushed = self.composite_3d
            && (node.flags.contains(blitz_dom::node::NodeFlags::FLATTENS_3D) || turned_blending())
            && node.children.iter().any(|&child| self.dom.get_node(child).is_some_and(|child| child.is_element()))
            // pyro (lane H2): a plane drawn through a perspective is rasterized in its plane already.
            && !(self.three_d.depth.get() > 0 && node.plane_3d().is_some_and(|plane| plane.is_projective()));
        if local_raster_pushed {
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.0,
                cx.transform,
                &Rect::new(-1e7, -1e7, 1e7, 1e7),
                Some(crate::layers::local_raster_marker()),
                None,
            );
            self.local_raster_depth.set(self.local_raster_depth.get() + 1);
        }

        // pyro: a split inline's fragments are painted by its pieces' lines (`inline_box_fragments`).
        let paints_own_box = !node.is_inline_with_only_block_children()
            && !node.is_split_inline()
            && !hides_empty_cell(self.dom.as_ref(), node)
            && !self
                .spanner_row_container
                .get()
                .is_some_and(|container| self.dom.as_ref().carries_spanners_of(container, node_id));

        // pyro: `opacity` and `filter` apply to everything the element paints, as one group
        // (Chromium's effect node): its outset box-shadow, its box, its content and descendants
        // (positioned ones too: the element is their stacking context) and its outline. The
        // group is recorded first, and its layer covers what it painted (its ink, plus what the
        // filter reaches), where the layer was the border box (descendants and the outline outside
        // it were cut off, the shadow was drawn at full strength outside the layer).
        let has_effect_layer = has_opacity || filter.is_some() || backdrop_filter.is_some() || filter_steps.is_some();
        let element = ElementPaint {
            node,
            paints_own_box: paints_own_box && !hidden,
            hidden,
            should_clip,
            is_text_input,
            content_position,
            child_clip_rect,
            clip_rect: source_rect,
        };

        // clip-path clip layer (it clips the shadow and the outline too, as in Chromium)
        clip_path::with_element_clip(
            scene,
            element_clip.as_ref(),
            cx.transform,
            self.scale,
            element_clip.as_ref().map_or(Rect::ZERO, |_| cx.visible_area()),
            |scene| {
                // If the element has a CSS `mask`, then push an isolation layer for the
                // masked content. The mask is applied when the layer is popped below.
                let mask_layer_pushed = cx.maybe_push_css_mask_layer(scene);
                // `cx.transform` is mutated to apply scroll offsets while drawing content.
                // Save it so that the mask can be drawn untransformed by scroll offsets.
                let unscrolled_transform = cx.transform;

                if has_effect_layer {
                    let mut group = Scene::new();
                    self.paint_element(&mut cx, &mut group, &element);
                    self.paint_effect_group(
                        scene,
                        group,
                        EffectLayer {
                            transform: unscrolled_transform,
                            visible: clip_rect,
                            border_box: cx.frame.border_box,
                            opacity,
                            filter,
                            backdrop_filter,
                            backdrop_clip,
                            #[cfg(feature = "svg")]
                            steps: filter_steps,
                        },
                    );
                } else {
                    self.paint_element(&mut cx, scene, &element);
                }

                // Apply the CSS `mask` (if any) to the content drawn above
                cx.transform = unscrolled_transform;
                cx.maybe_pop_css_mask_layer(scene, mask_layer_pushed);
            },
        );

        if local_raster_pushed {
            scene.pop_layer();
            self.local_raster_depth.set(self.local_raster_depth.get() - 1);
        }
        self.layer_manager
            .maybe_pop_layer(scene, backdrop_root_pushed);
        self.layer_manager
            .maybe_pop_layer(scene, blend_layer_pushed);
        self.layer_manager
            .maybe_pop_layer(scene, css_clip_layer_pushed);
    }

    /// Paints an element and its descendants, in Chromium's order: outset box-shadow,
    /// background, inset box-shadow, border, the content clipped to the padding box (when the
    /// element clips), scrollbars, outline. Leaves `cx.transform` as it found it.
    fn paint_element(
        &self,
        cx: &mut ElementCx<'_, '_>,
        scene: &mut impl PaintScene,
        element: &ElementPaint<'_>,
    ) {
        let ElementPaint {
            node,
            paints_own_box,
            hidden,
            should_clip,
            is_text_input,
            content_position,
            child_clip_rect,
            clip_rect,
        } = *element;
        let unscrolled_transform = cx.transform;
        // pyro: a clip handed over by an ancestor that clips (`BlitzDomPainter::handed_clips`): it
        // cuts this element's own box and its content, inside its effect group, except the boxes
        // its owner does not contain.
        let handed = self.handed_clip.borrow_mut().take();
        let outer_escape_owner = self.escape_owner.get();
        if let Some((owner, device_clip)) = &handed {
            self.escape_owner.set(Some((node.id, *owner)));
            scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, device_clip);
        }

        if paints_own_box {
            // pyro: the root element paints the canvas background, under its own shadow, in
            // place of its background; the body whose background that is paints none.
            let is_root = self.root_element_id == Some(node.id);
            if is_root
                && cx.context.backgrounds
                && let Some(source) = self.canvas_background
            {
                // Not under the root element's own transform (Chromium paints the canvas in
                // the view's space): its images are positioned against the untransformed box.
                let own = node.transform().as_deref().copied().unwrap_or_default();
                if own.determinant().abs() > 1e-12 {
                    let canvas_cx = ElementCx {
                        transform: cx.transform * own.inverse(),
                        frame: cx.frame.clone(),
                        style: cx.style.clone(),
                        ..*cx
                    };
                    canvas_cx.draw_canvas_background(scene, source, self.canvas_rect());
                }
            }
            cx.draw_outset_box_shadow(scene);
            // pyro: a cell of a positioned row or row group paints their backgrounds first.
            if cx.context.backgrounds && !hidden {
                cx.draw_positioned_table_part_backgrounds(scene);
            }
            if cx.context.backgrounds && !is_root && self.canvas_background != Some(node.id) {
                cx.draw_background(scene);
            }
            cx.draw_inset_box_shadow(scene);
        }
        if cx.context.backgrounds && !hidden {
            cx.draw_table_row_backgrounds(scene);
        }
        // pyro: a collapsed table whose cells hold half of the borders around them draws the
        // borders over the cells' backgrounds, after its children (Chromium paints collapsed
        // borders after the cells' backgrounds); one with uniform gutters draws them here.
        // pyro: and a collapsed table paints its parts in phases, its borders after the
        // backgrounds of their blocks, before their inline content (`table_borders_in_phases`).
        let borders_in_phases = cx.table_borders_in_phases();
        let borders_after_cells = !borders_in_phases && matches!(&cx.element.special_data,
            blitz_dom::node::SpecialElementData::TableRoot(table) if table.collapsed_edges.is_some());
        if !borders_after_cells && !borders_in_phases && !hidden {
            cx.draw_table_borders(scene);
        }
        if paints_own_box {
            cx.draw_border(scene);
        }
        if !hidden {
            cx.stroke_devtools(scene);
        }
        if handed.is_some() {
            scene.pop_layer();
        }

        let own_clip = if is_text_input { cx.frame.text_input_clip_path() } else { cx.overflow_clip_path(&cx.style) };
        // (A handed clip, in this element's space; the element clips nothing itself then.)
        let handed_local = handed.as_ref().map(|(_, device_clip)| cx.transform.inverse() * device_clip.clone());
        let should_clip = should_clip || handed.is_some();
        let clip = handed_local.as_ref().unwrap_or(&own_clip);
        // pyro: a clipping box that is no stacking context whose child holds boxes it does not
        // contain clips its children each (a clip layer split is the same clip) and hands the
        // child its clip (`ElementCx::clip_transfer_child`).
        let transfer = (should_clip && handed.is_none() && !is_text_input && !node.flags.is_inline_root())
            .then(|| cx.clip_transfer_child())
            .flatten();
        let outer_handed_clips = transfer.map(|child| {
            self.handed_clips.replace(Some((node.id, child, cx.transform * clip.clone(), clip_rect)))
        });
        let should_clip = should_clip && transfer.is_none();

        // pyro: the out-of-flow boxes of this clipping stacking context whose containing block
        // lies outside it are not clipped by it: they are painted after its clip, in its
        // scrolled space (Chromium clips a box by its containing-block chain; WPT css-position
        // `position-fixed-scroll-nested-fixed`, `position-absolute-under-non-containing-stacking-context`).
        // The z-ordered children keep their places in the stacking context's paint order (as hit
        // testing visits them); only the clip differs: those with a negative `z-index` are
        // painted before the clip layer, the positioned level and positive `z-index` after it,
        // each in the clip unless it escapes it.
        let escapes = should_clip && cx.has_clip_escapes();
        let outer_escapes = self.clip_escapes_of.replace(None);
        // pyro: a box with an outline paints it before its positioned descendants (z-index auto
        // or 0, and positive), as Chromium paints a layer's outline before its z-order lists
        // (`PaintLayerPainter`): they go after it, in its clip (WPT css-overflow `clip-005`).
        let defers = !hidden && (escapes || cx.defers_positioned());
        let outer_defers = self.positioned_after_outline_of.replace(if defers { Some(node.id) } else { None });
        let outer_neg = self.neg_z_outside_clip_of.replace(if escapes { Some(node.id) } else { None });
        if escapes {
            let content_position = Point {
                x: content_position.x - node.scroll_offset().x,
                y: content_position.y - node.scroll_offset().y,
            };
            let transform = self.scrolled_transform(node, cx.transform);
            let saved = cx.transform;
            cx.transform = transform;
            cx.register_negative_inline_spans(content_position);
            cx.transform = saved;
            if let Some(sc) = &node.stacking_context {
                for child in sc.neg_z_hoisted_children() {
                    let clipped = should_clip && !cx.escapes_own_clip(child.node_id);
                    self.layer_manager.maybe_with_layer(scene, clipped, BlendMode::default(), 1.0, cx.transform, clip, None, None, |scene| {
                        cx.draw_hoisted_children(scene, std::iter::once(child), transform, child_clip_rect_for(clipped, clip_rect, child_clip_rect));
                    });
                }
            }
        }
        // Clip layer if box requires clipping. Opacity set to 1.0
        self.layer_manager.maybe_with_layer(
            scene,
            should_clip,
            BlendMode::default(),
            1.0, // opacity
            cx.transform,
            clip,
            None,
            None,
            |scene| {
                // Now that background has been drawn, offset pos and cx in order to draw our contents scrolled
                let content_position = Point {
                    x: content_position.x - node.scroll_offset().x,
                    y: content_position.y - node.scroll_offset().y,
                };

                cx.transform = self.scrolled_transform(node, cx.transform);
                // pyro: an `<svg>` paints whatever its own visibility: what in it is visible
                // (`visibility: visible` on a child, a filter's region) is (usvg drops the hidden
                // shapes; WPT filter-effects `svg-visibility-hidden-element-with-filter-002`).
                // pyro: `content-visibility: hidden` skips a replaced element's content (its image,
                // canvas, video frame, inline SVG or document; CSS Contain 2 § 4.1).
                let skipped = cx.style.get_box().slow_clone_content_visibility()
                    == style::values::specified::box_::ContentVisibility::Hidden;
                // pyro: a `content: filter(<image>, ...)` image is drawn through its filters, in a
                // layer over the content box (WPT filter-effects `filter-function-001`).
                let content_filter = cx.content_image_filter();
                if let Some(filter) = &content_filter {
                    scene.push_layer(
                        Fill::NonZero,
                        BlendMode::default(),
                        1.0,
                        cx.transform,
                        &cx.frame.content_box,
                        Some(filter.clone()),
                        None,
                    );
                }
                #[cfg(feature = "svg")]
                if !skipped {
                    cx.draw_svg(scene);
                }
                if !hidden && !skipped && content_filter.is_some() {
                    cx.draw_image(scene);
                }
                if content_filter.is_some() {
                    scene.pop_layer();
                }
                if !hidden {
                    if !skipped && content_filter.is_none() {
                        cx.draw_image(scene);
                        cx.draw_selection_tint(scene);
                        cx.draw_content_gradient(scene);
                    }
                    #[cfg(feature = "custom-widget")]
                    cx.draw_custom_widget(scene);
                    if !skipped {
                        cx.draw_sub_document(scene);
                    }
                    cx.draw_input(scene);
                    cx.draw_text_input_text(scene, content_position);
                }
                // pyro: a multi-column container paints its column rules over the border, under
                // the content, clipped and scrolled with it, and its content once per column
                // (`render/multicol.rs`).
                if paints_own_box {
                    cx.draw_column_rules(scene, child_clip_rect);
                }
                if !cx.draw_columns(scene, content_position, child_clip_rect) {
                    // pyro: floats among the paint children: in the phases of CSS 2.1 Appendix E.
                    // pyro: and so are in-flow boxes that overlap (`FLOW_BOXES_OVERLAP`).
                    if borders_in_phases
                        || node.flags.intersects(
                            blitz_dom::node::NodeFlags::HAS_FLOAT_PAINT_CHILDREN
                                | blitz_dom::node::NodeFlags::FLOW_BOXES_OVERLAP,
                        )
                    {
                        cx.draw_children_in_phases(
                            scene,
                            cx.transform,
                            child_clip_rect,
                            content_position,
                        );
                        return;
                    }
                    // pyro: the stacking contexts with a negative z-index go under the box's own
                    // inline content (CSS 2.1 Appendix E, step 3 before step 7).
                    cx.register_negative_inline_spans(content_position);
                    cx.draw_neg_z_children(scene, cx.transform, child_clip_rect);
                    if !hidden {
                        cx.draw_marker(scene, content_position);
                    }
                    cx.draw_inline_layout(scene, content_position);
                    cx.draw_children_above(scene, cx.transform, child_clip_rect);
                    if !cx.has_positioned_level_children() {
                        cx.draw_inline_boxes_over_atomics(scene, content_position);
                    }
                }
            },
        );
        self.clip_escapes_of.set(outer_escapes);
        self.positioned_after_outline_of.set(outer_defers);
        self.neg_z_outside_clip_of.set(outer_neg);
        cx.transform = unscrolled_transform;
        // pyro: the deferred positioned children (below) are drawn each in the clip already.
        if let Some(outer) = outer_handed_clips {
            *self.handed_clips.borrow_mut() = outer;
        }
        // pyro: the borders of a collapsed table with positioned parts, after its last positioned
        // cell (`table_borders_deferred`).
        cx.draw_deferred_table_borders(scene);
        if hidden {
            self.escape_owner.set(outer_escape_owner);
            return;
        }
        if borders_after_cells {
            cx.draw_table_borders(scene);
        }

        // Overlay scrollbars, drawn unscrolled above the
        // clipped content.
        #[cfg(feature = "scrollbars")]
        cx.draw_scrollbars(scene);
        // pyro: classic scrollbars (`blitz_dom::ScrollbarMode::Classic`) in their gutters.
        cx.draw_classic_scrollbars(scene);

        // pyro: and those of a table's rows and row groups, which paint no box.
        cx.draw_table_part_outlines(scene);
        // The outline goes over the box and its contents (Chromium paints it
        // in a phase of its own after the foreground): one with a negative
        // `outline-offset` lies inside the box, over its background.
        cx.draw_outline(scene);
        if defers {
            let transform = self.scrolled_transform(node, cx.transform);
            cx.draw_positioned_after_outline(scene, transform, should_clip, clip, clip_rect, child_clip_rect);
        }
        self.escape_owner.set(outer_escape_owner);
    }

    /// pyro: the layer of an element's opacity and filters around the `group` it painted. The
    /// layer is the group's ink (what its commands can touch, [`crate::bounds::scene_bounds`]),
    /// grown by what the filter reaches and cut to the visible area, in the element's space (the
    /// filter's lengths are in it). A group that paints nothing visible draws nothing.
    fn paint_effect_group(&self, scene: &mut impl PaintScene, mut group: Scene, layer: EffectLayer) {
        let EffectLayer {
            transform,
            visible,
            border_box,
            opacity,
            filter,
            backdrop_filter,
            backdrop_clip,
            #[cfg(feature = "svg")]
            steps,
        } = layer;
        #[cfg(feature = "svg")]
        if let Some(steps) = steps {
            let layer = EffectLayer {
                transform,
                visible,
                border_box,
                opacity,
                filter,
                backdrop_filter,
                backdrop_clip,
                steps: None,
            };
            self.paint_filter_steps(scene, group, layer, steps);
            return;
        }
        // A group of one shape in one colour needs no layer: its opacity is that colour's.
        if filter.is_none() && backdrop_filter.is_none() && crate::bounds::fold_opacity(&mut group.commands, opacity) {
            scene.append_scene(group, Affine::IDENTITY);
            return;
        }
        let mut ink = crate::bounds::scene_bounds(&group.commands);
        if backdrop_filter.is_some() {
            let border_box = transform.transform_rect_bbox(border_box);
            ink = Some(ink.map_or(border_box, |ink| ink.union(border_box)));
        }
        let Some(ink) = ink else {
            return;
        };
        if transform.determinant().abs() < 1e-12 {
            return;
        }
        let inverse = transform.inverse();
        let mut bounds = inverse.transform_rect_bbox(ink);
        if let Some(filter) = &filter {
            let reach = filter.expansion_rect();
            bounds = Rect::new(
                bounds.x0 + reach.x0,
                bounds.y0 + reach.y0,
                bounds.x1 + reach.x1,
                bounds.y1 + reach.y1,
            );
        }
        // The visible area is in screen coordinates (the scene's, less the initial offset).
        let visible = visible + Vec2::new(self.initial_x, self.initial_y);
        let bounds = bounds.intersect(inverse.transform_rect_bbox(visible)).expand();
        if !(bounds.width() > 0.0 && bounds.height() > 0.0) {
            return;
        }
        // pyro: an opaque group without a filter around a backdrop filter is a clip layer (the same
        // pixels; vello_cpu's u8 compositing of a full layer rounded a level apart by where clips
        // around it end).
        let group_backdrop = backdrop_filter.as_ref().filter(|_| opacity < 1.0 || filter.is_some()).cloned();
        let pushed = self.layer_manager.maybe_push_layer(
            scene,
            true,
            BlendMode::default(),
            opacity,
            transform,
            &bounds,
            filter,
            group_backdrop,
        );
        // pyro: the backdrop filter's draw, first in the group: under the element's content, and
        // inside its opacity and filter (see `render_element`).
        if let (Some(backdrop), Some((clip, space))) = (backdrop_filter, &backdrop_clip) {
            scene.push_layer(Fill::NonZero, BlendMode::default(), 1.0, transform * *space, clip, None, Some(backdrop));
            scene.pop_layer();
        }
        scene.append_scene(group, Affine::IDENTITY);
        self.layer_manager.maybe_pop_layer(scene, pushed);
    }

    /// pyro: [`Self::paint_effect_group`] for a `filter` list with an SVG filter graph in it
    /// (`filters::filter_steps`): a layer for each step around the group, the first innermost, a
    /// graph's in the element's user space (CSS px from its border box's corner) clipped to its
    /// filter region, and the opacity (and a backdrop filter) around them all. A graph paints its
    /// region whatever the group holds, nothing included.
    #[cfg(feature = "svg")]
    fn paint_filter_steps(
        &self,
        scene: &mut impl PaintScene,
        group: Scene,
        layer: EffectLayer,
        steps: Vec<crate::filters::FilterStep>,
    ) {
        use crate::filters::FilterStep;
        let EffectLayer { transform, visible, border_box, opacity, backdrop_filter, backdrop_clip, .. } = layer;
        if transform.determinant().abs() < 1e-12 {
            return;
        }
        let inverse = transform.inverse();
        let user = Affine::translate(border_box.origin().to_vec2()) * Affine::scale(self.scale);
        // What the layers can paint, in the element's space: the ink, grown by each chain, and
        // each graph's filter region in its place.
        let mut extent = crate::bounds::scene_bounds(&group.commands).map(|ink| inverse.transform_rect_bbox(ink));
        for step in &steps {
            extent = match step {
                FilterStep::Chain(filter) => extent.map(|b| {
                    let reach = filter.expansion_rect();
                    Rect::new(b.x0 + reach.x0, b.y0 + reach.y0, b.x1 + reach.x1, b.y1 + reach.y1)
                }),
                FilterStep::Graph(graph) => Some(user.transform_rect_bbox(graph.region)),
            };
        }
        if backdrop_filter.is_some() {
            extent = Some(extent.map_or(border_box, |extent| extent.union(border_box)));
        }
        let Some(extent) = extent else {
            return;
        };
        let visible = visible + Vec2::new(self.initial_x, self.initial_y);
        let bounds = extent.intersect(inverse.transform_rect_bbox(visible)).expand();
        if !(bounds.width() > 0.0 && bounds.height() > 0.0) {
            return;
        }
        let pushed = self.layer_manager.maybe_push_layer(
            scene,
            opacity < 1.0 || backdrop_filter.is_some(),
            BlendMode::default(),
            opacity,
            transform,
            &bounds,
            None,
            backdrop_filter.clone(),
        );
        if let (Some(backdrop), Some((clip, space))) = (backdrop_filter, &backdrop_clip) {
            scene.push_layer(Fill::NonZero, BlendMode::default(), 1.0, transform * *space, clip, None, Some(backdrop));
            scene.pop_layer();
        }
        for step in steps.iter().rev() {
            match step {
                FilterStep::Chain(filter) => scene.push_layer(
                    Fill::NonZero,
                    BlendMode::default(),
                    1.0,
                    transform,
                    &bounds,
                    Some(filter.clone()),
                    None,
                ),
                FilterStep::Graph(graph) => graph.push_layer(scene, transform * user),
            }
        }
        scene.append_scene(group, Affine::IDENTITY);
        for _ in &steps {
            scene.pop_layer();
        }
        self.layer_manager.maybe_pop_layer(scene, pushed);
    }

    /// pyro: whether a box in `node`'s subtree (`node` left out) has a `mix-blend-mode` other than
    /// `normal`.
    fn has_blending_descendant(&self, node: &Node) -> bool {
        let mut stack: Vec<NodeId> = node.children.to_vec();
        while let Some(id) = stack.pop() {
            let Some(child) = self.dom.get_node(id) else { continue };
            if child.primary_styles().is_some_and(|style| {
                style.get_effects().mix_blend_mode != style::computed_values::mix_blend_mode::T::Normal
            }) {
                return true;
            }
            stack.extend(child.children.iter().copied());
        }
        false
    }

    fn render_node(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let node = &self.dom.as_ref().tree()[node_id];
        // pyro: a table cell in collapsed columns only is not rendered (CSS 2.1 § 17.5.5).
        if let Some(table) = node.layout_parent.get().and_then(|parent| self.dom.as_ref().get_node(parent))
            && let Some(element) = table.element_data()
            && let blitz_dom::node::SpecialElementData::TableRoot(context) = &element.special_data
            && context.cell_in_collapsed_columns(node_id)
        {
            return;
        }
        // pyro: a positioned inline box's entry in the paint tree (`NodeFlags::INLINE_PASS_ENTRY`):
        // its inline root paints its content, after its own in-flow content.
        if node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY) {
            self.draw_inline_pass_entry(scene, node_id);
            return;
        }

        match &node.data {
            // pyro: once per tree level; a tree of any depth paints on a stack grown on
            // demand (blitz-dom `util::ensure_stack`).
            NodeData::Element(_) | NodeData::AnonymousBlock(_) => blitz_dom::util::ensure_stack(|| {
                self.render_element(scene, node_id, parent_style_transform, clip_rect)
            }),
            NodeData::Text(TextNodeData { .. }) => {
                // Text nodes should never be rendered directly
                // (they should always be rendered as part of an inline layout)
                // unreachable!()
            }
            NodeData::Document(_) => {}
            // NodeData::Doctype => {}
            NodeData::Comment { .. } => {} // NodeData::ProcessingInstruction { .. } => {}
        }
    }

    /// pyro: paints the content of the positioned inline box `span_id` that its inline root left for
    /// its entry in the paint tree (`inline_pass_entries`): its fragments and text, as the root paints
    /// them. Nothing when the root painted it itself (or was not painted).
    fn draw_inline_pass_entry(&self, scene: &mut impl PaintScene, span_id: NodeId) {
        let Some(entries) = self.inline_pass_entries.borrow_mut().remove(&span_id) else { return };
        for (root_id, transform, pos, column) in entries {
            // (Each column's lines in its column only.)
            let pushed = column.map(|(start, end)| {
                const UNBOUNDED: f64 = 1e9;
                let range = Rect::new(
                    -UNBOUNDED,
                    (start as f64 * self.scale).max(-UNBOUNDED),
                    UNBOUNDED,
                    (end as f64 * self.scale).min(UNBOUNDED),
                );
                self.layer_manager.maybe_push_layer(scene, true, peniko::BlendMode::default(), 1.0, transform, &range, None, None)
            });
            self.draw_inline_pass_entry_in(scene, span_id, root_id, transform, pos);
            if let Some(pushed) = pushed {
                self.layer_manager.maybe_pop_layer(scene, pushed);
            }
        }
    }

    /// pyro: one entry of `draw_inline_pass_entry`: `span_id`'s content laid out by the inline root
    /// `root_id` at `transform` and content position `pos`.
    fn draw_inline_pass_entry_in(&self, scene: &mut impl PaintScene, span_id: NodeId, root_id: NodeId, transform: Affine, pos: Point) {
        let Some(root) = self.dom.get_node(root_id) else { return };
        let cx = self.element_cx(root, root.final_layout().clone(), transform, None);
        // pyro: an inline stacking context keeping out-of-flow boxes (blitz-dom
        // `restore_out_of_flow_stacking_owners`) paints them after its fragments, in a group that is
        // their backdrop root (WPT filter-effects `backdrop-filter-backdrop-root-clip-path-2`).
        let owned = clip_path::owned_hoisted(self.dom.as_ref(), span_id);
        if !owned.is_empty() {
            let viewport = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.0,
                Affine::IDENTITY,
                &viewport,
                Some(crate::layers::backdrop_root_marker()),
                None,
            );
        }
        cx.draw_inline_pass(scene, pos, span_id);
        if !owned.is_empty() {
            let everything = Rect::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY);
            cx.draw_hoisted_children(scene, owned.iter(), transform, everything);
            scene.pop_layer();
        }
        // Then the atomic inline boxes in it (badges), which its root skipped (`draw_paint_child`), moved
        // by the sticky offsets of the inline boxes around them (layout placed them with the relative ones).
        let children: Vec<NodeId> = root.paint_children.borrow().iter().flatten().copied().collect();
        for child_id in children {
            let Some(child) = self.dom.get_node(child_id) else { continue };
            if self.inline_entry_owner(child, root_id) != Some(span_id) {
                continue;
            }
            let mut sticky = (0.0f64, 0.0f64);
            let mut current = child.parent;
            while let Some(id) = current.filter(|&id| id != root_id) {
                let Some(node) = self.dom.get_node(id) else { break };
                let Some(style) = node.primary_styles() else { break };
                if !style.clone_display().is_inline_flow() {
                    break;
                }
                if style.clone_position() == style::computed_values::position::T::Sticky {
                    let own = node.sticky_offset();
                    sticky = (sticky.0 + own.x, sticky.1 + own.y);
                }
                current = node.parent;
            }
            let child_transform = transform.pre_translate(Vec2::new(sticky.0 * self.scale, sticky.1 * self.scale));
            let everything = Rect::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY);
            self.render_node(scene, child_id, child_transform, everything);
        }
    }

    /// pyro: the positioned inline box (`NodeFlags::INLINE_PASS_ENTRY`) whose content the atomic inline
    /// box `child`, laid out by the inline root `root_id`, is part of: the innermost relative or sticky
    /// inline box around it. `None` for any other box (a float, a positioned or block-level box).
    fn inline_entry_owner(&self, child: &Node, root_id: NodeId) -> Option<NodeId> {
        let style = child.primary_styles()?;
        if style.clone_position() != style::computed_values::position::T::Static
            || style.clone_float() != style::computed_values::float::T::None
            || !style.clone_display().is_inline_flow() && style.clone_display().outside() != style::values::specified::box_::DisplayOutside::Inline
        {
            return None;
        }
        drop(style);
        let parent = child.parent?;
        let span = *self.dom.inline_span_positioned_chain(parent, root_id).first()?;
        self.dom
            .get_node(span)
            .is_some_and(|node| node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY))
            .then_some(span)
    }

    fn element_cx(
        &'dom self,
        node: &'dom Node,
        layout: Layout,
        transform: Affine,
        custom_widget_scene: Option<&'a Scene>,
    ) -> ElementCx<'dom, 'a> {
        let style = node
            .stylo_element_data()
            .primary_styles()
            .as_ref()
            .map(|styles| (*styles).clone())
            .unwrap_or_else(|| {
                ComputedValues::initial_values_with_font_override(Font::initial_values()).to_arc()
            });

        let scale = self.scale;

        // todo: maybe cache this so we don't need to constantly be figuring it out
        // It is quite a bit of math to calculate during render/traverse
        // Also! we can cache the bezpaths themselves, saving us a bunch of work
        let element = node.element_data().unwrap();

        // pyro: a table with cells in the collapsing border model has the borders its grid was laid
        // out with (half of its outer collapsed borders) and no padding: its background is positioned
        // in them.
        let mut layout = layout;
        if let blitz_dom::node::SpecialElementData::TableRoot(table) = &element.special_data
            && table.border_collapse == style::computed_values::border_collapse::T::Collapse
            && !table.cells.is_empty()
        {
            layout.border = table.style.border.map(|side| side.into_raw().value());
            layout.padding = taffy::Rect::ZERO;
        }

        // pyro: a table with captions is its wrapper box; it paints its own box (background,
        // borders, shadows, outline) around its grid alone (`TableContext::grid_box`).
        let frame = match &element.special_data {
            blitz_dom::node::SpecialElementData::TableRoot(table) => match *table.grid_box.borrow() {
                Some((top, height)) => {
                    let mut grid = layout;
                    grid.size.height = height;
                    let frame = create_css_rect(&style, &grid, scale);
                    let dy = Vec2::new(0.0, top as f64 * scale);
                    CssBox::new(
                        frame.border_box + dy,
                        frame.border_width,
                        frame.padding_width,
                        frame.outline_width,
                        frame.border_radii,
                    )
                }
                None => create_css_rect(&style, &layout, scale),
            },
            _ => create_css_rect(&style, &layout, scale),
        };

        ElementCx {
            context: self,
            frame,
            scale,
            style,
            node,
            element,
            transform,
            #[cfg(feature = "svg")]
            svg: match element.image_data() {
                Some(blitz_dom::node::ImageData::Svg(svg)) => Some(svg),
                _ => None,
            },
            text_input: element.text_input_data(),
            list_item: element.list_item_data.as_deref(),
            devtools: self.dom.devtools(),
            custom_widget_scene,
        }
    }
}

/// pyro: how a column shares a box of its multi-column container (`ElementCx::column_share`).
enum ColumnShare {
    /// It starts in the column: cut at the column's end only.
    Inside,
    /// Another column holds it.
    Outside,
    /// A break goes through it: each column shows its slice.
    Cut,
}

/// pyro: the element whose background paints the canvas (CSS Backgrounds 3 §2.11.2), as Blink's
/// `LayoutBox::BackgroundTransfersToView` decides it: the root element, unless it is an HTML
/// `html` element without a background of its own, when it is its first `body` child, if that
/// is rendered (not `display: none` or `contents`) and neither element has containment. `None`
/// when the root element is not rendered.
fn canvas_background_source(dom: &BaseDocument) -> Option<NodeId> {
    let root = dom.try_root_element()?;
    let root_style = root.primary_styles()?;
    if root_style.get_box().display.is_none() {
        return None;
    }
    let has_background = |style: &ComputedValues| {
        let background = style.get_background();
        background
            .background_image
            .0
            .iter()
            .any(|image| !matches!(image, style::values::computed::Image::None))
            || background
                .background_color
                .resolve_to_absolute(&style.clone_color())
                .alpha
                > 0.0
    };
    let contained = |style: &ComputedValues| !style.get_box().slow_clone_contain().is_empty();
    // pyro: a root its `content: url()` replaces renders no `body` (WPT css-content
    // `element-replacement-root-canvas-bg-from-body`).
    let replaced = root.element_data().is_some_and(|element| element.content_image.is_some());
    if has_background(&root_style)
        || contained(&root_style)
        || replaced
        || !root.data.is_element_with_tag_name(&local_name!("html"))
    {
        return Some(root.id);
    }
    let body = root
        .children
        .iter()
        .filter_map(|&id| dom.get_node(id))
        .find(|node| node.data.is_element_with_tag_name(&local_name!("body")));
    let Some(body) = body else {
        return Some(root.id);
    };
    let Some(body_style) = body.primary_styles() else {
        return Some(root.id);
    };
    let display = body_style.get_box().display;
    if display.is_none() || display.is_contents() || contained(&body_style) {
        return Some(root.id);
    }
    Some(body.id)
}

fn to_image_quality(image_rendering: ImageRendering) -> peniko::ImageQuality {
    match image_rendering {
        ImageRendering::Auto => peniko::ImageQuality::Medium,
        ImageRendering::CrispEdges => peniko::ImageQuality::Low,
        ImageRendering::Pixelated => peniko::ImageQuality::Low,
    }
}

/// The transform from an SVG's rendered tree to a viewport of the given size.
#[cfg(feature = "svg")]
fn svg_viewport_affine(svg: &blitz_dom::node::SvgImageData, width: f32, height: f32) -> Affine {
    let t = svg.viewport_transform(width, height);
    Affine::new([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f64::from))
}

/// Ensure that the `resized_image` field has a correctly sized image
fn to_peniko_image(image: &RasterImageData, quality: peniko::ImageQuality) -> peniko::ImageBrush {
    peniko::ImageBrush {
        image: ImageData {
            data: image.data.clone(),
            format: peniko::ImageFormat::Rgba8,
            width: image.width,
            height: image.height,
            alpha_type: peniko::ImageAlphaType::Alpha,
        },
        sampler: ImageSampler {
            x_extend: peniko::Extend::Repeat,
            y_extend: peniko::Extend::Repeat,
            quality,
            alpha: 1.0,
        },
    }
}

/// pyro: an `<img>`, `<video>` or `<canvas>`: the elements the UA sheet gives `overflow: clip`.
/// pyro: whether `child_id`, an out-of-flow box painted in the stacking context of `node_id`,
/// has a containing block outside that element: its overflow clip does not clip it (it is not in
/// its containing-block chain).
fn escapes_clip_of(tree: &blitz_dom::NodeTree, node_id: NodeId, child_id: NodeId) -> bool {
    let Some(child) = tree.get(child_id) else { return false };
    if !child.is_out_of_flow() {
        return false;
    }
    let mut cb = child.containing_block();
    while let Some(id) = cb {
        if id == node_id {
            return false;
        }
        cb = tree.get(id).and_then(|node| node.containing_block());
    }
    true
}

/// pyro: whether `node` (a clipping box) paints any z-ordered box (negative `z-index`,
/// positioned level or above) whose containing block lies outside it ([`escapes_clip_of`]).
fn has_clip_escapes(tree: &blitz_dom::NodeTree, node: &Node) -> bool {
    has_clip_escapes_of(tree, node, node.id)
}

/// pyro: [`has_clip_escapes`] for the clip of `owner` (`node` itself, or the box that handed it its
/// clip: `BlitzDomPainter::handed_clips`).
fn has_clip_escapes_of(tree: &blitz_dom::NodeTree, node: &Node, owner: NodeId) -> bool {
    // pyro: a clipping box that is no stacking context too: a box escaping its clip paints among
    // its positioned children (blitz-dom `restore_out_of_flow_stacking_owners`, `clip_owner_of`).
    node.stacking_context.as_ref().is_some_and(|sc| {
        sc.pos_z_hoisted_children()
            .chain(sc.neg_z_hoisted_children())
            .any(|child| escapes_clip_of(tree, owner, child.node_id))
    }) || node
            .paint_children
            .borrow()
            .as_ref()
            .is_some_and(|children| children.iter().any(|&id| escapes_clip_of(tree, owner, id)))
}

/// pyro: the culling rect of a child painted in its parent's clip (`clipped`: `inner`, the
/// parent's clip applied) or escaping it (`outer`, what the parent's ancestors leave visible).
fn child_clip_rect_for(clipped: bool, outer: Rect, inner: Rect) -> Rect {
    if clipped { inner } else { outer }
}

fn is_ua_clipped_replaced(element: &ElementData) -> bool {
    // pyro: not a broken image showing its alt text: a box of text, clipped as any box.
    if element.image_broken {
        return false;
    }
    let name = &element.name.local;
    *name == local_name!("img") || *name == local_name!("video") || *name == local_name!("canvas")
}

/// pyro: whether the replaced content of `element` (an `<img>`, `<video>` or `<canvas>`, which
/// the UA sheet gives `overflow: clip`) is not cut at its box: its `overflow` is `visible` on
/// both axes.
fn replaced_overflow_is_visible(element: &ElementData, style: &ComputedValues) -> bool {
    let box_style = style.get_box();
    is_ua_clipped_replaced(element)
        && matches!(box_style.overflow_x, Overflow::Visible)
        && matches!(box_style.overflow_y, Overflow::Visible)
}

/// pyro: when replaced content is cut at its content box (`ElementCx::with_replaced_content_clip`).
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "svg"), allow(dead_code))]
enum ReplacedClip {
    /// Not at all: an inline `<svg>` with `overflow: visible`.
    Never,
    /// Always: an SVG's content reaches past its viewport, or its content edge is rounded.
    Always,
    /// Where its object-fit rect overflows the box, or its content edge is rounded inside the
    /// padding box (a rounded padding box clips the rest).
    Fit,
}

/// A context of loaded and hot data to draw the element from
struct ElementCx<'dom, 'a> {
    context: &'dom BlitzDomPainter<'dom, 'a>,
    frame: CssBox,
    style: style::servo_arc::Arc<ComputedValues>,
    scale: f64,
    node: &'dom Node,
    element: &'dom ElementData,
    transform: Affine,
    #[cfg(feature = "svg")]
    svg: Option<&'dom blitz_dom::node::SvgImageData>,
    text_input: Option<&'dom TextInputData>,
    list_item: Option<&'dom ListItemLayout>,
    devtools: &'dom DevtoolSettings,
    #[cfg_attr(not(feature = "custom-widget"), expect(unused))]
    custom_widget_scene: Option<&'a Scene>,
}

/// Strokes an inline root's text, each line cut where `text-overflow: ellipsis` cuts it (with the
/// ellipsis drawn after what it keeps). pyro: shared by `draw_inline_layout` and the
/// `background-clip: text` mask (`draw_text_mask`), which must cut its glyphs alike. `context` has
/// the text's shift and line tops (`tops`) set.
#[allow(clippy::too_many_arguments)]
fn stroke_inline_text(
    scene: &mut impl PaintScene,
    dom: &BaseDocument,
    node: &Node,
    text_layout: &blitz_dom::node::TextLayout,
    transform: Affine,
    options: TextPaintOptions,
    context: &mut DrawTextContext,
    tops: Option<crate::text::LineTops>,
    scale: f64,
) {
    let clamp = text_layout.clamp.as_ref();
    if text_layout.ellipsis.is_none() && clamp.is_none() {
        crate::text::stroke_text(
            scene,
            text_layout.layout.lines(),
            dom,
            transform,
            options,
            node.id,
            Some(&text_layout.text),
            context,
            None,
            Some(&text_layout.layout),
        );
        return;
    }
    for (index, line) in text_layout.layout.lines().enumerate() {
        // pyro: the lines after a line clamp's are not painted.
        if clamp.is_some_and(|clamp| index >= clamp.lines as usize) {
            break;
        }
        let truncation = line_truncation(node, text_layout, &line, index, scale as f32);
        context.cut = truncation.as_ref().map(|truncation| truncation.cut);
        // pyro: a clamp line laid out again (`LineClamp::relaid`) has the root text it keeps on
        // the baseline of a line of the ellipsis, at its top (WPT css-overflow `line-clamp-auto-041`).
        let relaid_shift = clamp
            .filter(|clamp| clamp.relaid && index + 1 == clamp.lines as usize)
            .and_then(|clamp| clamp.ellipsis.lines().next().map(|l| l.metrics().baseline))
            .map_or(0.0, |baseline| (line.metrics().block_min_coord + baseline - line.metrics().baseline) as f64);
        crate::text::stroke_text(
            scene,
            std::iter::once(line.clone()),
            dom,
            transform * Affine::translate((0.0, relaid_shift)),
            options,
            node.id,
            Some(&text_layout.text),
            context,
            None,
            Some(&text_layout.layout),
        );
        context.cut = None;
        // The ellipsis on the line's baseline, next to the last atom kept.
        if let Some(LineTruncation { x, ellipsis, text, .. }) = truncation {
            // pyro: in every pass of `draw_inline_effects` (its runs are not where
            // the layout's are, which `RunFilter` looks at).
            let filter = context.run_filter.take();
            let baseline = ellipsis.lines().next().map_or(0.0, |l| l.metrics().baseline);
            let shift = tops.map_or(0.0, |tops| tops.shift(line.metrics()));
            // pyro: a clamp line laid out again holds the ellipsis alone at its top
            // (`LineClamp::relaid`).
            let relaid = clamp.is_some_and(|clamp| clamp.relaid && index + 1 == clamp.lines as usize);
            let y = if relaid { line.metrics().block_min_coord } else { line.metrics().baseline - baseline };
            let offset = (x as f64, y as f64 + shift);
            context.line_tops = None;
            // pyro: on the first line, in the `::first-line` colour (the line's root style).
            let mut options = options;
            // (A line clamp's own ellipsis is shaped in the first line's style already.)
            let text_overflow = text_layout.ellipsis.as_deref().is_some_and(|own| std::ptr::eq(own, ellipsis));
            if index == 0 && text_overflow && options.brush_override.is_none() {
                if let Some(first_line) = text_layout.first_line {
                    options.brush_override = dom
                        .get_node(first_line)
                        .and_then(|node| node.primary_styles().map(|style| style.clone_color().as_srgb_color()));
                }
            }
            crate::text::stroke_text(
                scene,
                ellipsis.lines(),
                dom,
                transform * Affine::translate(offset),
                options,
                node.id,
                Some(text),
                context,
                None,
                None,
            );
            context.line_tops = tops;
            context.run_filter = filter;
        }
    }
}

/// pyro: whether `node` is an in-flow atomic inline of an inline root whose line ends with an
/// ellipsis before the box's end edge (its start edge in right-to-left lines).
fn hidden_by_ellipsis(dom: &BaseDocument, node: &Node) -> bool {
    let tree = dom.tree();
    let mut root = node.layout_parent.get();
    let mut text_layout = None;
    for _ in 0..8 {
        let Some(id) = root else { return false };
        let Some(candidate) = tree.get(id) else { return false };
        if let Some(layout) = candidate.element_data().and_then(|el| el.inline_layout_data.as_deref()) {
            text_layout = Some((candidate, layout));
            break;
        }
        root = candidate.layout_parent.get();
    }
    let Some((root, text_layout)) = text_layout else { return false };
    if text_layout.ellipsis.is_none() && text_layout.clamp.is_none() {
        return false;
    }
    let id = node.id.as_u64();
    let scale = dom.viewport().scale();
    for (index, line) in text_layout.layout.lines().enumerate() {
        let ibox = line.items().find_map(|item| match item {
            parley::PositionedLayoutItem::InlineBox(ibox) if ibox.id == id && ibox.kind == parley::InlineBoxKind::InFlow => Some(ibox),
            _ => None,
        });
        let Some(ibox) = ibox else { continue };
        let Some(truncation) = line_truncation(root, text_layout, &line, index, scale) else { return false };
        let cut = truncation.cut;
        // (A cut at a text offset keeps the start of the line in its direction: its edge is
        // where that ends, `truncate_line_at_soft_wrap`.)
        let keeps_left = match cut.split {
            Some(_) => !line.is_rtl(),
            None => cut.keep_left,
        };
        return if keeps_left { ibox.x + ibox.width > cut.edge + scale / 64.0 } else { ibox.x < cut.edge - scale / 64.0 };
    }
    false
}

/// pyro: what ends a line short: the atoms cut from it, and the ellipsis drawn at `x` (layout
/// coordinates) after those kept.
struct LineTruncation<'a> {
    cut: crate::text::TextCut,
    x: f32,
    ellipsis: &'a parley::Layout<blitz_dom::node::TextBrush>,
    text: &'a str,
}

/// pyro: how line `index` of the inline root `node` ends short, if it does: the line a line
/// clamp ends at (`TextLayout::clamp`) always ends with "…"; any other line that overflows the
/// content box ends with the `text-overflow` ellipsis (`TextLayout::ellipsis`).
fn line_truncation<'a>(
    node: &Node,
    text_layout: &'a blitz_dom::node::TextLayout,
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    index: usize,
    scale: f32,
) -> Option<LineTruncation<'a>> {
    // pyro: none while the text is being edited: its editing host (the nearest element with
    // `contenteditable` other than `false`) has the focus, as Chromium shows the whole text
    // (CSS Overflow 3 § ellipsis interaction; WPT css-overflow
    // `text-overflow-ellipsis-editable-div-with-caret`).
    if editing_host_focused(node) {
        return None;
    }
    // The lines were broken in the unrounded box: a line as wide as the box (a
    // box sized to its text) must not count as overflowing it by a rounding.
    let width = node.unrounded_layout().content_box_width() * scale;
    // pyro: the end edge is the scrollport's, where the box is scrolled to (CSS Overflow 4
    // `text-overflow`: the ellipsis shows at the visible edge; WPT css-overflow `text-overflow-021`).
    // (Scroll offsets are kept in whole px: one up to half a px short of the fractional maximum
    // still shows the content's end.)
    let scroll_x = (node.scroll_offset().x as f32).max(0.0);
    let scrolled = if scroll_x > 0.0 { (scroll_x + 0.5) * scale } else { 0.0 };
    line_truncation_scrolled(text_layout, line, index, scale, width, wraps(node), scrolled)
}

/// pyro: whether the lines of the inline root `node` have soft wrap opportunities: none where
/// its `text-wrap-mode` is `nowrap` (`white-space: pre`: WPT css-overflow
/// `line-clamp/block-ellipsis-013`) but at a `<wbr>`.
fn wraps(node: &Node) -> bool {
    node.primary_styles().is_none_or(|style| {
        style.get_inherited_text().text_wrap_mode != style::computed_values::text_wrap_mode::T::Nowrap
    })
}

/// pyro: whether `node` is in an editing host (`contenteditable` other than `false`, the
/// nearest such element up its ancestors) that has the focus.
fn editing_host_focused(node: &Node) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if let Some(value) = n.attr(local_name!("contenteditable")) {
            if value.eq_ignore_ascii_case("false") {
                return false;
            }
            return n.is_focussed();
        }
        current = n.parent.map(|id| &n.tree()[id]);
    }
    false
}

/// pyro: [`line_truncation`] at an inline size `width` (device px).
fn line_truncation_at<'a>(
    text_layout: &'a blitz_dom::node::TextLayout,
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    index: usize,
    scale: f32,
    width: f32,
    wraps: bool,
) -> Option<LineTruncation<'a>> {
    line_truncation_scrolled(text_layout, line, index, scale, width, wraps, 0.0)
}

/// pyro: [`line_truncation_at`] in a box scrolled `scrolled` device px along the line: its end
/// edge that much further, past the line's own end (`LineMetrics::inline_max_coord`).
fn line_truncation_scrolled<'a>(
    text_layout: &'a blitz_dom::node::TextLayout,
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    index: usize,
    scale: f32,
    width: f32,
    wraps: bool,
    scrolled: f32,
) -> Option<LineTruncation<'a>> {
    // pyro: the line's paragraph's direction (`unicode-bidi: plaintext`: WPT css-overflow
    // `line-clamp-with-text-overflow-string-009`).
    let rtl = line.is_rtl();
    let clamp = text_layout.clamp.as_ref().filter(|clamp| index + 1 == clamp.lines as usize);
    // pyro: `block-ellipsis: no-ellipsis` leaves the clamp line whole: nothing makes room on it
    // (Chromium; WPT css-overflow `line-clamp/block-ellipsis-023`).
    if clamp.is_some_and(|clamp| clamp.ellipsis.lines().next().is_none()) {
        return None;
    }
    let (ellipsis, text, forced) = match clamp {
        Some(clamp) => (&*clamp.ellipsis, "\u{2026}", true),
        None => (text_layout.ellipsis.as_deref()?, text_layout.ellipsis_text.as_str(), false),
    };
    // pyro: a standard line clamp's ellipsis after the last soft wrap opportunity that leaves
    // room for it (CSS Overflow 4 `block-ellipsis`; WPT css-overflow `block-ellipsis-001`).
    if let Some(clamp) = clamp.filter(|clamp| clamp.at_soft_wraps) {
        let hyphenated = &*clamp.hyphenated_ellipsis;
        if let Some((cut, x, at_hyphen)) =
            truncate_line_at_soft_wrap(line, ellipsis, hyphenated, width, scale, rtl, wraps, &text_layout.text, scrolled)
        {
            // (At a soft hyphen, a hyphen shows before the ellipsis.)
            return Some(match at_hyphen {
                true => LineTruncation { cut, x, ellipsis: hyphenated, text: "-\u{2026}" },
                false => LineTruncation { cut, x, ellipsis, text },
            });
        }
    }
    let (cut, x) = truncate_line(line, ellipsis, width, scale, rtl, forced, scrolled)?;
    Some(LineTruncation { cut, x, ellipsis, text })
}

/// pyro: a line that ends with a standard line clamp's ellipsis: the line keeps what precedes
/// (in its paragraph's direction) its last soft wrap opportunity that leaves room for the
/// ellipsis (less the spaces before it), and nothing when none does: a word too long to fit
/// beside the ellipsis is displaced by it. Atomic inlines are content without opportunities of
/// their own. `None` when the whole line fits beside the ellipsis (the cut of [`truncate_line`]
/// keeps it all). The cut splits the text there; its edge is where the kept content ends (an
/// atomic inline past it is hidden: `hidden_by_ellipsis`).
fn truncate_line_at_soft_wrap(
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    ellipsis: &parley::Layout<blitz_dom::node::TextBrush>,
    hyphenated: &parley::Layout<blitz_dom::node::TextBrush>,
    width: f32,
    scale: f32,
    rtl: bool,
    wraps: bool,
    text: &str,
    scrolled: f32,
) -> Option<(crate::text::TextCut, f32, bool)> {
    let epsilon = scale / 64.0;
    let metrics = line.metrics();
    // (An empty one, `block-ellipsis: no-ellipsis`, has no lines and no width.)
    let ellipsis_width = Some(ellipsis.full_width()).filter(|w| w.is_finite()).map_or(0.0, |w| w.max(0.0));
    let hyphenated_width = Some(hyphenated.full_width()).filter(|w| w.is_finite()).map_or(ellipsis_width, |w| w.max(0.0));
    let width = if metrics.inline_max_coord > 0.0 { width.min(metrics.inline_max_coord) } else { width } + scrolled;
    // The line's atoms: (logical key, x0, x1, whether it is white space, the byte offset a soft
    // wrap opportunity before it breaks at). A cluster's key is its text offset; an atomic
    // inline's, that of the cluster before it in the line's direction (it has no text).
    let mut atoms: Vec<(f64, f32, f32, bool, Option<usize>)> = Vec::new();
    for item in line.items() {
        match item {
            parley::PositionedLayoutItem::GlyphRun(run) => {
                let mut x = run.offset();
                for cluster in run.run().visual_clusters() {
                    let advance = cluster.advance();
                    let space = cluster.is_space_or_nbsp() || cluster.is_hard_line_break() || advance <= 0.0;
                    // (Without soft wrapping, a `<wbr>` (U+200B) still is one, as Chromium has it;
                    // the wrapping of the cluster before the opportunity, the one the line would
                    // break after, decides: WPT css-overflow `line-clamp/block-ellipsis-027`.)
                    let start = cluster.text_range().start;
                    let wraps_here = wraps && cluster.previous_logical().is_none_or(|before| before.wraps());
                    let opportunity = (cluster.is_soft_wrap_opportunity()
                        && (wraps_here || text.get(..start).is_some_and(|before| before.ends_with('\u{200B}'))))
                    .then_some(start);
                    atoms.push((cluster.text_range().start as f64, x, x + advance, space, opportunity));
                    x += advance;
                }
            }
            parley::PositionedLayoutItem::InlineBox(ibox) => {
                if ibox.kind == parley::InlineBoxKind::InFlow {
                    atoms.push((f64::NAN, ibox.x, ibox.x + ibox.width, false, None));
                }
            }
        }
    }
    atoms.sort_by(|a, b| a.1.total_cmp(&b.1));
    if rtl {
        atoms.reverse();
    }
    let mut previous = line.text_range().start as f64 - 1.0;
    for atom in atoms.iter_mut() {
        if atom.0.is_nan() {
            atom.0 = previous + 0.5;
        } else {
            previous = atom.0;
        }
    }
    atoms.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Where the content walked so far ends (its far edge in the line's direction), and whether
    // the ellipsis fits beyond it.
    // (The line's items start at its `inline_min_coord`, past a float before it: WPT css-overflow
    // `line-clamp/block-ellipsis-041`.)
    let line_start = if rtl { metrics.inline_min_coord + metrics.offset + metrics.advance } else { metrics.inline_min_coord + metrics.offset };
    let mut far = line_start;
    let fits_with = |far: f32, w: f32| if rtl { far - w >= -epsilon } else { far + w <= width + epsilon };
    let fits = |far: f32| fits_with(far, ellipsis_width);
    // (A break at a soft hyphen shows a hyphen too.)
    let after_hyphen = |offset: usize| text.get(..offset).is_some_and(|before| before.ends_with('\u{AD}'));
    // The last break that fits: its byte offset, where the content before it ends, and whether
    // it is at a soft hyphen.
    let mut best: Option<(usize, f32, bool)> = None;
    for (index, &(_, x0, x1, space, opportunity)) in atoms.iter().enumerate() {
        if index > 0
            && let Some(offset) = opportunity
        {
            let hyphen = after_hyphen(offset);
            if fits_with(far, if hyphen { hyphenated_width } else { ellipsis_width }) {
                best = Some((offset, far, hyphen));
            }
        }
        if !space {
            far = if rtl { far.min(x0) } else { far.max(x1) };
        }
    }
    // (The whole line fits beside it once its trailing spaces and break hang.)
    if fits(far) {
        return None;
    }
    let (split, end, hyphen) = best.unwrap_or((line.text_range().start, line_start, false));
    let x = if rtl { end - if hyphen { hyphenated_width } else { ellipsis_width } } else { end };
    Some((crate::text::TextCut { edge: end, keep_left: true, split: Some(split) }, x, hyphen))
}

/// pyro: `text-overflow: ellipsis` on one line, as Chromium applies it: a line whose content
/// (without its hanging white space) overflows the content box `width` wide (device px) by more
/// than a layout unit (1/64 CSS px at `scale`) loses the atoms that do not fit beside the
/// ellipsis at its end edge, and the ellipsis follows the last atom kept (at the line's start
/// when none is). Returns the cut and the ellipsis's x (layout coordinates), `None` for a line
/// that fits.
fn truncate_line(
    line: &parley::Line<'_, blitz_dom::node::TextBrush>,
    ellipsis: &parley::Layout<blitz_dom::node::TextBrush>,
    width: f32,
    scale: f32,
    rtl: bool,
    forced: bool,
    scrolled: f32,
) -> Option<(crate::text::TextCut, f32)> {
    let epsilon = scale / 64.0;
    let metrics = line.metrics();
    // (An empty one, `block-ellipsis: no-ellipsis`, has no lines and no width.)
    let ellipsis_width = Some(ellipsis.full_width()).filter(|w| w.is_finite()).map_or(0.0, |w| w.max(0.0));
    // pyro: the line's own edges where floats shorten it (Blink truncates at the line box's end).
    let width = if metrics.inline_max_coord > 0.0 { width.min(metrics.inline_max_coord) } else { width } + scrolled;
    let start = metrics.inline_min_coord.clamp(0.0, width);
    // A line that ends with the ellipsis whether it overflows or not (`forced`: a line
    // clamp's) keeps all of its atoms when the ellipsis fits after them.
    if forced {
        // pyro: the line's items start at its `inline_min_coord`, past a float before it (WPT
        // css-overflow `line-clamp-with-floats-005`).
        let line_start = metrics.inline_min_coord + metrics.offset;
        let (start, end) = (line_start, line_start + metrics.advance);
        if !rtl && end - metrics.hanging_advance + ellipsis_width <= width + epsilon {
            let cut = crate::text::TextCut { edge: f32::INFINITY, keep_left: true, split: None };
            return Some((cut, end - metrics.hanging_advance));
        }
        if rtl && start + metrics.hanging_advance - ellipsis_width >= -epsilon {
            let cut = crate::text::TextCut { edge: f32::NEG_INFINITY, keep_left: false, split: None };
            return Some((cut, start + metrics.hanging_advance - ellipsis_width));
        }
    }
    let runs = || {
        line.items().filter_map(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => Some(run),
            parley::PositionedLayoutItem::InlineBox(_) => None,
        })
    };
    if !rtl {
        if !forced && metrics.offset + metrics.advance - metrics.hanging_advance <= width + epsilon {
            return None;
        }
        let cut = crate::text::TextCut { edge: width - ellipsis_width, keep_left: true, split: None };
        let end = runs().filter_map(|run| cut.kept(&run).1).map(|(_, x1)| x1).fold(None, |a: Option<f32>, x| Some(a.map_or(x, |a| a.max(x))));
        // pyro: a line clamp's ellipsis follows the kept content less the spaces at its end
        // (Chromium; WPT css-overflow `line-clamp/webkit-line-clamp-024`).
        if forced && let Some(end) = end {
            let mut content_end: Option<f32> = None;
            for run in runs() {
                let mut x = run.offset();
                for cluster in run.run().visual_clusters() {
                    let x1 = x + cluster.advance();
                    if x1 <= cut.edge + epsilon && !cluster.is_space_or_nbsp() && cluster.advance() > 0.0 {
                        content_end = Some(content_end.map_or(x1, |e| e.max(x1)));
                    }
                    x = x1;
                }
            }
            if let Some(content_end) = content_end.filter(|&e| e < end) {
                return Some((crate::text::TextCut { edge: content_end, keep_left: true, split: None }, content_end));
            }
        }
        if let Some(end) = end {
            return Some((cut, end));
        }
        // Nothing fits beside the ellipsis: the line keeps its first atom (Blink's
        // `LineTruncator` never hides the first child whole).
        let first = runs().min_by(|a, b| a.offset().total_cmp(&b.offset()));
        let Some(edge) = first.and_then(|run| crate::text::TextCut::end_atom(&run, false)).map(|(_, x1)| x1) else {
            return Some((cut, metrics.offset));
        };
        Some((crate::text::TextCut { edge, keep_left: true, split: None }, edge))
    } else {
        if !forced && metrics.offset + metrics.hanging_advance >= start - epsilon {
            return None;
        }
        let cut = crate::text::TextCut { edge: start + ellipsis_width, keep_left: false, split: None };
        let start = runs().filter_map(|run| cut.kept(&run).1).map(|(x0, _)| x0).fold(None, |a: Option<f32>, x| Some(a.map_or(x, |a| a.min(x))));
        if let Some(start) = start {
            return Some((cut, start - ellipsis_width));
        }
        let last = runs().max_by(|a, b| (a.offset() + a.advance()).total_cmp(&(b.offset() + b.advance())));
        let Some(edge) = last.and_then(|run| crate::text::TextCut::end_atom(&run, true)).map(|(x0, _)| x0) else {
            return Some((cut, metrics.offset + metrics.advance - ellipsis_width));
        };
        Some((crate::text::TextCut { edge, keep_left: false, split: None }, edge - ellipsis_width))
    }
}

fn convert_rect(rect: &parley::BoundingBox) -> kurbo::Rect {
    peniko::kurbo::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1)
}

impl ElementCx<'_, '_> {
    /// pyro: whether this element's used colour scheme is dark: its `color-scheme` names dark and
    /// not light, or both (or neither) and the viewport prefers dark.
    pub(crate) fn dark_color_scheme(&self) -> bool {
        use style::values::specified::color::ColorSchemeFlags;
        let bits = self.style.get_inherited_ui().color_scheme.bits;
        let (dark, light) = (bits.contains(ColorSchemeFlags::DARK), bits.contains(ColorSchemeFlags::LIGHT));
        if dark != light {
            return dark;
        }
        // pyro: `normal` is the page's scheme where its `<meta name=color-scheme>` declares one
        // (WPT css/mediaqueries `prefers-color-scheme-svg-image-normal-with-meta-dark`).
        if !dark && self.context.dom.page_color_scheme_is_declared() {
            return self.context.dom.page_color_scheme_is_dark();
        }
        self.context.dom.viewport().color_scheme == blitz_traits::shell::ColorScheme::Dark
    }

    /// pyro: paints the box's classic scrollbars ([`Node::classic_scrollbars`]) that show, in
    /// their gutters over the padding box's edges, unscrolled: the track, the arrow buttons and
    /// the thumb ([`paint_classic_scrollbar`]). A gutter `scrollbar-gutter` reserves alone paints
    /// nothing (the background shows), as in Chrome.
    fn draw_classic_scrollbars(&self, scene: &mut impl PaintScene) {
        let Some(bars) = self.node.classic_scrollbars() else {
            return;
        };
        if !bars.shown.x && !bars.shown.y {
            return;
        }
        // A box that is not visible paints no scrollbars (Chrome).
        if self.style.get_inherited_box().visibility != StyloVisibility::Visible {
            return;
        }
        let colors = ClassicScrollbarColors::of(self.node);
        let layout = self.node.final_layout();
        let s = self.scale;
        let g = bars.gutters;
        let (b, size) = (layout.border, layout.size);
        // The padding box, CSS px from the border box origin.
        let (px0, py0) = (b.left as f64, b.top as f64);
        let (px1, py1) = ((size.width - b.right) as f64, (size.height - b.bottom) as f64);
        let client = (
            (self.node.client_width() as f64).max(0.0),
            (self.node.client_height() as f64).max(0.0),
        );
        // The content's extent as the DOM reports it (`scrollWidth`/`scrollHeight`): the resolved
        // scrollable overflow from the scrollport's start (non-reversed axes).
        let scroll = {
            let doc_scale = self.context.dom.as_ref().viewport().scale_f64();
            let o = self.node.scrollable_overflow().scale_from_origin(1.0 / doc_scale);
            let reversed = layout.scrollable_overflow_rect.is_reversed();
            let w = if reversed.x { px1 - g.right as f64 - o.x0 } else { o.x1 - (px0 + g.left as f64) };
            let h = if reversed.y { py1 - g.bottom as f64 - o.y0 } else { o.y1 - (py0 + g.top as f64) };
            (w.max(client.0), h.max(client.1))
        };
        // The scroll offset from the start of the scroll range (which is negative in a reversed
        // axis: `direction: rtl`, whose thumb starts at the right).
        let offset = {
            let reversed = layout.scrollable_overflow_rect.is_reversed();
            let o = *self.node.scroll_offset();
            blitz_dom::Point {
                x: if reversed.x { o.x + (scroll.0 - client.0) } else { o.x },
                y: if reversed.y { o.y + (scroll.1 - client.1) } else { o.y },
            }
        };
        // A padding box too small for its gutters shows no scrollbars (Chrome: WPT css-overflow
        // `overflow-body-propagation-016`, a 0x0 `overflow: scroll` box).
        if px1 - px0 < (g.left + g.right) as f64 || py1 - py0 < (g.top + g.bottom) as f64 {
            return;
        }
        let bottom_bar = if bars.shown.x { g.bottom as f64 } else { 0.0 };
        if bars.shown.y {
            let width = if bars.vertical_on_left { g.left } else { g.right } as f64;
            let x0 = if bars.vertical_on_left { px0 } else { px1 - width };
            let track = Rect::new(x0, py0, x0 + width, py1 - bottom_bar);
            paint_classic_scrollbar(scene, colors, self.transform, track.scale_from_origin(s), true, client.1, scroll.1, offset.y, s);
        }
        if bars.shown.x {
            let height = g.bottom as f64;
            let (left, right) = if bars.shown.y {
                if bars.vertical_on_left { (g.left as f64, 0.0) } else { (0.0, g.right as f64) }
            } else {
                (0.0, 0.0)
            };
            let track = Rect::new(px0 + left, py1 - height, px1 - right, py1);
            paint_classic_scrollbar(scene, colors, self.transform, track.scale_from_origin(s), false, client.0, scroll.0, offset.x, s);
            if bars.shown.y {
                // The corner between the two.
                let x0 = if bars.vertical_on_left { px0 } else { px1 - right };
                let corner = Rect::new(x0, py1 - height, x0 + left.max(right), py1).scale_from_origin(s);
                scene.fill(Fill::NonZero, self.transform, colors.corner, None, &corner);
            }
        }
    }

    /// Paint overlay scrollbar thumbs for scroll containers: `overflow:
    /// scroll`, or `auto` when the content overflows (never `hidden`/`clip`,
    /// which scroll only programmatically). Thumbs appear on scroll and fade
    /// out after a delay ([`BaseDocument::scrollbar_opacity`]); never-scrolled
    /// containers paint nothing, keeping thumbs out of static reftest
    /// screenshots.
    ///
    /// Geometry comes from [`Node::scrollbar_thumb`], shared with the
    /// thumb-drag hit testing in blitz-dom.
    #[cfg(feature = "scrollbars")]
    fn draw_scrollbars(&self, scene: &mut impl PaintScene) {
        // css-scrollbars-1 scrollbar-color: author thumb/track colors
        use blitz_dom::node::{ScrollbarColor, ScrollbarRef};
        use taffy::AbsoluteAxis;
        let (custom_thumb, custom_track) = match self.node.scrollbar_color() {
            ScrollbarColor::Auto => (None, None),
            ScrollbarColor::Colors { thumb, track } => {
                (Some(thumb.as_srgb_color()), Some(track.as_srgb_color()))
            }
        };

        let drag_target = self.context.scrollbar_drag_target;
        let hovered_thumb = self.context.hovered_scrollbar;

        // scrollbar-color doesn't affect overlay visibility: persistence is
        // UA policy, not author styling.
        let node_id = self.node.id;
        let opacity = self.context.dom.scrollbar_opacity(node_id);
        if opacity == 0.0 {
            return;
        }

        // Default thumb palette for the used color scheme; thumbs paint as
        // fill plus a thin contrast stroke so they read over same-colored
        // content.
        let dark_scheme =
            self.context.dom.viewport().color_scheme == blitz_traits::shell::ColorScheme::Dark;
        let (thumb_rest, thumb_hover, thumb_active, stroke_color) = if dark_scheme {
            (
                Color::from_rgba8(214, 214, 214, 178),
                Color::from_rgba8(190, 190, 190, 222),
                Color::from_rgba8(172, 172, 172, 255),
                Color::from_rgba8(0, 0, 0, 102),
            )
        } else {
            (
                Color::from_rgba8(128, 128, 128, 178),
                Color::from_rgba8(152, 152, 152, 222),
                Color::from_rgba8(170, 170, 170, 255),
                Color::from_rgba8(255, 255, 255, 102),
            )
        };

        // Chromium's hovered/pressed scrollbar contrast ratios.
        const HOVER_CONTRAST: f32 = 1.8;
        const ACTIVE_CONTRAST: f32 = 1.3;

        for axis in [AbsoluteAxis::Vertical, AbsoluteAxis::Horizontal] {
            if !self.node.wants_scrollbar(axis) {
                continue;
            }
            let Some(thumb) = self.node.scrollbar_thumb(axis) else {
                continue;
            };

            let rect = thumb.scale_from_origin(self.scale);

            // Track (only when the author specified a track color)
            if let Some(track_color) = custom_track {
                let padding_box = self.frame.padding_box;
                let track_rect = match axis {
                    AbsoluteAxis::Horizontal => {
                        Rect::new(padding_box.x0, rect.y0, padding_box.x1, rect.y1)
                    }
                    AbsoluteAxis::Vertical => {
                        Rect::new(rect.x0, padding_box.y0, rect.x1, padding_box.y1)
                    }
                };
                scene.fill(
                    Fill::NonZero,
                    self.transform,
                    track_color.multiply_alpha(opacity),
                    None,
                    &track_rect,
                );
            }

            let this = ScrollbarRef { node_id, axis };
            let is_active = drag_target == Some(this);
            let is_hovered = hovered_thumb == Some(this);
            let color = match custom_thumb {
                Some(base) if is_active => crate::color::blend_for_contrast(base, ACTIVE_CONTRAST),
                Some(base) if is_hovered => crate::color::blend_for_contrast(base, HOVER_CONTRAST),
                Some(base) => base,
                None if is_active => thumb_active,
                None if is_hovered => thumb_hover,
                None => thumb_rest,
            };
            let radius = match axis {
                AbsoluteAxis::Horizontal => rect.height() / 2.0,
                AbsoluteAxis::Vertical => rect.width() / 2.0,
            };
            scene.fill(
                Fill::NonZero,
                self.transform,
                color.multiply_alpha(opacity),
                None,
                &rect.to_rounded_rect(radius),
            );
            // Contrast stroke, default thumbs only: an author-specified
            // scrollbar-color is rendered exactly as given.
            if custom_thumb.is_none() {
                let stroke_width = self.scale;
                let stroke_rect = rect.inset(-stroke_width / 2.0);
                scene.stroke(
                    &Stroke::new(stroke_width),
                    self.transform,
                    stroke_color.multiply_alpha(opacity),
                    None,
                    &stroke_rect.to_rounded_rect(radius - stroke_width / 2.0),
                );
            }
        }
    }

    /// pyro: paints the boxes of the inline (non-atomic) elements in this inline root's lines, as
    /// Chromium does. An element has one fragment per line it has content on: from the start of
    /// its first content there to the end of its last (its own start and end edges without their
    /// margins among them), as tall as its content area (the ascent and descent of its first
    /// font) plus its block-axis padding and border. The element's first fragment has its start
    /// side and its last the end side; the sides in between are open (`box-decoration-break:
    /// slice`): no border, no padding, square corners. A fragment's edges are snapped to CSS
    /// pixels and it paints as a box does: outer shadow, background, inner shadow, border,
    /// outline. On a line, a box paints before the boxes inside it. `transform` is the lines', and
    /// `tops` where each line's text goes: its boxes go with it. pyro: the fragments in paint
    /// order (by line), for `paint_inline_box_fragment`.
    fn inline_box_fragments(
        &self,
        text_layout: &blitz_dom::node::TextLayout,
        transform: Affine,
        tops: Option<crate::text::LineTops>,
        turned: Option<&dyn Fn(usize) -> Affine>,
    ) -> Vec<InlineBoxFragment> {
        let layout = &text_layout.layout;
        let mut painted_fragments = Vec::new();
        use parley::{InlineBoxKind, PositionedLayoutItem};
        use style::computed_values::direction::T as Direction;
        use style::values::computed::{Length, LengthPercentage};
        use style::values::generics::image::GenericImage;

        struct Fragment {
            node: NodeId,
            /// How many inline elements of the root the fragment's is in, itself included.
            depth: usize,
            line: usize,
            x0: f32,
            x1: f32,
            top: f32,
            bottom: f32,
            /// pyro: the (visual) index of the last item it holds.
            last_item: usize,
            /// pyro: the top of its line box (where the text of the line above ends).
            line_top: f32,
            /// pyro: its element's style on the first line (`BaseDocument::first_line_style`).
            style: Option<style::servo_arc::Arc<ComputedValues>>,
        }

        let styles = layout.styles();
        // pyro: a root of text alone has no inline box, but for its `::first-line` box.
        if styles.len() < 2 && text_layout.first_line.is_none() {
            return painted_fragments;
        }
        let dom = self.context.dom;
        let root = self.node.id;
        let length = |v: &LengthPercentage| v.resolve(Length::new(0.0)).px();

        // Whether an element has a box to paint.
        let mut painted: HashMap<NodeId, bool> = HashMap::new();
        let mut paints = |id: NodeId| {
            *painted.entry(id).or_insert_with(|| {
                let Some(node) = dom.get_node(id).filter(|node| node.element_data().is_some()) else {
                    return false;
                };
                let Some(style) = node.primary_styles() else {
                    return false;
                };
                // pyro: a `display: contents` element is a span of its text's styles (blitz-dom's
                // inline build) without a box.
                if style.get_inherited_box().visibility != StyloVisibility::Visible
                    || style.clone_display().is_contents()
                {
                    return false;
                }
                let background = style.get_background();
                let border = style.get_border();
                background.background_color.resolve_to_absolute(&style.clone_color()).as_srgb_color()
                    != Color::TRANSPARENT
                    || background.background_image.0.iter().any(|image| !matches!(image, GenericImage::None))
                    || !border.border_top_style.none_or_hidden()
                    || !border.border_right_style.none_or_hidden()
                    || !border.border_bottom_style.none_or_hidden()
                    || !border.border_left_style.none_or_hidden()
                    || !style.get_effects().box_shadow.0.is_empty()
                    || style.get_outline().outline_width.0.to_f64_px() != 0.0
            })
        };

        // The inline elements a style's content is in, innermost first, each with the style it
        // made (a `display: contents` element or a `<br>` inside it shares its brush).
        let mut chain: Vec<(NodeId, u16)> = Vec::new();
        let chain_of = |chain: &mut Vec<(NodeId, u16)>, mut style_index: u16| {
            chain.clear();
            loop {
                let Some(style) = styles.get(usize::from(style_index)) else {
                    break;
                };
                let id = style.brush.id;
                if id == root {
                    break;
                }
                match chain.last_mut() {
                    Some(last) if last.0 == id => last.1 = style_index,
                    _ => chain.push((id, style_index)),
                }
                if style_index == 0 {
                    break;
                }
                style_index = layout.style_parent(style_index);
            }
        };

        let mut fragments: Vec<Fragment> = Vec::new();
        let rtl_layout = layout.is_rtl();
        // pyro: the first line's content is in the `::first-line` box (blitz-dom
        // `TextLayout::first_line`), outside the elements on it, which have their styles there.
        let first_line = text_layout.first_line.filter(|&id| dom.get_node(id).is_some());
        let mut first_line_styles = text_layout.first_line_styles.clone();
        let mut item_index = 0usize;
        for (line_index, line) in layout.lines().enumerate() {
            let first = fragments.len();
            let metrics = line.metrics();
            // pyro: the empty inline boxes of an invisible line box have no height: nothing to
            // paint (WPT css-inline `empty-span-size-*`).
            if metrics.invisible {
                item_index += line.items().count();
                continue;
            }
            // Collapsible white space hanging at the line's end is in no box (preserved and
            // ideographic spaces that hang are: `LineMetrics::collapsed_advance`). pyro: the line starts at its
            // `inline_min_coord`, past the floats before it (its items are placed from there): without
            // it every box on a line beside a float was cut to nothing (WPT `clear-inline-001`).
            // pyro: from where its items end, which counts justification (a justified line's
            // `advance` does not: WPT css-text `text-justify-and-trailing-spaces-001`).
            let items_end = line
                .items()
                .map(|item| match &item {
                    PositionedLayoutItem::GlyphRun(run) => run.offset() + run.advance(),
                    PositionedLayoutItem::InlineBox(ibox) => ibox.x + ibox.width,
                })
                .fold(metrics.inline_min_coord + metrics.offset + metrics.advance, f32::max);
            let content_end = items_end - metrics.collapsed_advance;
            for item in line.items() {
                let (style_index, x0, x1, edge) = match &item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let x0 = run.offset();
                        let mut x1 = x0 + run.advance();
                        // pyro: a run of bidi formatting characters alone (U+202E written in the
                        // text, placed at the level before it) is no content of a box: it made a
                        // fragment of its own away from the content it reorders.
                        if run.advance() == 0.0
                            && text_layout.text.get(run.run().text_range()).is_some_and(|text| {
                                !text.is_empty()
                                    && text.chars().all(|c| {
                                        matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
                                    })
                            })
                        {
                            continue;
                        }
                        if !rtl_layout {
                            x1 = x1.min(content_end).max(x0);
                        }
                        (run.style_index(), x0, x1, None)
                    }
                    PositionedLayoutItem::InlineBox(ibox) => match ibox.kind {
                        InlineBoxKind::InFlow => (ibox.style_index, ibox.x, ibox.x + ibox.width, None),
                        InlineBoxKind::SpanStart | InlineBoxKind::SpanEnd => {
                            (ibox.style_index, ibox.x, ibox.x + ibox.width, Some((NodeId::from_u64(ibox.id), ibox.kind)))
                        }
                        _ => continue,
                    },
                };
                item_index += 1;
                chain_of(&mut chain, style_index);
                let on_first_line = first_line.filter(|_| line.text_range().start == 0);
                if let Some(first_line) = on_first_line {
                    chain.push((first_line, line.root_style()));
                }
                let depth = chain.len();
                for (level, &(node, style_index)) in chain.iter().enumerate() {
                    if !paints(node) {
                        continue;
                    }
                    // The element's own edge is in its box without the margin.
                    let (mut x0, mut x1) = (x0, x1);
                    if let Some((_, kind)) = edge.filter(|&(id, _)| id == node)
                        && let Some(style) = dom.get_node(node).and_then(|node| node.primary_styles())
                    {
                        // pyro: the inline-start and -end margins (top and bottom in a vertical
                        // writing mode, whose lines are laid out turned).
                        let (start, end) = blitz_dom::span_edge_margins(&style, 0.0);
                        let rtl = style.clone_direction() == Direction::Rtl;
                        let margin = if kind == InlineBoxKind::SpanStart { start } else { end } * self.scale as f32;
                        if (kind == InlineBoxKind::SpanStart) != rtl {
                            x0 += margin;
                        } else {
                            x1 -= margin;
                        }
                    }
                    // pyro: a fragment holds the items of its element that are next to each other in
                    // visual order: bidi reordering can split what an element holds on a line.
                    match fragments[first..].iter_mut().rev().find(|fragment| fragment.node == node) {
                        Some(fragment) if fragment.last_item + 1 == item_index || fragment.last_item == item_index => {
                            fragment.x0 = fragment.x0.min(x0);
                            fragment.x1 = fragment.x1.max(x1);
                            fragment.last_item = item_index;
                        }
                        _ => {
                            let (top, bottom) = line.span_box_area(style_index);
                            let shift = tops.map_or(0.0, |tops| tops.shift(metrics)) as f32;
                            let style = on_first_line
                                .filter(|&first_line| first_line != node)
                                .and_then(|first_line| dom.first_line_style(root, first_line, node, &mut first_line_styles));
                            fragments.push(Fragment {
                                node,
                                depth: depth - level,
                                line: line_index,
                                x0,
                                x1,
                                top: top + shift,
                                bottom: bottom + shift,
                                last_item: item_index,
                                line_top: metrics.block_min_coord + shift,
                                style,
                            });
                        }
                    }
                }
            }
        }
        if fragments.is_empty() {
            return painted_fragments;
        }

        // An element's first and last fragment on its first line, and on its last (CSS 2.1 § 8.6:
        // in a left-to-right element the first line's left-most fragment has the left side and the
        // last line's right-most the right side; in a right-to-left one the first line's
        // right-most has the right side and the last line's left-most the left side), and how wide
        // its fragments are together (device px): with `box-decoration-break: slice` its fragments
        // are pieces of one box.
        let mut ends: HashMap<NodeId, ([usize; 4], f32)> = HashMap::new();
        for (index, fragment) in fragments.iter().enumerate() {
            let width = fragment.x1 - fragment.x0;
            ends.entry(fragment.node)
                .and_modify(|(ends, total)| {
                    if fragment.line == fragments[ends[0]].line {
                        ends[1] = index;
                    }
                    if fragment.line > fragments[ends[2]].line {
                        ends[2] = index;
                    }
                    ends[3] = index;
                    *total += width;
                })
                .or_insert(([index; 4], width));
        }
        // pyro: a split inline's start and end are in the pieces that hold them.
        let piece = |node: NodeId| {
            text_layout
                .split_pieces
                .iter()
                .find(|piece| piece.0 == node)
                .map_or((true, true), |piece| (piece.1, piece.2))
        };
        // pyro: relatively positioned elements move their fragments.
        let relative = dom.inline_root_has_relative_spans(root);
        // pyro: the fragments on the lines a line clamp discarded are not painted; the boxes they
        // continue keep their sides as they were (WPT css-overflow `line-clamp/block-ellipsis-004`).
        let kept_lines = text_layout.clamp.as_ref().map_or(usize::MAX, |clamp| clamp.lines as usize);
        let mut order: Vec<usize> = (0..fragments.len()).filter(|&index| fragments[index].line < kept_lines).collect();
        order.sort_by_key(|&index| (fragments[index].line, fragments[index].depth));

        // The boxes are where the text is.
        let transform = transform * Affine::translate((self.fractional_text_x_of_node(), 0.0));
        let [a, b, c, d, tx, ty] = transform.as_coeffs();
        let translation_only = a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0;
        let scale = self.scale as f32;
        for index in order {
            let fragment = &fragments[index];
            let Some(node) = dom.get_node(fragment.node) else {
                continue;
            };
            let Some(style) = node.primary_styles() else {
                continue;
            };
            let (sides, total) = ends[&fragment.node];
            let (first, last) = (sides[0], sides[3]);
            let (has_start, has_end) = piece(fragment.node);
            let rtl = style.clone_direction() == Direction::Rtl;
            let (left, right) = if rtl {
                (has_end && sides[2] == index, has_start && sides[1] == index)
            } else {
                (has_start && sides[0] == index, has_end && sides[3] == index)
            };
            // pyro: a box sliced across lines (`box-decoration-break: slice`, the initial value)
            // casts the shadows and shows the background images of the whole box, its fragments
            // laid side by side, each fragment its piece of them (Blink's
            // `PaintRectForImageStrip`): how far the whole box reaches past the fragment's left
            // and right (device px). Only where that shows: a box of one colour paints the same
            // either way.
            let effects = style.get_effects();
            // pyro: an outer shadow reaching left of the box, over the text before it on its line
            // (`InlineBoxFragment::reaches_back`).
            // pyro: and so does a box pulled back over that text by a negative left margin (its
            // background over the text, the text after it over its background: WPT CSS2
            // `css1/c5504-imrgn-l-005`).
            let pulled_back = match &style.get_margin().margin_left {
                style::values::generics::length::GenericMargin::LengthPercentage(margin) => {
                    margin.resolve(CSSPixelLength::new(0.0)).px() < 0.0
                }
                _ => false,
            };
            let reaches_back = !rtl
                && (pulled_back
                    || effects
                        .box_shadow
                        .0
                        .iter()
                        .any(|shadow| !shadow.inset && shadow.base.horizontal.px() - shadow.spread.px() - shadow.base.blur.px() < 0.0));
            let stitch = (!(first == index && last == index)
                && style.get_border().box_decoration_break == style::values::computed::BoxDecorationBreak::Slice
                && (!effects.box_shadow.0.is_empty()
                    || style
                        .get_background()
                        .background_image
                        .0
                        .iter()
                        .any(|image| !matches!(image, GenericImage::None))))
            .then(|| {
                let width = fragment.x1 - fragment.x0;
                let earlier: f32 = fragments[first..index]
                    .iter()
                    .filter(|other| other.node == fragment.node)
                    .map(|other| other.x1 - other.x0)
                    .sum();
                let later = total - earlier - width;
                if rtl { (later, earlier) } else { (earlier, later) }
            });
            // pyro: on the first line, its edges as the element has them there (`em` widths follow
            // the `::first-line` font size).
            let line_style = fragment.style.clone();
            let edge_style: &ComputedValues = match &line_style {
                Some(line_style) => line_style,
                None => &style,
            };
            let (border, padding) = (edge_style.get_border(), edge_style.get_padding());
            let side = |open: bool, width: f32| if open { 0.0 } else { width };
            // The used width: none where the side has no border style.
            let used = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
                if style.none_or_hidden() { 0.0 } else { width.0.to_f32_px() }
            };
            let whole_border = taffy::Rect {
                left: used(&border.border_left_width, border.border_left_style),
                right: used(&border.border_right_width, border.border_right_style),
                top: used(&border.border_top_width, border.border_top_style),
                bottom: used(&border.border_bottom_width, border.border_bottom_style),
            };
            let whole_padding = taffy::Rect {
                left: length(&padding.padding_left.0),
                right: length(&padding.padding_right.0),
                top: length(&padding.padding_top.0),
                bottom: length(&padding.padding_bottom.0),
            };
            // pyro: a vertical writing mode's lines are laid out turned (`draw_vertical_inline_layout`):
            // a line's left (inline start) is the box's top, its over side (where y is least) its
            // right, in `vertical-rl` and `vertical-lr` alike. The fragment is the box's physical
            // piece: its top and bottom sides are the open ones, its right and left reach over and
            // under the line.
            if let Some(turned) = turned {
                drop(style);
                let border = taffy::Rect {
                    top: side(!left, whole_border.top),
                    bottom: side(!right, whole_border.bottom),
                    ..whole_border
                };
                let padding = taffy::Rect {
                    top: side(!left, whole_padding.top),
                    bottom: side(!right, whole_padding.bottom),
                    ..whole_padding
                };
                let rect = Rect::new(
                    fragment.x0 as f64,
                    (fragment.top - (border.right + padding.right) * scale) as f64,
                    fragment.x1 as f64,
                    (fragment.bottom + (border.left + padding.left) * scale) as f64,
                );
                let line_transform = turned(fragment.line);
                let [a, b, c, d, tx, ty] = self.transform.as_coeffs();
                let (transform, size) = if a == 1.0 && b == 0.0 && c == 0.0 && d == 1.0 {
                    // To whole CSS pixels, as horizontal fragments.
                    let bounds = (self.transform.inverse() * line_transform).transform_rect_bbox(rect);
                    let snap = |v: f64| (v / self.scale).round() * self.scale;
                    let (x0, y0) = (snap(tx + bounds.x0), snap(ty + bounds.y0));
                    let (x1, y1) = (snap(tx + bounds.x1), snap(ty + bounds.y1));
                    (Affine::translate((x0, y0)), Size::new(x1 - x0, y1 - y0))
                } else {
                    let bounds = (self.transform.inverse() * line_transform).transform_rect_bbox(rect);
                    (self.transform * Affine::translate((bounds.x0, bounds.y0)), bounds.size())
                };
                painted_fragments.push(InlineBoxFragment {
                    text_start: None,
                    node: fragment.node,
                    line: fragment.line,
                    transform,
                    size,
                    border,
                    padding,
                    whole: None,
                    left,
                    right,
                    reaches_above: false,
                    reaches_back: false,
                    x0: fragment.x0,
                    style: fragment.style.clone(),
                    turned: true,
                });
                continue;
            }
            let border = taffy::Rect {
                left: side(!left, whole_border.left),
                right: side(!right, whole_border.right),
                ..whole_border
            };
            let padding = taffy::Rect {
                left: side(!left, whole_padding.left),
                right: side(!right, whole_padding.right),
                ..whole_padding
            };
            drop(style);
            let rect = Rect::new(
                fragment.x0 as f64,
                (fragment.top - (border.top + padding.top) * scale) as f64,
                fragment.x1 as f64,
                (fragment.bottom + (border.bottom + padding.bottom) * scale) as f64,
            );
            let rect = if relative {
                let (dx, dy) = dom.inline_span_relative_offset(fragment.node, root);
                rect + kurbo::Vec2::new((dx * scale) as f64, (dy * scale) as f64)
            } else {
                rect
            };
            // Where a rectangle of the lines goes, and its size (device px).
            let place = |rect: Rect| {
                if translation_only {
                    // To whole CSS pixels, as Chromium snaps a box's decorations
                    // (and as block boxes are laid out), whatever the device scale.
                    let snap = |v: f64| (v / self.scale).round() * self.scale;
                    let (x0, y0) = (snap(tx + rect.x0), snap(ty + rect.y0));
                    let (x1, y1) = (snap(tx + rect.x1), snap(ty + rect.y1));
                    (Affine::translate((x0, y0)), Size::new(x1 - x0, y1 - y0))
                } else {
                    (transform * Affine::translate((rect.x0, rect.y0)), rect.size())
                }
            };
            let (transform, size) = place(rect);
            // The whole box, for a sliced box's shadows and background images (see
            // `paint_inline_box_fragment`).
            let whole = stitch.map(|(grow_left, grow_right)| {
                let whole = Rect::new(rect.x0 - grow_left as f64, rect.y0, rect.x1 + grow_right as f64, rect.y1);
                let (transform, size) = place(whole);
                (transform, size, whole_border, whole_padding)
            });
            painted_fragments.push(InlineBoxFragment {
                node: fragment.node,
                line: fragment.line,
                transform,
                size,
                border,
                padding,
                whole,
                left,
                right,
                reaches_above: (rect.y0 as f32) < fragment.line_top,
                reaches_back: reaches_back && left,
                text_start: (pulled_back && left && !rtl)
                    .then(|| {
                        text_layout.layout.inline_boxes().find(|ibox| {
                            ibox.id == fragment.node.as_u64() && ibox.kind == parley::InlineBoxKind::SpanStart
                        })
                    })
                    .flatten()
                    .map(|ibox| ibox.index),
                x0: fragment.x0,
                style: fragment.style.clone(),
                turned: false,
            });
        }
        painted_fragments
    }

    /// pyro: paints a fragment of `inline_box_fragments`: outer shadow, background, inner shadow,
    /// border and outline. `transform` and `tops` are the root's text's (for a `background-clip:
    /// text` mask).
    fn paint_inline_box_fragment(
        &self,
        scene: &mut impl PaintScene,
        fragment: &InlineBoxFragment,
        transform: Affine,
        tops: Option<crate::text::LineTops>,
    ) {
        let Some(node) = self.context.dom.get_node(fragment.node) else {
            return;
        };
        let (text_transform, text_shift) = (transform, self.fractional_text_x_of_node());
        let box_cx = |transform: Affine, size: Size, border, padding| {
            let layout = Layout {
                size: taffy::Size {
                    width: (size.width / self.scale) as f32,
                    height: (size.height / self.scale) as f32,
                },
                border,
                padding,
                ..Layout::new()
            };
            self.context.element_cx(node, layout, transform, None)
        };
        let (transform, size, left, right) = (fragment.transform, fragment.size, fragment.left, fragment.right);
        let mut cx = box_cx(transform, size, fragment.border, fragment.padding);
        if let Some(style) = &fragment.style {
            cx.style = style.clone();
        }
        // pyro: the open sides' corners are square, left out before overlapping radii are scaled
        // down (a turned (vertical) fragment's open sides are its top and bottom).
        if !(left && right) {
            let keep = if fragment.turned { [left, left, right, right] } else { [left, right, right, left] };
            let layout = Layout {
                size: taffy::Size {
                    width: (size.width / self.scale) as f32,
                    height: (size.height / self.scale) as f32,
                },
                border: fragment.border,
                padding: fragment.padding,
                ..Layout::new()
            };
            cx.frame = create_css_rect_with_corners(&cx.style, &layout, self.scale, keep);
        }
        // pyro: a box painted in a group of its own (a clip path, a mask, opacity) keeps its outline
        // under the text, in its group (WPT css-masking `clip-path-inline-007`).
        let grouped = {
            use style::values::generics::image::GenericImage;
            !matches!(cx.style.get_svg().clip_path, style::values::computed::basic_shape::ClipPath::None)
                || cx.style.get_svg().mask_image.0.iter().any(|image| !matches!(image, GenericImage::None))
                || cx.style.get_effects().opacity < 1.0
        };
        let outline_now = match self.context.inline_outlines.get() {
            0 => true,
            1 => grouped,
            _ => {
                if !grouped {
                    cx.draw_outline(scene);
                }
                return;
            }
        };
        // The whole box, for a sliced box's shadows and background images, and the fragment's
        // piece of it: its border box, and for the outer shadows the space past its closed
        // sides and above and below it.
        let whole = fragment.whole.map(|(transform, size, border, padding)| {
            let mut whole = box_cx(transform, size, border, padding);
            if let Some(style) = &fragment.style {
                whole.style = style.clone();
            }
            whole
        });
        let piece = Rect::from_origin_size(Point::ZERO, size);
        let far = 1e6;
        let outside = if fragment.turned {
            Rect::new(-far, if left { -far } else { 0.0 }, far, if right { far } else { size.height })
        } else {
            Rect::new(if left { -far } else { 0.0 }, -far, if right { far } else { size.width }, far)
        };
        match &whole {
            Some(whole) => {
                scene.push_clip_layer(Fill::NonZero, transform, &outside);
                whole.draw_outset_box_shadow(scene);
                scene.pop_layer();
            }
            None => cx.draw_outset_box_shadow(scene),
        }
        if self.context.backgrounds {
            // pyro: a `background-clip: text` layer shows through the box's own glyphs.
            let text_clip = cx.style.get_background().background_clip.0.iter().any(|clip| {
                *clip == style::properties::generated::longhands::background_clip::single_value::computed_value::T::Text
            });
            if text_clip {
                // The glyphs `text-overflow: ellipsis` cuts from the line are not in the mask
                // either (`stroke_inline_text`).
                let scale = self.scale as f32;
                let cut = self.element.inline_layout_data.as_ref().and_then(|text_layout| {
                    let line = text_layout.layout.get(fragment.line)?;
                    line_truncation(self.node, text_layout, &line, fragment.line, scale).map(|truncation| truncation.cut)
                });
                self.context.draw_text_context.borrow_mut().inline_mask = Some(crate::text::InlineTextMask {
                    element: fragment.node,
                    root: self.node.id,
                    transform: text_transform,
                    tops,
                    text_shift,
                    line: fragment.line,
                    cut,
                });
            }
            match &whole {
                Some(whole) => {
                    scene.push_clip_layer(Fill::NonZero, transform, &piece);
                    whole.draw_background(scene);
                    scene.pop_layer();
                }
                None => cx.draw_background(scene),
            }
            if text_clip {
                self.context.draw_text_context.borrow_mut().inline_mask = None;
            }
        }
        match &whole {
            Some(whole) => {
                scene.push_clip_layer(Fill::NonZero, transform, &piece);
                whole.draw_inset_box_shadow(scene);
                scene.pop_layer();
            }
            None => cx.draw_inset_box_shadow(scene),
        }
        cx.draw_border(scene);
        // Over the box, as a block's outline is (see `render_element`), unless the inline root
        // draws it after its text (`inline_outlines`).
        if outline_now {
            cx.draw_outline(scene);
        }
    }

    fn draw_inline_layout(&self, scene: &mut impl PaintScene, pos: Point) {
        // (Where a positioned inline box left for its paint-tree entry is painted from: `draw_inline_pass`.)
        let pos_before_offset = pos;
        if self.node.flags.is_inline_root() {
            // pyro: inline elements with a clip path or a mask (clip_path.rs).
            if self.draw_inline_effects(scene, pos) {
                return;
            }
            let text_layout = self.element
                .inline_layout_data
                .as_ref()
                .unwrap_or_else(|| {
                    panic!("Tried to render node marked as inline root that does not have an inline layout: {:?}", self.node);
                });

            if let Some(block_lr) = self.node.vertical_block_axis() {
                self.draw_vertical_inline_layout(scene, text_layout, pos, block_lr, None);
                return;
            }

            // Text keeps its unrounded position; only box edges are snapped to whole pixels.
            let subpixel_offset = text_layout.content_box_subpixel_offset;
            let pos = Point {
                x: pos.x + subpixel_offset.x as f64,
                y: pos.y + subpixel_offset.y as f64 + text_layout.block_offset as f64,
            };
            let transform =
                self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));
            // pyro: each line's top at the whole CSS pixel Chromium paints it at.
            let (text_shift, tops) = self.text_placement(text_layout.block_offset as f64);

            // Render the boxes of inline elements (e.g. `<span style="background: ...">`)
            // behind the text and selection highlight. pyro: line by line, each line's boxes
            // and then its text, as CSS 2.1 Appendix E paints line boxes (a box on a line
            // paints over the text of the line above where it reaches into it), when a box after
            // the first line reaches above its line and no selection, ellipsis or annotation
            // interleaves.
            let fragments = self.inline_box_fragments(text_layout, transform, tops, None);
            // pyro: the boxes' outlines go over the text (`inline_outlines`), drawn at the end.
            let outer_outlines = self.context.inline_outlines.replace(1);
            let selection = self.context.selection_ranges.get(&self.node.id).copied();
            let highlighted = self.context.highlight_ranges.get(&self.node.id);
            let by_line = highlighted.is_none()
                && text_layout.ellipsis.is_none()
                && text_layout.clamp.is_none()
                && text_layout.annotations.is_empty()
                && fragments
                    .iter()
                    .any(|fragment| (fragment.line > 0 && fragment.reaches_above) || fragment.reaches_back);
            if by_line {
                let options = TextPaintOptions {
                    scale: self.scale,
                    brush_override: None,
                    shadows: true,
                };
                for (index, line) in text_layout.layout.lines().enumerate() {
                    let on_line = || fragments.iter().filter(move |fragment| fragment.line == index);
                    // pyro: the text before the first box whose shadow reaches back over it, the
                    // boxes before that box, then the rest (`InlineBoxFragment::reaches_back`).
                    let first_back = on_line()
                        .filter(|fragment| fragment.reaches_back)
                        .min_by(|a, b| a.x0.total_cmp(&b.x0));
                    let edge = first_back.map(|fragment| fragment.x0);
                    // pyro: a box pulled back by a negative margin splits the line's text where its
                    // element's text starts, in tree order (`InlineBoxFragment::text_start`).
                    let split = first_back.and_then(|fragment| fragment.text_start);
                    let cuts;
                    let passes: &[Option<crate::text::TextCut>] = match edge {
                        Some(edge) => {
                            cuts = [
                                Some(crate::text::TextCut { edge, keep_left: true, split }),
                                Some(crate::text::TextCut { edge, keep_left: false, split }),
                            ];
                            &cuts
                        }
                        None => &[None],
                    };
                    for (pass, cut) in passes.iter().enumerate() {
                        // (A fragment's `background-clip: text` mask borrows the text context.)
                        for fragment in on_line() {
                            let before = edge.is_some_and(|edge| fragment.x0 < edge);
                            if edge.is_none() || (pass == 0) == before {
                                self.paint_inline_box_fragment(scene, fragment, transform, tops);
                            }
                        }
                        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
                        let outer_cut = std::mem::replace(&mut draw_text_context.cut, *cut);
                        draw_text_context.text_shift = text_shift;
                        draw_text_context.line_tops = tops;
                        draw_text_context.first_line = text_layout.first_line;
                        crate::text::stroke_text(
                            scene,
                            std::iter::once(line),
                            self.context.dom,
                            transform,
                            options,
                            self.node.id,
                            Some(&text_layout.text),
                            &mut draw_text_context,
                            None,
                            Some(&text_layout.layout),
                        );
                        draw_text_context.text_shift = 0.0;
                        draw_text_context.line_tops = None;
                        draw_text_context.first_line = None;
                        draw_text_context.cut = outer_cut;
                    }
                }
                self.context.inline_outlines.set(2);
                for fragment in &fragments {
                    self.paint_inline_box_fragment(scene, fragment, transform, tops);
                }
                self.context.inline_outlines.set(outer_outlines);
                return;
            }
            // pyro: positioned inline boxes paint after the in-flow content (`positioned_passes`;
            // not with highlights or truncated lines).
            let passes = match highlighted.is_none() && text_layout.ellipsis.is_none() && text_layout.clamp.is_none() {
                true => self.positioned_passes(text_layout),
                false => Vec::new(),
            };
            let main_pass = (!passes.is_empty()).then_some(None);
            for fragment in &fragments {
                if self.fragment_in_pass(fragment.node, main_pass) {
                    self.paint_inline_box_fragment(scene, fragment, transform, tops);
                }
            }

            // Render text selection highlight (if any) using cached selection ranges
            // pyro: highlighted as Chromium paints selected text (`highlight`), but in lines cut
            // by an ellipsis; a recording paint (the PDF's) draws no selection.
            let highlight = highlighted
                .filter(|_| {
                    text_layout.ellipsis.is_none()
                        && text_layout.clamp.is_none()
                        && self.context.draw_text_context.borrow().runs.is_none()
                });
            if let (Some((sel_start, sel_end)), None) = (selection, highlight) {
                crate::text::draw_text_selection(
                    scene,
                    &text_layout.layout,
                    |line| match (tops, text_layout.layout.get(line)) {
                        (Some(tops), Some(line)) => transform * Affine::translate((0.0, tops.shift(line.metrics()))),
                        _ => transform,
                    },
                    sel_start,
                    sel_end,
                );
            }

            // Render text
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            draw_text_context.text_shift = text_shift;
            draw_text_context.line_tops = tops;
            draw_text_context.first_line = text_layout.first_line;
            let options = TextPaintOptions {
                scale: self.scale,
                brush_override: None,
                shadows: true,
            };
            match highlight {
                Some(ranges) => crate::highlight::stroke_highlighted_text(
                    scene,
                    self.context.dom,
                    self.node.id,
                    &text_layout.layout,
                    &text_layout.text,
                    |_| transform,
                    tops,
                    options,
                    &self.context.highlight_layers,
                    ranges,
                    &mut draw_text_context,
                    |_| None,
                ),
                None => {
                    draw_text_context.positioned_pass = main_pass;
                    stroke_inline_text(
                        scene,
                        self.context.dom,
                        self.node,
                        text_layout,
                        transform,
                        options,
                        &mut draw_text_context,
                        tops,
                        self.scale,
                    );
                    draw_text_context.positioned_pass = None;
                }
            }
            // pyro: then each positioned inline box: its fragments, then its text; a box listed in the
            // paint tree (`NodeFlags::INLINE_PASS_ENTRY`) is left for its entry there, which paints it
            // in the positioned layer in tree order (`draw_inline_pass_entry`).
            drop(draw_text_context);
            for &pass in passes.iter().skip(1) {
                if let Some(span) = pass
                    && self.context.dom.get_node(span).is_some_and(|node| {
                        node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY)
                    })
                {
                    // pyro: in a multi-column inline root, the column that shows the box records it
                    // (`PaintContext::inline_column_range`).
                    // (Each column that shows one of its lines.)
                    let shown_here = self.context.inline_column_range.get().is_none_or(|(start, end)| {
                        let mut own = fragments.iter().filter(|fragment| fragment.node == span).peekable();
                        own.peek().is_none()
                            || own.any(|fragment| {
                                // (Its line's top in the root's box: the strip.)
                                let layout = &text_layout.layout;
                                let line_top = layout.get(fragment.line).map_or(0.0, |line| line.metrics().block_min_coord / layout.scale());
                                let y = (pos_before_offset.y / self.scale) as f32 + text_layout.block_offset + line_top;
                                y >= start - 0.5 && y < end - 0.5
                            })
                    });
                    match self.context.inline_column_range.get() {
                        Some(range) if shown_here => self
                            .context
                            .inline_pass_entries
                            .borrow_mut()
                            .entry(span)
                            .or_default()
                            .push((self.node.id, self.transform, pos_before_offset, Some(range))),
                        Some(_) => {}
                        None => {
                            self.context.inline_pass_entries.borrow_mut().insert(span, vec![(self.node.id, self.transform, pos_before_offset, None)]);
                        }
                    }
                    continue;
                }
                for fragment in fragments.iter().filter(|fragment| self.fragment_in_pass(fragment.node, Some(pass))) {
                    self.paint_inline_box_fragment(scene, fragment, transform, tops);
                }
                let mut draw_text_context = self.context.draw_text_context.borrow_mut();
                draw_text_context.positioned_pass = Some(pass);
                stroke_inline_text(
                    scene,
                    self.context.dom,
                    self.node,
                    text_layout,
                    transform,
                    options,
                    &mut draw_text_context,
                    tops,
                    self.scale,
                );
                draw_text_context.positioned_pass = None;
            }
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            // pyro: ruby annotations and emphasis marks, over the text of each line.
            if !text_layout.annotations.is_empty() {
                let recording = draw_text_context.runs.is_some();
                for (index, line) in text_layout.layout.lines().enumerate() {
                    if text_layout.clamp.as_ref().is_some_and(|clamp| index >= clamp.lines as usize) {
                        break;
                    }
                    let line_transform = match tops {
                        Some(tops) => transform * Affine::translate((0.0, tops.shift(line.metrics()))),
                        None => transform,
                    } * Affine::translate((text_shift, 0.0));
                    let cut = line_truncation(self.node, text_layout, &line, index, self.scale as f32)
                        .map(|truncation| truncation.cut);
                    crate::text::draw_text_annotations(
                        scene,
                        &line,
                        &text_layout.annotations,
                        &text_layout.text,
                        self.context.dom,
                        line_transform,
                        self.scale,
                        recording,
                        None,
                        None,
                        cut,
                    );
                }
            }
            draw_text_context.text_shift = 0.0;
            draw_text_context.line_tops = None;
            draw_text_context.first_line = None;
            drop(draw_text_context);
            // pyro: the caret in this text (`caret`).
            if let Some(caret) = self.context.caret.as_ref().filter(|caret| caret.node == self.node.id && !caret.empty) {
                let layout = self.node.final_layout();
                let content_width = (layout.size.width
                    - layout.border.left
                    - layout.border.right
                    - layout.padding.left
                    - layout.padding.right) as f64;
                crate::caret::draw_text_caret(scene, caret, &text_layout.layout, |index| match (tops, text_layout.layout.get(index)) {
                    (Some(tops), Some(line)) => transform * Affine::translate((0.0, tops.shift(line.metrics()))),
                    _ => transform,
                }, content_width, self.scale);
            }
            // (A box listed in the paint tree paints its own outline with its entry.)
            let entries: Vec<NodeId> = passes
                .iter()
                .skip(1)
                .filter_map(|&pass| pass)
                .filter(|&span| {
                    self.context.dom.get_node(span).is_some_and(|node| {
                        node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY)
                    })
                })
                .collect();
            self.context.inline_outlines.set(2);
            for fragment in &fragments {
                if entries.iter().any(|&span| self.fragment_in_pass(fragment.node, Some(Some(span)))) {
                    continue;
                }
                self.paint_inline_box_fragment(scene, fragment, transform, tops);
            }
            self.context.inline_outlines.set(outer_outlines);
        } else if let Some(caret) = self.context.caret.as_ref().filter(|caret| caret.node == self.node.id && caret.empty) {
            // pyro: the caret of an empty editing host, at the start of its content box.
            let layout = self.node.final_layout();
            let content_width = (layout.size.width
                - layout.border.left
                - layout.border.right
                - layout.padding.left
                - layout.padding.right) as f64;
            // (Its line, the first, starts after the `text-indent`: none when `hanging`.)
            let text_indent = &self.style.get_inherited_text().text_indent;
            let indent = match text_indent.hanging {
                true => 0.0,
                false => text_indent.length.resolve(style::values::computed::Length::new(content_width as f32)).px() as f64,
            };
            // pyro: and where its `text-align` puts the empty line's start.
            let x = {
                use style::values::specified::text::TextAlignKeyword as A;
                let rtl = caret.rtl;
                let align = self.style.clone_text_align();
                let (left, right) = (indent, content_width - indent);
                match align {
                    A::Center | A::MozCenter => content_width / 2.0,
                    A::Left | A::MozLeft => left,
                    A::Right | A::MozRight => right,
                    A::End => if rtl { left } else { right },
                    A::Start | A::Justify | A::JustifyAll => if rtl { right } else { left },
                }
            };
            let transform = self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));
            crate::caret::draw_empty_caret(scene, caret, transform, content_width, x, self.scale);
        }
    }

    /// What moves this inline root's text from its box's whole-pixel x to its layout-unit x (device px).
    fn fractional_text_x_of_node(&self) -> f64 {
        self.text_placement(0.0).0
    }

    /// pyro: where this node's text goes, as Chromium places it: what moves it from its content
    /// box's whole-pixel x to its layout-unit x (device px), and the tops of the lines of a layout
    /// `block_offset` CSS px down the content box (see [`crate::text::LineTops`]).
    fn text_placement(&self, block_offset: f64) -> (f64, Option<crate::text::LineTops>) {
        self.text_placement_of(self.node, block_offset)
    }

    /// pyro: [`Self::text_placement`] of the text of another node (a list item's first line may
    /// be a descendant's).
    fn text_placement_of(&self, node: &Node, block_offset: f64) -> (f64, Option<crate::text::LineTops>) {
        if !self.context.fractional_text_x {
            return (0.0, None);
        }
        let (position, layout) = self.context.dom.physical_unrounded_geometry(node.id);
        let painted = node.final_layout();
        let g = node.start_gutters();
        let (gx, gy) = (g.x as f64, g.y as f64);
        let start = position.x as f64 + (layout.border.left + layout.padding.left) as f64 + gx;
        let painted_x = (position.x as f64).round() + (painted.border.left + painted.padding.left) as f64 + gx;
        let tops = crate::text::LineTops {
            origin: position.y as f64 + (layout.border.top + layout.padding.top) as f64 + gy + block_offset,
            painted: (position.y as f64).round() + (painted.border.top + painted.padding.top) as f64 + gy + block_offset,
            scale: self.scale,
        };
        (((start * 64.0).floor() / 64.0 - painted_x) * self.scale, Some(tops))
    }

    /// Paint an inline layout of a vertical writing mode. The layout's lines are logical: x is the
    /// inline axis and y the block axis, running from the content box's top and its block-start edge.
    /// Each line is drawn rotated a quarter turn clockwise (`vertical-*`'s sideways glyphs have their
    /// tops to the right), which takes y to -x. In `vertical-rl` that also runs the lines the right
    /// way. In `vertical-lr` they run the other way, so a line's slot is found first and the line is
    /// rotated within it. (pyro: `sideways-lr` turns them counterclockwise.)
    fn draw_vertical_inline_layout(
        &self,
        scene: &mut impl PaintScene,
        text_layout: &blitz_dom::node::TextLayout,
        pos: Point,
        block_lr: bool,
        only_pass: Option<NodeId>,
    ) {
        let line_transform = self.vertical_line_transform(text_layout, pos, block_lr);
        self.draw_turned_lines(scene, text_layout, &line_transform, pos, only_pass);
        // pyro: the caret in this text (`caret`), over its turned lines.
        if only_pass.is_none()
            && let Some(caret) = self.context.caret.as_ref().filter(|caret| caret.node == self.node.id && !caret.empty)
        {
            let layout = self.node.final_layout();
            let inline_size = (layout.size.height
                - layout.border.top
                - layout.border.bottom
                - layout.padding.top
                - layout.padding.bottom) as f64;
            crate::caret::draw_text_caret(scene, caret, &text_layout.layout, &line_transform, inline_size, self.scale);
        }
    }

    /// pyro: the transform from the layout space of each line (by index) of this vertical inline
    /// layout to the scene, as [`Self::draw_vertical_inline_layout`] paints them.
    pub(super) fn vertical_line_transform<'t>(
        &'t self,
        text_layout: &'t blitz_dom::node::TextLayout,
        pos: Point,
        block_lr: bool,
    ) -> impl Fn(usize) -> Affine + 't {
        let block_offset = text_layout.block_offset as f64;
        let layout = self.node.final_layout();
        let g = self.node.gutter_insets();
        let content_width = (layout.size.width
            - layout.border.left
            - layout.border.right
            - layout.padding.left
            - layout.padding.right
            - g.left
            - g.right) as f64;
        let origin = if block_lr { pos.x + block_offset } else { pos.x + content_width - block_offset };
        // Boxes are painted at whole pixels. Chromium rounds text only in the block axis, so a
        // line starts where it was laid out in the inline axis.
        let (position, unrounded) = self.context.dom.physical_unrounded_geometry(self.node.id);
        let inline_start = position.y as f64;
        let top = pos.y + inline_start - inline_start.round();
        // pyro: `sideways-lr` turns its lines the other way, a quarter counterclockwise: they
        // run bottom to top, from the content box's bottom, with their line-over side on the left.
        let sideways_lr = self.style.writing_mode.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR);
        let inset = unrounded.border + unrounded.padding + g;
        let bottom = top + (unrounded.size.height - inset.top - inset.bottom) as f64;
        move |index: usize| {
            if sideways_lr {
                return self.transform * Affine::new([0.0, -1.0, 1.0, 0.0, origin * self.scale, bottom * self.scale]);
            }
            let flip = if block_lr {
                text_layout.layout.get(index).map_or(0.0, |line| {
                    let metrics = line.metrics();
                    (metrics.block_min_coord + metrics.block_max_coord) as f64
                })
            } else {
                0.0
            };
            self.transform
                * Affine::new([0.0, 1.0, -1.0, 0.0, origin * self.scale + flip, top * self.scale])
        }
    }

    /// pyro: paints the content of the positioned inline box `span` of this inline root, as
    /// `draw_inline_layout` paints each positioned pass (its fragments, then its text), for its entry in
    /// the paint tree (`draw_inline_pass_entry`). `pos` is the position `draw_inline_layout` was given.
    fn draw_inline_pass(&self, scene: &mut impl PaintScene, pos: Point, span: NodeId) {
        let Some(text_layout) = self.element.inline_layout_data.as_ref() else { return };
        // pyro: on vertical lines, as `draw_vertical_inline_layout` paints its passes (WPT
        // css-position `position-sticky-writing-modes`).
        if let Some(block_lr) = self.node.vertical_block_axis() {
            self.draw_vertical_inline_layout(scene, text_layout, pos, block_lr, Some(span));
            return;
        }
        let subpixel_offset = text_layout.content_box_subpixel_offset;
        let pos = Point {
            x: pos.x + subpixel_offset.x as f64,
            y: pos.y + subpixel_offset.y as f64 + text_layout.block_offset as f64,
        };
        let transform = self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));
        let (text_shift, tops) = self.text_placement(text_layout.block_offset as f64);
        let fragments = self.inline_box_fragments(text_layout, transform, tops, None);
        for fragment in fragments.iter().filter(|fragment| self.fragment_in_pass(fragment.node, Some(Some(span)))) {
            self.paint_inline_box_fragment(scene, fragment, transform, tops);
        }
        let options = TextPaintOptions { scale: self.scale, brush_override: None, shadows: true };
        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
        draw_text_context.text_shift = text_shift;
        draw_text_context.line_tops = tops;
        draw_text_context.first_line = text_layout.first_line;
        draw_text_context.positioned_pass = Some(Some(span));
        stroke_inline_text(
            scene,
            self.context.dom,
            self.node,
            text_layout,
            transform,
            options,
            &mut draw_text_context,
            tops,
            self.scale,
        );
        draw_text_context.positioned_pass = None;
        draw_text_context.text_shift = 0.0;
        draw_text_context.line_tops = None;
        draw_text_context.first_line = None;
    }

    /// pyro: paints the lines of a vertical writing mode's inline layout, each with its
    /// transform from layout space (see [`Self::draw_vertical_inline_layout`]).
    /// pyro: the paint passes of an inline root holding positioned inline boxes (relative or
    /// sticky, not atomic): its in-flow content first (`None`), then each box with its content in
    /// tree order (an outer box before the boxes in it), as CSS 2.1 Appendix E paints positioned
    /// descendants after the in-flow content, so a box's text and shadows cover the root's later
    /// text. Empty (one pass drawing all) without such boxes, or in a recording paint.
    fn positioned_passes(&self, text_layout: &blitz_dom::node::TextLayout) -> Vec<Option<NodeId>> {
        let doc = self.context.dom;
        if !doc.inline_root_has_relative_spans(self.node.id) || self.context.draw_text_context.borrow().runs.is_some() {
            return Vec::new();
        }
        let mut order: Vec<NodeId> = Vec::new();
        for line in text_layout.layout.lines() {
            for item in line.items() {
                // pyro: and atomic inline boxes (badges), from their parent.
                let chain = match item {
                    parley::PositionedLayoutItem::GlyphRun(run) => doc.inline_span_positioned_chain(run.style().brush.id, self.node.id),
                    parley::PositionedLayoutItem::InlineBox(inline_box) => {
                        match doc.get_node(NodeId::from_u64(inline_box.id)).and_then(|node| node.parent) {
                            Some(parent) => doc.inline_span_positioned_chain(parent, self.node.id),
                            None => Vec::new(),
                        }
                    }
                };
                for id in chain.into_iter().rev() {
                    if !order.contains(&id) {
                        order.push(id);
                    }
                }
            }
        }
        if order.is_empty() {
            return Vec::new();
        }
        std::iter::once(None).chain(order.into_iter().map(Some)).collect()
    }

    /// pyro: whether the inline box fragment of `node` paints in the positioned pass `pass`
    /// (`positioned_passes`; every fragment without passes).
    fn fragment_in_pass(&self, node: NodeId, pass: Option<Option<NodeId>>) -> bool {
        pass.is_none_or(|pass| self.context.dom.inline_span_positioned_chain(node, self.node.id).first().copied() == pass)
    }

    fn draw_turned_lines(
        &self,
        scene: &mut impl PaintScene,
        text_layout: &blitz_dom::node::TextLayout,
        line_transform: impl Fn(usize) -> Affine,
        entry_pos: Point,
        only_pass: Option<NodeId>,
    ) {
        let wm = self.style.writing_mode;
        let sideways = wm.intersects(WritingMode::TEXT_SIDEWAYS | WritingMode::VERTICAL_SIDEWAYS);
        let all_upright = wm.contains(WritingMode::UPRIGHT);
        let vertical = |line: &parley::Line<'_, blitz_dom::node::TextBrush>| {
            Some(
                crate::text::VerticalLine::new(line, sideways, all_upright)
                    .counterclockwise(wm.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR)),
            )
        };
        // pyro: one positioned inline box's pass: its fragments, then its text.
        let paint_pass = |scene: &mut _, pass: Option<NodeId>| {
            for fragment in self.inline_box_fragments(text_layout, Affine::IDENTITY, None, Some(&line_transform)) {
                if self.fragment_in_pass(fragment.node, Some(pass)) {
                    self.paint_inline_box_fragment(scene, &fragment, line_transform(fragment.line), None);
                }
            }
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            draw_text_context.positioned_pass = Some(pass);
            for (index, line) in text_layout.layout.lines().enumerate() {
                let turned = vertical(&line);
                crate::text::stroke_text(
                    scene,
                    std::iter::once(line),
                    self.context.dom,
                    line_transform(index),
                    TextPaintOptions { scale: self.scale, brush_override: None, shadows: true },
                    self.node.id,
                    Some(&text_layout.text),
                    &mut draw_text_context,
                    turned,
                    Some(&text_layout.layout),
                );
            }
            draw_text_context.positioned_pass = None;
        };
        // (The pass of a box left for its paint-tree entry: `draw_inline_pass`.)
        if let Some(span) = only_pass {
            paint_pass(scene, Some(span));
            return;
        }

        // pyro: positioned inline boxes paint after the in-flow content (`positioned_passes`; not
        // with highlights or truncated lines).
        let passes = match self.context.highlight_ranges.get(&self.node.id).is_none()
            && text_layout.ellipsis.is_none()
            && text_layout.clamp.is_none()
            && !wm.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR)
        {
            true => self.positioned_passes(text_layout),
            false => Vec::new(),
        };
        let main_pass = (!passes.is_empty()).then_some(None);

        // pyro: the boxes of inline elements, as on horizontal lines (`inline_box_fragments`), but
        // for `sideways-lr`, whose lines turn the other way (it keeps their background colours).
        if !wm.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR) {
            for fragment in self.inline_box_fragments(text_layout, Affine::IDENTITY, None, Some(&line_transform)) {
                if self.fragment_in_pass(fragment.node, main_pass) {
                    self.paint_inline_box_fragment(scene, &fragment, line_transform(fragment.line), None);
                }
            }
        } else if self.context.backgrounds {
            for (index, line) in text_layout.layout.lines().enumerate() {
                let vertical = vertical(&line);
                crate::text::draw_inline_backgrounds(
                    scene,
                    std::iter::once(line),
                    self.context.dom,
                    line_transform(index),
                    self.node.id,
                    vertical,
                );
            }
        }
        // pyro: highlighted as on horizontal lines (`highlight`: the `::selection` style, the
        // originating text's shadows under it), but in lines cut by an ellipsis; a recording
        // paint draws none. Upright text too: its glyphs keep their layout advances (parley
        // `shape::upright`; WPT css-pseudo `active-selection-031`).
        let highlight = self.context.highlight_ranges.get(&self.node.id).filter(|_| {
            text_layout.ellipsis.is_none()
                && text_layout.clamp.is_none()
                && self.context.draw_text_context.borrow().runs.is_none()
        });
        if let (Some(&(sel_start, sel_end)), None) = (self.context.selection_ranges.get(&self.node.id), highlight) {
            crate::text::draw_text_selection(
                scene,
                &text_layout.layout,
                &line_transform,
                sel_start,
                sel_end,
            );
        }
        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
        if let Some(ranges) = highlight {
            crate::highlight::stroke_highlighted_text(
                scene,
                self.context.dom,
                self.node.id,
                &text_layout.layout,
                &text_layout.text,
                &line_transform,
                None,
                TextPaintOptions { scale: self.scale, brush_override: None, shadows: true },
                &self.context.highlight_layers,
                ranges,
                &mut draw_text_context,
                vertical,
            );
        }
        // pyro: `text-overflow` and line clamps cut the lines at the content box's inline size
        // (its physical height), as on horizontal lines (`stroke_inline_text`).
        let truncates = text_layout.ellipsis.is_some() || text_layout.clamp.is_some();
        let inline_size = truncates.then(|| {
            let layout = self.context.dom.physical_unrounded_layout(self.node.id);
            let inset = layout.border.top + layout.border.bottom + layout.padding.top + layout.padding.bottom;
            (layout.size.height - inset).max(0.0) * self.scale as f32
        });
        draw_text_context.positioned_pass = main_pass;
        for (index, line) in text_layout.layout.lines().enumerate() {
            if text_layout.clamp.as_ref().is_some_and(|clamp| index >= clamp.lines as usize) {
                break;
            }
            let vertical = vertical(&line);
            let truncation = inline_size.and_then(|width| {
                line_truncation_at(text_layout, &line, index, self.scale as f32, width, wraps(self.node))
            });
            draw_text_context.cut = truncation.as_ref().map(|truncation| truncation.cut);
            if highlight.is_none() { crate::text::stroke_text(
                scene,
                std::iter::once(line.clone()),
                self.context.dom,
                line_transform(index),
                TextPaintOptions {
                    scale: self.scale,
                    brush_override: None,
                    shadows: true,
                },
                self.node.id,
                Some(&text_layout.text),
                &mut draw_text_context,
                vertical,
                Some(&text_layout.layout),
            ); }
            draw_text_context.cut = None;
            if let Some(LineTruncation { x, ellipsis, text, .. }) = truncation {
                let filter = draw_text_context.run_filter.take();
                let baseline = ellipsis.lines().next().map_or(0.0, |l| l.metrics().baseline);
                let offset = (x as f64, (line.metrics().baseline - baseline) as f64);
                for ellipsis_line in ellipsis.lines() {
                    let ellipsis_vertical = vertical.map(|_| {
                        crate::text::VerticalLine::new(
                            &ellipsis_line,
                            self.style.writing_mode.intersects(WritingMode::TEXT_SIDEWAYS | WritingMode::VERTICAL_SIDEWAYS),
                            self.style.writing_mode.contains(WritingMode::UPRIGHT),
                        )
                        .counterclockwise(self.style.writing_mode.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR))
                    });
                    crate::text::stroke_text(
                        scene,
                        std::iter::once(ellipsis_line),
                        self.context.dom,
                        line_transform(index) * Affine::translate(offset),
                        TextPaintOptions {
                            scale: self.scale,
                            brush_override: None,
                            shadows: true,
                        },
                        self.node.id,
                        Some(text),
                        &mut draw_text_context,
                        ellipsis_vertical,
                        None,
                    );
                }
                draw_text_context.run_filter = filter;
            }
            // pyro: ruby annotations and emphasis marks, turned with their line.
            if !text_layout.annotations.is_empty() {
                let recording = draw_text_context.runs.is_some();
                let line = text_layout.layout.get(index);
                if let Some(line) = line {
                    crate::text::draw_text_annotations(
                        scene,
                        &line,
                        &text_layout.annotations,
                        &text_layout.text,
                        self.context.dom,
                        line_transform(index),
                        self.scale,
                        recording,
                        vertical,
                        None,
                        None,
                    );
                }
            }
        }
        draw_text_context.positioned_pass = None;
        drop(draw_text_context);
        // pyro: then each positioned inline box: its fragments, then its text; a box listed in the
        // paint tree (`NodeFlags::INLINE_PASS_ENTRY`) is left for its entry there, as on horizontal
        // lines (`draw_inline_layout`).
        for &pass in passes.iter().skip(1) {
            if let Some(span) = pass
                && self
                    .context
                    .dom
                    .get_node(span)
                    .is_some_and(|node| node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY))
            {
                self.context.inline_pass_entries.borrow_mut().insert(span, vec![(self.node.id, self.transform, entry_pos, None)]);
                continue;
            }
            paint_pass(scene, pass);
        }
    }

    /// pyro: a text control in a vertical writing mode. The editor's layout is logical (laid out at
    /// the content box's inline size, `layout/mod.rs`): its value, placeholder, selection and caret
    /// are drawn through the turn of a vertical line (`draw_vertical_inline_layout`), lines running
    /// from the right in `vertical-rl` and from the left in `vertical-lr` with glyph tops to the
    /// right (`sideways-lr`: turned the other way, from the bottom), a single line centred in the
    /// block axis. A focused control's text is scrolled to its caret as a horizontal one's is
    /// (`TextInputData::scroll_offset`: the inline axis of a single line, the block axis of a
    /// textarea). Returns `false` when the control is horizontal.
    fn draw_vertical_text_input_text(&self, scene: &mut impl PaintScene, pos: Point, input_data: &TextInputData) -> bool {
        // (The control's own writing mode: a horizontal control in a vertical line is not turned.)
        let wm = self.style.writing_mode;
        if !wm.is_vertical() {
            return false;
        }
        let block_lr = wm.is_vertical_lr();
        let Some(layout) = input_data.editor.try_layout() else { return true };
        let s = self.scale;
        let unrounded = self.node.unrounded_layout();
        let content_width = unrounded.content_box_width() as f64;
        let content_height = unrounded.content_box_height() as f64;
        let scroll = self.node.scroll_offset();
        let left = pos.x + scroll.x + input_data.content_box_subpixel_offset.x as f64;
        let top = pos.y + scroll.y + input_data.content_box_subpixel_offset.y as f64;
        // The block-axis offset of the first line: a single line is centred.
        let centred = |height: f32| {
            if input_data.is_multiline { 0.0 } else { ((content_width - height as f64 / s) / 2.0).max(0.0) }
        };
        let block_offset = centred(layout.height());
        let sideways_lr = self.style.writing_mode.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR);
        // The editor's scroll, in its logical device pixels.
        let scroll_offset = if self.node.is_focussed() { input_data.scroll_offset as f64 * s } else { 0.0 };
        let (scroll_x, scroll_y) = if input_data.is_multiline { (0.0, scroll_offset) } else { (scroll_offset, 0.0) };
        // Logical (x inline, y block, device px) to physical device px, for shapes (a reflection in
        // `vertical-lr`, where only glyphs keep their tops to the right).
        let shapes = |block_offset: f64| -> Affine {
            let to_physical = if sideways_lr {
                Affine::new([0.0, -1.0, 1.0, 0.0, (left + block_offset) * s, (top + content_height) * s])
            } else if block_lr {
                Affine::new([0.0, 1.0, 1.0, 0.0, (left + block_offset) * s, top * s])
            } else {
                Affine::new([0.0, 1.0, -1.0, 0.0, (left + content_width - block_offset) * s, top * s])
            };
            self.transform * to_physical * Affine::translate((-scroll_x, -scroll_y))
        };
        // One line's glyphs.
        let line_transform = |metrics: &parley::layout::LineMetrics, block_offset: f64, scroll: (f64, f64)| -> Affine {
            let base = if sideways_lr {
                Affine::new([0.0, -1.0, 1.0, 0.0, (left + block_offset) * s, (top + content_height) * s])
            } else if block_lr {
                // The line flipped within its slot (whose scrolled position is mirrored with it).
                let flip = (metrics.block_min_coord + metrics.block_max_coord) as f64 - 2.0 * scroll.1;
                Affine::new([0.0, 1.0, -1.0, 0.0, (left + block_offset) * s + flip, top * s])
            } else {
                Affine::new([0.0, 1.0, -1.0, 0.0, (left + content_width - block_offset) * s, top * s])
            };
            self.transform * base * Affine::translate((-scroll.0, -scroll.1))
        };

        // An empty control shows its placeholder, unscrolled, under the caret.
        if let Some(placeholder) = input_data.shown_placeholder() {
            let block_offset = centred(placeholder.layout.height());
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            for line in placeholder.layout.lines() {
                crate::text::stroke_text(
                    scene,
                    std::iter::once(line.clone()),
                    self.context.dom,
                    line_transform(line.metrics(), block_offset, (0.0, 0.0)),
                    TextPaintOptions { scale: s, brush_override: Some(placeholder.color.as_srgb_color()), shadows: true },
                    self.node.id,
                    Some(&placeholder.text),
                    &mut draw_text_context,
                    None,
                    None,
                );
            }
        }

        if self.node.is_focussed() {
            let text_color = self.style.get_inherited_text().color.as_srgb_color();
            let (selection_background, _) =
                crate::highlight::control_selection_colors(self.context.dom, self.node.id, text_color);
            for (rect, _line_idx) in input_data.editor.selection_geometry().iter() {
                scene.fill(Fill::NonZero, shapes(block_offset), selection_background, None, &convert_rect(rect));
            }
            if let Some(cursor) = input_data.editor.cursor_geometry(1.5) {
                // (In its `caret-shape`, as a horizontal control's; a block caret's second colour
                // for the text under it is not drawn here.)
                let (rect, caret_color, _) = self.control_caret(input_data, &cursor);
                scene.fill(Fill::NonZero, shapes(block_offset), caret_color, None, &rect);
            }
        }

        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
        let options = TextPaintOptions { scale: s, brush_override: None, shadows: true };
        for line in layout.lines() {
            crate::text::stroke_text(
                scene,
                std::iter::once(line.clone()),
                self.context.dom,
                line_transform(line.metrics(), block_offset, (scroll_x, scroll_y)),
                options,
                self.node.id,
                None,
                &mut draw_text_context,
                None,
                None,
            );
        }
        true
    }

    /// pyro: a text control's caret, in the editor's (logical) coordinates: its rectangle for the
    /// control's `caret-shape`, its colour, and for a block caret over text the colour that text takes.
    fn control_caret(&self, input_data: &TextInputData, cursor: &parley::BoundingBox) -> (Rect, peniko::Color, Option<peniko::Color>) {
        let mut caret_text = None;
        let color = self.style.get_inherited_text().color;
        // pyro: the caret in its `caret-shape` (`caret.rs`): a block covers the
        // character after it (or is the width of `0`), towards the character's side
        // in right-to-left text, in a translucent text colour for `auto`, the text
        // under it in `caret-color`'s second colour; an underscore is a line under
        // that character.
        use style::values::computed::CaretShape;
        let shape = self.style.get_inherited_ui().caret_shape;
        let block = shape == CaretShape::Block;
        let caret_color = match &self.style.get_inherited_ui().caret_color.0 {
            ColorOrAuto::Auto if block => {
                let color = color.as_srgb_color();
                color.with_alpha(color.components[3] * 0.5)
            }
            ColorOrAuto::Auto => color.as_srgb_color(),
            ColorOrAuto::Color(caret_color) => caret_color.resolve_to_absolute(&color).as_srgb_color(),
        };
        let mut rect = convert_rect(&cursor);
        if matches!(shape, CaretShape::Block | CaretShape::Underscore)
            && input_data.editor.raw_selection().is_collapsed()
        {
            let index = input_data.editor.raw_selection().focus().index();
            let cluster = input_data
                .editor
                .try_layout()
                .and_then(|layout| parley::Cluster::from_byte_index(layout, index))
                .filter(|cluster| !cluster.is_hard_line_break());
            let width = cluster.as_ref().map_or_else(
                || self.context.dom.zero_advance(self.node.id) as f64 * self.scale,
                |cluster| cluster.advance() as f64,
            );
            let rtl = cluster.as_ref().map_or_else(
                || self.style.clone_direction() == style::computed_values::direction::T::Rtl,
                |cluster| cluster.is_rtl(),
            );
            // (The bar's left edge is the caret position.)
            let x = rect.x0;
            let (x0, x1) = if rtl { (x - width, x) } else { (x, x + width) };
            // (At whole device pixels, as a highlight's edges are.)
            let (x0, x1) = (x0.round(), x1.round());
            rect = match shape {
                CaretShape::Block => Rect::new(x0, rect.y0, x1, rect.y1),
                _ => {
                    let thickness = ((self.context.dom.zero_advance(self.node.id) as f64 / 5.0).max(1.0)) * self.scale;
                    Rect::new(x0, rect.y1 - thickness, x1, rect.y1)
                }
            };
            if block
                && let ColorOrAuto::Color(text) = &self.style.get_inherited_ui().caret_color.1
            {
                caret_text = Some(text.resolve_to_absolute(&color).as_srgb_color());
            }
        }

        (rect, caret_color, caret_text)
    }

    fn draw_text_input_text(&self, scene: &mut impl PaintScene, pos: Point) {
        // Render the text in text inputs
        if let Some(input_data) = self.text_input {
            if self.draw_vertical_text_input_text(scene, pos, input_data) {
                return;
            }
            // For single-line inputs, add an offset to vertically center the text input layout
            // within the content box of it's node.
            let y_offset = self.node.text_input_v_centering_offset(self.scale);
            let subpixel_offset = input_data.content_box_subpixel_offset;
            // pyro: the value, placeholder and caret at the content box's layout-unit x, as
            // inline text (`fractional_text_x_of_node`).
            // pyro: `pos` is the content box less the element's scroll offset, which this
            // element's transform (scrolled for its content) applies already: the offset is added
            // back, or `scrollLeft` moved a control's text twice (WPT css-overflow
            // `scrollable-overflow-input-001`: 80px for 40).
            let scroll = self.node.scroll_offset();
            let content_origin = Point {
                x: pos.x + scroll.x + subpixel_offset.x as f64 + self.fractional_text_x_of_node() / self.scale,
                y: pos.y + scroll.y + subpixel_offset.y as f64,
            };
            let pos = Point {
                x: content_origin.x,
                y: content_origin.y + y_offset,
            };

            // Apply the scroll offset (stored in CSS pixels, scaled here to device pixels) so
            // that the caret stays visible. Single-line inputs scroll horizontally; multi-line
            // inputs scroll vertically.
            // pyro: only in a focused control, which follows its caret; an unfocused one shows
            // its text where the element's own scroll offset (`scrollLeft`, applied with its
            // content) puts it, as Chromium: both moved its text twice (WPT css-overflow
            // `scrollable-overflow-input-001`: 80px for `scrollLeft` 40).
            let scroll_offset = if self.node.is_focussed() { input_data.scroll_offset as f64 * self.scale } else { 0.0 };
            let (scroll_x, scroll_y) = if input_data.is_multiline {
                (0.0, scroll_offset)
            } else {
                (scroll_offset, 0.0)
            };

            let transform = self.transform
                * Affine::translate((pos.x * self.scale - scroll_x, pos.y * self.scale - scroll_y));
            // pyro: each line's top at the whole CSS pixel Chromium paints it at. A single line
            // takes its selection and caret along; in a textarea they stay where the layout has them.
            let tops = self.text_placement(y_offset - scroll_y / self.scale).1;
            let single_line_shift = tops
                .filter(|_| !input_data.is_multiline)
                .zip(input_data.editor.try_layout().and_then(|layout| layout.lines().next()))
                .map(|(tops, line)| tops.shift(line.metrics()));
            let (transform, tops) = match single_line_shift {
                Some(shift) => (transform * Affine::translate((0.0, shift)), None),
                None => (transform, tops),
            };

            // pyro: an empty input shows its placeholder, its lines placed as the
            // value's (a single line centred in the content box), under the caret.
            if let Some(placeholder) = input_data.shown_placeholder() {
                let layout = self.node.final_layout();
                let box_width = layout.content_box_width() * self.scale as f32;
                let y = if input_data.is_multiline {
                    0.0
                } else {
                    let height = placeholder.layout.height() / self.scale as f32;
                    ((layout.content_box_height() - height) / 2.0).max(0.0) as f64
                };
                let origin = Point {
                    x: content_origin.x,
                    y: content_origin.y + y,
                };
                let mut draw_text_context = self.context.draw_text_context.borrow_mut();
                // pyro: its lines at whole CSS pixels, as the value's.
                draw_text_context.line_tops = self.text_placement(y).1;
                for line in placeholder.layout.lines() {
                    // A wrapped (textarea) placeholder is aligned by its layout.
                    let x = if input_data.is_multiline {
                        0.0
                    } else {
                        placeholder.line_start(line.metrics().advance, box_width) as f64
                    };
                    let transform = self.transform
                        * Affine::translate((origin.x * self.scale + x, origin.y * self.scale));
                    crate::text::stroke_text(
                        scene,
                        std::iter::once(line),
                        self.context.dom,
                        transform,
                        TextPaintOptions {
                            scale: self.scale,
                            brush_override: Some(placeholder.color.as_srgb_color()),
                            shadows: true,
                        },
                        self.node.id,
                        Some(&placeholder.text),
                        &mut draw_text_context,
                        None,
                        None,
                    );
                }
                draw_text_context.line_tops = None;
            }

            // pyro: the text a block caret covers, in `caret-color`'s second colour, and the block.
            let mut caret_text: Option<(Color, Rect)> = None;
            if self.node.is_focussed() {
                // Render selection/caret
                // pyro: in the control's `::selection` colours (`control_selection_colors`).
                let text_color = self.style.get_inherited_text().color.as_srgb_color();
                let (selection_background, _) =
                    crate::highlight::control_selection_colors(self.context.dom, self.node.id, text_color);
                for (rect, _line_idx) in input_data.editor.selection_geometry().iter() {
                    scene.fill(
                        Fill::NonZero,
                        transform,
                        selection_background,
                        None,
                        &convert_rect(rect),
                    );
                }
                if let Some(cursor) = input_data.editor.cursor_geometry(1.5) {
                    let (rect, caret_color, text) = self.control_caret(input_data, &cursor);
                    if let Some(text) = text {
                        caret_text = Some((text, rect));
                    }
                    scene.fill(Fill::NonZero, transform, caret_color, None, &rect);
                };
            }

            // Render text
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            draw_text_context.line_tops = tops;
            let options = TextPaintOptions { scale: self.scale, brush_override: None, shadows: true };
            let layout = input_data.editor.try_layout().unwrap();
            // pyro: `text-overflow` in a text control, as Chromium has it: while the control
            // is not focused and not scrolled, each line that overflows its content box ends
            // with the ellipsis (a focused control shows its text clipped, to edit it).
            let ellipsis = input_data
                .ellipsis
                .as_ref()
                .filter(|_| {
                    let scroll = self.node.scroll_offset();
                    !self.node.is_focussed() && input_data.scroll_offset == 0.0 && scroll.x == 0.0 && scroll.y == 0.0
                });
            // pyro: selected text in the `::selection` colour: the text outside the selection
            // first, then inside it (`control_selection_colors`).
            let selected = self
                .node
                .is_focussed()
                .then(|| {
                    let text_color = self.style.get_inherited_text().color.as_srgb_color();
                    crate::highlight::control_selection_colors(self.context.dom, self.node.id, text_color).1
                })
                .flatten()
                .map(|color| {
                    let rects: Vec<Rect> =
                        input_data.editor.selection_geometry().iter().map(|(rect, _)| convert_rect(rect)).collect();
                    (color, rects)
                })
                .filter(|(_, rects)| !rects.is_empty());
            if let Some((_, rects)) = &selected {
                const FAR: f64 = 1e6;
                let mut outside = kurbo::BezPath::new();
                outside.extend(Rect::new(-FAR, -FAR, FAR, FAR).path_elements(0.1));
                for rect in rects {
                    outside.extend(rect.path_elements(0.1));
                }
                scene.push_clip_layer(Fill::EvenOdd, transform, &outside);
            }
            match ellipsis {
                None => crate::text::stroke_text(
                    scene,
                    layout.lines(),
                    self.context.dom,
                    transform,
                    options,
                    self.node.id,
                    None,
                    &mut draw_text_context,
                    None,
                    None,
                ),
                Some((ellipsis, text)) => {
                    let width = self.node.unrounded_layout().content_box_width() * self.scale as f32;
                    for line in layout.lines() {
                        let truncation = truncate_line(&line, ellipsis, width, self.scale as f32, layout.is_rtl(), false, 0.0);
                        draw_text_context.cut = truncation.map(|(cut, _)| cut);
                        crate::text::stroke_text(
                            scene,
                            std::iter::once(line.clone()),
                            self.context.dom,
                            transform,
                            options,
                            self.node.id,
                            None,
                            &mut draw_text_context,
                            None,
                            None,
                        );
                        draw_text_context.cut = None;
                        if let Some((_, x)) = truncation {
                            let baseline = ellipsis.lines().next().map_or(0.0, |l| l.metrics().baseline);
                            let shift = tops.map_or(0.0, |tops| tops.shift(line.metrics()));
                            let offset = (x as f64, (line.metrics().baseline - baseline) as f64 + shift);
                            draw_text_context.line_tops = None;
                            crate::text::stroke_text(
                                scene,
                                ellipsis.lines(),
                                self.context.dom,
                                transform * Affine::translate(offset),
                                options,
                                self.node.id,
                                Some(text),
                                &mut draw_text_context,
                                None,
                                None,
                            );
                            draw_text_context.line_tops = tops;
                        }
                    }
                }
            }
            if let Some((color, rects)) = selected {
                scene.pop_layer();
                let mut inside = kurbo::BezPath::new();
                for rect in &rects {
                    inside.extend(rect.path_elements(0.1));
                }
                scene.push_clip_layer(Fill::NonZero, transform, &inside);
                crate::text::stroke_text(
                    scene,
                    layout.lines(),
                    self.context.dom,
                    transform,
                    TextPaintOptions { scale: self.scale, brush_override: Some(color), shadows: false },
                    self.node.id,
                    None,
                    &mut draw_text_context,
                    None,
                    None,
                );
                scene.pop_layer();
            }
            if let Some((color, rect)) = caret_text {
                scene.push_clip_layer(Fill::NonZero, transform, &rect);
                crate::text::stroke_text(
                    scene,
                    layout.lines(),
                    self.context.dom,
                    transform,
                    TextPaintOptions { scale: self.scale, brush_override: Some(color), shadows: false },
                    self.node.id,
                    None,
                    &mut draw_text_context,
                    None,
                    None,
                );
                scene.pop_layer();
            }
            draw_text_context.line_tops = None;
        }
    }

    /// pyro: an inside marker that is an image (`list-style-image`): at the start of the
    /// item's first line, its bottom on the baseline (the inline layout left room for it).
    fn draw_inside_marker_image(&self, scene: &mut impl PaintScene, pos: Point, width: f32, height: f32) {
        let image = self.element.list_style_images.first().and_then(Option::as_ref);
        if image.is_none() && !self.generated_marker() {
            return;
        }
        let Some(text_layout) = self.element.inline_layout_data.as_ref() else { return };
        let Some(line) = text_layout.layout.lines().next() else { return };
        let metrics = line.metrics();
        let layout_scale = text_layout.layout.scale();
        let rtl = text_layout.layout.is_rtl();
        let line_x = if rtl { metrics.inline_max_coord / layout_scale - width } else { metrics.inline_min_coord / layout_scale };
        let x = pos.x + f64::from(line_x);
        let y = pos.y + f64::from(text_layout.block_offset + metrics.baseline / layout_scale - height);
        let origin = Affine::translate(((x * self.scale).round(), (y * self.scale).round()));
        let Some(image) = image else {
            self.draw_marker_gradient(scene, origin, width, height);
            return;
        };
        match &image.image {
            blitz_dom::node::ImageData::Raster(raster) if raster.width > 0 && raster.height > 0 => {
                let quality = to_image_quality(self.style.clone_image_rendering());
                // pyro: `image-orientation: none` turns the decoded pixels back (as `<img>`).
                let unoriented = blitz_dom::util::image_unoriented(&self.style) && raster.orientation != 1;
                let (shown_width, shown_height) = raster.shown_size(unoriented);
                let size = Affine::scale_non_uniform(
                    f64::from(width) * self.scale / f64::from(shown_width),
                    f64::from(height) * self.scale / f64::from(shown_height),
                );
                let unturn = if unoriented { Affine::new(raster.unturn()) } else { Affine::IDENTITY };
                scene.draw_image(to_peniko_image(raster, quality).as_ref(), self.transform * origin * size * unturn);
            }
            #[cfg(feature = "svg")]
            blitz_dom::node::ImageData::Svg(svg) => {
                // pyro: drawn into the marker box (a sizeless image gets the default square), the
                // root's background filling it as a browser's canvas.
                let base = self.transform * origin * Affine::scale(self.scale);
                if let Some(color) = svg.intrinsic_dimensions.root_background {
                    let color = peniko::Color::from_rgba8(color.red, color.green, color.blue, color.alpha);
                    let rect = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
                    scene.fill(peniko::Fill::NonZero, base, color, None, &rect);
                }
                let viewport = svg.viewport_transform(width, height);
                let viewport = Affine::new([viewport.sx, viewport.ky, viewport.kx, viewport.sy, viewport.tx, viewport.ty].map(f64::from));
                anyrender_svg::render_svg_tree(scene, &svg.tree, base * viewport);
            }
            _ => {}
        }
    }

    fn draw_marker(&self, scene: &mut impl PaintScene, pos: Point) {
        if let Some(ListItemLayout {
            marker: Marker::Image { width, height },
            position: ListItemLayoutPosition::Inside,
            ..
        }) = self.list_item
        {
            self.draw_inside_marker_image(scene, pos, *width, *height);
            return;
        }
        if let Some(ListItemLayout {
            marker,
            position: ListItemLayoutPosition::Outside(layout),
            content_box_subpixel_offset,
            ..
        }) = self.list_item
        {
            // Right align and pad the bullet when rendering outside
            let x_padding = match marker {
                Marker::Char(_) => 8.0,
                Marker::String(_) | Marker::Image { .. } => 0.0,
            };
            // Outside markers are placed outside the list item's border box
            // (`pos` is the origin of its content box)
            let item_layout = self.node.final_layout();
            // pyro: after its right edge in a right-to-left item.
            let rtl = !self.style.writing_mode.is_bidi_ltr();
            let x_offset = if rtl {
                item_layout.size.width - item_layout.padding.left - item_layout.border.left + x_padding
            } else {
                -(layout.full_width() / layout.scale()
                    + x_padding
                    + item_layout.padding.left
                    + item_layout.border.left)
            };

            // Align the marker with the baseline of the first line of text in the list item
            if let Marker::Char(symbol @ ('•' | '◦' | '▪')) = marker {
                self.draw_marker_symbol(scene, pos, *symbol, layout);
                return;
            }
            if let Marker::Image { width, height } = marker {
                self.draw_marker_image(scene, pos, *width, *height, layout);
                return;
            }
            if self.draw_combined_outside_marker(scene, pos, layout) {
                return;
            }
            let first_line = self
                .element
                .inline_layout_data
                .as_ref()
                .and_then(|text_layout| Some((text_layout, text_layout.layout.lines().next()?)));
            // pyro: the marker goes with the line, which floats beside the item move.
            let line_x = first_line.as_ref().filter(|_| !rtl).map_or(0.0, |(text_layout, line)| {
                marker_line_x(line) / text_layout.layout.scale()
            });
            let y_offset = if first_line.is_none() {
                // pyro: no line of its own (its first line is in a block inside it, an anonymous
                // one beside a nested list): on the baseline of that line, with its trim shift
                // (WPT css-inline `text-box-trim-list-001`).
                let own = layout.lines().next().map_or(0.0, |line| line.metrics().baseline) / layout.scale();
                self.marker_baseline(layout).0 - own
            } else if let Some((text_layout, first_text_line)) = &first_line
            {
                // pyro: with the line, where it is painted (`LineTops`).
                let shift = self
                    .text_placement(text_layout.block_offset as f64)
                    .1
                    .map_or(0.0, |tops| tops.shift(first_text_line.metrics()));
                (first_text_line.metrics().baseline
                    - layout.lines().next().unwrap().metrics().baseline
                    + shift as f32)
                    / layout.scale()
                    + text_layout.block_offset
            } else {
                0.0
            };

            let pos = Point {
                x: pos.x + (content_box_subpixel_offset.x + x_offset + line_x) as f64,
                y: pos.y + (content_box_subpixel_offset.y + y_offset) as f64,
            };

            let transform =
                self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));

            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            crate::text::stroke_text(
                scene,
                layout.lines(),
                self.context.dom,
                transform,
                TextPaintOptions {
                    scale: self.scale,
                    brush_override: None,
                    shadows: true,
                },
                self.node.id,
                None,
                &mut draw_text_context,
                None,
                None,
            );
            // pyro: its emphasis marks, as over the item's text.
            if let Some(item) = self.list_item.filter(|item| !item.annotations.is_empty()) {
                let text = match marker {
                    Marker::String(text) => text.clone(),
                    Marker::Char(c) => c.to_string(),
                    Marker::Image { .. } => String::new(),
                };
                let recording = draw_text_context.runs.is_some();
                for line in layout.lines() {
                    crate::text::draw_text_annotations(
                        scene,
                        &line,
                        &item.annotations,
                        &text,
                        self.context.dom,
                        transform,
                        self.scale,
                        recording,
                        None,
                        None,
                        None,
                    );
                }
            }
        }
    }

    /// pyro: an outside marker whose `::marker` style combines it (`text-combine-upright: all`) in
    /// the item's vertical lines: upright in a box one em square (its text squeezed to fit, as
    /// combined text is), before the start of the item's first line and its inline-start padding
    /// and border, on the line's baseline (WPT css-pseudo `marker-text-combine-upright*`). `false`
    /// when the marker is not such a one.
    fn draw_combined_outside_marker(
        &self,
        scene: &mut impl PaintScene,
        pos: Point,
        layout: &parley::Layout<blitz_dom::node::TextBrush>,
    ) -> bool {
        use style::values::computed::TextCombineUpright;
        let Some(block_lr) = self.node.vertical_block_axis() else { return false };
        if self.style.writing_mode.is_sideways() {
            return false;
        }
        let Some(style_node) = self.list_item.and_then(|item| item.style_node) else { return false };
        let Some(marker_style) = self.context.dom.get_node(style_node).and_then(|node| node.primary_styles()) else {
            return false;
        };
        if marker_style.get_inherited_text().text_combine_upright != TextCombineUpright::All {
            return false;
        }
        let Some(text_layout) = self.element.inline_layout_data.as_ref() else { return false };
        let Some(line) = text_layout.layout.lines().next() else { return false };
        let Some(marker_line) = layout.lines().next() else { return false };
        // In the item's line space (device px along the line, then across it).
        let scale = f64::from(text_layout.layout.scale());
        let em = f64::from(marker_style.get_font().font_size.used_size.0.px()) * scale;
        let item = self.node.final_layout();
        let inline_inset = f64::from(item.padding.top + item.border.top) * scale;
        // As an inline-block of combined text on the line: one em tall, its text's line (the
        // marker's) centred in it, on the line's baseline.
        let metrics = line.metrics();
        let marker_metrics = marker_line.metrics();
        let marker_height = f64::from(marker_metrics.block_max_coord - marker_metrics.block_min_coord);
        let marker_ascent = f64::from(marker_metrics.baseline - marker_metrics.block_min_coord);
        let top = f64::from(metrics.baseline) - (em - marker_height) / 2.0 - marker_ascent;
        let start = f64::from(marker_line_x(&line)) - inline_inset - em;
        let in_line = Rect::new(start, top, start + em, top + em);
        let line_transform = self.vertical_line_transform(text_layout, pos, block_lr)(0);
        let local = (self.transform.inverse() * line_transform).transform_rect_bbox(in_line);
        // The marker's text, upright, squeezed to the em and centred in the box.
        // (With its trailing space: Blink's outside markers keep their white space.)
        let width = f64::from(marker_metrics.advance.max(layout.full_width()));
        let squeeze = if width > em { em / width } else { 1.0 };
        let line_height = marker_height;
        let x = local.x0 + (local.width() - width * squeeze) / 2.0;
        let y = local.y0 + (local.height() - line_height) / 2.0 - f64::from(marker_metrics.block_min_coord);
        let transform = self.transform * Affine::translate((x, y)) * Affine::scale_non_uniform(squeeze, 1.0);
        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
        crate::text::stroke_text(
            scene,
            layout.lines(),
            self.context.dom,
            transform,
            TextPaintOptions { scale: self.scale, brush_override: None, shadows: true },
            self.node.id,
            None,
            &mut draw_text_context,
            None,
            None,
        );
        true
    }

    /// pyro: an outside `disc`, `circle` or `square` marker, as Chromium paints one: a shape
    /// sized from the item's font (Blink's `ListMarker::RelativeSymbolMarkerRect`), not a glyph
    /// of a bullet font. With `a` the font's ascent in whole pixels and `o = a * 2 / 3` in
    /// whole numbers, the shape is `(o + 1) / 2` px wide and high, starts `o + 7` px before
    /// the item's border box and `a - 3 * (a - o) / 2` px above the baseline of the item's
    /// first line, and is at whole CSS pixels. A circle is a 1px line centred on the shape's
    /// edge. The first line is the item's own or that of the first box in it that has one; the
    /// font is the item's, which the marker's layout is a line of (blitz-dom).
    fn draw_marker_symbol(
        &self,
        scene: &mut impl PaintScene,
        pos: Point,
        symbol: char,
        marker_layout: &parley::Layout<blitz_dom::node::TextBrush>,
    ) {
        let font_ascent = |line: &parley::Line<'_, blitz_dom::node::TextBrush>| {
            line.items().find_map(|item| match item {
                parley::PositionedLayoutItem::GlyphRun(run) => Some(run.run().font_metrics().ascent),
                _ => None,
            })
        };
        let Some(marker_line) = marker_layout.lines().next() else { return };
        let layout_scale = marker_layout.scale();
        let item_layout = self.node.final_layout();
        let (baseline, line_x) = self.marker_baseline(marker_layout);
        let ascent = (font_ascent(&marker_line).unwrap_or(0.0) / layout_scale).round() as i32;
        let offset = ascent * 2 / 3;
        let size = ((offset + 1) / 2) as f64;
        let subpixel = self.list_item.map_or(taffy::Point::ZERO, |item| item.content_box_subpixel_offset);
        // pyro: mirrored after the border box's right edge in a right-to-left item.
        let x = if self.style.writing_mode.is_bidi_ltr() {
            pos.x + (subpixel.x + line_x - item_layout.padding.left - item_layout.border.left) as f64
                - (offset + 7) as f64
        } else {
            pos.x + (subpixel.x + item_layout.size.width - item_layout.padding.left - item_layout.border.left) as f64
                + (offset + 7) as f64
                - size
        };
        let y = pos.y + (subpixel.y + baseline) as f64 - (ascent - 3 * (ascent - offset) / 2) as f64;
        let (x, y) = ((x + 0.5).floor() * self.scale, (y + 0.5).floor() * self.scale);
        let rect = Rect::new(x, y, x + size * self.scale, y + size * self.scale);
        // pyro: in the colour of the item's `::marker` style, when a rule styles the marker.
        let styled = self.list_item.and_then(|item| item.style_node).unwrap_or(self.node.id);
        let paint = anyrender::Paint::from(crate::text::text_fill_color(self.context.dom, styled));
        match symbol {
            '•' => scene.fill(Fill::NonZero, self.transform, &paint, None, &kurbo::Ellipse::from_rect(rect)),
            '◦' => {
                scene.stroke(&Stroke::new(self.scale), self.transform, &paint, None, &kurbo::Ellipse::from_rect(rect))
            }
            _ => scene.fill(Fill::NonZero, self.transform, &paint, None, &rect),
        }
    }

    /// pyro: the baseline an outside marker sits on, in CSS px from the top of the list item's
    /// content box: that of the item's first line, the first line of the first box in it that has
    /// one, or, when there is none, that of the marker's own line (a line of the item's font).
    /// With it, how far floats move that line's start (the marker goes with the line).
    fn marker_baseline(&self, marker_layout: &parley::Layout<blitz_dom::node::TextBrush>) -> (f32, f32) {
        let layout_scale = marker_layout.scale();
        let item_layout = self.node.final_layout();
        // The first line, and its box's border box top from the item's.
        let (mut node, mut top) = (self.node, 0.0);
        let mut first_line = None;
        for _ in 0..8 {
            let text = node.data.downcast_element().and_then(|element| element.inline_layout_data.as_ref());
            if let Some((text, line)) = text.and_then(|text| Some((text, text.layout.lines().next()?))) {
                first_line = Some((node, text, line));
                break;
            }
            let children = node.layout_children.borrow();
            let child = children.as_ref().and_then(|children| {
                children.iter().filter_map(|id| self.context.dom.get_node(*id)).find(|child| {
                    child.final_layout().size.height > 0.0
                        && !child.primary_styles().is_some_and(|s| s.clone_position().is_absolutely_positioned())
                })
            });
            let Some(child) = child else { break };
            top += child.final_layout().location.y;
            node = child;
        }
        match &first_line {
            Some((node, text, line)) => {
                let shift = self
                    .text_placement_of(node, text.block_offset as f64)
                    .1
                    .map_or(0.0, |tops| tops.shift(line.metrics()));
                let layout = node.final_layout();
                let content_top = top + layout.border.top + layout.padding.top
                    - item_layout.border.top
                    - item_layout.padding.top;
                let line_scale = text.layout.scale();
                (
                    content_top + (line.metrics().baseline + shift as f32) / layout_scale + text.block_offset,
                    marker_line_x(line) / line_scale,
                )
            }
            None => (marker_layout.lines().next().map_or(0.0, |line| line.metrics().baseline / layout_scale), 0.0),
        }
    }

    /// pyro: an outside `list-style-image` marker, as Chromium places one
    /// (`ListMarker::InlineMarginsForOutside`): the image at its natural size, ending 7px before
    /// the item's border box, its bottom on the baseline of the item's first line (an inline
    /// replaced box on the marker's line), at whole device pixels.
    /// pyro: a gradient `list-style-image` (no natural size: `layout::list` makes the marker a
    /// square of half the font's ascent, as Blink).
    fn marker_gradient(&self) -> Option<&style::values::computed::Gradient> {
        match &self.style.get_list().list_style_image {
            style::values::computed::Image::Gradient(gradient) => Some(gradient),
            _ => None,
        }
    }

    /// pyro: whether `list-style-image` is generated (a gradient, or `image(<color>)`, to which
    /// `light-dark(none, none)` computes): a marker of the default size (`layout::list`).
    fn generated_marker(&self) -> bool {
        matches!(
            self.style.get_list().list_style_image,
            style::values::computed::Image::Gradient(_) | style::values::computed::Image::Image(_)
        )
    }

    fn draw_marker_gradient(&self, scene: &mut impl PaintScene, origin: Affine, width: f32, height: f32) {
        let rect = Rect::new(0.0, 0.0, f64::from(width) * self.scale, f64::from(height) * self.scale);
        if let style::values::computed::Image::Image(color) = &self.style.get_list().list_style_image {
            let color = color.resolve_to_absolute(&self.style.clone_color()).as_srgb_color();
            scene.fill(peniko::Fill::NonZero, self.transform * origin, color, None, &rect);
            return;
        }
        let Some(gradient) = self.marker_gradient() else { return };
        let (gradient, brush_transform) =
            crate::gradient::to_peniko_gradient(gradient, rect, rect, self.scale, &self.style.clone_color());
        scene.fill(peniko::Fill::NonZero, self.transform * origin, &gradient, brush_transform, &rect);
    }

    fn draw_marker_image(
        &self,
        scene: &mut impl PaintScene,
        pos: Point,
        width: f32,
        height: f32,
        marker_layout: &parley::Layout<blitz_dom::node::TextBrush>,
    ) {
        let image = self.element.list_style_images.first().and_then(Option::as_ref);
        if image.is_none() && !self.generated_marker() {
            return;
        }
        let (baseline, line_x) = self.marker_baseline(marker_layout);
        let item_layout = self.node.final_layout();
        let subpixel = self.list_item.map_or(taffy::Point::ZERO, |item| item.content_box_subpixel_offset);
        // The room after the image (`ListItemLayout::image_gap`).
        let gap = self.list_item.map_or(7.0, |item| item.image_gap);
        let mut x = pos.x + (subpixel.x + line_x - item_layout.padding.left - item_layout.border.left) as f64
            - f64::from(width + gap);
        let mut y = pos.y + f64::from(subpixel.y + baseline - height);
        // pyro: in a vertical writing mode the marker goes before the item's inline-start edge
        // (its top, or its bottom where the inline axis runs upward), centred on its first line.
        if let Some(block_lr) = self.node.vertical_block_axis() {
            let content_width = f64::from(item_layout.content_box_width());
            let content_height = f64::from(item_layout.content_box_height());
            let center = self
                .element
                .inline_layout_data
                .as_ref()
                .and_then(|text| {
                    let line = text.layout.lines().next()?;
                    let metrics = line.metrics();
                    Some(f64::from(text.block_offset) + f64::from(metrics.block_min_coord + metrics.block_max_coord) / 2.0 / f64::from(text.layout.scale()))
                })
                .unwrap_or(0.0);
            // (The inline start is the bottom where the inline axis runs upward: `sideways-lr`, or
            // right-to-left `vertical-*`.)
            let start_at_bottom = !self.style.writing_mode.is_inline_tb();
            let block = if block_lr { center } else { content_width - center };
            x = pos.x + block - f64::from(width) / 2.0;
            y = if start_at_bottom {
                pos.y + content_height + f64::from(item_layout.padding.bottom + item_layout.border.bottom) + f64::from(gap)
            } else {
                pos.y - f64::from(item_layout.padding.top + item_layout.border.top + height + gap)
            };
        }
        let origin = Affine::translate(((x * self.scale).round(), (y * self.scale).round()));
        let Some(image) = image else {
            self.draw_marker_gradient(scene, origin, width, height);
            return;
        };
        match &image.image {
            blitz_dom::node::ImageData::Raster(raster) if raster.width > 0 && raster.height > 0 => {
                let quality = to_image_quality(self.style.clone_image_rendering());
                // pyro: `image-orientation: none` turns the decoded pixels back (as `<img>`).
                let unoriented = blitz_dom::util::image_unoriented(&self.style) && raster.orientation != 1;
                let (shown_width, shown_height) = raster.shown_size(unoriented);
                let size = Affine::scale_non_uniform(
                    f64::from(width) * self.scale / f64::from(shown_width),
                    f64::from(height) * self.scale / f64::from(shown_height),
                );
                let unturn = if unoriented { Affine::new(raster.unturn()) } else { Affine::IDENTITY };
                scene.draw_image(to_peniko_image(raster, quality).as_ref(), self.transform * origin * size * unturn);
            }
            #[cfg(feature = "svg")]
            blitz_dom::node::ImageData::Svg(svg) => {
                // pyro: drawn into the marker box (a sizeless image gets the default square), the
                // root's background filling it as a browser's canvas.
                let base = self.transform * origin * Affine::scale(self.scale);
                if let Some(color) = svg.intrinsic_dimensions.root_background {
                    let color = peniko::Color::from_rgba8(color.red, color.green, color.blue, color.alpha);
                    let rect = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
                    scene.fill(peniko::Fill::NonZero, base, color, None, &rect);
                }
                let viewport = svg.viewport_transform(width, height);
                let viewport = Affine::new([viewport.sx, viewport.ky, viewport.kx, viewport.sy, viewport.tx, viewport.ty].map(f64::from));
                anyrender_svg::render_svg_tree(scene, &svg.tree, base * viewport);
            }
            _ => {}
        }
    }

    fn draw_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // Negative z_index hoisted nodes
        self.draw_neg_z_children(scene, parent_style_transform, clip_rect);
        self.draw_children_above(scene, parent_style_transform, clip_rect);
    }

    /// pyro: CSS 2.1 Appendix E paints a line's inline-level boxes in tree order: the background
    /// and borders of an inline box that comes after an atomic inline (an inline-block, a form
    /// control) go over it where they overlap (a label pulled back over its checkbox by a
    /// negative margin). The inline content paints its boxes before the atomic inlines, which
    /// paint after it (`FlowPhase::Foreground`): those later boxes are painted again over them,
    /// clipped to where they meet.
    fn draw_inline_boxes_over_atomics(&self, scene: &mut impl PaintScene, pos: Point) {
        if !self.node.flags.is_inline_root() || !self.context.backgrounds {
            return;
        }
        let Some(text_layout) = self.element.inline_layout_data.as_ref() else { return };
        // The atomic inlines of each line, in layout space. Most lines have none: nothing more
        // is done for them.
        let line_count = text_layout.layout.len();
        let mut atomics: Vec<Vec<(NodeId, Rect)>> = Vec::new();
        for (index, line) in text_layout.layout.lines().enumerate() {
            for item in line.items() {
                if let parley::PositionedLayoutItem::InlineBox(ibox) = item
                    && ibox.kind == parley::InlineBoxKind::InFlow
                {
                    let rect = Rect::new(
                        ibox.x as f64,
                        ibox.y as f64,
                        (ibox.x + ibox.width) as f64,
                        (ibox.y + ibox.height) as f64,
                    );
                    if atomics.is_empty() {
                        atomics.resize_with(line_count.max(index + 1), Vec::new);
                    }
                    atomics[index].push((NodeId::from_u64(ibox.id), rect));
                }
            }
        }
        if atomics.is_empty() {
            return;
        }
        // Tree order of the root's inline content, numbered once (no child list is searched per
        // pair): an atomic inline comes before an inline box where its number is the smaller.
        let order = inline_tree_order(self.context.dom, self.node.id);
        // `vertical`: the lines run along the device y axis.
        let paint = |scene: &mut _, fragments: Vec<InlineBoxFragment>, line_transform: &dyn Fn(usize) -> Affine, horizontal: Option<(Affine, Option<crate::text::LineTops>)>, vertical: bool| {
            let along = |rect: Rect| if vertical { (rect.y0, rect.y1) } else { (rect.x0, rect.x1) };
            // Per line with atomics: their device rects in tree order numbers, sorted by where
            // they start along the line, with the furthest end so far (to stop a search early).
            struct LineAtomics {
                atomics: Vec<(f64, f64, usize, Rect)>,
                max_end: Vec<f64>,
            }
            let mut lines: Vec<Option<LineAtomics>> = Vec::new();
            lines.resize_with(atomics.len(), || None);
            for fragment in &fragments {
                let Some(&fragment_order) = order.get(&fragment.node) else { continue };
                let Some(line_atomics) = atomics.get(fragment.line).filter(|list| !list.is_empty()) else { continue };
                let line = lines[fragment.line].get_or_insert_with(|| {
                    let transform = line_transform(fragment.line);
                    let mut list: Vec<(f64, f64, usize, Rect)> = line_atomics
                        .iter()
                        .filter_map(|&(atomic, rect)| {
                            let atomic_order = *order.get(&atomic)?;
                            let rect = transform.transform_rect_bbox(rect);
                            let (start, end) = along(rect);
                            Some((start, end, atomic_order, rect))
                        })
                        .collect();
                    list.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let mut furthest = f64::NEG_INFINITY;
                    let max_end = list.iter().map(|a| { furthest = furthest.max(a.1); furthest }).collect();
                    LineAtomics { atomics: list, max_end }
                });
                let fragment_rect = fragment.transform.transform_rect_bbox(Rect::from_origin_size((0.0, 0.0), fragment.size));
                let (start, end) = along(fragment_rect);
                // The atomics starting before the fragment ends, back to the last that may still
                // reach into it; in tree order, as the line paints them.
                let upto = line.atomics.partition_point(|a| a.0 < end);
                let mut from = upto;
                while from > 0 && line.max_end[from - 1] > start {
                    from -= 1;
                }
                let mut hits: Vec<(usize, Rect)> = line.atomics[from..upto]
                    .iter()
                    .filter(|a| a.1 > start && a.2 < fragment_order)
                    .filter_map(|a| {
                        let meet = fragment_rect.intersect(a.3);
                        (meet.width() > 0.0 && meet.height() > 0.0).then_some((a.2, meet))
                    })
                    .collect();
                hits.sort_by_key(|hit| hit.0);
                for (_, meet) in hits {
                    PaintScene::push_clip_layer(scene, Fill::NonZero, Affine::IDENTITY, &meet);
                    match horizontal {
                        Some((transform, tops)) => self.paint_inline_box_fragment(scene, fragment, transform, tops),
                        None => self.paint_inline_box_fragment(scene, fragment, line_transform(fragment.line), None),
                    }
                    PaintScene::pop_layer(scene);
                }
            }
        };
        if let Some(block_lr) = self.node.vertical_block_axis() {
            if self.style.writing_mode.contains(WritingMode::WRITING_MODE_SIDEWAYS_LR) {
                return;
            }
            let line_transform = self.vertical_line_transform(text_layout, pos, block_lr);
            let fragments = self.inline_box_fragments(text_layout, Affine::IDENTITY, None, Some(&line_transform));
            paint(scene, fragments, &line_transform, None, true);
        } else {
            let subpixel_offset = text_layout.content_box_subpixel_offset;
            let pos = Point {
                x: pos.x + subpixel_offset.x as f64,
                y: pos.y + subpixel_offset.y as f64 + text_layout.block_offset as f64,
            };
            let transform = self.transform * Affine::translate((pos.x * self.scale, pos.y * self.scale));
            let (_, tops) = self.text_placement(text_layout.block_offset as f64);
            let fragments = self.inline_box_fragments(text_layout, transform, tops, None);
            paint(scene, fragments, &|_| transform, Some((transform, tops)), false);
        }
    }

    /// pyro: [`Self::draw_children`] without the stacking contexts of negative z-index.
    fn draw_children_above(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // Regular children
        if let Some(children) = &*self.node.paint_children.borrow() {
            for child_id in children {
                self.draw_paint_child(scene, *child_id, parent_style_transform, clip_rect);
            }
        }

        self.draw_pos_z_children(scene, parent_style_transform, clip_rect);
    }

    /// pyro: see [`escapes_clip_of`].
    fn escapes_own_clip(&self, child_id: NodeId) -> bool {
        escapes_clip_of(self.context.dom.as_ref().tree(), self.clip_owner(), child_id)
    }

    /// pyro: see [`has_clip_escapes`].
    fn has_clip_escapes(&self) -> bool {
        has_clip_escapes_of(self.context.dom.as_ref().tree(), self.node, self.clip_owner())
    }

    /// pyro: the box whose clip this element applies: itself, or the box that handed it its clip
    /// (`BlitzDomPainter::escape_owner`).
    fn clip_owner(&self) -> NodeId {
        match self.context.escape_owner.get() {
            Some((node, owner)) if node == self.node.id => owner,
            _ => self.node.id,
        }
    }

    /// pyro: the paint child of this clipping box that takes its clip over
    /// (`BlitzDomPainter::handed_clips`): a stacking context that does not clip itself and holds
    /// boxes this box does not contain (their containing block lies outside it): they must escape
    /// this box's clip inside the child's effect group, which a clip layer around the child would
    /// hold them in (WPT css-color `clip-opacity-out-of-flow`).
    fn clip_transfer_child(&self) -> Option<NodeId> {
        let tree = self.context.dom.as_ref().tree();
        let children = self.node.paint_children.borrow().clone().unwrap_or_default();
        children.into_iter().find(|&id| {
            let child = &tree[id];
            let Some(sc) = &child.stacking_context else { return false };
            if child.primary_styles().is_none_or(|style| self.context.should_clip_node(child, &style)) {
                return false;
            }
            sc.pos_z_hoisted_children()
                .chain(sc.neg_z_hoisted_children())
                .any(|c| escapes_clip_of(tree, self.node.id, c.node_id))
                || child
                    .paint_children
                    .borrow()
                    .as_ref()
                    .is_some_and(|c| c.iter().any(|&id| escapes_clip_of(tree, self.node.id, id)))
        })
    }

    /// pyro: whether `child_id`, one of this element's `paint_children`, paints at the positioned
    /// level: positioned, or a stacking context (CSS 2.1 Appendix E step 8).
    fn is_positioned_level(&self, child_id: NodeId) -> bool {
        let tree = self.context.dom.as_ref().tree();
        let Some(child) = tree.get(child_id) else { return false };
        child.element_data().is_some()
            && (child.primary_styles().is_some_and(|style| style.clone_position() != style::computed_values::position::T::Static)
                || child.is_stacking_context_root(false))
    }

    /// pyro: whether this element's outline goes before its positioned-level children and those
    /// with a positive `z-index` (it has a drawn outline and such children; not a multi-column
    /// container, which paints them in its columns).
    fn defers_positioned(&self) -> bool {
        if self.outline_line().is_none() || self.node.column_fragments().is_some() || self.node.vertical_column_fragments().is_some() {
            return false;
        }
        self.node.stacking_context.as_ref().is_some_and(|sc| sc.pos_z_hoisted_children().len() > 0)
            || self.node.paint_children.borrow().as_ref().is_some_and(|children| {
                children.iter().any(|&id| self.is_positioned_level(id))
            })
    }

    /// pyro: whether this element paints children at the positioned level or with a positive
    /// `z-index` (`draw_inline_boxes_over_atomics` would paint over them).
    fn has_positioned_level_children(&self) -> bool {
        self.node.stacking_context.as_ref().is_some_and(|sc| sc.pos_z_hoisted_children().len() > 0)
            || self.node.paint_children.borrow().as_ref().is_some_and(|children| {
                children.iter().any(|&id| self.is_positioned_level(id))
            })
    }

    /// pyro: whether `child_id` is left for after this element's outline (`defers_positioned`).
    fn skips_positioned(&self, child_id: NodeId) -> bool {
        self.context.positioned_after_outline_of.get() == Some(self.node.id) && self.is_positioned_level(child_id)
    }

    /// pyro: the children [`Self::skips_positioned`] left out, after the outline: the positioned
    /// level in tree order, then those with a positive `z-index`; each in this element's clip
    /// (`clip`, when `should_clip`) unless its containing block lies outside it.
    #[allow(clippy::too_many_arguments)]
    fn draw_positioned_after_outline(
        &self,
        scene: &mut impl PaintScene,
        transform: Affine,
        should_clip: bool,
        clip: &kurbo::BezPath,
        outer_clip_rect: Rect,
        clip_rect: Rect,
    ) {
        let positioned: Vec<NodeId> = self
            .node
            .paint_children
            .borrow()
            .as_ref()
            .map(|children| children.iter().copied().filter(|&id| self.is_positioned_level(id)).collect())
            .unwrap_or_default();
        let layers = &self.context.layer_manager;
        for id in positioned {
            let clipped = should_clip && !self.escapes_own_clip(id);
            layers.maybe_with_layer(scene, clipped, BlendMode::default(), 1.0, self.transform, clip, None, None, |scene| {
                self.draw_paint_child(scene, id, transform, child_clip_rect_for(clipped, outer_clip_rect, clip_rect));
            });
        }
        if let Some(sc) = &self.node.stacking_context {
            for child in sc.pos_z_hoisted_children() {
                let clipped = should_clip && !self.escapes_own_clip(child.node_id);
                layers.maybe_with_layer(scene, clipped, BlendMode::default(), 1.0, self.transform, clip, None, None, |scene| {
                    self.draw_hoisted_children(scene, std::iter::once(child), transform, child_clip_rect_for(clipped, outer_clip_rect, clip_rect));
                });
            }
        }
    }

    /// pyro: the multi-column container (horizontal, no transform between) below this element
    /// that `child_id`, which this element paints, is in: the container, its border box's offset
    /// from this element's (CSS px), and the child's border box range in its strip (its overflow
    /// below included).
    fn multicol_between(&self, child_id: NodeId) -> Option<(NodeId, Vec2, f32, f32)> {
        let (container, origin, top, bottom) = self.node.multicol_between(child_id)?;
        Some((container, Vec2::new(origin.x as f64, origin.y as f64), top, bottom))
    }

    /// pyro: the background and borders of `child_id`'s fragment in a column with strip range
    /// `start..end` of multi-column container `container` (`transform`: the column's), where the
    /// child is of `box-decoration-break: clone` and a break goes through it there: its border box
    /// as far as the column shows it, its block-start border and padding repeated above where it is
    /// cut at the column's start and its block-end ones below where it is cut at its end (the
    /// column's content is moved down for the first: `Breaks::clones`). Nothing for any other box.
    fn draw_clone_fragment(&self, scene: &mut impl PaintScene, container: NodeId, child_id: NodeId, start: f32, end: f32, transform: Affine) {
        let tree = self.context.dom.as_ref().tree();
        let child = &tree[child_id];
        let Some(style) = child.primary_styles() else { return };
        let clone = style.get_border().box_decoration_break == style::values::computed::BoxDecorationBreak::Clone;
        // (Of `box-sizing: border-box`, its repeated borders take its content's room, inside the
        // slice: `Breaks::clones`.)
        let border_box = style.get_position().box_sizing == style::computed_values::box_sizing::T::BorderBox;
        drop(style);
        let layout = *child.final_layout();
        // (Not one whose content overflows it: `Breaks::clones`.)
        if !clone || child.element_data().is_none() || layout.scrollable_overflow_rect.bottom() > layout.size.height + 0.01 {
            return;
        }
        let mut origin = Vec2::new(layout.location.x as f64, layout.location.y as f64);
        let mut at = child.layout_parent.get();
        let mut steps = 0;
        while let Some(parent) = at.filter(|&p| p != container) {
            steps += 1;
            if steps > 64 {
                return;
            }
            let location = tree[parent].final_layout().location;
            origin += Vec2::new(location.x as f64, location.y as f64);
            at = tree[parent].layout_parent.get();
        }
        if at.is_none() {
            return;
        }
        let (top, bottom) = (origin.y as f32, origin.y as f32 + layout.size.height);
        // (Its border box, not what overflows it, is in the column and a break goes through it.)
        let (cut_start, cut_end) = (top < start - 0.01, bottom > end + 0.01);
        if (!cut_start && !cut_end) || bottom <= start + 0.01 || top >= end - 0.01 {
            return;
        }
        let (above, below) = if border_box {
            (0.0, 0.0)
        } else {
            (layout.border.top + layout.padding.top, layout.border.bottom + layout.padding.bottom)
        };
        let from = if cut_start { start - above } else { top };
        let to = if cut_end { end + below } else { bottom };
        let mut piece = layout;
        piece.size.height = (to - from).max(0.0);
        let box_transform = transform.pre_translate(Vec2::new(origin.x * self.scale, from as f64 * self.scale));
        let cx = self.context.element_cx(child, piece, box_transform, None);
        cx.draw_background(scene);
        cx.draw_border(scene);
    }

    /// pyro: how a column with strip range `start..end` of multi-column container `container`
    /// shares the border box of its paint child `child_id` (see `PaintContext::column_range`).
    fn column_share(&self, container: NodeId, child_id: NodeId, start: f32, end: f32) -> ColumnShare {
        let tree = self.context.dom.as_ref().tree();
        let child = &tree[child_id];
        let size = child.final_layout().size;
        let mut top = child.final_layout().location.y;
        let mut at = child.layout_parent.get();
        let mut steps = 0;
        while let Some(parent) = at.filter(|&p| p != container) {
            steps += 1;
            if steps > 64 {
                return ColumnShare::Cut;
            }
            top += tree[parent].final_layout().location.y;
            at = tree[parent].layout_parent.get();
        }
        if at.is_none() {
            return ColumnShare::Cut;
        }
        // (What overflows it below counts: it goes on in the columns after.)
        let overflow = child.final_layout().scrollable_overflow_rect.bottom().abs();
        // (A nested multi-column container may show content in any later column: its spanners'
        // overflow is not in its overflow rectangle.)
        let bottom = if child.column_fragments().is_some() { f32::INFINITY } else { top + size.height.max(overflow) };
        const EPS: f32 = 0.01;
        if top >= start - EPS && top < end - EPS {
            // (Starts in the column.)
            ColumnShare::Inside
        } else if top < start - EPS && bottom > start + EPS {
            ColumnShare::Cut
        } else if top < start - EPS && size.height <= EPS && start.is_infinite() {
            ColumnShare::Inside
        } else {
            ColumnShare::Outside
        }
    }

    /// pyro: whether `child_id` is left out of this pass of a multi-column container's children
    /// (`PaintContext::multicol_pass`).
    fn multicol_skips(&self, child_id: NodeId) -> bool {
        let Some((container, only_unfragmented)) = self.context.multicol_pass.get() else {
            return false;
        };
        if container != self.node.id {
            return false;
        }
        let tree = self.context.dom.as_ref().tree();
        let child = &tree[child_id];
        // Its containing block is the container itself, not a positioned (or transformed,
        // contained) inline or box between them, which is fragmented with the columns.
        let mut unfragmented = child.is_out_of_flow() && child.containing_block() == Some(container);
        let mut ancestor = child.parent;
        while unfragmented && let Some(id) = ancestor.filter(|&id| id != container) {
            let node = &tree[id];
            if let Some(style) = node.primary_styles() {
                let box_style = style.get_box();
                if box_style.position != style::computed_values::position::T::Static
                    || !box_style.transform.0.is_empty()
                    || box_style.slow_clone_contain().intersects(
                        style::values::specified::box_::Contain::LAYOUT | style::values::specified::box_::Contain::PAINT,
                    )
                {
                    unfragmented = false;
                }
            }
            ancestor = node.parent;
        }
        unfragmented != only_unfragmented
    }

    /// Paints `child_id`, one of this node's `paint_children`.
    fn draw_paint_child(
        &self,
        scene: &mut impl PaintScene,
        child_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // pyro: this element clips its children each (`BlitzDomPainter::handed_clips`): the child
        // that takes its clip over gets it, the others are drawn in it.
        let per_child = self
            .context
            .handed_clips
            .borrow()
            .clone()
            .filter(|(parent, _, _, _)| *parent == self.node.id);
        if let Some((_, transfer, device_clip, outer_rect)) = per_child {
            if child_id == transfer {
                *self.context.handed_clip.borrow_mut() = Some((self.node.id, device_clip));
                self.draw_paint_child_unclipped(scene, child_id, parent_style_transform, outer_rect);
                // (Not left for another element when this one painted nothing.)
                self.context.handed_clip.borrow_mut().take();
            } else {
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &device_clip);
                self.draw_paint_child_unclipped(scene, child_id, parent_style_transform, clip_rect);
                scene.pop_layer();
            }
            return;
        }
        self.draw_paint_child_unclipped(scene, child_id, parent_style_transform, clip_rect);
    }

    fn draw_paint_child_unclipped(
        &self,
        scene: &mut impl PaintScene,
        child_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // pyro: an inline (not atomic, not listed for its own content) that keeps out-of-flow boxes
        // in its stacking context stands for them here (blitz-dom
        // `restore_out_of_flow_stacking_owners`): they paint at this positioned level, placed as this
        // element's hoisted children are, in a group with its opacity and filter that is their
        // backdrop root (its own fragments paint with its inline root's content).
        if let Some(child) = self.context.dom.get_node(child_id)
            && !child.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY)
            && child.primary_styles().is_some_and(|style| style.clone_display().is_inline_flow())
            && clip_path::owns_hoisted(self.context.dom.as_ref(), child_id)
        {
            let owned = clip_path::owned_hoisted(self.context.dom.as_ref(), child_id);
            let style = child.primary_styles().unwrap();
            let opacity = style.get_effects().opacity;
            let filter = convert_filters(&style.get_effects().filter.0, self.scale, &style.clone_color(), self.context.dom)
                .map(Arc::new);
            drop(style);
            let viewport = Rect::new(0.0, 0.0, self.context.width as f64, self.context.height as f64);
            let grouped = opacity < 1.0 || filter.is_some();
            if grouped {
                scene.push_layer(Fill::NonZero, BlendMode::default(), opacity, Affine::IDENTITY, &viewport, filter, None);
            }
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.0,
                Affine::IDENTITY,
                &viewport,
                Some(crate::layers::backdrop_root_marker()),
                None,
            );
            self.draw_hoisted_children(scene, owned.iter(), parent_style_transform, clip_rect);
            scene.pop_layer();
            if grouped {
                scene.pop_layer();
            }
            return;
        }
        // Fixed-position children of the root element are positioned against
        // the viewport and do not scroll with it, so cancel out the viewport
        // scroll (applied in `paint_scene`). A fixed box contained by a
        // transformed ancestor scrolls with it like any out-of-flow box.
        // pyro: so does one lifted here from a containing block that is no
        // layer root (a `backdrop-filter` box): it is placed by its offset.
        if self.multicol_skips(child_id) || self.skips_positioned(child_id) {
            return;
        }
        // pyro: a box this element paints from inside a multi-column container below it (a
        // stacking context hoisted here, past the container's columns) is painted in each column
        // it is in, moved and clipped as the column's own content (Chromium; WPT css-multicol
        // `ancestor-toggle-spanner-002`).
        if !self.context.hoisted_in_column.get()
            && let Some((container, origin, child_top, child_bottom)) = self.multicol_between(child_id)
            && let Some(columns) = self.context.dom.as_ref().column_fragments(container)
            && columns.fragments.len() > 1
        {
            let scale = self.scale;
            self.context.hoisted_in_column.set(true);
            // (The columns its range reaches only: they are in strip order.)
            let first = columns.fragments.partition_point(|f| f.end <= child_top);
            for fragment in columns.fragments[first..].iter().take_while(|f| f.start < child_bottom) {
                if !(child_top < fragment.end && child_bottom > fragment.start) || fragment.spanner {
                    continue;
                }
                let range = Rect::new(
                    -1e7,
                    ((origin.y + fragment.start as f64) * scale).max(-1e7),
                    1e7,
                    ((origin.y + fragment.end as f64) * scale).min(1e7),
                );
                let offset = Vec2::new(fragment.offset.x as f64 * scale, fragment.offset.y as f64 * scale);
                let moved = parent_style_transform.pre_translate(offset);
                let pushed = self.context.layer_manager.maybe_push_layer(
                    scene,
                    true,
                    peniko::BlendMode::default(),
                    1.0,
                    moved,
                    &range,
                    None,
                    None,
                );
                self.draw_paint_child(scene, child_id, moved, clip_rect);
                self.context.layer_manager.maybe_pop_layer(scene, pushed);
            }
            self.context.hoisted_in_column.set(false);
            return;
        }
        // pyro: in a column painted without a block-axis clip (`PaintContext::column_range`), a
        // child of the container that starts in the column is cut at its end only (what
        // overflows it above shows, as in Chromium; what runs past its end goes on in the next
        // column), one that runs in from an earlier column is cut at both edges, and one wholly
        // in another column is left to it.
        if let Some((container, start, end, transform, outer_range)) = self.context.column_range.get()
            && container == self.node.id
        {
            // (A positioned inline box listed in the paint tree paints its lines from the entries
            // its root recorded, each clipped to its column (`inline_pass_entries`): once, not
            // sorted by its box: WPT css-position `position-absolute-in-inline-007a`.)
            if self
                .context
                .dom
                .get_node(child_id)
                .is_some_and(|node| node.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY))
            {
                self.context.column_range.set(None);
                self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
                self.context.column_range.set(Some((container, start, end, transform, outer_range)));
                return;
            }
            let range = outer_range;
            let share = self.column_share(container, child_id, start, end);
            match share {
                ColumnShare::Outside => return,
                ColumnShare::Inside | ColumnShare::Cut => {
                    // pyro: a box of `box-decoration-break: clone` a break goes through has its
                    // fragment's background and borders, all four, here (under its content).
                    self.draw_clone_fragment(scene, container, child_id, start, end, transform);
                    let range = match share {
                        ColumnShare::Inside => Rect::new(range.x0, -1e9, range.x1, range.y1),
                        _ => range,
                    };
                    let pushed = self.context.layer_manager.maybe_push_layer(
                        scene,
                        true,
                        peniko::BlendMode::default(),
                        1.0,
                        transform,
                        &range,
                        None,
                        None,
                    );
                    // (Its own descendants are not sorted again.)
                    self.context.column_range.set(None);
                    self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
                    self.context.column_range.set(Some((container, start, end, transform, outer_range)));
                    self.context.layer_manager.maybe_pop_layer(scene, pushed);
                    return;
                }
            }
        }
        // pyro: a spanner's row paints that spanner alone, whole (`spanner_row_spanner`), and the
        // boxes it is in (their own boxes left out: `spanner_row_container`).
        if let (Some(container), Some(spanner)) =
            (self.context.spanner_row_container.get(), self.context.spanner_row_spanner.get())
        {
            let dom = self.context.dom.as_ref();
            if child_id == spanner {
                self.context.spanner_row_container.set(None);
                self.context.spanner_row_spanner.set(None);
                self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
                self.context.spanner_row_container.set(Some(container));
                self.context.spanner_row_spanner.set(Some(spanner));
                return;
            }
            let mut at = dom.tree()[spanner].layout_parent.get();
            let mut on_path = false;
            while let Some(node) = at.filter(|&n| n != container) {
                if node == child_id {
                    on_path = true;
                    break;
                }
                at = dom.tree()[node].layout_parent.get();
            }
            if !on_path {
                return;
            }
        }
        if self.context.spanner_row_except.get() == Some(child_id) {
            return;
        }
        // pyro: a column spanner is painted in its own row, not in a column that reaches it (its
        // outline over the row's edge would show in the column before it).
        if let Some(container) = self.context.column_pass_container.get()
            && self.context.dom.as_ref().is_column_spanner_of(container, child_id)
        {
            return;
        }
        let child = &self.context.dom.as_ref().tree()[child_id];
        // pyro: an atomic inline box in a positioned inline box this inline root left to its paint-tree
        // entry paints there, with the box's content (`draw_inline_pass_entry`).
        if self.context.inline_entry_owner(child, self.node.id).is_some_and(|span| {
            self.context.inline_pass_entries.borrow().get(&span).is_some_and(|entries| entries.iter().any(|entry| entry.0 == self.node.id))
        }) {
            return;
        }
        // pyro: moved with a sticky inline containing block (`inline_sticky_containing_block_offset`).
        let sticky = child.inline_sticky_containing_block_offset();
        let parent_style_transform = if sticky.x == 0.0 && sticky.y == 0.0 {
            parent_style_transform
        } else {
            parent_style_transform.pre_translate(kurbo::Vec2 { x: sticky.x as f64 * self.scale, y: sticky.y as f64 * self.scale })
        };
        let lifted = child.lifted_paint_offset(self.node.id);
        // (pyro: not when the root is the fixed box's containing block itself: `root_contains_fixed`.)
        let child_transform = if child.taffy_position() == taffy::Position::Fixed
            && Some(self.node.id) == self.context.root_element_id
            && lifted.is_none()
            && !self.node.root_contains_fixed()
        {
            let scroll = self.context.dom.as_ref().viewport_scroll();
            parent_style_transform.pre_translate(kurbo::Vec2 {
                x: scroll.x * self.scale,
                y: scroll.y * self.scale,
            })
        } else if let Some(offset) = lifted {
            parent_style_transform.pre_translate(kurbo::Vec2 {
                x: offset.x as f64 * self.scale,
                y: offset.y as f64 * self.scale,
            })
        } else {
            parent_style_transform
        };
        self.render_node(scene, child_id, child_transform, clip_rect);
    }

    /// pyro: the paint level of `child`, one of this node's `paint_children` (as
    /// blitz-dom's paint tree sorts them): 0 in-flow, 1 a float, 2 a positioned box or a
    /// stacking context (a positioned float too).
    /// pyro: whether this element is a collapsed table that paints its parts in phases
    /// ([`Node::paints_in_table_phases`]) and its borders between them
    /// (`draw_children_in_phases`). Not in columns or a 3D plane, which paint children their way.
    /// pyro: whether this element is a collapsed table that paints its parts in phases
    /// ([`Node::paints_in_table_phases`]) and its borders between them
    /// (`draw_children_in_phases`). Not in columns, which paint children their way.
    fn table_borders_in_phases(&self) -> bool {
        matches!(&self.element.special_data,
            blitz_dom::node::SpecialElementData::TableRoot(table)
                if table.border_collapse == style::computed_values::border_collapse::T::Collapse
                    && !table.cells.is_empty())
            && self.node.column_fragments().is_none()
    }

    /// pyro: a collapsed table with positioned parts (rows, row groups, cells) leaves its borders to
    /// the last cell painted in one of them (`last_positioned_table_cell`): the positioned parts
    /// paint later, in their own layers, and Chromium paints the borders over them too (WPT
    /// `collapsed-border-positioned-tr-td`).
    fn table_borders_deferred(&self) -> bool {
        let blitz_dom::node::SpecialElementData::TableRoot(table) = &self.element.special_data else {
            return false;
        };
        table.border_collapse == style::computed_values::border_collapse::T::Collapse
            && last_positioned_table_cell(self.context.dom.as_ref(), table).is_some()
    }

    /// pyro: the borders of the collapsed table this cell is the last positioned cell of
    /// (`table_borders_deferred`), after the cell.
    fn draw_deferred_table_borders(&self, scene: &mut impl PaintScene) {
        if !self.node.primary_styles().is_some_and(|style| {
            style.clone_display().inside() == style::values::specified::box_::DisplayInside::TableCell
        }) {
            return;
        }
        let dom = self.context.dom.as_ref();
        let Some(table_node) = self.node.layout_parent.get().and_then(|id| dom.get_node(id)) else { return };
        let Some(blitz_dom::node::SpecialElementData::TableRoot(table)) =
            table_node.element_data().map(|element| &element.special_data)
        else {
            return;
        };
        if table.border_collapse != style::computed_values::border_collapse::T::Collapse
            || last_positioned_table_cell(dom, table) != Some(self.node.id)
            || table_node
                .primary_styles()
                .is_none_or(|style| style.get_inherited_box().visibility != StyloVisibility::Visible)
        {
            return;
        }
        let location = self.node.final_layout().location;
        let transform = self.transform
            * Affine::translate(Vec2::new(-location.x as f64, -location.y as f64) * self.scale);
        let table_cx = self.context.element_cx(table_node, *table_node.final_layout(), transform, None);
        table_cx.draw_table_borders(scene);
    }

    /// pyro: whether `child` is a scroll container painted in this box's phases, clipped and
    /// scrolled (`Node::scroller_paints_in_ancestor_phases`): only one whose paint children are
    /// all in-flow boxes (a float or positioned box needs its layer).
    fn scroller_in_phases(&self, child: &Node) -> bool {
        child.scroller_paints_in_ancestor_phases()
            && child.paint_children.borrow().as_ref().is_some_and(|children| {
                let tree = self.context.dom.as_ref().tree();
                children.iter().all(|&id| self.paint_level(&tree[id]) == 0)
            })
    }

    /// pyro: whether `child` is a flex or grid container painted in this box's phases
    /// (`Node::items_paint_in_ancestor_phases`), its paint children all in-flow items.
    fn items_in_phases(&self, child: &Node) -> bool {
        // (Shared with hit testing; a box in the top layer is painted there, not as an item.)
        child.items_in_phases()
            && child.paint_children.borrow().as_ref().is_some_and(|children| {
                children.iter().all(|&id| !self.context.dom.is_in_top_layer(id))
            })
    }

    fn paint_level(&self, child: &Node) -> u8 {
        let Some(style) = child.primary_styles() else {
            return 0;
        };
        if style.clone_position() != style::computed_values::position::T::Static
            || child.is_stacking_context_root(false)
            || self.context.dom.is_in_top_layer(child.id)
        {
            2
        } else if style.clone_float() != style::computed_values::float::T::None {
            1
        } else {
            0
        }
    }

    /// pyro: paints the children of a node with floats among its `paint_children`
    /// (`NodeFlags::HAS_FLOAT_PAINT_CHILDREN`) and its own inline content in the phases of CSS 2.1
    /// Appendix E: the stacking contexts with negative `z-index` (step 3), the backgrounds of the
    /// block-level boxes of its normal flow (step 4), the floats (step 5), the inline content of
    /// itself and of those boxes (step 7), then the positioned boxes and stacking contexts (steps
    /// 8 and 9). Its own box is painted already.
    fn draw_children_in_phases(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
        content_position: Point,
    ) {
        self.draw_neg_z_children(scene, parent_style_transform, clip_rect);
        self.draw_flow_children(
            scene,
            parent_style_transform,
            clip_rect,
            FlowPhase::Background,
        );
        // pyro: a collapsed table's borders, over the backgrounds of the blocks in its cells
        // (Blink paints them after its descendants' block backgrounds), under their floats and
        // inline content.
        if self.table_borders_in_phases()
            && !self.table_borders_deferred()
            && self.style.get_inherited_box().visibility == StyloVisibility::Visible
        {
            self.draw_table_borders(scene);
        }
        // pyro: a `line-clamp` container's floats are clipped at its block-end content edge, not
        // at its other edges (CSS Overflow 4 § 5.2; WPT css-overflow `line-clamp-with-floats-*`).
        let clamp_clip = {
            let clamp = &self.style.get_box().line_clamp;
            (!clamp.webkit_legacy && !clamp.max_lines.is_none()).then(|| {
                const FAR: f64 = 1e7;
                Rect::new(-FAR, -FAR, FAR, self.frame.content_box.y1)
            })
        };
        if let Some(rect) = &clamp_clip {
            scene.push_clip_layer(Fill::NonZero, self.transform, rect);
        }
        if let Some(children) = &*self.node.paint_children.borrow() {
            let tree = self.context.dom.as_ref().tree();
            for &child_id in children {
                if self.paint_level(&tree[child_id]) == 1 {
                    self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
                }
            }
        }
        if clamp_clip.is_some() {
            scene.pop_layer();
        }
        self.draw_marker(scene, content_position);
        self.draw_inline_layout(scene, content_position);
        let outer_outlines = self.context.deferred_outlines.replace(false);
        self.draw_flow_children(
            scene,
            parent_style_transform,
            clip_rect,
            FlowPhase::Foreground,
        );
        self.draw_inline_boxes_over_atomics(scene, content_position);
        if self.context.deferred_outlines.get() {
            self.draw_flow_children(scene, parent_style_transform, clip_rect, FlowPhase::Outline);
        }
        self.context.deferred_outlines.set(outer_outlines);
        if let Some(children) = &*self.node.paint_children.borrow() {
            let tree = self.context.dom.as_ref().tree();
            for &child_id in children {
                if self.paint_level(&tree[child_id]) == 2 {
                    // pyro: a float painted here (positioned, a stacking context) is clipped as
                    // the others are (WPT css-overflow `line-clamp-with-floats-008`, `-009`).
                    let clip = clamp_clip.filter(|_| {
                        tree[child_id]
                            .primary_styles()
                            .is_some_and(|style| style.clone_float() != style::computed_values::float::T::None)
                    });
                    if let Some(rect) = &clip {
                        scene.push_clip_layer(Fill::NonZero, self.transform, rect);
                    }
                    self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
                    if clip.is_some() {
                        scene.pop_layer();
                    }
                }
            }
        }
        self.draw_pos_z_children(scene, parent_style_transform, clip_rect);
    }

    /// pyro: one phase (`FlowPhase::Background` or `Foreground`) of the in-flow `paint_children`
    /// of a node painted in phases: a block box of the normal flow
    /// (`Node::paints_in_ancestor_phases`) in that phase, any other block-level box whole with
    /// the backgrounds, an inline-level one (an inline block, an image) whole with the inline
    /// content.
    fn draw_flow_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
        phase: FlowPhase,
    ) {
        let Some(children) = &*self.node.paint_children.borrow() else {
            return;
        };
        let tree = self.context.dom.as_ref().tree();
        for &child_id in children {
            let child = &tree[child_id];
            if self.paint_level(child) != 0 {
                continue;
            }
            let child_phase = if child.paints_in_ancestor_phases()
                || child.paints_in_table_phases()
                || block_level_frame_in_phases(child)
                || self.scroller_in_phases(child)
                || self.items_in_phases(child)
            {
                phase
            } else if phase == FlowPhase::Outline {
                // (A box painted whole paints its outline with it.)
                continue;
            } else if child.is_inline_level() == (phase == FlowPhase::Foreground) {
                FlowPhase::All
            } else {
                continue;
            };
            self.context.flow_phase.set(child_phase);
            self.draw_paint_child(scene, child_id, parent_style_transform, clip_rect);
            self.context.flow_phase.set(FlowPhase::All);
        }
    }

    /// The overflow clips a box hoisted to this stacking context (a positioned descendant with a
    /// `z-index`) is painted in, outermost first: those of the boxes between it and this one
    /// that are its containing-block ancestors. pyro: such a box makes no stacking context, so
    /// the child is not painted within its clip layer, and it showed whole outside an
    /// `overflow: hidden` ancestor. Each clip is its box's node, with the box's border box origin
    /// in this context's space (the child's, less what lies between the two).
    fn hoisted_child_clips(
        &self,
        hoisted_child: &blitz_dom::HoistedPaintChild,
        position: taffy::Point<f32>,
        parent_style_transform: Affine,
    ) -> Vec<(NodeId, Affine, kurbo::BezPath)> {
        let tree = self.node.tree();
        let mut clips: Vec<(NodeId, Affine, kurbo::BezPath)> = Vec::new();
        let mut between = (0.0f32, 0.0f32);
        let mut current = tree.get(hoisted_child.node_id).and_then(|child| child.containing_block());
        while let Some(id) = current {
            if id == self.node.id {
                clips.reverse();
                return clips;
            }
            let Some(node) = tree.get(id) else { break };
            // (Out of a multi-column container's strip the walk is not a sum of offsets.)
            if node.column_fragments().is_some() {
                break;
            }
            let location = node.final_layout().location;
            let scroll = *node.scroll_offset();
            between.0 += location.x - scroll.x as f32;
            between.1 += location.y - scroll.y as f32;
            if let Some(styles) = node.primary_styles().filter(|_| node.element_data().is_some())
                && self.context.should_clip_node(node, &styles)
            {
                let origin = Vec2::new(
                    f64::from(position.x - between.0 + location.x) * self.scale,
                    f64::from(position.y - between.1 + location.y) * self.scale,
                );
                let frame = create_css_rect(&styles, node.final_layout(), self.scale);
                let clip = if node.element_data().is_some_and(|el| el.text_input_data().is_some()) {
                    frame.text_input_clip_path()
                } else {
                    box_shadow::overflow_clip_path(&frame, &styles, self.scale)
                };
                clips.push((id, parent_style_transform.pre_translate(origin), clip));
            }
            current = node.containing_block();
        }
        Vec::new()
    }

    /// Paints boxes hoisted to this stacking context, in order, where their layout put them,
    /// each inside its [`Self::hoisted_child_clips`]. pyro: the clip layers stay pushed from one
    /// child to the next as far as the two share their outer clips (the sticky cells of a table
    /// and the buttons in its cells, all in one scroller): a clip layer costs the raster its
    /// whole area, and one stack of them per child cost a ToS table screen's capture half as
    /// much again. A layer the layer limit refused is not popped.
    ///
    /// pyro: where the device pixels are known (screenshots), each child is recorded first, and when
    /// each of its clips covers in whole every pixel the child's ink touches
    /// ([`crate::bounds::clip_covers_pixels`]), none is pushed: they would cut nothing, and one layer
    /// per button inside its `overflow: hidden` table cell still cost a ToS finances capture a tenth
    /// more (Chromium skips a clip that contains what it clips too). A child one clip cuts keeps all
    /// of them. Every clip stays around a child rasterized on a grid of its own (a local-raster
    /// layer, a 3D plane), and around one with a backdrop filter or a blend mode, whose layers read
    /// what the clips hold.
    fn draw_hoisted_children<'c>(
        &self,
        scene: &mut impl PaintScene,
        children: impl Iterator<Item = &'c blitz_dom::HoistedPaintChild>,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let same_place = |a: &Affine, b: &Affine| {
            a.as_coeffs().iter().zip(b.as_coeffs()).all(|(a, b)| (a - b).abs() <= 1e-3)
        };
        let device = self.context.device_pixels.filter(|_| {
            self.context.local_raster_depth.get() == 0 && self.context.three_d.depth.get() == 0
        });
        // The clip layers open, outermost first: node, transform, whether the layer was pushed.
        let mut open: Vec<(NodeId, Affine, bool)> = Vec::new();
        for hoisted_child in children {
            if self.multicol_skips(hoisted_child.node_id) {
                continue;
            }
            let position = hoisted_child.position(self.node.tree(), self.node.id);
            // pyro: moved with a sticky inline containing block (not memoised: it changes on scroll).
            let sticky = self.node.tree()[hoisted_child.node_id].inline_sticky_containing_block_offset();
            let position = taffy::Point { x: position.x + sticky.x, y: position.y + sticky.y };
            let clips = self.hoisted_child_clips(hoisted_child, position, parent_style_transform);
            // Culled to every clip, pushed or not: what lies outside one is cut off either way.
            let child_clip_rect = clips.iter().fold(clip_rect, |rect, (_, transform, clip)| {
                rect.intersect(transform.transform_rect_bbox(clip.bounding_box()))
            });
            let pos = kurbo::Vec2 { x: position.x as f64 * self.scale, y: position.y as f64 * self.scale };
            let transform = parent_style_transform.pre_translate(pos);
            // pyro: a stacking context hoisted here out of a multi-column container below this
            // element is painted in each column it is in, moved and clipped as the column's own
            // content (Chromium; WPT css-multicol `ancestor-toggle-spanner-002`).
            if clips.is_empty()
                && let Some((container, origin, child_top, child_bottom)) = self.multicol_between(hoisted_child.node_id)
                && let Some(columns) = self.context.dom.as_ref().column_fragments(container)
                && columns.fragments.len() > 1
            {
                for &(_, _, pushed) in open.iter().rev() {
                    self.context.layer_manager.maybe_pop_layer(scene, pushed);
                }
                open.clear();
                let scale = self.scale;
                // (At its place in the strip, which each column moves: not in the column it starts in.)
                let strip = blitz_dom::hoisted_child_strip_position(self.node.tree(), self.node.id, hoisted_child.node_id, container);
                let child_node = &self.node.tree()[hoisted_child.node_id];
                let pos = kurbo::Vec2 { x: (strip.x + sticky.x) as f64 * self.scale, y: (strip.y + sticky.y) as f64 * self.scale };
                // (The columns its range reaches only: they are in strip order.)
                let first = columns.fragments.partition_point(|f| f.end <= child_top);
                for fragment in columns.fragments[first..].iter().take_while(|f| f.start < child_bottom) {
                    if fragment.spanner || !(child_top < fragment.end && child_bottom > fragment.start) {
                        continue;
                    }
                    let range = Rect::new(
                        -1e7,
                        ((origin.y + fragment.start as f64) * scale).max(-1e7),
                        1e7,
                        ((origin.y + fragment.end as f64) * scale).min(1e7),
                    );
                    // (Its border box where Chromium snaps it: the column's exact edge plus its own
                    // unrounded place, rounded; not the rounded column edge plus its rounded place,
                    // which takes a box flush with the last column's end past the container's:
                    // WPT css-multicol `multicol-gap-fraction-002`.)
                    let exact = if fragment.exact_x.is_finite() { fragment.exact_x - fragment.x } else { 0.0 };
                    let unrounded = child_node.unrounded_layout().location.x as f64;
                    let rounded = child_node.final_layout().location.x as f64;
                    let along = strip.x as f64 + sticky.x as f64;
                    let x = (((fragment.offset.x + exact) as f64 + along + unrounded) * scale).round() - (along + rounded) * scale;
                    let offset = Vec2::new(x, fragment.offset.y as f64 * scale);
                    let moved = parent_style_transform.pre_translate(offset);
                    let pushed =
                        self.context.layer_manager.maybe_push_layer(scene, true, BlendMode::default(), 1.0, moved, &range, None, None);
                    self.render_node(scene, hoisted_child.node_id, moved.pre_translate(pos), clip_rect);
                    self.context.layer_manager.maybe_pop_layer(scene, pushed);
                }
                continue;
            }
            let mut recorded = None;
            let mut needed = vec![true; clips.len()];
            if let Some(device) = device.filter(|_| !clips.is_empty()) {
                let mut child = Scene::new();
                self.render_node(&mut child, hoisted_child.node_id, transform, child_clip_rect);
                if !child.commands.iter().any(reads_backdrop) {
                    match crate::bounds::scene_bounds(&child.commands) {
                        // All of them, or none: a box one clip cuts is painted in all of its clips, as
                        // before, so the layers it is composited through are the same and round alike.
                        Some(ink) => {
                            let ink = device.transform_rect_bbox(ink);
                            let covered = clips.iter().all(|(_, clip_transform, clip)| {
                                crate::bounds::clip_covers_pixels(clip, device * *clip_transform, ink)
                            });
                            needed.fill(!covered);
                        }
                        // (It paints nothing.)
                        None => needed.fill(false),
                    }
                }
                recorded = Some(child);
            }
            let wanted: Vec<_> = clips.iter().zip(&needed).filter(|(_, needed)| **needed).map(|(clip, _)| clip).collect();
            let keep = open
                .iter()
                .zip(&wanted)
                .take_while(|(o, c)| o.0 == c.0 && same_place(&o.1, &c.1))
                .count();
            for &(_, _, pushed) in open[keep..].iter().rev() {
                self.context.layer_manager.maybe_pop_layer(scene, pushed);
            }
            open.truncate(keep);
            for &(id, clip_transform, ref clip) in wanted.into_iter().skip(keep) {
                let pushed = self.context.layer_manager.maybe_push_layer(
                    scene,
                    true,
                    BlendMode::default(),
                    1.0,
                    clip_transform,
                    clip,
                    None,
                    None,
                );
                open.push((id, clip_transform, pushed));
            }
            match recorded {
                Some(child) => scene.append_scene(child, Affine::IDENTITY),
                None => self.render_node(scene, hoisted_child.node_id, transform, child_clip_rect),
            }
        }
        for &(_, _, pushed) in open.iter().rev() {
            self.context.layer_manager.maybe_pop_layer(scene, pushed);
        }
    }

    /// pyro: a positioned inline box of this inline root with a negative `z-index`, listed in this
    /// box's own stacking context, is painted before the root's inline content, which is what
    /// leaves it its place and position (`inline_pass_entries`): its entry found nothing and the
    /// root then skipped it, so its text was never painted (WPT css-contain `contain-layout-016`,
    /// `contain-paint-020`). The root says where it is before the negative layer is painted.
    /// `pos` is the position `draw_inline_layout` is given.
    fn register_negative_inline_spans(&self, pos: Point) {
        if !self.node.flags.is_inline_root() {
            return;
        }
        let Some(hoisted) = &self.node.stacking_context else { return };
        for child in hoisted.neg_z_hoisted_children() {
            let Some(span) = self.context.dom.get_node(child.node_id) else { continue };
            if !span.flags.contains(blitz_dom::node::NodeFlags::INLINE_PASS_ENTRY) {
                continue;
            }
            // (Its inline root: the nearest one above it.)
            let mut at = span.parent;
            let mut root = None;
            while let Some(id) = at {
                let Some(node) = self.context.dom.get_node(id) else { break };
                if node.flags.is_inline_root() {
                    root = Some(id);
                    break;
                }
                at = node.parent;
            }
            if root == Some(self.node.id) {
                self.context.inline_pass_entries.borrow_mut().insert(child.node_id, vec![(self.node.id, self.transform, pos, None)]);
            }
        }
    }

    fn draw_neg_z_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // (pyro: before the clip when some escape it: `neg_z_outside_clip_of`.)
        if self.context.neg_z_outside_clip_of.get() == Some(self.node.id) {
            return;
        }
        if let Some(hoisted) = &self.node.stacking_context {
            self.draw_hoisted_children(scene, hoisted.neg_z_hoisted_children(), parent_style_transform, clip_rect);
        }
    }

    fn draw_pos_z_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // Positive z_index hoisted nodes
        // (pyro: after the outline when it defers them: `defers_positioned`.)
        if self.context.positioned_after_outline_of.get() == Some(self.node.id) {
            return;
        }
        if let Some(hoisted) = &self.node.stacking_context {
            self.draw_hoisted_children(scene, hoisted.pos_z_hoisted_children(), parent_style_transform, clip_rect);
        }
    }

    /// pyro: the content of an inline `<svg>`'s `<foreignObject>` (a root laid out in a box of
    /// its size, `BaseDocument::svg_foreign_objects`), in the place usvg draws its stand-in
    /// image: its box `0..size` mapped onto the image's `0..fo.size`. usvg applied the element's
    /// own transform, clip path, mask, filter and opacity to the image; its box is cut at its
    /// edges unless its `overflow` is visible (the UA sheet's is hidden, as Chromium's).
    #[cfg(feature = "svg")]
    fn draw_foreign_object<S: PaintScene>(&self, scene: &mut S, fo: anyrender_svg::ForeignObject) {
        let id = NodeId::from_u64(fo.id);
        let Some(node) = self.context.dom.get_node(id) else { return };
        if node.element_data().is_none() || node.layout_parent.get() != Some(self.node.id) {
            return;
        }
        let Some(style) = node.primary_styles() else { return };
        let layout = *node.final_layout();
        let (width, height) = (f64::from(layout.size.width), f64::from(layout.size.height));
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let scale = self.scale;
        let transform = fo.transform * Affine::scale_non_uniform(fo.size.0 / (width * scale), fo.size.1 / (height * scale));
        let cx = self.context.element_cx(node, layout, transform, None);
        let overflow = style.get_box();
        let clips = overflow.overflow_x != style::values::computed::Overflow::Visible
            || overflow.overflow_y != style::values::computed::Overflow::Visible;
        drop(style);
        if clips {
            scene.push_clip_layer(Fill::NonZero, transform, &Rect::new(0.0, 0.0, width * scale, height * scale));
        }
        let clip_rect = Rect::new(-1e9, -1e9, 1e9, 1e9);
        let pb = (layout.padding + layout.border).map(f64::from);
        let content_position = Point { x: pb.left * scale, y: pb.top * scale };
        // pyro: its `backdrop-filter`, over its box (the capture and the draw markers
        // `render_element` pushes; anyrender_vello_cpu `backdrop`): WPT filter-effects
        // `backdrop-filter-svg-foreignObject`.
        let backdrop = convert_filters(
            &cx.style.get_effects().backdrop_filter.0,
            scale,
            &cx.style.clone_color(),
            self.context.dom,
        )
        .map(Arc::new);
        if let Some(backdrop) = backdrop {
            let border_box = Rect::new(0.0, 0.0, width * scale, height * scale).to_path(0.1);
            scene.push_layer(Fill::NonZero, BlendMode::default(), 0.0, transform, &border_box, None, Some(backdrop.clone()));
            scene.pop_layer();
            scene.push_layer(Fill::NonZero, BlendMode::default(), 1.0, transform, &border_box, None, Some(backdrop));
            scene.pop_layer();
        }
        if cx.style.get_inherited_box().visibility == StyloVisibility::Visible {
            cx.draw_background(scene);
            cx.draw_border(scene);
        }
        if node.flags.intersects(
            blitz_dom::node::NodeFlags::HAS_FLOAT_PAINT_CHILDREN | blitz_dom::node::NodeFlags::FLOW_BOXES_OVERLAP,
        ) {
            cx.draw_children_in_phases(scene, transform, clip_rect, content_position);
        } else {
            cx.draw_neg_z_children(scene, transform, clip_rect);
            cx.draw_inline_layout(scene, content_position);
            cx.draw_children_above(scene, transform, clip_rect);
            if !cx.has_positioned_level_children() {
                cx.draw_inline_boxes_over_atomics(scene, content_position);
            }
        }
        if clips {
            scene.pop_layer();
        }
    }

    /// pyro: see [`Self::draw_svg`]; `false` when the image is drawn from its usvg tree.
    #[cfg(feature = "svg")]
    fn draw_svg_image_document_into<S: PaintScene>(&self, scene: &mut S) -> bool {
        let Some(entry) = self.context.dom.svg_image_documents.get(&self.node.id) else {
            return false;
        };
        let scale = self.scale;
        let content = self.frame.content_box;
        let (width, height) = (f64::from(entry.size.0) * scale, f64::from(entry.size.1) * scale);
        if (content.width() - width).abs() > scale || (content.height() - height).abs() > scale {
            return false;
        }
        let sub_doc = entry.document.inner();
        let translation = self.transform.translation();
        let painter = BlitzDomPainter::new(
            &sub_doc,
            scale,
            width.ceil() as u32,
            height.ceil() as u32,
            translation.x + content.origin().x,
            translation.y + content.origin().y,
            self.custom_widget_scenes,
        );
        scene.push_clip_layer(Fill::NonZero, self.transform, &content);
        painter.paint_scene(scene);
        scene.pop_layer();
        true
    }

    #[cfg(feature = "svg")]
    fn draw_svg(&self, scene: &mut impl PaintScene) {
        use style::properties::generated::longhands::object_fit::computed_value::T as ObjectFit;

        let Some(svg) = self.svg else {
            return;
        };
        // pyro: its `prefers-color-scheme` blocks and `light-dark()` colours follow this element's
        // used colour scheme (blitz-dom `SvgImageData::for_color_scheme`).
        let schemed;
        let svg = match svg.for_color_scheme(self.dark_color_scheme()) {
            Some(dark) => {
                schemed = dark;
                &schemed
            }
            None => svg,
        };
        // pyro: an SVG image with a `<foreignObject>` drawn as a document (blitz-dom
        // `svg_image_documents`), when it is drawn at the size the document is laid out at.
        if self.draw_svg_image_document_into(scene) {
            return;
        }

        // pyro: an SVG image (not an inline `<svg>`) is drawn at its unsnapped content size and
        // stretched to the snapped box, as Chromium draws an image into its pixel-snapped rect:
        // a 100x12.5 image box snapped to 100x13 is filled, without a seam where the `meet` of a
        // 8:1 view box left a part of a pixel (WPT css-sizing
        // `image-fractional-height-with-wide-aspect-ratio`).
        let snapped = (self.frame.content_box.width() as f32, self.frame.content_box.height() as f32);
        let (width, height) = if self.element.name.local != local_name!("svg") {
            let layout = self.node.unrounded_layout();
            let w = (layout.size.width - layout.padding.left - layout.padding.right - layout.border.left - layout.border.right)
                * self.scale as f32;
            let h = (layout.size.height - layout.padding.top - layout.padding.bottom - layout.border.top - layout.border.bottom)
                * self.scale as f32;
            if w > 0.0 && h > 0.0 && (w - snapped.0).abs() < 1.0 && (h - snapped.1).abs() < 1.0 { (w, h) } else { snapped }
        } else {
            snapped
        };
        let stretch = Affine::scale_non_uniform((snapped.0 / width) as f64, (snapped.1 / height) as f64);
        // pyro: an inline `<svg>` without a `viewBox` lays its content out in its box, in user
        // units of a CSS px times its zoom (`zoom: 2` draws its content twice as large).
        let units = self.scale as f32 * self.style.effective_zoom.value();
        // pyro: a source that depends on the laid-out size, serialized again at it (blitz-dom
        // `inline_svg_at_viewport`; WPT css-transforms `transform-percent-010`)
        // pyro: and one with selected text, serialized again with its `::selection` paint
        // (`inline_svg_with_selection`; WPT css-pseudo `svg-text-selection-*`).
        let with_selection = match self.context.draw_text_context.borrow().runs.is_none() {
            true => self.context.dom.as_ref().inline_svg_with_selection(self.node.id, width / units, height / units),
            false => None,
        };
        let at_viewport = if with_selection.is_some() {
            with_selection
        } else if svg.viewport_dependent {
            self.context.dom.as_ref().inline_svg_at_viewport(self.node.id, width / units, height / units)
        } else {
            None
        };
        let svg = at_viewport.as_deref().unwrap_or(svg);
        let sized = svg.sized(width / units, height / units);
        let svg = sized.as_ref().unwrap_or(svg);
        let svg_size = svg.tree.size();

        let x = self.frame.content_box.origin().x;
        let y = self.frame.content_box.origin().y;

        // An SVG with an aspect ratio but no natural size is sized by the default sizing
        // algorithm, which for `none` and `scale-down` contains it within the content box.
        let object_fit = match self.style.clone_object_fit() {
            ObjectFit::None | ObjectFit::ScaleDown
                if svg.intrinsic_width().is_none()
                    && svg.intrinsic_height().is_none()
                    && svg.viewbox_aspect_ratio().is_some() =>
            {
                ObjectFit::Contain
            }
            object_fit => object_fit,
        };
        let object_position = self.style.get_object_position();

        // Apply object-fit algorithm
        let container_size = taffy::Size { width, height };
        // pyro: `object-view-box` (see `draw_image`): the view box is fitted and positioned,
        // the SVG around it drawn at the view box's scale.
        // (An SVG without a natural size has no view box: there is nothing to take a part of.)
        let view = self
            .svg
            .is_some_and(|natural| natural.intrinsic_width().is_some() && natural.intrinsic_height().is_some())
            .then(|| blitz_dom::util::object_view_box(&self.style, svg_size.width(), svg_size.height()))
            .flatten();
        let object_size = match view {
            Some((_, _, width, height)) => taffy::Size { width, height },
            None => taffy::Size {
                width: svg_size.width(),
                height: svg_size.height(),
            },
        };
        let paint_size = compute_object_fit(container_size, Some(object_size), object_fit);

        // Compute object-position
        let x_offset = object_position.horizontal.resolve(
            CSSPixelLength::new(container_size.width - paint_size.width) / self.scale as f32,
        ) * self.scale as f32;
        let y_offset = object_position.vertical.resolve(
            CSSPixelLength::new(container_size.height - paint_size.height) / self.scale as f32,
        ) * self.scale as f32;
        let (x_offset, y_offset, paint_size) = match view {
            Some((left, top, width, height)) => {
                let (x_scale, y_scale) = (paint_size.width / width, paint_size.height / height);
                (
                    CSSPixelLength::new(x_offset.px() - left * x_scale),
                    CSSPixelLength::new(y_offset.px() - top * y_scale),
                    taffy::Size {
                        width: svg_size.width() * x_scale,
                        height: svg_size.height() * y_scale,
                    },
                )
            }
            None => (x_offset, y_offset, paint_size),
        };
        let (origin_x, origin_y) = (x, y);
        let x = x + x_offset.px() as f64;
        let y = y + y_offset.px() as f64;

        let viewport = svg_viewport_affine(svg, paint_size.width, paint_size.height);
        let transform = self.transform
            * Affine::translate((origin_x, origin_y))
            * stretch
            * Affine::translate((x - origin_x, y - origin_y))
            * viewport;

        // pyro: the viewport clip, as Chromium has it (`LayoutSVGRoot::ShouldApplyViewportClip`,
        // `ReplacedPainter`): an inline `<svg>` clips its content unless its `overflow` is
        // `visible` (the UA sheet's `svg:not(:root) { overflow: hidden }`; `overflow="visible"`
        // is its presentation attribute); the root `<svg>` of an SVG document and an SVG image
        // always, whatever the `overflow`. The cut is the content box (`overflow-clip-margin:
        // content-box`), along the content edge's curve. It is a layer only where it cuts
        // something: the content (its strokes and filter regions too) reaches past the box, or
        // the content edge is rounded. An inline `<svg>` that does not clip is not cut at all
        // (no object-fit or rounded cut either: Chromium skips its clip).
        let inline = self.element.name.local == local_name!("svg");
        let document_root = inline && self.node.is_root_element();
        // The content's ink (strokes and filter regions too) in the frame's coordinates.
        let ink = || {
            let bounds = svg.tree.root().abs_layer_bounding_box();
            (Affine::translate((x, y)) * viewport).transform_rect_bbox(Rect::new(
                bounds.left().into(),
                bounds.top().into(),
                bounds.right().into(),
                bounds.bottom().into(),
            ))
        };
        // A hundredth of a device pixel of float noise from the viewport transform.
        let reaches_past = |ink: Rect, rect: Rect| {
            let e = 0.01;
            ink.x0 < rect.x0 - e || ink.y0 < rect.y0 - e || ink.x1 > rect.x1 + e || ink.y1 > rect.y1 + e
        };
        // pyro: an SVG image's viewport is its concrete object rect, not the box: under
        // `object-fit: contain`, `none` or `scale-down` content outside the viewport does not
        // show in the rest of the box (Chromium draws the image into that rect, clipped). A
        // second cut, only where the content reaches past a rect that leaves some of the box.
        let mut object_clip = None;
        let mut margin_clip: Option<kurbo::BezPath> = None;
        let (visible_x, visible_y) = {
            let b = self.style.get_box();
            (matches!(b.overflow_x, Overflow::Visible), matches!(b.overflow_y, Overflow::Visible))
        };
        let clip = if inline && !document_root && visible_x && visible_y {
            ReplacedClip::Never
        } else if inline && !document_root && visible_x != visible_y {
            // pyro: an inline `<svg>` visible on one axis is cut on the other alone, at its
            // content box, with square corners (Chromium clips it per axis and does not round
            // the cut; WPT css-overflow/rounded-overflow-clip-visible,
            // overflow-clip-x-visible-y-svg).
            let content = self.frame.content_box;
            const FAR: f64 = 1.0e7;
            object_clip = Some(Rect::new(
                if visible_x { -FAR } else { content.x0 },
                if visible_y { -FAR } else { content.y0 },
                if visible_x { FAR } else { content.x1 },
                if visible_y { FAR } else { content.y1 },
            ));
            ReplacedClip::Never
        } else if !inline && replaced_overflow_is_visible(self.element, &self.style) {
            // pyro: an SVG image whose `overflow` the author made visible is cut at its own
            // viewport (the object rect) only, not at the box.
            let object = Rect::new(x, y, x + paint_size.width as f64, y + paint_size.height as f64);
            if reaches_past(ink(), object) {
                object_clip = Some(object);
            }
            ReplacedClip::Never
        } else if inline && !document_root && {
            use style::values::generics::box_::OverflowClipMarginBox;
            let margin = &self.style.get_margin().overflow_clip_margin;
            margin.offset.px() != 0.0 || margin.visual_box != OverflowClipMarginBox::ContentBox
        } {
            // pyro: an inline `<svg>` whose author moved its `overflow-clip-margin` off the UA
            // sheet's `content-box` is cut there (Chromium applies the margin to a replaced
            // element that `overflow: hidden` clips; WPT css-overflow `overflow-clip-margin-svg`).
            margin_clip = Some(box_shadow::margin_clip_path(&self.frame, &self.style, self.scale, true, true));
            ReplacedClip::Never
        } else {
            let content = self.frame.content_box;
            if content.is_zero_area() {
                return;
            }
            let ink = ink();
            if !inline {
                let object = Rect::new(x, y, x + paint_size.width as f64, y + paint_size.height as f64);
                let covers = object.x0 <= content.x0
                    && object.y0 <= content.y0
                    && object.x1 >= content.x1
                    && object.y1 >= content.y1;
                if !covers && reaches_past(ink, object) {
                    object_clip = Some(object);
                }
            }
            if reaches_past(ink, content) || self.content_edge_is_rounded() {
                ReplacedClip::Always
            } else {
                ReplacedClip::Fit
            }
        };
        self.with_replaced_content_clip(
            scene,
            container_size,
            paint_size,
            x_offset.px(),
            y_offset.px(),
            clip,
            |scene| {
                // pyro: an SVG image (not an inline `<svg>`) is an isolated group: what
                // blends in it blends with nothing under the image.
                let isolate = !inline && anyrender_svg::has_blend_modes(&svg.tree);
                if let Some(object) = object_clip {
                    scene.push_clip_layer(Fill::NonZero, self.transform, &object);
                }
                if let Some(clip) = &margin_clip {
                    scene.push_clip_layer(Fill::NonZero, self.transform, clip);
                }
                if isolate {
                    scene.push_layer(
                        Fill::NonZero,
                        BlendMode::default(),
                        1.0,
                        self.transform,
                        &self.frame.content_box_path(),
                        None,
                        None,
                    );
                }
                // pyro: an SVG image's root background covers its whole viewport (the object
                // rect), not just the `viewBox` usvg paints it in: the bands of a letterboxed
                // image are painted here (WPT css-sizing `aspect-ratio/replaced-element-004`).
                if let (false, Some(color)) = (inline, svg.intrinsic_dimensions.root_background) {
                    let size = svg.tree.size();
                    let painted = (Affine::translate((x, y)) * viewport).transform_rect_bbox(Rect::new(
                        0.0,
                        0.0,
                        size.width().into(),
                        size.height().into(),
                    ));
                    let object = Rect::new(x, y, x + paint_size.width as f64, y + paint_size.height as f64);
                    let mut bands = kurbo::BezPath::new();
                    bands.extend(object.path_elements(0.1));
                    bands.extend(painted.path_elements(0.1));
                    let color = peniko::Color::from_rgba8(color.red, color.green, color.blue, color.alpha);
                    scene.fill(Fill::EvenOdd, self.transform, color, None, &bands);
                }
                // pyro: with what the document knows of its filters (tainted primitives), and its
                // `<foreignObject>`s' content painted where usvg draws their images.
                let mut handler = |scene: &mut _, node: &anyrender_svg::usvg::Node| match anyrender_svg::foreign_object() {
                    Some(fo) => self.draw_foreign_object(scene, fo),
                    None => anyrender_svg::draw_error_box(scene, node),
                };
                match crate::filters::svg_filter_notes(self.context.dom, self.node, &svg.tree) {
                    Some(notes) => {
                        anyrender_svg::render_svg_tree_with_notes_and(scene, &svg.tree, transform, notes, &mut handler)
                    }
                    None => anyrender_svg::render_svg_tree_with(scene, &svg.tree, transform, &mut handler),
                }
                if isolate {
                    scene.pop_layer();
                }
                if margin_clip.is_some() {
                    scene.pop_layer();
                }
                if object_clip.is_some() {
                    scene.pop_layer();
                }
            },
        );
    }

    /// pyro: whether a corner of the content box is rounded: its radii exceed the border and
    /// padding there (the content edge's curve, CSS Backgrounds 3 §5.3).
    fn content_edge_is_rounded(&self) -> bool {
        let (r, b, p) = (self.frame.border_radii, self.frame.border_width, self.frame.padding_width);
        let i = kurbo::Insets::new(b.x0 + p.x0, b.y0 + p.y0, b.x1 + p.x1, b.y1 + p.y1);
        let rounded = |r: kurbo::Vec2, x: f64, y: f64| r.x > x && r.y > y;
        rounded(r.top_left, i.x0, i.y0)
            || rounded(r.top_right, i.x1, i.y0)
            || rounded(r.bottom_right, i.x1, i.y1)
            || rounded(r.bottom_left, i.x0, i.y1)
    }

    /// Clip replaced content to the content box when its object-fit/object-position
    /// rect overflows it.
    /// pyro: or as `clip` says (see [`ReplacedClip`]).
    fn with_replaced_content_clip<S: PaintScene>(
        &self,
        scene: &mut S,
        container_size: taffy::Size<f32>,
        paint_size: taffy::Size<f32>,
        x_offset: f32,
        y_offset: f32,
        clip: ReplacedClip,
        paint: impl FnOnce(&mut S),
    ) {
        let overflows = x_offset < 0.0
            || y_offset < 0.0
            || x_offset + paint_size.width > container_size.width
            || y_offset + paint_size.height > container_size.height;
        // pyro: replaced content is cut to the content edge's curve (CSS Backgrounds 3 §5.3),
        // which lies inside the padding box's clip once a border or padding is set: a corner
        // whose radii both exceed the border and padding there is still rounded.
        let b = (self.frame.border_width, self.frame.padding_width);
        let inset = b.0.x0 + b.1.x0 != 0.0 || b.0.y0 + b.1.y0 != 0.0 || b.0.x1 + b.1.x1 != 0.0 || b.0.y1 + b.1.y1 != 0.0;
        let rounded_content = inset && self.content_edge_is_rounded();
        // pyro: a raster image the element's own layer clips already (`should_clip_node`), at a
        // padding box that is its content box: cut once, not twice (two antialiased cuts along
        // a rounded edge cover its pixels as the square of one; WPT css-overflow
        // `overflow-img-display-table`).
        let p = self.frame.padding_width;
        let clipped_by_element = self.element.raster_image_data().is_some()
            && !replaced_overflow_is_visible(self.element, &self.style)
            && p.x0 == 0.0
            && p.y0 == 0.0
            && p.x1 == 0.0
            && p.y1 == 0.0;
        let push = match clip {
            ReplacedClip::Never => false,
            ReplacedClip::Always => true,
            ReplacedClip::Fit => (overflows || rounded_content) && !clipped_by_element,
        };
        self.context.layer_manager.maybe_with_layer(
            scene,
            push,
            BlendMode::default(),
            1.0,
            self.transform,
            &self.frame.content_box_path(),
            None,
            None,
            paint,
        );
    }

    /// pyro: the gradient a box's `content` replaces it with (`ElementData::content_gradient`): an
    /// image without a natural size, so it fills the content box.
    fn draw_content_gradient(&self, scene: &mut impl PaintScene) {
        if !self.element.content_gradient {
            return;
        }
        let Some(gradient) = blitz_dom::util::content_gradient(&self.style) else { return };
        let content = self.frame.content_box;
        if content.width() <= 0.0 || content.height() <= 0.0 {
            return;
        }
        let tile = Rect::new(0.0, 0.0, content.width(), content.height());
        let current_color = self.style.clone_color();
        let (gradient, gradient_transform) = crate::gradient::to_peniko_gradient(
            gradient,
            tile,
            self.frame.border_box.bounding_box(),
            self.scale,
            &current_color,
        );
        let transform = self.transform * Affine::translate(content.origin().to_vec2());
        scene.fill(Fill::NonZero, transform, anyrender::Paint::Gradient(&gradient), gradient_transform, &tile);
    }

    /// pyro: Chromium's 16x16 broken-image icon (its `IDR_BROKENIMAGE` resource, as Chrome draws it at
    /// 1x, captured with its alpha) with its top-left corner at `at` (device px), cut to `clip`.
    fn draw_broken_image_icon(&self, scene: &mut impl PaintScene, at: Point, clip: Rect) {
        static ICON: std::sync::OnceLock<peniko::ImageBrush> = std::sync::OnceLock::new();
        let brush = ICON.get_or_init(|| peniko::ImageBrush {
            image: ImageData {
                data: peniko::Blob::new(std::sync::Arc::new(crate::broken_image_icon::BROKEN_IMAGE_16.to_vec())),
                format: peniko::ImageFormat::Rgba8,
                width: 16,
                height: 16,
                alpha_type: peniko::ImageAlphaType::Alpha,
            },
            sampler: ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                quality: peniko::ImageQuality::Medium,
                alpha: 1.0,
            },
        });
        scene.push_clip_layer(Fill::NonZero, self.transform, &clip);
        scene.draw_image(brush.as_ref(), self.transform * Affine::translate(at.to_vec2()) * Affine::scale(self.scale));
        scene.pop_layer();
    }

    /// pyro: a replaced element the selection holds (`BaseDocument::is_selected_replaced`) is
    /// painted over with its `::selection` background, an opaque one made translucent as Blink's
    /// `Color::BlendWithWhite` (WPT css-pseudo `active-selection-041`).
    fn draw_selection_tint(&self, scene: &mut impl PaintScene) {
        if !self.context.dom.is_selected_replaced(self.node.id) || self.context.draw_text_context.borrow().runs.is_some() {
            return;
        }
        let text = self.style.get_inherited_text().color.as_srgb_color();
        let (background, _) = crate::highlight::control_selection_colors(self.context.dom, self.node.id, text);
        let tint = blend_with_white(background);
        if tint.components[3] <= 0.0 {
            return;
        }
        let rect = self.frame.content_box;
        scene.fill(Fill::NonZero, self.transform, tint, None, &rect);
    }

    /// pyro: the filters of this element's `content` when it is one `filter()` image whose
    /// image the element shows (`ElementData::content_image`).
    fn content_image_filter(&self) -> Option<Arc<anyrender::Filter>> {
        use style::values::computed::{Content, ContentItem};
        use style::values::generics::image::GenericImage;
        self.element.content_image.as_ref()?;
        let Content::Items(items) = &self.style.get_counters().content else { return None };
        let [ContentItem::Image(image @ GenericImage::Filter(_))] = &items.items[..] else { return None };
        crate::filters::image_filter(image, self.scale, &self.style.clone_color(), self.context.dom).1
    }

    fn draw_image(&self, scene: &mut impl PaintScene) {
        // pyro: the icon box before a broken image's alt text (`BaseDocument::show_image_alt_text`).
        if self.element.attrs().iter().any(|a| &*a.name.local == "pyro-broken-image-icon") {
            let content = self.frame.content_box;
            self.draw_broken_image_icon(scene, content.origin(), content);
            return;
        }
        // pyro: a broken image without alt text, given a size: Chromium's 1px rgb(192, 192, 192)
        // border round its content box and its broken-image icon 2px in (`draw_broken_image_icon`).
        if self.element.image_failed && !self.element.image_broken {
            let content = self.frame.content_box;
            if content.width() > 0.0 && content.height() > 0.0 {
                let line = self.scale;
                let silver = Color::from_rgb8(192, 192, 192);
                for rect in [
                    Rect::new(content.x0, content.y0, content.x1, content.y0 + line),
                    Rect::new(content.x0, content.y1 - line, content.x1, content.y1),
                    Rect::new(content.x0, content.y0 + line, content.x0 + line, content.y1 - line),
                    Rect::new(content.x1 - line, content.y0 + line, content.x1, content.y1 - line),
                ] {
                    scene.fill(Fill::NonZero, self.transform, silver, None, &rect);
                }
                self.draw_broken_image_icon(scene, Point::new(content.x0 + 2.0 * line, content.y0 + 2.0 * line), content);
            }
            return;
        }
        if let Some(image) = self.element.raster_image_data() {
            let width = self.frame.content_box.width() as u32;
            let height = self.frame.content_box.height() as u32;
            let x = self.frame.content_box.origin().x;
            let y = self.frame.content_box.origin().y;

            let object_fit = self.style.clone_object_fit();
            let object_position = self.style.get_object_position();
            let image_rendering = self.style.clone_image_rendering();
            let quality = to_image_quality(image_rendering);

            // Apply object-fit algorithm
            let container_size = taffy::Size {
                width: width as f32,
                height: height as f32,
            };
            // pyro: `object-view-box`: the view box is what is fitted and positioned; the image
            // around it is drawn at the view box's scale, where the content box has room.
            // pyro: `image-orientation: none`: the pixels were turned by the EXIF orientation
            // when decoded and are drawn turned back (`unturn`, from the image as stored to the
            // decoded pixels' space).
            let unoriented = blitz_dom::util::image_unoriented(&self.style) && image.orientation != 1;
            let (shown_width, shown_height) = image.shown_size(unoriented);
            let (shown_width, shown_height) = (shown_width as f32, shown_height as f32);
            let unturn = unoriented.then(|| Affine::new(image.unturn()));
            let view = blitz_dom::util::object_view_box(&self.style, shown_width, shown_height);
            let object_size = match view {
                Some((_, _, width, height)) => taffy::Size { width, height },
                None => taffy::Size {
                    width: shown_width,
                    height: shown_height,
                },
            };
            let paint_size = compute_object_fit(container_size, Some(object_size), object_fit);

            // Compute object-position
            let x_offset = object_position.horizontal.resolve(
                CSSPixelLength::new(container_size.width - paint_size.width) / self.scale as f32,
            ) * self.scale as f32;
            let y_offset = object_position.vertical.resolve(
                CSSPixelLength::new(container_size.height - paint_size.height) / self.scale as f32,
            ) * self.scale as f32;
            let x = x + x_offset.px() as f64;
            let y = y + y_offset.px() as f64;

            let x_scale = paint_size.width as f64 / object_size.width as f64;
            let y_scale = paint_size.height as f64 / object_size.height as f64;
            // The whole image's place (its part outside the view box too).
            let (view_x, view_y) = view.map_or((0.0, 0.0), |(left, top, _, _)| (left as f64, top as f64));
            let (x, y) = (x - view_x * x_scale, y - view_y * y_scale);
            let (x_offset, y_offset, paint_size) = match view {
                Some(_) => (
                    CSSPixelLength::new(x_offset.px() - (view_x * x_scale) as f32),
                    CSSPixelLength::new(y_offset.px() - (view_y * y_scale) as f32),
                    taffy::Size {
                        width: (shown_width as f64 * x_scale) as f32,
                        height: (shown_height as f64 * y_scale) as f32,
                    },
                ),
                None => (x_offset, y_offset, paint_size),
            };
            let transform = self
                .transform
                .pre_translate(Vec2 { x, y })
                .pre_scale_non_uniform(x_scale, y_scale);
            let transform = match unturn {
                Some(unturn) => transform * unturn,
                None => transform,
            };

            self.with_replaced_content_clip(
                scene,
                container_size,
                paint_size,
                x_offset.px(),
                y_offset.px(),
                match replaced_overflow_is_visible(self.element, &self.style) {
                    true => ReplacedClip::Never,
                    false => ReplacedClip::Fit,
                },
                |scene| {
                    // pyro: a pixelated image stops at its edge: a partly covered edge row or
                    // column (`object-position: 50% 13%` puts the image 1.3px down) sampled the
                    // opposite edge's pixels (the brush repeated). A smoothed image keeps
                    // blending its edge with the opposite one, as a repeating background's
                    // tiles do across their seams (WPT `background-repeat-round`,
                    // `background-size-cover`, whose references are scaled `<img>`s).
                    let mut brush = to_peniko_image(image, quality);
                    if quality == peniko::ImageQuality::Low {
                        brush.sampler.x_extend = peniko::Extend::Pad;
                        brush.sampler.y_extend = peniko::Extend::Pad;
                    }
                    scene.draw_image(brush.as_ref(), transform);
                },
            );
        }
    }

    #[cfg(feature = "custom-widget")]
    fn draw_custom_widget(&self, scene: &mut impl PaintScene) {
        if let Some(widget_scene) = self.custom_widget_scene {
            let x = self.frame.content_box.origin().x;
            let y = self.frame.content_box.origin().y;
            let transform = self.transform.then_translate(Vec2 { x, y });

            // TODO: eliminate clone
            scene.append_scene(widget_scene.clone(), transform);
        }
    }

    fn draw_sub_document(&self, scene: &mut impl PaintScene) {
        if let Some(sub_doc) = self.element.sub_doc_data().map(|doc| doc.inner()) {
            let scale = self.scale;
            let width = self.frame.content_box.width() as u32;
            let height = self.frame.content_box.height() as u32;

            // pyro: the document is painted at its viewport's origin and placed with the frame's
            // whole transform (rotated and scaled frames: WPT css-transforms
            // `transform-iframe-002`, `transform-iframe-scroll-position`), clipped to its
            // content box.
            let origin = self.frame.content_box.origin();
            let transform = self.transform * Affine::translate((origin.x, origin.y));
            let painter = BlitzDomPainter::new(
                &sub_doc,
                scale,
                width,
                height,
                0.0,
                0.0,
                self.custom_widget_scenes,
            );
            let mut recording = anyrender::recording::Scene::new();
            painter.paint_scene(&mut recording);
            let clip = Rect::new(0.0, 0.0, width as f64, height as f64);
            scene.push_clip_layer(Fill::NonZero, transform, &clip);
            scene.append_scene(recording, transform);
            scene.pop_layer();
        }
    }

    fn stroke_devtools(&self, scene: &mut impl PaintScene) {
        if self.devtools.show_layout {
            let shape = &self.frame.border_box;
            let stroke = Stroke::new(self.scale);

            let stroke_color = match self.node.taffy_display() {
                taffy::Display::Block | taffy::Display::FlowRoot => {
                    Color::new([1.0, 0.0, 0.0, 1.0])
                }
                taffy::Display::Flex => Color::new([0.0, 1.0, 0.0, 1.0]),
                taffy::Display::Grid => Color::new([0.0, 0.0, 1.0, 1.0]),
                taffy::Display::None => Color::new([0.0, 0.0, 1.0, 1.0]),
            };

            scene.stroke(&stroke, self.transform, stroke_color, None, &shape);
        }
    }
}
impl<'dom, 'a> std::ops::Deref for ElementCx<'dom, 'a> {
    type Target = BlitzDomPainter<'dom, 'a>;
    fn deref(&self) -> &Self::Target {
        self.context
    }
}

/// pyro: whether `node` is a block-level frame (an element showing a document: an `<iframe>`, or an
/// `<object>`/`<embed>` given one) of the normal flow that makes no layer of its own, painted in
/// its painter's phases: its background and borders with the other block backgrounds (CSS 2.1
/// Appendix E step 4), its document with the inline content (step 7: "if the element is a
/// block-level replaced element, then: the replaced content, atomically"), so a later block's
/// background that overlaps it goes under its content (WPT css/CSS2/normal-flow
/// `cross-domain-iframe-paint-order`). It was painted whole with the backgrounds.
fn block_level_frame_in_phases(node: &blitz_dom::Node) -> bool {
    node.element_data().is_some_and(|element| element.sub_doc_data().is_some())
        && !node.is_inline_level()
        && !node.is_stacking_context_root(false)
        && node.primary_styles().is_some_and(|style| {
            // (Its own overflow clip is the frame's, around its document only.)
            style.clone_float() == style::values::computed::Float::None
                && style.clone_position() == style::computed_values::position::T::Static
        })
}

/// pyro: whether a recorded command is a layer that reads what lies under it as drawn so far: a
/// backdrop filter's, or one with a blend mode (`mix-blend-mode`, a mask). Clips around it stay
/// (`ElementCx::draw_hoisted_children`).
fn reads_backdrop(command: &anyrender::recording::RenderCommand) -> bool {
    matches!(command, anyrender::recording::RenderCommand::PushLayer(layer)
        if layer.backdrop_filter.is_some() || layer.blend != BlendMode::default())
}

fn insets_from_taffy_rect(input: taffy::Rect<f64>) -> Insets {
    Insets {
        x0: input.left,
        y0: input.top,
        x1: input.right,
        y1: input.bottom,
    }
}

/// Convert Stylo and Taffy types into Kurbo types
fn create_css_rect(style: &ComputedValues, layout: &Layout, scale: f64) -> CssBox {
    create_css_rect_with_corners(style, layout, scale, [true; 4])
}

/// pyro: [`create_css_rect`] with only the corners `keep` marks (top left, top right, bottom
/// right, bottom left) rounded: the others are left out before overlapping radii are scaled down,
/// as Blink's `CalcRadiiFor` does for the open sides of an inline box's fragment.
fn create_css_rect_with_corners(style: &ComputedValues, layout: &Layout, scale: f64, keep: [bool; 4]) -> CssBox {
    // Resolve and rescale
    // We have to scale since document pixels are not same same as rendered pixels
    let width: f64 = layout.size.width as f64;
    let height: f64 = layout.size.height as f64;
    let border_box = Rect::new(0.0, 0.0, width * scale, height * scale);
    let border = insets_from_taffy_rect(layout.border.map(|p| p as f64 * scale));
    let padding = insets_from_taffy_rect(layout.padding.map(|p| p as f64 * scale));
    let outline_width = style.get_outline().outline_width.0.to_f64_px() * scale;

    // Resolve the radii to a length. need to downscale since the radii are in document pixels
    let resolve_w = CSSPixelLength::new(width as _);
    let resolve_h = CSSPixelLength::new(height as _);
    let resolve_radii = |radius: &BorderCornerRadius| -> Vec2 {
        Vec2 {
            x: scale * radius.0.width.0.resolve(resolve_w).px() as f64,
            y: scale * radius.0.height.0.resolve(resolve_h).px() as f64,
        }
    };
    let s_border = style.get_border();
    // pyro: `border-shape` ignores `border-radius` and `corner-shape` (CSS Borders 4 §11.5.1).
    let border_shape = !matches!(
        s_border.border_shape,
        style::values::generics::basic_shape::GenericBorderShape::None
    );
    // pyro: a corner whose shape is (nearly) square has no radius (Blink's `CalcRadiiFor`).
    let shaped = |radius: &BorderCornerRadius, k: f32| {
        if border_shape || crate::kurbo_css::contoured::is_degenerate(k as f64) {
            Vec2::ZERO
        } else {
            resolve_radii(radius)
        }
    };
    // pyro: a button of native appearance (the UA sheet's `-moz-default-appearance: button`,
    // any `appearance` but `none` (the compat keywords and `textfield` act as `auto` on a
    // button), no author or user border or background) has Chromium's native look's 1px corners,
    // which are not its computed `border-radius` (0, as Chromium's): one an author styles, or of
    // `appearance: none`, is square (css-ui-4's appearance-disabling properties).
    let native_button = {
        use style::computed_values::appearance::T as Appearance;
        let box_style = style.get_box();
        *box_style.get__moz_default_appearance() == Appearance::Button
            && *box_style.get_appearance() != Appearance::None
            && !style
                .flags
                .contains(style::computed_value_flags::ComputedValueFlags::HAS_AUTHOR_OR_USER_SPECIFIED_BORDER_BACKGROUND)
    };
    // pyro: a menu list of native appearance (the UA sheet's `-moz-default-appearance: menulist`;
    // not `menulist-button`, drawn with its own CSS border, nor `none`), no author or user border or
    // background, has Chromium's themed field's 2px corners (WPT css-ui
    // `appearance-menulist-button-002`: menulist-button is not an alias of auto).
    let native_menulist = {
        use style::computed_values::appearance::T as Appearance;
        let box_style = style.get_box();
        *box_style.get__moz_default_appearance() == Appearance::Menulist
            && !matches!(*box_style.get_appearance(), Appearance::None | Appearance::MenulistButton)
            && !style
                .flags
                .contains(style::computed_value_flags::ComputedValueFlags::HAS_AUTHOR_OR_USER_SPECIFIED_BORDER_BACKGROUND)
    };
    let native = |radius: Vec2| {
        if native_button && radius == Vec2::ZERO && !border_shape {
            Vec2::new(scale, scale)
        } else if native_menulist && radius == Vec2::ZERO && !border_shape {
            Vec2::new(2.0 * scale, 2.0 * scale)
        } else {
            radius
        }
    };
    let kept = |keep: bool, radius: Vec2| if keep { native(radius) } else { Vec2::ZERO };
    let border_radii = NonUniformRoundedRectRadii {
        top_left: kept(keep[0], shaped(&s_border.border_top_left_radius, s_border.corner_top_left_shape.k)),
        top_right: kept(keep[1], shaped(&s_border.border_top_right_radius, s_border.corner_top_right_shape.k)),
        bottom_right: kept(keep[2], shaped(&s_border.border_bottom_right_radius, s_border.corner_bottom_right_shape.k)),
        bottom_left: kept(keep[3], shaped(&s_border.border_bottom_left_radius, s_border.corner_bottom_left_shape.k)),
    };

    let frame = CssBox::new(border_box, border, padding, outline_width, border_radii);
    // pyro: `corner-shape` (CSS Borders 4).
    let exponent = |k: f32| crate::kurbo_css::contoured::exponent(k as f64);
    let curvature = crate::kurbo_css::CornerCurvature {
        top_left: exponent(s_border.corner_top_left_shape.k),
        top_right: exponent(s_border.corner_top_right_shape.k),
        bottom_right: exponent(s_border.corner_bottom_right_shape.k),
        bottom_left: exponent(s_border.corner_bottom_left_shape.k),
    };
    if curvature.is_round() || !frame.has_border_radius() {
        frame
    } else {
        frame.with_corner_curvature(curvature)
    }
}

/// pyro: what [`BlitzDomPainter::paint_element`] needs of `render_element`'s state.
#[derive(Clone, Copy)]
struct ElementPaint<'n> {
    node: &'n Node,
    paints_own_box: bool,
    /// pyro: `visibility` hides what the element paints itself; its descendants still paint.
    hidden: bool,
    should_clip: bool,
    is_text_input: bool,
    content_position: Point,
    child_clip_rect: Rect,
    /// pyro: what the element's ancestors leave visible (its own clip not applied).
    clip_rect: Rect,
}

/// pyro: an element's opacity and filter layer ([`BlitzDomPainter::paint_effect_group`]).
struct EffectLayer {
    /// The element's transform (unscrolled): the layer's and its filter's space.
    transform: Affine,
    /// The area of the screen not clipped away by the element's ancestors.
    visible: Rect,
    /// The element's border box, in its space (a backdrop filter covers it).
    border_box: Rect,
    opacity: f32,
    filter: Option<Arc<anyrender::Filter>>,
    backdrop_filter: Option<Arc<anyrender::Filter>>,
    /// pyro: the element's border box with its radii, in its space: what its backdrop shows in.
    /// (With the space it is in: the filter's, in the element's.)
    backdrop_clip: Option<(kurbo::BezPath, Affine)>,
    /// pyro: the `filter` list as layers when an SVG filter in it is a graph (`filter` is then
    /// `None`): see `filters::filter_steps`.
    #[cfg(feature = "svg")]
    steps: Option<Vec<crate::filters::FilterStep>>,
}

/// pyro: a fragment of an inline (non-atomic) element's box on one line, ready to paint
/// (`ElementCx::inline_box_fragments`).
struct InlineBoxFragment {
    node: NodeId,
    line: usize,
    /// Where its box goes and its size (device px), and its border and padding (none on an
    /// open side).
    transform: Affine,
    size: Size,
    border: taffy::Rect<f32>,
    padding: taffy::Rect<f32>,
    /// A box sliced across lines with shadows or background images: the whole box, placed as the
    /// fragment is, with its border and padding.
    whole: Option<(Affine, Size, taffy::Rect<f32>, taffy::Rect<f32>)>,
    /// Whether the fragment has its left and its right side.
    left: bool,
    right: bool,
    /// Whether its box reaches above its line box, over the line before.
    reaches_above: bool,
    /// pyro: whether an outer shadow of its box reaches left of it, over the text before it on its
    /// line: that text is painted first, as CSS 2.1 Appendix E paints a line's boxes and text in tree
    /// order (WPT css-backgrounds `box-shadow-overlapping-001`).
    reaches_back: bool,
    /// pyro: where its element's text starts (a byte offset in the root's text) when a negative
    /// left margin pulls it back over the text before it: that text is what is painted first then,
    /// wherever it reaches, not the text left of the fragment.
    text_start: Option<usize>,
    /// pyro: its left edge in the line's coordinates (the text's).
    x0: f32,
    /// pyro: its element's style on the first line, in place of its own (`::first-line`).
    style: Option<style::servo_arc::Arc<ComputedValues>>,
    /// pyro: a fragment of a vertical writing mode's turned line: `left` and `right` are its top
    /// and bottom sides.
    turned: bool,
}

/// pyro: the last cell (in tree order, which is the order positioned boxes paint in) of a collapsed
/// table that is positioned or in a positioned row or row group; `None` without one.
fn last_positioned_table_cell(dom: &BaseDocument, table: &blitz_dom::TableContext) -> Option<NodeId> {
    let positioned = |id: NodeId| {
        dom.get_node(id)
            .and_then(|node| node.primary_styles())
            .is_some_and(|style| style.clone_position() != style::computed_values::position::T::Static)
    };
    let row_positioned: Vec<bool> =
        table.rows.iter().map(|row| positioned(row.node_id) || row.group.is_some_and(positioned)).collect();
    if !row_positioned.iter().any(|p| *p) && !table.cells.iter().any(|cell| positioned(cell.node_id)) {
        return None;
    }
    table
        .cells
        .iter()
        .rev()
        .find(|cell| row_positioned.get(cell.row as usize).copied().unwrap_or(false) || positioned(cell.node_id))
        .map(|cell| cell.node_id)
}

/// pyro: where an outside marker's line starts: floats beside the item move the line and its
/// marker (WPT CSS2 `list-style-applies-to-017`), but the item's own floats on the line (in its
/// content, also with text after them) come after the marker: it stays before the item's border
/// box (WPT css-display `display-flow-root-list-item-001`, css-lists
/// `outside-marker-with-float-in-content`).
fn marker_line_x(line: &parley::Line<'_, blitz_dom::node::TextBrush>) -> f32 {
    let own_float = line.items().any(|item| {
        matches!(item, parley::PositionedLayoutItem::InlineBox(ibox) if ibox.kind == parley::InlineBoxKind::CustomOutOfFlow)
    });
    if own_float { 0.0 } else { line.metrics().inline_min_coord }
}

/// pyro: an empty table cell with `empty-cells: hide` in the separated borders model paints no
/// background and no border (CSS 2.1 §17.6.1.1; WPT CSS2 `empty-cells-applies-to-014`). A cell is
/// empty when it holds nothing but collapsible white space and out-of-flow boxes.
fn hides_empty_cell(dom: &blitz_dom::BaseDocument, node: &blitz_dom::Node) -> bool {
    use style::values::specified::box_::DisplayInside;
    let Some(style) = node.primary_styles() else { return false };
    if style.get_inherited_table().empty_cells != style::computed_values::empty_cells::T::Hide
        || style.get_inherited_table().border_collapse != style::computed_values::border_collapse::T::Separate
        || style.clone_display().inside() != DisplayInside::TableCell
    {
        return false;
    }
    node.children.iter().all(|&id| {
        let Some(child) = dom.get_node(id) else { return true };
        if child.is_text_node() {
            return child.is_whitespace_node();
        }
        child.primary_styles().is_none_or(|style| {
            style.clone_display().is_none()
                || matches!(
                    style.clone_position(),
                    style::computed_values::position::T::Absolute | style::computed_values::position::T::Fixed
                )
        })
    }) && node.before().is_none()
        && node.after().is_none()
}


/// pyro: Blink's `Color::BlendWithWhite`: an opaque colour as the most transparent one (alpha 60%
/// to 73%, in steps) that gives it over white; a translucent colour as it is.
fn blend_with_white(color: Color) -> Color {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    if a != 255 {
        return color;
    }
    let blend = |c: u8, alpha: u8| -> i32 { ((i32::from(c) - (255 - i32::from(alpha))) as f32 / (f32::from(alpha) / 255.0)) as i32 };
    let mut out = (r as i32, g as i32, b as i32, 255u8);
    let mut alpha = 153u8;
    while alpha < 204 {
        let (nr, ng, nb) = (blend(r, alpha), blend(g, alpha), blend(b, alpha));
        out = (nr, ng, nb, alpha);
        if [nr, ng, nb].iter().all(|v| (0..=255).contains(v)) {
            break;
        }
        alpha += 17;
    }
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    Color::from_rgba8(clamp(out.0), clamp(out.1), clamp(out.2), out.3)
}

/// pyro: whether `a` comes before `b` in tree order and is not inside it (an inline box paints
/// before its own content).
/// pyro: the tree order (pre-order numbers) of the nodes of the inline root `root`'s inline
/// content: its DOM descendants through inline boxes (and `display: contents`), not descending
/// into atomic inlines or anything else laid out apart, which are numbered themselves. Nodes not in its child lists (pseudo-elements) are not numbered.
fn inline_tree_order(doc: &BaseDocument, root: NodeId) -> HashMap<NodeId, usize> {
    let mut order = HashMap::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        order.insert(id, order.len());
        let Some(node) = doc.get_node(id) else { continue };
        if id != root {
            let Some(style) = node.primary_styles() else { continue };
            let display = style.get_box().display;
            let inline_box = display.outside() == style::values::specified::box_::DisplayOutside::Inline
                && display.inside() == style::values::specified::box_::DisplayInside::Flow;
            if !(inline_box || display.is_contents()) {
                continue;
            }
        }
        stack.extend(node.children.iter().rev().copied());
    }
    order
}

/// pyro: a classic scrollbar's colours: Chrome's defaults, or `scrollbar-color`'s (the arrows
/// in the thumb colour, the corner in the track colour).
#[derive(Clone, Copy)]
pub(crate) struct ClassicScrollbarColors {
    track: Color,
    thumb: Color,
    arrow: Color,
    corner: Color,
}

impl ClassicScrollbarColors {
    pub(crate) fn of(node: &Node) -> Self {
        match node.scrollbar_color() {
            blitz_dom::node::ScrollbarColor::Auto => Self {
                track: CLASSIC_SCROLLBAR_TRACK,
                thumb: CLASSIC_SCROLLBAR_THUMB,
                arrow: CLASSIC_SCROLLBAR_ARROW,
                corner: CLASSIC_SCROLLBAR_CORNER,
            },
            blitz_dom::node::ScrollbarColor::Colors { thumb, track } => {
                let (thumb, track) = (thumb.as_srgb_color(), track.as_srgb_color());
                Self { track, thumb, arrow: thumb, corner: track }
            }
        }
    }
}

/// pyro: classic scrollbar colours (Chrome's on Linux: `NativeThemeBase`).
const CLASSIC_SCROLLBAR_TRACK: Color = Color::from_rgba8(0xf1, 0xf1, 0xf1, 0xff);
const CLASSIC_SCROLLBAR_THUMB: Color = Color::from_rgba8(0xc1, 0xc1, 0xc1, 0xff);
const CLASSIC_SCROLLBAR_ARROW: Color = Color::from_rgba8(0x50, 0x50, 0x50, 0xff);
const CLASSIC_SCROLLBAR_CORNER: Color = Color::from_rgba8(0xdc, 0xdc, 0xdc, 0xff);

/// pyro: paints a classic scrollbar over `track` (scene px under `transform`): the track, an arrow
/// button at each end (as long as the bar is thick) and the thumb, whose length is the track's
/// (between the buttons) times `client / scroll` (CSS px: the scrollport's and the content's
/// length) and whose start follows `offset` (the scroll offset) over the scroll range.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_classic_scrollbar(
    scene: &mut impl PaintScene,
    colors: ClassicScrollbarColors,
    transform: Affine,
    track: Rect,
    vertical: bool,
    client: f64,
    scroll: f64,
    offset: f64,
    scale: f64,
) {
    if track.width() <= 0.0 || track.height() <= 0.0 {
        return;
    }
    scene.fill(Fill::NonZero, transform, colors.track, None, &track);
    let (thickness, length) = if vertical { (track.width(), track.height()) } else { (track.height(), track.width()) };
    // The buttons are as long as the bar is thick, while two of them fit.
    let button = if length >= 2.0 * thickness { thickness } else { 0.0 };
    if button > 0.0 {
        for start in [true, false] {
            let (cx, cy) = if vertical {
                (track.center().x, if start { track.y0 + button / 2.0 } else { track.y1 - button / 2.0 })
            } else {
                (if start { track.x0 + button / 2.0 } else { track.x1 - button / 2.0 }, track.center().y)
            };
            let r = (thickness * 0.2).max(scale);
            let dir = if start { -1.0 } else { 1.0 };
            let mut arrow = kurbo::BezPath::new();
            if vertical {
                arrow.move_to((cx, cy + dir * r * 0.6));
                arrow.line_to((cx - r, cy - dir * r * 0.4));
                arrow.line_to((cx + r, cy - dir * r * 0.4));
            } else {
                arrow.move_to((cx + dir * r * 0.6, cy));
                arrow.line_to((cx - dir * r * 0.4, cy - r));
                arrow.line_to((cx - dir * r * 0.4, cy + r));
            }
            arrow.close_path();
            scene.fill(Fill::NonZero, transform, colors.arrow, None, &arrow);
        }
    }
    let room = length - 2.0 * button;
    if scroll <= client + 0.5 || room <= 0.0 || client <= 0.0 {
        return;
    }
    let min_thumb = thickness.min(room);
    let thumb_len = (room * client / scroll).clamp(min_thumb, room);
    let range = scroll - client;
    let progress = (offset / range).clamp(0.0, 1.0);
    // Whole device pixels, as Chrome paints its scrollbar parts.
    let thumb_len = thumb_len.round();
    let start = (button + progress * (room - thumb_len)).round();
    let inset = (2.0 * scale).min(thickness / 4.0);
    let thumb = if vertical {
        Rect::new(track.x0 + inset, track.y0 + start, track.x1 - inset, track.y0 + start + thumb_len)
    } else {
        Rect::new(track.x0 + start, track.y0 + inset, track.x0 + start + thumb_len, track.y1 - inset)
    };
    scene.fill(Fill::NonZero, transform, colors.thumb, None, &thumb);
}

/// pyro: whether `style` draws an outline (a width and a style other than `none`).
fn has_visible_outline(style: &ComputedValues) -> bool {
    use style::values::computed::OutlineStyle;
    let outline = style.get_outline();
    match outline.outline_style {
        OutlineStyle::Auto => true,
        OutlineStyle::BorderStyle(border) => !border.none_or_hidden() && outline.outline_width.0.to_f64_px() > 0.0,
    }
}
