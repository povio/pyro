//! Lookup behind named access on the Window object (`window.foo` for
//! `<div id=foo>`). JavaScript reaches it on every global-property miss
//! (`typeof process`, `window.__SOME_HOOK__`), so a name no element has must
//! stay O(1) on a page that keeps mutating: rebuilding from a tree walk after
//! each mutation would make such a miss quadratic. Each document keeps a
//! superset of its names as hashes, caught up from the change journal, and only
//! a name in it pays for an exact lookup.

use crate::dom::{attr_name_hash, Change};
use crate::node::{ElementData, NIL};
use crate::{Dom, DomResult, NodeId};
use rustc_hash::{FxHashMap, FxHashSet};

/// Exact answers kept per document between id/name-affecting changes.
const EXACT_CACHE_SIZE: usize = 256;

/// One document's names and what they are valid for.
pub(crate) struct WindowNames {
    /// Slot generation of the document it was built for.
    gen: u32,
    /// `Dom::version` it is known valid at.
    version: u64,
    /// Nodes the last build walked: the most work worth doing to catch up
    /// instead of rebuilding.
    walked: usize,
    /// Hashes of every name in the document, and of names that have since left
    /// it, until the next build.
    hashes: FxHashSet<u64>,
    /// Hashes the last build found, to bound the leftovers.
    built: usize,
    /// Answers since the last change of any element's id, name or position.
    exact: FxHashMap<String, Vec<u32>>,
}

/// What `Dom::window_name_may_exist` takes: FNV-1a over a name's UTF-16 code
/// units (Latin-1 ones widened), so no string is built for a miss.
pub fn window_name_hash(units: impl Iterator<Item = u16>) -> u64 {
    units.fold(0xcbf29ce484222325, |h, u| (h ^ u64::from(u)).wrapping_mul(0x100000001b3))
}

/// Whether this element's `name` attribute names it on the Window. An iframe
/// names its child window, which the caller resolves.
pub(crate) fn named_by_name(e: &ElementData) -> bool {
    // Atoms compare as integers: asked of every node entering a tree.
    use markup5ever::local_name;
    e.is_html()
        && matches!(
            e.name.local,
            local_name!("embed")
                | local_name!("form")
                | local_name!("iframe")
                | local_name!("img")
                | local_name!("object")
        )
}

impl Dom {
    /// Tells `hook` each name a Document may start to answer as a named
    /// property (see [`Dom::document_named_elements`]): a qualifying element
    /// entering a tree, or getting its `name` (or, for an img, `id`). A
    /// superset: elements outside documents and names a Document already has
    /// are told too. The script engine drops what it cached about the name
    /// (lp-js `ImpureNames`), so `hook` may not touch the DOM.
    pub fn set_named_item_hook(&mut self, hook: Option<std::rc::Rc<dyn Fn(&str)>>) {
        self.named_item_hook = hook;
    }

    /// `node` and its descendants (no shadow trees) entered a tree.
    pub(crate) fn announce_named_items(&self, node: u32) {
        let Some(hook) = self.named_item_hook.as_ref() else { return };
        let mut cur = node;
        while cur != NIL {
            self.announce_element_names(cur, hook);
            cur = self.following_in(cur, node);
        }
    }

    /// An attribute of element `i` was set.
    pub(crate) fn announce_named_attr(&self, i: u32, name: &markup5ever::QualName) {
        let Some(hook) = self.named_item_hook.as_ref() else { return };
        if name.ns.is_empty() && matches!(&*name.local, "name" | "id") {
            self.announce_element_names(i, hook);
        }
    }

    fn announce_element_names(&self, i: u32, hook: &std::rc::Rc<dyn Fn(&str)>) {
        let Some(e) = self.n(i).element() else { return };
        if !named_by_name(e) {
            return;
        }
        if let Some(name) = e.attr_local(&markup5ever::local_name!("name")).filter(|v| !v.is_empty()) {
            hook(name);
            if &*e.name.local == "img" {
                if let Some(id) = e.id().filter(|v| !v.is_empty()) {
                    hook(id);
                }
            }
        }
    }

