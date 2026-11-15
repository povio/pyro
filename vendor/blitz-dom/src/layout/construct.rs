use blitz_traits::node_id::NodeId;
use core::str;
use std::sync::Arc;
use stylo_taffy::CopyStyleGetters;

use markup5ever::{QualName, local_name, ns};
use parley::StyleProperty;
use style::{
    computed_values::position::T as PositionProperty,
    data::ElementData as StyloElementData,
    shared_lock::StylesheetGuards,
    values::{
        computed::{Content, Display, Float},
        specified::box_::{DisplayInside, DisplayOutside},
    },
};
use thin_vec::ThinVec;

use crate::{
    BaseDocument, ElementData, Node, NodeData,
    layout::damage::{
        CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC, INLINE_CONTENT, sort_layout_children_by_order,
    },
    node::{NodeFlags, NodeKind, SpecialElementData, TextInputData, TextLayout},
    qual_name, stylo_to_parley,
    traversal::{iter_children, iter_children_and_pseudos},
};

use super::{
    counters::Piece, damage::ALL_DAMAGE, list::collect_list_item_children,
    replaced::is_inline_box, table::build_table_context,
};

const DUMMY_NAME: QualName = qual_name!("div", html);

#[derive(Clone)]
pub(crate) struct ConstructionTask {
    pub(crate) node_id: NodeId,
    pub(crate) data: ConstructionTaskData,
}

pub(crate) struct ConstructionTaskResult {
    pub(crate) node_id: NodeId,
    pub(crate) data: ConstructionTaskResultData,
}

#[derive(Clone)]
pub(crate) enum ConstructionTaskData {
    InlineLayout(Box<TextLayout>),
}

pub(crate) enum ConstructionTaskResultData {
    InlineLayout(Box<TextLayout>),
}

/// Accumulator threaded through layout-child collection.
///
/// `children` is the list of layout children being built up, and
/// `anonymous_block_id` is the currently-open anonymous block container (if
/// any) that wrapping children are being appended to.
#[derive(Default)]
pub(crate) struct LayoutChildren {
    pub(crate) children: ThinVec<NodeId>,
    pub(crate) anonymous_block_id: Option<NodeId>,
    /// All anonymous blocks created while collecting these layout children.
    ///
    /// These are recorded on the container node so they can be deallocated the
    /// next time it is reconstructed.
    pub(crate) anonymous_blocks: ThinVec<NodeId>,
}

impl LayoutChildren {
    /// Append a single layout child.
    fn push(&mut self, child_id: NodeId, doc: &mut BaseDocument) {
        self.maybe_push_anon_block(doc);
        self.children.push(child_id);
    }

    /// Append all layout children in `slice`.
    fn extend(&mut self, slice: &[NodeId], doc: &mut BaseDocument) {
        self.maybe_push_anon_block(doc);
        self.children.extend_from_slice(slice);
    }

    fn maybe_push_anon_block(&mut self, doc: &mut BaseDocument) {
        fn block_is_only_whitespace(doc: &BaseDocument, node_id: NodeId) -> bool {
            for child_id in doc.nodes[node_id].children.iter().copied() {
                let child = &doc.nodes[child_id];
                if !child.is_collapsible_whitespace_node() {
                    return false;
                }
            }

            true
        }

        // If anonymous block node only contains whitespace then delete it
        if let Some(anon_id) = self.anonymous_block_id {
            if block_is_only_whitespace(doc, anon_id) {
                // Remove by identity, not pop(): hoisted display:contents
                // children may have been pushed after the anon block.
                if let Some(pos) = self.children.iter().rposition(|id| *id == anon_id) {
                    self.children.remove(pos);
                }
                self.anonymous_blocks.retain(|id| *id != anon_id);
                doc.remove_node_from_tree(anon_id);
            }
        }

        self.anonymous_block_id = None;
    }

    fn push_wrapped(
        &mut self,
        container_node_id: NodeId,
        child_id: NodeId,
        doc: &mut BaseDocument,
    ) {
        if self.anonymous_block_id.is_none() {
            self.create_anonymous_block(container_node_id, doc);
        }
        doc.nodes[self.anonymous_block_id.unwrap()]
            .children
            .push(child_id);
    }

    fn create_anonymous_block(&mut self, container_node_id: NodeId, doc: &mut BaseDocument) {
        use style::selector_parser::PseudoElement;

        const NAME: QualName = QualName {
            prefix: None,
            ns: ns!(html),
            local: local_name!("div"),
        };
        let node_id = doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(
            NAME,
            Vec::new(),
        ))));

        // Set style data. pyro: the anonymous blocks of an inline box split around block-level
        // content are boxes of the block container around it (CSS 2.1 § 9.2.1.1), so they
        // inherit from it: its strut, alignment and direction; the split inline is a span in them.
        let mut style_node_id = container_node_id;
        while doc.nodes[style_node_id].is_split_inline() {
            match doc.nodes[style_node_id].parent {
                Some(parent) if doc.nodes[parent].primary_styles().is_some() => style_node_id = parent,
                _ => break,
            }
        }
        let parent_style = doc.nodes[style_node_id].primary_styles().unwrap();
        let read_guard = doc.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        let style = doc.stylist.style_for_anonymous::<&Node>(
            &guards,
            &PseudoElement::ServoAnonymousBox,
            &parent_style,
        );
        let mut stylo_element_data = StyloElementData {
            damage: ALL_DAMAGE,
            ..Default::default()
        };
        drop(parent_style);

        stylo_element_data.styles.primary = Some(style);
        stylo_element_data.set_restyled();

        *doc.nodes[node_id]
            .stylo_element_data_mut()
            .ensure_init_mut() = stylo_element_data;
        doc.nodes[node_id].update_layout_styles();

        if doc.nodes[container_node_id]
            .flags
            .contains(NodeFlags::IS_IN_DOCUMENT)
        {
            doc.nodes[node_id].flags.insert(NodeFlags::IS_IN_DOCUMENT);
        }
        doc.nodes[node_id].parent = Some(container_node_id);
        doc.nodes[node_id]
            .layout_parent
            .set(Some(container_node_id));

        self.children.push(node_id);
        self.anonymous_block_id = Some(node_id);
        self.anonymous_blocks.push(node_id);
    }
}

fn push_children_and_pseudos(layout_children: &mut ThinVec<NodeId>, node: &Node) {
    if let Some(before) = node.before() {
        layout_children.push(before);
    }
    layout_children.extend(node.children.iter().copied().filter(|child_id| {
        let child_node = node.with(*child_id);
        child_node.data.kind() != NodeKind::Comment
    }));
    if let Some(after) = node.after() {
        layout_children.push(after);
    }
}

/// Wrapping policy of the nearest non-contents ancestor container. Children
/// hoisted out of display:contents nodes must be wrapped in anonymous blocks
/// exactly as if they were direct children of that container: without this,
/// a bare text node could end up as the layout child of a block/flex/grid
/// container, which cannot be laid out (text nodes carry no style).
#[derive(Copy, Clone)]
struct WrapContext {
    /// The container whose style anonymous blocks inherit from.
    container_node_id: NodeId,
    needs_wrap: fn(NodeKind, DisplayOutside) -> bool,
}

fn block_item_needs_wrap(child_node_kind: NodeKind, display_outside: DisplayOutside) -> bool {
    child_node_kind == NodeKind::Text || display_outside == DisplayOutside::Inline
}

/// Used for flex/grid containers, and for flow containers whose in-flow
/// children are all out-of-flow (where inline elements are pushed raw, but
/// bare text still cannot be laid out directly).
fn text_item_needs_wrap(child_node_kind: NodeKind, _display_outside: DisplayOutside) -> bool {
    child_node_kind == NodeKind::Text
}

/// Push a single hoisted child, recursing through display:contents nodes and
/// wrapping text/inline children per the ancestor container's `WrapContext`.
fn push_hoisted_child(
    doc: &mut BaseDocument,
    child_id: NodeId,
    out: &mut LayoutChildren,
    wrap: Option<WrapContext>,
) {
    let child = &doc.nodes[child_id];
    let child_display = child.display_style().unwrap_or(Display::inline());
    if matches!(child_display.inside(), DisplayInside::Contents) {
        collect_layout_children_with_wrap(doc, child_id, out, wrap);
        return;
    }
    // No box: it doesn't end a run of inline content.
    if matches!(child_display.inside(), DisplayInside::None) && out.anonymous_block_id.is_some() {
        out.children.push(child_id);
        return;
    }
    if let Some(wrap_ctx) = wrap {
        let child_node_kind = if child.is_line_break() {
            NodeKind::Text
        } else {
            child.data.kind()
        };
        let display_outside = if child.is_or_contains_block() {
            DisplayOutside::Block
        } else {
            child_display.outside()
        };
        if (wrap_ctx.needs_wrap)(child_node_kind, display_outside) {
            out.push_wrapped(wrap_ctx.container_node_id, child_id, doc);
            return;
        }
    }
    out.push(child_id, doc);
}

