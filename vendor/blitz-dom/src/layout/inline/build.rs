use blitz_traits::node_id::NodeId;
use markup5ever::local_name;
use parley::{
    FontContext, IndentOptions, InlineBox, InlineBoxKind, LayoutContext, TreeBuilder, WhiteSpaceCollapse,
};
use style::{
    computed_values::position::T as PositionProperty,
    values::{
        computed::{Display, Float},
        specified::box_::{DisplayInside, DisplayOutside},
    },
};
use stylo_taffy::CopyStyleGetters;

use style::values::computed::WhiteSpaceTrim;
use crate::{
    NodeData,
    layout::list::BULLET_FONT_FAMILY,
    layout::replaced::is_inline_box,
    layout::text_transform::{CaseTransform, TextTransformer},
    node::{
        FirstLetterSplit, ListItemLayout, ListItemLayoutPosition, Marker, TextAnnotation, TextAnnotationKind, TextBrush,
        TextLayout,
    },
    stylo_to_parley,
};

/// pyro: the text style a `display: contents` element gives its text: its own, but the box
/// properties it has none for (`vertical-align` applies to inline boxes: a `display: contents`
/// table cell's inherited `middle` must not shift its text, WPT css-display
/// `display-contents-td-001`).
fn contents_style(id: NodeId, style: &style::properties::ComputedValues) -> parley::TextStyle<'static, 'static, TextBrush> {
    let mut text_style = stylo_to_parley::style(id, style);
    text_style.vertical_align = Default::default();
    text_style
}

/// pyro: the `display: contents` elements between `child_id`, a node an anonymous inline root
/// holds, and the root's `container`, outermost first: their content was hoisted into the root
/// (`collect_layout_children_with_wrap`), and their text styles apply to it.
fn contents_ancestors(nodes: &crate::NodeTree, child_id: NodeId, container: Option<NodeId>) -> Vec<NodeId> {
    let mut chain = Vec::new();
    let mut current = nodes[child_id].parent;
    while let Some(id) = current {
        if Some(id) == container {
            break;
        }
        let node = &nodes[id];
        if !node.primary_styles().is_some_and(|style| style.clone_display().is_contents()) {
            break;
        }
        chain.push(id);
        current = node.parent;
    }
    chain.reverse();
    chain
}

/// pyro: the inline-axis space the start and the end side of an inline (non-atomic) box take:
/// margin, border and padding, in CSS pixels; `None` for a side with none of them. A side whose
/// margin, border and padding add up to nothing (a border and a negative margin as wide) still
/// is an edge: it paints, and its line is no empty line (CSS 2.1 § 9.4.2). Percentages resolve
/// against zero and `auto` margins are zero. In a vertical writing mode only the margins are
/// edges (top and bottom, by the inline direction): borders and padding stay out, as their sides
/// are not painted there.
pub(crate) fn span_edge_widths(style: &style::properties::ComputedValues) -> (Option<f32>, Option<f32>) {
    let (start, end, _) = span_edge_widths_in(style, 0.0);
    (start, end)
}

/// pyro: [`span_edge_widths`] with percentages resolved against `basis` (the width of the
/// span's containing block, CSS px; zero while it is not known and for intrinsic sizes), and
/// whether a side has a percentage: such a side is an edge whatever it resolves to, and each
/// layout gives it its width.
pub(crate) fn span_edge_widths_in(
    style: &style::properties::ComputedValues,
    basis: f32,
) -> (Option<f32>, Option<f32>, bool) {
    use style::values::computed::{Length, LengthPercentage, Margin};
    use style::values::generics::length::GenericMargin;
    let length = |v: &LengthPercentage| v.resolve(Length::new(basis)).px();
    let margin = |v: &Margin| match v {
        GenericMargin::LengthPercentage(v) => length(v),
        _ => 0.0,
    };
    let margin_percent = |v: &Margin| match v {
        GenericMargin::LengthPercentage(v) => v.has_percentage(),
        _ => false,
    };
    let (b, p, m) = (style.get_border(), style.get_padding(), style.get_margin());
    if style.writing_mode.is_vertical() {
        let edge = |v: &Margin| {
            let (width, percent) = (margin(v), margin_percent(v));
            ((width != 0.0 || percent).then_some(width), percent)
        };
        let ((top, top_percent), (bottom, bottom_percent)) = (edge(&m.margin_top), edge(&m.margin_bottom));
        let percent = top_percent || bottom_percent;
        return if style.writing_mode.is_inline_tb() { (top, bottom, percent) } else { (bottom, top, percent) };
    }
    // The used width: none where the side has no border style.
    let border = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
        if style.none_or_hidden() { 0.0 } else { width.0.to_f32_px() }
    };
    let side = |margin: f32, border: f32, padding: f32, percent: bool| {
        (margin != 0.0 || border != 0.0 || padding != 0.0 || percent).then_some(margin + border + padding)
    };
    let left_percent = margin_percent(&m.margin_left) || p.padding_left.0.has_percentage();
    let right_percent = margin_percent(&m.margin_right) || p.padding_right.0.has_percentage();
    let left = side(
        margin(&m.margin_left),
        border(&b.border_left_width, b.border_left_style),
        length(&p.padding_left.0),
        left_percent,
    );
    let right = side(
        margin(&m.margin_right),
        border(&b.border_right_width, b.border_right_style),
        length(&p.padding_right.0),
        right_percent,
    );
    let percent = left_percent || right_percent;
    if style.clone_direction() == style::computed_values::direction::T::Rtl {
        (right, left, percent)
    } else {
        (left, right, percent)
    }
}

/// pyro: the margins of an inline box on its inline-start and inline-end sides (CSS px,
/// percentages against `basis`, `auto` 0): left and right (right and left when right-to-left), or
/// in a vertical writing mode top and bottom (bottom and top when the inline direction is reversed).
/// The part of a span edge ([`span_edge_widths`]) outside its box.
pub fn span_edge_margins(style: &style::properties::ComputedValues, basis: f32) -> (f32, f32) {
    use style::values::computed::{Length, Margin};
    use style::values::generics::length::GenericMargin;
    let margin = |v: &Margin| match v {
        GenericMargin::LengthPercentage(v) => v.resolve(Length::new(basis)).px(),
        _ => 0.0,
    };
    let m = style.get_margin();
    let wm = style.writing_mode;
    if wm.is_vertical() {
        let (top, bottom) = (margin(&m.margin_top), margin(&m.margin_bottom));
        if wm.is_inline_tb() { (top, bottom) } else { (bottom, top) }
    } else {
        let (left, right) = (margin(&m.margin_left), margin(&m.margin_right));
        if style.clone_direction() == style::computed_values::direction::T::Rtl { (right, left) } else { (left, right) }
    }
}

/// pyro: the bidi formatting characters that the start and the end of an inline box stand for,
/// by its `unicode-bidi` and `direction` (CSS Writing Modes 3 § 2.4.2), as Blink inserts them.
fn bidi_controls(style: &style::properties::ComputedValues) -> Option<(&'static [char], &'static [char])> {
    use style::computed_values::direction::T as Direction;
    use style::computed_values::unicode_bidi::T as UnicodeBidi;
    let rtl = style.clone_direction() == Direction::Rtl;
    Some(match style.clone_unicode_bidi() {
        UnicodeBidi::Normal => return None,
        UnicodeBidi::Embed if rtl => (&['\u{202B}'], &['\u{202C}']),
        UnicodeBidi::Embed => (&['\u{202A}'], &['\u{202C}']),
        UnicodeBidi::Isolate if rtl => (&['\u{2067}'], &['\u{2069}']),
        UnicodeBidi::Isolate => (&['\u{2066}'], &['\u{2069}']),
        UnicodeBidi::BidiOverride if rtl => (&['\u{202E}'], &['\u{202C}']),
        UnicodeBidi::BidiOverride => (&['\u{202D}'], &['\u{202C}']),
        UnicodeBidi::IsolateOverride if rtl => (&['\u{2067}', '\u{202E}'], &['\u{202C}', '\u{2069}']),
        UnicodeBidi::IsolateOverride => (&['\u{2066}', '\u{202D}'], &['\u{202C}', '\u{2069}']),
        UnicodeBidi::Plaintext => (&['\u{2068}'], &['\u{2069}']),
    })
}

/// pyro: whether `c` is one of the bidi formatting characters `bidi_controls` gives, or the
/// strut of an empty inline box (U+2061, `TreeBuilder::push_empty_box_strut`): no content.
pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{2061}')
}

/// pyro: whether `c` makes a paragraph's bidi resolution give right-to-left levels: a strong
/// right-to-left or Arabic character, an Arabic number, or a right-to-left mark or formatting
/// character (by block, the Arabic letter mark among them; a superset of bidi classes R, AL and
/// AN).
fn is_rtl_char(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x08FF
        | 0xFB1D..=0xFDFF
        | 0xFE70..=0xFEFF
        | 0x10800..=0x10FFF
        | 0x1E800..=0x1EFFF
        | 0x200F
        | 0x202B
        | 0x202E
        | 0x2067)
}

/// pyro: what a build saw of the `unicode-bidi` of the root and its inline boxes.
#[derive(Default)]
struct BidiScan {
    /// Some box stands for bidi formatting characters (`bidi_controls`).
    controls: bool,
    /// Such a box, or the root, is right-to-left.
    rtl: bool,
    /// Some inline box (not atomic, not split) is relatively positioned (or sticky).
    relative: bool,
    /// Some inline box's margin or padding at an edge is a percentage (`span_edge_widths_in`).
    percent_edges: bool,
    /// Some inline box has a border or padding on a block-axis side: its box reaches out of its
    /// line (`TextLayout::span_block_edges`).
    block_edges: bool,
    /// The `<br>`s with `clear`, each with the offset of its newline (`TextLayout::br_clears`).
    br_clears: Vec<(usize, NodeId)>,
    /// How many in-flow atomic inline boxes were pushed.
    atomic_boxes: usize,
    /// Some inline box held nothing and pushed an empty-box strut.
    empty_box_struts: bool,
    /// The split inlines the layout holds pieces of, with whether it holds their start and end
    /// (`TextLayout::split_pieces`).
    split_pieces: Vec<(NodeId, bool, bool)>,
    /// The bidi formatting characters pushed (with `bidi`) whose embedding, override or isolate
    /// is open, outermost first: a forced line break (a paragraph separator to the bidi
    /// algorithm, which closes them) opens them again (`reopen_bidi_controls`).
    open_controls: Vec<&'static [char]>,
}

