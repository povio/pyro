#[cfg(feature = "devtools")]
use blitz_traits::devtools::{
    AttributeChanged, CharacterDataChanged, MutationRecord, NodeInserted, NodeRemoved,
};
use blitz_traits::node_id::NodeId;
use markup5ever::LocalName;
use std::collections::HashSet;
use std::mem;
use std::ops::{Deref, DerefMut, Range};

use crate::layout::damage::{ALL_DAMAGE, CHILDREN_CHANGED};
use crate::net::{ImageHandler, ResourceHandler, StylesheetHandler};
use crate::node::{CanvasData, NodeFlags, SpecialElementData};
use crate::stylo_device::DeviceChanges;
use crate::util::ImageType;
use crate::{
    Attribute, BaseDocument, Document, ElementData, Node, NodeData, QualName, local_name, qual_name,
};
use blitz_traits::shell::Viewport;
use selectors::matching::ElementSelectorFlags;
use style::Atom;
use style::invalidation::element::invalidation_map::TSStateForInvalidation;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::stylesheets::OriginSet;
use style_dom::ElementState;
use thin_vec::ThinVec;

macro_rules! tag_and_attr {
    ($tag:tt, $attr:tt) => {
        (&local_name!($tag), &local_name!($attr))
    };
}

/// Buffer a mutation record for the document's devtools provider (if any).
/// The record is only built if there is a provider.
macro_rules! devtools {
    ($mutr:expr, $record:expr) => {
        #[cfg(feature = "devtools")]
        if $mutr.doc.devtools_provider.is_some() {
            $mutr.devtools_records.push($record);
        }
    };
}

#[derive(Debug, Clone)]
pub enum AppendTextErr {
    /// The node is not a text node
    NotTextNode,
}

/// Operations that happen almost immediately, but are deferred within a
/// function for borrow-checker reasons.
enum SpecialOp {
    LoadImage(NodeId),
    LoadIframe(NodeId),
    LoadStylesheet(NodeId),
    UnloadStylesheet(NodeId),
    LoadCustomPaintSource(NodeId),
    ProcessButtonInput(NodeId),
    UnloadSubDocument(NodeId),
    #[cfg(feature = "custom-widget")]
    UnloadCustomWidget(NodeId),
}

pub struct DocumentMutator<'doc> {
    /// Document is public as an escape hatch, but users of this API should ideally avoid using it
    /// and prefer exposing additional functionality in DocumentMutator.
    pub doc: &'doc mut BaseDocument,

    eager_op_queue: Vec<SpecialOp>,
    /// pyro: the attribute being set or cleared is the engine's state (`crate::engine_state`,
    /// authenticated by the document's nonce), under its canonical `urn:x-pyro:state` name: it goes
    /// to the engine's store, and only it drives the top layer, shadow markers and state tokens. A
    /// page's attribute in that namespace is an ordinary attribute.
    state_write: bool,

    // Tracked nodes for deferred processing when mutations have completed
    title_node: Option<NodeId>,
    style_nodes: HashSet<NodeId>,
    /// pyro: `<link>` elements whose `media` changed (`BaseDocument::refresh_link_media` at flush).
    link_media_nodes: HashSet<NodeId>,
    form_nodes: HashSet<NodeId>,

    /// Whether an element/attribute that affect animation status has been seen
    recompute_is_animating: bool,

    /// Whether any mutation that affects rendered output has been performed
    mutations_occurred: bool,

    /// The (latest) node which has been mounted in and had autofocus=true, if any
    #[cfg(feature = "autofocus")]
    node_to_autofocus: Option<NodeId>,

    /// Mutations to report to the document's devtools provider on flush
    #[cfg(feature = "devtools")]
    devtools_records: Vec<MutationRecord>,
}

impl Drop for DocumentMutator<'_> {
    fn drop(&mut self) {
        self.flush(); // Defined at bottom of file
        if self.mutations_occurred {
            self.doc.has_changes = true;
            self.doc.shell_provider.request_redraw();
        }
    }
}

