//! pyro: `::first-letter` (CSS 2.1 5.12.2, CSS Pseudo-Elements 4 2.2).
//!
//! The first letter of a block container's first formatted line takes the style of the
//! container's `::first-letter`: the first typographic letter unit of the line's text with the
//! punctuation before and after it (Unicode Ps, Pe, Pi, Pf and Po, as CSS 2.1 and Chromium
//! include). A block container passes its first line on to its first in-flow block child, so the
//! letter may be in a descendant block; the nearest container with a `::first-letter` style is the
//! one that styles it. A table cell, inline-block, float or positioned box keeps its first line.
//! White space that does not collapse before the letter (a no-break space) is content before it,
//! so the line has no first letter, as in Gecko (Chromium skips it).
//!
//! The text keeps its node. The inline root whose first line it is wraps the letter's bytes in a
//! span of the pseudo-element's style when it builds its inline layout (`TextLayout::
//! first_letter`, read by `inline::build`). The style is on a node of its own, owned by the
//! originating element (`ElementData::first_letter`): the span's brush, so paint reads the
//! letter's colour, background and decorations as an inline element's. Its parent is the letter's
//! text's parent, from which the style inherits (the fictional tag sequence of CSS 2.1 nests the
//! pseudo-element in the innermost element holding the letter). A floated first letter (a drop
//! cap) is not supported: it is left unstyled.

use blitz_traits::node_id::NodeId;
use icu_properties::props::GeneralCategory;
use icu_properties::{CodePointMapData, CodePointMapDataBorrowed};
use markup5ever::{local_name, ns, QualName};
use style::{
    data::ElementData as StyloElementData,
    properties::ComputedValues,
    selector_parser::PseudoElement,
    servo_arc::Arc,
    shared_lock::StylesheetGuards,
    values::computed::{Display, Float},
    values::specified::box_::{DisplayInside, DisplayOutside},
};
use stylo_taffy::CopyStyleGetters;

use super::replaced::is_inline_box_element;
use crate::{node::FirstLetterSplit, BaseDocument, ElementData, Node, NodeData};

const GENERAL_CATEGORY: CodePointMapDataBorrowed<'static, GeneralCategory> = CodePointMapData::<GeneralCategory>::new();

/// Punctuation that joins a first letter (CSS 2.1: the Ps, Pe, Pi, Pf and Po classes; Chromium's
/// `IsPunctuationForFirstLetter`). Dashes (Pd) and connectors (Pc) do not.
fn is_punctuation(c: char) -> bool {
    matches!(
        GENERAL_CATEGORY.get(c),
        GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

/// A character that belongs to the letter before it (a combining mark, a joiner or a variation
/// selector), so that the letter is a whole typographic letter unit.
fn extends_letter(c: char) -> bool {
    matches!(
        GENERAL_CATEGORY.get(c),
        GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark
    ) || matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}')
}

/// What one text node holds of the first letter.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TextScan {
    /// White space only: the letter is further on.
    Empty,
    /// The first letter is these bytes of the text: leading punctuation, the letter and the
    /// punctuation after it. Text that is only punctuation is a first letter of its own.
    Letter(std::ops::Range<usize>),
    /// Punctuation then a space: no first letter here (Chromium moves on to the next text).
    Skip,
    /// White space that does not collapse (a no-break space, an em space) before the letter:
    /// content before it on the line, so there is no first letter (CSS Pseudo-Elements 4: the
    /// letter is "not preceded by any other content").
    Stop,
}

/// Where the first letter is in `text` (Chromium's `FirstLetterPseudoElement::FirstLetterLength`
/// for one text node; collapsible white space before it stays outside the letter).
pub(crate) fn scan_text(text: &str) -> TextScan {
    let mut chars = text.char_indices().peekable();
    while chars.next_if(|&(_, c)| c.is_ascii_whitespace()).is_some() {}
    let Some(&(start, first)) = chars.peek() else {
        return TextScan::Empty;
    };
    if first.is_whitespace() {
        return TextScan::Stop;
    }
    while chars.next_if(|&(_, c)| is_punctuation(c)).is_some() {}
    match chars.next() {
        None => return TextScan::Letter(start..text.len()),
        Some((_, c)) if c.is_whitespace() => return TextScan::Skip,
        Some(_) => {}
    }
    while chars.next_if(|&(_, c)| extends_letter(c)).is_some() {}
    while chars.next_if(|&(_, c)| is_punctuation(c)).is_some() {}
    let end = chars.peek().map_or(text.len(), |&(index, _)| index);
    TextScan::Letter(start..end)
}

/// The `::first-letter` style an element's cascade kept, if any.
fn eager_style(node: &Node) -> Option<Arc<ComputedValues>> {
    if !matches!(node.data, NodeData::Element(_)) {
        return None;
    }
    let data = node.try_stylo_element_data()?.get()?;
    data.styles.pseudos.get(&PseudoElement::FirstLetter).cloned()
}

