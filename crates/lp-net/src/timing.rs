//! When the phases of a fetch happened, for Resource Timing and Navigation
//! Timing: plain `Instant`s on the response head ([`NetTiming`]), filled in as
//! the I/O thread goes. A new connection reports its DNS, TCP and TLS times
//! through the connector: [`ProxyConnector`](crate::proxy) stamps them into a
//! [`ConnInfo`] that hyper copies into every response of that connection, and
//! the resolver finds the connection it resolves for in a task-local.
//!
//! The cost per request is a few `Instant::now()` calls; per new connection
//! one small allocation.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Instant;

use hyper_util::client::legacy::connect::dns::{GaiResolver, Name};
use tokio::sync::watch;
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

/// Host name resolution for the connector: `getaddrinfo` (hyper's resolver)
/// with one lookup per name in flight at a time, so concurrent connects to
/// one host (a page's six parallel connections to a fresh origin, each of
/// which hyper resolves on its own) share a single `getaddrinfo` instead of
/// each running one on a blocking-pool thread; `localhost` is loopback
/// without a lookup (RFC 6761), in `getaddrinfo`'s order. Nothing is kept
/// once a lookup ends (no cache): every later connect resolves again. Each
/// connect notes the lookup's times on the connection it opens.
#[derive(Clone)]
pub(crate) struct TimedResolver<R = GaiResolver> {
    inner: R,
    inflight: Inflight,
}

/// Addresses of one lookup, shared by the connects that waited for it.
type Resolved = Result<Arc<[SocketAddr]>, Arc<io::Error>>;

/// Lookups in flight by host name: followers wait on the leader's channel.
type Inflight = Arc<Mutex<HashMap<Box<str>, watch::Receiver<Option<Resolved>>>>>;

type Addrs = std::vec::IntoIter<SocketAddr>;

impl TimedResolver {
    pub(crate) fn new() -> Self {
        Self::over(GaiResolver::new())
    }
}

impl<R> TimedResolver<R> {
    pub(crate) fn over(inner: R) -> Self {
        TimedResolver { inner, inflight: Inflight::default() }
    }
}

/// `localhost` (with or without the root dot) as `getaddrinfo` answers it
/// on macOS and glibc: `::1` first, then `127.0.0.1`. Other names, including
/// `*.localhost`, still go to the system resolver as before.
fn loopback(name: &str) -> Option<Vec<SocketAddr>> {
    let host = name.strip_suffix('.').unwrap_or(name);
    host.eq_ignore_ascii_case("localhost").then(|| {
        vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0), SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)]
    })
}

/// The lookup a connect leads for its name: it publishes the result to the
/// followers and takes the name out of the in-flight map, also when the
/// connect is dropped before the lookup ends (the followers then look up on
/// their own).
struct Leader {
    inflight: Inflight,
    name: Box<str>,
    tx: watch::Sender<Option<Resolved>>,
}

impl Leader {
    fn finish(self, result: &Result<Vec<SocketAddr>, io::Error>) {
        let shared: Resolved = match result {
            Ok(addrs) => Ok(Arc::from(addrs.as_slice())),
            Err(e) => Err(Arc::new(io::Error::new(e.kind(), e.to_string()))),
        };
        let _ = self.tx.send(Some(shared));
        // The entry goes with the sender (`Drop`).
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        let mut inflight = self.inflight.lock().unwrap();
        // Only this lookup's entry: a later leader of the same name keeps its own.
        if inflight.get(&*self.name).is_some_and(|rx| rx.same_channel(&self.tx.subscribe())) {
            inflight.remove(&*self.name);
        }
    }
}

/// A follower's copy of what the leader resolved.
fn shared_result(shared: &Resolved) -> Result<Addrs, io::Error> {
    match shared {
        Ok(addrs) => Ok(addrs.to_vec().into_iter()),
        Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
    }
}

impl<R> Service<Name> for TimedResolver<R>
where
    R: Service<Name, Error = io::Error> + Clone + Send + 'static,
    R::Response: Iterator<Item = SocketAddr>,
    R::Future: Send,
{
    type Response = Addrs;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Addrs, io::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let times = CONNECTING.try_with(Arc::clone).ok();
        let start = Instant::now();
        let noted = move |result: Result<Addrs, io::Error>| {
            if let Some(times) = times {
                let _ = times.dns.set((start, Instant::now()));
            }
            result
        };
        if let Some(addrs) = loopback(name.as_str()) {
            return Box::pin(std::future::ready(noted(Ok(addrs.into_iter()))));
        }
        let mut inflight = self.inflight.lock().unwrap();
        if let Some(rx) = inflight.get(name.as_str()) {
            let mut rx = rx.clone();
            drop(inflight);
            let mut inner = self.inner.clone();
            return Box::pin(async move {
                let shared = rx.wait_for(|r| r.is_some()).await.ok().map(|r| r.clone().expect("waited for Some"));
                let result = match shared {
                    Some(shared) => shared_result(&shared),
                    // The leading connect was dropped before its lookup ended.
                    None => inner.call(name).await.map(|addrs| addrs.collect::<Vec<_>>().into_iter()),
                };
                noted(result)
            });
        }
        let (tx, rx) = watch::channel(None);
        inflight.insert(name.as_str().into(), rx);
        drop(inflight);
        let leader = Leader { inflight: self.inflight.clone(), name: name.as_str().into(), tx };
        let resolving = self.inner.call(name);
        Box::pin(async move {
            let result = resolving.await.map(|addrs| addrs.collect::<Vec<_>>());
            leader.finish(&result);
            noted(result.map(Vec::into_iter))
        })
    }
}

