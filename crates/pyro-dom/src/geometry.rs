//! The geometry layout queries report: what an external engine (Blitz, in
//! pyro-render) laid the document out as. pyro-dom has no layout of its own; a
//! [`GeometrySource`] answers per element, and a [`LayoutResult`] holds the
//! whole document's boxes and words for queries that need them all (hit
//! testing, text ranges, carets, scroll anchoring).

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::layout::{Layout, Rect};
use crate::node::NodeId;

/// An element's `position`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoxPosition {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// Sides of a box: top, right, bottom, left.
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// One element's boxes from the external engine (document CSS px).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExternalBox {
    /// Border box.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Padding box size (`clientWidth/Height`).
    pub client_width: f64,
    pub client_height: f64,
    /// `scrollWidth/Height`.
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// Whether its scroll origin is at the end of the horizontal and the vertical axis (a
    /// right-to-left or reversed flex scroll container): it scrolls to negative offsets there.
    pub scroll_reversed: [bool; 2],
    /// Inside (or itself) a `position: fixed` box (then viewport coordinates).
    pub fixed: bool,
    /// Border and padding widths ([`TOP`], [`RIGHT`], [`BOTTOM`], [`LEFT`]).
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
    /// Discarded by a line clamp: laid out, not painted, not hit.
    pub hidden: bool,
    /// The border box's origin with no CSS transform applied, when a transform moves it
    /// (`offsetLeft`/`offsetTop` ignore transforms).
    pub layout_origin: Option<[f64; 2]>,
    /// A flex or grid container whose items paint with the inline content of its painter, its own
    /// box with the block backgrounds (the engine's paint phases; hit testing follows them).
    pub items_in_phases: bool,
}

/// One word of a text node on a line (document CSS px).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalWord {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub fixed: bool,
    pub vertical_edges: bool,
    /// UTF-16 offsets and their document coordinate on the physical inline axis.
    pub edges: Vec<(u32, f64)>,
}

/// A `position: sticky` element's inputs (its box is laid out as if static).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExternalSticky {
    /// [`TOP`], [`RIGHT`], [`BOTTOM`], [`LEFT`] insets: px plus a fraction
    /// of the scrollport (`None`: auto).
    pub insets: [Option<(f64, f64)>; 4],
    /// Its containing block's content box (x, y, width, height), document
    /// coordinates: the box never leaves it.
    pub limit: Option<(f64, f64, f64, f64)>,
    /// The [`crate::snapshot::render_key`] of its containing block's element:
    /// the sticky boxes holding that element move the limit with them.
    pub cb: Option<u64>,
}

/// The whole document's boxes as the external engine laid it out, keyed
/// by [`crate::snapshot::render_key`] (the words: [`GeometrySource::text_words`]).
#[derive(Clone, Debug, Default)]
pub struct ExternalGeometry {
    /// Element boxes in tree order.
    pub boxes: Vec<(u64, ExternalBox)>,
    pub sticky: Vec<(u64, ExternalSticky)>,
    /// Scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
}

/// A viewport point to hit-test ([`GeometrySource::hit_candidates`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitProbe {
    pub x: f64,
    pub y: f64,
    /// Only `y` counts (WebDriver's vertical hit test).
    pub ignore_x: bool,
    /// The window scroll offset: a box outside every `position: fixed` box
    /// lies at the point plus it in document coordinates.
    pub window_scroll: (f64, f64),
}

/// One element [`GeometrySource::hit_candidates`] reports: a candidate
/// (`hit`: its box may hold the point) or an ancestor of one, with its box
/// and its place in tree order among the reported elements.
#[derive(Clone, Debug, PartialEq)]
pub struct HitVisit {
    pub key: u64,
    pub order: u32,
    pub bounds: ExternalBox,
    pub hit: bool,
}

/// An element's CSS boxes as quads (CDP `DOM.getBoxModel` and
/// `getContentQuads`), document CSS px (viewport ones inside a fixed box).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalQuads {
    /// The border box as four points (x, y), clockwise from its top-left
    /// corner, after the transforms of the element and its containing
    /// blocks.
    pub border: [f64; 8],
    /// Whether a transform other than a translation applies (else `border`
    /// is the element's axis-aligned box).
    pub transformed: bool,
    /// Border, padding and margin widths ([`TOP`], [`RIGHT`], [`BOTTOM`],
    /// [`LEFT`]), in the element's own (untransformed) space.
    pub border_widths: [f64; 4],
    pub padding: [f64; 4],
    pub margin: [f64; 4],
    /// The untransformed border box size (`offsetWidth`/`offsetHeight`).
    pub width: f64,
    pub height: f64,
    /// One border-box quad per line fragment of an inline box laid out over
    /// several lines (empty for other boxes).
    pub fragments: Vec<[f64; 8]>,
    pub fixed: bool,
}

/// What a sub-resource a geometry source loads is for (the request's
/// destination: its `Accept`, `Sec-Fetch-Dest` and CDP resource type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubresourceKind {
    Image,
    /// An image fetched in CORS mode with credentials only to the same origin ("anonymous", a
    /// `shape-outside` image): a cross-origin response the CORS check does not pass fails.
    CorsImage,
    Font,
    /// An `@import`ed sheet.
    Style,
}

/// pyro: what a sub-resource request of a geometry source carries beyond its URL, as a CSS
/// `url()`'s request URL modifiers ask (CSS Values 5 § 4.5.4); the defaults are a sub-resource's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubresourceOptions {
    /// `anonymous` or `use-credentials`: the request's mode is "cors", with those credentials.
    pub crossorigin: Option<String>,
    /// Integrity metadata: a response whose body matches none of it fails.
    pub integrity: Option<String>,
    /// The request's referrer (the style sheet a `url()` was written in); `None`: the document.
    pub referrer: Option<String>,
    /// The request's referrer policy keyword; `None`: the document's.
    pub referrer_policy: Option<String>,
}

/// pyro: the one key of a layout body ([`SubresourceBodies`]): the requesting DOCUMENT (its
/// client: frame id and loader id, so per document, which subsumes its origin, cookie state and
/// sandboxing) and the whole request (URL, mode and credentials, integrity metadata, referrer,
/// referrer policy). Nothing is stored or looked up by a bare URL; documents share nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutBodyKey {
    document: String,
    url: String,
    options: SubresourceOptions,
    /// What the response is used as (an image's CORS mode is in `options`).
    destination: SubresourceKind,
}

