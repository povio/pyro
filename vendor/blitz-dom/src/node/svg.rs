//! SVG image data and CSS intrinsic sizing for SVG.

use std::sync::Arc;

use usvg::roxmltree;

/// Dimensions declared on the root `<svg>` element, before any resolution.
///
/// Unlike [`usvg::Tree::size`], which always produces a concrete size, this
/// preserves what the SVG actually declared: absent attributes are `None` and
/// percentage lengths are kept unresolved.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvgIntrinsicDimensions {
    /// The root `width` attribute, if declared. Percentages are unresolved.
    pub width: Option<svgtypes::Length>,
    /// The root `height` attribute, if declared. Percentages are unresolved.
    pub height: Option<svgtypes::Length>,
    /// The root `viewBox`, if declared and valid.
    pub view_box: Option<usvg::NonZeroRect>,
    /// The root `preserveAspectRatio` (default `xMidYMid meet`).
    pub preserve_aspect_ratio: svgtypes::AspectRatio,
    /// Whether the root declared a `viewBox` with a zero width or height,
    /// which disables rendering of the element per the SVG spec.
    pub degenerate_view_box: bool,
    /// pyro: the root's `background-color` (attribute or `style` declaration). usvg paints it
    /// over the `viewBox` only; a browser paints the root's background over the whole viewport
    /// (the canvas), so an image drawn letterboxed shows it in the bands too.
    pub root_background: Option<svgtypes::Color>,
}

impl SvgIntrinsicDimensions {
    /// Extract the `width`/`height`/`viewBox` attributes declared on the root
    /// element of an already-parsed SVG document, with absent attributes as
    /// `None` and percentages unresolved. [`usvg::Tree`] does not preserve
    /// these (it always resolves to a concrete size), so they are read from
    /// the XML document here.
    pub fn from_xmltree(doc: &roxmltree::Document) -> Self {
        let root = doc.root_element();

        let parse_length = |name: &str| -> Option<svgtypes::Length> {
            root.attribute(name)?.parse::<svgtypes::Length>().ok()
        };
        // Parsed manually rather than via `svgtypes::ViewBox`, which rejects
        // zero sizes: a zero `viewBox` width/height is distinguished from an
        // invalid `viewBox` as it disables rendering of the element.
        let view_box_dims = root.attribute("viewBox").and_then(|s| {
            let mut numbers = svgtypes::NumberListParser::from(s);
            let x = numbers.next()?.ok()? as f32;
            let y = numbers.next()?.ok()? as f32;
            let w = numbers.next()?.ok()? as f32;
            let h = numbers.next()?.ok()? as f32;
            (numbers.next().is_none() && w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0)
                .then_some((x, y, w, h))
        });
        let view_box =
            view_box_dims.and_then(|(x, y, w, h)| usvg::NonZeroRect::from_xywh(x, y, w, h));
        let degenerate_view_box = view_box_dims.is_some_and(|(_, _, w, h)| w == 0.0 || h == 0.0);
        let preserve_aspect_ratio = root
            .attribute("preserveAspectRatio")
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        let root_background = root
            .attribute("style")
            .and_then(|style| {
                style.split(';').rev().find_map(|declaration| {
                    let (name, value) = declaration.split_once(':')?;
                    // pyro: or the `background` shorthand when it is a colour alone.
                    let name = name.trim();
                    let value = value.trim();
                    match name {
                        "background-color" => Some(value.parse::<svgtypes::Color>().ok()),
                        "background" => Some(value.parse::<svgtypes::Color>().ok()),
                        _ => None,
                    }
                })
            })
            .flatten()
            .or_else(|| root.attribute("background-color").and_then(|value| value.parse::<svgtypes::Color>().ok()));

        Self {
            root_background,
            width: parse_length("width"),
            height: parse_length("height"),
            view_box,
            preserve_aspect_ratio,
            degenerate_view_box,
        }
    }
}

