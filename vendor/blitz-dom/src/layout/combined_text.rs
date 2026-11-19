//! pyro: `text-combine-upright: all` text that is no inline element's (CSS Writing Modes 3 § 9.1).
//!
//! Stylo makes an inline element whose text is combined in a vertical line a horizontal box one em
//! square (`adjust_for_text_combine_upright`), which the line places as an atomic inline. Text that
//! is the direct content of a vertical block container (or of the anonymous block holding its
//! inline content) has no such element: each such text run gets a box of its own here, as
//! `layout::first_letter` gives an initial letter one. The box is no child of the DOM: it is an
//! anonymous span holding a copy of the text, a layout child of the inline root, which builds an
//! inline box for it in place of the text (`TextNodeData::combined_box`).

use blitz_traits::node_id::NodeId;
use markup5ever::{QualName, local_name, ns};
use style::{
    data::ElementData as StyloElementData,
    properties::ComputedValues,
    servo_arc::Arc,
    values::computed::{Display, TextCombineUpright},
};

use crate::{BaseDocument, ElementData, NodeData, node::NodeFlags};

/// Whether the inline root `root`'s direct text is combined text that needs boxes.
fn root_combines(style: &ComputedValues) -> bool {
    style.get_inherited_text().text_combine_upright == TextCombineUpright::All
        && style.writing_mode.is_vertical()
        && !style.writing_mode.is_sideways()
        // Emphasis marks of combined text are left as Stylo leaves them (upright glyphs).
        && matches!(style.get_inherited_text().text_emphasis_style, style::values::computed::TextEmphasisStyle::None)
}

/// Gives each direct text child of the inline root `root` that is combined text its box (and
/// takes the boxes of those that are not), before the root's inline layout is built
/// (`BaseDocument::resolve_deferred_tasks`).
pub(crate) fn prepare(doc: &mut BaseDocument, root: NodeId) {
    prepare_marker(doc, root);
    let Some(style) = doc.nodes[root].primary_styles().map(|style| Arc::clone(&*style)) else { return };
    let combines = root_combines(&style);
    // The root's direct text, and the text of its inline elements (`digits N`).
    let mut texts: Vec<(NodeId, Arc<ComputedValues>)> = Vec::new();
    collect_texts(doc, root, &style, &mut texts);
    for (text_id, parent_style) in texts {
        let content = match &doc.nodes[text_id].data {
            NodeData::Text(data) => data.content.clone(),
            _ => continue,
        };
        let direct = doc.nodes[text_id].parent == Some(root);
        let wanted = if direct && combines {
            !content.trim().is_empty()
        } else {
            digits_combine(&parent_style, content.trim())
        };
        let style = parent_style;
        let existing = match &doc.nodes[text_id].data {
            NodeData::Text(data) => data.combined_box.filter(|&id| doc.nodes.contains_key(id)),
            _ => None,
        };
        if !wanted {
            if let Some(box_id) = existing {
                drop_box(doc, root, box_id);
            }
            if let NodeData::Text(data) = &mut doc.nodes[text_id].data {
                data.combined_box = None;
            }
            continue;
        }
        let box_id = existing.unwrap_or_else(|| {
            const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("span") };
            doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(NAME, Vec::new()))))
        });
        set_box_style(doc, box_id, &style);
        set_box_text(doc, root, box_id, content.trim());
        {
            let mut children = doc.nodes[root].layout_children.borrow_mut();
            let children = children.get_or_insert_with(Default::default);
            if !children.contains(&box_id) {
                children.push(box_id);
            }
        }
        build_box_layout(doc, box_id);
        if let NodeData::Text(data) = &mut doc.nodes[text_id].data {
            data.combined_box = Some(box_id);
        }
    }
}

/// pyro: an inside list marker whose `::marker` style combines its text in a vertical line gets a
/// box as combined text does (`ElementData::marker_combined_box`), which the item's inline layout
/// places in place of the marker's text (WPT css-pseudo `marker-text-combine-upright*`).
fn prepare_marker(doc: &mut BaseDocument, root: NodeId) {
    use crate::node::{ListItemLayoutPosition, Marker};
    let Some(element) = doc.nodes[root].element_data() else { return };
    let existing = element.marker_combined_box.filter(|&id| doc.nodes.contains_key(id));
    let wanted = element.list_item_data.as_deref().and_then(|item| {
        let text = match (&item.position, &item.marker) {
            (ListItemLayoutPosition::Inside, Marker::String(text)) => text.clone(),
            (ListItemLayoutPosition::Inside, Marker::Char(c)) => format!("{c} "),
            _ => return None,
        };
        let style = doc.nodes.get(item.style_node?)?.primary_styles().map(|style| Arc::clone(&*style))?;
        // (In the item's writing mode: the marker's style node is no box of its own.)
        let writing_mode = doc.nodes[root].primary_styles()?.writing_mode;
        (style.get_inherited_text().text_combine_upright == TextCombineUpright::All
            && writing_mode.is_vertical()
            && !writing_mode.is_sideways()
            && !text.trim().is_empty())
        .then_some((text, style))
    });
    let Some((text, style)) = wanted else {
        if let Some(box_id) = existing {
            drop_box(doc, root, box_id);
        }
        if let Some(element) = doc.nodes[root].element_data_mut() {
            element.marker_combined_box = None;
        }
        return;
    };
    let box_id = existing.unwrap_or_else(|| {
        const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("span") };
        doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(NAME, Vec::new()))))
    });
    set_box_style(doc, box_id, &style);
    set_box_text(doc, root, box_id, &text);
    {
        let mut children = doc.nodes[root].layout_children.borrow_mut();
        let children = children.get_or_insert_with(Default::default);
        if !children.contains(&box_id) {
            children.push(box_id);
        }
    }
    build_box_layout(doc, box_id);
    if let Some(element) = doc.nodes[root].element_data_mut() {
        element.marker_combined_box = Some(box_id);
    }
}

