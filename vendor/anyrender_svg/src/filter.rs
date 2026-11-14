// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: SVG filters (usvg's) as anyrender filter graphs, which anyrender_vello_cpu applies to a
//! filter layer's content (its `graph` module: the output is the last node, a subregion is a crop
//! node). Primitives it does not do (`feImage`, `feTile`, `feTurbulence`, `feMorphology`,
//! `feConvolveMatrix`, `feDisplacementMap`, lighting, a blur of two deviations) leave the whole
//! filter out: the group is drawn unfiltered, as before.
//!
//! Each primitive works in its `color-interpolation-filters` space, as Chromium's filters do: an
//! input in the other space is read through a `feComponentTransfer` converting it (sRGB tables),
//! a value converted back is the value it came from, a flood is drawn in the space of whatever
//! reads it, and the output is converted to sRGB. `feOffset` moves its input as it is.

use std::collections::HashMap;

use anyrender::Filter;
use anyrender::filters::color_transformation::ColorMatrix;
use anyrender::filters::component_transfer::{
    ComponentTransferFilter, GammaTransferFunction, LinearTransferFunction, TransferFunction,
};
use anyrender::filters::composite::{ArithmeticCompositeOperator, CompositeOperator};
use anyrender::filters::{ExternalImageSource, FilterEffect, FilterId, FilterInput, FilterInputs, FilterSource};
use kurbo::Vec2;
use peniko::color::AlphaColor;
use usvg::filter::{self as svg, ColorInterpolation, ColorMatrixKind, Input, Kind};

/// The image id anyrender_vello_cpu reads as a crop of its input.
use anyrender_vello_cpu::FILTER_CROP_IMAGE as CROP_IMAGE;

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
            Some([x0, y0, x1, y1]) => self.add(
                FilterEffect::Image(ExternalImageSource {
                    image_id: CROP_IMAGE,
                    transform: Some([x0, y0, x1, y1, 0.0, 0.0]),
                }),
                FilterInputs::single(key.into()),
            ),
            None => key,
        }
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
                let [r, g, b] = match space {
                    Space::Srgb => [r, g, b],
                    Space::Linear => [to_linear(r), to_linear(g), to_linear(b)],
                };
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