/// A parsed SVG image.
///
/// usvg always resolves the root `<svg>` to a concrete [`usvg::Tree::size`],
/// falling back to the `viewBox` size when `width`/`height` are absent or given
/// as percentages. For CSS sizing purposes, however, such an SVG has *no*
/// intrinsic width/height (only an intrinsic aspect ratio). The accessors on
/// this type resolve the CSS intrinsic dimensions from the declared root
/// attributes, which are captured at parse time.
#[derive(Debug, Clone)]
pub struct SvgImageData {
    /// The parsed SVG tree.
    pub tree: Arc<usvg::Tree>,
    /// The dimensions declared on the root `<svg>` element.
    pub intrinsic_dimensions: SvgIntrinsicDimensions,
    /// pyro: the source of an inline `<svg>` without a `viewBox`, whose content
    /// is laid out in its CSS box (see [`Self::sized`]).
    pub inline_source: Option<Arc<str>>,
    /// pyro: the document an SVG image was loaded from, which an inline `<svg>` with CSS
    /// `mask-image` layers of it embeds (`svg_css`).
    pub source: Option<Arc<[u8]>>,
    /// pyro: the source of an SVG image with a `<foreignObject>`, which is rendered as a document
    /// of its own (`BaseDocument::svg_image_documents`) to draw its HTML content.
    pub foreign_source: Option<Arc<str>>,
    /// pyro: an inline `<svg>` whose source depends on its laid-out size (a `view-box` reference box of a root
    /// sized in percentages or not at all): painted from a source serialized again at that size
    /// (`BaseDocument::inline_svg_at_viewport`).
    pub viewport_dependent: bool,
}

impl SvgImageData {
    /// Parse an SVG image from raw data, capturing both the rendered
    /// [`usvg::Tree`] and the declared root dimensions from a single XML
    /// parse.
    ///
    /// Like [`usvg::Tree::from_data`], gzip-compressed data (SVGZ) is
    /// decompressed first.
    pub fn from_data(data: &[u8], options: &usvg::Options) -> Result<Self, usvg::Error> {
        // Gzip magic bytes, matching the SVGZ detection in `usvg::Tree::from_data`.
        let decompressed;
        let data = if data.starts_with(&[0x1f, 0x8b]) {
            decompressed = usvg::decompress_svgz(data)?;
            decompressed.as_slice()
        } else {
            data
        };

        let text = std::str::from_utf8(data).map_err(|_| usvg::Error::NotAnUtf8Str)?;
        let xml_options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(text, xml_options)
            .map_err(usvg::Error::ParsingFailed)?;
        let intrinsic_dimensions = SvgIntrinsicDimensions::from_xmltree(&doc);
        let tree = match usvg::Tree::from_xmltree(&doc, options) {
            Ok(tree) => tree,
            // pyro: a root `width` or `height` of zero is a natural size of zero, not an invalid image
            // (browsers lay it out at that size, its `viewBox` ratio kept, and draw its content in the box
            // they give it): the content is parsed as of an SVG without a size (WPT
            // `grid-item-aspect-ratio-stretch-3`, `grid-item-no-aspect-ratio-stretch-4..6`).
            Err(usvg::Error::InvalidSize) => {
                let zero = |length: Option<svgtypes::Length>| length.is_some_and(|length| length.number == 0.0);
                if !zero(intrinsic_dimensions.width) && !zero(intrinsic_dimensions.height) {
                    return Err(usvg::Error::InvalidSize);
                }
                let without =
                    root_attrs_replaced(text, &[], &["width", "height"]).ok_or(usvg::Error::InvalidSize)?;
                let xml_options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
                let doc = roxmltree::Document::parse_with_options(&without, xml_options)
                    .map_err(usvg::Error::ParsingFailed)?;
                usvg::Tree::from_xmltree(&doc, options)?
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            tree: Arc::new(tree),
            intrinsic_dimensions,
            inline_source: None,
            source: None,
            foreign_source: None,
            viewport_dependent: false,
        })
    }

