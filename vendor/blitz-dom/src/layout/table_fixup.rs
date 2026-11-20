//! pyro: the anonymous table objects of CSS 2.1 §17.2.1 (css-tables-3 §3.3, "fixup").
//!
//! - Consecutive table parts whose parent box is not a table (cells, rows, row groups, columns,
//!   column groups and captions in a block container) are wrapped in an anonymous table
//!   ([`wrap_misparented_table_parts`]).
//! - The children of a table, row group or row that are not its proper children are wrapped in an
//!   anonymous row (in a table or row group) or cell (in a row) ([`fixup_children`]).
//!
//! The anonymous boxes are nodes of the slab (`NodeData::AnonymousBlock`) styled as the UA sheet's
//! `::-servo-anonymous-table`, `::-servo-anonymous-table-row` and `::-servo-anonymous-table-cell`
//! boxes, with the box they are in as their parent style. Each is owned (`Node::anonymous_blocks`) by
//! the box whose children it wraps, which frees it when it is built again; its `children` are the
//! DOM nodes it wraps, whose `parent` stays their DOM parent (as with anonymous blocks).

use blitz_traits::node_id::NodeId;
use markup5ever::{local_name, ns, QualName};
use style::{
    data::ElementData as StyloElementData,
    properties::ComputedValues,
    selector_parser::PseudoElement,
    shared_lock::StylesheetGuards,
    values::specified::box_::{DisplayInside, DisplayOutside},
};
use thin_vec::ThinVec;

use stylo_taffy::CopyStyleGetters;

use crate::{node::NodeFlags, node::NodeKind, BaseDocument, ElementData, Node, NodeData};

use super::damage::ALL_DAMAGE;

/// A tabular container whose children are fixed up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Level {
    Table,
    RowGroup,
    Row,
}

/// What a child is to the table fixup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// Generates no box (a comment, `display: none`).
    Hidden,
    /// White space that collapses away: dropped between table parts.
    Space,
    /// Absolutely positioned: laid out by the table (its `oof_children`), never wrapped.
    OutOfFlow,
    Caption,
    /// A column or a column group.
    Column,
    RowGroup,
    Row,
    Cell,
    /// A `display: contents` element: its children are in its place (`box_children`).
    Contents,
    /// Anything else: text, inline and block boxes, tables.
    Other,
}

impl Kind {
    /// A table-non-root box (css-tables-3): a proper table child or a cell.
    fn is_table_non_root(self) -> bool {
        matches!(self, Kind::Caption | Kind::Column | Kind::RowGroup | Kind::Row | Kind::Cell)
    }

    fn is_proper_child_of(self, level: Level) -> bool {
        match level {
            Level::Table => matches!(self, Kind::Caption | Kind::Column | Kind::RowGroup | Kind::Row),
            Level::RowGroup => self == Kind::Row,
            Level::Row => self == Kind::Cell,
        }
    }
}

/// What the node is to the table fixup.
pub(crate) fn kind(doc: &BaseDocument, id: NodeId) -> Kind {
    let node = &doc.nodes[id];
    match node.data.kind() {
        NodeKind::Comment => return Kind::Hidden,
        // White space between table parts goes whatever `white-space` says (CSS 2.1 §17.2.1:
        // "anonymous inline boxes that contain only white space"), as in Chromium.
        NodeKind::Text => {
            return if node.is_whitespace_node() { Kind::Space } else { Kind::Other };
        }
        NodeKind::Element | NodeKind::AnonymousBlock => {}
        _ => return Kind::Hidden,
    }
    if node.is_line_break() {
        return Kind::Other;
    }
    let Some(styles) = node.primary_styles() else { return Kind::Hidden };
    let display = styles.clone_display();
    if display.outside() == DisplayOutside::None {
        return if display.inside() == DisplayInside::Contents { Kind::Contents } else { Kind::Hidden };
    }
    if styles.clone_position().is_absolutely_positioned() {
        return Kind::OutOfFlow;
    }
    drop(styles);
    let kind = match display.inside() {
        DisplayInside::TableRowGroup | DisplayInside::TableHeaderGroup | DisplayInside::TableFooterGroup => {
            Kind::RowGroup
        }
        DisplayInside::TableRow => Kind::Row,
        DisplayInside::TableCell => Kind::Cell,
        DisplayInside::TableColumn | DisplayInside::TableColumnGroup => Kind::Column,
        _ if display.outside() == DisplayOutside::TableCaption => Kind::Caption,
        _ => return Kind::Other,
    };
    // A replaced element with a table display is inline content (CSS Display 3 §2.7).
    if is_replaced(node) {
        Kind::Other
    } else {
        kind
    }
}

