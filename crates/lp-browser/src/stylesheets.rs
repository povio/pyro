//! Document-owned linked stylesheets. Transport completion never owns the document.
use crate::frames::FrameKey;
use crate::loader::ResourceNode;
use crate::{Browser, FetchRequest, LoaderId, Page, Resource, ResourceCancellation, ResourceKind};
use lp_dom::script::{ResourceEvent, ResourceKind as DomResourceKind, ResourceReason};
use lp_dom::NodeId;
use lp_runtime::TaskSource;
use lp_webapi::PageDom;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) const MAX_SHEET_BYTES: usize = 4 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SHEETS: usize = 128;

struct Sheet {
    node: Rc<ResourceNode>,
    url: String,
    crossorigin: Option<String>,
    integrity: bool,
    generation: u64,
    pending: bool,
    parser_inserted: bool,
    bytes: usize,
    cancel: Option<ResourceCancellation>,
}
#[derive(Default)]
pub(crate) struct Stylesheets {
    entries: HashMap<NodeId, Sheet>,
    generation: u64,
    bytes: usize,
}
impl Stylesheets {
    pub(crate) fn pending(&self) -> usize {
        self.entries.values().filter(|s| s.pending).count()
    }
    fn remove(&mut self, node: NodeId) {
        if let Some(sheet) = self.entries.remove(&node) {
            self.bytes -= sheet.bytes;
            // Dropping the cancellation handle stops the native request.
        }
    }
}
impl Page {
    /// Pending linked stylesheets and retained decoded CSS bytes. This is
    /// logical source accounting, not process memory or parsed-rule overhead.
    pub fn stylesheet_usage(&self) -> (usize, usize) {
        (self.stylesheets.pending(), self.stylesheets.bytes)
    }
}

fn remove_sheet(page: &PageDom, node: NodeId) {
    page.apply_style_event(&ResourceEvent { node, kind: DomResourceKind::Stylesheet, reason: ResourceReason::Removed });
}

