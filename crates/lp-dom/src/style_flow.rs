//! Used style values for flow layout (`flow.rs`), read from the computed-style
//! cascade (`StyleEngine::computed_value`). Owned by the style engine; `flow.rs`
//! only consumes [`FlowStyleSource`].
//!
//! Values come from author stylesheets and inline styles first, then a small
//! UA table (display, margins and paddings of the HTML rendering section) when
//! no author value applies. Lengths reach the layout as `Len { px, percent }`
//! with em/rem/vw/vh already resolved. Keywords the layout cannot use yet
//! (`fit-content`, `min-content`, ...) become `auto`.

use markup5ever::ns;

use crate::computed::{self, LengthContext};
use crate::dom::Dom;
use crate::flow::{
    Align, Dim, FlexDirection, FlowStyle, FlowStyleSource, InnerDisplay, Justify, Len, OuterDisplay, Position,
    TablePart, TextAlign,
};
use crate::node::NodeId;
use crate::style::{DeclaredEntry, FlowProp, StyleEngine, TypedValue, DEFAULT_FONT_SIZE};
use std::rc::Rc;

/// What the typed slot of a flow longhand says for one element.
enum Own<'a> {
    /// A literal the string cascade would return unchanged.
    Literal(&'a TypedValue),
    /// No own declaration, and the property does not inherit: nothing applies.
    None,
    /// Inherited without an own value, a value the string cascade must
    /// resolve (var(), relative units, CSS-wide keywords), or an SVG element:
    /// read through the string cascade.
    Fallback,
}

/// Flow styles read through the typed declaration slots, each falling back
/// to the string cascade (`StyleEngine::used_value`) whenever the winning
/// declaration needs it. A typed slot always holds the cascade winner, so a
/// fallback never substitutes a lower-priority literal.
struct Reads<'a> {
    engine: &'a StyleEngine,
    dom: &'a Dom,
    el: NodeId,
    entry: Option<Rc<DeclaredEntry>>,
}

impl Reads<'_> {
    fn own(&self, prop: FlowProp) -> Own<'_> {
        let Some(entry) = &self.entry else { return Own::Fallback };
        match &entry.flow[prop as usize] {
            Some((TypedValue::Other, _)) => Own::Fallback,
            Some((literal, _)) => Own::Literal(literal),
            None if prop.inherited() => Own::Fallback,
            None => Own::None,
        }
    }

    /// The string cascade's computed value (`used_value`).
    fn string(&self, prop: FlowProp) -> Option<String> {
        self.engine.used_value(self.dom, self.el, prop.name())
    }

    /// A length property: `Some(px, percent)` as `resolve_len` gives for the
    /// computed value, resolved in `cx`.
    fn len(&self, prop: FlowProp, cx: &LengthContext) -> Option<(f64, f64)> {
        match self.own(prop) {
            Own::Literal(TypedValue::Px(px)) => Some((px * cx.zoom, 0.0)),
            Own::Literal(TypedValue::Pct(pct)) => Some((0.0, *pct)),
            Own::Literal(TypedValue::Number(n)) => (*n == 0.0).then_some((0.0, 0.0)),
            Own::Literal(TypedValue::Keyword(_)) | Own::None => None,
            Own::Literal(TypedValue::Other) | Own::Fallback => {
                self.string(prop).and_then(|v| computed::resolve_len(&v, cx))
            }
        }
    }

    /// A keyword property, trimmed and lowercased; `None` when nothing applies.
    fn keyword(&self, prop: FlowProp) -> Option<std::borrow::Cow<'_, str>> {
        match self.own(prop) {
            Own::Literal(TypedValue::Keyword(k)) if k.bytes().any(|b| b.is_ascii_uppercase()) => {
                Some(std::borrow::Cow::Owned(k.to_ascii_lowercase()))
            }
            Own::Literal(TypedValue::Keyword(k)) => Some(std::borrow::Cow::Borrowed(k)),
            Own::None => None,
            _ => self.string(prop).map(|v| std::borrow::Cow::Owned(v.trim().to_ascii_lowercase())),
        }
    }

    /// A keyword property trimmed, in its declared case (readers that compare
    /// case-sensitively); `None` when nothing applies.
    fn keyword_raw(&self, prop: FlowProp) -> Option<std::borrow::Cow<'_, str>> {
        match self.own(prop) {
            Own::Literal(TypedValue::Keyword(k)) => Some(std::borrow::Cow::Borrowed(k)),
            Own::None => None,
            _ => self.string(prop).map(|v| std::borrow::Cow::Owned(v.trim().to_owned())),
        }
    }

    /// A number property as `resolve_number` reads the computed value.
    fn number(&self, prop: FlowProp) -> Option<f64> {
        match self.own(prop) {
            Own::Literal(TypedValue::Number(n)) => Some(*n),
            Own::Literal(TypedValue::Px(_) | TypedValue::Pct(_) | TypedValue::Keyword(_)) | Own::None => None,
            _ => self.string(prop).and_then(|v| computed::resolve_number(&v)),
        }
    }
}

