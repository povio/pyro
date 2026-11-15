//! pyro: filters that are no chain: an SVG `<filter>`'s primitives, which read the source graphic,
//! its alpha and each other's results. [`Graph`] holds them as nodes, each computed over one region
//! of device pixels: the part of the filter region (the layer's clip) that the output needs.
//! Everything outside the filter region is transparent, the source graphic too, as SVG asks.
//!
//! Two conventions hold between the producers (anyrender_svg) and this module:
//!
//! - The graph's output is its last node. `anyrender::Filter::add` leaves a graph's output at the
//!   first node, and the setter is private.
//! - A primitive's subregion, where it is smaller than the filter region, is an `Image` node of the
//!   image id [`CROP_IMAGE`] after it, its input that node, the rectangle (in the filter's space,
//!   the layer's) in its transform: `[x0, y0, x1, y1, _, _]`. Everything outside it is cleared.
//!
//! Colour spaces are the producer's: a primitive in linearRGB reads its inputs through a
//! `feComponentTransfer` into linearRGB and its result goes back to sRGB through another, and the
//! colour functions of a row are applied to each pixel in one go ([`color_ops`]), as Skia composes
//! colour filters.
//!
//! The other primitives ([`crate::primitives`]) take what anyrender's types cannot say as these
//! conventions:
//!
//! - `feTile` (`FilterEffect::Tile`) tiles the rectangle of the crop node it reads (its input's
//!   subregion); read from anything else it passes its input on.
//! - `feConvolveMatrix` (`FilterEffect::ConvolveMatrix`): `size` is the number of columns, and
//!   `values` holds the kernel, row by row, then the number of rows, the target's x and y and the
//!   edge mode (0 duplicate, 1 wrap, 2 none); without those four, the kernel is square, its target
//!   its centre and its edge mode duplicate.
//! - The lighting primitives read the lighting colour as the colour of the `Flood` node that is their
//!   second input.
//! - `Image` nodes of the ids below the crop's: an `feImage`'s content ([`IMAGE_SOURCE`]), a blur of
//!   two deviations ([`BLUR_XY`]), `feMorphology` of two radii ([`MORPHOLOGY`]) and `feTurbulence`
//!   ([`TURBULENCE`]), their parameters in the transform.

use anyrender::Filter;
use anyrender::filters::component_transfer::TransferFunction;
use anyrender::filters::composite::CompositeOperator;
use anyrender::filters::displacement::ColorChannel;
use anyrender::filters::lighting::LightSource;
use anyrender::filters::morphology::MorphologyOperator;
use anyrender::filters::{ExternalImageSource, FilterEffect, FilterInput, FilterSource};
use kurbo::{Affine, Point, Rect};
use peniko::Mix;
use peniko::color::{AlphaColor, Srgb};
use vello_cpu::Pixmap;

use crate::filters::{ColorOp, PixelRect, blur, blur_xy, color_ops, div255, drop_shadow, reach};
use crate::primitives::{self, Convolution, Edge, Light, Lighting, Turbulence};

/// pyro: the image id of an `Image` filter node that stands for a crop of its input to the
/// rectangle `[x0, y0, x1, y1, ..]` of its transform, in the filter's space: a primitive's
/// subregion (see the module's documentation).
pub const CROP_IMAGE: u32 = u32::MAX;

/// pyro: the image id of an `Image` filter node (no input) that is the content of the filter
/// layer's `feImage` number `k` (`[k, 0, ..]` in its transform). That content is drawn in the filter
/// layer, before what the filter applies to, as a layer of opacity 0 whose filter is one such node
/// of the transform `[k, 1, ..]` ([`image_source_marker`]): replayed, it is rendered apart for the
/// graph; any other painter draws it invisible.
pub const IMAGE_SOURCE: u32 = u32::MAX - 1;
/// pyro: an `Image` filter node that is a Gaussian blur of its input of deviations `[x, y, ..]` (the
/// filter's units).
pub const BLUR_XY: u32 = u32::MAX - 2;
/// pyro: an `Image` filter node that is `feMorphology` of its input: `[radius x, radius y, 1 to
/// dilate or 0 to erode, ..]` (the filter's units).
pub const MORPHOLOGY: u32 = u32::MAX - 3;
/// pyro: an `Image` filter node (no input) that is `feTurbulence`: `[base frequency x, y, octaves,
/// seed, 1 for fractal noise or 0 for turbulence, ..]`.
pub const TURBULENCE: u32 = u32::MAX - 4;

/// pyro: which `feImage` a layer is the content of (see [`IMAGE_SOURCE`]), if it is one.
pub(crate) fn image_source_marker(filter: &Filter) -> Option<usize> {
    match filter.nodes() {
        [node] => match &node.effect {
            FilterEffect::Image(ExternalImageSource { image_id: IMAGE_SOURCE, transform: Some([k, marker, ..]) })
                if *marker == 1.0 =>
            {
                Some(k.max(0.0) as usize)
            }
            _ => None,
        },
        _ => None,
    }
}

/// What a node reads.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Input {
    Source,
    SourceAlpha,
    Node(usize),
}

