//! HTML "select an image source" for an `<img>`: the matching `<source>` of
//! a `<picture>` parent, else the image's own `srcset` (with `sizes` giving
//! the source size of width descriptors), else its `src`.

use crate::dom::Dom;
use crate::media::Viewport;
use crate::node::{NodeId, NIL};

/// One image candidate of a `srcset`: its URL and pixel density.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub density: f64,
}

/// HTML "parse a srcset attribute". A width descriptor becomes a density
/// against `source_width` (CSS px, the `sizes` source size). Candidates whose
/// descriptors are invalid (HTML's descriptor parser: a `w` integer above 0,
/// an `x` floating-point number of 0 or more, an `h` integer above 0 only
/// with a `w`, each at most once, numbers without `+`) are dropped; a
/// parenthesized run inside the descriptors does not end the candidate at
/// its commas.
pub fn parse_srcset(input: &str, source_width: f64) -> Vec<Candidate> {
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0c' | '\r');
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        while pos < chars.len() && (is_space(chars[pos]) || chars[pos] == ',') {
            pos += 1;
        }
        if pos >= chars.len() {
            return out;
        }
        let start = pos;
        while pos < chars.len() && !is_space(chars[pos]) {
            pos += 1;
        }
        let mut url: String = chars[start..pos].iter().collect();
        let mut descriptors = Vec::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',').to_owned();
        } else {
            // The descriptor tokenizer.
            enum State {
                Descriptor,
                InParens,
                After,
            }
            let mut state = State::Descriptor;
            let mut current = String::new();
            while pos < chars.len() && is_space(chars[pos]) {
                pos += 1;
            }
            loop {
                let c = chars.get(pos).copied();
                match state {
                    State::Descriptor => match c {
                        Some(c) if is_space(c) => {
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            state = State::After;
                        }
                        Some(',') => {
                            pos += 1;
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            break;
                        }
                        Some('(') => {
                            current.push('(');
                            state = State::InParens;
                        }
                        None => {
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            break;
                        }
                        Some(c) => current.push(c),
                    },
                    State::InParens => match c {
                        Some(')') => {
                            current.push(')');
                            state = State::Descriptor;
                        }
                        None => {
                            if !current.is_empty() {
                                descriptors.push(std::mem::take(&mut current));
                            }
                            break;
                        }
                        Some(c) => current.push(c),
                    },
                    State::After => match c {
                        Some(c) if is_space(c) => {}
                        None => break,
                        Some(_) => {
                            state = State::Descriptor;
                            continue;
                        }
                    },
                }
                pos += 1;
            }
        }
        if url.is_empty() {
            continue;
        }
        let Some((density, width)) = parse_descriptors(&descriptors) else { continue };
        let density = match (density, width) {
            (Some(d), _) => d,
            // A zero source size makes the density infinite (natural size 0).
            (None, Some(w)) if source_width >= 0.0 => w / source_width,
            _ => 1.0,
        };
        out.push(Candidate { url, density });
    }
}

/// HTML's srcset descriptor parser: the `x` density and `w` width, or `None`
/// on an error. Integers of any size are valid (read as f64).
fn parse_descriptors(descriptors: &[String]) -> Option<(Option<f64>, Option<f64>)> {
    let (mut density, mut width, mut height) = (None, None::<f64>, None::<f64>);
    for descriptor in descriptors {
        let (number, unit) = descriptor.split_at(descriptor.len() - descriptor.chars().last()?.len_utf8());
        match unit {
            "w" if valid_integer(number) => {
                if width.is_some() || density.is_some() {
                    return None;
                }
                width = Some(number.parse::<f64>().ok().filter(|w| *w > 0.0)?);
            }
            "x" if valid_float(number) => {
                if width.is_some() || density.is_some() || height.is_some() {
                    return None;
                }
                density = Some(number.parse::<f64>().ok().filter(|d| d.is_finite() && *d >= 0.0)?);
            }
            "h" if valid_integer(number) => {
                if height.is_some() || density.is_some() {
                    return None;
                }
                height = Some(number.parse::<f64>().ok().filter(|h| *h > 0.0)?);
            }
            _ => return None,
        }
    }
    if height.is_some() && width.is_none() {
        return None;
    }
    Some((density, width))
}

