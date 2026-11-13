//! Document-owned style sheets: linked ones, fetched here, and the sheets of
//! `<style>` elements, which get `load` (or `error`) once processed, after
//! the `@import`s they start with are fetched. Transport completion never
//! owns the document.
use crate::frames::FrameKey;
use crate::loader::ResourceNode;
use crate::{Browser, FetchRequest, LoaderId, Page, Resource, ResourceCancellation, ResourceKind};
use pyro_dom::script::{ResourceEvent, ResourceKind as DomResourceKind, ResourceReason};
use pyro_dom::NodeId;
use pyro_runtime::TaskSource;
use pyro_webapi::PageDom;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) const MAX_SHEET_BYTES: usize = 4 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SHEETS: usize = 128;
/// `@import` nesting a `<style>` waits for (Blitz lays out as deep).
const MAX_IMPORT_DEPTH: usize = 16;
/// Imported sheets fetched for one `<style>`, nested ones included.
const MAX_STYLE_IMPORTS: usize = 64;

struct Sheet {
    node: Rc<ResourceNode>,
    url: String,
    crossorigin: Option<String>,
    /// `integrity` metadata with a recognized hash (checked on completion).
    integrity: Option<String>,
    generation: u64,
    pending: bool,
    parser_inserted: bool,
    bytes: usize,
    cancel: Option<ResourceCancellation>,
}
/// A `<style>` whose sheet's `@import`s are being fetched.
struct StyleImports {
    _node: Rc<ResourceNode>,
    /// Its processing (a new one, for new text, drops this one).
    generation: u64,
    /// Fetches not finished, nested imports included.
    pending: usize,
    /// Fetches started, nested imports included.
    started: usize,
    /// An import of the sheet itself failed (one of an imported sheet does
    /// not count, as in Chromium): the element gets `error`.
    failed: bool,
    /// Blocks the parser's scripts while pending (HTML "contributes a
    /// script-blocking style sheet").
    script_blocking: bool,
    /// Dropping one stops its request.
    cancels: Vec<Option<ResourceCancellation>>,
}

