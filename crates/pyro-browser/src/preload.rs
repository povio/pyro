//! `<link rel=modulepreload>` and `<link rel=preload>`: the resource is
//! fetched through the page's loader (so it lands in the context's HTTP
//! cache for the import or request that follows) and the link gets `load`
//! or `error`. Vite's preload helper awaits those events before running a
//! dynamic `import()`; without them a lazily loaded chunk never loads.
//!
//! A modulepreload also keeps the module's source per document, the way
//! Chromium's preload fills the module map: an `import()` (or a second
//! modulepreload) of that URL takes it from there instead of requesting it
//! again, waiting for the preload when it is still in flight. The import
//! takes the source itself: the engine's module map has the module from
//! then on, so the document keeps no second copy (a mid-size SPA's chunks
//! are megabytes of text), only a marker that answers a later modulepreload
//! of the URL without a request. `rel=preload as=script` stays a plain
//! fetch into the HTTP cache.
//!
//! `<link rel=prefetch>` is a low-priority fetch into the HTTP cache too (destination "",
//! initiator "prefetch": CSP checks it against `default-src`, or any fetch directive that allows
//! it, as Chrome), with the link's nonce and CORS mode. The link gets `load` for any response
//! (a 404 too, as Chrome) and `error` for a network error or a CSP block; a blocked prefetch
//! still gets a Resource Timing entry, with nothing transferred, as Chrome's. One fetch per URL
//! per document: a link of a URL already prefetched (or prefetching) takes that fetch's outcome.

use std::collections::HashMap;
use std::rc::Rc;

use pyro_dom::NodeId;
use pyro_runtime::TaskSource;

use crate::frames::FrameKey;
use crate::loader::ScriptFetch;
use crate::{Browser, FetchRequest, ResourceKind};

/// Links fetched (or being fetched) per document: node → URL.
#[derive(Default)]
pub(crate) struct Preloads {
    entries: HashMap<NodeId, String>,
    /// Module sources fetched (or being fetched) by preload or import: URL → source.
    modules: HashMap<String, ModuleEntry>,
    /// The fetch options of scripts for the modules they import (HTML "descendant script fetch
    /// options"), by script URL or inline module script identity.
    module_options: HashMap<String, ModuleFetchOptions>,
    /// The nonce of the document's inline scripts, for the `import()`s the engine reports with
    /// the document (or its base) URL as their referrer: an inline classic or module script's,
    /// and a string compilation's (`eval`, `Function`, a `setTimeout` string). They can only be
    /// attributed to the inline scripts as a whole ([`InlineNonce`]).
    inline_nonce: InlineNonce,
    /// `<link rel=prefetch>` fetches by URL.
    prefetches: HashMap<String, PrefetchEntry>,
    /// Compression dictionary fetches waiting for the document's load event.
    dictionaries: Vec<DeferredFetch>,
}

/// A fetch started once the document has loaded.
type DeferredFetch = Box<dyn FnOnce(&mut Browser)>;

/// The link-values (RFC 8288) of the `Link` header `values`: each target as written, with the
/// text of its parameters (`; rel=preload; as=script`).
fn link_values<'a>(values: impl Iterator<Item = &'a str>) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    for value in values {
        let mut rest = value;
        loop {
            rest = rest.trim_start_matches(|c: char| c == ',' || c.is_ascii_whitespace());
            let Some(after) = rest.strip_prefix('<') else { break };
            let Some(end) = after.find('>') else { break };
            let target = &after[..end];
            rest = &after[end + 1..];
            // The link's parameters, up to the next link-value (a quoted `,` stays in its value).
            let mut quoted = false;
            let split = rest.char_indices().find(|&(_, c)| {
                if c == '"' {
                    quoted = !quoted;
                }
                c == ',' && !quoted
            });
            let (params, next) = match split {
                Some((i, _)) => (&rest[..i], &rest[i..]),
                None => (rest, ""),
            };
            rest = next;
            out.push((target, params));
        }
    }
    out
}

/// The value of parameter `name` among a link's `params` (unquoted), the first one.
fn link_param<'a>(params: &'a str, name: &str) -> Option<&'a str> {
    params.split(';').find_map(|p| {
        let (n, v) = p.split_once('=')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim().trim_matches('"'))
    })
}

