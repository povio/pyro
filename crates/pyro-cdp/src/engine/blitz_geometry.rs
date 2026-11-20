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

/// Network fetches for layouts still in flight, by the document whose geometry made them (its
/// client): a PDF waits for its own document's, never another page's.
static IN_FLIGHT: std::sync::Mutex<Option<std::collections::HashMap<String, usize>>> = std::sync::Mutex::new(None);

/// A `printToPDF` waits for its document's fetches in flight: the last one to finish calls the
/// wakers waiting for that document (from the network thread). Every waiting PDF keeps its own.
type Waker = Box<dyn FnOnce() + Send>;
static PDF_WAKERS: std::sync::Mutex<Vec<(String, Waker)>> = std::sync::Mutex::new(Vec::new());

/// Makes this thread's new documents take their geometry from Blitz, each
/// loading its sub-resources through its own network.
pub(crate) fn install(statics: pyro_net::StaticFiles) {
    pyro_layout::install_with(Rc::new(move |loader: Option<&Arc<dyn SubresourceLoader>>| {
        Arc::new(LocalFiles { statics: statics.clone(), loader: loader.cloned() })
            as Arc<dyn Fetcher>
    }));
}

/// Loads one document's sub-resources synchronously: `data:` and `file:`
/// URLs and `serve --static` roots from disk, the rest through the
/// document's network (its bodies kept for it alone, `LayoutBodyKey`).
struct LocalFiles {
    statics: pyro_net::StaticFiles,
    /// The document's network (`None`: it has none, and loads nothing from
    /// the network).
    loader: Option<Arc<dyn SubresourceLoader>>,
}

/// A layout fetch in flight, counted until the loader drops its callback.
struct InFlight {
    bodies: Arc<SubresourceBodies>,
    key: LayoutBodyKey,
    /// The document whose geometry made it (counted under it).
    owner: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        // No answer (the document went first): not in flight. (A failure is recorded below.)
        self.bodies.abandon(&self.key);
        let last = {
            let mut counts = IN_FLIGHT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let counts = counts.get_or_insert_with(Default::default);
            match counts.get_mut(&self.owner) {
                Some(n) if *n > 1 => {
                    *n -= 1;
                    false
                }
                _ => {
                    counts.remove(&self.owner);
                    true
                }
            }
        };
        if last {
            wake_pdfs(&self.owner);
        }
    }
}

/// pyro: what a request is used as, by its destination (an image's CORS mode by its options).
pub(crate) fn subresource_kind(options: &SubresourceOptions, destination: pyro_render::Destination) -> SubresourceKind {
    let plain_cors = options.crossorigin.as_deref() == Some("anonymous")
        && options.integrity.is_none()
        && options.referrer.is_none()
        && options.referrer_policy.is_none();
    match destination {
        pyro_render::Destination::Font => SubresourceKind::Font,
        pyro_render::Destination::Style => SubresourceKind::Style,
        _ if plain_cors => SubresourceKind::CorsImage,
        _ => SubresourceKind::Image,
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
    fn network(
        &self,
        url: &pyro_net::Url,
        options: &SubresourceOptions,
        destination: pyro_render::Destination,
        client: Option<&str>,
    ) -> Option<Arc<LayoutBody>> {
        // No network of its own: nothing goes out, never through another
        // context's client.
        let loader = self.loader.as_ref()?;
        let bodies = loader.bodies().clone();
        // pyro: the request's own client (a frame document this geometry renders names its own),
        // else this document's; and the whole request (`LayoutBodyKey`).
        let document = client.map_or_else(|| loader.document(), str::to_owned);
        // pyro: what the response is used as comes from where the request was made (its
        // destination), never from its URL: an image at `image.css` is an image.
        let kind = subresource_kind(options, destination);
        let key = LayoutBodyKey::new(&document, url.as_str(), options, kind);
        // pyro: one failure state, the store's (capture reads the same).
        if bodies.has_failed(&key) {
            return None;
        }
        // pyro: when this document first needed it (`SubresourceBodies::late`).
        bodies.needed(&key);
        if let Some(body) = bodies.get(&key) {
            return Some(body);
        }
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
        let owner = loader.document();
        {
            let mut counts = IN_FLIGHT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            *counts.get_or_insert_with(Default::default).entry(owner.clone()).or_insert(0) += 1;
        }
        let fetch = InFlight { bodies, key, owner };
        loader.load(
            &document,
            url.as_str().to_owned(),
            kind,
            options,
            Box::new(move |result| match result {
                // The body with its final URL, tainting and status, written together.
                Ok(body) if !body.bytes.is_empty() => fetch.bodies.finish(fetch.key.clone(), body),
                // An empty body counts as a failure (nothing to lay out with).
                _ => fetch.bodies.fail(&fetch.key),
            }),
        );
        None
    }

    /// `url` as `options` say: a local body, or the document's network body (with its final URL
    /// and tainting from the stored value).
    fn load(
        &self,
        url: String,
        options: &SubresourceOptions,
        destination: pyro_render::Destination,
        client: Option<&str>,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
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
                        None if matches!(u.scheme(), "http" | "https") => return self.network(&u, options, destination, client),
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
                referrer_policy: body.referrer_policy.clone(),
            })
            .ok_or_else(|| format!("{url}: not a local resource")),
        );
    }
}

