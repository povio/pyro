//! What a renderer needs to paint the document as it is now
//! ([`PageDom::render_snapshot`]): HTML with form state written in, live
//! sheet text, the emulated viewport and the window scroll position. Plain
//! data, so another thread can render it.
//!
//! Canvas bitmaps and `<video>` frames travel beside the document
//! ([`RenderSnapshot::canvases`]).
//! Not captured: video pixels, iframe contents (empty boxes),
//! `:active` state (the hovered
//! and focused elements are carried as render keys for the renderer to
//! apply, and so is the selection), the caret, CSS animations/transitions (rendered at
//! their final values), shadow-tree style scoping (shadow `<style>` applies
//! document-wide).

use std::collections::HashMap;

use pyro_dom::snapshot::RenderHtml;
use pyro_dom::NodeId;

use crate::PageDom;

#[derive(Clone, Debug, Default)]
pub struct RenderSnapshot {
    /// Empty when `tree` is set.
    pub html: String,
    /// The document as keyed nodes ([`PageDom::render_snapshot_tree`]).
    pub tree: Option<pyro_dom::snapshot::RenderTree>,
    /// `<link rel=stylesheet>` texts by absolute URL (CSSOM edits included).
    pub sheets: HashMap<String, String>,
    /// CSS px.
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub dark: bool,
    /// Emulated `print` media.
    pub print: bool,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// Scrolled elements by render key, with their offset (CSS px), in
    /// every form.
    pub element_scroll: Vec<(u64, (f64, f64))>,
    /// The `position: sticky` elements that their scroll containers hold
    /// away from their laid-out place, by render key, with that offset (CSS
    /// px), in every form.
    pub sticky_offsets: Vec<(u64, (f64, f64))>,
    /// The current scroll targets (CSS Overflow 5 § 2.1) by render key, each scroll container's
    /// (`u64::MAX`: the viewport's) with its target's, in every form.
    pub scroll_targets: Vec<(u64, u64)>,
    pub popover_anchors: Vec<(u64, u64)>,
    /// Render keys (`pyro_dom::snapshot::render_key`) of the element under the
    /// pointer and the focused element, in every form.
    pub hovered: Option<u64>,
    pub focused: Option<u64>,
    /// Bitmaps of the canvases drawn on (`PageDom::canvas_bitmaps`), in
    /// every form.
    pub canvases: Vec<crate::canvas::CanvasBitmap>,
    /// The elements among `canvases` whose pixels a page must not read back (a video frame of
    /// media of another origin without CORS: `current_frame`'s `clean` is false), which the
    /// renderer marks so that the HTML-in-Canvas privacy paint taints the canvas.
    pub tainting_canvases: Vec<NodeId>,
    /// An XML document (`pyro_dom::Dom::is_xml_document`), in every form.
    pub xml_document: bool,
    /// A document in quirks mode (`pyro_dom::Dom::is_quirks_mode`), in every form.
    pub quirks_mode: bool,
    /// A document in limited-quirks mode (`pyro_dom::Dom::is_limited_quirks_mode`), in every form.
    pub limited_quirks_mode: bool,
    /// The document's HTTP `Content-Language`
    /// (`pyro_dom::Dom::content_language`), in every form.
    pub content_language: Option<String>,
    /// pyro: the nonce the markup was serialised with (its engine attributes' names carry it:
    /// `pyro_dom::snapshot::engine_name`): a renderer of the markup alone (a `capture_only`
    /// snapshot) needs it to take them, and takes none otherwise.
    pub nonce: String,
    /// The document's `CSS.registerProperty()` registrations, in order, in every form: name,
    /// syntax, inherits, initial value.
    pub registered_properties: Vec<(String, String, bool, Option<String>)>,
    /// The document selection as a renderer paints it, in every form: from
    /// the start to the end, each a text node's render key and an offset in
    /// UTF-16 code units of its data (`text_selection`).
    pub selection: Option<((u64, u32), (u64, u32))>,
    /// The highlights under the selection, bottom first, in every form: custom highlights
    /// (`CSS.highlights`, by priority, then registration) by name, then the target text (`None`),
    /// each with its ranges as [`RenderSnapshot::selection`]'s.
    pub highlights: Vec<(Option<String>, Vec<((u64, u32), (u64, u32))>)>,
    /// The focused text control's selection, in every form: its render key and the selection's
    /// start and end in UTF-16 code units of its value.
    pub control_selection: Option<(u64, u32, u32)>,
    /// The replaced elements (images, video, canvas) the document selection holds, in every
    /// form: their render keys (painted with the selection's tint).
    pub selected_replaced: Vec<u64>,
    /// The caret in editable content, in every form: the render key of a text node and
    /// an offset in UTF-16 code units of its data, or of an element with no text at the caret
    /// and 0 (`caret_point`).
    pub caret: Option<(u64, u32)>,
    /// The element `:target` matches (its render key), in every form.
    pub url_target: Option<u64>,
}

