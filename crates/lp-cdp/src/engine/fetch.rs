//! Fetch interception at the request and response stages. Network
//! callbacks only enqueue owned decisions; protocol state stays on the
//! browser thread. Response bodies are buffered (by lp-net) only for
//! requests paused at the response stage.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use lp_net::{
    AuthChallenge, AuthDecision, InterceptDecision, Interceptor, PausedRequest, PausedResponse, RequestId, RequestInfo,
    ResponseDecision,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::EngineHost;
use crate::fetch_protocol::{base64_decode, EnableParams, InterceptState, RequestStage, MAX_PAUSED};
use crate::msg::{CdpError, CdpResult};
use crate::network_protocol::{fetch_resource_type, headers_json, protocol_request_id, write_post_data};
use crate::outbox::Outbox;
use crate::target::{ContextKey, PageCommand};

#[derive(Default)]
pub(super) struct FetchSession {
    pub(super) rules: InterceptState<Pending>,
    ids: HashMap<RequestId, String>,
    /// Requests continued with `interceptResponse: true`.
    intercept_response: std::collections::HashSet<RequestId>,
}

impl FetchSession {
    pub(super) fn needs_observation(&self) -> bool {
        self.rules.is_enabled() || !self.ids.is_empty()
    }
}

pub(super) struct Pending {
    native_id: RequestId,
    decision: Decision,
}

enum Decision {
    Request(oneshot::Sender<InterceptDecision>),
    Auth(oneshot::Sender<AuthDecision>),
    /// Paused at the response stage, with the response it holds.
    Response(oneshot::Sender<ResponseDecision>, Box<PausedResponse>),
}

impl Decision {
    fn resume(self) {
        match self {
            Self::Request(tx) => {
                let _ = tx.send(continue_unmodified());
            }
            Self::Auth(tx) => {
                let _ = tx.send(AuthDecision::Default);
            }
            Self::Response(tx, _) => {
                let _ = tx.send(ResponseDecision::Continue);
            }
        }
    }
}

fn continue_unmodified() -> InterceptDecision {
    InterceptDecision::Continue { url: None, method: None, headers: None, body: None }
}

enum Queued {
    Request(PausedRequest, oneshot::Sender<InterceptDecision>),
    Auth(AuthChallenge, oneshot::Sender<AuthDecision>),
    Response(Box<PausedResponse>, oneshot::Sender<ResponseDecision>),
}

impl Queued {
    fn request(&self) -> (&RequestInfo, RequestId) {
        match self {
            Self::Request(request, _) => (&request.request, request.id),
            Self::Auth(challenge, _) => (&challenge.request, challenge.id),
            Self::Response(response, _) => (&response.request, response.id),
        }
    }
    fn is_closed(&self) -> bool {
        match self {
            Self::Request(_, tx) => tx.is_closed(),
            Self::Auth(_, tx) => tx.is_closed(),
            Self::Response(_, tx) => tx.is_closed(),
        }
    }
    fn resume(self) {
        match self {
            Self::Request(_, tx) => Decision::Request(tx).resume(),
            Self::Auth(_, tx) => Decision::Auth(tx).resume(),
            Self::Response(response, tx) => Decision::Response(tx, response).resume(),
        }
    }
}

#[derive(Default)]
struct QueueState {
    items: VecDeque<(Queued, usize)>,
    bytes: usize,
    stopped: bool,
}

pub(super) struct FetchQueue {
    state: Mutex<QueueState>,
    auth: AtomicBool,
    wake: lp_net::NetWake,
    out: Outbox,
}

