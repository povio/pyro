//! pyro: CSS Overflow 5's scroll markers and scroll buttons: the boxes of `::scroll-marker-group`,
//! `::scroll-marker` and `::scroll-button()`, and `:target-current`.
//!
//! The boxes are nodes outside the DOM, as `::before` boxes are, styled in normal matching mode
//! against the node itself (`Stylist::compute_pseudo_node_style`), so that state pseudo-classes
//! after the pseudo-element match the node's own state. Each node's `ScrollPseudos::pseudo` names
//! the pseudo-element and its originating element (selector matching and `attr()` go there).
//!
//! - A scroll container whose `scroll-marker-group` names a side has a group node
//!   (`ScrollPseudos::group`), a box next to its own: right before it or right after it in its
//!   parent's box list (`with_box_siblings`, used by box construction), or, for the root element,
//!   its first or last box (CSS Overflow 5 § 3.1.3).
//! - The group's children are the marker nodes of the elements whose nearest scroll container
//!   (through their containing blocks) it is and whose `::scroll-marker` has `content`, in tree
//!   order (§ 3.1.5).
//! - An element whose `::scroll-button()` has `content` has a button node per direction, boxes
//!   right after its own (before an `after` group), in the order block-start, inline-start,
//!   inline-end, block-end, as Chromium places them (WPT `scroll-buttons-layout-parent`). A
//!   button is a `button` element's box (the UA rules for `button` apply to it).
//!
//! `BaseDocument::resolve_scroll_markers` brings the nodes up to date after a style traversal
//! (the whole document, only on pages that use the feature), before counters are evaluated.
//! After layout, `BaseDocument::update_target_current` picks each group's active marker.

use blitz_traits::node_id::NodeId;
use markup5ever::{local_name, ns, QualName};
use smallvec::SmallVec;
use style::properties::ComputedValues;
use style::selector_parser::PseudoElement;
use style::servo_arc::Arc;
use style::shared_lock::StylesheetGuards;
use style::stylesheets::Origin;
use style::values::computed::Overflow;
use style::values::generics::counters::Content;

use super::damage::ALL_DAMAGE;
use crate::node::NodeFlags;
use style::data::ElementData as StyloElementData;
use stylo_taffy::CopyStyleGetters;
use crate::tree::NodeTree;
use crate::{BaseDocument, ElementData, Node, NodeData};

/// pyro: the scroll pseudo-elements of an element (its group, buttons and marker), or, for the
/// node of a pseudo-element's box, which pseudo-element it is.
#[derive(Debug, Clone, Default)]
pub struct ScrollPseudos {
    /// Of a scroll container: its `::scroll-marker-group` box.
    pub group: Option<NodeId>,
    /// Whether the group's box goes before the scroll container's box.
    pub group_before: bool,
    /// Its `::scroll-button()` boxes: block-start, inline-start, inline-end, block-end.
    pub buttons: [Option<NodeId>; 4],
    /// Its `::scroll-marker` box (a child of a group's box).
    pub marker: Option<NodeId>,
    /// Of a pseudo-element's box: the pseudo-element (`::scroll-button(*)` for every button) and
    /// its originating element.
    pub pseudo: Option<(PseudoElement, NodeId)>,
    /// Of a button's box: its direction (0 block-start, 1 inline-start, 2 inline-end, 3 block-end).
    pub direction: u8,
    /// Of a multi-column scroll container: its `::column` node (styled by no rule of its own, it
    /// stands for its columns in selector matching: `::column::scroll-marker`).
    pub column: Option<NodeId>,
    /// Of a multi-column scroll container with column markers: how many it has (one per column
    /// of its last layout, `update_target_current` checks).
    pub column_markers: Option<u32>,
    /// Of a column's marker: the column's index.
    pub index: u32,
}

impl ScrollPseudos {
    fn is_empty(&self) -> bool {
        self.group.is_none()
            && self.buttons.iter().all(Option::is_none)
            && self.marker.is_none()
            && self.pseudo.is_none()
            && self.column.is_none()
    }
}

impl Node {
    /// pyro: the element's scroll pseudo-elements, or what this pseudo-element box node is.
    pub(crate) fn scroll_pseudos(&self) -> Option<&ScrollPseudos> {
        self.element_data()?.scroll.as_deref()
    }

    /// pyro: the scroll pseudo-element this node is the box of, and its originating element.
    pub(crate) fn scroll_pseudo(&self) -> Option<(PseudoElement, NodeId)> {
        match self.data {
            NodeData::AnonymousBlock(ref data) => data.scroll.as_deref()?.pseudo.clone(),
            _ => None,
        }
    }
}

/// The boxes generated next to `node`'s in its parent's box list: its group before it, and after
/// it its buttons and its group after it. The root element's are its own first and last boxes
/// instead (`own_boxes`).
fn box_siblings(node: &Node) -> Option<(Option<NodeId>, SmallVec<[NodeId; 5]>)> {
    if node.is_root_element() {
        return None;
    }
    pseudo_boxes(node)
}

/// The boxes beside `node` (`box_siblings`) in `container`'s box list: none when `container` is
/// an anonymous block wrapping a run of its parent's boxes (`NodeFlags::WRAPS_BOX_RUN`). The walk
/// over the parent placed them already, in that block or beside it: placing them again would put
/// them twice, and a block-level group beside an inline-level scroller would be wrapped again
/// without end (the block would hold block-level content beside inline content once more).
fn box_siblings_in(nodes: &NodeTree, container: NodeId, node: &Node) -> Option<(Option<NodeId>, SmallVec<[NodeId; 5]>)> {
    if nodes.get(container).is_some_and(|container| container.flags.contains(NodeFlags::WRAPS_BOX_RUN)) {
        return None;
    }
    box_siblings(node)
}

/// pyro: the scroll pseudo-element boxes generated before and after `node`'s own box in the box
/// list of `container` (`box_siblings_in`): the inline formatting context builders take them there.
pub(crate) fn siblings_of(nodes: &NodeTree, container: NodeId, node: &Node) -> Option<(Option<NodeId>, SmallVec<[NodeId; 5]>)> {
    box_siblings_in(nodes, container, node)
}

/// pyro: `child` with the boxes beside it in `container`'s box list, in order (`box_siblings_in`).
pub(crate) fn with_own_siblings(nodes: &NodeTree, container: NodeId, child: NodeId) -> SmallVec<[NodeId; 7]> {
    let mut out = SmallVec::new();
    match nodes.get(child).and_then(|node| box_siblings_in(nodes, container, node)) {
        Some((before, after)) => {
            out.extend(before);
            out.push(child);
            out.extend(after);
        }
        None => out.push(child),
    }
    out
}

/// The root element's group and buttons, its first and last boxes (CSS Overflow 5 § 3.1.3).
fn own_boxes(node: &Node) -> Option<(Option<NodeId>, SmallVec<[NodeId; 5]>)> {
    if !node.is_root_element() {
        return None;
    }
    pseudo_boxes(node)
}

fn pseudo_boxes(node: &Node) -> Option<(Option<NodeId>, SmallVec<[NodeId; 5]>)> {
    let pseudos = node.scroll_pseudos()?;
    if pseudos.group.is_none() && pseudos.buttons.iter().all(Option::is_none) {
        return None;
    }
    let before = pseudos.group.filter(|_| pseudos.group_before);
    let mut after = SmallVec::new();
    after.extend(pseudos.buttons.iter().flatten().copied());
    if !pseudos.group_before {
        after.extend(pseudos.group);
    }
    Some((before, after))
}

/// pyro: the children `ids` of `container` with the scroll pseudo-element boxes generated next to
/// them (`box_siblings`) around each, and, when `container` is the root element, its own group
/// and buttons as its first and last boxes. Box construction iterates its children through this.
pub(crate) fn with_box_siblings<'a>(
    nodes: &'a NodeTree,
    container: NodeId,
    ids: impl IntoIterator<Item = NodeId> + 'a,
) -> impl Iterator<Item = NodeId> + 'a {
    let (first, last) = match nodes.get(container).and_then(own_boxes) {
        Some((before, after)) => (before, after),
        None => (None, SmallVec::new()),
    };
    first
        .into_iter()
        .chain(ids.into_iter().flat_map(move |id| {
            let mut out: SmallVec<[NodeId; 7]> = SmallVec::new();
            match nodes.get(id).and_then(|node| box_siblings_in(nodes, container, node)) {
                Some((before, after)) => {
                    out.extend(before);
                    out.push(id);
                    out.extend(after);
                }
                None => out.push(id),
            }
            out
        }))
        .chain(last)
}

/// `ids` (the children of `container`, and its `::before` and `::after`) with their indices, and
/// the scroll pseudo-element boxes around them (`with_box_siblings`) at index `usize::MAX`.
pub(crate) fn indexed_with_box_siblings(
    nodes: &NodeTree,
    container: NodeId,
    ids: impl IntoIterator<Item = NodeId>,
) -> Vec<(usize, NodeId)> {
    let mut out = Vec::new();
    let own = nodes.get(container).and_then(own_boxes);
    if let Some((Some(first), _)) = &own {
        out.push((usize::MAX, *first));
    }
    for (index, id) in ids.into_iter().enumerate() {
        match nodes.get(id).and_then(|node| box_siblings_in(nodes, container, node)) {
            Some((before, after)) => {
                out.extend(before.map(|id| (usize::MAX, id)));
                out.push((index, id));
                out.extend(after.iter().map(|&id| (usize::MAX, id)));
            }
            None => out.push((index, id)),
        }
    }
    if let Some((_, last)) = own {
        out.extend(last.iter().map(|&id| (usize::MAX, id)));
    }
    out
}

