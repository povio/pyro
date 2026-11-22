//! The `Dom` store: one arena per page holding every document (main, parsed,
//! template-inert, DOMParser) so nodes can be adopted across documents without
//! changing identity. Unlike upstream (which never frees nodes before page
//! teardown, see browser/src/RecyclingAllocator.zig:19-25), detached trees are
//! reclaimed by `collect_garbage` once no external reference remains.

use std::collections::VecDeque;

use markup5ever::{local_name, ns, LocalName, Namespace, Prefix, QualName};
use rustc_hash::FxHashSet;

use crate::error::{hierarchy, not_found, DomError, DomErrorKind, DomResult};
use crate::mutation::MutationState;
use crate::node::*;

struct Slot {
    gen: u32,
    node: Option<Node>,
}

/// What a tree walk needs of a slot, 16 bytes, in an array beside the slots
/// (`Dom::walk`): the links a pre-order walk follows (copies of the node's,
/// written by `link` and `unlink` with them) and the node's [`summary`]
/// (written by every change of what it digests). Four fit a cache line
/// where a slot (72 bytes) needs more than one, so a selector query walks
/// a tree reading this array, and the node and its boxed `ElementData`
/// only for the elements the summary does not rule out.
#[derive(Clone, Copy)]
pub(crate) struct Walk {
    pub(crate) first_child: u32,
    pub(crate) next: u32,
    pub(crate) parent: u32,
    pub(crate) sum: u32,
}

/// [`summary`]'s bit for an element (0 for any other node).
pub(crate) const SUM_ELEMENT: u32 = 1 << 15;
/// [`summary`]'s bits for the element's name.
pub(crate) const SUM_NAME: u32 = SUM_ELEMENT | (SUM_ELEMENT - 1);

/// A 32-bit digest of a node for selector queries (`crate::select_query`):
/// 0 for a node that is no element; for an element, bit 15 is set, bits
/// 0-14 are a hash of its local name and bits 16-31 a one-bit-per-token
/// Bloom filter of its classes. Equal names have equal hashes, and a class
/// the element has sets its bit, so a mismatch proves a type or class test
/// fails; a match proves nothing.
#[inline]
pub(crate) fn summary(kind: &NodeKind) -> u32 {
    match kind {
        NodeKind::Element(e) => {
            let mut bloom = 0u32;
            for c in e.classes() {
                bloom |= class_bit(c);
            }
            name_hash(&e.name.local) | bloom
        }
        _ => 0,
    }
}

/// [`summary`]'s name bits (and element bit) for an element named `local`.
#[inline]
pub(crate) fn name_hash(local: &markup5ever::LocalName) -> u32 {
    SUM_ELEMENT | (mix(local.get_hash()) >> 49) as u32
}

/// The bit of [`summary`]'s class filter that class `class` sets.
#[inline]
pub(crate) fn class_bit(class: &Atom) -> u32 {
    1 << (16 + (mix(class.get_hash()) >> 60) as u32)
}

#[inline]
fn mix(h: u64) -> u64 {
    (h ^ (h >> 29)).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// Style invalidation's view of attribute writes (see `note_style_attr`).
#[derive(Default)]
pub(crate) struct StyleSnapshots {
    /// Attribute local names some selector tests (besides `class` and `id`).
    names: rustc_hash::FxHashSet<markup5ever::LocalName>,
    /// Writes since the last read, in order: element index and generation,
    /// attribute, old value. An append (no lookups) keeps the write path as
    /// cheap as without a cascade; the first write per attribute wins when
    /// the cascade reads them.
    log: Vec<(u32, u32, QualName, Option<Box<str>>)>,
    /// The log outgrew `STYLE_LOG_CAP`: the cascade restyles everything.
    overflowed: bool,
}

/// Attribute writes kept between two style reads at most.
const STYLE_LOG_CAP: usize = 1 << 16;
const REMOVAL_LOG_LEN: usize = 1 << 12;

/// One journaled DOM mutation (node indices in this DOM).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// An attribute of the element was set, changed or removed; `.1` is the
    /// hash of its local name (`attr_name_hash`), 0 for no attribute (state
    /// only selectors cannot see, e.g. an image's natural size). `.2`: a
    /// `data-*`/`aria-*` attribute (no namespace), which only selectors that
    /// name it can observe (style and layout never read them).
    Attr(u32, u64, bool),
    /// Node `.1` was inserted into `.0` (one entry per inserted node).
    Inserted(u32, u32),
    /// `.1` was removed from parent `.0`, before sibling `.2` (NIL: it was
    /// the last child). The removed subtree keeps cached state, and may be
    /// queried detached or reinserted.
    Removed(u32, u32, u32),
    /// The character data of the node changed.
    Text(u32),
    /// Form-control state of the element changed (:checked, :valid...).
    State(u32),
    /// The focused or hovered element changed. Consumers compare the focused
    /// and hovered elements themselves (this entry restyles nothing).
    Pseudo,
    /// Anything else (e.g. a shadow root attached), with where it was made
    /// (diagnostics: `PYRO_FLOW_STATS`).
    Unknown(&'static std::panic::Location<'static>),
}

/// What the members of a live collection depend on
/// ([`Dom::collection_unchanged_since`]).
#[derive(Clone, Debug, Default)]
pub struct CollectionWatch {
    /// Members are the root's children (else its descendants).
    pub children_only: bool,
    /// Only element nodes are members (text nodes coming and going do not
    /// matter).
    pub elements_only: bool,
    /// `attr_name_hash` of the attributes membership depends on.
    pub attrs: Vec<u64>,
    /// Form-control state matters (selectedness).
    pub state: bool,
}

/// Most changes the journal holds (32 bytes each: at most 1 MiB per DOM).
/// On overflow it keeps the newer half, so a consumer that reads it after
/// every batch stays covered for batches of up to half of this (a React
/// commit of a 50-row list journals some 13k changes).
pub const JOURNAL_CAPACITY: usize = 32768;

/// The hash `Change::Attr` records for an attribute local name.
pub fn attr_name_hash(local: &str) -> u64 {
    markup5ever::LocalName::from(local).get_hash()
}

impl Change {
    /// An attribute change of element `i`.
    pub(crate) fn attr(i: u32, name: &QualName) -> Change {
        let plain = name.ns.is_empty() && (name.local.starts_with("data-") || name.local.starts_with("aria-"));
        Change::Attr(i, name.local.get_hash(), plain)
    }
}

/// Shared `tagName`/`nodeName` strings (see `Dom::tag_name_shared`).
struct NameCache {
    tags: rustc_hash::FxHashMap<(Option<Prefix>, LocalName, bool), std::sync::Arc<str>>,
    /// `#text`, `#cdata-section`, `#comment`, `#document`, `#document-fragment`.
    fixed: [std::sync::Arc<str>; 5],
}

impl Default for NameCache {
    fn default() -> Self {
        NameCache {
            tags: Default::default(),
            fixed: [
                "#text".into(),
                "#cdata-section".into(),
                "#comment".into(),
                "#document".into(),
                "#document-fragment".into(),
            ],
        }
    }
}

pub struct Dom {
    /// Process-unique id (caches keyed by DOM identity and `version`).
    uid: u64,
    slots: Vec<Slot>,
    /// One [`Walk`] per slot, kept in step with the node in it.
    walk: Vec<Walk>,
    free: Vec<u32>,
    /// The generation a new slot starts at: above that of every slot the
    /// store gave back (`release_free_tail`), so a `NodeId` of a node that
    /// lived at the index before never names the node made there after.
    gen_floor: u32,
    /// The most nodes alive at once since a document last ended
    /// (`release_free_tail`).
    live_peak: u32,
    /// Bumped on every tree, attribute or character-data mutation. Live
    /// collections compare against it to reuse cached results.
    version: u64,
    /// See `tree_version()`.
    tree_version: u64,
    /// See `style_text_revision()`.
    style_text_revision: u64,
    /// Bumped whenever one of the queues the embedder's post-callback hook
    /// drains gets an entry: a queued observer, iframe event, custom element
    /// reaction, upgrade candidate or slot change. A mutation alone does not
    /// bump it (what follows every mutation compares `version`), so the hook
    /// compares it against the value it last found nothing at, instead of
    /// checking every queue after every native callback.
    hook_epoch: u64,
    /// Told each name a Document may start to answer as a named property
    /// (`Dom::set_named_item_hook`).
    pub(crate) named_item_hook: Option<std::rc::Rc<dyn Fn(&str)>>,
    /// Qualified names as the `tagName`/`nodeName` strings script sees,
    /// shared so a realm can keep one JavaScript string per name.
    names: std::cell::RefCell<NameCache>,
    /// Roots of trees that may have become unreachable.
    candidates: Vec<u32>,
    /// A `<base>` element was created; until then no document walks for one.
    pub(crate) base_seen: bool,
    /// Bumped whenever a document's viewport `<meta>` changes
    /// (`Dom::viewport_meta_version`).
    pub(crate) viewport_meta_version: u64,
    /// An `<audio>` or `<video>` element was created; until then no
    /// mutation looks for media load triggers (`media_element`).
    pub(crate) media_seen: bool,
    /// A `<canvas>` element was created: loaded images keep their bytes
    /// for canvas drawing (`Dom::canvas_seen`).
    pub(crate) canvas_seen: bool,
    /// An SVG `<animate>` element was created: until then the page has no
    /// SMIL animations to look for (`Dom::smil_seen`).
    pub(crate) smil_seen: bool,
    /// A `style` attribute that mentions `transition` was set or parsed: inline transitions to
    /// track (`Dom::inline_transitions_seen`).
    pub(crate) inline_transitions_seen: bool,
    pub(crate) media: crate::media_element::MediaQueue,
    live: u32,
    /// Bumped whenever a node is removed from its parent.
    removals: u64,
    /// The latest removals, oldest first: the removed subtree's root, or NIL for a deactivated document.
    removal_log: VecDeque<u32>,
    /// What each `version` bump changed, for caches that can update
    /// selectively (style invalidation). Bounded: see `changes_since`.
    journal: Vec<(u64, Change)>,
    /// The content height of the responsively embedded document each iframe shows (by slot), for the
    /// renderer (`set_frame_content_height`, snapshot `data-pyro-frame-content-height`).
    pub(crate) frame_content_heights: rustc_hash::FxHashMap<u32, f32>,
    /// pyro: the user agent's pan and zoom of an `<svg>` (`SVGSVGElement.currentTranslate` and
    /// `currentScale`): its translation and scale, when not the identity.
    pub(crate) svg_points: rustc_hash::FxHashMap<u32, std::rc::Weak<crate::svg_point_list::PointLists>>,
    pub(crate) svg_numbers: rustc_hash::FxHashMap<u32, rustc_hash::FxHashMap<&'static str, std::rc::Weak<crate::svg_point_list::NumberLists>>>,
    pub(crate) svg_strings: rustc_hash::FxHashMap<(u32, crate::svg_string_list::StringListAttribute), Vec<String>>,
    pub(crate) svg_pans: rustc_hash::FxHashMap<u32, (f64, f64, f64)>,
    /// The document each iframe shows as the renderer paints it (by slot): its render HTML, URL and
    /// scroll position (`set_frame_documents`, snapshot `srcdoc` + `data-pyro-frame-*`).
    pub(crate) frame_documents: rustc_hash::FxHashMap<u32, FrameDocument>,
    /// pyro: each sheet link's response `Referrer-Policy` (`Dom::set_sheet_policy`), by its slot.
    pub(crate) sheet_policies: rustc_hash::FxHashMap<u32, String>,
    /// pyro: the sheet links whose response's tainting is opaque (`Dom::set_sheet_policy`).
    pub(crate) sheet_tainted: rustc_hash::FxHashSet<u32>,
    /// pyro: each document's snapshot nonce (`Dom::snapshot_nonce`), by its slot.
    snapshot_nonces: std::cell::RefCell<rustc_hash::FxHashMap<u32, String>>,
    /// pyro: each frame document's animations now, by its client (`FrameDocument::client`): the
    /// renderer samples them into the frame's document (`data-pyro-key` elements).
    pub frame_animations: rustc_hash::FxHashMap<String, std::sync::Arc<crate::animation::AnimationFrame>>,
    /// Versions at or below this have no complete journal.
    journal_floor: u64,
    /// Old values of attributes selectors read, from their first change
    /// since the style cascade last read styles (Stylo's invalidation
    /// snapshots). `None` until a cascade asks for them, so a page that
    /// never reads styles pays nothing.
    style_snapshots: std::cell::RefCell<Option<StyleSnapshots>>,
    /// Custom element names whose definitions disable shadow roots.
    shadow_disabled: Vec<(Option<NodeId>, RegistryAssociation, LocalName, LocalName)>,
    scoped_registry_docs: rustc_hash::FxHashMap<u64, Vec<(u64, NodeId)>>,
    registry_associations: rustc_hash::FxHashMap<u32, RegistryAssociation>,
    custom_definition_owners: rustc_hash::FxHashMap<u32, CustomDefinitionOwner>,
    /// Shadow roots ever attached (selective style invalidation does not
    /// model slot assignment, so consumers fall back to full invalidation).
    shadow_roots: u32,
    /// Elements created by the parser or cloned with `on*` attributes whose
    /// handlers the bindings have not compiled yet.
    pending_inline_handlers: Vec<NodeId>,
    pub(crate) mutation: MutationState,
    /// Ranges to adjust on mutation (`crate::live_range`); empty when none exists.
    pub(crate) live_ranges: Vec<std::rc::Weak<crate::live_range::LiveRange>>,
    pub(crate) live_iterators: Vec<std::rc::Weak<crate::live_iterator::LiveIterator>>,
    /// The document whose window a node was created for, by node slot: for
    /// the nodes that moved to a document of another window, and for the
    /// documents without a browsing context (see `creation_document`).
    creation_docs: rustc_hash::FxHashMap<u32, NodeId>,
    /// Elements to try to upgrade to custom elements (see `crate::upgrade`).
    pub(crate) upgrades: crate::upgrade::UpgradeQueue,
    pub(crate) slot_state: crate::slots::SlotState,
    pub(crate) events: crate::events::ListenerStore,
    pub(crate) resources: crate::script::ResourceQueue,
    pub(crate) iframes: crate::iframe::IframeQueue,
    /// Custom element callback reactions awaiting the bindings (`custom`).
    pub(crate) reactions: crate::custom::ReactionQueue,
    /// `ElementInternals` state (`internals`).
    pub(crate) internals: crate::internals::InternalsStore,
    /// Modal dialogs and showing popovers (`top_layer`).
    pub(crate) top_layer: crate::top_layer::TopLayer,
    /// Queued `<details>` toggles and whether accordions are in use.
    pub(crate) details: crate::details::DetailsState,
    /// Nonces hidden from content attributes (`nonce`).
    pub(crate) nonces: crate::nonce::NonceState,
    /// Natural sizes of loaded `<img>` elements, set by the embedder.
    natural_sizes: rustc_hash::FxHashMap<NodeId, (f64, f64)>,
    /// `naturalWidth`/`naturalHeight` where they differ from what layout uses: an SVG image's
    /// natural dimensions completed against the default object size (CSS Images 3).
    reported_natural_sizes: rustc_hash::FxHashMap<NodeId, (f64, f64)>,
    /// The source an `<img>` selected from its `srcset` or its `<picture>`'s `<source>`s (the URL
    /// as written), set by the image loader: what a render snapshot gives as the image's `src`.
    image_sources: rustc_hash::FxHashMap<NodeId, String>,
    /// Named access on the Window, per document (see `window_names`).
    pub(crate) window_names: std::cell::RefCell<rustc_hash::FxHashMap<u32, crate::window_names::WindowNames>>,
    /// Attr nodes of an element's attributes, by element slot (see `attr_node`).
    pub(crate) attr_nodes: rustc_hash::FxHashMap<u32, Vec<u32>>,
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of `compareDocumentPosition`.
pub mod position {
    pub const DISCONNECTED: u16 = 0x01;
    pub const PRECEDING: u16 = 0x02;
    pub const FOLLOWING: u16 = 0x04;
    pub const CONTAINS: u16 = 0x08;
    pub const CONTAINED_BY: u16 = 0x10;
    pub const IMPLEMENTATION_SPECIFIC: u16 = 0x20;
}

impl Dom {
    pub fn new() -> Self {
        static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Dom {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            slots: Vec::new(),
            walk: Vec::new(),
            free: Vec::new(),
            version: 0,
            tree_version: 0,
            style_text_revision: 0,
            hook_epoch: 0,
            named_item_hook: None,
            names: std::cell::RefCell::new(NameCache::default()),
            candidates: Vec::new(),
            gen_floor: 1,
            live_peak: 0,
            live: 0,
            removals: 0,
            removal_log: VecDeque::new(),
            journal: Vec::new(),
            frame_content_heights: Default::default(),
            svg_points: Default::default(),
            svg_numbers: Default::default(),
            svg_strings: Default::default(),
            svg_pans: Default::default(),
            frame_documents: Default::default(),
            snapshot_nonces: Default::default(),
            sheet_policies: Default::default(),
            sheet_tainted: Default::default(),
            frame_animations: Default::default(),
            journal_floor: 0,
            style_snapshots: Default::default(),
            shadow_disabled: Vec::new(),
            scoped_registry_docs: Default::default(),
            registry_associations: Default::default(),
            custom_definition_owners: Default::default(),
            shadow_roots: 0,
            pending_inline_handlers: Vec::new(),
            base_seen: false,
            viewport_meta_version: 0,
            media_seen: false,
            canvas_seen: false,
            smil_seen: false,
            inline_transitions_seen: false,
            media: Default::default(),
            mutation: MutationState::default(),
            live_ranges: Vec::new(),
            live_iterators: Vec::new(),
            creation_docs: Default::default(),
            upgrades: Default::default(),
            slot_state: Default::default(),
            events: Default::default(),
            resources: Default::default(),
            iframes: Default::default(),
            reactions: Default::default(),
            internals: Default::default(),
            top_layer: Default::default(),
            details: Default::default(),
            nonces: Default::default(),
            natural_sizes: Default::default(),
            reported_natural_sizes: Default::default(),
            image_sources: Default::default(),
            window_names: Default::default(),
            attr_nodes: Default::default(),
        }
    }

    // ----- slot access -------------------------------------------------

    #[inline]
    pub(crate) fn n(&self, idx: u32) -> &Node {
        self.slots[idx as usize].node.as_ref().expect("live slot")
    }
    #[inline]
    pub(crate) fn n_mut(&mut self, idx: u32) -> &mut Node {
        self.slots[idx as usize].node.as_mut().expect("live slot")
    }
    /// Node `idx`'s walk links and [`summary`].
    #[inline]
    pub(crate) fn w(&self, idx: u32) -> Walk {
        let w = self.walk[idx as usize];
        #[cfg(debug_assertions)]
        {
            let n = self.n(idx);
            debug_assert_eq!(
                (w.first_child, w.next, w.parent, w.sum),
                (n.first_child, n.next, n.parent, summary(&n.kind)),
                "walk entry out of step"
            );
        }
        w
    }
    /// Recomputes element `idx`'s [`summary`] after its classes changed.
    pub(crate) fn resummarize(&mut self, idx: u32) {
        self.walk[idx as usize].sum = summary(&self.n(idx).kind);
    }
    #[inline]
    pub(crate) fn id_of(&self, idx: u32) -> NodeId {
        NodeId { idx, gen: self.generation_at(idx) }
    }
    #[inline]
    pub(crate) fn opt_id(&self, idx: u32) -> Option<NodeId> {
        (idx != NIL).then(|| self.id_of(idx))
    }
    #[inline]
    pub(crate) fn check(&self, id: NodeId) -> DomResult<u32> {
        match self.slots.get(id.idx as usize) {
            Some(s) if s.gen == id.gen && s.node.is_some() => Ok(id.idx),
            _ => Err(DomError::StaleNode),
        }
    }
    /// [`Dom::check`] and the node in one slot lookup.
    #[inline]
    pub(crate) fn live(&self, id: NodeId) -> DomResult<(u32, &Node)> {
        match self.slots.get(id.idx as usize) {
            Some(Slot { gen, node: Some(n) }) if *gen == id.gen => Ok((id.idx, n)),
            _ => Err(DomError::StaleNode),
        }
    }
    /// Number of slots, live or free: no node has an index at or past it
    /// (`verify_tree_state`; tables kept by node index can end here).
    pub fn slot_count(&self) -> u32 {
        self.slots.len() as u32
    }
    pub(crate) fn generation_at(&self, idx: u32) -> u32 {
        self.slots.get(idx as usize).map_or(0, |s| s.gen)
    }
    pub(crate) fn slot_gen(&self, idx: u32) -> u32 {
        self.generation_at(idx)
    }

    pub(crate) fn slot_live(&self, idx: u32) -> bool {
        self.slots.get(idx as usize).is_some_and(|s| s.node.is_some())
    }

    pub fn is_alive(&self, id: NodeId) -> bool {
        self.check(id).is_ok()
    }
    pub fn node(&self, id: NodeId) -> DomResult<&Node> {
        Ok(self.live(id)?.1)
    }
    /// Whether a `<canvas>` element was ever created in this DOM.
    pub fn canvas_seen(&self) -> bool {
        self.canvas_seen
    }
    /// Whether an SVG `<animate>` element was ever created in this DOM.
    pub fn smil_seen(&self) -> bool {
        self.smil_seen
    }

    /// Whether a `style` attribute that mentions `transition` was ever set or parsed in this
    /// store: its elements' style changes may start transitions (React and UI kits set
    /// `transition` inline).
    pub fn inline_transitions_seen(&self) -> bool {
        self.inline_transitions_seen
    }
    /// Natural (intrinsic) size of a loaded image element, in CSS px.
    pub fn natural_size(&self, el: NodeId) -> Option<(f64, f64)> {
        self.check(el).ok()?;
        self.natural_sizes.get(&el).copied()
    }
    /// `naturalWidth` and `naturalHeight` of an image element: [`Dom::set_reported_natural_size`]'s
    /// value, else its [`Dom::natural_size`].
    pub fn reported_natural_size(&self, el: NodeId) -> Option<(f64, f64)> {
        self.check(el).ok()?;
        self.reported_natural_sizes.get(&el).or_else(|| self.natural_sizes.get(&el)).copied()
    }
    /// Records or clears what `naturalWidth`/`naturalHeight` report when it is not the layout's
    /// natural size (an SVG without natural dimensions reports the default object size).
    pub fn set_reported_natural_size(&mut self, el: NodeId, size: Option<(f64, f64)>) {
        if self.check(el).is_err() {
            return;
        }
        match size {
            Some(s) => self.reported_natural_sizes.insert(el, s),
            None => self.reported_natural_sizes.remove(&el),
        };
    }
    /// Records or clears an image element's natural size. A change bumps
    /// the version so layout and style caches recompute; it is journaled
    /// like an attribute change of the image (only its own box depends on it).
    pub fn set_natural_size(&mut self, el: NodeId, size: Option<(f64, f64)>) {
        let Ok(i) = self.check(el) else { return };
        let changed = match size {
            Some(s) => self.natural_sizes.insert(el, s) != Some(s),
            None => self.natural_sizes.remove(&el).is_some(),
        };
        if changed {
            self.bump();
            self.note(Change::Attr(i, 0, false));
        }
    }
    /// The source the image loader selected for an `<img>` from `srcset` or `<picture>`.
    pub fn image_source(&self, el: NodeId) -> Option<&str> {
        self.image_sources.get(&el).map(String::as_str)
    }
    /// Records or clears it; a change is journaled like an attribute change of the image.
    pub fn set_image_source(&mut self, el: NodeId, source: Option<&str>) {
        let Ok(i) = self.check(el) else { return };
        let changed = match source {
            Some(s) if self.image_sources.get(&el).is_some_and(|old| old == s) => false,
            Some(s) => {
                self.image_sources.insert(el, s.to_owned());
                true
            }
            None => self.image_sources.remove(&el).is_some(),
        };
        if changed {
            self.bump();
            self.note(Change::Attr(i, 0, false));
        }
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    /// See the `hook_epoch` field: changes whenever a queue the
    /// post-callback hook drains may have work (conservative; never misses
    /// one). Work that follows any mutation follows `version` instead.
    pub fn hook_epoch(&self) -> u64 {
        self.hook_epoch
    }
    pub(crate) fn touch_hook(&mut self) {
        self.hook_epoch = self.hook_epoch.wrapping_add(1);
    }
    /// Process-unique identity of this DOM (with `version`, a cache key
    /// that changes on every mutation).
    pub fn uid(&self) -> u64 {
        self.uid
    }
    #[track_caller]
    pub(crate) fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
        // Unknown until a precise `note` refines it: an un-annotated bump
        // (forms, parser, script state) makes consumers invalidate fully.
        self.push_change(Change::Unknown(std::panic::Location::caller()));
    }
    /// Bumped only by node insertion/removal, attribute changes and
    /// character-data changes (not by form-control state, focus or hover),
    /// i.e. the mutations that invalidate XPath iterator results.
    pub fn tree_version(&self) -> u64 {
        self.tree_version
    }

