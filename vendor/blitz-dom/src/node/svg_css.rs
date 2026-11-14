//! pyro: an inline `<svg>` subtree as a standalone SVG document for usvg, with
//! the page's CSS applied.
//!
//! usvg resolves presentation attributes, `style` attributes and the `<style>`
//! sheets inside the SVG, but it never sees the page's style sheets. So each SVG
//! element's properties that author CSS sets (style sheets, its `style`
//! attribute, animations) are written out with their computed values: painting
//! and text properties as declarations appended to its `style` attribute (usvg
//! ranks those above presentation attributes, as CSS does), geometry properties
//! as the attributes themselves. Transforms are resolved here, against the
//! reference box `transform-box` names (usvg knows neither the property nor CSS
//! transform syntax), and written as a `transform` matrix. A CSS `clip-path`
//! basic shape becomes a `<clipPath>` of its own, and `mask-image: url(#m)` the
//! `mask` attribute. Properties the page's CSS sets on the root's HTML ancestors
//! reach the root as inherited values.
//!
//! The fill and stroke boxes (`transform-box: fill-box`, `clip-path: circle()
//! fill-box`) are an element's bounding boxes. When an element needs one, the
//! subtree is written twice: the first document is parsed by usvg for its
//! elements' boxes, the second uses them.

use std::collections::HashMap;
use std::fmt::Write as _;

use html_escape::{encode_quoted_attribute_to_string, encode_text_to_string};
use kurbo::{Affine, Rect, Shape};
use markup5ever::{QualName, local_name, ns};
use style::properties::{ComputedValues, LonghandId, PropertyDeclarationId};
use style::rule_tree::CascadeOrigin;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::SharedRwLockReadGuard;
use style::values::computed::basic_shape::{BasicShape, ClipPath};
use style::values::computed::{CSSPixelLength, LengthPercentage};
use style::values::generics::basic_shape::{
    GenericBasicShape, GenericPathOrShapeFunction, GenericShapeRadius, ShapeBox, ShapeGeometryBox,
};
use style::values::generics::image::GenericImage;
use style::values::generics::position::GenericPositionOrAuto;
use style::values::specified::TransformBox;
use style_traits::ToCss;
use stylo_taffy::CopyStyleGetters;

use super::serialize::{XLINK_NS, is_xlink_ns_decl, replace_current_color, write_attr_name};
use super::{ElementData, Node, NodeData};
use blitz_traits::node_id::NodeId;

/// Properties written as `style` declarations when author CSS sets them.
const STYLE_PROPERTIES: &[(LonghandId, &str)] = &[
    (LonghandId::Fill, "fill"),
    (LonghandId::FillOpacity, "fill-opacity"),
    (LonghandId::FillRule, "fill-rule"),
    (LonghandId::Stroke, "stroke"),
    (LonghandId::StrokeWidth, "stroke-width"),
    (LonghandId::StrokeOpacity, "stroke-opacity"),
    (LonghandId::StrokeDasharray, "stroke-dasharray"),
    (LonghandId::StrokeDashoffset, "stroke-dashoffset"),
    (LonghandId::StrokeLinecap, "stroke-linecap"),
    (LonghandId::StrokeLinejoin, "stroke-linejoin"),
    (LonghandId::StrokeMiterlimit, "stroke-miterlimit"),
    (LonghandId::Opacity, "opacity"),
    (LonghandId::Visibility, "visibility"),
    (LonghandId::Display, "display"),
    (LonghandId::MixBlendMode, "mix-blend-mode"),
    (LonghandId::Isolation, "isolation"),
    (LonghandId::Color, "color"),
    (LonghandId::Filter, "filter"),
    (LonghandId::MaskType, "mask-type"),
    (LonghandId::FontFamily, "font-family"),
    (LonghandId::FontSize, "font-size"),
    (LonghandId::FontStyle, "font-style"),
    (LonghandId::FontWeight, "font-weight"),
    (LonghandId::LetterSpacing, "letter-spacing"),
    (LonghandId::WordSpacing, "word-spacing"),
];

/// The properties of `STYLE_PROPERTIES` written on the root `<svg>`: those
/// its content inherits. Its box effects are blitz-paint's.
const ROOT_PROPERTIES: &[LonghandId] = &[
    LonghandId::Fill,
    LonghandId::FillOpacity,
    LonghandId::FillRule,
    LonghandId::Stroke,
    LonghandId::StrokeWidth,
    LonghandId::StrokeOpacity,
    LonghandId::StrokeDasharray,
    LonghandId::StrokeDashoffset,
    LonghandId::StrokeLinecap,
    LonghandId::StrokeLinejoin,
    LonghandId::StrokeMiterlimit,
    LonghandId::Visibility,
    LonghandId::Color,
    LonghandId::FontFamily,
    LonghandId::FontSize,
    LonghandId::FontStyle,
    LonghandId::FontWeight,
    LonghandId::LetterSpacing,
    LonghandId::WordSpacing,
];

