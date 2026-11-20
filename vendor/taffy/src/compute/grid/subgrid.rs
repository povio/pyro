//! pyro: subgrids (css-grid-2 §9).
//!
//! A grid item whose `grid-template-columns` or `grid-template-rows` is `subgrid`, and which is no
//! independent formatting context ([`GridItemStyle::is_subgrid`]), takes the tracks it spans in its parent
//! grid as its own in that axis. The parent:
//!
//! - gives it a [`SubgridContext`] through the tree ([`LayoutGridContainer::set_subgrid_context`]): how many
//!   tracks it spans, the names their lines have in the parent, its gap and, once the parent sized them,
//!   the sizes of its tracks: the parent's, less the subgrid's margin, border and padding at its edges and
//!   half the difference of its gap to the parent's between them, so that its gutters are centred on the
//!   parent's;
//! - sizes its tracks with the subgrid's items in the subgrid's place ("virtual" [`GridItem`]s, through
//!   nested subgrids as well), each with that margin, border and padding at the subgrid's edges, and that
//!   half gap difference at its other edges, as an extra margin;
//! - stretches the subgrid in its subgridded axes, whatever its own alignment and size.
//!
//! The subgrid places its items as its parent placed them for sizing ([`place_subgrid_items`]): with the
//! spanned track count as its explicit grid and no implicit tracks in a subgridded axis (an item's area is
//! clamped to it), and with the parent's line names added to its own. It then lays them out in fixed
//! tracks of the sizes its parent gave it ([`subgrid_tracks`]).
//!
//! An orthogonal subgrid's items are read (style, subgrid axes, baseline flow) in the sizing grid's axes, its
//! own tracks found through [`AxisMap`]s; subgrids nested in it are mapped against the sizing grid directly.
//!
//! Not done: the standalone (own-track) sizes of an orthogonal subgrid's items.

use super::explicit_grid::{compute_explicit_grid_size_in_axis, AutoRepeatStrategy};
use super::implicit_grid::compute_grid_size_estimate;
use super::placement::{place_grid_items, ItemPlacement};
use super::types::{CellOccupancyMatrix, GridItem, GridTrack, NamedLineResolver};
use super::{OriginZeroLine, MAX_GRID_TRACKS};
use crate::geometry::{AbsoluteAxis, AbstractAxis, InBothAbsAxis, Line, Rect, Size};
use crate::style::{
    AlignSelf, AvailableSpace, BoxGenerationMode, CoreStyle, Dimension, Direction, GridContainerStyle, GridItemStyle,
    GridPlacement, LengthPercentage, LengthPercentageAuto, MaxTrackSizingFunction, MinTrackSizingFunction,
};
use crate::style_helpers::TaffyAuto;
use crate::tree::{LayoutGridContainer, LayoutPartialTreeExt, NodeId, SizingMode};
use crate::util::sys::{GridTrackVec, Vec};
use crate::util::ResolveOrZero;
use crate::CheapCloneStr;

/// pyro: what a grid gives a child that is a subgrid (css-grid-2 §9), per subgridded axis. Kept by the tree
/// ([`LayoutGridContainer::set_subgrid_context`]) and read by the child's own layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubgridContext<S> {
    /// The columns (the subgrid's inline axis), when it is a subgrid in that axis
    pub columns: Option<SubgridAxisContext<S>>,
    /// The rows (the subgrid's block axis), when it is a subgrid in that axis
    pub rows: Option<SubgridAxisContext<S>>,
    /// The subgrid's own columns (when only its rows are subgridded) as its last layout of a definite width sized
    /// them: the parent sizes its rows with the subgrid's items, whose widths are the columns they span
    pub own_columns: Option<OwnTracks>,
    /// pyro: the subgrid's own rows (when only its columns are subgridded) as its last layout sized them: the
    /// parent sizes its columns with the subgrid's items, whose heights are the rows they span
    pub own_rows: Option<OwnTracks>,
}

/// pyro: the tracks of a subgrid's own (standalone) axis as its last layout sized them ([`SubgridContext`])
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OwnTracks {
    /// The subgrid's content-box size in that axis they were sized in
    pub inner_size: f32,
    /// The tracks' sizes, in order
    pub sizes: Vec<f32>,
    /// The gap between them
    pub gap: f32,
}

impl OwnTracks {
    /// The size of the area of `lines` (origin-zero), gaps between its tracks included; `None` past the tracks
    fn area(&self, lines: Line<OriginZeroLine>) -> Option<f32> {
        let (start, end) = (lines.start.0, lines.end.0);
        if start < 0 || end as usize > self.sizes.len() || end <= start {
            return None;
        }
        let sizes: f32 = self.sizes[start as usize..end as usize].iter().sum();
        Some(sizes + self.gap * (end - start - 1) as f32)
    }
}

impl<S> SubgridContext<S> {
    /// The context of `axis` (`Horizontal`: columns), if the child is a subgrid in it
    pub fn axis(&self, axis: AbsoluteAxis) -> Option<&SubgridAxisContext<S>> {
        match axis {
            AbsoluteAxis::Horizontal => self.columns.as_ref(),
            AbsoluteAxis::Vertical => self.rows.as_ref(),
        }
    }

    /// The context of `axis`, to change it
    fn axis_mut(&mut self, axis: AbsoluteAxis) -> &mut Option<SubgridAxisContext<S>> {
        match axis {
            AbsoluteAxis::Horizontal => &mut self.columns,
            AbsoluteAxis::Vertical => &mut self.rows,
        }
    }
}

/// pyro: a subgrid's tracks in one subgridded axis, as its parent grid gives them ([`SubgridContext`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SubgridAxisContext<S> {
    /// The number of the parent's tracks the subgrid spans: its explicit tracks in this axis (it has no implicit ones)
    pub track_count: u16,
    /// The names the parent's lines give the subgrid's lines, one set per line (`track_count + 1`), in the
    /// subgrid's order: their explicit names and those of the parent's named areas the subgrid overlaps
    pub line_names: Vec<Vec<S>>,
    /// The subgrid's gap in this axis: its own, or with `normal` its parent's
    pub gap: f32,
    /// Whether the subgrid's tracks run the other way from its parent's (its `direction` is the other one)
    pub reversed: bool,
    /// The sizes of the subgrid's tracks, in its order, once the parent sized its own
    pub track_sizes: Option<Vec<f32>>,
    /// Where the subgrid's tracks start and end, in its order, from the start of its content box along its own
    /// direction: centred on the parent's gutters, a track can end before it starts (a gap wider than it), so
    /// that its size (`track_sizes`, never negative) does not give where the next one starts
    pub track_spans: Option<Vec<(f32, f32)>>,
    /// pyro: per line of the subgrid (in its order), the baselines of the parent's baseline-sharing groups that
    /// the subgrid's items there take part in (css-grid-2 §9), as the distance from the line to the group's
    /// baseline that an item of the subgrid aligned there gets (`[start side, end side]`)
    pub baselines: Option<Vec<[Option<f32>; 2]>>,
    /// pyro: where the parent synthesizes baselines in this axis (`GridContainerStyle::baseline_synthesis`), which
    /// the subgrid's items share its groups with
    pub baseline_synthesis: Option<f32>,
}

