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

impl LayoutPassState<'_> {
    /// The line count `node_id` clamps its lines to (the legacy line clamp), if it does.
    pub(crate) fn line_clamp_of(&self, node_id: NodeId) -> Option<u32> {
        use style::computed_value_flags::ComputedValueFlags;
        let style = self.nodes[node_id].primary_styles()?;
        if !style.flags.contains(ComputedValueFlags::WEBKIT_LINE_CLAMP_APPLIES) {
            return None;
        }
        let lines = style.get_box().line_clamp.max_lines.lines.into_rust()?;
        u32::try_from(lines.0).ok().filter(|&lines| lines > 0)
    }

    /// Clamps the lines of `root` (laid out to `output` from `inputs`) to `max_lines`: marks the
    /// line it ends at and ends an auto-height box there.
    pub(crate) fn clamp_lines(&mut self, root: NodeId, max_lines: u32, inputs: LayoutInput, output: &mut LayoutOutput) {
        let found = self.find_clamp_line(root, max_lines);
        self.nodes[root].flags.set(NodeFlags::CLAMPS_LINES, found.is_some());
        let Some(bottom) = found else { return };
        let style = self.nodes[root].layout_style();
        use taffy::CoreStyle as _;
        if inputs.known_dimensions.height.is_some() || !style.size().height.is_auto() {
            return;
        }
        let padding = style.padding().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let border = style.border().resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let min = style.min_size().height.maybe_resolve(inputs.parent_size.height, resolve_calc_value);
        let max = style.max_size().height.maybe_resolve(inputs.parent_size.height, resolve_calc_value);
        let inset = padding.top + border.top + padding.bottom + border.bottom;
        let box_sizing_inset = if style.box_sizing() == taffy::BoxSizing::ContentBox { inset } else { 0.0 };
        let (min, max) = (min.map(|v| v + box_sizing_inset), max.map(|v| v + box_sizing_inset));
        let height = (bottom + padding.bottom + border.bottom).maybe_clamp(min, max).max(inset);
        drop(style);
        output.size.height = height;
    }

    /// Forgets the clamp a box that no longer clamps its lines left on them.
    pub(crate) fn unclamp_lines(&mut self, root: NodeId) {
        self.find_clamp_line(root, u32::MAX);
        self.nodes[root].flags.remove(NodeFlags::CLAMPS_LINES);
    }

    /// Walks the line boxes of `root`'s block formatting context in tree order and marks the
    /// one `max_lines` names as the clamp line when more lines follow it (every other block
    /// loses its mark). Returns that line's bottom from the top of `root`'s border box.
    fn find_clamp_line(&mut self, root: NodeId, max_lines: u32) -> Option<f32> {
        let scale = self.doc.viewport.scale();
        let mut count = 0u32;
        // The block holding the clamp line, how many of its lines stay, and the line's bottom.
        let mut clamp_line: Option<(NodeId, u32, f32)> = None;
        let mut clamped = false;
        // Blocks to visit (last first) with their top from the root's border box.
        let mut stack: Vec<(NodeId, f32)> = vec![(root, 0.0)];
        while let Some((id, top)) = stack.pop() {
            if self.nodes[id].flags.is_inline_root() {
                let node = &mut self.nodes[id];
                let Some(text_layout) = node.data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut())
                else {
                    continue;
                };
                text_layout.clamp = None;
                let lines = text_layout.counted_lines;
                if clamp_line.is_none() && count + lines >= max_lines && lines > 0 {
                    let kept = max_lines - count;
                    let bottom = text_layout
                        .layout
                        .get(kept as usize - 1)
                        .map_or(0.0, |line| line.metrics().block_max_coord / scale);
                    clamp_line = Some((id, kept, top + text_layout.line_box_top + bottom));
                    clamped = lines > kept;
                } else if clamp_line.is_some() && lines > 0 {
                    clamped = true;
                }
                count = count.saturating_add(lines);
                continue;
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
                        && child.primary_styles().is_some_and(|style| {
                            use style::values::computed::Overflow;
                            let box_style = style.get_box();
                            !box_style.float.is_floating()
                                && !crate::node::style_is_out_of_flow(&style)
                                && matches!(box_style.overflow_x, Overflow::Visible | Overflow::Clip)
                                && matches!(box_style.overflow_y, Overflow::Visible | Overflow::Clip)
                        });
                if in_context && (child.flags.is_inline_root() || child.layout_children.borrow().is_some()) {
                    stack.push((child_id, top + child.unrounded_layout().location.y));
                }
            }
            stack[first..].reverse();
        }
        let (id, kept, bottom) = clamp_line.filter(|_| clamped)?;
        self.set_clamp(id, kept);
        Some(bottom)
    }

    /// Marks the block `id` as ending at its line `kept` with an ellipsis, shaped in its style.
    fn set_clamp(&mut self, id: NodeId, kept: u32) {
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
        let ellipsis = {
            let mut font_ctx = doc.font_ctx.lock().unwrap();
            let mut builder = doc.layout_ctx.tree_builder(&mut font_ctx, scale, true, &parley_style);
            builder.set_base_direction(if rtl { parley::BaseDirection::Rtl } else { parley::BaseDirection::Ltr });
            builder.push_text("\u{2026}");
            let mut layout = builder.build().0;
            layout.break_all_lines(None);
            Box::new(layout)
        };
        drop(style);
        if let Some(text_layout) = doc.nodes[id].data.downcast_element_mut().and_then(|el| el.inline_layout_data.as_mut()) {
            text_layout.clamp = Some(LineClamp { lines: kept, ellipsis });
        }
    }
}
