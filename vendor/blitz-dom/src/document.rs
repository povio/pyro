use crate::NodeTree;
use crate::events::{DragMode, handle_dom_event};
use crate::layout::construct::ConstructionTask;
use crate::layout::damage::ALL_DAMAGE;
use crate::mutator::ViewportMut;
use crate::net::{
    Resource, ResourceHandler, ResourceLoadResponse, StylesheetHandler, StylesheetLoader,
};
use crate::node::{ImageData, NodeFlags, RasterImageData, SpecialElementData, Status, TextBrush};
use crate::scrolling::ScrollAnimationState;
use crate::selection::TextSelection;
use crate::stylo_device::{DeviceChanges, make_device};
use crate::stylo_to_cursor_icon::stylo_to_cursor_icon;
use crate::traversal::TreeTraverser;
use crate::url::DocumentUrl;
use crate::util::ImageType;
use crate::{
    DEFAULT_CSS, DocumentConfig, DocumentMutator, DummyHtmlParserProvider, ElementData,
    EventDriver, HtmlParserProvider, Node, NodeData, NoopEventHandler, StyleThreading,
    TextNodeData,
};
use bitflags::bitflags;
use blitz_traits::devtools::DevtoolSettings;
use blitz_traits::events::{DomEvent, HitResult, UiEvent};
use blitz_traits::navigation::{DummyNavigationProvider, NavigationProvider};
use blitz_traits::net::{AbortSignal, DummyNetProvider, NetProvider, Request};
use blitz_traits::node_id::NodeId;
use blitz_traits::shell::{DummyShellProvider, ShellProvider, Viewport};
use cursor_icon::CursorIcon;
use linebender_resource_handle::Blob;
use markup5ever::{LocalName, local_name};
use parley::{FontContext, PlainEditorDriver};
use selectors::{Element, matching::QuirksMode};
use smallvec::SmallVec;
use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{
    Arc, LazyLock, Mutex, MutexGuard, Once, OnceLock, RwLockReadGuard, RwLockWriteGuard,
};
use std::task::{Context as TaskContext, Waker};
use style::Atom;
use style::animation::DocumentAnimationSet;
use style::attr::{AttrIdentifier, AttrValue};
use style::computed_value_flags::ComputedValueFlags;
use style::data::{ElementData as StyloElementData, ElementStyles};
use style::invalidation::element::restyle_hints::RestyleHint;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::properties::style_structs::Font;
use style::selector_parser::ServoElementSnapshot;
use style::servo_arc::Arc as ServoArc;
use style::values::GenericAtomIdent;
use style::values::computed::UserSelect;
use style::values::computed::ui::CursorKind;
use style::{
    device::Device,
    dom::{TDocument, TNode},
    media_queries::MediaList,
    selector_parser::SnapshotMap,
    shared_lock::{SharedRwLock, StylesheetGuards},
    stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet},
    stylist::Stylist,
};
use style_dom::ElementState;
use stylo_taffy::CopyStyleGetters;
use thin_vec::ThinVec;
use url::Url;

/// pyro: whether a paint for a canvas a page can read back is running on this thread (blitz-paint
/// `paint_element_subtree`, which holds [`crate::node::UnvisitedPaint`]).
fn blitz_dom_privacy_paint() -> bool {
    crate::node::UnvisitedPaint::active()
}
use web_time::Instant;

#[cfg(feature = "parallel-construct")]
use thread_local::ThreadLocal;

/// A user-agent stylesheet, parsed with a read-only lock so that it can be shared between documents.
///
/// Documents with the same user-agent stylesheets (in the same order) also share Stylo's
/// cached user-agent cascade data. `@import` rules are ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct UserAgentStylesheet(DocumentStyleSheet);

pub(crate) fn init_stylo_prefs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        style_config::set_pref!("layout.css.anchor-positioning.enabled", true);
        // pyro: `container-type: scroll-state` and `@container scroll-state(scrollable)`.
        style_config::set_pref!("layout.css.scroll-state.enabled", true);
        style_config::set_pref!("layout.css.flexbox-balance.enabled", true);
        // pyro: `offset-path: url()` to an SVG shape element (resolve.rs `motion_url_path`).
        style_config::set_pref!("layout.css.motion-path-url.enabled", true);
        style_config::set_pref!("layout.css.text-box.enabled", true);
        // pyro: `font-format()`/`font-tech()` in `@supports`, `tech()` in `@font-face` sources.
        style_config::set_pref!("layout.css.font-tech.enabled", true);
        // pyro: `text-autospace` (parley spaces ideographs from letters and numerals).
        style_config::set_pref!("layout.css.text-autospace.enabled", true);
        // pyro: `text-combine-upright` (no text-autospace in such upright text; the layout ignores it).
        style_config::set_pref!("layout.css.text-combine-upright.enabled", true);
        // pyro: `font-variant-emoji` (parley picks a colour or a text font by it).
        style_config::set_pref!("layout.css.font-variant-emoji.enabled", true);
        // pyro: `initial-letter` on `::first-letter` (`layout::first_letter`).
        style_config::set_pref!("layout.css.initial-letter.enabled", true);
        // pyro: `dominant-baseline` on HTML boxes (`stylo_to_parley::vertical_align`).
        style_config::set_pref!("layout.css.dominant-baseline.enabled", true);
        // pyro: `light-dark()` of images (the computed value is the image of the used scheme).
        style_config::set_pref!("layout.css.light-dark.images.enabled", true);
        // pyro: `cross-fade()` (blitz-paint draws it).
        style_config::set_pref!("layout.css.cross-fade.enabled", true);
        style_config::set_pref!("layout.css.background-clip.text.enabled", true);
        // pyro: `background-clip: border-area` (blitz-paint clips it to the border's area).
        style_config::set_pref!("layout.css.background-clip.border-area.enabled", true);
        style_config::set_pref!("layout.css.webkit-text-fill-color.enabled", true);
        style_config::set_pref!("layout.css.columns.enabled", true);
        style_config::set_pref!("layout.css.hyphens.enabled", true);
        // pyro: `text-wrap-style: balance` (blitz-dom balances the lines of such a block).
        style_config::set_pref!("layout.css.text-wrap-style.balance.enabled", true);
        // pyro: `text-wrap-style: pretty` (laid out as `auto`, as CSS Text 4 allows). Chrome
        // rejects `white-space-collapse: preserve-spaces`, which stays off.
        style_config::set_pref!("layout.css.text-wrap-style.pretty.enabled", true);
        style_config::set_pref!("layout.css.basic-shape-shape.enabled", true);
        style_config::set_pref!("layout.css.tree-counting-functions.enabled", true);
        style_config::set_pref!("layout.css.progress-function.enabled", true);
        style_config::set_pref!("layout.css.random.enabled", true);
        style_config::set_pref!("layout.css.font-variations.enabled", true);
        style_config::set_pref!("layout.css.text-indent-keywords.enabled", true);
        // pyro: `grid-template-rows/columns: subgrid` (Taffy lays subgrids out, css-grid-2 §9).
        style_config::set_pref!("layout.css.grid-template-subgrid-value.enabled", true);
        style_config::set_pref!("layout.css.word-break.break-word.enabled", true);
        #[cfg(feature = "writing-mode")]
        {
            style_config::set_pref!("layout.css.writing-mode.enabled", true);
            style_config::set_pref!("layout.css.writing-mode.sideways.enabled", true);
        }

        // Properties that were previously enabled by the `layout.unimplemented` pref
        style_config::set_pref!("layout.css.appearance.enabled", true);
        style_config::set_pref!("layout.css.backdrop-filter.enabled", true);
        style_config::set_pref!("layout.css.contain.enabled", true);
        style_config::set_pref!("layout.css.content.alt-text.enabled", true);
        style_config::set_pref!("layout.css.corner-shape.enabled", true);
        // pyro: `closest-corner`/`farthest-corner` radii in `ellipse()` (CSS Shapes 2).
        style_config::set_pref!("layout.css.ellipse-corners.enabled", true);
        style_config::set_pref!("layout.css.counters.enabled", true);
        // pyro: `::first-line` parses and is cascaded (`layout::first_line`).
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
        style_config::set_pref!("layout.css.overscroll-behavior.enabled", true);
        style_config::set_pref!("layout.css.scroll-driven-animations.enabled", true);
        // pyro: `display` in `@keyframes` and Web Animations keyframes, and Stylo's `Animate for
        // Display` (css-display-4 §2.1: the non-`none` value throughout, `none` only at its end).
        // Without it a `@keyframes` rule animating only `display` has no steps, and keyframes to
        // or from `none` flip halfway (WPT css-display `animations/display-interpolation{,-none}`).
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
        style_config::set_pref!("layout.css.writing-mode.text-orientation.enabled", true);
        style_config::set_pref!("layout.css.zoom.enabled", true);
        // pyro: the features pyro-dom's cascade enables. Prefs are process-wide and read
        // while parsing, so without these a sheet parsed here before pyro-dom's first cascade dropped
        // such rules. `:has()` comes with invalidation (`invalidate_has_anchors`).
        style_config::set_pref!("layout.css.has-selector.enabled", true);
        style_config::set_pref!("layout.css.container-queries.enabled", true);
        style_config::set_pref!("layout.css.nth-child-of.enabled", true);
        style_config::set_pref!("layout.css.starting-style-at-rules.enabled", true);
        style_config::set_pref!("layout.css.at-scope.enabled", true);
        style_config::set_pref!("layout.css.style-queries.enabled", true);
        style_config::set_pref!("layout.css.line-clamp.enabled", true);
        style_config::set_pref!("layout.css.fit-content-function.enabled", true);
        style_config::set_pref!("layout.css.supports.at-rule.enabled", true);
        style_config::set_pref!("layout.threads", -1);
    });
}

impl UserAgentStylesheet {
    pub fn parse(css: &str) -> Self {
        // Pref-gated UA declarations must be enabled before parsing.
        init_stylo_prefs();

        let lock = SharedRwLock::read_only();
        let data = Stylesheet::from_str(
            css,
            DocumentUrl::default().url_extra_data(),
            Origin::UserAgent,
            ServoArc::new(lock.wrap(MediaList::empty())),
            lock,
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        );
        Self(DocumentStyleSheet(ServoArc::new(data)))
    }

    /// The default user-agent stylesheet ([`DEFAULT_CSS`]), parsed once.
    pub fn default_sheet() -> Self {
        static SHEET: LazyLock<UserAgentStylesheet> =
            LazyLock::new(|| UserAgentStylesheet::parse(DEFAULT_CSS));
        SHEET.clone()
    }

    pub fn sheet(&self) -> &DocumentStyleSheet {
        &self.0
    }
}

pub enum DocGuard<'a> {
    Ref(&'a BaseDocument),
    RefCell(std::cell::Ref<'a, BaseDocument>),
    RwLock(RwLockReadGuard<'a, BaseDocument>),
    Mutex(MutexGuard<'a, BaseDocument>),
}

impl Deref for DocGuard<'_> {
    type Target = BaseDocument;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Ref(base_document) => base_document,
            Self::RefCell(refcell_guard) => refcell_guard,
            Self::RwLock(rw_lock_read_guard) => rw_lock_read_guard,
            Self::Mutex(mutex_guard) => mutex_guard,
        }
    }
}

pub enum DocGuardMut<'a> {
    Ref(&'a mut BaseDocument),
    RefCell(std::cell::RefMut<'a, BaseDocument>),
    RwLock(RwLockWriteGuard<'a, BaseDocument>),
    Mutex(MutexGuard<'a, BaseDocument>),
}

impl Deref for DocGuardMut<'_> {
    type Target = BaseDocument;
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Ref(base_document) => base_document,
            Self::RefCell(refcell_guard) => refcell_guard,
            Self::RwLock(rw_lock_read_guard) => rw_lock_read_guard,
            Self::Mutex(mutex_guard) => mutex_guard,
        }
    }
}

impl DerefMut for DocGuardMut<'_> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Ref(base_document) => base_document,
            Self::RefCell(refcell_guard) => &mut *refcell_guard,
            Self::RwLock(rw_lock_read_guard) => &mut *rw_lock_read_guard,
            Self::Mutex(mutex_guard) => &mut *mutex_guard,
        }
    }
}

/// Abstraction over wrappers around [`BaseDocument`] to allow for them all to
/// be driven by [`blitz-shell`](https://docs.rs/blitz-shell)
pub trait Document: Any + 'static {
    fn inner(&self) -> DocGuard<'_>;
    fn inner_mut(&mut self) -> DocGuardMut<'_>;

    /// Update the [`Document`] in response to a [`UiEvent`] (click, keypress, etc)
    fn handle_ui_event(&mut self, event: UiEvent) {
        let mut doc = self.inner_mut();
        let mut driver = EventDriver::new(&mut *doc, NoopEventHandler);
        driver.handle_ui_event(event);
    }

    /// Poll any pending async operations, and flush changes to the underlying [`BaseDocument`]
    fn poll(&mut self, task_context: Option<TaskContext>) -> bool {
        // Default implementation does nothing
        let _ = task_context;
        false
    }

    /// Get the [`Document`]'s id
    fn id(&self) -> usize {
        self.inner().id
    }
}

pub struct PlainDocument(pub BaseDocument);
impl Document for PlainDocument {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::Ref(&self.0)
    }
    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::Ref(&mut self.0)
    }
}

impl Document for BaseDocument {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::Ref(self)
    }
    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::Ref(self)
    }
}

impl Document for Rc<RefCell<BaseDocument>> {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::RefCell(self.borrow())
    }

    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::RefCell(self.borrow_mut())
    }
}

pub enum DocumentEvent {
    ResourceLoad(ResourceLoadResponse),
    /// A navigation originating from within an iframe's sub-document
    /// (e.g. a link click), to be applied to the iframe identified by `node_id`.
    NavigateIframe {
        node_id: NodeId,
        url: Url,
    },
}

/// pyro: files a traversal's replaced styles into `changes`
/// (`BaseDocument::style_changes`): an element already there keeps its
/// first old style and takes the new one.
pub(crate) fn note_style_changes(
    changes: &mut Vec<StyleChange>,
    index: &mut HashMap<(NodeId, u8), usize>,
    new: Vec<StyleChange>,
) {
    for change in new {
        match index.get(&(change.node, change.pseudo)) {
            Some(&i) => changes[i].after = change.after,
            None => {
                index.insert((change.node, change.pseudo), changes.len());
                changes.push(change);
            }
        }
    }
}

/// pyro: the slots of [`BaseDocument::take_anchor_evals`]: `left`, `right`, `top`, `bottom`,
/// `width`, `height`, `min-width`, `min-height`, `max-width`, `max-height`.
pub const ANCHOR_USED_SLOTS: usize = 10;

/// pyro: an element whose used `anchor()`/`anchor-size()` values changed, by slot
/// ([`ANCHOR_USED_SLOTS`]; `None`: not an anchor value, or not resolved).
#[derive(Clone, Debug)]
pub struct AnchorEval {
    pub node: NodeId,
    pub before: [Option<f32>; ANCHOR_USED_SLOTS],
    pub after: [Option<f32>; ANCHOR_USED_SLOTS],
}

/// pyro: an element whose primary style a style traversal replaced
/// ([`BaseDocument::take_style_changes`]).
#[derive(Clone)]
pub struct StyleChange {
    pub node: NodeId,
    /// pyro: 0 for the element's own style, 1 its `::before`'s, 2 its `::after`'s.
    pub pseudo: u8,
    pub before: style::servo_arc::Arc<style::properties::ComputedValues>,
    pub after: style::servo_arc::Arc<style::properties::ComputedValues>,
}

pub struct BaseDocument {
    /// ID of the document
    id: usize,

    // Config
    /// Base url for resolving linked resources (stylesheets, images, fonts, etc)
    pub(crate) url: DocumentUrl,
    fragment_document: Option<DocumentUrl>,
    // Devtool settings. Currently used to render debug overlays
    pub(crate) devtool_settings: DevtoolSettings,
    // Viewport details such as the dimensions, HiDPI scale, and zoom factor,
    pub(crate) viewport: Viewport,
    /// CSS media type used to evaluate `@media` rules.
    pub(crate) media_type: MediaType,
    pub(crate) media_environment: crate::MediaEnvironment,
    /// External `<use href>` targets, by href as written: the local id that
    /// replaces it and the definition (SVG source with that id). Added to
    /// each `<svg>` that refers to them when it is rendered.
    pub svg_sprites: HashMap<String, crate::util::SvgSprite>,
    /// Fetched SVG image resources accessible to this document's href resolver.
    pub svg_images: crate::util::SvgResources,
    /// pyro: HTML-in-Canvas privacy: [`Self::svg_images`] without the resources of an opaque
    /// response, with the generation of the map it was taken from ([`Self::svg_images_for_paint`],
    /// `SvgResourceMap::generation`).
    pub(crate) privacy_svg_images: std::cell::RefCell<Option<(u64, crate::util::SvgResources)>>,
    /// Changes to the stylist [`Device`] that have been requested since the
    /// last [`resolve`](Self::resolve), to be applied (coalesced into a single
    /// device rebuild) at the start of the next resolve.
    pub(crate) pending_device_changes: DeviceChanges,
    /// pyro: the page's supported color schemes, from its first valid `<meta name=color-scheme>`.
    pub(crate) page_color_scheme: style::values::specified::color::ColorSchemeFlags,
    /// pyro: the `<meta>` elements made in this document (removed ones drop out when looked at).
    pub(crate) meta_nodes: Vec<NodeId>,
    /// pyro: the root's available space in the last layout pass.
    pub(crate) last_layout_space: Option<taffy::Size<taffy::AvailableSpace>>,
    /// pyro: classic scrollbars (`stylo_taffy::scrollbars`): the viewport's scrollbar gutters (physical
    /// edges, CSS px), which the initial containing block is smaller by, and which of its scrollbars
    /// show (x: the horizontal one). All zero with overlay scrollbars.
    pub viewport_scrollbars: ViewportScrollbars,
    /// pyro: whether `resolve_layout` laid the document out since `resolve` last checked the
    /// classic `overflow: auto` scrollbars.
    pub(crate) layout_ran: bool,
    /// pyro: this is the document of an `<iframe scrolling=no>`: its viewport shows no scrollbars
    /// (`viewport_overflow` is `hidden`).
    pub(crate) frame_no_scrollbars: bool,
    /// pyro: classic scrollbars: what viewport units see the viewport smaller by (CSS px; the
    /// root's `scroll`/`stable` gutters, `stylo` `Device::set_viewport_unit_inset`).
    pub(crate) viewport_unit_inset: (f32, f32),
    /// See [`BaseDocument::set_layout_size`].
    layout_size: Option<(f32, f32)>,
    /// See [`BaseDocument::set_layout_size`].
    pub(crate) layout_available: Option<(f32, f32)>,
    /// Document-wide work for the next [`resolve`](Self::resolve).
    pub(crate) flags: DocumentFlags,
    /// Strategy for Stylo's style traversal during `resolve`.
    pub(crate) style_threading: StyleThreading,
    /// Whether incremental layout is enabled for this document. When disabled,
    /// every node is treated as damaged on every `resolve`, so the same damage
    /// pipeline reconstructs and relays out the whole tree.
    pub(crate) incremental_layout: bool,
    /// How deeply this document is nested within other documents
    /// (0 for a root document). Used to limit `<iframe>` nesting depth.
    pub(crate) subdocument_depth: usize,
    pub raster_budget: pyro_raster::PageRasters,
    /// pyro: the id of the frame this is the document of (`DocumentConfig::frame_client`).
    pub(crate) frame_client: Option<String>,
    /// pyro: `DocumentConfig::frames`: the documents its frame elements show.
    pub(crate) frames: crate::FrameTable,
    /// pyro: its sheets' `Referrer-Policy`, by sheet object (`net::SheetPolicies`).
    pub(crate) sheet_policies: crate::net::SheetPolicies,
    /// pyro: the URL data objects of its own CSS (`net::InlineUrls`).
    pub(crate) inline_urls: crate::net::InlineUrls,

    // Events
    pub(crate) tx: Sender<DocumentEvent>,
    // rx will always be Some, except temporarily while processing events
    pub(crate) rx: Option<Receiver<DocumentEvent>>,

    /// A slotmap-backed tree of nodes
    ///
    /// We pin the tree to a guarantee to the nodes it creates that the tree is stable in memory.
    /// There is no way to create the tree - publicly or privately - that would invalidate that invariant.
    pub(crate) nodes: Box<NodeTree>,

    /// The id of the root node (a Document node)
    pub(crate) root_node_id: NodeId,

    // Stylo
    /// The Stylo engine
    pub(crate) stylist: Stylist,
    pub(crate) animations: DocumentAnimationSet,
    /// Stylo shared lock
    pub(crate) guard: SharedRwLock,
    /// Stylo invalidation map. We insert into this map prior to mutating nodes.
    pub(crate) snapshots: SnapshotMap,

    // Parley contexts
    /// A Parley font context
    pub(crate) font_ctx: Arc<Mutex<parley::FontContext>>,
    #[cfg(feature = "parallel-construct")]
    /// Thread-and-document-local copies to the font context
    pub(crate) thread_font_contexts: ThreadLocal<RefCell<Box<FontContext>>>,
    /// A Parley layout context
    pub(crate) layout_ctx: parley::LayoutContext<TextBrush>,
    /// pyro: the `font-family` names `@font-face` rules registered each font file under, by blob
    /// id: `@font-palette-values` rules name the fonts they apply to by family
    /// (`BaseDocument::font_face_families`).
    pub(crate) font_face_families: HashMap<u64, Vec<String>>,

    /// The real (non-anonymous) node which is currently hovered (if any).
    /// This is never a layout-generated (anonymous) node, so it remains valid
    /// across box-tree reconstruction.
    pub(crate) hover_node_id: Option<NodeId>,
    /// The precise (may be anonymous) layout node under the pointer (if any).
    /// This can be invalidated by box-tree reconstruction, and is re-resolved against
    /// fresh layout at the end of every `resolve` pass.
    pub(crate) hover_hit_node_id: Option<NodeId>,
    /// Whether the node which is currently hovered is a text node/span
    pub(crate) hover_node_is_text: bool,
    /// The last known pointer position in client coordinates (viewport-relative, unscrolled).
    pub(crate) last_client_pointer_position: Option<taffy::Point<f32>>,
    /// The node which is currently focussed (if any)
    pub(crate) focus_node_id: Option<NodeId>,
    /// The node which is currently active (if any)
    pub(crate) active_node_id: Option<NodeId>,
    /// The focused node matches `:focus-visible` (FOCUSRING), as the
    /// embedder decides it.
    pub(crate) focus_ring: bool,
    /// The node which recieved a mousedown event (if any)
    pub(crate) mousedown_node_id: Option<NodeId>,
    /// The last time a mousedown was made (for double-click detection)
    pub(crate) last_mousedown_time: Option<Instant>,
    /// The position where mousedown occurred (for selection drags and double-click detection)
    pub(crate) mousedown_position: taffy::Point<f32>,
    /// How many clicks have been made in quick succession
    pub(crate) click_count: u16,
    /// Whether we're currently in a text selection drag (moved 2px+ from mousedown)
    pub(crate) drag_mode: DragMode,
    /// The scrollbar thumb currently under the pointer, if any
    pub(crate) hovered_scrollbar: Option<crate::node::ScrollbarRef>,
    /// When each scroll container's overlay scrollbars were last shown
    /// (scrolled, or the pointer left the thumb); drives their fade-out
    pub(crate) scrollbar_activity: HashMap<NodeId, Instant>,
    /// Whether and what kind of scroll animation is currently in progress
    pub(crate) scroll_animation: ScrollAnimationState,

    /// Text selection state (for non-input text)
    pub(crate) text_selection: TextSelection,

    // TODO: collapse animating state into a bitflags
    /// Whether there are active CSS animations/transitions (so we should re-render every frame)
    pub(crate) has_active_animations: bool,
    /// pyro: the embedder's sampled animations (`animation.rs`).
    pub(crate) sampled_animations: crate::animation::SampledAnimations,
    /// Whether there is a `<canvas>` element in the DOM (so we should re-render every frame)
    pub(crate) has_canvas: bool,
    /// Whether there are subdocuments that are animating (so we should re-render every frame)
    pub(crate) subdoc_is_animating: bool,

    /// Map of id attribute values to node IDs for fast lookups.
    /// May contain multiple nodes for the same id: `get_element_by_id`
    /// returns the first in tree order.
    pub(crate) nodes_to_id: HashMap<String, SmallVec<[NodeId; 1]>>,
    /// Map of `<style>` and `<link>` node IDs to their associated stylesheet
    pub(crate) nodes_to_stylesheet: BTreeMap<NodeId, DocumentStyleSheet>,
    /// pyro: the host whose shadow root holds the sheet of a `<style>` or `<link>` element of a
    /// shadow tree (not in `nodes_to_stylesheet`: `crate::shadow`).
    pub(crate) shadow_sheet_hosts: HashMap<NodeId, NodeId>,
    /// pyro: the text of `<link>` sheets with `@page` rules, by node. Parsing drops what
    /// printing needs of them: the servo build of Stylo has no `size` and no `page`.
    pub(crate) linked_page_css: BTreeMap<NodeId, String>,
    /// Incremented whenever `nodes_to_stylesheet` changes (a stylesheet is
    /// added, replaced or removed), so that CSSOM wrappers can detect that
    /// their cached rule data is stale.
    pub(crate) stylesheet_generation: u64,
    /// Map from form control node ID's to their associated forms node ID's
    pub(crate) controls_to_form: HashMap<NodeId, NodeId>,
    /// Nodes that contain sub documents
    pub(crate) sub_document_nodes: HashSet<NodeId>,
    /// pyro: the `content-visibility: auto` elements whose relevance is determined
    /// (`NodeFlags::CV_DETERMINED`), those a query forced to render (`CV_FORCED`), and the scroll
    /// position relevance was last determined at (`update_content_visibility`).
    pub(crate) cv_auto: HashSet<NodeId>,
    /// pyro: the anchors each absolutely positioned box resolved at its last layout, with the
    /// layout pass (`cv_layout_pass`) that laid it out (`BaseDocument::record_cv_anchor_use`).
    pub(crate) cv_anchor_uses: RefCell<HashMap<NodeId, (u64, Vec<NodeId>)>>,
    /// pyro: the layout passes run so far (`resolve_layout_in_flush`).
    pub(crate) cv_layout_pass: std::cell::Cell<u64>,
    /// pyro: the (anchor, positioned box) pairs of the last rendering frame
    /// (`commit_cv_anchor_frame`): each anchor's ancestors below the positioned box's are relevant
    /// to the user (`update_content_visibility`).
    pub(crate) cv_anchor_frame: HashSet<(NodeId, NodeId)>,
    /// pyro: the elements that may remember a size (CSS Sizing 4 § 5.1): styled with
    /// `contain-intrinsic-size: auto` in an axis (or `content-visibility: auto`), or holding a
    /// remembered size (`BaseDocument::record_remembered_sizes`).
    pub(crate) cis_auto: HashSet<NodeId>,
    pub(crate) remembered_size_work: std::cell::Cell<u64>,
    /// pyro: sizes are to be remembered at the end of the next layout (`request_remembered_sizes`).
    pub(crate) remembered_pending: bool,
    pub(crate) cv_forced: HashSet<NodeId>,
    pub(crate) cv_scroll: Option<(f64, f64)>,
    /// pyro: a resolve again for relevance a fresh transform changed is running.
    pub(crate) cv_resolving: bool,
    /// pyro: steps the relevance passes took (candidates looked up, chain steps walked): what
    /// tests bound (`content_visibility_work`).
    pub(crate) cv_work: std::cell::Cell<u64>,
    /// pyro: the user-agent sheets it was made with, which the frame documents it makes take too
    /// (the embedder's: no native CSS animations or transitions in a frame either, as its
    /// animations are sampled in, `frame_animations`).
    pub(crate) ua_stylesheets: Vec<UserAgentStylesheet>,
    /// pyro: the animations of frame documents now, by frame client (`data-pyro-frame-client`):
    /// sampled into those sub-documents at each resolve, their elements by `data-pyro-key`
    /// (handed down to nested frames).
    pub(crate) frame_animations:
        std::sync::Arc<HashMap<String, std::sync::Arc<Vec<(u64, crate::animation::ElementEffects)>>>>,
    /// Load state (abort controller and in-flight request id) for each
    /// `<iframe>` element whose sub-document is loaded automatically
    pub(crate) iframe_loads: HashMap<NodeId, crate::iframe::IframeLoad>,
    /// Whether the DOM or focus changed since the accessibility tree was last pushed
    pub(crate) has_changes: bool,
    /// Set of changed nodes for updating the accessibility tree
    pub(crate) deferred_construction_nodes: Vec<ConstructionTask>,
    /// pyro: the block each `display: run-in` box that runs in runs into, by block
    /// (`layout::construct::resolve_run_ins`).
    pub(crate) run_in_targets: std::collections::HashMap<NodeId, NodeId>,
    /// Boxes laid out by an out-of-flow containing block (`oof_containing_block`
    /// set), so paint-tree passes visit them instead of every node.
    pub(crate) oof_hoisted: HashSet<NodeId>,
    /// pyro: nodes the style traversal gave a `position: sticky`
    /// style (`BaseDocument::has_sticky`). Entries whose node is gone or no
    /// longer sticky are dropped after each traversal.
    pub(crate) sticky_nodes: HashSet<NodeId>,
    /// pyro: the elements whose primary style the style traversals
    /// replaced since [`BaseDocument::take_style_changes`], with the style
    /// before and after (a transition's before-change and after-change
    /// styles), kept while `track_style_changes` is on. `style_change_index`
    /// finds an element's entry, so one restyled twice keeps its first old and
    /// its last new style.
    pub(crate) style_changes: Vec<StyleChange>,
    /// pyro: the originating elements of the scroll marker groups that scrolled to keep their
    /// active marker in view since the last [`Self::take_scroll_group_moves`]: the page fires
    /// `scroll` and `scrollend` at them (`layout::scroll_markers`).
    pub(crate) scroll_group_moves: Vec<NodeId>,
    /// pyro: the page's scroll offsets for scroll-driven animations (`scroll_timeline`): its
    /// scrolled elements' and the viewport's. Layout queries do not move the nodes' own scroll
    /// offsets (a capture does), so scroll timelines read the page's here.
    /// `None` (a document made for one capture): the nodes' own offsets and the viewport's.
    pub(crate) timeline_scroll: Option<(std::collections::HashMap<NodeId, (f64, f64)>, (f64, f64))>,
    pub(crate) style_change_index: HashMap<(NodeId, u8), usize>,
    pub(crate) track_style_changes: bool,
    /// pyro: the boxes that left a container's list (`Node::layout_child_of`) during
    /// `resolve_layout_children`; those no container collected again by its end leave the layout
    /// tree with their subtrees (`BaseDocument::leave_layout_tree`). Empty between passes.
    pub(crate) left_layout_lists: Vec<NodeId>,
    /// pyro: the `<foreignObject>` elements of each inline `<svg>` (by the `<svg>`'s id), set when
    /// the `<svg>`'s box is built. Each is a root of its own: its HTML content is laid out in a box
    /// of its `width` and `height` and painted where usvg draws its stand-in image
    /// (`svg_css`, blitz-paint `draw_svg`). See [`BaseDocument::foreign_object_roots`].
    pub(crate) svg_foreign_objects: HashMap<NodeId, Vec<NodeId>>,
    /// pyro: the documents SVG images with a `<foreignObject>` are drawn from, by the element
    /// showing the image: its source parsed as XML (no script runs in it), laid out at the image's
    /// natural size (`BaseDocument::update_svg_image_documents`), painted by blitz-paint in place of
    /// the image's usvg tree, which has no HTML content. Set once such an image loaded.
    pub svg_image_documents: HashMap<NodeId, SvgImageDocument>,
    /// pyro: inline `<svg>`s serialized again at their laid-out size (`inline_svg_at_viewport`), by node:
    /// the size and the parse.
    #[cfg(feature = "svg")]
    pub(crate) viewport_svgs: RefCell<HashMap<NodeId, ((u32, u32), Option<Arc<crate::node::SvgImageData>>)>>,
    pub(crate) has_svg_image_documents: bool,
    /// A resolve ran since the paint tree was last built: `ensure_paint_tree`
    /// rebuilds the nodes flagged `PAINT_TREE_DIRTY` before the next paint or
    /// hit test.
    pub(crate) paint_tree_dirty: bool,
    /// pyro: a resolve ran since the hit bounds were last brought up
    /// to date: `ensure_hit_overflow` computes the boxes flagged
    /// `LayoutData::hit_stale` again before the next hit test.
    pub(crate) hit_overflow_dirty: bool,
    /// pyro: a multi-column container has been laid out in this document (`layout::multicol`;
    /// never cleared): geometry queries look for columns only then.
    pub(crate) has_multicol: bool,

