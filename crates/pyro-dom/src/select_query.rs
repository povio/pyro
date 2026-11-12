//! Blink's `SelectorQuery` fast paths for the DOM's selector APIs. A selector
//! list made only of type, universal, id, class and attribute tests joined by
//! descendant and child combinators is compiled once, when its text is parsed,
//! into tests on what elements already store (name atoms, the parsed `id` and
//! `class` atoms). `matches`, `closest` and `querySelector(All)` run it with
//! one small context per call instead of the `selectors` machinery (a matching
//! context, selector caches and trait dispatch per element), and a query
//! whose selector names an id walks only the subtree that can match.

use markup5ever::{ns, LocalName};
use selectors::attr::{AttrSelectorOperator, CaseSensitivity, ParsedCaseSensitivity};
use selectors::parser::{Combinator, Component, SelectorImpl, SelectorList};

use crate::dom::Dom;
use crate::node::{Atom, DocumentKind, ElementData, Node, NodeKind, QuirksMode, NIL};

/// A selector implementation whose parsed selectors this module compiles:
/// pyro-dom's and Stylo's, which store the same atoms.
pub(crate) trait Source: SelectorImpl {
    fn ident(i: &Self::Identifier) -> &Atom;
    fn local(n: &Self::LocalName) -> &LocalName;
    fn value(v: &Self::AttrValue) -> &str;
}

impl Source for crate::select::Impl {
    fn ident(i: &Self::Identifier) -> &Atom {
        &i.0
    }
    fn local(n: &Self::LocalName) -> &LocalName {
        &n.0
    }
    fn value(v: &Self::AttrValue) -> &str {
        &v.0
    }
}

impl Source for style::selector_parser::SelectorImpl {
    fn ident(i: &Self::Identifier) -> &Atom {
        &i.0
    }
    fn local(n: &Self::LocalName) -> &LocalName {
        &n.0
    }
    fn value(v: &Self::AttrValue) -> &str {
        v.as_ref()
    }
}

/// A type selector's or attribute selector's name: as written and in ASCII
/// lowercase (what HTML elements in HTML documents are compared with).
struct Name {
    name: LocalName,
    lower: LocalName,
    mixed_case: bool,
}

impl Name {
    fn new(name: &LocalName, lower: &LocalName) -> Name {
        Name { name: name.clone(), lower: lower.clone(), mixed_case: name != lower }
    }
    /// The name an element is compared with (`selectors::matching::select_name`);
    /// `html`: it is an HTML element in an HTML document.
    #[inline]
    fn pick(&self, html: impl FnOnce() -> bool) -> &LocalName {
        if !self.mixed_case || html() {
            &self.lower
        } else {
            &self.name
        }
    }
}

enum AttrTest {
    /// `[name]`.
    Exists(Name),
    /// `[name op value]`; `name` is lowercase (the only form `selectors`
    /// parses as `AttributeInNoNamespace`).
    Value { name: LocalName, op: AttrSelectorOperator, value: Box<str>, case: ParsedCaseSensitivity },
}

/// The simple selectors of one compound selector, all of which must hold.
#[derive(Default)]
struct Compound {
    local: Option<Name>,
    id: Option<Atom>,
    classes: Box<[Atom]>,
    attrs: Box<[AttrTest]>,
    /// The combinator to the compound on the left: the parent must match it
    /// (`>`), or an ancestor (descendant).
    child: bool,
    /// What the type and class tests need of an element's summary
    /// (`crate::dom::summary`): the name's hash (a name compared as written
    /// in some elements and lowercased in others has none) and the bits of
    /// its classes.
    sum_name: Option<u32>,
    sum_classes: u32,
}

impl Compound {
    fn seal(&mut self) {
        self.sum_name = self.local.as_ref().filter(|n| !n.mixed_case).map(|n| crate::dom::name_hash(&n.lower));
        self.sum_classes = self.classes.iter().fold(0, |m, c| m | crate::dom::class_bit(c));
    }
}

/// One complex selector: its compounds from the subject leftwards.
struct Complex {
    compounds: Box<[Compound]>,
}

