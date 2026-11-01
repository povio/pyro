//! CSS selector matching for querySelector(All)/matches/closest using the
//! servo `selectors` crate (full Selectors 4 syntax incl. :is/:where/:has/
//! :not/:nth-child(of S)/:scope).

use std::borrow::Borrow;
use std::fmt;

use cssparser::{CowRcStr, ParseError, Parser as CssParser, ToCss};
use markup5ever::{local_name, ns, LocalName, Namespace};
use precomputed_hash::PrecomputedHash;
use rustc_hash::FxHashMap;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode as SelQuirks, SelectorCaches,
};
use selectors::matching::{matches_selector_list, ElementSelectorFlags};
use selectors::parser::{ParseRelative, SelectorList, SelectorParseErrorKind};
use selectors::OpaqueElement;

use crate::dom::Dom;
use crate::error::{DomError, DomErrorKind, DomResult};
use crate::node::{NodeId, NodeKind, NIL};

#[derive(Debug, Clone)]
pub struct Impl;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssString(pub String);
impl<'a> From<&'a str> for CssString {
    fn from(s: &'a str) -> Self {
        CssString(s.to_owned())
    }
}
impl AsRef<str> for CssString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl ToCss for CssString {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_string(&self.0, dest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssLocalName(pub LocalName);
impl<'a> From<&'a str> for CssLocalName {
    fn from(s: &'a str) -> Self {
        CssLocalName(LocalName::from(s))
    }
}
impl ToCss for CssLocalName {
    /// Escaped as a CSS identifier, so a serialized selector parses back
    /// to the same one (Tailwind's `.empty\:before\:inline-block`).
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_identifier(&self.0, dest)
    }
}
impl PrecomputedHash for CssLocalName {
    fn precomputed_hash(&self) -> u32 {
        self.0.precomputed_hash()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssNamespace(pub Namespace);
impl Borrow<Namespace> for CssNamespace {
    fn borrow(&self) -> &Namespace {
        &self.0
    }
}
impl PrecomputedHash for CssNamespace {
    fn precomputed_hash(&self) -> u32 {
        self.0.precomputed_hash()
    }
}
impl ToCss for CssNamespace {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.0)
    }
}
impl<'a> From<&'a str> for CssNamespace {
    fn from(s: &'a str) -> Self {
        CssNamespace(Namespace::from(s))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoClass {
    AnyLink,
    Link,
    Visited,
    Checked,
    Indeterminate,
    Disabled,
    Enabled,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Focus,
    FocusWithin,
    FocusVisible,
    Hover,
    Active,
    Target,
    Defined,
    Default,
    Empty,
    Valid,
    Invalid,
    Open,
    Modal,
    PopoverOpen,
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        use PseudoClass::*;
        dest.write_str(match self {
            AnyLink => ":any-link",
            Link => ":link",
            Visited => ":visited",
            Checked => ":checked",
            Indeterminate => ":indeterminate",
            Disabled => ":disabled",
            Enabled => ":enabled",
            Required => ":required",
            Optional => ":optional",
            ReadOnly => ":read-only",
            ReadWrite => ":read-write",
            PlaceholderShown => ":placeholder-shown",
            Focus => ":focus",
            FocusWithin => ":focus-within",
            FocusVisible => ":focus-visible",
            Hover => ":hover",
            Active => ":active",
            Target => ":target",
            Defined => ":defined",
            Default => ":default",
            Empty => ":empty",
            Valid => ":valid",
            Invalid => ":invalid",
            Open => ":open",
            Modal => ":modal",
            PopoverOpen => ":popover-open",
        })
    }
}

impl selectors::parser::NonTSPseudoClass for PseudoClass {
    fn is_active_or_hover(&self) -> bool {
        matches!(self, PseudoClass::Active | PseudoClass::Hover)
    }
    fn is_user_action_state(&self) -> bool {
        matches!(self, PseudoClass::Active | PseudoClass::Hover | PseudoClass::Focus)
    }
}

/// Pseudo-elements parse (so selectors stay valid) but never match an
/// element, matching browser querySelector behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PseudoElement(pub String);
impl ToCss for PseudoElement {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str("::")?;
        dest.write_str(&self.0)
    }
}
impl selectors::parser::PseudoElement for PseudoElement {}

impl selectors::parser::SelectorImpl for Impl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssLocalName;
    type LocalName = CssLocalName;
    type NamespaceUrl = CssNamespace;
    type NamespacePrefix = CssLocalName;
    type BorrowedNamespaceUrl = Namespace;
    type BorrowedLocalName = CssLocalName;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
}

