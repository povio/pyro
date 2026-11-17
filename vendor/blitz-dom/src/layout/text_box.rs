//! pyro: `text-box-trim` and `text-box-edge` (CSS Inline 3 § 5).
//!
//! On a block container, `text-box-trim` trims the block-start side of its first formatted line
//! and the block-end side of its last formatted line to a metric of the root inline box
//! (`text-box-edge`): the half-leading and what else the line box holds beyond that metric no
//! longer takes room in the block. The inline layout of the root that holds that line shifts its
//! lines up and gives its content that much less height ([`block_trim`]). On an inline box, it
//! moves the edges of the box's content area, which its backgrounds, borders and padding go by
//! ([`span_trim`], `parley::TextStyle::text_box_trim`).

use blitz_traits::node_id::NodeId;
use style::properties::ComputedValues;
use style::values::computed::{TextBoxEdge, TextBoxTrim};
use style::values::specified::text::TextEdgeKeyword;
use style::values::specified::box_::{DisplayInside, DisplayOutside};

use super::first_letter::is_out_of_flow;
use crate::BaseDocument;

fn edge(keyword: TextEdgeKeyword) -> parley::TextEdge {
    match keyword {
        TextEdgeKeyword::Text => parley::TextEdge::Text,
        TextEdgeKeyword::Ideographic => parley::TextEdge::Ideographic,
        TextEdgeKeyword::IdeographicInk => parley::TextEdge::IdeographicInk,
        TextEdgeKeyword::Cap => parley::TextEdge::Cap,
        TextEdgeKeyword::Ex => parley::TextEdge::Ex,
        TextEdgeKeyword::Alphabetic => parley::TextEdge::Alphabetic,
    }
}

/// The over and under edges of `text-box-edge` (`auto` is `text`).
fn edges(style: &ComputedValues) -> (parley::TextEdge, parley::TextEdge) {
    match style.slow_clone_text_box_edge() {
        TextBoxEdge::Auto => (parley::TextEdge::Text, parley::TextEdge::Text),
        TextBoxEdge::TextEdge(text_edge) => (edge(text_edge.over), edge(text_edge.under)),
    }
}

/// Whether the line-over side is at the block end (`vertical-lr` with upright or turned glyphs).
fn line_inverted(style: &ComputedValues) -> bool {
    style.writing_mode.intersects(style::logical_geometry::WritingMode::LINE_INVERTED)
}

/// The trimmed content area edges of an inline box with style `style`, by line side.
pub(crate) fn span_trim(style: &ComputedValues) -> parley::TextBoxTrim {
    let trim = style.slow_clone_text_box_trim();
    if trim.is_empty() {
        return parley::TextBoxTrim::default();
    }
    let (over, under) = edges(style);
    let (start, end) = (trim.contains(TextBoxTrim::TRIM_START), trim.contains(TextBoxTrim::TRIM_END));
    let (trim_over, trim_under) = if line_inverted(style) { (end, start) } else { (start, end) };
    parley::TextBoxTrim { over: trim_over.then_some(over), under: trim_under.then_some(under) }
}

/// pyro: the sides (block-start, block-end) a multi-column container's columns trim at its column
/// breaks (horizontal writing modes only): its own (Blink: the column boxes inherit
/// `text-box-trim`).
pub(crate) fn fragment_trims(style: &ComputedValues) -> (bool, bool) {
    let trim = style.slow_clone_text_box_trim();
    (trim.contains(TextBoxTrim::TRIM_START), trim.contains(TextBoxTrim::TRIM_END))
}

/// pyro: the sides a block with style `style`, inside a multi-column container, trims at column
/// breaks, from those of its container (`outer`): on a side of its own `text-box-trim` only with
/// `box-decoration-break: clone`, on others as its container (Blink: WPT css-inline
/// `text-box-trim-multicol-004`/`005`).
pub(crate) fn fragment_trims_within(style: &ComputedValues, outer: (bool, bool)) -> (bool, bool) {
    let trim = style.slow_clone_text_box_trim();
    let clone = style.get_border().box_decoration_break == style::values::computed::BoxDecorationBreak::Clone;
    let side = |flag: TextBoxTrim, outer: bool| if trim.contains(flag) { clone } else { outer };
    (side(TextBoxTrim::TRIM_START, outer.0), side(TextBoxTrim::TRIM_END, outer.1))
}

