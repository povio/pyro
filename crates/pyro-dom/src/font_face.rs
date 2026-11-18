//! CSS Font Loading 3 with Stylo's parsers: `FontFace` descriptors parsed and serialized as the
//! `@font-face` rule's (`descriptor`), its source string as a `src` value (`sources`), the
//! CSS-connected faces of a document's sheets (`css_faces`) and the `font` shorthand
//! `FontFaceSet.check()`/`load()` match faces with (`font_query`).

use std::fmt::Write as _;

use style::context::QuirksMode;
use style::font_face::Source;
use style::properties::font_face::{DescriptorId, Descriptors};
use style::servo_arc::Arc;
use style::shared_lock::SharedRwLock;
use style::stylesheets::{AllowImportRules, CssRule, CssRuleType, Origin, StylesheetContents, UrlExtraData};
use style_traits::{CssStringWriter, ToCss};

/// The `FontFace` attributes and the descriptor each reflects, with its initial value.
pub const ATTRIBUTES: &[(&str, &str, &str)] = &[
    ("style", "font-style", "normal"),
    ("weight", "font-weight", "normal"),
    ("stretch", "font-stretch", "normal"),
    ("width", "font-width", "normal"),
    ("unicodeRange", "unicode-range", "U+0-10FFFF"),
    ("featureSettings", "font-feature-settings", "normal"),
    ("variationSettings", "font-variation-settings", "normal"),
    ("display", "font-display", "auto"),
    ("ascentOverride", "ascent-override", "normal"),
    ("descentOverride", "descent-override", "normal"),
    ("lineGapOverride", "line-gap-override", "normal"),
    ("sizeAdjust", "size-adjust", "100%"),
];

fn context<'a>(url: &'a UrlExtraData) -> style::parser::ParserContext<'a> {
    style::parser::ParserContext::new(
        Origin::Author,
        url,
        Some(CssRuleType::FontFace),
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    )
}

fn url_data(base: &str) -> UrlExtraData {
    let url = url::Url::parse(base).unwrap_or_else(|_| url::Url::parse("about:blank").expect("url"));
    UrlExtraData(Arc::new(url))
}

/// `value` parsed as the `@font-face` descriptor `name` and serialized, `None` when it does not
/// parse (`FontFace` throws a `SyntaxError`). CSS-wide keywords and `var()` are not descriptor
/// values.
pub fn descriptor(name: &str, value: &str) -> Option<String> {
    crate::stylo::enable_features();
    let id = DescriptorId::from_ident(name).ok()?;
    let url = url_data("about:blank");
    let context = context(&url);
    let mut descriptors = Descriptors::default();
    let mut parser = cssparser::Parser::new(value);
    descriptors.set(id, &context, &mut parser).ok()?;
    let mut css = CssStringWriter::new();
    descriptors.get(id, &mut css).ok()?;
    Some(css)
}

/// One source of a face, in the order to try.
#[derive(Clone, Debug, PartialEq)]
pub enum FontSource {
    /// `url()`, resolved; the `format()` hint, if any.
    Url(String, Option<String>),
    /// `local()`: a font installed on the system.
    Local(String),
}

fn convert_sources(list: &style::font_face::SourceList) -> Vec<FontSource> {
    list.0
        .iter()
        .filter_map(|source| match source {
            Source::Url(u) => {
                let url = u.url.url()?.to_string();
                let format = u.format_hint.as_ref().map(|f| f.to_css_string());
                Some(FontSource::Url(url, format))
            }
            Source::Local(name) => Some(FontSource::Local(name.name.to_string())),
        })
        .collect()
}

/// A `FontFace` source string as a `src` value resolved against `base`, `None` when it does not
/// parse.
pub fn sources(value: &str, base: &str) -> Option<Vec<FontSource>> {
    crate::stylo::enable_features();
    let url = url_data(base);
    let context = context(&url);
    let mut descriptors = Descriptors::default();
    let mut parser = cssparser::Parser::new(value);
    descriptors.set(DescriptorId::Src, &context, &mut parser).ok()?;
    Some(convert_sources(descriptors.src.as_ref()?))
}