/// The context the grid `node` has from its parent, in the axes its style makes it a subgrid in
pub(super) fn own_context<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    style: &impl GridContainerStyle<CustomIdent = Tree::CustomIdent>,
) -> Option<SubgridContext<Tree::CustomIdent>> {
    let mut context = tree.subgrid_context(node)?;
    if style.subgrid_line_names(AbsoluteAxis::Horizontal).is_none() {
        context.columns = None;
    }
    if style.subgrid_line_names(AbsoluteAxis::Vertical).is_none() {
        context.rows = None;
    }
    (context.columns.is_some() || context.rows.is_some()).then_some(context)
}

/// The explicit track count of a subgrid's axis from its context, if it is subgridded
pub(super) fn subgrid_track_count<S>(context: Option<&SubgridContext<S>>, axis: AbsoluteAxis) -> Option<u16> {
    context.and_then(|context| context.axis(axis)).map(|axis| axis.track_count)
}

/// Name the lines of a subgrid's subgridded axes in `names`: its own `<line-name-list>` and the names its
/// parent's lines give it
pub(super) fn add_subgrid_line_names<S: CheapCloneStr>(
    names: &mut NamedLineResolver<S>,
    style: &impl GridContainerStyle<CustomIdent = S>,
    context: &SubgridContext<S>,
) {
    for axis in [AbsoluteAxis::Horizontal, AbsoluteAxis::Vertical] {
        let Some(sub) = context.axis(axis) else { continue };
        let line_count = sub.track_count as usize + 1;
        let own = style.subgrid_line_names(axis).map(|list| list.expand(line_count)).unwrap_or_default();
        names.add_subgrid_line_names(axis, &own, &sub.line_names);
    }
}

/// A placement with an automatic span (`auto`, or one line and `auto`) given `span` tracks: a subgrid's
/// span in a subgridded axis is the number of tracks its `<line-name-list>` names, at least one (css-grid-2 §9)
fn with_automatic_span<S: CheapCloneStr>(placement: Line<GridPlacement<S>>, span: u16) -> Line<GridPlacement<S>> {
    match (&placement.start, &placement.end) {
        (GridPlacement::Auto, GridPlacement::Auto) => Line { start: GridPlacement::Auto, end: GridPlacement::Span(span) },
        (GridPlacement::Line(_) | GridPlacement::NamedLine(..), GridPlacement::Auto) => {
            Line { start: placement.start, end: GridPlacement::Span(span) }
        }
        (GridPlacement::Auto, GridPlacement::Line(_) | GridPlacement::NamedLine(..)) => {
            Line { start: GridPlacement::Span(span), end: placement.end }
        }
        _ => placement,
    }
}

/// The `grid-column` and `grid-row` of a grid's in-flow child with named lines resolved (origin-zero), and the
/// axes in which the child is a subgrid (`width`: columns); a subgrid's automatic span is given by its line names
pub(super) fn child_placement<Tree: LayoutGridContainer>(
    tree: &Tree,
    child: NodeId,
    child_style: &impl GridItemStyle<CustomIdent = Tree::CustomIdent>,
    names: &NamedLineResolver<Tree::CustomIdent>,
    explicit: Size<u16>,
) -> (ItemPlacement, Size<bool>) {
    let subgrid = Size {
        width: child_style.is_subgrid(AbsoluteAxis::Horizontal),
        height: child_style.is_subgrid(AbsoluteAxis::Vertical),
    };
    let mut column = child_style.grid_column();
    let mut row = child_style.grid_row();
    if subgrid.width || subgrid.height {
        let own = tree.get_grid_container_style(child);
        let span = |axis| {
            own.subgrid_line_names(child_style.subgrid_axis(axis))
                .map_or(1, |list| list.fixed_len().saturating_sub(1).clamp(1, MAX_GRID_TRACKS as usize)) as u16
        };
        if subgrid.width {
            column = with_automatic_span(column, span(AbsoluteAxis::Horizontal));
        }
        if subgrid.height {
            row = with_automatic_span(row, span(AbsoluteAxis::Vertical));
        }
    }
    let placement = InBothAbsAxis {
        horizontal: names.resolve_column_names(&column).into_origin_zero(explicit.width),
        vertical: names.resolve_row_names(&row).into_origin_zero(explicit.height),
    };
    (placement, subgrid)
}

/// Lines clamped to an explicit grid of `count` tracks, spanning at least one of them (a subgrid has no
/// implicit tracks in a subgridded axis: css-grid-2 §9, as an overlarge grid clamps, css-grid-1 §7.1)
pub(super) fn clamp_to_subgrid(line: Line<OriginZeroLine>, count: u16) -> Line<OriginZeroLine> {
    let count = count.max(1) as i16;
    let start = line.start.0.clamp(0, count - 1);
    let end = line.end.0.clamp(start + 1, count);
    Line { start: OriginZeroLine(start), end: OriginZeroLine(end) }
}

/// The definite lines of a subgrid's items in its subgridded axes, clamped to its explicit grid before they are
/// auto-placed in the other axis (css-grid-2 §9: an item's grid area is clamped): an item whose lines lie past the
/// subgrid takes cells in it, which the next items there find occupied, instead of cells of implicit tracks that
/// the clamp after placement then laid over each other (WPT `css/css-grid/subgrid/repeat-auto-fill-001`)
pub(super) fn clamp_placements<S>(placements: &mut [ItemPlacement], context: &SubgridContext<S>) {
    fn clamp(line: &mut Line<crate::style::OriginZeroGridPlacement>, count: u16) {
        if line.is_definite() {
            let lines = clamp_to_subgrid(line.resolve_definite_grid_lines(), count);
            *line = Line {
                start: crate::style::GenericGridPlacement::Line(lines.start),
                end: crate::style::GenericGridPlacement::Line(lines.end),
            };
        }
    }
    for placement in placements {
        if let Some(columns) = &context.columns {
            clamp(&mut placement.horizontal, columns.track_count);
        }
        if let Some(rows) = &context.rows {
            clamp(&mut placement.vertical, rows.track_count);
        }
    }
}

/// Clamp the items of a subgrid to its explicit grid in its subgridded axes
pub(super) fn clamp_items<S>(items: &mut [GridItem], context: &SubgridContext<S>) {
    for item in items {
        if let Some(columns) = &context.columns {
            item.column = clamp_to_subgrid(item.column, columns.track_count);
        }
        if let Some(rows) = &context.rows {
            item.row = clamp_to_subgrid(item.row, rows.track_count);
        }
    }
}

/// The tracks of a subgridded axis: the sizes its parent gave as fixed tracks, or `auto` tracks while it has
/// given none, between gutters of the subgrid's gap (the outer ones collapsed, as for any grid)
pub(super) fn subgrid_tracks<S>(tracks: &mut GridTrackVec<GridTrack>, axis: &SubgridAxisContext<S>) {
    let gap = LengthPercentage::length(axis.gap);
    tracks.clear();
    tracks.push(GridTrack::gutter(gap));
    for index in 0..axis.track_count.max(1) as usize {
        let track = match axis.track_sizes.as_ref().and_then(|sizes| sizes.get(index)) {
            Some(&size) => GridTrack::new(MinTrackSizingFunction::length(size), MaxTrackSizingFunction::length(size)),
            None => GridTrack::new(MinTrackSizingFunction::AUTO, MaxTrackSizingFunction::AUTO),
        };
        tracks.push(track);
        tracks.push(GridTrack::gutter(gap));
    }
    tracks.first_mut().unwrap().collapse();
    tracks.last_mut().unwrap().collapse();
}

