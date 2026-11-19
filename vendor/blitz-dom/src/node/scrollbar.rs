//! Overlay scrollbar geometry and css-scrollbars-1 style accessors for
//! [`Node`]. Geometry is shared between painting (blitz-paint) and thumb
//! hit-testing so the two cannot drift.

use blitz_traits::node_id::NodeId;
use kurbo::Rect as KurboRect;
use stylo_taffy::CopyStyleGetters;
use taffy::AbsoluteAxis;
use web_time::Duration;

use super::Node;

/// How long overlay scrollbars stay fully opaque after their last activity
/// (a scroll, or the pointer leaving the thumb), and how long the fade-out
/// takes. Chromium's overlay values.
pub(crate) const FADE_DELAY: Duration = Duration::from_millis(500);
pub(crate) const FADE_DURATION: Duration = Duration::from_millis(200);

/// Overlay scrollbar opacity as a function of time since the scroll
/// container's last scrollbar activity: fully opaque through the fade
/// delay, then fading linearly to hidden.
pub(crate) fn opacity_at(elapsed: Duration) -> f32 {
    match elapsed.checked_sub(FADE_DELAY) {
        None => 1.0,
        Some(fading) => 1.0 - (fading.as_secs_f32() / FADE_DURATION.as_secs_f32()).min(1.0),
    }
}

/// A specific scrollbar: one axis of one scroll container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarRef {
    pub node_id: NodeId,
    pub axis: AbsoluteAxis,
}

/// The computed value of `scrollbar-width` (css-scrollbars-1).
pub use style::properties::generated::longhands::scrollbar_width::computed_value::T as ScrollbarWidth;

/// The computed value of `scrollbar-color` (css-scrollbars-1), with colors
/// fully resolved (no `currentColor`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ScrollbarColor {
    #[default]
    Auto,
    Colors {
        thumb: style::color::AbsoluteColor,
        track: style::color::AbsoluteColor,
    },
}

/// pyro: a box's classic scrollbars ([`Node::classic_scrollbars`]), physical, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicScrollbars {
    /// The gutter at each edge of the padding box.
    pub gutters: taffy::Rect<f32>,
    /// Whether the horizontal (x) and vertical (y) scrollbars show (a gutter `scrollbar-gutter`
    /// reserves alone shows none).
    pub shown: taffy::Point<bool>,
    /// Whether the vertical scrollbar is on the left (`direction: rtl`).
    pub vertical_on_left: bool,
}

impl Node {
    /// pyro: the box's classic scrollbars, when the process has classic scrollbars
    /// (`stylo_taffy::scrollbars`) and the box has any gutter; as layout reserved them
    /// (`TaffyStyloStyle::scrollbar_gutter`).
    pub fn classic_scrollbars(&self) -> Option<ClassicScrollbars> {
        use style::values::computed::{Overflow, ScrollbarGutter};
        use stylo_taffy::scrollbars::{GutterInputs, ScrollbarMode, scrollbar_mode, thickness};
        if scrollbar_mode() != ScrollbarMode::Classic
            || self.is_root_element()
            || self.flags.contains(super::NodeFlags::OVERFLOW_TO_VIEWPORT)
        {
            return None;
        }
        let el = self.element_data()?;
        // (A replaced element is no scroll container: Chrome paints no scrollbars on an
        // `overflow: scroll` image, WPT css-overflow `overflow-img-scroll-replaced`.)
        if el.name.local == crate::local_name!("input")
            || el.name.local == crate::local_name!("frameset")
            || crate::layout::replaced::is_replaced(el)
        {
            return None;
        }
        let style = self.primary_styles()?;
        let thickness = thickness(style.clone_scrollbar_width());
        if thickness <= 0.0 {
            return None;
        }
        let box_styles = style.get_box();
        let overflow = taffy::Point { x: box_styles.overflow_x, y: box_styles.overflow_y };
        let auto_shows = taffy::Point {
            x: self.flags.contains(super::NodeFlags::AUTO_SCROLLBAR_X),
            y: self.flags.contains(super::NodeFlags::AUTO_SCROLLBAR_Y),
        };
        let wm = style.writing_mode;
        let inputs = GutterInputs {
            thickness,
            overflow,
            auto_shows,
            stable: box_styles.scrollbar_gutter.contains(ScrollbarGutter::STABLE),
            both_edges: box_styles.scrollbar_gutter.contains(ScrollbarGutter::BOTH_EDGES),
            vertical: wm.is_vertical(),
            rtl: !wm.is_bidi_ltr(),
        };
        let gutters = inputs.gutters().edges;
        if gutters.left + gutters.right + gutters.top + gutters.bottom <= 0.0 {
            return None;
        }
        let shows = |overflow: Overflow, auto: bool| overflow == Overflow::Scroll || (overflow == Overflow::Auto && auto);
        Some(ClassicScrollbars {
            gutters,
            shown: taffy::Point { x: shows(overflow.x, auto_shows.x), y: shows(overflow.y, auto_shows.y) },
            vertical_on_left: inputs.rtl && !inputs.vertical,
        })
    }