impl DocumentMutator<'_> {
    pub fn new<'doc>(doc: &'doc mut BaseDocument) -> DocumentMutator<'doc> {
        DocumentMutator {
            doc,
            eager_op_queue: Vec::new(),
            state_write: false,
            title_node: None,
            style_nodes: HashSet::new(),
            link_media_nodes: HashSet::new(),
            form_nodes: HashSet::new(),
            recompute_is_animating: false,
            mutations_occurred: false,
            #[cfg(feature = "autofocus")]
            node_to_autofocus: None,
            #[cfg(feature = "devtools")]
            devtools_records: Vec::new(),
        }
    }

    // Query methods

    pub fn node_has_parent(&self, node_id: NodeId) -> bool {
        self.doc.nodes[node_id].parent.is_some()
    }

    pub fn previous_sibling_id(&self, node_id: NodeId) -> Option<NodeId> {
        self.doc.nodes[node_id].backward(1).map(|node| node.id)
    }

    pub fn next_sibling_id(&self, node_id: NodeId) -> Option<NodeId> {
        self.doc.nodes[node_id].forward(1).map(|node| node.id)
    }

    pub fn parent_id(&self, node_id: NodeId) -> Option<NodeId> {
        self.doc.nodes[node_id].parent
    }

    pub fn last_child_id(&self, node_id: NodeId) -> Option<NodeId> {
        self.doc.nodes[node_id].children.last().copied()
    }

    pub fn child_ids(&self, node_id: NodeId) -> ThinVec<NodeId> {
        self.doc.nodes[node_id].children.clone()
    }

    pub fn element_name(&self, node_id: NodeId) -> Option<&QualName> {
        self.doc.nodes[node_id].element_data().map(|el| &el.name)
    }

    pub fn node_at_path(&self, start_node_id: NodeId, path: &[u8]) -> NodeId {
        let mut current = &self.doc.nodes[start_node_id];
        for i in path {
            let new_id = current.children[*i as usize];
            current = &self.doc.nodes[new_id];
        }
        current.id
    }

    // Node creation methods

    pub fn create_comment_node(&mut self, contents: &str) -> NodeId {
        self.doc.create_node(NodeData::Comment {
            contents: contents.to_string(),
        })
    }

    pub fn create_text_node(&mut self, text: &str) -> NodeId {
        self.doc.create_text_node(text)
    }

    pub fn create_element(&mut self, name: QualName, attrs: Vec<Attribute>) -> NodeId {
        // pyro: the engine's attributes (under this document's nonce names) go to its store, by
        // canonical name; every other attribute, the page's `data-pyro-*` too, is the element's.
        let nonce = self.doc.frames.nonce.clone();
        // pyro: and its state attributes (`urn:x-pyro:state:<nonce>`), by local name.
        let (state, attrs): (Vec<Attribute>, Vec<Attribute>) =
            attrs.into_iter().partition(|a| crate::engine_state(&nonce, &a.name).is_some());
        // pyro: the top layer is marked in pyro-dom's state attribute.
        let pyro_state = state.iter().find(|a| &*a.name.local == "state").map(|a| a.value.clone());
        let (engine, attrs): (Vec<(String, Attribute)>, Vec<(String, Attribute)>) = attrs
            .into_iter()
            .map(|a| (crate::engine_field(&nonce, &a.name).unwrap_or_default(), a))
            .partition(|(field, _)| !field.is_empty());
        let mut data = ElementData::new(name, attrs.into_iter().map(|(_, a)| a).collect());
        for (field, a) in engine {
            data.attrs.set_reserved(&field, &a.value);
        }
        for a in &state {
            data.attrs.set_state(&a.name.local, &a.value);
        }
        if !state.is_empty() {
            data.sync_lp_attribute_state();
        }
        data.flush_style_attribute(self.doc.guard(), &self.doc.url.url_extra_data());
        // pyro: a link to the document itself is visited.
        if data.is_link() {
            let (base, document) = self.doc.link_urls();
            data.flush_link_visited(&base, &document);
        }

        let id = self.doc.create_node(NodeData::Element(Box::new(data)));
        let node = self.doc.get_node_mut(id).unwrap();

        // Initialise style data
        *node.stylo_element_data_mut().ensure_init_mut() = style::data::ElementData {
            damage: ALL_DAMAGE,
            ..Default::default()
        };

        if let Some(state) = pyro_state {
            self.doc.note_lp_state(id, Some(&state));
        }
        // pyro: a shadow host or an assigned slot (`crate::shadow`).
        if state.iter().any(|a| crate::shadow::is_shadow_marker(&QualName::new(None, crate::PYRO_STATE_NS.into(), a.name.local.clone()))) {
            self.doc.sync_shadow_markers(id);
        }
        id
    }

    pub fn deep_clone_node(&mut self, node_id: NodeId) -> NodeId {
        self.doc.deep_clone_node(node_id)
    }

    /// Create (or return the existing) "template contents" fragment node for a
    /// `<template>` element. The contents node is detached from the tree (so it
    /// is inert: never styled or rendered) and is exposed to JavaScript as the
    /// template's `content` DocumentFragment.
    pub fn template_contents(&mut self, template_id: NodeId) -> NodeId {
        if let Some(contents_id) = self.try_template_contents(template_id) {
            return contents_id;
        }
        let name = QualName::new(
            None,
            markup5ever::ns!(html),
            markup5ever::LocalName::from("#document-fragment"),
        );
        let contents_id = self.create_element(name, Vec::new());
        if let Some(element) = self
            .doc
            .get_node_mut(template_id)
            .and_then(|node| node.element_data_mut())
        {
            element.template_contents = Some(contents_id);
        }
        contents_id
    }

    /// The "template contents" fragment node of a `<template>` element (if any)
    pub fn try_template_contents(&self, template_id: NodeId) -> Option<NodeId> {
        self.doc
            .get_node(template_id)
            .and_then(|node| node.element_data())
            .and_then(|element| element.template_contents)
    }

    // Node mutation methods

    /// Set the text of a text or comment node
    pub fn set_node_text(&mut self, node_id: NodeId, value: &str) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        let node = &mut self.doc.nodes[node_id];

        let text = match node.data {
            NodeData::Text(ref mut text) => text,
            // Comments are not rendered, so changing one doesn't damage anything
            NodeData::Comment { ref mut contents } => {
                if *contents != value {
                    *contents = value.to_string();
                    if node_is_in_document {
                        devtools!(
                            self,
                            MutationRecord::CharacterDataChanged(CharacterDataChanged { node_id })
                        );
                    }
                }
                return;
            }
            // TODO: otherwise this is basically element.textContent which is a bit different - need to parse as html
            _ => return,
        };

        let changed = text.content != value;
        if changed {
            let emptiness_flipped = text.content.is_empty() != value.is_empty();
            self.mutations_occurred |= node_is_in_document;
            text.content.clear();
            text.content.push_str(value);
            node.insert_damage(ALL_DAMAGE);
            if node_is_in_document {
                devtools!(
                    self,
                    MutationRecord::CharacterDataChanged(CharacterDataChanged { node_id })
                );
            }
            // Mark ancestors dirty so the style traversal visits this subtree.
            // Without this, the traversal may skip nodes with pending damage.
            node.mark_ancestors_dirty();
            let parent_id = node.parent;

            // Also insert damage on the parent element, since text content changes
            // affect the parent's layout (text may wrap differently, change size, etc.)
            if let Some(parent_id) = parent_id {
                let parent = &mut self.doc.nodes[parent_id];
                parent.insert_damage(CHILDREN_CHANGED);
                self.respace_operators(parent_id);
                if emptiness_flipped {
                    self.restyle_for_text_emptiness_change(parent_id);
                }
            }

            self.maybe_record_node(parent_id);
        }
    }

    pub fn append_text_to_node(
        &mut self,
        node_id: NodeId,
        text: &str,
    ) -> Result<(), AppendTextErr> {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        let node = &mut self.doc.nodes[node_id];
        node.insert_damage(ALL_DAMAGE);
        node.mark_ancestors_dirty();
        let parent_id = node.parent;
        match node.text_data_mut() {
            Some(data) => {
                let emptiness_flipped = data.content.is_empty() && !text.is_empty();
                data.content += text;
                self.mutations_occurred |= node_is_in_document;
                if node_is_in_document {
                    devtools!(
                        self,
                        MutationRecord::CharacterDataChanged(CharacterDataChanged { node_id })
                    );
                }
                if let Some(parent_id) = parent_id {
                    self.doc.nodes[parent_id].insert_damage(CHILDREN_CHANGED);
                    self.respace_operators(parent_id);
                    if emptiness_flipped {
                        self.restyle_for_text_emptiness_change(parent_id);
                    }
                }
                Ok(())
            }
            None => Err(AppendTextErr::NotTextNode),
        }
    }

    pub fn add_attrs_if_missing(&mut self, node_id: NodeId, attrs: Vec<Attribute>) {
        let node = &mut self.doc.nodes[node_id];
        node.insert_damage(ALL_DAMAGE);
        let element_data = node.element_data_mut().expect("Not an element");

        let existing_names = element_data
            .attrs
            .iter()
            .map(|e| e.name.clone())
            .collect::<HashSet<_>>();

        for attr in attrs
            .into_iter()
            .filter(|attr| !existing_names.contains(&attr.name))
        {
            self.set_attribute(node_id, attr.name, &attr.value);
        }
    }

    /// pyro: `multiple` / `size` switch a select between a
    /// drop-down (no boxes for its options) and a list box (options laid
    /// out): its boxes are built again.
    fn rebuild_select_for_attribute(&mut self, node_id: NodeId, name: &QualName) {
        if !matches!(name.local.as_ref() as &str, "multiple" | "size") {
            return;
        }
        let node = &mut self.doc.nodes[node_id];
        if node.data.downcast_element().is_some_and(|el| el.name.local == local_name!("select")) {
            node.insert_damage(ALL_DAMAGE);
            node.mark_ancestors_dirty();
        }
    }

    /// pyro: a customizable select's picker (`appearance: base-select`) lays its options out
    /// while it is open (`:open`, from pyro-dom's state tokens): the select's boxes are built
    /// again when that state flips.
    fn rebuild_select_for_open_state(&mut self, node_id: NodeId, name: &QualName, value: Option<&str>) {
        if !is_lp_state(name) {
            return;
        }
        let node = &mut self.doc.nodes[node_id];
        let Some(el) = node.element_data() else { return };
        if el.name.local != local_name!("select") {
            return;
        }
        let open = crate::has_invalidation::pyro_state_bits(value).contains(ElementState::OPEN);
        if el.element_state.contains(ElementState::OPEN) != open {
            node.insert_damage(ALL_DAMAGE);
            node.mark_ancestors_dirty();
        }
    }

    /// pyro: a text input lays its `placeholder` out with its box: the box is
    /// built again when the attribute changes.
    fn rebuild_text_input_for_attribute(&mut self, node_id: NodeId, name: &QualName) {
        if name.local != local_name!("placeholder") {
            return;
        }
        let node = &mut self.doc.nodes[node_id];
        if node.element_data().is_some_and(|el| el.text_input_data().is_some()) {
            node.insert_damage(ALL_DAMAGE);
            node.mark_ancestors_dirty();
        }
    }

    pub fn set_attribute(&mut self, node_id: NodeId, name: QualName, value: &str) {
        // pyro: an engine attribute goes to the engine's store (`create_element`).
        if let Some(field) = crate::engine_field(&self.doc.frames.nonce.clone(), &name) {
            if let Some(element) = self.doc.nodes[node_id].element_data_mut() {
                element.attrs.set_reserved(&field, value);
            }
            self.engine_attribute_changed(node_id, &field, true);
            return;
        }
        // pyro: the engine's state, under its canonical name (`state_write`).
        if let Some(local) = crate::engine_state(&self.doc.frames.nonce.clone(), &name) {
            let previous = std::mem::replace(&mut self.state_write, true);
            self.set_attribute_inner(node_id, QualName::new(None, crate::PYRO_STATE_NS.into(), local.into()), value);
            self.state_write = previous;
            return;
        }
        let previous = std::mem::replace(&mut self.state_write, false);
        self.set_attribute_inner(node_id, name, value);
        self.state_write = previous;
    }

    fn set_attribute_inner(&mut self, node_id: NodeId, name: QualName, value: &str) {
        if self.state_write && is_lp_state(&name) && self.doc.nodes[node_id].is_element() {
            self.doc.note_lp_state(node_id, Some(value));
        }
        // pyro: `:has()` anchors that can see the element follow
        // the change (`has_invalidation`); `None` (one flag test) when no
        // `:has()` search visited it.
        let change = crate::has_invalidation::AttributeChange::before(self.doc, node_id, &name);
        self.rebuild_select_for_attribute(node_id, &name);
        self.rebuild_select_for_open_state(node_id, &name, Some(value));
        self.rebuild_text_input_for_attribute(node_id, &name);
        let shadow_marker = self.state_write && crate::shadow::is_shadow_marker(&name);
        // pyro: `quotes: auto` follows the language (`layout::counters`).
        // pyro: `start`, `reversed` and `value` number list items (`layout::counters`).
        if matches!(
            name.local,
            local_name!("lang") | local_name!("start") | local_name!("reversed") | local_name!("value")
        ) {
            self.doc.counters_dirty = true;
        }
        self.set_attribute_impl(node_id, name, value);
        if shadow_marker {
            self.doc.sync_shadow_markers(node_id);
        }
        if let Some(change) = change.and_then(|c| c.after(self.doc, node_id)) {
            self.doc.invalidate_relative_attribute(node_id, change);
        }
        // pyro: `:has-slotted(<selector>)` of its slot (`crate::shadow`).
        self.doc.invalidate_has_slotted(node_id);
        // pyro: and the children a `<details>` element renders in its slots (`details:not([open])
        // > *`).
        self.doc.invalidate_slotted_children(node_id);
    }

    fn set_attribute_impl(&mut self, node_id: NodeId, name: QualName, value: &str) {
        let engine = self.state_write;
        // pyro: what a link's visited state is decided against (`flush_link_visited`), for an
        // `href` change only.
        let link_urls = (name.local == local_name!("href")).then(|| self.doc.link_urls());
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        if node_is_in_document {
            self.doc.snapshot_node(node_id, &name.local);

            self.restyle_for_attribute_change(node_id, &name.local);
            self.restyle_siblings_for_attribute_change(node_id, &name, Some(value));
        }

        // pyro: a `<style>`'s `media` is its sheet's media list (`process_style_element`).
        if name.local == local_name!("media")
            && node_is_in_document
            && self.doc.nodes[node_id].element_data().is_some_and(|e| e.name.local == local_name!("style"))
        {
            self.style_nodes.insert(node_id);
        }
        // pyro: a `<link>`'s `media` is its sheet's media list too (`refresh_link_media`).
        if name.local == local_name!("media")
            && node_is_in_document
            && self.doc.nodes[node_id].element_data().is_some_and(|e| e.name.local == local_name!("link"))
        {
            self.link_media_nodes.insert(node_id);
        }

        if name.local == local_name!("id") && node_is_in_document {
            if let Some(old_id) = self.doc.nodes[node_id]
                .element_data()
                .map(|element| element.id.clone())
            {
                if let Some(old_id) = old_id {
                    self.doc.remove_from_id_map(&old_id, node_id);
                }
                self.doc.add_to_id_map(value, node_id);
            }
        }

        let node = &mut self.doc.nodes[node_id];

        let NodeData::Element(ref mut element) = node.data else {
            return;
        };

        self.mutations_occurred |= node_is_in_document;
        // If element is a CustomWidget, then Ccall attribute_changed on it
        #[cfg(feature = "custom-widget")]
        if let SpecialElementData::CustomWidget(widget_data) = &mut element.special_data {
            let old_value = element.attrs.get(&name).as_ref().map(|attr| &*attr.value);
            widget_data
                .widget
                .attribute_changed(&name.local, old_value, Some(value));
        }

        // pyro: the engine's state goes to its store (`state_write`).
        if engine {
            element.attrs.set_state(&name.local, value);
        } else {
            element.attrs.set(name.clone(), value);
        }
        if is_state_attribute(&element.name, &name, engine) {
            element.sync_lp_attribute_state();
        }
        if node_is_in_document {
            devtools!(
                self,
                MutationRecord::AttributeChanged(AttributeChanged {
                    node_id,
                    name: blitz_traits::SmolStr::new(&*name.local),
                })
            );
        }

        // Focusability is cached on the element and comes from these
        // attributes, so it has to follow a change to one of them: a widget
        // that hands the focus around its own children - a menu, a grid -
        // sets their tabindex after creating them.
        if name.local == local_name!("tabindex")
            || name.local == local_name!("href")
            || name.local == local_name!("disabled")
        {
            element.flush_is_focussable();
        }

        if name.local == local_name!("href") {
            element.flush_link_state();
            if let Some((base, document)) = &link_urls {
                element.flush_link_visited(base, document);
            }
        }

        if name.local == local_name!("open") {
            element.flush_open_state();
        }

        let tag = &element.name.local;
        let attr = &name.local;

        // pyro: only the `id` attribute in no namespace is the element's ID (`xml:id` is not;
        // WPT selectors historical-xmlid).
        if *attr == local_name!("id") && name.ns == markup5ever::ns!() {
            element.id = Some(Atom::from(value))
        }

        // pyro: the `class` attribute in no namespace only, as for `id`.
        if *attr == local_name!("class") && name.ns == markup5ever::ns!() {
            element.classes = crate::node::parse_classes(value);
        }

        if *attr == local_name!("value") {
            if let Some(input_data) = element.text_input_data_mut() {
                // Update text input value
                input_data.set_text(
                    &mut self.doc.font_ctx.lock().unwrap(),
                    &mut self.doc.layout_ctx,
                    value,
                );
            }
            return;
        }

        if *attr == local_name!("style") || (engine && &**attr == "inline-declarations") {
            element.flush_style_attribute(&self.doc.guard, &self.doc.url.url_extra_data());
            node.set_restyle_hint(RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
            return;
        }

        // pyro: an element with pyro-dom's state tokens gets `:enabled`/`:disabled` from them.
        if *attr == local_name!("disabled") && element.can_be_disabled() && !element.has_pyro_state() {
            node.disable();
            return;
        }

        // If node if not in the document, then don't apply any special behaviours
        // and simply set the attribute value
        if !node.flags.is_in_document() {
            // Except checkedness, which is read back from the checkbox state once it exists
            if (tag, attr) == tag_and_attr!("input", "checked")
                && element.checkbox_input_checked().is_some()
            {
                set_input_checked_state(element, value.to_string());
            }
            return;
        }

        // Custom widgets replace the element's built-in tag-specific behaviour
        #[cfg(feature = "custom-widget")]
        if element.custom_widget_data().is_some() {
            return;
        }

        if (tag, attr) == tag_and_attr!("input", "checked") {
            set_input_checked_state(element, value.to_string());
        } else if &**tag == "img" && matches!(&**attr, "src" | "crossorigin" | "referrerpolicy") {
            self.eager_op_queue.push(SpecialOp::LoadImage(node_id));
        } else if (tag, attr) == tag_and_attr!("embed", "src")
            // pyro: an image button fetches its `src` like `<img>`;
            // `type` becoming "image" starts the load (HTML "image button
            // state": the type changing to Image Button updates the image data).
            || (*tag == local_name!("input")
                && (*attr == local_name!("src") || *attr == local_name!("type"))
                && crate::layout::replaced::is_image_input(element))
        {
            self.load_image(node_id);
        } else if (tag, attr) == tag_and_attr!("video", "poster")
            // pyro: an `<object>`'s image (`load_image`).
            || (tag, attr) == tag_and_attr!("object", "data")
            || (tag, attr) == tag_and_attr!("object", "type")
        {
            self.load_image(node_id);
        } else if (tag, attr) == tag_and_attr!("canvas", "src") {
            self.load_custom_paint_src(node_id);
        } else if (tag, attr) == tag_and_attr!("link", "href")
            || (tag, attr) == tag_and_attr!("link", "rel")
        {
            self.load_linked_stylesheet(node_id);
        } else if (tag, attr) == tag_and_attr!("iframe", "src")
            || (tag, attr) == tag_and_attr!("iframe", "srcdoc")
            || (tag, attr) == tag_and_attr!("object", "srcdoc")
            || (tag, attr) == tag_and_attr!("embed", "srcdoc")
            || (matches!(&**tag, "iframe" | "frame" | "object" | "embed") && &**attr == crate::FRAME_ATTRIBUTE)
        {
            self.load_iframe(node_id);
        }
    }

    /// pyro: the engine's attribute `field` (canonical) of `node_id` was set (`set`) or removed:
    /// a frame marker loads or drops the frame document; any one lays the element out again.
    fn engine_attribute_changed(&mut self, node_id: NodeId, field: &str, set: bool) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        let tag = self.doc.nodes[node_id].element_data().map(|e| e.name.local.clone());
        if node_is_in_document {
            self.doc.snapshot_node(node_id, &markup5ever::LocalName::from(field));
            self.restyle_for_attribute_change(node_id, &markup5ever::LocalName::from(field));
        }
        if field != crate::FRAME_ATTRIBUTE {
            return;
        }
        match tag.as_deref() {
            Some("iframe" | "frame") if set || node_is_in_document => self.load_iframe(node_id),
            Some("object" | "embed") if set => self.load_iframe(node_id),
            Some("object" | "embed") => {
                self.remove_sub_document(node_id);
                self.load_image(node_id);
            }
            _ => {}
        }
    }

    pub fn clear_attribute(&mut self, node_id: NodeId, name: QualName) {
        if let Some(field) = crate::engine_field(&self.doc.frames.nonce.clone(), &name) {
            if let Some(element) = self.doc.nodes[node_id].element_data_mut() {
                element.attrs.remove_reserved(&field);
            }
            self.engine_attribute_changed(node_id, &field, false);
            return;
        }
        if let Some(local) = crate::engine_state(&self.doc.frames.nonce.clone(), &name) {
            let previous = std::mem::replace(&mut self.state_write, true);
            self.clear_attribute_inner(node_id, QualName::new(None, crate::PYRO_STATE_NS.into(), local.into()));
            self.state_write = previous;
            return;
        }
        let previous = std::mem::replace(&mut self.state_write, false);
        self.clear_attribute_inner(node_id, name);
        self.state_write = previous;
    }

    fn clear_attribute_inner(&mut self, node_id: NodeId, name: QualName) {
        if self.state_write && is_lp_state(&name) {
            self.doc.note_lp_state(node_id, None);
        }
        // pyro: as in `set_attribute`.
        let change = crate::has_invalidation::AttributeChange::before(self.doc, node_id, &name);
        self.rebuild_select_for_attribute(node_id, &name);
        self.rebuild_select_for_open_state(node_id, &name, None);
        self.rebuild_text_input_for_attribute(node_id, &name);
        let shadow_marker = self.state_write && crate::shadow::is_shadow_marker(&name);
        // pyro: `quotes: auto` follows the language (`layout::counters`).
        // pyro: `start`, `reversed` and `value` number list items (`layout::counters`).
        if matches!(
            name.local,
            local_name!("lang") | local_name!("start") | local_name!("reversed") | local_name!("value")
        ) {
            self.doc.counters_dirty = true;
        }
        self.clear_attribute_impl(node_id, name);
        if shadow_marker {
            self.doc.sync_shadow_markers(node_id);
        }
        if let Some(change) = change.and_then(|c| c.after(self.doc, node_id)) {
            self.doc.invalidate_relative_attribute(node_id, change);
        }
        // pyro: `:has-slotted(<selector>)` of its slot (`crate::shadow`).
        self.doc.invalidate_has_slotted(node_id);
        // pyro: and the children a `<details>` element renders in its slots (`details:not([open])
        // > *`).
        self.doc.invalidate_slotted_children(node_id);
    }

    fn clear_attribute_impl(&mut self, node_id: NodeId, name: QualName) {
        let engine = self.state_write;
        // pyro: what a link's visited state is decided against (`flush_link_visited`), for an
        // `href` change only.
        let link_urls = (name.local == local_name!("href")).then(|| self.doc.link_urls());
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        if node_is_in_document {
            self.doc.snapshot_node(node_id, &name.local);

            self.restyle_for_attribute_change(node_id, &name.local);
            self.restyle_siblings_for_attribute_change(node_id, &name, None);
        }

        // pyro: without its `media`, a `<style>`'s sheet is for all media again.
        if name.local == local_name!("media")
            && node_is_in_document
            && self.doc.nodes[node_id].element_data().is_some_and(|e| e.name.local == local_name!("style"))
        {
            self.style_nodes.insert(node_id);
        }
        // pyro: a `<link>`'s `media` is its sheet's media list too (`refresh_link_media`).
        if name.local == local_name!("media")
            && node_is_in_document
            && self.doc.nodes[node_id].element_data().is_some_and(|e| e.name.local == local_name!("link"))
        {
            self.link_media_nodes.insert(node_id);
        }

        if name.local == local_name!("id") && node_is_in_document {
            if let Some(old_id) = self.doc.nodes[node_id]
                .element_data()
                .and_then(|element| element.id.clone())
            {
                self.doc.remove_from_id_map(&old_id, node_id);
            }
        }

        let node = &mut self.doc.nodes[node_id];

        let Some(element) = node.element_data_mut() else {
            return;
        };

        let removed_attr = if engine { element.attrs.remove_state(&name.local) } else { element.attrs.remove(&name) };
        let had_attr = removed_attr.is_some();
        if !had_attr {
            return;
        }
        if is_state_attribute(&element.name, &name, engine) {
            element.sync_lp_attribute_state();
        }
        self.mutations_occurred |= node_is_in_document;
        if node_is_in_document {
            devtools!(
                self,
                MutationRecord::AttributeChanged(AttributeChanged {
                    node_id,
                    name: blitz_traits::SmolStr::new(&*name.local),
                })
            );
        }

        // If element is a CustomWidget, then call attribute_changed on it
        #[cfg(feature = "custom-widget")]
        if let SpecialElementData::CustomWidget(widget_data) = &mut element.special_data {
            let old_value = removed_attr.as_ref().map(|attr| &*attr.value);
            widget_data
                .widget
                .attribute_changed(&name.local, old_value, None);
        }

        if name.local == local_name!("id") && name.ns == markup5ever::ns!() {
            element.id = None;
        }

        if name.local == local_name!("class") && name.ns == markup5ever::ns!() {
            element.classes.clear();
        }

        // As in `set_attribute`: taking one of these away can make the element
        // unfocusable again.
        if name.local == local_name!("tabindex")
            || name.local == local_name!("href")
            || name.local == local_name!("disabled")
        {
            element.flush_is_focussable();
        }

        if name.local == local_name!("href") {
            element.flush_link_state();
            if let Some((base, document)) = &link_urls {
                element.flush_link_visited(base, document);
            }
        }

        if name.local == local_name!("open") {
            element.flush_open_state();
        }

        // Update text input value
        if name.local == local_name!("value") {
            if let Some(input_data) = element.text_input_data_mut() {
                input_data.set_text(
                    &mut self.doc.font_ctx.lock().unwrap(),
                    &mut self.doc.layout_ctx,
                    "",
                );
            }
        }

        let tag = &element.name.local;
        let attr = &name.local;

        if *attr == local_name!("disabled") && element.can_be_disabled() && !element.has_pyro_state() {
            node.enable();
            return;
        }

        if *attr == local_name!("style") || (engine && &**attr == "inline-declarations") {
            element.flush_style_attribute(&self.doc.guard, &self.doc.url.url_extra_data());
            node.set_restyle_hint(RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        } else if (tag, attr) == tag_and_attr!("input", "checked") {
            element.set_checkbox_input_checked(false);
        } else if (tag, attr) == tag_and_attr!("canvas", "src") {
            self.recompute_is_animating = true;
        } else if (tag, attr) == tag_and_attr!("link", "href") || (tag, attr) == tag_and_attr!("link", "rel") {
            // pyro: without its `rel` too (as a `rel` without `stylesheet`: `load_linked_stylesheet`).
            self.unload_stylesheet(node_id);
        } else if matches!(&**tag, "object" | "embed") && &**attr == crate::FRAME_ATTRIBUTE {
            // pyro: the snapshot no longer gives it a frame document: its image or fallback again.
            self.remove_sub_document(node_id);
            self.load_image(node_id);
        } else if ((tag, attr) == tag_and_attr!("iframe", "srcdoc")
            || (matches!(&**tag, "iframe" | "frame") && &**attr == crate::FRAME_ATTRIBUTE))
            && node_is_in_document
        {
            // Fall back to loading from the `src` attribute (if any)
            self.load_iframe(node_id);
        } else if (tag, attr) == tag_and_attr!("object", "srcdoc") || (tag, attr) == tag_and_attr!("embed", "srcdoc") {
            // pyro: the object's frame document is gone: its image or fallback again.
            self.remove_sub_document(node_id);
            self.load_image(node_id);
        } else if &**tag == "img" && matches!(&**attr, "src" | "crossorigin" | "referrerpolicy") && node_is_in_document {
            self.eager_op_queue.push(SpecialOp::LoadImage(node_id));
        } else if matches!((&**tag, &**attr), ("object", "data" | "type") | ("embed", "src") | ("video", "poster")) {
            // pyro: an `<object>` without its image shows its fallback again (`load_image`).
            self.load_image(node_id);
        }
    }

    pub fn set_style_property(&mut self, node_id: NodeId, name: &str, value: &str) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.set_style_property(node_id, name, value);
        self.mutations_occurred |= node_is_in_document;
    }

    pub fn remove_style_property(&mut self, node_id: NodeId, name: &str) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.remove_style_property(node_id, name);
        self.mutations_occurred |= node_is_in_document;
    }

    pub fn set_sub_document(&mut self, node_id: NodeId, sub_document: Box<dyn Document>) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.set_sub_document(node_id, sub_document);
        self.mutations_occurred |= node_is_in_document;
    }

    pub fn remove_sub_document(&mut self, node_id: NodeId) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.remove_sub_document(node_id);
        self.mutations_occurred |= node_is_in_document;
    }

    #[cfg(feature = "custom-widget")]
    pub fn set_custom_widget(&mut self, node_id: NodeId, widget: Box<dyn crate::Widget>) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.set_custom_widget(node_id, widget);
        self.mutations_occurred |= node_is_in_document;
    }

    #[cfg(feature = "custom-widget")]
    pub fn remove_custom_widget(&mut self, node_id: NodeId) {
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.doc.remove_custom_widget(node_id);
        self.mutations_occurred |= node_is_in_document;
    }

    /// Remove the node from it's parent but don't drop it
    pub fn remove_node(&mut self, node_id: NodeId) {
        // A removed element inside a `:has()` argument changes its anchors.
        self.doc.invalidate_relative_selectors_for_removal(node_id);
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        // Process the subtree *before* severing the parent link so that
        // interaction state referencing removed nodes can retarget to the
        // nearest surviving ancestor.
        self.process_removed_subtree(node_id);
        self.doc.unlist_hoisted_leaving(node_id);

        let node = &mut self.doc.nodes[node_id];

        // Update child_idx values
        if let Some(parent_id) = node.parent.take() {
            self.mutations_occurred |= node_is_in_document;
            let parent = &mut self.doc.nodes[parent_id];
            parent.insert_damage(CHILDREN_CHANGED);
            let index = parent.index_of_child(node_id).unwrap_or(0);
            parent.children.retain(|id| *id != node_id);
            if node_is_in_document {
                let removed_element = self.doc.nodes[node_id].is_element();
                self.restyle_for_child_list_change(parent_id, index..index, removed_element);
                devtools!(
                    self,
                    MutationRecord::NodeRemoved(NodeRemoved { parent_id, node_id })
                );
            }
            self.maybe_record_node(parent_id);
        }
    }

    pub fn remove_and_drop_node(&mut self, node_id: NodeId) -> Option<Node> {
        self.remove_and_drop_node_with(node_id, &mut |_| {})
    }

    /// Like [`Self::remove_and_drop_node`], but calls `on_drop` with the id of
    /// every dropped node (the node itself and all of its descendants).
    pub fn remove_and_drop_node_with(
        &mut self,
        node_id: NodeId,
        on_drop: &mut dyn FnMut(NodeId),
    ) -> Option<Node> {
        // pyro: a dropped element inside a `:has()` argument
        // changes its anchors, as in `remove_node`.
        self.doc.invalidate_relative_selectors_for_removal(node_id);
        let node_is_in_document = self.doc.nodes[node_id].flags.is_in_document();
        self.process_removed_subtree(node_id);
        self.doc.unlist_hoisted_leaving(node_id);

        let node = self.doc.drop_node_ignoring_parent_with(node_id, on_drop);
        self.mutations_occurred |= node_is_in_document;

        // Update child_idx values
        if let Some(parent_id) = node.as_ref().and_then(|node| node.parent) {
            let parent = &mut self.doc.nodes[parent_id];
            parent.insert_damage(CHILDREN_CHANGED);
            let parent_is_in_doc = parent.flags.is_in_document();
            let index = parent.index_of_child(node_id).unwrap_or(0);
            parent.children.retain(|id| *id != node_id);

            if parent_is_in_doc {
                let removed_element = node.as_ref().is_some_and(|node| node.is_element());
                self.restyle_for_child_list_change(parent_id, index..index, removed_element);
                devtools!(
                    self,
                    MutationRecord::NodeRemoved(NodeRemoved { parent_id, node_id })
                );
            }

            self.maybe_record_node(parent_id);
        }

        node
    }

    pub fn remove_and_drop_all_children(&mut self, node_id: NodeId) {
        // pyro: dropped elements inside a `:has()` argument change their anchors,
        // as in `remove_node`; checked while they are still linked.
        for &child_id in &self.doc.nodes[node_id].children {
            self.doc.invalidate_relative_selectors_for_removal(child_id);
        }
        let parent = &mut self.doc.nodes[node_id];
        let parent_is_in_doc = parent.flags.is_in_document();
        let children = mem::take(&mut parent.children);
        self.mutations_occurred |= parent_is_in_doc && !children.is_empty();

        if parent_is_in_doc {
            let removed_element = children.iter().any(|id| self.doc.nodes[*id].is_element());
            self.restyle_for_child_list_change(node_id, 0..0, removed_element);
        }

        for child_id in children {
            self.process_removed_subtree(child_id);
            let _ = self.doc.drop_node_ignoring_parent(child_id);
            if parent_is_in_doc {
                devtools!(
                    self,
                    MutationRecord::NodeRemoved(NodeRemoved {
                        parent_id: node_id,
                        node_id: child_id,
                    })
                );
            }
        }
        self.maybe_record_node(node_id);
    }

    /// Restyle an in-document element whose attribute is about to change. Only the element
    /// is hinted: its snapshot lets Stylo's invalidator find the siblings and descendants
    /// whose selectors depend on the attribute. `style`, `class` and `id` reach layout only
    /// through the computed style (whose diff yields the damage); other attributes may not.
    /// pyro: restyles the siblings of `node_id` (and their subtrees) for an attribute
    /// change Stylo's snapshot invalidation cannot follow to them: a selector list of
    /// `:nth-child(... of S)` (the parent carries `HAS_SLOW_SELECTOR_NTH_OF`), which Stylo
    /// leaves to the embedder, as Gecko's `RestyleSiblingsForNthOf` does. The pseudo-classes
    /// pyro's state attribute (`urn:x-pyro:state`) carries are element
    /// states ([`ElementData::sync_lp_attribute_state`]), so the snapshot sees them change and
    /// Stylo's state invalidation reaches the siblings and descendants (`.r:invalid + .l`)
    /// itself; only its `natural=W,H` token (an image's natural size), which layout reads, is
    /// layout damage here when it changes.
    /// Upstream restyled the parent's subtree on every attribute change; it now hints the element
    /// alone (blitz#1276).
    fn restyle_siblings_for_attribute_change(&mut self, node_id: NodeId, name: &QualName, value: Option<&str>) {
        if self.state_write && is_lp_state(name) {
            let natural =
                |v: &str| v.split_ascii_whitespace().find(|t| t.starts_with("natural=")).map(str::to_owned);
            let node = &mut self.doc.nodes[node_id];
            let old = node
                .element_data()
                .and_then(|e| e.attrs.state(&name.local).and_then(natural));
            if old != value.and_then(natural) {
                if let Some(mut data) = node.try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                    data.damage.insert(ALL_DAMAGE);
                }
                node.mark_damaged();
            }
            // pyro: the directionality of a `dir=auto` element (its `dir=` token) is its
            // descendants' too (`:dir()`, as a `dir` change, `restyle_for_attribute_change`).
            let direction = |v: &str| v.split_ascii_whitespace().find(|t| t.starts_with("dir=")).map(str::to_owned);
            let old = node
                .element_data()
                .and_then(|e| e.attrs.iter().find(|a| a.name == *name).and_then(|a| direction(&a.value)));
            if old != value.and_then(direction) {
                if let Some(mut data) = node.try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                    data.hint |= RestyleHint::RESTYLE_DESCENDANTS;
                }
                node.mark_ancestors_dirty();
            }
            // pyro: the `view-transition-contain` token (the scope of an element-scoped view
            // transition until its update callback is done): the element keeps the border-box size
            // of its last layout, from before the token came, until the token goes
            // (`LayoutData::view_transition_size`).
            let contain = |v: &str| v.split_ascii_whitespace().any(|t| t == "view-transition-contain");
            let was_contained = self.doc.nodes[node_id]
                .element_data()
                .and_then(|e| e.attrs.state(&name.local))
                .is_some_and(contain);
            let contained = value.is_some_and(contain);
            if was_contained != contained {
                // (Physical and unrounded: a fractional size stays what script saw.)
                let size = contained.then(|| self.doc.physical_unrounded_layout(node_id).size);
                let node = &mut self.doc.nodes[node_id];
                node.layout_data_mut().view_transition_size = size;
                if let Some(mut data) = node.try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                    data.damage.insert(ALL_DAMAGE);
                }
                node.mark_damaged();
            }
        }
        let Some(parent_id) = self.doc.nodes[node_id].parent else {
            return;
        };
        let parent = &self.doc.nodes[parent_id];
        let nth_of = parent.is_element()
            && parent
                .selector_flags()
                .get()
                .contains(ElementSelectorFlags::HAS_SLOW_SELECTOR_NTH_OF);
        if !nth_of {
            return;
        }
        for &id in parent.children.clone().iter() {
            let sibling = &mut self.doc.nodes[id];
            if id != node_id && sibling.is_element() && sibling.primary_styles().is_some() {
                sibling.set_restyle_hint(RestyleHint::restyle_subtree());
            }
        }
    }

    fn restyle_for_attribute_change(&mut self, node_id: NodeId, attr: &LocalName) {
        let node = &mut self.doc.nodes[node_id];
        let is_style = *attr == local_name!("style");
        // Attributes that box construction or layout read directly. Any other attribute only
        // reaches layout through the cascade, so its damage comes from the computed-style diff.
        // Inline SVG is rendered from its serialized source, so every SVG attribute counts.
        let is_svg = node
            .element_data()
            .is_some_and(|element| element.name.ns == markup5ever::ns!(svg));
        let read_by_layout = is_svg
            || matches!(
                *attr,
                local_name!("type")
                    | local_name!("value")
                    | local_name!("checked")
                    | local_name!("width")
                    | local_name!("height")
                    | local_name!("colspan")
                    | local_name!("rowspan")
                    | local_name!("rows")
                    | local_name!("cols")
                    | local_name!("start")
                    | local_name!("reversed")
                    | local_name!("src")
                    | local_name!("srcdoc")
                    | local_name!("span")
            );
        // A table's `cellpadding` is a presentational hint on its cells, not on the table.
        let restyles_cells = *attr == local_name!("cellpadding")
            && node.data.is_element_with_tag_name(&local_name!("table"));
        // pyro: `dir` decides the directionality of the element's descendants too (`:dir()`).
        let restyles_cells = restyles_cells || *attr == local_name!("dir");
        if let Some(mut data) = node.try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
            data.hint |= if is_style {
                RestyleHint::RESTYLE_STYLE_ATTRIBUTE
            } else {
                RestyleHint::RESTYLE_SELF
            };
            if restyles_cells {
                data.hint |= RestyleHint::RESTYLE_DESCENDANTS;
            }
            if read_by_layout {
                data.damage.insert(ALL_DAMAGE);
            }
        }
        if read_by_layout {
            node.mark_damaged();
        }
        // Mark ancestors dirty so the style traversal visits this subtree.
        node.mark_ancestors_dirty();
    }

    /// Restyle an in-document element whose child list changed, `changed` being the position
    /// of the inserted children (empty, at the former position of a removed child;
    /// `removed_element` says whether that child was an element). New children are styled by
    /// the traversal regardless; the element and its other children only need restyling where
    /// its selector flags record a selector that depends on them.
    ///
    /// pyro: a change of text (or other non-element) children alone moves no
    /// element's index or edge status, so the siblings are left alone (`:empty` still restyles
    /// the parent). Inserted children that already have a style were moved here from this same
    /// parent (`add_children_to_parent` skips their subtree restyle, their ancestors being
    /// unchanged): they are restyled whenever the parent carries a sibling-dependent flag. The
    /// same conditions inside a `:has()` argument reach their anchors through
    /// `invalidate_relative_selectors_for_ts_state`.
    /// pyro: an `<mo>`'s spacing follows its text and its position in its row
    /// (`mathml_operator`): `node_id`'s children or text changed, so its operators, or it when it is
    /// one, build their presentational hints again.
    fn respace_operators(&mut self, node_id: NodeId) {
        if !crate::mathml_operator::affects_spacing(&self.doc.nodes[node_id]) {
            return;
        }
        let node = &self.doc.nodes[node_id];
        let is_mo = |n: &crate::node::Node| {
            n.element_data().is_some_and(|e| e.name.ns == markup5ever::ns!(mathml) && e.name.local == local_name!("mo"))
        };
        let operators: Vec<NodeId> = if is_mo(node) {
            vec![node_id]
        } else {
            node.children.iter().copied().filter(|c| is_mo(&self.doc.nodes[*c])).collect()
        };
        for id in operators {
            let node = &mut self.doc.nodes[id];
            if let Some(element) = node.element_data_mut() {
                element.attrs.forget_presentational_hints();
            }
            node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
        }
    }

    /// pyro: a text child became empty or stopped being empty. Empty text leaves an element
    /// `:empty` (`stylo.rs` `is_empty`), so the parent restyles where a `:empty` selector looked at
    /// it, and so do the `:has()` anchors whose argument tests its `:empty`.
    fn restyle_for_text_emptiness_change(&mut self, parent_id: NodeId) {
        let parent = &mut self.doc.nodes[parent_id];
        if !parent.selector_flags().get().contains(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
            return;
        }
        if let Some(mut data) = parent.try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
            data.hint |= RestyleHint::RESTYLE_SELF;
        }
        parent.set_dirty_descendants();
        parent.mark_ancestors_dirty();
        self.doc.invalidate_relative_selectors_for_ts_state(parent_id, TSStateForInvalidation::EMPTY);
    }

    fn restyle_for_child_list_change(
        &mut self,
        parent_id: NodeId,
        changed: Range<usize>,
        removed_element: bool,
    ) {
        self.respace_operators(parent_id);
        // pyro: the scroll markers of the groups around a changed child (`layout::scroll_markers`).
        self.doc.scroll_markers_dirty = true;
        let parent = &self.doc.nodes[parent_id];
        let flags = parent.selector_flags().get();
        let mut hint = RestyleHint::empty();
        // pyro: a `:has()` anchor among the ancestors is restyled by Stylo's relative
        // selector invalidator when an element it can match comes or goes
        // (`invalidate_relative_selectors_for_insertion`), not here; an argument's `:empty`
        // matches by the child list alone, so the parent is looked up for one.
        if flags.contains(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
            hint |= RestyleHint::RESTYLE_SELF;
            self.doc
                .invalidate_relative_selectors_for_ts_state(parent_id, TSStateForInvalidation::EMPTY);
        }
        let is_element = |id: &NodeId| self.doc.nodes[*id].is_element();
        let element_changed = removed_element || parent.children[changed.clone()].iter().any(is_element);
        // pyro: an element that came or went can change counters and quotes (`layout::counters`);
        // text alone cannot (a text probe measured in a loop no longer walks the whole document).
        if element_changed {
            self.doc.counters_dirty = true;
        }
        // A change with no earlier element sibling restyles all children with one hint rather
        // than hinting each later sibling, so inserting or removing at the front stays linear.
        let front_change = flags.contains(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
            && !parent.children[..changed.start].iter().any(is_element);
        if !element_changed {
            // No element came or went: no index or edge status moved.
        } else if front_change
            || flags.intersects(
                ElementSelectorFlags::HAS_SLOW_SELECTOR
                    | ElementSelectorFlags::HAS_SLOW_SELECTOR_NTH_OF
                    | ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION,
            )
        {
            hint |= RestyleHint::RESTYLE_DESCENDANTS;
            // pyro: every other child's position may have changed, and with it an
            // argument's `:nth-child()`, `:first-child` or `:last-child`
            // (`invalidate_relative_selectors_for_ts_state`); the inserted children are
            // covered by their own insertion.
            let positional = TSStateForInvalidation::NTH
                | TSStateForInvalidation::NTH_EDGE_FIRST
                | TSStateForInvalidation::NTH_EDGE_LAST;
            for id in parent.children[..changed.start]
                .iter()
                .chain(&parent.children[changed.end..])
            {
                self.doc.invalidate_relative_selectors_for_ts_state(*id, positional);
            }
        } else {
            let children = mem::take(&mut self.doc.nodes[parent_id].children);
            let is_element = |id: &NodeId| self.doc.nodes[*id].is_element();
            // The moved-in children, whose index or edge status may have changed (new ones
            // have no style data for the hint and are styled regardless).
            let moved_in = if flags.intersects(
                ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS
                    | ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR,
            ) {
                changed.clone()
            } else {
                0..0
            };
            let later = if flags.contains(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                changed.end..children.len()
            } else {
                0..0
            };
            // The element children whose `:first-child`/`:last-child` status flipped: the one
            // after the change when no element precedes it (it was or now is first), the one
            // before when no element follows it (it was or now is last); text nodes don't
            // count. No other element's edge status moved.
            let mut edge = [None, None];
            if flags.contains(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR) {
                if !children[..changed.start].iter().any(is_element) {
                    edge[0] = children[changed.end..].iter().copied().find(is_element);
                }
                if !children[changed.end..].iter().any(is_element) {
                    edge[1] = children[..changed.start]
                        .iter()
                        .rev()
                        .copied()
                        .find(is_element);
                }
            }
            for id in children[moved_in]
                .iter()
                .chain(children[later.clone()].iter())
                .chain(edge.iter().flatten())
            {
                self.doc.nodes[*id].set_restyle_hint(RestyleHint::restyle_subtree());
            }
            self.doc.nodes[parent_id].children = children;
            // pyro: the arguments that match by position
            // (`invalidate_relative_selectors_for_ts_state`), once the child list is back for
            // the sibling walk: `:nth-child()` on the later siblings, `:first-child`/
            // `:last-child` on the children whose edge status flipped. The moved-in children
            // are covered by their own insertion.
            let children = &self.doc.nodes[parent_id].children;
            for id in &children[later] {
                self.doc
                    .invalidate_relative_selectors_for_ts_state(*id, TSStateForInvalidation::NTH);
            }
            let edge_states = [TSStateForInvalidation::NTH_EDGE_FIRST, TSStateForInvalidation::NTH_EDGE_LAST];
            for (id, state) in edge.iter().zip(edge_states) {
                if let Some(id) = id {
                    self.doc.invalidate_relative_selectors_for_ts_state(*id, state);
                }
            }
        }

        let parent = &mut self.doc.nodes[parent_id];
        if !hint.is_empty() {
            if let Some(mut data) = parent
                .try_stylo_element_data_mut()
                .and_then(|s| s.get_mut())
            {
                data.hint |= hint;
            }
        }
        // The traversal only descends into an element flagged as having dirty descendants.
        parent.set_dirty_descendants();
        parent.mark_ancestors_dirty();
    }

    // Tree mutation methods
    pub fn remove_node_if_unparented(&mut self, node_id: NodeId) {
        self.remove_node_if_unparented_with(node_id, &mut |_| {});
    }

    /// Like [`Self::remove_node_if_unparented`], but calls `on_drop` with the id of
    /// every dropped node (the node itself and all of its descendants).
    pub fn remove_node_if_unparented_with(
        &mut self,
        node_id: NodeId,
        on_drop: &mut dyn FnMut(NodeId),
    ) {
        if let Some(node) = self.doc.get_node(node_id) {
            if node.parent.is_none() {
                self.remove_and_drop_node_with(node_id, on_drop);
            }
        }
    }

    /// Remove all of the children from old_parent_id and append them to new_parent_id
    pub fn append_children(&mut self, parent_id: NodeId, child_ids: &[NodeId]) {
        self.add_children_to_parent(parent_id, child_ids, &|parent, child_ids| {
            let index = parent.children.len();
            parent.children.extend_from_slice(child_ids);
            index
        });
    }

    pub fn insert_nodes_before(&mut self, anchor_node_id: NodeId, new_node_ids: &[NodeId]) {
        let parent_id = self.doc.nodes[anchor_node_id].parent.unwrap();
        self.add_children_to_parent(parent_id, new_node_ids, &|parent, child_ids| {
            let node_child_idx = parent.index_of_child(anchor_node_id).unwrap();
            parent
                .children
                .splice(node_child_idx..node_child_idx, child_ids.iter().copied());
            node_child_idx
        });
    }

    fn add_children_to_parent(
        &mut self,
        parent_id: NodeId,
        child_ids: &[NodeId],
        insert_children_fn: &dyn Fn(&mut Node, &[NodeId]) -> usize,
    ) {
        let new_parent_is_in_document = self.doc.nodes[parent_id].flags.is_in_document();
        self.mutations_occurred |= new_parent_is_in_document && !child_ids.is_empty();

        // If one of the moved subtrees contains the focused node, the ancestors it is
        // detached from stop matching :focus-within. Record them while the parent links
        // are still intact; the state is transferred after the children are reattached.
        let mut focus_within_old_chain: Option<Vec<NodeId>> = None;

        // Detach the children from their old parents *before* inserting them into
        // the new parent (matching DOM `insertBefore` semantics). If a child is
        // being moved within the same parent then detaching it after insertion
        // would remove both the old and the newly-inserted entries from the
        // parent's child list, and anchor indices would be computed against a
        // child list that still contains the moved nodes.
        // pyro: children moved within this element parent keep their ancestors, so
        // only their siblings changed, and `restyle_for_child_list_change` restyles them for
        // that; a move under a non-element parent keeps the subtree restyle.
        let parent_is_element = self.doc.nodes[parent_id].is_element();
        let mut same_parent_moves: Vec<NodeId> = Vec::new();
        for child_id in child_ids.iter().copied() {
            if parent_is_element
                && self.doc.nodes[child_id].parent == Some(parent_id)
                && self.doc.nodes[child_id].flags.is_in_document()
            {
                same_parent_moves.push(child_id);
            }
            // pyro: a moved child leaves its old parent as
            // `remove_node` does (`:has()` anchors, `:empty`), checked while
            // it is still linked.
            if self.doc.nodes[child_id].parent.is_some() {
                self.doc.invalidate_relative_selectors_for_removal(child_id);
            }
            let child = &self.doc.nodes[child_id];
            if child.contains_focus() {
                focus_within_old_chain = Some(self.doc.maybe_node_element_ancestors(child.parent));
            }

            if self.doc.nodes[child_id].parent.is_some() {
                self.doc.unlist_hoisted_leaving(child_id);
            }
            let child = &mut self.doc.nodes[child_id];
            let child_was_in_doc = child.flags.is_in_document();
            self.mutations_occurred |= child_was_in_doc;
            let Some(old_parent_id) = child.parent.take() else {
                continue;
            };

            let old_parent = &mut self.doc.nodes[old_parent_id];
            old_parent.insert_damage(CHILDREN_CHANGED);
            let index = old_parent.index_of_child(child_id).unwrap_or(0);
            old_parent.children.retain(|id| *id != child_id);

            if child_was_in_doc {
                let removed_element = self.doc.nodes[child_id].is_element();
                self.restyle_for_child_list_change(old_parent_id, index..index, removed_element);
                devtools!(
                    self,
                    MutationRecord::NodeRemoved(NodeRemoved {
                        parent_id: old_parent_id,
                        node_id: child_id,
                    })
                );
            }

            self.maybe_record_node(old_parent_id);
        }

        let new_parent = &mut self.doc.nodes[parent_id];
        new_parent.insert_damage(CHILDREN_CHANGED);
        let index = insert_children_fn(new_parent, child_ids);

        if new_parent_is_in_document {
            self.restyle_for_child_list_change(parent_id, index..index + child_ids.len(), false);
        }

        // pyro: whether the children land under a `display: none` element (see below).
        // Any child may hold styled elements: a new element (no style data) may hold a moved
        // one (pyro-dom builds a new wrapper by moving children into it before inserting it).
        let under_display_none = self.is_under_display_none(parent_id);
        let mut moved_document_sheets = Vec::new();
        for child_id in child_ids.iter().copied() {
            let child = &mut self.doc.nodes[child_id];
            let child_was_in_doc = child.flags.is_in_document();
            child.parent = Some(parent_id);
            let child = &mut self.doc.nodes[child_id];
            // A moved or re-attached subtree keeps its styles, but everything above it changed.
            // pyro: a move within the same parent changed no ancestor (see
            // `same_parent_moves`).
            if new_parent_is_in_document
                && child.primary_styles().is_some()
                && !same_parent_moves.contains(&child_id)
            {
                child.set_restyle_hint(RestyleHint::restyle_subtree());
            }

            if new_parent_is_in_document {
                devtools!(
                    self,
                    MutationRecord::NodeInserted(NodeInserted {
                        parent_id,
                        node_id: child_id,
                        previous_sibling_id: self.devtools_previous_sibling(parent_id, child_id),
                    })
                );
            }

            // pyro: Stylo keeps no style data under a `display: none` element (it drops
            // it when an element becomes one, `clear_descendant_data`, and never visits the
            // children of one), but a moved element kept its own: styles, inline layout and the
            // flags built from them, which no pass would replace. Under a `display: none` element
            // it has none, as a fresh build has, and is styled and built from scratch if shown.
            if under_display_none {
                self.doc.iter_subtree_mut(child_id, |id, doc| {
                    doc.nodes[id].flags.reset_construction_flags();
                    // Nor pseudo-elements, built from its styles: built again if it is shown.
                    for idx in 0..2 {
                        if let Some(pe) = doc.nodes[id].pe_by_index(idx) {
                            doc.nodes[id].set_pe_by_index(idx, None);
                            doc.remove_and_drop_pe(pe);
                        }
                    }
                    crate::layout::first_letter::drop_style_node(doc, id);
                    let node = &mut doc.nodes[id];
                    if let Some(element) = node.element_data_mut() {
                        element.take_inline_layout();
                        // SAFETY: no style traversal is running while the DOM is mutated.
                        unsafe { element.stylo_element_data.clear() };
                    }
                });
                // Nor a layout tree (its old container takes it out of its own list).
                self.doc.leave_layout_tree(child_id);
            }

            if new_parent_is_in_document && !child_was_in_doc {
                self.process_added_subtree(child_id);
            } else if !new_parent_is_in_document && child_was_in_doc {
                self.process_removed_subtree(child_id);
            } else if new_parent_is_in_document {
                // pyro: a moved `<style>` with `@scope` rules has a new implicit scope root (its new
                // parent: `BaseDocument::process_style_element`): its sheet is parsed again.
                let mut moved_styles = Vec::new();
                self.doc.iter_subtree_mut(child_id, |id, doc| {
                    let node = &doc.nodes[id];
                    if doc.nodes_to_stylesheet.contains_key(&id) && node.containing_shadow_host_id().is_none() {
                        moved_document_sheets.push(id);
                    }
                    if node.data.is_element_with_tag_name(&local_name!("style"))
                        && crate::document::sheet_has_scope_rules(&node.text_content())
                    {
                        moved_styles.push(id);
                    }
                });
                self.style_nodes.extend(moved_styles);
            }
        }
        self.doc.reposition_document_stylesheets(moved_document_sheets);
        // pyro: the `:has()` anchors that can see the new positions,
        // once every child is linked (a sibling inserted with it counts).
        if new_parent_is_in_document {
            for &child_id in child_ids {
                self.doc.invalidate_relative_selectors_for_insertion(child_id);
            }
        }

        // Transfer :focus-within from the old ancestor chain to the new one (the
        // shared prefix stays). If the subtree left the document, the focus was
        // cleared with it and only the old chain needs the state removed.
        if let Some(old_chain) = focus_within_old_chain {
            let new_chain = match self.doc.focus_node_id {
                Some(_) => self.doc.node_element_ancestors(parent_id),
                None => Vec::new(),
            };
            let same_count = old_chain
                .iter()
                .zip(&new_chain)
                .take_while(|(o, n)| o == n)
                .count();
            for &id in old_chain.iter().skip(same_count) {
                self.doc
                    .snapshot_node_and(id, ElementState::FOCUS_WITHIN, |node| {
                        node.remove_focus_within()
                    });
            }
            for &id in new_chain.iter().skip(same_count) {
                self.doc
                    .snapshot_node_and(id, ElementState::FOCUS_WITHIN, |node| {
                        node.add_focus_within()
                    });
            }
        }

        self.maybe_record_node(parent_id);
    }

    /// pyro: whether `node_id` is or is inside a `display: none` element: the nearest
    /// styled element from it up says so (the elements below one have no styles; an element not
    /// styled yet below a shown one will be).
    fn is_under_display_none(&self, node_id: NodeId) -> bool {
        let mut at = Some(node_id);
        while let Some(id) = at {
            let node = &self.doc.nodes[id];
            if node.is_element() {
                if let Some(style) = node.primary_styles() {
                    return style.get_box().display.is_none();
                }
            }
            at = node.parent;
        }
        false
    }

    /// The nearest preceding sibling of `node_id` in `parent_id`'s children
    /// that devtools show (not a whitespace-only text node or anonymous block)
    #[cfg(feature = "devtools")]
    fn devtools_previous_sibling(&self, parent_id: NodeId, node_id: NodeId) -> Option<NodeId> {
        let parent = &self.doc.nodes[parent_id];
        let index = parent.index_of_child(node_id)?;
        parent.children[..index].iter().rev().copied().find(|id| {
            let node = &self.doc.nodes[*id];
            !node.is_whitespace_node() && !node.is_anonymous()
        })
    }

    // Tree mutation methods (that defer to other methods)
    pub fn insert_nodes_after(&mut self, anchor_node_id: NodeId, new_node_ids: &[NodeId]) {
        match self.next_sibling_id(anchor_node_id) {
            Some(id) => self.insert_nodes_before(id, new_node_ids),
            None => {
                let parent_id = self.parent_id(anchor_node_id).unwrap();
                self.append_children(parent_id, new_node_ids)
            }
        }
    }

    pub fn reparent_children(&mut self, old_parent_id: NodeId, new_parent_id: NodeId) {
        let child_ids = std::mem::take(&mut self.doc.nodes[old_parent_id].children);
        self.maybe_record_node(old_parent_id);
        self.append_children(new_parent_id, &child_ids);
    }

    pub fn replace_node_with(&mut self, anchor_node_id: NodeId, new_node_ids: &[NodeId]) {
        self.insert_nodes_before(anchor_node_id, new_node_ids);
        self.remove_node(anchor_node_id);
    }

    // === ParentNode / ChildNode mixin mutation methods ===
    //
    // These implement the DOM spec's mutation semantics for the ParentNode and
    // ChildNode mixins (`append`/`prepend`/`replaceChildren` and
    // `before`/`after`/`replaceWith`). Callers pass lists of node ids with
    // string arguments already converted to (detached) text nodes and
    // DocumentFragment arguments already expanded into their children (per the
    // DOM spec's "insert a node" steps).

    /// Detach (rather than drop) any already-parented nodes, so that references
    /// to them (and their descendants) remain valid
    fn detach_all(&mut self, node_ids: &[NodeId]) {
        for node_id in node_ids {
            if self.node_has_parent(*node_id) {
                self.remove_node(*node_id);
            }
        }
    }

    /// The nearest sibling of `anchor_id` (in the direction given by `offset`:
    /// +1 = following, -1 = preceding) which is not in `excluded` — the spec's
    /// "viable next/previous sibling" for ChildNode mutation methods
    fn viable_sibling(
        &self,
        anchor_id: NodeId,
        offset: isize,
        excluded: &[NodeId],
    ) -> Option<NodeId> {
        let node = self.doc.get_node(anchor_id)?;
        let parent = self.doc.get_node(node.parent?)?;
        let mut index = parent.index_of_child(anchor_id)?;
        loop {
            index = index.checked_add_signed(offset)?;
            let sibling_id = *parent.children.get(index)?;
            if !excluded.contains(&sibling_id) {
                return Some(sibling_id);
            }
        }
    }

    /// ParentNode's `prepend()`: insert the nodes at the start of `parent_id`'s
    /// children (ParentNode's `append()` is [`append_children`](Self::append_children))
    pub fn prepend_nodes(&mut self, parent_id: NodeId, node_ids: &[NodeId]) {
        self.detach_all(node_ids);
        match self.child_ids(parent_id).first().copied() {
            Some(first_child_id) => self.insert_nodes_before(first_child_id, node_ids),
            None => self.append_children(parent_id, node_ids),
        }
    }

    /// ParentNode's `replaceChildren()`: replace all of `parent_id`'s children
    /// with the given nodes. The existing children are detached rather than
    /// dropped, so that references to them remain valid.
    pub fn replace_children(&mut self, parent_id: NodeId, node_ids: &[NodeId]) {
        self.detach_all(node_ids);
        for child_id in self.child_ids(parent_id) {
            self.remove_node(child_id);
        }
        self.append_children(parent_id, node_ids);
    }

    /// ChildNode's `before()`: insert the nodes before `anchor_id`. The
    /// insertion point is after the nearest preceding sibling which isn't
    /// itself being inserted (or at the start of the parent if there is none).
    /// Does nothing if the anchor has no parent.
    pub fn before_node(&mut self, anchor_id: NodeId, node_ids: &[NodeId]) {
        let Some(parent_id) = self.parent_id(anchor_id) else {
            return;
        };
        let viable_prev = self.viable_sibling(anchor_id, -1, node_ids);
        self.detach_all(node_ids);
        match viable_prev {
            Some(prev_id) => self.insert_nodes_after(prev_id, node_ids),
            None => match self.child_ids(parent_id).first().copied() {
                Some(first_child_id) => self.insert_nodes_before(first_child_id, node_ids),
                None => self.append_children(parent_id, node_ids),
            },
        }
    }

    /// ChildNode's `after()`: insert the nodes after `anchor_id`. The insertion
    /// point is before the nearest following sibling which isn't itself being
    /// inserted (or at the end of the parent if there is none). Does nothing if
    /// the anchor has no parent.
    pub fn after_node(&mut self, anchor_id: NodeId, node_ids: &[NodeId]) {
        let Some(parent_id) = self.parent_id(anchor_id) else {
            return;
        };
        let viable_next = self.viable_sibling(anchor_id, 1, node_ids);
        self.detach_all(node_ids);
        match viable_next {
            Some(next_id) => self.insert_nodes_before(next_id, node_ids),
            None => self.append_children(parent_id, node_ids),
        }
    }

    /// ChildNode's `replaceWith()`: replace `anchor_id` with the nodes in its
    /// parent's child list. If the anchor is itself one of the inserted nodes,
    /// they are inserted at its old position (the spec's "viable next sibling"
    /// handling). The anchor is detached rather than dropped, so references to
    /// it remain valid. Does nothing if the anchor has no parent.
    pub fn replace_with_nodes(&mut self, anchor_id: NodeId, node_ids: &[NodeId]) {
        let Some(parent_id) = self.parent_id(anchor_id) else {
            return;
        };
        let viable_next = self.viable_sibling(anchor_id, 1, node_ids);
        self.detach_all(node_ids);
        if self.node_has_parent(anchor_id) {
            self.replace_node_with(anchor_id, node_ids);
        } else {
            match viable_next {
                Some(next_id) => self.insert_nodes_before(next_id, node_ids),
                None => self.append_children(parent_id, node_ids),
            }
        }
    }
}

