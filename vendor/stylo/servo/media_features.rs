/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Servo's media feature list and evaluator.

use crate::derives::*;
use crate::queries::feature::{AllowsRanges, Evaluator, FeatureFlags, QueryFeatureDescription};
use crate::queries::values::{Orientation, PrefersColorScheme};
use crate::media_queries::MediaType;
use crate::values::computed::{CSSPixelLength, Context, Ratio, Resolution};
use crate::values::specified::color::ForcedColors;
use crate::Atom;
use std::fmt::Debug;
use std::sync::LazyLock;

/// https://drafts.csswg.org/mediaqueries-4/#width
fn eval_width(context: &Context) -> CSSPixelLength {
    CSSPixelLength::new(context.device().au_viewport_size().width.to_f32_px())
}

/// https://drafts.csswg.org/mediaqueries-4/#height
fn eval_height(context: &Context) -> CSSPixelLength {
    CSSPixelLength::new(context.device().au_viewport_size().height.to_f32_px())
}

/// https://drafts.csswg.org/mediaqueries-4/#device-width
fn eval_device_width(context: &Context) -> CSSPixelLength {
    let device = context.device();
    let scaled = device.device_size() / device.device_pixel_ratio();
    CSSPixelLength::new(scaled.width)
}

/// https://drafts.csswg.org/mediaqueries-4/#device-height
fn eval_device_height(context: &Context) -> CSSPixelLength {
    let device = context.device();
    let scaled = device.device_size() / device.device_pixel_ratio();
    CSSPixelLength::new(scaled.height)
}

