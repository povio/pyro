//! CSS selector matching for querySelector(All)/matches/closest using the
//! servo `selectors` crate (full Selectors 4 syntax incl. :is/:where/:has/
//! :not/:nth-child(of S)/:scope).

use std::borrow::Borrow;
use std::fmt;
use std::rc::Rc;

use cssparser::{ParseError, Parser as CssParser, ToCss};
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
use crate::node::{Atom, NodeId, NodeKind, NIL};
use crate::select_query::FastList;

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

/// An id or class name in a selector: the atom elements store theirs as
/// (`ElementData::id_atom`, `ElementData::classes`), so matching compares
/// atoms.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CssIdent(pub Atom);
impl<'a> From<&'a str> for CssIdent {
    fn from(s: &'a str) -> Self {
        CssIdent(Atom::from(s))
    }
}
impl ToCss for CssIdent {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        cssparser::serialize_identifier(&self.0, dest)
    }
}
impl PrecomputedHash for CssIdent {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PseudoClass {
    /// `:lang(ranges)`: comma-separated language ranges.
    Lang(Box<str>),
    /// `:dir(ltr|rtl)`.
    Dir(Box<str>),
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
    InRange,
    OutOfRange,
    UserValid,
    UserInvalid,
    /// `:state(ident)`: a custom state of the element's `CustomStateSet`.
    State(Atom),
}

impl ToCss for PseudoClass {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        use PseudoClass::*;
        match self {
            Lang(r) => return write!(dest, ":lang({r})"),
            Dir(d) => return write!(dest, ":dir({d})"),
            State(s) => {
                dest.write_str(":state(")?;
                cssparser::serialize_identifier(s, dest)?;
                return dest.write_str(")");
            }
            _ => {}
        }
        dest.write_str(match self {
            Lang(_) | Dir(_) | State(_) => unreachable!(),
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
            InRange => ":in-range",
            OutOfRange => ":out-of-range",
            UserValid => ":user-valid",
            UserInvalid => ":user-invalid",
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
impl selectors::parser::PseudoElement for PseudoElement {
    fn is_in_pseudo_element_tree(&self) -> bool { self.0.starts_with("view-transition-") && self.0.contains('(') }
}

impl selectors::parser::SelectorImpl for Impl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = CssString;
    type Identifier = CssIdent;
    type LocalName = CssLocalName;
    type NamespaceUrl = CssNamespace;
    type NamespacePrefix = CssLocalName;
    type BorrowedNamespaceUrl = Namespace;
    type BorrowedLocalName = CssLocalName;
    type NonTSPseudoClass = PseudoClass;
    type PseudoElement = PseudoElement;
}

/// Holds the `@namespace` declarations in scope; none outside a stylesheet.
struct SelParser<'n>(&'n [(String, String)]);

impl<'i> selectors::parser::Parser<'i> for SelParser<'_> {
    type Impl = Impl;
    type Error = SelectorParseErrorKind;