impl<'doc> DocumentMutator<'doc> {
    pub fn flush(&mut self) {
        if self.recompute_is_animating {
            self.doc.has_canvas = self.doc.compute_has_canvas();
        }

        if let Some(id) = self.title_node {
            let title = self.doc.nodes[id].text_content();
            self.doc.shell_provider.set_window_title(title);
        }

        // Add/Update inline stylesheets (<style> elements)
        for id in self.style_nodes.drain() {
            self.doc.process_style_element(id);
        }
        for id in self.link_media_nodes.drain() {
            self.doc.refresh_link_media(id);
        }

        for id in self.form_nodes.drain() {
            self.doc.reset_form_owner(id);
        }

        #[cfg(feature = "autofocus")]
        if let Some(node_id) = self.node_to_autofocus.take() {
            if self.doc.get_node(node_id).is_some() {
                self.doc.set_focus_to(node_id);
            }
        }

        #[cfg(feature = "devtools")]
        if !self.devtools_records.is_empty() {
            if let Some(provider) = &self.doc.devtools_provider {
                provider.mutations(&self.devtools_records);
            }
            self.devtools_records.clear();
        }
    }

    pub fn set_inner_html(&mut self, node_id: NodeId, html: &str) {
        self.remove_and_drop_all_children(node_id);
        self.doc
            .html_parser_provider
            .clone()
            .parse_inner_html(self, node_id, html);
    }