/// The resolved URLs of a document response's `Link` header values with the
/// `compression-dictionary` relation (RFC 8288 link-values; others are skipped).
pub(crate) fn compression_dictionary_links<'a>(values: impl Iterator<Item = &'a str>, base: &url::Url) -> Vec<String> {
    let mut out = Vec::new();
    for (target, params) in link_values(values) {
        let dictionary = link_param(params, "rel")
            .is_some_and(|rel| rel.split_ascii_whitespace().any(|r| r.eq_ignore_ascii_case("compression-dictionary")));
        if dictionary {
            if let Ok(url) = base.join(target.trim()) {
                out.push(url.to_string());
            }
        }
    }
    out
}

/// A `Link` response header's `preload`, `modulepreload` or `prefetch` link: the fetch the
/// document starts before it parses anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkFetch {
    pub url: String,
    pub kind: ResourceKind,
    pub crossorigin: Option<String>,
}

/// The fetches the `Link` header `values` of a document response (from `base`) ask for, in order
/// (`as=` picks a preload's destination as it does on a `<link>`). A link of several relations
/// is fetched once, as a module preload before a preload before a prefetch.
pub(crate) fn link_header_fetches<'a>(values: impl Iterator<Item = &'a str>, base: &url::Url) -> Vec<LinkFetch> {
    let mut out = Vec::new();
    for (target, params) in link_values(values) {
        let rel = link_param(params, "rel").unwrap_or_default();
        let has = |name: &str| rel.split_ascii_whitespace().any(|r| r.eq_ignore_ascii_case(name));
        let kind = if has("modulepreload") {
            ResourceKind::ModuleScript
        } else if has("preload") {
            match link_param(params, "as").map(str::to_ascii_lowercase).as_deref() {
                Some("script") => ResourceKind::Script,
                Some("style") => ResourceKind::Stylesheet,
                Some("font") => ResourceKind::Font,
                Some("track") => ResourceKind::Track,
                Some("fetch") => ResourceKind::Fetch,
                _ => ResourceKind::Image,
            }
        } else if has("prefetch") {
            ResourceKind::Prefetch
        } else {
            continue;
        };
        let Ok(url) = base.join(target.trim()) else { continue };
        let crossorigin = link_param(params, "crossorigin").map(str::to_owned);
        out.push(LinkFetch { url: url.to_string(), kind, crossorigin });
    }
    out
}

/// Called with whether a prefetch succeeded (`load`) or not (`error`).
type PrefetchWaiter = Box<dyn FnOnce(&mut Browser, bool)>;

enum PrefetchEntry {
    /// In flight: the links waiting for its outcome.
    Pending(Vec<PrefetchWaiter>),
    /// Fetched: a later link of the URL loads at once. A failure is not kept (the next link
    /// fetches again).
    Done,
}

/// The one nonce of a document's inline scripts that ran (CSP): set by the first non-empty one,
/// cleared for good by a later different one (fail closed: no inline script vouches for an
/// `import()` then).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum InlineNonce {
    #[default]
    Unset,
    Set(String),
    Conflicting,
}

impl InlineNonce {
    /// An inline script with `nonce` (its element's, read from it; empty: none) runs.
    pub(crate) fn note(&mut self, nonce: &str) {
        if nonce.is_empty() {
            return;
        }
        match self {
            InlineNonce::Unset => *self = InlineNonce::Set(nonce.to_owned()),
            InlineNonce::Set(known) if known != nonce => *self = InlineNonce::Conflicting,
            InlineNonce::Set(_) | InlineNonce::Conflicting => {}
        }
    }

    /// The nonce, while every nonced inline script that ran had it.
    pub(crate) fn get(&self) -> Option<&str> {
        match self {
            InlineNonce::Set(nonce) => Some(nonce),
            InlineNonce::Unset | InlineNonce::Conflicting => None,
        }
    }
}

/// What a script hands the module fetches it starts (its static imports, `import()`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModuleFetchOptions {
    /// `None`: the document's policy.
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
    /// `None`: "same-origin".
    pub credentials: Option<pyro_net::Credentials>,
    /// The cryptographic nonce of the script element the options came from (HTML "descendant
    /// script fetch options"): CSP checks the imports with it. Only a script element's options
    /// set it, read from the element; `None` for anything else (an event handler's or a
    /// `javascript:` URL's `import()` has no script element), and when two elements registered
    /// options under one key with different nonces.
    pub nonce: Option<String>,
}

/// Scripts' fetch options kept per document, at most.
const MAX_MODULE_OPTIONS: usize = 4096;

impl Preloads {
    /// The fetch options of the script `key` names, when known.
    pub(crate) fn module_options(&self, key: &str) -> Option<ModuleFetchOptions> {
        self.module_options.get(key).cloned()
    }