impl LayoutBodyKey {
    /// The ONE constructor: `url` fetched with `options` as `destination` for the document
    /// `document` (its client), as the request was (or is) sent.
    pub fn new(document: &str, url: &str, options: &SubresourceOptions, destination: SubresourceKind) -> Self {
        let destination = match destination {
            SubresourceKind::CorsImage => SubresourceKind::Image,
            other => other,
        };
        LayoutBodyKey { document: document.to_owned(), url: url.to_owned(), options: options.clone(), destination }
    }
    pub fn document(&self) -> &str {
        &self.document
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn options(&self) -> &SubresourceOptions {
        &self.options
    }
}

/// pyro: a layout body: the response's bytes, its URL after redirects, its tainting and its
/// status, written together once its checks (status, CORS, integrity) passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutBody {
    pub image_lease: Option<std::sync::Arc<pyro_raster::SourceLease>>,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub final_url: String,
    /// The response's tainting is "opaque" (a no-cors response of another origin, or one whose
    /// redirect chain went through one).
    pub opaque: bool,
    pub status: u16,
    /// pyro: the `Referrer-Policy` it was served with (a style sheet's requests follow it).
    pub referrer_policy: Option<String>,
}

impl LayoutBody {
    /// A body of a response that is not the network's (a `data:` URL, a local file) at `url`.
    pub fn local(url: &str, bytes: Vec<u8>) -> Self {
        LayoutBody { image_lease: None, bytes: std::sync::Arc::new(bytes), final_url: url.to_owned(), opaque: false, status: 200, referrer_policy: None }
    }
}

/// How a document's geometry source reaches the network for what it lays the
/// document out with (images, fonts, imported sheets): the embedder's loader
/// for that document ([`crate::layout::Layout::set_subresource_loader`]), which
/// sends each request as the document's own sub-resource requests go, through
/// its browser context (proxy, user agent, cookies, extra headers, offline
/// state, interception). A geometry source without one loads nothing from the
/// network.
pub trait SubresourceLoader: Send + Sync {
    /// Starts loading `url` as `options` say (a CSS `url()`'s request URL modifiers) for what
    /// `kind` says it is used as. `done` gets the response (bytes, final URL, tainting, status)
    /// once it passed its checks (a 2xx status, CORS, integrity), or why there is none, on any
    /// thread. A loader that can no longer send the request (its document is gone) drops `done`
    /// without calling it.
    /// pyro: `document` is the client the request is for (this document's, or a frame document's
    /// of the same page that this geometry renders): the loader sends it as that document's
    /// request, or not at all when that is no live document of the page.
    fn load(
        &self,
        document: &str,
        url: String,
        kind: SubresourceKind,
        options: &SubresourceOptions,
        done: Box<dyn FnOnce(Result<LayoutBody, String>) + Send>,
    );
    /// The store this document's bodies are kept in ([`SubresourceBodies`]).
    fn bodies(&self) -> &std::sync::Arc<SubresourceBodies>;
    /// pyro: this document (its client), which every key of its bodies names ([`LayoutBodyKey`]).
    fn document(&self) -> String;
    /// pyro: this document's layout asked for the font of `key`.
    fn font_asked(&self, _key: &LayoutBodyKey) {}
    /// pyro: the fonts this document's layout asked for ([`Self::font_asked`]), `None` when the
    /// loader does not keep track (then every font in flight for the document counts).
    fn asked_fonts(&self) -> Option<Vec<LayoutBodyKey>> {
        None
    }
    /// pyro: this document's layout asked for the CSS image of `key`.
    fn image_asked(&self, _key: &LayoutBodyKey) {}
    /// pyro: the CSS images this document's layout asked for ([`Self::image_asked`]), `None` when
    /// the loader does not keep track.
    fn asked_images(&self) -> Option<Vec<LayoutBodyKey>> {
        None
    }
}

/// pyro: what a document's layout did with a font request (one request key: the document, the
/// font's URL, its options): [`SubresourceBodies::font_load`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontLoad {
    /// Its layout never asked for it.
    NotAsked,
    /// Asked for and not answered yet (sent or about to be).
    Loading,
    Loaded,
    Failed,
}

/// pyro: a slot of the store: a load in flight, its body, or its failure (the document's layout
/// did fetch it: a capture waits for it as for one with a body, and it is not asked again).
#[derive(Clone)]
enum Slot {
    Pending(u64),
    Ready(std::sync::Arc<LayoutBody>),
    Failed,
}

#[derive(Default)]
struct BodySlots {
    budgets: std::collections::HashMap<String, pyro_raster::PageRasters>,
    images: indexmap::IndexMap<LayoutBodyKey, ImageSlot>,
    bytes: usize,
    other: std::collections::HashMap<LayoutBodyKey, Slot>,
}

struct ImageSlot {
    state: Slot,
    dispatched: Option<(Option<String>, String)>,
}

impl ImageSlot {
    fn cost(&self, key: &LayoutBodyKey) -> usize {
        BodySlots::cost(key, &self.state) + self.dispatched.as_ref().map_or(0, |(source, policy)| {
            source.as_ref().map_or(0, String::capacity) + policy.capacity()
        })
    }
}

impl BodySlots {
    const IMAGE_BYTES: usize = 16 << 20;
    const IMAGE_ENTRIES: usize = 256;

    fn cost(key: &LayoutBodyKey, slot: &Slot) -> usize {
        let options = &key.options;
        key.document.capacity() + key.url.capacity() + [&options.crossorigin, &options.integrity, &options.referrer, &options.referrer_policy]
            .into_iter().flatten().map(String::capacity).sum::<usize>() + match slot {
                Slot::Ready(body) => body.bytes.capacity() + body.final_url.capacity() + body.referrer_policy.as_ref().map_or(0, String::capacity),
                _ => 0,
            }
    }

    fn get(&self, key: &LayoutBodyKey) -> Option<&Slot> {
        if key.destination == SubresourceKind::Image { self.images.get(key).map(|image| &image.state) } else { self.other.get(key) }
    }

    fn remove(&mut self, key: &LayoutBodyKey) -> Option<Slot> {
        if key.destination != SubresourceKind::Image { return self.other.remove(key); }
        let (key, slot) = self.images.shift_remove_entry(key)?;
        self.bytes -= slot.cost(&key);
        Some(slot.state)
    }

    fn insert(&mut self, key: LayoutBodyKey, mut slot: Slot) {
        if key.destination != SubresourceKind::Image { self.other.insert(key, slot); return; }
        if let (Slot::Ready(body), Some(page)) = (&mut slot, self.budgets.get(&key.document)) {
            match page.retain_source(body.bytes.clone()) {
                Ok(lease) => std::sync::Arc::make_mut(body).image_lease = Some(lease),
                Err(_) => slot = Slot::Failed,
            }
        }
        let dispatched = self.images.shift_remove_entry(&key).and_then(|(old, slot)| {
            self.bytes -= slot.cost(&old);
            slot.dispatched
        });
        self.insert_image(key, ImageSlot { state: slot, dispatched });
    }

