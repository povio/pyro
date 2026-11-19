//! `Page` domain and translation of `pyro_browser::PageEvent`s into CDP
//! events, following upstream `src/server/cdp/domains/page.zig` ordering.

use pyro_browser::{FrameId, FrameInfo, HistoryHandling, InitScript, NavigateOpts, NavigationReason, PageEvent};
use pyro_js::RealmId;
use serde::Deserialize;
use serde_json::json;

use super::runtime::{context_description, context_id};
use super::EngineHost;
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

/// `Page.setDocumentContent` in the frame's main world.
const SET_DOCUMENT_CONTENT: &str = "function (html) { const d = document; d.open(); d.write(html); d.close(); }";

/// `Page.close` with `beforeunload`: dispatches a cancelable event whose
/// `returnValue` is a string, as `BeforeUnloadEvent`'s is; returns whether a
/// handler asked to stay.
const FIRE_BEFOREUNLOAD: &str = "function () {
  let returnValue = '';
  const e = new Event('beforeunload', { cancelable: true });
  Object.defineProperty(e, 'returnValue', {
    get() { return returnValue; }, set(v) { returnValue = String(v); }, configurable: true,
  });
  dispatchEvent(e);
  return e.defaultPrevented || returnValue !== '';
}";

fn reason_name(reason: NavigationReason) -> Option<&'static str> {
    match reason {
        NavigationReason::AddressBar => None,
        NavigationReason::Anchor => Some("anchorClick"),
        NavigationReason::FormGet => Some("formSubmissionGet"),
        NavigationReason::FormPost => Some("formSubmissionPost"),
        NavigationReason::Script | NavigationReason::History => Some("scriptInitiated"),
        NavigationReason::InitialFrameNavigation => Some("initialFrameNavigation"),
    }
}

#[derive(Default)]
pub(crate) struct FrameEvents {
    parent: Option<FrameId>,
    pub(super) navigating: bool,
    /// A frameRequestedNavigation was announced for the pending navigation;
    /// an aborted navigation then reports frameClearedScheduledNavigation.
    requested: bool,
    pending_realms: Vec<(RealmId, Option<String>, String)>,
}

pub(crate) struct PendingNavigation {
    frame: FrameId,
    request: u64,
    session: String,
}

/// `Page.Frame.secureContextType` of a frame at `url` (its own URL only;
/// Chrome also reports an insecure ancestor).
fn secure_context_type(url: &str) -> &'static str {
    let Ok(url) = pyro_net::Url::parse(url) else { return "InsecureScheme" };
    if !pyro_net::is_potentially_trustworthy_url(&url) {
        return "InsecureScheme";
    }
    match url.scheme() {
        "http" | "ws" => "SecureLocalhost",
        _ => "Secure",
    }
}

/// `Page.Frame`. `unreachable`: the frame shows the error document of a
/// navigation to that URL (Chrome's `unreachableUrl`).
fn frame_description(
    id: &str,
    parent: Option<&FrameId>,
    loader: &str,
    url: &str,
    name: &str,
    origin: &str,
    unreachable: Option<&str>,
) -> serde_json::Value {
    let mut value = json!({
        "id": id, "loaderId": loader, "url": url, "name": name,
        "domainAndRegistry": "", "securityOrigin": if origin == "null" { "://" } else { origin },
        "mimeType": "text/html", "secureContextType": secure_context_type(url),
        "crossOriginIsolatedContextType": "NotIsolated", "gatedAPIFeatures": [],
    });
    if let Some(parent) = parent {
        value["parentId"] = json!(parent.0);
    }
    if let Some(unreachable) = unreachable {
        value["unreachableUrl"] = json!(unreachable);
    }
    value
}

fn frame_tree(frame: &FrameInfo, frames: &[FrameInfo]) -> serde_json::Value {
    let mut value = json!({"frame": frame_description(&frame.id.0, frame.parent.as_ref(), &frame.loader_id.0,
        &frame.url, &frame.name, &frame.origin, frame.unreachable_url.as_deref())});
    let children: Vec<_> =
        frames.iter().filter(|f| f.parent.as_ref() == Some(&frame.id)).map(|f| frame_tree(f, frames)).collect();
    if !children.is_empty() {
        value["childFrames"] = json!(children);
    }
    value
}

