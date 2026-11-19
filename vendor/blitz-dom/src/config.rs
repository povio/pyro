use crate::{HtmlParserProvider, UserAgentStylesheet};
#[cfg(feature = "devtools")]
use blitz_traits::devtools::DevtoolsProvider;
use blitz_traits::{
    navigation::NavigationProvider,
    net::{AbortSignal, NetProvider},
    shell::{ShellProvider, Viewport},
};
use parley::{FontContext, LineBreakOverrideFn};
use std::sync::Arc;
use style::media_queries::MediaType;

/// Strategy for Stylo's style traversal during `resolve`.
///
/// `Parallel` traversals are serialised process-wide (Stylo's global pool can
/// only run one traversal at a time), so documents resolving from several
/// threads never run their style traversals concurrently but also never
/// overlap: it only pays off when a single thread resolves all documents
/// (e.g. a UI thread, including nested sub-documents).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum StyleThreading {
    /// Use Stylo's parallel traversal via its global rayon thread pool.
    Parallel,
    /// Run style traversal sequentially on the calling thread, bypassing
    /// the global pool and its lock. Threads resolving their own documents
    /// run independently of each other.
    #[default]
    Sequential,
}

/// pyro: one frame document of the render snapshot's frame table ([`DocumentConfig::frames`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameContent {
    /// Its markup.
    pub html: String,
    /// Its URL, which its relative URLs resolve against.
    pub url: String,
    /// Its viewport's scroll position.
    pub scroll: (f64, f64),
    /// The frame (frame id and loader id) its requests are made for.
    pub client: String,
    /// Its own frames' documents.
    pub frames: FrameTable,
}

/// pyro: frame documents by the index a frame element's `data-pyro-frame` names.
pub type FrameTable = Arc<std::collections::HashMap<String, FrameContent>>;

/// pyro: the attribute a frame element names its frame table entry in: `"<index>:<version>"`.
pub const FRAME_ATTRIBUTE: &str = "data-pyro-frame";

/// Options used when constructing a [`BaseDocument`](crate::BaseDocument)
pub struct DocumentConfig {
    /// The initial `Viewport`
    pub viewport: Option<Viewport>,
    /// The base url which relative URLs are resolved against
    pub base_url: Option<String>,
    /// pyro: the id of the frame this is the document of (a frame document of the render
    /// snapshot): its requests made with other than the defaults name it as their client.
    pub frame_client: Option<String>,
    /// pyro: the frame table of the render snapshot: the documents its frame elements show, by
    /// the index their `data-pyro-frame` attribute names. A frame element loads only the entry
    /// it names (content, URL, client), never content from its own attributes.
    pub frames: FrameTable,
    /// User Agent stylesheets, in cascade order. Defaults to `vec![UserAgentStylesheet::default_sheet()]`.
    ///
    /// The list is used as given, so replacing it removes the default sheet unless it is
    /// included, e.g. `vec![UserAgentStylesheet::default_sheet(), custom]`.
    pub ua_stylesheets: Vec<UserAgentStylesheet>,
    /// Net provider to handle network requests for resources
    pub net_provider: Option<Arc<dyn NetProvider>>,
    /// Navigation provider to handle link clicks and form submissions
    pub navigation_provider: Option<Arc<dyn NavigationProvider>>,
    /// Shell provider to redraw requests, clipboard, etc
    pub shell_provider: Option<Arc<dyn ShellProvider>>,
    /// HTML parser provider. Used to parse HTML for setInnerHTML
    pub html_parser_provider: Option<Arc<dyn HtmlParserProvider>>,
    /// Parley `FontContext`
    pub font_ctx: Option<FontContext>,
    /// The CSS media type used to evaluate `@media` rules.
    /// Defaults to [`MediaType::screen`].
    pub media_type: Option<MediaType>,
    /// Strategy for Stylo's style traversal.
    /// Defaults to [`StyleThreading::Sequential`].
    pub style_threading: StyleThreading,
    /// Whether incremental layout is enabled. Defaults to `true`.
    ///
    /// When disabled, every node is treated as damaged on each `resolve` and the
    /// whole tree is reconstructed and laid out from scratch through the same
    /// damage-driven pipeline used by incremental mode.
    pub incremental: Option<bool>,
    /// If set, every sub-resource `Request` blitz-dom creates for this
    /// document will carry this signal. Aborting it cancels every in-flight
    /// fetch tied to this document.
    pub abort_signal: Option<AbortSignal>,
    /// How deeply this document is nested within other documents
    /// (0 for a root document). Used to limit `<iframe>` nesting depth.
    pub subdocument_depth: usize,
    /// Soft wrap opportunities to add or remove from the default (UAX #14, as ICU tailors it),
    /// e.g. [`parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE`] for Chrome's breaks between a hyphen
    /// and a digit. Applies to every text layout of the document (line breaking and min/max
    /// content sizes alike) and is inherited by its iframes. Defaults to `None`.
    pub line_break_override: Option<&'static LineBreakOverrideFn>,
    /// pyro: an XML document (XHTML or other XML): type and attribute selectors match names
    /// case-sensitively there (Selectors 4, "Case sensitivity"). Defaults to `false`: HTML.
    pub xml_document: bool,
    /// pyro: a document in quirks mode: a percentage height whose containing block is a block
    /// of `auto` height resolves against the nearest ancestor that has one (Quirks Mode §3.5,
    /// "The percentage height calculation quirk"). Defaults to `false`.
    pub quirks_mode: bool,
    /// pyro: a document in quirks or limited-quirks mode: a line without text, preserved white
    /// space or inline boxes with edges has no strut (Quirks Mode §3.3, "The line height
    /// calculation quirk"). Defaults to `false`.
    pub line_height_quirk: bool,
    /// pyro: the document's language from the protocol (HTTP `Content-Language`), the last
    /// fallback for `:lang()` when no element and no `<meta http-equiv>` pragma gives one.
    pub content_language: Option<String>,
    /// Devtools provider notified of DOM mutations (e.g. by a remote inspector)
    #[cfg(feature = "devtools")]
    pub devtools_provider: Option<Arc<dyn DevtoolsProvider>>,
}

impl Default for DocumentConfig {
    fn default() -> Self {
        Self {
            viewport: None,
            base_url: None,
            frame_client: None,
            frames: FrameTable::default(),
            ua_stylesheets: vec![UserAgentStylesheet::default_sheet()],
            net_provider: None,
            navigation_provider: None,
            shell_provider: None,
            html_parser_provider: None,
            font_ctx: None,
            media_type: None,
            style_threading: StyleThreading::default(),
            incremental: None,
            abort_signal: None,
            subdocument_depth: 0,
            line_break_override: None,
            xml_document: false,
            quirks_mode: false,
            line_height_quirk: false,
            content_language: None,
            #[cfg(feature = "devtools")]
            devtools_provider: None,
        }
    }
}