/// pyro: what a horizontal line of an inline root with style `root_style` loses over (`start`) or
/// under its root inline box's `text-box-edge` metric when it starts or ends a column fragment
/// (layout units, from the line's block-start or block-end side).
pub(crate) fn fragment_line_trim(line: &parley::Line<'_, crate::node::TextBrush>, root_style: &ComputedValues, start: bool) -> f32 {
    let (over, under) = edges(root_style);
    let m = line.metrics();
    if start {
        (line.span_text_edge(0, over, true) - m.block_min_coord).max(0.0)
    } else {
        (m.block_max_coord - line.span_text_edge(0, under, false)).max(0.0)
    }
}

/// A side of a line trimmed for a block: the metric the line ends at, and whether that is its
/// line-over side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineTrim {
    pub edge: parley::TextEdge,
    pub over: bool,
}

/// The trims of the first line (block-start side) and of the last line (block-end side) of the
/// inline root `root`: those of the nearest block container with `text-box-trim` whose first or
/// last formatted line that line is.
pub(crate) fn block_trim(doc: &BaseDocument, root: NodeId) -> (Option<LineTrim>, Option<LineTrim>) {
    if !doc.text_box_trim_styles {
        return (None, None);
    }
    (side(doc, root, true), side(doc, root, false))
}

fn side(doc: &BaseDocument, root: NodeId, start: bool) -> Option<LineTrim> {
    let flag = if start { TextBoxTrim::TRIM_START } else { TextBoxTrim::TRIM_END };
    // The metric is the `text-box-edge` of the line's own root inline box (an inherited
    // property), not that of the box that trims.
    let root_style = doc.nodes[root]
        .primary_styles()
        .or_else(|| doc.nodes[root].parent.and_then(|parent| doc.nodes[parent].primary_styles()))?;
    let mut current = root;
    loop {
        let node = &doc.nodes[current];
        let split = node.is_split_inline();
        let anonymous = node.is_anonymous();
        let style = node.primary_styles();
        if !anonymous && !split
            && let Some(style) = style.as_deref()
            && style.slow_clone_text_box_trim().contains(flag)
        {
            let (over, under) = edges(&root_style);
            // The block-start side is the line-over side, unless the lines are inverted.
            let is_over = start != line_inverted(&root_style);
            return Some(LineTrim { edge: if is_over { over } else { under }, over: is_over });
        }
        // Only an in-flow block box passes its line on to its container.
        let display = node.display_style()?;
        let in_flow = style.as_deref().is_some_and(|style| !is_out_of_flow(style));
        let block = anonymous
            || (display.outside() == DisplayOutside::Block
                && matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot));
        if !split && (!block || !in_flow) {
            return None;
        }
        // A box with a border or padding on that side keeps its line to itself (Blink).
        if !anonymous && !split && style.as_deref().is_some_and(|style| has_block_edge(style, start)) {
            return None;
        }
        let parent = node.layout_parent.get()?;
        if !is_edge_in_flow_child(doc, parent, current, start) {
            return None;
        }
        current = parent;
    }
}

