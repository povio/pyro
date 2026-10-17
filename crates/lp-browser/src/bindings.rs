//! Connects document bindings to the browser's lifecycle and task queues.

use crate::{Browser, LoaderId, NavigateOpts, NavigationReason, PageId};
use lp_js::JsError;
use lp_runtime::{LoopHandle, Scheduler, TaskSource};
use lp_webapi::{NavigationRequest, PageDom, PageHooks, RemotePoster};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

struct Hooks {
    page: PageId,
    loader: LoaderId,
    sched: Scheduler<Browser>,
    handle: LoopHandle<Browser>,
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
        bindings
            .set_user_agent_override(state.user_agent_override.clone())
            .map_err(|e| JsError::Type(format!("invalid user agent request defaults: {e}")))?;
        bindings.set_request_context(lp_webapi::RequestContext {
            frame_id: Some(state.main_frame.id.0.clone()),
            loader_id: Some(loader.0.clone()),
        });
        bindings.create_window_target(doc);
        bindings.set_ready_state(doc, "loading");
        *bindings.net.borrow_mut() = self.context_net(context).cloned();
        bindings.set_hooks(Some(Rc::new(Hooks {
            page,
            loader: loader.clone(),
            sched: self.scheduler(),
            handle: self.loop_handle(),
        })));
        Ok(bindings)
    }

    pub(crate) fn drain_dom_resources(&mut self, page: PageId) {
        let Some(bindings) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) else { return };
        let events = bindings.dom.borrow_mut().take_resource_events();
        for event in events {
            bindings.apply_style_event(&event);
        }
    }
}
