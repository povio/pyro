//! pyro: the scrollbar mode of the process: overlay scrollbars (the default), which take no layout
//! space, or classic scrollbars, which take a fixed thickness out of the scroll container's padding
//! box (as Chrome's on Linux, the platform wpt.fyi runs it on: 15 CSS px, 11 for
//! `scrollbar-width: thin`).
//!
//! The mode is set once, at startup (`pyro serve --scrollbars`), before any layout runs.

use std::sync::atomic::{AtomicU8, Ordering};

use style::values::computed::Overflow;
use style::values::computed::ui::ScrollbarWidth;

/// How scrollbars take space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollbarMode {
    /// Scrollbars overlay the content and take no layout space; `scrollbar-gutter` changes nothing.
    #[default]
    Overlay,
    /// Scrollbars take [`CLASSIC_THICKNESS`] (or [`CLASSIC_THIN_THICKNESS`]) CSS px of layout space.
    Classic,
}

/// The thickness of a classic scrollbar (Chrome's on Linux and Windows).
pub const CLASSIC_THICKNESS: f32 = 15.0;
/// The thickness of a classic `scrollbar-width: thin` scrollbar (Chrome's).
pub const CLASSIC_THIN_THICKNESS: f32 = 11.0;

static MODE: AtomicU8 = AtomicU8::new(0);

/// Sets the process's scrollbar mode. Call before any layout runs: boxes laid out earlier keep the
/// mode they were laid out in.
pub fn set_scrollbar_mode(mode: ScrollbarMode) {
    MODE.store(mode as u8, Ordering::Relaxed);
}

/// The process's scrollbar mode.
#[inline]
pub fn scrollbar_mode() -> ScrollbarMode {
    match MODE.load(Ordering::Relaxed) {
        1 => ScrollbarMode::Classic,
        _ => ScrollbarMode::Overlay,
    }
}

/// The layout thickness of a scrollbar of the given `scrollbar-width` in the given mode: zero for
/// overlay scrollbars and for `scrollbar-width: none`.
#[inline]
pub fn thickness_in(mode: ScrollbarMode, width: ScrollbarWidth) -> f32 {
    match (mode, width) {
        (ScrollbarMode::Overlay, _) | (_, ScrollbarWidth::None) => 0.0,
        (ScrollbarMode::Classic, ScrollbarWidth::Thin) => CLASSIC_THIN_THICKNESS,
        (ScrollbarMode::Classic, ScrollbarWidth::Auto) => CLASSIC_THICKNESS,
    }
}

/// The layout thickness of a scrollbar of the given `scrollbar-width` in the process's mode.
#[inline]
pub fn thickness(width: ScrollbarWidth) -> f32 {
    thickness_in(scrollbar_mode(), width)
}

/// Which physical scrollbars a scroll container shows and which physical edges of its padding box
/// hold scrollbar space, in classic mode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gutters {
    /// The space at each physical edge.
    pub edges: taffy::Rect<f32>,
}

/// What decides a box's classic scrollbar gutters (physical axes).
#[derive(Clone, Copy, Debug)]
pub struct GutterInputs {
    /// The scrollbar thickness ([`thickness`]); zero means no gutters at all.
    pub thickness: f32,
    /// Used `overflow-x` and `overflow-y`.
    pub overflow: taffy::Point<Overflow>,
    /// Whether an `overflow: auto` axis shows its scrollbar (its content overflows): x is the
    /// horizontal scrollbar (content overflowing horizontally), y the vertical one.
    pub auto_shows: taffy::Point<bool>,
    /// `scrollbar-gutter: stable`.
    pub stable: bool,
    /// `scrollbar-gutter: stable both-edges`.
    pub both_edges: bool,
    /// The box's inline axis is vertical (a vertical writing mode).
    pub vertical: bool,
    /// The box's direction is right-to-left.
    pub rtl: bool,
}