    /// Nodes that contain custom widgets
    #[cfg(feature = "custom-widget")]
    pub(crate) custom_widget_nodes: HashSet<NodeId>,
    /// Rendering resources allocated by custom widgets that should be deallocated during the next render
    #[cfg(feature = "custom-widget")]
    pub(crate) pending_resource_deallocations: Vec<anyrender::ResourceId>,

    /// Cache of loaded images, keyed by URL. Allows reusing images across multiple
    /// elements without re-fetching from the network.
    pub(crate) image_cache: HashMap<String, ImageData>,

    /// Tracks in-flight image requests. When an image is being fetched, additional
    /// requests for the same URL are queued here instead of starting new fetches.
    /// Value is a list of (node_id, image_type) pairs waiting for the image.
    pub(crate) pending_images: HashMap<String, Vec<(NodeId, ImageType)>>,

    /// Nodes whose `background-image`/`mask-image` layers need flushing to
    /// dedicated storage on the node because their style changed (populated by
    /// the style traversal and by pseudo-element box construction).
    pub(crate) pending_style_image_nodes: Vec<NodeId>,

    /// pyro: a style traversal kept a generated `::before` /
    /// `::after` style for some element (sticky; see
    /// `BaseDocument::may_have_generated_before_after`).
    pub(crate) generated_before_after: bool,

    /// pyro: a style traversal kept a `::first-letter` style for some element (sticky): inline
    /// roots look for a first letter to style (`layout::first_letter`).
    pub(crate) first_letter_styles: bool,
    /// pyro: elements whose `::first-letter` style a style traversal changed, for the next
    /// resolve (`layout::first_letter::style_changed`).
    pub(crate) first_letter_changed: Vec<NodeId>,
    /// pyro: the list items with a `::marker` style node the last style traversal restyled
    /// (`layout::list::item_restyled`).
    pub(crate) marker_items_restyled: Vec<NodeId>,
    /// pyro: the scroll pseudo-element boxes may be out of date: a style traversal ran or the
    /// tree changed since `resolve_scroll_markers` (`layout::scroll_markers`).
    pub(crate) scroll_markers_dirty: bool,
    /// pyro: the scroll pseudo-element boxes the pointer is over (`:hover`), innermost first
    /// (`layout::scroll_markers::hover_scroll_pseudos`).
    pub(crate) scroll_pseudo_hover: Vec<NodeId>,
    /// pyro: the scroll container whose marker group has the focus, and whether it shows the focus
    /// ring (`set_focused_scroll_group`).
    pub(crate) focused_scroll_group: Option<(NodeId, bool)>,
    pub(crate) named_anchor_nodes: HashMap<(Option<NodeId>, style::values::DashedIdent), HashSet<NodeId>>,
    pub(crate) popover_anchors: HashMap<NodeId, NodeId>,
    anchor_node_names: HashMap<NodeId, (Option<NodeId>, style::values::specified::position::AnchorNameIdent)>,
    anchor_rule_users: HashMap<Atom, HashSet<usize>>,
    anchor_list_users: HashMap<usize, HashSet<NodeId>>,
    anchor_user_rules: HashMap<NodeId, style::values::specified::position::PositionTryFallbacksList>,
    pub(crate) anchor_lookup_visits: std::cell::Cell<u64>,
    pub(crate) anchor_geometry_steps: std::cell::Cell<u64>,
    pub(crate) anchor_transform_sources: RefCell<HashMap<NodeId, HashSet<NodeId>>>,
    pub(crate) anchor_transform_users: RefCell<HashMap<NodeId, (ServoArc<ComputedValues>, HashSet<NodeId>)>>,
    pub(crate) anchor_transform_dirty: HashSet<NodeId>,
    pub(crate) anchor_index: RefCell<crate::layout::anchor_index::AnchorIndex>,
    pub(crate) anchor_try_visits: std::cell::Cell<u64>,
    pub(crate) anchor_list_visits: std::cell::Cell<u64>,
    pub(crate) anchor_try_scrollports: HashMap<Option<NodeId>, HashSet<NodeId>>,
    pub(crate) anchor_try_sources: HashMap<NodeId, HashSet<Option<NodeId>>>,
    pub(crate) anchor_scroll_dirty: HashSet<NodeId>,
    pub(crate) anchor_last_option: HashMap<NodeId, (usize, style::servo_arc::Arc<style::properties::ComputedValues>)>,
    pub(crate) anchor_flush_choices: HashMap<NodeId, Option<usize>>,
    pub(crate) anchor_rendered_options: HashMap<NodeId, usize>,
    pub(crate) anchor_area_snapshots: RefCell<HashMap<NodeId, ((NodeId, NodeId, style::values::specified::position::PositionArea, Option<usize>), taffy::Point<f32>)>>,
    pub(crate) anchor_pending_options: HashSet<NodeId>,
    /// pyro: the used px of each anchored element's `anchor()`/`anchor-size()` insets and sizes
    /// as its last layout resolved them ([`ANCHOR_USED_SLOTS`]), and, for the ones whose values
    /// changed since the last [`Self::take_anchor_evals`], their values as of that call: a CSS
    /// transition's before-change and after-change values (Chrome computes `anchor()` to px
    /// before it transitions).
    pub(crate) anchor_used: RefCell<HashMap<NodeId, [Option<f32>; ANCHOR_USED_SLOTS]>>,
    pub(crate) anchor_used_changed: RefCell<HashMap<NodeId, [Option<f32>; ANCHOR_USED_SLOTS]>>,
    /// pyro: an element styled with a `scroll-marker-group` that names a side, or with
    /// `scroll-target-group: auto` (sticky).
    pub(crate) scroll_marker_groups: bool,
    /// pyro: the elements with scroll pseudo-element boxes (`ElementData::scroll`).
    pub(crate) scroll_pseudo_owners: std::collections::HashSet<NodeId>,
    pub(crate) transition_trees: Option<Box<style::FxHashMap<NodeId, crate::view_transition::Tree>>>,
    pub transition_paint: Option<Box<crate::view_transition::PaintState>>,
    /// pyro: each element with `scroll-target-group: auto` and the links in its group.
    pub(crate) scroll_target_groups: Vec<(NodeId, Vec<NodeId>)>,
    /// pyro: each scroll container's current scroll target (CSS Overflow 5 § 2.1; the viewport's
    /// under the root element), as the embedder tracks them (`set_scroll_targets`).
    pub(crate) scroll_targets: std::collections::HashMap<NodeId, NodeId>,
    /// pyro: the direction of each scroll container's last scroll on each axis (-1 towards the
    /// start, 1 towards the end, 0 none yet), as the embedder tracks them
    /// (`set_scroll_directions`): `scroll-state(scrolled)`.
    pub(crate) scroll_directions: std::collections::HashMap<NodeId, (i8, i8)>,
    /// pyro: the elements that are a scroll container's snap target, with the axes (x, y) they
    /// are snapped on, as the embedder tracks them (`set_snapped`): `scroll-state(snapped)`.
    pub(crate) snapped_targets: std::collections::HashMap<NodeId, (bool, bool)>,
    /// pyro: the edges each sticky box is held at by its insets (bit 0 top, 1 right, 2 bottom,
    /// 3 left), as the embedder tracks them (`set_stuck`): `scroll-state(stuck)`.
    pub(crate) stuck_targets: std::collections::HashMap<NodeId, u8>,
    /// pyro: a style traversal kept a `::first-line` style for some element (sticky): inline
    /// roots look for the style of their first line (`layout::first_line`).
    pub(crate) first_line_styles: bool,
    /// pyro: elements whose `::first-line` style a style traversal changed, for the next resolve
    /// (`layout::first_line::style_changed`).
    pub(crate) first_line_changed: Vec<NodeId>,
    /// pyro: the embedder's selection, painted in place of `text_selection` (`dom_selection`).
    pub(crate) dom_selection: Option<crate::DomSelection>,
    /// pyro: the embedder's caret (`dom_caret`), painted in its editing host.
    pub(crate) dom_caret: Option<(NodeId, usize)>,
    /// pyro: the element `:target` matches (`set_url_target`).
    pub(crate) url_target: Option<NodeId>,
    /// pyro: the replaced elements the selection holds (painted with its tint).
    pub(crate) selected_replaced: Vec<NodeId>,
    /// pyro: the embedder's highlights (custom highlights and the target text), bottom first
    /// (`dom_highlights`).
    pub(crate) dom_highlights: Vec<(crate::HighlightName, Vec<crate::DomSelection>)>,
    /// pyro: the styles of elements on first lines, cascaded again (`layout::first_line`).
    pub(crate) first_line_reparented: crate::layout::first_line::ReparentCache,

    /// pyro: a style traversal kept a `::before` / `::after` whose content reads counters or
    /// quotes (sticky): each resolve with damage evaluates them (`layout::counters`).
    pub(crate) generated_counters: bool,
    /// pyro: a style traversal styled an element with `text-box-trim` (sticky): inline layouts
    /// look for the trims of their first and last lines (`layout::text_box`).
    pub(crate) text_box_trim_styles: bool,
    /// pyro: a style traversal styled an element `column-span: all` (sticky): a multi-column
    /// container looks for spanners below its children too (`layout::multicol`); without one its
    /// children are all it reads.
    pub(crate) column_span_styles: bool,
    /// pyro: an element was ever styled with a `scroll-snap-type` (snapping is looked for only
    /// then: `mandatory_snap_containers`).
    pub(crate) scroll_snap_styles: bool,
    /// pyro: an element was ever styled `scroll-initial-target: nearest` (initial scroll targets
    /// are looked for only then: `initial_scroll_targets`).
    pub(crate) scroll_initial_target_styles: bool,
    /// pyro: the generated text of each element's `::after` and `::before` with counters and
    /// quotes evaluated (`layout::counters`), while `generated_counters`.
    pub(crate) generated_text: HashMap<NodeId, crate::layout::counters::GeneratedText>,
    /// pyro: a style change or a child list change since the last evaluation can have changed
    /// counters or quotes (`layout::counters`).
    pub(crate) counters_dirty: bool,
    /// pyro: a list item's `::marker` `content` reads a counter other than its number (sticky):
    /// the counter walk evaluates marker contents too (`layout::counters`, `marker_texts`).
    pub(crate) marker_counters: bool,
    /// pyro: `marker_counters` was just set: the resolve runs one more pass, which evaluates them.
    pub(crate) marker_counters_pending: bool,
    /// pyro: per size container placed beside floats, the top of the float-avoiding slot it was last
    /// placed in to be styled at that slot's width: placement goes on from there (css-contain-3
    /// container queries beside floats move forward only; WPT container-queries
    /// `inline-size-bfc-floats`). Cleared at each resolve.
    pub(crate) container_float_floors: std::collections::HashMap<crate::NodeId, f32>,
    /// pyro: the text of each list item's `::marker` `content` that reads counters, evaluated in
    /// tree order (`layout::counters`).
    pub(crate) marker_texts: HashMap<NodeId, String>,
    /// pyro: the number of each list item's marker, its `list-item` counter value
    /// (`layout::counters`): elements, and `::before` (true) / `::after` list items by their
    /// originating element.
    pub(crate) list_item_numbers: (HashMap<NodeId, i32>, HashMap<(NodeId, bool), i32>),

    /// pyro: the top layer (modal dialogs, showing popovers) as
    /// the embedder marks it, by rank (bottom first): elements whose
    /// `urn:x-pyro:state` `state` attribute has a `top-layer=N`
    /// token. Maintained when that attribute is set or cleared; usually
    /// empty, and then painting costs one emptiness check per box.
    pub(crate) top_layer: crate::layout::paint_tree::TopLayer,

    // Tracks in-flight "critical" resources (e.g. stylesheets linked from the `<head>`),
    // keyed by request id
    pub(crate) pending_critical_resources: HashSet<usize>,

    // Service providers
    /// Network provider. Can be used to fetch assets.
    pub net_provider: Arc<dyn NetProvider>,
    /// Navigation provider. Can be used to navigate to a new page (bubbles up the event
    /// on e.g. clicking a Link)
    pub navigation_provider: Arc<dyn NavigationProvider>,
    /// Shell provider. Can be used to request a redraw or set the cursor icon
    pub shell_provider: Arc<dyn ShellProvider>,
    /// HTML parser provider. Used to parse HTML for setInnerHTML
    pub html_parser_provider: Arc<dyn HtmlParserProvider>,
    /// Carried on every sub-resource `Request` this document issues; aborting
    /// it cancels all in-flight fetches tied to this document. Set via
    /// [`DocumentConfig::abort_signal`].
    pub(crate) abort_signal: Option<AbortSignal>,
    /// Devtools provider notified of DOM mutations. Set via
    /// [`DocumentConfig::devtools_provider`] or [`Self::set_devtools_provider`].
    #[cfg(feature = "devtools")]
    pub(crate) devtools_provider: Option<Arc<dyn blitz_traits::devtools::DevtoolsProvider>>,
    /// Title shown for this document in devtools (instead of its `<title>`).
    /// Set via [`Self::set_devtools_title`].
    #[cfg(feature = "devtools")]
    pub(crate) devtools_title: Option<String>,
    /// Reusable buffers for document-wide passes.
    pub(crate) scratch: DocumentScratch,
}

/// Buffers reused across runs of document-wide passes to avoid reallocating
/// them. Recursive passes use them as stacks: each level pushes above the
/// length it found and truncates back to it before returning.
#[derive(Default)]
pub(crate) struct DocumentScratch {
    /// `(paint order, node)` pairs of the paint lists under construction in
    /// `build_paint_tree_impl`.
    pub(crate) keyed_children: Vec<((i32, i32), NodeId)>,
}

bitflags! {
    /// Document-wide work for the next [`resolve`](BaseDocument::resolve).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) struct DocumentFlags: u8 {
        /// Recompute the transform and scrollable overflow of every node, not
        /// only of the damaged ones. Both are kept in device pixels, so a scale
        /// change makes them stale throughout the tree.
        const RESOLVE_ALL_TRANSFORMS = 0b0000_0001;
    }
}

/// pyro: the attribute the render snapshot writes on a sheet link: the key of its text in the
/// snapshot's sheets (by the link, never its URL).
pub const SHEET_ATTRIBUTE: &str = "data-pyro-sheet";

/// pyro: the attribute the render snapshot writes on a sheet link: its response's `Referrer-Policy`.
pub const SHEET_POLICY_ATTRIBUTE: &str = "data-pyro-sheet-referrer-policy";

impl BaseDocument {
    /// Create a new (empty) [`BaseDocument`] with the specified configuration
    pub fn new(config: DocumentConfig) -> Self {
        // pyro: on a grown stack: in a debug build this frame (the document and its Stylist built by
        // value) is about 256 KB, more than a small thread has (a 256 KB layout thread overflowed).
        crate::util::ensure_stack(|| Self::new_inner(config))
    }

