//! Paint Timing: `first-paint` and `first-contentful-paint`, decided at a real rendering
//! update (frame) from what the document shows.
//!
//! # Definition (headless)
//!
//! Nothing is rasterized, so a "paint" is the rendering update in which the document is in
//! the state that would be painted, and its presentation is the end of that update
//! ([`FrameInfo::presented`]; `paintTime` is [`FrameInfo::paint_time`], when layout was final).
//! Both entries carry `startTime == presentationTime`.
//!
//! - `first-contentful-paint`: the first frame in which the document's own content paints
//!   something contentful: the shared scan's candidates ([`scan::painted_candidates`]: text
//!   with a visible area, loaded images, `url()` background images, SVG, drawn canvases, video
//!   posters; not hidden, not at opacity 0, not outside the viewport or an overflow clip), or
//!   a text control whose value is not only whitespace. A subframe's content is not the
//!   embedder's (each document has its own timeline and scan).
//! - `first-paint`: the first frame in which anything other than the default painted: all of
//!   the above, or a root background colour (`html`, else `body`, paints the whole canvas), or
//!   a visible box with a background colour or image (gradients included), a border or a
//!   box shadow that reaches the viewport.
//!
//! # Frames
//!
//! `Browser::note_first_paint` (after each task that changed something) asks for a frame while
//! the document has no `first-contentful-paint` ([`timeline::request_rendering`]: one plain
//! frame timer, never an eager frame). [`on_frame`] asks again only while the scan is waiting
//! for a CSS image or poster to finish loading (nothing in the document says when), then
//! stops: a blank page that sits still pays nothing.
//!
//! Not done: pseudo-element content (`::before` text and images, `::first-letter`),
//! `border-image`, `mask-image`, WebGL canvases and decoded video frames (there are none).

use std::rc::Rc;

use pyro_dom::layout::Rect;
use pyro_dom::style::CheckVisibilityOptions;
use pyro_dom::{Dom, NodeId, NodeType};

use crate::page::PageDom;

use super::timeline::{report_paint, request_rendering, PaintRecord};
use super::{scan, FrameInfo};

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["paint"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = true;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
/// `PerformancePaintTiming` is registered with the other performance entry classes.
pub fn register(_engine: &pyro_js::Engine) {}

/// Whether the document still needs frames for its paint entries
/// (`first-contentful-paint` not reported): the caller (`Browser::note_first_paint`) asks for one.
///
/// Only while a scan could find something the last one did not ([`scan::stale`]): a document
/// that never paints anything contentful (blank, style-only, hidden) is scanned once per change,
/// not once per frame, or each frame's task would ask for the next and the event loop would
/// never go idle (`run_until_idle`, `fetch --wait`, lifecycle events). A hidden document's
/// frames scan nothing ([`on_frame`]), so it must not ask either.
pub fn wants_frame(page: &PageDom) -> bool {
    !page.is_hidden()
        && !page.timeline().paint_records().iter().any(|r| r.name == "first-contentful-paint")
        && scan::stale(page)
}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    let records = page.timeline().paint_records();
    let has = |name: &str| records.iter().any(|r| r.name == name);
    if has("first-contentful-paint") || page.is_hidden() {
        return;
    }
    let scan = scan::painted_candidates(page, frame);
    let contentful = !scan.candidates.is_empty() || text_control_painted(page, scan.viewport);
    // Content implies a first paint; a document with none looks for boxes that paint.
    let painted = !has("first-paint") && (contentful || non_default_paint(page, scan.viewport));
    let report = |name: &'static str| {
        report_paint(
            page,
            PaintRecord { name, start: frame.presented, paint_time: frame.paint_time, presentation_time: Some(frame.presented) },
        )
    };
    if painted {
        report("first-paint");
    }
    if contentful {
        report("first-contentful-paint");
    } else if scan.waiting {
        // A CSS image or poster is on its way and nothing in the document will say so.
        request_rendering(page);
    }
}

/// A colour value (`getComputedStyle`) that paints nothing.
fn transparent(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("transparent") {
        return true;
    }
    let lower = v.to_ascii_lowercase();
    if lower.starts_with("rgba(") || lower.starts_with("rgb(") || lower.starts_with("hsla(") || lower.starts_with("hsl(") {
        let inner = lower.trim_end_matches(')');
        // `rgb(r g b / a)` or `rgba(r, g, b, a)`.
        let alpha = match inner.rsplit_once('/') {
            Some((_, a)) => Some(a),
            None if inner.matches(',').count() == 3 => inner.rsplit_once(',').map(|(_, a)| a),
            None => None,
        };
        if let Some(a) = alpha {
            let a = a.trim();
            return match a.strip_suffix('%') {
                Some(p) => p.trim().parse::<f64>().is_ok_and(|p| p <= 0.0),
                None => a.parse::<f64>().is_ok_and(|a| a <= 0.0),
            };
        }
    }
    false
}