    /// Elements with recorded attribute snapshots (tests).
    #[doc(hidden)]
    pub fn style_snapshot_count(&self) -> usize {
        self.style_snapshots.borrow().as_ref().map_or(0, |s| {
            let mut elements: Vec<u32> = s.log.iter().map(|e| e.0).collect();
            elements.sort_unstable();
            elements.dedup();
            elements.len()
        })
    }

    /// Records attribute `name`'s value before a write (`None`: absent)
    /// when a style cascade reads this DOM and a selector can test it. A
    /// write no selector can see (most `data-*`, `aria-*`), a detached
    /// element's, or a second write to the same attribute before the next
    /// style read costs a lookup.
    #[inline]
    pub(crate) fn note_style_attr(&self, i: u32, name: &QualName, old: Option<&str>) {
        let mut all = self.style_snapshots.borrow_mut();
        let Some(state) = all.as_mut() else { return };
        let local = &name.local;
        if !(*local == local_name!("class") || *local == local_name!("id") || state.names.contains(local)) {
            return;
        }
        if self.n(i).parent == NIL || state.overflowed {
            return;
        }
        if state.log.len() >= STYLE_LOG_CAP {
            state.overflowed = true;
            state.log = Vec::new();
            return;
        }
        state.log.push((i, self.generation_at(i), name.clone(), old.map(Into::into)));
    }

    fn push_change(&mut self, change: Change) {
        const CAP: usize = JOURNAL_CAPACITY;
        if self.journal.len() >= CAP {
            // Keep the newer half: consumers read the journal after every
            // batch, so recent versions stay covered. (Clearing it all made
            // the next style flush after every 8192 changes, e.g. in the
            // middle of a React commit, restyle the whole document.)
            let drop = CAP / 2;
            // Entries share a version within one bump: cut at a version
            // boundary so a version is either fully covered or not at all.
            let cut_version = self.journal[drop - 1].0;
            self.journal_floor = cut_version;
            if cut_version == self.version {
                // One bump filled the journal: it cannot cover its version.
                self.journal.clear();
                return;
            }
            let end = self.journal.partition_point(|(v, _)| *v <= cut_version);
            self.journal.drain(..end);
        }
        self.journal.push((self.version, change));
    }

    /// Bumped whenever the child text content of an HTML `<style>` element
    /// may have changed (a child inserted or removed, a child's data
    /// changed), and by the changes that are never refined from `Unknown`
    /// (a document reset, a shadow root attached). With the style engine's
    /// sheet generation and which sheets are connected, it says when the
    /// author sheets' texts may have changed.
    pub fn style_text_revision(&self) -> u64 {
        self.style_text_revision
    }

    /// The [`Dom::style_text_revision`] at which the child text content of
    /// the HTML `<style>` element `el` last changed (0: not since it was
    /// created); `None` for a stale id or another node.
    pub fn style_element_text_revision(&self, el: NodeId) -> Option<u64> {
        let e = self.node(el).ok()?.element()?;
        (e.is_html() && e.name.local == local_name!("style")).then_some(e.style_text_revision)
    }

    /// The changes no `note` refines: everything may have changed.
    fn bump_style_text(&mut self) {
        self.style_text_revision = self.style_text_revision.wrapping_add(1);
    }

    /// `change` changed the children of an HTML `<style>`, or a child's
    /// data: its sheet text may differ.
    #[inline]
    fn note_style_text(&mut self, change: Change) {
        let parent = match change {
            Change::Inserted(p, _) | Change::Removed(p, _, _) => p,
            Change::Text(i) => self.n(i).parent,
            _ => return,
        };
        if parent == NIL {
            return;
        }
        let revision = self.style_text_revision.wrapping_add(1);
        if let NodeKind::Element(e) = &mut self.n_mut(parent).kind {
            if e.is_html() && e.name.local == local_name!("style") {
                e.style_text_revision = revision;
                self.style_text_revision = revision;
            }
        }
    }

    /// Refines what the latest `version` bump changed (replacing its
    /// `Unknown` entry).
    pub(crate) fn note(&mut self, change: Change) {
        // Before the journal: an overflow may have cleared it.
        self.note_style_text(change);
        match self.journal.last_mut() {
            Some((v, c)) if *v == self.version => *c = change,
            _ => {}
        }
    }

    /// Adds another change to the latest `version` bump (after `note`).
    fn note_more(&mut self, change: Change) {
        self.note_style_text(change);
        if self.journal.last().is_some_and(|(v, _)| *v == self.version) {
            self.push_change(change);
        }
    }

    /// The changes made after `version`, in order, or None when the journal
    /// no longer covers it (overflow): the caller must invalidate fully.
    pub fn changes_since(&self, version: u64) -> Option<impl Iterator<Item = Change> + '_> {
        if version < self.journal_floor || version > self.version {
            return None;
        }
        let start = self.journal.partition_point(|(v, _)| *v <= version);
        Some(self.journal[start..].iter().map(|(_, c)| *c))
    }

    /// Whether the members of a live collection over `root` (computed at
    /// `since`) are still the same, judged from the change journal: true
    /// when no change since then can have added, removed or reordered one.
    /// Uncovered versions, unknown changes and long change runs answer
    /// false (recompute).
    pub fn collection_unchanged_since(&self, root: NodeId, since: u64, watch: &CollectionWatch) -> bool {
        if since == self.version {
            return true;
        }
        let Ok(r) = self.check(root) else { return false };
        let Some(changes) = self.changes_since(since) else { return false };
        // Past this, walking the journal costs more than recomputing.
        const MOST: usize = 4096;
        // Whether a node at `idx` (a parent recorded by a change) lies in
        // the collection's scope. A freed slot is not known: changed.
        let in_scope = |idx: u32| -> Option<bool> {
            if !self.slot_live(idx) {
                return None;
            }
            Some(if watch.children_only { idx == r } else { self.is_inclusive_ancestor(r, idx) })
        };
        let counts = |idx: u32| -> bool { !watch.elements_only || !self.slot_live(idx) || self.n(idx).is_element() };
        for (i, c) in changes.enumerate() {
            if i >= MOST {
                return false;
            }
            let relevant = match c {
                Change::Pseudo | Change::Text(_) => Some(false),
                Change::State(n) => {
                    if watch.state {
                        in_scope(n)
                    } else {
                        Some(false)
                    }
                }
                Change::Attr(n, hash, _) => {
                    if watch.attrs.contains(&hash) {
                        // The element itself is the one that may join or leave.
                        if !self.slot_live(n) {
                            None
                        } else if watch.children_only {
                            Some(self.n(n).parent == r)
                        } else {
                            Some(n != r && self.is_inclusive_ancestor(r, n))
                        }
                    } else {
                        Some(false)
                    }
                }
                Change::Inserted(p, n) | Change::Removed(p, n, _) => {
                    if counts(n) {
                        in_scope(p)
                    } else {
                        Some(false)
                    }
                }
                Change::Unknown(_) => None,
            };
            if relevant != Some(false) {
                return false;
            }
        }
        true
    }

    /// Number of shadow roots attached in this DOM so far.
    pub fn shadow_root_count(&self) -> u32 {
        self.shadow_roots
    }

    /// Whether `host`'s shadow root is a user-agent one (rendered as if absent).
    pub(crate) fn has_ua_shadow_idx(&self, sr: u32) -> bool {
        matches!(&self.n(sr).kind, NodeKind::Fragment(f) if f.flags & crate::node::shadow_flags::USER_AGENT != 0)
    }

    /// Records a tree, attribute or character-data mutation.
    #[track_caller]
    pub(crate) fn bump_tree(&mut self) {
        self.tree_version = self.tree_version.wrapping_add(1);
        self.bump();
    }
    /// Monotonic count of node removals (cheap "did anything disconnect" check).
    pub fn removal_count(&self) -> u64 {
        self.removals
    }

    fn note_removal(&mut self, root: u32) {
        self.removals = self.removals.wrapping_add(1);
        if self.removal_log.len() == REMOVAL_LOG_LEN {
            self.removal_log.pop_front();
        }
        self.removal_log.push_back(root);
    }