    #[inline(never)]
    fn new_inner(config: DocumentConfig) -> Self {
        init_stylo_prefs();

        static ID_GENERATOR: AtomicUsize = AtomicUsize::new(1);

        let id = ID_GENERATOR.fetch_add(1, Ordering::SeqCst);
        // pyro: a frame's document names its frame as the client of every request it makes
        // (`BaseDocument::request_client`), unmodified ones included.
        if config.subdocument_depth > 0 {
            if let Some(net) = config.net_provider.as_ref() {
                net.document_client(id, config.frame_client.clone().unwrap_or_default());
            }
        }

        let font_ctx = config
            .font_ctx
            .map(|mut font_ctx| {
                font_ctx.source_cache.make_shared();
                // font_ctx.collection.make_shared();
                font_ctx
            })
            .unwrap_or_else(|| {
                use parley::fontique::{Collection, CollectionOptions, SourceCache};
                let mut font_ctx = FontContext {
                    source_cache: SourceCache::new_shared(),
                    collection: Collection::new(CollectionOptions {
                        shared: false,
                        system_fonts: cfg!(all(
                            feature = "system-fonts",
                            not(target_arch = "wasm32")
                        )),
                    }),
                };
                font_ctx
                    .collection
                    .register_fonts(Blob::new(Arc::new(crate::BULLET_FONT) as _), None);
                font_ctx
            });
        let font_ctx = Arc::new(Mutex::new(font_ctx));

        let viewport = config.viewport.unwrap_or_default();
        let media_type = config.media_type.unwrap_or_else(MediaType::screen);
        let mut device = make_device(&viewport, None, media_type.clone(), config.media_environment, font_ctx.clone());
        let snapshots = SnapshotMap::new();
        let mut nodes = Box::new(NodeTree::new(config.raster_budget.clone()));
        nodes.xml_document = config.xml_document;
        nodes.quirks_mode = config.quirks_mode;
        nodes.line_height_quirk = config.line_height_quirk;
        nodes.content_language = config.content_language.map(Into::into);
        // pyro: one lock for the thread's documents, so a parsed
        // sheet can serve them all (`make_stylesheet`).
        let guard = crate::util::thread_style_lock();
        let nodes_to_id = HashMap::new();

        let base_url = config
            .base_url
            .and_then(|url| DocumentUrl::from_str(&url).ok())
            .unwrap_or_default();
        let fragment_document = config.document_url.and_then(|text| DocumentUrl::from_str(&text).ok());
        device.set_document_url(fragment_document.as_ref().unwrap_or(&base_url).inline_url_data().clone());
        // pyro: a quirks-mode document matches ID and class selectors ASCII case-insensitively
        // (HTML "case-sensitivity of selectors"; WPT css/selectors/invalidation
        // `quirks-mode-stylesheet-dynamic-add-001`).
        let stylist = Stylist::new(device, if nodes.quirks_mode { QuirksMode::Quirks } else { QuirksMode::NoQuirks });
        nodes.base_url = without_fragment(base_url.as_str()).to_owned();

        let net_provider = config
            .net_provider
            .unwrap_or_else(|| Arc::new(DummyNetProvider));
        let navigation_provider = config
            .navigation_provider
            .unwrap_or_else(|| Arc::new(DummyNavigationProvider));
        let shell_provider = config
            .shell_provider
            .unwrap_or_else(|| Arc::new(DummyShellProvider));
        let html_parser_provider = config
            .html_parser_provider
            .unwrap_or_else(|| Arc::new(DummyHtmlParserProvider));

        let (tx, rx) = channel();

        let mut doc = Self {
            id,
            tx,
            rx: Some(rx),

            guard,
            nodes,
            root_node_id: NodeId::default(),
            stylist,
            animations: DocumentAnimationSet::default(),
            snapshots,
            nodes_to_id,
            viewport,
            media_type,
            media_environment: config.media_environment,
            svg_sprites: HashMap::new(),
            svg_images: Default::default(),
            privacy_svg_images: Default::default(),
            pending_device_changes: DeviceChanges::empty(),
            page_color_scheme: style::values::specified::color::ColorSchemeFlags::empty(),
            meta_nodes: Default::default(),
            last_layout_space: None,
            viewport_scrollbars: ViewportScrollbars::default(),
            layout_ran: false,
            frame_no_scrollbars: false,
            viewport_unit_inset: (0.0, 0.0),
            layout_size: None,
            layout_available: None,
            flags: DocumentFlags::empty(),
            style_threading: config.style_threading,
            incremental_layout: config.incremental.unwrap_or(true),
            subdocument_depth: config.subdocument_depth,
            raster_budget: config.raster_budget,
            frame_client: config.frame_client.clone(),
            frames: config.frames.clone(),
            sheet_policies: Default::default(),
            inline_urls: std::sync::Arc::new(std::sync::Mutex::new(vec![base_url.inline_url_data().clone()])),
            devtool_settings: DevtoolSettings::default(),
            url: base_url,
            fragment_document,
            nodes_to_stylesheet: BTreeMap::new(),
            shadow_sheet_hosts: HashMap::new(),
            linked_page_css: BTreeMap::new(),
            stylesheet_generation: 0,
            font_ctx,
            font_face_families: HashMap::new(),
            #[cfg(feature = "parallel-construct")]
            thread_font_contexts: ThreadLocal::new(),
            layout_ctx: {
                let mut layout_ctx = parley::LayoutContext::new();
                layout_ctx.set_line_break_override(config.line_break_override);
                layout_ctx
            },

            hover_node_id: None,
            hover_hit_node_id: None,
            hover_node_is_text: false,
            last_client_pointer_position: None,
            focus_node_id: None,
            active_node_id: None,
            focus_ring: false,
            mousedown_node_id: None,
            has_active_animations: false,
            sampled_animations: Default::default(),
            subdoc_is_animating: false,
            has_canvas: false,
            sub_document_nodes: HashSet::new(),
            cv_auto: HashSet::new(),
            cv_anchor_uses: RefCell::new(HashMap::new()),
            cv_layout_pass: std::cell::Cell::new(0),
            cv_anchor_frame: HashSet::new(),
            cis_auto: HashSet::new(),
            remembered_size_work: std::cell::Cell::new(0),
            remembered_pending: false,
            cv_forced: HashSet::new(),
            cv_scroll: None,
            cv_resolving: false,
            cv_work: std::cell::Cell::new(0),
            ua_stylesheets: config.ua_stylesheets.clone(),
            frame_animations: Default::default(),
            iframe_loads: HashMap::new(),

            #[cfg(feature = "custom-widget")]
            custom_widget_nodes: HashSet::new(),
            #[cfg(feature = "custom-widget")]
            pending_resource_deallocations: Vec::new(),

            has_changes: false,
            deferred_construction_nodes: Vec::new(),
            run_in_targets: std::collections::HashMap::new(),
            oof_hoisted: HashSet::new(),
            sticky_nodes: HashSet::new(),
            style_changes: Vec::new(),
            scroll_group_moves: Vec::new(),
            timeline_scroll: Default::default(),
            style_change_index: HashMap::new(),
            track_style_changes: false,
            left_layout_lists: Vec::new(),
            svg_foreign_objects: HashMap::new(),
            svg_image_documents: HashMap::new(),
            #[cfg(feature = "svg")]
            viewport_svgs: RefCell::new(HashMap::new()),
            has_svg_image_documents: false,
            paint_tree_dirty: true,
            hit_overflow_dirty: true,
            has_multicol: false,
            image_cache: HashMap::new(),
            pending_images: HashMap::new(),
            pending_style_image_nodes: Vec::new(),
            generated_before_after: false,
            first_letter_styles: false,
            first_letter_changed: Vec::new(),
            marker_items_restyled: Vec::new(),
            scroll_markers_dirty: true,
            scroll_pseudo_hover: Vec::new(),
            focused_scroll_group: None,
            named_anchor_nodes: HashMap::new(),
            popover_anchors: HashMap::new(),
            anchor_node_names: HashMap::new(),
            anchor_rule_users: HashMap::new(),
            anchor_list_users: HashMap::new(),
            anchor_user_rules: HashMap::new(),
            anchor_lookup_visits: Default::default(),
            anchor_geometry_steps: Default::default(),
            anchor_transform_sources: Default::default(),
            anchor_transform_users: Default::default(),
            anchor_transform_dirty: Default::default(),
            anchor_index: Default::default(),
            anchor_try_visits: Default::default(),
            anchor_list_visits: Default::default(),
            anchor_try_scrollports: Default::default(),
            anchor_try_sources: Default::default(),
            anchor_scroll_dirty: Default::default(),
            anchor_last_option: Default::default(),
            anchor_flush_choices: Default::default(),
            anchor_rendered_options: Default::default(),
            anchor_area_snapshots: Default::default(),
            anchor_pending_options: Default::default(),
            anchor_used: Default::default(),
            anchor_used_changed: Default::default(),
            scroll_marker_groups: false,
            scroll_pseudo_owners: Default::default(),
            transition_trees: None,
            transition_paint: None,
            scroll_target_groups: Vec::new(),
            scroll_targets: Default::default(),
            scroll_directions: Default::default(),
            snapped_targets: Default::default(),
            stuck_targets: Default::default(),
            first_line_styles: false,
            first_line_changed: Vec::new(),
            dom_selection: None,
            dom_caret: None,
            url_target: None,
            selected_replaced: Vec::new(),
            dom_highlights: Vec::new(),
            first_line_reparented: Default::default(),
            generated_counters: false,
            text_box_trim_styles: false,
            column_span_styles: false,
            scroll_snap_styles: false,
            scroll_initial_target_styles: false,
            generated_text: HashMap::new(),
            counters_dirty: true,
            marker_counters: false,
            marker_counters_pending: false,
            container_float_floors: Default::default(),
            marker_texts: HashMap::new(),
            list_item_numbers: Default::default(),
            top_layer: Default::default(),
            pending_critical_resources: HashSet::new(),
            controls_to_form: HashMap::new(),
            net_provider,
            navigation_provider,
            shell_provider,
            html_parser_provider,
            abort_signal: config.abort_signal,
            #[cfg(feature = "devtools")]
            devtools_provider: config.devtools_provider,
            #[cfg(feature = "devtools")]
            devtools_title: None,
            last_mousedown_time: None,
            mousedown_position: taffy::Point::ZERO,
            click_count: 0,
            drag_mode: DragMode::None,
            hovered_scrollbar: None,
            scrollbar_activity: HashMap::new(),
            scroll_animation: ScrollAnimationState::None,
            text_selection: TextSelection::default(),
            scratch: DocumentScratch::default(),
        };

        // Initialise document with root Document node
        doc.root_node_id = doc.create_node(NodeData::Document(Box::default()));
        doc.root_node_mut().flags.insert(NodeFlags::IS_IN_DOCUMENT);

        for sheet in config.ua_stylesheets {
            doc.add_user_agent_stylesheet(sheet);
        }

        // Stylo data on the root node container is needed to render the node
        let stylo_element_data = StyloElementData {
            styles: ElementStyles {
                primary: Some(
                    ComputedValues::initial_values_with_font_override(Font::initial_values())
                        .to_arc(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let stylo_data = doc.root_node_mut().stylo_element_data_mut();
        *stylo_data.ensure_init_mut() = stylo_element_data;

        doc
    }

    /// Set the Document's networking provider
    pub fn set_net_provider(&mut self, net_provider: Arc<dyn NetProvider>) {
        self.net_provider = net_provider;
    }

    /// Set the Document's navigation provider
    pub fn set_navigation_provider(&mut self, navigation_provider: Arc<dyn NavigationProvider>) {
        self.navigation_provider = navigation_provider;
    }

    /// Set the Document's shell provider
    pub fn set_shell_provider(&mut self, shell_provider: Arc<dyn ShellProvider>) {
        self.shell_provider = shell_provider;
    }

    /// Set (or clear) the Document's devtools provider, which is notified of DOM mutations
    #[cfg(feature = "devtools")]
    pub fn set_devtools_provider(
        &mut self,
        devtools_provider: Option<Arc<dyn blitz_traits::devtools::DevtoolsProvider>>,
    ) {
        self.devtools_provider = devtools_provider;
    }

    /// The title shown for this document in devtools, if overridden with
    /// [`Self::set_devtools_title`]
    #[cfg(feature = "devtools")]
    pub fn devtools_title(&self) -> Option<&str> {
        self.devtools_title.as_deref()
    }

    /// Set (or clear) the title shown for this document in devtools (e.g. as
    /// the CDP target title). Defaults to the document's `<title>`, which an
    /// application's UI document may use for its window title instead.
    #[cfg(feature = "devtools")]
    pub fn set_devtools_title(&mut self, title: Option<String>) {
        self.devtools_title = title;
    }

    /// Set the Document's html parser provider
    pub fn set_html_parser_provider(&mut self, html_parser_provider: Arc<dyn HtmlParserProvider>) {
        self.html_parser_provider = html_parser_provider;
    }

    /// Set base url for resolving linked resources (stylesheets, images, fonts, etc)
    pub fn set_base_url(&mut self, url: &str) {
        self.url = DocumentUrl::from(Url::parse(url).unwrap());
        self.stylist.device_mut().set_document_url(self.fragment_document.as_ref().unwrap_or(&self.url).inline_url_data().clone());
        crate::net::adopt_inline(&self.inline_urls, self.url.inline_url_data());
        self.nodes.base_url = without_fragment(self.url.as_str()).to_owned();
    }

    /// The base url used for resolving linked resources (stylesheets, images, fonts, etc)
    pub fn base_url(&self) -> &Url {
        &self.url
    }

    pub fn guard(&self) -> &SharedRwLock {
        &self.guard
    }

    pub fn tree(&self) -> &NodeTree {
        &self.nodes
    }

    pub fn id(&self) -> usize {
        self.id
    }

    /// Wrapper around [`crate::net::stamped_request`]. Use the free function
    /// when `&self` would conflict with a held `&mut` borrow on a field.
    pub(crate) fn build_request(&self, url: url::Url) -> Request {
        crate::net::stamped_request(url, self.abort_signal.as_ref())
    }

    pub fn favicon_url(&self) -> Option<String> {
        self.tree().iter().find_map(|(_, node)| {
            let data = &node.data;
            if !data.is_element_with_tag_name(&local_name!("link")) {
                return None;
            }
            let rel = data.attr(local_name!("rel"))?;
            if !rel
                .split_ascii_whitespace()
                .any(|v| v.eq_ignore_ascii_case("icon"))
            {
                return None;
            }
            data.attr(local_name!("href")).map(|s| s.to_string())
        })
    }

    pub fn get_node(&self, node_id: NodeId) -> Option<&Node> {
        self.nodes.get(node_id)
    }

    /// pyro: the `font-family` names that `@font-face` rules registered font file `blob_id` (a
    /// blob id) under.
    pub fn font_face_families(&self, blob_id: u64) -> &[String] {
        self.font_face_families.get(&blob_id).map_or(&[], Vec::as_slice)
    }

    /// pyro: the `@font-palette-values` rules in effect, in cascade order: of two rules for the
    /// same palette name and font family, the later one wins.
    pub fn font_palette_values_rules(
        &self,
    ) -> impl Iterator<Item = &style::servo_arc::Arc<style::stylesheets::FontPaletteValuesRule>> + '_ {
        self.stylist
            .iter_extra_data_origins_rev()
            .flat_map(|(data, _)| data.font_palette_values.iter().map(|(rule, _)| rule))
    }

    pub fn get_node_mut(&mut self, node_id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(node_id)
    }

    pub fn get_focussed_node_id(&self) -> Option<NodeId> {
        self.focus_node_id
            .or(self.try_root_element().map(|el| el.id))
    }

    /// The node holding focus. Unlike [`Self::get_focussed_node_id`] (the node key events
    /// go to), `None` when nothing does.
    pub fn focused_node(&self) -> Option<NodeId> {
        self.focus_node_id
    }

    pub fn mutate<'doc>(&'doc mut self) -> DocumentMutator<'doc> {
        DocumentMutator::new(self)
    }

    pub fn handle_dom_event<F: FnMut(DomEvent)>(
        &mut self,
        event: &mut DomEvent,
        dispatch_event: F,
    ) {
        handle_dom_event(self, event, dispatch_event)
    }

    pub fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// Find the label's bound input elements:
    /// the element id referenced by the "for" attribute of a given label element
    /// or the first input element which is nested in the label
    /// Note that although there should only be one bound element,
    /// we return all possibilities instead of just the first
    /// in order to allow the caller to decide which one is correct
    pub fn label_bound_input_element(&self, label_node_id: NodeId) -> Option<&Node> {
        let label_element = self.nodes[label_node_id].element_data()?;
        if let Some(target_element_dom_id) = label_element.attr(local_name!("for")) {
            TreeTraverser::new(self)
                .filter_map(|id| {
                    let node = self.get_node(id)?;
                    let element_data = node.element_data()?;
                    if element_data.name.local != local_name!("input") {
                        return None;
                    }
                    let id = element_data.id.as_ref()?;
                    if *id == *target_element_dom_id {
                        Some(node)
                    } else {
                        None
                    }
                })
                .next()
        } else {
            TreeTraverser::new_with_root(self, label_node_id)
                .filter_map(|child_id| {
                    let node = self.get_node(child_id)?;
                    let element_data = node.element_data()?;
                    if element_data.name.local == local_name!("input") {
                        Some(node)
                    } else {
                        None
                    }
                })
                .next()
        }
    }

    pub fn toggle_checkbox(el: &mut ElementData) -> bool {
        let Some(is_checked) = el.checkbox_input_checked() else {
            return false;
        };
        let checked = !is_checked;
        el.set_checkbox_input_checked(checked);

        checked
    }

    pub fn toggle_radio(&mut self, radio_set_name: String, target_radio_id: NodeId) {
        let radio_ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter_map(|(i, node)| {
                let el = node.data.downcast_element()?;
                (el.attr(local_name!("name")) == Some(&radio_set_name)
                    && el.checkbox_input_checked().is_some())
                .then_some(i)
            })
            .collect();

        for i in radio_ids {
            let checked = i == target_radio_id;
            self.snapshot_node_and(i, ElementState::CHECKED, |node| {
                if let Some(el) = node.element_data_mut() {
                    el.set_checkbox_input_checked(checked);
                }
                node.mark_ancestors_dirty();
            });
        }
    }

    /// Toggle the `open` attribute of a `<details>` element, expanding or
    /// collapsing it. This is the default action triggered when the element's
    /// first `<summary>` child is activated.
    pub fn toggle_details_open(&mut self, details_id: NodeId) {
        use crate::qual_name;

        let node = &self.nodes[details_id];
        if !node.data.is_element_with_tag_name(&local_name!("details")) {
            return;
        }
        let is_open = node.data.has_attr(local_name!("open"));

        // Note: HTML attributes are in the empty (null) namespace, so the
        // QualName must not use the html namespace here, else it won't match
        // an `open` attribute created by the HTML parser.
        let mut mutator = self.mutate();
        if is_open {
            mutator.clear_attribute(details_id, qual_name!("open"));
        } else {
            mutator.set_attribute(details_id, qual_name!("open"), "");
        }
        drop(mutator);

        self.shell_provider.request_redraw();
    }

    pub fn set_style_property(&mut self, node_id: NodeId, name: &str, value: &str) {
        let node = &mut self.nodes[node_id];
        let did_change = node.element_data_mut().unwrap().set_style_property(
            name,
            value,
            &self.guard,
            self.url.url_extra_data(),
        );
        if did_change {
            node.set_restyle_hint(RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        }
    }

    pub fn remove_style_property(&mut self, node_id: NodeId, name: &str) {
        let node = &mut self.nodes[node_id];
        let did_change = node.element_data_mut().unwrap().remove_style_property(
            name,
            &self.guard,
            self.url.url_extra_data(),
        );
        if did_change {
            node.set_restyle_hint(RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        }
    }

    pub fn sub_document_node_ids(&self) -> Vec<NodeId> {
        self.sub_document_nodes.iter().copied().collect()
    }

    pub fn set_sub_document(&mut self, node_id: NodeId, sub_document: Box<dyn Document>) {
        self.nodes[node_id]
            .element_data_mut()
            .unwrap()
            .set_sub_document(sub_document);
        self.sub_document_nodes.insert(node_id);
    }

    pub fn remove_sub_document(&mut self, node_id: NodeId) {
        self.nodes[node_id]
            .element_data_mut()
            .unwrap()
            .remove_sub_document();
        self.sub_document_nodes.remove(&node_id);
        if let Some(load) = self.iframe_loads.remove(&node_id) {
            load.abort_controller.abort();
        }
    }

    /// Poll all sub-documents (see [`Document::poll`]), allowing them to make progress
    /// on any pending async operations (e.g. JavaScript timers). Hosts which poll a
    /// wrapper around a [`BaseDocument`] should call this from their `poll` implementation.
    ///
    /// Returns `true` if any sub-document reported changes.
    pub fn poll_subdocuments(&mut self, waker: Option<&Waker>) -> bool {
        let mut has_changes = false;
        for node_id in self.sub_document_nodes.iter().copied() {
            let Some(sub_doc) = self
                .nodes
                .get_mut(node_id)
                .and_then(|node| node.subdoc_mut())
            else {
                continue;
            };
            let task_context = waker.map(TaskContext::from_waker);
            has_changes |= sub_doc.poll(task_context);
        }
        has_changes
    }

    #[cfg(feature = "custom-widget")]
    pub fn custom_widget_node_ids(&self) -> Vec<NodeId> {
        self.custom_widget_nodes.iter().copied().collect()
    }

    #[cfg(feature = "custom-widget")]
    pub fn take_pending_resource_deallocations(&mut self) -> Vec<anyrender::ResourceId> {
        std::mem::take(&mut self.pending_resource_deallocations)
    }

    #[cfg(feature = "custom-widget")]
    pub fn set_custom_widget(&mut self, node_id: NodeId, widget: Box<dyn crate::Widget>) {
        self.nodes[node_id]
            .element_data_mut()
            .unwrap()
            .set_custom_widget(widget);
        self.custom_widget_nodes.insert(node_id);
    }

    #[cfg(feature = "custom-widget")]
    pub fn remove_custom_widget(&mut self, node_id: NodeId) {
        let resources_to_deallocate = self.nodes[node_id]
            .element_data_mut()
            .unwrap()
            .remove_custom_widget();
        self.pending_resource_deallocations
            .extend_from_slice(&resources_to_deallocate);
        self.custom_widget_nodes.remove(&node_id);
    }

    pub fn root_node(&self) -> &Node {
        &self.nodes[self.root_node_id]
    }

    pub fn root_node_mut(&mut self) -> &mut Node {
        &mut self.nodes[self.root_node_id]
    }

    pub fn try_root_element(&self) -> Option<&Node> {
        TDocument::as_node(&self.root_node()).first_element_child()
    }

    pub fn root_element(&self) -> &Node {
        TDocument::as_node(&self.root_node())
            .first_element_child()
            .unwrap()
            .as_element()
            .unwrap()
    }

    pub fn create_node(&mut self, node_data: NodeData) -> NodeId {
        let tree_ptr = self.nodes.as_mut() as *mut NodeTree;
        let guard = self.guard.clone();

        let id = self
            .nodes
            .insert_with_key(|id| Node::new(tree_ptr, id, guard, node_data));

        self.has_changes = true;
        id
    }

    /// Remove a node from the node tree, clearing any interaction state
    /// (hover/active/focus/mousedown/selection/drag/scrollbar) that references
    /// it so that stale NodeIds are never dereferenced after the slot is freed.
    pub(crate) fn remove_node_from_tree(&mut self, node_id: NodeId) -> Option<Node> {
        if self.transition_trees.is_some() { self.clear_transition_tree(node_id); }
        self.clear_interaction_state_for_removed_node(node_id);
        // pyro: a dropped element leaves the top layer.
        if !self.top_layer.is_empty() {
            self.top_layer.remove(node_id);
        }
        // pyro: a freed box leaves the hoisted list of its containing block, and the boxes it
        // was the containing block of stop pointing at it: either id would be a stale key there.
        if let Some(node) = self.nodes.get(node_id) {
            let cb = node.oof_containing_block.take();
            let hoisted = std::mem::take(&mut *node.hoisted_children.borrow_mut());
            if let Some(cb) = cb {
                self.unlist_hoisted(cb, node_id);
            }
            for id in hoisted {
                if let Some(child) = self.nodes.get(id)
                    && child.oof_containing_block.get() == Some(node_id)
                {
                    child.oof_containing_block.set(None);
                    self.oof_hoisted.remove(&id);
                }
            }
        }
        self.update_anchor_names(node_id, None);
        self.update_anchor_rule_users(node_id, None);
        self.clear_anchor_try_sources(node_id);
        self.clear_anchor_transform_sources(node_id);
        if let Some((_, blocks)) = self.anchor_transform_users.get_mut().remove(&node_id) {
            for block in blocks { if let Some(sources) = self.anchor_transform_sources.get_mut().get_mut(&block) { sources.remove(&node_id); } }
        }
        self.anchor_transform_dirty.remove(&node_id);
        self.anchor_last_option.remove(&node_id);
        self.anchor_area_snapshots.get_mut().remove(&node_id);
        self.anchor_used.get_mut().remove(&node_id);
        self.anchor_used_changed.get_mut().remove(&node_id);
        self.anchor_rendered_options.remove(&node_id);
        self.anchor_pending_options.remove(&node_id);
        self.anchor_scroll_dirty.remove(&node_id);
        if let Some(blocks) = self.anchor_try_scrollports.remove(&Some(node_id)) {
            for block in blocks { if let Some(sources) = self.anchor_try_sources.get_mut(&block) { sources.remove(&Some(node_id)); } }
        }
        self.nodes.remove(node_id)
    }

    pub(crate) fn update_anchor_names(&mut self, node_id: NodeId, names: Option<(Option<NodeId>, style::values::specified::position::AnchorNameIdent)>) {
        if names.is_none() && self.anchor_node_names.is_empty() { return; }
        if self.anchor_node_names.get(&node_id) == names.as_ref() { return; }
        self.anchor_index.get_mut().clear();
        if let Some((scope, old)) = self.anchor_node_names.remove(&node_id) {
            for name in old.0.iter() {
                let key = (scope, name.clone());
                if let Some(ids) = self.named_anchor_nodes.get_mut(&key) {
                    ids.remove(&node_id);
                    if ids.is_empty() { self.named_anchor_nodes.remove(&key); }
                }
            }
        }
        if let Some((scope, names)) = names {
            for name in names.0.iter() { self.named_anchor_nodes.entry((scope, name.clone())).or_default().insert(node_id); }
            self.anchor_node_names.insert(node_id, (scope, names));
        }
    }

    pub fn anchor_candidate_visits(&self) -> u64 { self.anchor_lookup_visits.get() }

    pub fn anchor_query_samples(&self) -> u64 { self.nodes.anchor_query_samples.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn anchor_geometry_steps(&self) -> u64 { self.anchor_geometry_steps.get() }

    pub(crate) fn clear_anchor_transform_sources(&mut self, block: NodeId) {
        if self.anchor_transform_sources.get_mut().is_empty() { return; }
        if let Some(sources) = self.anchor_transform_sources.get_mut().remove(&block) {
            let users = self.anchor_transform_users.get_mut();
            for source in sources {
                if let Some((_, blocks)) = users.get_mut(&source) {
                    blocks.remove(&block);
                    if blocks.is_empty() { users.remove(&source); }
                }
            }
        }
    }

    pub(crate) fn update_anchor_rule_users(&mut self, id: NodeId, rules: Option<style::values::specified::position::PositionTryFallbacksList>) {
        use style::values::specified::position::{PositionTryFallbacksList, PositionTryFallbacksItem};
        fn names(list: &PositionTryFallbacksList) -> impl Iterator<Item=&Atom> {
            list.0.iter().filter_map(|item| match item {
                PositionTryFallbacksItem::IdentAndOrTactic(item) if !item.ident.is_empty() => Some(&item.ident.0),
                _ => None,
            })
        }
        if rules.is_none() && self.anchor_user_rules.is_empty() { return; }
        if self.anchor_user_rules.get(&id).map(|list| list.0.as_ptr()) == rules.as_ref().map(|list| list.0.as_ptr()) { return; }
        if let Some(old) = self.anchor_user_rules.remove(&id) {
            self.anchor_area_snapshots.get_mut().remove(&id);
            self.anchor_rendered_options.remove(&id);
            self.anchor_last_option.remove(&id);
            self.anchor_pending_options.remove(&id);
            let key = old.0.as_ptr() as usize;
            if let Some(users) = self.anchor_list_users.get_mut(&key) {
                users.remove(&id);
                if users.is_empty() {
                    self.anchor_list_users.remove(&key);
                    for name in names(&old) {
                        if let Some(lists) = self.anchor_rule_users.get_mut(name) {
                            lists.remove(&key);
                            if lists.is_empty() { self.anchor_rule_users.remove(name); }
                        }
                    }
                }
            }
        }
        if let Some(list) = rules {
            let key = list.0.as_ptr() as usize;
            let users = self.anchor_list_users.entry(key).or_default();
            if users.is_empty() {
                for name in names(&list) { self.anchor_rule_users.entry(name.clone()).or_default().insert(key); }
            }
            users.insert(id);
            self.anchor_user_rules.insert(id, list);
        }
    }

    pub(crate) fn invalidate_anchor_rule_users(&mut self, names: impl IntoIterator<Item=Atom>) {
        if self.anchor_rule_users.is_empty() { return; }
        let mut blocks = HashSet::new();
        let lists: HashSet<_> = names.into_iter().filter_map(|name| self.anchor_rule_users.get(&name)).flatten().copied().collect();
        for key in lists {
            if let Some(users) = self.anchor_list_users.get(&key) {
                for id in users {
                    if let Some(node) = self.nodes.get(*id).filter(|node| node.flags.is_in_document()) {
                        blocks.extend(node.containing_block());
                        self.anchor_last_option.remove(id);
                        self.anchor_area_snapshots.get_mut().remove(id);
                        self.anchor_rendered_options.remove(id);
                        self.anchor_pending_options.remove(id);
                    }
                }
            }
        }
        for id in blocks { self.nodes[id].insert_damage(crate::layout::damage::ONLY_RELAYOUT); }
    }
    pub fn commit_anchor_frame(&mut self) {
        if self.anchor_pending_options.is_empty() { return; }
        for id in self.anchor_pending_options.drain() {
            if let Some((index,_)) = self.anchor_last_option.get(&id) { self.anchor_rendered_options.insert(id,*index); }
            else { self.anchor_rendered_options.remove(&id); }
        }
    }
    pub fn anchor_index_work(&self) -> u64 { self.anchor_index.borrow().work.get() }
    pub fn anchor_boundary_entries(&self) -> usize { self.anchor_index.borrow().boundary_entries() }
    pub fn has_anchor_sources(&self) -> bool { !self.named_anchor_nodes.is_empty() || !self.popover_anchors.is_empty() }

    pub fn set_popover_anchors(&mut self, anchors: HashMap<NodeId, NodeId>) {
        if self.popover_anchors == anchors { return; }
        let old = std::mem::replace(&mut self.popover_anchors, anchors);
        for id in old.keys().chain(self.popover_anchors.keys()) {
            if old.get(id) == self.popover_anchors.get(id) { continue; }
            if let Some(node) = self.nodes.get_mut(*id) {
                let block = node.containing_block();
                node.insert_damage(crate::layout::damage::ONLY_RELAYOUT);
                if let Some(block) = block.and_then(|id| self.nodes.get_mut(id)) { block.insert_damage(crate::layout::damage::ONLY_RELAYOUT); }
            }
        }
    }
    pub fn anchor_try_visits(&self) -> u64 { self.anchor_try_visits.get() }
    pub fn anchor_list_visits(&self) -> u64 { self.anchor_list_visits.get() }
    pub fn anchor_clip_steps(&self) -> u64 { self.nodes.anchor_clip_steps.load(std::sync::atomic::Ordering::Relaxed) }

    pub(crate) fn clear_anchor_try_sources(&mut self, block: NodeId) {
        if self.anchor_try_sources.is_empty() { return; }
        if let Some(sources) = self.anchor_try_sources.remove(&block) {
            for source in sources {
                if let Some(blocks) = self.anchor_try_scrollports.get_mut(&source) {
                    blocks.remove(&block);
                    if blocks.is_empty() { self.anchor_try_scrollports.remove(&source); }
                }
            }
        }
    }

    /// pyro: takes `id` out of the hoisted list of `containing_block` and lays that box out
    /// again: it lists the boxes it laid out, and `id` is no longer one of them.
    fn unlist_hoisted(&mut self, containing_block: NodeId, id: NodeId) {
        self.oof_hoisted.remove(&id);
        let Some(cb) = self.nodes.get_mut(containing_block) else { return };
        let listed = {
            let mut list = cb.hoisted_children.borrow_mut();
            let before = list.len();
            list.retain(|&child| child != id);
            list.len() != before
        };
        if listed {
            cb.insert_damage(crate::layout::damage::ONLY_RELAYOUT);
        }
    }

    /// pyro: `root`'s subtree leaves its place in the tree (removed or moved). Its boxes that
    /// a containing block outside it laid out leave that block's hoisted list: the block is
    /// not laid out again when the subtree went with no damage inside the block (a detached
    /// subtree's own damage reaches no ancestor), and an id it kept would outlive the box
    /// (`invalid SlotMap key` in rounding once the box is dropped).
    pub(crate) fn unlist_hoisted_leaving(&mut self, root: NodeId) {
        if self.oof_hoisted.is_empty() {
            return;
        }
        let mut leaving = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get(id) else { continue };
            if let Some(cb) = node.oof_containing_block.get() {
                // Inside the subtree: `cb` is `id` or one of its ancestors up to `root`.
                let mut inside = false;
                let mut at = Some(id);
                while let Some(a) = at {
                    if a == cb {
                        inside = true;
                        break;
                    }
                    if a == root {
                        break;
                    }
                    at = self.nodes.get(a).and_then(|n| n.parent);
                }
                if !inside {
                    leaving.push((cb, id));
                }
            }
            stack.extend(node.children.iter().copied());
            stack.extend(node.before());
            stack.extend(node.after());
        }
        for (cb, id) in leaving {
            if let Some(node) = self.nodes.get(id) {
                node.oof_containing_block.set(None);
            }
            self.unlist_hoisted(cb, id);
        }
    }

    /// The nearest element ancestor of `node_id` that is still in the
    /// document. Used to retarget hover/active state when the node they
    /// reference is removed. Tolerates already-removed ancestors (subtree
    /// teardown proceeds root-first) by giving up and returning `None`.
    fn nearest_surviving_element_ancestor(&self, node_id: NodeId) -> Option<NodeId> {
        let mut current = self.get_node(node_id)?.parent;
        while let Some(id) = current {
            let node = self.get_node(id)?;
            if node.is_element() && node.flags.is_in_document() {
                return Some(id);
            }
            current = node.parent;
        }
        None
    }

    /// Clear any interaction state (hover/active/focus/mousedown/selection/
    /// drag/scrollbar) that references `node_id`, which is being removed from
    /// the document, running the usual teardown steps. `node_id` must still be
    /// present in the slab.
    ///
    /// This matches browser semantics (WebKit `hoveredElementDidDetach` /
    /// `elementInActiveChainDidDetach`, Blink `HoveredElementDetached` /
    /// `ActiveChainNodeDetached`):
    /// - Hover and active retarget to the nearest surviving element ancestor
    ///   as a *transient bridge*: the HOVER/ACTIVE element-state bits along
    ///   the surviving chain stay lit (no one-frame gap in `:hover`/`:active`
    ///   styling), and the subsequent hover diff can unset exactly the right
    ///   bits. Hover is then re-resolved against the pointer position by
    ///   [`Self::refresh_hover`] at the end of the next resolve pass (the
    ///   analogue of WebKit's "fake mouse move"), which corrects the bridge
    ///   value — including cases where the removed node overflowed its
    ///   ancestor's box, so the ancestor was never truly under the pointer.
    /// - Focus resets to the body (encoded as `None`), running blur
    ///   side-effects (clearing focus element state and disabling IME for
    ///   text inputs).
    pub(crate) fn clear_interaction_state_for_removed_node(&mut self, node_id: NodeId) {
        if !self.nodes.contains_key(node_id) {
            return;
        }

        if self.hover_node_id == Some(node_id) {
            self.hover_node_id = self.nearest_surviving_element_ancestor(node_id);
            self.hover_node_is_text = false;
        }
        if self.hover_hit_node_id == Some(node_id) {
            self.hover_hit_node_id = None;
        }
        if self.active_node_id == Some(node_id) {
            self.active_node_id = self.nearest_surviving_element_ancestor(node_id);
        }
        if self.focus_node_id == Some(node_id) {
            let shell_provider = self.shell_provider.clone();
            self.nodes[node_id].blur(shell_provider);
            // Nothing on the chain contains the focus anymore. Only the surviving
            // ancestors need a snapshot for restyling: nodes in the removed subtree had
            // theirs dropped already and may be freed entirely.
            let mut maybe_id = Some(node_id);
            while let Some(node) = maybe_id.and_then(|id| self.get_node(id)) {
                let id = node.id;
                maybe_id = node.parent;
                if node.flags.is_in_document() {
                    self.snapshot_node_and(id, ElementState::FOCUS_WITHIN, |node| {
                        node.remove_focus_within()
                    });
                } else {
                    self.nodes[id].remove_focus_within();
                }
            }
            self.focus_node_id = None;
        }
        if self.mousedown_node_id == Some(node_id) {
            self.mousedown_node_id = None;
        }
        if self.text_selection.anchor.node_or_parent == Some(node_id)
            || self.text_selection.focus.node_or_parent == Some(node_id)
        {
            self.text_selection.clear();
        }
        if self
            .hovered_scrollbar
            .is_some_and(|scrollbar| scrollbar.node_id == node_id)
        {
            self.hovered_scrollbar = None;
        }
        let drag_references_node = match &self.drag_mode {
            DragMode::Panning(state) => state.target == node_id,
            DragMode::ScrollbarDrag(state) => state.scrollbar.node_id == node_id,
            DragMode::Selecting | DragMode::None => false,
        };
        if drag_references_node {
            self.drag_mode = DragMode::None;
        }
        self.scrollbar_activity.remove(&node_id);
    }

    pub(crate) fn drop_node_ignoring_parent(&mut self, node_id: NodeId) -> Option<Node> {
        self.drop_node_ignoring_parent_with(node_id, &mut |_| {})
    }

    /// Like [`Self::drop_node_ignoring_parent`], but calls `on_drop` with the id of
    /// every dropped node (the node itself and all of its descendants).
    pub(crate) fn drop_node_ignoring_parent_with(
        &mut self,
        node_id: NodeId,
        on_drop: &mut dyn FnMut(NodeId),
    ) -> Option<Node> {
        let mut node = self.remove_node_from_tree(node_id);
        if let Some(node) = &mut node {
            on_drop(node_id);
            if let Some(before) = node.before() {
                crate::util::ensure_stack(|| self.drop_node_ignoring_parent_with(before, on_drop));
            }
            if let Some(after) = node.after() {
                crate::util::ensure_stack(|| self.drop_node_ignoring_parent_with(after, on_drop));
            }
            // pyro: the node holding its `::first-letter` style (with the text of an initial
            // letter box).
            if let Some(first_letter) = node.element_data().and_then(|element| element.first_letter)
                && let Some(letter) = self.remove_node_from_tree(first_letter)
            {
                for &child in &letter.children {
                    self.remove_node_from_tree(child);
                }
            }
            // pyro: its `::marker` style.
            if let Some(marker_style) = node.element_data().and_then(|element| element.marker_style) {
                self.remove_node_from_tree(marker_style);
            }
            // pyro: its scroll pseudo-element boxes (its group with the markers in it, its
            // buttons), and its marker, which leaves its group.
            if let Some(pseudos) = node.element_data().and_then(|element| element.scroll.as_deref()).cloned() {
                for owned in pseudos.group.into_iter().chain(pseudos.buttons.iter().flatten().copied()).chain(pseudos.column) {
                    self.drop_scroll_pseudo_node(owned);
                }
                if let Some(marker) = pseudos.marker
                    && self.nodes.get(marker).is_some_and(|marker| marker.scroll_pseudo().is_some_and(|(_, origin)| origin == node_id))
                {
                    let group = self.nodes[marker].parent;
                    self.drop_scroll_pseudo_node(marker);
                    if let Some(group) = group.and_then(|group| self.nodes.get_mut(group)) {
                        group.children.retain(|&id| id != marker);
                        group.insert_damage(crate::layout::damage::ALL_DAMAGE);
                        group.mark_damaged();
                    }
                }
                self.scroll_markers_dirty = true;
            }
            // pyro: and its `::first-line` style.
            if let Some(first_line) = node.element_data().and_then(|element| element.first_line) {
                self.remove_node_from_tree(first_line);
            }

            for &child in &node.children {
                crate::util::ensure_stack(|| self.drop_node_ignoring_parent_with(child, on_drop));
            }

            // Anonymous blocks live only in the slab, so deallocate the ones this
            // node owns rather than leaking them.
            for &anon_id in &node.anonymous_blocks {
                self.deallocate_anonymous_block(anon_id);
            }
        }
        node
    }

    /// Deallocate an anonymous block created in a previous construction
    /// round, along with any anonymous blocks nested within it.
    pub(crate) fn deallocate_anonymous_block(&mut self, anon_id: NodeId) {
        // The block may already have been removed from the slab (e.g. a
        // whitespace-only anonymous block dropped during construction).
        if !self.nodes.contains_key(anon_id) {
            return;
        }

        // Free any anonymous blocks that this block owns before removing it.
        let nested = std::mem::take(&mut self.nodes[anon_id].anonymous_blocks);
        for nested_id in nested {
            crate::util::ensure_stack(|| self.deallocate_anonymous_block(nested_id));
        }

        // pyro: the boxes it wrapped or laid out have it as their layout parent no more (a box
        // `display: none` since keeps no pointer to a freed one; a fresh build has none).
        let wrapped: Vec<NodeId> = self.nodes[anon_id]
            .children
            .iter()
            .copied()
            .chain(self.nodes[anon_id].layout_children.borrow().iter().flatten().copied())
            .collect();
        for id in wrapped {
            if let Some(node) = self.nodes.get(id) {
                if node.layout_parent.get() == Some(anon_id) {
                    node.layout_parent.set(None);
                }
                if node.layout_child_of.get() == Some(anon_id) {
                    node.layout_child_of.set(None);
                }
            }
        }
        // pyro: so have the boxes of a split inline's head or tail it laid out, which are not its
        // children but the split inline's (`find_inline_layout_embedded_boxes`).
        let anon = &self.nodes[anon_id];
        if anon
            .flags
            .intersects(crate::node::NodeFlags::ENDS_WITH_SPLIT_HEAD | crate::node::NodeFlags::STARTS_WITH_SPLIT_TAIL)
        {
            let len = anon.children.len();
            let mut pending: Vec<NodeId> = anon
                .children
                .iter()
                .copied()
                .enumerate()
                .filter(|&(index, _)| anon.split_ref_at(index, len).is_some())
                .map(|(_, id)| id)
                .collect();
            while let Some(id) = pending.pop() {
                let Some(node) = self.nodes.get(id) else { continue };
                for child_id in node.before().into_iter().chain(node.children.iter().copied()).chain(node.after()) {
                    let Some(child) = self.nodes.get(child_id) else { continue };
                    if child.layout_parent.get() == Some(anon_id) {
                        child.layout_parent.set(None);
                    }
                    if child.layout_child_of.get() == Some(anon_id) {
                        child.layout_child_of.set(None);
                    }
                    if child.is_element() {
                        pending.push(child_id);
                    }
                }
            }
        }

        self.remove_node_from_tree(anon_id);
    }

    /// pyro: `node_id` is out of the layout tree (no container lists it: it is
    /// `display: none` or below one, a part a table ignores, or out of the document), and so is
    /// what its subtree built, as a fresh build has none there: its layout children that still
    /// point at it leave its list (`Node::layout_child_of`), and theirs in turn, and its anonymous
    /// blocks are freed. A box that comes back is built again: styled from scratch (below
    /// `display: none`), added (`process_added_subtree`), or damaged on its way in (a table's
    /// cells, `enter_layout_tree`; a former table's other children, `damage_former_table_parts`).
    pub(crate) fn leave_layout_tree(&mut self, node_id: NodeId) {
        let Some(node) = self.nodes.get(node_id) else { return };
        let anonymous = node.is_anonymous();
        let children = node.layout_children.borrow_mut().take();
        for child_id in children.into_iter().flatten() {
            let Some(child) = self.nodes.get(child_id) else { continue };
            if child.layout_child_of.get() != Some(node_id) {
                continue;
            }
            child.layout_child_of.set(None);
            // pyro: an anonymous box leaving the layout tree is freed with its owner's anonymous
            // boxes: its children keep no pointer to it.
            if anonymous && child.layout_parent.get() == Some(node_id) {
                child.layout_parent.set(None);
            }
            crate::util::ensure_stack(|| self.leave_layout_tree(child_id));
        }
        let anonymous_blocks = std::mem::take(&mut self.nodes[node_id].anonymous_blocks);
        for anon_id in anonymous_blocks {
            self.deallocate_anonymous_block(anon_id);
        }
    }

    /// Whether the DOM or focus changed since the last [`Self::clear_changes`]
    /// (i.e. whether the accessibility tree is out of date)
    pub fn has_changes(&self) -> bool {
        self.has_changes
    }

    /// Mark the accessibility tree as up to date
    pub fn clear_changes(&mut self) {
        self.has_changes = false;
    }

    pub fn create_text_node(&mut self, text: &str) -> NodeId {
        let content = text.to_string();
        let data = NodeData::Text(TextNodeData::new(content));
        self.create_node(data)
    }

    pub fn deep_clone_node(&mut self, node_id: NodeId) -> NodeId {
        // Load existing node
        let node = &self.nodes[node_id];
        let mut data = node.data.clone();

        match &mut data {
            NodeData::Element(elem) | NodeData::AnonymousBlock(elem) => {
                if let Some(arc) = elem.style_attribute.as_mut() {
                    let read_guard = self.guard().read();
                    let block = arc.read_with(&read_guard);
                    *arc = ServoArc::new(self.guard().wrap(block.clone()));
                }
            }
            _ => {}
        }

        let children = node.children.clone();

        // Create new node
        let new_node_id = self.create_node(data);

        // Recursively clone children
        let new_children: ThinVec<NodeId> = children
            .into_iter()
            .map(|child_id| self.deep_clone_node(child_id))
            .collect();
        for &child_id in &new_children {
            self.nodes[child_id].parent = Some(new_node_id);
        }
        self.nodes[new_node_id].children = new_children;

        new_node_id
    }

    pub(crate) fn remove_and_drop_pe(&mut self, node_id: NodeId) -> Option<Node> {
        fn remove_pe_ignoring_parent(doc: &mut BaseDocument, node_id: NodeId) -> Option<Node> {
            let mut node = doc.remove_node_from_tree(node_id);
            if let Some(node) = &mut node {
                for &child in &node.children {
                    crate::util::ensure_stack(|| remove_pe_ignoring_parent(doc, child));
                }
                for &anon_id in &node.anonymous_blocks {
                    doc.deallocate_anonymous_block(anon_id);
                }
            }
            node
        }

        let node = remove_pe_ignoring_parent(self, node_id);

        // Update child_idx values
        if let Some(parent_id) = node.as_ref().and_then(|node| node.parent) {
            let parent = &mut self.nodes[parent_id];
            parent.children.retain(|id| *id != node_id);
        }

        node
    }

    pub(crate) fn resolve_url(&self, raw: &str) -> Option<url::Url> {
        self.url.resolve_relative(raw)
    }

    pub fn print_tree(&self) {
        crate::util::walk_tree(0, self.root_node());
    }

    pub fn print_subtree(&self, node_id: NodeId) {
        crate::util::walk_tree(0, &self.nodes[node_id]);
    }

    pub fn reload_resource_by_href(&mut self, href_to_reload: &str) {
        for &node_id in self.nodes_to_stylesheet.keys() {
            let node = &self.nodes[node_id];
            let Some(element) = node.element_data() else {
                continue;
            };

            if element.name.local == local_name!("link") {
                if let Some(href) = element.attr(local_name!("href")) {
                    // println!("Node {node_id} {href} {href_to_reload} {} {}", resolved_href.as_str(), resolved_href.as_str() == url_to_reload);
                    if href == href_to_reload
                        && let Some(resolved_href) = self.resolve_url(href)
                    {
                        let mut request = self.build_request(resolved_href.clone());
                        request.options.destination = blitz_traits::net::Destination::Style;
                        let marked = crate::engine_marked(&self.frames.nonce, element);
                        request.options.sheet =
                            element.reserved_attr(SHEET_ATTRIBUTE).filter(|_| marked).map(str::to_owned);
                        self.net_provider.fetch(
                            self.id(),
                            request,
                            ResourceHandler::boxed(
                                self.tx.clone(),
                                self.id,
                                Some(node_id),
                                self.shell_provider.clone(),
                                StylesheetHandler {
                                    source_url: resolved_href,
                                    guard: self.guard.clone(),
                                    net_provider: self.net_provider.clone(),
                                    abort_signal: self.abort_signal.clone(),
                                    client: self.request_client(),
                                    policy: element
                                        .reserved_attr(SHEET_POLICY_ATTRIBUTE)
                                        .filter(|_| marked)
                                        .map(str::to_owned),
                                    policies: self.sheet_policies.clone(),
                                    quirks_mode: self.sheet_quirks_mode(),
                                },
                            ),
                        );
                    }
                }
            }
        }
    }

    pub fn process_style_element(&mut self, target_id: NodeId) {
        // pyro: the sheet is the element's text as it is. Character references were decoded by
        // the parser where the markup has them (XML text, SVG `<style>` in HTML); an HTML
        // `<style>` is raw text and an XML CDATA section is literal, so a `&#x46;` left in
        // either stays four characters, as in every browser (WPT css/CSS2 content-177).
        let css = self.nodes[target_id].text_content();
        let host = self.nodes[target_id].containing_shadow_host_id();
        // pyro: a `media` attribute is the sheet's media list (HTML `<style media>`): a sheet
        // whose media do not match applies nothing. Parsed apart (not the shared sheet of its
        // text, whose media list is empty).
        // pyro: a sheet with `@scope` rules is parsed for its element alone, with the implicit
        // scope root of its prelude-less ones (CSS Cascade 6 § 2.5: the element's parent, or the
        // shadow host when the element is at its shadow tree's top), not shared with other
        // elements or shadow trees (WPT css-cascade `scope-part`, `scope-shadow-sharing`).
        let implicit_scope_root = sheet_has_scope_rules(&css).then(|| self.implicit_scope_root_of(target_id));
        let sheet = match (self.node_media_list(target_id), implicit_scope_root) {
            (media, Some(root)) => {
                let mut sheet =
                    self.parse_stylesheet_with_media(&css, Origin::Author, media.unwrap_or_else(MediaList::empty));
                sheet.implicit_scope_root = root;
                DocumentStyleSheet(ServoArc::new(sheet))
            }
            (Some(media), None) => {
                DocumentStyleSheet(ServoArc::new(self.parse_stylesheet_with_media(&css, Origin::Author, media)))
            }
            (None, None) => self.make_stylesheet_in(&css, Origin::Author, host),
        };
        self.add_stylesheet_for_node(sheet, target_id);
    }

    /// pyro: the implicit scope root of the `@scope` rules in the sheet of `owner` (a `<style>`):
    /// its parent element (`InLightTree`/`InShadowTree`), the host of the shadow tree whose top
    /// it is in (`ShadowHost`), or none (a `<style>` that is no element's child).
    fn implicit_scope_root_of(&self, owner: NodeId) -> Option<style::stylesheets::scope_rule::ImplicitScopeRoot> {
        use style::stylesheets::scope_rule::ImplicitScopeRoot;
        let opaque = |id: NodeId| {
            selectors::OpaqueElement::from_ptr(
                std::ptr::NonNull::new((id.as_u64() as usize).wrapping_add(1) as *mut ()).unwrap(),
            )
        };
        let node = &self.nodes[owner];
        let parent = node.parent?;
        if node.parent_is_shadow_root() {
            return Some(ImplicitScopeRoot::ShadowHost(opaque(parent)));
        }
        // A node assigned to a slot is the slot's child here: its parent in its node tree is the
        // host of the slot's shadow tree.
        let parent = if self.nodes[parent].is_assigned_slot() {
            self.nodes[parent].containing_shadow_host_id()?
        } else {
            parent
        };
        if !self.nodes[parent].is_element() {
            return None;
        }
        Some(match self.nodes[parent].containing_shadow_host_id() {
            Some(_) => ImplicitScopeRoot::InShadowTree(opaque(parent)),
            None => ImplicitScopeRoot::InLightTree(opaque(parent)),
        })
    }

    /// pyro: a `<link>`'s `media` changed: its sheet (each fetch parses its own) takes the new
    /// media list and is inserted again, so the cascade sees it.
    pub(crate) fn refresh_link_media(&mut self, node_id: NodeId) {
        let Some(sheet) = self.nodes_to_stylesheet.get(&node_id).cloned() else {
            return;
        };
        let media = self.node_media_list(node_id).unwrap_or_else(MediaList::empty);
        *sheet.0.media.write_with(&mut self.guard.write()) = media;
        self.add_stylesheet_for_node(sheet, node_id);
    }

    /// pyro: the media list of the `media` attribute of the `<style>` or `<link>` element
    /// `node_id`; `None` without one (or an empty one: all media).
    pub(crate) fn node_media_list(&self, node_id: NodeId) -> Option<MediaList> {
        use style::stylesheets::CssRuleType;
        let text = self.nodes.get(node_id)?.attr(local_name!("media"))?.trim();
        if text.is_empty() {
            return None;
        }
        let extra = self.url.url_extra_data();
        let mut context = style::parser::ParserContext::new(
            Origin::Author,
            &extra,
            Some(CssRuleType::Media),
            style_traits::ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            Default::default(),
            None,
            None,
            Default::default(),
        );
        let mut parser = cssparser::Parser::new(text);
        Some(MediaList::parse(&mut context, &mut parser))
    }

    pub fn remove_user_agent_stylesheet(&mut self, sheet: &UserAgentStylesheet) {
        if self.useragent_stylesheets().any(|s| *s == sheet.0) {
            self.stylist
                .remove_stylesheet(sheet.0.clone(), &self.guard.read());
        }
    }

    fn image_owners(&self) -> impl Iterator<Item = &ElementData> {
        self.nodes.iter().filter_map(|(_, node)| {
            node.flags.is_in_document().then(|| node.element_data()).flatten()
        }).flat_map(|element| {
            let styles = [element.first_letter, element.first_line, element.marker_style];
            std::iter::once(element).chain(styles.into_iter().flatten().filter_map(|id| self.nodes.get(id)?.element_data()))
        })
    }

    /// pyro: the URLs of the images the document's elements reference now (their CSS image slots:
    /// backgrounds, masks, border images, references, list markers, shapes), in the document only.
    /// An image no element references any more is no longer the document's to wait for.
    /// The requests (URL and request options) of those images, not only their URLs: a variant of
    /// the same URL made otherwise is another request.
    pub fn referenced_images(&self) -> Vec<(String, blitz_traits::net::FetchOptions)> {
        self.referenced_images_with(true)
    }

    /// pyro: [`Self::referenced_images`] without those of `<img>` elements (`elements`: false),
    /// whose loads the document's own image loader tracks (and delays `load` for unless the image
    /// is lazy): what the document's `load` waits for on the renderer's side.
    pub fn referenced_images_with(&self, elements: bool) -> Vec<(String, blitz_traits::net::FetchOptions)> {
        let mut out = Vec::new();
        for element in self.image_owners() {
            if elements || element.name.local != local_name!("img") {
                out.extend(element.source_image_request.as_deref().cloned());
            }
            if element.name.ns == markup5ever::ns!(svg) && matches!(&*element.name.local, "image" | "feImage" | "use") {
                for attr in element.attrs() {
                    if &*attr.name.local == "href" && !attr.value.trim().starts_with('#') {
                        if let Some(mut url) = self.resolve_url(attr.value.trim()) {
                            if &*element.name.local == "use" { url.set_fragment(None); }
                            out.push((url.to_string(), Default::default()));
                        }
                    }
                }
            }
            if let Some((url, _)) = &element.content_image {
                out.push((url.clone(), Default::default()));
            }
            for slot in element
                .background_images
                .iter()
                .chain(element.mask_images.iter())
                .chain(element.border_images.iter())
                .chain(element.reference_images.iter())
                .chain(element.list_style_images.iter())
                .chain(element.shape_images.iter())
                .flatten()
            {
                out.push((slot.url.as_str().to_owned(), slot.options.clone()));
            }
        }
        out
    }

    pub fn referenced_image_urls(&self) -> std::collections::HashSet<String> {
        let mut urls = std::collections::HashSet::new();
        for element in self.image_owners() {
            for slot in element
                .background_images
                .iter()
                .chain(element.mask_images.iter())
                .chain(element.border_images.iter())
                .chain(element.reference_images.iter())
                .chain(element.list_style_images.iter())
                .chain(element.shape_images.iter())
                .flatten()
            {
                urls.insert(slot.url.as_str().to_owned());
            }
        }
        urls
    }

    /// Whether a resolved CSS URL refers to this document, ignoring its fragment.
    pub fn is_document_url(&self, url: &Url) -> bool {
        self.fragment_document.as_ref().unwrap_or(&self.url).is_same_document(url)
    }

    /// The document URL used to identify same-document resources.
    pub fn url(&self) -> &url::Url {
        self.fragment_document.as_ref().unwrap_or(&self.url)
    }

    /// pyro: `options` for a request this document makes, with this document's frame as the
    /// request's client when it is a frame's (`FetchOptions::client`, the frame's id, never its
    /// URL): a CORS check compares the response with that frame's origin, not the page's.
    /// Requests with the defaults stay the page's.
    pub(crate) fn request_options(
        &self,
        mut options: blitz_traits::net::FetchOptions,
    ) -> blitz_traits::net::FetchOptions {
        // pyro: (an inline sheet's `url()` has the document as its referrer already:
        // `net::url_fetch_options`, by the sheet's identity, never by comparing URLs.)
        if !options.is_default() {
            options.client = self.request_client();
        }
        options
    }

    /// pyro: the client this document's requests with other than the defaults name: its frame's
    /// id in a frame's document (one without an id names `""`, which no frame has, so its
    /// requests fail rather than go out as the page's), `None` in the page's document.
    pub(crate) fn request_client(&self) -> Option<String> {
        (self.subdocument_depth > 0).then(|| self.frame_client.clone().unwrap_or_default())
    }

    /// pyro: the quirks mode the document's author sheets (`<style>`, linked and imported) parse
    /// in: a quirks-mode document's allow the unitless length quirk on the properties Quirks Mode
    /// §3.6 lists, as Chromium's. Style attributes, CSSOM edits and the stylist's selector
    /// matching stay in no-quirks mode, as pyro's CSSOM parses them.
    pub(crate) fn sheet_quirks_mode(&self) -> QuirksMode {
        if self.nodes.quirks_mode { QuirksMode::Quirks } else { QuirksMode::NoQuirks }
    }

    /// pyro: what a link's `href` resolves against, and the document's own URL (without a
    /// fragment), which links to it are visited at (`ElementData::flush_link_visited`).
    pub(crate) fn link_urls(&self) -> (url::Url, url::Url) {
        let base: url::Url = (*self.url).clone();
        let mut document = base.clone();
        document.set_fragment(None);
        (base, document)
    }

    /// Iterate over the author stylesheets (from `<style>` and `<link>` nodes)
    /// currently associated with this document
    pub fn author_stylesheets(&self) -> impl Iterator<Item = &DocumentStyleSheet> {
        self.nodes_to_stylesheet.values()
    }

    /// Iterate over the user-agent stylesheets currently associated with this document
    pub fn useragent_stylesheets(&self) -> impl Iterator<Item = &DocumentStyleSheet> {
        (0..self.stylist.sheet_count(Origin::UserAgent))
            .filter_map(|i| self.stylist.sheet_at(Origin::UserAgent, i))
    }

    /// Append a user-agent stylesheet. Adding a sheet that is already present is a no-op.
    pub fn add_user_agent_stylesheet(&mut self, sheet: UserAgentStylesheet) {
        if self.useragent_stylesheets().any(|s| *s == sheet.0) {
            return;
        }
        self.stylist.append_stylesheet(sheet.0, &self.guard.read());
    }

    pub fn make_stylesheet(&self, css: impl AsRef<str>, origin: Origin) -> DocumentStyleSheet {
        self.make_stylesheet_in(css, origin, None)
    }

    /// [`Self::make_stylesheet`] for an element of the shadow tree of `host` (`None`: of the
    /// document's tree).
    pub(crate) fn make_stylesheet_in(&self, css: impl AsRef<str>, origin: Origin, host: Option<NodeId>) -> DocumentStyleSheet {
        // pyro: an author sheet without `@import` (whose loads are
        // the document's) parses once per thread for its text and base URL:
        // every document of a test suite or a PDF template carries the same
        // bundle. Sheets are immutable here (pyro's CSSOM edits
        // the text and syncs it, not Blitz's rules).
        let css = css.as_ref();
        // pyro: a quirks-mode document's sheets parse differently (`sheet_quirks_mode`), so they
        // are neither taken from nor given to other documents.
        let shared = origin == Origin::Author && !css.contains("@import") && !self.nodes.quirks_mode;
        if shared {
            if let Some(sheet) = crate::util::shared_sheet(css, self.url.as_str()) {
                // A stylist holds a sheet once: another element of this
                // document with the same text (two `<style>`s alike, adopted
                // sheets reordered) parses its own. So does a shadow root; shadow roots
                // share theirs (a component's instances: one cascade data for all of them).
                let taken = match host {
                    None => self.nodes_to_stylesheet.values().any(|s| *s == sheet),
                    Some(host) => {
                        self.nodes.shadow_roots.get(&host).is_some_and(|r| r.sheets.values().any(|s| *s == sheet))
                    }
                };
                if !taken {
                    // pyro: its URL data (the first document's object) is this document's own CSS's.
                    let url_data = sheet.0.contents.read_with(&sheet.0.shared_lock.read()).url_data.0.clone();
                    crate::net::adopt_inline(&self.inline_urls, &url_data);
                    return sheet;
                }
                return self.parse_stylesheet(css, origin);
            }
        }
        let sheet = self.parse_stylesheet(css, origin);
        if shared {
            crate::util::share_sheet(css, self.url.as_str(), &sheet);
        }
        sheet
    }

    fn parse_stylesheet(&self, css: &str, origin: Origin) -> DocumentStyleSheet {
        DocumentStyleSheet(ServoArc::new(self.parse_stylesheet_with_media(css, origin, MediaList::empty())))
    }

    /// pyro: [`Self::parse_stylesheet`] with the sheet's media list (`<style media>`).
    fn parse_stylesheet_with_media(&self, css: &str, origin: Origin, media: MediaList) -> Stylesheet {
        let quirks_mode = if origin == Origin::Author { self.sheet_quirks_mode() } else { QuirksMode::NoQuirks };
        Stylesheet::from_str(
            css.as_ref(),
            self.url.url_extra_data(),
            origin,
            ServoArc::new(self.guard.wrap(media)),
            self.guard.clone(),
            Some(&StylesheetLoader {
                tx: self.tx.clone(),
                doc_id: self.id,
                net_provider: self.net_provider.clone(),
                shell_provider: self.shell_provider.clone(),
                abort_signal: self.abort_signal.clone(),
                import_depth: 0,
                client: self.request_client(),
                inline: Some(self.inline_urls.clone()),
                policies: self.sheet_policies.clone(),
                quirks_mode,
            }),
            None,
            quirks_mode,
            AllowImportRules::Yes,
        )
    }

    pub fn upsert_stylesheet_for_node(&mut self, node_id: NodeId) {
        let raw_styles = self.nodes[node_id].text_content();
        let host = self.nodes[node_id].containing_shadow_host_id();
        let sheet = self.make_stylesheet_in(raw_styles, Origin::Author, host);
        self.add_stylesheet_for_node(sheet, node_id);
    }

    pub fn add_stylesheet_for_node(&mut self, stylesheet: DocumentStyleSheet, node_id: NodeId) {
        // pyro: the sheet of an element of a shadow tree is that shadow root's (`crate::shadow`).
        let host = self.nodes.get(node_id).and_then(|node| node.containing_shadow_host_id());
        if let Some(old) = self.nodes_to_stylesheet.remove(&node_id) {
            self.stylist.remove_stylesheet(old, &self.guard.read());
            self.stylesheet_generation += 1;
        }
        self.remove_shadow_stylesheet(node_id);
        if let Some(host) = host {
            crate::net::fetch_font_face(
                self.tx.clone(),
                self.id,
                Some(node_id),
                &stylesheet.0,
                &self.net_provider,
                &self.shell_provider,
                &self.guard.read(),
                self.abort_signal.as_ref(),
                Some(self.stylist.device()),
            );
            let element = &mut self.nodes[node_id].element_data_mut().unwrap();
            element.special_data = SpecialElementData::Stylesheet(stylesheet.clone());
            self.add_shadow_stylesheet(host, node_id, stylesheet);
            return;
        }
        let old = self.nodes_to_stylesheet.insert(node_id, stylesheet.clone());
        self.stylesheet_generation += 1;

        if let Some(old) = old {
            self.stylist.remove_stylesheet(old, &self.guard.read())
        }

        // Fetch @font-face fonts
        crate::net::fetch_font_face(
            self.tx.clone(),
            self.id,
            Some(node_id),
            &stylesheet.0,
            &self.net_provider,
            &self.shell_provider,
            &self.guard.read(),
            self.abort_signal.as_ref(),
            Some(self.stylist.device()),
        );

        // Store data on element
        let element = &mut self.nodes[node_id].element_data_mut().unwrap();
        element.special_data = SpecialElementData::Stylesheet(stylesheet.clone());

        // A node slot can be reused after removal. The cascade follows the live
        // owner elements' document order, including when a sheet is replaced.
        let insertion_point = self.next_document_stylesheet(node_id);

        if let Some(insertion_point) = insertion_point {
            self.stylist.insert_stylesheet_before(
                stylesheet,
                insertion_point.clone(),
                &self.guard.read(),
            )
        } else {
            self.stylist
                .append_stylesheet(stylesheet, &self.guard.read())
        }
    }

    /// Reposition already loaded document sheets after their owners move. Remove the
    /// whole moved group first so insertion always sees an ordered set of other sheets.
    pub(crate) fn reposition_document_stylesheets(&mut self, owners: Vec<NodeId>) {
        if owners.is_empty() { return; }
        let guard = self.guard.read();
        let mut moved = Vec::with_capacity(owners.len());
        for owner in owners {
            if let Some(sheet) = self.nodes_to_stylesheet.remove(&owner) {
                self.stylist.remove_stylesheet(sheet.clone(), &guard);
                moved.push((owner, sheet));
            }
        }
        for (owner, sheet) in moved {
            let next = self.next_document_stylesheet(owner).cloned();
            self.nodes_to_stylesheet.insert(owner, sheet.clone());
            if let Some(next) = next {
                self.stylist.insert_stylesheet_before(sheet, next, &guard);
            } else {
                self.stylist.append_stylesheet(sheet, &guard);
            }
        }
        self.stylesheet_generation += 1;
    }

    fn next_document_stylesheet(&self, owner: NodeId) -> Option<&DocumentStyleSheet> {
        let node = &self.nodes[owner];
        if let Some(parent) = node.parent {
            // When all stylesheet owners are siblings, their order is the parent's
            // child list. Avoid constructing two ancestor chains per comparison.
            if self.nodes_to_stylesheet.keys().all(|id| self.nodes.get(*id).is_some_and(|n| n.parent == Some(parent))) {
                let siblings = &self.nodes[parent].children;
                if siblings.last() == Some(&owner) { return None; }
                let index = node.child_index()?;
                return siblings[index + 1..].iter().find_map(|id| self.nodes_to_stylesheet.get(id));
            }
        }
        let root = node.owner_doc().id;
        self.nodes_to_stylesheet.keys().copied()
            .filter(|&id| id != owner && self.nodes.get(id).is_some_and(|n| n.owner_doc().id == root))
            .filter(|&id| self.compare_document_order(owner, id).is_lt())
            .min_by(|&a, &b| self.compare_document_order(a, b))
            .and_then(|id| self.nodes_to_stylesheet.get(&id))
    }

    pub fn handle_messages(&mut self) {
        // Remove event Reciever from the Document so that we can process events
        // without holding a borrow to the Document
        let rx = self.rx.take().unwrap();

        while let Ok(msg) = rx.try_recv() {
            self.handle_message(msg);
        }

        // Put Reciever back
        self.rx = Some(rx);
    }

    pub fn handle_message(&mut self, msg: DocumentEvent) {
        match msg {
            DocumentEvent::ResourceLoad(resource) => self.load_resource(resource),
            DocumentEvent::NavigateIframe { node_id, url } => self.navigate_iframe(node_id, url),
        }
    }

    /// pyro: whether the style traversals record the elements whose
    /// primary style they replace ([`Self::take_style_changes`]); off until an
    /// embedder wants them (transition events). Turning it off drops what was
    /// recorded.
    pub fn set_track_style_changes(&mut self, on: bool) {
        self.track_style_changes = on;
        if !on {
            self.style_changes.clear();
            self.style_change_index.clear();
        }
    }

    /// pyro: the elements whose primary style a traversal replaced
    /// since the last call (while tracked), each once, with its style before
    /// the first replacement and after the last: a transition's before-change
    /// and after-change styles. An element styled for the first time, or whose
    /// styles were dropped, is no change. O(elements restyled), not a scan.
    /// pyro: the page's scroll offsets that scroll-driven animations follow (`timeline_scroll`).
    pub fn set_timeline_scroll(&mut self, elements: std::collections::HashMap<NodeId, (f64, f64)>, viewport: (f64, f64)) -> bool {
        let mut invalidated = false;
        if !self.anchor_try_scrollports.is_empty() {
            let old = self.timeline_scroll.as_ref().map(|(elements, _)| elements);
            let changed = elements.keys().chain(old.into_iter().flat_map(|map| map.keys()).filter(|id| !elements.contains_key(id)))
                .filter(|id| elements.get(id).copied().unwrap_or_default() != old.and_then(|map| map.get(id)).copied().unwrap_or_default()).copied().map(Some);
            let viewport_changed = self.timeline_scroll.as_ref().map(|(_, offset)| *offset).unwrap_or_default() != viewport;
            for source in changed.chain(viewport_changed.then_some(None)) {
                if let Some(blocks) = self.anchor_try_scrollports.get(&source) {
                    for block in blocks {
                        self.nodes[*block].insert_damage(crate::layout::damage::ONLY_RELAYOUT);
                        if source != Some(*block) { self.anchor_scroll_dirty.insert(*block); }
                    }
                    invalidated = true;
                }
            }
        }
        // pyro: `scroll-state()` containers read the offsets (`refresh_scroll_state_queries`).
        // (And the active scroll marker follows the scroll position: `update_target_current`.)
        let scroll_state_changed = (!self.nodes.scroll_state_queries.lock().unwrap().is_empty() || !self.scroll_pseudo_owners.is_empty())
            && self.timeline_scroll.as_ref().is_none_or(|(old, old_viewport)| *old != elements || *old_viewport != viewport);
        self.timeline_scroll = Some((elements, viewport));
        if scroll_state_changed {
            self.update_scroll_marker_states();
            invalidated = true;
        }
        invalidated
    }

    /// pyro: the direction of each scroll container's last scroll on each axis (`-1` towards the
    /// start, `1` towards the end), which `scroll-state(scrolled)` queries. Whether it changed
    /// (a container with such a query then styles again at the next resolve).
    pub fn set_scroll_directions(&mut self, directions: std::collections::HashMap<NodeId, (i8, i8)>) -> bool {
        if self.scroll_directions == directions {
            return false;
        }
        self.scroll_directions = directions;
        if self.nodes.scroll_state_queries.lock().unwrap().is_empty() {
            return false;
        }
        self.update_scroll_marker_states();
        true
    }

    /// pyro: the elements that are a scroll container's snap target, with the axes (x, y) they
    /// are snapped on, which `scroll-state(snapped)` queries. Whether it changed (a container with
    /// such a query then styles again at the next resolve).
    pub fn set_snapped(&mut self, snapped: std::collections::HashMap<NodeId, (bool, bool)>) -> bool {
        if self.snapped_targets == snapped {
            return false;
        }
        self.snapped_targets = snapped;
        if self.nodes.scroll_state_queries.lock().unwrap().is_empty() {
            return false;
        }
        self.update_scroll_marker_states();
        true
    }

    /// pyro: the sticky boxes held at an edge by their insets, with the edges (bit 0 top, 1 right,
    /// 2 bottom, 3 left), which `scroll-state(stuck)` queries. Whether it changed (a container
    /// with such a query then styles again at the next resolve).
    pub fn set_stuck(&mut self, stuck: std::collections::HashMap<NodeId, u8>) -> bool {
        if self.stuck_targets == stuck {
            return false;
        }
        self.stuck_targets = stuck;
        if self.nodes.scroll_state_queries.lock().unwrap().is_empty() {
            return false;
        }
        self.update_scroll_marker_states();
        true
    }

    /// pyro: whether a container was queried for `scroll-state()` (the embedder tells the
    /// state that queries follow only then).
    pub fn uses_scroll_state(&self) -> bool {
        !self.nodes.scroll_state_queries.lock().unwrap().is_empty()
    }

    /// pyro: see [`Self::scroll_group_moves`].
    pub fn take_scroll_group_moves(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.scroll_group_moves)
    }

    /// pyro: the elements whose used `anchor()`/`anchor-size()` values ([`ANCHOR_USED_SLOTS`])
    /// changed since the last call, with their values then and now.
    pub fn take_anchor_evals(&mut self) -> Vec<AnchorEval> {
        let changed = std::mem::take(self.anchor_used_changed.get_mut());
        let used = self.anchor_used.get_mut();
        changed
            .into_iter()
            .filter_map(|(node, before)| {
                let after = *used.get(&node)?;
                (before != after).then_some(AnchorEval { node, before, after })
            })
            .collect()
    }

    /// pyro: the used px of `node`'s `anchor()`/`anchor-size()` values as its last layout
    /// resolved them ([`ANCHOR_USED_SLOTS`]).
    pub fn anchor_used(&self, node: NodeId) -> Option<[Option<f32>; ANCHOR_USED_SLOTS]> {
        self.anchor_used.borrow().get(&node).copied()
    }

    /// pyro: records the used px of an anchored element's `anchor()`/`anchor-size()` value in
    /// slot `slot` ([`ANCHOR_USED_SLOTS`]), as its layout resolved it.
    pub(crate) fn record_anchor_used(&self, node: NodeId, slot: usize, px: f32) {
        let mut used = self.anchor_used.borrow_mut();
        let values = used.entry(node).or_insert([None; ANCHOR_USED_SLOTS]);
        if values[slot] == Some(px) {
            return;
        }
        self.anchor_used_changed.borrow_mut().entry(node).or_insert(*values);
        values[slot] = Some(px);
    }

    /// pyro: drops the used `anchor()`/`anchor-size()` px recorded for `node`
    /// ([`Self::record_anchor_used`]): the next ones it records start no transition from them.
    pub(crate) fn forget_anchor_used(&self, node: NodeId) {
        if self.anchor_used.borrow_mut().remove(&node).is_some() {
            self.anchor_used_changed.borrow_mut().remove(&node);
        }
    }

    /// pyro: after a layout, drops the used `anchor()`/`anchor-size()` px of the elements that have
    /// no box now (gone, `display: none` or `contents`, or in a `display: none` subtree), so a box
    /// that comes back does not transition from the px it had before it went away.
    pub(crate) fn forget_boxless_anchor_used(&mut self) {
        if self.anchor_used.get_mut().is_empty() {
            return;
        }
        let nodes = &self.nodes;
        let boxless = |id: NodeId| {
            let Some(node) = nodes.get(id) else { return true };
            if !node.has_boxes() {
                return true;
            }
            let mut current = Some(node);
            while let Some(n) = current {
                if n.is_element() && n.primary_styles().is_none_or(|style| style.get_box().display.is_none()) {
                    return true;
                }
                current = n.parent.and_then(|p| nodes.get(p));
            }
            false
        };
        let gone: Vec<NodeId> = self.anchor_used.get_mut().keys().copied().filter(|&id| boxless(id)).collect();
        for id in gone {
            self.anchor_used.get_mut().remove(&id);
            self.anchor_used_changed.get_mut().remove(&id);
        }
    }

    pub fn take_style_changes(&mut self) -> Vec<StyleChange> {
        self.style_change_index.clear();
        std::mem::take(&mut self.style_changes)
    }

    /// pyro: whether a multi-column container has been laid out in this document (see
    /// `layout::multicol`).
    pub fn has_multicol(&self) -> bool {
        self.has_multicol
    }

    /// pyro: whether any node of the document (attached or not) is
    /// styled `position: sticky`, from the set the style traversal keeps
    /// (O(sticky nodes), not a scan of the document).
    pub fn has_sticky(&self) -> bool {
        self.sticky_node_ids().next().is_some()
    }

    /// pyro: the nodes styled `position: sticky` (see
    /// [`Self::has_sticky`]), in no particular order.
    pub fn sticky_node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.sticky_nodes
            .iter()
            .copied()
            .filter(|id| self.nodes.get(*id).is_some_and(crate::stylo::is_sticky))
    }

    /// pyro: whether a `resolve` could change anything: a resource arrived (it is
    /// taken in now), something awaits restyle or layout here or in a subdocument, or an
    /// animation runs. The last pass's `refresh_hover` can leave a restyle behind.
    pub fn needs_resolve(&mut self) -> bool {
        let rx = self.rx.take().unwrap();
        let mut arrived = false;
        while let Ok(msg) = rx.try_recv() {
            self.handle_message(msg);
            arrived = true;
        }
        self.rx = Some(rx);
        if arrived || !self.pending_device_changes.is_empty() || self.is_animating() {
            return true;
        }
        let root_is_clean = self.try_root_element().is_some_and(|root| {
            !root.has_dirty_descendants()
                && !root.has_damaged_descendants()
                && !root.has_snapshot()
                && root.damage().is_some_and(|damage| damage.is_empty())
        });
        if !root_is_clean {
            return true;
        }
        let sub_documents: Vec<NodeId> = self.sub_document_nodes.iter().copied().collect();
        sub_documents.into_iter().any(|node_id| {
            self.nodes[node_id]
                .subdoc_mut()
                .is_some_and(|doc| doc.inner_mut().needs_resolve())
        })
    }

    /// pyro: an `<img>` that represents no image (it failed to load, or has no usable source)
    /// and has non-empty `alt` text is laid out as a non-replaced box holding that text, as
    /// browsers show it (HTML rendering § images: "an inline box containing the text"; WPT
    /// css-overflow `overflow-img-scroll-non-replaced`). Its text is an anonymous child.
    pub fn show_image_alt_text(&mut self, node_id: NodeId) {
        let Some(node) = self.nodes.get(node_id) else { return };
        let Some(element) = node.element_data() else { return };
        if element.name.local != local_name!("img") || element.image_broken {
            return;
        }
        let Some(alt) = element.attr(local_name!("alt")).filter(|alt| !alt.is_empty()).map(str::to_owned) else {
            return;
        };
        let failed = element.image_failed;
        let node = &mut self.nodes[node_id];
        if let Some(element) = node.element_data_mut() {
            element.image_broken = true;
            element.special_data = SpecialElementData::None;
        }
        node.clear_layout_cache();
        {
            let mut mutator = self.mutate();
            // pyro: an image that failed to load shows Chromium's broken-image icon before its
            // alt text: a 16px square box blitz-paint draws it in (`draw_broken_image_icon`).
            if failed {
                let style = crate::node::Attribute {
                    name: markup5ever::QualName::new(None, markup5ever::ns!(), local_name!("style")),
                    value: "display:inline-block;width:16px;height:16px;vertical-align:top".to_string(),
                };
                let marker = crate::node::Attribute {
                    name: markup5ever::QualName::new(None, markup5ever::ns!(), LocalName::from("pyro-broken-image-icon")),
                    value: String::new(),
                };
                let icon = mutator.create_element(markup5ever::QualName::new(None, markup5ever::ns!(html), local_name!("span")), vec![style, marker]);
                mutator.append_children(node_id, &[icon]);
            }
            let text = mutator.create_text_node(&alt);
            mutator.append_children(node_id, &[text]);
        }
        let mut current = Some(node_id);
        while let Some(id) = current {
            self.nodes[id].insert_damage(ALL_DAMAGE);
            self.nodes[id].clear_layout_cache();
            current = self.nodes[id].parent;
        }
    }

    /// Whether the Document has pending requests for "critical" resources (that should block rendering)
    pub fn has_pending_critical_resources(&self) -> bool {
        !self.pending_critical_resources.is_empty()
    }

    pub fn load_resource(&mut self, res: ResourceLoadResponse) {
        self.pending_critical_resources.remove(&res.request_id);

        let resource = match res.result {
            Ok(resource) => resource,
            Err(err) => {
                if let Some(url) = res.resolved_url.as_ref() {
                    let waiting_nodes = self.pending_images.remove(url).unwrap_or_default();
                    // pyro: a broken `<img>` with alt text shows that text.
                    for (node_id, image_type) in &waiting_nodes {
                        if matches!(image_type, ImageType::Image) {
                            if let Some(element) = self.nodes.get_mut(*node_id).and_then(|n| n.element_data_mut()) {
                                if element.name.local == local_name!("img") {
                                    element.image_failed = true;
                                }
                            }
                            self.nodes[*node_id].insert_damage(ALL_DAMAGE);
                            self.show_image_alt_text(*node_id);
                        }
                    }
                    // pyro: a layer image that fails is no longer loading: a list item keeps its
                    // `list-style-type` marker, and a mask or a referenced document that failed is
                    // not waited for (a mask still loading hides its element).
                    for (node_id, image_type) in &waiting_nodes {
                        let Some(el) = self.nodes.get_mut(*node_id).and_then(|node| node.element_data_mut()) else {
                            continue;
                        };
                        let slot = match *image_type {
                            ImageType::ListStyle(idx) => el.list_style_images.get_mut(idx),
                            ImageType::Background(idx) => el.background_images.get_mut(idx),
                            ImageType::Mask(idx) => el.mask_images.get_mut(idx),
                            ImageType::BorderImage(idx) => el.border_images.get_mut(idx),
                            ImageType::Reference(idx) => el.reference_images.get_mut(idx),
                            ImageType::Image | ImageType::Shape(_) => None,
                        };
                        if let Some(Some(image)) = slot {
                            if image.status == Status::Loading
                                && (matches!(image_type, ImageType::ListStyle(_)) || image.request_key() == *url)
                            {
                                image.status = Status::Error;
                            }
                        }
                    }
                    #[cfg(feature = "tracing")]
                    tracing::warn!(
                        url = url.as_str(),
                        waiting_nodes = waiting_nodes.len(),
                        error = err.as_str(),
                        "Resource load failed"
                    );
                    #[cfg(not(feature = "tracing"))]
                    let _ = (waiting_nodes, err);
                } else {
                    #[cfg(feature = "tracing")]
                    tracing::warn!(error = err.as_str(), "Resource load failed (no url)");
                    #[cfg(not(feature = "tracing"))]
                    let _ = err;
                }
                return;
            }
        };

        match resource {
            Resource::Css(css, page_css) => {
                let node_id = res.node_id.unwrap();
                // pyro: a link that stopped being a stylesheet link while its sheet was fetched
                // (its `rel` lost `stylesheet`, or its `href` went) takes no sheet.
                let still_links = self.nodes.get(node_id).is_some_and(|node| {
                    node.attr(local_name!("href")).is_some()
                        && node
                            .attr(local_name!("rel"))
                            .is_some_and(|rel| rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet")))
                });
                if still_links {
                    // pyro: a `<link media>` is its sheet's media list (each fetch parses its own sheet).
                    if let Some(media) = self.node_media_list(node_id) {
                        *css.0.media.write_with(&mut self.guard.write()) = media;
                    }
                    self.add_stylesheet_for_node(css, node_id);
                    match page_css {
                        Some(text) => {
                            self.linked_page_css.insert(node_id, text);
                        }
                        None => {
                            self.linked_page_css.remove(&node_id);
                        }
                    }
                }
            }
            Resource::ImportedCss(import_rule, sheet) => {
                let mut guard = self.guard.write();
                import_rule.write_with(&mut guard).stylesheet =
                    style::stylesheets::import_rule::ImportSheet::Sheet(sheet);
                drop(guard);
                self.stylist
                    .force_stylesheet_origins_dirty(style::stylesheets::OriginSet::all());
            }
            Resource::Image(_kind, width, height, image_data, orientation) => {
                // Create the ImageData and cache it
                let raster = RasterImageData { width, height, data: image_data, orientation };
                let image = ImageData::Raster(raster);

                let Some(url) = res.resolved_url.as_ref() else {
                    return;
                };

                self.apply_loaded_image(url, image);
            }
            #[cfg(feature = "svg")]
            Resource::Svg(_kind, svg) => {
                // pyro: one with a `<foreignObject>` is drawn as a document (`svg_image_documents`).
                self.has_svg_image_documents |= svg.foreign_source.is_some();
                // Create the ImageData and cache it
                let image = ImageData::Svg(svg);

                let Some(url) = res.resolved_url.as_ref() else {
                    return;
                };

                self.apply_loaded_image(url, image);
            }
            Resource::DocumentSrc(html) => {
                let Some(node_id) = res.node_id else {
                    return;
                };
                self.apply_iframe_html(node_id, res.request_id, res.resolved_url, &html);
            }
            Resource::Font(bytes, overrides) => {
                // pyro: SVG text draws with it too (usvg's font database).
                #[cfg(feature = "svg")]
                if let Some(family) = &overrides.family_name {
                    crate::util::add_web_svg_font(&bytes, family);
                }
                let font = crate::util::intern_font_blob(bytes, overrides.family_name.as_deref());

                // Build a `FontInfoOverride` from the `@font-face` descriptors
                // captured during stylesheet parsing. Without this, parley
                // reads the family name from the TTF's own metadata, which
                // means CSS `font-family: 'Avenir Book'` won't match a font
                // file that internally identifies as `Avenir 45 Book`.
                // The face is only used for the code points of its `unicode-range`, and its
                // precedence orders it among the faces matching a style equally.
                let weight_override = overrides.weight.map(parley::fontique::FontWeight::new);
                let info_override = parley::fontique::FontInfoOverride {
                    family_name: overrides.family_name.as_deref(),
                    weight: weight_override,
                    width: overrides.width.map(parley::fontique::FontWidth::from_percentage),
                    slant_range: overrides.slant_range,
                    face_metrics: overrides.face_metrics,
                    face_settings: overrides.face_settings,
                    weight_range: overrides.weight_range,
                    width_range: overrides.width_range,
                    style: overrides.style,
                    unicode_range: overrides.unicode_range.as_ref(),
                    precedence: Some(overrides.precedence),
                    ..Default::default()
                };

                // TODO: Investigate eliminating double-box
                let mut global_font_ctx = self.font_ctx.lock().unwrap();
                global_font_ctx
                    .collection
                    .register_fonts(font.clone(), Some(info_override));
                if let Some(family) = &overrides.family_name {
                    let families = self.font_face_families.entry(font.id()).or_default();
                    if !families.contains(family) {
                        families.push(family.clone());
                    }
                }

                #[cfg(feature = "parallel-construct")]
                {
                    rayon::broadcast(|_ctx| {
                        let mut font_ctx = self
                            .thread_font_contexts
                            .get_or(|| RefCell::new(Box::new(global_font_ctx.clone())))
                            .borrow_mut();
                        font_ctx
                            .collection
                            .register_fonts(font.clone(), Some(info_override));
                    });
                }
                drop(global_font_ctx);

                // Reshaping updates text advances. Font-relative values in computed
                // styles (including pseudo-elements) also need the new metrics.
                let device = self.stylist.device();
                if device.used_root_font_metrics() {
                    device.update_root_font_metrics();
                }
                if let Some(root) = self.try_root_element()
                    && device.style_uses_font_metrics()
                {
                    let id = root.id;
                    if let Some(style) = root.primary_styles() {
                        let height = device.calc_line_height(style.get_font(), style.writing_mode, None);
                        device.set_root_line_height(style.effective_zoom.unzoom(height.0.px()));
                    }
                    self.nodes[id].set_restyle_hint(RestyleHint::recascade_subtree());
                    self.sampled_animations.invalidate_bases();
                }
                self.invalidate_inline_contexts();
            }
            Resource::None => {
                // Do nothing
            }
        }
    }

    /// pyro: damages the outermost `<svg>` around SVG element `node_id`, so that its box (and the
    /// SVG source it is drawn from) is built again.
    #[cfg(feature = "svg")]
    /// pyro: every inline `<svg>` root is built again (its source changes with
    /// [`Self::svg_images`]).
    pub fn damage_inline_svgs(&mut self) {
        let roots: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.element_data().is_some_and(|e| e.name.ns == markup5ever::ns!(svg) && e.name.local == local_name!("svg"))
                    && !node.parent.and_then(|p| self.nodes.get(p)).and_then(|p| p.element_data()).is_some_and(|e| e.name.ns == markup5ever::ns!(svg))
            })
            .map(|(id, _)| id)
            .collect();
        for id in roots {
            if let Some(node) = self.get_node_mut(id) {
                node.clear_layout_cache();
                node.insert_damage(ALL_DAMAGE);
            }
        }
    }