    fn flush_eager_ops(&mut self) {
        let mut ops = mem::take(&mut self.eager_op_queue);
        let mut images = HashSet::new();
        for op in ops.drain(0..) {
            match op {
                SpecialOp::LoadImage(node_id) => {
                    if images.insert(node_id) && self.doc.get_node(node_id).is_some_and(|node| node.flags.is_in_document()) {
                        self.load_image(node_id);
                    }
                }
                SpecialOp::LoadIframe(node_id) => self.load_iframe(node_id),
                SpecialOp::LoadStylesheet(node_id) => self.load_linked_stylesheet(node_id),
                SpecialOp::UnloadStylesheet(node_id) => self.unload_stylesheet(node_id),
                SpecialOp::LoadCustomPaintSource(node_id) => self.load_custom_paint_src(node_id),
                SpecialOp::ProcessButtonInput(node_id) => self.process_button_input(node_id),
                SpecialOp::UnloadSubDocument(node_id) => self.remove_sub_document(node_id),
                #[cfg(feature = "custom-widget")]
                SpecialOp::UnloadCustomWidget(node_id) => self.remove_custom_widget(node_id),
            }
        }

        // Queue is empty, but put Vec back anyway so allocation can be reused.
        self.eager_op_queue = ops;
    }

    fn process_added_subtree(&mut self, node_id: NodeId) {
        self.doc.iter_subtree_mut(node_id, |node_id, doc| {
            let node = &mut doc.nodes[node_id];
            node.flags.set(NodeFlags::IS_IN_DOCUMENT, true);
            node.insert_damage(ALL_DAMAGE);

            // If the node has an "id" attribute, store it in the ID map.
            if let Some(id_attr) = node.attr(local_name!("id")).map(ToString::to_string) {
                doc.add_to_id_map(&id_attr, node_id);
            }

            let node = &mut doc.nodes[node_id];
            let NodeData::Element(ref mut element) = node.data else {
                return;
            };

            // Custom widgets replace the element's built-in tag-specific behaviour
            #[cfg(feature = "custom-widget")]
            let has_custom_widget = element.custom_widget_data().is_some();
            #[cfg(not(feature = "custom-widget"))]
            let has_custom_widget = false;

            // Custom post-processing by element tag name
            let tag = element.name.local.as_ref();
            match tag {
                "title" => self.title_node = Some(node_id),
                "style" => {
                    self.style_nodes.insert(node_id);
                }
                _ if has_custom_widget => {}
                "link" => self.eager_op_queue.push(SpecialOp::LoadStylesheet(node_id)),
                // pyro: candidates for `<meta name=color-scheme>` (`update_page_color_scheme`).
                "meta" => {
                    if !doc.meta_nodes.contains(&node_id) {
                        doc.meta_nodes.push(node_id);
                    }
                }
                "img" => self.eager_op_queue.push(SpecialOp::LoadImage(node_id)),
                // pyro: a `<video>` paints its poster image (no
                // video frame is ever decoded).
                "video" => self.eager_op_queue.push(SpecialOp::LoadImage(node_id)),
                // pyro: an image button is replaced content showing
                // its `src` image, so it loads like `<img>` (and is a form
                // control like any other input).
                "input" if crate::layout::replaced::is_image_input(element) => {
                    self.eager_op_queue.push(SpecialOp::LoadImage(node_id));
                    self.eager_op_queue
                        .push(SpecialOp::ProcessButtonInput(node_id));
                    self.form_nodes.insert(node_id);
                }
                // pyro: an `<embed>` shows the image its `src` names, as an `<img>` does, or the frame
                // document the render snapshot gives it (`srcdoc`) as an iframe does (plugins are
                // not done).
                "embed" if element.attr(local_name!("srcdoc")).is_some() || element.reserved_attr(crate::FRAME_ATTRIBUTE).is_some() => {
                    self.eager_op_queue.push(SpecialOp::LoadIframe(node_id))
                }
                "embed" => self.eager_op_queue.push(SpecialOp::LoadImage(node_id)),
                "iframe" => self.eager_op_queue.push(SpecialOp::LoadIframe(node_id)),
                // pyro: an `<object>` loads the image its `data` names (`load_image`), and is a
                // form-associated element like those below.
                "object" => {
                    // pyro: an `<object>` the render snapshot gives a frame document (`srcdoc`)
                    // shows it as an iframe does.
                    if element.attr(local_name!("srcdoc")).is_some() || element.reserved_attr(crate::FRAME_ATTRIBUTE).is_some() {
                        self.eager_op_queue.push(SpecialOp::LoadIframe(node_id));
                    } else {
                        self.eager_op_queue.push(SpecialOp::LoadImage(node_id));
                    }
                    self.eager_op_queue
                        .push(SpecialOp::ProcessButtonInput(node_id));
                    self.form_nodes.insert(node_id);
                }
                "canvas" => self
                    .eager_op_queue
                    .push(SpecialOp::LoadCustomPaintSource(node_id)),
                "button" | "fieldset" | "input" | "select" | "textarea" | "output" => {
                    self.eager_op_queue
                        .push(SpecialOp::ProcessButtonInput(node_id));
                    self.form_nodes.insert(node_id);
                }
                _ => {}
            }

            #[cfg(feature = "autofocus")]
            if node.is_focussable() {
                if let NodeData::Element(ref element) = node.data {
                    // `autofocus` is a boolean attribute, so presence alone enables it
                    // (`<input autofocus>`). "false" is still ignored, because Dioxus
                    // writes `autofocus: false` as the string "false".
                    if let Some(value) = element.attr(local_name!("autofocus")) {
                        if value != "false" {
                            self.node_to_autofocus = Some(node_id);
                        }
                    }
                }
            }
        });

        self.flush_eager_ops();
    }