/// Records the TAO check of one hop (network or cache) on the current
/// request task: once a hop fails, the fetch's details stay hidden. No
/// requesting origin (a browser-initiated navigation): nothing to check.
pub(crate) fn note_timing_allow(url: &url::Url, headers: &http::HeaderMap, origin: Option<&str>) {
    if origin.is_some_and(|origin| !timing_allowed(url, headers, origin)) {
        note(|t| t.timing_allow_failed = true);
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

    /// A resolver that answers every name with one address once released,
    /// counting its lookups.
    #[derive(Clone)]
    struct Fake {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        gate: Arc<tokio::sync::Notify>,
        fail: bool,
    }

    impl Fake {
        fn new(fail: bool) -> Self {
            Fake { calls: Default::default(), gate: Default::default(), fail }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    const ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 0);

    impl Service<Name> for Fake {
        type Response = Addrs;
        type Error = io::Error;
        type Future = Pin<Box<dyn Future<Output = Result<Addrs, io::Error>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, _name: Name) -> Self::Future {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (gate, fail) = (self.gate.clone(), self.fail);
            Box::pin(async move {
                gate.notified().await;
                if fail {
                    Err(io::Error::new(io::ErrorKind::NotFound, "failed to lookup address information"))
                } else {
                    Ok(vec![ADDR].into_iter())
                }
            })
        }
    }

    fn name(host: &str) -> Name {
        host.parse().unwrap()
    }

    async fn settle() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn concurrent_lookups_of_one_name_share_a_single_getaddrinfo() {
        let fake = Fake::new(false);
        let resolver = TimedResolver::over(fake.clone());
        let tasks: Vec<_> = (0..5)
            .map(|_| {
                let mut r = resolver.clone();
                tokio::spawn(async move { r.call(name("example.test")).await.map(|a| a.collect::<Vec<_>>()) })
            })
            .collect();
        // Another name is its own lookup.
        let mut other = resolver.clone();
        let other = tokio::spawn(async move { other.call(name("other.test")).await.map(|a| a.collect::<Vec<_>>()) });
        settle().await;
        assert_eq!(fake.calls(), 2, "five connects to one host: one lookup; the other host: one");
        assert_eq!(resolver.inflight.lock().unwrap().len(), 2);
        fake.gate.notify_waiters();
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), vec![ADDR]);
        }
        assert_eq!(other.await.unwrap().unwrap(), vec![ADDR]);
        assert!(resolver.inflight.lock().unwrap().is_empty(), "nothing is kept after the lookups: no cache");
        // Later connects look up again.
        let mut r = resolver.clone();
        let again = tokio::spawn(async move { r.call(name("example.test")).await.map(|a| a.collect::<Vec<_>>()) });
        settle().await;
        assert_eq!(fake.calls(), 3);
        fake.gate.notify_waiters();
        assert_eq!(again.await.unwrap().unwrap(), vec![ADDR]);
    }

    #[tokio::test]
    async fn followers_share_the_leaders_failure_too() {
        let fake = Fake::new(true);
        let resolver = TimedResolver::over(fake.clone());
        let tasks: Vec<_> = (0..3)
            .map(|_| {
                let mut r = resolver.clone();
                tokio::spawn(async move { r.call(name("missing.test")).await.map(|a| a.collect::<Vec<_>>()) })
            })
            .collect();
        settle().await;
        fake.gate.notify_waiters();
        for task in tasks {
            let err = task.await.unwrap().unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
            assert!(err.to_string().contains("failed to lookup"), "{err}");
        }
        assert_eq!(fake.calls(), 1);
    }

    #[tokio::test]
    async fn a_dropped_leader_leaves_its_followers_to_look_up_themselves() {
        let fake = Fake::new(false);
        let resolver = TimedResolver::over(fake.clone());
        let mut r = resolver.clone();
        let leader = tokio::spawn(async move { r.call(name("example.test")).await.map(|a| a.collect::<Vec<_>>()) });
        settle().await;
        let mut r = resolver.clone();
        let follower = tokio::spawn(async move { r.call(name("example.test")).await.map(|a| a.collect::<Vec<_>>()) });
        settle().await;
        assert_eq!(fake.calls(), 1);
        leader.abort();
        let _ = leader.await;
        settle().await;
        assert_eq!(fake.calls(), 2, "the follower resolves on its own");
        assert!(resolver.inflight.lock().unwrap().is_empty(), "the dropped leader took its entry out");
        fake.gate.notify_waiters();
        assert_eq!(follower.await.unwrap().unwrap(), vec![ADDR]);
    }

    #[tokio::test]
    async fn localhost_is_loopback_without_a_lookup() {
        let fake = Fake::new(false);
        let mut resolver = TimedResolver::over(fake.clone());
        for host in ["localhost", "localhost.", "LocalHost"] {
            let addrs: Vec<_> = resolver.call(name(host)).await.unwrap().collect();
            assert_eq!(
                addrs,
                [SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0), SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)],
                "{host}"
            );
        }
        assert_eq!(fake.calls(), 0);
        assert!(resolver.inflight.lock().unwrap().is_empty());
        // Subdomains are not (the system resolver decides, as before).
        let mut r = resolver.clone();
        let sub = tokio::spawn(async move { r.call(name("app.localhost")).await.map(|a| a.collect::<Vec<_>>()) });
        settle().await;
        assert_eq!(fake.calls(), 1);
        fake.gate.notify_waiters();
        assert_eq!(sub.await.unwrap().unwrap(), vec![ADDR]);
    }

    /// Review (perf/network C6): dropping a follower leaves the leader and
    /// the other followers alone; a leader dropped while two followers wait
    /// leaves the name free for a fresh leader; every waiter gets the
    /// failure's kind.
    #[tokio::test]
    async fn review_follower_drop_and_releader_after_leader_drop() {
        let fake = Fake::new(false);
        let resolver = TimedResolver::over(fake.clone());
        let spawn = |host: &'static str| {
            let mut r = resolver.clone();
            tokio::spawn(async move { r.call(name(host)).await.map(|a| a.collect::<Vec<_>>()) })
        };
        let leader = spawn("a.test");
        settle().await;
        let f1 = spawn("a.test");
        let f2 = spawn("a.test");
        settle().await;
        f1.abort();
        let _ = f1.await;
        settle().await;
        assert_eq!(fake.calls(), 1, "a dropped follower costs nothing");
        fake.gate.notify_waiters();
        assert_eq!(leader.await.unwrap().unwrap(), vec![ADDR]);
        assert_eq!(f2.await.unwrap().unwrap(), vec![ADDR]);
        assert!(resolver.inflight.lock().unwrap().is_empty());

        // Leader dropped with two followers: both look up on their own, and a
        // later caller leads a new shared lookup (the old entry is gone).
        let leader = spawn("b.test");
        settle().await;
        let (g1, g2) = (spawn("b.test"), spawn("b.test"));
        settle().await;
        leader.abort();
        let _ = leader.await;
        settle().await;
        assert_eq!(fake.calls(), 4, "1 (a) + 1 (dropped leader) + 2 (followers on their own)");
        let late = spawn("b.test");
        let late2 = spawn("b.test");
        settle().await;
        assert_eq!(fake.calls(), 5, "the late callers share one new lookup");
        assert_eq!(resolver.inflight.lock().unwrap().len(), 1);
        fake.gate.notify_waiters();
        for t in [g1, g2, late, late2] {
            assert_eq!(t.await.unwrap().unwrap(), vec![ADDR]);
        }
        assert!(resolver.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn review_localhost_spellings_and_subdomains() {
        let fake = Fake::new(true);
        let mut resolver = TimedResolver::over(fake.clone());
        for host in ["LOCALHOST", "LOCALHOST.", "localhost"] {
            let addrs: Vec<_> = resolver.call(name(host)).await.unwrap().collect();
            assert_eq!(addrs.len(), 2, "{host}");
            assert!(addrs.iter().all(|a| a.ip().is_loopback()), "{host}");
        }
        assert_eq!(fake.calls(), 0);
        // Not localhost: the system resolver (here failing) decides.
        for host in ["localhost..", "localhostx", "a.localhost", "localhost.test"] {
            let mut r = resolver.clone();
            let t = tokio::spawn(async move { r.call(name(host)).await.map(|a| a.collect::<Vec<_>>()) });
            settle().await;
            fake.gate.notify_waiters();
            assert!(t.await.unwrap().is_err(), "{host}");
        }
        assert_eq!(fake.calls(), 4);
    }
}
