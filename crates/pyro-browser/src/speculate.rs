//! Speculative preloading (HTML "speculative HTML parsing"): when the
//! parser stops at a parser-blocking script (its fetch, or the stylesheets
//! it waits for), the markup it has not reached is scanned
//! ([`pyro_dom::prescan`]) and the scripts, stylesheets, preload links and
//! images found there are requested at once, so their round trips overlap
//! the script's instead of following it. The responses are kept per
//! document, keyed by what the element's request would be (URL, kind, CORS
//! attributes); when the parser reaches the element, its fetch takes the
//! speculative one — pending (it waits for that response) or complete (the
//! response is delivered as a task) — and nothing is requested twice, with
//! or without an HTTP cache. A request no element made by the end of
//! parsing is cancelled; a navigation cancels them all with the document.
//!
//! What a scripts writes (`document.write`) goes in front of the remaining
//! input and is scanned when the parser next blocks; the scan never undoes
//! a request, so markup a write made unreachable may have been requested in
//! vain (as in other browsers). A `<base href>` in scanned markup resolves
//! the URLs after it when the document has none yet; a `<meta
//! name=referrer>` stops the scan, since requests after it would carry
//! another policy. Nonces (CSP is not enforced) and `integrity` do not
//! change a request; `crossorigin` (CORS mode and credentials) is part of
//! the key.
//!
//! A speculative request is made when the scan finds the element, so it
//! carries the cookies of that time: a cookie a script before the element
//! sets (`document.cookie`, or a response the script fetches) is not sent
//! with it, nor is a referrer policy a script changes applied. Chrome's
//! preload scanner behaves the same (a script setting a cookie the server
//! reads for the next request of the document sees the difference).
//!
//! Each speculative fetch serves one element. The first element whose
//! request matches takes it; while it is pending, its slot leaves the map,
//! so a second element with the same URL (an image repeated in a list, a
//! stylesheet included twice) makes its own request, as without
//! speculation, and an element that drops its claim (its `src` changed, it
//! was removed) cancels that fetch without leaving anything behind for a
//! later element to wait on.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};

use pyro_dom::prescan::{Prescan, Prescanned};
use pyro_dom::NodeId;
use pyro_js::ModuleType;
use pyro_runtime::TaskSource;

use crate::frames::FrameKey;
use crate::loader::{FetchDone, ResourceCancellation, ResourceLoader};
use crate::loader::{FetchRequest, Resource, ResourceKind};
use crate::{Browser, LoaderId};

/// Unclaimed speculative fetches in flight at most. The per-origin
/// connection limit is the network client's; this bounds how far ahead of
/// the parser the document's requests queue, so a request a script makes
/// waits behind at most this many.
const MAX_IN_FLIGHT: usize = 16;
/// Speculative fetches per document at most.
const MAX_SPECULATED: usize = 1024;

#[derive(Default)]
pub(crate) struct Speculation {
    scanner: Prescan,
    /// Bytes at the back of the parser's input the scan has covered, and
    /// the parser's appended count at that time (what is new since is at
    /// the front, written by a script, or past that count at the back).
    tail_scanned: usize,
    seen_appended: usize,
    /// A scanned `<base href>`, resolved: scanned URLs after it resolve
    /// against it.
    base: Option<String>,
    /// By URL (one speculative request per URL: the first found).
    entries: HashMap<String, Entry>,
    /// Requests not started yet, in document order.
    queue: VecDeque<FetchRequest>,
    /// Pending fetches an element claimed (out of `entries`): kept here so
    /// the document owns them (dropping it cancels their transport) until
    /// they complete; one its element cancelled stays (inert, at most
    /// `MAX_SPECULATED`) until the document goes.
    claimed: Vec<Rc<RefCell<Slot>>>,
    /// Started and not claimed by an element.
    in_flight: usize,
    speculated: usize,
}

/// What, besides the URL, makes two document requests interchangeable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Shape {
    url: String,
    frame: Option<String>,
    loader: Option<String>,
    kind: ResourceKind,
    cors: bool,
    credentials: pyro_net::Credentials,
    pub(crate) referrer_source: Option<url::Url>,
    pub(crate) referer: Option<String>,
    pub(crate) policy: pyro_net::ReferrerPolicy,
    module: Option<ModuleType>,
}