struct SelParser;

impl<'i> selectors::parser::Parser<'i> for SelParser {
    type Impl = Impl;
    type Error = SelectorParseErrorKind;

    fn parse_is_and_where(&self) -> bool {
        true
    }
    fn parse_has(&self) -> bool {
        true
    }
    fn parse_nth_child_of(&self) -> bool {
        true
    }
    fn parse_host(&self) -> bool {
        true
    }
    fn is_is_alias(&self, name: &str) -> bool {
        name.eq_ignore_ascii_case("-webkit-any") || name.eq_ignore_ascii_case("matches")
    }
    fn parse_non_ts_pseudo_class(&self, name: CowRcStr<'i>) -> Result<PseudoClass, ParseError<Self::Error>> {
        use PseudoClass::*;
        let pc = match_ignore_ascii_case(
            &name,
            &[
                ("any-link", AnyLink),
                ("link", Link),
                ("visited", Visited),
                ("checked", Checked),
                ("indeterminate", Indeterminate),
                ("disabled", Disabled),
                ("enabled", Enabled),
                ("required", Required),
                ("optional", Optional),
                ("read-only", ReadOnly),
                ("read-write", ReadWrite),
                ("placeholder-shown", PlaceholderShown),
                ("focus", Focus),
                ("focus-within", FocusWithin),
                ("focus-visible", FocusVisible),
                ("hover", Hover),
                ("active", Active),
                ("target", Target),
                ("defined", Defined),
                ("default", Default),
                ("valid", Valid),
                ("invalid", Invalid),
                ("open", Open),
                ("modal", Modal),
                ("popover-open", PopoverOpen),
            ],
        );
        pc.ok_or_else(|| ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement))
    }
    fn parse_pseudo_element(&self, name: CowRcStr<'i>) -> Result<PseudoElement, ParseError<Self::Error>> {
        Ok(PseudoElement(name.to_ascii_lowercase()))
    }
}