/// What a node does, in device pixels.
#[derive(Clone, Debug)]
enum Op {
    /// The input as it is (`feGaussianBlur` of no deviation).
    Pass,
    /// Colour functions in a row ([`color_ops`]).
    Color(Vec<ColorOp>),
    Blur {
        sigma: f64,
    },
    /// The input moved (bilinearly for a fraction of a pixel).
    Offset {
        dx: f64,
        dy: f64,
    },
    /// A premultiplied colour over the whole region.
    Flood([u8; 4]),
    /// `feComposite`: the first input composited with the second.
    Composite(CompositeOperator),
    /// `feBlend`: the first input blended over the second (separable modes only).
    Blend(Mix),
    DropShadow {
        dx: f64,
        dy: f64,
        sigma: f64,
        color: AlphaColor<Srgb>,
    },
    /// The input with everything outside the rectangle (device pixels) cleared.
    Crop(PixelRect),
    /// `feImage` number `k`'s content.
    Image(usize),
    /// A blur of two deviations (device px).
    BlurXY {
        sx: f64,
        sy: f64,
    },
    /// `feMorphology` of radii in device px.
    Morphology {
        rx: usize,
        ry: usize,
        dilate: bool,
    },
    /// `feTile` of the rectangle (device pixels).
    Tile(PixelRect),
    /// `feDisplacementMap` of the first input by the second: the user-space displacement `scale *
    /// (C - 0.5)` of channels `x` and `y` mapped to device pixels by `m`.
    Displace {
        m: [f64; 4],
        scale: f64,
        x: usize,
        y: usize,
    },
    Convolve(Box<Convolution>),
    /// `feTurbulence`, evaluated at the user-space points `to_user` maps device pixels to.
    Turbulence(Box<Turbulence>, Affine),
    /// A lighting primitive, its light in device pixels.
    Lighting(Box<Lighting>),
}

#[derive(Clone, Debug)]
struct Node {
    op: Op,
    inputs: [Option<Input>; 2],
}

/// A filter graph in device pixels (see the module's documentation).
#[derive(Clone, Debug)]
pub(crate) struct Graph {
    nodes: Vec<Node>,
    /// How far (device px) the output reads from each of its pixels, along any path.
    reach: i32,
    /// Whether the output reads an `feImage`'s content.
    images: bool,
}

/// How far a node whose pixels may come from anywhere in its input reads (`feTile`): what the
/// layer's clip holds, up to this.
const FAR: i32 = 4096;

