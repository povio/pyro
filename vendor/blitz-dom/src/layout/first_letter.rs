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
//! cap) is a box as an initial letter's (below), a block of the letter's style floated as it says,
//! placed by the first line as any float in it (`FirstLetterSplit::floated`).
//!
//! An `initial-letter` (CSS Inline 3 § 7, as Blink lays it out) makes the style node the letter's
//! box: a block of its own holding a copy of the letter's text, whose font size makes the
//! letter's cap height span `size` lines of the paragraph. The root's text leaves the letter out
//! and holds the box instead, as a float-like inline box at the letter's place; the first line
//! places it (`inline::layout`): at the line's start, aligned to its lines, sized by the ink of
//! its glyphs, an exclusion that the lines beside it and later floats keep out of.

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

use super::replaced::is_inline_box;
use crate::{
    node::{FirstLetterSplit, InitialLetter, NodeFlags},
    BaseDocument, ElementData, Node, NodeData,
};

const GENERAL_CATEGORY: CodePointMapDataBorrowed<'static, GeneralCategory> = CodePointMapData::<GeneralCategory>::new();

/// Punctuation that joins a first letter before it (pyro: CSS Pseudo 4 § 2.2: any P* class,
/// dashes and connectors too; WPT css-pseudo `first-letter-trailing-punctuation`).
fn is_punctuation(c: char) -> bool {
    matches!(
        GENERAL_CATEGORY.get(c),
        GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::ConnectorPunctuation
    )
}

/// pyro: punctuation that may follow the first letter: all but opening punctuation and dashes.
fn is_trailing_punctuation(c: char) -> bool {
    is_punctuation(c)
        && !matches!(GENERAL_CATEGORY.get(c), GeneralCategory::OpenPunctuation | GeneralCategory::DashPunctuation)
}

/// pyro: a space separator other than U+0020 (typographic space, CSS Pseudo 4 § 2.2).
fn is_typographic_space(c: char) -> bool {
    c != ' ' && GENERAL_CATEGORY.get(c) == GeneralCategory::SpaceSeparator
}

/// pyro: a word-separator character (CSS Text 3 § 8.2).
fn is_word_separator(c: char) -> bool {
    matches!(c, ' ' | '\u{A0}' | '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}')
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
    // pyro: a punctuation mark keeps the combining marks after it (WPT css-pseudo
    // `first-letter-004`).
    // pyro: so do typographic spaces between the punctuation and the letter, other than a plain
    // space (WPT css-pseudo `first-letter-trailing-punctuation`: "«&nbsp;T").
    while chars.next_if(|&(_, c)| is_punctuation(c)).is_some() {
        while chars.next_if(|&(_, c)| extends_letter(c)).is_some() {}
        while chars.next_if(|&(_, c)| is_typographic_space(c)).is_some() {}
    }
    match chars.next() {
        None => return TextScan::Letter(start..text.len()),
        Some((_, c)) if c.is_whitespace() => return TextScan::Skip,
        Some(_) => {}
    }
    while chars.next_if(|&(_, c)| extends_letter(c)).is_some() {}
    // pyro: punctuation after the letter, except opening punctuation and dashes; a typographic
    // space that is not a word separator bridges to more of it, but is not taken alone.
    loop {
        if chars.next_if(|&(_, c)| is_trailing_punctuation(c)).is_some() {
            while chars.next_if(|&(_, c)| extends_letter(c)).is_some() {}
            continue;
        }
        let mut ahead = chars.clone();
        let mut spaces = 0;
        while ahead.next_if(|&(_, c)| is_typographic_space(c) && !is_word_separator(c)).is_some() {
            spaces += 1;
        }
        if spaces > 0 && ahead.peek().is_some_and(|&(_, c)| is_trailing_punctuation(c)) {
            chars = ahead;
            continue;
        }
        break;
    }
    let end = chars.peek().map_or(text.len(), |&(index, _)| index);
    TextScan::Letter(start..end)
}