/// A compiled selector list (see the module documentation).
pub struct FastList {
    selectors: Box<[Complex]>,
    /// Some test depends on the document: an id or class (case-insensitive
    /// in quirks mode), a mixed-case name or a value compared as HTML says.
    needs_doc: bool,
    /// One selector with an id: the subtree a query walks follows from the
    /// element with it (`subject`: the subject compound has it).
    anchor: Option<(Atom, bool)>,
}

/// Compiles `list` when every selector in it is of the supported form.
pub(crate) fn compile<I: Source>(list: &SelectorList<I>) -> Option<FastList> {
    let mut selectors = Vec::with_capacity(list.len());
    let mut needs_doc = false;
    for sel in list.slice() {
        let mut compounds = Vec::new();
        let mut cur = Compound::default();
        let (mut classes, mut attrs) = (Vec::new(), Vec::new());
        for c in sel.iter_raw_match_order() {
            match c {
                Component::LocalName(n) => {
                    let name = Name::new(I::local(&n.name), I::local(&n.lower_name));
                    needs_doc |= name.mixed_case;
                    if cur.local.replace(name).is_some() {
                        return None;
                    }
                }
                Component::ExplicitUniversalType => {}
                Component::ID(id) => {
                    needs_doc = true;
                    if cur.id.replace(I::ident(id).clone()).is_some() {
                        return None;
                    }
                }
                Component::Class(class) => {
                    needs_doc = true;
                    classes.push(I::ident(class).clone());
                }
                Component::AttributeInNoNamespaceExists { local_name, local_name_lower } => {
                    let name = Name::new(I::local(local_name), I::local(local_name_lower));
                    needs_doc |= name.mixed_case;
                    attrs.push(AttrTest::Exists(name));
                }
                Component::AttributeInNoNamespace { local_name, operator, value, case_sensitivity } => {
                    needs_doc |=
                        *case_sensitivity == ParsedCaseSensitivity::AsciiCaseInsensitiveIfInHtmlElementInHtmlDocument;
                    attrs.push(AttrTest::Value {
                        name: I::local(local_name).clone(),
                        op: *operator,
                        value: I::value(value).into(),
                        case: *case_sensitivity,
                    });
                }
                Component::Combinator(comb @ (Combinator::Child | Combinator::Descendant)) => {
                    cur.child = *comb == Combinator::Child;
                    cur.classes = std::mem::take(&mut classes).into();
                    cur.attrs = std::mem::take(&mut attrs).into();
                    cur.seal();
                    compounds.push(std::mem::take(&mut cur));
                }
                _ => return None,
            }
        }
        cur.classes = classes.into();
        cur.attrs = attrs.into();
        cur.seal();
        compounds.push(cur);
        selectors.push(Complex { compounds: compounds.into() });
    }
    let anchor = match &selectors[..] {
        [one] => one.compounds.iter().enumerate().find_map(|(k, c)| Some((c.id.clone()?, k == 0))),
        _ => None,
    };
    Some(FastList { selectors: selectors.into(), needs_doc, anchor })
}

/// What one call's tests need from the document, read once.
#[derive(Clone, Copy)]
pub(crate) struct Cx<'a> {
    dom: &'a Dom,
    /// Ids and classes compare ASCII case-insensitively.
    quirks: bool,
    /// An HTML document: its HTML elements compare names in lowercase.
    html_doc: bool,
}

/// How a compound failed to match from an element (`selectors`'
/// `SelectorMatchingResult` for descendant and child combinators).
#[derive(PartialEq)]
enum Res {
    Matched,
    /// Try the next ancestor for the closest descendant combinator.
    Restart,
    /// No ancestor can match: stop.
    Global,
}

