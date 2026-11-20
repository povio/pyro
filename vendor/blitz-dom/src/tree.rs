//! Versioned storage for the nodes of the DOM tree.

use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use blitz_traits::node_id::NodeId;
use slotmap::{Key as _, KeyData, SlotMap};

use crate::Node;

slotmap::new_key_type! {
    /// The internal [`slotmap`] key for node storage. Only used at the
    /// storage boundary: all public APIs use [`NodeId`].
    struct NodeKey;
}

#[inline(always)]
fn to_key(id: NodeId) -> NodeKey {
    NodeKey::from(KeyData::from_ffi(id.as_u64()))
}

#[inline(always)]
fn to_id(key: NodeKey) -> NodeId {
    NodeId::from_u64(key.data().as_ffi())
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ContainerQuerySize {
    pub axes: euclid::default::Size2D<bool>,
    pub size: euclid::default::Size2D<Option<app_units::Au>>,
}

/// The versioned map in which the nodes of the DOM tree are stored, backed by
/// a [`slotmap::SlotMap`].
///
/// Nodes are addressed by [`NodeId`], which carries the slot's version in
/// addition to its index: when a node is dropped and its slot reused, ids
/// referring to the dropped node no longer resolve ([`NodeTree::get`] returns
/// `None`, and indexing panics) instead of aliasing the new occupant.
pub struct NodeTree {
    map: SlotMap<NodeKey, Node>,
    // Sparse: only containers actually queried by Stylo occupy an entry. Style
    // workers can ask about the same container concurrently.
    pub(crate) container_query_sizes: Mutex<HashMap<NodeId, ContainerQuerySize>>,
    pub(crate) anchor_query_choices: Mutex<HashMap<NodeId, Option<style::values::specified::position::PositionTryFallbacksItem>>>,
    pub(crate) anchor_query_samples: AtomicU64,
    /// Bumped whenever box geometry may have moved (layout ran or a scroll
    /// offset changed). Values derived from `final_layout()` (e.g. hoisted
    /// paint child offsets) are memoised against it. Starts at 1 so that a
    /// stamp of 0 is never current.
    geometry_generation: AtomicU64,
    pub(crate) anchor_clip_steps: AtomicU64,
    pub(crate) anchor_visibility: Mutex<(u64, HashMap<NodeId,bool>)>,
    pub(crate) anchor_offsets: Mutex<(u64, HashMap<NodeId,crate::Point<f64>>)>,
    /// Scroll offset of the viewport (the root element scrolls the viewport,
    /// so its offset lives here rather than on the node).
    viewport_scroll: crate::Point<f64>,
    /// pyro: [`crate::DocumentConfig::xml_document`].
    pub(crate) xml_document: bool,
    /// pyro: [`crate::DocumentConfig::quirks_mode`].
    pub(crate) quirks_mode: bool,
    /// pyro: [`crate::DocumentConfig::line_height_quirk`].
    pub(crate) line_height_quirk: bool,
    /// pyro: [`crate::DocumentConfig::content_language`].
    pub(crate) content_language: Option<Box<str>>,
    /// pyro: the styles of each shadow root, by host (`crate::shadow`).
    pub(crate) shadow_roots: HashMap<NodeId, crate::shadow::ShadowStyles>,
    /// pyro: an element of this tree has been a shadow host (`crate::shadow`): until then,
    /// every node is in the document's tree and its DOM parent is its parent.
    pub(crate) has_shadow_hosts: bool,
    /// pyro: the document's base URL without its fragment (`BaseDocument::url`), which a `url()`
    /// naming an element of this document (`url(#m)`) resolves to.
    pub(crate) base_url: String,
    /// pyro: the document lays out classic scrollbars (`stylo_taffy::scrollbars`): the process's
    /// mode when it was made, or [`crate::BaseDocument::set_classic_scrollbars`]'s.
    pub(crate) classic_scrollbars: bool,
}

impl NodeTree {
    pub(crate) fn new() -> Self {
        Self {
            map: SlotMap::with_key(),
            container_query_sizes: Mutex::new(HashMap::new()),
            anchor_query_choices: Default::default(),
            anchor_query_samples: Default::default(),
            geometry_generation: AtomicU64::new(1),
            anchor_clip_steps: AtomicU64::new(0),
            anchor_visibility: Default::default(),
            anchor_offsets: Default::default(),
            viewport_scroll: crate::Point::ZERO,
            xml_document: false,
            quirks_mode: false,
            line_height_quirk: false,
            content_language: None,
            shadow_roots: HashMap::new(),
            has_shadow_hosts: false,
            base_url: String::new(),
            classic_scrollbars: stylo_taffy::scrollbars::scrollbar_mode() == stylo_taffy::scrollbars::ScrollbarMode::Classic,
        }
    }

    pub fn viewport_scroll(&self) -> crate::Point<f64> {
        self.viewport_scroll
    }

    /// Memoised paint geometry may depend on the viewport scroll, so this
    /// bumps the geometry generation like any other scroll write.
    pub(crate) fn set_viewport_scroll(&mut self, scroll: crate::Point<f64>) {
        self.viewport_scroll = scroll;
        self.bump_geometry_generation();
    }

    /// See the `geometry_generation` field.
    pub fn geometry_generation(&self) -> u64 {
        self.geometry_generation.load(Ordering::Relaxed)
    }

    pub(crate) fn bump_geometry_generation(&self) {
        self.geometry_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of live nodes in the map.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether `id` resolves to a live node.
    pub fn contains_key(&self, id: NodeId) -> bool {
        self.map.contains_key(to_key(id))
    }

    /// Get a reference to the node with the given id, if it is still live.
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.map.get(to_key(id))
    }

    /// Get a mutable reference to the node with the given id, if it is still live.
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.map.get_mut(to_key(id))
    }

    /// Insert a node constructed with knowledge of its own id.
    pub(crate) fn insert_with_key(&mut self, f: impl FnOnce(NodeId) -> Node) -> NodeId {
        to_id(self.map.insert_with_key(|key| f(to_id(key))))
    }

    /// Remove the node with the given id, returning it if it was still live.
    pub(crate) fn remove(&mut self, id: NodeId) -> Option<Node> {
        self.container_query_sizes.get_mut().unwrap().remove(&id);
        self.anchor_query_choices.get_mut().unwrap().remove(&id);
        self.map.remove(to_key(id))
    }

    /// Iterate over all live `(NodeId, &Node)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.map.iter().map(|(key, node)| (to_id(key), node))
    }

    /// Iterate over all live `(NodeId, &mut Node)` pairs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (NodeId, &mut Node)> {
        self.map.iter_mut().map(|(key, node)| (to_id(key), node))
    }
}

impl Index<NodeId> for NodeTree {
    type Output = Node;

    #[track_caller]
    #[inline]
    fn index(&self, id: NodeId) -> &Node {
        &self.map[to_key(id)]
    }
}

impl IndexMut<NodeId> for NodeTree {
    #[track_caller]
    #[inline]
    fn index_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.map[to_key(id)]
    }
}
