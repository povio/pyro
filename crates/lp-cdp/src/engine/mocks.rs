//! `LP.run` network mocks: `{"mock": {...}}` answers matching requests of
//! the page's browser context from the I/O thread, without a CDP round
//! trip, optionally after a delay (loading states) or as a failure (error
//! states). Rules live until `{"unmock": {...}}`, the context is disposed or
//! their `times` are used up. They sit in front of CDP `Fetch` interception
//! on the same context: a request no rule answers continues as before.
//!
//! A `passthrough` rule lets the request reach the real backend and gets
//! its response back (`lp_net::InterceptDecision::PauseResponse`), to
//! rewrite it (`patch`) or hold it (`hold`, `delay`). A `hold` without
//! `passthrough` keeps the request unsent until `{"release": {...}}`, then
//! answers it (status/body given) or lets it through. Held replies are
//! `oneshot` senders parked here; releasing one is a send, from any thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use lp_net::http::{header, HeaderMap, HeaderName, HeaderValue};
use lp_net::{
    AuthChallenge, AuthDecision, InterceptDecision, Interceptor, PausedRequest, PausedResponse, RequestId, RequestInfo,
    ResponseDecision,
};
use serde_json::Value;
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
    /// The request goes to the real backend; `patch`, `hold` and `delay`
    /// then apply to its response.
    pub passthrough: bool,
    /// Park the request (or, with `passthrough`, its response) under this
    /// name until a `release` step or `delay`.
    pub hold: Option<String>,
    /// Whether `status`, `body` or `json` were given (a held request without
    /// them continues to the network on release).
    pub responds: bool,
    /// Rewrites of the real response (`passthrough`).
    pub patch: Option<ResponsePatch>,
}

/// `mock.patch`: changes applied to a passed-through response.
#[derive(Clone, Debug, Default)]
pub(super) struct ResponsePatch {
    pub status: Option<u16>,
    /// Set (replacing same-named) response headers.
    pub headers: HeaderMap,
    /// A JSON merge patch (RFC 7396) onto a JSON object body; any other
    /// value, or a non-JSON real body, is replaced by it.
    pub json: Option<Value>,
    /// Replaces the body.
    pub body: Option<Bytes>,
}

impl ResponsePatch {
    fn apply(&self, resp: &PausedResponse) -> ResponseDecision {
        let status = self.status.unwrap_or(resp.status);
        let mut headers = resp.headers.clone();
        let mut body = resp.body.clone();
        let mut content_type = None;
        if let Some(b) = &self.body {
            body = b.clone();
        } else if let Some(patch) = &self.json {
            let merged = match serde_json::from_slice::<Value>(&resp.body) {
                Ok(mut real) if real.is_object() && patch.is_object() => {
                    merge_patch(&mut real, patch);
                    real
                }
                Ok(_) => patch.clone(),
                Err(_) => {
                    content_type = Some(HeaderValue::from_static("application/json"));
                    patch.clone()
                }
            };
            body = Bytes::from(merged.to_string());
        }
        if self.body.is_some() || self.json.is_some() {
            headers.remove(header::CONTENT_ENCODING);
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        }
        if let Some(ct) = content_type {
            headers.insert(header::CONTENT_TYPE, ct);
        }
        for (name, value) in &self.headers {
            headers.insert(name.clone(), value.clone());
        }
        ResponseDecision::Fulfill { status, headers, body }
    }
}