    /// A script element's own options (its `crossorigin` and `referrerpolicy`).
    pub(crate) fn set_module_options(&mut self, key: String, mut options: ModuleFetchOptions) {
        if options == ModuleFetchOptions::default() && !self.module_options.contains_key(&key) {
            return;
        }
        // Two elements of one URL with different nonces: neither is the imports' (fail closed).
        if let Some(previous) = self.module_options.get(&key) {
            if previous.nonce.is_some() && previous.nonce != options.nonce {
                options.nonce = None;
            }
        }
        if self.module_options.len() < MAX_MODULE_OPTIONS || self.module_options.contains_key(&key) {
            self.module_options.insert(key, options);
        }
    }

    /// An inline script of the document with `nonce` ran ([`InlineNonce::note`]).
    pub(crate) fn note_inline_nonce(&mut self, nonce: &str) {
        self.inline_nonce.note(nonce);
    }

    /// The nonce of the document's inline scripts ([`InlineNonce::get`]).
    pub(crate) fn inline_nonce(&self) -> Option<&str> {
        self.inline_nonce.get()
    }

    /// An imported module's options: its importer's, unless it has its own already.
    pub(crate) fn inherit_module_options(&mut self, key: String, options: ModuleFetchOptions) {
        if self.module_options.len() < MAX_MODULE_OPTIONS {
            self.module_options.entry(key).or_insert(options);
        }
    }

    /// HTML "fetch a single module script": a module response's `Referrer-Policy` becomes the
    /// referrer policy of the modules it imports.
    pub(crate) fn note_module_referrer_policy(&mut self, key: String, policy: pyro_net::ReferrerPolicy) {
        if self.module_options.len() < MAX_MODULE_OPTIONS || self.module_options.contains_key(&key) {
            self.module_options.entry(key).or_default().referrer_policy = Some(policy);
        }
    }
}

/// Called with the preloaded module's source, or the fetch error.
pub(crate) type ModuleWaiter = Box<dyn FnOnce(&mut Browser, Result<String, String>)>;

enum ModuleEntry {
    /// The waiters, each with whether it imports the module (else it is a
    /// modulepreload link waiting for its `load` event).
    Pending(Vec<(ModuleWaiter, bool)>),
    /// Fetched, not imported yet.
    Done(String),
    /// Fetched and imported: the engine's module map has the module, and
    /// the document keeps no source. A later modulepreload link of the URL
    /// loads at once, as one answered from the module map; an import (the
    /// engine asks only for a module it does not have) requests it.
    Imported,
}

/// Preloads started per document, at most (a page's own chunks number far fewer).
const MAX_PRELOADS: usize = 2048;

impl Browser {
    /// Imports and modulepreloads share one document fetch, regardless of which starts first.
    /// `opts`: the fetch's referrer and referrer policy (its importer's); the first request of a
    /// URL fetches it with its own.
    pub(crate) fn fetch_imported_module(
        &mut self,
        page: FrameKey,
        url: String,
        opts: ScriptFetch,
        waiter: ModuleWaiter,
    ) {
        let Some(waiter) = self.preloaded_module_for(page, &url, waiter, true) else { return };
        let Some(mut doc) = self.doc_mut(page) else {
            self.fetch_script(page, url, ResourceKind::ModuleScript, opts, waiter);
            return;
        };
        let bindings = doc.main_frame.page_dom.clone();
        let document = bindings.as_ref().map(|bindings| bindings.document());
        let loader = doc.main_frame.loader_id.clone();
        doc.preloads.modules.insert(url.clone(), ModuleEntry::Pending(Vec::new()));
        drop(doc);
        self.fetch_script(page, url.clone(), ResourceKind::ModuleScript, opts, move |browser, result| {
            if browser.doc(page).is_none_or(|doc| doc.main_frame.loader_id != loader)
                || bindings
                    .as_ref()
                    .is_some_and(|bindings| bindings.is_disposed() || Some(bindings.document()) != document)
            {
                waiter(browser, result);
                return;
            }
            if let Some(mut doc) = browser.doc_mut(page) {
                match doc.preloads.modules.get_mut(&url) {
                    Some(ModuleEntry::Pending(waiters)) => waiters.insert(0, (waiter, true)),
                    _ => {
                        doc.preloads.modules.insert(url.clone(), ModuleEntry::Pending(vec![(waiter, true)]));
                    }
                }
            }
            browser.module_preloaded(page, &url, result);
        });
    }

