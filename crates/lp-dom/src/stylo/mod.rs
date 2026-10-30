//! The cascade: Stylo (Servo's style system) over lp-dom nodes.
//!
//! [`Cascade`] keeps a Stylo `Stylist` (the UA sheet plus the author sheets
//! the [`StyleEngine`](crate::style::StyleEngine) holds) and the style data
//! of every element it styled ([`node::Store`], a side table: lp-dom nodes
//! stay as they are). Styles are computed lazily: nothing runs until a style
//! is read, and a read restyles only what changed since the last one, judged
//! from lp-dom's change journal, so the DOM mutation path does no style work.
//!
//! Invalidation is conservative: a changed attribute, state or child list
//! restyles the subtree of the changed node's parent (which covers the
//! element itself, its descendants, and sibling combinators), plus the
//! enclosing forms and fieldsets (`:valid`/`:invalid`) and, when a radio
//! button may have changed groups, every radio button; focus and hover moves
//! restyle the old and new targets and their ancestors. Sheets with `:has()`,
//! journal gaps and unknown changes restyle everything. Restyling reports
//! only the elements whose computed values changed (Stylo's damage), so the
//! layout caches built on them stay precise either way.

mod hints;
mod node;
pub mod select;

use std::sync::LazyLock;

use euclid::{Scale, Size2D};
use style::context::{QuirksMode, RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext};
use style::context::{StyleContext, ThreadLocalStyleContext};
use style::device::servo::FontMetricsProvider;
use style::device::Device;
use style::dom::{TElement, TNode};
use style::font_metrics::FontMetrics;
use style::global_style_data::GLOBAL_STYLE_DATA;
use style::invalidation::element::restyle_hints::RestyleHint;
use style::media_queries::{MediaList, MediaType};
use style::properties::style_structs::Font;
use style::properties::ComputedValues;
use style::queries::values::PrefersColorScheme;
use style::selector_parser::{PseudoElement, SnapshotMap};
use style::servo::media_features::PointerCapabilities;
use style::servo_arc::Arc;
use style::shared_lock::{SharedRwLock, StylesheetGuards};
use style::stylesheets::{AllowImportRules, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
use style::stylist::{RuleInclusion, Stylist};
use style::thread_state::{self, ThreadState};
use style::traversal::{recalc_style_at, resolve_style, DomTraversal};
use style::traversal_flags::TraversalFlags;
use style::values::computed::font::{GenericFontFamily, QueryFontMetricsFlags};
use style::values::computed::{CSSPixelLength, Length};
use style::Atom;

use crate::dom::{Change, Dom};
use crate::media::{ColorScheme, Viewport};
use crate::node::NIL;
use markup5ever::local_name;

pub(crate) use node::{Cx, SNode, Store};

/// A property's computed value as `getComputedStyle` serializes it: a
/// longhand directly, a shorthand when all its longhands serialize alike, a
/// custom property as specified. `None` for unknown properties.
pub fn serialize(style: &ComputedValues, name: &str) -> Option<String> {
    use style::properties::{PropertyDeclarationId, PropertyId};
    let id = PropertyId::parse_enabled_for_all_content(name).ok()?;
    match id.as_shorthand() {
        Ok(shorthand) => {
            // The value all its longhands share...
            let values: Vec<(&str, String)> = shorthand
                .longhands()
                .map(|l| (l.name(), style.computed_value_to_string(PropertyDeclarationId::Longhand(l))))
                .collect();
            if values.iter().all(|(_, v)| *v == values[0].1) {
                return values.into_iter().next().map(|(_, v)| v);
            }
            // ...else the shorthand serialization of a block declaring the
            // longhands' resolved values (CSSOM's).
            let mut css = String::new();
            for (name, value) in &values {
                css.push_str(name);
                css.push(':');
                css.push_str(value);
                css.push(';');
            }
            let url = url_data();
            let block = style::properties::parse_style_attribute(
                &css,
                &url,
                None,
                QuirksMode::NoQuirks,
                style::stylesheets::CssRuleType::Style,
            );
            let mut out = String::new();
            block.shorthand_to_css(shorthand, &mut out).ok()?;
            (!out.is_empty()).then_some(out)
        }
        Err(declaration) => {
            // Stylo resolves every length unzoomed but a line-height length.
            if name.eq_ignore_ascii_case("line-height") {
                if let style::values::generics::font::GenericLineHeight::Length(l) = style.get_font().line_height {
                    let mut out = String::new();
                    let px = style.effective_zoom.unzoom(l.0.px());
                    style_traits::ToCss::to_css(&CSSPixelLength::new(px), &mut style_traits::CssWriter::new(&mut out))
                        .ok()?;
                    return Some(out);
                }
            }
            let value = style.computed_value_to_string(declaration);
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

/// The URL style values resolve against (computed styles keep relative
/// `url()`s as written; the loader resolves them).
pub(crate) fn url_data() -> UrlExtraData {
    static URL: LazyLock<UrlExtraData> =
        LazyLock::new(|| UrlExtraData(Arc::new(url::Url::parse("about:blank").expect("url"))));
    URL.clone()
}

/// The HTML UA stylesheet (Mozilla's, as blitz-dom ships it), parsed once per
/// process with a read-only lock so every document shares it and its
/// cascade data.
fn ua_sheet() -> DocumentStyleSheet {
    static SHEET: LazyLock<DocumentStyleSheet> = LazyLock::new(|| {
        let lock = SharedRwLock::read_only();
        DocumentStyleSheet(Arc::new(Stylesheet::from_str(
            include_str!("ua.css"),
            url_data(),
            Origin::UserAgent,
            Arc::new(lock.wrap(MediaList::empty())),
            lock,
            None,
            None,
            QuirksMode::NoQuirks,
            AllowImportRules::No,
        )))
    });
    SHEET.clone()
}

/// Font metrics for `ex`, `ch`, `cap` and `ic` units: lp-dom does not load
/// fonts, so these are Stylo's fallbacks (0.5em, 0.5em, ...), as before.
#[derive(Debug)]
struct NoFontMetrics;
impl FontMetricsProvider for NoFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        size: CSSPixelLength,
        _f: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics { ascent: size * 0.8, ..FontMetrics::default() }
    }
    fn base_size_for_generic(&self, generic: GenericFontFamily) -> Length {
        Length::new(if generic == GenericFontFamily::Monospace { 13.0 } else { 16.0 })
    }
}

fn device(viewport: &Viewport, quirks: QuirksMode) -> Device {
    let size = Size2D::new(viewport.width as f32, viewport.height as f32);
    let dpr = viewport.device_scale_factor as f32;
    Device::new(
        MediaType::screen(),
        quirks,
        size,
        size * Scale::new(dpr),
        Scale::new(dpr),
        Box::new(NoFontMetrics),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        match viewport.color_scheme {
            ColorScheme::Dark => PrefersColorScheme::Dark,
            _ => PrefersColorScheme::Light,
        },
        PointerCapabilities::default(),
        PointerCapabilities::default(),
    )
}

struct NoPainters;
impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

/// Stylo's preorder traversal: restyle each element it reaches, noting
/// the ones whose computed values changed.
struct Recalc<'a> {
    context: SharedStyleContext<'a>,
    changed: std::sync::Mutex<Vec<u32>>,
    visited: std::sync::atomic::AtomicUsize,
}

impl<'dom> DomTraversal<SNode<'dom>> for Recalc<'_> {
    fn process_preorder<F: FnMut(SNode<'dom>)>(
        &self,
        context: &mut StyleContext<SNode<'dom>>,
        node: SNode<'dom>,
        note_child: F,
    ) {
        if let Some(el) = node.as_element() {
            self.visited.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // SAFETY: the sequential traversal has exclusive access.
            let mut data = unsafe { el.ensure_data() };
            data.damage = Default::default();
            recalc_style_at(self, context, el, &mut data, note_child);
            // Stylo's damage: what changed between the old and new computed
            // values (its own style or an eager pseudo's), empty when equal.
            if !std::mem::take(&mut data.damage).is_empty() {
                self.changed.lock().unwrap().push(el.idx as u32);
            }
            drop(data);
            if let Some(slot) = el.slot() {
                slot.state.set(el.state());
            }
            // SAFETY: as above.
            unsafe { el.unset_dirty_descendants() };
        }
    }
    fn needs_postorder_traversal() -> bool {
        false
    }
    fn process_postorder(&self, _context: &mut StyleContext<SNode<'dom>>, _node: SNode<'dom>) {
        unreachable!("no postorder traversal")
    }
    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.context
    }
}

