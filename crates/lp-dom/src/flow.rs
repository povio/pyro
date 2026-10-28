//! Flow layout, Stage 1 (docs/dom/FLOW_LAYOUT.md): CSS block and inline
//! formatting with relative/absolute/fixed positioning, computed from used
//! style values supplied by a [`FlowStyleSource`]. Opt-in through
//! `Layout::set_geometry(Geometry::Flow)`; the default geometry remains the
//! upstream approximation in `layout.rs`.
//!
//! Used values come from `StyleEngine`'s [`FlowStyleSource`] implementation
//! (`style_flow.rs`, owned by the style engine).
//!
//! Stage 1 lays out flex, grid and table containers as block containers and
//! ignores floats, sticky positioning, margin collapsing through parents and
//! baseline alignment. Text uses the engine's fixed-advance measure, so text
//! widths are approximate; boxes with explicit sizes match Chromium.

use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::layout::{text_width as glyph_width, Rect};
use crate::node::{NodeId, NodeType};
use crate::Dom;

#[path = "flow_reuse.rs"]
mod reuse;

// ----- style input contract -------------------------------------------------

/// A length: `px` plus `percent` of the containing block's matching axis
/// (so `calc(100% - 8px)` is `{ px: -8, percent: 100 }`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Len {
    pub px: f64,
    pub percent: f64,
}

impl Len {
    pub const ZERO: Len = Len { px: 0.0, percent: 0.0 };
    pub fn px(px: f64) -> Len {
        Len { px, percent: 0.0 }
    }
    pub fn percent(percent: f64) -> Len {
        Len { px: 0.0, percent }
    }
    pub fn resolve(self, basis: f64) -> f64 {
        self.px + self.percent * basis / 100.0
    }
}

/// A size or offset that may be `auto` (or `none` for max sizes).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Dim {
    #[default]
    Auto,
    Len(Len),
    /// `min-content` / `max-content` / `fit-content` (widths; behaves as
    /// `auto` wherever intrinsic sizing does not apply).
    Content(ContentSize),
}

/// Intrinsic size keywords.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContentSize {
    Min,
    Max,
    Fit,
}

impl Dim {
    pub fn px(px: f64) -> Dim {
        Dim::Len(Len::px(px))
    }
    pub fn resolve(self, basis: Option<f64>) -> Option<f64> {
        match self {
            Dim::Auto | Dim::Content(_) => None,
            Dim::Len(l) if l.percent != 0.0 => basis.map(|b| l.resolve(b)),
            Dim::Len(l) => Some(l.px),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum OuterDisplay {
    #[default]
    Block,
    Inline,
    /// The element generates no box; its children are laid out in its place.
    Contents,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum InnerDisplay {
    #[default]
    Flow,
    /// Establishes a new formatting context (inline-block, flow-root).
    FlowRoot,
    Flex,
    Grid,
    Table,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

/// `align-items` / `align-self` values the flex layout acts on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Stretch,
    Start,
    End,
    Center,
    /// Laid out as Start in Stage 2.
    Baseline,
}

/// `justify-content` values the flex layout acts on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Justify {
    #[default]
    Start,
    End,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Used `text-align` of a block container's line boxes, for left-to-right
/// text: `left`, `start`, `justify` (approximated) and `match-parent`
/// resolving to left are [`TextAlign::Start`]; `right` / `end` are
/// [`TextAlign::End`]; `center` (and `-webkit-center`) is
/// [`TextAlign::Center`]. Inherited, so the style source reports the
/// element's computed value.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

/// Table-internal display types (`display: table-row` etc.).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TablePart {
    #[default]
    None,
    HeaderGroup,
    BodyGroup,
    FooterGroup,
    Row,
    Cell,
    Caption,
    Column,
    ColumnGroup,
}

/// One end of a grid track's sizing function.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TrackBreadth {
    Len(Len),
    /// `<n>fr` (only meaningful as a max breadth).
    Fr(f64),
    Auto,
    MinContent,
    MaxContent,
    /// `min(a, b)` / `max(a, b)` of two lengths that only resolve against
    /// the grid's size (`min(100%, 20rem)`).
    Min(Len, Len),
    Max(Len, Len),
}

impl TrackBreadth {
    /// The breadth as a length against `available`; percentages need a
    /// definite available size. None for intrinsic and flexible breadths.
    pub fn length(self, available: Option<f64>) -> Option<f64> {
        let one = |l: Len| (l.percent == 0.0 || available.is_some()).then(|| l.resolve(available.unwrap_or(0.0)));
        match self {
            TrackBreadth::Len(l) => one(l),
            TrackBreadth::Min(a, b) => Some(one(a)?.min(one(b)?)),
            TrackBreadth::Max(a, b) => Some(one(a)?.max(one(b)?)),
            _ => None,
        }
    }
}

/// A grid track sizing function `minmax(min, max)`; a single size `s` is
/// `minmax(s, s)` except `<n>fr`, which is `minmax(auto, <n>fr)`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Track {
    pub min: TrackBreadth,
    pub max: TrackBreadth,
}

impl Track {
    pub const AUTO: Track = Track { min: TrackBreadth::Auto, max: TrackBreadth::Auto };
}

impl Default for Track {
    fn default() -> Self {
        Track::AUTO
    }
}

/// `repeat(auto-fill | auto-fit, <tracks>)` inside a track list.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoRepeat {
    /// Index in [`TrackList::tracks`] where the repetitions go.
    pub at: usize,
    /// `auto-fit`: repeated tracks without items collapse.
    pub fit: bool,
    pub tracks: Vec<Track>,
}

/// `grid-template-columns` / `grid-template-rows` (`none` = empty).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackList {
    pub tracks: Vec<Track>,
    pub auto_repeat: Option<AutoRepeat>,
}

/// `grid-column` / `grid-row` of an item: 1-based lines (negative from the
/// end of the explicit grid), `None` = auto; `span` applies when an end is
/// auto.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GridPlacement {
    pub start: Option<i32>,
    pub end: Option<i32>,
    pub span: u32,
}

impl Default for GridPlacement {
    fn default() -> Self {
        GridPlacement { start: None, end: None, span: 1 }
    }
}

/// Splits a value at top-level whitespace (not inside parentheses or brackets).
fn grid_tokens(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in v.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Splits `a, b` at the top-level comma.
fn grid_args(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in v.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur).trim().to_owned());
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur.trim().to_owned());
    out
}

fn parse_breadth(v: &str, len: &dyn Fn(&str) -> Option<Len>) -> Option<TrackBreadth> {
    let v = v.trim();
    Some(match v.to_ascii_lowercase().as_str() {
        "auto" => TrackBreadth::Auto,
        "min-content" => TrackBreadth::MinContent,
        "max-content" => TrackBreadth::MaxContent,
        l if l.ends_with("fr") => TrackBreadth::Fr(l.trim_end_matches("fr").trim().parse().ok()?),
        l if (l.starts_with("min(") || l.starts_with("max(")) && l.ends_with(')') => match len(v) {
            // The style engine folds comparable arguments itself.
            Some(l) => TrackBreadth::Len(l),
            None => {
                let args = grid_args(&v[4..v.len() - 1]);
                let [a, b] = args.as_slice() else { return None };
                let (a, b) = (len(a)?, len(b)?);
                if l.starts_with("min(") {
                    TrackBreadth::Min(a, b)
                } else {
                    TrackBreadth::Max(a, b)
                }
            }
        },
        _ => TrackBreadth::Len(len(v)?),
    })
}

/// Parses one track sizing function (`200px`, `1fr`, `minmax(0, 1fr)`,
/// `fit-content(10rem)`, `auto`...). `len` resolves a length or percentage.
pub fn parse_track(v: &str, len: &dyn Fn(&str) -> Option<Len>) -> Option<Track> {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    let inner = |prefix: &str| lower.starts_with(prefix).then(|| v[prefix.len()..v.len().saturating_sub(1)].to_owned());
    if let Some(args) = inner("minmax(") {
        let a = grid_args(&args);
        if a.len() != 2 {
            return None;
        }
        let min = match parse_breadth(&a[0], len)? {
            TrackBreadth::Fr(_) => TrackBreadth::Auto,
            b => b,
        };
        return Some(Track { min, max: parse_breadth(&a[1], len)? });
    }
    if let Some(arg) = inner("fit-content(") {
        // Approximation: grows with content up to the limit.
        return Some(Track { min: TrackBreadth::Auto, max: parse_breadth(&arg, len)? });
    }
    let b = parse_breadth(v, len)?;
    Some(match b {
        TrackBreadth::Fr(_) => Track { min: TrackBreadth::Auto, max: b },
        _ => Track { min: b, max: b },
    })
}

/// Parses `grid-template-columns` / `-rows` (`none`, line names ignored,
/// `repeat(<n> | auto-fill | auto-fit, ...)`). Unparseable lists are `none`.
pub fn parse_track_list(v: &str, len: &dyn Fn(&str) -> Option<Len>) -> TrackList {
    let mut list = TrackList::default();
    let v = v.trim();
    if v.eq_ignore_ascii_case("none") || v.is_empty() {
        return list;
    }
    for tok in grid_tokens(v) {
        if tok.starts_with('[') {
            continue;
        }
        if tok.to_ascii_lowercase().starts_with("repeat(") {
            let args = grid_args(&tok["repeat(".len()..tok.len().saturating_sub(1)]);
            if args.len() != 2 {
                return TrackList::default();
            }
            let mut tracks = Vec::new();
            for t in grid_tokens(&args[1]) {
                if t.starts_with('[') {
                    continue;
                }
                match parse_track(&t, len) {
                    Some(t) => tracks.push(t),
                    None => return TrackList::default(),
                }
            }
            match args[0].to_ascii_lowercase().as_str() {
                "auto-fill" | "auto-fit" => {
                    let fit = args[0].eq_ignore_ascii_case("auto-fit");
                    list.auto_repeat = Some(AutoRepeat { at: list.tracks.len(), fit, tracks });
                }
                n => {
                    let Ok(n) = n.parse::<usize>() else { return TrackList::default() };
                    for _ in 0..n.min(1000) {
                        list.tracks.extend_from_slice(&tracks);
                    }
                }
            }
            continue;
        }
        match parse_track(&tok, len) {
            Some(t) => list.tracks.push(t),
            None => return TrackList::default(),
        }
    }
    list
}

/// Parses `grid-column` / `grid-row` from the shorthand (`span 2 / span 2`,
/// `1 / -1`, `2`) or the `-start` / `-end` longhands (which win).
pub fn parse_grid_placement(shorthand: Option<&str>, start: Option<&str>, end: Option<&str>) -> GridPlacement {
    let (mut s, mut e) = (None::<String>, None::<String>);
    if let Some(v) = shorthand {
        let mut parts = v.splitn(2, '/');
        s = parts.next().map(|p| p.trim().to_owned());
        e = parts.next().map(|p| p.trim().to_owned());
    }
    if let Some(v) = start {
        s = Some(v.trim().to_owned());
    }
    if let Some(v) = end {
        e = Some(v.trim().to_owned());
    }
    let mut p = GridPlacement::default();
    // Each side is a line number or `span <n>`; a span on the start side
    // wins over one on the end side.
    let side = |v: Option<&str>| -> (Option<i32>, Option<u32>) {
        let Some(v) = v else { return (None, None) };
        let v = v.trim().to_ascii_lowercase();
        if let Some(n) = v.strip_prefix("span") {
            return (None, Some(n.trim().parse::<u32>().ok().filter(|n| *n > 0).unwrap_or(1)));
        }
        (v.parse::<i32>().ok().filter(|n| *n != 0), None)
    };
    let (start_line, start_span) = side(s.as_deref());
    let (end_line, end_span) = side(e.as_deref());
    p.start = start_line;
    p.end = end_line;
    if let Some(n) = start_span.or(end_span) {
        p.span = n;
    }
    p
}

/// top, right, bottom, left.
pub type Sides<T> = [T; 4];
pub const TOP: usize = 0;
pub const RIGHT: usize = 1;
pub const BOTTOM: usize = 2;
pub const LEFT: usize = 3;

/// Used style values of one rendered element.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowStyle {
    pub outer: OuterDisplay,
    pub inner: InnerDisplay,
    pub position: Position,
    pub inset: Sides<Dim>,
    pub width: Dim,
    pub height: Dim,
    pub min_width: Dim,
    pub max_width: Dim,
    pub min_height: Dim,
    pub max_height: Dim,
    pub margin: Sides<Dim>,
    pub padding: Sides<Len>,
    pub border: Sides<f64>,
    pub border_box: bool,
    pub clips_x: bool,
    pub clips_y: bool,
    pub font_size: f64,
    /// px; None = `normal`.
    pub line_height: Option<f64>,
    pub nowrap: bool,
    /// Where an otherwise unbreakable word may break when it overflows its
    /// line (overflow-wrap / word-break).
    pub wrap: WrapMode,
    /// Used `font-weight` (100..900; 400 normal, 700 bold). Text is measured
    /// with the system UI font's advances at this weight.
    pub font_weight: u16,
    /// Inherited `text-align` (aligns this element's own line boxes).
    pub text_align: TextAlign,
    /// `<img>`/`<iframe>`-like replaced element: atomic, sized by its own
    /// width/height only.
    pub replaced: bool,
    /// Natural size (px) of a loaded image (`<img>`), from its resource;
    /// None when unknown. Gives replaced sizing its intrinsic size and ratio.
    pub intrinsic: Option<(f64, f64)>,
    /// Effective CSS `zoom` (the element's times its ancestors'; 1 = none),
    /// already applied to the absolute lengths above.
    pub zoom: f64,
    // Flex container (Stage 2).
    pub flex_direction: FlexDirection,
    pub flex_wrap: bool,
    pub row_gap: Len,
    pub column_gap: Len,
    pub align_items: Align,
    pub justify_content: Justify,
    // Flex item.
    pub flex_grow: f64,
    pub flex_shrink: f64,
    pub flex_basis: Dim,
    /// `flex_basis` was a percentage (`flex: 1` gives `0%`): against an
    /// indefinite container main size it is `content`.
    pub flex_basis_percent: bool,
    /// None = `auto` (the container's align-items).
    pub align_self: Option<Align>,
    // Tables (Stage 3).
    pub table_part: TablePart,
    /// `border-spacing` (horizontal, vertical) in px; ignored when collapsed.
    pub border_spacing: (f64, f64),
    pub border_collapse: bool,
    pub table_layout_fixed: bool,
    // Grid container (Stage 4). Gaps reuse `row_gap` / `column_gap`.
    pub grid_template_columns: TrackList,
    pub grid_template_rows: TrackList,
    pub grid_auto_columns: Track,
    pub grid_auto_rows: Track,
    /// `grid-auto-flow: column` (else row; `dense` is not supported).
    pub grid_auto_flow_column: bool,
    /// `justify-items` (grid); `align-items` is shared with flex.
    pub justify_items: Align,
    // Grid item.
    pub grid_column: GridPlacement,
    pub grid_row: GridPlacement,
    /// None = `auto` (the container's justify-items).
    pub justify_self: Option<Align>,
}

impl Default for FlowStyle {
    fn default() -> Self {
        FlowStyle {
            outer: OuterDisplay::Block,
            inner: InnerDisplay::Flow,
            position: Position::Static,
            inset: [Dim::Auto; 4],
            width: Dim::Auto,
            height: Dim::Auto,
            min_width: Dim::Auto,
            max_width: Dim::Auto,
            min_height: Dim::Auto,
            max_height: Dim::Auto,
            margin: [Dim::px(0.0); 4],
            padding: [Len::ZERO; 4],
            border: [0.0; 4],
            border_box: false,
            clips_x: false,
            clips_y: false,
            font_size: 16.0,
            line_height: None,
            nowrap: false,
            wrap: WrapMode::Normal,
            font_weight: 400,
            text_align: TextAlign::Start,
            replaced: false,
            intrinsic: None,
            zoom: 1.0,
            flex_direction: FlexDirection::Row,
            flex_wrap: false,
            row_gap: Len::ZERO,
            column_gap: Len::ZERO,
            align_items: Align::Stretch,
            justify_content: Justify::Start,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dim::Auto,
            flex_basis_percent: false,
            align_self: None,
            table_part: TablePart::None,
            border_spacing: (0.0, 0.0),
            border_collapse: false,
            table_layout_fixed: false,
            grid_template_columns: TrackList::default(),
            grid_template_rows: TrackList::default(),
            grid_auto_columns: Track::AUTO,
            grid_auto_rows: Track::AUTO,
            grid_auto_flow_column: false,
            justify_items: Align::Stretch,
            grid_column: GridPlacement::default(),
            grid_row: GridPlacement::default(),
            justify_self: None,
        }
    }
}

/// Supplies used style values. `None`: the element is not rendered.
pub trait FlowStyleSource {
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle>;
    /// `el`'s generated `::before` (`after`: `::after`) content.
    fn generated(&self, _dom: &Dom, _el: NodeId, _after: bool) -> Option<std::rc::Rc<Generated>> {
        None
    }
}

/// Generated content of a `::before` / `::after` pseudo-element: its text
/// (strings only) and the box it lays out in, in its originating element's
/// font.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub text: std::rc::Rc<str>,
    pub display: GeneratedDisplay,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GeneratedDisplay {
    /// Text in the line, like the element's own text.
    Inline,
    /// An atomic box one line of its text wide; empty, it is a zero-size box
    /// that still makes a line (the element's strut).
    InlineBlock,
    /// Its own line(s) in a block container.
    Block,
}

// ----- output ---------------------------------------------------------------

/// One element's geometry. Rects are border boxes in document coordinates,
/// except boxes inside `position: fixed`, which are in viewport coordinates.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FlowBox {
    pub rect: Rect,
    /// Inside (or itself) a `position: fixed` box.
    pub fixed: bool,
    /// Padding box size (clientWidth/Height before scrollbars).
    pub client_width: f64,
    pub client_height: f64,
    /// Padding box plus overflowing descendants (scrollWidth/Height).
    pub scroll_width: f64,
    pub scroll_height: f64,
    /// Positioned (relative/absolute/fixed/sticky): painted above in-flow boxes.
    pub positioned: bool,
    /// Tree order, for paint order among equals.
    pub order: u32,
    /// `order` of the nearest ancestor clipping this box's overflow (0: none).
    pub clipped_by: u32,
}

/// One word of a text node on a line.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub node: NodeId,
    /// UTF-16 offsets of the word in the node's data.
    pub start: u32,
    pub end: u32,
    pub rect: Rect,
    pub fixed: bool,
    text: std::rc::Rc<str>,
    font_size: f64,
    weight: u16,
    order: u32,
}

impl TextRun {
    /// The offset of the character boundary nearest to `dx` px from the
    /// run's left edge, as Chromium places a caret under a point.
    pub fn offset_at(&self, dx: f64) -> u32 {
        let mut x = 0.0;
        let mut offset = self.start;
        for c in self.text.chars() {
            let w = text_width(c.encode_utf8(&mut [0; 4]), self.font_size, self.weight);
            if dx < x + w / 2.0 {
                return offset;
            }
            x += w;
            offset += c.len_utf16() as u32;
        }
        self.end
    }
}

/// Where a word comes from in its text node.
#[derive(Clone, Debug)]
struct WordSource {
    node: NodeId,
    start: u32,
    end: u32,
    text: std::rc::Rc<str>,
    font_size: f64,
    weight: u16,
}

#[derive(Default, Debug)]
pub struct FlowResult {
    pub boxes: FxHashMap<NodeId, FlowBox>,
    /// Text nodes laid out in line boxes: the union of their fragments
    /// (document coordinates, or viewport ones inside `position: fixed`),
    /// and whether they are inside a fixed box. Range geometry uses it.
    pub texts: FxHashMap<NodeId, (Rect, bool)>,
    /// Order stamp of each text entry, so later shifts of the boxes laid out
    /// after an order (cell centring, bottom-anchored abs boxes) move them.
    text_orders: Vec<(u32, NodeId)>,
    /// Each laid-out word of a text node, for positions within text (the
    /// caret under a point). Same coordinates as `texts`.
    pub text_runs: Vec<TextRun>,
    /// Document scroll size (at least the viewport).
    pub document_width: f64,
    pub document_height: f64,
    /// Boxes of `position: sticky` elements, laid out as if static; their
    /// scroll-dependent offset is applied when geometry is read
    /// (`Layout::sticky_shift`).
    pub sticky: rustc_hash::FxHashSet<NodeId>,
    /// Per order (index = order - 1): the element recorded under it and its
    /// positioning facts, so a later pass can replay a subtree (`reuse`).
    slots: Vec<Slot>,
}

/// What a layout order was recorded for.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    el: Option<NodeId>,
    /// The box is positioned itself (not only inside a positioned box).
    own_positioned: bool,
    /// `order` of the nearest positioned ancestor box (0: none).
    positioned_by: u32,
}

impl FlowResult {
    pub fn get(&self, el: NodeId) -> Option<&FlowBox> {
        self.boxes.get(&el)
    }
}

/// The first difference between two layouts of the same document (boxes,
/// texts, text runs, sticky boxes, document size), positions compared to
/// 1e-6 px; None when they agree.
pub fn difference(want: &FlowResult, got: &FlowResult) -> Option<String> {
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(1.0);
    let same_rect =
        |a: &Rect, b: &Rect| near(a.x, b.x) && near(a.y, b.y) && near(a.width, b.width) && near(a.height, b.height);
    if want.boxes.len() != got.boxes.len() {
        return Some(format!("{} boxes, want {}", got.boxes.len(), want.boxes.len()));
    }
    for (el, w) in &want.boxes {
        let Some(g) = got.boxes.get(el) else { return Some(format!("{el:?}: no box")) };
        let same = same_rect(&w.rect, &g.rect)
            && w.fixed == g.fixed
            && near(w.client_width, g.client_width)
            && near(w.client_height, g.client_height)
            && near(w.scroll_width, g.scroll_width)
            && near(w.scroll_height, g.scroll_height)
            && w.positioned == g.positioned
            && w.order == g.order
            && w.clipped_by == g.clipped_by;
        if !same {
            return Some(format!("{el:?}: got {g:?}, want {w:?}"));
        }
    }
    if want.texts.len() != got.texts.len() {
        return Some(format!("{} texts, want {}", got.texts.len(), want.texts.len()));
    }
    for (t, (w, wf)) in &want.texts {
        match got.texts.get(t) {
            Some((g, gf)) if same_rect(w, g) && wf == gf => {}
            other => return Some(format!("text {t:?}: got {other:?}, want {:?}", (w, wf))),
        }
    }
    if want.text_runs.len() != got.text_runs.len() {
        return Some(format!("{} text runs, want {}", got.text_runs.len(), want.text_runs.len()));
    }
    for (w, g) in want.text_runs.iter().zip(&got.text_runs) {
        if w.node != g.node
            || w.start != g.start
            || w.end != g.end
            || !same_rect(&w.rect, &g.rect)
            || w.order != g.order
        {
            return Some(format!("text run: got {g:?}, want {w:?}"));
        }
    }
    if want.sticky != got.sticky {
        return Some("sticky boxes differ".into());
    }
    if !near(want.document_width, got.document_width) || !near(want.document_height, got.document_height) {
        return Some(format!(
            "document {}x{}, want {}x{}",
            got.document_width, got.document_height, want.document_width, want.document_height
        ));
    }
    None
}

