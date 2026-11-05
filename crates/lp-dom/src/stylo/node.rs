//! Stylo's DOM traits (`TNode`, `TElement`, `TDocument`, `TShadowRoot`) and
//! the `selectors` element trait on lp-dom nodes, after blitz-dom's
//! implementation (MIT OR Apache-2.0).
//!
//! A [`SNode`] is a node index plus the styling context of one flush: the
//! `Dom`, the per-element style data ([`Store`]) and the stylesheet lock. The
//! traversal is sequential, so the store hands out its data through
//! `UnsafeCell`s the way blitz-dom's nodes do.

use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ptr::NonNull;

use markup5ever::{local_name, ns, LocalName, LocalNameStaticSet, Namespace, NamespaceStaticSet};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::{BloomFilter, BLOOM_HASH_MASK};
use selectors::matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode};
use selectors::sink::Push;
use selectors::OpaqueElement;
use style::applicable_declarations::ApplicableDeclarationBlock;
use style::bloom::each_relevant_element_hash;
use style::context::{QuirksMode, SharedStyleContext};
use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::selector_parser::{NonTSPseudoClass, PseudoElement, RestyleDamage, SelectorImpl};
use style::servo_arc::{Arc, ArcBorrow};
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::scope_rule::ImplicitScopeRoot;
use style::values::{AtomIdent, AtomString, GenericAtomIdent};
use style::Atom;
use style_dom::ElementState;

use rustc_hash::FxHashMap;

use crate::dom::Dom;
use crate::node::{NodeKind, NIL};
use crate::select::PseudoClass;

/// A node generation and its parsed `style` attribute.
type StyleAttrSlot = (u32, Option<ParsedStyleAttr>);

/// A `style` attribute's text and its parse.
type ParsedStyleAttr = (Box<str>, Arc<Locked<PropertyDeclarationBlock>>);

/// Style data of one element, created when the element is first styled.
pub(crate) struct Slot {
    /// Generation of the node it belongs to (a freed slot's index is reused).
    pub(crate) gen: u32,
    pub(crate) data: ElementDataWrapper,
    dirty_descendants: Cell<bool>,
    /// An invalidation snapshot of this element is in the cascade's map.
    pub(crate) has_snapshot: Cell<bool>,
    pub(crate) handled_snapshot: Cell<bool>,
}

impl Slot {
    fn new(gen: u32) -> Box<Slot> {
        Box::new(Slot {
            gen,
            data: ElementDataWrapper::default(),
            dirty_descendants: Cell::new(false),
            has_snapshot: Cell::new(false),
            handled_snapshot: Cell::new(false),
        })
    }
}

/// Style data of every styled element, by node index. Sized to the DOM's
/// slots before a traversal, so `Box<Slot>` addresses stay put while the
/// traversal holds references into them.
#[derive(Default)]
pub(crate) struct Store {
    slots: UnsafeCell<Vec<Option<Box<Slot>>>>,
    /// Selector flags by node index, with the node's generation: set while
    /// matching, also on elements not styled yet (a `:has()` search visits
    /// descendants before the traversal styles them).
    flags: Vec<Cell<(u32, u32)>>,
    /// `style` attributes as parsed, with the text they were parsed from,
    /// by node index and generation (boxed: references stay put). Apart
    /// from the slots: lazily resolved elements have none.
    style_attrs: UnsafeCell<FxHashMap<u32, Box<StyleAttrSlot>>>,
}

impl Store {
    /// The selector flags of `idx` as the node of generation `gen`.
    pub(crate) fn flags(&self, idx: u32, gen: u32) -> ElementSelectorFlags {
        match self.flags.get(idx as usize).map(Cell::get) {
            Some((g, bits)) if g == gen => ElementSelectorFlags::from_bits_truncate(bits as usize),
            _ => ElementSelectorFlags::empty(),
        }
    }

    fn add_flags(&self, idx: u32, gen: u32, flags: ElementSelectorFlags) {
        if let Some(cell) = self.flags.get(idx as usize) {
            let old = self.flags(idx, gen);
            cell.set((gen, (old | flags).bits() as u32));
        }
    }