/// pyro: the node whose box holds `node_id`'s box, as far as its ancestors go: its parent, except
/// for a scroll marker group's or scroll button's box, which is beside its originating element's
/// box (inside the root element's): the element's parent.
pub(crate) fn box_parent(nodes: &NodeTree, node_id: NodeId) -> Option<NodeId> {
    let node = nodes.get(node_id)?;
    match node.scroll_pseudo() {
        Some((pseudo, origin))
            if (pseudo == PseudoElement::ScrollMarkerGroup || pseudo.is_scroll_button())
                && nodes.get(origin).is_some_and(|origin| !origin.is_root_element()) =>
        {
            nodes[origin].parent
        }
        _ => node.parent,
    }
}

/// Whether `node` has boxes generated next to its own (`box_siblings`).
pub(crate) fn has_box_siblings(node: &Node) -> bool {
    !node.is_root_element() && pseudo_boxes(node).is_some()
}

/// Whether any child of `container` has boxes next to its own, or `container` is a root element
/// with boxes of its own.
pub(crate) fn any_box_siblings(nodes: &NodeTree, container: NodeId) -> bool {
    let node = &nodes[container];
    own_boxes(node).is_some()
        || node.children.iter().any(|&child| {
            nodes.get(child).is_some_and(|child| !child.is_root_element() && box_siblings_in(nodes, container, child).is_some())
        })
}

/// pyro: the scroll pseudo-element boxes `node` owns (its group, with the markers in it, and its
/// buttons): the walks over the tree that visit `::before` and `::after` visit these too.
pub(crate) fn owned_pseudo_nodes(node: &Node) -> SmallVec<[NodeId; 5]> {
    let mut out = SmallVec::new();
    if let Some(pseudos) = node.scroll_pseudos() {
        out.extend(pseudos.group);
        out.extend(pseudos.buttons.iter().flatten().copied());
    }
    out
}

/// Whether a `::scroll-button()` selector argument `pe` selects the button of direction
/// `direction` of an element whose writing mode is `wm`.
pub(crate) fn scroll_button_matches(pe: &PseudoElement, direction: u8, wm: style::logical_geometry::WritingMode) -> bool {
    use style::logical_geometry::PhysicalSide;
    let physical = |side: PhysicalSide| {
        // The logical side of the element's box on that physical side.
        let block_start = wm.block_start_physical_side();
        let block_end = wm.block_end_physical_side();
        let inline_start = wm.inline_start_physical_side();
        let inline_end = wm.inline_end_physical_side();
        [block_start, inline_start, inline_end, block_end].iter().position(|&s| s == side) == Some(direction as usize)
    };
    match pe {
        PseudoElement::ScrollButtonAll => true,
        PseudoElement::ScrollButtonBlockStart => direction == 0,
        PseudoElement::ScrollButtonInlineStart => direction == 1,
        PseudoElement::ScrollButtonInlineEnd => direction == 2,
        PseudoElement::ScrollButtonBlockEnd => direction == 3,
        PseudoElement::ScrollButtonUp => physical(PhysicalSide::Top),
        PseudoElement::ScrollButtonDown => physical(PhysicalSide::Bottom),
        PseudoElement::ScrollButtonLeft => physical(PhysicalSide::Left),
        PseudoElement::ScrollButtonRight => physical(PhysicalSide::Right),
        _ => false,
    }
}

/// Whether the element with this style is a scroll container (CSS Overflow 3 § 3). `overflow`
/// does not apply to a non-replaced inline box (nor to a box `display: contents` or `none`): a
/// `<span style="overflow: auto">` has no scroll marker group and no scroll buttons (WPT
/// inline-with-scroll-marker-group-crash; a block-level group beside an inline is no box it has).
fn is_scroll_container(style: &ComputedValues) -> bool {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let scrolls = |overflow: Overflow| !matches!(overflow, Overflow::Visible | Overflow::Clip);
    let b = style.get_box();
    let display = b.display;
    if matches!(display.inside(), DisplayInside::Contents | DisplayInside::None)
        || (display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow)
    {
        return false;
    }
    scrolls(b.slow_clone_overflow_x()) || scrolls(b.slow_clone_overflow_y())
}

/// Whether an element with this style is the containing block of its absolutely positioned
/// descendants (`fixed`: of its fixed positioned ones), as far as scroll markers need to know.
fn contains_out_of_flow(style: &ComputedValues, fixed: bool) -> bool {
    use style::values::specified::box_::Contain;
    let b = style.get_box();
    let contain = b.used_containment();
    (!fixed && b.slow_clone_position() != style::computed_values::position::T::Static)
        || !b.transform.0.is_empty()
        || b.slow_clone_perspective() != style::values::computed::Perspective::None
        || contain.intersects(Contain::LAYOUT | Contain::PAINT)
        || !style.get_effects().filter.0.is_empty()
}

/// The scroll containers holding the children of an element, by how the child is positioned.
#[derive(Clone, Copy, Default)]
struct Containers {
    in_flow: Option<NodeId>,
    absolute: Option<NodeId>,
    fixed: Option<NodeId>,
}

/// What a pass found an element generates.
struct Found {
    /// The group of each scroll container that has one: the container, whether it goes before,
    /// and the elements whose markers it holds, in tree order, with their marker styles.
    groups: Vec<(NodeId, bool, Vec<(NodeId, Arc<ComputedValues>)>)>,
    /// The elements with buttons, with the style of each (by direction).
    buttons: Vec<(NodeId, [Option<Arc<ComputedValues>>; 4])>,
    /// Each element with `scroll-target-group: auto`, with the links in it (whose nearest such
    /// ancestor it is), in tree order.
    target_groups: Vec<(NodeId, Vec<NodeId>)>,
}

fn generates(style: &ComputedValues) -> bool {
    !matches!(style.get_counters().content, Content::Normal | Content::None)
        && style.clone_display() != style::values::computed::Display::None
}

impl BaseDocument {
    /// Whether any author or user rule styles a scroll pseudo-element.
    fn scroll_pseudo_rules(&self, pseudo: PseudoElement) -> bool {
        [Origin::Author, Origin::User]
            .into_iter()
            .any(|origin| self.stylist.cascade_data().borrow_for_origin(origin).normal_rules(&[pseudo.clone()]).is_some())
    }

    /// pyro: brings the scroll pseudo-element boxes up to date (see the module). Runs after a
    /// style traversal or a change to the tree (`scroll_markers_dirty`), on pages with a scroll
    /// container that has a group, with author rules for `::scroll-marker`, `::scroll-button()`
    /// or `::scroll-marker-group`, or with scroll pseudo-element boxes already.
    pub(crate) fn resolve_scroll_markers(&mut self) {
        if !self.scroll_markers_dirty {
            return;
        }
        self.scroll_markers_dirty = false;
        let marker_rules = self.scroll_pseudo_rules(PseudoElement::ScrollMarker);
        let button_rules = self.scroll_pseudo_rules(PseudoElement::ScrollButtonAll);
        if !self.scroll_marker_groups && !marker_rules && !button_rules && self.scroll_pseudo_owners.is_empty() {
            self.scroll_target_groups.clear();
            return;
        }
        let Some(root) = self.try_root_element().map(|node| node.id) else { return };
        let mut found = Found { groups: Vec::new(), buttons: Vec::new(), target_groups: Vec::new() };
        // The index of each grouped scroll container's entry in `found.groups`.
        let mut group_index: std::collections::HashMap<NodeId, usize> = Default::default();
        self.find_scroll_pseudos(root, None, Containers::default(), None, button_rules, &mut found, &mut group_index);
        self.scroll_target_groups = std::mem::take(&mut found.target_groups);
        self.apply_scroll_pseudos(found);
    }

