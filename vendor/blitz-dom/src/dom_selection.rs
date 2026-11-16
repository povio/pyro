//! pyro: the page's selection, a DOM range the embedder keeps (pyro-webapi's document
//! selection), painted as highlighted text. The embedder sets it before a paint
//! ([`BaseDocument::set_dom_selection`]); blitz-paint reads it as the inline roots' selected byte
//! ranges ([`BaseDocument::get_text_selection_ranges`]), as it reads blitz's own selection.

use blitz_traits::node_id::NodeId;
use std::collections::HashMap;

use crate::{BaseDocument, NodeData};

/// A selection's ends: text nodes, each with an offset in UTF-16 code units of its data, the
/// start first in tree order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomSelection {
    pub start: (NodeId, usize),
    pub end: (NodeId, usize),
}

impl BaseDocument {
    /// pyro: whether author or user rules (style sheets, style attributes, animations) give
    /// `style` a declaration of some of `longhands`. A `revert` at the highest priority declares
    /// nothing (it rolls back to the user agent's, which declares none of these for a highlight
    /// pseudo-element); `revert-layer` defers to the declarations below it. Highlight painting asks
    /// it of a `::selection` style: one without its own `color` or `background-color` is drawn in
    /// the system's highlight colours (CSS Pseudo-Elements 4 § 3.4, as Chromium).
    pub fn style_declares(
        &self,
        style: &style::properties::ComputedValues,
        longhands: &[style::properties::LonghandId],
    ) -> bool {
        use style::properties::{CSSWideKeyword, PropertyDeclaration, PropertyDeclarationId};
        use style::stylesheets::Origin;
        let guard = self.guard.read();
        longhands.iter().any(|&longhand| {
            for node in style.rules().self_and_ancestors() {
                if node.cascade_level().origin().origin() == Origin::UserAgent {
                    continue;
                }
                let Some(source) = node.style_source() else { continue };
                let Some((declaration, _)) = source.read(&guard).get(PropertyDeclarationId::Longhand(longhand)) else {
                    continue;
                };
                return match declaration {
                    PropertyDeclaration::CSSWideKeyword(wide) => match wide.keyword {
                        CSSWideKeyword::Revert => false,
                        CSSWideKeyword::RevertLayer | CSSWideKeyword::RevertRule => continue,
                        _ => true,
                    },
                    _ => true,
                };
            }
            false
        })
    }

    /// pyro: the selection painted from now on (`None`: none), in place of blitz's own.
    pub fn set_dom_selection(&mut self, selection: Option<DomSelection>) {
        self.dom_selection = selection;
    }

    /// pyro: the inline roots holding text of `selection`, each with the bytes of its text
    /// (`TextLayout::text`) the selection covers.
    pub(crate) fn dom_selection_ranges(&self, selection: DomSelection) -> Vec<(NodeId, usize, usize)> {
        // The text nodes from the start to the end, in tree order, each with the UTF-16 range of
        // its data selected (`None`: to its end).
        let mut selected: HashMap<NodeId, (usize, Option<usize>)> = HashMap::new();
        let mut stack = vec![self.root_node().id];
        let mut inside = false;
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get(id) else { continue };
            if let NodeData::Text(_) = node.data {
                let start = if id == selection.start.0 {
                    inside = true;
                    selection.start.1
                } else {
                    0
                };
                if !inside {
                    continue;
                }
                if id == selection.end.0 {
                    selected.insert(id, (start, Some(selection.end.1)));
                    break;
                }
                selected.insert(id, (start, None));
                continue;
            }
            stack.extend(node.children.iter().rev().copied());
        }
        if selected.is_empty() {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for (root_id, node) in self.nodes.iter() {
            let Some(layout) = node.element_data().and_then(|element| element.inline_layout_data.as_deref()) else {
                continue;
            };
            if !node.flags.is_in_document() {
                continue;
            }
            let (mut start, mut end) = (usize::MAX, 0);
            for (text_id, bytes) in &layout.text_nodes {
                let Some(&(from, to)) = selected.get(text_id) else { continue };
                let Some(NodeData::Text(data)) = self.nodes.get(*text_id).map(|node| &node.data) else { continue };
                let Some(produced) = layout.text.get(bytes.clone()) else { continue };
                let from = bytes.start + produced_offset(&data.content, produced, from);
                let to = bytes.start + to.map_or(produced.len(), |to| produced_offset(&data.content, produced, to));
                if from < to {
                    start = start.min(from);
                    end = end.max(to);
                }
            }
            if start < end {
                ranges.push((root_id, start, end));
            }
        }
        ranges
    }
}

/// Where `utf16` (an offset in UTF-16 code units of a text node's data, `content`) is in the
/// text the node produced in its inline layout (`produced`, its white space collapsed): in bytes
/// of `produced`. Collapsed white space maps to where it collapsed.
fn produced_offset(content: &str, produced: &str, utf16: usize) -> usize {
    if content == produced {
        let mut units = 0;
        for (index, c) in content.char_indices() {
            if units >= utf16 {
                return index;
            }
            units += c.len_utf16();
        }
        return content.len();
    }
    let mut out = produced.char_indices().peekable();
    // White space that the node before left to this node's text (its trailing collapsible white
    // space belongs to the next node, `TextLayout::text_nodes`) is not in this content.
    if content.chars().next().is_some_and(|c| !c.is_whitespace()) {
        while out.next_if(|&(_, p)| p.is_whitespace()).is_some() {}
    }
    let mut units = 0;
    for c in content.chars() {
        if units >= utf16 {
            break;
        }
        units += c.len_utf16();
        match out.peek() {
            Some(&(_, p)) if p == c || (p.is_whitespace() && c.is_whitespace()) => {
                out.next();
            }
            // Collapsed white space, or text the layout changed (`text-transform`).
            _ if c.is_whitespace() => {}
            Some(_) => {
                out.next();
            }
            None => break,
        }
    }
    out.peek().map_or(produced.len(), |&(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::produced_offset;

    #[test]
    fn offsets_follow_collapsed_white_space() {
        assert_eq!(produced_offset("abc", "abc", 2), 2);
        // UTF-16: 'é' is one unit, two bytes; '𝄞' two units, four bytes.
        assert_eq!(produced_offset("é𝄞x", "é𝄞x", 3), 6);
        // "\n  Lorem  ipsum" collapsed to "Lorem ipsum".
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 3), 0);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 8), 5);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 10), 6);
        assert_eq!(produced_offset("\n  Lorem  ipsum", "Lorem ipsum", 15), 11);
    }

    #[test]
    fn offsets_skip_white_space_of_the_node_before() {
        // "Text <span>sample</span>": the span's text produced " sample".
        assert_eq!(produced_offset("sample", " sample", 0), 1);
        assert_eq!(produced_offset("sample", " sample", 5), 6);
        assert_eq!(produced_offset("sample", " sample", 6), 7);
    }
}
