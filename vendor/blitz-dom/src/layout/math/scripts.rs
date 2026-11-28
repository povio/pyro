//! pyro: the layout class of `msub`, `msup`, `msubsup` and `mmultiscripts` (MathML Core § 3.4.1
//! and § 3.4.3, Chromium's `MathMLScriptsLayoutAlgorithm`), also the layout of a `munder`,
//! `mover` or `munderover` whose base is an operator with `movablelimits` in compact style
//! (`underover`).
//!
//! The base on the baseline, a subscript after it (moved back by the italic correction of a large
//! operator base) shifted down, a superscript after it (moved on by the base's italic correction)
//! shifted up, each followed by `SpaceAfterScript`. The shifts are the font's MATH constants
//! (`SubscriptShiftDown`, `SubscriptTopMax`, `SubscriptBaselineDropMin`, `SuperscriptShiftUp`,
//! or `SuperscriptShiftUpCramped` in a cramped (`math-shift: compact`) element,
//! `SuperscriptBottomMin`, `SuperscriptBaselineDropMax`) or their fallbacks, and with both scripts
//! the gap between them is kept at `SubSuperscriptGapMin`, first by raising the superscript up to
//! `SuperscriptBottomMaxWithSubscript` and then by lowering the subscript. `mmultiscripts` is a
//! row of subscript/superscript pairs after the base, and, after an `mprescripts`, pairs before
//! it (each pair right-aligned), all pairs sharing the largest shifts; a `none` is an empty
//! script. An `mmultiscripts` with an invalid structure (an odd number of scripts, a second
//! `mprescripts`) is laid out as a row.
//!
//! The ink extents of the base and the scripts are their line metrics (as in `underover`).
use markup5ever::ns;
use thin_vec::ThinVec;

use super::types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout};
use super::underover::{Fallbacks, child_italic_correction, embellished_operator, math_constant};
use super::{MathConstant, row};
use crate::layout::LayoutPassState;
use crate::{BaseDocument, NodeId};

/// Construction: the children are the layout children (`mmultiscripts`' `none` and `mprescripts`
/// are among them).
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, _kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    super::build_children(doc, node_id, children)
}

/// The constants of the script shifts (CSS px).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Parameters {
    subscript_shift_down: f32,
    subscript_top_max: f32,
    subscript_baseline_drop_min: f32,
    superscript_shift_up: f32,
    superscript_bottom_min: f32,
    superscript_baseline_drop_max: f32,
    sub_superscript_gap_min: f32,
    superscript_bottom_max_with_subscript: f32,
    space_after_script: f32,
}

impl Parameters {
    fn new(ctx: &MathContext, fallbacks: &Fallbacks, cramped: bool) -> Parameters {
        let constant = |c: MathConstant| math_constant(ctx, fallbacks, c);
        Parameters {
            subscript_shift_down: constant(MathConstant::SubscriptShiftDown),
            subscript_top_max: constant(MathConstant::SubscriptTopMax),
            subscript_baseline_drop_min: constant(MathConstant::SubscriptBaselineDropMin),
            superscript_shift_up: constant(if cramped { MathConstant::SuperscriptShiftUpCramped } else { MathConstant::SuperscriptShiftUp }),
            superscript_bottom_min: constant(MathConstant::SuperscriptBottomMin),
            superscript_baseline_drop_max: constant(MathConstant::SuperscriptBaselineDropMax),
            sub_superscript_gap_min: constant(MathConstant::SubSuperscriptGapMin),
            superscript_bottom_max_with_subscript: constant(MathConstant::SuperscriptBottomMaxWithSubscript),
            space_after_script: constant(MathConstant::SpaceAfterScript),
        }
    }

    /// The shifts of the subscript's baseline down and of the superscript's up from the base's
    /// baseline (§ 3.4.1.2 to § 3.4.1.4); 0 for a script that is not there.
    fn shifts(&self, base: &ChildMetrics, sub: Option<&ChildMetrics>, sup: Option<&ChildMetrics>) -> (f32, f32) {
        let mut sub_shift = 0.0f32;
        let mut super_shift = 0.0f32;
        if let Some(sub) = sub {
            sub_shift = self
                .subscript_shift_down
                .max(sub.ascent - self.subscript_top_max)
                .max(self.subscript_baseline_drop_min + base.descent);
        }
        if let Some(sup) = sup {
            super_shift = self
                .superscript_shift_up
                .max(self.superscript_bottom_min + sup.descent)
                .max(base.ascent - self.superscript_baseline_drop_max);
        }
        if let (Some(sub), Some(sup)) = (sub, sup) {
            let mut gap = (sub_shift - sub.ascent) + (super_shift - sup.descent);
            if gap < self.sub_superscript_gap_min {
                let mut delta = self.superscript_bottom_max_with_subscript - (super_shift - sup.descent);
                if delta > 0.0 {
                    delta = delta.min(self.sub_superscript_gap_min - gap);
                    super_shift += delta;
                    gap += delta;
                }
                let delta = self.sub_superscript_gap_min - gap;
                if delta > 0.0 {
                    sub_shift += delta;
                }
            }
        }
        (sub_shift, super_shift)
    }
}

