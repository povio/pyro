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

type Delivery = (Box<dyn NetHandler>, String, Bytes);

pub(crate) fn new_cache() -> Cache {
    Cache::default()
}

struct Provider {
    /// The page's loader and sheets of the latest snapshot: a mirrored
    /// document keeps its provider across renders.
    fetcher: Mutex<Arc<dyn Fetcher>>,
    sheets: Mutex<HashMap<String, String>>,
    cache: Cache,
    tx: Sender<Delivery>,
    outstanding: Arc<AtomicUsize>,
}

impl NetProvider for Provider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.to_string();
        let options = request.options;
        // A CORS fetch's body (or one fetched with other options) is cached apart: one fetched
        // without CORS never stands in for it.
        let key = options.cache_key(&url);
        let plain = options.is_default();
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
            self.sheets
                .lock()
                .ok()
                .and_then(|s| s.get(&url).map(|text| Bytes::from(utf8_sheet(text))))
                .or_else(|| self.cache.lock().ok().and_then(|c| c.get(&url).cloned()))
        };
        if let Some(bytes) = ready {
            let _ = self.tx.send((handler, url, bytes));
            return;
        }
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        let fetcher = self.fetcher.lock().map(|f| f.clone());
        let Ok(fetcher) = fetcher else { return };
        let done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> = Box::new(move |result| {
            let bytes = match result {
                Ok(body) => {
                    let bytes = Bytes::from(body);
                    if !inline {
                        if let Ok(mut c) = cache.lock() {
                            c.insert(key, bytes.clone());
                        }
                    }
                    bytes
                }
                // Empty bytes: Blitz's handlers treat it as a failed load.
                Err(_) => Bytes::new(),
            };
            let _ = tx.send((handler, url, bytes));
        });
        if plain {
            fetcher.fetch(request.url.to_string(), done);
        } else if options == (blitz_traits::net::FetchOptions {
            cors: Some(blitz_traits::net::CorsCredentials::SameOrigin),
            ..Default::default()
        }) {
            fetcher.fetch_cors(request.url.to_string(), done);
        } else {
            fetcher.fetch_with(request.url.to_string(), &options, done);
        }
    }
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
            cache,
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
                    Ok((handler, url, bytes)) => {
                        handler.bytes(url, bytes);
                        self.outstanding.fetch_sub(1, Ordering::AcqRel);
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
        while let Ok((handler, url, bytes)) = bridge.rx.try_recv() {
            handler.bytes(url, bytes);
        }
        assert_eq!(*recorder.0.lock().unwrap(), vec![(url.to_string(), false), (url.to_string(), true)]);
        assert_eq!(*got.lock().unwrap(), vec![Bytes::from_static(b"no-cors"), Bytes::new()]);
    }
}