    /// The shared module map for an importer (`import`), which takes
    /// the source out of the document, or for a modulepreload link, which
    /// only learns whether the fetch succeeded (its waiter gets an empty
    /// source).
    fn preloaded_module_for(
        &mut self,
        page: FrameKey,
        url: &str,
        waiter: ModuleWaiter,
        import: bool,
    ) -> Option<ModuleWaiter> {
        let Some(mut doc) = self.doc_mut(page) else { return Some(waiter) };
        let source = match doc.preloads.modules.get_mut(url) {
            Some(ModuleEntry::Pending(waiters)) => {
                waiters.push((waiter, import));
                return None;
            }
            // A link: the module was fetched (and is kept, or imported).
            Some(ModuleEntry::Done(_) | ModuleEntry::Imported) if !import => String::new(),
            // The import takes the source and leaves the marker.
            Some(entry @ ModuleEntry::Done(_)) => {
                let ModuleEntry::Done(source) = std::mem::replace(entry, ModuleEntry::Imported) else {
                    unreachable!("matched Done")
                };
                source
            }
            Some(ModuleEntry::Imported) | None => return Some(waiter),
        };
        drop(doc);
        waiter(self, Ok(source));
        None
    }

    /// A modulepreload's fetch finished: wake what waited for it, and keep
    /// the source for an import when none took it.
    fn module_preloaded(&mut self, page: FrameKey, url: &str, result: Result<String, String>) {
        let Some(mut doc) = self.doc_mut(page) else { return };
        let (key, waiters) = match doc.preloads.modules.remove_entry(url) {
            Some((key, ModuleEntry::Pending(waiters))) => (key, waiters),
            Some((key, ModuleEntry::Done(_) | ModuleEntry::Imported)) => (key, Vec::new()),
            None => (url.to_owned(), Vec::new()),
        };
        let last_import = waiters.iter().rposition(|(_, import)| *import);
        // What the importers get: the last one the source itself, the
        // others a copy. With none, the document keeps it.
        let mut source = match result {
            Ok(source) if last_import.is_none() => {
                doc.preloads.modules.insert(key, ModuleEntry::Done(source));
                Ok(None)
            }
            Ok(source) => {
                doc.preloads.modules.insert(key, ModuleEntry::Imported);
                Ok(Some(source))
            }
            // HTML does not keep a failed fetch in the module map: what
            // waited for this one fails, the next request fetches again.
            Err(e) => Err(e),
        };
        drop(doc);
        for (i, (waiter, import)) in waiters.into_iter().enumerate() {
            let outcome = match &mut source {
                Err(e) => Err(e.clone()),
                Ok(_) if !import => Ok(String::new()),
                Ok(source) if Some(i) == last_import => Ok(source.take().unwrap_or_default()),
                Ok(source) => Ok(source.clone().unwrap_or_default()),
            };
            waiter(self, outcome);
        }
    }