/// Whether the node is a replaced element (`img`, `canvas`, ...).
fn is_replaced(node: &Node) -> bool {
    node.data.downcast_element().is_some_and(super::replaced::is_replaced)
}

/// Whether the node is an element (or pseudo-element) with `display: contents`.
fn is_contents(node: &Node) -> bool {
    !node.is_text_node() && node.display_style().is_some_and(|display| display.inside() == DisplayInside::Contents)
}

/// The children of `parent` that generate boxes in its box, in order: its `::before`, its children
/// and its `::after`, with each `display: contents` element replaced by its own.
fn box_children(doc: &mut BaseDocument, parent: NodeId, out: &mut Vec<NodeId>) {
    // A table part's pseudo-elements are made here: no other construction reaches its box (one that
    // has none and is styled to have none has nothing to make).
    if has_pseudo_boxes(&doc.nodes[parent]) {
        super::construct::flush_pseudo_elements(doc, parent);
    }
    let node = &doc.nodes[parent];
    let (before, after) = (node.before(), node.after());
    let ids: Vec<NodeId> = before.into_iter().chain(node.children.iter().copied()).chain(after).collect();
    for id in ids {
        if is_contents(&doc.nodes[id]) {
            crate::util::ensure_stack(|| box_children(doc, id, out));
        } else {
            out.push(id);
        }
    }
}

/// Whether `parent`'s children are not all what its box lays out as they are: it has pseudo-element
/// boxes, or a child that is not a proper child of `level` (nor hidden, white space or out of flow).
fn needs_fixup(doc: &BaseDocument, parent: NodeId, level: Level) -> bool {
    let node = &doc.nodes[parent];
    has_pseudo_boxes(node)
        || node.children.iter().any(|&id| match kind(doc, id) {
            Kind::Hidden | Kind::Space | Kind::OutOfFlow => false,
            kind => !kind.is_proper_child_of(level),
        })
}

/// The children `parent` (a table, row group or row) lays out, with those that are not its proper
/// children wrapped in anonymous rows (a table, a row group) or cells (a row), which `parent` owns
/// (they are added to `anonymous`). White space between table parts, and before or after one at the
/// start or end, is dropped; out-of-flow boxes are kept as they are.
///
/// `None` in the common case, when `parent` lays out its own children (every one a proper child or
/// generating no box, which the table build passes over).
///
/// (`_vertical`: the table's writing mode is vertical; the fixup is the same. Its bare
/// `text-combine-upright` text is boxed by `layout::combined_text`.)
pub(crate) fn fixup_children(
    doc: &mut BaseDocument,
    parent: NodeId,
    level: Level,
    _vertical: bool,
    anonymous: &mut Vec<NodeId>,
) -> Option<Vec<NodeId>> {
    if !needs_fixup(doc, parent, level) {
        return None;
    }
    let mut items = Vec::new();
    box_children(doc, parent, &mut items);
    let kinds: Vec<Kind> = items.iter().map(|&id| kind(doc, id)).collect();
    // The kind of the nearest box-generating sibling (not hidden, not white space) on each side.
    let neighbour = |from: usize, step: isize| -> Option<Kind> {
        let mut at = from as isize + step;
        while at >= 0 && (at as usize) < kinds.len() {
            match kinds[at as usize] {
                Kind::Hidden | Kind::Space => at += step,
                kind => return Some(kind),
            }
        }
        None
    };
    let mut out = Vec::with_capacity(items.len());
    let mut run: Vec<NodeId> = Vec::new();
    let flush = |doc: &mut BaseDocument, run: &mut Vec<NodeId>, out: &mut Vec<NodeId>, anonymous: &mut Vec<NodeId>| {
        if run.is_empty() {
            return;
        }
        let pseudo = match level {
            Level::Table | Level::RowGroup => PseudoElement::ServoAnonymousTableRow,
            Level::Row => PseudoElement::ServoAnonymousTableCell,
        };
        let id = create_anonymous_table_box(doc, parent, &pseudo, std::mem::take(run));
        anonymous.push(id);
        out.push(id);
    };
    for (at, (&id, &kind)) in items.iter().zip(&kinds).enumerate() {
        match kind {
            Kind::Hidden => {}
            Kind::Space => {
                let before = neighbour(at, -1);
                let after = neighbour(at, 1);
                let dropped = before.is_none_or(Kind::is_table_non_root) && after.is_none_or(Kind::is_table_non_root);
                if !dropped {
                    run.push(id);
                }
            }
            Kind::OutOfFlow => {
                flush(doc, &mut run, &mut out, anonymous);
                out.push(id);
            }
            kind if kind.is_proper_child_of(level) => {
                flush(doc, &mut run, &mut out, anonymous);
                out.push(id);
            }
            _ => run.push(id),
        }
    }
    flush(doc, &mut run, &mut out, anonymous);
    Some(out)
}

