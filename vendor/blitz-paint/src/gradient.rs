use crate::color::Color;
use color::DynamicColor;
use kurbo::{self, Affine, Point, Rect, Vec2};
use peniko::{self, ColorStop, Gradient, LinearGradientPosition, SweepGradientPosition};
use style::color::mix::{ColorInterpolationMethod, ColorMixItem};
use style::values::generics::color::ColorMixFlags;
use style::color::{AbsoluteColor, ColorSpace as StyloColorSpace};
use style::{
    OwnedSlice,
    values::{
        computed::{
            Angle, AngleOrPercentage, CSSPixelLength, Gradient as StyloGradient, LengthPercentage,
            LineDirection, Percentage,
        },
        generics::{
            NonNegative,
            color::GenericColor,
            image::{
                EndingShape, GenericCircle, GenericEllipse, GenericEndingShape, GenericGradient,
                GenericGradientItem, GradientFlags, ShapeExtent,
            },
            position::GenericPosition,
        },
        specified::{
            percentage::ToPercentage,
            position::{HorizontalPositionKeyword, VerticalPositionKeyword},
        },
    },
};

type GradientItem<T> = GenericGradientItem<GenericColor<Percentage>, T>;
type LinearGradient<'a> = (
    &'a LineDirection,
    &'a [GradientItem<LengthPercentage>],
    GradientFlags,
);
type RadialGradient<'a> = (
    &'a EndingShape<NonNegative<CSSPixelLength>, NonNegative<LengthPercentage>>,
    &'a GenericPosition<LengthPercentage, LengthPercentage>,
    &'a OwnedSlice<GenericGradientItem<GenericColor<Percentage>, LengthPercentage>>,
    GradientFlags,
);
type ConicGradient<'a> = (
    &'a Angle,
    &'a GenericPosition<LengthPercentage, LengthPercentage>,
    &'a OwnedSlice<GenericGradientItem<GenericColor<Percentage>, AngleOrPercentage>>,
    GradientFlags,
);

pub(crate) fn to_peniko_gradient(
    gradient: &StyloGradient,
    origin_rect: Rect,
    bounding_box: Rect,
    scale: f64,
    current_color: &AbsoluteColor,
) -> (peniko::Gradient, Option<Affine>) {
    // pyro: the colour interpolation method (`in hsl`, `in oklch longer hue`; Stylo makes Oklab
    // the default when a stop uses a modern colour syntax) is applied by `Ramp`, which hands
    // the renderer sRGB samples.
    match gradient {
        // https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/linear-gradient
        GenericGradient::Linear {
            direction,
            items,
            flags,
            color_interpolation_method,
            ..
        } => linear_gradient(
            (direction, items, *flags),
            *color_interpolation_method,
            origin_rect,
            bounding_box,
            scale,
            current_color,
        ),
        GenericGradient::Radial {
            shape,
            position,
            items,
            flags,
            color_interpolation_method,
            ..
        } => radial_gradient(
            (shape, position, items, *flags),
            *color_interpolation_method,
            origin_rect,
            scale,
            current_color,
        ),
        GenericGradient::Conic {
            angle,
            position,
            items,
            flags,
            color_interpolation_method,
            ..
        } => conic_gradient(
            (angle, position, items, *flags),
            *color_interpolation_method,
            origin_rect,
            current_color,
        ),
    }
}

/// A stop's position along a gradient line of `length` device px, in units of the line.
fn length_position(position: &LengthPercentage, length: f64, scale: f64) -> f64 {
    let css_length = (length / scale) as f32;
    if css_length > 0.0 && css_length.is_finite() {
        // Infinite positions (`calc(infinity * 1%)`) stay at their end of the line.
        let px = position.resolve(CSSPixelLength::new(css_length)).px();
        (px.clamp(f32::MIN, f32::MAX) as f64 / css_length as f64).clamp(-1e30, 1e30)
    } else if css_length > 0.0 {
        // A line without an end: only an infinite position is not at its start.
        let px = position.resolve(CSSPixelLength::new(f32::MAX)).px();
        if px >= f32::MAX { 1.0 } else { 0.0 }
    } else {
        // No line to measure along: everything is past its end.
        position
            .to_percentage_of(CSSPixelLength::new(0.0))
            .map_or(0.0, |percentage| percentage.to_percentage().unwrap_or(0.0) as f64)
    }
}