impl Shape {
    pub(crate) fn of(browser: &Browser, request: &FetchRequest) -> Option<Shape> {
        if request.post.is_some() || request.part.is_some() {
            return None;
        }
        let chain = crate::loader::request_referrer(browser, request);
        let referer = url::Url::parse(&request.url).ok().and_then(|url| chain.header(&url));
        Some(Shape {
            url: request.url.clone(),
            frame: request.frame_id.clone(),
            loader: request.loader_id.clone(),
            kind: request.kind,
            cors: request.kind == ResourceKind::ModuleScript || request.crossorigin.is_some(),
            credentials: request.effective_credentials(),
            referer,
            referrer_source: chain.referrer,
            policy: chain.policy,
            module: request.module,
        })
    }
}

enum Entry {
    /// In the queue.
    Queued,
    Started(Shape, Rc<RefCell<Slot>>),
}

#[derive(Default)]
struct Slot {
    result: Option<Result<Resource, String>>,
    /// The element's completion, once its request claimed a pending fetch.
    waiter: Option<FetchDone>,
    cancel: Option<ResourceCancellation>,
    /// An element took it while pending: it is in `claimed`, not `entries`,
    /// and not counted in `in_flight`.
    claimed: bool,
}

enum Claim {
    /// Nothing speculated for it.
    Fetch,
    Done(Result<Resource, String>),
    Pending(Rc<RefCell<Slot>>),
}

impl Speculation {
    /// Parsing ended: what no element claimed is dropped (an in-flight
    /// request is cancelled); claimed fetches complete for their elements.
    pub(crate) fn end_of_parsing(&mut self) {
        self.queue.clear();
        self.entries.clear();
        self.in_flight = 0;
    }