impl<'a> Cx<'a> {
    /// The context for `list` on nodes of the document `doc`.
    #[inline]
    pub(crate) fn new(dom: &'a Dom, list: &FastList, doc: u32) -> Cx<'a> {
        if !list.needs_doc {
            return Cx { dom, quirks: false, html_doc: false };
        }
        match &dom.n(doc).kind {
            NodeKind::Document(d) => {
                Cx { dom, quirks: d.quirks == QuirksMode::Quirks, html_doc: d.kind == DocumentKind::Html }
            }
            _ => Cx { dom, quirks: false, html_doc: false },
        }
    }

    /// Whether element node `n` (its data `e`) matches a selector of `list`.
    #[inline]
    pub(crate) fn matches(&self, list: &FastList, n: &Node, e: &ElementData) -> bool {
        list.selectors.iter().any(|sel| self.complex(sel, n, e))
    }

    /// Whether the summary `sum` of an element proves that no selector of
    /// `list` matches it.
    #[inline]
    pub(crate) fn rejects_all(&self, list: &FastList, sum: u32) -> bool {
        list.selectors.iter().all(|sel| sel.compounds.first().is_none_or(|subject| self.rejects(subject, sum)))
    }

    /// Whether the summary `sum` of an element proves that `c` fails on it.
    #[inline(always)]
    fn rejects(&self, c: &Compound, sum: u32) -> bool {
        if let Some(h) = c.sum_name {
            if sum & crate::dom::SUM_NAME != h {
                return true;
            }
        }
        // Quirks mode compares classes ASCII case-insensitively.
        !self.quirks && sum & c.sum_classes != c.sum_classes
    }

    #[inline]
    fn complex(&self, sel: &Complex, n: &Node, e: &ElementData) -> bool {
        let Some((subject, rest)) = sel.compounds.split_first() else { return false };
        self.compound(subject, e) && (rest.is_empty() || self.ancestors(subject.child, rest, n) == Res::Matched)
    }

    /// `rest[0]` (and what follows it) against the parent of `node` (`child`)
    /// or its ancestors, nearest first. A shadow root or document ends the
    /// walk: these selectors have no `:host`. The walk reads the dense walk
    /// array (`crate::dom::Walk`), and an ancestor's data only when its
    /// summary does not rule out `rest[0]`.
    fn ancestors(&self, child: bool, rest: &[Compound], node: &Node) -> Res {
        let (c, more) = rest.split_first().expect("a compound");
        let mut parent = node.parent;
        loop {
            if parent == NIL {
                return Res::Global;
            }
            let w = self.dom.w(parent);
            if w.sum == 0 {
                // No element: a document or shadow root (or fragment).
                return Res::Global;
            }
            if !self.rejects(c, w.sum) {
                let node = self.dom.n(parent);
                let NodeKind::Element(e) = &node.kind else { return Res::Global };
                if self.compound(c, e) {
                    let r = if more.is_empty() { Res::Matched } else { self.ancestors(c.child, more, node) };
                    if r != Res::Restart {
                        return r;
                    }
                }
            }
            if child {
                return Res::Restart;
            }
            parent = w.parent;
        }
    }

    /// The tests of `c` on `e`: inline, so a universal compound reads
    /// nothing of the element and a type test is one atom compare.
    #[inline(always)]
    fn compound(&self, c: &Compound, e: &ElementData) -> bool {
        let html = || self.html_doc && e.is_html();
        if let Some(n) = &c.local {
            if e.name.local != *n.pick(html) {
                return false;
            }
        }
        if let Some(id) = &c.id {
            match e.id_atom() {
                Some(own) if own == id || (self.quirks && own.eq_ignore_ascii_case(id)) => {}
                _ => return false,
            }
        }
        if !c.classes.is_empty() {
            let own = e.classes();
            for class in &*c.classes {
                if !own.contains(class) && !(self.quirks && own.iter().any(|o| o.eq_ignore_ascii_case(class))) {
                    return false;
                }
            }
        }
        c.attrs.is_empty() || self.attrs(c, e)
    }

    #[inline(never)]
    fn attrs(&self, c: &Compound, e: &ElementData) -> bool {
        c.attrs.iter().all(|t| attr_matches(t, e, || self.html_doc && e.is_html()))
    }
}

fn attr_matches(t: &AttrTest, e: &ElementData, html: impl Fn() -> bool) -> bool {
    match t {
        AttrTest::Exists(n) => {
            let want = n.pick(html);
            e.attrs.iter().any(|a| a.name.local == *want && a.name.ns == ns!())
        }
        AttrTest::Value { name, op, value, case } => {
            let case = match case {
                ParsedCaseSensitivity::CaseSensitive | ParsedCaseSensitivity::ExplicitCaseSensitive => {
                    CaseSensitivity::CaseSensitive
                }
                ParsedCaseSensitivity::AsciiCaseInsensitive => CaseSensitivity::AsciiCaseInsensitive,
                ParsedCaseSensitivity::AsciiCaseInsensitiveIfInHtmlElementInHtmlDocument if html() => {
                    CaseSensitivity::AsciiCaseInsensitive
                }
                ParsedCaseSensitivity::AsciiCaseInsensitiveIfInHtmlElementInHtmlDocument => {
                    CaseSensitivity::CaseSensitive
                }
            };
            e.attrs.iter().any(|a| a.name.local == *name && a.name.ns == ns!() && op.eval_str(&a.value, value, case))
        }
    }
}

impl Dom {
    /// `Element.matches` for a compiled list on node `n`; `None` when it is
    /// no element.
    pub(crate) fn fast_matches(&self, n: &Node, list: &FastList) -> Option<bool> {
        let NodeKind::Element(e) = &n.kind else { return None };
        Some(Cx::new(self, list, n.owner_doc).matches(list, n, e))
    }