// ----- layout ---------------------------------------------------------------

/// Measurements of element subtrees kept across layout passes: intrinsic
/// widths, content heights at a width, atomic baselines. Each is a function
/// of the element's subtree (DOM, styles), the width (key) and the viewport
/// only (measuring probes start from a fresh context), so the owner drops
/// the entries of every changed element and its ancestors before a pass
/// (`invalidate`), or all of them (`clear`).
#[derive(Default)]
pub struct MeasureCache {
    max_content: FxHashMap<NodeId, f64>,
    min_content: FxHashMap<NodeId, f64>,
    heights: FxHashMap<(NodeId, u64), f64>,
    baselines: FxHashMap<(NodeId, u64, u64), Option<f64>>,
    /// Laid-out subtrees of the last pass, replayed by the next one.
    reuse: reuse::Reuse,
    /// The flow styles the last pass used, which seed the next pass (minus
    /// those of restyled elements: `layout_style_unchanged`).
    styles: FxHashMap<NodeId, Option<StyleRef>>,
    generated: FxHashMap<(NodeId, bool), Option<std::rc::Rc<Generated>>>,
}

/// Attributes the flow layout reads itself (form control and replaced sizes,
/// table spans); other attributes reach it only through restyles. Hashes of
/// their local names, as `Change::Attr` carries them.
pub fn layout_attribute(hash: u64) -> bool {
    static NAMES: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    // 0: state no attribute names, such as an image's natural size.
    hash == 0
        || NAMES
            .get_or_init(|| {
                [
                    "type",
                    "value",
                    "size",
                    "cols",
                    "rows",
                    "width",
                    "height",
                    "multiple",
                    "colspan",
                    "rowspan",
                    "span",
                    "placeholder",
                ]
                .iter()
                .map(|n| crate::dom::attr_name_hash(n))
                .collect()
            })
            .contains(&hash)
}

impl MeasureCache {
    pub fn clear(&mut self) {
        self.max_content.clear();
        self.min_content.clear();
        self.heights.clear();
        self.baselines.clear();
        self.reuse.clear();
        self.styles.clear();
        self.generated.clear();
    }

    /// Whether a restyled node lays out as the last pass laid it out: it is
    /// not an element, the last pass did not lay it out (it could only
    /// start to through a change of an ancestor or of the tree), or its
    /// flow style and generated content are those that pass used. Otherwise
    /// forgets them so the next pass asks again.
    pub fn layout_style_unchanged(&mut self, dom: &Dom, src: &dyn FlowStyleSource, el: NodeId) -> bool {
        if dom.node_type(el).ok() != Some(NodeType::Element) {
            return true;
        }
        if !self.reuse.has_previous() {
            return false;
        }
        let Some(old) = self.styles.get(&el) else { return true };
        let same_generated = |after: bool| match self.generated.get(&(el, after)) {
            // Not asked: the last pass did not lay out its contents.
            None => true,
            Some(old) => *old == src.generated(dom, el, after),
        };
        let unchanged = match (old, used_flow_style(dom, src, el)) {
            (None, None) => true,
            (Some(old), Some(new)) => **old == new,
            _ => false,
        } && same_generated(false)
            && same_generated(true);
        if !unchanged {
            self.forget_style(el);
        }
        unchanged
    }

    /// Forgets the flow style of an element whose layout attributes changed.
    pub fn forget_style(&mut self, el: NodeId) {
        self.styles.remove(&el);
        self.generated.remove(&(el, false));
        self.generated.remove(&(el, true));
    }

    /// Keeps the result of the pass just run, so the next pass can copy the
    /// subtrees it would lay out the same way from it instead. Without this
    /// call, the next pass lays every subtree out again.
    pub fn keep_result(&mut self, result: std::rc::Rc<FlowResult>) {
        self.reuse.keep(result);
    }

    pub fn invalidate(&mut self, dirty: &rustc_hash::FxHashSet<NodeId>) {
        if dirty.is_empty() {
            return;
        }
        self.max_content.retain(|k, _| !dirty.contains(k));
        self.min_content.retain(|k, _| !dirty.contains(k));
        self.heights.retain(|k, _| !dirty.contains(&k.0));
        self.baselines.retain(|k, _| !dirty.contains(&k.0));
        self.reuse.invalidate(dirty);
    }

    pub fn len(&self) -> usize {
        self.max_content.len() + self.min_content.len() + self.heights.len() + self.baselines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Lays out the document rooted at `doc` for a `vw`×`vh` viewport.
pub fn layout(dom: &Dom, doc: NodeId, src: &dyn FlowStyleSource, vw: f64, vh: f64) -> FlowResult {
    layout_cached(dom, doc, src, vw, vh, &mut MeasureCache::default())
}

/// `layout`, reusing and refilling subtree measurements from `cache`
/// (valid for this DOM, styles and viewport: see `MeasureCache`).
pub fn layout_cached(
    dom: &Dom,
    doc: NodeId,
    src: &dyn FlowStyleSource,
    vw: f64,
    vh: f64,
    cache: &mut MeasureCache,
) -> FlowResult {
    let memo = Memo {
        max_content: RefCell::new(std::mem::take(&mut cache.max_content)),
        min_content: RefCell::new(std::mem::take(&mut cache.min_content)),
        heights: RefCell::new(std::mem::take(&mut cache.heights)),
        baselines: RefCell::new(std::mem::take(&mut cache.baselines)),
        reuse: RefCell::new(std::mem::take(&mut cache.reuse)),
        styles: RefCell::new(std::mem::take(&mut cache.styles)),
        generated: RefCell::new(std::mem::take(&mut cache.generated)),
        ..Memo::default()
    };
    memo.reuse.borrow_mut().begin_pass();
    let out = layout_with_memo(dom, doc, src, vw, vh, &memo);
    cache.max_content = memo.max_content.into_inner();
    cache.min_content = memo.min_content.into_inner();
    cache.heights = memo.heights.into_inner();
    cache.baselines = memo.baselines.into_inner();
    cache.reuse = memo.reuse.into_inner();
    cache.styles = memo.styles.into_inner();
    cache.generated = memo.generated.into_inner();
    cache.reuse.end_pass(memo.relaid.get());
    out
}

fn layout_with_memo(dom: &Dom, doc: NodeId, src: &dyn FlowStyleSource, vw: f64, vh: f64, memo: &Memo) -> FlowResult {
    let mut cx = Ctx::new(dom, src, vw, vh, memo);
    cx.cb_height = Some(vh);
    let icb = Rect { x: 0.0, y: 0.0, width: vw, height: vh };
    cx.pending.push(Vec::new());
    let mut cursor = 0.0;
    if let Ok(children) = dom.children(doc) {
        let kids: Vec<NodeId> = children.collect();
        for c in kids {
            if dom.node_type(c).ok() == Some(NodeType::Element) {
                if let Some(style) = cx.style_of(c) {
                    let h = cx.block_child(c, &style, 0.0, cursor, vw, false, (0.0, 0.0));
                    cursor += h;
                }
            }
        }
    }
    let pending = cx.pending.pop().unwrap_or_default();
    cx.place_absolute(pending, icb, false);
    let (mut w, mut h) = (vw, vh);
    for b in cx.out.boxes.values().filter(|b| !b.fixed && b.clipped_by == 0) {
        w = w.max(b.rect.x + b.rect.width);
        h = h.max(b.rect.y + b.rect.height);
    }
    cx.out.document_width = w.ceil();
    cx.out.document_height = h.ceil();
    std::mem::take(&mut cx.out)
}

struct Ctx<'a> {
    dom: &'a Dom,
    src: &'a dyn FlowStyleSource,
    vw: f64,
    vh: f64,
    out: FlowResult,
    order: u32,
    /// Shared by the pass and its measuring probes.
    memo: &'a Memo,
    /// Definite content-box height of the containing block, against which
    /// in-flow percentage heights resolve (None: they behave as auto).
    cb_height: Option<f64>,
    /// `cb_height` of the box whose contents are being laid out (for its own
    /// percentage min/max heights).
    own_cb_height: Option<f64>,
    /// Baselines (y) of the first and last non-empty line boxes laid out by
    /// this context, for atomic inline baselines.
    first_baseline: Option<f64>,
    last_baseline: Option<f64>,
    /// Absolutely positioned descendants waiting for their containing block:
    /// one list per open positioned ancestor (bottom = initial containing
    /// block). Each entry: element, style, static position, fixed context.
    pending: Vec<Vec<Pending>>,
    /// A measuring probe: its buffers go back to the memo when dropped.
    probe: bool,
    /// A probe whose boxes nobody reads (only the height it returns):
    /// replayed subtrees advance the order without copying their boxes.
    measure_only: bool,
    /// Set by a block container right before laying out a block-level
    /// child in its flow (taken by `block_child`): the child's margins may
    /// collapse with its own children's.
    block_flow_child: bool,
    /// The margin a block container's contents leave below their last
    /// in-flow child when it collapses with the container's bottom margin
    /// (set by `contents`).
    trailing: Margin,
    /// The bottom margin `block_child` leaves for the next sibling: its own,
    /// collapsed with its contents' `trailing`.
    last_trailing: Margin,
}

impl Drop for Ctx<'_> {
    fn drop(&mut self) {
        if self.probe {
            let buffers =
                ProbeBuffers { out: std::mem::take(&mut self.out), pending: std::mem::take(&mut self.pending) };
            self.memo.probe_buffers.borrow_mut().push(buffers);
        }
    }
}

/// Per-pass caches. Nested flex/table measurement would otherwise lay the
/// same subtree out again at every ancestor level (exponential in depth),
/// and ask the style source for the same element many times.
/// A pass's shared view of an element's flow style (looked up many times per
/// pass; cloning the struct on every lookup dominated layout time).
type StyleRef = std::rc::Rc<FlowStyle>;

/// The inline elements (and text node) owning an inline item, outermost
/// first; shared by all items of the same node.
type Owners = std::rc::Rc<[NodeId]>;

#[derive(Default)]
struct Memo {
    styles: RefCell<FxHashMap<NodeId, Option<StyleRef>>>,
    max_content: RefCell<FxHashMap<NodeId, f64>>,
    min_content: RefCell<FxHashMap<NodeId, f64>>,
    /// Content height by (element, content width bits).
    heights: RefCell<FxHashMap<(NodeId, u64), f64>>,
    /// Atomic inline baseline by (element, content width bits, height bits).
    baselines: RefCell<FxHashMap<(NodeId, u64, u64), Option<f64>>>,
    /// Buffers of finished measuring probes, reused (cleared, capacity kept)
    /// by the next probe: probes lay whole subtrees out and are discarded.
    probe_buffers: RefCell<Vec<ProbeBuffers>>,
    /// Per element: its DOM depth and the depth of its nearest (inclusive)
    /// ancestor whose style clips overflow. Per pass only (the DOM and the
    /// memoized `style_of` answers are fixed for the pass and its probes).
    clip_facts: RefCell<FxHashMap<NodeId, ClipFact>>,
    /// Scratch path for `clip_fact` (reused, so a miss allocates nothing).
    clip_path: RefCell<Vec<NodeId>>,
    /// Generated content per (element, after), kept across passes like
    /// `styles`.
    generated: RefCell<FxHashMap<(NodeId, bool), Option<std::rc::Rc<Generated>>>>,
    /// `leading_margin` per (element, containing block width).
    leading: RefCell<FxHashMap<(NodeId, u64), Margin>>,
    /// `table_rows` per table.
    tables: RefCell<FxHashMap<NodeId, std::rc::Rc<(Vec<TableRow>, usize)>>>,
    /// Subtrees of the previous pass and of this one (`reuse`).
    reuse: RefCell<reuse::Reuse>,
    /// The pass recorded some element twice: its order ranges are not
    /// subtrees, so none of them is kept for the next pass.
    relaid: std::cell::Cell<bool>,
}

/// DOM depth of an element (document element = 1) and the depth of its
/// nearest inclusive clipping ancestor (`NO_CLIP` when none).
#[derive(Clone, Copy)]
struct ClipFact {
    depth: u32,
    clip_depth: u32,
}

const NO_CLIP: u32 = 0;

/// Work counters for the text scroll-size scan (debug builds only; read by
/// the in-crate benchmark). Not compiled into release binaries.
#[cfg(debug_assertions)]
pub mod record_scan_stats {
    use std::sync::atomic::AtomicU64;
    /// Ancestor steps of the reference walk (`clipped_inside_walk`).
    pub static WALK_STEPS: AtomicU64 = AtomicU64::new(0);
    /// Elements whose clip fact was computed (one path node each).
    pub static FACT_STEPS: AtomicU64 = AtomicU64::new(0);
    /// Text-per-box pairs examined.
    pub static PAIRS: AtomicU64 = AtomicU64::new(0);
}

#[derive(Default)]
struct ProbeBuffers {
    out: FlowResult,
    pending: Vec<Vec<Pending>>,
}

struct Pending {
    el: NodeId,
    style: StyleRef,
    static_x: f64,
    static_y: f64,
}

/// Adjoining vertical margins (CSS 2.1 8.3.1): the largest positive and
/// the most negative of them; they collapse to the sum of the two.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Margin {
    pos: f64,
    neg: f64,
}

impl Margin {
    const ZERO: Margin = Margin { pos: 0.0, neg: 0.0 };

    fn of(m: f64) -> Margin {
        Margin { pos: m.max(0.0), neg: m.min(0.0) }
    }

    fn with(self, o: Margin) -> Margin {
        Margin { pos: self.pos.max(o.pos), neg: self.neg.min(o.neg) }
    }

    fn value(self) -> f64 {
        self.pos + self.neg
    }
}

/// Whether a block container's margins collapse with those of its first
/// (top) and last (bottom) in-flow children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Collapse {
    top: bool,
    bottom: bool,
}

/// A box that lays its contents out in their own block formatting context:
/// its margins never collapse with its children's.
fn establishes_bfc(style: &FlowStyle) -> bool {
    style.inner != InnerDisplay::Flow
        || style.outer != OuterDisplay::Block
        || style.clips_x
        || style.clips_y
        || style.replaced
        || matches!(style.position, Position::Absolute | Position::Fixed)
}

/// A laid-out box's used edges.
struct Edges {
    margin: Sides<f64>,
    padding: Sides<f64>,
    border: Sides<f64>,
}

impl Edges {
    fn horizontal(&self) -> f64 {
        self.padding[LEFT] + self.padding[RIGHT] + self.border[LEFT] + self.border[RIGHT]
    }
    fn vertical(&self) -> f64 {
        self.padding[TOP] + self.padding[BOTTOM] + self.border[TOP] + self.border[BOTTOM]
    }
}

/// `overflow-wrap` / `word-break` for words longer than their line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrapMode {
    /// Long words overflow (`overflow-wrap: normal`).
    #[default]
    Normal,
    /// `overflow-wrap: break-word`: break a word that cannot fit on an empty
    /// line; min-content sizes still keep whole words.
    BreakWord,
    /// `overflow-wrap: anywhere`, `word-break: break-all | break-word`: as
    /// BreakWord, and min-content sizes allow a break between any characters.
    Anywhere,
}

/// An item of an inline formatting context.
enum InlineItem {
    Word {
        width: f64,
        height: f64,
        space_before: f64,
        owners: Owners,
        ascent: f64,
        /// The font's (ascent, descent) without leading: the content area
        /// that text and inline element boxes report.
        content: (f64, f64),
        /// The word, its font size and weight, when it may be broken if it
        /// cannot fit on an empty line (None: it overflows instead).
        breakable: Option<(std::rc::Rc<str>, f64, u16)>,
        /// `white-space` of the element containing the word forbids a soft
        /// wrap before it (nowrap / pre). The containing inline element
        /// decides, not the block (a whitespace-normal span in a nowrap cell
        /// wraps).
        nowrap: bool,
        /// The text node and range the word comes from.
        source: Option<WordSource>,
    },
    Break,
    Atomic {
        el: NodeId,
        style: StyleRef,
        owners: Owners,
    },
    /// A block-level box inside inline content (block-in-inline): it ends the
    /// current line and takes its own margin-box height, without a strut.
    Block {
        el: NodeId,
        style: StyleRef,
        owners: Owners,
    },
    /// An absolutely/fixed positioned box inside inline content: placed
    /// later against its containing block, from the current line position.
    OutOfFlow {
        el: NodeId,
        style: StyleRef,
    },
    /// A generated `::before` / `::after` inline-block box (no element).
    Generated {
        width: f64,
        height: f64,
        ascent: f64,
        owners: Owners,
    },
    /// An inline element that starts or ends (for empty inline boxes).
    Empty {
        owners: Owners,
        height: f64,
        ascent: f64,
        /// No inline-axis margin, border or padding: alone on a line it
        /// makes no line box (CSS 2.1 9.4.2).
        phantom: bool,
    },
}

impl<'a> Ctx<'a> {
    fn new(dom: &'a Dom, src: &'a dyn FlowStyleSource, vw: f64, vh: f64, memo: &'a Memo) -> Self {
        Ctx {
            dom,
            src,
            vw,
            vh,
            out: FlowResult::default(),
            order: 0,
            pending: Vec::new(),
            memo,
            cb_height: None,
            own_cb_height: None,
            first_baseline: None,
            last_baseline: None,
            probe: false,
            measure_only: false,
            block_flow_child: false,
            trailing: Margin::ZERO,
            last_trailing: Margin::ZERO,
        }
    }

