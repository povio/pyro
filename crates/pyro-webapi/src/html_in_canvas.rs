//! HTML-in-Canvas (WICG html-in-canvas), events part: `canvas.requestPaint()`
//! and the `paint` event (`CanvasPaintEvent.changedElements`) of a
//! `<canvas content=drawable>`.
//!
//! Each animation frame, after its callbacks and style update
//! ([`run_paint_step`]), the changes the DOM journal recorded since the last
//! frame are attributed to the drawable canvases: a change inside a canvas's
//! subtree makes it owe a `paint` event, and the nearest element with the
//! `drawable` attribute above the change (inclusive, below that canvas and
//! still rendered) is one of its changed elements. A canvas newly connected
//! (or newly `content=drawable`) owes its initial paint, and `requestPaint()`
//! owes one without changes. Events fire in reverse tree order (descendant
//! canvases before their ancestors), as Chrome does.
//!
//! Everything is gated on `Dom::drawable_canvas_seen`: a document that never
//! had an element given `content=drawable` returns after one flag test, and
//! pages without a `<canvas content=drawable>` fire nothing and change nothing. Gaps:
//! changes the journal does not record (hover, scrolling, animations
//! without a DOM write) fire no `paint`; shadow trees are not searched for
//! canvases on a full rescan; a drawable canvas made only by `cloneNode` is
//! not noticed until some element is given `content=drawable`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::html_in_canvas::Touch;
use pyro_js::{JsError, Scope, Value};
use pyro_dom::{Dom, NodeId};

use crate::page::PageDom;

/// Per-document state.
#[derive(Default)]
pub(crate) struct State {
    /// The DOM version up to which changes were attributed (`None`: never).
    seen: Option<u64>,
    /// The connected `<canvas content=drawable>` elements known.
    canvases: Vec<NodeId>,
    /// Canvases owing a `paint` event, with their changed drawable elements.
    pending: Vec<(NodeId, Vec<NodeId>)>,
    /// Canvases a `paint` event was fired at since they became drawable:
    /// `drawElementImage` draws only into those.
    ready: Vec<NodeId>,
    /// The border box size (CSS px) of each drawable element of a canvas as of
    /// the canvas's last `paint` event: (canvas, element, width, height).
    painted_sizes: Vec<(NodeId, NodeId, f64, f64)>,
    /// `updateElementGeometry` transforms: (canvas, element, the matrix's 6
    /// (2D) or 16 values).
    transforms: Vec<(NodeId, NodeId, Vec<f64>)>,
    /// `updateElementGeometry` clips: (canvas, element, x, y, width, height), the rectangle in the
    /// coordinate space of the element's canvas transform (a negative size already normalised).
    clips: Vec<(NodeId, NodeId, [f64; 4])>,
    /// The elements hit testing finds in their canvas, oldest first: drawn
    /// (or given a transform) since; a later draw puts one last (on top).
    hittable: Vec<(NodeId, NodeId)>,
}

pub(crate) type Shared = RefCell<State>;

/// An HTML `<canvas>` whose `content` attribute is `drawable` (ASCII
/// case-insensitive).
pub fn is_drawable_canvas(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "canvas")
        && dom.get_attribute(el, "content").ok().flatten().is_some_and(|v| v.eq_ignore_ascii_case("drawable"))
}

fn is_drawable(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok() && dom.has_attribute(el, "drawable").unwrap_or(false)
}

/// The parent in the shadow-including tree: a shadow root's host, else the parent.
fn parent(dom: &Dom, n: NodeId) -> Option<NodeId> {
    let p = dom.parent(n).ok().flatten()?;
    if dom.is_shadow_root(p).unwrap_or(false) {
        return dom.fragment_host(p).ok().flatten();
    }
    Some(p)
}

/// The nearest drawable canvas above `el`.
fn owner_canvas(dom: &Dom, el: NodeId) -> Option<NodeId> {
    let mut cur = parent(dom, el);
    while let Some(a) = cur {
        if is_drawable_canvas(dom, a) {
            return Some(a);
        }
        cur = parent(dom, a);
    }
    None
}

impl State {
    fn owe(&mut self, canvas: NodeId, changed: Option<NodeId>) {
        let at = match self.pending.iter().position(|(c, _)| *c == canvas) {
            Some(at) => at,
            None => {
                self.pending.push((canvas, Vec::new()));
                self.pending.len() - 1
            }
        };
        if let Some(el) = changed {
            if !self.pending[at].1.contains(&el) {
                self.pending[at].1.push(el);
            }
        }
    }

    /// Adds the drawable canvases in `root`'s subtree (inclusive) not yet
    /// known; each owes its initial paint.
    fn discover(&mut self, dom: &Dom, root: NodeId) {
        let found = dom.descendants(root).into_iter().flatten().filter(|&n| is_drawable_canvas(dom, n));
        for canvas in found.collect::<Vec<_>>() {
            if !self.canvases.contains(&canvas) {
                self.canvases.push(canvas);
                self.owe(canvas, None);
            }
        }
    }

