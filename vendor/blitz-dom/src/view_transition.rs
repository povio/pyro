use crate::{BaseDocument, ElementData, NodeData, NodeId};
use markup5ever::{QualName, local_name, ns};
use style::context::CascadeInputs;
use style::properties::{ComputedValues, PropertyDeclarationBlock};
use style::rule_tree::{CascadeLevel, CascadeOrigin};
use style::selector_parser::PseudoElement;
use style::servo_arc::Arc;
use style::shared_lock::{Locked, StylesheetGuards};
use style::stylesheets::layer_rule::LayerOrder;

pub struct Participant {
    pub name: String,
    pub target: Option<NodeId>,
    pub old: Option<Geometry>,
    pub new: Option<Geometry>,
}

#[derive(Default)]
pub struct PaintState {
    pub roots: style::FxHashMap<NodeId, NodeId>,
    pub suppressed: style::FxHashSet<NodeId>,
    pub images: style::FxHashMap<NodeId, Raster>,
}

pub struct Raster {
    pub pixels: std::sync::Arc<Vec<u8>>,
    pub size: (f64, f64),
    pub ink: [f64; 4],
}

#[derive(Clone, Copy)]
pub struct Geometry {
    pub size: (f64, f64),
    pub transform: [f64; 6],
}

impl Geometry {
    fn declarations(self, scale: f64) -> String {
        let [a, b, c, d, x, y] = self.transform;
        format!(
            "width:{}px;height:{}px;transform:matrix({a},{b},{c},{d},{},{});",
            self.size.0 / scale,
            self.size.1 / scale,
            (x + ((a-1.)*self.size.0 + c*self.size.1)*0.5) / scale,
            (y + (b*self.size.0 + (d-1.)*self.size.1)*0.5) / scale
        )
    }
}

struct BoxNode {
    samples: Vec<(usize,f64)>,
    public_pseudo: String,
    id: NodeId,
    pseudo: PseudoElement,
    declarations: Option<Arc<Locked<PropertyDeclarationBlock>>>,
}

pub(crate) struct Tree {
    nodes: Vec<BoxNode>,
    live_participants: Vec<NodeId>,
    group_frames: style::FxHashMap<NodeId, (style::Atom, String)>,
}

impl BaseDocument {
    /// pyro: whether element `id` can take layout containment (css-contain-2 § 3.2), as the scope
    /// of an element-scoped view transition must (css-view-transitions-2; else its transition is
    /// skipped with an InvalidStateError, as in Chrome): it has a box (not `display: none` or
    /// `contents`) that is no non-atomic inline box and no internal table box but a cell.
    pub fn layout_containment_eligible(&self, id: NodeId) -> bool {
        use style::values::specified::box_::{DisplayInside, DisplayOutside};
        let Some(node) = self.get_node(id) else { return false };
        let Some(styles) = node.primary_styles() else { return false };
        let display = styles.get_box().clone_display();
        if display.is_none() || display.is_contents() {
            return false;
        }
        match display.outside() {
            DisplayOutside::Inline => {
                display.inside() != DisplayInside::Flow
                    || node.data.downcast_element().is_some_and(|el| {
                        crate::layout::replaced::is_replaced(el) || crate::layout::replaced::is_inline_box(el)
                    })
            }
            DisplayOutside::InternalTable => display.inside() == DisplayInside::TableCell,
            DisplayOutside::None => false,
            _ => true,
        }
    }

