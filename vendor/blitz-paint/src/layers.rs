use anyrender::{Filter, PaintScene};
use blitz_dom::{BaseDocument, NodeId};
use kurbo::{Affine, Shape};
use peniko::{BlendMode, Fill};
use std::{cell::Cell, collections::HashSet, sync::Arc};
use style::computed_values::mix_blend_mode::T as MixBlendMode;
use style::values::specified::box_::DisplayInside;

const LAYER_LIMIT: u32 = 1024;

#[derive(Default)]
pub(crate) struct LayerManager {
    layers_used: Cell<u32>,
    layer_depth: Cell<u32>,
    layers_wanted: Cell<u32>,

    #[allow(unused)] // Only used for debugging. Enabled as required.
    layer_depth_used: Cell<u32>,
}

impl LayerManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn maybe_with_layer<S: PaintScene, F: FnOnce(&mut S)>(
        &self,
        scene: &mut S,
        condition: bool,
        blend_mode: BlendMode,
        opacity: f32,
        transform: Affine,
        shape: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
        paint_layer: F,
    ) {
        let layer_used = self.maybe_push_layer(
            scene,
            condition,
            blend_mode,
            opacity,
            transform,
            shape,
            filter,
            backdrop_filter,
        );
        paint_layer(scene);
        self.maybe_pop_layer(scene, layer_used);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn maybe_push_layer(
        &self,
        scene: &mut impl PaintScene,
        condition: bool,
        blend_mode: BlendMode,
        opacity: f32,
        transform: Affine,
        shape: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
    ) -> bool {
        if !condition {
            return false;
        }
        self.layers_wanted.update(|x| x + 1);

        // Check if clips are above limit
        let layers_available = self.layers_used.get() <= LAYER_LIMIT;
        if !layers_available {
            return false;
        }

        // Actually push the layer
        if blend_mode == BlendMode::default()
            && opacity == 1.0
            && filter.is_none()
            && backdrop_filter.is_none()
        {
            scene.push_clip_layer(Fill::NonZero, transform, shape);
        } else {
            scene.push_layer(
                Fill::NonZero,
                blend_mode,
                opacity,
                transform,
                shape,
                filter,
                backdrop_filter,
            );
        };

        // Update accounting
        self.layers_used.update(|x| x + 1);
        self.layer_depth.update(|x| x + 1);
        self.layer_depth.update(|x| x.max(self.layer_depth.get()));

        true
    }

    pub(crate) fn maybe_pop_layer(&self, scene: &mut impl PaintScene, condition: bool) {
        if condition {
            scene.pop_layer();
            self.layer_depth.update(|x| x - 1);
        }
    }
}

/// pyro: the stacking contexts that an element with a `mix-blend-mode` blends in (its nearest
/// ancestor that is one: the root element's at the latest). Each is painted as an isolated group,
/// as Chromium isolates a stacking context with blending descendants: the element blends with what
/// that context painted, never with what lies under it (the canvas under the root element's).
/// Without one, a `difference` box over its parent's box blended with the page behind too. One pass
/// over the nodes' styles; usually empty.
pub(crate) fn blend_isolation(dom: &BaseDocument) -> HashSet<NodeId> {
    let tree = dom.tree();
    let mut contexts = HashSet::new();
    for (_, node) in tree.iter() {
        let blends = node
            .primary_styles()
            .is_some_and(|style| style.get_effects().mix_blend_mode != MixBlendMode::Normal);
        if !blends {
            continue;
        }
        let mut ancestor = node.parent;
        while let Some(id) = ancestor {
            let candidate = &tree[id];
            let flex_or_grid_item = candidate
                .parent
                .and_then(|parent| tree[parent].primary_styles())
                .is_some_and(|style| {
                    matches!(
                        style.get_box().display.inside(),
                        DisplayInside::Flex | DisplayInside::Grid
                    )
                });
            if candidate.is_root_element() || candidate.is_stacking_context_root(flex_or_grid_item)
            {
                contexts.insert(id);
                break;
            }
            ancestor = candidate.parent;
        }
    }
    contexts
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::{Scene, recording::RenderCommand};
    use kurbo::Rect;
    use peniko::{Compose, Mix};

    #[test]
    fn blend_layers_are_not_replaced_by_clips() {
        for blend in [
            Mix::Multiply.into(),
            BlendMode::new(Mix::Normal, Compose::PlusLighter),
        ] {
            let manager = LayerManager::default();
            let mut scene = Scene::new();
            manager.maybe_with_layer(
                &mut scene,
                true,
                blend,
                1.0,
                Affine::IDENTITY,
                &Rect::ZERO,
                None,
                None,
                |_| {},
            );
            assert!(
                matches!(&scene.commands[0], RenderCommand::PushLayer(layer) if layer.blend == blend)
            );
            assert!(matches!(scene.commands[1], RenderCommand::PopLayer));
        }
    }

    #[test]
    fn normal_layers_clip_unless_isolation_is_forced() {
        for (opacity, isolated) in [(1.0, false), (1.0 - f32::EPSILON, true)] {
            let manager = LayerManager::default();
            let mut scene = Scene::new();
            manager.maybe_with_layer(
                &mut scene,
                true,
                BlendMode::default(),
                opacity,
                Affine::IDENTITY,
                &Rect::ZERO,
                None,
                None,
                |_| {},
            );
            assert_eq!(
                matches!(scene.commands[0], RenderCommand::PushLayer(_)),
                isolated
            );
            assert!(matches!(scene.commands[1], RenderCommand::PopLayer));
        }
    }

    #[test]
    fn disabled_or_exhausted_layers_do_not_pop() {
        let manager = LayerManager::default();
        let mut scene = Scene::new();
        for condition in [false, true] {
            manager.layers_used.set(LAYER_LIMIT + 1);
            manager.maybe_with_layer(
                &mut scene,
                condition,
                Mix::Multiply.into(),
                1.0,
                Affine::IDENTITY,
                &Rect::ZERO,
                None,
                None,
                |_| {},
            );
        }
        assert!(scene.commands.is_empty());
    }
}