/// `filter` as an anyrender filter graph, or `None` when a primitive is one anyrender_vello_cpu
/// does not do.
pub(crate) fn convert(filter: &svg::Filter) -> Option<Filter> {
    let region = filter.rect();
    let mut b =
        Builder { filter: Filter::empty(), converted: HashMap::new(), origins: HashMap::new(), floods: HashMap::new() };
    let mut results: Vec<(&str, Value)> = Vec::new();
    for (index, primitive) in filter.primitives().iter().enumerate() {
        let space = match primitive.color_interpolation() {
            ColorInterpolation::SRGB => Space::Srgb,
            ColorInterpolation::LinearRGB => Space::Linear,
        };
        // A name refers to the latest result of that name before the primitive (usvg makes a
        // missing one the previous result, or the source).
        let lookup = |input: &Input| match input {
            Input::SourceGraphic => Value::Pixels(Key::Source, Some(Space::Srgb)),
            Input::SourceAlpha => Value::Pixels(Key::SourceAlpha, None),
            Input::Reference(name) => results
                .iter()
                .rev()
                .find(|(result, _)| result == name)
                .map_or(Value::Pixels(Key::Source, Some(Space::Srgb)), |(_, value)| *value),
        };
        // The subregion, where it leaves part of the filter region out.
        let sub = primitive.rect();
        let crop = (sub.left() > region.left() + 1e-3
            || sub.top() > region.top() + 1e-3
            || sub.right() < region.right() - 1e-3
            || sub.bottom() < region.bottom() - 1e-3)
            .then(|| [sub.left(), sub.top(), sub.right(), sub.bottom()]);
        let value = match primitive.kind() {
            // usvg stands a transparent black flood in for a primitive it could not read
            // (`feConvolveMatrix` of a zero divisor, lighting without a light source): the filter
            // is left out, as one of a primitive anyrender_vello_cpu does not do.
            Kind::Flood(flood) if flood.opacity().get() == 0.0 && flood.color() == usvg::Color::black() => {
                return None;
            }
            Kind::Flood(flood) => Value::Flood(index, color(flood.color(), flood.opacity().get()), crop),
            Kind::Offset(offset) => {
                let (input, from) = b.read_as_is(lookup(offset.input()), space);
                let moved = b.add(
                    FilterEffect::Offset(Vec2::new(f64::from(offset.dx()), f64::from(offset.dy()))),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(moved, crop), from)
            }
            Kind::ColorMatrix(matrix) => {
                let input = b.read(lookup(matrix.input()), space);
                let node = b.add(
                    FilterEffect::ColorMatrix(ColorMatrix(color_matrix(matrix.kind())?)),
                    FilterInputs::single(input.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::ComponentTransfer(transfer) => {
                let input = b.read(lookup(transfer.input()), space);
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
                if x != y {
                    return None;
                }
                let input = b.read(lookup(blur.input()), space);
                let node = b.add(FilterEffect::blur(x), FilterInputs::single(input.into()));
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            Kind::DropShadow(shadow) => {
                let (x, y) = (shadow.std_dev_x().get(), shadow.std_dev_y().get());
                if x != y {
                    return None;
                }
                let input = b.read(lookup(shadow.input()), space);
                let [r, g, bl, a] = color(shadow.color(), shadow.opacity().get());
                let [r, g, bl] = match space {
                    Space::Srgb => [r, g, bl],
                    Space::Linear => [to_linear(r), to_linear(g), to_linear(bl)],
                };
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
                    let top = b.read(lookup(input), space);
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
                let top = b.read(lookup(composite.input1()), space);
                let under = b.read(lookup(composite.input2()), space);
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
                if matches!(
                    blend.mode(),
                    usvg::BlendMode::Hue
                        | usvg::BlendMode::Saturation
                        | usvg::BlendMode::Color
                        | usvg::BlendMode::Luminosity
                ) {
                    return None;
                }
                let top = b.read(lookup(blend.input1()), space);
                let under = b.read(lookup(blend.input2()), space);
                let node = b.add(
                    FilterEffect::Blend(crate::util::to_mix(blend.mode())),
                    FilterInputs::dual(top.into(), under.into()),
                );
                Value::Pixels(b.cropped(node, crop), Some(space))
            }
            _ => return None,
        };
        results.push((primitive.result(), value));
    }
    let (_, output) = *results.last()?;
    let output = b.read(output, Space::Srgb);
    // The output is the last node: anything else (the source, an earlier node) is passed through
    // one more.
    let nodes = b.filter.nodes().len();
    if nodes == 0 || output != Key::Node((nodes - 1) as u16) {
        b.add(FilterEffect::Offset(Vec2::ZERO), FilterInputs::single(output.into()));
    }
    Some(b.filter)
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
        let filter = convert(&first_filter(&tree)).unwrap();
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
        let filter = convert(&first_filter(&tree)).unwrap();
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

    /// Unsupported primitives leave the filter out; a subregion becomes a crop.
    #[test]
    fn unsupported_primitives_and_subregions() {
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f"><feTurbulence baseFrequency="0.1"/></filter>
                <rect width="50" height="50" fill="red" filter="url(#f)"/></svg>"#,
        );
        assert!(convert(&first_filter(&tree)).is_none());
        let tree = parse(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
                <filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100">
                  <feFlood flood-color="green" x="10" y="10" width="20" height="20"/>
                </filter>
                <rect width="50" height="50" fill="red" filter="url(#f)"/></svg>"#,
        );
        let filter = convert(&first_filter(&tree)).unwrap();
        let last = filter.nodes().last().unwrap();
        let FilterEffect::Image(image) = &last.effect else { panic!("{last:?}") };
        assert_eq!(image.image_id, CROP_IMAGE);
        assert_eq!(image.transform, Some([10.0, 10.0, 30.0, 30.0, 0.0, 0.0]));
    }
}
