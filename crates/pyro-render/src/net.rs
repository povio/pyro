//! Sub-resources for Blitz: `<link>` sheets come from the snapshot, other
//! URLs (images, fonts, `@import`) go to a [`Fetcher`], which loads them
//! through the page's own network stack (cookies, mocks, HTTP cache). The
//! bytes come back over a channel and are handed to Blitz on the render
//! thread, so decoding never runs on the browser thread.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blitz_dom::BaseDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

use crate::Snapshot;

/// Loads a sub-resource the renderer needs. `done` may be called on any
/// thread, at most once; an error renders the resource as missing.
pub trait Fetcher: Send + Sync + 'static {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>);
    /// [`Self::fetch`] in CORS mode, credentials only to the same origin ("anonymous", as
    /// `shape-outside` images load): a cross-origin response the CORS check does not pass is an
    /// error. A fetcher that cannot check fails it (the default).
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err(format!("{url}: no CORS fetch")));
    }
    /// [`Self::fetch`] with `options` other than a sub-resource's defaults (a CSS `url()`'s
    /// request URL modifiers): its mode and credentials, integrity metadata, referrer and
    /// referrer policy. A response the CORS check or the integrity check does not pass is an
    /// error. A fetcher that cannot honour them all makes a CORS fetch of the URL when the mode
    /// is "cors" and fails one with integrity metadata (the default).
    fn fetch_with(
        &self,
        url: String,
        options: &blitz_traits::net::FetchOptions,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        if options.integrity.is_some() {
            done(Err(format!("{url}: no integrity check")));
        } else if options.cors.is_some() {
            self.fetch_cors(url, done);
        } else {
            self.fetch(url, done);
        }
    }
    /// pyro: [`Self::fetch`], [`Self::fetch_cors`] or [`Self::fetch_with`] as `options` say,
    /// `done` also getting the response's URL after redirects (a style sheet resolves its URLs
    /// against it). A fetcher that does not know it reports `url` (the default).
    /// pyro: whether a render need not wait for `url` (fetched with `options`): an image the
    /// document never fetched itself (a style change since its load), which Chrome paints only once
    /// it has arrived. Its bytes reach the document at a later render of it (the default: none).
    /// pyro: whether a request of the document `client` names (`None`: the snapshot's page
    /// document) may be answered at all, before any sheet or cache lookup: a client that is no
    /// live document of the snapshot gets nothing (the default: all).
    fn admits(&self, _client: Option<&str>) -> bool {
        true
    }
    fn defers(&self, _url: &str, _options: &blitz_traits::net::FetchOptions) -> bool {
        false
    }
    /// pyro: whether `url` (fetched with `options`) came more than `period` after its document
    /// first needed it, by the document's timeline (its layout's and its screenshots' alike):
    /// a `font-display: optional` font is then not used (`NetHandler::block_period`). `None`
    /// (the default): not known, the font is used.
    fn late(&self, _url: &str, _options: &blitz_traits::net::FetchOptions, _period: std::time::Duration) -> Option<bool> {
        None
    }
    fn fetch_final(
        &self,
        url: String,
        options: &blitz_traits::net::FetchOptions,
        done: Box<dyn FnOnce(Result<Fetched, String>) + Send>,
    ) {
        let asked = url.clone();
        let done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> =
            Box::new(move |r| done(r.map(|body| Fetched { body, url: asked, opaque: false, referrer_policy: None })));
        if options.is_default() {
            self.fetch(url, done);
        } else if is_plain_cors(options) {
            self.fetch_cors(url, done);
        } else {
            self.fetch_with(url, options, done);
        }
    }
}

/// pyro: a response as [`Fetcher::fetch_final`] reports it.
#[derive(Clone, Debug)]
pub struct Fetched {
    /// The body of a successful response.
    pub body: Vec<u8>,
    /// Its URL after redirects.
    pub url: String,
    /// Its tainting is "opaque" (Fetch): a no-cors request that reached another origin at any
    /// hop, though it ended back at the document's.
    pub opaque: bool,
    /// pyro: the `Referrer-Policy` it was served with (a style sheet's requests follow it).
    pub referrer_policy: Option<String>,
}

/// pyro: whether `options` ask only for a CORS fetch with credentials to the same origin
/// ([`Fetcher::fetch_cors`]).
pub fn is_plain_cors(options: &blitz_traits::net::FetchOptions) -> bool {
    // (Whatever its destination.)
    blitz_traits::net::FetchOptions { destination: Default::default(), ..options.clone() }
        == blitz_traits::net::FetchOptions {
            cors: Some(blitz_traits::net::CorsCredentials::SameOrigin),
            ..Default::default()
        }
}

type Cache = Arc<Mutex<HashMap<String, Bytes>>>;