impl FlowStyleSource for StyleEngine {
    fn generated(&self, dom: &Dom, el: NodeId, after: bool) -> Option<std::rc::Rc<crate::flow::Generated>> {
        self.generated_content(dom, el, after)
    }

    /// Cached per element across layout passes; the cache is invalidated
    /// with the cascade memo (selectively for journaled DOM changes).
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle> {
        if let Some(cached) = self.cached_flow_style(dom, el) {
            return cached;
        }
        if let Some((style, from)) = self.shared_flow_style(dom, el) {
            self.store_flow_style(el, style.clone(), Some(from));
            return style;
        }
        let started = crate::layout::flow_stats::enabled().then(std::time::Instant::now);
        let style = self.flow_style_uncached(dom, el);
        if let Some(t) = started {
            crate::layout::flow_stats::note_style_miss(t.elapsed());
        }
        self.store_flow_style(el, style.clone(), None);
        style
    }
}

impl StyleEngine {
    /// The element's flow style computed from its own cascade, never shared
    /// or cached (differential tests of style sharing).
    #[doc(hidden)]
    pub fn flow_style_unshared(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle> {
        self.flow_style_uncached(dom, el)
    }
}

impl StyleEngine {
    // flow.rs owns FlowStyle and adds fields (zoom, flex) over time; `..Default` keeps this file compiling.
    #[allow(clippy::needless_update)]
    fn flow_style_uncached(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle> {
        let element = dom.element(el).ok()?;
        if self.has_display_none(dom, el) {
            return None;
        }
        let tag = element.name.local.to_ascii_lowercase().to_string();
        let svg = element.name.ns == ns!(svg);
        let value = |name: &str| self.used_value(dom, el, name);
        // SVG presentation attributes act as declarations below the rules: the string cascade reads them.
        let reads = Reads {
            engine: self,
            dom,
            el,
            entry: if self.typed_flow_reads() && !svg { self.declared_entry(dom, el) } else { None },
        };
        let root_font_size = dom
            .document_element(self.document())
            .ok()
            .flatten()
            .and_then(|root| px_of(self.used_value(dom, root, "font-size").as_deref()))
            .unwrap_or(DEFAULT_FONT_SIZE);
        let viewport = self.viewport();
        // CSS `zoom` (css-viewport, Chromium's standardized zoom): the used
        // value of every absolute length is multiplied by the effective zoom,
        // the product of the element's and its ancestors' zoom. Lengths
        // resolve through `cx` (zoom applied); the font size scales once and
        // `em` follows it.
        let unzoomed = LengthContext {
            font_size: DEFAULT_FONT_SIZE,
            root_font_size,
            viewport_width: viewport.width as f64,
            viewport_height: viewport.height as f64,
            line_height: None,
            zoom: 1.0,
        };
        let own_zoom = match reads.own(FlowProp::Zoom) {
            Own::Literal(TypedValue::Number(n)) => Some(*n),
            Own::Literal(TypedValue::Pct(p)) => Some(p / 100.0),
            Own::None => None,
            _ => value("zoom").and_then(|v| {
                let v = v.trim();
                v.strip_suffix('%').map_or_else(
                    || computed::resolve_number_in(v, &unzoomed),
                    |p| p.parse::<f64>().ok().map(|p| p / 100.0),
                )
            }),
        }
        .filter(|z| z.is_finite() && *z > 0.0)
        .unwrap_or(1.0);
        let parent_zoom = dom
            .parent_element(el)
            .ok()
            .flatten()
            .and_then(|p| <Self as FlowStyleSource>::flow_style(self, dom, p))
            .map_or(1.0, |p| p.zoom);
        let zoom = parent_zoom * own_zoom;
        let font_size = px_of(value("font-size").as_deref()).unwrap_or(DEFAULT_FONT_SIZE) * zoom;
        let cx = LengthContext { font_size, zoom, ..unzoomed };
        let line_height_from_len = |(px, pct): (f64, f64)| {
            if pct != 0.0 {
                layout_unit_trunc(px + pct * font_size / 100.0)
            } else {
                (px * 64.0).round() / 64.0
            }
        };
        let line_height = match reads.own(FlowProp::LineHeight) {
            // `normal` and every other keyword (`resolve_len` rejects them) give None.
            Own::Literal(TypedValue::Keyword(_)) => None,
            Own::Literal(TypedValue::Number(n)) => Some(layout_unit_trunc(n * font_size)),
            Own::Literal(TypedValue::Px(px)) => Some(line_height_from_len((px * cx.zoom, 0.0))),
            Own::Literal(TypedValue::Pct(pct)) => Some(line_height_from_len((0.0, *pct))),
            Own::None => None,
            _ => value("line-height").and_then(|v| {
                let v = v.trim();
                if v.eq_ignore_ascii_case("normal") {
                    None
                } else if let Some(n) = computed::resolve_number(v) {
                    Some(layout_unit_trunc(n * font_size))
                } else {
                    computed::resolve_len(v, &cx).map(line_height_from_len)
                }
            }),
        };
        // `lh` resolves against the used line height (`normal` ~ 1.2em).
        let cx = LengthContext { line_height: Some(line_height.unwrap_or(1.2 * font_size)), ..cx };
        let len = |prop: FlowProp| reads.len(prop, &cx).map(to_len);
        let dim = |prop: FlowProp| match reads.own(prop) {
            Own::Literal(TypedValue::Keyword(k)) => match &**k {
                // Intrinsic size keywords (flow honours them for widths).
                "min-content" => Dim::Content(crate::flow::ContentSize::Min),
                "max-content" => Dim::Content(crate::flow::ContentSize::Max),
                "fit-content" | "-webkit-fit-content" | "-moz-fit-content" => {
                    Dim::Content(crate::flow::ContentSize::Fit)
                }
                _ => Dim::Auto,
            },
            Own::Literal(TypedValue::Px(_) | TypedValue::Pct(_) | TypedValue::Number(_)) => {
                reads.len(prop, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l)))
            }
            Own::None => Dim::Auto,
            _ => match value(prop.name()) {
                Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                    "min-content" => Dim::Content(crate::flow::ContentSize::Min),
                    "max-content" => Dim::Content(crate::flow::ContentSize::Max),
                    "fit-content" | "-webkit-fit-content" | "-moz-fit-content" => {
                        Dim::Content(crate::flow::ContentSize::Fit)
                    }
                    _ => computed::resolve_len(&v, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l))),
                },
                None => Dim::Auto,
            },
        };

        // SVG internals have no CSS boxes; the outer <svg> is an atomic inline box.
        if svg && tag.as_str() != "svg" {
            return Some(FlowStyle { outer: OuterDisplay::Contents, font_size, zoom, ..FlowStyle::default() });
        }
        let display = reads.keyword(FlowProp::Display).map_or_else(|| ua_display(&tag).to_owned(), |k| k.into_owned());
        let table_part = match display.as_str() {
            "table-header-group" => TablePart::HeaderGroup,
            "table-row-group" => TablePart::BodyGroup,
            "table-footer-group" => TablePart::FooterGroup,
            "table-row" => TablePart::Row,
            "table-cell" => TablePart::Cell,
            "table-caption" => TablePart::Caption,
            "table-column" => TablePart::Column,
            "table-column-group" => TablePart::ColumnGroup,
            _ => TablePart::None,
        };
        let (outer, inner) = match display.as_str() {
            "inline" => (OuterDisplay::Inline, InnerDisplay::Flow),
            "inline-block" => (OuterDisplay::Inline, InnerDisplay::FlowRoot),
            "flex" => (OuterDisplay::Block, InnerDisplay::Flex),
            "inline-flex" => (OuterDisplay::Inline, InnerDisplay::Flex),
            "grid" => (OuterDisplay::Block, InnerDisplay::Grid),
            "inline-grid" => (OuterDisplay::Inline, InnerDisplay::Grid),
            "flow-root" => (OuterDisplay::Block, InnerDisplay::FlowRoot),
            "table" => (OuterDisplay::Block, InnerDisplay::Table),
            "inline-table" => (OuterDisplay::Inline, InnerDisplay::Table),
            "contents" => (OuterDisplay::Contents, InnerDisplay::Flow),
            "none" => return None,
            // block, list-item, table parts (laid out as blocks in Stage 1), unknown values.
            _ => (OuterDisplay::Block, InnerDisplay::Flow),
        };
        let position = match reads.keyword_raw(FlowProp::Position).as_deref() {
            Some("relative") => Position::Relative,
            Some("absolute") => Position::Absolute,
            Some("fixed") => Position::Fixed,
            Some("sticky") => Position::Sticky,
            _ => Position::Static,
        };
        let ua = ua_box(&tag, font_size, zoom);
        const MARGINS: [FlowProp; 4] =
            [FlowProp::MarginTop, FlowProp::MarginRight, FlowProp::MarginBottom, FlowProp::MarginLeft];
        const PADDINGS: [FlowProp; 4] =
            [FlowProp::PaddingTop, FlowProp::PaddingRight, FlowProp::PaddingBottom, FlowProp::PaddingLeft];
        const BORDER_STYLES: [FlowProp; 4] = [
            FlowProp::BorderTopStyle,
            FlowProp::BorderRightStyle,
            FlowProp::BorderBottomStyle,
            FlowProp::BorderLeftStyle,
        ];
        const BORDER_WIDTHS: [FlowProp; 4] = [
            FlowProp::BorderTopWidth,
            FlowProp::BorderRightWidth,
            FlowProp::BorderBottomWidth,
            FlowProp::BorderLeftWidth,
        ];
        let margin = [0, 1, 2, 3].map(|i| match reads.own(MARGINS[i]) {
            Own::Literal(TypedValue::Keyword(k)) => {
                if k.eq_ignore_ascii_case("auto") {
                    Dim::Auto
                } else {
                    Dim::px(0.0)
                }
            }
            Own::Literal(TypedValue::Px(_) | TypedValue::Pct(_) | TypedValue::Number(_)) => {
                reads.len(MARGINS[i], &cx).map_or(Dim::px(0.0), |l| Dim::Len(to_len(l)))
            }
            Own::None => ua.margin[i],
            _ => match value(MARGINS[i].name()) {
                Some(v) if v.trim().eq_ignore_ascii_case("auto") => Dim::Auto,
                Some(v) => computed::resolve_len(&v, &cx).map_or(Dim::px(0.0), |l| Dim::Len(to_len(l))),
                None => ua.margin[i],
            },
        });
        let padding = [0, 1, 2, 3].map(|i| len(PADDINGS[i]).unwrap_or(ua.padding[i]));
        // A border width's computed value is `{px}px` only for absolute lengths; keywords and the rest fall to 3px.
        let border_width_px = |prop: FlowProp| match reads.own(prop) {
            Own::Literal(TypedValue::Px(px)) => Some(*px),
            Own::Literal(TypedValue::Number(n)) => (*n == 0.0).then_some(0.0),
            Own::Literal(TypedValue::Pct(_) | TypedValue::Keyword(_)) | Own::None => None,
            _ => px_of(value(prop.name()).as_deref()),
        };
        let border = [0, 1, 2, 3].map(|i| match reads.keyword_raw(BORDER_STYLES[i]).as_deref() {
            None | Some("none" | "hidden") => 0.0,
            Some(_) => border_width_px(BORDER_WIDTHS[i]).unwrap_or(3.0) * zoom,
        });
        // Computed overflow per axis, after `visible` → `auto` coercion.
        let (overflow_x, overflow_y) = (reads.keyword_raw(FlowProp::OverflowX), reads.keyword_raw(FlowProp::OverflowY));
        let overflow = crate::style::computed_overflow_keywords(overflow_x.as_deref(), overflow_y.as_deref());
        // Blink's ComputedLineHeightAsFixed: numbers and percentages scale the
        // font size into a truncated LayoutUnit (1/64 px), fixed lengths round,
        // so `1.8462` at 13px lays out as 24px.
        let nowrap = matches!(reads.keyword_raw(FlowProp::WhiteSpace).as_deref(), Some("nowrap" | "pre"));
        let keyword = |prop: FlowProp| reads.keyword(prop).map(|k| k.into_owned()).unwrap_or_default();
        // word-break: break-word is word-break: normal + overflow-wrap: anywhere.
        let overflow_wrap = Some(keyword(FlowProp::OverflowWrap))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| keyword(FlowProp::WordWrap));
        let wrap = match (keyword(FlowProp::WordBreak).as_str(), overflow_wrap.as_str()) {
            ("break-all" | "break-word", _) | (_, "anywhere") => crate::flow::WrapMode::Anywhere,
            (_, "break-word") => crate::flow::WrapMode::BreakWord,
            _ => crate::flow::WrapMode::Normal,
        };
        Some(FlowStyle {
            outer,
            inner,
            position,
            inset: [dim(FlowProp::Top), dim(FlowProp::Right), dim(FlowProp::Bottom), dim(FlowProp::Left)],
            width: dim(FlowProp::Width),
            height: dim(FlowProp::Height),
            min_width: dim(FlowProp::MinWidth),
            max_width: dim(FlowProp::MaxWidth),
            min_height: dim(FlowProp::MinHeight),
            max_height: dim(FlowProp::MaxHeight),
            margin,
            padding,
            border,
            border_box: reads.keyword(FlowProp::BoxSizing).as_deref() == Some("border-box"),
            clips_x: overflow.0 != "visible",
            clips_y: overflow.1 != "visible",
            font_size,
            line_height,
            nowrap,
            wrap,
            // Used font-weight for the flow text measure.
            font_weight: match value("font-weight").map(|v| v.trim().to_ascii_lowercase()).unwrap_or_default().as_str()
            {
                "bold" | "bolder" => 700,
                "lighter" => 300,
                "" | "normal" => 400,
                v => v.parse::<f64>().map_or(400, |n| n.clamp(1.0, 1000.0) as u16),
            },
            replaced: matches!(tag.as_str(), "img" | "iframe" | "video" | "canvas" | "embed" | "object" | "svg"),
            // A loaded <img>'s natural size (lp_dom::image via the embedder).
            intrinsic: if tag == "img" { dom.natural_size(el).map(|(w, h)| (w * zoom, h * zoom)) } else { None },
            zoom,
            flex_direction: match keyword(FlowProp::FlexDirection).as_str() {
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => FlexDirection::Row,
            },
            flex_wrap: matches!(keyword(FlowProp::FlexWrap).as_str(), "wrap" | "wrap-reverse"),
            row_gap: len(FlowProp::RowGap).unwrap_or(Len::ZERO),
            column_gap: len(FlowProp::ColumnGap).unwrap_or(Len::ZERO),
            align_items: align(&keyword(FlowProp::AlignItems)).unwrap_or(Align::Stretch),
            align_self: align(&keyword(FlowProp::AlignSelf)),
            justify_content: match keyword(FlowProp::JustifyContent).as_str() {
                "flex-end" | "end" | "right" => Justify::End,
                "center" => Justify::Center,
                "space-between" => Justify::SpaceBetween,
                "space-around" => Justify::SpaceAround,
                "space-evenly" => Justify::SpaceEvenly,
                _ => Justify::Start,
            },
            flex_grow: reads.number(FlowProp::FlexGrow).filter(|n| *n >= 0.0).unwrap_or(0.0),
            flex_shrink: reads.number(FlowProp::FlexShrink).filter(|n| *n >= 0.0).unwrap_or(1.0),
            flex_basis: match reads.own(FlowProp::FlexBasis) {
                // `auto`, `content` and every other keyword (`resolve_len` rejects them) give Auto.
                Own::Literal(TypedValue::Keyword(_)) | Own::None => Dim::Auto,
                Own::Literal(TypedValue::Px(_) | TypedValue::Pct(_) | TypedValue::Number(_)) => {
                    reads.len(FlowProp::FlexBasis, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l)))
                }
                _ => match value("flex-basis") {
                    Some(v) if matches!(v.trim(), "auto" | "content") => Dim::Auto,
                    Some(v) => computed::resolve_len(&v, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l))),
                    None => Dim::Auto,
                },
            },
            flex_basis_percent: match reads.own(FlowProp::FlexBasis) {
                Own::Literal(TypedValue::Pct(_)) => true,
                Own::Literal(TypedValue::Px(_) | TypedValue::Number(_) | TypedValue::Keyword(_)) | Own::None => false,
                _ => value("flex-basis").is_some_and(|v| v.contains('%')),
            },
            table_part,
            // border-spacing/collapse inherit, so cells and rows see their table's values; UA table spacing is 2px.
            border_spacing: value("border-spacing")
                .and_then(|v| {
                    let parts: Vec<f64> = v
                        .split_ascii_whitespace()
                        .map(|p| computed::resolve_len(p, &cx).map(|(px, _)| px))
                        .collect::<Option<_>>()?;
                    match parts.as_slice() {
                        [both] => Some((*both, *both)),
                        [h, v] => Some((*h, *v)),
                        _ => None,
                    }
                })
                .unwrap_or(if inner == InnerDisplay::Table { (2.0, 2.0) } else { (0.0, 0.0) }),
            border_collapse: keyword(FlowProp::BorderCollapse) == "collapse",
            table_layout_fixed: keyword(FlowProp::TableLayout) == "fixed",
            // Grid container and item inputs (flow Stage 4).
            grid_template_columns: value("grid-template-columns")
                .map(|v| crate::flow::parse_track_list(&v, &|t| computed::resolve_len(t, &cx).map(to_len)))
                .unwrap_or_default(),
            grid_template_rows: value("grid-template-rows")
                .map(|v| crate::flow::parse_track_list(&v, &|t| computed::resolve_len(t, &cx).map(to_len)))
                .unwrap_or_default(),
            grid_auto_columns: value("grid-auto-columns")
                .and_then(|v| crate::flow::parse_track(&v, &|t| computed::resolve_len(t, &cx).map(to_len)))
                .unwrap_or_default(),
            grid_auto_rows: value("grid-auto-rows")
                .and_then(|v| crate::flow::parse_track(&v, &|t| computed::resolve_len(t, &cx).map(to_len)))
                .unwrap_or_default(),
            grid_auto_flow_column: value("grid-auto-flow")
                .map(|v| v.trim().to_ascii_lowercase())
                .unwrap_or_default()
                .starts_with("column"),
            justify_items: align(&keyword(FlowProp::JustifyItems)).unwrap_or(Align::Stretch),
            justify_self: align(&keyword(FlowProp::JustifySelf)),
            grid_column: crate::flow::parse_grid_placement(
                value("grid-column").as_deref(),
                value("grid-column-start").as_deref(),
                value("grid-column-end").as_deref(),
            ),
            grid_row: crate::flow::parse_grid_placement(
                value("grid-row").as_deref(),
                value("grid-row-start").as_deref(),
                value("grid-row-end").as_deref(),
            ),
            // text-align inherits; LTR maps start/left/justify to Start.
            text_align: match reads.keyword(FlowProp::TextAlign).as_deref() {
                Some("center" | "-webkit-center") => TextAlign::Center,
                Some("right" | "end" | "-webkit-right") => TextAlign::End,
                Some(_) => TextAlign::Start,
                None if matches!(tag.as_str(), "th" | "center") => TextAlign::Center,
                None => TextAlign::Start,
            },
            ..FlowStyle::default()
        })
    }
}

