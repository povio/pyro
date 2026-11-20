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
            x / scale,
            y / scale
        )
    }
}

struct BoxNode {
    id: NodeId,
    pseudo: PseudoElement,
    declarations: Option<Arc<Locked<PropertyDeclarationBlock>>>,
}

pub(crate) struct Tree {
    nodes: Vec<BoxNode>,
    group_frames: style::FxHashMap<NodeId, (style::Atom, String)>,
}

impl BaseDocument {
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
        let mut descriptors = vec![(ViewTransition, 0, String::new())];
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
            descriptors.push((ViewTransitionGroup(arg), 0, css));
            descriptors.push((
                ViewTransitionPair(arg),
                group,
                if both { "isolation:isolate".into() } else { String::new() },
            ));
            for (exists, pseudo, fade) in [
                (part.old.is_some(), ViewTransitionOld(arg), "out"),
                (part.new.is_some(), ViewTransitionNew(arg), "in"),
            ] {
                if exists {
                    descriptors.push((
                        pseudo,
                        group + 1,
                        format!(
                            "animation-name:-ua-view-transition-fade-{fade}{}",
                            if both { ",-ua-mix-blend-mode-plus-lighter" } else { "" }
                        ),
                    ));
                }
            }
        }
        let mut nodes = Vec::<BoxNode>::with_capacity(descriptors.len());
        for (pseudo, parent, css) in descriptors {
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
            let Some(style) = self.transition_cascade(origin, parent, pseudo, declarations.as_ref()) else {
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
            nodes.push(BoxNode { id, pseudo, declarations });
        }
        let count = nodes.len();
        let group_frames = generated.into_iter().map(|(index, name, css)| (nodes[index].id, (name, css))).collect();
        self.layout_transition_tree(origin, nodes[0].id);
        self.transition_trees.get_or_insert_with(Default::default).insert(origin, Tree { nodes, group_frames });
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
        let Some(trees) = self.transition_trees.take() else {
            return;
        };
        for (origin, tree) in trees.iter() {
            for item in &tree.nodes {
                if let Some(parent) = self.nodes.get(item.id).and_then(|node| node.parent)
                    && let Some(style) =
                        self.transition_cascade(*origin, parent, item.pseudo, item.declarations.as_ref())
                {
                    self.nodes[item.id].stylo_element_data_mut().ensure_init_mut().styles.primary = Some(style);
                    self.nodes[item.id].update_layout_styles();
                    self.nodes[item.id].clear_layout_cache();
                }
            }
            self.layout_transition_tree(*origin, tree.nodes[0].id);
        }
        self.transition_trees = Some(trees);
    }
}
