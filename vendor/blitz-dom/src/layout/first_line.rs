//! pyro: `::first-line` (CSS 2.1 5.12.1, CSS Pseudo-Elements 4 2.1).
//!
//! The first formatted line of a block container takes the style of the container's
//! `::first-line`, as if the line's content were wrapped in an inline box of that style (the
//! fictional tag sequence of CSS 2.1): text on the line inherits from it, and the elements on the
//! line inherit through it. As for `::first-letter`, a block container passes its first line on to
//! its first in-flow block child, and the nearest container with a `::first-line` style is the one
//! that styles it (`first_letter::origin`); the style then inherits from the block holding the
//! line.
//!
//! The style is on a node of its own, owned by the originating element
//! (`ElementData::first_line`), whose parent is the inline root holding the line: the first-line
//! box. It is not in the inline layout, whose lines are broken in the root's own styles (the
//! properties that would change the layout, fonts and spacing, are not applied). Paint draws the
//! first line with it (blitz-paint): the line's text in its colour, decorations and shadows, its
//! background as an inline box around the line's content, and the elements on the line with their
//! styles reparented onto it ([`BaseDocument::first_line_style`]), as Gecko reparents them.

use blitz_traits::node_id::NodeId;
use markup5ever::{QualName, local_name, ns};
use style::{
    context::{CascadeInputs, TreeCountingCaches},
    data::ElementData as StyloElementData,
    properties::{ComputedValues, FirstLineReparenting},
    rule_cache::RuleCacheConditions,
    selector_parser::PseudoElement,
    servo_arc::Arc,
    shared_lock::StylesheetGuards,
    values::specified::position::PositionTryFallbacksTryTactic,
};

use super::first_letter::{self, first_line_block, origin};
use crate::{BaseDocument, ElementData, Node};

/// The `::first-line` style node of the inline root `root`'s first line, created or updated, when
/// some block container's `::first-line` styles that line (`None` otherwise).
pub(crate) fn style_node_for(doc: &mut BaseDocument, root: NodeId) -> Option<NodeId> {
    let (origin_id, eager) = origin(doc, root, &PseudoElement::FirstLine)?;
    let style = if root == origin_id {
        eager
    } else {
        // Inherit from the block holding the line: the rules the cascade matched for the
        // originating element, cascaded again with that parent.
        let parent_style: Arc<ComputedValues> = (*doc.nodes[root].primary_styles()?).clone();
        let read_guard = doc.guard.read();
        let guards = StylesheetGuards::same(&read_guard);
        doc.stylist.compute_pseudo_element_style_with_inputs(
            CascadeInputs::new_from_style(&eager),
            &PseudoElement::FirstLine,
            &guards,
            Some(&*parent_style),
            Some(&doc.nodes[origin_id]),
        )
    };
    Some(style_node(doc, origin_id, root, style))
}

/// pyro: the styles the inline elements of `root` have on its first line, when its `::first-line`
/// style (node `first_line`) changes layout (font size, line height, font family, weight or style,
/// letter or word spacing): `TextLayout::first_line_styles`. Empty otherwise, and past 4,096
/// elements.
pub(crate) fn layout_styles(doc: &BaseDocument, root: NodeId, first_line: NodeId) -> Vec<(NodeId, Arc<ComputedValues>)> {
    let (Some(line), Some(own)) = (
        doc.nodes.get(first_line).and_then(|node| node.primary_styles()),
        doc.nodes.get(root).and_then(|node| node.primary_styles()),
    ) else {
        return Vec::new();
    };
    let (lf, of) = (line.get_font(), own.get_font());
    let (lt, ot) = (line.get_inherited_text(), own.get_inherited_text());
    if lf.font_size == of.font_size
        && lf.line_height == of.line_height
        && lf.font_family == of.font_family
        && lf.font_weight == of.font_weight
        && lf.font_style == of.font_style
        && lt.letter_spacing == ot.letter_spacing
        && lt.word_spacing == ot.word_spacing
    {
        return Vec::new();
    }
    let line: Arc<ComputedValues> = (*line).clone();
    drop(own);
    // Cascaded again in full with the first-line box as the root's content's parent (not
    // reparented: `em` lengths of reset properties, an edge's `1em` border, follow it too).
    let mut out: Vec<(NodeId, Arc<ComputedValues>)> = Vec::new();
    let mut stack: Vec<(NodeId, Arc<ComputedValues>)> =
        doc.nodes[root].children.iter().rev().map(|&id| (id, line.clone())).collect();
    while let Some((id, parent)) = stack.pop() {
        if out.len() >= 4096 {
            break;
        }
        let Some(node) = doc.nodes.get(id) else { continue };
        if !node.is_element() || !node.display_style().is_some_and(|d| d.is_inline_flow()) {
            continue;
        }
        let Some(own) = node.primary_styles().map(|style| (*style).clone()) else { continue };
        let style = recascade(doc, node, &own, &parent);
        stack.extend(node.children.iter().rev().map(|&child| (child, style.clone())));
        out.push((id, style));
    }
    out
}

