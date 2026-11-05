//! Where a node is, kept up to date when it moves (as Blink does): the
//! `tree_flags` in every node header (connected, in a shadow tree) and, per
//! tree scope (a document or a shadow root), its elements by id.
//!
//! A parentless node other than a document or shadow root has no flags, so
//! linking a node under a parent gives its subtree the parent's flags and
//! unlinking clears them: one traversal of the subtree, only when they change
//! (never for a subtree built detached, as parsers and `cloneNode` build
//! them). The same traversal moves the subtree's ids between scopes, and
//! notices `<base>` elements entering or leaving a document tree.

use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{Hash, Hasher};

use markup5ever::local_name;
use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::node::{tree_flags, Atom, NodeKind, NIL};

/// An id as a map key: the atom, hashed and compared as its text so the map
/// is searched by `&str` without interning it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IdKey(Atom);

impl Hash for IdKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (*self.0).hash(state)
    }
}

impl Borrow<str> for IdKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
struct IdEntry {
    /// The first element with the id in tree order; NIL once the first of
    /// several left (found again by the next lookup).
    first: Cell<u32>,
    /// Elements of the scope with the id.
    count: u32,
}

/// One tree scope's elements by (non-empty) id.
#[derive(Debug, Default)]
pub(crate) struct IdMap(FxHashMap<IdKey, IdEntry>);

