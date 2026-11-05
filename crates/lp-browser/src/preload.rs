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
//! are megabytes of text). `rel=preload as=script` stays a plain fetch into
//! the HTTP cache.

use std::collections::HashMap;
use std::rc::Rc;

use lp_dom::NodeId;
use lp_runtime::TaskSource;

use crate::frames::FrameKey;
use crate::{Browser, FetchRequest, ResourceKind};

/// Links fetched (or being fetched) per document: node → URL.
#[derive(Default)]
pub(crate) struct Preloads {
    entries: HashMap<NodeId, String>,
    /// Module sources fetched (or being fetched) by modulepreload: URL → source.
    modules: HashMap<String, ModuleEntry>,
}

/// Called with the preloaded module's source, or the fetch error.
pub(crate) type ModuleWaiter = Box<dyn FnOnce(&mut Browser, Result<String, String>)>;

enum ModuleEntry {
    /// The waiters, each with whether it imports the module (else it is a
    /// modulepreload link waiting for its `load` event).
    Pending(Vec<(ModuleWaiter, bool)>),
    /// Fetched, not imported yet.
    Done(String),
}

/// Preloads started per document, at most (a page's own chunks number far fewer).
const MAX_PRELOADS: usize = 2048;

impl Browser {
    /// Hands the importing `waiter` the source a modulepreload fetched (or
    /// fetches) for `url` in this document; gives it back when there is no
    /// such preload, so the caller requests the module itself.
    pub(crate) fn preloaded_module(&mut self, page: FrameKey, url: &str, waiter: ModuleWaiter) -> Option<ModuleWaiter> {
        self.preloaded_module_for(page, url, waiter, true)
    }

    /// [`Browser::preloaded_module`] for an importer (`import`), which takes
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
            Some(ModuleEntry::Done(source)) if import => {
                let source = std::mem::take(source);
                doc.preloads.modules.remove(url);
                source
            }
            Some(ModuleEntry::Done(_)) => String::new(),
            None => return Some(waiter),
        };
        drop(doc);
        waiter(self, Ok(source));
        None
    }

    /// A modulepreload's fetch finished: wake what waited for it, and keep
    /// the source for an import when none took it.
    fn module_preloaded(&mut self, page: FrameKey, url: &str, result: Result<String, String>) {
        let Some(mut doc) = self.doc_mut(page) else { return };
        let waiters = match doc.preloads.modules.remove(url) {
            Some(ModuleEntry::Pending(waiters)) => waiters,
            Some(ModuleEntry::Done(_)) | None => Vec::new(),
        };
        let imported = waiters.iter().any(|(_, import)| *import);
        if let (Ok(source), false) = (&result, imported) {
            doc.preloads.modules.insert(url.to_owned(), ModuleEntry::Done(source.clone()));
        }
        drop(doc);
        // The last importer gets the source itself, the others a copy;
        // links only the outcome.
        let (mut source, failed) = match result {
            Ok(source) => (Some(source), None),
            Err(e) => (None, Some(e)),
        };
        let last_import = waiters.iter().rposition(|(_, import)| *import);
        for (i, (waiter, import)) in waiters.into_iter().enumerate() {
            let outcome = match (&failed, import) {
                (Some(e), _) => Err(e.clone()),
                (None, false) => Ok(String::new()),
                (None, true) if Some(i) == last_import => Ok(source.take().unwrap_or_default()),
                (None, true) => Ok(source.clone().unwrap_or_default()),
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
            (connected && (module || preload))
                .then(|| {
                    let href = attr("href").filter(|v| !v.trim().is_empty())?;
                    let url = url::Url::parse(&frame.base_url).ok()?.join(href.trim()).ok()?.to_string();
                    let kind = if module {
                        ResourceKind::ModuleScript
                    } else {
                        match attr("as").as_deref().map(str::to_ascii_lowercase).as_deref() {
                            Some("script") => ResourceKind::Script,
                            Some("style") => ResourceKind::Stylesheet,
                            _ => ResourceKind::Image,
                        }
                    };
                    Some((url, kind, attr("crossorigin")))
                })
                .flatten()
        };
        let current = state.preloads.entries.get(&node).cloned();
        drop(state);
        let Some((url, kind, crossorigin)) = config else {
            if let Some(mut doc) = self.doc_mut(page) {
                doc.preloads.entries.remove(&node);
            }
            return;
        };
        if current.as_deref() == Some(url.as_str()) {
            return;
        }
        {
            let Some(mut doc) = self.doc_mut(page) else { return };
            if doc.preloads.entries.len() >= MAX_PRELOADS {
                return;
            }
            doc.preloads.entries.insert(node, url.clone());
        }
        if bindings.dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let hold = Rc::new(crate::loader::ResourceNode { dom: bindings.dom.clone(), node });
        let doc_id = bindings.document();
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
                lp_webapi::lifecycle::resource_loaded(&bindings, node, result.is_ok());
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
        let finish = move |b: &mut Browser, result: Result<crate::Resource, String>| {
            let _hold = hold;
            let result = match result {
                Ok(r) if (200..300).contains(&r.status) => Ok(r.text()),
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
                lp_webapi::lifecycle::resource_loaded(&bindings, node, result.is_ok());
            }
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let request = FetchRequest {
            post: None,
            url,
            kind,
            page: page.page,
            referrer: Some(frame.base_url.clone()),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin,
        };
        resource_loader.fetch(self, request, Box::new(finish));
    }
}
