use anyrender::{Filter, PaintScene};
use kurbo::{Affine, Shape};
use peniko::{BlendMode, Fill};
use std::{cell::Cell, sync::Arc};

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
        self.maybe_push_filled_layer(
            scene,
            condition,
            Fill::NonZero,
            blend_mode,
            opacity,
            transform,
            shape,
            filter,
            backdrop_filter,
        )
    }

    /// pyro: [`Self::maybe_with_layer`] for a clip whose shape has its own fill rule
    /// (`clip-path` with `evenodd`).
    pub(crate) fn maybe_with_clip_layer<S: PaintScene, F: FnOnce(&mut S)>(
        &self,
        scene: &mut S,
        condition: bool,
        fill: Fill,
        transform: Affine,
        shape: &impl Shape,
        paint_layer: F,
    ) {
        let layer_used = self.maybe_push_filled_layer(
            scene,
            condition,
            fill,
            BlendMode::default(),
            1.0,
            transform,
            shape,
            None,
            None,
        );
        paint_layer(scene);
        self.maybe_pop_layer(scene, layer_used);
    }

    /// [`Self::maybe_push_layer`] whose clip shape fills by `fill`.
    #[allow(clippy::too_many_arguments)]
    fn maybe_push_filled_layer(
        &self,
        scene: &mut impl PaintScene,
        condition: bool,
        fill: Fill,
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
            scene.push_clip_layer(fill, transform, shape);
        } else {
            scene.push_layer(
                fill,
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