/// A subgrid stretched in `axes` (css-grid-2 §9): its own alignment, size, minimum and maximum size and
/// aspect ratio take no part there, and an `auto` margin is zero
pub(super) fn stretch_subgrid_item(item: &mut GridItem, axes: Size<bool>) {
    if axes.width {
        item.size.width = Dimension::AUTO;
        item.min_size.width = LengthPercentageAuto::AUTO;
        item.max_size.width = LengthPercentageAuto::AUTO;
        item.justify_self = AlignSelf::STRETCH;
        for margin in [&mut item.margin.left, &mut item.margin.right] {
            if margin.is_auto() {
                *margin = LengthPercentageAuto::length(0.0);
            }
        }
    }
    if axes.height {
        item.size.height = Dimension::AUTO;
        item.min_size.height = LengthPercentageAuto::AUTO;
        item.max_size.height = LengthPercentageAuto::AUTO;
        item.align_self = AlignSelf::STRETCH;
        for margin in [&mut item.margin.top, &mut item.margin.bottom] {
            if margin.is_auto() {
                *margin = LengthPercentageAuto::length(0.0);
            }
        }
    }
    if axes.width || axes.height {
        item.aspect_ratio = None;
    }
}

/// Place the in-flow children of the subgrid `node` in its grid, as its own layout places them: with its
/// context's track counts (and no implicit tracks) and line names in its subgridded axes. The items are
/// aligned by the subgrid's `align-items` and `justify-items`; their lines are the subgrid's (origin-zero).
/// Also answers the subgrid's line names (for nested subgrids). The subgrid's own size is not known to its
/// parent, so a `repeat(auto-fill)` in its own axis repeats once, where its own layout may repeat it more.
pub(super) fn place_subgrid_items<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    context: &SubgridContext<Tree::CustomIdent>,
) -> (Vec<GridItem>, NamedLineResolver<Tree::CustomIdent>) {
    let style = tree.get_grid_container_style(node);
    let explicit_in = |axis| match subgrid_track_count(Some(context), axis) {
        Some(count) => (0, count),
        None => compute_explicit_grid_size_in_axis(
            &style,
            None,
            AutoRepeatStrategy::MinRepetitionsThatDoOverflow,
            |val, basis| tree.calc(val, basis),
            axis,
        ),
    };
    let (column_repetitions, column_template_count) = explicit_in(AbsoluteAxis::Horizontal);
    let (row_repetitions, row_template_count) = explicit_in(AbsoluteAxis::Vertical);
    let mut names = NamedLineResolver::new(&style, column_repetitions, row_repetitions);
    let explicit = Size {
        width: match context.columns {
            Some(_) => column_template_count,
            None => column_template_count.max(names.area_column_count()).min(MAX_GRID_TRACKS),
        },
        height: match context.rows {
            Some(_) => row_template_count,
            None => row_template_count.max(names.area_row_count()).min(MAX_GRID_TRACKS),
        },
    };
    names.set_explicit_column_count(explicit.width);
    names.set_explicit_row_count(explicit.height);
    add_subgrid_line_names(&mut names, &style, context);
    let align_items = style.align_items();
    let justify_items = style.justify_items();
    let flow = style.grid_auto_flow();
    drop(style);

    let mut items: Vec<GridItem> = Vec::new();
    let mut placements: Vec<ItemPlacement> = Vec::new();
    for (index, child) in tree.child_ids(node).enumerate() {
        let child_style = tree.get_grid_child_style(child);
        if child_style.box_generation_mode() == BoxGenerationMode::None || child_style.position().is_out_of_flow() {
            continue;
        }
        let (placement, subgrid) = child_placement(tree, child, &child_style, &names, explicit);
        let mut item = GridItem::new_with_style_and_order(child, child_style, align_items, justify_items, index as u16);
        item.subgrid = subgrid;
        items.push(item);
        placements.push(placement);
    }
    clamp_placements(&mut placements, context);
    let (column_counts, row_counts) = compute_grid_size_estimate(explicit.width, explicit.height, &placements);
    let mut occupancy = CellOccupancyMatrix::with_track_counts(column_counts, row_counts);
    place_grid_items(&mut occupancy, &mut items, &placements, flow);
    clamp_items(&mut items, context);
    (items, names)
}

/// The margin, border and padding of a box, `auto` margins as zero, percentages against `basis`
fn margin_border_padding<Tree: LayoutGridContainer>(
    tree: &Tree,
    style: &impl CoreStyle,
    basis: Option<f32>,
) -> Rect<f32> {
    let calc = |val, basis| tree.calc(val, basis);
    let margin = style.margin().map(|margin| margin.resolve_or_zero(basis, calc));
    let border = style.border().map(|border| border.resolve_or_zero(basis, calc));
    let padding = style.padding().map(|padding| padding.resolve_or_zero(basis, calc));
    margin + border + padding
}

/// The used gap of a subgrid in `axis`: its own, or with `normal` its parent's (`parent_gap`)
fn subgrid_gap<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    axis: AbsoluteAxis,
    parent_gap: f32,
    basis: Option<f32>,
) -> f32 {
    let style = tree.get_grid_container_style(node);
    if style.gap_is_normal().get_abs(axis) {
        return parent_gap;
    }
    style.gap().get_abs(axis).resolve_or_zero(basis, |val, basis| tree.calc(val, basis))
}

/// What a grid knows of itself for the contexts of its subgrid children
pub(super) struct SubgridParent<'a, S: CheapCloneStr> {
    /// The grid's line names
    pub names: &'a NamedLineResolver<S>,
    /// The grid's gaps (`width`: between columns)
    pub gap: Size<f32>,
    /// The grid's `direction`
    pub direction: Direction,
    /// The basis of percentages in its subgrids' margins, borders, paddings and gaps (the grid's inner width)
    pub percent_basis: Option<f32>,
}

/// How a subgrid's own axis matches one of its parent's: which of its own it is (the other one in an
/// orthogonal flow), and whether its tracks run the other way
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AxisMap {
    own: AbsoluteAxis,
    reversed: bool,
}

/// The maps of a subgrid's axes against those of the grid whose writing mode its `style` is read in
fn axis_maps(style: &impl GridItemStyle) -> InBothAbsAxis<AxisMap> {
    let map = |axis| AxisMap { own: style.subgrid_axis(axis), reversed: style.subgrid_reversed(axis) };
    InBothAbsAxis { horizontal: map(AbsoluteAxis::Horizontal), vertical: map(AbsoluteAxis::Vertical) }
}