/// pyro: after a forced line break (`<br>`), the bidi formatting characters still open before it,
/// again: the break ends the bidi algorithm's paragraph, which closes every embedding, override
/// and isolate, and Blink re-opens them after it (`InlineItemsBuilder::AppendForcedBreak`), so a
/// `unicode-bidi: bidi-override` holds on the lines after a `<br>` too.
fn reopen_bidi_controls(builder: &mut TreeBuilder<TextBrush>, bidi_scan: &BidiScan) {
    for controls in &bidi_scan.open_controls {
        controls.iter().for_each(|&c| builder.push_bidi_control(c));
    }
}

/// pyro: the split inlines (`NodeFlags::IS_SPLIT_INLINE`) whose box holds the anonymous block
/// `root_id` (inline content between two of their block-level boxes), innermost first: its
/// content is inside their spans, with neither their start nor their end.
/// pyro: the `unicode-bidi` an inline root's lines take: an anonymous block's is its parent's
/// (the block's immediate inline content, which a block's `bidi-override` or `plaintext` governs;
/// `unicode-bidi` is not inherited), not that of its `::before` or `::after`.
fn root_unicode_bidi(
    nodes: &crate::NodeTree,
    node_id: NodeId,
    style: &style::properties::ComputedValues,
) -> style::computed_values::unicode_bidi::T {
    let node = &nodes[node_id];
    if node.is_anonymous()
        && let Some(parent_id) = node.layout_parent.get().or(node.parent)
        && nodes[parent_id].before() != Some(node_id)
        && nodes[parent_id].after() != Some(node_id)
        && let Some(parent_style) = nodes[parent_id].primary_styles()
        && parent_style.clone_display().outside() != style::values::specified::box_::DisplayOutside::Inline
    {
        return parent_style.clone_unicode_bidi();
    }
    style.clone_unicode_bidi()
}

/// pyro: the direction of an anonymous box of the root element (its own text, a run of its
/// inline children): the root's used direction, that of its `<body>` child (css-writing-modes-3 §8,
/// unless either has containment). `None` for other boxes.
pub(crate) fn principal_direction(
    nodes: &crate::NodeTree,
    node_id: NodeId,
) -> Option<style::computed_values::direction::T> {
    let node = &nodes[node_id];
    if !node.is_anonymous() {
        return None;
    }
    let root_id = node.layout_parent.get().or(node.parent)?;
    let root = &nodes[root_id];
    // (Not its `::before` or `::after`, which have computed values of their own.)
    if root.before() == Some(node_id) || root.after() == Some(node_id) {
        return None;
    }
    let is_root = root.data.downcast_element().is_some_and(|el| *el.name.local == *"html")
        && root.parent.is_some_and(|parent| matches!(nodes[parent].data, crate::node::NodeData::Document(_)));
    if !is_root || root.primary_styles().is_none_or(|style| !style.clone_contain().is_empty()) {
        return None;
    }
    let body = root
        .children
        .iter()
        .map(|&child| &nodes[child])
        .find(|child| child.data.downcast_element().is_some_and(|el| *el.name.local == *"body"))?;
    let style = body.primary_styles()?;
    style.clone_contain().is_empty().then(|| style.clone_direction())
}

fn split_pieces(nodes: &crate::NodeTree, root_id: NodeId) -> Vec<(NodeId, bool, bool)> {
    let mut pieces = Vec::new();
    let root = &nodes[root_id];
    // A pseudo-element is an anonymous box of its own, not a piece.
    let is_pseudo = root.parent.is_some_and(|parent| {
        nodes[parent].before() == Some(root_id) || nodes[parent].after() == Some(root_id)
    });
    if !root.is_anonymous() || is_pseudo {
        return pieces;
    }
    let mut child = root;
    while let Some(parent) = child.parent.map(|id| &nodes[id]).filter(|node| node.is_split_inline()) {
        pieces.push((parent.id, false, false));
        child = parent;
    }
    pieces
}

/// pyro: what the start of an inline box's span pushed that its end pushes too: its end edge and
/// the bidi formatting characters closing its `unicode-bidi`.
struct SpanEnd {
    edge: Option<InlineBox>,
    controls: Option<&'static [char]>,
}

/// pyro: pushes the span of the inline box `node_id` (style `style`): the bidi formatting
/// characters of its `unicode-bidi` (with `bidi`), its start edge with `start`, and the strut
/// when it has an edge. With `end`, its end edge goes in the returned [`SpanEnd`].
#[allow(clippy::too_many_arguments)]
/// pyro: whether an inline box with `style` has a border or padding over or under it (left or
/// right of it in a vertical writing mode).
fn has_block_edges(style: &style::properties::ComputedValues) -> bool {
    use style::values::computed::Length;
    let (b, p) = (style.get_border(), style.get_padding());
    let border = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
        !style.none_or_hidden() && width.0.to_f32_px() != 0.0
    };
    let padding = |v: &style::values::computed::NonNegativeLengthPercentage| {
        v.0.resolve(Length::new(0.0)).px() != 0.0 || v.0.has_percentage()
    };
    if style.writing_mode.is_vertical() {
        return border(&b.border_left_width, b.border_left_style)
            || border(&b.border_right_width, b.border_right_style)
            || padding(&p.padding_left)
            || padding(&p.padding_right);
    }
    border(&b.border_top_width, b.border_top_style)
        || border(&b.border_bottom_width, b.border_bottom_style)
        || padding(&p.padding_top)
        || padding(&p.padding_bottom)
}

fn open_span(
    builder: &mut TreeBuilder<TextBrush>,
    node_id: NodeId,
    style: &style::properties::ComputedValues,
    start: bool,
    end: bool,
    bidi: bool,
    bidi_scan: &mut BidiScan,
    strut: &mut bool,
) -> SpanEnd {
    let mut parley_style = stylo_to_parley::style(node_id, style);
    parley_style.text_box_trim = crate::layout::text_box::span_trim(style);
    builder.push_style_span(parley_style);
    // (A sticky one is painted moved too: `BaseDocument::inline_span_relative_offset`.)
    bidi_scan.relative |= matches!(style.clone_position(), PositionProperty::Relative | PositionProperty::Sticky)
        || crate::layout::inline::relative::effect_entry(style);
    let rtl = style.clone_direction() == style::computed_values::direction::T::Rtl;
    let controls = bidi_controls(style);
    if let Some((start_controls, _)) = controls {
        bidi_scan.controls = true;
        bidi_scan.rtl |= rtl;
        if bidi {
            start_controls.iter().for_each(|&c| builder.push_bidi_control(c));
            bidi_scan.open_controls.push(start_controls);
        }
    }
    let (start_edge, end_edge, percent) = span_edge_widths_in(style, 0.0);
    bidi_scan.percent_edges |= percent;
    bidi_scan.block_edges |= has_block_edges(style);
    let (start_edge, end_edge) = (start_edge.filter(|_| start), end_edge.filter(|_| end));
    let scale = builder.scale();
    let edge = |kind, width: f32| InlineBox {
        id: node_id.as_u64(),
        kind,
        index: 0,
        width: width * scale,
        height: 0.0,
        baseline: None,
        vertical_align: Default::default(),
        rtl,
        central: false,
        glue: false,
    };
    // pyro: a right-to-left span with one edge has the other too, empty (see
    // `build_inline_layout_recursive`).
    let both = rtl && (start_edge.is_some() || end_edge.is_some());
    if start_edge.is_some() || (both && start) {
        builder.push_inline_box(edge(InlineBoxKind::SpanStart, start_edge.unwrap_or(0.0)));
    }
    if (start_edge.is_some() || end_edge.is_some()) && std::mem::take(strut) {
        builder.push_text("\u{2060}");
    }
    SpanEnd {
        edge: (end_edge.is_some() || (both && end)).then(|| edge(InlineBoxKind::SpanEnd, end_edge.unwrap_or(0.0))),
        controls: controls.map(|(_, end_controls)| end_controls),
    }
}

/// pyro: pushes the end of a span `open_span` started.
fn close_span(builder: &mut TreeBuilder<TextBrush>, end: SpanEnd, bidi: bool, bidi_scan: &mut BidiScan) {
    if let Some(edge) = end.edge {
        builder.push_inline_box(edge);
    }
    if let (true, Some(controls)) = (bidi, end.controls) {
        controls.iter().for_each(|&c| builder.push_bidi_control(c));
        bidi_scan.open_controls.pop();
    }
    builder.pop_style_span();
}

pub(crate) fn build_inline_layout_into(
    nodes: &crate::NodeTree,
    layout_ctx: &mut LayoutContext<TextBrush>,
    font_ctx: &mut FontContext,
    text_layout: &mut TextLayout,
    scale: f32,
    inline_context_root_node_id: NodeId,
) {
    // pyro: built with the bidi formatting characters of `unicode-bidi` when they matter.
    let built = build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, false, &[], false);
    let bidi = !built;
    if bidi {
        build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, false, &[], true);
    }
    // A line holding atomic inlines but no text still has its strut (CSS 2.1
    // 10.8.1): the root's font and line height. Parley makes no line height
    // without text, so a line of empty inline-blocks was 0px tall. A
    // word joiner (zero-width, no break opportunity) provides the strut.
    // pyro: so has the line of a list item's outside marker in an item without in-flow content.
    // pyro: not in quirks and limited-quirks mode, where a line without text has no strut (Quirks
    // Mode §3.3, "The line height calculation quirk"): an image alone in a block leaves no gap
    // under it (WPT css-sizing `intrinsic-percent-replaced-006`).
    let marker = outside_marker(&nodes[inline_context_root_node_id]);
    if (marker || (text_layout.layout.inline_boxes().len() > 0 && !nodes.line_height_quirk))
        // (The zero-width span an outside image marker adds to the first line is no text.)
        && text_layout.text.chars().all(|c| (c.is_whitespace() && c != '\n') || is_bidi_control(c) || c == '\u{2060}')
    {
        build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, true, &[], bidi);
    }
}

/// pyro: the word separator CSS Text 4 `word-space-transform` turns zero width spaces and
/// `<wbr>` into in `style`, if any.
fn word_space_char(style: &style::properties::ComputedValues) -> Option<char> {
    use style::values::computed::WordSpaceTransform as W;
    let value = style.get_inherited_text().word_space_transform;
    if value.contains(W::SPACE) {
        Some(' ')
    } else if value.contains(W::IDEOGRAPHIC_SPACE) {
        Some('\u{3000}')
    } else {
        None
    }
}