/// pyro: `own` (the style of `node`) cascaded again in full with `parent` as its parent.
fn recascade(doc: &BaseDocument, node: &Node, own: &Arc<ComputedValues>, parent: &ComputedValues) -> Arc<ComputedValues> {
    let read_guard = doc.guard.read();
    let guards = StylesheetGuards::same(&read_guard);
    let inputs = CascadeInputs::new_from_style(own);
    let pseudo = own.pseudo();
    doc.stylist.cascade_style_and_visited(
        Some(node),
        pseudo.as_ref(),
        &inputs,
        &guards,
        Some(parent),
        Some(parent),
        FirstLineReparenting::No,
        &PositionTryFallbacksTryTactic::default(),
        None,
        &mut RuleCacheConditions::default(),
        &mut TreeCountingCaches::default(),
    )
}

/// The originating element's first-line node, created when it has none, with `style` and the
/// inline root holding the line as its parent.
fn style_node(doc: &mut BaseDocument, origin_id: NodeId, root: NodeId, style: Arc<ComputedValues>) -> NodeId {
    const NAME: QualName = QualName { prefix: None, ns: ns!(html), local: local_name!("span") };
    let node_id = match doc.nodes[origin_id].element_data().and_then(|element| element.first_line) {
        Some(node_id) if doc.nodes.contains_key(node_id) => node_id,
        _ => {
            let node_id =
                doc.create_node(crate::NodeData::AnonymousBlock(Box::new(ElementData::new(NAME, Vec::new()))));
            if let Some(element) = doc.nodes[origin_id].element_data_mut() {
                element.first_line = Some(node_id);
            }
            node_id
        }
    };
    let node = &mut doc.nodes[node_id];
    node.parent = Some(root);
    let unchanged = node.primary_styles().is_some_and(|old| std::ptr::eq::<ComputedValues>(&**old, &*style));
    if !unchanged {
        let mut data = StyloElementData::default();
        data.styles.primary = Some(style);
        data.set_restyled();
        *node.stylo_element_data_mut().ensure_init_mut() = data;
        node.update_layout_styles();
        // Its background images load as an element's do.
        doc.pending_style_image_nodes.push(node_id);
    }
    node_id
}

