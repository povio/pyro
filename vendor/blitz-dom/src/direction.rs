//! pyro: the directionality of an element (HTML "The dir attribute"), for `:dir()`: `dir="ltr"`
//! or `dir="rtl"` on the element; `dir="auto"` (and a `<bdi>` without a valid `dir`) by its
//! content, which pyro-dom's render tree gives as a `dir=ltr` or `dir=rtl` token of the element's
//! state attribute (else here: the first strong character of its text, a text control's value),
//! left to right when nothing decides; a `tel` input without `dir` left to right; else its
//! parent's in its node tree (a shadow tree's first level: the host's; an assigned node: its
//! host's, not its slot's, `crate::shadow`). The root is left to right. The same rules as
//! pyro-dom's `direction.rs`, which answers `:dir()` for script.

use icu_properties::props::BidiClass;
use icu_properties::{CodePointMapData, CodePointMapDataBorrowed};
use markup5ever::{local_name, ns};

use crate::node::{Node, NodeData};

const BIDI_CLASS: CodePointMapDataBorrowed<'static, BidiClass> = CodePointMapData::<BidiClass>::new();

/// Whether element `node`'s directionality is right to left.
pub(crate) fn is_rtl(node: &Node) -> bool {
    let mut current = node;
    loop {
        if let Some(element) = current.element_data() {
            let html = element.name.ns == ns!(html);
            let dir = dir_attr(current);
            match dir {
                Some(Dir::Ltr) => return false,
                Some(Dir::Rtl) => return true,
                _ => {}
            }
            let bdi = html && element.name.local == local_name!("bdi");
            if dir == Some(Dir::Auto) || (bdi && dir.is_none()) {
                if let Some(rtl) = state_direction(current) {
                    return rtl;
                }
                return auto_direction(current).unwrap_or(false);
            }
            if html
                && element.name.local == local_name!("input")
                && dir.is_none()
                && element.attr(local_name!("type")).is_some_and(|t| t.eq_ignore_ascii_case("tel"))
            {
                return false;
            }
        }
        // The parent in the node tree; the host for the first level of a shadow tree.
        let parent = if current.parent_is_shadow_root() {
            current.parent.map(|parent| current.with(parent))
        } else {
            current.dom_parent_element()
        };
        match parent {
            Some(parent) if parent.is_element() => current = parent,
            _ => return false,
        }
    }
}

/// The directionality pyro-dom's render tree gives an element (`Some(true)`: right to left).
fn state_direction(node: &Node) -> Option<bool> {
    let state = node.element_data()?.attrs.state("state")?;
    state.split_ascii_whitespace().find_map(|token| match token {
        "dir=rtl" => Some(true),
        "dir=ltr" => Some(false),
        _ => None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Ltr,
    Rtl,
    Auto,
}

/// The valid value of `node`'s own `dir` attribute.
fn dir_attr(node: &Node) -> Option<Dir> {
    let value = node.element_data()?.attr(local_name!("dir"))?;
    [("ltr", Dir::Ltr), ("rtl", Dir::Rtl), ("auto", Dir::Auto)]
        .into_iter()
        .find(|(name, _)| value.eq_ignore_ascii_case(name))
        .map(|(_, dir)| dir)
}

/// The direction of the first strong character of `text` (`Some(true)`: right to left).
fn text_direction(text: &str) -> Option<bool> {
    text.chars().find_map(|c| match BIDI_CLASS.get(c) {
        BidiClass::LeftToRight => Some(false),
        BidiClass::RightToLeft | BidiClass::ArabicLetter => Some(true),
        _ => None,
    })
}

/// HTML "auto directionality": a text control's value's, or its contained text's.
fn auto_direction(node: &Node) -> Option<bool> {
    let element = node.element_data()?;
    if element.name.ns == ns!(html) {
        match element.name.local {
            local_name!("input") => return text_direction(element.attr(local_name!("value")).unwrap_or("")),
            local_name!("textarea") => return contained_text_direction(node),
            _ => {}
        }
    }
    contained_text_direction(node)
}

/// HTML "contained text auto directionality": the descendants' text in tree order, leaving out
/// `bdi`, `script`, `style` and `textarea` elements and those with a valid `dir` of their own.
fn contained_text_direction(node: &Node) -> Option<bool> {
    node.children.iter().find_map(|&child| {
        let child = node.with(child);
        match &child.data {
            NodeData::Text(text) => text_direction(&text.content),
            NodeData::Element(element) => {
                let skipped = (element.name.ns == ns!(html)
                    && matches!(
                        element.name.local,
                        local_name!("bdi") | local_name!("script") | local_name!("style") | local_name!("textarea")
                    ))
                    || dir_attr(child).is_some();
                if skipped { None } else { crate::util::ensure_stack(|| contained_text_direction(child)) }
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::text_direction;

    #[test]
    fn the_first_strong_character_decides() {
        assert_eq!(text_direction("123 abc"), Some(false));
        assert_eq!(text_direction("1 \u{05D0}bc"), Some(true));
        assert_eq!(text_direction("\u{0627}"), Some(true));
        assert_eq!(text_direction("12 ."), None);
    }
}