    /// Walks the subtree of `node_id` in tree order: `container` is the scroll container holding
    /// it, `containers` those holding its children.
    #[allow(clippy::too_many_arguments)]
    fn find_scroll_pseudos(
        &self,
        node_id: NodeId,
        container: Option<NodeId>,
        containers: Containers,
        target_group: Option<usize>,
        button_rules: bool,
        found: &mut Found,
        group_index: &mut std::collections::HashMap<NodeId, usize>,
    ) {
        let node = &self.nodes[node_id];
        let Some(style) = node.primary_styles() else { return };
        let display = style.clone_display();
        if display == style::values::computed::Display::None {
            return;
        }
        let is_root = node.is_root_element();
        let has_box = !display.is_contents();
        // The root element is the viewport's scroll container; a `body` whose overflow the
        // viewport takes is none.
        let scrolls = has_box
            && (is_root || (is_scroll_container(&style) && !self.body_overflow_goes_to_viewport(node_id)));

        // Its marker, in the group of the scroll container holding it.
        if let Some(container) = container
            && let Some(&index) = group_index.get(&container)
            && let Some(marker_style) = self.probe_pseudo_style(node_id, &style, PseudoElement::ScrollMarker, None)
            && generates(&marker_style)
        {
            found.groups[index].2.push((node_id, marker_style));
        }

        // Its group.
        let marker_group = style.get_box().slow_clone_scroll_marker_group();
        if scrolls && marker_group.generates_group() {
            group_index.insert(node_id, found.groups.len());
            found.groups.push((node_id, marker_group.is_before(), Vec::new()));
        }

        // Its buttons (a `display: contents` element's too: they follow its contents).
        if button_rules {
            let wm = style.writing_mode;
            let mut styles: [Option<Arc<ComputedValues>>; 4] = Default::default();
            for (direction, slot) in styles.iter_mut().enumerate() {
                let matches = move |pe: &PseudoElement| scroll_button_matches(pe, direction as u8, wm);
                *slot = self
                    .probe_pseudo_style(node_id, &style, PseudoElement::ScrollButtonAll, Some(&matches))
                    .filter(|style| generates(style));
            }
            if styles.iter().any(Option::is_some) {
                found.buttons.push((node_id, styles));
            }
        }

        // A link of the scroll target group it is in, or a scroll target group.
        if let Some(group) = target_group
            && let Some(element) = node.element_data()
            && element.name.local == local_name!("a")
            && element.attr(local_name!("href")).is_some()
        {
            found.target_groups[group].1.push(node_id);
        }
        let target_group = match style.get_box().slow_clone_scroll_target_group() {
            style::values::computed::ScrollTargetGroup::Auto => {
                found.target_groups.push((node_id, Vec::new()));
                Some(found.target_groups.len() - 1)
            }
            style::values::computed::ScrollTargetGroup::None => target_group,
        };

        // The scroll containers holding its children.
        let inner = if scrolls { Some(node_id) } else { container };
        let children = Containers {
            in_flow: if has_box { inner } else { containers.in_flow },
            absolute: if has_box && (is_root || contains_out_of_flow(&style, false)) { inner } else { containers.absolute },
            fixed: if has_box && (is_root || contains_out_of_flow(&style, true)) { inner } else { containers.fixed },
        };
        drop(style);
        for &child_id in node.children.iter() {
            let child = &self.nodes[child_id];
            if !matches!(child.data, NodeData::Element(_)) {
                continue;
            }
            let child_container = match child.primary_styles().map(|style| style.get_box().slow_clone_position()) {
                Some(style::computed_values::position::T::Absolute) => children.absolute,
                Some(style::computed_values::position::T::Fixed) => children.fixed,
                _ => children.in_flow,
            };
            crate::util::ensure_stack(|| {
                self.find_scroll_pseudos(child_id, child_container, children, target_group, button_rules, found, group_index)
            });
        }
    }

    /// The style of a pseudo-element of `node_id` when a rule gives it one, matched on the
    /// element (state pseudo-classes after the pseudo-element do not match): whether it has a box.
    fn probe_pseudo_style(
        &self,
        node_id: NodeId,
        style: &ComputedValues,
        pseudo: PseudoElement,
        matching_fn: Option<&dyn Fn(&PseudoElement) -> bool>,
    ) -> Option<Arc<ComputedValues>> {
        use style::stylist::RuleInclusion;
        let read_guard = self.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        self.stylist.lazily_compute_pseudo_element_style::<&Node>(
            &guards,
            &self.nodes[node_id],
            &pseudo,
            RuleInclusion::All,
            style,
            true,
            matching_fn,
        )
    }

    /// The style of the pseudo-element box node `node_id` (`Stylist::compute_pseudo_node_style`).
    fn pseudo_node_style(&self, node_id: NodeId) -> Option<Arc<ComputedValues>> {
        let (_, mut origin) = self.nodes[node_id].scroll_pseudo()?;
        // (A column's marker inherits from the multi-column container, Multi-column 2 § 4.8.)
        while let Some((_, next)) = self.nodes[origin].scroll_pseudo() {
            origin = next;
        }
        let origin_style = self.nodes[origin].primary_styles()?;
        let read_guard = self.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        self.stylist.compute_pseudo_node_style::<&Node>(&guards, &self.nodes[node_id], &origin_style, false)
    }

    /// Creates the box node of `pseudo` of `origin`, a child of `parent`.
    fn create_scroll_pseudo_node(&mut self, pseudo: PseudoElement, origin: NodeId, parent: NodeId, direction: u8) -> NodeId {
        let name = if pseudo.is_scroll_button() {
            QualName::new(None, ns!(html), local_name!("button"))
        } else {
            QualName::new(None, ns!(html), local_name!("div"))
        };
        let mut data = ElementData::new(name, Vec::new());
        data.scroll = Some(Box::new(ScrollPseudos { pseudo: Some((pseudo, origin)), direction, ..Default::default() }));
        let id = self.create_node(NodeData::AnonymousBlock(Box::new(data)));
        let in_document = self.nodes[origin].flags.contains(NodeFlags::IS_IN_DOCUMENT);
        let node = &mut self.nodes[id];
        node.parent = Some(parent);
        if in_document {
            node.flags.insert(NodeFlags::IS_IN_DOCUMENT);
        }
        self.scroll_pseudo_owners.insert(origin);
        id
    }

    /// Gives the box node `node_id` the style `style` (when it changed), with the damage of a new box.
    /// A marker or button whose content reads counters has them evaluated (`layout::counters`).
    fn set_scroll_pseudo_style(&mut self, node_id: NodeId, style: Arc<ComputedValues>) -> bool {
        if super::counters::reads_counters(&style) {
            self.generated_counters = true;
            self.counters_dirty = true;
        }
        let old = self.nodes[node_id].primary_styles().map(|style| Arc::clone(&*style));
        if let Some(old) = &old
            && (Arc::ptr_eq(old, &style)
                || matches!(
                    style::selector_parser::RestyleDamage::compute_style_difference::<&Node>(old, &style).change,
                    style::matching::StyleChange::Unchanged
                ))
        {
            return false;
        }
        let node = &mut self.nodes[node_id];
        node.record_out_of_flow(&style);
        let mut data = StyloElementData::default();
        data.styles.primary = Some(style);
        data.set_restyled();
        data.damage = ALL_DAMAGE;
        *node.stylo_element_data_mut().ensure_init_mut() = data;
        node.update_layout_styles();
        node.mark_damaged();
        self.pending_style_image_nodes.push(node_id);
        true
    }

    /// Damages `node_id` so that it builds its box again (and its parent box, which lists it).
    fn damage_for_construction(&mut self, node_id: NodeId) {
        if let Some(node) = self.nodes.get_mut(node_id) {
            node.insert_damage(ALL_DAMAGE);
            node.mark_damaged();
        }
    }

    /// The node whose box list holds `node_id`'s box: its layout parent, else its DOM parent.
    fn box_list_owner(&self, node_id: NodeId) -> Option<NodeId> {
        let node = &self.nodes[node_id];
        if node.is_root_element() {
            return Some(node_id);
        }
        node.layout_parent.get().filter(|&id| self.nodes.contains_key(id)).or(node.parent)
    }