/// An HTML valid non-negative integer: ASCII digits only.
fn valid_integer(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// An HTML valid floating-point number: `-`? digits (`.` digits)? or `-`?
/// `.` digits, then an optional exponent; no `+`, no `Infinity`.
fn valid_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    let whole = digits(&mut i);
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    } else if !whole {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// HTML "select an image source": the candidate for a device pixel ratio,
/// the lowest density that is at least `dpr`, else the highest; of
/// candidates with the same density only the first counts.
pub fn pick_candidate(candidates: &[Candidate], dpr: f64) -> Option<&Candidate> {
    let mut enough: Option<&Candidate> = None;
    let mut highest: Option<&Candidate> = None;
    for c in candidates {
        if c.density >= dpr && enough.is_none_or(|e| c.density < e.density) {
            enough = Some(c);
        }
        if highest.is_none_or(|h| c.density > h.density) {
            highest = Some(c);
        }
    }
    enough.or(highest)
}

impl Dom {
    /// The URL (as written) the `<picture>` parent of `img` selects for it:
    /// the first preceding `<source>` sibling with a `srcset`, whose `media`
    /// matches `viewport` and whose `type`, when given, is an image type.
    /// `None` when `img` is not a picture's child or no source applies.
    pub fn picture_source(&self, img: NodeId, viewport: &Viewport) -> Option<String> {
        self.picture_candidate(img, viewport).map(|c| c.url)
    }

    /// [`Dom::picture_source`] with the candidate's pixel density (the image's natural size is
    /// its pixel size divided by it).
    pub fn picture_candidate(&self, img: NodeId, viewport: &Viewport) -> Option<Candidate> {
        self.picture_candidate_auto(img, viewport, None)
    }

    /// [`Dom::picture_candidate`] where `sizes="auto"` of a lazy-loaded `<img>` (HTML: the first
    /// entry `auto`) is `auto_width`, the image's concrete width in CSS px; `None` leaves the
    /// `auto` entry out, as for an image that is not lazy.
    pub fn picture_candidate_auto(&self, img: NodeId, viewport: &Viewport, auto_width: Option<f64>) -> Option<Candidate> {
        let i = self.check(img).ok()?;
        let lazy = self.n(i).element().and_then(|e| e.attr_local(&markup5ever::local_name!("loading"))).is_some_and(|l| l.eq_ignore_ascii_case("lazy"));
        let auto = auto_width.filter(|_| lazy);
        let source_size = |sizes: Option<&str>, viewport: &Viewport| source_size_auto(sizes, viewport, auto);
        let parent = self.n(i).parent;
        let html =
            |idx: u32, local: &str| self.n(idx).element().is_some_and(|e| e.is_html() && &*e.name.local == local);
        if parent == NIL || !html(parent, "picture") {
            return self.own_srcset_source(i, viewport, auto);
        }
        let mut child = self.n(parent).first_child;
        while child != NIL && child != i {
            if html(child, "source") {
                let e = self.n(child).element().expect("source element");
                // The media query list through the style engine, as `sizes` conditions are: an
                // invalid query, or one with an unknown condition, matches nothing, also negated
                // (`not all and (!)`).
                let media_ok = e.attr_local(&markup5ever::local_name!("media")).is_none_or(|m| {
                    m.trim().is_empty() || crate::cssom::MediaQueryList::parse(m).matches(&sizes_device(viewport))
                });
                let type_ok = e.attr_local(&markup5ever::local_name!("type")).is_none_or(supported_image_type);
                if media_ok && type_ok {
                    if let Some(srcset) = e.attr_local(&markup5ever::local_name!("srcset")) {
                        let candidates = parse_srcset(
                            srcset,
                            source_size(e.attr_local(&markup5ever::local_name!("sizes")), viewport),
                        );
                        if let Some(c) = pick_candidate(&candidates, viewport.device_scale_factor) {
                            return Some(c.clone());
                        }
                    }
                }
            }
            child = self.n(child).next;
        }
        self.own_srcset_source(i, viewport, auto)
    }

    /// The candidate the `<img>`'s own `srcset` selects; its `src` counts as
    /// a 1x candidate when the set has no 1x one and no width descriptors.
    fn own_srcset_source(&self, img: u32, viewport: &Viewport, auto: Option<f64>) -> Option<Candidate> {
        let e = self.n(img).element()?;
        let srcset = e.attr_local(&markup5ever::local_name!("srcset"))?;
        let mut candidates =
            parse_srcset(srcset, source_size_auto(e.attr_local(&markup5ever::local_name!("sizes")), viewport, auto));
        let has_width = srcset_has_width_descriptor(srcset);
        if let Some(src) = e.attr_local(&markup5ever::local_name!("src")).filter(|src| !src.is_empty()) {
            if !has_width && !candidates.iter().any(|c| c.density == 1.0) {
                candidates.push(Candidate { url: src.to_owned(), density: 1.0 });
            }
        }
        pick_candidate(&candidates, viewport.device_scale_factor).cloned()
    }
}

/// HTML "update the source set" step: a `<source type>` that is empty (after trimming) or whose
/// MIME type's essence (parameters dropped; anything else, a list, `image/*`, spaces inside,
/// is no MIME type essence) names an image format the browser supports.
pub fn supported_image_type(t: &str) -> bool {
    let t = t.trim_matches(|c: char| c.is_ascii_whitespace());
    if t.is_empty() {
        return true;
    }
    let essence = t.split(';').next().unwrap_or("").trim_matches(|c: char| c.is_ascii_whitespace()).to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "image/gif" | "image/png" | "image/apng" | "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/webp" | "image/avif" | "image/svg+xml"
            | "image/x-icon" | "image/vnd.microsoft.icon" | "image/bmp"
    )
}