/// Whether a box with this display is a block container (which `::first-letter` applies to).
fn is_block_container(display: Display) -> bool {
    match display.inside() {
        DisplayInside::Flow => display.outside() != DisplayOutside::Inline,
        DisplayInside::FlowRoot | DisplayInside::TableCell => true,
        _ => false,
    }
}

fn is_out_of_flow(style: &ComputedValues) -> bool {
    CopyStyleGetters::clone_float(style) != Float::None
        || CopyStyleGetters::clone_position(style).is_absolutely_positioned()
}

/// The element whose `::first-letter` styles the first line of the inline root `root`, with
/// that style: the root itself, or the nearest block container whose first line is the root's.
fn origin(doc: &BaseDocument, root: NodeId) -> Option<(NodeId, Arc<ComputedValues>)> {
    let mut current = root;
    loop {
        let node = &doc.nodes[current];
        let display = node.display_style()?;
        if let Some(style) = eager_style(node) {
            // A floated first letter is not supported.
            let applies = is_block_container(display) && CopyStyleGetters::clone_float(&*style) == Float::None;
            return applies.then_some((current, style));
        }
        // Only an in-flow block box passes its first line on to its container.
        let in_flow = node.primary_styles().is_some_and(|style| !is_out_of_flow(&style));
        if display.outside() != DisplayOutside::Block
            || !matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
            || !in_flow
        {
            return None;
        }
        let parent = node.layout_parent.get()?;
        let parent_node = &doc.nodes[parent];
        if !parent_node.display_style().is_some_and(is_block_container) {
            return None;
        }
        // The box must be the parent's first in-flow child.
        let children = parent_node.layout_children.borrow();
        let first = children.as_deref()?.iter().copied().find(|&child| {
            let child = &doc.nodes[child];
            child.display_style().is_none_or(|display| display != Display::None)
                && !child.primary_styles().is_some_and(|style| is_out_of_flow(&style))
        });
        if first != Some(current) {
            return None;
        }
        current = parent;
    }
}

enum Found {
    Letter(NodeId, std::ops::Range<usize>),
    /// Something that ends the search with no first letter (an atomic inline, a line break).
    Stop,
}

/// The text node and bytes of the first letter in the inline content of `root`, walked in the
/// order `inline::build` pushes it.
fn find_letter(doc: &BaseDocument, root: NodeId) -> Option<(NodeId, std::ops::Range<usize>)> {
    fn walk(doc: &BaseDocument, node_id: NodeId) -> Option<Found> {
        let node = &doc.nodes[node_id];
        match &node.data {
            NodeData::Text(data) => match scan_text(&data.content) {
                TextScan::Letter(range) => Some(Found::Letter(node_id, range)),
                TextScan::Stop => Some(Found::Stop),
                TextScan::Empty | TextScan::Skip => None,
            },
            NodeData::Element(element) | NodeData::AnonymousBlock(element) => {
                if element.name.local == local_name!("input") && element.attr(local_name!("type")) == Some("hidden") {
                    return None;
                }
                if node.is_line_break() {
                    return Some(Found::Stop);
                }
                let display = node.display_style().unwrap_or(Display::inline());
                match (display.outside(), display.inside()) {
                    (DisplayOutside::None, DisplayInside::None) => None,
                    (DisplayOutside::None, DisplayInside::Contents) => {
                        node.children.iter().find_map(|&child| crate::util::ensure_stack(|| walk(doc, child)))
                    }
                    (DisplayOutside::Inline, DisplayInside::Flow) if !is_inline_box_element(&element.name.local) => {
                        node.before()
                            .into_iter()
                            .chain(node.children.iter().copied())
                            .chain(node.after())
                            .find_map(|child| crate::util::ensure_stack(|| walk(doc, child)))
                    }
                    // Floats and positioned boxes are not on the line; any other box is an
                    // atomic inline, before which there is no first letter.
                    _ => match node.primary_styles() {
                        Some(style) if is_out_of_flow(&style) => None,
                        _ => Some(Found::Stop),
                    },
                }
            }
            _ => None,
        }
    }
    let root_node = &doc.nodes[root];
    let found = root_node
        .before()
        .into_iter()
        .chain(root_node.children.iter().copied())
        .chain(root_node.after())
        .find_map(|child| walk(doc, child))?;
    match found {
        Found::Letter(text, range) => Some((text, range)),
        Found::Stop => None,
    }
}

/// The first letter the inline root `root` styles when it builds its inline layout, creating or
/// updating the node with its style. `None` when its first line has no `::first-letter`.
pub(crate) fn split(doc: &mut BaseDocument, root: NodeId) -> Option<FirstLetterSplit> {
    let (origin_id, eager) = origin(doc, root)?;
    let (text, range) = find_letter(doc, root)?;
    let text_parent = doc.nodes[text].parent?;
    let style = if text_parent == origin_id {
        eager
    } else {
        // Inherit from the element holding the letter: the rules the cascade matched for the
        // originating element, cascaded again with that parent.
        let parent_style: Arc<ComputedValues> = (*doc.nodes[text_parent].primary_styles()?).clone();
        let read_guard = doc.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        doc.stylist.compute_pseudo_element_style_with_inputs(
            style::context::CascadeInputs::new_from_style(&eager),
            &PseudoElement::FirstLetter,
            &guards,
            Some(&*parent_style),
            Some(&doc.nodes[origin_id]),
        )
    };
    let style_node = style_node(doc, origin_id, text_parent, style);
    Some(FirstLetterSplit { text, range, style_node })
}