/// Push the container's children (and ::before/::after pseudos) as layout
/// children, hoisting transparently through display:contents nodes and
/// filtering out comments and whitespace.
fn push_hoisted_children_and_pseudos(
    doc: &mut BaseDocument,
    container_node_id: NodeId,
    out: &mut LayoutChildren,
    wrap: Option<WrapContext>,
) {
    if let Some(before) = doc.nodes[container_node_id].before() {
        push_hoisted_child(doc, before, out, wrap);
    }
    // Take children array from node to avoid borrow checker issues.
    let children = std::mem::take(&mut doc.nodes[container_node_id].children);
    for child_id in children.iter().copied() {
        let child = &doc.nodes[child_id];
        if child.data.kind() == NodeKind::Comment || child.is_collapsible_whitespace_node() {
            continue;
        }
        push_hoisted_child(doc, child_id, out, wrap);
    }
    doc.nodes[container_node_id].children = children;
    if let Some(after) = doc.nodes[container_node_id].after() {
        push_hoisted_child(doc, after, out, wrap);
    }
}

fn push_non_whitespace_children_and_pseudos(layout_children: &mut ThinVec<NodeId>, node: &Node) {
    if let Some(before) = node.before() {
        layout_children.push(before);
    }
    layout_children.extend(node.children.iter().copied().filter(|child_id| {
        let child_node = node.with(*child_id);
        !child_node.is_whitespace_node() && child_node.data.kind() != NodeKind::Comment
    }));
    if let Some(after) = node.after() {
        layout_children.push(after);
    }
}

/// Result of classifying the in-flow children of a flow container as
/// all-block, all-inline and/or all-out-of-flow.
struct FlowClassification {
    all_block: bool,
    all_inline: bool,
    all_out_of_flow: bool,
    has_contents: bool,
    /// An in-flow inline-level child that is laid out in an anonymous block (text, or an inline box without block content).
    has_inline_level: bool,
}

impl Default for FlowClassification {
    fn default() -> Self {
        Self {
            all_block: true,
            all_inline: true,
            all_out_of_flow: true,
            has_contents: false,
            has_inline_level: false,
        }
    }
}

/// Classify a flow container's children (including its ::before/::after
/// pseudo-elements) for inline-vs-block layout, recursing transparently
/// through display:contents nodes (whose children participate in the
/// container's formatting context).
fn classify_flow_children(
    doc: &BaseDocument,
    container_node_id: NodeId,
    classification: &mut FlowClassification,
) {
    let node = &doc.nodes[container_node_id];
    // ::before/::after pseudos vote like children: a pseudo with display:contents is
    // transparent for box generation, so its children (e.g. generated text) participate
    // in the container's formatting context, and any other pseudo generates its own
    // inline or block box in that formatting context.
    let pseudo_ids = node.before().into_iter().chain(node.after());
    let child_ids = node.children.iter().copied().chain(pseudo_ids);
    let child_count = node.children.len();
    for (index, child_id) in child_ids.enumerate() {
        let child = &doc.nodes[child_id];

        // pyro: a split inline's head or tail is inline content.
        if index < child_count && node.split_ref_at(index, child_count).is_some() {
            classification.all_out_of_flow = false;
            classification.all_block = false;
            classification.has_inline_level = true;
            continue;
        }

        // Comment nodes generate no boxes and must not affect the
        // inline-vs-block classification: an unstyled comment would
        // default to display:inline below and force an inline
        // formatting context on the container, swallowing element
        // siblings into the inline layout (zero-sizing any
        // out-of-flow ones).
        if child.data.kind() == NodeKind::Comment {
            continue;
        }

        // Unwraps on Text and SVG nodes
        let style = child.primary_styles();
        let style = style.as_ref();
        let display = child.display_style().unwrap_or(Display::inline());
        if matches!(display.inside(), DisplayInside::Contents) {
            // Transparent for box generation: the contents node casts
            // no vote itself — its children decide.
            classification.has_contents = true;
            crate::util::ensure_stack(|| classify_flow_children(doc, child_id, classification));
        } else if matches!(display.inside(), DisplayInside::None) {
            // display:none children generate no boxes and cast no vote.
            continue;
        } else if child.is_line_break() {
            classification.all_out_of_flow = false;
            classification.all_block = false;
        } else {
            let position = style
                .map(|s| s.clone_position())
                .unwrap_or(PositionProperty::Static);
            let float = style.map(|s| s.clone_float()).unwrap_or(Float::None);

            // Ignore nodes whose whitespace is entirely collapsed away
            if child.is_collapsible_whitespace_node() {
                continue;
            }

            // display:none children generate no boxes and cast no vote
            if matches!(display.outside(), DisplayOutside::None) {
                continue;
            }

            let is_in_flow = matches!(
                position,
                PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky
            ) && matches!(float, Float::None);

            if !is_in_flow {
                continue;
            }

            classification.all_out_of_flow = false;
            match display.outside() {
                DisplayOutside::None => {}
                DisplayOutside::Block
                | DisplayOutside::TableCaption
                | DisplayOutside::InternalTable => classification.all_inline = false,
                DisplayOutside::Inline => {
                    classification.all_block = false;

                    // We need the "complex" tree fixing when an inline contains a block
                    if child.is_or_contains_block() {
                        classification.all_inline = false;
                    } else {
                        classification.has_inline_level = true;
                    }
                }
            }
        }
    }
}

pub(crate) fn collect_layout_children(
    doc: &mut BaseDocument,
    container_node_id: NodeId,
    out: &mut LayoutChildren,
) {
    collect_layout_children_with_wrap(doc, container_node_id, out, None);
    place_inside_marker(doc, container_node_id, out);
    // pyro: table parts among a block container's children are wrapped in anonymous tables
    // (CSS 2.1 §17.2.1, `table_fixup`). A table fixes up its own children; an inline root's
    // layout children are its atomic inlines.
    let node = &doc.nodes[container_node_id];
    let is_table = node.display_style().is_some_and(|display| display.inside() == DisplayInside::Table);
    if !is_table && !node.flags.is_inline_root() {
        let LayoutChildren { children, anonymous_blocks, .. } = out;
        super::table_fixup::wrap_misparented_table_parts(doc, container_node_id, children, anonymous_blocks);
    }
}

/// pyro: the inside marker of a list item that is no inline root of its own (it holds blocks) goes
/// on the first line of its first box (CSS Lists 3, Chromium): an anonymous block of its first
/// inline content takes it, else (a block first) the marker is an anonymous block's line before it.
fn place_inside_marker(doc: &mut BaseDocument, item: NodeId, out: &mut LayoutChildren) {
    use crate::node::ListItemLayoutPosition;
    let node = &doc.nodes[item];
    if node.flags.is_inline_root() || node.is_anonymous() {
        return;
    }
    let Some(marker) = node
        .element_data()
        .and_then(|element| element.list_item_data.as_deref())
        .filter(|data| matches!(data.position, ListItemLayoutPosition::Inside))
        .cloned()
    else {
        return;
    };
    // (An empty item's inside symbol marker stays unpainted, as below.)
    if out.children.is_empty() {
        return;
    }
    let block = match out.children.first() {
        Some(&first) if out.anonymous_blocks.contains(&first) => first,
        _ => {
            out.create_anonymous_block(item, doc);
            let block = out.children.pop().unwrap();
            out.children.insert(0, block);
            out.anonymous_block_id = None;
            block
        }
    };
    if let Some(element) = doc.nodes[block].element_data_mut() {
        element.list_item_data = Some(Box::new(marker));
    }
}