/// Inherited properties whose values the root gets from its HTML ancestors,
/// with the value usvg assumes without one (as stylo serializes it): the
/// root's computed value is written when it differs.
const INHERITED_FROM_HTML: &[(LonghandId, &str, &str)] = &[
    (LonghandId::Fill, "fill", "rgb(0, 0, 0)"),
    (LonghandId::FillOpacity, "fill-opacity", "1"),
    (LonghandId::FillRule, "fill-rule", "nonzero"),
    (LonghandId::Stroke, "stroke", "none"),
    (LonghandId::StrokeWidth, "stroke-width", "1px"),
    (LonghandId::StrokeOpacity, "stroke-opacity", "1"),
    (LonghandId::StrokeDasharray, "stroke-dasharray", "none"),
    (LonghandId::StrokeDashoffset, "stroke-dashoffset", "0px"),
    (LonghandId::StrokeLinecap, "stroke-linecap", "butt"),
    (LonghandId::StrokeLinejoin, "stroke-linejoin", "miter"),
    (LonghandId::StrokeMiterlimit, "stroke-miterlimit", "4"),
    (LonghandId::Visibility, "visibility", "visible"),
    // usvg's text defaults are 12px Times New Roman; the page's are its own.
    (LonghandId::FontSize, "font-size", "12px"),
    (LonghandId::FontFamily, "font-family", "\"Times New Roman\""),
    (LonghandId::FontWeight, "font-weight", "400"),
    (LonghandId::FontStyle, "font-style", "normal"),
];

/// The properties that make up an SVG element's transform.
const TRANSFORM_PROPERTIES: &[LonghandId] = &[
    LonghandId::Transform,
    LonghandId::Translate,
    LonghandId::Rotate,
    LonghandId::Scale,
    LonghandId::TransformOrigin,
    LonghandId::TransformBox,
];

/// An element's object bounding box (fill box) and stroke bounding box, in its
/// own user space.
#[derive(Clone, Copy, Debug, Default)]
struct Boxes {
    fill: Rect,
    stroke: Rect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// Writes ids on the elements whose boxes are needed.
    Boxes,
    Final,
}

struct Writer<'a> {
    out: String,
    guard: Option<SharedRwLockReadGuard<'a>>,
    pass: Pass,
    /// Set in the first pass: some element needs its fill or stroke box.
    needs_boxes: bool,
    /// The id each element whose box is needed has in the first pass's document.
    box_ids: HashMap<String, NodeId>,
    boxes: HashMap<NodeId, Boxes>,
    /// `<clipPath>` elements made for CSS basic shapes, written at the end.
    defs: String,
    next_id: usize,
    /// The nearest viewport's reference box size (`transform-box: view-box`).
    viewports: Vec<(f64, f64)>,
}

/// The source of the `<svg>` subtree at `root` for usvg (see the module docs).
pub(crate) fn svg_source(root: &Node) -> String {
    let guard = root.element_data().and_then(|data| data.guard.as_ref()).map(|lock| lock.read());
    let mut writer = Writer {
        out: String::new(),
        guard,
        pass: Pass::Boxes,
        needs_boxes: false,
        box_ids: HashMap::new(),
        boxes: HashMap::new(),
        defs: String::new(),
        next_id: 0,
        viewports: Vec::new(),
    };
    writer.write_node(root, 0);
    if !writer.needs_boxes {
        return writer.finish();
    }
    // The boxes come from usvg's own parse of the first document.
    let first = writer.finish();
    if let Ok(svg) = crate::util::parse_svg_image(first.as_bytes()) {
        writer.boxes = collect_boxes(&svg.tree, &writer.box_ids);
    }
    writer.out.clear();
    writer.defs.clear();
    writer.next_id = 0;
    writer.pass = Pass::Final;
    writer.write_node(root, 0);
    writer.finish()
}

/// The boxes of the elements named in `ids`, from a parsed tree.
fn collect_boxes(tree: &usvg::Tree, ids: &HashMap<String, NodeId>) -> HashMap<NodeId, Boxes> {
    fn rect(r: usvg::Rect) -> Rect {
        Rect::new(r.left() as f64, r.top() as f64, r.right() as f64, r.bottom() as f64)
    }
    fn walk(group: &usvg::Group, ids: &HashMap<String, NodeId>, out: &mut HashMap<NodeId, Boxes>) {
        for node in group.children() {
            if let Some(&id) = ids.get(node.id()) {
                out.entry(id)
                    .or_insert(Boxes { fill: rect(node.bounding_box()), stroke: rect(node.stroke_bounding_box()) });
            }
            if let usvg::Node::Group(g) = node {
                walk(g, ids, out);
                if let Some(clip) = g.clip_path() {
                    walk(clip.root(), ids, out);
                }
                if let Some(mask) = g.mask() {
                    walk(mask.root(), ids, out);
                }
            }
        }
    }
    let mut out = HashMap::new();
    walk(tree.root(), ids, &mut out);
    for clip in tree.clip_paths() {
        walk(clip.root(), ids, &mut out);
    }
    for mask in tree.masks() {
        walk(mask.root(), ids, &mut out);
    }
    for pattern in tree.patterns() {
        walk(pattern.root(), ids, &mut out);
    }
    out
}