    /// Keeps `key`'s image, within its document's share: at most `IMAGE_ENTRIES` images and
    /// `IMAGE_BYTES` per document, its oldest going first. A store serves every context with the
    /// same network identity (`Browser::layout_bodies_for`), so a bound over all of them would let
    /// one document's images evict another's (a page's ready body gone before its screenshot).
    fn insert_image(&mut self, key: LayoutBodyKey, mut slot: ImageSlot) {
        if slot.cost(&key) > Self::IMAGE_BYTES { slot.state = Slot::Failed; }
        let cost = slot.cost(&key);
        if cost > Self::IMAGE_BYTES { return; }
        let (mut count, mut bytes) = self.images.iter().filter(|(k, _)| k.document == key.document)
            .fold((0usize, 0usize), |(count, bytes), (k, slot)| (count + 1, bytes + slot.cost(k)));
        while count >= Self::IMAGE_ENTRIES || bytes > Self::IMAGE_BYTES - cost {
            let Some(index) = self.images.iter().position(|(k, _)| k.document == key.document) else { break };
            let (old, slot) = self.images.shift_remove_index(index).unwrap();
            let freed = slot.cost(&old);
            self.bytes -= freed;
            (count, bytes) = (count - 1, bytes - freed);
        }
        self.images.insert(key, slot);
        self.bytes += cost;
    }
}

