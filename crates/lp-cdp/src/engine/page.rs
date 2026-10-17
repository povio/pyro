//! `Page` domain and translation of `lp_browser::PageEvent`s into CDP
//! events, following upstream `src/server/cdp/domains/page.zig` ordering.

use lp_browser::{InitScript, NavigateOpts, NavigationReason, PageEvent};
use serde::Deserialize;
use serde_json::json;

use super::runtime::{context_description, context_id};
use super::EngineHost;
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

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

fn security_origin(url: &str) -> &str {
    // Opaque origins (about:blank before any load) report an empty origin.
    if url.starts_with("about:") {
        ""
    } else {
        url
    }
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
                Ok(Some(json!({})))
            }
            "disable" => {
                self.session_mut(cmd.session_id)?.page_enabled = false;
                Ok(Some(json!({})))
            }
            "getFrameTree" => {
                let (frame_id, loader_id, url) = self.main_frame(cmd.page)?;
                Ok(Some(json!({
                    "frameTree": {
                        "frame": {
                            "id": frame_id,
                            "loaderId": loader_id,
                            "url": url,
                            "domainAndRegistry": "",
                            "securityOrigin": security_origin(&url),
                            "mimeType": "text/html",
                            "secureContextType": "InsecureScheme",
                            "crossOriginIsolatedContextType": "NotIsolated",
                            "gatedAPIFeatures": [],
                        },
                    },
                })))
            }
            "setLifecycleEventsEnabled" => {
                #[derive(Deserialize)]
                struct P {
                    enabled: bool,
                }
                let enabled = cmd.command.params::<P>()?.enabled;
                let navigating = self.pages.get(&cmd.page).is_some_and(|p| p.navigating);
                self.session_mut(cmd.session_id)?.lifecycle_events = enabled;
                if enabled && !navigating {
                    // The current document is fully loaded (only about:blank
                    // loads today and it makes no requests): replay its
                    // lifecycle like upstream does for a completed frame.
                    let (frame_id, loader_id, _) = self.main_frame(cmd.page)?;
                    let now = self.thread.browser.timestamp();
                    for name in ["DOMContentLoaded", "load", "networkAlmostIdle", "networkIdle"] {
                        self.lifecycle(cmd.session_id, name, &frame_id, &loader_id, now);
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
                let (frame_id, _, _) = self.main_frame(cmd.page)?;
                if p.frame_id != frame_id {
                    return Err(CdpError::server("Frame with the given id does not belong to the target."));
                }
                let page_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                let realm = self
                    .thread
                    .browser
                    .isolated_world(page_id, &p.world_name)
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
                let (frame_id, _, _) = self.main_frame(cmd.page)?;
                if p.frame_id.as_ref().is_some_and(|f| *f != frame_id) {
                    return Err(CdpError::unsupported("Page.navigate of a child frame"));
                }
                self.start_navigation(cmd, p.url, p.referrer, NavigationReason::AddressBar)
            }
            "reload" => {
                let (_, _, url) = self.main_frame(cmd.page)?;
                self.start_navigation(cmd, url, None, NavigationReason::AddressBar)
            }
            // Accepted because there is nothing to change: the engine enforces
            // no CSP, opens no file chooser and has a single foreground page.
            "setBypassCSP" | "setInterceptFileChooserDialog" | "bringToFront" => Ok(Some(json!({}))),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    fn main_frame(&self, page: PageKey) -> CdpResult<(String, String, String)> {
        let state = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?;
        let p = self.thread.browser.page(state.id).ok_or_else(|| CdpError::server("page is gone"))?;
        Ok((p.main_frame.id.0.clone(), p.main_frame.loader_id.0.clone(), p.main_frame.url.clone()))
    }

    fn run_now(&self, page: PageKey, source: &str, world: Option<&str>, id: &str) -> CdpResult<()> {
        let state = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?;
        let Some(p) = self.thread.browser.page(state.id) else { return Ok(()) };
        let realm = match world {
            Some(name) => p.main_frame.world(name).cloned(),
            None => p.main_frame.main_realm().cloned(),
        };
        if let Some(realm) = realm {
            if let Ok(scope) = realm.scope() {
                if let Err(e) = scope.eval(source, Some(&format!("init-script:{id}")), 1) {
                    scope.report_exception(&e);
                }
            }
        }
        Ok(())
    }

    /// Starts a navigation whose response is sent when it commits or fails.
    fn start_navigation(
        &mut self,
        cmd: &PageCommand<'_, '_>,
        url: String,
        referrer: Option<String>,
        reason: NavigationReason,
    ) -> CdpResult<Option<serde_json::Value>> {
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        state.pending_navigations.push((cmd.command.id, cmd.session_id.to_owned()));
        let page_id = state.id;
        let opts = NavigateOpts { url, reason, cdp_request_id: Some(cmd.command.id), referrer };
        // Failures are reported through FrameNavigateFailed, which carries the
        // request id and produces the response (with errorText), as Chrome does.
        if let Err(error) = self.thread.browser.navigate(page_id, opts) {
            let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
            let still_pending = state.pending_navigations.iter().any(|(id, _)| *id == cmd.command.id);
            let failed_event_queued = self.has_pending_events();
            if still_pending && !failed_event_queued {
                // Rejected before the navigation started (no event will answer).
                let state = self.pages.get_mut(&cmd.page).expect("checked above");
                state.pending_navigations.retain(|(id, _)| *id != cmd.command.id);
                return Err(CdpError::server(format!("navigation failed: {error}")));
            }
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
    fn navigate_response(&mut self, page: PageKey, request_id: u64, result: serde_json::Value) {
        let Some(state) = self.pages.get_mut(&page) else { return };
        let Some(pos) = state.pending_navigations.iter().position(|(id, _)| *id == request_id) else { return };
        let (_, session) = state.pending_navigations.swap_remove(pos);
        if self.sessions.contains_key(&session) {
            self.out.push(msg::result_frame(request_id, &result, Some(&session)));
        }
    }

    pub(super) fn on_page_event(&mut self, page: PageKey, event: PageEvent) {
        let all = self.sessions_of(page, |_| true);
        let lifecycle = self.sessions_of(page, |s| s.lifecycle_events);
        let runtime = self.sessions_of(page, |s| s.runtime_enabled);
        match event {
            PageEvent::FrameNavigate { frame_id, loader_id, url, reason, .. } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    state.navigating = true;
                    state.pending_realms.clear();
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
            PageEvent::RealmDestroyed { realm, .. } => {
                // Handles into a disposed realm can never be used again.
                for s in &all {
                    if let Some(session) = self.sessions.get(s) {
                        session.handles.borrow_mut().release_context(context_id(realm));
                    }
                }
                let navigating = self.pages.get(&page).is_some_and(|p| p.navigating);
                if !navigating {
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
                let navigating = self.pages.get(&page).is_some_and(|p| p.navigating);
                if navigating {
                    if let Some(state) = self.pages.get_mut(&page) {
                        state.pending_realms.push((realm, world, origin));
                    }
                } else {
                    let context = context_description(realm, world.as_deref(), &origin, &frame_id.0);
                    for s in &runtime {
                        self.event("Runtime.executionContextCreated", &json!({ "context": context }), s);
                    }
                }
            }
            PageEvent::FrameNavigated { frame_id, loader_id, url, reason, cdp_request_id, timestamp } => {
                if let Some(id) = cdp_request_id {
                    self.navigate_response(page, id, json!({ "frameId": frame_id.0, "loaderId": loader_id.0 }));
                }
                for s in &lifecycle {
                    self.lifecycle(s, "init", &frame_id.0, &loader_id.0, timestamp);
                }
                if reason_name(reason).is_some() {
                    for s in &all {
                        self.event("Page.frameClearedScheduledNavigation", &json!({ "frameId": frame_id.0 }), s);
                    }
                }
                for s in &runtime {
                    self.event("Runtime.executionContextsCleared", &json!({}), s);
                }
                let frame = json!({
                    "id": frame_id.0,
                    "loaderId": loader_id.0,
                    "url": url,
                    "domainAndRegistry": "",
                    "securityOrigin": security_origin(&url),
                    "mimeType": "text/html",
                    "secureContextType": "InsecureScheme",
                    "crossOriginIsolatedContextType": "NotIsolated",
                    "gatedAPIFeatures": [],
                });
                for s in &all {
                    self.event("Page.frameNavigated", &json!({ "frame": frame, "type": "Navigation" }), s);
                }
                let pending = match self.pages.get_mut(&page) {
                    Some(state) => {
                        state.navigating = false;
                        std::mem::take(&mut state.pending_realms)
                    }
                    None => Vec::new(),
                };
                for (realm, world, origin) in pending {
                    let context = context_description(realm, world.as_deref(), &origin, &frame_id.0);
                    for s in &runtime {
                        self.event("Runtime.executionContextCreated", &json!({ "context": context }), s);
                    }
                }
                for s in &all {
                    self.event("DOM.documentUpdated", &json!({}), s);
                }
            }
            PageEvent::FrameNavigateFailed { frame_id, loader_id, error, cdp_request_id, .. } => {
                if let Some(state) = self.pages.get_mut(&page) {
                    state.navigating = false;
                    state.pending_realms.clear();
                }
                if let Some(id) = cdp_request_id {
                    self.navigate_response(
                        page,
                        id,
                        json!({ "frameId": frame_id.0, "loaderId": loader_id.0, "errorText": error }),
                    );
                }
                for s in &all {
                    self.event("Page.frameStoppedLoading", &json!({ "frameId": frame_id.0 }), s);
                }
            }
            PageEvent::FrameDomContentLoaded { frame_id, loader_id, timestamp } => {
                for s in &all {
                    self.event("Page.domContentEventFired", &json!({ "timestamp": timestamp }), s);
                }
                for s in &lifecycle {
                    self.lifecycle(s, "DOMContentLoaded", &frame_id.0, &loader_id.0, timestamp);
                }
            }
            PageEvent::FrameLoaded { frame_id, loader_id, timestamp } => {
                for s in &all {
                    self.event("Page.loadEventFired", &json!({ "timestamp": timestamp }), s);
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
                for s in &all {
                    self.event("Page.navigatedWithinDocument", &json!({ "frameId": frame_id.0, "url": url }), s);
                }
            }
            PageEvent::FrameChildFrameCreated { parent, frame_id } => {
                for s in &all {
                    self.event("Page.frameAttached", &json!({ "frameId": frame_id.0, "parentFrameId": parent.0 }), s);
                }
            }
            PageEvent::FrameChildFrameDetached { frame_id, .. } => {
                for s in &all {
                    self.event("Page.frameDetached", &json!({ "frameId": frame_id.0, "reason": "remove" }), s);
                }
            }
            PageEvent::JavascriptDialogOpening { kind, message, default_prompt, .. } => {
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
            PageEvent::FrameDestroyed { .. } => {}
        }
    }
}