    /// pyro: the image drawn for an element whose used colour scheme is dark (`dark`), when the
    /// image uses `light-dark()` and its own `color-scheme` allows dark: parsed again with each
    /// `light-dark()` its dark colour (WPT css-borders `border-image-color-scheme`). `None`
    /// otherwise: the image as loaded is the light one.
    pub fn for_color_scheme(&self, dark: bool) -> Option<SvgImageData> {
        let source = self.source.as_deref().filter(|_| dark)?;
        crate::svg_refs::find_bytes(source, b"light-dark(")?;
        if !crate::svg_refs::allows_dark_scheme(source) {
            return None;
        }
        let dark_source = crate::svg_refs::resolve_light_dark(source, true)?;
        let mut svg = crate::util::parse_svg_image(&dark_source).ok()?;
        svg.source = self.source.clone();
        Some(svg)
    }

    /// pyro: an inline `<svg>` without a `viewBox`, parsed again for a CSS box
    /// `width` x `height` (CSS px): its viewport is the box, so user units are
    /// CSS px and percentages (a `<use>` of a `<symbol>` sized `100%` by default)
    /// resolve against the box. Parsed alone, usvg resolves them against
    /// 100x100 and then shrinks the tree to its content's bounds, which drew
    /// sprite icons too large. `None` for other images, or an empty box.
    pub fn sized(&self, width: f32, height: f32) -> Option<SvgImageData> {
        let source = self.inline_source.as_deref()?;
        if !(width > 0.0 && height > 0.0) {
            return None;
        }
        let source = with_root_size(source, width, height)?;
        crate::util::parse_svg_image(source.as_bytes()).ok()
    }

    /// The intrinsic width in CSS px, present only when the root `<svg>`
    /// declared an absolute (non-percentage) `width`.
    pub fn intrinsic_width(&self) -> Option<f32> {
        use svgtypes::LengthUnit;
        let declared = self
            .intrinsic_dimensions
            .width
            .is_some_and(|len| len.unit != LengthUnit::Percent);
        // pyro: an absolute length is the natural width as declared; usvg's resolved size is not
        // it when the height is missing (it took the content's bounds: 46px for `width="20px"`
        // with a circle sized in percentages; WPT `grid-item-no-aspect-ratio-stretch-*`).
        if let Some(px) = self.intrinsic_dimensions.width.and_then(absolute_length_px) {
            return Some(px);
        }
        declared.then(|| self.tree.size().width())
    }

    /// The intrinsic height in CSS px, present only when the root `<svg>`
    /// declared an absolute (non-percentage) `height`.
    pub fn intrinsic_height(&self) -> Option<f32> {
        use svgtypes::LengthUnit;
        let declared = self
            .intrinsic_dimensions
            .height
            .is_some_and(|len| len.unit != LengthUnit::Percent);
        // pyro: as for the width
        if let Some(px) = self.intrinsic_dimensions.height.and_then(absolute_length_px) {
            return Some(px);
        }
        declared.then(|| self.tree.size().height())
    }

    /// The aspect ratio of the root `<svg>`'s `viewBox`, if it declares one.
    pub fn viewbox_aspect_ratio(&self) -> Option<f32> {
        self.intrinsic_dimensions
            .view_box
            .map(|view_box| view_box.width() / view_box.height())
    }

    /// The transform from the rendered tree (in [`usvg::Tree::size`] units) to a
    /// viewport of the given size, applying the root `viewBox` and
    /// `preserveAspectRatio`. Without a `viewBox` the tree is stretched to fill it.
    pub fn viewport_transform(&self, width: f32, height: f32) -> usvg::Transform {
        let size = self.tree.size();
        let stretch = usvg::Transform::from_scale(width / size.width(), height / size.height());
        let Some(view_box) = self.intrinsic_dimensions.view_box else {
            return stretch;
        };
        let aspect = self.intrinsic_dimensions.preserve_aspect_ratio;
        // usvg has already mapped the `viewBox` onto `Tree::size`, so undo that first.
        let view_box_to_tree = view_box_transform(view_box, aspect, size.width(), size.height());
        match view_box_to_tree.invert() {
            Some(tree_to_view_box) => {
                view_box_transform(view_box, aspect, width, height).pre_concat(tree_to_view_box)
            }
            None => stretch,
        }
    }