    pub(crate) fn preload_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        let frame = state.main_frame.clone();
        let config = {
            let dom = bindings.dom.borrow();
            let attr = |name| dom.get_attribute(node, name).ok().flatten().map(str::to_owned);
            let connected = dom.is_connected(node).unwrap_or(false);
            let rel = attr("rel").unwrap_or_default();
            let module = rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("modulepreload"));
            let preload = rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("preload"));
            let prefetch = rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("prefetch"));
            let dictionary = rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("compression-dictionary"));
            (connected && (module || preload || prefetch || dictionary))
                .then(|| {
                    // An image preload's source set chooses its URL (the first candidate here: one
                    // density); no other link fetches from `imagesrcset`.
                    let image_preload =
                        preload && !module && attr("as").is_some_and(|a| a.eq_ignore_ascii_case("image"));
                    let srcset = attr("imagesrcset").filter(|_| image_preload).and_then(|s| first_srcset_url(&s));
                    let href = srcset.or_else(|| attr("href")).filter(|v| !v.trim().is_empty())?;
                    let base = crate::loader::node_base_url(&dom, node).unwrap_or_else(|| frame.base_url.clone());
                    let url = url::Url::parse(&base).ok()?.join(href.trim()).ok()?.to_string();
                    let kind = if module {
                        ResourceKind::ModuleScript
                    } else if !preload {
                        ResourceKind::Prefetch
                    } else {
                        match attr("as").as_deref().map(str::to_ascii_lowercase).as_deref() {
                            Some("script") => ResourceKind::Script,
                            Some("style") => ResourceKind::Stylesheet,
                            // Sec-Fetch-Dest font, track and "" (empty).
                            Some("font") => ResourceKind::Font,
                            Some("track") => ResourceKind::Track,
                            Some("fetch") => ResourceKind::Fetch,
                            _ => ResourceKind::Image,
                        }
                    };
                    let integrity = attr("integrity").filter(|v| !pyro_webapi::sri::is_empty(v));
                    let referrer_policy = crate::loader::element_referrer_policy(&dom, node);
                    // A compression dictionary alone (not also a preload or prefetch).
                    let dictionary = dictionary && kind == ResourceKind::Prefetch && !prefetch;
                    Some((url, kind, attr("crossorigin"), integrity, referrer_policy, dictionary))
                })
                .flatten()
        };
        let current = state.preloads.entries.get(&node).cloned();
        drop(state);
        let Some((url, kind, crossorigin, integrity, referrer_policy, dictionary)) = config else {
            if let Some(mut doc) = self.doc_mut(page) {
                doc.preloads.entries.remove(&node);
            }
            return;
        };
        // What the link fetched: a dictionary's CORS settings are part of it (another one
        // fetches again).
        let fetched = if dictionary { format!("{url} {crossorigin:?}") } else { url.clone() };
        if current.as_deref() == Some(fetched.as_str()) {
            return;
        }
        {
            let Some(mut doc) = self.doc_mut(page) else { return };
            if doc.preloads.entries.len() >= MAX_PRELOADS {
                return;
            }
            doc.preloads.entries.insert(node, fetched);
        }
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(crate::loader::ResourceNode { dom: bindings.dom.clone(), node });
        let doc_id = bindings.document();
        if kind == ResourceKind::Prefetch {
            let referrer = frame.navigation_referrer();
            let target = PrefetchTarget { node, hold, bindings, crossorigin, referrer_policy, referrer };
            if dictionary {
                self.dictionary_link(page, &frame, url, target);
            } else {
                self.prefetch_link(page, &frame, url, target);
            }
            return;
        }
        let module = kind == ResourceKind::ModuleScript;
        if module {
            // The same module already preloaded (or preloading) in this
            // document: the link gets its event from that fetch.
            let (hold, bindings) = (hold.clone(), bindings.clone());
            let waiter: ModuleWaiter = Box::new(move |_: &mut Browser, result: Result<String, String>| {
                let _hold = hold;
                if bindings.is_disposed() || bindings.document() != doc_id {
                    return;
                }
                pyro_webapi::lifecycle::resource_loaded(&bindings, node, result.is_ok());
            });
            match self.preloaded_module_for(page, &url, waiter, false) {
                None => return,
                Some(waiter) => {
                    if let Some(mut doc) = self.doc_mut(page) {
                        doc.preloads.modules.insert(url.clone(), ModuleEntry::Pending(vec![(waiter, false)]));
                    }
                }
            }
        }
        let finish_url = url.clone();
        let cors = module || crossorigin.is_some();
        let finish = move |b: &mut Browser, result: Result<crate::Resource, String>| {
            let _hold = hold;
            let result = match result {
                Ok(r) if (200..300).contains(&r.status) => {
                    // SRI: a mismatching (or opaque) response is a network error.
                    let origin = b.doc(page).map(|p| p.main_frame.origin.clone()).unwrap_or_default();
                    match crate::loader::integrity_error(&r, integrity.as_deref(), cors, &origin) {
                        Some(e) => {
                            b.report_console_error(page, &e);
                            Err(e)
                        }
                        // A modulepreload is a JavaScript module script:
                        // strict MIME check, UTF-8 whatever the charset.
                        None if module => {
                            match pyro_webapi::module_script::mime_error(
                                pyro_js::ModuleType::JavaScript,
                                &r.mime,
                                &r.url,
                            ) {
                                Some(e) => Err(e),
                                None => Ok(pyro_webapi::module_script::utf8_decode(&r.body)),
                            }
                        }
                        None => Ok(r.text()),
                    }
                }
                Ok(r) => Err(format!("HTTP {} for {}", r.status, r.url)),
                Err(e) => Err(e),
            };
            if bindings.is_disposed() || bindings.document() != doc_id {
                return;
            }
            if module {
                // The link's own event comes through its waiter.
                b.module_preloaded(page, &finish_url, result);
            } else {
                pyro_webapi::lifecycle::resource_loaded(&bindings, node, result.is_ok());
            }
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let mut request = FetchRequest {
            csp: None,
            required_csp: None,
            purpose: None,
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind,
            page: page.page,
            referrer: frame.navigation_referrer(),
            referrer_policy,
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
            initiator: Some(pyro_webapi::resource_timing::Initiator::Link),
            module: None,
            part: None,
            accept_signature: None,
        };
        // CSP for the preload's destination, with the link's nonce.
        let Some(finish) = self.element_csp_check(page, &mut request, node, Box::new(finish)) else { return };
        if let Some(cancel) = self.fetch_document_resource(page, &resource_loader, request, finish) {
            cancel.disarm();
        }
    }
}