    pub(crate) fn damage_inline_svg_root(&mut self, node_id: NodeId) {
        let mut root = None;
        let mut next = Some(node_id);
        while let Some(id) = next {
            let Some(node) = self.get_node(id) else { break };
            let Some(element) = node.element_data() else { break };
            if element.name.ns != markup5ever::ns!(svg) {
                break;
            }
            if element.name.local == local_name!("svg") {
                root = Some(id);
            }
            next = node.parent;
        }
        if let Some(node) = root.and_then(|id| self.get_node_mut(id)) {
            node.clear_layout_cache();
            node.insert_damage(ALL_DAMAGE);
        }
    }

    /// Cache a loaded image and apply it to all nodes waiting on it
    /// (`<img>` elements, `background-image` layers and `mask-image` layers).
    fn apply_loaded_image(&mut self, url: &str, image: ImageData) {
        // Get all nodes waiting for this image
        let waiting_nodes = self.pending_images.remove(url).unwrap_or_default();

        #[cfg(feature = "tracing")]
        tracing::info!(
            "Image {url} loaded, applying to {} nodes",
            waiting_nodes.len()
        );

        // Cache the image
        self.image_cache.insert(url.to_string(), image.clone());

        // Apply to all waiting nodes
        for (node_id, image_type) in waiting_nodes {
            let Some(node) = self.get_node_mut(node_id) else {
                continue;
            };

            match image_type {
                ImageType::Image => {
                    let element = node.element_data_mut().unwrap();
                    #[cfg(feature = "custom-widget")]
                    if element.custom_widget_data().is_some() {
                        continue;
                    }
                    // pyro: the image of its `content: url()` stands for its own (`src`, `poster`).
                    if element.content_image.as_ref().is_some_and(|(content, _)| content != url) {
                        continue;
                    }
                    // pyro: an `<object>` shows only some images (`replaced::object_shows_image`).
                    if element.name.local == local_name!("object")
                        && !crate::layout::replaced::object_shows_image(&image)
                    {
                        continue;
                    }
                    element.special_data = SpecialElementData::Image(Box::new(image.clone()));

                    // Clear layout cache
                    node.clear_layout_cache();
                    node.insert_damage(ALL_DAMAGE);
                }
                // pyro: a document an element's `clip-path`, `filter` or `mask` references an
                // element of: the box is painted, and an inline `<svg>` written, with it.
                // pyro: into the slot only while it still holds this request (a late completion of
                // a request the slot no longer names is not its image).
                ImageType::Reference(idx) => {
                    if let Some(Some(reference)) = node
                        .element_data_mut()
                        .and_then(|el| el.reference_images.get_mut(idx))
                        .filter(|slot| slot.as_ref().is_some_and(|slot| slot.request_key() == url))
                    {
                        reference.status = Status::Ok;
                        reference.image = image.clone();
                    }
                    #[cfg(feature = "svg")]
                    self.damage_inline_svg_root(node_id);
                }
                // pyro: a float's `shape-outside` image is its float area: the boxes around
                // it are laid out again.
                ImageType::Shape(idx) => {
                    // Only into the slot of the image the shape still names (`url` is its URL, or
                    // its CORS key): an image it named before finishing late is not its shape.
                    let Some(Some(shape)) = node.element_data_mut().and_then(|el| el.shape_images.get_mut(idx)) else {
                        continue;
                    };
                    if shape.request_key() != url {
                        continue;
                    }
                    shape.status = Status::Ok;
                    shape.image = image.clone();
                    node.clear_layout_cache();
                    node.insert_damage(ALL_DAMAGE);
                    let mut ancestor = node.layout_parent.get().or(node.parent);
                    while let Some(id) = ancestor {
                        let Some(parent) = self.nodes.get_mut(id) else { break };
                        parent.clear_layout_cache();
                        parent.insert_damage(ALL_DAMAGE);
                        ancestor = parent.layout_parent.get().or(parent.parent);
                    }
                }
                ImageType::Background(idx)
                | ImageType::Mask(idx)
                | ImageType::BorderImage(idx)
                | ImageType::ListStyle(idx) => {
                    let layer_image = node.element_data_mut().and_then(|el| {
                        let images = match image_type {
                            ImageType::Background(_) => &mut el.background_images,
                            ImageType::Mask(_) => &mut el.mask_images,
                            ImageType::BorderImage(_) => &mut el.border_images,
                            ImageType::ListStyle(_) => &mut el.list_style_images,
                            ImageType::Image | ImageType::Reference(_) | ImageType::Shape(_) => unreachable!(),
                        };
                        images.get_mut(idx)
                    });
                    // pyro: into the slot only while it still holds this request.
                    let layer_image = layer_image.filter(|slot| slot.as_ref().is_some_and(|slot| slot.request_key() == url));
                    if let Some(Some(layer_image)) = layer_image {
                        layer_image.status = Status::Ok;
                        layer_image.image = image.clone();
                    }
                    // pyro: a list item's image is its marker: the list builds it again.
                    if matches!(image_type, ImageType::ListStyle(_)) {
                        node.clear_layout_cache();
                        node.insert_damage(ALL_DAMAGE);
                    }
                    // pyro: an inline `<svg>` is drawn from a source written with its elements'
                    // mask images (`svg_css`): its box is built again with the image.
                    #[cfg(feature = "svg")]
                    if matches!(image_type, ImageType::Mask(_)) {
                        self.damage_inline_svg_root(node_id);
                    }
                }
            }
        }
    }