/// `align-items` / `align-self`; `None` for `auto` and unset (inherit the container's).
fn align(value: &str) -> Option<Align> {
    Some(match value {
        "stretch" | "normal" => Align::Stretch,
        "flex-start" | "start" | "self-start" => Align::Start,
        "flex-end" | "end" | "self-end" => Align::End,
        "center" => Align::Center,
        "baseline" | "first baseline" | "last baseline" => Align::Baseline,
        _ => return None,
    })
}

fn to_len((px, percent): (f64, f64)) -> Len {
    Len { px, percent }
}

fn px_of(value: Option<&str>) -> Option<f64> {
    value?.trim().strip_suffix("px")?.parse().ok()
}

/// HTML rendering section display defaults. `display: none` elements are
/// filtered earlier by `has_display_none`.
pub(crate) fn ua_display(tag: &str) -> &'static str {
    match tag {
        "html" | "body" | "div" | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "dl" | "dt" | "dd"
        | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside" | "form" | "fieldset" | "figure"
        | "figcaption" | "blockquote" | "pre" | "hr" | "address" | "details" | "summary" | "dialog" | "menu"
        | "legend" | "center" | "search" | "hgroup" | "optgroup" | "option" => "block",
        "li" => "list-item",
        "table" => "table",
        "thead" => "table-header-group",
        "tbody" => "table-row-group",
        "tfoot" => "table-footer-group",
        "tr" => "table-row",
        "td" | "th" => "table-cell",
        "caption" => "table-caption",
        "colgroup" => "table-column-group",
        "col" => "table-column",
        "button" | "input" | "select" | "textarea" | "meter" | "progress" => "inline-block",
        _ => "inline",
    }
}