/// The `::first-letter` style an element's cascade kept, if any.
fn eager_style(node: &Node) -> Option<Arc<ComputedValues>> {
    eager_pseudo(node, &PseudoElement::FirstLetter)
}

/// The style of an eagerly cascaded pseudo-element (`pseudo`) an element's cascade kept, if any.
fn eager_pseudo(node: &Node, pseudo: &PseudoElement) -> Option<Arc<ComputedValues>> {
    if !matches!(node.data, NodeData::Element(_)) {
        return None;
    }
    let data = node.try_stylo_element_data()?.get()?;
    data.styles.pseudos.get(pseudo).cloned()
}

/// Whether a box with this display is a block container (which `::first-letter` applies to).
fn is_block_container(display: Display) -> bool {
    match display.inside() {
        DisplayInside::Flow => display.outside() != DisplayOutside::Inline,
        DisplayInside::FlowRoot | DisplayInside::TableCell => true,
        _ => false,
    }
}

pub(super) fn is_out_of_flow(style: &ComputedValues) -> bool {
    CopyStyleGetters::clone_float(style) != Float::None
        || CopyStyleGetters::clone_position(style).is_absolutely_positioned()
}

/// The element whose `::first-letter` (pyro: or `::first-line`, `pseudo`) styles the first line
/// of the inline root `root`, with that style: the root itself, or the nearest block container
/// whose first line is the root's.
pub(super) fn origin(
    doc: &BaseDocument,
    root: NodeId,
    pseudo: &PseudoElement,
) -> Option<(NodeId, Arc<ComputedValues>)> {
    let mut current = root;
    loop {
        let node = &doc.nodes[current];
        let display = node.display_style()?;
        // pyro: an inline split around block-level boxes (`NodeFlags::IS_SPLIT_INLINE`) holds the
        // anonymous blocks of its pieces in the block it is in: its first line is that block's.
        let split = node.is_split_inline();
        if let Some(style) = eager_pseudo(node, pseudo).filter(|_| !split) {
            // A floated first letter is not supported. pyro: an initial letter is (its
            // `float` does not apply).
            let applies = is_block_container(display);
            return applies.then_some((current, style));
        }
        // Only an in-flow block box passes its first line on to its container. pyro: an
        // anonymous block is one, whatever display its anonymous style computes.
        let in_flow = node.primary_styles().is_some_and(|style| !is_out_of_flow(&style));
        // pyro: a `::before`/`::after` box is not an anonymous block: its display decides (an
        // inline-block one is atomic; WPT css-pseudo `first-line-with-inline-block-before`).
        let pseudo_box = node.parent.is_some_and(|parent| {
            let parent = &doc.nodes[parent];
            parent.before() == Some(current) || parent.after() == Some(current)
        });
        let block = (node.is_anonymous() && !pseudo_box)
            || (display.outside() == DisplayOutside::Block
                && matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot));
        if !split && (!block || !in_flow) {
            return None;
        }
        let parent = node.layout_parent.get()?;
        let parent_node = &doc.nodes[parent];
        if !parent_node.is_split_inline() && !parent_node.display_style().is_some_and(is_block_container) {
            return None;
        }
        // The box must be the parent's first in-flow child.
        if !is_first_in_flow_child(doc, parent, current) {
            return None;
        }
        current = parent;
    }
}

