//! Stylo (Servo's style system) over pyro-dom nodes, for selector matching
//! (the DOM's query APIs, [`select`]) and for reading computed values
//! ([`serialize`]). Computed styles themselves come from the layout
//! engine's cascade (Blitz), whose `ComputedValues` are Stylo's too.

pub mod declarations;
mod node;
pub mod select;

use std::sync::LazyLock;

use style::context::QuirksMode;
use style::properties::ComputedValues;
use style::servo_arc::Arc;
use style::stylesheets::UrlExtraData;
use style::values::computed::CSSPixelLength;

/// Turns on the Stylo features the layout engine (Blitz) turns on when it
/// makes its first document. Prefs are process-wide and read while parsing:
/// without this, a property or value pyro-dom parsed before the first layout
/// (an `element.style` write, say `gridTemplateColumns`) would be unknown,
/// and the property cached as such. The list is blitz-dom's
/// `init_stylo_prefs` (Stylo 0.23 names).
pub(crate) fn enable_features() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        style_config::set_pref!("layout.css.anchor-positioning.enabled", true);
        style_config::set_pref!("layout.css.flexbox-balance.enabled", true);
        style_config::set_pref!("layout.css.text-box.enabled", true);
        // pyro: `font-format()`/`font-tech()` in `@supports`, `tech()` in `@font-face` sources.
        style_config::set_pref!("layout.css.font-tech.enabled", true);
        // pyro: `initial-letter` on `::first-letter` (`layout::first_letter`).
        style_config::set_pref!("layout.css.initial-letter.enabled", true);
        style_config::set_pref!("layout.css.dominant-baseline.enabled", true);
        // pyro: `light-dark()` of images (the computed value is the image of the used scheme).
        style_config::set_pref!("layout.css.light-dark.images.enabled", true);
        style_config::set_pref!("layout.css.background-clip.text.enabled", true);
        style_config::set_pref!("layout.css.webkit-text-fill-color.enabled", true);
        style_config::set_pref!("layout.css.columns.enabled", true);
        style_config::set_pref!("layout.css.hyphens.enabled", true);
        style_config::set_pref!("layout.css.font-variant-emoji.enabled", true);
        style_config::set_pref!("layout.css.basic-shape-shape.enabled", true);
        style_config::set_pref!("layout.css.tree-counting-functions.enabled", true);
        style_config::set_pref!("layout.css.progress-function.enabled", true);
        style_config::set_pref!("layout.css.random.enabled", true);
        style_config::set_pref!("layout.css.font-variations.enabled", true);
        style_config::set_pref!("layout.css.text-indent-keywords.enabled", true);
        // pyro: `grid-template-rows/columns: subgrid` (Taffy lays subgrids out, css-grid-2 §9).
        style_config::set_pref!("layout.css.grid-template-subgrid-value.enabled", true);
        style_config::set_pref!("layout.css.word-break.break-word.enabled", true);
        // pyro-render builds Blitz with its `writing-mode` feature.
        style_config::set_pref!("layout.css.writing-mode.enabled", true);
        style_config::set_pref!("layout.css.writing-mode.sideways.enabled", true);
        // Properties Stylo 0.22 enabled with `layout.unimplemented`.
        style_config::set_pref!("layout.css.appearance.enabled", true);
        style_config::set_pref!("layout.css.backdrop-filter.enabled", true);
        style_config::set_pref!("layout.css.contain.enabled", true);
        style_config::set_pref!("layout.css.content.alt-text.enabled", true);
        style_config::set_pref!("layout.css.corner-shape.enabled", true);
        // pyro: `closest-corner`/`farthest-corner` radii in `ellipse()` (CSS Shapes 2).
        style_config::set_pref!("layout.css.ellipse-corners.enabled", true);
        style_config::set_pref!("layout.css.counters.enabled", true);
        style_config::set_pref!("layout.css.first-line.enabled", true);
        style_config::set_pref!("layout.css.font-size-adjust.enabled", true);
        // pyro: `font-synthesis-style`, `-small-caps` and `-position` (and the shorthand).
        style_config::set_pref!("layout.css.font-synthesis.enabled", true);
        // pyro: `font-variant-caps: all-small-caps | petite-caps | all-petite-caps | unicase |
        // titling-caps` (blitz-dom maps each to its OpenType features).
        style_config::set_pref!("layout.css.font-variant-caps.all-values.enabled", true);
        // pyro: `font-palette` and `@font-palette-values` (blitz-paint draws COLR glyphs in the
        // palette they select).
        style_config::set_pref!("layout.css.font-palette.enabled", true);
        style_config::set_pref!("layout.css.forced-color-adjust.enabled", true);
        style_config::set_pref!("layout.css.masking.enabled", true);
        style_config::set_pref!("layout.css.math-properties.enabled", true);
        style_config::set_pref!("layout.css.motion-path.enabled", true);
        // pyro: as blitz-dom's `init_stylo_prefs`, for declarations script parses before the
        // layout engine starts: `offset-path: url()`, `text-wrap-style: balance | pretty`.
        style_config::set_pref!("layout.css.motion-path-url.enabled", true);
        style_config::set_pref!("layout.css.text-wrap-style.balance.enabled", true);
        style_config::set_pref!("layout.css.text-wrap-style.pretty.enabled", true);
        style_config::set_pref!("layout.css.overscroll-behavior.enabled", true);
        style_config::set_pref!("layout.css.scroll-driven-animations.enabled", true);
        // pyro: as blitz-dom's `init_stylo_prefs`: `display` in `@keyframes`, and Stylo's
        // `Animate for Display` (css-display-4 §2.1).
        style_config::set_pref!("layout.css.display-animations.enabled", true);
        style_config::set_pref!("layout.css.scroll-behavior.enabled", true);
        style_config::set_pref!("layout.css.scrollbar-styling.enabled", true);
        style_config::set_pref!("layout.css.text-decoration-inset.enabled", true);
        style_config::set_pref!("layout.css.text-decoration-inset-percentage.enabled", true);
        // pyro: ruby annotations and emphasis marks (`TextAnnotation`).
        style_config::set_pref!("layout.css.text-emphasis.enabled", true);
        style_config::set_pref!("layout.css.ruby.enabled", true);
        style_config::set_pref!("layout.css.text-overflow.enabled", true);
        style_config::set_pref!("layout.css.text-underline-position.enabled", true);
        style_config::set_pref!("layout.css.view-transitions.enabled", true);
        style_config::set_pref!("dom.viewTransitions.cross-document.enabled", true);
        style_config::set_pref!("layout.css.writing-mode.text-orientation.enabled", true);
        style_config::set_pref!("layout.css.text-autospace.enabled", true);
        style_config::set_pref!("layout.css.text-combine-upright.enabled", true);
        style_config::set_pref!("layout.css.zoom.enabled", true);
        style_config::set_pref!("layout.css.has-selector.enabled", true);
        style_config::set_pref!("layout.css.container-queries.enabled", true);
        style_config::set_pref!("layout.css.nth-child-of.enabled", true);
        style_config::set_pref!("layout.css.starting-style-at-rules.enabled", true);
        style_config::set_pref!("layout.css.at-scope.enabled", true);
        style_config::set_pref!("layout.css.style-queries.enabled", true);
        style_config::set_pref!("layout.css.line-clamp.enabled", true);
        style_config::set_pref!("layout.css.fit-content-function.enabled", true);
        style_config::set_pref!("layout.css.supports.at-rule.enabled", true);
    });
}