    /// The root `width` attribute resolved against a containing block width:
    /// percentages resolve against the containing block (`None` if it is
    /// indefinite) and an absent attribute is `None`.
    ///
    /// This is only appropriate for an inline `<svg>` element, where the
    /// attributes behave as presentation attributes. SVG used as an image
    /// (e.g. `<img src>` or a background) must use [`Self::intrinsic_width`],
    /// as its intrinsic dimensions are context-free per CSS.
    pub fn resolved_width(&self, container_width: Option<f32>) -> Option<f32> {
        use svgtypes::LengthUnit;
        match self.intrinsic_dimensions.width {
            Some(len) if len.unit != LengthUnit::Percent => Some(self.tree.size().width()),
            Some(len) => container_width.map(|cw| cw * (len.number as f32) / 100.0),
            None => None,
        }
    }

    /// The root `height` attribute resolved against a containing block height.
    /// See [`Self::resolved_width`].
    pub fn resolved_height(&self, container_height: Option<f32>) -> Option<f32> {
        use svgtypes::LengthUnit;
        match self.intrinsic_dimensions.height {
            Some(len) if len.unit != LengthUnit::Percent => Some(self.tree.size().height()),
            Some(len) => container_height.map(|ch| ch * (len.number as f32) / 100.0),
            None => None,
        }
    }

    /// The intrinsic aspect ratio of the SVG: the ratio of its declared
    /// `width`/`height` when both are absolute lengths, otherwise the
    /// `viewBox` ratio, otherwise the ratio of the resolved
    /// [`usvg::Tree::size`] (which is always non-zero).
    pub fn aspect_ratio(&self) -> f32 {
        match (self.intrinsic_width(), self.intrinsic_height()) {
            (Some(w), Some(h)) => w / h,
            _ => self.viewbox_aspect_ratio().unwrap_or_else(|| {
                let size = self.tree.size();
                size.width() / size.height()
            }),
        }
    }

    /// The intrinsic dimensions of the SVG resolved per CSS replaced element
    /// sizing: a missing dimension is computed from the declared one and the
    /// intrinsic aspect ratio; if neither is declared, the resolved
    /// [`usvg::Tree::size`] is used as a fallback.
    pub fn intrinsic_size(&self) -> (f32, f32) {
        let aspect_ratio = self.aspect_ratio();
        match (self.intrinsic_width(), self.intrinsic_height()) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, w / aspect_ratio),
            (None, Some(h)) => (h * aspect_ratio, h),
            (None, None) => {
                // No intrinsic dimensions. If there is an intrinsic aspect ratio, apply
                // the CSS default sizing algorithm: contain within the default object
                // size of 300x150. Otherwise fall back to the resolved tree size.
                if self.viewbox_aspect_ratio().is_some() {
                    let scale = (300.0 / aspect_ratio).min(150.0);
                    (scale * aspect_ratio, scale)
                } else {
                    let size = self.tree.size();
                    (size.width(), size.height())
                }
            }
        }
    }
}