fn match_ignore_ascii_case<T: Copy>(name: &str, table: &[(&str, T)]) -> Option<T> {
    table.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

pub type Selectors = SelectorList<Impl>;

/// Parses a selector list; invalid syntax maps to a `SyntaxError` DOMException.
pub fn parse_selectors(text: &str) -> DomResult<Selectors> {
    let mut parser = CssParser::new(text);
    SelectorList::parse(&SelParser, &mut parser, ParseRelative::No)
        .map_err(|_| DomError::new(DomErrorKind::Syntax, "is not a valid selector"))
}

/// A selector list for the DOM's query APIs: Stylo's (`crate::stylo::select`),
/// or lp-dom's when only it parses one. Queries for one id, class, tag or attribute
/// presence take lp-dom's direct paths (id index, cached class sets).
#[derive(Clone)]
pub struct Query {
    stylo: Option<crate::stylo::select::List>,
    lp: Option<Selectors>,
}

/// Parses a selector list for [`Dom::query_first`] and friends; invalid
/// syntax maps to a `SyntaxError` DOMException.
pub fn parse_query(text: &str) -> DomResult<Query> {
    let stylo = crate::stylo::select::parse(text);
    let lp = match &stylo {
        None => Some(parse_selectors(text)?),
        // One id, class, tag or attribute presence: lp-dom's direct tests
        // (the id index, cached class sets) beat general matching.
        Some(_) => parse_selectors(text).ok().filter(|l| simple(l).is_some()),
    };
    Ok(Query { stylo, lp })
}

impl Dom {
    /// `querySelector`: the first element below `root` matching `q`.
    pub fn query_first(&self, root: NodeId, q: &Query) -> DomResult<Option<NodeId>> {
        match (&q.stylo, &q.lp) {
            (_, Some(lp)) => self.query_selector(root, lp),
            (Some(list), None) => {
                let r = self.check(root)?;
                Ok(crate::stylo::select::query(self, r, list, true).first().map(|&i| self.id_of(i)))
            }
            (None, None) => Ok(None),
        }
    }

    /// `querySelectorAll`: the elements below `root` matching `q`, in tree order.
    pub fn query_all(&self, root: NodeId, q: &Query) -> DomResult<Vec<NodeId>> {
        match (&q.stylo, &q.lp) {
            (_, Some(lp)) => self.query_selector_all(root, lp),
            (Some(list), None) => {
                let r = self.check(root)?;
                Ok(crate::stylo::select::query(self, r, list, false).into_iter().map(|i| self.id_of(i)).collect())
            }
            (None, None) => Ok(Vec::new()),
        }
    }

    /// `Element.matches(selectors)`.
    pub fn element_matches(&self, el: NodeId, q: &Query) -> DomResult<bool> {
        match (&q.stylo, &q.lp) {
            (Some(list), _) => {
                let i = self.check(el)?;
                if !self.n(i).is_element() {
                    return Err(DomError::Type("not an element"));
                }
                Ok(crate::stylo::select::matches(self, i, list))
            }
            (None, Some(lp)) => self.matches(el, lp),
            (None, None) => Ok(false),
        }
    }

    /// `Element.closest(selectors)`.
    pub fn element_closest(&self, el: NodeId, q: &Query) -> DomResult<Option<NodeId>> {
        match (&q.stylo, &q.lp) {
            (Some(list), _) => {
                let i = self.check(el)?;
                if !self.n(i).is_element() {
                    return Ok(None);
                }
                Ok(crate::stylo::select::closest(self, i, list).map(|i| self.id_of(i)))
            }
            (None, Some(lp)) => self.closest(el, lp),
            (None, None) => Ok(None),
        }
    }
}

/// Small parse cache: SPA code calls the same selectors repeatedly.
#[derive(Default)]
pub struct SelectorCache {
    map: FxHashMap<Box<str>, Query>,
}
const SELECTOR_CACHE_CAP: usize = 512;
impl SelectorCache {
    pub fn get(&mut self, text: &str) -> DomResult<Query> {
        if let Some(s) = self.map.get(text) {
            return Ok(s.clone());
        }
        let s = parse_query(text)?;
        if self.map.len() >= SELECTOR_CACHE_CAP {
            self.map.clear();
        }
        self.map.insert(text.into(), s.clone());
        Ok(s)
    }
}

#[derive(Clone, Copy)]
pub struct ElemRef<'a> {
    dom: &'a Dom,
    idx: u32,
}

/// Per-element class tokens as interned atoms sorted by their precomputed
/// hash, valid for one (DOM uid, version).
#[derive(Default)]
struct ClassSets {
    key: (u64, u64),
    sets: FxHashMap<u32, Box<[LocalName]>>,
}

thread_local! {
    static CLASS_SETS: std::cell::RefCell<ClassSets> = std::cell::RefCell::new(ClassSets::default());
}

impl fmt::Debug for ElemRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ElemRef({})", self.idx)
    }
}

