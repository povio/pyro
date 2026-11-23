/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Common handling for the specified value CSS url() values.

use crate::derives::*;
use crate::parser::{Parse, ParserContext};
use crate::stylesheets::CorsMode;
use cssparser::{Parser, Token};
use std::fmt::{self, Write};
use style_traits::{CssWriter, ParseError, StyleParseErrorKind, ToCss};

#[cfg(feature = "gecko")]
pub mod gecko;
#[cfg(feature = "gecko")]
pub use gecko::{ComputedUrl, CssUrl, SpecifiedUrl};
#[cfg(feature = "servo")]
pub mod servo;
#[cfg(feature = "servo")]
pub use servo::{ComputedUrl, CssUrl, SpecifiedUrl};

impl CssUrl {
    /// Parse a URL with a particular CORS mode.
    pub fn parse_with_cors_mode(
        context: &ParserContext,
        input: &mut Parser,
        cors_mode: CorsMode,
    ) -> Result<Self, ParseError> {
        let start = input.position().byte_index();
        // pyro: `url( <string> <url-modifier>* )` (CSS Values 4 § 4.5.1): only the functional
        // notation with a quoted URL takes modifiers, the request URL modifiers of CSS Values 5
        // § 4.5.4. An unquoted `url()` is one token and takes none.
        let (url, modifiers) = match *input.next()? {
            Token::UnquotedUrl(ref value) => (value.as_ref().to_owned(), None),
            Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                input.parse_nested_block(|input| {
                    let url = input.expect_string()?.as_ref().to_owned();
                    let modifiers = RequestUrlModifiers::parse(context, input)?;
                    Ok((url, modifiers))
                })?
            },
            _ => return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError)),
        };
        let end = input.position().byte_index();
        let url = Self::parse_from_string(url, start, end, context, cors_mode)?;
        Ok(match modifiers {
            Some(modifiers) => url.with_modifiers(modifiers),
            None => url,
        })
    }

    /// Parse a URL from a string value that is a valid CSS token for a URL,
    /// enforcing attr()-tainting constraints if applicable.
    /// https://drafts.csswg.org/css-values-5/#attr-security
    pub fn parse_from_string(
        url: String,
        url_start: usize,
        url_end: usize,
        context: &ParserContext,
        cors_mode: CorsMode,
    ) -> Result<Self, ParseError> {
        use crate::substitution::AttrTaintedRange;
        use style_traits::StyleParseErrorKind;
        let range = AttrTaintedRange::new(url_start, url_end);
        if context.disallow_urls_in_range(&range) {
            return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
        }
        Ok(Self::new_from_string(url, context, cors_mode))
    }

    /// Create a new CSS URL that is attr()-untainted given a valid CSS token for a URL.
    /// Be cautious when calling `expect_url()` to not bypass attr()-tainting checks. If
    /// it's possible attr()'s were substituted into the `url`, DO NOT use this method.
    /// https://drafts.csswg.org/css-values-5/#attr-security
    pub fn new_from_untainted_string(
        url: String,
        context: &ParserContext,
        cors_mode: CorsMode,
    ) -> Self {
        debug_assert!(context.attr_tainted_regions.is_empty());
        Self::new_from_string(url, context, cors_mode)
    }
}

impl Parse for CssUrl {
    fn parse(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        Self::parse_with_cors_mode(context, input, CorsMode::None)
    }
}

/// pyro: the `cross-origin()` request URL modifier (CSS Values 5 § 4.5.4.1): the request's mode
/// is "cors", its credentials mode "same-origin" (`anonymous`) or "include" (`use-credentials`).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum CrossOriginModifier {
    /// `cross-origin(anonymous)`
    Anonymous,
    /// `cross-origin(use-credentials)`
    UseCredentials,
}

impl CrossOriginModifier {
    /// The keyword.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::UseCredentials => "use-credentials",
        }
    }
}

/// pyro: the `referrer-policy()` request URL modifier (CSS Values 5 § 4.5.4.3): a referrer
/// policy keyword (Referrer Policy § 3).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum ReferrerPolicyModifier {
    /// `no-referrer`
    NoReferrer,
    /// `no-referrer-when-downgrade`
    NoReferrerWhenDowngrade,
    /// `same-origin`
    SameOrigin,
    /// `origin`
    Origin,
    /// `strict-origin`
    StrictOrigin,
    /// `origin-when-cross-origin`
    OriginWhenCrossOrigin,
    /// `strict-origin-when-cross-origin`
    StrictOriginWhenCrossOrigin,
    /// `unsafe-url`
    UnsafeUrl,
}

impl ReferrerPolicyModifier {
    const ALL: [Self; 8] = [
        Self::NoReferrer,
        Self::NoReferrerWhenDowngrade,
        Self::SameOrigin,
        Self::Origin,
        Self::StrictOrigin,
        Self::OriginWhenCrossOrigin,
        Self::StrictOriginWhenCrossOrigin,
        Self::UnsafeUrl,
    ];