impl GutterInputs {
    /// The physical gutters (as Chrome lays them out): the vertical scrollbar on the right (on the
    /// left for `direction: rtl` in a horizontal writing mode), the horizontal one at the bottom;
    /// `scrollbar-gutter` reserves the gutter at the inline-end edge (both inline edges with
    /// `both-edges`) whether or not that scrollbar shows, when the overflow in the axis that
    /// scrollbar scrolls is `hidden`, `scroll` or `auto`.
    pub fn gutters(&self) -> Gutters {
        let t = self.thickness;
        let mut edges = taffy::Rect { left: 0.0, right: 0.0, top: 0.0, bottom: 0.0 };
        if t <= 0.0 {
            return Gutters { edges };
        }
        let shows = |overflow: Overflow, auto_shows: bool| match overflow {
            Overflow::Scroll => true,
            Overflow::Auto => auto_shows,
            _ => false,
        };
        let stable_applies = |overflow: Overflow| {
            self.stable && matches!(overflow, Overflow::Hidden | Overflow::Scroll | Overflow::Auto)
        };
        let vertical_bar = shows(self.overflow.y, self.auto_shows.y);
        let horizontal_bar = shows(self.overflow.x, self.auto_shows.x);
        if self.vertical {
            // The inline axis is vertical: `scrollbar-gutter` is about the horizontal scrollbar.
            if vertical_bar {
                edges.right = t;
            }
            if horizontal_bar || stable_applies(self.overflow.x) {
                edges.bottom = t;
                if self.both_edges && stable_applies(self.overflow.x) {
                    edges.top = t;
                }
            }
        } else {
            if vertical_bar || stable_applies(self.overflow.y) {
                if self.rtl {
                    edges.left = t;
                } else {
                    edges.right = t;
                }
                if self.both_edges && stable_applies(self.overflow.y) {
                    edges.left = t;
                    edges.right = t;
                }
            }
            if horizontal_bar {
                edges.bottom = t;
            }
        }
        Gutters { edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use style::values::computed::Overflow::{self, *};

    fn inputs(x: Overflow, y: Overflow) -> GutterInputs {
        GutterInputs {
            thickness: CLASSIC_THICKNESS,
            overflow: taffy::Point { x, y },
            auto_shows: taffy::Point { x: false, y: false },
            stable: false,
            both_edges: false,
            vertical: false,
            rtl: false,
        }
    }

    fn edges(g: Gutters) -> [f32; 4] {
        [g.edges.left, g.edges.right, g.edges.top, g.edges.bottom]
    }

    #[test]
    fn thickness_is_zero_for_overlay_and_none() {
        assert_eq!(thickness_in(ScrollbarMode::Overlay, ScrollbarWidth::Auto), 0.0);
        assert_eq!(thickness_in(ScrollbarMode::Classic, ScrollbarWidth::Auto), 15.0);
        assert_eq!(thickness_in(ScrollbarMode::Classic, ScrollbarWidth::Thin), 11.0);
        assert_eq!(thickness_in(ScrollbarMode::Classic, ScrollbarWidth::None), 0.0);
    }

    #[test]
    fn overflow_scroll_always_takes_both_gutters() {
        assert_eq!(edges(inputs(Scroll, Scroll).gutters()), [0.0, 15.0, 0.0, 15.0]);
    }

    #[test]
    fn overflow_auto_takes_a_gutter_only_when_its_scrollbar_shows() {
        let mut i = inputs(Auto, Auto);
        assert_eq!(edges(i.gutters()), [0.0; 4]);
        i.auto_shows.y = true;
        assert_eq!(edges(i.gutters()), [0.0, 15.0, 0.0, 0.0]);
    }

    #[test]
    fn stable_reserves_the_inline_end_gutter_for_auto_and_hidden_not_visible_or_clip() {
        for (y, reserved) in [(Auto, true), (Hidden, true), (Scroll, true), (Visible, false), (Clip, false)] {
            let mut i = inputs(Visible, y);
            i.stable = true;
            assert_eq!(i.gutters().edges.right, if reserved { 15.0 } else { 0.0 }, "{y:?}");
        }
    }

    #[test]
    fn both_edges_reserves_both_inline_edges() {
        let mut i = inputs(Hidden, Auto);
        i.stable = true;
        i.both_edges = true;
        assert_eq!(edges(i.gutters()), [15.0, 15.0, 0.0, 0.0]);
    }

    #[test]
    fn rtl_puts_the_vertical_scrollbar_on_the_left() {
        let mut i = inputs(Hidden, Scroll);
        i.rtl = true;
        assert_eq!(edges(i.gutters()), [15.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn vertical_writing_modes_reserve_the_stable_gutter_at_the_bottom() {
        let mut i = inputs(Auto, Auto);
        i.vertical = true;
        i.stable = true;
        assert_eq!(edges(i.gutters()), [0.0, 0.0, 0.0, 15.0]);
        // The vertical scrollbar stays on the right (Chrome, vertical-rl `overflow: scroll`).
        let mut i = inputs(Scroll, Scroll);
        i.vertical = true;
        assert_eq!(edges(i.gutters()), [0.0, 15.0, 0.0, 15.0]);
    }
}