impl Graph {
    /// `filter` under `transform` (the layer's, to device pixels), or `None` when a node the
    /// output needs is one this backend does not do (the layer is then drawn unfiltered).
    pub(crate) fn new(filter: &Filter, transform: Affine) -> Option<Self> {
        let [a, b, c, d, _, _] = transform.as_coeffs();
        let scale = (a * d - b * c).abs().sqrt();
        let nodes = filter.nodes();
        let n = nodes.len();
        if n == 0 || !scale.is_finite() || scale == 0.0 {
            return None;
        }
        // The nodes the output needs: inputs come before their readers.
        let mut needed = vec![false; n];
        needed[n - 1] = true;
        for i in (0..n).rev() {
            if !needed[i] {
                continue;
            }
            for input in [&nodes[i].inputs.primary, &nodes[i].inputs.secondary].into_iter().flatten() {
                if let FilterInput::Result(id) = input {
                    let id = usize::from(id.0);
                    if id >= i {
                        return None;
                    }
                    needed[id] = true;
                }
            }
        }
        let mut index = vec![usize::MAX; n];
        let mut graph: Vec<Node> = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            if !needed[i] {
                continue;
            }
            // A missing input is the source graphic (a chain's first node has none).
            let input = |input: &Option<FilterInput>| match input {
                None | Some(FilterInput::Source(FilterSource::SourceGraphic)) => Some(Input::Source),
                Some(FilterInput::Source(FilterSource::SourceAlpha)) => Some(Input::SourceAlpha),
                Some(FilterInput::Source(_)) => None,
                Some(FilterInput::Result(id)) => Some(Input::Node(index[usize::from(id.0)])),
            };
            let primary = input(&node.inputs.primary)?;
            let (op, inputs) = match &node.effect {
                FilterEffect::ColorMatrix(matrix) => {
                    (Op::Color(vec![ColorOp::Matrix(matrix.0)]), [Some(primary), None])
                }
                FilterEffect::ComponentTransfer(transfer) => {
                    let functions = [
                        transfer.red_function.clone(),
                        transfer.green_function.clone(),
                        transfer.blue_function.clone(),
                        transfer.alpha_function.clone(),
                    ];
                    let op = if functions.iter().all(|f| matches!(f, TransferFunction::Identity)) {
                        Op::Pass
                    } else {
                        Op::Color(vec![ColorOp::Transfer(Box::new(functions))])
                    };
                    (op, [Some(primary), None])
                }
                FilterEffect::GaussianBlur(blur) => {
                    let op = if blur.std_deviation > 0.0 {
                        Op::Blur { sigma: f64::from(blur.std_deviation) * scale }
                    } else {
                        Op::Pass
                    };
                    (op, [Some(primary), None])
                }
                FilterEffect::DropShadow(shadow) => {
                    let (dx, dy) = (f64::from(shadow.dx), f64::from(shadow.dy));
                    let op = Op::DropShadow {
                        dx: a * dx + c * dy,
                        dy: b * dx + d * dy,
                        sigma: f64::from(shadow.std_deviation.max(0.0)) * scale,
                        color: shadow.color,
                    };
                    (op, [Some(primary), None])
                }
                FilterEffect::Offset(v) => {
                    let op = Op::Offset { dx: a * v.x + c * v.y, dy: b * v.x + d * v.y };
                    (op, [Some(primary), None])
                }
                FilterEffect::Flood(color) => {
                    let p = color.premultiply().to_rgba8();
                    (Op::Flood([p.r, p.g, p.b, p.a]), [None, None])
                }
                FilterEffect::Composite(operator) => {
                    (Op::Composite(*operator), [Some(primary), Some(input(&node.inputs.secondary)?)])
                }
                FilterEffect::Blend(mix) => (Op::Blend(*mix), [Some(primary), Some(input(&node.inputs.secondary)?)]),
                FilterEffect::Image(image) if image.image_id == CROP_IMAGE => {
                    let [x0, y0, x1, y1, ..] = image.transform?;
                    let rect = Rect::new(f64::from(x0), f64::from(y0), f64::from(x1), f64::from(y1));
                    (Op::Crop(PixelRect::around(transform.transform_rect_bbox(rect))), [Some(primary), None])
                }
                FilterEffect::Image(image) if image.image_id == IMAGE_SOURCE => {
                    let k = image.transform.map_or(0.0, |t| t[0]);
                    (Op::Image(k.max(0.0) as usize), [None, None])
                }
                FilterEffect::Image(image) if image.image_id == BLUR_XY => {
                    let [x, y, ..] = image.transform?;
                    let (sx, sy) = axis_scales(transform);
                    let (sx, sy) = (f64::from(x.max(0.0)) * sx, f64::from(y.max(0.0)) * sy);
                    let op = if sx > 0.0 || sy > 0.0 { Op::BlurXY { sx, sy } } else { Op::Pass };
                    (op, [Some(primary), None])
                }
                FilterEffect::Image(image) if image.image_id == MORPHOLOGY => {
                    let [x, y, dilate, ..] = image.transform?;
                    (morphology(transform, f64::from(x), f64::from(y), dilate != 0.0)?, [Some(primary), None])
                }
                FilterEffect::Morphology(m) => {
                    let r = f64::from(m.radius);
                    (morphology(transform, r, r, m.operator == MorphologyOperator::Dilate)?, [Some(primary), None])
                }
                FilterEffect::Image(image) if image.image_id == TURBULENCE => {
                    let [fx, fy, octaves, seed, fractal, ..] = image.transform?;
                    if transform.determinant().abs() < 1e-12 {
                        return None;
                    }
                    let t = Turbulence {
                        base_frequency: (f64::from(fx.max(0.0)), f64::from(fy.max(0.0))),
                        octaves: octaves.clamp(0.0, 24.0) as u32,
                        seed: seed as i32,
                        fractal_noise: fractal != 0.0,
                    };
                    (Op::Turbulence(Box::new(t), transform.inverse()), [None, None])
                }
                FilterEffect::Tile => {
                    // The rectangle of the crop it reads, its input's subregion.
                    let op = match primary {
                        Input::Node(k) => match graph[k].op {
                            Op::Crop(rect) => Op::Tile(rect),
                            _ => Op::Pass,
                        },
                        _ => Op::Pass,
                    };
                    (op, [Some(primary), None])
                }
                FilterEffect::DisplacementMap(map) => {
                    let channel = |c: ColorChannel| match c {
                        ColorChannel::Red => 0,
                        ColorChannel::Green => 1,
                        ColorChannel::Blue => 2,
                        ColorChannel::Alpha => 3,
                    };
                    let op = Op::Displace {
                        m: [a, b, c, d],
                        scale: f64::from(map.scale),
                        x: channel(map.x_channel),
                        y: channel(map.y_channel),
                    };
                    (op, [Some(primary), Some(input(&node.inputs.secondary)?)])
                }
                FilterEffect::ConvolveMatrix(kernel) => (convolution(kernel)?, [Some(primary), None]),
                FilterEffect::DiffuseLighting(_) | FilterEffect::SpecularLighting(_) => {
                    // The lighting colour: the flood that is the second input.
                    let color = match node.inputs.secondary {
                        Some(FilterInput::Result(id)) => match &nodes.get(usize::from(id.0))?.effect {
                            FilterEffect::Flood(color) => color.to_rgba8(),
                            _ => return None,
                        },
                        _ => return None,
                    };
                    let (light, surface_scale, constant, specular_exponent) = match &node.effect {
                        FilterEffect::DiffuseLighting(l) => {
                            (&l.light_source, l.surface_scale, l.diffuse_constant, None)
                        }
                        FilterEffect::SpecularLighting(l) => (
                            &l.light_source,
                            l.surface_scale,
                            l.specular_constant,
                            Some(f64::from(l.specular_exponent)),
                        ),
                        _ => unreachable!(),
                    };
                    let lighting = Lighting {
                        light: light_in_device(light, transform)?,
                        surface_scale: f64::from(surface_scale),
                        constant: f64::from(constant),
                        specular_exponent,
                        color: [color.r, color.g, color.b],
                    };
                    (Op::Lighting(Box::new(lighting)), [Some(primary), None])
                }
                _ => return None,
            };
            index[i] = graph.len();
            graph.push(Node { op, inputs });
        }
        // A colour node read only by the colour node after it joins that node: the functions of
        // a row are applied without rounding between them.
        let last = graph.len() - 1;
        let mut uses = uses(&graph);
        for j in 0..graph.len() {
            let Some(Input::Node(k)) = graph[j].inputs[0] else {
                continue;
            };
            if k == last || uses[k] != 1 || !matches!(graph[j].op, Op::Color(_)) {
                continue;
            }
            if let Op::Color(before) = &graph[k].op {
                let mut ops = before.clone();
                let Op::Color(after) = &graph[j].op else { unreachable!() };
                ops.extend(after.iter().cloned());
                graph[j] = Node { op: Op::Color(ops), inputs: [graph[k].inputs[0], None] };
                graph[k] = Node { op: Op::Pass, inputs: [None, None] };
                uses[k] = 0;
            }
        }
        let mut reaches = vec![0i32; graph.len()];
        for (i, node) in graph.iter().enumerate() {
            let read = node
                .inputs
                .iter()
                .flatten()
                .map(|input| match *input {
                    Input::Node(k) => reaches[k],
                    _ => 0,
                })
                .max()
                .unwrap_or(0);
            let moved = |dx: f64, dy: f64| dx.abs().max(dy.abs()).ceil() as i32 + 1;
            let own = match &node.op {
                Op::Blur { sigma } => reach(*sigma),
                Op::BlurXY { sx, sy } => reach(sx.max(*sy)),
                Op::Offset { dx, dy } => moved(*dx, *dy),
                Op::DropShadow { dx, dy, sigma, .. } => reach(*sigma) + moved(*dx, *dy),
                Op::Morphology { rx, ry, .. } => (*rx.max(ry)).min(FAR as usize) as i32,
                Op::Tile(_) => FAR,
                Op::Displace { m, scale, .. } => {
                    let half = scale.abs() / 2.0;
                    moved(half * (m[0].abs() + m[2].abs()), half * (m[1].abs() + m[3].abs())).min(FAR)
                }
                Op::Convolve(k) => k.columns.max(k.rows).min(FAR as usize) as i32,
                Op::Lighting(_) => 1,
                _ => 0,
            };
            reaches[i] = read.saturating_add(own).min(FAR);
        }
        let images = graph.iter().any(|node| matches!(node.op, Op::Image(_)));
        Some(Self { reach: reaches[last], nodes: graph, images })
    }

    /// Whether the graph reads the content of `feImage`s (see [`IMAGE_SOURCE`]).
    pub(crate) fn reads_images(&self) -> bool {
        self.images
    }

    /// The pixels of the source the output over `out` reads.
    pub(crate) fn source_region(&self, out: PixelRect) -> PixelRect {
        PixelRect { x0: out.x0 - self.reach, y0: out.y0 - self.reach, x1: out.x1 + self.reach, y1: out.y1 + self.reach }
    }

    /// The output over `region`, from `source`: the layer's content (premultiplied RGBA) there, and
    /// `images`, the content of its `feImage`s there (by number; transparent where missing).
    pub(crate) fn render(&self, source: Pixmap, images: &[Option<Pixmap>], region: PixelRect) -> Pixmap {
        let (w, h) = (source.width(), source.height());
        let mut uses = uses(&self.nodes);
        let last = self.nodes.len() - 1;
        let mut results: Vec<Option<Pixmap>> = (0..self.nodes.len()).map(|_| None).collect();
        let mut source_alpha: Option<Pixmap> = None;
        let mut scratch = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if uses[i] == 0 && i != last {
                continue;
            }
            let mut read = |input: Option<Input>, results: &[Option<Pixmap>]| -> Pixmap {
                match input {
                    Some(Input::Source) | None => source.clone(),
                    Some(Input::SourceAlpha) => source_alpha
                        .get_or_insert_with(|| {
                            let mut alpha = source.clone();
                            for px in alpha.data_as_u8_slice_mut().chunks_exact_mut(4) {
                                px[..3].fill(0);
                            }
                            alpha
                        })
                        .clone(),
                    Some(Input::Node(k)) => results[k].clone().unwrap_or_else(|| Pixmap::new(w, h)),
                }
            };
            let (wu, hu) = (usize::from(w), usize::from(h));
            let result = match &node.op {
                Op::Pass => read(node.inputs[0], &results),
                Op::Color(ops) => {
                    let mut pixels = read(node.inputs[0], &results);
                    color_ops(pixels.data_as_u8_slice_mut(), ops);
                    pixels
                }
                Op::Blur { sigma } => {
                    let mut pixels = read(node.inputs[0], &results);
                    blur(pixels.data_as_u8_slice_mut(), &mut scratch, wu, hu, 4, *sigma);
                    pixels
                }
                Op::Offset { dx, dy } => offset(&read(node.inputs[0], &results), *dx, *dy),
                Op::Flood(color) => {
                    let mut pixels = Pixmap::new(w, h);
                    for px in pixels.data_as_u8_slice_mut().chunks_exact_mut(4) {
                        px.copy_from_slice(color);
                    }
                    pixels
                }
                Op::Composite(operator) => {
                    let mut top = read(node.inputs[0], &results);
                    composite(&mut top, &read(node.inputs[1], &results), *operator);
                    top
                }
                Op::Blend(mix) => {
                    let mut top = read(node.inputs[0], &results);
                    blend(&mut top, &read(node.inputs[1], &results), *mix);
                    top
                }
                Op::DropShadow { dx, dy, sigma, color } => {
                    let mut pixels = read(node.inputs[0], &results);
                    drop_shadow(pixels.data_as_u8_slice_mut(), &mut scratch, wu, hu, *dx, *dy, *sigma, *color);
                    pixels
                }
                Op::Crop(rect) => {
                    let mut pixels = read(node.inputs[0], &results);
                    let keep = local(*rect, region);
                    for (y, row) in pixels.data_as_u8_slice_mut().chunks_exact_mut(wu * 4).enumerate() {
                        let y = y as i32;
                        for (x, px) in row.chunks_exact_mut(4).enumerate() {
                            let x = x as i32;
                            if x < keep.x0 || x >= keep.x1 || y < keep.y0 || y >= keep.y1 {
                                px.fill(0);
                            }
                        }
                    }
                    pixels
                }
                Op::Image(k) => match images.get(*k) {
                    Some(Some(pixels)) if pixels.width() == w && pixels.height() == h => pixels.clone(),
                    _ => Pixmap::new(w, h),
                },
                Op::BlurXY { sx, sy } => {
                    let mut pixels = read(node.inputs[0], &results);
                    blur_xy(pixels.data_as_u8_slice_mut(), &mut scratch, wu, hu, 4, *sx, *sy);
                    pixels
                }
                Op::Morphology { rx, ry, dilate } => {
                    let mut pixels = read(node.inputs[0], &results);
                    primitives::morphology(&mut pixels, *rx, *ry, *dilate);
                    pixels
                }
                Op::Tile(rect) => primitives::tile(&read(node.inputs[0], &results), local(*rect, region)),
                Op::Displace { m, scale, x, y } => {
                    let map = read(node.inputs[1], &results);
                    primitives::displace(&read(node.inputs[0], &results), &map, *m, *scale, *x, *y)
                }
                Op::Convolve(k) => primitives::convolve(&read(node.inputs[0], &results), k),
                Op::Turbulence(t, to_user) => {
                    let origin = Affine::translate((f64::from(region.x0), f64::from(region.y0)));
                    primitives::turbulence(w, h, t, *to_user * origin)
                }
                Op::Lighting(l) => {
                    let mut l = (**l).clone();
                    let (ox, oy) = (f64::from(region.x0), f64::from(region.y0));
                    match &mut l.light {
                        Light::Distant(_) => {}
                        Light::Point(at) | Light::Spot { at, .. } => {
                            at[0] -= ox;
                            at[1] -= oy;
                        }
                    }
                    primitives::lighting(&read(node.inputs[0], &results), &l)
                }
            };
            for input in node.inputs.iter().flatten() {
                if let Input::Node(k) = *input {
                    uses[k] -= 1;
                    if uses[k] == 0 {
                        results[k] = None;
                    }
                }
            }
            results[i] = Some(result);
        }
        let mut output = results[last].take().unwrap_or_else(|| Pixmap::new(w, h));
        output.set_may_have_transparency(true);
        output
    }
}

