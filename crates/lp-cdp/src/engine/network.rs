//! Per-context observations routed to the page sessions that enabled Network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use lp_browser::ContextId;
use lp_net::{NetEvent, RequestId};
use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::network_protocol::{BodyBudget, BodyStore, NetworkTranslator, MAX_INFLIGHT};
use crate::target::{ContextKey, PageCommand, PageKey};

#[derive(Default)]
pub(super) struct NetworkWake {
    ready: Arc<AtomicBool>,
    posted: Arc<AtomicBool>,
}

impl NetworkWake {
    pub(super) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Fetch interception decisions must wake command processing even while
    /// network response observations wait for their separate delivery task.
    pub(super) fn callback(&self, handle: lp_runtime::LoopHandle<lp_browser::Browser>) -> lp_net::NetWake {
        let ready = self.ready.clone();
        let posted = self.posted.clone();
        Arc::new(move || {
            ready.store(true, Ordering::Release);
            if !posted.swap(true, Ordering::AcqRel) {
                let posted = posted.clone();
                let _ = handle.post(move |_| posted.store(false, Ordering::Release));
            }
        })
    }
}

pub(super) struct NetworkContext {
    pub(super) id: ContextId,
    subscription: Option<Arc<super::network_delivery::Inbox>>,
    routes: HashMap<RequestId, PageKey>,
    pub(super) fetch: Option<Arc<super::fetch::FetchQueue>>,
}

impl NetworkContext {
    pub(super) fn new(id: ContextId) -> Self {
        Self { id, subscription: None, routes: HashMap::new(), fetch: None }
    }
}

pub(super) struct NetworkPage {
    pub(super) bodies: BodyStore,
    translator: NetworkTranslator,
}

impl NetworkPage {
    pub(super) fn new(budget: BodyBudget) -> Self {
        Self { bodies: BodyStore::new(budget), translator: NetworkTranslator::default() }
    }
}