/// Whether `child` is the first in-flow box among the layout children of `parent`.
pub(super) fn is_first_in_flow_child(doc: &BaseDocument, parent: NodeId, child: NodeId) -> bool {
    let children = doc.nodes[parent].layout_children.borrow();
    let first = children.as_deref().and_then(|children| {
        children.iter().copied().find(|&id| {
            let node = &doc.nodes[id];
            node.display_style().is_none_or(|display| display != Display::None)
                && !node.primary_styles().is_some_and(|style| is_out_of_flow(&style))
        })
    });
    first == Some(child)
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
            // pyro: a preserved line break before any letter ends the first line without one
            // (WPT css-pseudo `first-letter-with-preceding-new-line`).
            NodeData::Text(data)
                if data.content.trim_start_matches(|c: char| c.is_ascii_whitespace() && c != '\n').starts_with('\n')
                    && node.parent.and_then(|parent| doc.nodes[parent].primary_styles()).is_some_and(|style| {
                        use style::computed_values::white_space_collapse::T as Collapse;
                        matches!(
                            style.get_inherited_text().white_space_collapse,
                            Collapse::Preserve | Collapse::PreserveBreaks | Collapse::BreakSpaces
                        )
                    }) =>
            {
                Some(Found::Stop)
            }
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
                    (DisplayOutside::Inline, DisplayInside::Flow) if !is_inline_box(element) => {
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
    let (origin_id, eager) = origin(doc, root, &PseudoElement::FirstLetter)?;
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
    let letter_text = match &doc.nodes[text].data {
        NodeData::Text(data) => data.content.get(range.clone()).unwrap_or_default().to_string(),
        _ => String::new(),
    };
    let initial = initial_letter(doc, root, &style).filter(|_| !letter_text.is_empty());
    let floated = initial.is_none() && !letter_text.is_empty() && CopyStyleGetters::clone_float(&*style) != Float::None;
    let style_node = match initial {
        Some((_, font_size, text_line_height)) => {
            let rtl = doc.nodes[root]
                .primary_styles()
                .is_some_and(|root_style| root_style.clone_direction() == style::computed_values::direction::T::Rtl);
            let box_style = initial_letter_box_style(doc, &style, font_size, text_line_height, rtl);
            let box_id = style_node(doc, origin_id, text_parent, box_style);
            set_letter_text(doc, root, box_id, &letter_text);
            box_id
        }
        // pyro: a floated first letter is a floated block box of the letter.
        None if floated => {
            let mut builder = style::properties::StyleBuilder::for_derived_style(
                doc.stylist.device(),
                Some(&doc.stylist),
                &style,
                None,
            );
            builder.set_display(Display::Block);
            let box_style = builder.build();
            let box_id = style_node(doc, origin_id, text_parent, box_style);
            set_letter_text(doc, root, box_id, &letter_text);
            box_id
        }
        None => {
            let style_node = style_node(doc, origin_id, text_parent, style);
            clear_letter_text(doc, style_node);
            style_node
        }
    };
    Some(FirstLetterSplit { text, range, style_node, initial: initial.map(|(initial, _, _)| initial), floated })
}

/// pyro: whether the `::first-letter` style `letter` of the inline root `root` makes an initial
/// letter (CSS Inline 3 § 7): with what the first line needs to place it, the font size of its
/// text (its cap height spans `size` lines, Blink's `ComputeInitialLetterFont`) and the text's
/// line height (the font's height).
fn initial_letter(doc: &BaseDocument, root: NodeId, letter: &ComputedValues) -> Option<(InitialLetter, f32, f32)> {
    use style::values::computed::font::LineHeight;
    use style::values::generics::text::InitialLetterKeyword;
    let value = letter.slow_clone_initial_letter();
    if !(value.size > 0.0) {
        return None;
    }
    let root_style = doc.nodes[root].primary_styles()?;
    let centered = root_style.writing_mode.is_vertical() && !root_style.writing_mode.is_sideways();
    let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: doc.font_ctx.clone() };
    // The paragraph's first available font, its ascent and descent whole pixels as Blink's.
    let paragraph_font = root_style.get_font();
    let paragraph_size = paragraph_font.font_size.computed_size().px();
    let (ascent, descent, gap, paragraph_cap) = provider.extents_for(paragraph_font, paragraph_size, ' ')?;
    let (ascent, descent) = (ascent.round(), descent.round());
    let line_height = match root_style.clone_line_height() {
        LineHeight::Normal => ascent + descent + gap.max(0.0).round(),
        LineHeight::Number(number) => number.0 * paragraph_size,
        LineHeight::Length(length) => length.px(),
    };
    // Blink's `CalculateLeadingSpace`: the half-leading over the text floored, the rest under it.
    let leading = line_height - (ascent + descent);
    let descent = descent + (leading - (leading / 2.0).floor());
    // The letter's font size: its cap height is `size - 1` lines and the paragraph's cap height.
    let letter_font = letter.get_font();
    let letter_size = letter_font.font_size.computed_size().px();
    let (_, _, _, letter_cap) = provider.extents_for(letter_font, letter_size, ' ')?;
    if !(letter_cap > 0.0) {
        return None;
    }
    let desired_cap = line_height * (value.size - 1.0) + paragraph_cap;
    let mut font_size = desired_cap * letter_size / letter_cap;
    while font_size > 1.0 {
        let (_, _, _, cap) = provider.extents_for(letter_font, font_size, ' ')?;
        if cap <= desired_cap + 0.001 {
            break;
        }
        font_size -= 1.0;
    }
    let (text_ascent, text_descent, _, _) = provider.extents_for(letter_font, font_size, ' ')?;
    // The sink of Blink's legacy positioning: `drop` and no sink are the size rounded up.
    let sink = match value.keyword {
        InitialLetterKeyword::Raise => 1,
        InitialLetterKeyword::None if value.sink > 0 => value.sink,
        _ => value.size.ceil() as i32,
    };
    Some((
        InitialLetter { size: value.size, sink, line_height, descent, centered },
        font_size,
        text_ascent.round() + text_descent.round(),
    ))
}

