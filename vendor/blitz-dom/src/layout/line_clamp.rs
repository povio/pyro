//! pyro: the legacy line clamp (`-webkit-line-clamp` on a `display: -webkit-box` box with
//! `-webkit-box-orient: vertical`; Stylo's `WEBKIT_LINE_CLAMP_APPLIES`), as Blink lays it out.
//!
//! The box is a block container (Stylo computes its display to `flow-root`). Its lines are the
//! line boxes of its block formatting context, in tree order: its own, and those of its in-flow
//! block descendants that establish no formatting context of their own. When there are more
//! than the clamp's count, the box with an auto height ends at the bottom of the line that
//! count names, that line ends with "…" (in the style of the block that holds it), and the
//! lines after it in the same block are not painted. Lines in later blocks are left to the
//! box's `overflow`, which the pages that clamp set.
//!
//! The clamp is found after the box's own layout ([`LayoutPassState::clamp_lines`]): the line
//! boxes and block positions it reads are the ones that layout made. A measuring run of the
//! box therefore runs as a full layout.

use taffy::{LayoutInput, LayoutOutput, ResolveOrZero as _, MaybeResolve as _, MaybeMath as _};

use super::{LayoutPassState, resolve_calc_value};
use crate::node::{LineClamp, NodeFlags};
use crate::NodeId;
use markup5ever::local_name;

/// pyro: how a box clamps its lines ([`LayoutPassState::line_clamp_of`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClampSpec {
    /// The most lines it keeps (`u32::MAX`: no count).
    pub(crate) lines: u32,
    /// `line-clamp: auto`: no more lines than fit in its block size.
    pub(crate) auto: bool,
}