impl crate::BaseDocument {
    /// pyro: an element of an inline `<svg>` that the page's CSS references by `url(#id)` from a
    /// box (`clip-path: url(#c)` naming a `<clipPath>`, `mask-image: url(#m)` a `<mask>`), as an
    /// SVG document of its own for usvg: the outermost `<svg>` around the element, serialized as
    /// inline SVG is for usvg, in `<defs>` (so none of it is drawn and all of it can be
    /// referenced), then `content`, which references the element, in a viewport `width` x
    /// `height` (user units: percentages in the element's content resolve against it). Parsed as
    /// inline SVG is. `None` when the element is in no `<svg>`, or the document does not parse.
    pub fn svg_reference_document(
        &self,
        element: blitz_traits::node_id::NodeId,
        content: &str,
        width: f32,
        height: f32,
    ) -> Option<SvgImageData> {
        use markup5ever::{local_name, ns};
        let mut root = None;
        let mut next = Some(element);
        while let Some(id) = next {
            let node = self.get_node(id)?;
            let Some(data) = node.element_data() else {
                break;
            };
            if data.name.ns != ns!(svg) {
                break;
            }
            if data.name.local == local_name!("svg") {
                root = Some(node);
            }
            next = node.parent;
        }
        let root = root?;
        let source = self.with_svg_sprites(self.with_svg_images(root.svg_source(), root.id));
        // pyro: `content` in a group anyrender_svg knows a box's reference by (its masks of
        // opaque shapes are drawn as the shapes).
        let document = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
             width=\"{width}\" height=\"{height}\"><defs>{source}</defs><g id=\"__pyro_box_reference\">{content}</g></svg>"
        );
        crate::util::parse_svg_image(document.as_bytes()).ok()
    }
}

/// Map a `viewBox` onto a viewport of the given size per `preserveAspectRatio`.
fn view_box_transform(
    view_box: usvg::NonZeroRect,
    aspect: svgtypes::AspectRatio,
    width: f32,
    height: f32,
) -> usvg::Transform {
    use svgtypes::Align;

    let sx = width / view_box.width();
    let sy = height / view_box.height();
    let (sx, sy) = if aspect.align == Align::None {
        (sx, sy)
    } else if aspect.slice {
        (sx.max(sy), sx.max(sy))
    } else {
        (sx.min(sy), sx.min(sy))
    };
    let (align_x, align_y) = match aspect.align {
        Align::None | Align::XMinYMin => (0.0, 0.0),
        Align::XMidYMin => (0.5, 0.0),
        Align::XMaxYMin => (1.0, 0.0),
        Align::XMinYMid => (0.0, 0.5),
        Align::XMidYMid => (0.5, 0.5),
        Align::XMaxYMid => (1.0, 0.5),
        Align::XMinYMax => (0.0, 1.0),
        Align::XMidYMax => (0.5, 1.0),
        Align::XMaxYMax => (1.0, 1.0),
    };
    let tx = (width - view_box.width() * sx) * align_x - view_box.x() * sx;
    let ty = (height - view_box.height() * sy) * align_y - view_box.y() * sy;
    usvg::Transform::from_row(sx, 0.0, 0.0, sy, tx, ty)
}

impl SvgImageData {
    /// pyro: an SVG image as loaded from `url`, `source` its data, `self` that data parsed. A
    /// fragment naming a `<view>` element (`sprite.svg#icon`) shows the document through the
    /// view's `viewBox` and `preserveAspectRatio` (SVG 2 §5.8.3, as Chromium draws SVG sprites):
    /// the root takes them, so the image gets the view's aspect ratio. An image with no
    /// `viewBox` keeps its source: its content is laid out in the box it is
    /// drawn in (the concrete object size: a background's size, an `<img>`'s content box), as
    /// an inline `<svg>` without a `viewBox` is ([`Self::sized`]), so percentages (a `100%`
    /// rect, a `10%` stroke) resolve against that box rather than against usvg's 100x100.
    pub fn for_url(self, source: &[u8], url: &str) -> Self {
        let Ok(text) = std::str::from_utf8(source) else {
            return self;
        };
        let fragment = url
            .split_once('#')
            .map(|(_, fragment)| fragment)
            .filter(|fragment| !fragment.is_empty() && !fragment.starts_with("svgView("));
        // pyro: the element the fragment names is the document's `:target` (WPT css-masking
        // `mask-image-url-image-hash`): usvg's style sheets know no `:target`, so it is written
        // as that id selector.
        let targeted = fragment.and_then(|fragment| with_target(text, fragment));
        let text = targeted.as_deref().unwrap_or(text);
        let viewed = fragment.and_then(|fragment| with_view(text, fragment)).or(targeted.clone());
        let (mut svg, text) = match viewed {
            Some(viewed) => match crate::util::parse_svg_image(viewed.as_bytes()) {
                Ok(svg) => (svg, viewed),
                Err(_) => (self, text.to_string()),
            },
            None => (self, text.to_string()),
        };
        // pyro: so does one with a zero `width` or `height` (no natural ratio), as Blink draws it:
        // the concrete object size is its viewport, unscaled (a `width="0" height="20"` circle of
        // `r="50%"` stretched to 350x250 is a circle; WPT css-grid
        // `grid-item-no-aspect-ratio-stretch-*`). One with natural dimensions is scaled.
        let (width, height) = (svg.intrinsic_width(), svg.intrinsic_height());
        let no_natural_ratio = (width.is_none() && height.is_none()) || width == Some(0.0) || height == Some(0.0);
        if svg.intrinsic_dimensions.view_box.is_none() && no_natural_ratio && !svg.intrinsic_dimensions.degenerate_view_box {
            svg.inline_source = Some(text.into());
        }
        svg
    }
}

