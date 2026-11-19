use blitz_traits::node_id::NodeId;
use selectors::context::QuirksMode;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering as Ao;
use std::{
    io::Cursor,
    sync::{Arc, atomic::AtomicUsize, mpsc::Sender},
};
use style::{
    font_face::{
        FontFaceSourceFormat, FontFaceSourceFormatKeyword, FontStyleRange, Source, SourceList,
    },
    media_queries::MediaList,
    servo_arc::Arc as ServoArc,
    shared_lock::SharedRwLock,
    shared_lock::{Locked, SharedRwLockReadGuard},
    stylesheets::{
        CssRule, DocumentStyleSheet, ImportRule, Origin, Stylesheet, StylesheetInDocument,
        StylesheetLoader as ServoStylesheetLoader, UrlExtraData,
        import_rule::{ImportLayer, ImportSheet, ImportSupportsCondition},
    },
    values::{CssUrl, SourceLocation},
};

use blitz_traits::net::{AbortSignal, Bytes, NetHandler, NetProvider, Request};
use blitz_traits::shell::ShellProvider;

use url::Url;

use crate::{document::DocumentEvent, util::ImageType};

pub(crate) fn stamped_request(url: Url, signal: Option<&AbortSignal>) -> Request {
    let mut req = Request::get(url);
    if let Some(sig) = signal {
        req = req.signal(sig.clone());
    }
    req
}

/// pyro: marks `request` to be fetched in CORS mode with credentials only to the same origin
/// ("anonymous", as `shape-outside` images load): a cross-origin response the CORS check does not
/// pass is a network error. A provider that cannot make such a request must fail it.
pub fn mark_cors(request: &mut Request) {
    request.options.cors = Some(blitz_traits::net::CorsCredentials::SameOrigin);
}

/// pyro: whether `request` is to be fetched in CORS mode ([`mark_cors`], `cross-origin()`).
pub fn is_cors_request(request: &Request) -> bool {
    request.options.cors.is_some()
}

/// pyro: what a `url()`'s request URL modifiers ask of its request (CSS Values 5 § 4.5.4):
/// `cross-origin()` its mode and credentials, `integrity()` its integrity metadata,
/// `referrer-policy()` its referrer policy; with any of them, the style sheet the `url()` was
/// written in is its referrer.
pub fn url_fetch_options(modifiers: Option<&style::url::RequestUrlModifiers>) -> blitz_traits::net::FetchOptions {
    use blitz_traits::net::{CorsCredentials, FetchOptions};
    use style::url::CrossOriginModifier;
    let Some(modifiers) = modifiers else { return FetchOptions::default() };
    FetchOptions {
        cors: modifiers.cross_origin.map(|mode| match mode {
            CrossOriginModifier::Anonymous => CorsCredentials::SameOrigin,
            CrossOriginModifier::UseCredentials => CorsCredentials::Include,
        }),
        integrity: modifiers.integrity.clone(),
        referrer_policy: modifiers.referrer_policy.map(|policy| policy.as_str().to_owned()),
        referrer: modifiers.referrer.as_ref().map(|url| (**url).clone()),
    }
}

/// pyro: whether a CORS fetch of `url` from the document at `document` is checked (and so may
/// differ from a fetch without CORS): a cross-origin URL, not `data:` (whose response is the same).
pub(crate) fn cors_checked(url: &Url, document: &Url) -> bool {
    url.scheme() != "data" && url.origin() != document.origin()
}

/// pyro: the key of an image fetched in CORS mode ([`mark_cors`], a `shape-outside` image) in
/// the document's image cache and pending loads: a response fetched without CORS (an `<img>`'s)
/// never stands in for it, nor it for one. (A space is never in a serialized URL.)
pub(crate) fn cors_image_key(url: &str) -> String {
    format!("cors-anonymous {url}")
}

/// Carries `@font-face` descriptors from CSS parsing through to font
/// registration so `parley::Collection::register_fonts` can alias the bytes
/// under the `font-family` declared in CSS rather than whatever family name
/// the TTF's own `name` table reports.
///
/// All fields are `Option` because each descriptor is independently optional
/// at the CSS level. Missing fields fall back to the values parley reads
/// from the font's own metadata.
#[derive(Clone, Debug, Default)]
pub struct FontFaceOverrides {
    /// `font-family` descriptor (the alias the rest of the stylesheet uses).
    pub family_name: Option<String>,
    /// `font-weight` descriptor as a single CSS weight (100–900). Stylo
    /// parses this as a range; we record the lower bound, which equals the
    /// upper bound in the common single-value case.
    pub weight: Option<f32>,
    /// pyro: `font-stretch` (`font-width`) descriptor in percent (a range's lower bound): faces
    /// of a family that differ in it only were all `normal` to font matching.
    pub width: Option<f32>,
    /// `font-style` descriptor mapped to fontique's `FontStyle`.
    pub style: Option<parley::fontique::FontStyle>,
    /// pyro: the slant angles the `font-style` descriptor allows a variable font's `slnt`
    /// axis: `(0, 0)` for `normal`, the range of `oblique <angle> <angle>`.
    pub slant_range: Option<(f32, f32)>,
    /// pyro: the `font-weight` descriptor's range (either order; a single weight is a range of
    /// one): font matching takes its weight nearest the one asked for, a `wght` axis is held to it.
    pub weight_range: Option<(f32, f32)>,
    /// pyro: the `font-stretch` (`font-width`) descriptor's range in percentages, as `weight_range`.
    pub width_range: Option<(f32, f32)>,
    /// pyro: the `size-adjust`, `ascent-override`, `descent-override` and `line-gap-override`
    /// descriptors, `None` when the rule has none of them.
    pub face_metrics: Option<parley::fontique::FaceMetrics>,
    /// pyro: the `font-feature-settings` and `font-variation-settings` descriptors, interned
    /// (`parley::fontique::FaceSettings::intern`), `None` when the rule has neither.
    pub face_settings: Option<u32>,
    /// `unicode-range` descriptor: the code points the face is used for.
    /// `None` (no descriptor) is every code point.
    pub unicode_range: Option<parley::fontique::UnicodeRange>,
    /// The rule's place among the `@font-face` rules of the process, rising
    /// in the order they are read: the face's precedence among the faces of a
    /// family that match a style equally. CSS checks those faces (a
    /// composite font, such as the per-subset faces of a Google Fonts family)
    /// last rule first, whichever face finished loading first.
    pub precedence: u64,
}