/// Sub-resource bodies loaded for documents' layouts (and the screenshots of them), each the
/// requesting document's alone ([`LayoutBodyKey`]). The only API: [`Self::get`],
/// [`Self::start`], [`Self::finish`], [`Self::fail`], [`Self::pending`].
#[derive(Default)]
pub struct SubresourceBodies {
    defaults: std::sync::Mutex<std::collections::HashMap<String, (String, &'static str)>>,
    policy_revision: std::sync::atomic::AtomicU64,
    next_request: std::sync::atomic::AtomicU64,
    slots: std::sync::Mutex<BodySlots>,
    arrivals: std::sync::atomic::AtomicU64,
    /// pyro: when each body arrived and when its document first needed it: one timeline for a
    /// document's layout and its screenshots ([`Self::late`], `font-display: optional`).
    came: std::sync::Mutex<std::collections::HashMap<LayoutBodyKey, std::time::Instant>>,
    needed: std::sync::Mutex<std::collections::HashMap<LayoutBodyKey, std::time::Instant>>,
}

fn locked<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SubresourceBodies {
    pub(crate) fn bind_raster_budget(&self, client: &str, page: pyro_raster::PageRasters) {
        locked(&self.slots).budgets.insert(client.to_owned(), page);
    }
    pub(crate) fn set_image_defaults(&self, document: &str, source: &str, policy: &'static str) {
        let mut defaults = locked(&self.defaults);
        if defaults.get(document).is_some_and(|(old, token)| old == source && *token == policy) { return; }
        if defaults.insert(document.to_owned(), (source.to_owned(), policy)).is_some() {
            self.retain_images(document, |_| false);
            self.policy_revision.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.arrived();
        }
    }

    pub fn policy_revision(&self) -> u64 { self.policy_revision.load(std::sync::atomic::Ordering::Relaxed) }

    pub fn effective_key(&self, mut key: LayoutBodyKey) -> LayoutBodyKey {
        if key.destination == SubresourceKind::Image {
            if let Some((source, policy)) = locked(&self.defaults).get(&key.document) {
                key.options.referrer.get_or_insert_with(|| source.clone());
                key.options.referrer_policy.get_or_insert_with(|| (*policy).to_owned());
            }
        }
        key
    }
    pub fn image_dispatched(&self, key: &LayoutBodyKey, source: Option<String>, policy: String) {
        let mut slots = locked(&self.slots);
        if let Some((key, mut slot)) = slots.images.shift_remove_entry(key) {
            slots.bytes -= slot.cost(&key);
            slot.dispatched = Some((source, policy));
            slots.insert_image(key, slot);
        }
    }

    pub fn dispatched_images(&self, document: &str, url: &str) -> Vec<(LayoutBodyKey, Option<String>, String)> {
        locked(&self.slots).images.iter().filter(|(key, _)| key.document == document && key.url == url)
            .filter_map(|(key, slot)| slot.dispatched.as_ref().map(|(source, policy)| (key.clone(), source.clone(), policy.clone())))
            .collect()
    }
    pub fn has_images(&self) -> bool {
        !locked(&self.slots).images.is_empty()
    }

    pub fn retain_images(&self, document: &str, keep: impl Fn(&LayoutBodyKey) -> bool) {
        let mut slots = locked(&self.slots);
        let BodySlots { bytes, images, .. } = &mut *slots;
        images.retain(|key, slot| {
            if key.document != document || keep(key) { return true; }
            *bytes -= slot.cost(key);
            false
        });
    }

    pub fn pending_images(&self, document: &str) -> Vec<LayoutBodyKey> {
        locked(&self.slots).images.iter().filter(|(key, slot)| key.document == document && matches!(slot.state, Slot::Pending(_))).map(|(key, _)| key.clone()).collect()
    }
    pub fn invalidate_image(&self, key: &LayoutBodyKey) {
        if key.destination == SubresourceKind::Image && locked(&self.slots).remove(key).is_some() {
            self.arrived();
        }
    }

    pub fn refresh_image(&self, key: &LayoutBodyKey, body: Option<LayoutBody>) -> bool {
        if key.destination != SubresourceKind::Image || key.options.integrity.is_some() {
            return false;
        }
        let mut slots = locked(&self.slots);
        let Some(slot) = slots.get(key) else { return false };
        match body {
            Some(body) => slots.insert(key.clone(), Slot::Ready(std::sync::Arc::new(body))),
            None if matches!(slot, Slot::Pending(_)) => return false,
            None => { slots.remove(key); }
        }
        drop(slots);
        self.arrived();
        true
    }

    /// The body of `key`, once loaded.
    pub fn get(&self, key: &LayoutBodyKey) -> Option<std::sync::Arc<LayoutBody>> {
        match locked(&self.slots).get(key) {
            Some(Slot::Ready(body)) => Some(body.clone()),
            _ => None,
        }
    }

    /// Whether a load of `key` is in flight.
    pub fn pending(&self, key: &LayoutBodyKey) -> bool {
        matches!(locked(&self.slots).get(key), Some(Slot::Pending(_)))
    }

    /// Whether `key` has a body, a load in flight or a failed load (its document fetched it).
    pub fn known(&self, key: &LayoutBodyKey) -> bool {
        locked(&self.slots).get(key).is_some()
    }

    /// Marks a load of `key` in flight: `false` when one is, or its body or failure is here,
    /// already.
    pub fn start(&self, key: &LayoutBodyKey) -> bool {
        self.start_request(key).is_some()
    }

    pub fn start_request(&self, key: &LayoutBodyKey) -> Option<u64> {
        let mut slots = locked(&self.slots);
        if slots.get(key).is_some() {
            return None;
        }
        let generation = self.next_request.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        slots.insert(key.clone(), Slot::Pending(generation));
        slots.get(key).map(|_| generation)
    }

    /// `key`'s response arrived and passed its checks: its body, final URL, tainting and status,
    /// written together. The document's layout lays out again.
    pub fn finish(&self, key: LayoutBodyKey, body: LayoutBody) {
        self.finish_request(key, body, None);
    }

    pub fn finish_request(&self, key: LayoutBodyKey, body: LayoutBody, generation: Option<u64>) {
        if body.bytes.is_empty() {
            return self.fail_request(&key, generation);
        }
        // pyro: only into the slot of a load in flight (`start`): a document forgotten since, or a
        // load nobody started, writes nothing.
        {
            let mut slots = locked(&self.slots);
            if !matches!(slots.get(&key), Some(Slot::Pending(id)) if generation.is_none_or(|expected| expected == *id)) {
                return;
            }
            slots.insert(key.clone(), Slot::Ready(std::sync::Arc::new(body)));
        }
        if key.destination == SubresourceKind::Font {
            locked(&self.came).entry(key).or_insert_with(std::time::Instant::now);
        }
        self.arrived();
    }

    /// `key`'s load failed: recorded as such (not asked again; a capture waits for it); a body
    /// stays.
    pub fn fail(&self, key: &LayoutBodyKey) {
        self.fail_request(key, None);
    }

    pub fn fail_request(&self, key: &LayoutBodyKey, generation: Option<u64>) {
        let mut slots = locked(&self.slots);
        if matches!(slots.get(key), Some(Slot::Pending(id)) if generation.is_none_or(|expected| expected == *id)) {
            slots.insert(key.clone(), Slot::Failed);
        }
    }

    /// `key`'s load got no answer (its document went first): no longer in flight, nothing kept.
    pub fn abandon(&self, key: &LayoutBodyKey) {
        self.abandon_request(key, None);
    }

    pub fn abandon_request(&self, key: &LayoutBodyKey, generation: Option<u64>) {
        let mut slots = locked(&self.slots);
        if matches!(slots.get(key), Some(Slot::Pending(id)) if generation.is_none_or(|expected| expected == *id)) {
            slots.remove(key);
        }
    }

    /// pyro: the state of the font request `key` of a document whose layout asked for the fonts
    /// `asked` ([`SubresourceLoader::asked_fonts`]): its slot's; with none, asked but not sent yet
    /// (still loading, never confused with a failure), else not asked at all.
    pub fn font_load(&self, key: &LayoutBodyKey, asked: Option<&[LayoutBodyKey]>) -> FontLoad {
        match locked(&self.slots).get(key) {
            Some(Slot::Pending(_)) => FontLoad::Loading,
            Some(Slot::Ready(_)) => FontLoad::Loaded,
            Some(Slot::Failed) => FontLoad::Failed,
            None if asked.is_some_and(|asked| asked.contains(key)) => FontLoad::Loading,
            None => FontLoad::NotAsked,
        }
    }

    /// pyro: whether `key`'s load failed ([`Self::fail`]).
    pub fn has_failed(&self, key: &LayoutBodyKey) -> bool {
        matches!(locked(&self.slots).get(key), Some(Slot::Failed))
    }

    /// pyro: whether any load in flight for `document` is one `matches` picks.
    pub fn any_pending(&self, document: &str, matches: impl Fn(&LayoutBodyKey) -> bool) -> bool {
        let slots = locked(&self.slots);
        slots.other.iter().chain(slots.images.iter().map(|(k, slot)| (k, &slot.state))).any(|(k, slot)| matches!(slot, Slot::Pending(_)) && k.document == document && matches(k))
    }

    /// pyro: drops what `document` loaded (it is gone: navigated away from, closed); a load still
    /// in flight for it is no longer waited for, and nothing of it serves another document.
    pub fn forget_document(&self, document: &str) {
        locked(&self.defaults).remove(document);
        let mut slots = locked(&self.slots);
        slots.budgets.remove(document);
        slots.other.retain(|k, _| k.document != document);
        slots.images.retain(|k, _| k.document != document);
        slots.bytes = slots.images.iter().map(|(key, slot)| slot.cost(key)).sum();
        locked(&self.came).retain(|k, _| k.document != document);
        locked(&self.needed).retain(|k, _| k.document != document);
    }

    /// A body arrived: the documents laid out without it lay out again.
    pub fn arrived(&self) {
        self.arrivals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many bodies arrived ([`Self::arrived`]).
    pub fn arrivals(&self) -> u64 {
        self.arrivals.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// pyro: `key`'s document needs its body (now, unless it did before).
    pub fn needed(&self, key: &LayoutBodyKey) {
        if key.destination == SubresourceKind::Font {
            locked(&self.needed).entry(key.clone()).or_insert_with(std::time::Instant::now);
        }
    }

    /// pyro: whether `key`'s body came more than `period` after its document first needed it
    /// (or has not come and that long has passed): a `font-display: optional` font it does not
    /// swap in. `None` when the document never asked for it.
    pub fn late(&self, key: &LayoutBodyKey, period: std::time::Duration) -> Option<bool> {
        let needed = *locked(&self.needed).get(key)?;
        let came = locked(&self.came).get(key).copied();
        Some(came.unwrap_or_else(std::time::Instant::now).saturating_duration_since(needed) > period)
    }
}

#[derive(Clone, Debug)]
pub struct TransitionBox {
    pub name: String,
    pub key: u64,
    pub size: (f64, f64),
    pub transform: [f64; 6],
    pub ink: [f64; 4],
    pub rgba: Option<std::sync::Arc<Vec<u8>>>,
}

#[derive(Clone, Debug)]
pub struct TransitionAllowance {
    pub pixels: u64,
    pub work: u64,
}

pub trait TransitionLease: std::fmt::Debug + Send + Sync {
    fn debit_failure(&self, spent: u64);
}

impl TransitionLease for () {
    fn debit_failure(&self, _: u64) {}
}

#[derive(Debug)]
pub struct TransitionJob {
    pub remaining: std::sync::Mutex<TransitionAllowance>,
    pub lease: std::sync::Arc<dyn TransitionLease>,
}

#[derive(Clone, Debug)]
pub struct TransitionFrame {
    pub document: String,
    pub root: u64,
    pub old: std::sync::Arc<[TransitionBox]>,
    pub new: std::sync::Arc<[TransitionBox]>,
    pub samples: std::sync::Arc<[(usize, usize, f64)]>,
    pub reservation: std::sync::Arc<dyn std::fmt::Debug + Send + Sync>,
    pub job: Option<std::sync::Arc<TransitionJob>>,
}

/// The engine laying the document out (Blitz in `render` builds).
pub trait GeometrySource {
    fn set_transition_samples(&self, _root: u64, _values: &[(usize,usize,f64)]) -> bool { true }
    fn transition_animation_styles(&self, _root: u64) -> Option<Vec<(usize,String, style::servo_arc::Arc<style::properties::ComputedValues>)>> { Some(Vec::new()) }
    fn set_transition_boxes(&self, _root: u64, _old: &[TransitionBox], _new: &[TransitionBox]) -> bool { true }
    fn clear_transition_boxes(&self, _root: u64) {}
    /// Whether the painted subtree can participate in a named view transition.
    fn view_transition_names_valid(&self, _root: u64) -> bool { true }
    fn capture_transition(&self, _root: u64, _old: bool, _pixels: u64, _work: &mut u64) -> Option<Vec<TransitionBox>> { Some(Vec::new()) }
    /// Brings the engine's layout up to the document; the other queries
    /// answer from it.
    fn sync(&self, dom: &Dom, layout: &Layout);
    /// Brings the engine's styles up to the document, leaving layout to
    /// the next [`GeometrySource::sync`] ([`GeometrySource::element_style`]
    /// answers from it).
    fn sync_styles(&self, _dom: &Dom, _style: &crate::style::StyleEngine) {}
    /// The page scrolled: its scrolled elements' offsets (by key) and the viewport's, which
    /// scroll-driven animations follow.
    fn set_scroll(&self, _elements: Vec<(u64, (f64, f64))>, _viewport: (f64, f64)) {}
    /// One element's box as of the last sync (`None`: not rendered).
    fn element_box(&self, key: u64) -> Option<ExternalBox>;
    fn anchor_scroll_shift(&self, _key: u64, _scroll: &dyn Fn(Option<u64>) -> (f64, f64), _sticky: &dyn Fn(u64) -> (f64, f64)) -> (f64, f64) { (0.0, 0.0) }
    /// The engine's resolved value of one element's `property` as of the last sync (`None`: the engine
    /// has none, and the document's own cascade answers). Asked for what only layout knows: a subgrid's
    /// `grid-template-columns`/`-rows` (`subgrid` with each line's names).
    fn resolved_style_value(&self, _key: u64, _property: &str) -> Option<String> {
        None
    }
    /// Resolves a transform using the element's layout box and a possibly
    /// sampled style, without modifying the renderer's stored style.
    fn resolved_transform_value(&self, _key: u64, _property: &str, _style: &style::properties::ComputedValues) -> Option<String> {
        None
    }
    /// Whether author CSS supplied either longhand of white-space on this element.
    fn has_author_white_space(&self, _key: u64) -> bool { false }
    /// Computed CSS geometry declarations on an SVG shape, excluding the
    /// default values that must leave XML presentation attributes in effect.
    fn svg_geometry_properties(&self, _key: u64) -> Vec<(&'static str, String)> {
        Vec::new()
    }
    fn sampled_calc_size(&self, _key: u64, _width: bool, _style: &style::properties::ComputedValues) -> Option<f32> {
        None
    }
    /// One element's boxes as quads as of the last sync (`None`: not
    /// rendered, or the engine does not report them).
    fn element_quads(&self, _key: u64) -> Option<ExternalQuads> {
        None
    }
    /// Hit regions (document x, y, width, height) when fragmentation restricts the element's
    /// bounds. `None` uses the whole box; an empty list admits no hits.
    fn element_fragments(&self, _key: u64) -> Option<Vec<[f64; 4]>> {
        None
    }
    /// Whether any box may be split into fragments (as of the last sync).
    fn has_fragmented_boxes(&self) -> bool {
        false
    }
    /// The whole document's boxes as of the last sync.
    fn geometry(&self) -> Option<ExternalGeometry>;
    /// Every word of every text node as of the last sync, in layout order
    /// (`None`: the engine reports none).
    fn text_words(&self) -> Option<Vec<(u64, ExternalWord)>> {
        None
    }
    /// [`GeometrySource::text_words`] for the text nodes below the element
    /// `key` (more words may come; the caller filters), as of the last sync,
    /// in the same order (`None`: the engine reads no part of the document
    /// alone, or `key` is not one of its nodes).
    fn subtree_words(&self, _key: u64) -> Option<Vec<(u64, ExternalWord)>> {
        None
    }
    /// The `position: sticky` elements as of the last sync, with their
    /// inputs: [`ExternalGeometry::sticky`] without the boxes.
    fn sticky_elements(&self) -> Vec<(u64, ExternalSticky)> {
        self.geometry().map(|g| g.sticky).unwrap_or_default()
    }
    /// The entry of [`Self::sticky_elements`] for one element, if it has one.
    fn sticky_element(&self, key: u64) -> Option<ExternalSticky> {
        self.sticky_elements().into_iter().find(|(k, _)| *k == key).map(|(_, s)| s)
    }
    /// The elements whose box may hold `probe` and their ancestors, in
    /// tree order, from a descent of the engine's tree that skips the
    /// subtrees that cannot hold it (`None`: the engine has no such
    /// descent, and the whole document's boxes are tested). `shifted`: the
    /// keys of the elements this document moves from where the engine laid
    /// them out (scrolled containers, sticky boxes); `offset`: how far any
    /// element's box is moved (document px added to the point), consulted
    /// when `shifted` is not empty. The candidates are a superset: the
    /// caller tests each box itself.
    fn hit_candidates(
        &self,
        _probe: &HitProbe,
        _offset: &dyn Fn(u64) -> (f64, f64),
        _shifted: &[u64],
    ) -> Option<Vec<HitVisit>> {
        None
    }
    /// The document's scroll size as of the last sync.
    fn document_size(&self) -> Option<(f64, f64)>;
    /// pyro: the space the viewport's shown classic scrollbars take (the vertical one's width,
    /// the horizontal one's height) as of the last sync; zero with overlay scrollbars.
    fn viewport_scrollbar_sizes(&self) -> (f64, f64) {
        (0.0, 0.0)
    }
    /// pyro: the viewport's classic scrollbar gutters (left, top, right, bottom; CSS px) as of the
    /// last sync, `None` when it has none: a point in them hits the root element.
    fn viewport_gutters(&self) -> Option<[f64; 4]> {
        None
    }
    /// Whether any element is `position: sticky` (as of the last sync).
    fn has_sticky(&self) -> bool;
    /// The document's URL (relative sub-resources resolve against it).
    fn set_base_url(&self, _url: &str) {}
    /// The document's network, for the sub-resources it is laid out with
    /// (see [`SubresourceLoader`]).
    fn set_subresource_loader(&self, _loader: std::sync::Arc<dyn SubresourceLoader>) {}
    /// The source itself (its owner reaches its engine's document).
    fn as_any(&self) -> &dyn std::any::Any;
    /// Whether the source answers now (else nothing is laid out).
    fn active(&self) -> bool {
        true
    }
    /// Whether a style rule of the engine's sheets depends on `:active`
    /// (as of the last sync); true when the engine cannot tell.
    fn styles_depend_on_active(&self) -> bool {
        true
    }
    /// Whether the engine must sync again although the document did not
    /// change (a sub-resource it laid out without arrived since).
    fn stale(&self) -> bool {
        false
    }
    /// pyro: whether a web font the engine asked its network for has not arrived yet
    /// (`document.fonts`: `status`, `ready`).
    fn fonts_pending(&self) -> bool {
        false
    }
    /// pyro: whether a CSS image (a background, mask, border, list or shape image) the engine's
    /// layout asked its network for has not arrived yet: the document's `load` waits for it, as
    /// Chrome's style resolution starts those fetches and they delay `load`.
    fn images_pending(&self) -> bool {
        false
    }
    /// pyro: what the engine's layout did with this document's request for the font at `url`
    /// (by its request key, never by URL alone), `None` when the engine loads no fonts.
    fn font_load(&self, _url: &str) -> Option<FontLoad> {
        None
    }
    /// One element's computed style as of the last sync (`None`: the engine
    /// has none for it, and the document's own cascade answers).
    fn element_style(&self, _key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        None
    }
    /// Authored timing values whose computation needs the element or its pseudo-element.
    fn element_timing_style(
        &self, _key: u64, _pseudo: Option<&style::selector_parser::PseudoElement>,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> { None }

    /// One element's starting style (`@starting-style`), as of the last sync (`None`: none applies).
    fn element_starting_style(&self, _key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        None
    }
    /// Whether the element is outside every `display: none` subtree of the
    /// rendered tree, from the styles the engine keeps as of the last style
    /// sync (`None`: the engine cannot tell, and the derived
    /// [`crate::style::ComputedStyle`] answers).
    fn element_displayed(&self, _key: u64) -> Option<bool> {
        None
    }
    /// pyro: the column boxes of the multi-column container `key` (border box coordinates: x, y,
    /// width, height); empty for any other element.
    fn column_boxes(&self, _key: u64) -> Vec<[f64; 4]> {
        Vec::new()
    }
    /// pyro: the keys of the scroll containers that snap mandatorily (CSS Scroll Snap 1 § 6.3:
    /// their initial scroll position snaps).
    fn mandatory_snap_containers(&self) -> Vec<u64> {
        Vec::new()
    }
    /// pyro: the target of the scroll marker at the document point (`x`, `y`): its element's key,
    /// or its multi-column container's and the column's index.
    fn scroll_marker_at(&self, _x: f64, _y: f64) -> Option<(u64, Option<u32>)> {
        None
    }
    /// pyro: whether an element was ever styled with a `scroll-snap-type`, as of the last sync
    /// (no snapping to look for otherwise).
    fn uses_scroll_snap(&self) -> bool {
        false
    }
    /// pyro: the keys of the elements whose `::scroll-marker-group` scrolled to keep its active
    /// marker in view since the last call.
    fn take_scroll_group_moves(&self) -> Vec<u64> {
        Vec::new()
    }
    /// The elements whose style was replaced since the last call, by key,
    /// with the style before and after (and 1 or 2 for a change of its `::before` or `::after`, 0
    /// of its own); recorded while the engine's
    /// [`crate::style::StyleEngine::track_style_changes`] is on.
    fn take_style_changes(
        &self,
    ) -> Vec<(u64, u8, style::servo_arc::Arc<style::properties::ComputedValues>, style::servo_arc::Arc<style::properties::ComputedValues>)>
    {
        Vec::new()
    }
    /// The computed style of one element's pseudo-element `pseudo`
    /// (`getComputedStyle(el, pseudo)`) as of the last sync; with
    /// `generated_only`, only a stored style of a pseudo-element its rules
    /// generate.
    /// A script scroll or view progress timeline's state as of the last sync
    /// (`pyro_render::Renderer::timeline_state`): scroll offset, start, end, scroll container key.
    /// `pseudo`: the subject's pseudo-element (2 `::before`, 3 `::after`; 0 the element).
    /// pyro: whether the viewport's horizontal scroll origin is on its right (the principal
    /// writing mode's), as the layout engine's document scrolls; `None`: not known.
    fn viewport_scrolls_leftward(&self) -> Option<bool> {
        None
    }
    /// pyro: renders the contents `content-visibility: auto` skips around the element keyed
    /// `key`, for a query that needs its layout (`scrollIntoView`; CSS Contain 2 § 4.1). Whether
    /// any were skipped (laid out again then).
    fn force_content_visibility(&self, _key: u64) -> bool {
        false
    }
    /// pyro: a rendering frame starts: `content-visibility: auto` relevance is determined anew
    /// (what queries forced to render since the last frame no longer is).
    fn content_visibility_frame(&self) {}
    /// pyro: the `content-visibility: auto` elements as of the last sync, by key: whether they
    /// skip their contents, and their last remembered size (for the renderer that paints).
    fn content_visibility_state(&self) -> Vec<(u64, bool, Option<(f32, f32)>)> {
        Vec::new()
    }
    fn timeline_state(
        &self,
        _source: Option<u64>,
        _subject: Option<u64>,
        _pseudo: u8,
        _axis: u8,
        _inset: [(u8, f64); 2],
    ) -> Option<(f64, f64, f64, Option<u64>, (f64, f64), f64)> {
        None
    }
    fn element_pseudo_style(
        &self,
        _key: u64,
        _pseudo: &style::selector_parser::PseudoElement,
        _generated_only: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        None
    }
    /// `false` when [`Self::element_pseudo_style`] with `generated_only`
    /// answers `None` for `::before` / `::after` of every element as of the
    /// last sync (no element is styled with a generated one).
    fn may_have_generated_before_after(&self) -> bool {
        true
    }
}

/// An element's boxes (document coordinates; viewport ones inside a fixed
/// box) and its place in tree order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutBox {
    pub rect: Rect,
    pub fixed: bool,
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// See [`ExternalBox::scroll_reversed`].
    pub scroll_reversed: [bool; 2],
    pub border: [f64; 4],
    pub padding: [f64; 4],
    pub position: BoxPosition,
    /// Tree order (1-based), for paint order among equals.
    pub order: u32,
    /// It or an ancestor is positioned: painted in the positioned layer.
    pub in_positioned: bool,
    /// See [`ExternalBox::hidden`].
    pub hidden: bool,
    /// See [`ExternalBox::layout_origin`].
    pub layout_origin: Option<[f64; 2]>,
    /// See [`ExternalBox::items_in_phases`].
    pub items_in_phases: bool,
}

impl LayoutBox {
    pub fn from_external(b: &ExternalBox, order: u32) -> LayoutBox {
        LayoutBox {
            rect: Rect { x: b.x, y: b.y, width: b.width, height: b.height },
            fixed: b.fixed,
            client_width: b.client_width,
            client_height: b.client_height,
            scroll_width: b.scroll_width,
            scroll_height: b.scroll_height,
            scroll_reversed: b.scroll_reversed,
            border: b.border,
            padding: b.padding,
            position: b.position,
            order,
            in_positioned: b.position != BoxPosition::Static,
            hidden: b.hidden,
            layout_origin: b.layout_origin,
            items_in_phases: b.items_in_phases,
        }
    }

    /// Positioned (relative/absolute/fixed/sticky), or inside such a box:
    /// painted above in-flow boxes.
    pub fn positioned(&self) -> bool {
        self.in_positioned
    }
}

/// One word of a text node on a line.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub node: NodeId,
    /// UTF-16 offsets of the word in the node's data.
    pub start: u32,
    pub end: u32,
    pub rect: Rect,
    pub fixed: bool,
    pub vertical_edges: bool,
    /// Caret positions (offset, physical inline coordinate) from `start` to `end`.
    edges: Rc<[(u32, f64)]>,
}

impl TextRun {
    /// UTF-16 offsets paired with document x (horizontal) or y (vertical).
    pub fn edges(&self) -> &[(u32, f64)] {
        &self.edges
    }

    pub fn offset_at(&self, distance: f64) -> u32 {
        let coord = distance + if self.vertical_edges { self.rect.y } else { self.rect.x };
        self.edges.iter().min_by(|a, b| (a.1 - coord).abs().total_cmp(&(b.1 - coord).abs())).map_or(self.start, |e| e.0)
    }
}

/// The whole document's boxes (or, for one hit test, the boxes of the
/// elements it may hit and of their ancestors: see
/// [`crate::layout::Layout::element_from_point`]).
#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    pub boxes: FxHashMap<NodeId, LayoutBox>,
    /// Document scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
    pub sticky: FxHashMap<NodeId, ExternalSticky>,
    /// Hit testing's derived data, filled per box on first use: it depends
    /// only on the tree, the styles and the boxes, which this result is
    /// valid for (a new result is read after any of them change).
    pub(crate) hit: RefCell<HitTestData>,
}

/// The whole document's words, for text ranges, carets and selection.
#[derive(Clone, Debug, Default)]
pub struct TextResult {
    /// Text nodes: the union of their words, and whether inside a fixed box.
    pub texts: FxHashMap<NodeId, (Rect, bool)>,
    /// Every word, in layout order.
    pub text_runs: Vec<TextRun>,
}

impl TextResult {
    pub fn from_words(dom: &Dom, words: Vec<(u64, ExternalWord)>) -> TextResult {
        let mut out = TextResult::default();
        let mut seen = FxHashSet::default();
        for (key, w) in words {
            let Some(node) = node_of(dom, key) else { continue };
            let (Some(first), Some(last)) = (w.edges.first(), w.edges.last()) else { continue };
            let rect = Rect { x: w.x, y: w.y, width: w.width, height: w.height };
            let (start, end) = (first.0.min(last.0), first.0.max(last.0));
            let union = out.texts.entry(node).or_insert((rect, w.fixed));
            if seen.insert(node) {
                *union = (rect, w.fixed);
            } else {
                let r = union.0;
                let (x0, y0) = (r.x.min(rect.x), r.y.min(rect.y));
                let (x1, y1) = ((r.x + r.width).max(rect.x + rect.width), (r.y + r.height).max(rect.y + rect.height));
                union.0 = Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
            }
            out.text_runs.push(TextRun { node, start, end, rect, fixed: w.fixed, vertical_edges: w.vertical_edges, edges: w.edges.into() });
        }
        out
    }
}

/// One stacking level of a paint key: (layer, z-index, box order).
pub(crate) type PaintEntry = (u8, i32, u32);

/// The paint-order state after an element's ancestor chain (itself
/// included), from which its own and its descendants' paint keys follow.
#[derive(Clone, Debug, Default)]
pub(crate) struct PaintChain {
    /// The stacking contexts entered, outermost first.
    pub(crate) contexts: Rc<[PaintEntry]>,
    /// The z-auto positioned box whose layer the content belongs to.
    pub(crate) positioned_layer: Option<PaintEntry>,
    /// The last box of the chain makes its own layer.
    pub(crate) own_layer: bool,
    /// pyro: the normal-flow phase the chain's in-flow boxes paint in within their layer: 0 with
    /// the block backgrounds, 1 with the inline content (the items of a container whose items
    /// paint so, the engine's `items_in_phases`, and their in-flow descendants).
    pub(crate) flow_phase: i32,
}

/// Per-box hit-testing data of one [`LayoutResult`].
#[derive(Clone, Debug, Default)]
pub(crate) struct HitTestData {
    pub(crate) chains: FxHashMap<NodeId, PaintChain>,
    pub(crate) keys: FxHashMap<NodeId, Rc<[PaintEntry]>>,
    /// The overflow clips that apply to an element: (ancestor, its clipped
    /// axes), nearest first.
    pub(crate) clips: FxHashMap<NodeId, Rc<[(NodeId, crate::style::ScrollAxes)]>>,
}

/// The node `key` names, when it is still in `dom`.
pub fn node_of(dom: &Dom, key: u64) -> Option<NodeId> {
    let id = NodeId { idx: key as u32, gen: (key >> 32) as u32 };
    dom.check(id).ok().map(|_| id)
}

impl LayoutResult {
    pub fn get(&self, el: NodeId) -> Option<&LayoutBox> {
        self.boxes.get(&el)
    }