/// One author sheet: its text's key and media, and the Stylo sheet parsed
/// from it.
struct AuthorSheet {
    key: SheetKey,
    has_relative: bool,
    sheet: DocumentStyleSheet,
    /// Attribute local names its selectors test.
    attrs: std::sync::Arc<[markup5ever::LocalName]>,
}

/// A sheet by content: text hash and length, `media`, quirks.
#[derive(Clone, PartialEq, Eq, Hash)]
struct SheetKey(u64, usize, Option<String>, bool);

impl SheetKey {
    fn new(text: &str, media: Option<String>, quirks: QuirksMode) -> Self {
        use std::hash::BuildHasher;
        SheetKey(rustc_hash::FxBuildHasher.hash_one(text), text.len(), media, quirks == QuirksMode::Quirks)
    }
}

thread_local! {
    /// The lock of every cascade on this thread, so parsed sheets can be
    /// shared between documents.
    static LOCK: SharedRwLock = SharedRwLock::new();
    /// Parsed author sheets by content, shared by the documents of the
    /// thread (a test suite loads the same bundle on every page).
    static PARSED: std::cell::RefCell<rustc_hash::FxHashMap<SheetKey, ParsedSheet>> = Default::default();
}

/// A parsed sheet and the attribute local names its selectors test.
type ParsedSheet = (DocumentStyleSheet, std::sync::Arc<[markup5ever::LocalName]>);