    /// A change at `node` (`children`: of its children): every drawable
    /// canvas above it owes a paint, and reports the nearest drawable
    /// element between the change and itself.
    fn attribute(&mut self, dom: &Dom, rendered: &dyn Fn(NodeId) -> bool, node: NodeId, children: bool) {
        let mut drawable = None;
        let mut cur = Some(node);
        while let Some(a) = cur {
            // A canvas's own attribute is no change of its content.
            if (a != node || children) && self.canvases.contains(&a) {
                self.owe(a, drawable.filter(|&d| rendered(d)));
                drawable = None;
            } else if drawable.is_none() && is_drawable(dom, a) {
                drawable = Some(a);
            }
            cur = parent(dom, a);
        }
    }
}

/// Whether the journal's touches are only insertions that were removed again before the frame: each
/// such element left two `Children` touches (its insertion and its removal) and one `Inserted` for a
/// node that is no longer connected, and nothing else was touched. The rendering is as it was.
fn cancelled_insertions(dom: &Dom, list: &[(NodeId, Touch)]) -> bool {
    let (mut children, mut gone) = (0usize, 0usize);
    for (node, touch) in list {
        match touch {
            Touch::Children => children += 1,
            Touch::Inserted if !dom.is_connected(*node).unwrap_or(false) => gone += 1,
            _ => return false,
        }
    }
    gone > 0 && children == 2 * gone
}

/// `canvas.requestPaint()`: the canvas owes a `paint` event in the next
/// frame, which is requested.
pub(crate) fn request_paint(page: &Rc<PageDom>, canvas: NodeId) {
    page.html_in_canvas.borrow_mut().owe(canvas, None);
    if let Some(hooks) = page.hooks() {
        hooks.request_animation_frame();
    }
}

/// The paint step of an animation frame, run after its callbacks and style
/// update: fires the `paint` events the drawable canvases owe.
pub fn run_paint_step(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    // A document that never had a drawable canvas (none created by the parser or given
    // content=drawable by setAttribute) has nothing to do: one flag test per frame. (A
    // requestPaint() is only owed by a drawable canvas, so it sets the flag first.)
    if !page.dom.borrow().drawable_canvas_seen() {
        return;
    }
    // HTML "update the rendering": the ResizeObserver step comes before the `paint` events
    // (WPT html-in-canvas `onpaint-fires-post-resize-observer`). (An IntersectionObserver's
    // observations are made at a task's end here, and delivered from a task of their own.)
    crate::resize_observer::rendering_step(page);
    let events = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        let mut state = page.html_in_canvas.borrow_mut();
        let doc = page.document();
        // Rendered: neither it nor an ancestor is `display: none`.
        let rendered = |el: NodeId| {
            let mut cur = Some(el);
            while let Some(a) = cur {
                if dom.element(a).is_ok()
                    && layout.style().computed_value(&dom, a, "display").as_deref() == Some("none")
                {
                    return false;
                }
                cur = parent(&dom, a);
            }
            dom.is_connected(el).unwrap_or(false)
        };
        let version = dom.version();
        let touched = match state.seen {
            Some(seen) if seen == version => Some(Vec::new()),
            Some(seen) => dom.touched_nodes_since(seen),
            None => None,
        };
        let first = state.seen.is_none();
        state.seen = Some(version);
        // Changes inside the document of a frame (an `<iframe>` in a canvas's subtree) are no node of
        // the canvas's subtree: the frame element stands for them.
        let mut in_frames: Vec<NodeId> = Vec::new();
        if let Some(list) = touched.as_ref().filter(|list| !list.is_empty() && !state.canvases.is_empty()) {
            for canvas in state.canvases.clone() {
                let frames: Vec<NodeId> = dom
                    .descendants(canvas)
                    .into_iter()
                    .flatten()
                    .filter(|&n| dom.element(n).is_ok_and(|e| e.is_html() && &*e.name.local == "iframe"))
                    .collect();
                for iframe in frames {
                    let Some(frame_doc) = page.frame_document(iframe) else { continue };
                    if list.iter().any(|(node, _)| dom.owner_document(*node).ok().flatten() == Some(frame_doc)) {
                        in_frames.push(iframe);
                    }
                }
            }
        }
        // An insertion undone within the frame (an element added and removed again) changed nothing.
        let touched = touched.map(|list| if cancelled_insertions(&dom, &list) { Vec::new() } else { list });
        match touched {
            None => {
                // First look, or the journal no longer covers the last one:
                // find the canvases anew; every known one may have changed.
                if dom.canvas_seen() {
                    if !first {
                        for canvas in state.canvases.clone() {
                            state.owe(canvas, None);
                        }
                    }
                    state.discover(&dom, doc);
                }
            }
            Some(list) => {
                for (node, touch) in list {
                    match touch {
                        Touch::Inserted => state.discover(&dom, node),
                        Touch::Node if dom.element(node).is_ok_and(|e| e.is_html() && &*e.name.local == "canvas") => {
                            if is_drawable_canvas(&dom, node) {
                                state.discover(&dom, node);
                            }
                            state.attribute(&dom, &rendered, node, false);
                        }
                        Touch::Node => state.attribute(&dom, &rendered, node, false),
                        Touch::Children => state.attribute(&dom, &rendered, node, true),
                    }
                }
            }
        }
        // Forget canvases that left the document or stopped being drawable
        // (one reconnected later counts as new).
        state.canvases.retain(|&c| dom.is_connected(c).unwrap_or(false) && is_drawable_canvas(&dom, c));
        for iframe in in_frames {
            state.attribute(&dom, &rendered, iframe, false);
        }
        // A running CSS animation or transition repaints its element each frame, with no write to
        // the DOM for the journal to show (a paused or finished one does not).
        if !state.canvases.is_empty() {
            let moving: Vec<NodeId> = page
                .animation_events
                .try_timelines()
                .map(|(animations, transitions)| {
                    animations
                        .iter()
                        .chain(transitions.iter())
                        .filter(|t| t.is_live() && !t.has_ended() && !t.is_paused())
                        .map(|t| t.el())
                        .collect()
                })
                .unwrap_or_default();
            for el in moving {
                if dom.is_connected(el).unwrap_or(false) {
                    state.attribute(&dom, &rendered, el, false);
                }
            }
        }
        let known = state.canvases.clone();
        state.ready.retain(|c| known.contains(c));
        let mut events: Vec<(NodeId, Vec<NodeId>)> = std::mem::take(&mut state.pending)
            .into_iter()
            .filter(|(c, _)| dom.is_connected(*c).unwrap_or(false) && is_drawable_canvas(&dom, *c))
            .collect();
        let precedes = |a: NodeId, b: NodeId| dom.compare_document_position(b, a).is_ok_and(|p| p & 0x02 != 0);
        // Reverse tree order: a canvas after another (or inside it) first.
        events.sort_by(|(a, _), (b, _)| {
            if a == b {
                std::cmp::Ordering::Equal
            } else if precedes(*a, *b) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        });
        for (_, changed) in events.iter_mut() {
            changed.sort_by(|a, b| {
                if a == b {
                    std::cmp::Ordering::Equal
                } else if precedes(*a, *b) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            });
        }
        // The sizes `captureElementImage` and `getElementImageDefaultSize`
        // report until the next paint: each drawable element's, now.
        for (canvas, _) in &events {
            let canvas = *canvas;
            state.painted_sizes.retain(|(c, _, _, _)| *c != canvas);
            let elements: Vec<NodeId> = dom
                .descendants(canvas)
                .into_iter()
                .flatten()
                .filter(|&el| el != canvas && is_drawable(&dom, el) && owner_canvas(&dom, el) == Some(canvas))
                .collect();
            for el in elements {
                let rect = layout.bounding_client_rect(&dom, el);
                state.painted_sizes.push((canvas, el, rect.width, rect.height));
            }
        }
        events
    };
    for (canvas, changed) in events {
        {
            let mut state = page.html_in_canvas.borrow_mut();
            if !state.ready.contains(&canvas) {
                state.ready.push(canvas);
            }
        }
        let ev = crate::event::EventCore::trusted(page, crate::event::EventInterface::CanvasPaintEvent, "paint", false, false);
        ev.fields.borrow_mut().changed_elements = changed;
        crate::target::fire_at_node(page, canvas, &ev);
    }
}