/// pyro: pushes `text` with each U+200B ZERO WIDTH SPACE as `word_space` (`word-space-transform`);
/// a space collapses with the spaces next to it (WPT css-text `word-space-transform-007`). One next
/// to a forced line break stays (a preserved
/// newline when `newlines`, a `<br>` after the text when `before_break`, or one before it).
fn push_word_spaces(builder: &mut TreeBuilder<TextBrush>, text: &str, word_space: Option<char>, newlines: bool, before_break: bool) {
    let Some(word_space) = word_space else {
        builder.push_text(text);
        return;
    };
    // The zero width spaces that end the text (before a `<br>` when `before_break`), found once.
    let trailing = text.trim_end_matches('\u{200B}').len();
    let mut rest = text;
    while let Some(at) = rest.find('\u{200B}') {
        let (head, tail) = rest.split_at(at);
        let offset = text.len() - tail.len();
        builder.push_text(head);
        let after = &tail['\u{200B}'.len_utf8()..];
        let previous = builder.text().chars().rev().find(|&c| !is_bidi_control(c));
        let next = after.chars().next();
        let at_break = previous.is_some_and(|c| c == '\n' && (newlines || head.is_empty()))
            || (newlines && next == Some('\n'))
            || (before_break && offset >= trailing);
        if at_break {
            builder.push_text("\u{200B}");
        } else {
            let mut buf = [0u8; 4];
            builder.push_text(word_space.encode_utf8(&mut buf));
        }
        rest = after;
    }
    builder.push_text(rest);
}

/// pyro: whether the inline root is a list item with an outside marker (`layout::list`).
fn outside_marker(root: &crate::Node) -> bool {
    root.element_data()
        .and_then(|element| element.list_item_data.as_deref())
        .is_some_and(|item| matches!(item.position, ListItemLayoutPosition::Outside(_)))
}