fn collect_layout_children_with_wrap(
    doc: &mut BaseDocument,
    container_node_id: NodeId,
    out: &mut LayoutChildren,
    wrap: Option<WrapContext>,
) {
    // Record the `display` the box is being constructed with
    if let Some(display) = doc.nodes[container_node_id].display_style() {
        *doc.nodes[container_node_id].display_constructed_as_mut() = display;
    }

    // pyro: a table built as another box now: its row groups and rows were parts of the
    // table, never boxes of their own, and are built as boxes now.
    let was_table = doc.nodes[container_node_id].flags.is_table_root();
    if was_table
        && doc.nodes[container_node_id]
            .display_style()
            .is_some_and(|display| display.inside() != DisplayInside::Table)
    {
        super::table::damage_former_table_parts(doc, container_node_id);
    }

    // Reset construction flags
    // TODO: make incremental and only remove this if the element is no longer an inline root
    doc.nodes[container_node_id]
        .flags
        .reset_construction_flags();
    // An inline root built again reuses its layout: the build keeps the lines its last break
    // produced over the text that did not change (`build_into_keeping_lines`).
    let previous_inline_layout = doc.nodes[container_node_id]
        .element_data_mut()
        .and_then(|element_data| element_data.take_inline_layout());

    flush_pseudo_elements(doc, container_node_id);

    // pyro: a select that was a drop-down had its options'
    // construct damage cleared (below); as a list box they must build their
    // boxes again.
    let listbox_select = doc.nodes[container_node_id].data.downcast_element().is_some_and(|el| {
        el.name.local == local_name!("select")
            && (el.attr(local_name!("multiple")).is_some()
                || el.attr(local_name!("size")).and_then(|s| s.trim().parse::<u32>().ok()).is_some_and(|s| s > 1))
    });
    if listbox_select {
        doc.iter_subtree_mut(container_node_id, |id: NodeId, doc: &mut BaseDocument| {
            if id != container_node_id {
                doc.nodes[id].insert_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT | CONSTRUCT_FC);
            }
        });
    }

    if let Some(el) = doc.nodes[container_node_id].data.downcast_element() {
        // Handle text inputs
        let tag_name = el.name.local.as_ref();
        if matches!(tag_name, "input" | "textarea") {
            let type_attr: Option<&str> = doc.nodes[container_node_id]
                .data
                .downcast_element()
                .and_then(|el| el.attr(local_name!("type")));
            if tag_name == "textarea" {
                create_text_editor(doc, container_node_id, true);
                return;
            } else if matches!(
                type_attr,
                None | Some("text" | "password" | "email" | "number" | "search" | "tel" | "url" | "time")
            ) {
                create_text_editor(doc, container_node_id, false);
                return;
            } else if matches!(type_attr, Some("checkbox" | "radio")) {
                create_checkbox_input(doc, container_node_id);
                return;
            }
        }

        // pyro: a drop-down select is an atomic box sized from its
        // options (layout/mod.rs); as in Chromium's menu list, its children
        // generate no boxes (their computed `display` stays `block`). A list
        // box (`multiple`, or `size` above 1) lays its options out.
        if tag_name == "select" {
            let listbox = el.attr(local_name!("multiple")).is_some()
                || el
                    .attr(local_name!("size"))
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .is_some_and(|s| s > 1);
            if !listbox {
                doc.iter_subtree_mut(container_node_id, |id: NodeId, doc: &mut BaseDocument| {
                    doc.nodes[id].remove_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT | CONSTRUCT_FC);
                });
                return;
            }

        }

        // pyro: an `<object>` showing an image is replaced content (`replaced::is_image_object`):
        // its children, the fallback, generate no boxes.
        if crate::layout::replaced::is_image_object(el) {
            doc.iter_subtree_mut(container_node_id, |id: NodeId, doc: &mut BaseDocument| {
                doc.nodes[id].remove_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT | CONSTRUCT_FC);
            });
            return;
        }

        #[cfg(feature = "svg")]
        if matches!(tag_name, "svg") {
            let outer_html = doc.with_svg_sprites(doc.get_node(container_node_id).unwrap().svg_source());
            // pyro: with the resources it references in the page's other `<svg>`s.
            let outer_html = doc.with_cross_svg_references(outer_html);

            // Remove contruction damage from subtree
            doc.iter_subtree_mut(container_node_id, |id: NodeId, doc: &mut BaseDocument| {
                doc.nodes[id].remove_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT | CONSTRUCT_FC);
            });

            match crate::util::parse_svg_image(outer_html.as_bytes()) {
                Ok(mut svg) => {
                    // pyro: without a `viewBox` its content is laid out in its CSS box
                    // (`SvgImageData::sized`, at paint).
                    if svg.intrinsic_dimensions.view_box.is_none() {
                        svg.inline_source = Some(outer_html.as_str().into());
                    }
                    doc.get_node_mut(container_node_id)
                        .unwrap()
                        .element_data_mut()
                        .unwrap()
                        .special_data =
                        SpecialElementData::Image(Box::new(crate::node::ImageData::Svg(svg)));
                }
                Err(err) => {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        node_id = ?container_node_id,
                        html = outer_html,
                        error = ?err,
                        "SVG parse failed",
                    );
                    #[cfg(not(feature = "tracing"))]
                    let _ = err;
                }
            };
            return;
        }

        //Only ol tags have start and reversed attributes
        let (mut index, reversed) = if tag_name == "ol" {
            (
                el.attr_parsed(local_name!("start"))
                    .map(|start: usize| start - 1)
                    .unwrap_or(0),
                el.attr_parsed(local_name!("reversed")).unwrap_or(false),
            )
        } else {
            (0, false)
        };
        collect_list_item_children(doc, &mut index, reversed, container_node_id);
    }

    // pyro: a list item's inside marker that is text (a number, or a symbol the page styles
    // through `::marker`) is inline content of its own: an item without any other has a line with
    // the marker, and so has the anonymous block an item of blocks puts its marker in
    // (`place_inside_marker`). (An empty item's bundled-font symbol marker stays unpainted: painting
    // it exposes WPT `list-style-position-applies-to-015`, an empty item in a table caption, until
    // table captions render.) An item with an outside marker and no in-flow content has the line
    // the marker sits on (Chromium: the marker's box sets the item's height and baseline), a line
    // of the item's strut (`inline::build`).
    let inside_marker = doc.nodes[container_node_id]
        .element_data()
        .and_then(|element| element.list_item_data.as_deref())
        .is_some_and(|item| match item.position {
            crate::node::ListItemLayoutPosition::Inside => {
                matches!(item.marker, crate::node::Marker::String(_))
                    || doc.nodes[container_node_id].is_anonymous()
            }
            crate::node::ListItemLayoutPosition::Outside(_) => true,
        });

    // Skip further construction if the node has no children or psuedo-children
    {
        let node = &doc.nodes[container_node_id];
        if node.children.is_empty() && node.before().is_none() && node.after().is_none() && !inside_marker {
            return;
        }
    }

    let container_display = doc.nodes[container_node_id].display_style().unwrap_or(
        match doc.nodes[container_node_id].data.kind() {
            NodeKind::AnonymousBlock => Display::Block,
            _ => Display::Inline,
        },
    );

    match container_display.inside() {
        DisplayInside::None => {}
        DisplayInside::Contents => {
            doc.nodes[container_node_id]
                .remove_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT | CONSTRUCT_FC);
            // display:contents is transparent for box generation: hoist the
            // children THEMSELVES (not their layout children) into the
            // parent, recursing only through nested contents nodes.
            push_hoisted_children_and_pseudos(doc, container_node_id, out, wrap);
        }
        DisplayInside::Flex | DisplayInside::Grid => {
            // ::before/::after pseudos must be checked too: a pseudo with
            // display:contents hoists its text content into the container.
            let container = &doc.nodes[container_node_id];
            let has_text_node_or_contents = container
                .children
                .iter()
                .copied()
                .chain(container.before())
                .chain(container.after())
                .map(|child_id| &doc.nodes[child_id])
                .any(|child| {
                    let display = child.display_style().unwrap_or(Display::inline());
                    let node_kind = child.data.kind();
                    display.inside() == DisplayInside::Contents
                        || node_kind == NodeKind::Text
                        || child.is_line_break()
                });

            if !has_text_node_or_contents {
                push_non_whitespace_children_and_pseudos(
                    &mut out.children,
                    &doc.nodes[container_node_id],
                );
            } else {
                collect_complex_layout_children(
                    doc,
                    container_node_id,
                    out,
                    true,
                    text_item_needs_wrap,
                );
            }

            // Flex/grid items are laid out in order-modified document order.
            sort_layout_children_by_order(&doc.nodes, &mut out.children);
        }

        DisplayInside::Table => {
            let built = build_table_context(doc, container_node_id);
            #[allow(clippy::arc_with_non_send_sync)]
            let data = SpecialElementData::TableRoot(Arc::new(built.context));
            doc.nodes[container_node_id]
                .flags
                .insert(NodeFlags::IS_TABLE_ROOT);
            doc.nodes[container_node_id]
                .data
                .downcast_element_mut()
                .unwrap()
                .special_data = data;
            // pyro: the table's pseudo-elements are in its rows and cells (`table_fixup`), and
            // the anonymous rows and cells it made are its own.
            out.extend(&built.layout_children, doc);
            out.anonymous_blocks.extend(built.anonymous);
        }

        // Flow, FlowRoot and TableCell, plus internal table displays (row,
        // row group, column, ...) occurring outside of a table. Blitz does
        // not yet generate anonymous table wrapper boxes for the latter, so
        // they are laid out as flow containers, which crucially ensures
        // their text children get wrapped rather than being pushed as bare
        // layout children (text nodes carry no style and cannot be laid out
        // as block/flex/grid items).
        _ => {
            let mut classification = FlowClassification::default();
            classify_flow_children(doc, container_node_id, &mut classification);

            // pyro: an inline box with block-level content is split around it (CSS 2.1 § 9.2.1.1).
            let split_inline = doc.nodes[container_node_id]
                .display_style()
                .is_some_and(|display| display.is_inline_flow())
                && !classification.all_inline
                && doc.nodes[container_node_id]
                    .parent
                    .is_some_and(|parent| lays_out_as_block(&doc.nodes[parent]));
            if split_inline {
                doc.nodes[container_node_id].flags.insert(NodeFlags::IS_SPLIT_INLINE);
            }

            // Inline box whose in-flow content is block-level only: it is laid out as block
            // boxes, so paint must not draw its own box over them.
            if doc.nodes[container_node_id]
                .display_style()
                .is_some_and(|display| display.is_inline_flow())
                && !classification.all_inline
                && !classification.has_inline_level
            {
                doc.nodes[container_node_id]
                    .flags
                    .insert(NodeFlags::IS_INLINE_WITH_ONLY_BLOCK_CHILDREN);
            }

            if classification.all_out_of_flow && !inside_marker {
                // Contents-transparent: a display:contents child may be
                // holding the out-of-flow elements (otherwise the contents
                // node itself would be pushed as a layout box).
                let wrap = Some(WrapContext {
                    container_node_id,
                    needs_wrap: text_item_needs_wrap,
                });
                return push_hoisted_children_and_pseudos(doc, container_node_id, out, wrap);
            }

            // TODO: fix display:contents
            // pyro: only a box holds an inline layout. The document node has none: a root
            // element that is inline content (a `<br>` appended to a document whose element was
            // removed) is its layout child, as every other root element (the queued inline layout
            // had no element to go to: WPT css/css-text/white-space/
            // pre-line-br-with-whitespace-child-crash.html).
            let holds_inline_layout = doc.nodes[container_node_id].element_data().is_some();
            if classification.all_inline && holds_inline_layout {
                // pyro: an inline root built again for a change to its box alone (a
                // cell's padding or collapsed border, its parent's children) keeps the inline
                // layout its content built: its text, the styles the text is shaped with, its
                // children and the boxes among them are as they were (no `INLINE_CONTENT` on it
                // or below it; read after `flush_pseudo_elements`, which damages the owner of a
                // pseudo-element that came, went or changed its text).
                let content_changed = doc.nodes[container_node_id]
                    .damage()
                    .is_none_or(|damage| damage.contains(INLINE_CONTENT));
                match previous_inline_layout {
                    Some(layout) if !content_changed => {
                        #[cfg(debug_assertions)]
                        assert_kept_inline_layout_is_current(doc, container_node_id, &layout);
                        doc.nodes[container_node_id].element_data_mut().unwrap().inline_layout_data = Some(layout);
                    }
                    previous => {
                        let layout = previous.unwrap_or_else(|| Box::new(TextLayout::new()));

                        // Queue node for inline layout construction. Deferring construction of inline layouts to a
                        // dedicated phase allows us to multithread the expensive text shaping step.
                        doc.deferred_construction_nodes.push(ConstructionTask {
                            node_id: container_node_id,
                            data: ConstructionTaskData::InlineLayout(layout),
                        });
                    }
                }
                doc.nodes[container_node_id]
                    .flags
                    .insert(NodeFlags::IS_INLINE_ROOT);

                find_inline_layout_embedded_boxes(doc, container_node_id, &mut out.children);
                return;
            }

            // pyro: a split inline holds what is between its first and its last block-level child;
            // its head and tail are inline content of its container's anonymous blocks.
            if split_inline {
                let middle = split_inline_middle(doc, container_node_id);
                collect_complex_layout_children_in(doc, container_node_id, out, false, block_item_needs_wrap, Some(middle));
                return;
            }

            // If the children are either all inline or all block then simply return the regular children
            // as the layout children
            if classification.all_block & !classification.has_contents {
                return push_non_whitespace_children_and_pseudos(
                    &mut out.children,
                    &doc.nodes[container_node_id],
                );
            } else if classification.all_inline & !classification.has_contents {
                return push_children_and_pseudos(&mut out.children, &doc.nodes[container_node_id]);
            }

            collect_complex_layout_children(
                doc,
                container_node_id,
                out,
                false,
                block_item_needs_wrap,
            );
        }
    }
}