    fn process_removed_subtree(&mut self, node_id: NodeId) {
        self.doc.iter_subtree_mut(node_id, |node_id, doc| {
            doc.nodes[node_id]
                .flags
                .set(NodeFlags::IS_IN_DOCUMENT, false);

            // Clear any interaction state that references this node, running
            // the usual teardown steps (unhover/unactive the surviving
            // ancestor chain, IME disable on blur of a focused input).
            doc.clear_interaction_state_for_removed_node(node_id);

            let node = &mut doc.nodes[node_id];

            // Clear the text selection if one of its endpoints references this node.
            // This prevents stale selection endpoint references.
            if doc.text_selection.anchor.node_or_parent == Some(node_id)
                || doc.text_selection.focus.node_or_parent == Some(node_id)
            {
                doc.text_selection.clear();
            }

            // Remove any snapshot for this node to prevent stale snapshot references
            // during style invalidation.
            if node.has_snapshot() {
                let opaque_id = style::dom::TNode::opaque(&&*node);
                doc.snapshots.remove(&opaque_id);
                node.set_has_snapshot(false);
            }

            // If the node has an "id" attribute remove it from the ID map.
            if let Some(id_attr) = node.attr(local_name!("id")).map(ToString::to_string) {
                doc.remove_from_id_map(&id_attr, node_id);
            }

            let node = &mut doc.nodes[node_id];
            let NodeData::Element(ref mut element) = node.data else {
                return;
            };

            match &element.special_data {
                SpecialElementData::SubDocument(_) => {
                    self.eager_op_queue
                        .push(SpecialOp::UnloadSubDocument(node_id));
                }
                #[cfg(feature = "custom-widget")]
                SpecialElementData::CustomWidget(_) => {
                    self.eager_op_queue
                        .push(SpecialOp::UnloadCustomWidget(node_id));
                }
                SpecialElementData::Stylesheet(_) => self
                    .eager_op_queue
                    .push(SpecialOp::UnloadStylesheet(node_id)),
                SpecialElementData::Image(_) => {}
                SpecialElementData::Canvas(_) => {
                    self.recompute_is_animating = true;
                }
                SpecialElementData::TableRoot(_) => {}
                SpecialElementData::TextInput(_) => {}
                SpecialElementData::CheckboxInput(_) => {}
                #[cfg(feature = "file-input")]
                SpecialElementData::FileInput(_) => {}
                SpecialElementData::None => {}
            }
        });

        self.flush_eager_ops();
    }

    fn maybe_record_node(&mut self, node_id: impl Into<Option<NodeId>>) {
        let Some(node_id) = node_id.into() else {
            return;
        };

        let Some(tag_name) = self.doc.nodes[node_id]
            .data
            .downcast_element()
            .map(|elem| &elem.name.local)
        else {
            return;
        };

        match tag_name.as_ref() {
            "title" => self.title_node = Some(node_id),
            "style" => {
                self.style_nodes.insert(node_id);
            }
            _ => {}
        }
    }

    fn load_linked_stylesheet(&mut self, target_id: NodeId) {
        let node = &self.doc.nodes[target_id];

        let mut is_in_head = false;
        let mut parent_id = node.parent;
        while let Some(id) = parent_id
            && !is_in_head
        {
            let parent = &self.doc.nodes[id];
            is_in_head |= parent.data.is_element_with_tag_name(&local_name!("head"));
            parent_id = parent.parent;
        }

        let rel_attr = node.attr(local_name!("rel"));
        let href_attr = node.attr(local_name!("href"));

        // pyro: a link that is no stylesheet link any more (its `rel` lost the `stylesheet` token,
        // or it has no `href`) stops styling: its sheet goes (HTML: the link no longer creates an
        // external resource link; WPT html/semantics/document-metadata `link-rel-attribute`).
        let (Some(rels), Some(href)) = (rel_attr, href_attr) else {
            self.unload_stylesheet(target_id);
            return;
        };
        if !rels
            .split_ascii_whitespace()
            .any(|rel| rel.eq_ignore_ascii_case("stylesheet"))
        {
            self.unload_stylesheet(target_id);
            return;
        }

        let Some(url) = self.doc.url.resolve_relative(href) else {
            return;
        };
        let handler = ResourceHandler::new(
            self.doc.tx.clone(),
            self.doc.id(),
            Some(node.id),
            self.doc.shell_provider.clone(),
            StylesheetHandler {
                source_url: url.clone(),
                guard: self.doc.guard.clone(),
                net_provider: self.doc.net_provider.clone(),
                abort_signal: self.doc.abort_signal.clone(),
                client: self.doc.request_client(),
                // pyro: the response's policy as the snapshot wrote it on the link (an engine
                // attribute page content cannot write).
                policy: self.doc.engine_attr(target_id, crate::document::SHEET_POLICY_ATTRIBUTE).map(str::to_owned),
                policies: self.doc.sheet_policies.clone(),
                quirks_mode: self.doc.sheet_quirks_mode(),
            },
        );

        if is_in_head && !self.doc.net_provider.is_noop() {
            self.doc
                .pending_critical_resources
                .insert(handler.request_id());
        }

        // pyro: a style sheet's request (its destination says so, never its URL).
        let mut request = self.doc.build_request(url);
        request.options.destination = blitz_traits::net::Destination::Style;
        // pyro: the snapshot's sheet of this link (its engine attribute), served by identity.
        request.options.sheet = self.doc.engine_attr(target_id, crate::document::SHEET_ATTRIBUTE).map(str::to_owned);
        self.doc.net_provider.fetch(self.doc.id(), request, Box::new(handler));
    }

    fn unload_stylesheet(&mut self, node_id: NodeId) {
        let node = &mut self.doc.nodes[node_id];
        let Some(element) = node.element_data_mut() else {
            unreachable!();
        };
        // An icon link never holds a stylesheet, and a stylesheet link holds none
        // until its fetch has completed.
        let stylesheet = match element.special_data.take() {
            SpecialElementData::Stylesheet(stylesheet) => stylesheet,
            other => {
                element.special_data = other;
                return;
            }
        };

        // pyro: a shadow tree's sheet leaves its shadow root (`crate::shadow`), which restyles the
        // shadow tree when it is next flushed.
        if self.doc.remove_shadow_stylesheet(node_id) {
            return;
        }
        let guard = self.doc.guard.read();
        self.doc.stylist.remove_stylesheet(stylesheet, &guard);
        self.doc
            .stylist
            .force_stylesheet_origins_dirty(OriginSet::all());

        self.doc.nodes_to_stylesheet.remove(&node_id);
        self.doc.linked_page_css.remove(&node_id);
        self.doc.stylesheet_generation += 1;
    }

    fn load_image(&mut self, target_id: NodeId) {
        if let Some(previous) = self.doc.nodes[target_id].element_data_mut().and_then(|element| element.source_image_request.take()) {
            let (url, options) = *previous;
            if let Some(waiters) = self.doc.pending_images.get_mut(&options.cache_key(&url)) {
                waiters.retain(|(id, kind)| *id != target_id || !matches!(kind, ImageType::Image));
            }
        }
        let node = &self.doc.nodes[target_id];
        // pyro: a `<video>`'s image is its poster; without one it
        // paints nothing.
        let is_video = node
            .element_data()
            .is_some_and(|e| e.name.local == local_name!("video"));
        // pyro: an `<object>` shows the image its `data` names, when its `type` (or the URL's
        // extension) says an image (`replaced::object_loads_image`); otherwise, and until the image
        // has loaded, its children (the fallback) render.
        let object = node.element_data().filter(|e| e.name.local == local_name!("object"));
        let attr = if is_video {
            local_name!("poster")
        } else if object.is_some() {
            local_name!("data")
        } else {
            local_name!("src")
        };
        let object_without_image = object.is_some_and(|e| {
            !crate::layout::replaced::object_loads_image(e)
                || e.attr(local_name!("data")).is_none_or(|data| data.trim().is_empty())
        });
        let object = object.is_some();
        if object_without_image {
            let node = &mut self.doc.nodes[target_id];
            let had_image = match node.element_data_mut() {
                Some(element) if matches!(element.special_data, SpecialElementData::Image(_)) => {
                    element.special_data = SpecialElementData::None;
                    true
                }
                _ => false,
            };
            if had_image {
                node.clear_layout_cache();
                // The fallback builds its boxes again.
                self.doc.iter_subtree_mut(target_id, |id: NodeId, doc: &mut BaseDocument| {
                    doc.nodes[id].insert_damage(ALL_DAMAGE);
                });
            }
            return;
        }
        if is_video && node.attr(attr.clone()).is_none_or(|v| v.trim().is_empty()) {
            let node = &mut self.doc.nodes[target_id];
            if let Some(element) = node.element_data_mut() {
                if matches!(element.special_data, SpecialElementData::Image(_)) {
                    element.special_data = SpecialElementData::None;
                    node.clear_layout_cache();
                    node.insert_damage(ALL_DAMAGE);
                }
            }
            return;
        }
        // pyro: an `<img>` showing its alt text (`ElementData::image_broken`) is an image again
        // while it loads a new source; one with no usable source shows its alt text.
        let is_img = node.element_data().is_some_and(|e| e.name.local == local_name!("img"));
        let usable = node.attr(attr.clone()).filter(|src| !src.is_empty()).is_some_and(|src| self.doc.resolve_url(src).is_some());
        if is_img && node.element_data().is_some_and(|e| e.image_broken) && usable {
            let alt_children = std::mem::take(&mut self.doc.nodes[target_id].children);
            for child in alt_children {
                self.doc.remove_node_from_tree(child);
            }
            if let Some(element) = self.doc.nodes[target_id].element_data_mut() {
                element.image_broken = false;
                element.image_failed = false;
            }
            self.doc.nodes[target_id].clear_layout_cache();
            self.doc.nodes[target_id].insert_damage(ALL_DAMAGE);
        }
        if is_img && !usable {
            self.doc.show_image_alt_text(target_id);
            return;
        }
        let node = &self.doc.nodes[target_id];
        if let Some(raw_src) = node.attr(attr) {
            if !raw_src.is_empty() {
                let Some(src) = self.doc.resolve_url(raw_src) else {
                    return;
                };
                // pyro: an `<img crossorigin>` is fetched in CORS mode (HTML "CORS settings
                // attribute": a missing value is no CORS, an invalid one `anonymous`), its
                // response kept apart from the same URL fetched otherwise.
                let mut options = blitz_traits::net::FetchOptions::default();
                if is_img {
                    options.referrer_policy = node.attrs().and_then(|attrs| attrs.iter().find(|attr| attr.name.ns == markup5ever::ns!() && &*attr.name.local == "referrerpolicy")).and_then(|attr| {
                        ["no-referrer", "no-referrer-when-downgrade", "origin", "origin-when-cross-origin", "same-origin", "strict-origin", "strict-origin-when-cross-origin", "unsafe-url"]
                            .into_iter().find(|token| attr.value.eq_ignore_ascii_case(token)).map(str::to_owned)
                    });
                    if let Some(value) = node.attr(local_name!("crossorigin")) {
                        options.cors = Some(if value.eq_ignore_ascii_case("use-credentials") {
                            blitz_traits::net::CorsCredentials::Include
                        } else {
                            blitz_traits::net::CorsCredentials::SameOrigin
                        });
                    }
                }
                let options = self.doc.request_options(options);
                let key = options.cache_key(src.as_str());
                if matches!(src.scheme(), "http" | "https") {
                    self.doc.nodes[target_id].element_data_mut().unwrap().source_image_request =
                        Some(Box::new((src.to_string(), options.clone())));
                }
                let src_string = key.as_str();

                // Check cache first
                if let Some(cached_image) = self.doc.image_cache.get(src_string) {
                    #[cfg(feature = "tracing")]
                    tracing::info!("Loading image {src_string} from cache");
                    // pyro: an `<object>` shows only some images (`replaced::object_shows_image`).
                    if object && !crate::layout::replaced::object_shows_image(cached_image) {
                        return;
                    }
                    let node = &mut self.doc.nodes[target_id];
                    node.element_data_mut().unwrap().special_data =
                        SpecialElementData::Image(Box::new(cached_image.clone()));
                    node.clear_layout_cache();
                    node.insert_damage(ALL_DAMAGE);
                    return;
                }

                // Check if there's already a pending request for this URL
                if let Some(waiting_list) = self.doc.pending_images.get_mut(src_string) {
                    #[cfg(feature = "tracing")]
                    tracing::info!("Image {src_string} already pending, queueing node {target_id}");
                    waiting_list.push((target_id, ImageType::Image));
                    return;
                }

                // Start fetch and track as pending
                #[cfg(feature = "tracing")]
                tracing::info!("Fetching image {src_string}");
                self.doc
                    .pending_images
                    .insert(src_string.to_string(), vec![(target_id, ImageType::Image)]);

                let mut request = self.doc.build_request(src);
                request.options = options.clone().for_destination(blitz_traits::net::Destination::Image);
                let handler = ImageHandler::new(ImageType::Image, &self.doc.raster_budget).options(options).document(&self.doc.url);
                self.doc.net_provider.fetch(
                    self.doc.id(),
                    request,
                    ResourceHandler::boxed(
                        self.doc.tx.clone(),
                        self.doc.id(),
                        None, // Don't pass node_id, we'll handle it via pending_images
                        self.doc.shell_provider.clone(),
                        handler,
                    ),
                );
            }
        }
    }

    fn load_iframe(&mut self, target_id: NodeId) {
        if self.doc.subdocument_depth >= crate::iframe::MAX_SUBDOCUMENT_DEPTH {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                "Not loading iframe: max sub-document nesting depth ({}) reached",
                crate::iframe::MAX_SUBDOCUMENT_DEPTH
            );
            return;
        }

        // pyro: in a render snapshot's document (it has a frame table), a frame element shows its
        // valid table entry's document or nothing, whatever attributes it has (`src`, `srcdoc`, an
        // unmarked element): never a load of its own.
        if self.doc.is_snapshot_document() {
            if let Some(frame) = self.doc.frame_entry(target_id) {
                self.doc.load_frame_document(target_id, &frame);
            }
            return;
        }
        let node = &self.doc.nodes[target_id];
        let Some(element) = node.element_data() else {
            return;
        };

        // `srcdoc` takes precedence over `src`
        if let Some(srcdoc) = element.attr(local_name!("srcdoc")) {
            let srcdoc = srcdoc.to_string();
            self.doc.load_iframe_srcdoc(target_id, &srcdoc);
            return;
        }

        let Some(raw_src) = element.attr(local_name!("src")) else {
            return;
        };
        if raw_src.is_empty() {
            return;
        }
        let Some(url) = self.doc.url.resolve_relative(raw_src) else {
            #[cfg(feature = "tracing")]
            tracing::warn!("Not loading iframe: could not resolve url {raw_src}");
            return;
        };
        self.doc.start_iframe_load(target_id, url);
    }

    fn load_custom_paint_src(&mut self, target_id: NodeId) {
        let node = &mut self.doc.nodes[target_id];
        if let Some(raw_src) = node.attr(local_name!("src")) {
            if let Ok(custom_paint_source_id) = raw_src.parse::<u64>() {
                self.recompute_is_animating = true;
                let canvas_data = SpecialElementData::Canvas(CanvasData {
                    custom_paint_source_id,
                });
                node.element_data_mut().unwrap().special_data = canvas_data;
            }
        }
    }

    fn process_button_input(&mut self, target_id: NodeId) {
        let node = &self.doc.nodes[target_id];
        let Some(data) = node.element_data() else {
            return;
        };

        let tagname = data.name.local.as_ref();
        let type_attr = data.attr(local_name!("type"));
        let value = data.attr(local_name!("value"));

        // Checkbox state must exist even if the input never gets a box (e.g. `display: none`
        // inside a `<label>`), so that it can be toggled and matched by `:checked`.
        if let ("input", Some("checkbox" | "radio")) = (tagname, type_attr) {
            let element = self.doc.nodes[target_id].element_data_mut().unwrap();
            if element.checkbox_input_checked().is_none() {
                let checked = element.has_attr(local_name!("checked"));
                element.special_data = SpecialElementData::CheckboxInput(checked);
                element.set_checkbox_input_checked(checked);
            }
            return;
        }

        // pyro: a file input's label (Chromium's "No file chosen"); its button is the box of its
        // `::file-selector-button` (`layout::construct::file_selector_button`).
        #[cfg(not(feature = "file-input"))]
        if tagname == "input"
            && type_attr.is_some_and(|t| t.eq_ignore_ascii_case("file"))
            && self.doc.nodes[target_id].children.is_empty()
        {
            let id = self.create_text_node("No file chosen");
            self.append_children(target_id, &[id]);
            return;
        }

        // Add content of "value" attribute as a text node child if:
        //   - Tag name is
        if let ("input", Some("button" | "submit" | "reset"), Some(value)) =
            (tagname, type_attr, value)
        {
            // pyro: an empty label is still a line, as in Chromium: the button is one line
            // tall (a zero-width word joiner, which draws nothing; WPT css-inline
            // `empty-span-size-001`).
            let value = if value.is_empty() { "\u{2060}".to_string() } else { value.to_string() };
            let id = self.create_text_node(&value);
            self.append_children(target_id, &[id]);
            return;
        }
        #[cfg(feature = "file-input")]
        if let ("input", Some("file")) = (tagname, type_attr) {
            let button_id = self.create_element(
                qual_name!("button", html),
                vec![
                    Attribute {
                        name: qual_name!("type", html),
                        value: "button".to_string(),
                    },
                    Attribute {
                        name: qual_name!("tabindex", html),
                        value: "-1".to_string(),
                    },
                ],
            );
            let label_id = self.create_element(qual_name!("label", html), vec![]);
            let text_id = self.create_text_node("No File Selected");
            let button_text_id = self.create_text_node("Browse");
            self.append_children(target_id, &[button_id, label_id]);
            self.append_children(label_id, &[text_id]);
            self.append_children(button_id, &[button_text_id]);
        }
    }
}

/// pyro: attributes whose value is element state
/// ([`ElementData::sync_lp_attribute_state`]): pyro-dom's `state` tokens and an
/// `<option>`'s `selected`.
fn is_state_attribute(element: &QualName, name: &QualName, engine: bool) -> bool {
    (engine && &*name.ns == crate::PYRO_STATE_NS && &*name.local == "state")
        || (element.local == local_name!("option") && name.local == local_name!("selected"))
}

/// pyro: pyro-dom's `urn:x-pyro:state` `state` attribute.
#[inline]
fn is_lp_state(name: &QualName) -> bool {
    &*name.local == "state" && &*name.ns == crate::PYRO_STATE_NS
}

/// Set 'checked' state on an input based on given attributevalue
fn set_input_checked_state(element: &mut ElementData, value: String) {
    let Ok(checked) = value.parse() else {
        return;
    };
    match element.special_data {
        SpecialElementData::CheckboxInput(_) => element.set_checkbox_input_checked(checked),
        // If we have just constructed the element, set the node attribute,
        // and NodeSpecificData will be created from that later
        // this simulates the checked attribute being set in html,
        // and the element's checked property being set from that
        SpecialElementData::None => element.attrs.push(Attribute {
            name: qual_name!("checked", html),
            value: checked.to_string(),
        }),
        _ => {}
    }
}

