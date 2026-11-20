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

use pyro_dom::layout::{LayoutBody, LayoutBodyKey, SubresourceBodies, SubresourceKind, SubresourceLoader, SubresourceOptions};
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

/// The requests whose network load failed for one document.
type Failed = Arc<std::sync::Mutex<std::collections::HashSet<LayoutBodyKey>>>;

/// Loads one document's sub-resources synchronously: `data:` and `file:`
/// URLs and `serve --static` roots from disk, the rest through the
/// document's network (its bodies kept for it alone, `LayoutBodyKey`).
struct LocalFiles {
    statics: pyro_net::StaticFiles,
    /// The document's network (`None`: it has none, and loads nothing from
    /// the network).
    loader: Option<Arc<dyn SubresourceLoader>>,
    /// What failed for this document, which does not ask for it again.
    failed: Failed,
}

/// A layout fetch in flight, counted until the loader drops its callback.
struct InFlight {
    bodies: Arc<SubresourceBodies>,
    key: LayoutBodyKey,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // No answer (the document went first): not in flight. (A failure is recorded below.)
        self.bodies.abandon(&self.key);
        let last = IN_FLIGHT.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1;
        if last && PDF_WAITING.load(std::sync::atomic::Ordering::Acquire) {
            wake_pdf();
        }
    }
}

/// pyro: a renderer's request options as layout's loader takes them.
fn layout_options(options: &pyro_render::FetchOptions) -> SubresourceOptions {
    SubresourceOptions {
        crossorigin: options.cors.map(|credentials| match credentials {
            pyro_render::CorsCredentials::SameOrigin => "anonymous".to_owned(),
            pyro_render::CorsCredentials::Include => "use-credentials".to_owned(),
        }),
        integrity: options.integrity.clone(),
        referrer: options.referrer.as_ref().map(|url| url.as_str().to_owned()),
        referrer_policy: options.referrer_policy.clone(),
    }
}

impl LocalFiles {
    /// The document's network body of `url` fetched as `options` say. A miss starts the fetch in
    /// the background (into the document's bodies) and answers `None` now: a layout never waits
    /// for the network (a request held by interception needs this thread).
    fn network(&self, url: &pyro_net::Url, options: &SubresourceOptions) -> Option<Arc<LayoutBody>> {
        // No network of its own: nothing goes out, never through another
        // context's client.
        let loader = self.loader.as_ref()?;
        let bodies = loader.bodies().clone();
        // pyro: the document's own request: its client and the whole request (`LayoutBodyKey`).
        let key = LayoutBodyKey::new(&loader.document(), url.as_str(), options);
        if self.failed.lock().is_ok_and(|failed| failed.contains(&key)) {
            return None;
        }
        // pyro: when this document first needed it (`SubresourceBodies::late`).
        bodies.needed(&key);
        if let Some(body) = bodies.get(&key) {
            return Some(body);
        }
        let path = url.path().to_ascii_lowercase();
        let font = [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e));
        let plain_cors = options.crossorigin.as_deref() == Some("anonymous")
            && options.integrity.is_none()
            && options.referrer.is_none()
            && options.referrer_policy.is_none();
        let kind = if plain_cors {
            SubresourceKind::CorsImage
        } else if font {
            SubresourceKind::Font
        } else if path.ends_with(".css") || url.host_str() == Some("fonts.googleapis.com") {
            SubresourceKind::Style
        } else {
            SubresourceKind::Image
        };
        match kind {
            SubresourceKind::Font => loader.font_asked(&key),
            // pyro: a CSS image this document's layout asked for: its `load` waits for it.
            SubresourceKind::Image | SubresourceKind::CorsImage => loader.image_asked(&key),
            SubresourceKind::Style => {}
        }
        if !bodies.start(&key) {
            // In flight already (this document asked before).
            return None;
        }
        IN_FLIGHT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fetch = InFlight { bodies, key };
        let failed = self.failed.clone();
        loader.load(
            url.as_str().to_owned(),
            kind,
            options,
            Box::new(move |result| match result {
                // The body with its final URL, tainting and status, written together.
                Ok(body) if !body.bytes.is_empty() => fetch.bodies.finish(fetch.key.clone(), body),
                // An empty body counts as a failure (nothing to lay out with).
                _ => {
                    if let Ok(mut failed) = failed.lock() {
                        failed.insert(fetch.key.clone());
                    }
                    fetch.bodies.fail(&fetch.key);
                }
            }),
        );
        None
    }

    /// `url` as `options` say: a local body, or the document's network body (with its final URL
    /// and tainting from the stored value).
    fn load(&self, url: String, options: &SubresourceOptions, done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>) {
        let body = (|| {
            // A data: URL (as serialized by the url crate) holds its body.
            if url.starts_with("data:") {
                return pyro_net::data_url::parse_str(&url).map(|(_, b)| Arc::new(LayoutBody::local(&url, b.to_vec())));
            }
            let u = pyro_net::Url::parse(&url).ok()?;
            let local = match u.scheme() {
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
            };
            // pyro: a local body with integrity metadata (`integrity()`) is used only when it matches.
            local
                .filter(|body| options.integrity.as_deref().is_none_or(|i| pyro_webapi::sri::matches(i, body)))
                .map(|body| Arc::new(LayoutBody::local(&url, body)))
        })();
        let cors = options.crossorigin.is_some();
        done(
            body.map(|body| pyro_render::Fetched {
                body: body.bytes.to_vec(),
                url: body.final_url.clone(),
                opaque: !cors && body.opaque,
            })
            .ok_or_else(|| format!("{url}: not a local resource")),
        );
    }
}