impl FetchQueue {
    fn push(&self, item: Queued) {
        let request = item.request().0;
        let extra = match &item {
            Queued::Auth(challenge, _) => challenge.origin.len() + challenge.scheme.len() + challenge.realm.len(),
            Queued::Request(..) => 0,
            Queued::Response(response, _) => response.body.len() + response.headers.len() * 64,
        };
        let bytes = request
            .url
            .as_str()
            .len()
            .saturating_add(request.post_data.as_ref().map_or(0, |b| b.len()))
            .saturating_add(request.headers.iter().map(|(n, v)| n.as_str().len() + v.as_bytes().len()).sum::<usize>())
            .saturating_add(request.frame_id.as_ref().map_or(0, String::len))
            .saturating_add(request.loader_id.as_ref().map_or(0, String::len))
            .saturating_add(request.method.as_str().len())
            .saturating_add(extra)
            .saturating_add(256);
        let notify = {
            let mut state = self.state.lock().unwrap();
            if state.stopped {
                drop(state);
                item.resume();
                return;
            }
            if state.items.len() >= 256 || bytes > (8usize * 1024 * 1024).saturating_sub(state.bytes) {
                self.out.exceed();
                return; // dropping the decision fails the request
            }
            let empty = state.items.is_empty();
            state.bytes += bytes;
            state.items.push_back((item, bytes));
            empty
        };
        if notify {
            (self.wake)();
        }
    }

    fn pop(&self) -> Option<Queued> {
        let mut state = self.state.lock().unwrap();
        let (item, bytes) = state.items.pop_front()?;
        state.bytes -= bytes;
        Some(item)
    }

    fn stop(&self) {
        let items = {
            let mut state = self.state.lock().unwrap();
            state.stopped = true;
            state.bytes = 0;
            std::mem::take(&mut state.items)
        };
        for (item, _) in items {
            item.resume();
        }
    }
}

impl Drop for FetchQueue {
    fn drop(&mut self) {
        for (item, _) in self.state.get_mut().unwrap().items.drain(..) {
            item.resume();
        }
    }
}

impl Interceptor for FetchQueue {
    fn paused(&self, request: PausedRequest, reply: oneshot::Sender<InterceptDecision>) {
        self.push(Queued::Request(request, reply));
    }
    fn handles_auth(&self) -> bool {
        self.auth.load(Ordering::Acquire)
    }
    fn auth_required(&self, challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        self.push(Queued::Auth(challenge, reply));
    }
    fn response(&self, response: PausedResponse, reply: oneshot::Sender<ResponseDecision>) {
        self.push(Queued::Response(Box::new(response), reply));
    }
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

fn headers(values: Option<Vec<Header>>) -> CdpResult<Option<HeaderMap>> {
    values
        .map(|values| {
            let mut headers = HeaderMap::new();
            for header in values {
                let name = HeaderName::from_bytes(header.name.as_bytes())
                    .map_err(|_| CdpError::invalid_params("Unsafe header name"))?;
                let value = HeaderValue::from_str(&header.value)
                    .map_err(|_| CdpError::invalid_params("Unsafe header value"))?;
                headers.append(name, value);
            }
            Ok(headers)
        })
        .transpose()
}

fn binary_headers(encoded: &str) -> CdpResult<HeaderMap> {
    let bytes = base64_decode(encoded)?;
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(&bytes);
    let mut headers = HeaderMap::new();
    if bytes.is_empty() {
        return Ok(headers);
    }
    for entry in bytes.split(|b| *b == 0) {
        let colon = entry
            .iter()
            .position(|b| *b == b':')
            .ok_or_else(|| CdpError::invalid_params("Invalid binary response header"))?;
        let name =
            HeaderName::from_bytes(&entry[..colon]).map_err(|_| CdpError::invalid_params("Unsafe header name"))?;
        let mut value = &entry[colon + 1..];
        while value.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            value = &value[1..];
        }
        while value.last().is_some_and(|b| matches!(b, b' ' | b'\t')) {
            value = &value[..value.len() - 1];
        }
        let value = HeaderValue::from_bytes(value).map_err(|_| CdpError::invalid_params("Unsafe header value"))?;
        headers.append(name, value);
    }
    Ok(headers)
}

