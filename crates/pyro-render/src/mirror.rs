//! A Blitz document kept per page between renders ([`crate::Live`]).
//!
//! Each render brings the page's [`Tree`] (pyro-dom's `Dom::render_tree`:
//! nodes in document order with stable keys). The first one builds the
//! document through blitz-dom's `DocumentMutator`, as its HTML parser does;
//! later ones are diffed against the previous tree by key and only the
//! differences are applied (attributes, text, inserted, removed and moved
//! nodes), so Stylo restyles and Taffy re-lays out what changed and keeps the
//! rest. A tree that shares too little with the previous one, a different
//! document (navigation) or a different base URL builds a new document.

use std::collections::HashMap;

use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

use blitz_dom::node::Attribute;
use blitz_dom::{BaseDocument, DocumentConfig, DocumentMutator, NodeId, QualName};

/// A document as pyro-dom serializes it for rendering: nodes in document order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tree {
    pub nodes: Vec<TreeNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    /// Identity across trees of the same document.
    pub key: u64,
    /// Index of the parent in `Tree::nodes`, [`TOP`] at the top level.
    pub parent: u32,
    pub kind: TreeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TreeKind {
    Element { name: QualName, attrs: Vec<(QualName, String)> },
    Text(String),
}

/// `TreeNode::parent` of a top-level node.
pub const TOP: u32 = u32::MAX;

/// A change to a page's [`Tree`] (pyro-dom's `Dom::render_delta`), applied to
/// its document without the whole tree ([`crate::Renderer::sync_delta`]).
#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp {
    /// The node's own content is now `kind` (same element name, or text);
    /// its children are unchanged.
    Node { key: u64, kind: TreeKind },
    /// The children of `parent` (`None`: the top level) are now `kids`;
    /// `nodes` are the subtrees of the inserted ones (parents index into
    /// `nodes`, [`TOP`] for a child of `parent`).
    Children { parent: Option<u64>, kids: Vec<u64>, nodes: Vec<TreeNode> },
}

/// Why a render built its document from scratch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuild {
    /// The page's first render (or the first after the mirror was dropped).
    First,
    /// Another document: navigation, or a new base URL.
    Document,
    /// More than half of the nodes were added or removed.
    Changes,
    /// The tree repeats a key.
    Malformed,
}

/// How a render used the page's mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MirrorStats {
    /// `Some`: built from scratch.
    pub rebuilt: Option<Rebuild>,
    /// Mutations applied (0 when the tree was unchanged).
    pub mutations: u32,
    /// Persistent documents the renderer holds after this render.
    pub pages: u32,
}

struct Entry {
    id: NodeId,
    /// Element name; `None` for text.
    name: Option<QualName>,
    parent: Option<u64>,
    children: Vec<u64>,
}

pub(crate) struct Mirror {
    pub(crate) doc: BaseDocument,
    pub(crate) bridge: crate::net::Bridge,
    pub(crate) scope: String,
    pub(crate) base_url: String,
    /// The document facts it was built with ([`crate::Snapshot::xml_document`],
    /// [`crate::Snapshot::content_language`]).
    pub(crate) facts: (bool, Option<String>, bool, bool),
    pub(crate) viewport: (u32, u32, u64, bool, bool, crate::InputCapabilities),
    sheets: HashMap<String, String>,
    tree: Arc<Tree>,
    nodes: FxHashMap<u64, Entry>,
    /// The key of each mirrored Blitz node, by the node id's slot (with the
    /// whole id, which a reused slot does not match): `nodes` reversed,
    /// maintained wherever an entry is added or removed.
    keys: Vec<(NodeId, u64)>,
    top: Vec<u64>,
    /// Nodes given a scroll offset by [`Mirror::set_element_scroll`].
    scrolled: Vec<NodeId>,
    /// The nodes given a sticky offset ([`Mirror::set_sticky_offsets`]).
    stuck: Vec<NodeId>,
    /// Canvas and video elements given a bitmap by [`Mirror::set_canvases`],
    /// with it and, for a video, the poster image it replaced.
    canvases: Vec<CanvasEntry>,
    /// Styles and selector caches of `display: none` subtrees resolved on
    /// demand (`getComputedStyle`), kept until the embedder's next sync
    /// ([`crate::Renderer::clear_undisplayed`]).
    pub(crate) undisplayed: std::cell::RefCell<blitz_dom::UndisplayedCaches>,
}

/// A bitmap given to an element, and the poster image it replaced (videos).
type CanvasEntry = (NodeId, Arc<Vec<u8>>, Option<Box<blitz_dom::node::ImageData>>);