    /// Slots of the nodes in subtrees removed after `removal_count` was `since`, as they are now.
    /// A slot may have been reused, and a node may be back in a document. `None` when the log
    /// doesn't go back that far or a document was deactivated: any node may have left.
    pub fn nodes_removed_since(&self, since: u64) -> Option<Vec<NodeId>> {
        let count = self.removals.wrapping_sub(since);
        if count > self.removal_log.len() as u64 {
            return None;
        }
        let mut nodes = Vec::new();
        for &root in self.removal_log.iter().skip(self.removal_log.len() - count as usize) {
            if root == NIL {
                return None;
            }
            // A freed root lists nothing (a reused slot: its node's subtree).
            if !self.slot_live(root) {
                continue;
            }
            let mut i = root;
            while i != NIL {
                nodes.push(self.id_of(i));
                i = self.next_shadow_including(i, root);
            }
        }
        Some(nodes)
    }
    /// Whether `collect_garbage` has anything to examine.
    pub fn has_gc_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }
    /// Number of live nodes in the store (all documents).
    pub fn live_nodes(&self) -> usize {
        self.live as usize
    }

    fn alloc(&mut self, kind: NodeKind, owner_doc: u32) -> u32 {
        let walk = Walk { first_child: NIL, next: NIL, parent: NIL, sum: summary(&kind) };
        let node = Node::new(kind, owner_doc);
        let idx = if let Some(idx) = self.free.pop() {
            self.slots[idx as usize].node = Some(node);
            self.walk[idx as usize] = walk;
            idx
        } else {
            let idx = u32::try_from(self.slots.len()).ok().filter(|&i| i != NIL).expect("DOM node limit");
            self.slots.push(Slot { gen: self.gen_floor, node: Some(node) });
            self.walk.push(walk);
            idx
        };
        self.live += 1;
        self.live_peak = self.live_peak.max(self.live);
        if owner_doc != NIL {
            self.doc_data_mut(owner_doc).owned += 1;
        }
        idx
    }

    /// Parser allocation: no collection candidate (node is linked at once).
    pub(crate) fn alloc_raw(&mut self, kind: NodeKind, owner_doc: u32) -> u32 {
        self.alloc(kind, owner_doc)
    }
    pub(crate) fn link_raw(&mut self, node: u32, parent: u32, before: u32) {
        if before != NIL && !self.live_ranges.is_empty() {
            self.ranges_before_insert(parent, before, 1);
        }
        self.link(node, parent, before);
        self.bump_tree();
        self.note(Change::Inserted(parent, node));
        self.announce_named_items(node);
    }
    pub(crate) fn unlink_raw(&mut self, node: u32) {
        let parent = self.n(node).parent;
        let next = self.n(node).next;
        if parent != NIL && !self.live_ranges.is_empty() {
            self.ranges_before_remove(node);
        }
        self.unlink(node);
        self.bump_tree();
        if parent != NIL {
            self.note(Change::Removed(parent, node, next));
        }
    }
    /// Appends a node a parser just made (`alloc_raw`) to `parent` once
    /// scripts may have run: the insertion a script's `appendChild` makes
    /// (records, reactions, upgrade candidates, resource and iframe steps),
    /// without the validity checks, as the tree is what the source says.
    pub(crate) fn append_parsed(&mut self, node: u32, parent: u32) {
        self.insert_nodes(&[node], parent, NIL, false);
    }
    pub(crate) fn set_owner_raw(&mut self, node: u32, doc: u32) {
        self.adopt_idx(node, doc);
    }
    pub(crate) fn adopt_raw(&mut self, node: u32, doc: u32) {
        self.adopt_idx(node, doc);
    }
    pub(crate) fn inert_doc_for_pub(&mut self, doc: u32) -> u32 {
        self.inert_doc_for(doc)
    }

    fn alloc_id(&mut self, kind: NodeKind, owner_doc: u32) -> NodeId {
        let idx = self.alloc(kind, owner_doc);
        self.candidates.push(idx);
        self.id_of(idx)
    }

    fn doc_data_mut(&mut self, doc: u32) -> &mut DocumentData {
        match &mut self.n_mut(doc).kind {
            NodeKind::Document(d) => d,
            _ => unreachable!("owner_doc must be a document"),
        }
    }
    fn doc_data(&self, doc: u32) -> &DocumentData {
        match &self.n(doc).kind {
            NodeKind::Document(d) => d,
            _ => unreachable!("owner_doc must be a document"),
        }
    }

    // ----- creation ----------------------------------------------------

    pub fn create_document(&mut self, kind: DocumentKind, url: &str) -> NodeId {
        let content_type = match kind {
            DocumentKind::Html => "text/html",
            DocumentKind::Xml => "application/xml",
        };
        let data = DocumentData {
            has_global_registry: false,
            preferred_sheet_name: None,
            raster_budget: Default::default(),
            kind,
            quirks: QuirksMode::NoQuirks,
            content_type: content_type.into(),
            charset: "UTF-8",
            url: url.into(),
            parsed_url: Default::default(),
            base_url: None,
            html_base: Default::default(),
            url_target: url_target(url),
            ids: Default::default(),
            inert_doc: NIL,
            scripting: false,
            owned: 0,
            held: 0,
            active: false,
            allow_declarative_shadow_roots: false,
            fragment_parse_host: false,
            focused: NIL,
            hovered: NIL,
            activated: NIL,
            focus_visible: false,
            design_mode: false,
            style_with_css: false,
            default_paragraph_separator: "div",
            document_element: NIL,
            head: NIL,
            body: NIL,
            viewport_meta: None,
            meta_referrer_policy: None,
            header_referrer_policy: None,
            layout_client: None,
            responsive_embedded: false,
            content_language: None,
        };
        let idx = self.alloc(NodeKind::Document(Box::new(data)), NIL);
        // A document owns itself, and is connected.
        self.n_mut(idx).owner_doc = idx;
        self.n_mut(idx).set_tree_flags(tree_flags::CONNECTED);
        self.doc_data_mut(idx).owned = 1;
        self.id_of(idx)
    }

    /// HTML `document.designMode` of `doc`: whether the whole document is editable.
    pub fn design_mode(&self, doc: NodeId) -> bool {
        self.check(doc).ok().is_some_and(|i| matches!(&self.n(i).kind, NodeKind::Document(d) if d.design_mode))
    }

    pub fn set_style_with_css(&mut self, doc: NodeId, css: bool) -> DomResult<()> {
        let i = self.check(doc)?;
        let NodeKind::Document(d) = &mut self.n_mut(i).kind else { return Err(DomError::Type("not a document")) };
        d.style_with_css = css;
        Ok(())
    }

    pub fn set_paragraph_separator(&mut self, doc: NodeId, value: &str) -> DomResult<bool> {
        let tag = if value.eq_ignore_ascii_case("div") {
            "div"
        } else if value.eq_ignore_ascii_case("p") {
            "p"
        } else {
            return Ok(false);
        };
        let i = self.check(doc)?;
        let NodeKind::Document(d) = &mut self.n_mut(i).kind else { return Err(DomError::Type("not a document")) };
        d.default_paragraph_separator = tag;
        Ok(true)
    }

    /// Sets `doc`'s `designMode` (`on`).
    pub fn set_design_mode(&mut self, doc: NodeId, on: bool) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.design_mode = on,
            _ => return Err(DomError::Type("not a document")),
        }
        Ok(())
    }

    pub fn inherit_raster_budget(&mut self, doc: NodeId, parent: NodeId) -> DomResult<()> {
        let budget = self.document_data(parent)?.raster_budget.clone();
        let index = self.check(doc)?;
        match &mut self.n_mut(index).kind {
            NodeKind::Document(data) => data.raster_budget = budget,
            _ => return Err(DomError::Type("not a document")),
        }
        Ok(())
    }

    /// Marks a document as displayed by a browsing context; active documents
    /// and everything reachable from them are never collected.
    pub fn set_document_active(&mut self, doc: NodeId, active: bool) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => { d.active = active; d.has_global_registry |= active; }
            _ => return Err(DomError::Type("not a document")),
        }
        if active {
            self.note_inert_realm(i);
        }
        if !active {
            self.candidates.push(i);
            // Nodes of a deactivated document stop counting as "live in a
            // browsing context" for pin reconciliation.
            self.note_removal(NIL);
        }
        Ok(())
    }

    /// The element `:target` matches in document `doc` (`DocumentData::url_target`): the first
    /// in tree order whose id is the URL's fragment, in the document tree.
    pub fn url_target_element(&self, doc: NodeId) -> Option<NodeId> {
        let id = self.document_data(doc).ok()?.url_target.clone()?;
        self.get_element_by_id(doc, &id).ok().flatten()
    }

        pub fn document_data(&self, doc: NodeId) -> DomResult<&DocumentData> {
        match &self.node(doc)?.kind {
            NodeKind::Document(d) => Ok(d),
            _ => Err(DomError::Type("not a document")),
        }
    }
    /// `doc`'s URL parsed (`None` if it does not parse): made on the first
    /// request for it and dropped when the URL changes, so a document's
    /// requests (each one's referrer) do not parse it again.
    pub fn document_url_parsed(&self, doc: NodeId) -> DomResult<Option<&url::Url>> {
        let d = self.document_data(doc)?;
        Ok(d.parsed_url.get_or_init(|| url::Url::parse(&d.url).ok()).as_ref())
    }
    pub fn set_document_url(&mut self, doc: NodeId, url: &str) -> DomResult<()> {
        let i = self.check(doc)?;
        let retargeted = match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => {
                d.url = url.into();
                d.parsed_url.take();
                let target = url_target(url);
                let retargeted = d.url_target != target;
                d.url_target = target;
                retargeted
            }
            _ => return Err(DomError::Type("not a document")),
        };
        self.html_base_changed(i);
        // `:target` matches another element: styles start over.
        self.publish_image_defaults(doc);
        if retargeted {
            self.bump();
        }
        Ok(())
    }
    /// What `doc`'s `<base>` resolves to may have changed: a `<base>` came
    /// or went, a connected one's `href` changed, or the fallback base URL.
    pub(crate) fn html_base_changed(&mut self, doc: u32) {
        if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
            d.html_base.take();
        }
    }

    pub(crate) fn is_html_meta(&self, idx: u32) -> bool {
        self.n(idx).element().is_some_and(|e| e.name.local == local_name!("meta") && e.is_html())
    }

    /// Blink's `HTMLMetaElement::ProcessContent` for the viewport, after the
    /// `<meta>` `idx` was inserted or its `name` or `content` changed: in a
    /// document tree, a viewport `<meta>` with a `content` becomes its
    /// document's ([`crate::viewport_meta`]). The latest one processed wins,
    /// unless it is of a lower kind, and removing it changes nothing, as in
    /// Chrome.
    pub(crate) fn process_viewport_meta(&mut self, idx: u32) {
        let n = self.n(idx);
        if !n.in_document_tree() {
            return;
        }
        let Some(e) = n.element() else { return };
        // HTML "Content security policy state" of a `<meta http-equiv>` child of the head: its
        // policy joins the document's (only its Trusted Types directives are kept).
        if e.attr_local(&local_name!("http-equiv")).is_some_and(|v| v.eq_ignore_ascii_case("content-security-policy")) {
            let doc = n.owner_doc;
            let in_head = self.doc_data(doc).head != NIL && n.parent == self.doc_data(doc).head;
            if let (true, Some(content)) = (in_head, e.attr_local(&local_name!("content")).map(str::to_owned)) {
                let doc = self.id_of(doc);
                self.add_csp_policies(doc, &content, false);
            }
            return;
        }
        // A responsive embedded sizing `<meta>` opts the document in while it has no body yet, and for good
        // (Blink `HTMLMetaElement::ProcessContent`; WPT css-sizing `responsive-iframe/*`)
        if e.attr_local(&local_name!("name")).is_some_and(|name| name.eq_ignore_ascii_case("responsive-embedded-sizing")) {
            let doc = n.owner_doc;
            let d = self.doc_data_mut(doc);
            if d.body == NIL && !d.responsive_embedded {
                d.responsive_embedded = true;
                self.viewport_meta_version += 1;
            }
            return;
        }
        let (Some(name), Some(content)) = (e.attr_local(&local_name!("name")), e.attr_local(&local_name!("content")))
        else {
            return;
        };
        let Some(meta) = crate::viewport_meta::MetaViewport::from_meta(name, content) else { return };
        let doc = n.owner_doc;
        let d = self.doc_data_mut(doc);
        if d.viewport_meta.as_ref().is_some_and(|old| old.kind > meta.kind || *old == meta) {
            return;
        }
        d.viewport_meta = Some(meta);
        self.viewport_meta_version += 1;
    }

    /// HTML's `<meta name=referrer>` steps, after the `<meta>` `idx` was
    /// inserted or its `name` or `content` changed: in a document tree, with
    /// a `content` naming a policy (or a legacy keyword), it sets its
    /// document's referrer policy. The latest one processed wins wherever it
    /// is in the tree; an empty or unknown value changes nothing, and
    /// neither does removing the element.
    pub(crate) fn process_referrer_meta(&mut self, idx: u32) {
        let n = self.n(idx);
        if !n.in_document_tree() {
            return;
        }
        let Some(e) = n.element() else { return };
        if !e.attr_local(&local_name!("name")).is_some_and(|name| name.eq_ignore_ascii_case("referrer")) {
            return;
        }
        let Some(policy) = e.attr_local(&local_name!("content")).and_then(meta_referrer_policy) else { return };
        let doc = n.owner_doc;
        self.doc_data_mut(doc).meta_referrer_policy = Some(policy);
        self.publish_image_defaults(self.id_of(doc));
    }

    pub fn effective_referrer_policy(&self, doc: NodeId) -> &'static str {
        self.document_data(doc).ok().and_then(|d| d.meta_referrer_policy.or(d.header_referrer_policy)).unwrap_or("strict-origin-when-cross-origin")
    }

    pub fn set_header_referrer_policy(&mut self, doc: NodeId, policy: Option<&'static str>) {
        if let Ok(index) = self.check(doc) {
            self.doc_data_mut(index).header_referrer_policy = policy;
            self.publish_image_defaults(doc);
        }
    }

    pub fn set_layout_client(&mut self, doc: NodeId, client: String, bodies: &std::sync::Arc<crate::geometry::SubresourceBodies>) {
        if let Ok(index) = self.check(doc) {
            bodies.bind_raster_budget(&client, self.doc_data_mut(index).raster_budget.clone());
            self.doc_data_mut(index).layout_client = Some((client, std::sync::Arc::downgrade(bodies)));
            self.publish_image_defaults(doc);
        }
    }

    fn publish_image_defaults(&self, doc: NodeId) {
        let Ok(data) = self.document_data(doc) else { return };
        let Some((client, weak)) = &data.layout_client else { return };
        if let Some(bodies) = weak.upgrade() {
            bodies.set_image_defaults(client, data.base_url.as_deref().unwrap_or(&data.url), self.effective_referrer_policy(doc));
        }
    }

    /// The referrer policy `doc`'s `<meta name=referrer>` elements gave it
    /// (a policy token), if any did.
    pub fn meta_referrer_policy(&self, doc: NodeId) -> Option<&'static str> {
        self.document_data(doc).ok()?.meta_referrer_policy
    }

    /// [`Dom::process_viewport_meta`] for the `<meta>` elements of an
    /// inserted subtree, in tree order.
    pub(crate) fn process_viewport_metas(&mut self, root: u32) {
        let mut i = root;
        while i != NIL {
            if self.is_html_meta(i) {
                self.process_referrer_meta(i);
                self.process_viewport_meta(i);
            }
            i = self.following_in(i, root);
        }
    }

    /// The viewport `<meta>` `doc` processed last: what lays it out under
    /// mobile emulation ([`crate::viewport_meta`]).
    pub fn viewport_meta(&self, doc: NodeId) -> Option<&crate::viewport_meta::MetaViewport> {
        self.document_data(doc).ok()?.viewport_meta.as_ref()
    }

    /// The content height of the document `iframe` shows when that document is responsively embedded
    /// (`None`: it is not), which the render snapshot gives the renderer: an iframe of `frame-sizing:
    /// content-height` takes it as its natural height (css-sizing-4).
    pub fn set_frame_content_height(&mut self, iframe: NodeId, height: Option<f32>) {
        let Ok(i) = self.check(iframe) else { return };
        if self.frame_content_heights.get(&i).copied() == height {
            return;
        }
        match height {
            Some(height) => self.frame_content_heights.insert(i, height),
            None => self.frame_content_heights.remove(&i),
        };
        self.bump();
        self.note(Change::State(i));
    }

    /// pyro: the pan and zoom (`currentTranslate` x and y, `currentScale`) of the `<svg>` `svg`.
    pub fn svg_pan(&self, svg: NodeId) -> (f64, f64, f64) {
        self.check(svg).ok().and_then(|i| self.svg_pans.get(&i).copied()).unwrap_or((0.0, 0.0, 1.0))
    }

    /// pyro: sets the pan and zoom of the `<svg>` `svg`, which its render tree carries.
    pub fn set_svg_pan(&mut self, svg: NodeId, pan: (f64, f64, f64)) {
        let Ok(i) = self.check(svg) else { return };
        if self.svg_pans.get(&i).copied().unwrap_or((0.0, 0.0, 1.0)) == pan {
            return;
        }
        if pan == (0.0, 0.0, 1.0) {
            self.svg_pans.remove(&i);
        } else {
            self.svg_pans.insert(i, pan);
        }
        self.bump();
        self.note(Change::State(i));
    }

    /// The documents the iframes show, for the renderer to paint as their content (see
    /// [`FrameDocument`]); iframes not in `documents` paint none.
    ///
    /// `scope`: the documents of the page whose frames these are. The store may be shared by pages (a
    /// popup shares its opener's): only entries of iframes in those documents (or of freed nodes) are
    /// dropped when missing from `documents`, never another page's.
    pub fn set_frame_documents(&mut self, scope: &[NodeId], documents: Vec<(NodeId, FrameDocument)>) {
        let keep: Vec<u32> = documents.iter().filter_map(|(iframe, _)| self.check(*iframe).ok()).collect();
        let in_scope = self.scope_indices(scope);
        let stale: Vec<u32> =
            self.frame_documents.keys().copied().filter(|i| !keep.contains(i) && self.in_frame_scope(*i, &in_scope)).collect();
        for i in stale {
            self.frame_documents.remove(&i);
            if self.slot_live(i) {
                self.bump();
                self.note(Change::State(i));
            }
        }
        for (iframe, document) in documents {
            self.set_frame_document(iframe, Some(document));
        }
    }

    fn scope_indices(&self, scope: &[NodeId]) -> Vec<u32> {
        scope.iter().filter_map(|d| self.check(*d).ok()).collect()
    }

    /// Whether iframe slot `i` belongs to one of the documents `scope` (by slot index), or is freed.
    fn in_frame_scope(&self, i: u32, scope: &[u32]) -> bool {
        if !self.slot_live(i) {
            return true;
        }
        scope.contains(&self.n(i).owner_doc)
    }

    /// pyro: the frame table of the render snapshot: each frame element's document by the index
    /// its [`crate::snapshot::FRAME_ATTRIBUTE`] names (the renderer looks the document up here;
    /// markup holds no frame content or client).
    pub fn frame_table(&self, doc: NodeId) -> FrameTable {
        let nonce = self.check(doc).map(|d| self.snapshot_nonce(d)).unwrap_or_default();
        let entries = self
            .frame_documents
            .iter()
            .filter(|(i, _)| self.slot_live(**i) && self.check(doc).is_ok_and(|d| self.n(**i).owner_doc == d))
            .map(|(i, d)| (crate::snapshot::render_key(self.id_of(*i)).to_string(), d.clone()))
            .collect();
        FrameTable { nonce, entries }
    }

    /// pyro: the `Referrer-Policy` of the response a sheet link's sheet is of: the render snapshot
    /// writes it on the link ([`crate::snapshot::SHEET_POLICY_ATTRIBUTE`]), which is how the
    /// renderer gets it for the sheet's own requests (by the sheet, never by its URL).
    pub fn set_sheet_policy(&mut self, link: NodeId, policy: Option<&str>) {
        let Ok(i) = self.check(link) else { return };
        let changed = match policy {
            Some(policy) => self.sheet_policies.insert(i, policy.to_owned()).as_deref() != Some(policy),
            None => self.sheet_policies.remove(&i).is_some(),
        };
        if changed {
            self.bump();
            self.note(Change::State(i));
        }
    }

    /// pyro: the response tainting of the sheet of `link` (opaque or not), which the renderer
    /// takes with its text (`Dom::sheet_key`).
    pub fn set_sheet_tainting(&mut self, link: NodeId, opaque: bool) {
        let Ok(i) = self.check(link) else { return };
        let changed = if opaque { self.sheet_tainted.insert(i) } else { self.sheet_tainted.remove(&i) };
        if changed {
            self.bump();
            self.note(Change::State(i));
        }
    }

    /// pyro: the key of the sheet of `link` in a render snapshot's sheets (its render key, `:o`
    /// when its response's tainting is opaque): the link's engine attribute names it, and the
    /// renderer serves the sheet by it, never by URL.
    pub fn sheet_key(&self, link: NodeId) -> String {
        let opaque = self.check(link).is_ok_and(|i| self.sheet_tainted.contains(&i));
        format!("{}{}", crate::snapshot::render_key(link), if opaque { ":o" } else { "" })
    }

    /// pyro: the nonce the frame markers of document slot `doc`'s render snapshots carry
    /// (`snapshot::FRAME_ATTRIBUTE`): 128 bits from the OS CSPRNG, drawn when the document is
    /// first serialised and kept for its life (a delta's markers match the table too). Held here
    /// only: page script never sees a snapshot, and the text it wrote existed before the nonce.
    pub(crate) fn snapshot_nonce(&self, doc: u32) -> String {
        self.snapshot_nonces
            .borrow_mut()
            .entry(doc)
            .or_insert_with(|| {
                let mut bits = [0u8; 16];
                getrandom::fill(&mut bits).expect("the OS CSPRNG");
                bits.iter().map(|b| format!("{b:02x}")).collect()
            })
            .clone()
    }

    /// [`Dom::set_frame_documents`] for one iframe (`None`: it paints none).
    pub fn set_frame_document(&mut self, iframe: NodeId, document: Option<FrameDocument>) {
        let Ok(i) = self.check(iframe) else { return };
        if self.frame_documents.get(&i) == document.as_ref() {
            return;
        }
        match document {
            Some(document) => self.frame_documents.insert(i, document),
            None => self.frame_documents.remove(&i),
        };
        self.bump();
        self.note(Change::State(i));
    }

    /// [`Dom::set_frame_content_height`] for all iframes at once: those not in `heights` have none.
    /// `scope`: as for [`Dom::set_frame_documents`].
    pub fn set_frame_content_heights(&mut self, scope: &[NodeId], heights: &[(NodeId, f32)]) {
        let keep: Vec<u32> = heights.iter().filter_map(|&(iframe, _)| self.check(iframe).ok()).collect();
        let in_scope = self.scope_indices(scope);
        let stale: Vec<u32> = self
            .frame_content_heights
            .keys()
            .copied()
            .filter(|i| !keep.contains(i) && self.in_frame_scope(*i, &in_scope))
            .collect();
        for i in stale {
            self.frame_content_heights.remove(&i);
            if self.slot_live(i) {
                self.bump();
                self.note(Change::State(i));
            }
        }
        for &(iframe, height) in heights {
            self.set_frame_content_height(iframe, Some(height));
        }
    }

    /// Whether `doc` opted in to responsive embedded sizing (`DocumentData::responsive_embedded`).
    pub fn is_responsively_embedded(&self, doc: NodeId) -> bool {
        self.document_data(doc).is_ok_and(|d| d.responsive_embedded)
    }

    /// Bumped whenever a document's viewport `<meta>` changes.
    pub fn viewport_meta_version(&self) -> u64 {
        self.viewport_meta_version
    }
    /// Sets an inherited, already-resolved base URL without changing URL/origin.
    /// None restores resolution against the document's current URL.
    pub fn set_document_base_url(&mut self, doc: NodeId, base: Option<&str>) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.base_url = base.map(Into::into),
            _ => return Err(DomError::Type("not a document")),
        }
        self.html_base_changed(i);
        self.publish_image_defaults(doc);
        Ok(())
    }
    /// The document base URL: its first `<base href>`, else the fallback.
    pub fn document_base_url(&self, doc: NodeId) -> DomResult<&str> {
        let d = self.document_data(doc)?;
        if self.base_seen {
            let html_base = d.html_base.get_or_init(|| self.first_base_href(doc));
            if let Some(base) = html_base {
                return Ok(base);
            }
        }
        self.document_fallback_base_url(doc)
    }
    /// The base URL ignoring `<base>`, which resolves `<base href>` itself.
    pub fn document_fallback_base_url(&self, doc: NodeId) -> DomResult<&str> {
        let d = self.document_data(doc)?;
        Ok(d.base_url.as_deref().unwrap_or(&d.url))
    }
    /// The `target` of the first `<base>` in `doc` that has one (HTML "get an element's
    /// target": a link without a target of its own uses it).
    pub fn base_target(&self, doc: NodeId) -> Option<String> {
        if !self.base_seen {
            return None;
        }
        self.descendants(doc).ok()?.find_map(|id| {
            let e = self.element(id).ok()?;
            (e.is_html() && *e.local_name() == *"base").then(|| self.get_attribute(id, "target").ok().flatten().map(str::to_owned))?
        })
    }
    fn first_base_href(&self, doc: NodeId) -> Option<Box<str>> {
        let href = self.descendants(doc).ok()?.find_map(|id| {
            let e = self.element(id).ok()?;
            (e.is_html() && *e.local_name() == *"base").then(|| self.get_attribute(id, "href").ok().flatten())?
        })?;
        let fallback = url::Url::parse(self.document_fallback_base_url(doc).ok()?).ok();
        let url = url::Url::options().base_url(fallback.as_ref()).parse(href).ok()?;
        // HTML: a data: or javascript: <base> is ignored.
        (!matches!(url.scheme(), "data" | "javascript")).then(|| url.as_str().into())
    }
    /// Makes `doc` an XML or HTML document of `content_type` (a navigation
    /// whose response decided it) decoded as `charset`.
    pub fn set_document_kind(
        &mut self,
        doc: NodeId,
        kind: DocumentKind,
        content_type: &str,
        charset: &'static str,
    ) -> DomResult<()> {
        let i = self.check(doc)?;
        if let NodeKind::Document(d) = &mut self.n_mut(i).kind {
            d.kind = kind;
            d.content_type = content_type.into();
            d.charset = charset;
            d.quirks = QuirksMode::NoQuirks;
        }
        self.bump();
        self.bump_style_text();
        Ok(())
    }

    /// The language of `doc` from the protocol: an HTTP `Content-Language`
    /// header's first language, as Chromium takes it (`FrameLoader::
    /// DidBeginDocument` keeps what comes before the first comma, trimmed).
    /// `:lang()` falls back to it when no element and no
    /// `<meta http-equiv="content-language">` gives one.
    pub fn set_content_language(&mut self, doc: NodeId, header: Option<&str>) -> DomResult<()> {
        let i = self.check(doc)?;
        let lang = header
            .and_then(|h| h.split(',').next())
            .map(|v| v.trim_matches(|c: char| c.is_ascii_whitespace()))
            .filter(|v| !v.is_empty())
            .map(Into::into);
        if let NodeKind::Document(d) = &mut self.n_mut(i).kind {
            d.content_language = lang;
        }
        self.bump();
        Ok(())
    }

    /// [`Dom::set_content_language`]'s language of `doc`.
    pub fn content_language(&self, doc: NodeId) -> Option<&str> {
        let i = self.check(doc).ok()?;
        match &self.n(i).kind {
            NodeKind::Document(d) => d.content_language.as_deref(),
            _ => None,
        }
    }

    /// Whether `doc` is an XML document (XHTML, SVG, other XML), whose
    /// selectors match names case-sensitively.
    pub fn is_xml_document(&self, doc: NodeId) -> bool {
        self.check(doc).is_ok_and(|i| matches!(&self.n(i).kind, NodeKind::Document(d) if d.kind == DocumentKind::Xml))
    }

    /// Whether `doc` is in quirks mode (not limited quirks): an HTML document whose doctype
    /// the parser found missing or legacy.
    pub fn is_quirks_mode(&self, doc: NodeId) -> bool {
        self.check(doc).is_ok_and(|i| matches!(&self.n(i).kind, NodeKind::Document(d) if d.quirks == QuirksMode::Quirks))
    }

    /// Whether `doc` is in limited-quirks mode (an HTML document whose doctype is a
    /// transitional or frameset one with a system identifier).
    pub fn is_limited_quirks_mode(&self, doc: NodeId) -> bool {
        self.check(doc)
            .is_ok_and(|i| matches!(&self.n(i).kind, NodeKind::Document(d) if d.quirks == QuirksMode::LimitedQuirks))
    }

    pub fn set_quirks_mode(&mut self, doc: NodeId, q: QuirksMode) -> DomResult<()> {
        let i = self.check(doc)?;
        if let NodeKind::Document(d) = &mut self.n_mut(i).kind {
            d.quirks = q;
        }
        Ok(())
    }

    fn owner_for(&self, doc: NodeId) -> DomResult<u32> {
        let i = self.check(doc)?;
        match self.n(i).kind {
            NodeKind::Document(_) => Ok(i),
            _ => Ok(self.n(i).owner_doc),
        }
    }

    fn implicit_registry(&self, node: u32) -> RegistryAssociation {
        let doc = self.n(node).owner_doc;
        if self.doc_data(doc).has_global_registry { RegistryAssociation::Default } else { RegistryAssociation::Null }
    }

    pub(crate) fn registry_idx(&self, node: u32) -> RegistryAssociation {
        self.registry_associations.get(&node).copied().unwrap_or_else(|| self.implicit_registry(node))
    }

    pub(crate) fn set_registry_idx(&mut self, node: u32, registry: RegistryAssociation) {
        if registry == self.implicit_registry(node) {
            if !self.registry_associations.is_empty() { self.registry_associations.remove(&node); }
        } else {
            self.registry_associations.insert(node, registry);
        }
    }

    pub fn registry_association(&self, node: NodeId) -> DomResult<RegistryAssociation> {
        Ok(self.registry_idx(self.check(node)?))
    }

    pub fn set_registry_association(&mut self, node: NodeId, registry: RegistryAssociation) -> DomResult<()> {
        let node = self.check(node)?;
        self.set_registry_idx(node, registry);
        Ok(())
    }

    pub fn live_scoped_registries(&self) -> FxHashSet<u64> {
        self.registry_associations.values().copied()
            .chain(self.custom_definition_owners.values().map(|d| d.registry))
            .chain(self.upgrades.pending_creations.values().copied())
            .filter_map(|association| match association {
                RegistryAssociation::Scoped(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    /// The order belongs to the registry across documents, even if an embedder
    /// keeps those documents in different DOM arenas.
    pub fn note_scoped_registry_document(&mut self, registry: u64, doc: NodeId) -> DomResult<()> {
        self.check(doc)?;
        self.note_registry_connection(registry, doc);
        Ok(())
    }

    fn note_registry_connection(&mut self, registry: u64, doc: NodeId) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let documents = self.scoped_registry_docs.entry(registry).or_default();
        if !documents.iter().any(|(_, known)| *known == doc) {
            documents.push((NEXT.fetch_add(1, Ordering::Relaxed), doc));
        }
    }

    /// Called once the registry has no live wrapper or DOM references.
    pub fn forget_scoped_registry(&mut self, registry: u64) {
        self.scoped_registry_docs.remove(&registry);
        self.forget_registry_definitions(RegistryAssociation::Scoped(registry), None);
        self.shadow_disabled.retain(|(_, association, _, _)| *association != RegistryAssociation::Scoped(registry));
    }

    pub fn scoped_registry_documents(&self, registry: u64) -> Vec<(u64, NodeId)> {
        self.scoped_registry_docs.get(&registry).cloned().unwrap_or_default()
    }

    pub fn custom_definition_owner(&self, el: NodeId) -> DomResult<Option<CustomDefinitionOwner>> {
        Ok(self.custom_definition_owners.get(&self.check(el)?).copied())
    }

    pub fn set_custom_definition_owner(&mut self, el: NodeId, owner: CustomDefinitionOwner) -> DomResult<()> {
        let i = self.check(el)?;
        self.custom_definition_owners.insert(i, owner);
        Ok(())
    }

    pub fn shadow_including_nodes(&self, root: NodeId) -> DomResult<Vec<NodeId>> {
        Ok(self.shadow_including_subtree(self.check(root)?).into_iter().map(|i| self.id_of(i)).collect())
    }

    /// Creates an element without name validation (parser / internal use).
    pub fn create_element_raw(&mut self, doc: NodeId, name: QualName) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        let is_template = name.ns == ns!(html) && name.local == local_name!("template");
        self.base_seen |= name.local == local_name!("base");
        match name.local {
            local_name!("audio") | local_name!("video") => self.media_seen = true,
            local_name!("canvas") => self.canvas_seen = true,
            local_name!("animate") if name.ns == ns!(svg) => self.smil_seen = true,
            _ => {}
        }
        let idx = self.alloc_raw(NodeKind::Element(Box::new(ElementData::new(name))), owner);
        self.set_registry_idx(idx, self.registry_idx(owner));
        if is_template {
            let inert = self.inert_doc_for(owner);
            let frag = self.alloc(NodeKind::Fragment(FragmentData::new(idx, None, 0)), inert);
            self.n_mut(idx).element_mut().unwrap().template_contents = frag;
        }
        self.candidates.push(idx);
        Ok(self.id_of(idx))
    }

    /// `document.createElement(localName)`.
    pub fn create_element(&mut self, doc: NodeId, local: &str) -> DomResult<NodeId> {
        validate_element_name(local)?;
        let owner = self.owner_for(doc)?;
        let html = self.doc_data(owner).kind == DocumentKind::Html;
        let (local, ns) = if html {
            // Script mostly passes lowercase names: no lowercased copy then.
            let local = if local.bytes().any(|b| b.is_ascii_uppercase()) {
                LocalName::from(local.to_ascii_lowercase())
            } else {
                LocalName::from(local)
            };
            (local, ns!(html))
        } else {
            // XML documents: "application/xhtml+xml" gets the HTML namespace.
            let ns = if &*self.doc_data(owner).content_type == "application/xhtml+xml" { ns!(html) } else { ns!() };
            (LocalName::from(local), ns)
        };
        self.create_element_raw(doc, QualName::new(None, ns, local))
    }

    /// [`Dom::create_element`] for an interned name.
    pub fn create_element_named(&mut self, doc: NodeId, name: &ScriptName) -> DomResult<NodeId> {
        if !name.valid_element {
            return Err(invalid_character());
        }
        let owner = self.owner_for(doc)?;
        let (local, ns) = if self.doc_data(owner).kind == DocumentKind::Html {
            (name.lower.clone(), ns!(html))
        } else {
            let ns = if &*self.doc_data(owner).content_type == "application/xhtml+xml" { ns!(html) } else { ns!() };
            (name.exact.clone(), ns)
        };
        self.create_element_raw(doc, QualName::new(None, ns, local))
    }

    /// `document.createElementNS(namespace, qualifiedName)`.
    pub fn create_element_ns(&mut self, doc: NodeId, namespace: Option<&str>, qualified: &str) -> DomResult<NodeId> {
        let name = validate_and_extract(namespace, qualified, NameContext::Element)?;
        self.create_element_raw(doc, name)
    }

    pub fn create_text(&mut self, doc: NodeId, data: &str) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        Ok(self.alloc_id(NodeKind::Text(data.to_owned()), owner))
    }
    pub fn create_comment(&mut self, doc: NodeId, data: &str) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        Ok(self.alloc_id(NodeKind::Comment(data.to_owned()), owner))
    }
    pub fn create_cdata(&mut self, doc: NodeId, data: &str) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        if self.doc_data(owner).kind == DocumentKind::Html {
            return Err(DomError::new(
                DomErrorKind::NotSupported,
                "CDATA sections are not supported in HTML documents",
            ));
        }
        if data.contains("]]>") {
            return Err(DomError::new(DomErrorKind::InvalidCharacter, "CDATA data contains ']]>'"));
        }
        Ok(self.alloc_id(NodeKind::CData(data.to_owned()), owner))
    }
    pub fn create_processing_instruction(&mut self, doc: NodeId, target: &str, data: &str) -> DomResult<NodeId> {
        validate_name(target)?;
        if data.contains("?>") {
            return Err(DomError::new(DomErrorKind::InvalidCharacter, "processing instruction data contains '?>'"));
        }
        let owner = self.owner_for(doc)?;
        let kind = NodeKind::ProcessingInstruction(Box::new(PiData { target: target.into(), data: data.to_owned() }));
        Ok(self.alloc_id(kind, owner))
    }
    pub fn create_fragment(&mut self, doc: NodeId) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        Ok(self.alloc_id(NodeKind::Fragment(FragmentData::new(NIL, None, 0)), owner))
    }
    pub fn create_doctype(&mut self, doc: NodeId, name: &str, public_id: &str, system_id: &str) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        let data = DoctypeData { name: name.into(), public_id: public_id.into(), system_id: system_id.into() };
        Ok(self.alloc_id(NodeKind::Doctype(Box::new(data)), owner))
    }

    /// The "appropriate template contents owner document" of `doc`.
    fn inert_doc_for(&mut self, doc: u32) -> u32 {
        let existing = self.doc_data(doc).inert_doc;
        if existing != NIL {
            return existing;
        }
        if self.n(doc).owner_doc == doc && self.is_inert_doc(doc) {
            return doc;
        }
        let kind = self.doc_data(doc).kind;
        let inert = self.create_document(kind, "about:blank").idx;
        // The inert document lives exactly as long as its creator: the
        // reference is dropped when the creator is collected.
        self.ext_ref_inc(inert);
        if let NodeKind::Document(d) = &mut self.n_mut(inert).kind {
            d.inert_doc = inert;
        }
        self.doc_data_mut(doc).inert_doc = inert;
        self.note_inert_realm(doc);
        inert
    }
    fn is_inert_doc(&self, doc: u32) -> bool {
        self.doc_data(doc).inert_doc == doc
    }

    // ----- shadow roots ------------------------------------------------

    /// `Element.attachShadow({mode})`.
    pub fn attach_shadow(&mut self, host: NodeId, mode: ShadowRootMode) -> DomResult<NodeId> {
        self.attach_shadow_with(host, mode, 0)
    }

    /// DOM "attach a shadow root"; `flags` are `shadow_flags`. A declarative
    /// root of the same mode is emptied and handed over instead.
    pub fn attach_shadow_with(&mut self, host: NodeId, mode: ShadowRootMode, flags: u8) -> DomResult<NodeId> {
        let h = self.check(host)?;
        let el = self.n(h).element().ok_or(DomError::Type("not an element"))?;
        let valid_host = el.is_html()
            && (matches!(
                &*el.name.local,
                "article"
                    | "aside"
                    | "blockquote"
                    | "body"
                    | "div"
                    | "footer"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "header"
                    | "main"
                    | "nav"
                    | "p"
                    | "section"
                    | "span"
            ) || is_valid_custom_element_name(&el.name.local));
        if !valid_host {
            return Err(DomError::new(DomErrorKind::NotSupported, "This element does not support attachShadow"));
        }
        if self.shadow_disabled.iter().any(|(doc, registry, name, local)| {
            *local == el.name.local && (name == local || el.is_value.as_ref() == Some(name))
                && *registry == self.registry_idx(h) && doc.is_none_or(|d| d == self.id_of(self.n(h).owner_doc))
        }) {
            return Err(DomError::new(
                DomErrorKind::NotSupported,
                "attachShadow() is disabled by disabledFeatures static field",
            ));
        }
        if el.shadow_root != NIL {
            let sr = el.shadow_root;
            let claimable = matches!(
                &self.n(sr).kind,
                NodeKind::Fragment(f) if f.shadow == Some(mode) && f.flags & shadow_flags::DECLARATIVE != 0
            );
            if !claimable {
                return Err(DomError::new(
                    DomErrorKind::NotSupported,
                    "Shadow root cannot be created on a host which already hosts a shadow tree",
                ));
            }
            let id = self.id_of(sr);
            self.replace_all(id, None)?;
            if let NodeKind::Fragment(f) = &mut self.n_mut(sr).kind {
                f.flags &= !shadow_flags::DECLARATIVE;
            }
            return Ok(id);
        }
        let sr = self.new_shadow_root(h, mode, flags);
        Ok(self.id_of(sr))
    }

    /// The user-agent shadow root of `host` (closed; script cannot reach it):
    /// SVG `<use>` element instances. Made on first use, on any element.
    pub fn ua_shadow_root(&mut self, host: NodeId) -> DomResult<NodeId> {
        let h = self.check(host)?;
        let existing = self.n(h).element().ok_or(DomError::Type("not an element"))?.shadow_root;
        if existing != NIL {
            return Ok(self.id_of(existing));
        }
        let sr = self.new_shadow_root(h, ShadowRootMode::Closed, crate::node::shadow_flags::USER_AGENT);
        Ok(self.id_of(sr))
    }

    fn new_shadow_root(&mut self, h: u32, mode: ShadowRootMode, mut flags: u8) -> u32 {
        // A custom host makes it available to its internals; a
        // precustomized one (its constructor running) is the bindings' to
        // tell, with the flag.
        if self.n(h).element().is_some_and(|e| e.flags & element_flags::CUSTOM_DEFINED != 0) {
            flags |= shadow_flags::AVAILABLE_TO_INTERNALS;
        }
        let owner = self.n(h).owner_doc;
        let sr = self.alloc(NodeKind::Fragment(FragmentData::new(h, Some(mode), flags)), owner);
        self.set_registry_idx(sr, self.registry_idx(owner));
        let tree = self.shadow_root_flags_for(h);
        self.n_mut(sr).set_tree_flags(tree);
        self.n_mut(h).element_mut().unwrap().shadow_root = sr;
        self.shadow_roots = self.shadow_roots.saturating_add(1);
        self.bump();
        self.bump_style_text();
        sr
    }

    /// The parser's `<template shadowrootmode>`: `None` leaves an ordinary
    /// template (not a valid host, or already a shadow host of any kind).
    pub(crate) fn attach_declarative_shadow(&mut self, h: u32, mode: ShadowRootMode, flags: u8) -> Option<u32> {
        if self.n(h).element()?.shadow_root != NIL {
            return None;
        }
        let id = self.id_of(h);
        let flags = flags | shadow_flags::DECLARATIVE | shadow_flags::AVAILABLE_TO_INTERNALS;
        let sr = self.attach_shadow_with(id, mode, flags).ok()?.idx;
        let registry = if flags & shadow_flags::KEEP_REGISTRY_NULL != 0 { RegistryAssociation::Null } else { self.registry_idx(self.n(h).owner_doc) };
        self.set_registry_idx(sr, registry);
        Some(sr)
    }

    /// `shadow_flags` of a shadow root (0 for anything else).
    pub fn shadow_root_flags(&self, id: NodeId) -> DomResult<u8> {
        let i = self.check(id)?;
        Ok(match &self.n(i).kind {
            NodeKind::Fragment(f) => f.flags,
            _ => 0,
        })
    }

    /// A custom element name defined with `disabledFeatures: ["shadow"]`.
    pub fn disable_shadow_for(&mut self, name: &str) {
        self.shadow_disabled.push((None, RegistryAssociation::Default, name.into(), name.into()));
    }

    pub fn disable_shadow_for_registry(&mut self, doc: NodeId, registry: RegistryAssociation, name: &str, local: &LocalName) {
        let doc = (registry == RegistryAssociation::Default).then_some(doc);
        let key = (doc, registry, LocalName::from(name), local.clone());
        if !self.shadow_disabled.contains(&key) { self.shadow_disabled.push(key); }
    }

    /// HTML "scripting is enabled" for the nodes of `doc`: set for a document
    /// of a browsing context whose sandbox allows scripts, and to the same
    /// value as the `scripting` its `DocumentParser` gets.
    pub fn set_document_scripting(&mut self, doc: NodeId, scripting: bool) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.scripting = scripting,
            _ => return Err(DomError::Type("not a document")),
        }
        Ok(())
    }
    /// Whether scripting is enabled for the nodes of `doc` (see `set_document_scripting`).
    pub fn document_scripting(&self, doc: NodeId) -> DomResult<bool> {
        let i = self.check(doc)?;
        match &self.n(i).kind {
            NodeKind::Document(d) => Ok(d.scripting),
            _ => Err(DomError::Type("not a document")),
        }
    }
    /// `document_scripting` of the document at index `doc`.
    pub(crate) fn scripting_of(&self, doc: u32) -> bool {
        self.doc_data(doc).scripting
    }
    pub fn set_allow_declarative_shadow_roots(&mut self, doc: NodeId, allow: bool) -> DomResult<()> {
        let d = self.owner_for(doc)?;
        self.doc_data_mut(d).allow_declarative_shadow_roots = allow;
        Ok(())
    }
    /// Makes the new, empty document `doc` the fragment parser's host:
    /// not connected (see `DocumentData::fragment_parse_host`).
    pub(crate) fn make_fragment_parse_host(&mut self, doc: u32) {
        debug_assert_eq!(self.n(doc).first_child, NIL, "an empty document");
        self.doc_data_mut(doc).fragment_parse_host = true;
        self.n_mut(doc).set_tree_flags(0);
    }
    /// Whether `doc` is the fragment parser's host document.
    pub(crate) fn is_fragment_parse_host(&self, doc: u32) -> bool {
        self.doc_data(doc).fragment_parse_host
    }
    pub(crate) fn allows_declarative_shadow_roots(&self, doc: u32) -> bool {
        self.doc_data(doc).allow_declarative_shadow_roots
    }

    // ----- slots -------------------------------------------------------

    pub(crate) fn is_slot(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.name.local == local_name!("slot") && e.is_html())
    }

    /// Whether `i`'s root is a shadow root (a bit kept by `tree_scope`).
    #[inline]
    /// Whether `i` is in a user-agent shadow tree (a `<use>` instance), at
    /// any depth of shadow trees.
    pub(crate) fn in_ua_shadow_tree(&self, mut i: u32) -> bool {
        while self.n(i).in_shadow_tree() {
            let root = self.root_idx(i);
            match &self.n(root).kind {
                NodeKind::Fragment(f) if f.flags & crate::node::shadow_flags::USER_AGENT != 0 => return true,
                NodeKind::Fragment(f) if f.host != NIL => i = f.host,
                _ => return false,
            }
        }
        false
    }

    pub(crate) fn in_shadow_tree(&self, i: u32) -> bool {
        let r = self.n(i).in_shadow_tree();
        debug_assert_eq!(r, matches!(&self.n(self.root_idx(i)).kind, NodeKind::Fragment(f) if f.shadow.is_some()));
        r
    }

    /// DOM "find a slot": by name, or (a root assigning manually) the slot whose manually
    /// assigned nodes hold `i` (`HTMLSlotElement.assign()`).
    pub(crate) fn find_slot(&self, i: u32, open_only: bool) -> Option<u32> {
        if !matches!(self.n(i).kind, NodeKind::Element(_) | NodeKind::Text(_)) {
            return None;
        }
        let parent = self.n(i).parent;
        if parent == NIL {
            return None;
        }
        let sr = self.n(parent).element().map_or(NIL, |e| e.shadow_root);
        if sr == NIL {
            return None;
        }
        let NodeKind::Fragment(f) = &self.n(sr).kind else { return None };
        if open_only && f.shadow == Some(ShadowRootMode::Closed) {
            return None;
        }
        if f.flags & shadow_flags::MANUAL_SLOTS != 0 {
            let id = self.id_of(i);
            return self.slot_state.manual.iter().find_map(|(slot, list)| {
                let s = self.check(*slot).ok()?;
                (list.contains(&id) && self.is_slot(s) && self.root_idx(s) == sr).then_some(s)
            });
        }
        let name = self.n(i).element().and_then(|e| e.attr_local(&local_name!("slot"))).unwrap_or("");
        let mut cur = self.n(sr).first_child;
        while cur != NIL {
            if self.is_slot(cur)
                && self.n(cur).element().and_then(|e| e.attr_local(&local_name!("name"))).unwrap_or("") == name
            {
                return Some(cur);
            }
            cur = self.following_in(cur, sr);
        }
        None
    }

    /// Those of `kids` (children of the shadow host `host`) no slot of its shadow tree takes, as
    /// [`Self::find_slot`] finds them, with one walk of the shadow tree for all of them.
    pub(crate) fn unassigned_children(&self, host: u32, kids: Vec<u32>) -> Vec<u32> {
        let sr = self.n(host).element().map_or(NIL, |e| e.shadow_root);
        let NodeKind::Fragment(f) = (if sr == NIL { return kids } else { &self.n(sr).kind }) else { return kids };
        if f.flags & shadow_flags::MANUAL_SLOTS != 0 {
            return kids.into_iter().filter(|&k| self.find_slot(k, false).is_none()).collect();
        }
        let mut names: rustc_hash::FxHashSet<&str> = rustc_hash::FxHashSet::default();
        let mut cur = self.n(sr).first_child;
        while cur != NIL {
            if self.is_slot(cur) {
                names.insert(self.n(cur).element().and_then(|e| e.attr_local(&local_name!("name"))).unwrap_or(""));
            }
            cur = self.following_in(cur, sr);
        }
        kids.into_iter()
            .filter(|&k| {
                !matches!(self.n(k).kind, NodeKind::Element(_) | NodeKind::Text(_))
                    || !names.contains(self.n(k).element().and_then(|e| e.attr_local(&local_name!("slot"))).unwrap_or(""))
            })
            .collect()
    }

    /// `Slottable.assignedSlot`.
    pub fn assigned_slot(&self, node: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(node)?;
        Ok(self.find_slot(i, true).map(|s| self.id_of(s)))
    }

    /// The slot `node` is assigned to, in an open or a closed shadow root: its parent in the flat
    /// tree (DOM "find a slot" with open false), for engine walks that script does not see.
    pub(crate) fn flat_tree_slot(&self, node: NodeId) -> Option<NodeId> {
        let i = self.check(node).ok()?;
        self.find_slot(i, false).map(|s| self.id_of(s))
    }

    pub(crate) fn slot_assigned(&self, slot: u32) -> Vec<u32> {
        let host = match &self.n(self.root_idx(slot)).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() && f.flags & shadow_flags::MANUAL_SLOTS != 0 => {
                // DOM "find slottables" of a manually assigning root: the slot's manually
                // assigned nodes that are children of the host, in the order they were given.
                let Some(list) = self.slot_state.manual.get(&self.id_of(slot)) else { return Vec::new() };
                return list
                    .iter()
                    .filter_map(|&n| self.check(n).ok())
                    .filter(|&n| {
                        self.n(n).parent == f.host && matches!(self.n(n).kind, NodeKind::Element(_) | NodeKind::Text(_))
                    })
                    .collect();
            }
            NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
            _ => return Vec::new(),
        };
        self.child_idx_iter(host).filter(|&c| self.find_slot(c, false) == Some(slot)).collect()
    }

    /// DOM "find flattened slottables".
    fn flatten_slot(&self, slot: u32, out: &mut Vec<u32>) {
        if !self.in_shadow_tree(slot) {
            return;
        }
        let assigned = self.slot_assigned(slot);
        let items: Vec<u32> = if assigned.is_empty() {
            self.child_idx_iter(slot)
                .filter(|&c| matches!(self.n(c).kind, NodeKind::Element(_) | NodeKind::Text(_)))
                .collect()
        } else {
            assigned
        };
        for c in items {
            if self.is_slot(c) && self.in_shadow_tree(c) {
                self.flatten_slot(c, out);
            } else {
                out.push(c);
            }
        }
    }

    /// `HTMLSlotElement.assignedNodes({flatten})`.
    pub fn assigned_nodes(&self, slot: NodeId, flatten: bool) -> DomResult<Vec<NodeId>> {
        let i = self.check(slot)?;
        let nodes = if flatten {
            let mut out = Vec::new();
            self.flatten_slot(i, &mut out);
            out
        } else {
            self.slot_assigned(i)
        };
        Ok(nodes.into_iter().map(|n| self.id_of(n)).collect())
    }

    pub fn shadow_root(&self, host: NodeId) -> DomResult<Option<NodeId>> {
        let h = self.check(host)?;
        Ok(self.n(h).element().and_then(|e| self.opt_id(e.shadow_root)))
    }
    pub fn template_contents(&self, template: NodeId) -> DomResult<Option<NodeId>> {
        let t = self.check(template)?;
        Ok(self.n(t).element().and_then(|e| self.opt_id(e.template_contents)))
    }
    /// Host of a shadow root or template-contents fragment.
    pub fn fragment_host(&self, frag: NodeId) -> DomResult<Option<NodeId>> {
        let f = self.check(frag)?;
        Ok(match &self.n(f).kind {
            NodeKind::Fragment(fd) => self.opt_id(fd.host),
            _ => None,
        })
    }
    pub fn shadow_root_mode(&self, id: NodeId) -> DomResult<Option<ShadowRootMode>> {
        let i = self.check(id)?;
        Ok(match &self.n(i).kind {
            NodeKind::Fragment(f) => f.shadow,
            _ => None,
        })
    }
    pub fn is_shadow_root(&self, id: NodeId) -> DomResult<bool> {
        let i = self.check(id)?;
        Ok(matches!(&self.n(i).kind, NodeKind::Fragment(f) if f.shadow.is_some()))
    }

    // ----- navigation --------------------------------------------------

    pub fn node_type(&self, id: NodeId) -> DomResult<NodeType> {
        Ok(self.node(id)?.kind.node_type())
    }
    pub fn parent(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.n(i).parent))
    }
    pub fn parent_element(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        let p = self.n(i).parent;
        Ok((p != NIL && self.n(p).is_element()).then(|| self.id_of(p)))
    }
    pub fn first_child(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.n(i).first_child))
    }
    pub fn last_child(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.n(i).last_child))
    }
    pub fn next_sibling(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.n(i).next))
    }
    pub fn prev_sibling(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(self.opt_id(self.n(i).prev))
    }
    pub fn child_count(&self, id: NodeId) -> DomResult<u32> {
        let i = self.check(id)?;
        Ok(self.n(i).child_count())
    }
    pub fn owner_document(&self, id: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        Ok(match self.n(i).kind {
            NodeKind::Document(_) => None,
            _ => Some(self.id_of(self.n(i).owner_doc)),
        })
    }
    /// Node document, including for documents themselves.
    pub fn node_document(&self, id: NodeId) -> DomResult<NodeId> {
        let i = self.check(id)?;
        Ok(self.id_of(self.n(i).owner_doc))
    }

    pub fn children(&self, id: NodeId) -> DomResult<ChildIter<'_>> {
        let i = self.check(id)?;
        Ok(ChildIter { dom: self, cur: self.n(i).first_child })
    }
    pub(crate) fn child_idx_iter(&self, idx: u32) -> impl Iterator<Item = u32> + '_ {
        let mut cur = self.n(idx).first_child;
        std::iter::from_fn(move || {
            if cur == NIL {
                return None;
            }
            let c = cur;
            cur = self.n(c).next;
            Some(c)
        })
    }
    pub fn element_children(&self, id: NodeId) -> DomResult<impl Iterator<Item = NodeId> + '_> {
        let i = self.check(id)?;
        Ok(self.child_idx_iter(i).filter(|&c| self.n(c).is_element()).map(|c| self.id_of(c)))
    }

    /// `childNodes[index]`: O(min(index, len-index)) walking from the nearer end.
    pub fn child_at(&self, id: NodeId, index: u32) -> DomResult<Option<NodeId>> {
        let i = self.check(id)?;
        let n = self.n(i);
        if index >= n.child_count() {
            return Ok(None);
        }
        if index <= n.child_count() / 2 {
            let mut c = n.first_child;
            for _ in 0..index {
                c = self.n(c).next;
            }
            Ok(Some(self.id_of(c)))
        } else {
            let mut c = n.last_child;
            for _ in 0..(n.child_count() - 1 - index) {
                c = self.n(c).prev;
            }
            Ok(Some(self.id_of(c)))
        }
    }

    /// Index of `id` among its siblings.
    pub fn index_in_parent(&self, id: NodeId) -> DomResult<u32> {
        let i = self.check(id)?;
        let mut k = 0;
        let mut c = self.n(i).prev;
        while c != NIL {
            k += 1;
            c = self.n(c).prev;
        }
        Ok(k)
    }

    /// Pre-order successor of `idx` within the subtree rooted at `root`
    /// (light tree only; template contents and shadow trees excluded).
    #[inline]
    pub(crate) fn following_in(&self, idx: u32, root: u32) -> u32 {
        let n = self.n(idx);
        if n.first_child != NIL {
            return n.first_child;
        }
        let mut cur = idx;
        while cur != root {
            let c = self.n(cur);
            if c.next != NIL {
                return c.next;
            }
            cur = c.parent;
            if cur == NIL {
                break;
            }
        }
        NIL
    }

    /// The node after `idx` in `root`'s shadow-including subtree, in
    /// shadow-including tree order with an element's shadow tree before its
    /// children (the order of post-connection steps), or NIL. Follows the
    /// tree's own links, so a walk needs no stack.
    pub(crate) fn next_shadow_including(&self, idx: u32, root: u32) -> u32 {
        let n = self.n(idx);
        if let NodeKind::Element(e) = &n.kind {
            if e.shadow_root != NIL {
                return e.shadow_root;
            }
        }
        if n.first_child != NIL {
            return n.first_child;
        }
        self.climb_shadow_including(idx, root, false)
    }

    /// [`Dom::next_shadow_including`] reading every child list from its last
    /// child to its first (an element's shadow tree still comes first): the
    /// order of a walk that pushes each node's children onto a stack in order.
    pub(crate) fn next_shadow_including_rev(&self, idx: u32, root: u32) -> u32 {
        let n = self.n(idx);
        if let NodeKind::Element(e) = &n.kind {
            if e.shadow_root != NIL {
                return e.shadow_root;
            }
        }
        if n.last_child != NIL {
            return n.last_child;
        }
        self.climb_shadow_including(idx, root, true)
    }

    /// [`Dom::next_shadow_including`] with an element's shadow tree after
    /// its children instead of before them.
    pub(crate) fn next_shadow_including_light_first(&self, idx: u32, root: u32) -> u32 {
        let n = self.n(idx);
        if n.first_child != NIL {
            return n.first_child;
        }
        if let NodeKind::Element(e) = &n.kind {
            if e.shadow_root != NIL {
                return e.shadow_root;
            }
        }
        let mut cur = idx;
        loop {
            if cur == root {
                return NIL;
            }
            let n = self.n(cur);
            if n.next != NIL {
                return n.next;
            }
            if n.parent != NIL {
                // The parent's children are done: its shadow tree is next.
                cur = n.parent;
                if let NodeKind::Element(e) = &self.n(cur).kind {
                    if e.shadow_root != NIL {
                        return e.shadow_root;
                    }
                }
                continue;
            }
            // A shadow root is done, and with it its host.
            let NodeKind::Fragment(FragmentData { host, shadow: Some(_), .. }) = &n.kind else { return NIL };
            cur = *host;
        }
    }

    /// The next node once `cur`'s subtree is done (see `next_shadow_including`).
    fn climb_shadow_including(&self, mut cur: u32, root: u32, rev: bool) -> u32 {
        loop {
            if cur == root {
                return NIL;
            }
            let n = self.n(cur);
            let sibling = if rev { n.prev } else { n.next };
            if sibling != NIL {
                return sibling;
            }
            if n.parent != NIL {
                cur = n.parent;
                continue;
            }
            // A shadow root is done: its host's children come next.
            let NodeKind::Fragment(FragmentData { host, shadow: Some(_), .. }) = &n.kind else { return NIL };
            let host = *host;
            let h = self.n(host);
            let first = if rev { h.last_child } else { h.first_child };
            if first != NIL {
                return first;
            }
            cur = host;
        }
    }

    /// Pre-order iterator over the inclusive descendants of `id`.
    pub fn descendants(&self, id: NodeId) -> DomResult<Descendants<'_>> {
        let i = self.check(id)?;
        Ok(Descendants { dom: self, root: i, next: i })
    }

    pub(crate) fn root_idx(&self, mut idx: u32) -> u32 {
        loop {
            let p = self.n(idx).parent;
            if p == NIL {
                return idx;
            }
            idx = p;
        }
    }
    /// `getRootNode({composed})`.
    pub fn root_node(&self, id: NodeId, composed: bool) -> DomResult<NodeId> {
        let mut i = self.check(id)?;
        loop {
            i = self.root_idx(i);
            if composed {
                if let NodeKind::Fragment(FragmentData { host, shadow: Some(_), .. }) = &self.n(i).kind {
                    i = *host;
                    continue;
                }
            }
            return Ok(self.id_of(i));
        }
    }
    /// Shadow-including connectedness: a bit `tree_scope` keeps.
    #[inline]
    pub(crate) fn is_connected_idx(&self, i: u32) -> bool {
        let r = self.n(i).is_connected();
        debug_assert_eq!(r, self.connected_by_walk(i));
        r
    }

    /// Connected to a document that a browsing context currently displays
    /// (a connected node's node document is the document it is in).
    pub fn is_in_active_document(&self, id: NodeId) -> bool {
        let Ok((_, n)) = self.live(id) else { return false };
        n.is_connected() && self.doc_data(n.owner_doc).active
    }

    /// Connected, in the document's own tree (not in a shadow tree): two
    /// bits `tree_scope` keeps, no walk.
    pub fn in_document_tree(&self, id: NodeId) -> bool {
        let Ok((i, n)) = self.live(id) else { return false };
        n.is_connected() && !self.in_shadow_tree(i)
    }

    /// `Node.isConnected` (shadow-including).
    /// pyro: whether every change since `version` was made outside the document tree: to
    /// nodes, or the child lists of nodes, that are not connected to a document (a clone a
    /// screenshot library styles, a fragment being built). Such changes cannot reach layout or
    /// computed styles, so the layout engine need not sync for them. `false` when the journal no
    /// longer covers `version`, or for unannotated changes and focus/hover changes. A node that
    /// was connected when it changed and is detached now was removed, which shows as a removal
    /// from a connected parent; one detached when it changed and connected since was inserted,
    /// which shows as an insertion into a connected parent: both answer `false`.
    pub fn only_detached_changes_since(&self, version: u64) -> bool {
        let Some(changes) = self.changes_since(version) else { return false };
        let detached = |i: u32| self.slot_live(i) && !self.n(i).is_connected();
        for change in changes {
            let outside = match change {
                Change::Attr(i, _, _) | Change::State(i) | Change::Text(i) => detached(i),
                Change::Inserted(parent, _) | Change::Removed(parent, _, _) => detached(parent),
                Change::Pseudo | Change::Unknown(_) => false,
            };
            if !outside {
                return false;
            }
        }
        true
    }

    pub fn is_connected(&self, id: NodeId) -> DomResult<bool> {
        let (i, n) = self.live(id)?;
        debug_assert_eq!(n.is_connected(), self.connected_by_walk(i));
        Ok(n.is_connected())
    }

    pub(crate) fn is_inclusive_ancestor(&self, anc: u32, mut node: u32) -> bool {
        while node != NIL {
            if node == anc {
                return true;
            }
            node = self.n(node).parent;
        }
        false
    }
    pub(crate) fn is_host_including_inclusive_ancestor(&self, anc: u32, mut node: u32) -> bool {
        loop {
            if self.is_inclusive_ancestor(anc, node) {
                return true;
            }
            let root = self.root_idx(node);
            match &self.n(root).kind {
                NodeKind::Fragment(f) if f.host != NIL => node = f.host,
                _ => return false,
            }
        }
    }
    /// `Node.contains(other)`.
    pub fn contains(&self, id: NodeId, other: NodeId) -> DomResult<bool> {
        let a = self.check(id)?;
        let b = self.check(other)?;
        Ok(self.is_inclusive_ancestor(a, b))
    }

    pub fn compare_document_position(&self, reference: NodeId, other: NodeId) -> DomResult<u16> {
        use position::*;
        let a = self.check(reference)?;
        let b = self.check(other)?;
        if a == b {
            return Ok(0);
        }
        // An attribute stands where its element does, just inside it.
        let (attr_a, attr_b) = (self.attr_data(a), self.attr_data(b));
        if attr_a.is_some() || attr_b.is_some() {
            let (node_a, node_b) = (attr_a.map_or(a, |x| x.owner), attr_b.map_or(b, |x| x.owner));
            if let (Some(x), Some(y)) = (attr_a, attr_b) {
                if node_a == node_b && node_a != NIL {
                    let attrs = &self.n(node_a).element().expect("attribute owner").attrs;
                    let at = |d: &AttrNodeData| {
                        attrs.iter().position(|v| v.name.ns == d.name.ns && v.name.local == d.name.local)
                    };
                    let dir = if at(y) < at(x) { PRECEDING } else { FOLLOWING };
                    return Ok(IMPLEMENTATION_SPECIFIC | dir);
                }
            }
            if node_a == NIL || node_b == NIL || self.root_idx(node_a) != self.root_idx(node_b) {
                let dir = if a < b { PRECEDING } else { FOLLOWING };
                return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC | dir);
            }
            if node_a == node_b {
                // Exactly one of the two is an attribute of the other.
                return Ok(if attr_a.is_some() { CONTAINS | PRECEDING } else { CONTAINED_BY | FOLLOWING });
            }
            if attr_b.is_none() && self.is_inclusive_ancestor(node_b, node_a) {
                return Ok(CONTAINS | PRECEDING);
            }
            if attr_a.is_none() && self.is_inclusive_ancestor(node_a, node_b) {
                return Ok(CONTAINED_BY | FOLLOWING);
            }
            return Ok(if self.tree_order_before(node_b, node_a) { PRECEDING } else { FOLLOWING });
        }
        let ra = self.root_idx(a);
        let rb = self.root_idx(b);
        if ra != rb {
            let dir = if ra < rb { PRECEDING } else { FOLLOWING };
            return Ok(DISCONNECTED | IMPLEMENTATION_SPECIFIC | dir);
        }
        if self.is_inclusive_ancestor(b, a) {
            return Ok(CONTAINS | PRECEDING);
        }
        if self.is_inclusive_ancestor(a, b) {
            return Ok(CONTAINED_BY | FOLLOWING);
        }
        if self.tree_order_before(b, a) {
            Ok(PRECEDING)
        } else {
            Ok(FOLLOWING)
        }
    }

    /// True if `x` precedes `y` in tree order, or is `y` (same tree assumed;
    /// false for nodes of different trees). Climbs to the children of the
    /// common ancestor without allocating, then walks their siblings both
    /// ways from `x`'s side, so the walk stops at whichever of `y`'s side or
    /// a list end comes first.
    pub(crate) fn tree_order_before(&self, x: u32, y: u32) -> bool {
        let depth = |mut n: u32| {
            let mut d = 0usize;
            while n != NIL {
                d += 1;
                n = self.n(n).parent;
            }
            d
        };
        let (dx, dy) = (depth(x), depth(y));
        let (mut a, mut b) = (x, y);
        for _ in dy..dx {
            a = self.n(a).parent;
        }
        for _ in dx..dy {
            b = self.n(b).parent;
        }
        if a == b {
            // One is an inclusive ancestor of the other.
            return dx <= dy;
        }
        loop {
            let (pa, pb) = (self.n(a).parent, self.n(b).parent);
            if pa == pb {
                break;
            }
            a = pa;
            b = pb;
        }
        if self.n(a).parent == NIL {
            return false; // different trees
        }
        // Siblings `a` and `b`.
        let (mut fwd, mut back) = (self.n(a).next, self.n(a).prev);
        loop {
            if fwd == b {
                return true;
            }
            if back == b || fwd == NIL {
                return false;
            }
            if back == NIL {
                return true;
            }
            fwd = self.n(fwd).next;
            back = self.n(back).prev;
        }
    }

    // ----- tree mutation -----------------------------------------------

    fn ensure_pre_insert_validity(&self, node: u32, parent: u32, child: u32) -> DomResult<()> {
        let pk = &self.n(parent).kind;
        if !matches!(pk, NodeKind::Document(_) | NodeKind::Fragment(_) | NodeKind::Element(_)) {
            return Err(hierarchy("parent cannot have children"));
        }
        if self.is_host_including_inclusive_ancestor(node, parent) {
            return Err(hierarchy("the new child contains the parent"));
        }
        if child != NIL && self.n(child).parent != parent {
            return Err(not_found("the reference node is not a child of this node"));
        }
        let nk = &self.n(node).kind;
        match nk {
            NodeKind::Document(_) => return Err(hierarchy("cannot insert a document")),
            NodeKind::Attr(_) => return Err(hierarchy("cannot insert an attribute")),
            NodeKind::Doctype(_) if !matches!(pk, NodeKind::Document(_)) => {
                return Err(hierarchy("doctype must be a child of a document"))
            }
            NodeKind::Text(_) | NodeKind::CData(_) if matches!(pk, NodeKind::Document(_)) => {
                return Err(hierarchy("text cannot be a child of a document"))
            }
            _ => {}
        }
        if matches!(pk, NodeKind::Document(_)) {
            self.check_document_children(node, parent, child, NIL)?;
        }
        Ok(())
    }

    /// Document child constraints shared by insert and replace. `replaced` is
    /// the child being replaced (ignored when counting existing children).
    fn check_document_children(&self, node: u32, parent: u32, child: u32, replaced: u32) -> DomResult<()> {
        let has_element = self.child_idx_iter(parent).any(|c| c != replaced && self.n(c).is_element());
        let has_doctype =
            self.child_idx_iter(parent).any(|c| c != replaced && matches!(self.n(c).kind, NodeKind::Doctype(_)));
        let doctype_following = |from: u32| {
            let mut c = from;
            while c != NIL {
                if matches!(self.n(c).kind, NodeKind::Doctype(_)) {
                    return true;
                }
                c = self.n(c).next;
            }
            false
        };
        let element_preceding = |from: u32| {
            let mut c = self.n(from).prev;
            while c != NIL {
                if c != replaced && self.n(c).is_element() {
                    return true;
                }
                c = self.n(c).prev;
            }
            false
        };
        let follows =
            |c: u32| if c == NIL { false } else { doctype_following(if c == replaced { self.n(c).next } else { c }) };
        match &self.n(node).kind {
            NodeKind::Fragment(_) => {
                let mut elems = 0;
                for c in self.child_idx_iter(node) {
                    match self.n(c).kind {
                        NodeKind::Element(_) => elems += 1,
                        NodeKind::Text(_) | NodeKind::CData(_) => {
                            return Err(hierarchy("text cannot be a child of a document"))
                        }
                        _ => {}
                    }
                }
                if elems > 1 {
                    return Err(hierarchy("document can have only one element child"));
                }
                if elems == 1
                    && (has_element
                        || (child != NIL && matches!(self.n(child).kind, NodeKind::Doctype(_)) && child != replaced)
                        || follows(child))
                {
                    return Err(hierarchy("document can have only one element child"));
                }
            }
            NodeKind::Element(_) => {
                if has_element
                    || (child != NIL && child != replaced && matches!(self.n(child).kind, NodeKind::Doctype(_)))
                    || follows(child)
                {
                    return Err(hierarchy("document can have only one element child"));
                }
            }
            NodeKind::Doctype(_)
                if (has_doctype || (child != NIL && element_preceding(child)) || (child == NIL && has_element)) =>
            {
                return Err(hierarchy("document can have only one doctype"));
            }
            _ => {}
        }
        Ok(())
    }

    /// "Ensure pre-insert validity" alone (`Range.insertNode` checks before it splits).
    pub fn check_pre_insert(&self, parent: NodeId, node: NodeId, child: Option<NodeId>) -> DomResult<()> {
        let c = match child {
            Some(c) => self.check(c)?,
            None => NIL,
        };
        self.ensure_pre_insert_validity(self.check(node)?, self.check(parent)?, c)
    }

    /// `parent.insertBefore(node, child)`.
    pub fn insert_before(&mut self, parent: NodeId, node: NodeId, child: Option<NodeId>) -> DomResult<NodeId> {
        let p = self.check(parent)?;
        let n = self.check(node)?;
        let c = match child {
            Some(c) => self.check(c)?,
            None => NIL,
        };
        self.ensure_pre_insert_validity(n, p, c)?;
        let mut reference = c;
        if reference == n {
            reference = self.n(n).next;
        }
        self.insert(n, p, reference, false);
        Ok(node)
    }

    pub fn append_child(&mut self, parent: NodeId, node: NodeId) -> DomResult<NodeId> {
        self.insert_before(parent, node, None)
    }

    /// `parent.moveBefore(node, child)`: DOM "move" (pre-move validity, then
    /// the state-preserving move).
    pub fn move_before(&mut self, parent: NodeId, node: NodeId, child: Option<NodeId>) -> DomResult<()> {
        let p = self.check(parent)?;
        let n = self.check(node)?;
        let c = match child {
            Some(c) => self.check(c)?,
            None => NIL,
        };
        // A move never connects or disconnects anything: both ends share one
        // shadow-including root, connected or not.
        if self.shadow_including_root_idx(p) != self.shadow_including_root_idx(n) {
            return Err(hierarchy("the node and the parent must share a shadow-including root"));
        }
        if !matches!(self.n(p).kind, NodeKind::Document(_) | NodeKind::Fragment(_) | NodeKind::Element(_)) {
            return Err(hierarchy("parent cannot have children"));
        }
        if self.is_host_including_inclusive_ancestor(n, p) {
            return Err(hierarchy("the new child contains the parent"));
        }
        if c != NIL && self.n(c).parent != p {
            return Err(not_found("the reference node is not a child of this node"));
        }
        match &self.n(n).kind {
            NodeKind::Element(_) | NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) => {}
            NodeKind::Text(_) | NodeKind::CData(_) => {
                if matches!(self.n(p).kind, NodeKind::Document(_)) {
                    return Err(hierarchy("text cannot be a child of a document"));
                }
            }
            _ => return Err(hierarchy("only elements and character data can be moved")),
        }
        if matches!(self.n(p).kind, NodeKind::Document(_)) {
            self.check_document_children(n, p, c, NIL)?;
        }
        let mut reference = c;
        if reference == n {
            reference = self.n(n).next;
        }
        self.move_node(n, p, reference);
        Ok(())
    }

    /// DOM "move" after validation: `node` goes into `parent` before `child`
    /// without leaving its shadow-including root. Unlike `remove` + `insert`
    /// the focused element stays focused, iframes keep their documents,
    /// modal dialogs and popovers stay open, and custom elements get one
    /// `connectedMoveCallback` reaction instead of a disconnect and a connect.
    fn move_node(&mut self, node: u32, parent: u32, child: u32) {
        let old_parent = self.n(node).parent;
        debug_assert!(old_parent != NIL);
        let old_prev = self.n(node).prev;
        let old_next = self.n(node).next;
        self.resources_before_move(node);
        self.options_before_remove(node);
        if !self.live_ranges.is_empty() {
            self.ranges_before_remove(node);
        }
        if !self.live_iterators.is_empty() {
            self.iterators_before_remove(node);
        }
        self.unlink(node);
        self.bump_tree();
        self.note(Change::Removed(old_parent, node, old_next));
        if child != NIL && !self.live_ranges.is_empty() {
            self.ranges_before_insert(parent, child, 1);
        }
        let new_prev = if child == NIL { self.n(parent).last_child } else { self.n(child).prev };
        self.link(node, parent, child);
        self.slots_after_move(old_parent, parent, node);
        self.resources_after_move(node, old_parent);
        self.bump_tree();
        self.note(Change::Inserted(parent, node));
        self.announce_named_items(node);
        if self.is_connected_idx(parent) {
            self.enqueue_custom_reactions(node, crate::custom::CustomReaction::Moved);
        }
        self.queue_child_list(old_parent, &[], &[node], old_prev, old_next);
        self.queue_child_list(parent, &[node], &[], new_prev, child);
    }

    /// The shadow-including root: a shadow root's host's root, recursively.
    fn shadow_including_root_idx(&self, idx: u32) -> u32 {
        let mut r = self.root_idx(idx);
        loop {
            match &self.n(r).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => r = self.root_idx(f.host),
                _ => return r,
            }
        }
    }

    /// `parent.removeChild(child)`.
    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> DomResult<NodeId> {
        let p = self.check(parent)?;
        let c = self.check(child)?;
        if self.n(c).parent != p {
            return Err(not_found("the node to be removed is not a child of this node"));
        }
        self.remove(c, false);
        Ok(child)
    }

    /// `ChildNode.remove()`.
    pub fn remove_node(&mut self, node: NodeId) -> DomResult<()> {
        let n = self.check(node)?;
        if self.n(n).parent != NIL {
            self.remove(n, false);
        }
        Ok(())
    }

    /// `parent.replaceChild(node, child)`.
    pub fn replace_child(&mut self, parent: NodeId, node: NodeId, child: NodeId) -> DomResult<NodeId> {
        let p = self.check(parent)?;
        let n = self.check(node)?;
        let c = self.check(child)?;
        let pk = &self.n(p).kind;
        if !matches!(pk, NodeKind::Document(_) | NodeKind::Fragment(_) | NodeKind::Element(_)) {
            return Err(hierarchy("parent cannot have children"));
        }
        if self.is_host_including_inclusive_ancestor(n, p) {
            return Err(hierarchy("the new child contains the parent"));
        }
        if self.n(c).parent != p {
            return Err(not_found("the node to be replaced is not a child of this node"));
        }
        match &self.n(n).kind {
            NodeKind::Document(_) => return Err(hierarchy("cannot insert a document")),
            NodeKind::Attr(_) => return Err(hierarchy("cannot insert an attribute")),
            NodeKind::Doctype(_) if !matches!(pk, NodeKind::Document(_)) => {
                return Err(hierarchy("doctype must be a child of a document"))
            }
            NodeKind::Text(_) | NodeKind::CData(_) if matches!(pk, NodeKind::Document(_)) => {
                return Err(hierarchy("text cannot be a child of a document"))
            }
            _ => {}
        }
        if matches!(self.n(p).kind, NodeKind::Document(_)) {
            self.check_document_children(n, p, c, c)?;
        }
        let mut reference = self.n(c).next;
        if reference == n {
            reference = self.n(n).next;
        }
        // Adopt the replacement before recording the old child's siblings:
        // it may itself be the previous sibling of the replaced node.
        if self.n(n).parent != NIL {
            self.remove(n, false);
        }
        let previous = self.n(c).prev;
        let mut removed = Vec::new();
        if self.n(c).parent != NIL {
            removed.push(c);
            self.remove(c, true);
        }
        let added: Vec<u32> = match self.n(n).kind {
            NodeKind::Fragment(_) => self.child_idx_iter(n).collect(),
            _ => vec![n],
        };
        self.insert(n, p, reference, true);
        self.queue_child_list(p, &added, &removed, previous, reference);
        Ok(child)
    }

    /// Replaces all children of `parent` with `node` (`textContent=`, `innerHTML=`, `replaceChildren`).
    pub fn replace_all(&mut self, parent: NodeId, node: Option<NodeId>) -> DomResult<()> {
        let p = self.check(parent)?;
        let n = match node {
            Some(n) => {
                let n = self.check(n)?;
                self.ensure_pre_insert_validity(n, p, NIL)?;
                n
            }
            None => NIL,
        };
        let removed: Vec<u32> = self.child_idx_iter(p).collect();
        // The record's list of added nodes, only when an observer may want it.
        let observed = self.mutation.any_registration();
        let added: Vec<u32> = match n {
            _ if !observed => Vec::new(),
            NIL => Vec::new(),
            n if matches!(self.n(n).kind, NodeKind::Fragment(_)) => self.child_idx_iter(n).collect(),
            n => vec![n],
        };
        for &c in &removed {
            self.remove(c, true);
        }
        if n != NIL {
            self.insert(n, p, NIL, true);
        }
        if observed && (!added.is_empty() || !removed.is_empty()) {
            self.queue_child_list(p, &added, &removed, NIL, NIL);
        }
        Ok(())
    }

    /// Core insert: `node` (or fragment children) into `parent` before `child`.
    fn insert(&mut self, node: u32, parent: u32, child: u32, suppress: bool) {
        if matches!(self.n(node).kind, NodeKind::Fragment(_)) {
            let kids: Vec<u32> = self.child_idx_iter(node).collect();
            for &k in &kids {
                self.remove(k, true);
            }
            if !kids.is_empty() {
                self.queue_child_list(node, &[], &kids, NIL, NIL);
                self.insert_nodes(&kids, parent, child, suppress);
            }
        } else {
            if self.n(node).parent != NIL {
                self.remove(node, false);
            }
            self.insert_nodes(&[node], parent, child, suppress);
        }
    }

    /// The rest of `insert` for `nodes` (not empty), already parentless.
    fn insert_nodes(&mut self, nodes: &[u32], parent: u32, child: u32, suppress: bool) {
        if child != NIL && !self.live_ranges.is_empty() {
            self.ranges_before_insert(parent, child, nodes.len() as u32);
        }
        let prev = if child == NIL { self.n(parent).last_child } else { self.n(child).prev };
        let doc = self.n(parent).owner_doc;
        for &k in nodes {
            self.adopt_idx(k, doc);
            self.link(k, parent, child);
        }
        let connected = self.is_connected_idx(parent);
        self.script_children_inserted(parent);
        for &k in nodes {
            self.resources_after_insert(k);
            self.picture_images_changed(parent, k);
            if self.internals.faces != 0 {
                self.faces_after_tree_change(k);
            }
            if connected {
                self.note_connected_listeners(k);
                self.enqueue_custom_reactions(k, crate::custom::CustomReaction::Connected);
                self.upgrades_after_insert(k);
            }
        }
        self.slots_after_child_change(parent, nodes);
        self.bump_tree();
        self.note(Change::Inserted(parent, nodes[0]));
        for &k in &nodes[1..] {
            self.note_more(Change::Inserted(parent, k));
        }
        for &k in nodes {
            self.announce_named_items(k);
        }
        if !suppress {
            self.queue_child_list(parent, nodes, &[], prev, child);
        }
        if connected {
            for &k in nodes {
                self.hide_nonces(k);
            }
        }
    }

    fn link(&mut self, node: u32, parent: u32, before: u32) {
        let prev = if before == NIL { self.n(parent).last_child } else { self.n(before).prev };
        {
            let n = self.n_mut(node);
            n.parent = parent;
            n.prev = prev;
            n.next = before;
            let w = &mut self.walk[node as usize];
            w.parent = parent;
            w.next = before;
        }
        if prev == NIL {
            self.n_mut(parent).first_child = node;
            self.walk[parent as usize].first_child = node;
        } else {
            self.n_mut(prev).next = node;
            self.walk[prev as usize].next = node;
        }
        if before == NIL {
            self.n_mut(parent).last_child = node;
        } else {
            self.n_mut(before).prev = node;
        }
        self.n_mut(parent).add_child_count();
        self.tree_linked(node);
        if !self.registry_associations.is_empty() && self.is_connected_idx(node) && !self.is_fragment_parse_host(self.n(node).owner_doc) {
            let mut i = node;
            while i != NIL {
                if let Some(&RegistryAssociation::Scoped(registry)) = self.registry_associations.get(&i) {
                    self.note_registry_connection(registry, self.id_of(self.n(i).owner_doc));
                }
                i = self.next_shadow_including(i, node);
            }
        }
    }

    /// Every unlink (DOM, parsers, moves) goes through here and every link
    /// through `link`, which keep `tree_scope`'s flags and id maps.
    fn unlink(&mut self, node: u32) {
        self.tree_unlinking(node);
        let (parent, prev, next) = {
            let n = self.n(node);
            (n.parent, n.prev, n.next)
        };
        if prev == NIL {
            self.n_mut(parent).first_child = next;
            self.walk[parent as usize].first_child = next;
        } else {
            self.n_mut(prev).next = next;
            self.walk[prev as usize].next = next;
        }
        if next == NIL {
            self.n_mut(parent).last_child = prev;
        } else {
            self.n_mut(next).prev = prev;
        }
        self.n_mut(parent).sub_child_count();
        let n = self.n_mut(node);
        n.parent = NIL;
        n.prev = NIL;
        n.next = NIL;
        let w = &mut self.walk[node as usize];
        w.parent = NIL;
        w.next = NIL;
    }

    fn remove(&mut self, node: u32, suppress: bool) {
        let parent = self.n(node).parent;
        debug_assert!(parent != NIL);
        let prev = self.n(node).prev;
        let next = self.n(node).next;
        self.resources_before_remove(node);
        self.picture_images_changed(parent, node);
        self.options_before_remove(node);
        self.top_layer_before_remove(node);
        let disconnect = self.reactions.any_custom && self.is_connected_idx(parent);
        self.note_removal(node);
        self.add_transient_observers(parent, node);
        if !self.live_ranges.is_empty() {
            self.ranges_before_remove(node);
        }
        if !self.live_iterators.is_empty() {
            self.iterators_before_remove(node);
        }
        self.unlink(node);
        // Removing steps (form owners) come before disconnectedCallback.
        if self.internals.faces != 0 {
            self.faces_after_tree_change(node);
        }
        if disconnect {
            self.enqueue_custom_reactions(node, crate::custom::CustomReaction::Disconnected);
        }
        self.slots_after_child_change(parent, &[node]);
        self.unfocus_removed(node);
        self.bump_tree();
        self.note(Change::Removed(parent, node, next));
        if !suppress {
            self.queue_child_list(parent, &[], &[node], prev, next);
        }
        self.candidates.push(node);
    }

    /// HTML's focus fixup rule: when the focused element leaves the
    /// document (it is `node` or inside it, already unlinked), the document
    /// has no focused element any more (`activeElement` is the body). No
    /// blur event; re-inserting the element does not restore its focus.
    fn unfocus_removed(&mut self, node: u32) {
        let doc = self.n(node).owner_doc;
        let focused = match &self.n(doc).kind {
            NodeKind::Document(d) if d.focused != NIL => d.focused,
            _ => return,
        };
        let mut i = focused;
        let inside = loop {
            if i == node {
                break true;
            }
            i = self.n(i).parent;
            if i == NIL {
                break false;
            }
        };
        if !inside {
            return;
        }
        if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
            d.focused = NIL;
        }
        self.note(Change::Pseudo);
    }

    /// Changes the node document of `node`'s inclusive descendants (incl.
    /// shadow trees) to `doc`. The contents of a `<template>` among them
    /// follow into that document's template contents owner.
    fn adopt_idx(&mut self, node: u32, doc: u32) {
        let old = self.n(node).owner_doc;
        if old == doc || matches!(self.n(node).kind, NodeKind::Document(_)) {
            return;
        }
        // A node that moves to a document of another window keeps the realm
        // it was created in (see `creation_document`).
        let created_in = self.realm_doc(old).filter(|&from| Some(from) != self.realm_doc(doc));
        let mut templates = Vec::new();
        let mut stack = vec![node];
        let mut moved = 0u32;
        let mut held = 0u32;
        while let Some(i) = stack.pop() {
            if self.n(i).owner_doc == old {
                let registry = self.registry_idx(i);
                let element = self.n(i).element().is_some();
                let shadow_flags = match &self.n(i).kind {
                    NodeKind::Fragment(f) if f.shadow.is_some() => Some(f.flags),
                    _ => None,
                };
                let registry = if self.is_fragment_parse_host(old) && !self.is_inert_doc(doc) {
                    // Fragment parser nodes already have their semantic registry;
                    // moving them out of our temporary owner is not DOM adoption.
                    registry
                } else if matches!(registry, RegistryAssociation::Scoped(_)) {
                    registry
                } else if element {
                    let parent = self.n(i).parent;
                    let source = if registry == RegistryAssociation::Null && parent != NIL
                        && !matches!(&self.n(parent).kind, NodeKind::Fragment(f) if f.shadow.is_none()) {
                        self.registry_idx(parent)
                    } else {
                        self.registry_idx(doc)
                    };
                    source.effective_global()
                } else if let Some(flags) = shadow_flags {
                    if registry == RegistryAssociation::Null && flags & shadow_flags::KEEP_REGISTRY_NULL != 0 {
                        RegistryAssociation::Null
                    } else {
                        self.registry_idx(doc).effective_global()
                    }
                } else {
                    registry
                };
                self.n_mut(i).owner_doc = doc;
                if element || shadow_flags.is_some() { self.set_registry_idx(i, registry); }
                moved += 1;
                held += u32::from(self.n(i).ext_refs > 0);
                if let Some(from) = created_in {
                    self.creation_docs.entry(i).or_insert(from);
                }
                // Inline `on*` handlers compile in the new document (a clone
                // of template contents had none in its inert one).
                self.note_inline_handlers(i);
                let contents = self.n(i).element().map_or(NIL, |e| e.template_contents);
                if contents != NIL {
                    templates.push(contents);
                }
            }
            stack.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
                // Attr nodes share their element's node document.
                stack.extend_from_slice(self.attr_nodes_of(i));
            }
        }
        if self.reactions.any_custom {
            self.enqueue_adopted(node, old, doc);
        }
        self.doc_data_mut(old).owned -= moved;
        self.doc_data_mut(doc).owned += moved;
        self.doc_data_mut(old).held -= held;
        self.doc_data_mut(doc).held += held;
        self.candidates.push(old);
        // HTML "template adopting steps": the contents of an adopted
        // `<template>` move to the template contents owner of its new
        // document (and so do the contents of the templates nested in them).
        for contents in templates {
            let inert = self.inert_doc_for(doc);
            self.adopt_idx(contents, inert);
        }
    }

    /// The document whose window's realm the nodes of `doc` are created in:
    /// itself while a browsing context displays it, else the one recorded
    /// for a document without one (`set_document_creator`).
    fn realm_doc(&self, doc: u32) -> Option<NodeId> {
        if self.doc_data(doc).active {
            return Some(self.id_of(doc));
        }
        if self.creation_docs.is_empty() {
            return None;
        }
        self.creation_docs.get(&doc).copied().filter(|&d| self.check(d).is_ok())
    }

    /// Gives the template contents owner of `doc` the realm of `doc`.
    fn note_inert_realm(&mut self, doc: u32) {
        let inert = self.doc_data(doc).inert_doc;
        if inert == NIL || inert == doc {
            return;
        }
        if let Some(realm) = self.realm_doc(doc) {
            self.creation_docs.insert(inert, realm);
        }
    }

    /// Records that `doc`, a document without a browsing context (DOMParser,
    /// `createHTMLDocument`, `new Document()`...), was created by script of
    /// `creator`'s window: its nodes are created in that realm. The first
    /// record stays.
    pub fn set_document_creator(&mut self, doc: NodeId, creator: NodeId) -> DomResult<()> {
        let d = self.check(doc)?;
        let c = self.check(creator)?;
        let NodeKind::Document(data) = &self.n(d).kind else { return Err(DomError::Type("not a document")) };
        if data.active || self.creation_docs.contains_key(&d) {
            return Ok(());
        }
        let creator_doc = if matches!(self.n(c).kind, NodeKind::Document(_)) { c } else { self.n(c).owner_doc };
        if let Some(realm) = self.realm_doc(creator_doc).filter(|&r| r != doc) {
            self.creation_docs.insert(d, realm);
            self.note_inert_realm(d);
        }
        Ok(())
    }

    /// The document whose window `node` was created for, which gives its
    /// realm: a node keeps the interfaces of that window when it is adopted
    /// into a document of another one. It is recorded when a node moves
    /// between documents of different windows, and for a document without a
    /// browsing context; otherwise it follows from the node document, and is
    /// the node document itself when no window is known for it.
    pub fn creation_document(&self, node: NodeId) -> DomResult<NodeId> {
        let i = self.check(node)?;
        if let Some(&doc) = self.creation_docs.get(&i) {
            if self.check(doc).is_ok() {
                return Ok(doc);
            }
        }
        let doc = if matches!(self.n(i).kind, NodeKind::Document(_)) { i } else { self.n(i).owner_doc };
        Ok(self.realm_doc(doc).unwrap_or_else(|| self.id_of(doc)))
    }

    /// `document.adoptNode(node)`.
    pub fn adopt_node(&mut self, doc: NodeId, node: NodeId) -> DomResult<NodeId> {
        let d = self.check(doc)?;
        let n = self.check(node)?;
        match &self.n(n).kind {
            NodeKind::Document(_) => return Err(DomError::new(DomErrorKind::NotSupported, "cannot adopt a document")),
            NodeKind::Fragment(f) if f.shadow.is_some() => return Err(hierarchy("cannot adopt a shadow root")),
            _ => {}
        }
        // An attribute leaves its element first.
        if let Some(owner) = self.attr_data(n).map(|a| a.owner).filter(|&o| o != NIL) {
            let owner = self.id_of(owner);
            self.remove_attribute_node(owner, node)?;
        }
        if self.n(n).parent != NIL {
            self.remove(n, false);
        }
        self.adopt_idx(n, d);
        Ok(node)
    }

    /// `node.cloneNode(deep)` / `document.importNode`. `doc` overrides the
    /// node document of the clone.
    /// Records `i` when it has an `on*` attribute (see `pending_inline_handlers`).
    pub(crate) fn note_inline_handlers(&mut self, i: u32) {
        let has = self.n(i).element().is_some_and(|e| e.attrs.iter().any(|a| a.name.local.starts_with("on")));
        if has {
            let id = self.id_of(i);
            self.pending_inline_handlers.push(id);
        }
    }
    pub fn has_pending_inline_handlers(&self) -> bool {
        !self.pending_inline_handlers.is_empty()
    }
    pub fn take_pending_inline_handlers(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.pending_inline_handlers)
    }

    pub fn clone_node(&mut self, node: NodeId, deep: bool, doc: Option<NodeId>) -> DomResult<NodeId> {
        let fallback = match doc {
            Some(d) => self.registry_association(d)?,
            None => RegistryAssociation::Null,
        };
        self.clone_node_with_registry(node, deep, doc, fallback)
    }

    pub fn clone_node_with_registry(&mut self, node: NodeId, deep: bool, doc: Option<NodeId>, fallback: RegistryAssociation) -> DomResult<NodeId> {
        let n = self.check(node)?;
        let d = match doc {
            Some(d) => self.check(d)?,
            None => self.n(n).owner_doc,
        };
        if let (Some(_), NodeKind::Document(_)) = (doc, &self.n(n).kind) {
            return Err(DomError::new(DomErrorKind::NotSupported, "cannot import a document"));
        }
        if matches!(&self.n(n).kind, NodeKind::Fragment(f) if f.shadow.is_some()) {
            return Err(DomError::new(DomErrorKind::NotSupported, "ShadowRoot nodes are not clonable."));
        }
        let c = self.clone_idx(n, d, deep, fallback)?;
        Ok(self.id_of(c))
    }

    fn clone_idx(&mut self, n: u32, doc: u32, deep: bool, fallback: RegistryAssociation) -> DomResult<u32> {
        let copy = match &self.n(n).kind {
            NodeKind::Document(d) => {
                let (kind, url, ct, q) = (d.kind, d.url.clone(), d.content_type.clone(), d.quirks);
                let allow_dsd = d.allow_declarative_shadow_roots;
                let nd = self.create_document(kind, &url).idx;
                if let NodeKind::Document(dd) = &mut self.n_mut(nd).kind {
                    dd.content_type = ct;
                    dd.quirks = q;
                    dd.allow_declarative_shadow_roots = allow_dsd;
                }
                if let registry @ RegistryAssociation::Scoped(_) = self.registry_idx(n) {
                    self.set_registry_idx(nd, registry);
                }
                nd
            }
            NodeKind::Element(e) => {
                let name = e.name.clone();
                let attrs = e.attr_state();
                let control = e.control.as_ref().map(|c| {
                    // Clone steps copy value/checkedness for input elements.
                    Box::new(ControlState { selection: (0, 0), ..(**c).clone() })
                });
                // HTML script cloning steps: "already started" is copied.
                let started = e.flags & crate::node::element_flags::SCRIPT_ALREADY_STARTED;
                // DOM "clone a single node": the copy has the same is value.
                let is_value = e.is_value.clone();
                let registry = match self.registry_idx(n) {
                    RegistryAssociation::Null => fallback,
                    other => other,
                };
                let registry = if registry == RegistryAssociation::Default { self.registry_idx(doc).effective_global() } else { registry };
                let id = self.create_element_raw(self.id_of(doc), name)?;
                self.set_registry_idx(id.idx, registry);
                let el = self.n_mut(id.idx).element_mut().unwrap();
                el.is_value = is_value;
                el.set_attr_state(attrs);
                el.control = control;
                el.flags |= started;
                self.resummarize(id.idx);
                self.note_inline_handlers(id.idx);
                id.idx
            }
            NodeKind::Text(s) => {
                let s = s.clone();
                self.alloc(NodeKind::Text(s), doc)
            }
            NodeKind::CData(s) => {
                let s = s.clone();
                self.alloc(NodeKind::CData(s), doc)
            }
            NodeKind::Comment(s) => {
                let s = s.clone();
                self.alloc(NodeKind::Comment(s), doc)
            }
            NodeKind::ProcessingInstruction(pi) => {
                let kind = NodeKind::ProcessingInstruction(pi.clone());
                self.alloc(kind, doc)
            }
            NodeKind::Doctype(d) => {
                let d = DoctypeData {
                    name: d.name.clone(),
                    public_id: d.public_id.clone(),
                    system_id: d.system_id.clone(),
                };
                self.alloc(NodeKind::Doctype(Box::new(d)), doc)
            }
            NodeKind::Fragment(_) => self.alloc(NodeKind::Fragment(FragmentData::new(NIL, None, 0)), doc),
            NodeKind::Attr(a) => {
                let name = a.name.clone();
                let value = self.attr_value_idx(n).unwrap_or("").to_owned();
                self.alloc(NodeKind::Attr(Box::new(AttrNodeData { name, value, owner: NIL })), doc)
            }
        };
        // Cloned documents own their own clones.
        let child_doc = if matches!(self.n(copy).kind, NodeKind::Document(_)) { copy } else { doc };
        if deep {
            let kids: Vec<u32> = self.child_idx_iter(n).collect();
            for k in kids {
                let kc = self.clone_idx(k, child_doc, true, fallback)?;
                self.link(kc, copy, NIL);
            }
            // Template contents are cloned too (HTML "cloning steps").
            let src_tc = self.n(n).element().map_or(NIL, |e| e.template_contents);
            let dst_tc = self.n(copy).element().map_or(NIL, |e| e.template_contents);
            if src_tc != NIL && dst_tc != NIL {
                let inert = self.n(dst_tc).owner_doc;
                let kids: Vec<u32> = self.child_idx_iter(src_tc).collect();
                for k in kids {
                    let kc = self.clone_idx(k, inert, true, RegistryAssociation::Null)?;
                    self.link(kc, dst_tc, NIL);
                }
            }
        }
        // A clonable shadow tree comes along, even in a shallow clone.
        let src_sr = self.n(n).element().map_or(NIL, |e| e.shadow_root);
        if src_sr != NIL {
            if let NodeKind::Fragment(FragmentData { shadow: Some(mode), flags, .. }) = self.n(src_sr).kind {
                if flags & shadow_flags::CLONABLE != 0 {
                    let dst_sr = self.new_shadow_root(copy, mode, flags & !shadow_flags::AVAILABLE_TO_INTERNALS);
                    let registry = self.registry_idx(src_sr);
                    let registry = if registry == RegistryAssociation::Default { self.registry_idx(doc).effective_global() } else { registry };
                    self.set_registry_idx(dst_sr, registry);
                    let kids: Vec<u32> = self.child_idx_iter(src_sr).collect();
                    for k in kids {
                        let kc = self.clone_idx(k, doc, true, RegistryAssociation::Null)?;
                        self.link(kc, dst_sr, NIL);
                    }
                }
            }
        }
        // Fresh, parentless clone: collectable unless a wrapper takes it.
        self.candidates.push(copy);
        Ok(copy)
    }

    // ----- attributes --------------------------------------------------

    fn el(&self, id: NodeId) -> DomResult<(u32, &ElementData)> {
        let (i, n) = self.live(id)?;
        n.element().map(|e| (i, e)).ok_or(DomError::Type("not an element"))
    }
    pub fn element(&self, id: NodeId) -> DomResult<&ElementData> {
        Ok(self.el(id)?.1)
    }

    fn html_lower<'a>(&self, el: &ElementData, owner: u32, name: &'a str) -> std::borrow::Cow<'a, str> {
        // Names are mostly lowercase already: test that before reading the
        // owner document.
        if name.bytes().any(|b| b.is_ascii_uppercase())
            && el.is_html()
            && self.doc_data(owner).kind == DocumentKind::Html
        {
            std::borrow::Cow::Owned(name.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(name)
        }
    }

    fn find_attr_qualified(&self, i: u32, name: &str) -> Option<usize> {
        let n = self.n(i);
        let el = n.element()?;
        find_attr_lowered(el, &self.html_lower(el, n.owner_doc, name))
    }

    /// The content attribute `name` as the element's own behaviour reads it
    /// (reflection, form and loading state): the attribute in no namespace
    /// with that local name, so `setAttributeNS(ns, "src", ..)` is not the
    /// element's `src`. A prefixed name (`xlink:href`) is a qualified name.
    fn find_attr_content(&self, i: u32, name: &str) -> Option<usize> {
        let n = self.n(i);
        let el = n.element()?;
        let local = self.html_lower(el, n.owner_doc, name);
        let found = el.attrs.iter().position(|a| a.name.ns == ns!() && *a.name.local == *local);
        match found {
            None if name.contains(':') => find_attr_lowered(el, &local),
            found => found,
        }
    }

    /// Value of the content attribute `name` (see `find_attr_content`); script's
    /// `getAttribute` is `get_attribute_qualified`.
    pub fn get_attribute(&self, id: NodeId, name: &str) -> DomResult<Option<&str>> {
        let (i, el) = self.el(id)?;
        Ok(self.find_attr_content(i, name).map(|k| &*el.attrs[k].value))
    }
    pub fn has_attribute(&self, id: NodeId, name: &str) -> DomResult<bool> {
        let (i, _) = self.el(id)?;
        Ok(self.find_attr_content(i, name).is_some())
    }
    /// `getAttribute(qualifiedName)`: the first attribute with that qualified
    /// name, whatever its namespace.
    pub fn get_attribute_qualified(&self, id: NodeId, name: &str) -> DomResult<Option<&str>> {
        let (i, el) = self.el(id)?;
        Ok(self.find_attr_qualified(i, name).map(|k| &*el.attrs[k].value))
    }
    /// [`Dom::get_attribute_qualified`] with the atom the element keeps for
    /// that value, if any (`id`: see [`ElementData::attr_atom`]).
    pub fn get_attribute_qualified_atom(&self, id: NodeId, name: &str) -> DomResult<Option<(&str, Option<&Atom>)>> {
        let (i, el) = self.el(id)?;
        Ok(self.find_attr_qualified(i, name).map(|k| (&*el.attrs[k].value, el.attr_atom(k))))
    }
    /// Position in `attributes()` of the first attribute with that qualified name.
    pub fn attribute_index_qualified(&self, id: NodeId, name: &str) -> DomResult<Option<usize>> {
        let (i, _) = self.el(id)?;
        Ok(self.find_attr_qualified(i, name))
    }
    /// `hasAttribute(qualifiedName)`.
    pub fn has_attribute_qualified(&self, id: NodeId, name: &str) -> DomResult<bool> {
        let (i, _) = self.el(id)?;
        Ok(self.find_attr_qualified(i, name).is_some())
    }
    /// The attribute `name` names on element `i`, as `find_attr_qualified`
    /// finds it for the name's text: the lowercase atom on an HTML element
    /// of an HTML document. An attribute without a prefix (nearly all) is
    /// matched by its atom, with no text compared.
    fn find_attr_named(&self, i: u32, name: &ScriptName) -> Option<usize> {
        let n = self.n(i);
        let el = n.element()?;
        let want = self.script_name_for(el, n.owner_doc, name);
        el.attrs.iter().position(|a| match &a.name.prefix {
            Some(p) if !p.is_empty() => {
                let name: &str = want;
                name.len() == p.len() + 1 + a.name.local.len()
                    && name.starts_with(&**p)
                    && name.as_bytes()[p.len()] == b':'
                    && name.ends_with(&*a.name.local)
            }
            _ => a.name.local == *want,
        })
    }

    /// `Dom::html_lower` for an interned name.
    #[inline]
    fn script_name_for<'a>(&self, el: &ElementData, owner: u32, name: &'a ScriptName) -> &'a LocalName {
        if name.has_upper && el.is_html() && self.doc_data(owner).kind == DocumentKind::Html {
            &name.lower
        } else {
            &name.exact
        }
    }

    /// [`Dom::get_attribute_qualified_atom`] for an interned name.
    pub fn get_attribute_named_atom(&self, id: NodeId, name: &ScriptName) -> DomResult<Option<(&str, Option<&Atom>)>> {
        let (i, el) = self.el(id)?;
        Ok(self.find_attr_named(i, name).map(|k| (&*el.attrs[k].value, el.attr_atom(k))))
    }
    /// [`Dom::has_attribute_qualified`] for an interned name.
    pub fn has_attribute_named(&self, id: NodeId, name: &ScriptName) -> DomResult<bool> {
        let (i, _) = self.el(id)?;
        Ok(self.find_attr_named(i, name).is_some())
    }
    /// [`Dom::set_attribute`] for an interned name: validated when it was
    /// interned, and a new attribute takes the atom.
    pub fn set_attribute_named(&mut self, id: NodeId, name: &ScriptName, value: &str) -> DomResult<()> {
        if !name.valid_attribute {
            return Err(invalid_character());
        }
        let (i, n) = self.live(id)?;
        let el = n.element().ok_or(DomError::Type("not an element"))?;
        let want = self.script_name_for(el, n.owner_doc, name).clone();
        match self.find_attr_named(i, name) {
            Some(k) => self.change_attr(i, k, value),
            None => self.append_attr(i, QualName::new(None, ns!(), want), value),
        }
        Ok(())
    }
    /// [`Dom::remove_attribute`] for an interned name.
    pub fn remove_attribute_named(&mut self, id: NodeId, name: &ScriptName) -> DomResult<bool> {
        let (i, _) = self.el(id)?;
        match self.find_attr_named(i, name) {
            Some(k) => {
                self.remove_attr_at(i, k);
                Ok(true)
            }
            None => Ok(false),
        }
    }
    pub fn get_attribute_ns(&self, id: NodeId, ns: Option<&str>, local: &str) -> DomResult<Option<&str>> {
        let (_, el) = self.el(id)?;
        let ns = ns.unwrap_or("");
        Ok(el.attrs.iter().find(|a| &*a.name.ns == ns && &*a.name.local == local).map(|a| &*a.value))
    }
    pub fn attributes(&self, id: NodeId) -> DomResult<&[Attr]> {
        Ok(&self.el(id)?.1.attrs)
    }

    /// `setAttribute(qualifiedName, value)`.
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) -> DomResult<()> {
        validate_attribute_name(name)?;
        let (i, n) = self.live(id)?;
        let el = n.element().ok_or(DomError::Type("not an element"))?;
        // Lowercased once, for the lookup and the new attribute's name.
        let name = self.html_lower(el, n.owner_doc, name);
        match find_attr_lowered(el, &name) {
            Some(k) => self.change_attr(i, k, value),
            None => {
                let qn = QualName::new(None, ns!(), LocalName::from(&*name));
                self.append_attr(i, qn, value);
            }
        }
        Ok(())
    }

    /// Set both the CSSOM serialization and the style parser's backing in one DOM mutation.
    pub fn set_cssom_inline_style(&mut self, id: NodeId, canonical: String, semantic: Option<String>) -> DomResult<()> {
        self.set_cssom_inline_style_with_typed_values(id, canonical, semantic, None)
    }

    pub fn set_cssom_inline_style_with_typed_values(
        &mut self, id: NodeId, canonical: String, semantic: Option<String>, typed: Option<crate::stylo::declarations::TypedStyleValues>,
    ) -> DomResult<()> {
        let (i, el) = self.el(id)?;
        let local = local_name!("style");
        let slot = el.attrs.iter().position(|a| a.name.local == local && a.name.ns == ns!());
        let backing = (semantic.is_some() || typed.is_some()).then(|| {
            std::sync::Arc::new(crate::node::InlineStyleBacking { semantic, typed })
        });
        let semantic_only = el.semantic_style != backing && slot.is_some_and(|k| *el.attrs[k].value == *canonical);
        match slot {
            Some(k) => self.change_attr(i, k, &canonical),
            None => self.append_attr(i, QualName::new(None, ns!(), local.clone()), &canonical),
        }
        let el = self.n_mut(i).element_mut().unwrap();
        el.semantic_style = backing;
        el.checked_style_backing = true;
        if semantic_only {
            self.bump();
            self.note(Change::attr(i, &QualName::new(None, ns!(), local)));
        }
        Ok(())
    }

    /// Sets the attribute in no namespace named `local`, as a reflected IDL
    /// attribute does (`className`, `id`, `slot`): the name is a valid,
    /// lowercase atom already, so nothing is validated, lowercased or looked up.
    pub fn set_attribute_local(&mut self, id: NodeId, local: &LocalName, value: &str) -> DomResult<()> {
        let (i, el) = self.el(id)?;
        match el.attrs.iter().position(|a| a.name.local == *local && a.name.ns == ns!()) {
            Some(k) => self.change_attr(i, k, value),
            None => self.append_attr(i, QualName::new(None, ns!(), local.clone()), value),
        }
        Ok(())
    }

    /// `setAttributeNS(namespace, qualifiedName, value)`.
    pub fn set_attribute_ns(&mut self, id: NodeId, ns: Option<&str>, qualified: &str, value: &str) -> DomResult<()> {
        let qn = validate_and_extract(ns, qualified, NameContext::Attribute)?;
        let (i, el) = self.el(id)?;
        match el.attrs.iter().position(|a| a.name.ns == qn.ns && a.name.local == qn.local) {
            Some(k) => self.change_attr(i, k, value),
            None => self.append_attr(i, qn, value),
        }
        Ok(())
    }

    /// Changes the value of the existing attribute (`ns`, `local`) without
    /// re-validating its name (`Attr.value` setter). Returns false if the
    /// element has no such attribute.
    pub fn set_existing_attribute_ns(
        &mut self,
        id: NodeId,
        ns: Option<&str>,
        local: &str,
        value: &str,
    ) -> DomResult<bool> {
        let (i, el) = self.el(id)?;
        let ns = ns.unwrap_or("");
        match el.attrs.iter().position(|a| &*a.name.ns == ns && &*a.name.local == local) {
            Some(k) => {
                self.change_attr(i, k, value);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Every write of element `i`'s attribute list goes through here (`name`:
    /// the attribute written), so what is derived from attributes elsewhere
    /// follows.
    pub(crate) fn edit_attrs<R>(&mut self, i: u32, name: &QualName, f: impl FnOnce(&mut ElementData) -> R) -> R {
        let plain = name.ns == ns!();
        if plain && !self.svg_strings.is_empty() {
            if let Some(kind) = crate::svg_string_list::StringListAttribute::from_name(&name.local) {
                self.svg_strings.remove(&(i, kind));
            }
        }
        let el = self.n_mut(i).element_mut().expect("element");
        let old_id = (plain && name.local == local_name!("id")).then(|| el.id_atom().cloned());
        let old_for = (plain && name.local == local_name!("for") && el.is_html() && el.name.local == local_name!("label"))
            .then(|| el.attr_local(&local_name!("for")).map(Box::<str>::from));
        let r = f(el);
        if plain && name.local == local_name!("disabled") && el.is_html() && el.name.local == local_name!("link") {
            el.link_explicitly_enabled = el.attr_local(&name.local).is_none();
        }
        debug_assert!(el.derived_in_step());
        let base = plain && name.local == local_name!("href") && el.is_html() && el.name.local == local_name!("base");
        if plain && &*name.local == "points" && !self.svg_points.is_empty() {
            if let Some(lists) = self.svg_points.get(&i).and_then(std::rc::Weak::upgrade) {
                lists.attribute_changed(self.n(i).element().unwrap().attr_local(&name.local).unwrap_or_default());
            }
        }
        if plain && !self.svg_numbers.is_empty() {
            if let Some(lists) = self.svg_numbers.get(&i).and_then(|attrs| attrs.get(&*name.local)).and_then(std::rc::Weak::upgrade) {
                lists.attribute_changed(&name.local, self.n(i).element().unwrap().attr_local(&name.local).unwrap_or_default());
            }
        }
        if plain && name.local == local_name!("class") {
            self.resummarize(i);
        }
        if let Some(old) = old_id {
            self.id_attr_changed(i, old);
        }
        if let Some(old) = old_for {
            self.label_for_attr_changed(i, old);
        }
        if base && self.n(i).in_document_tree() {
            let doc = self.n(i).owner_doc;
            self.html_base_changed(doc);
        }
        if plain && matches!(&*name.local, "title" | "type") && self.n(i).in_document_tree() {
            self.note_preferred_style(i);
        }
        r
    }

    pub(crate) fn note_preferred_style(&mut self, idx: u32) {
        let node = self.n(idx);
        let Some(element) = node.element() else { return };
        if !element.is_html() || element.name.local != local_name!("style")
            || element.attr("type").is_some_and(|kind| !kind.is_empty() && !kind.eq_ignore_ascii_case("text/css"))
        {
            return;
        }
        self.note_preferred_sheet_name(idx);
    }

    pub fn note_preferred_link_sheet(&mut self, link: NodeId) {
        let Ok(idx) = self.check(link) else { return };
        let node = self.n(idx);
        let Some(element) = node.element() else { return };
        if !node.in_document_tree() || !element.is_html() || element.name.local != local_name!("link")
            || element.link_explicitly_enabled || element.attr("disabled").is_some()
            || element.attr("rel").is_none_or(|rel| rel.split_ascii_whitespace().any(|token| token.eq_ignore_ascii_case("alternate")))
        {
            return;
        }
        self.note_preferred_sheet_name(idx);
    }

    fn note_preferred_sheet_name(&mut self, idx: u32) {
        let node = self.n(idx);
        let doc = node.owner_doc;
        if self.doc_data(doc).preferred_sheet_name.is_none() {
            let title = node.element().and_then(|el| el.attr("title")).filter(|title| !title.is_empty()).map(Into::into);
            self.doc_data_mut(doc).preferred_sheet_name = title;
        }
    }

    pub(crate) fn sheet_outside_preferred_set(&self, owner: NodeId) -> bool {
        let Ok(node) = self.node(owner) else { return false };
        let Some(element) = node.element() else { return false };
        if !element.is_html() || !matches!(&*element.name.local, "style" | "link")
            || !node.in_document_tree() || element.link_explicitly_enabled {
            return false;
        }
        let title = element.attr("title").filter(|title| !title.is_empty());
        let preferred = self.doc_data(node.owner_doc).preferred_sheet_name.as_deref();
        if element.name.local == local_name!("link") && element.attr("rel").is_some_and(|rel|
            rel.split_ascii_whitespace().any(|token| token.eq_ignore_ascii_case("alternate"))) {
            return title.is_none() || title != preferred;
        }
        title.is_some() && preferred.is_some() && title != preferred
    }

    pub(crate) fn change_attr(&mut self, i: u32, k: usize, value: &str) {
        let el = self.n(i).element().unwrap();
        let name = el.attrs[k].name.clone();
        let old = self.edit_attrs(i, &name, |el| el.replace_attr_value(k, value.into()));
        if *old != *value {
            if name.ns == ns!() && name.local == local_name!("style") {
                let el = self.n_mut(i).element_mut().unwrap();
                el.semantic_style = None;
                el.checked_style_backing = false;
            }
            self.note_style_attr(i, &name, Some(&old));
        }
        if *old == *value {
            // Same value: still an attribute mutation (records, XPath
            // iterators), but nothing that styles or geometry depend on
            // changed, so caches keyed by `version` stay valid.
            self.tree_version = self.tree_version.wrapping_add(1);
        } else {
            self.bump_tree();
            self.note(Change::attr(i, &name));
        }
        self.queue_attribute(i, &name, Some(&old));
        self.attribute_changed(i, &name, Some(&old), Some(value));
        if self.internals.faces != 0 {
            self.faces_after_attr(&name);
        }
        self.announce_named_attr(i, &name);
        if !drives_element_behaviour(&name) {
            return;
        }
        self.resources_after_attr(i, &name.local, false);
        if *old != *value {
            self.slots_after_attr_change(i, &name.local);
            self.details_after_attr(i, &name, None);
        }
    }
    pub(crate) fn append_attr(&mut self, i: u32, name: QualName, value: &str) {
        self.note_style_attr(i, &name, None);
        let attr = Attr { name: name.clone(), value: value.into() };
        self.edit_attrs(i, &name, |el| el.push_attr(attr));
        self.bump_tree();
        self.note(Change::attr(i, &name));
        self.queue_attribute(i, &name, None);
        self.attribute_changed(i, &name, None, Some(value));
        if self.internals.faces != 0 {
            self.faces_after_attr(&name);
        }
        self.announce_named_attr(i, &name);
        if !drives_element_behaviour(&name) {
            return;
        }
        self.slots_after_attr_change(i, &name.local);
        self.resources_after_attr(i, &name.local, true);
        self.details_after_attr(i, &name, Some(true));
    }

    /// `removeAttribute(qualifiedName)`. Returns whether an attribute was removed.
    pub fn remove_attribute(&mut self, id: NodeId, name: &str) -> DomResult<bool> {
        let (i, _) = self.el(id)?;
        match self.find_attr_qualified(i, name) {
            Some(k) => {
                self.remove_attr_at(i, k);
                Ok(true)
            }
            None => Ok(false),
        }
    }
    pub fn remove_attribute_ns(&mut self, id: NodeId, ns: Option<&str>, local: &str) -> DomResult<bool> {
        let (i, el) = self.el(id)?;
        let ns = ns.unwrap_or("");
        match el.attrs.iter().position(|a| &*a.name.ns == ns && &*a.name.local == local) {
            Some(k) => {
                self.remove_attr_at(i, k);
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn remove_attr_at(&mut self, i: u32, k: usize) {
        let name = self.n(i).element().unwrap().attrs[k].name.clone();
        let attr = self.edit_attrs(i, &name, |el| {
            if name.ns == ns!() && name.local == local_name!("style") {
                el.semantic_style = None;
                el.checked_style_backing = false;
            }
            el.remove_attr(k)
        });
        if !self.attr_nodes.is_empty() {
            self.attr_removed(i, &attr.name, &attr.value);
        }
        self.note_style_attr(i, &attr.name, Some(&attr.value));
        self.bump_tree();
        self.note(Change::attr(i, &attr.name));
        self.queue_attribute(i, &attr.name, Some(&attr.value));
        self.attribute_changed(i, &attr.name, Some(&attr.value), None);
        if self.internals.faces != 0 {
            self.faces_after_attr(&attr.name);
        }
        if !drives_element_behaviour(&attr.name) {
            return;
        }
        self.resources_after_attr(i, &attr.name.local, false);
        self.slots_after_attr_change(i, &attr.name.local);
        self.details_after_attr(i, &attr.name, Some(false));
    }

    /// `toggleAttribute(name, force)`.
    pub fn toggle_attribute(&mut self, id: NodeId, name: &str, force: Option<bool>) -> DomResult<bool> {
        validate_attribute_name(name)?;
        let present = self.has_attribute_qualified(id, name)?;
        match (present, force) {
            (false, None | Some(true)) => {
                self.set_attribute(id, name, "")?;
                Ok(true)
            }
            (true, None | Some(false)) => {
                self.remove_attribute(id, name)?;
                Ok(false)
            }
            (p, _) => Ok(p),
        }
    }

    // ----- names -------------------------------------------------------

    /// `Element.tagName` / `nodeName` for elements.
    pub fn tag_name(&self, id: NodeId) -> DomResult<String> {
        let (i, el) = self.el(id)?;
        let mut q = match &el.name.prefix {
            Some(p) if !p.is_empty() => format!("{}:{}", p, el.name.local),
            _ => el.name.local.to_string(),
        };
        if el.is_html() && self.doc_data(self.n(i).owner_doc).kind == DocumentKind::Html {
            q.make_ascii_uppercase();
        }
        Ok(q)
    }

    /// `tagName` as a shared string: one allocation per distinct name for
    /// the DOM's lifetime, so callers can cache by pointer identity.
    pub fn tag_name_shared(&self, id: NodeId) -> DomResult<std::sync::Arc<str>> {
        let (i, el) = self.el(id)?;
        let upper = el.is_html() && self.doc_data(self.n(i).owner_doc).kind == DocumentKind::Html;
        let key = (el.name.prefix.clone(), el.name.local.clone(), upper);
        if let Some(s) = self.names.borrow().tags.get(&key) {
            return Ok(s.clone());
        }
        let s: std::sync::Arc<str> = self.tag_name(id)?.into();
        self.names.borrow_mut().tags.insert(key, s.clone());
        Ok(s)
    }

    /// An unprefixed element's name as `tagName` spells it: its local name,
    /// uppercased when the flag is set (an HTML element in an HTML document).
    /// `None` for other nodes and prefixed names ([`Dom::tag_name_shared`]).
    #[inline]
    pub fn tag_name_parts(&self, id: NodeId) -> DomResult<Option<(&LocalName, bool)>> {
        let (_, node) = self.live(id)?;
        Ok(node.element().filter(|el| el.name.prefix.as_ref().is_none_or(|p| p.is_empty())).map(|el| {
            (&el.name.local, el.is_html() && self.doc_data(node.owner_doc).kind == DocumentKind::Html)
        }))
    }

    /// `nodeName` as a shared string (see [`Dom::tag_name_shared`]).
    pub fn node_name_shared(&self, id: NodeId) -> DomResult<std::sync::Arc<str>> {
        let i = self.check(id)?;
        let fixed = match &self.n(i).kind {
            NodeKind::Element(_) => return self.tag_name_shared(id),
            NodeKind::Text(_) => 0,
            NodeKind::CData(_) => 1,
            NodeKind::Comment(_) => 2,
            NodeKind::Document(_) => 3,
            NodeKind::Fragment(_) => 4,
            NodeKind::Doctype(_) | NodeKind::ProcessingInstruction(_) | NodeKind::Attr(_) => {
                return Ok(self.node_name(id)?.into())
            }
        };
        Ok(self.names.borrow().fixed[fixed].clone())
    }

    pub fn node_name(&self, id: NodeId) -> DomResult<String> {
        let i = self.check(id)?;
        Ok(match &self.n(i).kind {
            NodeKind::Element(_) => return self.tag_name(id),
            NodeKind::Text(_) => "#text".into(),
            NodeKind::CData(_) => "#cdata-section".into(),
            NodeKind::Comment(_) => "#comment".into(),
            NodeKind::Document(_) => "#document".into(),
            NodeKind::Fragment(_) => "#document-fragment".into(),
            NodeKind::Doctype(d) => d.name.to_string(),
            NodeKind::ProcessingInstruction(pi) => pi.target.to_string(),
            NodeKind::Attr(a) => a.qualified_name(),
        })
    }

    // ----- text --------------------------------------------------------

    /// `Node.textContent` getter (None for documents and doctypes).
    pub fn text_content(&self, id: NodeId) -> DomResult<Option<String>> {
        Ok(self.text_content_ref(id)?.map(std::borrow::Cow::into_owned))
    }

    /// [`Dom::text_content`], borrowed from the node holding it when one
    /// node's data is all of it (character data, an attribute's value, an
    /// element with one text node).
    pub fn text_content_ref(&self, id: NodeId) -> DomResult<Option<std::borrow::Cow<'_, str>>> {
        let (i, n) = self.live(id)?;
        Ok(match &n.kind {
            NodeKind::Document(_) | NodeKind::Doctype(_) => None,
            NodeKind::Attr(_) => self.attr_value_idx(i).map(std::borrow::Cow::Borrowed),
            k => match k.char_data() {
                Some(d) => Some(std::borrow::Cow::Borrowed(d)),
                None => Some(self.descendant_text(i)),
            },
        })
    }

    /// The concatenated data of the Text descendants of `root` (light tree)
    /// in one walk: borrowed while at most one of them is non-empty.
    pub(crate) fn descendant_text(&self, root: u32) -> std::borrow::Cow<'_, str> {
        let r = self.n(root);
        if r.first_child != NIL && r.first_child == r.last_child {
            if let NodeKind::Text(s) | NodeKind::CData(s) = &self.n(r.first_child).kind {
                return std::borrow::Cow::Borrowed(s);
            }
        }
        let mut first: &str = "";
        let mut out: Option<String> = None;
        let mut cur = r.first_child;
        while cur != NIL {
            if let NodeKind::Text(s) | NodeKind::CData(s) = &self.n(cur).kind {
                if let Some(out) = &mut out {
                    out.push_str(s);
                } else if first.is_empty() {
                    first = s;
                } else if !s.is_empty() {
                    let mut o = String::with_capacity(2 * (first.len() + s.len()));
                    o.push_str(first);
                    o.push_str(s);
                    out = Some(o);
                }
            }
            cur = self.following_in(cur, root);
        }
        out.map_or(std::borrow::Cow::Borrowed(first), std::borrow::Cow::Owned)
    }

    /// The child text content of `id` (HTML): the data of its Text (and
    /// CDATA) children, in order, not of deeper descendants. Borrowed while
    /// at most one of them is non-empty; empty for a stale id.
    pub fn child_text_content(&self, id: NodeId) -> std::borrow::Cow<'_, str> {
        let Ok(i) = self.check(id) else { return std::borrow::Cow::Borrowed("") };
        let mut first: &str = "";
        let mut out: Option<String> = None;
        let mut cur = self.n(i).first_child;
        while cur != NIL {
            if let NodeKind::Text(s) | NodeKind::CData(s) = &self.n(cur).kind {
                if let Some(out) = &mut out {
                    out.push_str(s);
                } else if first.is_empty() {
                    first = s;
                } else if !s.is_empty() {
                    let mut o = String::with_capacity(2 * (first.len() + s.len()));
                    o.push_str(first);
                    o.push_str(s);
                    out = Some(o);
                }
            }
            cur = self.n(cur).next;
        }
        out.map_or(std::borrow::Cow::Borrowed(first), std::borrow::Cow::Owned)
    }

    /// Concatenated data of descendant Text nodes (light tree).
    pub(crate) fn collect_text(&self, root: u32, out: &mut String) {
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            if let NodeKind::Text(s) | NodeKind::CData(s) = &self.n(cur).kind {
                out.push_str(s);
            }
            cur = self.following_in(cur, root);
        }
    }

    /// `Node.textContent` setter.
    pub fn set_text_content(&mut self, id: NodeId, value: &str) -> DomResult<()> {
        let i = self.check(id)?;
        match &self.n(i).kind {
            NodeKind::Document(_) | NodeKind::Doctype(_) => Ok(()),
            NodeKind::Attr(_) => self.set_attr_node_value(id, value),
            k if k.char_data().is_some() => self.replace_data(id, 0, u32::MAX, value),
            _ => {
                // Its one text child holds this text already: nothing to
                // do, as in Chrome (no records, the child stays; the spec's
                // "replace all" would swap it for an equal one).
                let first = self.n(i).first_child;
                if !value.is_empty()
                    && first != NIL
                    && self.n(first).next == NIL
                    && matches!(&self.n(first).kind, NodeKind::Text(s) if s == value)
                {
                    return Ok(());
                }
                let t = if value.is_empty() {
                    None
                } else {
                    let doc = self.id_of(self.n(i).owner_doc);
                    Some(self.create_text(doc, value)?)
                };
                self.replace_all(id, t)
            }
        }
    }

    pub fn char_data(&self, id: NodeId) -> DomResult<&str> {
        self.node(id)?.kind.char_data().ok_or(DomError::Type("not character data"))
    }

    /// `CharacterData.length` in UTF-16 code units.
    pub fn char_length(&self, id: NodeId) -> DomResult<u32> {
        Ok(utf16_len(self.char_data(id)?))
    }

    /// `substringData(offset, count)` in UTF-16 code units.
    pub fn substring_data(&self, id: NodeId, offset: u32, count: u32) -> DomResult<String> {
        let s = self.char_data(id)?;
        let (a, b) = utf16_range(s, offset, count)?;
        Ok(s[a..b].to_owned())
    }

    /// `replaceData(offset, count, data)`; also backs append/insert/delete.
    pub fn replace_data(&mut self, id: NodeId, offset: u32, count: u32, data: &str) -> DomResult<()> {
        let i = self.check(id)?;
        let s = self.n(i).kind.char_data().ok_or(DomError::Type("not character data"))?;
        let (a, b) = utf16_range(s, offset, count)?;
        let old = s.to_owned();
        self.n_mut(i).kind.char_data_mut().unwrap().replace_range(a..b, data);
        if self.n(i).kind.char_data() == Some(old.as_str()) {
            // Unchanged text: a mutation for observers and XPath iterators
            // only (see change_attr).
            self.tree_version = self.tree_version.wrapping_add(1);
        } else {
            self.bump_tree();
            self.note(Change::Text(i));
        }
        if !self.live_ranges.is_empty() {
            let count = count.min(utf16_len(&old) - offset);
            self.ranges_after_replace_data(i, offset, count, utf16_len(data));
        }
        self.queue_character_data(i, &old);
        self.resources_after_text(i);
        Ok(())
    }

    pub fn set_char_data(&mut self, id: NodeId, data: &str) -> DomResult<()> {
        self.replace_data(id, 0, u32::MAX, data)
    }

    /// `Text.splitText(offset)`.
    pub fn split_text(&mut self, id: NodeId, offset: u32) -> DomResult<NodeId> {
        let i = self.check(id)?;
        let s = match &self.n(i).kind {
            NodeKind::Text(s) | NodeKind::CData(s) => s,
            _ => return Err(DomError::Type("not a text node")),
        };
        let (a, _) = utf16_range(s, offset, 0)?;
        let tail = s[a..].to_owned();
        let doc = self.n(i).owner_doc;
        let kind = match &self.n(i).kind {
            NodeKind::CData(_) => NodeKind::CData(tail),
            _ => NodeKind::Text(tail),
        };
        let new = self.alloc(kind, doc);
        let parent = self.n(i).parent;
        if parent != NIL {
            let next = self.n(i).next;
            self.insert(new, parent, next, false);
            if !self.live_ranges.is_empty() {
                self.ranges_after_split(i, new, offset);
            }
        } else {
            self.candidates.push(new);
        }
        self.replace_data(id, offset, u32::MAX, "")?;
        Ok(self.id_of(new))
    }

    /// `Node.normalize()`.
    pub fn normalize(&mut self, id: NodeId) -> DomResult<()> {
        let root = self.check(id)?;
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            let next_in_order;
            if matches!(self.n(cur).kind, NodeKind::Text(_)) {
                let len = utf16_len(self.n(cur).kind.char_data().unwrap());
                if len == 0 {
                    next_in_order = self.following_skipping(cur, root);
                    self.remove(cur, false);
                    cur = next_in_order;
                    continue;
                }
                // Merge following contiguous text siblings.
                let mut data = String::new();
                let mut sib = self.n(cur).next;
                while sib != NIL && matches!(self.n(sib).kind, NodeKind::Text(_)) {
                    data.push_str(self.n(sib).kind.char_data().unwrap());
                    sib = self.n(sib).next;
                }
                if !data.is_empty() {
                    let cur_id = self.id_of(cur);
                    self.replace_data(cur_id, len, 0, &data)?;
                }
                if !self.live_ranges.is_empty() {
                    let mut base_len = len;
                    let mut sib = self.n(cur).next;
                    while sib != NIL && matches!(self.n(sib).kind, NodeKind::Text(_)) {
                        self.ranges_merge_text(cur, sib, base_len);
                        base_len += utf16_len(self.n(sib).kind.char_data().unwrap());
                        sib = self.n(sib).next;
                    }
                }
                let mut sib = self.n(cur).next;
                while sib != NIL && matches!(self.n(sib).kind, NodeKind::Text(_)) {
                    let nx = self.n(sib).next;
                    self.remove(sib, false);
                    sib = nx;
                }
            }
            cur = self.following_in(cur, root);
        }
        Ok(())
    }
    fn following_skipping(&self, idx: u32, root: u32) -> u32 {
        let mut cur = idx;
        while cur != root && cur != NIL {
            if self.n(cur).next != NIL {
                return self.n(cur).next;
            }
            cur = self.n(cur).parent;
        }
        NIL
    }

    // ----- lookups -----------------------------------------------------

    /// `getElementById` over the light tree under `root`, in tree order. A
    /// document or shadow root answers from its id map (kept by every link,
    /// unlink and `id` write, see `tree_scope`); anything else is walked.
    pub fn get_element_by_id(&self, root: NodeId, id: &str) -> DomResult<Option<NodeId>> {
        let r = self.check(root)?;
        Ok(self.opt_id(self.first_by_id(r, id)))
    }

    /// `document.documentElement`.
    pub fn document_element(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        let e = match &self.n(d).kind {
            NodeKind::Document(data) => data.document_element,
            _ => self.first_element_child_idx(d),
        };
        Ok((e != NIL).then(|| self.id_of(e)))
    }
    /// First element child of `parent` (NIL for none).
    pub(crate) fn first_element_child_idx(&self, parent: u32) -> u32 {
        self.child_idx_iter(parent).find(|&c| self.n(c).is_element()).unwrap_or(NIL)
    }
    pub fn doctype(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        Ok(self.child_idx_iter(d).find(|&c| matches!(self.n(c).kind, NodeKind::Doctype(_))).map(|c| self.id_of(c)))
    }
    /// `document.head`.
    pub fn head(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        let h = match &self.n(d).kind {
            NodeKind::Document(data) => data.head,
            _ => match self.first_element_child_idx(d) {
                html if html != NIL && self.is_html_root(html) => self.head_child_idx(html, NIL),
                _ => NIL,
            },
        };
        Ok((h != NIL).then(|| self.id_of(h)))
    }
    /// `document.body` (first body or frameset child of the html element).
    pub fn body(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        let b = match &self.n(d).kind {
            NodeKind::Document(data) => data.body,
            _ => match self.first_element_child_idx(d) {
                html if html != NIL && self.is_html_root(html) => self.body_child_idx(html, NIL),
                _ => NIL,
            },
        };
        Ok((b != NIL).then(|| self.id_of(b)))
    }
    /// Whether element `e` is an HTML `html` element: only such a document
    /// element has a `head` and a `body` (HTML "the html element").
    pub(crate) fn is_html_root(&self, e: u32) -> bool {
        self.n(e).element().is_some_and(|el| el.is_html() && el.name.local == local_name!("html"))
    }
    /// The first HTML `body` or `frameset` child of `html` after `after`
    /// (NIL: from the first child); NIL for none.
    pub(crate) fn body_child_idx(&self, html: u32, after: u32) -> u32 {
        let mut c = if after == NIL { self.n(html).first_child } else { self.n(after).next };
        while c != NIL {
            if self.n(c).element().is_some_and(|e| e.is_html() && is_body_name(&e.name.local)) {
                return c;
            }
            c = self.n(c).next;
        }
        NIL
    }
    /// The first HTML `head` child of `html` after `after` (NIL: from the
    /// first child); NIL for none.
    pub(crate) fn head_child_idx(&self, html: u32, after: u32) -> u32 {
        let mut c = if after == NIL { self.n(html).first_child } else { self.n(after).next };
        while c != NIL {
            if self.n(c).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("head")) {
                return c;
            }
            c = self.n(c).next;
        }
        NIL
    }

    // ----- control state -----------------------------------------------

    pub fn control_state(&self, id: NodeId) -> DomResult<Option<&ControlState>> {
        Ok(self.el(id)?.1.control.as_deref())
    }
    pub fn control_state_mut(&mut self, id: NodeId) -> DomResult<&mut ControlState> {
        let i = self.check(id)?;
        // Control state feeds :checked/:indeterminate/:valid matching.
        self.bump();
        self.note(Change::State(i));
        let el = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        Ok(el.control.get_or_insert_with(Default::default))
    }

    // ----- external references & collection ----------------------------

    /// Registers an external strong reference (JS wrapper creation, records).
    pub fn add_ref(&mut self, id: NodeId) -> DomResult<()> {
        let i = self.check(id)?;
        self.ext_ref_inc(i);
        Ok(())
    }

    /// [`Dom::add_ref`] for each of `ids` (a node listed twice gets two), in
    /// one pass that counts the first references of a run of nodes of one
    /// document at once; none is added when one is stale.
    pub fn add_refs(&mut self, ids: &[NodeId]) -> DomResult<()> {
        let (mut doc, mut firsts) = (NIL, 0);
        for (k, &id) in ids.iter().enumerate() {
            let Ok(i) = self.check(id) else {
                // Undo: each node returns to its count before the call, so
                // none becomes a collection candidate (unlike `release`).
                self.add_held(doc, firsts);
                for done in &ids[..k] {
                    self.ext_ref_dec(done.idx);
                }
                return Err(DomError::StaleNode);
            };
            let n = self.n_mut(i);
            n.ext_refs += 1;
            if n.ext_refs == 1 {
                let owner = n.owner_doc;
                if owner != doc {
                    self.add_held(doc, firsts);
                    (doc, firsts) = (owner, 0);
                }
                firsts += 1;
            }
        }
        self.add_held(doc, firsts);
        Ok(())
    }

    /// Counts `firsts` nodes of document `doc` as newly held.
    fn add_held(&mut self, doc: u32, firsts: u32) {
        if firsts > 0 && doc != NIL && self.slot_live(doc) {
            self.doc_data_mut(doc).held += firsts;
        }
    }

    /// Adds an external reference to node `i`, counting it in its document
    /// (`DocumentData::held`) when it is the node's first.
    fn ext_ref_inc(&mut self, i: u32) {
        let n = self.n_mut(i);
        n.ext_refs += 1;
        if n.ext_refs == 1 {
            let owner = n.owner_doc;
            if owner != NIL && self.slot_live(owner) {
                self.doc_data_mut(owner).held += 1;
            }
        }
    }

    /// Drops an external reference of node `i`; true when it was the last.
    fn ext_ref_dec(&mut self, i: u32) -> bool {
        let n = self.n_mut(i);
        if n.ext_refs == 0 {
            return false;
        }
        n.ext_refs -= 1;
        if n.ext_refs > 0 {
            return false;
        }
        let owner = n.owner_doc;
        if owner != NIL && self.slot_live(owner) {
            let d = self.doc_data_mut(owner);
            d.held = d.held.saturating_sub(1);
        }
        true
    }
    /// Drops an external reference. Collection happens in `collect_garbage`.
    pub fn release(&mut self, id: NodeId) {
        if let Ok(i) = self.check(id) {
            debug_assert!(self.n(i).ext_refs > 0, "unbalanced release");
            if self.ext_ref_dec(i) {
                self.candidates.push(i);
            }
        }
    }
    pub(crate) fn release_candidate(&mut self, idx: u32) {
        self.candidates.push(idx);
    }

    /// Drops the references an HTML parser held on the nodes it made. A
    /// connected node lives exactly as long as its document, so that document
    /// is the one collection candidate for all of them; only detached nodes
    /// are candidates of their own. Releasing them one by one made every
    /// parsed element a candidate, each a walk to its root at the next
    /// `collect_garbage`.
    pub(crate) fn release_parser_pins(&mut self, pins: Vec<NodeId>) {
        let mut last = NIL;
        for id in pins {
            let Ok(i) = self.check(id) else { continue };
            debug_assert!(self.n(i).ext_refs > 0, "unbalanced release");
            if !self.ext_ref_dec(i) {
                continue;
            }
            let n = self.n(i);
            let candidate = if n.is_connected() && n.owner_doc != NIL { n.owner_doc } else { i };
            if candidate != last {
                self.candidates.push(candidate);
                last = candidate;
            }
        }
    }
    pub fn ext_refs(&self, id: NodeId) -> DomResult<u32> {
        Ok(self.node(id)?.ext_refs)
    }

    /// Climbs through parents and fragment hosts to the component root.
    fn component_root(&self, mut idx: u32) -> u32 {
        loop {
            idx = self.root_idx(idx);
            match &self.n(idx).kind {
                NodeKind::Fragment(f) if f.host != NIL => idx = f.host,
                // An Attr node keeps its element (`ownerElement`) alive.
                NodeKind::Attr(a) if a.owner != NIL => idx = a.owner,
                _ => return idx,
            }
        }
    }

    /// The nodes of the component rooted at `root` (its tree, shadow trees,
    /// template contents and Attr nodes) into `out`, which is also the walk's
    /// work list; `false`, and `out` incomplete, as soon as one of them is
    /// referenced (a wrapper, a queued mutation record): that component stays,
    /// and a detached tree whose root has a wrapper is not walked at all.
    fn unreferenced_component(&self, root: u32, out: &mut Vec<u32>) -> bool {
        out.push(root);
        let mut k = 0;
        while let Some(&i) = out.get(k) {
            k += 1;
            if self.n(i).ext_refs > 0 || self.mutation.pins(i) {
                return false;
            }
            out.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    out.push(e.shadow_root);
                }
                if e.template_contents != NIL {
                    out.push(e.template_contents);
                }
                out.extend_from_slice(self.attr_nodes_of(i));
            }
        }
        true
    }

    /// Frees every detached component that has no external references, no
    /// pending mutation-observer interest and (for documents) no surviving
    /// nodes owned elsewhere. Call at a safe point (end of a task); never while
    /// a native caller still holds an unreferenced `NodeId` it intends to use.
    /// Returns the number of nodes freed.
    pub fn collect_garbage(&mut self) -> usize {
        let mut freed_total = 0;
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        let mut last_root = NIL;
        let mut nodes = Vec::new();
        let mut document_freed = false;
        while let Some(c) = self.candidates.pop() {
            if self.slots.get(c as usize).is_none_or(|s| s.node.is_none()) {
                continue;
            }
            if let Some(owner) = self.unreferenced_attached_attr(c) {
                self.free_slot(c, c);
                freed_total += 1;
                // It may have been all that kept its element.
                self.candidates.push(owner);
                continue;
            }
            let root = self.component_root(c);
            // Candidates come in runs of one component (a removed tree's
            // nodes, the nodes made for it): the previous root needs no hash.
            if root == last_root {
                continue;
            }
            last_root = root;
            if let NodeKind::Document(d) = &self.n(root).kind {
                // A document with a referenced node (in its tree or not)
                // stays: no need to walk the tree to find that out (nor to
                // remember it in `seen`, which candidates of live documents
                // only grew).
                if d.active || d.held > 0 {
                    continue;
                }
            }
            // `seen` holds the roots found alive, the only ones a later
            // candidate can name again (a freed root's slot is empty): a
            // component that is freed, as nearly every node a script made
            // and dropped is, costs no entry.
            if !seen.is_empty() && seen.contains(&root) {
                continue;
            }
            nodes.clear();
            if !self.unreferenced_component(root, &mut nodes) {
                seen.insert(root);
                continue;
            }
            if matches!(self.n(root).kind, NodeKind::Document(_)) {
                let own = nodes.iter().filter(|&&i| self.n(i).owner_doc == root).count() as u32;
                if self.doc_data(root).owned != own {
                    seen.insert(root);
                    continue; // nodes owned by this document are alive elsewhere
                }
                document_freed = true;
            }
            freed_total += nodes.len();
            let batch = std::mem::take(&mut nodes);
            for &i in &batch {
                self.free_slot(i, root);
            }
            nodes = batch;
            // Freeing may orphan other documents; re-examine them.
            if !seen.is_empty() {
                seen.clear();
            }
            last_root = NIL;
        }
        if document_freed {
            self.release_free_tail();
        }
        freed_total
    }

    /// Gives back the room of a store sized for documents larger than the
    /// ones it holds now (a page that went from a large document to small
    /// ones): the slots past the last live node, and the room of the store,
    /// its walk table and every table others keep by node index beyond twice
    /// what the documents since the last one ended needed.
    ///
    /// Nothing happens while the room is at most four times that need, as on
    /// a page that goes from one document to the next of about the same
    /// size. Otherwise the free list is remade lowest slot last (`alloc` pops
    /// it), so the nodes of the documents to come gather at the bottom and the
    /// top can go when the last older node does.
    ///
    /// Only when a document was freed: a script that builds and drops trees
    /// in one document reuses the slots it frees and pays nothing here.
    fn release_free_tail(&mut self) {
        /// Below this much room there is nothing worth giving back.
        const MIN_SLOTS: usize = 4096;
        let need = (std::mem::replace(&mut self.live_peak, self.live) as usize).max(MIN_SLOTS / 4);
        if self.slots.capacity() <= need * 4 {
            return;
        }
        let len = self.slots.len();
        let end = self.slots.iter().rposition(|s| s.node.is_some()).map_or(0, |i| i + 1);
        if end < len {
            let floor = self.slots[end..].iter().map(|s| s.gen).max().unwrap_or(1);
            self.gen_floor = self.gen_floor.max(floor);
            self.slots.truncate(end);
            self.walk.truncate(end);
        }
        // Every empty slot below the end, highest first.
        self.free.clear();
        let slots = &self.slots;
        self.free.extend((0..end as u32).rev().filter(|&i| slots[i as usize].node.is_none()));
        let keep = need * 2;
        self.slots.shrink_to(keep);
        self.walk.shrink_to(keep);
        self.free.shrink_to(keep);
        self.candidates.shrink_to(keep);
        // The journal of the documents that ended: what reads it starts over
        // on a new document anyway.
        if self.journal.capacity() > keep * 2 {
            self.journal_floor = self.version;
            self.journal = Vec::new();
        }
    }

    fn free_slot(&mut self, i: u32, component_root: u32) {
        let owner = self.n(i).owner_doc;
        if !self.svg_points.is_empty() { self.svg_points.remove(&i); }
        if !self.svg_numbers.is_empty() { self.svg_numbers.remove(&i); }
        if !self.registry_associations.is_empty() { self.registry_associations.remove(&i); }
        if !self.custom_definition_owners.is_empty() { self.custom_definition_owners.remove(&i); }
        if !self.svg_strings.is_empty() {
            use crate::svg_string_list::StringListAttribute;
            self.svg_strings.remove(&(i, StringListAttribute::RequiredExtensions));
            self.svg_strings.remove(&(i, StringListAttribute::SystemLanguage));
        }
        if owner == i {
            let doc = self.id_of(i);
            self.forget_registry_definitions(RegistryAssociation::Default, Some(doc));
            self.scoped_registry_docs.retain(|_, docs| {
                docs.retain(|(_, known)| *known != doc);
                !docs.is_empty()
            });
            self.shadow_disabled.retain(|(owner, _, _, _)| *owner != Some(doc));
            self.snapshot_nonces.get_mut().remove(&i);
        }
        let inert = match &self.n(i).kind {
            NodeKind::Document(d) if d.inert_doc != NIL && d.inert_doc != i => d.inert_doc,
            _ => NIL,
        };
        self.mutation.forget_node(i);
        self.events.forget(crate::events::TargetKey::Node(i));
        self.forget_attr_nodes(i);
        if !self.internals.is_empty() {
            self.forget_internals(i);
        }
        if !self.creation_docs.is_empty() {
            self.creation_docs.remove(&i);
        }
        let slot = &mut self.slots[i as usize];
        slot.node = None;
        slot.gen = slot.gen.wrapping_add(1).max(1);
        self.free.push(i);
        self.live -= 1;
        if owner != i && owner != NIL && self.slot_live(owner) {
            self.doc_data_mut(owner).owned -= 1;
            // Once per run of nodes of one document (a freed tree's nodes),
            // not once per node: each entry is a look at the document later.
            if owner != component_root && self.candidates.last() != Some(&owner) {
                self.candidates.push(owner);
            }
        }
        if inert != NIL && self.slot_live(inert) {
            self.ext_ref_dec(inert);
            self.candidates.push(inert);
        }
    }
}

