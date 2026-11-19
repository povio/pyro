//! pyro: scroll-driven animations (CSS Scroll-driven Animations 1). An animation whose
//! `animation-timeline` is `scroll()`, `view()` or a timeline name takes its progress from a scroll
//! position, not from time: this module resolves the timeline (its scroll container, axis and, for a
//! view progress timeline, its subject and insets) and the animation's attachment range
//! (`animation-range`), and gives the animation's progress through that range from the current scroll
//! offsets and the last layout (as browsers sample scroll timelines from the last layout).

use style::properties::ComputedValues;
use style::values::computed::animation::{AnimationRangeEnd, AnimationRangeStart, AnimationTimeline, ViewTimelineInset};
use style::values::computed::{LengthPercentage, LengthPercentageOrAuto};
use style::values::generics::animation::AnimationRangeValue;
use style::values::specified::animation::{ScrollAxis, Scroller, TimelineRangeName};
use style_traits::ToCss;

use crate::node::Node;
use crate::BaseDocument;
use blitz_traits::node_id::NodeId;

/// A resolved timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A scroll progress timeline of a scroll container.
    Scroll,
    /// A view progress timeline of `subject` in its nearest scroll container.
    View { subject: NodeId },
}

#[derive(Clone, Debug)]
struct Resolved {
    kind: Kind,
    /// The scroll container (`None`: the viewport, the root scroller).
    source: Option<NodeId>,
    /// Vertical (block axis of a horizontal writing mode, or `y`).
    vertical: bool,
    inset: Option<ViewTimelineInset>,
}

/// The scroll position, the scroll range and the scrollport size along an axis.
struct Scroll {
    offset: f64,
    max: f64,
    port: f64,
}

/// The progress of the animation at `index` of `node`'s style (`style`: the node's, or its
/// pseudo-element's) through its attachment range: 0 at the range's start, 1 at its end (outside
/// it below 0 or above 1). `None` when its timeline is `auto` (time-based) or inactive.
pub(crate) fn progress(doc: &BaseDocument, node: NodeId, style: &ComputedValues, index: usize) -> Option<f64> {
    let ui = style.get_ui();
    let count = ui.animation_timeline_count();
    if count == 0 {
        return None;
    }
    let timeline = ui.animation_timeline_at(index % count);
    let resolved = resolve(doc, node, &timeline)?;
    let scroll = scroll_of(doc, resolved.source, resolved.vertical)?;
    let start_count = ui.animation_range_start_count().max(1);
    let end_count = ui.animation_range_end_count().max(1);
    let range_start: AnimationRangeStart = ui.animation_range_start_at(index % start_count);
    let range_end: AnimationRangeEnd = ui.animation_range_end_at(index % end_count);
    let (start, end) = match resolved.kind {
        Kind::Scroll => {
            if scroll.max <= 0.0 {
                return None;
            }
            let whole = (0.0, scroll.max);
            (offset_in(&range_start.0, whole, whole, 0.0), offset_in(&range_end.0, whole, whole, 1.0))
        }
        Kind::View { subject } => {
            let ranges = view_ranges(doc, subject, &resolved, &scroll)?;
            (ranges.offset(&range_start.0, 0.0), ranges.offset(&range_end.0, 1.0))
        }
    };
    if !(end - start).is_finite() || end - start <= 0.0 {
        return None;
    }
    Some((scroll.offset - start) / (end - start))
}

/// One inset of a script-made view progress timeline (`ViewTimeline` `inset`): `auto`, a length
/// (px) or a percentage of the scrollport (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InsetValue {
    Auto,
    Px(f64),
    Fraction(f64),
}

/// What a script `ScrollTimeline` (`subject: None`) or `ViewTimeline` reads: the scroll offset of its
/// scroll container now and the scroll offsets of its start and end (a scroll timeline: 0 and the
/// scroll range; a view timeline: its cover range), and the scroll container (`None`: the viewport).
/// `None` when inactive: no scroll container (a `source` that is not one), or no layout.
pub fn timeline_state(
    doc: &BaseDocument,
    source: Option<NodeId>,
    subject: Option<NodeId>,
    axis: ScrollAxis,
    inset: [InsetValue; 2],
) -> Option<(f64, f64, f64, Option<NodeId>)> {
    timeline_ranges(doc, source, subject, axis, inset).map(|s| (s.offset, s.start, s.end, s.source))
}