    pub fn activate_transition_paint(&mut self, origin: NodeId, images: Vec<Option<Raster>>) -> bool {
        use PseudoElement::*;
        let Some(tree) = self.transition_trees.as_ref().and_then(|trees| trees.get(&origin)) else { return false };
        let mut images = images.into_iter();
        let mut bindings = Vec::new();
        for item in &tree.nodes {
            if !matches!(item.pseudo, ViewTransitionOld(_) | ViewTransitionNew(_)) { continue; }
            let Some(image) = images.next() else { return false; };
            if let Some(image) = image { bindings.push((item.id, image)); }
            else if matches!(item.pseudo, ViewTransitionNew(_)) {
                let mut ancestor = Some(item.id);
                let mut visible = true;
                while let Some(id) = ancestor.filter(|id| *id != origin) {
                    let node = &self.nodes[id];
                    if node.taffy_display() == taffy::Display::None { visible = false; break; }
                    ancestor = node.parent;
                }
                if visible { return false; }
            }
        }
        let root = tree.nodes[0].id;
        let paint = self.transition_paint.get_or_insert_with(Default::default);
        paint.suppressed.extend(tree.live_participants.iter().copied());
        paint.roots.insert(origin, root);
        paint.images.extend(bindings);
        self.resolve_transition_transforms(root);
        self.build_transition_paint_tree(root);
        true
    }
    pub fn transition_animation_styles(&self, origin: NodeId) -> Option<Vec<(usize, String, Arc<ComputedValues>)>> {
        let tree = self.transition_trees.as_ref()?.get(&origin)?;
        let mut result = Vec::new();
        let mut remaining = 1024usize;
        let mut ancestry = 16384usize;
        for &target in &tree.live_participants {
            let mut node = self.nodes.get(target)?;
            let mut top = 0.0;
            let height = node.final_layout().size.height;
            while let Some(parent) = node.layout_parent.get() {
                ancestry = ancestry.checked_sub(1)?;
                top += node.final_layout().location.y;
                node = self.nodes.get(parent)?;
                if let Some(columns) = self.column_fragments(parent) {
                    let first = columns.fragments.partition_point(|fragment| fragment.end <= top);
                    let last = columns.fragments.partition_point(|fragment| fragment.start < top + height);
                    if last > first + 1 { return None; }
                }
            }
        }
        for (index,item) in tree.nodes.iter().enumerate() {
            let style = self.nodes.get(item.id)?.primary_styles()?;
            let mut animated = false;
            for name in style.get_ui().animation_name_iter() {
                remaining = remaining.checked_sub(1)?;
                animated |= name.as_atom().is_some();
            }
            if animated { result.push((index,item.public_pseudo.clone(), style.clone())); }
        }
        Some(result)
    }
    pub fn set_transition_samples(&mut self, origin: NodeId, values: &[(usize,usize,f64)]) -> bool {
        let Some(tree) = self.transition_trees.as_mut().and_then(|trees|trees.get_mut(&origin)) else { return false; };
        if values.len()>1024 { return false; }
        for item in &mut tree.nodes { item.samples.clear(); }
        for &(index,animation,progress) in values {
            let Some(node) = tree.nodes.get_mut(index) else { return false; };
            if !progress.is_finite() || animation >=1024 { return false; }
            node.samples.push((animation,progress));
        }
        self.refresh_transition_scope(Some(origin))
    }
    pub fn clear_transition_tree(&mut self, origin: NodeId) {
        let Some(trees) = self.transition_trees.as_mut() else {
            return;
        };
        let removed = trees.remove(&origin);
        if trees.is_empty() {
            self.transition_trees = None;
        }
        if let Some(tree) = removed {
            for node in tree.nodes.into_iter().rev() {
                self.remove_node_from_tree(node.id);
            }
        }
    }