/// Bytes fetched for one document (`Snapshot::resource_scope`), reused by
/// later renders of it.
#[derive(Default)]
pub(crate) struct ResourceCache {
    scope: String,
    map: Cache,
}

impl ResourceCache {
    pub(crate) fn scoped(&mut self, scope: &str) -> Cache {
        if scope.is_empty() || scope != self.scope {
            self.scope = scope.to_owned();
            self.map = Cache::default();
        }
        self.map.clone()
    }
}

/// A handler, the URL asked for, the response's URL after redirects, whether its tainting is
/// opaque, the body, and whether the render waits for it (counted in `outstanding`).
type Delivery = (Box<dyn NetHandler>, String, Meta, Bytes, bool);

/// pyro: what a response says beyond its body: its URL after redirects, tainting and policy.
#[derive(Clone, Debug)]
struct Meta {
    final_url: String,
    opaque: bool,
    referrer_policy: Option<String>,
}

impl Meta {
    fn of(url: &str) -> Meta {
        Meta { final_url: url.to_owned(), opaque: false, referrer_policy: None }
    }
}

pub(crate) fn new_cache() -> Cache {
    Cache::default()
}

struct Provider {
    /// The page's loader and sheets of the latest snapshot: a mirrored
    /// document keeps its provider across renders.
    fetcher: Mutex<Arc<dyn Fetcher>>,
    /// The page document's own linked sheets by URL.
    sheets: Mutex<HashMap<String, String>>,
    /// pyro: each frame document's own linked sheets by URL, by its client (from the frame
    /// table): a frame's sheet is served only to it, never from the page's map or another
    /// frame's.
    frame_sheets: Mutex<HashMap<String, std::collections::BTreeMap<String, String>>>,
    cache: Cache,
    /// pyro: the URL after redirects of a cached response that was redirected, or whose tainting
    /// is opaque, by cache key, with that tainting.
    finals: Arc<Mutex<HashMap<String, Meta>>>,
    /// pyro: the client of each frame document's requests, by document id
    /// (`NetProvider::document_client`).
    clients: Mutex<HashMap<usize, String>>,
    tx: Sender<Delivery>,
    outstanding: Arc<AtomicUsize>,
}

impl NetProvider for Provider {
    fn document_client(&self, doc_id: usize, client: String) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.insert(doc_id, client);
        }
    }

    fn fetch(&self, doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.to_string();
        let mut options = request.options;
        // pyro: every request of a frame's document is that document's (the fetcher sends one
        // that names none as the page's).
        if options.client.is_none() {
            options.client = self.clients.lock().ok().and_then(|c| c.get(&doc_id).cloned());
        }
        // pyro: the client is admitted before anything is looked up, sheets and cache included.
        let admitted = self.fetcher.lock().ok().is_some_and(|f| f.admits(options.client.as_deref()));
        if !admitted {
            self.outstanding.fetch_add(1, Ordering::AcqRel);
            let meta = Meta::of(&url);
            let _ = self.tx.send((handler, url, meta, Bytes::new(), true));
            return;
        }
        // A CORS fetch's body (or one fetched with other options) is cached apart: one fetched
        // without CORS never stands in for it.
        let key = options.cache_key(&url);
        // (A frame document's own sheets come with the snapshot too.)
        let plain = blitz_traits::net::FetchOptions { client: None, ..options.clone() }.is_default();
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        // A data: URL holds its body: neither the sheets nor the cache (keyed
        // by the whole, possibly megabytes long, URL) can hold it for less.
        let inline = url.starts_with("data:");
        let ready = if inline {
            None
        } else if !plain {
            self.cache.lock().ok().and_then(|c| c.get(&key).cloned())
        } else {
            // A sheet's text is decoded already: as UTF-8 behind a byte order
            // mark, Blitz cannot decode it again by an `@charset` it starts with.
            // pyro: a frame document's from its own sheets only, the page's from the page's.
            let text = match options.client.as_deref() {
                Some(client) => self.frame_sheets.lock().ok().and_then(|s| s.get(client)?.get(&url).cloned()),
                None => self.sheets.lock().ok().and_then(|s| s.get(&url).cloned()),
            };
            text.map(|text| Bytes::from(utf8_sheet(&text)))
                .or_else(|| self.cache.lock().ok().and_then(|c| c.get(&key).cloned()))
        };
        // pyro: a `font-display: optional` font that came after its block period is a failed load
        // (`NetHandler::block_period`, by the document's timeline: `Fetcher::late`).
        let period = handler.block_period();
        let late_fetcher = period.and_then(|_| self.fetcher.lock().ok().map(|f| f.clone()));
        let late = {
            let (url, options) = (url.clone(), options.clone());
            move || match (period, &late_fetcher) {
                (Some(period), Some(fetcher)) => fetcher.late(&url, &options, period) == Some(true),
                _ => false,
            }
        };
        let ready = ready.map(|bytes| if late() { Bytes::new() } else { bytes });
        if let Some(bytes) = ready {
            let meta = self.finals.lock().ok().and_then(|f| f.get(&key).cloned()).unwrap_or_else(|| Meta::of(&url));
            let _ = self.tx.send((handler, url, meta, bytes, true));
            return;
        }
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        let finals = self.finals.clone();
        let fetcher = self.fetcher.lock().map(|f| f.clone());
        let Ok(fetcher) = fetcher else { return };
        // pyro: a fetch the render need not wait for (`Fetcher::defers`) is not outstanding: its
        // bytes reach the document at a later settle of it.
        let counted = inline || !fetcher.defers(&url, &options);
        if !counted {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
        }
        let done: Box<dyn FnOnce(Result<Fetched, String>) + Send> = Box::new(move |result| {
            let (bytes, meta) = match result {
                Ok(Fetched { body, url: final_url, opaque, referrer_policy }) => {
                    let bytes = Bytes::from(body);
                    let meta = Meta { final_url, opaque, referrer_policy };
                    if !inline {
                        if meta.final_url != url || meta.opaque || meta.referrer_policy.is_some() {
                            if let Ok(mut f) = finals.lock() {
                                f.insert(key.clone(), meta.clone());
                            }
                        }
                        if let Ok(mut c) = cache.lock() {
                            c.insert(key, bytes.clone());
                        }
                    }
                    if late() { (Bytes::new(), meta) } else { (bytes, meta) }
                }
                // Empty bytes: Blitz's handlers treat it as a failed load.
                Err(_) => (Bytes::new(), Meta::of(&url)),
            };
            let _ = tx.send((handler, url, meta, bytes, counted));
        });
        fetcher.fetch_final(request.url.to_string(), &options, done);
    }
}

