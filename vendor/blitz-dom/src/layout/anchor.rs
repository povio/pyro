use crate::{BaseDocument, Node};
use stylo_taffy::{CopyStyleGetters, WritingModeExt};
use euclid::default::{Point3D, Transform3D};

fn projected_bounds(matrix: &Transform3D<f64>, origin: taffy::Point<f32>, size: taffy::Size<f32>) -> Option<taffy::Rect<f32>> {
    let mut bounds = taffy::Rect { left: f32::INFINITY, right: f32::NEG_INFINITY, top: f32::INFINITY, bottom: f32::NEG_INFINITY };
    for x in [origin.x, origin.x + size.width] {
        for y in [origin.y, origin.y + size.height] {
            let point = matrix.transform_point3d(Point3D::new(x.into(), y.into(), 0.0))?;
            bounds.left = bounds.left.min(point.x as f32);
            bounds.right = bounds.right.max(point.x as f32);
            bounds.top = bounds.top.min(point.y as f32);
            bounds.bottom = bounds.bottom.max(point.y as f32);
        }
    }
    Some(bounds)
}

impl Node {
    pub fn hidden_by_anchor(&self) -> bool {
        if self.anchor_visibility_target.get().is_none() {return self.anchor_hidden.get() || self.anchor_invalid.get();}
        let mut target = self;
        let tree = self.tree();
        let generation = tree.geometry_generation();
        let mut cache = tree.anchor_visibility.lock().unwrap();
        if cache.0 != generation { cache.1.clear(); cache.0 = generation; }
        let mut path = Vec::new();
        let hidden = 'chain: loop {
            if let Some(&cached) = cache.1.get(&target.id) { break cached; }
            if path.len() >= tree.len() { break false; }
            path.push(target.id);
            tree.anchor_clip_steps.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
            if target.anchor_hidden.get() || target.anchor_invalid.get() {
                break true;
            }
            let Some(anchor) = target.anchor_visibility_target.get().and_then(|id| tree.get(id)) else { break false };
            if anchor
                .primary_styles()
                .is_some_and(|s| s.clone_visibility() != style::computed_values::visibility::T::Visible)
            {
                break true;
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
                tree.anchor_clip_steps.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
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
                            break 'chain true;
                        }
                    }
                    if style.clone_overflow_y() != Overflow::Visible {
                        y = y.max(layout.border.top);
                        bottom = bottom.min(layout.size.height - layout.border.bottom);
                        if y >= bottom && anchor.final_layout().size.height > 0.0 {
                            break 'chain true;
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
        };
        for id in path { cache.1.insert(id,hidden); }
        hidden
    }
}

impl BaseDocument {
    pub(super) fn anchor_placer_mode(&self, node: &Node) -> stylo_taffy::WritingMode {
        node.containing_block().and_then(|id| self.nodes[id].primary_styles()).map_or(stylo_taffy::WritingMode::empty(), |style| style.writing_mode)
    }

    pub(super) fn anchor_physical_size(&self, node: &Node) -> taffy::Size<f32> {
        self.anchor_placer_mode(node).logical_size(node.unrounded_layout().size)
    }