impl Fetcher for LocalFiles {
    /// pyro: by the document's timeline in its bodies.
    fn late(&self, url: &str, options: &pyro_render::FetchOptions, period: std::time::Duration) -> Option<bool> {
        let loader = self.loader.as_ref()?;
        let document = options.client.clone().unwrap_or_else(|| loader.document());
        let layout = layout_options(options);
        let key = LayoutBodyKey::new(&document, url, &layout, subresource_kind(&layout, options.destination));
        loader.bodies().late(&key, period)
    }

    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.load(url, &SubresourceOptions::default(), Default::default(), None, body_only(done))
    }

    /// pyro: as the screenshot's fetcher does, so layout queries and screenshots agree.
    fn fetch_with(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.load(url, &layout_options(options), options.destination, options.client.as_deref(), body_only(done))
    }

    /// `data:` and `file:` URLs and `serve --static` roots are this process's own; the network's
    /// responses pass the CORS check or fail.
    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let options = SubresourceOptions { crossorigin: Some("anonymous".to_owned()), ..Default::default() };
        self.load(url, &options, pyro_render::Destination::Image, None, body_only(done))
    }

    /// pyro: with the response's URL after redirects and its tainting, from the stored body, so a
    /// redirected sheet resolves its URLs as in screenshots.
    fn fetch_final(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
        self.load(url, &layout_options(options), options.destination, options.client.as_deref(), done)
    }
}

/// `done` for the body alone.
fn body_only(
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) -> Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send> {
    Box::new(move |result| done(result.map(|fetched| fetched.body)))
}

/// Calls the wakers waiting for `document`'s fetches.
fn wake_pdfs(document: &str) {
    let woken: Vec<Waker> = {
        let mut wakers = PDF_WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (woken, kept) = std::mem::take(&mut *wakers).into_iter().partition(|(d, _)| d == document);
        *wakers = kept;
        woken.into_iter().map(|(_, wake)| wake).collect()
    };
    for wake in woken {
        wake();
    }
}

/// Whether `document`'s layout sub-resources are still being fetched (a PDF of it printed now
/// would lack them).
pub(crate) fn fetching(document: &str) -> bool {
    IN_FLIGHT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|c| c.contains_key(document))
}