#[derive(Clone, Debug)]
pub enum Resource {
    /// pyro: the last field is the EXIF orientation (1 to 8) the pixels were turned by.
    Image(ImageType, u32, u32, Arc<Vec<u8>>, u8),
    #[cfg(feature = "svg")]
    Svg(ImageType, crate::node::SvgImageData),
    /// The sheet, and its text when it has `@page` rules (see `BaseDocument::linked_page_css`).
    Css(DocumentStyleSheet, Option<String>),
    /// Stylesheet loaded for an `@import` rule, to be attached to the rule on the document thread
    ImportedCss(ServoArc<Locked<ImportRule>>, ServoArc<Stylesheet>),
    Font(Bytes, FontFaceOverrides),
    /// HTML fetched for an `<iframe>` element's `src`
    DocumentSrc(String),
    None,
}

pub(crate) struct ResourceHandler<T: Send + Sync + 'static> {
    doc_id: usize,
    request_id: usize,
    node_id: Option<NodeId>,
    tx: Sender<DocumentEvent>,
    shell_provider: Arc<dyn ShellProvider>,
    data: T,
}

impl<T: Send + Sync + 'static> ResourceHandler<T> {
    pub(crate) fn new(
        tx: Sender<DocumentEvent>,
        doc_id: usize,
        node_id: Option<NodeId>,
        shell_provider: Arc<dyn ShellProvider>,
        data: T,
    ) -> Self {
        static REQUEST_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);
        Self {
            request_id: REQUEST_ID_COUNTER.fetch_add(1, Ao::Relaxed),
            doc_id,
            node_id,
            tx,
            shell_provider,
            data,
        }
    }

    pub(crate) fn boxed(
        tx: Sender<DocumentEvent>,
        doc_id: usize,
        node_id: Option<NodeId>,
        shell_provider: Arc<dyn ShellProvider>,
        data: T,
    ) -> Box<dyn NetHandler>
    where
        ResourceHandler<T>: NetHandler,
    {
        Box::new(Self::new(tx, doc_id, node_id, shell_provider, data)) as _
    }

    pub(crate) fn request_id(&self) -> usize {
        self.request_id
    }

    fn respond(&self, resolved_url: String, result: Result<Resource, String>) {
        let response = ResourceLoadResponse {
            request_id: self.request_id,
            node_id: self.node_id,
            resolved_url: Some(resolved_url),
            result,
        };
        let _ = self.tx.send(DocumentEvent::ResourceLoad(response));
        self.shell_provider.request_redraw();
    }
}

#[allow(unused)]
pub struct ResourceLoadResponse {
    pub request_id: usize,
    pub node_id: Option<NodeId>,
    pub resolved_url: Option<String>,
    pub result: Result<Resource, String>,
}

pub struct StylesheetHandler {
    pub source_url: Url,
    pub guard: SharedRwLock,
    pub net_provider: Arc<dyn NetProvider>,
    pub abort_signal: Option<AbortSignal>,
}

impl NetHandler for ResourceHandler<StylesheetHandler> {
    fn error(self: Box<Self>, url: String, error: String) {
        self.respond(url, Err(error));
    }

    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        // NOTE(Nico): I don't *think* external stylesheets should have HTML entities escaped
        // let escaped_css = html_escape::decode_html_entities(css);

        let sheet = Stylesheet::from_bytes(
            &bytes,
            self.data.source_url.clone().into(),
            None,
            None,
            Origin::Author,
            ServoArc::new(self.data.guard.wrap(MediaList::empty())),
            self.data.guard.clone(),
            Some(&StylesheetLoader {
                tx: self.tx.clone(),
                doc_id: self.doc_id,
                net_provider: self.data.net_provider.clone(),
                shell_provider: self.shell_provider.clone(),
                abort_signal: self.data.abort_signal.clone(),
                import_depth: 0,
            }),
            None, // error_reporter
            QuirksMode::NoQuirks,
        );

        self.respond(
            resolved_url,
            Ok(Resource::Css(
                DocumentStyleSheet(ServoArc::new(sheet)),
                linked_page_css(&bytes),
            )),
        );
    }
}

/// pyro: the text of a linked sheet with `@page` rules, kept for printing (the servo build
/// of Stylo parses neither `size` nor `page`).
fn linked_page_css(bytes: &[u8]) -> Option<String> {
    let css = String::from_utf8_lossy(bytes);
    css.contains("@page")
        .then(|| css.trim_start_matches('\u{feff}').to_owned())
}

/// Maximum depth of nested `@import` rules. Imports nested deeper than this
/// are refused, preventing unbounded recursion (e.g. a stylesheet whose
/// resolved `@import` URL yields another stylesheet importing in turn, with
/// the URL growing geometrically at each level).
const MAX_IMPORT_DEPTH: usize = 16;

