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
    /// The sheet is a `<link rel=stylesheet>`'s: any import that fails or is not CSS, however
    /// deep, makes its event `error` (as browsers do for links).
    link: bool,
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
    /// pyro: each style sheet response the document applied (a link's, by its node; an import's,
    /// by its request), with its URL after redirects and the `Referrer-Policy` it was served with:
    /// the policy of the requests that sheet makes (as Chrome sends them;
    /// `Browser::sheet_referrer_policy`). A sheet's next response replaces its record.
    pub(crate) responses: HashMap<SheetResponseId, SheetResponseRecord>,
}

/// pyro: which sheet a response record is of.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SheetResponseId {
    Link(NodeId),
    Import(String),
}

/// pyro: one sheet response's URL after redirects and `Referrer-Policy`.
#[derive(Clone, Debug)]
pub(crate) struct SheetResponseRecord {
    pub(crate) url: String,
    pub(crate) referrer_policy: Option<pyro_net::ReferrerPolicy>,
}

impl Stylesheets {
    /// pyro: the policy of the requests the sheet at `url` makes: its response's. The URL only
    /// finds the record (a request names its sheet by its URL); where responses at one URL say
    /// different things, none is taken for another: `no-referrer`.
    pub(crate) fn referrer_policy_of(&self, url: &str) -> Option<pyro_net::ReferrerPolicy> {
        let mut policies = self.responses.values().filter(|r| r.url == url).map(|r| r.referrer_policy);
        let first = policies.next()?;
        if policies.all(|p| p == first) { first } else { Some(pyro_net::ReferrerPolicy::NoReferrer) }
    }
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
        // pyro: its response goes with it (its policy no longer applies to anything).
        self.responses.remove(&SheetResponseId::Link(node));
    }
}
impl Page {
    /// Pending linked stylesheets and retained decoded CSS bytes. This is
    /// logical source accounting, not process memory or parsed-rule overhead.
    pub fn stylesheet_usage(&self) -> (usize, usize) {
        (self.stylesheets.pending(), self.stylesheets.bytes)
    }
}

/// The environment encodings of the sheet `node` links, in order: its
/// `charset` attribute (obsolete in HTML, read by Chromium), then its
/// document's encoding.
fn link_environment(bindings: &PageDom, node: NodeId) -> Vec<String> {
    let dom = bindings.dom.borrow();
    let charset = dom.get_attribute(node, "charset").ok().flatten().map(str::to_owned);
    charset.into_iter().chain(document_encoding(bindings)).collect()
}

/// The encoding `bindings`' document was decoded with: the environment
/// encoding of the sheets its `<style>` elements import.
fn document_encoding(bindings: &PageDom) -> Option<String> {
    let dom = bindings.dom.borrow();
    dom.document_data(bindings.document()).ok().map(|d| d.charset.to_owned())
}

fn remove_sheet(page: &PageDom, node: NodeId) {
    page.apply_style_event(&ResourceEvent { node, kind: DomResourceKind::Stylesheet, reason: ResourceReason::Removed });
}

