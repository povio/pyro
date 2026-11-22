//! Custom element callback reactions: the tree records which custom
//! elements were connected, disconnected, moved (`moveBefore`) or adopted,
//! which observed attributes changed and which form-associated custom
//! elements changed form owner or disabled state, per document whose
//! window created the element, and the bindings run the callbacks when the
//! DOM call returns.
//!
//! What costs a page that uses none of it: an insertion or removal tests
//! one flag (`any_custom`); an attribute change tests one element flag
//! (`OBSERVES_ATTRIBUTES`); the main document parser tests whether any name
//! is defined at all before it looks one up.

use std::rc::Rc;

use markup5ever::{LocalName, Namespace, QualName};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::error::{DomError, DomResult};
use crate::node::{element_flags, NodeId, NIL};

/// A queued callback reaction of a custom element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustomReaction {
    /// `connectedCallback`.
    Connected,
    /// `disconnectedCallback`.
    Disconnected,
    /// `connectedMoveCallback`, which falls back to `disconnectedCallback`
    /// then `connectedCallback` when the definition has none.
    Moved,
    /// `adoptedCallback(oldDocument, newDocument)`.
    Adopted { old: NodeId, new: NodeId },
    /// `attributeChangedCallback(name, oldValue, newValue, namespace)`, for
    /// an attribute the definition observes.
    AttributeChanged(Box<AttributeChange>),
    /// `formAssociatedCallback(form)`: the form owner changed.
    FormAssociated(Option<NodeId>),
    /// `formResetCallback()`: the form owner was reset.
    FormReset,
    /// `formDisabledCallback(disabled)`: the disabled state changed.
    FormDisabled(bool),
}

/// The arguments of an `attributeChangedCallback` reaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeChange {
    pub local: LocalName,
    pub ns: Namespace,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Outcome of the HTML parser's "create an element for a token" for a name
/// a window defined ([`ParserCustomElements::construct`]).
pub enum ParserConstruct {
    /// No definition (any more): the parser gives the element its
    /// attributes itself.
    NotDefined,
    /// The constructor ran on the element, which got the token's attributes.
    Constructed,
    /// The constructor threw or returned another object (reported): the
    /// element is "failed", never upgraded; the parser gives it its
    /// attributes.
    Failed,
}

/// The bindings' side of custom elements in the main document parser
/// (HTML "create an element for a token" with the synchronous custom
/// elements flag, and the element queue the parser's insertions push).
pub trait ParserCustomElements {
    /// Runs the constructor of `element`'s definition now, on `element`
    /// (made by the parser for a token, not inserted yet, without
    /// attributes); then appends the token's `attrs`
    /// ([`Dom::append_parser_attributes`]) and runs the reactions that
    /// queued.
    fn construct(&self, element: NodeId, attrs: &[(QualName, String)]) -> ParserConstruct;
    /// Runs the reactions and upgrades the parser's insertion queued.
    fn run_reactions(&self);
}

#[derive(Default)]
pub(crate) struct ReactionQueue {
    /// Some element was ever marked custom: until then no mutation walks a
    /// subtree looking for one.
    pub(crate) any_custom: bool,
    /// Reactions in enqueue order, per document of the window that runs them.
    queued: FxHashMap<NodeId, Vec<(NodeId, CustomReaction)>>,
    /// The attribute local names definitions observe, by element local
    /// name (all realms' definitions of a name together).
    observed: FxHashMap<LocalName, Vec<LocalName>>,
    /// The element names some window defined (the parser constructs those).
    defined: FxHashSet<LocalName>,
    /// Qualified prefilter for creation reactions. A definition in another
    /// registry does not enqueue an upgrade on a freshly parsed element.
    registry_definitions: FxHashSet<(Option<NodeId>, crate::RegistryAssociation, LocalName, LocalName)>,
    /// Installed by the bindings of a window with its first definition,
    /// by the window's document.
    parser: FxHashMap<NodeId, Rc<dyn ParserCustomElements>>,
}

