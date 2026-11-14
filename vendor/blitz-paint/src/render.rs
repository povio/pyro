mod background;
mod border;
mod box_shadow;
mod clip_path;
mod form_controls;
mod mask;

use blitz_dom::CopyStyleGetters;
use std::cell::RefCell;
use std::collections::HashMap;
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
use crate::{CustomWidgetSceneMap, SELECTION_COLOR};
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
use style::values::generics::color::GenericColor;
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
    /// Reusable scratch allocations shared by all text layouts painted for this document.
    pub(crate) draw_text_context: RefCell<DrawTextContext>,
    /// Cached selection ranges for O(1) lookup: node_id -> (start_offset, end_offset)
    pub(crate) selection_ranges: HashMap<NodeId, (usize, usize)>,
    /// Paint background colors and images (false: printing without them).
    pub backgrounds: bool,
    /// Print: text at its layout-unit x, not its box's whole-pixel x (Chromium keeps the fraction in the
    /// inline axis). Screenshots keep the box's x.
    pub fractional_text_x: bool,

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

        let layer_manager = LayerManager::default();
        let root_element_id = dom.try_root_element().map(|el| el.id);

        Self {
            dom,
            scale,
            width,
            height,
            initial_x,
            initial_y,
            root_element_id,
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
            draw_text_context: RefCell::new(DrawTextContext::default()),
            selection_ranges,
            backgrounds: true,
            fractional_text_x: false,
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
        let bg_width = (self.width as f32).max(root_element.final_layout().size.width);
        let bg_height = (self.height as f32).max(root_element.final_layout().size.height);

        let background_color = {
            let html_styles = root_element.primary_styles();
            let html_color = html_styles
                .as_ref()
                .map_or(&GenericColor::TRANSPARENT_BLACK, |s| {
                    s.get_background_color()
                });
            if *html_color == GenericColor::TRANSPARENT_BLACK {
                root_element
                    .children
                    .iter()
                    .find_map(|id| {
                        self.dom
                            .as_ref()
                            .get_node(*id)
                            .filter(|node| node.data.is_element_with_tag_name(&local_name!("body")))
                    })
                    .and_then(|body| body.primary_styles())
                    .map(|style| {
                        let current_color = style.clone_color();
                        style
                            .get_background_color()
                            .resolve_to_absolute(&current_color)
                    })
            } else {
                let current_color = html_styles.as_ref().unwrap().clone_color();
                Some(html_color.resolve_to_absolute(&current_color))
            }
        };

        if let Some(bg_color) = background_color.filter(|_| self.backgrounds) {
            let bg_color = bg_color.as_srgb_color();
            let rect = Rect::from_origin_size(
                (self.initial_x, self.initial_y),
                (bg_width as f64, bg_height as f64),
            );
            scene.fill(Fill::NonZero, Affine::IDENTITY, bg_color, None, &rect);
        }

        // The root clip rectangle is the viewport (in screen coordinates, with the
        // initial offset already subtracted). Elements outside of this are culled, and
        // scrollports narrow this rectangle further for their descendants.
        let viewport_clip_rect = Rect::new(0.0, 0.0, self.width as f64, self.height as f64);

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
        use style::selector_parser::PseudoElement;
        let Some(style) = self
            .dom
            .pseudo_element_style(id, &PseudoElement::Backdrop, false)
        else {
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
            convert_filters(&effects.backdrop_filter.0, self.scale, &style.clone_color()).map(Arc::new);
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

    fn node_transform(&self, node: &Node, parent: Affine) -> Affine {
        let location = node.final_layout().location;
        parent
            * Affine::translate(Vec2::new(location.x as f64, location.y as f64) * self.scale)
            * node.transform().as_deref().copied().unwrap_or_default()
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
            !display.is_inline_flow() && !is_internal_table_box_other_than_cell
        };
        let element = node.element_data();
        self.root_element_id != Some(node.id)
            && (element.and_then(|e| e.raster_image_data()).is_some()
                || element.and_then(|e| e.sub_doc_data()).is_some()
                || element.and_then(|e| e.text_input_data()).is_some()
                || contain_paint
                // pyro: `overflow` on `body` is the viewport's, not body's.
                || ((!matches!(box_style.overflow_x, Overflow::Visible)
                    || !matches!(box_style.overflow_y, Overflow::Visible))
                    && !self.dom.as_ref().body_overflow_goes_to_viewport(node.id)))
    }

    fn draw_text_mask(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        transform: Affine,
        root: bool,
    ) {
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
            cx.frame.padding_box_path()
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
                let layout_box = node.final_layout();
                let x = (layout_box.padding.left + layout_box.border.left) as f64
                    - node.scroll_offset().x;
                let y = (layout_box.padding.top + layout_box.border.top) as f64
                    - node.scroll_offset().y
                    + layout.block_offset as f64;
                let text_transform =
                    transform * Affine::translate((x * self.scale, y * self.scale));
                crate::text::stroke_text(
                    scene,
                    layout.layout.lines(),
                    self.dom,
                    text_transform,
                    TextPaintOptions {
                        scale: self.scale,
                        brush_override: Some(Color::BLACK),
                        // A `background-clip: text` mask is the glyphs alone.
                        shadows: false,
                    },
                    node.id,
                    None,
                    &mut self.draw_text_context.borrow_mut(),
                    None,
                );
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
        let node = &self.dom.as_ref().tree()[node_id];

        // Early return if the element is hidden
        if matches!(node.taffy_display(), taffy::Display::None) {
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

        // Hide elements with a visibility style other than visible
        if styles.get_inherited_box().visibility != StyloVisibility::Visible {
            return;
        }

        let effects = styles.get_effects();
        let opacity = effects.opacity;
        if opacity == 0.0 {
            return;
        }
        let has_opacity = opacity < 1.0;

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
        let scaled_pb = (padding + border).map(f64::from);
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
        if screen_bbox.x1 < clip_rect.x0
            || screen_bbox.x0 > clip_rect.x1
            || screen_bbox.y1 < clip_rect.y0
            || screen_bbox.y0 > clip_rect.y1
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
        let clip_area = content_box_size.width * content_box_size.height;
        let overflow_area =
            node.scrollable_overflow().width() * node.scrollable_overflow().height();
        if should_clip && clip_area < 0.01 && overflow_area < 0.01 {
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
        let child_clip_rect = if should_clip {
            let clip_box = if is_text_input {
                cx.frame.text_input_clip_path()
            } else {
                cx.frame.padding_box_path()
            };
            clip_rect.intersect(screen_transform.transform_rect_bbox(clip_box.bounding_box()))
        } else {
            clip_rect
        };

        // Compute clip-path (if any) and wrap all rendering in a clip layer
        let clip_path_shape = cx.clip_path_shape();
        let has_clip_path = clip_path_shape.is_some();
        let default_clip = cx.frame.border_box_path();
        let mut clip_path_for_layer = clip_path_shape.unwrap_or(default_clip);
        clip_path_for_layer.apply_affine(Affine::scale(self.scale));

        // CSS 2 `clip` property (absolutely positioned elements only). Applies to the whole of
        // the element's rendering (including outlines and shadows), so it is the outermost layer.
        let css_clip_rect = cx.css_clip_rect();
        if css_clip_rect.is_some_and(|rect| rect.is_zero_area()) {
            return;
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
        let isolate = styles.get_box().isolation == Isolation::Isolate;
        let blend_layer_pushed = self.layer_manager.maybe_push_layer(
            scene,
            isolate || blend != BlendMode::default(),
            blend,
            if isolate { 1.0 - f32::EPSILON } else { 1.0 },
            Affine::IDENTITY,
            &Rect::new(
                self.initial_x,
                self.initial_y,
                self.initial_x + self.width as f64,
                self.initial_y + self.height as f64,
            ),
            None,
            None,
        );

        let paints_own_box = !node.is_inline_with_only_block_children();

        // pyro: `opacity` and `filter` apply to everything the element paints, as one group
        // (Chromium's effect node): its outset box-shadow, its box, its content and descendants
        // (positioned ones too: the element is their stacking context) and its outline. The
        // group is recorded first, and its layer covers what it painted (its ink, plus what the
        // filter reaches), where the layer was the border box (descendants and the outline outside
        // it were cut off, the shadow was drawn at full strength outside the layer).
        let current_color = styles.clone_color();
        let filter = convert_filters(&effects.filter.0, self.scale, &current_color).map(Arc::new);
        let backdrop_filter =
            convert_filters(&effects.backdrop_filter.0, self.scale, &current_color).map(Arc::new);
        let has_effect_layer = has_opacity || filter.is_some() || backdrop_filter.is_some();
        let element = ElementPaint {
            node,
            paints_own_box,
            should_clip,
            is_text_input,
            content_position,
            child_clip_rect,
        };

        // clip-path clip layer (it clips the shadow and the outline too, as in Chromium)
        self.layer_manager.maybe_with_layer(
            scene,
            has_clip_path,
            BlendMode::default(),
            1.0,
            cx.transform,
            &clip_path_for_layer,
            None,
            None,
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
            should_clip,
            is_text_input,
            content_position,
            child_clip_rect,
        } = *element;
        let unscrolled_transform = cx.transform;

        if paints_own_box {
            cx.draw_outset_box_shadow(scene);
            if cx.context.backgrounds {
                cx.draw_background(scene);
            }
            cx.draw_inset_box_shadow(scene);
        }
        if cx.context.backgrounds {
            cx.draw_table_row_backgrounds(scene);
        }
        cx.draw_table_borders(scene);
        if paints_own_box {
            cx.draw_border(scene);
        }
        cx.stroke_devtools(scene);

        let clip = if is_text_input {
            &cx.frame.text_input_clip_path()
        } else {
            &cx.frame.padding_box_path()
        };

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
                cx.draw_image(scene);
                #[cfg(feature = "svg")]
                cx.draw_svg(scene);
                #[cfg(feature = "custom-widget")]
                cx.draw_custom_widget(scene);
                cx.draw_sub_document(scene);
                cx.draw_input(scene);
                cx.draw_text_input_text(scene, content_position);
                cx.draw_inline_layout(scene, content_position);
                cx.draw_marker(scene, content_position);
                cx.draw_children(scene, cx.transform, child_clip_rect);
            },
        );
        cx.transform = unscrolled_transform;

        // Overlay scrollbars, drawn unscrolled above the
        // clipped content.
        #[cfg(feature = "scrollbars")]
        cx.draw_scrollbars(scene);

        // The outline goes over the box and its contents (Chromium paints it
        // in a phase of its own after the foreground): one with a negative
        // `outline-offset` lies inside the box, over its background.
        cx.draw_outline(scene);
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
        } = layer;
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
        let pushed = self.layer_manager.maybe_push_layer(
            scene,
            true,
            BlendMode::default(),
            opacity,
            transform,
            &bounds,
            filter,
            backdrop_filter,
        );
        scene.append_scene(group, Affine::IDENTITY);
        self.layer_manager.maybe_pop_layer(scene, pushed);
    }

    fn render_node(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        let node = &self.dom.as_ref().tree()[node_id];

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
        let frame = create_css_rect(&style, &layout, scale);

        let element = node.element_data().unwrap();

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

/// Converts parley BoundingBox into peniko Rect
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
) -> Option<(crate::text::TextCut, f32)> {
    let epsilon = scale / 64.0;
    let metrics = line.metrics();
    let ellipsis_width = ellipsis.full_width();
    let runs = || {
        line.items().filter_map(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => Some(run),
            parley::PositionedLayoutItem::InlineBox(_) => None,
        })
    };
    if !rtl {
        if metrics.offset + metrics.advance - metrics.hanging_advance <= width + epsilon {
            return None;
        }
        let cut = crate::text::TextCut { edge: width - ellipsis_width, keep_left: true };
        let end = runs().filter_map(|run| cut.kept(&run).1).map(|(_, x1)| x1).fold(None, |a: Option<f32>, x| Some(a.map_or(x, |a| a.max(x))));
        Some((cut, end.unwrap_or(metrics.offset)))
    } else {
        if metrics.offset + metrics.hanging_advance >= -epsilon {
            return None;
        }
        let cut = crate::text::TextCut { edge: ellipsis_width, keep_left: false };
        let start = runs().filter_map(|run| cut.kept(&run).1).map(|(x0, _)| x0).fold(None, |a: Option<f32>, x| Some(a.map_or(x, |a| a.min(x))));
        Some((cut, start.unwrap_or(metrics.offset + metrics.advance) - ellipsis_width))
    }
}

