//! Element Timing: `PerformanceElementTiming` entries for the elements that carry an
//! `elementtiming` attribute, made when the shared scan ([`scan::painted_candidates`]) first
//! finds them painted.
//!
//! # What is reported
//!
//! In every rendering update, the candidates newly painted in that frame whose element has the
//! attribute (an empty value counts: the identifier is then the empty string):
//! - images (`<img>`, SVG `<image>`, `<input type=image>`) once loaded: `name` "image-paint",
//!   `url` the resolved source, `loadTime` the time the request finished;
//! - `<video poster>` and elements with a `url()` background image, likewise;
//! - the text of a block container with the attribute: "text-paint", `url` "", `loadTime` 0.
//!
//! An element is reported once per painted identity (node, kind, url): changing the text of an
//! already painted block adds no entry, changing an image's `src` adds one. `intersectionRect`
//! is the visible part of the box in the viewport (transforms and overflow clips applied), its
//! `naturalWidth`/`naturalHeight` the image's natural size (a CSS background image's only when
//! it is a `data:` URL: Pyro does not keep the decoded size of CSS images). Elements in shadow
//! trees, hidden ones (`display`, `visibility`, `opacity`), images that failed, and elements of
//! a child frame (it has its own timeline) are not reported: the scan does not make them
//! candidates. `element` reads null once the element is disconnected, in a shadow tree, or its
//! document is not fully active. Not done: pseudo-element content and `list-style-image`,
//! several background images (the first `url()` is the candidate), `<video>` poster natural
//! sizes (no decoder for SVG), `<object>`.
//!
//! # Times
//!
//! `renderTime` is the frame's presentation time and `presentationTime` the same, `paintTime`
//! the frame's paint time (the same reading as `paint` and `lcp`). An image whose response did
//! not pass the Timing-Allow-Origin check (Resource Timing hides its phases: the same decision,
//! read from the entry) gets a coarsened `renderTime`: the presentation time rounded down to
//! 4 ms and never before the image's `loadTime`; `presentationTime` is then null and
//! `paintTime` equals `renderTime`.
//!
//! # Frames
//!
//! A document that changed since the last scan is given a frame by `Browser::note_first_paint`
//! ([`wants_frame`]); [`on_frame`] asks for another only while the scan awaits a CSS image or
//! poster, then stops.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyro_dom::layout::Rect;
use pyro_dom::NodeId;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, Persistent, Scope, Value};

use crate::node::NodeData;
use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};
use crate::util::*;

use super::scan::{self, Candidate, Kind};
use super::timeline::{queue_entry, request_rendering, TimelineEntry};
use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["element"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = true;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<PerformanceElementTiming>();
}

/// `Element.elementTiming` (`[Reflect] attribute DOMString`, the `elementtiming` attribute);
/// called by `Element`'s definition while ENABLED.
pub(crate) fn define_attribute<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.accessor(
        "elementTiming",
        |s, n| {
            let dom = n.page().dom.borrow();
            let value = dom.get_attribute(n.id, "elementtiming").js()?.unwrap_or("");
            Ok(s.string(value))
        },
        |_s, n, v| {
            let value = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute_ns(n.id, None, "elementtiming", &value).js()
        },
    );
}

/// Whether the document has an element with an `elementtiming` attribute, looked up once per
/// DOM version (a page nobody marks pays one walk per change, no layout).
fn has_marked_element(page: &PageDom) -> bool {
    thread_local! {
        static LAST: std::cell::Cell<Option<(u64, u64, bool)>> = const { std::cell::Cell::new(None) };
    }
    let Ok(dom) = page.dom.try_borrow() else { return true };
    let version = dom.version();
    if let Some((id, v, marked)) = LAST.get() {
        if id == page.id() && v == version {
            return marked;
        }
    }
    let marked = dom
        .descendants(page.document())
        .is_ok_and(|mut nodes| nodes.any(|n| dom.element(n).is_ok() && dom.has_attribute(n, "elementtiming").unwrap_or(false)));
    LAST.set(Some((page.id(), version, marked)));
    marked
}

/// Whether the module has anything to watch: an observer of `element`, or an element that
/// carries the attribute (entries made before an observer registers are `buffered`).
fn watching(page: &PageDom) -> bool {
    page.timeline().is_observed("element") || has_marked_element(page)
}

/// Whether a document change since the last scan may have painted something this module reports
/// (`Browser::note_first_paint` asks, and requests the frame).
pub fn wants_frame(page: &PageDom) -> bool {
    ENABLED && !page.is_hidden() && watching(page) && scan::stale(page)
}