/// The property `name` names, if Stylo knows it (parsed once per name).
pub(crate) fn property_id(name: &str) -> Option<style::properties::PropertyId> {
    use style::properties::PropertyId;
    thread_local! {
        /// Property names as parsed (the same few dozen, read per element).
        static IDS: std::cell::RefCell<rustc_hash::FxHashMap<Box<str>, Option<PropertyId>>> = Default::default();
    }
    IDS.with(|ids| {
        if let Some(id) = ids.borrow().get(name) {
            return id.clone();
        }
        enable_features();
        let id = PropertyId::parse_enabled_for_all_content(name).ok();
        let mut ids = ids.borrow_mut();
        if ids.len() < 4096 {
            ids.insert(name.into(), id.clone());
        }
        id
    })
}

/// A property's computed value as `getComputedStyle` serializes it: a
/// longhand directly, a shorthand from its resolved longhands, a
/// custom property as specified. `None` for unknown properties.
pub fn serialize(style: &ComputedValues, name: &str) -> Option<String> {
    let id = property_id(name)?;
    match id.as_shorthand() {
        Ok(shorthand) => {
            if shorthand == style::properties::ShorthandId::All {
                return None;
            }
            let mut context = style::values::resolved::Context {
                style,
                for_property: id.clone(),
                current_longhand: None,
            };
            let declarations: Vec<_> = shorthand
                .longhands()
                .map(|longhand| style.computed_or_resolved_declaration(longhand, Some(&mut context)))
                .collect();
            let mut out = String::new();
            let declarations: Vec<_> = declarations.iter().collect();
            shorthand.longhands_to_css(&declarations, &mut out).ok()?;
            Some(resolved_alignment(shorthand.name(), out))
        }
        Err(declaration) => {
            // Serialize resolved line-height in CSS pixels at the element's effective zoom.
            if name.eq_ignore_ascii_case("line-height") {
                use style::values::generics::font::GenericLineHeight;
                let font = style.get_font();
                let zoomed_px = match font.line_height {
                    GenericLineHeight::Length(l) => Some(l.0.px()),
                    GenericLineHeight::Number(n) => Some(n.0 * font.font_size.computed_size().px()),
                    _ => None,
                };
                if let Some(length) = zoomed_px {
                    let mut out = String::new();
                    let px = style.effective_zoom.unzoom(length);
                    style_traits::ToCss::to_css(&CSSPixelLength::new(px), &mut style_traits::CssWriter::new(&mut out))
                        .ok()?;
                    return Some(out);
                }
            }
            let value = resolved_alignment(name, style.computed_value_to_string(declaration));
            // Gecko's prefixed keywords, as Chrome spells them.
            Some(match value.strip_prefix("-moz-") {
                Some(rest @ ("center" | "left" | "right")) if name.eq_ignore_ascii_case("text-align") => {
                    format!("-webkit-{rest}")
                }
                _ => value,
            })
        }
    }
}