#[derive(Clone)]
pub(crate) struct StylesheetLoader {
    pub(crate) tx: Sender<DocumentEvent>,
    pub(crate) doc_id: usize,
    pub(crate) net_provider: Arc<dyn NetProvider>,
    pub(crate) shell_provider: Arc<dyn ShellProvider>,
    pub(crate) abort_signal: Option<AbortSignal>,
    /// Depth of `@import` nesting for the stylesheet this loader is parsing
    /// (0 for a top-level stylesheet)
    pub(crate) import_depth: usize,
}
impl ServoStylesheetLoader for StylesheetLoader {
    fn request_stylesheet(
        &self,
        url: CssUrl,
        location: SourceLocation,
        lock: &SharedRwLock,
        media: ServoArc<Locked<MediaList>>,
        supports: Option<ImportSupportsCondition>,
        layer: ImportLayer,
    ) -> ServoArc<Locked<ImportRule>> {
        if self.import_depth >= MAX_IMPORT_DEPTH || !supports.as_ref().is_none_or(|s| s.enabled) {
            return ServoArc::new(lock.wrap(ImportRule {
                url,
                stylesheet: ImportSheet::new_refused(),
                supports,
                layer,
                source_location: location,
            }));
        }

        let import = ImportRule {
            url,
            stylesheet: ImportSheet::new_pending(),
            supports,
            layer,
            source_location: location,
        };

        let url = import.url.url().unwrap().clone();
        // pyro: fetched as the `url()`'s request URL modifiers say (CSS Values 5 § 4.5.4).
        let mut request = stamped_request(url.as_ref().clone(), self.abort_signal.as_ref());
        request.options = url_fetch_options(import.url.modifiers().map(|m| &**m));
        let import = ServoArc::new(lock.wrap(import));
        self.net_provider.fetch(
            self.doc_id,
            request,
            ResourceHandler::boxed(
                self.tx.clone(),
                self.doc_id,
                None, // node_id
                self.shell_provider.clone(),
                NestedStylesheetHandler {
                    url: url.clone(),
                    loader: StylesheetLoader {
                        import_depth: self.import_depth + 1,
                        ..self.clone()
                    },
                    lock: lock.clone(),
                    media,
                    import_rule: import.clone(),
                    net_provider: self.net_provider.clone(),
                },
            ),
        );

        import
    }
}

struct NestedStylesheetHandler {
    loader: StylesheetLoader,
    lock: SharedRwLock,
    url: ServoArc<Url>,
    media: ServoArc<Locked<MediaList>>,
    import_rule: ServoArc<Locked<ImportRule>>,
    net_provider: Arc<dyn NetProvider>,
}

impl NetHandler for ResourceHandler<NestedStylesheetHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        // NOTE(Nico): I don't *think* external stylesheets should have HTML entities escaped
        // let escaped_css = html_escape::decode_html_entities(css);

        let sheet = ServoArc::new(Stylesheet::from_bytes(
            &bytes,
            UrlExtraData(self.data.url.clone()),
            None,
            None,
            Origin::Author,
            self.data.media.clone(),
            self.data.lock.clone(),
            Some(&self.data.loader),
            None, // error_reporter
            QuirksMode::NoQuirks,
        ));

        // Fetch @font-face fonts
        fetch_font_face(
            self.tx.clone(),
            self.doc_id,
            self.node_id,
            &sheet,
            &self.data.net_provider,
            &self.shell_provider,
            &self.data.lock.read(),
            self.data.loader.abort_signal.as_ref(),
            None,
        );

        let import_rule = self.data.import_rule.clone();
        self.respond(resolved_url, Ok(Resource::ImportedCss(import_rule, sheet)))
    }
}

struct FontFaceHandler {
    format: FontFaceSourceFormatKeyword,
    overrides: FontFaceOverrides,
}
impl NetHandler for ResourceHandler<FontFaceHandler> {
    fn bytes(mut self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let result = self.data.parse(bytes);
        self.respond(resolved_url, result)
    }
}
impl FontFaceHandler {
    fn parse(&mut self, bytes: Bytes) -> Result<Resource, String> {
        if self.format == FontFaceSourceFormatKeyword::None && bytes.len() >= 4 {
            self.format = match &bytes.as_ref()[0..4] {
                // WOFF (v1) files begin with 0x774F4646 ('wOFF' in ascii)
                // See: <https://w3c.github.io/woff/woff1/spec/Overview.html#WOFFHeader>
                #[cfg(feature = "woff")]
                b"wOFF" => FontFaceSourceFormatKeyword::Woff,
                // WOFF2 files begin with 0x774F4632 ('wOF2' in ascii)
                // See: <https://w3c.github.io/woff/woff2/#woff20Header>
                #[cfg(feature = "woff")]
                b"wOF2" => FontFaceSourceFormatKeyword::Woff2,
                // Opentype fonts with CFF data begin with 0x4F54544F ('OTTO' in ascii)
                // See: <https://learn.microsoft.com/en-us/typography/opentype/spec/otff#organization-of-an-opentype-font>
                b"OTTO" => FontFaceSourceFormatKeyword::Opentype,
                // Opentype fonts truetype outlines begin with 0x00010000
                // See: <https://learn.microsoft.com/en-us/typography/opentype/spec/otff#organization-of-an-opentype-font>
                &[0x00, 0x01, 0x00, 0x00] => FontFaceSourceFormatKeyword::Truetype,
                // Truetype fonts begin with 0x74727565 ('true' in ascii)
                // See: <https://developer.apple.com/fonts/TrueType-Reference-Manual/RM06/Chap6.html#ScalerTypeNote>
                b"true" => FontFaceSourceFormatKeyword::Truetype,
                _ => FontFaceSourceFormatKeyword::None,
            }
        }

        // Satisfy rustc's mutability linting with woff feature both enabled/disabled
        #[cfg(feature = "woff")]
        let mut bytes = bytes;

        match self.format {
            #[cfg(feature = "woff")]
            FontFaceSourceFormatKeyword::Woff => {
                #[cfg(feature = "tracing")]
                tracing::info!("Decompressing woff1 font");

                // Use wuff crate to decompress font
                let decompressed = wuff::decompress_woff1(&bytes).ok();

                if let Some(decompressed) = decompressed {
                    bytes = Bytes::from(decompressed);
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!("Failed to decompress woff1 font");
                }
            }
            #[cfg(feature = "woff")]
            FontFaceSourceFormatKeyword::Woff2 => {
                #[cfg(feature = "tracing")]
                tracing::info!("Decompressing woff2 font");

                // Use wuff crate to decompress font
                let decompressed = wuff::decompress_woff2(&bytes).ok();

                if let Some(decompressed) = decompressed {
                    bytes = Bytes::from(decompressed);
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!("Failed to decompress woff2 font");
                }
            }
            FontFaceSourceFormatKeyword::None => {
                // Should this be an error?
                return Ok(Resource::None);
            }
            _ => {}
        }

        Ok(Resource::Font(bytes, std::mem::take(&mut self.overrides)))
    }
}