    /// The data of `idx` if it was styled as the node of generation `gen`.
    pub(crate) fn get(&self, idx: u32, gen: u32) -> Option<&Slot> {
        // SAFETY: slots are only replaced through `ensure` (single-threaded
        // traversal) and `&mut self` methods.
        let slots = unsafe { &*self.slots.get() };
        slots.get(idx as usize)?.as_deref().filter(|s| s.gen == gen)
    }

    /// # Safety
    /// No reference into the slot of `idx` may be alive (the traversal
    /// creates an element's data before anything borrows it).
    unsafe fn ensure(&self, idx: u32, gen: u32) -> &Slot {
        // SAFETY: see above; `reserve` sized the vector, so no reallocation.
        let slots = unsafe { &mut *self.slots.get() };
        if slots.len() <= idx as usize {
            slots.resize_with(idx as usize + 1, || None);
        }
        let slot = &mut slots[idx as usize];
        if slot.as_ref().is_none_or(|s| s.gen != gen) {
            *slot = Some(Slot::new(gen));
        }
        slot.as_deref().expect("slot")
    }

    /// The parsed `style` attribute of `idx` as the node of generation `gen`.
    ///
    /// # Safety
    /// Only the element's own style computation may hold the reference
    /// (styling is sequential).
    #[allow(clippy::mut_from_ref)]
    unsafe fn style_attr(&self, idx: u32, gen: u32) -> &mut Option<ParsedStyleAttr> {
        // SAFETY: see above; entries are boxed, so other insertions do not move them.
        let map = unsafe { &mut *self.style_attrs.get() };
        let entry = map.entry(idx).or_insert_with(|| Box::new((gen, None)));
        if entry.0 != gen {
            **entry = (gen, None);
        }
        &mut entry.1
    }
}

/// What every node handle of one styling pass shares.
pub(crate) struct Cx<'a> {
    pub(crate) dom: &'a Dom,
    pub(crate) store: &'a Store,
    pub(crate) lock: &'a SharedRwLock,
    pub(crate) quirks: QuirksMode,
    /// Open popovers (`:popover-open`; the style engine tracks them).
    pub(crate) popovers: &'a rustc_hash::FxHashSet<u32>,
    /// Ids and class lists as Stylo atoms, interned once per element and
    /// pass (boxed: references into them outlive later insertions).
    ids: UnsafeCell<FxHashMap<u32, Box<Option<Atom>>>>,
    classes: UnsafeCell<FxHashMap<u32, Box<[Atom]>>>,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(
        dom: &'a Dom,
        store: &'a Store,
        lock: &'a SharedRwLock,
        quirks: QuirksMode,
        popovers: &'a rustc_hash::FxHashSet<u32>,
    ) -> Self {
        Cx { dom, store, lock, quirks, popovers, ids: UnsafeCell::default(), classes: UnsafeCell::default() }
    }
    fn id_atom(&self, idx: u32) -> Option<&Atom> {
        // SAFETY: entries are boxed and never removed during the pass.
        let ids = unsafe { &mut *self.ids.get() };
        let entry =
            ids.entry(idx).or_insert_with(|| Box::new(self.dom.n(idx).element().and_then(|e| e.id()).map(Atom::from)));
        let ptr: *const Option<Atom> = &**entry;
        // SAFETY: see above.
        unsafe { &*ptr }.as_ref()
    }
    fn class_atoms(&self, idx: u32) -> &[Atom] {
        // SAFETY: entries are boxed and never removed during the pass.
        let classes = unsafe { &mut *self.classes.get() };
        let entry = classes.entry(idx).or_insert_with(|| {
            let attr = self.dom.n(idx).element().and_then(|e| e.attr("class")).unwrap_or("");
            attr.split_ascii_whitespace().map(Atom::from).collect()
        });
        let ptr: *const [Atom] = &**entry;
        // SAFETY: see above.
        unsafe { &*ptr }
    }
}

/// Elements' class tokens as atoms, for one (DOM uid, version).
type ClassAtoms = ((u64, u64), FxHashMap<u32, Box<[Atom]>>);

thread_local! {
    static CLASS_ATOMS: std::cell::RefCell<ClassAtoms> = Default::default();
}

