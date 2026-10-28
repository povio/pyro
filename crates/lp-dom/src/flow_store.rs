//! Storage of a flow result: boxes by layout order and text entries in the
//! order they were laid out, each with an index by node (`NodeId::index`).
//! A pass appends in order, and a replayed subtree is one contiguous range
//! of each, so the next pass copies it as a slice and fixes positions and
//! orders in one linear sweep (no hashing). Buffers are cleared sparsely
//! (only the index entries in use), so a cleared store keeps its capacity
//! and a small measuring probe costs no more than what it lays out.

use super::{FlowBox, NodeId, Rect};

/// What a layout order was recorded for, and the box recorded under it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Slot {
    pub(super) el: Option<NodeId>,
    /// The box is positioned itself (not only inside a positioned box).
    pub(super) own_positioned: bool,
    /// `order` of the nearest positioned ancestor box (0: none).
    pub(super) positioned_by: u32,
    /// The element is `position: sticky`.
    pub(super) sticky: bool,
    pub(super) b: FlowBox,
}

fn set_index(index: &mut Vec<u32>, node: NodeId, value: u32) {
    let i = node.index() as usize;
    if index.len() <= i {
        index.resize(i + 1, 0);
    }
    index[i] = value;
}

/// The boxes of a pass, by order (index = order - 1); an element laid out
/// more than once keeps its last box.
#[derive(Clone, Debug, Default)]
pub struct Boxes {
    pub(super) slots: Vec<Slot>,
    /// Per node index: the order of its box (0: none).
    index: Vec<u32>,
    len: usize,
}

impl Boxes {
    pub fn get(&self, el: &NodeId) -> Option<&FlowBox> {
        self.order_of(*el).map(|o| &self.slots[o as usize - 1].b)
    }

    pub fn get_mut(&mut self, el: &NodeId) -> Option<&mut FlowBox> {
        self.order_of(*el).map(|o| &mut self.slots[o as usize - 1].b)
    }

    pub fn contains_key(&self, el: &NodeId) -> bool {
        self.order_of(*el).is_some()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Each element and its box, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &FlowBox)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            let el = s.el.as_ref()?;
            (self.index.get(el.index() as usize) == Some(&(i as u32 + 1))).then_some((el, &s.b))
        })
    }

    pub fn values(&self) -> impl Iterator<Item = &FlowBox> {
        self.iter().map(|(_, b)| b)
    }

    fn order_of(&self, el: NodeId) -> Option<u32> {
        let o = *self.index.get(el.index() as usize)?;
        (o != 0 && self.slots[o as usize - 1].el == Some(el)).then_some(o)
    }

    /// Whether slot `i` holds the box its element is known by (not one
    /// replaced by a later layout of the element).
    pub(super) fn live(&self, i: usize) -> Option<NodeId> {
        let el = self.slots.get(i)?.el?;
        (self.index.get(el.index() as usize) == Some(&(i as u32 + 1))).then_some(el)
    }

    /// The live box in slot `i`.
    pub(super) fn at_mut(&mut self, i: usize) -> Option<&mut FlowBox> {
        self.live(i)?;
        Some(&mut self.slots[i].b)
    }

    /// Records `slot` (its box's order says where). Returns whether the
    /// element had a box already.
    pub(super) fn put(&mut self, el: NodeId, slot: Slot) -> bool {
        let i = slot.b.order as usize - 1;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, Slot::default());
        }
        let had = self.order_of(el).is_some();
        if !had {
            self.len += 1;
        }
        self.slots[i] = Slot { el: Some(el), ..slot };
        set_index(&mut self.index, el, i as u32 + 1);
        had
    }

    /// Copies `src`'s slots `lo..hi` to the orders after `at` (no box is
    /// recorded there yet), live boxes only, with `fix` applied to each.
    /// Returns whether any of their elements had a box already.
    pub(super) fn copy_range(
        &mut self,
        src: &Boxes,
        lo: usize,
        hi: usize,
        at: usize,
        mut fix: impl FnMut(&mut Slot),
    ) -> bool {
        let mut had = false;
        if self.slots.len() < at {
            self.slots.resize(at, Slot::default());
        }
        debug_assert_eq!(self.slots.len(), at, "copy into recorded orders");
        self.slots.extend_from_slice(&src.slots[lo..hi]);
        for (k, i) in (lo..hi).enumerate() {
            let s = &mut self.slots[at + k];
            let Some(el) = s.el else { continue };
            if src.index.get(el.index() as usize) != Some(&(i as u32 + 1)) {
                s.el = None;
                continue;
            }
            fix(s);
            if self.order_of(el).is_none() {
                self.len += 1;
            } else {
                had = true;
            }
            set_index(&mut self.index, el, (at + k) as u32 + 1);
        }
        had
    }

    pub(super) fn reserve_like(&mut self, like: &Boxes) {
        self.slots.reserve(like.slots.len());
        self.index.resize(like.index.len(), 0);
    }

    pub(super) fn clear(&mut self) {
        for s in &self.slots {
            if let Some(el) = s.el {
                if let Some(o) = self.index.get_mut(el.index() as usize) {
                    *o = 0;
                }
            }
        }
        self.slots.clear();
        self.len = 0;
    }
}