    /// The elements of `doc`'s tree named `name` on the Document (HTML "the
    /// Document object's supported property names"): embed, form, iframe,
    /// img and object by `name`, and img by `id` when it also has a `name`.
    pub fn document_named_elements(&self, doc: NodeId, name: &str) -> DomResult<Vec<NodeId>> {
        let r = self.check(doc)?;
        if name.is_empty() || !self.window_name_may_exist(doc, window_name_hash(name.encode_utf16())) {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                if named_by_name(e) && e.attr_local(&markup5ever::local_name!("name")) == Some(name)
                    || (e.is_html()
                        && &*e.name.local == "img"
                        && e.id() == Some(name)
                        && e.attr_local(&markup5ever::local_name!("name")).is_some_and(|n| !n.is_empty()))
                {
                    found.push(self.id_of(cur));
                }
            }
            cur = self.following_in(cur, r);
        }
        Ok(found)
    }

    /// Whether an element of `doc` may be named `name` on the Window. False is
    /// exact; true needs `window_named_elements`.
    pub fn window_name_may_exist(&self, doc: NodeId, hash: u64) -> bool {
        let Ok(r) = self.check(doc) else { return false };
        let mut all = self.window_names.borrow_mut();
        self.current_window_names(&mut all, r).hashes.contains(&hash)
    }

    /// The elements of `doc`'s tree (no shadow trees) named `name` on the Window,
    /// in tree order: any element by `id` (as Chromium; the standard says HTML
    /// elements), and embed, form, iframe, img and object by `name`.
    pub fn window_named_elements(&self, doc: NodeId, name: &str) -> DomResult<Vec<NodeId>> {
        let r = self.check(doc)?;
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = self.window_names.borrow_mut();
        let names = self.current_window_names(&mut all, r);
        if let Some(found) = names.exact.get(name) {
            return Ok(found.iter().map(|&i| self.id_of(i)).collect());
        }
        let mut found = Vec::new();
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                if e.id() == Some(name)
                    || (named_by_name(e) && e.attr_local(&markup5ever::local_name!("name")) == Some(name))
                {
                    found.push(cur);
                }
            }
            cur = self.following_in(cur, r);
        }
        if names.exact.len() >= EXACT_CACHE_SIZE {
            names.exact.clear();
        }
        let ids = found.iter().map(|&i| self.id_of(i)).collect();
        names.exact.insert(name.to_owned(), found);
        Ok(ids)
    }

    fn current_window_names<'a>(&self, all: &'a mut FxHashMap<u32, WindowNames>, r: u32) -> &'a mut WindowNames {
        let gen = self.slot_gen(r);
        if !all.contains_key(&r) {
            // Documents that closed leave entries behind: bound them here.
            all.retain(|&d, n| self.slot_live(d) && self.slot_gen(d) == n.gen);
        }
        let names = all.entry(r).or_insert_with(|| self.build_window_names(r, gen));
        if names.gen != gen || !self.catch_up_window_names(names) {
            *names = self.build_window_names(r, gen);
        }
        names
    }

    fn build_window_names(&self, r: u32, gen: u32) -> WindowNames {
        let mut hashes = FxHashSet::default();
        let mut walked = 0;
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            walked += 1;
            self.add_window_names(cur, &mut hashes);
            cur = self.following_in(cur, r);
        }
        let built = hashes.len();
        WindowNames { gen, version: self.version(), walked, hashes, built, exact: FxHashMap::default() }
    }

    fn add_window_names(&self, node: u32, hashes: &mut FxHashSet<u64>) {
        let Some(e) = self.n(node).element() else { return };
        if let Some(id) = e.id().filter(|v| !v.is_empty()) {
            hashes.insert(window_name_hash(id.encode_utf16()));
        }
        if named_by_name(e) {
            if let Some(name) = e.attr_local(&markup5ever::local_name!("name")).filter(|v| !v.is_empty()) {
                hashes.insert(window_name_hash(name.encode_utf16()));
            }
        }
    }

    /// Brings `names` up to the current version from the journal; false when a
    /// new build is cheaper or needed. Every element that gains a name or a
    /// place in the tree is journaled. Costs at most what the build walked,
    /// and is amortized against the mutations before it.
    fn catch_up_window_names(&self, names: &mut WindowNames) -> bool {
        if names.version == self.version() {
            return true;
        }
        let Some(changes) = self.changes_since(names.version) else { return false };
        let mut budget = names.walked.max(64);
        if changes.size_hint().1.is_none_or(|n| n > budget) {
            return false;
        }
        let (id_hash, name_attr_hash) = (attr_name_hash("id"), attr_name_hash("name"));
        for c in changes {
            let (node, subtree) = match c {
                Change::Text(_) | Change::State(_) | Change::Pseudo => continue,
                Change::Unknown(_) => return false,
                Change::Attr(node, hash, _) => {
                    if hash != id_hash && hash != name_attr_hash {
                        continue;
                    }
                    (node, false)
                }
                Change::Removed(..) => {
                    names.exact.clear();
                    continue;
                }
                Change::Inserted(_, node) => (node, true),
            };
            names.exact.clear();
            if !self.slot_live(node) {
                continue;
            }
            let mut cur = node;
            while cur != NIL {
                if budget == 0 {
                    return false;
                }
                budget -= 1;
                self.add_window_names(cur, &mut names.hashes);
                cur = if subtree { self.following_in(cur, node) } else { NIL };
            }
        }
        if names.hashes.len() > 2 * names.built + 1024 {
            return false;
        }
        names.version = self.version();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::window_name_hash;
    use crate::Dom;

    fn units(s: &str) -> u64 {
        window_name_hash(s.encode_utf16())
    }

    #[test]
    fn names_follow_mutations() {
        let mut dom = Dom::new();
        let d = dom.create_document(crate::node::DocumentKind::Html, "http://test/");
        let html = dom.create_element(d, "html").unwrap();
        dom.append_child(d, html).unwrap();
        let a = dom.create_element(d, "div").unwrap();
        dom.set_attribute(a, "id", "foo").unwrap();
        dom.append_child(html, a).unwrap();
        // `name` only names some elements.
        let form = dom.create_element(d, "form").unwrap();
        dom.set_attribute(form, "name", "foo").unwrap();
        dom.append_child(html, form).unwrap();
        let input = dom.create_element(d, "input").unwrap();
        dom.set_attribute(input, "name", "field").unwrap();
        dom.append_child(html, input).unwrap();

        assert!(dom.window_name_may_exist(d, units("foo")));
        assert!(!dom.window_name_may_exist(d, units("missing")));
        assert!(!dom.window_name_may_exist(d, units("field")));
        assert!(!dom.window_name_may_exist(d, units("")));
        assert_eq!(dom.window_named_elements(d, "foo").unwrap(), vec![a, form]);

        // A subtree inserted whole, then an attribute change, are picked up.
        let sub = dom.create_element(d, "section").unwrap();
        let inner = dom.create_element(d, "p").unwrap();
        dom.set_attribute(inner, "id", "inner").unwrap();
        dom.append_child(sub, inner).unwrap();
        assert!(dom.window_named_elements(d, "inner").unwrap().is_empty());
        dom.append_child(html, sub).unwrap();
        assert!(dom.window_name_may_exist(d, units("inner")));
        assert_eq!(dom.window_named_elements(d, "inner").unwrap(), vec![inner]);
        dom.set_attribute(a, "id", "bar").unwrap();
        assert!(dom.window_name_may_exist(d, units("bar")));
        assert_eq!(dom.window_named_elements(d, "foo").unwrap(), vec![form]);
        assert_eq!(dom.window_named_elements(d, "bar").unwrap(), vec![a]);

        // A removed element is no longer named, though its hash may linger.
        dom.remove_child(html, sub).unwrap();
        assert!(dom.window_named_elements(d, "inner").unwrap().is_empty());
    }
}