impl<'a> ElemRef<'a> {
    fn data(&self) -> &'a crate::node::ElementData {
        self.dom.n(self.idx).element().expect("element")
    }
    fn wrap(&self, idx: u32) -> Option<Self> {
        (idx != NIL && self.dom.n(idx).is_element()).then_some(ElemRef { dom: self.dom, idx })
    }
    fn sibling_element(&self, forward: bool) -> Option<Self> {
        let mut c = self.idx;
        loop {
            let n = self.dom.n(c);
            c = if forward { n.next } else { n.prev };
            if c == NIL {
                return None;
            }
            if self.dom.n(c).is_element() {
                return Some(ElemRef { dom: self.dom, idx: c });
            }
        }
    }
}

impl selectors::Element for ElemRef<'_> {
    type Impl = Impl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.dom.n(self.idx))
    }
    fn parent_element(&self) -> Option<Self> {
        self.wrap(self.dom.n(self.idx).parent)
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        let p = self.dom.n(self.idx).parent;
        p != NIL && matches!(&self.dom.n(p).kind, NodeKind::Fragment(f) if f.shadow.is_some())
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        let root = self.dom.root_idx(self.idx);
        match &self.dom.n(root).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() => self.wrap(f.host),
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
        self.dom
            .child_idx_iter(self.idx)
            .find(|&c| self.dom.n(c).is_element())
            .map(|idx| ElemRef { dom: self.dom, idx })
    }
    fn is_html_element_in_html_document(&self) -> bool {
        self.data().is_html() && self.dom.is_html_doc(self.dom.n(self.idx).owner_doc)
    }
    fn has_local_name(&self, name: &CssLocalName) -> bool {
        self.data().name.local == name.0
    }
    fn has_namespace(&self, ns: &Namespace) -> bool {
        &self.data().name.ns == ns
    }
    fn is_same_type(&self, other: &Self) -> bool {
        let (a, b) = (self.data(), other.data());
        a.name.local == b.name.local && a.name.ns == b.name.ns
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&CssNamespace>,
        local_name: &CssLocalName,
        operation: &AttrSelectorOperation<&CssString>,
    ) -> bool {
        self.data().attrs.iter().any(|a| {
            a.name.local == local_name.0
                && match ns {
                    NamespaceConstraint::Any => true,
                    NamespaceConstraint::Specific(n) => a.name.ns == n.0,
                }
                && operation.eval_str(&a.value)
        })
    }
    fn match_non_ts_pseudo_class(&self, pc: &PseudoClass, _ctx: &mut MatchingContext<Impl>) -> bool {
        self.dom.matches_state(self.idx, *pc)
    }
    fn match_pseudo_element(&self, _pe: &PseudoElement, _ctx: &mut MatchingContext<Impl>) -> bool {
        false
    }
    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}
    fn is_link(&self) -> bool {
        self.dom.is_link(self.idx)
    }
    fn is_html_slot_element(&self) -> bool {
        let d = self.data();
        d.is_html() && d.name.local == local_name!("slot")
    }
    fn has_id(&self, id: &CssLocalName, cs: CaseSensitivity) -> bool {
        self.data().id().is_some_and(|v| cs.eq(v.as_bytes(), id.0.as_bytes()))
    }
    fn has_class(&self, name: &CssLocalName, cs: CaseSensitivity) -> bool {
        if cs != CaseSensitivity::CaseSensitive {
            return self
                .data()
                .attr("class")
                .is_some_and(|v| v.split_ascii_whitespace().any(|c| cs.eq(c.as_bytes(), name.0.as_bytes())));
        }
        // Tailwind class attributes run to dozens of tokens and selector
        // matching tests them repeatedly (rightmost class, then ancestors for
        // descendant combinators): keep each element's token hashes for the
        // current DOM version.
        let wanted = name.0.precomputed_hash();
        CLASS_SETS.with(|cache| {
            let mut cache = cache.borrow_mut();
            let key = (self.dom.uid(), self.dom.version());
            if cache.key != key {
                cache.key = key;
                cache.sets.clear();
            }
            let set = cache.sets.entry(self.idx).or_insert_with(|| {
                let mut atoms: Vec<LocalName> =
                    self.data().attr("class").unwrap_or("").split_ascii_whitespace().map(LocalName::from).collect();
                atoms.sort_unstable_by_key(|a| a.precomputed_hash());
                atoms.into_boxed_slice()
            });
            // Interned atoms: equal strings are the same atom.
            let start = set.partition_point(|a| a.precomputed_hash() < wanted);
            set[start..].iter().take_while(|a| a.precomputed_hash() == wanted).any(|a| *a == name.0)
        })
    }
    fn has_custom_state(&self, _name: &CssLocalName) -> bool {
        false
    }
    fn imported_part(&self, _name: &CssLocalName) -> Option<CssLocalName> {
        None
    }
    fn is_part(&self, _name: &CssLocalName) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.dom.child_idx_iter(self.idx).all(|c| match &self.dom.n(c).kind {
            NodeKind::Element(_) => false,
            NodeKind::Text(s) | NodeKind::CData(s) => s.is_empty(),
            _ => true,
        })
    }
    fn is_root(&self) -> bool {
        let p = self.dom.n(self.idx).parent;
        p != NIL && matches!(self.dom.n(p).kind, NodeKind::Document(_))
    }
    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