/// pyro: `source` with its style sheets' `:target` written as `#fragment`, when `fragment` is a
/// plain identifier; `None` when nothing changes.
fn with_target(source: &str, fragment: &str) -> Option<String> {
    let plain = fragment.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !fragment.starts_with(|c: char| c.is_ascii_digit());
    (plain && source.contains(":target")).then(|| source.replace(":target", &format!("#{fragment}")))
}

/// pyro: `source` with its root showing the `<view>` whose id is `fragment`: the root takes the
/// view's `viewBox` and `preserveAspectRatio`. `None` without such a view (or without a
/// `viewBox` on it).
fn with_view(source: &str, fragment: &str) -> Option<String> {
    if !source.contains("<view") {
        return None;
    }
    let xml_options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(source, xml_options).ok()?;
    let view = doc
        .descendants()
        .find(|n| n.has_tag_name("view") && n.attribute("id") == Some(fragment))?;
    let mut attrs = vec![("viewBox", view.attribute("viewBox")?.to_string())];
    if let Some(aspect) = view.attribute("preserveAspectRatio") {
        attrs.push(("preserveAspectRatio", aspect.to_string()));
    }
    with_root_attrs(source, &attrs)
}

/// pyro: `source` (an SVG document whose root is `<svg`) with the root's
/// `width` and `height` attributes set to `width` x `height`.
fn with_root_size(source: &str, width: f32, height: f32) -> Option<String> {
    with_root_attrs(
        source,
        &[("width", width.to_string()), ("height", height.to_string())],
    )
}

/// pyro: `source` (an SVG document whose root is `<svg`) with the root's attributes named in
/// `attrs` set to their values there (first, in that order); its other attributes are kept.
fn with_root_attrs(source: &str, set: &[(&str, String)]) -> Option<String> {
    root_attrs_replaced(source, set, &[])
}