/// Type that allows mutable access to the viewport
/// And syncs it back to stylist on drop.
pub struct ViewportMut<'doc> {
    doc: &'doc mut BaseDocument,
    initial_viewport: Viewport,
}
impl ViewportMut<'_> {
    pub fn new(doc: &mut BaseDocument) -> ViewportMut<'_> {
        let initial_viewport = doc.viewport.clone();
        ViewportMut {
            doc,
            initial_viewport,
        }
    }
}
impl Deref for ViewportMut<'_> {
    type Target = Viewport;

    fn deref(&self) -> &Self::Target {
        &self.doc.viewport
    }
}
impl DerefMut for ViewportMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.doc.viewport
    }
}
impl Drop for ViewportMut<'_> {
    fn drop(&mut self) {
        if self.doc.viewport == self.initial_viewport {
            return;
        }

        let changes = DeviceChanges::from_viewports(&self.initial_viewport, &self.doc.viewport);
        self.doc.queue_device_changes(changes);
    }
}

#[cfg(test)]
mod test {
    use style::media_queries::MediaType;
    use style::servo_arc::Arc as ServoArc;
    use style_dom::ElementState;

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use blitz_traits::shell::{ColorScheme, ShellProvider, Viewport};

    use super::{DocumentMutator, NodeId};
    use crate::{Attribute, BaseDocument, DocumentConfig, ElementData, NodeData, qual_name};

    #[test]
    fn unresolvable_relative_urls_are_skipped() {
        // Without a base url, relative urls cannot be resolved against the default
        // `data:` document url. Loading such resources should be skipped, not panic.
        let mut document = BaseDocument::new(DocumentConfig::default());
        let root_id = document.root_node().id;
        let attr = |name, value: &str| Attribute {
            name,
            value: value.to_string(),
        };

        let mut mutator = document.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let img = mutator.create_element(
            qual_name!("img"),
            vec![attr(qual_name!("src"), "image.png")],
        );
        let link = mutator.create_element(
            qual_name!("link"),
            vec![
                attr(qual_name!("rel"), "stylesheet"),
                attr(qual_name!("href"), "style.css"),
            ],
        );
        mutator.append_children(html, &[img, link]);
        mutator.append_children(root_id, &[html]);
        mutator.set_attribute(img, qual_name!("src"), "other.png");
        drop(mutator);

        document.resolve(0.0);
    }

    #[test]
    fn removing_href_from_a_link_without_a_loaded_stylesheet() {
        // An icon link never holds a stylesheet, and a stylesheet link holds none
        // until its fetch completes (here it never starts, as the url is unresolvable).
        let mut document = BaseDocument::new(DocumentConfig::default());
        let root_id = document.root_node().id;
        let attr = |name, value: &str| Attribute {
            name,
            value: value.to_string(),
        };

        let mut mutator = document.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let icon = mutator.create_element(
            qual_name!("link"),
            vec![
                attr(qual_name!("rel"), "icon"),
                attr(qual_name!("href"), "favicon.png"),
            ],
        );
        let stylesheet = mutator.create_element(
            qual_name!("link"),
            vec![
                attr(qual_name!("rel"), "stylesheet"),
                attr(qual_name!("href"), "style.css"),
            ],
        );
        mutator.append_children(html, &[icon, stylesheet]);
        mutator.append_children(root_id, &[html]);
        mutator.clear_attribute(icon, qual_name!("href"));
        mutator.clear_attribute(stylesheet, qual_name!("href"));
        drop(mutator);

        document.resolve(0.0);
    }

    #[cfg(feature = "custom-widget")]
    #[test]
    fn custom_widget_replays_initial_attributes() {
        use std::sync::Mutex;

        type AttributeChange = (String, Option<String>, Option<String>);
        struct RecordingWidget(Arc<Mutex<Vec<AttributeChange>>>);

        impl crate::Widget for RecordingWidget {
            fn attribute_changed(&mut self, name: &str, old: Option<&str>, new: Option<&str>) {
                self.0.lock().unwrap().push((
                    name.to_owned(),
                    old.map(str::to_owned),
                    new.map(str::to_owned),
                ));
            }
        }

        for via_mutator in [false, true] {
            for initially_empty in [false, true] {
                let mut document = BaseDocument::new(DocumentConfig::default());
                let attrs = if initially_empty {
                    vec![]
                } else {
                    vec![
                        Attribute {
                            name: qual_name!("title"),
                            value: "initial".into(),
                        },
                        Attribute {
                            name: qual_name!("lang"),
                            value: "".into(),
                        },
                    ]
                };
                let id = document.create_node(NodeData::Element(Box::new(ElementData::new(
                    qual_name!("div"),
                    attrs,
                ))));
                let calls = Arc::new(Mutex::new(Vec::new()));
                let widget = Box::new(RecordingWidget(Arc::clone(&calls)));
                if via_mutator {
                    document.mutate().set_custom_widget(id, widget);
                } else {
                    document.set_custom_widget(id, widget);
                }

                let mut expected = if initially_empty {
                    vec![]
                } else {
                    vec![
                        ("title".into(), None, Some("initial".into())),
                        ("lang".into(), None, Some("".into())),
                    ]
                };
                assert_eq!(*calls.lock().unwrap(), expected);

                let mut mutator = document.mutate();
                mutator.set_attribute(id, qual_name!("title"), "updated");
                mutator.clear_attribute(id, qual_name!("title"));
                expected.extend([
                    (
                        "title".into(),
                        (!initially_empty).then(|| "initial".into()),
                        Some("updated".into()),
                    ),
                    ("title".into(), Some("updated".into()), None),
                ]);
                assert_eq!(*calls.lock().unwrap(), expected);
            }
        }
    }