impl Browser {
    pub(crate) fn stylesheet_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let loader = state.main_frame.loader_id.clone();
        let referrer = state.main_frame.base_url.clone();
        // Events describe mutations; inspect the final state so several href/rel
        // writes in one task do not launch intermediate requests.
        let config = {
            let dom = bindings.dom.borrow();
            let attr = |name| dom.get_attribute(node, name).ok().flatten();
            let enabled = dom.is_connected(node).unwrap_or(false)
                && attr("rel")
                    .is_some_and(|r| r.split_ascii_whitespace().any(|v| v.eq_ignore_ascii_case("stylesheet")))
                && !dom.has_attribute(node, "disabled").unwrap_or(false)
                && attr("type").is_none_or(|v| v.is_empty() || v.eq_ignore_ascii_case("text/css"));
            enabled
                .then(|| {
                    let href = attr("href").filter(|v| !v.trim().is_empty())?;
                    let url = url::Url::parse(&referrer).ok()?.join(href).ok()?.to_string();
                    Some((
                        url,
                        attr("crossorigin").map(str::to_owned),
                        attr("integrity").is_some_and(|v| !v.is_empty()),
                    ))
                })
                .flatten()
        };
        let Some((url, crossorigin, integrity)) = config else {
            drop(state);
            self.doc_mut(page).expect("page").stylesheets.remove(node);
            remove_sheet(&bindings, node);
            self.queue_stylesheets_changed(page, loader);
            return;
        };
        if state
            .stylesheets
            .entries
            .get(&node)
            .is_some_and(|s| s.url == url && s.crossorigin == crossorigin && s.integrity == integrity)
        {
            return;
        }
        drop(state);
        let mut guard = self.doc_mut(page).expect("page");
        let sheets = &mut guard.stylesheets;
        let replacing = sheets.entries.contains_key(&node);
        sheets.remove(node);
        if !replacing && sheets.entries.len() >= MAX_SHEETS {
            // Do not retain another sheet or start unbounded transport work.
            drop(guard);
            self.stylesheet_error_task(page, loader, bindings, node);
            return;
        }
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
        sheets.generation += 1;
        let generation = sheets.generation;
        sheets.entries.insert(
            node,
            Sheet {
                node: hold,
                url: url.clone(),
                crossorigin: crossorigin.clone(),
                integrity,
                generation,
                parser_inserted: bindings
                    .dom
                    .borrow()
                    .node(node)
                    .ok()
                    .and_then(|n| n.element())
                    .is_some_and(|e| e.flags & lp_dom::node::element_flags::PARSER_INSERTED != 0),
                pending: true,
                bytes: 0,
                cancel: None,
            },
        );
        drop(guard);
        // Reserve a cascade slot now; completion order must not reorder sheets.
        bindings.set_stylesheet_text(node, "");
        let weak = Rc::downgrade(&bindings);
        let finish = move |b: &mut Browser, result| {
            let Some(bindings) = weak.upgrade() else { return };
            b.finish_stylesheet(page, &loader, &bindings, node, generation, result);
        };
        if integrity {
            self.scheduler()
                .post(TaskSource::Networking, move |b| finish(b, Err("stylesheet integrity is unsupported".into())));
            return;
        }
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let frame = self.doc(page).expect("page").main_frame.clone();
        let request = FetchRequest {
            url,
            kind: ResourceKind::Stylesheet,
            page: page.page,
            referrer: Some(referrer),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
        };
        let cancel = resource_loader.fetch_cancellable(self, request, Box::new(finish));
        if let Some(mut sheet) = self.doc_mut(page).and_then(|p| p.filter_map(|p| p.stylesheets.entries.get_mut(&node)))
        {
            sheet.cancel = cancel;
        }
    }

    fn finish_stylesheet(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        bindings: &Rc<PageDom>,
        node: NodeId,
        generation: u64,
        result: Result<Resource, String>,
    ) {
        self.drain_dom_resources(page);
        let Some(mut state) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
        let sheets = &mut state.stylesheets;
        let Some(sheet) = sheets.entries.get_mut(&node).filter(|s| s.generation == generation && s.pending) else {
            return;
        };
        if let Some(cancel) = sheet.cancel.take() {
            cancel.disarm();
        }
        sheet.pending = false;
        let text = result
            .ok()
            .filter(|r| {
                (200..300).contains(&r.status)
                    && r.mime.eq_ignore_ascii_case("text/css")
                    && r.body.len() <= MAX_SHEET_BYTES
            })
            .map(|r| r.text());
        let text = text.filter(|t| t.len() <= MAX_SHEET_BYTES && t.len() <= MAX_DOCUMENT_BYTES - sheets.bytes);
        let ok = text.is_some();
        if let Some(text) = text {
            sheet.bytes = text.len();
            sheets.bytes += text.len();
            bindings.set_stylesheet_text(node, &text);
        } else {
            remove_sheet(bindings, node);
        }
        drop(state);
        // This callback is already a later owner-thread task. No DOM/state
        // borrow survives dispatch into JavaScript.
        lp_webapi::lifecycle::resource_loaded(bindings, node, ok);
        self.stylesheets_changed(page);
    }

    fn stylesheet_error_task(&self, page: FrameKey, loader: LoaderId, bindings: Rc<PageDom>, node: NodeId) {
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = ResourceNode { dom: bindings.dom.clone(), node };
        let weak = Rc::downgrade(&bindings);
        self.scheduler().post(TaskSource::Networking, move |b| {
            let _pin = &hold;
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                if let Some(bindings) = weak.upgrade() {
                    lp_webapi::lifecycle::resource_loaded(&bindings, hold.node, false);
                }
            }
        });
    }

    fn queue_stylesheets_changed(&self, page: FrameKey, loader: LoaderId) {
        self.scheduler().post(TaskSource::Networking, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.stylesheets_changed(page);
            }
        });
    }

    pub(crate) fn stylesheets_block_scripts(&self, page: FrameKey) -> bool {
        let Some(p) = self.doc(page) else { return false };
        let Some(bindings) = &p.main_frame.page_dom else { return false };
        let dom = bindings.dom.borrow();
        p.stylesheets.entries.values().any(|sheet| {
            sheet.pending && sheet.parser_inserted && {
                let media = dom.get_attribute(sheet.node.node, "media").ok().flatten().unwrap_or("");
                media.is_empty() || lp_dom::media::matches(media, &bindings.viewport())
            }
        })
    }
}