thread_local! {
    /// External `<use href>` values seen by the mirrors since the last
    /// [`take_sprite_hrefs`].
    static SPRITE_HREFS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// External SVG `<image>`/`<feImage>` hrefs seen by the mirrors since the last
    /// [`take_svg_image_hrefs`].
    static SVG_IMAGE_HREFS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Notes `kind`'s href when it is a `<use>` referring to another document, or an SVG `<image>` or
/// `<feImage>` showing a resource that is no `data:` URL ([`take_svg_image_hrefs`]).
fn note_sprite(kind: &TreeKind) {
    let TreeKind::Element { name, attrs } = kind else { return };
    if name.ns == blitz_dom::ns!(svg) && matches!(&*name.local, "image" | "feImage") {
        for (attr, value) in attrs {
            if attr.local == blitz_dom::local_name!("href") && crate::svg_use::is_external_image(value) {
                SVG_IMAGE_HREFS.with(|h| h.borrow_mut().push(value.clone()));
            }
        }
        return;
    }
    if name.local != blitz_dom::local_name!("use") {
        return;
    }
    for (attr, value) in attrs {
        if attr.local == blitz_dom::local_name!("href") && !value.starts_with('#') && !value.is_empty() {
            SPRITE_HREFS.with(|h| h.borrow_mut().push(value.clone()));
        }
    }
}

/// The external SVG `<image href>`/`<feImage href>` values created or changed since the last call.
pub(crate) fn take_svg_image_hrefs() -> Vec<String> {
    SVG_IMAGE_HREFS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

/// The external `<use href>` values created or changed since the last call.
pub(crate) fn take_sprite_hrefs() -> Vec<String> {
    SPRITE_HREFS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

fn create(m: &mut DocumentMutator<'_>, kind: &TreeKind) -> NodeId {
    note_sprite(kind);
    match kind {
        TreeKind::Text(t) => m.create_text_node(t),
        TreeKind::Element { name, attrs } => m.create_element(
            name.clone(),
            attrs.iter().map(|(name, value)| Attribute { name: name.clone(), value: value.clone() }).collect(),
        ),
    }
}

fn name_of(kind: &TreeKind) -> Option<QualName> {
    match kind {
        TreeKind::Element { name, .. } => Some(name.clone()),
        TreeKind::Text(_) => None,
    }
}

/// Keys of the children of each node, and of the top level.
fn children_of(tree: &Tree) -> (FxHashMap<u64, Vec<u64>>, Vec<u64>) {
    let mut kids: FxHashMap<u64, Vec<u64>> = FxHashMap::with_capacity_and_hasher(tree.nodes.len(), Default::default());
    let mut top = Vec::new();
    for n in &tree.nodes {
        if n.parent == TOP {
            top.push(n.key);
        } else {
            kids.entry(tree.nodes[n.parent as usize].key).or_default().push(n.key);
        }
    }
    (kids, top)
}

impl Mirror {
    /// Builds the document of `tree`.
    pub(crate) fn build(
        tree: Arc<Tree>,
        config: DocumentConfig,
        bridge: crate::net::Bridge,
        scope: String,
        sheets: HashMap<String, String>,
        viewport: (u32, u32, u64, bool, bool, crate::InputCapabilities),
    ) -> Result<Mirror, Rebuild> {
        let base_url = config.base_url.clone().unwrap_or_default();
        let facts = (config.xml_document, config.content_language.clone(), config.quirks_mode, config.line_height_quirk);
        let mut doc = BaseDocument::new(config);
        // Diagnostics: incremental restyle, full relayout every render (tells
        // a blitz-dom relayout difference from a mirroring one).
        if std::env::var_os("PYRO_MIRROR_FULL_LAYOUT").is_some() {
            doc.set_incremental_layout(false);
        }
        let root = doc.root_node().id;
        let (kids, top) = children_of(&tree);
        let mut nodes: FxHashMap<u64, Entry> =
            FxHashMap::with_capacity_and_hasher(tree.nodes.len(), Default::default());
        let mut keys = Vec::new();
        {
            let mut m = doc.mutate();
            let mut ids = Vec::with_capacity(tree.nodes.len());
            for n in &tree.nodes {
                let id = create(&mut m, &n.kind);
                ids.push(id);
                let (parent_id, parent_key) = match n.parent {
                    TOP => (root, None),
                    p => (ids[p as usize], Some(tree.nodes[p as usize].key)),
                };
                m.append_children(parent_id, &[id]);
                let entry = Entry {
                    id,
                    name: name_of(&n.kind),
                    parent: parent_key,
                    children: kids.get(&n.key).cloned().unwrap_or_default(),
                };
                if nodes.insert(n.key, entry).is_some() {
                    return Err(Rebuild::Malformed);
                }
                set_key(&mut keys, id, Some(n.key));
            }
        }
        let mirror = Mirror {
            doc,
            bridge,
            scope,
            base_url,
            facts,
            viewport,
            sheets,
            tree,
            nodes,
            keys,
            top,
            scrolled: Vec::new(),
            stuck: Vec::new(),
            canvases: Vec::new(),
            undisplayed: Default::default(),
        };
        mirror.debug_check_keys();
        Ok(mirror)
    }

    /// The Blitz node of `key`.
    /// Scrolls the elements keyed in `offsets` (CSS px) and puts the ones
    /// scrolled last time and absent now back at the origin.
    pub(crate) fn set_element_scroll(&mut self, offsets: &[(u64, (f64, f64))]) {
        if offsets.is_empty() && self.scrolled.is_empty() {
            return;
        }
        let now: Vec<(NodeId, (f64, f64))> =
            offsets.iter().filter_map(|&(key, o)| Some((self.node(key)?, o))).collect();
        // Each scroll offset that changes is told to Blitz: a scrolled box with a `perspective`
        // has the transforms under it resolved again.
        let set = |node: &mut blitz_dom::Node, offset: blitz_dom::Point<f64>| {
            if *node.scroll_offset() != offset {
                *node.scroll_offset_mut() = offset;
                node.scroll_offset_changed();
            }
        };
        for id in std::mem::take(&mut self.scrolled) {
            if !now.iter().any(|(n, _)| *n == id) {
                if let Some(node) = self.doc.get_node_mut(id) {
                    set(node, blitz_dom::Point { x: 0.0, y: 0.0 });
                }
            }
        }
        for (id, (x, y)) in now {
            if let Some(node) = self.doc.get_node_mut(id) {
                set(node, blitz_dom::Point { x, y });
                self.scrolled.push(id);
            }
        }
        // pyro: the active scroll markers follow the scroll positions (a group that scrolls keeps
        // its active marker in view), without waiting for a layout.
        self.doc.update_scroll_marker_states();
    }

    /// Gives the document the current scroll targets in `targets` (by key; `u64::MAX`: the
    /// viewport's, which is the root element's), for scroll marker selection.
    pub(crate) fn set_scroll_targets(&mut self, targets: &[(u64, u64)]) {
        let root = self.doc.try_root_element().map(|root| root.id);
        let targets: Vec<(NodeId, NodeId)> = targets
            .iter()
            .filter_map(|&(scroller, target)| {
                let scroller = if scroller == u64::MAX { root? } else { self.node(scroller)? };
                Some((scroller, self.node(target)?))
            })
            .collect();
        self.doc.set_scroll_targets(targets);
    }

    /// Gives the `position: sticky` elements in `offsets` (by key) their
    /// offset from where they are laid out, and takes it from those given one
    /// last time and absent now. Painting moves them by it.
    pub(crate) fn set_sticky_offsets(&mut self, offsets: &[(u64, (f64, f64))]) {
        if offsets.is_empty() && self.stuck.is_empty() {
            return;
        }
        let now: Vec<(NodeId, (f64, f64))> =
            offsets.iter().filter_map(|&(key, o)| Some((self.node(key)?, o))).collect();
        for id in std::mem::take(&mut self.stuck) {
            if !now.iter().any(|(n, _)| *n == id) {
                if let Some(node) = self.doc.get_node_mut(id) {
                    node.set_sticky_offset(blitz_dom::Point { x: 0.0, y: 0.0 });
                }
            }
        }
        for (id, (x, y)) in now {
            if let Some(node) = self.doc.get_node_mut(id) {
                node.set_sticky_offset(blitz_dom::Point { x, y });
                self.stuck.push(id);
            }
        }
    }

    /// Gives the canvases and videos in `canvases` their bitmap (a video's
    /// current frame) as their content (a raster image, painted like an
    /// `<img>`'s) and takes it from those given one last time and absent
    /// now: a canvas then paints nothing, a video its poster again.
    /// Unchanged bitmaps (the same `Arc`) are left as they are.
    pub(crate) fn set_canvases(&mut self, canvases: &[crate::CanvasImage]) {
        use blitz_dom::node::{ImageData, RasterImageData, SpecialElementData};
        if canvases.is_empty() && self.canvases.is_empty() {
            return;
        }
        let now: Vec<(NodeId, &crate::CanvasImage)> =
            canvases.iter().filter_map(|c| Some((self.node(c.key)?, c))).collect();
        let mut posters = Vec::new();
        for (id, rgba, poster) in std::mem::take(&mut self.canvases) {
            if now.iter().any(|(n, _)| *n == id) {
                posters.push((id, rgba, poster));
            } else if let Some(element) = self.doc.get_node_mut(id).and_then(|n| n.element_data_mut()) {
                element.special_data = match poster {
                    Some(image) => SpecialElementData::Image(image),
                    None => SpecialElementData::None,
                };
            }
        }
        for (id, canvas) in now {
            let Some(element) = self.doc.get_node_mut(id).and_then(|n| n.element_data_mut()) else { continue };
            let video = &*element.name.local == "video";
            if !(video || &*element.name.local == "canvas")
                || canvas.rgba.len() != canvas.width as usize * canvas.height as usize * 4
            {
                continue;
            }
            let ours = |image: &ImageData, rgba: &Arc<Vec<u8>>| matches!(image, ImageData::Raster(r) if std::ptr::eq(r.data.data().as_ptr(), rgba.as_ptr()));
            let (previous, mut poster) = match posters.iter_mut().find(|(n, _, _)| *n == id) {
                Some((_, rgba, poster)) => (Some(rgba.clone()), poster.take()),
                None => (None, None),
            };
            let same = matches!(&element.special_data, SpecialElementData::Image(image)
                if matches!(&**image, ImageData::Raster(r) if r.width == canvas.width && r.height == canvas.height)
                    && ours(image, &canvas.rgba));
            if !same {
                let replaced = std::mem::take(&mut element.special_data);
                // A video's poster (loaded before the frame, or since) comes
                // back when it shows no frame.
                if let SpecialElementData::Image(image) = replaced {
                    if video && !previous.as_ref().is_some_and(|p| ours(&image, p)) {
                        poster = Some(image);
                    }
                }
                let raster = RasterImageData::new(canvas.width, canvas.height, canvas.rgba.clone());
                element.special_data = SpecialElementData::Image(Box::new(ImageData::Raster(raster)));
            }
            self.canvases.push((id, canvas.rgba.clone(), poster));
        }
    }

    pub(crate) fn node(&self, key: u64) -> Option<NodeId> {
        self.nodes.get(&key).map(|e| e.id)
    }

    /// The key of the mirrored Blitz node `id`.
    pub(crate) fn key_of(&self, id: NodeId) -> Option<u64> {
        let slot = id.as_u64() as u32 as usize;
        self.keys.get(slot).filter(|(at, _)| *at == id && id != NodeId::default()).map(|&(_, key)| key)
    }

    /// Checks `keys` against `nodes` (debug builds).
    fn debug_check_keys(&self) {
        if cfg!(debug_assertions) {
            let mapped = self.keys.iter().filter(|(id, _)| *id != NodeId::default()).count();
            debug_assert_eq!(mapped, self.nodes.len(), "every mirrored node has its key");
            for (k, e) in &self.nodes {
                debug_assert_eq!(self.key_of(e.id), Some(*k), "node {:?} maps to its key", e.id);
            }
        }
    }

    /// Applies `tree` (the page now) and `sheets` (linked sheet texts by
    /// URL). Returns the number of mutations, or why the document must be
    /// built anew instead.
    pub(crate) fn apply(&mut self, tree: &Arc<Tree>, sheets: &HashMap<String, String>) -> Result<u32, Rebuild> {
        let changed_sheets: Vec<String> =
            sheets.iter().filter(|(url, text)| self.sheets.get(*url) != Some(text)).map(|(u, _)| u.clone()).collect();
        if Arc::ptr_eq(&self.tree, tree) && changed_sheets.is_empty() && sheets.len() == self.sheets.len() {
            return Ok(0);
        }
        let new = &tree.nodes;
        let mut index: FxHashMap<u64, usize> = FxHashMap::with_capacity_and_hasher(new.len(), Default::default());
        for (i, n) in new.iter().enumerate() {
            if index.insert(n.key, i).is_some() {
                return Err(Rebuild::Malformed);
            }
        }
        let (kids, top) = children_of(tree);

        // Gone, or now a node of another kind under the same key.
        let mut removed: FxHashSet<u64> = self
            .nodes
            .iter()
            .filter(|(key, e)| match index.get(*key) {
                None => true,
                Some(&i) => e.name != name_of(&new[i].kind),
            })
            .map(|(k, _)| *k)
            .collect();
        // Elements blitz-dom sets up once, when it builds their box, are
        // replaced when they change: a `<textarea>` (its editor takes the
        // text) and an `<input>` whose type changed. Their children are kept
        // and moved into the new element.
        let empty = Vec::new();
        let mut replace = FxHashSet::default();
        for (i, n) in new.iter().enumerate() {
            let dirty = match self.nodes.get(&n.key) {
                None => true,
                Some(e) => differs(&self.doc, e.id, &n.kind) || e.children != *kids.get(&n.key).unwrap_or(&empty),
            };
            if !dirty {
                continue;
            }
            let mut j = i as u32;
            let mut outer = None;
            // The textarea of a changed text node.
            while j != TOP {
                let node = &new[j as usize];
                if let TreeKind::Element { name, .. } = &node.kind {
                    if name.local == blitz_dom::local_name!("textarea") {
                        outer = Some(node.key);
                    }
                }
                j = node.parent;
            }
            if let TreeKind::Element { name, attrs } = &n.kind {
                if name.local == blitz_dom::local_name!("input") {
                    let is_type = |a: &QualName| a.local == blitz_dom::local_name!("type");
                    let now = attrs.iter().find(|(a, _)| is_type(a)).map(|(_, v)| v.to_ascii_lowercase());
                    let before = self.nodes.get(&n.key).and_then(|e| self.doc.get_node(e.id)).and_then(|node| {
                        let el = node.element_data()?;
                        el.attrs.iter().find(|a| is_type(&a.name)).map(|a| a.value.to_ascii_lowercase())
                    });
                    if now != before {
                        outer = Some(n.key);
                    }
                }
            }
            if let Some(key) = outer.filter(|k| self.nodes.contains_key(k)) {
                replace.insert(key);
            }
        }
        removed.extend(replace);
        let created = new.iter().filter(|n| !self.nodes.contains_key(&n.key) || removed.contains(&n.key)).count();
        if (removed.len() + created) * 2 > new.len().max(self.nodes.len()) {
            return Err(Rebuild::Changes);
        }

        let mut mutations = 0u32;
        let Mirror { doc, nodes, keys, top: old_top, .. } = self;
        let mut m = doc.mutate();

        // 1. Drop removed subtrees, keeping the nodes in them that survive
        // (moved elsewhere): they are detached first and placed below.
        for key in &removed {
            let parent_removed = nodes[key].parent.is_some_and(|p| removed.contains(&p));
            if parent_removed {
                continue;
            }
            let mut stack: Vec<u64> = nodes[key].children.clone();
            while let Some(k) = stack.pop() {
                if removed.contains(&k) {
                    stack.extend(nodes[&k].children.iter().copied());
                } else if let Some(e) = nodes.get(&k) {
                    m.remove_node(e.id);
                }
            }
            m.remove_and_drop_node(nodes[key].id);
            mutations += 1;
        }
        for key in &removed {
            if let Some(e) = nodes.remove(key) {
                set_key(keys, e.id, None);
            }
        }

        // 2. Create new nodes (detached; placed below), update changed ones.
        let mut created = FxHashSet::default();
        for n in new {
            match nodes.get(&n.key) {
                None => {
                    created.insert(n.key);
                    let id = create(&mut m, &n.kind);
                    nodes.insert(n.key, Entry { id, name: name_of(&n.kind), parent: None, children: Vec::new() });
                    set_key(keys, id, Some(n.key));
                    mutations += 1;
                }
                Some(e) => mutations += update(&mut m, e.id, &n.kind) as u32,
            }
        }

        // 3. Children in order, parents first (so a new subtree is attached
        // top-down, as a parser would).
        let root = m.doc.root_node().id;
        // A replaced node keeps its key: its parent's key list looks the same.
        let renewed = |want: &[u64]| want.iter().any(|k| created.contains(k));
        if *old_top != top || renewed(&top) {
            mutations += reorder(&mut m, root, &top, nodes);
            *old_top = top.clone();
        }
        for n in new {
            let want = kids.get(&n.key).unwrap_or(&empty);
            let e = &nodes[&n.key];
            if e.children != *want || renewed(want) {
                let id = e.id;
                mutations += reorder(&mut m, id, want, nodes);
                nodes.get_mut(&n.key).expect("entry").children = want.clone();
            }
        }
        for n in new {
            let parent = (n.parent != TOP).then(|| new[n.parent as usize].key);
            nodes.get_mut(&n.key).expect("entry").parent = parent;
        }

        // 4. Linked sheets whose text changed: setting `href` again reloads
        // them through the provider, which serves the new text.
        if !changed_sheets.is_empty() {
            let href = QualName::new(None, blitz_dom::ns!(), blitz_dom::local_name!("href"));
            for n in new {
                if let TreeKind::Element { name, attrs } = &n.kind {
                    if name.local == blitz_dom::local_name!("link") {
                        if let Some((_, url)) = attrs.iter().find(|(a, _)| a.local == href.local) {
                            if changed_sheets.contains(url) {
                                m.set_attribute(nodes[&n.key].id, href.clone(), url);
                                mutations += 1;
                            }
                        }
                    }
                }
            }
        }
        drop(m);
        self.sheets.clone_from(sheets);
        self.tree = tree.clone();
        self.debug_check_keys();
        Ok(mutations)
    }
}

impl Mirror {
    /// Applies changes since the last tree or delta: nodes changed in place,
    /// and new children lists with the inserted subtrees. Children dropped
    /// from a list and not placed elsewhere are removed. `Err`: the delta
    /// does not fit this document (the caller sends the whole tree).
    pub(crate) fn apply_delta(&mut self, ops: &[DeltaOp]) -> Result<u32, Rebuild> {
        // Check first: nothing is applied from a delta that does not fit.
        // Nodes the ops before (and the current one) create.
        let mut created: FxHashSet<u64> = FxHashSet::default();
        for op in ops {
            match op {
                DeltaOp::Node { key, kind } => {
                    let e = self.nodes.get(key).ok_or(Rebuild::Changes)?;
                    if e.name != name_of(kind) || self.needs_replacing(e.id, kind) {
                        return Err(Rebuild::Changes);
                    }
                }
                DeltaOp::Children { parent, nodes, kids } => {
                    if parent.is_some_and(|p| !self.nodes.contains_key(&p)) {
                        return Err(Rebuild::Changes);
                    }
                    // Every kid is a node already mirrored or created by
                    // this op or an earlier one.
                    created.extend(nodes.iter().map(|n| n.key));
                    if kids.iter().any(|k| !self.nodes.contains_key(k) && !created.contains(k)) {
                        if std::env::var_os("PYRO_MIRRORDBG").is_some() {
                            eprintln!(
                                "mirror-profile unknown kid parent={parent:x?} kids={} new={}",
                                kids.len(),
                                nodes.len()
                            );
                        }
                        return Err(Rebuild::Changes);
                    }
                    for n in nodes {
                        if let Some(e) = self.nodes.get(&n.key) {
                            if e.name != name_of(&n.kind) || self.needs_replacing(e.id, &n.kind) {
                                return Err(Rebuild::Changes);
                            }
                        }
                    }
                }
            }
        }
        let mut mutations = 0u32;
        let Mirror { doc, nodes, keys, top, .. } = self;
        let mut m = doc.mutate();
        let mut dropped: Vec<(u64, Option<u64>)> = Vec::new();
        for op in ops {
            match op {
                DeltaOp::Node { key, kind } => {
                    mutations += update(&mut m, nodes[key].id, kind) as u32;
                }
                DeltaOp::Children { parent, kids, nodes: sub } => {
                    // The inserted subtrees: new nodes created, moved ones updated.
                    for n in sub {
                        match nodes.get(&n.key) {
                            None => {
                                let id = create(&mut m, &n.kind);
                                nodes.insert(
                                    n.key,
                                    Entry { id, name: name_of(&n.kind), parent: None, children: Vec::new() },
                                );
                                set_key(keys, id, Some(n.key));
                                mutations += 1;
                            }
                            Some(e) => mutations += update(&mut m, e.id, &n.kind) as u32,
                        }
                    }
                    let mut kids_of: FxHashMap<u64, Vec<u64>> = FxHashMap::default();
                    for n in sub {
                        let p = if n.parent == TOP { None } else { Some(sub[n.parent as usize].key) };
                        if let Some(p) = p {
                            kids_of.entry(p).or_default().push(n.key);
                        }
                        nodes.get_mut(&n.key).expect("entry").parent = p.or(*parent);
                    }
                    for n in sub {
                        let want = kids_of.remove(&n.key).unwrap_or_default();
                        let e = &nodes[&n.key];
                        for old in missing_from(&e.children, &want) {
                            dropped.push((old, Some(n.key)));
                        }
                        if e.children != want || !want.is_empty() {
                            let id = e.id;
                            mutations += reorder(&mut m, id, &want, nodes);
                            nodes.get_mut(&n.key).expect("entry").children = want;
                        }
                    }
                    // The parent's own list.
                    let (pid, old) = match parent {
                        Some(p) => (nodes[p].id, nodes[p].children.clone()),
                        None => (m.doc.root_node().id, top.clone()),
                    };
                    for k in missing_from(&old, kids) {
                        dropped.push((k, *parent));
                    }
                    mutations += reorder(&mut m, pid, kids, nodes);
                    match parent {
                        Some(p) => nodes.get_mut(p).expect("entry").children = kids.clone(),
                        None => top.clone_from(kids),
                    }
                    for k in kids {
                        nodes.get_mut(k).expect("entry").parent = *parent;
                    }
                }
            }
        }
        // Children dropped from a list and not placed in another: removed
        // with their subtrees.
        for (key, from) in dropped {
            let Some(e) = nodes.get(&key) else { continue };
            if e.parent != from {
                continue;
            }
            let on_list = |p: &Option<u64>| match p {
                Some(p) => nodes.get(p).is_some_and(|pe| pe.children.contains(&key)),
                None => top.contains(&key),
            };
            if on_list(&e.parent) {
                continue;
            }
            m.remove_and_drop_node(e.id);
            mutations += 1;
            let mut stack = vec![key];
            while let Some(k) = stack.pop() {
                if let Some(e) = nodes.remove(&k) {
                    set_key(keys, e.id, None);
                    stack.extend(e.children);
                }
            }
        }
        drop(m);
        // The kept tree no longer describes the document: the next whole
        // tree is diffed in full.
        self.tree = Arc::new(Tree::default());
        self.debug_check_keys();
        Ok(mutations)
    }

    /// blitz-dom sets up an `<input>` once, when it builds its box: a new
    /// `type` replaces the element (the whole-tree path does that).
    fn needs_replacing(&self, id: NodeId, kind: &TreeKind) -> bool {
        let TreeKind::Element { name, attrs } = kind else { return false };
        if name.local != blitz_dom::local_name!("input") {
            return false;
        }
        let is_type = |a: &QualName| a.local == blitz_dom::local_name!("type");
        let now = attrs.iter().find(|(a, _)| is_type(a)).map(|(_, v)| v.to_ascii_lowercase());
        let before = self.doc.get_node(id).and_then(|node| {
            let el = node.element_data()?;
            el.attrs.iter().find(|a| is_type(&a.name)).map(|a| a.value.to_ascii_lowercase())
        });
        // pyro: within the plain text field states (no type, text, search, tel, url, email) the
        // element keeps the same text input data, so a switch updates in place. Danex's search box
        // toggles text/search as it is used, and each switch rebuilt the whole document.
        let text_field = |t: &Option<String>| matches!(t.as_deref(), None | Some("text" | "search" | "tel" | "url" | "email"));
        if text_field(&now) && text_field(&before) {
            return false;
        }
        now != before
    }
}

/// The keys of `old` that are not in `new`, in `old`'s order. Keys are
/// unique within a list, so the two lists' common prefix and suffix hold
/// none of them: only what lies between is compared, and an append, or an
/// edit near either end, of a long children list costs what changed, not
/// `old.len() * new.len()`.
fn missing_from(old: &[u64], new: &[u64]) -> Vec<u64> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = old.iter().rev().zip(new.iter().rev()).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[..old.len() - suffix], &new[..new.len() - suffix]);
    if old.is_empty() {
        return Vec::new();
    }
    if old.len().saturating_mul(new.len()) <= 256 {
        return old.iter().copied().filter(|k| !new.contains(k)).collect();
    }
    let new: FxHashSet<u64> = new.iter().copied().collect();
    old.iter().copied().filter(|k| !new.contains(k)).collect()
}

/// Records `key` (or none) as the key of Blitz node `id`.
fn set_key(keys: &mut Vec<(NodeId, u64)>, id: NodeId, key: Option<u64>) {
    let slot = id.as_u64() as u32 as usize;
    match key {
        Some(key) => {
            if slot >= keys.len() {
                keys.resize(slot + 1, (NodeId::default(), 0));
            }
            keys[slot] = (id, key);
        }
        None => {
            if keys.get(slot).is_some_and(|(at, _)| *at == id) {
                keys[slot] = (NodeId::default(), 0);
            }
        }
    }
}

/// Whether Blitz node `id` differs from `kind`: its text, or its attributes
/// (one not wanted, or a wanted name and value missing), as [`update`]
/// brings them over. A node of the other kind differs.
fn differs(doc: &BaseDocument, id: NodeId, kind: &TreeKind) -> bool {
    let Some(node) = doc.get_node(id) else { return true };
    match kind {
        TreeKind::Text(t) => node.text_data().is_none_or(|text| text.content != *t),
        TreeKind::Element { attrs, .. } => match node.element_data() {
            None => true,
            Some(el) => {
                el.attrs.iter().any(|a| !attrs.iter().any(|(n, _)| *n == a.name))
                    || attrs.iter().any(|(n, v)| !el.attrs.iter().any(|a| a.name == *n && a.value == *v))
            }
        },
    }
}

/// Brings an existing node to `kind` (same element name or both text),
/// comparing against what Blitz holds now. Returns whether it changed it.
fn update(m: &mut DocumentMutator<'_>, id: NodeId, kind: &TreeKind) -> bool {
    if !differs(m.doc, id, kind) {
        return false;
    }
    note_sprite(kind);
    match kind {
        TreeKind::Text(t) => m.set_node_text(id, t),
        TreeKind::Element { name: tag, attrs } => {
            let el = m.doc.get_node(id).and_then(|n| n.element_data());
            let current: Vec<(QualName, String)> =
                el.map(|e| e.attrs.iter().map(|a| (a.name.clone(), a.value.clone())).collect()).unwrap_or_default();
            let has_checkbox = el.is_some_and(|e| e.checkbox_input_checked().is_some());
            for (name, _) in &current {
                if !attrs.iter().any(|(n, _)| n == name) {
                    m.clear_attribute(id, name.clone());
                }
            }
            for (name, value) in attrs {
                if !current.iter().any(|(n, v)| n == name && v == value) {
                    if has_checkbox
                        && tag.local == blitz_dom::local_name!("input")
                        && name.local == blitz_dom::local_name!("checked")
                    {
                        // blitz-dom checks a built box only for a value that
                        // parses as a bool, while a parsed `checked=""` checks
                        // it. Before the box is built the attribute is what it
                        // reads, and a bool there would make blitz-dom push a
                        // second, HTML-namespaced `checked` that the next sync
                        // would take away again.
                        m.set_attribute(id, name.clone(), "true");
                    }
                    m.set_attribute(id, name.clone(), value);
                }
            }
        }
    }
    true
}

/// Makes `want` the children of `parent`, moving as few nodes as it can
/// (an insertion, removal or move costs one operation). Of the children
/// that stay, the longest run already in the wanted order (a longest
/// increasing subsequence of their current positions) stays put and every
/// other wanted node is placed right before its wanted successor, from the
/// end backwards: a rotation by one is one move, not one per node it
/// shifted. Children that do not belong here any more are detached last
/// (their new parent places them), so a wanted node moved out of one of
/// them never leaves the document on the way.
fn reorder(m: &mut DocumentMutator<'_>, parent: NodeId, want: &[u64], nodes: &FxHashMap<u64, Entry>) -> u32 {
    let want: Vec<NodeId> = want.iter().map(|k| nodes[k].id).collect();
    let wanted: FxHashSet<NodeId> = want.iter().copied().collect();
    let mut ops = 0;
    // Position of each staying child among the staying children.
    let mut current: FxHashMap<NodeId, usize> = FxHashMap::default();
    let mut leaving: Vec<NodeId> = Vec::new();
    for id in m.child_ids(parent) {
        if wanted.contains(&id) {
            let next = current.len();
            current.insert(id, next);
        } else {
            leaving.push(id);
        }
    }
    let at: Vec<Option<usize>> = want.iter().map(|id| current.get(id).copied()).collect();
    let keep = longest_increasing(&at);
    for i in (0..want.len()).rev() {
        if keep[i] {
            continue;
        }
        match want.get(i + 1) {
            Some(&next) => m.insert_nodes_before(next, &[want[i]]),
            None => m.append_children(parent, &[want[i]]),
        }
        ops += 1;
    }
    for id in leaving {
        m.remove_node(id);
        ops += 1;
    }
    debug_assert_eq!(m.child_ids(parent).as_slice(), want.as_slice());
    ops
}

/// Marks the entries (among the `Some`s, which are distinct) that form one
/// longest strictly increasing subsequence.
fn longest_increasing(at: &[Option<usize>]) -> Vec<bool> {
    // tails[k]: index into `at` of the smallest tail of an increasing run of
    // length k + 1; prev[i]: the entry before `i` in its run.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; at.len()];
    for (i, v) in at.iter().enumerate() {
        let Some(v) = *v else { continue };
        let k = tails.partition_point(|&t| at[t].expect("tail") < v);
        prev[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut keep = vec![false; at.len()];
    let mut next = tails.last().copied();
    while let Some(i) = next {
        keep[i] = true;
        next = prev[i];
    }
    keep
}

#[cfg(test)]
mod reorder_tests {
    use super::*;

    #[test]
    fn longest_increasing_keeps_one_longest_run() {
        let keep = |at: &[Option<usize>]| -> Vec<usize> {
            longest_increasing(at).iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect()
        };
        assert_eq!(keep(&[]), Vec::<usize>::new());
        assert_eq!(keep(&[Some(0), Some(1), Some(2)]), vec![0, 1, 2]);
        // A rotation: all but the moved one stay.
        assert_eq!(keep(&[Some(3), Some(0), Some(1), Some(2)]), vec![1, 2, 3]);
        assert_eq!(keep(&[Some(1), Some(2), Some(3), Some(0)]), vec![0, 1, 2]);
        // New nodes (`None`) are never kept and do not break a run.
        assert_eq!(keep(&[None, Some(0), None, Some(1)]), vec![1, 3]);
        assert_eq!(keep(&[Some(2), Some(1), Some(0)]).len(), 1);
        assert_eq!(keep(&[Some(0), Some(4), Some(1), Some(2), Some(5), Some(3)]), vec![0, 2, 3, 5]);
    }

    /// Every permutation of five children (plus a new one and a dropped
    /// one) reaches the wanted order, with one operation per node outside
    /// the kept run.
    #[test]
    fn reorder_reaches_every_order_with_the_fewest_moves() {
        fn permutations(n: usize) -> Vec<Vec<usize>> {
            if n == 0 {
                return vec![vec![]];
            }
            let mut out = Vec::new();
            for p in permutations(n - 1) {
                for at in 0..=p.len() {
                    let mut q = p.clone();
                    q.insert(at, n - 1);
                    out.push(q);
                }
            }
            out
        }
        for perm in permutations(5) {
            let mut doc = BaseDocument::new(DocumentConfig::default());
            let root = doc.root_node().id;
            let mut m = doc.mutate();
            let parent = m.create_element(blitz_dom::qual_name!("div"), vec![]);
            m.append_children(root, &[parent]);
            let mut nodes: FxHashMap<u64, Entry> = FxHashMap::default();
            let mut entry = |m: &mut DocumentMutator<'_>, key: u64| {
                let id = m.create_element(blitz_dom::qual_name!("span"), vec![]);
                nodes.insert(key, Entry { id, name: None, parent: None, children: Vec::new() });
                id
            };
            // Children 0..5 and 9 (dropped); 7 is new.
            let old: Vec<NodeId> = (0..5).chain([9]).map(|k| entry(&mut m, k)).collect();
            m.append_children(parent, &old);
            entry(&mut m, 7);
            let mut want: Vec<u64> = perm.iter().map(|&k| k as u64).collect();
            want.insert(2, 7);
            let ops = reorder(&mut m, parent, &want, &nodes);
            let ids: Vec<NodeId> = want.iter().map(|k| nodes[k].id).collect();
            assert_eq!(m.child_ids(parent).to_vec(), ids, "{perm:?}");
            assert!(m.doc.get_node(old[5]).unwrap().parent.is_none(), "dropped child detached");
            let at: Vec<Option<usize>> = perm.iter().map(|&k| Some(k)).collect();
            let kept = longest_increasing(&at).iter().filter(|k| **k).count() as u32;
            // One removal, the new node, and every moved node.
            assert_eq!(ops, 1 + 1 + (5 - kept), "{perm:?}");
        }
    }

    /// A wanted node that sits inside a child leaving the list stays in the
    /// document while it moves: it is placed before the leaving child is
    /// detached, as the DOM moved it (`parent.append(x); c.remove()`), so
    /// its subtree is not torn down and set up again (focus, selection,
    /// stylesheets, subdocuments).
    #[test]
    fn a_node_moved_out_of_a_leaving_child_stays_in_the_document() {
        let mut doc = BaseDocument::new(DocumentConfig::default());
        let root = doc.root_node().id;
        let mut m = doc.mutate();
        let html = m.create_element(blitz_dom::qual_name!("html"), vec![]);
        m.append_children(root, &[html]);
        let mut nodes: FxHashMap<u64, Entry> = FxHashMap::default();
        let mut entry = |m: &mut DocumentMutator<'_>, key: u64, tag: &str| {
            let name = QualName::new(None, blitz_dom::ns!(html), tag.into());
            let id = m.create_element(name, vec![]);
            nodes.insert(key, Entry { id, name: None, parent: None, children: Vec::new() });
            id
        };
        let parent = entry(&mut m, 0, "div");
        let leaving = entry(&mut m, 1, "div");
        let moved = entry(&mut m, 2, "input");
        let stays = entry(&mut m, 3, "span");
        m.append_children(html, &[parent]);
        m.append_children(leaving, &[moved]);
        m.append_children(parent, &[leaving, stays]);
        drop(m);
        doc.set_focus_to(moved);
        assert_eq!(doc.focused_node(), Some(moved));

        let mut m = doc.mutate();
        reorder(&mut m, parent, &[2, 3], &nodes);
        assert_eq!(m.child_ids(parent).to_vec(), vec![moved, stays]);
        assert!(m.doc.get_node(leaving).unwrap().parent.is_none());
        drop(m);
        assert_eq!(doc.focused_node(), Some(moved), "the moved node never left the document");
    }
}