impl EngineHost {
    pub(super) fn network_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" => {
                // The server's process budget is currently configured by its
                // embedder. Refuse per-session options rather than ignore them.
                let params: serde_json::Map<String, Value> = cmd.command.params()?;
                if !params.is_empty() {
                    return Err(CdpError::server("Network.enable buffer options are not supported"));
                }
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .network_enabled = true;
                self.update_network_subscriptions();
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "disable" => {
                if let Some(session) = self.sessions.get_mut(cmd.session_id) {
                    session.network_enabled = false;
                }
                self.update_network_subscriptions();
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "setExtraHTTPHeaders" => {
                #[derive(Deserialize)]
                struct Params {
                    headers: serde_json::Map<String, Value>,
                }
                let params: Params = cmd.command.params()?;
                let headers = parse_extra_headers(&params.headers)?;
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .extra_headers = headers;
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "getResponseBody" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    request_id: String,
                }
                let params: Params = cmd.command.params()?;
                if !self.sessions.get(cmd.session_id).is_some_and(|s| s.network_enabled) {
                    return Err(CdpError::server("Network domain is not enabled"));
                }
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.network.bodies.get_by_protocol_id(&params.request_id).map(Some)
            }
            "setCacheDisabled" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    cache_disabled: bool,
                }
                let params: Params = cmd.command.params()?;
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .cache_disabled = params.cache_disabled;
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "clearBrowserCache" => {
                // Chrome clears the cache shared by the target's browser
                // context: other pages of that context miss afterwards. With
                // the opt-in shared public-asset cache, its process-wide store
                // is part of what this context reads, so it is cleared too (a
                // no-op when sharing is off).
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let context =
                    self.contexts.get(&state.context).ok_or_else(|| CdpError::server("Browser context is gone"))?;
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    net.clear_http_cache();
                    net.clear_shared_asset_cache();
                }
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Hands the page the request overrides of its sessions that have Network
    /// enabled (Chrome applies a session's settings only while its Network
    /// domain is enabled): the merged extra headers (later sessions override
    /// earlier ones; an empty set clears) and cache bypass (any session).
    /// Sibling pages, even in the same browser context, are unaffected.
    pub(super) fn apply_network_overrides(&mut self, page: PageKey) {
        let Some(state) = self.pages.get(&page) else { return };
        let mut headers = http::HeaderMap::new();
        let mut cache_disabled = false;
        for session in state.sessions.iter().filter_map(|s| self.sessions.get(s)).filter(|s| s.network_enabled) {
            for (name, value) in &session.extra_headers {
                headers.insert(name.clone(), value.clone());
            }
            cache_disabled |= session.cache_disabled;
        }
        // False only for a page lp-browser no longer has, which then has no
        // requests left to apply them to.
        let _ = self.thread.browser.set_extra_http_headers(state.id, headers);
        let _ = self.thread.browser.set_cache_disabled(state.id, cache_disabled);
    }

    pub(super) fn update_network_subscriptions(&mut self) {
        for page in self.pages.values_mut() {
            if !page.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled)) {
                page.network.bodies.clear();
                page.network.translator = NetworkTranslator::default();
            }
        }
        self.update_fetch_interceptors();
        for (key, context) in &mut self.contexts {
            let wanted = self.pages.values().any(|page| {
                page.context == *key
                    && page
                        .sessions
                        .iter()
                        .any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled || s.fetch.needs_observation()))
            });
            if wanted && context.subscription.is_none() {
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    let subscription = super::network_delivery::Inbox::new(
                        net,
                        self.thread.handle(),
                        self.network_wake.ready.clone(),
                        self.collector.network_budget.clone(),
                        self.out.clone(),
                    );
                    self.collector.watch_network(*key, context.id, &subscription);
                    context.subscription = Some(subscription);
                }
            } else if !wanted {
                context.subscription = None;
                context.routes.clear();
            }
            context.routes.retain(|_, page| {
                self.pages
                    .get(page)
                    .is_some_and(|p| p.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled)))
            });
        }
    }

    pub(super) fn flush_network(&mut self) {
        self.network_wake.ready.store(false, Ordering::Release);
        let contexts: Vec<_> = self.contexts.keys().copied().collect();
        for key in contexts {
            // Bound work per flush so command handling is not starved by a
            // continuous response stream. Remaining events cause another wake.
            for index in 0..256 {
                if self.out.is_exceeded() {
                    if let Some(context) = self.contexts.get_mut(&key) {
                        context.subscription = None;
                        context.routes.clear();
                    }
                    break;
                }
                let result = self.contexts.get(&key).and_then(|c| c.subscription.as_ref()).and_then(|s| s.pop());
                match result {
                    Some(event) => self.on_network_event(key, event.event),
                    None => break,
                }
                if index == 255 {
                    self.network_wake.ready.store(true, Ordering::Release);
                }
            }
        }
    }

    pub(super) fn on_network_event(&mut self, context: ContextKey, event: NetEvent) {
        if let NetEvent::LoadingFinished { id, .. } | NetEvent::LoadingFailed { id, .. } = &event {
            self.finish_fetch(context, *id);
        }
        let id = match &event {
            NetEvent::RequestWillBeSent { id, .. }
            | NetEvent::ResponseReceived { id, .. }
            | NetEvent::DataReceived { id, .. }
            | NetEvent::LoadingFinished { id, .. }
            | NetEvent::LoadingFailed { id, .. } => *id,
        };
        if let NetEvent::RequestWillBeSent { request, .. } = &event {
            let page = self.pages.iter().find_map(|(key, page)| {
                (page.context == context && self.owns_frame(page.id, request.frame_id.as_ref())).then_some(*key)
            });
            let Some(page) = page else { return };
            if self.sessions_of(page, |s| s.network_enabled).is_empty() {
                return;
            }
            let context = self.contexts.get_mut(&context).expect("subscribed context");
            if !context.routes.contains_key(&id) && context.routes.len() >= MAX_INFLIGHT {
                self.out.exceed();
                return;
            }
            context.routes.insert(id, page);
        }
        let Some(page) = self.contexts.get(&context).and_then(|c| c.routes.get(&id)).copied() else { return };
        if matches!(event, NetEvent::LoadingFinished { .. } | NetEvent::LoadingFailed { .. }) {
            self.contexts.get_mut(&context).expect("subscribed context").routes.remove(&id);
        }
        let Some(state) = self.pages.get_mut(&page) else { return };
        state.network.bodies.observe(&event);
        if let Some((method, params)) = state.network.translator.translate(&event) {
            for session in self.sessions_of(page, |s| s.network_enabled) {
                self.event(method, &params, &session);
            }
        }
    }
}

/// `Network.Headers` to a header map, rejecting what Chrome 147 rejects with
/// the same messages (non-string values, invalid names and values).
fn parse_extra_headers(headers: &serde_json::Map<String, Value>) -> CdpResult<http::HeaderMap> {
    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CdpError::invalid_params("Invalid header name"))?;
        let value = value.as_str().ok_or_else(|| CdpError::invalid_params("Invalid header value, string expected"))?;
        let value = http::HeaderValue::from_str(value).map_err(|_| CdpError::invalid_params("Invalid header value"))?;
        map.insert(name, value);
    }
    Ok(map)
}