/// What `drawElementImage` draws: the element painted alone at its CSS size
/// (blitz `paint_element_subtree`), as straight sRGB RGBA, one pixel per CSS px.
pub(crate) struct ElementImage {
    /// The element's border box in CSS px.
    pub width: f64,
    pub height: f64,
    /// The pixels (`None`: nothing to paint, an empty box).
    pub pixels: Option<(u32, u32, Vec<u8>)>,
    /// The paint may hold content the page must not read back (an image of unknown origin, a
    /// nested document): drawing it makes the canvas not origin-clean.
    pub taints: bool,
}

fn invalid_state(message: &str) -> pyro_js::JsError {
    pyro_js::JsError::dom("InvalidStateError", format!("Failed to execute 'drawElementImage': {message}"))
}

/// HTML-in-Canvas: the image `drawElementImage` draws of `element` into the
/// 2D context of `canvas`. InvalidStateError when the canvas's content is not
/// drawable or it had no `paint` event since it became drawable, when
/// `element` is not a drawable element of it (its nearest drawable canvas)
/// or is in the top layer below it, or when either is not rendered
/// (`display: none`).
pub(crate) fn element_image(page: &Rc<PageDom>, canvas: NodeId, element: NodeId) -> pyro_js::JsResult<ElementImage> {
    // An element holding a frame (`<iframe>`, `<object>`, `<embed>`) is painted with the document
    // the frame shows, which the renderer's snapshot has only once the embedder gave it: that is
    // asked first, with no borrow of the DOM held.
    let owners: Vec<NodeId> = {
        let dom = page.dom.borrow();
        dom.descendants(element)
            .map(|nodes| {
                nodes
                    .filter(|&node| dom.element(node).is_ok_and(|e| e.is_html() && matches!(&*e.name.local, "iframe" | "object" | "embed" | "frame")))
                    .collect()
            })
            .unwrap_or_default()
    };
    if !owners.is_empty() && let Some(hooks) = page.hooks() {
        hooks.refresh_frame_documents(&owners);
    }
    let layout = page.layout();
    let dom = page.dom.borrow();
    check_painted(page, &layout, &dom, canvas, element)?;
    // (The rect query syncs the renderer's document, which the painting below reads.)
    let rect = layout.bounding_client_rect(&dom, element);
    let key = pyro_dom::snapshot::render_key(element);
    let painted = layout.synced_geometry_source(&dom).and_then(|source| source.element_image(key, 1.0));
    // Its border box as laid out (a transformed element is drawn untransformed, so its bounding
    // rect would stretch it); an element without one to paint keeps its bounding rect's size.
    Ok(match painted {
        Some(image) => ElementImage {
            width: image.width,
            height: image.height,
            pixels: Some((image.pixel_width, image.pixel_height, image.rgba)),
            taints: image.taints,
        },
        None => ElementImage { width: rect.width, height: rect.height, pixels: None, taints: false },
    })
}

