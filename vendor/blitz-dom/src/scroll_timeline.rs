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
/// pyro: and whether the timeline is at its end (its scroll container scrolled to the end of its
/// range), where an animation's active end is inclusive (Web Animations 2, the "at progress
/// timeline boundary" of the after phase).
pub(crate) fn progress(doc: &BaseDocument, node: NodeId, style: &ComputedValues, index: usize) -> Option<(f64, bool)> {
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
    Some(((scroll.offset - start) / (end - start), scroll.offset >= scroll.max))
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
            // pyro: a subject that generates no box (`display: none`) makes the view timeline
            // inactive (Scroll-driven Animations § 4.2; WPT scroll-animations
            // `css/view-timeline-dynamic`), not one measured from its stale layout.
            if !n.has_boxes() {
                return None;
            }
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
        Some(inset) => {
            // An `auto` inset is the scroll container's `scroll-padding` on that side: the snapport
            // is the view progress visibility range (Scroll-driven Animations 1).
            let padding = resolved.source.and_then(|id| doc.get_node(id)).or_else(|| doc.try_root_element()).and_then(|n| n.primary_styles()).map(|style| {
                let padding = style.get_padding();
                let basis = app_units::Au::from_f64_px(scroll.port);
                let sides = if resolved.vertical { [&padding.scroll_padding_top, &padding.scroll_padding_bottom] } else { [&padding.scroll_padding_left, &padding.scroll_padding_right] };
                sides.map(|value| value.to_used_value(basis).map_or(0.0, app_units::Au::to_f64_px))
            });
            let [auto_start, auto_end] = padding.unwrap_or([0.0, 0.0]);
            (inset_px(&inset.start, scroll.port, auto_start), inset_px(&inset.end, scroll.port, auto_end))
        }
        None => (0.0, 0.0),
    };
    let port = scroll.port;
    // pyro: a sticky subject (or one in a sticky box) moves while it is stuck: its start edge is
    // `start + shift(scroll offset)`, and each range edge is where that moving edge meets the
    // port's (Scroll-driven Animations 1 § "view progress timeline", Chrome's sticky ranges).
    if let Some(sticky) = sticky_travel(doc, subject, resolved, port, port_start) {
        let edges = (
            sticky.first(start, port - inset_end),
            sticky.first(start, port - inset_end - subject_size),
            sticky.last(start, inset_start),
            sticky.last(start, inset_start - subject_size),
        );
        if let (Some(a), Some(b), Some(c), Some(d)) = edges {
            let cover = (a, d);
            let contain = if subject_size <= port - inset_start - inset_end { (b, c) } else { (c, b) };
            return Some(ViewRanges {
                cover,
                contain,
                entry: (cover.0, contain.0),
                exit: (contain.1, cover.1),
                entry_crossing: (a, b),
                exit_crossing: (c, d),
            });
        }
    }
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

/// How far the sticky box holding a view timeline's subject moves with the scroll offset (along the
/// timeline's axis): the box's constraint rectangle, in the scroll container's content coordinates.
struct StickyTravel {
    /// The sticky box's static start edge, its size and its margins along the axis.
    top: f64,
    size: f64,
    margin: (f64, f64),
    /// Its `top`/`left` and `bottom`/`right` insets (`None`: auto).
    inset: (Option<f64>, Option<f64>),
    /// The content box of its containing block.
    block: (f64, f64),
    port: f64,
}

impl StickyTravel {
    /// The offset of the box from its static place at scroll offset `scroll` (CSS Positioned Layout 3
    /// § 3.4: the insets move it into the scrollport, its containing block holds it).
    fn shift(&self, scroll: f64) -> f64 {
        let mut shift: f64 = 0.0;
        if let Some(end) = self.inset.1 {
            shift = shift.min(scroll + self.port - end - (self.top + self.size));
        }
        if let Some(start) = self.inset.0 {
            shift = shift.max(scroll + start - self.top);
        }
        let high = self.block.1 - (self.top + self.size + self.margin.1);
        let low = self.block.0 - (self.top - self.margin.0);
        shift.min(high).max(low)
    }