    /// The node's computed `scrollbar-width`.
    pub fn scrollbar_width(&self) -> ScrollbarWidth {
        self.primary_styles()
            .map(|style| style.clone_scrollbar_width())
            .unwrap_or(ScrollbarWidth::Auto)
    }

    /// The node's computed `scrollbar-color`, with colors resolved against
    /// the element's computed `color`.
    pub fn scrollbar_color(&self) -> ScrollbarColor {
        let Some(style) = self.primary_styles() else {
            return ScrollbarColor::Auto;
        };
        let current_color = style.clone_color();
        match style.get_scrollbar_color() {
            style::values::computed::ScrollbarColor::Auto => ScrollbarColor::Auto,
            style::values::computed::ScrollbarColor::Colors { thumb, track } => {
                ScrollbarColor::Colors {
                    thumb: thumb.resolve_to_absolute(&current_color),
                    track: track.resolve_to_absolute(&current_color),
                }
            }
        }
    }

    /// Whether the node shows an overlay scrollbar in the given axis:
    /// always for `overflow: scroll`, only when the content overflows for
    /// `overflow: auto`, never otherwise — and never when
    /// `scrollbar-width: none`.
    pub fn wants_scrollbar(&self, axis: AbsoluteAxis) -> bool {
        use style::values::computed::Overflow;
        let Some(style) = self.primary_styles() else {
            return false;
        };
        if self.scrollbar_width() == ScrollbarWidth::None {
            return false;
        }
        let (overflow, scroll_extent) = match axis {
            AbsoluteAxis::Horizontal => (
                style.clone_overflow_x(),
                (self.scroll_width() - self.client_width()) as f64,
            ),
            AbsoluteAxis::Vertical => (
                style.clone_overflow_y(),
                (self.scroll_height() - self.client_height()) as f64,
            ),
        };
        match overflow {
            Overflow::Scroll => true,
            Overflow::Auto => scroll_extent > 0.5,
            _ => false,
        }
    }

    /// The scrollport (padding box) in (unscaled) CSS px relative to the
    /// node's border-box origin. Taffy has content-box helpers but none for
    /// the padding box.
    fn scrollport(&self) -> KurboRect {
        let layout = self.final_layout();
        KurboRect::new(
            layout.border.left as f64,
            layout.border.top as f64,
            layout.size.width as f64 - layout.border.right as f64,
            layout.size.height as f64 - layout.border.bottom as f64,
        )
    }