/// `element` is a drawable element of the drawable `canvas` (its nearest
/// drawable canvas), connected, with no top-layer element in between.
fn check_owned(dom: &Dom, canvas: NodeId, element: NodeId) -> pyro_js::JsResult<()> {
    if !is_drawable_canvas(dom, canvas) {
        return Err(invalid_state("The canvas's content is not drawable."));
    }
    if !dom.is_connected(element).unwrap_or(false) || !is_drawable(dom, element) {
        return Err(invalid_state("The element is not drawable."));
    }
    let mut cur = parent(dom, element);
    loop {
        let Some(a) = cur else { return Err(invalid_state("The element is not a child of the canvas.")) };
        if a == canvas {
            return Ok(());
        }
        if is_drawable_canvas(dom, a) {
            return Err(invalid_state("The element is not a child of the canvas."));
        }
        if dom.is_modal(a) || dom.is_popover_showing(a) {
            return Err(invalid_state("The element is in the top layer."));
        }
        cur = parent(dom, a);
    }
}

/// [`check_owned`], and the canvas had its `paint` event and the element is
/// rendered (neither it nor an ancestor is `display: none`).
fn check_painted(
    page: &Rc<PageDom>, layout: &pyro_dom::layout::Layout, dom: &Dom, canvas: NodeId, element: NodeId,
) -> pyro_js::JsResult<()> {
    if !is_drawable_canvas(dom, canvas) || !page.html_in_canvas.borrow().ready.contains(&canvas) {
        return Err(invalid_state("The canvas has no drawable content painted yet."));
    }
    check_owned(dom, canvas, element)?;
    let mut cur = Some(element);
    while let Some(a) = cur {
        if dom.element(a).is_ok() && layout.style().computed_value(dom, a, "display").as_deref() == Some("none") {
            return Err(invalid_state("The element is not rendered."));
        }
        cur = parent(dom, a);
    }
    Ok(())
}

/// HTML-in-Canvas `ElementImage`: what `captureElementImage` returns, the
/// element's paint and size as captured, which `drawElementImage` and
/// `getElementImageDefaultSize` accept in place of the element.
pub struct ElementImageClass;
impl pyro_js::JsClass for ElementImageClass {
    const NAME: &'static str = "ElementImage";
    type Data = ElementSnapshot;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut pyro_js::ClassDef<Self>) {
        // `close()`: the image is released; drawing it or asking for its size throws
        // InvalidStateError from then on (the pixels are dropped with the wrapper).
        def.method("close", 0, |s, image, _a| {
            image.closed.set(true);
            Ok(s.undefined())
        });
    }
}

/// The data of an [`ElementImageClass`].
pub struct ElementSnapshot {
    /// The element's size in CSS px as of the canvas's last paint.
    pub width: f64,
    pub height: f64,
    pub pixels: Option<(u32, u32, Rc<Vec<u8>>)>,
    pub taints: bool,
    /// `close()` was called.
    pub closed: Cell<bool>,
    /// The canvas it was captured from ([`owner_key`]): only that canvas, or an OffscreenCanvas
    /// holding its control, may draw or measure it.
    pub owner: (usize, NodeId),
    /// The element it was captured from: `updateElementGeometry` and `clearElementGeometry` take
    /// the ElementImage for it.
    pub element: NodeId,
}

/// `canvas` of `page` as an [`ElementSnapshot`] records its owner.
pub(crate) fn owner_key(page: &PageDom, canvas: NodeId) -> (usize, NodeId) {
    (Rc::as_ptr(&page.dom) as *const () as usize, canvas)
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<ElementImageClass>();
}

/// The element's size as of its canvas's last paint (its size now when none was recorded).
fn painted_size(page: &PageDom, layout: &pyro_dom::layout::Layout, dom: &Dom, canvas: NodeId, element: NodeId) -> (f64, f64) {
    let recorded = page
        .html_in_canvas
        .borrow()
        .painted_sizes
        .iter()
        .find(|(c, e, _, _)| *c == canvas && *e == element)
        .map(|&(_, _, w, h)| (w, h));
    recorded.unwrap_or_else(|| {
        let rect = layout.bounding_client_rect(dom, element);
        (rect.width, rect.height)
    })
}

