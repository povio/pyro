//! Versioned storage for the nodes of the DOM tree.

use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    /// Bumped whenever box geometry may have moved (layout ran or a scroll
    /// offset changed). Values derived from `final_layout()` (e.g. hoisted
    /// paint child offsets) are memoised against it. Starts at 1 so that a
    /// stamp of 0 is never current.
    geometry_generation: AtomicU64,
    /// The number of nodes in the map whose primary style is
    /// `position: sticky`. Maintained by every writer of a primary style
    /// ([`NodeTree::note_primary_style_change`]) and by [`NodeTree::remove`].
    /// Atomic because the style traversal writes styles in parallel through
    /// `&Node`.
    sticky_count: AtomicUsize,
    /// Scroll offset of the viewport (the root element scrolls the viewport,
    /// so its offset lives here rather than on the node).
    viewport_scroll: crate::Point<f64>,
}

impl NodeTree {
    pub(crate) fn new() -> Self {
        Self {
            map: SlotMap::with_key(),
            container_query_sizes: Mutex::new(HashMap::new()),
            geometry_generation: AtomicU64::new(1),
            sticky_count: AtomicUsize::new(0),
            viewport_scroll: crate::Point::ZERO,
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

    /// Whether any node in the map has a `position: sticky` primary style.
    pub fn has_sticky(&self) -> bool {
        self.sticky_count.load(Ordering::Relaxed) != 0
    }

    /// The number of nodes in the map with a `position: sticky` primary
    /// style (see the `sticky_count` field).
    pub fn sticky_count(&self) -> usize {
        self.sticky_count.load(Ordering::Relaxed)
    }

    /// Record that a node's primary style changed from `old` to `new`.
    /// Every writer of a primary style of a node in the map calls this.
    pub(crate) fn note_primary_style_change(
        &self,
        old: Option<&style::properties::ComputedValues>,
        new: Option<&style::properties::ComputedValues>,
    ) {
        self.note_sticky_change(old.is_some_and(is_sticky), new.is_some_and(is_sticky));
    }

    /// Like [`NodeTree::note_primary_style_change`], given whether the old and
    /// new primary styles are sticky.
    pub(crate) fn note_sticky_change(&self, old: bool, new: bool) {
        match (old, new) {
            (false, true) => {
                self.sticky_count.fetch_add(1, Ordering::Relaxed);
            }
            (true, false) => {
                self.sticky_count.fetch_sub(1, Ordering::Relaxed);
            }
            _ => {}
        }
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
        let key = self.map.insert_with_key(|key| f(to_id(key)));
        // New nodes start unstyled (their primary style is written later, by
        // a writer that counts it), so they never change `sticky_count` here.
        debug_assert!(self.map[key].primary_styles().is_none_or(|s| !is_sticky(&s)));
        to_id(key)
    }

    /// Remove the node with the given id, returning it if it was still live.
    pub(crate) fn remove(&mut self, id: NodeId) -> Option<Node> {
        self.container_query_sizes.get_mut().unwrap().remove(&id);
        let node = self.map.remove(to_key(id));
        if let Some(node) = &node {
            let style = node.primary_styles();
            self.note_primary_style_change(style.as_deref().map(|s| &**s), None);
        }
        node
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

/// Whether `style` is `position: sticky`. `position` is not inherited, so
/// anonymous boxes (whose styles inherit from their container) never are.
pub(crate) fn is_sticky(style: &style::properties::ComputedValues) -> bool {
    style.clone_position() == style::computed_values::position::T::Sticky
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