/// How many times each node is read.
fn uses(nodes: &[Node]) -> Vec<usize> {
    let mut uses = vec![0usize; nodes.len()];
    for node in nodes {
        for input in node.inputs.iter().flatten() {
            if let Input::Node(k) = *input {
                uses[k] += 1;
            }
        }
    }
    uses
}

/// Whether `mix` is a separable blend mode (one channel at a time).
fn separable(mix: Mix) -> bool {
    !matches!(mix, Mix::Hue | Mix::Saturation | Mix::Color | Mix::Luminosity)
}

/// `rect` (device pixels) in the pixels of `region`.
fn local(rect: PixelRect, region: PixelRect) -> PixelRect {
    PixelRect { x0: rect.x0 - region.x0, y0: rect.y0 - region.y0, x1: rect.x1 - region.x0, y1: rect.y1 - region.y0 }
}

/// How much `transform` scales lengths along the filter's x and y axes.
fn axis_scales(transform: Affine) -> (f64, f64) {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    (a.hypot(b), c.hypot(d))
}

/// `feMorphology` of radii `rx`, `ry` (the filter's units) in device pixels: the radius vector
/// mapped by the transform, rounded (as Skia rounds it), at most 256; a negative radius or one that
/// is not finite does nothing to the graph (`None`).
fn morphology(transform: Affine, rx: f64, ry: f64, dilate: bool) -> Option<Op> {
    if !(rx >= 0.0 && ry >= 0.0 && rx.is_finite() && ry.is_finite()) {
        return None;
    }
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let x = (a * rx).abs() + (c * ry).abs();
    let y = (b * rx).abs() + (d * ry).abs();
    let r = |v: f64| (v + 0.5).floor().clamp(0.0, 256.0) as usize;
    Some(Op::Morphology { rx: r(x), ry: r(y), dilate })
}

