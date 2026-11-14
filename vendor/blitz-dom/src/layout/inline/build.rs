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

use crate::{
    NodeData,
    layout::list::BULLET_FONT_FAMILY,
    layout::replaced::is_inline_box_element,
    layout::text_transform::{CaseTransform, TextTransformer},
    node::{
        FirstLetterSplit, ListItemLayout, ListItemLayoutPosition, Marker, TextAnnotation, TextAnnotationKind, TextBrush,
        TextLayout,
    },
    stylo_to_parley,
};

/// pyro: the inline-axis space the start and the end side of an inline (non-atomic) box take:
/// margin, border and padding, in CSS pixels. Percentages resolve against zero and `auto` margins
/// are zero. A vertical writing mode keeps the box edgeless.
pub(crate) fn span_edge_widths(style: &style::properties::ComputedValues) -> (f32, f32) {
    use style::values::computed::{Length, LengthPercentage, Margin};
    use style::values::generics::length::GenericMargin;
    if style.writing_mode.is_vertical() {
        return (0.0, 0.0);
    }
    let length = |v: &LengthPercentage| v.resolve(Length::new(0.0)).px();
    let margin = |v: &Margin| match v {
        GenericMargin::LengthPercentage(v) => length(v),
        _ => 0.0,
    };
    let (b, p, m) = (style.get_border(), style.get_padding(), style.get_margin());
    // The used width: none where the side has no border style.
    let border = |width: &style::values::computed::BorderSideWidth, style: style::values::computed::BorderStyle| {
        if style.none_or_hidden() { 0.0 } else { width.0.to_f32_px() }
    };
    let left = margin(&m.margin_left) + border(&b.border_left_width, b.border_left_style) + length(&p.padding_left.0);
    let right =
        margin(&m.margin_right) + border(&b.border_right_width, b.border_right_style) + length(&p.padding_right.0);
    if style.clone_direction() == style::computed_values::direction::T::Rtl {
        (right, left)
    } else {
        (left, right)
    }
}

pub(crate) fn build_inline_layout_into(
    nodes: &crate::NodeTree,
    layout_ctx: &mut LayoutContext<TextBrush>,
    font_ctx: &mut FontContext,
    text_layout: &mut TextLayout,
    scale: f32,
    inline_context_root_node_id: NodeId,
) {
    build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, false, &[]);
    // A line holding atomic inlines but no text still has its strut (CSS 2.1
    // 10.8.1): the root's font and line height. Parley makes no line height
    // without text, so a line of empty inline-blocks was 0px tall. A
    // word joiner (zero-width, no break opportunity) provides the strut.
    // pyro: so has the line of a list item's outside marker in an item without in-flow content.
    if (text_layout.layout.inline_boxes().len() > 0 || outside_marker(&nodes[inline_context_root_node_id]))
        && text_layout.text.chars().all(|c| c.is_whitespace() && c != '\n')
    {
        build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, true, &[]);
    }
}