    pub fn from_external(dom: &Dom, g: ExternalGeometry) -> LayoutResult {
        let mut out = LayoutResult {
            document_width: g.document_width,
            document_height: g.document_height,
            ..LayoutResult::default()
        };
        // Tree order: an element's ancestors are in before it.
        for (i, (key, b)) in g.boxes.iter().enumerate() {
            if let Some(el) = node_of(dom, *key) {
                out.insert_box(dom, el, LayoutBox::from_external(b, i as u32 + 1));
            }
        }
        for (key, s) in g.sticky {
            if let Some(el) = node_of(dom, key) {
                out.sticky.insert(el, s);
            }
        }
        out
    }

    /// Adds `el`'s box, inheriting the positioned layer from the nearest
    /// ancestor with a box (the ancestors are added before their
    /// descendants).
    pub(crate) fn insert_box(&mut self, dom: &Dom, el: NodeId, mut b: LayoutBox) {
        let mut cur = dom.parent_element(el).ok().flatten();
        while let Some(p) = cur {
            if let Some(pb) = self.boxes.get(&p) {
                b.in_positioned |= pb.in_positioned;
                break;
            }
            cur = dom.parent_element(p).ok().flatten();
        }
        self.boxes.insert(el, b);
    }
}

#[cfg(test)]
mod font_load_tests {
    use super::*;

