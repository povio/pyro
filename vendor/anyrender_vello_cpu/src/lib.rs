//! A [`vello_cpu`] backend for the [`anyrender`] 2D drawing abstraction
#![cfg_attr(docsrs, feature(doc_cfg))]

mod image_cache;
mod image_convert;
mod image_renderer;
mod scene;
mod window_renderer;

mod backdrop;
mod graph;
/// pyro: see the `graph` module (an SVG filter primitive's subregion).
pub use graph::CROP_IMAGE as FILTER_CROP_IMAGE;
/// pyro: see the `graph` module (the `Image` nodes for what anyrender's filter types cannot say:
/// an `feImage`'s content, a blur of two deviations, `feMorphology` of two radii, `feTurbulence`).
pub use graph::{
    BLUR_XY as FILTER_BLUR_XY, IMAGE_SOURCE as FILTER_IMAGE_SOURCE, MORPHOLOGY as FILTER_MORPHOLOGY,
    TURBULENCE as FILTER_TURBULENCE,
};
mod filters;
mod occlusion;
mod primitives;
/// pyro (lane H2): planes drawn through a perspective.
mod projective;
pub use projective::{Homography, projective_marker};

pub use image_cache::ImageCacheConfig;
pub use image_renderer::VelloCpuImageRenderer;
pub use scene::VelloCpuScenePainter;

#[cfg(any(
    feature = "pixels_window_renderer",
    feature = "softbuffer_window_renderer"
))]
pub use window_renderer::*;