/// Preserve the DOM double carried by a CSSUnitValue across the text bridge to Typed OM.
pub(crate) fn precise_typed_numeric(style: &ComputedValues, name: &str, css: &str) -> Option<String> {
    use style::typed_om::{NumericValue, TypedValue};
    let id = property_id(name)?;
    let longhand = id.longhand_id()?;
    let border_width = longhand.logical_group() == Some(style::properties::LogicalGroupId::BorderWidth);
    let list = if border_width {
        // CSSOM resolves a hidden/none border to zero. Typed OM exposes its computed width.
        style.property_value_to_typed_value_list(longhand)?
    } else {
        let mut context = style::values::resolved::Context { style, for_property: id, current_longhand: None };
        let declaration = style.computed_or_resolved_declaration(longhand, Some(&mut context));
        declaration.to_typed_value_list()?
    };
    let [TypedValue::Numeric(NumericValue::Unit(unit))] = list.values.as_slice() else { return None };
    let suffix = match unit.unit_str() {
        "percent" => "%",
        "number" => "",
        other => other,
    };
    // Compound values can reify differently for different properties. Only replace a
    // serialized standalone number with the same unit as the reified value.
    css.strip_suffix(suffix)?.parse::<f64>().ok()?;
    // CSSUnitValue stores numbers as f32 in Stylo, but an <integer> CSS
    // declaration preserves all 32 bits. Keep the exact integer serialization.
    if suffix.is_empty() && css.parse::<i32>().is_ok_and(|n|
        n as f32 == unit.value && f64::from(n) != f64::from(unit.value)) {
        return None;
    }
    let value = if border_width { style.effective_zoom.unzoom(unit.value) } else { unit.value };
    value.is_finite().then(|| format!("{}{suffix}", f64::from(value)))
}

