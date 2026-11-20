use crate::{BaseDocument, Node};
use stylo_taffy::CopyStyleGetters;

impl Node {
    pub fn hidden_by_anchor(&self) -> bool {
        if self.anchor_visibility_target.get().is_none() {return self.anchor_hidden.get() || self.anchor_invalid.get();}
        let mut target = self;
        let tree = self.tree();
        for _ in 0..tree.len() {
            if target.anchor_hidden.get() || target.anchor_invalid.get() {
                return true;
            }
            let Some(anchor) = target.anchor_visibility_target.get().and_then(|id| tree.get(id)) else { return false };
            if anchor
                .primary_styles()
                .is_some_and(|s| s.clone_visibility() != style::computed_values::visibility::T::Visible)
            {
                return true;
            }
            let layout = anchor.final_layout();
            let (mut x, mut y) = (layout.location.x, layout.location.y);
            let mut right = x + layout.size.width;
            let mut bottom = y + layout.size.height;
            let mut cursor = anchor.containing_block();
            for _ in 0..tree.len() {
                if cursor == target.containing_block() {
                    break;
                }
                let Some(parent) = cursor.and_then(|id| tree.get(id)) else { break };
                let layout = parent.final_layout();
                let scroll = parent.scroll_offset();
                x -= scroll.x as f32;
                right -= scroll.x as f32;
                y -= scroll.y as f32;
                bottom -= scroll.y as f32;
                if let Some(style) = parent.primary_styles() {
                    use style::values::computed::Overflow;
                    if style.clone_overflow_x() != Overflow::Visible {
                        x = x.max(layout.border.left);
                        right = right.min(layout.size.width - layout.border.right);
                        if x >= right && anchor.final_layout().size.width > 0.0 {
                            return true;
                        }
                    }
                    if style.clone_overflow_y() != Overflow::Visible {
                        y = y.max(layout.border.top);
                        bottom = bottom.min(layout.size.height - layout.border.bottom);
                        if y >= bottom && anchor.final_layout().size.height > 0.0 {
                            return true;
                        }
                    }
                }
                x += layout.location.x;
                right += layout.location.x;
                y += layout.location.y;
                bottom += layout.location.y;
                cursor = parent.containing_block();
            }
            target = anchor;
        }
        false
    }
}