/// The longhands that author-level declarations (style sheets, `style`
/// attributes, animations; not the UA sheet, nor presentational hints but
/// those of `clip-path` and `transform-origin`) set on the element.
fn author_longhands(style: &ComputedValues, guard: &SharedRwLockReadGuard) -> Vec<LonghandId> {
    let mut set = Vec::new();
    for node in style.rules().self_and_ancestors() {
        let Some(source) = node.style_source() else {
            continue;
        };
        // Presentational hints count for the properties stylo.rs maps from SVG
        // presentation attributes that usvg cannot read itself.
        let hints_only = match node.cascade_level().origin() {
            CascadeOrigin::UA | CascadeOrigin::User => continue,
            CascadeOrigin::PresHints => true,
            _ => false,
        };
        for declaration in source.read(guard).declarations() {
            if let PropertyDeclarationId::Longhand(id) = declaration.id() {
                if hints_only && !matches!(id, LonghandId::ClipPath | LonghandId::TransformOrigin) {
                    continue;
                }
                if !set.contains(&id) {
                    set.push(id);
                }
            }
        }
    }
    set
}

/// A computed value as CSS, with `url(...)` references reduced to their
/// fragment (`url(#id)`), the only form usvg follows.
fn computed_value(style: &ComputedValues, id: LonghandId) -> String {
    let value = style.computed_value_to_string(PropertyDeclarationId::Longhand(id));
    local_urls(&value)
}