/// The canvas grid's units per CSS px: its bitmap size over its box's size.
fn grid_scale(layout: &pyro_dom::layout::Layout, dom: &Dom, canvas: NodeId) -> (f64, f64) {
    let bitmap = |name: &str, default: f64| {
        dom.get_attribute(canvas, name)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map_or(default, f64::from)
    };
    let rect = layout.bounding_client_rect(dom, canvas);
    let scale = |bitmap: f64, css: f64| if css > 0.0 { bitmap / css } else { 1.0 };
    (scale(bitmap("width", 300.0), rect.width), scale(bitmap("height", 150.0), rect.height))
}

/// `canvas.captureElementImage(element)`.
pub(crate) fn capture<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, element: NodeId) -> pyro_js::JsResult<Value<'s>> {
    let image = element_image(page, canvas, element)?;
    let (width, height) = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        painted_size(page, &layout, &dom, canvas, element)
    };
    let snapshot = ElementSnapshot {
        width,
        height,
        pixels: image.pixels.map(|(w, h, data)| (w, h, Rc::new(data))),
        taints: image.taints,
        closed: Cell::new(false),
        owner: owner_key(page, canvas),
        element,
    };
    Ok(s.wrap::<ElementImageClass>(snapshot, None)?.value())
}

/// `canvas.getElementImageDefaultSize(element or elementImage)`: its size as
/// of the canvas's last paint in the canvas grid's units.
pub(crate) fn default_size<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, target: Value<'s>) -> pyro_js::JsResult<Value<'s>> {
    let layout = page.layout();
    let dom = page.dom.borrow();
    let (width, height) = match s.unwrap::<ElementImageClass>(target) {
        Some(image) => {
            if !is_drawable_canvas(&dom, canvas) {
                return Err(invalid_state("The canvas's content is not drawable."));
            }
            if image.closed.get() {
                return Err(invalid_state("The ElementImage is closed."));
            }
            if image.owner != owner_key(page, canvas) {
                return Err(invalid_state("The ElementImage was captured from a different canvas."));
            }
            (image.width, image.height)
        }
        None => {
            let element = element_of(s, page, target)?;
            check_painted(page, &layout, &dom, canvas, element)?;
            painted_size(page, &layout, &dom, canvas, element)
        }
    };
    let (sx, sy) = grid_scale(&layout, &dom, canvas);
    let out = s.object();
    out.set_atom("width", s.number(width * sx))?;
    out.set_atom("height", s.number(height * sy))?;
    Ok(out.value())
}

/// `OffscreenCanvas.getElementImageDefaultSize(elementImage)`: an ElementImage of the canvas whose
/// control this OffscreenCanvas took (`canvas`), in that canvas grid's units. An Element is the
/// canvas element's to measure, and a standalone OffscreenCanvas has no canvas: InvalidStateError.
pub(crate) fn offscreen_default_size<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: Option<NodeId>, target: Value<'s>) -> pyro_js::JsResult<Value<'s>> {
    let Some(image) = s.unwrap::<ElementImageClass>(target) else {
        return Err(match element_of(s, page, target) {
            Ok(_) => invalid_state("An Element can only be measured by its <canvas>."),
            Err(error) => error,
        });
    };
    let Some(canvas) = canvas else { return Err(invalid_state("The OffscreenCanvas has no canvas to take the ElementImage from.")) };
    let layout = page.layout();
    let dom = page.dom.borrow();
    if !is_drawable_canvas(&dom, canvas) {
        return Err(invalid_state("The canvas's content is not drawable."));
    }
    if image.closed.get() {
        return Err(invalid_state("The ElementImage is closed."));
    }
    if image.owner != owner_key(page, canvas) {
        return Err(invalid_state("The ElementImage was captured from a different canvas."));
    }
    let (sx, sy) = grid_scale(&layout, &dom, canvas);
    let out = s.object();
    out.set_atom("width", s.number(image.width * sx))?;
    out.set_atom("height", s.number(image.height * sy))?;
    Ok(out.value())
}

/// The element `value` is, of this page.
pub(crate) fn element_of(s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>) -> pyro_js::JsResult<NodeId> {
    s.unwrap::<crate::node::Node>(value)
        .filter(|node| Rc::ptr_eq(&node.page().dom, &page.dom) && node.page().dom.borrow().element(node.id).is_ok())
        .map(|node| node.id)
        .ok_or_else(|| JsError::Type("The provided value is not of type 'Element'.".into()))
}

fn dom_matrix<'s>(s: &Scope<'s>) -> pyro_js::JsResult<pyro_js::Object<'s>> {
    s.global().get_atom("DOMMatrix")?.as_object().ok_or_else(|| JsError::Type("DOMMatrix is not available.".into()))
}