    /// A context for measuring a subtree (its boxes are discarded), on
    /// reused buffers.
    fn probe(&self) -> Ctx<'a> {
        let mut cx = Ctx::new(self.dom, self.src, self.vw, self.vh, self.memo);
        cx.probe = true;
        if let Some(mut b) = self.memo.probe_buffers.borrow_mut().pop() {
            b.out.boxes.clear();
            b.out.texts.clear();
            b.out.sticky.clear();
            b.out.text_orders.clear();
            b.out.text_runs.clear();
            b.out.slots.clear();
            b.out.document_width = 0.0;
            b.out.document_height = 0.0;
            b.pending.clear();
            cx.out = b.out;
            cx.pending = b.pending;
        }
        cx
    }

    fn style_of(&self, el: NodeId) -> Option<StyleRef> {
        if let Some(s) = self.memo.styles.borrow().get(&el) {
            return s.clone();
        }
        let s = used_flow_style(self.dom, self.src, el).map(StyleRef::new);
        self.memo.styles.borrow_mut().insert(el, s.clone());
        s
    }

    /// Height of a textarea's value laid out in `content_w` (lines of the
    /// control's font, hard breaks kept, long lines wrapped), `None` for any
    /// other element. At least one line, like Chromium's empty textarea.
    fn textarea_text_height(&self, el: NodeId, content_w: f64) -> Option<f64> {
        if !matches!(control_kind(self.dom, el), Some(Control::TextArea)) {
            return None;
        }
        let style = self.style_of(el)?;
        let (a, d) = text_extents(&style);
        let line = a + d;
        let value = self.dom.value(el).unwrap_or_default();
        let width = content_w.max(1.0);
        let lines: f64 = value
            .split('\n')
            .map(|seg| (crate::layout::text_width(seg, style.font_size) / width).ceil().max(1.0))
            .sum();
        Some(lines.max(1.0) * line)
    }

    /// Width of a text control's value on one line (a text input, or a
    /// textarea's longest line), for `scrollWidth`: Chromium reports the
    /// text's width when the value is longer than the box, which truncation
    /// detectors (`offsetWidth < scrollWidth`) rely on. `None` for others.
    fn text_control_text_width(&self, el: NodeId) -> Option<f64> {
        if !matches!(control_kind(self.dom, el), Some(Control::Text | Control::TextArea)) {
            return None;
        }
        let style = self.style_of(el)?;
        let value = self.dom.value(el).unwrap_or_default();
        Some(value.split('\n').map(|seg| crate::layout::text_width(seg, style.font_size)).fold(0.0, f64::max))
    }

    /// Intrinsic content-box size of a replaced element without a declared
    /// size: form controls by Chromium's defaults (measured in headless
    /// Chromium at 13.33px UA and 16px preflight fonts), otherwise none.
    fn intrinsic(&self, el: NodeId, style: &FlowStyle) -> Option<(f64, f64)> {
        let fs = style.font_size;
        let normal = {
            let (a, d) = font_metrics(fs);
            a + d
        };
        let line = style.line_height.unwrap_or(normal);
        let attr_num = |name: &str| {
            self.dom
                .get_attribute(el, name)
                .ok()
                .flatten()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| *v > 0.0)
        };
        // Average glyph advance of the control font.
        let avg = 0.525 * fs;
        let Some(kind) = control_kind(self.dom, el) else {
            return self.replaced_intrinsic(el, style);
        };
        Some(match kind {
            Control::Text => (attr_num("size").unwrap_or(20.0) * avg + 0.4375 * fs, line),
            Control::Check => (13.0, 13.0),
            Control::Range => (129.0, 16.0),
            Control::Color => (44.0, 23.0),
            Control::File => (19.0 * fs, line),
            Control::Button(label) => (collapsed_width(&label, fs, style.font_weight), line),
            Control::TextArea => {
                (attr_num("cols").unwrap_or(20.0) * avg + 16.0, attr_num("rows").unwrap_or(2.0) * line)
            }
            Control::Select { rows } => {
                let widest = self
                    .dom
                    .descendants(el)
                    .into_iter()
                    .flatten()
                    .filter(|&n| local_tag(self.dom, n).is_some_and(|t| t.eq_ignore_ascii_case("option")))
                    .map(|n| {
                        collapsed_width(
                            &self.dom.text_content(n).ok().flatten().unwrap_or_default(),
                            fs,
                            style.font_weight,
                        )
                    })
                    .fold(0.0, f64::max);
                match rows {
                    None => (widest + 24.0, normal + 2.0),
                    Some(r) => (widest + 16.0, r * (normal + 2.0)),
                }
            }
        })
    }

    /// Size of a non-control replaced element from its `width`/`height`
    /// attributes (lucide icons: `<svg width="15" height="15">`), else the
    /// CSS default object size (300x150) for svg/canvas/video/iframe/embed/
    /// object; images without attributes have no known natural size.
    fn replaced_intrinsic(&self, el: NodeId, style: &FlowStyle) -> Option<(f64, f64)> {
        let tag = local_tag(self.dom, el)?;
        let is = |name: &str| tag.eq_ignore_ascii_case(name);
        let default = if ["svg", "canvas", "video", "iframe", "embed", "object"].into_iter().any(is) {
            (300.0, 150.0)
        } else if is("img") {
            // A loaded image's natural size, else nothing known.
            style.intrinsic.unwrap_or((0.0, 0.0))
        } else {
            return None;
        };
        let attr = |name: &str| {
            self.dom
                .get_attribute(el, name)
                .ok()
                .flatten()
                .map(|v| v.trim().trim_end_matches("px").parse::<f64>().ok().filter(|v| *v >= 0.0).unwrap_or(0.0))
        };
        let ratio = (default.0 > 0.0 && default.1 > 0.0).then(|| default.0 / default.1);
        Some(match (attr("width"), attr("height")) {
            (Some(w), Some(h)) => (w, h),
            // One attribute: the other follows the natural ratio when known.
            (Some(w), None) => (w, ratio.map_or(default.1, |r| w / r)),
            (None, Some(h)) => (ratio.map_or(default.0, |r| h * r), h),
            (None, None) => default,
        })
    }

    /// Intrinsic aspect ratio (width / height) of a replaced element that has
    /// one (images with a natural size, svg/canvas/video defaults, width and
    /// height attributes); form controls have none.
    fn replaced_ratio(&self, el: NodeId, style: &FlowStyle) -> Option<f64> {
        if control_kind(self.dom, el).is_some() {
            return None;
        }
        let (w, h) = self.replaced_intrinsic(el, style)?;
        (w > 0.0 && h > 0.0).then(|| w / h)
    }

    /// Content width of a replaced element whose width is auto (CSS 2.1
    /// 10.3.2 and 10.4): from a definite height and the ratio, else the
    /// intrinsic width; with a ratio, min/max-height clamps keep it.
    fn replaced_auto_width(&self, el: NodeId, style: &FlowStyle) -> Option<f64> {
        let (w, _) = self.intrinsic(el, style)?;
        let Some(ratio) = self.replaced_ratio(el, style) else { return Some(w) };
        let e = self.edges(style, 0.0);
        let to_content = |h: f64| if style.border_box { (h - e.vertical()).max(0.0) } else { h };
        if let Some(h) = style.height.resolve(self.cb_height).map(to_content) {
            return Some(h * ratio);
        }
        let mut h = w / ratio;
        if let Some(max) = style.max_height.resolve(self.cb_height).map(to_content) {
            h = h.min(max);
        }
        if let Some(min) = style.min_height.resolve(self.cb_height).map(to_content) {
            h = h.max(min);
        }
        Some(h * ratio)
    }

    fn insert_box(&mut self, el: NodeId, b: FlowBox) {
        let sticky = self
            .memo
            .styles
            .borrow()
            .get(&el)
            .is_some_and(|s| s.as_ref().is_some_and(|s| s.position == Position::Sticky));
        if sticky {
            self.out.sticky.insert(el);
        }
        let slot = Slot { el: Some(el), own_positioned: b.positioned, positioned_by: 0 };
        self.put_box(el, b, slot);
    }

    /// Records `b` under its order without looking at styles.
    fn put_box(&mut self, el: NodeId, b: FlowBox, slot: Slot) {
        let i = b.order as usize - 1;
        if self.out.slots.len() <= i {
            self.out.slots.resize(i + 1, Slot::default());
        }
        self.out.slots[i] = slot;
        if self.out.boxes.insert(el, b).is_some() && !self.probe {
            self.memo.relaid.set(true);
        }
    }

    /// Moves text entries recorded after `order` down by `dy`.
    /// Both lists are in order (orders only grow while they are appended),
    /// so the entries after `order` are a tail: shifting every table cell
    /// stays linear in the cell's own text, not the document's.
    fn shift_texts_after(&mut self, order: u32, dy: f64) {
        let first = self.out.text_orders.partition_point(|(o, _)| *o < order);
        for (_, t) in &self.out.text_orders[first..] {
            if let Some((r, _)) = self.out.texts.get_mut(t) {
                r.y += dy;
            }
        }
        let first = self.out.text_runs.partition_point(|r| r.order < order);
        for run in &mut self.out.text_runs[first..] {
            run.rect.y += dy;
        }
    }

    /// Calls `f` on every box recorded with an order above `order`.
    fn boxes_after(&mut self, order: u32, mut f: impl FnMut(&mut FlowBox)) {
        for i in order as usize..self.out.slots.len() {
            let Some(el) = self.out.slots[i].el else { continue };
            if let Some(b) = self.out.boxes.get_mut(&el).filter(|b| b.order as usize == i + 1) {
                f(b);
            }
        }
    }

    fn edges(&self, style: &FlowStyle, cb_width: f64) -> Edges {
        let m = |d: Dim| d.resolve(Some(cb_width)).unwrap_or(0.0);
        Edges {
            margin: [m(style.margin[0]), m(style.margin[1]), m(style.margin[2]), m(style.margin[3])],
            padding: style.padding.map(|p| p.resolve(cb_width).max(0.0)),
            border: style.border.map(|b| b.max(0.0)),
        }
    }

    fn clamp(v: f64, min: Option<f64>, max: Option<f64>) -> f64 {
        let v = max.map_or(v, |m| v.min(m));
        min.map_or(v, |m| v.max(m)).max(0.0)
    }

    /// Content-box width from a declared width (box-sizing aware).
    fn declared_content(style: &FlowStyle, declared: f64, edges_h: f64) -> f64 {
        if style.border_box {
            (declared - edges_h).max(0.0)
        } else {
            declared
        }
    }

    fn relative_offset(&self, style: &FlowStyle, cb_width: f64, cb_height: Option<f64>) -> (f64, f64) {
        // Sticky boxes lay out in place; their offset depends on scrolling
        // and is applied when geometry is read (`FlowResult::sticky`).
        if style.position != Position::Relative {
            return (0.0, 0.0);
        }
        let dx = match (style.inset[LEFT].resolve(Some(cb_width)), style.inset[RIGHT].resolve(Some(cb_width))) {
            (Some(l), _) => l,
            (None, Some(r)) => -r,
            _ => 0.0,
        };
        let dy = match (style.inset[TOP].resolve(cb_height), style.inset[BOTTOM].resolve(cb_height)) {
            (Some(t), _) => t,
            (None, Some(b)) => -b,
            _ => 0.0,
        };
        (dx, dy)
    }

    /// Lays out an in-flow block-level element at (x, y) (its margin edge)
    /// inside a containing block of width `cb_width`. Returns the vertical
    /// space it takes in its parent (margin box height, before collapsing).
    #[allow(clippy::too_many_arguments)]
    fn block_child(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        cb_width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let e = self.edges(style, cb_width);
        let declared = style.width.resolve(Some(cb_width));
        let mut auto_width = (cb_width - e.margin[LEFT] - e.margin[RIGHT] - e.horizontal()).max(0.0);
        if style.inner == InnerDisplay::Table && declared.is_none() {
            // Tables shrink to their columns.
            auto_width = self.table_max_content(el, style).min(auto_width);
        }
        if let Dim::Content(_) = style.width {
            // width: min-/max-/fit-content on a block box.
            auto_width = self.intrinsic_width(el, style, auto_width);
        }
        if style.replaced {
            // Replaced boxes keep their intrinsic width instead of filling.
            if let Some(w) = self.replaced_auto_width(el, style) {
                auto_width = w;
            }
        }
        let mut content_w = declared.map_or(auto_width, |w| Self::declared_content(style, w, e.horizontal()));
        let min_w = style.min_width.resolve(Some(cb_width)).map(|w| Self::declared_content(style, w, e.horizontal()));
        let max_w = style.max_width.resolve(Some(cb_width)).map(|w| Self::declared_content(style, w, e.horizontal()));
        content_w = Self::clamp(content_w, min_w, max_w);
        // Auto horizontal margins centre a box narrower than its containing block.
        let mut margin_left = e.margin[LEFT];
        let used = content_w + e.horizontal();
        let free = cb_width - used - e.margin[LEFT] - e.margin[RIGHT];
        match (style.margin[LEFT], style.margin[RIGHT]) {
            (Dim::Auto, Dim::Auto) if free > 0.0 => margin_left = free / 2.0,
            (Dim::Auto, _) if free > 0.0 => margin_left = free + e.margin[LEFT],
            _ => {}
        }
        let (dx, dy) = self.relative_offset(style, cb_width, None);
        let shift = (shift.0 + dx, shift.1 + dy);
        let bx = x + margin_left;
        let by = y + e.margin[TOP];
        let content_x = bx + e.border[LEFT] + e.padding[LEFT];
        let content_y = by + e.border[TOP] + e.padding[TOP];
        let order = self.next_order();
        let positioned = style.position != Position::Static;
        if positioned {
            self.pending.push(Vec::new());
        }
        let in_block_flow = std::mem::take(&mut self.block_flow_child);
        let declared_h =
            style
                .height
                .resolve(self.cb_height)
                .map(|h| if style.border_box { (h - e.vertical()).max(0.0) } else { h });
        let min_h =
            style.min_height.resolve(self.cb_height).map(|h| if style.border_box { h - e.vertical() } else { h });
        let collapse = if in_block_flow && !establishes_bfc(style) {
            Collapse {
                top: e.border[TOP] == 0.0 && e.padding[TOP] == 0.0,
                bottom: e.border[BOTTOM] == 0.0
                    && e.padding[BOTTOM] == 0.0
                    && declared_h.is_none()
                    && min_h.is_none_or(|m| m <= 0.0),
            }
        } else {
            Collapse::default()
        };
        // A box with a definite height is that tall whatever its contents:
        // a probe that only reads heights does not lay them out (table
        // cells at their row height, flexed items).
        let content_h = if self.measure_only && declared_h.is_some() {
            self.trailing = Margin::ZERO;
            0.0
        } else {
            self.contents(el, style, content_x, content_y, content_w, declared_h, fixed, shift, collapse)
        };
        let inner_trailing = std::mem::take(&mut self.trailing);
        let max_h =
            style.max_height.resolve(self.cb_height).map(|h| if style.border_box { h - e.vertical() } else { h });
        let height = Self::clamp(declared_h.unwrap_or(content_h), min_h, max_h);
        let rect =
            Rect { x: bx + shift.0, y: by + shift.1, width: content_w + e.horizontal(), height: height + e.vertical() };
        self.record(
            el,
            rect,
            &e,
            fixed,
            positioned,
            style.clips_x || style.clips_y,
            order,
            content_w,
            height,
            content_h,
        );
        if positioned {
            let pending = self.pending.pop().unwrap_or_default();
            let padding_box = Rect {
                x: rect.x + e.border[LEFT],
                y: rect.y + e.border[TOP],
                width: rect.width - e.border[LEFT] - e.border[RIGHT],
                height: rect.height - e.border[TOP] - e.border[BOTTOM],
            };
            self.place_absolute_into(el, pending, padding_box, fixed, style.clips_x || style.clips_y);
        }
        self.last_trailing = Margin::of(e.margin[BOTTOM]).with(inner_trailing);
        e.margin[TOP] + rect.height + e.margin[BOTTOM]
    }

    /// The top margin of a block-level box in a block flow, collapsed with
    /// the leading margins of its first in-flow children when they adjoin
    /// (no border, padding or line content between them), and through
    /// empty blocks before them.
    fn leading_margin(&self, el: NodeId, style: &FlowStyle, width: f64) -> Margin {
        let key = (el, width.to_bits());
        if let Some(&m) = self.memo.leading.borrow().get(&key) {
            return m;
        }
        let e = self.edges(style, width);
        let mut m = Margin::of(e.margin[TOP]);
        if !establishes_bfc(style)
            && e.border[TOP] == 0.0
            && e.padding[TOP] == 0.0
            && self.generated(el, false).is_none()
        {
            for (c, cs) in self.flatten_children(el) {
                let Some(cs) = cs else {
                    if self.dom.char_data(c).is_ok_and(is_blank) {
                        continue;
                    }
                    break;
                };
                if matches!(cs.position, Position::Absolute | Position::Fixed) {
                    continue;
                }
                if cs.outer != OuterDisplay::Block {
                    break;
                }
                m = m.with(self.leading_margin(c, &cs, width));
                if !self.collapses_through(c, &cs, width) {
                    break;
                }
                m = m.with(Margin::of(self.edges(&cs, width).margin[BOTTOM]));
            }
        }
        self.memo.leading.borrow_mut().insert(key, m);
        m
    }

    /// An empty block whose top and bottom margins collapse together (CSS
    /// 2.1 8.3.1): no height, padding, border or in-flow content (decided
    /// from the tree, before layout).
    fn collapses_through(&self, el: NodeId, style: &FlowStyle, width: f64) -> bool {
        let zero = |d: Dim| match d {
            Dim::Auto => true,
            Dim::Len(l) => l.resolve(width) == 0.0,
            Dim::Content(_) => false,
        };
        let e = self.edges(style, width);
        if establishes_bfc(style)
            || !zero(style.height)
            || !zero(style.min_height)
            || e.vertical() != 0.0
            || self.generated(el, false).is_some()
            || self.generated(el, true).is_some()
        {
            return false;
        }
        self.flatten_children(el).into_iter().all(|(c, cs)| match cs {
            None => self.dom.char_data(c).is_ok_and(is_blank),
            Some(cs) => {
                matches!(cs.position, Position::Absolute | Position::Fixed)
                    || (cs.outer == OuterDisplay::Block && self.collapses_through(c, &cs, width))
            }
        })
    }

    fn next_order(&mut self) -> u32 {
        self.order += 1;
        self.order
    }

    /// DOM depth of `el` and the depth of its nearest inclusive clipping
    /// ancestor, memoized for the pass. Iterative: walks up to the first
    /// element with a known fact (or the root), then fills the path back
    /// down, so each element is resolved once per pass and the walk is
    /// bounded by the DOM depth, not by recursion.
    fn clip_fact(&self, el: NodeId) -> ClipFact {
        if let Some(&f) = self.memo.clip_facts.borrow().get(&el) {
            return f;
        }
        let mut path = std::mem::take(&mut *self.memo.clip_path.borrow_mut());
        path.clear();
        let mut base = ClipFact { depth: 0, clip_depth: NO_CLIP };
        let mut cur = Some(el);
        while let Some(n) = cur {
            if let Some(&f) = self.memo.clip_facts.borrow().get(&n) {
                base = f;
                break;
            }
            path.push(n);
            cur = self.dom.parent_element(n).ok().flatten();
        }
        #[cfg(debug_assertions)]
        record_scan_stats::FACT_STEPS.fetch_add(path.len() as u64, std::sync::atomic::Ordering::Relaxed);
        for &n in path.iter().rev() {
            let depth = base.depth + 1;
            let clips = self.style_of(n).is_some_and(|s| s.clips_x || s.clips_y);
            base = ClipFact { depth, clip_depth: if clips { depth } else { base.clip_depth } };
            self.memo.clip_facts.borrow_mut().insert(n, base);
        }
        *self.memo.clip_path.borrow_mut() = path;
        base
    }

    /// Reference form of the text clip test (the walk the memoized facts
    /// replace): some ancestor of `t` strictly below `el` clips. Kept for the
    /// debug-build equality assertion in `record` and the benchmark.
    #[cfg(debug_assertions)]
    fn clipped_inside_walk(&self, t: NodeId, el: NodeId) -> bool {
        let mut cur = self.dom.parent_element(t).ok().flatten();
        while let Some(a) = cur.filter(|&a| a != el) {
            record_scan_stats::WALK_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if self.style_of(a).is_some_and(|s| s.clips_x || s.clips_y) {
                return true;
            }
            cur = self.dom.parent_element(a).ok().flatten();
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        el: NodeId,
        rect: Rect,
        e: &Edges,
        fixed: bool,
        positioned: bool,
        clips: bool,
        order: u32,
        content_w: f64,
        content_h: f64,
        flow_h: f64,
    ) {
        let client_width = content_w + e.padding[LEFT] + e.padding[RIGHT];
        let client_height = content_h + e.padding[TOP] + e.padding[BOTTOM];
        // Overflowing descendants extend the scroll size, except those clipped
        // by a box inside this one (and fixed boxes escaping a non-fixed one).
        let inner_x = rect.x + e.border[LEFT];
        let inner_y = rect.y + e.border[TOP];
        let mut sw = client_width;
        let mut sh = client_height.max(flow_h + e.padding[TOP] + e.padding[BOTTOM]);
        // A textarea's value is not laid out as children, yet its lines are
        // what `scrollHeight` reports (autosizing sets height 0 and reads it).
        if let Some(h) = self.textarea_text_height(el, content_w) {
            sh = sh.max(h + e.padding[TOP] + e.padding[BOTTOM]);
        }
        if let Some(w) = self.text_control_text_width(el) {
            sw = sw.max(w + e.padding[LEFT] + e.padding[RIGHT]);
        }
        // Overflowing text extends the scroll width too (Chromium: through
        // the inline-end padding), unless a clipping element inside this one
        // holds it.
        // Every text recorded after this box's order lies inside it (orders
        // are assigned at box start and texts take the current order while the
        // box's contents, including its own absolutely positioned descendants,
        // are laid out), so "clipped by an element inside this box" is: the
        // text's nearest clipping ancestor is deeper than this box.
        let first = self.out.text_orders.partition_point(|(o, _)| *o < order);
        let el_depth = self.clip_fact(el).depth;
        for &(_, t) in &self.out.text_orders[first..] {
            let Some(&(r, text_fixed)) = self.out.texts.get(&t) else { continue };
            if text_fixed && !fixed {
                continue;
            }
            let clipped_inside = match self.dom.parent_element(t).ok().flatten() {
                Some(p) => self.clip_fact(p).clip_depth > el_depth,
                None => false,
            };
            #[cfg(debug_assertions)]
            {
                record_scan_stats::PAIRS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug_assert!(self.dom.contains(el, t).unwrap_or(false), "text run outside the recorded box");
                debug_assert_eq!(clipped_inside, self.clipped_inside_walk(t, el), "clip fact differs from the walk");
            }
            if !clipped_inside {
                sw = sw.max(r.x + r.width - inner_x + e.padding[RIGHT]);
            }
        }
        self.boxes_after(order, |b| {
            let clipped_inside = b.clipped_by > order;
            if !clipped_inside && (fixed || !b.fixed) {
                sw = sw.max(b.rect.x + b.rect.width - inner_x);
                sh = sh.max(b.rect.y + b.rect.height - inner_y);
            }
            if clips && b.clipped_by == 0 {
                b.clipped_by = order;
            }
            // Descendants of a positioned box paint in its layer.
            if positioned {
                b.positioned = true;
            }
        });
        if positioned {
            for slot in self.out.slots.iter_mut().skip(order as usize) {
                if slot.positioned_by == 0 {
                    slot.positioned_by = order;
                }
            }
        }
        self.insert_box(
            el,
            FlowBox {
                rect,
                fixed,
                client_width,
                client_height,
                scroll_width: sw,
                scroll_height: sh,
                positioned,
                order,
                clipped_by: 0,
            },
        );
    }

    /// Lays out the children of a block container in its content box and
    /// returns the content height. `height`: the definite content height.
    #[allow(clippy::too_many_arguments)]
    fn contents(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        height: Option<f64>,
        fixed: bool,
        shift: (f64, f64),
        collapse: Collapse,
    ) -> f64 {
        let outer = std::mem::replace(&mut self.cb_height, height);
        let outer_own = std::mem::replace(&mut self.own_cb_height, outer);
        let h = self.contents_reused(el, style, x, y, width, height, fixed, shift, outer, collapse);
        if !collapse.bottom {
            self.trailing = Margin::ZERO;
        }
        self.cb_height = outer;
        self.own_cb_height = outer_own;
        h
    }

    #[allow(clippy::too_many_arguments)]
    fn contents_in(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        height: Option<f64>,
        fixed: bool,
        shift: (f64, f64),
        collapse: Collapse,
    ) -> f64 {
        if style.replaced {
            // With a ratio the height follows the used width (CSS 2.1 10.6.2).
            return match self.replaced_ratio(el, style) {
                Some(r) => width / r,
                None => self.intrinsic(el, style).map_or(0.0, |(_, h)| h),
            };
        }
        if style.inner == InnerDisplay::Flex {
            return self.flex(el, style, x, y, width, height, fixed, shift);
        }
        if style.inner == InnerDisplay::Table {
            return self.table(el, style, x, y, width, fixed, shift);
        }
        if style.inner == InnerDisplay::Grid {
            return self.grid(el, style, x, y, width, height, fixed, shift);
        }
        let kids = self.flatten_children(el);
        let mut cursor = y;
        // The margin below the last in-flow content, collapsing with the
        // next block's leading margin (CSS 2.1 8.3.1).
        let mut pending = Margin::ZERO;
        // The container's top margin took the leading margins of its first
        // in-flow blocks (`leading_margin`): they start at the content top.
        let mut absorbed = collapse.top;
        // The current run of inline-level content (children and generated
        // content), as items; `run_open`: it has begun.
        let mut items: Vec<InlineItem> = Vec::new();
        let mut run_open = false;
        let none = Owners::from([]);
        if let Some(g) = self.generated(el, false) {
            absorbed = false;
            generated_items(&g, style, &none, &mut items);
            run_open = true;
            if g.display == GeneratedDisplay::Block {
                let run = std::mem::take(&mut items);
                self.flow_run(run, style, x, width, fixed, shift, &mut cursor, &mut pending, &mut absorbed);
                run_open = false;
            }
        }
        for (child, child_style) in kids {
            let is_text = child_style.is_none();
            let out_of_flow =
                child_style.as_ref().is_some_and(|s| matches!(s.position, Position::Absolute | Position::Fixed));
            if out_of_flow {
                let s = child_style.expect("styled");
                let static_y = cursor + pending.value().min(0.0);
                self.defer_absolute(child, s, x - shift.0, static_y);
                continue;
            }
            let inline = is_text || child_style.as_ref().is_some_and(|s| s.outer == OuterDisplay::Inline);
            if inline {
                self.collect_inline(child, style, &none, &mut items);
                run_open = true;
                continue;
            }
            if run_open {
                let run = std::mem::take(&mut items);
                self.flow_run(run, style, x, width, fixed, shift, &mut cursor, &mut pending, &mut absorbed);
                run_open = false;
            }
            let s = child_style.expect("styled");
            let e = self.edges(&s, width);
            let lead = self.leading_margin(child, &s, width);
            let through = self.collapses_through(child, &s, width);
            let border_top = if absorbed { cursor } else { cursor + pending.with(lead).value() };
            self.block_flow_child = true;
            let taken = self.block_child(child, &s, x, border_top - e.margin[TOP], width, fixed, shift);
            let trailing = std::mem::take(&mut self.last_trailing);
            if through {
                // An empty block: its margins join those around it.
                if !absorbed {
                    pending = pending.with(lead).with(trailing);
                }
            } else {
                cursor = border_top + taken - e.margin[TOP] - e.margin[BOTTOM];
                pending = trailing;
                absorbed = false;
            }
        }
        let after = self.generated(el, true);
        if let Some(g) = after.as_ref().filter(|g| g.display != GeneratedDisplay::Block) {
            generated_items(g, style, &none, &mut items);
            run_open = true;
        }
        if run_open {
            let run = std::mem::take(&mut items);
            self.flow_run(run, style, x, width, fixed, shift, &mut cursor, &mut pending, &mut absorbed);
        }
        if let Some(g) = after.filter(|g| g.display == GeneratedDisplay::Block) {
            generated_items(&g, style, &none, &mut items);
            let run = std::mem::take(&mut items);
            self.flow_run(run, style, x, width, fixed, shift, &mut cursor, &mut pending, &mut absorbed);
        }
        if collapse.bottom {
            // The last margin collapses with the container's bottom margin.
            self.trailing = pending;
        } else {
            cursor += pending.value();
        }
        cursor - y
    }

    /// Lays out a run of inline content as line boxes below `cursor` (and
    /// the pending margin). A run of collapsible white space only makes no
    /// line: margins collapse across it.
    #[allow(clippy::too_many_arguments)]
    fn flow_run(
        &mut self,
        items: Vec<InlineItem>,
        style: &FlowStyle,
        x: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
        cursor: &mut f64,
        pending: &mut Margin,
        absorbed: &mut bool,
    ) {
        let content = items.iter().any(|it| match it {
            InlineItem::Word { height, .. } => *height > 0.0,
            InlineItem::OutOfFlow { .. } => false,
            _ => true,
        });
        let top = *cursor + pending.value();
        let h = self.inline_items(items, style, x, top, width, fixed, shift);
        if content || h > 0.0 {
            *cursor = top + h;
            *pending = Margin::ZERO;
            *absorbed = false;
        }
    }

    /// Children of `el` with their styles (None for text), with
    /// `display: contents` elements replaced by their children and
    /// unrendered elements dropped.
    fn flatten_children(&self, el: NodeId) -> Vec<(NodeId, Option<StyleRef>)> {
        let mut out = Vec::new();
        let Ok(children) = self.dom.children(el) else { return out };
        for c in children {
            match self.dom.node_type(c) {
                Ok(NodeType::Text) => {
                    if self.dom.char_data(c).is_ok_and(|t| !t.is_empty()) {
                        out.push((c, None));
                    }
                }
                Ok(NodeType::Element) => {
                    if let Some(s) = self.style_of(c) {
                        if s.outer == OuterDisplay::Contents {
                            out.extend(self.flatten_children(c));
                        } else {
                            out.push((c, Some(s)));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn defer_absolute(&mut self, el: NodeId, style: StyleRef, static_x: f64, static_y: f64) {
        let entry = Pending { el, style, static_x, static_y };
        if entry.style.position == Position::Fixed {
            // The viewport is the containing block.
            self.pending.first_mut().expect("initial containing block").push(entry);
        } else {
            self.pending.last_mut().expect("containing block").push(entry);
        }
    }

    /// Places the absolutely positioned descendants of positioned box `el`
    /// (recorded already) against its padding box, then extends its scroll
    /// size over them and, if it clips, marks them clipped by it.
    fn place_absolute_into(&mut self, el: NodeId, pending: Vec<Pending>, padding_box: Rect, fixed: bool, clips: bool) {
        let before = self.order;
        self.place_absolute(pending, padding_box, fixed);
        let Some(own) = self.out.boxes.get(&el).copied() else { return };
        let (mut sw, mut sh) = (own.scroll_width, own.scroll_height);
        self.boxes_after(before, |b| {
            let clipped_inside = b.clipped_by > before;
            if !clipped_inside && (own.fixed || !b.fixed) {
                sw = sw.max(b.rect.x + b.rect.width - padding_box.x);
                sh = sh.max(b.rect.y + b.rect.height - padding_box.y);
            }
            if clips && b.clipped_by == 0 && (own.fixed || !b.fixed) {
                b.clipped_by = own.order;
            }
        });
        if let Some(b) = self.out.boxes.get_mut(&el) {
            b.scroll_width = sw;
            b.scroll_height = sh;
        }
    }

    /// Places absolutely positioned boxes against `cb` (a padding box; the
    /// viewport for fixed ones).
    fn place_absolute(&mut self, pending: Vec<Pending>, cb: Rect, fixed_cb: bool) {
        for p in pending {
            let fixed = p.style.position == Position::Fixed || fixed_cb;
            let cb = if p.style.position == Position::Fixed {
                Rect { x: 0.0, y: 0.0, width: self.vw, height: self.vh }
            } else {
                cb
            };
            let s = &p.style;
            let e = self.edges(s, cb.width);
            let left = s.inset[LEFT].resolve(Some(cb.width));
            let right = s.inset[RIGHT].resolve(Some(cb.width));
            let top = s.inset[TOP].resolve(Some(cb.height));
            let bottom = s.inset[BOTTOM].resolve(Some(cb.height));
            let declared_w = s.width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            let available = (cb.width
                - e.margin[LEFT]
                - e.margin[RIGHT]
                - e.horizontal()
                - left.unwrap_or(0.0)
                - right.unwrap_or(0.0))
            .max(0.0);
            let mut content_w = match (declared_w, left, right) {
                (Some(w), _, _) => w,
                (None, Some(_), Some(_)) => available,
                _ => self.intrinsic_width(p.el, s, available),
            };
            let min_w = s.min_width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            let max_w = s.max_width.resolve(Some(cb.width)).map(|w| Self::declared_content(s, w, e.horizontal()));
            content_w = Self::clamp(content_w, min_w, max_w);
            let border_w = content_w + e.horizontal();
            let bx = match (left, right) {
                (Some(l), _) => cb.x + l + e.margin[LEFT],
                (None, Some(r)) => cb.x + cb.width - r - e.margin[RIGHT] - border_w,
                (None, None) => p.static_x + e.margin[LEFT],
            };
            let declared_h =
                s.height.resolve(Some(cb.height)).map(|h| if s.border_box { (h - e.vertical()).max(0.0) } else { h });
            let order = self.next_order();
            self.pending.push(Vec::new());
            // Lay out content at a provisional y, then move once the height is known.
            let provisional_y = match top {
                Some(t) => cb.y + t + e.margin[TOP],
                None => p.static_y + e.margin[TOP],
            };
            let content_x = bx + e.border[LEFT] + e.padding[LEFT];
            let content_y = provisional_y + e.border[TOP] + e.padding[TOP];
            let first_new = self.order;
            let definite_h = declared_h.or_else(|| match (top, bottom) {
                (Some(t), Some(b)) => {
                    Some((cb.height - t - b - e.margin[TOP] - e.margin[BOTTOM] - e.vertical()).max(0.0))
                }
                _ => None,
            });
            let content_h = self.contents(p.el, s, content_x, content_y, content_w, definite_h, fixed, (0.0, 0.0), Collapse::default());
            // min/max-height are border-box sizes under box-sizing: border-box.
            let to_content_h = |h: f64| if s.border_box { (h - e.vertical()).max(0.0) } else { h };
            let min_h = s.min_height.resolve(Some(cb.height)).map(to_content_h);
            let max_h = s.max_height.resolve(Some(cb.height)).map(to_content_h);
            let mut height = declared_h.unwrap_or(content_h);
            if declared_h.is_none() && top.is_some() && bottom.is_some() {
                height = (cb.height
                    - top.unwrap_or(0.0)
                    - bottom.unwrap_or(0.0)
                    - e.margin[TOP]
                    - e.margin[BOTTOM]
                    - e.vertical())
                .max(0.0);
            }
            let height = Self::clamp(height, min_h, max_h);
            let border_h = height + e.vertical();
            let by = match (top, bottom) {
                (Some(_), _) => provisional_y,
                (None, Some(b)) => cb.y + cb.height - b - e.margin[BOTTOM] - border_h,
                (None, None) => provisional_y,
            };
            let delta = by - provisional_y;
            if delta != 0.0 {
                self.boxes_after(first_new, |b| b.rect.y += delta);
                self.shift_texts_after(first_new, delta);
            }
            let rect = Rect { x: bx, y: by, width: border_w, height: border_h };
            self.record(p.el, rect, &e, fixed, true, s.clips_x || s.clips_y, order, content_w, height, content_h);
            let nested = self.pending.pop().unwrap_or_default();
            let padding_box = Rect {
                x: rect.x + e.border[LEFT],
                y: rect.y + e.border[TOP],
                width: rect.width - e.border[LEFT] - e.border[RIGHT],
                height: rect.height - e.border[TOP] - e.border[BOTTOM],
            };
            self.place_absolute_into(p.el, nested, padding_box, fixed, s.clips_x || s.clips_y);
        }
    }

    /// Content width of a box whose width is not a length: the intrinsic
    /// keyword it names, else shrink-to-fit in `available`.
    fn intrinsic_width(&self, el: NodeId, style: &FlowStyle, available: f64) -> f64 {
        match style.width {
            Dim::Content(ContentSize::Min) => self.min_content(el, style),
            Dim::Content(ContentSize::Max) => self.max_content(el, style),
            _ => self.shrink_to_fit(el, style, available),
        }
    }

    /// CSS 2.1 10.3.5: min(max(min-content, available), max-content).
    fn shrink_to_fit(&self, el: NodeId, style: &FlowStyle, available: f64) -> f64 {
        let max = self.max_content(el, style);
        if max <= available {
            return max;
        }
        max.min(available.max(self.min_content(el, style)))
    }

    /// Max-content width of an element's contents (no wrapping).
    fn max_content(&self, el: NodeId, style: &FlowStyle) -> f64 {
        if style.replaced {
            return style.width.resolve(None).or_else(|| self.replaced_auto_width(el, style)).unwrap_or(0.0);
        }
        if let Some(&w) = self.memo.max_content.borrow().get(&el) {
            return w;
        }
        let w = self.content_width(el, style, false);
        self.memo.max_content.borrow_mut().insert(el, w);
        w
    }

    /// Min-content width (`min`: the narrowest without overflow: longest
    /// word, widest atomic box) or max-content width (no wrapping) of an
    /// element's contents. Flex rows add their items and gaps.
    fn content_width(&self, el: NodeId, style: &FlowStyle, min: bool) -> f64 {
        if style.inner == InnerDisplay::Table {
            return if min { self.table_min_content(el, style) } else { self.table_max_content(el, style) };
        }
        if style.inner == InnerDisplay::Grid {
            return self.grid_content_width(el, style, min);
        }
        let flex = style.inner == InnerDisplay::Flex;
        let flex_row = flex && matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
        let mut best: f64 = 0.0;
        let mut line: f64 = 0.0;
        let mut sum: f64 = 0.0;
        let mut items = 0usize;
        let mut text_item = false;
        // Max-content lines: collapsible whitespace between pieces (text
        // nodes, inline elements) is one space, as in `collect_inline`
        // ("Page <span>1</span>" keeps its space); none at line ends.
        let mut pending_space: f64 = 0.0;
        let mut line_started = false;
        // Generated content (flow containers only) is text, or one atomic
        // box for an inline-block, or its own line(s) for a block.
        let generated = |g: &Generated, best: &mut f64, line: &mut f64, line_started: &mut bool, pending: &mut f64| {
            if g.text.chars().all(|c| c.is_ascii_whitespace()) {
                return;
            }
            let (fs, weight) = (style.font_size, style.font_weight);
            match g.display {
                GeneratedDisplay::Block => {
                    let w = if min {
                        min_text_width(&g.text, fs, weight, style.wrap)
                    } else {
                        collapsed_width(&g.text, fs, weight)
                    };
                    *best = best.max(*line).max(w);
                    *line = 0.0;
                    *line_started = false;
                    *pending = 0.0;
                }
                GeneratedDisplay::Inline if min && !style.nowrap => {
                    *best = best.max(min_text_width(&g.text, fs, weight, style.wrap));
                }
                _ if min => *best = best.max(collapsed_width(&g.text, fs, weight)),
                _ => {
                    if *line_started {
                        *line += *pending;
                    }
                    *line += collapsed_width(&g.text, fs, weight);
                    *line_started = true;
                    *pending = 0.0;
                }
            }
        };
        if !flex {
            if let Some(g) = self.generated(el, false) {
                generated(&g, &mut best, &mut line, &mut line_started, &mut pending_space);
            }
        }
        for (child, s) in self.flatten_children(el) {
            let w = match s {
                None => {
                    let text = self.dom.char_data(child).unwrap_or("");
                    if is_blank(text) && flex {
                        continue;
                    }
                    let w = if min && !style.nowrap {
                        min_text_width(text, style.font_size, style.font_weight, style.wrap)
                    } else {
                        collapsed_width(text, style.font_size, style.font_weight)
                    };
                    if flex_row {
                        // Adjacent text forms one anonymous item.
                        if !text_item {
                            items += 1;
                        }
                        text_item = true;
                        if min && !style.flex_wrap {
                            sum += w;
                        } else if min {
                            best = best.max(w);
                        } else {
                            sum += w;
                        }
                        continue;
                    }
                    if min || flex {
                        best = best.max(w);
                    } else if !is_blank(text) {
                        let space = text_width(" ", style.font_size, style.font_weight);
                        if line_started && (pending_space > 0.0 || text.starts_with(|c: char| c.is_ascii_whitespace()))
                        {
                            line += pending_space.max(space);
                        }
                        line += w;
                        line_started = true;
                        pending_space = if text.ends_with(|c: char| c.is_ascii_whitespace()) { space } else { 0.0 };
                    } else if line_started && !text.is_empty() {
                        pending_space = text_width(" ", style.font_size, style.font_weight);
                    }
                    continue;
                }
                Some(s) if matches!(s.position, Position::Absolute | Position::Fixed) => continue,
                Some(s) => {
                    let e = self.edges(&s, 0.0);
                    let to_content = |v: f64| Self::declared_content(&s, v, e.horizontal());
                    let percent = |d: Dim| matches!(d, Dim::Len(l) if l.percent != 0.0);
                    let inner = match s.width.resolve(None) {
                        Some(w) => to_content(w),
                        // Compressible replaced elements (form controls, images)
                        // with a percentage width or max-width contribute no
                        // min-content width (CSS Sizing 3, 5.2.2).
                        None if min && s.replaced && (percent(s.width) || percent(s.max_width)) => 0.0,
                        None if min => self.min_content(child, &s),
                        None => self.max_content(child, &s),
                    };
                    let inner = Self::clamp(
                        inner,
                        s.min_width.resolve(None).map(to_content),
                        s.max_width.resolve(None).map(to_content),
                    );
                    (inner + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT], s.outer)
                }
            };
            text_item = false;
            let (w, outer) = w;
            if flex_row {
                items += 1;
                if min && style.flex_wrap {
                    best = best.max(w);
                } else {
                    sum += w;
                }
            } else if outer == OuterDisplay::Inline && !flex {
                if min {
                    best = best.max(w);
                } else if w > 0.0 {
                    if line_started {
                        line += pending_space;
                    }
                    line += w;
                    line_started = true;
                    pending_space = 0.0;
                }
            } else {
                best = best.max(line).max(w);
                line = 0.0;
                line_started = false;
                pending_space = 0.0;
            }
        }
        if !flex {
            if let Some(g) = self.generated(el, true) {
                generated(&g, &mut best, &mut line, &mut line_started, &mut pending_space);
            }
        }
        if flex_row && items > 0 {
            let gap = style.column_gap.resolve(0.0) * (items - 1) as f64;
            if !(min && style.flex_wrap) {
                return sum + gap;
            }
        }
        best.max(line)
    }

    fn min_content(&self, el: NodeId, style: &FlowStyle) -> f64 {
        if style.replaced {
            return self.max_content(el, style);
        }
        if let Some(&w) = self.memo.min_content.borrow().get(&el) {
            return w;
        }
        let w = self.content_width(el, style, true);
        self.memo.min_content.borrow_mut().insert(el, w);
        w
    }

    /// Lays out a run of inline-level children as line boxes in
    /// [x, x+width); returns the run's height. Items sit on a common
    /// baseline: text by its font ascent (plus half-leading), atomic boxes by
    /// their bottom margin edge, and every line includes the parent's strut.
    #[allow(clippy::too_many_arguments)]
    fn inline_run(
        &mut self,
        run: &[NodeId],
        parent: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let mut items = Vec::new();
        for &n in run {
            self.collect_inline(n, parent, &Owners::from([]), &mut items);
        }
        self.inline_items(items, parent, x, y, width, fixed, shift)
    }

    /// Lays out inline items (`inline_run`) as line boxes in [x, x+width);
    /// returns their height.
    #[allow(clippy::too_many_arguments)]
    fn inline_items(
        &mut self,
        items: Vec<InlineItem>,
        parent: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let strut = text_extents(parent);
        let mut frags: FxHashMap<NodeId, Rect> = FxHashMap::default();
        let mut order_of: Vec<NodeId> = Vec::new();
        let mut line = Line::default();
        let mut line_y = y;
        for item in items {
            match item {
                InlineItem::Break if line.items.is_empty() => {
                    // A forced break on an empty line still makes a line box
                    // of the strut's height (`<p><br></p>`, consecutive brs).
                    line_y += strut.0 + strut.1;
                    line = Line::default();
                }
                InlineItem::Break => {
                    line.extents(strut);
                    line_y += self.flush_line(
                        &mut line,
                        x,
                        line_y,
                        width,
                        fixed,
                        shift,
                        &mut frags,
                        &mut order_of,
                        parent.text_align,
                    );
                }
                InlineItem::Word {
                    width: w,
                    height: h,
                    space_before,
                    owners,
                    ascent,
                    content,
                    breakable,
                    nowrap,
                    source,
                } => {
                    let mut source = source;
                    let gap = if line.x > 0.0 { space_before } else { 0.0 };
                    if line.x > 0.0 && line.x + gap + w > width + FIT_EPSILON && !nowrap {
                        line_y += self.flush_line(
                            &mut line,
                            x,
                            line_y,
                            width,
                            fixed,
                            shift,
                            &mut frags,
                            &mut order_of,
                            parent.text_align,
                        );
                    }
                    // A word wider than an empty line breaks where overflow-wrap
                    // or word-break allows: the longest prefixes that fit, one
                    // per line; the rest continues the last line.
                    let mut w = w;
                    if line.x == 0.0 && w > width + FIT_EPSILON {
                        if let Some((text, fs, weight)) = &breakable {
                            // A word broken over lines has no text run.
                            source = None;
                            let pieces = break_word(text, *fs, *weight, width);
                            let (last, full) = pieces.split_last().expect("a word has a piece");
                            for &piece in full {
                                line.push(0.0, piece, h, ascent, owners.clone(), None);
                                if let Some(it) = line.items.last_mut() {
                                    it.content = Some(content);
                                }
                                line.extents(strut);
                                line_y += self.flush_line(
                                    &mut line,
                                    x,
                                    line_y,
                                    width,
                                    fixed,
                                    shift,
                                    &mut frags,
                                    &mut order_of,
                                    parent.text_align,
                                );
                            }
                            w = *last;
                        }
                    }
                    let at = if line.x > 0.0 { line.x + space_before } else { 0.0 };
                    line.push(at, w, h, ascent, owners, None);
                    if let Some(it) = line.items.last_mut() {
                        it.content = Some(content);
                        it.source = source;
                    }
                    line.extents(strut);
                }
                InlineItem::Atomic { el, style, owners } => {
                    let e = self.edges(&style, width);
                    let content_w = match style.width.resolve(Some(width)) {
                        Some(w) => Self::declared_content(&style, w, e.horizontal()),
                        None => self.intrinsic_width(el, &style, (width - e.horizontal()).max(0.0)),
                    };
                    let outer_w = content_w + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT];
                    let outer_h = self.atomic_height(el, &style, content_w, width) + e.margin[TOP] + e.margin[BOTTOM];
                    if line.x > 0.0 && line.x + outer_w > width + FIT_EPSILON && !parent.nowrap {
                        line_y += self.flush_line(
                            &mut line,
                            x,
                            line_y,
                            width,
                            fixed,
                            shift,
                            &mut frags,
                            &mut order_of,
                            parent.text_align,
                        );
                    }
                    // Text-bearing controls sit on their text baseline; other
                    // atomic boxes on their CSS baseline (atomic_baseline) or
                    // their bottom margin edge.
                    let ascent = match control_kind(self.dom, el) {
                        Some(Control::Text | Control::Button(_) | Control::File) => {
                            e.margin[TOP] + e.border[TOP] + e.padding[TOP] + text_extents(&style).0
                        }
                        // A select ignores line-height: normal ascent plus its 1px inset.
                        Some(Control::Select { rows: None }) => {
                            e.margin[TOP] + e.border[TOP] + e.padding[TOP] + font_metrics(style.font_size).0 + 1.0
                        }
                        Some(_) => outer_h,
                        None => {
                            let content_h = outer_h - e.margin[TOP] - e.margin[BOTTOM] - e.vertical();
                            match self.atomic_baseline(el, &style, content_w, content_h.max(0.0)) {
                                Some(b) => e.margin[TOP] + e.border[TOP] + e.padding[TOP] + b,
                                None => outer_h,
                            }
                        }
                    };
                    let at = line.x;
                    line.push(at, outer_w, outer_h, ascent, owners, Some((el, style.clone())));
                    line.extents(strut);
                }
                InlineItem::Generated { width: w, height: h, ascent, owners } => {
                    if line.x > 0.0 && line.x + w > width + FIT_EPSILON && !parent.nowrap {
                        line_y += self.flush_line(
                            &mut line,
                            x,
                            line_y,
                            width,
                            fixed,
                            shift,
                            &mut frags,
                            &mut order_of,
                            parent.text_align,
                        );
                    }
                    let at = line.x;
                    line.push(at, w, h, ascent, owners, None);
                    if let Some(it) = line.items.last_mut() {
                        it.generated = true;
                    }
                    line.extents(strut);
                }
                InlineItem::Empty { owners, height, ascent, phantom } => {
                    let at = line.x;
                    line.push(at, 0.0, height, ascent, owners, None);
                    if let Some(it) = line.items.last_mut() {
                        it.phantom = phantom;
                    }
                    line.extents(strut);
                }
                InlineItem::Block { el, style, owners } => {
                    line_y += self.flush_line(
                        &mut line,
                        x,
                        line_y,
                        width,
                        fixed,
                        shift,
                        &mut frags,
                        &mut order_of,
                        parent.text_align,
                    );
                    line_y += self.block_child(el, &style, x, line_y, width, fixed, shift);
                    if let Some(r) = self.out.boxes.get(&el).map(|b| b.rect) {
                        for o in owners.iter() {
                            union(&mut frags, &mut order_of, *o, r);
                        }
                    }
                }
                InlineItem::OutOfFlow { el, style } => {
                    // Static position: where the next inline content would go.
                    self.defer_absolute(el, style.clone(), x - shift.0 + line.x, line_y);
                }
            }
        }
        line_y +=
            self.flush_line(&mut line, x, line_y, width, fixed, shift, &mut frags, &mut order_of, parent.text_align);
        // Inline element boxes: union of their fragments.
        for el in order_of {
            if let Some(r) = frags.get(&el).copied() {
                if self.dom.node_type(el).ok() == Some(NodeType::Text) {
                    self.out.texts.insert(el, (r, fixed));
                    self.out.text_orders.push((self.order, el));
                    continue;
                }
                let order = self.next_order();
                let positioned = self.style_of(el).is_some_and(|s| s.position != Position::Static);
                self.insert_box(
                    el,
                    FlowBox {
                        rect: r,
                        fixed,
                        client_width: 0.0,
                        client_height: 0.0,
                        scroll_width: 0.0,
                        scroll_height: 0.0,
                        positioned,
                        order,
                        clipped_by: 0,
                    },
                );
            }
        }
        line_y - y
    }

    /// Places a finished line's items at `line_y` and returns its height
    /// (0 for an empty line).
    #[allow(clippy::too_many_arguments)]
    fn flush_line(
        &mut self,
        line: &mut Line,
        x: f64,
        line_y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
        frags: &mut FxHashMap<NodeId, Rect>,
        order_of: &mut Vec<NodeId>,
        align: TextAlign,
    ) -> f64 {
        // A line holding only collapsible whitespace (zero-width spacers
        // with no owner) has no line box.
        // So does one holding only empty inline boxes without inline-axis
        // margins, borders or padding (CSS 2.1 9.4.2): they are 0x0 at its
        // start.
        let blank = line.items.iter().all(|it| {
            it.atomic.is_none() && !it.generated && it.width == 0.0 && (it.owners.is_empty() || it.phantom)
        });
        if blank {
            for it in line.items.drain(..).filter(|it| it.phantom) {
                let r = Rect { x: x + it.x + shift.0, y: line_y + shift.1, width: 0.0, height: 0.0 };
                for o in it.owners.iter() {
                    union(frags, order_of, *o, r);
                }
            }
            *line = Line::default();
            return 0.0;
        }
        if !line.ascent.is_finite() || !line.descent.is_finite() {
            line.ascent = line.ascent.max(0.0);
            line.descent = line.descent.max(0.0);
        }
        let baseline = line_y + line.ascent;
        self.first_baseline.get_or_insert(baseline);
        self.last_baseline = Some(baseline);
        // text-align: trailing collapsible spaces (ownerless zero-width
        // spacers) do not count; a line too long to fit is start-aligned.
        let extent = line
            .items
            .iter()
            .filter(|it| it.atomic.is_some() || it.generated || !it.owners.is_empty() || it.width > 0.0)
            .map(|it| it.x + it.width)
            .fold(0.0, f64::max);
        let free = (width - extent).max(0.0);
        let offset = match align {
            TextAlign::Start => 0.0,
            TextAlign::Center => free / 2.0,
            TextAlign::End => free,
        };
        for it in line.items.drain(..) {
            let left = x + offset + it.x;
            let top = baseline - it.ascent;
            if let Some((el, style)) = it.atomic {
                self.atomic(el, &style, left, top, width, fixed, shift);
                if let Some(r) = self.out.boxes.get(&el).map(|b| b.rect) {
                    for o in it.owners.iter() {
                        union(frags, order_of, *o, r);
                    }
                }
                continue;
            }
            // Text fragments report the font's content area (Chromium's text
            // and inline boxes), not the line height.
            let r = match it.content {
                Some((ca, cd)) => {
                    Rect { x: left + shift.0, y: baseline - ca + shift.1, width: it.width, height: ca + cd }
                }
                None => Rect { x: left + shift.0, y: top + shift.1, width: it.width, height: it.height },
            };
            for o in it.owners.iter() {
                union(frags, order_of, *o, r);
            }
            if let Some(src) = it.source {
                self.out.text_runs.push(TextRun {
                    node: src.node,
                    start: src.start,
                    end: src.end,
                    rect: r,
                    fixed,
                    text: src.text,
                    font_size: src.font_size,
                    weight: src.weight,
                    order: self.order,
                });
            }
        }
        let height = line.ascent + line.descent;
        *line = Line::default();
        height
    }

    fn atomic_height(&self, el: NodeId, style: &FlowStyle, content_w: f64, cb_width: f64) -> f64 {
        let e = self.edges(style, cb_width);
        let content_h = match style.height.resolve(self.cb_height) {
            Some(h) => {
                if style.border_box {
                    (h - e.vertical()).max(0.0)
                } else {
                    h
                }
            }
            None if style.replaced => match self.replaced_ratio(el, style) {
                Some(r) => content_w / r,
                None => self.intrinsic(el, style).map_or(0.0, |(_, h)| h),
            },
            None => self.measure_height(el, style, content_w),
        };
        content_h + e.vertical()
    }

    /// Content height of `el` laid out at `content_w`, without recording.
    fn measure_height(&self, el: NodeId, style: &FlowStyle, content_w: f64) -> f64 {
        let key = (el, content_w.to_bits());
        if let Some(&h) = self.memo.heights.borrow().get(&key) {
            return h;
        }
        let mut probe = self.probe();
        probe.measure_only = true;
        probe.pending.push(Vec::new());
        let h = probe.contents(el, style, 0.0, 0.0, content_w, None, false, (0.0, 0.0), Collapse::default());
        self.memo.heights.borrow_mut().insert(key, h);
        h
    }

    /// Baseline of an atomic inline box, from its content-box top (None: its
    /// bottom margin edge). CSS 2.1 10.8.1 / Flexbox 8.5 / Grid 10.6:
    /// inline-block: its last line box's baseline, unless it has none or
    /// clips its overflow; inline-flex / inline-grid: the first item's first
    /// baseline, else one synthesized from that item's bottom border edge.
    fn atomic_baseline(&self, el: NodeId, style: &FlowStyle, content_w: f64, content_h: f64) -> Option<f64> {
        if style.replaced || style.inner == InnerDisplay::Table {
            return None;
        }
        let flowroot = !matches!(style.inner, InnerDisplay::Flex | InnerDisplay::Grid);
        if flowroot && (style.clips_x || style.clips_y) {
            return None;
        }
        let key = (el, content_w.to_bits(), content_h.to_bits());
        if let Some(&b) = self.memo.baselines.borrow().get(&key) {
            return b;
        }
        let mut probe = self.probe();
        probe.pending.push(Vec::new());
        probe.contents(el, style, 0.0, 0.0, content_w, Some(content_h), false, (0.0, 0.0), Collapse::default());
        let baseline = if flowroot {
            probe.last_baseline
        } else {
            // The startmost in-flow item.
            let first = self.flatten_children(el).into_iter().find(|(c, s)| match s {
                None => self.dom.char_data(*c).is_ok_and(|t| !is_blank(t)),
                Some(s) => !matches!(s.position, Position::Absolute | Position::Fixed),
            });
            match first {
                // Anonymous text item: its first line.
                Some((_, None)) => probe.first_baseline,
                // An element item: its own first baseline, from a probe of its
                // contents at its laid-out size, else its bottom border edge.
                Some((c, Some(cs))) => probe.out.boxes.get(&c).copied().map(|b| {
                    let e = self.edges(&cs, content_w);
                    let mut item = self.probe();
                    item.pending.push(Vec::new());
                    let cw = (b.rect.width - e.horizontal()).max(0.0);
                    let ch = (b.rect.height - e.vertical()).max(0.0);
                    item.contents(c, &cs, 0.0, 0.0, cw, Some(ch), false, (0.0, 0.0), Collapse::default());
                    match item.first_baseline {
                        Some(fb) => b.rect.y + e.border[TOP] + e.padding[TOP] + fb,
                        None => b.rect.y + b.rect.height,
                    }
                }),
                None => None,
            }
        };
        self.memo.baselines.borrow_mut().insert(key, baseline);
        baseline
    }

    /// Lays out an atomic inline (inline-block / replaced) at (x, y), its
    /// margin edge.
    #[allow(clippy::too_many_arguments)]
    fn atomic(&mut self, el: NodeId, style: &FlowStyle, x: f64, y: f64, cb_width: f64, fixed: bool, shift: (f64, f64)) {
        let e = self.edges(style, cb_width);
        let content_w = match style.width.resolve(Some(cb_width)) {
            Some(w) => Self::declared_content(style, w, e.horizontal()),
            None => self.intrinsic_width(el, style, (cb_width - e.horizontal()).max(0.0)),
        };
        let mut s = style.clone();
        s.width = Dim::px(if style.border_box { content_w + e.horizontal() } else { content_w });
        s.margin[LEFT] = Dim::px(e.margin[LEFT]);
        s.margin[RIGHT] = Dim::px(e.margin[RIGHT]);
        self.block_child(el, &s, x, y, content_w + e.horizontal() + e.margin[LEFT] + e.margin[RIGHT], fixed, shift);
    }

    /// `el`'s generated `::before` (`after`: `::after`) content.
    fn generated(&self, el: NodeId, after: bool) -> Option<std::rc::Rc<Generated>> {
        if let Some(g) = self.memo.generated.borrow().get(&(el, after)) {
            return g.clone();
        }
        let g = self.src.generated(self.dom, el, after);
        self.memo.generated.borrow_mut().insert((el, after), g.clone());
        g
    }

    /// Flattens an inline-level node into items. `owners`: enclosing inline
    /// elements whose boxes include this content.
    fn collect_inline(&mut self, n: NodeId, parent: &FlowStyle, owners: &Owners, items: &mut Vec<InlineItem>) {
        match self.dom.node_type(n) {
            Ok(NodeType::Text) => {
                let text = self.dom.char_data(n).unwrap_or("");
                // The text node owns its words too: its box (FlowResult::texts)
                // is the union of their fragments, for Range geometry.
                let word_owners: Owners = owners.iter().copied().chain([n]).collect();
                text_items(text, parent, &word_owners, Some(n), items);
            }
            Ok(NodeType::Element) => {
                let Some(style) = self.style_of(n) else { return };
                if self.dom.element(n).is_ok_and(|e| e.is_html() && *e.local_name() == *"br") {
                    items.push(InlineItem::Break);
                    return;
                }
                if matches!(style.position, Position::Absolute | Position::Fixed) {
                    items.push(InlineItem::OutOfFlow { el: n, style });
                    return;
                }
                if style.outer == OuterDisplay::Block {
                    items.push(InlineItem::Block { el: n, style, owners: owners.clone() });
                    return;
                }
                if style.replaced || style.inner != InnerDisplay::Flow {
                    items.push(InlineItem::Atomic { el: n, style, owners: owners.clone() });
                    return;
                }
                let with: Owners = owners.iter().copied().chain([n]).collect();
                let before = items.len();
                // Generated content is inline here (a block-level one
                // included).
                if let Some(g) = self.generated(n, false) {
                    generated_items(&g, &style, &with, items);
                }
                for (c, _) in self.flatten_children(n) {
                    self.collect_inline(c, &style, &with, items);
                }
                if let Some(g) = self.generated(n, true) {
                    generated_items(&g, &style, &with, items);
                }
                if items.len() == before {
                    let (ascent, descent) = text_extents(&style);
                    let zero = |d: Dim| matches!(d, Dim::Auto) || d.resolve(Some(0.0)) == Some(0.0);
                    let phantom = zero(style.margin[LEFT])
                        && zero(style.margin[RIGHT])
                        && style.padding[LEFT].resolve(0.0) == 0.0
                        && style.padding[RIGHT].resolve(0.0) == 0.0
                        && style.border[LEFT] <= 0.0
                        && style.border[RIGHT] <= 0.0;
                    items.push(InlineItem::Empty { owners: with, height: ascent + descent, ascent, phantom });
                }
            }
            _ => {}
        }
    }
}

/// Words of `text` in `parent`'s font, owned by `word_owners`; `node`: the
/// text node `text` is the data of (its words get a source).
fn text_items(text: &str, parent: &FlowStyle, word_owners: &Owners, node: Option<NodeId>, items: &mut Vec<InlineItem>) {
    let fs = parent.font_size;
    let (ascent, descent) = text_extents(parent);
    let lh = ascent + descent;
    let space = text_width(" ", fs, parent.font_weight);
    // Collapsible whitespace becomes one space before the next word
    // (also across element boundaries via a zero-width word).
    let mut pending_space = false;
    // UTF-16 position in the data (each separator is one unit).
    let mut pos = 0u32;
    for (i, part) in text.split(|c: char| c.is_ascii_whitespace()).enumerate() {
        if i > 0 {
            pending_space = true;
            pos += 1;
        }
        let start = pos;
        pos += part.encode_utf16().count() as u32;
        if part.is_empty() {
            continue;
        }
        let breakable = (parent.wrap != WrapMode::Normal && !parent.nowrap)
            .then(|| (std::rc::Rc::<str>::from(part), fs, parent.font_weight));
        items.push(InlineItem::Word {
            width: text_width(part, fs, parent.font_weight),
            height: lh,
            space_before: if pending_space { space } else { 0.0 },
            owners: word_owners.clone(),
            ascent,
            content: font_metrics(fs),
            breakable,
            nowrap: parent.nowrap,
            source: node.map(|node| WordSource {
                node,
                start,
                end: pos,
                text: std::rc::Rc::from(part),
                font_size: fs,
                weight: parent.font_weight,
            }),
        });
        pending_space = false;
    }
    if pending_space {
        items.push(InlineItem::Word {
            width: 0.0,
            height: 0.0,
            space_before: space,
            owners: Owners::from([]),
            ascent: 0.0,
            content: (0.0, 0.0),
            breakable: None,
            nowrap: parent.nowrap,
            source: None,
        });
    }
}

/// Items of generated content in its originating element's font.
fn generated_items(g: &Generated, parent: &FlowStyle, owners: &Owners, items: &mut Vec<InlineItem>) {
    match g.display {
        // Generated text has no text node: no source, no text runs.
        GeneratedDisplay::Inline | GeneratedDisplay::Block => text_items(&g.text, parent, owners, None, items),
        GeneratedDisplay::InlineBlock => {
            // One line of its text; an empty box sits on the baseline.
            let empty = g.text.chars().all(|c| c.is_ascii_whitespace());
            let (ascent, descent) = text_extents(parent);
            items.push(InlineItem::Generated {
                width: if empty { 0.0 } else { collapsed_width(&g.text, parent.font_size, parent.font_weight) },
                height: if empty { 0.0 } else { ascent + descent },
                ascent: if empty { 0.0 } else { ascent },
                owners: owners.clone(),
            });
        }
    }
}

/// Form controls with an intrinsic size.
enum Control {
    Text,
    Check,
    Range,
    Color,
    File,
    Button(String),
    TextArea,
    /// `rows`: a list box (`multiple` or `size` > 1) showing that many rows.
    Select {
        rows: Option<f64>,
    },
}

/// The flow style the layout uses for `el`: the source's, with form
/// controls made atomic boxes sized by their own intrinsic size (their DOM
/// children, textarea text and options, are not laid out).
fn used_flow_style(dom: &Dom, src: &dyn FlowStyleSource, el: NodeId) -> Option<FlowStyle> {
    let mut s = src.flow_style(dom, el);
    if let Some(st) = s.as_mut() {
        if control_kind(dom, el).is_some() {
            st.replaced = true;
        }
    }
    s
}

/// An element's local name when it has no prefix (what `tagName` compares
/// against for these HTML tags), without building the `tagName` string.
fn local_tag(dom: &Dom, el: NodeId) -> Option<&str> {
    let e = dom.element(el).ok()?;
    if e.name.prefix.as_ref().is_some_and(|p| !p.is_empty()) {
        return None;
    }
    Some(&e.name.local)
}

fn control_kind(dom: &Dom, el: NodeId) -> Option<Control> {
    let tag = local_tag(dom, el)?;
    let attr = |name: &str| dom.get_attribute(el, name).ok().flatten();
    if tag.eq_ignore_ascii_case("input") {
        let ty = attr("type").unwrap_or("text").trim().to_ascii_lowercase();
        return Some(match ty.as_str() {
            "hidden" => return None,
            "checkbox" | "radio" => Control::Check,
            "range" => Control::Range,
            "color" => Control::Color,
            "file" => Control::File,
            "image" => return None,
            "button" | "submit" | "reset" => Control::Button(attr("value").map(str::to_owned).unwrap_or_else(|| {
                match ty.as_str() {
                    "submit" => "Submit",
                    "reset" => "Reset",
                    _ => "",
                }
                .to_owned()
            })),
            _ => Control::Text,
        });
    }
    if tag.eq_ignore_ascii_case("textarea") {
        return Some(Control::TextArea);
    }
    if tag.eq_ignore_ascii_case("select") {
        let size = attr("size").and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 1.0);
        let rows = size.or_else(|| attr("multiple").map(|_| 4.0));
        return Some(Control::Select { rows });
    }
    None
}

/// (ascent, descent) of the system UI font (flow_font.rs), the face flow
/// measures text with; the `normal` line height is their sum.
fn font_metrics(font_size: f64) -> (f64, f64) {
    crate::flow_font::vertical_metrics(font_size)
}

/// A text run's (ascent, descent) above/below the baseline, including
/// half-leading: the content area plus half the difference to `line-height`
/// on each side (`normal` = the content area).
fn text_extents(style: &FlowStyle) -> (f64, f64) {
    let (a, d) = font_metrics(style.font_size);
    match style.line_height {
        None => (a, d),
        Some(lh) => {
            // Chromium gives the top half of the leading floor(leading / 2)
            // and the rest to the bottom.
            let top = ((lh - a - d) / 2.0).floor();
            (a + top, lh - a - top)
        }
    }
}

/// A line box being filled.
struct Line {
    x: f64,
    /// Extents above and below the baseline. Either can be negative (a
    /// line height below the font's content area): they start unset.
    ascent: f64,
    descent: f64,
    items: Vec<LineItem>,
}

impl Default for Line {
    fn default() -> Self {
        Line { x: 0.0, ascent: f64::NEG_INFINITY, descent: f64::NEG_INFINITY, items: Vec::new() }
    }
}

struct LineItem {
    x: f64,
    width: f64,
    height: f64,
    /// An empty inline box that does not make a line box by itself.
    phantom: bool,
    ascent: f64,
    owners: Owners,
    atomic: Option<(NodeId, StyleRef)>,
    /// Text: the font's (ascent, descent) for its reported box.
    content: Option<(f64, f64)>,
    /// A generated inline-block: content even when empty (its line has a
    /// line box).
    generated: bool,
    /// A word of a text node: where it comes from (a text run).
    source: Option<WordSource>,
}

impl Line {
    fn push(
        &mut self,
        x: f64,
        width: f64,
        height: f64,
        ascent: f64,
        owners: Owners,
        atomic: Option<(NodeId, StyleRef)>,
    ) {
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(height - ascent);
        self.x = self.x.max(x + width);
        self.items.push(LineItem {
            x,
            width,
            height,
            phantom: false,
            ascent,
            owners,
            atomic,
            content: None,
            generated: false,
            source: None,
        });
    }

    /// The strut of the containing block's font.
    fn extents(&mut self, (ascent, descent): (f64, f64)) {
        self.ascent = self.ascent.max(ascent);
        self.descent = self.descent.max(descent);
    }
}

/// Content that fits its line within this much does not wrap: a box sized
/// to its max-content width must hold that content on one line despite
/// floating-point summation order.
const FIT_EPSILON: f64 = 1e-6;

/// Width of a run of text: the engine's fixed-advance measure, with a
/// no-break space as wide as a space (it does not collapse, but has the
/// same advance).
fn text_width(text: &str, font_size: f64, weight: u16) -> f64 {
    text.chars()
        .map(|c| match crate::flow_font::advance(c, font_size, weight) {
            Some(em) => em * font_size,
            // Outside the measured table (non-ASCII): the engine's measure.
            None => glyph_width(c.encode_utf8(&mut [0; 4]), font_size),
        })
        .sum()
}

/// Only collapsible white space (space, tab, line feeds) is blank; a
/// no-break space is content.
fn is_blank(text: &str) -> bool {
    text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
}

/// Widths of the pieces a too-long word breaks into for a `width` line: each
/// the longest prefix that fits (at least one character), the last the rest.
fn break_word(text: &str, font_size: f64, weight: u16, width: f64) -> Vec<f64> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (mut end, mut end_w) = (0, 0.0);
        for (i, c) in rest.char_indices() {
            let next = i + c.len_utf8();
            let w = text_width(&rest[..next], font_size, weight);
            if end > 0 && w > width + FIT_EPSILON {
                break;
            }
            (end, end_w) = (next, w);
        }
        pieces.push(end_w);
        rest = &rest[end..];
    }
    pieces
}

/// Min-content contribution of a text: its widest word, or with
/// `WrapMode::Anywhere` its widest character (a break may fall anywhere).
fn min_text_width(text: &str, font_size: f64, weight: u16, wrap: WrapMode) -> f64 {
    if wrap == WrapMode::Anywhere {
        text.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .map(|c| text_width(c.encode_utf8(&mut [0; 4]), font_size, weight))
            .fold(0.0, f64::max)
    } else {
        text.split_ascii_whitespace().map(|t| text_width(t, font_size, weight)).fold(0.0, f64::max)
    }
}

fn collapsed_width(text: &str, font_size: f64, weight: u16) -> f64 {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let space = text_width(" ", font_size, weight);
    words.iter().map(|w| text_width(w, font_size, weight)).sum::<f64>() + space * (words.len() - 1) as f64
}

fn union(frags: &mut FxHashMap<NodeId, Rect>, order: &mut Vec<NodeId>, el: NodeId, r: Rect) {
    match frags.get_mut(&el) {
        Some(u) => {
            let x0 = u.x.min(r.x);
            let y0 = u.y.min(r.y);
            let x1 = (u.x + u.width).max(r.x + r.width);
            let y1 = (u.y + u.height).max(r.y + r.height);
            *u = Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
        }
        None => {
            frags.insert(el, r);
            order.push(el);
        }
    }
}

// ----- flex layout (Stage 2) -------------------------------------------------------

/// One flex item: an element, or a run of text (anonymous item).
struct FlexItem {
    el: Option<NodeId>,
    text: Vec<NodeId>,
    style: StyleRef,
    margin: Sides<f64>,
    /// Border-box flex base size.
    base: f64,
    /// Border-box main size (hypothetical, then flexed).
    main: f64,
    min_main: f64,
    max_main: f64,
    /// Border-box cross size.
    cross: f64,
    stretched: bool,
}

impl<'a> Ctx<'a> {
    /// Lays out a flex container's content box at (x, y), `width` wide and
    /// `height` tall when definite; returns the content height.
    #[allow(clippy::too_many_arguments)]
    fn flex(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        height: Option<f64>,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
        let reverse = matches!(style.flex_direction, FlexDirection::RowReverse | FlexDirection::ColumnReverse);
        let main_gap = if row { style.column_gap.resolve(width) } else { style.row_gap.resolve(height.unwrap_or(0.0)) };
        let cross_gap =
            if row { style.row_gap.resolve(height.unwrap_or(0.0)) } else { style.column_gap.resolve(width) };
        let container_main = if row { Some(width) } else { height };
        let container_cross = if row { height } else { Some(width) };
        // Items in order; absolutely positioned children wait for their box.
        let mut items: Vec<FlexItem> = Vec::new();
        let mut text_run: Vec<NodeId> = Vec::new();
        let flush_text = |cx: &Ctx<'a>, run: &mut Vec<NodeId>, items: &mut Vec<FlexItem>| {
            let has_text = run.iter().any(|&t| cx.dom.char_data(t).is_ok_and(|d| !is_blank(d)));
            if has_text {
                let mut st =
                    FlowStyle { font_size: style.font_size, line_height: style.line_height, ..FlowStyle::default() };
                st.nowrap = style.nowrap;
                st.text_align = style.text_align;
                st.font_weight = style.font_weight;
                items.push(FlexItem {
                    el: None,
                    text: std::mem::take(run),
                    style: StyleRef::new(st),
                    margin: [0.0; 4],
                    base: 0.0,
                    main: 0.0,
                    min_main: 0.0,
                    max_main: f64::INFINITY,
                    cross: 0.0,
                    stretched: false,
                });
            } else {
                run.clear();
            }
        };
        for (child, child_style) in self.flatten_children(el) {
            match child_style {
                None => text_run.push(child),
                Some(s) if matches!(s.position, Position::Absolute | Position::Fixed) => {
                    self.defer_absolute(child, s, x - shift.0, y);
                }
                Some(s) => {
                    flush_text(self, &mut text_run, &mut items);
                    let e = self.edges(&s, width);
                    items.push(FlexItem {
                        el: Some(child),
                        text: Vec::new(),
                        margin: e.margin,
                        style: s,
                        base: 0.0,
                        main: 0.0,
                        min_main: 0.0,
                        max_main: f64::INFINITY,
                        cross: 0.0,
                        stretched: false,
                    });
                }
            }
        }
        flush_text(self, &mut text_run, &mut items);
        if reverse {
            items.reverse();
        }
        // Hypothetical main sizes (border boxes).
        for it in &mut items {
            let (main, min, max) = self.flex_base(it, row, width, container_main);
            // Hypothetical main size: the base size clamped by min/max.
            it.base = main;
            it.main = main.min(max).max(min);
            it.min_main = min;
            it.max_main = max;
        }
        // An auto-height column container's main size is its content clamped
        // by its own min/max height; items flex within that.
        let own = self.edges(style, width);
        let to_content_h = |v: f64| if style.border_box { (v - own.vertical()).max(0.0) } else { v };
        let own_min_h = style.min_height.resolve(self.own_cb_height).map(to_content_h);
        let own_max_h = style.max_height.resolve(self.own_cb_height).map(to_content_h);
        let mut container_main = container_main;
        if !row && container_main.is_none() && (own_min_h.is_some() || own_max_h.is_some()) {
            let n = items.len() as f64;
            let content: f64 =
                items.iter().map(|it| it.main + margins_main(it, row)).sum::<f64>() + main_gap * (n - 1.0).max(0.0);
            let clamped = Self::clamp(content, own_min_h, own_max_h);
            if clamped != content {
                container_main = Some(clamped);
            }
        }
        // Lines.
        let available = container_main.unwrap_or(f64::INFINITY);
        let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
        let mut start = 0;
        let mut used = 0.0;
        for (i, it) in items.iter().enumerate() {
            let outer = it.main + margins_main(it, row);
            let add = if i > start { main_gap } else { 0.0 };
            if style.flex_wrap && i > start && used + add + outer > available + FIT_EPSILON {
                lines.push(start..i);
                start = i;
                used = outer;
            } else {
                used += add + outer;
            }
        }
        if start < items.len() || items.is_empty() {
            lines.push(start..items.len());
        }
        // Resolve flexible lengths per line when the main size is definite.
        if let Some(cm) = container_main {
            for line in &lines {
                resolve_flexible_lengths(&mut items[line.clone()], cm, main_gap, row);
            }
        }
        // Cross sizes from content (for each item at its main size).
        for it in &mut items {
            it.cross = self.flex_cross_content(it, row, width);
        }
        // Line cross sizes.
        let single = lines.len() == 1;
        let mut line_cross: Vec<f64> = lines
            .iter()
            .map(|l| items[l.clone()].iter().map(|it| it.cross + margins_cross(it, row)).fold(0.0, f64::max))
            .collect();
        if single {
            match container_cross {
                Some(cc) => line_cross[0] = cc,
                // An auto-height row container is at least its min-height.
                None if row => line_cross[0] = Self::clamp(line_cross[0], own_min_h, own_max_h),
                None => {}
            }
        }
        // Stretch.
        for (li, line) in lines.iter().enumerate() {
            for it in &mut items[line.clone()] {
                let align = it.style.align_self.unwrap_or(style.align_items);
                // Only an `auto` cross size stretches (an intrinsic keyword
                // width such as fit-content does not).
                let declared_cross = if row {
                    it.style.height.resolve(height)
                } else if let Dim::Content(_) = it.style.width {
                    Some(0.0)
                } else {
                    it.style.width.resolve(Some(width))
                };
                // Auto cross-axis margins take precedence over stretching.
                if align == Align::Stretch && declared_cross.is_none() && auto_margins(it, row, false) == (false, false)
                {
                    let stretched = (line_cross[li] - margins_cross(it, row)).max(0.0);
                    it.cross = self.clamp_cross(it, row, width, stretched);
                    it.stretched = true;
                }
            }
        }
        // Place.
        let mut cross_pos = 0.0;
        let mut max_main_end: f64 = 0.0;
        for (li, line) in lines.iter().enumerate() {
            let n = line.len();
            let used: f64 = items[line.clone()].iter().map(|it| it.main + margins_main(it, row)).sum::<f64>()
                + main_gap * (n as f64 - 1.0).max(0.0);
            let free = container_main.map_or(0.0, |cm| (cm - used).max(0.0));
            // Auto main-axis margins absorb the free space before
            // justify-content (CSS Flexbox 8.1).
            let autos: usize = items[line.clone()]
                .iter()
                .map(|it| {
                    let (a, b) = auto_margins(it, row, true);
                    a as usize + b as usize
                })
                .sum();
            let per_auto = if autos > 0 { free / autos as f64 } else { 0.0 };
            let justify = if autos > 0 { Justify::Start } else { style.justify_content };
            let (mut pos, between) = match justify {
                Justify::Start => (0.0, 0.0),
                Justify::End => (free, 0.0),
                Justify::Center => (free / 2.0, 0.0),
                Justify::SpaceBetween if n > 1 => (0.0, free / (n as f64 - 1.0)),
                Justify::SpaceBetween => (0.0, 0.0),
                Justify::SpaceAround => (free / n as f64 / 2.0, free / n as f64),
                Justify::SpaceEvenly => (free / (n as f64 + 1.0), free / (n as f64 + 1.0)),
            };
            for i in line.clone() {
                let it = &items[i];
                let align = it.style.align_self.unwrap_or(style.align_items);
                let outer_cross = it.cross + margins_cross(it, row);
                let spare = line_cross[li] - outer_cross;
                let cross_off = match auto_margins(it, row, false) {
                    (true, true) => (spare / 2.0).max(0.0),
                    (true, false) => spare.max(0.0),
                    (false, true) => 0.0,
                    (false, false) => match align {
                        Align::End => spare,
                        Align::Center => spare / 2.0,
                        _ => 0.0,
                    },
                };
                let (auto_before, auto_after) = auto_margins(it, row, true);
                if auto_before {
                    pos += per_auto;
                }
                let (mx, my) =
                    if row { (x + pos, y + cross_pos + cross_off) } else { (x + cross_pos + cross_off, y + pos) };
                let (w, h) = if row { (it.main, it.cross) } else { (it.cross, it.main) };
                self.place_flex_item(i, &items, mx, my, w, h, width, fixed, shift);
                pos += it.main + margins_main(it, row) + main_gap + between;
                if auto_after {
                    pos += per_auto;
                }
            }
            max_main_end = max_main_end.max(pos - main_gap - between);
            cross_pos += line_cross[li] + cross_gap;
        }
        let cross_extent = (cross_pos - cross_gap).max(0.0);
        if row {
            cross_extent
        } else {
            height.unwrap_or(max_main_end)
        }
    }

    /// (hypothetical border-box main size, min, max).
    fn flex_base(&self, it: &FlexItem, row: bool, width: f64, container_main: Option<f64>) -> (f64, f64, f64) {
        let s = &it.style;
        let e = self.edges(s, width);
        let edges = if row { e.horizontal() } else { e.vertical() };
        let to_border = |v: f64| if s.border_box { v.max(edges) } else { v + edges };
        let declared = if row { s.width.resolve(Some(width)) } else { s.height.resolve(container_main) };
        // A percentage flex-basis against an indefinite container main size
        // is `content` (css-flexbox §7.2.3): the content size, whatever the
        // item's own width/height (Chromium; e.g. `flex: 1` in an auto-height
        // column).
        let content_basis = s.flex_basis_percent && container_main.is_none();
        let basis = match s.flex_basis {
            Dim::Len(l) if !content_basis && (l.percent == 0.0 || container_main.is_some()) => {
                Some(l.resolve(container_main.unwrap_or(0.0)))
            }
            _ => None,
        };
        let declared_basis = if content_basis { None } else { declared };
        let main = match (basis, declared_basis) {
            (Some(b), _) => to_border(b),
            (None, Some(d)) => to_border(d),
            (None, None) => {
                if row {
                    let content = match &it.el {
                        Some(el) => self.max_content(*el, s),
                        None => it
                            .text
                            .iter()
                            .map(|&t| collapsed_width(self.dom.char_data(t).unwrap_or(""), s.font_size, s.font_weight))
                            .sum(),
                    };
                    content + edges
                } else {
                    let cross_w = self.flex_cross_width(it, width);
                    let content = match &it.el {
                        Some(el) => self.measure_height(*el, s, cross_w),
                        None => text_extents(s).0 + text_extents(s).1,
                    };
                    content + edges
                }
            }
        };
        let (min, max) = if row {
            (s.min_width.resolve(Some(width)), s.max_width.resolve(Some(width)))
        } else {
            (s.min_height.resolve(container_main), s.max_height.resolve(container_main))
        };
        let max = max.map_or(f64::INFINITY, to_border);
        let min = match min {
            Some(m) => to_border(m),
            // `min-*: auto`: an item that does not clip its overflow cannot
            // shrink below its content (min-content width / content height),
            // capped by its specified size.
            None if (row && !s.clips_x) || (!row && !s.clips_y) => {
                // Text inputs, textareas and selects with a percentage width
                // are compressible (CSS Sizing 3 §5.2.2): no content-based
                // minimum, so `width:100%; flex:1` shrinks into the rest of
                // a flex line instead of wrapping (Chromium; TOS select
                // chips beside their search input).
                let compressible = row
                    && matches!(s.width, Dim::Len(l) if l.percent != 0.0)
                    && it.el.is_some_and(|el| {
                        matches!(
                            control_kind(self.dom, el),
                            Some(Control::Text | Control::TextArea | Control::Select { .. })
                        )
                    });
                let content = match (&it.el, row) {
                    _ if compressible => 0.0,
                    (Some(el), true) => self.min_content(*el, s),
                    (Some(el), false) => self.measure_height(*el, s, self.flex_cross_width(it, width)),
                    (None, true) => it
                        .text
                        .iter()
                        .map(|&t| {
                            min_text_width(self.dom.char_data(t).unwrap_or(""), s.font_size, s.font_weight, s.wrap)
                        })
                        .fold(0.0, f64::max),
                    (None, false) => text_extents(s).0 + text_extents(s).1,
                };
                let content = content + edges;
                declared.map_or(content, |d| content.min(to_border(d))).min(max)
            }
            None => 0.0,
        };
        (main, min, max)
    }

    /// Content-box width of an item in a column container before stretching.
    fn flex_cross_width(&self, it: &FlexItem, width: f64) -> f64 {
        let s = &it.style;
        let e = self.edges(s, width);
        match s.width.resolve(Some(width)) {
            Some(w) => Self::declared_content(s, w, e.horizontal()),
            None => (width - e.margin[LEFT] - e.margin[RIGHT] - e.horizontal()).max(0.0),
        }
    }

    /// The item's content-based border-box cross size at its main size,
    /// clamped by its min/max cross size.
    fn flex_cross_content(&self, it: &FlexItem, row: bool, width: f64) -> f64 {
        self.clamp_cross(it, row, width, self.flex_cross_unclamped(it, row, width))
    }

    /// Clamps a border-box cross size by the item's min/max height (row) or
    /// width (column). Percentages resolve against the containing block's
    /// definite size, if any.
    fn clamp_cross(&self, it: &FlexItem, row: bool, width: f64, cross: f64) -> f64 {
        let s = &it.style;
        let e = self.edges(s, width);
        let (edges, basis) = if row { (e.vertical(), self.cb_height) } else { (e.horizontal(), Some(width)) };
        let to_border = |v: f64| if s.border_box { v.max(edges) } else { v + edges };
        let (min, max) = if row { (s.min_height, s.max_height) } else { (s.min_width, s.max_width) };
        let max = max.resolve(basis).map(to_border);
        let min = min.resolve(basis).map(to_border);
        Self::clamp(cross, min, max)
    }

    fn flex_cross_unclamped(&self, it: &FlexItem, row: bool, width: f64) -> f64 {
        let s = &it.style;
        let e = self.edges(s, width);
        if row {
            if let Some(h) = s.height.resolve(self.cb_height) {
                return if s.border_box { h.max(e.vertical()) } else { h + e.vertical() };
            }
            let content_w = (it.main - e.horizontal()).max(0.0);
            let h = match &it.el {
                Some(el) => self.measure_height(*el, s, content_w),
                None => text_extents(s).0 + text_extents(s).1,
            };
            h + e.vertical()
        } else {
            match s.width.resolve(Some(width)) {
                Some(w) => {
                    if s.border_box {
                        w.max(e.horizontal())
                    } else {
                        w + e.horizontal()
                    }
                }
                None => {
                    let available = (width - e.margin[LEFT] - e.margin[RIGHT]).max(0.0);
                    if let (Dim::Content(_), Some(el)) = (s.width, &it.el) {
                        // width: min-/max-/fit-content in a column container.
                        let avail_content = (available - e.horizontal()).max(0.0);
                        return self.intrinsic_width(*el, s, avail_content) + e.horizontal();
                    }
                    let content = match &it.el {
                        Some(el) => self.max_content(*el, s),
                        None => it
                            .text
                            .iter()
                            .map(|&t| collapsed_width(self.dom.char_data(t).unwrap_or(""), s.font_size, s.font_weight))
                            .sum(),
                    };
                    (content + e.horizontal()).min(available)
                }
            }
        }
    }

    /// Lays out item `i` with its border box at (x, y) + margins, sized w×h.
    #[allow(clippy::too_many_arguments)]
    fn place_flex_item(
        &mut self,
        i: usize,
        items: &[FlexItem],
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        cb_width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) {
        let it = &items[i];
        let Some(el) = it.el else {
            // Anonymous text item: an inline run in its box.
            let parent = it.style.clone();
            self.inline_run(&it.text, &parent, x, y, w.max(1.0), fixed, shift);
            return;
        };
        let e = self.edges(&it.style, cb_width);
        let mut s = (*it.style).clone();
        s.border_box = true;
        s.width = Dim::px(w);
        s.height = Dim::px(h);
        s.min_width = Dim::Auto;
        s.max_width = Dim::Auto;
        s.min_height = Dim::Auto;
        s.max_height = Dim::Auto;
        s.margin =
            [Dim::px(e.margin[TOP]), Dim::px(e.margin[RIGHT]), Dim::px(e.margin[BOTTOM]), Dim::px(e.margin[LEFT])];
        // A flex item's position is (x, y); block_child adds the margins.
        self.block_child(el, &s, x, y, w + e.margin[LEFT] + e.margin[RIGHT], fixed, shift);
    }
}

