//! Screenshots: CDP `Page.captureScreenshot` and the `Pyro.run` step
//! `matchScreenshot`, rendered with pyro-render (Blitz) off the browser thread;
//! screencast frames (`screencast.rs`) are rendered the same way.
//!
//! The browser thread builds a snapshot of the page (HTML with form state,
//! live sheet text, viewport, scroll; `PageDom::render_snapshot`) and hands
//! it to the connection's render thread, created on first use. Sub-resources
//! the renderer asks for (images, fonts) are fetched back on this thread
//! through the page's loader. A finished render is queued in [`Renders`],
//! which wakes the event loop; [`EngineHost::flush_renders`] then sends the
//! `captureScreenshot` reply or settles the waiting `matchScreenshot` step.
//!
//! Baselines are decoded once and cached per name on the connection, with
//! the display-list hash of the first render that matched one exactly: a
//! later render with that hash passes without raster or compare.
//!
//! The snapshot is the page as a keyed node list (`PageDom::render_snapshot_tree`).
//! The render thread keeps each page's Blitz document between renders and
//! applies only what changed (`pyro_render::Live`). The node list is built
//! again only when the page's render stamp moved (DOM or sheets changed);
//! otherwise the previous one is sent again, and the render thread skips the
//! diff. Nothing is recorded for a page before its first screenshot, and a
//! closed page's document is dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pyro_browser::{Browser, PageId, ResourceKind};
use pyro_dom::NodeId;
use pyro_render::worker::{Baseline, Captured, ImageFormat, Job, MatchOptions, MatchOutcome, RawFormat, RenderWorker};
use pyro_render::{Area, Backend, Fetcher, Live, Snapshot, Timings, Tree, TreeKind, TreeNode};
use pyro_runtime::LoopHandle;
use serde::Deserialize;
use serde_json::{json, Value};

use super::driver::{RenderSettlement, ShotStep};
use super::EngineHost;
use crate::msg::{self, CdpError, CdpResult};
use crate::network_protocol::base64_encode;
use crate::target::PageCommand;

pub(super) enum Finished {
    /// Layout sub-resources arrived or a parked PDF's deadline passed.
    Fetched,
    Capture(u64, Result<Captured, String>),
    Pdf(u64, Result<pyro_render::PdfOutput, String>),
    Match(u64, Result<MatchOutcome, String>),
    /// A screencast frame (`screencast.rs`); `None`: the last frame's pixels.
    Frame(u64, Result<Option<Captured>, String>),
    /// A raw screencast frame was written to its socket (`screencast.rs`), by screencast id.
    Written(u32, Result<(), String>),
}

/// The page as a screencast frame shows it: its viewport (CSS px) and scroll.
pub(super) struct FrameView {
    pub width: u32,
    pub height: u32,
    pub scroll_x: f64,
    pub scroll_y: f64,
}

struct CaptureWait {
    id: u64,
    session: String,
    started: Instant,
    snapshot_ms: f64,
    /// `printToPDF` with `transferMode: ReturnAsStream`: the bytes go to
    /// an `IO` stream.
    stream: bool,
}

/// A `printToPDF` waiting for its page's layout sub-resources (web fonts,
/// images) before it paints.
struct PdfWait {
    capture: CaptureWait,
    pid: PageId,
    opts: pyro_render::PdfOptions,
    deadline: Instant,
}

/// A `captureScreenshot` waiting for the lazy content it started (images, iframes).
struct ShotWait {
    id: u64,
    session: String,
    pid: PageId,
    area: Area,
    format: ImageFormat,
    out: Option<std::path::PathBuf>,
    started: Instant,
    deadline: Instant,
    /// The lazy content this capture started: what it waits for.
    lazy: pyro_browser::LazyStarted,
}

/// Longest a screenshot waits for a lazy iframe it started to navigate (a hung navigation must
/// not hold it for [`PDF_FETCH_WAIT`]); its lazy images may take that long.
const SHOT_FRAME_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Longest a `printToPDF` waits for layout sub-resources.
const PDF_FETCH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

struct MatchWait {
    name: String,
    settled: RenderSettlement,
    started: Instant,
    /// Snapshot built on the browser thread.
    snapshot_ms: f64,
}

/// The latest node list sent for a page, and the render stamp it was built at.
struct LiveTree {
    stamp: (u64, u64, u64),
    tree: Arc<Tree>,
    sheets: HashMap<String, String>,
}

/// Render state of one connection.
#[derive(Default)]
pub(crate) struct Renders {
    /// Pages that took a screenshot.
    live: HashMap<PageId, LiveTree>,
    worker: Option<RenderWorker>,
    finished: Arc<Mutex<Vec<Finished>>>,
    ready: Arc<AtomicBool>,
    posted: Arc<AtomicBool>,
    captures: HashMap<u64, CaptureWait>,
    matches: HashMap<u64, MatchWait>,
    baselines: HashMap<String, Arc<Baseline>>,
    next: u64,
    /// `IO` streams by handle: bytes and the read offset.
    streams: HashMap<String, (Vec<u8>, usize)>,
    pdf_waits: Vec<PdfWait>,
    /// Screenshots waiting for the lazy content they started.
    shot_waits: Vec<ShotWait>,
    /// The poll thread for waits on the page's own loads (lazy content) runs: one at a time.
    polling: Arc<AtomicBool>,
    /// Bumped by each poll thread start: an older thread still running exits.
    poll_generation: Arc<std::sync::atomic::AtomicU64>,
    /// pyro: how many times a PDF wait was parked ([`EngineHost::pdf_parks`]).
    pdf_parks: u64,
}

impl Renders {
    /// A render finished and waits for `flush_renders`.
    pub(super) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    fn token(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Callback the render thread calls with a result: queue it, wake the loop.
    pub(super) fn deliver(&self, handle: LoopHandle<Browser>) -> impl Fn(Finished) + Send + Clone + 'static {
        let finished = self.finished.clone();
        let ready = self.ready.clone();
        let posted = self.posted.clone();
        move |f| {
            if let Ok(mut q) = finished.lock() {
                q.push(f);
            }
            ready.store(true, Ordering::Release);
            if !posted.swap(true, Ordering::AcqRel) {
                let posted = posted.clone();
                let _ = handle.post(move |_| posted.store(false, Ordering::Release));
            }
        }
    }

    fn worker(&mut self) -> CdpResult<&RenderWorker> {
        if self.worker.is_none() {
            let w =
                RenderWorker::spawn().map_err(|e| CdpError::server(format!("cannot start the render thread: {e}")))?;
            self.worker = Some(w);
        }
        Ok(self.worker.as_ref().expect("worker"))
    }

    pub(super) fn clear(&mut self) {
        self.captures.clear();
        self.matches.clear();
        self.baselines.clear();
        for page in self.live.keys().copied().collect::<Vec<_>>() {
            self.forget(page);
        }
    }