/// Whether a candidate of `srcset` has a width descriptor.
fn srcset_has_width_descriptor(srcset: &str) -> bool {
    srcset.split(',').any(|candidate| {
        candidate
            .split_ascii_whitespace()
            .skip(1)
            .any(|d| d.strip_suffix('w').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
    })
}

/// HTML "parse a sizes attribute": the source size in CSS px, through Stylo's
/// `<source-size-list>` (CSS syntax: comments, functions closed at the end, `calc()`/`min()`/
/// `max()`/`clamp()`, every length unit, media conditions with `not`/`and`/`or` and unknown ones
/// that match nothing; no media types). The first entry whose condition matches gives the size,
/// else the entry without one, else `100vw`. Font-relative units use the initial font (Stylo's
/// fallback metrics: `ex` and `ch` are 0.5em).
/// [`source_size`] with HTML's `auto`: a first entry `auto` is `auto` (the lazy image's concrete
/// width) when given, else left out.
pub fn source_size_auto(sizes: Option<&str>, viewport: &Viewport, auto: Option<f64>) -> f64 {
    let Some(sizes) = sizes else { return source_size(None, viewport) };
    let entries = split_top_level_commas(sizes);
    if entries.first().is_some_and(|first| first.trim().eq_ignore_ascii_case("auto")) {
        if let Some(width) = auto {
            return width.max(0.0);
        }
        let rest = entries[1..].join(",");
        return source_size(if rest.trim().is_empty() { None } else { Some(&rest) }, viewport);
    }
    source_size(Some(sizes), viewport)
}

pub fn source_size(sizes: Option<&str>, viewport: &Viewport) -> f64 {
    use style::context::QuirksMode;
    use style::servo_arc::Arc;
    use style::stylesheets::{CssRuleType, Origin, UrlExtraData};
    let default = f64::from(viewport.width);
    let Some(sizes) = sizes else { return default };
    crate::stylo::enable_features();
    let Ok(url) = url::Url::parse("about:blank") else { return default };
    let url = UrlExtraData(Arc::new(url));
    let context = style::parser::ParserContext::new(
        Origin::Author,
        &url,
        Some(CssRuleType::Style),
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    // An entry whose size divides by zero is invalid (Chromium; Stylo would compute NaN, or
    // clamp an infinity): left out before Stylo sees the list.
    let kept: Vec<&str> = split_top_level_commas(sizes).into_iter().filter(|entry| !divides_by_zero(entry)).collect();
    let sizes = kept.join(",");
    let mut parser = cssparser::Parser::new(&sizes);
    let list = style::values::specified::source_size_list::SourceSizeList::parse(&context, &mut parser);
    let device = sizes_device(viewport);
    f64::from(list.evaluate(&device, QuirksMode::NoQuirks).to_f32_px())
}

/// `s` split at its commas outside parentheses and brackets.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth <= 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Whether `entry` divides by a literal zero (`/ 0`, `/ 0.0`, `/ -0`) inside a function.
fn divides_by_zero(entry: &str) -> bool {
    let b = entry.as_bytes();
    let mut i = 0;
    while let Some(off) = b[i..].iter().position(|&c| c == b'/') {
        let mut j = i + off + 1;
        if b.get(j) == Some(&b'*') {
            // A comment.
            i = j;
            continue;
        }
        while b.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
            j += 1;
        }
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let start = j;
        while b.get(j).is_some_and(|c| *c == b'0' || *c == b'.') {
            j += 1;
        }
        let zero = j > start && b[start..j].contains(&b'0');
        if zero && !b.get(j).is_some_and(|c| c.is_ascii_alphanumeric()) {
            return true;
        }
        i = i + off + 1;
    }
    false
}

type MediaFontMetrics = Box<dyn style::device::servo::FontMetricsProvider>;

thread_local! {
    static MEDIA_FONTS: std::cell::Cell<Option<fn() -> MediaFontMetrics>> = const { std::cell::Cell::new(None) };
}

pub fn set_media_font_provider(provider: Option<fn() -> MediaFontMetrics>) {
    MEDIA_FONTS.set(provider);
}

/// Fallback metrics for embedders without a font provider.
#[derive(Debug)]
struct NoFontMetrics;
impl style::device::servo::FontMetricsProvider for NoFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &style::properties::style_structs::Font,
        _base_size: style::values::computed::CSSPixelLength,
        _flags: style::values::computed::font::QueryFontMetricsFlags,
    ) -> style::font_metrics::FontMetrics {
        Default::default()
    }
    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> style::values::computed::Length {
        let size = if generic == style::values::computed::font::GenericFontFamily::Monospace { 13.0 } else { 16.0 };
        style::values::computed::Length::new(size)
    }
}