impl Dom {
    /// `root` and its shadow-including descendants, in shadow-including tree
    /// order (an element's shadow tree comes before its children).
    pub(crate) fn shadow_including_subtree(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            out.push(i);
            let mut child = self.n(i).last_child;
            while child != NIL {
                stack.push(child);
                child = self.n(child).prev;
            }
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
        }
        out
    }

    fn is_custom_idx(&self, i: u32) -> bool {
        self.n(i).element().is_some_and(|e| e.flags & element_flags::CUSTOM_DEFINED != 0)
    }

    /// The document whose window runs `i`'s reactions: the one its realm
    /// belongs to (its definition is that window's).
    fn reaction_document(&self, i: u32) -> NodeId {
        let id = self.id_of(i);
        self.creation_document(id).unwrap_or(id)
    }

    pub(crate) fn push_custom_reaction(&mut self, i: u32, reaction: CustomReaction) {
        let doc = self.reaction_document(i);
        let node = self.id_of(i);
        self.reactions.queued.entry(doc).or_default().push((node, reaction));
        self.touch_hook();
    }

    /// Enqueues `reaction` for every custom element among `root` and its
    /// shadow-including descendants, in shadow-including tree order.
    pub(crate) fn enqueue_custom_reactions(&mut self, root: u32, reaction: CustomReaction) {
        if !self.reactions.any_custom {
            return;
        }
        // A leaf (most parser insertions) needs no walk.
        let leaf = self.n(root).first_child == NIL && self.n(root).element().is_none_or(|e| e.shadow_root == NIL);
        if leaf {
            if self.is_custom_idx(root) {
                self.push_custom_reaction(root, reaction);
            }
            return;
        }
        for i in self.shadow_including_subtree(root) {
            if self.is_custom_idx(i) {
                self.push_custom_reaction(i, reaction.clone());
            }
        }
    }

    /// `adoptedCallback` for the custom elements among `root` and its
    /// shadow-including descendants, which moved from `old` to `new`.
    pub(crate) fn enqueue_adopted(&mut self, root: u32, old: u32, new: u32) {
        let (old, new) = (self.id_of(old), self.id_of(new));
        self.enqueue_custom_reactions(root, CustomReaction::Adopted { old, new });
    }

    /// DOM "handle attribute changes", the custom element part: an
    /// `attributeChangedCallback` reaction when `i`'s definition observes
    /// `name`. Inlined so the common case is one flag test.
    #[inline]
    pub(crate) fn attribute_changed(&mut self, i: u32, name: &QualName, old: Option<&str>, new: Option<&str>) {
        if !self.inline_transitions_seen
            && name.local == markup5ever::local_name!("style")
            && new.is_some_and(|v| v.contains("transition"))
        {
            self.inline_transitions_seen = true;
        }
        if name.ns == markup5ever::ns!() && &*name.local == "popover" {
            self.popover_attribute_changed(i, old, new);
        }
        let observes = self.n(i).element().is_some_and(|e| e.flags & element_flags::OBSERVES_ATTRIBUTES != 0);
        if observes {
            self.observed_attribute_changed(i, name, old, new);
        }
    }

    #[cold]
    fn observed_attribute_changed(&mut self, i: u32, name: &QualName, old: Option<&str>, new: Option<&str>) {
        let e = self.n(i).element().expect("element");
        let observes = |key: &LocalName| self.reactions.observed.get(key).is_some_and(|names| names.contains(&name.local));
        let observed = observes(&e.name.local) || e.is_value.as_ref().is_some_and(observes);
        if !observed {
            return;
        }
        let change = AttributeChange {
            local: name.local.clone(),
            ns: name.ns.clone(),
            old: old.map(str::to_owned),
            new: new.map(str::to_owned),
        };
        self.push_custom_reaction(i, CustomReaction::AttributeChanged(Box::new(change)));
    }

    /// A window defined `name`, observing `observed` (attribute local
    /// names; empty without an `attributeChangedCallback`). The main
    /// document parser constructs elements of this name from now on.
    pub fn note_custom_definition(&mut self, name: &LocalName, observed: Vec<LocalName>) {
        self.reactions.defined.insert(name.clone());
        if !observed.is_empty() {
            let names = self.reactions.observed.entry(name.clone()).or_default();
            for o in observed {
                if !names.contains(&o) {
                    names.push(o);
                }
            }
        }
    }

    /// Installs (or with `None` removes) the bindings' side of the parser's
    /// custom elements for `document`, the document of their window.
    pub fn set_parser_custom_elements(&mut self, document: NodeId, hooks: Option<Rc<dyn ParserCustomElements>>) {
        // Documents collected since leave their window's hooks behind.
        let mut parser = std::mem::take(&mut self.reactions.parser);
        parser.retain(|doc, _| *doc != document && self.check(*doc).is_ok());
        if let Some(hooks) = hooks {
            parser.insert(document, hooks);
        }
        self.reactions.parser = parser;
    }

    pub fn note_registry_definition(&mut self, doc: NodeId, registry: crate::RegistryAssociation,
        name: &LocalName, local: &LocalName, observed: Vec<LocalName>) {
        self.note_custom_definition(name, observed);
        let owner = (registry == crate::RegistryAssociation::Default).then_some(doc);
        self.reactions.registry_definitions.insert((owner, registry, name.clone(), local.clone()));
    }

    pub(crate) fn fragment_definition_name(&self, i: u32) -> Option<LocalName> {
        let registry = self.registry_idx(i);
        if registry == crate::RegistryAssociation::Null { return None; }
        let owner = (registry == crate::RegistryAssociation::Default).then(|| self.id_of(self.n(i).owner_doc));
        let el = self.n(i).element().unwrap();
        let local = el.local_name();
        let found = |name: &LocalName| self.reactions.registry_definitions.contains(&(owner, registry, name.clone(), local.clone()));
        if found(local) { Some(local.clone()) } else { el.is_value.as_ref().filter(|is| found(is)).cloned() }
    }

    pub(crate) fn forget_registry_definitions(&mut self, registry: crate::RegistryAssociation, document: Option<NodeId>) {
        self.reactions.registry_definitions.retain(|(owner, r, _, _)| *r != registry || *owner != document);
    }

    /// The parser hooks of `document`, when an HTML element named `name`
    /// (with the is value `is`) may be defined.
    pub(crate) fn parser_custom_elements(
        &self,
        document: u32,
        name: &QualName,
        is: Option<&LocalName>,
    ) -> Option<Rc<dyn ParserCustomElements>> {
        if self.reactions.defined.is_empty()
            || name.ns != markup5ever::ns!(html)
            || !(self.reactions.defined.contains(&name.local) || is.is_some_and(|is| self.reactions.defined.contains(is)))
        {
            return None;
        }
        self.reactions.parser.get(&self.id_of(document)).cloned()
    }

    /// Whether custom element reactions are queued (for any document): the
    /// parser's test, under the borrow of its insertion, for whether to
    /// look for its document's.
    #[inline(always)]
    pub(crate) fn custom_reactions_queued(&self) -> bool {
        !self.reactions.queued.is_empty()
    }

    /// The parser hooks of `document`, when its insertion queued reactions.
    pub(crate) fn parser_reactions_pending(&self, document: u32) -> Option<Rc<dyn ParserCustomElements>> {
        if self.reactions.queued.is_empty() {
            return None;
        }
        // Only this document's window runs them here: another document's
        // queue (one whose window is gone, say) costs no call per insertion.
        let document = self.id_of(document);
        if !self.reactions.queued.contains_key(&document) {
            return None;
        }
        self.reactions.parser.get(&document).cloned()
    }

    /// The parser hooks of the active `document`, when what its XML parser
    /// inserted up to a script left them work: reactions queued for the
    /// document, or elements waiting for an upgrade attempt.
    pub(crate) fn parser_custom_work(&self, document: u32) -> Option<Rc<dyn ParserCustomElements>> {
        if self.reactions.parser.is_empty()
            || !matches!(&self.n(document).kind, crate::node::NodeKind::Document(data) if data.active)
        {
            return None;
        }
        let document = self.id_of(document);
        if !self.has_upgrade_candidates() && !self.reactions.queued.contains_key(&document) {
            return None;
        }
        self.reactions.parser.get(&document).cloned()
    }

    /// Sets the flags of an element about to be upgraded or constructed
    /// with a definition: whether it observes attributes and is
    /// form-associated.
    pub fn prepare_custom_element(&mut self, el: NodeId, observes: bool, form_associated: bool) -> DomResult<()> {
        let i = self.check(el)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        if observes {
            e.flags |= element_flags::OBSERVES_ATTRIBUTES;
        }
        if form_associated {
            e.flags |= element_flags::FORM_ASSOCIATED;
        }
        Ok(())
    }

    /// Custom element state "failed" (its constructor threw): never
    /// upgraded again, and the reactions queued for it are dropped.
    pub fn set_custom_failed(&mut self, el: NodeId) -> DomResult<()> {
        let i = self.check(el)?;
        let e = self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?;
        e.flags &= !(element_flags::OBSERVES_ATTRIBUTES | element_flags::FORM_ASSOCIATED);
        e.flags |= element_flags::CUSTOM_FAILED;
        for list in self.reactions.queued.values_mut() {
            list.retain(|(n, _)| *n != el);
        }
        self.reactions.queued.retain(|_, list| !list.is_empty());
        self.forget_form_associated(i);
        Ok(())
    }

    /// Whether the parser of `document` may construct an element named
    /// `name` with the is value `is` (a window defined that name, or the
    /// customized built-in `is` names, and gave its hooks).
    #[inline]
    pub(crate) fn parser_may_construct(&self, document: u32, name: &QualName, is: Option<&str>) -> bool {
        !self.reactions.defined.is_empty()
            && name.ns == markup5ever::ns!(html)
            && (self.reactions.defined.contains(&name.local)
                || is.is_some_and(|is| self.reactions.defined.contains(&LocalName::from(is))))
            && self.reactions.parser.contains_key(&self.id_of(document))
    }

    /// DOM "is value" of `el` (`None` for an autonomous or uncustomized element).
    pub fn is_value(&self, el: NodeId) -> Option<LocalName> {
        self.check(el).ok().and_then(|i| self.n(i).element()?.is_value.clone())
    }

    /// Sets `el`'s is value, as `createElement(name, {is})` does when it creates it.
    pub fn set_is_value(&mut self, el: NodeId, is: Option<LocalName>) -> DomResult<()> {
        let i = self.check(el)?;
        self.n_mut(i).element_mut().ok_or(DomError::Type("not an element"))?.is_value = is;
        Ok(())
    }

    /// The token's attributes of a parser-made element that was not
    /// constructed, set as the parser sets any element's (no records).
    pub(crate) fn set_parsed_attributes(&mut self, i: u32, attrs: Vec<(QualName, String)>) {
        if attrs.is_empty() {
            return;
        }
        let Some(e) = self.n_mut(i).element_mut() else { return };
        let mut all: Vec<crate::node::Attr> = e.attrs.to_vec();
        for (name, value) in attrs {
            if !all.iter().any(|a| a.name == name) {
                all.push(crate::node::Attr { name, value: value.as_str().into() });
            }
        }
        e.set_attrs(all);
        self.resummarize(i);
        self.note_inline_handlers(i);
    }

    /// Appends the attributes of the token the parser constructed `el` for,
    /// as DOM "append an attribute" does (mutation records and
    /// `attributeChangedCallback` reactions).
    pub fn append_parser_attributes(&mut self, el: NodeId, attrs: &[(QualName, String)]) -> DomResult<()> {
        let i = self.check(el)?;
        for (name, value) in attrs {
            if self.n(i).element().is_some_and(|e| e.attrs.iter().any(|a| a.name == *name)) {
                continue;
            }
            self.append_attr(i, name.clone(), value);
        }
        Ok(())
    }

    /// Whether any document has queued custom element reactions.
    pub fn has_custom_reactions(&self) -> bool {
        !self.reactions.queued.is_empty()
    }

    /// Whether `document` has queued custom element reactions.
    pub fn has_custom_reactions_for(&self, document: NodeId) -> bool {
        self.reactions.queued.contains_key(&document)
    }

    /// The reactions queued for elements of `document`, in order; cleared.
    /// An element may have been collected since: check it before use.
    pub fn take_custom_reactions(&mut self, document: NodeId) -> Vec<(NodeId, CustomReaction)> {
        self.reactions.queued.remove(&document).unwrap_or_default()
    }
}