struct UaBox {
    margin: [Dim; 4],
    padding: [Len; 4],
}

/// UA margins/paddings (HTML rendering section), used when no author value applies.
/// `font_size` is the zoomed font size; absolute defaults scale by `zoom`.
fn ua_box(tag: &str, font_size: f64, zoom: f64) -> UaBox {
    let em = |n: f64| Dim::px(n * font_size);
    let px = |n: f64| Dim::px(n * zoom);
    let zero = Dim::px(0.0);
    let vertical = |v: Dim| [v, zero, v, zero];
    let margin = match tag {
        "body" => [px(8.0); 4],
        "p" | "dl" | "pre" => vertical(em(1.0)),
        "ul" | "ol" | "menu" => vertical(em(1.0)),
        "blockquote" | "figure" => [em(1.0), px(40.0), em(1.0), px(40.0)],
        "h1" => vertical(em(0.67)),
        "h2" => vertical(em(0.83)),
        "h3" => vertical(em(1.0)),
        "h4" => vertical(em(1.33)),
        "h5" => vertical(em(1.67)),
        "h6" => vertical(em(2.33)),
        "dd" => [zero, zero, zero, px(40.0)],
        "hr" => [em(0.5), Dim::Auto, em(0.5), Dim::Auto],
        "fieldset" => [zero, px(2.0), zero, px(2.0)],
        _ => [zero; 4],
    };
    let padding = match tag {
        "ul" | "ol" | "menu" => [Len::ZERO, Len::ZERO, Len::ZERO, Len::px(40.0 * zoom)],
        "fieldset" => [
            Len::px(0.35 * font_size),
            Len::px(0.75 * font_size),
            Len::px(0.625 * font_size),
            Len::px(0.75 * font_size),
        ],
        "td" | "th" => [Len::px(zoom); 4],
        _ => [Len::ZERO; 4],
    };
    UaBox { margin, padding }
}

/// A float converted to Blink's LayoutUnit (1/64 px) by truncation, ignoring
/// f64 noise just below a unit boundary.
fn layout_unit_trunc(px: f64) -> f64 {
    (px * 64.0 + 1e-6).trunc() / 64.0
}