/// Whether the box is a table part that is misparented outside a table (a cell, row, row group,
/// column, column group or caption), as a layout child of a block container.
fn is_table_part(node: &Node) -> bool {
    !node.is_text_node()
        && !is_replaced(node)
        && node.display_style().is_some_and(|display| {
            matches!(display.outside(), DisplayOutside::InternalTable | DisplayOutside::TableCaption)
        })
}

/// Wraps each run of consecutive table parts among a block container's layout children in an
/// anonymous table, which the container owns. `display: none` boxes between two parts of a run
/// stay in it.
pub(crate) fn wrap_misparented_table_parts(
    doc: &mut BaseDocument,
    container: NodeId,
    children: &mut ThinVec<NodeId>,
    anonymous: &mut ThinVec<NodeId>,
) {
    if !children.iter().any(|&id| doc.nodes.get(id).is_some_and(is_table_part)) {
        return;
    }
    if doc.nodes[container].primary_styles().is_none() {
        return;
    }
    let mut out = ThinVec::with_capacity(children.len());
    let mut run: Vec<NodeId> = Vec::new();
    let mut hidden: Vec<NodeId> = Vec::new();
    let flush =
        |doc: &mut BaseDocument, run: &mut Vec<NodeId>, out: &mut ThinVec<NodeId>, anonymous: &mut ThinVec<NodeId>| {
            if run.is_empty() {
                return;
            }
            let id =
                create_anonymous_table_box(doc, container, &PseudoElement::ServoAnonymousTable, std::mem::take(run));
            anonymous.push(id);
            out.push(id);
        };
    for &id in children.iter() {
        let node = &doc.nodes[id];
        if is_table_part(node) {
            run.append(&mut hidden);
            run.push(id);
        } else if !run.is_empty() && node.display_style().is_some_and(|d| d.inside() == DisplayInside::None) {
            hidden.push(id);
        } else {
            flush(doc, &mut run, &mut out, anonymous);
            out.append(&mut ThinVec::from(std::mem::take(&mut hidden)));
            out.push(id);
        }
    }
    flush(doc, &mut run, &mut out, anonymous);
    out.extend(hidden);
    *children = out;
}

/// pyro: whether `node` is an anonymous inline table made by [`wrap_inline_table_parts`].
pub(crate) fn is_anonymous_inline_table(node: &Node) -> bool {
    node.is_anonymous()
        && node.display_style().is_some_and(|display| {
            display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Table
        })
}

/// pyro: the anonymous inline table a table part of an inline box is in
/// ([`wrap_inline_table_parts`]), and whether the part is its first.
pub(crate) fn inline_table_of(nodes: &crate::NodeTree, part: NodeId) -> Option<(NodeId, bool)> {
    // (Constant time: the marker is set for the parts of the tables made now; a table made before is
    // freed when its inline root is built again (its id no longer resolves), and a part moved since
    // has another parent than the table, which is in the part's parent inline box.)
    let table = nodes[part].inline_table.get()?;
    let table_node = nodes.get(table)?;
    (is_anonymous_inline_table(table_node) && table_node.parent.is_some() && table_node.parent == nodes[part].parent)
        .then(|| (table, table_node.children.first() == Some(&part)))
}