/// `feConvolveMatrix` from its node (see the module's documentation), or `None` when it is not
/// one this backend can do.
fn convolution(kernel: &anyrender::filters::convolution::ConvolutionKernel) -> Option<Op> {
    let columns = kernel.size as usize;
    if columns == 0 || columns > 64 {
        return None;
    }
    let values = &kernel.values;
    let (rows, target_x, target_y, edge) = if values.len() == columns * columns {
        (columns, columns / 2, columns / 2, Edge::Duplicate)
    } else {
        let extra = values.len().checked_sub(4)?;
        if extra % columns != 0 {
            return None;
        }
        let [rows, tx, ty, edge] = values[extra..] else { return None };
        let edge = match edge as i32 {
            0 => Edge::Duplicate,
            1 => Edge::Wrap,
            _ => Edge::None,
        };
        (rows as usize, tx as usize, ty as usize, edge)
    };
    if rows == 0 || rows > 64 || rows * columns > values.len() || target_x >= columns || target_y >= rows {
        return None;
    }
    if !(kernel.divisor.is_finite() && kernel.divisor != 0.0 && kernel.bias.is_finite()) {
        return None;
    }
    Some(Op::Convolve(Box::new(Convolution {
        columns,
        rows,
        kernel: values[..rows * columns].to_vec(),
        target_x,
        target_y,
        divisor: kernel.divisor,
        bias: kernel.bias,
        edge,
        preserve_alpha: kernel.preserve_alpha,
    })))
}

/// A lighting primitive's light source in device pixels: positions mapped by `transform`, heights
/// scaled by its mean scale (as Skia scales a light's z).
fn light_in_device(light: &LightSource, transform: Affine) -> Option<Light> {
    let (sx, sy) = axis_scales(transform);
    let sz = ((sx * sx + sy * sy) / 2.0).sqrt();
    let point = |x: f32, y: f32, z: f32| {
        let p = transform * Point::new(f64::from(x), f64::from(y));
        [p.x, p.y, f64::from(z) * sz]
    };
    Some(match light {
        LightSource::Distant(l) => {
            // Whole turns taken out first: -45 and 315 degrees light alike to the last bit (WPT
            // azimuth-and-elevation).
            let angle = |degrees: f32| f64::from(degrees).rem_euclid(360.0).to_radians();
            let (azimuth, elevation) = (angle(l.azimuth), angle(l.elevation));
            // The direction turned as the user space is (a mirror flips it).
            let v = [azimuth.cos() * elevation.cos(), azimuth.sin() * elevation.cos()];
            let [a, b, c, d, _, _] = transform.as_coeffs();
            let (x, y) = (a * v[0] + c * v[1], b * v[0] + d * v[1]);
            let length = x.hypot(y);
            let (x, y) = if length > 0.0 {
                let planar = (v[0] * v[0] + v[1] * v[1]).sqrt();
                (x / length * planar, y / length * planar)
            } else {
                (0.0, 0.0)
            };
            Light::Distant([x, y, elevation.sin()])
        }
        LightSource::Point(l) => Light::Point(point(l.x, l.y, l.z)),
        LightSource::Spot(l) => {
            let at = point(l.x, l.y, l.z);
            let towards = point(l.points_at_x, l.points_at_y, l.points_at_z);
            let v = [towards[0] - at[0], towards[1] - at[1], towards[2] - at[2]];
            let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let direction = if length > 0.0 { [v[0] / length, v[1] / length, v[2] / length] } else { v };
            Light::Spot {
                at,
                direction,
                exponent: f64::from(l.specular_exponent),
                cone: l.limiting_cone_angle.map(|angle| f64::from(angle).to_radians().cos()),
            }
        }
    })
}