    /// Drops what is kept for `page` (closed): its node list here, its
    /// document on the render thread.
    pub(super) fn forget(&mut self, page: PageId) {
        if self.live.remove(&page).is_some() {
            if let Some(w) = &self.worker {
                let _ = w.submit(Job::Forget { page: page.0 as u64 });
            }
        }
    }
}

/// Fetches what the renderer asks for through `page`'s loader, on the browser thread.
/// pyro: every request is a document's: the one the renderer names (a frame document's), else
/// the page's document of the snapshot (`client`); one whose document is no longer its frame's is
/// not sent.
struct PageFetcher {
    handle: LoopHandle<Browser>,
    page: PageId,
    /// The page's document when the snapshot was taken (`pyro_browser::document_client`).
    client: String,
    /// pyro: what the documents' own layouts fetched (`Browser::layout_bodies`), each body its
    /// document's alone (`LayoutBodyKey`).
    bodies: Option<Arc<pyro_dom::layout::SubresourceBodies>>,
    /// pyro: what each document of the page knew of its images when the snapshot was taken, by
    /// client (`Browser::document_images`): a request is asked about ITS document.
    documents: Arc<std::collections::HashMap<String, pyro_browser::DocumentImages>>,
}

impl PageFetcher {
    /// pyro: the document a renderer request is for (the one it names, else the page's).
    fn client_of<'a>(&'a self, options: &'a pyro_render::FetchOptions) -> &'a str {
        options.client.as_deref().unwrap_or(&self.client)
    }
}

/// pyro: the key of a renderer request (`options`) for the document `client` in the layout
/// bodies: that document's own request.
fn layout_key(url: &str, options: &pyro_render::FetchOptions, client: &str) -> pyro_dom::layout::LayoutBodyKey {
    let layout = pyro_dom::layout::SubresourceOptions {
        crossorigin: options.cors.map(|credentials| match credentials {
            pyro_render::CorsCredentials::SameOrigin => "anonymous".to_owned(),
            pyro_render::CorsCredentials::Include => "use-credentials".to_owned(),
        }),
        integrity: options.integrity.clone(),
        referrer: options.referrer.as_ref().map(|url| url.as_str().to_owned()),
        referrer_policy: options.referrer_policy.clone(),
    };
    // (Its destination as layout's loader keys it: the same request.)
    let kind = match options.destination {
        pyro_render::Destination::Font => pyro_dom::layout::SubresourceKind::Font,
        pyro_render::Destination::Style => pyro_dom::layout::SubresourceKind::Style,
        _ => pyro_dom::layout::SubresourceKind::Image,
    };
    pyro_dom::layout::LayoutBodyKey::new(client, url, &layout, kind)
}

impl Fetcher for PageFetcher {
    fn manages_image_bodies(&self) -> bool { true }
    /// pyro: the snapshot's page document, or a document of the page that was live when the
    /// snapshot was taken (its client in `documents`); any other gets nothing, cached or not.
    fn admits(&self, client: Option<&str>) -> bool {
        client.is_none_or(|client| client == self.client || self.documents.contains_key(client))
    }

    /// pyro: by the requesting document's timeline, which its layout keeps (`SubresourceBodies`).
    fn late(&self, url: &str, options: &pyro_render::FetchOptions, period: std::time::Duration) -> Option<bool> {
        let key = self.key(url, options);
        self.bodies.as_ref()?.late(&key, period)
    }

    /// pyro: an image the requesting document never fetched itself, asked for after its `load` (a
    /// style change since): Chrome paints it once it arrives, and a capture does not wait for it.
    /// One its layout fetched (or is fetching), or one in its list of available images, is waited
    /// for, as is one an element of it requested itself (an `<img>` whose load failed shows its alt
    /// text) and one no network answers (`about:`, `data:`, `blob:`, `file:`). All of it is that
    /// document's, a child frame's too, never the top document's or another document's.
    fn defers(&self, url: &str, options: &pyro_render::FetchOptions) -> bool {
        let client = self.client_of(options);
        let image = kind_of(options) == ResourceKind::Image;
        // pyro: an image the document is still fetching paints as loading now, as Chrome paints
        // what has arrived; its bytes reach the document at a later render (the capture waits for
        // that fetch, `serve_capture`, never sending it again).
        if image && self.bodies.as_ref().is_some_and(|bodies| bodies.pending(&self.key(url, options))) {
            return true;
        }
        let Some(document) = self.documents.get(client) else { return false };
        image
            && (url.starts_with("http://") || url.starts_with("https://"))
            && !document.requested.contains(url)
            && document.loaded
            && !document.available.contains(url)
            && self.bodies.as_ref().is_some_and(|bodies| !bodies.known(&self.key(url, options)))
    }

    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        self.request(url, &pyro_render::FetchOptions::default(), body_only(done))
    }

    fn fetch_cors(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let options = pyro_render::FetchOptions {
            cors: Some(pyro_render::CorsCredentials::SameOrigin),
            ..Default::default()
        };
        self.request(url, &options, body_only(done))
    }

    /// pyro: with the response's URL after redirects, against which an imported sheet resolves
    /// its URLs.
    fn fetch_final(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
        self.request(url, options, done)
    }

    fn fetch_with(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.request(url, options, body_only(done))
    }
}

/// `done` for the body alone.
fn body_only(
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) -> Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send> {
    Box::new(move |result| done(result.map(|fetched| Arc::unwrap_or_clone(fetched.body))))
}

impl PageFetcher {
    fn key(&self, url: &str, options: &pyro_render::FetchOptions) -> pyro_dom::layout::LayoutBodyKey {
        let key = layout_key(url, options, self.client_of(options));
        match &self.bodies { Some(bodies) => bodies.effective_key(key), None => key }
    }

    /// `url` fetched as `options` say, for the document they name (else the snapshot's page
    /// document); `done` gets the body of a 2xx response and its URL after redirects.
    fn request(
        &self,
        url: String,
        options: &pyro_render::FetchOptions,
        done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>,
    ) {
        let page = self.page;
        let source = options.clone();
        let kind = kind_of(options);
        let cors = options.cors.is_some();
        let mut options = subresource_options(options);
        options.client.get_or_insert_with(|| self.client.clone());
        // pyro: the same request's key in the requesting document's layout bodies (its own fetch).
        let key = self.key(&url, &source);
        options.referrer.clone_from(&key.options().referrer);
        options.referrer_policy = key.options().referrer_policy.as_deref().and_then(pyro_net::ReferrerPolicy::parse);
        // A closed loop drops `done`: the render stops waiting at its deadline.
        let request = CaptureRequest {
            page,
            url,
            kind,
            cors,
            options,
            client: key.document().to_owned(),
            key,
            until: Instant::now() + std::time::Duration::from_secs(10),
            waited: false,
        };
        let _ = self.handle.post(move |b| serve_capture(b, request, done));
    }
}

