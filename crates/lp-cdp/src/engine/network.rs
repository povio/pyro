//! Per-context observations routed to the page sessions that enabled Network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use lp_browser::ContextId;
use lp_net::{NetEvent, NetSubscription, RequestId, SubscriptionError, SubscriptionLimits};
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

    fn callback(&self, handle: lp_runtime::LoopHandle<lp_browser::Browser>) -> lp_net::NetWake {
        let ready = self.ready.clone();
        let posted = self.posted.clone();
        Arc::new(move || {
            ready.store(true, Ordering::Release);
            // One outstanding loop task suffices even if multiple contexts
            // publish while the browser is processing a command.
            if !posted.swap(true, Ordering::AcqRel) {
                let posted = posted.clone();
                let _ = handle.post(move |_| posted.store(false, Ordering::Release));
            }
        })
    }
}

pub(super) struct NetworkContext {
    pub(super) id: ContextId,
    subscription: Option<NetSubscription>,
    routes: HashMap<RequestId, PageKey>,
}

impl NetworkContext {
    pub(super) fn new(id: ContextId) -> Self {
        Self { id, subscription: None, routes: HashMap::new() }
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
                Ok(Some(json!({})))
            }
            "disable" => {
                if let Some(session) = self.sessions.get_mut(cmd.session_id) {
                    session.network_enabled = false;
                }
                self.update_network_subscriptions();
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
                // lp-net has no response cache: disabling it is already true,
                // and allowing caching does not require responses to be cached.
                let _ = params.cache_disabled;
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    pub(super) fn update_network_subscriptions(&mut self) {
        for page in self.pages.values_mut() {
            if !page.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled)) {
                page.network.bodies.clear();
                page.network.translator = NetworkTranslator::default();
            }
        }
        for (key, context) in &mut self.contexts {
            let wanted = self.pages.values().any(|page| {
                page.context == *key
                    && page.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled))
            });
            if wanted && context.subscription.is_none() {
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    context.subscription = Some(net.subscribe_with(
                        SubscriptionLimits::default(),
                        Some(self.network_wake.callback(self.thread.handle())),
                    ));
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
                let result = self.contexts.get(&key).and_then(|c| c.subscription.as_ref()).map(|s| s.try_recv());
                match result {
                    Some(Ok(event)) => self.on_network_event(key, event),
                    Some(Err(SubscriptionError::Empty)) | None => break,
                    Some(Err(SubscriptionError::Overflow | SubscriptionError::Closed)) => {
                        // No selective event loss: terminate the CDP connection
                        // if its observation stream can no longer be trusted.
                        if let Some(context) = self.contexts.get_mut(&key) {
                            context.subscription = None;
                            context.routes.clear();
                        }
                        self.out.exceed();
                        break;
                    }
                }
                if index == 255 {
                    self.network_wake.ready.store(true, Ordering::Release);
                }
            }
        }
    }

    fn on_network_event(&mut self, context: ContextKey, event: NetEvent) {
        let id = match &event {
            NetEvent::RequestWillBeSent { id, .. }
            | NetEvent::ResponseReceived { id, .. }
            | NetEvent::DataReceived { id, .. }
            | NetEvent::LoadingFinished { id, .. }
            | NetEvent::LoadingFailed { id, .. } => *id,
        };
        if let NetEvent::RequestWillBeSent { request, .. } = &event {
            let page = self.pages.iter().find_map(|(key, page)| {
                (page.context == context
                    && self
                        .thread
                        .browser
                        .page(page.id)
                        .is_some_and(|p| Some(&p.main_frame.id.0) == request.frame_id.as_ref()))
                .then_some(*key)
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
