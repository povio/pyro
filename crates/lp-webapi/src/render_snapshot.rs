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
    pub html: String,
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

impl PageDom {
    /// Snapshot of the document for rendering. `base_url` resolves link
    /// URLs (the frame's base URL); `mark` puts a valueless attribute on one
    /// element (a screenshot clip target).
    pub fn render_snapshot(&self, base_url: &str, mark: Option<(NodeId, &str)>) -> RenderSnapshot {
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
        let html = dom
            .render_html(
                self.document(),
                &RenderHtml { style_text: &style_text, stylesheet_href: &stylesheet_href, mark },
            )
            .unwrap_or_default();
        let viewport = layout.viewport();
        RenderSnapshot {
            html,
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
