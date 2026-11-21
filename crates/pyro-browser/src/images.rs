//! `<img>` resources: fetched through the page's resource loader (Image
//! destination, so CDP sees an Image request) to learn the natural size
//! from the file header. Pixels are not decoded here: when the document may
//! draw images on a canvas, the bytes are kept for it to decode on use
//! (`PageDom::set_image_bytes`).
//!
//! - An `<img src>` starts a load when its resolved URL (against the
//!   document base URL, so `<base href>` counts) changes, connected or not:
//!   `new Image()` and an `<img>` built before insertion load as in Chromium,
//!   when their document has a browsing context. The element is pinned while
//!   its request is in flight and released when it completes. An unchanged
//!   `src` does not re-request.
//! - `decode()` (`pyro_webapi::image_decode`) fulfills once the current
//!   request completed with a recognized image, and rejects with
//!   `EncodingError` for a broken one, a source changed before completion,
//!   no source, or an inactive document.
//! - `blob:` URLs read the page's blob store; `data:` URLs go through the
//!   network client.
//! - While loading, `complete` is false. Images that start before the
//!   window `load` event delay it, as in Chromium.
//! - Completion records the natural size (`pyro_dom::image`) and fires
//!   `load`. Network failure, a non-2xx status or a body that is neither a
//!   recognized image nor `image/*` fires `error`. An `image/*` body whose
//!   format has no header parser (for example AVIF) loads with an unknown
//!   size. SVG takes its size from numeric `width`/`height` on the root.
//! - An empty `src` fires `error`; removing `src` clears the image. The
//!   `<img>`'s own `srcset` (with `sizes`) selects the candidate for the
//!   device pixel ratio (`pyro_dom::picture`). Not modelled:
//!   `loading="lazy"` deferral (lazy images load eagerly), re-resolving a
//!   loaded `src` when a `<base>` is inserted later.
//! - An `<img>` child of a `<picture>` loads the first preceding `<source>`
//!   whose `media` matches and whose `type` is an image type
//!   (`pyro_dom::picture`). Moving, inserting or removing a `<source>` or the
//!   `<img>` selects again and reloads when the URL changed.
use crate::frames::FrameKey;
use crate::loader::ResourceNode;
use crate::{Browser, FetchRequest, LoaderId, Resource, ResourceCancellation, ResourceKind};
use pyro_dom::NodeId;
use pyro_runtime::TaskSource;
use pyro_webapi::PageDom;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use crate::speculate::Shape;
use crate::{FetchDone, ResourceLoader};
use std::rc::Rc;

/// Bytes kept from an image response for header parsing.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

struct Image {
    /// Pins the element while its request is in flight (a `new Image()`
    /// nobody references still fires `load`); released when it completes.
    _node: Option<Rc<ResourceNode>>,
    url: String,
    /// The `src` attribute value this load is for.
    src: String,
    /// The pixel density of the selected `srcset` candidate (1 without one).
    density: f64,
    generation: u64,
    pending: bool,
    blocks_load: bool,
    cancel: Option<ResourceCancellation>,
    /// The request's CORS settings and referrer policy states: a change restarts it.
    request_state: (u8, Option<pyro_net::ReferrerPolicy>, Option<url::Url>),
    /// Whether the finished request gave an image (`load`) or not (`error`): what a restart of
    /// the same request fires again without fetching (HTML's list of available images).
    outcome: Option<bool>,
}

impl Images {
    /// pyro: the URLs the document's elements requested as images themselves (`<img>`, SVG
    /// `<image>`, `<object>`), loaded or failed or in flight, for the document's life.
    pub(crate) fn urls(&self) -> Vec<String> {
        self.requested.iter().cloned().collect()
    }

    /// Tracks `node`'s request (`image.url`), replacing its previous one.
    fn track(&mut self, node: NodeId, image: Image) {
        self.offers.retain(|(_, owner, ..)| *owner != node);
        if self.requested.len() < 4096 {
            self.requested.insert(image.url.clone());
        }
        self.entries.insert(node, image);
    }
}

/// An `<img>`'s CORS settings attribute state (0 no CORS, 1 anonymous, 2 use-credentials).
fn image_cors(dom: &pyro_dom::Dom, node: NodeId) -> u8 {
    match dom.get_attribute(node, "crossorigin").ok().flatten() {
        None => 0,
        Some(v) if v.eq_ignore_ascii_case("use-credentials") => 2,
        Some(_) => 1,
    }
}

#[derive(Default)]
pub(crate) struct Images {
    flights: Rc<RefCell<HashMap<Shape, Rc<RefCell<ImageFlight>>>>>,
    offers: VecDeque<(Shape, NodeId, Resource, std::sync::Arc<pyro_raster::SourceLease>)>,
    offers_tree_version: Option<u64>,
    entries: HashMap<NodeId, Image>,
    /// pyro: every URL the document's elements requested as images (`<img>`, SVG `<image>`,
    /// `<object>`), kept after the request ends (a loaded `<img>`'s entry goes): a capture
    /// waits for those, the document fetched them (`Images::urls`).
    requested: HashSet<String>,
    /// Lazy images whose source was selected while they were not being rendered (detached, or
    /// `display: none`): their fetch waits (HTML: they never intersect the viewport) until they
    /// are rendered, or until `loading` stops being lazy, which fetches the source selected then.
    deferred: HashMap<NodeId, Option<pyro_dom::picture::Candidate>>,
    /// Each lazy image's last `sizes="auto"` width while it was rendered: what `auto` stays when
    /// it stops being rendered.
    auto_widths: HashMap<NodeId, f64>,
    /// The layout engine's sync stamp when the deferred images were last checked.
    checked_at: Option<(u64, u64, u32, u32)>,
    generation: u64,
    /// Entry count at which those of collected elements are dropped (a
    /// finished request no longer pins its element, and `new Image()`
    /// beacons would otherwise accumulate): amortized O(1) per load.
    sweep_at: usize,
}

impl Images {
    /// Pending images that started before the window `load` event.
    pub(crate) fn blocking_load(&self) -> usize {
        self.entries.values().filter(|i| i.pending && i.blocks_load).count()
    }
}

#[derive(Default)]
struct ImageFlight {
    waiters: BTreeMap<u64, (bool, FetchDone)>,
    next: u64,
    cancel: Option<ResourceCancellation>,
}