/// RFC 7396 JSON merge patch of `patch` (an object) into `target`.
fn merge_patch(target: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Default::default());
    }
    let map = target.as_object_mut().expect("object");
    for (key, value) in patch {
        if value.is_null() {
            map.remove(key);
        } else {
            merge_patch(map.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// A reply parked under a hold name.
enum Held {
    Request(oneshot::Sender<InterceptDecision>, InterceptDecision),
    Response(oneshot::Sender<ResponseDecision>, ResponseDecision),
}

impl Held {
    fn release(self) {
        match self {
            Held::Request(tx, decision) => {
                let _ = tx.send(decision);
            }
            Held::Response(tx, decision) => {
                let _ = tx.send(decision);
            }
        }
    }
}

/// What a passed-through request's response is to get.
struct PendingResponse {
    hold: Option<String>,
    delay: Duration,
    patch: Option<ResponsePatch>,
}

#[derive(Default)]
pub(super) struct DriverMocks {
    rules: Mutex<Vec<MockRule>>,
    next: AtomicU64,
    /// Held replies: (hold name, hold id, reply).
    holds: Mutex<Vec<(Option<String>, u64, Held)>>,
    /// Responses of passed-through requests still to arrive.
    pending: Mutex<HashMap<RequestId, PendingResponse>>,
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

    /// Releases every reply held under `name`; how many.
    pub(super) fn release(&self, name: &str) -> usize {
        let released: Vec<_> = {
            let mut holds = self.holds.lock().unwrap();
            let (out, keep) = std::mem::take(&mut *holds).into_iter().partition(|(n, ..)| n.as_deref() == Some(name));
            *holds = keep;
            out
        };
        let count = released.len();
        for (_, _, held) in released {
            held.release();
        }
        count
    }

    fn release_one(&self, id: u64) {
        let held = {
            let mut holds = self.holds.lock().unwrap();
            holds.iter().position(|(_, i, _)| *i == id).map(|i| holds.remove(i).2)
        };
        if let Some(held) = held {
            held.release();
        }
    }

    /// Parks `held` under `name` until `release` or, with a delay, until it
    /// elapses. Without a name or a delay nothing would release it: it is
    /// released at once.
    fn hold(self: &Arc<Self>, name: Option<String>, held: Held, delay: Duration) {
        if name.is_none() && delay.is_zero() {
            held.release();
            return;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.holds.lock().unwrap().push((name, id, held));
        if delay.is_zero() {
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let mocks = self.clone();
                handle.spawn(async move {
                    tokio::time::sleep(delay).await;
                    mocks.release_one(id);
                });
            }
            Err(_) => self.release_one(id),
        }
    }

    /// The most recently added rule matching `req`, consuming one of its uses.
    fn decide(&self, req: &RequestInfo) -> Option<MockRule> {
        let mut rules = self.rules.lock().unwrap();
        let pos = rules.iter().rposition(|r| {
            r.method.as_ref().is_none_or(|m| req.method.as_str().eq_ignore_ascii_case(m))
                && r.url.matches(req.url.as_str(), false)
        })?;
        let rule = rules[pos].clone();
        if let Some(times) = rules[pos].times.as_mut() {
            *times = times.saturating_sub(1);
            if *times == 0 {
                rules.remove(pos);
            }
        }
        Some(rule)
    }
}

fn continue_unmodified() -> InterceptDecision {
    InterceptDecision::Continue { url: None, method: None, headers: None, body: None }
}

/// The interceptor of a browser context: mocks first, then CDP `Fetch`
/// interception when a session enabled it, else the request continues.
pub(super) struct ContextInterceptor {
    pub mocks: Arc<DriverMocks>,
    pub fetch: Option<Arc<FetchQueue>>,
}

impl Interceptor for ContextInterceptor {
    fn paused(&self, request: PausedRequest, reply: oneshot::Sender<InterceptDecision>) {
        if let Some(rule) = self.mocks.decide(&request.request) {
            if rule.passthrough {
                if rule.hold.is_none() && rule.patch.is_none() && rule.delay.is_zero() {
                    let _ = reply.send(continue_unmodified());
                    return;
                }
                let pending = PendingResponse { hold: rule.hold, delay: rule.delay, patch: rule.patch };
                self.mocks.pending.lock().unwrap().insert(request.id, pending);
                let _ = reply.send(InterceptDecision::PauseResponse);
                return;
            }
            let decision = match rule.abort {
                Some(reason) => InterceptDecision::Fail(reason),
                None if rule.hold.is_some() && !rule.responds => continue_unmodified(),
                None => InterceptDecision::Fulfill { status: rule.status, headers: rule.headers, body: rule.body },
            };
            self.mocks.hold(rule.hold, Held::Request(reply, decision), rule.delay);
            return;
        }
        match &self.fetch {
            Some(fetch) => fetch.paused(request, reply),
            None => {
                let _ = reply.send(continue_unmodified());
            }
        }
    }

    fn response(&self, resp: PausedResponse, reply: oneshot::Sender<ResponseDecision>) {
        let Some(pending) = self.mocks.pending.lock().unwrap().remove(&resp.id) else {
            let _ = reply.send(ResponseDecision::Continue);
            return;
        };
        let decision = match &pending.patch {
            Some(patch) => patch.apply(&resp),
            None => ResponseDecision::Continue,
        };
        self.mocks.hold(pending.hold, Held::Response(reply, decision), pending.delay);
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
    extend_headers(&mut headers, extra)?;
    Ok(headers)
}

/// Appends `extra` to `headers`, validating names and values.
pub(super) fn extend_headers(headers: &mut HeaderMap, extra: &[(String, String)]) -> Result<(), String> {
    for (name, value) in extra {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("invalid header name {name:?}"))?;
        let value = HeaderValue::from_str(value).map_err(|_| format!("invalid header value for {name}"))?;
        headers.append(name, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_patch_follows_rfc_7396() {
        let mut target = json!({"a": {"b": 1, "c": 2}, "total": 1, "gone": true});
        merge_patch(&mut target, &json!({"a": {"b": null, "d": [1]}, "total": 999, "gone": null, "new": "x"}));
        assert_eq!(target, json!({"a": {"c": 2, "d": [1]}, "total": 999, "new": "x"}));
        let mut scalar = json!(5);
        merge_patch(&mut scalar, &json!({"k": 1}));
        assert_eq!(scalar, json!({"k": 1}));
    }
}
