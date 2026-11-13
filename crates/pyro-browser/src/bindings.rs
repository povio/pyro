//! Connects document bindings to the browser's lifecycle and task queues.

use crate::frames::FrameKey;
use crate::{Browser, ContextId, LoaderId, NavigateOpts, NavigationReason, ObserverCell};
use pyro_js::JsError;
use pyro_runtime::{LoopHandle, Scheduler, TaskSource};
use pyro_webapi::{NavigationRequest, PageDom, PageHooks, RemotePoster};
use std::rc::Rc;
use std::sync::Arc;

struct Hooks {
    page: FrameKey,
    /// Browser context of the page: native request ids are only unique
    /// within its `NetClient`.
    context: ContextId,
    loader: LoaderId,
    sched: Scheduler<Browser>,
    handle: LoopHandle<Browser>,
    /// The browser's observer cell (shared, so later `set_observer` calls apply).
    observer: ObserverCell,
    frames: std::rc::Weak<crate::frames::FrameHost>,
}

impl PageHooks for Hooks {
    fn frame_detached(&self) -> bool {
        // Only a child frame can leave its page.
        self.page.child.is_some()
            && self.frames.upgrade().is_some_and(|host| host.with_frame(self.page, |_| ()).is_none())
    }

    fn focus_document_is_live(&self) -> bool {
        self.frames.upgrade().is_some_and(|host| {
            host.matches(self.page, &self.loader)
                && host.frame(self.page).and_then(|f| f.page_dom).is_some_and(|p| !p.is_disposed())
        })
    }

    fn is_focused(&self) -> bool {
        self.frames.upgrade().is_some_and(|host| host.is_focused(self.page, &self.loader))
    }

    fn request_focus(&self, target: pyro_webapi::FocusTarget, token: &pyro_webapi::FocusTransition) -> bool {
        self.frames.upgrade().is_some_and(|host| host.request_focus(self.page, &self.loader, target, token))
    }

