//! Blitz geometry for the engine's pages (`pyro_layout`): sub-resources resolve
//! against the document's URL and load synchronously when they are local:
//! `data:` URLs, `file:` URLs and `serve --static` roots. Network ones are
//! fetched in the background through the document's own network
//! (`pyro_dom::layout::SubresourceLoader`, which pyro-browser gives every
//! document): as its page's sub-resource requests, through its browser
//! context's proxy, user agent, cookies, headers, offline state and request
//! interception, attributed to its frame. A document without one fetches
//! nothing: no request of a page goes out through another context's client.
//!
//! Bodies are kept by URL per network identity
//! (`pyro_dom::layout::SubresourceBodies`): the contexts that go out as the
//! browser does share one set for the process's life, a context with a proxy
//! or user agent of its own has its own. A layout never waits for the network,
//! and later documents of the same identity lay out with them at once; a body
//! fetched through one identity never serves another's document. A failed load
//! is remembered by its document only, since the failure may be its page's
//! own (interception, cookies).

use std::rc::Rc;
use std::sync::Arc;

use pyro_dom::layout::{SubresourceBodies, SubresourceKind, SubresourceLoader};
use pyro_render::Fetcher;

pub(crate) use pyro_layout::{set_fork_template, take_stats};

/// Network fetches for layouts still in flight.
static IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A `printToPDF` waits for the fetches in flight: the last one to finish
/// calls [`PDF_WAKER`] (from the network thread).
static PDF_WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
type Waker = Box<dyn FnOnce() + Send>;
static PDF_WAKER: std::sync::Mutex<Option<Waker>> = std::sync::Mutex::new(None);

/// Makes this thread's new documents take their geometry from Blitz, each
/// loading its sub-resources through its own network.
pub(crate) fn install(statics: pyro_net::StaticFiles) {
    pyro_layout::install_with(Rc::new(move |loader: Option<&Arc<dyn SubresourceLoader>>| {
        Arc::new(LocalFiles { statics: statics.clone(), loader: loader.cloned(), failed: Arc::default() })
            as Arc<dyn Fetcher>
    }));
}

/// URLs whose network load failed for one document.
type Failed = Arc<std::sync::Mutex<std::collections::HashSet<String>>>;

/// Loads one document's sub-resources synchronously: `data:` and `file:`
/// URLs and `serve --static` roots from disk, the rest through the
/// document's network (cached per identity).
struct LocalFiles {
    statics: pyro_net::StaticFiles,
    /// The document's network (`None`: it has none, and loads nothing from
    /// the network).
    loader: Option<Arc<dyn SubresourceLoader>>,
    /// What failed for this document, which does not ask for it again. Only
    /// bodies are shared with the identity: a failure may be this page's
    /// own (its interception aborted the request, its cookies got a 401),
    /// and the identity's other documents ask again.
    failed: Failed,
}

/// A layout fetch in flight, counted until the loader drops its callback.
struct InFlight {
    bodies: Arc<SubresourceBodies>,
    failed: Failed,
    key: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // No body (a failure, or no answer because the document went first):
        // the identity's next document asks again.
        {
            let mut bodies = self.bodies.lock();
            if bodies.get(&self.key).is_some_and(|b| b.is_empty()) {
                bodies.remove(&self.key);
            }
        }
        let last = IN_FLIGHT.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1;
        if last && PDF_WAITING.load(std::sync::atomic::Ordering::Acquire) {
            wake_pdf();
        }
    }
}