/// How a snapshot carries the document.
#[derive(Clone, Copy)]
enum Form {
    Html,
    Tree,
    None,
}

impl PageDom {
    /// Snapshot of the document for rendering. `base_url` resolves link
    /// URLs (the frame's base URL); `mark` puts a valueless attribute on one
    /// element (a screenshot clip target).
    pub fn render_snapshot(&self, base_url: &str, mark: Option<(NodeId, &str)>) -> RenderSnapshot {
        self.snapshot_as(base_url, mark, Form::Html, &[])
    }

    /// pyro: a frame's [`PageDom::render_snapshot`] with its animations now: its animated
    /// elements carry their render keys (`data-pyro-key`) for the renderer to sample them into
    /// the frame's document.
    pub fn render_snapshot_frame(self: &std::rc::Rc<Self>, base_url: &str, transitions: &[pyro_dom::geometry::TransitionFrame]) -> (RenderSnapshot, Option<pyro_dom::animation::AnimationFrame>) {
        let frame = self.animation_frame();
        if frame.elements.is_empty() && transitions.is_empty() {
            return (self.snapshot_as(base_url, None, Form::Html, &[]), None);
        }
        let mut keys: Vec<u64> = frame.elements.iter().map(|e| e.key).collect();
        for scope in transitions {
            keys.push(scope.root);
            keys.extend(scope.new.iter().map(|item| item.key));
        }
        (self.snapshot_as(base_url, None, Form::Html, &keys), (!frame.elements.is_empty()).then_some(frame))
    }

    /// As [`PageDom::render_snapshot`], with the document as a keyed node
    /// list (`pyro_dom::Dom::render_tree`) instead of HTML: a renderer can
    /// diff it against the previous one. `mark`'s element is its `mark`.
    pub fn render_snapshot_tree(&self, base_url: &str, mark: Option<NodeId>) -> RenderSnapshot {
        self.snapshot_as(base_url, mark.map(|m| (m, "")), Form::Tree, &[])
    }

    /// The base URL a render snapshot's document resolves URLs against: `frame_base_url` (the
    /// frame's), or the one a `<base href>` gives the document (HTML "document base URL"; WPT
    /// css-style-attr `style-attr-urls-002`: `url()` in a style attribute under `<base>`).
    pub fn render_base_url(&self, frame_base_url: &str) -> String {
        let dom = self.dom.borrow();
        let doc = self.document();
        match (dom.document_base_url(doc), dom.document_fallback_base_url(doc)) {
            (Ok(base), Ok(fallback)) if base != fallback => base.to_owned(),
            _ => frame_base_url.to_owned(),
        }
    }

    /// The viewport and scroll of a render snapshot, without the document.
    pub fn render_view(&self) -> RenderSnapshot {
        self.snapshot_as("", None, Form::None, &[])
    }

    /// Changes whenever a render snapshot of the document could: the DOM
    /// (tree, attributes, text, form state, image sizes) or the style
    /// engine's sheets. Viewport and scroll are not included.
    pub fn render_stamp(&self) -> (u64, u64, u64) {
        let dom = self.dom.borrow();
        let (version, generation, _, _) = self.layout.borrow().style().observation_stamp(&dom);
        (dom.uid(), version, generation)
    }