    /// Geometry of the overlay scrollbar thumb for the given axis, in
    /// (unscaled) CSS px relative to the node's border-box origin. `None`
    /// if there is no scrollable overflow in that axis.
    pub fn scrollbar_thumb(&self, axis: AbsoluteAxis) -> Option<KurboRect> {
        // Matches Chromium's overlay thumb in its interactive state.
        const THUMB_THICKNESS: f64 = 10.0;
        const THIN_THUMB_THICKNESS: f64 = 6.0;
        const THUMB_MARGIN: f64 = 2.0;
        const MIN_THUMB_LENGTH: f64 = 32.0;

        let (min, max) = self.scroll_range();
        let (scroll_extent, scroll_min) = match axis {
            AbsoluteAxis::Horizontal => (max.x - min.x, min.x),
            AbsoluteAxis::Vertical => (max.y - min.y, min.y),
        };
        if scroll_extent <= 0.5 {
            return None;
        }

        let thickness = match self.scrollbar_width() {
            ScrollbarWidth::Thin => THIN_THUMB_THICKNESS,
            _ => THUMB_THICKNESS,
        };

        let port = self.scrollport();
        let (viewport_len, scroll_offset) = match axis {
            AbsoluteAxis::Horizontal => (port.width(), self.scroll_offset().x),
            AbsoluteAxis::Vertical => (port.height(), self.scroll_offset().y),
        };
        let thumb_len = (viewport_len * viewport_len / (viewport_len + scroll_extent))
            .max(MIN_THUMB_LENGTH)
            .min(viewport_len);
        let progress = ((scroll_offset - scroll_min) / scroll_extent).clamp(0.0, 1.0);
        // Round a sub-pixel displacement up to a whole pixel so any nonzero
        // scroll visibly moves the thumb off the origin.
        let thumb_start = match progress * (viewport_len - thumb_len) {
            start if start > 0.0 && start < 1.0 => 1.0,
            start => start,
        };

        Some(match axis {
            AbsoluteAxis::Horizontal => KurboRect::new(
                port.x0 + thumb_start,
                port.y1 - THUMB_MARGIN - thickness,
                port.x0 + thumb_start + thumb_len,
                port.y1 - THUMB_MARGIN,
            ),
            AbsoluteAxis::Vertical => KurboRect::new(
                port.x1 - THUMB_MARGIN - thickness,
                port.y0 + thumb_start,
                port.x1 - THUMB_MARGIN,
                port.y0 + thumb_start + thumb_len,
            ),
        })
    }

    /// Content px scrolled per thumb px dragged, for the given axis.
    pub fn scrollbar_drag_ratio(&self, axis: AbsoluteAxis) -> f64 {
        let Some(thumb) = self.scrollbar_thumb(axis) else {
            return 0.0;
        };
        let port = self.scrollport();
        let (scroll_extent, viewport_len, thumb_len) = match axis {
            AbsoluteAxis::Horizontal => (
                (self.scroll_width() - self.client_width()) as f64,
                port.width(),
                thumb.width(),
            ),
            AbsoluteAxis::Vertical => (
                (self.scroll_height() - self.client_height()) as f64,
                port.height(),
                thumb.height(),
            ),
        };
        let track_play = viewport_len - thumb_len;
        if track_play <= 0.0 {
            return 0.0;
        }
        scroll_extent / track_play
    }

    /// The scrollbar thumb containing the given point (in this node's
    /// border-box coordinates), if any. The `scrollbars` feature's single
    /// behavioral gate: returning `None` keeps unpainted thumbs from ever
    /// claiming pointer events.
    pub(crate) fn scrollbar_at_local(&self, x: f64, y: f64) -> Option<ScrollbarRef> {
        if !cfg!(feature = "scrollbars") {
            return None;
        }
        for axis in [AbsoluteAxis::Vertical, AbsoluteAxis::Horizontal] {
            if !self.wants_scrollbar(axis) {
                continue;
            }
            if let Some(thumb) = self.scrollbar_thumb(axis)
                && thumb.contains(kurbo::Point::new(x, y))
            {
                return Some(ScrollbarRef {
                    node_id: self.id,
                    axis,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_holds_through_the_fade_delay_then_fades_out() {
        assert_eq!(opacity_at(Duration::ZERO), 1.0);
        assert_eq!(opacity_at(FADE_DELAY), 1.0);
        let mid_fade = opacity_at(FADE_DELAY + FADE_DURATION / 2);
        assert!((mid_fade - 0.5).abs() < 0.01, "got {mid_fade}");
        assert_eq!(opacity_at(FADE_DELAY + FADE_DURATION), 0.0);
        assert_eq!(opacity_at(Duration::from_secs(3600)), 0.0);
    }
}