/// pyro: the children of a split inline (with its `::before` and `::after`, comments left out) in
/// order, and whether each is in its middle: from its first block-level child (a block, or an
/// inline box that holds one) to its last, with the out-of-flow boxes and collapsible white space
/// right after the last, and right before the first when only white space is before them (a float
/// there floats beside the blocks, and paints as theirs do, as when the split inline was one box;
/// after inline content it stays on that content's line).
fn split_inline_sequence(nodes: &crate::NodeTree, node_id: NodeId) -> Vec<(NodeId, bool)> {
    let node = &nodes[node_id];
    let mut sequence: Vec<(NodeId, bool)> = node
        .before()
        .into_iter()
        .chain(node.children.iter().copied())
        .chain(node.after())
        .filter(|&id| nodes[id].data.kind() != NodeKind::Comment)
        .map(|id| (id, nodes[id].is_or_contains_block()))
        .collect();
    let out_of_flow = |id: NodeId| {
        nodes[id].primary_styles().is_some_and(|style| {
            style.clone_position().is_absolutely_positioned() || style.clone_float().is_floating()
        })
    };
    let whitespace = |id: NodeId| nodes[id].is_collapsible_whitespace_node();
    if let (Some(first), Some(last)) =
        (sequence.iter().position(|&(_, block)| block), sequence.iter().rposition(|&(_, block)| block))
    {
        // The out-of-flow boxes (and the white space among them) right before the first block
        // when nothing else is before them (a float after text stays on the text's line), and
        // right after the last.
        let mut start = first;
        for index in (0..first).rev() {
            let id = sequence[index].0;
            if out_of_flow(id) {
                start = index;
            } else if !whitespace(id) {
                start = first;
                break;
            }
        }
        let mut end = last;
        for index in last + 1..sequence.len() {
            let id = sequence[index].0;
            if out_of_flow(id) {
                end = index;
            } else if !whitespace(id) {
                break;
            }
        }
        for entry in &mut sequence[start..=end] {
            entry.1 = true;
        }
    }
    sequence
}

/// pyro: whether `node_id` is an inline box that holds block-level content, which construction
/// splits (CSS 2.1 § 9.2.1.1) when its container lays out as a block container.
pub(crate) fn is_split_inline(nodes: &crate::NodeTree, node_id: NodeId) -> bool {
    let node = &nodes[node_id];
    node.display_style().is_some_and(|display| display.is_inline_flow())
        && node.is_or_contains_block()
        && node.parent.is_some_and(|parent| lays_out_as_block(&nodes[parent]))
}

/// pyro: whether `node` lays its children out as a block container (a flow, flow-root or table
/// cell box), where an inline box with block-level content is split around it. Elsewhere (a table
/// part outside a table, laid out as a flex row) it stays one box laying them out as a block.
/// Both the container and the split inline's own construction ask it of the split inline's parent
/// in the tree, so that they agree when the container is an anonymous box around it.
fn lays_out_as_block(node: &Node) -> bool {
    match node.display_style() {
        Some(display) => matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot | DisplayInside::TableCell),
        None => node.is_anonymous(),
    }
}