/// A Stylo device for `viewport`'s media conditions and viewport units.
pub(crate) fn sizes_device(viewport: &Viewport) -> style::device::Device {
    use style::servo::media_features::PointerCapabilities;
    use style::properties::ComputedValues;
    use style::properties::style_structs::Font;
    let size = euclid::Size2D::new(viewport.width as f32, viewport.height as f32);
    let scale = viewport.device_scale_factor as f32;
    let mut pointer = match viewport.pointer {
        crate::media::Pointer::None => PointerCapabilities::empty(),
        crate::media::Pointer::Fine => PointerCapabilities::FINE,
        crate::media::Pointer::Coarse => PointerCapabilities::COARSE,
    };
    if viewport.hover { pointer |= PointerCapabilities::HOVER; }
    style::device::Device::new(
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        MEDIA_FONTS.get().map_or_else(|| Box::new(NoFontMetrics) as MediaFontMetrics, |make| make()),
        "Submit".into(),
        style::device::servo::MediaData {
            media_type: if viewport.print { style::media_queries::MediaType::print() } else { style::media_queries::MediaType::screen() },
            viewport_size: size,
            device_size: euclid::Size2D::new(viewport.screen_width.unwrap_or(viewport.width) as f32 * scale, viewport.screen_height.unwrap_or(viewport.height) as f32 * scale),
            device_pixel_ratio: euclid::Scale::new(scale),
            quirks_mode: style::context::QuirksMode::NoQuirks,
            prefers_color_scheme: match viewport.color_scheme {
                crate::media::ColorScheme::Dark => style::queries::values::PrefersColorScheme::Dark,
                _ => style::queries::values::PrefersColorScheme::Light,
            },
            prefers_reduced_motion: viewport.reduced_motion,
            primary_pointer_capabilities: pointer,
            all_pointer_capabilities: pointer,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_attribute_follows_css_syntax() {
        let vp = Viewport::new(800, 600);
        let size = |s: &str| source_size(Some(s), &vp);
        assert_eq!(size("1px"), 1.0);
        assert_eq!(size("/* */1px/* */"), 1.0);
        assert_eq!(size("(min-width:0) calc(1px"), 1.0);
        assert_eq!(size("(min-width:0) min(1px, 200vw"), 1.0);
        assert_eq!(size("10vmin"), 60.0);
        assert_eq!(size("10vmax"), 80.0);
        // Media types are not media conditions; an unknown condition matches nothing.
        assert_eq!(size("all 1px"), 800.0);
        assert_eq!(size("(unknown-mf-name) 1px, 2px"), 2.0);
        assert_eq!(size("not (unknown-mf-name) 1px, 2px"), 2.0);
        assert_eq!(size("(min-width:0) or (unknown-mf-name) 1px"), 1.0);
        // Percentages, negative lengths and divisions by zero are not source sizes.
        assert_eq!(size("50%, 3px"), 3.0);
        assert_eq!(size("-1px, 3px"), 3.0);
        assert_eq!(size("calc(0px / 0), 3px"), 3.0);
        assert_eq!(size("calc(-1px / 0)"), 800.0);
        assert_eq!(source_size(None, &vp), 800.0);
    }
}
