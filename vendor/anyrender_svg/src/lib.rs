// Copyright 2023 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Render an SVG into any impl of [`anyrender::PaintScene`].
//!
//! This currently lacks support for some important SVG features. Known missing features include: masking, filter effects, group backgrounds
//! path shape-rendering, and patterns.

// LINEBENDER LINT SET - lib.rs - v1
// See https://linebender.org/wiki/canonical-lints/
// These lints aren't included in Cargo.toml because they
// shouldn't apply to examples and tests
#![warn(unused_crate_dependencies)]
#![warn(clippy::print_stdout, clippy::print_stderr)]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_cfg))]
// The following lints are part of the Linebender standard set,
// but resolving them has been deferred for now.
// Feel free to send a PR that solves one or more of these.
#![allow(missing_docs, clippy::shadow_unrelated, clippy::missing_errors_doc)]
#![cfg_attr(test, allow(unused_crate_dependencies))] // Some dev dependencies are only used in tests

mod error;
mod render;
mod util;

pub use error::Error;
pub use usvg;

use anyrender::PaintScene;
use kurbo::Affine;

/// Append an SVG to an [`anyrender::PaintScene`].
///
/// This will draw a red box over (some) unsupported elements.
pub fn render_svg_str<S: PaintScene>(
    scene: &mut S,
    svg: &str,
    transform: Affine,
) -> Result<(), Error> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt)?;
    render_svg_tree(scene, &tree, transform);
    Ok(())
}

/// Append an SVG to an [`anyrender::PaintScene`] (with custom error handling).
///
/// See the [module level documentation](crate#unsupported-features) for a list of some unsupported svg features
pub fn render_svg_str_with<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    svg: &str,
    transform: Affine,
    error_handler: &mut F,
) -> Result<(), Error> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt)?;
    render_svg_tree_with(scene, &tree, transform, error_handler);
    Ok(())
}

/// Append a [`usvg::Tree`] to an [`anyrender::PaintScene`].
///
/// This will draw a red box over (some) unsupported elements.
pub fn render_svg_tree<S: PaintScene>(scene: &mut S, svg: &usvg::Tree, transform: Affine) {
    render_svg_tree_with(scene, svg, transform, &mut util::default_error_handler);
}