    fn post_task(&self, task: Box<dyn FnOnce()>) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::DomManipulation, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                task();
            }
        });
    }

    fn post_delayed_task(&self, delay: std::time::Duration, task: Box<dyn FnOnce()>) {
        let page = self.page;
        let loader = self.loader.clone();
        let task = std::cell::RefCell::new(Some(task));
        self.sched.set_timer(delay, false, true, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                if let Some(task) = task.borrow_mut().take() {
                    task();
                }
            }
        });
    }

    fn fetch_media(
        &self,
        request: pyro_webapi::MediaRequest,
        done: Box<dyn FnOnce(Result<pyro_webapi::MediaResponse, String>)>,
    ) -> Option<pyro_webapi::MediaFetchHandle> {
        let handle = crate::media::MediaFetch {
            cancelled: Rc::new(std::cell::Cell::new(false)),
            cancel: Rc::new(std::cell::RefCell::new(None)),
        };
        let (cancelled, slot) = (handle.cancelled.clone(), handle.cancel.clone());
        // A finished request has nothing to cancel.
        let finished = slot.clone();
        let done: crate::media::MediaDone = Box::new(move |result| {
            let cancel = finished.borrow_mut().take();
            if let Some(cancel) = cancel {
                cancel.disarm();
            }
            done(result);
        });
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Networking, move |b| {
            if cancelled.get() || !b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                return;
            }
            let cancel = b.fetch_media(page, loader, request, done);
            if cancelled.get() {
                drop(cancel);
            } else {
                *slot.borrow_mut() = cancel;
            }
        });
        Some(Box::new(handle))
    }

    fn set_media_timer(&self, delay: std::time::Duration, task: Box<dyn FnOnce()>) -> Option<u64> {
        let page = self.page;
        let loader = self.loader.clone();
        let task = std::cell::RefCell::new(Some(task));
        // Playback must not keep `run_until_idle` (or network idle) busy.
        let id = self.sched.set_timer(delay, false, false, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                let task = task.borrow_mut().take();
                if let Some(task) = task {
                    task();
                }
            }
        });
        Some(u64::from(id.0))
    }

    fn clear_media_timer(&self, id: u64) {
        if let Ok(id) = u32::try_from(id) {
            self.sched.clear_timer(pyro_runtime::TimerId(id));
        }
    }

    fn media_load_delay_released(&self) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Networking, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.maybe_fire_load(page);
            }
        });
    }

    fn navigate(&self, request: NavigationRequest) {
        let page = self.page;
        let loader = self.loader.clone();
        // HTML "navigate": a GET of the document's own URL with a fragment is
        // a fragment navigation, done at once (the URL has changed when the
        // link's `click()` returns) and never a reload.
        let own_frame = request.target.as_deref().is_none_or(|t| t.is_empty() || t.eq_ignore_ascii_case("_self"));
        let nav = own_frame
            .then(|| self.frames.upgrade().filter(|host| host.matches(page, &loader)).and_then(|host| host.nav(page)))
            .flatten();
        if request.method == "GET" && request.body.is_none() {
            if nav.as_ref().is_some_and(|nav| nav.navigate_to_fragment(&request.url)) {
                return;
            }
            // Navigation API: the document's `navigate` event may cancel
            // or intercept the load.
            if let Some(nav) = nav.as_ref().filter(|_| !request.url.starts_with("javascript:")) {
                match nav.navigate_event("push", &request.url, true) {
                    crate::nav::NavigateDecision::Cancel => return,
                    crate::nav::NavigateDecision::Intercept => return nav.commit_intercepted(&request.url, "push"),
                    crate::nav::NavigateDecision::Proceed => {}
                }
            }
        }
        // `document.open()` before the task runs drops it (stop loading).
        let stops = nav.map(|nav| nav.stop_count());
        self.sched.post(TaskSource::Navigation, move |b| {
            if !b.doc(page).is_some_and(|p| {
                p.main_frame.loader_id == loader && stops.is_none_or(|stops| p.nav.stop_count() == stops)
            }) {
                return;
            }
            let named = request.target.as_deref().and_then(|t| b.page(page.page)?.frames.named_frame(page, t));
            // HTML "navigate to a javascript: URL": the script runs in the
            // target document, which stays (replacing it with a string result
            // is not supported).
            if let Some(source) = javascript_url_source(&request.url) {
                let realm = b.doc(named.unwrap_or(page)).and_then(|p| p.main_frame.main_realm().cloned());
                if let Some(scope) = realm.as_ref().and_then(|realm| realm.scope().ok()) {
                    if let Err(e) = scope.eval(&source, None, 1) {
                        scope.report_exception(&e);
                    }
                }
                return;
            }
            let new_context = named.is_none()
                && request
                    .target
                    .as_deref()
                    .is_some_and(|t| !matches!(t.to_ascii_lowercase().as_str(), "" | "_self" | "_top" | "_parent"));
            // `_blank` (and any name that is not a frame of this page) opens a
            // popup page; the embedder creates and navigates it.
            if new_context && request.method == "GET" {
                let frame = b.doc(page).map(|p| (p.main_frame.id.clone(), p.main_frame.sandbox.popups_blocked));
                if let Some((frame_id, false)) = frame {
                    b.emit(
                        page.page,
                        crate::PageEvent::PopupRequested {
                            frame_id,
                            url: request.url,
                            opener: request.opener,
                            ticket: request.popup,
                        },
                    );
                }
                return;
            }
            let result = if new_context
                || (page.child.is_some() && request.target.as_deref().is_some_and(|t| matches!(t, "_top" | "_parent")))
            {
                Err(JsError::Unsupported(&"form POST to a new window and cross-frame navigation targets"))
            } else {
                b.navigate_frame_with_handling(
                    named.unwrap_or(page),
                    NavigateOpts {
                        post: request.body.map(|body| (body, request.content_type.unwrap_or_default())),
                        url: request.url,
                        reason: if request.method == "POST" {
                            NavigationReason::FormPost
                        } else {
                            NavigationReason::Anchor
                        },
                        cdp_request_id: None,
                        referrer: b.doc(page).map(|p| p.main_frame.url.clone()),
                    },
                    crate::HistoryHandling::Push,
                )
                .map(|_| ())
            };
            if let Err(e) = result {
                if let Some(realm) = b.doc(page).and_then(|p| p.main_frame.main_realm().cloned()) {
                    if let Ok(scope) = realm.scope() {
                        scope.report_exception(&e);
                    }
                }
            }
        });
    }

    fn popup_command(&self, popup: std::rc::Rc<pyro_webapi::PopupState>, command: pyro_webapi::PopupCommand) {
        let page = self.page;
        // The queue `navigate` uses, so this follows the popup's request.
        self.sched.post(TaskSource::Navigation, move |b| {
            b.emit(page.page, crate::PageEvent::PopupCommand { popup, command });
        });
    }

    fn close_window(&self) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            // Only a popup is script-closable (Chrome: "Scripts may close
            // only the windows that were opened by them").
            if page.child.is_some() || !b.page(page.page).is_some_and(|p| p.popup.is_some()) {
                return;
            }
            let Some(frame_id) =
                b.doc(page).filter(|p| p.main_frame.loader_id == loader).map(|p| p.main_frame.id.clone())
            else {
                return;
            };
            b.emit(page.page, crate::PageEvent::CloseRequested { frame_id });
        });
    }

    fn file_chooser(&self, input: pyro_dom::NodeId, multiple: bool) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::UserInteraction, move |b| {
            let Some(frame_id) =
                b.doc(page).filter(|p| p.main_frame.loader_id == loader).map(|p| p.main_frame.id.clone())
            else {
                return;
            };
            b.emit(page.page, crate::PageEvent::FileChooserOpened { frame_id, input, multiple });
        });
    }

    fn javascript_dialog(
        &self,
        kind: &'static str,
        message: String,
        default_prompt: Option<String>,
        url: String,
    ) -> Option<pyro_webapi::DialogAnswer> {
        // A live document's dialog may block in the embedder (its observer).
        let observer = self.observer.borrow().clone();
        let frame = self
            .frames
            .upgrade()
            .filter(|host| host.matches(self.page, &self.loader))
            .and_then(|host| host.with_frame(self.page, |f| f.id.clone()));
        if let (Some(observer), Some(frame)) = (observer, frame) {
            let answer =
                observer.javascript_dialog(self.page.page, &frame, &url, kind, &message, default_prompt.as_deref());
            if answer.is_some() {
                return answer;
            }
        }
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::UserInteraction, move |b| {
            let Some(frame_id) =
                b.doc(page).filter(|p| p.main_frame.loader_id == loader).map(|p| p.main_frame.id.clone())
            else {
                return;
            };
            b.emit(
                page.page,
                crate::PageEvent::JavascriptDialogOpening {
                    frame_id: frame_id.clone(),
                    kind: kind.into(),
                    message,
                    default_prompt,
                },
            );
            b.emit(page.page, crate::PageEvent::JavascriptDialogClosed { frame_id });
        });
        None
    }

    fn download(&self, request: pyro_webapi::DownloadRequest) {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Navigation, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.start_download(page, request);
            }
        });
    }

    fn document_reopened(&self) {
        // At once, not in the task below: a navigation the same script
        // starts commits against the opened document.
        let nav = self
            .frames
            .upgrade()
            .filter(|host| host.matches(self.page, &self.loader))
            .and_then(|host| host.nav(self.page));
        if let Some(nav) = nav {
            nav.document_opened();
        }
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::DomManipulation, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.document_reopened(page);
            }
        });
    }

    fn document_closed(&self) -> bool {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::DomManipulation, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.finish_written_document(page);
            }
        });
        true
    }

    fn written_script_queued(&self) -> bool {
        let page = self.page;
        let loader = self.loader.clone();
        self.sched.post(TaskSource::Networking, move |b| {
            if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                b.prepare_written_scripts(page);
            }
        });
        true
    }

    fn network_delivery(&self, id: pyro_net::RequestId, frontier: pyro_net::NetworkDelivery) {
        // Clone the observer out of the cell first, as `emit_to` does: the
        // callback must not run under the cell borrow.
        let observer = self.observer.borrow().clone();
        if let Some(observer) = observer {
            observer.on_network_delivery(self.page.page, self.context, &self.loader, id, frontier);
        }
    }

    fn worker_event(&self, event: pyro_webapi::WorkerEvent) {
        let observer = self.observer.borrow().clone();
        if let Some(observer) = observer {
            observer.on_worker_event(self.page.page, event);
        }
    }

    fn request_animation_frame(&self) {
        if let Some(host) = self.frames.upgrade() {
            if host.matches(self.page, &self.loader) {
                if let Some(realm) = host.frame(self.page).and_then(|f| f.main_realm().cloned()) {
                    crate::window::request_frame(&realm);
                }
            }
        }
    }

    fn iframe_connected(&self, node: pyro_dom::NodeId) {
        if let Some(host) = self.frames.upgrade() {
            if let Err(e) = host.connected(self.page, &self.loader, node) {
                if let Some(realm) = host.frame(self.page).and_then(|f| f.main_realm().cloned()) {
                    if let Ok(scope) = realm.scope() {
                        scope.report_exception(&e);
                    }
                }
            }
        }
    }
    fn script_connected(&self, node: pyro_dom::NodeId) {
        if let Some(host) = self.frames.upgrade() {
            if host.matches(self.page, &self.loader) {
                if let Some(frame) = host.frame(self.page) {
                    crate::loader::prepare_inserted_script(&frame, node);
                }
            }
        }
    }
    fn iframe_disconnected(&self, node: pyro_dom::NodeId) {
        if let Some(host) = self.frames.upgrade() {
            if host.matches(self.page, &self.loader) {
                if let Some(key) = host.owner(node) {
                    host.detach(key);
                }
            }
        }
    }
    fn iframe_attribute_changed(&self, node: pyro_dom::NodeId, attr: pyro_webapi::IframeAttribute) {
        let Some(host) = self.frames.upgrade() else { return };
        if !host.matches(self.page, &self.loader) {
            return;
        }
        let Some(key) = host.owner(node) else { return };
        if attr == pyro_webapi::IframeAttribute::Name {
            let name = host.dom.borrow().get_attribute(node, "name").ok().flatten().unwrap_or("").to_owned();
            if let Some(id) = key.child {
                if let Some(child) = host.children.borrow_mut().get_mut(&id) {
                    child.main_frame.name = name;
                }
            }
        } else if attr != pyro_webapi::IframeAttribute::Sandbox {
            if let Some(frame) = host.frame(key) {
                host.queue_attributes(key, frame.loader_id);
            }
        }
    }
    fn frame_document_value<'s>(
        &self,
        scope: &pyro_js::Scope<'s>,
        node: pyro_dom::NodeId,
    ) -> pyro_js::JsResult<Option<pyro_js::Value<'s>>> {
        let Some(host) = self.frames.upgrade() else { return Ok(Some(scope.null())) };
        let Some(key) = host.owner(node) else { return Ok(Some(scope.null())) };
        host.document_value(scope, self.page, key).map(Some)
    }
    fn sibling_realms(&self, scope: &pyro_js::Scope<'_>) -> Vec<pyro_js::Realm> {
        let Some(host) = self.frames.upgrade().filter(|h| h.has_children() || h.has_retired()) else {
            return Vec::new();
        };
        host.sibling_realms(self.page, scope.realm_id())
    }
    fn frame_window<'s>(&self, scope: &pyro_js::Scope<'s>, node: pyro_dom::NodeId) -> Option<pyro_js::Value<'s>> {
        let host = self.frames.upgrade()?;
        if !host.matches(self.page, &self.loader) {
            return None;
        }
        let key = host.owner(node)?;
        match host.window_value(scope, self.page, key) {
            Ok(v) => Some(v),
            Err(e) => {
                scope.report_exception(&e);
                None
            }
        }
    }

    fn remote_poster(&self) -> Option<RemotePoster> {
        let handle = self.handle.clone();
        let page = self.page;
        let loader = self.loader.clone();
        Some(Arc::new(move |task| {
            let loader = loader.clone();
            let _ = handle.post(move |b| {
                if b.doc(page).is_some_and(|p| p.main_frame.loader_id == loader) {
                    task();
                }
            });
        }))
    }

    fn thread_poster(&self) -> Option<RemotePoster> {
        let handle = self.handle.clone();
        Some(Arc::new(move |task| {
            let _ = handle.post(move |_| task());
        }))
    }
}