    fn apply_scroll_pseudos(&mut self, found: Found) {
        let mut owners = std::collections::HashSet::new();
        // Groups and their markers.
        let mut grouped = std::collections::HashSet::new();
        let mut marked = std::collections::HashSet::new();
        for (container, before, markers) in found.groups {
            grouped.insert(container);
            owners.insert(container);
            let existing = self.nodes[container].scroll_pseudos().and_then(|pseudos| pseudos.group);
            let was_before = self.nodes[container].scroll_pseudos().is_some_and(|pseudos| pseudos.group_before);
            let group = match existing {
                Some(group) => group,
                None => self.create_scroll_pseudo_node(PseudoElement::ScrollMarkerGroup, container, container, 0),
            };
            let pseudos = self.nodes[container].element_data_mut().unwrap().scroll.get_or_insert_with(Default::default);
            pseudos.group = Some(group);
            pseudos.group_before = before;
            if existing.is_none() || was_before != before {
                // The box list holding the container's box takes the group.
                if let Some(owner) = self.box_list_owner(container) {
                    self.damage_for_construction(owner);
                }
                self.damage_for_construction(container);
            }
            if let Some(style) = self.pseudo_node_style(group) {
                self.set_scroll_pseudo_style(group, style);
            }
            // The markers, in tree order: its columns' first.
            let mut children = self.column_markers(container, group);
            for (element, _) in &markers {
                marked.insert(*element);
                owners.insert(*element);
                let existing = self.nodes[*element].scroll_pseudos().and_then(|pseudos| pseudos.marker);
                let marker = match existing {
                    Some(marker) if self.nodes.contains_key(marker) => {
                        // A marker moving from another group leaves that group's children
                        // (which may be dropped later in this pass, or keep its box list).
                        let old_group = self.nodes[marker].parent.replace(group);
                        if let Some(old_group) = old_group.filter(|&old| old != group) {
                            self.detach_from_box_lists(marker, Some(old_group));
                        }
                        marker
                    }
                    _ => self.create_scroll_pseudo_node(PseudoElement::ScrollMarker, *element, group, 0),
                };
                self.nodes[*element].element_data_mut().unwrap().scroll.get_or_insert_with(Default::default).marker = Some(marker);
                if let Some(style) = self.pseudo_node_style(marker) {
                    self.set_scroll_pseudo_style(marker, style);
                }
                children.push(marker);
            }
            // Markers that left the group.
            let old_children = std::mem::take(&mut self.nodes[group].children);
            for old in old_children.iter().copied() {
                if !children.contains(&old) && self.nodes.get(old).is_some_and(|node| node.parent == Some(group)) {
                    self.drop_scroll_pseudo_node(old);
                }
            }
            if old_children[..] != children[..] {
                self.damage_for_construction(group);
                self.counters_dirty = true;
            }
            self.nodes[group].children = children.into();
        }

        // Buttons.
        let mut buttoned = std::collections::HashSet::new();
        for (element, styles) in found.buttons {
            buttoned.insert(element);
            owners.insert(element);
            let mut changed = false;
            for (direction, style) in styles.into_iter().enumerate() {
                let existing = self.nodes[element].scroll_pseudos().and_then(|pseudos| pseudos.buttons[direction]);
                match (existing, style) {
                    (Some(button), None) => {
                        self.drop_scroll_pseudo_node(button);
                        self.nodes[element].element_data_mut().unwrap().scroll.as_mut().unwrap().buttons[direction] = None;
                        changed = true;
                    }
                    (existing, Some(_)) => {
                        let button = match existing {
                            Some(button) => button,
                            None => {
                                changed = true;
                                self.create_scroll_pseudo_node(PseudoElement::ScrollButtonAll, element, element, direction as u8)
                            }
                        };
                        self.nodes[element].element_data_mut().unwrap().scroll.get_or_insert_with(Default::default).buttons
                            [direction] = Some(button);
                        self.update_scroll_button_state(button);
                        if let Some(style) = self.pseudo_node_style(button) {
                            self.set_scroll_pseudo_style(button, style);
                        }
                    }
                    (None, None) => {}
                }
            }
            if changed && let Some(owner) = self.box_list_owner(element) {
                self.damage_for_construction(owner);
                self.damage_for_construction(element);
            }
        }

        // What no longer generates.
        let previous: Vec<NodeId> = self.scroll_pseudo_owners.iter().copied().collect();
        for element in previous {
            let Some(node) = self.nodes.get(element) else {
                self.scroll_pseudo_owners.remove(&element);
                continue;
            };
            let Some(pseudos) = node.scroll_pseudos().cloned() else {
                self.scroll_pseudo_owners.remove(&element);
                continue;
            };
            let mut siblings_changed = false;
            if !grouped.contains(&element) && let Some(group) = pseudos.group {
                self.drop_scroll_pseudo_node(group);
                siblings_changed = true;
                let pseudos = self.nodes[element].element_data_mut().unwrap().scroll.as_mut().unwrap();
                pseudos.group = None;
                pseudos.column_markers = None;
                if let Some(column) = pseudos.column.take() {
                    self.drop_scroll_pseudo_node(column);
                }
            }
            if !buttoned.contains(&element) && pseudos.buttons.iter().any(Option::is_some) {
                for button in pseudos.buttons.iter().flatten() {
                    self.drop_scroll_pseudo_node(*button);
                }
                siblings_changed = true;
                self.nodes[element].element_data_mut().unwrap().scroll.as_mut().unwrap().buttons = [None; 4];
            }
            if !marked.contains(&element) && let Some(marker) = pseudos.marker {
                // (Dropped with its group, or left it above.)
                if self.nodes.get(marker).is_some_and(|node| node.scroll_pseudo().is_some_and(|(_, origin)| origin == element)) {
                    let group = self.nodes[marker].parent;
                    self.drop_scroll_pseudo_node(marker);
                    if let Some(group) = group.filter(|&group| self.nodes.contains_key(group)) {
                        self.nodes[group].children.retain(|&id| id != marker);
                        self.damage_for_construction(group);
                    }
                }
                self.nodes[element].element_data_mut().unwrap().scroll.as_mut().unwrap().marker = None;
            }
            if siblings_changed {
                if let Some(owner) = self.box_list_owner(element) {
                    self.damage_for_construction(owner);
                }
                self.damage_for_construction(element);
            }
            if self.nodes[element].scroll_pseudos().is_none_or(ScrollPseudos::is_empty) {
                self.nodes[element].element_data_mut().unwrap().scroll = None;
                if !owners.contains(&element) {
                    self.scroll_pseudo_owners.remove(&element);
                }
            }
        }
    }

    /// Drops a scroll pseudo-element box node, with its children (a group's markers, generated text).
    pub(crate) fn drop_scroll_pseudo_node(&mut self, node_id: NodeId) {
        let Some(node) = self.nodes.get(node_id) else { return };
        // A marker whose element is still its owner forgets it.
        if let Some((_, origin)) = node.scroll_pseudo()
            && let Some(pseudos) = self.nodes.get_mut(origin).and_then(|origin| origin.element_data_mut()).and_then(|data| data.scroll.as_mut())
            && pseudos.marker == Some(node_id)
        {
            pseudos.marker = None;
        }
        // Its children: a group's markers that are still its own (a marker that moved to another
        // group in this pass is that group's now), and generated text.
        let children = std::mem::take(&mut self.nodes[node_id].children);
        for child in children {
            match self.nodes.get(child) {
                None => {}
                Some(node) if node.scroll_pseudo().is_some() => {
                    if node.parent == Some(node_id) {
                        self.drop_scroll_pseudo_node(child);
                    }
                }
                Some(_) => {
                    self.drop_node_ignoring_parent(child);
                }
            }
        }
        let anonymous = std::mem::take(&mut self.nodes[node_id].anonymous_blocks);
        for anon in anonymous {
            self.deallocate_anonymous_block(anon);
        }
        // No list keeps its id: its parent's children (a marker's group), the anonymous block that
        // wrapped its box, and the box lists that held it.
        let parent = self.nodes[node_id].parent;
        self.detach_from_box_lists(node_id, parent);
        self.remove_node_from_tree(node_id);
    }

    /// Removes `node_id` from the children of `parent` (a scroll pseudo node's group) and from every
    /// box list that holds it (its layout parent's children when that is an anonymous block, and
    /// the layout and paint children lists), damaging each owner for box construction.
    fn detach_from_box_lists(&mut self, node_id: NodeId, parent: Option<NodeId>) {
        let node = &self.nodes[node_id];
        let mut owners: SmallVec<[NodeId; 3]> = SmallVec::new();
        owners.extend(parent);
        owners.extend(node.layout_parent.get());
        owners.extend(node.layout_child_of.get());
        for owner in owners {
            let Some(owner_node) = self.nodes.get_mut(owner) else { continue };
            let mut changed = false;
            // A DOM element's children are the DOM's: only pseudo nodes and anonymous blocks list
            // a pseudo box among theirs.
            if (owner_node.scroll_pseudo().is_some() || owner_node.is_anonymous()) && owner_node.children.contains(&node_id) {
                owner_node.children.retain(|&id| id != node_id);
                changed = true;
            }
            if let Some(list) = owner_node.layout_children.get_mut().as_mut()
                && list.contains(&node_id)
            {
                list.retain(|&id| id != node_id);
                changed = true;
            }
            if let Some(list) = owner_node.paint_children.get_mut().as_mut()
                && list.contains(&node_id)
            {
                list.retain(|&id| id != node_id);
                changed = true;
            }
            if changed {
                self.damage_for_construction(owner);
            }
        }
    }

    /// Sets a button's `:enabled`/`:disabled` state: disabled when its originating element cannot
    /// be scrolled in its direction (CSS Overflow 5 § 3.2).
    fn update_scroll_button_state(&mut self, button: NodeId) -> bool {
        use style_dom::ElementState;
        let Some((_, origin)) = self.nodes[button].scroll_pseudo() else { return false };
        let direction = self.nodes[button].scroll_pseudos().map_or(0, |pseudos| pseudos.direction);
        let enabled = self.can_scroll_towards(origin, direction);
        let Some(data) = self.nodes[button].element_data_mut() else { return false };
        let old = data.element_state;
        data.element_state.set(ElementState::ENABLED, enabled);
        data.element_state.set(ElementState::DISABLED, !enabled);
        old != data.element_state
    }

    /// Whether `node_id` can be scrolled towards its logical side `direction` (0 block-start,
    /// 1 inline-start, 2 inline-end, 3 block-end).
    fn can_scroll_towards(&self, node_id: NodeId, direction: u8) -> bool {
        let node = &self.nodes[node_id];
        let Some(style) = node.primary_styles() else { return false };
        if !node.is_root_element() && !is_scroll_container(&style) {
            return false;
        }
        let wm = style.writing_mode;
        drop(style);
        let target = match node.is_root_element() {
            true => crate::scrolling::ScrollTarget::Viewport,
            false => crate::scrolling::ScrollTarget::Node(node_id),
        };
        let (offset, min, max) = self.scroll_state(target, true);
        use style::logical_geometry::PhysicalSide;
        let side = match direction {
            0 => wm.block_start_physical_side(),
            1 => wm.inline_start_physical_side(),
            2 => wm.inline_end_physical_side(),
            _ => wm.block_end_physical_side(),
        };
        // Scroll offsets grow towards the bottom and right (on a reversed axis they are negative).
        const EPSILON: f64 = 0.5;
        // pyro: with mandatory snapping on the axis, a scroll goes no further than the last snap
        // position in that direction (Chromium disables the button there).
        let horizontal = matches!(side, PhysicalSide::Left | PhysicalSide::Right);
        if let Some(positions) = self.mandatory_snap_positions(node_id, horizontal) {
            let (cur, lo, hi) = if horizontal { (offset.x, min.x, max.x) } else { (offset.y, min.y, max.y) };
            let reachable = |p: f64| p.clamp(lo, hi);
            return match side {
                PhysicalSide::Top | PhysicalSide::Left => positions.iter().any(|&p| reachable(p) < cur - EPSILON),
                PhysicalSide::Bottom | PhysicalSide::Right => positions.iter().any(|&p| reachable(p) > cur + EPSILON),
            };
        }
        match side {
            PhysicalSide::Top => offset.y > min.y + EPSILON,
            PhysicalSide::Bottom => offset.y < max.y - EPSILON,
            PhysicalSide::Left => offset.x > min.x + EPSILON,
            PhysicalSide::Right => offset.x < max.x - EPSILON,
        }
    }
}