/// Fetch the fonts of all `@font-face` rules in `sheet`
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_font_face(
    tx: Sender<DocumentEvent>,
    doc_id: usize,
    node_id: Option<NodeId>,
    sheet: &Stylesheet,
    network_provider: &Arc<dyn NetProvider>,
    shell_provider: &Arc<dyn ShellProvider>,
    read_guard: &SharedRwLockReadGuard,
    abort_signal: Option<&AbortSignal>,
    device: Option<&style::device::Device>,
) {
    fetch_font_face_rules(
        sheet.contents(read_guard).rules(read_guard).iter(),
        tx,
        doc_id,
        node_id,
        network_provider,
        shell_provider,
        read_guard,
        abort_signal,
        device,
    )
}

/// Fetch the fonts of the `@font-face` rules among `rules`
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_font_face_rules<'a>(
    rules: impl Iterator<Item = &'a CssRule>,
    tx: Sender<DocumentEvent>,
    doc_id: usize,
    node_id: Option<NodeId>,
    network_provider: &Arc<dyn NetProvider>,
    shell_provider: &Arc<dyn ShellProvider>,
    read_guard: &'a SharedRwLockReadGuard,
    abort_signal: Option<&AbortSignal>,
    device: Option<&style::device::Device>,
) {
    // pyro: `@font-face` rules nested in conditional group rules (`@media`, `@supports`) and
    // `@layer` blocks count too (css-conditional at-media-content-002 / at-supports-content-002).
    // A `@supports` block whose condition fails is skipped, and so is a `@media` block whose
    // list does not match the device (when there is one: imported sheets load without it).
    fn flatten<'a>(
        rules: impl Iterator<Item = &'a CssRule>,
        guard: &'a SharedRwLockReadGuard,
        device: Option<&style::device::Device>,
        out: &mut Vec<&'a CssRule>,
    ) {
        for rule in rules {
            match rule {
                CssRule::Media(media) => {
                    let matches = device.is_none_or(|device| {
                        media.media_queries.read_with(guard).evaluate(
                            device,
                            QuirksMode::NoQuirks,
                            &mut style::stylesheets::CustomMediaEvaluator::none(),
                        )
                    });
                    if matches {
                        flatten(media.rules.read_with(guard).0.iter(), guard, device, out)
                    }
                }
                CssRule::Supports(supports) if supports.enabled => {
                    flatten(supports.rules.read_with(guard).0.iter(), guard, device, out)
                }
                CssRule::LayerBlock(layer) => flatten(layer.rules.read_with(guard).0.iter(), guard, device, out),
                _ => out.push(rule),
            }
        }
    }
    let mut flat = Vec::new();
    flatten(rules, read_guard, device, &mut flat);
    flat.into_iter()
        .filter_map(|rule| match rule {
            CssRule::FontFace(font_face) => {
                let descriptor = &font_face.read_with(read_guard).descriptors;
                let family = descriptor.font_family.as_ref()?;
                let src = descriptor.src.as_ref()?;
                // Capture the @font-face descriptors so parley can register
                // the font under the CSS-declared family name (and weight /
                // style) rather than whatever metadata the TTF reports.
                // Rules are read in order within a sheet; sheets in the order they load.
                static RULE_ORDER: AtomicU64 = AtomicU64::new(1);
                let overrides = FontFaceOverrides {
                    family_name: Some(family.name.to_string()),
                    // pyro: a reversed range (`900 100`) is the same range.
                    weight: descriptor.font_weight.as_ref().and_then(|range| {
                        let (a, b) = (range.0.compute()?.value(), range.1.compute()?.value());
                        Some(a.min(b))
                    }),
                    weight_range: descriptor
                        .font_weight
                        .as_ref()
                        .and_then(|range| Some((range.0.compute()?.value(), range.1.compute()?.value()))),
                    width_range: descriptor
                        .font_width
                        .as_ref()
                        .and_then(|range| range.compute())
                        .map(|range| (range.0.0.to_float(), range.1.0.to_float())),
                    width: descriptor
                        .font_width
                        .as_ref()
                        .and_then(|range| range.compute())
                        .map(|range| range.0.0.to_float().min(range.1.0.to_float())),
                    style: descriptor.font_style.as_ref().map(stylo_to_fontique_style),
                    slant_range: descriptor.font_style.as_ref().and_then(|style| match style {
                        FontStyleRange::Italic => None,
                        FontStyleRange::Oblique(min, max) => {
                            Some((min.degrees().unwrap_or(0.0), max.degrees().unwrap_or(0.0)))
                        }
                    }),
                    unicode_range: descriptor.unicode_range.as_deref().map(unicode_range),
                    precedence: RULE_ORDER.fetch_add(1, Ao::Relaxed),
                    face_metrics: {
                        // `normal` (no override) computes to -1.
                        let metric = |value: &Option<style::values::specified::font::MetricsOverride>| {
                            value.as_ref().and_then(|value| value.compute()).map(|p| p.0).filter(|v| *v >= 0.0)
                        };
                        let face = parley::fontique::FaceMetrics {
                            size_adjust: descriptor
                                .size_adjust
                                .as_ref()
                                .and_then(|value| value.compute())
                                .map(|p| p.0)
                                .filter(|v| *v >= 0.0 && *v != 1.0),
                            ascent: metric(&descriptor.ascent_override),
                            descent: metric(&descriptor.descent_override),
                            line_gap: metric(&descriptor.line_gap_override),
                        };
                        (face != parley::fontique::FaceMetrics::default()).then_some(face)
                    },
                    face_settings: {
                        let features = descriptor.font_feature_settings.as_ref().map_or(Vec::new(), |settings| {
                            settings
                                .0
                                .iter()
                                .filter_map(|f| Some((f.tag.0.to_be_bytes(), u16::try_from(f.value.resolve()?).ok()?)))
                                .collect()
                        });
                        let variations = descriptor.font_variation_settings.as_ref().map_or(Vec::new(), |settings| {
                            settings.0.iter().filter_map(|v| Some((v.tag.0.to_be_bytes(), v.value.resolve()?))).collect()
                        });
                        let id = parley::fontique::FaceSettings { features, variations }.intern();
                        (id != 0).then_some(id)
                    },
                };
                Some((src, overrides))
            }
            _ => None,
        })
        .for_each(|(source_list, overrides)| {
            if let Some((url, format)) = preferred_font_source(source_list) {
                network_provider.fetch(
                    doc_id,
                    stamped_request(url, abort_signal),
                    ResourceHandler::boxed(
                        tx.clone(),
                        doc_id,
                        node_id,
                        shell_provider.clone(),
                        FontFaceHandler { format, overrides },
                    ),
                );
            }
        })
}