/// Calls `wake` once no layout fetch of `document` is in flight (at once if none is).
pub(crate) fn wake_when_fetched(document: &str, wake: Waker) {
    PDF_WAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((document.to_owned(), wake));
    if !fetching(document) {
        wake_pdfs(document);
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
        log: Arc<Mutex<Vec<(String, String)>>>,
        final_url: Option<&'static str>,
        opaque: bool,
        kinds: Arc<Mutex<Vec<SubresourceKind>>>,
        asked_images: Arc<Mutex<Vec<String>>>,
    }
    impl SubresourceLoader for Loader {
        fn load(
            &self,
            document: &str,
            url: String,
            kind: SubresourceKind,
            _options: &SubresourceOptions,
            done: Box<dyn FnOnce(Result<LayoutBody, String>) + Send>,
        ) {
            self.kinds.lock().unwrap().push(kind);
            self.log.lock().unwrap().push((document.to_owned(), url.clone()));
            let final_url = self.final_url.map_or(url, str::to_owned);
            done(Ok(LayoutBody { bytes: Arc::new(b"ok".to_vec()), final_url, opaque: self.opaque, status: 200, referrer_policy: None }));
        }
        fn bodies(&self) -> &Arc<SubresourceBodies> {
            &self.bodies
        }
        fn document(&self) -> String {
            self.document.to_owned()
        }
        fn image_asked(&self, key: &LayoutBodyKey) {
            self.asked_images.lock().unwrap().push(key.url().to_owned());
        }
    }

    fn files(
        bodies: &Arc<SubresourceBodies>,
        log: &Arc<Mutex<Vec<(String, String)>>>,
        document: &'static str,
        final_url: Option<&'static str>,
        opaque: bool,
    ) -> LocalFiles {
        let loader = Loader {
            document,
            bodies: bodies.clone(),
            log: log.clone(),
            final_url,
            opaque,
            kinds: Arc::default(),
            asked_images: Arc::default(),
        };
        LocalFiles {
            statics: pyro_net::StaticFiles::default(),
            loader: Some(Arc::new(loader) as Arc<dyn SubresourceLoader>),
        }
    }

    fn loads(log: &Arc<Mutex<Vec<(String, String)>>>, document: &str) -> usize {
        log.lock().unwrap().iter().filter(|(d, _)| d == document).count()
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
            assert_eq!(a.network(&url, &options, pyro_render::Destination::Image, None), None, "the first load starts");
            assert_eq!(a.network(&url, &options, pyro_render::Destination::Image, None).map(|b| b.bytes.to_vec()), Some(b"ok".to_vec()), "A reuses its own");
            assert_eq!(sibling.network(&url, &options, pyro_render::Destination::Image, None), None, "a same-origin sibling asks for itself");
            assert_eq!(next.network(&url, &options, pyro_render::Destination::Image, None), None, "the frame's next document asks for itself");
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
        let key = LayoutBodyKey::new("F1#L1", url, &SubresourceOptions::default(), SubresourceKind::Image);
        let stored = bodies.get(&key).expect("stored");
        assert_eq!((stored.final_url.as_str(), stored.opaque, stored.status), ("http://elsewhere.test/hop.png", true, 200));
    }

    /// What a document loaded, or still waits for, goes with it (a navigation): nothing of it
    /// serves the frame's next document, and no load of it is still waited for.
    #[test]
    fn a_forgotten_document_leaves_nothing() {
        let bodies = Arc::new(SubresourceBodies::default());
        let key = LayoutBodyKey::new("F1#L1", "http://img.test/a.png", &SubresourceOptions::default(), SubresourceKind::Image);
        let pending = LayoutBodyKey::new("F1#L1", "http://img.test/b.png", &SubresourceOptions::default(), SubresourceKind::Image);
        assert!(bodies.start(&key));
        bodies.finish(key.clone(), LayoutBody::local("http://img.test/a.png", b"a".to_vec()));
        assert!(bodies.get(&key).is_some());
        assert!(bodies.start(&pending));
        bodies.forget_document("F1#L1");
        assert!(bodies.get(&key).is_none() && !bodies.pending(&pending));
        // A load of it that answers late writes nothing: `finish` needs a pending slot (cycle 4, 3d).
        bodies.finish(pending.clone(), LayoutBody::local("http://img.test/b.png", b"b".to_vec()));
        assert!(bodies.get(&pending).is_none());
        // Nor does an answer nobody asked for.
        let unasked = LayoutBodyKey::new("F2#L2", "http://img.test/c.png", &SubresourceOptions::default(), SubresourceKind::Image);
        bodies.finish(unasked.clone(), LayoutBody::local("http://img.test/c.png", b"c".to_vec()));
        assert!(bodies.get(&unasked).is_none());
    }


    /// A request's destination is where it was made, never its URL: a CSS image at `image.css`
    /// goes out as an image (an image's credentials) and its document's load waits for it; a
    /// style sheet at `sheet.png` goes out as a style sheet (security review cycle 3, B3).
    #[test]
    fn the_destination_is_the_requests_use_not_its_url() {
        let kinds = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let loader = Loader {
            document: "F#L",
            bodies: Arc::default(),
            log: Arc::default(),
            final_url: None,
            opaque: false,
            kinds: kinds.clone(),
            asked_images: asked.clone(),
        };
        let files = LocalFiles {
            statics: pyro_net::StaticFiles::default(),
            loader: Some(Arc::new(loader) as Arc<dyn SubresourceLoader>),
        };
        let image = pyro_net::Url::parse("http://img.test/image.css").unwrap();
        let sheet = pyro_net::Url::parse("http://img.test/sheet.png").unwrap();
        let font = pyro_net::Url::parse("http://img.test/font.css").unwrap();
        files.network(&image, &SubresourceOptions::default(), pyro_render::Destination::Image, None);
        files.network(&sheet, &SubresourceOptions::default(), pyro_render::Destination::Style, None);
        files.network(&font, &SubresourceOptions::default(), pyro_render::Destination::Font, None);
        let kinds: Vec<String> = kinds.lock().unwrap().iter().map(|k| format!("{k:?}")).collect();
        assert_eq!(kinds, ["Image", "Style", "Font"]);
        assert_eq!(*asked.lock().unwrap(), ["http://img.test/image.css"], "the load gate waits for the image");
    }

    /// A frame document this geometry renders makes its requests as itself: the request's client
    /// is the document it is sent for and kept under, never the geometry's own document (whose
    /// identity and bodies a child's request took: security review cycle 4, 1b).
    #[test]
    fn a_frame_documents_request_is_its_own() {
        let bodies = Arc::new(SubresourceBodies::default());
        let log = Arc::new(Mutex::new(Vec::new()));
        let top = files(&bodies, &log, "TOP#L", None, false);
        let options = pyro_render::FetchOptions { client: Some("CHILD#L".into()), ..Default::default() }
            .for_destination(pyro_render::Destination::Image);
        top.fetch_with("http://img.test/child.png".into(), &options, Box::new(|_| {}));
        assert_eq!(*log.lock().unwrap(), [("CHILD#L".to_owned(), "http://img.test/child.png".to_owned())]);
        let child = LayoutBodyKey::new("CHILD#L", "http://img.test/child.png", &SubresourceOptions::default(), SubresourceKind::Image);
        let own = LayoutBodyKey::new("TOP#L", "http://img.test/child.png", &SubresourceOptions::default(), SubresourceKind::Image);
        assert!(bodies.get(&child).is_some() && bodies.get(&own).is_none());
    }

    /// One URL used as an image and as a sheet is two requests, two keys (cycle 4, 3b).
    #[test]
    fn the_key_includes_the_destination() {
        let image = LayoutBodyKey::new("F#L", "http://x.test/a", &SubresourceOptions::default(), SubresourceKind::Image);
        let sheet = LayoutBodyKey::new("F#L", "http://x.test/a", &SubresourceOptions::default(), SubresourceKind::Style);
        let cors = LayoutBodyKey::new("F#L", "http://x.test/a", &SubresourceOptions::default(), SubresourceKind::CorsImage);
        assert_ne!(image, sheet);
        assert_eq!(image, cors, "an image's CORS mode is in its options");
    }
}