/// Append a [`usvg::Tree`] to an [`anyrender::PaintScene`] (with custom error handling).
///
/// See the [module level documentation](crate#unsupported-features) for a list of some unsupported svg features
pub fn render_svg_tree_with<S: PaintScene, F: FnMut(&mut S, &usvg::Node)>(
    scene: &mut S,
    svg: &usvg::Tree,
    transform: Affine,
    error_handler: &mut F,
) {
    // pyro: the tree's viewport in the scene bounds offscreen rasters.
    let size = svg.size();
    let viewport = kurbo::Rect::new(0.0, 0.0, size.width() as f64, size.height() as f64);
    let ctx = render::Ctx {
        cull: transform.transform_rect_bbox(viewport),
    };
    render::render_group(
        scene,
        svg.root(),
        Affine::IDENTITY,
        transform,
        error_handler,
        &ctx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::recording::{RenderCommand, Scene};
    use peniko::Fill;

    fn clip_rule(clip_attrs: &str, path_attrs: &str) -> Fill {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <defs><clipPath id="clip" {clip_attrs}>
                    <path {path_attrs} d="M0 0H100V100H0Z M25 25H75V75H25Z"/>
                </clipPath></defs>
                <rect width="100" height="100" clip-path="url(#clip)"/>
            </svg>"##
        );
        let mut scene = Scene::new();
        render_svg_str(&mut scene, &svg, Affine::IDENTITY).unwrap();
        scene
            .commands
            .iter()
            .find_map(|command| match command {
                RenderCommand::PushLayer(layer) => Some(layer.fill),
                // pyro: a clip path of one shape is a clip layer, or (clipping a
                // rectangle that holds it, as here) the shape filled.
                RenderCommand::PushClipLayer(layer) => Some(layer.fill),
                RenderCommand::Fill(fill) => Some(fill.fill),
                _ => None,
            })
            .expect("SVG should produce a clipping layer or the clip shape")
    }

    #[test]
    fn clip_rule_attribute_is_preserved() {
        assert_eq!(clip_rule("", ""), Fill::NonZero);
        assert_eq!(clip_rule("", r#"clip-rule="nonzero""#), Fill::NonZero);
        assert_eq!(clip_rule("", r#"clip-rule="evenodd""#), Fill::EvenOdd);
    }

    #[test]
    fn clip_rule_style_overrides_attribute_and_inherits() {
        assert_eq!(
            clip_rule("", r#"clip-rule="nonzero" style="clip-rule: evenodd""#),
            Fill::EvenOdd
        );
        assert_eq!(
            clip_rule(r#"style="clip-rule: evenodd""#, ""),
            Fill::EvenOdd
        );
        assert_eq!(
            clip_rule(r#"clip-rule="evenodd""#, r#"style="clip-rule: nonzero""#),
            Fill::NonZero
        );
    }

    fn commands(svg: &str) -> Vec<RenderCommand> {
        let mut scene = Scene::new();
        render_svg_str(&mut scene, svg, Affine::IDENTITY).unwrap();
        scene.commands
    }

    fn dest_in_layers(commands: &[RenderCommand]) -> usize {
        commands
            .iter()
            .filter(|c| matches!(c, RenderCommand::PushLayer(l) if l.blend.compose == peniko::Compose::DestIn))
            .count()
    }

    // pyro
    #[test]
    fn a_pattern_fills_with_a_repeated_image_of_its_tile() {
        let commands = commands(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <pattern id="p" patternUnits="userSpaceOnUse" width="10" height="20">
                    <rect width="5" height="20" fill="green"/></pattern>
                <rect width="100" height="100" fill="url(#p)"/>
            </svg>"##,
        );
        let image = commands
            .iter()
            .find_map(|c| match c {
                RenderCommand::Fill(f) => match &f.brush {
                    anyrender::Paint::Image(image) => Some(image.clone()),
                    _ => None,
                },
                _ => None,
            })
            .expect("the pattern is an image brush");
        assert_eq!((image.image.width, image.image.height), (10, 20));
        assert_eq!(image.sampler.x_extend, peniko::Extend::Repeat);
        // Left half green, right half transparent (premultiplied).
        assert_eq!(&image.image.data.data()[..4], &[0, 128, 0, 255]);
        assert_eq!(&image.image.data.data()[9 * 4..9 * 4 + 4], &[0, 0, 0, 0]);
    }

    // pyro
    #[test]
    fn a_clip_path_of_several_shapes_and_a_mask_are_dest_in_layers() {
        let one_shape = commands(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <clipPath id="c"><rect width="10" height="10"/></clipPath>
                <rect width="100" height="100" clip-path="url(#c)"/>
            </svg>"##,
        );
        assert_eq!(dest_in_layers(&one_shape), 0);
        let two_shapes = commands(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <clipPath id="c"><rect width="10" height="10"/><circle cx="50" cy="50" r="10"/></clipPath>
                <circle cx="50" cy="50" r="50" clip-path="url(#c)"/>
            </svg>"##,
        );
        assert_eq!(dest_in_layers(&two_shapes), 1);
        // A rectangle in one colour holding the shapes: the shapes filled with it.
        let filled = commands(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <clipPath id="c"><rect width="10" height="10"/><circle cx="50" cy="50" r="10"/></clipPath>
                <rect width="100" height="100" fill="green" clip-path="url(#c)"/>
            </svg>"##,
        );
        assert_eq!(dest_in_layers(&filled), 0);
        assert_eq!(
            filled
                .iter()
                .filter(|c| matches!(c, RenderCommand::Fill(_)))
                .count(),
            2
        );
        let masked = commands(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <mask id="m"><rect width="50" height="100" fill="white"/></mask>
                <rect width="100" height="100" mask="url(#m)"/>
            </svg>"##,
        );
        assert_eq!(dest_in_layers(&masked), 1);
    }

    #[test]
    fn fill_rule_does_not_control_clipping() {
        assert_eq!(clip_rule("", r#"fill-rule="evenodd""#), Fill::NonZero);
        assert_eq!(
            clip_rule("", r#"fill-rule="nonzero" clip-rule="evenodd""#),
            Fill::EvenOdd
        );
    }
}
