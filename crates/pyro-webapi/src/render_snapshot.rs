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
    /// Render keys (`pyro_dom::snapshot::render_key`) of the element under the
    /// pointer and the focused element, in every form.
    pub hovered: Option<u64>,
    pub focused: Option<u64>,
    /// Bitmaps of the canvases drawn on (`PageDom::canvas_bitmaps`), in
    /// every form.
    pub canvases: Vec<crate::canvas::CanvasBitmap>,
    /// An XML document (`pyro_dom::Dom::is_xml_document`), in every form.
    pub xml_document: bool,
    /// The document's HTTP `Content-Language`
    /// (`pyro_dom::Dom::content_language`), in every form.
    pub content_language: Option<String>,
    /// The document selection as a renderer paints it, in every form: from
    /// the start to the end, each a text node's render key and an offset in
    /// UTF-16 code units of its data (`text_selection`).
    pub selection: Option<((u64, u32), (u64, u32))>,
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
        self.snapshot_as(base_url, mark, Form::Html)
    }

    /// As [`PageDom::render_snapshot`], with the document as a keyed node
    /// list (`pyro_dom::Dom::render_tree`) instead of HTML: a renderer can
    /// diff it against the previous one. `mark`'s element is its `mark`.
    pub fn render_snapshot_tree(&self, base_url: &str, mark: Option<NodeId>) -> RenderSnapshot {
        self.snapshot_as(base_url, mark.map(|m| (m, "")), Form::Tree)
    }

    /// The viewport and scroll of a render snapshot, without the document.
    pub fn render_view(&self) -> RenderSnapshot {
        self.snapshot_as("", None, Form::None)
    }

    /// Changes whenever a render snapshot of the document could: the DOM
    /// (tree, attributes, text, form state, image sizes) or the style
    /// engine's sheets. Viewport and scroll are not included.
    pub fn render_stamp(&self) -> (u64, u64, u64) {
        let dom = self.dom.borrow();
        let (version, generation, _, _) = self.layout.borrow().style().observation_stamp(&dom);
        (dom.uid(), version, generation)
    }

    fn snapshot_as(&self, base_url: &str, mark: Option<(NodeId, &str)>, form: Form) -> RenderSnapshot {
        let mut canvases = self.canvas_bitmaps();
        canvases.extend(self.video_bitmaps());
        let selection = crate::editing::selection_range(self);
        let dom = self.dom.borrow();
        let selection = selection.and_then(|range| text_selection(&dom, self.document(), range));
        let layout = self.layout();
        RenderSnapshot { canvases, selection, ..snapshot_of(&dom, &layout, self.document(), base_url, mark, form) }
    }
}

fn snapshot_of(
    dom: &pyro_dom::Dom,
    layout: &pyro_dom::layout::Layout,
    document: NodeId,
    base_url: &str,
    mark: Option<(NodeId, &str)>,
    form: Form,
) -> RenderSnapshot {
    {
        let style = layout.style();
        let base = url::Url::parse(base_url).ok();
        let sheets = std::cell::RefCell::new(HashMap::new());
        let style_text = |owner: NodeId| style.sheet_text(owner).map(str::to_owned);
        let stylesheet_href = |link: NodeId| {
            let text = style.sheet_text(link)?;
            let href = dom.get_attribute(link, "href").ok().flatten()?;
            let url = base.as_ref()?.join(href.trim()).ok()?.to_string();
            sheets.borrow_mut().insert(url.clone(), text.to_owned());
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
                (dom.render_html_scrolled(document, &opts, &layout.scrolled_elements()).unwrap_or_default(), None)
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
            hovered: dom.hovered(document).map(pyro_dom::snapshot::render_key),
            focused: dom.focused(document).ok().flatten().map(pyro_dom::snapshot::render_key),
            canvases: Vec::new(),
            xml_document: dom.is_xml_document(document),
            content_language: dom.content_language(document).map(str::to_owned),
            selection: None,
        }
    }
}

/// The document selection as a renderer paints it ([`RenderSnapshot::selection`]): each end in
/// the text at it (a start between nodes moves forward to the start of the next rendered text, an
/// end back to the end of the one before), as the render key of a text node and an offset in
/// UTF-16 code units of its data. `None` when the selection is collapsed or holds no rendered
/// text. Text the render tree leaves out or replaces (in `<script>`, `<noscript>`, `<template>`,
/// frames, `<style>`, `<textarea>`) is passed over.
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