impl Images {
    fn offer(&mut self, shape: Shape, node: NodeId, resource: &Resource, bindings: &pyro_webapi::PageDom) {
        self.offers.retain(|(old, owner, ..)| old != &shape && *owner != node);
        const BUDGET: usize = 16 * 1024 * 1024;
        if !resource.immutable_local || resource.body.capacity() > BUDGET { return; }
        let mut bytes = resource.body.capacity() + self.offers.iter().map(|(_, _, entry, _)| entry.body.capacity()).sum::<usize>();
        while bytes > BUDGET || self.offers.len() >= 64 {
            bytes -= self.offers.pop_front().unwrap().2.body.capacity();
        }
        if let Some(lease) = bindings.retain_image_source(resource.body.clone()) { self.offers.push_back((shape, node, resource.clone(), lease)); }
    }
}

impl Browser {
    pub(crate) fn dispatch_image(&mut self, frame: FrameKey, loader: &Rc<dyn ResourceLoader>, request: FetchRequest, done: FetchDone, layout: bool) -> Option<ResourceCancellation> {
        let shape = Shape::of(self, &request).expect("whole image");
        let flights = {
            let mut doc = self.doc_mut(frame)?;
            if layout && !doc.images.flights.borrow().contains_key(&shape) {
                if let Some(index) = doc.images.offers.iter().position(|(key, ..)| key == &shape) {
                    let (_, owner, resource, lease) = doc.images.offers.remove(index).unwrap();
                    if doc.main_frame.page_dom.as_ref().is_some_and(|page| page.dom.borrow().is_connected(owner) == Ok(true)) {
                        drop(doc);
                        self.scheduler().post(TaskSource::Networking, move |browser| { let _lease = lease; done(browser, Ok(resource)); });
                        return None;
                    }
                }
            }
            doc.images.flights.clone()
        };
        let flight = flights.borrow_mut().entry(shape.clone()).or_default().clone();
        let id = {
            let mut flight = flight.borrow_mut();
            let id = flight.next;
            flight.next += 1;
            flight.waiters.insert(id, (layout, done));
            id
        };
        let weak = Rc::downgrade(&flights);
        let pending = Rc::downgrade(&flight);
        if id == 0 {
            let (registry, current, key) = (weak.clone(), pending.clone(), shape.clone());
            let cancel = self.fetch_document_resource(frame, loader, request, Box::new(move |browser, result| {
                let (Some(registry), Some(current)) = (registry.upgrade(), current.upgrade()) else { return };
                registry.borrow_mut().remove(&key);
                let waiters = std::mem::take(&mut current.borrow_mut().waiters);
                if let Some(cancel) = current.borrow_mut().cancel.take() { cancel.disarm(); }
                let (layout, elements): (Vec<_>, Vec<_>) = waiters.into_values().partition(|(layout, _)| *layout);
                for (_, waiter) in elements { waiter(browser, result.clone()); }
                if !layout.is_empty() {
                    if let Some(mut state) = browser.doc_mut(frame) {
                        state.images.offers.retain(|(shape, ..)| shape != &key);
                    }
                }
                for (_, waiter) in layout { waiter(browser, result.clone()); }
            }));
            flight.borrow_mut().cancel = cancel;
        }
        Some(ResourceCancellation::new(move || {
            let (Some(registry), Some(current)) = (weak.upgrade(), pending.upgrade()) else { return };
            let mut flight = current.borrow_mut();
            if flight.waiters.remove(&id).is_some() && flight.waiters.is_empty() {
                registry.borrow_mut().remove(&shape);
                drop(flight.cancel.take());
            }
        }))
    }
}

enum Source {
    None,
    Invalid,
    Url(String),
}

/// SVG natural size from numeric (px or unitless) root width/height.
/// An SVG image's natural dimensions as `naturalWidth`/`naturalHeight` report them (CSS Images 3
/// "natural dimensions", with HTML's density correction of the image's own dimensions): a missing
/// one comes from the other through the `viewBox` ratio, or is the default object size's (300 x
/// 150); without either, the ratio fitted into the default object size, or that size.
fn svg_reported_size(body: &[u8], density: f64) -> Option<((f64, f64), bool)> {
    let text = std::str::from_utf8(body.get(..body.len().min(4096))?).ok()?;
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let raw = |name: &str| {
        let i = tag.find(&format!(" {name}="))? + name.len() + 2;
        let quote = tag[i..].chars().next()?;
        let rest = &tag[i + 1..];
        Some(rest[..rest.find(quote)?].trim().to_owned())
    };
    // An absolute length (px or unitless); percentages and negative values are none.
    let length = |name: &str| raw(name)?.trim_end_matches("px").parse::<f64>().ok().filter(|v| *v >= 0.0);
    let ratio = raw("viewBox").and_then(|v| {
        let n: Vec<f64> = v.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| n[2] / n[3])
    });
    let d = if density > 0.0 { density } else { 1.0 };
    let (width, height) = (length("width"), length("height"));
    let intrinsic = width.is_some() || height.is_some();
    Some((match (width, height, ratio) {
        (Some(w), Some(h), _) => (w / d, h / d),
        (Some(w), None, Some(r)) => (w / d, w / r / d),
        (Some(w), None, None) => (w / d, 150.0),
        (None, Some(h), Some(r)) => (h * r / d, h / d),
        (None, Some(h), None) => (300.0, h / d),
        (None, None, Some(r)) if r > 2.0 => (300.0, 300.0 / r),
        (None, None, Some(r)) => (150.0 * r, 150.0),
        (None, None, None) => (300.0, 150.0),
    }, intrinsic))
}

fn svg_size(body: &[u8]) -> Option<(f64, f64)> {
    let text = std::str::from_utf8(body.get(..body.len().min(4096))?).ok()?;
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attr = |name: &str| {
        let i = tag.find(&format!(" {name}="))? + name.len() + 2;
        let quote = tag[i..].chars().next()?;
        let rest = &tag[i + 1..];
        let value = &rest[..rest.find(quote)?];
        value.trim().trim_end_matches("px").parse::<f64>().ok().filter(|v| *v > 0.0)
    };
    Some((attr("width")?, attr("height")?))
}

/// Whether `node` is being rendered as of the last completed layout, never running layout: the
/// full answer when that layout is current (a query then syncs nothing), else the engine's last
/// answer; `None` when that layout has not seen `node` (inserted since) or cannot tell.
fn rendered_as_laid_out(layout: &pyro_dom::layout::Layout, dom: &pyro_dom::Dom, node: NodeId) -> Option<bool> {
    if layout.style().engine_current(dom) {
        return Some(layout.check_visibility(dom, node, Default::default()));
    }
    layout.style().displayed_as_synced(dom, node)
}

impl Browser {
    pub(crate) fn image_event(&mut self, page: FrameKey, node: NodeId) {
        self.image_event_with(page, node, false);
    }

