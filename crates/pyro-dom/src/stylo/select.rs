//! The DOM's selector APIs (`querySelector[All]`, `matches`, `closest`)
//! through Stylo's `dom_apis`: the selector engine the cascade matches with,
//! over the same element handles.

use std::sync::LazyLock;

use selectors::SelectorList;
use style::dom::{TElement, TNode};
use style::dom_apis::{self, MayUseInvalidation, QueryAll, QueryFirst};
use style::selector_parser::{SelectorImpl, SelectorParser};
use style::shared_lock::SharedRwLock;

use super::node::{Cx, SNode, Store};
use crate::dom::Dom;
use crate::node::{NodeKind, QuirksMode, NIL};

/// A selector list parsed by Stylo.
pub type List = SelectorList<SelectorImpl>;

/// Parses `text` as Stylo does for `querySelector` (author origin, no
/// namespace prefixes); `None` when it is not a valid selector list.
pub fn parse(text: &str) -> Option<List> {
    SelectorParser::parse_author_origin_no_namespace(text, &super::url_data()).ok()
}

/// Matching outside a styling pass (handles carry the context): no style
/// data, no open popovers.
fn with_cx<R>(dom: &Dom, idx: u32, f: impl FnOnce(&Cx) -> R) -> R {
    static LOCK: LazyLock<SharedRwLock> = LazyLock::new(SharedRwLock::new);
    thread_local! {
        static STORE: Store = Store::default();
        static POPOVERS: rustc_hash::FxHashSet<u32> = Default::default();
    }
    let quirks = match &dom.n(dom.n(idx).owner_doc).kind {
        NodeKind::Document(d) => match d.quirks {
            QuirksMode::Quirks => style::context::QuirksMode::Quirks,
            QuirksMode::LimitedQuirks => style::context::QuirksMode::LimitedQuirks,
            QuirksMode::NoQuirks => style::context::QuirksMode::NoQuirks,
        },
        _ => style::context::QuirksMode::NoQuirks,
    };
    STORE.with(|store| {
        POPOVERS.with(|popovers| {
            let cx = Cx::new(dom, store, &LOCK, quirks, popovers);
            f(&cx)
        })
    })
}

/// The elements under `root` (a document, fragment or element; descendants
/// only) matching `list`, in tree order; at most one when `first`.
pub(crate) fn query(dom: &Dom, root: u32, list: &List, first: bool) -> Vec<u32> {
    with_cx(dom, root, |cx| {
        let root = SNode::with_cx(cx, root);
        if first {
            let mut out = None;
            dom_apis::query_selector::<SNode<&Cx>, QueryFirst>(root, list, &mut out, MayUseInvalidation::No);
            out.map(|e| e.as_node().opaque().0 as u32).into_iter().collect()
        } else {
            let mut out = Default::default();
            dom_apis::query_selector::<SNode<&Cx>, QueryAll>(root, list, &mut out, MayUseInvalidation::No);
            out.iter().map(|e: &SNode<&Cx>| e.as_node().opaque().0 as u32).collect()
        }
    })
}

/// `Element.matches`.
pub(crate) fn matches(dom: &Dom, el: u32, list: &List) -> bool {
    with_cx(dom, el, |cx| dom_apis::element_matches(&SNode::with_cx(cx, el), list, cx.quirks))
}

/// `Element.closest`.
pub(crate) fn closest(dom: &Dom, el: u32, list: &List) -> Option<u32> {
    with_cx(dom, el, |cx| {
        dom_apis::element_closest(SNode::with_cx(cx, el), list, cx.quirks).map(|e| e.as_node().opaque().0 as u32)
    })
    .filter(|&i| i != NIL)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use style_dom::ElementState;

    use super::*;
    use crate::parse::{parse_document, DomRef};

    #[test]
    fn element_state_carries_every_state_the_matcher_answers() {
        // Stylo's style sharing and state invalidation compare only
        // `TElement::state()` for state pseudo-classes, so the bits must
        // follow what `:in-range`/`:out-of-range` match.
        let dom_ref: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(
            &dom_ref,
            "http://test/",
            "<!doctype html><input id=a type=number min=0 max=10 value=5><input id=b type=number min=0 max=10 value=50>",
        );
        let dom = dom_ref.borrow();
        let state = |id: &str| {
            let el = dom.get_element_by_id(doc, id).unwrap().unwrap();
            let i = dom.check(el).unwrap();
            with_cx(&dom, i, |cx| SNode::with_cx(cx, i).state())
        };
        let (a, b) = (state("a"), state("b"));
        assert!(a.contains(ElementState::INRANGE) && !a.contains(ElementState::OUTOFRANGE), "{a:?}");
        assert!(b.contains(ElementState::OUTOFRANGE) && !b.contains(ElementState::INRANGE), "{b:?}");
    }
}