/// The context a grid (`parent`) gives its child `node`, a subgrid in `axes` placed at `area` (the grid's
/// origin-zero lines): the spanned track counts and their line names, the subgrid's gaps, and the track
/// sizes it was given before while it spans as many tracks (until the grid sizes them again)
fn child_context<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    axes: Size<bool>,
    area: InBothAbsAxis<Line<OriginZeroLine>>,
    parent: &SubgridParent<'_, Tree::CustomIdent>,
    maps: InBothAbsAxis<AxisMap>,
) -> SubgridContext<Tree::CustomIdent> {
    let previous = tree.subgrid_context(node);
    let mut context = SubgridContext {
        own_columns: previous.as_ref().and_then(|previous| previous.own_columns.clone()),
        own_rows: previous.as_ref().and_then(|previous| previous.own_rows.clone()),
        ..SubgridContext::default()
    };
    for axis in [AbsoluteAxis::Horizontal, AbsoluteAxis::Vertical] {
        if !axes.get_abs(axis) {
            continue;
        }
        let line = area.get(axis);
        let track_count = line.span();
        let AxisMap { own, reversed } = maps.get(axis);
        let mut line_names = parent.names.names_of_lines(axis, line.start.0, line.end.0);
        for (index, name) in parent.names.area_names_for_subgrid(axis, area) {
            if let Some(names) = line_names.get_mut(index as usize) {
                names.push(name);
            }
        }
        if reversed {
            line_names.reverse();
        }
        let previous_axis =
            previous.as_ref().and_then(|previous| previous.axis(own)).filter(|previous| previous.track_count == track_count);
        let track_sizes = previous_axis.and_then(|previous| previous.track_sizes.clone());
        let track_spans = previous_axis.and_then(|previous| previous.track_spans.clone());
        let baselines = previous_axis.and_then(|previous| previous.baselines.clone());
        *context.axis_mut(own) = Some(SubgridAxisContext {
            track_count,
            line_names,
            // (A percentage gap counts as zero while the parent sizes its tracks: the subgrid's size it resolves
            // against is not known yet; `update_subgrid_track_sizes` resolves it once it is)
            gap: subgrid_gap(tree, node, own, parent.gap.get_abs(axis), None),
            reversed,
            track_sizes,
            track_spans,
            baselines,
            baseline_synthesis: previous_axis.and_then(|previous| previous.baseline_synthesis),
        });
    }
    context
}

/// Give each in-flow item of a grid that is a subgrid its context, stretch it and keep it out of track sizing in
/// its subgridded axes, and append the items that size the grid's tracks in its place (virtual items: its own,
/// and those of nested subgrids). Clears the context of the other items. `items` holds the grid's own items.
pub(super) fn prepare_subgrid_items<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &mut Vec<GridItem>,
    parent: &SubgridParent<'_, Tree::CustomIdent>,
) {
    let own_items = items.len();
    for index in 0..own_items {
        let (node, subgrid) = (items[index].node, items[index].subgrid);
        if !subgrid.width && !subgrid.height {
            tree.set_subgrid_context(node, None);
            continue;
        }
        let area = InBothAbsAxis { horizontal: items[index].column, vertical: items[index].row };
        let maps = axis_maps(&tree.get_grid_child_style(node));
        let context = child_context(tree, node, subgrid, area, parent, maps);
        tree.set_subgrid_context(node, Some(context.clone()));
        let item = &mut items[index];
        item.excluded = subgrid;
        stretch_subgrid_item(item, subgrid);
        collect_virtual_items(tree, node, &context, area, subgrid, Rect::ZERO, parent, maps, items, true);
    }
}

