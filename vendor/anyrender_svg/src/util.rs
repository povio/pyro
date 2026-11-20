// Copyright 2023 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use anyrender::{Paint, PaintScene};
use kurbo::{Affine, BezPath, Point, Rect, Stroke};
use peniko::color::{self, DynamicColor};
use peniko::{Color, Fill, Mix};

#[cfg(feature = "image")]
use peniko::ImageBrush;

pub(crate) fn to_affine(ts: &usvg::Transform) -> Affine {
    let usvg::Transform {
        sx,
        kx,
        ky,
        sy,
        tx,
        ty,
    } = ts;
    Affine::new([sx, ky, kx, sy, tx, ty].map(|&x| f64::from(x)))
}

pub(crate) fn to_stroke(stroke: &usvg::Stroke) -> Stroke {
    let mut conv_stroke = Stroke::new(stroke.width().get() as f64)
        .with_caps(match stroke.linecap() {
            usvg::LineCap::Butt => kurbo::Cap::Butt,
            usvg::LineCap::Round => kurbo::Cap::Round,
            usvg::LineCap::Square => kurbo::Cap::Square,
        })
        .with_join(match stroke.linejoin() {
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => kurbo::Join::Miter,
            usvg::LineJoin::Round => kurbo::Join::Round,
            usvg::LineJoin::Bevel => kurbo::Join::Bevel,
        })
        .with_miter_limit(stroke.miterlimit().get() as f64);
    if let Some(dash_array) = stroke.dasharray().as_ref() {
        conv_stroke = conv_stroke.with_dashes(
            stroke.dashoffset() as f64,
            dash_array.iter().map(|x| *x as f64),
        );
    }
    conv_stroke
}

pub(crate) fn to_mix(blend_mode: usvg::BlendMode) -> Mix {
    match blend_mode {
        usvg::BlendMode::Normal => Mix::Normal,
        usvg::BlendMode::Multiply => Mix::Multiply,
        usvg::BlendMode::Screen => Mix::Screen,
        usvg::BlendMode::Overlay => Mix::Overlay,
        usvg::BlendMode::Darken => Mix::Darken,
        usvg::BlendMode::Lighten => Mix::Lighten,
        usvg::BlendMode::ColorDodge => Mix::ColorDodge,
        usvg::BlendMode::ColorBurn => Mix::ColorBurn,
        usvg::BlendMode::HardLight => Mix::HardLight,
        usvg::BlendMode::SoftLight => Mix::SoftLight,
        usvg::BlendMode::Difference => Mix::Difference,
        usvg::BlendMode::Exclusion => Mix::Exclusion,
        usvg::BlendMode::Hue => Mix::Hue,
        usvg::BlendMode::Saturation => Mix::Saturation,
        usvg::BlendMode::Color => Mix::Color,
        usvg::BlendMode::Luminosity => Mix::Luminosity,
    }
}

pub(crate) fn to_bez_path(path: &usvg::Path) -> BezPath {
    let mut local_path = BezPath::new();
    // The semantics of SVG paths don't line up with `BezPath`; we
    // must manually track initial points
    let mut just_closed = false;
    let mut most_recent_initial = (0., 0.);
    for elt in path.data().segments() {
        match elt {
            usvg::tiny_skia_path::PathSegment::MoveTo(p) => {
                if std::mem::take(&mut just_closed) {
                    local_path.move_to(most_recent_initial);
                }
                most_recent_initial = (p.x.into(), p.y.into());
                local_path.move_to(most_recent_initial);
            }
            usvg::tiny_skia_path::PathSegment::LineTo(p) => {
                if std::mem::take(&mut just_closed) {
                    local_path.move_to(most_recent_initial);
                }
                local_path.line_to(Point::new(p.x as f64, p.y as f64));
            }
            usvg::tiny_skia_path::PathSegment::QuadTo(p1, p2) => {
                if std::mem::take(&mut just_closed) {
                    local_path.move_to(most_recent_initial);
                }
                local_path.quad_to(
                    Point::new(p1.x as f64, p1.y as f64),
                    Point::new(p2.x as f64, p2.y as f64),
                );
            }
            usvg::tiny_skia_path::PathSegment::CubicTo(p1, p2, p3) => {
                if std::mem::take(&mut just_closed) {
                    local_path.move_to(most_recent_initial);
                }
                local_path.curve_to(
                    Point::new(p1.x as f64, p1.y as f64),
                    Point::new(p2.x as f64, p2.y as f64),
                    Point::new(p3.x as f64, p3.y as f64),
                );
            }
            usvg::tiny_skia_path::PathSegment::Close => {
                just_closed = true;
                local_path.close_path();
            }
        }
    }

    local_path
}

#[cfg(feature = "image")]
pub(crate) fn into_image(
    image: pyro_raster::Raster,
    rendering_mode: usvg::ImageRendering,
) -> ImageBrush {
    use peniko::ImageData;

    let quality = match rendering_mode {
        usvg::ImageRendering::HighQuality => peniko::ImageQuality::High,
        usvg::ImageRendering::OptimizeQuality | usvg::ImageRendering::Smooth => {
            peniko::ImageQuality::Medium
        }
        usvg::ImageRendering::OptimizeSpeed
        | usvg::ImageRendering::CrispEdges
        | usvg::ImageRendering::Pixelated => peniko::ImageQuality::Low,
    };

    ImageBrush::new(ImageData {
        data: image.blob,
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: image.width,
        height: image.height,
    })
    .with_quality(quality)
}

