//! What a renderer needs to paint the document as it is now
//! ([`PageDom::render_snapshot`]): HTML with form state written in, live
//! sheet text, the emulated viewport and the window scroll position. Plain
//! data, so another thread can render it.
//!
//! Not captured: canvas and video pixels, iframe contents (empty boxes),
//! element scroll offsets (only the window's), `:hover`/`:focus`/`:active`
//! state, selection and caret, CSS animations/transitions (rendered at
//! their final values), shadow-tree style scoping (shadow `<style>` applies
//! document-wide).

use std::collections::HashMap;

use lp_dom::snapshot::RenderHtml;
use lp_dom::NodeId;

use crate::PageDom;

#[derive(Clone, Debug, Default)]
pub struct RenderSnapshot {
    /// Empty when `tree` is set.
    pub html: String,
    /// The document as keyed nodes ([`PageDom::render_snapshot_tree`]).
    pub tree: Option<lp_dom::snapshot::RenderTree>,
    /// `<link rel=stylesheet>` texts by absolute URL (CSSOM edits included).
    pub sheets: HashMap<String, String>,
    /// CSS px.
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub dark: bool,
    pub scroll_x: f64,
    pub scroll_y: f64,
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
    /// list (`lp_dom::Dom::render_tree`) instead of HTML: a renderer can
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
        let dom = self.dom.borrow();
        let layout = self.layout.borrow();
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
        let opts = RenderHtml { style_text: &style_text, stylesheet_href: &stylesheet_href, mark };
        let (html, tree) = match form {
            Form::Html => (dom.render_html(self.document(), &opts).unwrap_or_default(), None),
            Form::Tree => (String::new(), Some(dom.render_tree(self.document(), &opts).unwrap_or_default())),
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
            dark: viewport.color_scheme == lp_dom::media::ColorScheme::Dark,
            scroll_x: layout.scroll_x() as f64,
            scroll_y: layout.scroll_y() as f64,
        }
    }
}
