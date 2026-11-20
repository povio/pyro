/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Servo-specific logic for CSS url() values.

use crate::derives::*;
use crate::parser::ParserContext;
use crate::stylesheets::CorsMode;
use crate::values::computed::{Context, ToComputedValue};
use servo_arc::Arc;
use std::fmt::{self, Write};
use std::ops::Deref;
use style_traits::{CssWriter, ToCss};
use to_shmem::{SharedMemoryBuilder, ToShmem};
use url::Url;

/// A CSS url() value for servo.
///
/// Servo eagerly resolves SpecifiedUrls, which it can then take advantage of
/// when computing values. In contrast, Gecko uses a different URL backend, so
/// eagerly resolving with rust-url would be duplicated work.
///
/// However, this approach is still not necessarily optimal: See
/// <https://bugzilla.mozilla.org/show_bug.cgi?id=1347435#c6>
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize, SpecifiedValueInfo)]
#[css(function = "url")]
#[repr(C)]
pub struct CssUrl(#[ignore_malloc_size_of = "Arc"] pub Arc<CssUrlData>);

/// Data shared between CssUrls.
///
#[derive(Debug, Deserialize, MallocSizeOf, Serialize, SpecifiedValueInfo)]
#[repr(C)]
pub struct CssUrlData {
    /// The original URI. This might be optional since we may insert computed
    /// values of images into the cascade directly, and we don't bother to
    /// convert their serialization.
    ///
    /// Refcounted since cloning this should be cheap and data: uris can be
    /// really large.
    #[ignore_malloc_size_of = "Arc"]
    original: Option<Arc<String>>,

    /// The resolved value for the url, if valid.
    #[ignore_malloc_size_of = "Arc"]
    resolved: Option<Arc<Url>>,

    /// pyro: the request URL modifiers written in the `url()` (CSS Values 5 § 4.5.4), if any.
    #[ignore_malloc_size_of = "Arc"]
    modifiers: Option<Arc<super::RequestUrlModifiers>>,
}

impl ToShmem for CssUrl {
    fn to_shmem(&self, _builder: &mut SharedMemoryBuilder) -> to_shmem::Result<Self> {
        unimplemented!(
            "If servo wants to share stylesheets across processes, ToShmem for Url must be implemented"
        );
    }
}

impl Deref for CssUrl {
    type Target = CssUrlData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl CssUrl {
    /// Try to parse a URL from a string value that is a valid CSS token for a
    /// URL.
    ///
    /// FIXME(emilio): Should honor CorsMode.
    pub(super) fn new_from_string(
        url: String,
        context: &ParserContext,
        _cors_mode: CorsMode,
    ) -> Self {
        let serialization = Arc::new(url);
        // Per https://drafts.csswg.org/css-values-4/#url-empty
        // If the original url is empty, then the resolved url is considered invalid.
        let resolved = (!serialization.is_empty())
            .then(|| context.url_data.0.join(&serialization))
            .and_then(Result::ok)
            .map(Arc::new);
        // pyro: every `url()` remembers the style sheet it was written in, its request's referrer
        // (Blink `CSSImageValue::PrepareFetch`; the fetch decides for inline style), with no
        // modifier of its own.
        let modifiers = resolved.is_some().then(|| {
            Arc::new(super::RequestUrlModifiers {
                cross_origin: None,
                integrity: None,
                referrer_policy: None,
                referrer: Some(context.url_data.0.clone()),
            })
        });
        CssUrl(Arc::new(CssUrlData {
            original: Some(serialization),
            resolved: resolved,
            modifiers,
        }))
    }

    /// pyro: this URL with the request URL modifiers `modifiers`.
    pub(super) fn with_modifiers(self, modifiers: super::RequestUrlModifiers) -> Self {
        CssUrl(Arc::new(CssUrlData {
            original: self.0.original.clone(),
            resolved: self.0.resolved.clone(),
            modifiers: Some(Arc::new(modifiers)),
        }))
    }

    /// pyro: the request URL modifiers written in the `url()`, if any.
    pub fn modifiers(&self) -> Option<&Arc<super::RequestUrlModifiers>> {
        self.modifiers.as_ref()
    }

    /// Returns true if the URL is definitely invalid. For Servo URLs, we can
    /// use its |resolved| status.
    pub fn is_invalid(&self) -> bool {
        self.resolved.is_none()
    }

    /// Returns true if this URL looks like a fragment.
    /// See https://drafts.csswg.org/css-values/#local-urls
    ///
    /// Since Servo currently stores resolved URLs, this is hard to implement. We
    /// either need to change servo to lazily resolve (like Gecko), or note this
    /// information in the tokenizer.
    pub fn is_fragment(&self) -> bool {
        error!("Can't determine whether the url is a fragment.");
        false
    }

    /// Returns the resolved url if it was valid.
    pub fn url(&self) -> Option<&Arc<Url>> {
        self.resolved.as_ref()
    }

    /// Return the resolved url as string, or the empty string if it's invalid.
    ///
    /// TODO(emilio): Should we return the original one if needed?
    pub fn as_str(&self) -> &str {
        match self.resolved {
            Some(ref url) => url.as_str(),
            None => "",
        }
    }

    /// Creates an already specified url value from an already resolved URL
    /// for insertion in the cascade.
    pub fn for_cascade(url: Arc<::url::Url>) -> Self {
        CssUrl(Arc::new(CssUrlData {
            original: None,
            resolved: Some(url),
            modifiers: None,
        }))
    }

    /// Gets a new url from a string for unit tests.
    pub fn new_for_testing(url: &str) -> Self {
        CssUrl(Arc::new(CssUrlData {
            original: Some(Arc::new(url.into())),
            resolved: (!url.is_empty())
                .then(|| ::url::Url::parse(url))
                .and_then(Result::ok)
                .map(Arc::new),
            modifiers: None,
        }))
    }
}

impl PartialEq for CssUrl {
    fn eq(&self, other: &Self) -> bool {
        // TODO(emilio): maybe we care about equality of the specified values if
        // present? Seems not.
        self.resolved == other.resolved && self.modifiers == other.modifiers
    }
}

impl Eq for CssUrl {}

impl ToCss for CssUrl {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        let string = match self.0.original {
            Some(ref original) => &**original,
            None => match self.resolved {
                Some(ref url) => url.as_str(),
                // This can only happen if the url wasn't specified by the
                // user *and* it's an invalid url that has been transformed
                // back to specified value via the "uncompute" functionality.
                None => "about:invalid",
            },
        };

        dest.write_str("url(")?;
        string.to_css(dest)?;
        if let Some(ref modifiers) = self.0.modifiers {
            modifiers.to_css(dest)?;
        }
        dest.write_char(')')
    }
}

/// A specified url() value for servo.
pub type SpecifiedUrl = CssUrl;

impl ToComputedValue for SpecifiedUrl {
    type ComputedValue = ComputedUrl;

    // If we can't resolve the URL from the specified one, we fall back to the original
    // but still return it as a ComputedUrl::Invalid
    fn to_computed_value(&self, _: &Context) -> Self::ComputedValue {
        match self.resolved {
            Some(ref url) => ComputedUrl::Valid(url.clone(), self.modifiers.clone()),
            None => match self.original {
                Some(ref url) => ComputedUrl::Invalid(url.clone()),
                None => {
                    unreachable!("Found specified url with neither resolved or original URI!");
                },
            },
        }
    }

    fn from_computed_value(computed: &ComputedUrl) -> Self {
        let data = match *computed {
            ComputedUrl::Valid(ref url, ref modifiers) => CssUrlData {
                original: None,
                resolved: Some(url.clone()),
                modifiers: modifiers.clone(),
            },
            ComputedUrl::Invalid(ref url) => CssUrlData {
                original: Some(url.clone()),
                resolved: None,
                modifiers: None,
            },
        };
        CssUrl(Arc::new(data))
    }
}

/// The computed value of a CSS `url()`, resolved relative to the stylesheet URL.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ComputedUrl {
    /// The `url()` was invalid or it wasn't specified by the user.
    Invalid(#[ignore_malloc_size_of = "Arc"] Arc<String>),
    /// The resolved `url()` relative to the stylesheet URL, and (pyro) its request URL
    /// modifiers, if any (CSS Values 5 § 4.5.4).
    Valid(
        #[ignore_malloc_size_of = "Arc"] Arc<Url>,
        #[ignore_malloc_size_of = "Arc"] Option<Arc<super::RequestUrlModifiers>>,
    ),
}

impl ComputedUrl {
    /// Returns the resolved url if it was valid.
    pub fn url(&self) -> Option<&Arc<Url>> {
        match *self {
            ComputedUrl::Valid(ref url, _) => Some(url),
            _ => None,
        }
    }

    /// pyro: the request URL modifiers of a valid `url()`, if any.
    pub fn modifiers(&self) -> Option<&Arc<super::RequestUrlModifiers>> {
        match *self {
            ComputedUrl::Valid(_, ref modifiers) => modifiers.as_ref(),
            _ => None,
        }
    }
}

impl ToCss for ComputedUrl {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        let string = match *self {
            ComputedUrl::Valid(ref url, _) => url.as_str(),
            ComputedUrl::Invalid(ref invalid_string) => invalid_string,
        };

        dest.write_str("url(")?;
        string.to_css(dest)?;
        if let Some(modifiers) = self.modifiers() {
            modifiers.to_css(dest)?;
        }
        dest.write_char(')')
    }
}