/// https://drafts.csswg.org/mediaqueries-4/#orientation
fn eval_orientation(context: &Context, value: Option<Orientation>) -> bool {
    Orientation::eval(context.device().au_viewport_size(), value)
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum Scan {
    Progressive,
    Interlace,
}

/// https://drafts.csswg.org/mediaqueries-4/#scan
fn eval_scan(_: &Context, _: Option<Scan>) -> bool {
    // Since we doesn't support the 'tv' media type, the 'scan' feature never
    // matches.
    false
}

/// https://drafts.csswg.org/mediaqueries-4/#resolution
fn eval_resolution(context: &Context) -> Resolution {
    Resolution::from_dppx(context.device().device_pixel_ratio().0)
}

/// https://compat.spec.whatwg.org/#css-media-queries-webkit-device-pixel-ratio
fn eval_device_pixel_ratio(context: &Context) -> f32 {
    eval_resolution(context).dppx()
}

fn eval_prefers_color_scheme(context: &Context, query_value: Option<PrefersColorScheme>) -> bool {
    match query_value {
        Some(v) => context.device().color_scheme() == v,
        None => true,
    }
}

bitflags! {
    /// https://drafts.csswg.org/mediaqueries-4/#mf-interaction
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct PointerCapabilities: u8 {
        /// The input mechanism includes a pointing device of limited accuracy, such as a finger on a touchscreen.
        const COARSE = 0b001;
        /// The input mechanism includes an accurate pointing device, such as a mouse.
        const FINE = 0b010;
        /// The input mechanism can conveniently hover over elements.
        const HOVER = 0b100;
    }
}

impl Default for PointerCapabilities {
    #[cfg(any(target_os = "ios", target_os = "android", target_env = "ohos"))]
    fn default() -> Self {
        PointerCapabilities::COARSE
    }
    #[cfg(not(any(target_os = "ios", target_os = "android", target_env = "ohos")))]
    fn default() -> Self {
        PointerCapabilities::FINE | PointerCapabilities::HOVER
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum Pointer {
    None,
    Coarse,
    Fine,
}

fn eval_pointer_capabilities(
    query_value: Option<Pointer>,
    pointer_capabilities: PointerCapabilities,
) -> bool {
    match query_value {
        None => !pointer_capabilities.is_empty(),
        Some(Pointer::None) => pointer_capabilities.is_empty(),
        Some(Pointer::Coarse) => pointer_capabilities.intersects(PointerCapabilities::COARSE),
        Some(Pointer::Fine) => pointer_capabilities.intersects(PointerCapabilities::FINE),
    }
}

/// https://drafts.csswg.org/mediaqueries-4/#pointer
fn eval_pointer(context: &Context, query_value: Option<Pointer>) -> bool {
    eval_pointer_capabilities(query_value, context.device().primary_pointer_capabilities())
}

/// https://drafts.csswg.org/mediaqueries-4/#descdef-media-any-pointer
fn eval_any_pointer(context: &Context, query_value: Option<Pointer>) -> bool {
    eval_pointer_capabilities(query_value, context.device().all_pointer_capabilities())
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum Hover {
    None,
    Hover,
}

fn eval_hover_capabilities(
    query_value: Option<Hover>,
    pointer_capabilities: PointerCapabilities,
) -> bool {
    let can_hover = pointer_capabilities.intersects(PointerCapabilities::HOVER);
    match query_value {
        Some(Hover::None) => !can_hover,
        Some(Hover::Hover) => can_hover,
        None => return can_hover,
    }
}

/// https://drafts.csswg.org/mediaqueries-4/#hover
fn eval_hover(context: &Context, query_value: Option<Hover>) -> bool {
    eval_hover_capabilities(query_value, context.device().primary_pointer_capabilities())
}

/// https://drafts.csswg.org/mediaqueries-4/#descdef-media-any-hover
fn eval_any_hover(context: &Context, query_value: Option<Hover>) -> bool {
    eval_hover_capabilities(query_value, context.device().all_pointer_capabilities())
}

/// <https://drafts.csswg.org/mediaqueries-4/#aspect-ratio>
fn eval_aspect_ratio(context: &Context) -> Ratio {
    let size = context.device().au_viewport_size();
    Ratio::new(size.width.0 as f32, size.height.0 as f32)
}

/// <https://drafts.csswg.org/mediaqueries-4/#device-aspect-ratio>
fn eval_device_aspect_ratio(context: &Context) -> Ratio {
    let size = context.device().device_size();
    Ratio::new(size.width, size.height)
}

/// https://drafts.csswg.org/mediaqueries-4/#grid
fn eval_grid(_: &Context) -> bool {
    // Bitmap devices only (no ttys).
    false
}

/// https://compat.spec.whatwg.org/#css-media-queries-webkit-transform-3d
fn eval_transform_3d(_: &Context) -> bool {
    true
}

/// https://drafts.csswg.org/mediaqueries-4/#color: bits per colour component (Blink reports 8).
fn eval_color(_: &Context) -> i32 {
    8
}

/// https://drafts.csswg.org/mediaqueries-4/#color-index: no colour lookup table.
fn eval_color_index(_: &Context) -> i32 {
    0
}

/// https://drafts.csswg.org/mediaqueries-4/#monochrome: a colour device.
fn eval_monochrome(_: &Context) -> i32 {
    0
}

/// Values for the color-gamut media feature; ordered so a narrower query matches a wider gamut.
#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, PartialOrd, ToCss)]
#[repr(u8)]
enum ColorGamut {
    Srgb,
    P3,
    Rec2020,
}

/// https://drafts.csswg.org/mediaqueries-4/#color-gamut (an sRGB device).
fn eval_color_gamut(_: &Context, query_value: Option<ColorGamut>) -> bool {
    match query_value {
        Some(v) => v <= ColorGamut::Srgb,
        None => true,
    }
}

/// Values for the dynamic-range and video-dynamic-range media features.
#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, PartialOrd, ToCss)]
#[repr(u8)]
enum DynamicRange {
    Standard,
    High,
}

/// https://drafts.csswg.org/mediaqueries-5/#dynamic-range (a standard-range device).
fn eval_dynamic_range(_: &Context, query_value: Option<DynamicRange>) -> bool {
    match query_value {
        Some(v) => v <= DynamicRange::Standard,
        None => true,
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, ToCss)]
#[repr(u8)]
enum NoPreferenceOrReduce {
    NoPreference,
    Reduce,
}

/// prefers-reduced-motion, prefers-reduced-transparency, prefers-reduced-data: no preference.
fn eval_prefers_reduced(_: &Context, query_value: Option<NoPreferenceOrReduce>) -> bool {
    match query_value {
        Some(v) => v == NoPreferenceOrReduce::NoPreference,
        None => false,
    }
}

fn eval_prefers_reduced_motion(context: &Context, value: Option<NoPreferenceOrReduce>) -> bool {
    context.device().media_data().prefers_reduced_motion == (value != Some(NoPreferenceOrReduce::NoPreference))
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, ToCss)]
#[repr(u8)]
enum PrefersContrast {
    More,
    Less,
    Custom,
    NoPreference,
}

/// https://drafts.csswg.org/mediaqueries-5/#prefers-contrast
fn eval_prefers_contrast(_: &Context, query_value: Option<PrefersContrast>) -> bool {
    match query_value {
        Some(v) => v == PrefersContrast::NoPreference,
        None => false,
    }
}

/// https://drafts.csswg.org/mediaqueries-5/#forced-colors
fn eval_forced_colors(context: &Context, query_value: Option<ForcedColors>) -> bool {
    let forced = context.device().forced_colors();
    match query_value {
        Some(query_value) => query_value == forced,
        None => forced != ForcedColors::None,
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, ToCss)]
#[repr(u8)]
enum InvertedColors {
    None,
    Inverted,
}

/// https://drafts.csswg.org/mediaqueries-5/#inverted
fn eval_inverted_colors(_: &Context, query_value: Option<InvertedColors>) -> bool {
    match query_value {
        Some(v) => v == InvertedColors::None,
        None => false,
    }
}

fn scrolling(context: &Context) -> bool {
    context.device().media_type() != MediaType::print()
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum OverflowBlock {
    None,
    Scroll,
    Paged,
}

/// https://drafts.csswg.org/mediaqueries-4/#mf-overflow-block
fn eval_overflow_block(context: &Context, query_value: Option<OverflowBlock>) -> bool {
    match query_value {
        None => true,
        Some(OverflowBlock::None) => false,
        Some(OverflowBlock::Scroll) => scrolling(context),
        Some(OverflowBlock::Paged) => !scrolling(context),
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum OverflowInline {
    None,
    Scroll,
}

/// https://drafts.csswg.org/mediaqueries-4/#mf-overflow-inline
fn eval_overflow_inline(context: &Context, query_value: Option<OverflowInline>) -> bool {
    match query_value {
        None => scrolling(context),
        Some(OverflowInline::None) => !scrolling(context),
        Some(OverflowInline::Scroll) => scrolling(context),
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum Update {
    None,
    Slow,
    Fast,
}

/// https://drafts.csswg.org/mediaqueries-4/#update
fn eval_update(context: &Context, query_value: Option<Update>) -> bool {
    match query_value {
        None => scrolling(context),
        Some(Update::None) => !scrolling(context),
        Some(Update::Slow) => false,
        Some(Update::Fast) => scrolling(context),
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, ToCss)]
#[repr(u8)]
enum DisplayMode {
    Browser,
    MinimalUi,
    Standalone,
    Fullscreen,
    PictureInPicture,
    WindowControlsOverlay,
    Unframed,
}

/// https://w3c.github.io/manifest/#the-display-mode-media-feature
fn eval_display_mode(_: &Context, query_value: Option<DisplayMode>) -> bool {
    match query_value {
        Some(v) => v == DisplayMode::Browser,
        None => true,
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, PartialEq, ToCss)]
#[repr(u8)]
enum Scripting {
    None,
    InitialOnly,
    Enabled,
}

/// https://drafts.csswg.org/mediaqueries-5/#scripting (pages run scripts).
fn eval_scripting(_: &Context, query_value: Option<Scripting>) -> bool {
    match query_value {
        Some(v) => v == Scripting::Enabled,
        None => true,
    }
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum NavigationControls {
    None,
    BackButton,
}

fn eval_navigation_controls(_: &Context, value: Option<NavigationControls>) -> bool {
    matches!(value, Some(NavigationControls::None))
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum DisplayState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
}

fn eval_display_state(_: &Context, value: Option<DisplayState>) -> bool {
    matches!(value, None | Some(DisplayState::Normal))
}

#[derive(Clone, Copy, Debug, FromPrimitive, Parse, ToCss)]
#[repr(u8)]
enum Resizable {
    True,
    False,
}

fn eval_resizable(_: &Context, value: Option<Resizable>) -> bool {
    matches!(value, None | Some(Resizable::True))
}

/// A list with all the media features that Servo supports.
pub static MEDIA_FEATURES: LazyLock<[QueryFeatureDescription; 38]> = LazyLock::new(|| [
    feature!(
        Atom::from("navigation-controls"),
        AllowsRanges::No,
        keyword_evaluator!(eval_navigation_controls, NavigationControls),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("display-state"),
        AllowsRanges::No,
        keyword_evaluator!(eval_display_state, DisplayState),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("resizable"),
        AllowsRanges::No,
        keyword_evaluator!(eval_resizable, Resizable),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("width"),
        AllowsRanges::Yes,
        Evaluator::Length(eval_width),
        FeatureFlags::VIEWPORT_DEPENDENT,
    ),
    feature!(
        atom!("height"),
        AllowsRanges::Yes,
        Evaluator::Length(eval_height),
        FeatureFlags::VIEWPORT_DEPENDENT,
    ),
    feature!(
        atom!("orientation"),
        AllowsRanges::No,
        keyword_evaluator!(eval_orientation, Orientation),
        FeatureFlags::VIEWPORT_DEPENDENT,
    ),
    feature!(
        atom!("pointer"),
        AllowsRanges::No,
        keyword_evaluator!(eval_pointer, Pointer),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("any-pointer"),
        AllowsRanges::No,
        keyword_evaluator!(eval_any_pointer, Pointer),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("hover"),
        AllowsRanges::No,
        keyword_evaluator!(eval_hover, Hover),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("any-hover"),
        AllowsRanges::No,
        keyword_evaluator!(eval_any_hover, Hover),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("aspect-ratio"),
        AllowsRanges::Yes,
        Evaluator::NumberRatio(eval_aspect_ratio),
        FeatureFlags::VIEWPORT_DEPENDENT,
    ),
    feature!(
        atom!("device-width"),
        AllowsRanges::Yes,
        Evaluator::Length(eval_device_width),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("device-height"),
        AllowsRanges::Yes,
        Evaluator::Length(eval_device_height),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("scan"),
        AllowsRanges::No,
        keyword_evaluator!(eval_scan, Scan),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("resolution"),
        AllowsRanges::Yes,
        Evaluator::Resolution(eval_resolution),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("device-pixel-ratio"),
        AllowsRanges::Yes,
        Evaluator::Float(eval_device_pixel_ratio),
        FeatureFlags::WEBKIT_PREFIX,
    ),
    feature!(
        atom!("-moz-device-pixel-ratio"),
        AllowsRanges::Yes,
        Evaluator::Float(eval_device_pixel_ratio),
        FeatureFlags::empty(),
    ),
    feature!(
        atom!("prefers-color-scheme"),
        AllowsRanges::No,
        keyword_evaluator!(eval_prefers_color_scheme, PrefersColorScheme),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("device-aspect-ratio"),
        AllowsRanges::Yes,
        Evaluator::NumberRatio(eval_device_aspect_ratio),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("grid"),
        AllowsRanges::No,
        Evaluator::BoolInteger(eval_grid),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("transform-3d"),
        AllowsRanges::No,
        Evaluator::BoolInteger(eval_transform_3d),
        FeatureFlags::WEBKIT_PREFIX,
    ),
    feature!(
        Atom::from("color"),
        AllowsRanges::Yes,
        Evaluator::Integer(eval_color),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("color-index"),
        AllowsRanges::Yes,
        Evaluator::Integer(eval_color_index),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("monochrome"),
        AllowsRanges::Yes,
        Evaluator::Integer(eval_monochrome),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("color-gamut"),
        AllowsRanges::No,
        keyword_evaluator!(eval_color_gamut, ColorGamut),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("dynamic-range"),
        AllowsRanges::No,
        keyword_evaluator!(eval_dynamic_range, DynamicRange),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("video-dynamic-range"),
        AllowsRanges::No,
        keyword_evaluator!(eval_dynamic_range, DynamicRange),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("prefers-reduced-motion"),
        AllowsRanges::No,
        keyword_evaluator!(eval_prefers_reduced_motion, NoPreferenceOrReduce),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("prefers-reduced-transparency"),
        AllowsRanges::No,
        keyword_evaluator!(eval_prefers_reduced, NoPreferenceOrReduce),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("prefers-reduced-data"),
        AllowsRanges::No,
        keyword_evaluator!(eval_prefers_reduced, NoPreferenceOrReduce),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("prefers-contrast"),
        AllowsRanges::No,
        keyword_evaluator!(eval_prefers_contrast, PrefersContrast),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("forced-colors"),
        AllowsRanges::No,
        keyword_evaluator!(eval_forced_colors, ForcedColors),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("inverted-colors"),
        AllowsRanges::No,
        keyword_evaluator!(eval_inverted_colors, InvertedColors),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("overflow-block"),
        AllowsRanges::No,
        keyword_evaluator!(eval_overflow_block, OverflowBlock),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("overflow-inline"),
        AllowsRanges::No,
        keyword_evaluator!(eval_overflow_inline, OverflowInline),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("update"),
        AllowsRanges::No,
        keyword_evaluator!(eval_update, Update),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("display-mode"),
        AllowsRanges::No,
        keyword_evaluator!(eval_display_mode, DisplayMode),
        FeatureFlags::empty(),
    ),
    feature!(
        Atom::from("scripting"),
        AllowsRanges::No,
        keyword_evaluator!(eval_scripting, Scripting),
        FeatureFlags::empty(),
    ),
]);