/// A node of the current styling pass. One word: Stylo's style-sharing
/// cache stores element handles in pointer-sized slots, so the pass context
/// is not in the handle but in [`CURRENT`], set by [`Cx::enter`].
#[derive(Clone, Copy)]
pub(crate) struct SNode<'a, C: CxSource<'a> = Tls<'a>> {
    pub(crate) idx: usize,
    src: C,
    _cx: PhantomData<&'a Cx<'a>>,
}

/// Where a node handle finds its pass context: [`Tls`] (one-word handles,
/// for the traversal) or a `&Cx` (selector matching, which would otherwise
/// read the thread-local at every step).
pub(crate) trait CxSource<'a>: Copy {
    fn get(self) -> &'a Cx<'a>;
}

/// The context [`Cx::enter`] made current on this thread.
#[derive(Clone, Copy)]
pub(crate) struct Tls<'a>(PhantomData<&'a Cx<'a>>);

impl<'a> CxSource<'a> for Tls<'a> {
    #[inline]
    fn get(self) -> &'a Cx<'a> {
        let ptr = CURRENT.with(|c| c.get());
        debug_assert!(!ptr.is_null(), "no styling pass entered");
        // SAFETY: handles exist only while their pass is entered (`Cx::enter`
        // guards every flush and style read), and the pass outlives them.
        unsafe { &*(ptr as *const Cx<'a>) }
    }
}

impl<'a> CxSource<'a> for &'a Cx<'a> {
    #[inline]
    fn get(self) -> &'a Cx<'a> {
        self
    }
}

thread_local! {
    /// The `Cx` of the styling pass running on this thread.
    static CURRENT: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

// SAFETY: the traversal is sequential (`traverse_dom` without a thread pool);
// Stylo only requires these bounds for its parallel mode.
unsafe impl<'a, C: CxSource<'a>> Send for SNode<'a, C> {}
unsafe impl<'a, C: CxSource<'a>> Sync for SNode<'a, C> {}

impl<'a, C: CxSource<'a>> PartialEq for SNode<'a, C> {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}
impl<'a, C: CxSource<'a>> Eq for SNode<'a, C> {}
impl<'a, C: CxSource<'a>> Hash for SNode<'a, C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.idx)
    }
}
impl<'a, C: CxSource<'a>> fmt::Debug for SNode<'a, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.node().kind {
            NodeKind::Element(e) => write!(f, "<{} #{}>", e.name.local, self.idx),
            _ => write!(f, "node #{}", self.idx),
        }
    }
}

impl<'a> SNode<'a> {}

impl<'a> SNode<'a, &'a Cx<'a>> {
    /// A node of `cx`, carrying it (selector matching outside the traversal).
    pub(crate) fn with_cx(cx: &'a Cx<'a>, idx: u32) -> Self {
        SNode { idx: idx as usize, src: cx, _cx: PhantomData }
    }
}

impl<'a, C: CxSource<'a>> SNode<'a, C> {
    fn at(&self, idx: u32) -> Self {
        SNode { idx: idx as usize, src: self.src, _cx: PhantomData }
    }
    #[inline]
    fn cx(&self) -> &'a Cx<'a> {
        self.src.get()
    }
    pub(crate) fn i(&self) -> u32 {
        self.idx as u32
    }
    fn with(&self, idx: u32) -> Option<Self> {
        (idx != NIL).then(|| self.at(idx))
    }
    fn node(&self) -> &'a crate::node::Node {
        self.cx().dom.n(self.i())
    }
    fn element(&self) -> &'a crate::node::ElementData {
        self.node().element().expect("element")
    }
    pub(crate) fn selector_flags(&self) -> ElementSelectorFlags {
        self.cx().store.flags(self.i(), self.cx().dom.generation_at(self.i()))
    }
    pub(crate) fn slot(&self) -> Option<&'a Slot> {
        self.cx().store.get(self.i(), self.cx().dom.generation_at(self.i()))
    }
    fn shadow_host_of_parent(&self) -> Option<u32> {
        let p = self.node().parent;
        match (p != NIL).then(|| &self.cx().dom.n(p).kind) {
            Some(NodeKind::Fragment(f)) if f.shadow.is_some() => Some(f.host),
            _ => None,
        }
    }
    fn sibling_element(&self, forward: bool) -> Option<Self> {
        let mut c = self.i();
        loop {
            let n = self.cx().dom.n(c);
            c = if forward { n.next } else { n.prev };
            if c == NIL {
                return None;
            }
            if self.cx().dom.n(c).is_element() {
                return self.with(c);
            }
        }
    }
    fn is_html_in_html_doc(&self) -> bool {
        self.element().is_html() && self.cx().dom.is_html_doc(self.node().owner_doc)
    }
    fn matches(&self, pc: PseudoClass) -> bool {
        self.cx().dom.matches_state(self.i(), &pc)
    }
}