impl Browser {
    pub(crate) fn create_document_bindings(
        &self,
        page: FrameKey,
        url: &str,
        loader: &LoaderId,
    ) -> Result<Rc<PageDom>, JsError> {
        let state = self.page(page.page).ok_or(JsError::ContextDestroyed)?;
        let settings = crate::frames::Settings {
            local: self
                .contexts
                .iter()
                .find(|c| c.id == state.context)
                .ok_or(JsError::ContextDestroyed)?
                .local_storage
                .clone(),
            services: self.context_services(state.context).ok_or(JsError::ContextDestroyed)?,
            max_touch_points: state.max_touch_points,
            session: state.session_storage.clone(),
            net: self.context_net(state.context).cloned(),
            viewport: state.viewport.clone(),
            user_agent: state.user_agent_override.clone(),
            extra_headers: state.extra_http_headers.clone(),
            cache_disabled: state.cache_disabled,
            bypass_service_worker: state.bypass_service_worker,
            scripts: state.init_scripts.clone(),
            bindings: state.bindings.clone(),
        };
        *state.frames.settings.borrow_mut() = settings;
        if page.child.is_none() {
            *state.frames.root.borrow_mut() = state.main_frame.clone();
        }
        let dom = create(&state.frames, page, url, loader)?;
        if page.child.is_none() {
            if let Some(ticket) = &state.popup {
                ticket.attach_page(&dom);
            }
        }
        Ok(dom)
    }