fn linear_gradient(
    gradient: LinearGradient,
    method: ColorInterpolationMethod,
    rect: Rect,
    bounding_box: Rect,
    scale: f64,
    current_color: &AbsoluteColor,
) -> (peniko::Gradient, Option<Affine>) {
    let (direction, items, flags) = gradient;

    // pyro: the line goes through the centre of the gradient's box (its tile), not of the
    // element's border box: a tile larger than the box (`background-size: 230px 50px` on a
    // 50px cell) or moved off it had its line shifted by the difference.
    let _ = bounding_box;
    let center = rect.center();
    let (start, end) = match direction {
        LineDirection::Angle(angle) => {
            let angle = -angle.radians64() + std::f64::consts::PI;
            let offset_length =
                rect.width() / 2.0 * angle.sin().abs() + rect.height() / 2.0 * angle.cos().abs();
            let offset_vec = Vec2::new(angle.sin(), angle.cos()) * offset_length;
            (center - offset_vec, center + offset_vec)
        }
        LineDirection::Horizontal(horizontal) => {
            let start = Point::new(rect.x0, rect.y0 + rect.height() / 2.0);
            let end = Point::new(rect.x1, rect.y0 + rect.height() / 2.0);
            match horizontal {
                HorizontalPositionKeyword::Right => (start, end),
                HorizontalPositionKeyword::Left => (end, start),
            }
        }
        LineDirection::Vertical(vertical) => {
            let start = Point::new(rect.x0 + rect.width() / 2.0, rect.y0);
            let end = Point::new(rect.x0 + rect.width() / 2.0, rect.y1);
            match vertical {
                VerticalPositionKeyword::Top => (end, start),
                VerticalPositionKeyword::Bottom => (start, end),
            }
        }
        LineDirection::Corner(horizontal, vertical) => {
            // pyro: "to <corner>" (CSS Images 3 §3.1.1): the line through the centre whose
            // perpendicular at the midpoint joins the two neighbouring corners, long enough
            // that the 0% and 100% lines pass through the corners. It was the diagonal itself,
            // which only a square's is.
            let (w, h) = (rect.width(), rect.height());
            let sign_x = match horizontal {
                HorizontalPositionKeyword::Right => 1.0,
                HorizontalPositionKeyword::Left => -1.0,
            };
            let sign_y = match vertical {
                VerticalPositionKeyword::Top => -1.0,
                VerticalPositionKeyword::Bottom => 1.0,
            };
            let norm = w.hypot(h);
            if norm > 0.0 {
                // Unit direction, perpendicular to the other diagonal.
                let dir = Vec2::new(sign_x * h / norm, sign_y * w / norm);
                let half = (w / 2.0) * dir.x.abs() + (h / 2.0) * dir.y.abs();
                let mid = rect.center();
                (mid - dir * half, mid + dir * half)
            } else {
                (rect.center(), rect.center())
            }
        }
    };

    let length = start.distance(end);
    let repeating = flags.contains(GradientFlags::REPEATING);
    let ramp = Ramp::new(items, method, current_color, |position| {
        length_position(position, length, scale)
    });

    let mut gradient = peniko::Gradient::new_linear(start, end).with_extend(if repeating {
        peniko::Extend::Repeat
    } else {
        peniko::Extend::Pad
    });
    let mut samples = Vec::new();
    if repeating {
        let (first, last) = (ramp.first_offset(), ramp.last_offset());
        // A period of no length paints the last stop's colour (as Chromium does).
        if last - first > 0.0 && (last - first) * length >= 1e-3 {
            ramp.sample(first, last, |t| (t - first) / (last - first), &mut samples);
            push_samples(&mut gradient, &samples);
            gradient.kind = peniko::GradientKind::Linear(LinearGradientPosition {
                start: start + (end - start) * first,
                end: start + (end - start) * last,
            });
        } else {
            push_solid(&mut gradient, ramp.last_color());
        }
    } else {
        ramp.sample(0.0, 1.0, |t| t, &mut samples);
        push_samples(&mut gradient, &samples);
    }

    (gradient, None)
}