/// pyro: a capture's request, served on the browser thread ([`serve_capture`]).
struct CaptureRequest {
    page: PageId,
    url: String,
    kind: ResourceKind,
    cors: bool,
    options: pyro_browser::SubresourceOptions,
    client: String,
    key: pyro_dom::layout::LayoutBodyKey,
    /// How long it waits for the document's own fetch of it to end.
    until: Instant,
    /// It waited for the document's own fetch of it: that fetch's outcome is its answer.
    waited: bool,
}

/// pyro: serves a capture's request: the client is checked before anything, then the document's
/// own layout fetch of the same request answers it (its body, or its failure); while that fetch
/// is in flight the capture waits for it (it never sends the same request again beside it); only
/// a request the document never made goes out.
fn serve_capture(b: &mut Browser, request: CaptureRequest, done: Box<dyn FnOnce(Result<pyro_render::Fetched, String>) + Send>) {
    let CaptureRequest { page, url, kind, cors, options, client, key, until, waited } = request;
    // pyro: a document that is no longer its frame's has nothing served and nothing sent.
    if !b.is_live_client(page, &client) {
        done(Err("the document is no longer its frame's".into()));
        return;
    }
    let bodies = b.layout_bodies(page);
    // pyro: the document's own fetch of it is in flight: wait for its outcome (a short timer,
    // bounded), then serve that.
    if bodies.as_ref().is_some_and(|bodies| bodies.pending(&key)) && Instant::now() < until {
        let retry = std::rc::Rc::new(std::cell::RefCell::new(Some((
            CaptureRequest { page, url, kind, cors, options, client, key, until, waited: true },
            done,
        ))));
        b.scheduler().set_timer(std::time::Duration::from_millis(5), false, true, move |b| {
            if let Some((request, done)) = retry.borrow_mut().take() {
                serve_capture(b, request, done);
            }
        });
        return;
    }
    // pyro: what the document's own layout fetched (an image, a font, an imported sheet: the same
    // request, `layout_key`) is used as it is, with the final URL, tainting and policy stored
    // with it, as Chrome paints what the document loaded (settle stage B). One failure state: what
    // the document's layout failed to fetch is a failure here too.
    if bodies.as_ref().is_some_and(|bodies| bodies.has_failed(&key)) {
        done(Err("the document's own fetch of it failed".into()));
        return;
    }
    if let Some(body) = bodies.as_ref().and_then(|bodies| bodies.get(&key)) {
        // pyro: the stored tainting, as it is.
        let opaque = body.opaque;
        done(Ok(pyro_render::Fetched {
            body: body.bytes.clone(),
            url: body.final_url.clone(),
            opaque,
            referrer_policy: body.referrer_policy.clone(),
        }));
        return;
    }
    // pyro: the document's own fetch it waited for ended with no body kept: that is its answer
    // (never the same request again beside it).
    if waited {
        done(Err("the document's own fetch of it ended without a body".into()));
        return;
    }
    // An imported sheet without a charset of its own decodes as the document.
    let environment = b.document_encoding(page);
    b.fetch_subresource_options(
        page,
        url,
        kind,
        options,
        Box::new(move |_, result| {
            done(result.and_then(|r| {
                if (200..300).contains(&r.status) {
                    // (`r.opaque` is the final origin's tainting: `fetch_subresource_options`, `loader::is_opaque`.)
                    let (url, opaque) = (r.url.clone(), r.opaque);
                    let referrer_policy = r.referrer_policy.map(|p| p.as_str().to_owned());
                    let body = if kind == ResourceKind::Image { r.body } else { r.render_body(environment.as_slice()).into() };
                    Ok(pyro_render::Fetched { body, url, opaque, referrer_policy })
                } else {
                    Err(format!("HTTP {}", r.status))
                }
            }))
        }),
    );
}

/// pyro: a renderer's request options (a CSS `url()`'s request URL modifiers) as the browser
/// takes them.
pub(super) fn subresource_options(options: &pyro_render::FetchOptions) -> pyro_browser::SubresourceOptions {
    pyro_browser::SubresourceOptions {
        crossorigin: options.cors.map(|credentials| match credentials {
            pyro_render::CorsCredentials::SameOrigin => "anonymous".to_owned(),
            pyro_render::CorsCredentials::Include => "use-credentials".to_owned(),
        }),
        integrity: options.integrity.clone(),
        referrer: options.referrer.as_ref().map(|url| url.as_str().to_owned()),
        referrer_policy: options.referrer_policy.as_deref().and_then(pyro_net::ReferrerPolicy::parse),
        client: options.client.clone(),
    }
}

/// pyro: what a renderer request's response is used as: its destination, from where it was made
/// (never its URL: an image at `image.css` is an image, with an image's credentials and load
/// gate). One that says none is an image (a CSS image or reference).
fn kind_of(options: &pyro_render::FetchOptions) -> ResourceKind {
    match options.destination {
        pyro_render::Destination::Font => ResourceKind::Font,
        pyro_render::Destination::Style => ResourceKind::Stylesheet,
        _ => ResourceKind::Image,
    }
}

/// pyro-dom's render tree as pyro-render takes it (the same `QualName`s).
pub(super) fn convert(t: pyro_dom::snapshot::RenderTree) -> Tree {
    use pyro_dom::snapshot::RenderNodeKind;
    Tree {
        nodes: t
            .nodes
            .into_iter()
            .map(|n| TreeNode {
                key: n.key,
                parent: n.parent,
                kind: match n.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(t) => TreeKind::Text(t),
                },
            })
            .collect(),
    }
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn timings_json(t: &Timings, snapshot_ms: f64, total_ms: f64) -> Value {
    json!({
        "snapshot": snapshot_ms,
        "decode": ms(t.decode),
        "fetch": ms(t.fetch),
        "layout": ms(t.layout),
        "scene": ms(t.scene),
        "raster": ms(t.raster),
        "compare": ms(t.compare),
        "encode": ms(t.encode),
        "write": ms(t.write),
        // CPU time of the browser process while it rendered (all threads).
        "cpu": ms(t.cpu),
        "total": total_ms,
        // How the page's persistent document was used: `rebuilt` names why
        // it was built from scratch (null: updated in place).
        "mirror": t.mirror.map(|m| json!({
            "rebuilt": m.rebuilt.map(|r| format!("{r:?}").to_lowercase()),
            "mutations": m.mutations,
            "pages": m.pages,
        })),
    })
}

/// `Page.captureScreenshot` parameters.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CaptureParams {
    format: Option<String>,
    quality: Option<u8>,
    clip: Option<ClipParams>,
    #[serde(default)]
    capture_beyond_viewport: bool,
    /// pyro: append the image to this file (or FIFO) instead of replying with it.
    path: Option<String>,
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClipParams {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: Option<f64>,
}