    #[test]
    fn image_body_stays_charged_after_document_eviction() {
        let bodies = SubresourceBodies::default();
        let page = pyro_raster::PageRasters::default();
        bodies.bind_raster_budget("doc", page.clone());
        let key = LayoutBodyKey::new("doc", "http://site.test/image", &Default::default(), SubresourceKind::Image);
        assert!(bodies.start(&key));
        bodies.finish(key.clone(), LayoutBody::local(key.url(), vec![0; 4096]));
        let retained = bodies.get(&key).unwrap();
        let charge = page.used();
        assert!(charge >= 4096);
        bodies.forget_document("doc");
        assert_eq!(page.used(), charge);
        drop(retained);
        assert_eq!(page.used(), 0);
    }

    #[test]
    fn layout_image_churn_bounds_encoded_bodies_and_metadata() {
        let bodies = SubresourceBodies::default();
        for index in 0..10_000 {
            let url = format!("https://example.test/{index}");
            let key = LayoutBodyKey::new("doc", &url, &Default::default(), SubresourceKind::Image);
            bodies.needed(&key);
            assert!(bodies.start(&key));
            bodies.image_dispatched(&key, Some(url.clone()), "origin".into());
            bodies.finish(key, LayoutBody::local(&url, vec![0; 2048]));
        }
        let slots = locked(&bodies.slots);
        let bytes: usize = slots.images.values().map(|slot| match &slot.state {
            Slot::Ready(body) => body.bytes.capacity(), _ => 0,
        }).sum();
        assert!(slots.images.len() <= 256 && bytes <= 16 << 20, "{} entries, {bytes} bytes", slots.images.len());
        assert_eq!(slots.bytes, slots.images.iter().map(|(key, slot)| slot.cost(key)).sum::<usize>());
        drop(slots);
        bodies.retain_images("doc", |_| false);
        assert_eq!(locked(&bodies.slots).bytes, 0);
        assert!(!bodies.has_images());
        assert!(locked(&bodies.came).is_empty() && locked(&bodies.needed).is_empty());
    }