    fn default_namespace(&self) -> Option<CssNamespace> {
        self.0.iter().rev().find(|(p, _)| p.is_empty()).map(|(_, url)| CssNamespace::from(url.as_str()))
    }
    fn namespace_for_prefix(&self, prefix: &CssLocalName) -> Option<CssNamespace> {
        self.0.iter().rev().find(|(p, _)| **p == *prefix.0).map(|(_, url)| CssNamespace::from(url.as_str()))
    }
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
    fn parse_non_ts_pseudo_class(&self, name: &str) -> Result<PseudoClass, ParseError<Self::Error>> {
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
                ("in-range", InRange),
                ("out-of-range", OutOfRange),
                ("user-valid", UserValid),
                ("user-invalid", UserInvalid),
            ],
        );
        pc.ok_or_else(|| ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement))
    }
    fn parse_non_ts_functional_pseudo_class(
        &self,
        name: &str,
        parser: &mut CssParser<'i>,
        _after_part: bool,
    ) -> Result<PseudoClass, ParseError<Self::Error>> {
        if name.eq_ignore_ascii_case("lang") {
            let mut ranges = Vec::new();
            loop {
                ranges.push(parser.expect_ident_or_string()?.to_string());
                if parser.try_parse(|p| p.expect_comma()).is_err() {
                    break;
                }
            }
            return Ok(PseudoClass::Lang(ranges.join(",").into()));
        }
        if name.eq_ignore_ascii_case("dir") {
            let d = parser.expect_ident()?.to_ascii_lowercase();
            return Ok(PseudoClass::Dir(d.into()));
        }
        if name.eq_ignore_ascii_case("state") {
            return Ok(PseudoClass::State(Atom::from(parser.expect_ident()?.as_ref())));
        }
        Err(ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement))
    }
    fn parse_pseudo_element(&self, name: &str) -> Result<PseudoElement, ParseError<Self::Error>> {
        let lower = name.to_ascii_lowercase();
        // Unknown pseudo-elements are invalid selectors, except the
        // `-webkit-` prefixed ones browsers accept (and never match).
        if !KNOWN_PSEUDO_ELEMENTS.contains(&lower.as_str()) && !lower.starts_with("-webkit-") {
            return Err(ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement));
        }
        Ok(PseudoElement(lower))
    }
    fn parse_functional_pseudo_element(
        &self,
        name: &str,
        parser: &mut CssParser<'i>,
    ) -> Result<PseudoElement, ParseError<Self::Error>> {
        let lower = name.to_ascii_lowercase();
        let invalid = || ParseError::custom(SelectorParseErrorKind::UnsupportedPseudoClassOrElement);
        let start = parser.position();
        match lower.as_str() {
            // `::slotted(<compound-selector>)`.
            "slotted" => {
                let list = SelectorList::parse(self, parser, ParseRelative::No)?;
                let [selector] = list.slice() else { return Err(invalid()) };
                let mut iter = selector.iter();
                while iter.next().is_some() {}
                if iter.next_sequence().is_some() {
                    return Err(invalid());
                }
            }
            // `::part(<ident>+)`.
            "part" => {
                parser.expect_ident()?;
                while parser.try_parse(|p| p.expect_ident().map(|_| ())).is_ok() {}
                if !parser.is_exhausted() {
                    return Err(invalid());
                }
            }
            "view-transition-group" | "view-transition-image-pair" | "view-transition-old" | "view-transition-new" => {
                let pseudo = style::selector_parser::transition_selector(&lower, parser).map_err(|_| invalid())?;
                return Ok(PseudoElement(pseudo.to_css_string()[2..].to_owned()));
            }
            "highlight" => {
                let pseudo = style::selector_parser::highlight_selector(parser).map_err(|_| invalid())?;
                return Ok(PseudoElement(pseudo.to_css_string()[2..].to_owned()));
            }
            "cue" | "cue-region"
            | "picker"
            | "scroll-button"
            | "column" => {
                // The argument is kept only for serialization.
                while parser.next().is_ok() {}
            }
            _ => return Err(invalid()),
        }
        let arg = parser.slice_from(start).to_owned();
        Ok(PseudoElement(format!("{lower}({arg})")))
    }
}