/// `value` with each `url(...)` written as `url(#fragment)`.
fn local_urls(value: &str) -> String {
    if !value.contains("url(") {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("url(") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let Some(end) = after.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = after[..end].trim().trim_matches(|c| c == '"' || c == '\'');
        match inner.rfind('#') {
            Some(hash) => {
                out.push_str("url(");
                out.push_str(&inner[hash..]);
                out.push(')');
            }
            None => out.push_str(&rest[start..start + 4 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The property names this module writes itself (from computed values), as
/// usvg reads them from a `style` attribute.
fn is_handled_property(name: &str) -> bool {
    STYLE_PROPERTIES.iter().any(|&(_, n)| n == name)
        || matches!(
            name,
            "transform" | "translate" | "rotate" | "scale" | "transform-origin" | "transform-box"
                | "clip-path" | "mask" | "mask-image" | "x" | "y" | "width" | "height" | "cx" | "cy"
                | "r" | "rx" | "ry" | "d"
                // Its longhands are written instead (usvg reads no line height in it).
                | "font"
        )
}

/// An element's `style` attribute without the declarations of the properties
/// this module writes itself: Stylo parsed those, and those it rejected
/// (`transform: scale(invalid)`) must not reach usvg either (which would read
/// `transform` in CSS syntax as an invalid transform attribute). The others
/// (`stop-color`, `marker-end`, ... which Stylo does not parse for servo) stay.
fn without_handled_declarations(style: &str) -> String {
    let mut out = String::with_capacity(style.len());
    let (mut depth, mut quote, mut start) = (0usize, None, 0usize);
    let mut declarations = Vec::new();
    for (i, c) in style.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                declarations.push(&style[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    declarations.push(&style[start..]);
    for declaration in declarations {
        let name = declaration.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        if declaration.trim().is_empty() || is_handled_property(&name) {
            continue;
        }
        out.push_str(declaration.trim());
        out.push(';');
    }
    out
}

/// A number for SVG source: no exponent, and short.
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let rounded = (v * 1e6).round() / 1e6;
    let s = format!("{rounded:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

/// An element's property written from CSS in place of its own attribute of
/// that name: as that attribute, or (for presentation attributes, which a
/// declaration of the element's own `style` attribute would override) as a
/// declaration after the `style` attribute's own.
struct Rewrite {
    name: &'static str,
    value: Option<String>,
    in_style: bool,
}

impl Writer<'_> {
    fn finish(&mut self) -> String {
        let mut out = std::mem::take(&mut self.out);
        if !self.defs.is_empty() {
            // Before the root's end tag (or as its children when it had none).
            if let Some(end) = out.rfind("</svg>") {
                out.insert_str(end, &format!("<defs>{}</defs>", self.defs));
            } else if let Some(end) = out.rfind("/>") {
                out.replace_range(end..end + 2, &format!("><defs>{}</defs></svg>", self.defs));
            }
        }
        out
    }

    fn new_id(&mut self, kind: &str) -> String {
        self.next_id += 1;
        format!("__pyro_{kind}_{}", self.next_id)
    }

    fn write_node(&mut self, node: &Node, nesting: usize) {
        match &node.data {
            NodeData::Text(data) => {
                encode_text_to_string(data.content.as_str(), &mut self.out);
            }
            NodeData::Element(data) => self.write_element(node, data, nesting),
            _ => {}
        }
    }

    fn write_element(&mut self, node: &Node, data: &ElementData, nesting: usize) {
        let styles = node.primary_styles().map(|s| ServoArc::clone(&*s));
        let is_svg = data.name.ns == ns!(svg);
        let is_root = nesting == 0;
        let tag = data.name.local.as_ref();
        let author = match (&styles, &self.guard) {
            (Some(style), Some(guard)) if is_svg => author_longhands(style, guard),
            _ => Vec::new(),
        };
        let current_color = styles.as_ref().map(|style| style.clone_color().to_css_string());

        // Viewports for `transform-box: view-box`.
        let pushed_viewport = is_svg && tag == "svg";
        if pushed_viewport {
            let viewport = self.viewport_size(data, styles.as_deref(), is_root);
            self.viewports.push(viewport);
        }

        let mut rewrites: Vec<Rewrite> = Vec::new();
        let mut declarations = String::new();
        let mut temp_id = None;
        if let (Some(style), true) = (&styles, is_svg) {
            for &(id, name) in STYLE_PROPERTIES {
                // The root's box effects (`opacity`, `display`, `filter`, ...) are
                // its CSS box's, which blitz-paint applies.
                if is_root && !ROOT_PROPERTIES.contains(&id) {
                    continue;
                }
                if author.contains(&id) {
                    let value = computed_value(style, id);
                    let _ = write!(declarations, "{name}:{value};");
                }
            }
            if is_root {
                for &(id, name, initial) in INHERITED_FROM_HTML {
                    if author.contains(&id) || data.attr(markup5ever::LocalName::from(name)).is_some() {
                        continue;
                    }
                    let value = computed_value(style, id);
                    if value != initial {
                        let _ = write!(declarations, "{name}:{value};");
                    }
                }
            } else {
                // A `transform-origin` attribute Stylo rejected (`top 100%`) is ignored, as
                // in CSS; usvg would read some of them.
                if !author.contains(&LonghandId::TransformOrigin)
                    && data.attr(local_name!("transform-origin")).is_some()
                {
                    rewrites.push(Rewrite { name: "transform-origin", value: None, in_style: false });
                }
                self.geometry_rewrites(tag, style, &author, &mut rewrites);
                // A shape of no geometry renders nothing, its filter included (SVG 2, as
                // Chromium paints it); usvg keeps the group the filter makes, where a flood
                // would show.
                if (author.contains(&LonghandId::Filter) || data.attr(local_name!("filter")).is_some())
                    && empty_shape(tag, data, &rewrites)
                {
                    rewrites.push(Rewrite { name: "filter", value: None, in_style: true });
                }
                if TRANSFORM_PROPERTIES.iter().any(|id| author.contains(id)) {
                    self.transform_rewrite(node, data, tag, style, &author, &mut rewrites, &mut temp_id);
                }
            }
            // Not on the root: its CSS box's.
            if !is_root {
                if author.contains(&LonghandId::ClipPath) {
                    self.clip_path_rewrite(node, data, style, &mut rewrites, &mut temp_id);
                }
                if author.contains(&LonghandId::MaskImage) {
                    self.mask_rewrite(style, &mut rewrites);
                }
            }
        }

        self.out.push('<');
        self.out.push_str(tag);
        if is_root && !data.attrs().iter().any(|attr| is_xlink_ns_decl(&attr.name)) {
            self.out.push_str(" xmlns:xlink=\"");
            self.out.push_str(XLINK_NS);
            self.out.push('"');
        }
        for rewrite in rewrites.iter().filter(|r| r.in_style) {
            let _ = write!(declarations, "{}:{};", rewrite.name, rewrite.value.as_deref().unwrap_or("none"));
        }
        let mut style_attr = None;
        for attr in data.attrs() {
            let local: &str = attr.name.local.as_ref();
            if attr.name.ns == ns!() && local == "style" && is_svg {
                style_attr = Some(attr.value.as_str());
                continue;
            }
            if attr.name.ns == ns!() && rewrites.iter().any(|r| r.name == local) {
                continue;
            }
            if temp_id.is_some() && attr.name.ns == ns!() && local == "id" {
                continue;
            }
            self.write_attr(&attr.name, &attr.value, current_color.as_deref());
        }
        for rewrite in rewrites.iter().filter(|r| !r.in_style) {
            if let Some(value) = &rewrite.value {
                self.out.push(' ');
                self.out.push_str(rewrite.name);
                self.out.push_str("=\"");
                encode_quoted_attribute_to_string(value, &mut self.out);
                self.out.push('"');
            }
        }
        if let Some(id) = &temp_id {
            self.out.push_str(" id=\"");
            encode_quoted_attribute_to_string(id, &mut self.out);
            self.out.push('"');
        }
        let style_attr = style_attr.map(without_handled_declarations).filter(|v| !v.trim().is_empty());
        match (style_attr, declarations.is_empty()) {
            (None, true) => {}
            (style_attr, _) => {
                // The computed declarations first: what is left of the element's own
                // declarations sets other properties, and usvg's tokenizer stops at
                // one it cannot read.
                let mut value = declarations.clone();
                value.push_str(style_attr.as_deref().unwrap_or(""));
                let value = match current_color.as_deref().and_then(|c| replace_current_color(&value, c)) {
                    Some(replaced) => replaced,
                    None => value,
                };
                self.out.push_str(" style=\"");
                encode_quoted_attribute_to_string(&value, &mut self.out);
                self.out.push('"');
            }
        }

        if node.children.is_empty() {
            self.out.push_str(" />");
        } else {
            self.out.push('>');
            for &child_id in &node.children {
                self.write_node(&node.tree()[child_id], nesting + 1);
            }
            self.out.push_str("</");
            self.out.push_str(tag);
            self.out.push('>');
        }
        if pushed_viewport {
            self.viewports.pop();
        }
    }

    fn write_attr(&mut self, name: &QualName, value: &str, current_color: Option<&str>) {
        self.out.push(' ');
        write_attr_name(name, &mut self.out);
        self.out.push_str("=\"");
        // pyro: the keyword in any case (`currentcolor` is how CSSOM, and so
        // React's `style`, writes it); usvg knows only `currentColor`.
        match current_color.and_then(|color| replace_current_color(value, color)) {
            Some(value) => encode_quoted_attribute_to_string(&value, &mut self.out),
            None => encode_quoted_attribute_to_string(value, &mut self.out),
        };
        self.out.push('"');
    }

    /// The reference box size of `transform-box: view-box` inside this `<svg>`:
    /// its `viewBox` size, else its own size.
    fn viewport_size(&self, data: &ElementData, style: Option<&ComputedValues>, is_root: bool) -> (f64, f64) {
        if let Some(view_box) = data.attr(local_name!("viewBox")) {
            let numbers: Vec<f64> = view_box
                .split(|c: char| c == ',' || c.is_ascii_whitespace())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            if let [_, _, w, h] = numbers[..] {
                if w > 0.0 && h > 0.0 {
                    return (w, h);
                }
            }
        }
        let parent = self.viewports.last().copied().unwrap_or((300.0, 150.0));
        let length = |name: markup5ever::LocalName, basis: f64, css: Option<f64>, fallback: f64| {
            if let Some(px) = css {
                return px;
            }
            match data.attr(name).and_then(|v| v.trim().parse::<svgtypes::Length>().ok()) {
                Some(len) if len.unit == svgtypes::LengthUnit::Percent => basis * len.number / 100.0,
                Some(len) => len.number,
                None => fallback,
            }
        };
        // The root's CSS size, when it is a plain length.
        let css = |width: bool| -> Option<f64> {
            if !is_root {
                return None;
            }
            let style = style?;
            let size = if width { style.get_position().width.clone() } else { style.get_position().height.clone() };
            match size {
                style::values::generics::length::GenericSize::LengthPercentage(lp) => {
                    lp.0.to_length().map(|l| l.px() as f64)
                }
                _ => None,
            }
        };
        let (fallback_w, fallback_h) = if is_root { (300.0, 150.0) } else { parent };
        (
            length(local_name!("width"), parent.0, css(true), fallback_w),
            length(local_name!("height"), parent.1, css(false), fallback_h),
        )
    }

    /// The element's fill and stroke boxes, from the first pass; in the first
    /// pass, an id for it to be found by.
    fn boxes(&mut self, node: &Node, data: &ElementData, temp_id: &mut Option<String>) -> Boxes {
        match self.pass {
            Pass::Boxes => {
                self.needs_boxes = true;
                let id = match (data.attr(local_name!("id")), temp_id.as_ref()) {
                    (_, Some(id)) => id.clone(),
                    (Some(id), None) if !id.is_empty() && !self.box_ids.contains_key(id) => id.to_string(),
                    _ => {
                        let id = self.new_id("box");
                        *temp_id = Some(id.clone());
                        id
                    }
                };
                self.box_ids.insert(id, node.id);
                Boxes::default()
            }
            Pass::Final => self.boxes.get(&node.id).copied().unwrap_or_default(),
        }
    }

    /// CSS geometry properties (`x`, `cx`, `r`, `width`, `d`, ...) as the
    /// attributes of the elements they apply to.
    fn geometry_rewrites(&self, tag: &str, style: &ComputedValues, author: &[LonghandId], rewrites: &mut Vec<Rewrite>) {
        let applies: &[(LonghandId, &'static str)] = match tag {
            "rect" => &[
                (LonghandId::X, "x"),
                (LonghandId::Y, "y"),
                (LonghandId::Width, "width"),
                (LonghandId::Height, "height"),
                (LonghandId::Rx, "rx"),
                (LonghandId::Ry, "ry"),
            ],
            "image" | "foreignObject" | "svg" | "use" => &[
                (LonghandId::X, "x"),
                (LonghandId::Y, "y"),
                (LonghandId::Width, "width"),
                (LonghandId::Height, "height"),
            ],
            "circle" => &[(LonghandId::Cx, "cx"), (LonghandId::Cy, "cy"), (LonghandId::R, "r")],
            "ellipse" => {
                &[(LonghandId::Cx, "cx"), (LonghandId::Cy, "cy"), (LonghandId::Rx, "rx"), (LonghandId::Ry, "ry")]
            }
            "path" => &[(LonghandId::D, "d")],
            _ => &[],
        };
        for &(id, name) in applies {
            if !author.contains(&id) {
                continue;
            }
            let value = computed_value(style, id);
            let value = match id {
                LonghandId::D => match value.strip_prefix("path(") {
                    Some(inner) => {
                        inner.trim_end_matches(')').trim().trim_matches(|c| c == '"' || c == '\'').to_string()
                    }
                    None => String::new(),
                },
                // `auto` keeps the attribute (an `auto` width is the attribute's).
                _ if value == "auto" => continue,
                _ => value,
            };
            rewrites.push(Rewrite { name, value: Some(value), in_style: false });
        }
    }

    /// The element's transform (CSS `transform`, `translate`, `rotate` and
    /// `scale`, or its `transform` attribute) about its `transform-origin` in
    /// the `transform-box` reference box, as a matrix.
    #[allow(clippy::too_many_arguments)]
    fn transform_rewrite(
        &mut self,
        node: &Node,
        data: &ElementData,
        tag: &str,
        style: &ComputedValues,
        author: &[LonghandId],
        rewrites: &mut Vec<Rewrite>,
        temp_id: &mut Option<String>,
    ) {
        let attribute_name = match tag {
            "pattern" => "patternTransform",
            "linearGradient" | "radialGradient" => "gradientTransform",
            _ => "transform",
        };
        let css_transform = author.iter().any(|id| {
            matches!(id, LonghandId::Transform | LonghandId::Translate | LonghandId::Rotate | LonghandId::Scale)
        });
        let attr_transform = data
            .attr(markup5ever::LocalName::from(attribute_name))
            .and_then(|v| v.parse::<svgtypes::Transform>().ok())
            .map(|t| Affine::new([t.a, t.b, t.c, t.d, t.e, t.f]));
        if !css_transform && attr_transform.is_none() {
            return;
        }

        // The reference box.
        let reference = match style.get_box().transform_box {
            TransformBox::ViewBox => {
                let (w, h) = self.viewports.last().copied().unwrap_or((300.0, 150.0));
                Rect::new(0.0, 0.0, w, h)
            }
            TransformBox::FillBox | TransformBox::ContentBox => self.boxes(node, data, temp_id).fill,
            TransformBox::StrokeBox | TransformBox::BorderBox => self.boxes(node, data, temp_id).stroke,
        };

        // The transform functions, about the box's origin.
        let mut box_styles = style.get_box().clone();
        let zero = || LengthPercentage::new_length(style::values::computed::Length::new(0.0));
        box_styles.transform_origin =
            style::values::computed::TransformOrigin::new(zero(), zero(), style::values::computed::Length::new(0.0));
        let size = euclid::default::Rect::new(
            euclid::default::Point2D::new(CSSPixelLength::new(0.0), CSSPixelLength::new(0.0)),
            euclid::default::Size2D::new(
                CSSPixelLength::new(reference.width() as f32),
                CSSPixelLength::new(reference.height() as f32),
            ),
        );
        let mut transform = if css_transform {
            crate::stylo_to_kurbo::resolve_2d_transform(&box_styles, size).unwrap_or(Affine::IDENTITY)
        } else {
            Affine::IDENTITY
        };
        if !author.contains(&LonghandId::Transform) {
            if let Some(attr) = attr_transform {
                transform *= attr;
            }
        }

        // The origin: CSS `transform-origin` (the attribute too, as a presentational
        // hint), else `0 0`, SVG's initial origin.
        let (ox, oy) = if author.contains(&LonghandId::TransformOrigin) {
            let origin = &style.get_box().transform_origin;
            (
                origin.horizontal.resolve(CSSPixelLength::new(reference.width() as f32)).px() as f64,
                origin.vertical.resolve(CSSPixelLength::new(reference.height() as f32)).px() as f64,
            )
        } else {
            (0.0, 0.0)
        };
        let origin = Affine::translate((reference.x0 + ox, reference.y0 + oy));
        let matrix = origin * transform * origin.inverse();

        let [a, b, c, d, e, f] = matrix.as_coeffs();
        let value = format!("matrix({} {} {} {} {} {})", num(a), num(b), num(c), num(d), num(e), num(f));
        if attribute_name == "transform" {
            // A `transform` declaration in the element's `style` attribute (CSS
            // syntax, which usvg rejects) is overridden, and the origin is folded
            // into the matrix.
            rewrites.push(Rewrite { name: "transform", value: Some(value), in_style: true });
            rewrites.push(Rewrite { name: "transform-origin", value: Some("0 0".into()), in_style: true });
        } else {
            rewrites.push(Rewrite { name: attribute_name, value: Some(value), in_style: false });
        }
    }

    /// CSS `clip-path` on an SVG element: a reference as the attribute, a
    /// basic shape or a box as a `<clipPath>` made for it.
    fn clip_path_rewrite(
        &mut self,
        node: &Node,
        data: &ElementData,
        style: &ComputedValues,
        rewrites: &mut Vec<Rewrite>,
        temp_id: &mut Option<String>,
    ) {
        let (shape, geometry_box) = match style.get_svg().clip_path.clone() {
            ClipPath::None => {
                rewrites.push(Rewrite { name: "clip-path", value: None, in_style: true });
                return;
            }
            ClipPath::Url(_) => {
                let value = computed_value(style, LonghandId::ClipPath);
                rewrites.push(Rewrite { name: "clip-path", value: Some(value), in_style: true });
                return;
            }
            ClipPath::Shape(shape, geometry_box) => (Some(shape), geometry_box),
            ClipPath::Box(geometry_box) => (None, geometry_box),
        };
        let reference = match geometry_box {
            ShapeGeometryBox::FillBox
            | ShapeGeometryBox::ShapeBox(ShapeBox::ContentBox)
            | ShapeGeometryBox::ShapeBox(ShapeBox::PaddingBox) => Some(self.boxes(node, data, temp_id).fill),
            ShapeGeometryBox::ViewBox => {
                let (w, h) = self.viewports.last().copied().unwrap_or((300.0, 150.0));
                Some(Rect::new(0.0, 0.0, w, h))
            }
            // `border-box` (the default), `margin-box` and `stroke-box`.
            _ => Some(self.boxes(node, data, temp_id).stroke),
        };
        // A fill or stroke box is not known in the first pass: a clip in it
        // could leave the element out of usvg's tree, and so without the box
        // this pass is for.
        let from_boxes = !matches!(geometry_box, ShapeGeometryBox::ViewBox);
        let (Some(reference), false) = (reference, from_boxes && self.pass == Pass::Boxes) else {
            return;
        };
        let element = match &shape {
            Some(shape) => match shape_element(shape, reference) {
                Some(element) => element,
                // An unsupported shape keeps the element unclipped.
                None => return,
            },
            None => rect_element(reference, 0.0, 0.0),
        };
        let id = self.new_id("clip");
        let _ = write!(self.defs, "<clipPath id=\"{id}\" clipPathUnits=\"userSpaceOnUse\">{element}</clipPath>");
        rewrites.push(Rewrite { name: "clip-path", value: Some(format!("url(#{id})")), in_style: true });
    }

    /// CSS `mask-image: url(#mask)` on an SVG element as its `mask` attribute.
    fn mask_rewrite(&mut self, style: &ComputedValues, rewrites: &mut Vec<Rewrite>) {
        let images = &style.get_svg().mask_image.0;
        match images.first() {
            Some(GenericImage::Url(_)) if images.len() == 1 => {
                let value = computed_value(style, LonghandId::MaskImage);
                rewrites.push(Rewrite { name: "mask", value: Some(value), in_style: true });
            }
            Some(GenericImage::None) if images.len() == 1 => {
                rewrites.push(Rewrite { name: "mask", value: None, in_style: true });
            }
            _ => {}
        }
    }
}

/// `rect` as a `<rect>` with corner radii `rx`, `ry`; an empty one clips
/// everything away (an empty `<clipPath>` would be invalid, which usvg drops).
/// Whether a shape element has no geometry and renders nothing (SVG 2): a `rect` without a positive
/// `width` and `height`, an `image` of a zero one, a `circle` without a positive `r`, an `ellipse`
/// of a zero `rx` or `ry` or neither, a `path` or `polygon` without `d` or `points`. Read from its
/// attributes after the CSS geometry `rewrites`.
fn empty_shape(tag: &str, data: &ElementData, rewrites: &[Rewrite]) -> bool {
    let value = |name: &str| -> Option<String> {
        match rewrites.iter().find(|r| r.name == name && !r.in_style) {
            Some(rewrite) => rewrite.value.clone(),
            None => data
                .attrs()
                .iter()
                .find(|attr| attr.name.ns == ns!() && &*attr.name.local == name)
                .map(|attr| attr.value.to_string()),
        }
    };
    // A length's number: `None` for none (or `auto`), 1 for one that is not a plain number
    // (`calc()`), taken to be something.
    let number = |name: &str| -> Option<f64> {
        let value = value(name)?;
        let value = value.trim();
        if value.is_empty() || value == "auto" {
            return None;
        }
        let end = value
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0)))
            .map_or(value.len(), |(i, _)| i);
        Some(value[..end].parse().unwrap_or(1.0))
    };
    let positive = |name: &str| number(name).is_some_and(|v| v > 0.0);
    let zero = |name: &str| number(name).is_some_and(|v| v <= 0.0);
    let blank = |name: &str| value(name).is_none_or(|v| v.trim().is_empty() || v.trim() == "none");
    match tag {
        "rect" => !positive("width") || !positive("height"),
        "image" => zero("width") || zero("height"),
        "circle" => !positive("r"),
        "ellipse" => zero("rx") || zero("ry") || (number("rx").is_none() && number("ry").is_none()),
        "path" => blank("d"),
        "polygon" | "polyline" => blank("points"),
        _ => false,
    }
}

fn rect_element(rect: Rect, rx: f64, ry: f64) -> String {
    let (w, h) = (rect.width().max(0.0), rect.height().max(0.0));
    let radii = if rx > 0.0 && ry > 0.0 { format!(" rx=\"{}\" ry=\"{}\"", num(rx), num(ry)) } else { String::new() };
    format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{radii} />", num(rect.x0), num(rect.y0), num(w), num(h))
}

/// A CSS basic shape in `reference` (user units) as the SVG shape element usvg
/// builds the same geometry from as for its own elements (a `<circle>` for
/// `circle()`, ...), with its fill rule as the `clip-rule`.
fn shape_element(shape: &BasicShape, reference: Rect) -> Option<String> {
    let (w, h) = (reference.width(), reference.height());
    let (ox, oy) = (reference.x0, reference.y0);
    let lp = |v: &LengthPercentage, basis: f64| v.resolve(CSSPixelLength::new(basis as f32)).px() as f64;
    let center = |position: &GenericPositionOrAuto<style::values::computed::Position>| match position {
        GenericPositionOrAuto::Auto => (w / 2.0, h / 2.0),
        GenericPositionOrAuto::Position(pos) => (lp(&pos.horizontal, w), lp(&pos.vertical, h)),
    };
    // A radius along one axis (an ellipse's): `size` is the box along it, `c`
    // the centre's offset along it.
    let axis_radius = |r: &GenericShapeRadius<LengthPercentage>, size: f64, c: f64| match r {
        GenericShapeRadius::Length(length) => lp(&length.0, size),
        GenericShapeRadius::ClosestSide => c.min(size - c).max(0.0),
        GenericShapeRadius::FarthestSide => c.max(size - c).max(0.0),
        // Not in the grammar of `ellipse()`.
        GenericShapeRadius::ClosestCorner | GenericShapeRadius::FarthestCorner => c.max(size - c).max(0.0),
    };
    let element = match shape {
        GenericBasicShape::Circle(circle) => {
            let (cx, cy) = center(&circle.position);
            let r = match &circle.radius {
                // A percentage of the box's normalized diagonal.
                GenericShapeRadius::Length(length) => lp(&length.0, (w * w + h * h).sqrt() / std::f64::consts::SQRT_2),
                GenericShapeRadius::ClosestSide => cx.min(w - cx).min(cy).min(h - cy).max(0.0),
                GenericShapeRadius::FarthestSide => cx.max(w - cx).max(cy).max(h - cy).max(0.0),
                GenericShapeRadius::ClosestCorner => cx.min(w - cx).max(0.0).hypot(cy.min(h - cy).max(0.0)),
                GenericShapeRadius::FarthestCorner => cx.max(w - cx).hypot(cy.max(h - cy)),
            };
            format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" />", num(ox + cx), num(oy + cy), num(r))
        }
        GenericBasicShape::Ellipse(ellipse) => {
            let (cx, cy) = center(&ellipse.position);
            let rx = axis_radius(&ellipse.semiaxis_x, w, cx);
            let ry = axis_radius(&ellipse.semiaxis_y, h, cy);
            format!("<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" />", num(ox + cx), num(oy + cy), num(rx), num(ry))
        }
        GenericBasicShape::Polygon(polygon) => {
            if polygon.coordinates.is_empty() {
                return None;
            }
            let points: Vec<String> = polygon
                .coordinates
                .iter()
                .map(|point| format!("{},{}", num(ox + lp(&point.0, w)), num(oy + lp(&point.1, h))))
                .collect();
            let evenodd = matches!(polygon.fill, style::values::generics::basic_shape::FillRule::Evenodd);
            format!(
                "<polygon points=\"{}\"{} />",
                points.join(" "),
                if evenodd { " clip-rule=\"evenodd\"" } else { "" }
            )
        }
        GenericBasicShape::Rect(inset) => {
            let top = lp(&inset.rect.0, h);
            let right = lp(&inset.rect.1, w);
            let bottom = lp(&inset.rect.2, h);
            let left = lp(&inset.rect.3, w);
            let rect = Rect::new(ox + left, oy + top, ox + w - right, oy + h - bottom);
            let radius = |r: &style::values::computed::BorderCornerRadius| (lp(&r.0.width.0, w), lp(&r.0.height.0, h));
            let round = &inset.round;
            let radii = [
                radius(&round.top_left),
                radius(&round.top_right),
                radius(&round.bottom_right),
                radius(&round.bottom_left),
            ];
            if radii.iter().all(|r| *r == radii[0]) {
                rect_element(rect, radii[0].0, radii[0].1)
            } else {
                // Corner radii of their own: circular (their average), as kurbo's
                // rounded rectangles are.
                let r = |(x, y): (f64, f64)| (x + y) / 2.0;
                let path = kurbo::RoundedRect::from_rect(
                    rect,
                    kurbo::RoundedRectRadii::new(r(radii[0]), r(radii[1]), r(radii[2]), r(radii[3])),
                )
                .to_path(0.01);
                format!("<path d=\"{}\" />", path.to_svg())
            }
        }
        GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) => {
            // The path data, which serializes as a CSS string, from the box's origin.
            let data = path.path.to_css_string();
            let evenodd = matches!(path.fill, style::values::generics::basic_shape::FillRule::Evenodd);
            let mut d = String::new();
            encode_quoted_attribute_to_string(data.trim_matches(|c| c == '"' || c == '\''), &mut d);
            format!(
                "<path d=\"{d}\" transform=\"translate({} {})\"{} />",
                num(ox),
                num(oy),
                if evenodd { " clip-rule=\"evenodd\"" } else { "" }
            )
        }
        _ => return None,
    };
    Some(element)
}

#[cfg(test)]
mod tests {
    use super::{local_urls, num};

    #[test]
    fn urls_keep_their_fragment() {
        assert_eq!(local_urls(r##"url("#grad")"##), "url(#grad)");
        assert_eq!(local_urls(r##"url("http://a.test/p.html#m") rgb(0, 0, 0)"##), "url(#m) rgb(0, 0, 0)");
        assert_eq!(local_urls("url(img.png)"), "url(img.png)");
        assert_eq!(local_urls("none"), "none");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.0000001), "0");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(123.4567891), "123.456789");
    }
}