/// Whether `style` has a border or padding on its block-start (`start`) or block-end side.
fn has_block_edge(style: &ComputedValues, start: bool) -> bool {
    use style::logical_geometry::PhysicalSide;
    use style::values::computed::Length;
    let wm = style.writing_mode;
    let side = if start { wm.block_start_physical_side() } else { wm.block_end_physical_side() };
    let (b, p) = (style.get_border(), style.get_padding());
    let (width, border_style, padding) = match side {
        PhysicalSide::Top => (&b.border_top_width, b.border_top_style, &p.padding_top),
        PhysicalSide::Right => (&b.border_right_width, b.border_right_style, &p.padding_right),
        PhysicalSide::Bottom => (&b.border_bottom_width, b.border_bottom_style, &p.padding_bottom),
        PhysicalSide::Left => (&b.border_left_width, b.border_left_style, &p.padding_left),
    };
    (!border_style.none_or_hidden() && width.0.to_f32_px() != 0.0)
        || padding.0.resolve(Length::new(0.0)).px() != 0.0
        || padding.0.has_percentage()
}

/// Whether `child` is the first (`start`) or last in-flow box among the layout children of `parent`.
fn is_edge_in_flow_child(doc: &BaseDocument, parent: NodeId, child: NodeId, start: bool) -> bool {
    let children = doc.nodes[parent].layout_children.borrow();
    let Some(children) = children.as_deref() else { return false };
    let in_flow = |id: &NodeId| {
        let node = &doc.nodes[*id];
        node.display_style().is_none_or(|display| display != style::values::specified::Display::None)
            && !node.primary_styles().is_some_and(|style| is_out_of_flow(&style))
            && !is_empty_anonymous_block(doc, *id)
    };
    let found = if start { children.iter().find(|id| in_flow(id)) } else { children.iter().rev().find(|id| in_flow(id)) };
    found == Some(&child)
}

/// pyro: whether `id` is an anonymous block that makes no line: it holds collapsible white space
/// and the empty heads or tails of split inlines (no content, no inline-axis edges). The block in
/// an inline after it holds its container's first formatted line (WPT css-inline
/// `text-box-trim-block-in-inline-*`).
fn is_empty_anonymous_block(doc: &BaseDocument, id: NodeId) -> bool {
    let node = &doc.nodes[id];
    if !node.is_anonymous() {
        return false;
    }
    let len = node.children.len();
    node.children.iter().enumerate().all(|(index, &child)| match node.split_ref_at(index, len) {
        Some(tail) => is_empty_split_part(doc, child, tail),
        None => doc.nodes[child].is_collapsible_whitespace_node(),
    })
}

/// pyro: whether the head (`tail: false`) or the tail of the split inline `id` holds nothing.
fn is_empty_split_part(doc: &BaseDocument, id: NodeId, tail: bool) -> bool {
    use style::values::computed::Length;
    let Some(style) = doc.nodes[id].primary_styles() else { return false };
    let (b, p, m) = (style.get_border(), style.get_padding(), style.get_margin());
    let zero = |v: &style::values::computed::LengthPercentage| v.resolve(Length::new(0.0)).px() == 0.0 && !v.has_percentage();
    let margin = |v: &style::values::computed::Margin| match v {
        style::values::generics::length::GenericMargin::LengthPercentage(v) => zero(v),
        _ => true,
    };
    let border = |w: &style::values::computed::BorderSideWidth, s: style::values::computed::BorderStyle| {
        s.none_or_hidden() || w.0.to_f32_px() == 0.0
    };
    let edges_empty = if style.writing_mode.is_vertical() {
        zero(&p.padding_top.0) && zero(&p.padding_bottom.0) && margin(&m.margin_top) && margin(&m.margin_bottom)
            && border(&b.border_top_width, b.border_top_style) && border(&b.border_bottom_width, b.border_bottom_style)
    } else {
        zero(&p.padding_left.0) && zero(&p.padding_right.0) && margin(&m.margin_left) && margin(&m.margin_right)
            && border(&b.border_left_width, b.border_left_style) && border(&b.border_right_width, b.border_right_style)
    };
    drop(style);
    if !edges_empty {
        return false;
    }
    let (part, nested) = super::construct::split_inline_part(&doc.nodes, id, tail);
    part.iter().all(|&child| doc.nodes[child].is_collapsible_whitespace_node())
        && nested.is_none_or(|nested| is_empty_split_part(doc, nested, tail))
}