impl Fetcher for LocalFiles {
    /// pyro: by the document's timeline in its bodies.
    fn late(&self, url: &str, options: &pyro_render::FetchOptions, period: std::time::Duration) -> Option<bool> {
        let loader = self.loader.as_ref()?;
        let key = LayoutBodyKey::new(&loader.document(), url, &layout_options(options));
        loader.bodies().late(&key, period)
    }

    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.load(url, &SubresourceOptions::default(), body_only(done))
    }

    /// pyro: as the screenshot's fetcher does, so layout queries and screenshots agree.
    fn fetch_with(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.load(url, &layout_options(options), body_only(done))
    }

    /// `data:` and `file:` URLs and `serve --static` roots are this process's own; the network's
    /// responses pass the CORS check or fail.
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let options = SubresourceOptions { crossorigin: Some("anonymous".to_owned()), ..Default::default() };
        self.load(url, &options, body_only(done))
    }

    /// pyro: with the response's URL after redirects and its tainting, from the stored body, so a
    /// redirected sheet resolves its URLs as in screenshots.
    fn fetch_final(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
        self.load(url, &layout_options(options), done)
    }
}

/// `done` for the body alone.
fn body_only(
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) -> Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send> {
    Box::new(move |result| done(result.map(|fetched| fetched.body)))
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

    /// One document's network: answers every load at once with "ok" (from `final_url`, with
    /// `opaque`), recording (document, URL).
    struct Loader {
        document: &'static str,
        bodies: Arc<SubresourceBodies>,
        log: Arc<Mutex<Vec<(&'static str, String)>>>,
        final_url: Option<&'static str>,
        opaque: bool,
    }
    impl SubresourceLoader for Loader {
        fn load(
            &self,
            url: String,
            _kind: SubresourceKind,
            _options: &SubresourceOptions,
            done: Box<dyn FnOnce(Result<LayoutBody, String>) + Send>,
        ) {
            self.log.lock().unwrap().push((self.document, url.clone()));
            let final_url = self.final_url.map_or(url, str::to_owned);
            done(Ok(LayoutBody { bytes: Arc::new(b"ok".to_vec()), final_url, opaque: self.opaque, status: 200 }));
        }
        fn bodies(&self) -> &Arc<SubresourceBodies> {
            &self.bodies
        }
        fn document(&self) -> String {
            self.document.to_owned()
        }
    }

    fn files(
        bodies: &Arc<SubresourceBodies>,
        log: &Arc<Mutex<Vec<(&'static str, String)>>>,
        document: &'static str,
        final_url: Option<&'static str>,
        opaque: bool,
    ) -> LocalFiles {
        let loader = Loader { document, bodies: bodies.clone(), log: log.clone(), final_url, opaque };
        LocalFiles {
            statics: pyro_net::StaticFiles::default(),
            loader: Some(Arc::new(loader) as Arc<dyn SubresourceLoader>),
            failed: Arc::default(),
        }
    }

    fn loads(log: &Arc<Mutex<Vec<(&'static str, String)>>>, document: &str) -> usize {
        log.lock().unwrap().iter().filter(|(d, _)| *d == document).count()
    }

    /// Every body is its requesting document's alone, whatever the request (plain or CORS): another
    /// document sharing the store, a same-origin one with other cookies included (frame F2 of the
    /// same origin, another loader of the same frame), makes its own request. Plain bodies were
    /// kept under the bare URL, modified ones per origin (security review cycles 1-3, layout body
    /// cache).
    #[test]
    fn a_body_is_its_documents_alone() {
        let bodies = Arc::new(SubresourceBodies::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let (a, sibling, next) =
            (files(&bodies, &log, "F1#L1", None, false), files(&bodies, &log, "F2#L2", None, false), files(&bodies, &log, "F1#L3", None, false));
        let url = pyro_net::Url::parse("http://img.test/shape.png").unwrap();
        for options in [SubresourceOptions::default(), SubresourceOptions { crossorigin: Some("anonymous".into()), ..Default::default() }] {
            assert_eq!(a.network(&url, &options), None, "the first load starts");
            assert_eq!(a.network(&url, &options).map(|b| b.bytes.to_vec()), Some(b"ok".to_vec()), "A reuses its own");
            assert_eq!(sibling.network(&url, &options), None, "a same-origin sibling asks for itself");
            assert_eq!(next.network(&url, &options), None, "the frame's next document asks for itself");
        }
        assert_eq!((loads(&log, "F1#L1"), loads(&log, "F2#L2"), loads(&log, "F1#L3")), (2, 2, 2));
    }

    /// A body's URL after redirects and its tainting are stored with it, under its own request's
    /// key, and geometry reports them as stored (it said `opaque: false` for every body and took
    /// the final URL from a bare-URL table).
    #[test]
    fn final_url_and_tainting_are_the_stored_bodys() {
        let bodies = Arc::new(SubresourceBodies::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let hop = files(&bodies, &log, "F1#L1", Some("http://elsewhere.test/hop.png"), true);
        let plain = files(&bodies, &log, "F2#L2", None, false);
        let url = "http://img.test/hop.png";
        let fetched = |files: &LocalFiles| {
            let got = Arc::new(Mutex::new(None));
            let sink = got.clone();
            files.fetch_final(url.to_owned(), &pyro_render::FetchOptions::default(), Box::new(move |r| *sink.lock().unwrap() = Some(r)));
            let r = got.lock().unwrap().take().expect("answered");
            r.map(|r| (r.url, r.opaque))
        };
        // The first ask starts the load (answered at once here); the next takes the stored body.
        let _ = fetched(&hop);
        let _ = fetched(&plain);
        assert_eq!(fetched(&hop), Ok(("http://elsewhere.test/hop.png".to_owned(), true)));
        assert_eq!(fetched(&plain), Ok((url.to_owned(), false)));
        let key = LayoutBodyKey::new("F1#L1", url, &SubresourceOptions::default());
        let stored = bodies.get(&key).expect("stored");
        assert_eq!((stored.final_url.as_str(), stored.opaque, stored.status), ("http://elsewhere.test/hop.png", true, 200));
    }

    /// What a document loaded, or still waits for, goes with it (a navigation): nothing of it
    /// serves the frame's next document, and no load of it is still waited for.
    #[test]
    fn a_forgotten_document_leaves_nothing() {
        let bodies = Arc::new(SubresourceBodies::default());
        let key = LayoutBodyKey::new("F1#L1", "http://img.test/a.png", &SubresourceOptions::default());
        let pending = LayoutBodyKey::new("F1#L1", "http://img.test/b.png", &SubresourceOptions::default());
        bodies.finish(key.clone(), LayoutBody::local("http://img.test/a.png", b"a".to_vec()));
        assert!(bodies.start(&pending));
        bodies.forget_document("F1#L1");
        assert!(bodies.get(&key).is_none() && !bodies.pending(&pending));
    }
}