fn resolved_alignment(name: &str, value: String) -> String {
    if matches!(name, "align-items" | "align-content" | "align-self" | "justify-items" | "justify-content" | "justify-self" | "place-items" | "place-content" | "place-self")
        && value.contains("flex-")
    {
        value.replace("flex-start", "flow-start").replace("flex-end", "flow-end")
    } else {
        value
    }
}

/// The URL style values resolve against (computed styles keep relative
/// `url()`s as written; the loader resolves them).
pub(crate) fn url_data() -> UrlExtraData {
    static URL: LazyLock<UrlExtraData> =
        LazyLock::new(|| UrlExtraData(Arc::new(url::Url::parse("about:blank").expect("url"))));
    URL.clone()
}

/// The longhand properties a computed style lists (`getComputedStyle(el)`'s
/// indexed properties), in alphabetical order.
pub fn longhand_names() -> &'static [&'static str] {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        use style::properties::ShorthandId;
        enable_features();
        // `all` covers every longhand but these two.
        let mut names: Vec<&'static str> = ShorthandId::All.longhands().map(|l| l.name()).collect();
        names.extend(["direction", "unicode-bidi"]);
        names.retain(|name| property_id(name).is_some());
        names.sort_unstable();
        names.dedup();
        names
    });
    &NAMES
}

/// Whether `name` is a CSS property (longhand or shorthand) the engine knows.
pub fn is_property(name: &str) -> bool {
    property_id(name).is_some()
}

/// Turns on the Stylo features the page's cascade parses with (Blitz sets the
/// same process-wide prefs when it makes its first document): `CSS.supports`
/// can run before any document was laid out.
fn ensure_parse_prefs() {
    enable_features();
}

/// `CSS.supports(property, value)` (CSS Conditional 3): whether `property`
/// names a property the engine supports (ASCII case-insensitively; a custom
/// property as written) and `value` parses as that property's value in an
/// author style rule. The few properties Stylo's servo build does not parse
/// but pyro-dom computes ([`crate::gecko_only`]) count as supported.
pub fn supports_declaration(property: &str, value: &str) -> bool {
    use style::properties::{parse_one_declaration_into, SourcePropertyDeclaration};
    use style::stylesheets::{CssRuleType, Origin};
    ensure_parse_prefs();
    let Ok(id) = style::properties::PropertyId::parse_enabled_for_all_content(property) else {
        let lower = property.to_ascii_lowercase();
        return crate::gecko_only::handles(&lower) && !value.trim().is_empty();
    };
    let mut declarations = SourcePropertyDeclaration::default();
    parse_one_declaration_into(
        &mut declarations,
        id,
        value,
        Origin::Author,
        &url_data(),
        None,
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    )
    .is_ok()
}

/// Parse a DOMMatrix transform list without an element or a reference box.
pub fn matrix_from_css_text(text: &str) -> Option<([f64; 16], bool)> {
    use style::parser::{Parse, ParserContext};
    use style::values::specified::transform::Transform;
    ensure_parse_prefs();
    let url = url_data();
    let context = ParserContext::new(
        style::stylesheets::Origin::Author, &url, None, style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks, Default::default(), None, None, Default::default(),
    );
    let transform = cssparser::Parser::new(text).parse_entirely(|input| Transform::parse(&context, input)).ok()?;
    let (matrix, has_3d) = transform.to_transform_3d_matrix_f64(None).ok()?;
    Some((matrix.to_array(), !has_3d))
}