/// pyro: the indices (into `split_inline_sequence`) of the first and last block-level children
/// of a split inline: what its own box holds.
fn split_inline_middle(doc: &BaseDocument, node_id: NodeId) -> std::ops::RangeInclusive<usize> {
    let sequence = split_inline_sequence(&doc.nodes, node_id);
    let first = sequence.iter().position(|&(_, block)| block).unwrap_or(0);
    let last = sequence.iter().rposition(|&(_, block)| block).unwrap_or(0);
    first..=last
}

/// pyro: the head (`tail: false`) or the tail of the split inline `node_id`: its children before
/// its first block-level child or after its last, in order, with that child when it is a split
/// inline itself, whose head (tail) is part of this one's: after the children for a head, before
/// them for a tail.
pub(crate) fn split_inline_part(nodes: &crate::NodeTree, node_id: NodeId, tail: bool) -> (Vec<NodeId>, Option<NodeId>) {
    let sequence = split_inline_sequence(nodes, node_id);
    let at = if tail {
        sequence.iter().rposition(|&(_, middle)| middle)
    } else {
        sequence.iter().position(|&(_, middle)| middle)
    };
    // A split inline right at the middle's edge has its head (tail) in this one's.
    let nested = at.map(|at| sequence[at].0).filter(|&id| is_split_inline(nodes, id));
    let part = match (at, tail) {
        (Some(at), false) => &sequence[..at],
        (Some(at), true) => &sequence[at + 1..],
        (None, _) => &sequence[..],
    };
    (part.iter().map(|&(id, _)| id).collect(), nested)
}

/// pyro: what a pseudo-element's `content` generates, with counters and quotes evaluated
/// (`layout::counters`): text, and an image per `url()`.
fn pe_content(
    doc: &BaseDocument,
    owner: NodeId,
    idx: usize,
    style: &style::properties::ComputedValues,
) -> Option<Vec<Piece>> {
    match doc.generated_text.get(&owner).and_then(|texts| texts[idx].as_ref()) {
        Some(pieces) => Some(pieces.clone()),
        None => super::counters::plain_content(style, owner),
    }
}

/// pyro: makes a pseudo-element's children what its content generates: a text node per run of
/// text and an `img` box per image, an inline replaced box inheriting from the pseudo-element.
/// Whether they changed.
fn sync_generated_children(
    doc: &mut BaseDocument,
    pe_node_id: NodeId,
    pe_style: &style::servo_arc::Arc<style::properties::ComputedValues>,
    pieces: &[Piece],
) -> bool {
    let same = {
        let children = &doc.nodes[pe_node_id].children;
        children.len() == pieces.len()
            && children.iter().zip(pieces).all(|(&child, piece)| {
                let child = &doc.nodes[child];
                match piece {
                    Piece::Text(text) => child.text_data().is_some_and(|data| data.content == *text),
                    Piece::Image(url) => child.element_data().is_some_and(|element| {
                        element.name.local == local_name!("img") && element.attr(local_name!("src")) == Some(url.as_str())
                    }),
                }
            })
    };
    if same {
        return false;
    }
    // A single text that changed (a counter's value) is edited in place.
    let single = match (pieces, &doc.nodes[pe_node_id].children[..]) {
        ([Piece::Text(text)], [child]) => Some((text.clone(), *child)),
        _ => None,
    };
    if let Some((text, child)) = single {
        if let Some(data) = doc.nodes[child].text_data_mut() {
            data.content = text;
            return true;
        }
    }
    for child in std::mem::take(&mut doc.nodes[pe_node_id].children) {
        doc.remove_node_from_tree(child);
    }
    let in_document = doc.nodes[pe_node_id].flags.contains(NodeFlags::IS_IN_DOCUMENT);
    for piece in pieces {
        let child = match piece {
            Piece::Text(text) => doc.create_text_node(text),
            Piece::Image(url) => {
                let src = crate::node::Attribute {
                    name: QualName::new(None, ns!(), local_name!("src")),
                    value: url.clone(),
                };
                const IMG: QualName = qual_name!("img", html);
                let child = doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(IMG, vec![src]))));
                let style = style::properties::StyleBuilder::for_inheritance(
                    doc.stylist.device(),
                    Some(&doc.stylist),
                    Some(&**pe_style),
                    None,
                )
                .build();
                let mut element_data = StyloElementData::default();
                element_data.styles.primary = Some(style);
                element_data.set_restyled();
                element_data.damage = ALL_DAMAGE;
                *doc.nodes[child].stylo_element_data_mut().ensure_init_mut() = element_data;
                doc.nodes[child].update_layout_styles();
                if in_document {
                    doc.nodes[child].flags.insert(NodeFlags::IS_IN_DOCUMENT);
                }
                doc.load_generated_image(child, url);
                child
            }
        };
        doc.nodes[child].parent = Some(pe_node_id);
        doc.nodes[pe_node_id].children.push(child);
    }
    true
}

pub(crate) fn flush_pseudo_elements(doc: &mut BaseDocument, node_id: NodeId) {
    let (before_style, after_style, before_node_id, after_node_id) = {
        let node = &doc.nodes[node_id];

        let before_node_id = node.before();
        let after_node_id = node.after();

        // Note: yes these are kinda backwards
        let style_data = node.try_stylo_element_data().and_then(|s| s.get());
        let before_style = style_data
            .as_ref()
            .and_then(|d| d.styles.pseudos.as_array()[1].clone());
        let after_style = style_data
            .as_ref()
            .and_then(|d| d.styles.pseudos.as_array()[0].clone());

        // pyro: `content: normal | none` generates no box (an
        // unset `var()` in `content` computes to `normal`).
        let generates =
            |s: &style::servo_arc::Arc<style::properties::ComputedValues>| !matches!(s.get_counters().content, Content::Normal | Content::None);
        let before_style = before_style.filter(generates);
        let after_style = after_style.filter(generates);

        (before_style, after_style, before_node_id, after_node_id)
    };
    // pyro: an optgroup's label row, as Chromium's
    // `-internal-optgroup-label` box, in the `::before` slot when the
    // optgroup has no `::before` of its own.
    let label = before_style.is_none().then(|| optgroup_label(doc, node_id)).flatten();
    let (before_style, label_text) = match label {
        Some((style, text)) => (Some(style), Some(text)),
        None => (before_style, None),
    };

    // Sync pseudo element
    // TODO: Make incremental
    for (idx, pe_style, pe_node_id) in [
        (1, before_style, before_node_id),
        (0, after_style, after_node_id),
    ] {
        // Delete psuedo element if it exists but shouldn't
        if let (Some(pe_node_id), None) = (pe_node_id, &pe_style) {
            doc.remove_and_drop_pe(pe_node_id);
            let node = &mut doc.nodes[node_id];
            node.set_pe_by_index(idx, None);
            node.insert_damage(ALL_DAMAGE);
        }

        // Create pseudo element if it should exist but doesn't
        if let (None, Some(pe_style)) = (pe_node_id, &pe_style) {
            let new_node_id = doc.create_node(NodeData::AnonymousBlock(Box::new(
                ElementData::new(DUMMY_NAME, Vec::new()),
            )));
            doc.nodes[new_node_id].parent = Some(node_id);
            doc.nodes[new_node_id].layout_parent.set(Some(node_id));
            if doc.nodes[node_id].flags.contains(NodeFlags::IS_IN_DOCUMENT) {
                doc.nodes[new_node_id]
                    .flags
                    .insert(NodeFlags::IS_IN_DOCUMENT);
            }

            let pieces = if idx == 1 && label_text.is_some() {
                label_text.clone().map(|text| vec![Piece::Text(text)])
            } else {
                pe_content(doc, node_id, idx, pe_style)
            };

            doc.nodes[new_node_id].record_out_of_flow(pe_style);
            let mut element_data = StyloElementData::default();
            element_data.styles.primary = Some(pe_style.clone());
            element_data.set_restyled();
            element_data.damage = ALL_DAMAGE;
            *doc.nodes[new_node_id]
                .stylo_element_data_mut()
                .ensure_init_mut() = element_data;
            doc.nodes[new_node_id].update_layout_styles();
            // pyro: damage is set with its flag (`Node::mark_damaged`), which the walks
            // that follow damage rely on to skip a subtree (`Node::has_no_damage_below`).
            doc.nodes[new_node_id].mark_damaged();
            sync_generated_children(doc, new_node_id, pe_style, pieces.as_deref().unwrap_or(&[]));

            let node = &mut doc.nodes[node_id];
            node.set_pe_by_index(idx, Some(new_node_id));
            node.insert_damage(ALL_DAMAGE);

            doc.pending_style_image_nodes.push(new_node_id);
        }

        // Else: Update psuedo element
        if let (Some(pe_node_id), Some(pe_style)) = (pe_node_id, pe_style) {
            // Sync the pseudo-element's text node with its `content` style, which
            // may have changed (e.g. `details[open] summary::after { content: ... }`).
            //
            // Note: this deliberately compares the text itself rather than relying on
            // the style-pointer comparison below, as the pseudo-element's style may
            // already have been updated by `sync_pseudo_element_styles` during the
            // style traversal without the text having been updated.
            let pieces = if idx == 1 && label_text.is_some() {
                label_text.clone().map(|text| vec![Piece::Text(text)])
            } else {
                pe_content(doc, node_id, idx, &pe_style)
            };
            if sync_generated_children(doc, pe_node_id, &pe_style, pieces.as_deref().unwrap_or(&[])) {
                doc.nodes[node_id].insert_damage(ALL_DAMAGE);
            }

            doc.nodes[pe_node_id].record_out_of_flow(&pe_style);
            let replaced = {
                let mut node_styles = doc.nodes[pe_node_id]
                    .try_stylo_element_data_mut()
                    .and_then(|s| s.get_mut());
                let node_styles = &mut node_styles.as_mut().unwrap();
                node_styles.damage.insert(ALL_DAMAGE);
                let primary_styles = &mut node_styles.styles.primary;

                let replaced = !std::ptr::eq(&**primary_styles.as_ref().unwrap(), &*pe_style);
                if replaced {
                    *primary_styles = Some(pe_style);
                    node_styles.set_restyled();
                }
                replaced
            };
            // pyro: with its flag, as above.
            doc.nodes[pe_node_id].mark_damaged();
            if replaced {
                doc.nodes[pe_node_id].update_layout_styles();
                doc.pending_style_image_nodes.push(pe_node_id);
            }
        }
    }
}

