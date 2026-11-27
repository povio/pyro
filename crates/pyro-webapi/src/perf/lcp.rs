//! Largest Contentful Paint: a `largest-contentful-paint` entry each time a rendering update
//! first paints something larger than everything painted before, until the first user input.
//!
//! # Candidates and size
//!
//! The shared scan's ([`scan::painted_candidates`]) newly painted candidates of the frame:
//! images (`<img>`, SVG `<image>`, `<input type=image>`), video posters, `url()` background
//! images and text blocks. Not SVG shapes or canvases (FCP counts them, LCP does not), not
//! images that cover the whole viewport (Chrome ignores them), not transparent text (unless it
//! has a `text-shadow` or a `-webkit-text-stroke`, which paint). `size` is the visible area; an
//! image shown larger than its natural size counts `visible * natural / displayed` (upscaling
//! does not make an image larger), a CSS image only knows its natural size from a `data:` URL
//! (else the container's visible area counts). A candidate is not reconsidered when it is
//! resized or its text changes (the scan's identity), a new `src` or a new element is new.
//! At most one entry per frame: the largest new candidate, when larger than the current largest.
//!
//! # Times
//!
//! `renderTime` is the frame's presentation time, `paintTime` its paint time, `loadTime` the
//! image request's end (0 for text), `startTime` the render time. An image whose response
//! failed Timing-Allow-Origin gets the coarsened render time of `element::render_times` (as
//! Chrome coarsens it, never before `loadTime`), `presentationTime` null. `url` is the image's
//! resolved URL (a `data:` URL cut at 100 characters), "" for text. `element` is null once the
//! element is disconnected or its document is not fully active.
//!
//! # Stopping
//!
//! The first trusted `mousedown`/`pointerdown`/`pointerup`/`keydown`/touch or `wheel`
//! (`timeline().last_input_time()` / `last_wheel_time()`) ends the search for good (a script
//! dispatched `scroll` or any untrusted event does not). A document nobody observes keeps
//! producing entries (late `buffered` observers read them) for its first [`UNOBSERVED_WALKS`]
//! document scans, then stops asking for frames until an observer of the type appears: a guess
//! that bounds the cost of a page that never looks.
//!
//! Not done: the low-entropy (bits per pixel) image exclusion, the natural size of CSS images
//! fetched over the network, `object-fit`/`object-position` clipping of an image's painted
//! part, soft-navigation entries, animated-image first-frame time, video frames.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::element::{data_url_dimensions, render_times, NodeRef};
use super::scan::{self, Candidate, Kind};
use super::timeline::{queue_entry, request_rendering, TimelineEntry};
use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["largest-contentful-paint"];
/// Flip to true when the module really works: only then are its types in
/// `supportedEntryTypes`, its classes registered and its hooks called.
pub const ENABLED: bool = true;

/// How many scans an unobserved document gets before `lcp` stops asking for frames.
pub const UNOBSERVED_WALKS: u64 = 60;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<LargestContentfulPaint>();
}

struct State {
    page: Weak<PageDom>,
    /// Size of the largest candidate reported.
    largest: f64,
    /// A user input ended the search.
    stopped: bool,
}

thread_local! {
    /// Per document (`PageDom::id`); entries of dropped documents are pruned on access.
    static STATES: RefCell<FxHashMap<u64, State>> = RefCell::new(FxHashMap::default());
}

fn with_state<R>(page: &Rc<PageDom>, f: impl FnOnce(&mut State) -> R) -> R {
    STATES.with(|states| {
        let mut states = states.borrow_mut();
        states.retain(|_, s| s.page.upgrade().is_some_and(|p| !p.is_disposed()));
        let state = states.entry(page.id()).or_insert_with(|| State { page: Rc::downgrade(page), largest: 0.0, stopped: false });
        f(state)
    })
}