/// The attribute local names the selectors of `sheet` test (attribute
/// selectors, at any depth: `:is()`, `:has()`, nested and conditional
/// rules). Attribute writes outside these need no invalidation snapshot.
fn attribute_names(sheet: &Stylesheet, device: &Device, guard: &style::shared_lock::SharedRwLockReadGuard) -> std::sync::Arc<[markup5ever::LocalName]> {
    use selectors::visitor::SelectorVisitor;
    use style::stylesheets::{AllRules, CssRule, RulesIterator, StylesheetInDocument};
    struct Names(rustc_hash::FxHashSet<markup5ever::LocalName>);
    impl SelectorVisitor for Names {
        type Impl = style::selector_parser::SelectorImpl;
        fn visit_attribute_selector(
            &mut self,
            _ns: &selectors::attr::NamespaceConstraint<&style::Namespace>,
            name: &style::LocalName,
            lower: &style::LocalName,
        ) -> bool {
            self.0.insert(name.0.clone());
            self.0.insert(lower.0.clone());
            true
        }
    }
    let mut names = Names(Default::default());
    let contents = sheet.contents(guard);
    let rules = contents.rules(guard);
    let custom_media: Box<style::stylesheets::CustomMediaMap> = Box::default();
    for rule in RulesIterator::<AllRules, _>::new(device, contents.quirks_mode, custom_media, guard, rules.iter()) {
        if let CssRule::Style(style_rule) = rule {
            for selector in style_rule.read_with(guard).selectors.slice() {
                selector.visit(&mut names);
            }
        }
    }
    names.0.into_iter().collect()
}

/// Sheets kept for reuse at most (cleared past it).
const PARSED_CAP: usize = 64;

/// What restyling changed.
pub(crate) enum Restyled {
    All,
    Some(Vec<u32>),
}

pub struct Cascade {
    lock: SharedRwLock,
    stylist: Stylist,
    store: Store,
    snapshots: SnapshotMap,
    sheets: Vec<AuthorSheet>,
    /// The sheets use `:has()`: any change restyles everything.
    has_relative: bool,
    quirks: QuirksMode,
    viewport: Viewport,
    /// What the stored styles were computed for: (DOM uid, DOM version,
    /// focused, hovered). `None`: restyle everything.
    styled: Option<(u64, u64, u32, u32)>,
    /// Elements whose computed style changed since `take_changes`.
    changes: Restyled,
    /// Open popovers (element indices).
    popovers: rustc_hash::FxHashSet<u32>,
    /// Whether the DOM knows the attribute names the sheets test.
    names_published: bool,
    /// Elements whose values the last flush changed, and elements it
    /// visited (diagnostics, tests).
    pub(crate) last_restyled: usize,
    pub(crate) last_visited: usize,
}

impl Cascade {
    pub fn new(viewport: &Viewport, quirks: crate::node::QuirksMode) -> Cascade {
        // Process-wide Stylo setup (idempotent).
        static PREFS: std::sync::Once = std::sync::Once::new();
        PREFS.call_once(|| {
            style_config::set_pref!("layout.threads", -1);
            style_config::set_pref!("layout.unimplemented", true);
            style_config::set_pref!("layout.columns.enabled", true);
            style_config::set_pref!("layout.css.tree-counting-functions.enabled", true);
            // Features Chrome ships; off by default in Stylo's servo mode
            // (a disabled property or selector drops the declaration or rule).
            style_config::set_pref!("layout.grid.enabled", true);
            style_config::set_pref!("layout.container-queries.enabled", true);
            style_config::set_pref!("layout.variable_fonts.enabled", true);
            style_config::set_pref!("layout.writing-mode.enabled", true);
            style_config::set_pref!("layout.css.has-selector.enabled", true);
            style_config::set_pref!("layout.css.nth-child-of.enabled", true);
            style_config::set_pref!("layout.css.starting-style-at-rules.enabled", true);
            style_config::set_pref!("layout.css.at-scope.enabled", true);
            style_config::set_pref!("layout.css.style-queries.enabled", true);
            style_config::set_pref!("layout.css.line-clamp.enabled", true);
            style_config::set_pref!("layout.css.fit-content-function.enabled", true);
            style_config::set_pref!("layout.css.supports.at-rule.enabled", true);
        });
        let quirks = match quirks {
            crate::node::QuirksMode::Quirks => QuirksMode::Quirks,
            crate::node::QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
            crate::node::QuirksMode::NoQuirks => QuirksMode::NoQuirks,
        };
        let lock = LOCK.with(|l| l.clone());
        let mut stylist = Stylist::new(device(viewport, quirks), quirks);
        stylist.append_stylesheet(ua_sheet(), &lock.read());
        Cascade {
            lock,
            stylist,
            store: Store::default(),
            snapshots: SnapshotMap::new(),
            sheets: Vec::new(),
            has_relative: false,
            quirks,
            viewport: viewport.clone(),
            styled: None,
            changes: Restyled::All,
            popovers: Default::default(),
            names_published: false,
            last_restyled: 0,
            last_visited: 0,
        }
    }