    #[test]
    fn media_type_defaults_to_screen() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        assert_eq!(*document.media_type(), MediaType::screen());
        assert_eq!(document.stylist_device().media_type(), MediaType::screen());
    }

    #[test]
    fn media_type_honors_config() {
        let mut document = BaseDocument::new(DocumentConfig {
            media_type: Some(MediaType::print()),
            ..Default::default()
        });
        assert_eq!(*document.media_type(), MediaType::print());
        assert_eq!(document.stylist_device().media_type(), MediaType::print());
    }

    #[test]
    fn set_media_type_updates_stylist_device() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        assert_eq!(document.stylist_device().media_type(), MediaType::screen());

        document.set_media_type(MediaType::print());
        assert_eq!(*document.media_type(), MediaType::print());
        assert_eq!(document.stylist_device().media_type(), MediaType::print());
    }

    #[test]
    fn mutator_remove_disabled() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let id = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("button"),
            vec![Attribute {
                name: qual_name!("disabled"),
                value: "".into(),
            }],
        ))));

        let node = document.get_node(id).unwrap();
        assert!(
            node.element_state().contains(ElementState::DISABLED),
            "form node is disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is not enabled yet"
        );

        let mut mutator = document.mutate();
        mutator.clear_attribute(id, qual_name!("disabled"));
        drop(mutator);

        let node = document.get_node(id).unwrap();
        assert!(
            !node.element_state().contains(ElementState::DISABLED),
            "form node is no longer disabled"
        );
        assert!(
            node.element_state().contains(ElementState::ENABLED),
            "form node is enabled"
        );
    }

    #[test]
    fn mutator_set_disabled() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let id = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("button"),
            vec![],
        ))));

        let node = document.get_node(id).unwrap();
        assert!(
            !node.element_state().contains(ElementState::DISABLED),
            "form node is not disabled"
        );
        assert!(
            node.element_state().contains(ElementState::ENABLED),
            "form node is enabled"
        );

        let mut mutator = document.mutate();
        mutator.set_attribute(id, qual_name!("disabled"), "");
        drop(mutator);

        let node = document.get_node(id).unwrap();

        assert!(
            node.element_state().contains(ElementState::DISABLED),
            "form node is disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is no longer enabled enabled"
        );
    }

    #[test]
    fn mutator_set_disabled_invalid_node() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let id = document.create_node(NodeData::Element(Box::new(ElementData::new(
            qual_name!("a"),
            vec![],
        ))));

        let node = document.get_node(id).unwrap();
        assert!(
            !node.element_state().contains(ElementState::DISABLED),
            "form node is not disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is enabled"
        );

        let mut mutator = document.mutate();
        mutator.set_attribute(id, qual_name!("disabled"), "");
        drop(mutator);

        let node = document.get_node(id).unwrap();
        assert!(
            !node.element_state().contains(ElementState::DISABLED),
            "form node is not disabled"
        );
        assert!(
            !node.element_state().contains(ElementState::ENABLED),
            "form node is enabled"
        );
    }

    #[test]
    fn mutator_id_attribute_updates_id_map() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let root_id = document.root_node().id;

        let node_id = {
            let mut mutator = document.mutate();
            let node_id = mutator.create_element(
                qual_name!("div"),
                vec![Attribute {
                    name: qual_name!("id"),
                    value: "old".into(),
                }],
            );
            mutator.append_children(root_id, &[node_id]);
            node_id
        };
        assert_eq!(document.get_element_by_id("old"), Some(node_id));

        {
            let mut mutator = document.mutate();
            mutator.set_attribute(node_id, qual_name!("id"), "new");
        }
        assert_eq!(document.get_element_by_id("new"), Some(node_id));
        assert_eq!(document.get_element_by_id("old"), None);

        {
            let mut mutator = document.mutate();
            mutator.clear_attribute(node_id, qual_name!("id"));
        }
        assert_eq!(document.get_element_by_id("new"), None);
    }

    #[test]
    fn get_element_by_id_duplicate_ids_first_in_tree_order_wins() {
        let mut document = BaseDocument::new(DocumentConfig::default());
        let root_id = document.root_node().id;

        let (first_id, second_id) = {
            let mut mutator = document.mutate();
            let first_id = mutator.create_element(qual_name!("div"), vec![]);
            let second_id = mutator.create_element(qual_name!("div"), vec![]);
            mutator.append_children(root_id, &[first_id, second_id]);
            // Assign the id to the later node first so that insertion order
            // differs from tree order
            mutator.set_attribute(second_id, qual_name!("id"), "dup");
            mutator.set_attribute(first_id, qual_name!("id"), "dup");
            (first_id, second_id)
        };
        assert_eq!(document.get_element_by_id("dup"), Some(first_id));

        {
            let mut mutator = document.mutate();
            mutator.remove_node(first_id);
        }
        assert_eq!(document.get_element_by_id("dup"), Some(second_id));
    }

    #[derive(Default)]
    struct RedrawShell {
        redraw_requests: AtomicUsize,
    }

    impl ShellProvider for RedrawShell {
        fn request_redraw(&self) {
            self.redraw_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn mutator_requests_redraw_only_after_mutation() {
        let shell = Arc::new(RedrawShell::default());
        let mut document = BaseDocument::new(DocumentConfig {
            shell_provider: Some(shell.clone()),
            ..Default::default()
        });
        let root_id = document.root_node().id;

        {
            let mut mutator = document.mutate();
            let parent_id = mutator.create_element(qual_name!("div"), vec![]);
            let child_id = mutator.create_element(qual_name!("span"), vec![]);
            mutator.append_children(parent_id, &[child_id]);
            mutator.remove_and_drop_all_children(parent_id);
            mutator.set_attribute(parent_id, qual_name!("id"), "detached");
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 0);

        {
            let mutator = document.mutate();
            assert_eq!(mutator.child_ids(root_id).len(), 0);
        }

        {
            let mut mutator = document.mutate();
            let node_id = mutator.create_element(qual_name!("div"), vec![]);
            mutator.append_children(root_id, &[node_id]);
            mutator.set_attribute(node_id, qual_name!("id"), "in-document");
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 1);

        {
            let mut mutator = document.mutate();
            let parent_id = mutator.create_element(qual_name!("div"), vec![]);
            let child_id = mutator.create_element(qual_name!("span"), vec![]);
            mutator.append_children(root_id, &[parent_id]);
            mutator.append_children(parent_id, &[child_id]);
            mutator.remove_and_drop_all_children(parent_id);
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 2);

        {
            let mut mutator = document.mutate();
            let parent_id = mutator.create_element(qual_name!("div"), vec![]);
            let child_id = mutator.create_element(qual_name!("span"), vec![]);
            let detached_target_id = mutator.create_element(qual_name!("div"), vec![]);
            mutator.append_children(root_id, &[parent_id]);
            mutator.append_children(parent_id, &[child_id]);
            assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 2);
            mutator.append_children(detached_target_id, &[child_id]);
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 3);
    }

    #[cfg(feature = "devtools")]
    #[test]
    fn devtools_provider_records_in_document_mutations() {
        use blitz_traits::devtools::{
            AttributeChanged, CharacterDataChanged, DevtoolsProvider, MutationRecord, NodeInserted,
            NodeRemoved,
        };
        use std::sync::Mutex;

        #[derive(Default)]
        struct Recorder(Mutex<Vec<MutationRecord>>);
        impl DevtoolsProvider for Recorder {
            fn mutations(&self, records: &[MutationRecord]) {
                self.0.lock().unwrap().extend_from_slice(records);
            }
        }

        let inserted = |parent_id, node_id, previous_sibling_id| {
            MutationRecord::NodeInserted(NodeInserted {
                parent_id,
                node_id,
                previous_sibling_id,
            })
        };
        let removed =
            |parent_id, node_id| MutationRecord::NodeRemoved(NodeRemoved { parent_id, node_id });
        let attr_changed = |node_id, name: &str| {
            MutationRecord::AttributeChanged(AttributeChanged {
                node_id,
                name: name.into(),
            })
        };
        let text_changed =
            |node_id| MutationRecord::CharacterDataChanged(CharacterDataChanged { node_id });

        let recorder = Arc::new(Recorder::default());
        let mut document = BaseDocument::new(DocumentConfig {
            devtools_provider: Some(recorder.clone()),
            ..Default::default()
        });
        let root_id = document.root_node().id;
        let take = || std::mem::take(&mut *recorder.0.lock().unwrap());

        // Building a detached subtree is not reported; inserting it reports its root only
        let mut mutator = document.mutate();
        let div = mutator.create_element(qual_name!("div"), vec![]);
        let text = mutator.create_text_node("a");
        mutator.append_children(div, &[text]);
        mutator.set_attribute(div, qual_name!("id"), "x");
        mutator.set_node_text(text, "b");
        mutator.flush();
        assert!(take().is_empty());
        // Mutations are delivered when the mutator is flushed
        mutator.append_children(root_id, &[div]);
        assert!(take().is_empty());
        mutator.flush();
        assert_eq!(take(), [inserted(root_id, div, None)]);

        mutator.set_attribute(div, qual_name!("class"), "c");
        mutator.clear_attribute(div, qual_name!("class"));
        mutator.clear_attribute(div, qual_name!("class"));
        mutator.set_node_text(text, "c");
        mutator.set_node_text(text, "c");
        mutator.append_text_to_node(text, "d").unwrap();
        let comment = mutator.create_comment_node("e");
        mutator.set_node_text(comment, "f");
        mutator.append_children(div, &[comment]);
        mutator.set_node_text(comment, "g");
        mutator.set_node_text(comment, "g");
        mutator.flush();
        assert_eq!(
            take(),
            [
                attr_changed(div, "class"),
                attr_changed(div, "class"),
                text_changed(text),
                text_changed(text),
                inserted(div, comment, Some(text)),
                text_changed(comment),
            ]
        );
        assert!(matches!(
            &mutator.doc.get_node(comment).unwrap().data,
            NodeData::Comment { contents } if contents == "g"
        ));

        // Moving a node reports its removal and insertion
        let span = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(root_id, &[span]);
        mutator.append_children(span, &[text]);
        mutator.remove_node(span);
        mutator.append_children(root_id, &[span]);
        mutator.remove_and_drop_all_children(span);
        mutator.remove_and_drop_node(div);
        mutator.flush();
        assert_eq!(
            take(),
            [
                inserted(root_id, span, Some(div)),
                removed(div, text),
                inserted(span, text, None),
                removed(root_id, span),
                inserted(root_id, span, Some(div)),
                removed(span, text),
                removed(root_id, div),
            ]
        );

        // Whitespace-only text is skipped when finding the previous sibling
        let ws = mutator.create_text_node(" ");
        let b = mutator.create_element(qual_name!("b"), vec![]);
        mutator.append_children(span, &[ws, b]);
        mutator.flush();
        assert_eq!(take(), [inserted(span, ws, None), inserted(span, b, None)]);
        drop(mutator);
    }

    #[test]
    fn moving_subtree_out_of_document_clears_in_document_flag() {
        let shell = Arc::new(RedrawShell::default());
        let mut document = BaseDocument::new(DocumentConfig {
            shell_provider: Some(shell.clone()),
            ..Default::default()
        });
        let root_id = document.root_node().id;
        let (child_id, grandchild_id, detached_parent_id) = {
            let mut mutator = document.mutate();
            let in_document_parent_id = mutator.create_element(qual_name!("div"), vec![]);
            let child_id = mutator.create_element(qual_name!("div"), vec![]);
            let grandchild_id = mutator.create_element(qual_name!("span"), vec![]);
            let detached_parent_id = mutator.create_element(qual_name!("section"), vec![]);
            mutator.append_children(root_id, &[in_document_parent_id]);
            mutator.append_children(in_document_parent_id, &[child_id]);
            mutator.append_children(child_id, &[grandchild_id]);
            (child_id, grandchild_id, detached_parent_id)
        };
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 1);
        assert!(document.get_node(child_id).unwrap().flags.is_in_document());
        assert!(
            document
                .get_node(grandchild_id)
                .unwrap()
                .flags
                .is_in_document()
        );

        {
            let mut mutator = document.mutate();
            mutator.append_children(detached_parent_id, &[child_id]);
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 2);
        assert!(!document.get_node(child_id).unwrap().flags.is_in_document());
        assert!(
            !document
                .get_node(grandchild_id)
                .unwrap()
                .flags
                .is_in_document()
        );

        {
            let mut mutator = document.mutate();
            mutator.set_attribute(child_id, qual_name!("id"), "detached");
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 2);

        {
            let mut mutator = document.mutate();
            mutator.append_children(root_id, &[child_id]);
        }
        assert_eq!(shell.redraw_requests.load(Ordering::Relaxed), 3);
        assert!(document.get_node(child_id).unwrap().flags.is_in_document());
        assert!(
            document
                .get_node(grandchild_id)
                .unwrap()
                .flags
                .is_in_document()
        );
    }

    #[test]
    fn style_property_updates_nested_layout() {
        let mut document = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = document.root_node().id;

        let mover_id = {
            let mut mutator = document.mutate();
            let parent_id = mutator.create_element(qual_name!("div"), vec![]);
            let mover_id = mutator.create_element(qual_name!("div"), vec![]);
            mutator.set_style_property(parent_id, "position", "relative");
            mutator.set_style_property(parent_id, "width", "800px");
            mutator.set_style_property(parent_id, "height", "600px");
            mutator.set_style_property(mover_id, "position", "absolute");
            mutator.set_style_property(mover_id, "left", "0px");
            mutator.set_style_property(mover_id, "top", "0px");
            mutator.append_children(parent_id, &[mover_id]);
            mutator.append_children(root_id, &[parent_id]);
            mover_id
        };

        document.resolve(0.0);
        assert_eq!(
            document
                .get_node(mover_id)
                .unwrap()
                .final_layout()
                .location
                .x,
            0.0
        );

        {
            let mut mutator = document.mutate();
            mutator.set_style_property(mover_id, "left", "120px");
        }

        document.resolve(0.0);
        assert_eq!(
            document
                .get_node(mover_id)
                .unwrap()
                .final_layout()
                .location
                .x,
            120.0
        );
    }

    /// Hit testing before the first resolve reaches elements that have no computed styles
    /// yet: a pointer already over a window as it opens.
    #[test]
    fn hit_testing_before_the_first_resolve() {
        let mut document = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = document.root_node().id;
        {
            let mut mutator = document.mutate();
            let html = mutator.create_element(qual_name!("html", html), vec![]);
            mutator.append_children(root_id, &[html]);
        }
        // Nothing is laid out yet, so nothing is hit; what matters is that it doesn't panic.
        assert!(document.hit(10.0, 10.0).is_none());
    }

    #[test]
    fn style_property_mutation_copies_blocks_in_rule_tree() {
        let mut document = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = document.root_node().id;

        let div_id = {
            let mut mutator = document.mutate();
            let div_id = mutator.create_element(qual_name!("div"), vec![]);
            mutator.set_style_property(div_id, "width", "100px");
            mutator.set_style_property(div_id, "height", "50px");
            mutator.append_children(root_id, &[div_id]);
            div_id
        };

        let style_block = |document: &BaseDocument| {
            document
                .get_node(div_id)
                .unwrap()
                .element_data()
                .unwrap()
                .style_attribute
                .clone()
                .unwrap()
        };

        let before_resolve = style_block(&document);
        {
            let mut mutator = document.mutate();
            mutator.set_style_property(div_id, "height", "60px");
        }
        // Not yet in the rule tree: mutated in place.
        assert!(ServoArc::ptr_eq(&before_resolve, &style_block(&document)));

        document.resolve(0.0);
        let in_rule_tree = style_block(&document);
        assert!(
            in_rule_tree
                .read_with(&document.guard.read())
                .immutable
                .load(Ordering::Relaxed)
        );

        {
            let mut mutator = document.mutate();
            mutator.set_style_property(div_id, "width", "200px");
        }
        let after_set = style_block(&document);
        assert!(!ServoArc::ptr_eq(&in_rule_tree, &after_set));
        {
            let guard = document.guard.read();
            let old = in_rule_tree.read_with(&guard);
            let new = after_set.read_with(&guard);
            assert_eq!(old.declarations().len(), 2);
            assert_eq!(new.declarations().len(), 2);
            assert_ne!(old, new);
        }

        document.resolve(0.0);
        assert_eq!(
            document.get_node(div_id).unwrap().final_layout().size.width,
            200.0
        );

        let in_rule_tree = style_block(&document);
        {
            let mut mutator = document.mutate();
            mutator.remove_style_property(div_id, "height");
        }
        let after_remove = style_block(&document);
        assert!(!ServoArc::ptr_eq(&in_rule_tree, &after_remove));
        {
            let guard = document.guard.read();
            assert_eq!(in_rule_tree.read_with(&guard).declarations().len(), 2);
            assert_eq!(after_remove.read_with(&guard).declarations().len(), 1);
        }
    }

    /// pyro: the top layer follows `top-layer=N` state tokens by
    /// rank (re-ranking, leaving, many entries) and forgets dropped elements.
    #[test]
    fn top_layer_follows_state_tokens_and_drops() {
        use markup5ever::{LocalName, Namespace, QualName};
        let state = || QualName::new(None, Namespace::from(crate::PYRO_STATE_NS), LocalName::from("state"));
        let mut document = BaseDocument::new(DocumentConfig::default());
        let ids: Vec<_> = {
            let mut mutator = document.mutate();
            (0..2000).map(|_| mutator.create_element(qual_name!("div"), vec![])).collect()
        };
        let order = |doc: &BaseDocument| doc.top_layer.iter().collect::<Vec<_>>();
        {
            let mut mutator = document.mutate();
            // Enter top first: ranks, not arrival, give the order.
            for (rank, &id) in ids.iter().enumerate().rev() {
                mutator.set_attribute(id, state(), &format!("popover-open top-layer={rank}"));
            }
        }
        assert_eq!(order(&document), ids);
        assert!(ids.iter().all(|&id| document.is_in_top_layer(id)));
        {
            // The bottom one leaves; the rest move down a rank.
            let mut mutator = document.mutate();
            mutator.set_attribute(ids[0], state(), "popover-open");
            for (rank, &id) in ids[1..].iter().enumerate() {
                mutator.set_attribute(id, state(), &format!("popover-open top-layer={rank}"));
            }
            // An element re-entering goes on top.
            mutator.set_attribute(ids[0], state(), &format!("modal top-layer={}", ids.len() - 1));
        }
        let mut expected = ids[1..].to_vec();
        expected.push(ids[0]);
        assert_eq!(order(&document), expected);
        {
            let mut mutator = document.mutate();
            mutator.clear_attribute(ids[5], state());
            mutator.remove_and_drop_node(ids[6]);
        }
        assert!(!document.is_in_top_layer(ids[5]) && !document.is_in_top_layer(ids[6]));
        expected.retain(|&id| id != ids[5] && id != ids[6]);
        assert_eq!(order(&document), expected);
    }

    fn qual_name(name: &str) -> crate::QualName {
        crate::QualName::new(None, crate::ns!(), crate::LocalName::from(name))
    }

    fn attr(name: &str, value: &str) -> Attribute {
        Attribute {
            name: qual_name(name),
            value: value.to_string(),
        }
    }

    fn div(m: &mut DocumentMutator<'_>, attrs: &[(&str, &str)]) -> NodeId {
        let attrs = attrs.iter().map(|(k, v)| attr(k, v)).collect();
        m.create_element(qual_name!("div"), attrs)
    }

    /// Tag, layout and resolved style of every element and `::before`/`::after`, in tree order.
    type Snapshot = Vec<(String, [f32; 4], Vec<String>)>;

    fn snapshot(doc: &BaseDocument) -> Snapshot {
        let mut out = Vec::new();
        let mut stack = vec![(doc.root_node().id, "")];
        while let Some((id, pseudo)) = stack.pop() {
            let node = doc.get_node(id).unwrap();
            if let Some(el) = node.element_data() {
                let l = node.final_layout();
                let rect = [l.location.x, l.location.y, l.size.width, l.size.height];
                let style = crate::resolved_style_property_names()
                    .iter()
                    .map(|name| doc.resolved_style_value(id, name))
                    .collect();
                out.push((format!("{}{pseudo}", el.name.local), rect, style));
            }
            stack.extend(node.after().map(|id| (id, "::after")));
            stack.extend(node.children.iter().rev().map(|id| (*id, "")));
            stack.extend(node.before().map(|id| (id, "::before")));
        }
        out
    }

    /// Asserts that the incremental result matches the fresh one, element by element,
    /// naming the first differing property.
    fn assert_matches_fresh(incremental: &Snapshot, fresh: &Snapshot) {
        let boxes = |s: &Snapshot| -> Vec<(String, [f32; 4])> {
            s.iter().map(|(n, r, _)| (n.clone(), *r)).collect()
        };
        assert_eq!(
            boxes(incremental),
            boxes(fresh),
            "incremental layout differs from a fresh layout of the mutated document"
        );
        for (i, ((name, _, inc), (_, _, fre))) in incremental.iter().zip(fresh).enumerate() {
            let differing: Vec<String> = crate::resolved_style_property_names()
                .iter()
                .zip(inc.iter().zip(fre))
                .filter(|(_, (a, b))| a != b)
                .map(|(property, (a, b))| format!("{property}: {a:?} vs {b:?}"))
                .collect();
            assert!(
                differing.is_empty(),
                "resolved style of {name} (element {i}) differs from a fresh layout of the mutated document: {}",
                differing.join(", ")
            );
        }
    }

    /// Checks that a document mutated after its first layout ends up with the same layout
    /// and styles as a document built directly in the mutated state. `build` returns the
    /// node ids that `mutate` operates on; the mutation must change the layout or styles.
    fn check_incremental_restyle(
        css: &str,
        build: impl Fn(&mut DocumentMutator<'_>) -> Vec<NodeId>,
        mutate: impl Fn(&mut DocumentMutator<'_>, &[NodeId]),
    ) {
        check_incremental_restyle_steps(css, build, &[&mutate]);
    }

    /// One mutation batch of [`check_incremental_restyle_steps`].
    type Step<'a> = &'a dyn Fn(&mut DocumentMutator<'_>, &[NodeId]);

    /// Like [`check_incremental_restyle`], but each step is a separate mutation batch
    /// with a layout in between; the fresh document applies all steps before its layout.
    fn check_incremental_restyle_steps(
        css: &str,
        build: impl Fn(&mut DocumentMutator<'_>) -> Vec<NodeId>,
        steps: &[Step<'_>],
    ) {
        let new_document = || {
            let mut doc = BaseDocument::new(DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                ..Default::default()
            });
            doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(&format!(
                "html, div {{ display: block }} {css}"
            )));
            let root_id = doc.root_node().id;
            let ids = {
                let mut m = doc.mutate();
                let html = m.create_element(qual_name!("html"), vec![]);
                m.append_children(root_id, &[html]);
                build(&mut m)
            };
            (doc, ids)
        };

        let (mut incremental, ids) = new_document();
        incremental.resolve(0.0);
        let before = snapshot(&incremental);
        for mutate in steps {
            mutate(&mut incremental.mutate(), &ids);
            incremental.resolve(0.0);
        }
        let after = snapshot(&incremental);

        let (mut fresh, ids) = new_document();
        for mutate in steps {
            mutate(&mut fresh.mutate(), &ids);
        }
        fresh.resolve(0.0);
        assert_matches_fresh(&after, &snapshot(&fresh));
        assert_ne!(
            before, after,
            "the mutation should change the layout or styles"
        );
    }

    fn parent_with_children(m: &mut DocumentMutator<'_>, count: usize) -> Vec<NodeId> {
        let root_id = m.doc.root_element().id;
        let parent = div(m, &[]);
        let children: Vec<NodeId> = (0..count).map(|_| div(m, &[])).collect();
        m.append_children(parent, &children);
        m.append_children(root_id, &[parent]);
        std::iter::once(parent).chain(children).collect()
    }

    /// `html > parent.hidden > [child, child]`, ids in that order; `.hidden` is `display: none`.
    fn hidden_parent_with_children(m: &mut DocumentMutator<'_>, count: usize) -> Vec<NodeId> {
        let ids = parent_with_children(m, count);
        m.set_attribute(ids[0], qual_name!("class"), "hidden");
        ids
    }

    const HIDDEN_CSS: &str = "div { height: 10px; width: 100px } .hidden { display: none }";

    fn unhide(m: &mut DocumentMutator<'_>, ids: &[NodeId]) {
        m.clear_attribute(ids[0], qual_name!("class"));
    }

    #[test]
    fn attribute_change_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} [data-a] ~ div {{ width: 200px; color: red }}"),
            |m| hidden_parent_with_children(m, 3),
            &[
                &|m, ids| m.set_attribute(ids[1], qual_name("data-a"), "1"),
                &unhide,
            ],
        );
    }

    #[test]
    fn class_change_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} .x {{ width: 200px }} .x div {{ color: red }}"),
            |m| {
                let ids = hidden_parent_with_children(m, 1);
                let grandchild = div(m, &[]);
                m.append_children(ids[1], &[grandchild]);
                ids
            },
            &[
                &|m, ids| m.set_attribute(ids[1], qual_name!("class"), "x"),
                &unhide,
            ],
        );
    }

    #[test]
    fn style_change_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} .half {{ width: 50% }}"),
            |m| {
                let ids = hidden_parent_with_children(m, 1);
                let grandchild = div(m, &[("class", "half")]);
                m.append_children(ids[1], &[grandchild]);
                ids
            },
            &[
                &|m, ids| m.set_attribute(ids[1], qual_name!("style"), "width: 400px"),
                &unhide,
            ],
        );
    }

    #[test]
    fn insert_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} div:last-child {{ height: 30px }} div:empty {{ width: 20px }}"),
            |m| hidden_parent_with_children(m, 1),
            &[
                &|m, ids| {
                    let child = div(m, &[]);
                    m.append_children(ids[0], &[child]);
                },
                &unhide,
            ],
        );
    }

    #[test]
    fn remove_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} div:first-child {{ height: 30px }}"),
            |m| hidden_parent_with_children(m, 2),
            &[
                &|m, ids| {
                    m.remove_and_drop_node(ids[1]);
                },
                &unhide,
            ],
        );
    }

    #[test]
    fn pseudo_rule_matched_in_hidden_subtree_then_unhide() {
        check_incremental_restyle_steps(
            &format!(
                "{HIDDEN_CSS} .x > div::before {{ content: 'a'; display: block; height: 20px }}"
            ),
            |m| {
                let ids = hidden_parent_with_children(m, 1);
                let grandchild = div(m, &[]);
                m.append_children(ids[1], &[grandchild]);
                ids
            },
            &[
                &|m, ids| m.set_attribute(ids[1], qual_name!("class"), "x"),
                &unhide,
            ],
        );
    }

    #[test]
    fn unhide_and_attribute_change_in_one_batch() {
        check_incremental_restyle(
            &format!("{HIDDEN_CSS} [data-a] ~ div {{ width: 200px; color: red }}"),
            |m| hidden_parent_with_children(m, 3),
            |m, ids| {
                unhide(m, ids);
                m.set_attribute(ids[1], qual_name("data-a"), "1");
            },
        );
    }

    #[test]
    fn hide_and_attribute_change_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} [data-a] ~ div {{ width: 200px; color: red }}"),
            |m| parent_with_children(m, 3),
            &[
                &|m, ids| {
                    m.set_attribute(ids[0], qual_name!("class"), "hidden");
                    m.set_attribute(ids[1], qual_name("data-a"), "1");
                },
                &unhide,
            ],
        );
    }

    #[test]
    fn hide_then_mutate_then_unhide() {
        check_incremental_restyle_steps(
            &format!("{HIDDEN_CSS} .x {{ color: red }} div:empty {{ width: 20px }}"),
            |m| parent_with_children(m, 2),
            &[
                &|m, ids| m.set_attribute(ids[0], qual_name!("class"), "hidden"),
                &|m, ids| {
                    m.set_attribute(ids[1], qual_name!("class"), "x");
                    m.remove_and_drop_node(ids[2]);
                },
                &unhide,
            ],
        );
    }

    #[test]
    fn class_change_restyles_descendants() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } .wide div { width: 300px }",
            |m| parent_with_children(m, 1),
            |m, ids| m.set_attribute(ids[0], qual_name!("class"), "wide"),
        );
    }

    #[test]
    fn class_removal_restyles_descendants() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } .wide div { width: 300px }",
            |m| {
                let ids = parent_with_children(m, 1);
                m.set_attribute(ids[0], qual_name!("class"), "wide");
                ids
            },
            |m, ids| m.clear_attribute(ids[0], qual_name!("class")),
        );
    }

    #[test]
    fn id_change_restyles_children() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } #x > div { height: 50px }",
            |m| parent_with_children(m, 2),
            |m, ids| m.set_attribute(ids[0], qual_name!("id"), "x"),
        );
    }

    #[test]
    fn attribute_change_restyles_later_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } [data-a] ~ div { width: 200px }",
            |m| parent_with_children(m, 3),
            |m, ids| m.set_attribute(ids[1], qual_name("data-a"), "1"),
        );
    }

    #[test]
    fn style_attribute_change_restyles_inheriting_descendants() {
        check_incremental_restyle(
            "div { height: 10px } .em { width: 2em }",
            |m| {
                let root_id = m.doc.root_element().id;
                let parent = div(m, &[]);
                let child = div(m, &[("class", "em")]);
                m.append_children(parent, &[child]);
                m.append_children(root_id, &[parent]);
                vec![parent, child]
            },
            |m, ids| m.set_attribute(ids[0], qual_name!("style"), "font-size: 40px"),
        );
    }

    #[test]
    fn style_width_change_relayouts_descendants() {
        check_incremental_restyle(
            "div { height: 10px } .half { width: 50% }",
            |m| {
                let root_id = m.doc.root_element().id;
                let parent = div(m, &[("style", "width: 400px")]);
                let child = div(m, &[("class", "half")]);
                m.append_children(parent, &[child]);
                m.append_children(root_id, &[parent]);
                vec![parent, child]
            },
            |m, ids| m.set_style_property(ids[0], "width", "200px"),
        );
    }

    #[test]
    fn appending_a_child_restyles_empty_parent() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } div:empty { height: 30px }",
            |m| parent_with_children(m, 0),
            |m, ids| {
                let child = div(m, &[]);
                m.append_children(ids[0], &[child]);
            },
        );
    }

    #[test]
    fn removing_the_last_child_restyles_empty_parent() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } div:empty { height: 30px }",
            |m| parent_with_children(m, 1),
            |m, ids| {
                m.remove_and_drop_node(ids[1]);
            },
        );
    }

    #[test]
    fn inserting_a_child_restyles_nth_child_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } div:nth-child(2n) { width: 200px }",
            |m| parent_with_children(m, 3),
            |m, ids| {
                let child = div(m, &[]);
                m.insert_nodes_before(ids[1], &[child]);
            },
        );
    }

    #[test]
    fn removing_a_child_restyles_nth_child_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } div:nth-child(2n) { width: 200px }",
            |m| parent_with_children(m, 3),
            |m, ids| {
                m.remove_and_drop_node(ids[1]);
            },
        );
    }

    #[test]
    fn appending_a_child_restyles_previous_last_child() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } div:last-child { width: 200px }",
            |m| parent_with_children(m, 2),
            |m, ids| {
                let child = div(m, &[]);
                m.append_children(ids[0], &[child]);
            },
        );
    }

    #[test]
    fn removing_the_first_child_restyles_new_first_child() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } div:first-child { width: 200px }",
            |m| parent_with_children(m, 2),
            |m, ids| {
                m.remove_and_drop_node(ids[1]);
            },
        );
    }

    #[test]
    fn inserting_a_sibling_restyles_later_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } .a ~ div { width: 200px }",
            |m| parent_with_children(m, 2),
            |m, ids| {
                let child = div(m, &[("class", "a")]);
                m.insert_nodes_before(ids[1], &[child]);
            },
        );
    }

    #[test]
    fn style_change_on_inline_element_rebuilds_inline_layout() {
        check_incremental_restyle(
            "div { width: 300px; font-size: 20px }",
            |m| {
                let root_id = m.doc.root_element().id;
                let block = div(m, &[]);
                let before = m.create_text_node("Text is available under the ");
                let link = m.create_element(qual_name!("a"), vec![]);
                let link_text =
                    m.create_text_node("Creative Commons Attribution-ShareAlike 4.0 License");
                let after = m.create_text_node("; additional terms may apply.");
                m.append_children(link, &[link_text]);
                m.append_children(block, &[before, link, after]);
                m.append_children(root_id, &[block]);
                vec![block, link]
            },
            |m, ids| m.set_attribute(ids[1], qual_name!("style"), "font-size: 40px"),
        );
    }

    #[test]
    fn moving_a_child_restyles_both_parents() {
        check_incremental_restyle(
            "div { height: 10px; width: 50px } div:empty { height: 30px } div:last-child { width: 200px }",
            |m| {
                let root_id = m.doc.root_element().id;
                let a = div(m, &[]);
                let b = div(m, &[]);
                let child = div(m, &[]);
                let other = div(m, &[]);
                m.append_children(a, &[child, other]);
                m.append_children(root_id, &[a, b]);
                vec![a, b, child]
            },
            |m, ids| m.append_children(ids[1], &[ids[2]]),
        );
    }

    #[test]
    fn class_change_recolors_descendants() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } .x div { color: red }",
            |m| parent_with_children(m, 1),
            |m, ids| m.set_attribute(ids[0], qual_name!("class"), "x"),
        );
    }

    const PSEUDO_CSS: &str = "div { height: 10px; width: 100px } \
        .x > div::before { content: 'a'; display: block; height: 20px }";

    #[test]
    fn class_change_creates_before_pseudo_on_children() {
        check_incremental_restyle(
            PSEUDO_CSS,
            |m| parent_with_children(m, 1),
            |m, ids| m.set_attribute(ids[0], qual_name!("class"), "x"),
        );
    }

    #[test]
    fn class_removal_removes_before_pseudo_on_children() {
        check_incremental_restyle(
            PSEUDO_CSS,
            |m| {
                let ids = parent_with_children(m, 1);
                m.set_attribute(ids[0], qual_name!("class"), "x");
                ids
            },
            |m, ids| m.clear_attribute(ids[0], qual_name!("class")),
        );
    }

    #[test]
    fn attribute_change_creates_after_pseudo_on_later_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } \
             [data-a] ~ div::after { content: 'a'; display: block; height: 20px }",
            |m| parent_with_children(m, 2),
            |m, ids| m.set_attribute(ids[1], qual_name("data-a"), ""),
        );
    }

    #[test]
    fn class_change_recolors_before_pseudo_on_children() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } \
             div::before { content: 'a'; display: block; height: 20px } \
             .x > div::before { color: red }",
            |m| parent_with_children(m, 1),
            |m, ids| m.set_attribute(ids[0], qual_name!("class"), "x"),
        );
    }

    #[test]
    fn attribute_change_recolors_after_pseudo_on_later_siblings() {
        check_incremental_restyle(
            "div { height: 10px; width: 100px } \
             div::after { content: 'a'; display: block; height: 20px } \
             [data-a] ~ div::after { color: red }",
            |m| parent_with_children(m, 2),
            |m, ids| m.set_attribute(ids[1], qual_name("data-a"), ""),
        );
    }

    /// Like `check_incremental_restyle`, with the focus on `ids[focus]` before the first layout.
    fn check_incremental_restyle_with_focus(
        css: &str,
        focus: usize,
        build: impl Fn(&mut DocumentMutator<'_>) -> Vec<NodeId>,
        mutate: impl Fn(&mut DocumentMutator<'_>, &[NodeId]),
    ) {
        let new_document = || {
            let mut doc = BaseDocument::new(DocumentConfig {
                viewport: Some(Viewport::new(800, 600, 1.0, ColorScheme::Light)),
                ..Default::default()
            });
            doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(&format!(
                "html, div {{ display: block }} {css}"
            )));
            let root_id = doc.root_node().id;
            let ids = {
                let mut m = doc.mutate();
                let html = m.create_element(qual_name!("html"), vec![]);
                m.append_children(root_id, &[html]);
                build(&mut m)
            };
            doc.set_focus_to(ids[focus]);
            (doc, ids)
        };

        let (mut incremental, ids) = new_document();
        incremental.resolve(0.0);
        let before = snapshot(&incremental);
        mutate(&mut incremental.mutate(), &ids);
        incremental.resolve(0.0);
        let after = snapshot(&incremental);

        let (mut fresh, ids) = new_document();
        mutate(&mut fresh.mutate(), &ids);
        fresh.resolve(0.0);
        assert_matches_fresh(&after, &snapshot(&fresh));
        assert_ne!(
            before, after,
            "the mutation should change the layout or styles"
        );
    }

    /// `html > [container > [sibling, holder > [input]], other]`, ids in that order.
    fn focus_tree(m: &mut DocumentMutator<'_>) -> Vec<NodeId> {
        let root_id = m.doc.root_element().id;
        let container = div(m, &[]);
        let sibling = div(m, &[]);
        let holder = div(m, &[]);
        let input = m.create_element(qual_name!("input"), vec![]);
        let other = div(m, &[]);
        m.append_children(holder, &[input]);
        m.append_children(container, &[sibling, holder]);
        m.append_children(root_id, &[container, other]);
        vec![container, sibling, holder, input, other]
    }

    #[test]
    fn removing_the_focused_subtree_restyles_focus_within_ancestors() {
        check_incremental_restyle_with_focus(
            "div { height: 10px; width: 100px } div:focus-within { height: 30px }",
            3,
            focus_tree,
            |m, ids| {
                m.remove_and_drop_node(ids[0]);
            },
        );
    }

    #[test]
    fn moving_the_focused_element_restyles_focus_within_ancestors() {
        check_incremental_restyle_with_focus(
            "div { height: 10px; width: 100px } div:focus-within { height: 30px }",
            3,
            focus_tree,
            |m, ids| m.append_children(ids[4], &[ids[3]]),
        );
    }

    /// `html > parent > [" ", child, child, " "]`, ids `[parent, child, child]`.
    fn parent_with_text_and_children(m: &mut DocumentMutator<'_>, count: usize) -> Vec<NodeId> {
        let root_id = m.doc.root_element().id;
        let parent = div(m, &[]);
        let leading = m.create_text_node(" ");
        let children: Vec<NodeId> = (0..count).map(|_| div(m, &[])).collect();
        let trailing = m.create_text_node(" ");
        m.append_children(parent, &[leading]);
        m.append_children(parent, &children);
        m.append_children(parent, &[trailing]);
        m.append_children(root_id, &[parent]);
        std::iter::once(parent).chain(children).collect()
    }

    #[test]
    fn inserting_before_the_first_element_child_restyles_it_across_text() {
        check_incremental_restyle(
            "div > div:first-child { width: 200px }",
            |m| parent_with_text_and_children(m, 2),
            |m, ids| {
                let new = div(m, &[]);
                m.insert_nodes_before(ids[1], &[new]);
            },
        );
    }

    #[test]
    fn inserting_after_the_last_element_child_restyles_it_across_text() {
        check_incremental_restyle(
            "div > div:last-child { width: 200px }",
            |m| parent_with_text_and_children(m, 2),
            |m, ids| {
                let new = div(m, &[]);
                m.insert_nodes_after(ids[2], &[new]);
            },
        );
    }

    #[test]
    fn removing_the_first_element_child_restyles_the_next_across_text() {
        check_incremental_restyle(
            "div > div:first-child { width: 200px }",
            |m| parent_with_text_and_children(m, 2),
            |m, ids| m.remove_node(ids[1]),
        );
    }

    #[test]
    fn removing_the_last_element_child_restyles_the_previous_across_text() {
        check_incremental_restyle(
            "div > div:last-child { width: 200px }",
            |m| parent_with_text_and_children(m, 2),
            |m, ids| m.remove_node(ids[2]),
        );
    }

    #[test]
    fn removing_the_first_child_restyles_nth_child_siblings() {
        check_incremental_restyle(
            "div > div:nth-child(2) { width: 200px }",
            |m| parent_with_children(m, 3),
            |m, ids| m.remove_node(ids[1]),
        );
    }

    #[test]
    fn removing_children_front_to_back_restyles_nth_child_siblings() {
        check_incremental_restyle(
            "div > div:nth-child(2) { width: 200px } div + div { color: red }",
            |m| parent_with_text_and_children(m, 4),
            |m, ids| {
                m.remove_node(ids[1]);
                m.remove_node(ids[2]);
            },
        );
    }

    #[test]
    fn inserting_at_the_front_restyles_adjacent_sibling_rules() {
        check_incremental_restyle(
            "div + div { color: red } div > div:nth-child(2) { width: 200px }",
            |m| parent_with_text_and_children(m, 3),
            |m, ids| {
                let new = div(m, &[("class", "a")]);
                m.insert_nodes_before(ids[1], &[new]);
            },
        );
    }
}