impl ClipParams {
    pub(super) fn area(self, beyond_viewport: bool) -> CdpResult<Area> {
        if !(self.width > 0.0 && self.height > 0.0) {
            return Err(CdpError::invalid_params("clip width and height must be positive"));
        }
        Ok(Area::Clip {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            scale: self.scale.unwrap_or(1.0),
            beyond_viewport,
        })
    }
}

impl EngineHost {
    fn snapshot(&mut self, pid: PageId, mark: Option<NodeId>) -> CdpResult<Snapshot> {
        let page = self.thread.browser.page(pid).ok_or_else(|| CdpError::server("page is gone"))?;
        // Responsive iframes take their documents' heights (css-sizing-4).
        page.update_responsive_frames();
        // Frames show their documents (the frame table), each with its own sheets.
        let mut transition_frames = page.update_frame_documents();
        let frame = &page.main_frame;
        let dom = frame.page_dom.clone().ok_or_else(|| CdpError::server("page has no document"))?;
        let stamp = dom.render_stamp();
        let base_url = dom.render_base_url(&frame.base_url);
        let (s, live) = match self.renders.live.get(&pid) {
            // Unchanged: the same `Arc`, which the render thread does not diff.
            Some(l) if l.stamp == stamp => {
                let mut s = dom.render_view();
                s.sheets = l.sheets.clone();
                (s, l.tree.clone())
            }
            _ => {
                let mut s = dom.render_snapshot_tree(&base_url, None);
                let tree = Arc::new(convert(s.tree.take().unwrap_or_default()));
                let sheets = s.sheets.clone();
                self.renders.live.insert(pid, LiveTree { stamp, tree: tree.clone(), sheets });
                (s, tree)
            }
        };
        let content_visibility = {
            let d = dom.dom.borrow();
            let l = dom.layout();
            l.content_visibility_state(&d)
        };
        // pyro: the frames' documents' animations, sampled into them by the renderer.
        let frame_animations: Vec<(String, Arc<pyro_render::AnimationFrame>)> = dom
            .dom
            .borrow()
            .frame_animations
            .iter()
            .map(|(client, frame)| (client.clone(), Arc::new(pyro_layout::animation_frame(frame))))
            .collect();
        // pyro: the frame table its frame elements name.
        let table = dom.dom.borrow().frame_table(dom.document());
        let frames = pyro_layout::frame_table(table);
        dom.transition_frames(&frames.nonce, &mut transition_frames);
        dom.reserve_transition_job(&frames.nonce, &mut transition_frames);
        let raster_budget = dom.dom.borrow().document_data(dom.document())
            .map_err(|_| CdpError::server("page has no document"))?.raster_budget.clone();
        Ok(Snapshot {
            raster_budget,
            transition_frames,
            html: String::new(),
            frames,
            document_url: (frame.url != base_url).then(|| frame.url.clone()),
            base_url,
            sheets: s.sheets,
            width: s.width,
            height: s.height,
            device_scale_factor: s.device_scale_factor,
            dark: s.dark,
            print: s.print,
            media_environment: pyro_render::media_environment(&page.viewport()),
            input: pyro_render::InputCapabilities {
                pointer: match page.viewport().pointer {
                    pyro_dom::media::Pointer::None => pyro_render::Pointer::None,
                    pyro_dom::media::Pointer::Fine => pyro_render::Pointer::Fine,
                    pyro_dom::media::Pointer::Coarse => pyro_render::Pointer::Coarse,
                },
                hover: page.viewport().hover,
            },
            scroll_x: s.scroll_x,
            scroll_y: s.scroll_y,
            resource_scope: format!("{}:{}", frame.loader_id.0, self.thread.browser.layout_bodies(pid).map_or(0, |bodies| bodies.policy_revision())),
            live: Some(Live {
                page: pid.0 as u64,
                tree: live,
                clip: mark.map(pyro_dom::snapshot::render_key),
                interaction: Some((s.hovered, s.focused)),
            }),
            background: self.pages.values().find(|p| p.id == pid).and_then(|p| p.background_override),
            canvases: s
                .canvases
                .into_iter()
                .map(|(node, width, height, rgba)| pyro_render::CanvasImage {
                    key: pyro_dom::snapshot::render_key(node),
                    width,
                    height,
                    rgba,
                    tainting: s.tainting_canvases.contains(&node),
                })
                .collect(),
            element_scroll: Some(s.element_scroll),
            sticky_offsets: Some(s.sticky_offsets),
            scroll_targets: Some(s.scroll_targets),
            popover_anchors: s.popover_anchors,
            // CSS animations, transitions and Web Animations as they are now.
            animations: Some(Arc::new(pyro_layout::animation_frame(&dom.animation_frame()))),
            // pyro: what `content-visibility: auto` skips as the page's layout engine determined it.
            content_visibility: Some(content_visibility),
            // pyro: the frames' documents' animations, sampled into them by the renderer.
            frame_animations,
            xml_document: s.xml_document,
            quirks_mode: s.quirks_mode,
            limited_quirks_mode: s.limited_quirks_mode,
            content_language: s.content_language,
            registered_properties: s.registered_properties,
            // The document selection, highlighted as Chromium paints it.
            selection: s.selection.map(|(start, end)| pyro_render::TextSelection { start, end }),
            // Custom highlights and the target text, under the selection.
            highlights: s
                .highlights
                .into_iter()
                .map(|(name, ranges)| pyro_render::TextHighlight {
                    name,
                    ranges: ranges.into_iter().map(|(start, end)| pyro_render::TextSelection { start, end }).collect(),
                })
                .collect(),
            control_selection: s.control_selection,
            selected_replaced: s.selected_replaced,
            caret: s.caret,
            url_target: s.url_target,
        })
    }

    /// What a viewport capture shows: the viewport, or under mobile
    /// emulation the visual viewport at its page scale, in the device's size
    /// (a page without `<meta name="viewport">` shows its whole 980 px
    /// layout shrunk to the phone's width), as Chrome's `captureScreenshot`
    /// without a clip.
    fn viewport_area(&self, pid: PageId) -> Area {
        let Some(dom) = self.thread.browser.page(pid).and_then(|p| p.main_frame.page_dom.clone()) else {
            return Area::Viewport;
        };
        dom.sync_mobile_layout();
        let layout = dom.layout();
        match layout.mobile_metrics() {
            Some(m) => Area::Clip {
                x: layout.scroll_x() as f64,
                y: layout.scroll_y() as f64,
                width: m.visual.0,
                height: m.visual.1,
                scale: m.scale,
                beyond_viewport: false,
            },
            None => Area::Viewport,
        }
    }

    fn fetcher(&self, pid: PageId) -> Arc<dyn Fetcher> {
        let browser = &self.thread.browser;
        let page = browser.page(pid);
        let client = page.map(|p| pyro_browser::document_client(&p.main_frame)).unwrap_or_default();
        let bodies = browser.layout_bodies(pid);
        let documents = Arc::new(browser.document_images(pid));
        Arc::new(PageFetcher { handle: self.thread.handle(), page: pid, client, bodies, documents })
    }