/// pyro: the style (UA `optgroup::-servo-text-control-inner-container`,
/// a pseudo-element pages cannot select) and text (its `label`) of an
/// optgroup's label row.
fn optgroup_label(
    doc: &BaseDocument,
    node_id: NodeId,
) -> Option<(style::servo_arc::Arc<style::properties::ComputedValues>, String)> {
    use style::selector_parser::PseudoElement;
    use style::stylist::RuleInclusion;
    let node = &doc.nodes[node_id];
    let el = node.data.downcast_element()?;
    if el.name.local != local_name!("optgroup") {
        return None;
    }
    // An empty label still has a line, as Chromium's row is a line tall
    // (its `min-height: 1.2em` is a fraction of a pixel short of it); a
    // no-break space, as it comes from the label's own font.
    let text = match el.attr(local_name!("label")).unwrap_or("") {
        "" => "\u{A0}".to_string(),
        label => label.to_string(),
    };
    let parent_style = node.primary_styles()?;
    let read_guard = doc.guard.read();
    let guards = StylesheetGuards::same(&read_guard);
    let style = doc.stylist.lazily_compute_pseudo_element_style::<&Node>(
        &guards,
        node,
        &PseudoElement::ServoTextControlInnerContainer,
        RuleInclusion::All,
        &parent_style,
        false,
        None,
    )?;
    Some((style, text))
}

/// Handles the cases where there are text nodes or inline nodes that need to be wrapped in an anonymous block node
fn collect_complex_layout_children(
    doc: &mut BaseDocument,
    container_node_id: NodeId,
    out: &mut LayoutChildren,
    hide_whitespace: bool,
    needs_wrap: fn(NodeKind, DisplayOutside) -> bool,
) {
    collect_complex_layout_children_in(doc, container_node_id, out, hide_whitespace, needs_wrap, None);
}

/// pyro: `collect_complex_layout_children` over the children (and pseudo-elements, comments left
/// out) whose index is in `middle`, the middle of a split inline: a split inline child at its start
/// has its head in the container's head (`split_inline_part`), and one at its end its tail in the
/// container's tail, so they do not stand for them here.
fn collect_complex_layout_children_in(
    doc: &mut BaseDocument,
    container_node_id: NodeId,
    out: &mut LayoutChildren,
    hide_whitespace: bool,
    needs_wrap: fn(NodeKind, DisplayOutside) -> bool,
    middle: Option<std::ops::RangeInclusive<usize>>,
) {
    let mut index = 0usize;
    doc.iter_children_and_pseudos_mut(container_node_id, |child_id, doc| {
        let mut at_middle_start = false;
        let mut at_middle_end = false;
        if doc.nodes[child_id].data.kind() != NodeKind::Comment {
            index += 1;
            if let Some(middle) = &middle {
                if !middle.contains(&(index - 1)) {
                    return;
                }
                at_middle_start = index - 1 == *middle.start();
                at_middle_end = index - 1 == *middle.end();
            }
        }
        // Get node kind (text, element, comment, etc). A `<br>` is a piece of text.
        let child_node_kind = if doc.nodes[child_id].is_line_break() {
            NodeKind::Text
        } else {
            doc.nodes[child_id].data.kind()
        };

        // Get Display style. Default to inline because nodes without styles are probably text nodes
        let contains_block = doc.nodes[child_id].is_or_contains_block();
        let child_display = &doc.nodes[child_id]
            .display_style()
            .unwrap_or(Display::inline());
        let display_inside = child_display.inside();
        let display_outside = if contains_block {
            DisplayOutside::Block
        } else {
            child_display.outside()
        };

        let is_whitespace_node = doc.nodes[child_id].is_whitespace_node();

        // Skip comment nodes. Note that we do *not* skip `Display::None` nodes as they may need to be hidden.
        // Taffy knows how to deal with `Display::None` children.
        //
        // Also hide all-whitespace flexbox children as these should be ignored
        //
        // White space that follows text or a `<br>` (an open anonymous block) stays if it's preserved.
        let keep_whitespace = out.anonymous_block_id.is_some()
            && !doc.nodes[child_id].is_collapsible_whitespace_node();
        if child_node_kind == NodeKind::Comment
            || (hide_whitespace && is_whitespace_node && !keep_whitespace)
        {
            // return;
        }
        // No box: it doesn't end a run of inline content.
        else if display_inside == DisplayInside::None && out.anonymous_block_id.is_some() {
            out.children.push(child_id);
        }
        // Recurse into `Display::Contents` nodes, wrapping hoisted text and
        // inline children as if they were direct children of this container
        else if display_inside == DisplayInside::Contents {
            let wrap = Some(WrapContext {
                container_node_id,
                needs_wrap,
            });
            collect_layout_children_with_wrap(doc, child_id, out, wrap)
        }
        // Push nodes that need wrapping into the current "anonymous block container".
        // If there is not an open one then we create one.
        else if needs_wrap(child_node_kind, display_outside) {
            out.push_wrapped(container_node_id, child_id, doc);
        }
        // A float after inline content belongs to that inline formatting context
        // (it sits beside the line it follows rather than below it).
        else if out.anonymous_block_id.is_some()
            && doc.nodes[child_id]
                .primary_styles()
                .is_some_and(|s| s.clone_float() != Float::None)
        {
            out.push_wrapped(container_node_id, child_id, doc);
        }
        // pyro: so does an absolutely positioned box that would be inline-level (its `display`
        // before `position` made it a block): its static position is on the line, after the
        // inline content it follows (WPT `remove-block-between-inline-and-abspos`).
        else if out.anonymous_block_id.is_some()
            && doc.nodes[child_id].primary_styles().is_some_and(|s| {
                s.clone_position().is_absolutely_positioned()
                    && s.get_box().original_display.outside() == DisplayOutside::Inline
            })
        {
            out.push_wrapped(container_node_id, child_id, doc);
        }
                // pyro: an inline box with block-level content is split around it (CSS 2.1 § 9.2.1.1):
        // its head (the inline content before its first block-level box) ends the open run of
        // inline content, in an anonymous block it is the last child of, and its tail starts the
        // next one; its own box holds what is between (`NodeFlags::IS_SPLIT_INLINE`).
        else if contains_block
            && child_display.is_inline_flow()
            && doc.nodes[child_id].parent.is_some_and(|parent| lays_out_as_block(&doc.nodes[parent]))
        {
            if !at_middle_start {
                out.push_wrapped(container_node_id, child_id, doc);
                if let Some(head) = out.anonymous_block_id {
                    doc.nodes[head].flags.insert(NodeFlags::ENDS_WITH_SPLIT_HEAD);
                }
            }
            // Its own construction builds its box (it may have been an inline of a line before).
            doc.nodes[child_id].insert_damage(CONSTRUCT_BOX);
            doc.nodes[child_id].mark_damaged();
            out.push(child_id, doc);
            if !at_middle_end {
                out.push_wrapped(container_node_id, child_id, doc);
                if let Some(tail) = out.anonymous_block_id {
                    doc.nodes[tail].flags.insert(NodeFlags::STARTS_WITH_SPLIT_TAIL);
                }
            }
        }
        // Else push the child directly (and close any open "anonymous block container")
        else {
            out.push(child_id, doc);
        }
    });

    // If anonymous block node only contains whitespace then delete it, else push it
    out.maybe_push_anon_block(doc);
}