/// `canvas.getElementTransform(element)`: a new DOMMatrix of the transform
/// `updateElementGeometry` set (identity without one).
pub(crate) fn get_transform<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, element: NodeId) -> pyro_js::JsResult<Value<'s>> {
    check_owned(&page.dom.borrow(), canvas, element)?;
    let values = page
        .html_in_canvas
        .borrow()
        .transforms
        .iter()
        .find(|(c, e, _)| *c == canvas && *e == element)
        .map(|(_, _, m)| m.clone());
    let ctor = dom_matrix(s)?;
    let matrix = match values {
        Some(values) => {
            let items: Vec<Value<'_>> = values.iter().map(|&v| s.number(v)).collect();
            ctor.construct(&[s.array(&items)?.value()])?
        }
        None => ctor.construct(&[])?,
    };
    Ok(matrix.value())
}

/// The element `updateElementGeometry` and `clearElementGeometry` take: an Element of this page, or an
/// ElementImage captured from `canvas` (the element it captured).
pub(crate) fn geometry_target(s: &Scope<'_>, page: &Rc<PageDom>, canvas: NodeId, value: Value<'_>) -> pyro_js::JsResult<NodeId> {
    if let Some(image) = s.unwrap::<ElementImageClass>(value) {
        if image.owner != owner_key(page, canvas) {
            return Err(JsError::dom("InvalidStateError", "The ElementImage was captured from a different canvas."));
        }
        return Ok(image.element);
    }
    element_of(s, page, value)
}

/// A member of a DOMRectInit: its number, 0 when absent or not finite.
fn rect_member(init: &pyro_js::Object<'_>, name: &'static str) -> pyro_js::JsResult<f64> {
    let value = init.get_atom(name)?;
    if value.is_undefined() {
        return Ok(0.0);
    }
    let number = value.to_number()?;
    Ok(if number.is_finite() { number } else { 0.0 })
}

/// `canvas.updateElementGeometry(element, {canvasTransform, canvasClip, preserveHitTestOrder})`: keeps
/// the transform (a DOMMatrixInit, validated as `DOMMatrix.fromMatrix` does; kept as it was when
/// omitted) and the clip (a DOMRectInit in the coordinate space of the transform, its negative size
/// normalised; kept when omitted), and puts the element on top of the hit testing order unless
/// `preserveHitTestOrder` and it is in it already.
pub(crate) fn update_geometry(s: &Scope<'_>, page: &Rc<PageDom>, canvas: NodeId, element: NodeId, options: Value<'_>) -> pyro_js::JsResult<()> {
    let object = options.as_object();
    let (init, clip_init, preserve_order) = match &object {
        Some(o) => (o.get_atom("canvasTransform")?, o.get_atom("canvasClip")?, o.get_atom("preserveHitTestOrder")?.to_bool()),
        None => (s.undefined(), s.undefined(), false),
    };
    let matrix = if init.is_undefined() {
        None
    } else {
        let ctor = dom_matrix(s)?;
        let from = ctor.get_atom("fromMatrix")?.as_object().ok_or_else(|| JsError::Type("DOMMatrix.fromMatrix is not available.".into()))?;
        Some(from.call(Some(ctor), &[init])?)
    };
    let clip = match clip_init.as_object() {
        Some(rect) => {
            let (mut x, mut y, mut w, mut h) = (rect_member(&rect, "x")?, rect_member(&rect, "y")?, rect_member(&rect, "width")?, rect_member(&rect, "height")?);
            if w < 0.0 {
                x += w;
                w = -w;
            }
            if h < 0.0 {
                y += h;
                h = -h;
            }
            Some([x, y, w, h])
        }
        None => None,
    };
    check_owned(&page.dom.borrow(), canvas, element)?;
    let values = match matrix {
        Some(matrix) => {
            let matrix = matrix.as_object().ok_or_else(|| JsError::Type("DOMMatrix.fromMatrix returned no matrix.".into()))?;
            let is_2d = matrix.get_atom("is2D")?.to_bool();
            let array = matrix.get_atom("toFloat64Array")?.as_object().ok_or_else(|| JsError::Type("toFloat64Array is not available.".into()))?.call(Some(matrix), &[])?;
            let array = array.as_object().ok_or_else(|| JsError::Type("toFloat64Array returned no array.".into()))?;
            let all: Vec<f64> = (0..16).map(|i| array.get_index(i).and_then(|v| v.to_number())).collect::<pyro_js::JsResult<_>>()?;
            // A 2D matrix as its six values (a, b, c, d, e, f): read back as 2D.
            Some(if is_2d { vec![all[0], all[1], all[4], all[5], all[12], all[13]] } else { all })
        }
        None => None,
    };
    // Hittable only once the geometry is accepted (a throwing call above records nothing); an
    // element already in the order keeps its place when asked to.
    let in_order = page.html_in_canvas.borrow().hittable.iter().any(|&(c, e)| c == canvas && e == element);
    if !(preserve_order && in_order) {
        note_drawn(page, canvas, element);
    }
    let mut state = page.html_in_canvas.borrow_mut();
    let has_transform = state.transforms.iter().any(|(c, e, _)| *c == canvas && *e == element);
    match values {
        Some(values) => {
            state.transforms.retain(|(c, e, _)| !(*c == canvas && *e == element));
            state.transforms.push((canvas, element, values));
        }
        // (Omitted: the transform it has stays; an element without one gets the identity.)
        None if !has_transform => state.transforms.push((canvas, element, vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        None => {}
    }
    if let Some(clip) = clip {
        state.clips.retain(|(c, e, _)| !(*c == canvas && *e == element));
        state.clips.push((canvas, element, clip));
    }
    Ok(())
}

/// `canvas.clearElementGeometry(element)`: its transform and clip, and its place in the hit testing
/// order, are gone.
pub(crate) fn clear_geometry(page: &Rc<PageDom>, canvas: NodeId, element: NodeId) -> pyro_js::JsResult<()> {
    check_owned(&page.dom.borrow(), canvas, element)?;
    let mut state = page.html_in_canvas.borrow_mut();
    state.transforms.retain(|(c, e, _)| !(*c == canvas && *e == element));
    state.clips.retain(|(c, e, _)| !(*c == canvas && *e == element));
    state.hittable.retain(|&(c, e)| !(c == canvas && e == element));
    Ok(())
}

/// `drawElementImage` (without `preserveElementGeometry`): the element's
/// canvas transform becomes where it was drawn. `values` (6, 2D) are in the
/// canvas grid (bitmap px); the element's geometry is in CSS px of the
/// canvas's box, as `updateElementGeometry` transforms are (WPT
/// canvas-transform-get-bounding-client-rect maps those unscaled on a canvas
/// whose bitmap is not its CSS size), so the grid's scale is taken out.
/// `drawElementImage` drew `element` into `canvas`: hit testing finds it
/// there, above what was drawn before.
pub(crate) fn note_drawn(page: &PageDom, canvas: NodeId, element: NodeId) {
    let mut state = page.html_in_canvas.borrow_mut();
    state.hittable.retain(|&(c, e)| !(c == canvas && e == element));
    state.hittable.push((canvas, element));
}

pub(crate) fn set_drawn_transform(page: &Rc<PageDom>, canvas: NodeId, element: NodeId, values: Vec<f64>) {
    let (sx, sy) = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        let (sx, sy) = grid_scale(&layout, &dom, canvas);
        // The canvas box's rect holds the zoom of the canvas; the grid's scale
        // per CSS px of the element does not (WPT canvas-transform-update, "not affected by zoom").
        let zoom = effective_zoom(&layout, &dom, canvas);
        (sx * zoom, sy * zoom)
    };
    let (sx, sy) = (if sx > 0.0 { sx } else { 1.0 }, if sy > 0.0 { sy } else { 1.0 });
    // The transform in the element's CSS px: the grid transform conjugated by the grid's scale
    // (S^-1 * M * S), so an element drawn at its default size, which is its CSS size in grid
    // units, is not scaled, and a translation is in CSS px.
    let values = if values.len() == 6 {
        let v = &values;
        vec![v[0], v[1] * sx / sy, v[2] * sy / sx, v[3], v[4] / sx, v[5] / sy]
    } else {
        values
    };
    let mut state = page.html_in_canvas.borrow_mut();
    state.transforms.retain(|(c, e, _)| !(*c == canvas && *e == element));
    state.transforms.push((canvas, element, values));
}

/// The computed `zoom` of `el` (1 where the engine reports none). Not its ancestors': `zoom` is
/// not inherited.
fn effective_zoom(layout: &pyro_dom::layout::Layout, dom: &Dom, el: NodeId) -> f64 {
    layout
        .style()
        .computed_value(dom, el, "zoom")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|z| z.is_finite() && *z > 0.0)
        .unwrap_or(1.0)
}