/// pyro: wraps each run of table parts among the children of an inline box in the inline content
/// of the inline root `root` in an anonymous inline table (CSS 2.1 § 17.2.1: "if a child C of a
/// table-row-group (etc.) parent ... the parent is an inline box, an anonymous inline-table box
/// is generated"; WPT css-tables `fixup-dynamic-anonymous-inline-table-001/002`). White space,
/// comments and `display: none` boxes between two parts stay in the run (the white space
/// generates no box: [`mark_space_between_inline_table_parts`]). The tables are owned by `root`
/// (`anonymous`). Whether it made any.
pub(crate) fn wrap_inline_table_parts(doc: &mut BaseDocument, root: NodeId, anonymous: &mut ThinVec<NodeId>) -> bool {
    fn visit(doc: &mut BaseDocument, parent: NodeId, anonymous: &mut ThinVec<NodeId>, made: &mut bool) {
        let children = doc.nodes[parent].children.clone();
        for child in children {
            let node = &doc.nodes[child];
            let Some(element) = node.element_data() else { continue };
            let Some(display) = node.display_style() else { continue };
            if display.outside() == DisplayOutside::Inline
                && display.inside() == DisplayInside::Flow
                && !super::replaced::is_inline_box(element)
            {
                wrap_runs(doc, child, anonymous, made);
                crate::util::ensure_stack(|| visit(doc, child, anonymous, made));
            } else if display.outside() == DisplayOutside::None && display.inside() == DisplayInside::Contents {
                crate::util::ensure_stack(|| visit(doc, child, anonymous, made));
            }
        }
    }
    fn wrap_runs(doc: &mut BaseDocument, inline: NodeId, anonymous: &mut ThinVec<NodeId>, made: &mut bool) {
        if !doc.nodes[inline].children.iter().any(|&id| doc.nodes.get(id).is_some_and(is_table_part)) {
            return;
        }
        let children = doc.nodes[inline].children.clone();
        let mut run: Vec<NodeId> = Vec::new();
        let mut flush = |doc: &mut BaseDocument, run: &mut Vec<NodeId>| {
            if run.is_empty() {
                return;
            }
            let parts = std::mem::take(run);
            let id = create_anonymous_table_box(doc, inline, &PseudoElement::ServoAnonymousTable, parts.clone());
            // An inline-level table.
            if let Some(style) = doc.nodes[id].primary_styles().map(|style| style.clone()) {
                let mut builder = style::properties::StyleBuilder::for_derived_style(
                    doc.stylist.device(),
                    Some(&doc.stylist),
                    &style,
                    None,
                );
                builder.set_display(style::values::specified::box_::Display::InlineTable);
                let inline_table = builder.build();
                if let Some(mut data) = doc.nodes[id].try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                    data.styles.primary = Some(inline_table);
                }
                doc.nodes[id].update_layout_styles();
            }
            for part in parts {
                doc.nodes[part].inline_table.set(Some(id));
            }
            anonymous.push(id);
            *made = true;
        };
        for child in children {
            let node = &doc.nodes[child];
            if is_table_part(node) {
                run.push(child);
            } else if node.is_whitespace_node()
                || node.data.kind() == NodeKind::Comment
                || node.display_style().is_some_and(|display| {
                    display.outside() == DisplayOutside::None && display.inside() == DisplayInside::None
                })
            {
                // (In the run, generating no box.)
            } else {
                flush(doc, &mut run);
            }
        }
        flush(doc, &mut run);
    }
    let mut made = false;
    visit(doc, root, anonymous, &mut made);
    made
}