impl EngineHost {
    pub(super) fn disable_fetch(&mut self, session: &str) {
        if let Some(session) = self.sessions.get_mut(session) {
            for pending in session.fetch.rules.disable() {
                pending.decision.resume();
            }
            session.fetch.ids.clear();
            session.fetch.intercept_response.clear();
        }
    }

    pub(super) fn update_fetch_interceptors(&mut self) {
        for (key, context) in &mut self.contexts {
            let sessions: Vec<_> = self
                .pages
                .values()
                .filter(|p| p.context == *key)
                .flat_map(|p| p.sessions.iter())
                .filter_map(|s| self.sessions.get(s))
                .collect();
            let wanted = sessions.iter().any(|s| s.fetch.rules.is_enabled());
            let auth = sessions.iter().any(|s| s.fetch.rules.handle_auth_requests);
            let mut changed = false;
            if wanted && context.fetch.is_none() {
                let queue = Arc::new(FetchQueue {
                    state: Mutex::default(),
                    auth: AtomicBool::new(auth),
                    wake: self.network_wake.callback(self.thread.handle()),
                    out: self.out.clone(),
                });
                context.fetch = Some(queue);
                changed = true;
            } else if !wanted {
                // A network hop retains its interceptor across awaits. Dropping
                // our Arc alone cannot release its paused decision.
                if let Some(queue) = context.fetch.take() {
                    queue.stop();
                    changed = true;
                }
            }
            if let Some(queue) = &context.fetch {
                queue.auth.store(auth, Ordering::Release);
            }
            // The context's interceptor: mocks (`LP.run`) in front of Fetch.
            let want_mocks = !context.mocks.is_empty();
            if changed || want_mocks != context.mocks_installed {
                context.mocks_installed = want_mocks;
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    if context.fetch.is_some() || want_mocks {
                        net.set_interceptor(Some(Arc::new(super::mocks::ContextInterceptor {
                            mocks: context.mocks.clone(),
                            fetch: context.fetch.clone(),
                        })));
                    } else {
                        net.set_interceptor(None);
                    }
                }
            }
        }
    }

    pub(super) fn finish_fetch(&mut self, context: ContextKey, id: RequestId) {
        let mut observation_finished = false;
        for page in self.pages.values().filter(|p| p.context == context) {
            for session in &page.sessions {
                if let Some(session) = self.sessions.get_mut(session) {
                    session.fetch.intercept_response.remove(&id);
                    if let Some(id) = session.fetch.ids.remove(&id) {
                        let _ = session.fetch.rules.take(&id);
                        observation_finished |= !session.fetch.needs_observation();
                    }
                }
            }
        }
        if observation_finished {
            self.update_network_subscriptions();
        }
    }

    pub(super) fn flush_fetch(&mut self) {
        let queues: Vec<_> = self.contexts.iter().filter_map(|(key, c)| c.fetch.clone().map(|q| (*key, q))).collect();
        for (context, queue) in queues {
            for _ in 0..256 {
                let Some(item) = queue.pop() else { break };
                if item.is_closed() {
                    continue;
                }
                let (request, native_id) = item.request();
                let page = self.pages.iter().find_map(|(key, p)| {
                    (p.context == context && self.owns_frame(p.id, request.frame_id.as_ref())).then_some(*key)
                });
                let Some(page) = page else {
                    item.resume();
                    continue;
                };
                let resource_type = fetch_resource_type(request.destination);
                let session_id = self
                    .sessions_of(page, |s| match &item {
                        Queued::Request(..) => {
                            s.fetch.rules.matches(RequestStage::Request, request.url.as_str(), resource_type)
                        }
                        Queued::Auth(..) => s.fetch.rules.handle_auth_requests,
                        Queued::Response(..) => {
                            s.fetch.intercept_response.contains(&native_id)
                                || s.fetch.rules.matches(RequestStage::Response, request.url.as_str(), resource_type)
                        }
                    })
                    .into_iter()
                    .next();
                let Some(session_id) = session_id else {
                    // Matched by a response-stage pattern only: let it run
                    // and pause its response.
                    let url = request.url.as_str().to_owned();
                    if let Queued::Request(_, tx) = item {
                        let wanted = self
                            .sessions_of(page, |s| s.fetch.rules.matches(RequestStage::Response, &url, resource_type));
                        let _ = tx.send(match wanted.is_empty() {
                            true => continue_unmodified(),
                            false => InterceptDecision::PauseResponse,
                        });
                    } else {
                        item.resume();
                    }
                    continue;
                };
                let mut request_json = json!({"url":request.url.as_str(),"method":request.method.as_str(),"headers":headers_json(&request.headers),
                    "hasPostData":false});
                write_post_data(&mut request_json, request.post_data.as_ref());
                let mut params = json!({"request":request_json,"frameId":request.frame_id,
                    "resourceType":fetch_resource_type(request.destination).as_str()});
                params["networkId"] =
                    json!(protocol_request_id(native_id, request.destination, request.loader_id.as_deref()));
                let (method, decision) = match item {
                    Queued::Request(_, tx) => ("Fetch.requestPaused", Decision::Request(tx)),
                    Queued::Response(response, tx) => {
                        params["responseStatusCode"] = json!(response.status);
                        if let Some(phrase) =
                            StatusCode::from_u16(response.status).ok().and_then(|s| s.canonical_reason())
                        {
                            params["responseStatusText"] = json!(phrase);
                        }
                        params["responseHeaders"] = json!(response
                            .headers
                            .iter()
                            .map(|(n, v)| json!({"name": n.as_str(), "value": String::from_utf8_lossy(v.as_bytes())}))
                            .collect::<Vec<_>>());
                        ("Fetch.requestPaused", Decision::Response(tx, response))
                    }
                    Queued::Auth(challenge, tx) => {
                        params["authChallenge"] = json!({"source":if challenge.proxy {"Proxy"} else {"Server"},
                            "origin":challenge.origin,"scheme":challenge.scheme,"realm":challenge.realm});
                        ("Fetch.authRequired", Decision::Auth(tx))
                    }
                };
                let fetch = &mut self.sessions.get_mut(&session_id).expect("selected session").fetch;
                let pending = Pending { native_id, decision };
                let previous = fetch.ids.get(&native_id).cloned();
                let again = method == "Fetch.authRequired" || matches!(pending.decision, Decision::Response(..));
                if matches!(pending.decision, Decision::Response(..)) {
                    fetch.intercept_response.remove(&native_id);
                }
                let id = if let Some(id) = previous.as_ref().filter(|_| again) {
                    if fetch.rules.pause_again(id, pending).is_err() {
                        self.out.exceed();
                        continue;
                    }
                    id.clone()
                } else {
                    if previous.is_none() && fetch.ids.len() >= MAX_PAUSED {
                        self.out.exceed();
                        continue;
                    }
                    let Ok(id) = fetch.rules.pause(pending) else {
                        self.out.exceed();
                        continue;
                    };
                    // A redirect preserves Network.requestId, but each hop
                    // has a distinct Fetch id. Auth retries reuse that hop's id.
                    if let Some(previous) = previous {
                        params["redirectedRequestId"] = json!(previous);
                    }
                    fetch.ids.insert(native_id, id.clone());
                    id
                };
                params["requestId"] = json!(id);
                self.event(method, &params, &session_id);
            }
            if !queue.state.lock().unwrap().items.is_empty() {
                (queue.wake)();
            }
        }
    }

    pub(super) fn fetch_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" => {
                let params: EnableParams = cmd.command.params()?;
                if self.sessions_of(cmd.page, |s| s.fetch.rules.is_enabled()).iter().any(|s| s != cmd.session_id) {
                    return Err(CdpError::server("Fetch is already enabled by another session of this page"));
                }
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .fetch
                    .rules
                    .enable(params)?;
                self.update_network_subscriptions();
            }
            "disable" => {
                self.disable_fetch(cmd.session_id);
                self.update_network_subscriptions();
            }
            "continueRequest" | "fulfillRequest" | "failRequest" | "continueWithAuth" | "continueResponse" => {
                self.fetch_decision(action, cmd)?
            }
            "getResponseBody" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    request_id: String,
                }
                let p: Params = cmd.command.params()?;
                let session = self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?;
                let Decision::Response(_, response) = &session.fetch.rules.pending(&p.request_id)?.decision else {
                    return Err(CdpError::server(
                        "Can only get response body on HeadersReceived pattern matched requests.",
                    ));
                };
                return Ok(Some(match std::str::from_utf8(&response.body) {
                    Ok(text) => json!({ "body": text, "base64Encoded": false }),
                    Err(_) => {
                        json!({ "body": crate::network_protocol::base64_encode(&response.body), "base64Encoded": true })
                    }
                }));
            }
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        }
        Ok(Some(json!({})))
    }

    fn fetch_decision(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Params {
            request_id: String,
            url: Option<String>,
            method: Option<String>,
            headers: Option<Vec<Header>>,
            post_data: Option<String>,
            response_code: Option<u16>,
            response_headers: Option<Vec<Header>>,
            response_phrase: Option<String>,
            binary_response_headers: Option<String>,
            body: Option<String>,
            error_reason: Option<String>,
            auth_challenge_response: Option<AuthResponse>,
            intercept_response: Option<bool>,
        }
        #[derive(Deserialize)]
        struct AuthResponse {
            response: String,
            username: Option<String>,
            password: Option<String>,
        }
        let p: Params = cmd.command.params()?;
        let session = self.sessions.get_mut(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?;
        let pending = session.fetch.rules.pending(&p.request_id)?;
        let at_response = matches!(&pending.decision, Decision::Response(..));
        if matches!(&pending.decision, Decision::Auth(_)) != (action == "continueWithAuth")
            || (action == "continueRequest" && at_response)
            || (action == "continueResponse" && !at_response)
        {
            return Err(CdpError::invalid_params("Decision does not match the paused request stage"));
        }
        let native_id = pending.native_id;
        let mut request_decision = None;
        let mut auth_decision = None;
        let mut response_decision = None;
        match action {
            "continueRequest" if p.intercept_response == Some(true) => {
                if p.url.is_some() || p.method.is_some() || p.headers.is_some() || p.post_data.is_some() {
                    return Err(CdpError::invalid_params("Overrides are not supported with interceptResponse"));
                }
                session.fetch.intercept_response.insert(native_id);
                request_decision = Some(InterceptDecision::PauseResponse);
            }
            "continueResponse" => {
                let Some(Decision::Response(_, response)) =
                    session.fetch.rules.pending(&p.request_id).ok().map(|p| &p.decision)
                else {
                    unreachable!("checked above")
                };
                if p.response_phrase.is_some() {
                    return Err(CdpError::server("Custom response phrases are not supported"));
                }
                let overridden =
                    p.response_code.is_some() || p.response_headers.is_some() || p.binary_response_headers.is_some();
                response_decision = Some(if overridden {
                    let headers = match (p.response_headers, p.binary_response_headers) {
                        (Some(_), Some(_)) => {
                            return Err(CdpError::invalid_params(
                                "Specify responseHeaders or binaryResponseHeaders, not both",
                            ))
                        }
                        (None, Some(binary)) => binary_headers(&binary)?,
                        (Some(text), None) => headers(Some(text))?.unwrap_or_default(),
                        (None, None) => response.headers.clone(),
                    };
                    let status = p.response_code.unwrap_or(response.status);
                    if StatusCode::from_u16(status).is_err() || status < 200 {
                        return Err(CdpError::invalid_params("Invalid http status code or phrase"));
                    }
                    ResponseDecision::Fulfill { status, headers, body: response.body.clone() }
                } else {
                    ResponseDecision::Continue
                });
            }
            "continueRequest" => {
                request_decision = Some(InterceptDecision::Continue {
                    url: p
                        .url
                        .map(|url| lp_net::Url::parse(&url).map_err(|_| CdpError::invalid_params("Invalid URL")))
                        .transpose()?,
                    method: p
                        .method
                        .map(|m| {
                            Method::from_bytes(m.as_bytes()).map_err(|_| CdpError::invalid_params("Invalid method"))
                        })
                        .transpose()?,
                    headers: headers(p.headers)?,
                    body: p.post_data.map(|b| base64_decode(&b).map(Into::into)).transpose()?,
                });
            }
            "fulfillRequest" => {
                let code = p.response_code.ok_or_else(|| CdpError::invalid_params("responseCode is required"))?;
                let status = StatusCode::from_u16(code)
                    .map_err(|_| CdpError::invalid_params("Invalid http status code or phrase"))?;
                if code < 200 {
                    return Err(CdpError::invalid_params("Invalid http status code or phrase"));
                }
                if p.response_phrase.as_deref().is_some_and(|p| Some(p) != status.canonical_reason()) {
                    return Err(CdpError::server("Custom response phrases are not supported"));
                }
                let headers = match (p.response_headers, p.binary_response_headers) {
                    (Some(_), Some(_)) => {
                        return Err(CdpError::invalid_params(
                            "Specify responseHeaders or binaryResponseHeaders, not both",
                        ))
                    }
                    (None, Some(binary)) => binary_headers(&binary)?,
                    (text, None) => headers(text)?.unwrap_or_default(),
                };
                let body: bytes::Bytes = base64_decode(p.body.as_deref().unwrap_or(""))?.into();
                match at_response {
                    true => response_decision = Some(ResponseDecision::Fulfill { status: code, headers, body }),
                    false => request_decision = Some(InterceptDecision::Fulfill { status: code, headers, body }),
                }
            }
            "failRequest" => {
                let reason = p.error_reason.ok_or_else(|| CdpError::invalid_params("errorReason is required"))?;
                if !matches!(
                    reason.as_str(),
                    "Failed"
                        | "Aborted"
                        | "TimedOut"
                        | "AccessDenied"
                        | "ConnectionClosed"
                        | "ConnectionReset"
                        | "ConnectionRefused"
                        | "ConnectionAborted"
                        | "ConnectionFailed"
                        | "NameNotResolved"
                        | "InternetDisconnected"
                        | "AddressUnreachable"
                        | "BlockedByClient"
                        | "BlockedByResponse"
                ) {
                    return Err(CdpError::invalid_params("Invalid errorReason"));
                }
                match at_response {
                    true => response_decision = Some(ResponseDecision::Fail(reason)),
                    false => request_decision = Some(InterceptDecision::Fail(reason)),
                }
            }
            "continueWithAuth" => {
                let auth = p
                    .auth_challenge_response
                    .ok_or_else(|| CdpError::invalid_params("authChallengeResponse is required"))?;
                auth_decision = Some(match auth.response.as_str() {
                    "Default" => AuthDecision::Default,
                    "CancelAuth" => AuthDecision::CancelAuth,
                    "ProvideCredentials" => AuthDecision::ProvideCredentials {
                        username: auth.username.unwrap_or_default(),
                        password: auth.password.unwrap_or_default(),
                    },
                    _ => return Err(CdpError::invalid_params("Invalid authentication response")),
                });
            }
            _ => unreachable!(),
        }
        let pending = session.fetch.rules.take(&p.request_id)?;
        let sent = match pending.decision {
            Decision::Request(tx) => tx.send(request_decision.expect("validated stage")).is_ok(),
            Decision::Auth(tx) => tx.send(auth_decision.expect("validated stage")).is_ok(),
            Decision::Response(tx, _) => tx.send(response_decision.expect("validated stage")).is_ok(),
        };
        if sent {
            Ok(())
        } else {
            session.fetch.ids.remove(&pending.native_id);
            Err(CdpError::server("Request is no longer paused"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_response_headers_preserve_bytes_duplicates_and_validate_framing() {
        let map = binary_headers("eC1ieXRlczog/4AAU2V0LUNvb2tpZTogYT0xAHNldC1jb29raWU6IGI9MgA=").unwrap();
        assert_eq!(map["x-bytes"].as_bytes(), &[0xff, 0x80]);
        assert_eq!(
            map.get_all("set-cookie").iter().map(|v| v.as_bytes()).collect::<Vec<_>>(),
            vec![b"a=1".as_slice(), b"b=2".as_slice()]
        );
        assert!(binary_headers("").unwrap().is_empty());
        for invalid in ["bm8gY29sb24=", "OiBlbXB0eSBuYW1l", "eDogdmFsdWUNCmluamVjdGVkOiB4", "eDogYQAAeTogYg==", "=="] {
            assert!(binary_headers(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn empty_fetch_patterns_still_observe_terminals_of_existing_pauses() {
        use crate::target::Host;
        use std::time::{Duration, Instant};
        let (out, _rx) = Outbox::channel(1024 * 1024);
        let mut host = EngineHost::new(out).unwrap();
        let context = host.create_context().unwrap();
        let (page, _) = host.create_page(context).unwrap();
        host.session_attached(page, "session");
        host.sessions
            .get_mut("session")
            .unwrap()
            .fetch
            .rules
            .enable(EnableParams { patterns: None, handle_auth_requests: false })
            .unwrap();
        host.update_network_subscriptions();
        host.navigate(page, "http://127.0.0.1:1/paused").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.sessions["session"].fetch.ids.is_empty() && Instant::now() < deadline {
            host.run_until_work(Some(deadline), || false);
            host.flush();
        }
        let id = host.sessions["session"].fetch.ids.values().next().expect("request paused").clone();
        let fetch = &mut host.sessions.get_mut("session").unwrap().fetch;
        fetch.rules.enable(EnableParams { patterns: Some(vec![]), handle_auth_requests: false }).unwrap();
        let pending = fetch.rules.take(&id).unwrap();
        host.update_network_subscriptions();
        let Decision::Request(reply) = pending.decision else { panic!("request stage") };
        reply.send(InterceptDecision::Fulfill { status: 200, headers: HeaderMap::new(), body: "done".into() }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !host.sessions["session"].fetch.ids.is_empty() && Instant::now() < deadline {
            host.run_until_work(Some(deadline), || false);
            host.flush();
        }
        assert!(
            host.sessions["session"].fetch.ids.is_empty(),
            "terminal must release the Fetch ID even after patterns become empty"
        );
        host.close_all();
    }

    #[test]
    fn stopped_queue_resumes_queued_and_late_decisions_with_retained_interceptor() {
        let (out, _) = Outbox::channel(1024);
        let queue =
            Arc::new(FetchQueue { state: Mutex::default(), auth: AtomicBool::new(true), wake: Arc::new(|| {}), out });
        // The network hop keeps this Arc while awaiting either decision.
        let retained: Arc<dyn Interceptor> = queue.clone();
        let request = RequestInfo {
            url: lp_net::Url::parse("http://localhost/").unwrap(),
            method: Method::GET,
            headers: HeaderMap::new(),
            post_data: None,
            destination: lp_net::Destination::Document,
            frame_id: None,
            loader_id: None,
        };
        let (tx, mut rx) = oneshot::channel();
        retained.paused(PausedRequest { id: RequestId(1), request: request.clone() }, tx);
        assert!(matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
        queue.stop();
        assert!(matches!(rx.try_recv(), Ok(InterceptDecision::Continue { .. })));
        assert_eq!(queue.state.lock().unwrap().bytes, 0);
        drop(queue);
        let (tx, mut rx) = oneshot::channel();
        retained.auth_required(
            AuthChallenge {
                id: RequestId(1),
                request,
                origin: "http://localhost".into(),
                scheme: "basic".into(),
                realm: "test".into(),
                proxy: false,
            },
            tx,
        );
        assert!(matches!(rx.try_recv(), Ok(AuthDecision::Default)));
    }
}