/// Whether a trusted input happened: the search for the largest paint is over.
fn input_seen(page: &PageDom) -> bool {
    let timeline = page.timeline();
    timeline.last_input_time().is_some() || timeline.last_wheel_time().is_some()
}

/// Whether the document should get a frame after the change a task made: not stopped, and
/// either observed or within its first scans.
pub fn wants_frame(page: &Rc<PageDom>) -> bool {
    // (A hidden document's frames scan nothing, so it must not ask: that would never settle.)
    if !ENABLED || input_seen(page) || page.is_hidden() {
        return false;
    }
    let watched = page.timeline().is_observed(ENTRY_TYPES[0]) || scan::walks(page) < UNOBSERVED_WALKS;
    watched && !with_state(page, |s| s.stopped) && scan::stale(page)
}

/// A text colour that paints nothing by itself (`transparent`, alpha 0).
fn transparent(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    if v.is_empty() || v == "transparent" {
        return true;
    }
    let Some(inner) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")).or_else(|| v.strip_prefix("hsla(")).or_else(|| v.strip_prefix("hsl(")) else {
        return false;
    };
    let inner = inner.trim_end_matches(')');
    let alpha = match inner.rsplit_once('/') {
        Some((_, a)) => Some(a),
        None if inner.matches(',').count() == 3 => inner.rsplit_once(',').map(|(_, a)| a),
        None => None,
    };
    alpha.is_some_and(|a| {
        let a = a.trim();
        match a.strip_suffix('%') {
            Some(p) => p.trim().parse::<f64>().is_ok_and(|p| p <= 0.0),
            None => a.parse::<f64>().is_ok_and(|a| a <= 0.0),
        }
    })
}

/// The size LCP gives `c`, or `None` when it is not eligible. See the module docs.
fn size_of(page: &PageDom, c: &Candidate, viewport: (f64, f64)) -> Option<f64> {
    let covers_viewport = c.area >= viewport.0 * viewport.1 && viewport.0 * viewport.1 > 0.0;
    let size = match c.kind {
        Kind::Svg | Kind::Canvas => return None,
        Kind::Text => {
            let dom = page.dom.borrow();
            let layout = page.layout();
            let style = |name: &str| layout.computed_style_value(&dom, c.node, name);
            let paints_without_color = {
                let shadow = style("text-shadow");
                let stroke = style("-webkit-text-stroke-width");
                (!shadow.is_empty() && shadow != "none") || stroke.trim().trim_end_matches("px").parse::<f64>().is_ok_and(|w| w > 0.0)
            };
            if transparent(&style("color")) && !paints_without_color {
                return None;
            }
            c.area
        }
        Kind::Image | Kind::VideoPoster | Kind::BackgroundImage => {
            if covers_viewport {
                return None;
            }
            let natural = c.natural.or_else(|| match c.kind {
                Kind::BackgroundImage => c.url.as_deref().and_then(data_url_dimensions),
                _ => None,
            });
            let displayed = c.border_box.width * c.border_box.height;
            match natural {
                Some((w, h)) if displayed > w * h && displayed > 0.0 => c.area * (w * h) / displayed,
                _ => c.area,
            }
        }
    };
    (size >= 1.0).then_some(size.floor())
}

/// What an entry holds, shared by the wrappers of every realm.
pub struct LcpEntry {
    node: NodeRef,
    id: String,
    url: String,
    size: f64,
    render_time: f64,
    load_time: f64,
    paint_time: f64,
    presentation_time: Option<f64>,
}

impl LcpEntry {
    fn start(&self) -> f64 {
        if self.render_time > 0.0 {
            self.render_time
        } else {
            self.load_time
        }
    }
}

pub struct LcpData {
    entry: Rc<LcpEntry>,
    base: PerformanceEntryData,
}

pub struct LargestContentfulPaint;

impl JsClass for LargestContentfulPaint {
    const NAME: &'static str = "LargestContentfulPaint";
    type Data = LcpData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &LcpData| &d.base);
        def.getter("loadTime", |s, d| Ok(s.number(d.entry.load_time)));
        def.getter("renderTime", |s, d| Ok(s.number(d.entry.render_time)));
        def.getter("size", |s, d| Ok(s.number(d.entry.size)));
        def.getter("id", |s, d| Ok(s.string(&d.entry.id)));
        def.getter("url", |s, d| Ok(s.string(&d.entry.url)));
        def.getter("element", |s, d| d.entry.node.element(s));
        // PaintTimingMixin.
        def.getter("paintTime", |s, d| Ok(s.number(d.entry.paint_time)));
        def.getter("presentationTime", |s, d| Ok(d.entry.presentation_time.map_or(s.null(), |t| s.number(t))));
        def.method("toJSON", 0, |s, d, _a| {
            let e = &d.entry;
            let o = s.object();
            o.set("name", s.string(""))?;
            o.set("entryType", s.string("largest-contentful-paint"))?;
            o.set("startTime", s.number(e.start()))?;
            o.set("duration", s.number(0.0))?;
            o.set("renderTime", s.number(e.render_time))?;
            o.set("loadTime", s.number(e.load_time))?;
            o.set("size", s.number(e.size))?;
            o.set("id", s.string(&e.id))?;
            o.set("url", s.string(&e.url))?;
            o.set("paintTime", s.number(e.paint_time))?;
            o.set("presentationTime", e.presentation_time.map_or(s.null(), |t| s.number(t)))?;
            Ok(o.value())
        });
    }
}

