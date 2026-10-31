//! Screenshots: CDP `Page.captureScreenshot` and the `LP.run` step
//! `matchScreenshot`, rendered with lp-render (Blitz) off the browser thread.
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
//! applies only what changed (`lp_render::Live`). The node list is built
//! again only when the page's render stamp moved (DOM or sheets changed);
//! otherwise the previous one is sent again, and the render thread skips the
//! diff. Nothing is recorded for a page before its first screenshot, and a
//! closed page's document is dropped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use lp_browser::{Browser, PageId, ResourceKind};
use lp_dom::NodeId;
use lp_render::worker::{Baseline, Captured, ImageFormat, Job, MatchOptions, MatchOutcome, RenderWorker};
use lp_render::{Area, Backend, Fetcher, Live, Snapshot, Timings, Tree, TreeKind, TreeNode};
use lp_runtime::LoopHandle;
use serde::Deserialize;
use serde_json::{json, Value};

use super::driver::{RenderSettlement, ShotStep};
use super::EngineHost;
use crate::msg::{self, CdpError, CdpResult};
use crate::network_protocol::base64_encode;
use crate::target::PageCommand;

enum Finished {
    Capture(u64, Result<Captured, String>),
    Pdf(u64, Result<lp_render::PdfOutput, String>),
    Match(u64, Result<MatchOutcome, String>),
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
    fn deliver(&self, handle: LoopHandle<Browser>) -> impl Fn(Finished) + Send + Clone + 'static {
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
struct PageFetcher {
    handle: LoopHandle<Browser>,
    page: PageId,
}

impl Fetcher for PageFetcher {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let page = self.page;
        let kind = kind_of(&url);
        // A closed loop drops `done`: the render stops waiting at its deadline.
        let _ = self.handle.post(move |b| {
            b.fetch_subresource(
                page,
                url,
                kind,
                Box::new(move |_, result| {
                    done(result.and_then(|r| {
                        if (200..300).contains(&r.status) {
                            Ok(r.body)
                        } else {
                            Err(format!("HTTP {}", r.status))
                        }
                    }))
                }),
            );
        });
    }
}

fn kind_of(url: &str) -> ResourceKind {
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
        ResourceKind::Font
    } else if path.ends_with(".css") {
        ResourceKind::Stylesheet
    } else {
        ResourceKind::Image
    }
}

/// lp-dom's render tree as lp-render takes it (the same `QualName`s).
pub(super) fn convert(t: lp_dom::snapshot::RenderTree) -> Tree {
    use lp_dom::snapshot::RenderNodeKind;
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
    pub(super) fn area(self) -> CdpResult<Area> {
        if !(self.width > 0.0 && self.height > 0.0) {
            return Err(CdpError::invalid_params("clip width and height must be positive"));
        }
        Ok(Area::Clip {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            scale: self.scale.unwrap_or(1.0),
        })
    }
}

impl EngineHost {
    fn snapshot(&mut self, pid: PageId, mark: Option<NodeId>) -> CdpResult<Snapshot> {
        let page = self.thread.browser.page(pid).ok_or_else(|| CdpError::server("page is gone"))?;
        let frame = &page.main_frame;
        let dom = frame.page_dom.clone().ok_or_else(|| CdpError::server("page has no document"))?;
        let stamp = dom.render_stamp();
        let (s, live) = match self.renders.live.get(&pid) {
            // Unchanged: the same `Arc`, which the render thread does not diff.
            Some(l) if l.stamp == stamp => {
                let mut s = dom.render_view();
                s.sheets = l.sheets.clone();
                (s, l.tree.clone())
            }
            _ => {
                let mut s = dom.render_snapshot_tree(&frame.base_url, None);
                let tree = Arc::new(convert(s.tree.take().unwrap_or_default()));
                let sheets = s.sheets.clone();
                self.renders.live.insert(pid, LiveTree { stamp, tree: tree.clone(), sheets });
                (s, tree)
            }
        };
        Ok(Snapshot {
            html: String::new(),
            base_url: frame.base_url.clone(),
            sheets: s.sheets,
            width: s.width,
            height: s.height,
            device_scale_factor: s.device_scale_factor,
            dark: s.dark,
            scroll_x: s.scroll_x,
            scroll_y: s.scroll_y,
            resource_scope: frame.loader_id.0.clone(),
            live: Some(Live { page: pid.0 as u64, tree: live, clip: mark.map(lp_dom::snapshot::render_key) }),
        })
    }

    fn fetcher(&self, pid: PageId) -> Arc<dyn Fetcher> {
        Arc::new(PageFetcher { handle: self.thread.handle(), page: pid })
    }