/// [`timeline_state`] with a view timeline's contain range and subject size (a scroll timeline: its
/// whole range and 0), for an animation's attachment range (`rangeStart`/`rangeEnd`).
#[derive(Clone, Copy, Debug)]
pub struct TimelineRanges {
    pub offset: f64,
    pub start: f64,
    pub end: f64,
    pub source: Option<NodeId>,
    pub contain: (f64, f64),
    pub subject_size: f64,
}

pub fn timeline_ranges(
    doc: &BaseDocument,
    source: Option<NodeId>,
    subject: Option<NodeId>,
    axis: ScrollAxis,
    inset: [InsetValue; 2],
) -> Option<TimelineRanges> {
    match subject {
        None => {
            // The root element scrolls as the viewport, and so does a body that is not a scroll
            // container of its own (the quirks-mode `document.scrollingElement`).
            let source = source.filter(|&id| {
                id != doc.root_element().id
                    && doc.get_node(id).is_none_or(|n| {
                        !(n.data.is_element_with_tag_name(&markup5ever::local_name!("body"))
                            && (!is_scroll_container(n) || is_viewport_propagated_body(doc, n)))
                    })
            });
            let axis_node = match source {
                Some(id) => {
                    let n = doc.get_node(id)?;
                    if !is_scroll_container(n) {
                        return None;
                    }
                    n
                }
                None => doc.root_element(),
            };
            let scroll = scroll_of(doc, source, vertical(axis_node, axis))?;
            Some(TimelineRanges {
                offset: scroll.offset,
                start: 0.0,
                end: scroll.max,
                source,
                contain: (0.0, scroll.max),
                subject_size: 0.0,
            })
        }
        Some(subject) => {
            let n = doc.get_node(subject)?;
            n.primary_styles()?;
            let source = nearest_scroll_container(doc, n);
            let vertical = vertical(n, axis);
            let lp = |v: InsetValue| match v {
                InsetValue::Auto => LengthPercentageOrAuto::Auto,
                InsetValue::Px(px) => LengthPercentageOrAuto::LengthPercentage(LengthPercentage::new_length(
                    style::values::computed::Length::new(px as f32),
                )),
                InsetValue::Fraction(f) => LengthPercentageOrAuto::LengthPercentage(LengthPercentage::new_percent(
                    style::values::computed::Percentage(f as f32),
                )),
            };
            let resolved = Resolved {
                kind: Kind::View { subject },
                source,
                vertical,
                inset: Some(ViewTimelineInset { start: lp(inset[0]), end: lp(inset[1]) }),
            };
            let scroll = scroll_of(doc, source, vertical)?;
            let ranges = view_ranges(doc, subject, &resolved, &scroll)?;
            let subject_size = ranges.entry_crossing.1 - ranges.entry_crossing.0;
            Some(TimelineRanges {
                offset: scroll.offset,
                start: ranges.cover.0,
                end: ranges.cover.1,
                source,
                contain: ranges.contain,
                subject_size,
            })
        }
    }
}

/// Whether the animation at `index` of `style` is driven by a scroll or view progress timeline.
pub fn timeline_driven(style: &ComputedValues, index: usize) -> bool {
    let ui = style.get_ui();
    let count = ui.animation_timeline_count();
    count > 0 && !ui.animation_timeline_at(index % count).is_auto()
}

/// The scroll offset of a range value: a named range's start plus the length-percentage of its length,
/// or (no name) the length-percentage of `whole`. `default` (0 or 1) stands for `normal`.
fn offset_in(value: &AnimationRangeValue<LengthPercentage>, range: (f64, f64), whole: (f64, f64), default: f64) -> f64 {
    let (from, to) = match value.name {
        TimelineRangeName::Normal => return whole.0 + (whole.1 - whole.0) * default,
        TimelineRangeName::None => whole,
        _ => range,
    };
    let length = to - from;
    from + value.lp.resolve(app_units::Au::from_f64_px(length).into()).px() as f64
}

/// A view progress timeline's named ranges, as scroll offsets of its scroll container.
struct ViewRanges {
    cover: (f64, f64),
    contain: (f64, f64),
    entry: (f64, f64),
    exit: (f64, f64),
    entry_crossing: (f64, f64),
    exit_crossing: (f64, f64),
}