    /// Snapshot the node's pre-mutation state (element state and attributes) ahead of
    /// an attribute mutation, so that the next style traversal can diff selector matches
    /// then-vs-now and invalidate the affected elements. `attr` is the attribute about to
    /// change; it is recorded as changed even if the element does not have it yet.
    pub fn snapshot_node(&mut self, node_id: NodeId, attr: &LocalName) {
        self.snapshot_node_impl(node_id, Some(attr))
    }

    /// Snapshot only the node's pre-change [`ElementState`] ahead of a state change
    /// (hover/focus/active/etc). Cheaper than [`Self::snapshot_node`] as it does not
    /// copy attributes or trigger attribute/class/id invalidation work.
    pub fn snapshot_node_state_only(&mut self, node_id: NodeId) {
        self.snapshot_node_impl(node_id, None)
    }

    /// pyro: `:has()` invalidation for DOM insertions and removals
    /// (state and attribute changes go through Stylo's relative selector
    /// invalidation instead, `has_invalidation`). Matching flags every element a
    /// `:has()` search visits (`RELATIVE_SELECTOR_SEARCH_DIRECTION_*`) and
    /// every anchor (`ANCHORS_RELATIVE_SELECTOR[_NON_SUBJECT]`); when `node_id` is in a
    /// search path, the anchors that can see it (its ancestors and their
    /// previous siblings, and its own previous siblings) restyle their
    /// subtrees.
    /// Insertions and removals go through Stylo's relative selector invalidator instead
    /// (`invalidate_relative_selectors_for_insertion`, stylo.rs).
    pub fn invalidate_has_anchors(&mut self, node_id: NodeId) {
        use selectors::matching::ElementSelectorFlags as F;
        let search = F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR
            | F::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING
            | F::RELATIVE_SELECTOR_SEARCH_DIRECTION_ANCESTOR_SIBLING;
        let flags = |n: &Node| if n.is_element() { n.selector_flags().get() } else { F::empty() };
        let Some(node) = self.nodes.get(node_id) else { return };
        if !flags(node).intersects(search) {
            return;
        }
        let mut anchors = Vec::new();
        let mut cur = Some(node_id);
        while let Some(id) = cur {
            let parent = self.nodes[id].parent;
            let is_anchor =
                |n: &Node| flags(n).intersects(F::ANCHORS_RELATIVE_SELECTOR | F::ANCHORS_RELATIVE_SELECTOR_NON_SUBJECT);
            if is_anchor(&self.nodes[id]) {
                anchors.push(id);
            }
            if let Some(p) = parent {
                for &s in &self.nodes[p].children {
                    if s == id {
                        break;
                    }
                    if is_anchor(&self.nodes[s]) {
                        anchors.push(s);
                    }
                }
            }
            cur = parent;
        }
        // An anchor whose `:has()` answer may change restyles its subtree,
        // and, when a sibling combinator follows the anchor's compound
        // (`:has(...) ~ .x`: the parent carries the slow-selector flags), the
        // siblings that compound can reach.
        let mut restyle = Vec::new();
        for id in anchors {
            restyle.push(id);
            if let Some(p) = self.nodes[id].parent {
                let pf = flags(&self.nodes[p]);
                if pf.contains(F::HAS_SLOW_SELECTOR) {
                    restyle.extend(self.nodes[p].children.iter().copied().filter(|&c| c != id));
                } else if pf.contains(F::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                    let children = &self.nodes[p].children;
                    let at = children.iter().position(|&c| c == id).map_or(children.len(), |i| i + 1);
                    restyle.extend(children[at..].iter().copied());
                }
            }
        }
        for id in restyle {
            if self.nodes[id].is_element() && self.nodes[id].primary_styles().is_some() {
                self.nodes[id].set_restyle_hint(RestyleHint::restyle_subtree());
            }
        }
    }

    fn snapshot_node_impl(&mut self, node_id: NodeId, changed_attr: Option<&LocalName>) {
        // `:has()` anchors are invalidated after the change, from the
        // relative selector map (`has_invalidation`).
        let node = &mut self.nodes[node_id];

        // Do not snapshot nodes that have never been styled. A snapshot records an element's
        // pre-mutation state so a restyle can diff selector matches then-vs-now. An element
        // that has never been styled has no "then" to diff against. Snapshotting it anyway
        // makes Stylo's invalidation unwrap its (absent) primary style and panic.
        let has_been_styled = node.primary_styles().is_some();
        if !has_been_styled {
            return;
        }

        let opaque_node_id = TNode::opaque(&&*node);
        node.set_has_snapshot(true);
        node.snapshot_handled()
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // A snapshot records the element's state/attributes as they were *before the first
        // mutation* since the last style flush (matching Gecko's ServoElementSnapshot
        // semantics): state is captured at most once, and attributes are captured at most
        // once, but a state-only snapshot is upgraded to also capture attributes if an
        // attribute mutation follows.
        let needs_attrs = changed_attr.is_some()
            && self
                .snapshots
                .get_mut(&opaque_node_id)
                .is_none_or(|snapshot| snapshot.attrs.is_none());

        let (attrs, changed_attrs) = if needs_attrs {
            let node = &self.nodes[node_id];
            let attrs: Option<Vec<_>> = node.attrs().map(|attrs| {
                attrs
                    .iter()
                    .map(|attr| {
                        let ident = AttrIdentifier {
                            local_name: GenericAtomIdent(attr.name.local.clone()),
                            name: GenericAtomIdent(attr.name.local.clone()),
                            namespace: GenericAtomIdent(attr.name.ns.clone()),
                            prefix: None,
                        };

                        let value = if attr.name.local == local_name!("id") {
                            AttrValue::Atom(Atom::from(&*attr.value))
                        } else if attr.name.local == local_name!("class") {
                            let classes = attr
                                .value
                                .split_ascii_whitespace()
                                .map(Atom::from)
                                .collect();
                            AttrValue::TokenList(OnceLock::from(attr.value.clone()), classes)
                        } else {
                            AttrValue::String(attr.value.clone())
                        };

                        (ident, value)
                    })
                    .collect()
            });

            let mut changed_attrs: Vec<_> = attrs
                .as_ref()
                .map(|attrs| attrs.iter().map(|attr| attr.0.name.clone()).collect())
                .unwrap_or_default();
            if let Some(attr) = changed_attr {
                if !changed_attrs.iter().any(|name| name.0 == *attr) {
                    changed_attrs.push(GenericAtomIdent(attr.clone()));
                }
            }

            (attrs, changed_attrs)
        } else {
            (None, Vec::new())
        };

        if let Some(snapshot) = self.snapshots.get_mut(&opaque_node_id) {
            // The existing snapshot's state is preserved: it records the state before
            // the *first* change since the last style flush.
            if needs_attrs {
                snapshot.attrs = attrs;
                snapshot.changed_attrs = changed_attrs;
                snapshot.class_changed = true;
                snapshot.id_changed = true;
                snapshot.other_attributes_changed = true;
            } else if let Some(attr) = changed_attr {
                if !snapshot.changed_attrs.iter().any(|name| name.0 == *attr) {
                    snapshot.changed_attrs.push(GenericAtomIdent(attr.clone()));
                }
            }
        } else {
            self.snapshots.insert(
                opaque_node_id,
                ServoElementSnapshot {
                    state: Some(*self.nodes[node_id].element_state()),
                    attrs,
                    changed_attrs,
                    class_changed: needs_attrs,
                    id_changed: needs_attrs,
                    other_attributes_changed: needs_attrs,
                },
            );
        }
    }

    /// Returns whether any style rule depends on any of the given [`ElementState`] bits.
    /// If not, changing those bits cannot affect styling and snapshotting can be skipped.
    pub fn style_depends_on_state(&self, state: ElementState) -> bool {
        self.stylist.iter_origins().any(|(data, _)| {
            data.has_state_dependency(state) || data.has_nth_of_state_dependency(state)
        })
    }

    /// Apply a state change (hover/focus/active/etc affecting the given [`ElementState`]
    /// bits) to a node, taking a state-only snapshot beforehand if any style rule
    /// depends on those bits.
    pub fn snapshot_node_and(
        &mut self,
        node_id: NodeId,
        state: ElementState,
        cb: impl FnOnce(&mut Node),
    ) {
        if !self.style_depends_on_state(state) {
            cb(&mut self.nodes[node_id]);
            return;
        }
        self.snapshot_node_state_only(node_id);
        let state_of = |n: &Node| if n.is_element() { *n.element_state() } else { ElementState::empty() };
        let before = state_of(&self.nodes[node_id]);
        cb(&mut self.nodes[node_id]);
        // pyro: the `:has()` anchors that can see the element (a
        // flag test when no `:has()` depends on these states).
        let changed = before ^ state_of(&self.nodes[node_id]);
        self.invalidate_relative_state(node_id, changed);
    }

    // Takes (x, y) co-ordinates (relative to the )
    /// Builds the paint tree first if a resolve left it stale
    /// ([`ensure_paint_tree`](Self::ensure_paint_tree)).
    pub fn hit(&mut self, x: f32, y: f32) -> Option<HitResult> {
        self.hit_with_scrollbar(x, y).0
    }

    /// The topmost element at viewport coordinates (x, y), or `None` if the point
    /// is outside the viewport. Anonymous boxes and text nodes are resolved to
    /// their nearest element; a point over the background hits the root element.
    ///
    /// This implements the hit-testing semantics of `document.elementFromPoint()`.
    /// Hit testing consults layout, so [`resolve`](Self::resolve) should be called
    /// before this method to ensure layout is up to date.
    pub fn element_from_point(&mut self, x: f32, y: f32) -> Option<NodeId> {
        let viewport = self.viewport();
        let scale = viewport.scale();
        let (viewport_width, viewport_height) = (
            viewport.window_size.0 as f32 / scale,
            viewport.window_size.1 as f32 / scale,
        );
        if x < 0.0 || y < 0.0 || x > viewport_width || y > viewport_height {
            return None;
        }

        let Some(hit) = self.hit(x, y) else {
            // The point is within the viewport but over no box: the root element
            // (which covers the viewport in an HTML document) is the hit target
            return self.try_root_element().map(|root| root.id);
        };

        // Resolve anonymous boxes / pseudo-elements, then text nodes, to elements
        let mut node_id = self.nearest_non_anonymous_ancestor(hit.node_id)?;
        loop {
            let node = self.get_node(node_id)?;
            if node.is_element() {
                return Some(node_id);
            }
            node_id = node.parent?;
        }
    }

    /// All elements at viewport coordinates (x, y), from topmost to bottommost,
    /// as for `document.elementsFromPoint()`.
    ///
    /// The spec's paint-order list is approximated with the hit element followed
    /// by its ancestor elements (which is correct for non-overlapping content).
    /// As with [`element_from_point`](Self::element_from_point), layout should be
    /// resolved before calling this method.
    pub fn elements_from_point(&mut self, x: f32, y: f32) -> Vec<NodeId> {
        let mut element_ids = Vec::new();
        let mut current = self.element_from_point(x, y);
        while let Some(node_id) = current {
            element_ids.push(node_id);
            current = self
                .get_node(node_id)
                .and_then(|node| node.parent)
                .filter(|parent_id| {
                    self.get_node(*parent_id)
                        .is_some_and(|node| node.is_element())
                });
        }
        element_ids
    }

    /// Walk up the tree to the nearest DOM node whose id is stable across
    /// box-tree reconstruction, so canonicalized interaction state never goes
    /// stale.
    ///
    /// Layout-generated nodes (anonymous blocks and `::before`/`::after`
    /// pseudo-elements, both stored as anonymous blocks) get new ids on every
    /// reconstruction, so we skip any anonymous node *and* a non-anonymous node
    /// whose parent is anonymous (the pseudo's text content). The first
    /// non-anonymous node with a non-anonymous parent is a real DOM node; the
    /// root element's `Document` parent guarantees termination.
    ///
    /// Returns `None` if `node_id` (or an ancestor) no longer exists.
    pub fn nearest_non_anonymous_ancestor(&self, node_id: NodeId) -> Option<NodeId> {
        // Recurse up the tree keeping a window of the current node and its
        // parent, advancing one step per iteration so each node is looked up
        // exactly once.
        let mut node = self.get_node(node_id)?;
        loop {
            let parent = match node.parent {
                Some(parent_id) => self.get_node(parent_id)?,
                None => return Some(node.id),
            };
            if !node.is_anonymous() && !parent.is_anonymous() {
                return Some(node.id);
            }
            node = parent;
        }
    }

    pub fn focus_next_node(&mut self) -> Option<NodeId> {
        let focussed_node_id = self.get_focussed_node_id()?;
        let id = self.next_node(&self.nodes[focussed_node_id], |node| node.is_focussable())?;
        self.set_focus_to(id);
        Some(id)
    }

    /// Move focus to the previous focussable node in the document
    pub fn focus_prev_node(&mut self) -> Option<NodeId> {
        let focussed_node_id = self.get_focussed_node_id()?;
        let id = self.prev_node(&self.nodes[focussed_node_id], |node| node.is_focussable())?;
        self.set_focus_to(id);
        Some(id)
    }

    /// pyro: the shadow hosts whose shadow trees hold `id` (innermost first): they match `:focus`
    /// while it is focused (CSS Scoping 1 § 3.1: a shadow host is focused when its shadow tree
    /// holds the focused element; `crate::shadow`).
    fn shadow_hosts_of(&self, id: NodeId) -> Vec<NodeId> {
        let mut hosts = Vec::new();
        let mut current = self.nodes.get(id).and_then(|node| node.containing_shadow_host_id());
        while let Some(host) = current {
            hosts.push(host);
            current = self.nodes.get(host).and_then(|node| node.containing_shadow_host_id());
        }
        hosts
    }

    /// pyro: gives (or takes) the shadow hosts of the focused element `id` their `:focus`.
    fn set_shadow_hosts_focus(&mut self, id: Option<NodeId>, focus: bool) {
        let Some(id) = id else { return };
        for host in self.shadow_hosts_of(id) {
            self.snapshot_node_and(host, ElementState::FOCUS, |node| {
                if let Some(data) = node.element_data_mut() {
                    data.element_state.set(ElementState::FOCUS, focus);
                }
                node.mark_ancestors_dirty();
            });
        }
    }