fn on_screen(rect: Rect, view: (f64, f64)) -> bool {
    rect.width > 0.0 && rect.height > 0.0 && rect.right() > 0.0 && rect.bottom() > 0.0 && rect.left() < view.0 && rect.top() < view.1
}

/// Text controls (`input`, `textarea`) whose value has something to show.
fn text_control_painted(page: &PageDom, viewport: (f64, f64)) -> bool {
    let dom = page.dom.borrow();
    let Ok(nodes) = dom.descendants(page.document()) else { return false };
    let mut controls: Vec<NodeId> = Vec::new();
    for id in nodes {
        if !matches!(dom.node_type(id), Ok(NodeType::Element)) {
            continue;
        }
        let Ok(el) = dom.element(id) else { continue };
        let text_like = match &*el.name.local {
            "textarea" => true,
            "input" => el
                .attr("type")
                .is_none_or(|t| matches!(t.to_ascii_lowercase().as_str(), "" | "text" | "search" | "url" | "tel" | "email" | "password")),
            _ => false,
        };
        if text_like && &*el.name.ns == "http://www.w3.org/1999/xhtml" {
            controls.push(id);
        }
    }
    if controls.is_empty() {
        return false;
    }
    let layout = page.layout();
    let shown = CheckVisibilityOptions { check_visibility: true, check_opacity: true };
    controls.into_iter().any(|id| {
        dom.value(id).is_ok_and(|v| !v.trim().is_empty())
            && layout.check_visibility(&dom, id, shown)
            && on_screen(layout.laid_out_client_rect(&dom, id), viewport)
    })
}

/// Whether something other than the default paints: a root background colour, or a visible box
/// reaching the viewport with a background, a border or a shadow of its own.
fn non_default_paint(page: &PageDom, viewport: (f64, f64)) -> bool {
    let dom = page.dom.borrow();
    let layout = page.layout();
    let document = page.document();
    let shown = CheckVisibilityOptions { check_visibility: true, check_opacity: true };
    // The canvas background: the root element's, else the body's (CSS Backgrounds § 2.11.2).
    let html = dom.document_element(document).ok().flatten();
    let body = dom.body(document).ok().flatten();
    if [html, body].into_iter().flatten().any(|root| !transparent(&layout.computed_style_value(&dom, root, "background-color"))) {
        return true;
    }
    let Ok(nodes) = dom.descendants(document) else { return false };
    let view = Rect { x: 0.0, y: 0.0, width: viewport.0, height: viewport.1 };
    for id in nodes {
        if !matches!(dom.node_type(id), Ok(NodeType::Element)) {
            continue;
        }
        // Cheap first: only a box that shows and reaches the viewport is looked at.
        if !layout.check_visibility(&dom, id, shown) {
            continue;
        }
        let rect = layout.laid_out_client_rect(&dom, id);
        if !on_screen(rect, (view.width, view.height)) {
            continue;
        }
        if paints_own_box(&dom, &layout, id) {
            return true;
        }
    }
    false
}

/// An element whose own box paints: a background colour or image, a border, a box shadow.
fn paints_own_box(dom: &Dom, layout: &pyro_dom::layout::Layout, el: NodeId) -> bool {
    let style = |name: &str| layout.computed_style_value(dom, el, name);
    if !transparent(&style("background-color")) {
        return true;
    }
    let image = style("background-image");
    if !image.is_empty() && image != "none" {
        return true;
    }
    let shadow = style("box-shadow");
    if !shadow.is_empty() && shadow != "none" {
        return true;
    }
    ["top", "right", "bottom", "left"].iter().any(|side| {
        let border_style = style(&format!("border-{side}-style"));
        if border_style.is_empty() || border_style == "none" || border_style == "hidden" {
            return false;
        }
        let width = style(&format!("border-{side}-width"));
        let zero = width.trim().trim_end_matches("px").parse::<f64>().is_ok_and(|w| w <= 0.0);
        !zero && !transparent(&style(&format!("border-{side}-color")))
    })
}