/// The originating element's first-letter node, created when it has none, with `style` and the
/// letter's text's parent as its parent.
fn style_node(doc: &mut BaseDocument, origin_id: NodeId, parent: NodeId, style: Arc<ComputedValues>) -> NodeId {
    const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("span") };
    let node_id = match doc.nodes[origin_id].element_data().and_then(|element| element.first_letter) {
        Some(node_id) if doc.nodes.contains_key(node_id) => node_id,
        _ => {
            let node_id = doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(NAME, Vec::new()))));
            if let Some(element) = doc.nodes[origin_id].element_data_mut() {
                element.first_letter = Some(node_id);
            }
            node_id
        }
    };
    let node = &mut doc.nodes[node_id];
    node.parent = Some(parent);
    let unchanged = node.primary_styles().is_some_and(|old| std::ptr::eq::<ComputedValues>(&**old, &*style));
    if !unchanged {
        let mut data = StyloElementData::default();
        data.styles.primary = Some(style);
        data.set_restyled();
        *node.stylo_element_data_mut().ensure_init_mut() = data;
        node.update_layout_styles();
    }
    node_id
}

/// An element's `::first-letter` style came, went or changed in the style traversal: the inline
/// root holding its first letter builds its inline layout again, reached from the letter's text's
/// parent (or, before the element had a first letter, from the block its first line is in).
pub(crate) fn style_changed(doc: &mut BaseDocument, origin_id: NodeId) {
    let Some(node) = doc.nodes.get(origin_id) else {
        return;
    };
    let has_style = eager_style(node).is_some();
    let target = match node.element_data().and_then(|element| element.first_letter) {
        Some(style_node) if doc.nodes.contains_key(style_node) => doc.nodes[style_node].parent,
        _ => has_style.then(|| first_line_block(doc, origin_id)),
    };
    if let Some(target) = target.filter(|&id| doc.nodes.contains_key(id)) {
        doc.nodes[target].insert_damage(super::damage::ALL_DAMAGE);
        doc.nodes[target].mark_damaged();
    }
    if !has_style {
        drop_style_node(doc, origin_id);
    }
}

/// The deepest block on the chain of first in-flow block children from `block`: where its first
/// line is.
fn first_line_block(doc: &BaseDocument, block: NodeId) -> NodeId {
    let mut current = block;
    'down: loop {
        for &child_id in doc.nodes[current].children.iter() {
            let child = &doc.nodes[child_id];
            match &child.data {
                NodeData::Text(_) if child.is_collapsible_whitespace_node() => continue,
                NodeData::Element(_) => {
                    let Some(style) = child.primary_styles() else { continue };
                    let display = style.clone_display();
                    if display == Display::None || is_out_of_flow(&style) {
                        continue;
                    }
                    if display.outside() == DisplayOutside::Block && is_block_container(display) {
                        current = child_id;
                        continue 'down;
                    }
                    return current;
                }
                NodeData::Text(_) => return current,
                _ => continue,
            }
        }
        return current;
    }
}

/// Drops an element's first-letter node.
pub(crate) fn drop_style_node(doc: &mut BaseDocument, origin_id: NodeId) {
    let Some(node_id) = doc
        .nodes
        .get_mut(origin_id)
        .and_then(|node| node.element_data_mut())
        .and_then(|element| element.first_letter.take())
    else {
        return;
    };
    if doc.nodes.contains_key(node_id) {
        doc.remove_node_from_tree(node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{scan_text, TextScan};

    #[test]
    fn letter_with_punctuation() {
        assert_eq!(scan_text(")T)est"), TextScan::Letter(0..3));
        assert_eq!(scan_text("  \"This\""), TextScan::Letter(2..4));
        assert_eq!(scan_text("42 Filler"), TextScan::Letter(0..1));
        assert_eq!(scan_text("\u{00BF}T\u{00BF}est"), TextScan::Letter(0..5));
        // Dashes and connectors are not first-letter punctuation.
        assert_eq!(scan_text("-T-est"), TextScan::Letter(0..1));
        assert_eq!(scan_text("\u{2019}T\u{2019}x"), TextScan::Letter(0..7));
    }

    #[test]
    fn white_space_and_punctuation_only() {
        assert_eq!(scan_text(" \n "), TextScan::Empty);
        assert_eq!(scan_text("\"("), TextScan::Letter(0..2));
        assert_eq!(scan_text("\" T"), TextScan::Skip);
        // A no-break or em space is content before the letter.
        assert_eq!(scan_text("\u{A0}A"), TextScan::Stop);
        assert_eq!(scan_text(" \u{2003}"), TextScan::Stop);
    }

    #[test]
    fn letter_keeps_its_marks() {
        // e + combining acute accent.
        assert_eq!(scan_text("e\u{0301}tude"), TextScan::Letter(0..3));
    }
}