    /// The scroll offsets where `shift` changes its slope.
    fn breaks(&self) -> Vec<f64> {
        let high = self.block.1 - (self.top + self.size + self.margin.1);
        let low = self.block.0 - (self.top - self.margin.0);
        let mut out = vec![0.0];
        for (shift, from_start) in [(0.0, true), (high, true), (low, true), (0.0, false), (high, false), (low, false)] {
            if from_start {
                if let Some(start) = self.inset.0 { out.push(shift + self.top - start); }
            } else if let Some(end) = self.inset.1 {
                out.push(shift + self.top + self.size + end - self.port);
            }
        }
        out.retain(|s| s.is_finite());
        out.sort_by(|a, b| a.total_cmp(b));
        out.dedup();
        out
    }

    /// The subject's start edge, in port coordinates, at `scroll`; `start` is its static content start.
    fn edge(&self, start: f64, scroll: f64) -> f64 {
        start + self.shift(scroll) - scroll
    }

    /// The smallest scroll offset where the (never rising) edge is at or before `at`.
    fn first(&self, start: f64, at: f64) -> Option<f64> {
        let breaks = self.breaks();
        let y = |s: f64| self.edge(start, s);
        let (first, last) = (*breaks.first()?, *breaks.last()?);
        if y(first) <= at {
            let slope = y(first) - y(first - 1.0);
            return Some(if slope < 0.0 { first + y(first) - at } else { first });
        }
        for pair in breaks.windows(2) {
            let (y0, y1) = (y(pair[0]), y(pair[1]));
            if y1 <= at {
                return Some(pair[0] + (at - y0) * (pair[1] - pair[0]) / (y1 - y0));
            }
        }
        let slope = y(last + 1.0) - y(last);
        (slope < 0.0).then(|| last + (at - y(last)) / slope)
    }

    /// The largest scroll offset where the edge is at or after `at`.
    fn last(&self, start: f64, at: f64) -> Option<f64> {
        let breaks = self.breaks();
        let y = |s: f64| self.edge(start, s);
        let (first, last) = (*breaks.first()?, *breaks.last()?);
        if y(last) >= at {
            let slope = y(last + 1.0) - y(last);
            return Some(if slope < 0.0 { last + (at - y(last)) / slope } else { last });
        }
        for pair in breaks.windows(2).rev() {
            let (y0, y1) = (y(pair[0]), y(pair[1]));
            if y0 >= at {
                return Some(pair[0] + (at - y0) * (pair[1] - pair[0]) / (y1 - y0));
            }
        }
        let slope = y(first) - y(first - 1.0);
        (slope < 0.0).then(|| first - (at - y(first)))
    }
}

/// The travel of the nearest `position: sticky` box holding `subject` (it or an ancestor, inside its
/// scroll container) along the axis, when it has an inset on that axis; `None` otherwise. `port_start`
/// is the container's content origin in layout coordinates.
fn sticky_travel(doc: &BaseDocument, subject: NodeId, resolved: &Resolved, port: f64, port_start: f64) -> Option<StickyTravel> {
    use style::values::generics::position::GenericInset;
    let mut current = Some(subject);
    let sticky = loop {
        let id = current?;
        if Some(id) == resolved.source {
            return None;
        }
        let node = doc.get_node(id)?;
        if crate::stylo::is_sticky(node) {
            break node;
        }
        current = node.parent;
    };
    let style = sticky.primary_styles()?;
    let inset = |v: style::values::computed::Inset| match v {
        GenericInset::LengthPercentage(lp) => Some(lp.resolve(style::values::computed::Length::new(port as f32)).px() as f64),
        _ => None,
    };
    let (first, second) = if resolved.vertical {
        (inset(style.slow_clone_top()), inset(style.slow_clone_bottom()))
    } else {
        (inset(style.slow_clone_left()), inset(style.slow_clone_right()))
    };
    if first.is_none() && second.is_none() {
        return None;
    }
    let layout = sticky.final_layout();
    let axis = |size: taffy::Size<f32>| if resolved.vertical { size.height } else { size.width } as f64;
    let along = |node: &Node| {
        let (p, scroll) = (node.absolute_position(0.0, 0.0), node.scroll_offset());
        if resolved.vertical { p.y as f64 + scroll.y } else { p.x as f64 + scroll.x }
    };
    let origin = along(sticky) - port_start;
    let margin = if resolved.vertical { (layout.margin.top, layout.margin.bottom) } else { (layout.margin.left, layout.margin.right) };
    // The containing block: the nearest ancestor that is no inline box.
    let mut block = sticky.parent;
    while let Some(id) = block {
        let node = doc.get_node(id)?;
        if node.primary_styles().is_some_and(|s| {
            let display = s.clone_display();
            !(display.is_inline_flow() || display.is_contents())
        }) {
            break;
        }
        block = node.parent;
    }
    let block_node = doc.get_node(block?)?;
    let range = if Some(block_node.id) == resolved.source {
        // A direct child of the scroll container: its scrollable content bounds it.
        let size = if resolved.vertical { block_node.scroll_height() } else { block_node.scroll_width() } as f64;
        (0.0, size)
    } else {
        let layout = block_node.final_layout();
        let (before, after) = if resolved.vertical {
            (layout.border.top + layout.padding.top, layout.border.bottom + layout.padding.bottom)
        } else {
            (layout.border.left + layout.padding.left, layout.border.right + layout.padding.right)
        };
        let begin = along(block_node) - port_start;
        (begin + before as f64, begin + axis(layout.size) - after as f64)
    };
    Some(StickyTravel { top: origin, size: axis(layout.size), margin: (margin.0 as f64, margin.1 as f64), inset: (first, second), block: range, port })
}