/// The code points of an `@font-face` `unicode-range` descriptor.
fn unicode_range(ranges: &[cssparser::UnicodeRange]) -> parley::fontique::UnicodeRange {
    parley::fontique::UnicodeRange::new(ranges.iter().map(|range| range.start..=range.end))
}

/// The first source in an `@font-face` `src` list that is in a format we
/// support, with its resolved url and format.
fn preferred_font_source(source_list: &SourceList) -> Option<(Url, FontFaceSourceFormatKeyword)> {
    source_list.0.iter().find_map(|source| {
        let url_source = match source {
            Source::Url(url_source) => url_source,
            // TODO: support local fonts in @font-face
            Source::Local(_) => return None,
        };

        // A relative url with no base url to resolve against
        // yields None; skip the source instead of panicking
        let Some(url) = url_source.url.url() else {
            #[cfg(feature = "tracing")]
            tracing::warn!("Skipping @font-face source with unresolvable url");
            return None;
        };

        // pyro: a `tech()` source needing a technology we do not render is skipped.
        if !style::font_face::FontFaceSourceTechFlags::SERVO_SUPPORTED.contains(url_source.tech_flags) {
            return None;
        }

        let format = match &url_source.format_hint {
            Some(FontFaceSourceFormat::Keyword(keyword)) => *keyword,
            Some(FontFaceSourceFormat::String(string)) => font_format_from_string(string),
            None => font_format_from_extension(url),
        };

        let supported = match format {
            FontFaceSourceFormatKeyword::None
            | FontFaceSourceFormatKeyword::Collection
            | FontFaceSourceFormatKeyword::Opentype
            | FontFaceSourceFormatKeyword::Truetype => true,
            FontFaceSourceFormatKeyword::Woff | FontFaceSourceFormatKeyword::Woff2 => {
                cfg!(feature = "woff")
            }
            FontFaceSourceFormatKeyword::EmbeddedOpentype
            | FontFaceSourceFormatKeyword::Svg
            | FontFaceSourceFormatKeyword::Unknown => false,
        };
        if !supported {
            #[cfg(feature = "tracing")]
            tracing::warn!("Skipping unsupported font of type {:?}", format);
            return None;
        }

        Some((url.as_ref().clone(), format))
    })
}

/// Map a `format("...")` string hint to a format. Besides the keywords, this
/// accepts the legacy `-variations` strings and the `ttf`/`otf` extensions.
fn font_format_from_string(format: &str) -> FontFaceSourceFormatKeyword {
    const FORMATS: &[(&str, FontFaceSourceFormatKeyword)] = &[
        ("collection", FontFaceSourceFormatKeyword::Collection),
        (
            "embedded-opentype",
            FontFaceSourceFormatKeyword::EmbeddedOpentype,
        ),
        ("opentype", FontFaceSourceFormatKeyword::Opentype),
        ("opentype-variations", FontFaceSourceFormatKeyword::Opentype),
        ("otf", FontFaceSourceFormatKeyword::Opentype),
        ("svg", FontFaceSourceFormatKeyword::Svg),
        ("truetype", FontFaceSourceFormatKeyword::Truetype),
        ("truetype-variations", FontFaceSourceFormatKeyword::Truetype),
        ("ttf", FontFaceSourceFormatKeyword::Truetype),
        ("woff", FontFaceSourceFormatKeyword::Woff),
        ("woff-variations", FontFaceSourceFormatKeyword::Woff),
        ("woff2", FontFaceSourceFormatKeyword::Woff2),
        ("woff2-variations", FontFaceSourceFormatKeyword::Woff2),
    ];
    FORMATS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(format))
        .map_or(FontFaceSourceFormatKeyword::Unknown, |(_, format)| *format)
}

