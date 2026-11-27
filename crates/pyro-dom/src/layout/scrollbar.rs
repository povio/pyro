//! Classic scrollbar parts under the pointer (CSS Overflow 3 § 3: the scrollbars of a scroll
//! container are UA-drawn boxes between its border and its padding): which part a client point
//! is on, with the geometry a thumb drag needs. The thumb is laid out as painting does it
//! (`blitz_paint::paint_classic_scrollbar`): the track between the two arrow buttons, a thumb as
//! long as the track times `client / scroll` (at least as long as the bar is thick), at the scroll
//! offset's share of the range.

use super::*;

/// A part of a classic scrollbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarPart {
    /// The arrow button at the start of the bar.
    ButtonStart,
    /// The arrow button at the end of the bar.
    ButtonEnd,
    /// The track before the thumb.
    TrackBefore,
    /// The track after the thumb.
    TrackAfter,
    Thumb,
}

/// A classic scrollbar part a client point is on, and the scrollbar's geometry along its axis
/// (client px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarHit {
    /// The scroll container (`None`: the viewport).
    pub target: Option<NodeId>,
    /// 0: the horizontal scrollbar (it scrolls x), 1: the vertical one.
    pub axis: usize,
    pub part: ScrollbarPart,
    /// Where the track between the buttons starts and how long it is.
    pub room_start: f64,
    pub room: f64,
    /// Where the thumb starts and how long it is.
    pub thumb_start: f64,
    pub thumb_len: f64,
    /// The scrollport's length along the axis and the scroll range (`scroll - client`).
    pub client: f64,
    pub range: f64,
    /// The axis scrolls to negative offsets (its scroll origin is at its end).
    pub reversed: bool,
}

/// One scrollbar laid out: its track (x0, y0, x1, y1; client px), and what the thumb follows.
struct Bar {
    track: [f64; 4],
    vertical: bool,
    client: f64,
    scroll: f64,
    /// The scroll offset from the start of the range.
    offset: f64,
    reversed: bool,
    target: Option<NodeId>,
}

impl Bar {
    /// The part at `point`, when it is on the track and the content overflows.
    fn hit(&self, point: (f64, f64)) -> Option<ScrollbarHit> {
        let [x0, y0, x1, y1] = self.track;
        if point.0 < x0 || point.0 >= x1 || point.1 < y0 || point.1 >= y1 {
            return None;
        }
        let (thickness, length, start, along) =
            if self.vertical { (x1 - x0, y1 - y0, y0, point.1) } else { (y1 - y0, x1 - x0, x0, point.0) };
        // The buttons are as long as the bar is thick, while two of them fit.
        let button = if length >= 2.0 * thickness { thickness } else { 0.0 };
        let room = length - 2.0 * button;
        if self.scroll <= self.client + 0.5 || room <= 0.0 || self.client <= 0.0 {
            return None;
        }
        let thumb_len = (room * self.client / self.scroll).clamp(thickness.min(room), room).round();
        let range = self.scroll - self.client;
        let progress = (self.offset / range).clamp(0.0, 1.0);
        let thumb_start = start + (button + progress * (room - thumb_len)).round();
        let part = if along < start + button {
            ScrollbarPart::ButtonStart
        } else if along >= start + length - button {
            ScrollbarPart::ButtonEnd
        } else if along < thumb_start {
            ScrollbarPart::TrackBefore
        } else if along >= thumb_start + thumb_len {
            ScrollbarPart::TrackAfter
        } else {
            ScrollbarPart::Thumb
        };
        Some(ScrollbarHit {
            target: self.target,
            axis: usize::from(self.vertical),
            part,
            room_start: start + button,
            room,
            thumb_start,
            thumb_len,
            client: self.client,
            range,
            reversed: self.reversed,
        })
    }
}