impl<'a, C: CxSource<'a>> NodeInfo for SNode<'a, C> {
    fn is_element(&self) -> bool {
        self.node().is_element()
    }
    fn is_text_node(&self) -> bool {
        matches!(self.node().kind, NodeKind::Text(_) | NodeKind::CData(_))
    }
}

impl<'a, C: CxSource<'a>> TDocument for SNode<'a, C> {
    type ConcreteNode = SNode<'a, C>;
    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }
    fn is_html_document(&self) -> bool {
        self.cx().dom.is_html_doc(self.i())
    }
    fn quirks_mode(&self) -> QuirksMode {
        self.cx().quirks
    }
    fn shared_lock(&self) -> &SharedRwLock {
        self.cx().lock
    }
}

impl<'a, C: CxSource<'a>> TShadowRoot for SNode<'a, C> {
    type ConcreteNode = SNode<'a, C>;
    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }
    fn host(&self) -> SNode<'a, C> {
        match &self.node().kind {
            NodeKind::Fragment(f) => self.at(f.host),
            _ => unreachable!("shadow root"),
        }
    }
    /// Shadow-tree sheets are not scoped: they apply document-wide (as in
    /// the old cascade), so a shadow root has no cascade data of its own.
    fn style_data<'b>(&self) -> Option<&'b style::stylist::CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

/// The children styled under an element: a shadow host's shadow tree, then
/// its light children (inheriting from the host: slots are not modeled).
pub(crate) struct Children<'a, C: CxSource<'a>> {
    of: SNode<'a, C>,
    next: u32,
    then: u32,
}

impl<'a, C: CxSource<'a>> Iterator for Children<'a, C> {
    type Item = SNode<'a, C>;
    fn next(&mut self) -> Option<SNode<'a, C>> {
        if self.next == NIL {
            if self.then == NIL {
                return None;
            }
            self.next = std::mem::replace(&mut self.then, NIL);
        }
        let cur = self.next;
        self.next = self.of.cx().dom.n(cur).next;
        Some(self.of.at(cur))
    }
}

impl<'a, C: CxSource<'a>> TNode for SNode<'a, C> {
    type ConcreteElement = SNode<'a, C>;
    type ConcreteDocument = SNode<'a, C>;
    type ConcreteShadowRoot = SNode<'a, C>;

    fn parent_node(&self) -> Option<Self> {
        self.with(self.node().parent)
    }
    fn first_child(&self) -> Option<Self> {
        self.with(self.node().first_child)
    }
    fn last_child(&self) -> Option<Self> {
        self.with(self.node().last_child)
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.with(self.node().prev)
    }
    fn next_sibling(&self) -> Option<Self> {
        self.with(self.node().next)
    }
    fn owner_doc(&self) -> Self::ConcreteDocument {
        self.at(self.node().owner_doc)
    }
    fn is_in_document(&self) -> bool {
        matches!(self.cx().dom.n(self.cx().dom.root_idx(self.i())).kind, NodeKind::Document(_))
    }
    fn traversal_parent(&self) -> Option<SNode<'a, C>> {
        if let Some(host) = self.shadow_host_of_parent() {
            return self.with(host);
        }
        self.parent_node().filter(|p| p.is_element())
    }
    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.idx)
    }
    fn debug_id(self) -> usize {
        self.idx
    }
    fn as_element(&self) -> Option<SNode<'a, C>> {
        self.is_element().then_some(*self)
    }
    fn as_document(&self) -> Option<SNode<'a, C>> {
        matches!(self.node().kind, NodeKind::Document(_)).then_some(*self)
    }
    fn as_shadow_root(&self) -> Option<SNode<'a, C>> {
        matches!(&self.node().kind, NodeKind::Fragment(f) if f.shadow.is_some()).then_some(*self)
    }
}