/// Builds the inline layout of `inline_context_root_node_id`. pyro: without `bidi`, returns
/// false without building when the content needs the bidi formatting characters of its inline
/// boxes' `unicode-bidi` (some box has one, and the paragraph has right-to-left content: a
/// left-to-right paragraph without any resolves every level to 0 with or without them).
#[allow(clippy::too_many_arguments)]
fn build_inline_layout_with(
    nodes: &crate::NodeTree,
    layout_ctx: &mut LayoutContext<TextBrush>,
    font_ctx: &mut FontContext,
    text_layout: &mut TextLayout,
    scale: f32,
    inline_context_root_node_id: NodeId,
    strut: bool,
    // pyro: the room annotations take beyond the content area of the spans that hold them (see
    // `TextAnnotation`), measured by a first build.
    annotation_space: &[(NodeId, parley::AnnotationSpace)],
    bidi: bool,
) -> bool {
    // Get the inline context's root node's text styles
    let root_node = &nodes[inline_context_root_node_id];
    let root_node_style = root_node.primary_styles().or_else(|| {
        root_node
            .parent
            .and_then(|parent_id| nodes[parent_id].primary_styles())
    });
    // pyro: vertical lines of text that is not sideways align on the central baseline.
    let central_baseline = root_node_style
        .as_deref()
        .is_some_and(|s| s.writing_mode.is_vertical() && !s.writing_mode.is_sideways());

    let mut parley_style = root_node_style
        .as_ref()
        .map(|s| stylo_to_parley::style(inline_context_root_node_id, s))
        .unwrap_or_else(|| parley::TextStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..Default::default()
        });

    // pyro: combined text (`text-combine-upright: all`, a horizontal box in a vertical line) of
    // two, three or four characters takes the font's half-, third- or quarter-width forms
    // (`hwid`, `twid`, `qwid`), as Blink does before it compresses it (css-writing-modes-4 §9.1.1).
    if let Some(style) = root_node_style.as_deref()
        && style.flags.contains(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED)
    {
        let count = root_node.text_content().trim().chars().count();
        let tag: Option<&[u8; 4]> = match count {
            2 => Some(b"hwid"),
            3 => Some(b"twid"),
            4 => Some(b"qwid"),
            _ => None,
        };
        if let Some(tag) = tag {
            let mut features = match &parley_style.font_features {
                parley::FontFeatures::List(list) => list.to_vec(),
                _ => Vec::new(),
            };
            if !features.iter().any(|f| f.tag == crate::stylo_to_parley::parley::Tag::from_bytes(*tag)) {
                features.push(parley::FontFeature { tag: crate::stylo_to_parley::parley::Tag::from_bytes(*tag), value: 1 });
            }
            parley_style.font_features = parley::FontFeatures::List(std::borrow::Cow::Owned(features));
        }
    }

    // The formatting-context root's vertical alignment positions its box in
    // its parent's context (for example a middle-aligned table cell). It does
    // not shift that box's own line strut or inherited direct text. Descendant
    // spans and atomic inline boxes retain their separately collected styles.
    parley_style.vertical_align = parley::VerticalAlign::BASELINE;
    parley_style.annotation_space = space_of(annotation_space, inline_context_root_node_id);

    // Create a parley tree builder
    let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
    if let Some(style) = root_node_style.as_deref() {
        let direction = principal_direction(nodes, inline_context_root_node_id).unwrap_or_else(|| style.clone_direction());
        builder.set_base_direction(stylo_to_parley::base_direction(
            direction,
            root_unicode_bidi(nodes, inline_context_root_node_id, style),
        ));
    }

    let text_transform = root_node_style
        .as_deref()
        .map(|s| CaseTransform::from_style(s))
        .unwrap_or(CaseTransform::NONE);

    // pyro: a root's `bidi-override` overrides the levels of its whole inline content.
    let mut bidi_scan = BidiScan::default();
    if let Some(style) = root_node_style.as_deref() {
        use style::computed_values::direction::T as Direction;
        use style::computed_values::unicode_bidi::T as UnicodeBidi;
        // (The root's anonymous text takes `<body>`'s direction, `principal_direction`.)
        let direction = principal_direction(nodes, inline_context_root_node_id).unwrap_or_else(|| style.clone_direction());
        let rtl = direction == Direction::Rtl;
        bidi_scan.rtl = rtl;
        if matches!(
            root_unicode_bidi(nodes, inline_context_root_node_id, style),
            UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride
        ) {
            bidi_scan.controls = true;
            if bidi {
                let control: &'static [char] = if rtl { &['\u{202E}'] } else { &['\u{202D}'] };
                builder.push_bidi_control(control[0]);
                bidi_scan.open_controls.push(control);
            }
        }
    }

    // Render position-inside list items
    if let Some(ListItemLayout {
        marker,
        style_node,
        position: ListItemLayoutPosition::Inside,
        tabular_nums,
        isolate,
        image_gap,
        ..
    }) = root_node
        .element_data()
        .and_then(|el| el.list_item_data.as_deref())
    {
        // pyro: in the item's `::marker` style, when a rule styles the marker.
        let styled = style_node
            .and_then(|id| nodes.get(id).and_then(|node| node.primary_styles().map(|style| (id, style))))
            .map(|(id, style)| {
                let mut parley_style = stylo_to_parley::style(id, &style);
                // (With the room its emphasis marks take, `FoundAnnotations`.)
                parley_style.annotation_space = space_of(annotation_space, id);
                parley_style
            });
        if let Some(style) = &styled {
            builder.push_style_span(style.clone());
        }
        let parley_style = styled.as_ref().unwrap_or(&parley_style);
        // pyro: a string marker is bidi-isolated in the item's direction (the UA's `::marker {
        // unicode-bidi: isolate }`; WPT css-lists `list-style-type-string-006`).
        // (`unicode-bidi: normal` in a `::marker` rule leaves it in the item's paragraph; WPT
        // css-pseudo `marker-unicode-bidi-normal`.)
        // (A symbol too: WPT css-lists `list-marker-symbol-bidi`.)
        let isolate = *isolate && matches!(marker, Marker::String(_) | Marker::Char(_));
        if isolate {
            let rtl = root_node_style
                .as_deref()
                .is_some_and(|style| style.clone_direction() == style::computed_values::direction::T::Rtl);
            if bidi {
                builder.push_bidi_control(if rtl { '\u{2067}' } else { '\u{2066}' });
            } else {
                bidi_scan.controls = true;
                bidi_scan.rtl |= rtl;
            }
        }
        // pyro: combined in a vertical line: its box (`layout::combined_text`), placed as one
        // letter.
        let combined = root_node
            .element_data()
            .and_then(|element| element.marker_combined_box)
            .filter(|&id| nodes.get(id).is_some());
        if let Some(box_id) = combined {
            bidi_scan.atomic_boxes += 1;
            builder.push_inline_box(InlineBox {
                id: box_id.as_u64(),
                kind: InlineBoxKind::InFlow,
                index: 0,
                width: 0.0,
                height: 0.0,
                baseline: None,
                vertical_align: Default::default(),
                rtl: false,
                central: false,
                glue: true,
            });
        }
        match marker {
            _ if combined.is_some() => {}
            // Bullet glyphs live in the bundled bullet font. The position-outside
            // path already asks for it; without the same span here a marker like
            // disclosure-closed (U+25B8) falls back to the element's own font and
            // renders as a missing glyph.
            Marker::Char(char) => {
                let mut marker_style = parley_style.clone();
                marker_style.font_family = BULLET_FONT_FAMILY.into();
                // pyro: a symbol and its space are not spaced by `letter-spacing` or
                // `word-spacing` (Blink; WPT css-pseudo `marker-word-spacing`).
                marker_style.letter_spacing = 0.0;
                marker_style.word_spacing = 0.0;
                builder.push_style_span(marker_style);
                builder.push_text(&format!("{char} "));
                builder.pop_style_span();
            }
            // pyro: with tabular digits (`ListItemLayout::tabular_nums`).
            Marker::String(str) if *tabular_nums => {
                let features = stylo_to_parley::with_tabular_nums(&parley_style.font_features);
                builder.push_style_modification_span(&[parley::StyleProperty::FontFeatures(
                    parley::FontFeatures::List(std::borrow::Cow::Owned(features)),
                )]);
                builder.push_text(str);
                builder.pop_style_span();
            }
            Marker::String(str) => builder.push_text(str),
            // pyro: an image marker inside the item: room for it at the start of the first
            // line, as a run of no size of its own that is as tall as the image above the
            // baseline and as wide as it plus Blink's 7px after a marker (blitz-paint draws
            // the image there, `draw_inside_marker_image`).
            Marker::Image { width, height } => {
                let mut marker_style = parley_style.clone();
                marker_style.font_size = 0.01;
                marker_style.line_height = parley::LineHeight::Absolute(*height);
                marker_style.letter_spacing = *width + *image_gap;
                marker_style.vertical_align = parley::VerticalAlign {
                    alignment: parley::AlignmentBaseline::Baseline,
                    shift: parley::BaselineShift::Length(*height / 2.0),
                };
                builder.push_style_span(marker_style);
                builder.push_text("\u{a0}");
                builder.pop_style_span();
            }
        }
        if isolate && bidi {
            builder.push_bidi_control('\u{2069}');
        }
        if styled.is_some() {
            builder.pop_style_span();
        }
    };
    // The marker is a separate box, so words in the content don't continue from it.
    let mut text_transformer = TextTransformer::default();
    text_transformer.word_break(&builder);
    let text_nodes = &mut Vec::new();
    // pyro: ruby annotations and emphasis marks found on the way (see `TextAnnotation`).
    let annotations = &mut FoundAnnotations {
        space: annotation_space.to_vec(),
        first_line: (text_layout.first_line_styles.clone(), text_layout.first_line_end.unwrap_or(usize::MAX)),
        ..Default::default()
    };
    if let Some(style) = root_node.primary_styles() {
        annotations.enter(inline_context_root_node_id, &style);
    }
    // pyro: an inside marker's `::marker` style has emphasis marks of its own (WPT css-pseudo
    // `marker-text-emphasis`): its text's brush is the style's node.
    if let Some(ListItemLayout { style_node: Some(id), position: ListItemLayoutPosition::Inside, .. }) =
        root_node.element_data().and_then(|el| el.list_item_data.as_deref())
        && let Some(style) = nodes.get(*id).and_then(|node| node.primary_styles())
    {
        annotations.enter(*id, &style);
    }
    if root_node.data.is_element_with_tag_name(&local_name!("ruby")) {
        annotations.ruby_base_start = Some(builder.text().len());
        annotations.ruby = Some(inline_context_root_node_id);
    }
    // The strut goes before the first in-flow inline box: trailing white
    // space after it still hangs, leading white space before it still
    // collapses.
    let strut_build = strut;
    // pyro: without one, it starts the line of the list item's outside marker.
    let marker_line = strut
        && outside_marker(root_node)
        && text_layout.layout.inline_boxes().all(|inline_box| inline_box.kind != InlineBoxKind::InFlow);
    if marker_line {
        builder.push_text("\u{2060}");
    }
    // pyro: an outside image marker is an inline replaced box on the item's first line, its
    // bottom on the baseline: the line is as tall as it above the baseline (WPT css-lists
    // `outside-marker-taller-than-content`), also where the line has text (WPT css-pseudo
    // `marker-content-012`).
    if (marker_line || (outside_marker(root_node) && !strut_build))
        && let Some(Marker::Image { height, .. }) =
            root_node.element_data().and_then(|el| el.list_item_data.as_deref()).map(|item| &item.marker)
    {
        {
            let mut marker_style = parley_style.clone();
            marker_style.font_size = 0.01;
            marker_style.line_height = parley::LineHeight::Absolute(*height);
            marker_style.vertical_align = parley::VerticalAlign {
                alignment: parley::AlignmentBaseline::Baseline,
                shift: parley::BaselineShift::Length(*height / 2.0),
            };
            builder.push_style_span(marker_style);
            builder.push_text("\u{2060}");
            builder.pop_style_span();
        }
    }
    let strut = &mut { strut && !marker_line };
    // pyro: the first letter the root's `::first-letter` styles (`layout::first_letter`).
    let first_letter_split = text_layout.first_letter.clone();
    let first_letter = first_letter_split.as_ref();

    // pyro: a piece of the split inlines whose box holds it is wrapped in their spans.
    let pieces = split_pieces(nodes, inline_context_root_node_id);
    let mut piece_ends = Vec::with_capacity(pieces.len());
    for &(id, _, _) in pieces.iter().rev() {
        let end = nodes[id]
            .primary_styles()
            .map(|style| open_span(&mut builder, id, &style, false, false, bidi, &mut bidi_scan, strut));
        piece_ends.push(end);
    }
    bidi_scan.split_pieces.extend(pieces.iter().copied());

    if let Some(before_id) = root_node.before() {
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes, inline_context_root_node_id,
            before_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
            bidi,
            &mut bidi_scan,
        );
    }
    let child_count = root_node.children.len();
    // pyro: the `display: contents` elements whose content an anonymous root holds, open as spans
    // (outermost first): their text takes their styles (`contents_ancestors`).
    let container = root_node.is_anonymous().then_some(root_node.parent).flatten();
    let mut open_contents: Vec<NodeId> = Vec::new();
    for (index, child_id) in root_node.children.iter().copied().enumerate() {
        if container.is_some() {
            let chain = contents_ancestors(nodes, child_id, container);
            let common = open_contents.iter().zip(&chain).take_while(|(a, b)| a == b).count();
            for _ in common..open_contents.len() {
                builder.pop_style_span();
            }
            open_contents.truncate(common);
            for &id in &chain[common..] {
                match nodes[id].primary_styles() {
                    Some(style) => builder.push_style_span(contents_style(id, &style)),
                    None => builder.push_style_modification_span(&[]),
                }
                open_contents.push(id);
            }
        }
        // pyro: the head or the tail of a split inline.
        if let Some(tail) = root_node.split_ref_at(index, child_count) {
            build_split_part(
                &mut builder,
                &mut text_transformer,
                nodes,
                child_id,
                tail,
                text_nodes,
                strut,
                first_letter,
                annotations,
                bidi,
                &mut bidi_scan,
            );
            continue;
        }
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes, inline_context_root_node_id,
            child_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
            bidi,
            &mut bidi_scan,
        );
    }
    for _ in open_contents.drain(..) {
        builder.pop_style_span();
    }
    if let Some(after_id) = root_node.after() {
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes, inline_context_root_node_id,
            after_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
            bidi,
            &mut bidi_scan,
        );
    }

    for end in piece_ends.into_iter().rev().flatten() {
        close_span(&mut builder, end, bidi, &mut bidi_scan);
    }
    text_layout.split_pieces = std::mem::take(&mut bidi_scan.split_pieces);
    text_layout.br_clears = std::mem::take(&mut bidi_scan.br_clears);

    // pyro: the bidi formatting characters change the levels only when some are needed and the
    // paragraph has right-to-left content (see `build_inline_layout_with`).
    if !bidi
        && bidi_scan.controls
        && (bidi_scan.rtl || (!builder.text().is_ascii() && builder.text().chars().any(is_rtl_char)))
    {
        return false;
    }

    // pyro: `::first-line` styles that change layout apply to the first line's text (CSS Pseudo 4
    // § 2.1): its font size, line height, font family, weight and style, letter and word spacing.
    text_layout.first_line_layout = false;
    if let (Some(first_line), Some(root_style)) = (text_layout.first_line, root_node_style.as_ref())
        && let Some(line_style) = nodes.get(first_line).and_then(|node| node.primary_styles())
    {
        let line = stylo_to_parley::style(first_line, &line_style);
        let root = stylo_to_parley::style(inline_context_root_node_id, root_style);
        let mut changes = Vec::new();
        if line.font_size != root.font_size {
            changes.push(parley::StyleProperty::FontSize(line.font_size));
        }
        if line.line_height != root.line_height {
            changes.push(parley::StyleProperty::LineHeight(line.line_height));
        }
        if line.font_family != root.font_family {
            changes.push(parley::StyleProperty::FontFamily(line.font_family.clone()));
        }
        if line.font_weight != root.font_weight {
            changes.push(parley::StyleProperty::FontWeight(line.font_weight));
        }
        if line.font_style != root.font_style {
            changes.push(parley::StyleProperty::FontStyle(line.font_style));
        }
        if line.letter_spacing != root.letter_spacing {
            changes.push(parley::StyleProperty::LetterSpacing(line.letter_spacing));
        }
        if line.word_spacing != root.word_spacing {
            changes.push(parley::StyleProperty::WordSpacing(line.word_spacing));
        }
        if !changes.is_empty() {
            builder.set_first_line(text_layout.first_line_end.unwrap_or(usize::MAX), &changes);
            text_layout.first_line_layout = true;
        }
    }

    // The first build keeps the lines of the layout's last break over the text that did not
    // change, for `BreakLines::resume`. The strut build follows a build of the same content.
    text_layout.text = if strut_build {
        builder.build_into(&mut text_layout.layout)
    } else {
        builder.build_into_keeping_lines(&mut text_layout.layout)
    };
    text_layout.text_nodes = std::mem::take(text_nodes);
    // pyro: the annotations' own layouts, now that the builder has let go of the contexts. They
    // take room in their lines: the spans that hold them are built again with it.
    let built: Vec<(TextAnnotation, NodeId)> = std::mem::take(&mut annotations.found)
        .into_iter()
        .filter_map(|(node_id, kind, holder)| {
            Some((build_annotation(nodes, layout_ctx, font_ctx, scale, node_id, kind)?, holder))
        })
        .collect();
    let space = annotation_room(&built, scale);
    text_layout.annotations = built.into_iter().map(|(annotation, _)| annotation).collect();
    if annotation_space.is_empty() && !space.is_empty() {
        return build_inline_layout_with(
            nodes,
            layout_ctx,
            font_ctx,
            text_layout,
            scale,
            inline_context_root_node_id,
            strut_build,
            &space,
            bidi,
        );
    }
    // pyro: the "…" (or the string of `text-overflow: "<string>"`) that `text-overflow` ends an
    // overflowing line with, in the root's own style (its first available font, as in Chromium).
    // (An anonymous block holds lines of its parent: `overflow` and `text-overflow` are the
    // parent's, which the anonymous box's own style does not inherit. A pseudo-element's box
    // is anonymous too and has its own.)
    let ellipsis = root_node_style.as_deref().and_then(|style| line_end_ellipsis(style, false)).or_else(|| {
        let parent = root_node.parent.filter(|_| root_node.is_anonymous())?;
        line_end_ellipsis(&*nodes[parent].primary_styles()?, false)
    });
    let rtl_lines = text_layout.layout.is_rtl();
    text_layout.ellipsis = ellipsis.as_deref().map(|text| {
        let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
        // Its base direction is that of the root's lines (under `unicode-bidi: plaintext`, the
        // one the root's text resolved to).
        builder.set_base_direction(if rtl_lines {
            parley::BaseDirection::Rtl
        } else {
            parley::BaseDirection::Ltr
        });
        builder.push_text(text);
        let mut layout = builder.build().0;
        layout.break_all_lines(None);
        Box::new(layout)
    });
    text_layout.ellipsis_text = ellipsis.unwrap_or_default();
    // pyro: the tab stops of preserved tabs (CSS Text 3 § 4.2). The tab's own style has the
    // `tab-size`; a number counts in the advance of a space in the root's first available font
    // (the block container's, not the tab's inline box) plus the root's letter and word spacing.
    // A tab closer than 0.5ch to its stop goes on to the next. Set before the content widths are
    // measured, as each layout of this text has the same stops.
    if text_layout.text.contains('\t') {
        let mut space_style = parley_style.clone();
        space_style.letter_spacing = 0.0;
        space_style.word_spacing = 0.0;
        let mut advance_of = |text: &str| {
            let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &space_style);
            builder.push_text(text);
            let mut layout = builder.build().0;
            layout.break_all_lines(None);
            layout.full_width()
        };
        // The space (U+0020) of the first font that has one: measured between two digits, as a
        // font without spaces may still map U+00A0 (WPT css-text `tab-size-integer-005`).
        let ch = advance_of("0");
        let space = (advance_of("0 0") - 2.0 * ch).max(0.0)
            + (parley_style.letter_spacing + parley_style.word_spacing) * scale;
        text_layout.layout.set_tab_stops(space, ch / 2.0);
    }
    text_layout.layout.set_line_height_quirk(nodes.line_height_quirk);
    text_layout.layout.set_central_baseline(central_baseline);
    // pyro: the shaped text's content widths, for every layout of it (`TextLayout::
    // content_widths`): a function of the shaped clusters alone while no in-flow inline box, whose
    // size each layout sets, takes part in them. Measured with no text indent; the layout sets its
    // own before it reads them.
    let text_only = text_layout.layout.inline_boxes().all(|inline_box| inline_box.kind != InlineBoxKind::InFlow);
    // (Nor an edge whose width each layout sets: a percentage margin or padding.)
    let text_only = text_only && !bidi_scan.percent_edges;
    text_layout.text_content_widths = text_only.then(|| {
        text_layout.layout.set_text_indent(0.0, IndentOptions::default());
        text_layout.layout.calculate_content_widths()
    });
    text_layout.bidi_controls = bidi || bidi_scan.empty_box_struts;
    // (Not in a multi-column container of a vertical writing mode, which is laid out as one
    // column (`layout::multicol`): what the offset moves out of its column would show there,
    // where the next column covers it.)
    text_layout.relative_spans = bidi_scan.relative;
    text_layout.percent_span_edges = bidi_scan.percent_edges;
    text_layout.span_block_edges = bidi_scan.block_edges;
    return true;

    /// pyro: lays out on one line what an annotation draws: a ruby annotation's content in its
    /// style, or the emphasis mark of an element in its font at half its size (as an `<rt>` has it).
    fn build_annotation(
        nodes: &crate::NodeTree,
        layout_ctx: &mut LayoutContext<TextBrush>,
        font_ctx: &mut FontContext,
        scale: f32,
        node_id: NodeId,
        kind: TextAnnotationKind,
    ) -> Option<TextAnnotation> {
        use style::values::computed::text::TextEmphasisPosition;
        use style::values::specified::text::RubyPosition;
        let node = &nodes[node_id];
        let style = node.primary_styles()?;
        let itext = style.get_inherited_text();
        let mut parley_style = stylo_to_parley::style(node_id, &style);
        // pyro: an annotation is centred over its base by its layout's width and its glyphs are
        // drawn em-wide (`draw_annotation`): no tall upright advances (parley `shape::upright`),
        // as emphasis marks have none (WPT css-text-decor `text-emphasis-style-property-005`).
        parley_style.upright = false;
        let (mut layout, under) = match kind {
            TextAnnotationKind::Ruby { .. } => {
                let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
                let text_transform = CaseTransform::from_style(&style);
                let mut text_transformer = TextTransformer::default();
                let mut nested = FoundAnnotations::default();
                let children = node.before().into_iter().chain(node.children.iter().copied()).chain(node.after());
                for child_id in children {
                    build_inline_layout_recursive(
                        &mut builder,
                        &mut text_transformer,
                        nodes, node_id,
                        child_id,
                        &text_transform,
                        &mut Vec::new(),
                        &mut false,
                        None,
                        &mut nested,
                        false,
                        &mut BidiScan::default(),
                    );
                }
                // pyro: `ruby-position` applies to the annotation container, which inherits it from
                // the `<ruby>` (or is the `<rtc>`): an `<rt>`'s own value does not move it, as in
                // Chromium (WPT css-inline `text-box-trim-ruby-start-001`).
                let position = node
                    .parent
                    .and_then(|parent| nodes[parent].primary_styles())
                    .map_or(itext.ruby_position, |parent| parent.get_inherited_text().ruby_position);
                let under = matches!(position, RubyPosition::Under | RubyPosition::AlternateUnder);
                (builder.build().0, under)
            }
            TextAnnotationKind::Emphasis => emphasis_layout(layout_ctx, font_ctx, scale, &style, parley_style)?,
        };
        layout.break_all_lines(None);
        Some(TextAnnotation { node_id, kind, under, layout: Box::new(layout) })
    }

    /// pyro: the head (`tail: false`) or the tail of the split inline `node_id`, in its span:
    /// a head has its start, a tail its end (`construct::split_inline_part`).
    #[allow(clippy::too_many_arguments)]
    fn build_split_part(
        builder: &mut TreeBuilder<TextBrush>,
        text_transformer: &mut TextTransformer,
        nodes: &crate::NodeTree,
        node_id: NodeId,
        tail: bool,
        text_nodes: &mut Vec<(NodeId, std::ops::Range<usize>)>,
        strut: &mut bool,
        first_letter: Option<&FirstLetterSplit>,
        annotations: &mut FoundAnnotations,
        bidi: bool,
        bidi_scan: &mut BidiScan,
    ) {
        let node = &nodes[node_id];
        let Some(style) = node.primary_styles() else {
            return;
        };
        let text_transform = CaseTransform::from_style(&style);
        let end = open_span(builder, node_id, &style, !tail, tail, bidi, bidi_scan, strut);
        drop(style);
        bidi_scan.split_pieces.push((node_id, !tail, tail));
        let (part, nested) = crate::layout::construct::split_inline_part(nodes, node_id, tail);
        if let (true, Some(nested)) = (tail, nested) {
            crate::util::ensure_stack(|| build_split_part(
                builder, text_transformer, nodes, nested, true, text_nodes, strut, first_letter, annotations, bidi,
                bidi_scan,
            ));
        }
        for child_id in part {
            crate::util::ensure_stack(|| build_inline_layout_recursive(
                builder, text_transformer, nodes, node_id, child_id, &text_transform, text_nodes, strut, first_letter, annotations,
                bidi, bidi_scan,
            ));
        }
        if let (false, Some(nested)) = (tail, nested) {
            crate::util::ensure_stack(|| build_split_part(
                builder, text_transformer, nodes, nested, false, text_nodes, strut, first_letter, annotations, bidi,
                bidi_scan,
            ));
        }
        close_span(builder, end, bidi, bidi_scan);
    }

    #[allow(clippy::too_many_arguments)]
    /// pyro: builds `node_id`'s inline content, with the scroll pseudo-element boxes generated
    /// beside it (`scroll_markers`) before and after it, unless `container` (whose box list holds
    /// `node_id`'s box) placed them already (`scroll_markers::siblings_of`).
    #[allow(clippy::too_many_arguments)]
    fn build_inline_layout_recursive(
        builder: &mut TreeBuilder<TextBrush>,
        text_transformer: &mut TextTransformer,
        nodes: &crate::NodeTree,
        container: NodeId,
        node_id: NodeId,
        parent_text_transform: &CaseTransform,
        text_nodes: &mut Vec<(NodeId, std::ops::Range<usize>)>,
        strut: &mut bool,
        first_letter: Option<&FirstLetterSplit>,
        annotations: &mut FoundAnnotations,
        bidi: bool,
        bidi_scan: &mut BidiScan,
    ) {
        match crate::layout::scroll_markers::siblings_of(nodes, container, &nodes[node_id]) {
            None => build_inline_layout_node(
                builder, text_transformer, nodes, node_id, parent_text_transform, text_nodes, strut, first_letter,
                annotations, bidi, bidi_scan,
            ),
            Some((before, after)) => {
                for id in before.into_iter().chain(std::iter::once(node_id)).chain(after) {
                    build_inline_layout_node(
                        builder, text_transformer, nodes, id, parent_text_transform, text_nodes, strut, first_letter,
                        annotations, bidi, bidi_scan,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_inline_layout_node(
        builder: &mut TreeBuilder<TextBrush>,
        text_transformer: &mut TextTransformer,
        nodes: &crate::NodeTree,
        node_id: NodeId,
        parent_text_transform: &CaseTransform,
        text_nodes: &mut Vec<(NodeId, std::ops::Range<usize>)>,
        strut: &mut bool,
        first_letter: Option<&FirstLetterSplit>,
        annotations: &mut FoundAnnotations,
        bidi: bool,
        bidi_scan: &mut BidiScan,
    ) {
        let node = &nodes[node_id];

        let style = node.primary_styles();
        let style = style.as_ref();

        let text_transform = style
            .map(|s| CaseTransform::from_style(s))
            .unwrap_or(CaseTransform::NONE);

        match &node.data {
            NodeData::Element(element_data) | NodeData::AnonymousBlock(element_data) => {
                // if the input type is hidden, hide it
                if *element_data.name.local == *"input" {
                    if let Some("hidden") = element_data.attr(local_name!("type")) {
                        return;
                    }
                }

                // pyro: `<wbr>` is a soft wrap opportunity (HTML § 15.3.3: as U+200B ZERO WIDTH
                // SPACE), also under `word-break: keep-all` and in CJK text.
                if element_data.name.local.eq_str_ignore_ascii_case("wbr")
                    && element_data.name.ns == markup5ever::ns!(html)
                    && !node
                        .display_style()
                        .is_some_and(|d| matches!(d.inside(), DisplayInside::None))
                {
                    let wst = node.primary_styles().and_then(|style| word_space_char(&style));
                    let before_break = node.forward(1).is_some_and(|next| next.is_line_break());
                    push_word_spaces(builder, "\u{200B}", wst, false, before_break);
                    return;
                }

                if node.is_line_break() {
                    // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    // TODO: update span id for br spans
                    // pyro: in quirks mode a `<br>` alone on its line sets the line's height with its
                    // own line height (Quirks Mode §3.3; Blink's `LayoutBR`).
                    let br_line_height = style
                        .filter(|_| nodes.line_height_quirk)
                        .map(|s| crate::stylo_to_parley::style(node_id, s).line_height);
                    match br_line_height {
                        Some(line_height) => builder.push_style_modification_span(&[
                            parley::StyleProperty::WhiteSpaceCollapse(WhiteSpaceCollapse::Preserve),
                            parley::StyleProperty::LineHeight(line_height),
                        ]),
                        None => builder.push_style_modification_span(&[
                            parley::StyleProperty::WhiteSpaceCollapse(WhiteSpaceCollapse::Preserve),
                        ]),
                    }
                    if node.primary_styles().is_some_and(|style| {
                        style.clone_clear() != style::computed_values::clear::T::None
                    }) {
                        bidi_scan.br_clears.push((builder.text().len(), node_id));
                    }
                    builder.push_text("\n");
                    builder.pop_style_span();
                    if bidi {
                        reopen_bidi_controls(builder, bidi_scan);
                    }
                    text_transformer.word_break(builder);
                    return;
                }

                let display = node.display_style().unwrap_or(Display::inline());
                let position = style
                    .map(|s| s.clone_position())
                    .unwrap_or(PositionProperty::Static);
                let float = style.map(|s| s.clone_float()).unwrap_or(Float::None);
                let box_kind = if position.is_absolutely_positioned() {
                    InlineBoxKind::OutOfFlow
                } else if float.is_floating() {
                    InlineBoxKind::CustomOutOfFlow
                } else {
                    InlineBoxKind::InFlow
                };

                match (display.outside(), display.inside()) {
                    (DisplayOutside::None, DisplayInside::None) => {
                        // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    }
                    (DisplayOutside::None, DisplayInside::Contents) => {
                        // pyro: the element has no box, but its text takes its styles (font, spacing,
                        // white space, and through the brush its colour), as in Blink (WPT css-display
                        // `display-contents-text-inherit`, `-line-height`, a slot's text,
                        // css-shadow `reslot-text-inheritance`). Upstream took only its white space.
                        match style {
                            Some(s) => builder.push_style_span(contents_style(node.id, s)),
                            None => builder.push_style_modification_span(&[]),
                        }
                        // pyro: with its `::before` and `::after`.
                        let children = node.before().into_iter().chain(node.children.iter().copied()).chain(node.after());
                        for child_id in children {
                            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                            crate::util::ensure_stack(|| build_inline_layout_recursive(
                                builder,
                                text_transformer,
                                nodes, node_id,
                                child_id,
                                &text_transform,
                                text_nodes,
                                strut,
                                first_letter,
                                annotations,
                                bidi,
                                bidi_scan,
                            ));
                        }
                        builder.pop_style_span();
                    }
                    (DisplayOutside::Inline, DisplayInside::Flow) => {
                        let tag_name = &element_data.name.local;

                        if is_inline_box(element_data) {
                            if box_kind == InlineBoxKind::InFlow {
                                bidi_scan.atomic_boxes += 1;
                                text_transformer.word_break(builder);
                                if std::mem::take(strut) {
                                    builder.push_text("\u{2060}");
                                }
                            }
                            builder.push_inline_box(InlineBox {
                                id: node_id.as_u64(),
                                kind: box_kind,
                                // Overridden by push_inline_box method
                                index: 0,
                                // Width and height are set during layout
                                width: 0.0,
                                height: 0.0,
                                baseline: None,
                                vertical_align: node
                                    .primary_styles()
                                    .map(|s| stylo_to_parley::vertical_align(&s))
                                    .unwrap_or_default(),
                                rtl: false,
                                central: false,
                                glue: false,
                            });
                        } else if annotations.is_ruby_annotation(tag_name) {
                            // pyro: a ruby annotation of the text since its base started, drawn
                            // over that text rather than in the line (`TextAnnotation`).
                            annotations.annotate_ruby_base(node_id, builder.text().len());
                            // pyro: its absolutely positioned descendants (its `::before` and
                            // `::after` too) are no annotation text: they are positioned as out of
                            // flow boxes of this line, at the annotation's place (WPT css-contain
                            // `contain-layout-004`, `-005`: containment does not apply to ruby
                            // annotations, so they are positioned against the box around).
                            let mut stack: Vec<NodeId> =
                                node.before().into_iter().chain(node.children.iter().copied()).chain(node.after()).collect();
                            stack.reverse();
                            while let Some(id) = stack.pop() {
                                let child = &nodes[id];
                                let Some(cs) = child.primary_styles() else { continue };
                                if cs.clone_display().is_none() {
                                    continue;
                                }
                                if cs.clone_position().is_absolutely_positioned() {
                                    builder.push_inline_box(InlineBox {
                                        id: id.as_u64(),
                                        kind: InlineBoxKind::OutOfFlow,
                                        index: 0,
                                        width: 0.0,
                                        height: 0.0,
                                        baseline: None,
                                        vertical_align: Default::default(),
                                        rtl: false,
                                        central: false,
                                        glue: false,
                                    });
                                    continue;
                                }
                                let mut kids: Vec<NodeId> =
                                    child.before().into_iter().chain(child.children.iter().copied()).chain(child.after()).collect();
                                kids.reverse();
                                stack.extend(kids);
                            }
                        } else {
                            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                            let mut style = node
                                .primary_styles()
                                .map(|s| stylo_to_parley::style(node.id, &s))
                                .unwrap_or_else(|| parley::TextStyle {
                                    white_space_collapse: WhiteSpaceCollapse::Collapse,
                                    ..Default::default()
                                });
                            style.annotation_space = space_of(&annotations.space, node_id);
                            if let Some(computed) = node.primary_styles() {
                                style.text_box_trim = crate::layout::text_box::span_trim(&computed);
                                // pyro: an inline bidi isolate breaks shaping at its edges (CSS
                                // Text 3 § 7.3). A `<ruby>` (an inline box here, which the UA
                                // sheet isolates) lays its bases out as one text.
                                use style::computed_values::unicode_bidi::T as UnicodeBidi;
                                // pyro: CSS Text 4 `wrap-inside: avoid` on it and the inline boxes around
                                // it in this inline formatting context.
                                style.wrap_avoid_depth = {
                                    let mut depth = 0u8;
                                    let mut id = Some(node_id);
                                    while let Some(ancestor) = id.map(|id| &nodes[id]) {
                                        let Some(s) = ancestor.primary_styles() else { break };
                                        if s.clone_display() != style::values::computed::Display::Inline {
                                            break;
                                        }
                                        if s.get_text().wrap_inside == style::values::computed::WrapInside::Avoid {
                                            depth = depth.saturating_add(1);
                                        }
                                        id = ancestor.parent;
                                    }
                                    depth
                                };
                                style.isolates = !matches!(*tag_name, local_name!("ruby") | local_name!("rb") | local_name!("rtc"))
                                    && matches!(
                                        computed.clone_unicode_bidi(),
                                        UnicodeBidi::Isolate | UnicodeBidi::IsolateOverride | UnicodeBidi::Plaintext
                                    );
                            }

                            // pyro: CSS Text 4 `white-space-trim`.
                            let trim = node.primary_styles().map_or(WhiteSpaceTrim::NONE, |s| s.get_text().white_space_trim);
                            if trim.contains(WhiteSpaceTrim::DISCARD_BEFORE) {
                                builder.discard_pending_whitespace();
                            }
                            builder.push_style_span(style);
                            if trim.contains(WhiteSpaceTrim::DISCARD_INNER) {
                                builder.discard_next_whitespace();
                            }
                            // pyro: emphasis marks over the span's own text; a `<ruby>`'s first
                            // base starts here.
                            if let Some(style) = node.primary_styles() {
                                annotations.enter(node_id, &style);
                            }
                            let is_ruby = *tag_name == local_name!("ruby");
                            let outer_ruby = (annotations.ruby_base_start, annotations.ruby);
                            if is_ruby {
                                annotations.ruby_base_start = Some(builder.text().len());
                                annotations.ruby = Some(node_id);
                            }

                            bidi_scan.relative |= node.primary_styles().is_some_and(|style| {
                                matches!(style.clone_position(), PositionProperty::Relative | PositionProperty::Sticky)
                                    || crate::layout::inline::relative::effect_entry(&style)
                            });
                            bidi_scan.block_edges |= node.primary_styles().is_some_and(|style| has_block_edges(&style));
                            // pyro: the bidi formatting characters its `unicode-bidi` stands for,
                            // around its edges, as Blink places them.
                            let controls = node.primary_styles().and_then(|style| {
                                let controls = bidi_controls(&style)?;
                                bidi_scan.controls = true;
                                bidi_scan.rtl |= style.clone_direction() == style::computed_values::direction::T::Rtl;
                                Some(controls)
                            });
                            if let (true, Some((start, _))) = (bidi, controls) {
                                start.iter().for_each(|&c| builder.push_bidi_control(c));
                                bidi_scan.open_controls.push(start);
                            }
                            let held = (builder.text().len(), bidi_scan.atomic_boxes);

                            // pyro: the span's inline-axis margin, border and padding take space
                            // at its start and its end (CSS 2.1 9.4.2): an edge box at each, inside
                            // the span's style, when there is any.
                            let (start_edge, end_edge, percent) =
                                // pyro: on the first line, with its `::first-line` style there.
                                match annotations.first_line_style(node_id, builder.text().len()) {
                                    Some(s) => span_edge_widths_in(&s, 0.0),
                                    None => node.primary_styles().map(|s| span_edge_widths_in(&s, 0.0)).unwrap_or_default(),
                                };
                            bidi_scan.percent_edges |= percent;
                            let scale = builder.scale();
                            let rtl = node.primary_styles().is_some_and(|style| {
                                style.clone_direction() == style::computed_values::direction::T::Rtl
                            });
                            let edge = |kind, width: f32| InlineBox {
                                id: node_id.as_u64(),
                                kind,
                                index: 0,
                                width: width * scale,
                                height: 0.0,
                                baseline: None,
                                vertical_align: Default::default(),
                                rtl,
                                central: false,
                                glue: false,
                            };
                            // pyro: a right-to-left span's edges trade sides on the line (its end
                            // edge goes left of its content): the line needs the span's content
                            // as items of its own, which an edge at each end of it gives (text
                            // is shaped across a span's boundary otherwise). With one edge it
                            // has the other too, empty.
                            let both = rtl && (start_edge.is_some() || end_edge.is_some());
                            if start_edge.is_some() || both {
                                builder.push_inline_box(edge(InlineBoxKind::SpanStart, start_edge.unwrap_or(0.0)));
                            }
                            // A span with such an edge is not empty: its line has the strut.
                            if (start_edge.is_some() || end_edge.is_some()) && std::mem::take(strut) {
                                builder.push_text("\u{2060}");
                            }

                            if let Some(before_id) = node.before() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes, node_id,
                                    before_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                    bidi,
                                    bidi_scan,
                                ));
                            }

                            for child_id in node.children.iter().copied() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes, node_id,
                                    child_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                    bidi,
                                    bidi_scan,
                                ));
                            }
                            if let Some(after_id) = node.after() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes, node_id,
                                    after_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                    bidi,
                                    bidi_scan,
                                ));
                            }

                            if is_ruby {
                                (annotations.ruby_base_start, annotations.ruby) = outer_ruby;
                            }
                            // pyro: a span that holds nothing still has its strut in the line (CSS
                            // 2.1 § 10.8: its font and line height count in the line's height).
                            if trim.contains(WhiteSpaceTrim::DISCARD_INNER) {
                                builder.discard_pending_whitespace();
                            }
                            if held == (builder.text().len(), bidi_scan.atomic_boxes) {
                                builder.push_empty_box_strut();
                                bidi_scan.empty_box_struts = true;
                            }
                            if end_edge.is_some() || both {
                                builder.push_inline_box(edge(InlineBoxKind::SpanEnd, end_edge.unwrap_or(0.0)));
                            }
                            if let (true, Some((_, end))) = (bidi, controls) {
                                end.iter().for_each(|&c| builder.push_bidi_control(c));
                                bidi_scan.open_controls.pop();
                            }
                            builder.pop_style_span();
                            if trim.contains(WhiteSpaceTrim::DISCARD_AFTER) {
                                builder.discard_next_whitespace();
                            }
                        }
                    }
                    // Inline box
                    (_, _) => {
                        // pyro: a table part in an inline box is in an anonymous inline table: the
                        // table is the box, at its first part (`table_fixup::wrap_inline_table_parts`).
                        let (node_id, node) = match (display.outside() == style::values::specified::box_::DisplayOutside::InternalTable)
                            .then(|| crate::layout::table_fixup::inline_table_of(nodes, node_id))
                            .flatten()
                        {
                            Some((table, true)) => (table, &nodes[table]),
                            Some((_, false)) => return,
                            None => (node_id, node),
                        };
                        // pyro: CSS Text 4 `white-space-trim` around an atomic inline.
                        let trim = node.primary_styles().map_or(WhiteSpaceTrim::NONE, |s| s.get_text().white_space_trim);
                        if box_kind == InlineBoxKind::InFlow && trim.contains(WhiteSpaceTrim::DISCARD_BEFORE) {
                            builder.discard_pending_whitespace();
                        }
                        if box_kind == InlineBoxKind::InFlow {
                            bidi_scan.atomic_boxes += 1;
                            text_transformer.word_break(builder);
                            if std::mem::take(strut) {
                                builder.push_text("\u{2060}");
                            }
                        }
                        builder.push_inline_box(InlineBox {
                            id: node_id.as_u64(),
                            kind: box_kind,
                            // Overridden by push_inline_box method
                            index: 0,
                            // Width and height are set during layout
                            width: 0.0,
                            height: 0.0,
                            baseline: None,
                            vertical_align: node
                                .primary_styles()
                                .map(|s| stylo_to_parley::vertical_align(&s))
                                .unwrap_or_default(),
                            rtl: false,
                            central: false,
                            // pyro: `text-combine-upright` text (Stylo makes it a horizontal inline-block in
                            // the vertical line) breaks lines as one letter (`InlineBox::glue`).
                            glue: box_kind == InlineBoxKind::InFlow
                                && node.primary_styles().is_some_and(|style| style.flags.contains(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED)),
                        });
                        if box_kind == InlineBoxKind::InFlow && trim.contains(WhiteSpaceTrim::DISCARD_AFTER) {
                            builder.discard_next_whitespace();
                        }
                    }
                };
            }
            NodeData::Text(data) => {
                // pyro: combined text no inline element holds is a box of its own, placed as one
                // letter (`layout::combined_text`, `InlineBox::glue`).
                if let Some(box_id) = data.combined_box.filter(|&id| nodes.get(id).is_some()) {
                    bidi_scan.atomic_boxes += 1;
                    text_transformer.word_break(builder);
                    builder.push_inline_box(InlineBox {
                        id: box_id.as_u64(),
                        kind: InlineBoxKind::InFlow,
                        index: 0,
                        width: 0.0,
                        height: 0.0,
                        baseline: None,
                        vertical_align: Default::default(),
                        rtl: false,
                        central: false,
                        glue: true,
                    });
                    return;
                }
                // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                // dbg!(&data.content);
                // pyro: white space between two table parts generates no box (`table_fixup`).
                if node.flags.contains(crate::node::NodeFlags::IS_SPACE_BETWEEN_TABLE_PARTS) {
                    return;
                }

                let start = builder.text().len();
                // pyro: `hyphens: auto` adds soft hyphens (`layout::hyphenate`), by the style of
                // the text's parent, to words that may go on in the text before and after.
                let parent_style = node.parent.and_then(|parent| nodes.get(parent)?.primary_styles());
                let hyphenates = parent_style.as_ref().is_some_and(|style| {
                    style.get_inherited_text().hyphens == style::values::computed::text::Hyphens::Auto
                });
                let after = if hyphenates { word_after(nodes, node_id) } else { String::new() };
                let hyphenate = |before: &str, text: &str| match &parent_style {
                    Some(style) if hyphenates => {
                        crate::layout::hyphenate::hyphenate(before, text, &after, style).into_owned()
                    }
                    _ => text.to_owned(),
                };
                // pyro: the first letter in a span of the `::first-letter` style.
                let letter = first_letter
                    .filter(|split| split.text == node_id && data.content.get(split.range.clone()).is_some())
                    .and_then(|split| Some((split, nodes.get(split.style_node)?.primary_styles()?.clone())));
                match letter {
                    // pyro: an initial letter is a box of its own (`layout::first_letter`): the
                    // root's text leaves the letter out and holds the box in its place.
                    Some((split, _)) if split.initial.is_some() || split.floated => {
                        let content = &data.content;
                        let head = &content[..split.range.start];
                        let text = text_transformer.transform(head, parent_text_transform, builder);
                        { let before = word_before(builder.text()); builder.push_text(&hyphenate(&before, text)); }
                        builder.push_inline_box(InlineBox {
                            id: split.style_node.as_u64(),
                            kind: InlineBoxKind::CustomOutOfFlow,
                            // Overridden by push_inline_box method
                            index: 0,
                            // Placed by the first line
                            width: 0.0,
                            height: 0.0,
                            baseline: None,
                            vertical_align: Default::default(),
                            rtl: false,
                            central: false,
                            glue: false,
                        });
                        let tail = &content[split.range.end..];
                        let text = text_transformer.transform(tail, parent_text_transform, builder);
                        { let before = word_before(builder.text()); builder.push_text(&hyphenate(&before, text)); }
                    }
                    Some((split, letter_style)) => {
                        let content = &data.content;
                        let head = &content[..split.range.start];
                        let text = text_transformer.transform(head, parent_text_transform, builder);
                        { let before = word_before(builder.text()); builder.push_text(&hyphenate(&before, text)); }
                        builder.push_style_span(stylo_to_parley::style(split.style_node, &letter_style));
                        let letter = &content[split.range.clone()];
                        let text = text_transformer.transform(letter, &CaseTransform::from_style(&letter_style), builder);
                        builder.push_text(text);
                        builder.pop_style_span();
                        let tail = &content[split.range.end..];
                        let text = text_transformer.transform(tail, parent_text_transform, builder);
                        { let before = word_before(builder.text()); builder.push_text(&hyphenate(&before, text)); }
                    }
                    None => {
                        let text =
                            text_transformer.transform(&data.content, parent_text_transform, builder);
                        let before = if hyphenates { word_before(builder.text()) } else { String::new() };
                        let text = hyphenate(&before, text);
                        // pyro: a preserved newline ends the bidi paragraph as a `<br>` does: the
                        // embeddings and overrides still open go on after it.
                        let preserves_newlines = bidi
                            && !bidi_scan.open_controls.is_empty()
                            && parent_style.as_ref().is_some_and(|style| {
                                use style::computed_values::white_space_collapse::T as Collapse;
                                !matches!(style.get_inherited_text().white_space_collapse, Collapse::Collapse | Collapse::PreserveSpaces)
                            });
                        // pyro: CSS Text 4 `word-space-transform`.
                        let wst = parent_style.as_ref().and_then(|style| word_space_char(style));
                        if preserves_newlines && text.contains('\n') {
                            for piece in text.split_inclusive('\n') {
                                builder.push_text(piece);
                                if piece.ends_with('\n') {
                                    reopen_bidi_controls(builder, bidi_scan);
                                }
                            }
                        } else if wst.is_some() && text.contains('\u{200B}') {
                            let newlines = parent_style.as_ref().is_some_and(|style| {
                                use style::computed_values::white_space_collapse::T as Collapse;
                                !matches!(style.get_inherited_text().white_space_collapse, Collapse::Collapse | Collapse::PreserveSpaces)
                            });
                            let before_break = node.forward(1).is_some_and(|next| next.is_line_break());
                            push_word_spaces(builder, &text, wst, newlines, before_break);
                        } else {
                            builder.push_text(&text);
                        }
                    }
                }
                text_nodes.push((node_id, start..builder.text().len()));
            }
            NodeData::Comment { .. } => {
                // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            }
            NodeData::Document(_) => unreachable!(),
        }
    }
}