/// CSS Flexbox 9.7: grow or shrink unfrozen items from their base sizes,
/// freezing min/max violators until the line's free space is distributed.
fn resolve_flexible_lengths(items: &mut [FlexItem], container_main: f64, gap: f64, row: bool) {
    let n = items.len();
    if n == 0 {
        return;
    }
    let gaps = gap * (n - 1) as f64;
    let margins: f64 = items.iter().map(|it| margins_main(it, row)).sum();
    let hypothetical: f64 = items.iter().map(|it| it.main).sum();
    let grow = hypothetical + margins + gaps < container_main;
    let mut target: Vec<f64> = items.iter().map(|it| it.main).collect();
    let mut frozen: Vec<bool> = items
        .iter()
        .map(|it| {
            let factor = if grow { it.style.flex_grow } else { it.style.flex_shrink };
            factor == 0.0 || (grow && it.base > it.main) || (!grow && it.base < it.main)
        })
        .collect();
    for _ in 0..=n {
        if frozen.iter().all(|f| *f) {
            break;
        }
        let used: f64 = items.iter().zip(&frozen).zip(&target).map(|((it, f), t)| if *f { *t } else { it.base }).sum();
        let free = container_main - gaps - margins - used;
        if grow {
            let sum: f64 = items.iter().zip(&frozen).filter(|(_, f)| !**f).map(|(it, _)| it.style.flex_grow).sum();
            // Factors summing below 1 take only that fraction of the space.
            let free = if sum < 1.0 { free * sum } else { free };
            for (i, it) in items.iter().enumerate() {
                if !frozen[i] && sum > 0.0 {
                    target[i] = it.base + free * it.style.flex_grow / sum;
                }
            }
        } else {
            let sum: f64 =
                items.iter().zip(&frozen).filter(|(_, f)| !**f).map(|(it, _)| it.style.flex_shrink * it.base).sum();
            for (i, it) in items.iter().enumerate() {
                if !frozen[i] {
                    target[i] = if sum > 0.0 { it.base + free * it.style.flex_shrink * it.base / sum } else { it.base };
                }
            }
        }
        let mut total = 0.0;
        let mut violation = vec![0.0; n];
        for (i, it) in items.iter().enumerate() {
            if !frozen[i] {
                let clamped = target[i].min(it.max_main).max(it.min_main).max(0.0);
                violation[i] = clamped - target[i];
                total += violation[i];
                target[i] = clamped;
            }
        }
        for i in 0..n {
            if !frozen[i]
                && (total == 0.0 || (total > 0.0 && violation[i] > 0.0) || (total < 0.0 && violation[i] < 0.0))
            {
                frozen[i] = true;
            }
        }
    }
    for (it, t) in items.iter_mut().zip(target) {
        it.main = t;
    }
}