/// Append the items of the subgrid `node` (placed at `area` in the sizing grid's lines) that size the sizing
/// grid's tracks in `sizing` (the axes it shares with that grid), with their extra margins: `edge_extra` (what
/// the subgrid's own edges carry from enclosing subgrids) plus its margin, border and padding at its edges, and
/// half its gap's difference to the sizing grid's elsewhere. Nested subgrids give their items in turn.
///
/// Where no item of the subgrid lies at an edge, the items at the edge-most occupied track are also given
/// stretched to that edge with its margin, border and padding (css-grid-2 §9's hypothetical items); a subgrid
/// with no item at all in an axis gives an item of no size spanning its tracks with them at both edges, so
/// that its margin, border and padding still take room (as in Blink).
#[allow(clippy::too_many_arguments)]
fn collect_virtual_items<Tree: LayoutGridContainer>(
    tree: &Tree,
    node: NodeId,
    context: &SubgridContext<Tree::CustomIdent>,
    area: InBothAbsAxis<Line<OriginZeroLine>>,
    sizing: Size<bool>,
    edge_extra: Rect<f32>,
    parent: &SubgridParent<'_, Tree::CustomIdent>,
    maps: InBothAbsAxis<AxisMap>,
    out: &mut Vec<GridItem>,
    // pyro: whether `node` is a child of the sizing grid (not a nested subgrid)
    top_level: bool,
) {
    // An orthogonal subgrid's own axes are the sizing grid's swapped
    let orthogonal = maps.horizontal.own != AbsoluteAxis::Horizontal;
    let style = tree.get_grid_child_style(node);
    let mbp = margin_border_padding(tree, &style, parent.percent_basis);
    let direction = style.direction();
    drop(style);
    let (items, names) = place_subgrid_items(tree, node, context);
    let rtl = parent.direction.is_rtl();
    // The extra margins at the subgrid's start and end in an axis of the sizing grid
    let mbp_edges = Size {
        width: if rtl { (mbp.right, mbp.left) } else { (mbp.left, mbp.right) },
        height: (mbp.top, mbp.bottom),
    };
    let extra_edges = Size {
        width: if rtl { (edge_extra.right, edge_extra.left) } else { (edge_extra.left, edge_extra.right) },
        height: (edge_extra.top, edge_extra.bottom),
    };
    let edge_margins = |axis: AbsoluteAxis| {
        let ((mbp_start, mbp_end), (extra_start, extra_end)) = (mbp_edges.get_abs(axis), extra_edges.get_abs(axis));
        (extra_start + mbp_start, extra_end + mbp_end)
    };
    // Set the extra margins of an axis (start and end in the sizing grid's order) on a physical rect
    let set_extra = |extra: &mut Rect<f32>, axis: AbsoluteAxis, start: f32, end: f32| match axis {
        AbsoluteAxis::Horizontal => (extra.left, extra.right) = if rtl { (end, start) } else { (start, end) },
        AbsoluteAxis::Vertical => (extra.top, extra.bottom) = (start, end),
    };
    let first_new = out.len();
    // This subgrid's own items as they were pushed (index in `out`), for the edge items
    let mut direct: Vec<usize> = Vec::new();
    // pyro: the areas of the nested subgrids (and the axes they are nested in), which take this subgrid's
    // tracks as its items do for where its margin, border and padding go (`orthogonal-writing-mode-001`)
    let mut nested_areas: Vec<(Size<bool>, InBothAbsAxis<Line<OriginZeroLine>>)> = Vec::new();
    for mut item in items {
        let local = InBothAbsAxis { horizontal: item.column, vertical: item.row };
        item.subgrid_owner = Some(node);
        item.owner_lines = Size { width: item.column, height: item.row };
        let mut extra = Rect::ZERO;
        for axis in [AbsoluteAxis::Horizontal, AbsoluteAxis::Vertical] {
            let own = maps.get(axis).own;
            let Some(sub) = context.axis(own).filter(|_| sizing.get_abs(axis)) else {
                // The subgrid's own tracks: the item's area in the sizing grid is the subgrid's whole span
                match axis {
                    AbsoluteAxis::Horizontal => item.column = area.horizontal,
                    AbsoluteAxis::Vertical => item.row = area.vertical,
                }
                continue;
            };
            let count = sub.track_count as i16;
            let line = local.get(own);
            let (start, end) = if sub.reversed { (count - line.end.0, count - line.start.0) } else { (line.start.0, line.end.0) };
            let origin = area.get(axis).start.0;
            let mapped = Line { start: OriginZeroLine(origin + start), end: OriginZeroLine(origin + end) };
            let half_gap_difference = (sub.gap - parent.gap.get_abs(axis)) / 2.0;
            let (edge_start, edge_end) = edge_margins(axis);
            let start_extra = if start == 0 { edge_start } else { half_gap_difference };
            let end_extra = if end == count { edge_end } else { half_gap_difference };
            match axis {
                AbsoluteAxis::Horizontal => item.column = mapped,
                AbsoluteAxis::Vertical => item.row = mapped,
            }
            set_extra(&mut extra, axis, start_extra, end_extra);
        }
        item.is_virtual = true;
        item.extra_margin = extra;
        // pyro: the items of a subgrid that is a child of the sizing grid, in axes mapped straight onto it, take
        // part in its baseline alignment (`record_subgrid_baselines`); the others are aligned by their fallback.
        let no_baseline = |alignment: crate::style::AlignSelf| match alignment.keyword {
            crate::AlignItemsKeyword::Baseline => crate::style::AlignSelf::START,
            crate::AlignItemsKeyword::LastBaseline => crate::style::AlignSelf::END,
            _ => alignment,
        };
        if orthogonal {
            // (its alignment in the subgrid's axes, which are this grid's swapped; its baseline flow is read against
            // this grid's writing mode already, as its style)
            core::mem::swap(&mut item.align_self, &mut item.justify_self);
        }
        if !top_level || maps.vertical.reversed {
            item.align_self = no_baseline(item.align_self);
        }
        if !top_level || maps.horizontal.reversed {
            item.justify_self = no_baseline(item.justify_self);
        }
        let mut excluded = Size { width: !sizing.width, height: !sizing.height };
        let nested = Size { width: item.subgrid.width && sizing.width, height: item.subgrid.height && sizing.height };
        if (nested.width || nested.height) && orthogonal {
            // pyro: a subgrid nested in an orthogonal one: its flags, like its style, are read in the sizing grid's
            // axes, as is the area it was mapped to there, so its context is found against the sizing grid's
            // lines directly (its lines' names: the sizing grid's; its gaps: this subgrid's)
            let gap_of = |axis: AbsoluteAxis| context.axis(maps.get(axis).own).map_or(parent.gap.get_abs(axis), |sub| sub.gap);
            let nested_parent = SubgridParent {
                names: parent.names,
                gap: Size { width: gap_of(AbsoluteAxis::Horizontal), height: gap_of(AbsoluteAxis::Vertical) },
                direction: parent.direction,
                percent_basis: parent.percent_basis,
            };
            let nested_maps = axis_maps(&tree.get_grid_child_style(item.node));
            let mapped_area = InBothAbsAxis { horizontal: item.column, vertical: item.row };
            let nested_context = child_context(tree, item.node, nested, mapped_area, &nested_parent, nested_maps);
            collect_virtual_items(tree, item.node, &nested_context, mapped_area, nested, extra, parent, nested_maps, out, false);
            nested_areas.push((nested, mapped_area));
            stretch_subgrid_item(&mut item, nested);
            excluded.width |= nested.width;
            excluded.height |= nested.height;
        } else if nested.width || nested.height {
            // A nested subgrid: its lines' names are this subgrid's for its area; its direction is compared
            // with the sizing grid's, whose lines its items are mapped to.
            let nested_parent = SubgridParent {
                names: &names,
                gap: Size {
                    width: context.columns.as_ref().map_or(parent.gap.width, |columns| columns.gap),
                    height: context.rows.as_ref().map_or(parent.gap.height, |rows| rows.gap),
                },
                direction,
                percent_basis: parent.percent_basis,
            };
            // Its maps against the sizing grid, and against this subgrid (its names are this subgrid's)
            let nested_maps = axis_maps(&tree.get_grid_child_style(item.node));
            let relative = InBothAbsAxis {
                horizontal: AxisMap {
                    own: nested_maps.horizontal.own,
                    reversed: nested_maps.horizontal.reversed != maps.horizontal.reversed,
                },
                vertical: AxisMap {
                    own: nested_maps.vertical.own,
                    reversed: nested_maps.vertical.reversed != maps.vertical.reversed,
                },
            };
            let mut nested_context = child_context(tree, item.node, nested, local, &nested_parent, relative);
            for axis in [AbsoluteAxis::Horizontal, AbsoluteAxis::Vertical] {
                let map = nested_maps.get(axis);
                if let Some(sub) = nested_context.axis_mut(map.own).as_mut() {
                    sub.reversed = map.reversed;
                }
            }
            let mapped_area = InBothAbsAxis { horizontal: item.column, vertical: item.row };
            nested_areas.push((nested, mapped_area));
            let before = out.len();
            collect_virtual_items(tree, item.node, &nested_context, mapped_area, nested, extra, parent, nested_maps, out, false);
            // pyro: a nested subgrid in both its axes has no tracks of its own: its items belong to this subgrid's
            // tracks, at its lines here (`resolve_standalone_heights`; WPT css-grid `subgrid/standalone-axis-size-013`)
            if item.subgrid.width && item.subgrid.height && !relative.horizontal.reversed && !relative.vertical.reversed {
                let shift = |lines: Line<OriginZeroLine>, by: OriginZeroLine| Line {
                    start: OriginZeroLine(lines.start.0 + by.0),
                    end: OriginZeroLine(lines.end.0 + by.0),
                };
                for virtual_item in &mut out[before..] {
                    if virtual_item.subgrid_owner == Some(item.node) {
                        virtual_item.subgrid_owner = Some(node);
                        virtual_item.owner_lines = Size {
                            width: shift(virtual_item.owner_lines.width, local.horizontal.start),
                            height: shift(virtual_item.owner_lines.height, local.vertical.start),
                        };
                    }
                }
            }
            stretch_subgrid_item(&mut item, nested);
            excluded.width |= nested.width;
            excluded.height |= nested.height;
        }
        item.excluded = excluded;
        if !(excluded.width && excluded.height) {
            direct.push(out.len());
            out.push(item);
        } else if nested.width || nested.height {
            // A nested subgrid that sizes nothing here itself: kept (taking no part in sizing) for where it
            // lies, which the widths of its items in its own columns are found from (`resolve_standalone_widths`)
            out.push(item);
        }
    }

    for axis in [AbsoluteAxis::Horizontal, AbsoluteAxis::Vertical] {
        if !sizing.get_abs(axis) {
            continue;
        }
        let excluded_in = |item: &GridItem| match axis {
            AbsoluteAxis::Horizontal => item.excluded.width,
            AbsoluteAxis::Vertical => item.excluded.height,
        };
        let lines_of = |item: &GridItem| match axis {
            AbsoluteAxis::Horizontal => item.column,
            AbsoluteAxis::Vertical => item.row,
        };
        let span = area.get(axis);
        let (edge_start, edge_end) = edge_margins(axis);
        // A zero-size item over `lines` of the subgrid's tracks that carries its margin, border and padding
        // (`start`, `end`) into track sizing
        let spacer_over = |lines: Line<OriginZeroLine>, start: f32, end: f32| {
            let mut spacer = GridItem::new_with_style_and_order(
                node,
                tree.get_grid_child_style(node),
                crate::style::AlignItems::START,
                crate::style::AlignItems::START,
                u16::MAX,
            );
            spacer.size = Size { width: Dimension::AUTO, height: Dimension::AUTO };
            match axis {
                AbsoluteAxis::Horizontal => {
                    spacer.size.width = Dimension::length(0.0);
                    spacer.column = lines;
                    spacer.row = area.vertical;
                    spacer.excluded = Size { width: false, height: true };
                }
                AbsoluteAxis::Vertical => {
                    spacer.size.height = Dimension::length(0.0);
                    spacer.row = lines;
                    spacer.column = area.horizontal;
                    spacer.excluded = Size { width: true, height: false };
                }
            }
            spacer.min_size = Size { width: LengthPercentageAuto::AUTO, height: LengthPercentageAuto::AUTO };
            spacer.max_size = Size { width: LengthPercentageAuto::AUTO, height: LengthPercentageAuto::AUTO };
            spacer.padding = Rect::zero();
            spacer.border = Rect::zero();
            spacer.margin = Rect::zero();
            spacer.box_sizing = crate::style::BoxSizing::BorderBox;
            spacer.aspect_ratio = None;
            spacer.is_compressible_replaced = false;
            // (no part in baseline alignment)
            spacer.align_self = crate::style::AlignSelf::START;
            spacer.justify_self = crate::style::AlignSelf::START;
            spacer.is_virtual = true;
            spacer.subgrid_owner = Some(node);
            let mut extra = Rect::ZERO;
            set_extra(&mut extra, axis, start, end);
            spacer.extra_margin = extra;
            spacer
        };
        // Where no item of the subgrid starts at its start (ends at its end), its margin, border and padding
        // there go into its edge track alone, not across the empty tracks between it and the edge-most item;
        // an empty subgrid has both in its edge tracks (Chromium; WPT css-grid `subgrid/grid-gap-002`,
        // `orthogonal-writing-mode-002`, `-003`)
        let empty = !out[first_new..].iter().any(|item| !excluded_in(item));
        let own: Vec<Line<OriginZeroLine>> = direct
            .iter()
            .filter(|&&index| !excluded_in(&out[index]))
            .map(|&index| lines_of(&out[index]))
            .chain(nested_areas.iter().filter(|(nested, _)| nested.get_abs(axis)).map(|(_, area)| area.get(axis)))
            .collect();
        if own.is_empty() && !empty {
            continue;
        }
        let first = own.iter().map(|lines| lines.start.0).min().unwrap_or(span.end.0);
        let last = own.iter().map(|lines| lines.end.0).max().unwrap_or(span.start.0);
        if span.span() == 1 && first != span.start.0 && last != span.end.0 {
            // (one track holds both edges)
            out.push(spacer_over(span, edge_start, edge_end));
            continue;
        }
        if first != span.start.0 && edge_start != 0.0 {
            let lines = Line { start: span.start, end: OriginZeroLine(span.start.0 + 1) };
            out.push(spacer_over(lines, edge_start, 0.0));
        }
        if last != span.end.0 && edge_end != 0.0 {
            let lines = Line { start: OriginZeroLine(span.end.0 - 1), end: span.end };
            out.push(spacer_over(lines, 0.0, edge_end));
        }
    }
}