impl<'a, C: CxSource<'a>> selectors::Element for SNode<'a, C> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        let ptr = NonNull::new((self.idx).wrapping_add(1) as *mut ()).expect("non-null");
        OpaqueElement::from_non_null_ptr(ptr)
    }
    fn parent_element(&self) -> Option<Self> {
        self.parent_node().filter(|p| p.is_element())
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        self.shadow_host_of_parent().is_some()
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        let root = self.cx().dom.root_idx(self.i());
        match &self.cx().dom.n(root).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() => self.with(f.host),
            _ => None,
        }
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling_element(false)
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling_element(true)
    }
    fn first_element_child(&self) -> Option<Self> {
        let first = self.cx().dom.child_idx_iter(self.i()).find(|&c| self.cx().dom.n(c).is_element())?;
        self.with(first)
    }
    fn is_html_element_in_html_document(&self) -> bool {
        self.is_html_in_html_doc()
    }
    fn has_local_name(&self, name: &LocalName) -> bool {
        self.element().name.local == *name
    }
    fn has_namespace(&self, ns: &Namespace) -> bool {
        self.element().name.ns == *ns
    }
    fn is_same_type(&self, other: &Self) -> bool {
        let (a, b) = (self.element(), other.element());
        a.name.local == b.name.local && a.name.ns == b.name.ns
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&GenericAtomIdent<NamespaceStaticSet>>,
        local_name: &GenericAtomIdent<LocalNameStaticSet>,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        self.element().attrs.iter().any(|a| {
            a.name.local == local_name.0
                && match ns {
                    NamespaceConstraint::Any => true,
                    NamespaceConstraint::Specific(n) => a.name.ns == n.0,
                }
                && operation.eval_str(&a.value)
        })
    }
    fn match_non_ts_pseudo_class(&self, pc: &NonTSPseudoClass, _ctx: &mut MatchingContext<SelectorImpl>) -> bool {
        use NonTSPseudoClass as N;
        match pc {
            N::Active | N::Visited | N::Fullscreen | N::Autofill => false,
            N::Target => self.matches(PseudoClass::Target),
            N::AnyLink | N::Link => self.matches(PseudoClass::Link),
            N::Checked => self.matches(PseudoClass::Checked),
            N::Indeterminate => self.matches(PseudoClass::Indeterminate),
            N::Disabled => self.matches(PseudoClass::Disabled),
            N::Enabled => self.matches(PseudoClass::Enabled),
            N::Required => self.matches(PseudoClass::Required),
            N::Optional => self.matches(PseudoClass::Optional),
            N::ReadOnly => self.matches(PseudoClass::ReadOnly),
            N::ReadWrite => self.matches(PseudoClass::ReadWrite),
            N::PlaceholderShown => self.matches(PseudoClass::PlaceholderShown),
            N::Focus => self.matches(PseudoClass::Focus),
            N::FocusWithin => self.matches(PseudoClass::FocusWithin),
            N::FocusVisible => self.matches(PseudoClass::FocusVisible),
            N::Hover => self.matches(PseudoClass::Hover),
            N::Defined => self.matches(PseudoClass::Defined),
            N::Default => self.matches(PseudoClass::Default),
            N::Valid => self.matches(PseudoClass::Valid),
            N::Invalid => self.matches(PseudoClass::Invalid),
            N::Open => self.matches(PseudoClass::Open),
            N::Modal => self.matches(PseudoClass::Modal),
            N::PopoverOpen => self.cx().popovers.contains(&self.i()) || self.matches(PseudoClass::PopoverOpen),
            N::Lang(lang) => self.match_element_lang(None, lang),
            _ => false,
        }
    }
    fn match_pseudo_element(&self, _pe: &PseudoElement, _ctx: &mut MatchingContext<SelectorImpl>) -> bool {
        false
    }
    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        let (cx, i) = (self.cx(), self.i());
        let own = flags.for_self();
        if !own.is_empty() {
            cx.store.add_flags(i, cx.dom.generation_at(i), own);
        }
        let parent = flags.for_parent();
        if !parent.is_empty() {
            if let Some(p) = selectors::Element::parent_element(self) {
                cx.store.add_flags(p.i(), cx.dom.generation_at(p.i()), parent);
            }
        }
    }
    fn is_link(&self) -> bool {
        // Asked of every element matched: only a, area and link can be links.
        let e = self.element();
        matches!(&*e.name.local, "a" | "area" | "link") && self.matches(PseudoClass::AnyLink)
    }
    fn is_html_slot_element(&self) -> bool {
        let e = self.element();
        e.is_html() && e.name.local == local_name!("slot")
    }
    // Compared as strings: interning every element's id and classes per
    // query (`querySelectorAll('.x')` over a page) costs more than matching.
    fn has_id(&self, id: &AtomIdent, cs: CaseSensitivity) -> bool {
        let Some(own) = self.element().id() else { return false };
        cs.eq(own.as_bytes(), id.0.as_bytes())
    }
    fn has_class(&self, name: &AtomIdent, cs: CaseSensitivity) -> bool {
        let Some(attr) = self.element().attr("class") else { return false };
        if cs != CaseSensitivity::CaseSensitive {
            let name = name.0.as_bytes();
            return attr.split_ascii_whitespace().any(|c| cs.eq(c.as_bytes(), name));
        }
        // Tailwind class lists run to dozens of tokens and matching tests
        // them repeatedly: keep each element's tokens as atoms (pointer
        // compares) for the current DOM version.
        let dom = self.cx().dom;
        CLASS_ATOMS.with(|cache| {
            let mut cache = cache.borrow_mut();
            let key = (dom.uid(), dom.version());
            if cache.0 != key {
                cache.0 = key;
                cache.1.clear();
            }
            let atoms =
                cache.1.entry(self.i()).or_insert_with(|| attr.split_ascii_whitespace().map(Atom::from).collect());
            atoms.contains(&name.0)
        })
    }
    fn imported_part(&self, _name: &AtomIdent) -> Option<AtomIdent> {
        None
    }
    fn is_part(&self, _name: &AtomIdent) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.cx().dom.child_idx_iter(self.i()).all(|c| match &self.cx().dom.n(c).kind {
            NodeKind::Element(_) => false,
            NodeKind::Text(s) | NodeKind::CData(s) => s.is_empty(),
            _ => true,
        })
    }
    fn is_root(&self) -> bool {
        let p = self.node().parent;
        p != NIL && matches!(self.cx().dom.n(p).kind, NodeKind::Document(_))
    }
    fn has_custom_state(&self, _name: &AtomIdent) -> bool {
        false
    }
    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        each_relevant_element_hash(*self, |hash| filter.insert_hash(hash & BLOOM_HASH_MASK));
        true
    }
}