    pub(crate) fn drain_dom_resources(&mut self, page: FrameKey) {
        let Some(host) = self.page(page.page).map(|p| p.frames.clone()) else { return };
        // Runs at every checkpoint: leave early when nothing is queued.
        let (iframe_events, resource_events) = {
            let dom = host.dom.borrow();
            (dom.has_iframe_events() || dom.has_media_events(), dom.has_resource_events())
        };
        if !iframe_events && !resource_events {
            return;
        }
        // Parser mutations have no native-callback boundary: flush before scripts.
        if iframe_events {
            for key in host.keys() {
                if let Some(bindings) = host.page_dom(key) {
                    bindings.flush_iframe_events();
                }
            }
        }
        if !resource_events {
            return;
        }
        let events = host.dom.borrow_mut().take_resource_events();
        let mut stylesheet_nodes = std::collections::HashSet::new();
        let mut image_nodes = std::collections::HashSet::new();
        let mut preload_nodes = std::collections::HashSet::new();
        for event in events {
            let document = host.dom.borrow().owner_document(event.node).ok().flatten();
            let target =
                host.keys().into_iter().find(|key| host.with_frame(*key, |f| f.document == document).unwrap_or(false));
            let Some(target) = target else { continue };
            let Some(bindings) = host.page_dom(target) else { continue };
            if event.kind == pyro_dom::script::ResourceKind::Stylesheet {
                if stylesheet_nodes.insert(event.node) {
                    self.stylesheet_event(target, event.node);
                }
            } else if event.kind == pyro_dom::script::ResourceKind::Img {
                if image_nodes.insert(event.node) {
                    self.image_event(target, event.node);
                }
            } else if event.kind == pyro_dom::script::ResourceKind::Preload {
                if preload_nodes.insert(event.node) {
                    self.preload_event(target, event.node);
                }
            } else if event.kind == pyro_dom::script::ResourceKind::Script {
                // Script-inserted (or `src`-changed) scripts: "already
                // started" makes repeats of one element no-ops.
                self.script_event(target, event.node);
            } else if event.kind == pyro_dom::script::ResourceKind::Meta {
                // A refresh pragma acts on insertion only.
                if event.reason == pyro_dom::script::ResourceReason::Connected {
                    self.meta_refresh_event(target, event.node);
                }
            } else {
                bindings.apply_style_event(&event);
            }
        }
    }
}