    pub fn create_transition_tree(&mut self, origin: NodeId, participants: &[Participant]) -> Option<usize> {
        use PseudoElement::*;
        if participants.len() > 512
            || self.transition_trees.as_ref().is_some_and(|trees| trees.len() >= 16 || trees.contains_key(&origin))
        {
            return None;
        }
        let scale = self.viewport().scale_f64();
        let mut descriptors = vec![(ViewTransition, 0, String::new(), String::from("::view-transition"))];
        let mut generated = Vec::new();
        for part in participants {
            let classes = part
                .target
                .and_then(|id| self.nodes.get(id))
                .and_then(|node| {
                    node.primary_styles().map(|style| {
                        style
                            .get_view_transition_class()
                            .value
                            .iter()
                            .take(33)
                            .map(|item| item.0.clone())
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            let arg = PseudoElement::transition_box_argument(&part.name, &classes)?;
            let geometry = part.new.or(part.old)?;
            let mut css = geometry.declarations(scale);
            if let Some(target) = part.target {
                let filter = self.resolved_style_value(target, "backdrop-filter");
                if !filter.is_empty() && filter != "none" { css.push_str("backdrop-filter:"); css.push_str(&filter); css.push(';'); }
            }
            let both = part.old.is_some() && part.new.is_some();
            if both {
                css.push_str("animation-name:-ua-view-transition-group-anim-");
                cssparser::serialize_identifier(&part.name, &mut css).ok()?;
                generated.push((
                    descriptors.len(),
                    style::Atom::from(format!("-ua-view-transition-group-anim-{}", part.name)),
                    part.old?.declarations(scale),
                ));
            }
            let group = descriptors.len();
            let mut ident = String::new();
            cssparser::serialize_identifier(&part.name, &mut ident).ok()?;
            descriptors.push((ViewTransitionGroup(arg.clone()), 0, css, format!("::view-transition-group({ident})")));
            descriptors.push((
                ViewTransitionPair(arg.clone()),
                group,
                if both { "isolation:isolate".into() } else { String::new() },
                format!("::view-transition-image-pair({ident})"),
            ));
            for (exists, pseudo, fade) in [
                (part.old.is_some(), ViewTransitionOld(arg.clone()), "out"),
                (part.new.is_some(), ViewTransitionNew(arg.clone()), "in"),
            ] {
                if exists {
                    let intrinsic = if fade == "out" { part.old? } else { part.new? };
                    descriptors.push((
                        pseudo,
                        group + 1,
                        format!(
                            "aspect-ratio:{} / {};animation-name:-ua-view-transition-fade-{fade}{}",
                            intrinsic.size.0, intrinsic.size.1,
                            if both { ",-ua-mix-blend-mode-plus-lighter" } else { "" }
                        ),
                        format!("::view-transition-{}({ident})", if fade == "out" {"old"} else {"new"}),
                    ));
                }
            }
        }
        let mut nodes = Vec::<BoxNode>::with_capacity(descriptors.len());
        for (pseudo, parent, css, public_pseudo) in descriptors {
            let parent = nodes.get(parent).map_or(origin, |node| node.id);
            let declarations = (!css.is_empty()).then(|| {
                Arc::new(self.guard.wrap(style::properties::declaration_block::parse_style_attribute(
                    &css,
                    &self.url.url_extra_data(),
                    None,
                    self.stylist.quirks_mode(),
                    style::stylesheets::CssRuleType::Style,
                )))
            });
            let Some(style) = self.transition_cascade(origin, parent, pseudo.clone(), declarations.as_ref()) else {
                for node in nodes.into_iter().rev() {
                    self.remove_node_from_tree(node.id);
                }
                return None;
            };
            let element = ElementData::new(QualName::new(None, ns!(html), local_name!("div")), Vec::new());
            let id = self.create_node(NodeData::AnonymousBlock(Box::new(element)));
            self.nodes[id].parent = Some(parent);
            if parent != origin {
                self.nodes[parent].children.push(id);
                self.nodes[parent].layout_children.borrow_mut().get_or_insert_with(Default::default).push(id);
                self.nodes[id].layout_parent.set(Some(parent));
                self.nodes[id].layout_child_of.set(Some(parent));
            }
            self.nodes[id].stylo_element_data_mut().ensure_init_mut().styles.primary = Some(style);
            self.nodes[id].update_layout_styles();
            nodes.push(BoxNode { id, pseudo, declarations, samples:Vec::new(), public_pseudo });
        }
        let count = nodes.len();
        let group_frames = generated.into_iter().map(|(index, name, css)| (nodes[index].id, (name, css))).collect();
        self.layout_transition_tree(origin, nodes[0].id);
        let live_participants = participants.iter().filter(|part| part.new.is_some()).filter_map(|part| part.target).collect();
        self.transition_trees.get_or_insert_with(Default::default).insert(origin, Tree { nodes, group_frames, live_participants });
        Some(count)
    }

    fn layout_transition_tree(&mut self, origin: NodeId, root: NodeId) {
        let size = if self.try_root_element().is_some_and(|node| node.id == origin) {
            let viewport = self.viewport();
            let scale = viewport.scale_f64() as f32;
            taffy::Size { width: viewport.window_size.0 as f32 / scale, height: viewport.window_size.1 as f32 / scale }
        } else { self.nodes[origin].final_layout().size };
        let space = size.map(taffy::AvailableSpace::Definite);
        let mut state = crate::layout::LayoutPassState::new(self);
        #[cfg(feature = "writing-mode")]
        let space = {
            state.layout_wm = state.layout_wm_of(root);
            if state.layout_wm.is_vertical() { space.transpose() } else { space }
        };
        let root = crate::taffy_node_id(root);
        taffy::compute_root_layout_in(&mut state, root, space, space);
        #[cfg(feature = "writing-mode")]
        state.physicalise_and_round_layout(root);
        #[cfg(not(feature = "writing-mode"))]
        taffy::round_layout(&mut state, root);
    }

    pub(crate) fn transition_group_from(&self, group: NodeId, name: &style::Atom) -> Option<&str> {
        let trees = self.transition_trees.as_ref()?;
        let root = self.nodes.get(group)?.parent?;
        let origin = self.nodes.get(root)?.parent?;
        let (expected, css) = trees.get(&origin)?.group_frames.get(&group)?;
        (expected == name).then_some(css)
    }

    fn transition_cascade(
        &self,
        origin: NodeId,
        parent: NodeId,
        pseudo: PseudoElement,
        block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
    ) -> Option<Arc<ComputedValues>> {
        let base = self.pseudo_element_style(origin, &pseudo, false)?;
        let _thread = crate::stylo::LayoutThreadScope::enter();
        let parent = self.nodes.get(parent)?.primary_styles()?;
        let guard = self.guard.read();
        let guards = StylesheetGuards::same(&guard);
        let rules = block
            .and_then(|block| {
                self.stylist.rule_tree().update_rule_at_level(
                    CascadeLevel::new(CascadeOrigin::UA),
                    LayerOrder::style_attribute(),
                    Some(block.borrow_arc()),
                    base.rules(),
                    &guards,
                    &mut false,
                )
            })
            .unwrap_or_else(|| base.rules().clone());
        Some(self.stylist.compute_pseudo_element_style_with_inputs(
            CascadeInputs { rules: Some(rules), ..Default::default() },
            &pseudo,
            &guards,
            Some(&parent),
            Some(&self.nodes[origin]),
        ))
    }

    pub fn transition_box_style(&self, origin: NodeId, pseudo: &PseudoElement) -> Option<Arc<ComputedValues>> {
        let tree = self.transition_trees.as_ref()?.get(&origin)?;
        let node = tree.nodes.iter().find(|node| node.pseudo.matches_transition_selector(pseudo))?;
        self.nodes.get(node.id)?.primary_styles().map(|style| style.clone())
    }

    pub(crate) fn refresh_transition_trees(&mut self) {
        self.refresh_transition_scope(None);
    }

    fn refresh_transition_scope(&mut self, selected: Option<NodeId>) -> bool {
        let Some(trees) = self.transition_trees.take() else {
            return false;
        };
        let mut complete = true;
        for (origin, tree) in trees.iter() {
            if selected.is_some_and(|scope| scope != *origin) { continue; }
            let mut budget = 4096;
            for item in &tree.nodes {
                if let Some(parent) = self.nodes.get(item.id).and_then(|node| node.parent)
                    && let Some(style) =
                        self.transition_cascade(*origin, parent, item.pseudo.clone(), item.declarations.as_ref())
                {
                    let Some(style) = self.sample_transition_style(*origin,parent,item.pseudo.clone(),style,&item.samples,tree.group_frames.get(&item.id),&mut budget) else { complete = false; break; };
                    self.nodes[item.id].stylo_element_data_mut().ensure_init_mut().styles.primary = Some(style);
                    self.nodes[item.id].update_layout_styles();
                    self.nodes[item.id].clear_layout_cache();
                }
            }
            self.layout_transition_tree(*origin, tree.nodes[0].id);
        }
        self.transition_trees = Some(trees);
        complete
    }
}
