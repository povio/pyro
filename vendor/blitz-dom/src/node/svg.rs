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

        Self {
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
        let tree = usvg::Tree::from_xmltree(&doc, options)?;
        Ok(Self {
            tree: Arc::new(tree),
            intrinsic_dimensions: SvgIntrinsicDimensions::from_xmltree(&doc),
            inline_source: None,
        })
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
        let source = self.with_svg_sprites(root?.svg_source());
        let document = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
             width=\"{width}\" height=\"{height}\"><defs>{source}</defs>{content}</svg>"
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
    /// `viewBox`, `width` or `height` keeps its source: its content is laid out in the box it is
    /// drawn in (the concrete object size: a background's size, an `<img>`'s content box), as
    /// an inline `<svg>` without a `viewBox` is ([`Self::sized`]), so percentages (a `100%`
    /// rect, a `10%` stroke) resolve against that box rather than against usvg's 100x100.
    pub fn for_url(self, source: &[u8], url: &str) -> Self {
        let Ok(text) = std::str::from_utf8(source) else {
            return self;
        };
        let viewed = url
            .split_once('#')
            .map(|(_, fragment)| fragment)
            .filter(|fragment| !fragment.is_empty() && !fragment.starts_with("svgView("))
            .and_then(|fragment| with_view(text, fragment));
        let (mut svg, text) = match viewed {
            Some(viewed) => match crate::util::parse_svg_image(viewed.as_bytes()) {
                Ok(svg) => (svg, viewed),
                Err(_) => (self, text.to_string()),
            },
            None => (self, text.to_string()),
        };
        if svg.intrinsic_dimensions.view_box.is_none()
            && svg.intrinsic_width().is_none()
            && svg.intrinsic_height().is_none()
            && !svg.intrinsic_dimensions.degenerate_view_box
        {
            svg.inline_source = Some(text.into());
        }
        svg
    }
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
        if set.iter().any(|(set_name, _)| *set_name == name) {
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