/// Whether the child box `id` is an `mprescripts`.
fn is_prescripts(state: &LayoutPassState<'_>, id: NodeId) -> bool {
    state.nodes[id].element_data().is_some_and(|element| element.name.ns == ns!(mathml) && &*element.name.local == "mprescripts")
}

/// The structure of an `mmultiscripts`' children (indices into them): the postscript pairs, the
/// `mprescripts` and the prescript pairs; `None` if invalid (§ 3.4.3: an odd number of scripts on
/// either side, a second `mprescripts`, or one as the base).
struct Multiscripts {
    postscripts: Vec<(usize, usize)>,
    prescripts_at: Option<usize>,
    prescripts: Vec<(usize, usize)>,
}

fn parse_multiscripts(state: &LayoutPassState<'_>, children: &[ChildMetrics]) -> Option<Multiscripts> {
    if is_prescripts(state, children[0].id) {
        return None;
    }
    let rest = &children[1..];
    let marker = rest.iter().position(|child| is_prescripts(state, child.id));
    let none: &[ChildMetrics] = &[];
    let (post, pre): (&[ChildMetrics], &[ChildMetrics]) = match marker {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, none),
    };
    if post.len() % 2 != 0 || pre.len() % 2 != 0 || pre.iter().any(|child| is_prescripts(state, child.id)) {
        return None;
    }
    let first_post = 1;
    let first_pre = marker.map_or(0, |at| 1 + at + 1);
    Some(Multiscripts {
        postscripts: (0..post.len() / 2).map(|pair| (first_post + 2 * pair, first_post + 2 * pair + 1)).collect(),
        prescripts_at: marker.map(|at| 1 + at),
        prescripts: (0..pre.len() / 2).map(|pair| (first_pre + 2 * pair, first_pre + 2 * pair + 1)).collect(),
    })
}

/// `math-shift: compact` on the element.
fn is_cramped(state: &LayoutPassState<'_>, node_id: NodeId) -> bool {
    state.nodes[node_id]
        .primary_styles()
        .is_some_and(|style| style.get_font().math_shift == style::values::computed::MathShift::Compact)
}

/// Layout of a box `construct` built (see the module's documentation).
pub(crate) fn layout(state: &mut LayoutPassState<'_>, node_id: NodeId, kind: MathKind, ctx: &MathContext, children: &[ChildMetrics]) -> MathLayout {
    let valid = match kind {
        MathKind::Sub | MathKind::Sup => children.len() == 2,
        MathKind::SubSup => children.len() == 3,
        MathKind::Multiscripts => !children.is_empty(),
        _ => false,
    };
    if !valid {
        return row::layout_row(ctx, children);
    }
    let multiscripts = if kind == MathKind::Multiscripts {
        match parse_multiscripts(state, children) {
            Some(parsed) => Some(parsed),
            None => return row::layout_row(ctx, children),
        }
    } else {
        None
    };
    let fallbacks = Fallbacks::load(state, node_id, ctx);
    let params = Parameters::new(ctx, &fallbacks, is_cramped(state, node_id));
    let base = children[0];
    // § 3.4.1.2: a large operator's italic correction pulls the subscript back; any other base's
    // pushes the superscript on.
    let base_italic = child_italic_correction(state, &base);
    let large_op = embellished_operator(state, base.id).is_some_and(|properties| properties.largeop);
    let (italic, large_op_italic) = if large_op { (0.0, base_italic) } else { (base_italic, 0.0) };

    match multiscripts {
        Some(structure) => layout_multiscripts(&params, children, &structure, italic, large_op_italic),
        None => {
            let (sub, sup) = match kind {
                MathKind::Sub => (Some(children[1]), None),
                MathKind::Sup => (None, Some(children[1])),
                _ => (Some(children[1]), Some(children[2])),
            };
            layout_postscripts(&params, base, sub, sup, italic, large_op_italic)
        }
    }
}