/// An `@font-face` rule of a document's sheets: a CSS-connected `FontFace`.
#[derive(Clone, Debug, PartialEq)]
pub struct CssFace {
    pub family: String,
    /// The descriptors the rule sets (`ATTRIBUTES`' descriptor names), serialized.
    pub descriptors: Vec<(&'static str, String)>,
    pub sources: Vec<FontSource>,
}

/// The `@font-face` rules with a family and a source in `css` (a sheet's text, its URLs resolved
/// against `base`), nested ones included, in order.
pub fn css_faces(css: &str, base: &str, out: &mut Vec<CssFace>) {
    if !css.contains("@font-face") && !css.to_ascii_lowercase().contains("@font-face") {
        return;
    }
    crate::stylo::enable_features();
    let lock = SharedRwLock::new();
    let contents = StylesheetContents::from_str(
        css,
        url_data(base),
        Origin::Author,
        &lock,
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::No,
        None,
    );
    let guard = lock.read();
    fn walk(rules: &[CssRule], guard: &style::shared_lock::SharedRwLockReadGuard, out: &mut Vec<CssFace>) {
        for rule in rules {
            if let CssRule::FontFace(face) = rule {
                let d = &face.read_with(guard).descriptors;
                let (Some(family), Some(src)) = (d.font_family.as_ref(), d.src.as_ref()) else { continue };
                let mut descriptors = Vec::new();
                for &(_, name, _) in ATTRIBUTES {
                    let Ok(id) = DescriptorId::from_ident(name) else { continue };
                    let mut css = CssStringWriter::new();
                    if d.get(id, &mut css).is_ok() && !css.is_empty() {
                        descriptors.push((name, css));
                    }
                }
                out.push(CssFace { family: family.name.to_string(), descriptors, sources: convert_sources(src) });
            } else {
                walk(rule.children(guard), guard, out);
            }
        }
    }
    walk(contents.rules(&guard), &guard, out);
}

/// The `font` shorthand of `FontFaceSet.check()`/`load()`, its longhands serialized.
#[derive(Clone, Debug, PartialEq)]
pub struct FontQuery {
    /// The named families in order (generic families are left out: no face of a set matches one).
    pub families: Vec<String>,
    pub style: String,
    pub weight: String,
    pub stretch: String,
}

/// `font` parsed as the `font` shorthand, `None` when it does not parse or is a CSS-wide keyword or
/// holds `var()` (`FontFaceSet` throws a `SyntaxError`).
pub fn font_query(font: &str) -> Option<FontQuery> {
    use style::properties::{
        parse_one_declaration_into, PropertyDeclaration, PropertyId, ShorthandId, SourcePropertyDeclaration,
    };
    use style::values::specified::font::FontFamily;
    crate::stylo::enable_features();
    let mut declarations = SourcePropertyDeclaration::default();
    parse_one_declaration_into(
        &mut declarations,
        PropertyId::NonCustom(ShorthandId::Font.into()),
        font,
        Origin::Author,
        &crate::stylo::url_data(),
        None,
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    )
    .ok()?;
    let mut query =
        FontQuery { families: Vec::new(), style: "normal".into(), weight: "normal".into(), stretch: "normal".into() };
    let mut family_seen = false;
    for decl in declarations.drain().declarations {
        let text = |d: &PropertyDeclaration| {
            let mut s = String::new();
            let _ = d.to_css(&mut s);
            s
        };
        match &decl {
            PropertyDeclaration::CSSWideKeyword(_) | PropertyDeclaration::WithVariables(_) => return None,
            PropertyDeclaration::FontFamily(FontFamily::Values(list)) => {
                family_seen = true;
                for f in list.list.iter() {
                    if let style::values::computed::font::SingleFontFamily::FamilyName(name) = f {
                        query.families.push(name.name.to_string());
                    }
                }
            }
            PropertyDeclaration::FontFamily(_) => family_seen = true,
            PropertyDeclaration::FontStyle(_) => query.style = text(&decl),
            PropertyDeclaration::FontWeight(_) => query.weight = text(&decl),
            PropertyDeclaration::FontWidth(_) => query.stretch = text(&decl),
            _ => {}
        }
    }
    family_seen.then_some(query)
}

/// A CSS string literal of `s` (`@font-face` text written for script-created faces).
pub fn css_string(s: &str) -> String {
    let mut out = String::new();
    let _ = cssparser::serialize_string(s, &mut out);
    out
}

/// One `@font-face` rule for a script-created face: `family` (any string), the serialized
/// `descriptors` (from [`descriptor`]) and its source URLs in order (a loaded face's is a `data:`
/// URL of its bytes).
pub fn face_rule(family: &str, descriptors: &[(String, String)], urls: &[String]) -> String {
    let src: Vec<String> = urls.iter().map(|u| format!("url({})", css_string(u))).collect();
    let mut rule = format!("@font-face{{font-family:{};src:{};", css_string(family), src.join(","));
    for (name, value) in descriptors {
        // Only descriptor names this module knows, with values Stylo parses.
        if ATTRIBUTES.iter().any(|(_, n, _)| n == name) && descriptor(name, value).is_some() {
            let _ = write!(rule, "{name}:{value};");
        }
    }
    rule.push('}');
    rule
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_round_trip_or_fail() {
        assert_eq!(descriptor("font-weight", "bold").as_deref(), Some("bold"));
        assert_eq!(descriptor("font-weight", "100 400").as_deref(), Some("100 400"));
        assert_eq!(descriptor("font-style", "oblique 40deg").as_deref(), Some("oblique 40deg"));
        assert_eq!(descriptor("unicode-range", "U+400-4ff").as_deref(), Some("U+400-4FF"));
        assert_eq!(descriptor("font-weight", "inherit"), None);
        assert_eq!(descriptor("font-weight", "var(--x)"), None);
        assert_eq!(descriptor("font-weight", "bold extra"), None);
    }

    #[test]
    fn sources_resolve_and_reject() {
        let s = sources("url(a.woff2) format('woff2'), local(Foo)", "https://e.test/css/x.css").unwrap();
        assert_eq!(s.len(), 2);
        assert!(matches!(&s[0], FontSource::Url(u, Some(_)) if u == "https://e.test/css/a.woff2"));
        assert_eq!(s[1], FontSource::Local("Foo".into()));
        assert!(sources("x", "https://e.test/").is_none());
    }

    #[test]
    fn css_faces_of_a_sheet() {
        let mut out = Vec::new();
        css_faces(
            "@media all { @font-face { font-family: Lato; src: url(l.ttf); font-weight: bold; unicode-range: U+400-4FF } }\
             @font-face { font-family: NoSrc }",
            "https://e.test/",
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].family, "Lato");
        assert_eq!(out[0].sources, vec![FontSource::Url("https://e.test/l.ttf".into(), None)]);
        assert!(out[0].descriptors.contains(&("font-weight", "bold".into())));
    }

    #[test]
    fn font_queries() {
        let q = font_query("bold 12px 'Web Font', serif").unwrap();
        assert_eq!(q.families, vec!["Web Font".to_string()]);
        assert_eq!(q.weight, "bold");
        assert!(font_query("inherit").is_none());
        assert!(font_query("var(--f)").is_none());
        assert!(font_query("12px").is_none());
    }
}