impl<'a> IntoIterator for &'a Boxes {
    type Item = (&'a NodeId, &'a FlowBox);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// A text node laid out in line boxes: the union of its fragments
/// (document coordinates, or viewport ones inside `position: fixed`), and
/// whether it is inside a fixed box.
#[derive(Clone, Copy, Debug)]
pub(super) struct TextEntry {
    /// The layout order current when the text was recorded.
    pub(super) order: u32,
    pub(super) node: NodeId,
    pub(super) at: (Rect, bool),
}

/// The text entries of a pass, in the order they were recorded (orders
/// only grow); a text laid out more than once keeps its last entry.
#[derive(Clone, Debug, Default)]
pub struct Texts {
    pub(super) list: Vec<TextEntry>,
    /// Per node index: 1 + the position of its entry (0: none).
    index: Vec<u32>,
    len: usize,
}

impl Texts {
    pub fn get(&self, node: &NodeId) -> Option<&(Rect, bool)> {
        self.pos_of(*node).map(|p| &self.list[p].at)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Each text node and its geometry, in the order laid out.
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &(Rect, bool))> {
        self.list.iter().enumerate().filter(|(p, e)| self.live(*p, e.node)).map(|(_, e)| (&e.node, &e.at))
    }

    fn live(&self, p: usize, node: NodeId) -> bool {
        self.index.get(node.index() as usize) == Some(&(p as u32 + 1))
    }

    fn pos_of(&self, node: NodeId) -> Option<usize> {
        let p = *self.index.get(node.index() as usize)?;
        (p != 0 && self.list[p as usize - 1].node == node).then(|| p as usize - 1)
    }

    pub(super) fn push(&mut self, order: u32, node: NodeId, at: (Rect, bool)) {
        if self.pos_of(node).is_none() {
            self.len += 1;
        }
        self.list.push(TextEntry { order, node, at });
        set_index(&mut self.index, node, self.list.len() as u32);
    }

    /// The first entry recorded at or after `order`.
    pub(super) fn first_from(&self, order: u32) -> usize {
        self.list.partition_point(|e| e.order < order)
    }

    /// The entry at position `p` when it is its node's current one.
    pub(super) fn live_at(&self, p: usize) -> Option<&TextEntry> {
        self.list.get(p).filter(|e| self.live(p, e.node))
    }

    /// Appends `src`'s entries `lo..hi` with `fix` applied to each; those
    /// replaced later in `src` stay replaced.
    pub(super) fn copy_range(&mut self, src: &Texts, lo: usize, hi: usize, mut fix: impl FnMut(&mut TextEntry)) {
        let at = self.list.len();
        self.list.extend_from_slice(&src.list[lo..hi]);
        for (k, p) in (lo..hi).enumerate() {
            let e = &mut self.list[at + k];
            fix(e);
            if src.live(p, e.node) {
                let node = e.node;
                if self.pos_of(node).is_none() {
                    self.len += 1;
                }
                set_index(&mut self.index, node, (at + k) as u32 + 1);
            }
        }
    }