fn radial_gradient(
    gradient: RadialGradient,
    method: ColorInterpolationMethod,
    rect: Rect,
    scale: f64,
    current_color: &AbsoluteColor,
) -> (peniko::Gradient, Option<Affine>) {
    let (shape, position, items, flags) = gradient;
    let repeating = flags.contains(GradientFlags::REPEATING);

    let mut gradient = peniko::Gradient::new_radial((0.0, 0.0), 1.0).with_extend(if repeating {
        peniko::Extend::Repeat
    } else {
        peniko::Extend::Pad
    });

    let (width_px, height_px) = (
        resolve_gradient_length(&position.horizontal, rect.width()),
        resolve_gradient_length(&position.vertical, rect.height()),
    );

    let gradient_scale: Option<Vec2> = match shape {
        GenericEndingShape::Circle(circle) => {
            let scale = match circle {
                GenericCircle::Extent(extent) => match extent {
                    ShapeExtent::FarthestSide => width_px
                        .max(rect.width() - width_px)
                        .max(height_px.max(rect.height() - height_px)),
                    ShapeExtent::ClosestSide => width_px
                        .min(rect.width() - width_px)
                        .min(height_px.min(rect.height() - height_px)),
                    ShapeExtent::FarthestCorner => {
                        (width_px.max(rect.width() - width_px)
                            + height_px.max(rect.height() - height_px))
                            * 0.5_f64.sqrt()
                    }
                    ShapeExtent::ClosestCorner => {
                        (width_px.min(rect.width() - width_px)
                            + height_px.min(rect.height() - height_px))
                            * 0.5_f64.sqrt()
                    }
                    _ => 0.0,
                },
                GenericCircle::Radius(radius) => radius.0.px() as f64,
            };
            Some(Vec2::new(scale, scale))
        }
        GenericEndingShape::Ellipse(ellipse) => match ellipse {
            GenericEllipse::Extent(extent) => match extent {
                ShapeExtent::FarthestCorner | ShapeExtent::FarthestSide => {
                    let mut scale = Vec2::new(
                        width_px.max(rect.width() - width_px),
                        height_px.max(rect.height() - height_px),
                    );
                    if *extent == ShapeExtent::FarthestCorner {
                        scale *= 2.0_f64.sqrt();
                    }
                    Some(scale)
                }
                ShapeExtent::ClosestCorner | ShapeExtent::ClosestSide => {
                    let mut scale = Vec2::new(
                        width_px.min(rect.width() - width_px),
                        height_px.min(rect.height() - height_px),
                    );
                    if *extent == ShapeExtent::ClosestCorner {
                        scale *= 2.0_f64.sqrt();
                    }
                    Some(scale)
                }
                _ => None,
            },
            GenericEllipse::Radii(x, y) => Some(Vec2::new(
                resolve_gradient_length(&x.0, rect.width()),
                resolve_gradient_length(&y.0, rect.height()),
            )),
        },
    };

    // If the gradient has no valid scale, we don't need to calculate the color stops
    let Some(gradient_scale) = gradient_scale else {
        return (gradient, None);
    };
    let ramp = Ramp::new(items, method, current_color, |position| {
        length_position(position, gradient_scale.x, scale)
    });
    let translation = get_translation(position, rect);
    // pyro: an ending shape without a width or a height (CSS Images 3 §3.2.3 has it paint as a
    // shape too small to see), and a gradient whose stops are all before its centre, are the
    // last stop's colour; so is a repeating one without a period.
    let (first, last) = (ramp.first_offset(), ramp.last_offset());
    let degenerate = !(gradient_scale.x > 0.0 && gradient_scale.y > 0.0)
        || if repeating {
            !((last - first) * gradient_scale.x.min(gradient_scale.y) >= 1e-3)
        } else {
            !(last > 0.0)
        };
    if degenerate {
        push_solid(&mut gradient, ramp.last_color());
        return (gradient, Some(Affine::translate(translation)));
    }
    // A centre or a radius far beyond what the renderer's numbers hold (`at 0% calc(infinity *
    // 1px)`): the box is a point at that scale, of the one colour at its distance.
    const FAR: f64 = 1e9;
    if translation.x.abs() > FAR
        || translation.y.abs() > FAR
        || gradient_scale.x > FAR
        || gradient_scale.y > FAR
    {
        let center = rect.center();
        let t = ((center.x - translation.x) / gradient_scale.x)
            .hypot((center.y - translation.y) / gradient_scale.y);
        let t = if t.is_finite() { t } else { 0.0 };
        push_solid(&mut gradient, Some(ramp.color_at(t)));
        return (gradient, None);
    }
    let mut samples = Vec::new();
    // The renderer's unit radius is where the last sample is: the last stop, or one period.
    let reach = if repeating {
        sample_period_from_zero(&ramp, &mut samples);
        last - first
    } else {
        // A hard stop at the last position ends there: the renderer needs room after it for
        // the colour that follows.
        let count = ramp.stops.len();
        let reach = if count > 1 && ramp.stops[count - 2].offset == last { last * 1.01 } else { last };
        let reach = reach.min(1e12);
        ramp.sample(0.0, reach, |t| t / reach, &mut samples);
        reach
    };
    push_samples(&mut gradient, &samples);
    let transform = Affine::scale_non_uniform(gradient_scale.x * reach, gradient_scale.y * reach)
        .then_translate(translation);
    (gradient, Some(transform))
}