#[derive(Default)]
pub(crate) struct Stylesheets {
    entries: HashMap<NodeId, Sheet>,
    generation: u64,
    bytes: usize,
    /// `<style>` elements with a processed CSS sheet (connected, a CSS
    /// `type`): a `type` change processes one that had none.
    styles: HashSet<NodeId>,
    /// `<style>` elements waiting for their `@import`s.
    style_imports: HashMap<NodeId, StyleImports>,
    /// `load` / `error` events of `<style>` elements queued and not fired
    /// yet: like the imports, they delay the document's load event.
    style_events: usize,
}
impl Stylesheets {
    pub(crate) fn pending(&self) -> usize {
        self.entries.values().filter(|s| s.pending).count()
    }
    /// `<style>` elements still to fire `load` or `error`.
    pub(crate) fn styles_pending(&self) -> bool {
        self.style_events != 0 || !self.style_imports.is_empty()
    }
    /// `<style>` events queued and not fired yet.
    pub(crate) fn style_events_queued(&self) -> bool {
        self.style_events != 0
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
                    // `<base href>` aware; the referrer stays the frame's URL.
                    let base = crate::loader::node_base_url(&dom, node).unwrap_or_else(|| referrer.clone());
                    let url = url::Url::parse(&base).ok()?.join(href).ok()?.to_string();
                    Some((
                        url,
                        attr("crossorigin").map(str::to_owned),
                        attr("integrity").filter(|v| !pyro_webapi::sri::is_empty(v)).map(str::to_owned),
                    ))
                })
                .flatten()
        };
        let Some((url, crossorigin, integrity)) = config else {
            drop(state);
            self.doc_mut(page).expect("page").stylesheets.remove(node);
            bindings.unblock_rendering(node);
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
                integrity: integrity.clone(),
                generation,
                parser_inserted: bindings
                    .dom
                    .borrow()
                    .node(node)
                    .ok()
                    .and_then(|n| n.element())
                    .is_some_and(|e| e.flags & pyro_dom::node::element_flags::PARSER_INSERTED != 0),
                pending: true,
                bytes: 0,
                cancel: None,
            },
        );
        drop(guard);
        // HTML "block rendering": a stylesheet the parser inserted blocks
        // implicitly, any other one through `blocking=render`; both only
        // when their media matches. `finish_stylesheet` unblocks.
        let blocker = {
            let dom = bindings.dom.borrow();
            let attr = |name| dom.get_attribute(node, name).ok().flatten();
            let media = attr("media").unwrap_or("");
            let parser_inserted =
                dom.element(node).is_ok_and(|e| e.flags & pyro_dom::node::element_flags::PARSER_INSERTED != 0);
            let explicit =
                attr("blocking").is_some_and(|v| v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("render")));
            if !(media.is_empty() || pyro_dom::media::matches(media, &bindings.viewport())) {
                None
            } else if parser_inserted {
                Some(pyro_webapi::RenderBlocker::Implicit)
            } else {
                explicit.then_some(pyro_webapi::RenderBlocker::Explicit)
            }
        };
        if let Some(blocker) = blocker {
            bindings.block_rendering(node, blocker);
        }
        // Reserve a cascade slot now; completion order must not reorder sheets.
        bindings.set_stylesheet_text(node, "");
        let weak = Rc::downgrade(&bindings);
        let cors = crossorigin.is_some();
        let finish = move |b: &mut Browser, result: Result<Resource, String>| {
            let Some(bindings) = weak.upgrade() else { return };
            // SRI: a mismatching (or opaque) response is a network error.
            let result = result.and_then(|r| {
                let origin = b.doc(page).map(|p| p.main_frame.origin.clone()).unwrap_or_default();
                match crate::loader::integrity_error(&r, integrity.as_deref(), cors, &origin) {
                    Some(e) => {
                        b.report_console_error(page, &e);
                        Err(e)
                    }
                    None => Ok(r),
                }
            });
            b.finish_stylesheet(page, &loader, &bindings, node, generation, result);
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let frame = self.doc(page).expect("page").main_frame.clone();
        let request = FetchRequest {
            post: None,
            url,
            kind: ResourceKind::Stylesheet,
            page: page.page,
            referrer: Some(referrer),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
            initiator: Some(pyro_webapi::resource_timing::Initiator::Link),
            module: None,
            part: None,
        };
        let cancel = self.fetch_document_resource(page, &resource_loader, request, Box::new(finish));
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
        bindings.unblock_rendering(node);
        // This callback is already a later owner-thread task. No DOM/state
        // borrow survives dispatch into JavaScript.
        pyro_webapi::lifecycle::resource_loaded(bindings, node, ok);
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
                    pyro_webapi::lifecycle::resource_loaded(&bindings, hold.node, false);
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
        if p.stylesheets.style_imports.values().any(|s| s.script_blocking) {
            return true;
        }
        let Some(bindings) = &p.main_frame.page_dom else { return false };
        let dom = bindings.dom.borrow();
        p.stylesheets.entries.values().any(|sheet| {
            sheet.pending && sheet.parser_inserted && {
                let media = dom.get_attribute(sheet.node.node, "media").ok().flatten().unwrap_or("");
                media.is_empty() || pyro_dom::media::matches(media, &bindings.viewport())
            }
        })
    }

    /// HTML "update a style block" for the `<style>` `node`, whose text,
    /// `type` or connection changed (`reason`; the style engine has its
    /// text already). A connected one of a CSS `type` gets `load` in a later
    /// task, once the `@import`s its sheet starts with (and theirs) are
    /// fetched, or `error` when one of its own fails, as in Chromium: there
    /// each processing (insertion, a text or `type` change; not a `media`
    /// change) fires one, and the parser's element is processed when it is
    /// popped. While its imports load, a parser-inserted element blocks the
    /// parser's scripts and (media matching) rendering. Returns whether the
    /// element was processed.
    pub(crate) fn style_element_event(&mut self, page: FrameKey, node: NodeId, reason: ResourceReason) -> bool {
        let Some(state) = self.doc(page) else { return false };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return false };
        let loader = state.main_frame.loader_id.clone();
        let had_sheet = state.stylesheets.styles.contains(&node);
        drop(state);
        let processing = {
            let dom = bindings.dom.borrow();
            let attr = |name| dom.get_attribute(node, name).ok().flatten();
            let flags = dom.element(node).map_or(0, |e| e.flags);
            let css = reason != ResourceReason::Removed
                && dom.is_connected(node).unwrap_or(false)
                && attr("type").is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("text/css"));
            if !css {
                None
            } else if flags & pyro_dom::node::element_flags::STYLE_PARSING != 0
                || (reason == ResourceReason::AttrChanged && had_sheet)
            {
                // Its text is still coming, or an attribute other than a
                // `type` that made it CSS changed.
                return false;
            } else {
                let base = crate::loader::node_base_url(&dom, node).unwrap_or_default();
                let base = url::Url::parse(&base).ok();
                let urls: Vec<String> = pyro_dom::cssom::leading_imports(&dom.child_text_content(node))
                    .iter()
                    .filter_map(|href| Some(base.as_ref()?.join(href).ok()?.to_string()))
                    .collect();
                let media = attr("media").unwrap_or("");
                let media_matches = media.is_empty() || pyro_dom::media::matches(media, &bindings.viewport());
                let parser_inserted = flags & pyro_dom::node::element_flags::PARSER_INSERTED != 0;
                let explicit = attr("blocking")
                    .is_some_and(|v| v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("render")));
                let blocker = match (media_matches, parser_inserted, explicit) {
                    (false, ..) => None,
                    (true, true, _) => Some(pyro_webapi::RenderBlocker::Implicit),
                    (true, false, explicit) => explicit.then_some(pyro_webapi::RenderBlocker::Explicit),
                };
                Some((urls, parser_inserted && media_matches, blocker))
            }
        };
        let previous = {
            let Some(mut doc) = self.doc_mut(page) else { return false };
            let sheets = &mut doc.stylesheets;
            if processing.is_some() {
                sheets.styles.insert(node);
            } else {
                sheets.styles.remove(&node);
            }
            sheets.style_imports.remove(&node)
        };
        if let Some(previous) = &previous {
            bindings.unblock_rendering(node);
            if previous.script_blocking {
                self.queue_stylesheets_changed(page, loader.clone());
            }
        }
        let Some((urls, script_blocking, blocker)) = processing else { return false };
        if urls.is_empty() {
            self.queue_style_event(page, loader, &bindings, node, true);
            return true;
        }
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return false;
        }
        let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
        let generation = {
            let Some(mut doc) = self.doc_mut(page) else { return false };
            let sheets = &mut doc.stylesheets;
            sheets.generation += 1;
            let generation = sheets.generation;
            sheets.style_imports.insert(
                node,
                StyleImports {
                    _node: hold,
                    generation,
                    pending: 0,
                    started: 0,
                    failed: false,
                    script_blocking,
                    cancels: Vec::new(),
                },
            );
            generation
        };
        if let Some(blocker) = blocker {
            bindings.block_rendering(node, blocker);
        }
        let referrer = self.doc(page).map(|p| p.main_frame.url.clone()).unwrap_or_default();
        for url in urls {
            self.fetch_style_import(page, &loader, node, generation, url, 1, &referrer);
        }
        self.style_imports_settled(page, &loader, node);
        true
    }

    /// Fetches one sheet a `<style>`'s sheet imports, `depth` imports deep.
    #[allow(clippy::too_many_arguments)]
    fn fetch_style_import(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        node: NodeId,
        generation: u64,
        url: String,
        depth: usize,
        referrer: &str,
    ) {
        let index = {
            let Some(mut doc) = self.doc_mut(page) else { return };
            let Some(entry) = doc.stylesheets.style_imports.get_mut(&node).filter(|e| e.generation == generation)
            else {
                return;
            };
            if entry.started >= MAX_STYLE_IMPORTS {
                return;
            }
            entry.started += 1;
            entry.pending += 1;
            entry.cancels.push(None);
            entry.cancels.len() - 1
        };
        let finish_loader = loader.clone();
        let finish_url = url.clone();
        let finish = move |b: &mut Browser, result: Result<Resource, String>| {
            b.style_import_done(page, &finish_loader, node, generation, index, depth, &finish_url, result);
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let Some(frame) = self.doc(page).map(|p| p.main_frame.clone()) else { return };
        let request = FetchRequest {
            post: None,
            url,
            kind: ResourceKind::Stylesheet,
            page: page.page,
            referrer: Some(referrer.to_owned()),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: None,
            initiator: Some(pyro_webapi::resource_timing::Initiator::Css),
            module: None,
            part: None,
        };
        let cancel = self.fetch_document_resource(page, &resource_loader, request, Box::new(finish));
        if let Some(mut doc) = self.doc_mut(page) {
            if let Some(slot) = doc
                .stylesheets
                .style_imports
                .get_mut(&node)
                .filter(|e| e.generation == generation)
                .and_then(|e| e.cancels.get_mut(index))
            {
                *slot = cancel;
            }
        }
    }

    /// An imported sheet of a `<style>` arrived (or failed): the layout gets
    /// its body, its own imports are fetched, and once nothing is pending
    /// the element's event is queued and what it blocked goes on.
    #[allow(clippy::too_many_arguments)]
    fn style_import_done(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        node: NodeId,
        generation: u64,
        index: usize,
        depth: usize,
        url: &str,
        result: Result<Resource, String>,
    ) {
        let ok = result.as_ref().is_ok_and(|r| (200..300).contains(&r.status));
        {
            let Some(mut doc) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
            let Some(entry) = doc.stylesheets.style_imports.get_mut(&node).filter(|e| e.generation == generation)
            else {
                return;
            };
            if let Some(cancel) = entry.cancels.get_mut(index).and_then(Option::take) {
                cancel.disarm();
            }
            entry.pending -= 1;
            // Chromium: a failed import of an imported sheet still loads it.
            entry.failed |= !ok && depth == 1;
        }
        if let Some(r) = result.as_ref().ok().filter(|r| ok && r.mime.eq_ignore_ascii_case("text/css")) {
            pyro_dom::layout::provide_resource(url, &r.body);
            if depth < MAX_IMPORT_DEPTH {
                let base = url::Url::parse(&r.url).ok();
                let nested: Vec<String> = pyro_dom::cssom::leading_imports(&r.text())
                    .iter()
                    .filter_map(|href| Some(base.as_ref()?.join(href).ok()?.to_string()))
                    .collect();
                for nested in nested {
                    self.fetch_style_import(page, loader, node, generation, nested, depth + 1, &r.url);
                }
            }
        }
        if self.style_imports_settled(page, loader, node) {
            // As in Chromium, the parser goes on before the event fires.
            self.stylesheets_changed(page);
        }
    }

    /// Once none of a `<style>`'s imports is pending: queues its event and
    /// stops its render blocking. True when it blocked the parser's scripts.
    fn style_imports_settled(&mut self, page: FrameKey, loader: &LoaderId, node: NodeId) -> bool {
        let (finished, bindings) = {
            let Some(mut doc) = self.doc_mut(page) else { return false };
            let bindings = doc.main_frame.page_dom.clone();
            let sheets = &mut doc.stylesheets;
            if sheets.style_imports.get(&node).is_none_or(|e| e.pending != 0) {
                return false;
            }
            (sheets.style_imports.remove(&node), bindings)
        };
        let (Some(finished), Some(bindings)) = (finished, bindings) else { return false };
        bindings.unblock_rendering(node);
        self.queue_style_event(page, loader.clone(), &bindings, node, !finished.failed);
        finished.script_blocking
    }

    /// Fires `load` (`ok`) or `error` at a processed `<style>` in a later
    /// task, even if it is removed by then (as in Chromium). Until then it
    /// delays the document's load event.
    fn queue_style_event(&mut self, page: FrameKey, loader: LoaderId, bindings: &Rc<PageDom>, node: NodeId, ok: bool) {
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = ResourceNode { dom: bindings.dom.clone(), node };
        let weak = Rc::downgrade(bindings);
        let Some(mut doc) = self.doc_mut(page) else { return };
        doc.stylesheets.style_events += 1;
        drop(doc);
        self.scheduler().post(TaskSource::DomManipulation, move |b| {
            let Some(mut doc) = b.doc_mut(page).filter(|p| p.main_frame.loader_id == loader) else { return };
            doc.stylesheets.style_events -= 1;
            drop(doc);
            if let Some(bindings) = weak.upgrade() {
                pyro_webapi::lifecycle::resource_loaded(&bindings, hold.node, ok);
            }
            // DOMContentLoaded and the load event may have waited for it.
            b.stylesheets_changed(page);
        });
    }
}