impl LayoutPassState<'_> {
    /// How `node_id` clamps its lines (the legacy line clamp, or `line-clamp`), if it does.
    pub(crate) fn line_clamp_of(&self, node_id: NodeId) -> Option<ClampSpec> {
        use style::computed_value_flags::ComputedValueFlags;
        let style = self.nodes[node_id].primary_styles()?;
        let clamp = &style.get_box().line_clamp;
        // pyro: and `line-clamp` (CSS Overflow 4, not the legacy shorthand) on a block container,
        // laid out the same way (WPT css-inline `text-box-trim-line-clamp-*`).
        let standard = !clamp.webkit_legacy && {
            use style::values::specified::box_::{DisplayInside, DisplayOutside};
            let display = style.get_box().display;
            // (An inline-block is a block container too: WPT css-overflow `line-clamp-015`.)
            (display.outside() == DisplayOutside::Block
                && matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
                || display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::FlowRoot)
                // (Not a multi-column container, nor a fieldset: WPT css-overflow `line-clamp-039`,
                // `line-clamp-form-fieldset-001`.)
                && !super::multicol::is_multicol_style(&style)
                && self.nodes[node_id]
                    .data
                    .downcast_element()
                    .is_none_or(|el| el.name.local != local_name!("fieldset"))
        };
        if !standard && !style.flags.contains(ComputedValueFlags::WEBKIT_LINE_CLAMP_APPLIES) {
            return None;
        }
        // pyro: `line-clamp: auto` (and `<integer> auto`) clamps where the lines would overflow
        // the box's block size (CSS Overflow 4 § 5.2 `max-lines`; WPT css-overflow
        // `line-clamp-auto-*`).
        let auto = standard && matches!(clamp.max_lines.kw, style::values::generics::box_::MaxLinesKeyword::Auto);
        let lines = match clamp.max_lines.lines.clone().into_rust() {
            Some(lines) => u32::try_from(lines.0).ok().filter(|&lines| lines > 0)?,
            None if auto => u32::MAX,
            None => return None,
        };
        Some(ClampSpec { lines, auto })
    }

    /// Clamps the lines of `root` (laid out to `output` from `inputs`) to `max_lines`: marks the
    /// line it ends at and ends an auto-height box there.
    pub(crate) fn clamp_lines(&mut self, root: NodeId, spec: ClampSpec, inputs: LayoutInput, output: &mut LayoutOutput) {
        let style = self.nodes[root].layout_style();
        use taffy::CoreStyle as _;
        let padding = style.padding().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let border = style.border().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let min = style.min_size().height.maybe_resolve(inputs.parent_size.height, resolve_calc_value);
        let max = style.max_size().height.maybe_resolve(inputs.parent_size.height, resolve_calc_value);
        let height = style.size().height.maybe_resolve(inputs.parent_size.height, resolve_calc_value);
        let inset = padding.top + border.top + padding.bottom + border.bottom;
        let box_sizing_inset = if style.box_sizing() == taffy::BoxSizing::ContentBox { inset } else { 0.0 };
        let (min, max) = (min.map(|v| v + box_sizing_inset), max.map(|v| v + box_sizing_inset));
        let auto_height = inputs.known_dimensions.height.is_none() && style.size().height.is_auto();
        drop(style);
        // `auto`: the lines end where the content box does, at the box's block size (its height,
        // else its maximum height), from the top of its border box.
        let limit = spec
            .auto
            // (A `min-height` above the `max-height` wins: WPT css-overflow `line-clamp-auto-014`.)
            // (The used height: the authored one within `min-height` and `max-height`.)
            .then(|| {
                inputs
                    .known_dimensions
                    .height
                    .or(height.map(|h| (h + box_sizing_inset).maybe_clamp(min, max)))
                    .or(max.map(|max| min.map_or(max, |min| min.max(max))))
            })
            .flatten()
            .map(|block_size| block_size - padding.bottom - border.bottom);
        let content_top = padding.top + border.top;
        let found = self.find_clamp_line(root, spec.lines, limit, content_top);
        self.nodes[root].flags.set(NodeFlags::CLAMPS_LINES, found.is_some());
        let Some((bottom, block)) = found else { return };
        // pyro: the blocks the clamp line is in end after it (CSS Overflow 4 § 5.4: the content
        // after the clamp point is discarded), their own bottom padding and border below it, and
        // what follows them is not painted (WPT css-overflow `line-clamp-012`,
        // `line-clamp-auto-020`).
        // pyro: the last baseline is the clamp line's, the last line kept (an inline-block sits on
        // it: WPT css-overflow `line-clamp-015`).
        let clamp_baseline = {
            let scale = self.doc.viewport.scale();
            let mut top = 0.0f32;
            let mut at = block;
            while at != root {
                top += self.nodes[at].unrounded_layout().location.y;
                match self.nodes[at].layout_parent.get() {
                    Some(parent) => at = parent,
                    None => break,
                }
            }
            self.nodes[block].data.downcast_element().and_then(|el| el.inline_layout_data.as_deref()).and_then(|text| {
                let kept = text.clamp.as_ref()?.lines as usize;
                let line = text.layout.get(kept.checked_sub(1)?)?;
                Some(top + text.line_box_top + line.metrics().baseline / scale)
            })
        };
        if let Some(baseline) = clamp_baseline {
            output.baselines.last = Some(baseline);
        }
        let bottom = self.end_blocks_at_clamp(root, block, bottom);
        // pyro: what was discarded adds no scrollable overflow: the content ends at the clamp
        // point (and its bottom padding).
        let overflow = output.scrollable_overflow_rect;
        if !overflow.is_reversed().y {
            // (From the top of the padding box.)
            let mut rect = overflow.rect();
            rect.bottom = rect.bottom.min((bottom - border.top + padding.bottom).max(0.0));
            output.scrollable_overflow_rect = taffy::ScrollableOverflow::new(rect, overflow.is_reversed());
        }
        if !auto_height {
            return;
        }
        let height = (bottom + padding.bottom + border.bottom).maybe_clamp(min, max).max(inset);
        output.size.height = height;
    }

    /// Forgets the clamp a box that no longer clamps its lines left on them.
    pub(crate) fn unclamp_lines(&mut self, root: NodeId) {
        self.find_clamp_line(root, u32::MAX, None, 0.0);
        self.nodes[root].flags.remove(NodeFlags::CLAMPS_LINES);
    }

    /// Walks the line boxes of `root`'s block formatting context in tree order and marks the
    /// one `max_lines` names as the clamp line when more lines follow it (every other block
    /// loses its mark). Returns that line's bottom from the top of `root`'s border box.
    /// pyro: with `limit` (`line-clamp: auto`), no line whose bottom is past it stays: the clamp
    /// line is then the last one above it (or none, the box keeping no line: its content ends at
    /// `content_top`).
    fn find_clamp_line(
        &mut self,
        root: NodeId,
        max_lines: u32,
        limit: Option<f32>,
        content_top: f32,
    ) -> Option<(f32, NodeId)> {
        let scale = self.doc.viewport.scale();
        // pyro: the clamp line is the box's last formatted line: `text-box-trim: trim-end` trims
        // its block-end side (WPT css-inline `text-box-trim-line-clamp-*`).
        let trims_end = self.nodes[root].primary_styles().is_some_and(|style| {
            style.slow_clone_text_box_trim().contains(style::values::computed::TextBoxTrim::TRIM_END)
        });
        // pyro: the standard `line-clamp` discards what follows the clamp line (`continue:
        // discard`): later blocks' lines are not painted either.
        let discards = self.nodes[root].primary_styles().is_some_and(|style| !style.get_box().line_clamp.webkit_legacy);
        // pyro: and its `block-ellipsis` (the legacy clamp's is always "…").
        let ellipsis_text: String = match self.nodes[root].primary_styles() {
            Some(style) if discards => {
                use style::values::generics::box_::BlockEllipsis;
                match &style.get_box().line_clamp.block_ellipsis {
                    BlockEllipsis::Ellipsis => "\u{2026}".to_owned(),
                    BlockEllipsis::NoEllipsis => String::new(),
                    BlockEllipsis::String(text) => text.to_string(),
                }
            }
            _ => "\u{2026}".to_owned(),
        };
        let mut discarded: Vec<NodeId> = Vec::new();
        let mut count = 0u32;
        // The clamp point: after `kept` lines of an inline root (`ClampPoint::Lines`), or after a
        // block-level box whose lines are not the root's (`ClampPoint::Box`), and its bottom.
        let mut clamp_line: Option<ClampPoint> = None;
        let mut clamped = false;
        // The last clamp point before the one at hand: after the last line of a block whose lines
        // all stay, or after a box of its own (WPT css-overflow `line-clamp-auto-034`, `-039`).
        let mut last_lines: Option<ClampPoint> = None;
        // Blocks to visit (last first) with their top from the root's border box. (`whole`: a
        // block-level box whose lines are not the root's, a clamp point after it.) What must fit
        // under the limit for a clamp point is where the root's content would end with it: the
        // blocks around the point closed after it (`clamp_point_end`: their bottom padding,
        // border and collapsing margins, their own `height`, `min-height`, `max-height`; WPT
        // css-overflow `line-clamp-auto-029`, `-035`, `-040`, `-042`..`-044`).
        let mut stack: Vec<(NodeId, f32, bool)> = vec![(root, 0.0, false)];
        while let Some((id, top, whole)) = stack.pop() {
            if whole {
                let layout = *self.nodes[id].unrounded_layout();
                let bottom = top + layout.size.height;
                let point = ClampPoint::Box(id, bottom);
                if clamp_line.is_none() {
                    // (A box whose margins collapse through it adds nothing past where it is: they
                    // collapse with the margin before it, counted already: WPT css-overflow
                    // `line-clamp-auto-032`.)
                    let self_collapsing = layout.size.height == 0.0
                        && layout.padding.top + layout.padding.bottom + layout.border.top + layout.border.bottom == 0.0;
                    let margin = if self_collapsing { 0.0 } else { layout.margin.bottom };
                    let end = match self.nodes[id].layout_parent.get() {
                        Some(parent) if parent != root => self.clamp_point_end(root, parent, bottom, margin),
                        _ => bottom + margin,
                    };
                    if limit.is_some_and(|limit| end > limit + 0.01) {
                        clamp_line = Some(last_lines.unwrap_or(ClampPoint::Lines(id, 0, content_top)));
                        clamped = true;
                    } else {
                        last_lines = Some(point);
                    }
                } else if layout.size.height > 0.0 {
                    clamped = true;
                }
                continue;
            }
            if self.nodes[id].flags.is_inline_root() {
                // (What an earlier clamp hid in it shows again unless this one hides it.)
                let all: Vec<NodeId> = self.nodes[id].layout_children.borrow().iter().flatten().copied().collect();
                for child_id in all {
                    self.nodes[child_id].flags.remove(NodeFlags::HIDDEN_BY_CLAMP);
                }
                let node = &mut self.nodes[id];
                let Some(text_layout) = node.data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut())
                else {
                    continue;
                };
                text_layout.clamp = None;
                let lines = text_layout.counted_lines;
                let root_style = trims_end.then(|| node.primary_styles()).flatten();
                let text_layout = node.data.downcast_element().and_then(|el| el.inline_layout_data.as_ref()).unwrap();
                // The bottom of line `index` from the top of the root's border box.
                let line_bottom = |index: u32| {
                    top + text_layout.line_box_top
                        + text_layout.layout.get(index as usize).map_or(0.0, |line| {
                            let trim = root_style
                                .as_deref()
                                .map_or(0.0, |style| super::text_box::fragment_line_trim(&line, style, false));
                            (line.metrics().block_max_coord - trim) / scale
                        })
                };
                let bottoms: Vec<f32> = (0..lines).map(|index| line_bottom(index)).collect();
                drop(line_bottom);
                drop(root_style);
                if clamp_line.is_none() && lines > 0 {
                    // The lines of this block that stay: no more than the count leaves, and none
                    // that ends past the limit.
                    let mut kept = (max_lines - count).min(lines);
                    if let Some(limit) = limit {
                        kept = (0..kept)
                            .find(|&index| {
                                let bottom = bottoms[index as usize];
                                let end = if id == root { bottom } else { self.clamp_point_end(root, id, bottom, 0.0) };
                                end > limit + 0.01
                            })
                            .unwrap_or(kept);
                    }
                    if kept == 0 {
                        // The clamp point is before this block's first line: after the last line
                        // of an earlier block, or before any line.
                        clamp_line = Some(last_lines.unwrap_or(ClampPoint::Lines(id, 0, content_top)));
                        clamped = true;
                        if discards || last_lines.is_some() {
                            discarded.push(id);
                        }
                    } else if kept < lines || count + lines >= max_lines {
                        clamp_line = Some(ClampPoint::Lines(id, kept, bottoms[kept as usize - 1]));
                        clamped = lines > kept;
                    } else {
                        last_lines = Some(ClampPoint::Lines(id, lines, bottoms[lines as usize - 1]));
                    }
                } else if clamp_line.is_some() && lines > 0 {
                    clamped = true;
                    if discards {
                        discarded.push(id);
                    }
                }
                count = count.saturating_add(lines);
                continue;
            }
            // (What an earlier clamp hid shows again unless this one hides it.)
            let all: Vec<NodeId> = self.nodes[id].layout_children.borrow().iter().flatten().copied().collect();
            for child_id in all {
                self.nodes[child_id].flags.remove(NodeFlags::HIDDEN_BY_CLAMP);
            }
            let children = self.nodes[id].layout_children.borrow();
            let Some(children) = children.as_ref() else { continue };
            let first = stack.len();
            for &child_id in children.iter() {
                let child = &self.nodes[child_id];
                // An anonymous block (of the box's own lines), or a block in flow and in this
                // formatting context.
                let anonymous = matches!(child.data, crate::node::NodeData::AnonymousBlock(_));
                let in_context = anonymous
                    || child.is_element()
                        && child.taffy_display() == taffy::Display::Block
                        // (A fieldset is a formatting context of its own.)
                        && child.data.downcast_element().is_some_and(|el| el.name.local != local_name!("fieldset"))
                        // (A box that clamps its own lines keeps its clamp: WPT-less, review of
                        // round 6. A standard line clamp is an independent formatting context.)
                        && self.line_clamp_of(child_id).is_none()
                        && child.primary_styles().is_some_and(|style| {
                            use style::values::computed::Overflow;
                            let box_style = style.get_box();
                            // (A flow root's lines are its own: WPT css-overflow `line-clamp-auto-034`.)
                            box_style.display.inside() != style::values::specified::box_::DisplayInside::FlowRoot
                                && !box_style.float.is_floating()
                                && !crate::node::style_is_out_of_flow(&style)
                                && matches!(box_style.overflow_x, Overflow::Visible | Overflow::Clip)
                                && matches!(box_style.overflow_y, Overflow::Visible | Overflow::Clip)
                        });
                // (An empty block is a box of its own, a clamp point after it: WPT css-overflow
                // `line-clamp-auto-039`.)
                // (Out-of-flow children leave a block empty: WPT css-overflow
                // `line-clamp-with-abspos-023`.)
                let descends = in_context
                    && (child.flags.is_inline_root()
                        || child.layout_children.borrow().as_ref().is_some_and(|c| c.iter().any(|&c| !self.nodes[c].is_out_of_flow())));
                // Any other box of the normal flow here is block-level and a clamp point after it.
                let whole = !descends
                    && !anonymous
                    && child.is_element()
                    && child.primary_styles().is_some_and(|style| {
                        !style.get_box().float.is_floating()
                            && !crate::node::style_is_out_of_flow(&style)
                            && style.get_box().display != style::values::specified::box_::Display::None
                    });
                if descends || whole {
                    let layout = child.unrounded_layout();
                    stack.push((child_id, top + layout.location.y, whole));
                }
            }
            stack[first..].reverse();
        }
        let (id, kept, bottom) = match clamp_line.filter(|_| clamped)? {
            ClampPoint::Lines(id, kept, bottom) => (id, Some(kept), bottom),
            // (No line ends at it: no ellipsis.)
            ClampPoint::Box(id, bottom) => (id, None, bottom),
        };
        let Some(kept) = kept else {
            for &other in &discarded {
                self.hide_children_below(other, f32::NEG_INFINITY);
            }
            for id in discarded {
                if let Some(text_layout) =
                    self.nodes[id].data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut())
                {
                    text_layout.clamp = Some(LineClamp { lines: 0, ellipsis: Box::default(), hyphenated_ellipsis: Box::default(), at_soft_wraps: discards });
                }
            }
            return Some((bottom, id));
        };
        // (The legacy `-webkit-line-clamp` too puts its ellipsis at a soft wrap opportunity, as
        // Chromium does: WPT css-overflow `line-clamp/webkit-line-clamp-035`.)
        self.set_clamp(id, kept, true, &ellipsis_text);
        // pyro: the floats and atomic inlines of the clamp line's block below its clamp line,
        // and those of the blocks discarded after it, are hidden with the lines (WPT css-overflow
        // `line-clamp-with-floats-003`, `-005`).
        let block_top: f32 = {
            let mut top = 0.0;
            let mut at = id;
            while at != root {
                top += self.nodes[at].unrounded_layout().location.y;
                match self.nodes[at].layout_parent.get() {
                    Some(parent) => at = parent,
                    None => break,
                }
            }
            top
        };
        self.hide_children_below(id, bottom - block_top);
        for &other in &discarded {
            self.hide_children_below(other, f32::NEG_INFINITY);
        }
        for id in discarded {
            if let Some(text_layout) =
                self.nodes[id].data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut())
            {
                text_layout.clamp = Some(LineClamp { lines: 0, ellipsis: Box::default(), hyphenated_ellipsis: Box::default(), at_soft_wraps: discards });
            }
        }
        Some((bottom, id))
    }

    /// pyro: ends the blocks from `block` (the one holding the clamp line, whose bottom is
    /// `bottom` from the top of `root`'s border box) up to `root` at the clamp point: an auto
    /// height ends below it by the block's bottom padding and border, and the boxes in flow after
    /// each are hidden (`NodeFlags::HIDDEN_BY_CLAMP`). Returns where the content of `root` ends.
    fn end_blocks_at_clamp(&mut self, root: NodeId, block: NodeId, bottom: f32) -> f32 {
        let mut chain = Vec::new();
        let mut id = block;
        while id != root {
            chain.push(id);
            match self.nodes[id].layout_parent.get() {
                Some(parent) => id = parent,
                None => return bottom,
            }
        }
        let top: f32 = chain.iter().map(|&id| self.nodes[id].unrounded_layout().location.y).sum();
        // The clamp point in the coordinates of the block at hand, and the margin below it that
        // collapses through the bottom of the blocks around it (CSS 2.1 § 8.3.1; WPT css-overflow
        // `line-clamp-auto-029`).
        let mut inner = bottom - top;
        let mut margin = 0.0f32;
        for (index, &id) in chain.iter().enumerate() {
            let layout = *self.nodes[id].unrounded_layout();
            let end;
            (end, margin) = self.close_block(id, inner, margin);
            if end < layout.size.height {
                self.nodes[id].update_unrounded_layout(|unrounded| unrounded.size.height = end);
            }
            let parent = chain.get(index + 1).copied().unwrap_or(root);
            self.hide_boxes_after(parent, id);
            inner = layout.location.y + end;
        }
        inner + margin
    }

    /// pyro: block `id` closed after content ending at `inner` (its border box's coordinates)
    /// with `margin` below it collapsing through its bottom: its block size then (no more than
    /// laid out, at least its `min-height`; its `height` when not `auto`) and the margin below
    /// its bottom edge.
    fn close_block(&self, id: NodeId, inner: f32, margin: f32) -> (f32, f32) {
        let node = &self.nodes[id];
        let layout = node.unrounded_layout();
        let min = {
            use taffy::CoreStyle as _;
            let style = node.layout_style();
            let inset = layout.padding.top + layout.padding.bottom + layout.border.top + layout.border.bottom;
            let inset = if style.box_sizing() == taffy::BoxSizing::ContentBox { inset } else { 0.0 };
            style.min_size().height.maybe_resolve(None, resolve_calc_value).map_or(0.0, |min| min + inset)
        };
        let (end, margin) = if block_end_collapses_through(node) {
            (inner.max(layout.padding.top + layout.border.top), margin)
        } else if height_is_auto(node) {
            let top_inset = layout.padding.top + layout.border.top;
            ((inner + margin).max(top_inset) + layout.padding.bottom + layout.border.bottom, 0.0)
        } else {
            return (layout.size.height, collapse_margins(0.0, layout.margin.bottom));
        };
        // (A box that keeps a `min-height` past the point does not let a margin through.)
        let floored = end.max(min);
        let margin = if floored > end { 0.0 } else { margin };
        (floored.min(layout.size.height), collapse_margins(margin, layout.margin.bottom))
    }

    /// pyro: where the content of the clamp root `root` ends (from its border box's top) for a
    /// clamp point at `bottom` (from the same top) in its descendant block `block`, `margin`
    /// below it: the blocks from `block` up closed after it ([`Self::close_block`]).
    fn clamp_point_end(&self, root: NodeId, block: NodeId, bottom: f32, margin: f32) -> f32 {
        let mut chain = Vec::new();
        let mut id = block;
        while id != root {
            chain.push(id);
            match self.nodes[id].layout_parent.get() {
                Some(parent) => id = parent,
                None => return bottom + margin,
            }
        }
        let top: f32 = chain.iter().map(|&id| self.nodes[id].unrounded_layout().location.y).sum();
        let (mut inner, mut margin) = (bottom - top, margin);
        for &id in &chain {
            let location = self.nodes[id].unrounded_layout().location.y;
            let end;
            (end, margin) = self.close_block(id, inner, margin);
            inner = location + end;
        }
        inner + margin
    }

    /// pyro: hides the children of the inline root `block` laid out in it (floats, atomic
    /// inlines; not absolutely positioned boxes) whose top is at `y` or below (its coordinates).
    fn hide_children_below(&mut self, block: NodeId, y: f32) {
        let children: Vec<NodeId> = self.nodes[block].layout_children.borrow().iter().flatten().copied().collect();
        let scale = self.doc.viewport.scale();
        for id in children {
            let out_of_flow = self.nodes[id].primary_styles().is_some_and(|style| crate::node::style_is_out_of_flow(&style));
            if !out_of_flow {
                if self.nodes[id].unrounded_layout().location.y >= y - 0.01 {
                    self.nodes[id].flags.insert(NodeFlags::HIDDEN_BY_CLAMP);
                }
                continue;
            }
            // pyro: an absolutely positioned box whose containing block is a positioned inline of
            // this block that is after the clamp point (its placeholder on a discarded line) is
            // discarded with it; one contained by this block or above stays (CSS Overflow 4;
            // WPT css-overflow `line-clamp-with-abspos-015`, `-024`).
            let mut inline_cb = None;
            let mut at = self.nodes[id].parent;
            while let Some(parent) = at {
                if parent == block || self.nodes[block].parent == Some(parent) {
                    break;
                }
                if self.nodes[parent].primary_styles().is_some_and(|style| {
                    style.get_box().position != style::computed_values::position::T::Static
                }) {
                    inline_cb = Some(parent);
                    break;
                }
                at = self.nodes[parent].parent;
            }
            let Some(cb) = inline_cb else { continue };
            // (Where the containing inline starts: its first text, else the box's placeholder. One
            // that starts before the clamp point contains it: WPT css-overflow
            // `line-clamp-with-abspos-016`.)
            let is_inside = |mut node: NodeId| loop {
                if node == cb {
                    break true;
                }
                match self.nodes[node].parent {
                    Some(parent) if parent != block => node = parent,
                    _ => break false,
                }
            };
            let after = y == f32::NEG_INFINITY
                || self.nodes[block].data.downcast_element().and_then(|el| el.inline_layout_data.as_ref()).is_some_and(|text| {
                    let start = text.text_nodes.iter().filter(|(node, _)| is_inside(*node)).map(|(_, range)| range.start).min();
                    text.layout.lines().any(|line| {
                        let top = text.line_box_top + line.metrics().block_min_coord / scale;
                        let starts_here = match start {
                            Some(start) => line.text_range().contains(&start),
                            None => line.items().any(|item| {
                                matches!(item, parley::PositionedLayoutItem::InlineBox(ibox) if ibox.id == id.as_u64())
                            }),
                        };
                        top >= y - 0.01 && starts_here
                    })
                });
            if after {
                self.nodes[id].flags.insert(NodeFlags::HIDDEN_BY_CLAMP);
            }
        }
    }

    /// pyro: hides the boxes in flow (floats too) among `parent`'s children after `child`.
    fn hide_boxes_after(&mut self, parent: NodeId, child: NodeId) {
        let children: Vec<NodeId> = self.nodes[parent].layout_children.borrow().iter().flatten().copied().collect();
        let Some(at) = children.iter().position(|&id| id == child) else { return };
        for &id in &children[at + 1..] {
            let out_of_flow = self.nodes[id].primary_styles().is_some_and(|style| crate::node::style_is_out_of_flow(&style));
            if !out_of_flow {
                self.nodes[id].flags.insert(NodeFlags::HIDDEN_BY_CLAMP);
            }
        }
    }

    /// Marks the block `id` as ending at its line `kept` with an ellipsis, shaped in its style.
    fn set_clamp(&mut self, id: NodeId, kept: u32, at_soft_wraps: bool, text: &str) {
        let scale = self.doc.viewport.scale();
        let doc = &mut *self.doc;
        let node = &doc.nodes[id];
        let Some(style) =
            node.primary_styles().or_else(|| node.parent.and_then(|parent| doc.nodes[parent].primary_styles()))
        else {
            return;
        };
        let parley_style = crate::stylo_to_parley::style(id, &style);
        let rtl = node
            .data
            .downcast_element()
            .and_then(|el| el.inline_layout_data.as_ref())
            .is_some_and(|text_layout| text_layout.layout.is_rtl());
        // (`no-ellipsis`: none at all; an empty text would still shape a line.)
        let mut shape = |text: &str| -> Box<parley::layout::Layout<crate::node::TextBrush>> {
            if text.is_empty() {
                return Box::default();
            }
            let mut font_ctx = doc.font_ctx.lock().unwrap();
            let mut builder = doc.layout_ctx.tree_builder(&mut font_ctx, scale, true, &parley_style);
            builder.set_base_direction(if rtl { parley::BaseDirection::Rtl } else { parley::BaseDirection::Ltr });
            builder.push_text(text);
            let mut layout = builder.build().0;
            layout.break_all_lines(None);
            Box::new(layout)
        };
        let ellipsis = shape(text);
        let hyphenated_ellipsis = if text.is_empty() { Box::default() } else { shape(&format!("-{text}")) };
        // pyro: a right-aligned or centred clamp line whose content and ellipsis fit is aligned
        // with the ellipsis (Chromium; WPT css-overflow `line-clamp/webkit-line-clamp-051`, `-052`).
        let alignment = crate::stylo_to_parley::text_align(stylo_taffy::CopyStyleGetters::clone_text_align(&**style));
        drop(style);
        let ellipsis_width = ellipsis.full_width().max(0.0);
        if kept > 0
            && ellipsis_width > 0.0
            && !rtl
            && let Some(text_layout) = doc.nodes[id].data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut())
        {
            let index = kept as usize - 1;
            let shift = text_layout.layout.get(index).and_then(|line| {
                let atomic = line.items().any(|item| {
                    matches!(item, parley::PositionedLayoutItem::InlineBox(ibox) if ibox.kind == parley::InlineBoxKind::InFlow)
                });
                let offset = line.metrics().offset;
                let share = match alignment {
                    parley::Alignment::Right | parley::Alignment::End => 1.0,
                    parley::Alignment::Center => 0.5,
                    _ => return None,
                };
                (!atomic && offset >= ellipsis_width * share - 0.01).then(|| offset - ellipsis_width * share)
            });
            if let Some(offset) = shift {
                text_layout.layout.set_line_offset(index, offset);
            }
        }
        if let Some(text_layout) = doc.nodes[id].data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut()) {
            text_layout.clamp = Some(LineClamp { lines: kept, ellipsis, hyphenated_ellipsis, at_soft_wraps });
        }
    }
}

/// pyro: whether the bottom margin of a block's last child collapses with the block's own: an
/// auto height and no bottom padding or border (CSS 2.1 § 8.3.1; the blocks a line clamp walks
/// are in its formatting context).
fn block_end_collapses_through(node: &crate::node::Node) -> bool {
    let layout = node.unrounded_layout();
    height_is_auto(node) && layout.padding.bottom == 0.0 && layout.border.bottom == 0.0
}

fn height_is_auto(node: &crate::node::Node) -> bool {
    use taffy::CoreStyle as _;
    node.is_anonymous() || node.layout_style().size().height.is_auto()
}

/// Two adjoining margins collapsed: the largest positive one less the most negative one.
fn collapse_margins(a: f32, b: f32) -> f32 {
    a.max(b).max(0.0) + a.min(b).min(0.0)
}

/// pyro: where a line clamp's content ends: after `kept` lines of an inline root, or after a
/// block-level box; with its bottom from the top of the clamp root's border box.
#[derive(Clone, Copy)]
enum ClampPoint {
    Lines(NodeId, u32, f32),
    Box(NodeId, f32),
}