    #[test]
    fn one_documents_images_do_not_evict_anothers() {
        let bodies = SubresourceBodies::default();
        let kept = LayoutBodyKey::new("page", "https://example.test/kept", &Default::default(), SubresourceKind::Image);
        assert!(bodies.start(&kept));
        bodies.finish(kept.clone(), LayoutBody::local("https://example.test/kept", vec![0; 2048]));
        for index in 0..1_000 {
            let url = format!("https://example.test/{index}");
            let key = LayoutBodyKey::new("other", &url, &Default::default(), SubresourceKind::Image);
            assert!(bodies.start(&key));
            bodies.finish(key, LayoutBody::local(&url, vec![0; 64 << 10]));
        }
        assert!(bodies.get(&kept).is_some(), "another document's images evicted this one's");
        let slots = locked(&bodies.slots);
        let other = slots.images.keys().filter(|key| key.document == "other").count();
        assert!(other <= BodySlots::IMAGE_ENTRIES, "{other}");
        assert_eq!(slots.bytes, slots.images.iter().map(|(key, slot)| slot.cost(key)).sum::<usize>());
    }

    #[test]
    fn image_refresh_only_updates_requested_slots() {
        let bodies = SubresourceBodies::default();
        let key = LayoutBodyKey::new("doc", "http://site.test/image", &Default::default(), SubresourceKind::Image);
        let body = |value| LayoutBody::local(key.url(), vec![value]);
        assert!(!bodies.refresh_image(&key, Some(body(1))));
        let older = bodies.start_request(&key).unwrap();
        assert!(bodies.refresh_image(&key, Some(body(2))));
        bodies.finish(key.clone(), body(1));
        bodies.abandon(&key);
        assert_eq!(bodies.get(&key).unwrap().bytes.as_slice(), [2]);
        assert!(bodies.refresh_image(&key, Some(body(3))));
        assert_eq!(bodies.get(&key).unwrap().bytes.as_slice(), [3]);
        assert!(bodies.refresh_image(&key, None));
        let current = bodies.start_request(&key).unwrap();
        bodies.finish_request(key.clone(), body(1), Some(older));
        bodies.abandon_request(&key, Some(older));
        assert!(bodies.pending(&key));
        bodies.fail_request(&key, Some(current));
        assert!(bodies.refresh_image(&key, Some(body(4))));
        assert_eq!(bodies.get(&key).unwrap().bytes.as_slice(), [4]);
    }

