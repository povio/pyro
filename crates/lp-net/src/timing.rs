//! When the phases of a fetch happened, for Resource Timing and Navigation
//! Timing: plain `Instant`s on the response head ([`NetTiming`]), filled in as
//! the I/O thread goes. A new connection reports its DNS, TCP and TLS times
//! through the connector: [`ProxyConnector`](crate::proxy) stamps them into a
//! [`ConnInfo`] that hyper copies into every response of that connection, and
//! the resolver finds the connection it resolves for in a task-local.
//!
//! The cost per request is a few `Instant::now()` calls; per new connection
//! one small allocation.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Instant;

use hyper_util::client::legacy::connect::dns::{GaiAddrs, GaiResolver, Name};
use tower_service::Service;

/// The phases of one fetch (all hops), as `Instant`s on this process's
/// monotonic clock. `None`: the phase did not happen (a response from the
/// HTTP cache, a static file, an interception, or a reused connection for
/// the connection fields).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetTiming {
    /// The fetch was handed to the client (`NetClient::fetch`).
    pub start: Option<Instant>,
    /// The last redirect response was received (`redirectEnd`); `None`
    /// without redirects.
    pub redirect_end: Option<Instant>,
    /// Redirects followed.
    pub redirect_count: u16,
    /// A redirect went to another origin (Navigation Timing hides the
    /// redirects then).
    pub cross_origin_redirect: bool,
    /// The final hop began (`fetchStart`).
    pub fetch_start: Option<Instant>,
    /// The connection the final hop opened, `None` when it reused one.
    pub connection: Option<ConnectionTiming>,
    /// The request went out (`requestStart`).
    pub request_start: Option<Instant>,
    /// The response head arrived (`responseStart`).
    pub response_start: Option<Instant>,
    /// Some hop was cross-origin to the requesting origin and did not pass
    /// the Timing-Allow-Origin check: only the fetch's start and end may be
    /// exposed (Fetch "TAO check").
    pub timing_allow_failed: bool,
}

/// The phases of opening a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionTiming {
    /// The connector was called (`domainLookupStart` without a lookup).
    pub start: Instant,
    /// The host name lookup, when one ran (an IP literal needs none).
    pub dns: Option<(Instant, Instant)>,
    /// TCP (and a proxy tunnel) connected; TLS starts here for https
    /// (`secureConnectionStart`).
    pub tcp_end: Instant,
    pub secure: bool,
    /// The connection is ready for the request (`connectEnd`).
    pub end: Instant,
}

/// The connection's times as the connector stamps them (shared with the
/// resolver and the stream).
#[derive(Debug)]
pub(crate) struct ConnTimes {
    start: Instant,
    secure: bool,
    dns: OnceLock<(Instant, Instant)>,
    tcp_end: OnceLock<Instant>,
    end: OnceLock<Instant>,
}

impl ConnTimes {
    pub(crate) fn new(secure: bool) -> Arc<ConnTimes> {
        Arc::new(ConnTimes {
            start: Instant::now(),
            secure,
            dns: OnceLock::new(),
            tcp_end: OnceLock::new(),
            end: OnceLock::new(),
        })
    }
    pub(crate) fn tcp_connected(&self) {
        let _ = self.tcp_end.set(Instant::now());
    }
    /// The connection is handed to hyper (after TLS for https).
    pub(crate) fn ready(&self) {
        let _ = self.end.get_or_init(Instant::now);
    }
}

/// Response extension: the times of the connection the response came over.
#[derive(Clone, Debug)]
pub(crate) struct ConnInfo(pub(crate) Arc<ConnTimes>);

impl ConnInfo {
    pub(crate) fn timing(&self) -> ConnectionTiming {
        let t = &self.0;
        let tcp_end = t.tcp_end.get().copied().unwrap_or(t.start);
        ConnectionTiming {
            start: t.start,
            dns: t.dns.get().copied(),
            tcp_end,
            secure: t.secure,
            end: t.end.get().copied().unwrap_or(tcp_end),
        }
    }
}

tokio::task_local! {
    /// The connection being opened by the current connector future.
    pub(crate) static CONNECTING: Arc<ConnTimes>;
    /// The phases of the fetch the current request task runs, copied into
    /// every response head it makes.
    pub(crate) static FETCH_TIMING: std::cell::Cell<NetTiming>;
}

/// Updates the current request task's timing (nothing outside one).
pub(crate) fn note(f: impl FnOnce(&mut NetTiming)) {
    let _ = FETCH_TIMING.try_with(|cell| {
        let mut timing = cell.get();
        f(&mut timing);
        cell.set(timing);
    });
}

/// The current request task's timing (default outside one).
pub(crate) fn current() -> NetTiming {
    FETCH_TIMING.try_with(std::cell::Cell::get).unwrap_or_default()
}

/// A response answered without the network (cache, static file,
/// interception): the request and response "start" now, unless the network
/// already got that far (a 304 revalidation).
pub(crate) fn served_locally() {
    note(|t| {
        let now = Instant::now();
        t.request_start.get_or_insert(now);
        t.response_start.get_or_insert(now);
    });
}

/// `getaddrinfo` (hyper's resolver), noting the lookup's times on the
/// connection being opened.
#[derive(Clone)]
pub(crate) struct TimedResolver(GaiResolver);

impl TimedResolver {
    pub(crate) fn new() -> Self {
        TimedResolver(GaiResolver::new())
    }
}

impl Service<Name> for TimedResolver {
    type Response = GaiAddrs;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<GaiAddrs, io::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let times = CONNECTING.try_with(Arc::clone).ok();
        let start = Instant::now();
        let resolving = self.0.call(name);
        Box::pin(async move {
            let result = resolving.await;
            if let Some(times) = times {
                let _ = times.dns.set((start, Instant::now()));
            }
            result
        })
    }
}

/// Fetch "TAO check" of one hop: `url`'s response passes for `origin` (the
/// requesting document's serialized origin) when it is same-origin or its
/// `Timing-Allow-Origin` lists `*` or that origin.
pub fn timing_allowed(url: &url::Url, headers: &http::HeaderMap, origin: &str) -> bool {
    let listed = headers
        .get_all("timing-allow-origin")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .any(|v| v == "*" || v == origin);
    listed || url.origin().ascii_serialization() == origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_allow_origin_lists() {
        let url = url::Url::parse("https://b.test/x").unwrap();
        let mut headers = http::HeaderMap::new();
        assert!(!timing_allowed(&url, &headers, "https://a.test"));
        assert!(timing_allowed(&url, &headers, "https://b.test"));
        headers.append("timing-allow-origin", "https://c.test, https://a.test".parse().unwrap());
        assert!(timing_allowed(&url, &headers, "https://a.test"));
        assert!(!timing_allowed(&url, &headers, "https://d.test"));
        headers.append("timing-allow-origin", "*".parse().unwrap());
        assert!(timing_allowed(&url, &headers, "https://d.test"));
        // A value is an origin, not a prefix or a host.
        let mut headers = http::HeaderMap::new();
        headers.insert("timing-allow-origin", "a.test".parse().unwrap());
        assert!(!timing_allowed(&url, &headers, "https://a.test"));
    }
}