impl Dom {
    pub(crate) fn is_html_doc(&self, doc: u32) -> bool {
        matches!(&self.n(doc).kind, NodeKind::Document(d) if d.kind == crate::node::DocumentKind::Html)
    }

    fn is_link(&self, idx: u32) -> bool {
        self.n(idx)
            .element()
            .is_some_and(|e| e.is_html() && matches!(&*e.name.local, "a" | "area" | "link") && e.attr("href").is_some())
    }

    pub(crate) fn matches_state(&self, idx: u32, pc: PseudoClass) -> bool {
        use PseudoClass::*;
        let e = self.n(idx).element().expect("element");
        match pc {
            AnyLink | Link => self.is_link(idx),
            Visited | Active | Target => false,
            Checked => self.checkedness_idx(idx) || self.option_selected_idx(idx),
            Indeterminate => e.control.as_ref().is_some_and(|c| c.indeterminate),
            Disabled => self.is_disabled_idx(idx),
            Enabled => self.is_form_control_idx(idx) && !self.is_disabled_idx(idx),
            Required => self.is_form_control_idx(idx) && e.attr("required").is_some(),
            Optional => self.is_form_control_idx(idx) && e.attr("required").is_none(),
            ReadWrite => self.is_mutable_text_idx(idx),
            ReadOnly => !self.is_mutable_text_idx(idx),
            PlaceholderShown => {
                e.attr("placeholder").is_some()
                    && self.is_mutable_text_idx(idx)
                    && self.control_value_idx(idx).is_empty()
            }
            Focus | FocusVisible => self.focused_idx(self.n(idx).owner_doc) == idx,
            FocusWithin => {
                let f = self.focused_idx(self.n(idx).owner_doc);
                f != NIL && self.shadow_including_inclusive_ancestor(idx, f)
            }
            Hover => {
                let h = self.hovered_idx(self.n(idx).owner_doc);
                h != NIL && self.shadow_including_inclusive_ancestor(idx, h)
            }
            Defined => !e.name.local.contains('-') || e.flags & crate::node::element_flags::CUSTOM_DEFINED != 0,
            Default => {
                (e.is_html() && matches!(&*e.name.local, "input") && e.attr("checked").is_some())
                    || (e.is_html() && e.name.local == local_name!("option") && e.attr("selected").is_some())
            }
            Empty => false,
            Valid | Invalid => {
                let id = self.id_of(idx);
                let is_form = e.is_html() && matches!(&*e.name.local, "form" | "fieldset");
                let candidate = self.will_validate(id).unwrap_or(false);
                if !candidate && !is_form {
                    return false;
                }
                let ok = if is_form {
                    self.descendants(id).is_ok_and(|mut d| {
                        d.all(|n| {
                            !self.will_validate(n).unwrap_or(false)
                                || self.validity(n).map(|v| v.valid()).unwrap_or(true)
                        })
                    })
                } else {
                    self.validity(id).map(|v| v.valid()).unwrap_or(true)
                };
                (pc == Valid) == ok
            }
            Open => e.is_html() && matches!(&*e.name.local, "details" | "dialog") && e.attr("open").is_some(),
            // Modal dialogs and open popovers are tracked by the style engine,
            // not by selector matching.
            Modal | PopoverOpen => false,
        }
    }

