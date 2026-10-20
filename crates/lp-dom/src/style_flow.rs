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
use crate::flow::{Dim, FlowStyle, FlowStyleSource, InnerDisplay, Len, OuterDisplay, Position};
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
        let value = |name: &str| self.computed_value(dom, el, name);
        let font_size = px_of(value("font-size").as_deref()).unwrap_or(DEFAULT_FONT_SIZE);
        let root_font_size = dom
            .document_element(self.document())
            .ok()
            .flatten()
            .and_then(|root| px_of(self.computed_value(dom, root, "font-size").as_deref()))
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
            Some(v) => computed::resolve_len(&v, &cx).map_or(Dim::Auto, |l| Dim::Len(to_len(l))),
            None => Dim::Auto,
        };

        // SVG internals have no CSS boxes; the outer <svg> is an atomic inline box.
        if svg && tag.as_str() != "svg" {
            return Some(FlowStyle { outer: OuterDisplay::Contents, font_size, ..FlowStyle::default() });
        }
        let display = value("display").unwrap_or_else(|| ua_display(&tag).to_owned());
        let (outer, inner) = match display.trim().to_ascii_lowercase().as_str() {
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
        let line_height = value("line-height").and_then(|v| {
            let v = v.trim();
            if v.eq_ignore_ascii_case("normal") {
                None
            } else if let Some(n) = computed::resolve_number(v) {
                Some(n * font_size)
            } else {
                computed::resolve_len(v, &cx).map(|(px, pct)| px + pct * font_size / 100.0)
            }
        });
        let nowrap = matches!(value("white-space").as_deref().map(str::trim), Some("nowrap" | "pre"));
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
            replaced: matches!(tag.as_str(), "img" | "iframe" | "video" | "canvas" | "embed" | "object" | "svg"),
            ..FlowStyle::default()
        })
    }
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
        "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption" | "colgroup" | "col" => "block",
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
        _ => [Len::ZERO; 4],
    };
    UaBox { margin, padding }
}