    pub(super) fn reserve_like(&mut self, like: &Texts) {
        self.list.reserve(like.list.len());
        self.index.resize(like.index.len(), 0);
    }

    pub(super) fn clear(&mut self) {
        for e in &self.list {
            if let Some(p) = self.index.get_mut(e.node.index() as usize) {
                *p = 0;
            }
        }
        self.list.clear();
        self.len = 0;
    }
}

impl<'a> IntoIterator for &'a Texts {
    type Item = (&'a NodeId, &'a (Rect, bool));
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// Values keyed by (node, `K`), grouped by node, so dropping everything
/// known about a node is one removal (not a scan of every key).
#[derive(Debug)]
pub(super) struct ByNode<K, V> {
    map: rustc_hash::FxHashMap<NodeId, Vec<(K, V)>>,
    len: usize,
}

impl<K, V> Default for ByNode<K, V> {
    fn default() -> Self {
        ByNode { map: Default::default(), len: 0 }
    }
}

impl<K: PartialEq, V> ByNode<K, V> {
    pub(super) fn get(&self, node: NodeId, key: &K) -> Option<&V> {
        self.map.get(&node)?.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(super) fn insert(&mut self, node: NodeId, key: K, value: V) {
        let values = self.map.entry(node).or_default();
        match values.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => {
                values.push((key, value));
                self.len += 1;
            }
        }
    }

    pub(super) fn remove(&mut self, node: &NodeId) {
        if let Some(values) = self.map.remove(node) {
            self.len -= values.len();
        }
    }

    pub(super) fn clear(&mut self) {
        self.map.clear();
        self.len = 0;
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }
}

/// A map by node held in a dense list, with an index by node index: a
/// lookup is two array reads.
#[derive(Debug)]
pub(super) struct NodeMap<V> {
    /// Per node index: 1 + the position of its value (0: none).
    index: Vec<u32>,
    list: Vec<(NodeId, V)>,
}

impl<V> Default for NodeMap<V> {
    fn default() -> Self {
        NodeMap { index: Vec::new(), list: Vec::new() }
    }
}

impl<V> NodeMap<V> {
    fn pos(&self, node: &NodeId) -> Option<usize> {
        let p = *self.index.get(node.index() as usize)?;
        (p != 0 && self.list[p as usize - 1].0 == *node).then(|| p as usize - 1)
    }

    pub(super) fn get_mut(&mut self, node: &NodeId) -> Option<&mut V> {
        self.pos(node).map(|p| &mut self.list[p].1)
    }

    /// Returns the value replaced.
    pub(super) fn insert(&mut self, node: NodeId, value: V) -> Option<V> {
        if let Some(p) = self.pos(&node) {
            return Some(std::mem::replace(&mut self.list[p].1, value));
        }
        self.list.push((node, value));
        set_index(&mut self.index, node, self.list.len() as u32);
        None
    }

    pub(super) fn remove(&mut self, node: &NodeId) -> Option<V> {
        let p = self.pos(node)?;
        self.index[node.index() as usize] = 0;
        let (_, v) = self.list.swap_remove(p);
        if let Some((moved, _)) = self.list.get(p) {
            self.index[moved.index() as usize] = p as u32 + 1;
        }
        Some(v)
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&NodeId, &mut V) -> bool) {
        let mut p = 0;
        while p < self.list.len() {
            let (node, v) = &mut self.list[p];
            if keep(node, v) {
                p += 1;
            } else {
                let node = *node;
                self.remove(&node);
            }
        }
    }

    pub(super) fn clear(&mut self) {
        for (node, _) in &self.list {
            self.index[node.index() as usize] = 0;
        }
        self.list.clear();
    }

    pub(super) fn len(&self) -> usize {
        self.list.len()
    }
}