    /// `Page.captureScreenshot`: replied to from `flush_renders`.
    pub(super) fn capture_screenshot(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let p = cmd.command.params::<Option<CaptureParams>>()?.unwrap_or_default();
        let format = match p.format.as_deref() {
            None | Some("png") => ImageFormat::Png,
            Some("jpeg") => ImageFormat::Jpeg(p.quality.unwrap_or(80)),
            // pyro's raw formats (no encoding), by ffmpeg's pixel format names.
            Some("rgba") => ImageFormat::Raw(RawFormat::Rgba),
            Some("rgb24") => ImageFormat::Raw(RawFormat::Rgb24),
            Some("yuv420p") => ImageFormat::Raw(RawFormat::Yuv420p),
            Some(other) => {
                return Err(CdpError::invalid_params(format!(
                    "format '{other}' is not supported (png, jpeg, and pyro's rgba, rgb24, yuv420p)"
                )))
            }
        };
        let out = match p.path {
            Some(path) if !std::path::Path::new(&path).is_absolute() => {
                return Err(CdpError::invalid_params(format!("path '{path}' is not absolute")))
            }
            path => path.map(std::path::PathBuf::from),
        };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let area = match (p.clip, p.capture_beyond_viewport) {
            (Some(clip), beyond_viewport) => clip.area(beyond_viewport)?,
            (None, true) => Area::FullPage,
            (None, false) => self.viewport_area(pid),
        };
        // Lazy images and iframes the lazy load check has not let go yet: all of them for a
        // full-page capture (a clip beyond the viewport too, as a full-page tool sends one),
        // those near the viewport otherwise. The capture waits for them, as a print does.
        let whole = matches!(area, Area::FullPage | Area::Clip { beyond_viewport: true, .. });
        let started = Instant::now();
        let (id, session) = (cmd.command.id, cmd.session_id.to_owned());
        let lazy = self.thread.browser.approve_lazy_content(pid, whole);
        if !lazy.is_empty() && self.thread.browser.lazy_loading(&lazy, true) {
            let deadline = started + PDF_FETCH_WAIT;
            self.renders.shot_waits.push(ShotWait { id, session, pid, area, format, out, started, deadline, lazy });
            self.poll_waits();
            return Ok(None);
        }
        self.start_capture(id, session, pid, area, format, out, started).map(|()| None)
    }