/// An element's `::first-line` style came, went or changed in the style traversal: the block its
/// first line is in builds its inline layout again (which finds the style node anew). A style that
/// changed on a first line that has its node only restyles the node: the layout does not read it.
pub(crate) fn style_changed(doc: &mut BaseDocument, origin_id: NodeId) {
    doc.first_line_reparented.0.get_mut().unwrap().clear();
    if !doc.nodes.contains_key(origin_id) {
        return;
    }
    let node_id = doc.nodes[origin_id].element_data().and_then(|element| element.first_line);
    let root = node_id.and_then(|id| doc.nodes.get(id)).and_then(|node| node.parent);
    if let (Some(node_id), Some(root)) = (node_id, root)
        && doc.nodes.get(root).is_some_and(|root| {
            root.element_data()
                .and_then(|element| element.inline_layout_data.as_ref())
                .is_some_and(|layout| layout.first_line == Some(node_id))
        })
        && style_node_for(doc, root) == Some(node_id)
    {
        return;
    }
    let block = first_line_block(doc, origin_id);
    doc.nodes[block].insert_damage(super::damage::ALL_DAMAGE);
    doc.nodes[block].mark_damaged();
    // The root that held the line (the block itself, or an anonymous block in it).
    if let Some(root) = doc.nodes[origin_id]
        .element_data()
        .and_then(|element| element.first_line)
        .and_then(|node_id| doc.nodes.get(node_id))
        .and_then(|node| node.parent)
        .filter(|&root| root != block && doc.nodes.contains_key(root))
    {
        doc.nodes[root].insert_damage(super::damage::ALL_DAMAGE);
        doc.nodes[root].mark_damaged();
    }
    let has_style = doc.nodes[origin_id]
        .try_stylo_element_data()
        .and_then(|data| data.get().map(|data| data.styles.pseudos.get(&PseudoElement::FirstLine).is_some()))
        .unwrap_or(false);
    if !has_style {
        drop_style_node(doc, origin_id);
    }
}

/// Whether the first line of the inline root `root` is the first formatted line of a block (CSS
/// 2.1 16.1: "the first line of an anonymous block box is only affected if it is the first child
/// of its parent element"): a root that is an element, or an anonymous block that is its
/// container's first in-flow child (through inlines split around blocks, whose pieces' blocks
/// are their container's). `text-indent` indents only such lines.
pub(crate) fn starts_a_block(doc: &BaseDocument, root: NodeId) -> bool {
    let mut current = root;
    loop {
        let node = &doc.nodes[current];
        if !node.is_anonymous() && !node.is_split_inline() {
            return true;
        }
        let Some(parent) = node.layout_parent.get() else { return true };
        // pyro: an anonymous flex or grid item is a block container of its own: its first line
        // is a first formatted line (WPT css-text `text-indent/anonymous-flex-item-001`).
        if is_flex_or_grid_container(&doc.nodes[parent]) {
            return true;
        }
        if !first_letter::is_first_in_flow_child(doc, parent, current) {
            return false;
        }
        current = parent;
    }
}

/// pyro: whether `node` lays out its children as flex or grid items.
pub(crate) fn is_flex_or_grid_container(node: &crate::Node) -> bool {
    node.display_style().is_some_and(|display| {
        matches!(display.inside(), style::values::specified::box_::DisplayInside::Flex | style::values::specified::box_::DisplayInside::Grid)
    })
}

/// Drops an element's first-line node.
pub(crate) fn drop_style_node(doc: &mut BaseDocument, origin_id: NodeId) {
    let Some(node_id) = doc
        .nodes
        .get_mut(origin_id)
        .and_then(|node| node.element_data_mut())
        .and_then(|element| element.first_line.take())
    else {
        return;
    };
    if doc.nodes.contains_key(node_id) {
        doc.remove_node_from_tree(node_id);
    }
}

impl BaseDocument {
    /// pyro: the element the `::first-line` box of the inline root `root` is in, among the
    /// ancestors of the root's content: the root, or for an anonymous block (whose content's
    /// elements are its container's children, or a split inline's pieces) the nearest ancestor that
    /// is not an inline split around block-level boxes (the pieces are inside the first-line box).
    pub fn first_line_parent(&self, root: NodeId) -> NodeId {
        let Some(node) = self.nodes.get(root).filter(|node| node.is_anonymous()) else {
            return root;
        };
        let mut parent = node.parent;
        while let Some(id) = parent {
            match self.nodes.get(id) {
                Some(node) if node.is_split_inline() => parent = node.parent,
                Some(_) => return id,
                None => break,
            }
        }
        root
    }