/// pyro: whether a single-line input's text is laid out at `line-height:
/// normal` whatever the input's line height is, as the inner editor of
/// Chromium's input is (Blink's `CreateInnerEditorStyle`): when the line height
/// is no more than the font size, when the input has a fixed height above the
/// line height, and when its height is a percentage or a `calc()`. The line
/// height counts in whole pixels, cut. The placeholder's line is `normal` always.
pub(crate) fn input_line_is_normal(style: &style::properties::ComputedValues) -> bool {
    use style::values::computed::font::LineHeight;
    use style::values::generics::length::GenericSize;
    let font_size = style.slow_clone_font_size().used_size().px();
    let line_height = match style.clone_line_height() {
        LineHeight::Normal => return true,
        LineHeight::Number(number) => (font_size * number.0).trunc(),
        LineHeight::Length(length) => length.0.px().trunc(),
    };
    if font_size >= line_height {
        return true;
    }
    let height = if style.writing_mode.is_vertical() { style.slow_clone_width() } else { style.slow_clone_height() };
    match height {
        GenericSize::LengthPercentage(height) => match height.0.to_length() {
            Some(height) => height.px() > line_height,
            None => true,
        },
        _ => false,
    }
}

fn create_text_editor(doc: &mut BaseDocument, input_element_id: NodeId, is_multiline: bool) {
    let node = &mut doc.nodes[input_element_id];
    let mut parley_style = node
        .primary_styles()
        .as_ref()
        .map(|s| stylo_to_parley::style(node.id, s))
        .unwrap_or_default();
    if !is_multiline && node.primary_styles().is_some_and(|s| input_line_is_normal(&s)) {
        parley_style.line_height = parley::LineHeight::NORMAL;
    }
    let alignment = node
        .primary_styles()
        .map(|s| stylo_to_parley::text_align(s.clone_text_align()))
        .unwrap_or(parley::layout::Alignment::Start);
    let base_direction = node
        .primary_styles()
        .map(|s| stylo_to_parley::base_direction(s.clone_direction(), s.clone_unicode_bidi()))
        .unwrap_or(parley::BaseDirection::Auto);

    let initial_text = if is_multiline {
        node.text_content()
    } else {
        node.attr(local_name!("value")).unwrap_or("").to_string()
    };
    let placeholder = input_placeholder(doc, input_element_id, is_multiline);
    // pyro: the `text-overflow` ellipsis of the control's lines, in its own style.
    let ellipsis = doc.nodes[input_element_id]
        .primary_styles()
        .and_then(|style| super::inline::build::line_end_ellipsis(&style, true))
        .map(|text| {
            let mut font_ctx = doc.font_ctx.lock().unwrap();
            let mut builder =
                doc.layout_ctx.tree_builder(&mut font_ctx, doc.viewport.scale(), true, &parley_style);
            builder.set_base_direction(base_direction);
            builder.push_text(&text);
            let mut layout = builder.build().0;
            layout.break_all_lines(None);
            (Box::new(layout), text)
        });
    let node = &mut doc.nodes[input_element_id];
    let is_focussed = node.is_focussed();
    let element = &mut node.data.downcast_element_mut().unwrap();
    if !matches!(element.special_data, SpecialElementData::TextInput(_)) {
        let mut text_input_data = TextInputData::new(is_multiline);
        let editor = &mut text_input_data.editor;
        editor.set_text(&initial_text);
        element.special_data = SpecialElementData::TextInput(text_input_data);

        // An input focused before its editor existed (e.g. focused before the first layout)
        // could not enable IME when it was focused, so enable it now.
        if is_focussed {
            doc.shell_provider.set_ime_enabled(true);
        }
    }

    let SpecialElementData::TextInput(text_input_data) = &mut element.special_data else {
        unreachable!();
    };

    // pyro: a textarea's placeholder is broken into lines at its width by layout.
    text_input_data.placeholder = placeholder;
    text_input_data.ellipsis = ellipsis;
    let editor = &mut text_input_data.editor;
    editor.set_scale(doc.viewport.scale_f64() as f32);
    editor.set_width(None);

    let styles = editor.edit_styles();
    styles.retain(|_| false);
    styles.insert(StyleProperty::FontFamily(parley_style.font_family));
    styles.insert(StyleProperty::FontSize(parley_style.font_size));
    styles.insert(StyleProperty::FontWidth(parley_style.font_width));
    styles.insert(StyleProperty::FontStyle(parley_style.font_style));
    styles.insert(StyleProperty::FontWeight(parley_style.font_weight));
    styles.insert(StyleProperty::FontVariations(parley_style.font_variations));
    styles.insert(StyleProperty::FontFeatures(parley_style.font_features));
    styles.insert(StyleProperty::LineHeight(parley_style.line_height));
    styles.insert(StyleProperty::WordSpacing(parley_style.word_spacing));
    styles.insert(StyleProperty::LetterSpacing(parley_style.letter_spacing));
    styles.insert(StyleProperty::WordBreak(parley_style.word_break));
    styles.insert(StyleProperty::OverflowWrap(parley_style.overflow_wrap));
    styles.insert(StyleProperty::TextWrapMode(parley_style.text_wrap_mode));
    styles.insert(StyleProperty::WhiteSpaceCollapse(
        parley_style.white_space_collapse,
    ));
    styles.insert(StyleProperty::Brush(parley_style.brush));
    editor.set_alignment(alignment);
    editor.set_base_direction(base_direction);

    editor.refresh_layout(&mut doc.font_ctx.lock().unwrap(), &mut doc.layout_ctx);
}

/// pyro: the `placeholder` attribute of an `<input>` or `<textarea>` laid out
/// in its `::placeholder` style, as Chromium paints it while the value is
/// empty: a single-line input's text loses its line breaks (HTML), and the UA
/// sheet's `line-height: initial !important` makes its line `normal` (Stylo
/// applies no author `line-height` to `::placeholder` either). `None` when the
/// element has no placeholder text.
fn input_placeholder(
    doc: &mut BaseDocument,
    node_id: NodeId,
    is_multiline: bool,
) -> Option<Box<crate::node::InputPlaceholder>> {
    use style::selector_parser::PseudoElement;
    use style::stylist::RuleInclusion;
    let node = &doc.nodes[node_id];
    let attr = node.attr(local_name!("placeholder"))?;
    let text: String = if is_multiline {
        attr.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        attr.chars().filter(|c| !matches!(c, '\n' | '\r')).collect()
    };
    if text.is_empty() {
        return None;
    }
    let input_style = node.primary_styles()?;
    let placeholder_style = {
        let read_guard = doc.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        doc.stylist.lazily_compute_pseudo_element_style::<&Node>(
            &guards,
            node,
            &PseudoElement::Placeholder,
            RuleInclusion::All,
            &input_style,
            false,
            None,
        )
    };
    let style: &style::properties::ComputedValues = match &placeholder_style {
        Some(style) => style,
        None => &input_style,
    };
    let mut text_style = stylo_to_parley::style(node_id, style);
    text_style.line_height = parley::LineHeight::NORMAL;
    let mut color = style.clone_color();
    color.alpha *= style.get_effects().opacity;
    let mut placeholder = Box::new(crate::node::InputPlaceholder {
        text,
        style: text_style,
        layout: parley::Layout::new(),
        color,
        alignment: stylo_to_parley::text_align(style.clone_text_align()),
        base_direction: stylo_to_parley::base_direction(style.clone_direction(), style.clone_unicode_bidi()),
    });
    let scale = doc.viewport.scale();
    let mut font_ctx = doc.font_ctx.lock().unwrap();
    placeholder.relayout(&mut font_ctx, &mut doc.layout_ctx, scale, None);
    Some(placeholder)
}