/// HTML-in-Canvas geometry: the client rect `rect` of `el` as its canvas
/// transform places it. The element's box (with its own CSS transforms,
/// relative to its untransformed border-box origin) is mapped by the canvas
/// transform into the canvas's content box; its laid-out position in the
/// canvas no longer counts. `rect` itself without a transform.
pub(crate) fn transformed_rect(page: &PageDom, el: NodeId, rect: pyro_dom::layout::Rect) -> pyro_dom::layout::Rect {
    let found = {
        let state = page.html_in_canvas.borrow();
        if state.transforms.is_empty() {
            return rect;
        }
        state.transforms.iter().find(|(_, e, _)| *e == el).map(|(c, _, v)| (*c, v.clone()))
    };
    let Some((canvas, values)) = found else { return rect };
    let layout = page.layout();
    let dom = page.dom.borrow();
    if !is_drawable_canvas(&dom, canvas) || !dom.is_connected(el).unwrap_or(false) {
        return rect;
    }
    use pyro_dom::geometry::{LEFT, TOP};
    let source = layout.synced_geometry_source(&dom);
    let element_box = |id: NodeId| source.and_then(|source| source.element_box(pyro_dom::snapshot::render_key(id)));
    // The canvas's content box origin in client coordinates.
    let canvas_rect = layout.bounding_client_rect(&dom, canvas);
    let (inset_x, inset_y) =
        element_box(canvas).map_or((0.0, 0.0), |b| (b.border[LEFT] + b.padding[LEFT], b.border[TOP] + b.padding[TOP]));
    let origin = (canvas_rect.x + inset_x, canvas_rect.y + inset_y);
    // Where the element's CSS-transformed box starts relative to its untransformed origin.
    let local = element_box(el).map_or((0.0, 0.0), |b| match b.layout_origin {
        Some([ux, uy]) => (b.x - ux, b.y - uy),
        None => (0.0, 0.0),
    });
    // The matrix's 2D part: (a, b, c, d, e, f), from 6 values or the 16 of a 3D matrix.
    let [a, b, c, d, e, f] = match values.len() {
        6 => [values[0], values[1], values[2], values[3], values[4], values[5]],
        16 => [values[0], values[1], values[4], values[5], values[12], values[13]],
        _ => return rect,
    };
    let map = |x: f64, y: f64| (a * x + c * y + e, b * x + d * y + f);
    let (x0, y0, x1, y1) = (local.0, local.1, local.0 + rect.width, local.1 + rect.height);
    let corners = [map(x0, y0), map(x1, y0), map(x0, y1), map(x1, y1)];
    let min_x = corners.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let min_y = corners.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_x = corners.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let max_y = corners.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    pyro_dom::layout::Rect { x: origin.0 + min_x, y: origin.1 + min_y, width: max_x - min_x, height: max_y - min_y }
}