/// Elements react to their own content attributes (`src`, `slot`, `name`..),
/// which are in no namespace, plus SVG's `xlink:href`: an attribute that only
/// shares a local name with one, set through `setAttributeNS`, changes nothing.
fn drives_element_behaviour(name: &QualName) -> bool {
    name.ns == ns!() || name.ns == ns!(xlink)
}

pub struct ChildIter<'a> {
    dom: &'a Dom,
    cur: u32,
}
impl Iterator for ChildIter<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.cur == NIL {
            return None;
        }
        let c = self.cur;
        self.cur = self.dom.n(c).next;
        Some(self.dom.id_of(c))
    }
}

pub struct Descendants<'a> {
    dom: &'a Dom,
    root: u32,
    next: u32,
}
impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        if self.next == NIL {
            return None;
        }
        let c = self.next;
        self.next = self.dom.following_in(c, self.root);
        Some(self.dom.id_of(c))
    }
}

// ----- helpers -----------------------------------------------------------

/// `body` or `frameset`: the local names of a document's body element.
pub(crate) fn is_body_name(local: &LocalName) -> bool {
    *local == local_name!("body") || *local == local_name!("frameset")
}

/// A name script passed as a string (`getAttribute(name)`,
/// `createElement(name)`), interned once: as written and ASCII-lowercased
/// (one atom when it has no uppercase letter), with the checks that depend on
/// its text alone. Whoever keeps one for a JavaScript string resolves that
/// string's later uses without reading its characters.
#[derive(Clone, Debug)]
pub struct ScriptName {
    pub exact: LocalName,
    pub lower: LocalName,
    pub has_upper: bool,
    /// `validate_attribute_name` accepts it.
    pub valid_attribute: bool,
    /// `validate_element_name` accepts it.
    pub valid_element: bool,
}