/// Whether the item's (start, end) margins on the main (`main`) or cross
/// axis are `auto`. Text items have none.
fn auto_margins(it: &FlexItem, row: bool, main: bool) -> (bool, bool) {
    if it.el.is_none() {
        return (false, false);
    }
    let (a, b) = if row == main { (LEFT, RIGHT) } else { (TOP, BOTTOM) };
    (it.style.margin[a] == Dim::Auto, it.style.margin[b] == Dim::Auto)
}

fn margins_main(it: &FlexItem, row: bool) -> f64 {
    if row {
        it.margin[LEFT] + it.margin[RIGHT]
    } else {
        it.margin[TOP] + it.margin[BOTTOM]
    }
}

fn margins_cross(it: &FlexItem, row: bool) -> f64 {
    if row {
        it.margin[TOP] + it.margin[BOTTOM]
    } else {
        it.margin[LEFT] + it.margin[RIGHT]
    }
}

// ----- table layout (Stage 3) -------------------------------------------------------

struct TableCell {
    el: NodeId,
    style: StyleRef,
    col: usize,
    span: usize,
    /// Rows the cell spans (at least 1, at most the rest of its row group).
    rowspan: usize,
}

struct TableRow {
    el: Option<NodeId>,
    style: Option<StyleRef>,
    group: Option<NodeId>,
    cells: Vec<TableCell>,
}