impl EngineHost {
    pub(super) fn page_domain(
        &mut self,
        action: &str,
        cmd: &PageCommand<'_, '_>,
    ) -> CdpResult<Option<serde_json::Value>> {
        match action {
            "enable" => {
                self.session_mut(cmd.session_id)?.page_enabled = true;
                self.sync_dialog_sessions(cmd.page);
                Ok(Some(json!({})))
            }
            "disable" => {
                self.session_mut(cmd.session_id)?.page_enabled = false;
                self.sync_dialog_sessions(cmd.page);
                Ok(Some(json!({})))
            }
            "getFrameTree" => {
                let page = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let frames = self.thread.browser.frames(page.id);
                let root =
                    frames.iter().find(|f| f.parent.is_none()).ok_or_else(|| CdpError::server("page is gone"))?;
                let mut tree = frame_tree(root, &frames);
                if page.initial_popup {
                    // Chrome's uncommitted initial popup document (see Host::set_initial_popup).
                    tree["frame"]["url"] = json!(":");
                }
                Ok(Some(json!({"frameTree": tree})))
            }
            "setLifecycleEventsEnabled" => {
                #[derive(Deserialize)]
                struct P {
                    enabled: bool,
                }
                let enabled = cmd.command.params::<P>()?.enabled;
                self.session_mut(cmd.session_id)?.lifecycle_events = enabled;
                if enabled {
                    let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                    let now = self.thread.browser.timestamp();
                    for frame in self.thread.browser.frames(state.id) {
                        if state.frames.get(&frame.id).is_some_and(|f| f.navigating) {
                            continue;
                        }
                        for (name, reached) in [
                            ("DOMContentLoaded", frame.dom_content_loaded),
                            ("load", frame.loaded),
                            ("networkAlmostIdle", frame.network_idle.almost_idle),
                            ("networkIdle", frame.network_idle.idle),
                        ] {
                            if reached {
                                self.lifecycle(cmd.session_id, name, &frame.id.0, &frame.loader_id.0, now);
                            }
                        }
                    }
                }
                Ok(Some(json!({})))
            }
            "addScriptToEvaluateOnNewDocument" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    source: String,
                    world_name: Option<String>,
                    #[serde(default)]
                    run_immediately: bool,
                }
                let p: P = cmd.command.params()?;
                let world = p.world_name.filter(|w| !w.is_empty());
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let id = state.next_script_id.to_string();
                state.next_script_id += 1;
                let page_id = state.id;
                if p.run_immediately {
                    self.run_now(cmd.page, &p.source, world.as_deref(), &id)?;
                }
                self.thread.browser.add_init_script(page_id, InitScript { id: id.clone(), source: p.source, world });
                Ok(Some(json!({ "identifier": id })))
            }
            "removeScriptToEvaluateOnNewDocument" => {
                #[derive(Deserialize)]
                struct P {
                    identifier: String,
                }
                let identifier = cmd.command.params::<P>()?.identifier;
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                self.thread.browser.remove_init_script(page_id, &identifier);
                Ok(Some(json!({})))
            }
            "createIsolatedWorld" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    frame_id: String,
                    world_name: String,
                }
                let p: P = cmd.command.params()?;
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                let realm = self
                    .thread
                    .browser
                    .isolated_world_in_frame(page_id, &FrameId(p.frame_id), &p.world_name)
                    .map_err(|e| CdpError::server(format!("cannot create isolated world: {e}")))?;
                Ok(Some(json!({ "executionContextId": context_id(realm.id()) })))
            }
            "navigate" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    url: String,
                    referrer: Option<String>,
                    frame_id: Option<String>,
                }
                let p: P = cmd.command.params()?;
                let (root, _, _) = self.main_frame(cmd.page)?;
                let frame = FrameId(p.frame_id.unwrap_or(root));
                self.start_navigation(cmd, frame, p.url, p.referrer, HistoryHandling::Push)
            }
            "reload" => {
                let (frame, _, url) = self.main_frame(cmd.page)?;
                self.start_navigation(cmd, FrameId(frame), url, None, HistoryHandling::Reload)
            }
            "getNavigationHistory" => {
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                let p = self.thread.browser.page(page_id).ok_or_else(|| CdpError::server("page is gone"))?;
                let (index, entries) = p.nav.history_entries();
                // The current entry's title is read from its document now;
                // titles of other entries are not kept.
                let title = p.main_frame.page_dom.as_ref().map(|d| d.document_title()).unwrap_or_default();
                let entries: Vec<_> = entries
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let title = if i == index { title.as_str() } else { "" };
                        json!({ "id": e.id, "url": e.url, "userTypedURL": e.url, "title": title, "transitionType": "other" })
                    })
                    .collect();
                Ok(Some(json!({ "currentIndex": index, "entries": entries })))
            }
            "navigateToHistoryEntry" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    entry_id: u64,
                }
                let p: P = cmd.command.params()?;
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                let page = self.thread.browser.page(page_id).ok_or_else(|| CdpError::server("page is gone"))?;
                // Like Chrome, the traversal is only started here: its commit is
                // reported by navigation events, not by this response.
                if !page.nav.traverse_to_entry(p.entry_id) {
                    return Err(CdpError::invalid_params("No entry with passed id"));
                }
                Ok(Some(json!({})))
            }
            "getLayoutMetrics" => self.layout_metrics(cmd),
            #[cfg(feature = "render")]
            "captureScreenshot" => self.capture_screenshot(cmd),
            #[cfg(feature = "render")]
            "printToPDF" => self.print_to_pdf(cmd),
            #[cfg(not(feature = "render"))]
            "captureScreenshot" => {
                Err(CdpError::server("Page.captureScreenshot: pyro was built without the render feature"))
            }
            #[cfg(not(feature = "render"))]
            "printToPDF" => {
                Err(CdpError::server("Page.printToPDF: pyro was built without the render feature"))
            }
            // screencast.rs
            "startScreencast" => self.start_screencast(cmd),
            "screencastFrameAck" => self.screencast_frame_ack(cmd),
            "stopScreencast" => self.stop_screencast(cmd),
            // A blocking dialog is answered by its nested read loop
            // (dialog.rs). A reported one was already dismissed (it did not
            // block); this only answers the client that reacts to its
            // javascriptDialogOpening.
            "handleJavaScriptDialog" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                #[allow(dead_code)]
                struct P {
                    accept: bool,
                    prompt_text: Option<String>,
                }
                cmd.command.params::<P>()?;
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                if state.unanswered_dialogs == 0 {
                    return Err(CdpError::invalid_params("No dialog is showing"));
                }
                state.unanswered_dialogs -= 1;
                Ok(Some(json!({})))
            }
            "setInterceptFileChooserDialog" => {
                #[derive(Deserialize)]
                struct P {
                    enabled: bool,
                }
                let enabled = cmd.command.params::<P>()?.enabled;
                self.session_mut(cmd.session_id)?.intercept_file_chooser = enabled;
                Ok(Some(json!({})))
            }
            // From the page's next document on, as in Chrome (Playwright sends
            // it before the first navigation): see `Browser::set_bypass_csp`.
            "setBypassCSP" => {
                #[derive(Deserialize)]
                struct P {
                    enabled: bool,
                }
                let p = cmd.command.params::<P>()?;
                let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                if !self.thread.browser.set_bypass_csp(id, p.enabled) {
                    return Err(CdpError::server("page is gone"));
                }
                Ok(Some(json!({})))
            }
            // A single foreground page: nothing to change.
            "bringToFront" => Ok(Some(json!({}))),
            // Like document.open(); document.write(html); document.close() in
            // the frame's main world (Chrome keeps the URL too).
            "setDocumentContent" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    frame_id: String,
                    html: String,
                }
                let p: P = cmd.command.params()?;
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                let frame = self
                    .thread
                    .browser
                    .frame(page_id, &FrameId(p.frame_id))
                    .ok_or_else(|| CdpError::server("No frame for given id found"))?;
                let realm = frame
                    .main_realm()
                    .cloned()
                    .ok_or_else(|| CdpError::server("No Document instance to set its content"))?;
                // Chrome restarts the document's lifecycle under its loader:
                // init, then DOMContentLoaded and load as the write finishes.
                let now = self.thread.browser.timestamp();
                for s in self.sessions_of(cmd.page, |s| s.page_enabled && s.lifecycle_events) {
                    self.lifecycle(&s, "init", &frame.id.0, &frame.loader_id.0, now);
                }
                let scope = realm.scope().map_err(super::remote::js_to_cdp)?;
                let mut args = pyro_js::RootedVec::new();
                args.push(&scope, scope.string(&p.html));
                scope
                    .call_function_rooted(SET_DOCUMENT_CONTENT, None, &args)
                    .map_err(|e| CdpError::server(e.details(&scope).message))?;
                Ok(Some(json!({})))
            }
            "close" => self.close_running_beforeunload(cmd),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// `Page.close`: fires `beforeunload` at the main window first. When a
    /// handler cancels it (`preventDefault()` or a `returnValue`), Chrome
    /// asks with a "beforeunload" dialog: answered by
    /// `Page.handleJavaScriptDialog` while a session has Page enabled
    /// (dialog.rs), the page closes on accept and stays on dismiss. Without
    /// such a session it closes. Chrome additionally shows the dialog only
    /// after a user gesture on the page; the engine tracks no user
    /// activation, so any cancelling handler asks.
    fn close_running_beforeunload(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let (frame_id, url, realm) = {
            let page = self.thread.browser.page(page_id).ok_or_else(|| CdpError::server("page is gone"))?;
            (page.main_frame.id.0.clone(), page.main_frame.url.clone(), page.main_frame.main_realm().cloned())
        };
        // Chrome answers Page.close before beforeunload runs.
        self.reply(cmd, &json!({}));
        let cancelled = realm.is_some_and(|realm| {
            realm.scope().is_ok_and(|scope| {
                match scope.call_function_rooted(FIRE_BEFOREUNLOAD, None, &pyro_js::RootedVec::new()) {
                    Ok(v) => v.to_bool(),
                    Err(e) => {
                        scope.report_exception(&e);
                        false
                    }
                }
            })
        });
        let mut close = true;
        if cancelled && self.collector.dialogs.would_block(page_id) {
            // Events the handlers caused go out ahead of the dialog.
            self.thread.browser.checkpoint();
            self.flush();
            let answer = self.collector.dialogs.show(&self.out, page_id, &frame_id, &url, "beforeunload", "", "");
            close = answer.is_none_or(|a| a.accept);
        }
        if close {
            self.close_requests.push(cmd.page);
        }
        Ok(None)
    }

    /// Keeps the opener's `popup.location.href` current.
    fn popup_url_committed(&self, page: PageKey, url: &str) {
        if let Some(ticket) = self.pages.get(&page).and_then(|state| state.popup_ticket.as_ref()) {
            ticket.set_url(url);
        }
    }

    fn main_frame(&self, page: PageKey) -> CdpResult<(String, String, String)> {
        let state = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?;
        let p = self.thread.browser.page(state.id).ok_or_else(|| CdpError::server("page is gone"))?;
        Ok((p.main_frame.id.0.clone(), p.main_frame.loader_id.0.clone(), p.main_frame.url.clone()))
    }

    /// `runImmediately`: evaluates in the current document, creating the
    /// isolated world first when it does not exist yet (its context is
    /// announced like any other).
    fn run_now(&mut self, page: PageKey, source: &str, world: Option<&str>, id: &str) -> CdpResult<()> {
        let page_id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        for info in self.thread.browser.frames(page_id) {
            // An earlier script can synchronously remove a later frame from
            // this snapshot. Skip it without failing future installation.
            let Some(main) = self.thread.browser.with_frame(page_id, &info.id, |f| f.main_realm().cloned()) else {
                continue;
            };
            let realm = match world {
                Some(name) => Some(
                    self.thread
                        .browser
                        .isolated_world_in_frame(page_id, &info.id, name)
                        .map_err(|e| CdpError::server(format!("cannot create isolated world: {e}")))?,
                ),
                None => main,
            };
            if let Some(realm) = realm {
                if let Ok(scope) = realm.scope() {
                    if let Err(e) = scope.eval(source, Some(&format!("init-script:{id}")), 1) {
                        scope.report_exception(&e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Starts a navigation whose response is sent when it commits or fails.
    fn start_navigation(
        &mut self,
        cmd: &PageCommand<'_, '_>,
        frame: FrameId,
        url: String,
        referrer: Option<String>,
        history: HistoryHandling,
    ) -> CdpResult<Option<serde_json::Value>> {
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let page_id = state.id;
        let native = self
            .thread
            .browser
            .frame(page_id, &frame)
            .ok_or_else(|| CdpError::server("Frame with the given id does not belong to the target."))?;
        state.pending_navigations.push(PendingNavigation {
            frame: frame.clone(),
            request: cmd.command.id,
            session: cmd.session_id.to_owned(),
        });
        let opts = NavigateOpts {
            post: None,
            url,
            reason: NavigationReason::AddressBar,
            cdp_request_id: Some(cmd.command.id),
            referrer,
            referrer_policy: None,
        };
        let result = if native.parent.is_some() {
            self.thread.browser.navigate_frame(page_id, &frame, opts)
        } else {
            self.thread.browser.navigate_with_handling(page_id, opts, history)
        };
        if let Err(error) = result {
            // Reject immediately; a queued FrameNavigateFailed may still emit
            // lifecycle notifications but cannot send a duplicate reply.
            self.pages
                .get_mut(&cmd.page)
                .expect("page exists")
                .pending_navigations
                .retain(|p| p.request != cmd.command.id || p.session != cmd.session_id);
            return Err(CdpError::server(format!("navigation failed: {error}")));
        }
        Ok(None)
    }

    fn lifecycle(&self, session_id: &str, name: &str, frame_id: &str, loader_id: &str, timestamp: f64) {
        self.event(
            "Page.lifecycleEvent",
            &json!({ "frameId": frame_id, "loaderId": loader_id, "name": name, "timestamp": timestamp }),
            session_id,
        );
    }

    /// Sends a `Page.navigate`/`Page.reload` response on the session that issued it.
    fn navigate_response(&mut self, page: PageKey, frame: &FrameId, request_id: u64, result: serde_json::Value) {
        let Some(state) = self.pages.get_mut(&page) else { return };
        let Some(pos) = state.pending_navigations.iter().position(|p| p.request == request_id && p.frame == *frame)
        else {
            return;
        };
        let session = state.pending_navigations.swap_remove(pos).session;
        if self.sessions.contains_key(&session) {
            self.out.push(msg::result_frame(request_id, &result, Some(&session)));
        }
    }

    pub(super) fn on_page_event(&mut self, page: PageKey, event: PageEvent) {
        // Page-domain events go only to sessions that enabled Page (Chrome
        // semantics); lifecycle additionally needs setLifecycleEventsEnabled.
        let all = self.sessions_of(page, |s| s.page_enabled);
        let lifecycle = self.sessions_of(page, |s| s.page_enabled && s.lifecycle_events);
        let runtime = self.sessions_of(page, |s| s.runtime_enabled);
        let root = self.pages.get(&page).and_then(|p| self.thread.browser.page(p.id)).map(|p| p.main_frame.id.clone());
        match event {
            PageEvent::FrameNavigate { frame_id, loader_id, url, reason, cdp_request_id, .. } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    let frame = state.frames.entry(frame_id.clone()).or_default();
                    frame.navigating = true;
                    frame.requested = reason_name(reason).is_some();
                    frame.pending_realms.clear();
                    if root.as_ref() == Some(&frame_id) {
                        state.network.bodies.clear();
                    } else {
                        state.network.bodies.clear_frame(&frame_id.0);
                    }
                    let mut retained = Vec::new();
                    for pending in state.pending_navigations.drain(..) {
                        if pending.frame == frame_id && Some(pending.request) != cdp_request_id {
                            self.out.push(msg::result_frame(
                                pending.request,
                                &json!({"frameId":frame_id.0,"errorText":"Navigation superseded"}),
                                Some(&pending.session),
                            ));
                        } else {
                            retained.push(pending);
                        }
                    }
                    state.pending_navigations = retained;
                }
                for s in &all {
                    if let Some(reason) = reason_name(reason) {
                        if reason != "initialFrameNavigation" {
                            self.event(
                                "Page.frameScheduledNavigation",
                                &json!({ "frameId": frame_id.0, "delay": 0, "reason": reason, "url": url }),
                                s,
                            );
                        }
                        self.event(
                            "Page.frameRequestedNavigation",
                            &json!({ "frameId": frame_id.0, "reason": reason, "url": url, "disposition": "currentTab" }),
                            s,
                        );
                    }
                    self.event(
                        "Page.frameStartedNavigating",
                        &json!({ "frameId": frame_id.0, "url": url, "loaderId": loader_id.0, "navigationType": "differentDocument" }),
                        s,
                    );
                    self.event("Page.frameStartedLoading", &json!({ "frameId": frame_id.0 }), s);
                }
            }
            PageEvent::RealmDestroyed { frame_id, realm } => {
                // Evaluations still awaiting a promise in it can never settle.
                self.pending.fail_context(context_id(realm));
                // Release handles even if their session never enabled Page.
                for s in self.sessions_of(page, |_| true) {
                    if let Some(session) = self.sessions.get(&s) {
                        session.handles.borrow_mut().release_context(context_id(realm));
                    }
                }
                let navigating =
                    self.pages.get(&page).and_then(|p| p.frames.get(&frame_id)).is_some_and(|f| f.navigating);
                if !navigating || root.as_ref() != Some(&frame_id) {
                    for s in &runtime {
                        self.event(
                            "Runtime.executionContextDestroyed",
                            &json!({ "executionContextId": context_id(realm), "executionContextUniqueId": realm.0.to_string() }),
                            s,
                        );
                    }
                }
            }
            PageEvent::RealmCreated { frame_id, realm, world, origin } => {
                let navigating =
                    self.pages.get(&page).and_then(|p| p.frames.get(&frame_id)).is_some_and(|f| f.navigating);
                if navigating {
                    if let Some(state) = self.pages.get_mut(&page) {
                        state.frames.entry(frame_id.clone()).or_default().pending_realms.push((realm, world, origin));
                    }
                } else {
                    let context = context_description(realm, world.as_deref(), &origin, &frame_id.0);
                    self.event_to("Runtime.executionContextCreated", &json!({ "context": context }), &runtime);
                }
            }
            PageEvent::FrameNavigated {
                frame_id,
                loader_id,
                url,
                reason,
                cdp_request_id,
                timestamp,
                unreachable_url,
            } => {
                if root.as_ref() == Some(&frame_id) {
                    if let Some(state) = self.pages.get_mut(&page) {
                        state.uncommitted_popup_of = None;
                    }
                    self.popup_url_committed(page, &url);
                    for session in self.sessions_of(page, |s| s.dom_enabled) {
                        self.event("DOM.documentUpdated", &json!({}), &session);
                    }
                }
                if let Some(id) = cdp_request_id {
                    self.navigate_response(
                        page,
                        &frame_id,
                        id,
                        json!({ "frameId": frame_id.0, "loaderId": loader_id.0 }),
                    );
                }
                for s in &lifecycle {
                    self.lifecycle(s, "init", &frame_id.0, &loader_id.0, timestamp);
                }
                if reason_name(reason).is_some() {
                    for s in &all {
                        self.event("Page.frameClearedScheduledNavigation", &json!({ "frameId": frame_id.0 }), s);
                    }
                }
                if root.as_ref() == Some(&frame_id) {
                    for s in &runtime {
                        self.event("Runtime.executionContextsCleared", &json!({}), s);
                    }
                }
                let (parent, pending) = match self.pages.get_mut(&page) {
                    Some(state) => {
                        let frame = state.frames.entry(frame_id.clone()).or_default();
                        frame.navigating = false;
                        (frame.parent.clone(), std::mem::take(&mut frame.pending_realms))
                    }
                    None => (None, Vec::new()),
                };
                let info = self.pages.get(&page).and_then(|p| {
                    self.thread.browser.with_frame(p.id, &frame_id, |f| (f.origin.clone(), f.name.clone()))
                });
                let origin = pending
                    .iter()
                    .find(|(_, world, _)| world.is_none())
                    .map(|(_, _, origin)| origin.as_str())
                    .or_else(|| info.as_ref().map(|(origin, _)| origin.as_str()))
                    .unwrap_or("null");
                let name = info.as_ref().map_or("", |(_, name)| name.as_str());
                let frame = frame_description(
                    &frame_id.0,
                    parent.as_ref(),
                    &loader_id.0,
                    &url,
                    name,
                    origin,
                    unreachable_url.as_deref(),
                );
                self.event_to("Page.frameNavigated", &json!({"frame":frame,"type":"Navigation"}), &all);
                for (realm, world, origin) in pending {
                    let context = context_description(realm, world.as_deref(), &origin, &frame_id.0);
                    self.event_to("Runtime.executionContextCreated", &json!({ "context": context }), &runtime);
                }
            }
            PageEvent::FrameNavigateFailed { frame_id, loader_id, error, cdp_request_id, error_page, .. } => {
                if let Some(id) = cdp_request_id {
                    self.navigate_response(
                        page,
                        &frame_id,
                        id,
                        json!({ "frameId": frame_id.0, "loaderId": loader_id.0, "errorText": error }),
                    );
                }
                // Chrome answers Page.navigate with the error, then commits
                // its error document under the same loader: the frame is
                // still navigating, and that document's load ends it.
                if error_page {
                    return;
                }
                let mut requested = false;
                if let Some(state) = self.pages.get_mut(&page) {
                    let frame = state.frames.entry(frame_id.clone()).or_default();
                    frame.navigating = false;
                    requested = std::mem::take(&mut frame.requested);
                    frame.pending_realms.clear();
                }
                for s in &all {
                    // Chrome clears a requested (anchor/script) navigation it
                    // aborted, e.g. when the response became a download.
                    if requested {
                        self.event("Page.frameClearedScheduledNavigation", &json!({ "frameId": frame_id.0 }), s);
                    }
                    self.event("Page.frameStoppedLoading", &json!({ "frameId": frame_id.0 }), s);
                }
            }
            PageEvent::FrameDomContentLoaded { frame_id, loader_id, timestamp } => {
                if root.as_ref() == Some(&frame_id) {
                    for s in &all {
                        self.event("Page.domContentEventFired", &json!({ "timestamp": timestamp }), s);
                    }
                }
                for s in &lifecycle {
                    self.lifecycle(s, "DOMContentLoaded", &frame_id.0, &loader_id.0, timestamp);
                }
            }
            PageEvent::FrameLoaded { frame_id, loader_id, timestamp } => {
                if root.as_ref() == Some(&frame_id) {
                    for s in &all {
                        self.event("Page.loadEventFired", &json!({ "timestamp": timestamp }), s);
                    }
                }
                for s in &lifecycle {
                    self.lifecycle(s, "load", &frame_id.0, &loader_id.0, timestamp);
                }
                for s in &all {
                    self.event("Page.frameStoppedLoading", &json!({ "frameId": frame_id.0 }), s);
                }
            }
            PageEvent::FrameNetworkAlmostIdle { frame_id, loader_id } => {
                let now = self.thread.browser.timestamp();
                for s in &lifecycle {
                    self.lifecycle(s, "networkAlmostIdle", &frame_id.0, &loader_id.0, now);
                }
            }
            PageEvent::FrameNetworkIdle { frame_id, loader_id } => {
                let now = self.thread.browser.timestamp();
                for s in &lifecycle {
                    self.lifecycle(s, "networkIdle", &frame_id.0, &loader_id.0, now);
                }
            }
            PageEvent::FrameNavigatedWithinDocument { frame_id, url, .. } => {
                if root.as_ref() == Some(&frame_id) {
                    self.popup_url_committed(page, &url);
                }
                for s in &all {
                    self.event("Page.navigatedWithinDocument", &json!({ "frameId": frame_id.0, "url": url }), s);
                }
            }
            PageEvent::FrameChildFrameCreated { parent, frame_id } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    state.frames.insert(
                        frame_id.clone(),
                        FrameEvents {
                            parent: Some(parent.clone()),
                            navigating: true,
                            requested: false,
                            pending_realms: Vec::new(),
                        },
                    );
                }
                for s in &all {
                    self.event("Page.frameAttached", &json!({ "frameId": frame_id.0, "parentFrameId": parent.0 }), s);
                }
            }
            PageEvent::FrameChildFrameDetached { frame_id, .. } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    state.frames.remove(&frame_id);
                    state.network.bodies.clear_frame(&frame_id.0);
                    state.pending_navigations.retain(|p| {
                        if p.frame != frame_id {
                            return true;
                        }
                        self.out.push(msg::result_frame(
                            p.request,
                            &json!({"frameId":frame_id.0,"errorText":"Frame detached"}),
                            Some(&p.session),
                        ));
                        false
                    });
                }
                for s in &all {
                    self.event("Page.frameDetached", &json!({ "frameId": frame_id.0, "reason": "remove" }), s);
                }
            }
            PageEvent::PopupRequested { url, opener, ticket, .. } => {
                self.popup_requests.push(crate::target::PopupRequest {
                    opener: page,
                    url,
                    opener_access: opener,
                    handle: ticket.map(|t| t as std::rc::Rc<dyn std::any::Any>),
                });
            }
            PageEvent::PopupCommand { popup, command } => {
                self.popup_commands.push((popup, command));
            }
            PageEvent::CloseRequested { .. } => {
                self.close_requests.push(page);
            }
            PageEvent::FileChooserOpened { frame_id, input, multiple } => {
                self.file_chooser_opened(page, &frame_id, input, multiple);
            }
            PageEvent::DownloadStarted => {
                let owner = self.download_owner(page);
                if let Some(state) = self.pages.get_mut(&owner) {
                    state.download_count += 1;
                }
            }
            PageEvent::Download { frame_id, url, suggested_filename, body } => {
                let transient = self.pages.get(&page).is_some_and(|state| state.uncommitted_popup_of.is_some());
                let owner = self.download_owner(page);
                if let Some(state) = self.pages.get_mut(&owner) {
                    state.downloads.push(super::DownloadRecord {
                        url: url.clone(),
                        filename: suggested_filename.clone(),
                        body: body.clone(),
                    });
                    // Keep the newest bodies within the budget.
                    let mut kept = 0usize;
                    for d in state.downloads.iter_mut().rev() {
                        let len = d.body.as_ref().map_or(0, |b| b.len());
                        if kept + len > super::DOWNLOAD_BUDGET {
                            d.body = Err("evicted".into());
                        } else {
                            kept += len;
                        }
                    }
                    self.waiters.log_changed = true;
                }
                // Announced with the frame that started it (the popup's, as
                // in Chrome): clients report it on the opener.
                self.deliver_download(page, &frame_id.0, &url, &suggested_filename, body);
                if transient {
                    self.close_requests.push(page);
                }
            }
            PageEvent::JavascriptDialogOpening { kind, message, default_prompt, .. } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    state.unanswered_dialogs = state.unanswered_dialogs.saturating_add(1).min(16);
                }
                let (_, _, url) = self.main_frame(page).unwrap_or_default();
                for s in &all {
                    self.event(
                        "Page.javascriptDialogOpening",
                        &json!({
                            "url": url, "message": message, "type": kind,
                            "hasBrowserHandler": false, "defaultPrompt": default_prompt.clone().unwrap_or_default(),
                        }),
                        s,
                    );
                }
            }
            PageEvent::JavascriptDialogClosed { frame_id } => {
                for s in &all {
                    self.event(
                        "Page.javascriptDialogClosed",
                        &json!({ "frameId": frame_id.0, "result": false, "userInput": "" }),
                        s,
                    );
                }
            }
            PageEvent::FrameDestroyed { .. } => {}
        }
    }
}