impl Layout {
    /// The classic scrollbar part at the client point `point` that belongs to `el`, the element
    /// the point hit (`None`: not on a scrollbar of it, or its scrollbars are overlays). The root
    /// element answers for the viewport's scrollbars, which a hit in their gutters goes to.
    pub fn scrollbar_hit(&self, dom: &Dom, el: NodeId, point: (f64, f64)) -> Option<ScrollbarHit> {
        let source = self.synced_geometry_source(dom)?.clone();
        if dom.document_element(self.document()).ok().flatten() == Some(el) {
            return self.viewport_scrollbar_hit(dom, &source, point);
        }
        if self.is_root_container(dom, el) {
            return None;
        }
        let b = source.element_box(crate::snapshot::render_key(el))?;
        if b.hidden {
            return None;
        }
        let r = self.bounding_client_rect(dom, el);
        // A box scaled by a transform has no 1:1 scrollbar: not dragged.
        if (r.width - b.width).abs() > 0.5 || (r.height - b.height).abs() > 0.5 {
            return None;
        }
        // The padding box, which the gutters are at the edges of.
        let (x0, y0) = (r.x + b.border[LEFT], r.y + b.border[TOP]);
        let (x1, y1) = (r.x + r.width - b.border[RIGHT], r.y + r.height - b.border[BOTTOM]);
        if point.0 < x0 || point.0 >= x1 || point.1 < y0 || point.1 >= y1 {
            return None;
        }
        // What the content box does not have of the padding box is the gutters.
        let gutter_x = ((x1 - x0) - b.client_width).max(0.0);
        let gutter_y = ((y1 - y0) - b.client_height).max(0.0);
        if gutter_x < 0.5 && gutter_y < 0.5 {
            return None;
        }
        let rtl = self.style.computed_value(dom, el, "direction").is_some_and(|d| d == "rtl");
        let horizontal_mode = self.style.computed_value(dom, el, "writing-mode").is_none_or(|w| w.starts_with("horizontal"));
        let on_left = rtl && horizontal_mode;
        let (scroll_x, scroll_y) = self.element_scroll.get(&el).copied().unwrap_or((0, 0));
        let from_start = |offset: i32, reversed: bool, scroll: f64, client: f64| {
            let offset = offset as f64;
            if reversed { offset + (scroll - client).max(0.0) } else { offset }
        };
        let mut bars = Vec::new();
        if gutter_x >= 0.5 {
            let left = if on_left { x0 } else { x1 - gutter_x };
            bars.push(Bar {
                track: [left, y0, left + gutter_x, y1 - gutter_y],
                vertical: true,
                client: b.client_height,
                scroll: b.scroll_height,
                offset: from_start(scroll_y, b.scroll_reversed[1], b.scroll_height, b.client_height),
                reversed: b.scroll_reversed[1],
                target: Some(el),
            });
        }
        if gutter_y >= 0.5 {
            let (left, right) = if gutter_x >= 0.5 { if on_left { (gutter_x, 0.0) } else { (0.0, gutter_x) } } else { (0.0, 0.0) };
            bars.push(Bar {
                track: [x0 + left, y1 - gutter_y, x1 - right, y1],
                vertical: false,
                client: b.client_width,
                scroll: b.scroll_width,
                offset: from_start(scroll_x, b.scroll_reversed[0], b.scroll_width, b.client_width),
                reversed: b.scroll_reversed[0],
                target: Some(el),
            });
        }
        bars.iter().find_map(|bar| bar.hit(point))
    }

    /// [`Self::scrollbar_hit`] for the viewport's scrollbars (the gutters at the window's edges).
    fn viewport_scrollbar_hit(&self, dom: &Dom, source: &Rc<dyn GeometrySource>, point: (f64, f64)) -> Option<ScrollbarHit> {
        let [gl, gt, gr, gb] = source.viewport_gutters()?;
        let (w, h) = self.window_size();
        let (w, h) = (w as f64, h as f64);
        let (gutter_x, gutter_y) = (gl + gr, gt + gb);
        let on_left = gl > 0.0;
        let (client_w, client_h) = self.without_scrollbars(dom, (w as u32, h as u32));
        let (scroll_w, scroll_h) = self.document_extent(dom);
        let reversed_x = self.viewport_x_reversed(dom);
        let (scroll_x, scroll_y) = self.window_scroll;
        let mut bars = Vec::new();
        if gutter_x >= 0.5 {
            let left = if on_left { 0.0 } else { w - gutter_x };
            bars.push(Bar {
                track: [left, 0.0, left + gutter_x, h - gutter_y],
                vertical: true,
                client: client_h,
                scroll: scroll_h,
                offset: scroll_y as f64,
                reversed: false,
                target: None,
            });
        }
        if gutter_y >= 0.5 {
            let (left, right) = if gutter_x >= 0.5 { if on_left { (gutter_x, 0.0) } else { (0.0, gutter_x) } } else { (0.0, 0.0) };
            let offset = scroll_x as f64 + if reversed_x { (scroll_w - client_w).max(0.0) } else { 0.0 };
            bars.push(Bar {
                track: [left, h - gutter_y, w - right, h],
                vertical: false,
                client: client_w,
                scroll: scroll_w,
                offset,
                reversed: reversed_x,
                target: None,
            });
        }
        bars.iter().find_map(|bar| bar.hit(point))
    }

    /// Whether snapping is held off ([`Self::set_snap_suspended`]).
    pub fn snap_suspended(&self) -> bool {
        self.snap_suspended
    }

    /// Holds snapping off for the scroll writes that follow: a scrollbar thumb drag follows the
    /// pointer unsnapped, and snaps where it ends (CSS Scroll Snap 1 § 6.2: a user scroll snaps
    /// at its end position).
    pub fn set_snap_suspended(&mut self, suspended: bool) {
        self.snap_suspended = suspended;
    }
}