/// pyro: the ruby annotations and emphasis marks an inline root's build finds (see
/// [`TextAnnotation`]), laid out once the root's own text is built.
#[derive(Default)]
struct FoundAnnotations {
    /// Inside a `<ruby>`: where in the text the base of its next annotation starts.
    ruby_base_start: Option<usize>,
    /// The innermost `<ruby>` the build is in.
    ruby: Option<NodeId>,
    /// `<rt>`s with the bases they annotate, and elements with `text-emphasis`, in tree order,
    /// each with the span whose box holds it: the `<ruby>`, or the emphasized element.
    found: Vec<(NodeId, TextAnnotationKind, NodeId)>,
    /// The room the annotations of each span take, from the build before.
    space: Vec<(NodeId, parley::AnnotationSpace)>,
    /// pyro: `TextLayout::first_line_styles` and where the first line's text ends.
    first_line: (Vec<(NodeId, style::servo_arc::Arc<style::properties::ComputedValues>)>, usize),
}

impl FoundAnnotations {
    /// pyro: the style of element `node_id` on the first line, when its content starting at text
    /// offset `at` is on it and `::first-line` changes layout there.
    fn first_line_style(&self, node_id: NodeId, at: usize) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        if at >= self.first_line.1 {
            return None;
        }
        self.first_line.0.iter().find(|(id, _)| *id == node_id).map(|(_, style)| style.clone())
    }

    /// An element whose text is in the root: it has emphasis marks over its own text when its
    /// `text-emphasis-style` (inherited) is not `none`.
    fn enter(&mut self, node_id: NodeId, style: &style::properties::ComputedValues) {
        use style::values::computed::text::TextEmphasisStyle;
        if !matches!(style.get_inherited_text().text_emphasis_style, TextEmphasisStyle::None) {
            self.found.push((node_id, TextAnnotationKind::Emphasis, node_id));
        }
    }

    /// Whether an element of this name is a ruby annotation here: an `<rt>` (or an `<rtc>` of
    /// them) inside a `<ruby>`.
    fn is_ruby_annotation(&self, tag_name: &markup5ever::LocalName) -> bool {
        self.ruby_base_start.is_some() && (*tag_name == local_name!("rt") || *tag_name == local_name!("rtc"))
    }

    /// The annotation `node_id` of the text since the base started, up to `end`; the next base
    /// starts there.
    fn annotate_ruby_base(&mut self, node_id: NodeId, end: usize) {
        let start = self.ruby_base_start.replace(end).unwrap_or(end);
        let holder = self.ruby.unwrap_or(node_id);
        self.found.push((node_id, TextAnnotationKind::Ruby { base: start..end }, holder));
    }
}