/// A node an entry names, held without keeping its page alive: the entry's `element` is that
/// node while it is connected in a fully active document, else null. Shared by `lcp` and `shift`.
pub(super) struct NodeRef {
    page: Weak<PageDom>,
    id: NodeId,
}

impl NodeRef {
    /// Holds `id` (a reference in its DOM, so the slot is not reused); `None` when it is gone.
    pub(super) fn new(page: &Rc<PageDom>, id: NodeId) -> Option<NodeRef> {
        page.dom.try_borrow_mut().ok()?.add_ref(id).ok()?;
        Some(NodeRef { page: Rc::downgrade(page), id })
    }

    pub(super) fn page(&self) -> Option<Rc<PageDom>> {
        self.page.upgrade()
    }

    /// The element's wrapper in `s`'s realm, or null.
    pub(super) fn element<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let Some(page) = self.page.upgrade() else { return Ok(s.null()) };
        if !page.is_fully_active() {
            return Ok(s.null());
        }
        let shown = {
            let dom = page.dom.borrow();
            dom.is_in_active_document(self.id) && dom.in_document_tree(self.id)
        };
        if !shown {
            return Ok(s.null());
        }
        Ok(crate::node::wrap_node(s, &page, self.id)?.value())
    }
}

impl Drop for NodeRef {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            page.release_node(self.id);
        }
    }
}

/// Whether Resource Timing exposes the fetch phases of `url`'s response (`Timing-Allow-Origin`
/// passed, or same-origin all the way): the entry a failed check leaves has no request start,
/// no response start and no body sizes (`resource_timing::entry_with_clock`). A URL with no
/// entry (`data:`, `blob:`, a cleared buffer) is exposed.
pub(super) fn timing_exposed(page: &PageDom, url: &str) -> bool {
    if !(url.starts_with("http:") || url.starts_with("https:")) {
        return true;
    }
    let entries = page.resource_timings().entries();
    entries.iter().rev().find(|e| e.name == url).is_none_or(|e| {
        e.request_start > 0.0 || e.response_start > 0.0 || e.encoded_body_size > 0 || e.decoded_body_size > 0
    })
}

/// `(renderTime, paintTime, presentationTime)` of something painted in `frame`, whose image (if
/// any) was fetched from `url` and finished loading at `load_time`. See the module docs.
pub(super) fn render_times(page: &PageDom, frame: &FrameInfo, url: Option<&str>, load_time: Option<f64>) -> (f64, f64, Option<f64>) {
    if url.is_none_or(|u| timing_exposed(page, u)) {
        (frame.presented, frame.paint_time, Some(frame.presented))
    } else {
        let coarse = ((frame.presented / 4.0).floor() * 4.0).max(load_time.unwrap_or(0.0));
        (coarse, coarse, None)
    }
}

/// What an entry holds, shared by the wrappers of every realm.
pub struct ElementEntry {
    node: NodeRef,
    name: &'static str,
    identifier: String,
    id: String,
    url: String,
    natural: (f64, f64),
    render_time: f64,
    load_time: f64,
    paint_time: f64,
    presentation_time: Option<f64>,
    rect: Rect,
}

impl ElementEntry {
    /// `startTime`: the render time, the load time when that is 0.
    fn start(&self) -> f64 {
        if self.render_time > 0.0 {
            self.render_time
        } else {
            self.load_time
        }
    }
}

/// The data of one wrapper: `base` is what `PerformanceEntry`'s getters read, `rect` the
/// `intersectionRect` object (one per entry object, so `toJSON()` holds the same one).
pub struct ElementData {
    entry: Rc<ElementEntry>,
    base: PerformanceEntryData,
    rect: RefCell<Option<Persistent>>,
}

pub struct PerformanceElementTiming;

impl JsClass for PerformanceElementTiming {
    const NAME: &'static str = "PerformanceElementTiming";
    type Data = ElementData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ElementData| &d.base);
        def.getter("renderTime", |s, d| Ok(s.number(d.entry.render_time)));
        def.getter("loadTime", |s, d| Ok(s.number(d.entry.load_time)));
        def.getter("intersectionRect", intersection_rect);
        def.getter("identifier", |s, d| Ok(s.string(&d.entry.identifier)));
        def.getter("naturalWidth", |s, d| Ok(s.number(d.entry.natural.0)));
        def.getter("naturalHeight", |s, d| Ok(s.number(d.entry.natural.1)));
        def.getter("id", |s, d| Ok(s.string(&d.entry.id)));
        def.getter("element", |s, d| d.entry.node.element(s));
        def.getter("url", |s, d| Ok(s.string(&d.entry.url)));
        // PaintTimingMixin.
        def.getter("paintTime", |s, d| Ok(s.number(d.entry.paint_time)));
        def.getter("presentationTime", |s, d| Ok(d.entry.presentation_time.map_or(s.null(), |t| s.number(t))));
        def.method("toJSON", 0, |s, d, _a| to_json(s, d));
    }
}