fn inset_px(value: &LengthPercentageOrAuto, port: f64, auto: f64) -> f64 {
    match value {
        LengthPercentageOrAuto::Auto => auto,
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
/// pyro: an ancestor whose `timeline-scope` names it (or is `all`) makes the timeline of the one
/// element in its subtree declaring it visible (§4.5; pseudo-elements count). With none, or more
/// than one, the name is inactive.
fn named(doc: &BaseDocument, node: &Node, name: &str) -> Option<Resolved> {
    let mut current = Some(node.id);
    while let Some(id) = current {
        let el = doc.get_node(id)?;
        if let Some(found) = declared(doc, el, name) {
            return found;
        }
        if let Some(style) = el.primary_styles() {
            let scope = &style.get_ui().timeline_scope.value;
            let css = scope.to_css_string();
            let names = css.split(',').map(str::trim).collect::<Vec<_>>();
            if !scope.is_none() && names.iter().any(|&scoped| scoped == name || scoped == "all") {
                let mut found = Vec::new();
                let mut stack = vec![id];
                while let Some(id) = stack.pop() {
                    let Some(n) = doc.get_node(id) else { continue };
                    if found.len() > 1 {
                        break;
                    }
                    if n.id != el.id {
                        if let Some(resolved) = declared(doc, n, name) {
                            found.push(resolved);
                        }
                    }
                    stack.extend(n.before());
                    stack.extend(n.after());
                    stack.extend(n.children.iter().rev().copied().filter(|&c| doc.get_node(c).is_some_and(Node::is_element)));
                }
                return match found.len() {
                    1 => found.pop().flatten(),
                    _ => None,
                };
            }
        }
        current = el.parent.filter(|&p| doc.get_node(p).is_some_and(Node::is_element));
    }
    None
}

/// The timeline `name` that `el` declares itself (`Some(None)`: declared, but inactive; `None`:
/// not declared).
fn declared(doc: &BaseDocument, el: &Node, name: &str) -> Option<Option<Resolved>> {
    let style = el.primary_styles()?;
    let ui = style.get_ui();
    let axes = ui.scroll_timeline_axis_count().max(1);
    for i in (0..ui.scroll_timeline_name_count()).rev() {
        if ui.scroll_timeline_name_at(i).to_css_string() == name {
            let axis = ui.scroll_timeline_axis_at(i % axes);
            // The element itself is the scroll container (none: inactive).
            if !is_scroll_container(el) {
                return Some(None);
            }
            return Some(Some(Resolved { kind: Kind::Scroll, source: Some(el.id), vertical: vertical(el, axis), inset: None }));
        }
    }
    let axes = ui.view_timeline_axis_count().max(1);
    let insets = ui.view_timeline_inset_count().max(1);
    for i in (0..ui.view_timeline_name_count()).rev() {
        if ui.view_timeline_name_at(i).to_css_string() == name {
            return Some(Some(Resolved {
                kind: Kind::View { subject: el.id },
                source: nearest_scroll_container(doc, el),
                vertical: vertical(el, ui.view_timeline_axis_at(i % axes)),
                inset: Some(ui.view_timeline_inset_at(i % insets)),
            }));
        }
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
    // pyro: an absolutely positioned box scrolls with its containing block's scroll containers,
    // not with the scroll containers it merely sits inside in the tree (Scroll-driven Animations
    // 1: `scroll(nearest)` is the nearest scroll container ancestor of the box's containing block
    // chain; WPT scroll-timeline-nearest-with-absolute-positioned-element). Out-of-flow boxes
    // know their containing block from layout (`Node::containing_block`).
    let out_of_flow = node.primary_styles().is_some_and(|s| {
        use style::computed_values::position::T as Position;
        matches!(s.slow_clone_position(), Position::Absolute | Position::Fixed)
    });
    let mut current = if out_of_flow { node.containing_block() } else { node.parent };
    while let Some(id) = current {
        let n = doc.get_node(id)?;
        if id == root {
            return None;
        }
        if !n.is_element() {
            // (Anonymous boxes of the containing block chain are passed over.)
            if out_of_flow {
                current = n.containing_block().or(n.parent);
                continue;
            }
            return None;
        }
        // (The body's overflow propagates to the viewport.)
        if is_scroll_container(n) && !is_viewport_propagated_body(doc, n) {
            return Some(id);
        }
        current = if out_of_flow { n.containing_block().or(n.parent) } else { n.parent };
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
    // pyro: the root element (the scrolling element) scrolls as the viewport does.
    let source = source.filter(|&id| doc.try_root_element().is_none_or(|root| root.id != id));
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
            // pyro: the scroll range `scrollWidth`/`scrollHeight` give (the scrollable overflow
            // resolved after layout: in-flow children's end margins too), as a scroll timeline's
            // (WPT scroll-animations `css/scroll-timeline-update-reversed-animation`).
            let size = size.max(resolved_scroll_size(doc, n, vertical));
            Some(Scroll { offset, max: (size - client).max(0.0), port: client })
        }
        None => {
            let root = doc.root_element();
            let viewport = doc.viewport();
            let scale = viewport.zoom() as f64;
            let (w, h) = viewport.window_size;
            // (Less the viewport's classic scrollbar gutters: its scrollport.)
            let g = doc.viewport_scrollbars.gutters;
            let port = if vertical {
                (h as f64 / scale - (g.top + g.bottom) as f64).max(0.0)
            } else {
                (w as f64 / scale - (g.left + g.right) as f64).max(0.0)
            };
            let page = match &doc.timeline_scroll {
                Some((_, viewport)) => *viewport,
                None => (doc.viewport_scroll().x, doc.viewport_scroll().y),
            };
            let offset = if vertical { page.1 } else { page.0 };
            // pyro: a root whose horizontal scroll origin is at its right edge (`vertical-rl`, or
            // horizontal with `direction: rtl`) scrolls to negative offsets: progress counts from
            // that origin (WPT scroll-animations `css/scroll-timeline-default-writing-mode-rl`).
            // (By the principal writing mode, as the viewport scrolls: `viewport_scrolls_leftward`.)
            let reversed = !vertical && doc.viewport_scrolls_leftward();
            let offset = if reversed { -offset } else { offset };
            // (The document's scroll size, leftward overflow counted where it scrolls from the
            // right: as `scrollWidth`.)
            let (dw, dh) = doc.document_scroll_size((w as f64 / scale, h as f64 / scale));
            let size = if vertical { dh } else { dw }.max(if vertical { root.scroll_height() } else { root.scroll_width() } as f64);
            Some(Scroll { offset, max: (size - port).max(0.0), port })
        }
    }
}

/// pyro: the scroll size along an axis from the box's resolved scrollable overflow (device px in
/// its border box), from its padding box's start edge (as the layout queries' `scrollHeight`).
fn resolved_scroll_size(doc: &BaseDocument, n: &Node, vertical: bool) -> f64 {
    let scale = doc.viewport().scale_f64();
    let o = n.scrollable_overflow().scale_from_origin(1.0 / scale);
    let l = n.final_layout();
    let reversed = l.scrollable_overflow_rect.is_reversed();
    if vertical {
        if reversed.y { (l.size.height - l.border.bottom) as f64 - o.y0 } else { o.y1 - l.border.top as f64 }
    } else if reversed.x {
        (l.size.width - l.border.right) as f64 - o.x0
    } else {
        o.x1 - l.border.left as f64
    }
}
