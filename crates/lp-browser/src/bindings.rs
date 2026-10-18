//! Connects document bindings to the browser's lifecycle and task queues.

use crate::{Browser, ContextId, LoaderId, NavigateOpts, NavigationReason, ObserverCell, PageId};
use lp_js::JsError;
use lp_runtime::{LoopHandle, Scheduler, TaskSource};
use lp_webapi::{NavigationRequest, PageDom, PageHooks, RemotePoster};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct Hooks {
    page: PageId,
    /// Browser context of the page: native request ids are only unique
    /// within its `NetClient`.
    context: ContextId,
    loader: LoaderId,
    sched: Scheduler<Browser>,
    handle: LoopHandle<Browser>,
    /// The browser's observer cell (shared, so later `set_observer` calls apply).
    observer: ObserverCell,
}

impl PageHooks for Hooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::DomManipulation, move |b| {
            if b.page(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                task();
            }
        });
    }

    fn navigate(&self, request: NavigationRequest) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            if !b.page(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                return;
            }
            let result = if request.method != "GET"
                || request.target.as_deref().is_some_and(|t| !matches!(t, "" | "_self" | "_top" | "_parent"))
            {
                Err(JsError::Unsupported("form POST and named navigation targets"))
            } else {
                b.navigate(
                    page,
                    NavigateOpts {
                        url: request.url,
                        reason: NavigationReason::Anchor,
                        cdp_request_id: None,
                        referrer: b.page(page).map(|p| p.main_frame.url.clone()),
                    },
                )
                .map(|_| ())
            };
            if let Err(e) = result {
                if let Some(realm) = b.page(page).and_then(|p| p.main_frame.main_realm()) {
                    if let Ok(scope) = realm.scope() {
                        scope.report_exception(&e);
                    }
                }
            }
        });
    }

    fn network_delivery(&self, id: lp_net::RequestId, frontier: lp_net::NetworkDelivery) {
        // Clone the observer out of the cell first, as `emit_to` does: the
        // callback must not run under the cell borrow.
        let observer = self.observer.borrow().clone();
        if let Some(observer) = observer {
            observer.on_network_delivery(self.page, self.context, &self.loader, id, frontier);
        }
    }

    fn remote_poster(&self) -> Option<RemotePoster> {
        let handle = self.handle.clone();
        let page = self.page;
        let loader = self.loader.clone();
        Some(Arc::new(move |task| {
            let loader = loader.clone();
            let _ = handle.post(move |b| {
                if b.page(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                    task();
                }
            });
        }))
    }
}

impl Browser {
    pub(crate) fn create_document_bindings(
        &self,
        page: PageId,
        url: &str,
        loader: &LoaderId,
    ) -> Result<Rc<PageDom>, JsError> {
        let state = self.page(page).ok_or_else(|| JsError::Type("no such page".into()))?;
        let context = state.context;
        let local_storage = self
            .contexts
            .iter()
            .find(|c| c.id == context)
            .ok_or_else(|| JsError::Type("no such browser context".into()))?
            .local_storage
            .clone();
        let session_storage = state.session_storage.clone();
        let dom = Rc::new(RefCell::new(lp_dom::Dom::new()));
        let doc = if url == "about:blank" {
            lp_dom::parse::parse_document(&dom, url, "<!doctype html><html><head></head><body></body></html>")
        } else {
            dom.borrow_mut().create_document(lp_dom::DocumentKind::Html, url)
        };
        dom.borrow_mut().set_document_active(doc, true).map_err(|e| JsError::Type(format!("{e:?}")))?;
        dom.borrow_mut().set_resource_tracking(true);
        let bindings = PageDom::new(dom, doc);
        bindings.set_storage_namespaces(local_storage, session_storage);
        bindings.set_viewport(state.viewport.clone());
        bindings
            .set_user_agent_override(state.user_agent_override.clone())
            .map_err(|e| JsError::Type(format!("invalid user agent request defaults: {e}")))?;
        bindings.set_extra_http_headers(state.extra_http_headers.clone());
        bindings.set_cache_disabled(state.cache_disabled);
        bindings.set_request_context(lp_webapi::RequestContext {
            frame_id: Some(state.main_frame.id.0.clone()),
            loader_id: Some(loader.0.clone()),
        });
        bindings.create_window_target(doc);
        bindings.set_ready_state(doc, "loading");
        *bindings.net.borrow_mut() = self.context_net(context).cloned();
        bindings.set_hooks(Some(Rc::new(Hooks {
            page,
            context,
            loader: loader.clone(),
            sched: self.scheduler(),
            handle: self.loop_handle(),
            observer: self.observer.clone(),
        })));
        Ok(bindings)
    }

    pub(crate) fn drain_dom_resources(&mut self, page: PageId) {
        let Some(bindings) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) else { return };
        let events = bindings.dom.borrow_mut().take_resource_events();
        let mut stylesheet_nodes = std::collections::HashSet::new();
        for event in events {
            if event.kind == lp_dom::script::ResourceKind::Stylesheet {
                if stylesheet_nodes.insert(event.node) {
                    self.stylesheet_event(page, event.node);
                }
            } else {
                bindings.apply_style_event(&event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PageEvent, PageObserver};
    use std::cell::Cell;

    type Seen = (PageId, ContextId, LoaderId, lp_net::RequestId, lp_net::NetworkDelivery);

    /// Unregisters itself from the browser's observer cell during the
    /// callback, which needs the cell not to be borrowed at that point.
    struct SelfRemoving {
        cell: ObserverCell,
        calls: Cell<u32>,
        seen: RefCell<Vec<Seen>>,
    }

    impl PageObserver for SelfRemoving {
        fn on_page_event(&self, _page: PageId, _event: &PageEvent) {}
        fn on_network_delivery(
            &self,
            page: PageId,
            context: ContextId,
            loader: &LoaderId,
            request: lp_net::RequestId,
            frontier: lp_net::NetworkDelivery,
        ) {
            self.calls.set(self.calls.get() + 1);
            self.seen.borrow_mut().push((page, context, loader.clone(), request, frontier));
            *self.cell.try_borrow_mut().expect("observer cell borrowed during the callback") = None;
        }
    }

    #[test]
    fn network_delivery_forwards_identity_without_holding_the_observer_cell() {
        let event_loop = lp_runtime::EventLoop::<Browser>::new();
        let cell: ObserverCell = Rc::new(RefCell::new(None));
        let observer =
            Rc::new(SelfRemoving { cell: cell.clone(), calls: Cell::new(0), seen: RefCell::new(Vec::new()) });
        *cell.borrow_mut() = Some(observer.clone());
        let hooks = Hooks {
            page: PageId(3),
            context: ContextId(7),
            loader: LoaderId("L1".into()),
            sched: event_loop.scheduler(),
            handle: event_loop.handle(),
            observer: cell.clone(),
        };
        let frontier = lp_net::NetworkDelivery { response: true, data_bytes: 5, terminal: false };
        hooks.network_delivery(lp_net::RequestId(1), frontier);
        // The observer removed itself: later deliveries reach nobody.
        hooks.network_delivery(lp_net::RequestId(1), lp_net::NetworkDelivery { terminal: true, ..frontier });
        assert_eq!(observer.calls.get(), 1);
        assert_eq!(
            observer.seen.borrow().as_slice(),
            [(PageId(3), ContextId(7), LoaderId("L1".into()), lp_net::RequestId(1), frontier)]
        );
        assert!(cell.borrow().is_none());
    }
}