    /// Clear the focussed node
    pub fn clear_focus(&mut self) {
        if let Some(id) = self.focus_node_id {
            self.set_shadow_hosts_focus(Some(id), false);
            let shell_provider = self.shell_provider.clone();
            self.snapshot_node_and(id, ElementState::FOCUS | ElementState::FOCUSRING, |node| {
                node.blur(shell_provider)
            });
            for &ancestor in self.node_element_ancestors(id).iter() {
                self.snapshot_node_and(ancestor, ElementState::FOCUS_WITHIN, |node| {
                    node.remove_focus_within()
                });
            }
            self.focus_node_id = None;
            self.has_changes = true;
            self.shell_provider.request_redraw();
        }
    }

    pub fn set_mousedown_node_id(&mut self, node_id: Option<NodeId>) {
        self.mousedown_node_id = node_id.and_then(|id| self.nearest_non_anonymous_ancestor(id));
    }
    pub fn set_focus_to(&mut self, focus_node_id: NodeId) -> bool {
        let Some(focus_node_id) = self.nearest_non_anonymous_ancestor(focus_node_id) else {
            return false;
        };
        if Some(focus_node_id) == self.focus_node_id {
            return false;
        }
        // A node that is not in the document cannot be focused
        if !self.nodes[focus_node_id].flags.is_in_document() {
            return false;
        }

        #[cfg(feature = "tracing")]
        tracing::info!("Focussed node {focus_node_id}");

        let shell_provider = self.shell_provider.clone();

        // Remove focus from the old node
        if let Some(id) = self.focus_node_id {
            self.set_shadow_hosts_focus(Some(id), false);
            self.snapshot_node_and(id, ElementState::FOCUS | ElementState::FOCUSRING, |node| {
                node.blur(shell_provider.clone())
            });
        }

        // Focus the new node
        let ring = self.focus_ring;
        self.snapshot_node_and(
            focus_node_id,
            ElementState::FOCUS | ElementState::FOCUSRING,
            |node| {
                node.focus(shell_provider);
                if let Some(data) = node.element_data_mut() {
                    data.element_state.set(ElementState::FOCUSRING, ring);
                }
            },
        );
        self.set_shadow_hosts_focus(Some(focus_node_id), true);

        // Update :focus-within on the ancestor chains (shared prefix stays)
        let old_node_path = self.maybe_node_element_ancestors(self.focus_node_id);
        let new_node_path = self.node_element_ancestors(focus_node_id);
        let same_count = old_node_path
            .iter()
            .zip(&new_node_path)
            .take_while(|(o, n)| o == n)
            .count();
        for &id in old_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::FOCUS_WITHIN, |node| {
                node.remove_focus_within()
            });
        }
        for &id in new_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::FOCUS_WITHIN, |node| {
                node.add_focus_within()
            });
        }

        self.focus_node_id = Some(focus_node_id);
        self.has_changes = true;

        // The focus is styled (:focus, :focus-visible, :focus-within), so
        // moving it changes what is on screen. Nothing else asks for the
        // frame: a keyboard focus change touches no layout and no content.
        self.shell_provider.request_redraw();

        true
    }

    /// Whether the focused node (now and later) matches `:focus-visible`;
    /// true when a focused node's state changed.
    pub fn set_focus_ring(&mut self, on: bool) -> bool {
        if self.focus_ring == on {
            return false;
        }
        self.focus_ring = on;
        let Some(id) = self.focus_node_id else { return false };
        self.snapshot_node_and(id, ElementState::FOCUSRING, |node| {
            if let Some(data) = node.element_data_mut() {
                data.element_state.set(ElementState::FOCUSRING, on);
            }
            node.mark_ancestors_dirty();
        });
        true
    }

    /// Puts `:active` on `node` and its ancestors (off the previous active
    /// node's); true when anything changed.
    pub fn set_active_to(&mut self, node: Option<NodeId>) -> bool {
        let node = node.and_then(|id| self.nearest_non_anonymous_ancestor(id));
        if node == self.active_node_id {
            return false;
        }
        self.unactive_node();
        for &id in self.maybe_node_element_ancestors(node).iter() {
            self.snapshot_node_and(id, ElementState::ACTIVE, |node| node.active());
        }
        self.active_node_id = node;
        true
    }

    pub fn active_node(&mut self) -> bool {
        let Some(hover_node_id) = self.get_hover_node_id() else {
            return false;
        };

        if let Some(active_node_id) = self.active_node_id {
            if active_node_id == hover_node_id {
                return true;
            }
            self.unactive_node();
        }

        // hover_node_id is canonicalized when stored, so this always holds.
        debug_assert!(
            self.get_node(hover_node_id)
                .is_some_and(|node| !node.is_anonymous()),
            "interaction state must reference DOM nodes, not layout-generated nodes"
        );
        let active_node_id = Some(hover_node_id);

        let node_path = self.maybe_node_element_ancestors(active_node_id);
        for &id in node_path.iter() {
            self.snapshot_node_and(id, ElementState::ACTIVE, |node| node.active());
        }

        self.active_node_id = active_node_id;

        true
    }

    pub fn unactive_node(&mut self) -> bool {
        let Some(active_node_id) = self.active_node_id.take() else {
            return false;
        };

        let node_path = self.maybe_node_element_ancestors(Some(active_node_id));
        for &id in node_path.iter() {
            self.snapshot_node_and(id, ElementState::ACTIVE, |node| node.unactive());
        }

        true
    }

    /// The scrollbar thumb currently under the pointer, if any.
    pub fn hovered_scrollbar(&self) -> Option<crate::node::ScrollbarRef> {
        self.hovered_scrollbar
    }

    /// The scrollbar thumb currently being dragged, if any.
    pub fn scrollbar_drag_target(&self) -> Option<crate::node::ScrollbarRef> {
        match &self.drag_mode {
            DragMode::ScrollbarDrag(state) => Some(state.scrollbar),
            _ => None,
        }
    }

    /// The current opacity of `node_id`'s overlay scrollbars. They show at
    /// full opacity on scroll and fade out after a delay (Chromium's overlay
    /// timings); the pointer resting on a thumb, or dragging it, holds them
    /// visible.
    pub fn scrollbar_opacity(&self, node_id: NodeId) -> f32 {
        let interacting = |scrollbar: &crate::node::ScrollbarRef| scrollbar.node_id == node_id;
        if self.hovered_scrollbar.as_ref().is_some_and(interacting)
            || self
                .scrollbar_drag_target()
                .as_ref()
                .is_some_and(interacting)
        {
            return 1.0;
        }
        self.scrollbar_activity.get(&node_id).map_or(0.0, |last| {
            crate::node::scrollbar::opacity_at(last.elapsed())
        })
    }

    /// Show `node_id`'s overlay scrollbars at full opacity and restart their
    /// fade-out delay.
    pub(crate) fn show_scrollbars(&mut self, node_id: NodeId) {
        if cfg!(feature = "scrollbars") {
            self.scrollbar_activity.insert(node_id, Instant::now());
        }
    }

    /// Whether any overlay scrollbars are awaiting or animating their
    /// fade-out (so frames must keep rendering until they finish).
    fn scrollbars_animating(&self) -> bool {
        use crate::node::scrollbar::{FADE_DELAY, FADE_DURATION};
        self.scrollbar_activity
            .values()
            .any(|last| last.elapsed() < FADE_DELAY + FADE_DURATION)
    }

    /// [`hit`](Self::hit), also resolving the innermost overlay scrollbar
    /// thumb under the point (shares the traversal, so it costs nothing
    /// extra).
    pub(crate) fn hit_with_scrollbar(
        &mut self,
        x: f32,
        y: f32,
    ) -> (Option<HitResult>, Option<crate::node::ScrollbarRef>) {
        // Hit testing walks `paint_children` and the stacking contexts.
        self.ensure_paint_tree();
        if TDocument::as_node(&self.root_node())
            .first_element_child()
            .is_none()
        {
            #[cfg(feature = "tracing")]
            tracing::warn!("No DOM - not resolving hit test");
            return (None, None);
        }
        let mut scrollbar = None;
        let hit = self
            .root_element()
            .hit_inner(x, y, self.viewport().scale_f64(), &mut scrollbar);
        (hit, scrollbar)
    }

    pub fn set_hover_to(&mut self, x: f32, y: f32) -> bool {
        // Record the pointer position in client (unscrolled) coordinates so
        // that `refresh_hover` can re-resolve hover state after layout or
        // scroll changes.
        self.last_client_pointer_position = Some(taffy::Point {
            x: x - self.viewport_scroll().x as f32,
            y: y - self.viewport_scroll().y as f32,
        });

        let (hit, hovered_scrollbar) = self.hit_with_scrollbar(x, y);
        // A faded-out thumb is not interactive: pointer moves never fade
        // overlay scrollbars back in (only scrolling shows them).
        let hovered_scrollbar =
            hovered_scrollbar.filter(|scrollbar| self.scrollbar_opacity(scrollbar.node_id) > 0.0);
        // Scrollbar-thumb hover is part of hover state: track it here so a
        // pointer crossing a thumb restyles it even when the hit node (the
        // content under the overlay thumb) is unchanged.
        let scrollbar_changed = hovered_scrollbar != self.hovered_scrollbar;
        if scrollbar_changed {
            // Entering a thumb restores full opacity mid-fade; leaving one
            // restarts the fade-out delay.
            for scrollbar in [self.hovered_scrollbar, hovered_scrollbar]
                .into_iter()
                .flatten()
            {
                self.show_scrollbars(scrollbar.node_id);
            }
        }
        self.hovered_scrollbar = hovered_scrollbar;

        // Store both the precise layout node that was hit (transient: used for
        // cursor/style queries) and its canonical DOM target (persistent: must
        // not reference layout-generated nodes, whose ids die on box-tree
        // reconstruction).
        let hit_node_id = hit.map(|hit| hit.node_id);
        let hover_node_id = hit_node_id.and_then(|id| self.nearest_non_anonymous_ancestor(id));
        let new_is_text = hit.map(|hit| hit.is_text).unwrap_or(false);

        self.hover_hit_node_id = hit_node_id;
        self.hover_node_is_text = new_is_text;

        // Return early if the new node is the same as the already-hovered node
        if hover_node_id == self.hover_node_id {
            return scrollbar_changed;
        }

        let old_node_path = self.maybe_node_element_ancestors(self.hover_node_id);
        let new_node_path = self.maybe_node_element_ancestors(hover_node_id);
        let same_count = old_node_path
            .iter()
            .zip(&new_node_path)
            .take_while(|(o, n)| o == n)
            .count();
        for &id in old_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::HOVER, |node| node.unhover());
        }
        for &id in new_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::HOVER, |node| node.hover());
        }

        self.hover_node_id = hover_node_id;

        // Request redraw
        self.shell_provider.request_redraw();

        true
    }

    /// Puts `:hover` on `node` and its ancestors (and off the previous
    /// ones), as a pointer over it would: for an embedder that tracks the
    /// pointer itself. `None` clears hover.
    /// pyro: the element `:target` matches from now on (the embedder's indicated element, from
    /// the document URL's fragment): its `URLTARGET` state. Whether it changed.
    pub fn set_url_target(&mut self, node: Option<NodeId>) -> bool {
        if node == self.url_target {
            return false;
        }
        if let Some(old) = self.url_target.filter(|&id| self.nodes.get(id).is_some_and(Node::is_element)) {
            self.snapshot_node_and(old, ElementState::URLTARGET, |node| {
                if let Some(data) = node.element_data_mut() {
                    data.element_state.remove(ElementState::URLTARGET);
                }
                node.mark_ancestors_dirty();
            });
        }
        if let Some(new) = node.filter(|&id| self.nodes.get(id).is_some_and(Node::is_element)) {
            self.snapshot_node_and(new, ElementState::URLTARGET, |node| {
                if let Some(data) = node.element_data_mut() {
                    data.element_state.insert(ElementState::URLTARGET);
                }
                node.mark_ancestors_dirty();
            });
        }
        self.url_target = node;
        true
    }

        pub fn set_hover_node(&mut self, node: Option<NodeId>) -> bool {
        let node = node.and_then(|id| self.nearest_non_anonymous_ancestor(id));
        if node == self.hover_node_id {
            return false;
        }
        let old_node_path = self.maybe_node_element_ancestors(self.hover_node_id);
        let new_node_path = self.maybe_node_element_ancestors(node);
        let same_count = old_node_path.iter().zip(&new_node_path).take_while(|(o, n)| o == n).count();
        for &id in old_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::HOVER, |node| node.unhover());
        }
        for &id in new_node_path.iter().skip(same_count) {
            self.snapshot_node_and(id, ElementState::HOVER, |node| node.hover());
        }
        self.hover_node_id = node;
        self.hover_hit_node_id = node;
        true
    }

    pub fn clear_hover(&mut self) -> bool {
        // The pointer is no longer over the document, so stop re-resolving
        // hover state against it.
        self.last_client_pointer_position = None;
        self.hover_hit_node_id = None;

        let Some(hover_node_id) = self.hover_node_id else {
            return false;
        };

        let old_node_path = self.maybe_node_element_ancestors(Some(hover_node_id));
        for &id in old_node_path.iter() {
            self.snapshot_node_and(id, ElementState::HOVER, |node| node.unhover());
        }

        self.hover_node_id = None;
        self.hover_node_is_text = false;

        // Request redraw
        self.shell_provider.request_redraw();

        true
    }

    /// Re-resolve hover state against the current layout using the last known
    /// pointer position.
    ///
    /// TODO: synthesizing pointerenter/pointerleave DOM events for
    /// hover changes caused by layout shifts.
    pub fn refresh_hover(&mut self) -> bool {
        let Some(pos) = self.last_client_pointer_position else {
            return false;
        };
        let x = pos.x + self.viewport_scroll().x as f32;
        let y = pos.y + self.viewport_scroll().y as f32;
        let prev_hover = self.hover_state();
        let changed = self.set_hover_to(x, y);
        if self.hover_state() != prev_hover {
            self.update_cursor();
        }
        changed
    }

    /// The hover state that determines the cursor: the hovered node, the precise hit
    /// node and whether the hit is text
    pub(crate) fn hover_state(&self) -> (Option<NodeId>, Option<NodeId>, bool) {
        (
            self.hover_node_id,
            self.hover_hit_node_id,
            self.hover_node_is_text,
        )
    }

    /// Send the current cursor to the shell.
    ///
    /// Hover updates don't send the cursor themselves: the cursor of a node hosting a
    /// sub-document is only known once the sub-document has handled the event.
    pub(crate) fn update_cursor(&self) {
        self.shell_provider.set_cursor(self.get_cursor());
    }

    pub fn get_hover_node_id(&self) -> Option<NodeId> {
        self.hover_node_id
    }

    pub fn get_mousedown_node_id(&self) -> Option<NodeId> {
        self.mousedown_node_id
    }

    /// Lays out at this size (CSS px) instead of the viewport's: `viewport` is
    /// what viewport units and media queries see, `available` what the root
    /// element fills. Print has both: Blink's page area rounds up for the
    /// content and its initial containing block rounds down. `None` goes back.
    pub fn set_layout_size(&mut self, size: Option<((f32, f32), (f32, f32))>) {
        let viewport = size.map(|s| s.0);
        self.layout_available = size.map(|s| s.1);
        if self.layout_size != viewport {
            self.layout_size = viewport;
            self.queue_device_changes(DeviceChanges::VIEWPORT_SIZE);
        }
    }

    pub fn set_viewport(&mut self, viewport: Viewport) {
        let changes = DeviceChanges::from_viewports(&self.viewport, &viewport);
        self.viewport = viewport;
        self.queue_device_changes(changes);
    }

    /// pyro: an inline `<svg>` whose source depends on its laid-out size (`SvgImageData::viewport_dependent`),
    /// serialized and parsed again for a content box of `width` x `height` CSS px (kept until the size
    /// changes); `None` when that fails.
    #[cfg(feature = "svg")]
    pub fn inline_svg_at_viewport(&self, node_id: NodeId, width: f32, height: f32) -> Option<Arc<crate::node::SvgImageData>> {
        let key = ((width * 64.0).round() as u32, (height * 64.0).round() as u32);
        if let Some((size, svg)) = self.viewport_svgs.borrow().get(&node_id) {
            if *size == key {
                return svg.clone();
            }
        }
        let node = self.get_node(node_id)?;
        let (source, _) = node.svg_source_for_viewport(Some((width as f64, height as f64)));
        let source = self.with_cross_svg_references(self.with_svg_sprites(source));
        let svg = crate::util::parse_svg_resources(source.as_bytes(), Some(&self.svg_images_for_paint()), &self.raster_budget).ok().map(|mut svg| {
            if svg.intrinsic_dimensions.view_box.is_none() {
                svg.inline_source = crate::util::SvgSource::copy(&source, &self.raster_budget);
            }
            Arc::new(svg)
        });
        self.viewport_svgs.borrow_mut().insert(node_id, (key, svg.clone()));
        svg
    }

    /// pyro: HTML-in-Canvas privacy: an inline `<svg>` painted for a canvas a page can read back
    /// (`UnvisitedPaint` is active), serialised and parsed again for a content box of `width` x
    /// `height` CSS px: with the colours of its elements as unvisited (the serialisation reads their
    /// styles as painted), without the resources of other `<svg>`s or sprite files
    /// ([`Self::with_cross_svg_references`], [`Self::with_svg_sprites`]) and without the images of
    /// opaque responses ([`Self::svg_images_for_paint`]). Not kept (the `<svg>` built for the page
    /// is). `None` when the source does not parse.
    #[cfg(feature = "svg")]
    pub fn inline_svg_for_privacy(&self, node_id: NodeId, width: f32, height: f32) -> Option<Arc<crate::node::SvgImageData>> {
        let node = self.get_node(node_id)?;
        let (mut source, _) = node.svg_source_for_viewport(Some((width as f64, height as f64)));
        // A filter with the image of an opaque response is not applied (its `url(#id)` is `none`,
        // which makes the whole `filter` value invalid).
        for id in self.opaque_filter_ids(node_id) {
            for quote in ["", "\"", "'"] {
                source = source.replace(&format!("url({quote}#{id}{quote})"), "none");
            }
        }
        let source = self.with_cross_svg_references(self.with_svg_sprites(source));
        let resources = self.svg_images_for_paint();
        crate::util::parse_svg_resources(source.as_bytes(), Some(&resources), &self.raster_budget).ok().map(|mut svg| {
            if svg.intrinsic_dimensions.view_box.is_none() {
                svg.inline_source = crate::util::SvgSource::copy(&source, &self.raster_budget);
            }
            Arc::new(svg)
        })
    }

    /// pyro: HTML-in-Canvas privacy: whether the `<filter>` `filter` has an `feImage` whose image
    /// came from an opaque response ([`Self::svg_images`]). A filter that does is not applied
    /// to what is painted for a canvas a page can read back (Chromium ignores it, it does not
    /// draw the element without its output).
    pub fn filter_has_opaque_image(&self, filter: NodeId) -> bool {
        let Ok(images) = self.svg_images.read() else { return true };
        let mut stack = vec![filter];
        while let Some(id) = stack.pop() {
            let Some(node) = self.get_node(id) else { continue };
            if let Some(element) = node.element_data() {
                if element.name.ns == markup5ever::ns!(svg) && (&*element.name.local).eq_ignore_ascii_case("feImage") {
                    let opaque = element.attrs().iter().any(|attr| {
                        &*attr.name.local == "href" && images.get::<str>(&attr.value).is_some_and(|image| image.is_opaque())
                    });
                    if opaque {
                        return true;
                    }
                }
            }
            stack.extend(node.children.iter().copied());
        }
        false
    }

    /// pyro: HTML-in-Canvas privacy: the ids of the `<filter>`s in the subtree of `root` that
    /// [`Self::filter_has_opaque_image`] names.
    #[cfg(feature = "svg")]
    fn opaque_filter_ids(&self, root: NodeId) -> Vec<String> {
        let mut ids = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.get_node(id) else { continue };
            if let Some(element) = node.element_data() {
                if element.name.ns == markup5ever::ns!(svg) && &*element.name.local == "filter" {
                    let named = element.attrs().iter().find(|attr| &*attr.name.local == "id").map(|attr| attr.value.to_string());
                    if let Some(named) = named.filter(|_| self.filter_has_opaque_image(id)) {
                        ids.push(named);
                    }
                }
            }
            stack.extend(node.children.iter().copied());
        }
        ids
    }

    /// pyro: [`Self::svg_images`], as what is painted for a canvas a page can read back may use it
    /// (`UnvisitedPaint` is active): without the resources of opaque responses (another origin
    /// without CORS, a redirect through one), which usvg then finds no image for and leaves out
    /// (an `<image>`, or an `feImage`, which is then empty). Kept until the map changes.
    pub fn svg_images_for_paint(&self) -> crate::util::SvgResources {
        if !blitz_dom_privacy_paint() {
            return self.svg_images.clone();
        }
        let Ok(images) = self.svg_images.read() else { return Default::default() };
        // (The map's generation, read under the read lock: a change after it is another one.)
        let generation = self.svg_images.generation();
        if let Some((known, kept)) = &*self.privacy_svg_images.borrow()
            && *known == generation
        {
            return kept.clone();
        }
        let kept: crate::util::SvgResources = Arc::new(crate::util::SvgResourceMap::new(
            images.iter().filter(|(_, image)| !image.is_opaque()).map(|(href, image)| (href.clone(), image.clone())).collect(),
        ));
        *self.privacy_svg_images.borrow_mut() = Some((generation, kept.clone()));
        kept
    }

    pub(crate) fn with_svg_sprites(&self, mut svg: String) -> String {
        // pyro: HTML-in-Canvas privacy: a sprite file is fetched without a record of its origin
        // (`SvgResource::opaque` is false for all), so none is merged into what a canvas a page
        // can read back is painted from: the `<use>` of one draws nothing there.
        if self.svg_sprites.is_empty() || blitz_dom_privacy_paint() {
            return svg;
        }
        let mut defs = String::new();
        const MAX_EXPANDED: usize = 8 << 20;
        for (href, sprite) in &self.svg_sprites {
            let attr = format!("href=\"{}\"", href.replace('&', "&amp;").replace('"', "&quot;"));
            let count = svg.matches(&attr).count();
            if count == 0 { continue; }
            let replacement = format!("href=\"#{}\"", sprite.local);
            let needed = svg.len().saturating_add(replacement.len().saturating_sub(attr.len()).saturating_mul(count))
                .saturating_add(defs.len()).saturating_add(sprite.prefix.len()).saturating_add(sprite.suffix.len())
                .saturating_add(sprite.local.len()).saturating_add(13);
            if needed > MAX_EXPANDED { continue; }
            let Ok(text) = std::str::from_utf8(&sprite.source.bytes) else { continue };
            svg = svg.replace(&attr, &replacement);
            defs.push_str(&text[sprite.prefix.clone()]);
            defs.push_str(&sprite.local);
            defs.push_str(&text[sprite.suffix.clone()]);
        }
        if let (false, Some(end)) = (defs.is_empty(), svg.find('>')) {
            svg.insert_str(end + 1, &format!("<defs>{defs}</defs>"));
        }
        svg
    }

    /// pyro: `svg` (an inline `<svg>`'s source) with the resources of the page's other inline
    /// `<svg>`s that it references and does not define itself (`filter="url(#f)"`,
    /// `fill="url(#gradient)"`, a gradient's `href="#base"`, where the `<filter>` or gradient
    /// stands in a hidden `<svg>` of definitions), copied into a `<defs>` first in it: ids are
    /// the document's, and usvg reads each `<svg>` alone. What a copied element references is
    /// copied too. (A later change to the definitions does not rebuild the `<svg>`s that use
    /// them.)
    #[cfg(feature = "svg")]
    pub(crate) fn with_cross_svg_references(&self, mut svg: String) -> String {
        if !svg.contains("(#") && !svg.contains("href=\"#") {
            return svg;
        }
        // pyro: HTML-in-Canvas privacy: a resource defined outside the painted subtree (a gradient,
        // pattern, marker, filter, mask or clip path of another `<svg>`) is not read by it: its
        // colours (`currentColor` of the link around it) and images are the page's, not the
        // subtree's. The reference is then unresolved, and drawn as usvg draws one (none, or no
        // paint).
        if blitz_dom_privacy_paint() {
            return svg;
        }
        fn referenced(text: &str, out: &mut Vec<String>) {
            for (open, close) in [("url(#", ')'), ("href=\"#", '"')] {
                let mut rest = text;
                while let Some(start) = rest.find(open) {
                    let after = &rest[start + open.len()..];
                    let Some(end) = after.find(close) else {
                        break;
                    };
                    let id = after[..end].trim();
                    if !id.is_empty() && !out.iter().any(|known| known == id) {
                        out.push(id.to_string());
                    }
                    rest = &after[end..];
                }
            }
        }
        let defined = |text: &str, id: &str| text.contains(&format!(" id=\"{id}\""));
        let mut ids = Vec::new();
        referenced(&svg, &mut ids);
        let mut defs = String::new();
        let mut next = 0;
        while next < ids.len() && next < 64 {
            let id = ids[next].clone();
            next += 1;
            if defined(&svg, &id) || defined(&defs, &id) {
                continue;
            }
            let Some(node) = self.get_element_by_id(&id).and_then(|node_id| self.get_node(node_id)) else {
                continue;
            };
            let resource = node.element_data().is_some_and(|data| {
                data.name.ns == markup5ever::ns!(svg)
                    && matches!(
                        data.name.local.as_ref(),
                        "filter" | "clipPath" | "mask" | "linearGradient" | "radialGradient" | "pattern" | "marker"
                    )
            });
            // pyro: nor one in contents `content-visibility: hidden` skips (CSS Contain 2 § 4.1:
            // not rendered, its resources neither; WPT css-contain
            // `content-visibility/content-visibility-svg-gradient`).
            if !resource || self.skipped_by_content_visibility(node.id) {
                continue;
            }
            let source = node.svg_source();
            if svg.len().saturating_add(defs.len()).saturating_add(source.len()).saturating_add(13) > 8 << 20 { continue; }
            referenced(&source, &mut ids);
            defs.push_str(&source);
        }
        if let (false, Some(end)) = (defs.is_empty(), svg.find('>')) {
            svg.insert_str(end + 1, &format!("<defs>{defs}</defs>"));
        }
        svg
    }

    /// Returns the current CSS media type used to evaluate `@media` rules.
    pub fn media_type(&self) -> &MediaType {
        &self.media_type
    }

    /// Sets the CSS media type used to evaluate `@media` rules (e.g. `screen` or `print`)
    /// and queues a stylist device rebuild so updated rules apply on the next restyle.
    pub fn set_media_type(&mut self, media_type: MediaType) {
        if self.media_type == media_type {
            return;
        }
        self.media_type = media_type;
        self.queue_device_changes(DeviceChanges::MEDIA_TYPE);
    }

    pub fn set_media_environment(&mut self, environment: crate::MediaEnvironment) {
        if self.media_environment != environment {
            self.media_environment = environment;
            self.queue_device_changes(DeviceChanges::MEDIA_TYPE);
        }
    }

    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    pub fn viewport_mut(&mut self) -> ViewportMut<'_> {
        ViewportMut::new(self)
    }

    pub fn zoom_by(&mut self, increment: f32) {
        *self.viewport_mut().zoom_mut() += increment;
    }

    pub fn zoom_to(&mut self, zoom: f32) {
        *self.viewport_mut().zoom_mut() = zoom;
    }

    pub fn get_viewport(&self) -> Viewport {
        self.viewport.clone()
    }

    /// The line-break override the document was created with
    /// ([`crate::DocumentConfig::line_break_override`]).
    pub fn line_break_override(&self) -> Option<&'static parley::LineBreakOverrideFn> {
        self.layout_ctx.line_break_override()
    }

    /// Returns whether incremental layout is currently enabled for this document.
    ///
    /// Both modes run the same style → damage → box construction → layout
    /// pipeline. In incremental mode only nodes carrying `RestyleDamage` (from
    /// mutations or restyles) are reconstructed and have their layout caches
    /// cleared; in non-incremental mode every node is marked damaged at the
    /// start of each `resolve`, so everything is rebuilt each frame.
    pub fn incremental_layout(&self) -> bool {
        self.incremental_layout
    }

    /// Enables or disables incremental layout for this document.
    ///
    /// Disabling it does not switch to a separate code path: all nodes are
    /// simply treated as damaged on every `resolve` (see [`Self::incremental_layout`]).
    pub fn set_incremental_layout(&mut self, enabled: bool) {
        self.incremental_layout = enabled;
    }

    pub fn devtools(&self) -> &DevtoolSettings {
        &self.devtool_settings
    }

    pub fn devtools_mut(&mut self) -> &mut DevtoolSettings {
        &mut self.devtool_settings
    }

    pub fn subdoc(&self, node_id: NodeId) -> Option<&dyn Document> {
        self.get_node(node_id)
            .and_then(|node| node.element_data())
            .and_then(|el| el.sub_doc_data())
    }

    pub fn subdoc_mut(&mut self, node_id: NodeId) -> Option<&mut dyn Document> {
        self.get_node_mut(node_id)
            .and_then(|node| node.element_data_mut())
            .and_then(|el| el.sub_doc_data_mut())
    }

    pub fn is_animating(&self) -> bool {
        #[cfg(feature = "custom-widget")]
        let custom_widget_is_animating = self.custom_widget_nodes.iter().any(|&node_id| {
            self.nodes[node_id]
                .element_data()
                .and_then(|el| el.custom_widget_data())
                .is_some_and(|data| data.widget.requires_redraw())
        });
        #[cfg(not(feature = "custom-widget"))]
        let custom_widget_is_animating = false;

        self.has_canvas
            | self.has_active_animations
            | self.subdoc_is_animating
            | custom_widget_is_animating
            | (self.scroll_animation != ScrollAnimationState::None)
            | self.scrollbars_animating()
    }

    /// Record pending [`DeviceChanges`] and request a redraw so that they are
    /// flushed (via [`Self::flush_pending_device_changes`]) on the next resolve.
    /// pyro: HTML `<meta name=color-scheme>`: the first such element (tree order) whose content
    /// is a valid value gives the page's supported color schemes; a change rebuilds the device.
    pub(crate) fn update_page_color_scheme(&mut self) {
        use style::values::specified::color::ColorSchemeFlags;
        let mut flags = None;
        // Only `<meta name=color-scheme>` elements in the document, in tree order (by a walk when
        // there are several).
        let mut candidates: Vec<NodeId> = Vec::new();
        let nodes = &self.nodes;
        self.meta_nodes.retain(|&id| nodes.get(id).is_some_and(|n| n.element_data().is_some_and(|e| e.name.local == markup5ever::local_name!("meta"))));
        for &id in &self.meta_nodes {
            let node = &self.nodes[id];
            // In the document tree, not a shadow tree.
            if node.is_in_document()
                && node.containing_shadow_host_id().is_none()
                && node.element_data().is_some_and(|e| e.attr(markup5ever::local_name!("name")).is_some_and(|n| n.trim().eq_ignore_ascii_case("color-scheme")))
            {
                candidates.push(id);
            }
        }
        if candidates.is_empty() && self.page_color_scheme.is_empty() {
            return;
        }
        if candidates.len() > 1 {
            let mut order = Vec::new();
            let root = self.root_node_id;
            self.iter_subtree_mut(root, |id, _| {
                if candidates.contains(&id) {
                    order.push(id);
                }
            });
            candidates = order;
        }
        for id in candidates {
            let node = &self.nodes[id];
            let Some(el) = node.element_data() else { continue };
            if el.name.local != markup5ever::local_name!("meta")
                || !el.attr(markup5ever::local_name!("name")).is_some_and(|n| n.trim().eq_ignore_ascii_case("color-scheme"))
            {
                continue;
            }
            let Some(content) = el.attr(markup5ever::local_name!("content")) else { continue };
            let tokens: Vec<String> = content.split_ascii_whitespace().map(str::to_ascii_lowercase).collect();
            let parsed = if tokens.len() == 1 && tokens[0] == "normal" {
                Some(ColorSchemeFlags::empty())
            } else if !tokens.is_empty() && !tokens.iter().any(|t| t == "normal") {
                let mut f = ColorSchemeFlags::empty();
                for t in &tokens {
                    match t.as_str() {
                        "light" => f |= ColorSchemeFlags::LIGHT,
                        "dark" => f |= ColorSchemeFlags::DARK,
                        "only" => f |= ColorSchemeFlags::ONLY,
                        _ => {}
                    }
                }
                (f.intersects(ColorSchemeFlags::LIGHT | ColorSchemeFlags::DARK)).then_some(f)
            } else {
                None
            };
            if parsed.is_some() {
                flags = parsed;
                break;
            }
        }
        let flags = flags.unwrap_or_else(ColorSchemeFlags::empty);
        if flags != self.page_color_scheme {
            self.page_color_scheme = flags;
            self.queue_device_changes(DeviceChanges::COLOR_SCHEME);
        }
    }

    /// pyro: the rendered legend of fieldset `fieldset` (HTML § 15.3.13): its first layout child when
    /// that is a `<legend>` (`layout::construct::place_rendered_legend` puts it first), laid out over
    /// the fieldset's block-start border.
    pub fn fieldset_rendered_legend(&self, fieldset: NodeId) -> Option<NodeId> {
        let node = self.nodes.get(fieldset)?;
        if !node.is_fieldset() || node.flags.is_inline_root() {
            return None;
        }
        let first = node.layout_children.borrow().as_ref()?.first().copied()?;
        let legend = self.nodes.get(first)?;
        (legend.data.downcast_element().is_some_and(|el| el.name.local == markup5ever::local_name!("legend"))
            && legend.primary_styles().is_some_and(|style| {
                !style.get_box().float.is_floating() && !crate::node::style_is_out_of_flow(&style)
            }))
        .then_some(first)
    }

    /// pyro: whether the page declares its color schemes (a `<meta name=color-scheme>`).
    pub fn page_color_scheme_is_declared(&self) -> bool {
        !self.page_color_scheme.is_empty()
    }

    /// pyro: whether the page's color scheme (its `<meta name=color-scheme>`, with the user's
    /// preference when it supports both) is dark: the canvas is painted dark then.
    pub fn page_color_scheme_is_dark(&self) -> bool {
        use style::values::specified::color::ColorSchemeFlags;
        let flags = self.page_color_scheme;
        let (dark, light) = (flags.contains(ColorSchemeFlags::DARK), flags.contains(ColorSchemeFlags::LIGHT));
        if dark != light {
            return dark;
        }
        dark && self.viewport.color_scheme == blitz_traits::shell::ColorScheme::Dark
    }

    pub(crate) fn queue_device_changes(&mut self, changes: DeviceChanges) {
        if changes.is_empty() {
            return;
        }
        self.pending_device_changes |= changes;
        self.shell_provider.request_redraw();
    }

    /// Apply any pending device changes to the stylist, coalescing all changes
    /// since the last flush into a single device rebuild.
    pub(crate) fn flush_pending_device_changes(&mut self) {
        let changes = std::mem::take(&mut self.pending_device_changes);
        if changes.is_empty() {
            return;
        }

        let mut device = make_device(&self.viewport, self.layout_size, self.media_type.clone(), self.media_environment, self.font_ctx.clone());
        device.set_document_url(self.fragment_document.as_ref().unwrap_or(&self.url).inline_url_data().clone());
        device.set_page_color_scheme(self.page_color_scheme);
        device.set_viewport_unit_inset(self.viewport_unit_inset);
        self.set_stylist_device(device);
        self.scroll_viewport_by(0.0, 0.0); // Clamp scroll offset
        // pyro: shadow roots' cascade data holds their sheets' media query results.
        self.invalidate_shadow_styles();

        // Text is shaped at a specific scale factor, so cached inline layouts
        // must be invalidated when the scale changes, as must every node's
        // transform and scrollable overflow (kept in device pixels).
        if changes.contains(DeviceChanges::SCALE) {
            self.invalidate_inline_contexts();
            self.flags.insert(DocumentFlags::RESOLVE_ALL_TRANSFORMS);
        }

        // Color-scheme changes affect values that are resolved at cascade time
        // (`light-dark()`, system colors) without necessarily flipping any
        // media query result, so conservatively recascade the whole tree.
        if changes.contains(DeviceChanges::COLOR_SCHEME) {
            if let Some(root_id) = self.try_root_element().map(|el| el.id) {
                self.nodes[root_id].set_restyle_hint(RestyleHint::recascade_subtree());
            }
        }
    }

    /// Update the device and reset the stylist to process the new size
    pub fn set_stylist_device(&mut self, device: Device) {
        // Seed the new device with the root element's current style and font-relative
        // unit state (used to resolve rem/rlh/rex/rch/rcap/ric units). Stylo only
        // updates this state when the root element's style *changes* during a restyle,
        // so a freshly-built device would otherwise resolve these units against the
        // default font-size (16px) until the root's font-size next changes.
        let root_styles = self
            .try_root_element()
            .and_then(|root| root.primary_styles());
        if let Some(root_style) = root_styles.as_deref() {
            device.set_root_style(root_style);

            let font = root_style.get_font();
            let font_size = font.slow_clone_font_size().computed_size();
            device.set_root_font_size(root_style.effective_zoom.unzoom(font_size.px()));

            let line_height = device
                .calc_line_height(font, root_style.writing_mode, None)
                .0;
            device.set_root_line_height(root_style.effective_zoom.unzoom(line_height.px()));
        }
        drop(root_styles);

        let old_device = self.stylist.device();
        let style_uses_media_conditions = old_device.style_uses_media_conditions();
        if style_uses_media_conditions {
            device.note_style_media_condition();
        }
        if old_device.style_uses_font_metrics() {
            device.note_style_font_metrics();
        }
        let viewport_size_changed = old_device.au_viewport_size() != device.au_viewport_size()
            || old_device.au_viewport_unit_size() != device.au_viewport_unit_size();
        let styles_use_viewport_units = old_device.used_viewport_size();

        // Percentage/auto boxes can resize without a computed-style change.
        // Keep their geometry dirty until layout, even if a public device read
        // flushes the queued update before resolve_style gets to inspect it.
        if viewport_size_changed {
            if let Some(root_id) = self.try_root_element().map(|root| root.id) {
                self.nodes[root_id].insert_damage(crate::layout::damage::ONLY_RELAYOUT);
            }
        }

        let origins = {
            let guard = &self.guard;
            let guards = StylesheetGuards {
                author: &guard.read(),
                ua_or_user: &guard.read(),
            };
            self.stylist.set_device(device, &guards)
        };

        // Only fully invalidate element styles when the media query results of
        // some origin actually changed. `force_stylesheet_origins_dirty` fully
        // invalidates styles even when `origins` is empty, which would force a
        // full restyle on every viewport resize.
        if !origins.is_empty() {
            self.stylist.force_stylesheet_origins_dirty(origins);
        }

        // Styles that resolve viewport units (vw/vh/etc) still need recomputing
        // when the viewport size changes, even if no media query results
        // changed. Invalidate just the elements whose styles use viewport
        // units (tracked via per-element computed value flags).
        // Inline media conditions use the same per-style dependency flag. Their
        // predicates can also change on a media type or input-device update.
        if (viewport_size_changed && styles_use_viewport_units) || style_uses_media_conditions {
            if self
                .stylist
                .get_custom_property_initial_values_flags()
                .intersects(ComputedValueFlags::USES_VIEWPORT_UNITS)
            {
                self.stylist.rebuild_initial_values_for_custom_properties();
            }
            if let Some(root) = self.try_root_element() {
                style::invalidation::viewport_units::invalidate(root);
            }
        }
    }

    pub fn stylist_device(&mut self) -> &Device {
        self.flush_pending_device_changes();
        self.stylist.device()
    }

    pub fn get_cursor(&self) -> Option<CursorIcon> {
        // Prefer the precise hit node: `cursor` and `user-select` may be set on
        // a pseudo-element or resolved on an anonymous box, and text hits carry
        // is_text via the hit node. Fall back to the canonical hover node if
        // the hit node has been removed (it is transient across resolves), and
        // to the default cursor if the pointer is over no box.
        let Some(node_id) = self
            .hover_hit_node_id
            .filter(|&id| self.nodes.contains_key(id))
            .or(self.get_hover_node_id())
        else {
            return Some(CursorIcon::Default);
        };
        let node = &self.nodes[node_id];

        if let Some(subdoc) = node.subdoc().map(|doc| doc.inner())
            && subdoc.get_hover_node_id().is_some()
        {
            return subdoc.get_cursor();
        }

        let style = node.primary_styles()?;
        let user_select = style.clone_user_select();
        let keyword = style.get_cursor().keyword;

        // Return cursor from style if it is non-auto
        if keyword != CursorKind::Auto {
            return stylo_to_cursor_icon(keyword);
        }

        // Return text cursor for text inputs
        if node
            .element_data()
            .is_some_and(|e| e.text_input_data().is_some())
        {
            return Some(CursorIcon::Text);
        }

        // Use "pointer" cursor if any ancestor is a link. Walk DOM parents: an atomic
        // inline's layout parent is its inline root, which skips the inline elements
        // (such as `<a>`) between them.
        let mut maybe_node = Some(node);
        while let Some(node) = maybe_node {
            if node.is_link() {
                return Some(CursorIcon::Pointer);
            }

            maybe_node = node.parent.map(|node_id| node.with(node_id));
        }

        // Return text cursor for text nodes
        if self.hover_node_is_text {
            return Some(match user_select {
                UserSelect::Text | UserSelect::All | UserSelect::Auto | UserSelect::Contain => {
                    CursorIcon::Text
                }
                UserSelect::None => CursorIcon::Default,
            });
        }

        // Else fallback to default cursor
        Some(CursorIcon::Default)
    }

    pub fn viewport_scroll(&self) -> crate::Point<f64> {
        self.nodes.viewport_scroll()
    }

    pub fn set_viewport_scroll(&mut self, scroll: crate::Point<f64>) {
        self.nodes.set_viewport_scroll(scroll);
    }

    /// Find the node targeted by a URL fragment (the `#...` part of a URL).
    ///
    /// Per the HTML spec, this is the element whose `id` matches the fragment, falling
    /// back to the first `<a>` element whose `name` attribute matches.
    pub fn get_fragment_target(&self, fragment: &str) -> Option<NodeId> {
        if let Some(node_id) = self.get_element_by_id(fragment) {
            return Some(node_id);
        }

        // Fall back to a named anchor: `<a name="...">`
        self.nodes.iter().find_map(|(id, node)| {
            let el = node.element_data()?;
            (el.name.local == local_name!("a") && el.attr(local_name!("name")) == Some(fragment))
                .then_some(id)
        })
    }

    /// `node_id`'s document-relative position and unrounded (sub-pixel) layout in physical
    /// coordinates, for CSSOM geometry APIs such as `getBoundingClientRect`.
    #[cfg(not(feature = "writing-mode"))]
    pub(crate) fn physical_unrounded_geometry(
        &self,
        node_id: NodeId,
    ) -> (crate::util::Point<f32>, taffy::Layout) {
        let mut pos = crate::util::Point { x: 0.0, y: 0.0 };
        let mut current = Some(node_id);
        while let Some(id) = current {
            let node = &self.nodes[id];
            pos.x += node.unrounded_layout().location.x - node.scroll_offset().x as f32;
            pos.y += node.unrounded_layout().location.y - node.scroll_offset().y as f32;
            current = node.containing_block();
            // pyro: out of a multi-column container's strip into its columns (`layout::multicol`).
            if let Some(containing) = current {
                pos = self.nodes[containing].strip_to_columns(pos);
            }
        }
        (pos, *self.nodes[node_id].unrounded_layout())
    }

    /// The text of the `<link>` sheet of `node_id`, when it has `@page` rules.
    pub fn linked_page_css(&self, node_id: NodeId) -> Option<&str> {
        self.linked_page_css.get(&node_id).map(String::as_str)
    }

    /// Computes the size and position of the `Node` relative to the viewport
    pub fn get_client_bounding_rect(&self, node_id: NodeId) -> Option<BoundingRect> {
        // Non-atomic inline elements have no layout box of their own: return
        // the union of their per-line-box fragment rects.
        if let Some(rects) = self.inline_fragment_rects(node_id) {
            return rects.reduce(BoundingRect::union);
        }

        self.get_node(node_id)?;
        let (pos, layout) = self.physical_unrounded_geometry(node_id);
        let size = layout.size;

        Some(BoundingRect {
            x: snap_to_layout_unit(pos.x as f64 - self.viewport_scroll().x),
            y: snap_to_layout_unit(pos.y as f64 - self.viewport_scroll().y),
            width: snap_to_layout_unit(size.width as f64),
            height: snap_to_layout_unit(size.height as f64),
        })
    }

    /// CSSOM View's `offsetLeft`/`offsetTop`/`offsetWidth`/`offsetHeight` as a single rect:
    /// the node's border box, positioned relative to the padding edge of its `offsetParent`
    /// (or to the initial containing block when the `offsetParent` is a non-positioned
    /// `body`, or there is none).
    ///
    /// Nodes with their own layout box use that box directly. Non-atomic inline elements
    /// have no layout box: they use the bounding box of their per-line-box fragments, and
    /// their `offsetParent` is resolved from the containing inline root.
    pub fn offset_rect(&self, node_id: NodeId) -> Option<BoundingRect> {
        let node = self.get_node(node_id)?;

        // Nodes with their own layout box: use it directly
        let Some(rects) = self.inline_fragment_rects(node_id) else {
            let pos = node.offset_top_left();
            let size = node.unrounded_layout().size;
            #[cfg(feature = "writing-mode")]
            let size = match node.containing_block() {
                Some(parent) if self.nodes[parent].layout_data().writing_mode.is_vertical() => {
                    size.transpose()
                }
                _ => size,
            };
            return Some(BoundingRect {
                x: pos.x as f64,
                y: pos.y as f64,
                width: size.width as f64,
                height: size.height as f64,
            });
        };

        // Union the per-line-box fragments into a single bounding box
        let bounds = rects.reduce(BoundingRect::union)?;

        // Fragment rects are viewport-relative; convert to document-relative
        let scroll = self.viewport_scroll();
        let mut x = bounds.x + scroll.x;
        let mut y = bounds.y + scroll.y;

        // Resolve the offsetParent from the inline root (the root itself if it is positioned)
        let inline_root = node.inline_root_ancestor()?;
        let offset_parent = if inline_root.is_offset_parent() {
            Some(inline_root)
        } else {
            inline_root.offset_parent()
        };
        // Make the position relative to the offsetParent's padding edge
        if let Some(parent) = offset_parent.filter(|parent| !parent.is_static_body()) {
            let (parent_pos, parent_layout) = self.physical_unrounded_geometry(parent.id);
            let border = parent_layout.border;
            x -= (parent_pos.x + border.left) as f64;
            y -= (parent_pos.y + border.top) as f64;
        }

        Some(BoundingRect {
            x,
            y,
            width: bounds.width,
            height: bounds.height,
        })
    }

    /// Computes the sizes and positions of the `Node`'s box fragments relative to the
    /// viewport (CSSOM `getClientRects()` semantics). Nodes with their own layout box
    /// return a single rect. Non-atomic inline elements (which are laid out as style
    /// spans within an inline root's text layout) return one rect per line box.
    pub fn node_client_rects(&self, node_id: NodeId) -> Vec<BoundingRect> {
        match self.inline_fragment_rects(node_id) {
            Some(rects) => rects.collect(),
            None => self.get_client_bounding_rect(node_id).into_iter().collect(),
        }
    }

    /// Computes per-line-box fragment rects for a non-atomic inline element by walking
    /// the containing inline root's text layout. Returns `None` for nodes that have
    /// their own layout box (which should use `get_client_bounding_rect` instead).
    pub fn inline_fragment_rects(
        &self,
        node_id: NodeId,
    ) -> Option<impl Iterator<Item = BoundingRect> + '_> {
        let node = self.get_node(node_id)?;

        // Only non-atomic inline elements lack their own layout box: they are
        // flattened into the containing inline root's text layout as style spans.
        let fragments = node.inline_fragment_boxes()?;
        let inline_root = node.inline_root_ancestor()?;

        // Fragment boxes are relative to the inline root's border box.
        let (root_pos, _) = self.physical_unrounded_geometry(inline_root.id);
        let origin_x = root_pos.x as f64 - self.viewport_scroll().x;
        let origin_y = root_pos.y as f64 - self.viewport_scroll().y;

        Some(fragments.map(move |rect| BoundingRect {
            x: snap_to_layout_unit(origin_x + rect.left as f64),
            y: snap_to_layout_unit(origin_y + rect.top as f64),
            width: snap_to_layout_unit((rect.right - rect.left) as f64),
            height: snap_to_layout_unit((rect.bottom - rect.top) as f64),
        }))
    }

    /// The first element in tree order with the given tag name. The root element and
    /// its children are checked first as a fast path before a full tree search, making
    /// this suitable for the `documentElement`/`head`/`body` document accessors.
    pub fn find_element_by_tag_name(&self, tag: &LocalName) -> Option<&Node> {
        let root = self.try_root_element()?;
        if root.data.is_element_with_tag_name(tag) {
            return Some(root);
        }
        root.children
            .iter()
            .copied()
            .find(|child_id| {
                self.get_node(*child_id)
                    .is_some_and(|child| child.data.is_element_with_tag_name(tag))
            })
            .or_else(|| {
                TreeTraverser::new(self)
                    .find(|node_id| self.nodes[*node_id].data.is_element_with_tag_name(tag))
            })
            .map(|node_id| &self.nodes[node_id])
    }

    /// The document's `body` element, as for `document.body`
    pub fn find_body_node(&self) -> Option<&Node> {
        self.find_element_by_tag_name(&local_name!("body"))
    }

    /// The document's `head` element, as for `document.head`
    pub fn find_head_node(&self) -> Option<&Node> {
        self.find_element_by_tag_name(&local_name!("head"))
    }

    /// The document's `title` element
    pub fn find_title_node(&self) -> Option<&Node> {
        self.find_element_by_tag_name(&local_name!("title"))
    }

    /// pyro: selects `start..end` (UTF-16 code units of its value) in the text input at
    /// `node_id`, as the page's DOM has it (the focused control's selection, painted).
    pub fn set_text_input_selection(&mut self, node_id: NodeId, start: u32, end: u32) {
        let Some(text) = self
            .nodes
            .get(node_id)
            .and_then(|node| node.element_data())
            .and_then(|el| el.text_input_data())
            .map(|input| input.editor.raw_text().to_owned())
        else {
            return;
        };
        let byte_of = |utf16: u32| {
            let mut units = 0u32;
            for (index, ch) in text.char_indices() {
                if units >= utf16 {
                    return index;
                }
                units += ch.len_utf16() as u32;
            }
            text.len()
        };
        let (start, end) = (byte_of(start), byte_of(end));
        self.with_text_input(node_id, |mut driver| driver.select_byte_range(start, end));
        // pyro: a control scrolls its text to keep the caret in view (as browsers do when the
        // selection moves).
        self.clamp_text_input_scroll(node_id);
    }

    pub fn with_text_input(
        &mut self,
        node_id: NodeId,
        cb: impl FnOnce(PlainEditorDriver<TextBrush>),
    ) {
        let Some(node) = self.nodes.get_mut(node_id) else {
            return;
        };

        if let Some(text_input) = node
            .element_data_mut()
            .and_then(|el| el.text_input_data_mut())
        {
            let mut font_ctx = self.font_ctx.lock().unwrap();
            let layout_ctx = &mut self.layout_ctx;
            let driver = text_input.editor.driver(&mut font_ctx, layout_ctx);
            cb(driver)
        }
    }

    /// Recompute the scroll offset of the text input at `node_id` (if any) so that its caret
    /// remains visible within the input's content box.
    pub(crate) fn clamp_text_input_scroll(&mut self, node_id: NodeId) {
        if self.nodes.get(node_id).is_none() {
            return;
        }
        // pyro: the content box's physical size, unrounded (text that fits it exactly is not
        // scrolled by a rounding remainder): `unrounded_layout` is in the axes of the box that
        // placed the control, which a vertical parent swaps.
        let (_, layout) = self.physical_unrounded_geometry(node_id);
        let mut content_box_width = layout.content_box_width();
        let mut content_box_height = layout.content_box_height();
        let node = &mut self.nodes[node_id];
        // pyro: the editor's layout is logical: in a vertical writing mode its inline size (the
        // single line's scroll axis) is the content box's height. (The control's own writing
        // mode, from its style: a horizontal control in a vertical line is not turned.)
        if node.primary_styles().is_some_and(|style| style.writing_mode.is_vertical()) {
            core::mem::swap(&mut content_box_width, &mut content_box_height);
        }

        if let Some(text_input) = node
            .element_data_mut()
            .and_then(|el| el.text_input_data_mut())
        {
            text_input.clamp_scroll_offset(content_box_width, content_box_height);
        }
    }

    pub(crate) fn compute_has_canvas(&self) -> bool {
        TreeTraverser::new(self).any(|node_id| {
            let node = &self.nodes[node_id];
            let Some(element) = node.element_data() else {
                return false;
            };
            if element.name.local == local_name!("canvas") && element.has_attr(local_name!("src")) {
                return true;
            }

            false
        })
    }

    // Text selection methods

    /// Find the text position (inline_root_id, byte_offset) at a given point.
    /// Uses hit() for proper coordinate transformation, then finds the inline root
    /// and byte offset.
    pub fn find_text_position(&mut self, x: f32, y: f32) -> Option<(NodeId, usize)> {
        let hit = self.hit(x, y)?;
        let hit_node = self.get_node(hit.node_id)?;
        let inline_root = hit_node.inline_root_ancestor()?;
        let byte_offset = inline_root.text_offset_at_point(hit.x, hit.y)?;
        Some((inline_root.id, byte_offset))
    }

    /// Set the text selection range (creates a new selection from anchor to focus)
    pub fn set_text_selection(
        &mut self,
        anchor_node: NodeId,
        anchor_offset: usize,
        focus_node: NodeId,
        focus_offset: usize,
    ) {
        self.text_selection =
            TextSelection::new(anchor_node, anchor_offset, focus_node, focus_offset);

        // For anonymous blocks, switch to storing parent+sibling_index (stable reference)
        if let (Some(parent), Some(idx)) = self.anonymous_block_location(anchor_node) {
            self.text_selection
                .anchor
                .set_anonymous(parent, idx, anchor_offset);
        }
        if let (Some(parent), Some(idx)) = self.anonymous_block_location(focus_node) {
            self.text_selection
                .focus
                .set_anonymous(parent, idx, focus_offset);
        }
    }

    /// Get the parent ID and sibling index for a node if it's an anonymous block.
    /// Returns (None, None) for non-anonymous blocks.
    fn anonymous_block_location(&self, node_id: NodeId) -> (Option<NodeId>, Option<usize>) {
        let Some(node) = self.get_node(node_id) else {
            return (None, None);
        };

        if !node.is_anonymous() {
            return (None, None);
        }

        let Some(parent_id) = node.parent else {
            return (None, None);
        };

        let Some(parent) = self.get_node(parent_id) else {
            return (Some(parent_id), None);
        };

        let layout_children = parent.layout_children.borrow();
        let Some(children) = layout_children.as_ref() else {
            return (Some(parent_id), None);
        };

        // Find the index of this anonymous block among siblings
        let mut anon_index = 0;
        for &child_id in children.iter() {
            if child_id == node_id {
                return (Some(parent_id), Some(anon_index));
            }
            if self.get_node(child_id).is_some_and(|n| n.is_anonymous()) {
                anon_index += 1;
            }
        }

        (Some(parent_id), None)
    }

    /// Clear the text selection
    pub fn clear_text_selection(&mut self) {
        self.text_selection.clear();
    }

    /// Update the selection focus point (used during mouse drag to extend selection).
    pub fn update_selection_focus(&mut self, focus_node: NodeId, focus_offset: usize) {
        // For anonymous blocks, store parent+sibling_index; otherwise store node directly
        if let (Some(parent), Some(idx)) = self.anonymous_block_location(focus_node) {
            self.text_selection
                .focus
                .set_anonymous(parent, idx, focus_offset);
        } else {
            self.text_selection.set_focus(focus_node, focus_offset);
        }
    }

    /// Extend text selection to the given point. Returns true if selection was updated.
    /// This is a convenience method that combines find_text_position and update_selection_focus.
    pub fn extend_text_selection_to_point(&mut self, x: f32, y: f32) -> bool {
        if !self.text_selection.anchor.is_some() {
            return false;
        }

        if let Some((node, offset)) = self.find_text_position(x, y) {
            self.update_selection_focus(node, offset);
            self.shell_provider.request_redraw();
            true
        } else {
            false
        }
    }

    /// Find the Nth anonymous block under a parent.
    fn find_anonymous_block_by_index(
        &self,
        parent_id: NodeId,
        target_index: usize,
    ) -> Option<NodeId> {
        let parent = self.get_node(parent_id)?;
        let layout_children = parent.layout_children.borrow();
        let children = layout_children.as_ref()?;

        children
            .iter()
            .filter(|&&child_id| self.get_node(child_id).is_some_and(|n| n.is_anonymous()))
            .nth(target_index)
            .copied()
    }

    /// Check if there is an active (non-empty) text selection
    pub fn has_text_selection(&self) -> bool {
        self.text_selection.is_active()
    }

    /// Get the selected text content, supporting selection across multiple inline roots.
    pub fn get_selected_text(&self) -> Option<String> {
        let ranges = self.get_text_selection_ranges();
        if ranges.is_empty() {
            return None;
        }

        let mut result = String::new();
        for (node_id, start, end) in &ranges {
            let node = self.get_node(*node_id)?;
            let element_data = node.element_data()?;
            let inline_layout = element_data.inline_layout_data.as_ref()?;

            if *end > inline_layout.text.len() {
                continue;
            }

            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(&inline_layout.text[*start..*end]);
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Get all selection ranges as Vec<(node_id, start_offset, end_offset)>.
    /// Returns empty vec if no selection.
    pub fn get_text_selection_ranges(&self) -> Vec<(NodeId, usize, usize)> {
        // pyro: the embedder's selection, when it set one.
        if let Some(selection) = self.dom_selection {
            return self.dom_selection_ranges(selection);
        }
        let lookup = |parent_id, idx| self.find_anonymous_block_by_index(parent_id, idx);

        let anchor_node = match self.text_selection.anchor.resolve_node_id(lookup) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let focus_node = match self.text_selection.focus.resolve_node_id(lookup) {
            Some(id) => id,
            None => return Vec::new(),
        };

        // Guard against stale selection endpoints: nodes may have been removed from
        // the document (e.g. by script) since the selection was made.
        let node_is_in_doc = |node_id: NodeId| {
            self.nodes
                .get(node_id)
                .is_some_and(|node| node.flags.is_in_document())
        };
        if !node_is_in_doc(anchor_node) || !node_is_in_doc(focus_node) {
            return Vec::new();
        }

        // Single node selection
        if anchor_node == focus_node {
            let start = self
                .text_selection
                .anchor
                .offset
                .min(self.text_selection.focus.offset);
            let end = self
                .text_selection
                .anchor
                .offset
                .max(self.text_selection.focus.offset);

            if start == end {
                return Vec::new();
            }
            return vec![(anchor_node, start, end)];
        }

        // Multi-node selection: collect all inline roots between anchor and focus
        let inline_roots = self.collect_inline_roots_in_range(anchor_node, focus_node);
        if inline_roots.is_empty() {
            return Vec::new();
        }

        // Determine document order using the collected inline_roots order
        // (inline_roots is already in document order from first to last)
        let first_in_roots = inline_roots[0];

        let (first_node, first_offset, last_node, last_offset) =
            if first_in_roots == anchor_node || (first_in_roots != focus_node) {
                // anchor is first (or neither endpoint is in roots, which shouldn't happen)
                (
                    anchor_node,
                    self.text_selection.anchor.offset,
                    focus_node,
                    self.text_selection.focus.offset,
                )
            } else {
                // focus is first
                (
                    focus_node,
                    self.text_selection.focus.offset,
                    anchor_node,
                    self.text_selection.anchor.offset,
                )
            };

        let mut ranges = Vec::with_capacity(inline_roots.len());

        for &node_id in &inline_roots {
            let Some(node) = self.get_node(node_id) else {
                continue;
            };
            let Some(element_data) = node.element_data() else {
                continue;
            };
            let Some(inline_layout) = element_data.inline_layout_data.as_ref() else {
                continue;
            };

            let text_len = inline_layout.text.len();

            if node_id == first_node && node_id == last_node {
                let start = first_offset.min(last_offset);
                let end = first_offset.max(last_offset);
                if start < end && end <= text_len {
                    ranges.push((node_id, start, end));
                }
            } else if node_id == first_node {
                if first_offset < text_len {
                    ranges.push((node_id, first_offset, text_len));
                }
            } else if node_id == last_node {
                if last_offset > 0 && last_offset <= text_len {
                    ranges.push((node_id, 0, last_offset));
                }
            } else if text_len > 0 {
                ranges.push((node_id, 0, text_len));
            }
        }

        ranges
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BoundingRect {
    /// The smallest rect containing both `self` and `other`.
    pub fn union(self, other: BoundingRect) -> BoundingRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        BoundingRect {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }
}

/// Snap a CSSOM geometry value to a 1/64px grid (the precision of Blink's `LayoutUnit`).
/// Layout values are accumulated in `f32`, so e.g. seven `55/7`-wide flex items would
/// otherwise end at `55.0000005` and appear to overflow their 55px container.
fn snap_to_layout_unit(value: f64) -> f64 {
    (value * 64.0).round() / 64.0
}

impl Drop for BaseDocument {
    fn drop(&mut self) {
        #[cfg(feature = "devtools")]
        if let Some(provider) = &self.devtools_provider {
            provider.document_dropped();
        }
    }
}

impl AsRef<BaseDocument> for BaseDocument {
    fn as_ref(&self) -> &BaseDocument {
        self
    }
}

impl AsMut<BaseDocument> for BaseDocument {
    fn as_mut(&mut self) -> &mut BaseDocument {
        self
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;
    use blitz_traits::shell::ColorScheme;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct CountingShellProvider {
        redraw_count: AtomicUsize,
    }
    impl ShellProvider for CountingShellProvider {
        fn request_redraw(&self) {
            self.redraw_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn zoom_requests_redraw() {
        let shell_provider = Arc::new(CountingShellProvider::default());
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            shell_provider: Some(shell_provider.clone() as _),
            ..Default::default()
        });
        doc.resolve(0.0);

        let base = shell_provider.redraw_count.load(Ordering::SeqCst);
        doc.zoom_by(0.5);
        assert!(shell_provider.redraw_count.load(Ordering::SeqCst) > base);

        let base = shell_provider.redraw_count.load(Ordering::SeqCst);
        doc.zoom_to(1.0);
        assert!(shell_provider.redraw_count.load(Ordering::SeqCst) > base);
    }
}

#[cfg(test)]
mod hover_state_tests {
    use super::*;
    use crate::{Attribute, qual_name};
    use blitz_traits::shell::ColorScheme;

    /// Build `<html><body style="margin:0"><div style="width:300px">some text
    /// <div style="height:50px"></div></div></body></html>` manually (the HTML
    /// parser lives in blitz-html, which would be a circular dev-dependency).
    /// The bare text next to a block sibling gets wrapped in an anonymous
    /// block, which becomes the inline root: text hits report the anonymous
    /// block as the hit node.
    fn make_doc() -> (BaseDocument, NodeId) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute {
            name: qual_name!("style"),
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![style("margin:0")]);
        let container = mutator.create_element(qual_name!("div"), vec![style("width:300px")]);
        let text = mutator.create_text_node("some text");
        let block = mutator.create_element(qual_name!("div"), vec![style("height:50px")]);
        mutator.append_children(container, &[text, block]);
        mutator.append_children(body, &[container]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        (doc, container)
    }

    /// Whether text laid out with a real (non-zero-metric) font. Without the
    /// `system-fonts` feature text measures 0x0 and text hits are impossible,
    /// making these tests vacuous.
    fn text_has_size(doc: &BaseDocument, container: NodeId) -> bool {
        doc.nodes[container].final_layout().size.height > 50.0
    }

    /// Regression test: hovering bare text wrapped in an anonymous block must
    /// report a text cursor. The hit node for such text is the anonymous
    /// inline root itself, while the *stored* hover target is canonicalized to
    /// the containing element — the cursor must be derived from the precise
    /// hit node, not the canonical target.
    #[test]
    fn hovering_text_in_anonymous_block_reports_text_cursor() {
        let (mut doc, container) = make_doc();
        if !text_has_size(&doc, container) {
            eprintln!("skipping: no usable font (text measures 0x0)");
            return;
        }

        doc.set_hover_to(5.0, 8.0);
        assert!(doc.hover_node_is_text, "expected a text hit");
        let hit_id = doc.hover_hit_node_id.expect("expected a hit node");
        assert!(
            doc.nodes[hit_id].is_anonymous(),
            "expected the hit node to be the anonymous inline root"
        );
        assert_eq!(
            doc.get_hover_node_id(),
            Some(container),
            "expected the stored hover target to be the containing element"
        );
        assert_eq!(doc.get_cursor(), Some(CursorIcon::Text));
    }

    /// Hovering the empty region of the anonymous block (right of the text) is
    /// not a text hit: default cursor, same canonical hover target.
    #[test]
    fn hovering_anonymous_block_whitespace_reports_default_cursor() {
        let (mut doc, container) = make_doc();
        if !text_has_size(&doc, container) {
            eprintln!("skipping: no usable font (text measures 0x0)");
            return;
        }

        doc.set_hover_to(250.0, 8.0);
        assert!(!doc.hover_node_is_text);
        assert_eq!(doc.get_hover_node_id(), Some(container));
        assert_eq!(doc.get_cursor(), Some(CursorIcon::Default));
    }

    /// Hovering below the content hits no box: default cursor (`None` would hide it).
    #[test]
    fn hovering_below_content_reports_default_cursor() {
        let (mut doc, _) = make_doc();

        doc.set_hover_to(5.0, 250.0);
        assert_eq!(doc.get_hover_node_id(), None);
        assert_eq!(doc.get_cursor(), Some(CursorIcon::Default));
    }
}

#[cfg(test)]
mod hover_invalidation_tests {
    use super::*;
    use crate::{Attribute, QualName, qual_name};
    use blitz_traits::shell::ColorScheme;

    /// Build `<html><body style="margin:0"><div style="width:300px;height:100px">
    /// <span>text</span></div></body></html>` with a `div:hover` rule.
    fn make_doc() -> (BaseDocument, NodeId, NodeId) {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let sheet = UserAgentStylesheet::parse(
            "div:hover { background-color: rgb(255, 0, 0); } div:hover span { color: rgb(0, 255, 0); }",
        );
        doc.add_user_agent_stylesheet(sheet);
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute {
            name: qual_name!("style"),
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![style("margin:0")]);
        let div =
            mutator.create_element(qual_name!("div"), vec![style("width:300px;height:100px")]);
        let span = mutator.create_element(qual_name!("span"), vec![]);
        let text = mutator.create_text_node("some text");
        mutator.append_children(span, &[text]);
        mutator.append_children(div, &[span]);
        mutator.append_children(body, &[div]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        (doc, div, span)
    }

    fn bg_color(doc: &BaseDocument, id: NodeId) -> String {
        format!(
            "{:?}",
            doc.nodes[id]
                .primary_styles()
                .unwrap()
                .get_background()
                .background_color
        )
    }

    fn text_color(doc: &BaseDocument, id: NodeId) -> String {
        format!(
            "{:?}",
            doc.nodes[id].primary_styles().unwrap().clone_color()
        )
    }

    #[test]
    fn hover_styles_apply_and_clear() {
        let (mut doc, div, span) = make_doc();
        let initial_bg = bg_color(&doc, div);
        let initial_color = text_color(&doc, span);

        // Hover the div
        doc.set_hover_to(10.0, 10.0);
        assert!(doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        let hovered_bg = bg_color(&doc, div);
        let hovered_color = text_color(&doc, span);
        assert_ne!(initial_bg, hovered_bg, "hover should change div background");
        assert_ne!(
            initial_color, hovered_color,
            "hover should change span color"
        );

        // Move the pointer off the div (below it, over the body)
        doc.set_hover_to(10.0, 200.0);
        assert!(!doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        assert_eq!(
            bg_color(&doc, div),
            initial_bg,
            "unhover should restore div background"
        );
        assert_eq!(
            text_color(&doc, span),
            initial_color,
            "unhover should restore span color"
        );
    }

    /// Mimics BBC's headline hover pattern: `a:link:hover .headline` with the
    /// headline several levels below the anchor.
    #[test]
    fn ancestor_hover_with_link_state_updates_descendant() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let sheet = UserAgentStylesheet::parse(
            ".promo:link:hover .headline, .promo:visited:hover .headline { color: rgb(184, 0, 0); text-decoration-line: underline; }",
        );
        doc.add_user_agent_stylesheet(sheet);
        let root_id = doc.root_node().id;
        let attr = |name: QualName, value: &str| Attribute {
            name,
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(
            qual_name!("body"),
            vec![attr(qual_name!("style"), "margin:0")],
        );
        let a = mutator.create_element(
            qual_name!("a"),
            vec![
                attr(qual_name!("href"), "https://example.com"),
                attr(qual_name!("class"), "promo"),
                attr(
                    qual_name!("style"),
                    "display:block;width:300px;height:100px",
                ),
            ],
        );
        let p = mutator.create_element(qual_name!("p"), vec![]);
        let span = mutator.create_element(
            qual_name!("span"),
            vec![attr(qual_name!("class"), "headline")],
        );
        let text = mutator.create_text_node("Headline text");
        mutator.append_children(span, &[text]);
        mutator.append_children(p, &[span]);
        mutator.append_children(a, &[p]);
        mutator.append_children(body, &[a]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        let initial_color = text_color(&doc, span);

        doc.set_hover_to(10.0, 10.0);
        assert!(doc.nodes[a].is_hovered());
        doc.resolve(0.0);
        let hovered_color = text_color(&doc, span);
        assert_ne!(
            initial_color, hovered_color,
            "hovering the anchor should change the headline color"
        );

        doc.set_hover_to(10.0, 200.0);
        assert!(!doc.nodes[a].is_hovered());
        doc.resolve(0.0);
        assert_eq!(
            text_color(&doc, span),
            initial_color,
            "unhovering the anchor should restore the headline color"
        );
    }

    /// Toggling a checkbox must invalidate `:checked`-dependent styles.
    #[test]
    fn checkbox_toggle_updates_checked_styles() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let sheet = UserAgentStylesheet::parse(
            "input:checked { color: rgb(184, 0, 0); } input:checked + label { color: rgb(0, 184, 0); }",
        );
        doc.add_user_agent_stylesheet(sheet);
        let root_id = doc.root_node().id;
        let attr = |name: QualName, value: &str| Attribute {
            name,
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(
            qual_name!("body"),
            vec![attr(qual_name!("style"), "margin:0")],
        );
        let input = mutator.create_element(
            qual_name!("input"),
            vec![attr(qual_name!("type"), "checkbox")],
        );
        let label = mutator.create_element(qual_name!("label"), vec![]);
        let text = mutator.create_text_node("label text");
        mutator.append_children(label, &[text]);
        mutator.append_children(body, &[input, label]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        let initial_input_color = text_color(&doc, input);
        let initial_label_color = text_color(&doc, label);

        // Toggle the checkbox on (as the click handler does)
        doc.snapshot_node_and(input, ElementState::CHECKED, |node| {
            if let Some(el) = node.element_data_mut() {
                BaseDocument::toggle_checkbox(el);
            }
            node.mark_ancestors_dirty();
        });
        doc.resolve(0.0);
        assert_ne!(
            text_color(&doc, input),
            initial_input_color,
            "checking should change the input color"
        );
        assert_ne!(
            text_color(&doc, label),
            initial_label_color,
            "checking should change the sibling label color"
        );

        // Toggle the checkbox back off
        doc.snapshot_node_and(input, ElementState::CHECKED, |node| {
            if let Some(el) = node.element_data_mut() {
                BaseDocument::toggle_checkbox(el);
            }
            node.mark_ancestors_dirty();
        });
        doc.resolve(0.0);
        assert_eq!(
            text_color(&doc, input),
            initial_input_color,
            "unchecking should restore the input color"
        );
        assert_eq!(
            text_color(&doc, label),
            initial_label_color,
            "unchecking should restore the sibling label color"
        );
    }

    /// A repaint-only style change to an existing `::before` pseudo-element
    /// (no box construction damage) must be flushed to the pseudo-element's
    /// anonymous node during the style traversal.
    #[test]
    fn hover_updates_existing_pseudo_element_style() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let sheet = UserAgentStylesheet::parse(
            "div::before { content: \"x\"; color: rgb(1, 2, 3); } \
             div:hover::before { color: rgb(0, 0, 255); }",
        );
        doc.add_user_agent_stylesheet(sheet);
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute {
            name: qual_name!("style"),
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![style("margin:0")]);
        let div = mutator.create_element(
            qual_name!("div"),
            vec![style("display:block;width:300px;height:100px")],
        );
        let text = mutator.create_text_node("some text");
        mutator.append_children(div, &[text]);
        mutator.append_children(body, &[div]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        let before = doc.nodes[div].before().expect("::before node should exist");
        let initial_color = text_color(&doc, before);

        doc.set_hover_to(10.0, 10.0);
        assert!(doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        assert_ne!(
            text_color(&doc, before),
            initial_color,
            "hover should change the ::before color"
        );

        doc.set_hover_to(10.0, 200.0);
        assert!(!doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        assert_eq!(
            text_color(&doc, before),
            initial_color,
            "unhover should restore the ::before color"
        );
    }

    /// Background-image layers must be flushed to the node's dedicated image
    /// storage when its style changes (queued during the style traversal),
    /// now that image flushing no longer runs on every node in the flush pass.
    #[test]
    fn hover_updates_background_image_layers() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let sheet = UserAgentStylesheet::parse(
            "div { background-image: url(\"https://example.com/a.png\"); } \
             div:hover { background-image: url(\"https://example.com/b.png\"); }",
        );
        doc.add_user_agent_stylesheet(sheet);
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute {
            name: qual_name!("style"),
            value: value.to_string(),
        };

        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![style("margin:0")]);
        let div = mutator.create_element(
            qual_name!("div"),
            vec![style("display:block;width:300px;height:100px")],
        );
        mutator.append_children(body, &[div]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        let background_image_url = |doc: &BaseDocument, id: NodeId| -> Option<String> {
            let elem = doc.nodes[id].data.downcast_element().unwrap();
            elem.background_images
                .first()
                .and_then(|img| img.as_ref())
                .map(|img| img.url.as_str().to_string())
        };

        doc.resolve(0.0);
        assert_eq!(
            background_image_url(&doc, div).as_deref(),
            Some("https://example.com/a.png"),
            "initial resolve should flush the background image"
        );

        doc.set_hover_to(10.0, 10.0);
        assert!(doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        assert_eq!(
            background_image_url(&doc, div).as_deref(),
            Some("https://example.com/b.png"),
            "hover should flush the changed background image"
        );

        doc.set_hover_to(10.0, 200.0);
        assert!(!doc.nodes[div].is_hovered());
        doc.resolve(0.0);
        assert_eq!(
            background_image_url(&doc, div).as_deref(),
            Some("https://example.com/a.png"),
            "unhover should flush the restored background image"
        );
    }
}

#[cfg(test)]
mod picture_display_tests {
    use super::*;
    use crate::qual_name;
    use blitz_traits::shell::ColorScheme;

    /// `<picture>` must generate a box (inline, as in browsers): the
    /// aspect-ratio wrapper pattern relies on it for its absolutely positioned
    /// child. `display: contents` would leave the `<img>` in flow instead.
    #[test]
    fn picture_is_not_display_contents() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        let picture = mutator.create_element(qual_name!("picture"), vec![]);
        mutator.append_children(body, &[picture]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);

        doc.resolve(0.0);
        let display = doc.nodes[picture]
            .primary_styles()
            .expect("picture should be styled")
            .get_box()
            .display;
        assert!(!display.is_contents());
    }
}

#[cfg(test)]
mod inline_with_only_block_children_tests {
    use super::*;
    use crate::qual_name;
    use blitz_traits::shell::ColorScheme;

    #[test]
    fn inline_with_only_block_children_is_detected() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(
            "html, body, div { display: block } b { display: contents }",
        ));
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        mutator.append_children(root_id, &[html]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        mutator.append_children(html, &[body]);

        let inline_with_block = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_block]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(inline_with_block, &[block_child]);

        let inline_with_whitespace_and_block = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_whitespace_and_block]);
        let whitespace = mutator.create_text_node("\n  ");
        mutator.append_children(inline_with_whitespace_and_block, &[whitespace]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(inline_with_whitespace_and_block, &[block_child]);

        let inline_with_text_and_block = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_text_and_block]);
        let text = mutator.create_text_node("text");
        mutator.append_children(inline_with_text_and_block, &[text]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(inline_with_text_and_block, &[block_child]);

        // The nested span is itself laid out as a block box, so the outer span has
        // block-level content only. Chrome paints nothing for the outer span either.
        let inline_with_nested_block = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_nested_block]);
        let nested_inline = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(inline_with_nested_block, &[nested_inline]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(nested_inline, &[block_child]);

        let inline_with_comment_and_block = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_comment_and_block]);
        let comment = mutator.create_comment_node("comment");
        mutator.append_children(inline_with_comment_and_block, &[comment]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(inline_with_comment_and_block, &[block_child]);

        let inline_with_contents_text_and_block =
            mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_contents_text_and_block]);
        let contents = mutator.create_element(qual_name!("b"), vec![]);
        mutator.append_children(inline_with_contents_text_and_block, &[contents]);
        let text = mutator.create_text_node("text");
        mutator.append_children(contents, &[text]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(inline_with_contents_text_and_block, &[block_child]);

        let inline_with_text = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[inline_with_text]);
        let text = mutator.create_text_node("text");
        mutator.append_children(inline_with_text, &[text]);

        let empty_inline = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[empty_inline]);

        let block_with_block = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(body, &[block_with_block]);
        let nested_block = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(block_with_block, &[nested_block]);
        drop(mutator);

        doc.resolve(0.0);
        assert!(doc.nodes[inline_with_block].is_inline_with_only_block_children());
        assert!(doc.nodes[inline_with_whitespace_and_block].is_inline_with_only_block_children());
        assert!(!doc.nodes[inline_with_text_and_block].is_inline_with_only_block_children());
        assert!(doc.nodes[inline_with_nested_block].is_inline_with_only_block_children());
        assert!(doc.nodes[inline_with_comment_and_block].is_inline_with_only_block_children());
        assert!(
            !doc.nodes[inline_with_contents_text_and_block].is_inline_with_only_block_children()
        );
        assert!(!doc.nodes[inline_with_text].is_inline_with_only_block_children());
        assert!(!doc.nodes[empty_inline].is_inline_with_only_block_children());
        assert!(!doc.nodes[block_with_block].is_inline_with_only_block_children());
    }

    #[test]
    fn inline_with_only_block_children_follows_child_edits() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(
            "html, body, div { display: block }",
        ));
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        mutator.append_children(root_id, &[html]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        mutator.append_children(html, &[body]);
        let span = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[span]);
        let text = mutator.create_text_node("text");
        mutator.append_children(span, &[text]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(span, &[block_child]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(!doc.nodes[span].is_inline_with_only_block_children());

        let mut mutator = doc.mutate();
        mutator.remove_and_drop_node(text);
        drop(mutator);
        doc.resolve(0.0);
        assert!(doc.nodes[span].is_inline_with_only_block_children());

        let mut mutator = doc.mutate();
        let inline_child = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(span, &[inline_child]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(!doc.nodes[span].is_inline_with_only_block_children());
    }

    #[test]
    fn inline_with_only_block_children_follows_text_edits() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(
            "html, body, div { display: block }",
        ));
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        mutator.append_children(root_id, &[html]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        mutator.append_children(html, &[body]);
        let span = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[span]);
        let whitespace = mutator.create_text_node("\n  ");
        mutator.append_children(span, &[whitespace]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(span, &[block_child]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(doc.nodes[span].is_inline_with_only_block_children());

        let mut mutator = doc.mutate();
        mutator.append_text_to_node(whitespace, "x").unwrap();
        drop(mutator);
        doc.resolve(0.0);
        assert!(!doc.nodes[span].is_inline_with_only_block_children());
    }

    #[test]
    fn inline_with_only_block_children_follows_contents_descendant_edits() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        doc.add_user_agent_stylesheet(crate::UserAgentStylesheet::parse(
            "html, body, div { display: block } b { display: contents }",
        ));
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        mutator.append_children(root_id, &[html]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        mutator.append_children(html, &[body]);
        let span = mutator.create_element(qual_name!("span"), vec![]);
        mutator.append_children(body, &[span]);
        let contents = mutator.create_element(qual_name!("b"), vec![]);
        mutator.append_children(span, &[contents]);
        let block_in_contents = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(contents, &[block_in_contents]);
        let block_child = mutator.create_element(qual_name!("div"), vec![]);
        mutator.append_children(span, &[block_child]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(doc.nodes[span].is_inline_with_only_block_children());

        let mut mutator = doc.mutate();
        let text = mutator.create_text_node("text");
        mutator.append_children(contents, &[text]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(!doc.nodes[span].is_inline_with_only_block_children());
    }
}

#[cfg(test)]
mod font_face_override_tests {
    use super::*;
    use crate::net::{FontFaceOverrides, Resource, ResourceLoadResponse};

    /// Regression-pin for the `@font-face` descriptor-honouring fix.
    ///
    /// The bug was that `Resource::Font` carried only the raw font bytes,
    /// so `load_resource` registered fonts with `info_override = None` and
    /// parley fell back to the TTF's internal `name` table. After the fix,
    /// `Resource::Font` carries `FontFaceOverrides` and `load_resource`
    /// builds a `FontInfoOverride` from them — meaning a CSS-declared
    /// `font-family` alias wins over the file's own metadata.
    ///
    /// We drive `load_resource` directly with a fabricated response rather
    /// than go through HTML parsing → `fetch_font_face`, because the
    /// downstream HTML parser lives in `blitz-html` (would be a circular
    /// crate dependency). The mapping from `@font-face` descriptors into
    /// `FontFaceOverrides` is covered by the unit tests in `net.rs`; this
    /// test pins the load-side of the pipeline.
    #[test]
    fn font_face_overrides_alias_family_name() {
        const ALIAS: &str = "AliasedFamily";

        let mut document = BaseDocument::new(DocumentConfig::default());

        // Sanity: the alias name is not registered before we feed the font.
        {
            let mut ctx = document.font_ctx.lock().unwrap();
            assert!(
                ctx.collection.family_id(ALIAS).is_none(),
                "alias must not exist before registration",
            );
        }

        // Drive `load_resource` with a `Resource::Font` whose overrides
        // assert the CSS-side family name. We use the bullet font as a
        // valid font payload — its internal `name` table is irrelevant to
        // the assertion; what matters is whether the override wins.
        let response = ResourceLoadResponse {
            request_id: 0,
            node_id: None,
            resolved_url: Some(String::from("test://aliased-family")),
            result: Ok(Resource::Font(
                blitz_traits::net::Bytes::from_static(crate::BULLET_FONT),
                FontFaceOverrides {
                    family_name: Some(String::from(ALIAS)),
                    weight: Some(800.0),
                    style: Some(parley::fontique::FontStyle::Italic),
                    ..Default::default()
                },
            )),
        };
        document.load_resource(response);

        // The override must have taken effect: parley's `Collection` now
        // resolves the CSS-declared alias to a registered family.
        let mut ctx = document.font_ctx.lock().unwrap();
        let family_id = ctx
            .collection
            .family_id(ALIAS)
            .expect("CSS-declared family name should be registered as a family alias");
        let resolved_name = ctx
            .collection
            .family_name(family_id)
            .expect("family id should resolve back to a name");
        assert_eq!(
            resolved_name, ALIAS,
            "registered family should report the CSS-declared name, \
             not the font file's internal `name` table entry",
        );
    }

    /// The faces of a family that match a style equally are yielded last rule first, however
    /// they arrive, each with its `unicode-range`; the first available font is the first whose
    /// range includes U+0020.
    #[test]
    fn font_face_unicode_ranges_and_rule_order_reach_the_query() {
        use parley::fontique::{Attributes, QueryFamily, QueryStatus, UnicodeRange};

        let latin = UnicodeRange::new([0x20..=0x7E]);
        let cyrillic = UnicodeRange::new([0x400..=0x4FF]);
        // The first rule is for latin, the second for cyrillic.
        let faces = [(1, &latin), (2, &cyrillic)];
        for arrival in [[0, 1], [1, 0]] {
            let mut document = BaseDocument::new(DocumentConfig::default());
            for (precedence, range) in arrival.map(|i| faces[i]) {
                document.load_resource(ResourceLoadResponse {
                    request_id: 0,
                    node_id: None,
                    resolved_url: Some(format!("test://ranged-{precedence}")),
                    result: Ok(Resource::Font(
                        blitz_traits::net::Bytes::from_static(crate::BULLET_FONT),
                        FontFaceOverrides {
                            family_name: Some(String::from("Ranged")),
                            unicode_range: Some(range.clone()),
                            precedence,
                            ..Default::default()
                        },
                    )),
                });
            }

            let mut ctx = document.font_ctx.lock().unwrap();
            let ctx = &mut *ctx;
            let mut query = ctx.collection.query(&mut ctx.source_cache);
            query.set_families([QueryFamily::Named("Ranged")]);
            query.set_attributes(Attributes::default());
            let mut ranges = Vec::new();
            query.matches_with(|font| {
                ranges.push(font.unicode_range.clone());
                QueryStatus::Continue
            });
            assert_eq!(ranges, [cyrillic.clone(), latin.clone()], "arrival {arrival:?}");
            let first = query.first_available_font().map(|font| font.unicode_range);
            assert_eq!(first.as_ref(), Some(&latin), "arrival {arrival:?}");
        }
    }
}

#[cfg(test)]
mod change_tracking_tests {
    use super::*;
    use crate::qual_name;
    use blitz_traits::shell::ColorScheme;

    #[test]
    fn mutations_and_focus_changes_are_tracked() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        let button = mutator.create_element(qual_name!("button"), vec![]);
        mutator.append_children(body, &[button]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root_id, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        assert!(doc.has_changes());

        doc.clear_changes();
        assert!(!doc.has_changes());

        doc.mutate()
            .set_attribute(button, qual_name!("aria-label"), "Submit");
        assert!(doc.has_changes());

        doc.clear_changes();
        assert!(doc.set_focus_to(button));
        assert!(doc.has_changes());

        doc.clear_changes();
        doc.clear_focus();
        assert!(doc.has_changes());
    }
}

/// pyro: an SVG image drawn as a document (see [`BaseDocument::svg_image_documents`]).
pub struct SvgImageDocument {
    storage: pyro_raster::Reservation,
    /// The image's source (to tell a new image from the one the document was made of).
    pub source: std::sync::Arc<crate::util::SvgSource<str>>,
    /// The size it is laid out at, in CSS px.
    pub size: (u32, u32),
    pub document: Box<dyn Document>,
}

impl BaseDocument {
    /// pyro: makes and lays out the documents of the SVG images with a `<foreignObject>` that
    /// elements of this document show ([`BaseDocument::svg_image_documents`]), at the size each is
    /// drawn at: its natural size, else the element's content box.
    #[cfg(feature = "svg")]
    pub(crate) fn update_svg_image_documents(&mut self) {
        if !self.has_svg_image_documents {
            return;
        }
        let mut wanted = Vec::new();
        for (id, node) in self.nodes.iter() {
            if !node.flags.is_in_document() {
                continue;
            }
            let Some(ImageData::Svg(svg)) = node.element_data().and_then(|el| el.image_data()) else { continue };
            let Some(source) = svg.foreign_source.clone() else { continue };
            let layout = node.final_layout();
            let content = (layout.content_box_width(), layout.content_box_height());
            let width = svg.intrinsic_width().unwrap_or(content.0);
            let height = svg.intrinsic_height().unwrap_or(content.1);
            if !(width >= 1.0 && height >= 1.0) || width > 4096.0 || height > 4096.0 {
                continue;
            }
            wanted.push((id, source, (width.ceil() as u32, height.ceil() as u32)));
        }
        self.svg_image_documents.retain(|id, _| wanted.iter().any(|(w, ..)| w == id));
        let scale = self.viewport.hidpi_scale;
        for (id, source, size) in wanted {
            let current = self
                .svg_image_documents
                .get(&id)
                .is_some_and(|doc| std::sync::Arc::ptr_eq(&doc.source, &source) && doc.size == size);
            let viewport = Viewport::new(
                (size.0 as f32 * scale).ceil() as u32,
                (size.1 as f32 * scale).ceil() as u32,
                scale,
                self.viewport.color_scheme,
            );
            if !current {
                self.svg_image_documents.remove(&id);
                if self.subdocument_depth >= crate::iframe::MAX_SUBDOCUMENT_DEPTH {
                    continue;
                }
                let mut minimum = std::mem::size_of::<BaseDocument>() + std::mem::size_of::<NodeTree>() + source.len();
                let Ok(xml) = usvg::roxmltree::Document::parse(&source) else { continue };
                for node in xml.descendants() {
                    minimum = minimum.saturating_add(std::mem::size_of::<(Node, u32)>());
                    if node.is_element() {
                        minimum = minimum.saturating_add(std::mem::size_of::<ElementData>()).saturating_add(node.attributes().len() * std::mem::size_of::<crate::node::Attribute>());
                    }
                }
                drop(xml);
                let Ok(storage) = self.raster_budget.reserve(minimum) else { continue };
                let config = crate::DocumentConfig {
                    viewport: Some(viewport.clone()),
                    base_url: None,
                    document_url: None,
                    font_ctx: Some(self.font_ctx.lock().unwrap().clone()),
                    media_type: Some(self.media_type.clone()),
                    media_environment: self.media_environment,
                    subdocument_depth: self.subdocument_depth + 1,
                    raster_budget: self.raster_budget.clone(),
                    xml_document: true,
                    // pyro: an SVG image's document is no render snapshot: nothing legitimate gives
                    // it engine attributes, so every one is rejected (a nonce no parsed attribute
                    // value can hold: `engine_marked`), and its frame elements load nothing.
                    frames: std::sync::Arc::new(crate::FrameTableData {
                        nonce: crate::REJECT_ALL_NONCE.to_owned(),
                        entries: Default::default(),
                    }),
                    ..Default::default()
                };
                let Some(document) = self.html_parser_provider.clone().parse_svg_document(&source, config) else {
                    continue;
                };
                self.svg_image_documents.insert(id, SvgImageDocument { source, size, document, storage });
            }
            let entry = self.svg_image_documents.get_mut(&id).unwrap();
            let mut doc = entry.document.inner_mut();
            if current {
                doc.set_viewport(viewport);
                doc.set_media_environment(self.media_environment);
            }
            doc.resolve(0.0);
            doc.ensure_paint_tree();
            let mut bytes = std::mem::size_of::<Self>() + std::mem::size_of::<NodeTree>() + doc.nodes.slot_bytes();
            for (_, node) in doc.nodes.iter() {
                bytes = bytes.saturating_add(node.children.capacity() * std::mem::size_of::<NodeId>());
                if let Some(element) = node.element_data() {
                    bytes = bytes.saturating_add(std::mem::size_of::<ElementData>() + element.attrs.capacity() * std::mem::size_of::<crate::node::Attribute>());
                    for attr in element.attrs() { bytes = bytes.saturating_add(attr.value.capacity()); }
                } else if let Some(text) = node.text_data() { bytes = bytes.saturating_add(text.content.capacity()); }
            }
            drop(doc);
            if entry.storage.grow_to(&self.raster_budget, bytes).is_err() { self.svg_image_documents.remove(&id); }
        }
    }
}

/// pyro: `url` without its fragment.
pub(crate) fn without_fragment(url: &str) -> &str {
    url.split_once('#').map_or(url, |(before, _)| before)
}

/// pyro: whether a sheet's text may hold `@scope` rules (at-rule names are ASCII case-insensitive).
pub(crate) fn sheet_has_scope_rules(css: &str) -> bool {
    css.as_bytes().windows(6).any(|w| w.eq_ignore_ascii_case(b"@scope"))
}

/// pyro: the viewport's classic scrollbars ([`BaseDocument::viewport_scrollbars`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportScrollbars {
    /// The gutter at each physical edge of the viewport (CSS px): the scrollport and the initial
    /// containing block are the viewport less these.
    pub gutters: taffy::Rect<f32>,
    /// Whether the horizontal (x) and vertical (y) scrollbars show (`overflow: scroll`, or `auto`
    /// with the document overflowing); a `scrollbar-gutter: stable` gutter alone shows none.
    pub shown: taffy::Point<bool>,
    /// The thickness a shown scrollbar has.
    pub thickness: taffy::Size<f32>,
}

impl ViewportScrollbars {
    /// The viewport less the gutters: the scrollport, in viewport coordinates (CSS px).
    pub fn scrollport(&self, width: f32, height: f32) -> kurbo::Rect {
        let g = self.gutters;
        kurbo::Rect::new(
            g.left as f64,
            g.top as f64,
            (width - g.right).max(g.left) as f64,
            (height - g.bottom).max(g.top) as f64,
        )
    }

    /// Whether any edge has a gutter.
    pub fn any(&self) -> bool {
        let g = self.gutters;
        g.left > 0.0 || g.right > 0.0 || g.top > 0.0 || g.bottom > 0.0
    }
}