impl ViewRanges {
    fn offset(&self, value: &AnimationRangeValue<LengthPercentage>, default: f64) -> f64 {
        let range = match value.name {
            TimelineRangeName::Contain => self.contain,
            TimelineRangeName::Entry => self.entry,
            TimelineRangeName::Exit => self.exit,
            TimelineRangeName::EntryCrossing => self.entry_crossing,
            TimelineRangeName::ExitCrossing => self.exit_crossing,
            _ => self.cover,
        };
        offset_in(value, range, self.cover, default)
    }
}

fn view_ranges(doc: &BaseDocument, subject: NodeId, resolved: &Resolved, scroll: &Scroll) -> Option<ViewRanges> {
    let subject_node = doc.get_node(subject)?;
    let size = subject_node.final_layout().size;
    let subject_size = if resolved.vertical { size.height } else { size.width } as f64;
    // The subject's start edge in the scroll container's content coordinates (at scroll offset 0).
    // `absolute_position` takes off the scroll offsets on the way up, the subject's own included
    // (added back); the scroll container's and its ancestors' cancel out against its own position.
    let subject_pos = subject_node.absolute_position(0.0, 0.0);
    let own_scroll = subject_node.scroll_offset();
    let subject_start = if resolved.vertical { subject_pos.y as f64 + own_scroll.y } else { subject_pos.x as f64 + own_scroll.x };
    let port_start = match resolved.source {
        Some(source) => {
            let source = doc.get_node(source)?;
            let p = source.absolute_position(0.0, 0.0);
            let border = source.final_layout().border;
            (if resolved.vertical { p.y + border.top } else { p.x + border.left }) as f64
        }
        None => 0.0,
    };
    let start = subject_start - port_start;
    let end = start + subject_size;
    let (inset_start, inset_end) = match &resolved.inset {
        Some(inset) => (inset_px(&inset.start, scroll.port), inset_px(&inset.end, scroll.port)),
        None => (0.0, 0.0),
    };
    let port = scroll.port;
    let cover = (start - port + inset_end, end - inset_start);
    let contain = if subject_size <= port - inset_start - inset_end {
        (end - port + inset_end, start - inset_start)
    } else {
        (start - inset_start, end - port + inset_end)
    };
    Some(ViewRanges {
        cover,
        contain,
        entry: (cover.0, contain.0),
        exit: (contain.1, cover.1),
        entry_crossing: (cover.0, cover.0 + subject_size),
        exit_crossing: (cover.1 - subject_size, cover.1),
    })
}

fn inset_px(value: &LengthPercentageOrAuto, port: f64) -> f64 {
    match value {
        LengthPercentageOrAuto::Auto => 0.0,
        LengthPercentageOrAuto::LengthPercentage(lp) => lp.resolve(app_units::Au::from_f64_px(port).into()).px() as f64,
    }
}

/// The timeline `timeline` names for `node`.
fn resolve(doc: &BaseDocument, node: NodeId, timeline: &AnimationTimeline) -> Option<Resolved> {
    let n = doc.get_node(node)?;
    match timeline {
        AnimationTimeline::Auto => None,
        AnimationTimeline::Scroll(f) => {
            let source = match f.scroller {
                Scroller::Root => None,
                Scroller::SelfElement => Some(node).filter(|_| is_scroll_container(n)),
                Scroller::Nearest => nearest_scroll_container(doc, n),
            };
            // A `self` that is not a scroll container: inactive.
            if f.scroller == Scroller::SelfElement && source.is_none() {
                return None;
            }
            Some(Resolved { kind: Kind::Scroll, source, vertical: vertical(n, f.axis), inset: None })
        }
        AnimationTimeline::View(v) => Some(Resolved {
            kind: Kind::View { subject: node },
            source: nearest_scroll_container(doc, n),
            vertical: vertical(n, v.axis),
            inset: Some(v.inset.clone()),
        }),
        AnimationTimeline::Timeline(name) => {
            let name = name.to_css_string();
            if name == "none" || name.is_empty() {
                return None;
            }
            named(doc, n, &name)
        }
    }
}