impl IdMap {
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Dom {
    // ----- tree flags ---------------------------------------------------

    /// After `node` was linked under its parent: it and its shadow-including
    /// descendants take the parent's place.
    #[inline]
    pub(crate) fn tree_linked(&mut self, node: u32) {
        let parent = self.n(node).parent;
        let flags = self.n(parent).tree_flags();
        debug_assert_eq!(self.n(node).tree_flags(), 0, "a parentless node has no tree flags");
        if flags != 0 {
            self.retag(node, flags);
        }
    }

    /// Before `node` is unlinked from its parent: it and its shadow-including
    /// descendants become a detached tree.
    #[inline]
    pub(crate) fn tree_unlinking(&mut self, node: u32) {
        if self.n(node).tree_flags() != 0 {
            self.retag(node, 0);
        }
    }

    /// Flags of a new shadow root of host `h`.
    pub(crate) fn shadow_root_flags_for(&self, h: u32) -> u32 {
        tree_flags::IN_SHADOW_TREE | (self.n(h).tree_flags() & tree_flags::CONNECTED)
    }

    /// Gives `node`'s light subtree `flags` (the shadow trees in it follow
    /// its connectedness), moving its ids out of or into its tree scope:
    /// out when `flags` is 0 (still linked), else in (just linked).
    fn retag(&mut self, node: u32, flags: u32) {
        let entering = flags != 0;
        let old = self.n(node).tree_flags();
        // Light nodes of a document tree (not of a shadow tree).
        let doc_tree = if entering { flags } else { old } == tree_flags::CONNECTED;
        let mut scope = NIL;
        let mut base_moved = false;
        let mut shadows: Vec<(u32, u32)> = Vec::new();
        let mut cur = node;
        while cur != NIL {
            let n = self.n_mut(cur);
            n.set_tree_flags(flags);
            if let NodeKind::Element(e) = &n.kind {
                let sr = e.shadow_root;
                let id = e.id_atom().filter(|a| !a.is_empty()).cloned();
                base_moved |= doc_tree && e.is_html() && e.name.local == local_name!("base");
                if let Some(id) = id {
                    if scope == NIL {
                        scope = self.root_idx(node);
                    }
                    if entering {
                        self.id_added(scope, &id, cur);
                    } else {
                        self.id_removed(scope, &id, cur);
                    }
                }
                if sr != NIL {
                    let sf = tree_flags::IN_SHADOW_TREE | (flags & tree_flags::CONNECTED);
                    if self.n(sr).tree_flags() != sf {
                        shadows.push((sr, sf));
                    }
                }
            }
            cur = self.following_in(cur, node);
        }
        if base_moved {
            let doc = if scope != NIL { scope } else { self.root_idx(node) };
            self.html_base_changed(doc);
        }
        // Shadow trees inside: only their connectedness changes (their ids
        // stay in their own scope).
        while let Some((root, f)) = shadows.pop() {
            let mut cur = root;
            while cur != NIL {
                let n = self.n_mut(cur);
                n.set_tree_flags(f);
                if let NodeKind::Element(e) = &n.kind {
                    let sr = e.shadow_root;
                    if sr != NIL {
                        let sf = tree_flags::IN_SHADOW_TREE | (f & tree_flags::CONNECTED);
                        if self.n(sr).tree_flags() != sf {
                            shadows.push((sr, sf));
                        }
                    }
                }
                cur = self.following_in(cur, root);
            }
        }
    }

    // ----- id maps -------------------------------------------------------

    /// The id map of tree scope `scope` (a document or shadow root).
    fn id_map(&self, scope: u32) -> Option<&IdMap> {
        match &self.n(scope).kind {
            NodeKind::Document(d) => Some(&d.ids),
            NodeKind::Fragment(f) if f.shadow.is_some() => f.ids.as_deref(),
            _ => None,
        }
    }

    fn id_map_mut(&mut self, scope: u32) -> &mut IdMap {
        match &mut self.n_mut(scope).kind {
            NodeKind::Document(d) => &mut d.ids,
            NodeKind::Fragment(f) if f.shadow.is_some() => f.ids.get_or_insert_with(Default::default),
            _ => unreachable!("tree scope root"),
        }
    }

    fn id_added(&mut self, scope: u32, id: &Atom, el: u32) {
        let first = self.id_map(scope).and_then(|m| m.0.get(&**id)).map(|e| e.first.get());
        let first = match first {
            None => el,
            Some(NIL) => NIL,
            Some(f) if self.tree_order_before(el, f) => el,
            Some(f) => f,
        };
        let entry =
            self.id_map_mut(scope).0.entry(IdKey(id.clone())).or_insert(IdEntry { first: Cell::new(el), count: 0 });
        entry.count += 1;
        entry.first.set(first);
    }

    fn id_removed(&mut self, scope: u32, id: &Atom, el: u32) {
        let map = &mut self.id_map_mut(scope).0;
        let Some(entry) = map.get_mut(&**id) else {
            debug_assert!(false, "indexed id missing");
            return;
        };
        entry.count -= 1;
        if entry.count == 0 {
            map.remove(&**id);
        } else if entry.first.get() == el {
            entry.first.set(NIL);
        }
    }

    /// Element `i`'s `id` changed from `old` (attribute write).
    pub(crate) fn id_attr_changed(&mut self, i: u32, old: Option<Atom>) {
        if self.n(i).tree_flags() == 0 {
            return;
        }
        let new = self.n(i).element().and_then(|e| e.id_atom()).cloned();
        if new == old {
            return;
        }
        let scope = self.root_idx(i);
        if let Some(old) = old.filter(|a| !a.is_empty()) {
            self.id_removed(scope, &old, i);
        }
        if let Some(new) = new.filter(|a| !a.is_empty()) {
            self.id_added(scope, &new, i);
        }
    }

    /// The first element with id `id` in the light tree under `root`:
    /// `Some(NIL)` for none, `None` when `root` is no tree scope (it has
    /// no index: callers walk).
    pub(crate) fn indexed_by_id(&self, root: u32, id: &str) -> Option<u32> {
        let map = match &self.n(root).kind {
            NodeKind::Document(d) => &d.ids,
            NodeKind::Fragment(f) if f.shadow.is_some() => match f.ids.as_deref() {
                Some(m) => m,
                None => return Some(NIL),
            },
            _ => return None,
        };
        let Some(entry) = map.0.get(id) else { return Some(NIL) };
        let first = entry.first.get();
        if first != NIL {
            return Some(first);
        }
        // The first of several left: find the new first once.
        let found = self.walk_by_id(root, id);
        debug_assert!(found != NIL, "an indexed id has an element");
        entry.first.set(found);
        Some(found)
    }

    /// The first element with id `id` in the light tree under `root`
    /// (`root` excluded), by walking it.
    pub(crate) fn walk_by_id(&self, root: u32, id: &str) -> u32 {
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            if self.n(cur).element().and_then(|e| e.id()) == Some(id) {
                return cur;
            }
            cur = self.following_in(cur, root);
        }
        NIL
    }