    /// Makes `sheets` (text and `media` attribute, in cascade order) the
    /// author sheets; unchanged sheets keep their parse.
    pub fn set_sheets(&mut self, sheets: Vec<(String, Option<String>)>) {
        let sheets: Vec<(SheetKey, String)> =
            sheets.into_iter().map(|(t, m)| (SheetKey::new(&t, m, self.quirks), t)).collect();
        let same = sheets.len() == self.sheets.len() && sheets.iter().zip(&self.sheets).all(|((k, _), s)| *k == s.key);
        if same {
            return;
        }
        let guard = self.lock.read();
        let mut old = std::mem::take(&mut self.sheets);
        for s in &old {
            self.stylist.remove_stylesheet(s.sheet.clone(), &guard);
        }
        drop(guard);
        for (key, text) in sheets {
            let reuse = old.iter().position(|s| s.key == key);
            let sheet = match reuse {
                Some(i) => old.swap_remove(i),
                None => {
                    // The same text twice in a document: a sheet of its own
                    // (a stylist holds a sheet once).
                    let twice = self.sheets.iter().any(|s| s.key == key);
                    let shared = (!twice).then(|| PARSED.with(|p| p.borrow().get(&key).cloned())).flatten();
                    let (sheet, attrs) = shared.unwrap_or_else(|| {
                        let sheet = self.parse(&text, key.2.as_deref());
                        let attrs = attribute_names(&sheet.0, self.stylist.device(), &self.lock.read());
                        if !twice {
                            PARSED.with(|p| {
                                let mut p = p.borrow_mut();
                                if p.len() >= PARSED_CAP {
                                    p.clear();
                                }
                                p.insert(key.clone(), (sheet.clone(), attrs.clone()));
                            });
                        }
                        (sheet, attrs)
                    });
                    AuthorSheet { has_relative: text.contains(":has("), key, sheet, attrs }
                }
            };
            self.stylist.append_stylesheet(sheet.sheet.clone(), &self.lock.read());
            self.sheets.push(sheet);
        }
        self.has_relative = self.sheets.iter().any(|s| s.has_relative);
        self.names_published = false;
        self.styled = None;
    }

    fn parse(&self, text: &str, media: Option<&str>) -> DocumentStyleSheet {
        let media = match media {
            Some(m) if !m.trim().is_empty() => {
                let mut parser = cssparser::Parser::new(m);
                let url = url_data();
                let mut context = style::parser::ParserContext::new(
                    Origin::Author,
                    &url,
                    Some(style::stylesheets::CssRuleType::Media),
                    style_traits::ParsingMode::DEFAULT,
                    self.quirks,
                    Default::default(),
                    None,
                    None,
                    Default::default(),
                );
                MediaList::parse(&mut context, &mut parser)
            }
            _ => MediaList::empty(),
        };
        DocumentStyleSheet(Arc::new(Stylesheet::from_str(
            text,
            url_data(),
            Origin::Author,
            Arc::new(self.lock.wrap(media)),
            self.lock.clone(),
            None,
            None,
            self.quirks,
            AllowImportRules::No,
        )))
    }

    /// Applies a new viewport (size, scale, color scheme): media queries and
    /// viewport units are re-evaluated on the next flush.
    pub fn set_viewport(&mut self, viewport: &Viewport) {
        if *viewport == self.viewport {
            return;
        }
        self.viewport = viewport.clone();
        let guard = self.lock.read();
        let guards = StylesheetGuards { author: &guard, ua_or_user: &guard };
        // The root's font size and line height carry over (`rem`, `rlh`):
        // Stylo records them only when the root's own values change.
        let (font_size, line_height) =
            (self.stylist.device().root_font_size(), self.stylist.device().root_line_height());
        self.stylist.set_device(device(viewport, self.quirks), &guards);
        self.stylist.device().set_root_font_size(font_size.px());
        self.stylist.device().set_root_line_height(line_height.px());
        self.stylist.force_stylesheet_origins_dirty(style::stylesheets::OriginSet::all());
        self.styled = None;
    }

    /// Sets the open popovers; a change restyles everything.
    pub(crate) fn set_popovers(&mut self, popovers: rustc_hash::FxHashSet<u32>) {
        if popovers != self.popovers {
            self.popovers = popovers;
            self.styled = None;
        }
    }

