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

use pyro_dom::layout::{SubresourceBodies, SubresourceKind, SubresourceLoader, SubresourceOptions};
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
    /// With `options` other than the defaults (CORS mode, integrity, referrer: a CSS `url()`'s
    /// request URL modifiers), fetched as they say and cached apart.
    fn network(&self, url: &pyro_net::Url, options: &SubresourceOptions) -> Option<Vec<u8>> {
        // No network of its own: nothing goes out, never through another
        // context's client.
        let loader = self.loader.as_ref()?;
        let bodies = loader.bodies().clone();
        let cors = options.crossorigin.is_some();
        let plain_cors = cors
            && options.crossorigin.as_deref() == Some("anonymous")
            && options.integrity.is_none()
            && options.referrer.is_none()
            && options.referrer_policy.is_none();
        // pyro: a body loaded with other than the defaults (a CORS response passed for this
        // document's origin) is this document's alone: the frames sharing the bodies do not reuse
        // it without their own check (`SubresourceOptions::body_key`).
        let key = options.body_key(url.as_str(), loader.document().as_deref());
        if self.failed.lock().is_ok_and(|failed| failed.contains(&key)) {
            return None;
        }
        {
            let mut cache = bodies.lock();
            match cache.get(&key) {
                Some(b) if !b.is_empty() => return Some(b.to_vec()),
                Some(_) => {
                    // pyro: in flight for another document of the identity: this one waits on it too.
                    let path = url.path().to_ascii_lowercase();
                    if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
                        loader.font_asked(url.as_str());
                    } else if !path.ends_with(".css") {
                        loader.image_asked(&key);
                    }
                    return None;
                }
                // An empty entry marks the fetch in flight.
                None => cache.insert(key.clone(), Arc::new(Vec::new())),
            };
        }
        IN_FLIGHT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fetch = InFlight { bodies, failed: self.failed.clone(), key };
        let path = url.path().to_ascii_lowercase();
        let kind = if plain_cors {
            SubresourceKind::CorsImage
        } else if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
            loader.font_asked(url.as_str());
            SubresourceKind::Font
        } else if path.ends_with(".css") || url.host_str() == Some("fonts.googleapis.com") {
            SubresourceKind::Style
        } else {
            SubresourceKind::Image
        };
        // pyro: a CSS image this document's layout asked for: its `load` waits for it.
        if matches!(kind, SubresourceKind::Image | SubresourceKind::CorsImage) {
            loader.image_asked(&fetch.key);
        }
        loader.load_with(
            url.as_str().to_owned(),
            kind,
            options,
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
        self.load(url, &SubresourceOptions::default(), done)
    }

    /// pyro: as the screenshot's fetcher does, so layout queries and screenshots agree.
    fn fetch_with(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        let options = SubresourceOptions {
            crossorigin: options.cors.map(|credentials| match credentials {
                pyro_render::CorsCredentials::SameOrigin => "anonymous".to_owned(),
                pyro_render::CorsCredentials::Include => "use-credentials".to_owned(),
            }),
            integrity: options.integrity.clone(),
            referrer: options.referrer.as_ref().map(|url| url.as_str().to_owned()),
            referrer_policy: options.referrer_policy.clone(),
        };
        self.load(url, &options, done)
    }

    /// `data:` and `file:` URLs and `serve --static` roots are this process's own; the network's
    /// responses pass the CORS check or fail.
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let options = SubresourceOptions { crossorigin: Some("anonymous".to_owned()), ..Default::default() };
        self.load(url, &options, done)
    }

    /// pyro: with the response's URL after redirects as the document's network recorded it, so a
    /// redirected sheet resolves its URLs as in screenshots.
    fn fetch_final(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
        let bodies = self.loader.as_ref().map(|l| l.bodies().clone());
        let asked = url.clone();
        let done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> = Box::new(move |result| {
            let url = bodies.as_ref().map_or_else(|| asked.clone(), |b| b.final_url(&asked));
            done(result.map(|body| pyro_render::Fetched { body, url, opaque: false }))
        });
        if options.is_default() {
            self.fetch(url, done)
        } else {
            self.fetch_with(url, options, done)
        }
    }
}

impl LocalFiles {
    fn load(&self, url: String, options: &SubresourceOptions, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
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
                        // The loader checks a network response's integrity.
                        None if matches!(u.scheme(), "http" | "https") => return self.network(&u, options),
                        None => None,
                    },
                },
            }
        })();
        // pyro: a local body with integrity metadata (`integrity()`) is used only when it matches.
        let body = body.filter(|body| options.integrity.as_deref().is_none_or(|i| pyro_webapi::sri::matches(i, body)));
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

#[cfg(test)]
mod document_bodies_tests {
    use super::*;
    use std::sync::Mutex;

    /// One document's network over shared bodies: answers every load at once with "ok",
    /// recording (document, URL).
    struct Loader {
        document: &'static str,
        bodies: Arc<SubresourceBodies>,
        log: Arc<Mutex<Vec<(&'static str, String)>>>,
    }
    impl SubresourceLoader for Loader {
        fn load(&self, url: String, _kind: SubresourceKind, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            self.log.lock().unwrap().push((self.document, url));
            done(Ok(b"ok".to_vec()));
        }
        fn bodies(&self) -> &Arc<SubresourceBodies> {
            &self.bodies
        }
        fn document(&self) -> Option<String> {
            Some(self.document.to_owned())
        }
    }

    /// A body loaded in CORS mode for one document (whose origin the server allowed) is not
    /// served to another document sharing the bodies (a sandboxed or cross-origin sibling frame),
    /// which makes its own request and check; a plain body is shared. The bodies were keyed by
    /// URL and modifiers alone (security review cycle 1, HIGH 2).
    #[test]
    fn a_cors_body_is_its_documents_alone() {
        let bodies = Arc::new(SubresourceBodies::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let files = |document: &'static str| LocalFiles {
            statics: pyro_net::StaticFiles::default(),
            loader: Some(
                Arc::new(Loader { document, bodies: bodies.clone(), log: log.clone() }) as Arc<dyn SubresourceLoader>
            ),
            failed: Arc::default(),
        };
        let (a, b) = (files("A"), files("B"));
        let cors = SubresourceOptions { crossorigin: Some("anonymous".into()), ..Default::default() };
        let url = pyro_net::Url::parse("http://img.test/shape.png").unwrap();
        assert_eq!(a.network(&url, &cors), None, "the first load starts");
        assert_eq!(a.network(&url, &cors), Some(b"ok".to_vec()), "A reuses its own");
        assert_eq!(b.network(&url, &cors), None, "B asks for itself");
        assert_eq!(log.lock().unwrap().iter().filter(|(d, _)| *d == "B").count(), 1);
        let plain = SubresourceOptions::default();
        assert_eq!(a.network(&url, &plain), None);
        assert_eq!(b.network(&url, &plain), Some(b"ok".to_vec()), "a plain body is shared");
    }
}