/// pyro: the text nodes of the inline root `root`: its own and those of its inline elements
/// (not of atomic inlines, which are roots of their own), each with its parent's style.
fn collect_texts(doc: &BaseDocument, parent: NodeId, parent_style: &Arc<ComputedValues>, out: &mut Vec<(NodeId, Arc<ComputedValues>)>) {
    for &child in &doc.nodes[parent].children {
        let node = &doc.nodes[child];
        match &node.data {
            NodeData::Text(_) => out.push((child, Arc::clone(parent_style))),
            NodeData::Element(_) => {
                let Some(style) = node.primary_styles().map(|style| Arc::clone(&*style)) else { continue };
                let display = style.get_box().display;
                if display.is_contents()
                    || (display.outside() == style::values::specified::box_::DisplayOutside::Inline
                        && display.inside() == style::values::specified::box_::DisplayInside::Flow)
                {
                    collect_texts(doc, child, &style, out);
                }
            }
            _ => {}
        }
    }
}

/// pyro: `text-combine-upright: digits N` in a vertical line: text that is a run of one to N ASCII
/// digits is combined (CSS Writing Modes 3 § 9.1.1). Digit runs inside other text are not split out.
fn digits_combine(style: &ComputedValues, text: &str) -> bool {
    let Some(limit) = style.get_inherited_text().text_combine_upright.digits() else { return false };
    style.writing_mode.is_vertical()
        && !style.writing_mode.is_sideways()
        && !text.is_empty()
        && text.chars().count() <= limit
        && text.chars().all(|c| c.is_ascii_digit())
}

/// The box's style: the root's inherited properties, as Stylo adjusts a combined inline element
/// (horizontal, one em square, not wrapping, centred, the line one em tall), an inline-block.
fn set_box_style(doc: &mut BaseDocument, box_id: NodeId, parent: &ComputedValues) {
    use style::computed_values::writing_mode::T as WritingModeProperty;
    use style::values::computed::{Length, LengthPercentage, Size, TextAlign};
    use style::values::generics::NonNegative;
    use style::values::specified::text::TextWrapMode;
    let mut builder =
        style::properties::StyleBuilder::for_inheritance(doc.stylist.device(), Some(&doc.stylist), Some(parent), None);
    builder.set_display(Display::InlineBlock);
    builder.add_flags(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED);
    builder.mutate_inherited_box().set_writing_mode(WritingModeProperty::HorizontalTb);
    builder.writing_mode = style::logical_geometry::WritingMode::new(builder.get_inherited_box());
    let em = parent.get_font().slow_clone_font_size().computed_size();
    {
        let text = builder.mutate_inherited_text();
        text.set_text_wrap_mode(TextWrapMode::Nowrap);
        text.set_text_align(TextAlign::Center);
    }
    let one_em = || Size::LengthPercentage(NonNegative(LengthPercentage::new_length(em)));
    builder.mutate_position().set_width(one_em());
    builder.mutate_position().set_height(one_em());
    builder
        .mutate_font()
        .set_line_height(style::values::computed::LineHeight::Length(NonNegative(Length::new(em.px()))));
    let style = builder.build();
    let node = &mut doc.nodes[box_id];
    let mut data = StyloElementData::default();
    data.styles.primary = Some(style);
    data.set_restyled();
    data.damage = super::damage::ALL_DAMAGE;
    *node.stylo_element_data_mut().ensure_init_mut() = data;
    node.update_layout_styles();
}

/// The box holds a copy of the text and is an inline root, a layout child of `root`.
fn set_box_text(doc: &mut BaseDocument, root: NodeId, box_id: NodeId, content: &str) {
    let existing = doc.nodes[box_id]
        .children
        .first()
        .copied()
        .filter(|&id| matches!(doc.nodes.get(id).map(|node| &node.data), Some(NodeData::Text(_))));
    let text_id = match existing {
        Some(text_id) => {
            if let NodeData::Text(data) = &mut doc.nodes[text_id].data
                && data.content != content
            {
                data.content = content.to_string();
            }
            text_id
        }
        None => {
            let text_id = doc.create_text_node(content);
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
    node.parent = Some(root);
    node.flags.insert(NodeFlags::IS_INLINE_ROOT);
    *node.layout_children.borrow_mut() = Some(Default::default());
    node.layout_parent.set(Some(root));
}

fn build_box_layout(doc: &mut BaseDocument, box_id: NodeId) {
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

fn drop_box(doc: &mut BaseDocument, root: NodeId, box_id: NodeId) {
    if let Some(children) = doc.nodes[root].layout_children.borrow_mut().as_mut() {
        children.retain(|&id| id != box_id);
    }
    for child in std::mem::take(&mut doc.nodes[box_id].children) {
        doc.remove_node_from_tree(child);
    }
    doc.remove_node_from_tree(box_id);
}
