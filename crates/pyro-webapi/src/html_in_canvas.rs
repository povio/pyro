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
//! Everything is gated on a document that has a canvas: pages without a
//! `<canvas content=drawable>` fire nothing and change nothing. Gaps:
//! changes the journal does not record (hover, scrolling, animations
//! without a DOM write) fire no `paint`; shadow trees are not searched for
//! canvases on a full rescan.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::html_in_canvas::Touch;
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
    // A document that never had a canvas, and no requestPaint() owed: nothing to do.
    if !page.dom.borrow().canvas_seen() && page.html_in_canvas.borrow().pending.is_empty() {
        return;
    }
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
    let layout = page.layout();
    let dom = page.dom.borrow();
    if !is_drawable_canvas(&dom, canvas) || !page.html_in_canvas.borrow().ready.contains(&canvas) {
        return Err(invalid_state("The canvas has no drawable content painted yet."));
    }
    if !is_drawable(&dom, element) {
        return Err(invalid_state("The element is not drawable."));
    }
    // `element`'s nearest drawable canvas above it must be `canvas`, with no
    // top-layer element (a modal dialog, an open popover) in between.
    let mut cur = parent(&dom, element);
    loop {
        let Some(a) = cur else { return Err(invalid_state("The element is not a child of the canvas.")) };
        if a == canvas {
            break;
        }
        if is_drawable_canvas(&dom, a) {
            return Err(invalid_state("The element is not a child of the canvas."));
        }
        if dom.is_modal(a) || dom.is_popover_showing(a) {
            return Err(invalid_state("The element is in the top layer."));
        }
        cur = parent(&dom, a);
    }
    // Rendered: neither it nor an ancestor (the canvas included) is `display: none`.
    let mut cur = Some(element);
    while let Some(a) = cur {
        if dom.element(a).is_ok() && layout.style().computed_value(&dom, a, "display").as_deref() == Some("none") {
            return Err(invalid_state("The element is not rendered."));
        }
        cur = parent(&dom, a);
    }
    let rect = layout.bounding_client_rect(&dom, element);
    let key = pyro_dom::snapshot::render_key(element);
    let pixels = layout.synced_geometry_source(&dom).and_then(|source| source.element_image(key, 1.0));
    Ok(ElementImage { width: rect.width, height: rect.height, pixels })
}