impl BaseDocument {
    /// pyro: scrolls the marker group `group`, when it is a scroll container, to bring `marker` (one
    /// of its markers) into view on each axis it scrolls: to the marker's snap position where the
    /// group snaps that axis and the marker has a `scroll-snap-align`, else by the least distance.
    /// Whether the group's scroll offset changed.
    fn scroll_group_to_marker(&mut self, group: NodeId, marker: NodeId) -> bool {
        use style::values::computed::BorderStyle;
        use style_traits::ToCss as _;
        let Some(style) = self.nodes[group].primary_styles() else { return false };
        if !is_scroll_container(&style) {
            return false;
        }
        let b = style.get_border();
        let side = |w: app_units::Au, s: BorderStyle| if s.none_or_hidden() { 0.0 } else { w.to_f64_px() };
        let borders = [
            (side(b.border_left_width.0, b.border_left_style), side(b.border_right_width.0, b.border_right_style)),
            (side(b.border_top_width.0, b.border_top_style), side(b.border_bottom_width.0, b.border_bottom_style)),
        ];
        let snap = style.get_box().scroll_snap_type.to_css_string();
        let scrolls = [
            style.get_box().overflow_x != Overflow::Visible && style.get_box().overflow_x != Overflow::Clip,
            style.get_box().overflow_y != Overflow::Visible && style.get_box().overflow_y != Overflow::Clip,
        ];
        drop(style);
        let align = self.nodes[marker]
            .primary_styles()
            .map(|style| style.get_box().scroll_snap_align.to_css_string())
            .unwrap_or_default();
        let mut parts = align.split_whitespace();
        let block_align = parts.next().unwrap_or("none").to_owned();
        let inline_align = parts.next().map(str::to_owned).unwrap_or_else(|| block_align.clone());
        let mut words = snap.split_whitespace();
        let snap_axis = words.next().unwrap_or("none").to_owned();
        let layout = *self.nodes[group].final_layout();
        let marker_layout = *self.nodes[marker].final_layout();
        let content_end = |horizontal: bool| {
            self.nodes[group]
                .layout_children
                .borrow()
                .iter()
                .flatten()
                .map(|&child| {
                    let l = self.nodes[child].final_layout();
                    if horizontal { l.location.x + l.size.width + l.margin.right } else { l.location.y + l.size.height + l.margin.bottom }
                })
                .fold(0.0f32, f32::max) as f64
        };
        let mut offset = *self.nodes[group].scroll_offset();
        for (axis, horizontal) in [(0usize, true), (1usize, false)] {
            if !scrolls[axis] {
                continue;
            }
            let (start_border, end_border) = borders[axis];
            let (size, location, extent, padding_start, padding_end) = if horizontal {
                (layout.size.width, marker_layout.location.x, marker_layout.size.width, layout.padding.left, layout.padding.right)
            } else {
                (layout.size.height, marker_layout.location.y, marker_layout.size.height, layout.padding.top, layout.padding.bottom)
            };
            let client = size as f64 - start_border - end_border;
            let max = (content_end(horizontal) - start_border + padding_end as f64 - client).max(0.0);
            let (lo, hi) = (location as f64 - start_border, location as f64 - start_border + extent as f64);
            let current = if horizontal { offset.x } else { offset.y };
            let snaps = match snap_axis.as_str() {
                "both" => true,
                "x" | "inline" => horizontal,
                "y" | "block" => !horizontal,
                _ => false,
            };
            let keyword = if horizontal { inline_align.as_str() } else { block_align.as_str() };
            let target = match (snaps, keyword) {
                (true, "start") => lo,
                (true, "end") => hi - client,
                (true, "center") => (lo + hi) / 2.0 - client / 2.0,
                _ if lo - (padding_start as f64) < current => lo - padding_start as f64,
                _ if hi + (padding_end as f64) > current + client => hi + padding_end as f64 - client,
                _ => current,
            };
            let target = target.clamp(0.0, max);
            if horizontal { offset.x = target } else { offset.y = target }
        }
        let moved = *self.nodes[group].scroll_offset() != offset;
        *self.nodes[group].scroll_offset_mut() = offset;
        moved
    }

    /// pyro: the snap positions of `scroller` on one axis when its `scroll-snap-type` snaps that
    /// axis mandatorily (`None` otherwise): `scroll-snap-align` of the boxes laid out in it, in
    /// its padding box (scroll-padding and scroll-margin left out).
    fn mandatory_snap_positions(&self, scroller: NodeId, horizontal: bool) -> Option<Vec<f64>> {
        use style::values::computed::BorderStyle;
        use style_traits::ToCss as _;
        let node = &self.nodes[scroller];
        let style = node.primary_styles()?;
        let snap = style.get_box().scroll_snap_type.to_css_string();
        let mut words = snap.split_whitespace();
        let axis = words.next()?;
        let on_axis = match axis {
            "both" => true,
            "x" | "inline" => horizontal,
            "y" | "block" => !horizontal,
            _ => false,
        };
        if !on_axis || words.next() != Some("mandatory") {
            return None;
        }
        let b = style.get_border();
        let side = |w: app_units::Au, s: BorderStyle| if s.none_or_hidden() { 0.0 } else { w.to_f64_px() };
        let (start_border, end_border) = if horizontal {
            (side(b.border_left_width.0, b.border_left_style), side(b.border_right_width.0, b.border_right_style))
        } else {
            (side(b.border_top_width.0, b.border_top_style), side(b.border_bottom_width.0, b.border_bottom_style))
        };
        drop(style);
        let layout = node.final_layout();
        let size = if horizontal { layout.size.width } else { layout.size.height } as f64 - start_border - end_border;
        let mut positions = Vec::new();
        // The boxes laid out in the scroller, their positions relative to its border box.
        let mut stack: Vec<(NodeId, f64)> = node.layout_children.borrow().iter().flatten().map(|&c| (c, 0.0)).collect();
        while let Some((id, base)) = stack.pop() {
            let child = &self.nodes[id];
            let l = child.final_layout();
            let start = base + if horizontal { l.location.x } else { l.location.y } as f64;
            let extent = if horizontal { l.size.width } else { l.size.height } as f64;
            if let Some(cs) = child.primary_styles() {
                let align = cs.get_box().scroll_snap_align.to_css_string();
                let mut parts = align.split_whitespace();
                let block = parts.next().unwrap_or("none");
                let inline = parts.next().unwrap_or(block);
                let keyword = if horizontal { inline } else { block };
                let (lo, hi) = (start - start_border, start - start_border + extent);
                match keyword {
                    "start" => positions.push(lo),
                    "end" => positions.push(hi - size),
                    "center" => positions.push((lo + hi) / 2.0 - size / 2.0),
                    _ => {}
                }
                // Not into nested scroll containers.
                if is_scroll_container(&cs) {
                    continue;
                }
            }
            for &c in child.layout_children.borrow().iter().flatten() {
                stack.push((c, start));
            }
        }
        (!positions.is_empty()).then_some(positions)
    }
}

/// What a marker targets: an element, or a column (`Some(index)`) of a multi-column container.
type Target = (NodeId, Option<u32>);

/// An axis of a scroll container, and whether its start is at its far (bottom or right) side.
#[derive(Clone, Copy)]
struct Axis {
    x: bool,
    reversed: bool,
}

fn axis_of(side: style::logical_geometry::PhysicalSide) -> Axis {
    use style::logical_geometry::PhysicalSide;
    match side {
        PhysicalSide::Top => Axis { x: false, reversed: false },
        PhysicalSide::Bottom => Axis { x: false, reversed: true },
        PhysicalSide::Left => Axis { x: true, reversed: false },
        PhysicalSide::Right => Axis { x: true, reversed: true },
    }
}

impl BaseDocument {
    /// The nearest scroll container of `node_id` through its containing blocks (the root element
    /// for the viewport), as `find_scroll_pseudos` finds it.
    fn nearest_scroll_container(&self, node_id: NodeId) -> Option<NodeId> {
        use style::computed_values::position::T as Position;
        let position = |id: NodeId| self.nodes[id].primary_styles().map(|style| style.get_box().slow_clone_position());
        let mut need = position(node_id)?;
        let mut current = self.nodes[node_id].parent;
        while let Some(id) = current {
            let node = &self.nodes[id];
            current = node.parent;
            let Some(style) = node.primary_styles() else { continue };
            if style.clone_display().is_contents() {
                continue;
            }
            let is_root = node.is_root_element();
            match need {
                Position::Absolute if !is_root && !contains_out_of_flow(&style, false) => continue,
                Position::Fixed if !is_root && !contains_out_of_flow(&style, true) => continue,
                _ => {}
            }
            if is_root || (is_scroll_container(&style) && !self.body_overflow_goes_to_viewport(id)) {
                return Some(id);
            }
            need = style.get_box().slow_clone_position();
        }
        None
    }