impl<'a> Ctx<'a> {
    /// Rows in display order (header groups, then bodies/bare rows, then
    /// footers), each with its cells' column positions.
    /// The table's rows in display order and its column count, memoized
    /// for the pass (the table and its measurements ask several times).
    fn table_rows(&self, table: NodeId) -> std::rc::Rc<(Vec<TableRow>, usize)> {
        if let Some(t) = self.memo.tables.borrow().get(&table) {
            return t.clone();
        }
        let t = std::rc::Rc::new(self.table_rows_uncached(table));
        self.memo.tables.borrow_mut().insert(table, t.clone());
        t
    }

    fn table_rows_uncached(&self, table: NodeId) -> (Vec<TableRow>, usize) {
        let mut head = Vec::new();
        let mut body = Vec::new();
        let mut foot = Vec::new();
        for (child, style) in self.flatten_children(table) {
            let Some(style) = style else { continue };
            match style.table_part {
                TablePart::HeaderGroup | TablePart::BodyGroup | TablePart::FooterGroup => {
                    let rows: Vec<TableRow> = self
                        .flatten_children(child)
                        .into_iter()
                        .filter_map(|(r, rs)| {
                            let rs = rs?;
                            (rs.table_part == TablePart::Row).then(|| self.table_row(r, rs, Some(child)))
                        })
                        .collect();
                    match style.table_part {
                        TablePart::HeaderGroup => head.extend(rows),
                        TablePart::FooterGroup => foot.extend(rows),
                        _ => body.extend(rows),
                    }
                }
                TablePart::Row => body.push(self.table_row(child, style, None)),
                _ => {}
            }
        }
        let mut rows: Vec<TableRow> = head.into_iter().chain(body).chain(foot).collect();
        // Slots: each row group (consecutive rows directly in the table form
        // one) places its cells in the first columns not taken by a cell
        // spanning down from a row above (HTML table processing model).
        let mut start = 0;
        while start < rows.len() {
            let end = start + rows[start..].iter().take_while(|r| r.group == rows[start].group).count();
            let mut taken: Vec<usize> = Vec::new();
            for r in start..end {
                let mut col = 0;
                for c in rows[r].cells.iter_mut() {
                    while taken.get(col).is_some_and(|&n| n > 0) {
                        col += 1;
                    }
                    c.col = col;
                    c.rowspan = c.rowspan.min(end - r);
                    if taken.len() < col + c.span {
                        taken.resize(col + c.span, 0);
                    }
                    for t in &mut taken[col..col + c.span] {
                        *t = (*t).max(c.rowspan);
                    }
                    col += c.span;
                }
                for t in &mut taken {
                    *t = t.saturating_sub(1);
                }
            }
            start = end;
        }
        let cols = rows.iter().map(|r| r.cells.iter().map(|c| c.col + c.span).max().unwrap_or(0)).max().unwrap_or(0);
        (rows, cols)
    }

