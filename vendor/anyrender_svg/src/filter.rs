// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: SVG filters (usvg's) as anyrender filter graphs, which anyrender_vello_cpu applies to a
//! filter layer's content (its `graph` module: the output is the last node, a subregion is a crop
//! node, and the conventions it documents for what anyrender's filter types cannot say). A filter
//! with a primitive it does not do (`feDropShadow` of two deviations, a primitive usvg could not
//! read) is left out: the group is drawn unfiltered, as before.
//!
//! Each primitive works in its `color-interpolation-filters` space, as Chromium's filters do: an
//! input in the other space is read through a `feComponentTransfer` converting it (sRGB tables),
//! a value converted back is the value it came from, a flood is drawn in the space of whatever
//! reads it, and the output is converted to sRGB. `feOffset`, `feTile` and the input that
//! `feDisplacementMap` moves are taken as they are; `feImage` is sRGB.
//!
//! Tainted primitives (Filter Effects, "tainted filter primitives"): a flood, drop shadow or
//! lighting colour computed from `currentcolor` (which a `:visited` link's colour can be) is
//! tainted, and so is every result made from a tainted one. usvg resolves `currentcolor`, so the
//! caller says which primitives are tainted ([`PrimitiveNotes`]). An `feDisplacementMap` whose map
//! is tainted passes its input on unmoved, as Chromium's does: the map may not move pixels.

use std::collections::HashMap;
use std::sync::Arc;

use anyrender::filters::color_transformation::ColorMatrix;
use anyrender::filters::component_transfer::{
    ComponentTransferFilter, GammaTransferFunction, LinearTransferFunction, TransferFunction,
};
use anyrender::filters::composite::{ArithmeticCompositeOperator, CompositeOperator};
use anyrender::filters::convolution::ConvolutionKernel;
use anyrender::filters::displacement::{ColorChannel, DisplacementMapFilter};
use anyrender::filters::lighting::{
    DiffuseLightingFilter, DistantLightSource, LightSource, PointLightSource, SpecularLightingFilter, SpotLightSource,
};
use anyrender::filters::morphology::{MorphologyFilter, MorphologyOperator};
use anyrender::filters::{ExternalImageSource, FilterEffect, FilterId, FilterInput, FilterInputs, FilterSource};
use anyrender::{Filter, PaintScene};
use kurbo::{Affine, Rect, Vec2};
use peniko::color::AlphaColor;
use peniko::{BlendMode, Fill};
use usvg::filter::{self as svg, ColorInterpolation, ColorMatrixKind, Input, Kind};

/// The image ids anyrender_vello_cpu reads as a crop of its input, an `feImage`'s content, a blur
/// of two deviations, `feMorphology` of two radii and `feTurbulence`.
use anyrender_vello_cpu::{
    FILTER_BLUR_XY as BLUR_XY, FILTER_CROP_IMAGE as CROP_IMAGE, FILTER_IMAGE_SOURCE as IMAGE_SOURCE,
    FILTER_MORPHOLOGY as MORPHOLOGY, FILTER_TURBULENCE as TURBULENCE,
};

/// pyro: what the document knows of a filter primitive that usvg's tree does not keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimitiveNotes {
    /// Its colour (`flood-color`, `lighting-color`) is computed from `currentcolor`.
    pub tainted: bool,
    /// It is a lighting primitive without a light source: its result is transparent black (usvg
    /// stands a transparent flood in for it, which otherwise means a primitive it could not read).
    pub no_light: bool,
}

/// pyro: what the document knows of the primitives of the filters of a tree (by the `<filter>`'s
/// id, then by primitive), for [`crate::render_svg_tree_with_notes`].
pub type FilterNotes = std::collections::HashMap<String, Vec<PrimitiveNotes>>;