/// Percent-decodes a URL fragment (bytes compared to the element's UTF-8 id).
pub(crate) fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Pseudo-elements a selector may name (CSS Pseudo-Elements 4, Scoping,
/// View Transitions, the form control parts browsers ship).
const KNOWN_PSEUDO_ELEMENTS: &[&str] = &[
    "before",
    "after",
    "first-line",
    "first-letter",
    "selection",
    "backdrop",
    "placeholder",
    "marker",
    "file-selector-button",
    "spelling-error",
    "grammar-error",
    "target-text",
    "details-content",
    "view-transition",
    "cue",
    "checkmark",
    "picker-icon",
    "scroll-marker",
    "scroll-marker-group",
    "search-text",
];

fn match_ignore_ascii_case<T: Clone>(name: &str, table: &[(&str, T)]) -> Option<T> {
    table.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

pub type Selectors = SelectorList<Impl>;

/// Parses a selector list; invalid syntax maps to a `SyntaxError` DOMException.
pub fn parse_selectors(text: &str) -> DomResult<Selectors> {
    parse_selectors_in(text, &[])
}

/// [`parse_selectors`] with the `@namespace` declarations of a stylesheet.
pub fn parse_selectors_in(text: &str, namespaces: &[(String, String)]) -> DomResult<Selectors> {
    let mut parser = CssParser::new(text);
    SelectorList::parse(&SelParser(namespaces), &mut parser, ParseRelative::No)
        .map_err(|_| DomError::new(DomErrorKind::Syntax, "is not a valid selector"))
}

/// A selector list for the DOM's query APIs: Stylo's (`crate::stylo::select`),
/// or pyro-dom's when only it parses one. Lists of type, id, class and attribute
/// tests under descendant and child combinators also have a compiled form
/// (`crate::select_query`), which the queries take instead.
#[derive(Clone)]
pub struct Query {
    stylo: Option<crate::stylo::select::List>,
    lp: Option<Selectors>,
    fast: Option<std::sync::Arc<FastList>>,
}

/// Parses a selector list for [`Dom::query_first`] and friends; invalid
/// syntax maps to a `SyntaxError` DOMException.
pub fn parse_query(text: &str) -> DomResult<Query> {
    let stylo = crate::stylo::select::parse(text);
    let lp = match &stylo {
        None => Some(parse_selectors(text)?),
        Some(_) => None,
    };
    let fast = match (&stylo, &lp) {
        (Some(list), _) => crate::select_query::compile(list),
        (None, Some(list)) => crate::select_query::compile(list),
        (None, None) => None,
    };
    Ok(Query { stylo, lp, fast: fast.map(std::sync::Arc::new) })
}

impl Query {
    /// Whether the queries take the compiled form (tests).
    #[doc(hidden)]
    pub fn is_compiled(&self) -> bool {
        self.fast.is_some()
    }
}

impl Dom {
    /// `querySelector`: the first element below `root` matching `q`.
    pub fn query_first(&self, root: NodeId, q: &Query) -> DomResult<Option<NodeId>> {
        if let Some(fast) = &q.fast {
            let r = self.check(root)?;
            let mut first = NIL;
            self.fast_select(r, fast, true, |i| {
                first = i;
                false
            });
            return Ok(self.opt_id(first));
        }
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
        let mut out = Vec::new();
        self.query_all_into(root, q, &mut out)?;
        Ok(out)
    }

    /// [`Dom::query_all`] appending to `out`, a buffer the caller may keep
    /// for the next query (no growth of a fresh vector per call).
    pub fn query_all_into(&self, root: NodeId, q: &Query, out: &mut Vec<NodeId>) -> DomResult<()> {
        if let Some(fast) = &q.fast {
            let r = self.check(root)?;
            self.fast_select(r, fast, false, |i| {
                out.push(self.id_of(i));
                true
            });
            return Ok(());
        }
        match (&q.stylo, &q.lp) {
            (_, Some(lp)) => out.extend(self.query_selector_all(root, lp)?),
            (Some(list), None) => {
                let r = self.check(root)?;
                out.extend(crate::stylo::select::query(self, r, list, false).into_iter().map(|i| self.id_of(i)));
            }
            (None, None) => {}
        }
        Ok(())
    }

    /// `Element.matches(selectors)`.
    pub fn element_matches(&self, el: NodeId, q: &Query) -> DomResult<bool> {
        if let Some(fast) = &q.fast {
            let (_, n) = self.live(el)?;
            return self.fast_matches(n, fast).ok_or(DomError::Type("not an element"));
        }
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
        if let Some(fast) = &q.fast {
            let (i, n) = self.live(el)?;
            return Ok(self.fast_closest(i, n, fast).map(|i| self.id_of(i)));
        }
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
    /// The same queries keyed by the selector's characters as a JavaScript
    /// string stores them (one map per width), for callers that have those
    /// and no `String`: `Element.matches` runs thousands of times per
    /// Playwright locator, and a lookup is one hash and compare of the bytes.
    latin1: FxHashMap<Rc<[u8]>, Rc<Query>>,
    utf16: FxHashMap<Box<[u16]>, Query>,
    /// The Latin-1 selector looked up last and its query (shared with
    /// `latin1`): a run of calls with one selector, as a selector engine's
    /// `matches` loop makes, compares the characters and nothing else.
    last: Option<(Rc<[u8]>, Rc<Query>)>,
}
const SELECTOR_CACHE_CAP: usize = 512;
impl SelectorCache {
    pub fn get(&mut self, text: &str) -> DomResult<Query> {
        if let Some(s) = self.map.get(text) {
            return Ok(s.clone());
        }
        let s = parse_query(text)?;
        self.make_room(self.map.len());
        self.map.insert(text.into(), s.clone());
        Ok(s)
    }

    /// Empties the cache when a map about to gain an entry (holding `len`)
    /// is full. Each map is checked: several UTF-16 keys (lone surrogates)
    /// convert to one text, so `utf16` can outgrow `map`.
    fn make_room(&mut self, len: usize) {
        if len >= SELECTOR_CACHE_CAP {
            self.map.clear();
            self.latin1.clear();
            self.utf16.clear();
            self.last = None;
        }
    }

    /// Entries in the cache's maps (tests).
    #[doc(hidden)]
    pub fn entry_count(&self) -> usize {
        self.map.len() + self.latin1.len() + self.utf16.len()
    }

    /// `f` on the query of a selector given as Latin-1 characters, without
    /// taking it out of the cache. The repeat of the last selector is the
    /// whole of the inlined part: one compare of the characters.
    #[inline]
    pub fn with_latin1<R>(&mut self, chars: &[u8], f: impl FnOnce(&Query) -> R) -> DomResult<R> {
        if let Some((key, q)) = &self.last {
            if **key == *chars {
                return Ok(f(q));
            }
        }
        let (key, q) = self.latin1_entry(chars)?;
        let r = f(&q);
        self.last = Some((key, q));
        Ok(r)
    }

    /// The entry of `chars`, parsed and stored when the cache has none.
    #[inline(never)]
    fn latin1_entry(&mut self, chars: &[u8]) -> DomResult<(Rc<[u8]>, Rc<Query>)> {
        Ok(match self.latin1.get_key_value(chars) {
            Some((key, q)) => (key.clone(), q.clone()),
            None => {
                let q = Rc::new(self.get(&chars.iter().map(|&c| char::from(c)).collect::<String>())?);
                self.make_room(self.latin1.len());
                let key: Rc<[u8]> = chars.into();
                self.latin1.insert(key.clone(), q.clone());
                (key, q)
            }
        })
    }

    /// [`SelectorCache::with_latin1`] for UTF-16 code units.
    pub fn with_utf16<R>(&mut self, units: &[u16], f: impl FnOnce(&Query) -> R) -> DomResult<R> {
        if let Some(q) = self.utf16.get(units) {
            return Ok(f(q));
        }
        let q = self.get(&String::from_utf16_lossy(units))?;
        let r = f(&q);
        self.make_room(self.utf16.len());
        self.utf16.insert(units.into(), q);
        Ok(r)
    }
}

#[derive(Clone, Copy)]
pub struct ElemRef<'a> {
    dom: &'a Dom,
    idx: u32,
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
        if !self.dom.n(self.idx).in_shadow_tree() {
            return None;
        }
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
        self.dom.matches_state(self.idx, pc)
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
    fn has_id(&self, id: &CssIdent, cs: CaseSensitivity) -> bool {
        self.data().id_atom().is_some_and(|own| match cs {
            CaseSensitivity::CaseSensitive => *own == id.0,
            CaseSensitivity::AsciiCaseInsensitive => own.eq_ignore_ascii_case(&id.0),
        })
    }
    fn has_class(&self, name: &CssIdent, cs: CaseSensitivity) -> bool {
        let classes = self.data().classes();
        match cs {
            CaseSensitivity::CaseSensitive => classes.contains(&name.0),
            CaseSensitivity::AsciiCaseInsensitive => classes.iter().any(|c| c.eq_ignore_ascii_case(&name.0)),
        }
    }
    fn has_class_prefix(&self, prefix: &CssIdent, quirks: bool) -> bool {
        self.data().classes().iter().any(|class| selectors::attr::matches_class_prefix(class, &prefix.0, quirks))
    }
    fn has_custom_state(&self, name: &CssIdent) -> bool {
        self.dom.has_custom_state_idx(self.idx, &name.0)
    }
    fn imported_part(&self, _name: &CssIdent) -> Option<CssIdent> {
        None
    }
    fn is_part(&self, _name: &CssIdent) -> bool {
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
        self.n(idx).element().is_some_and(|e| {
            e.is_html()
                && matches!(&*e.name.local, "a" | "area" | "link")
                && e.attr_local(&local_name!("href")).is_some()
        })
    }

    /// `:target`: the element the document URL's fragment names.
    /// An element of the document tree whose id is the URL's decoded
    /// fragment, which the document keeps as an atom (`DocumentData::url_target`).
    fn is_url_target(&self, idx: u32) -> bool {
        let n = self.n(idx);
        let Some(id) = n.element().and_then(|e| e.id_atom()) else { return false };
        if !n.in_document_tree() {
            return false;
        }
        matches!(&self.n(n.owner_doc).kind, NodeKind::Document(d) if d.url_target.as_ref() == Some(id))
    }

    pub(crate) fn matches_state(&self, idx: u32, pc: &PseudoClass) -> bool {
        use PseudoClass::*;
        let e = self.n(idx).element().expect("element");
        match pc {
            Lang(ranges) => self.lang_matches(idx, ranges),
            Dir(d) => match &**d {
                "ltr" => self.directionality_idx(idx) == crate::direction::Direction::Ltr,
                "rtl" => self.directionality_idx(idx) == crate::direction::Direction::Rtl,
                _ => false,
            },
            AnyLink | Link => self.is_link(idx),
            Visited => false,
            Active => {
                let a = self.activated_idx(self.n(idx).owner_doc);
                a != NIL && self.user_action_chain_holds(idx, a)
            }
            Target => self.is_url_target(idx),
            Checked => self.checkedness_idx(idx) || self.option_selected_idx(idx),
            Indeterminate => {
                e.control.as_ref().is_some_and(|c| c.indeterminate)
                    // A progress bar without a value (HTML §4.16.3).
                    || (e.is_html()
                        && e.name.local == local_name!("progress")
                        && e.attr_local(&local_name!("value")).is_none())
            }
            Disabled => self.is_disabled_idx(idx),
            Enabled => (self.is_form_control_idx(idx) || self.is_face_idx(idx)) && !self.is_disabled_idx(idx),
            State(s) => self.has_custom_state_idx(idx, s),
            Required => self.required_applies_idx(idx) && e.attr_local(&local_name!("required")).is_some(),
            Optional => self.required_applies_idx(idx) && e.attr_local(&local_name!("required")).is_none(),
            ReadWrite => self.matches_read_write_idx(idx),
            ReadOnly => !self.matches_read_write_idx(idx),
            PlaceholderShown => {
                e.attr_local(&local_name!("placeholder")).is_some()
                    && self.is_mutable_text_idx(idx)
                    && self.control_value_idx(idx).is_empty()
            }
            // The focused element, and every shadow host whose shadow tree holds it (at any depth
            // of nested shadow trees).
            Focus => {
                let f = self.focused_idx(self.n(idx).owner_doc);
                f != NIL && (f == idx || self.shadow_host_of_focused(idx, f))
            }
            FocusVisible => {
                let doc = self.n(idx).owner_doc;
                self.focused_idx(doc) == idx && self.focus_visible_now(doc)
            }
            FocusWithin => {
                let f = self.focused_idx(self.n(idx).owner_doc);
                f != NIL && self.user_action_chain_holds(idx, f)
            }
            Hover => {
                let h = self.hovered_idx(self.n(idx).owner_doc);
                h != NIL && self.user_action_chain_holds(idx, h)
            }
            Defined => e.is_defined(),
            Default => {
                // A checkbox or radio button checked by default (HTML :default); other input
                // types ignore `checked`.
                (e.is_html()
                    && matches!(&*e.name.local, "input")
                    && matches!(self.input_type_idx(idx), crate::forms::InputType::Checkbox | crate::forms::InputType::Radio)
                    && e.attr_local(&local_name!("checked")).is_some())
                    || (e.is_html()
                        && e.name.local == local_name!("option")
                        && e.attr_local(&local_name!("selected")).is_some())
            }
            Empty => false,
            InRange | OutOfRange => self.range_state_idx(idx).is_some_and(|in_range| in_range == (*pc == InRange)),
            // User validity (HTML §4.10.18.6): set once the user commits a
            // change or a submission validates the form, cleared by reset.
            // Only controls get it, so `Valid`/`Invalid` see a candidate.
            UserValid | UserInvalid => {
                e.control.as_ref().is_some_and(|c| c.user_validity)
                    && self.matches_state(idx, if *pc == UserValid { &Valid } else { &Invalid })
            }
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
                (*pc == Valid) == ok
            }
            Open => {
                e.is_html()
                    && match &*e.name.local {
                        "details" | "dialog" => e.attr_local(&local_name!("open")).is_some(),
                        // A customizable select whose picker is open.
                        "select" => self.is_select_picker_open_idx(idx),
                        _ => false,
                    }
            }
            // A dialog shown with `showModal()` that is still open.
            Modal => {
                e.is_html()
                    && &*e.name.local == "dialog"
                    && e.attr_local(&local_name!("open")).is_some()
                    && self.is_modal_idx(idx)
            }
            PopoverOpen => self.is_popover_showing_idx(idx),
        }
    }

    /// Whether `:hover`, `:active` or `:focus-within` reaches `anc` from the element `node` that
    /// holds the state: `anc` is a shadow-including inclusive ancestor of `node` and no element in
    /// the top layer (a modal dialog, a showing popover, or one kept there for its `overlay`
    /// transition) lies strictly between them. Chrome stops the chain at the innermost top-layer
    /// element, which matches but whose ancestors do not (WPT css/selectors `*-toplayer-001`,
    /// `toplayer-transition-001`, `user-action-pseudo-top-layer`).
    pub(crate) fn user_action_chain_holds(&self, anc: u32, node: u32) -> bool {
        if self.top_layer().is_empty() || anc == node {
            return self.shadow_including_inclusive_ancestor(anc, node);
        }
        let mut cur = node;
        loop {
            if cur == anc {
                return true;
            }
            if self.n(cur).is_element() && self.top_layer_rank(self.id_of(cur)).is_some() {
                return false;
            }
            let p = self.n(cur).parent;
            cur = if p != NIL {
                p
            } else {
                match &self.n(cur).kind {
                    NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
                    _ => return false,
                }
            };
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
        matches_selector_list(sel, self.element_ref(idx), &mut ctx)
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
            // A tree scope's id map gives the first element with the id in
            // the tree: the answer when it is inside the scope. (Duplicate
            // ids, a match outside the scope, a detached tree: scan.)
            match self.indexed_by_id(self.root_idx(r), id) {
                Some(NIL) => return Ok(None),
                Some(f) if f != r && self.is_inclusive_ancestor(r, f) => return Ok(Some(self.id_of(f))),
                _ => {}
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
            if self.indexed_by_id(self.root_idx(r), id) == Some(NIL) {
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
        let mut cur = self.n(r).first_child;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                let hit = match &fast {
                    Some(Simple::Tag(name, lower)) => {
                        let want = if html_doc && e.is_html() { lower } else { name };
                        e.name.local == **want
                    }
                    // The class atoms parsed when the attribute was written.
                    Some(Simple::Class(name)) => e.has_class(name),
                    Some(Simple::Attr(name, lower)) => {
                        let want = if html_doc && e.is_html() { lower } else { name };
                        e.attrs.iter().any(|a| a.name.ns == ns!() && a.name.local == **want)
                    }
                    Some(Simple::Id(_)) | None => matches_selector_list(sel, self.element_ref(cur), &mut ctx),
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
    Id(&'a Atom),
    Class(&'a Atom),
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

impl Dom {
    /// Whether `host` is a shadow host with `focused` in its shadow tree, or in a shadow tree
    /// nested in it (`:focus` on shadow hosts).
    fn shadow_host_of_focused(&self, host: u32, focused: u32) -> bool {
        let mut cur = focused;
        loop {
            let p = self.n(cur).parent;
            if p == NIL {
                return false;
            }
            match &self.n(p).kind {
                crate::node::NodeKind::Fragment(f) if f.shadow.is_some() => {
                    if f.host == host {
                        return true;
                    }
                    if f.host == NIL {
                        return false;
                    }
                    cur = f.host;
                }
                _ => cur = p,
            }
        }
    }
}