/// White space between two table parts of an inline box generates no box, as if the parts were in
/// one anonymous inline table (CSS 2.1 §17.2.1 removes white space between two internal table
/// boxes; an inline box lays out its table parts as atomic inlines). Called once the construction of
/// an inline formatting context has visited the children of `inline` (an inline box with table
/// parts among them, `NodeFlags::ENCLOSES_TABLE_PARTS`), which cleared their flags: flags the
/// white-space text children between two of its table parts
/// (`NodeFlags::IS_SPACE_BETWEEN_TABLE_PARTS`). Boxes that generate nothing (comments,
/// `display: none`) are passed over; anything else between two parts keeps the white space.
pub(crate) fn mark_space_between_inline_table_parts(nodes: &mut crate::NodeTree, inline: NodeId) {
    let mut spaces: Vec<NodeId> = Vec::new();
    let mut between: Vec<NodeId> = Vec::new();
    // Whether the last box-generating child was a table part.
    let mut after_part = false;
    for &id in nodes[inline].children.iter() {
        let node = &nodes[id];
        if node.is_text_node() {
            if after_part && node.is_whitespace_node() {
                between.push(id);
            } else {
                after_part = false;
                between.clear();
            }
        } else if node.data.kind() == NodeKind::Comment
            || node.display_style().is_some_and(|display| {
                display.outside() == DisplayOutside::None && display.inside() == DisplayInside::None
            })
        {
            continue;
        } else if is_table_part(node) {
            spaces.append(&mut between);
            after_part = true;
        } else {
            after_part = false;
            between.clear();
        }
    }
    for id in spaces {
        nodes[id].flags.insert(NodeFlags::IS_SPACE_BETWEEN_TABLE_PARTS);
    }
}

/// The pseudo-element an anonymous box is styled as, by the `display` it was given: a table, row
/// or cell of the fixup, else an anonymous block.
pub(crate) fn anonymous_box_pseudo(style: &ComputedValues) -> PseudoElement {
    match style.clone_display().inside() {
        DisplayInside::Table => PseudoElement::ServoAnonymousTable,
        DisplayInside::TableRow => PseudoElement::ServoAnonymousTableRow,
        DisplayInside::TableCell => PseudoElement::ServoAnonymousTableCell,
        _ => PseudoElement::ServoAnonymousBox,
    }
}

/// A new anonymous table box (`pseudo`) in `parent`'s box, wrapping `children`.
fn create_anonymous_table_box(
    doc: &mut BaseDocument,
    parent: NodeId,
    pseudo: &PseudoElement,
    children: Vec<NodeId>,
) -> NodeId {
    const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("div") };
    let node_id = doc.create_node(NodeData::AnonymousBlock(Box::new(ElementData::new(NAME, Vec::new()))));
    let style = {
        let parent_style = doc.nodes[parent].primary_styles().expect("a table box's parent is styled");
        let read_guard = doc.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        doc.stylist.style_for_anonymous::<&Node>(&guards, pseudo, &parent_style)
    };
    let mut data = StyloElementData { damage: ALL_DAMAGE, ..Default::default() };
    data.styles.primary = Some(style);
    data.set_restyled();
    *doc.nodes[node_id].stylo_element_data_mut().ensure_init_mut() = data;
    doc.nodes[node_id].update_layout_styles();
    doc.nodes[node_id].mark_damaged();
    if doc.nodes[parent].flags.contains(NodeFlags::IS_IN_DOCUMENT) {
        doc.nodes[node_id].flags.insert(NodeFlags::IS_IN_DOCUMENT);
    }
    doc.nodes[node_id].parent = Some(parent);
    doc.nodes[node_id].layout_parent.set(Some(parent));
    doc.nodes[node_id].children = children.into();
    node_id
}

/// Whether the node has, or is styled to have, a `::before` or `::after` that generates a box:
/// part of its box's children (`box_children`).
pub(crate) fn has_pseudo_boxes(node: &Node) -> bool {
    use style::values::computed::Content;
    if node.before().is_some() || node.after().is_some() {
        return true;
    }
    node.try_stylo_element_data().and_then(|data| data.get()).is_some_and(|data| {
        data.styles.pseudos.as_array()[..2]
            .iter()
            .flatten()
            .any(|style| !matches!(style.get_counters().content, Content::Normal | Content::None))
    })
}

/// Whether the node is an anonymous table, row or cell made by the fixup.
pub(crate) fn is_anonymous_table_box(node: &Node) -> bool {
    node.is_anonymous()
        && node.display_style().is_some_and(|display| {
            matches!(display.inside(), DisplayInside::Table | DisplayInside::TableRow | DisplayInside::TableCell)
        })
}