/// pyro: `source` with the root's attributes named in `set` set (first, in that order) and those named in
/// `drop` removed; its other attributes are kept.
fn root_attrs_replaced(source: &str, set: &[(&str, String)], drop: &[&str]) -> Option<String> {
    let start = source.find("<svg")?;
    // The end of the root's start tag: the first `>` outside an attribute value.
    let mut quote = None;
    let end = source[start..].char_indices().find_map(|(i, c)| {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(start + i),
            _ => {}
        }
        None
    })?;
    let tag = &source[start + 4..end];
    let mut attrs = String::with_capacity(tag.len() + 32);
    let mut rest = tag;
    // Drops the attributes being set, keeps every other attribute.
    while let Some(i) = rest.find(|c: char| c.is_ascii_whitespace()) {
        attrs.push_str(&rest[..i]);
        let after = rest[i..].trim_start();
        let name_end = after.find(|c: char| c == '=' || c.is_ascii_whitespace() || c == '/').unwrap_or(after.len());
        let name = &after[..name_end];
        let value_end = after[name_end..]
            .strip_prefix('=')
            .and_then(|v| {
                let q = v.chars().next().filter(|q| *q == '"' || *q == '\'')?;
                v[1..].find(q).map(|e| name_end + 1 + 1 + e + 1)
            })
            .unwrap_or(name_end);
        if set.iter().any(|(set_name, _)| *set_name == name) || drop.contains(&name) {
            rest = &after[value_end..];
        } else {
            attrs.push(' ');
            attrs.push_str(&after[..value_end]);
            rest = &after[value_end..];
        }
    }
    attrs.push_str(rest);
    let mut root = String::from("<svg");
    for (name, value) in set {
        let value = value.replace('&', "&amp;").replace('"', "&quot;");
        root.push_str(&format!(" {name}=\"{value}\""));
    }
    Some(format!(
        "{}{root}{attrs}{}",
        &source[..start],
        &source[end..]
    ))
}

#[cfg(test)]
mod sized_tests {
    use super::with_root_size;

    #[test]
    fn the_root_gets_the_box_size_and_keeps_its_other_attributes() {
        let source = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" stroke-width="1.8" height='24' class="a>b"><rect width="50%"/></svg>"#;
        assert_eq!(
            with_root_size(source, 16.0, 20.5).unwrap(),
            r#"<svg width="16" height="20.5" xmlns="http://www.w3.org/2000/svg" stroke-width="1.8" class="a>b"><rect width="50%"/></svg>"#
        );
    }
}

/// pyro: one `url()` of an element that may name an element of another SVG document
/// ([`reference_candidates`]).
pub(crate) enum ReferenceCandidate<'a> {
    /// A computed `clip-path` or `filter` URL.
    Url(&'a style::servo_arc::Arc<url::Url>),
    /// The URL written in an SVG element's `filter` or `mask` attribute (not a fragment alone).
    Attr(&'a str),
}

/// pyro: the `url()`s of an element's computed `clip-path` and `filter`, then of an SVG element's
/// `clip-path`, `filter` and `mask` attributes when they name another document: what [`super::ElementData::reference_images`] has an entry for, in order.
pub(crate) fn reference_candidates<'a>(
    element: &'a super::ElementData,
    style: &'a style::properties::ComputedValues,
) -> Vec<ReferenceCandidate<'a>> {
    use style::url::ComputedUrl;
    use style::values::computed::basic_shape::ClipPath;
    use style::values::generics::effects::GenericFilter;
    let mut out = Vec::new();
    let svg = element.name.ns == markup5ever::ns!(svg);
    // An SVG element's `clip-path` attribute is a presentation attribute, whose URL the style
    // engine resolves without the document's base: it is taken as written, below.
    let clip_path_attr = svg
        && element.attr(markup5ever::local_name!("clip-path")).is_some_and(|value| value.trim_start().starts_with("url("));
    if let (ClipPath::Url(ComputedUrl::Valid(url)), false) = (style.get_clip_path(), clip_path_attr) {
        out.push(ReferenceCandidate::Url(url));
    }
    for filter in style.get_effects().filter.0.iter() {
        if let GenericFilter::Url(ComputedUrl::Valid(url)) = filter {
            out.push(ReferenceCandidate::Url(url));
        }
    }
    if svg {
        for attr in element.attrs().iter() {
            let name: &str = attr.name.local.as_ref();
            if attr.name.ns != markup5ever::ns!() || (name != "filter" && name != "mask" && name != "clip-path") {
                continue;
            }
            if let Some(url) = attr_reference_url(&attr.value) {
                out.push(ReferenceCandidate::Attr(url));
            }
        }
    }
    out
}