    fn snapshot_as(&self, base_url: &str, mark: Option<(NodeId, &str)>, form: Form, keyed: &[u64]) -> RenderSnapshot {
        // pyro: a mandatory snap container starts at a snap position (CSS Scroll Snap 1 § 6.3),
        // which a capture shows even when no rendering update ran since the page loaded.
        if let (Ok(dom), Ok(mut layout)) = (self.dom.try_borrow(), self.layout.try_borrow_mut()) {
            layout.snap_initial_positions(&dom);
        }
        let mut canvases = self.canvas_bitmaps();
        let mut tainting_canvases = Vec::new();
        for (bitmap, clean) in self.video_frames() {
            if !clean {
                tainting_canvases.push(bitmap.0);
            }
            canvases.push(bitmap);
        }
        let selection = crate::editing::selection_range(self);
        let dom = self.dom.borrow();
        let selected_replaced = selection.map_or_else(Vec::new, |range| replaced_in(&dom, self.document(), range));
        let caret = selection.and_then(|range| caret_point(&dom, self.document(), range));
        let selection = selection.and_then(|range| text_selection(&dom, self.document(), range));
        let mut highlights = Vec::new();
        {
            let custom = self.highlights.borrow();
            let mut order: Vec<&crate::highlights::CustomHighlight> = custom.iter().collect();
            order.sort_by_key(|highlight| highlight.priority);
            for highlight in order {
                let ranges: Vec<_> = highlight
                    .ranges
                    .iter()
                    .filter_map(|range| text_selection(&dom, self.document(), range.points()))
                    .collect();
                if !ranges.is_empty() {
                    highlights.push((Some(highlight.name.clone()), ranges));
                }
            }
            let target: Vec<_> = self
                .target_text
                .borrow()
                .iter()
                .filter_map(|owner| text_selection(&dom, self.document(), (owner.live.start(), owner.live.end())))
                .collect();
            if !target.is_empty() {
                highlights.push((None, target));
            }
        }
        // The focused text control's selection (painted with its `::selection` style).
        let control_selection = dom.focused(self.document()).ok().flatten().and_then(|id| {
            let (start, end) = dom.selection(id).ok()?;
            Some((pyro_dom::snapshot::render_key(id), start, end))
        });
        let layout = self.layout();
        RenderSnapshot {
            canvases,
            tainting_canvases,
            selection,
            highlights,
            control_selection,
            selected_replaced,
            caret,
            url_target: dom.url_target_element(self.document()).map(pyro_dom::snapshot::render_key),
            ..snapshot_of(&dom, &layout, self.document(), base_url, mark, form, keyed)
        }
    }
}

fn snapshot_of(
    dom: &pyro_dom::Dom,
    layout: &pyro_dom::layout::Layout,
    document: NodeId,
    base_url: &str,
    mark: Option<(NodeId, &str)>,
    form: Form,
    keyed: &[u64],
) -> RenderSnapshot {
    {
        let style = layout.style();
        let base = url::Url::parse(base_url).ok();
        let sheets = std::cell::RefCell::new(HashMap::new());
        let style_text = |owner: NodeId| style.sheet_text(&dom, owner).map(str::to_owned);
        let stylesheet_href = |link: NodeId| {
            let text = style.sheet_text(&dom, link)?;
            // pyro: the sheet's own URL, its response's after redirects (its URLs resolve
            // against it), else its href.
            let url = style.sheet_base_url(&dom, link, base.as_ref()?.as_str())?;
            sheets.borrow_mut().insert(dom.sheet_key(link), text.to_owned());
            Some(url)
        };
        let adopted = |scope: NodeId| style.adopted_sheets(scope);
        let opts = RenderHtml {
            style_text: &style_text,
            stylesheet_href: &stylesheet_href,
            mark,
            adopted: style.has_adopted().then_some(&adopted as _),
        };
        let (html, tree) = match form {
            Form::Html => {
                (
                dom.render_html_keyed(document, &opts, &layout.scrolled_elements(), &layout.sticky_offsets(dom), keyed)
                    .unwrap_or_default(),
                None,
            )
            }
            Form::Tree => (String::new(), Some(dom.render_tree(document, &opts).unwrap_or_default())),
            Form::None => (String::new(), None),
        };
        let viewport = layout.viewport();
        RenderSnapshot {
            html,
            tree,
            sheets: sheets.into_inner(),
            width: viewport.width,
            height: viewport.height,
            device_scale_factor: viewport.device_scale_factor,
            dark: viewport.color_scheme == pyro_dom::media::ColorScheme::Dark,
            print: viewport.print,
            scroll_x: layout.scroll_x() as f64,
            scroll_y: layout.scroll_y() as f64,
            element_scroll: layout.scrolled_elements(),
            sticky_offsets: layout.sticky_offsets(dom),
            scroll_targets: layout.scroll_targets(dom),
            popover_anchors: dom.popover_anchor_keys(),
            hovered: dom.hovered(document).map(pyro_dom::snapshot::render_key),
            focused: dom.focused(document).ok().flatten().map(pyro_dom::snapshot::render_key),
            canvases: Vec::new(),
            tainting_canvases: Vec::new(),
            xml_document: dom.is_xml_document(document),
            quirks_mode: dom.is_quirks_mode(document),
            limited_quirks_mode: dom.is_limited_quirks_mode(document),
            content_language: dom.content_language(document).map(str::to_owned),
            nonce: dom.frame_table(document).nonce,
            registered_properties: layout
                .style()
                .registered_properties()
                .iter()
                .map(|p| (p.name.clone(), p.syntax.clone(), p.inherits, p.initial_value.clone()))
                .collect(),
            selection: None,
            highlights: Vec::new(),
            control_selection: None,
            selected_replaced: Vec::new(),
            caret: None,
            url_target: None,
        }
    }
}