/// `pixels` moved by `(dx, dy)` device px: each output pixel the input at (x - dx, y - dy),
/// transparent beyond it, interpolated between four pixels for a fraction of a pixel.
fn offset(pixels: &Pixmap, dx: f64, dy: f64) -> Pixmap {
    let (w, h) = (usize::from(pixels.width()), usize::from(pixels.height()));
    let mut out = Pixmap::new(pixels.width(), pixels.height());
    let src = pixels.data_as_u8_slice();
    let dst = out.data_as_u8_slice_mut();
    // Whole pixels when within a hundredth of one.
    let snap = |v: f64| if (v - v.round()).abs() < 0.01 { v.round() } else { v };
    let (dx, dy) = (snap(dx), snap(dy));
    let (ix, fx) = (dx.floor(), dx - dx.floor());
    let (iy, fy) = (dy.floor(), dy - dy.floor());
    let (ix, iy) = (ix as isize, iy as isize);
    let weights = [
        ((1.0 - fx) * (1.0 - fy) * 65536.0).round() as u32,
        (fx * (1.0 - fy) * 65536.0).round() as u32,
        ((1.0 - fx) * fy * 65536.0).round() as u32,
        (fx * fy * 65536.0).round() as u32,
    ];
    let at = |x: isize, y: isize, c: usize| -> u32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            0
        } else {
            u32::from(src[(y as usize * w + x as usize) * 4 + c])
        }
    };
    for y in 0..h {
        let sy = y as isize - iy;
        for x in 0..w {
            let sx = x as isize - ix;
            let px = &mut dst[(y * w + x) * 4..(y * w + x) * 4 + 4];
            for (c, v) in px.iter_mut().enumerate() {
                *v = if weights[0] == 65536 {
                    at(sx, sy, c)
                } else {
                    (at(sx - 1, sy - 1, c) * weights[3]
                        + at(sx, sy - 1, c) * weights[2]
                        + at(sx - 1, sy, c) * weights[1]
                        + at(sx, sy, c) * weights[0]
                        + 32768)
                        >> 16
                } as u8;
            }
        }
    }
    out
}

/// `feComposite`: `top` (premultiplied RGBA, the first input) composited with `bottom` (the
/// second) by `operator`, into `top`. The arithmetic one is computed on the premultiplied values,
/// each clamped, the colour to the alpha, as Skia's arithmetic image filter does.
fn composite(top: &mut Pixmap, bottom: &Pixmap, operator: CompositeOperator) {
    for (a, b) in top.data_as_u8_slice_mut().chunks_exact_mut(4).zip(bottom.data_as_u8_slice().chunks_exact(4)) {
        let (aa, ba) = (u32::from(a[3]), u32::from(b[3]));
        match operator {
            CompositeOperator::Over => {
                for c in 0..4 {
                    a[c] = (u32::from(a[c]) + div255(u32::from(b[c]) * (255 - aa))).min(255) as u8;
                }
            }
            CompositeOperator::In => {
                for c in 0..4 {
                    a[c] = div255(u32::from(a[c]) * ba) as u8;
                }
            }
            CompositeOperator::Out => {
                for c in 0..4 {
                    a[c] = div255(u32::from(a[c]) * (255 - ba)) as u8;
                }
            }
            CompositeOperator::Atop => {
                for c in 0..4 {
                    a[c] = div255(u32::from(a[c]) * ba + u32::from(b[c]) * (255 - aa)).min(255) as u8;
                }
            }
            CompositeOperator::Xor => {
                for c in 0..4 {
                    a[c] = div255(u32::from(a[c]) * (255 - ba) + u32::from(b[c]) * (255 - aa)).min(255) as u8;
                }
            }
            CompositeOperator::Arithmetic(k) => {
                let v = |c: usize| {
                    let (i1, i2) = (f32::from(a[c]) / 255.0, f32::from(b[c]) / 255.0);
                    (k.k1 * i1 * i2 + k.k2 * i1 + k.k3 * i2 + k.k4).clamp(0.0, 1.0)
                };
                let alpha = v(3);
                let rgb = [v(0), v(1), v(2)];
                for c in 0..3 {
                    a[c] = (rgb[c].min(alpha) * 255.0 + 0.5) as u8;
                }
                a[3] = (alpha * 255.0 + 0.5) as u8;
            }
        }
    }
}