/// Guess a font's format from the file extension of its url's path. Urls
/// with no known extension, such as `data:` urls, get
/// `FontFaceSourceFormatKeyword::None` and are sniffed from their bytes once
/// fetched.
fn font_format_from_extension(url: &Url) -> FontFaceSourceFormatKeyword {
    const EXTENSIONS: &[(&str, FontFaceSourceFormatKeyword)] = &[
        ("eot", FontFaceSourceFormatKeyword::EmbeddedOpentype),
        ("otf", FontFaceSourceFormatKeyword::Opentype),
        ("svg", FontFaceSourceFormatKeyword::Svg),
        ("ttc", FontFaceSourceFormatKeyword::Collection),
        ("ttf", FontFaceSourceFormatKeyword::Truetype),
        ("woff", FontFaceSourceFormatKeyword::Woff),
        ("woff2", FontFaceSourceFormatKeyword::Woff2),
    ];
    if url.cannot_be_a_base() {
        return FontFaceSourceFormatKeyword::None;
    }
    let file_name = url.path().rsplit('/').next().unwrap_or_default();
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return FontFaceSourceFormatKeyword::None;
    };
    EXTENSIONS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(extension))
        .map_or(FontFaceSourceFormatKeyword::None, |(_, format)| *format)
}

/// Translate stylo's `@font-face` `font-style` descriptor into the fontique
/// `FontStyle` enum used by parley. Stylo encodes Italic and Oblique-with-
/// angle distinctly; CSS's bare `normal` is parsed as `Oblique(0deg, 0deg)`
/// by stylo (see the `FontStyle::parse` impl in stylo's `font_face.rs`), so
/// that pattern is treated as `Normal` here.
fn stylo_to_fontique_style(style: &FontStyleRange) -> parley::fontique::FontStyle {
    use parley::fontique::FontStyle as Fq;
    match style {
        FontStyleRange::Italic => Fq::Italic,
        FontStyleRange::Oblique(min, max) => {
            let angle = min.degrees();
            // Stylo emits `Oblique(0deg, 0deg)` for the literal CSS `normal`
            // keyword. Map that back to `Normal` so parley's font matching
            // doesn't misclassify upright fonts.
            // pyro: a range that has 0deg in it (`oblique 0deg 15deg`, a variable font's `slnt`
            // axis) is an upright face that can slant (`FontFaceOverrides::slant_range`).
            let (low, high) = (angle.unwrap_or(0.0), max.degrees().unwrap_or(0.0));
            if low.min(high) <= 0.0 && low.max(high) >= 0.0 {
                Fq::Normal
            } else {
                Fq::Oblique(angle)
            }
        }
    }
}

/// Handles HTML fetched for an `<iframe>` element's `src`
pub(crate) struct DocumentSrcHandler;

impl NetHandler for ResourceHandler<DocumentSrcHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let html = String::from_utf8_lossy(&bytes).into_owned();
        self.respond(resolved_url, Ok(Resource::DocumentSrc(html)));
    }
}

pub struct ImageHandler {
    kind: ImageType,
    /// pyro: what the request was made with ([`mark_cors`], [`url_fetch_options`]): an image
    /// fetched with other than the defaults answers under its [`FetchOptions::cache_key`].
    ///
    /// [`FetchOptions::cache_key`]: blitz_traits::net::FetchOptions::cache_key
    options: blitz_traits::net::FetchOptions,
}
impl ImageHandler {
    pub fn new(kind: ImageType) -> Self {
        Self { kind, options: Default::default() }
    }

    /// pyro: fetched in CORS mode ([`mark_cors`]) when `cors` is.
    pub fn cors(mut self, cors: bool) -> Self {
        self.options.cors = cors.then_some(blitz_traits::net::CorsCredentials::SameOrigin);
        self
    }

    /// pyro: fetched with `options` (a `url()`'s request URL modifiers).
    pub fn options(mut self, options: blitz_traits::net::FetchOptions) -> Self {
        self.options = options;
        self
    }
}

impl NetHandler for ResourceHandler<ImageHandler> {
    fn bytes(self: Box<Self>, resolved_url: String, bytes: Bytes) {
        let result = self.data.parse(&resolved_url, bytes);
        // pyro: an image fetched with CORS (or other options) answers under its key.
        let key = self.data.options.cache_key(&resolved_url);
        self.respond(key, result)
    }
}

/// pyro: decoded raster images by encoded bytes, for every document
/// of the process (a template's logo decoded once, not once per document):
/// (encoded bytes, width, height, RGBA8 pixels). Bounded at 64 images and
/// 64 MB of pixels.
type DecodedImage = (Bytes, u32, u32, Arc<Vec<u8>>, u8);
static DECODED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u64, Vec<DecodedImage>>>> =
    std::sync::LazyLock::new(Default::default);