/// Record a subgrid's own columns (only its rows are subgridded) as its layout sized them, for its parent's
/// row sizing ([`SubgridContext::own_columns`]): `inner_width` is its content-box width.
pub(super) fn record_own_columns<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    node: NodeId,
    columns: &[GridTrack],
    inner_width: f32,
) {
    let Some(mut context) = tree.subgrid_context(node) else { return };
    let sizes: Vec<f32> = columns.iter().skip(1).step_by(2).map(|track| track.base_size).collect();
    let gap = columns.get(2).filter(|_| columns.len() > 3).map_or(0.0, |gutter| gutter.base_size);
    let own = OwnTracks { inner_size: inner_width, sizes, gap };
    if context.own_columns.as_ref() != Some(&own) {
        context.own_columns = Some(own);
        tree.set_subgrid_context(node, Some(context));
    }
}

/// pyro: record the own rows of a subgrid in its columns only (`SubgridContext::own_rows`)
pub(super) fn record_own_rows<Tree: LayoutGridContainer>(tree: &mut Tree, node: NodeId, rows: &[GridTrack], inner_height: f32) {
    let Some(mut context) = tree.subgrid_context(node) else { return };
    let sizes: Vec<f32> = rows.iter().skip(1).step_by(2).map(|track| track.base_size).collect();
    let gap = rows.get(2).filter(|_| rows.len() > 3).map_or(0.0, |gutter| gutter.base_size);
    let own = OwnTracks { inner_size: inner_height, sizes, gap };
    if context.own_rows.as_ref() != Some(&own) {
        context.own_rows = Some(own);
        tree.set_subgrid_context(node, Some(context));
    }
}

/// pyro: before the grid sizes its columns: the heights of the items of its subgrid children whose rows are their
/// own (only their columns are subgridded), from those rows as the subgrid sizes them provisionally (laid out at
/// its definite height, else at its max-content height; css-grid-2 §9 "standalone axis")
pub(super) fn resolve_standalone_heights<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &mut [GridItem],
    own_item_count: usize,
    rows: &[GridTrack],
) {
    if !items[own_item_count..].iter().any(|item| item.excluded.height && item.subgrid_owner.is_some()) {
        return;
    }
    // The subgrid children in their columns only: laid out, each sizes its own rows and those of the subgrids
    // nested in it (`record_own_rows`)
    // pyro: an owner stretched over rows of fixed sizes is laid out at their height (css-grid-2 §9: its own rows are
    // sized at the height it gets), not at its max-content height (WPT css-grid `subgrid/standalone-axis-size-011`)
    let fixed_rows_height = |item: &GridItem| -> Option<f32> {
        if !matches!(item.align_self.keyword, crate::AlignItemsKeyword::Stretch | crate::AlignItemsKeyword::Normal) {
            return None;
        }
        let (start, end) = (item.row_indexes.start as usize + 1, item.row_indexes.end as usize);
        if start >= end || end > rows.len() {
            return None;
        }
        let calc = |val, basis| tree.calc(val, basis);
        let mut sum = 0.0;
        for track in &rows[start..end] {
            let min = track.min_track_sizing_function.definite_value(None, calc)?;
            let max = track.max_track_sizing_function.definite_value(None, calc)?;
            if (min - max).abs() > f32::EPSILON {
                return None;
            }
            sum += min;
        }
        let margin = item.margin.map(|margin| margin.resolve_to_option(0.0, calc));
        if margin.top.is_none() || margin.bottom.is_none() {
            return None;
        }
        Some((sum - margin.top.unwrap_or(0.0) - margin.bottom.unwrap_or(0.0)).max(0.0))
    };
    let owners: Vec<(NodeId, Option<f32>)> = items[..own_item_count]
        .iter()
        .filter(|item| item.subgrid.width && !item.subgrid.height)
        .map(|item| (item.node, fixed_rows_height(item)))
        .collect();
    for (owner, stretched_height) in owners {
        let style = tree.get_grid_child_style(owner);
        // (an orthogonal subgrid's own columns are this grid's rows: not handled)
        if style.subgrid_axis(AbsoluteAxis::Vertical) != AbsoluteAxis::Vertical {
            continue;
        }
        let calc = |val, basis| tree.calc(val, basis);
        let height = crate::util::MaybeResolve::maybe_resolve(style.size().height, None::<f32>, calc).map(|height: f32| {
            if style.box_sizing() == crate::style::BoxSizing::ContentBox {
                let padding = style.padding().map(|padding| padding.resolve_or_zero(None, calc));
                let border = style.border().map(|border| border.resolve_or_zero(None, calc));
                height + padding.vertical_axis_sum() + border.vertical_axis_sum()
            } else {
                height
            }
        });
        drop(style);
        let height = height.or(stretched_height);
        tree.measure_child_size(
            owner,
            Size { width: None, height },
            Size::NONE,
            Size {
                width: AvailableSpace::MaxContent,
                height: height.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
            },
            SizingMode::InherentSize,
            AbsoluteAxis::Vertical,
            Line::FALSE,
        );
    }
    for item in items[own_item_count..].iter_mut().filter(|item| item.excluded.height) {
        let Some(owner) = item.subgrid_owner else { continue };
        if tree.get_grid_child_style(owner).subgrid_axis(AbsoluteAxis::Vertical) != AbsoluteAxis::Vertical {
            continue;
        }
        if let Some(own) = tree.subgrid_context(owner).and_then(|context| context.own_rows) {
            item.standalone_size.height = own.area(item.owner_lines.height);
        }
    }
}