std::thread_local! {
    /// The notes of the tree being rendered.
    static NOTES: std::cell::RefCell<Option<FilterNotes>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `notes` as the notes of the filters it renders (the previous ones restored after).
pub(crate) fn with_notes<R>(notes: FilterNotes, f: impl FnOnce() -> R) -> R {
    let previous = NOTES.with(|n| n.replace(Some(notes)));
    let result = f();
    NOTES.with(|n| *n.borrow_mut() = previous);
    result
}

/// What the document knows of primitive `index` of the filter `id` (nothing when it was not told).
pub(crate) fn notes(id: &str, index: usize) -> PrimitiveNotes {
    NOTES.with(|n| {
        n.borrow()
            .as_ref()
            .and_then(|notes| notes.get(id))
            .and_then(|notes| notes.get(index).copied())
            .unwrap_or_default()
    })
}

/// pyro: an SVG filter (usvg's) as an anyrender filter graph: a layer clipped to the filter
/// region, its filter the graph, holding the content the filter applies to and, before it, what
/// its `feImage`s draw ([`FilterGraph::push_layer`]).
#[derive(Clone, Debug)]
pub struct FilterGraph {
    /// The graph, in the filter's user space.
    pub filter: Arc<Filter>,
    /// The filter region, in its user space.
    pub region: Rect,
    /// Whether the filter's result is tainted (a filter after it in a list then reads a tainted
    /// source).
    pub tainted: bool,
    source: Arc<svg::Filter>,
    /// The `feImage` primitives the graph reads (their indices), by their number in it.
    images: Vec<usize>,
}

impl FilterGraph {
    /// `filter` as a graph, or `None` when it has a primitive anyrender_vello_cpu does not do.
    /// `notes` says what the document knows of its primitives (by index); `source_tainted`,
    /// whether the source graphic is tainted (a filter list's earlier functions made it so).
    pub fn new(
        filter: &Arc<svg::Filter>,
        notes: &dyn Fn(usize) -> PrimitiveNotes,
        source_tainted: bool,
    ) -> Option<Self> {
        let region = filter.rect();
        let (graph, images, tainted) = convert(filter, notes, source_tainted)?;
        Some(Self {
            filter: Arc::new(graph),
            region: Rect::new(
                f64::from(region.left()),
                f64::from(region.top()),
                f64::from(region.right()),
                f64::from(region.bottom()),
            ),
            tainted,
            source: filter.clone(),
            images,
        })
    }

    /// Pushes the filter's layer under `transform` (the filter's user space to the scene's) and
    /// draws what its `feImage`s show, each in a layer of its own that only the graph reads (see
    /// anyrender_vello_cpu's `FILTER_IMAGE_SOURCE`). The content to filter follows, then a pop.
    pub fn push_layer<S: PaintScene>(&self, scene: &mut S, transform: Affine) {
        scene.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            1.0,
            transform,
            &self.region,
            Some(self.filter.clone()),
            None,
        );
        for (k, &index) in self.images.iter().enumerate() {
            let primitive = &self.source.primitives()[index];
            let Kind::Image(image) = primitive.kind() else { continue };
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                0.0,
                transform,
                &self.region,
                Some(Arc::new(Filter::single(FilterEffect::Image(ExternalImageSource {
                    image_id: IMAGE_SOURCE,
                    transform: Some([k as f32, 1.0, 0.0, 0.0, 0.0, 0.0]),
                })))),
                None,
            );
            // At the subregion's corner: an image fills the subregion (usvg laid it out at the
            // origin), and an element is drawn from there, as Blink places it (`FEImage`: WPT
            // feimage-element-ref-geometry).
            let sub = primitive.rect();
            let base = Affine::translate((f64::from(sub.x()), f64::from(sub.y())));
            let ctx = crate::render::Ctx { cull: transform.transform_rect_bbox(self.region) };
            crate::render::render_group(
                scene,
                image.root(),
                base,
                transform,
                &mut crate::util::default_error_handler,
                &ctx,
            );
            scene.pop_layer();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Space {
    Srgb,
    Linear,
}

/// An input of the graph or one of its nodes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    Source,
    SourceAlpha,
    Node(u16),
}

impl From<Key> for FilterInput {
    fn from(key: Key) -> Self {
        match key {
            Key::Source => FilterInput::Source(FilterSource::SourceGraphic),
            Key::SourceAlpha => FilterInput::Source(FilterSource::SourceAlpha),
            Key::Node(id) => FilterInput::Result(FilterId(id)),
        }
    }
}

/// A primitive's result.
#[derive(Clone, Copy, Debug)]
enum Value {
    /// Pixels in a colour space (`None`: in either, as an alpha alone).
    Pixels(Key, Option<Space>),
    /// A flood of an unpremultiplied sRGB colour, drawn when read, by the primitive's index, with
    /// its subregion's crop.
    Flood(usize, [f32; 4], Option<[f32; 4]>),
}

/// A result as a later primitive reads it: its value, its subregion (`[x0, y0, x1, y1]`) and
/// whether it is tainted.
#[derive(Clone, Copy, Debug)]
struct Read {
    value: Value,
    subregion: [f32; 4],
    tainted: bool,
}

struct Builder {
    filter: Filter,
    /// Values converted to a space, by the value and the space.
    converted: HashMap<(Key, Space), Key>,
    /// What each conversion converted, and from which space.
    origins: HashMap<Key, (Key, Space)>,
    /// Floods drawn, by primitive and space.
    floods: HashMap<(usize, Space), Key>,
}

impl Builder {
    fn add(&mut self, effect: FilterEffect, inputs: FilterInputs) -> Key {
        Key::Node(self.filter.add(effect, inputs).0)
    }

    /// `key` cropped to `crop` (`[x0, y0, x1, y1]`), if there is one.
    fn cropped(&mut self, key: Key, crop: Option<[f32; 4]>) -> Key {
        match crop {
            Some(rect) => self.crop(key, rect),
            None => key,
        }
    }

    fn crop(&mut self, key: Key, [x0, y0, x1, y1]: [f32; 4]) -> Key {
        self.add(
            FilterEffect::Image(ExternalImageSource {
                image_id: CROP_IMAGE,
                transform: Some([x0, y0, x1, y1, 0.0, 0.0]),
            }),
            FilterInputs::single(key.into()),
        )
    }

    /// An `Image` node of one of anyrender_vello_cpu's ids, with its parameters.
    fn op(&mut self, id: u32, params: [f32; 6], inputs: FilterInputs) -> Key {
        self.add(FilterEffect::Image(ExternalImageSource { image_id: id, transform: Some(params) }), inputs)
    }

    /// `value` with its colours in `space`.
    fn read(&mut self, value: Value, space: Space) -> Key {
        match value {
            Value::Pixels(key, None) => key,
            Value::Pixels(key, Some(from)) if from == space => key,
            Value::Pixels(key, Some(from)) => {
                if let Some(&(origin, origin_space)) = self.origins.get(&key)
                    && origin_space == space
                {
                    return origin;
                }
                if let Some(&converted) = self.converted.get(&(key, space)) {
                    return converted;
                }
                let converted =
                    self.add(FilterEffect::ComponentTransfer(conversion(space)), FilterInputs::single(key.into()));
                self.converted.insert((key, space), converted);
                self.origins.insert(converted, (key, from));
                converted
            }
            Value::Flood(index, color, crop) => {
                if let Some(&key) = self.floods.get(&(index, space)) {
                    return key;
                }
                let [r, g, b, a] = color;
                let [r, g, b] = in_space([r, g, b], space);
                let flood = self.add(FilterEffect::Flood(AlphaColor::new([r, g, b, a])), FilterInputs::NONE);
                let key = self.cropped(flood, crop);
                self.floods.insert((index, space), key);
                key
            }
        }
    }

    /// `value` as it is, for an operation that does not see colours (`feOffset`): its key and
    /// space (a flood drawn in `space`).
    fn read_as_is(&mut self, value: Value, space: Space) -> (Key, Option<Space>) {
        match value {
            Value::Pixels(key, from) => (key, from),
            Value::Flood(..) => (self.read(value, space), Some(space)),
        }
    }
}

/// An sRGB colour's channels in `space`.
fn in_space([r, g, b]: [f32; 3], space: Space) -> [f32; 3] {
    match space {
        Space::Srgb => [r, g, b],
        Space::Linear => [to_linear(r), to_linear(g), to_linear(b)],
    }
}

/// `filter` as an anyrender filter graph, the `feImage` primitives it reads (their indices) and
/// whether its result is tainted; `None` when a primitive is one anyrender_vello_cpu does not do.
fn convert(
    filter: &svg::Filter,
    notes: &dyn Fn(usize) -> PrimitiveNotes,
    source_tainted: bool,
) -> Option<(Filter, Vec<usize>, bool)> {
    let region = filter.rect();
    let whole = [region.left(), region.top(), region.right(), region.bottom()];
    let mut b =
        Builder { filter: Filter::empty(), converted: HashMap::new(), origins: HashMap::new(), floods: HashMap::new() };
    let mut results: Vec<(&str, Read)> = Vec::new();
    let mut images: Vec<usize> = Vec::new();
    for (index, primitive) in filter.primitives().iter().enumerate() {
        let space = match primitive.color_interpolation() {
            ColorInterpolation::SRGB => Space::Srgb,
            ColorInterpolation::LinearRGB => Space::Linear,
        };
        let note = notes(index);
        let source =
            Read { value: Value::Pixels(Key::Source, Some(Space::Srgb)), subregion: whole, tainted: source_tainted };
        // A name refers to the latest result of that name before the primitive (usvg makes a
        // missing one the previous result, or the source).
        let lookup = |input: &Input| match input {
            Input::SourceGraphic => source,
            Input::SourceAlpha => Read { value: Value::Pixels(Key::SourceAlpha, None), ..source },
            Input::Reference(name) => {
                results.iter().rev().find(|(result, _)| result == name).map_or(source, |(_, read)| *read)
            }
        };
        // The subregion, where it leaves part of the filter region out.
        let sub = primitive.rect();
        let subregion = [sub.left(), sub.top(), sub.right(), sub.bottom()];
        let crop = (sub.left() > region.left() + 1e-3
            || sub.top() > region.top() + 1e-3
            || sub.right() < region.right() - 1e-3
            || sub.bottom() < region.bottom() - 1e-3)
            .then_some(subregion);
        let mut tainted = note.tainted;
        let value = match primitive.kind() {
            // usvg stands a transparent black flood in for a primitive it could not read
            // (`feConvolveMatrix` of a zero divisor, an `feImage` it could not load, lighting without
            // a light source): the filter is left out, as one of a primitive anyrender_vello_cpu
            // does not do; but lighting without a light is transparent black, as in Chromium.
            Kind::Flood(flood)
                if flood.opacity().get() == 0.0 && flood.color() == usvg::Color::black() && !note.no_light =>
            {
                return None;
            }
            Kind::Flood(flood) => Value::Flood(index, color(flood.color(), flood.opacity().get()), crop),
            Kind::Offset(offset) => {
                let input = lookup(offset.input());
                tainted |= input.tainted;
                let (input, from) = b.read_as_is(input.value, space);
                let moved = b.add(
                    FilterEffect::Offset(Vec2::new(f64::from(offset.dx()), f64::from(offset.dy()))),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(moved, crop), from)
            }
            Kind::ColorMatrix(matrix) => {
                let input = lookup(matrix.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let node = b.add(
                    FilterEffect::ColorMatrix(ColorMatrix(color_matrix(matrix.kind())?)),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::ComponentTransfer(transfer) => {
                let input = lookup(transfer.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let node = b.add(
                    FilterEffect::ComponentTransfer(ComponentTransferFilter {
                        red_function: transfer_function(transfer.func_r()),
                        green_function: transfer_function(transfer.func_g()),
                        blue_function: transfer_function(transfer.func_b()),
                        alpha_function: transfer_function(transfer.func_a()),
                    }),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::GaussianBlur(blur) => {
                let (x, y) = (blur.std_dev_x().get(), blur.std_dev_y().get());
                let input = lookup(blur.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let node = if x == y {
                    b.add(FilterEffect::blur(x), FilterInputs::single(input.into()))
                } else {
                    b.op(BLUR_XY, [x, y, 0.0, 0.0, 0.0, 0.0], FilterInputs::single(input.into()))
                };
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::DropShadow(shadow) => {
                let (x, y) = (shadow.std_dev_x().get(), shadow.std_dev_y().get());
                if x != y {
                    return None;
                }
                let input = lookup(shadow.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let [r, g, bl, a] = color(shadow.color(), shadow.opacity().get());
                let [r, g, bl] = in_space([r, g, bl], space);
                let node = b.add(
                    FilterEffect::drop_shadow(shadow.dx(), shadow.dy(), x, AlphaColor::new([r, g, bl, a])),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            // The inputs over one another, the first at the bottom.
            Kind::Merge(merge) => {
                let mut merged: Option<Key> = None;
                for input in merge.inputs() {
                    let input = lookup(input);
                    tainted |= input.tainted;
                    let top = b.read(input.value, space);
                    merged = Some(match merged {
                        None => top,
                        Some(under) => b.add(
                            FilterEffect::Composite(CompositeOperator::Over),
                            FilterInputs::dual(top.into(), under.into()),
                        ),
                    });
                }
                match merged {
                    Some(key) => Value::Pixels(b.cropped(key, crop), Some(space)),
                    None => Value::Flood(index, [0.0; 4], crop),
                }
            }
            Kind::Composite(composite) => {
                let (top, under) = (lookup(composite.input1()), lookup(composite.input2()));
                tainted |= top.tainted || under.tainted;
                let top = b.read(top.value, space);
                let under = b.read(under.value, space);
                let operator = match composite.operator() {
                    svg::CompositeOperator::Over => CompositeOperator::Over,
                    svg::CompositeOperator::In => CompositeOperator::In,
                    svg::CompositeOperator::Out => CompositeOperator::Out,
                    svg::CompositeOperator::Atop => CompositeOperator::Atop,
                    svg::CompositeOperator::Xor => CompositeOperator::Xor,
                    svg::CompositeOperator::Arithmetic { k1, k2, k3, k4 } => {
                        CompositeOperator::Arithmetic(ArithmeticCompositeOperator { k1, k2, k3, k4 })
                    }
                };
                let node = b.add(FilterEffect::Composite(operator), FilterInputs::dual(top.into(), under.into()));
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::Blend(blend) => {
                let (top, under) = (lookup(blend.input1()), lookup(blend.input2()));
                tainted |= top.tainted || under.tainted;
                let top = b.read(top.value, space);
                let under = b.read(under.value, space);
                let node = b.add(
                    FilterEffect::Blend(crate::util::to_mix(blend.mode())),
                    FilterInputs::dual(top.into(), under.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            // The map moves the input as it is; read through a tainted map, the input passes on
            // unmoved.
            Kind::DisplacementMap(displacement) => {
                let (input, map) = (lookup(displacement.input1()), lookup(displacement.input2()));
                tainted |= input.tainted;
                if map.tainted {
                    input.value
                } else {
                    let (moved, from) = b.read_as_is(input.value, space);
                    let map = b.read(map.value, space);
                    let channel = |c: svg::ColorChannel| match c {
                        svg::ColorChannel::R => ColorChannel::Red,
                        svg::ColorChannel::G => ColorChannel::Green,
                        svg::ColorChannel::B => ColorChannel::Blue,
                        svg::ColorChannel::A => ColorChannel::Alpha,
                    };
                    let node = b.add(
                        FilterEffect::DisplacementMap(DisplacementMapFilter {
                            scale: displacement.scale(),
                            x_channel: channel(displacement.x_channel_selector()),
                            y_channel: channel(displacement.y_channel_selector()),
                        }),
                        FilterInputs::dual(moved.into(), map.into()),
                    );
                    Value::Pixels(b.cropped(node, crop), from)
                }
            }
            // The input's subregion repeated: a crop to it, which the tile reads its rectangle from.
            Kind::Tile(tile) => {
                let input = lookup(tile.input());
                tainted |= input.tainted;
                let (key, from) = b.read_as_is(input.value, space);
                let cropped = b.crop(key, input.subregion);
                let node = b.add(FilterEffect::Tile, FilterInputs::single(cropped.into()));
                Value::Pixels(b.cropped(node, crop), from)
            }
            Kind::Morphology(morphology) => {
                let input = lookup(morphology.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let (rx, ry) = (morphology.radius_x().get(), morphology.radius_y().get());
                let dilate = morphology.operator() == svg::MorphologyOperator::Dilate;
                let node = if rx == ry {
                    let operator = if dilate { MorphologyOperator::Dilate } else { MorphologyOperator::Erode };
                    b.add(
                        FilterEffect::Morphology(MorphologyFilter { operator, radius: rx }),
                        FilterInputs::single(input.into()),
                    )
                } else {
                    b.op(
                        MORPHOLOGY,
                        [rx, ry, f32::from(u8::from(dilate)), 0.0, 0.0, 0.0],
                        FilterInputs::single(input.into()),
                    )
                };
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::ConvolveMatrix(convolve) => {
                let input = lookup(convolve.input());
                tainted |= input.tainted;
                let input = b.read(input.value, space);
                let m = convolve.matrix();
                let mut values: Vec<f32> = m.data().to_vec();
                let edge = match convolve.edge_mode() {
                    svg::EdgeMode::Duplicate => 0.0,
                    svg::EdgeMode::Wrap => 1.0,
                    svg::EdgeMode::None => 2.0,
                };
                values.extend([m.rows() as f32, m.target_x() as f32, m.target_y() as f32, edge]);
                let node = b.add(
                    FilterEffect::ConvolveMatrix(ConvolutionKernel {
                        size: m.columns(),
                        values,
                        divisor: convolve.divisor().get(),
                        bias: convolve.bias(),
                        preserve_alpha: convolve.preserve_alpha(),
                    }),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::Turbulence(turbulence) => {
                let fractal = turbulence.kind() == svg::TurbulenceKind::FractalNoise;
                let node = b.op(
                    TURBULENCE,
                    [
                        turbulence.base_frequency_x().get(),
                        turbulence.base_frequency_y().get(),
                        turbulence.num_octaves() as f32,
                        turbulence.seed() as f32,
                        f32::from(u8::from(fractal)),
                        0.0,
                    ],
                    FilterInputs::NONE,
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            // The input's alpha is the surface; the lighting colour is a flood (in the primitive's
            // space) the node reads as its second input.
            Kind::DiffuseLighting(lighting) => {
                let input = lookup(lighting.input());
                tainted |= input.tainted;
                let (surface, _) = b.read_as_is(input.value, space);
                let color = lighting_color(&mut b, lighting.lighting_color(), space);
                let node = b.add(
                    FilterEffect::DiffuseLighting(DiffuseLightingFilter {
                        surface_scale: lighting.surface_scale(),
                        diffuse_constant: lighting.diffuse_constant(),
                        kernel_unit_length: 1.0,
                        light_source: light_source(lighting.light_source()),
                    }),
                    FilterInputs::dual(surface.into(), color.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::SpecularLighting(lighting) => {
                let input = lookup(lighting.input());
                tainted |= input.tainted;
                let (surface, _) = b.read_as_is(input.value, space);
                let color = lighting_color(&mut b, lighting.lighting_color(), space);
                let node = b.add(
                    FilterEffect::SpecularLighting(SpecularLightingFilter {
                        surface_scale: lighting.surface_scale(),
                        specular_constant: lighting.specular_constant(),
                        specular_exponent: lighting.specular_exponent(),
                        kernel_unit_length: 1.0,
                        light_source: light_source(lighting.light_source()),
                    }),
                    FilterInputs::dual(surface.into(), color.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::Image(_) => {
                let k = images.len();
                images.push(index);
                let node = b.op(IMAGE_SOURCE, [k as f32, 0.0, 0.0, 0.0, 0.0, 0.0], FilterInputs::NONE);
                Value::Pixels(b.cropped(node, crop), Some(Space::Srgb))
            }
        };
        results.push((primitive.result(), Read { value, subregion, tainted }));
    }
    let (_, output) = *results.last()?;
    let output_tainted = output.tainted;
    let output = b.read(output.value, Space::Srgb);
    // The output is the last node: anything else (the source, an earlier node) is passed through
    // one more.
    let nodes = b.filter.nodes().len();
    if nodes == 0 || output != Key::Node((nodes - 1) as u16) {
        b.add(FilterEffect::Offset(Vec2::ZERO), FilterInputs::single(output.into()));
    }
    Some((b.filter, images, output_tainted))
}

/// A lighting colour as the flood node that carries it (opaque, in `space`).
fn lighting_color(b: &mut Builder, color: usvg::Color, space: Space) -> Key {
    let [r, g, bl, _] = self::color(color, 1.0);
    let [r, g, bl] = in_space([r, g, bl], space);
    b.add(FilterEffect::Flood(AlphaColor::new([r, g, bl, 1.0])), FilterInputs::NONE)
}

fn light_source(light: svg::LightSource) -> LightSource {
    match light {
        svg::LightSource::DistantLight(l) => {
            LightSource::Distant(DistantLightSource { azimuth: l.azimuth, elevation: l.elevation })
        }
        svg::LightSource::PointLight(l) => LightSource::Point(PointLightSource { x: l.x, y: l.y, z: l.z }),
        svg::LightSource::SpotLight(l) => LightSource::Spot(SpotLightSource {
            x: l.x,
            y: l.y,
            z: l.z,
            points_at_x: l.points_at_x,
            points_at_y: l.points_at_y,
            points_at_z: l.points_at_z,
            specular_exponent: l.specular_exponent.get(),
            limiting_cone_angle: l.limiting_cone_angle,
        }),
    }
}

/// An unpremultiplied sRGB colour with its opacity.
fn color(color: usvg::Color, opacity: f32) -> [f32; 4] {
    let c = |v: u8| f32::from(v) / 255.0;
    [c(color.red), c(color.green), c(color.blue), opacity]
}

/// `feColorMatrix`'s matrix, with Chromium's constants (`FEColorMatrix`).
fn color_matrix(kind: &ColorMatrixKind) -> Option<[f32; 20]> {
    let mut m = [0.0f32; 20];
    match kind {
        ColorMatrixKind::Matrix(values) => m.copy_from_slice(values.get(..20)?),
        ColorMatrixKind::Saturate(s) => {
            let s = s.get();
            m[0] = 0.213 + 0.787 * s;
            m[1] = 0.715 - 0.715 * s;
            m[2] = 0.072 - 0.072 * s;
            m[5] = 0.213 - 0.213 * s;
            m[6] = 0.715 + 0.285 * s;
            m[7] = 0.072 - 0.072 * s;
            m[10] = 0.213 - 0.213 * s;
            m[11] = 0.715 - 0.715 * s;
            m[12] = 0.072 + 0.928 * s;
            m[18] = 1.0;
        }
        ColorMatrixKind::HueRotate(degrees) => {
            let (sin, cos) = (degrees.to_radians()).sin_cos();
            m[0] = 0.213 + cos * 0.787 - sin * 0.213;
            m[1] = 0.715 - cos * 0.715 - sin * 0.715;
            m[2] = 0.072 - cos * 0.072 + sin * 0.928;
            m[5] = 0.213 - cos * 0.213 + sin * 0.143;
            m[6] = 0.715 + cos * 0.285 + sin * 0.140;
            m[7] = 0.072 - cos * 0.072 - sin * 0.283;
            m[10] = 0.213 - cos * 0.213 - sin * 0.787;
            m[11] = 0.715 - cos * 0.715 + sin * 0.715;
            m[12] = 0.072 + cos * 0.928 + sin * 0.072;
            m[18] = 1.0;
        }
        ColorMatrixKind::LuminanceToAlpha => {
            m[15] = 0.2125;
            m[16] = 0.7154;
            m[17] = 0.0721;
        }
    }
    Some(m)
}

fn transfer_function(function: &svg::TransferFunction) -> TransferFunction {
    match function {
        svg::TransferFunction::Identity => TransferFunction::Identity,
        svg::TransferFunction::Table(values) if values.is_empty() => TransferFunction::Identity,
        svg::TransferFunction::Table(values) => TransferFunction::Table(values.clone().into()),
        svg::TransferFunction::Discrete(values) if values.is_empty() => TransferFunction::Identity,
        svg::TransferFunction::Discrete(values) => TransferFunction::Discrete(values.clone()),
        svg::TransferFunction::Linear { slope, intercept } => {
            TransferFunction::Linear(LinearTransferFunction { slope: *slope, intercept: *intercept })
        }
        svg::TransferFunction::Gamma { amplitude, exponent, offset } => {
            TransferFunction::Gamma(GammaTransferFunction {
                amplitude: *amplitude,
                exponent: *exponent,
                offset: *offset,
            })
        }
    }
}

/// sRGB to linearRGB, of a value in 0..1.
fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// linearRGB to sRGB, of a value in 0..1.
fn to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// The conversion of the colour channels into `space` from the other: tables of 256 entries
/// into linearRGB (exact at each byte) and 1024 back (finer where the curve is steep).
fn conversion(space: Space) -> ComponentTransferFilter {
    let table: Vec<f32> = match space {
        Space::Linear => (0..256).map(|i| to_linear(i as f32 / 255.0)).collect(),
        Space::Srgb => (0..1024).map(|i| to_srgb(i as f32 / 1023.0)).collect(),
    };
    let function = TransferFunction::Table(table.into());
    ComponentTransferFilter {
        red_function: function.clone(),
        green_function: function.clone(),
        blue_function: function,
        alpha_function: TransferFunction::Identity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(svg: &str) -> usvg::Tree {
        usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap()
    }

    fn first_filter(tree: &usvg::Tree) -> std::sync::Arc<svg::Filter> {
        fn find(group: &usvg::Group) -> Option<std::sync::Arc<svg::Filter>> {
            if let Some(f) = group.filters().first() {
                return Some(f.clone());
            }
            group.children().iter().find_map(|node| match node {
                usvg::Node::Group(g) => find(g),
                _ => None,
            })
        }
        find(tree.root()).unwrap()
    }

    /// A row of sRGB colour primitives: one node each, no conversion, the last the output.
    #[test]
    fn srgb_colour_primitives_need_no_conversion() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" color-interpolation-filters="sRGB">
                  <feColorMatrix type="hueRotate" values="90"/>
                  <feComponentTransfer><feFuncG type="linear" slope="10000"/></feComponentTransfer>
                </filter>
                <rect width="100" height="100" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = graph(&first_filter(&tree)).unwrap();
        assert_eq!(filter.nodes().len(), 2);
        assert!(matches!(filter.nodes()[0].effect, FilterEffect::ColorMatrix(_)));
        assert!(matches!(filter.nodes()[1].effect, FilterEffect::ComponentTransfer(_)));
    }

    /// linearRGB (the default): the source converted in, the output converted back; a flood read
    /// in linearRGB is drawn there, and an offset moves what it reads as it is.
    #[test]
    fn linear_rgb_converts_inputs_and_the_output() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f">
                  <feFlood flood-color="green" result="flood"/>
                  <feOffset in="SourceGraphic" dx="5" result="moved"/>
                  <feComposite in="flood" in2="moved" operator="in"/>
                </filter>
                <rect width="50" height="50" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = graph(&first_filter(&tree)).unwrap();
        let kinds: Vec<&str> = filter
            .nodes()
            .iter()
            .map(|n| match n.effect {
                FilterEffect::Flood(_) => "flood",
                FilterEffect::Offset(_) => "offset",
                FilterEffect::ComponentTransfer(_) => "convert",
                FilterEffect::Composite(_) => "composite",
                _ => "other",
            })
            .collect();
        // The offset source (sRGB) is converted for the composite; the flood is drawn in
        // linearRGB; the result goes back to sRGB.
        assert_eq!(kinds, ["offset", "flood", "convert", "composite", "convert"]);
    }

    /// The graph alone, with nothing known of the primitives.
    fn graph(filter: &std::sync::Arc<svg::Filter>) -> Option<Filter> {
        convert(filter, &|_| PrimitiveNotes::default(), false).map(|(filter, ..)| filter)
    }

    /// The kinds of a graph's nodes, by short names.
    fn kinds(filter: &Filter) -> Vec<&'static str> {
        filter
            .nodes()
            .iter()
            .map(|n| match &n.effect {
                FilterEffect::Flood(_) => "flood",
                FilterEffect::Offset(_) => "offset",
                FilterEffect::DisplacementMap(_) => "displace",
                FilterEffect::Tile => "tile",
                FilterEffect::Image(image) if image.image_id == CROP_IMAGE => "crop",
                FilterEffect::Image(image) if image.image_id == IMAGE_SOURCE => "image",
                FilterEffect::Image(image) if image.image_id == TURBULENCE => "turbulence",
                FilterEffect::DiffuseLighting(_) => "diffuse",
                _ => "other",
            })
            .collect()
    }

    /// Unsupported primitives leave the filter out; a subregion becomes a crop.
    #[test]
    fn unsupported_primitives_and_subregions() {
        // usvg's stand-in for a primitive it could not read (a zero divisor).
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f"><feConvolveMatrix kernelMatrix="1 0 0 0 0 0 0 0 0" divisor="0"/></filter>
                <rect width="50" height="50" fill="red" filter="url(#f)"/></svg>"#,
        );
        assert!(graph(&first_filter(&tree)).is_none());
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100">
                  <feFlood flood-color="green" x="10" y="10" width="20" height="20"/>
                </filter>
                <rect width="50" height="50" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = graph(&first_filter(&tree)).unwrap();
        let last = filter.nodes().last().unwrap();
        let FilterEffect::Image(image) = &last.effect else { panic!("{last:?}") };
        assert_eq!(image.image_id, CROP_IMAGE);
        assert_eq!(image.transform, Some([10.0, 10.0, 30.0, 30.0, 0.0, 0.0]));
    }

    /// WPT tainting-*: a displacement map moves the source by an untainted map; through a map a
    /// `currentcolor` flood made (tainted), it passes the source on unmoved, and a tainted result
    /// stays tainted through the primitives after it.
    #[test]
    fn a_tainted_map_displaces_nothing() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" color-interpolation-filters="sRGB">
                  <feFlood flood-color="rgb(0, 255, 128)"/>
                  <feGaussianBlur stdDeviation="0"/>
                  <feDisplacementMap in="SourceGraphic" xChannelSelector="G" yChannelSelector="B" scale="100"/>
                </filter>
                <rect width="100" height="100" fill="green" filter="url(#f)"/></svg>"#,
        );
        let filter = first_filter(&tree);
        let moved = graph(&filter).unwrap();
        assert!(kinds(&moved).contains(&"displace"), "{:?}", kinds(&moved));
        let tainted_flood = |index: usize| PrimitiveNotes { tainted: index == 0, ..Default::default() };
        let (unmoved, _, tainted) = convert(&filter, &tainted_flood, false).unwrap();
        assert!(!kinds(&unmoved).contains(&"displace"), "{:?}", kinds(&unmoved));
        assert!(!tainted, "the source passed on is not tainted");
        // A tainted source (a filter list's `drop-shadow(currentcolor)` before it) taints the result.
        let (_, _, tainted) = convert(&filter, &|_| PrimitiveNotes::default(), true).unwrap();
        assert!(tainted);
    }

    /// `feTile` reads its input cropped to the input's subregion; `feImage`, `feTurbulence` and the
    /// lighting primitives (their colour a flood they read) are nodes of their own.
    #[test]
    fn tiles_images_noise_and_lighting() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100" color-interpolation-filters="sRGB">
                  <feFlood flood-color="green" width="5" height="5"/>
                  <feTile/>
                </filter>
                <rect width="100" height="100" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = graph(&first_filter(&tree)).unwrap();
        assert_eq!(kinds(&filter), ["flood", "crop", "crop", "tile"]);
        let tree = parse(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <rect id="r" width="10" height="10" fill="blue"/>
                <filter id="f" color-interpolation-filters="sRGB">
                  <feImage href="#r" result="image"/>
                  <feTurbulence baseFrequency="0.05"/>
                  <feDiffuseLighting in="image" lighting-color="red"><feDistantLight elevation="45"/></feDiffuseLighting>
                </filter>
                <rect width="100" height="100" fill="red" filter="url(#f)"/></svg>"##,
        );
        let (filter, images, _) = convert(&first_filter(&tree), &|_| PrimitiveNotes::default(), false).unwrap();
        assert_eq!(kinds(&filter), ["image", "turbulence", "flood", "diffuse"]);
        assert_eq!(images, [0]);
    }

    /// An SVG rendered as the page is (recorded, then replayed by anyrender_vello_cpu): RGBA bytes.
    fn raster(svg: &str, width: u32, height: u32) -> Vec<u8> {
        use anyrender::ImageRenderer as _;
        let tree = parse(svg);
        let mut scene = anyrender::Scene::new();
        crate::render_svg_tree(&mut scene, &tree, kurbo::Affine::IDENTITY);
        let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::with_threads(width, height, 0);
        let mut pixels = Vec::new();
        renderer.render_to_vec(|painter| painter.append_scene(scene, kurbo::Affine::IDENTITY), &mut pixels);
        pixels
    }

    fn px(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        pixels[(y * width + x) * 4..(y * width + x) * 4 + 4].try_into().unwrap()
    }

    /// `feImage` of an element draws it in the filtered element's user space (WPT
    /// feImage-with-hidden-container-defs), of an SVG image fills the subregion (WPT
    /// feimage-svg-resolution); what it draws shows only through the filter.
    #[test]
    fn images_draw_through_the_filter() {
        let pixels = raster(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <defs><g id="img"><rect x="50" width="50" height="100" fill="#008000"/></g>
                  <filter filterUnits="userSpaceOnUse" id="flt" x="0" y="0" width="100" height="100">
                    <feImage href="#img"/></filter></defs>
                <rect filter="url(#flt)" width="100" height="100"/></svg>"##,
            100,
            100,
        );
        assert_eq!(px(&pixels, 100, 75, 50), [0, 128, 0, 255]);
        assert_eq!(px(&pixels, 100, 25, 50), [0; 4], "the rect is replaced");
        let pixels = raster(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100">
                  <feImage href="data:image/svg+xml,&lt;svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'>&lt;rect x='1' width='1' height='2' fill='green'/>&lt;/svg>"/>
                </filter>
                <rect width="100" height="100" filter="url(#f)"/></svg>"#,
            100,
            100,
        );
        assert_eq!(px(&pixels, 100, 75, 50), [0, 128, 0, 255]);
        assert_eq!(px(&pixels, 100, 25, 50), [0; 4]);
    }

    /// A hidden element's filter still fills its region (WPT svg-visibility-hidden-element-with-filter-003).
    #[test]
    fn a_hidden_text_fills_its_filter_region() {
        let pixels = raster(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="150">
                <filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100">
                  <feFlood flood-color="green"/></filter>
                <text fill="red" x="20" y="20" visibility="hidden" filter="url(#f)">SVGText</text></svg>"#,
            300,
            150,
        );
        assert_eq!(px(&pixels, 300, 95, 95), [0, 128, 0, 255]);
    }

    /// Lighting without a light source is transparent black (Chromium) when the document says so;
    /// usvg's stand-in alone still leaves the filter out.
    #[test]
    fn lighting_without_a_light() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" color-interpolation-filters="sRGB"><feDiffuseLighting lighting-color="blue"/></filter>
                <rect width="100" height="100" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = first_filter(&tree);
        assert!(graph(&filter).is_none());
        let no_light = |_| PrimitiveNotes { no_light: true, ..Default::default() };
        let (graph, ..) = convert(&filter, &no_light, false).unwrap();
        let FilterEffect::Flood(color) = &graph.nodes()[0].effect else { panic!() };
        assert_eq!(color.components[3], 0.0);
    }
}