/// pyro: the style of an initial letter box: its `::first-letter` style with the text's font
/// size and line height (Blink's `StyleForInitialLetterText`), a block floated to the start of
/// the line (the float placement rules do not apply to it: the first line places it).
fn initial_letter_box_style(
    doc: &BaseDocument,
    letter: &Arc<ComputedValues>,
    font_size: f32,
    line_height: f32,
    rtl: bool,
) -> Arc<ComputedValues> {
    use style::values::computed::font::{FontSize, LineHeight};
    use style::values::computed::NonNegativeLength;
    use style::values::specified::font::KeywordInfo;
    let mut builder =
        style::properties::StyleBuilder::for_derived_style(doc.stylist.device(), Some(&doc.stylist), letter, None);
    let size = NonNegativeLength::new(font_size);
    builder.set_font_size(FontSize { computed_size: size, used_size: size, keyword_info: KeywordInfo::none() });
    builder.set_line_height(LineHeight::Length(NonNegativeLength::new(line_height)));
    builder.set_display(Display::Block);
    builder.set_float(if rtl { Float::Right } else { Float::Left });
    builder.build()
}

/// pyro: the initial letter box `box_id` of the inline root `root` holds `letter` in a text node of
/// its own, and is an inline root of it.
fn set_letter_text(doc: &mut BaseDocument, root: NodeId, box_id: NodeId, letter: &str) {
    let existing = doc.nodes[box_id]
        .children
        .first()
        .copied()
        .filter(|&id| matches!(doc.nodes.get(id).map(|node| &node.data), Some(NodeData::Text(_))));
    let text_id = match existing {
        Some(text_id) => {
            if let NodeData::Text(data) = &mut doc.nodes[text_id].data
                && data.content != letter
            {
                data.content = letter.to_string();
            }
            text_id
        }
        None => {
            clear_letter_text(doc, box_id);
            let text_id = doc.create_text_node(letter);
            doc.nodes[text_id].parent = Some(box_id);
            doc.nodes[box_id].children.push(text_id);
            text_id
        }
    };
    let in_document = doc.nodes[root].flags.is_in_document();
    for id in [box_id, text_id] {
        doc.nodes[id].flags.set(NodeFlags::IS_IN_DOCUMENT, in_document);
    }
    let node = &mut doc.nodes[box_id];
    node.flags.insert(NodeFlags::IS_INLINE_ROOT | NodeFlags::IS_INITIAL_LETTER_BOX);
    *node.layout_children.borrow_mut() = Some(Default::default());
    node.layout_parent.set(Some(root));
}