fn create_checkbox_input(doc: &mut BaseDocument, input_element_id: NodeId) {
    let node = &mut doc.nodes[input_element_id];

    let element = &mut node.data.downcast_element_mut().unwrap();
    if !matches!(element.special_data, SpecialElementData::CheckboxInput(_)) {
        let checked = element.has_attr(local_name!("checked"));
        element.special_data = SpecialElementData::CheckboxInput(checked);
        element.set_checkbox_input_checked(checked);
    }
}

/// Find and return the "layout_children" (inline boxes) for an inline layout
/// without actually constructing the layout. This allows us to defer the expensive
/// construction of the Parley layout (which invokes text shaping) to a paralell phase.
pub(crate) fn find_inline_layout_embedded_boxes(
    doc: &mut BaseDocument,
    inline_context_root_node_id: NodeId,
    layout_children: &mut ThinVec<NodeId>,
) {
    flush_inline_pseudos_recursive(doc, inline_context_root_node_id);

    // pyro: an anonymous block standing for a split inline's head or tail holds only that part
    // of it (the split inline itself is a box of its container).
    let root = &doc.nodes[inline_context_root_node_id];
    if root.flags.intersects(NodeFlags::ENDS_WITH_SPLIT_HEAD | NodeFlags::STARTS_WITH_SPLIT_TAIL) {
        let children = root.children.clone();
        for (index, child_id) in children.iter().copied().enumerate() {
            match doc.nodes[inline_context_root_node_id].split_ref_at(index, children.len()) {
                Some(tail) => find_split_part_boxes(&mut doc.nodes, inline_context_root_node_id, child_id, tail, layout_children),
                None => find_inline_layout_embedded_boxes_recursive(
                    &mut doc.nodes,
                    inline_context_root_node_id,
                    child_id,
                    layout_children,
                ),
            }
        }
        return;
    }

    iter_children_and_pseudos!(doc.nodes[inline_context_root_node_id], |child_id| {
        find_inline_layout_embedded_boxes_recursive(
            &mut doc.nodes,
            inline_context_root_node_id,
            child_id,
            layout_children,
        );
    });

    fn find_split_part_boxes(
        nodes: &mut crate::NodeTree,
        root_id: NodeId,
        split_id: NodeId,
        tail: bool,
        layout_children: &mut ThinVec<NodeId>,
    ) {
        let (part, nested) = split_inline_part(nodes, split_id, tail);
        if let (true, Some(nested)) = (tail, nested) {
            crate::util::ensure_stack(|| find_split_part_boxes(nodes, root_id, nested, true, layout_children));
        }
        for child_id in part {
            crate::util::ensure_stack(|| find_inline_layout_embedded_boxes_recursive(nodes, root_id, child_id, layout_children));
        }
        if let (false, Some(nested)) = (tail, nested) {
            crate::util::ensure_stack(|| find_split_part_boxes(nodes, root_id, nested, false, layout_children));
        }
    }

    fn flush_inline_pseudos_recursive(doc: &mut BaseDocument, node_id: NodeId) {
        doc.iter_children_mut(node_id, |child_id, doc| {
            flush_pseudo_elements(doc, child_id);
            // pyro: the child's display decides (it read its parent's, so the pseudo-elements
            // of an inline inside an inline were never made).
            let display = doc.nodes[child_id]
                .display_style()
                .unwrap_or(Display::inline());
            let do_recurse = match (display.outside(), display.inside()) {
                (DisplayOutside::None, DisplayInside::Contents) => true,
                (DisplayOutside::Inline, DisplayInside::Flow) => true,
                (_, _) => false,
            };
            if do_recurse {
                crate::util::ensure_stack(|| flush_inline_pseudos_recursive(doc, child_id));
            }
        });
    }

    fn find_inline_layout_embedded_boxes_recursive(
        nodes: &mut crate::NodeTree,
        parent_id: NodeId,
        node_id: NodeId,
        layout_children: &mut ThinVec<NodeId>,
    ) {
        let node = &mut nodes[node_id];

        // Set layout_parent for node.
        node.layout_parent.set(Some(parent_id));

        match &node.data {
            NodeData::Element(element_data) | NodeData::AnonymousBlock(element_data) => {
                // if the input type is hidden, hide it
                if *element_data.name.local == *"input" {
                    if let Some("hidden") = element_data.attr(local_name!("type")) {
                        return;
                    }
                }

                let display = node.display_style().unwrap_or(Display::inline());

                if node.is_line_break() {
                    node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    return;
                }

                match (display.outside(), display.inside()) {
                    (DisplayOutside::None, DisplayInside::None) => {
                        node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    }
                    (DisplayOutside::None, DisplayInside::Contents) => {
                        node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                        iter_children!(nodes[node_id], |child_id| {
                            crate::util::ensure_stack(|| find_inline_layout_embedded_boxes_recursive(
                                nodes,
                                parent_id,
                                child_id,
                                layout_children,
                            ));
                        });
                    }
                    (DisplayOutside::Inline, DisplayInside::Flow) => {
                        if is_inline_box(element_data) {
                            layout_children.push(node_id);
                        } else {
                            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                            iter_children_and_pseudos!(nodes[node_id], |child_id| {
                                crate::util::ensure_stack(|| find_inline_layout_embedded_boxes_recursive(
                                    nodes,
                                    node_id,
                                    child_id,
                                    layout_children,
                                ));
                            });
                            // pyro: white space between its table parts goes (`table_fixup`).
                            if nodes[node_id].flags.contains(NodeFlags::ENCLOSES_TABLE_PARTS) {
                                nodes[node_id].flags.remove(NodeFlags::ENCLOSES_TABLE_PARTS);
                                super::table_fixup::mark_space_between_inline_table_parts(nodes, node_id);
                            }
                        }
                    }
                    // Inline box
                    (_, _) => {
                        layout_children.push(node_id);
                        // pyro: a table part in an inline box (`table_fixup`).
                        if display.outside() == DisplayOutside::InternalTable {
                            if let Some(parent) = nodes[node_id].parent {
                                nodes[parent].flags.insert(NodeFlags::ENCLOSES_TABLE_PARTS);
                            }
                        }
                    }
                };
            }
            NodeData::Comment { .. } | NodeData::Text(_) => {
                node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                node.flags.remove(NodeFlags::IS_SPACE_BETWEEN_TABLE_PARTS);
            }
            NodeData::Document(_) => unreachable!(),
        }
    }
}

/// Debug builds: an inline root that kept its inline layout (no `INLINE_CONTENT` on it or below
/// it) would build the same text, from the same text nodes, with the same inline boxes.
#[cfg(debug_assertions)]
fn assert_kept_inline_layout_is_current(doc: &mut BaseDocument, node_id: NodeId, kept: &TextLayout) {
    let mut fresh = TextLayout { first_letter: kept.first_letter.clone(), ..TextLayout::new() };
    let BaseDocument { nodes, layout_ctx, font_ctx, viewport, .. } = doc;
    let mut font_ctx = font_ctx.lock().unwrap();
    super::inline::build_inline_layout_into(nodes, layout_ctx, &mut font_ctx, &mut fresh, viewport.scale(), node_id);
    assert_eq!(fresh.text, kept.text, "inline root {node_id:?} kept a stale inline layout (text)");
    assert_eq!(fresh.text_nodes, kept.text_nodes, "inline root {node_id:?} kept a stale inline layout (text nodes)");
    assert_eq!(
        fresh.layout.inline_boxes().len(),
        kept.layout.inline_boxes().len(),
        "inline root {node_id:?} kept a stale inline layout (inline boxes)"
    );
    // The content widths kept with the shaped text (`TextLayout::text_content_widths`) go with
    // the kept layout: they must be what a build of the same content measures now.
    let widths = |t: &TextLayout| t.text_content_widths.map(|w| (w.min.to_bits(), w.max.to_bits()));
    assert_eq!(
        widths(&fresh),
        widths(kept),
        "inline root {node_id:?} kept stale content widths with its inline layout"
    );
}