impl<'a, C: CxSource<'a>> TElement for SNode<'a, C> {
    type ConcreteNode = SNode<'a, C>;
    type TraversalChildrenIterator = Children<'a, C>;

    fn as_node(&self) -> SNode<'a, C> {
        *self
    }
    fn implicit_scope_for_sheet_in_shadow_root(
        _opaque_host: OpaqueElement,
        _sheet_index: usize,
    ) -> Option<ImplicitScopeRoot> {
        None
    }
    /// Shadow-tree children inherit from the host (the flat tree), as
    /// they are styled.
    fn inheritance_parent(&self) -> Option<Self> {
        TElement::traversal_parent(self)
    }
    fn traversal_children(&self) -> LayoutIterator<Children<'a, C>> {
        let e = self.element();
        let light = self.node().first_child;
        let shadow = e.shadow_root;
        let (next, then) = if shadow != NIL { (self.cx().dom.n(shadow).first_child, light) } else { (light, NIL) };
        // An empty shadow tree: straight to the light children.
        let (next, then) = if next == NIL { (then, NIL) } else { (next, then) };
        LayoutIterator(Children { of: *self, next, then })
    }
    fn is_html_element(&self) -> bool {
        self.element().is_html()
    }
    fn is_mathml_element(&self) -> bool {
        self.element().name.ns == ns!(mathml)
    }
    fn is_svg_element(&self) -> bool {
        self.element().name.ns == ns!(svg)
    }
    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        let text = self.element().attr("style")?;
        let (cx, i) = (self.cx(), self.i());
        // SAFETY: styling is sequential, and only this element's own style
        // computation reads its parsed style attribute.
        let cached = unsafe { cx.store.style_attr(i, cx.dom.generation_at(i)) };
        if cached.as_ref().is_none_or(|(t, _)| &**t != text) {
            let url = super::url_data();
            let block = style::properties::parse_style_attribute(
                text,
                &url,
                None,
                self.cx().quirks,
                style::stylesheets::CssRuleType::Style,
            );
            *cached = Some((text.into(), Arc::new(self.cx().lock.wrap(block))));
        }
        cached.as_ref().map(|(_, b)| b.borrow_arc())
    }
    fn state(&self) -> ElementState {
        let mut s = ElementState::empty();
        let mut set = |pc: PseudoClass, flag: ElementState| {
            if self.matches(pc) {
                s |= flag;
            }
        };
        // Every state `match_non_ts_pseudo_class` answers from, so
        // invalidation snapshots see each one change.
        set(PseudoClass::Hover, ElementState::HOVER);
        set(PseudoClass::Focus, ElementState::FOCUS);
        set(PseudoClass::FocusWithin, ElementState::FOCUS_WITHIN);
        set(PseudoClass::FocusVisible, ElementState::FOCUSRING);
        set(PseudoClass::Checked, ElementState::CHECKED);
        set(PseudoClass::Indeterminate, ElementState::INDETERMINATE);
        set(PseudoClass::Disabled, ElementState::DISABLED);
        set(PseudoClass::Enabled, ElementState::ENABLED);
        set(PseudoClass::Required, ElementState::REQUIRED);
        set(PseudoClass::Optional, ElementState::OPTIONAL_);
        set(PseudoClass::ReadOnly, ElementState::READONLY);
        set(PseudoClass::ReadWrite, ElementState::READWRITE);
        set(PseudoClass::Link, ElementState::UNVISITED);
        set(PseudoClass::PlaceholderShown, ElementState::PLACEHOLDER_SHOWN);
        set(PseudoClass::Defined, ElementState::DEFINED);
        set(PseudoClass::Default, ElementState::DEFAULT);
        set(PseudoClass::Valid, ElementState::VALID);
        set(PseudoClass::Invalid, ElementState::INVALID);
        set(PseudoClass::Open, ElementState::OPEN);
        set(PseudoClass::Modal, ElementState::MODAL);
        set(PseudoClass::Target, ElementState::URLTARGET);
        if self.cx().popovers.contains(&self.i()) || self.matches(PseudoClass::PopoverOpen) {
            s |= ElementState::POPOVER_OPEN;
        }
        s
    }
    fn has_part_attr(&self) -> bool {
        false
    }
    fn exports_any_part(&self) -> bool {
        false
    }
    fn id(&self) -> Option<&Atom> {
        self.cx().id_atom(self.i())
    }
    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        for a in self.cx().class_atoms(self.i()) {
            callback(AtomIdent::cast(a));
        }
    }
    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&style::LocalName),
    {
        for a in &self.element().attrs {
            callback(&GenericAtomIdent(a.name.local.clone()));
        }
    }
    fn has_dirty_descendants(&self) -> bool {
        self.slot().is_some_and(|s| s.dirty_descendants.get())
    }
    fn has_snapshot(&self) -> bool {
        self.slot().is_some_and(|s| s.has_snapshot.get())
    }
    fn handled_snapshot(&self) -> bool {
        self.slot().is_some_and(|s| s.handled_snapshot.get())
    }
    unsafe fn set_handled_snapshot(&self) {
        if let Some(s) = self.slot() {
            s.handled_snapshot.set(true);
        }
    }
    unsafe fn set_dirty_descendants(&self) {
        if let Some(s) = self.slot() {
            s.dirty_descendants.set(true);
        }
    }
    unsafe fn unset_dirty_descendants(&self) {
        if let Some(s) = self.slot() {
            s.dirty_descendants.set(false);
        }
    }
    fn store_children_to_process(&self, _n: isize) {
        unreachable!("sequential traversal")
    }
    fn did_process_child(&self) -> isize {
        unreachable!("sequential traversal")
    }
    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: the traversal creates an element's data before borrowing it.
        let slot = unsafe { self.cx().store.ensure(self.i(), self.cx().dom.generation_at(self.i())) };
        slot.data.borrow_mut()
    }
    unsafe fn clear_data(&self) {
        if let Some(slot) = self.slot() {
            *slot.data.borrow_mut() = Default::default();
        }
    }
    fn has_data(&self) -> bool {
        self.slot().is_some()
    }
    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.slot().map(|s| s.data.borrow())
    }
    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        self.slot().map(|s| s.data.borrow_mut())
    }
    fn skip_item_display_fixup(&self) -> bool {
        false
    }
    fn may_have_animations(&self) -> bool {
        false
    }
    fn has_animations(&self, _context: &SharedStyleContext) -> bool {
        false
    }
    fn has_css_animations(&self, _context: &SharedStyleContext, _pseudo: Option<PseudoElement>) -> bool {
        false
    }
    fn has_css_transitions(&self, _context: &SharedStyleContext, _pseudo: Option<PseudoElement>) -> bool {
        false
    }
    fn animation_rule(&self, _context: &SharedStyleContext) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn transition_rule(&self, _context: &SharedStyleContext) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn shadow_root(&self) -> Option<SNode<'a, C>> {
        self.with(self.element().shadow_root)
    }
    fn containing_shadow(&self) -> Option<SNode<'a, C>> {
        let root = self.cx().dom.root_idx(self.i());
        self.with(root).and_then(|r| r.as_shadow_root())
    }
    fn get_attr(&self, attr: &style::LocalName, ns: &style::Namespace) -> Option<String> {
        self.element().attrs.iter().find(|a| a.name.local == attr.0 && a.name.ns == ns.0).map(|a| a.value.to_string())
    }
    fn lang_attr(&self) -> Option<style::selector_parser::AttrValue> {
        self.element().attr("lang").map(style::values::AtomString::from)
    }
    fn match_element_lang(
        &self,
        override_lang: Option<Option<style::selector_parser::AttrValue>>,
        value: &style::selector_parser::Lang,
    ) -> bool {
        match override_lang {
            Some(Some(lang)) => crate::direction::lang_range_matches(lang.as_ref(), value),
            Some(None) => false,
            None => self.cx().dom.lang_matches(self.i(), value),
        }
    }
    fn is_html_document_body_element(&self) -> bool {
        let e = self.element();
        if !(e.is_html() && e.name.local == local_name!("body")) {
            return false;
        }
        let p = self.node().parent;
        p != NIL && {
            let pp = self.cx().dom.n(p).parent;
            pp != NIL && matches!(self.cx().dom.n(pp).kind, NodeKind::Document(_))
        }
    }
    fn synthesize_presentational_hints_for_legacy_attributes<V>(&self, _visited: VisitedHandlingMode, hints: &mut V)
    where
        V: Push<ApplicableDeclarationBlock>,
    {
        let e = self.element();
        if e.name.ns == ns!(svg) {
            super::hints::svg_presentation(e, self.cx().lock, self.cx().quirks, hints);
            return;
        }
        super::hints::synthesize(e, self.cx().lock, hints);
    }
    fn local_name(&self) -> &LocalName {
        &self.element().name.local
    }
    fn namespace(&self) -> &Namespace {
        &self.element().name.ns
    }
    fn query_container_size(
        &self,
        _display: &style::values::specified::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        Default::default()
    }
    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }
    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.selector_flags().contains(flags)
    }
    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        let flags = self.selector_flags();
        for f in [
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING,
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR,
            ElementSelectorFlags::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING,
        ] {
            if flags.contains(f) {
                return f;
            }
        }
        ElementSelectorFlags::empty()
    }
    fn compute_layout_damage(_old: &ComputedValues, _new: &ComputedValues) -> RestyleDamage {
        RestyleDamage::reconstruct()
    }
}
