//! Where a node is, kept up to date when it moves (as Blink does): the
//! `tree_flags` in every node header (connected, in a shadow tree) and, per
//! tree scope (a document or a shadow root), its elements by id and its
//! `<label>` elements by `for` (`element.labels`).
//!
//! A parentless node other than a document or shadow root has no flags, so
//! linking a node under a parent gives its subtree the parent's flags and
//! unlinking clears them: one traversal of the subtree, only when they change
//! (never for a subtree built detached, as parsers and `cloneNode` build
//! them; the fragment parser's host document is not connected either). The same traversal moves the subtree's ids between scopes, and
//! notices `<base>` elements entering or leaving a document tree.

use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{Hash, Hasher};

use markup5ever::local_name;
use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::node::{tree_flags, Atom, NodeKind, NIL};

/// An id (or a label's `for`) as a map key: the atom, hashed and compared as its text so the map
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
    /// Elements of the scope with the id. Compiled queries rely on it (one:
    /// the only candidate or the subtree to walk; none: no match), so every
    /// tree or `id` write must go through `id_added` / `id_removed`.
    count: u32,
}

/// One tree scope's index: its elements by (non-empty) id, and its HTML
/// `<label>` elements by (non-empty) `for`.
#[derive(Debug, Default)]
pub(crate) struct ScopeIndex {
    ids: FxHashMap<IdKey, IdEntry>,
    /// The HTML `<label>` elements of the scope with a non-empty `for`, by
    /// its value, in the order they were indexed (few per value: sorted
    /// into tree order when read). Every tree or `for` write goes through
    /// `label_for_added` / `label_for_removed`; keyed by the label's
    /// `for` atom (`ElementData::label_for_atom`), so linking, unlinking
    /// and `for` writes allocate no key.
    labels_for: FxHashMap<IdKey, Vec<u32>>,
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
        if self.n(node).is_element() {
            self.document_roots_linked(node, parent);
        }
    }

    /// Before `node` is unlinked from its parent: it and its shadow-including
    /// descendants become a detached tree.
    #[inline]
    pub(crate) fn tree_unlinking(&mut self, node: u32) {
        if self.n(node).tree_flags() != 0 {
            self.retag(node, 0);
        }
        if self.n(node).is_element() {
            self.document_roots_unlinking(node);
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
                let label_for = label_for_of(e).cloned();
                base_moved |= doc_tree && e.is_html() && e.name.local == local_name!("base");
                if let Some(id) = id {
                    if scope == NIL {
                        scope = self.root_idx(node);
                    }
                    if entering {
                        self.id_added(scope, &id, cur, Some(node));
                    } else {
                        self.id_removed(scope, &id, cur);
                    }
                }
                if let Some(f) = label_for {
                    if scope == NIL {
                        scope = self.root_idx(node);
                    }
                    if entering {
                        self.label_for_added(scope, f, cur);
                    } else {
                        self.label_for_removed(scope, &f, cur);
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

    // ----- scope indexes -------------------------------------------------

    /// The index of tree scope `scope` (a document or shadow root).
    fn scope_index(&self, scope: u32) -> Option<&ScopeIndex> {
        match &self.n(scope).kind {
            NodeKind::Document(d) => Some(&d.ids),
            NodeKind::Fragment(f) if f.shadow.is_some() => f.ids.as_deref(),
            _ => None,
        }
    }

    fn scope_index_mut(&mut self, scope: u32) -> &mut ScopeIndex {
        match &mut self.n_mut(scope).kind {
            NodeKind::Document(d) => &mut d.ids,
            NodeKind::Fragment(f) if f.shadow.is_some() => f.ids.get_or_insert_with(Default::default),
            _ => unreachable!("tree scope root"),
        }
    }

    /// The index of the tree scope rooted at `root` for lookups: `None`
    /// when `root` is no tree scope, or a fragment parser's host document
    /// (callers walk).
    fn lookup_index(&self, root: u32) -> Option<Option<&ScopeIndex>> {
        match &self.n(root).kind {
            NodeKind::Document(d) if d.fragment_parse_host => None,
            NodeKind::Document(d) => Some(Some(&d.ids)),
            NodeKind::Fragment(f) if f.shadow.is_some() => Some(f.ids.as_deref()),
            _ => None,
        }
    }

    /// Element `el` with id `id` joined `scope`: as part of the subtree
    /// `entering` (linked into the scope, its elements added in tree order),
    /// or alone (`None`: its `id` was set).
    fn id_added(&mut self, scope: u32, id: &Atom, el: u32, entering: Option<u32>) {
        let first = self.scope_index(scope).and_then(|m| m.ids.get(&**id)).map(|e| e.first.get());
        let first = match (first, entering) {
            (None, _) => el,
            (Some(NIL), _) => NIL,
            // Added earlier in this pass, so before `el`: one climb from the
            // first, not a walk of the siblings between them.
            (Some(f), Some(sub)) if self.is_inclusive_ancestor(sub, f) => f,
            // Outside the subtree: before or after all of it.
            (Some(f), Some(sub)) => {
                if self.tree_order_before(sub, f) {
                    el
                } else {
                    f
                }
            }
            (Some(f), None) if self.tree_order_before(el, f) => el,
            (Some(f), None) => f,
        };
        let entry =
            self.scope_index_mut(scope).ids.entry(IdKey(id.clone())).or_insert(IdEntry { first: Cell::new(el), count: 0 });
        entry.count += 1;
        entry.first.set(first);
    }

    fn id_removed(&mut self, scope: u32, id: &Atom, el: u32) {
        let map = &mut self.scope_index_mut(scope).ids;
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
            self.id_added(scope, &new, i, None);
        }
    }

    /// The first element with id `id` in the light tree under `root`:
    /// `Some(NIL)` for none, `None` when `root` is no tree scope (it has
    /// no index: callers walk).
    pub(crate) fn indexed_by_id(&self, root: u32, id: &str) -> Option<u32> {
        self.indexed_by_id_count(root, id).map(|(first, _)| first)
    }

    /// [`Dom::indexed_by_id`] and how many elements of the scope have the
    /// id: `Some((NIL, 0))` for none.
    pub(crate) fn indexed_by_id_count(&self, root: u32, id: &str) -> Option<(u32, u32)> {
        let Some(map) = self.lookup_index(root)? else { return Some((NIL, 0)) };
        let Some(entry) = map.ids.get(id) else { return Some((NIL, 0)) };
        let first = entry.first.get();
        if first != NIL {
            return Some((first, entry.count));
        }
        // The first of several left: find the new first once.
        let found = self.walk_by_id(root, id);
        debug_assert!(found != NIL, "an indexed id has an element");
        entry.first.set(found);
        Some((found, entry.count))
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

    // ----- labels by `for` -----------------------------------------------

    /// HTML `<label>` `el` with the non-empty `for` value `f` joined `scope`
    /// (linked into it, or its `for` was set).
    fn label_for_added(&mut self, scope: u32, f: Atom, el: u32) {
        self.scope_index_mut(scope).labels_for.entry(IdKey(f)).or_default().push(el);
    }

    fn label_for_removed(&mut self, scope: u32, f: &Atom, el: u32) {
        let map = &mut self.scope_index_mut(scope).labels_for;
        let Some(labels) = map.get_mut(&**f) else {
            debug_assert!(false, "indexed label missing");
            return;
        };
        match labels.iter().position(|&l| l == el) {
            Some(k) => {
                labels.swap_remove(k);
            }
            None => debug_assert!(false, "indexed label missing"),
        }
        if labels.is_empty() {
            map.remove(&**f);
        }
    }

    /// HTML `<label>` `i`'s `for` changed from `old` (attribute write).
    pub(crate) fn label_for_attr_changed(&mut self, i: u32, old: Option<Atom>) {
        if self.n(i).tree_flags() == 0 {
            return;
        }
        let new = self.n(i).element().and_then(label_for_of).cloned();
        if new == old {
            return;
        }
        let scope = self.root_idx(i);
        if let Some(old) = old {
            self.label_for_removed(scope, &old, i);
        }
        if let Some(new) = new {
            self.label_for_added(scope, new, i);
        }
    }

    /// The HTML `<label>` elements of the tree under `root` whose `for` is
    /// `id` (non-empty), in no particular order: `None` when `root` is no
    /// tree scope (callers walk).
    pub(crate) fn labels_for_indexed(&self, root: u32, id: &str) -> Option<&[u32]> {
        let Some(index) = self.lookup_index(root)? else { return Some(&[]) };
        Some(index.labels_for.get(id).map(Vec::as_slice).unwrap_or(&[]))
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
            if matches!(&n.kind, NodeKind::Document(d) if !d.fragment_parse_host) || in_shadow && root == i {
                scopes.push(i);
            }
        }
        for scope in scopes {
            let mut want: FxHashMap<Atom, (u32, u32)> = FxHashMap::default();
            let mut want_labels: FxHashMap<Atom, Vec<u32>> = FxHashMap::default();
            let mut cur = self.n(scope).first_child;
            while cur != NIL {
                if let Some(e) = self.n(cur).element() {
                    if let Some(id) = e.id_atom().filter(|a| !a.is_empty()) {
                        want.entry(id.clone()).or_insert((cur, 0)).1 += 1;
                    }
                    if let Some(f) = label_for_of(e) {
                        want_labels.entry(f.clone()).or_default().push(cur);
                    }
                }
                cur = self.following_in(cur, scope);
            }
            let have = self.scope_index(scope);
            let have_len = have.map_or(0, |m| m.ids.len());
            if have_len != want.len() {
                return Err(format!("scope {scope}: {have_len} ids indexed, {} in the tree", want.len()));
            }
            let have_labels = have.map_or(0, |m| m.labels_for.len());
            if have_labels != want_labels.len() {
                return Err(format!(
                    "scope {scope}: {have_labels} label `for` values indexed, {} in the tree",
                    want_labels.len()
                ));
            }
            for (f, mut labels) in want_labels {
                let mut indexed = have.and_then(|m| m.labels_for.get(&*f)).cloned().unwrap_or_default();
                labels.sort_unstable();
                indexed.sort_unstable();
                if indexed != labels {
                    return Err(format!("scope {scope}: labels for {f:?} indexed {indexed:?}, tree {labels:?}"));
                }
            }
            for (id, (first, count)) in want {
                let Some(entry) = have.and_then(|m| m.ids.get(&*id)) else {
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
        // Each document's kept document element, head and body are what
        // walking its children finds.
        for i in 0..self.slot_count() {
            if !self.slot_live(i) {
                continue;
            }
            let NodeKind::Document(d) = &self.n(i).kind else { continue };
            let root = self.first_element_child_idx(i);
            let (head, body) = match root {
                r if r != NIL && self.is_html_root(r) => (self.head_child_idx(r, NIL), self.body_child_idx(r, NIL)),
                _ => (NIL, NIL),
            };
            if (d.document_element, d.head, d.body) != (root, head, body) {
                return Err(format!(
                    "document {i}: kept (root, head, body) ({}, {}, {}), tree ({root}, {head}, {body})",
                    d.document_element, d.head, d.body
                ));
            }
        }
        Ok(())
    }

    /// Connectedness by walking to the shadow-including root.
    pub(crate) fn connected_by_walk(&self, i: u32) -> bool {
        matches!(&self.n(self.shadow_including_root_of(i)).kind, NodeKind::Document(d) if !d.fragment_parse_host)
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

impl Dom {
    // ----- document element, head and body -----------------------------

    /// The document whose document element is `parent`, an HTML `html`
    /// element (the only one with a head and a body), if any.
    fn document_of_html_root(&self, parent: u32) -> u32 {
        let doc = self.n(parent).parent;
        match doc != NIL
            && matches!(&self.n(doc).kind, NodeKind::Document(d) if d.document_element == parent)
            && self.is_html_root(parent)
        {
            true => doc,
            false => NIL,
        }
    }

    /// Records `element` (NIL: none) as `doc`'s document element, with its
    /// `head` and `body` (none unless it is an HTML `html` element).
    fn set_document_element(&mut self, doc: u32, element: u32) {
        let (head, body) = match element {
            e if e != NIL && self.is_html_root(e) => (self.head_child_idx(e, NIL), self.body_child_idx(e, NIL)),
            _ => (NIL, NIL),
        };
        if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
            d.document_element = element;
            d.head = head;
            d.body = body;
        }
    }

    /// After element `node` was linked under `parent`: it may be a
    /// document's document element now, or that element's head or body.
    fn document_roots_linked(&mut self, node: u32, parent: u32) {
        if let NodeKind::Document(d) = &self.n(parent).kind {
            let current = d.document_element;
            // The first element child: a document has one, but the parsers'
            // raw links do not check.
            let first = self.first_element_child_idx(parent);
            if first != current {
                self.set_document_element(parent, first);
            }
            return;
        }
        let doc = self.document_of_html_root(parent);
        if doc == NIL {
            return;
        }
        let Some(e) = self.n(node).element().filter(|e| e.is_html()) else { return };
        if e.name.local == local_name!("head") {
            let head = self.head_child_idx(parent, NIL);
            if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
                d.head = head;
            }
        } else if crate::dom::is_body_name(&e.name.local) {
            let body = self.body_child_idx(parent, NIL);
            if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
                d.body = body;
            }
        }
    }

    /// Before element `node` is unlinked: the document element, head or
    /// body it is passes to the next such sibling (the first after it).
    fn document_roots_unlinking(&mut self, node: u32) {
        let parent = self.n(node).parent;
        if parent == NIL {
            return;
        }
        if let NodeKind::Document(d) = &self.n(parent).kind {
            if d.document_element == node {
                let mut next = self.n(node).next;
                while next != NIL && !self.n(next).is_element() {
                    next = self.n(next).next;
                }
                self.set_document_element(parent, next);
            }
            return;
        }
        let doc = self.document_of_html_root(parent);
        if doc == NIL {
            return;
        }
        let NodeKind::Document(d) = &self.n(doc).kind else { return };
        let (head, body) = (d.head, d.body);
        if head == node {
            let next = self.head_child_idx(parent, node);
            if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
                d.head = next;
            }
        } else if body == node {
            let next = self.body_child_idx(parent, node);
            if let NodeKind::Document(d) = &mut self.n_mut(doc).kind {
                d.body = next;
            }
        }
    }
}

/// The non-empty `for` of an HTML `<label>` element, which the scope index
/// keys it by.
#[inline]
pub(crate) fn label_for_of(e: &crate::node::ElementData) -> Option<&Atom> {
    e.label_for_atom()
}