/// `msub`, `msup` and `msubsup`.
fn layout_postscripts(
    params: &Parameters,
    base: ChildMetrics,
    sub: Option<ChildMetrics>,
    sup: Option<ChildMetrics>,
    italic: f32,
    large_op_italic: f32,
) -> MathLayout {
    let (sub_shift, super_shift) = params.shifts(&base, sub.as_ref(), sup.as_ref());
    let base_width = base.margin_box_width();
    let mut width = base_width;
    let mut ascent = base.ascent;
    let mut descent = base.descent;
    if let Some(sub) = &sub {
        width = width.max(base_width - large_op_italic + sub.margin_box_width() + params.space_after_script);
        ascent = ascent.max(sub.ascent - sub_shift);
        descent = descent.max(sub.descent + sub_shift);
    }
    if let Some(sup) = &sup {
        width = width.max(base_width + italic + sup.margin_box_width() + params.space_after_script);
        ascent = ascent.max(sup.ascent + super_shift);
        descent = descent.max(sup.descent - super_shift);
    }
    let mut placements = Vec::with_capacity(3);
    placements.push(ChildPlacement { metrics: base, inline_offset: 0.0, block_offset: ascent - base.ascent });
    if let Some(sub) = sub {
        placements.push(ChildPlacement { metrics: sub, inline_offset: base_width - large_op_italic, block_offset: ascent + sub_shift - sub.ascent });
    }
    if let Some(sup) = sup {
        placements.push(ChildPlacement { metrics: sup, inline_offset: base_width + italic, block_offset: ascent - super_shift - sup.ascent });
    }
    MathLayout { content_width: width, ascent, descent, children: placements, italic_correction: 0.0 }
}