fn conic_gradient(
    gradient: ConicGradient,
    method: ColorInterpolationMethod,
    rect: Rect,
    current_color: &AbsoluteColor,
) -> (peniko::Gradient, Option<Affine>) {
    let (angle, position, items, flags) = gradient;
    const TURN: f32 = std::f32::consts::PI * 2.0;

    let repeating = flags.contains(GradientFlags::REPEATING);
    let mut gradient = peniko::Gradient::new_sweep((0.0, 0.0), 0.0, TURN);
    let ramp = Ramp::new(items, method, current_color, |position| match position {
        AngleOrPercentage::Angle(angle) => angle.radians64() / (std::f64::consts::PI * 2.0),
        AngleOrPercentage::Percentage(percentage) => {
            percentage.to_percentage().unwrap_or(0.0) as f64
        }
    });
    let mut samples = Vec::new();
    let period = ramp.last_offset() - ramp.first_offset();
    if !repeating {
        ramp.sample(0.0, 1.0, |t| t, &mut samples);
        push_samples(&mut gradient, &samples);
    } else if period >= 1e-5 && period < 1.0 {
        sample_period_from_zero(&ramp, &mut samples);
        push_samples(&mut gradient, &samples);
        gradient = gradient.with_extend(peniko::Extend::Repeat);
        gradient.kind = peniko::GradientKind::Sweep(SweepGradientPosition {
            center: Point::new(0.0, 0.0),
            start_angle: 0.0,
            end_angle: TURN * period as f32,
        });
    } else if period >= 1.0 {
        // The turn is within one period: the part of it from 0, cut at the turn's end.
        let first = ramp.first_offset();
        let into = (-first).rem_euclid(period);
        let room = period - into;
        if room >= 1.0 {
            ramp.sample(first + into, first + into + 1.0, |t| t - first - into, &mut samples);
        } else {
            ramp.sample(first + into, first + period, |t| t - first - into, &mut samples);
            ramp.sample(first, first + 1.0 - room, |t| t - first + room, &mut samples);
        }
        push_samples(&mut gradient, &samples);
    } else {
        push_solid(&mut gradient, ramp.last_color());
    }

    let gradient_transform = Some(
        Affine::rotate(angle.radians() as f64 - std::f64::consts::PI / 2.0)
            .then_translate(get_translation(position, rect)),
    );

    (gradient, gradient_transform)
}