impl LocalFiles {
    /// A network body from the identity's cache. A miss starts the fetch in
    /// the background (into the cache) and answers `None` now: a layout
    /// never waits for the network (a request held by interception needs
    /// this thread), and the identity's next documents get the body at once.
    fn network(&self, url: &pyro_net::Url) -> Option<Vec<u8>> {
        // No network of its own: nothing goes out, never through another
        // context's client.
        let loader = self.loader.as_ref()?;
        let bodies = loader.bodies().clone();
        let key = url.as_str().to_owned();
        if self.failed.lock().is_ok_and(|failed| failed.contains(&key)) {
            return None;
        }
        {
            let mut cache = bodies.lock();
            match cache.get(&key) {
                Some(b) if !b.is_empty() => return Some(b.to_vec()),
                Some(_) => return None,
                // An empty entry marks the fetch in flight.
                None => cache.insert(key.clone(), Arc::new(Vec::new())),
            };
        }
        IN_FLIGHT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fetch = InFlight { bodies, failed: self.failed.clone(), key };
        let path = url.path().to_ascii_lowercase();
        let kind = if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
            SubresourceKind::Font
        } else if path.ends_with(".css") || url.host_str() == Some("fonts.googleapis.com") {
            SubresourceKind::Style
        } else {
            SubresourceKind::Image
        };
        loader.load(
            url.as_str().to_owned(),
            kind,
            Box::new(move |result| match result {
                Ok(body) if !body.is_empty() => {
                    fetch.bodies.lock().insert(fetch.key.clone(), Arc::new(body));
                    fetch.bodies.arrived();
                }
                // An empty body counts as a failure (nothing to lay out with).
                _ => {
                    if let Ok(mut failed) = fetch.failed.lock() {
                        failed.insert(fetch.key.clone());
                    }
                    // pyro: no longer in flight (`document.fonts` waits on empty entries).
                    let mut bodies = fetch.bodies.lock();
                    if bodies.get(&fetch.key).is_some_and(|b| b.is_empty()) {
                        bodies.remove(&fetch.key);
                    }
                }
            }),
        );
        None
    }
}

impl Fetcher for LocalFiles {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = (|| {
            // A data: URL (as serialized by the url crate) holds its body.
            if url.starts_with("data:") {
                return pyro_net::data_url::parse_str(&url).map(|(_, b)| b.to_vec());
            }
            let u = pyro_net::Url::parse(&url).ok()?;
            match u.scheme() {
                "data" => pyro_net::data_url::parse(&u).map(|(_, b)| b.to_vec()),
                "file" => std::fs::read(u.to_file_path().ok()?).ok(),
                _ => match self.statics.lookup(&u, false) {
                    Some(hit) => Some(hit.file.body.to_vec()),
                    None => match self.statics.resolve(&u, false) {
                        Some(hit) => std::fs::read(hit.path).ok(),
                        None if matches!(u.scheme(), "http" | "https") => self.network(&u),
                        None => None,
                    },
                },
            }
        })();
        done(body.ok_or_else(|| format!("{url}: not a local resource")));
    }
}

fn wake_pdf() {
    PDF_WAITING.store(false, std::sync::atomic::Ordering::Release);
    let waker = PDF_WAKER.lock().ok().and_then(|mut w| w.take());
    if let Some(wake) = waker {
        wake();
    }
}

/// Whether layout sub-resources are still being fetched (a PDF printed now
/// would lack them).
pub(crate) fn fetching() -> bool {
    IN_FLIGHT.load(std::sync::atomic::Ordering::Acquire) > 0
}

/// Calls `wake` once no layout fetch is in flight (at once if none is),
/// replacing an earlier waker.
pub(crate) fn wake_when_fetched(wake: Waker) {
    if let Ok(mut w) = PDF_WAKER.lock() {
        *w = Some(wake);
    }
    PDF_WAITING.store(true, std::sync::atomic::Ordering::Release);
    if !fetching() {
        wake_pdf();
    }
}

/// Brings the page document's own Blitz document (the one its layout
/// queries were answered from) up to date for `printToPDF`, which starts
/// its sub-resource fetches ([`fetching`]). `None`: the document has no
/// Blitz geometry (print through the render thread).
pub(crate) fn sync_for_pdf(page_dom: &pyro_webapi::PageDom) -> Option<()> {
    let dom = page_dom.dom.borrow();
    page_dom.layout().synced_geometry_source(&dom).map(|_| ())
}

/// The PDF pages of the page document's Blitz document, painted on this
/// thread; the render thread writes them.
pub(crate) fn paint_pdf(
    page_dom: &pyro_webapi::PageDom,
    opts: &pyro_render::PdfOptions,
) -> Option<Result<pyro_render::PdfPaint, String>> {
    pyro_layout::paint_pdf(&page_dom.dom.borrow(), &page_dom.layout(), opts)
}