/// `mmultiscripts` (§ 3.4.3.1).
fn layout_multiscripts(params: &Parameters, children: &[ChildMetrics], structure: &Multiscripts, italic: f32, large_op_italic: f32) -> MathLayout {
    let base = children[0];
    let pairs = || structure.prescripts.iter().chain(&structure.postscripts).copied();

    // All pairs share the largest shifts.
    let (mut sub_shift, mut super_shift) = (0.0f32, 0.0f32);
    for (sub, sup) in pairs() {
        let (pair_sub, pair_super) = params.shifts(&base, Some(&children[sub]), Some(&children[sup]));
        sub_shift = sub_shift.max(pair_sub);
        super_shift = super_shift.max(pair_super);
    }
    let mut ascent = base.ascent;
    let mut descent = base.descent;
    for (sub, sup) in pairs() {
        ascent = ascent.max(children[sub].ascent - sub_shift).max(children[sup].ascent + super_shift);
        descent = descent.max(children[sub].descent + sub_shift).max(children[sup].descent - super_shift);
    }
    let sub_block = |sub: &ChildMetrics| ascent + sub_shift - sub.ascent;
    let sup_block = |sup: &ChildMetrics| ascent - super_shift - sup.ascent;

    // (child index, placement)
    let mut placed: Vec<(usize, ChildPlacement)> = Vec::with_capacity(children.len());
    let mut x = 0.0f32;
    // The prescripts, each pair right-aligned and preceded by `SpaceAfterScript`.
    for &(sub, sup) in &structure.prescripts {
        x += params.space_after_script;
        let (sub_metrics, sup_metrics) = (children[sub], children[sup]);
        let pair_width = sub_metrics.margin_box_width().max(sup_metrics.margin_box_width());
        placed.push((sub, ChildPlacement { metrics: sub_metrics, inline_offset: x + pair_width - sub_metrics.margin_box_width(), block_offset: sub_block(&sub_metrics) }));
        placed.push((sup, ChildPlacement { metrics: sup_metrics, inline_offset: x + pair_width - sup_metrics.margin_box_width(), block_offset: sup_block(&sup_metrics) }));
        x += pair_width;
    }
    placed.push((0, ChildPlacement { metrics: base, inline_offset: x, block_offset: ascent - base.ascent }));
    if let Some(at) = structure.prescripts_at {
        let marker = children[at];
        placed.push((at, ChildPlacement { metrics: marker, inline_offset: x, block_offset: ascent - marker.ascent }));
    }
    // The postscripts after the base.
    let mut cursor = x + base.margin_box_width();
    let mut width = cursor;
    for &(sub, sup) in &structure.postscripts {
        let (sub_metrics, sup_metrics) = (children[sub], children[sup]);
        placed.push((sub, ChildPlacement { metrics: sub_metrics, inline_offset: cursor - large_op_italic, block_offset: sub_block(&sub_metrics) }));
        placed.push((sup, ChildPlacement { metrics: sup_metrics, inline_offset: cursor + italic, block_offset: sup_block(&sup_metrics) }));
        let end = (cursor - large_op_italic + sub_metrics.margin_box_width()).max(cursor + italic + sup_metrics.margin_box_width());
        width = width.max(end + params.space_after_script);
        cursor += sub_metrics.margin_box_width().max(sup_metrics.margin_box_width()) + params.space_after_script;
    }
    placed.sort_by_key(|(index, _)| *index);
    MathLayout { content_width: width, ascent, descent, children: placed.into_iter().map(|(_, placement)| placement).collect(), italic_correction: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;
    use taffy::{Rect, ScrollableOverflow, Size};

    fn child(id: u64, width: f32, ascent: f32, descent: f32) -> ChildMetrics {
        ChildMetrics {
            id: NodeId::from_u64(id),
            margin: Rect::ZERO,
            size: Size { width, height: ascent + descent },
            overflow: ScrollableOverflow::ZERO,
            ascent,
            descent,
            italic_correction: 0.0,
            kind: None,
        }
    }

    fn params() -> Parameters {
        Parameters {
            subscript_shift_down: 3.0,
            subscript_top_max: 8.0,
            subscript_baseline_drop_min: 2.0,
            superscript_shift_up: 7.0,
            superscript_bottom_min: 2.0,
            superscript_baseline_drop_max: 5.0,
            sub_superscript_gap_min: 4.0,
            superscript_bottom_max_with_subscript: 6.0,
            space_after_script: 1.0,
        }
    }

    #[test]
    fn subscript_shift_is_the_largest_of_three() {
        let p = params();
        let base = child(1, 10.0, 10.0, 4.0);
        // Descent-based: 2 + 4 = 6 beats 3 and (ascent 6 - 8).
        let (sub_shift, _) = p.shifts(&base, Some(&child(2, 5.0, 6.0, 1.0)), None);
        assert_eq!(sub_shift, 6.0);
        // A tall subscript is held down by SubscriptTopMax: 20 - 8 = 12.
        let (sub_shift, _) = p.shifts(&base, Some(&child(2, 5.0, 20.0, 1.0)), None);
        assert_eq!(sub_shift, 12.0);
    }

    #[test]
    fn superscript_shift_is_the_largest_of_three() {
        let p = params();
        // ShiftUp 7; bottom-min 2 + 9 = 11 wins; base ascent 10 - 5 = 5.
        let (_, super_shift) = p.shifts(&child(1, 10.0, 10.0, 4.0), None, Some(&child(2, 5.0, 3.0, 9.0)));
        assert_eq!(super_shift, 11.0);
    }

    #[test]
    fn colliding_scripts_are_pushed_apart() {
        let p = params();
        let base = child(1, 10.0, 10.0, 0.0);
        let sub = child(2, 5.0, 3.0, 0.0);
        let sup = child(3, 5.0, 3.0, 0.0);
        let (sub_shift, super_shift) = p.shifts(&base, Some(&sub), Some(&sup));
        // Sub 3 (the descent term 2 loses), super 7: gap (3 - 3) + (7 - 0) = 7 >= 4, nothing moves.
        assert_eq!((sub_shift, super_shift), (3.0, 7.0));
        let tight = Parameters { sub_superscript_gap_min: 12.0, ..p };
        let (sub_shift, super_shift) = tight.shifts(&base, Some(&sub), Some(&sup));
        // gap 7 < 12: the superscript rises 6 - 7 < 0 (no), so the subscript drops 5.
        assert_eq!((sub_shift, super_shift), (8.0, 7.0));
    }

    #[test]
    fn subsup_layout_places_the_scripts_after_the_base() {
        let p = params();
        let layout = layout_postscripts(&p, child(1, 10.0, 10.0, 2.0), Some(child(2, 4.0, 3.0, 1.0)), Some(child(3, 6.0, 3.0, 1.0)), 0.0, 0.0);
        assert_eq!(layout.content_width, 10.0 + 6.0 + 1.0);
        assert_eq!(layout.children.len(), 3);
        assert_eq!(layout.children[1].inline_offset, 10.0);
        assert!(layout.children[1].block_offset > layout.children[2].block_offset);
    }
}