fn decoded_key(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

impl ImageHandler {
    fn parse(&self, url: &str, bytes: Bytes) -> Result<Resource, String> {
        let key = decoded_key(&bytes);
        if let Some((_, w, h, data, orientation)) = DECODED
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|v| v.iter().find(|e| e.0[..] == bytes[..]).cloned())
        {
            return Ok(Resource::Image(self.kind, w, h, data, orientation));
        }
        // pyro: `ImageReader::decode`, keeping the image's ICC profile.
        let decoded = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .expect("IO errors impossible with Cursor")
            .into_decoder()
            .and_then(|mut decoder| {
                image::Limits::default().reserve(image::ImageDecoder::total_bytes(&decoder))?;
                let icc = image::ImageDecoder::icc_profile(&mut decoder).ok().flatten();
                // pyro: the image as its EXIF orientation says to show it (`image-orientation:
                // from-image`, the initial value): a photo taken sideways was drawn sideways.
                let orientation = image::ImageDecoder::orientation(&mut decoder).ok();
                image::DynamicImage::from_decoder(decoder).map(|mut image| {
                    if let Some(orientation) = orientation {
                        image.apply_orientation(orientation);
                    }
                    (image, icc, orientation.map_or(1, |o| o.to_exif()))
                })
            });
        let image_err = match decoded {
            Ok((image, icc, orientation)) => {
                let width = image.width();
                let height = image.height();
                let mut rgba = image.into_rgba8().into_raw();
                // pyro: a tagged image's pixels in sRGB.
                if let Some(icc) = icc {
                    crate::icc::to_srgb(&icc, &mut rgba);
                }
                let raw_rgba8_data = Arc::new(rgba);
                {
                    const MAX_IMAGES: usize = 64;
                    const MAX_BYTES: usize = 64 << 20;
                    let mut cache = DECODED.lock().unwrap();
                    let (count, size) = cache
                        .values()
                        .flatten()
                        .fold((0, 0), |(n, b), e| (n + 1, b + e.3.len()));
                    if count >= MAX_IMAGES || size + raw_rgba8_data.len() > MAX_BYTES {
                        cache.clear();
                    }
                    if raw_rgba8_data.len() <= MAX_BYTES {
                        cache.entry(key).or_default().push((
                            bytes.clone(),
                            width,
                            height,
                            raw_rgba8_data.clone(),
                            orientation,
                        ));
                    }
                }
                return Ok(Resource::Image(self.kind, width, height, raw_rgba8_data, orientation));
            }
            Err(e) => e.to_string(),
        };

        #[cfg(feature = "svg")]
        let svg_err = {
            use crate::util::parse_svg_image;
            match parse_svg_image(&bytes) {
                // pyro: its `<view>` fragment, and its source when it has no size. The source is
                // also kept for inline SVG that masks with it (`svg_css`), up to a size: an image of
                // any kind can be a mask image (the cache is by URL), and most never are; a larger
                // one leaves the SVG element it masks unmasked.
                Ok(svg) => {
                    let mut svg = svg.for_url(&bytes, url);
                    const MAX_KEPT_SOURCE: usize = 256 << 10;
                    if bytes.len() <= MAX_KEPT_SOURCE {
                        svg.source = Some(std::sync::Arc::from(&bytes[..]));
                    }
                    // pyro: one with a `<foreignObject>` is drawn as a document of its own
                    // (`BaseDocument::svg_image_documents`).
                    if bytes.len() <= MAX_KEPT_SOURCE
                        && memchr_foreign_object(&bytes)
                        && let Ok(text) = std::str::from_utf8(&bytes)
                    {
                        svg.foreign_source = Some(std::sync::Arc::from(text));
                    }
                    return Ok(Resource::Svg(self.kind, svg));
                }
                // pyro: a document loaded for an element of it that a `clip-path`, `filter` or
                // `mask` references need not be an image usvg accepts (a root of no size,
                // `<svg width="0" height="0">`, is common): it is kept for its source.
                Err(e) => {
                    if let (ImageType::Reference(_), true) = (self.kind, bytes.len() <= 256 << 10) {
                        let sized = crate::node::embeddable_svg(&bytes);
                        if let Some(mut svg) = sized.and_then(|sized| parse_svg_image(sized.as_bytes()).ok()) {
                            svg.source = Some(std::sync::Arc::from(&bytes[..]));
                            return Ok(Resource::Svg(self.kind, svg));
                        }
                    }
                    e.to_string()
                }
            }
        };
        #[cfg(not(feature = "svg"))]
        let svg_err = {
            let _ = url;
            "svg feature disabled"
        };

        Err(format!(
            "Could not parse image ({} bytes): image-crate error: {image_err}; svg fallback error: {svg_err}",
            bytes.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parley::fontique::FontStyle as Fq;
    use style::stylesheets::AllowImportRules;
    use style::values::specified::Angle;

    fn oblique(min_deg: f32, max_deg: f32) -> FontStyleRange {
        FontStyleRange::Oblique(Angle::from_degrees(min_deg), Angle::from_degrees(max_deg))
    }

    /// The `unicode-range` of each `@font-face` rule in `css`
    fn unicode_ranges(css: &str) -> Vec<Option<parley::fontique::UnicodeRange>> {
        let lock = SharedRwLock::new();
        let base_url = Url::parse("https://example.com/css/style.css").unwrap();
        let sheet = Stylesheet::from_str(
            css,
            UrlExtraData(ServoArc::new(base_url)),
            Origin::Author,
            ServoArc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        );
        let guard = lock.read();
        sheet
            .contents(&guard)
            .rules(&guard)
            .iter()
            .filter_map(|rule| match rule {
                CssRule::FontFace(font_face) => Some(
                    font_face
                        .read_with(&guard)
                        .descriptors
                        .unicode_range
                        .as_deref()
                        .map(unicode_range),
                ),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unicode_ranges_keep_their_code_points() {
        let ranges = unicode_ranges(
            "@font-face { font-family: a; src: url(a.woff2); unicode-range: U+0400-045F, U+25-FF, u+4??, U+20AC }
            @font-face { font-family: a; src: url(b.woff2) }",
        );
        let [Some(range), None] = ranges.as_slice() else {
            panic!("{ranges:?}");
        };
        // Wildcards expand; overlapping intervals merge.
        assert_eq!(
            range.ranges().unwrap(),
            [0x25..=0xFF, 0x400..=0x4FF, 0x20AC..=0x20AC]
        );
        for code_point in [0x25, 0x41, 0xFF, 0x400, 0x4FF, 0x20AC] {
            assert!(range.contains(code_point), "{code_point:#x}");
        }
        for code_point in [0x20, 0x24, 0x100, 0x3FF, 0x500, 0x20AB, 0x20AD] {
            assert!(!range.contains(code_point), "{code_point:#x}");
        }
    }

    /// The source picked from the `src` list of each `@font-face` rule in `css`
    fn preferred_sources(css: &str) -> Vec<Option<(String, FontFaceSourceFormatKeyword)>> {
        let lock = SharedRwLock::new();
        let base_url = Url::parse("https://example.com/css/style.css").unwrap();
        let sheet = Stylesheet::from_str(
            css,
            UrlExtraData(ServoArc::new(base_url)),
            Origin::Author,
            ServoArc::new(lock.wrap(MediaList::empty())),
            lock.clone(),
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        );
        let guard = lock.read();
        sheet
            .contents(&guard)
            .rules(&guard)
            .iter()
            .filter_map(|rule| match rule {
                CssRule::FontFace(font_face) => font_face.read_with(&guard).descriptors.src.clone(),
                _ => None,
            })
            .map(|src| preferred_font_source(&src).map(|(url, format)| (url.to_string(), format)))
            .collect()
    }

    #[test]
    fn bootstrap_glyphicons_skip_eot_source() {
        let sources = preferred_sources(
            "@font-face {
                font-family: 'Glyphicons Halflings';
                src: url('../fonts/glyphicons-halflings-regular.eot');
                src: url('../fonts/glyphicons-halflings-regular.eot?#iefix') format('embedded-opentype'),
                    url('../fonts/glyphicons-halflings-regular.woff2') format('woff2'),
                    url('../fonts/glyphicons-halflings-regular.woff') format('woff'),
                    url('../fonts/glyphicons-halflings-regular.ttf') format('truetype'),
                    url('../fonts/glyphicons-halflings-regular.svg#glyphicons_halflingsregular') format('svg');
            }",
        );
        let expected = if cfg!(feature = "woff") {
            (
                "https://example.com/fonts/glyphicons-halflings-regular.woff2",
                FontFaceSourceFormatKeyword::Woff2,
            )
        } else {
            (
                "https://example.com/fonts/glyphicons-halflings-regular.ttf",
                FontFaceSourceFormatKeyword::Truetype,
            )
        };
        assert_eq!(sources, [Some((expected.0.to_string(), expected.1))]);
    }

    #[test]
    fn format_strings_map_to_formats() {
        use FontFaceSourceFormatKeyword as Kw;
        for (string, format) in [
            ("collection", Kw::Collection),
            ("embedded-opentype", Kw::EmbeddedOpentype),
            ("opentype", Kw::Opentype),
            ("OpenType", Kw::Opentype),
            ("opentype-variations", Kw::Opentype),
            ("otf", Kw::Opentype),
            ("svg", Kw::Svg),
            ("truetype", Kw::Truetype),
            ("truetype-variations", Kw::Truetype),
            ("ttf", Kw::Truetype),
            ("woff", Kw::Woff),
            ("woff-variations", Kw::Woff),
            ("woff2", Kw::Woff2),
            ("woff2-variations", Kw::Woff2),
            ("x-unknown", Kw::Unknown),
        ] {
            assert_eq!(font_format_from_string(string), format, "{string}");
        }
    }

    #[test]
    fn extension_ignores_query_and_fragment() {
        use FontFaceSourceFormatKeyword as Kw;
        for (url, format) in [
            ("https://example.com/a.eot?#iefix", Kw::EmbeddedOpentype),
            ("https://example.com/a.svg#font", Kw::Svg),
            ("https://example.com/a.TTF?v=4.7.0", Kw::Truetype),
            ("https://example.com/a.otf", Kw::Opentype),
            ("https://example.com/a.woff2?v=1#x", Kw::Woff2),
            ("https://example.com/fonts.v2/font", Kw::None),
            ("https://example.com/font?name=a.ttf", Kw::None),
            ("data:font/ttf;base64,AAEAAA.ttf", Kw::None),
        ] {
            let url = Url::parse(url).unwrap();
            assert_eq!(font_format_from_extension(&url), format, "{url}");
        }
    }

    #[test]
    fn unsupported_sources_are_skipped() {
        use FontFaceSourceFormatKeyword as Kw;
        let sources = preferred_sources(
            "@font-face { font-family: a; src: url(a.ttf) format('x-unknown'), url(a.otf); }
            @font-face { font-family: b; src: url(b.svg#b), url(b.eot), url(b.ttf?v=1#x); }
            @font-face { font-family: c; src: url(c.eot) format(embedded-opentype); }
            @font-face { font-family: d; src: url(d.eot) format(truetype); }",
        );
        assert_eq!(
            sources,
            [
                Some(("https://example.com/css/a.otf".to_string(), Kw::Opentype)),
                Some((
                    "https://example.com/css/b.ttf?v=1#x".to_string(),
                    Kw::Truetype
                )),
                None,
                Some(("https://example.com/css/d.eot".to_string(), Kw::Truetype)),
            ]
        );
    }

    #[test]
    fn italic_maps_to_italic() {
        assert_eq!(stylo_to_fontique_style(&FontStyleRange::Italic), Fq::Italic,);
    }

    #[test]
    fn oblique_zero_zero_maps_to_normal() {
        // Stylo parses bare CSS `normal` as `Oblique(0deg, 0deg)`; the
        // helper must round-trip that back to `FontStyle::Normal` so
        // parley's matching doesn't misclassify upright fonts.
        assert_eq!(stylo_to_fontique_style(&oblique(0.0, 0.0)), Fq::Normal);
    }

    #[test]
    fn oblique_single_angle_maps_to_oblique_with_min() {
        assert_eq!(
            stylo_to_fontique_style(&oblique(14.0, 14.0)),
            Fq::Oblique(Some(14.0)),
        );
    }

    #[test]
    fn oblique_range_uses_min_angle() {
        // For a range, fontique's single-angle representation takes the
        // lower bound — confirm `min` (not `max`) is what gets through.
        assert_eq!(
            stylo_to_fontique_style(&oblique(10.0, 20.0)),
            Fq::Oblique(Some(10.0)),
        );
    }
}

/// pyro: whether SVG source `bytes` has a `<foreignObject>` element (a cheap test).
#[cfg(feature = "svg")]
fn memchr_foreign_object(bytes: &[u8]) -> bool {
    bytes.windows(14).any(|w| w == b"foreignObject ")
        || bytes.windows(14).any(|w| w == b"foreignObject>")
        || bytes.windows(14).any(|w| w == b"foreignObject\n")
}