/// The nearest ancestor-or-self of `node` declaring timeline `name`: a scroll progress timeline
/// before a view progress timeline on the same element (Scroll-driven Animations 1 §4.4.1).
fn named(doc: &BaseDocument, node: &Node, name: &str) -> Option<Resolved> {
    let mut current = Some(node.id);
    while let Some(id) = current {
        let el = doc.get_node(id)?;
        if let Some(style) = el.primary_styles() {
            let ui = style.get_ui();
            let axes = ui.scroll_timeline_axis_count().max(1);
            for i in (0..ui.scroll_timeline_name_count()).rev() {
                if ui.scroll_timeline_name_at(i).to_css_string() == name {
                    let axis = ui.scroll_timeline_axis_at(i % axes);
                    // The element itself is the scroll container (none: inactive).
                    if !is_scroll_container(el) {
                        return None;
                    }
                    return Some(Resolved { kind: Kind::Scroll, source: Some(id), vertical: vertical(el, axis), inset: None });
                }
            }
            let axes = ui.view_timeline_axis_count().max(1);
            let insets = ui.view_timeline_inset_count().max(1);
            for i in (0..ui.view_timeline_name_count()).rev() {
                if ui.view_timeline_name_at(i).to_css_string() == name {
                    return Some(Resolved {
                        kind: Kind::View { subject: id },
                        source: nearest_scroll_container(doc, el),
                        vertical: vertical(el, ui.view_timeline_axis_at(i % axes)),
                        inset: Some(ui.view_timeline_inset_at(i % insets)),
                    });
                }
            }
        }
        current = el.parent.filter(|&p| doc.get_node(p).is_some_and(Node::is_element));
    }
    None
}

/// Whether the axis is vertical for `node`'s writing mode.
fn vertical(node: &Node, axis: ScrollAxis) -> bool {
    let horizontal_writing = node.primary_styles().is_none_or(|s| s.writing_mode.is_horizontal());
    match axis {
        ScrollAxis::Y => true,
        ScrollAxis::X => false,
        ScrollAxis::Block => horizontal_writing,
        ScrollAxis::Inline => !horizontal_writing,
    }
}

fn is_scroll_container(node: &Node) -> bool {
    use style::values::computed::Overflow;
    let Some(style) = node.primary_styles() else { return false };
    let b = style.get_box();
    let scrolls = |o: Overflow| matches!(o, Overflow::Auto | Overflow::Scroll | Overflow::Hidden);
    scrolls(b.overflow_x) || scrolls(b.overflow_y)
}

/// The nearest ancestor scroll container (`None`: the viewport).
fn nearest_scroll_container(doc: &BaseDocument, node: &Node) -> Option<NodeId> {
    let root = doc.root_element().id;
    let mut current = node.parent;
    while let Some(id) = current {
        let n = doc.get_node(id)?;
        if id == root || !n.is_element() {
            return None;
        }
        // (The body's overflow propagates to the viewport.)
        if is_scroll_container(n) && !is_viewport_propagated_body(doc, n) {
            return Some(id);
        }
        current = n.parent;
    }
    None
}

fn is_viewport_propagated_body(doc: &BaseDocument, node: &Node) -> bool {
    let is_body = node.data.is_element_with_tag_name(&markup5ever::local_name!("body"));
    if !is_body {
        return false;
    }
    // The root's overflow is `visible`: the body's goes to the viewport.
    !is_scroll_container(doc.root_element())
}

/// The scroll state of `source` (`None`: the viewport) along the axis.
fn scroll_of(doc: &BaseDocument, source: Option<NodeId>, vertical: bool) -> Option<Scroll> {
    match source {
        Some(id) => {
            let n = doc.get_node(id)?;
            // The page's offset (layout queries leave the node's own as the last capture set it).
            let own = (n.scroll_offset().x, n.scroll_offset().y);
            let page = match &doc.timeline_scroll {
                Some((elements, _)) => elements.get(&id).copied().unwrap_or((0.0, 0.0)),
                None => own,
            };
            let offset = if vertical { page.1 } else { page.0 };
            let (client, size) = if vertical {
                (n.client_height() as f64, n.scroll_height() as f64)
            } else {
                (n.client_width() as f64, n.scroll_width() as f64)
            };
            Some(Scroll { offset, max: (size - client).max(0.0), port: client })
        }
        None => {
            let root = doc.root_element();
            let viewport = doc.viewport();
            let scale = viewport.zoom() as f64;
            let (w, h) = viewport.window_size;
            let port = if vertical { h as f64 / scale } else { w as f64 / scale };
            let page = match &doc.timeline_scroll {
                Some((_, viewport)) => *viewport,
                None => (doc.viewport_scroll().x, doc.viewport_scroll().y),
            };
            let offset = if vertical { page.1 } else { page.0 };
            let size = if vertical { root.scroll_height() as f64 } else { root.scroll_width() as f64 };
            Some(Scroll { offset, max: (size - port).max(0.0), port })
        }
    }
}