/// pyro: a first-letter style node that is no initial letter box (any more) holds no text, is no
/// inline root and no layout child of its root.
fn clear_letter_text(doc: &mut BaseDocument, style_node: NodeId) {
    for child in std::mem::take(&mut doc.nodes[style_node].children) {
        doc.remove_node_from_tree(child);
    }
    if !doc.nodes[style_node].flags.contains(NodeFlags::IS_INITIAL_LETTER_BOX) {
        return;
    }
    leave_layout_parent(doc, style_node);
    let node = &mut doc.nodes[style_node];
    node.flags.remove(NodeFlags::IS_INLINE_ROOT | NodeFlags::IS_INITIAL_LETTER_BOX);
    if let Some(element) = node.element_data_mut() {
        element.inline_layout_data = None;
    }
    *node.layout_children.borrow_mut() = None;
}

/// pyro: an initial letter box is no longer among the layout children of the root it started.
fn leave_layout_parent(doc: &mut BaseDocument, box_id: NodeId) {
    if let Some(parent) = doc.nodes[box_id].layout_parent.take()
        && let Some(parent) = doc.nodes.get(parent)
        && let Some(children) = parent.layout_children.borrow_mut().as_mut()
    {
        children.retain(|&id| id != box_id);
    }
}

/// pyro: the inline root `root` starts with the initial letter box `box_id`: the box is the first
/// of the root's layout children (the boxes its lines place), and its own inline layout (the
/// letter alone) is built now, before the root's (`BaseDocument::resolve_deferred_tasks`).
pub(crate) fn build_initial_letter_box(doc: &mut BaseDocument, root: NodeId, box_id: NodeId) {
    {
        let mut children = doc.nodes[root].layout_children.borrow_mut();
        let children = children.get_or_insert_with(Default::default);
        if children.first() != Some(&box_id) {
            children.retain(|&id| id != box_id);
            children.insert(0, box_id);
        }
    }
    let scale = doc.viewport.scale();
    let mut layout = doc.nodes[box_id]
        .element_data_mut()
        .and_then(|element| element.inline_layout_data.take())
        .map(|layout| *layout)
        .unwrap_or_else(crate::node::TextLayout::new);
    layout = crate::node::TextLayout { layout: std::mem::take(&mut layout.layout), ..crate::node::TextLayout::new() };
    {
        let BaseDocument { nodes, layout_ctx, font_ctx, .. } = doc;
        let mut font_ctx = font_ctx.lock().unwrap();
        super::inline::build_inline_layout_into(nodes, layout_ctx, &mut font_ctx, &mut layout, scale, box_id);
    }
    let node = &mut doc.nodes[box_id];
    node.clear_layout_cache();
    if let Some(element) = node.element_data_mut() {
        element.inline_layout_data = Some(Box::new(layout));
    }
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
        // pyro: its background images load as an element's do.
        doc.pending_style_image_nodes.push(node_id);
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
pub(super) fn first_line_block(doc: &BaseDocument, block: NodeId) -> NodeId {
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
        // pyro: with the text of an initial letter box, out of its root's layout children.
        for child in std::mem::take(&mut doc.nodes[node_id].children) {
            doc.remove_node_from_tree(child);
        }
        if doc.nodes[node_id].flags.contains(NodeFlags::IS_INITIAL_LETTER_BOX) {
            leave_layout_parent(doc, node_id);
        }
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
        // pyro: a dash before the letter joins it, one after it does not (CSS Pseudo 4).
        assert_eq!(scan_text("-T-est"), TextScan::Letter(0..2));
        assert_eq!(scan_text("_T_est"), TextScan::Letter(0..3));
        assert_eq!(scan_text("T(rail"), TextScan::Letter(0..1));
        assert_eq!(scan_text("\u{AB}\u{A0}T\u{A0}\u{BB}est"), TextScan::Letter(0..5));
        assert_eq!(scan_text("T\u{2003}.est"), TextScan::Letter(0..5));
        assert_eq!(scan_text("T.\u{2003}est"), TextScan::Letter(0..2));
        assert_eq!(scan_text("\u{104E}\u{300}T\u{300}x"), TextScan::Letter(0..8));
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