/// pyro: whether the inline root is a list item with an outside marker (`layout::list`).
fn outside_marker(root: &crate::Node) -> bool {
    root.element_data()
        .and_then(|element| element.list_item_data.as_deref())
        .is_some_and(|item| matches!(item.position, ListItemLayoutPosition::Outside(_)))
}

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
) {
    // Get the inline context's root node's text styles
    let root_node = &nodes[inline_context_root_node_id];
    let root_node_style = root_node.primary_styles().or_else(|| {
        root_node
            .parent
            .and_then(|parent_id| nodes[parent_id].primary_styles())
    });

    let mut parley_style = root_node_style
        .as_ref()
        .map(|s| stylo_to_parley::style(inline_context_root_node_id, s))
        .unwrap_or_else(|| parley::TextStyle {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..Default::default()
        });

    // The formatting-context root's vertical alignment positions its box in
    // its parent's context (for example a middle-aligned table cell). It does
    // not shift that box's own line strut or inherited direct text. Descendant
    // spans and atomic inline boxes retain their separately collected styles.
    parley_style.vertical_align = parley::VerticalAlign::BASELINE;
    parley_style.annotation_space = space_of(annotation_space, inline_context_root_node_id);

    // Create a parley tree builder
    let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
    if let Some(style) = root_node_style.as_deref() {
        builder.set_base_direction(stylo_to_parley::base_direction(
            style.clone_direction(),
            style.clone_unicode_bidi(),
        ));
    }

    let text_transform = root_node_style
        .as_deref()
        .map(|s| CaseTransform::from_style(s))
        .unwrap_or(CaseTransform::NONE);

    // Render position-inside list items
    if let Some(ListItemLayout {
        marker,
        position: ListItemLayoutPosition::Inside,
        ..
    }) = root_node
        .element_data()
        .and_then(|el| el.list_item_data.as_deref())
    {
        match marker {
            // Bullet glyphs live in the bundled bullet font. The position-outside
            // path already asks for it; without the same span here a marker like
            // disclosure-closed (U+25B8) falls back to the element's own font and
            // renders as a missing glyph.
            Marker::Char(char) => {
                let mut marker_style = parley_style.clone();
                marker_style.font_family = BULLET_FONT_FAMILY.into();
                builder.push_style_span(marker_style);
                builder.push_text(&format!("{char} "));
                builder.pop_style_span();
            }
            Marker::String(str) => builder.push_text(str),
            // (Image markers are outside only: `layout::list`.)
            Marker::Image { .. } => {}
        }
    };
    // The marker is a separate box, so words in the content don't continue from it.
    let mut text_transformer = TextTransformer::default();
    text_transformer.word_break(&builder);
    let text_nodes = &mut Vec::new();
    // pyro: ruby annotations and emphasis marks found on the way (see `TextAnnotation`).
    let annotations = &mut FoundAnnotations { space: annotation_space.to_vec(), ..Default::default() };
    if let Some(style) = root_node.primary_styles() {
        annotations.enter(inline_context_root_node_id, &style);
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
    let strut = &mut { strut && !marker_line };
    // pyro: the first letter the root's `::first-letter` styles (`layout::first_letter`).
    let first_letter_split = text_layout.first_letter.clone();
    let first_letter = first_letter_split.as_ref();

    if let Some(before_id) = root_node.before() {
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes,
            before_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
        );
    }
    for child_id in root_node.children.iter().copied() {
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes,
            child_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
        );
    }
    if let Some(after_id) = root_node.after() {
        build_inline_layout_recursive(
            &mut builder,
            &mut text_transformer,
            nodes,
            after_id,
            &text_transform,
            text_nodes,
            strut,
            first_letter,
            annotations,
        );
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
        build_inline_layout_with(nodes, layout_ctx, font_ctx, text_layout, scale, inline_context_root_node_id, strut_build, &space);
        return;
    }
    // pyro: the "…" that `text-overflow: ellipsis` ends an overflowing line with, in the root's
    // own style (its first available font, as in Chromium).
    text_layout.ellipsis = root_node_style
        .as_deref()
        .filter(|style| ends_lines_with_ellipsis(style))
        .map(|_| {
            let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
            builder.push_text("\u{2026}");
            let mut layout = builder.build().0;
            layout.break_all_lines(None);
            Box::new(layout)
        });
    // pyro: the shaped text's content widths, for every layout of it (`TextLayout::
    // content_widths`): a function of the shaped clusters alone while no in-flow inline box, whose
    // size each layout sets, takes part in them. Measured with no text indent; the layout sets its
    // own before it reads them.
    let text_only = text_layout.layout.inline_boxes().all(|inline_box| inline_box.kind != InlineBoxKind::InFlow);
    text_layout.text_content_widths = text_only.then(|| {
        text_layout.layout.set_text_indent(0.0, IndentOptions::default());
        text_layout.layout.calculate_content_widths()
    });
    return;

    /// pyro: whether `text-overflow` puts an ellipsis at the end of the lines of a box with this
    /// style: the box clips its inline overflow, and the end side's value is `ellipsis`. Stylo
    /// keeps a single value as `second` (the end side); of two, `first` is the line-left side's,
    /// where right-to-left lines end.
    fn ends_lines_with_ellipsis(style: &style::properties::ComputedValues) -> bool {
        use style::computed_values::direction::T as Direction;
        use style::values::computed::Overflow;
        use style::values::specified::text::TextOverflowSide;
        if style.get_box().overflow_x == Overflow::Visible {
            return false;
        }
        let overflow = &style.get_text().text_overflow;
        let end = if !overflow.sides_are_logical && style.clone_direction() == Direction::Rtl {
            &overflow.first
        } else {
            &overflow.second
        };
        matches!(end, TextOverflowSide::Ellipsis)
    }

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
                        nodes,
                        child_id,
                        &text_transform,
                        &mut Vec::new(),
                        &mut false,
                        None,
                        &mut nested,
                    );
                }
                let under = matches!(itext.ruby_position, RubyPosition::Under | RubyPosition::AlternateUnder);
                (builder.build().0, under)
            }
            TextAnnotationKind::Emphasis => {
                let mark = emphasis_mark(&itext.text_emphasis_style)?;
                parley_style.font_size *= 0.5;
                parley_style.line_height = parley::LineHeight::FontSizeRelative(1.0);
                parley_style.letter_spacing = 0.0;
                parley_style.word_spacing = 0.0;
                let mut builder = layout_ctx.tree_builder(font_ctx, scale, true, &parley_style);
                builder.push_text(mark);
                // `auto` is `over right`, but `under right` for Chinese. In a vertical writing
                // mode the right side is over the line.
                let position = itext.text_emphasis_position;
                let under = if style.writing_mode.is_vertical() {
                    position.contains(TextEmphasisPosition::LEFT)
                } else if position.contains(TextEmphasisPosition::AUTO) {
                    let lang: &str = &style.get_font()._x_lang.0;
                    let lang = lang.to_ascii_lowercase();
                    lang == "zh" || lang.starts_with("zh-")
                } else {
                    position.contains(TextEmphasisPosition::UNDER)
                };
                (builder.build().0, under)
            }
        };
        layout.break_all_lines(None);
        Some(TextAnnotation { node_id, kind, under, layout: Box::new(layout) })
    }

    fn build_inline_layout_recursive(
        builder: &mut TreeBuilder<TextBrush>,
        text_transformer: &mut TextTransformer,
        nodes: &crate::NodeTree,
        node_id: NodeId,
        parent_text_transform: &CaseTransform,
        text_nodes: &mut Vec<(NodeId, std::ops::Range<usize>)>,
        strut: &mut bool,
        first_letter: Option<&FirstLetterSplit>,
        annotations: &mut FoundAnnotations,
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

                if node.is_line_break() {
                    // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    // TODO: update span id for br spans
                    builder.push_style_modification_span(&[
                        parley::StyleProperty::WhiteSpaceCollapse(WhiteSpaceCollapse::Preserve),
                    ]);
                    builder.push_text("\n");
                    builder.pop_style_span();
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
                        let whitespace = style.map(|s| {
                            [
                                parley::StyleProperty::WhiteSpaceCollapse(
                                    stylo_to_parley::white_space_collapse(
                                        s.clone_white_space_collapse(),
                                    ),
                                ),
                                parley::StyleProperty::TextWrapMode(
                                    stylo_to_parley::text_wrap_mode(s.clone_text_wrap_mode()),
                                ),
                            ]
                        });
                        builder.push_style_modification_span(
                            whitespace
                                .as_ref()
                                .map_or(&[][..], |styles| styles.as_slice()),
                        );
                        for child_id in node.children.iter().copied() {
                            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                            crate::util::ensure_stack(|| build_inline_layout_recursive(
                                builder,
                                text_transformer,
                                nodes,
                                child_id,
                                &text_transform,
                                text_nodes,
                                strut,
                                first_letter,
                                annotations,
                            ));
                        }
                        builder.pop_style_span();
                    }
                    (DisplayOutside::Inline, DisplayInside::Flow) => {
                        let tag_name = &element_data.name.local;

                        if is_inline_box_element(tag_name) {
                            if box_kind == InlineBoxKind::InFlow {
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
                            });
                        } else if annotations.is_ruby_annotation(tag_name) {
                            // pyro: a ruby annotation of the text since its base started, drawn
                            // over that text rather than in the line (`TextAnnotation`).
                            annotations.annotate_ruby_base(node_id, builder.text().len());
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

                            builder.push_style_span(style);
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

                            // pyro: the span's inline-axis margin, border and padding take space
                            // at its start and its end (CSS 2.1 9.4.2): an edge box at each, inside
                            // the span's style, when there is any.
                            let (start_edge, end_edge) =
                                node.primary_styles().map(|s| span_edge_widths(&s)).unwrap_or_default();
                            let scale = builder.scale();
                            let edge = |kind, width: f32| InlineBox {
                                id: node_id.as_u64(),
                                kind,
                                index: 0,
                                width: width * scale,
                                height: 0.0,
                                baseline: None,
                                vertical_align: Default::default(),
                            };
                            if start_edge != 0.0 {
                                builder.push_inline_box(edge(InlineBoxKind::SpanStart, start_edge));
                            }
                            // A span with such an edge is not empty: its line has the strut.
                            if (start_edge != 0.0 || end_edge != 0.0) && std::mem::take(strut) {
                                builder.push_text("\u{2060}");
                            }

                            if let Some(before_id) = node.before() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes,
                                    before_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                ));
                            }

                            for child_id in node.children.iter().copied() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes,
                                    child_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                ));
                            }
                            if let Some(after_id) = node.after() {
                                crate::util::ensure_stack(|| build_inline_layout_recursive(
                                    builder,
                                    text_transformer,
                                    nodes,
                                    after_id,
                                    &text_transform,
                                    text_nodes,
                                    strut,
                                    first_letter,
                                    annotations,
                                ));
                            }

                            if is_ruby {
                                (annotations.ruby_base_start, annotations.ruby) = outer_ruby;
                            }
                            if end_edge != 0.0 {
                                builder.push_inline_box(edge(InlineBoxKind::SpanEnd, end_edge));
                            }
                            builder.pop_style_span();
                        }
                    }
                    // Inline box
                    (_, _) => {
                        if box_kind == InlineBoxKind::InFlow {
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
                        });
                    }
                };
            }
            NodeData::Text(data) => {
                // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                // dbg!(&data.content);

                let start = builder.text().len();
                // pyro: the first letter in a span of the `::first-letter` style.
                let letter = first_letter
                    .filter(|split| split.text == node_id && data.content.get(split.range.clone()).is_some())
                    .and_then(|split| Some((split, nodes.get(split.style_node)?.primary_styles()?.clone())));
                match letter {
                    Some((split, letter_style)) => {
                        let content = &data.content;
                        let head = &content[..split.range.start];
                        let text = text_transformer.transform(head, parent_text_transform, builder);
                        builder.push_text(text);
                        builder.push_style_span(stylo_to_parley::style(split.style_node, &letter_style));
                        let letter = &content[split.range.clone()];
                        let text = text_transformer.transform(letter, &CaseTransform::from_style(&letter_style), builder);
                        builder.push_text(text);
                        builder.pop_style_span();
                        let tail = &content[split.range.end..];
                        let text = text_transformer.transform(tail, parent_text_transform, builder);
                        builder.push_text(text);
                    }
                    None => {
                        let text =
                            text_transformer.transform(&data.content, parent_text_transform, builder);
                        builder.push_text(text);
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
}

impl FoundAnnotations {
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