    /// [`Browser::image_event`]; `restart`: a relevant mutation, which starts a new request (and
    /// fires `load` again) even when the URL is the one already loaded.
    pub(crate) fn image_event_with(&mut self, page: FrameKey, node: NodeId, restart: bool) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let base = state.main_frame.base_url.clone();
        let loaded = state.main_frame.loaded;
        // Inspect the final state: several src writes in one task start one load.
        let raw_src = bindings.dom.borrow().get_attribute(node, "src").ok().flatten().map(str::to_owned);
        // A `<picture>` parent's matching `<source>` provides the image
        // instead of `src` (a `<source>` or the `<img>` moving in or out of
        // the picture is a relevant mutation: the DOM queues this event).
        // A lazy image's concrete width, for `sizes="auto"` (laid out, else its `width`
        // attribute, else the default object size's 300 px).
        drop(state);
        let (lazy, hidden, rendered_width, attr_width, auto_css_width) = {
            let dom = bindings.dom.borrow();
            let lazy = dom.get_attribute(node, "loading").ok().flatten().is_some_and(|l| l.eq_ignore_ascii_case("lazy"));
            let connected = dom.is_connected(node).unwrap_or(false);
            // "Being rendered" from the last completed layout, never running layout here (this
            // runs per inserted image); its size only when that layout is current (free then).
            let laid_out = if connected && lazy { rendered_as_laid_out(&bindings.layout(), &dom, node) } else { None };
            let hidden = lazy && (!connected || laid_out == Some(false));
            let rendered = if laid_out == Some(true) && bindings.layout().style().engine_current(&dom) {
                bindings.layout().rendered_content_size(&dom, node)
            } else {
                None
            };
            // `object-fit: cover` (sizes-auto 2d, tentative): the width the image is drawn at,
            // max(width, height x ratio), the ratio from `h` descriptors, else the dimension
            // attributes.
            let rendered = rendered.map(|(w, h)| {
                let cover = bindings.layout().style().computed_value(&dom, node, "object-fit").is_some_and(|v| v == "cover");
                let ratio = || {
                    let from_h = dom.get_attribute(node, "srcset").ok().flatten().and_then(|srcset| {
                        srcset.split(',').find_map(|c| {
                            let mut parts = c.split_ascii_whitespace().skip(1);
                            let (mut w, mut h) = (None, None);
                            for d in parts.by_ref() {
                                if let Some(v) = d.strip_suffix('w') { w = v.parse::<f64>().ok(); }
                                if let Some(v) = d.strip_suffix('h') { h = v.parse::<f64>().ok(); }
                            }
                            w.zip(h).filter(|(_, h)| *h > 0.0).map(|(w, h)| w / h)
                        })
                    });
                    let attr = |n: &str| dom.get_attribute(node, n).ok().flatten().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 0.0);
                    from_h.or_else(|| attr("width").zip(attr("height")).map(|(w, h)| w / h))
                };
                if cover { ratio().map_or(w, |r| w.max(h * r)) } else { w }
            });
            let attr = dom.get_attribute(node, "width").ok().flatten().and_then(|w| w.trim().parse::<f64>().ok());
            // Lazy images only: the style read flushes style, which an eager image's insertion
            // must not do (it started the layout engine's fetches during parsing).
            let auto_css = lazy
                && rendered.is_some()
                && attr.is_none()
                && bindings.layout().style().computed_value(&dom, node, "width").is_none_or(|w| w == "auto")
                && dom.natural_size(node).is_none();
            (lazy, hidden, rendered, attr, auto_css)
        };
        // A lazy image's concrete width, for `sizes="auto"`: laid out (the default object size's
        // 300 px while its width is `auto` and no image gave a natural width). Not rendered: its
        // last width while rendered when still connected, else none (the `auto` entry is skipped).
        let _ = attr_width;
        let auto_width = if !lazy {
            None
        } else if let Some(w) = rendered_width {
            let w = if auto_css_width { 300.0 } else { w };
            if let Some(mut p) = self.doc_mut(page) {
                p.images.auto_widths.insert(node, w);
            }
            Some(w)
        } else if bindings.dom.borrow().is_connected(node).unwrap_or(false) {
            // Connected but not rendered (display: none): the last width; a disconnected image
            // has none.
            self.doc(page).and_then(|p| p.images.auto_widths.get(&node).copied())
        } else {
            None
        };
        // Not rendered as of the last layout (or detached): select now, fetch once rendered. Not
        // in that layout yet (inserted since): fetched now, as an eager image.
        let deferred_now = hidden;
        let stored = self.doc_mut(page).and_then(|mut p| if deferred_now { None } else { p.images.deferred.remove(&node) });
        let candidate = match stored {
            // `loading` stopped being lazy: the source selected while deferred is fetched.
            Some(selected) if !lazy && !restart => selected,
            _ => bindings.dom.borrow().picture_candidate_auto(node, &bindings.viewport(), auto_width),
        };
        if deferred_now {
            // Not rendered and lazy: select now, fetch once rendered (or eager).
            bindings.dom.borrow_mut().set_image_source(node, candidate.as_ref().map(|c| c.url.as_str()));
            if let Some(mut p) = self.doc_mut(page) {
                p.images.deferred.insert(node, candidate);
            }
            return;
        }
        let Some(state) = self.doc(page) else { return };
        let density = candidate.as_ref().map_or(1.0, |c| c.density);
        let picture_source = candidate.map(|c| c.url);
        // The renderer draws the selected source (its snapshot's `src`).
        bindings.dom.borrow_mut().set_image_source(node, picture_source.as_deref());
        let source = {
            let dom = bindings.dom.borrow();
            match picture_source.as_deref().or(dom.get_attribute(node, "src").ok().flatten()) {
                None => Source::None,
                Some(v) if v.trim().is_empty() => Source::Invalid,
                // `<base href>` aware; the referrer stays the frame's URL.
                Some(v) => url::Url::parse(&crate::loader::node_base_url(&dom, node).unwrap_or_else(|| base.clone()))
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    .map_or(Source::Invalid, |u| Source::Url(u.to_string())),
            }
        };
        let policy = crate::loader::element_referrer_policy(&bindings.dom.borrow(), node).unwrap_or_else(|| bindings.referrer_policy());
        let request_state = (image_cors(&bindings.dom.borrow(), node), Some(policy), url::Url::parse(&base).ok());
        let available = match &source {
            Source::Url(url) if density == 1.0 => bindings.available_html_image(url, request_state.0, policy, request_state.2.clone()),
            _ => None,
        };
        let current =
            state.images.entries.get(&node).map(|i| (i.url.clone(), i.pending, i.request_state == request_state, i.outcome));
        if let Some(old) = state.images.entries.get(&node).filter(|old| {
            old.request_state != request_state || !matches!(&source, Source::Url(url) if *url == old.url)
        }) {
            if let Some(bodies) = self.layout_bodies(page.page) {
                let (cors, policy, source) = &old.request_state;
                let options = pyro_dom::layout::SubresourceOptions {
                    crossorigin: match cors { 0 => None, 2 => Some("use-credentials".into()), _ => Some("anonymous".into()) },
                    referrer: source.as_ref().map(ToString::to_string),
                    referrer_policy: policy.map(|policy| policy.as_str().to_owned()),
                    ..Default::default()
                };
                bodies.invalidate_image(&pyro_dom::layout::LayoutBodyKey::new(&crate::document_client(&state.main_frame), &old.url, &options, pyro_dom::layout::SubresourceKind::Image));
            }
        }
        drop(state);
        let url = match source {
            Source::Url(url) if current.as_ref().is_some_and(|(u, pending, same, _)| *u == url && *same && (!restart || *pending)) => {
                // Same request: a `decode()` made since waits for nothing.
                if current.is_some_and(|(_, pending, _, _)| !pending) {
                    bindings.settle_image_decodes(node);
                }
                return;
            }
            // A restarted request for the image already loaded: HTML's list of available
            // images gives it again without fetching, and `load` (or `error`) fires again.
            Source::Url(url) if current.as_ref().is_some_and(|(u, pending, same, outcome)| *u == url && *same && !pending && outcome.is_some()) => {
                let ok = current.and_then(|(_, _, _, outcome)| outcome).unwrap_or(false);
                let weak = Rc::downgrade(&bindings);
                self.scheduler().post(TaskSource::DomManipulation, move |_b| {
                    if let Some(bindings) = weak.upgrade() {
                        bindings.settle_image_decodes(node);
                        pyro_webapi::lifecycle::resource_loaded(&bindings, node, ok);
                    }
                });
                let _ = url;
                return;
            }
            // pyro: an image in the list of available images (HTML "update the image data" step
            // 7): completely available without a fetch; `load` fires in a task.
            Source::Url(url) if available.is_some() => {
                let image = available.expect("available");
                let generation = {
                    let mut guard = self.doc_mut(page).expect("page");
                    let images = &mut guard.images;
                    images.generation += 1;
                    let generation = images.generation;
                    images.track(
                        node,
                        Image {
                            _node: None,
                            url: url.clone(),
                            src: raw_src.clone().unwrap_or_default(),
                            density,
                            generation,
                            pending: false,
                            blocks_load: false,
                            cancel: None,
                            request_state: request_state.clone(),
                            outcome: Some(true),
                        },
                    );
                    generation
                };
                let _ = generation;
                bindings.set_image_state(node, Some(raw_src.as_deref().unwrap_or("")), false);
                {
                    let mut dom = bindings.dom.borrow_mut();
                    dom.set_natural_size(node, image.natural);
                    dom.set_reported_natural_size(node, image.reported);
                }
                bindings.image_loaded(node, raw_src.as_deref().unwrap_or(""), true);
                if let Some((bytes, clean)) = image.bytes {
                    if request_state.0 != 0 || bindings.wants_image_bytes() {
                        bindings.set_image_bytes(node, bytes.bytes().clone(), clean);
                    }
                }
                let weak = Rc::downgrade(&bindings);
                self.scheduler().post(TaskSource::DomManipulation, move |_b| {
                    if let Some(bindings) = weak.upgrade() {
                        bindings.settle_image_decodes(node);
                        pyro_webapi::lifecycle::resource_loaded(&bindings, node, true);
                    }
                });
                return;
            }
            Source::Url(url) => url,
            Source::None | Source::Invalid => {
                let invalid = matches!(source, Source::Invalid);
                let mut state = self.doc_mut(page).expect("page");
                state.images.entries.remove(&node);
                state.images.offers.retain(|(_, owner, ..)| *owner != node);
                drop(state);
                bindings.set_image_state(node, None, false);
                bindings.keep_loading_image(node, false);
                { let mut dom = bindings.dom.borrow_mut(); dom.set_natural_size(node, None); dom.set_reported_natural_size(node, None); }
                if invalid {
                    self.image_error_task(page, loader, &bindings, node);
                }
                return;
            }
        };
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
        let generation = {
            let mut guard = self.doc_mut(page).expect("page");
            let images = &mut guard.images;
            images.generation += 1;
            let generation = images.generation;
            if images.entries.len() >= images.sweep_at {
                let dom = bindings.dom.borrow();
                images.entries.retain(|id, i| i.pending || dom.is_alive(*id));
                images.sweep_at = (images.entries.len() * 2).max(64);
            }
            // Replacing an entry drops its cancellation, stopping the old request.
            images.track(
                node,
                Image {
                    _node: Some(hold),
                    url: url.clone(),
                    src: raw_src.clone().unwrap_or_default(),
                    density,
                    generation,
                    pending: true,
                    blocks_load: !loaded,
                    cancel: None,
                    request_state,
                    outcome: None,
                },
            );
            generation
        };
        // A `srcset`/`<picture>` source without `src` is a request too
        // (`None` would clear the image and reject pending `decode()`s).
        bindings.set_image_state(node, Some(raw_src.as_deref().unwrap_or("")), true);
        bindings.keep_loading_image(node, true);
        { let mut dom = bindings.dom.borrow_mut(); dom.set_natural_size(node, None); dom.set_reported_natural_size(node, None); }
        let weak = Rc::downgrade(&bindings);
        let finish = move |b: &mut Browser, result, key| {
            let Some(bindings) = weak.upgrade() else { return };
            b.finish_image(page, &loader, &bindings, node, generation, result, key);
        };
        if url.starts_with("blob:") {
            let result = bindings
                .blob_url(&url)
                .map(|(bytes, mime)| Resource {
                    url: url.clone(),
                    status: 200,
                    mime: mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase(),
                    charset: None,
                    body: bytes.to_vec().into(),
                    immutable_local: false,
                    content_disposition: None,
                    header_csp: None,
                    refresh: None,
                    content_language: None,
                    referrer_policy: None,
                    sent_referrer: None,
                    timing: None,
                    partial: None,
                nosniff: false, opaque: false,
                })
                .ok_or_else(|| "revoked or unknown blob URL".to_owned());
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, result, None));
            return;
        }
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into()), None));
            return;
        };
        let frame = self.doc(page).expect("page").main_frame.clone();
        let request = FetchRequest {
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind: ResourceKind::Image,
            page: page.page,
            referrer: Some(base),
            referrer_policy: crate::loader::element_referrer_policy(&bindings.dom.borrow(), node),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: bindings.dom.borrow().get_attribute(node, "crossorigin").ok().flatten().map(str::to_owned),
            initiator: Some(pyro_webapi::resource_timing::Initiator::Img),
            module: None,
            part: None,
        };
        let key = Shape::of(self, &request);
        let cancel = self.dispatch_image(page, &resource_loader, request, Box::new(move |browser, result| finish(browser, result, key)), false);
        if let Some(mut image) = self.doc_mut(page).and_then(|p| p.filter_map(|p| p.images.entries.get_mut(&node))) {
            image.cancel = cancel;
        }
    }

    fn finish_image(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        bindings: &Rc<PageDom>,
        node: NodeId,
        generation: u64,
        result: Result<Resource, String>,
        key: Option<Shape>,
    ) {
        // A src change queued in this task supersedes this result.
        self.drain_dom_resources(page);
        let Some(mut state) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
        let Some(image) = state.images.entries.get_mut(&node).filter(|i| i.generation == generation && i.pending)
        else {
            return;
        };
        if let Some(cancel) = image.cancel.take() {
            cancel.disarm();
        }
        image.pending = false;
        let src = image.src.clone();
        let request_url = image.url.clone();
        let cors_state = image.request_state.0;
        let density = image.density;
        // A CORS request (`crossorigin`): its response passed the CORS check, so its pixels may
        // be read back, which is what the attribute is for (the bytes are kept for a canvas
        // the script makes later).
        let cors = image.request_state.0 != 0;
        // Nothing needs the element pinned once its request completed.
        let hold = image._node.take();
        drop(state);
        // `decode()` resolves for a parsed header or a format without a
        // header parser; a PNG/GIF/JPEG/WebP whose header does not parse is
        // corrupt.
        let mut decodable = false;
        let mut raster = None;
        let mut reported = None;
        let outcome = result.ok().filter(|r| (200..300).contains(&r.status) && !r.body.is_empty()).and_then(|r| {
            let body = &r.body[..r.body.len().min(MAX_IMAGE_BYTES)];
            if let Some((w, h)) = pyro_dom::image::dimensions(body) {
                decodable = true;
                raster = Some(r);
                return Some(Some((w as f64, h as f64)));
            }
            if r.mime == "image/svg+xml" || body.trim_ascii_start().starts_with(b"<svg") {
                decodable = true;
                reported = svg_reported_size(body, density);
                let size = svg_size(body);
                // pyro: its bytes too, for a canvas that draws it (pyro-canvas rasterizes SVG).
                raster = Some(r);
                return Some(size);
            }
            decodable = !matches!(r.mime.as_str(), "image/png" | "image/gif" | "image/jpeg" | "image/webp");
            r.mime.starts_with("image/").then_some(None)
        });
        let decodable = decodable && outcome.is_some();
        // The density-corrected natural size (HTML): a `2x` candidate of 200 px is 100 px wide.
        let natural =
            outcome.flatten().map(|(w, h)| if density > 0.0 && density != 1.0 { (w / density, h / density) } else { (w, h) });
        // An SVG with one natural dimension is laid out at its completed, density-corrected
        // size; one with none keeps sizing from its ratio in layout.
        let natural = natural.or(reported.filter(|(_, intrinsic)| *intrinsic).map(|(size, _)| size));
        let reported = reported.map(|(size, _)| size);
        {
            let mut dom = bindings.dom.borrow_mut();
            dom.set_natural_size(node, natural);
            dom.set_reported_natural_size(node, reported);
        }
        bindings.image_loaded(node, &src, decodable);
        // pyro: a successful load goes into the list of available images (HTML "update the image
        // data"), its bytes with it for a canvas the script makes later.
        let raster = raster.filter(|r| r.body.len() <= MAX_IMAGE_BYTES);
        let request_referrer = key.as_ref().map(|key| (key.policy, key.referrer_source.clone(), key.referer.clone()));
        if let Some(key) = key {
            let mut used = false;
            if let (Some(state), Some(bodies)) = (self.doc(page), self.layout_bodies(page.page)) {
                for (slot, source, policy) in bodies.dispatched_images(&crate::document_client(&state.main_frame), &request_url) {
                    let options = slot.options();
                    let mut request = crate::subresource_request_with(&state.main_frame, page.page, request_url.clone(), crate::ResourceKind::Image, &crate::SubresourceOptions {
                        crossorigin: options.crossorigin.clone(), referrer: options.referrer.clone(),
                        referrer_policy: options.referrer_policy.as_deref().and_then(pyro_net::ReferrerPolicy::parse),
                        ..Default::default()
                    });
                    let current = Shape::of(self, &request);
                    request.referrer = source;
                    request.referrer_policy = pyro_net::ReferrerPolicy::parse(&policy);
                    if Shape::of(self, &request).as_ref() != Some(&key) {
                        if current.as_ref() == Some(&key) { bodies.invalidate_image(&slot); }
                        continue;
                    }
                    let body = raster.as_ref().filter(|resource| resource.immutable_local).map(|resource| pyro_dom::layout::LayoutBody {
                        image_lease: None,
                        bytes: resource.body.clone(), final_url: resource.url.clone(), status: resource.status,
                        opaque: crate::loader::is_opaque(resource, cors, &state.main_frame.origin),
                        referrer_policy: resource.referrer_policy.map(|policy| policy.as_str().to_owned()),
                    });
                    used |= bodies.refresh_image(&slot, body);
                }
            }
            if !used && bindings.dom.borrow().is_connected(node) == Ok(true) {
                if let (Some(mut state), Some(resource)) = (self.doc_mut(page), raster.as_ref()) {
                    state.images.offer(key, node, resource, &bindings);
                }
            }
        }
        // pyro: a no-cors response is clean only if its tainting is "basic": not one whose redirect
        // chain went to another origin, though it ended at the document's.
        let clean = raster.as_ref().is_some_and(|r| cors || (!r.opaque && self.image_origin_clean(page, &r.url)));
        if outcome.is_some() && density == 1.0 {
            bindings.remember_available_image(
                &request_url,
                cors_state,
                pyro_webapi::AvailableImage {
                    request_referrer,
                    natural,
                    reported,
                    bytes: raster.as_ref().and_then(|r| bindings.retain_image_source(r.body.clone())).map(|bytes| (bytes, clean)),
                },
            );
        }
        // Canvas drawing decodes the pixels from the bytes, when the
        // document may draw images (it has a canvas or used canvas APIs).
        if let Some(r) = raster.filter(|_| cors || bindings.wants_image_bytes()) {
            bindings.set_image_bytes(node, r.body, clean);
        }
        if let Some(mut state) = self.doc_mut(page) {
            if let Some(image) = state.images.entries.get_mut(&node).filter(|i| i.generation == generation) {
                image.outcome = Some(outcome.is_some());
            }
        }
        // A later owner-thread task: no DOM/state borrow survives dispatch.
        pyro_webapi::lifecycle::resource_loaded(bindings, node, outcome.is_some());
        bindings.keep_loading_image(node, false);
        drop(hold);

        self.maybe_fire_load(page);
    }

    /// HTML "origin-clean" for an image response at `url` (no CORS mode:
    /// images load without `crossorigin`): same-origin with the document,
    /// or a `data:`/`blob:` URL.
    pub(crate) fn image_origin_clean(&self, page: FrameKey, url: &str) -> bool {
        if url.starts_with("data:") || url.starts_with("blob:") {
            return true;
        }
        let Some(state) = self.doc(page) else { return false };
        // The document's origin: opaque in a sandboxed frame, inherited by
        // about:blank and about:srcdoc documents.
        let document = &state.main_frame.origin;
        document != "null" && *document == crate::loader::origin_of(url)
    }

    /// HTML `<object>` "obtain the resource": a connected, rendered `<object>` with a `data` URL fetches it;
    /// a failed fetch (network error, a non-2xx status) fires `error`, any other response `load`
    /// (what the content is drawn as is the renderer's). An object without `data` gets no
    /// event (it shows its fallback content). Pending fetches delay the window's `load`.
    pub(crate) fn object_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let base = state.main_frame.base_url.clone();
        let loaded = state.main_frame.loaded;
        let (connected, data) = {
            let dom = bindings.dom.borrow();
            (dom.is_connected(node).unwrap_or(false), dom.get_attribute(node, "data").ok().flatten().map(str::to_owned))
        };
        // "If the element is not being rendered": fallback, nothing is fetched (an `<object>`
        // in a `display: none` subtree neither loads nor delays the window's `load`).
        let rendered = connected && {
            let dom = bindings.dom.borrow();
            rendered_as_laid_out(&bindings.layout(), &dom, node) != Some(false)
        };
        if connected && !rendered && data.is_some() {
            self.note_hidden_object(page, node);
        }
        let source = match data.filter(|_| rendered) {
            None => Source::None,
            Some(v) if v.trim().is_empty() => Source::None,
            Some(v) => {
                let dom = bindings.dom.borrow();
                url::Url::parse(&crate::loader::node_base_url(&dom, node).unwrap_or_else(|| base.clone()))
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    .map_or(Source::Invalid, |u| Source::Url(u.to_string()))
            }
        };
        drop(state);
        self.fetch_for_load_event(page, node, &bindings, loader, base, loaded, source, false, 0);
    }

    /// The deferred lazy images of `page` that are rendered now run their steps again (select
    /// with their layout, then fetch).
    pub(crate) fn fetch_rendered_images(&mut self, page: FrameKey) {
        let Some(mut state) = self.doc_mut(page) else { return };
        if let Some(bindings) = state.main_frame.page_dom.as_ref().filter(|_| !state.images.offers.is_empty()).cloned() {
            let dom = bindings.dom.borrow();
            if state.images.offers_tree_version != Some(dom.tree_version()) {
                state.images.offers.retain(|(_, owner, ..)| dom.is_connected(*owner) == Ok(true));
                state.images.offers_tree_version = Some(dom.tree_version());
            }
        }
        if state.images.deferred.is_empty() {
            return;
        }
        let nodes: Vec<NodeId> = state.images.deferred.keys().copied().collect();
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let checked = state.images.checked_at;
        drop(state);
        // Decided once per completed layout (this runs at every microtask checkpoint): the
        // engine's answers change only when it syncs, and nothing here makes it sync. An image
        // connected since that layout is not in it: it is fetched as an eager image.
        let synced = bindings.layout().style().engine_sync_stamp();
        let stamp_moved = synced != checked;
        if let Some(mut p) = self.doc_mut(page) {
            p.images.checked_at = synced;
        }
        for node in nodes {
            let rendered = {
                let dom = bindings.dom.borrow();
                if !dom.is_alive(node) {
                    drop(dom);
                    if let Some(mut p) = self.doc_mut(page) {
                        p.images.deferred.remove(&node);
                    }
                    continue;
                }
                dom.is_connected(node).unwrap_or(false)
                    && match rendered_as_laid_out(&bindings.layout(), &dom, node) {
                        Some(rendered) => stamp_moved && rendered,
                        None => true,
                    }
            };
            if rendered {
                self.image_event_with(page, node, false);
            }
        }
    }

    /// Remembers the `<object>`/`<embed>` `node`, not fetched while not being rendered.
    fn note_hidden_object(&self, page: FrameKey, node: NodeId) {
        if let Some(p) = self.page(page.page) {
            let mut hidden = p.frames.hidden_objects.borrow_mut();
            if !hidden.iter().any(|(_, n)| *n == node) {
                hidden.push((page, node));
            }
        }
    }

    /// HTML runs an `<object>`'s or `<embed>`'s steps again when its rendering changes: those not
    /// fetched while not being rendered that are rendered now are fetched (`object_event`,
    /// `embed_event`, which remember them again when they are still not rendered).
    pub(crate) fn fetch_rendered_objects(&mut self, host: &Rc<crate::frames::FrameHost>) {
        if host.hidden_objects.borrow().is_empty() {
            return;
        }
        // Decided once per completed layout, never syncing (this runs at every microtask
        // checkpoint): nothing changes until the engine syncs again.
        let synced = host.hidden_objects.borrow().first().and_then(|(page, _)| {
            let bindings = self.doc(*page).and_then(|p| p.main_frame.page_dom.clone())?;
            let stamp = bindings.layout().style().engine_sync_stamp();
            Some(stamp)
        });
        // An object connected since that layout is not in it: checked (and fetched) now.
        let unseen = host.hidden_objects.borrow().iter().any(|(page, node)| {
            self.doc(*page).and_then(|p| p.main_frame.page_dom.clone()).is_some_and(|b| {
                let dom = b.dom.borrow();
                dom.is_connected(*node).unwrap_or(false) && b.layout().style().displayed_as_synced(&dom, *node).is_none()
            })
        });
        if let Some(stamp) = synced {
            if stamp == host.hidden_checked_at.get() && !unseen {
                return;
            }
            host.hidden_checked_at.set(stamp);
        }
        let hidden = std::mem::take(&mut *host.hidden_objects.borrow_mut());
        for (page, node) in hidden {
            let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) else { continue };
            let (connected, embed, rendered) = {
                let dom = bindings.dom.borrow();
                let connected = dom.is_connected(node).unwrap_or(false);
                let embed = dom.element(node).is_ok_and(|e| &*e.name.local == "embed");
                (connected, embed, connected && rendered_as_laid_out(&bindings.layout(), &dom, node) != Some(false))
            };
            if !connected {
                continue;
            }
            if !rendered {
                host.hidden_objects.borrow_mut().push((page, node));
            } else {
                self.apply_same_document_url(page);
                if embed { self.embed_event(page, node); } else { self.object_event(page, node); }
            }
        }
    }

    /// `<embed src>` (HTML "the embed element"): a rendered one fetches its `src`; the response's
    /// Content-Type decides whether it shows a document (a child navigable, which fires its
    /// `load`), else `load` fires when the fetch is done. An `<embed>` whose `type` or URL already
    /// names a document is a frame container from the start (frames.rs).
    pub(crate) fn embed_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let base = state.main_frame.base_url.clone();
        let loaded = state.main_frame.loaded;
        let (connected, src) = {
            let dom = bindings.dom.borrow();
            (dom.is_connected(node).unwrap_or(false), dom.get_attribute(node, "src").ok().flatten().map(str::to_owned))
        };
        let rendered = connected && {
            let dom = bindings.dom.borrow();
            rendered_as_laid_out(&bindings.layout(), &dom, node) != Some(false)
        };
        if connected && !rendered && src.is_some() {
            self.note_hidden_object(page, node);
        }
        let source = match src.filter(|_| rendered) {
            None => Source::None,
            Some(v) if v.trim().is_empty() => Source::None,
            Some(v) => {
                let dom = bindings.dom.borrow();
                url::Url::parse(&crate::loader::node_base_url(&dom, node).unwrap_or_else(|| base.clone()))
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    // A `javascript:` URL in `src` does nothing (no fetch, no `load`).
                    .filter(|u| u.scheme() != "javascript")
                    .map_or(Source::None, |u| Source::Url(u.to_string()))
            }
        };
        drop(state);
        self.fetch_for_load_event(page, node, &bindings, loader, base, loaded, source, false, 0);
    }

    /// SVG `<image>` (SVG 2 §5.7): a connected one fetches its `href` (or
    /// `xlink:href`) for its `load` and `error` events, as `<object>` does
    /// (the renderer draws the image itself). No `href`, no event. Its fetch
    /// delays the window's `load`. Instances of `<use>` load theirs.
    pub(crate) fn svg_image_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let base = state.main_frame.base_url.clone();
        let loaded = state.main_frame.loaded;
        let href = {
            let dom = bindings.dom.borrow();
            let connected = dom.is_connected(node).unwrap_or(false)
                || dom.root_node(node, true).ok().is_some_and(|r| dom.is_connected(r).unwrap_or(false));
            connected
                .then(|| {
                    dom.get_attribute(node, "href")
                        .ok()
                        .flatten()
                        .or_else(|| dom.get_attribute_ns(node, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten())
                        .map(str::to_owned)
                })
                .flatten()
        };
        let source = match href {
            None => Source::None,
            Some(v) if v.trim().is_empty() => Source::None,
            Some(v) => {
                let dom = bindings.dom.borrow();
                url::Url::parse(&crate::loader::node_base_url(&dom, node).unwrap_or_else(|| base.clone()))
                    .ok()
                    .and_then(|b| b.join(v.trim()).ok())
                    .map_or(Source::Invalid, |u| Source::Url(u.to_string()))
            }
        };
        drop(state);
        // pyro: its `crossorigin` (a CORS settings attribute) is its request's mode, as an `<img>`'s.
        let cors = image_cors(&bindings.dom.borrow(), node);
        self.fetch_for_load_event(page, node, &bindings, loader, base, loaded, source, true, cors);
    }

    /// Fetches `source` for `node`'s `load` / `error` events only (`<object>`,
    /// SVG `<image>`): a failed fetch or a non-2xx status is `error`, and with
    /// `image` so is a body no image decoder takes (an empty one, or a PNG,
    /// GIF, JPEG or WebP whose header does not parse).
    #[allow(clippy::too_many_arguments)]
    fn fetch_for_load_event(
        &mut self,
        page: FrameKey,
        node: NodeId,
        bindings: &Rc<PageDom>,
        loader: LoaderId,
        base: String,
        loaded: bool,
        source: Source,
        image: bool,
        // pyro: the CORS state of an SVG `<image>`'s request (0: none, 1: anonymous, 2:
        // use-credentials).
        cors: u8,
    ) {
        let bindings = bindings.clone();
        let Some(state) = self.doc(page) else { return };
        // pyro: the request it has: its URL and its CORS mode (a change of either is a new request,
        // with its own tainting, never the old image).
        let current = state.images.entries.get(&node).map(|i| (i.url.clone(), i.request_state.0));
        drop(state);
        let url = match source {
            Source::Url(url) if current.as_ref() == Some(&(url.clone(), cors)) => return,
            Source::Url(url) => url,
            Source::None => {
                self.doc_mut(page).expect("page").images.entries.remove(&node);
                return;
            }
            Source::Invalid => {
                self.doc_mut(page).expect("page").images.entries.remove(&node);
                self.image_error_task(page, loader, &bindings, node);
                return;
            }
        };
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
        let generation = {
            let mut guard = self.doc_mut(page).expect("page");
            let images = &mut guard.images;
            images.generation += 1;
            let generation = images.generation;
            images.track(
                node,
                Image {
                    _node: Some(hold),
                    url: url.clone(),
                    src: String::new(),
                    density: 1.0,
                    generation,
                    pending: true,
                    blocks_load: !loaded,
                    cancel: None,
                    request_state: (cors, None, None),
                    outcome: None,
                },
            );
            generation
        };
        let weak = Rc::downgrade(&bindings);
        // pyro: the layout body key of the request as sent (set below, once it is built).
        let key: Rc<std::cell::RefCell<Option<pyro_dom::layout::LayoutBodyKey>>> = Rc::default();
        let finish_key = key.clone();
        let finish = move |b: &mut Browser, result: Result<Resource, String>| {
            let Some(bindings) = weak.upgrade() else { return };
            let key = finish_key.borrow_mut().take();
            b.finish_object(page, &loader, &bindings, node, generation, result, image, key);
        };
        if url.starts_with("blob:") {
            let ok = bindings.blob_url(&url).is_some();
            self.scheduler().post(TaskSource::Networking, move |b| {
                finish(b, if ok { Ok(Resource { url: String::new(), status: 200, mime: String::new(), charset: None, body: Default::default(), immutable_local: false, content_disposition: None, header_csp: None, refresh: None, content_language: None, referrer_policy: None, sent_referrer: None, timing: None, partial: None, nosniff: false, opaque: false }) } else { Err("revoked or unknown blob URL".into()) })
            });
            return;
        }
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let frame = self.doc(page).expect("page").main_frame.clone();
        let request = FetchRequest {
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind: ResourceKind::Object,
            page: page.page,
            referrer: Some(base),
            referrer_policy: None,
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: match cors {
                0 => None,
                2 => Some("use-credentials".to_owned()),
                _ => Some("anonymous".to_owned()),
            },
            initiator: Some(pyro_webapi::resource_timing::Initiator::Other),
            module: None,
            part: None,
        };
        *key.borrow_mut() = Some(crate::layout_key_of(&frame, &request, None));
        let cancel = self.fetch_document_resource(page, &resource_loader, request, Box::new(finish));
        if let Some(mut image) = self.doc_mut(page).and_then(|p| p.filter_map(|p| p.images.entries.get_mut(&node))) {
            image.cancel = cancel;
        }
    }

    fn finish_object(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        bindings: &Rc<PageDom>,
        node: NodeId,
        generation: u64,
        result: Result<Resource, String>,
        image: bool,
        // pyro: the layout body key of the request as sent (`layout_key_of`).
        key: Option<pyro_dom::layout::LayoutBodyKey>,
    ) {
        self.drain_dom_resources(page);
        let Some(mut state) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
        let Some(entry) = state.images.entries.get_mut(&node).filter(|i| i.generation == generation && i.pending)
        else {
            return;
        };
        if let Some(cancel) = entry.cancel.take() {
            cancel.disarm();
        }
        entry.pending = false;
        let hold = entry._node.take();
        let url = entry.url.clone();
        let cors = entry.request_state.0;
        drop(state);
        // pyro: an SVG `<image>`'s raster goes into the list of available images (a canvas draws
        // it from there), clean only for a CORS response or a non-opaque one of the document's
        // origin, as an `<img>`'s.
        if image {
            if let Some(r) = result.as_ref().ok().filter(|r| (200..300).contains(&r.status) && r.body.len() <= MAX_IMAGE_BYTES) {
                if let Some((w, h)) = pyro_dom::image::dimensions(&r.body) {
                    let clean = cors != 0 || (!r.opaque && self.image_origin_clean(page, &r.url));
                    bindings.remember_available_image(
                        &url,
                        cors,
                        pyro_webapi::AvailableImage {
                            request_referrer: None,
                            natural: Some((w as f64, h as f64)),
                            reported: None,
                            bytes: bindings.retain_image_source(r.body.clone()).map(|bytes| (bytes, clean)),
                        },
                    );
                }
            }
        }
        // The page's layout engine lays the element out from the same body
        // (an `<object>` image's size), which it would otherwise fetch on its
        // own and might not have when `load` fires and scripts measure.
        // pyro: the document's own no-cors response only, with its URL after redirects, tainting
        // and status (`provide_layout_body`); a CORS one (an SVG `<image crossorigin>`'s) is another
        // request, which layout makes itself.
        if let Some(r) = result
            .as_ref()
            .ok()
            .filter(|r| cors == 0 && (200..300).contains(&r.status) && r.body.len() <= MAX_IMAGE_BYTES)
        {
            if let Some(key) = key {
                self.provide_layout_body(page, key, pyro_dom::layout::LayoutBody {
                    image_lease: None,
                    bytes: r.body.clone(),
                    final_url: r.url.clone(),
                    opaque: r.opaque,
                    status: r.status,
                    referrer_policy: r.referrer_policy.map(|p| p.as_str().to_owned()),
                });
            }
        }
        // HTML "the object element": the response's Content-Type decides what the object shows.
        // A document gets a child navigable (which loads it and fires the element's `load`), as
        // when `type` says so.
        if !image {
            if let Some(r) = result.as_ref().ok().filter(|r| (200..300).contains(&r.status)) {
                let essence = r.mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
                // Without a Content-Type, the element's `type` or URL says.
                let document = if essence.is_empty() {
                    bindings.dom.borrow().element(node).is_ok_and(pyro_dom::iframe::object_hosts_document)
                } else {
                    matches!(essence.as_str(), "text/html" | "application/xhtml+xml")
                };
                if document {
                    // A later fetch of the same URL (after a `type` change) runs again.
                    if let Some(mut state) = self.doc_mut(page) {
                        state.images.entries.remove(&node);
                    }
                    bindings.dom.borrow_mut().set_object_document(node);
                    drop(hold);
                    self.drain_dom_resources(page);
                    self.maybe_fire_load(page);
                    return;
                }
            }
        }
        // An `<embed>` fires `load` once its fetch is done, whatever came back.
        let embed = !image && bindings.dom.borrow().element(node).is_ok_and(|e| &*e.name.local == "embed");
        let ok = embed || result.is_ok_and(|r| {
            (200..300).contains(&r.status)
                && (!image || {
                    let body = &r.body[..r.body.len().min(MAX_IMAGE_BYTES)];
                    !body.is_empty()
                        && (pyro_dom::image::dimensions(body).is_some()
                            || r.mime == "image/svg+xml"
                            || body.trim_ascii_start().starts_with(b"<svg")
                            || !matches!(r.mime.as_str(), "image/png" | "image/gif" | "image/jpeg" | "image/webp"))
                })
        });
        pyro_webapi::lifecycle::resource_loaded(bindings, node, ok);
        drop(hold);
        self.maybe_fire_load(page);
    }

    fn image_error_task(&self, page: FrameKey, loader: LoaderId, bindings: &Rc<PageDom>, node: NodeId) {
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = ResourceNode { dom: bindings.dom.clone(), node };
        let weak = Rc::downgrade(bindings);
        self.scheduler().post(TaskSource::Networking, move |b| {
            let _pin = &hold;
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                if let Some(bindings) = weak.upgrade() {
                    pyro_webapi::lifecycle::resource_loaded(&bindings, hold.node, false);
                }
            }
        });
    }
}