    fn anchor_physical_layout(&self, node: &Node, placer: &Node) -> taffy::Layout {
        let raw = node.unrounded_layout();
        let Some(style) = placer.primary_styles() else { return *raw };
        let wm = style.writing_mode;
        if !wm.swaps_axes() { return *raw; }
        let x = if wm.is_vertical_lr() { raw.location.y } else { self.anchor_physical_size(placer).width - raw.location.y - raw.size.height };
        taffy::Layout { location: taffy::Point { x, y: raw.location.x }, size: raw.size.transpose(), border: wm.physical_rect(raw.border), padding: wm.physical_rect(raw.padding), ..*raw }
    }

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
        let (_, _, extent) = if func.target_element.value.0.is_empty() {
            self.default_anchor_rect(node, container, style)
        } else {
            self.anchor_rect(node, container, &func.target_element.value, func.target_element.scope)
        }?;
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
    pub(super) fn default_anchor_rect(&self, node: &Node, container: &Node, style: &style::properties::ComputedValues) -> Option<(crate::NodeId, taffy::Point<f32>, taffy::Size<f32>)> {
        use style::values::specified::position::PositionAnchorKeyword;
        let pos = style.get_position();
        match &pos.position_anchor.value {
            PositionAnchorKeyword::Ident(name) => self.anchor_rect(node,container,name,pos.position_anchor.scope),
            PositionAnchorKeyword::Auto | PositionAnchorKeyword::Normal if !pos.position_area.is_none() || matches!(pos.position_anchor.value,PositionAnchorKeyword::Auto) => {
                let source = *self.popover_anchors.get(&node.id)?;
                self.nodes.get(source)?;
                self.anchor_lookup_visits.set(self.anchor_lookup_visits.get().saturating_add(1));
                self.anchor_geometry(node,container,source,Some(node.id))
            }
            _ => None,
        }
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
            let id = self.anchor_index.borrow_mut().lookup(self, key, origin, node.id, container.id)?;
            visit();
            id
        } else {
            let id = *candidates.iter().next()?;
            visit();
            let boundary = |start| std::iter::successors(Some(start), |&id| self.nodes[id].parent)
                .take(self.nodes.len()).find(|&id| is_boundary(id));
            if boundary(id) != boundary(origin) { return None; }
            id
        };
        self.anchor_geometry(node, container, id, (!indexed).then_some(origin))
    }

    fn anchor_geometry(&self, node: &Node, container: &Node, id: crate::NodeId, mut check_order: Option<crate::NodeId>) -> Option<(crate::NodeId, taffy::Point<f32>, taffy::Size<f32>)> {
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
        let mut extent = self.anchor_physical_size(anchor);
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
        let mut matrix = Transform3D::identity();
        let mut strip = point;
        for _ in 0..self.nodes.len() {
            if cursor == container.id || cursor == node.id {
                break;
            }
            let current = &self.nodes[cursor];
            if current.is_out_of_flow() && let Some(origin) = check_order.take() && !self.compare_document_order(id, origin).is_lt() { return None; }
            let parent = if current.is_out_of_flow() { current.containing_block() } else { current.layout_parent.get() };
            let Some(parent) = parent else { break };
            self.anchor_geometry_steps.set(self.anchor_geometry_steps.get().saturating_add(1));
            let layout = &self.anchor_physical_layout(current, &self.nodes[parent]);
            if self.anchor_transform_sources.borrow_mut().entry(container.id).or_default().insert(cursor)
                && let Some(style) = current.primary_styles() {
                self.anchor_transform_users.borrow_mut().entry(cursor).or_insert_with(|| (style.clone(), Default::default())).1.insert(container.id);
            }
            if current.has_transform_style() {
                use crate::stylo_to_kurbo::{ResolvedTransform, resolve_transform};
                let style = current.primary_styles()?;
                let box_style = style.get_box();
                let reference = Node::layout_transform_reference_box(layout, box_style.transform_box);
                match resolve_transform(box_style, reference) {
                    ResolvedTransform::Spatial(own) => matrix = matrix.then(&own),
                    ResolvedTransform::Flat(Some(own)) => {
                        let [a, b, c, d, e, f] = own.as_coeffs();
                        matrix = matrix.then(&Transform3D::new(a,b,0.,0.,c,d,0.,0.,0.,0.,1.,0.,e,f,0.,1.));
                    }
                    ResolvedTransform::Flat(None) => (),
                }
            }
            matrix = matrix.then_translate(euclid::vec3(layout.location.x.into(), layout.location.y.into(), 0.));
            let parent_node = &self.nodes[parent];
            if let Some(style) = parent_node.primary_styles() {
                let box_style = style.get_box();
                if !matches!(box_style.perspective, style::values::generics::box_::GenericPerspective::None) {
                    let reference = Node::layout_transform_reference_box(parent_node.unrounded_layout(), box_style.transform_box);
                    if let Some(perspective) = crate::stylo_to_kurbo::resolve_perspective_matrix(box_style, reference) { matrix = matrix.then(&perspective); }
                }
            }
            if !parent_node.is_preserve_3d() { matrix = matrix.then_scale(1.,1.,0.); }
            strip.x += layout.location.x;
            strip.y += layout.location.y;
            if let Some(columns) = self.nodes[parent].column_fragments() {
                let bottom = strip.y + extent.height;
                let pieces = columns.hoisted_columns(strip.y, bottom).filter_map(|fragment| {
                    let start = strip.y.max(fragment.start);
                    let rect = projected_bounds(&matrix, taffy::Point { x: point.x, y: point.y + (start - strip.y) }, taffy::Size { width: extent.width, height: bottom.min(fragment.end) - start })?;
                    Some(taffy::Rect { left: rect.left + fragment.offset.x, right: rect.right + fragment.offset.x, top: rect.top + fragment.offset.y, bottom: rect.bottom + fragment.offset.y })
                });
                if let Some(rect) = pieces.reduce(|a, b| taffy::Rect {
                    left: a.left.min(b.left),
                    right: a.right.max(b.right),
                    top: a.top.min(b.top),
                    bottom: a.bottom.max(b.bottom),
                }) {
                    point = taffy::Point { x: rect.left, y: rect.top };
                    extent = taffy::Size { width: rect.right - rect.left, height: rect.bottom - rect.top };
                    strip = point;
                    matrix = Transform3D::identity();
                }
            }
            cursor = parent;
        }
        if cursor != container.id {
            return None;
        }
        let rect = projected_bounds(&matrix, point, extent)?;
        Some((id, taffy::Point { x: rect.left, y: rect.top }, taffy::Size { width: rect.right - rect.left, height: rect.bottom - rect.top }))
    }
}