impl ScriptName {
    pub fn new(name: &str) -> ScriptName {
        let exact = LocalName::from(name);
        let has_upper = name.bytes().any(|b| b.is_ascii_uppercase());
        let lower = if has_upper { LocalName::from(name.to_ascii_lowercase()) } else { exact.clone() };
        ScriptName {
            exact,
            lower,
            has_upper,
            valid_attribute: is_valid_attribute_local_name(name),
            valid_element: is_valid_element_local_name(name),
        }
    }
}

/// The first attribute of `el` whose qualified name is `name`, which is
/// already lowercased as `Dom::html_lower` says.
#[inline]
fn find_attr_lowered(el: &ElementData, name: &str) -> Option<usize> {
    el.attrs.iter().position(|a| match &a.name.prefix {
        Some(p) if !p.is_empty() => {
            name.len() == p.len() + 1 + a.name.local.len()
                && name.starts_with(&**p)
                && name.as_bytes()[p.len()] == b':'
                && name.ends_with(&*a.name.local)
        }
        _ => *a.name.local == *name,
    })
}

/// The id `:target` matches for a document at `url`: its fragment,
/// percent-decoded (`None` without one, or when it is not UTF-8, which no id is).
fn url_target(url: &str) -> Option<Atom> {
    let (_, frag) = url.split_once('#')?;
    if frag.is_empty() {
        return None;
    }
    String::from_utf8(crate::select::percent_decode(frag)).ok().map(|s| Atom::from(s.as_str()))
}