    pub(crate) fn shadow_including_inclusive_ancestor(&self, anc: u32, mut node: u32) -> bool {
        loop {
            if self.is_inclusive_ancestor(anc, node) {
                return true;
            }
            let r = self.root_idx(node);
            match &self.n(r).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => node = f.host,
                _ => return false,
            }
        }
    }

    fn element_ref(&self, idx: u32) -> ElemRef<'_> {
        ElemRef { dom: self, idx }
    }

    fn quirks(&self, idx: u32) -> SelQuirks {
        match &self.n(self.n(idx).owner_doc).kind {
            NodeKind::Document(d) => match d.quirks {
                crate::node::QuirksMode::Quirks => SelQuirks::Quirks,
                crate::node::QuirksMode::LimitedQuirks => SelQuirks::LimitedQuirks,
                crate::node::QuirksMode::NoQuirks => SelQuirks::NoQuirks,
            },
            _ => SelQuirks::NoQuirks,
        }
    }

    fn matches_with(&self, sel: &Selectors, idx: u32, scope: u32, caches: &mut SelectorCaches) -> bool {
        let mut ctx = MatchingContext::new(
            MatchingMode::Normal,
            None,
            caches,
            self.quirks(idx),
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        if scope != NIL && self.n(scope).is_element() {
            ctx.scope_element = Some(OpaqueElement::new(self.n(scope)));
        }
        matches_selector_list(sel, &self.element_ref(idx), &mut ctx)
    }

    /// `Element.matches(selectors)`.
    pub fn matches(&self, el: NodeId, sel: &Selectors) -> DomResult<bool> {
        let i = self.check(el)?;
        if !self.n(i).is_element() {
            return Err(DomError::Type("not an element"));
        }
        let mut caches = SelectorCaches::default();
        Ok(self.matches_with(sel, i, i, &mut caches))
    }

    /// `Element.closest(selectors)`.
    pub fn closest(&self, el: NodeId, sel: &Selectors) -> DomResult<Option<NodeId>> {
        let start = self.check(el)?;
        let mut caches = SelectorCaches::default();
        let mut cur = start;
        while cur != NIL && self.n(cur).is_element() {
            if self.matches_with(sel, cur, start, &mut caches) {
                return Ok(Some(self.id_of(cur)));
            }
            cur = self.n(cur).parent;
        }
        Ok(None)
    }

    /// `querySelector` on a Document, DocumentFragment or Element.
    pub fn query_selector(&self, root: NodeId, sel: &Selectors) -> DomResult<Option<NodeId>> {
        let r = self.check(root)?;
        let quirks = self.quirks(r) == SelQuirks::Quirks;
        if let (Some(Simple::Id(id)), false) = (simple(sel), quirks) {
            // The id index gives the first element with the id in the tree:
            // the answer when it is inside the scope. (Duplicate ids, a match
            // outside the scope: scan.)
            let tree = self.root_idx(r);
            match self.get_element_by_id(self.id_of(tree), id)? {
                None => return Ok(None),
                Some(found) => {
                    let f = self.check(found)?;
                    if f != r && self.is_inclusive_ancestor(r, f) {
                        return Ok(Some(found));
                    }
                }
            }
        }
        let mut first = None;
        self.select(r, sel, |idx| {
            first = Some(idx);
            false
        });
        Ok(first.map(|i| self.id_of(i)))
    }

    /// `querySelectorAll` (static result, tree order).
    pub fn query_selector_all(&self, root: NodeId, sel: &Selectors) -> DomResult<Vec<NodeId>> {
        let r = self.check(root)?;
        if let (Some(Simple::Id(id)), false) = (simple(sel), self.quirks(r) == SelQuirks::Quirks) {
            // No element with the id in the whole tree: nothing to scan.
            if self.get_element_by_id(self.id_of(self.root_idx(r)), id)?.is_none() {
                return Ok(Vec::new());
            }
        }
        let mut out = Vec::new();
        self.select(r, sel, |idx| {
            out.push(self.id_of(idx));
            true
        });
        Ok(out)
    }

    /// Calls `found` for each element under `r` matching `sel`, in tree
    /// order, while it returns true. Simple selectors (one tag, class or
    /// attribute presence) are tested directly; others go through
    /// `selectors` with one matching context for the whole walk.
    fn select(&self, r: u32, sel: &Selectors, mut found: impl FnMut(u32) -> bool) {
        let quirks = self.quirks(r);
        let html_doc = self.is_html_doc(self.n(r).owner_doc);
        let fast = simple(sel).filter(|s| quirks != SelQuirks::Quirks || !matches!(s, Simple::Class(_)));
        let mut caches = SelectorCaches::default();
        let mut ctx = MatchingContext::new(
            MatchingMode::Normal,
            None,
            &mut caches,
            quirks,
            NeedsSelectorFlags::No,
            MatchingForInvalidation::No,
        );
        if self.n(r).is_element() {
            ctx.scope_element = Some(OpaqueElement::new(self.n(r)));
        }
        let class = match &fast {
            Some(Simple::Class(name)) => Some(CssLocalName((*name).clone())),
            _ => None,
        };
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                let hit = match &fast {
                    Some(Simple::Tag(name, lower)) => {
                        let want = if html_doc && e.is_html() { lower } else { name };
                        e.name.local == **want
                    }
                    // The per-element class sets `has_class` keeps for the
                    // DOM version (long Tailwind attributes split once).
                    Some(Simple::Class(_)) => {
                        let name = class.as_ref().expect("class");
                        selectors::Element::has_class(&self.element_ref(cur), name, CaseSensitivity::CaseSensitive)
                    }
                    Some(Simple::Attr(name, lower)) => {
                        let want = if html_doc && e.is_html() { lower } else { name };
                        e.attrs.iter().any(|a| a.name.ns == ns!() && a.name.local == **want)
                    }
                    Some(Simple::Id(_)) | None => matches_selector_list(sel, &self.element_ref(cur), &mut ctx),
                };
                if hit && !found(cur) {
                    return;
                }
            }
            cur = self.following_in(cur, r);
        }
    }
}

/// A selector list of one selector made of one simple selector, which
/// `select` tests without the matching machinery.
enum Simple<'a> {
    Id(&'a str),
    Class(&'a LocalName),
    /// The name as written and lowercased (HTML elements in HTML documents).
    Tag(&'a LocalName, &'a LocalName),
    /// `[name]` in no namespace: as written and lowercased.
    Attr(&'a LocalName, &'a LocalName),
}

fn simple(sel: &Selectors) -> Option<Simple<'_>> {
    use selectors::parser::Component;
    let [one] = sel.slice() else { return None };
    let mut parts = one.iter_raw_match_order();
    let first = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some(match first {
        Component::ID(id) => Simple::Id(&id.0),
        Component::Class(c) => Simple::Class(&c.0),
        Component::LocalName(n) => Simple::Tag(&n.name.0, &n.lower_name.0),
        Component::AttributeInNoNamespaceExists { local_name, local_name_lower } => {
            Simple::Attr(&local_name.0, &local_name_lower.0)
        }
        _ => return None,
    })
}