impl EngineHost {
    /// The page a download of `page` belongs to (its `Pyro.run` download
    /// steps): the opener of a popup that committed no document of its own
    /// (a `target=_blank` link or `window.open` whose response is an
    /// attachment), as Playwright reports such a download on the opener,
    /// else `page`. Chrome closes a tab it opened for a download; the
    /// caller closes the popup once it has delivered the download.
    fn download_owner(&self, page: PageKey) -> PageKey {
        self.pages
            .get(&page)
            .and_then(|state| state.uncommitted_popup_of)
            .filter(|opener| self.pages.contains_key(opener))
            .unwrap_or(page)
    }

    /// Delivers a page download as Chrome does under an allow behavior:
    /// Browser.downloadWillBegin, the file written to downloadPath (named by
    /// guid for allowAndName, else as suggested), then Browser.downloadProgress
    /// inProgress and completed (with filePath), or canceled on failure. Deny
    /// and default behaviors write nothing (headless Chrome's default denies
    /// too) and report the download canceled, which is where Playwright's
    /// `acceptDownloads: false` gets its `download` event. Events go to the
    /// browser session and only when eventsEnabled.
    fn deliver_download(
        &mut self,
        page: PageKey,
        frame_id: &str,
        url: &str,
        suggested_filename: &str,
        body: Result<bytes::Bytes, String>,
    ) {
        let context = self.pages.get(&page).map(|p| p.context);
        let Some(behavior) =
            self.download_behavior.get(&context).or_else(|| self.download_behavior.get(&None)).cloned()
        else {
            return;
        };
        let guid = new_guid();
        if let (true, Some(context)) = (behavior.events_enabled, context) {
            let announced = self.announced_downloads.entry(context).or_default();
            if announced.len() >= super::ANNOUNCED_DOWNLOADS {
                announced.pop_front();
            }
            announced.push_back(guid.clone());
        }
        let emit = |host: &Self, method: &str, params: serde_json::Value| {
            if behavior.events_enabled {
                host.out.push(crate::msg::event_frame(method, &params, None));
            }
        };
        emit(
            self,
            "Browser.downloadWillBegin",
            json!({"frameId": frame_id, "guid": guid, "url": url, "suggestedFilename": suggested_filename}),
        );
        let allowed = matches!(behavior.behavior.as_str(), "allow" | "allowAndName");
        let Some(dir) = behavior.download_path.clone().filter(|_| allowed) else {
            // Measured on headless Chromium: nothing received, then canceled.
            for state in ["inProgress", "canceled"] {
                emit(
                    self,
                    "Browser.downloadProgress",
                    json!({"guid": guid, "totalBytes": 0, "receivedBytes": 0, "state": state}),
                );
            }
            return;
        };
        let name = if behavior.behavior == "allowAndName" { guid.clone() } else { suggested_filename.to_owned() };
        let path = std::path::Path::new(&dir).join(name);
        let written = body.and_then(|bytes| {
            std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(&path, &bytes))
                .map(|_| bytes.len())
                .map_err(|e| e.to_string())
        });
        match written {
            Ok(total) => {
                emit(
                    self,
                    "Browser.downloadProgress",
                    json!({"guid": guid, "totalBytes": total, "receivedBytes": 0, "state": "inProgress"}),
                );
                emit(
                    self,
                    "Browser.downloadProgress",
                    json!({"guid": guid, "totalBytes": total, "receivedBytes": total, "state": "completed", "filePath": path.to_string_lossy()}),
                );
            }
            Err(_) => emit(
                self,
                "Browser.downloadProgress",
                json!({"guid": guid, "totalBytes": 0, "receivedBytes": 0, "state": "canceled"}),
            ),
        }
    }
}

/// A random version-4 UUID, as Chrome uses for download guids.
fn new_guid() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        b = nanos.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

#[cfg(test)]
mod secure_context_tests {
    #[test]
    fn secure_context_types() {
        for (url, want) in [
            ("https://a.test/", "Secure"),
            ("about:blank", "Secure"),
            ("http://app.localhost:3000/", "SecureLocalhost"),
            ("http://127.0.0.1/", "SecureLocalhost"),
            ("http://a.test/", "InsecureScheme"),
        ] {
            assert_eq!(super::secure_context_type(url), want, "{url}");
        }
    }
}