fn intersection_rect<'s>(s: &Scope<'s>, d: &ElementData) -> JsResult<Value<'s>> {
    if let Some(cached) = &*d.rect.borrow() {
        return Ok(cached.get(s));
    }
    let Some(page) = d.entry.node.page() else { return Ok(s.null()) };
    let rect = crate::canvas::rect_value(s, &page, true, d.entry.rect)?;
    *d.rect.borrow_mut() = Some(Persistent::new(s, rect));
    Ok(rect)
}

/// The default `toJSON()`: every attribute of the interface and its parent but `element`.
fn to_json<'s>(s: &Scope<'s>, d: &ElementData) -> JsResult<Value<'s>> {
    let e = &d.entry;
    let o = s.object();
    o.set("name", s.string(e.name))?;
    o.set("entryType", s.string("element"))?;
    o.set("startTime", s.number(e.start()))?;
    o.set("duration", s.number(0.0))?;
    o.set("renderTime", s.number(e.render_time))?;
    o.set("loadTime", s.number(e.load_time))?;
    o.set("intersectionRect", intersection_rect(s, d)?)?;
    o.set("identifier", s.string(&e.identifier))?;
    o.set("naturalWidth", s.number(e.natural.0))?;
    o.set("naturalHeight", s.number(e.natural.1))?;
    o.set("id", s.string(&e.id))?;
    o.set("url", s.string(&e.url))?;
    o.set("paintTime", s.number(e.paint_time))?;
    o.set("presentationTime", e.presentation_time.map_or(s.null(), |t| s.number(t)))?;
    Ok(o.value())
}

/// The entry as the timeline queues it.
struct ElementTimingEntry(Rc<ElementEntry>);

impl TimelineEntry for ElementTimingEntry {
    fn entry_type(&self) -> &'static str {
        "element"
    }
    fn start_time(&self) -> f64 {
        self.0.start()
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let data = ElementData {
            entry: self.0.clone(),
            base: PerformanceEntryData::new(self.0.name, "element", self.0.start(), 0.0),
            rect: RefCell::new(None),
        };
        Ok(s.wrap::<PerformanceElementTiming>(data, Some(key))?.value())
    }
}

/// Natural size of a `data:` image URL, from its header.
pub(super) fn data_url_dimensions(url: &str) -> Option<(f64, f64)> {
    if !url.starts_with("data:") {
        return None;
    }
    let (_, body) = pyro_net::data_url::parse_str(url)?;
    let (w, h) = pyro_dom::image::dimensions(&body)?;
    Some((f64::from(w), f64::from(h)))
}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    if page.is_hidden() || !watching(page) {
        return;
    }
    let scan = scan::painted_candidates(page, frame);
    // The newly painted candidates of elements that carry the attribute (no layout here).
    let marked: Vec<(Candidate, String, String)> = {
        let dom = page.dom.borrow();
        scan.newly_painted(frame)
            .filter(|c| matches!(c.kind, Kind::Text | Kind::Image | Kind::VideoPoster | Kind::BackgroundImage))
            .filter_map(|c| {
                let identifier = dom.get_attribute(c.node, "elementtiming").ok().flatten()?.to_owned();
                let id = dom.get_attribute(c.node, "id").ok().flatten().unwrap_or("").to_owned();
                Some((c.clone(), identifier, id))
            })
            .collect()
    };
    for (c, identifier, id) in marked {
        let image = c.kind != Kind::Text;
        let source = if image { c.url.as_deref() } else { None };
        let (render_time, paint_time, presentation_time) = render_times(page, frame, source, c.load_time);
        let Some(node) = NodeRef::new(page, c.node) else { continue };
        let entry = ElementEntry {
            node,
            name: if image { "image-paint" } else { "text-paint" },
            identifier,
            id,
            url: source.unwrap_or("").to_owned(),
            natural: c.natural.or_else(|| source.and_then(data_url_dimensions)).unwrap_or((0.0, 0.0)),
            render_time,
            load_time: if image { c.load_time.unwrap_or(0.0) } else { 0.0 },
            paint_time,
            presentation_time,
            rect: c.visible,
        };
        queue_entry(page, Rc::new(ElementTimingEntry(Rc::new(entry))));
    }
    // A CSS image or poster is on its way and nothing in the document will say so.
    if scan.waiting && scan::stale(page) {
        request_rendering(page);
    }
}