/// Before the grid sizes its rows: the widths of the items of subgrids (at any depth) whose columns are their own
/// (only their rows are subgridded), from those columns as the subgrid lays them out at the width it now gets
/// (laid out at that width when its last layout was at another)
pub(super) fn resolve_standalone_widths<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &mut [GridItem],
    own_item_count: usize,
    columns: &[GridTrack],
    parent: &SubgridParent<'_, Tree::CustomIdent>,
) {
    let _ = parent;
    let mut owners: Vec<NodeId> = Vec::new();
    for item in &items[own_item_count..] {
        if let Some(owner) = item.subgrid_owner.filter(|_| item.excluded.width) {
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
    }
    let mut resolved: Vec<(NodeId, Option<OwnTracks>, f32)> = Vec::new();
    for owner in owners {
        resolve_owner(tree, items, own_item_count, columns, owner, &mut resolved, 0);
    }
    for item in items[own_item_count..].iter_mut().filter(|item| item.excluded.width) {
        let Some(owner) = item.subgrid_owner else { continue };
        if let Some((_, own, inner)) = resolved.iter().find(|(node, _, _)| *node == owner) {
            item.standalone_size.width = match own {
                Some(own) => own.area(item.owner_lines.width),
                None => Some(*inner),
            };
        }
    }
}

/// The own columns of the subgrid `owner` (its content-box width with them), laid out at the width its area
/// gives it: the grid's columns where it lies in them, else its owner's own columns (resolved first)
fn resolve_owner<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &mut [GridItem],
    own_item_count: usize,
    columns: &[GridTrack],
    owner: NodeId,
    resolved: &mut Vec<(NodeId, Option<OwnTracks>, f32)>,
    depth: usize,
) -> Option<f32> {
    if let Some((_, _, inner)) = resolved.iter().find(|(node, _, _)| *node == owner) {
        return Some(*inner);
    }
    if depth > 16 {
        return None;
    }
    // Where the subgrid lies: a grid item, or (nested) the item a subgrid gave for it
    let index = items.iter().enumerate().position(|(index, item)| {
        item.node == owner && item.subgrid_owner != Some(owner) && (index < own_item_count || item.is_virtual)
    })?;
    let record = &items[index];
    let area_width = if !record.excluded.width || index < own_item_count {
        let range = record.track_range_excluding_lines(AbstractAxis::Inline);
        let lines: f32 = columns[range].iter().map(|track| track.base_size).sum();
        lines - record.extra_margin.left - record.extra_margin.right
    } else {
        let outer = record.subgrid_owner?;
        let lines = record.owner_lines.width;
        resolve_owner(tree, items, own_item_count, columns, outer, resolved, depth + 1)?;
        let (_, own, inner) = resolved.iter().find(|(node, _, _)| *node == outer)?;
        match own {
            Some(own) => own.area(lines)?,
            None => *inner,
        }
    };
    let row_span = items[index].row.span();
    let style = tree.get_grid_child_style(owner);
    let calc = |val, basis| tree.calc(val, basis);
    let margin = style.margin().map(|margin| margin.resolve_or_zero(Some(area_width), calc));
    let padding = style.padding().map(|padding| padding.resolve_or_zero(Some(area_width), calc));
    let border = style.border().map(|border| border.resolve_or_zero(Some(area_width), calc));
    drop(style);
    let width = (area_width - margin.horizontal_axis_sum()).max(0.0);
    let inner = (width - padding.horizontal_axis_sum() - border.horizontal_axis_sum()).max(0.0);
    let matches = |tree: &Tree| {
        tree.subgrid_context(owner)
            .and_then(|context| context.own_columns)
            .filter(|own| (own.inner_size - inner).abs() < 0.01)
    };
    let measure = |tree: &mut Tree| {
        tree.measure_child_size(
            owner,
            Size { width: Some(width), height: None },
            Size { width: Some(area_width), height: None },
            Size { width: AvailableSpace::Definite(width), height: AvailableSpace::MaxContent },
            SizingMode::InherentSize,
            AbsoluteAxis::Vertical,
            Line::FALSE,
        );
    };
    if matches(tree).is_none() {
        // A nested subgrid whose own grid has not given it its context in this layout yet: one for its rows,
        // without their sizes, so that it lays out as a subgrid in them
        let context = tree.subgrid_context(owner);
        if context.as_ref().is_none_or(|context| context.rows.is_none()) {
            let mut context = context.unwrap_or_default();
            context.rows = Some(SubgridAxisContext {
                track_count: row_span,
                line_names: (0..=row_span).map(|_| Vec::new()).collect(),
                gap: 0.0,
                reversed: false,
                track_sizes: None,
                track_spans: None,
                baselines: None,
                baseline_synthesis: None,
            });
            tree.set_subgrid_context(owner, Some(context));
        }
        measure(tree);
        if matches(tree).is_none() {
            // A cached layout stood: drop the stale columns (and with them the cache), and lay out again
            if let Some(mut context) = tree.subgrid_context(owner) {
                if context.own_columns.take().is_some() {
                    tree.set_subgrid_context(owner, Some(context));
                    measure(tree);
                }
            }
        }
    }
    resolved.push((owner, matches(tree), inner));
    Some(inner)
}

