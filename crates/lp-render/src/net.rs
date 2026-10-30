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

use blitz_html::HtmlDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

use crate::Snapshot;

/// Loads a sub-resource the renderer needs. `done` may be called on any
/// thread, at most once; an error renders the resource as missing.
pub trait Fetcher: Send + Sync + 'static {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>);
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

struct Provider {
    fetcher: Arc<dyn Fetcher>,
    sheets: HashMap<String, String>,
    cache: Cache,
    tx: Sender<Delivery>,
    outstanding: Arc<AtomicUsize>,
}

impl NetProvider for Provider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.to_string();
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        let ready = self
            .sheets
            .get(&url)
            .map(|text| Bytes::from(text.clone().into_bytes()))
            .or_else(|| self.cache.lock().ok().and_then(|c| c.get(&url).cloned()));
        if let Some(bytes) = ready {
            let _ = self.tx.send((handler, url, bytes));
            return;
        }
        let tx = self.tx.clone();
        let cache = self.cache.clone();
        self.fetcher.fetch(
            url.clone(),
            Box::new(move |result| {
                let bytes = match result {
                    Ok(body) => {
                        let bytes = Bytes::from(body);
                        if let Ok(mut c) = cache.lock() {
                            c.insert(url.clone(), bytes.clone());
                        }
                        bytes
                    }
                    // Empty bytes: Blitz's handlers treat it as a failed load.
                    Err(_) => Bytes::new(),
                };
                let _ = tx.send((handler, url, bytes));
            }),
        );
    }
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
        let provider =
            Arc::new(Provider { fetcher, sheets: snap.sheets.clone(), cache, tx, outstanding: outstanding.clone() });
        Bridge { provider, rx, outstanding }
    }

    pub(crate) fn provider(&self) -> Arc<dyn NetProvider> {
        self.provider.clone()
    }

    /// Resolves the document until no sub-resource is outstanding (styles
    /// can request images and fonts, sheets can request more), or until
    /// `deadline`. Returns the time spent waiting for fetches.
    pub(crate) fn settle(&self, doc: &mut HtmlDocument, deadline: Instant) -> Duration {
        let mut waited = Duration::ZERO;
        loop {
            doc.resolve(0.0);
            if self.outstanding.load(Ordering::Acquire) == 0 {
                return waited;
            }
            while self.outstanding.load(Ordering::Acquire) > 0 {
                let t = Instant::now();
                let next = match self.rx.try_recv() {
                    Ok(d) => Ok(d),
                    Err(_) => self.rx.recv_timeout(deadline.saturating_duration_since(t)),
                };
                waited += t.elapsed();
                match next {
                    Ok((handler, url, bytes)) => {
                        handler.bytes(url, bytes);
                        self.outstanding.fetch_sub(1, Ordering::AcqRel);
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        // Render with what arrived; pending sheets would block
                        // painting, so they are dropped.
                        doc.handle_messages();
                        return waited;
                    }
                }
            }
            doc.handle_messages();
        }
    }
}