pub fn utf16_len(s: &str) -> u32 {
    if s.is_ascii() {
        return s.len() as u32;
    }
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

/// Converts a UTF-16 (offset, count) to a byte range, clamping count. An
/// offset inside a surrogate pair snaps to the start of that character.
pub fn utf16_range(s: &str, offset: u32, count: u32) -> DomResult<(usize, usize)> {
    if s.is_ascii() {
        let len = s.len() as u64;
        if offset as u64 > len {
            return Err(DomError::new(DomErrorKind::IndexSize, "offset is greater than the length"));
        }
        let end = (offset as u64 + count as u64).min(len);
        return Ok((offset as usize, end as usize));
    }
    let to_byte = |target: u64| -> Option<usize> {
        let mut u = 0u64;
        for (b, ch) in s.char_indices() {
            if u >= target {
                return Some(b);
            }
            u += ch.len_utf16() as u64;
            if u > target {
                return Some(b);
            }
        }
        (u >= target).then_some(s.len())
    };
    let a =
        to_byte(offset as u64).ok_or(DomError::new(DomErrorKind::IndexSize, "offset is greater than the length"))?;
    let b = to_byte(offset as u64 + count as u64).unwrap_or(s.len());
    Ok((a, b.max(a)))
}

fn is_name_start(c: char) -> bool {
    matches!(c, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}
fn is_name_char(c: char) -> bool {
    is_name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// XML `Name` production check (processing instruction targets, XML parsing).
pub fn validate_name(name: &str) -> DomResult<()> {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_name_start(c) && chars.all(is_name_char) => Ok(()),
        _ => Err(DomError::new(DomErrorKind::InvalidCharacter, "the string contains invalid characters")),
    }
}

fn invalid_character() -> DomError {
    DomError::new(DomErrorKind::InvalidCharacter, "the string contains invalid characters")
}

/// DOM "valid element local name".
pub fn is_valid_element_local_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if first.is_ascii_alphabetic() {
        return !name.contains(|c: char| c.is_ascii_whitespace() || matches!(c, '\0' | '/' | '>'));
    }
    if !(matches!(first, ':' | '_') || first >= '\u{80}') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_') || c >= '\u{80}')
}

/// HTML "valid custom element name".
pub fn is_valid_custom_element_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.contains('-')
        && is_valid_element_local_name(name)
        && !name.chars().any(|c| c.is_ascii_uppercase())
        && !RESERVED.contains(&name)
}

