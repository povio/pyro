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
use crate::style::{StyleEngine, DEFAULT_FONT_SIZE};

impl FlowStyleSource for StyleEngine {
    // flow.rs owns FlowStyle and adds fields (zoom, flex) over time; `..Default` keeps this file compiling.
    #[allow(clippy::needless_update)]
    fn flow_style(&self, dom: &Dom, el: NodeId) -> Option<FlowStyle> {
        let element = dom.element(el).ok()?;
        if self.has_display_none(dom, el) {
            return None;
        }
        let tag = element.name.local.to_ascii_lowercase().to_string();
        let svg = element.name.ns == ns!(svg);
        let value = |name: &str| self.used_value(dom, el, name);
        let font_size = px_of(value("font-size").as_deref()).unwrap_or(DEFAULT_FONT_SIZE);
        let root_font_size = dom
            .document_element(self.document())
            .ok()
            .flatten()
            .and_then(|root| px_of(self.used_value(dom, root, "font-size").as_deref()))
            .unwrap_or(DEFAULT_FONT_SIZE);
        let viewport = self.viewport();
        let cx = LengthContext {
            font_size,
            root_font_size,
            viewport_width: viewport.width as f64,
            viewport_height: viewport.height as f64,
        };
        let len = |name: &str| value(name).and_then(|v| computed::resolve_len(&v, &cx)).map(to_len);
        let dim = |name: &str| match value(name) {
            // Intrinsic size keywords (flow honours them for widths).
            Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                "min-content" => Dim::Content(crate::flow::ContentSize::Min),
                "max-content" => Dim::Content(crate::flow::ContentSize::Max),
                "fit-content" | "-webkit-fit-content" | "-moz-fit-content" => {
                    Dim::Content(crate::flow::ContentSize::Fit)
                }
                _ => computed::resolve_len(&v, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l))),
            },
            None => Dim::Auto,
        };

        // SVG internals have no CSS boxes; the outer <svg> is an atomic inline box.
        if svg && tag.as_str() != "svg" {
            return Some(FlowStyle { outer: OuterDisplay::Contents, font_size, ..FlowStyle::default() });
        }
        let display = value("display").unwrap_or_else(|| ua_display(&tag).to_owned()).trim().to_ascii_lowercase();
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
        let position = match value("position").as_deref().map(str::trim) {
            Some("relative") => Position::Relative,
            Some("absolute") => Position::Absolute,
            Some("fixed") => Position::Fixed,
            Some("sticky") => Position::Sticky,
            _ => Position::Static,
        };
        let ua = ua_box(&tag, font_size);
        let side =
            |prefix: &str, i: usize| ["top", "right", "bottom", "left"].map(|s| format!("{prefix}-{s}"))[i].clone();
        let margin = [0, 1, 2, 3].map(|i| match value(&side("margin", i)) {
            Some(v) if v.trim().eq_ignore_ascii_case("auto") => Dim::Auto,
            Some(v) => computed::resolve_len(&v, &cx).map_or(Dim::px(0.0), |l| Dim::Len(to_len(l))),
            None => ua.margin[i],
        });
        let padding = [0, 1, 2, 3].map(|i| len(&side("padding", i)).unwrap_or(ua.padding[i]));
        let border = [0, 1, 2, 3].map(|i| {
            let s = ["top", "right", "bottom", "left"][i];
            match value(&format!("border-{s}-style")).as_deref().map(str::trim) {
                None | Some("none" | "hidden") => 0.0,
                Some(_) => px_of(value(&format!("border-{s}-width")).as_deref()).unwrap_or(3.0),
            }
        });
        let clips = |name: &str| {
            matches!(value(name).as_deref().map(str::trim), Some("hidden" | "scroll" | "auto" | "clip" | "overlay"))
        };
        // Blink's ComputedLineHeightAsFixed: numbers and percentages scale the
        // font size into a truncated LayoutUnit (1/64 px), fixed lengths round,
        // so `1.8462` at 13px lays out as 24px.
        let line_height = value("line-height").and_then(|v| {
            let v = v.trim();
            if v.eq_ignore_ascii_case("normal") {
                None
            } else if let Some(n) = computed::resolve_number(v) {
                Some(layout_unit_trunc(n * font_size))
            } else {
                computed::resolve_len(v, &cx).map(|(px, pct)| {
                    if pct != 0.0 {
                        layout_unit_trunc(px + pct * font_size / 100.0)
                    } else {
                        (px * 64.0).round() / 64.0
                    }
                })
            }
        });
        let nowrap = matches!(value("white-space").as_deref().map(str::trim), Some("nowrap" | "pre"));
        let keyword = |name: &str| value(name).map(|v| v.trim().to_ascii_lowercase()).unwrap_or_default();
        Some(FlowStyle {
            outer,
            inner,
            position,
            inset: [dim("top"), dim("right"), dim("bottom"), dim("left")],
            width: dim("width"),
            height: dim("height"),
            min_width: dim("min-width"),
            max_width: dim("max-width"),
            min_height: dim("min-height"),
            max_height: dim("max-height"),
            margin,
            padding,
            border,
            border_box: value("box-sizing").is_some_and(|v| v.trim().eq_ignore_ascii_case("border-box")),
            clips_x: clips("overflow-x"),
            clips_y: clips("overflow-y"),
            font_size,
            line_height,
            nowrap,
            // Used font-weight for the flow text measure.
            font_weight: match keyword("font-weight").as_str() {
                "bold" | "bolder" => 700,
                "lighter" => 300,
                "" | "normal" => 400,
                v => v.parse::<f64>().map_or(400, |n| n.clamp(1.0, 1000.0) as u16),
            },
            replaced: matches!(tag.as_str(), "img" | "iframe" | "video" | "canvas" | "embed" | "object" | "svg"),
            // Non-standard `zoom`: a number or percentage; `normal`/unknown values are 1.
            zoom: value("zoom")
                .and_then(|v| {
                    let v = v.trim();
                    v.strip_suffix('%')
                        .map_or_else(|| computed::resolve_number(v), |p| p.parse::<f64>().ok().map(|p| p / 100.0))
                })
                .filter(|z| *z > 0.0)
                .unwrap_or(1.0),
            flex_direction: match keyword("flex-direction").as_str() {
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => FlexDirection::Row,
            },
            flex_wrap: matches!(keyword("flex-wrap").as_str(), "wrap" | "wrap-reverse"),
            row_gap: len("row-gap").unwrap_or(Len::ZERO),
            column_gap: len("column-gap").unwrap_or(Len::ZERO),
            align_items: align(&keyword("align-items")).unwrap_or(Align::Stretch),
            align_self: align(&keyword("align-self")),
            justify_content: match keyword("justify-content").as_str() {
                "flex-end" | "end" | "right" => Justify::End,
                "center" => Justify::Center,
                "space-between" => Justify::SpaceBetween,
                "space-around" => Justify::SpaceAround,
                "space-evenly" => Justify::SpaceEvenly,
                _ => Justify::Start,
            },
            flex_grow: value("flex-grow")
                .and_then(|v| computed::resolve_number(&v))
                .filter(|n| *n >= 0.0)
                .unwrap_or(0.0),
            flex_shrink: value("flex-shrink")
                .and_then(|v| computed::resolve_number(&v))
                .filter(|n| *n >= 0.0)
                .unwrap_or(1.0),
            flex_basis: match value("flex-basis") {
                Some(v) if matches!(v.trim(), "auto" | "content") => Dim::Auto,
                Some(v) => computed::resolve_len(&v, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l))),
                None => Dim::Auto,
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
            border_collapse: keyword("border-collapse") == "collapse",
            table_layout_fixed: keyword("table-layout") == "fixed",
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
            grid_auto_flow_column: keyword("grid-auto-flow").starts_with("column"),
            justify_items: align(&keyword("justify-items")).unwrap_or(Align::Stretch),
            justify_self: align(&keyword("justify-self")),
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
            text_align: match value("text-align").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
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
fn ua_display(tag: &str) -> &'static str {
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
fn ua_box(tag: &str, font_size: f64) -> UaBox {
    let em = |n: f64| Dim::px(n * font_size);
    let zero = Dim::px(0.0);
    let vertical = |v: Dim| [v, zero, v, zero];
    let margin = match tag {
        "body" => [Dim::px(8.0); 4],
        "p" | "dl" | "pre" => vertical(em(1.0)),
        "ul" | "ol" | "menu" => vertical(em(1.0)),
        "blockquote" | "figure" => [em(1.0), Dim::px(40.0), em(1.0), Dim::px(40.0)],
        "h1" => vertical(em(0.67)),
        "h2" => vertical(em(0.83)),
        "h3" => vertical(em(1.0)),
        "h4" => vertical(em(1.33)),
        "h5" => vertical(em(1.67)),
        "h6" => vertical(em(2.33)),
        "dd" => [zero, zero, zero, Dim::px(40.0)],
        "hr" => [em(0.5), Dim::Auto, em(0.5), Dim::Auto],
        "fieldset" => [zero, Dim::px(2.0), zero, Dim::px(2.0)],
        _ => [zero; 4],
    };
    let padding = match tag {
        "ul" | "ol" | "menu" => [Len::ZERO, Len::ZERO, Len::ZERO, Len::px(40.0)],
        "fieldset" => [
            Len::px(0.35 * font_size),
            Len::px(0.75 * font_size),
            Len::px(0.625 * font_size),
            Len::px(0.75 * font_size),
        ],
        "td" | "th" => [Len::px(1.0); 4],
        _ => [Len::ZERO; 4],
    };
    UaBox { margin, padding }
}

/// A float converted to Blink's LayoutUnit (1/64 px) by truncation, ignoring
/// f64 noise just below a unit boundary.
fn layout_unit_trunc(px: f64) -> f64 {
    (px * 64.0 + 1e-6).trunc() / 64.0
}