    /// The first element with id `id` (non-empty) in the light tree under
    /// `root`: the index of a tree scope, a walk of anything else.
    pub(crate) fn first_by_id(&self, root: u32, id: &str) -> u32 {
        if id.is_empty() {
            return NIL;
        }
        self.indexed_by_id(root, id).unwrap_or_else(|| self.walk_by_id(root, id))
    }

    // ----- verification (tests) ------------------------------------------

    /// Checks every live node's tree flags, attribute atoms and every tree
    /// scope's id map against recomputing them by walks. For tests.
    #[doc(hidden)]
    pub fn verify_tree_state(&self) -> Result<(), String> {
        let mut scopes = Vec::new();
        for i in 0..self.slot_count() {
            if !self.slot_live(i) {
                continue;
            }
            let n = self.n(i);
            let root = self.root_idx(i);
            let in_shadow = matches!(&self.n(root).kind, NodeKind::Fragment(f) if f.shadow.is_some());
            let connected = self.connected_by_walk(i);
            if n.is_connected() != connected || n.in_shadow_tree() != in_shadow {
                return Err(format!(
                    "node {i}: flags (connected {}, in shadow {}), walk ({connected}, {in_shadow})",
                    n.is_connected(),
                    n.in_shadow_tree()
                ));
            }
            if connected && !matches!(n.kind, NodeKind::Document(_)) {
                let top = self.shadow_including_root_of(i);
                if n.owner_doc != top {
                    return Err(format!("node {i}: connected to {top} but owned by {}", n.owner_doc));
                }
            }
            if let Some(e) = n.element() {
                if !e.derived_in_step() {
                    return Err(format!("element {i}: id/class atoms differ from its attributes"));
                }
            }
            if matches!(n.kind, NodeKind::Document(_)) || in_shadow && root == i {
                scopes.push(i);
            }
        }
        for scope in scopes {
            let mut want: FxHashMap<Atom, (u32, u32)> = FxHashMap::default();
            let mut cur = self.n(scope).first_child;
            while cur != NIL {
                if let Some(id) = self.n(cur).element().and_then(|e| e.id_atom()).filter(|a| !a.is_empty()) {
                    want.entry(id.clone()).or_insert((cur, 0)).1 += 1;
                }
                cur = self.following_in(cur, scope);
            }
            let have = self.id_map(scope);
            let have_len = have.map_or(0, IdMap::len);
            if have_len != want.len() {
                return Err(format!("scope {scope}: {have_len} ids indexed, {} in the tree", want.len()));
            }
            for (id, (first, count)) in want {
                let Some(entry) = have.and_then(|m| m.0.get(&*id)) else {
                    return Err(format!("scope {scope}: id {id:?} not indexed"));
                };
                let indexed_first = entry.first.get();
                if entry.count != count || (indexed_first != NIL && indexed_first != first) {
                    return Err(format!(
                        "scope {scope}: id {id:?} indexed ({indexed_first}, {}), tree ({first}, {count})",
                        entry.count
                    ));
                }
            }
        }
        Ok(())
    }

    /// Connectedness by walking to the shadow-including root.
    pub(crate) fn connected_by_walk(&self, i: u32) -> bool {
        matches!(self.n(self.shadow_including_root_of(i)).kind, NodeKind::Document(_))
    }

    fn shadow_including_root_of(&self, mut i: u32) -> u32 {
        loop {
            i = self.root_idx(i);
            match &self.n(i).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => i = f.host,
                _ => return i,
            }
        }
    }
}