/// The caret as a renderer paints it ([`RenderSnapshot::caret`]): a collapsed selection in
/// editable content (while no other element has focus), at the text there (the start of the text after it, else the end
/// of the text before it), or the element it is in when no text is next to it (an empty host).
fn caret_point(
    dom: &pyro_dom::Dom,
    document: NodeId,
    (start, end): (crate::editing::Point, crate::editing::Point),
) -> Option<(u64, u32)> {
    use crate::editing::{deep_backward, deep_forward, editing_host, is_text};
    if start != end {
        return None;
    }
    // Chromium paints the caret of a collapsed selection in editable content while no other
    // element has focus (a script's `collapse()` into an editing host does not focus it).
    let host = editing_host(dom, start.0)?;
    let focused = dom.focused(document).ok().flatten();
    let body = dom.body(document).ok().flatten();
    let root = dom.document_element(document).ok().flatten();
    if focused.is_some_and(|focused| focused != host && Some(focused) != body && Some(focused) != root) {
        return None;
    }
    let (node, offset) = if is_text(dom, start.0) {
        start
    } else {
        let forward = deep_forward(dom, start);
        let backward = deep_backward(dom, start);
        if is_text(dom, forward.0) {
            forward
        } else if is_text(dom, backward.0) {
            backward
        } else {
            (backward.0, 0)
        }
    };
    Some((pyro_dom::snapshot::render_key(node), offset))
}

/// The document selection as a renderer paints it ([`RenderSnapshot::selection`]): each end in
/// the text at it (a start between nodes moves forward to the start of the next rendered text, an
/// end back to the end of the one before), as the render key of a text node and an offset in
/// UTF-16 code units of its data. `None` when the selection is collapsed or holds no rendered
/// text. Text the render tree leaves out or replaces (in `<script>`, `<noscript>`, `<template>`,
/// frames, `<style>`, `<textarea>`) is passed over.
/// The replaced elements (`img`, `video`, `canvas`) inside the selection `start..end`: their
/// render keys.
fn replaced_in(dom: &pyro_dom::Dom, document: NodeId, (start, end): (crate::editing::Point, crate::editing::Point)) -> Vec<u64> {
    use crate::editing::local;
    use crate::range::compare_boundary_points_dom as compare;
    if start == end {
        return Vec::new();
    }
    dom.descendants(document)
        .ok()
        .into_iter()
        .flatten()
        .filter(|&n| matches!(local(dom, n).as_str(), "img" | "video" | "canvas"))
        .filter(|&n| {
            let Some(parent) = dom.parent(n).ok().flatten() else { return false };
            let Some(index) = dom.children(parent).ok().and_then(|mut c| c.position(|c| c == n)) else { return false };
            let (before, after) = ((parent, index as u32), (parent, index as u32 + 1));
            compare(dom, start, before) <= 0 && compare(dom, after, end) <= 0
        })
        .map(pyro_dom::snapshot::render_key)
        .collect()
}

fn text_selection(
    dom: &pyro_dom::Dom,
    document: NodeId,
    (start, end): (crate::editing::Point, crate::editing::Point),
) -> Option<((u64, u32), (u64, u32))> {
    use crate::editing::{deep_backward, deep_forward, is_text, len, local};
    use crate::range::compare_boundary_points_dom as compare;
    if start == end {
        return None;
    }
    let rendered = |text: NodeId| {
        let mut current = dom.parent(text).ok().flatten();
        while let Some(node) = current {
            if matches!(
                local(dom, node).as_str(),
                "script" | "noscript" | "template" | "iframe" | "frame" | "style" | "textarea"
            ) {
                return false;
            }
            current = dom.parent(node).ok().flatten();
        }
        true
    };
    let texts = || dom.descendants(document).ok().into_iter().flatten().filter(|&n| is_text(dom, n) && rendered(n));
    let first = deep_forward(dom, start);
    let first = match is_text(dom, first.0) && rendered(first.0) {
        true => first,
        false => (texts().find(|&t| compare(dom, (t, 0), start) >= 0)?, 0),
    };
    let last = deep_backward(dom, end);
    let last = match is_text(dom, last.0) && rendered(last.0) {
        true => last,
        false => {
            let text = texts().filter(|&t| compare(dom, (t, len(dom, t)), end) <= 0).last()?;
            (text, len(dom, text))
        }
    };
    (compare(dom, first, last) < 0)
        .then(|| ((pyro_dom::snapshot::render_key(first.0), first.1), (pyro_dom::snapshot::render_key(last.0), last.1)))
}