/// What a `<link rel=prefetch>`'s fetch needs from the link.
struct PrefetchTarget {
    node: NodeId,
    hold: Rc<crate::loader::ResourceNode>,
    bindings: Rc<pyro_webapi::PageDom>,
    crossorigin: Option<String>,
    referrer_policy: Option<pyro_net::ReferrerPolicy>,
    referrer: Option<String>,
}

/// A compression dictionary's request (`ResourceKind::Dictionary`).
fn dictionary_request(
    page: FrameKey,
    url: String,
    frame_id: String,
    loader_id: String,
    referrer: Option<String>,
    referrer_policy: Option<pyro_net::ReferrerPolicy>,
    crossorigin: Option<String>,
) -> FetchRequest {
    FetchRequest {
        csp: None,
        required_csp: None,
        purpose: None,
        parser_priority: false,
        credentials: None,
        post: None,
        url,
        kind: ResourceKind::Dictionary,
        page: page.page,
        referrer,
        referrer_policy,
        frame_id: Some(frame_id),
        loader_id: Some(loader_id),
        crossorigin,
        initiator: Some(pyro_webapi::resource_timing::Initiator::Link),
        module: None,
        part: None,
        accept_signature: None,
    }
}

/// The URL of an `imagesrcset`'s first image candidate.
fn first_srcset_url(srcset: &str) -> Option<String> {
    let first = srcset.split(',').map(str::trim).find(|c| !c.is_empty())?;
    first.split_ascii_whitespace().next().map(str::to_owned)
}

impl Browser {
    /// `<link rel=compression-dictionary>`: fetches the dictionary at `url` (the network client
    /// registers it), a fetch of its own each time the link's URL or CORS settings change (no
    /// fetch is shared between links) once the document has loaded, and the link gets `load` for
    /// any response or `error` for a network error, as a prefetch.
    fn dictionary_link(&mut self, page: FrameKey, frame: &crate::Frame, url: String, target: PrefetchTarget) {
        let PrefetchTarget { node, hold, bindings, crossorigin, referrer_policy, referrer } = target;
        let doc_id = bindings.document();
        let loader = frame.loader_id.clone();
        let (frame_id, loader_id) = (frame.id.0.clone(), frame.loader_id.0.clone());
        self.after_load(page, &loader, move |b: &mut Browser| {
            if bindings.is_disposed() || bindings.document() != doc_id {
                return;
            }
            let finish = move |_: &mut Browser, result: Result<crate::Resource, String>| {
                let _hold = hold;
                if bindings.is_disposed() || bindings.document() != doc_id {
                    return;
                }
                pyro_webapi::lifecycle::resource_loaded(&bindings, node, result.is_ok());
            };
            let Some(resource_loader) = b.loader_for(page) else {
                b.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
                return;
            };
            let mut request = dictionary_request(page, url, frame_id, loader_id, referrer, referrer_policy, crossorigin);
            let Some(finish) = b.element_csp_check(page, &mut request, node, Box::new(finish)) else { return };
            if let Some(cancel) = b.fetch_document_resource(page, &resource_loader, request, finish) {
                cancel.disarm();
            }
        });
    }

    /// The document's `Link` headers with `rel=preload`, `modulepreload` or `prefetch`
    /// ([`link_header_fetches`]): each is fetched at once, as its `<link>` would be, under the
    /// document's CSP and `Connection-Allowlist` (checked by the loader as it is for any request
    /// the document makes that no element checked).
    pub(crate) fn link_headers(&mut self, page: FrameKey, links: Vec<LinkFetch>) {
        let Some(frame) = self.doc(page).map(|d| d.main_frame.clone()) else { return };
        let Some(resource_loader) = self.loader_for(page) else { return };
        for link in links.into_iter().take(MAX_PRELOADS) {
            let request = FetchRequest {
                csp: None,
                required_csp: None,
                parser_priority: false,
                credentials: None,
                post: None,
                url: link.url,
                kind: link.kind,
                page: page.page,
                referrer: frame.navigation_referrer(),
                referrer_policy: None,
                frame_id: Some(frame.id.0.clone()),
                loader_id: Some(frame.loader_id.0.clone()),
                crossorigin: link.crossorigin,
                initiator: Some(pyro_webapi::resource_timing::Initiator::Link),
                module: None,
                part: None,
                accept_signature: None,
            };
            if let Some(cancel) = self.fetch_document_resource(page, &resource_loader, request, Box::new(|_, _| {})) {
                cancel.disarm();
            }
        }
    }