    /// Restyles document `doc` (index) where it changed since the last flush.
    pub(crate) fn flush(&mut self, dom: &Dom, doc: u32) {
        let root = match dom.child_idx_iter(doc).find(|&c| dom.n(c).is_element()) {
            Some(r) => r,
            None => return,
        };
        let focused = dom.focused_idx(doc);
        let hovered = dom.hovered_idx(doc);
        let stamp = (dom.uid(), dom.version(), focused, hovered);
        if self.styled == Some(stamp) {
            return;
        }
        self.store.reserve(dom.slot_len());
        // Attributes before their change (and from here on, kept for the next flush).
        let attrs = dom.take_style_snapshots(doc);
        if !self.names_published {
            static UA_NAMES: LazyLock<std::sync::Arc<[markup5ever::LocalName]>> = LazyLock::new(|| {
                let sheet = ua_sheet();
                let lock = SharedRwLock::read_only();
                let guard = lock.read();
                let device = device(&Viewport::default(), QuirksMode::NoQuirks);
                attribute_names(&sheet.0, &device, &guard)
            });
            dom.add_style_attr_names(UA_NAMES.iter().cloned());
            dom.add_style_attr_names(self.sheets.iter().flat_map(|s| s.attrs.iter().cloned()));
            self.names_published = true;
        }
        let previous = self.styled.filter(|s| s.0 == stamp.0);
        let full = match previous {
            None => true,
            Some((_, version, old_focus, old_hover)) => {
                !self.invalidate(dom, doc, version, &attrs, (old_focus, old_hover), (focused, hovered))
            }
        };
        if full {
            self.store.sweep(dom);
        }
        let cx = Cx::new(dom, &self.store, &self.lock, self.quirks, &self.popovers);
        let _entered = cx.enter();
        let root = SNode::new(&cx, root);
        if full {
            if let Some(mut data) = root.mutate_data() {
                data.hint.insert(RestyleHint::restyle_subtree());
            }
        }
        thread_state::enter(ThreadState::LAYOUT);
        let guard = self.lock.read();
        let guards = StylesheetGuards { author: &guard, ua_or_user: &guard };
        self.stylist.flush(&guards).process_style(root, Some(&self.snapshots));
        let context = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &self.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: false,
            animations: Default::default(),
            current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &NoPainters,
        };
        let token = Recalc::pre_traverse(root, &context);
        let mut changed = Vec::new();
        self.last_visited = 0;
        if token.should_traverse() {
            let traversal = Recalc { context, changed: Default::default(), visited: Default::default() };
            style::driver::traverse_dom(&traversal, token, None);
            changed = traversal.changed.into_inner().unwrap();
            self.last_visited = traversal.visited.into_inner();
        }
        thread_state::exit(ThreadState::LAYOUT);
        for (node, _) in self.snapshots.drain() {
            let el = SNode::new(&cx, node.0 as u32);
            if let Some(slot) = el.slot() {
                slot.has_snapshot.set(false);
                slot.handled_snapshot.set(false);
                // The next snapshot's old state, whether or not it restyled.
                slot.state.set(el.state());
            }
        }
        self.last_restyled = changed.len();
        // Full restyles compare old and new values too: only the first
        // styling (nothing to compare with) reports everything.
        if let Restyled::Some(earlier) = &mut self.changes {
            earlier.extend(changed);
        }
        self.styled = Some(stamp);
    }

    /// What the flushes since the last call restyled (for caches derived
    /// from computed styles).
    pub(crate) fn take_changes(&mut self) -> Restyled {
        std::mem::replace(&mut self.changes, Restyled::Some(Vec::new()))
    }

    /// The style of `idx`'s `::before` (`after` false) or `::after`, as the
    /// traversal computed it (Stylo computes these eagerly when a rule
    /// targets them).
    pub(crate) fn pseudo(&self, dom: &Dom, idx: u32, after: bool) -> Option<Arc<ComputedValues>> {
        let cx = Cx::new(dom, &self.store, &self.lock, self.quirks, &self.popovers);
        let _entered = cx.enter();
        let el = SNode::new(&cx, idx);
        let data = el.borrow_data()?;
        let pseudo = if after { PseudoElement::After } else { PseudoElement::Before };
        data.styles.pseudos.get(&pseudo).cloned()
    }

    /// Marks what changed since `version` for restyling. False when the
    /// journal cannot say (restyle everything).
    ///
    /// Elements whose attributes or state changed get Stylo invalidation
    /// snapshots (their attributes before the change, from `attrs`, and the
    /// state they were styled with), from which Stylo restyles exactly what
    /// the selectors that depend on them reach. Insertions and removals
    /// restyle the parent's children when its selector flags say siblings
    /// may match differently (`:nth-child`, `+`, `~`, `:first-child`,
    /// `:empty`), and style new children otherwise. With `:has()`, the
    /// anchors a change can reach (Stylo's relative-selector flags) restyle.
    fn invalidate(
        &mut self,
        dom: &Dom,
        doc: u32,
        version: u64,
        attrs: &rustc_hash::FxHashMap<u32, crate::dom::AttrChanges>,
        old: (u32, u32),
        new: (u32, u32),
    ) -> bool {
        use selectors::matching::ElementSelectorFlags as F;
        use style::attr::{AttrIdentifier, AttrValue};
        use style::dom::OpaqueNode;
        use style::selector_parser::ServoElementSnapshot;
        let Some(changes) = dom.changes_since(version) else { return false };
        let live = |n: u32| n != NIL && dom.slot_live(n) && dom.root_idx(n) == doc;
        // Elements whose own attributes or state may have changed.
        let mut changed: Vec<u32> = Vec::new();
        // Elements whose subtree restyles.
        let mut subtrees: Vec<u32> = Vec::new();
        // Parents with new children to style.
        let mut parents: Vec<u32> = Vec::new();
        // Where a change happened, for `:has()` anchors (with whether the
        // node itself may be an anchor of the change: insertions, removals).
        let mut relative: Vec<(u32, bool)> = Vec::new();
        // A radio's state depends on its group (`:checked` siblings make it
        // `:valid`); a form's, fieldset's and select's validity on their
        // controls and options.
        let mut radios = false;
        let is_input = |n: u32| {
            dom.slot_live(n) && dom.n(n).element().is_some_and(|e| e.is_html() && e.name.local == local_name!("input"))
        };
        let is_radio = |n: u32| is_input(n) && dom.input_type_idx(n) == crate::forms::InputType::Radio;
        // The form and fieldset elements at and above `up`.
        let control = |mut up: u32, changed: &mut Vec<u32>| {
            while up != NIL && dom.slot_live(up) {
                if dom
                    .n(up)
                    .element()
                    .is_some_and(|e| e.is_html() && matches!(&*e.name.local, "form" | "fieldset" | "select"))
                {
                    changed.push(up);
                }
                up = dom.n(up).parent;
            }
        };
        let cx = Cx::new(dom, &self.store, &self.lock, self.quirks, &self.popovers);
        let _entered = cx.enter();
        let flags = |n: u32| if dom.n(n).is_element() { SNode::new(&cx, n).selector_flags() } else { F::empty() };
        let structural = F::HAS_SLOW_SELECTOR
            | F::HAS_SLOW_SELECTOR_LATER_SIBLINGS
            | F::HAS_EDGE_CHILD_SELECTOR
            | F::HAS_EMPTY_SELECTOR
            | F::MAY_HAVE_TREE_COUNTING_FUNCTION;
        let mut moved: Vec<u32> = Vec::new();
        let mut own_rules: Vec<u32> = Vec::new();
        for (i, change) in changes.enumerate() {
            if i > 10_000 {
                return false;
            }
            let c = change;
            match c {
                Change::Attr(n, _, _) | Change::State(n) => {
                    if !live(n) {
                        continue;
                    }
                    changed.push(n);
                    relative.push((n, false));
                    // An attribute no selector tests (no snapshot): only the
                    // element's own declarations can change (`style`,
                    // presentational hints); data-/aria- ones, nothing.
                    if matches!(c, Change::Attr(_, h, false) if h != 0) && !attrs.contains_key(&n) {
                        own_rules.push(n);
                    }
                    // Any attribute of an input may move it into or out of a group.
                    radios |= matches!(c, Change::Attr(..)) && is_input(n) || is_radio(n);
                    control(dom.n(n).parent, &mut changed);
                    // `:nth-child(An+B of S)`: a sibling entering or leaving S
                    // moves the later ones' (and earlier ones', for nth-last).
                    let p = dom.n(n).parent;
                    if live(p) && flags(p).contains(F::HAS_SLOW_SELECTOR_NTH_OF) {
                        subtrees.push(p);
                    }
                }
                Change::Text(n) => {
                    let p = dom.n(n).parent;
                    if live(p) && flags(p).contains(F::HAS_EMPTY_SELECTOR) {
                        subtrees.push(p);
                    }
                }
                Change::Inserted(p, c) | Change::Removed(p, c, _) => {
                    if !live(p) {
                        continue;
                    }
                    if matches!(change, Change::Inserted(..)) && live(c) && dom.n(c).is_element() {
                        // A moved element keeps its old style data: restyle it.
                        moved.push(c);
                    }
                    if dom.slot_live(c) && !radios {
                        let mut cur = c;
                        while cur != NIL && !radios {
                            radios = is_radio(cur);
                            cur = dom.following_in(cur, c);
                        }
                    }
                    control(p, &mut changed);
                    relative.push((p, true));
                    // The focused or hovered element moved with `c`: where it
                    // was, `:focus-within` / `:hover` ended.
                    if matches!(change, Change::Removed(..)) && live(c) {
                        let inside = |t: u32| {
                            let mut a = t;
                            while live(a) {
                                if a == c {
                                    return true;
                                }
                                a = dom.n(a).parent;
                            }
                            false
                        };
                        if [old.0, new.0, old.1, new.1].into_iter().any(|t| t != NIL && inside(t)) {
                            let mut a = p;
                            while live(a) && dom.n(a).is_element() {
                                changed.push(a);
                                a = dom.n(a).parent;
                            }
                        }
                    }
                    if !dom.n(p).is_element() {
                        // The document element itself was inserted or removed.
                        return false;
                    }
                    if flags(p).intersects(structural) {
                        subtrees.push(p);
                    } else {
                        parents.push(p);
                    }
                }
                Change::Pseudo => {}
                Change::Unknown(_) => return false,
            }
        }
        if radios {
            let mut cur = doc;
            while cur != NIL {
                if is_radio(cur) {
                    changed.push(cur);
                }
                cur = dom.following_in(cur, doc);
            }
        }
        // Focus and hover move: the old and new targets and their ancestors
        // (`:focus-within`, `:hover` on ancestors) change state.
        for (a, b) in [(old.0, new.0), (old.1, new.1)] {
            if a != b {
                for t in [a, b] {
                    let mut n = t;
                    while live(n) && dom.n(n).is_element() {
                        changed.push(n);
                        relative.push((n, false));
                        n = dom.n(n).parent;
                    }
                }
            }
        }
        changed.sort_unstable();
        changed.dedup();
        relative.extend(changed.iter().map(|&n| (n, false)));

        // `:has()`: the anchors a change at `n` can reach, found from the
        // flags matching left on the elements a relative search visited.
        let mut selves: Vec<u32> = Vec::new();
        if self.has_relative {
            relative.sort_unstable();
            relative.dedup();
            let reach = |a: u32, subtrees: &mut Vec<u32>, selves: &mut Vec<u32>| {
                let f = flags(a);
                if f.contains(F::ANCHORS_RELATIVE_SELECTOR_NON_SUBJECT) {
                    // Elements below it or after it may match through it.
                    let p = dom.n(a).parent;
                    subtrees.push(if live(p) && dom.n(p).is_element() { p } else { a });
                } else if f.contains(F::ANCHORS_RELATIVE_SELECTOR) {
                    selves.push(a);
                }
            };
            for (n, inclusive) in relative {
                // A child inserted or removed: the parent and its children
                // (earlier siblings of the child) may anchor.
                if inclusive {
                    for c in dom.child_idx_iter(n).filter(|&c| dom.n(c).is_element()) {
                        reach(c, &mut subtrees, &mut selves);
                    }
                }
                let mut at = n;
                let mut first = true;
                while live(at) && dom.n(at).is_element() {
                    if inclusive || !first {
                        reach(at, &mut subtrees, &mut selves);
                    }
                    // Earlier siblings anchor `:has(~ x)` and `:has(+ x)`.
                    if flags(at).intersects(F::RELATIVE_SELECTOR_SEARCH_DIRECTION_SIBLING) {
                        let mut s = dom.n(at).prev;
                        while s != NIL {
                            if dom.n(s).is_element() {
                                reach(s, &mut subtrees, &mut selves);
                            }
                            s = dom.n(s).prev;
                        }
                    }
                    first = false;
                    at = dom.n(at).parent;
                }
            }
        }

        // Ancestors lead the traversal down to `n`.
        let lead = |n: u32| {
            let mut up = TNode::traversal_parent(&SNode::new(&cx, n));
            while let Some(a) = up {
                if a.has_dirty_descendants() {
                    break;
                }
                // SAFETY: no traversal is running.
                unsafe { a.set_dirty_descendants() };
                up = TNode::traversal_parent(&a);
            }
        };
        // A never styled element (new, or under `display: none`): its nearest
        // styled ancestor restyles its subtree. False when there is none.
        let restyle_from = |n: u32| -> bool {
            let mut at = n;
            while live(at) {
                if dom.n(at).is_element() {
                    if let Some(mut data) = SNode::new(&cx, at).mutate_data() {
                        data.hint.insert(RestyleHint::restyle_subtree());
                        drop(data);
                        lead(at);
                        return true;
                    }
                }
                at = dom.n(at).parent;
            }
            false
        };
        for &n in &changed {
            let el = SNode::new(&cx, n);
            let Some(slot) = el.slot().filter(|_| el.has_data()) else {
                if !restyle_from(n) {
                    return false;
                }
                continue;
            };
            let (old_state, new_state) = (slot.state.get(), el.state());
            let changes = attrs.get(&n).filter(|c| c.gen == dom.generation_at(n));
            let now = &dom.n(n).element().expect("element").attrs;
            // The attributes as they were: the current ones with each
            // recorded change undone.
            let old_attrs = changes.map(|c| {
                let mut old: Vec<(markup5ever::QualName, Box<str>)> = now
                    .iter()
                    .filter(|a| !c.old.iter().any(|(n, _)| *n == a.name))
                    .map(|a| (a.name.clone(), a.value.clone()))
                    .collect();
                old.extend(c.old.iter().filter_map(|(n, v)| v.clone().map(|v| (n.clone(), v))));
                old
            });
            if old_state == new_state && old_attrs.is_none() {
                continue;
            }
            let mut snapshot = ServoElementSnapshot::new();
            snapshot.state = Some(old_state);
            if let (Some(c), Some(old_attrs)) = (changes, old_attrs) {
                for (name, old) in &c.old {
                    let current = now.iter().find(|a| a.name == *name).map(|a| &a.value);
                    if current == old.as_ref() {
                        continue;
                    }
                    match &*name.local {
                        "class" if name.ns.is_empty() => snapshot.class_changed = true,
                        "id" if name.ns.is_empty() => snapshot.id_changed = true,
                        _ => snapshot.other_attributes_changed = true,
                    }
                    snapshot.changed_attrs.push(style::values::GenericAtomIdent(name.local.clone()));
                }
                snapshot.attrs = Some(
                    old_attrs
                        .into_iter()
                        .map(|(name, value)| {
                            let value = match (&*name.local, name.ns.is_empty()) {
                                ("class", true) => AttrValue::from_serialized_tokenlist(value.into()),
                                ("id", true) => AttrValue::from_atomic(value.into()),
                                _ => AttrValue::String(value.into()),
                            };
                            let ident = AttrIdentifier {
                                local_name: style::values::GenericAtomIdent(name.local.clone()),
                                name: style::values::GenericAtomIdent(name.local),
                                namespace: style::values::GenericAtomIdent(name.ns),
                                prefix: name.prefix.map(style::values::GenericAtomIdent),
                            };
                            (ident, value)
                        })
                        .collect(),
                );
            }
            // Attributes no selector reads but the element's own rules do:
            // `style`, presentational hints.
            let own = snapshot.changed_attrs.iter().any(|a| !matches!(&*a.0, "class" | "id"));
            self.snapshots.insert(OpaqueNode(n as usize), snapshot);
            slot.has_snapshot.set(true);
            if own {
                if let Some(mut data) = el.mutate_data() {
                    data.hint.insert(RestyleHint::RESTYLE_SELF);
                }
            }
            lead(n);
        }
        for n in own_rules {
            if let Some(mut data) = SNode::new(&cx, n).mutate_data() {
                data.hint.insert(RestyleHint::RESTYLE_SELF);
                drop(data);
                lead(n);
            }
        }
        for n in moved {
            if let Some(mut data) = SNode::new(&cx, n).mutate_data() {
                data.hint.insert(RestyleHint::restyle_subtree());
                drop(data);
                lead(n);
            }
        }
        for n in subtrees {
            if !restyle_from(n) {
                return false;
            }
        }
        for n in selves {
            match SNode::new(&cx, n).mutate_data() {
                Some(mut data) => {
                    data.hint.insert(RestyleHint::RESTYLE_SELF);
                    drop(data);
                    lead(n);
                }
                None => {
                    if !restyle_from(n) {
                        return false;
                    }
                }
            }
        }
        for p in parents {
            // SAFETY: no traversal is running.
            match SNode::new(&cx, p).slot().filter(|_| SNode::new(&cx, p).has_data()) {
                Some(_) => {
                    unsafe { SNode::new(&cx, p).set_dirty_descendants() };
                    lead(p);
                }
                None => {
                    if !restyle_from(p) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// The computed style of element `idx` (after [`Cascade::flush`]):
    /// stored when the traversal styled it, else resolved on demand (an
    /// element under `display: none`, or outside the document).
    pub(crate) fn style(&self, dom: &Dom, idx: u32) -> Option<Arc<ComputedValues>> {
        if !dom.slot_live(idx) || !dom.n(idx).is_element() {
            return None;
        }
        let cx = Cx::new(dom, &self.store, &self.lock, self.quirks, &self.popovers);
        let _entered = cx.enter();
        let el = SNode::new(&cx, idx);
        if let Some(data) = el.borrow_data() {
            if let Some(style) = data.styles.get_primary() {
                return Some(style.clone());
            }
        }
        thread_state::enter(ThreadState::LAYOUT);
        let guard = self.lock.read();
        let guards = StylesheetGuards { author: &guard, ua_or_user: &guard };
        let shared = SharedStyleContext {
            traversal_flags: TraversalFlags::empty(),
            stylist: &self.stylist,
            options: GLOBAL_STYLE_DATA.options.clone(),
            guards,
            visited_styles_enabled: false,
            animations: Default::default(),
            current_time_for_animations: 0.0,
            snapshot_map: &self.snapshots,
            registered_speculative_painters: &NoPainters,
        };
        let style = {
            let mut thread_local = ThreadLocalStyleContext::new();
            let mut context = StyleContext { shared: &shared, thread_local: &mut thread_local };
            resolve_style(&mut context, el, RuleInclusion::All, None, None).primary().clone()
        };
        thread_state::exit(ThreadState::LAYOUT);
        Some(style)
    }

    /// Elements with stored style data.
    pub fn styled_elements(&self) -> usize {
        self.store.len()
    }
}