/// pyro: a gradient's colour stops after the fix-up of CSS Images 3 §3.4.3, evaluated here
/// rather than by the renderer: positions in units of the gradient line (0 its start, 1 its
/// end; turns for a conic gradient), in order, with the colours as specified. The renderer gets
/// sRGB samples of it (`sample`), which it interpolates premultiplied: the stops the stylesheet
/// gave where that is all the interpolation there is (sRGB, no hints), and as many more as the
/// curve needs in another colour space (`in oklch longer hue`) or under a transition hint.
///
/// Before, the stops went to the renderer as they were, with the colour space as a setting: a
/// stop before 0 or past 1 was not cut where the gradient box ends, the padding stops the old
/// code added were interpolated with the neighbour (`longer hue` between the last stop and its
/// own copy went once round the hue circle), and stops in decreasing order were not fixed up.
struct Ramp {
    stops: Vec<RampStop>,
    method: ColorInterpolationMethod,
}

struct RampStop {
    offset: f64,
    color: AbsoluteColor,
    /// A transition hint between the previous stop and this one: where the midpoint colour is,
    /// as a fraction of the way from that stop to this one.
    hint: Option<f64>,
}

/// Premultiplied sRGB.
type Sample = [f32; 4];

impl Ramp {
    fn new<T>(
        items: &[GradientItem<T>],
        method: ColorInterpolationMethod,
        current_color: &AbsoluteColor,
        position_of: impl Fn(&T) -> f64,
    ) -> Self {
        // Colour stops with their positions, and the hint before each.
        let mut stops: Vec<(Option<f64>, AbsoluteColor, Option<f64>)> = Vec::new();
        let mut hint = None;
        for item in items {
            match item {
                GenericGradientItem::SimpleColorStop(color) => {
                    stops.push((None, color.resolve_to_absolute(current_color), hint.take()));
                }
                GenericGradientItem::ComplexColorStop { color, position } => {
                    let position = position_of(position);
                    let position = if position.is_finite() { position } else { 0.0 };
                    stops.push((
                        Some(position),
                        color.resolve_to_absolute(current_color),
                        hint.take(),
                    ));
                }
                GenericGradientItem::InterpolationHint(position) => {
                    let position = position_of(position);
                    hint = position.is_finite().then_some(position);
                }
            }
        }
        let count = stops.len();
        if count == 0 {
            return Self { stops: Vec::new(), method };
        }
        // 1. The first stop is at 0 and the last at 1 unless they say where they are.
        stops[0].0.get_or_insert(0.0);
        stops[count - 1].0.get_or_insert(1.0);
        // 2. A stop or hint before one that precedes it moves up to it.
        let mut furthest = f64::NEG_INFINITY;
        for (position, _, hint) in &mut stops {
            if let Some(hint) = hint {
                *hint = hint.max(furthest);
                furthest = *hint;
            }
            if let Some(position) = position {
                *position = position.max(furthest);
                furthest = *position;
            }
        }
        // 3. A run of stops without positions is spaced evenly between its neighbours.
        let mut i = 1;
        while i < count {
            if stops[i].0.is_some() {
                i += 1;
                continue;
            }
            let run_end = (i..count).find(|&j| stops[j].0.is_some()).unwrap_or(count - 1);
            let from = stops[i - 1].0.unwrap_or(0.0);
            let to = stops[run_end].0.unwrap_or(from);
            let steps = (run_end - (i - 1)) as f64;
            for j in i..run_end {
                stops[j].0 = Some(from + (to - from) * (j - (i - 1)) as f64 / steps);
            }
            i = run_end + 1;
        }
        let mut ramp = Vec::with_capacity(count);
        for (idx, (position, color, hint)) in stops.iter().enumerate() {
            let offset = position.unwrap_or(0.0);
            let previous = if idx == 0 { offset } else { ramp_offset(&ramp, idx - 1) };
            let hint = hint.and_then(|hint| {
                let span = offset - previous;
                (span > 0.0).then(|| ((hint - previous) / span).clamp(0.0, 1.0))
            });
            // The midpoint is where the hint puts it without one.
            let hint = hint.filter(|fraction| (*fraction - 0.5).abs() > 1e-9);
            ramp.push(RampStop { offset, color: *color, hint });
        }
        Self { stops: ramp, method }
    }