/// pyro: each frame document's sheets in `table` (nested ones too), by its client.
fn frame_sheets(table: &blitz_dom::FrameTable) -> HashMap<String, std::collections::BTreeMap<String, String>> {
    let mut out = HashMap::new();
    let mut stack = vec![table.clone()];
    while let Some(table) = stack.pop() {
        for frame in table.entries.values() {
            out.insert(frame.client.clone(), frame.sheets.clone());
            stack.push(frame.frames.clone());
        }
    }
    out
}

/// `text` as UTF-8 behind a byte order mark, which outranks every other
/// source of a style sheet's encoding (CSS Syntax).
fn utf8_sheet(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    bytes.extend_from_slice(b"\xEF\xBB\xBF");
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

pub(crate) struct Bridge {
    provider: Arc<Provider>,
    rx: Receiver<Delivery>,
    outstanding: Arc<AtomicUsize>,
}

impl Bridge {
    pub(crate) fn new(snap: &Snapshot, fetcher: Arc<dyn Fetcher>, cache: Cache) -> Bridge {
        let (tx, rx) = channel();
        let outstanding = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(Provider {
            fetcher: Mutex::new(fetcher),
            sheets: Mutex::new(snap.sheets.clone()),
            frame_sheets: Mutex::new(frame_sheets(&snap.frames)),
            cache,
            finals: Arc::default(),
            clients: Mutex::default(),
            tx,
            outstanding: outstanding.clone(),
        });
        Bridge { provider, rx, outstanding }
    }

    /// Serves `snap`'s sheets and fetches through `fetcher` from now on (a
    /// mirrored document's next render).
    pub(crate) fn retarget(&self, snap: &Snapshot, fetcher: Arc<dyn Fetcher>) {
        if let Ok(mut f) = self.provider.fetcher.lock() {
            *f = fetcher;
        }
        if let Ok(mut s) = self.provider.sheets.lock() {
            s.clone_from(&snap.sheets);
        }
        self.set_frames(&snap.frames);
    }

    /// pyro: the frame table now (its frames' sheets).
    pub(crate) fn set_frames(&self, frames: &blitz_dom::FrameTable) {
        if let Ok(mut s) = self.provider.frame_sheets.lock() {
            *s = frame_sheets(frames);
        }
    }

    pub(crate) fn provider(&self) -> Arc<dyn NetProvider> {
        self.provider.clone()
    }

    /// Resolves the document until no sub-resource is outstanding (styles
    /// can request images and fonts, sheets can request more), or until
    /// `deadline`. Returns the time spent waiting for fetches and the number
    /// of style and layout passes, and whether the latest resource state
    /// was fully resolved with no follow-up style/layout damage.
    ///
    /// The first layout waits for what the document has asked for so far
    /// (sheets, `@font-face` fonts, images), at most `first_layout_wait`
    /// past the sheets, so a page with web fonts is laid out once, with its
    /// fonts, instead of once with fallback fonts and again when they
    /// arrive. Past that wait it lays out with what arrived and goes on
    /// waiting for the rest until `deadline`.
    pub(crate) fn settle(
        &self,
        doc: &mut BaseDocument,
        deadline: Instant,
        first_layout_wait: Duration,
        style_only: bool,
    ) -> (Duration, u32, bool) {
        let mut waited = Duration::ZERO;
        let mut passes = 0;
        let mut first_layout = Some(Instant::now() + first_layout_wait);
        // pyro: what arrived since the last settle, deferred fetches' bytes included.
        while let Ok((handler, url, meta, bytes, counted)) = self.rx.try_recv() {
            handler.bytes_meta(url, meta.final_url, meta.opaque, meta.referrer_policy, bytes);
            if counted {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
            }
        }
        loop {
            let wait_until = first_layout.map_or(deadline, |t| t.min(deadline));
            while self.outstanding.load(Ordering::Acquire) > 0 {
                let t = Instant::now();
                let next = match self.rx.try_recv() {
                    Ok(d) => Ok(d),
                    Err(_) => self.rx.recv_timeout(wait_until.saturating_duration_since(t)),
                };
                waited += t.elapsed();
                match next {
                    Ok((handler, url, meta, bytes, counted)) => {
                        handler.bytes_meta(url, meta.final_url, meta.opaque, meta.referrer_policy, bytes);
                        if counted {
                            self.outstanding.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) if first_layout.is_some() && Instant::now() < deadline => {
                        // Lay out with what arrived; keep waiting afterwards.
                        break;
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        // Render with what arrived; pending sheets would block
                        // painting, so they are dropped.
                        doc.handle_messages();
                        // Even prior positive passes can precede these newly
                        // handled bytes. The caller must resolve them now.
                        return (waited, passes, false);
                    }
                }
            }
            doc.handle_messages();
            // Taking a sheet can ask for more (fonts, `@import`).
            if first_layout.is_some_and(|t| Instant::now() < t) && self.outstanding.load(Ordering::Acquire) > 0 {
                continue;
            }
            first_layout = None;
            match style_only {
                true => doc.resolve_style(0.0),
                false => doc.resolve(0.0),
            }
            passes += 1;
            if self.outstanding.load(Ordering::Acquire) == 0 {
                // A resolve may refresh hover after clearing damage, leaving
                // another style pass necessary. Only elide a truly clean pass.
                let clean = doc.try_root_element().is_some_and(|root| {
                    !root.has_dirty_descendants()
                        && !root.has_damaged_descendants()
                        && !root.has_snapshot()
                        && root.damage().is_some_and(|damage| damage.is_empty())
                });
                return (waited, passes, clean);
            }
        }
    }
}

#[cfg(test)]
mod cors_tests {
    use super::*;

    /// Answers every fetch with its URL's bytes, recording (URL, CORS).
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, bool)>>);
    impl Fetcher for Arc<Recorder> {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0.lock().unwrap().push((url.clone(), false));
            done(Ok(b"no-cors".to_vec()));
        }
        fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.0.lock().unwrap().push((url.clone(), true));
            done(Err("CORS check failed".into()));
        }
    }

    struct Sink(Arc<Mutex<Vec<Bytes>>>);
    impl NetHandler for Sink {
        fn bytes(self: Box<Self>, _url: String, bytes: Bytes) {
            self.0.lock().unwrap().push(bytes);
        }
    }

    /// A CORS request goes to `fetch_cors`, and a body fetched without CORS for the same URL never
    /// answers it from the cache (nor its failure the other).
    #[test]
    fn cors_requests_are_fetched_and_cached_apart() {
        let recorder = Arc::new(Recorder::default());
        let bridge = Bridge::new(&Snapshot::default(), Arc::new(recorder.clone()), new_cache());
        let provider = bridge.provider();
        let got = Arc::new(Mutex::new(Vec::new()));
        let url = "http://other.test/a.png";
        provider.fetch(0, Request::get(url.parse().unwrap()), Box::new(Sink(got.clone())));
        let mut cors = Request::get(url.parse().unwrap());
        blitz_dom::net::mark_cors(&mut cors);
        provider.fetch(0, cors, Box::new(Sink(got.clone())));
        while let Ok((handler, url, meta, bytes, _)) = bridge.rx.try_recv() {
            handler.bytes_meta(url, meta.final_url, meta.opaque, meta.referrer_policy, bytes);
        }
        assert_eq!(*recorder.0.lock().unwrap(), vec![(url.to_string(), false), (url.to_string(), true)]);
        assert_eq!(*got.lock().unwrap(), vec![Bytes::from_static(b"no-cors"), Bytes::new()]);
    }
}
