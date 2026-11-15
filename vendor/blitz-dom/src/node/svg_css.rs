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
    /// The `<svg>` being written.
    root: &'a Node,
    /// The local names of its elements, by id (the first of an id), when needed.
    ids: Option<HashMap<String, String>>,
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
    /// The root's `perspective`, which its children's 3D transforms are seen through.
    root_perspective: Option<euclid::default::Transform3D<f64>>,
    /// Set while a text's shadow copy is written (see [`text_shadow`]).
    shadow_copy: Option<TextShadow>,
}

/// The source of the `<svg>` subtree at `root` for usvg (see the module docs).
pub(crate) fn svg_source(root: &Node) -> String {
    let guard = root.element_data().and_then(|data| data.guard.as_ref()).map(|lock| lock.read());
    let mut writer = Writer {
        out: String::new(),
        guard,
        root,
        ids: None,
        pass: Pass::Boxes,
        needs_boxes: false,
        box_ids: HashMap::new(),
        boxes: HashMap::new(),
        defs: String::new(),
        next_id: 0,
        viewports: Vec::new(),
        root_perspective: None,
        shadow_copy: None,
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

/// An SVG `<text>`'s `text-shadow`, which is written as a copy of the text before it, moved by
/// the offset and painted in the shadow's colour (blurred by half the blur radius), as Chromium
/// draws text shadows: what the text paints, its fill and stroke with their opacities and dashes.
/// `None` without one, or with several.
fn text_shadow(style: &ComputedValues, root: NodeId) -> Option<TextShadow> {
    use crate::util::ToColorColor as _;
    let text = style.get_inherited_text();
    let [shadow] = &text.text_shadow.0[..] else {
        return None;
    };
    let c = shadow.color.resolve_to_absolute(&text.color).as_color_color().to_rgba8();
    // In CSS px without the `zoom` Stylo's computed lengths have (see [`filter_value`]).
    let zoom = f64::from(style.effective_zoom.value());
    Some(TextShadow {
        dx: f64::from(shadow.horizontal.px()) / zoom,
        dy: f64::from(shadow.vertical.px()) / zoom,
        deviation: f64::from(shadow.blur.px()) / zoom / 2.0,
        color: [c.r, c.g, c.b, c.a],
        root,
    })
}

/// A `<text>`'s `text-shadow` (see [`text_shadow`]).
#[derive(Clone, Copy)]
struct TextShadow {
    dx: f64,
    dy: f64,
    /// Half the blur radius: the standard deviation Chromium blurs text shadows by.
    deviation: f64,
    color: [u8; 4],
    /// The `<text>`.
    root: NodeId,
}

/// Whether a `<g>` written around `node` (or next to it) is drawn as `node` would be: its parent
/// is a container that draws its children in order. Not in a `<switch>` (which would choose the
/// `<g>`, without `node`'s conditions, over `node`), a `<clipPath>` (which drops it), a `<text>`
/// or a gradient.
fn in_group_container(node: &Node) -> bool {
    let Some(parent) = node.parent.and_then(|id| node.tree()[id].element_data()) else {
        return false;
    };
    parent.name.ns == ns!(svg)
        && matches!(&*parent.name.local, "svg" | "g" | "a" | "symbol" | "defs" | "mask" | "pattern" | "marker")
}

/// An element's own fill (or stroke) as usvg reads it: from author CSS (which is written for it)
/// or else its attribute; `Some(None)` for `none`, `Some(Some(alpha))` for a paint (its colour's
/// alpha, 1 for others), `None` when it has none of its own (it inherits).
fn own_paint(data: &ElementData, style: &ComputedValues, author: &[LonghandId], fill: bool) -> Option<Option<f64>> {
    use style::values::generics::svg::SVGPaintKind;
    let (id, name) = if fill { (LonghandId::Fill, "fill") } else { (LonghandId::Stroke, "stroke") };
    if author.contains(&id) {
        let svg = style.get_inherited_svg();
        let paint = if fill { &svg.fill } else { &svg.stroke };
        return Some(match &paint.kind {
            SVGPaintKind::None => None,
            SVGPaintKind::Color(color) => Some(f64::from(color.resolve_to_absolute(&style.clone_color()).alpha)),
            _ => Some(1.0),
        });
    }
    let value = data.attr(markup5ever::LocalName::from(name))?.trim();
    match svgtypes::Paint::from_str(value) {
        Ok(svgtypes::Paint::None) => Some(None),
        Ok(svgtypes::Paint::Color(color)) => Some(Some(f64::from(color.alpha) / 255.0)),
        Ok(_) => Some(Some(1.0)),
        Err(_) => None,
    }
}

/// The declarations that paint an element of a text's shadow copy: its fills and strokes that
/// are not `none` in the shadow's colour, with their colours' alphas times its alpha. The text
/// itself (`top`) gets the paints it has, inherited ones included (`inherited`); an element in
/// it only those it has of its own (it inherits the rest from the copy).
fn shadow_paint_declarations(
    own: [Option<Option<f64>>; 2],
    inherited: [Option<f64>; 2],
    top: bool,
    [r, g, b, a]: [u8; 4],
    out: &mut String,
) {
    for (i, name) in ["fill", "stroke"].into_iter().enumerate() {
        let paint = match (own[i], top) {
            (Some(paint), _) => paint,
            (None, true) => inherited[i],
            (None, false) => continue,
        };
        if let Some(alpha) = paint {
            let _ = write!(out, "{name}:rgba({r},{g},{b},{});", num(alpha * f64::from(a) / 255.0));
        }
    }
}

/// A `drop-shadow()` usvg reads: its colour resolved to `#rrggbbaa` (usvg knows neither
/// `currentcolor` nor `color-mix()`, and a colour function's parentheses would end the value in
/// a `style` attribute: [`nested_parentheses`]), its third length the standard deviation (as
/// usvg, and Chromium, take it). The lengths are in CSS px, as SVG user units are.
fn drop_shadow_function(
    x: f64,
    y: f64,
    deviation: f64,
    color: &style::values::computed::Color,
    current_color: &style::color::AbsoluteColor,
) -> String {
    use crate::util::ToColorColor as _;
    let c = color.resolve_to_absolute(current_color).as_color_color().to_rgba8();
    format!("drop-shadow({}px {}px {}px #{:02x}{:02x}{:02x}{:02x})", num(x), num(y), num(deviation), c.r, c.g, c.b, c.a)
}

/// An element's computed `filter` for usvg: `url()`s as their fragment, drop shadows with
/// their colours resolved ([`drop_shadow_function`]), the other functions as CSS writes them.
/// Lengths in CSS px without the element's `zoom` (Stylo's computed lengths have it; the
/// resolved value does not): the `<svg>`'s box scales its user units by it.
fn filter_value(style: &ComputedValues) -> String {
    use style::computed_values::filter::single_value::T as Filter;
    let current_color = style.clone_color();
    let filters = &style.get_effects().filter.0;
    if filters.is_empty() {
        return "none".to_string();
    }
    let zoom = f64::from(style.effective_zoom.value());
    let unzoom = |px: f32| f64::from(px) / zoom;
    filters
        .iter()
        .map(|filter| match filter {
            Filter::DropShadow(shadow) => drop_shadow_function(
                unzoom(shadow.horizontal.px()),
                unzoom(shadow.vertical.px()),
                unzoom(shadow.blur.px()),
                &shadow.color,
                &current_color,
            ),
            Filter::Blur(length) => format!("blur({}px)", num(unzoom(length.0.px()))),
            Filter::Url(_) => local_urls(&filter.to_css_string()),
            filter => filter.to_css_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `value` has parentheses in parentheses (`drop-shadow(rgb(0, 0, 0) 1px 1px)`): usvg's
/// `style` attribute tokenizer ends a function at its first `)`, and drops the declaration.
fn nested_parentheses(value: &str) -> bool {
    let mut depth = 0usize;
    for c in value.chars() {
        match c {
            '(' => {
                depth += 1;
                if depth > 1 {
                    return true;
                }
            }
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// The fragment of a lone `url(...)` value (`url(#a)`, `url("#a")`, `url(doc.svg#a)`).
fn url_fragment(value: &str) -> Option<String> {
    let inner = value.trim().strip_prefix("url(")?.strip_suffix(')')?;
    let inner = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    let (_, fragment) = inner.rsplit_once('#')?;
    (!fragment.is_empty()).then(|| fragment.to_string())
}

/// Whether `value` is a transform list as SVG's grammar has it (and Chromium's parser): what
/// svgtypes reads, without a comma that separates nothing (`rotate(90,)`, `scale(,2)`,
/// `translate(1,,2)`, a list ending in one), which svgtypes lets through.
fn is_transform_list(value: &str) -> bool {
    if value.parse::<svgtypes::Transform>().is_err() {
        return false;
    }
    // The previous character that is not white space.
    let mut previous = None;
    for c in value.chars() {
        if c.is_ascii_whitespace() {
            continue;
        }
        let separates_nothing = match (previous, c) {
            (Some('(' | ','), ',') | (Some(','), ')') => true,
            _ => false,
        };
        if separates_nothing {
            return false;
        }
        previous = Some(c);
    }
    previous != Some(',')
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
        // A `<text>`'s `text-shadow`: a copy of it before it, moved by the offset and painted
        // in the shadow's colour ([`text_shadow`]).
        if nesting > 0
            && self.shadow_copy.is_none()
            && data.name.ns == ns!(svg)
            && &*data.name.local == "text"
            && in_group_container(node)
        {
            let shadow = node.primary_styles().and_then(|style| text_shadow(&style, node.id));
            if let Some(shadow) = shadow.filter(|shadow| shadow.color[3] > 0) {
                let _ = write!(self.out, "<g transform=\"translate({} {})\"", num(shadow.dx), num(shadow.dy));
                if shadow.deviation > 0.0 {
                    let _ = write!(self.out, " filter=\"blur({}px)\"", num(shadow.deviation));
                }
                self.out.push('>');
                self.shadow_copy = Some(shadow);
                self.write_element(node, data, nesting);
                self.shadow_copy = None;
                self.out.push_str("</g>");
            }
        }
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
            if is_root {
                // In the root's box, which its user space is (without a `viewBox`).
                self.root_perspective = styles.as_deref().and_then(|style| {
                    let size = euclid::default::Rect::new(
                        euclid::default::Point2D::new(CSSPixelLength::new(0.0), CSSPixelLength::new(0.0)),
                        euclid::default::Size2D::new(
                            CSSPixelLength::new(viewport.0 as f32),
                            CSSPixelLength::new(viewport.1 as f32),
                        ),
                    );
                    crate::stylo_to_kurbo::resolve_perspective_matrix(style.get_box(), size)
                });
            }
        }

        let mut rewrites: Vec<Rewrite> = Vec::new();
        let mut declarations = String::new();
        let mut temp_id = None;
        if let (Some(style), true) = (&styles, is_svg) {
            // `filter` is a declaration of the `style` attribute (which wins over the `<svg>`'s
            // own style sheets, as the page's value does), or the attribute when it has
            // parentheses in parentheses, which usvg's `style` tokenizer cannot read
            // ([`nested_parentheses`]).
            if !is_root && author.contains(&LonghandId::Filter) {
                let value = filter_value(style);
                let value = match current_color.as_deref().and_then(|c| replace_current_color(&value, c)) {
                    Some(replaced) => replaced,
                    None => value,
                };
                let in_style = !nested_parentheses(&value);
                rewrites.push(Rewrite { name: "filter", value: Some(value), in_style });
            }
            for &(id, name) in STYLE_PROPERTIES {
                // The root's box effects (`opacity`, `display`, `filter`, ...) are
                // its CSS box's, which blitz-paint applies.
                if is_root && !ROOT_PROPERTIES.contains(&id) {
                    continue;
                }
                if id == LonghandId::Filter {
                    continue;
                }
                if author.contains(&id) {
                    let value = computed_value(style, id);
                    let _ = write!(declarations, "{name}:{value};");
                }
            }
            if let (Some(shadow), false) = (self.shadow_copy, is_root) {
                let own = [own_paint(data, style, &author, true), own_paint(data, style, &author, false)];
                let top = shadow.root == node.id;
                let inherited = if top { self.inherited_paints(node) } else { [None, None] };
                shadow_paint_declarations(own, inherited, top, shadow.color, &mut declarations);
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
                    let under_root = nesting == 1;
                    self.transform_rewrite(node, data, tag, style, &author, &mut rewrites, &mut temp_id, under_root);
                }
            }
            // Not on the root: its CSS box's.
            if !is_root {
                if author.contains(&LonghandId::ClipPath) {
                    self.clip_path_rewrite(node, data, style, &mut rewrites, &mut temp_id);
                }
                if author.contains(&LonghandId::MaskImage) {
                    if !self.css_mask_rewrite(node, data, style, &mut rewrites, &mut temp_id) {
                        self.mask_rewrite(style, &mut rewrites);
                    }
                }
            }
        }
        if is_svg && !is_root {
            self.invalid_reference_rewrites(data, &mut rewrites);
            // A transform attribute that is no transform list (`rotate(90,)`) is ignored, as
            // in Chromium; usvg reads some of them.
            for name in ["transform", "patternTransform", "gradientTransform"] {
                if rewrites.iter().any(|r| r.name == name) {
                    continue;
                }
                if data.attr(markup5ever::LocalName::from(name)).is_some_and(|v| !is_transform_list(v)) {
                    rewrites.push(Rewrite { name, value: None, in_style: false });
                }
            }
        }

        // `mix-blend-mode: plus-lighter`, which usvg does not know: a group anyrender_svg
        // composites with the operator its id names.
        let plus_lighter = is_svg
            && !is_root
            && in_group_container(node)
            && author.contains(&LonghandId::MixBlendMode)
            && styles.as_ref().is_some_and(|style| {
                style.get_effects().mix_blend_mode == style::computed_values::mix_blend_mode::T::PlusLighter
            });
        if plus_lighter {
            let id = self.new_id("compose");
            let _ = write!(self.out, "<g id=\"__pyro_compose_pluslighter_{}\">", &id["__pyro_compose_".len()..]);
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
            if (temp_id.is_some() || self.shadow_copy.is_some()) && attr.name.ns == ns!() && local == "id" {
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
        if let (Some(id), None) = (&temp_id, self.shadow_copy) {
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
        if plus_lighter {
            self.out.push_str("</g>");
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
        under_root: bool,
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
            .filter(|v| is_transform_list(v))
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
        let (ox, oy) = (reference.x0 + ox, reference.y0 + oy);

        use crate::stylo_to_kurbo::{Flattened, ResolvedTransform, flatten_transform, resolve_transform};
        let resolved = if css_transform { resolve_transform(&box_styles, size) } else { ResolvedTransform::Flat(None) };
        let matrix = match resolved {
            ResolvedTransform::Flat(transform) => {
                let mut transform = transform.unwrap_or(Affine::IDENTITY);
                if !author.contains(&LonghandId::Transform) {
                    if let Some(attr) = attr_transform {
                        transform *= attr;
                    }
                }
                let origin = Affine::translate((ox, oy));
                origin * transform * origin.inverse()
            }
            // A 3D transform (and the root's `perspective`, for its children), flattened into
            // the plane: exactly when the element stays parallel to it (a depth under a
            // perspective is a scale about the perspective origin).
            ResolvedTransform::Spatial(m) => {
                // The attribute's transform first, as above, when CSS sets no `transform`.
                let attr = match attr_transform {
                    Some(attr) if !author.contains(&LonghandId::Transform) => {
                        let [a, b, c, d, e, f] = attr.as_coeffs();
                        euclid::default::Transform3D::new_2d(a, b, c, d, e, f)
                    }
                    _ => euclid::default::Transform3D::identity(),
                };
                let mut m = euclid::default::Transform3D::translation(-ox, -oy, 0.0)
                    .then(&attr)
                    .then(&m)
                    .then(&euclid::default::Transform3D::translation(ox, oy, 0.0));
                if let (true, Some(perspective)) = (under_root, self.root_perspective) {
                    m = m.then(&perspective);
                }
                let in_box = euclid::default::Transform3D::translation(reference.x0, reference.y0, 0.0).then(&m);
                match flatten_transform(&in_box, (reference.width(), reference.height())) {
                    Flattened::Affine(affine) => affine * Affine::translate((-reference.x0, -reference.y0)),
                    // Behind the viewer: nothing is drawn (usvg leaves out an element of a
                    // transform it cannot invert).
                    Flattened::Hidden => Affine::new([0.0; 6]),
                }
            }
        };

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

    /// The fill and stroke `node` inherits as usvg reads them ([`own_paint`] of the nearest SVG
    /// ancestor that has one; else black and `none`).
    fn inherited_paints(&self, node: &Node) -> [Option<f64>; 2] {
        let mut out: [Option<Option<f64>>; 2] = [None, None];
        let mut next = node.parent;
        while let Some(id) = next {
            let ancestor = &node.tree()[id];
            let Some(data) = ancestor.element_data().filter(|data| data.name.ns == ns!(svg)) else {
                break;
            };
            if let (Some(style), Some(guard)) = (ancestor.primary_styles(), &self.guard) {
                let author = author_longhands(&style, guard);
                for (i, slot) in out.iter_mut().enumerate() {
                    if slot.is_none() {
                        *slot = own_paint(data, &style, &author, i == 0);
                    }
                }
            }
            next = ancestor.parent;
        }
        [out[0].unwrap_or(Some(1.0)), out[1].unwrap_or(None)]
    }

    /// The local name of the element of the `<svg>` being written whose id is `id` (the first
    /// one, as usvg takes it).
    fn element_named(&mut self, id: &str) -> Option<&str> {
        let ids = self.ids.get_or_insert_with(|| {
            let mut ids = HashMap::new();
            let mut stack = vec![self.root];
            while let Some(node) = stack.pop() {
                if let Some(data) = node.element_data() {
                    if let Some(id) = data.attr(local_name!("id")).filter(|id| !id.is_empty()) {
                        ids.entry(id.to_string()).or_insert_with(|| data.name.local.to_string());
                    }
                }
                // In tree order: the first of an id wins.
                stack.extend(node.children.iter().rev().map(|&child| &node.tree()[child]));
            }
            ids
        });
        ids.get(id).map(String::as_str)
    }

    /// `clip-path` and `mask` references (attributes, or as CSS wrote them) to an element of
    /// this `<svg>` that is no `<clipPath>` or `<mask>`, as `none`: such a reference is ignored
    /// (CSS Masking; usvg leaves the element out instead).
    fn invalid_reference_rewrites(&mut self, data: &ElementData, rewrites: &mut Vec<Rewrite>) {
        for (name, element) in [("clip-path", "clipPath"), ("mask", "mask")] {
            let value = match rewrites.iter().find(|r| r.name == name) {
                Some(rewrite) => rewrite.value.clone(),
                None => data.attr(markup5ever::LocalName::from(name)).map(str::to_string),
            };
            let Some(fragment) = value.as_deref().and_then(url_fragment) else {
                continue;
            };
            if self.element_named(&fragment).is_none_or(|found| found == element) {
                continue;
            }
            rewrites.retain(|r| r.name != name);
            rewrites.push(Rewrite { name, value: None, in_style: true });
        }
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

    /// The SVG box a `mask-clip` or `mask-origin` value names on an SVG element (CSS Masking:
    /// `content-box` and `padding-box` are its fill box, `border-box` its stroke box), in its
    /// user space; `None` for `no-clip`, `Err` for a box this does not do.
    fn mask_box(&self, kind: MaskBox, boxes: Boxes) -> Result<Option<Rect>, ()> {
        Ok(Some(match kind {
            MaskBox::Fill => boxes.fill,
            MaskBox::Stroke => boxes.stroke,
            MaskBox::View => {
                let (w, h) = self.viewports.last().copied().unwrap_or((300.0, 150.0));
                Rect::new(0.0, 0.0, w, h)
            }
            MaskBox::NoClip => return Ok(None),
            MaskBox::Unsupported => return Err(()),
        }))
    }

    /// CSS mask layers on an SVG element (`mask-image` SVG images, linear gradients and
    /// references to a `<mask>`, placed by `mask-size`, `mask-position`, `mask-repeat` and
    /// `mask-origin`, cut to `mask-clip`, with `mask-mode` and `mask-composite`) as a `<mask>`
    /// made for it, which usvg applies as it applies its own: each layer is its tiles in a group
    /// clipped to its clip box, a luminance layer the alpha of a white rectangle under a luminance
    /// `<mask>` of them, and each layer after the bottom one is composited onto those below it
    /// with its operator by anyrender_svg (a group whose id names the operator,
    /// `__pyro_compose_*`). An image that failed to load is a transparent layer. Returns `false`
    /// when the mask is left to [`Self::mask_rewrite`]: one reference to a `<mask>` of this
    /// `<svg>` (usvg's own `mask`), `none`, or a layer this does not draw (a raster image, a
    /// gradient other than a linear one), which leaves the element unmasked, as before.
    fn css_mask_rewrite(
        &mut self,
        node: &Node,
        data: &ElementData,
        style: &ComputedValues,
        rewrites: &mut Vec<Rewrite>,
        temp_id: &mut Option<String>,
    ) -> bool {
        use crate::node::{ImageData, Status};
        use style::values::specified::svg::MaskMode;

        let svg = style.get_svg();
        let images = &svg.mask_image.0;
        if images.iter().all(|image| matches!(image, GenericImage::None)) {
            return false;
        }
        let mode = |i: usize| *cyclic(&svg.mask_mode.0, i);
        // A reference to one `<mask>` of this `<svg>`: usvg's `mask`.
        let local_mask = |this: &mut Self, image: &GenericImage<_, _, _, _, _, _>| -> Option<String> {
            let GenericImage::Url(_) = image else {
                return None;
            };
            let fragment = url_fragment(&local_urls(&image.to_css_string()))?;
            (this.element_named(&fragment) == Some("mask")).then_some(fragment)
        };
        // One such reference is usvg's own `mask`, whose region keeps a stroke past the fill box
        // (a `mask-mode` other than `match-source` is not applied to a `<mask>` either way).
        if let [image] = &images[..] {
            if local_mask(self, image).is_some() {
                return false;
            }
        }

        // What each layer draws.
        let zoom = style.effective_zoom.value() as f64;
        let current_color = style.clone_color();
        let mut sources = Vec::with_capacity(images.len());
        for (i, image) in images.iter().enumerate() {
            let source = match image {
                GenericImage::None => LayerSource::None,
                GenericImage::Url(_) => match local_mask(self, image) {
                    Some(fragment) => LayerSource::Mask(fragment),
                    None => match data.mask_images.get(i).and_then(Option::as_ref) {
                        Some(resource) if resource.status == Status::Ok => match &resource.image {
                            ImageData::Svg(loaded) => match &loaded.source {
                                // A `<mask>` of another document (`url(sprite.svg#m)`), which this
                                // does not resolve: the element is left unmasked, as before. As
                                // the layer's image, the document would mask by what it draws
                                // (nothing, for a sprite).
                                Some(source)
                                    if url_fragment(&image.to_css_string())
                                        .is_some_and(|id| document_has_mask(source, &id)) =>
                                {
                                    return false;
                                }
                                Some(source) => LayerSource::Svg {
                                    source: source.clone(),
                                    width: loaded.intrinsic_width().map(f64::from),
                                    height: loaded.intrinsic_height().map(f64::from),
                                    ratio: match (loaded.intrinsic_width(), loaded.intrinsic_height()) {
                                        (Some(w), Some(h)) if h > 0.0 => Some(f64::from(w / h)),
                                        _ => loaded.viewbox_aspect_ratio().map(f64::from),
                                    },
                                },
                                None => return false,
                            },
                            ImageData::None => LayerSource::None,
                            ImageData::Raster(_) => return false,
                        },
                        // An image that failed to load (or has not loaded) is a transparent layer;
                        // as the only layer, it leaves the element unmasked, as in Chromium (WPT
                        // `bad-mask-image-svg-2` with one, `bad-mask-image-svg` with two).
                        _ if images.len() == 1 => return false,
                        _ => LayerSource::None,
                    },
                },
                GenericImage::Gradient(gradient) => match linear_gradient(gradient, &current_color, zoom) {
                    Some(gradient) => LayerSource::Gradient(gradient),
                    None => return false,
                },
                _ => return false,
            };
            sources.push(source);
        }

        // The boxes, from the first pass; nothing is masked in it (a mask could leave the element
        // out of usvg's tree, and so without the box this pass is for).
        let boxes = self.boxes(node, data, temp_id);
        if self.pass == Pass::Boxes {
            return true;
        }
        let mut layers = Vec::with_capacity(sources.len());
        for (i, source) in sources.into_iter().enumerate() {
            let (Ok(clip), Ok(Some(origin))) = (
                self.mask_box(MaskBox::from_clip(*cyclic(&svg.mask_clip.0, i)), boxes),
                self.mask_box(MaskBox::from_origin(*cyclic(&svg.mask_origin.0, i)), boxes),
            ) else {
                return false;
            };
            layers.push((source, clip, origin));
        }
        // The mask painting area: the clip boxes' union, or what can show for `no-clip`.
        let area = layers.iter().try_fold(Rect::ZERO, |area: Rect, (_, clip, _)| {
            clip.map(|clip| if area.is_zero_area() { clip } else { area.union(clip) })
        });
        let area = area.unwrap_or(Rect::new(-1e6, -1e6, 1e6, 1e6));
        if !(area.width() > 0.0 && area.height() > 0.0) {
            return false;
        }

        // The layers, bottom first.
        let mut content = String::new();
        let count = layers.len();
        for (i, (source, clip, origin)) in layers.iter().enumerate().rev() {
            let painted = clip.unwrap_or(area);
            let mut layer = String::new();
            match source {
                LayerSource::None => {}
                LayerSource::Mask(fragment) => {
                    // A white rectangle over the fill box (the box a `<mask>` in
                    // `objectBoundingBox` units is relative to) under the `<mask>`.
                    let rect = boxes.fill;
                    let _ = write!(
                        layer,
                        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fff\" mask=\"url(#{})\" />",
                        num(rect.x0),
                        num(rect.y0),
                        num(rect.width()),
                        num(rect.height()),
                        escape(fragment)
                    );
                }
                LayerSource::Svg { .. } | LayerSource::Gradient(_) => {
                    let (intrinsic_w, intrinsic_h, ratio) = match source {
                        LayerSource::Svg { width, height, ratio, .. } => (*width, *height, *ratio),
                        _ => (None, None, None),
                    };
                    let size = cyclic(&svg.mask_size.0, i);
                    let repeat = cyclic(&svg.mask_repeat.0, i);
                    let Some((tile_w, tile_h)) =
                        tile_size(size, repeat, origin, (intrinsic_w, intrinsic_h, ratio), zoom)
                    else {
                        // An image of no size draws nothing.
                        layers_push(&mut content, i, count, &layer, svg);
                        continue;
                    };
                    let position = (cyclic(&svg.mask_position_x.0, i), cyclic(&svg.mask_position_y.0, i));
                    let resolve = |lp: &LengthPercentage, basis: f64| {
                        lp.resolve(CSSPixelLength::new((basis * zoom) as f32)).px() as f64 / zoom
                    };
                    let xs = axis_tiles(
                        origin.x0,
                        origin.width(),
                        painted.x0,
                        painted.x1,
                        tile_w,
                        resolve(position.0, origin.width() - tile_w),
                        repeat.0,
                    );
                    let ys = axis_tiles(
                        origin.y0,
                        origin.height(),
                        painted.y0,
                        painted.y1,
                        tile_h,
                        resolve(position.1, origin.height() - tile_h),
                        repeat.1,
                    );
                    let (Some(xs), Some(ys)) = (xs, ys) else {
                        return false;
                    };
                    if xs.len() * ys.len() > MAX_AXIS_TILES {
                        return false;
                    }
                    // The tile, defined once.
                    let tile_id = self.new_id("mask_tile");
                    match source {
                        LayerSource::Svg { source, .. } => {
                            let _ = write!(
                                self.defs,
                                "<image id=\"{tile_id}\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" \
                                 href=\"data:image/svg+xml;base64,{}\" />",
                                num(tile_w),
                                num(tile_h),
                                base64(source)
                            );
                        }
                        LayerSource::Gradient(gradient) => {
                            let gradient_id = self.new_id("mask_gradient");
                            let _ = write!(self.defs, "{}", gradient.element(&gradient_id, tile_w, tile_h));
                            let _ = write!(
                                self.defs,
                                "<rect id=\"{tile_id}\" width=\"{}\" height=\"{}\" fill=\"url(#{gradient_id})\" />",
                                num(tile_w),
                                num(tile_h)
                            );
                        }
                        _ => unreachable!(),
                    }
                    let mut tiles = String::new();
                    let mut inside = true;
                    for &y in &ys {
                        for &x in &xs {
                            let _ = write!(tiles, "<use href=\"#{tile_id}\" x=\"{}\" y=\"{}\" />", num(x), num(y));
                            inside &= x >= painted.x0 - 1e-6
                                && y >= painted.y0 - 1e-6
                                && x + tile_w <= painted.x1 + 1e-6
                                && y + tile_h <= painted.y1 + 1e-6;
                        }
                    }
                    if inside || clip.is_none() {
                        layer = tiles;
                    } else {
                        let clip_id = self.new_id("mask_clip");
                        let _ = write!(
                            self.defs,
                            "<clipPath id=\"{clip_id}\" clipPathUnits=\"userSpaceOnUse\">{}</clipPath>",
                            rect_element(painted, 0.0, 0.0)
                        );
                        layer = format!("<g clip-path=\"url(#{clip_id})\">{tiles}</g>");
                    }
                    if mode(i) == MaskMode::Luminance && !layer.is_empty() {
                        // The layer's luminance as the alpha of a white rectangle.
                        let mask_id = self.new_id("mask_luminance");
                        let _ = write!(
                            self.defs,
                            "<mask id=\"{mask_id}\" maskUnits=\"userSpaceOnUse\" maskContentUnits=\"userSpaceOnUse\" \
                             x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" style=\"mask-type:luminance\">{layer}</mask>",
                            num(painted.x0),
                            num(painted.y0),
                            num(painted.width()),
                            num(painted.height())
                        );
                        layer = format!(
                            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fff\" mask=\"url(#{mask_id})\" />",
                            num(painted.x0),
                            num(painted.y0),
                            num(painted.width()),
                            num(painted.height())
                        );
                    }
                }
            }
            layers_push(&mut content, i, count, &layer, svg);
        }

        let mask_id = self.new_id("css_mask");
        let _ = write!(
            self.defs,
            "<mask id=\"{mask_id}\" maskUnits=\"userSpaceOnUse\" maskContentUnits=\"userSpaceOnUse\" x=\"{}\" \
             y=\"{}\" width=\"{}\" height=\"{}\" style=\"mask-type:alpha\">{content}</mask>",
            num(area.x0),
            num(area.y0),
            num(area.width()),
            num(area.height())
        );
        rewrites.retain(|r| r.name != "mask");
        rewrites.push(Rewrite { name: "mask", value: Some(format!("url(#{mask_id})")), in_style: true });
        true
    }
}

/// Whether the SVG document `source` has a `<mask>` of id `id`.
fn document_has_mask(source: &[u8], id: &str) -> bool {
    use usvg::roxmltree;
    let Ok(text) = std::str::from_utf8(source) else {
        return false;
    };
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    roxmltree::Document::parse_with_options(text, options).is_ok_and(|doc| {
        doc.descendants().any(|node| node.tag_name().name() == "mask" && node.attribute("id") == Some(id))
    })
}

/// A CSS mask layer's image (see [`Writer::css_mask_rewrite`]).
enum LayerSource {
    None,
    /// A `<mask>` of the `<svg>`, by id.
    Mask(String),
    /// An SVG image: its document, natural size and ratio.
    Svg {
        source: std::sync::Arc<[u8]>,
        width: Option<f64>,
        height: Option<f64>,
        ratio: Option<f64>,
    },
    Gradient(LinearGradient),
}

/// `layer` (CSS mask layer `i` of `count`) added to the layers below it in `content`: the bottom
/// one as it is, the others in a group anyrender_svg composites with their `mask-composite`.
fn layers_push(content: &mut String, i: usize, count: usize, layer: &str, svg: &style::properties::style_structs::SVG) {
    use style::values::specified::svg::MaskComposite;
    if i + 1 == count {
        content.push_str(layer);
        return;
    }
    let operator = match *cyclic(&svg.mask_composite.0, i) {
        MaskComposite::Add => "srcover",
        MaskComposite::Subtract => "srcout",
        MaskComposite::Intersect => "srcin",
        MaskComposite::Exclude => "xor",
    };
    let _ = write!(content, "<g id=\"__pyro_compose_{operator}_{i}\">{layer}</g>");
}

/// The SVG boxes of `mask-clip` and `mask-origin` (see [`Writer::mask_box`]).
#[derive(Clone, Copy)]
enum MaskBox {
    Fill,
    Stroke,
    View,
    NoClip,
    Unsupported,
}

impl MaskBox {
    fn from_clip(clip: style::values::specified::background::BackgroundClip) -> Self {
        use style::values::specified::background::BackgroundClip as C;
        match clip {
            C::ContentBox | C::PaddingBox | C::FillBox => Self::Fill,
            C::BorderBox | C::StrokeBox => Self::Stroke,
            C::ViewBox => Self::View,
            C::NoClip => Self::NoClip,
            #[allow(unreachable_patterns)]
            _ => Self::Unsupported,
        }
    }

    fn from_origin(origin: style::values::specified::background::BackgroundOrigin) -> Self {
        use style::values::specified::background::BackgroundOrigin as O;
        match origin {
            O::ContentBox | O::PaddingBox | O::FillBox => Self::Fill,
            O::BorderBox | O::StrokeBox => Self::Stroke,
            O::ViewBox => Self::View,
        }
    }
}

/// The item of a CSS list for layer `i` (lists repeat to the number of layers).
fn cyclic<T>(list: &[T], i: usize) -> &T {
    &list[i % list.len()]
}

/// The size of a mask layer's tile (`mask-size`, then `round` repeats fitting a whole number
/// of tiles), from the positioning area `origin` and the image's natural width, height and
/// ratio; `None` when it is empty.
fn tile_size(
    size: &style::values::computed::background::BackgroundSize,
    repeat: &style::values::specified::background::BackgroundRepeat,
    origin: &Rect,
    (natural_w, natural_h, ratio): (Option<f64>, Option<f64>, Option<f64>),
    zoom: f64,
) -> Option<(f64, f64)> {
    use style::values::generics::background::GenericBackgroundSize as Size;
    use style::values::generics::length::GenericLengthPercentageOrAuto as Auto;
    use style::values::specified::background::BackgroundRepeatKeyword as Repeat;
    let (area_w, area_h) = (origin.width(), origin.height());
    let contain = |cover: bool| match ratio {
        Some(r) if r > 0.0 => {
            let wider = area_w / area_h > r;
            if wider != cover { (area_h * r, area_h) } else { (area_w, area_w / r) }
        }
        _ => (area_w, area_h),
    };
    let resolve = |value: &Auto<style::values::computed::NonNegativeLengthPercentage>, basis: f64| match value {
        Auto::Auto => None,
        Auto::LengthPercentage(lp) => Some(lp.0.resolve(CSSPixelLength::new((basis * zoom) as f32)).px() as f64 / zoom),
    };
    let (mut w, mut h, w_auto, h_auto) = match size {
        Size::Cover => {
            let (w, h) = contain(true);
            (w, h, false, false)
        }
        Size::Contain => {
            let (w, h) = contain(false);
            (w, h, false, false)
        }
        Size::ExplicitSize { width, height } => {
            let (w, h) = (resolve(width, area_w), resolve(height, area_h));
            let (w, h) = match (w, h) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, ratio.map_or(natural_h.unwrap_or(area_h), |r| w / r)),
                (None, Some(h)) => (ratio.map_or(natural_w.unwrap_or(area_w), |r| h * r), h),
                (None, None) => match (natural_w, natural_h, ratio) {
                    (Some(w), Some(h), _) => (w, h),
                    (Some(w), None, Some(r)) => (w, w / r),
                    (None, Some(h), Some(r)) => (h * r, h),
                    (_, _, Some(_)) => contain(false),
                    _ => (natural_w.unwrap_or(area_w), natural_h.unwrap_or(area_h)),
                },
            };
            (w, h, width.is_auto(), height.is_auto())
        }
    };
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return None;
    }
    // `round`: a whole number of tiles in the positioning area; an `auto` other side keeps the
    // ratio.
    let rounded = |tile: f64, area: f64| area / (area / tile).round().max(1.0);
    match (repeat.0 == Repeat::Round, repeat.1 == Repeat::Round) {
        (true, true) => (w, h) = (rounded(w, area_w), rounded(h, area_h)),
        (true, false) => {
            let new_w = rounded(w, area_w);
            if h_auto {
                h *= new_w / w;
            }
            w = new_w;
        }
        (false, true) => {
            let new_h = rounded(h, area_h);
            if w_auto {
                w *= new_h / h;
            }
            h = new_h;
        }
        (false, false) => {}
    }
    Some((w, h))
}

/// The most tiles a mask layer has along one axis ([`axis_tiles`]); more leave the element to
/// [`Writer::mask_rewrite`].
const MAX_AXIS_TILES: usize = 4096;

/// Where a mask layer's tiles start along one axis: the positioning area starts at `start` and
/// is `length` long, the painted area runs from `from` to `to`, a tile is `tile` long, the
/// position (resolved against the area less a tile) is `offset`. `None` for more than
/// [`MAX_AXIS_TILES`] tiles.
fn axis_tiles(
    start: f64,
    length: f64,
    from: f64,
    to: f64,
    tile: f64,
    offset: f64,
    repeat: style::values::specified::background::BackgroundRepeatKeyword,
) -> Option<Vec<f64>> {
    use style::values::specified::background::BackgroundRepeatKeyword as Repeat;
    // Counted before any is placed: a period below the float spacing at `from` would not move
    // a tile on (`1000 + 1e-14 == 1000`).
    let repeated = |first: f64, period: f64| -> Option<Vec<f64>> {
        if !(period > 0.0 && period.is_finite()) {
            return None;
        }
        let x0 = first - ((first - from) / period).ceil() * period;
        let count = ((to - x0) / period).ceil();
        if !(count.is_finite() && count <= (MAX_AXIS_TILES + 1) as f64) {
            return None;
        }
        let tiles: Vec<f64> = (0..count.max(0.0) as usize)
            .map(|k| x0 + k as f64 * period)
            .filter(|&x| x < to && x + tile > from)
            .collect();
        (tiles.len() <= MAX_AXIS_TILES).then_some(tiles)
    };
    match repeat {
        Repeat::NoRepeat => Some(vec![start + offset]),
        Repeat::Repeat | Repeat::Round => repeated(start + offset, tile),
        Repeat::Space => {
            let n = (length / tile + 1e-9).floor();
            if n >= 2.0 {
                repeated(start, tile + (length - n * tile) / (n - 1.0))
            } else if n >= 1.0 {
                Some(vec![start + offset])
            } else {
                Some(Vec::new())
            }
        }
    }
}

/// A CSS `linear-gradient()` (not repeating) for a tile: its angle and its stops, with positions
/// as fractions of the gradient line or lengths on it.
struct LinearGradient {
    /// Clockwise from "to top", in radians; `None` for a corner (`to top right`), which depends
    /// on the tile's shape: (rightwards, downwards).
    angle: Result<f64, (bool, bool)>,
    stops: Vec<(Option<StopPosition>, [u8; 4])>,
}

enum StopPosition {
    Fraction(f64),
    /// A length in user units.
    Length(f64),
}

impl LinearGradient {
    /// The gradient for a `w` x `h` tile at the origin, as a `<linearGradient>` in user units.
    fn element(&self, id: &str, w: f64, h: f64) -> String {
        let angle = match self.angle {
            Ok(angle) => angle,
            // Perpendicular to the diagonal between the neighbouring corners.
            Err((right, down)) => {
                let a = h.atan2(w);
                match (right, down) {
                    (true, false) => a,
                    (true, true) => std::f64::consts::PI - a,
                    (false, true) => std::f64::consts::PI + a,
                    (false, false) => -a,
                }
            }
        };
        let (dx, dy) = (angle.sin(), -angle.cos());
        let length = (w * dx).abs() + (h * dy).abs();
        let (cx, cy) = (w / 2.0, h / 2.0);
        let (x1, y1) = (cx - dx * length / 2.0, cy - dy * length / 2.0);
        let (x2, y2) = (cx + dx * length / 2.0, cy + dy * length / 2.0);
        // Positions: a missing first is 0, a missing last 1, others between their neighbours;
        // none before the one before it.
        let n = self.stops.len();
        let mut offsets: Vec<Option<f64>> = self
            .stops
            .iter()
            .map(|(position, _)| {
                position.as_ref().map(|p| match p {
                    StopPosition::Fraction(f) => *f,
                    StopPosition::Length(l) if length > 0.0 => l / length,
                    StopPosition::Length(_) => 0.0,
                })
            })
            .collect();
        if offsets.first().is_some_and(Option::is_none) {
            offsets[0] = Some(0.0);
        }
        if offsets.last().is_some_and(Option::is_none) {
            offsets[n - 1] = Some(1.0);
        }
        let mut max = f64::NEG_INFINITY;
        for offset in offsets.iter_mut().flatten() {
            max = max.max(*offset);
            *offset = max;
        }
        let mut i = 0;
        while i < n {
            if offsets[i].is_some() {
                i += 1;
                continue;
            }
            let start = i - 1;
            let end = (i..n).find(|&j| offsets[j].is_some()).unwrap_or(n - 1);
            let (a, b) = (offsets[start].unwrap_or(0.0), offsets[end].unwrap_or(1.0));
            for j in i..end {
                offsets[j] = Some(a + (b - a) * (j - start) as f64 / (end - start) as f64);
            }
            i = end;
        }
        let mut out = format!(
            "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
            num(x1),
            num(y1),
            num(x2),
            num(y2)
        );
        for ((_, [r, g, b, a]), offset) in self.stops.iter().zip(offsets) {
            let _ = write!(
                out,
                "<stop offset=\"{}\" stop-color=\"rgb({r},{g},{b})\" stop-opacity=\"{}\" />",
                num(offset.unwrap_or(0.0)),
                num(f64::from(*a) / 255.0)
            );
        }
        out.push_str("</linearGradient>");
        out
    }
}

/// A computed `linear-gradient()` as a [`LinearGradient`]; `None` for any other gradient (radial,
/// conic, repeating) or one with interpolation hints.
fn linear_gradient(
    gradient: &style::values::computed::image::Gradient,
    current_color: &style::color::AbsoluteColor,
    zoom: f64,
) -> Option<LinearGradient> {
    use crate::util::ToColorColor as _;
    use style::values::computed::image::LineDirection;
    use style::values::generics::image::{GenericGradient, GradientFlags, GradientItem};
    use style::values::specified::position::{HorizontalPositionKeyword as H, VerticalPositionKeyword as V};
    let GenericGradient::Linear { direction, items, flags, .. } = gradient else {
        return None;
    };
    if flags.contains(GradientFlags::REPEATING) {
        return None;
    }
    let angle = match direction {
        LineDirection::Angle(angle) => Ok(f64::from(angle.radians())),
        LineDirection::Horizontal(H::Right) => Ok(std::f64::consts::FRAC_PI_2),
        LineDirection::Horizontal(H::Left) => Ok(-std::f64::consts::FRAC_PI_2),
        LineDirection::Vertical(V::Top) => Ok(0.0),
        LineDirection::Vertical(V::Bottom) => Ok(std::f64::consts::PI),
        LineDirection::Corner(h, v) => Err((*h == H::Right, *v == V::Bottom)),
    };
    let rgba = |color: &style::values::computed::Color| {
        let c = color.resolve_to_absolute(current_color).as_color_color().to_rgba8();
        [c.r, c.g, c.b, c.a]
    };
    let mut stops = Vec::with_capacity(items.len());
    for item in items.iter() {
        match item {
            GradientItem::SimpleColorStop(color) => stops.push((None, rgba(color))),
            GradientItem::ComplexColorStop { color, position } => {
                let position = match position.to_percentage() {
                    Some(p) => StopPosition::Fraction(f64::from(p.0)),
                    None => StopPosition::Length(f64::from(position.to_length()?.px()) / zoom),
                };
                stops.push((Some(position), rgba(color)));
            }
            GradientItem::InterpolationHint(_) => return None,
        }
    }
    (stops.len() >= 2).then_some(LinearGradient { angle, stops })
}

/// `text` escaped for an attribute value.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    encode_quoted_attribute_to_string(text, &mut out);
    out
}

/// `bytes` in base64 (for a `data:` URL).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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
    use super::{MAX_AXIS_TILES, axis_tiles, local_urls, num};
    use style::values::specified::background::BackgroundRepeatKeyword as Repeat;

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

    /// A mask tile shorter than the float spacing at its position (`1000 + 1e-14 == 1000`) is
    /// too many tiles, not a loop that never moves on.
    #[test]
    fn tiles_below_the_float_spacing_are_too_many() {
        for repeat in [Repeat::Repeat, Repeat::Round, Repeat::Space] {
            assert_eq!(axis_tiles(1000.0, 10.0, 1000.0, 1010.0, 1e-14, 0.0, repeat), None);
            assert_eq!(axis_tiles(10.0, 10.0, 10.0, 20.0, 1e-16, 0.0, repeat), None);
        }
        let tiles = axis_tiles(0.0, 100.0, -5.0, 100.0, 10.0, 0.0, Repeat::Repeat).unwrap();
        assert_eq!(tiles, (-1..10).map(|k| f64::from(k) * 10.0).collect::<Vec<_>>());
        let many = axis_tiles(0.0, 1.0, 0.0, MAX_AXIS_TILES as f64 + 1.0, 1.0, 0.0, Repeat::Repeat);
        assert_eq!(many, None);
    }
}
