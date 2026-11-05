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
    /// (diagnostics: `LIGHTNINGPANDA_FLOW_STATS`).
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

/// Per root (slot index), the first element in tree order for each id.
type IdIndex = rustc_hash::FxHashMap<u32, RootIds>;

/// One root's id map and what it is valid for.
struct RootIds {
    /// Slot generation of the root it was built for.
    gen: u32,
    /// `Dom::version` it is known valid at.
    version: u64,
    /// Nodes the build walked: the most journal entries worth scanning
    /// instead of rebuilding.
    walked: usize,
    ids: rustc_hash::FxHashMap<String, u32>,
}

enum IdIndexCheck {
    Valid,
    /// More changes than the root's walk: cheaper to rebuild just this root.
    TooLong,
    /// An id-affecting change or a journal gap since then.
    Changed,
}

pub struct Dom {
    /// Process-unique id (caches keyed by DOM identity and `version`).
    uid: u64,
    slots: Vec<Slot>,
    free: Vec<u32>,
    /// Bumped on every tree, attribute or character-data mutation. Live
    /// collections compare against it to reuse cached results.
    version: u64,
    /// See `tree_version()`.
    tree_version: u64,
    /// Roots of trees that may have become unreachable.
    candidates: Vec<u32>,
    /// A `<base>` element was created; until then no document walks for one.
    pub(crate) base_seen: bool,
    /// Documents with a computed `html_base`, reset on the next mutation.
    html_base_docs: std::cell::RefCell<Vec<NodeId>>,
    live: u32,
    /// Bumped whenever a node is removed from its parent.
    removals: u64,
    /// The latest removals, oldest first: the removed subtree's root, or NIL for a deactivated document.
    removal_log: VecDeque<u32>,
    /// What each `version` bump changed, for caches that can update
    /// selectively (style invalidation). Bounded: see `changes_since`.
    journal: Vec<(u64, Change)>,
    /// Versions at or below this have no complete journal.
    journal_floor: u64,
    /// Old values of attributes selectors read, from their first change
    /// since the style cascade last read styles (Stylo's invalidation
    /// snapshots). `None` until a cascade asks for them, so a page that
    /// never reads styles pays nothing.
    style_snapshots: std::cell::RefCell<Option<StyleSnapshots>>,
    /// Custom element names whose definitions disable shadow roots.
    shadow_disabled: Vec<Box<str>>,
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
    /// Modal dialogs and showing popovers (`top_layer`).
    pub(crate) top_layer: crate::top_layer::TopLayer,
    /// Nonces hidden from content attributes (`nonce`).
    pub(crate) nonces: crate::nonce::NonceState,
    /// Natural sizes of loaded `<img>` elements, set by the embedder.
    natural_sizes: rustc_hash::FxHashMap<NodeId, (f64, f64)>,
    /// `getElementById` index (see `get_element_by_id`).
    id_index: std::cell::RefCell<IdIndex>,
    /// Named access on the Window, per document (see `window_names`).
    pub(crate) window_names: std::cell::RefCell<rustc_hash::FxHashMap<u32, crate::window_names::WindowNames>>,
    /// Attr nodes of an element's attributes, by element slot (see `attr_node`).
    pub(crate) attr_nodes: rustc_hash::FxHashMap<u32, Vec<u32>>,
    #[cfg(test)]
    id_index_builds: std::cell::Cell<u64>,
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
            free: Vec::new(),
            version: 0,
            tree_version: 0,
            candidates: Vec::new(),
            live: 0,
            removals: 0,
            removal_log: VecDeque::new(),
            journal: Vec::new(),
            journal_floor: 0,
            style_snapshots: Default::default(),
            shadow_disabled: Vec::new(),
            shadow_roots: 0,
            pending_inline_handlers: Vec::new(),
            base_seen: false,
            html_base_docs: Default::default(),
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
            top_layer: Default::default(),
            nonces: Default::default(),
            natural_sizes: Default::default(),
            id_index: Default::default(),
            window_names: Default::default(),
            attr_nodes: Default::default(),
            #[cfg(test)]
            id_index_builds: Default::default(),
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
    #[inline]
    pub(crate) fn id_of(&self, idx: u32) -> NodeId {
        NodeId { idx, gen: self.slots[idx as usize].gen }
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

    pub(crate) fn generation_at(&self, idx: u32) -> u32 {
        self.slots.get(idx as usize).map_or(0, |s| s.gen)
    }
    pub(crate) fn slot_gen(&self, idx: u32) -> u32 {
        self.slots[idx as usize].gen
    }

    pub(crate) fn slot_live(&self, idx: u32) -> bool {
        self.slots.get(idx as usize).is_some_and(|s| s.node.is_some())
    }

    pub fn is_alive(&self, id: NodeId) -> bool {
        self.check(id).is_ok()
    }
    pub fn node(&self, id: NodeId) -> DomResult<&Node> {
        let i = self.check(id)?;
        Ok(self.n(i))
    }
    /// Natural (intrinsic) size of a loaded image element, in CSS px.
    pub fn natural_size(&self, el: NodeId) -> Option<(f64, f64)> {
        self.check(el).ok()?;
        self.natural_sizes.get(&el).copied()
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
    pub fn version(&self) -> u64 {
        self.version
    }
    /// Process-unique identity of this DOM (with `version`, a cache key
    /// that changes on every mutation).
    pub fn uid(&self) -> u64 {
        self.uid
    }
    #[track_caller]
    pub(crate) fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
        if self.base_seen {
            self.reset_html_bases();
        }
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

    /// Refines what the latest `version` bump changed (replacing its
    /// `Unknown` entry).
    pub(crate) fn note(&mut self, change: Change) {
        match self.journal.last_mut() {
            Some((v, c)) if *v == self.version => *c = change,
            _ => {}
        }
    }

    /// Adds another change to the latest `version` bump (after `note`).
    fn note_more(&mut self, change: Change) {
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
        let mut stack: Vec<u32> = Vec::new();
        for &root in self.removal_log.iter().skip(self.removal_log.len() - count as usize) {
            if root == NIL {
                return None;
            }
            stack.push(root);
            while let Some(i) = stack.pop() {
                if !self.slot_live(i) {
                    continue;
                }
                nodes.push(self.id_of(i));
                stack.extend(self.child_idx_iter(i));
                if let Some(e) = self.n(i).element() {
                    if e.shadow_root != NIL {
                        stack.push(e.shadow_root);
                    }
                }
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
        let node = Node::new(kind, owner_doc);
        let idx = if let Some(idx) = self.free.pop() {
            self.slots[idx as usize].node = Some(node);
            idx
        } else {
            let idx = u32::try_from(self.slots.len()).ok().filter(|&i| i != NIL).expect("DOM node limit");
            self.slots.push(Slot { gen: 1, node: Some(node) });
            idx
        };
        self.live += 1;
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
            kind,
            quirks: QuirksMode::NoQuirks,
            content_type: content_type.into(),
            charset: "UTF-8",
            url: url.into(),
            base_url: None,
            html_base: Default::default(),
            inert_doc: NIL,
            owned: 0,
            active: false,
            allow_declarative_shadow_roots: false,
            focused: NIL,
            hovered: NIL,
        };
        let idx = self.alloc(NodeKind::Document(Box::new(data)), NIL);
        // A document owns itself.
        self.n_mut(idx).owner_doc = idx;
        self.doc_data_mut(idx).owned = 1;
        self.id_of(idx)
    }

    /// Marks a document as displayed by a browsing context; active documents
    /// and everything reachable from them are never collected.
    pub fn set_document_active(&mut self, doc: NodeId, active: bool) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.active = active,
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

    pub fn document_data(&self, doc: NodeId) -> DomResult<&DocumentData> {
        match &self.node(doc)?.kind {
            NodeKind::Document(d) => Ok(d),
            _ => Err(DomError::Type("not a document")),
        }
    }
    pub fn set_document_url(&mut self, doc: NodeId, url: &str) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.url = url.into(),
            _ => return Err(DomError::Type("not a document")),
        }
        self.reset_html_bases();
        Ok(())
    }
    fn reset_html_bases(&mut self) {
        for doc in std::mem::take(self.html_base_docs.get_mut()) {
            if let Ok(i) = self.check(doc) {
                if let NodeKind::Document(d) = &mut self.n_mut(i).kind {
                    d.html_base = Default::default();
                }
            }
        }
    }
    /// Sets an inherited, already-resolved base URL without changing URL/origin.
    /// None restores resolution against the document's current URL.
    pub fn set_document_base_url(&mut self, doc: NodeId, base: Option<&str>) -> DomResult<()> {
        let i = self.check(doc)?;
        match &mut self.n_mut(i).kind {
            NodeKind::Document(d) => d.base_url = base.map(Into::into),
            _ => return Err(DomError::Type("not a document")),
        }
        self.reset_html_bases();
        Ok(())
    }
    /// The document base URL: its first `<base href>`, else the fallback.
    pub fn document_base_url(&self, doc: NodeId) -> DomResult<&str> {
        let d = self.document_data(doc)?;
        if self.base_seen {
            let html_base = d.html_base.get_or_init(|| {
                self.html_base_docs.borrow_mut().push(doc);
                self.first_base_href(doc)
            });
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
        Ok(())
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

    /// Creates an element without name validation (parser / internal use).
    pub fn create_element_raw(&mut self, doc: NodeId, name: QualName) -> DomResult<NodeId> {
        let owner = self.owner_for(doc)?;
        let is_template = name.ns == ns!(html) && name.local == local_name!("template");
        self.base_seen |= name.local == local_name!("base");
        let idx = self.alloc_raw(NodeKind::Element(Box::new(ElementData::new(name))), owner);
        if is_template {
            let inert = self.inert_doc_for(owner);
            let frag = self.alloc(NodeKind::Fragment(FragmentData { host: idx, shadow: None, flags: 0 }), inert);
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
            (LocalName::from(local.to_ascii_lowercase()), ns!(html))
        } else {
            // XML documents: "application/xhtml+xml" gets the HTML namespace.
            let ns = if &*self.doc_data(owner).content_type == "application/xhtml+xml" { ns!(html) } else { ns!() };
            (LocalName::from(local), ns)
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
        Ok(self.alloc_id(NodeKind::Fragment(FragmentData { host: NIL, shadow: None, flags: 0 }), owner))
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
        self.n_mut(inert).ext_refs += 1;
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
            ) || (el.name.local.contains('-')
                && !matches!(
                    &*el.name.local,
                    "annotation-xml"
                        | "color-profile"
                        | "font-face"
                        | "font-face-src"
                        | "font-face-uri"
                        | "font-face-format"
                        | "font-face-name"
                        | "missing-glyph"
                )));
        if !valid_host {
            return Err(DomError::new(DomErrorKind::NotSupported, "This element does not support attachShadow"));
        }
        if self.shadow_disabled.iter().any(|name| **name == *el.name.local) {
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

    fn new_shadow_root(&mut self, h: u32, mode: ShadowRootMode, flags: u8) -> u32 {
        let owner = self.n(h).owner_doc;
        let sr = self.alloc(NodeKind::Fragment(FragmentData { host: h, shadow: Some(mode), flags }), owner);
        self.n_mut(h).element_mut().unwrap().shadow_root = sr;
        self.shadow_roots = self.shadow_roots.saturating_add(1);
        self.bump();
        sr
    }

    /// The parser's `<template shadowrootmode>`: `None` leaves an ordinary
    /// template (not a valid host, or already a shadow host of any kind).
    pub(crate) fn attach_declarative_shadow(&mut self, h: u32, mode: ShadowRootMode, flags: u8) -> Option<u32> {
        if self.n(h).element()?.shadow_root != NIL {
            return None;
        }
        let id = self.id_of(h);
        self.attach_shadow_with(id, mode, flags | shadow_flags::DECLARATIVE).ok().map(|sr| sr.idx)
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
        self.shadow_disabled.push(name.into());
    }

    pub fn set_allow_declarative_shadow_roots(&mut self, doc: NodeId, allow: bool) -> DomResult<()> {
        let d = self.owner_for(doc)?;
        self.doc_data_mut(d).allow_declarative_shadow_roots = allow;
        Ok(())
    }
    pub(crate) fn allows_declarative_shadow_roots(&self, doc: u32) -> bool {
        self.doc_data(doc).allow_declarative_shadow_roots
    }

    // ----- slots -------------------------------------------------------

    pub(crate) fn is_slot(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.is_html() && &*e.name.local == "slot")
    }

    pub(crate) fn in_shadow_tree(&self, i: u32) -> bool {
        matches!(&self.n(self.root_idx(i)).kind, NodeKind::Fragment(f) if f.shadow.is_some())
    }

    /// DOM "find a slot" (named assignment; manual slots hold no state yet).
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
        if f.flags & shadow_flags::MANUAL_SLOTS != 0 || (open_only && f.shadow == Some(ShadowRootMode::Closed)) {
            return None;
        }
        let name = self.n(i).element().and_then(|e| e.attr("slot")).unwrap_or("");
        let mut cur = self.n(sr).first_child;
        while cur != NIL {
            if self.is_slot(cur) && self.n(cur).element().and_then(|e| e.attr("name")).unwrap_or("") == name {
                return Some(cur);
            }
            cur = self.following_in(cur, sr);
        }
        None
    }

    /// `Slottable.assignedSlot`.
    pub fn assigned_slot(&self, node: NodeId) -> DomResult<Option<NodeId>> {
        let i = self.check(node)?;
        Ok(self.find_slot(i, true).map(|s| self.id_of(s)))
    }

    pub(crate) fn slot_assigned(&self, slot: u32) -> Vec<u32> {
        let host = match &self.n(self.root_idx(slot)).kind {
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
        Ok(self.n(i).child_count)
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
        if index >= n.child_count {
            return Ok(None);
        }
        if index <= n.child_count / 2 {
            let mut c = n.first_child;
            for _ in 0..index {
                c = self.n(c).next;
            }
            Ok(Some(self.id_of(c)))
        } else {
            let mut c = n.last_child;
            for _ in 0..(n.child_count - 1 - index) {
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
    pub(crate) fn is_connected_idx(&self, mut i: u32) -> bool {
        loop {
            i = self.root_idx(i);
            match &self.n(i).kind {
                NodeKind::Document(_) => return true,
                NodeKind::Fragment(FragmentData { host, shadow: Some(_), .. }) => i = *host,
                _ => return false,
            }
        }
    }

    /// Connected to a document that a browsing context currently displays.
    pub fn is_in_active_document(&self, id: NodeId) -> bool {
        let Ok(mut i) = self.check(id) else { return false };
        loop {
            i = self.root_idx(i);
            match &self.n(i).kind {
                NodeKind::Document(d) => return d.active,
                NodeKind::Fragment(FragmentData { host, shadow: Some(_), .. }) => i = *host,
                _ => return false,
            }
        }
    }

    /// `Node.isConnected` (shadow-including).
    pub fn is_connected(&self, id: NodeId) -> DomResult<bool> {
        let root = self.root_node(id, true)?;
        Ok(matches!(self.n(root.idx).kind, NodeKind::Document(_)))
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
    fn is_host_including_inclusive_ancestor(&self, anc: u32, mut node: u32) -> bool {
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

    /// True if `x` precedes `y` in tree order (same tree assumed).
    pub(crate) fn tree_order_before(&self, x: u32, y: u32) -> bool {
        let path = |mut n: u32| {
            let mut v = Vec::with_capacity(16);
            while n != NIL {
                v.push(n);
                n = self.n(n).parent;
            }
            v.reverse();
            v
        };
        let px = path(x);
        let py = path(y);
        let mut k = 0;
        while k < px.len() && k < py.len() && px[k] == py[k] {
            k += 1;
        }
        if k == px.len() {
            return true; // x is ancestor of y
        }
        if k == py.len() {
            return false;
        }
        // Siblings px[k] and py[k]: walk forward from px[k].
        let mut c = self.n(px[k]).next;
        while c != NIL {
            if c == py[k] {
                return true;
            }
            c = self.n(c).next;
        }
        false
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
        let added: Vec<u32> = match n {
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
        if !added.is_empty() || !removed.is_empty() {
            self.queue_child_list(p, &added, &removed, NIL, NIL);
        }
        Ok(())
    }

    /// Core insert: `node` (or fragment children) into `parent` before `child`.
    fn insert(&mut self, node: u32, parent: u32, child: u32, suppress: bool) {
        let nodes: Vec<u32> = if matches!(self.n(node).kind, NodeKind::Fragment(_)) {
            let kids: Vec<u32> = self.child_idx_iter(node).collect();
            for &k in &kids {
                self.remove(k, true);
            }
            if !kids.is_empty() {
                self.queue_child_list(node, &[], &kids, NIL, NIL);
            }
            kids
        } else {
            if self.n(node).parent != NIL {
                self.remove(node, false);
            }
            vec![node]
        };
        if nodes.is_empty() {
            return;
        }
        if child != NIL && !self.live_ranges.is_empty() {
            self.ranges_before_insert(parent, child, nodes.len() as u32);
        }
        let prev = if child == NIL { self.n(parent).last_child } else { self.n(child).prev };
        let doc = self.n(parent).owner_doc;
        for &k in &nodes {
            self.adopt_idx(k, doc);
            self.link(k, parent, child);
        }
        let connected = self.is_connected_idx(parent);
        self.script_children_inserted(parent);
        for &k in &nodes {
            self.resources_after_insert(k);
            self.picture_images_changed(parent, k);
            if connected {
                self.note_connected_listeners(k);
                self.enqueue_custom_reactions(k, crate::custom::CustomReaction::Connected);
                self.upgrades_after_insert(k);
            }
        }
        self.slots_after_child_change(parent, &nodes);
        self.bump_tree();
        self.note(Change::Inserted(parent, nodes[0]));
        for &k in &nodes[1..] {
            self.note_more(Change::Inserted(parent, k));
        }
        if !suppress {
            self.queue_child_list(parent, &nodes, &[], prev, child);
        }
        if connected {
            for &k in &nodes {
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
        }
        if prev == NIL {
            self.n_mut(parent).first_child = node;
        } else {
            self.n_mut(prev).next = node;
        }
        if before == NIL {
            self.n_mut(parent).last_child = node;
        } else {
            self.n_mut(before).prev = node;
        }
        self.n_mut(parent).child_count += 1;
    }

    fn unlink(&mut self, node: u32) {
        let (parent, prev, next) = {
            let n = self.n(node);
            (n.parent, n.prev, n.next)
        };
        if prev == NIL {
            self.n_mut(parent).first_child = next;
        } else {
            self.n_mut(prev).next = next;
        }
        if next == NIL {
            self.n_mut(parent).last_child = prev;
        } else {
            self.n_mut(next).prev = prev;
        }
        self.n_mut(parent).child_count -= 1;
        let n = self.n_mut(node);
        n.parent = NIL;
        n.prev = NIL;
        n.next = NIL;
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
        if self.reactions.any_custom && self.is_connected_idx(parent) {
            self.enqueue_custom_reactions(node, crate::custom::CustomReaction::Disconnected);
        }
        self.note_removal(node);
        self.add_transient_observers(parent, node);
        if !self.live_ranges.is_empty() {
            self.ranges_before_remove(node);
        }
        if !self.live_iterators.is_empty() {
            self.iterators_before_remove(node);
        }
        self.unlink(node);
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
        while let Some(i) = stack.pop() {
            if self.n(i).owner_doc == old {
                self.n_mut(i).owner_doc = doc;
                moved += 1;
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
        self.doc_data_mut(old).owned -= moved;
        self.doc_data_mut(doc).owned += moved;
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
        let c = self.clone_idx(n, d, deep)?;
        Ok(self.id_of(c))
    }

    fn clone_idx(&mut self, n: u32, doc: u32, deep: bool) -> DomResult<u32> {
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
                nd
            }
            NodeKind::Element(e) => {
                let name = e.name.clone();
                let attrs = e.attrs.clone();
                let control = e.control.as_ref().map(|c| {
                    // Clone steps copy value/checkedness for input elements.
                    Box::new(ControlState { selection: (0, 0), ..(**c).clone() })
                });
                // HTML script cloning steps: "already started" is copied.
                let started = e.flags & crate::node::element_flags::SCRIPT_ALREADY_STARTED;
                let id = self.create_element_raw(self.id_of(doc), name)?;
                let el = self.n_mut(id.idx).element_mut().unwrap();
                el.attrs = attrs;
                el.control = control;
                el.flags |= started;
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
            NodeKind::Fragment(_) => {
                self.alloc(NodeKind::Fragment(FragmentData { host: NIL, shadow: None, flags: 0 }), doc)
            }
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
                let kc = self.clone_idx(k, child_doc, true)?;
                self.link(kc, copy, NIL);
            }
            // Template contents are cloned too (HTML "cloning steps").
            let src_tc = self.n(n).element().map_or(NIL, |e| e.template_contents);
            let dst_tc = self.n(copy).element().map_or(NIL, |e| e.template_contents);
            if src_tc != NIL && dst_tc != NIL {
                let inert = self.n(dst_tc).owner_doc;
                let kids: Vec<u32> = self.child_idx_iter(src_tc).collect();
                for k in kids {
                    let kc = self.clone_idx(k, inert, true)?;
                    self.link(kc, dst_tc, NIL);
                }
            }
        }
        // A clonable shadow tree comes along, even in a shallow clone.
        let src_sr = self.n(n).element().map_or(NIL, |e| e.shadow_root);
        if src_sr != NIL {
            if let NodeKind::Fragment(FragmentData { shadow: Some(mode), flags, .. }) = self.n(src_sr).kind {
                if flags & shadow_flags::CLONABLE != 0 {
                    let dst_sr = self.new_shadow_root(copy, mode, flags);
                    let kids: Vec<u32> = self.child_idx_iter(src_sr).collect();
                    for k in kids {
                        let kc = self.clone_idx(k, doc, true)?;
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
        let i = self.check(id)?;
        self.n(i).element().map(|e| (i, e)).ok_or(DomError::Type("not an element"))
    }
    pub fn element(&self, id: NodeId) -> DomResult<&ElementData> {
        Ok(self.el(id)?.1)
    }

    fn html_lower<'a>(&self, el: &ElementData, owner: u32, name: &'a str) -> std::borrow::Cow<'a, str> {
        if el.is_html()
            && self.doc_data(owner).kind == DocumentKind::Html
            && name.bytes().any(|b| b.is_ascii_uppercase())
        {
            std::borrow::Cow::Owned(name.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(name)
        }
    }

    fn find_attr_qualified(&self, i: u32, name: &str) -> Option<usize> {
        let el = self.n(i).element()?;
        let name = self.html_lower(el, self.n(i).owner_doc, name);
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

    /// The content attribute `name` as the element's own behaviour reads it
    /// (reflection, form and loading state): the attribute in no namespace
    /// with that local name, so `setAttributeNS(ns, "src", ..)` is not the
    /// element's `src`. A prefixed name (`xlink:href`) is a qualified name.
    fn find_attr_content(&self, i: u32, name: &str) -> Option<usize> {
        let el = self.n(i).element()?;
        let local = self.html_lower(el, self.n(i).owner_doc, name);
        let found = el.attrs.iter().position(|a| a.name.ns == ns!() && *a.name.local == *local);
        match found {
            None if name.contains(':') => self.find_attr_qualified(i, name),
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
        let (i, el) = self.el(id)?;
        match self.find_attr_qualified(i, name) {
            Some(k) => self.change_attr(i, k, value),
            None => {
                let local = self.html_lower(el, self.n(i).owner_doc, name);
                let qn = QualName::new(None, ns!(), LocalName::from(&*local));
                self.append_attr(i, qn, value);
            }
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

    pub(crate) fn change_attr(&mut self, i: u32, k: usize, value: &str) {
        let el = self.n(i).element().unwrap();
        let name = el.attrs[k].name.clone();
        let old = std::mem::replace(&mut self.n_mut(i).element_mut().unwrap().attrs[k].value, value.into());
        if *old != *value {
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
        if !drives_element_behaviour(&name) {
            return;
        }
        self.resources_after_attr(i, &name.local, false);
        if *old != *value {
            self.slots_after_attr_change(i, &name.local);
        }
    }
    pub(crate) fn append_attr(&mut self, i: u32, name: QualName, value: &str) {
        self.note_style_attr(i, &name, None);
        self.n_mut(i).element_mut().unwrap().attrs.push(Attr { name: name.clone(), value: value.into() });
        self.bump_tree();
        self.note(Change::attr(i, &name));
        self.queue_attribute(i, &name, None);
        if !drives_element_behaviour(&name) {
            return;
        }
        self.slots_after_attr_change(i, &name.local);
        self.resources_after_attr(i, &name.local, true);
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
        let attr = self.n_mut(i).element_mut().unwrap().attrs.remove(k);
        if !self.attr_nodes.is_empty() {
            self.attr_removed(i, &attr.name, &attr.value);
        }
        self.note_style_attr(i, &attr.name, Some(&attr.value));
        self.bump_tree();
        self.note(Change::attr(i, &attr.name));
        self.queue_attribute(i, &attr.name, Some(&attr.value));
        if !drives_element_behaviour(&attr.name) {
            return;
        }
        self.resources_after_attr(i, &attr.name.local, false);
        self.slots_after_attr_change(i, &attr.name.local);
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
        let i = self.check(id)?;
        Ok(match &self.n(i).kind {
            NodeKind::Document(_) | NodeKind::Doctype(_) => None,
            NodeKind::Attr(_) => self.attr_value_idx(i).map(str::to_owned),
            k if k.char_data().is_some() => Some(k.char_data().unwrap().to_owned()),
            _ => {
                let mut out = String::new();
                self.collect_text(i, &mut out);
                Some(out)
            }
        })
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

    /// `getElementById` over the light tree rooted at `root` (tree order).
    pub fn get_element_by_id(&self, root: NodeId, id: &str) -> DomResult<Option<NodeId>> {
        if id.is_empty() {
            return Ok(None);
        }
        let r = self.check(root)?;
        // Chromium keeps an id map; a tree walk per call made accessible-name
        // computation (aria-labelledby/-describedby lookups) quadratic on
        // large pages. The index is rebuilt lazily after a mutation that can
        // change it.
        let mut index = self.id_index.borrow_mut();
        // A freed root's slot can be reused by a new node without a mutation
        // being journaled: maps are only valid for the generation they were
        // built for. (A live root keeps its whole subtree, and every node that
        // leaves it is journaled as removed.)
        let gen = self.slots[r as usize].gen;
        let stale = match index.get(&r) {
            None => false,
            Some(e) if e.gen != gen => true,
            Some(e) if e.version == self.version => false,
            Some(e) => match self.id_index_check(e.version, e.walked) {
                IdIndexCheck::Valid => false,
                IdIndexCheck::TooLong => true,
                IdIndexCheck::Changed => {
                    // Every map validated at or before this one spans the same
                    // id-affecting change: drop them all, which also bounds
                    // maps of roots that are never looked up again.
                    let since = e.version;
                    index.retain(|_, o| o.version > since);
                    true
                }
            },
        };
        if stale {
            index.remove(&r);
        }
        let entry = index.entry(r).or_insert_with(|| self.build_id_map(r, gen));
        entry.version = self.version;
        Ok(entry.ids.get(id).map(|&i| self.id_of(i)))
    }

    fn build_id_map(&self, r: u32, gen: u32) -> RootIds {
        #[cfg(test)]
        self.id_index_builds.set(self.id_index_builds.get() + 1);
        let mut ids = rustc_hash::FxHashMap::default();
        let mut walked = 0;
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            walked += 1;
            if let Some(v) = self.n(cur).element().and_then(|e| e.id()) {
                if !v.is_empty() {
                    ids.entry(v.to_owned()).or_insert(cur);
                }
            }
            cur = self.following_in(cur, r);
        }
        RootIds { gen, version: self.version, walked, ids }
    }

    /// Whether an id map validated at `since` is still valid. The journal must
    /// cover every change since then, and none of them can add, remove or
    /// reorder an id: text, form-control state, focus/hover and attributes
    /// other than `id` keep it. Insertions, removals, `id` changes, unknown
    /// changes and journal gaps invalidate it. More changes than the nodes
    /// its build walked (`walked`) rebuild it too, so a scan never costs
    /// more than walking that root again.
    fn id_index_check(&self, since: u64, walked: usize) -> IdIndexCheck {
        let Some(changes) = self.changes_since(since) else { return IdIndexCheck::Changed };
        if changes.size_hint().1.is_none_or(|n| n > walked) {
            return IdIndexCheck::TooLong;
        }
        let id_hash = local_name!("id").get_hash();
        let mut changes = changes;
        let valid = changes.all(|c| match c {
            Change::Text(_) | Change::State(_) | Change::Pseudo => true,
            Change::Attr(_, hash, _) => hash != id_hash,
            Change::Inserted(..) | Change::Removed(..) | Change::Unknown(_) => false,
        });
        if valid {
            IdIndexCheck::Valid
        } else {
            IdIndexCheck::Changed
        }
    }

    /// `document.documentElement`.
    pub fn document_element(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        Ok(self.child_idx_iter(d).find(|&c| self.n(c).is_element()).map(|c| self.id_of(c)))
    }
    pub fn doctype(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let d = self.check(doc)?;
        Ok(self.child_idx_iter(d).find(|&c| matches!(self.n(c).kind, NodeKind::Doctype(_))).map(|c| self.id_of(c)))
    }
    fn html_child(&self, parent: u32, name: &LocalName) -> Option<u32> {
        self.child_idx_iter(parent).find(|&c| self.n(c).element().is_some_and(|e| e.is_html() && &e.name.local == name))
    }
    /// `document.head`.
    pub fn head(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let Some(html) = self.document_element(doc)? else { return Ok(None) };
        Ok(self.html_child(html.idx, &local_name!("head")).map(|c| self.id_of(c)))
    }
    /// `document.body` (first body or frameset child of the html element).
    pub fn body(&self, doc: NodeId) -> DomResult<Option<NodeId>> {
        let Some(html) = self.document_element(doc)? else { return Ok(None) };
        Ok(self
            .child_idx_iter(html.idx)
            .find(|&c| {
                self.n(c).element().is_some_and(|e| {
                    e.is_html() && (e.name.local == local_name!("body") || e.name.local == local_name!("frameset"))
                })
            })
            .map(|c| self.id_of(c)))
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
        self.n_mut(i).ext_refs += 1;
        Ok(())
    }
    /// Drops an external reference. Collection happens in `collect_garbage`.
    pub fn release(&mut self, id: NodeId) {
        if let Ok(i) = self.check(id) {
            let n = self.n_mut(i);
            debug_assert!(n.ext_refs > 0, "unbalanced release");
            n.ext_refs = n.ext_refs.saturating_sub(1);
            if n.ext_refs == 0 {
                self.candidates.push(i);
            }
        }
    }
    pub(crate) fn release_candidate(&mut self, idx: u32) {
        self.candidates.push(idx);
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

    fn component_nodes(&self, root: u32, out: &mut Vec<u32>) {
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            out.push(i);
            stack.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
                if e.template_contents != NIL {
                    stack.push(e.template_contents);
                }
                stack.extend_from_slice(self.attr_nodes_of(i));
            }
        }
    }

    /// Frees every detached component that has no external references, no
    /// pending mutation-observer interest and (for documents) no surviving
    /// nodes owned elsewhere. Call at a safe point (end of a task); never while
    /// a native caller still holds an unreferenced `NodeId` it intends to use.
    /// Returns the number of nodes freed.
    pub fn collect_garbage(&mut self) -> usize {
        let mut freed_total = 0;
        let mut seen: FxHashSet<u32> = FxHashSet::default();
        let mut nodes = Vec::new();
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
            if !seen.insert(root) {
                continue;
            }
            if let NodeKind::Document(d) = &self.n(root).kind {
                if d.active {
                    continue;
                }
            }
            nodes.clear();
            self.component_nodes(root, &mut nodes);
            if nodes.iter().any(|&i| self.n(i).ext_refs > 0 || self.mutation.pins(i)) {
                continue;
            }
            if matches!(self.n(root).kind, NodeKind::Document(_)) {
                let own = nodes.iter().filter(|&&i| self.n(i).owner_doc == root).count() as u32;
                if self.doc_data(root).owned != own {
                    continue; // nodes owned by this document are alive elsewhere
                }
            }
            freed_total += nodes.len();
            let batch = std::mem::take(&mut nodes);
            for &i in &batch {
                self.free_slot(i, root);
            }
            nodes = batch;
            // Freeing may orphan other documents; re-examine them.
            seen.clear();
        }
        freed_total
    }

    fn free_slot(&mut self, i: u32, component_root: u32) {
        let owner = self.n(i).owner_doc;
        let inert = match &self.n(i).kind {
            NodeKind::Document(d) if d.inert_doc != NIL && d.inert_doc != i => d.inert_doc,
            _ => NIL,
        };
        self.mutation.forget_node(i);
        self.events.forget(crate::events::TargetKey::Node(i));
        self.forget_attr_nodes(i);
        if !self.creation_docs.is_empty() {
            self.creation_docs.remove(&i);
        }
        let slot = &mut self.slots[i as usize];
        slot.node = None;
        slot.gen = slot.gen.wrapping_add(1).max(1);
        self.free.push(i);
        self.live -= 1;
        if owner != i && owner != NIL && self.slots[owner as usize].node.is_some() {
            self.doc_data_mut(owner).owned -= 1;
            if owner != component_root {
                self.candidates.push(owner);
            }
        }
        if inert != NIL && self.slots[inert as usize].node.is_some() {
            let n = self.n_mut(inert);
            n.ext_refs = n.ext_refs.saturating_sub(1);
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
    if is_valid_element_local_name(name) { Ok(()) } else { Err(invalid_character()) }
}

/// `setAttribute(qualifiedName)` / `createAttribute(localName)` name check.
pub fn validate_attribute_name(name: &str) -> DomResult<()> {
    if is_valid_attribute_local_name(name) { Ok(()) } else { Err(invalid_character()) }
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
mod id_index_tests {
    //! Reuse of the `getElementById` index, observed through the test-only
    //! build counter. Behaviour is covered differentially in
    //! tests/id_index.rs.
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::parse::{parse_document, DomRef};

    fn doc(html: &str) -> (DomRef, NodeId) {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let d = parse_document(&dom, "http://test/", html);
        dom.borrow_mut().set_document_active(d, true).unwrap();
        (dom, d)
    }

    /// Full-walk reference for one lookup.
    fn walk(dom: &Dom, root: NodeId, id: &str) -> Option<NodeId> {
        dom.descendants(root).unwrap().skip(1).find(|&k| {
            dom.node_type(k).unwrap() == NodeType::Element && dom.get_attribute_ns(k, None, "id").unwrap() == Some(id)
        })
    }

    #[test]
    fn the_index_is_reused_across_irrelevant_changes_and_rebuilt_otherwise() {
        let mut html = String::from("<body><input id=field>");
        for i in 0..200 {
            html.push_str(&format!("<div id=n{i}>x</div>"));
        }
        let (dom, d) = doc(&html);
        let mut dom = dom.borrow_mut();
        let field = dom.get_element_by_id(d, "field").unwrap().unwrap();
        let mid = dom.get_element_by_id(d, "n100").unwrap().unwrap();
        let text = dom.first_child(mid).unwrap().unwrap();
        let builds = dom.id_index_builds.get();
        let lookup = |dom: &Dom| assert_eq!(dom.get_element_by_id(d, "n199").unwrap(), walk(dom, d, "n199"));

        // One irrelevant change of each kind between lookups: no rebuild.
        for i in 0..50 {
            dom.set_attribute(mid, "data-x", &i.to_string()).unwrap();
            lookup(&dom);
            dom.set_char_data(text, &format!("y{i}")).unwrap();
            lookup(&dom);
            dom.set_value(field, &format!("v{i}")).unwrap();
            lookup(&dom);
            dom.set_focused(d, Some(if i % 2 == 0 { field } else { mid })).unwrap();
            lookup(&dom);
        }
        // Many irrelevant changes between two lookups, within the document's walk.
        for i in 0..150 {
            dom.set_attribute(mid, "class", &format!("k{i}")).unwrap();
        }
        lookup(&dom);
        assert_eq!(dom.id_index_builds.get(), builds, "irrelevant changes rebuilt the index");

        // Each id-affecting change rebuilds once.
        let mut expect = builds;
        let mut step = |dom: &mut Dom, f: &dyn Fn(&mut Dom)| {
            f(dom);
            lookup(dom);
            lookup(dom);
            expect += 1;
            assert_eq!(dom.id_index_builds.get(), expect);
        };
        step(&mut dom, &|dom| dom.set_attribute(mid, "id", "renamed").unwrap());
        step(&mut dom, &|dom| {
            dom.remove_attribute(mid, "id").unwrap();
        });
        step(&mut dom, &|dom| {
            let e = dom.create_element(d, "p").unwrap();
            let body = dom.body(d).unwrap().unwrap();
            dom.append_child(body, e).unwrap();
        });
        step(&mut dom, &|dom| dom.remove_node(mid).unwrap());
        // More changes than the document walk visited: rebuilt rather than scanned.
        step(&mut dom, &|dom| {
            for i in 0..2000 {
                dom.set_attribute(field, "data-y", &i.to_string()).unwrap();
            }
        });
        // Journal overflow hides an id change: rebuilt.
        step(&mut dom, &|dom| {
            dom.set_attribute(field, "id", "field2").unwrap();
            for i in 0..(JOURNAL_CAPACITY * 2) {
                dom.set_attribute(field, "data-z", &i.to_string()).unwrap();
            }
        });
        assert_eq!(dom.get_element_by_id(d, "field").unwrap(), None);
        assert_eq!(dom.get_element_by_id(d, "field2").unwrap(), Some(field));
    }

    #[test]
    fn a_tiny_root_rebuilds_instead_of_scanning_a_long_journal() {
        let mut html = String::from("<body>");
        for i in 0..200 {
            html.push_str(&format!("<div id=n{i}>x</div>"));
        }
        let (dom, d) = doc(&html);
        let mut dom = dom.borrow_mut();
        let mid = dom.get_element_by_id(d, "n100").unwrap().unwrap();
        let frag = dom.create_fragment(d).unwrap();
        let t = dom.create_element(d, "i").unwrap();
        dom.set_attribute(t, "id", "t").unwrap();
        dom.append_child(frag, t).unwrap();
        assert_eq!(dom.get_element_by_id(frag, "t").unwrap(), Some(t));
        assert_eq!(dom.get_element_by_id(d, "n199").unwrap(), walk(&dom, d, "n199"));
        let builds = dom.id_index_builds.get();
        // One irrelevant change: within both roots' bounds, both reused.
        dom.set_attribute(mid, "data-x", "0").unwrap();
        assert_eq!(dom.get_element_by_id(frag, "t").unwrap(), Some(t));
        assert_eq!(dom.get_element_by_id(d, "n199").unwrap(), walk(&dom, d, "n199"));
        assert_eq!(dom.id_index_builds.get(), builds);
        // More irrelevant changes than the fragment's walk (1 node) but fewer
        // than the document's: only the fragment rebuilds.
        for i in 0..50 {
            dom.set_attribute(mid, "data-x", &i.to_string()).unwrap();
        }
        assert_eq!(dom.get_element_by_id(frag, "t").unwrap(), Some(t));
        assert_eq!(dom.id_index_builds.get(), builds + 1);
        assert_eq!(dom.get_element_by_id(d, "n199").unwrap(), walk(&dom, d, "n199"));
        assert_eq!(dom.id_index_builds.get(), builds + 1);
        // An insertion invalidates both; the first lookup drops every map
        // validated at or before its own.
        let body = dom.body(d).unwrap().unwrap();
        let p = dom.create_element(d, "p").unwrap();
        dom.append_child(body, p).unwrap();
        assert_eq!(dom.get_element_by_id(d, "n199").unwrap(), walk(&dom, d, "n199"));
        assert!(!dom.id_index.borrow().contains_key(&frag.idx), "the fragment's map spans the insertion");
        assert_eq!(dom.get_element_by_id(frag, "t").unwrap(), Some(t));
        assert_eq!(dom.id_index_builds.get(), builds + 3);
    }
}