struct LcpTimingEntry(Rc<LcpEntry>);

impl TimelineEntry for LcpTimingEntry {
    fn entry_type(&self) -> &'static str {
        "largest-contentful-paint"
    }
    fn start_time(&self) -> f64 {
        self.0.start()
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let data = LcpData { entry: self.0.clone(), base: PerformanceEntryData::new("", "largest-contentful-paint", self.0.start(), 0.0) };
        Ok(s.wrap::<LargestContentfulPaint>(data, Some(key))?.value())
    }
}

/// The end of a rendering update of `page`'s document; called when ENABLED.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    if page.is_hidden() || with_state(page, |s| s.stopped) {
        return;
    }
    if input_seen(page) {
        with_state(page, |s| s.stopped = true);
        return;
    }
    if !page.timeline().is_observed(ENTRY_TYPES[0]) && scan::walks(page) >= UNOBSERVED_WALKS {
        return;
    }
    let scan = scan::painted_candidates(page, frame);
    let largest = with_state(page, |s| s.largest);
    // The largest new candidate of this frame (the first of equals, document order).
    let mut best: Option<(f64, &Candidate)> = None;
    for c in scan.newly_painted(frame) {
        let Some(size) = size_of(page, c, scan.viewport) else { continue };
        if size > largest && best.is_none_or(|(b, _)| size > b) {
            best = Some((size, c));
        }
    }
    if let Some((size, c)) = best {
        let image = c.kind != Kind::Text;
        let source = if image { c.url.as_deref() } else { None };
        let url = source.map(|u| if u.starts_with("data:") { u.chars().take(100).collect::<String>() } else { u.to_owned() });
        let (render_time, paint_time, presentation_time) = render_times(page, frame, source, c.load_time);
        let id = page.dom.borrow().get_attribute(c.node, "id").ok().flatten().unwrap_or("").to_owned();
        if let Some(node) = NodeRef::new(page, c.node) {
            with_state(page, |s| s.largest = size);
            let entry = LcpEntry {
                node,
                id,
                url: url.unwrap_or_default(),
                size,
                render_time,
                load_time: if image { c.load_time.unwrap_or(0.0) } else { 0.0 },
                paint_time,
                presentation_time,
            };
            queue_entry(page, Rc::new(LcpTimingEntry(Rc::new(entry))));
        }
    }
    // A CSS image or poster is on its way and nothing in the document will say so.
    if scan.waiting && scan::stale(page) {
        request_rendering(page);
    }
}