    fn first_offset(&self) -> f64 {
        self.stops.first().map_or(0.0, |stop| stop.offset)
    }

    fn last_offset(&self) -> f64 {
        self.stops.last().map_or(0.0, |stop| stop.offset)
    }

    fn last_color(&self) -> Option<Sample> {
        self.stops.last().map(|stop| srgb_sample(&stop.color))
    }

    /// The colour at `t` inside segment `segment` (between stops `segment` and `segment + 1`;
    /// -1 is before the first stop, the last index after the last one).
    fn color_in(&self, segment: isize, t: f64) -> Sample {
        let last = self.stops.len() as isize - 1;
        if segment < 0 {
            return srgb_sample(&self.stops[0].color);
        }
        if segment >= last {
            return srgb_sample(&self.stops[last as usize].color);
        }
        let from = &self.stops[segment as usize];
        let to = &self.stops[segment as usize + 1];
        let span = to.offset - from.offset;
        let mut fraction = if span > 0.0 { ((t - from.offset) / span).clamp(0.0, 1.0) } else { 1.0 };
        if let Some(hint) = to.hint {
            // CSS Images 4 §3.5.3: the hint's position H gets the midpoint colour.
            fraction = if hint <= 0.0 {
                1.0
            } else if hint >= 1.0 {
                0.0
            } else {
                fraction.powf(0.5f64.ln() / hint.ln())
            };
        }
        // A missing component is the other stop's across the whole segment, its ends too
        // (`color(srgb none 0.5 0.5)` after a red stop is red there): those go through the mix.
        let missing = has_missing(&from.color) || has_missing(&to.color);
        // pyro: in sRGB the segment's ends are exact: each missing component the other end's (0
        // where both miss it), and the renderer's line between them is the interpolation.
        if missing && self.method.space == StyloColorSpace::Srgb && (fraction <= 0.0 || fraction >= 1.0) {
            let (end, other) = if fraction <= 0.0 { (from, to) } else { (to, from) };
            return srgb_filled(&end.color, &other.color);
        }
        if fraction <= 0.0 && !missing {
            return srgb_sample(&from.color);
        }
        if fraction >= 1.0 && !missing {
            return srgb_sample(&to.color);
        }
        let fraction = fraction.clamp(0.0, 1.0) as f32;
        let mixed = style::color::mix::mix_many(
            self.method,
            [
                ColorMixItem::new(from.color, 1.0 - fraction),
                ColorMixItem::new(to.color, fraction),
            ],
            ColorMixFlags::empty(),
        );
        // `mix_many` rounds the alpha to thousandths: take the exact one (a missing alpha is
        // the other side's, which the mix has).
        let alpha = from.color.alpha + (to.color.alpha - from.color.alpha) * fraction;
        let mut sample = srgb_sample(&mixed);
        let old_alpha = sample[3];
        let missing = style::color::ColorFlags::ALPHA_IS_NONE;
        if old_alpha > 0.0 && !from.color.flags.contains(missing) && !to.color.flags.contains(missing) {
            let alpha = alpha.clamp(0.0, 1.0);
            for channel in &mut sample[..3] {
                *channel = *channel / old_alpha * alpha;
            }
            sample[3] = alpha;
        }
        sample
    }

    /// The colour at `t` (past a hard stop at `t`).
    fn color_at(&self, t: f64) -> Sample {
        if self.stops.is_empty() {
            return [0.0; 4];
        }
        let segment = self.stops.iter().filter(|stop| stop.offset <= t).count() as isize - 1;
        self.color_in(segment, t)
    }

    /// Whether the renderer's premultiplied sRGB interpolation between the ends of `segment` is
    /// not the gradient's.
    fn curved(&self, segment: isize) -> bool {
        if segment < 0 || segment >= self.stops.len() as isize - 1 {
            return false;
        }
        let to = &self.stops[segment as usize + 1];
        // pyro: missing components of sRGB stops are resolved at the segment's ends
        // (`color_in`): the renderer's line between them is the interpolation.
        to.hint.is_some() || self.method.space != StyloColorSpace::Srgb
    }