/// DOM "valid attribute local name".
pub fn is_valid_attribute_local_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(|c: char| c.is_ascii_whitespace() || matches!(c, '\0' | '/' | '=' | '>'))
}

/// DOM "valid namespace prefix".
fn is_valid_namespace_prefix(name: &str) -> bool {
    !name.is_empty() && !name.contains(|c: char| c.is_ascii_whitespace() || matches!(c, '\0' | '/' | '>'))
}

/// DOM "valid doctype name" (`createDocumentType`).
pub fn validate_doctype_name(name: &str) -> DomResult<()> {
    if name.contains(|c: char| c.is_ascii_whitespace() || matches!(c, '\0' | '>')) {
        Err(invalid_character())
    } else {
        Ok(())
    }
}

/// `createElement(localName)` name check.
pub fn validate_element_name(name: &str) -> DomResult<()> {
    if is_valid_element_local_name(name) {
        Ok(())
    } else {
        Err(invalid_character())
    }
}

/// `setAttribute(qualifiedName)` / `createAttribute(localName)` name check.
pub fn validate_attribute_name(name: &str) -> DomResult<()> {
    if is_valid_attribute_local_name(name) {
        Ok(())
    } else {
        Err(invalid_character())
    }
}

/// What a namespaced qualified name names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameContext {
    Element,
    Attribute,
}