/// Give each subgrid item of a grid its tracks in `axis` from the grid's sized `tracks`: each spanned track less
/// the subgrid's margin, border and padding at the subgrid's edges, and less half the difference of the subgrid's
/// gap to the grid's gutter (as `aligned` tracks are placed, alignment's distributed space included) at its other
/// edges, so that the subgrid's gutters are centred on the grid's
pub(super) fn update_subgrid_track_sizes<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &[GridItem],
    axis: AbsoluteAxis,
    tracks: &[GridTrack],
    aligned: bool,
    parent: &SubgridParent<'_, Tree::CustomIdent>,
) {
    let rtl = axis == AbsoluteAxis::Horizontal && parent.direction.is_rtl();
    for item in items.iter().filter(|item| !item.is_virtual && item.subgrid.get_abs(axis)) {
        let Some(mut context) = tree.subgrid_context(item.node) else { continue };
        let style = tree.get_grid_child_style(item.node);
        let mbp = margin_border_padding(tree, &style, parent.percent_basis);
        let own = style.subgrid_axis(axis);
        drop(style);
        let Some(sub) = context.axis_mut(own).as_mut() else { continue };
        let (edge_start, edge_end) = match axis {
            AbsoluteAxis::Horizontal if rtl => (mbp.right, mbp.left),
            AbsoluteAxis::Horizontal => (mbp.left, mbp.right),
            AbsoluteAxis::Vertical => (mbp.top, mbp.bottom),
        };
        let abstract_axis = match axis {
            AbsoluteAxis::Horizontal => AbstractAxis::Inline,
            AbsoluteAxis::Vertical => AbstractAxis::Block,
        };
        let first = item.placement_indexes(abstract_axis).start as usize + 1;
        let count = sub.track_count as usize;
        if count == 0 || first + 2 * count > tracks.len() {
            continue;
        }
        // The spanned tracks' start and end along the grid's own direction
        let mut spans: Vec<(f32, f32)> = Vec::with_capacity(count);
        let mut position = 0.0;
        for k in 0..count {
            let track = &tracks[first + 2 * k];
            let span = if aligned {
                if rtl {
                    (-(track.offset + track.base_size), -track.offset)
                } else {
                    (track.offset, track.offset + track.base_size)
                }
            } else {
                if k > 0 {
                    position += tracks[first + 2 * k - 1].base_size;
                }
                let span = (position, position + track.base_size);
                position = span.1;
                span
            };
            spans.push(span);
        }
        // The subgrid's tracks: its margin, border and padding in from its edges, its gutters centred on the grid's
        let mut sub_spans: Vec<(f32, f32)> = (0..count)
            .map(|k| {
                let (start, end) = spans[k];
                let gap_start = if k == 0 { start } else { start + (sub.gap - (start - spans[k - 1].1)) / 2.0 };
                let gap_end = if k + 1 == count { end } else { end - (sub.gap - (spans[k + 1].0 - end)) / 2.0 };
                let start = if k == 0 { start + edge_start } else { gap_start };
                let end = if k + 1 == count { end - edge_end } else { gap_end };
                // pyro: an edge track that its margin, border and padding (not its gutters) cover more than whole
                // is empty at the inset edge, not reversed (its far line resolves there: WPT css-grid
                // `subgrid/abs-pos-002`).
                let edge_reversed = end < start && gap_end >= gap_start;
                match (k == 0, k + 1 == count) {
                    (_, true) if edge_reversed => (end, end),
                    (true, false) if edge_reversed => (start, start),
                    _ => (start, end),
                }
            })
            .collect();
        let content_start = spans[0].0 + edge_start;
        let content_end = spans[count - 1].1 - edge_end;
        // pyro: a percentage gap resolves against the subgrid's content box, now that the parent's tracks give it
        // (Chromium: the tracks were sized with it as zero, so items can overflow; WPT css-grid
        // `subgrid/grid-gap-003`)
        let gap_changed = {
            let style = tree.get_grid_container_style(item.node);
            let gap = (!style.gap_is_normal().get_abs(own)).then(|| style.gap().get_abs(own));
            drop(style);
            match gap {
                Some(gap) if gap.0.uses_percentage() => {
                    let resolved = gap.resolve_or_zero(Some((content_end - content_start).max(0.0)), |val, basis| tree.calc(val, basis));
                    let changed = resolved != sub.gap;
                    sub.gap = resolved;
                    changed
                }
                _ => false,
            }
        };
        for span in &mut sub_spans {
            *span = if sub.reversed {
                (content_end - span.1, content_end - span.0)
            } else {
                (span.0 - content_start, span.1 - content_start)
            };
        }
        if sub.reversed {
            sub_spans.reverse();
        }
        let sizes: Vec<f32> = sub_spans.iter().map(|(start, end)| (end - start).max(0.0)).collect();
        if gap_changed || sub.track_sizes.as_ref() != Some(&sizes) || sub.track_spans.as_ref() != Some(&sub_spans) {
            sub.track_sizes = Some(sizes);
            sub.track_spans = Some(sub_spans);
            tree.set_subgrid_context(item.node, Some(context));
        }
    }
}

/// Place a subgrid's tracks in a subgridded axis where its parent's lines put them ([`SubgridAxisContext::
/// track_spans`]), after the grid aligned them: `content_start` is where its content box starts (physical),
/// `content_size` its size, `rtl` whether its tracks run right to left
pub(super) fn place_subgrid_tracks<S>(
    tracks: &mut [GridTrack],
    axis: &SubgridAxisContext<S>,
    content_start: f32,
    content_size: f32,
    rtl: bool,
) {
    let Some(spans) = &axis.track_spans else { return };
    for (index, &(start, end)) in spans.iter().enumerate() {
        let track = 2 * index + 1;
        if track + 1 >= tracks.len() {
            break;
        }
        let (left, right) = if rtl { (content_size - end, content_size - start) } else { (start, end) };
        tracks[track].offset = content_start + left;
        // The gutter at the track's physical end edge starts where the track ends
        let gutter = if rtl { track - 1 } else { track + 1 };
        tracks[gutter].offset = content_start + right;
    }
}

/// pyro: give each subgrid the baselines of the groups its items (virtual items here) take part in, in the axis
/// `axis` (css-grid-2 §9): an item's shim is the same in the subgrid as here, its distance to the group's baseline
/// less the subgrid's margin, border and padding (or gap difference) at the line.
pub(super) fn record_subgrid_baselines<Tree: LayoutGridContainer>(
    tree: &mut Tree,
    items: &[GridItem],
    axis: AbstractAxis,
    synthesis: f32,
) {
    let mut owners: Vec<NodeId> = items.iter().filter_map(|item| item.subgrid_owner).collect();
    owners.sort_unstable_by_key(|owner| u64::from(*owner));
    owners.dedup();
    for owner in owners {
        let Some(mut context) = tree.subgrid_context(owner) else { continue };
        let own_axis = tree.get_grid_child_style(owner).subgrid_axis(match axis {
            AbstractAxis::Inline => AbsoluteAxis::Horizontal,
            AbstractAxis::Block => AbsoluteAxis::Vertical,
        });
        // (Orthogonal or reversed subgrids are not mapped.)
        let Some(sub) = context.axis_mut(own_axis).as_mut().filter(|sub| !sub.reversed) else { continue };
        let mut baselines: Vec<[Option<f32>; 2]> = (0..=sub.track_count).map(|_| [None, None]).collect();
        for item in items.iter().filter(|item| item.subgrid_owner == Some(owner) && item.participates_in_baseline_alignment_in(axis)) {
            let lines = item.owner_lines.get_abs(own_axis);
            let is_end = item.baseline_group_is_end(axis);
            let Some(distance) = item.baseline_distance else { continue };
            let line = if is_end { lines.end.0 } else { lines.start.0 };
            let Some(slot) = usize::try_from(line).ok().and_then(|line| baselines.get_mut(line)) else { continue };
            let slot = &mut slot[is_end as usize];
            *slot = Some(slot.map_or(distance, |other: f32| other.max(distance)));
        }
        let baselines = baselines.iter().any(|line| line[0].is_some() || line[1].is_some()).then_some(baselines);
        let synthesis = baselines.is_some().then_some(synthesis);
        if sub.baselines != baselines || sub.baseline_synthesis != synthesis {
            sub.baselines = baselines;
            sub.baseline_synthesis = synthesis;
            tree.set_subgrid_context(owner, Some(context));
        }
    }
}