    /// Samples of `[from, to]` (`from < to`), appended to `out` with `at(t)` as their offsets.
    fn sample(&self, from: f64, to: f64, at: impl Fn(f64) -> f64, out: &mut Vec<(f64, Sample)>) {
        if self.stops.is_empty() {
            return;
        }
        let mut breaks = vec![from];
        for stop in &self.stops {
            if stop.offset > from && stop.offset < to && breaks.last() != Some(&stop.offset) {
                breaks.push(stop.offset);
            }
        }
        breaks.push(to);
        // How finely a curved segment may be cut: 2^depth pieces.
        let depth = if breaks.len() > 9 { 6 } else { 9 };
        for pair in breaks.windows(2) {
            let (p, q) = (pair[0], pair[1]);
            let middle = p + (q - p) / 2.0;
            let segment = self.stops.iter().filter(|stop| stop.offset <= middle).count() as isize - 1;
            let start = self.color_in(segment, p);
            let end = self.color_in(segment, q);
            out.push((at(p), start));
            if self.curved(segment) && start != end {
                self.subdivide(segment, (p, start), (q, end), depth, 3, &at, out);
            } else if self.curved(segment) {
                // Equal ends are not a constant colour: a full turn of hue ends where it began.
                self.subdivide(segment, (p, start), (q, end), depth, 4, &at, out);
            }
            out.push((at(q), end));
        }
    }

    /// The samples strictly between two of a curved segment: its halves, until the renderer's
    /// line between two samples is within a quarter level of the curve's middle (`forced`
    /// levels are cut regardless: a curve can cross its chord at the middle).
    #[allow(clippy::too_many_arguments)]
    fn subdivide(
        &self,
        segment: isize,
        (p, start): (f64, Sample),
        (q, end): (f64, Sample),
        depth: u32,
        forced: u32,
        at: &impl Fn(f64) -> f64,
        out: &mut Vec<(f64, Sample)>,
    ) {
        if depth == 0 {
            return;
        }
        let middle = p + (q - p) / 2.0;
        if !(middle > p && middle < q) {
            return;
        }
        let color = self.color_in(segment, middle);
        let off = (0..4)
            .map(|i| (color[i] - (start[i] + end[i]) / 2.0).abs())
            .fold(0.0f32, f32::max);
        if forced == 0 && off <= 0.25 / 255.0 {
            return;
        }
        let forced = forced.saturating_sub(1);
        self.subdivide(segment, (p, start), (middle, color), depth - 1, forced, at, out);
        out.push((at(middle), color));
        self.subdivide(segment, (middle, color), (q, end), depth - 1, forced, at, out);
    }
}

/// pyro: `color` in premultiplied sRGB with each missing component `other`'s (0 where both miss
/// it), clipped to the gamut (CSS Color 4 §12.2, interpolating in sRGB).
fn srgb_filled(color: &AbsoluteColor, other: &AbsoluteColor) -> Sample {
    use style::color::ColorFlags as F;
    let (color, other) = (color.to_color_space(StyloColorSpace::Srgb), other.to_color_space(StyloColorSpace::Srgb));
    let mut components = *color.raw_components();
    let others = other.raw_components();
    for (i, flag) in [F::C0_IS_NONE, F::C1_IS_NONE, F::C2_IS_NONE, F::ALPHA_IS_NONE].into_iter().enumerate() {
        if color.flags.contains(flag) {
            components[i] = if other.flags.contains(flag) { 0.0 } else { others[i] };
        }
    }
    let fix = |c: f32| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
    let [r, g, b, a] = components.map(fix);
    [r * a, g * a, b * a, a]
}

/// Whether a component of `color` is `none`.
fn has_missing(color: &AbsoluteColor) -> bool {
    use style::color::ColorFlags as F;
    color.flags.intersects(F::C0_IS_NONE | F::C1_IS_NONE | F::C2_IS_NONE | F::ALPHA_IS_NONE)
}