/// DOM "validate and extract" for namespaced names.
pub fn validate_and_extract(namespace: Option<&str>, qualified: &str, context: NameContext) -> DomResult<QualName> {
    let ns = namespace.filter(|n| !n.is_empty()).unwrap_or("");
    let (prefix, local) = match qualified.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, qualified),
    };
    if prefix.is_some_and(|p| !is_valid_namespace_prefix(p)) {
        return Err(invalid_character());
    }
    let valid = match context {
        NameContext::Element => is_valid_element_local_name(local),
        NameContext::Attribute => is_valid_attribute_local_name(local),
    };
    if !valid {
        return Err(invalid_character());
    }
    const XML: &str = "http://www.w3.org/XML/1998/namespace";
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    let nserr = || DomError::new(DomErrorKind::Namespace, "invalid namespace for qualified name");
    if prefix.is_some() && ns.is_empty() {
        return Err(nserr());
    }
    if prefix == Some("xml") && ns != XML {
        return Err(nserr());
    }
    if (qualified == "xmlns" || prefix == Some("xmlns")) != (ns == XMLNS) {
        return Err(nserr());
    }
    Ok(QualName::new(prefix.map(Prefix::from), Namespace::from(ns), LocalName::from(local)))
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    /// The stack walks the stackless ones replace: children pushed last to
    /// first (mode 0) or first to last (mode 1) with the shadow root pushed
    /// last, or the shadow root pushed first and children last to first (2).
    fn stack_order(dom: &Dom, root: u32, mode: u8) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            out.push(i);
            let kids: Vec<u32> = dom.child_idx_iter(i).collect();
            let sr = dom.n(i).element().map_or(NIL, |e| e.shadow_root);
            if mode == 2 && sr != NIL {
                stack.push(sr);
            }
            if mode == 1 {
                stack.extend(kids);
            } else {
                stack.extend(kids.into_iter().rev());
            }
            if mode != 2 && sr != NIL {
                stack.push(sr);
            }
        }
        out
    }

    fn walk(dom: &Dom, root: u32, mode: u8) -> Vec<u32> {
        let mut out = Vec::new();
        let mut i = root;
        while i != NIL {
            out.push(i);
            i = match mode {
                0 => dom.next_shadow_including(i, root),
                1 => dom.next_shadow_including_rev(i, root),
                _ => dom.next_shadow_including_light_first(i, root),
            };
        }
        out
    }

    #[test]
    fn stackless_walks_keep_the_stack_orders() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "about:blank");
        let el = |dom: &mut Dom, name: &str| dom.create_element(doc, name).unwrap();
        let root = el(&mut dom, "div");
        let a = el(&mut dom, "section");
        let b = el(&mut dom, "span");
        let c = el(&mut dom, "p");
        let host = el(&mut dom, "div");
        dom.append_child(root, a).unwrap();
        dom.append_child(root, host).unwrap();
        dom.append_child(root, c).unwrap();
        dom.append_child(a, b).unwrap();
        let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let s1 = el(&mut dom, "slot");
        let s2 = el(&mut dom, "em");
        let inner_host = el(&mut dom, "span");
        dom.append_child(sr, s1).unwrap();
        dom.append_child(sr, inner_host).unwrap();
        dom.append_child(s1, s2).unwrap();
        let inner = dom.attach_shadow(inner_host, ShadowRootMode::Closed).unwrap();
        let deep = el(&mut dom, "i");
        dom.append_child(inner, deep).unwrap();
        let light = el(&mut dom, "u");
        dom.append_child(host, light).unwrap();
        let t = dom.create_text(doc, "x").unwrap();
        dom.append_child(light, t).unwrap();
        for start in [root, a, host, c, light] {
            let r = dom.check(start).unwrap();
            for mode in [0, 1, 2] {
                assert_eq!(walk(&dom, r, mode), stack_order(&dom, r, mode), "start {start:?} mode {mode}");
            }
        }
        // root, section, span, host, its shadow root, slot, em, the inner
        // host, its shadow root, i, u, the text, p.
        assert_eq!(walk(&dom, dom.check(root).unwrap(), 0).len(), 13);
    }
}

/// The document an iframe shows, as the render snapshot hands it to the renderer: the iframe's
/// `srcdoc` (its render HTML, frames in it with theirs), the URL its relative URLs resolve against,
/// and its viewport's scroll position (CSS px).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameDocument {
    pub document_url: Option<String>,
    pub html: String,
    pub url: String,
    pub scroll: (f64, f64),
    /// pyro: the frame's id (CDP frame id): the client the renderer's requests for the document
    /// name, so they go out as that frame's, with its origin (opaque in a sandboxed frame).
    pub client: String,
    /// pyro: the frame table of this document's own frames ([`Dom::frame_table`] of its DOM), by
    /// the index its markup names them with.
    pub frames: FrameTable,
    /// pyro: the texts of this document's own linked sheets, by URL: served to it alone (never
    /// merged with another document's sheets at the same URL).
    pub sheets: std::collections::BTreeMap<String, String>,
}

/// pyro: a document's frame table: its frame elements' documents by index, and the nonce its
/// markers carry ([`Dom::frame_table`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameTable {
    pub nonce: String,
    pub entries: Vec<(String, FrameDocument)>,
}

impl FrameDocument {
    /// pyro: changes when what the renderer loads for the frame does (not its scroll position).
    pub fn version(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        self.html.hash(&mut h);
        self.url.hash(&mut h);
        self.document_url.hash(&mut h);
        self.client.hash(&mut h);
        self.sheets.hash(&mut h);
        for (index, frame) in &self.frames.entries {
            index.hash(&mut h);
            frame.version().hash(&mut h);
        }
        h.finish()
    }
}

/// The policy token a `<meta name=referrer content>` value names: a
/// referrer policy, ASCII case-insensitive (the empty string is none), or
/// one of the legacy keywords `never`, `default`, `always` and
/// `origin-when-crossorigin`.
fn meta_referrer_policy(content: &str) -> Option<&'static str> {
    const POLICIES: [&str; 8] = [
        "no-referrer",
        "no-referrer-when-downgrade",
        "same-origin",
        "origin",
        "strict-origin",
        "origin-when-cross-origin",
        "strict-origin-when-cross-origin",
        "unsafe-url",
    ];
    const LEGACY: [(&str, &str); 4] = [
        ("never", "no-referrer"),
        ("default", "strict-origin-when-cross-origin"),
        ("always", "unsafe-url"),
        ("origin-when-crossorigin", "origin-when-cross-origin"),
    ];
    let token = content.trim_matches(|c: char| c.is_ascii_whitespace());
    LEGACY
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(token))
        .map(|(_, policy)| *policy)
        .or_else(|| POLICIES.iter().find(|p| p.eq_ignore_ascii_case(token)).copied())
}