    /// A font request its layout asked for is loading until answered, whether or not it went out
    /// yet; a failed one is failed, never "not asked"; the state is the request's (document,
    /// URL, options), never another request's of the same URL (security review cycle 5, 6).
    #[test]
    fn a_font_request_is_loading_from_the_ask_and_known_by_its_key() {
        let bodies = SubresourceBodies::default();
        let key = |doc: &str, url: &str| LayoutBodyKey::new(doc, url, &SubresourceOptions::default(), SubresourceKind::Font);
        let face = key("d1", "http://t/f.woff2");
        assert_eq!(bodies.font_load(&face, Some(&[])), FontLoad::NotAsked);
        // Asked, request not out yet.
        assert_eq!(bodies.font_load(&face, Some(&[face.clone()])), FontLoad::Loading);
        assert!(bodies.start(&face));
        assert_eq!(bodies.font_load(&face, Some(&[face.clone()])), FontLoad::Loading);
        // Another document's, or a CORS request of the same URL, is not this one.
        let cors = LayoutBodyKey::new(
            "d1",
            "http://t/f.woff2",
            &SubresourceOptions { crossorigin: Some("anonymous".into()), ..Default::default() },
            SubresourceKind::Font,
        );
        assert_eq!(bodies.font_load(&cors, Some(&[])), FontLoad::NotAsked);
        assert_eq!(bodies.font_load(&key("d2", "http://t/f.woff2"), Some(&[])), FontLoad::NotAsked);
        bodies.fail(&face);
        assert_eq!(bodies.font_load(&face, Some(&[face.clone()])), FontLoad::Failed);
    }
}