    /// pyro: the style `node` (an element, or the text of one, in the inline content of `root`)
    /// has on the first line of `root`, whose `::first-line` style node is `first_line`
    /// (`TextLayout::first_line`): its own style reparented onto the first-line box (and through
    /// the elements between them, reparented in turn), as Gecko's `ReparentStyle` does: the
    /// inherited properties come from the new parent unless the element sets them, the others are
    /// its own. `first_line` itself has its style; `root` and the element the box is in
    /// ([`Self::first_line_parent`]), outside it, and nodes that are not in its content have none.
    /// `cache` keeps the styles found for one paint of the line (pass the same one for the runs of
    /// one root); a style cascaded again is kept across paints while the element's own style and
    /// its new parent's are the same (`ReparentCache`).
    pub fn first_line_style(
        &self,
        root: NodeId,
        first_line: NodeId,
        node: NodeId,
        cache: &mut Vec<(NodeId, Arc<ComputedValues>)>,
    ) -> Option<Arc<ComputedValues>> {
        if node == first_line {
            return self.nodes.get(first_line)?.primary_styles().map(|style| (*style).clone());
        }
        let outer = self.first_line_parent(root);
        if node == root || node == outer {
            return None;
        }
        if let Some((_, style)) = cache.iter().find(|(id, _)| *id == node) {
            return Some(style.clone());
        }
        let current = self.nodes.get(node)?;
        let parent = current.parent?;
        let parent_style = match parent == root || parent == outer {
            true => (*self.nodes.get(first_line)?.primary_styles()?).clone(),
            false => crate::util::ensure_stack(|| self.first_line_style(root, first_line, parent, cache))?,
        };
        let style = match current.primary_styles() {
            Some(own) if current.is_element() || current.is_anonymous() => {
                let own: Arc<ComputedValues> = (*own).clone();
                // pyro: for a canvas a page can read back (`UnvisitedPaint`), the style is
                // cascaded again with the visited rules of its own style, so its colours are
                // read unvisited from the result; the cache holds only what the page shows.
                if crate::node::UnvisitedPaint::active() {
                    crate::node::UnvisitedPaint::view(reparent(self, current, &own, &parent_style))
                } else {
                    let kept = self.first_line_reparented.0.lock().unwrap().get(&node).and_then(|entry| {
                        (Arc::ptr_eq(&entry.own, &own) && Arc::ptr_eq(&entry.parent, &parent_style))
                            .then(|| entry.style.clone())
                    });
                    match kept {
                        Some(style) => style,
                        None => {
                            let style = reparent(self, current, &own, &parent_style);
                            let entry = ReparentEntry { own, parent: parent_style, style: style.clone() };
                            self.first_line_reparented.0.lock().unwrap().insert(node, entry);
                            style
                        }
                    }
                }
            }
            // Text inherits all of its style.
            _ => parent_style,
        };
        cache.push((node, style.clone()));
        Some(style)
    }
}

/// pyro: the styles `BaseDocument::first_line_style` cascaded again, by element, with the own
/// and parent styles they were cascaded from (held, so that the same address is the same style):
/// a paint reuses one while both are the same. An entry of an element that left the first line
/// stays until the element's styles change; the cache is dropped with the document's
/// `::first-line` styles' changes (`style_changed`).
#[derive(Default)]
pub struct ReparentCache(std::sync::Mutex<std::collections::HashMap<NodeId, ReparentEntry>>);

struct ReparentEntry {
    own: Arc<ComputedValues>,
    parent: Arc<ComputedValues>,
    style: Arc<ComputedValues>,
}

/// `own` (the style of `node`) cascaded again with `parent` as its parent, its reset properties
/// kept (Stylo's `FirstLineReparenting`).
fn reparent(
    doc: &BaseDocument,
    node: &Node,
    own: &Arc<ComputedValues>,
    parent: &ComputedValues,
) -> Arc<ComputedValues> {
    let read_guard = doc.guard.read();
    let guards = StylesheetGuards::same(&read_guard);
    let inputs = CascadeInputs::new_from_style(own);
    let pseudo = own.pseudo();
    doc.stylist.cascade_style_and_visited(
        Some(node),
        pseudo.as_ref(),
        &inputs,
        &guards,
        Some(parent),
        Some(parent),
        FirstLineReparenting::Yes { style_to_reparent: own },
        &PositionTryFallbacksTryTactic::default(),
        None,
        &mut RuleCacheConditions::default(),
        &mut TreeCountingCaches::default(),
    )
}