/// pyro: child-list restyle hints are narrowed to what selector
/// flags say can change (a move within one parent keeps its ancestors; only
/// the neighbour whose edge status flipped is restyled for
/// `:first-child`/`:last-child`; text children alone restyle nothing). The
/// narrowed hints must still give every element the style a freshly built
/// document gives it.
#[cfg(test)]
mod child_list_restyle_tests {
    use crate::{Attribute, BaseDocument, DocumentConfig, NodeId, local_name, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    struct Doc {
        doc: BaseDocument,
        body: NodeId,
        divs: Vec<NodeId>,
    }

    /// `<html><head><style>{css}</style></head><body>` with a `div` per
    /// class, resolved. `tag` keeps the sheet text apart from other tests'.
    fn make_doc(css: &str, tag: &str, classes: &[&str]) -> Doc {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html"), vec![]);
        let head = m.create_element(qual_name!("head"), vec![]);
        let style = m.create_element(qual_name!("style"), vec![]);
        let text = m.create_text_node(&format!("/* {tag} */ {css}"));
        m.append_children(style, &[text]);
        m.append_children(head, &[style]);
        let body = m.create_element(qual_name!("body"), vec![]);
        let mut divs = Vec::new();
        for class in classes {
            let attr = Attribute { name: qual_name!("class"), value: class.to_string() };
            let el = m.create_element(qual_name!("div"), vec![attr]);
            let inner = m.create_element(qual_name!("span"), vec![]);
            m.append_children(el, &[inner]);
            m.append_children(body, &[el]);
            divs.push(el);
        }
        m.append_children(html, &[head, body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        Doc { doc, body, divs }
    }

    /// Already-styled elements the next traversal will restyle.
    fn restyles(doc: &BaseDocument) -> Vec<NodeId> {
        doc.nodes
            .iter()
            .filter(|(_, n)| n.is_element() && n.primary_styles().is_some())
            .filter(|(_, n)| {
                n.try_stylo_element_data()
                    .and_then(|s| s.get())
                    .is_some_and(|d| !d.hint.is_empty())
            })
            .map(|(_, n)| n.id)
            .collect()
    }

    /// The (`color`, `width`) of each body element child and of its `span`,
    /// in tree order.
    fn styles(doc: &BaseDocument, body: NodeId) -> Vec<String> {
        let mut out = Vec::new();
        for &id in &doc.nodes[body].children {
            let node = &doc.nodes[id];
            if !node.is_element() {
                continue;
            }
            for id in std::iter::once(id).chain(node.children.iter().copied()) {
                let style = doc.nodes[id].primary_styles().expect("styled");
                out.push(format!(
                    "{:?} {:?}",
                    style.get_inherited_text().color,
                    style.get_position().width
                ));
            }
        }
        out
    }

    /// The styles a fresh document with the same body children (by class,
    /// in order) gets.
    fn fresh_styles(d: &Doc, css: &str, tag: &str) -> Vec<String> {
        let classes: Vec<String> = d.doc.nodes[d.body]
            .children
            .iter()
            .filter_map(|&id| d.doc.nodes[id].element_data())
            .map(|e| e.attr(local_name!("class")).unwrap_or_default().to_string())
            .collect();
        let classes: Vec<&str> = classes.iter().map(String::as_str).collect();
        let fresh = make_doc(css, tag, &classes);
        styles(&fresh.doc, fresh.body)
    }

    const SIBLING_CSS: &[(&str, &str)] = &[
        ("first", "div:first-child { color: red } div:first-child span { width: 7px }"),
        ("last", "div:last-child { color: red } div:last-child span { width: 7px }"),
        ("nth", "div:nth-child(2n) { color: red } div:nth-child(3) span { width: 7px }"),
        ("next", ".a + div { color: red } .b + div span { width: 7px }"),
        ("later", ".a ~ .b { color: red } .c ~ div span { width: 7px }"),
        ("only", "div:only-child { color: red } div:nth-last-child(2) span { width: 7px }"),
    ];

    /// Each step mutates the body's children; after every step the styles
    /// equal a fresh document's.
    #[test]
    fn narrowed_hints_match_a_fresh_document() {
        type Step = fn(&mut Doc);
        let steps: &[(&str, Step)] = &[
            ("move last to front", |d| {
                let (first, last) = (d.divs[0], *d.divs.last().unwrap());
                d.doc.mutate().insert_nodes_before(first, &[last]);
            }),
            ("move front to end", |d| {
                let first = d.doc.nodes[d.body].children[0];
                d.doc.mutate().append_children(d.body, &[first]);
            }),
            ("move middle forward", |d| {
                let kids = d.doc.nodes[d.body].children.clone();
                d.doc.mutate().insert_nodes_before(kids[0], &[kids[2]]);
            }),
            ("text at both ends", |d| {
                let first = d.doc.nodes[d.body].children[0];
                let mut m = d.doc.mutate();
                let t1 = m.create_text_node("x");
                let t2 = m.create_text_node("y");
                m.insert_nodes_before(first, &[t1]);
                m.append_children(d.body, &[t2]);
            }),
            ("remove first element", |d| {
                let first = *d.doc.nodes[d.body]
                    .children
                    .iter()
                    .find(|&&id| d.doc.nodes[id].is_element())
                    .unwrap();
                d.doc.mutate().remove_node(first);
            }),
            ("append new", |d| {
                let mut m = d.doc.mutate();
                let attr = Attribute { name: qual_name!("class"), value: "a".into() };
                let el = m.create_element(qual_name!("div"), vec![attr]);
                let inner = m.create_element(qual_name!("span"), vec![]);
                m.append_children(el, &[inner]);
                m.append_children(d.body, &[el]);
            }),
            ("remove last element", |d| {
                let last = *d.doc.nodes[d.body]
                    .children
                    .iter()
                    .rev()
                    .find(|&&id| d.doc.nodes[id].is_element())
                    .unwrap();
                d.doc.mutate().remove_node(last);
            }),
            ("swap the remaining", |d| {
                let els: Vec<NodeId> = d.doc.nodes[d.body]
                    .children
                    .iter()
                    .copied()
                    .filter(|&id| d.doc.nodes[id].is_element())
                    .collect();
                d.doc.mutate().insert_nodes_before(els[0], &[els[els.len() - 1]]);
            }),
            ("down to one", |d| {
                let els: Vec<NodeId> = d.doc.nodes[d.body]
                    .children
                    .iter()
                    .copied()
                    .filter(|&id| d.doc.nodes[id].is_element())
                    .collect();
                let mut m = d.doc.mutate();
                for &id in &els[1..] {
                    m.remove_node(id);
                }
            }),
        ];
        for (tag, css) in SIBLING_CSS {
            let tag = format!("child-list-{tag}");
            let mut d = make_doc(css, &tag, &["a", "b", "c", "a", "b"]);
            assert_eq!(styles(&d.doc, d.body), fresh_styles(&d, css, &tag), "{tag}: initial");
            for (name, step) in steps {
                step(&mut d);
                d.doc.resolve(0.0);
                assert_eq!(styles(&d.doc, d.body), fresh_styles(&d, css, &tag), "{tag}: {name}");
            }
        }
    }

    /// A move within one parent restyles nothing when no sibling-dependent
    /// selector was tested on its children.
    #[test]
    fn same_parent_move_without_sibling_selectors_restyles_nothing() {
        let d = &mut make_doc(".a { color: red } .b span { width: 7px }", "child-list-plain", &["a", "b", "c"]);
        d.doc.mutate().insert_nodes_before(d.divs[0], &[d.divs[2]]);
        assert_eq!(restyles(&d.doc), Vec::<NodeId>::new());
        d.doc.mutate().append_children(d.body, &[d.divs[1]]);
        assert_eq!(restyles(&d.doc), Vec::<NodeId>::new());
        d.doc.resolve(0.0);
        let order: Vec<NodeId> = d.doc.nodes[d.body].children.to_vec();
        assert_eq!(order, vec![d.divs[2], d.divs[0], d.divs[1]]);
    }

    /// A move to another parent still restyles the moved subtree.
    #[test]
    fn move_to_another_parent_restyles_the_moved_element() {
        let d = &mut make_doc(".a .b { color: red }", "child-list-reparent", &["a", "b"]);
        d.doc.mutate().append_children(d.divs[0], &[d.divs[1]]);
        assert_eq!(restyles(&d.doc), vec![d.divs[1]]);
    }

    /// Text children coming and going move no element's index or edge
    /// status.
    #[test]
    fn text_children_alone_restyle_nothing() {
        let css = "div:first-child, div:last-child { color: red } div:nth-child(2) { width: 3px } .a + div, .a ~ div { width: 5px }";
        let d = &mut make_doc(css, "child-list-text", &["a", "b", "c"]);
        let mut m = d.doc.mutate();
        let t1 = m.create_text_node("x");
        let t2 = m.create_text_node("y");
        m.insert_nodes_before(d.divs[0], &[t1]);
        m.append_children(d.body, &[t2]);
        m.remove_node(t1);
        drop(m);
        assert_eq!(restyles(&d.doc), Vec::<NodeId>::new());
    }

    /// `:first-child`/`:last-child` restyle only the element whose edge
    /// status flipped: appending restyles the old last element, not the
    /// first one.
    #[test]
    fn edge_selectors_restyle_only_the_flipped_neighbour() {
        let css = "div:first-child { color: red } div:last-child { color: blue }";
        let d = &mut make_doc(css, "child-list-edge", &["a", "b", "c"]);
        let mut m = d.doc.mutate();
        let el = m.create_element(qual_name!("div"), vec![]);
        m.append_children(d.body, &[el]);
        drop(m);
        assert_eq!(restyles(&d.doc), vec![d.divs[2]]);
        d.doc.resolve(0.0);

        d.doc.mutate().remove_node(d.divs[1]);
        assert_eq!(restyles(&d.doc), Vec::<NodeId>::new(), "a middle element left");
        d.doc.mutate().remove_node(d.divs[0]);
        assert_eq!(restyles(&d.doc), vec![d.divs[2]], "the next one became first");
    }
}