    /// `Page.captureScreenshot`: replied to from `flush_renders`.
    pub(super) fn capture_screenshot(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let p = cmd.command.params::<Option<CaptureParams>>()?.unwrap_or_default();
        let format = match p.format.as_deref() {
            None | Some("png") => ImageFormat::Png,
            Some("jpeg") => ImageFormat::Jpeg(p.quality.unwrap_or(80)),
            Some(other) => {
                return Err(CdpError::invalid_params(format!("format '{other}' is not supported (png, jpeg)")))
            }
        };
        let area = match (p.clip, p.capture_beyond_viewport) {
            (Some(clip), _) => clip.area()?,
            (None, true) => Area::FullPage,
            (None, false) => Area::Viewport,
        };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let started = Instant::now();
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
            done: Box::new(move |r| deliver(Finished::Capture(token, r))),
        };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        let wait =
            CaptureWait { id: cmd.command.id, session: cmd.session_id.to_owned(), started, snapshot_ms, stream: false };
        self.renders.captures.insert(token, wait);
        Ok(None)
    }

    /// `Page.printToPDF`: one PDF page per Paprize page box (`.pz-page`),
    /// else the document cut to the paper size; replied to from
    /// `flush_renders`. Margins, scale, header/footer and page ranges are
    /// not applied.
    pub(super) fn print_to_pdf(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            landscape: Option<bool>,
            paper_width: Option<f64>,
            paper_height: Option<f64>,
            transfer_mode: Option<String>,
        }
        let p = cmd.command.params::<Option<P>>()?.unwrap_or_default();
        let (mut w, mut h) = (p.paper_width.unwrap_or(8.5) * 96.0, p.paper_height.unwrap_or(11.0) * 96.0);
        if p.landscape == Some(true) {
            std::mem::swap(&mut w, &mut h);
        }
        let opts = lp_render::PdfOptions { page_selector: ".pz-page".into(), paper: (w, h) };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let started = Instant::now();
        let snapshot = self.snapshot(pid, None)?;
        let snapshot_ms = ms(started.elapsed());
        let fetcher = self.fetcher(pid);
        let token = self.renders.token();
        let deliver = self.renders.deliver(self.thread.handle());
        let job = Job::Pdf { snapshot, opts, fetcher, done: Box::new(move |r| deliver(Finished::Pdf(token, r))) };
        self.renders.worker()?.submit(job).map_err(|_| CdpError::server("the render thread is gone"))?;
        let wait = CaptureWait {
            id: cmd.command.id,
            session: cmd.session_id.to_owned(),
            started,
            snapshot_ms,
            stream: p.transfer_mode.as_deref() == Some("ReturnAsStream"),
        };
        self.renders.captures.insert(token, wait);
        Ok(None)
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
            let s = dom.render_snapshot(&frame.base_url, element.map(|id| (id, lp_render::CLIP_ATTRIBUTE)));
            let value = json!({
                "captured": true, "snapshotMs": ms(started.elapsed()),
                "snapshot": { "html": s.html, "baseUrl": frame.base_url, "sheets": s.sheets,
                    "width": s.width, "height": s.height, "deviceScaleFactor": s.device_scale_factor,
                    "dark": s.dark, "scrollX": s.scroll_x, "scrollY": s.scroll_y }
            });
            return Ok(std::rc::Rc::new(std::cell::RefCell::new(Some(Ok(value)))));
        }
        let area = match (spec.clip, spec.full_page, element) {
            (Some((x, y, width, height)), _, _) => Area::Clip { x, y, width, height, scale: 1.0 },
            (None, true, _) => Area::FullPage,
            (None, false, Some(_)) => Area::Element,
            (None, false, None) => Area::Viewport,
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
                    let digest = lp_render::digest(png);
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
        let snapshot = self.snapshot(pid, element)?;
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
        for f in finished {
            match f {
                Finished::Capture(token, result) => {
                    let Some(wait) = self.renders.captures.remove(&token) else { continue };
                    let frame = match result {
                        Ok(c) => {
                            let total = ms(wait.started.elapsed());
                            let body = json!({
                                "data": base64_encode(&c.bytes),
                                "lp": { "width": c.width, "height": c.height, "sceneHash": format!("{:016x}", c.scene_hash), "ms": timings_json(&c.timings, wait.snapshot_ms, total) },
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
                            let lp = json!({
                                "pages": o.pages.iter().map(|p| json!({
                                    "x": p.rect.x0, "y": p.rect.y0, "width": p.rect.width(), "height": p.rect.height(),
                                    "overflowRight": p.overflow_right, "overflowBottom": p.overflow_bottom, "culprit": p.culprit,
                                })).collect::<Vec<_>>(),
                                "bySelector": o.by_selector,
                                "commands": o.commands,
                                "stats": o.stats,
                                "ms": {
                                    "snapshot": wait.snapshot_ms, "fetch": ms(o.timings.fetch), "layout": ms(o.timings.layout),
                                    "paint": ms(o.timings.scene), "pdf": ms(o.timings.raster), "total": total,
                                },
                            });
                            let body = if wait.stream {
                                let handle = format!("lp-pdf-{token}");
                                self.renders.streams.insert(handle.clone(), (o.bytes, 0));
                                json!({ "data": "", "stream": handle, "lp": lp })
                            } else {
                                json!({ "data": base64_encode(&o.bytes), "lp": lp })
                            };
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
            }
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