/// `CSS.supports(conditionText)`: `text` parsed as a `<supports-condition>`,
/// else as one declaration in parentheses, and evaluated as `@supports`
/// would be (declarations, `selector()`, `not` / `and` / `or`; unknown
/// syntax is false).
pub fn supports_condition(text: &str) -> bool {
    use style::stylesheets::supports_rule::SupportsCondition;
    use style::stylesheets::{CssRuleType, Origin};
    ensure_parse_prefs();
    let url = url_data();
    let mut context = style::parser::ParserContext::new(
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
    let parse = |text: &str| {
        let mut parser = cssparser::Parser::new(text);
        parser.parse_entirely(|p| SupportsCondition::parse(p)).ok()
    };
    match parse(text) {
        Some(condition) => condition.eval(&mut context),
        None => parse(&format!("({text})")).is_some_and(|condition| condition.eval(&mut context)),
    }
}

pub use style::selector_parser::PseudoElement;

/// What `getComputedStyle(el, pseudoElt)`'s second argument selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudoArg {
    /// No colon first (or empty): the element's own style.
    Element,
    /// A pseudo-element the engine styles.
    Pseudo(PseudoElement),
    /// Starts with a colon but is no supported pseudo-element: an empty
    /// style (CSSOM: `obj` is null).
    Invalid,
}

/// Parses `getComputedStyle`'s `pseudoElt` (CSSOM): text without a leading
/// colon is ignored; otherwise exactly one `<pseudo-element-selector>` with
/// nothing after it, the legacy single-colon forms of `::before`, `::after`,
/// `::first-line`, and `::first-letter` included.
pub fn parse_pseudo_arg(text: &str) -> PseudoArg {
    if !text.starts_with(':') {
        return PseudoArg::Element;
    }
    if text.get(..18).is_some_and(|prefix| prefix.eq_ignore_ascii_case("::view-transition-")) {
        let mut parser = cssparser::Parser::new(&text[2..]);
        if let Ok(kind) = parser.expect_function().cloned() {
            let result = parser.parse_nested_block(|input| {
                input.try_parse(|input| input.expect_ident_cloned())?;
                Ok::<_, cssparser::ParseError<()>>(())
            });
            if result.is_ok() && parser.is_exhausted() {
                let mut parser = cssparser::Parser::new(&text[2..]);
                let _ = parser.next();
                if let Ok(pseudo) = parser.parse_nested_block(|input| style::selector_parser::transition_selector(&kind.to_ascii_lowercase(), input)) { return PseudoArg::Pseudo(pseudo); }
            }
        }
        return PseudoArg::Invalid;
    }
    // `::highlight(<name>)` (CSS Custom Highlight API).
    if let Some(inner) = text
        .get(..12)
        .filter(|head| head.eq_ignore_ascii_case("::highlight("))
        .and_then(|_| text[12..].strip_suffix(')'))
    {
        return style::selector_parser::highlight_selector(&mut cssparser::Parser::new(inner))
            .map(PseudoArg::Pseudo).unwrap_or(PseudoArg::Invalid);
    }
    let mut parser = cssparser::Parser::new(text);
    let mut next = || parser.next_including_whitespace_and_comments().ok().cloned();
    let _ = next(); // the leading colon
    let (double, ident) = match next() {
        Some(cssparser::Token::Colon) => (true, next()),
        other => (false, other),
    };
    let Some(cssparser::Token::Ident(name)) = ident else { return PseudoArg::Invalid };
    if next().is_some() {
        return PseudoArg::Invalid;
    }
    let name = name.to_ascii_lowercase();
    let pseudo = match name.as_str() {
        "before" => PseudoElement::Before,
        "after" => PseudoElement::After,
        "first-line" => PseudoElement::FirstLine,
        "first-letter" => PseudoElement::FirstLetter,
        "selection" if double => PseudoElement::Selection,
        "view-transition" if double => PseudoElement::ViewTransition,
        "target-text" if double => PseudoElement::TargetText,
        "spelling-error" if double => PseudoElement::SpellingError,
        "grammar-error" if double => PseudoElement::GrammarError,
        "search-text" if double => PseudoElement::SearchText,
        "checkmark" if double => PseudoElement::Checkmark,
        "marker" if double => PseudoElement::Marker,
        "placeholder" if double => PseudoElement::Placeholder,
        "backdrop" if double => PseudoElement::Backdrop,
        "file-selector-button" if double => PseudoElement::FileSelectorButton,
        "-webkit-scrollbar" if double => PseudoElement::WebkitScrollbar,
        "-webkit-scrollbar-thumb" if double => PseudoElement::WebkitScrollbarThumb,
        "-webkit-scrollbar-track" if double => PseudoElement::WebkitScrollbarTrack,
        "-webkit-scrollbar-track-piece" if double => PseudoElement::WebkitScrollbarTrackPiece,
        "-webkit-scrollbar-button" if double => PseudoElement::WebkitScrollbarButton,
        "-webkit-scrollbar-corner" if double => PseudoElement::WebkitScrollbarCorner,
        "-webkit-resizer" if double => PseudoElement::WebkitResizer,
        _ => return PseudoArg::Invalid,
    };
    PseudoArg::Pseudo(pseudo)
}

