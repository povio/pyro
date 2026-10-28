//! `LP.run` network mocks: `{"mock": {...}}` answers matching requests of
//! the page's browser context from the I/O thread, without a CDP round
//! trip, optionally after a delay (loading states) or as a failure (error
//! states). Rules live until `{"unmock": {...}}`, the context is disposed or
//! their `times` are used up. They sit in front of CDP `Fetch` interception
//! on the same context: a request no rule answers continues as before.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use lp_net::http::{HeaderMap, HeaderName, HeaderValue};
use lp_net::{AuthChallenge, AuthDecision, InterceptDecision, Interceptor, PausedRequest, RequestInfo};
use tokio::sync::oneshot;

use super::driver::Pattern;
use super::fetch::FetchQueue;

/// What one `mock` step registers.
#[derive(Clone, Debug)]
pub(super) struct MockRule {
    pub id: u64,
    pub url: Pattern,
    pub method: Option<String>,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub delay: Duration,
    /// `Fail(reason)` instead of a response (CDP `Network.ErrorReason`).
    pub abort: Option<String>,
    /// Remaining uses; `None` is unlimited.
    pub times: Option<u32>,
}

#[derive(Default)]
pub(super) struct DriverMocks {
    rules: Mutex<Vec<MockRule>>,
    next: AtomicU64,
}

impl DriverMocks {
    pub(super) fn add(&self, mut rule: MockRule) -> u64 {
        rule.id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let id = rule.id;
        self.rules.lock().unwrap().push(rule);
        id
    }

    /// Removes the rules whose URL pattern equals `url` (all when `None`);
    /// how many went.
    pub(super) fn remove(&self, url: Option<&Pattern>) -> usize {
        let mut rules = self.rules.lock().unwrap();
        let before = rules.len();
        rules.retain(|r| url.is_some_and(|u| u.text != r.url.text));
        before - rules.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rules.lock().unwrap().is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.rules.lock().unwrap().len()
    }

    /// The decision for `req`, from the most recently added matching rule.
    fn decide(&self, req: &RequestInfo) -> Option<(InterceptDecision, Duration)> {
        let mut rules = self.rules.lock().unwrap();
        let pos = rules.iter().rposition(|r| {
            r.method.as_ref().is_none_or(|m| req.method.as_str().eq_ignore_ascii_case(m))
                && r.url.matches(req.url.as_str(), false)
        })?;
        let decision = {
            let r = &rules[pos];
            match &r.abort {
                Some(reason) => InterceptDecision::Fail(reason.clone()),
                None => {
                    InterceptDecision::Fulfill { status: r.status, headers: r.headers.clone(), body: r.body.clone() }
                }
            }
        };
        let delay = rules[pos].delay;
        if let Some(times) = rules[pos].times.as_mut() {
            *times = times.saturating_sub(1);
            if *times == 0 {
                rules.remove(pos);
            }
        }
        Some((decision, delay))
    }
}

/// The interceptor of a browser context: mocks first, then CDP `Fetch`
/// interception when a session enabled it, else the request continues.
pub(super) struct ContextInterceptor {
    pub mocks: Arc<DriverMocks>,
    pub fetch: Option<Arc<FetchQueue>>,
}

impl Interceptor for ContextInterceptor {
    fn paused(&self, request: PausedRequest, reply: oneshot::Sender<InterceptDecision>) {
        if let Some((decision, delay)) = self.mocks.decide(&request.request) {
            if delay.is_zero() {
                let _ = reply.send(decision);
                return;
            }
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = reply.send(decision);
                    });
                }
                Err(_) => {
                    let _ = reply.send(decision);
                }
            }
            return;
        }
        match &self.fetch {
            Some(fetch) => fetch.paused(request, reply),
            None => {
                let _ = reply.send(InterceptDecision::Continue { url: None, method: None, headers: None, body: None });
            }
        }
    }

    fn handles_auth(&self) -> bool {
        self.fetch.as_ref().is_some_and(|f| f.handles_auth())
    }

    fn auth_required(&self, challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        match &self.fetch {
            Some(fetch) => fetch.auth_required(challenge, reply),
            None => {
                let _ = reply.send(AuthDecision::Default);
            }
        }
    }
}

/// Builds a rule's headers: `Content-Type` from `content_type` (or the
/// body kind), then the caller's headers.
pub(super) fn headers_for(
    content_type: Option<&str>,
    default_type: &str,
    extra: &[(String, String)],
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let ct = content_type.unwrap_or(default_type);
    headers.insert(
        lp_net::http::header::CONTENT_TYPE,
        HeaderValue::from_str(ct).map_err(|_| format!("invalid contentType {ct:?}"))?,
    );
    for (name, value) in extra {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("invalid header name {name:?}"))?;
        let value = HeaderValue::from_str(value).map_err(|_| format!("invalid header value for {name}"))?;
        headers.append(name, value);
    }
    Ok(headers)
}