/// pyro: the mark `text-emphasis-style` draws (CSS Text Decoration 3, 3.1.1), if any.
fn emphasis_mark(style: &style::values::computed::text::TextEmphasisStyle) -> Option<&str> {
    use style::values::computed::text::TextEmphasisStyle;
    use style::values::specified::text::TextEmphasisShapeKeyword as Shape;
    match style {
        TextEmphasisStyle::None => None,
        TextEmphasisStyle::String(mark) => Some(mark.as_ref()).filter(|mark: &&str| !mark.is_empty()),
        TextEmphasisStyle::Keyword { fill, shape } => Some(match (shape, fill.is_filled()) {
            (Shape::Dot, true) => "\u{2022}",
            (Shape::Dot, false) => "\u{25E6}",
            (Shape::Circle, true) => "\u{25CF}",
            (Shape::Circle, false) => "\u{25CB}",
            (Shape::DoubleCircle, true) => "\u{25C9}",
            (Shape::DoubleCircle, false) => "\u{25CE}",
            (Shape::Triangle, true) => "\u{25B2}",
            (Shape::Triangle, false) => "\u{25B3}",
            (Shape::Sesame, true) => "\u{FE45}",
            (Shape::Sesame, false) => "\u{FE46}",
        }),
    }
}

/// pyro: the room the annotations a span holds take beyond its content area (none without any).
fn space_of(space: &[(NodeId, parley::AnnotationSpace)], node_id: NodeId) -> parley::AnnotationSpace {
    space.iter().find(|(id, _)| *id == node_id).map(|(_, space)| *space).unwrap_or_default()
}