/// Validates a `CSS.registerProperty()` definition (CSS Properties and
/// Values API, "registerProperty"): `name` a custom property name, `syntax`
/// a syntax definition, and `initial` present, parsing per `syntax` and
/// computationally independent unless `syntax` is `*`. Returns the initial
/// value's text; `Err` carries the SyntaxError message.
pub fn validate_property_registration(
    name: &str,
    syntax: &str,
    initial: Option<&str>,
) -> Result<Option<String>, &'static str> {
    use style::properties_and_values::registry::PropertyRegistration;
    use style::properties_and_values::rule::PropertyRegistrationError;
    use style::properties_and_values::syntax::Descriptor;
    ensure_parse_prefs();
    let Ok(custom_name) = style::custom_properties::parse_name(name) else {
        return Err("The name provided is not a valid custom property name.");
    };
    let custom_name = style::custom_properties::Name::from(custom_name);
    let syntax = Descriptor::from_str(syntax, false)
        .map_err(|_| "The syntax provided is not a valid custom property syntax.")?;
    let value = match initial {
        Some(text) => {
            let mut parser = cssparser::Parser::new(text);
            let url = url_data();
            let context = style::parser::ParserContext::new(
                style::stylesheets::Origin::Author,
                &url,
                None,
                style_traits::ParsingMode::DEFAULT,
                style::context::QuirksMode::NoQuirks,
                Default::default(),
                None,
                None,
                Default::default(),
            );
            let value = parser
                .parse_entirely(|p| style::custom_properties::SpecifiedValue::parse(&context, p))
                .map_err(|_| "The initial value provided does not parse for the given syntax.")?;
            Some(value)
        }
        None => None,
    };
    PropertyRegistration::validate_initial_value(
        style::properties::PropertyIdRef::Custom(&custom_name),
        &syntax,
        value.as_ref(),
    )
    .map_err(|e| match e {
        PropertyRegistrationError::NoInitialValue => "An initial value must be provided if the syntax is not '*'",
        PropertyRegistrationError::InvalidInitialValue => {
            "The initial value provided does not parse for the given syntax."
        }
        PropertyRegistrationError::InitialValueNotComputationallyIndependent => {
            "The initial value provided is not computationally independent."
        }
    })?;
    Ok(initial.map(|t| t.trim_matches(|c: char| c.is_ascii_whitespace()).to_owned()))
}