/// Whether client point (`x`, `y`) is inside the clip `updateElementGeometry` gave `element` (true
/// without one): the point in the element's own coordinate space, which its canvas transform maps
/// from (`laid_out`, its rect as laid out, stands for the transform when it has none).
fn inside_clip(page: &PageDom, canvas: NodeId, element: NodeId, laid_out: &pyro_dom::layout::Rect, x: f64, y: f64) -> bool {
    let (clip, values) = {
        let state = page.html_in_canvas.borrow();
        let Some(&(_, _, clip)) = state.clips.iter().find(|(c, e, _)| *c == canvas && *e == element) else { return true };
        (clip, state.transforms.iter().find(|(c, e, _)| *c == canvas && *e == element).map(|(_, _, v)| v.clone()))
    };
    let local = match values {
        None => (x - laid_out.x, y - laid_out.y),
        Some(values) => {
            let [a, b, c, d, e, f] = match values.len() {
                6 => [values[0], values[1], values[2], values[3], values[4], values[5]],
                16 => [values[0], values[1], values[4], values[5], values[12], values[13]],
                _ => return false,
            };
            use pyro_dom::geometry::{LEFT, TOP};
            let layout = page.layout();
            let dom = page.dom.borrow();
            let source = layout.synced_geometry_source(&dom);
            let canvas_rect = layout.bounding_client_rect(&dom, canvas);
            let (inset_x, inset_y) = source
                .and_then(|source| source.element_box(pyro_dom::snapshot::render_key(canvas)))
                .map_or((0.0, 0.0), |b| (b.border[LEFT] + b.padding[LEFT], b.border[TOP] + b.padding[TOP]));
            let (px, py) = (x - (canvas_rect.x + inset_x) - e, y - (canvas_rect.y + inset_y) - f);
            let det = a * d - b * c;
            if det.abs() < 1e-12 {
                return false;
            }
            ((d * px - c * py) / det, (-b * px + a * py) / det)
        }
    };
    let [cx, cy, cw, ch] = clip;
    local.0 >= cx && local.0 < cx + cw && local.1 >= cy && local.1 < cy + ch
}

/// HTML-in-Canvas hit testing over `hits` (topmost first) at client point
/// (`x`, `y`): a canvas's children are not hit where they are laid out; the
/// elements drawn into a drawable canvas (or given a canvas transform) are hit
/// where their canvas transform places them, the last drawn on top, just
/// above their canvas, unless `pointer-events: none` or inert. `hits` as they
/// are on a page without a drawable canvas.
pub(crate) fn adjust_hits(page: &PageDom, x: f64, y: f64, hits: Vec<NodeId>) -> Vec<NodeId> {
    if !page.dom.borrow().drawable_canvas_seen() {
        return hits;
    }
    let candidates: Vec<(NodeId, NodeId)> = page.html_in_canvas.borrow().hittable.iter().rev().copied().collect();
    let mut placed: Vec<(NodeId, NodeId)> = Vec::new();
    for (canvas, element) in candidates {
        let rect = {
            let layout = page.layout();
            let dom = page.dom.borrow();
            if !dom.is_connected(element).unwrap_or(false)
                || owner_canvas(&dom, element) != Some(canvas)
                || layout.style().is_inert(&dom, element)
                || layout.style().computed_value(&dom, element, "pointer-events").as_deref() == Some("none")
            {
                continue;
            }
            layout.bounding_client_rect(&dom, element)
        };
        let laid_out = rect;
        let rect = transformed_rect(page, element, rect);
        if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height && inside_clip(page, canvas, element, &laid_out, x, y) {
            placed.push((canvas, element));
        }
    }
    let dom = page.dom.borrow();
    let mut out = Vec::with_capacity(hits.len() + placed.len());
    for hit in hits {
        // A canvas's child laid out under the point is not hit there.
        if owner_canvas(&dom, hit).is_some() {
            continue;
        }
        if is_drawable_canvas(&dom, hit) {
            out.extend(placed.iter().filter(|(c, _)| *c == hit).map(|(_, e)| *e));
        }
        out.push(hit);
    }
    out
}