impl Browser {
    /// `restart`: the link stopped being a stylesheet link since its sheet was requested (its
    /// `rel` lost `stylesheet`), so what it links now is requested again even if it is the same.
    pub(crate) fn stylesheet_event(&mut self, page: FrameKey, node: NodeId, restart: bool) {
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
                        crate::loader::element_referrer_policy(&dom, node),
                    ))
                })
                .flatten()
        };
        let Some((url, crossorigin, integrity, referrer_policy)) = config else {
            drop(state);
            self.doc_mut(page).expect("page").stylesheets.remove(node);
            bindings.unblock_rendering(node);
            remove_sheet(&bindings, node);
            self.queue_stylesheets_changed(page, loader);
            return;
        };
        if !restart
            && state
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
        bindings.reserve_stylesheet(node);
        let weak = Rc::downgrade(&bindings);
        let cors = crossorigin.is_some();
        let finish = move |b: &mut Browser, result: Result<Resource, String>| {
            let Some(bindings) = weak.upgrade() else { return };
            // SRI: a mismatching (or opaque) response is a network error.
            let result = result.and_then(|mut r| {
                let origin = b.doc(page).map(|p| p.main_frame.origin.clone()).unwrap_or_default();
                // pyro: its tainting, which the sheet's origin-clean flag follows (CSSOM).
                r.opaque = crate::loader::is_opaque(&r, cors, &origin);
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
            credentials: None,
            post: None,
            url,
            kind: ResourceKind::Stylesheet,
            page: page.page,
            referrer: Some(referrer),
            referrer_policy,
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
        let document_url = state.main_frame.url.clone();
        let sheets = &mut state.stylesheets;
        let Some(sheet) = sheets.entries.get_mut(&node).filter(|s| s.generation == generation && s.pending) else {
            return;
        };
        if let Some(cancel) = sheet.cancel.take() {
            cancel.disarm();
        }
        sheet.pending = false;
        let environment = link_environment(bindings, node);
        let quirks = bindings.dom.borrow().is_quirks_mode(bindings.document());
        let sheet_url = result.as_ref().map(|r| r.url.clone()).unwrap_or_default();
        // pyro: a CORS-cross-origin response's sheet is not origin-clean (CSSOM), nor (as
        // Chrome) a network error's, which is no CORS-same-origin response either.
        let opaque = result.as_ref().map_or(true, |r| r.opaque);
        // pyro: this link's response now, with its policy (replacing its previous response's).
        match result.as_ref() {
            Ok(r) => {
                let record = SheetResponseRecord { url: r.url.clone(), referrer_policy: r.referrer_policy };
                sheets.responses.insert(SheetResponseId::Link(node), record);
            }
            Err(_) => {
                sheets.responses.remove(&SheetResponseId::Link(node));
            }
        }
        let text = result
            .ok()
            .filter(|r| {
                (200..300).contains(&r.status)
                    && stylesheet_mime_ok(r, quirks, &document_url)
                    && r.body.len() <= MAX_SHEET_BYTES
            })
            .map(|r| {
                let environment: Vec<&str> = environment.iter().map(String::as_str).collect();
                pyro_dom::cssom::decode_stylesheet(&r.body, r.charset.as_deref(), &environment).0.into_owned()
            });
        let text = text.filter(|t| t.len() <= MAX_SHEET_BYTES && t.len() <= MAX_DOCUMENT_BYTES - sheets.bytes);
        let ok = text.is_some();
        // The `@import`s the sheet starts with: its event waits for them (HTML: a sheet's
        // critical subresources).
        let imports: Vec<pyro_dom::cssom::ImportRequest> = text.as_deref().map_or_else(Vec::new, |text| {
            let base = url::Url::parse(&sheet_url).ok();
            resolve_imports(pyro_dom::cssom::leading_import_requests(text), base.as_ref())
        });
        if let Some(text) = text {
            sheet.bytes = text.len();
            sheets.bytes += text.len();
            bindings.set_sheet_response(node, !opaque);
            // pyro: at its URL after redirects, which its URLs resolve against.
            bindings.set_stylesheet_response(node, &sheet_url, &text);
        } else {
            bindings.set_sheet_response(node, !opaque);
            remove_sheet(bindings, node);
        }
        drop(state);
        bindings.unblock_rendering(node);
        if !imports.is_empty() && bindings.dom.borrow_mut().add_ref(node).is_ok() {
            let hold = Rc::new(ResourceNode { dom: bindings.dom.clone(), node });
            let generation = {
                let Some(mut doc) = self.doc_mut(page) else { return };
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
                        script_blocking: false,
                        cancels: Vec::new(),
                        link: true,
                    },
                );
                generation
            };
            let environment = document_encoding(bindings).unwrap_or_default();
            for import in imports {
                self.fetch_style_import(page, loader, node, generation, import, 1, &sheet_url, &environment);
            }
            self.style_imports_settled(page, loader, node);
            self.stylesheets_changed(page);
            return;
        }
        // This callback is already a later owner-thread task. No DOM/state
        // borrow survives dispatch into JavaScript.
        pyro_webapi::lifecycle::resource_loaded(bindings, node, ok);
        self.stylesheets_changed(page);
    }

    fn stylesheet_error_task(&self, page: FrameKey, loader: LoaderId, bindings: Rc<PageDom>, node: NodeId) {
        // pyro: a rejected link (at the sheet limit) has a new, failed response too: a sheet object
        // got before is detached (`PageDom::set_sheet_response`, the one place generations advance).
        bindings.set_sheet_response(node, false);
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
                let urls = resolve_imports(
                    pyro_dom::cssom::leading_import_requests(&dom.child_text_content(node)),
                    base.as_ref(),
                );
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
                    link: false,
                },
            );
            generation
        };
        if let Some(blocker) = blocker {
            bindings.block_rendering(node, blocker);
        }
        let referrer = self.doc(page).map(|p| p.main_frame.url.clone()).unwrap_or_default();
        let environment = document_encoding(&bindings).unwrap_or_default();
        for import in urls {
            self.fetch_style_import(page, &loader, node, generation, import, 1, &referrer, &environment);
        }
        self.style_imports_settled(page, &loader, node);
        true
    }

    /// Fetches one sheet a `<style>`'s sheet imports, `depth` imports deep;
    /// `environment` is the importing sheet's encoding.
    #[allow(clippy::too_many_arguments)]
    fn fetch_style_import(
        &mut self,
        page: FrameKey,
        loader: &LoaderId,
        node: NodeId,
        generation: u64,
        import: pyro_dom::cssom::ImportRequest,
        depth: usize,
        referrer: &str,
        environment: &str,
    ) {
        let url = import.url.clone();
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
        let environment = environment.to_owned();
        // pyro: a modified import's request URL modifiers (CSS Values 5 § 4.5.4): its mode, its
        // integrity check (a response failing it is a failed import) and its referrer policy.
        let cors = import.cross_origin.is_some();
        let integrity = import.integrity.clone();
        let modified = import.is_modified();
        let request_key = pyro_dom::cssom::import_key(&url, &import);
        let document_origin = self.doc(page).map(|p| p.main_frame.origin.clone()).unwrap_or_else(|| "null".into());
        // pyro: the layout body key of the request as sent (set below, once it is built).
        let layout_key: Rc<std::cell::RefCell<Option<pyro_dom::layout::LayoutBodyKey>>> = Rc::default();
        let finish_layout_key = layout_key.clone();
        let finish = move |b: &mut Browser, result: Result<Resource, String>| {
            let layout_key = finish_layout_key.borrow_mut().take();
            let result = result.and_then(|mut r| {
                // pyro: its tainting, which the sheet's origin-clean flag follows (CSSOM).
                r.opaque = crate::loader::is_opaque(&r, cors, &document_origin);
                match crate::loader::integrity_error(&r, integrity.as_deref(), cors, &document_origin) {
                    Some(message) => Err(message),
                    None => Ok(r),
                }
            });
            b.style_import_done(
                page,
                &finish_loader,
                node,
                generation,
                index,
                depth,
                &finish_url,
                &environment,
                result,
                &request_key,
                layout_key,
            );
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let Some(frame) = self.doc(page).map(|p| p.main_frame.clone()) else { return };
        // The parent sheet is the import's referrer, whatever its mode (as Blink sends a style
        // sheet's requests).
        let referrer = referrer.to_owned();
        let sheet_policy = self.sheet_referrer_policy(page, &referrer);
        let request = FetchRequest {
            credentials: None,
            post: None,
            url,
            kind: ResourceKind::Stylesheet,
            page: page.page,
            referrer: Some(referrer),
            // (Else the parent sheet's `Referrer-Policy`, as Chrome sends a sheet's requests.)
            referrer_policy: import
                .referrer_policy
                .as_deref()
                .and_then(pyro_net::ReferrerPolicy::parse)
                .or(sheet_policy),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: import.cross_origin.clone(),
            initiator: Some(pyro_webapi::resource_timing::Initiator::Css),
            module: None,
            part: None,
        };
        *layout_key.borrow_mut() = Some(crate::layout_key_of(&frame, &request));
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
        environment: &str,
        result: Result<Resource, String>,
        request_key: &str,
        // pyro: the layout body key of the request as sent (`layout_key_of`).
        layout_key: Option<pyro_dom::layout::LayoutBodyKey>,
    ) {
        let modified = request_key != url;
        let ok = result.as_ref().is_ok_and(|r| (200..300).contains(&r.status));
        {
            let Some(mut doc) = self.doc_mut(page).filter(|p| &p.main_frame.loader_id == loader) else { return };
            let quirks = doc.main_frame.page_dom.as_ref().is_some_and(|b| b.dom.borrow().is_quirks_mode(b.document()));
            let document_url = doc.main_frame.url.clone();
            let Some(entry) = doc.stylesheets.style_imports.get_mut(&node).filter(|e| e.generation == generation)
            else {
                return;
            };
            if let Some(cancel) = entry.cancels.get_mut(index).and_then(Option::take) {
                cancel.disarm();
            }
            entry.pending -= 1;
            // Chromium: a failed import of an imported sheet still loads a `<style>`; a link's
            // fails with any import that fails or is not CSS.
            let css = result.as_ref().is_ok_and(|r| stylesheet_mime_ok(r, quirks, &document_url));
            // A `<style>`'s own import also fails on a type that is not CSS (a text/plain sheet).
            entry.failed |= if entry.link { !ok || !css } else { (!ok || !css) && depth == 1 };
        }
        if let Some(r) = result.as_ref().ok().filter(|r| ok && r.mime.eq_ignore_ascii_case("text/css")) {
            if let Some(mut doc) = self.doc_mut(page) {
                let record = SheetResponseRecord { url: r.url.clone(), referrer_policy: r.referrer_policy };
                doc.stylesheets.responses.insert(SheetResponseId::Import(request_key.to_owned()), record);
            }
            let (text, encoding) = pyro_dom::cssom::decode_stylesheet(&r.body, r.charset.as_deref(), &[environment]);
            // CSSOM: `CSSImportRule.styleSheet` lists the imported rules.
            if let Some(bindings) = self.doc(page).and_then(|d| d.main_frame.page_dom.clone()) {
                // pyro: under its request (URL and modifiers), at its URL after redirects.
                bindings.provide_imported_sheet(request_key, &r.url, &text, r.opaque);
            }
            // The page's layouts take this sheet rather than fetching it
            // again (they read `file:` and `data:` URLs themselves), decoded
            // (they cannot know its environment encoding).
            // (Not a modified import's: the layouts key its request apart, by its modifiers.)
            if let Some(layout_key) = layout_key.filter(|_| url.starts_with("http")) {
                self.provide_layout_body(page, layout_key, pyro_dom::layout::LayoutBody {
                    bytes: std::sync::Arc::new(pyro_dom::cssom::stylesheet_utf8_bytes(&text)),
                    final_url: r.url.clone(),
                    opaque: r.opaque,
                    status: r.status,
                });
            }
            if depth < MAX_IMPORT_DEPTH {
                let base = url::Url::parse(&r.url).ok();
                let nested = resolve_imports(pyro_dom::cssom::leading_import_requests(&text), base.as_ref());
                for nested in nested {
                    self.fetch_style_import(page, loader, node, generation, nested, depth + 1, &r.url, encoding);
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

/// Whether a `<link rel=stylesheet>` response may be a style sheet (HTML "process the linked
/// resource", as browsers apply it): `text/css`; with `X-Content-Type-Options: nosniff` only
/// that. Without nosniff, a missing or unparsable Content-Type is taken as CSS, and another
/// valid type only by a quirks-mode document for a same-origin sheet.
fn stylesheet_mime_ok(r: &Resource, quirks: bool, document_url: &str) -> bool {
    let mime = r.mime.trim();
    if mime.eq_ignore_ascii_case("text/css") {
        return true;
    }
    if r.nosniff {
        return false;
    }
    let valid = mime.split_once('/').is_some_and(|(t, s)| !t.trim().is_empty() && !s.trim().is_empty());
    if !valid {
        return true;
    }
    let origin = |u: &str| url::Url::parse(u).ok().map(|u| u.origin());
    quirks && origin(&r.url).is_some_and(|o| Some(o) == origin(document_url))
}

/// pyro: `imports` with their URLs resolved against `base` (those that do not resolve dropped).
fn resolve_imports(
    imports: Vec<pyro_dom::cssom::ImportRequest>,
    base: Option<&url::Url>,
) -> Vec<pyro_dom::cssom::ImportRequest> {
    imports
        .into_iter()
        .filter_map(|mut import| {
            import.url = base?.join(&import.url).ok()?.to_string();
            Some(import)
        })
        .collect()
}