    /// The border box of `node_id` relative to the padding box of its scroll container
    /// `scroller`, unscrolled (the viewport's: the document's coordinates).
    fn rect_in_scroller(&self, scroller: NodeId, node_id: NodeId) -> (f32, f32, f32, f32) {
        self.layout_rect_in_scroller(scroller, node_id, false)
    }

    fn layout_rect_in_scroller(&self, scroller: NodeId, node_id: NodeId, strip: bool) -> (f32, f32, f32, f32) {
        let layout = |id: NodeId| if strip { self.nodes[id].unrounded_layout() } else { self.nodes[id].final_layout() };
        let size = layout(node_id).size;
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut current = node_id;
        loop {
            let node = &self.nodes[current];
            let location = layout(current).location;
            x += location.x;
            y += location.y;
            match node.containing_block() {
                Some(cb) if cb == scroller => break,
                Some(cb) if self.nodes.contains_key(cb) => {
                    let scroll = *self.nodes[cb].scroll_offset();
                    if !self.nodes[cb].is_root_element() {
                        x -= scroll.x as f32;
                        y -= scroll.y as f32;
                    }
                    current = cb;
                }
                _ => break,
            }
        }
        if !self.nodes[scroller].is_root_element() {
            let border = layout(scroller).border;
            x -= border.left;
            y -= border.top;
        }
        (x, y, size.width, size.height)
    }

    /// The size of the scrollport of `scroller` (the viewport for the root element).
    fn scrollport_size(&self, scroller: NodeId) -> (f32, f32) {
        let node = &self.nodes[scroller];
        if node.is_root_element() {
            let scale = self.viewport.scale_f64();
            return (
                (self.viewport.window_size.0 as f64 / scale) as f32,
                (self.viewport.window_size.1 as f64 / scale) as f32,
            );
        }
        let layout = node.final_layout();
        (
            (layout.size.width - layout.border.left - layout.border.right - layout.scrollbar_size.width).max(0.0),
            (layout.size.height - layout.border.top - layout.border.bottom - layout.scrollbar_size.height).max(0.0),
        )
    }

    /// Of `candidates` (boxes in `scroller`, in tree order), the active one: the one whose
    /// scroll-into-view position is nearest the scroll position, in the block axis and then the
    /// inline axis, after the positions the scroll position cannot reach are spread over the ends
    /// (CSS Overflow 5 § 3.1.8's example algorithm, as Chromium selects).
    fn select_in_scroller(&self, scroller: NodeId, candidates: &[Target]) -> Option<Target> {
        let first = *candidates.first()?;
        let style = self.nodes[scroller].primary_styles()?;
        let wm = style.writing_mode;
        drop(style);
        let target = match self.nodes[scroller].is_root_element() {
            true => crate::scrolling::ScrollTarget::Viewport,
            false => crate::scrolling::ScrollTarget::Node(scroller),
        };
        let (mut offset, min, max) = self.scroll_state(target, true);
        // pyro: the page's scroll offset (layout syncs leave the node's own as the last capture set
        // it; `set_timeline_scroll`), so the active marker follows script scrolls between captures.
        if let Some((elements, viewport)) = &self.timeline_scroll {
            let (x, y) = match target {
                crate::scrolling::ScrollTarget::Viewport => *viewport,
                _ => elements.get(&scroller).copied().unwrap_or((offset.x, offset.y)),
            };
            offset = crate::Point { x, y };
        }
        let port = self.scrollport_size(scroller);
        let mut cands: Vec<Target> = candidates.to_vec();
        for axis in [axis_of(wm.block_start_physical_side()), axis_of(wm.inline_start_physical_side())] {
            let (offset, min, max, port) = match axis.x {
                true => (offset.x as f32, min.x as f32, max.x as f32, port.0),
                false => (offset.y as f32, min.y as f32, max.y as f32, port.1),
            };
            let range = max - min;
            let position = if axis.reversed { max - offset } else { offset - min };
            let mut positions: Vec<f32> = cands
                .iter()
                .map(|&target| {
                    let (x, y, w, h) = self.target_rect(scroller, target);
                    match (axis.x, axis.reversed) {
                        (true, false) => x,
                        (false, false) => y,
                        (true, true) => port - (x + w),
                        (false, true) => port - (y + h),
                    }
                })
                .collect();
            // Targets the scroll position cannot reach (near the start or the end) are spread
            // over the first and last eighth of a scrollport.
            if range > 0.0 {
                let distribute = (port / 8.0).min(range / 2.0);
                let lowest = positions.iter().copied().fold(f32::INFINITY, f32::min);
                let highest = positions.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                for p in positions.iter_mut() {
                    if *p < distribute {
                        *p = if distribute > lowest { (*p - lowest) / (distribute - lowest) * distribute } else { 0.0 };
                    } else if *p > range - distribute {
                        let end = range - distribute;
                        *p = if highest > end { (*p - end) / (highest - end) * distribute + end } else { range };
                    }
                }
            }
            // The position nearest the scroll position (the first of two as near), as Chromium
            // selects (WPT `scroll-target-group-012`; `scroll-marker-selection-picks-closest`, which
            // CSS Overflow 5 § 3.1.8's half-scrollport rule also passes).
            let mut sorted = positions.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            sorted.dedup_by(|a, b| (*a - *b).abs() < 0.5);
            const EPSILON: f32 = 0.5;
            let mut selected: Option<f32> = None;
            for &p in &sorted {
                if selected.is_none_or(|best| (p - position).abs() < (best - position).abs() - 0.01) {
                    selected = Some(p);
                }
            }
            let selected = selected.or(sorted.first().copied())?;
            let kept: Vec<Target> = cands
                .iter()
                .zip(&positions)
                .filter(|&(_, &p)| (p - selected).abs() < EPSILON)
                .map(|(&target, _)| target)
                .collect();
            cands = kept;
            if cands.len() <= 1 {
                break;
            }
        }
        cands.first().copied().or(Some(first))
    }

    /// Of `targets` (in the tree order of their markers), the index of the active one: in the
    /// nearest scroll container they share, the target (or the scroll container holding targets)
    /// the scroll position selects, and so on down.
    fn select_active_target(&self, targets: &[Option<Target>]) -> Option<usize> {
        let live: Vec<(usize, Target)> = targets
            .iter()
            .enumerate()
            .filter_map(|(index, target)| {
                target
                    .filter(|&(id, _)| self.nodes.contains_key(id) && self.nodes[id].primary_styles().is_some())
                    .map(|target| (index, target))
            })
            .collect();
        let (first_index, _) = *live.first()?;
        // Each target's chain of scroll containers, outermost first (a column's ends with its
        // multi-column container).
        let chains: Vec<Vec<NodeId>> = live
            .iter()
            .map(|&(_, (id, column))| {
                let mut chain = Vec::new();
                if column.is_some() {
                    chain.push(id);
                }
                let mut current = self.nearest_scroll_container(id);
                while let Some(scroller) = current {
                    chain.push(scroller);
                    current = self.nearest_scroll_container(scroller);
                }
                chain.reverse();
                chain
            })
            .collect();
        // The nearest scroll container they all share.
        let mut depth = 0;
        while chains.iter().all(|chain| chain.len() > depth && chain[depth] == chains[0][depth]) {
            depth += 1;
        }
        if depth == 0 {
            return Some(first_index);
        }
        let mut scroller = chains[0][depth - 1];
        let mut remaining: Vec<usize> = (0..live.len()).collect();
        loop {
            // Each remaining target, or the scroll container holding it right in `scroller`.
            let level = chains[remaining[0]].iter().position(|&id| id == scroller).unwrap_or(0) + 1;
            let representatives: Vec<Target> = remaining
                .iter()
                .map(|&i| chains[i].get(level).map(|&id| (id, None)).unwrap_or(live[i].1))
                .collect();
            let mut unique: Vec<Target> = Vec::new();
            for &r in &representatives {
                if !unique.contains(&r) {
                    unique.push(r);
                }
            }
            let active = match self.select_by_scroll_target(scroller, &unique) {
                Some(active) => active,
                None => self.select_in_scroller(scroller, &unique)?,
            };
            let next: Vec<usize> = remaining
                .iter()
                .zip(&representatives)
                .filter(|&(_, &r)| r == active)
                .map(|(&i, _)| i)
                .collect();
            // The active box is a target, or a scroll container holding targets.
            if let Some(&target) = next.iter().find(|&&i| live[i].1 == active) {
                return Some(live[target].0);
            }
            if next.is_empty() {
                return Some(first_index);
            }
            scroller = active.0;
            remaining = next;
        }
    }