impl BaseDocument {
    pub(super) fn uses_anchor_queries(style: &style::properties::ComputedValues) -> bool {
        use style::values::computed::length::{Margin, MaxSize, Size};
        use style::values::computed::position::Inset;
        use style::values::specified::align::AlignFlags;
        let pos = style.get_position();
        let margin = style.get_margin();
        !pos.position_area.is_none()
            || !pos.position_try_fallbacks.value.is_none()
            || [pos.justify_self.0, pos.align_self.0].iter().any(|v| v.value() == AlignFlags::ANCHOR_CENTER)
            || [&pos.left, &pos.top, &pos.right, &pos.bottom].iter().any(|v| {
                matches!(
                    v,
                    Inset::AnchorFunction(_) | Inset::AnchorSizeFunction(_) | Inset::AnchorContainingCalcFunction(_)
                )
            })
            || [&pos.width, &pos.height, &pos.min_width, &pos.min_height]
                .iter()
                .any(|v| matches!(v, Size::AnchorSizeFunction(_) | Size::AnchorContainingCalcFunction(_)))
            || [&pos.max_width, &pos.max_height]
                .iter()
                .any(|v| matches!(v, MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_)))
            || [&margin.margin_top, &margin.margin_right, &margin.margin_bottom, &margin.margin_left]
                .iter()
                .any(|v| matches!(v, Margin::AnchorSizeFunction(_) | Margin::AnchorContainingCalcFunction(_)))
    }
    pub(crate) fn anchor_calc_size(
        &self,
        value: &style::values::computed::LengthPercentage,
        node: &Node,
        container: &Node,
        horizontal: bool,
        style: &style::properties::ComputedValues,
    ) -> Option<style::values::computed::LengthPercentage> {
        use style::values::computed::length_percentage::{CalcNode, Unpacked};
        let Unpacked::Calc(calc) = value.unpack() else { return Some(value.clone()) };
        calc.substitute_anchors(|term| match term {
            CalcNode::AnchorSize(func) => self.anchor_dimension(func, node, container, horizontal, style),
            _ => None,
        })
    }

    pub(crate) fn anchor_dimension<Fallback>(
        &self,
        func: &style::values::generics::length::GenericAnchorSizeFunction<Fallback>,
        node: &Node,
        container: &Node,
        horizontal: bool,
        style: &style::properties::ComputedValues,
    ) -> Option<f32> {
        use style::values::generics::length::AnchorSizeKeyword as Axis;
        use style::values::specified::position::PositionAnchorKeyword;
        let default = &style.get_position().position_anchor;
        let (name, level) = if func.target_element.value.0.is_empty() {
            let PositionAnchorKeyword::Ident(name) = &default.value else { return None };
            (name, default.scope)
        } else {
            (&func.target_element.value, func.target_element.scope)
        };
        let (_, _, extent) = self.anchor_rect(node, container, name, level)?;
        let width = match func.size {
            Axis::None => horizontal,
            Axis::Width => true,
            Axis::Height => false,
            Axis::Block => container.primary_styles()?.writing_mode.is_vertical(),
            Axis::Inline => !container.primary_styles()?.writing_mode.is_vertical(),
            Axis::SelfBlock => style.writing_mode.is_vertical(),
            Axis::SelfInline => !style.writing_mode.is_vertical(),
        };
        Some(if width { extent.width } else { extent.height })
    }
    pub(super) fn anchor_rect(
        &self,
        node: &Node,
        container: &Node,
        name: &style::values::DashedIdent,
        level: style::rule_tree::CascadeLevel,
    ) -> Option<(crate::NodeId, taffy::Point<f32>, taffy::Size<f32>)> {
        let origin = node.scroll_pseudo().map_or(node.id, |(_, id)| id);
        let scope = level.get_shadow_root_for_scoped(&self.nodes[origin]).map(|root| root.id);
        let key = (scope, name.clone());
        let candidates = self.named_anchor_nodes.get(&key)?;
        let visit = || self.anchor_lookup_visits.set(self.anchor_lookup_visits.get().saturating_add(1));
        let is_boundary = |id: crate::NodeId| {
                let current = &self.nodes[id];
                current.primary_styles().is_some_and(|s| {
                    let boundary = &s.get_box().anchor_scope;
                    boundary.value.includes(name)
                        && boundary.scope.get_shadow_root_for_scoped(current).map(|root| root.id) == scope
                })
        };
        let indexed = candidates.len() > 1;
        let id = if indexed {
            let id = self.anchor_index.borrow_mut().lookup(self, key, origin, node.id, container.id, is_boundary)?;
            visit();
            id
        } else {
            let id = *candidates.iter().next()?;
            visit();
            let boundary = |start| std::iter::successors(Some(start), |&id| self.nodes[id].parent)
                .take(self.nodes.len()).find(|&id| is_boundary(id));
            if boundary(id) != boundary(origin) || (self.nodes[id].is_out_of_flow() && !self.compare_document_order(id, origin).is_lt()) { return None; }
            id
        };
        self.anchor_geometry(node, container, id)
    }

    pub(super) fn anchor_geometry(&self, node: &Node, container: &Node, id: crate::NodeId) -> Option<(crate::NodeId, taffy::Point<f32>, taffy::Size<f32>)> {
        let anchor = &self.nodes[id];
        if id == node.id || (!anchor.flags.is_in_document() && !anchor.is_anonymous()) {
            return None;
        }
        let anchor_style = anchor.primary_styles()?;
        if anchor_style.get_box().display.is_none() {
            return None;
        }
        let mut cursor = id;
        let mut point = taffy::Point::ZERO;
        let mut extent = anchor.unrounded_layout().size;
        if let Some(fragments) = anchor.inline_fragment_boxes() {
            let Some(rect) = fragments.reduce(|a, b| taffy::Rect {
                left: a.left.min(b.left),
                right: a.right.max(b.right),
                top: a.top.min(b.top),
                bottom: a.bottom.max(b.bottom),
            }) else {
                return None;
            };
            point = taffy::Point { x: rect.left, y: rect.top };
            extent = taffy::Size { width: rect.right - rect.left, height: rect.bottom - rect.top };
            cursor = anchor.inline_root_ancestor()?.id;
        }
        for _ in 0..self.nodes.len() {
            if cursor == container.id || cursor == node.id {
                break;
            }
            let current = &self.nodes[cursor];
            let parent = if current.is_out_of_flow() { current.containing_block() } else { current.layout_parent.get() };
            let Some(parent) = parent else { break };
            point.x += current.unrounded_layout().location.x;
            point.y += current.unrounded_layout().location.y;
            if let Some(columns) = self.nodes[parent].column_fragments() {
                let bottom = point.y + extent.height;
                let pieces = columns.hoisted_columns(point.y, bottom).map(|fragment| taffy::Rect {
                    left: point.x + fragment.offset.x,
                    right: point.x + extent.width + fragment.offset.x,
                    top: point.y.max(fragment.start) + fragment.offset.y,
                    bottom: bottom.min(fragment.end) + fragment.offset.y,
                });
                if let Some(rect) = pieces.reduce(|a, b| taffy::Rect {
                    left: a.left.min(b.left),
                    right: a.right.max(b.right),
                    top: a.top.min(b.top),
                    bottom: a.bottom.max(b.bottom),
                }) {
                    point = taffy::Point { x: rect.left, y: rect.top };
                    extent = taffy::Size { width: rect.right - rect.left, height: rect.bottom - rect.top };
                }
            }
            cursor = parent;
        }
        if cursor != container.id {
            return None;
        }
        Some((id, point, extent))
    }
}
