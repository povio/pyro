//! `<link rel=modulepreload>` and `<link rel=preload>`: the resource is
//! fetched through the page's loader (so it lands in the context's HTTP
//! cache for the import or request that follows) and the link gets `load`
//! or `error`. Vite's preload helper awaits those events before running a
//! dynamic `import()`; without them a lazily loaded chunk never loads.

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
}

/// Preloads started per document, at most (a page's own chunks number far fewer).
const MAX_PRELOADS: usize = 2048;

impl Browser {
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
        let finish = move |_: &mut Browser, result: Result<crate::Resource, String>| {
            let _hold = hold;
            if bindings.is_disposed() || bindings.document() != doc_id {
                return;
            }
            let ok = result.is_ok_and(|r| (200..300).contains(&r.status));
            lp_webapi::lifecycle::resource_loaded(&bindings, node, ok);
        };
        let Some(resource_loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| finish(b, Err("no resource loader".into())));
            return;
        };
        let request = FetchRequest {
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