/// `feBlend`: `top` (the first input) blended over `bottom` (the second) by `mix`, into `top`:
/// Compositing and Blending's formula on premultiplied values.
fn blend(top: &mut Pixmap, bottom: &Pixmap, mix: Mix) {
    for (s, b) in top.data_as_u8_slice_mut().chunks_exact_mut(4).zip(bottom.data_as_u8_slice().chunks_exact(4)) {
        let (sa, ba) = (f32::from(s[3]) / 255.0, f32::from(b[3]) / 255.0);
        let alpha = sa + ba - sa * ba;
        let mut out = [0.0f32; 4];
        let unpremultiplied = |p: &[u8], a: f32| -> [f32; 3] {
            std::array::from_fn(|c| if a > 0.0 { (f32::from(p[c]) / 255.0 / a).min(1.0) } else { 0.0 })
        };
        let (cs, cb) = (unpremultiplied(s, sa), unpremultiplied(b, ba));
        let mixed: [f32; 3] = if separable(mix) {
            std::array::from_fn(|c| mix_channel(mix, cb[c], cs[c]))
        } else {
            mix_colors(mix, cb, cs)
        };
        for c in 0..3 {
            let (sp, bp) = (f32::from(s[c]) / 255.0, f32::from(b[c]) / 255.0);
            out[c] = sp * (1.0 - ba) + bp * (1.0 - sa) + sa * ba * mixed[c];
        }
        out[3] = alpha;
        for c in 0..4 {
            s[c] = (out[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        for c in 0..3 {
            s[c] = s[c].min(s[3]);
        }
    }
}

/// A separable blend mode's function of the backdrop `cb` and the source `cs` (unpremultiplied).
fn mix_channel(mix: Mix, cb: f32, cs: f32) -> f32 {
    let hard_light = |cb: f32, cs: f32| {
        if cs <= 0.5 {
            cb * 2.0 * cs
        } else {
            let s = 2.0 * cs - 1.0;
            cb + s - cb * s
        }
    };
    match mix {
        Mix::Multiply => cb * cs,
        Mix::Screen => cb + cs - cb * cs,
        Mix::Overlay => hard_light(cs, cb),
        Mix::Darken => cb.min(cs),
        Mix::Lighten => cb.max(cs),
        Mix::ColorDodge => {
            if cb == 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        Mix::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        Mix::HardLight => hard_light(cb, cs),
        Mix::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        Mix::Difference => (cb - cs).abs(),
        Mix::Exclusion => cb + cs - 2.0 * cb * cs,
        _ => cs,
    }
}

/// A non-separable blend mode of the backdrop `cb` and the source `cs` (unpremultiplied RGB), as
/// Compositing and Blending defines them.
fn mix_colors(mix: Mix, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    let lum = |c: [f32; 3]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let clip = |c: [f32; 3]| {
        let l = lum(c);
        let n = c[0].min(c[1]).min(c[2]);
        let x = c[0].max(c[1]).max(c[2]);
        let mut c = c;
        if n < 0.0 && l - n > 0.0 {
            c = c.map(|v| l + (v - l) * l / (l - n));
        }
        if x > 1.0 && x - l > 0.0 {
            c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
        }
        c
    };
    let set_lum = |c: [f32; 3], l: f32| {
        let d = l - lum(c);
        clip(c.map(|v| v + d))
    };
    let sat = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    let set_sat = |c: [f32; 3], s: f32| {
        let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        if max > min { c.map(|v| (v - min) * s / (max - min)) } else { [0.0; 3] }
    };
    match mix {
        Mix::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Mix::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Mix::Color => set_lum(cs, lum(cb)),
        _ => set_lum(cb, lum(cs)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::filters::FilterInputs;
    use peniko::color::palette;

    fn pixmap(w: u16, h: u16, color: [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        for px in p.data_as_u8_slice_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&color);
        }
        p
    }

    fn at(p: &Pixmap, x: usize, y: usize) -> [u8; 4] {
        let i = (y * usize::from(p.width()) + x) * 4;
        p.data_as_u8_slice()[i..i + 4].try_into().unwrap()
    }

    const REGION: PixelRect = PixelRect { x0: 0, y0: 0, x1: 10, y1: 10 };

    /// `feFlood` then `feComposite in` the source: the flood's colour where the source is.
    #[test]
    fn a_flood_composited_in_the_source() {
        let mut filter = Filter::empty();
        let flood = filter.add(FilterEffect::Flood(palette::css::LIME), FilterInputs::NONE);
        filter.add(
            FilterEffect::Composite(CompositeOperator::In),
            FilterInputs::dual(FilterInput::Result(flood), FilterInput::Source(FilterSource::SourceGraphic)),
        );
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        let mut source = Pixmap::new(10, 10);
        source.data_as_u8_slice_mut()[..4].copy_from_slice(&[255, 0, 0, 255]);
        let out = graph.render(source, &[], REGION);
        assert_eq!(at(&out, 0, 0), [0, 255, 0, 255]);
        assert_eq!(at(&out, 1, 0), [0; 4]);
    }

    /// `feMerge` as `over`s, `feOffset` by whole pixels, and a subregion's crop.
    #[test]
    fn offsets_merges_and_crops() {
        let mut filter = Filter::empty();
        let moved = filter.add(
            FilterEffect::Offset(kurbo::Vec2::new(2.0, 1.0)),
            FilterInputs::single(FilterInput::Source(FilterSource::SourceGraphic)),
        );
        let flood = filter.add(FilterEffect::Flood(palette::css::BLUE), FilterInputs::NONE);
        let cropped = filter.add(
            FilterEffect::Image(ExternalImageSource {
                image_id: CROP_IMAGE,
                transform: Some([5.0, 0.0, 10.0, 10.0, 0.0, 0.0]),
            }),
            FilterInputs::single(FilterInput::Result(flood)),
        );
        filter.add(
            FilterEffect::Composite(CompositeOperator::Over),
            FilterInputs::dual(FilterInput::Result(moved), FilterInput::Result(cropped)),
        );
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        let mut source = Pixmap::new(10, 10);
        source.data_as_u8_slice_mut()[..4].copy_from_slice(&[255, 0, 0, 255]);
        let out = graph.render(source, &[], REGION);
        assert_eq!(at(&out, 2, 1), [255, 0, 0, 255], "the source, moved");
        assert_eq!(at(&out, 0, 0), [0; 4], "nothing left behind");
        assert_eq!(at(&out, 4, 5), [0; 4], "the flood, cropped");
        assert_eq!(at(&out, 5, 5), [0, 0, 255, 255], "the flood in its subregion");
    }

    /// Colour nodes in a row are one pass, without rounding between them; a node this backend
    /// does not do leaves the whole graph out.
    #[test]
    fn colour_rows_and_unsupported_nodes() {
        let mut filter = Filter::empty();
        let first = filter.add(
            FilterEffect::ColorMatrix(anyrender::filters::color_transformation::ColorMatrix::invert(1.0)),
            FilterInputs::NONE,
        );
        filter.add(
            FilterEffect::ColorMatrix(anyrender::filters::color_transformation::ColorMatrix::invert(1.0)),
            FilterInputs::single(FilterInput::Result(first)),
        );
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        assert_eq!(graph.nodes.iter().filter(|n| matches!(n.op, Op::Color(_))).count(), 1);
        let out = graph.render(pixmap(10, 10, [10, 20, 30, 255]), &[], REGION);
        assert_eq!(at(&out, 3, 3), [10, 20, 30, 255]);
        // A background image is not done here: no graph, the layer is drawn unfiltered.
        let mut filter = Filter::empty();
        filter.add(
            FilterEffect::Offset(kurbo::Vec2::ZERO),
            FilterInputs::single(FilterInput::Source(FilterSource::BackgroundImage)),
        );
        assert!(Graph::new(&filter, Affine::IDENTITY).is_none());
    }

    #[test]
    fn blend_multiply() {
        let mut top = pixmap(1, 1, [255, 128, 0, 255]);
        blend(&mut top, &pixmap(1, 1, [128, 128, 128, 255]), Mix::Multiply);
        assert_eq!(at(&top, 0, 0), [128, 64, 0, 255]);
    }

    /// pyro: the luminosity mode keeps the backdrop's colour at the source's luminosity.
    #[test]
    fn blend_luminosity() {
        let mut top = pixmap(1, 1, [255, 255, 255, 255]);
        blend(&mut top, &pixmap(1, 1, [255, 0, 0, 255]), Mix::Luminosity);
        assert_eq!(at(&top, 0, 0), [255, 255, 255, 255]);
        let mut top = pixmap(1, 1, [0, 0, 0, 255]);
        blend(&mut top, &pixmap(1, 1, [255, 0, 0, 255]), Mix::Luminosity);
        assert_eq!(at(&top, 0, 0), [0, 0, 0, 255]);
    }

    /// pyro: `feTile` tiles the rectangle of the crop it reads; a lighting node takes its colour from
    /// the flood that is its second input; an `feImage` node reads the image given for it.
    #[test]
    fn tiles_lighting_and_images() {
        let mut filter = Filter::empty();
        let cropped = filter.add(
            FilterEffect::Image(ExternalImageSource {
                image_id: CROP_IMAGE,
                transform: Some([0.0, 0.0, 2.0, 10.0, 0.0, 0.0]),
            }),
            FilterInputs::single(FilterInput::Source(FilterSource::SourceGraphic)),
        );
        filter.add(FilterEffect::Tile, FilterInputs::single(FilterInput::Result(cropped)));
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        let mut source = Pixmap::new(10, 10);
        source.data_as_u8_slice_mut()[4..8].copy_from_slice(&[0, 0, 255, 255]);
        let out = graph.render(source, &[], REGION);
        assert_eq!(at(&out, 7, 0), [0, 0, 255, 255], "column 1 repeats every 2");
        assert_eq!(at(&out, 8, 0), [0; 4]);

        let mut filter = Filter::empty();
        let color = filter.add(FilterEffect::Flood(AlphaColor::from_rgb8(0, 255, 128)), FilterInputs::NONE);
        filter.add(
            FilterEffect::DiffuseLighting(anyrender::filters::lighting::DiffuseLightingFilter {
                surface_scale: 1.0,
                diffuse_constant: 1.0,
                kernel_unit_length: 1.0,
                light_source: LightSource::Distant(anyrender::filters::lighting::DistantLightSource {
                    azimuth: 0.0,
                    elevation: 90.0,
                }),
            }),
            FilterInputs::dual(FilterInput::Source(FilterSource::SourceGraphic), FilterInput::Result(color)),
        );
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        let out = graph.render(pixmap(10, 10, [0, 0, 0, 255]), &[], REGION);
        assert_eq!(at(&out, 5, 5), [0, 255, 128, 255]);

        let mut filter = Filter::empty();
        filter.add(
            FilterEffect::Image(ExternalImageSource {
                image_id: IMAGE_SOURCE,
                transform: Some([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            }),
            FilterInputs::NONE,
        );
        let graph = Graph::new(&filter, Affine::IDENTITY).unwrap();
        assert!(graph.reads_images());
        let out = graph.render(Pixmap::new(10, 10), &[None, Some(pixmap(10, 10, [0, 128, 0, 255]))], REGION);
        assert_eq!(at(&out, 9, 9), [0, 128, 0, 255]);
    }

    /// pyro: a displacement in a scaled user space moves by the scaled amount (WPT
    /// fedisplacementmap-non-uniform-transform-001).
    #[test]
    fn displacement_under_a_scale() {
        let mut filter = Filter::empty();
        let map = filter.add(FilterEffect::Flood(AlphaColor::from_rgb8(255, 255, 0)), FilterInputs::NONE);
        filter.add(
            FilterEffect::DisplacementMap(anyrender::filters::displacement::DisplacementMapFilter {
                scale: 4.0,
                x_channel: ColorChannel::Red,
                y_channel: ColorChannel::Green,
            }),
            FilterInputs::dual(FilterInput::Source(FilterSource::SourceGraphic), FilterInput::Result(map)),
        );
        let graph = Graph::new(&filter, Affine::scale_non_uniform(2.0, 1.0)).unwrap();
        let mut source = Pixmap::new(10, 10);
        let i = (3 * 10 + 6) * 4;
        source.data_as_u8_slice_mut()[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
        // User (2, 2) is device (4, 2): the pixel at (2, 1) reads (6, 3).
        let out = graph.render(source, &[], REGION);
        assert_eq!(at(&out, 2, 1), [0, 0, 255, 255]);
    }
}