    /// Checks the waiting captures and prints again every 50 ms (the page's own loads end
    /// without a waker): one poll thread, which stops once nothing waits on such loads.
    fn poll_waits(&self) {
        if self.renders.polling.swap(true, Ordering::AcqRel) {
            return;
        }
        // A thread stopped and a new one started before it saw the stop: the old one exits on
        // the generation change, so only one ever polls.
        let generation = self.renders.poll_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let poll = self.renders.deliver(self.thread.handle());
        let (running, current) = (self.renders.polling.clone(), self.renders.poll_generation.clone());
        let _ = std::thread::Builder::new().name("pyro-capture-poll".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !running.load(Ordering::Acquire) || current.load(Ordering::Acquire) != generation {
                break;
            }
            poll(Finished::Fetched);
        });
    }

    /// Stops the poll thread when no capture or print waits on the page's own loads.
    fn stop_polling_if_idle(&self) {
        let waiting = !self.renders.shot_waits.is_empty()
            || self.renders.pdf_waits.iter().any(|w| self.thread.browser.content_loading(w.pid));
        if !waiting {
            self.renders.polling.store(false, Ordering::Release);
        }
    }

    /// Captures the waiting screenshots whose lazy content is in or whose deadline passed.
    fn resume_shots(&mut self) {
        let now = Instant::now();
        let waits = std::mem::take(&mut self.renders.shot_waits);
        for w in waits {
            // Its own lazy images until the deadline; its lazy iframes' navigations for a shorter
            // while (a hung one must not hold it).
            let frames = now < w.started + SHOT_FRAME_WAIT;
            if now < w.deadline && self.thread.browser.lazy_loading(&w.lazy, frames) {
                self.renders.shot_waits.push(w);
                continue;
            }
            let (id, session) = (w.id, w.session.clone());
            let ShotWait { id: wid, session: wsession, pid, area, format, out, started, .. } = w;
            if let Err(e) = self.start_capture(wid, wsession, pid, area, format, out, started) {
                self.out.push(msg::error_frame(Some(id), &e, Some(&session)));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_capture(
        &mut self,
        id: u64,
        session: String,
        pid: PageId,
        area: Area,
        format: ImageFormat,
        out: Option<std::path::PathBuf>,
        started: Instant,
    ) -> CdpResult<()> {
        let snapshot = self.snapshot(pid, None)?;
        let snapshot_ms = ms(started.elapsed());
        let fetcher = self.fetcher(pid);
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let job = Job::Capture {
            snapshot,
            area,
            format,
            backend: Backend::Cpu,
            fetcher,
            out,
            done: Box::new(move |r| deliver(Finished::Capture(token, r))),
        };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        let wait = CaptureWait { id, session, started, snapshot_ms, stream: false };
        self.renders.captures.insert(token, wait);
        Ok(())
    }

    /// Starts a screencast frame of `pid` (`screencast.rs`): the viewport,
    /// scaled down in the display list to fit `max` (device px), and not
    /// rasterized when its display list hashes as `unless`. The result goes
    /// to `screencast_rendered`, by the returned token.
    pub(super) fn render_frame(
        &mut self,
        pid: PageId,
        max: (Option<u32>, Option<u32>),
        format: ImageFormat,
        unless: Option<u64>,
    ) -> CdpResult<(u64, FrameView)> {
        let snapshot = self.snapshot(pid, None)?;
        let view = FrameView {
            width: snapshot.width,
            height: snapshot.height,
            scroll_x: snapshot.scroll_x,
            scroll_y: snapshot.scroll_y,
        };
        // As `Renderer::paint` sizes the viewport.
        let (width, height) = (f64::from(snapshot.width.max(1)), f64::from(snapshot.height.max(1)));
        let dpr = snapshot.device_scale_factor.clamp(0.1, 16.0);
        let scale = [max.0.map(|m| f64::from(m) / (width * dpr)), max.1.map(|m| f64::from(m) / (height * dpr))]
            .into_iter()
            .flatten()
            .fold(1.0, f64::min);
        let area = if scale < 1.0 {
            Area::Clip { x: snapshot.scroll_x, y: snapshot.scroll_y, width, height, scale, beyond_viewport: false }
        } else {
            Area::Viewport
        };
        let fetcher = self.fetcher(pid);
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let done = Box::new(move |r| deliver(Finished::Frame(token, r)));
        let job = Job::Frame { snapshot, area, format, unless, fetcher, done };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        Ok((token, view))
    }

    /// `Page.printToPDF`: one PDF page per Paprize page box (`.pz-page`),
    /// else the document cut into the paper's content box (margins, scale);
    /// replied to from `flush_renders`. Header/footer templates, tagged PDF
    /// and the document outline are accepted and not produced.
    pub(super) fn print_to_pdf(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            landscape: Option<bool>,
            paper_width: Option<f64>,
            paper_height: Option<f64>,
            margin_top: Option<f64>,
            margin_bottom: Option<f64>,
            margin_left: Option<f64>,
            margin_right: Option<f64>,
            scale: Option<f64>,
            #[serde(default)]
            print_background: bool,
            page_ranges: Option<String>,
            transfer_mode: Option<String>,
            // Accepted, not produced.
            prefer_css_page_size: Option<bool>,
            #[allow(dead_code)]
            display_header_footer: Option<bool>,
            #[allow(dead_code)]
            header_template: Option<String>,
            #[allow(dead_code)]
            footer_template: Option<String>,
            #[serde(rename = "generateTaggedPDF")]
            generate_tagged_pdf: Option<bool>,
            #[allow(dead_code)]
            generate_document_outline: Option<bool>,
        }
        let p = cmd.command.params::<Option<P>>()?.unwrap_or_default();
        let (mut w, mut h) = (p.paper_width.unwrap_or(8.5), p.paper_height.unwrap_or(11.0));
        if !(w > 0.0 && w.is_finite()) {
            return Err(CdpError::invalid_params("paperWidth is zero or negative"));
        }
        if !(h > 0.0 && h.is_finite()) {
            return Err(CdpError::invalid_params("paperHeight is zero or negative"));
        }
        let scale = p.scale.unwrap_or(1.0);
        if !(0.1..=2.0).contains(&scale) {
            return Err(CdpError::invalid_params("scale is outside [0.1 - 2] range"));
        }
        if p.landscape == Some(true) {
            std::mem::swap(&mut w, &mut h);
        }
        // Chrome's default margin is about 1 cm (0.4 in).
        let mut margins = [0.0; 4];
        for (i, (name, v)) in [
            ("marginTop", p.margin_top),
            ("marginRight", p.margin_right),
            ("marginBottom", p.margin_bottom),
            ("marginLeft", p.margin_left),
        ]
        .into_iter()
        .enumerate()
        {
            let v = v.unwrap_or(0.4);
            if !(v >= 0.0 && v.is_finite()) {
                return Err(CdpError::invalid_params(format!("{name} is negative")));
            }
            margins[i] = v;
        }
        let page_area = pyro_render::chrome_page_area((w, h), margins);
        let Some((paper, margins)) = pyro_render::chrome_page_geometry((w, h), margins) else {
            return Err(CdpError::invalid_params("invalid print parameters: content area is empty"));
        };
        let page_ranges = match p.page_ranges.as_deref().map(str::trim) {
            None | Some("") => Vec::new(),
            Some(ranges) => {
                parse_page_ranges(ranges).ok_or_else(|| CdpError::invalid_params("Page range syntax error"))?
            }
        };
        let opts = pyro_render::PdfOptions {
            page_selector: ".pz-page".into(),
            paper,
            margins,
            page_area: Some(page_area),
            scale,
            print_background: p.print_background,
            page_ranges,
            // `preferCSSPageSize` writes pages of the box's own size (close to Chrome's @page size).
            place_on_paper: p.prefer_css_page_size != Some(true),
            print_media: self.pages.get(&cmd.page).and_then(|state| state.emulated_media.as_ref()).is_none_or(|raw| {
                !matches!(raw.get("media").and_then(|m| m.as_str()).map(str::trim), Some("screen"))
            }),
            // Chrome's default: tagged.
            tagged: p.generate_tagged_pdf.unwrap_or(true),
        };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let started = Instant::now();
        let stream = p.transfer_mode.as_deref() == Some("ReturnAsStream");
        // Blitz geometry: print the document layout queries were answered
        // from (one Blitz document per page document), once the fonts and
        // images its layout started fetching are in. The browser thread
        // keeps running meanwhile.
        // Chrome loads lazy content to print: all of it starts now, and the print waits for it.
        self.thread.browser.approve_lazy_content(pid, true);
        let page_dom = self.thread.browser.page(pid).and_then(|p| p.main_frame.page_dom.clone());
        if page_dom.is_some_and(|d| super::blitz_geometry::sync_for_pdf(&d).is_some()) {
            let capture = CaptureWait {
                id: cmd.command.id,
                session: cmd.session_id.to_owned(),
                started,
                snapshot_ms: 0.0,
                stream,
            };
            let wait = PdfWait { capture, pid, opts, deadline: started + PDF_FETCH_WAIT };
            if self.pdf_fetching(pid) {
                self.park_pdf(wait);
                return Ok(None);
            }
            return self.paint_pdf(wait).map(|()| None);
        }
        let snapshot = self.snapshot(pid, None)?;
        let snapshot_ms = ms(started.elapsed());
        let fetcher = self.fetcher(pid);
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let job = Job::Pdf { snapshot, opts, fetcher, done: Box::new(move |r| deliver(Finished::Pdf(token, r))) };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        let wait = CaptureWait { id: cmd.command.id, session: cmd.session_id.to_owned(), started, snapshot_ms, stream };
        self.renders.captures.insert(token, wait);
        Ok(None)
    }

    /// Waits for the layout fetches in flight (woken by the last one, or
    /// at the deadline) before printing.
    /// pyro: woken only once what the PDF waits for may have finished: a waker is registered first,
    /// then fired at once only when nothing the PDF waits for is pending ([`Self::pdf_fetching`]);
    /// otherwise the end of any layout fetch wakes it (`InFlight`'s drop wakes every PDF). It no
    /// longer parked on the idle top document alone, which woke it at once while only a frame's
    /// fetch was pending, again and again until the deadline.
    fn park_pdf(&mut self, wait: PdfWait) {
        self.renders.pdf_parks += 1;
        let deliver = self.renders.deliver(self.thread.handle());
        let at_deadline = deliver.clone();
        let deadline = wait.deadline;
        let first = self.renders.pdf_waits.is_empty();
        let document = self.pdf_document(wait.pid);
        let pid = wait.pid;
        self.renders.pdf_waits.push(wait);
        super::blitz_geometry::park_until_fetched(&document, Box::new(move || deliver(Finished::Fetched)));
        if !self.pdf_fetching(pid) {
            super::blitz_geometry::wake_parked(&document);
        } else if self.thread.browser.content_loading(pid) {
            // The page's own loads (lazy content the print started) end without waking the
            // geometry: the poll thread checks again.
            self.poll_waits();
        }
        if first {
            // The deadline of the first parked wait; later ones get theirs
            // from a waker that fires before, or this one.
            let _ = std::thread::Builder::new().name("pyro-pdf-deadline".into()).spawn(move || {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                at_deadline(Finished::Fetched);
            });
        }
    }

    /// Prints the parked PDFs whose fetches are done or whose deadline
    /// passed; the rest keep waiting.
    fn resume_pdfs(&mut self) {
        let now = Instant::now();
        let waits = std::mem::take(&mut self.renders.pdf_waits);
        let mut parked = Vec::new();
        for wait in waits {
            if self.pdf_fetching(wait.pid) && now < wait.deadline {
                parked.push(wait);
                continue;
            }
            let (id, session) = (wait.capture.id, wait.capture.session.clone());
            if let Err(e) = self.paint_pdf(wait) {
                self.out.push(msg::error_frame(Some(id), &e, Some(&session)));
            }
        }
        for wait in parked {
            self.park_pdf(wait);
        }
    }

    /// pyro: whether a PDF of page `pid` still waits: its geometry's fetches in flight, or any
    /// fetch of the page's documents (a frame's too) pending in their layout bodies, whichever
    /// geometry made it (a frame's own one may have, and the page's then only waits on it).
    fn pdf_fetching(&self, pid: PageId) -> bool {
        if super::blitz_geometry::fetching(&self.pdf_document(pid)) || self.thread.browser.content_loading(pid) {
            return true;
        }
        let browser = &self.thread.browser;
        let Some(bodies) = browser.layout_bodies(pid) else { return false };
        browser.document_images(pid).keys().any(|client| bodies.any_pending(client, |_| true))
    }

    /// pyro: how many times a PDF wait was parked on this connection (a wait parks again only after
    /// a fetch ended).
    pub fn pdf_parks(&self) -> u64 {
        self.renders.pdf_parks
    }

    /// pyro: the document a PDF of page `pid` waits for the layout fetches of (its client).
    fn pdf_document(&self, pid: PageId) -> String {
        self.thread.browser.page(pid).map(|p| pyro_browser::document_client(&p.main_frame)).unwrap_or_default()
    }

    /// Paints `wait`'s page here and has the render thread write the PDF.
    fn paint_pdf(&mut self, wait: PdfWait) -> CdpResult<()> {
        let page_dom = self.thread.browser.page(wait.pid).and_then(|p| p.main_frame.page_dom.clone());
        let paint = page_dom
            .and_then(|d| super::blitz_geometry::paint_pdf(&d, &wait.opts))
            .ok_or_else(|| CdpError::server("page is gone"))?
            .map_err(CdpError::server)?;
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let job = Job::PdfWrite { paint: Box::new(paint), done: Box::new(move |r| deliver(Finished::Pdf(token, r))) };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        self.renders.captures.insert(token, wait.capture);
        Ok(())
    }

    /// The `printToPDF` reply: the PDF (inline, or as an `IO` stream) and
    /// pyro's report (`pyro`: pages, overflow, mapping stats, timings).
    fn pdf_body(&mut self, o: pyro_render::PdfOutput, stream: bool, token: u64, snapshot_ms: f64, total: f64) -> Value {
        let report = json!({
            "pages": o.pages.iter().map(|p| json!({
                "x": p.rect.x0, "y": p.rect.y0, "width": p.rect.width(), "height": p.rect.height(),
                "overflowRight": p.overflow_right, "overflowBottom": p.overflow_bottom, "culprit": p.culprit,
            })).collect::<Vec<_>>(),
            "bySelector": o.by_selector,
            "commands": o.commands,
            "stats": o.stats,
            "ms": {
                "snapshot": snapshot_ms, "fetch": ms(o.timings.fetch), "layout": ms(o.timings.layout),
                "paint": ms(o.timings.scene), "pdf": ms(o.timings.raster), "total": total,
            },
        });
        if stream {
            let handle = format!("pyro-pdf-{token}");
            self.renders.streams.insert(handle.clone(), (o.bytes, 0));
            json!({ "data": "", "stream": handle, "pyro": report })
        } else {
            json!({ "data": base64_encode(&o.bytes), "pyro": report })
        }
    }

    /// `IO.read` / `IO.close` on the streams `printToPDF` made.
    pub(super) fn io_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize)]
        struct P {
            handle: String,
            offset: Option<usize>,
            size: Option<usize>,
        }
        let p: P = cmd.command.params()?;
        match action {
            "read" => {
                let (bytes, pos) = self
                    .renders
                    .streams
                    .get_mut(&p.handle)
                    .ok_or_else(|| CdpError::invalid_params("Invalid stream handle"))?;
                if let Some(o) = p.offset {
                    *pos = o.min(bytes.len());
                }
                let end = (*pos + p.size.unwrap_or(1 << 20)).min(bytes.len());
                let data = base64_encode(&bytes[*pos..end]);
                *pos = end;
                Ok(Some(json!({ "base64Encoded": true, "data": data, "eof": end >= bytes.len() })))
            }
            "close" => {
                self.renders.streams.remove(&p.handle);
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Starts a `matchScreenshot` step on `pid`'s current document; `element`
    /// is the element to capture (the step's locator).
    pub(super) fn start_shot(
        &mut self,
        pid: PageId,
        spec: &ShotStep,
        element: Option<NodeId>,
    ) -> CdpResult<RenderSettlement> {
        let started = Instant::now();
        if spec.capture_only {
            let page = self.thread.browser.page(pid).ok_or_else(|| CdpError::server("page is gone"))?;
            let frame = &page.main_frame;
            let dom = frame.page_dom.as_ref().ok_or_else(|| CdpError::server("page has no document"))?;
            let base_url = dom.render_base_url(&frame.base_url);
            let s = dom.render_snapshot(&base_url, element.map(|id| (id, pyro_render::CLIP_ATTRIBUTE)));
            let value = json!({
                "captured": true, "snapshotMs": ms(started.elapsed()),
                "snapshot": { "html": s.html, "baseUrl": base_url, "documentUrl": frame.url, "sheets": s.sheets,
                    "width": s.width, "height": s.height, "deviceScaleFactor": s.device_scale_factor,
                    "dark": s.dark, "scrollX": s.scroll_x, "scrollY": s.scroll_y,
                    "xmlDocument": s.xml_document, "quirksMode": s.quirks_mode,
                    "limitedQuirksMode": s.limited_quirks_mode, "contentLanguage": s.content_language,
                    "nonce": s.nonce }
            });
            return Ok(std::rc::Rc::new(std::cell::RefCell::new(Some(Ok(value)))));
        }
        let area = match (spec.clip, spec.full_page, element) {
            (Some((x, y, width, height)), _, _) => {
                Area::Clip { x, y, width, height, scale: 1.0, beyond_viewport: false }
            }
            (None, true, _) => Area::FullPage,
            (None, false, Some(_)) => Area::Element,
            (None, false, None) => self.viewport_area(pid),
        };
        let opts = MatchOptions {
            tolerance: (spec.threshold * 255.0).round() as u8,
            max_diff_pixels: spec.max_diff_pixels,
            max_diff_ratio: spec.max_diff_pixel_ratio,
            backend: Backend::Cpu,
        };
        let baseline = if spec.update {
            self.renders.baselines.remove(&spec.name);
            None
        } else {
            match &spec.baseline {
                Some(png) => {
                    let digest = pyro_render::digest(png);
                    let cached = self.renders.baselines.get(&spec.name).filter(|b| b.digest == digest).cloned();
                    let b = cached.unwrap_or_else(|| {
                        // Decoded on the render thread when first compared.
                        let b = Arc::new(Baseline::from_png(png.clone(), spec.scene_hash));
                        self.renders.baselines.insert(spec.name.clone(), b.clone());
                        b
                    });
                    Some(b)
                }
                None => self.renders.baselines.get(&spec.name).cloned(),
            }
        };
        let t = Instant::now();
        let mut snapshot = self.snapshot(pid, element)?;
        if spec.settle_animations {
            // A screenshot assertion compares stable pixels: no animation mid-flight.
            let dom = self.thread.browser.page(pid).and_then(|p| p.main_frame.page_dom.clone());
            if let Some(dom) = dom {
                snapshot.animations = Some(Arc::new(pyro_layout::animation_frame(&dom.settled_animation_frame())));
            }
        }
        let snapshot_ms = ms(t.elapsed());
        let fetcher = self.fetcher(pid);
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let job = Job::Match {
            snapshot,
            area,
            baseline,
            opts,
            fetcher,
            done: Box::new(move |r| deliver(Finished::Match(token, r))),
        };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        let settled: RenderSettlement = std::rc::Rc::default();
        let wait = MatchWait { name: spec.name.clone(), settled: settled.clone(), started, snapshot_ms };
        self.renders.matches.insert(token, wait);
        Ok(settled)
    }

    /// Sends finished captures and settles finished matches.
    pub(super) fn flush_renders(&mut self) {
        if !self.renders.ready.swap(false, Ordering::AcqRel) {
            return;
        }
        let finished = std::mem::take(&mut *self.renders.finished.lock().unwrap_or_else(|e| e.into_inner()));
        let mut fetched = false;
        for f in finished {
            match f {
                Finished::Fetched => fetched = true,
                Finished::Capture(token, result) => {
                    let Some(wait) = self.renders.captures.remove(&token) else { continue };
                    let frame = match result {
                        Ok(c) => {
                            let total = ms(wait.started.elapsed());
                            // `bytes`: the image's size, also when it went to the capture's file.
                            let body = json!({
                                "data": base64_encode(&c.bytes),
                                "pyro": { "width": c.width, "height": c.height, "bytes": c.size, "sceneHash": format!("{:016x}", c.scene_hash), "ms": timings_json(&c.timings, wait.snapshot_ms, total) },
                            });
                            msg::result_frame(wait.id, &body, Some(&wait.session))
                        }
                        Err(e) => msg::error_frame(Some(wait.id), &CdpError::server(e), Some(&wait.session)),
                    };
                    self.out.push(frame);
                }
                Finished::Pdf(token, result) => {
                    let Some(wait) = self.renders.captures.remove(&token) else { continue };
                    let frame = match result {
                        Ok(o) => {
                            let total = ms(wait.started.elapsed());
                            let body = self.pdf_body(o, wait.stream, token, wait.snapshot_ms, total);
                            msg::result_frame(wait.id, &body, Some(&wait.session))
                        }
                        Err(e) => msg::error_frame(Some(wait.id), &CdpError::server(e), Some(&wait.session)),
                    };
                    self.out.push(frame);
                }
                Finished::Match(token, result) => {
                    let Some(wait) = self.renders.matches.remove(&token) else { continue };
                    let total = ms(wait.started.elapsed());
                    let outcome =
                        result.map_err(CdpError::server).and_then(|o| match_result(&wait.name, o, &wait, total));
                    *wait.settled.borrow_mut() = Some(outcome);
                }
                Finished::Frame(token, result) => self.screencast_rendered(token, result),
                Finished::Written(id, result) => self.screencast_written(id, result),
            }
        }
        if fetched && !self.renders.pdf_waits.is_empty() {
            self.resume_pdfs();
        }
        if fetched && !self.renders.shot_waits.is_empty() {
            self.resume_shots();
        }
        if fetched {
            self.stop_polling_if_idle();
        }
    }
}

fn match_result(name: &str, o: MatchOutcome, wait: &MatchWait, total_ms: f64) -> CdpResult<Value> {
    let mut v = json!({
        "name": name,
        "pass": o.pass,
        "hashHit": o.hash_hit,
        "recorded": o.recorded,
        "diffPixels": o.diff_pixels,
        "sceneHash": format!("{:016x}", o.scene_hash),
        "width": o.width,
        "height": o.height,
        "ms": timings_json(&o.timings, wait.snapshot_ms, total_ms),
    });
    if let Some(actual) = &o.actual {
        v["actual"] = json!(base64_encode(actual));
    }
    if let Some(diff) = &o.diff {
        v["diff"] = json!(base64_encode(diff));
    }
    if o.pass {
        return Ok(v);
    }
    let message = format!(
        "matchScreenshot '{name}': {} pixels differ ({}x{}); the actual image and the diff are in error.data",
        o.diff_pixels, o.width, o.height
    );
    Err(CdpError::server(message).with_data(Some(v)))
}

/// `printToPDF` `pageRanges` ("1-5, 8, 11-13"; "-3" from the first page,
/// "4-" to the last) as 1-based inclusive ranges; `None` when malformed.
fn parse_page_ranges(ranges: &str) -> Option<Vec<(usize, usize)>> {
    let page = |s: &str| s.trim().parse::<usize>().ok().filter(|&n| n > 0);
    ranges
        .split(',')
        .map(|range| {
            let range = range.trim();
            let (from, to) = match range.split_once('-') {
                Some((from, to)) if from.trim().is_empty() && to.trim().is_empty() => return None,
                Some((from, to)) => (
                    if from.trim().is_empty() { 1 } else { page(from)? },
                    if to.trim().is_empty() { usize::MAX } else { page(to)? },
                ),
                None => (page(range)?, page(range)?),
            };
            (from <= to).then_some((from, to))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_page_ranges;

    #[test]
    fn page_ranges_parse_like_chrome() {
        assert_eq!(parse_page_ranges("1-3, 5"), Some(vec![(1, 3), (5, 5)]));
        assert_eq!(parse_page_ranges("-2,4-"), Some(vec![(1, 2), (4, usize::MAX)]));
        for bad in ["0", "3-1", "a", "1-2-3", "1,,2", "-"] {
            assert_eq!(parse_page_ranges(bad), None, "{bad}");
        }
    }
}