/// The script of a `javascript:` URL (HTML "evaluate a javascript: URL": the
/// percent-decoded rest of the URL), or `None` for any other URL.
fn javascript_url_source(url: &str) -> Option<String> {
    const SCHEME: &str = "javascript:";
    let rest = url.get(SCHEME.len()..).filter(|_| url[..SCHEME.len()].eq_ignore_ascii_case(SCHEME))?;
    Some(percent_encoding::percent_decode_str(rest).decode_utf8_lossy().into_owned())
}

/// Every about:blank document's markup.
const BLANK_MARKUP: &str = "<!doctype html><html><head></head><body></body></html>";
/// The nodes `BLANK_MARKUP` makes: the document, the doctype, html, head, body.
const BLANK_NODES: usize = 5;

/// An about:blank document has nodes beyond the ones it was made with: a
/// script or `Runtime.evaluate` filled it through DOM APIs (an inserted
/// iframe, whose documents are its own, included). A shadow tree is not
/// among the document's descendants: one attached to body (the only one of
/// the five that can host one) counts as well.
pub(crate) fn blank_document_has_content(page_dom: &PageDom) -> bool {
    let dom = page_dom.dom.borrow();
    let Ok(nodes) = dom.descendants(page_dom.document()) else { return true };
    nodes
        .take(BLANK_NODES + 1)
        .enumerate()
        .any(|(i, node)| i == BLANK_NODES || dom.shadow_root(node).is_ok_and(|root| root.is_some()))
}