pub(crate) fn to_brush(paint: &usvg::Paint, opacity: usvg::Opacity) -> Option<(Paint, Affine)> {
    match paint {
        usvg::Paint::Color(color) => Some((
            Paint::Solid(Color::from_rgba8(
                color.red,
                color.green,
                color.blue,
                opacity.to_u8(),
            )),
            Affine::IDENTITY,
        )),
        usvg::Paint::LinearGradient(gr) => {
            let stops: Vec<peniko::ColorStop> = gr
                .stops()
                .iter()
                .map(|stop| peniko::ColorStop {
                    offset: stop.offset().get(),
                    color: DynamicColor::from_alpha_color(Color::from_rgba8(
                        stop.color().red,
                        stop.color().green,
                        stop.color().blue,
                        (stop.opacity() * opacity).to_u8(),
                    )),
                })
                .collect();
            let start = Point::new(gr.x1() as f64, gr.y1() as f64);
            let end = Point::new(gr.x2() as f64, gr.y2() as f64);
            let arr = [
                gr.transform().sx,
                gr.transform().ky,
                gr.transform().kx,
                gr.transform().sy,
                gr.transform().tx,
                gr.transform().ty,
            ]
            .map(f64::from);
            let transform = Affine::new(arr);
            let gradient = peniko::Gradient::new_linear(start, end)
                .with_stops(stops.as_slice())
                .with_extend(to_extend(gr.spread_method()));
            Some((Paint::Gradient(gradient), transform))
        }
        usvg::Paint::RadialGradient(gr) => {
            let stops: Vec<peniko::ColorStop> = gr
                .stops()
                .iter()
                .map(|stop| peniko::ColorStop {
                    offset: stop.offset().get(),
                    color: DynamicColor::from_alpha_color(Color::from_rgba8(
                        stop.color().red,
                        stop.color().green,
                        stop.color().blue,
                        (stop.opacity() * opacity).to_u8(),
                    )),
                })
                .collect();

            let start_center = Point::new(gr.fx() as f64, gr.fy() as f64);
            let end_center = Point::new(gr.cx() as f64, gr.cy() as f64);
            let start_radius = gr.fr().get();
            let end_radius = gr.r().get();
            let arr = [
                gr.transform().sx,
                gr.transform().ky,
                gr.transform().kx,
                gr.transform().sy,
                gr.transform().tx,
                gr.transform().ty,
            ]
            .map(f64::from);
            let transform = Affine::new(arr);
            let gradient = peniko::Gradient::new_two_point_radial(
                start_center,
                start_radius,
                end_center,
                end_radius,
            )
            .with_stops(stops.as_slice())
            .with_extend(to_extend(gr.spread_method()));
            Some((Paint::Gradient(gradient), transform))
        }
        usvg::Paint::Pattern(_) => None,
    }
}

fn to_extend(spread_method: usvg::SpreadMethod) -> peniko::Extend {
    match spread_method {
        usvg::SpreadMethod::Pad => peniko::Extend::Pad,
        usvg::SpreadMethod::Reflect => peniko::Extend::Reflect,
        usvg::SpreadMethod::Repeat => peniko::Extend::Repeat,
    }
}

/// Error handler function for [`super::append_tree_with`] which draws a transparent red box
/// instead of unsupported SVG features
pub(crate) fn default_error_handler<S: PaintScene>(scene: &mut S, node: &usvg::Node) {
    // pyro: a `<foreignObject>` this renderer cannot draw draws nothing.
    if crate::foreign_object().is_some() {
        return;
    }
    let bb = node.bounding_box();
    let rect = Rect {
        x0: bb.left() as f64,
        y0: bb.top() as f64,
        x1: bb.right() as f64,
        y1: bb.bottom() as f64,
    };
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color::palette::css::RED.multiply_alpha(0.5),
        None,
        &rect,
    );
}

#[cfg(feature = "image")]
pub(crate) fn decode_raw_raster_image(
    img: &usvg::ImageKind,
) -> Result<pyro_raster::Raster, image::ImageError> {
    let data = match img {
        usvg::ImageKind::JPEG(data) | usvg::ImageKind::PNG(data) |
        usvg::ImageKind::GIF(data) | usvg::ImageKind::WEBP(data) => data,
        usvg::ImageKind::SVG(_) => unreachable!(),
    };

    RASTERS.with_borrow_mut(|scope| scope.as_mut().expect("SVG render scope").decode(data))
}

#[cfg(feature = "image")]
thread_local! { static RASTERS: std::cell::RefCell<Option<pyro_raster::DocumentBudget>> = const { std::cell::RefCell::new(None) }; }

#[cfg(feature = "image")]
pub(crate) struct RasterScope(bool);

#[cfg(feature = "image")]
impl RasterScope {
    pub(crate) fn enter() -> Self {
        Self(RASTERS.with_borrow_mut(|current| {
            if current.is_some() { return false; }
            *current = Some(Default::default());
            true
        }))
    }
}

#[cfg(feature = "image")]
impl Drop for RasterScope {
    fn drop(&mut self) {
        if self.0 { RASTERS.with_borrow_mut(|current| { current.take(); }); }
    }
}