    /// `Element.closest` for a compiled list on node `el` (`n`).
    pub(crate) fn fast_closest(&self, el: u32, n: &Node, list: &FastList) -> Option<u32> {
        let cx = Cx::new(self, list, n.owner_doc);
        let mut cur = el;
        let mut node = n;
        while let NodeKind::Element(e) = &node.kind {
            if cx.matches(list, node, e) {
                return Some(cur);
            }
            cur = node.parent;
            if cur == NIL {
                break;
            }
            node = self.n(cur);
        }
        None
    }

    /// Calls `found` for each element below `r` matching `list`, in tree
    /// order, while it returns true (`first_only`: it returns false on the
    /// first, as `querySelector`'s does).
    pub(crate) fn fast_select(&self, r: u32, list: &FastList, first_only: bool, mut found: impl FnMut(u32) -> bool) {
        let cx = Cx::new(self, list, self.n(r).owner_doc);
        let mut start = r;
        if let (Some((id, subject)), false) = (&list.anchor, cx.quirks) {
            // The tree scope's id index: where the elements with the id are.
            // (Several elements with it, or a tree without an index: walk.)
            match self.indexed_by_id_count(self.root_idx(r), id) {
                None => {}
                Some((_, 0)) => return,
                Some((e, count)) if *subject && (count == 1 || first_only) => {
                    // The one element that can match; or, of several with the
                    // id, the first in the scope: every match has the id, so
                    // that one is the first match when it is below `r` and
                    // matches (otherwise walk).
                    if e != r && self.is_inclusive_ancestor(r, e) {
                        let n = self.n(e);
                        if let NodeKind::Element(data) = &n.kind {
                            if cx.matches(list, n, data) {
                                found(e);
                                return;
                            }
                        }
                    }
                    if count == 1 {
                        return;
                    }
                }
                Some((e, 1)) => {
                    // Every match is below the one element with the id.
                    if self.is_inclusive_ancestor(r, e) {
                        start = e;
                    } else if !self.is_inclusive_ancestor(e, r) {
                        return;
                    }
                }
                Some(_) => {}
            }
        }
        // The walk reads the dense walk array (`crate::dom::Walk`), and a
        // node only when it is an element its summary does not rule out.
        let mut cur = self.w(start).first_child;
        while cur != NIL {
            let w = self.w(cur);
            if w.sum != 0 && !cx.rejects_all(list, w.sum) {
                let n = self.n(cur);
                if let NodeKind::Element(e) = &n.kind {
                    if cx.matches(list, n, e) && !found(cur) {
                        return;
                    }
                }
            }
            // The pre-order successor within `start`, from the entry in hand.
            if w.first_child != NIL {
                cur = w.first_child;
                continue;
            }
            let mut c = cur;
            let mut cw = w;
            cur = loop {
                if c == start {
                    break NIL;
                }
                if cw.next != NIL {
                    break cw.next;
                }
                c = cw.parent;
                if c == NIL {
                    break NIL;
                }
                cw = self.w(c);
            };
        }
    }
}