fn convert_rect(rect: &parley::BoundingBox) -> kurbo::Rect {
    peniko::kurbo::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1)
}

impl ElementCx<'_, '_> {
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
    /// `tops` where each line's text goes: its boxes go with it.
    fn draw_inline_boxes(
        &self,
        scene: &mut impl PaintScene,
        layout: &parley::Layout<blitz_dom::node::TextBrush>,
        transform: Affine,
        tops: Option<crate::text::LineTops>,
    ) {
        use parley::{InlineBoxKind, PositionedLayoutItem};
        use style::computed_values::direction::T as Direction;
        use style::values::computed::{Length, LengthPercentage, Margin};
        use style::values::generics::image::GenericImage;
        use style::values::generics::length::GenericMargin;

        struct Fragment {
            node: NodeId,
            /// How many inline elements of the root the fragment's is in, itself included.
            depth: usize,
            line: usize,
            x0: f32,
            x1: f32,
            top: f32,
            bottom: f32,
        }

        let styles = layout.styles();
        if styles.len() < 2 {
            return;
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
        for (line_index, line) in layout.lines().enumerate() {
            let first = fragments.len();
            let metrics = line.metrics();
            // White space hanging at the line's end is in no box.
            let content_end = metrics.offset + metrics.advance - metrics.hanging_advance;
            for item in line.items() {
                let (style_index, x0, x1, edge) = match &item {
                    PositionedLayoutItem::GlyphRun(run) => {
                        let x0 = run.offset();
                        let mut x1 = x0 + run.advance();
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
                chain_of(&mut chain, style_index);
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
                        let margin = |v: &Margin| match v {
                            GenericMargin::LengthPercentage(v) => length(v) * self.scale as f32,
                            _ => 0.0,
                        };
                        let rtl = style.clone_direction() == Direction::Rtl;
                        if (kind == InlineBoxKind::SpanStart) != rtl {
                            x0 += margin(&style.get_margin().margin_left);
                        } else {
                            x1 -= margin(&style.get_margin().margin_right);
                        }
                    }
                    match fragments[first..].iter_mut().find(|fragment| fragment.node == node) {
                        Some(fragment) => {
                            fragment.x0 = fragment.x0.min(x0);
                            fragment.x1 = fragment.x1.max(x1);
                        }
                        None => {
                            let (top, bottom) = line.span_content_area(style_index);
                            let shift = tops.map_or(0.0, |tops| tops.shift(metrics)) as f32;
                            fragments.push(Fragment {
                                node,
                                depth: depth - level,
                                line: line_index,
                                x0,
                                x1,
                                top: top + shift,
                                bottom: bottom + shift,
                            });
                        }
                    }
                }
            }
        }
        if fragments.is_empty() {
            return;
        }

        // An element's first and last fragment.
        let mut ends: HashMap<NodeId, (usize, usize)> = HashMap::new();
        for (index, fragment) in fragments.iter().enumerate() {
            ends.entry(fragment.node).and_modify(|ends| ends.1 = index).or_insert((index, index));
        }
        let mut order: Vec<usize> = (0..fragments.len()).collect();
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
            let (starts, ends) = (ends[&fragment.node].0 == index, ends[&fragment.node].1 == index);
            let (left, right) =
                if style.clone_direction() == Direction::Rtl { (ends, starts) } else { (starts, ends) };
            let (border, padding) = (style.get_border(), style.get_padding());
            let side = |open: bool, width: f32| if open { 0.0 } else { width };
            // The used width: none where the side has no border style.
            let used = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
                if style.none_or_hidden() { 0.0 } else { width.0.to_f32_px() }
            };
            let border = taffy::Rect {
                left: side(!left, used(&border.border_left_width, border.border_left_style)),
                right: side(!right, used(&border.border_right_width, border.border_right_style)),
                top: used(&border.border_top_width, border.border_top_style),
                bottom: used(&border.border_bottom_width, border.border_bottom_style),
            };
            let padding = taffy::Rect {
                left: side(!left, length(&padding.padding_left.0)),
                right: side(!right, length(&padding.padding_right.0)),
                top: length(&padding.padding_top.0),
                bottom: length(&padding.padding_bottom.0),
            };
            drop(style);
            let rect = Rect::new(
                fragment.x0 as f64,
                (fragment.top - (border.top + padding.top) * scale) as f64,
                fragment.x1 as f64,
                (fragment.bottom + (border.bottom + padding.bottom) * scale) as f64,
            );
            let (transform, size) = if translation_only {
                // To whole CSS pixels, as Chromium snaps a box's decorations
                // (and as block boxes are laid out), whatever the device scale.
                let snap = |v: f64| (v / self.scale).round() * self.scale;
                let (x0, y0) = (snap(tx + rect.x0), snap(ty + rect.y0));
                let (x1, y1) = (snap(tx + rect.x1), snap(ty + rect.y1));
                (Affine::translate((x0, y0)), Size::new(x1 - x0, y1 - y0))
            } else {
                (transform * Affine::translate((rect.x0, rect.y0)), rect.size())
            };
            let layout = Layout {
                size: taffy::Size {
                    width: (size.width / self.scale) as f32,
                    height: (size.height / self.scale) as f32,
                },
                border,
                padding,
                ..Layout::new()
            };
            let mut cx = self.context.element_cx(node, layout, transform, None);
            if !left {
                cx.frame.border_radii.top_left = Vec2::ZERO;
                cx.frame.border_radii.bottom_left = Vec2::ZERO;
            }
            if !right {
                cx.frame.border_radii.top_right = Vec2::ZERO;
                cx.frame.border_radii.bottom_right = Vec2::ZERO;
            }
            cx.draw_outset_box_shadow(scene);
            if self.context.backgrounds {
                cx.draw_background(scene);
            }
            cx.draw_inset_box_shadow(scene);
            cx.draw_border(scene);
            // Over the box, as a block's outline is (see `render_element`).
            cx.draw_outline(scene);
        }
    }

    fn draw_inline_layout(&self, scene: &mut impl PaintScene, pos: Point) {
        if self.node.flags.is_inline_root() {
            let text_layout = self.element
                .inline_layout_data
                .as_ref()
                .unwrap_or_else(|| {
                    panic!("Tried to render node marked as inline root that does not have an inline layout: {:?}", self.node);
                });

            if let Some(block_lr) = self.node.vertical_block_axis() {
                self.draw_vertical_inline_layout(scene, text_layout, pos, block_lr);
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
            // behind the text and selection highlight.
            self.draw_inline_boxes(scene, &text_layout.layout, transform, tops);

            // Render text selection highlight (if any) using cached selection ranges
            if let Some(&(sel_start, sel_end)) = self.context.selection_ranges.get(&self.node.id) {
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
            let options = TextPaintOptions {
                scale: self.scale,
                brush_override: None,
                shadows: true,
            };
            match text_layout.ellipsis.as_deref() {
                None => crate::text::stroke_text(
                    scene,
                    text_layout.layout.lines(),
                    self.context.dom,
                    transform,
                    options,
                    self.node.id,
                    Some(&text_layout.text),
                    &mut draw_text_context,
                    None,
                ),
                Some(ellipsis) => {
                    // The lines were broken in the unrounded box: a line as wide as the box (a
                    // box sized to its text) must not count as overflowing it by a rounding.
                    let width = self.node.unrounded_layout().content_box_width() * self.scale as f32;
                    for line in text_layout.layout.lines() {
                        let truncation = truncate_line(&line, ellipsis, width, self.scale as f32, text_layout.layout.is_rtl());
                        draw_text_context.cut = truncation.map(|(cut, _)| cut);
                        crate::text::stroke_text(
                            scene,
                            std::iter::once(line.clone()),
                            self.context.dom,
                            transform,
                            options,
                            self.node.id,
                            Some(&text_layout.text),
                            &mut draw_text_context,
                            None,
                        );
                        draw_text_context.cut = None;
                        // The ellipsis on the line's baseline, next to the last atom kept.
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
                                Some("\u{2026}"),
                                &mut draw_text_context,
                                None,
                            );
                            draw_text_context.line_tops = tops;
                        }
                    }
                }
            }
            draw_text_context.text_shift = 0.0;
            draw_text_context.line_tops = None;
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
        let start = position.x as f64 + (layout.border.left + layout.padding.left) as f64;
        let painted_x = (position.x as f64).round() + (painted.border.left + painted.padding.left) as f64;
        let tops = crate::text::LineTops {
            origin: position.y as f64 + (layout.border.top + layout.padding.top) as f64 + block_offset,
            painted: (position.y as f64).round() + (painted.border.top + painted.padding.top) as f64 + block_offset,
            scale: self.scale,
        };
        (((start * 64.0).floor() / 64.0 - painted_x) * self.scale, Some(tops))
    }

    /// Paint an inline layout of a vertical writing mode. The layout's lines are logical: x is the
    /// inline axis and y the block axis, running from the content box's top and its block-start edge.
    /// Each line is drawn rotated a quarter turn clockwise (`vertical-*`'s sideways glyphs have their
    /// tops to the right), which takes y to -x. In `vertical-rl` that also runs the lines the right
    /// way. In `vertical-lr` they run the other way, so a line's slot is found first and the line is
    /// rotated within it.
    fn draw_vertical_inline_layout(
        &self,
        scene: &mut impl PaintScene,
        text_layout: &blitz_dom::node::TextLayout,
        pos: Point,
        block_lr: bool,
    ) {
        let block_offset = text_layout.block_offset as f64;
        let layout = self.node.final_layout();
        let content_width = (layout.size.width
            - layout.border.left
            - layout.border.right
            - layout.padding.left
            - layout.padding.right) as f64;
        let origin = if block_lr { pos.x + block_offset } else { pos.x + content_width - block_offset };
        // Boxes are painted at whole pixels. Chromium rounds text only in the block axis, so a
        // line starts where it was laid out in the inline axis.
        let inline_start = self.context.dom.physical_unrounded_geometry(self.node.id).0.y as f64;
        let top = pos.y + inline_start - inline_start.round();
        let line_transform = |index: usize| {
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
        };

        let wm = self.style.writing_mode;
        let sideways = wm.intersects(WritingMode::TEXT_SIDEWAYS | WritingMode::VERTICAL_SIDEWAYS);
        let all_upright = wm.contains(WritingMode::UPRIGHT);
        let vertical = |line: &parley::Line<'_, blitz_dom::node::TextBrush>| {
            Some(crate::text::VerticalLine::new(line, sideways, all_upright))
        };

        if self.context.backgrounds {
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
        if let Some(&(sel_start, sel_end)) = self.context.selection_ranges.get(&self.node.id) {
            crate::text::draw_text_selection(
                scene,
                &text_layout.layout,
                line_transform,
                sel_start,
                sel_end,
            );
        }
        let mut draw_text_context = self.context.draw_text_context.borrow_mut();
        for (index, line) in text_layout.layout.lines().enumerate() {
            let vertical = vertical(&line);
            crate::text::stroke_text(
                scene,
                std::iter::once(line),
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
            );
        }
    }

    fn draw_text_input_text(&self, scene: &mut impl PaintScene, pos: Point) {
        // Render the text in text inputs
        if let Some(input_data) = self.text_input {
            // For single-line inputs, add an offset to vertically center the text input layout
            // within the content box of it's node.
            let y_offset = self.node.text_input_v_centering_offset(self.scale);
            let subpixel_offset = input_data.content_box_subpixel_offset;
            // pyro: the value, placeholder and caret at the content box's layout-unit x, as
            // inline text (`fractional_text_x_of_node`).
            let content_origin = Point {
                x: pos.x + subpixel_offset.x as f64 + self.fractional_text_x_of_node() / self.scale,
                y: pos.y + subpixel_offset.y as f64,
            };
            let pos = Point {
                x: content_origin.x,
                y: content_origin.y + y_offset,
            };

            // Apply the scroll offset (stored in CSS pixels, scaled here to device pixels) so
            // that the caret stays visible. Single-line inputs scroll horizontally; multi-line
            // inputs scroll vertically.
            let scroll_offset = input_data.scroll_offset as f64 * self.scale;
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
                    );
                }
                draw_text_context.line_tops = None;
            }

            if self.node.is_focussed() {
                // Render selection/caret
                for (rect, _line_idx) in input_data.editor.selection_geometry().iter() {
                    scene.fill(
                        Fill::NonZero,
                        transform,
                        SELECTION_COLOR,
                        None,
                        &convert_rect(rect),
                    );
                }
                if let Some(cursor) = input_data.editor.cursor_geometry(1.5) {
                    let color = self.style.get_inherited_text().color;
                    let caret_color = match &self.style.get_inherited_ui().caret_color.0 {
                        ColorOrAuto::Auto => color,
                        ColorOrAuto::Color(caret_color) => caret_color.resolve_to_absolute(&color),
                    };

                    scene.fill(
                        Fill::NonZero,
                        transform,
                        caret_color.as_srgb_color(),
                        None,
                        &convert_rect(&cursor),
                    );
                };
            }

            // Render text
            let mut draw_text_context = self.context.draw_text_context.borrow_mut();
            draw_text_context.line_tops = tops;
            crate::text::stroke_text(
                scene,
                input_data.editor.try_layout().unwrap().lines(),
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
            );
            draw_text_context.line_tops = None;
        }
    }

    fn draw_marker(&self, scene: &mut impl PaintScene, pos: Point) {
        if let Some(ListItemLayout {
            marker,
            position: ListItemLayoutPosition::Outside(layout),
            content_box_subpixel_offset,
        }) = self.list_item
        {
            // Right align and pad the bullet when rendering outside
            let x_padding = match marker {
                Marker::Char(_) => 8.0,
                Marker::String(_) => 0.0,
            };
            // Outside markers are placed outside the list item's border box
            // (`pos` is the origin of its content box)
            let item_layout = self.node.final_layout();
            let x_offset = -(layout.full_width() / layout.scale()
                + x_padding
                + item_layout.padding.left
                + item_layout.border.left);

            // Align the marker with the baseline of the first line of text in the list item
            if let Marker::Char(symbol @ ('•' | '◦' | '▪')) = marker {
                self.draw_marker_symbol(scene, pos, *symbol, layout);
                return;
            }
            let first_line = self
                .element
                .inline_layout_data
                .as_ref()
                .and_then(|text_layout| Some((text_layout, text_layout.layout.lines().next()?)));
            let y_offset = if let Some((text_layout, first_text_line)) = &first_line
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
                x: pos.x + (content_box_subpixel_offset.x + x_offset) as f64,
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
            );
        }
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
        // The baseline from the item's content box top, in CSS px.
        let baseline = match &first_line {
            Some((node, text, line)) => {
                let shift = self
                    .text_placement_of(node, text.block_offset as f64)
                    .1
                    .map_or(0.0, |tops| tops.shift(line.metrics()));
                let layout = node.final_layout();
                let content_top = top + layout.border.top + layout.padding.top
                    - item_layout.border.top
                    - item_layout.padding.top;
                content_top + (line.metrics().baseline + shift as f32) / layout_scale + text.block_offset
            }
            None => marker_line.metrics().baseline / layout_scale,
        };
        let ascent = (font_ascent(&marker_line).unwrap_or(0.0) / layout_scale).round() as i32;
        let offset = ascent * 2 / 3;
        let size = ((offset + 1) / 2) as f64;
        let subpixel = self.list_item.map_or(taffy::Point::ZERO, |item| item.content_box_subpixel_offset);
        let x = pos.x + (subpixel.x - item_layout.padding.left - item_layout.border.left) as f64 - (offset + 7) as f64;
        let y = pos.y + (subpixel.y + baseline) as f64 - (ascent - 3 * (ascent - offset) / 2) as f64;
        let (x, y) = ((x + 0.5).floor() * self.scale, (y + 0.5).floor() * self.scale);
        let rect = Rect::new(x, y, x + size * self.scale, y + size * self.scale);
        let paint = anyrender::Paint::from(crate::text::text_fill_color(self.context.dom, self.node.id));
        match symbol {
            '•' => scene.fill(Fill::NonZero, self.transform, &paint, None, &kurbo::Ellipse::from_rect(rect)),
            '◦' => {
                scene.stroke(&Stroke::new(self.scale), self.transform, &paint, None, &kurbo::Ellipse::from_rect(rect))
            }
            _ => scene.fill(Fill::NonZero, self.transform, &paint, None, &rect),
        }
    }

    fn draw_children(
        &self,
        scene: &mut impl PaintScene,
        parent_style_transform: Affine,
        clip_rect: Rect,
    ) {
        // Negative z_index hoisted nodes

        if let Some(hoisted) = &self.node.stacking_context {
            for hoisted_child in hoisted.neg_z_hoisted_children() {
                let position = hoisted_child.position(self.node.tree(), self.node.id);
                let pos = kurbo::Vec2 {
                    x: position.x as f64 * self.scale,
                    y: position.y as f64 * self.scale,
                };
                self.render_node(
                    scene,
                    hoisted_child.node_id,
                    parent_style_transform.pre_translate(pos),
                    clip_rect,
                );
            }
        }

        // Regular children
        if let Some(children) = &*self.node.paint_children.borrow() {
            for child_id in children {
                // Fixed-position children of the root element are positioned against
                // the viewport and do not scroll with it, so cancel out the viewport
                // scroll (applied in `paint_scene`). A fixed box contained by a
                // transformed ancestor scrolls with it like any out-of-flow box.
                let child = &self.context.dom.as_ref().tree()[*child_id];
                let child_transform = if child.taffy_position() == taffy::Position::Fixed
                    && Some(self.node.id) == self.context.root_element_id
                {
                    let scroll = self.context.dom.as_ref().viewport_scroll();
                    parent_style_transform.pre_translate(kurbo::Vec2 {
                        x: scroll.x * self.scale,
                        y: scroll.y * self.scale,
                    })
                } else if let Some(offset) = child.lifted_paint_offset(self.node.id) {
                    parent_style_transform.pre_translate(kurbo::Vec2 {
                        x: offset.x as f64 * self.scale,
                        y: offset.y as f64 * self.scale,
                    })
                } else {
                    parent_style_transform
                };
                self.render_node(scene, *child_id, child_transform, clip_rect);
            }
        }

        // Positive z_index hoisted nodes
        if let Some(hoisted) = &self.node.stacking_context {
            for hoisted_child in hoisted.pos_z_hoisted_children() {
                let position = hoisted_child.position(self.node.tree(), self.node.id);
                let pos = kurbo::Vec2 {
                    x: position.x as f64 * self.scale,
                    y: position.y as f64 * self.scale,
                };
                self.render_node(
                    scene,
                    hoisted_child.node_id,
                    parent_style_transform.pre_translate(pos),
                    clip_rect,
                );
            }
        }
    }

    #[cfg(feature = "svg")]
    fn draw_svg(&self, scene: &mut impl PaintScene) {
        use style::properties::generated::longhands::object_fit::computed_value::T as ObjectFit;

        let Some(svg) = self.svg else {
            return;
        };

        let width = self.frame.content_box.width() as f32;
        let height = self.frame.content_box.height() as f32;
        // pyro: an inline `<svg>` without a `viewBox` lays its content out in its box.
        let sized = svg.sized(width / self.scale as f32, height / self.scale as f32);
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
        let object_size = taffy::Size {
            width: svg_size.width(),
            height: svg_size.height(),
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

        let viewport = svg_viewport_affine(svg, paint_size.width, paint_size.height);
        let transform = (self.transform * viewport).then_translate(Vec2 { x, y });

        self.with_replaced_content_clip(
            scene,
            container_size,
            paint_size,
            x_offset.px(),
            y_offset.px(),
            |scene| {
                anyrender_svg::render_svg_tree(scene, &svg.tree, transform);
            },
        );
    }

    /// Clip replaced content to the content box when its object-fit/object-position
    /// rect overflows it.
    fn with_replaced_content_clip<S: PaintScene>(
        &self,
        scene: &mut S,
        container_size: taffy::Size<f32>,
        paint_size: taffy::Size<f32>,
        x_offset: f32,
        y_offset: f32,
        paint: impl FnOnce(&mut S),
    ) {
        let overflows = x_offset < 0.0
            || y_offset < 0.0
            || x_offset + paint_size.width > container_size.width
            || y_offset + paint_size.height > container_size.height;
        self.context.layer_manager.maybe_with_layer(
            scene,
            overflows,
            BlendMode::default(),
            1.0,
            self.transform,
            &self.frame.content_box_path(),
            None,
            None,
            paint,
        );
    }

    fn draw_image(&self, scene: &mut impl PaintScene) {
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
            let object_size = taffy::Size {
                width: image.width as f32,
                height: image.height as f32,
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
            let transform = self
                .transform
                .pre_translate(Vec2 { x, y })
                .pre_scale_non_uniform(x_scale, y_scale);

            self.with_replaced_content_clip(
                scene,
                container_size,
                paint_size,
                x_offset.px(),
                y_offset.px(),
                |scene| {
                    scene.draw_image(to_peniko_image(image, quality).as_ref(), transform);
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

            // TODO: Support arbitrary transforms of subdocuments
            let translation = self.transform.translation();
            let initial_x = translation.x + self.frame.content_box.origin().x;
            let initial_y = translation.y + self.frame.content_box.origin().y;
            // let transform = self.transform.then_translate(Vec2 { x, y });

            let painter = BlitzDomPainter::new(
                &sub_doc,
                scale,
                width,
                height,
                initial_x,
                initial_y,
                self.custom_widget_scenes,
            );
            painter.paint_scene(scene);
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
    let border_radii = NonUniformRoundedRectRadii {
        top_left: resolve_radii(&s_border.border_top_left_radius),
        top_right: resolve_radii(&s_border.border_top_right_radius),
        bottom_right: resolve_radii(&s_border.border_bottom_right_radius),
        bottom_left: resolve_radii(&s_border.border_bottom_left_radius),
    };

    CssBox::new(border_box, border, padding, outline_width, border_radii)
}

/// pyro: what [`BlitzDomPainter::paint_element`] needs of `render_element`'s state.
#[derive(Clone, Copy)]
struct ElementPaint<'n> {
    node: &'n Node,
    paints_own_box: bool,
    should_clip: bool,
    is_text_input: bool,
    content_position: Point,
    child_clip_rect: Rect,
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
}