    /// The keyword, as the `Referrer-Policy` header and the `referrerpolicy` attribute spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoReferrer => "no-referrer",
            Self::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            Self::SameOrigin => "same-origin",
            Self::Origin => "origin",
            Self::StrictOrigin => "strict-origin",
            Self::OriginWhenCrossOrigin => "origin-when-cross-origin",
            Self::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            Self::UnsafeUrl => "unsafe-url",
        }
    }
}

/// pyro: the request URL modifiers of a `url()` (CSS Values 5 § 4.5.4), each at most once, and
/// the URL of the style sheet the `url()` was written in, the referrer of its request whatever
/// its mode, as Blink sends it (`CSSImageValue::PrepareFetch`: the sheet for an external sheet,
/// the document for inline style; CSS Values 4 "fetch a style resource" says the sheet only in
/// CORS mode). Every valid `url()` has these, most with no modifier set.
#[derive(Clone, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct RequestUrlModifiers {
    /// Fragment-only spelling retained alongside the referrer when computing a local CSS URL.
    #[cfg(feature = "servo")]
    #[serde(default)]
    #[ignore_malloc_size_of = "Arc"]
    local_fragment: Option<servo_arc::Arc<String>>,
    /// `cross-origin()`
    pub cross_origin: Option<CrossOriginModifier>,
    /// `integrity()`: the integrity metadata (Subresource Integrity § 3.3).
    pub integrity: Option<String>,
    /// `referrer-policy()`
    pub referrer_policy: Option<ReferrerPolicyModifier>,
    /// The URL of the style sheet (for a `<style>` sheet or a `style` attribute, the document's
    /// base URL) the `url()` was written in.
    #[ignore_malloc_size_of = "Arc"]
    pub referrer: Option<servo_arc::Arc<url::Url>>,
}

impl RequestUrlModifiers {
    /// Whether this resource was specified with a fragment-only URL.
    #[cfg(feature = "servo")]
    pub fn is_local_fragment(&self) -> bool {
        self.local_fragment.is_some()
    }

    /// Parses `<request-url-modifier>*` up to the end of `input`: `None` when there is none, an
    /// error for any other token, an unknown modifier, or one given twice.
    pub fn parse(context: &ParserContext, input: &mut Parser) -> Result<Option<Self>, ParseError> {
        let mut modifiers = Self {
            #[cfg(feature = "servo")]
            local_fragment: None,
            cross_origin: None,
            integrity: None,
            referrer_policy: None,
            referrer: None,
        };
        let mut any = false;
        while !input.is_exhausted() {
            let name = input.expect_function()?.clone();
            if name.eq_ignore_ascii_case("cross-origin") {
                if modifiers.cross_origin.is_some() {
                    return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
                }
                modifiers.cross_origin = Some(input.parse_nested_block(|input| {
                    let ident = input.expect_ident()?.clone();
                    if ident.eq_ignore_ascii_case("anonymous") {
                        Ok(CrossOriginModifier::Anonymous)
                    } else if ident.eq_ignore_ascii_case("use-credentials") {
                        Ok(CrossOriginModifier::UseCredentials)
                    } else {
                        Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError))
                    }
                })?);
            } else if name.eq_ignore_ascii_case("integrity") {
                if modifiers.integrity.is_some() {
                    return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
                }
                modifiers.integrity = Some(
                    input.parse_nested_block(|input| Ok(input.expect_string()?.as_ref().to_owned()))?,
                );
            } else if name.eq_ignore_ascii_case("referrer-policy") {
                if modifiers.referrer_policy.is_some() {
                    return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
                }
                modifiers.referrer_policy = Some(input.parse_nested_block(|input| {
                    let ident = input.expect_ident()?.clone();
                    match ReferrerPolicyModifier::ALL
                        .into_iter()
                        .find(|policy| ident.eq_ignore_ascii_case(policy.as_str()))
                    {
                        Some(policy) => Ok(policy),
                        None => Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError)),
                    }
                })?);
            } else {
                return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
            }
            any = true;
        }
        if !any {
            return Ok(None);
        }
        modifiers.referrer = Some(context.url_data.0.clone());
        Ok(Some(modifiers))
    }
}

impl style_traits::SpecifiedValueInfo for RequestUrlModifiers {}

impl ToCss for RequestUrlModifiers {
    /// Each modifier given, after a space, in the canonical order (CSS Values 5 § 4.5.4).
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        if let Some(cross_origin) = self.cross_origin {
            dest.write_str(" cross-origin(")?;
            dest.write_str(cross_origin.as_str())?;
            dest.write_char(')')?;
        }
        if let Some(ref integrity) = self.integrity {
            dest.write_str(" integrity(")?;
            integrity.to_css(dest)?;
            dest.write_char(')')?;
        }
        if let Some(policy) = self.referrer_policy {
            dest.write_str(" referrer-policy(")?;
            dest.write_str(policy.as_str())?;
            dest.write_char(')')?;
        }
        Ok(())
    }
}