    fn table_row(&self, row: NodeId, style: StyleRef, group: Option<NodeId>) -> TableRow {
        let mut col = 0;
        let mut cells = Vec::new();
        for (c, cs) in self.flatten_children(row) {
            let Some(cs) = cs else { continue };
            if cs.table_part != TablePart::Cell {
                continue;
            }
            let span = self
                .dom
                .get_attribute(c, "colspan")
                .ok()
                .flatten()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&n| n >= 1)
                .unwrap_or(1)
                .min(1000);
            // rowspan=0: to the end of the row group.
            let rowspan = self
                .dom
                .get_attribute(c, "rowspan")
                .ok()
                .flatten()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map_or(1, |n| if n == 0 { usize::MAX } else { n.min(65534) });
            cells.push(TableCell { el: c, style: cs, col, span, rowspan });
            col += span;
        }
        TableRow { el: Some(row), style: Some(style), group, cells }
    }

    fn spacing(style: &FlowStyle) -> (f64, f64) {
        if style.border_collapse {
            (0.0, 0.0)
        } else {
            style.border_spacing
        }
    }

    /// A cell's border-box width from its declared width, or its
    /// max-content width when `content` is set.
    fn cell_width(&self, cell: &TableCell, basis: f64, content: bool) -> Option<f64> {
        let e = self.edges(&cell.style, basis);
        match cell.style.width.resolve(Some(basis)) {
            Some(w) => Some(if cell.style.border_box { w.max(e.horizontal()) } else { w + e.horizontal() }),
            None if content => Some(self.max_content(cell.el, &cell.style) + e.horizontal()),
            None => None,
        }
    }

    /// A border-box cell width clamped by the cell's `min-width` and
    /// `max-width`, as an automatic-layout column takes them (`min-width`
    /// wins). Chromium sizes the column to the clamped cell, so a cell is
    /// never wider than its columns; fixed layout ignores both.
    fn clamp_cell_width(&self, cell: &TableCell, basis: f64, w: f64) -> f64 {
        let e = self.edges(&cell.style, basis);
        let border = |v: f64| if cell.style.border_box { v.max(e.horizontal()) } else { v + e.horizontal() };
        let max = cell.style.max_width.resolve(Some(basis)).map(border);
        let min = cell.style.min_width.resolve(Some(basis)).map(border);
        let w = max.map_or(w, |m| w.min(m));
        min.map_or(w, |m| w.max(m))
    }

    /// Column border-box widths for a table whose content box is `width`
    /// (None: shrink to the columns).
    fn table_columns(
        &self,
        el: NodeId,
        table: &FlowStyle,
        rows: &[TableRow],
        cols: usize,
        width: Option<f64>,
    ) -> Vec<f64> {
        let (sx, _) = Self::spacing(table);
        let spacing = sx * (cols as f64 + 1.0);
        let basis = width.unwrap_or(0.0) - spacing;
        let mut declared: Vec<Option<f64>> = vec![None; cols];
        let mut widths = vec![0.0; cols];
        if table.table_layout_fixed && width.is_some() {
            declared = self.fixed_column_widths(el, rows, cols, basis);
            for (i, d) in declared.iter().enumerate() {
                widths[i] = d.unwrap_or(0.0);
            }
        } else {
            for row in rows {
                for c in row.cells.iter().filter(|c| c.span == 1) {
                    if let Some(d) = self.cell_width(c, basis, false) {
                        let d = self.clamp_cell_width(c, basis, d);
                        declared[c.col] = Some(declared[c.col].map_or(d, |p: f64| p.max(d)));
                    }
                    let w = self.clamp_cell_width(c, basis, self.cell_width(c, basis, true).unwrap_or(0.0));
                    widths[c.col] = f64::max(widths[c.col], w);
                }
            }
            for (i, d) in declared.iter().enumerate() {
                if let Some(d) = d {
                    widths[i] = widths[i].max(*d);
                }
            }
        }
        if let Some(w) = width {
            let used: f64 = widths.iter().sum::<f64>() + spacing;
            let free = w - used;
            if free > 0.0 {
                let auto: Vec<usize> = (0..cols).filter(|&i| declared[i].is_none()).collect();
                if table.table_layout_fixed && !auto.is_empty() {
                    for &i in &auto {
                        widths[i] += free / auto.len() as f64;
                    }
                } else {
                    let targets: Vec<usize> = if auto.is_empty() { (0..cols).collect() } else { auto };
                    let total: f64 = targets.iter().map(|&i| widths[i]).sum();
                    for &i in &targets {
                        let share = if total > 0.0 { widths[i] / total } else { 1.0 / targets.len() as f64 };
                        widths[i] += free * share;
                    }
                }
            }
        }
        widths
    }

    /// Max-content width of a table's content box.
    fn table_max_content(&self, table: NodeId, style: &FlowStyle) -> f64 {
        let memo_rows = self.table_rows(table);
        let (rows, cols) = (memo_rows.0.as_slice(), memo_rows.1);
        let (sx, _) = Self::spacing(style);
        let content = self.table_columns(table, style, &rows, cols, None).iter().sum::<f64>();
        // A fixed-layout table is at least its declared columns.
        let fixed = if style.table_layout_fixed { self.fixed_columns_sum(table, &rows, cols) } else { 0.0 };
        content.max(fixed) + sx * (cols as f64 + 1.0)
    }

    /// Declared widths of a fixed-layout table's columns (CSS 2.1 17.5.2.1):
    /// `<col>`/`<colgroup>` widths (with `span`) first, then the first row's
    /// cells, a spanning cell's width split evenly over its columns. None:
    /// auto (shares what the table has left).
    fn fixed_column_widths(&self, table: NodeId, rows: &[TableRow], cols: usize, basis: f64) -> Vec<Option<f64>> {
        let mut declared: Vec<Option<f64>> = vec![None; cols];
        let span_of = |el: NodeId| {
            self.dom
                .get_attribute(el, "span")
                .ok()
                .flatten()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(1)
                .min(1000)
        };
        let mut next = 0usize;
        let mut assign = |declared: &mut Vec<Option<f64>>, n: usize, w: Option<f64>| {
            for d in declared.iter_mut().skip(next).take(n) {
                *d = w;
            }
            next += n;
        };
        for (child, style) in self.flatten_children(table) {
            let Some(style) = style else { continue };
            match style.table_part {
                TablePart::ColumnGroup => {
                    let group_width = style.width.resolve(Some(basis));
                    let columns: Vec<(NodeId, StyleRef)> = self
                        .flatten_children(child)
                        .into_iter()
                        .filter_map(|(c, s)| s.filter(|s| s.table_part == TablePart::Column).map(|s| (c, s)))
                        .collect();
                    if columns.is_empty() {
                        assign(&mut declared, span_of(child), group_width);
                    }
                    for (c, s) in columns {
                        assign(&mut declared, span_of(c), s.width.resolve(Some(basis)).or(group_width));
                    }
                }
                TablePart::Column => assign(&mut declared, span_of(child), style.width.resolve(Some(basis))),
                _ => {}
            }
        }
        if let Some(first) = rows.first() {
            for c in &first.cells {
                let Some(w) = self.cell_width(c, basis, false) else { continue };
                let share = w / c.span as f64;
                for d in declared.iter_mut().skip(c.col).take(c.span) {
                    if d.is_none() {
                        *d = Some(share);
                    }
                }
            }
        }
        declared
    }

    /// The declared columns of a fixed-layout table, summed (auto: 0).
    fn fixed_columns_sum(&self, table: NodeId, rows: &[TableRow], cols: usize) -> f64 {
        self.fixed_column_widths(table, rows, cols, 0.0).iter().map(|d| d.unwrap_or(0.0)).sum()
    }

    /// Min-content width of a table's content box: each column at least its
    /// cells' declared widths and min-content widths (spanning cells aside,
    /// as for the max-content columns). A fit-content box around a table
    /// shrinks to this when its container is narrower (TOS finance tables:
    /// columns sized by their headers, wider text cells wrapping).
    fn table_min_content(&self, table: NodeId, style: &FlowStyle) -> f64 {
        let memo_rows = self.table_rows(table);
        let (rows, cols) = (memo_rows.0.as_slice(), memo_rows.1);
        let (sx, _) = Self::spacing(style);
        if style.table_layout_fixed {
            // Fixed layout ignores cell contents: the declared columns.
            return self.fixed_columns_sum(table, &rows, cols) + sx * (cols as f64 + 1.0);
        }
        let mut widths = vec![0.0f64; cols];
        for row in rows {
            for c in row.cells.iter().filter(|c| c.span == 1) {
                let e = self.edges(&c.style, 0.0);
                let content = self.clamp_cell_width(c, 0.0, self.min_content(c.el, &c.style) + e.horizontal());
                let declared = self.cell_width(c, 0.0, false).map_or(0.0, |d| self.clamp_cell_width(c, 0.0, d));
                widths[c.col] = widths[c.col].max(content).max(declared);
            }
        }
        widths.iter().sum::<f64>() + sx * (cols as f64 + 1.0)
    }

    /// Lays out a table's rows and cells in its content box; returns the
    /// content height.
    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let memo_rows = self.table_rows(el);
        let (rows, cols) = (memo_rows.0.as_slice(), memo_rows.1);
        if cols == 0 {
            return 0.0;
        }
        let (sx, sy) = Self::spacing(style);
        let widths = self.table_columns(el, style, &rows, cols, Some(width));
        let mut starts = Vec::with_capacity(cols + 1);
        let mut cx = x + sx;
        for w in &widths {
            starts.push(cx);
            cx += w + sx;
        }
        starts.push(cx);
        // Row heights: the tallest single-row cell (and the row's own
        // height); a spanning cell taller than its rows grows them in
        // proportion to their heights (all into the last when they are
        // empty), as Chromium does.
        let mut cell_heights: Vec<Vec<(f64, f64, f64)>> = Vec::with_capacity(rows.len());
        let mut row_hs: Vec<f64> = Vec::with_capacity(rows.len());
        for row in rows {
            let mut row_h = row.style.as_ref().and_then(|s| s.height.resolve(None)).unwrap_or(0.0);
            let mut hs = Vec::with_capacity(row.cells.len());
            for c in &row.cells {
                let w = span_width(&widths, c, sx);
                let e = self.edges(&c.style, width);
                let content_w = (w - e.horizontal()).max(0.0);
                let content_h = self.measure_height(c.el, &c.style, content_w);
                let declared =
                    c.style
                        .height
                        .resolve(None)
                        .map(|h| if c.style.border_box { (h - e.vertical()).max(0.0) } else { h });
                let h = declared.unwrap_or(0.0).max(content_h) + e.vertical();
                hs.push((content_h, e.vertical(), h));
                if c.rowspan == 1 {
                    row_h = row_h.max(h);
                }
            }
            cell_heights.push(hs);
            row_hs.push(row_h);
        }
        for (r, row) in rows.iter().enumerate() {
            for (c, &(_, _, h)) in row.cells.iter().zip(&cell_heights[r]) {
                if c.rowspan < 2 {
                    continue;
                }
                let spanned = &mut row_hs[r..r + c.rowspan];
                let have: f64 = spanned.iter().sum();
                let extra = h - have - sy * (c.rowspan - 1) as f64;
                if extra <= 0.0 {
                    continue;
                }
                if have > 0.0 {
                    for rh in spanned.iter_mut() {
                        *rh += extra * *rh / have;
                    }
                } else if let Some(last) = spanned.last_mut() {
                    *last += extra;
                }
            }
        }
        let mut cursor = y + sy;
        let mut groups: Vec<(NodeId, Rect)> = Vec::new();
        let mut group_orders: Vec<(NodeId, u32)> = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let row_h = row_hs[r];
            // A row group precedes its rows (and cells) in paint order.
            if let Some(g) = row.group {
                if !group_orders.iter().any(|(n, _)| *n == g) {
                    let order = self.next_order();
                    group_orders.push((g, order));
                }
            }
            let row_order = self.next_order();
            for (c, &(content_h, vertical, _)) in row.cells.iter().zip(&cell_heights[r]) {
                let w = span_width(&widths, c, sx);
                let cell_h = row_hs[r..r + c.rowspan].iter().sum::<f64>() + sy * (c.rowspan - 1) as f64;
                let mut s = (*c.style).clone();
                s.border_box = true;
                s.width = Dim::px(w);
                s.height = Dim::px(cell_h);
                s.margin = [Dim::px(0.0); 4];
                // The columns already took the cell's min/max width.
                s.min_width = Dim::Auto;
                s.max_width = Dim::Auto;
                s.min_height = Dim::Auto;
                s.max_height = Dim::Auto;
                let before = self.order;
                self.block_child(c.el, &s, starts[c.col], cursor, w, fixed, shift);
                // Cell content is vertically centred (rows default to middle).
                let dy = ((cell_h - vertical - content_h) / 2.0).max(0.0);
                if dy > 0.0 {
                    let cell_order = before + 1;
                    self.boxes_after(cell_order, |b| b.rect.y += dy);
                    self.shift_texts_after(cell_order, dy);
                }
            }
            let rect = Rect {
                x: starts[0] + shift.0,
                y: cursor + shift.1,
                width: starts[cols] - sx - starts[0],
                height: row_h,
            };
            if let Some(r) = row.el {
                self.insert_box(r, simple_box(rect, fixed, row_order));
            }
            if let Some(g) = row.group {
                match groups.iter_mut().find(|(n, _)| *n == g) {
                    Some((_, u)) => {
                        let bottom = (u.y + u.height).max(rect.y + rect.height);
                        u.height = bottom - u.y;
                    }
                    None => groups.push((g, rect)),
                }
            }
            cursor += row_h + sy;
        }
        for (g, rect) in groups {
            let order = group_orders.iter().find(|(n, _)| *n == g).map_or_else(|| self.next_order(), |(_, o)| *o);
            self.insert_box(g, simple_box(rect, fixed, order));
        }
        cursor - y
    }
}

/// A cell's border-box width: its columns' widths and the spacing between
/// them, summed directly. The difference of the accumulated column starts
/// loses exactness (a 150px column after fractional ones: 149.99999999999994),
/// where Chromium's columns keep their width.
fn span_width(widths: &[f64], cell: &TableCell, sx: f64) -> f64 {
    widths[cell.col..cell.col + cell.span].iter().sum::<f64>() + sx * (cell.span - 1) as f64
}

fn simple_box(rect: Rect, fixed: bool, order: u32) -> FlowBox {
    FlowBox {
        rect,
        fixed,
        client_width: rect.width,
        client_height: rect.height,
        scroll_width: rect.width,
        scroll_height: rect.height,
        positioned: false,
        order,
        clipped_by: 0,
    }
}

// ----- hit testing ------------------------------------------------------------

impl FlowResult {
    /// Topmost element at document point (x, y) for which `hit` allows hit
    /// testing: positioned boxes paint above in-flow ones, later boxes above
    /// earlier. `clip` reports whether an ancestor clipping overflow excludes
    /// the point.
    pub fn element_at(
        &self,
        x: f64,
        y: f64,
        scroll: (f64, f64),
        hit: impl Fn(NodeId) -> bool,
        clipped: impl Fn(NodeId, f64, f64) -> bool,
    ) -> Option<NodeId> {
        let mut best: Option<(bool, u32, NodeId)> = None;
        for (&el, b) in &self.boxes {
            let (px, py) = if b.fixed { (x - scroll.0, y - scroll.1) } else { (x, y) };
            let r = b.rect;
            if px < r.x || py < r.y || px > r.x + r.width || py > r.y + r.height || r.width <= 0.0 || r.height <= 0.0 {
                continue;
            }
            if !hit(el) || clipped(el, x, y) {
                continue;
            }
            let key = (b.positioned, b.order, el);
            if best.as_ref().is_none_or(|k| (k.0, k.1) < (key.0, key.1)) {
                best = Some(key);
            }
        }
        best.map(|k| k.2)
    }
}

// ----- grid layout (Stage 4) ------------------------------------------------

struct GridItem {
    /// None: an anonymous item for a run of text.
    el: Option<NodeId>,
    text: Vec<NodeId>,
    style: StyleRef,
    col: usize,
    col_span: usize,
    row: usize,
    row_span: usize,
}

/// A sized axis: track sizes, which tracks collapsed (auto-fit), the gap.
struct Axis {
    sizes: Vec<f64>,
    collapsed: Vec<bool>,
    gap: f64,
}

impl Axis {
    /// Start offset of each track and the end of the last one.
    fn starts(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.sizes.len() + 1);
        let mut pos = 0.0;
        let mut first = true;
        for (i, s) in self.sizes.iter().enumerate() {
            if !self.collapsed[i] {
                if !first {
                    pos += self.gap;
                }
                first = false;
            }
            out.push(pos);
            pos += s;
        }
        out.push(pos);
        out
    }

    fn total(&self) -> f64 {
        *self.starts().last().unwrap_or(&0.0)
    }

    /// Size of the area spanning `span` tracks from `start` (gaps included).
    fn area(&self, starts: &[f64], start: usize, span: usize) -> f64 {
        let last = start + span.max(1) - 1;
        (starts[last] + self.sizes[last] - starts[start]).max(0.0)
    }
}

/// Expands a track list for `available` space (auto-repeat count).
/// Returns the tracks and, for auto-fit, the index range of the repetitions.
fn expand_tracks(list: &TrackList, available: Option<f64>, gap: f64) -> (Vec<Track>, Option<std::ops::Range<usize>>) {
    let Some(rep) = &list.auto_repeat else { return (list.tracks.clone(), None) };
    let fixed = |b: TrackBreadth| b.length(available);
    let size = |t: &Track| fixed(t.max).or_else(|| fixed(t.min));
    let rep_sizes: Option<Vec<f64>> = rep.tracks.iter().map(size).collect();
    let mut count = 1usize;
    if let (Some(avail), Some(rep_sizes)) = (available, rep_sizes) {
        let others: f64 = list.tracks.iter().map(|t| size(t).unwrap_or(0.0)).sum();
        let rep_sum: f64 = rep_sizes.iter().sum();
        let n = list.tracks.len() + rep.tracks.len();
        if rep_sum + gap > 0.0 {
            let total = |k: usize| others + k as f64 * rep_sum + (n + (k - 1) * rep.tracks.len() - 1) as f64 * gap;
            while count < 1000 && total(count + 1) <= avail {
                count += 1;
            }
        }
    }
    let mut out = list.tracks[..rep.at].to_vec();
    let from = out.len();
    for _ in 0..count {
        out.extend_from_slice(&rep.tracks);
    }
    let to = out.len();
    out.extend_from_slice(&list.tracks[rep.at..]);
    (out, rep.fit.then_some(from..to))
}

/// Resolves a placement against `lines` explicit lines: (start index or
/// auto, span).
fn resolve_lines(p: GridPlacement, lines: usize) -> (Option<usize>, usize) {
    let lines = lines as i32;
    let index = |n: i32| if n > 0 { n - 1 } else { (lines + n).max(0) };
    let span = p.span.max(1) as usize;
    match (p.start, p.end) {
        (Some(s), Some(e)) => {
            let (a, b) = (index(s), index(e));
            let (a, b) = if b < a { (b, a) } else { (a, b) };
            (Some(a as usize), ((b - a).max(1)) as usize)
        }
        (Some(s), None) => (Some(index(s) as usize), span),
        (None, Some(e)) => (Some((index(e) - span as i32).max(0) as usize), span),
        (None, None) => (None, span),
    }
}

/// A resolved placement on one axis: (start index or auto, span).
type Lines = (Option<usize>, usize);

/// Places items on the grid (auto-placement, sparse). Returns the column
/// and row counts including implicit tracks.
fn place_grid_items(
    items: &mut [GridItem],
    explicit_cols: usize,
    explicit_rows: usize,
    column_flow: bool,
) -> (usize, usize) {
    // Work in (major, minor) = (rows, cols) for row flow, swapped for column flow.
    let resolved: Vec<(Lines, Lines)> = items
        .iter()
        .map(|it| {
            let c = resolve_lines(it.style.grid_column, explicit_cols + 1);
            let r = resolve_lines(it.style.grid_row, explicit_rows + 1);
            if column_flow {
                (c, r)
            } else {
                (r, c)
            }
        })
        .collect();
    let explicit_minor = if column_flow { explicit_rows } else { explicit_cols };
    let mut minor_count = explicit_minor.max(1);
    for ((_, _), (start, span)) in &resolved {
        minor_count = minor_count.max(start.unwrap_or(0) + span);
    }
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let fits = |occ: &Vec<Vec<bool>>, major: usize, minor: usize, major_span: usize, minor_span: usize| {
        (major..major + major_span).all(|r| (minor..minor + minor_span).all(|c| !occ.get(r).is_some_and(|row| row[c])))
    };
    let mark = |occ: &mut Vec<Vec<bool>>, major: usize, minor: usize, major_span: usize, minor_span: usize| {
        while occ.len() < major + major_span {
            occ.push(vec![false; minor_count]);
        }
        for row in occ.iter_mut().skip(major).take(major_span) {
            for cell in row.iter_mut().skip(minor).take(minor_span) {
                *cell = true;
            }
        }
    };
    let mut placed: Vec<Option<(usize, usize, usize, usize)>> = vec![None; items.len()];
    // 1. Items locked to a major position.
    for (i, ((major, major_span), (minor, minor_span))) in resolved.iter().enumerate() {
        let Some(major) = *major else { continue };
        let minor = match *minor {
            Some(m) => m,
            None => (0..=minor_count.saturating_sub(*minor_span))
                .find(|&c| fits(&occupied, major, c, *major_span, *minor_span))
                .unwrap_or(0),
        };
        mark(&mut occupied, major, minor, *major_span, *minor_span);
        placed[i] = Some((major, *major_span, minor, *minor_span));
    }
    // 2. The rest, in order, with a sparse cursor.
    let (mut cur_major, mut cur_minor) = (0usize, 0usize);
    for (i, ((major, major_span), (minor, minor_span))) in resolved.iter().enumerate() {
        if major.is_some() {
            continue;
        }
        let minor_span = (*minor_span).min(minor_count);
        match *minor {
            Some(m) => {
                if m < cur_minor {
                    cur_major += 1;
                }
                cur_minor = m;
                while !fits(&occupied, cur_major, m, *major_span, minor_span) {
                    cur_major += 1;
                }
            }
            None => loop {
                if cur_minor + minor_span > minor_count {
                    cur_major += 1;
                    cur_minor = 0;
                    continue;
                }
                if fits(&occupied, cur_major, cur_minor, *major_span, minor_span) {
                    break;
                }
                cur_minor += 1;
            },
        }
        mark(&mut occupied, cur_major, cur_minor, *major_span, minor_span);
        placed[i] = Some((cur_major, *major_span, cur_minor, minor_span));
        cur_minor += minor_span;
    }
    let explicit_major = if column_flow { explicit_cols } else { explicit_rows };
    let major_count = occupied.len().max(explicit_major);
    for (it, p) in items.iter_mut().zip(placed) {
        let (major, major_span, minor, minor_span) = p.unwrap_or((0, 1, 0, 1));
        if column_flow {
            (it.col, it.col_span, it.row, it.row_span) = (major, major_span, minor, minor_span);
        } else {
            (it.row, it.row_span, it.col, it.col_span) = (major, major_span, minor, minor_span);
        }
    }
    if column_flow {
        (major_count, minor_count)
    } else {
        (minor_count, major_count)
    }
}