/// pyro: the room the annotations each span holds take beyond its content area (CSS px): the
/// content area of each annotation (laid out at `scale`), over or under the text as it is drawn, on
/// top of those of the annotations before it whose bases overlap its own on the same side (nested
/// ruby: an annotation of a base that has one goes outside it).
fn annotation_room(built: &[(TextAnnotation, NodeId)], scale: f32) -> Vec<(NodeId, parley::AnnotationSpace)> {
    let mut space: Vec<(NodeId, parley::AnnotationSpace)> = Vec::new();
    let mut stacked: Vec<f32> = Vec::with_capacity(built.len());
    for (index, (annotation, holder)) in built.iter().enumerate() {
        let height = annotation.layout.lines().next().map_or(0.0, |line| {
            let metrics = line.metrics();
            (metrics.content_block_max_coord - metrics.content_block_min_coord) / scale
        });
        let inside = built[..index]
            .iter()
            .zip(&stacked)
            .filter(|((other, _), _)| other.under == annotation.under && bases_overlap(&other.kind, &annotation.kind))
            .map(|(_, height)| *height)
            .fold(0.0, f32::max);
        let height = height.max(0.0) + inside;
        stacked.push(height);
        if height <= 0.0 {
            continue;
        }
        let index = match space.iter().position(|(id, _)| id == holder) {
            Some(index) => index,
            None => {
                space.push((*holder, parley::AnnotationSpace::default()));
                space.len() - 1
            }
        };
        let room = &mut space[index].1;
        if annotation.under {
            room.under = room.under.max(height);
        } else {
            room.over = room.over.max(height);
        }
    }
    space
}