    /// An unclaimed fetch completed or was claimed.
    fn release_in_flight(&mut self) {
        debug_assert!(self.in_flight > 0, "speculative in-flight count underflow");
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    fn add(&mut self, request: FetchRequest) {
        if self.speculated >= MAX_SPECULATED || self.entries.contains_key(&request.url) {
            return;
        }
        self.speculated += 1;
        self.entries.insert(request.url.clone(), Entry::Queued);
        self.queue.push_back(request);
    }
}

/// The document fields a speculative request copies from the element's.
struct Requester {
    page: crate::PageId,
    frame_id: String,
    loader_id: String,
    url: String,
    base_url: String,
}

impl Requester {
    fn request(
        &self,
        url: String,
        kind: ResourceKind,
        crossorigin: Option<String>,
        module: Option<ModuleType>,
    ) -> FetchRequest {
        use pyro_webapi::resource_timing::Initiator;
        // Scripts send the document URL as referrer, the others the base
        // URL (as `fetch_script`, `stylesheet_event` and `image_event` do).
        let (referrer, initiator) = match kind {
            ResourceKind::Script | ResourceKind::ModuleScript => (self.url.clone(), Initiator::Script),
            ResourceKind::Image => (self.base_url.clone(), Initiator::Img),
            _ => (self.base_url.clone(), Initiator::Link),
        };
        FetchRequest {
            csp: None,
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind,
            page: self.page,
            referrer: Some(referrer),
            referrer_policy: None,
            frame_id: Some(self.frame_id.clone()),
            loader_id: Some(self.loader_id.clone()),
            crossorigin,
            initiator: Some(initiator),
            module,
            part: None,
        }
    }
}

impl Browser {
    /// The parser stopped at a blocking script: scans the input it has not
    /// reached for the document's next requests and starts them.
    pub(crate) fn speculate_ahead(&mut self, page: FrameKey) {
        let requests = {
            let Some(mut load) = self.load_mut(page) else { return };
            let load = &mut *load;
            let spec = &mut load.speculation;
            if spec.scanner.stopped() || spec.speculated >= MAX_SPECULATED {
                return;
            }
            let Some(parser) = load.parser.as_mut() else { return };
            if parser.in_insertion_point() {
                return;
            }
            let appended = parser.appended_input();
            let back_new = appended.saturating_sub(spec.seen_appended);
            // What remains of the input the last scan covered: at most what
            // it was, and at most what was behind each write since (the
            // parser consumes written input before it).
            let mut scanned_left = spec.tail_scanned;
            if let Some(mark) = parser.take_written_low_mark() {
                let behind = (mark + spec.seen_appended as isize).max(0) as usize;
                scanned_left = scanned_left.min(behind);
            }
            let seen = &mut spec.seen_appended;
            let parser = &*parser;
            let texts = parser.unconsumed_input(|total, chunks| {
                *seen = appended;
                // Front to back: [written since][scanned][appended since].
                let old = total.saturating_sub(back_new);
                let front_new = old.saturating_sub(scanned_left);
                (copy_range(chunks, 0, front_new), copy_range(chunks, old, total), total)
            });
            let Some((front, back, total)) = texts else { return };
            spec.tail_scanned = total;
            if front.is_empty() && back.is_empty() {
                return;
            }
            let mut items = Vec::new();
            for text in [front, back] {
                if !text.is_empty() {
                    spec.scanner.scan(&text, |item| items.push(item));
                }
            }
            if items.is_empty() {
                return;
            }
            let (fallback, document_base) = {
                let dom = load.dom.borrow();
                let doc = parser.document();
                (
                    dom.document_fallback_base_url(doc).unwrap_or_default().to_owned(),
                    dom.document_base_url(doc).unwrap_or_default().to_owned(),
                )
            };
            // The base URL, parsed once: the document's (its first <base
            // href>, else the fallback) or the one a scanned <base> set.
            let mut base = url::Url::parse(spec.base.as_deref().unwrap_or(&document_base)).ok();
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Prescanned::Base { href } => {
                        // The document's first <base href> wins; one it already has stays.
                        if spec.base.is_none() && document_base == fallback {
                            let new_base = url::Url::parse(&fallback)
                                .ok()
                                .and_then(|f| f.join(&href).ok())
                                .filter(|u| !matches!(u.scheme(), "data" | "javascript"));
                            if let Some(new_base) = new_base {
                                spec.base = Some(new_base.to_string());
                                base = Some(new_base);
                            }
                        }
                    }
                    Prescanned::ReferrerPolicy => break,
                    item => {
                        let href = match &item {
                            Prescanned::Script { src, .. } | Prescanned::Image { src } => src.trim(),
                            Prescanned::Stylesheet { href, .. } | Prescanned::Preload { href, .. } => href.trim(),
                            _ => unreachable!("handled above"),
                        };
                        if let Some(url) = base.as_ref().and_then(|b| b.join(href).ok()) {
                            resolved.push((url.to_string(), item));
                        }
                    }
                }
            }
            resolved
        };
        if requests.is_empty() {
            return;
        }
        let Some(requester) = self.requester(page) else { return };
        let requests = requests
            .into_iter()
            .filter_map(|(url, item)| match item {
                Prescanned::Script { module: true, crossorigin, .. } => {
                    Some(requester.request(url, ResourceKind::ModuleScript, crossorigin, Some(ModuleType::JavaScript)))
                }
                Prescanned::Script { crossorigin, .. } => {
                    Some(requester.request(url, ResourceKind::Script, crossorigin, None))
                }
                Prescanned::Stylesheet { crossorigin, .. } => {
                    Some(requester.request(url, ResourceKind::Stylesheet, crossorigin, None))
                }
                Prescanned::Preload { as_, crossorigin, .. } => {
                    // As `preload_event` maps `as`.
                    let kind = match as_.as_deref().map(str::to_ascii_lowercase).as_deref() {
                        Some("script") => ResourceKind::Script,
                        Some("style") => ResourceKind::Stylesheet,
                        _ => ResourceKind::Image,
                    };
                    Some(requester.request(url, kind, crossorigin, None))
                }
                Prescanned::Image { .. } if url.starts_with("blob:") => None,
                Prescanned::Image { .. } => Some(requester.request(url, ResourceKind::Image, None, None)),
                Prescanned::Base { .. } | Prescanned::ReferrerPolicy => None,
            })
            .collect();
        self.speculate(page, requests);
    }

    /// A parser-blocking external script waits for stylesheets before it
    /// is prepared: its fetch starts now, as the element's will take it.
    pub(crate) fn speculate_script(&mut self, page: FrameKey, node: NodeId) {
        let Some(requester) = self.requester(page) else { return };
        let request = {
            let Some(load) = self.doc(page).and_then(|p| p.filter_map(|p| p.load.as_ref())) else { return };
            let dom = load.dom.borrow();
            let Ok(info) = dom.script_info(node) else { return };
            // As `prepare_script` classifies it: only a classic script that
            // is not `nomodule` is fetched.
            if info.kind != pyro_dom::script::ScriptKind::Classic || info.nomodule || info.already_started {
                return;
            }
            let Some(src) = info.src.as_deref() else { return };
            let base = crate::loader::node_base_url(&dom, node).unwrap_or_else(|| requester.base_url.clone());
            let Some(url) = crate::loader::resolve_url(&base, src) else { return };
            let mut request = requester.request(url, ResourceKind::Script, info.crossorigin.clone(), None);
            request.parser_priority = true;
            // As the element's own request will be sent (it takes this one).
            request.referrer_policy = pyro_net::ReferrerPolicy::from_attribute(info.referrerpolicy.as_deref());
            request
        };
        self.speculate(page, vec![request]);
    }

    fn requester(&self, page: FrameKey) -> Option<Requester> {
        let state = self.doc(page)?;
        let frame = &state.main_frame;
        Some(Requester {
            page: page.page,
            frame_id: frame.id.0.clone(),
            loader_id: frame.loader_id.0.clone(),
            url: frame.url.clone(),
            base_url: frame.base_url.clone(),
        })
    }

    fn speculate(&mut self, page: FrameKey, requests: Vec<FetchRequest>) {
        if requests.is_empty() {
            return;
        }
        // CSP: a request whose destination a policy of the document governs is not speculated.
        // The scanner knows neither the element's nonce nor where a redirect goes, and a
        // speculative result an element claims would skip the element's own check and its
        // violations (or block what the element's nonce allows). Its element fetches it, checked.
        let list = self
            .doc(page)
            .and_then(|p| p.main_frame.page_dom.clone())
            .and_then(|bindings| pyro_webapi::csp::policy_list(&bindings));
        let requests: Vec<FetchRequest> = match list {
            None => requests,
            Some(list) => requests
                .into_iter()
                .filter(|request| {
                    let Some(dest) = crate::loader::csp_dest(request) else { return true };
                    let Some(effective) = pyro_net::csp::effective_directive(dest) else { return true };
                    !list.policies.iter().any(|policy| policy.directive_for(effective).is_some())
                })
                .collect(),
        };
        if requests.is_empty() {
            return;
        }
        {
            let Some(mut load) = self.load_mut(page) else { return };
            for request in requests {
                load.speculation.add(request);
            }
        }
        self.pump_speculation(page);
    }

    /// Whether `page` still loads the document `loader_id` names.
    fn loading(&self, page: FrameKey, loader_id: Option<&LoaderId>) -> bool {
        self.doc(page).is_some_and(|d| d.load.as_ref().map(|l| &l.loader_id) == loader_id)
    }

    /// Starts queued speculative fetches up to the in-flight limit.
    fn pump_speculation(&mut self, page: FrameKey) {
        let Some(loader) = self.loader_for(page) else { return };
        let loader_id = self.current_loader(page);
        loop {
            let request = {
                let Some(mut load) = self.load_mut(page) else { return };
                let spec = &mut load.speculation;
                if spec.in_flight >= MAX_IN_FLIGHT {
                    return;
                }
                let Some(request) = spec.queue.pop_front() else { return };
                request
            };
            let shape = Shape::of(self, &request).expect("no body, no part");
            let slot = {
                let Some(mut load) = self.load_mut(page) else { return };
                let spec = &mut load.speculation;
                match spec.entries.get_mut(&request.url) {
                    Some(entry @ Entry::Queued) => {
                        let slot = Rc::new(RefCell::new(Slot::default()));
                        *entry = Entry::Started(shape, slot.clone());
                        spec.in_flight += 1;
                        slot
                    }
                    // Claimed before it started: the element fetches it.
                    _ => continue,
                }
            };
            let weak = Rc::downgrade(&slot);
            let loader_id = loader_id.clone();
            let url = request.url.clone();
            let cancel = loader.fetch_cancellable(
                self,
                request,
                Box::new(move |b, result| b.speculation_done(page, loader_id, url, weak, result)),
            );
            slot.borrow_mut().cancel = cancel;
        }
    }

    fn speculation_done(
        &mut self,
        page: FrameKey,
        loader_id: Option<LoaderId>,
        url: String,
        slot: Weak<RefCell<Slot>>,
        result: Result<Resource, String>,
    ) {
        // Gone with its document.
        let Some(slot) = slot.upgrade() else { return };
        let (claimed, waiter) = {
            let mut s = slot.borrow_mut();
            if let Some(cancel) = s.cancel.take() {
                cancel.disarm();
            }
            (s.claimed, s.waiter.take())
        };
        if self.loading(page, loader_id.as_ref()) {
            if let Some(mut load) = self.load_mut(page) {
                let spec = &mut load.speculation;
                if claimed {
                    spec.claimed.retain(|s| !Rc::ptr_eq(s, &slot));
                } else if matches!(spec.entries.get(&url), Some(Entry::Started(_, s)) if Rc::ptr_eq(s, &slot)) {
                    // Still unclaimed in the map (parsing has not ended).
                    spec.release_in_flight();
                }
            }
        }
        match (claimed, waiter) {
            (true, Some(waiter)) => waiter(self, result),
            // The claimant cancelled; the response is dropped.
            (true, None) => {}
            (false, _) => slot.borrow_mut().result = Some(result),
        }
        if !claimed {
            self.pump_speculation(page);
        }
    }

    fn claim_speculated(&mut self, page: FrameKey, request: &FetchRequest) -> Claim {
        let shape = Shape::of(self, request);
        let Some(mut load) = self.load_mut(page) else { return Claim::Fetch };
        let spec = &mut load.speculation;
        if spec.entries.is_empty() {
            return Claim::Fetch;
        }
        match spec.entries.get(&request.url) {
            None => Claim::Fetch,
            Some(Entry::Queued) => {
                // Not started: the element's own fetch goes (the queue skips it).
                spec.entries.remove(&request.url);
                Claim::Fetch
            }
            // Another shape of request for the URL: its own fetch.
            Some(Entry::Started(dispatched, _)) if Some(dispatched) != shape.as_ref() => Claim::Fetch,
            Some(Entry::Started(_, slot)) => {
                let slot = slot.clone();
                let result = slot.borrow_mut().result.take();
                match result {
                    Some(result) => {
                        spec.entries.remove(&request.url);
                        Claim::Done(result)
                    }
                    None => {
                        // Out of the map: a later request for the URL
                        // fetches on its own.
                        spec.entries.remove(&request.url);
                        slot.borrow_mut().claimed = true;
                        spec.claimed.push(slot.clone());
                        spec.release_in_flight();
                        Claim::Pending(slot)
                    }
                }
            }
        }
    }

    /// Fetches a resource of `page`'s document through `loader`, as
    /// `loader.fetch_cancellable` would, taking the speculative fetch of the
    /// same request first. `done` runs as a later task either way; dropping
    /// the returned handle cancels transport.
    pub(crate) fn fetch_document_resource(
        &mut self,
        page: FrameKey,
        loader: &Rc<dyn ResourceLoader>,
        request: FetchRequest,
        done: FetchDone,
    ) -> Option<ResourceCancellation> {
        // A prefetch never takes a speculative fetch (the prescanner never starts one).
        if request.kind == ResourceKind::Prefetch {
            return loader.fetch_cancellable(self, request, done);
        }
        match self.claim_speculated(page, &request) {
            Claim::Fetch => loader.fetch_cancellable(self, request, done),
            Claim::Done(result) => {
                self.scheduler().post(TaskSource::Networking, move |b| done(b, result));
                None
            }
            Claim::Pending(slot) => {
                slot.borrow_mut().waiter = Some(done);
                self.pump_speculation(page);
                let weak = Rc::downgrade(&slot);
                // Dropping the handle cancels the transport (the slot stays in
                // `claimed`, unreachable by URL, until the document goes).
                Some(ResourceCancellation::new(move || {
                    if let Some(slot) = weak.upgrade() {
                        let cancel = {
                            let mut s = slot.borrow_mut();
                            s.waiter = None;
                            s.cancel.take()
                        };
                        drop(cancel);
                    }
                }))
            }
        }
    }
}

/// Bytes `from..to` of the concatenated chunks.
fn copy_range(chunks: &[&str], from: usize, to: usize) -> String {
    let mut out = String::with_capacity(to.saturating_sub(from));
    let mut offset = 0;
    for text in chunks {
        let (start, end) = (offset, offset + text.len());
        offset = end;
        if end <= from {
            continue;
        }
        if start >= to {
            break;
        }
        let lo = from.saturating_sub(start);
        let hi = (to - start).min(text.len());
        if let Some(part) = text.get(lo..hi) {
            out.push_str(part);
        }
    }
    out
}