pub(crate) fn create(
    host: &Rc<crate::frames::FrameHost>,
    page: FrameKey,
    url: &str,
    loader: &LoaderId,
) -> Result<Rc<PageDom>, JsError> {
    let frame = host.frame(page).ok_or(JsError::ContextDestroyed)?;
    let settings = host.settings.borrow().clone();
    let dom = host.dom.clone();
    let doc = if url == "about:blank" {
        pyro_dom::parse::parse_document(&dom, url, BLANK_MARKUP)
    } else {
        dom.borrow_mut().create_document(pyro_dom::DocumentKind::Html, url)
    };
    dom.borrow_mut().set_document_active(doc, true).map_err(|e| JsError::type_error(format!("{e:?}")))?;
    dom.borrow_mut().set_allow_declarative_shadow_roots(doc, true).map_err(|e| JsError::type_error(format!("{e:?}")))?;
    dom.borrow_mut().set_resource_tracking(true);
    dom.borrow_mut()
        .set_document_base_url(doc, matches!(url, "about:blank" | "about:srcdoc").then_some(frame.base_url.as_str()))
        .map_err(|e| JsError::type_error(format!("{e:?}")))?;
    let bindings = PageDom::new(dom, doc);
    bindings.set_storage_namespaces(settings.local, settings.session);
    let mut viewport = settings.viewport;
    // Only the main frame lays out as a phone; frames take their iframe's box.
    viewport.mobile &= page.child.is_none();
    bindings.set_viewport(viewport);
    if page.child.is_some() {
        bindings.set_frame_viewport_source(frame_viewport_source(host, &frame));
    }
    bindings.set_nested_frame(page.child.is_some());
    bindings.set_opaque_origin(frame.sandbox.opaque);
    bindings.set_max_touch_points(settings.max_touch_points);
    bindings
        .set_user_agent_override(settings.user_agent)
        .map_err(|e| JsError::type_error(format!("invalid request defaults: {e}")))?;
    bindings.set_extra_http_headers(settings.extra_headers);
    bindings.set_cache_disabled(settings.cache_disabled);
    bindings.set_bypass_service_worker(settings.bypass_service_worker);
    bindings.set_scripting_disabled(frame.sandbox.scripts_disabled);
    bindings.set_request_context(pyro_webapi::RequestContext {
        frame_id: Some(frame.id.0),
        loader_id: Some(loader.0.clone()),
    });
    // After the frame id: a navigation a service worker handled reserved
    // the new client's id under it.
    bindings.set_context_services(settings.services);
    bindings.create_window_target(doc);
    bindings.set_ready_state(doc, "loading");
    *bindings.net.borrow_mut() = settings.net;
    bindings.set_hooks(Some(Rc::new(Hooks {
        page,
        context: host.context,
        loader: loader.clone(),
        sched: host.sched.clone(),
        handle: host.handle.clone(),
        observer: host.observer.clone(),
        frames: Rc::downgrade(host),
    })));
    Ok(bindings)
}