/// pyro: whether two ruby annotations annotate overlapping text.
fn bases_overlap(a: &TextAnnotationKind, b: &TextAnnotationKind) -> bool {
    match (a, b) {
        (TextAnnotationKind::Ruby { base: a }, TextAnnotationKind::Ruby { base: b }) => {
            a.start < b.end && b.start < a.end
        }
        _ => false,
    }
}

/// pyro: what `text-overflow` puts at the end of the overflowing lines of a box with this
/// style: "…" for `ellipsis`, the string of a `<string>` value (one line: its white space
/// collapsed, with every line-breaking control character (U+000A to U+000D, U+0085, U+2028,
/// U+2029) as a space, as Chromium draws it), `None` when the box does not clip its inline overflow
/// or the end side's value is `clip`. Stylo keeps a single value as `second` (the end
/// side); of two, `first` is the line-left side's, where right-to-left lines end.
pub(crate) fn line_end_ellipsis(style: &style::properties::ComputedValues, clips: bool) -> Option<String> {
    use style::computed_values::direction::T as Direction;
    use style::values::computed::Overflow;
    use style::values::specified::text::TextOverflowSide;
    if !clips && style.get_box().overflow_x == Overflow::Visible {
        return None;
    }
    let overflow = &style.get_text().text_overflow;
    let end = if !overflow.sides_are_logical && style.clone_direction() == Direction::Rtl {
        &overflow.first
    } else {
        &overflow.second
    };
    match end {
        TextOverflowSide::Clip => None,
        TextOverflowSide::Ellipsis => Some("\u{2026}".to_string()),
        TextOverflowSide::String(string) => {
            let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}');
            let mut text = String::new();
            for c in AsRef::<str>::as_ref(string).chars() {
                if !is_space(c) {
                    text.push(c);
                } else if !text.ends_with(' ') {
                    text.push(' ');
                }
            }
            Some(text)
        }
    }
}

/// pyro: the end of the word that the built text ends in (its letters, marks and soft hyphens,
/// across invisible formatting), for `hyphens: auto`.
fn word_before(text: &str) -> String {
    use crate::layout::hyphenate::{is_invisible, is_word_char};
    let mut chars: Vec<char> = Vec::new();
    for c in text.chars().rev() {
        if is_invisible(c) {
            continue;
        }
        if !is_word_char(c) || chars.len() >= 64 {
            break;
        }
        chars.push(c);
    }
    chars.iter().rev().collect()
}

/// pyro: the start of the word that goes on after the text node `node_id`, in the text of the
/// inline content that follows it (out-of-flow boxes and hidden elements passed over, atomic
/// inlines, `<br>` and blocks ending it), for `hyphens: auto`.
fn word_after(nodes: &crate::NodeTree, node_id: NodeId) -> String {
    use crate::layout::hyphenate::{is_invisible, is_word_char};
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let mut out = String::new();
    // Whether an element's content continues the inline content: an inline or contents element.
    let continues = |id: NodeId| {
        nodes.get(id).and_then(|node| node.primary_styles()).is_some_and(|style| {
            let display = style.get_box().display;
            matches!(
                (display.outside(), display.inside()),
                (DisplayOutside::Inline, DisplayInside::Flow) | (DisplayOutside::None, DisplayInside::Contents)
            ) && !style.clone_position().is_absolutely_positioned()
                && !style.clone_float().is_floating()
        })
    };
    let passed_over = |id: NodeId| {
        nodes.get(id).and_then(|node| node.primary_styles()).is_some_and(|style| {
            let display = style.get_box().display;
            display.is_none()
                || style.clone_position().is_absolutely_positioned()
                || style.clone_float().is_floating()
        })
    };
    // The next node after `id` and its subtree, inside inline content.
    let next = |mut id: NodeId| -> Option<NodeId> {
        loop {
            let node = nodes.get(id)?;
            let parent = node.parent?;
            let siblings = &nodes.get(parent)?.children;
            let index = siblings.iter().position(|&child| child == id)?;
            if let Some(&sibling) = siblings.get(index + 1) {
                return Some(sibling);
            }
            if !continues(parent) {
                return None;
            }
            id = parent;
        }
    };
    let mut current = next(node_id);
    while let Some(id) = current {
        let node = &nodes[id];
        match &node.data {
            crate::node::NodeData::Text(data) => {
                for c in data.content.chars() {
                    if is_invisible(c) {
                        continue;
                    }
                    if !is_word_char(c) || out.len() >= 64 {
                        return out;
                    }
                    out.push(c);
                }
                current = next(id);
            }
            crate::node::NodeData::Element(element) => {
                if passed_over(id) {
                    current = next(id);
                } else if continues(id) && !element.name.local.eq_str_ignore_ascii_case("br") {
                    current = match node.children.first() {
                        Some(&child) => Some(child),
                        None => next(id),
                    };
                } else {
                    return out;
                }
            }
            _ => current = next(id),
        }
    }
    out
}

/// pyro: the one-mark layout of an element's `text-emphasis` in `parley_style` (its style's) at
/// half its size, and whether the marks go under the line; `None` without marks.
fn emphasis_layout(
    layout_ctx: &mut LayoutContext<TextBrush>,
    font_ctx: &mut FontContext,
    scale: f32,
    style: &style::properties::ComputedValues,
    mut parley_style: parley::TextStyle<'_, '_, TextBrush>,
) -> Option<(parley::Layout<TextBrush>, bool)> {
    use style::values::computed::text::TextEmphasisPosition;
    let itext = style.get_inherited_text();

    let mark = emphasis_mark(&itext.text_emphasis_style)?;
    parley_style.font_size *= 0.5;
    parley_style.line_height = parley::LineHeight::FontSizeRelative(1.0);
    parley_style.letter_spacing = 0.0;
    parley_style.word_spacing = 0.0;
    let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
    builder.push_text(mark);
    // `auto` is `over right`, but `under right` for Chinese. In a vertical writing
    // mode the right side is over the line (pyro: under it in `sideways-lr`).
    let position = itext.text_emphasis_position;
    let under = if style.writing_mode.is_vertical() {
        let sideways_lr = style
            .writing_mode
            .contains(style::logical_geometry::WritingMode::WRITING_MODE_SIDEWAYS_LR);
        position.contains(TextEmphasisPosition::LEFT) != sideways_lr
    } else if position.contains(TextEmphasisPosition::AUTO) {
        let lang: &str = &style.get_font()._x_lang.0;
        let lang = lang.to_ascii_lowercase();
        lang == "zh" || lang.starts_with("zh-")
    } else {
        position.contains(TextEmphasisPosition::UNDER)
    };
    Some((builder.build().0, under))
}

/// pyro: the emphasis marks of an outside list marker whose text has `node_id`'s style
/// (`layout::list`), as [`build_annotation`] builds an element's.
pub(crate) fn marker_emphasis(
    nodes: &crate::NodeTree,
    layout_ctx: &mut LayoutContext<TextBrush>,
    font_ctx: &mut FontContext,
    scale: f32,
    node_id: NodeId,
) -> Option<TextAnnotation> {
    let style = nodes[node_id].primary_styles()?;
    let parley_style = stylo_to_parley::style(node_id, &style);
    let (mut layout, under) = emphasis_layout(layout_ctx, font_ctx, scale, &style, parley_style)?;
    layout.break_all_lines(None);
    Some(TextAnnotation { node_id, kind: TextAnnotationKind::Emphasis, under, layout: Box::new(layout) })
}