/// CSS Grid 12.3-12.8 (simplified): sizes tracks from fixed breadths and the
/// items' (min, max) contributions, then maximizes, expands `fr` tracks and
/// stretches `auto` tracks. `contrib(i)` = the item's (min, max) outer size
/// in this axis; `span(i)` = (start, span) of item i.
#[allow(clippy::too_many_arguments)]
fn size_tracks(
    tracks: &[Track],
    collapsed: &[bool],
    available: Option<f64>,
    gap: f64,
    n_items: usize,
    span: &dyn Fn(usize) -> (usize, usize),
    contrib: &mut dyn FnMut(usize) -> (f64, f64),
    stretch_auto: bool,
) -> Vec<f64> {
    let n = tracks.len();
    let resolve = |b: TrackBreadth| b.length(available);
    let is_flex = |t: &Track| matches!(t.max, TrackBreadth::Fr(_));
    let mut base = vec![0.0; n];
    let mut limit = vec![f64::INFINITY; n];
    for (i, t) in tracks.iter().enumerate() {
        if collapsed[i] {
            limit[i] = 0.0;
            continue;
        }
        if let Some(v) = resolve(t.min) {
            base[i] = v;
        }
        limit[i] = match t.max {
            TrackBreadth::Fr(_) => base[i],
            b => resolve(b).unwrap_or(f64::INFINITY),
        };
    }
    // Intrinsic contributions: single-span items first, then spanning ones.
    let mut intrinsic_max = vec![None::<f64>; n];
    let mut order: Vec<usize> = (0..n_items).collect();
    order.sort_by_key(|&i| span(i).1);
    for i in order {
        let (start, len) = span(i);
        if start + len > n || len == 0 {
            continue;
        }
        let range = start..start + len;
        if range.clone().any(|k| collapsed[k]) {
            continue;
        }
        let crosses_flex = range.clone().any(|k| is_flex(&tracks[k]));
        let intrinsic_min = |k: usize| {
            matches!(tracks[k].min, TrackBreadth::Auto | TrackBreadth::MinContent | TrackBreadth::MaxContent)
        };
        let intrinsic_max_k = |k: usize| {
            matches!(tracks[k].max, TrackBreadth::Auto | TrackBreadth::MinContent | TrackBreadth::MaxContent)
        };
        if !range.clone().any(|k| intrinsic_min(k) || intrinsic_max_k(k)) {
            continue;
        }
        let (min_c, max_c) = contrib(i);
        if len == 1 {
            let k = start;
            match tracks[k].min {
                TrackBreadth::Auto | TrackBreadth::MinContent => base[k] = base[k].max(min_c),
                TrackBreadth::MaxContent => base[k] = base[k].max(max_c),
                _ => {}
            }
            let m = match tracks[k].max {
                TrackBreadth::MinContent => Some(min_c),
                TrackBreadth::Auto | TrackBreadth::MaxContent => Some(max_c),
                _ => None,
            };
            if let Some(m) = m {
                intrinsic_max[k] = Some(intrinsic_max[k].map_or(m, |v: f64| v.max(m)));
            }
            continue;
        }
        if crosses_flex {
            // Spanning items crossing a flexible track are handled by fr sizing.
            continue;
        }
        let gaps = gap * (len - 1) as f64;
        let targets: Vec<usize> = range.clone().filter(|&k| intrinsic_min(k)).collect();
        let have: f64 = range.clone().map(|k| base[k]).sum::<f64>() + gaps;
        if min_c > have && !targets.is_empty() {
            let add = (min_c - have) / targets.len() as f64;
            for &k in &targets {
                base[k] += add;
            }
        }
        let targets: Vec<usize> = range.clone().filter(|&k| intrinsic_max_k(k)).collect();
        let have: f64 = range.clone().map(|k| intrinsic_max[k].unwrap_or(base[k])).sum::<f64>() + gaps;
        if max_c > have && !targets.is_empty() {
            let add = (max_c - have) / targets.len() as f64;
            for &k in &targets {
                intrinsic_max[k] = Some(intrinsic_max[k].unwrap_or(base[k]) + add);
            }
        }
    }
    for k in 0..n {
        if limit[k].is_infinite() {
            limit[k] = intrinsic_max[k].unwrap_or(base[k]);
        }
        limit[k] = limit[k].max(base[k]);
    }
    let open = collapsed.iter().filter(|c| !**c).count();
    let gaps = gap * open.saturating_sub(1) as f64;
    // Maximize: grow base sizes towards their limits.
    let free = |base: &[f64]| available.map(|a| a - gaps - base.iter().sum::<f64>());
    match free(&base) {
        Some(mut f) if f > 0.0 => {
            for _ in 0..n {
                let growable: Vec<usize> = (0..n).filter(|&k| base[k] < limit[k] && !is_flex(&tracks[k])).collect();
                if growable.is_empty() || f <= 1e-9 {
                    break;
                }
                let share = f / growable.len() as f64;
                for k in growable {
                    let add = share.min(limit[k] - base[k]);
                    base[k] += add;
                    f -= add;
                }
            }
        }
        None => {
            // Indefinite space: tracks take their limits (max-content).
            for k in 0..n {
                if !is_flex(&tracks[k]) {
                    base[k] = limit[k];
                }
            }
        }
        _ => {}
    }
    // Expand flexible tracks.
    let fr = |k: usize| match tracks[k].max {
        TrackBreadth::Fr(f) => f,
        _ => 0.0,
    };
    let flex: Vec<usize> = (0..n).filter(|&k| is_flex(&tracks[k]) && !collapsed[k]).collect();
    if !flex.is_empty() {
        let unit = match available {
            Some(a) => {
                let mut inflexible = vec![false; n];
                let mut unit = 0.0;
                for _ in 0..=flex.len() {
                    let fixed: f64 = (0..n).filter(|&k| !is_flex(&tracks[k]) || inflexible[k]).map(|k| base[k]).sum();
                    let sum: f64 = flex.iter().filter(|&&k| !inflexible[k]).map(|&k| fr(k)).sum::<f64>();
                    let leftover = (a - gaps - fixed).max(0.0);
                    unit = leftover / sum.max(1.0);
                    let mut changed = false;
                    for &k in &flex {
                        if !inflexible[k] && base[k] > unit * fr(k) {
                            inflexible[k] = true;
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                unit
            }
            None => flex
                .iter()
                .map(|&k| {
                    let f = fr(k);
                    let content = limit[k].max(intrinsic_max[k].unwrap_or(0.0));
                    if f > 1.0 {
                        content / f
                    } else {
                        content
                    }
                })
                .fold(0.0, f64::max),
        };
        for &k in &flex {
            base[k] = base[k].max(unit * fr(k));
        }
    } else if stretch_auto {
        // Stretch auto tracks into the remaining space.
        if let Some(f) = free(&base).filter(|f| *f > 0.0) {
            let autos: Vec<usize> =
                (0..n).filter(|&k| !collapsed[k] && matches!(tracks[k].max, TrackBreadth::Auto)).collect();
            if !autos.is_empty() {
                let share = f / autos.len() as f64;
                for k in autos {
                    base[k] += share;
                }
            }
        }
    }
    base
}

impl<'a> Ctx<'a> {
    /// Grid items of `el` (with anonymous items for text runs); absolutely
    /// positioned children are returned separately.
    fn grid_children(&self, el: NodeId, style: &FlowStyle) -> (Vec<GridItem>, Vec<(NodeId, StyleRef)>) {
        let mut items = Vec::new();
        let mut absolute = Vec::new();
        let mut text: Vec<NodeId> = Vec::new();
        let flush = |text: &mut Vec<NodeId>, items: &mut Vec<GridItem>| {
            if text.iter().any(|&t| self.dom.char_data(t).is_ok_and(|d| !is_blank(d))) {
                let st = FlowStyle {
                    font_size: style.font_size,
                    line_height: style.line_height,
                    nowrap: style.nowrap,
                    text_align: style.text_align,
                    font_weight: style.font_weight,
                    ..FlowStyle::default()
                };
                items.push(GridItem {
                    el: None,
                    text: std::mem::take(text),
                    style: StyleRef::new(st),
                    col: 0,
                    col_span: 1,
                    row: 0,
                    row_span: 1,
                });
            } else {
                text.clear();
            }
        };
        for (child, s) in self.flatten_children(el) {
            match s {
                None => text.push(child),
                Some(s) if matches!(s.position, Position::Absolute | Position::Fixed) => absolute.push((child, s)),
                Some(s) => {
                    flush(&mut text, &mut items);
                    items.push(GridItem {
                        el: Some(child),
                        text: Vec::new(),
                        style: s,
                        col: 0,
                        col_span: 1,
                        row: 0,
                        row_span: 1,
                    });
                }
            }
        }
        flush(&mut text, &mut items);
        (items, absolute)
    }

    /// An item's (min-content, max-content) outer width.
    fn grid_item_widths(&self, it: &GridItem, cb_width: f64) -> (f64, f64) {
        let s = &it.style;
        let e = self.edges(s, cb_width);
        let outer = e.horizontal() + e.margin[LEFT] + e.margin[RIGHT];
        let to_content = |v: f64| Self::declared_content(s, v, e.horizontal());
        let (min, max) = match (&it.el, s.width.resolve(Some(cb_width))) {
            (_, Some(w)) => (to_content(w), to_content(w)),
            (Some(el), None) => (self.min_content(*el, s), self.max_content(*el, s)),
            (None, None) => {
                let min = it
                    .text
                    .iter()
                    .map(|&t| min_text_width(self.dom.char_data(t).unwrap_or(""), s.font_size, s.font_weight, s.wrap))
                    .fold(0.0, f64::max);
                let max = it
                    .text
                    .iter()
                    .map(|&t| collapsed_width(self.dom.char_data(t).unwrap_or(""), s.font_size, s.font_weight))
                    .sum();
                (min, max)
            }
        };
        let clamp = |v: f64| {
            Self::clamp(
                v,
                s.min_width.resolve(Some(cb_width)).map(to_content),
                s.max_width.resolve(Some(cb_width)).map(to_content),
            )
        };
        (clamp(min) + outer, clamp(max) + outer)
    }

    /// An item's used border-box width in an area `area_w` wide.
    fn grid_item_width(&self, it: &GridItem, area_w: f64, justify: Align) -> f64 {
        let s = &it.style;
        let e = self.edges(s, area_w);
        let to_content = |v: f64| Self::declared_content(s, v, e.horizontal());
        let available = (area_w - e.margin[LEFT] - e.margin[RIGHT]).max(0.0);
        let content = match s.width.resolve(Some(area_w)) {
            Some(w) => to_content(w),
            None if justify == Align::Stretch && !s.replaced => (available - e.horizontal()).max(0.0),
            None => {
                let (_, max) = self.grid_item_widths(it, area_w);
                (max - e.horizontal() - e.margin[LEFT] - e.margin[RIGHT]).min((available - e.horizontal()).max(0.0))
            }
        };
        let content = Self::clamp(
            content,
            s.min_width.resolve(Some(area_w)).map(to_content),
            s.max_width.resolve(Some(area_w)).map(to_content),
        );
        content + e.horizontal()
    }

    /// An item's content-based outer height at border-box width `w`.
    fn grid_item_height(&self, it: &GridItem, w: f64, cb_width: f64, area_h: Option<f64>) -> f64 {
        let s = &it.style;
        let e = self.edges(s, cb_width);
        let to_content = |v: f64| if s.border_box { (v - e.vertical()).max(0.0) } else { v };
        let content_w = (w - e.horizontal()).max(0.0);
        let content = match (s.height.resolve(area_h), &it.el) {
            (Some(h), _) => to_content(h),
            (None, Some(el)) => self.measure_height(*el, s, content_w),
            (None, None) => {
                let mut probe = self.probe();
                probe.pending.push(Vec::new());
                probe.inline_run(&it.text, s, 0.0, 0.0, content_w.max(1.0), false, (0.0, 0.0))
            }
        };
        let content = Self::clamp(
            content,
            s.min_height.resolve(area_h).map(to_content),
            s.max_height.resolve(area_h).map(to_content),
        );
        content + e.vertical() + e.margin[TOP] + e.margin[BOTTOM]
    }

    /// Lays out a grid container's content box at (x, y), `width` wide and
    /// `height` tall when definite; returns the content height.
    #[allow(clippy::too_many_arguments)]
    fn grid(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        width: f64,
        height: Option<f64>,
        fixed: bool,
        shift: (f64, f64),
    ) -> f64 {
        let (mut items, absolute) = self.grid_children(el, style);
        for (child, s) in absolute {
            self.defer_absolute(child, s, x - shift.0, y);
        }
        let col_gap = style.column_gap.resolve(width);
        let row_gap = style.row_gap.resolve(height.unwrap_or(0.0));
        let (cols, col_fit) = expand_tracks(&style.grid_template_columns, Some(width), col_gap);
        let (rows, row_fit) = expand_tracks(&style.grid_template_rows, height, row_gap);
        let (ncols, nrows) = place_grid_items(&mut items, cols.len(), rows.len(), style.grid_auto_flow_column);
        let col_tracks: Vec<Track> =
            (0..ncols).map(|i| cols.get(i).copied().unwrap_or(style.grid_auto_columns)).collect();
        let row_tracks: Vec<Track> = (0..nrows).map(|i| rows.get(i).copied().unwrap_or(style.grid_auto_rows)).collect();
        let collapse = |n: usize, fit: &Option<std::ops::Range<usize>>, used: &dyn Fn(usize) -> bool| {
            (0..n).map(|k| fit.as_ref().is_some_and(|r| r.contains(&k)) && !used(k)).collect::<Vec<bool>>()
        };
        let col_collapsed =
            collapse(ncols, &col_fit, &|k| items.iter().any(|it| (it.col..it.col + it.col_span).contains(&k)));
        let row_collapsed =
            collapse(nrows, &row_fit, &|k| items.iter().any(|it| (it.row..it.row + it.row_span).contains(&k)));
        // Columns.
        let mut widths_cache: Vec<Option<(f64, f64)>> = vec![None; items.len()];
        let col_sizes = size_tracks(
            &col_tracks,
            &col_collapsed,
            Some(width),
            col_gap,
            items.len(),
            &|i| (items[i].col, items[i].col_span),
            &mut |i| *widths_cache[i].get_or_insert_with(|| self.grid_item_widths(&items[i], width)),
            style.justify_content == Justify::Start,
        );
        let col_axis = Axis { sizes: col_sizes, collapsed: col_collapsed, gap: col_gap };
        let col_starts = col_axis.starts();
        // Item widths, then rows from their heights.
        let used_w: Vec<f64> = items
            .iter()
            .map(|it| {
                let area = col_axis.area(&col_starts, it.col, it.col_span);
                self.grid_item_width(it, area, it.style.justify_self.unwrap_or(style.justify_items))
            })
            .collect();
        let own = self.edges(style, width);
        let to_content_h = |v: f64| if style.border_box { (v - own.vertical()).max(0.0) } else { v };
        let own_min_h = style.min_height.resolve(self.own_cb_height).map(to_content_h);
        let own_max_h = style.max_height.resolve(self.own_cb_height).map(to_content_h);
        let mut heights_cache: Vec<Option<f64>> = vec![None; items.len()];
        let size_rows = |available: Option<f64>, heights_cache: &mut Vec<Option<f64>>| {
            size_tracks(
                &row_tracks,
                &row_collapsed,
                available,
                row_gap,
                items.len(),
                &|i| (items[i].row, items[i].row_span),
                &mut |i| {
                    let h = *heights_cache[i]
                        .get_or_insert_with(|| self.grid_item_height(&items[i], used_w[i], width, None));
                    (h, h)
                },
                true,
            )
        };
        let mut row_sizes = size_rows(height, &mut heights_cache);
        if height.is_none() {
            // Auto height: the rows' size clamped by min/max height; redo
            // with that as the available space when clamped.
            let axis = Axis { sizes: row_sizes.clone(), collapsed: row_collapsed.clone(), gap: row_gap };
            let total = axis.total();
            let clamped = Self::clamp(total, own_min_h, own_max_h);
            if (clamped - total).abs() > 1e-9 {
                row_sizes = size_rows(Some(clamped), &mut heights_cache);
            }
        }
        let row_axis = Axis { sizes: row_sizes, collapsed: row_collapsed, gap: row_gap };
        let row_starts = row_axis.starts();
        // Distribute leftover column space (justify-content).
        let free_x = (width - col_axis.total()).max(0.0);
        let (offset_x, between_x) = distribute(style.justify_content, free_x, ncols);
        // Place items.
        for (i, it) in items.iter().enumerate() {
            let area_x = x + offset_x + col_starts[it.col] + between_x * it.col as f64;
            let area_w = col_axis.area(&col_starts, it.col, it.col_span) + between_x * (it.col_span - 1) as f64;
            let area_y = y + row_starts[it.row];
            let area_h = row_axis.area(&row_starts, it.row, it.row_span);
            let e = self.edges(&it.style, area_w);
            let w = used_w[i];
            let justify = it.style.justify_self.unwrap_or(style.justify_items);
            let align = it.style.align_self.unwrap_or(style.align_items);
            let auto_h = it.style.height.resolve(Some(area_h)).is_none();
            let h = if align == Align::Stretch && auto_h && !it.style.replaced {
                (area_h - e.margin[TOP] - e.margin[BOTTOM]).max(0.0)
            } else {
                self.grid_item_height(it, w, area_w, Some(area_h)) - e.margin[TOP] - e.margin[BOTTOM]
            };
            let outer_w = w + e.margin[LEFT] + e.margin[RIGHT];
            let outer_h = h + e.margin[TOP] + e.margin[BOTTOM];
            let dx = match justify {
                Align::End => area_w - outer_w,
                Align::Center => (area_w - outer_w) / 2.0,
                _ => 0.0,
            };
            let dy = match align {
                Align::End => area_h - outer_h,
                Align::Center => (area_h - outer_h) / 2.0,
                _ => 0.0,
            };
            match it.el {
                Some(child) => self.place_sized(child, &it.style, area_x + dx, area_y + dy, w, h, area_w, fixed, shift),
                None => {
                    let run = it.text.clone();
                    self.inline_run(&run, &it.style, area_x + dx, area_y + dy, w.max(1.0), fixed, shift);
                }
            }
        }
        height.unwrap_or_else(|| row_axis.total())
    }

    /// Min-content (`min`) or max-content width of a grid container's
    /// contents: the sum of its columns sized from the items' contributions.
    fn grid_content_width(&self, el: NodeId, style: &FlowStyle, min: bool) -> f64 {
        let (mut items, _) = self.grid_children(el, style);
        let gap = style.column_gap.resolve(0.0);
        let (cols, _) = expand_tracks(&style.grid_template_columns, None, gap);
        let (rows, _) = expand_tracks(&style.grid_template_rows, None, 0.0);
        let (ncols, _) = place_grid_items(&mut items, cols.len(), rows.len(), style.grid_auto_flow_column);
        let tracks: Vec<Track> = (0..ncols).map(|i| cols.get(i).copied().unwrap_or(style.grid_auto_columns)).collect();
        let collapsed = vec![false; ncols];
        let sizes = size_tracks(
            &tracks,
            &collapsed,
            None,
            gap,
            items.len(),
            &|i| (items[i].col, items[i].col_span),
            &mut |i| {
                let (a, b) = self.grid_item_widths(&items[i], 0.0);
                if min {
                    (a, a)
                } else {
                    (a, b)
                }
            },
            false,
        );
        Axis { sizes, collapsed, gap }.total()
    }

    /// Lays out `el` with its border box at (x, y) + margins, sized w x h.
    #[allow(clippy::too_many_arguments)]
    fn place_sized(
        &mut self,
        el: NodeId,
        style: &FlowStyle,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        cb_width: f64,
        fixed: bool,
        shift: (f64, f64),
    ) {
        let e = self.edges(style, cb_width);
        let mut s = style.clone();
        s.border_box = true;
        s.width = Dim::px(w);
        s.height = Dim::px(h);
        s.min_width = Dim::Auto;
        s.max_width = Dim::Auto;
        s.min_height = Dim::Auto;
        s.max_height = Dim::Auto;
        s.margin =
            [Dim::px(e.margin[TOP]), Dim::px(e.margin[RIGHT]), Dim::px(e.margin[BOTTOM]), Dim::px(e.margin[LEFT])];
        self.block_child(el, &s, x, y, w + e.margin[LEFT] + e.margin[RIGHT], fixed, shift);
    }
}

/// (offset before the first track, extra space between tracks) for
/// `justify-content` over `n` tracks.
fn distribute(justify: Justify, free: f64, n: usize) -> (f64, f64) {
    let n = n.max(1) as f64;
    match justify {
        Justify::Start => (0.0, 0.0),
        Justify::End => (free, 0.0),
        Justify::Center => (free / 2.0, 0.0),
        Justify::SpaceBetween if n > 1.0 => (0.0, free / (n - 1.0)),
        Justify::SpaceBetween => (0.0, 0.0),
        Justify::SpaceAround => (free / n / 2.0, free / n),
        Justify::SpaceEvenly => (free / (n + 1.0), free / (n + 1.0)),
    }
}