    /// The document's `Link` headers with `rel=compression-dictionary`: each is fetched once the
    /// document has loaded (CSP checks it at the document, `connect-src`).
    pub(crate) fn dictionary_headers(&mut self, page: FrameKey, urls: Vec<String>) {
        let Some(frame) = self.doc(page).map(|d| d.main_frame.clone()) else { return };
        let loader = frame.loader_id.clone();
        for url in urls.into_iter().take(MAX_PRELOADS) {
            let (frame_id, loader_id) = (frame.id.0.clone(), frame.loader_id.0.clone());
            let referrer = frame.navigation_referrer();
            self.after_load(page, &loader, move |b: &mut Browser| {
                let Some(resource_loader) = b.loader_for(page) else { return };
                let request = dictionary_request(page, url, frame_id, loader_id, referrer, None, None);
                if let Some(cancel) = b.fetch_document_resource(page, &resource_loader, request, Box::new(|_, _| {})) {
                    cancel.disarm();
                }
            });
        }
    }

    /// Runs `fetch` once `page`'s document (of `loader`) has fired its load event, or its load
    /// was aborted: now, or then ([`Browser::start_dictionary_fetches`]); not if another document
    /// has replaced it by then. Its callers bound how many wait (`MAX_PRELOADS` links, as many
    /// headers).
    fn after_load(&mut self, page: FrameKey, loader: &crate::LoaderId, fetch: impl FnOnce(&mut Browser) + 'static) {
        let Some(mut doc) = self.doc_mut(page) else { return };
        if doc.main_frame.loader_id != *loader {
            return;
        }
        let loader = loader.clone();
        let fetch = move |b: &mut Browser| {
            if b.doc(page).is_some_and(|d| d.main_frame.loader_id == loader) {
                fetch(b);
            }
        };
        if !doc.main_frame.loaded {
            doc.preloads.dictionaries.push(Box::new(fetch));
            return;
        }
        drop(doc);
        self.scheduler().post(TaskSource::Networking, fetch);
    }

    /// The document's load event fired (or `window.stop()` aborted its load): the compression
    /// dictionaries waiting for it are fetched.
    pub(crate) fn start_dictionary_fetches(&mut self, page: FrameKey) {
        let fetches = match self.doc_mut(page) {
            Some(mut doc) => std::mem::take(&mut doc.preloads.dictionaries),
            None => return,
        };
        for fetch in fetches {
            self.scheduler().post(TaskSource::Networking, fetch);
        }
    }