    /// The border box of a target in its scroll container `scroller`, as `rect_in_scroller`: an
    /// element's, or a column's box.
    fn target_rect(&self, scroller: NodeId, (id, column): Target) -> (f32, f32, f32, f32) {
        let Some(index) = column else {
            return self.rect_in_scroller(scroller, id);
        };
        let border = self.nodes[id].final_layout().border;
        let fragment = self.nodes[id]
            .column_fragments()
            .and_then(|columns| columns.fragments.iter().filter(|fragment| !fragment.spanner).nth(index as usize).copied());
        match fragment {
            Some(f) => (f.x - border.left, f.top - border.top, f.width, f.height),
            None => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The target of a marker node: its element, or its multi-column container and column.
    fn marker_target(&self, marker: NodeId) -> Option<Target> {
        let node = self.nodes.get(marker)?;
        let (_, origin) = node.scroll_pseudo()?;
        match self.nodes.get(origin)?.scroll_pseudo() {
            Some((PseudoElement::Column, container)) => Some((container, Some(node.scroll_pseudos()?.index))),
            _ => Some((origin, None)),
        }
    }

    /// pyro: after layout, sets `:target-current` on the active marker of each group and the
    /// `:enabled`/`:disabled` state of each scroll button, which depend on scroll positions.
    /// Whether a state changed (styles were computed again, or elements wait for a restyle).
    /// pyro: after scroll offsets changed outside a layout (the page told new ones), on documents
    /// that use scroll markers: the next resolve picks the active markers again
    /// ([`Self::update_target_current`], which restyles what depends on them).
    /// pyro: the scroll containers whose `scroll-snap-type` snaps an axis mandatorily (their
    /// initial scroll position snaps: CSS Scroll Snap 1 § 6.3).
    pub fn mandatory_snap_containers(&self) -> Vec<NodeId> {
        use style_traits::ToCss as _;
        if !self.scroll_snap_styles {
            return Vec::new();
        }
        self.nodes
            .iter()
            .filter(|(_, node)| node.is_element() && node.flags.is_in_document())
            .filter(|(_, node)| {
                node.primary_styles().is_some_and(|style| {
                    is_scroll_container(&style) && style.get_box().scroll_snap_type.to_css_string().ends_with("mandatory")
                })
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// pyro: the elements styled `scroll-initial-target: nearest` (CSS Scroll Snap 2 § 3: their
    /// nearest scroll container starts scrolled to them), in tree order.
    pub fn initial_scroll_targets(&self) -> Vec<NodeId> {
        use style::values::computed::ScrollInitialTarget;
        if !self.scroll_initial_target_styles {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut stack = vec![self.root_node().id];
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get(id) else { continue };
            if node.is_element()
                && node.primary_styles().is_some_and(|style| {
                    style.get_box().slow_clone_scroll_initial_target() == ScrollInitialTarget::Nearest
                })
            {
                found.push(id);
            }
            // (A shadow host's children are its shadow tree's: a target in one counts too.)
            stack.extend(node.children.iter().rev().copied());
        }
        found
    }

    /// pyro: the target of the `::scroll-marker` box at the document point (`x`, `y`): its
    /// element, or its multi-column container and column (CSS Overflow 5 § 3.2: activating a
    /// marker scrolls to its target; WPT css-overflow `column-scroll-marker-002`).
    pub fn scroll_marker_at(&self, x: f32, y: f32) -> Option<(NodeId, Option<u32>)> {
        for &owner in &self.scroll_pseudo_owners {
            let Some(group) = self.nodes.get(owner).and_then(|n| n.scroll_pseudos()).and_then(|s| s.group) else { continue };
            let Some(group_node) = self.nodes.get(group) else { continue };
            let markers: Vec<NodeId> = group_node.layout_children.borrow().iter().flatten().copied().collect();
            for marker in markers {
                let Some(node) = self.nodes.get(marker) else { continue };
                if !matches!(node.scroll_pseudo(), Some((PseudoElement::ScrollMarker, _))) {
                    continue;
                }
                let origin = node.absolute_position(0.0, 0.0);
                let (left, top) = (origin.x + node.scroll_offset().x as f32, origin.y + node.scroll_offset().y as f32);
                let size = node.final_layout().size;
                if x >= left && y >= top && x < left + size.width && y < top + size.height {
                    return self.marker_target(marker);
                }
            }
        }
        None
    }

    /// pyro: the `::scroll-button()` of `owner` hit at the document point (`x`, `y`), by the
    /// painted hit test (`hit`: stacking order, clips, `visibility` and `pointer-events` as for any
    /// box): the physical direction it scrolls (`(dx, dy)`, each -1, 0 or 1) and whether it is
    /// enabled (`owner` can scroll that way, `can_scroll_towards`). The hit box must be the button
    /// or inside it (its text): one of `owner`'s own boxes or a child's covering the button is no
    /// button hit. Nothing is hit-tested for an element without buttons. CSS Overflow 5 § 2.2: a
    /// button's activation scrolls its originating element by a page in its direction.
    pub fn scroll_button_hit(&mut self, owner: NodeId, x: f32, y: f32) -> Option<((i8, i8), bool)> {
        use style::logical_geometry::PhysicalSide;
        let buttons = self.nodes.get(owner)?.scroll_pseudos()?.buttons;
        if buttons.iter().all(Option::is_none) {
            return None;
        }
        let hit = self.hit(x, y)?;
        // From the hit box up to `owner`: a button's box is a child of its originating element.
        let mut id = hit.node_id;
        let direction = loop {
            if id == owner {
                return None;
            }
            if let Some(direction) = buttons.iter().position(|&button| button == Some(id)) {
                break direction;
            }
            id = self.nodes.get(id)?.parent?;
        };
        let wm = self.nodes[owner].primary_styles()?.writing_mode;
        let side = match direction {
            0 => wm.block_start_physical_side(),
            1 => wm.inline_start_physical_side(),
            2 => wm.inline_end_physical_side(),
            _ => wm.block_end_physical_side(),
        };
        let delta = match side {
            PhysicalSide::Top => (0, -1),
            PhysicalSide::Bottom => (0, 1),
            PhysicalSide::Left => (-1, 0),
            PhysicalSide::Right => (1, 0),
        };
        Some((delta, self.can_scroll_towards(owner, direction as u8)))
    }

    /// pyro: whether an element of the document was ever styled with a `scroll-snap-type`.
    pub fn uses_scroll_snap(&self) -> bool {
        self.scroll_snap_styles
    }

    /// pyro: whether an element of the document was ever styled `scroll-initial-target: nearest`.
    pub fn uses_scroll_initial_target(&self) -> bool {
        self.scroll_initial_target_styles
    }

    /// pyro: whether the document has scroll pseudo-elements (markers, groups, buttons).
    pub fn uses_scroll_markers(&self) -> bool {
        !self.scroll_pseudo_owners.is_empty()
    }

    pub fn update_scroll_marker_states(&mut self) {
        // pyro: and `scroll-state(scrollable)` queries (`resolve::refresh_scroll_state_queries`).
        let scroll_state_queries = !self.nodes.scroll_state_queries.lock().unwrap().is_empty();
        if !self.scroll_pseudo_owners.is_empty() || scroll_state_queries {
            if let Some(root) = self.try_root_element().map(|root| root.id) {
                self.nodes[root].mark_damaged();
            }
        }
    }

    pub(crate) fn update_target_current(&mut self) -> bool {
        use style_dom::ElementState;
        use style::selector_parser::{TARGET_AFTER, TARGET_BEFORE, TARGET_CURRENT};
        const TARGET_STATES: ElementState = TARGET_CURRENT.union(TARGET_BEFORE).union(TARGET_AFTER);
        // `:target-current` on the active marker, `:target-before` on the ones before it and
        // `:target-after` on the ones after it (none without an active marker).
        let target_states = |index: usize, active: Option<usize>| match active {
            Some(active) if index == active => TARGET_CURRENT,
            Some(active) if index < active => TARGET_BEFORE,
            Some(_) => TARGET_AFTER,
            None => ElementState::empty(),
        };
        if self.scroll_pseudo_owners.is_empty() && self.scroll_target_groups.is_empty() {
            return false;
        }
        let mut changed = false;
        // Column markers built for another number of columns are built again.
        let owners: Vec<NodeId> = self.scroll_pseudo_owners.iter().copied().collect();
        if owners.iter().any(|&owner| self.column_markers_stale(owner)) {
            self.scroll_markers_dirty = true;
            self.resolve_scroll_markers();
            changed = true;
        }
        // `::scroll-marker`s.
        let owners: Vec<NodeId> = self.scroll_pseudo_owners.iter().copied().collect();
        for owner in owners {
            let Some(group) = self.nodes.get(owner).and_then(|node| node.scroll_pseudos()).and_then(|pseudos| pseudos.group) else {
                continue;
            };
            let markers: Vec<NodeId> = self.nodes[group].children.iter().copied().collect();
            let targets: Vec<Option<Target>> = markers.iter().map(|&marker| self.marker_target(marker)).collect();
            let active = self.select_active_target(&targets);
            let mut active_changed = false;
            for (index, &marker) in markers.iter().enumerate() {
                let Some(data) = self.nodes.get_mut(marker).and_then(|node| node.element_data_mut()) else { continue };
                let state = target_states(index, active);
                if data.element_state & TARGET_STATES != state {
                    data.element_state.remove(TARGET_STATES);
                    data.element_state.insert(state);
                    if let Some(style) = self.pseudo_node_style(marker) {
                        self.set_scroll_pseudo_style(marker, style);
                    }
                    changed = true;
                    active_changed |= index == active.unwrap_or(usize::MAX);
                }
            }
            // pyro: a group that scrolls keeps its active marker in view (CSS Overflow 5 § 3.1.5,
            // as Chromium: aligned as the marker snaps in a snapping group, else the nearest
            // edge; WPT css-overflow `scroll-marker-group-snap-aligns-to-active`).
            if active_changed && let Some(active) = active {
                let moved = self.scroll_group_to_marker(group, markers[active]);
                if moved {
                    self.scroll_group_moves.push(owner);
                }
            }
        }
        // Scroll buttons.
        let owners: Vec<NodeId> = self.scroll_pseudo_owners.iter().copied().collect();
        for owner in owners {
            let Some(buttons) = self.nodes.get(owner).and_then(|node| node.scroll_pseudos()).map(|pseudos| pseudos.buttons) else {
                continue;
            };
            for button in buttons.into_iter().flatten() {
                if self.nodes.contains_key(button) && self.update_scroll_button_state(button) {
                    if let Some(style) = self.pseudo_node_style(button) {
                        self.set_scroll_pseudo_style(button, style);
                    }
                    changed = true;
                }
            }
        }
        // The links of scroll target groups (`scroll-target-group: auto`).
        let groups = self.scroll_target_groups.clone();
        for (_, links) in groups {
            let targets: Vec<Option<Target>> = links.iter().map(|&link| self.link_target(link).map(|id| (id, None))).collect();
            let active = self.select_active_target(&targets);
            for (index, link) in links.into_iter().enumerate() {
                let state = if targets[index].is_some() { target_states(index, active) } else { ElementState::empty() };
                let Some(node) = self.nodes.get(link) else { continue };
                if node.element_data().is_some_and(|data| data.element_state & TARGET_STATES != state) {
                    self.snapshot_node_state_only(link);
                    let node = &mut self.nodes[link];
                    let data = node.element_data_mut().unwrap();
                    data.element_state.remove(TARGET_STATES);
                    data.element_state.insert(state);
                    node.mark_ancestors_dirty();
                    changed = true;
                }
            }
        }
        changed
    }

    /// The element a link's `href` points at in this document (its fragment's).
    fn link_target(&self, link: NodeId) -> Option<NodeId> {
        let href = self.nodes.get(link)?.attr(local_name!("href"))?;
        let href = href.trim();
        let fragment = match href.strip_prefix('#') {
            Some(fragment) => fragment.to_string(),
            None => {
                let url = self.resolve_url(href)?;
                let document = self.resolve_url("")?;
                if url[..url::Position::AfterQuery] != document[..url::Position::AfterQuery] {
                    return None;
                }
                url.fragment()?.to_string()
            }
        };
        let fragment = percent_encoding::percent_decode_str(&fragment).decode_utf8_lossy().into_owned();
        self.nodes_to_id.get(&fragment)?.first().copied().filter(|&id| self.nodes[id].flags.is_in_document())
    }
}

impl BaseDocument {
    /// The markers of the columns of the multi-column scroll container `container`, in its group
    /// `group` (`::column::scroll-marker`, Multi-column 2 § 4.8): one per column of its last layout
    /// (none before it is laid out; `update_target_current` builds them again when the count
    /// changes). Its column markers that are no longer wanted are dropped.
    fn column_markers(&mut self, container: NodeId, group: NodeId) -> Vec<NodeId> {
        let existing: Vec<NodeId> = self.nodes[group]
            .children
            .iter()
            .copied()
            .filter(|&id| {
                self.nodes.get(id).and_then(|node| node.scroll_pseudo()).is_some_and(|(_, origin)| {
                    self.nodes.get(origin).and_then(|node| node.scroll_pseudo()) == Some((PseudoElement::Column, container))
                })
            })
            .collect();
        let multicol = self.nodes[container].primary_styles().is_some_and(|style| super::multicol::is_multicol_style(&style));
        let column = self.nodes[container].scroll_pseudos().and_then(|pseudos| pseudos.column);
        let column = match (multicol, column) {
            (false, _) => None,
            (true, Some(column)) => Some(column),
            (true, None) => {
                let column = self.create_scroll_pseudo_node(PseudoElement::Column, container, container, 0);
                self.nodes[container].element_data_mut().unwrap().scroll.get_or_insert_with(Default::default).column = Some(column);
                Some(column)
            }
        };
        // Whether a rule gives the columns' markers content.
        let wanted = column.is_some_and(|column| {
            use style::stylist::RuleInclusion;
            let Some(style) = self.nodes[container].primary_styles().map(|style| Arc::clone(&*style)) else { return false };
            let read_guard = self.guard.read();
            let guards = StylesheetGuards::same(&read_guard);
            self.stylist
                .lazily_compute_pseudo_element_style::<&Node>(
                    &guards,
                    &self.nodes[column],
                    &PseudoElement::ScrollMarker,
                    RuleInclusion::All,
                    &style,
                    true,
                    None,
                )
                .is_some_and(|style| generates(&style))
        });
        let count = match wanted {
            true => self.nodes[container]
                .column_fragments()
                .map_or(0, |columns| columns.fragments.iter().filter(|fragment| !fragment.spanner).count()),
            false => 0,
        };
        if let Some(pseudos) = self.nodes[container].element_data_mut().and_then(|data| data.scroll.as_mut()) {
            pseudos.column_markers = wanted.then_some(count as u32);
        }
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let marker = match existing.get(index) {
                Some(&marker) => marker,
                None => self.create_scroll_pseudo_node(PseudoElement::ScrollMarker, column.unwrap(), group, 0),
            };
            if let Some(pseudos) = self.nodes[marker].element_data_mut().and_then(|data| data.scroll.as_mut()) {
                pseudos.index = index as u32;
            }
            if let Some(style) = self.pseudo_node_style(marker) {
                self.set_scroll_pseudo_style(marker, style);
            }
            out.push(marker);
        }
        for &extra in existing.iter().skip(count) {
            self.drop_scroll_pseudo_node(extra);
        }
        out
    }

    /// Whether a multi-column scroll container's column markers no longer match its columns.
    fn column_markers_stale(&self, container: NodeId) -> bool {
        let Some(wanted) = self.nodes.get(container).and_then(|node| node.scroll_pseudos()).and_then(|pseudos| pseudos.column_markers)
        else {
            return false;
        };
        let count = self.nodes[container]
            .column_fragments()
            .map_or(0, |columns| columns.fragments.iter().filter(|fragment| !fragment.spanner).count());
        count as u32 != wanted
    }
}

impl BaseDocument {
    /// pyro: whether styles follow scroll positions (scroll markers' `:target-current`, scroll
    /// buttons' `:disabled`): an embedder that sets a scroll position after a resolve resolves
    /// again.
    pub fn tracks_scroll_states(&self) -> bool {
        !self.scroll_pseudo_owners.is_empty()
            || !self.scroll_target_groups.is_empty()
            || !self.nodes.scroll_state_queries.lock().unwrap().is_empty()
    }

    /// pyro: the current scroll target of each scroll container (CSS Overflow 5 § 2.1: the
    /// element a scroll into view last revealed in it, until it scrolls otherwise; the viewport's
    /// under the root element), which the embedder tracks. Scroll marker selection starts from it.
    pub fn set_scroll_targets(&mut self, targets: Vec<(NodeId, NodeId)>) {
        let targets: std::collections::HashMap<NodeId, NodeId> = targets.into_iter().collect();
        if targets != self.scroll_targets {
            self.scroll_targets = targets;
        }
    }

    /// Of `candidates` (in `scroller`, in tree order), the one a reverse tree-order walk from
    /// `scroller`'s current scroll target meets first (CSS Overflow 5 § 3.1.8): the last one at or
    /// before it in tree order (a column: the column showing it), or the first one.
    fn select_by_scroll_target(&self, scroller: NodeId, candidates: &[Target]) -> Option<Target> {
        let target = *self.scroll_targets.get(&scroller)?;
        if !self.nodes.get(target).is_some_and(|node| node.flags.is_in_document()) {
            return None;
        }
        // The target must be inside the scroll container.
        let mut ancestor = self.nodes[target].parent;
        let mut inside = self.nodes[scroller].is_root_element();
        while let Some(id) = ancestor.filter(|_| !inside) {
            inside = id == scroller;
            ancestor = self.nodes[id].parent;
        }
        if !inside {
            return None;
        }
        let path = self.tree_path(target);
        let mut chosen = None;
        let mut target_columns = std::collections::HashMap::new();
        for &candidate in candidates {
            let at_or_before = match candidate {
                (container, Some(index)) => target_columns.entry(container).or_insert_with(|| {
                    let (_, y, _, _) = self.layout_rect_in_scroller(container, target, true);
                    let strip_y = y + self.nodes[container].unrounded_layout().border.top;
                    let mut ordinal = 0;
                    for fragment in &self.nodes[container].column_fragments()?.fragments {
                        if strip_y < fragment.strip_end { return (!fragment.spanner).then_some(ordinal); }
                        ordinal += usize::from(!fragment.spanner);
                    }
                    None
                }).is_some_and(|column| index as usize <= column),
                (id, None) => self.tree_path(id) <= path,
            };
            if at_or_before {
                chosen = Some(candidate);
            }
        }
        chosen.or(candidates.first().copied())
    }

    /// The child indices from the root down to `node_id` (tree order compares them).
    fn tree_path(&self, node_id: NodeId) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = node_id;
        while let Some(parent) = self.nodes.get(current).and_then(|node| node.parent) {
            path.push(self.nodes[parent].children.iter().position(|&id| id == current).unwrap_or(0));
            current = parent;
        }
        path.reverse();
        path
    }
}