fn ramp_offset(ramp: &[RampStop], idx: usize) -> f64 {
    ramp[idx].offset
}

/// `color` in premultiplied sRGB, clipped to the gamut.
fn srgb_sample(color: &AbsoluteColor) -> Sample {
    let srgb = color.to_color_space(StyloColorSpace::Srgb);
    let [r, g, b, a] = *srgb.raw_components();
    let fix = |c: f32| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
    let a = fix(a);
    [fix(r) * a, fix(g) * a, fix(b) * a, a]
}

/// The renderer's stops for samples (offsets in 0..=1, in order).
fn push_samples(gradient: &mut Gradient, samples: &[(f64, Sample)]) {
    let mut last: Option<(f32, Sample)> = None;
    let mut run = 0;
    for &(offset, sample) in samples {
        let offset = (offset as f32).clamp(0.0, 1.0);
        // At one offset the first and the last sample count (a hard stop): drop those between.
        if let Some((last_offset, last_sample)) = last {
            if last_offset == offset && last_sample == sample {
                continue;
            }
            if last_offset == offset {
                run += 1;
                if run > 1 {
                    gradient.stops.pop();
                }
            } else {
                run = 0;
            }
        }
        last = Some((offset, sample));
        let [r, g, b, a] = sample;
        let color = if a > 0.0 { [r / a, g / a, b / a, a] } else { [0.0, 0.0, 0.0, 0.0] };
        gradient.stops.push(ColorStop {
            offset,
            color: DynamicColor::from_alpha_color(Color::new(color)),
        });
    }
    // The renderer needs two stops.
    if gradient.stops.len() == 1 {
        let stop = gradient.stops[0];
        gradient.stops.push(ColorStop { offset: 1.0, ..stop });
    }
}

/// A gradient of one colour.
fn push_solid(gradient: &mut Gradient, sample: Option<Sample>) {
    if let Some(sample) = sample {
        push_samples(gradient, &[(0.0, sample), (1.0, sample)]);
    }
}

/// One period of a repeating ramp as it lies from 0 (the centre of a radial gradient, the start
/// angle of a conic one): the part of the period that 0 falls in, then the period's start.
fn sample_period_from_zero(ramp: &Ramp, out: &mut Vec<(f64, Sample)>) {
    let first = ramp.first_offset();
    let period = ramp.last_offset() - first;
    let into = (-first).rem_euclid(period);
    if into > 0.0 && into < period {
        ramp.sample(first + into, first + period, |t| (t - first - into) / period, out);
        ramp.sample(first, first + into, |t| (t - first + period - into) / period, out);
    } else {
        ramp.sample(first, first + period, |t| (t - first) / period, out);
    }
}

#[inline]
fn resolve_gradient_length(value: &LengthPercentage, basis: f64) -> f64 {
    // Percentage resolution can overflow even though computed lengths are finite.
    value
        .resolve(CSSPixelLength::new(basis as f32))
        .px()
        .clamp(f32::MIN, f32::MAX) as f64
}

#[inline]
fn get_translation(
    position: &GenericPosition<LengthPercentage, LengthPercentage>,
    rect: Rect,
) -> Vec2 {
    Vec2::new(
        rect.x0 + resolve_gradient_length(&position.horizontal, rect.width()),
        rect.y0 + resolve_gradient_length(&position.vertical, rect.height()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_percentage_overflow_is_clamped() {
        for percentage in [f32::MAX, f32::MIN] {
            let value = LengthPercentage::new_percent(Percentage(percentage));
            assert_eq!(resolve_gradient_length(&value, 300.0), percentage as f64);
        }
    }

    #[test]
    fn finite_gradient_lengths_are_preserved() {
        let length = LengthPercentage::new_length(CSSPixelLength::new(-12.0));
        let percentage = LengthPercentage::new_percent(Percentage(-0.5));
        assert_eq!(resolve_gradient_length(&length, 300.0), -12.0);
        assert_eq!(resolve_gradient_length(&percentage, 300.0), -150.0);
    }
}