    /// Prefetches `url` for the link (`<link rel=prefetch>`), or joins the document's fetch of
    /// the URL: the link gets `load` or `error` in a later task either way.
    fn prefetch_link(&mut self, page: FrameKey, frame: &crate::Frame, url: String, target: PrefetchTarget) {
        let PrefetchTarget { node, hold, bindings, crossorigin, referrer_policy, referrer } = target;
        let doc_id = bindings.document();
        let event_bindings = bindings.clone();
        let waiter: PrefetchWaiter = Box::new(move |_: &mut Browser, ok: bool| {
            let _hold = hold;
            if event_bindings.is_disposed() || event_bindings.document() != doc_id {
                return;
            }
            pyro_webapi::lifecycle::resource_loaded(&event_bindings, node, ok);
        });
        let fetched = {
            let Some(mut doc) = self.doc_mut(page) else { return };
            let prefetches = &mut doc.preloads.prefetches;
            let full = prefetches.len() >= MAX_PRELOADS;
            match prefetches.get_mut(&url) {
                Some(PrefetchEntry::Pending(waiters)) => {
                    waiters.push(waiter);
                    return;
                }
                Some(PrefetchEntry::Done) => Some(waiter),
                // Bounded: past the limit a new URL is not prefetched (a hint, not a request).
                None if full => return,
                None => {
                    prefetches.insert(url.clone(), PrefetchEntry::Pending(vec![waiter]));
                    None
                }
            }
        };
        if let Some(waiter) = fetched {
            self.scheduler().post(TaskSource::Networking, move |b| waiter(b, true));
            return;
        }
        let finish_url = url.clone();
        let loader = frame.loader_id.clone();
        let start = std::time::Instant::now();
        let finish = move |b: &mut Browser, result: Result<crate::Resource, String>| {
            // Any response loads (a 404 or 500 too, as Chrome); a network error (CORS, CSP) is
            // an error.
            let ok = result.is_ok();
            if bindings.is_disposed() || bindings.document() != doc_id {
                return;
            }
            // A CSP block never reached the network: its entry has no response (Chrome reports
            // one with nothing transferred).
            if let Err(e) = &result {
                if e.starts_with(pyro_net::csp::BLOCKED_BY_CSP) {
                    bindings.queue_resource_timing(pyro_webapi::resource_timing::FetchReport {
                        url: finish_url.clone(),
                        initiator: pyro_webapi::resource_timing::Initiator::Link,
                        start,
                        end: std::time::Instant::now(),
                        response: None,
                        render_blocking: false,
                    });
                }
            }
            let waiters = match b.doc_mut(page) {
                Some(mut doc) if doc.main_frame.loader_id == loader => {
                    match doc.preloads.prefetches.remove(&finish_url) {
                        Some(PrefetchEntry::Pending(waiters)) => {
                            if ok {
                                doc.preloads.prefetches.insert(finish_url.clone(), PrefetchEntry::Done);
                            }
                            waiters
                        }
                        Some(PrefetchEntry::Done) => {
                            doc.preloads.prefetches.insert(finish_url.clone(), PrefetchEntry::Done);
                            Vec::new()
                        }
                        None => Vec::new(),
                    }
                }
                _ => Vec::new(),
            };
            for waiter in waiters {
                waiter(b, ok);
            }
        };
        // Mixed content: a secure document does not prefetch a URL that is not potentially
        // trustworthy (Chrome blocks it; WPT mixed-content `link-prefetch-tag`). Secure: its
        // origin is https:, or for an opaque one (a data: frame) its nearest ancestor's with an
        // origin, as `frame_ancestors_block` walks them. (The network client's mixed-content list
        // leaves prefetches out.)
        let mut origin = frame.origin.clone();
        let mut next = frame.parent.clone();
        while origin == "null" {
            let Some(id) = next.take() else { break };
            let Some(ancestor) = self.frame(page.page, &id) else { break };
            origin = ancestor.origin;
            next = ancestor.parent;
        }
        if origin.starts_with("https:")
            && pyro_net::Url::parse(&url).is_ok_and(|u| !pyro_net::secure_context::is_potentially_trustworthy_url(&u))
        {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("net::ERR_BLOCKED_BY_CLIENT".into())));
            return;
        }
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let mut request = FetchRequest {
            csp: None,
            required_csp: None,
            purpose: None,
            // Low priority: never ahead of the parser's requests.
            parser_priority: false,
            credentials: None,
            post: None,
            url,
            kind: ResourceKind::Prefetch,
            page: page.page,
            referrer,
            referrer_policy,
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
            initiator: Some(pyro_webapi::resource_timing::Initiator::Link),
            module: None,
            part: None,
            accept_signature: None,
        };
        // CSP: default-src or any fetch directive (`RequestDest::Prefetch`), with the link's
        // nonce.
        let Some(finish) = self.element_csp_check(page, &mut request, node, Box::new(finish)) else { return };
        // The fetch outlives the link and does not delay the document's load event.
        if let Some(cancel) = self.fetch_document_resource(page, &resource_loader, request, finish) {
            cancel.disarm();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InlineNonce;

    #[test]
    fn the_inline_nonce_is_the_one_every_nonced_inline_script_had() {
        let mut nonce = InlineNonce::default();
        assert_eq!(nonce.get(), None);
        // A script without a nonce changes nothing.
        nonce.note("");
        assert_eq!(nonce.get(), None);
        nonce.note("abc");
        assert_eq!(nonce.get(), Some("abc"));
        nonce.note("");
        nonce.note("abc");
        assert_eq!(nonce.get(), Some("abc"));
        // A different one clears it for good.
        nonce.note("xyz");
        assert_eq!(nonce.get(), None);
        nonce.note("abc");
        assert_eq!(nonce.get(), None);
    }
}