/// A child document's viewport: its iframe's content box in the parent's
/// layout, with the parent viewport's other metrics. Read again only when
/// the parent's layout inputs changed; `None` before the parent lays the
/// iframe out (or while either document is borrowed).
fn frame_viewport_source(
    host: &Rc<crate::frames::FrameHost>,
    frame: &crate::Frame,
) -> Option<pyro_webapi::FrameViewportSource> {
    let owner = frame.owner?;
    let parent = host.key(frame.parent.as_ref()?)?;
    let parent = Rc::downgrade(&host.page_dom(parent)?);
    type Synced = (((u64, u64, u32, u32), u64), pyro_dom::media::Viewport);
    let last: std::cell::RefCell<Option<Synced>> = Default::default();
    Some(Rc::new(move || {
        let parent = parent.upgrade()?;
        let layout = parent.try_layout()?;
        let dom = parent.dom.try_borrow().ok()?;
        let stamp = layout.observation_stamp(&dom);
        if let Some((s, v)) = last.borrow().as_ref() {
            // The stamp covers the parent's viewport (a style generation).
            if *s == stamp {
                return Some(v.clone());
            }
        }
        let (width, height) = layout.content_size(&dom, owner)?;
        let mut viewport = layout.viewport().clone();
        viewport.width = width.round() as u32;
        viewport.height = height.round() as u32;
        viewport.mobile = false;
        *last.borrow_mut() = Some((stamp, viewport.clone()));
        Some(viewport)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PageEvent, PageId, PageObserver};
    use std::cell::{Cell, RefCell};

    type Seen = (PageId, ContextId, LoaderId, pyro_net::RequestId, pyro_net::NetworkDelivery);

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
            request: pyro_net::RequestId,
            frontier: pyro_net::NetworkDelivery,
        ) {
            self.calls.set(self.calls.get() + 1);
            self.seen.borrow_mut().push((page, context, loader.clone(), request, frontier));
            *self.cell.try_borrow_mut().expect("observer cell borrowed during the callback") = None;
        }
    }

    #[test]
    fn network_delivery_forwards_identity_without_holding_the_observer_cell() {
        let event_loop = pyro_runtime::EventLoop::<Browser>::new();
        let cell: ObserverCell = Rc::new(RefCell::new(None));
        let observer =
            Rc::new(SelfRemoving { cell: cell.clone(), calls: Cell::new(0), seen: RefCell::new(Vec::new()) });
        *cell.borrow_mut() = Some(observer.clone());
        let hooks = Hooks {
            page: PageId(3).into(),
            context: ContextId(7),
            loader: LoaderId("L1".into()),
            sched: event_loop.scheduler(),
            handle: event_loop.handle(),
            observer: cell.clone(),
            frames: std::rc::Weak::new(),
        };
        let frontier = pyro_net::NetworkDelivery { response: true, data_bytes: 5, terminal: false };
        hooks.network_delivery(pyro_net::RequestId(1), frontier);
        // The observer removed itself: later deliveries reach nobody.
        hooks.network_delivery(pyro_net::RequestId(1), pyro_net::NetworkDelivery { terminal: true, ..frontier });
        assert_eq!(observer.calls.get(), 1);
        assert_eq!(
            observer.seen.borrow().as_slice(),
            [(PageId(3), ContextId(7), LoaderId("L1".into()), pyro_net::RequestId(1), frontier)]
        );
        assert!(cell.borrow().is_none());
    }
}