/// The URL of an attribute value that is one `url()` naming an element of another document.
fn attr_reference_url(value: &str) -> Option<&str> {
    let inner = value.trim().strip_prefix("url(")?.strip_suffix(')')?;
    let inner = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    (!inner.starts_with('#') && inner.contains('#')).then_some(inner)
}

/// pyro: whether an element's stored reference documents are not those its style and
/// attributes name now (they are fetched again then, `flush_svg_references_from_style`).
pub(crate) fn references_out_of_sync(
    element: &super::ElementData,
    style: &style::properties::ComputedValues,
) -> bool {
    let candidates = reference_candidates(element, style);
    if candidates.len() != element.reference_images.len() {
        return true;
    }
    std::iter::zip(candidates, element.reference_images.iter()).any(|(candidate, stored)| {
        match (candidate, stored) {
            (ReferenceCandidate::Url(url), Some(data)) => **url != *data.url,
            (ReferenceCandidate::Url(_), None) => true,
            // Resolved against the document's base: the stored URL ends with what is written.
            (ReferenceCandidate::Attr(raw), Some(data)) => {
                let stored = data.url.as_str();
                stored != raw && !stored.ends_with(raw.trim_start_matches("./"))
            }
            // An attribute URL that does not resolve stays without an entry.
            (ReferenceCandidate::Attr(_), None) => false,
        }
    })
}

/// pyro: the element of id `id` in the SVG document `source`: its local name and its `attr`
/// attribute. For a `<clipPath>`, `<mask>` or `<filter>` of another document that a `url()`
/// names (`clip-path: url(shapes.svg#clip)`).
pub fn svg_source_element(source: &[u8], id: &str, attr: &str) -> Option<(String, Option<String>)> {
    let text = std::str::from_utf8(source).ok()?;
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, options).ok()?;
    let node = doc.descendants().find(|node| node.is_element() && node.attribute("id") == Some(id))?;
    Some((node.tag_name().name().to_string(), node.attribute(attr).map(str::to_string)))
}

/// pyro: the SVG document `source` as markup that can be a child of another SVG document's
/// `<defs>`: from its root's start tag on (without the XML declaration and document type).
/// Its root fills its viewport: a document of resources often has a root of no size
/// (`<svg width="0" height="0">`), which usvg does not parse, and whose content it would drop.
pub(crate) fn embeddable_svg(source: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(source).ok()?;
    let start = text.find("<svg")?;
    with_root_attrs(text[start..].trim_end(), &[("width", "100%".to_string()), ("height", "100%".to_string())])
}

impl SvgImageData {
    /// pyro: [`crate::BaseDocument::svg_reference_document`] for an element of another SVG
    /// document, `source` (`clip-path: url(shapes.svg#clip)` on a box): that document in
    /// `<defs>`, then `content`, which references the element by `url(#id)`.
    pub fn reference_document_from_source(source: &[u8], content: &str, width: f32, height: f32) -> Option<SvgImageData> {
        let source = embeddable_svg(source)?;
        let source = source.as_str();
        let document = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
             width=\"{width}\" height=\"{height}\"><defs>{source}</defs><g id=\"__pyro_box_reference\">{content}</g></svg>"
        );
        crate::util::parse_svg_image(document.as_bytes()).ok()
    }
}

/// pyro: an SVG length in an absolute unit (user units are CSS px) in CSS px; `None` for percentages and
/// font-relative units.
fn absolute_length_px(length: svgtypes::Length) -> Option<f32> {
    use svgtypes::LengthUnit;
    let scale = match length.unit {
        LengthUnit::None | LengthUnit::Px => 1.0,
        LengthUnit::In => 96.0,
        LengthUnit::Cm => 96.0 / 2.54,
        LengthUnit::Mm => 96.0 / 25.4,
        LengthUnit::Pt => 4.0 / 3.0,
        LengthUnit::Pc => 16.0,
        LengthUnit::Em | LengthUnit::Ex | LengthUnit::Percent => return None,
    };
    let px = (length.number * scale) as f32;
    (px.is_finite() && px >= 0.0).then_some(px)
}
