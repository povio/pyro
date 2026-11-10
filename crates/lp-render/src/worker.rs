//! The render thread: one per CDP connection, created on first use. It owns
//! a [`Renderer`] (fonts, raster backend, sub-resource cache), so the
//! browser thread only builds a snapshot and goes on with its event loop.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::{
    compare, decode_png, diff_image, encode_jpeg, encode_png, Area, Backend, Fetcher, Renderer, RgbaImage, Snapshot,
    Timings,
};

/// A baseline, cached per name on the connection. The PNG is decoded on the
/// render thread the first time a render is compared with it (never on a
/// hash hit), then kept decoded.
pub struct Baseline {
    /// xxh3 of the PNG bytes.
    pub digest: u64,
    png: Mutex<Option<Vec<u8>>>,
    image: OnceLock<Result<RgbaImage, String>>,
    /// Display-list hash of a render that matched it exactly (0: none yet).
    scene_hash: AtomicU64,
}

impl Baseline {
    pub fn from_png(png: Vec<u8>, scene_hash: Option<u64>) -> Baseline {
        Baseline {
            digest: crate::digest(&png),
            png: Mutex::new(Some(png)),
            image: OnceLock::new(),
            scene_hash: AtomicU64::new(scene_hash.unwrap_or(0)),
        }
    }

    pub fn from_image(image: RgbaImage, scene_hash: Option<u64>) -> Baseline {
        Baseline {
            digest: 0,
            png: Mutex::new(None),
            image: OnceLock::from(Ok(image)),
            scene_hash: AtomicU64::new(scene_hash.unwrap_or(0)),
        }
    }

    /// The decoded image (decoding it on first use).
    pub fn image(&self) -> Result<&RgbaImage, String> {
        self.image
            .get_or_init(|| {
                let png = self.png.lock().ok().and_then(|mut p| p.take()).unwrap_or_default();
                decode_png(&png).map_err(|e| format!("baseline: {e}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn scene_hash(&self) -> Option<u64> {
        Some(self.scene_hash.load(Ordering::Acquire)).filter(|h| *h != 0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MatchOptions {
    /// Largest per-channel difference a pixel may have and still match.
    pub tolerance: u8,
    /// Differing pixels allowed.
    pub max_diff_pixels: Option<u64>,
    /// Differing pixels allowed as a fraction of the image. With both set,
    /// the smaller limit applies (Playwright's comparators.js); with
    /// neither, no pixel may differ.
    pub max_diff_ratio: Option<f64>,
    pub backend: Backend,
}

impl MatchOptions {
    /// Playwright fails when `count > limit` with an unfloored ratio limit;
    /// for an integer count that is `count > floor(limit)`.
    pub fn allowed(&self, width: u32, height: u32) -> u64 {
        let by_ratio = self.max_diff_ratio.map(|r| (r.clamp(0.0, 1.0) * width as f64 * height as f64).floor() as u64);
        match (self.max_diff_pixels, by_ratio) {
            (Some(p), Some(r)) => p.min(r),
            (p, r) => p.or(r).unwrap_or(0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg(u8),
}

pub struct Captured {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub scene_hash: u64,
    pub timings: Timings,
}

#[derive(Debug)]
pub struct MatchOutcome {
    pub pass: bool,
    /// Passed on the display-list hash: no raster, no compare.
    pub hash_hit: bool,
    /// No baseline was given: the render is returned as `actual`.
    pub recorded: bool,
    pub diff_pixels: u64,
    pub scene_hash: u64,
    pub width: u32,
    pub height: u32,
    pub timings: Timings,
    /// PNG of the render, when recorded or failed.
    pub actual: Option<Vec<u8>>,
    /// PNG of the difference, when failed.
    pub diff: Option<Vec<u8>>,
}

pub enum Job {
    Capture {
        snapshot: Snapshot,
        area: Area,
        format: ImageFormat,
        backend: Backend,
        fetcher: Arc<dyn Fetcher>,
        done: Box<dyn FnOnce(Result<Captured, String>) + Send>,
    },
    Match {
        snapshot: Snapshot,
        area: Area,
        baseline: Option<Arc<Baseline>>,
        opts: MatchOptions,
        fetcher: Arc<dyn Fetcher>,
        done: Box<dyn FnOnce(Result<MatchOutcome, String>) + Send>,
    },
    /// `Page.printToPDF`.
    Pdf {
        snapshot: Snapshot,
        opts: crate::PdfOptions,
        fetcher: Arc<dyn Fetcher>,
        done: Box<dyn FnOnce(Result<crate::PdfOutput, String>) + Send>,
    },
    /// Writes pages painted elsewhere ([`crate::Renderer::paint_pdf_page`]).
    PdfWrite { paint: Box<crate::PdfPaint>, done: Box<dyn FnOnce(Result<crate::PdfOutput, String>) + Send> },
    /// Drops a page's persistent document ([`crate::Live`]).
    Forget { page: u64 },
}

/// The render thread has exited.
#[derive(Debug)]
pub struct WorkerGone;

/// Handle to the render thread; dropping it ends the thread after the
/// queued jobs.
pub struct RenderWorker {
    tx: Sender<Job>,
}

/// Run on the render thread a second after its last job ([`set_idle_release`]).
static IDLE_RELEASE: OnceLock<fn()> = OnceLock::new();

/// How long the render thread waits without a job before it runs the
/// [`set_idle_release`] hook.
const IDLE_RELEASE_AFTER: Duration = Duration::from_secs(1);

/// Registers what the render thread runs once a second has passed since its
/// last job: an allocator with per-thread heaps (the executable's) returns
/// the pages of what the render freed and of the images the browser thread
/// freed since, which only the thread that allocated them can (a closed SPA
/// after one screenshot: about 2 MB).
pub fn set_idle_release(release: fn()) {
    let _ = IDLE_RELEASE.set(release);
}

/// The render thread's stack: a main thread's, as the browser threads have.
const RENDER_STACK_BYTES: usize = 8 * 1024 * 1024;

impl RenderWorker {
    pub fn spawn() -> std::io::Result<RenderWorker> {
        let (tx, rx) = channel::<Job>();
        // The browser thread's stack (lp-cdp `BROWSER_STACK_BYTES`): the render lays the page out
        // again, once per tree level as there, and paints it recursively. Those recursions grow the
        // stack on demand (blitz-dom `util::ensure_stack`), so no depth overflows it; 8 MB keeps
        // pages up to about 3,000 levels on the thread's own stack, without mapping segments.
        std::thread::Builder::new().name("lp-render".into()).stack_size(RENDER_STACK_BYTES).spawn(move || {
            let mut renderer = None;
            // A job ran since the idle hook did.
            let mut due = false;
            loop {
                let job = match (due, IDLE_RELEASE.get()) {
                    (true, Some(release)) => match rx.recv_timeout(IDLE_RELEASE_AFTER) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => {
                            release();
                            due = false;
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    },
                    _ => match rx.recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    },
                };
                let r = renderer.get_or_insert_with(Renderer::new);
                run(r, job);
                due = true;
            }
        })?;
        Ok(RenderWorker { tx })
    }

    /// Queues a job; `Err` when the thread is gone.
    pub fn submit(&self, job: Job) -> Result<(), WorkerGone> {
        self.tx.send(job).map_err(|_| WorkerGone)
    }
}

fn run(r: &mut Renderer, job: Job) {
    match job {
        Job::Capture { snapshot, area, format, backend, fetcher, done } => {
            let out = guarded(r, &snapshot, |r| capture(r, &snapshot, area, format, backend, &fetcher));
            done(out);
        }
        Job::Match { snapshot, area, baseline, opts, fetcher, done } => {
            let out =
                guarded(r, &snapshot, |r| match_baseline(r, &snapshot, area, baseline.as_deref(), opts, &fetcher));
            done(out);
        }
        Job::Pdf { snapshot, opts, fetcher, done } => {
            let out = guarded(r, &snapshot, |r| r.print_pdf(&snapshot, &fetcher, &opts).map_err(|e| e.to_string()));
            done(out);
        }
        Job::PdfWrite { paint, done } => {
            let out = catch_unwind(AssertUnwindSafe(|| r.write_pdf(*paint).map_err(|e| e.to_string())))
                .unwrap_or_else(|_| Err("the PDF writer panicked".into()));
            done(out);
        }
        Job::Forget { page } => r.forget(page),
    }
}

/// Runs a render; a panic becomes an error, and drops the page's persistent
/// document (it may be half updated).
fn guarded<T>(
    r: &mut Renderer,
    snap: &Snapshot,
    f: impl FnOnce(&mut Renderer) -> Result<T, String>,
) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(|| f(&mut *r))).unwrap_or_else(|panic| {
        if let Some(live) = &snap.live {
            r.forget(live.page);
        }
        let what = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(format!("renderer panicked: {what}"))
    })
}

pub fn capture(
    r: &mut Renderer,
    snap: &Snapshot,
    area: Area,
    format: ImageFormat,
    backend: Backend,
    fetcher: &Arc<dyn Fetcher>,
) -> Result<Captured, String> {
    let (image, scene_hash, mut timings) = r.render(snap, area, backend, fetcher).map_err(|e| e.to_string())?;
    let t = Instant::now();
    let bytes = match format {
        ImageFormat::Png => encode_png(&image),
        ImageFormat::Jpeg(q) => encode_jpeg(&image, q)?,
    };
    timings.encode = t.elapsed();
    Ok(Captured { bytes, width: image.width, height: image.height, scene_hash, timings })
}

pub fn match_baseline(
    r: &mut Renderer,
    snap: &Snapshot,
    area: Area,
    baseline: Option<&Baseline>,
    opts: MatchOptions,
    fetcher: &Arc<dyn Fetcher>,
) -> Result<MatchOutcome, String> {
    let painted = r.paint(snap, area, fetcher).map_err(|e| e.to_string())?;
    let (scene_hash, width, height) = (painted.hash, painted.width, painted.height);
    let outcome = |pass, hash_hit, diff_pixels, timings| MatchOutcome {
        pass,
        hash_hit,
        recorded: false,
        diff_pixels,
        scene_hash,
        width,
        height,
        timings,
        actual: None,
        diff: None,
    };
    if let Some(b) = baseline {
        // The hash stands for the pixels of the CPU backend only.
        if opts.backend == Backend::Cpu && b.scene_hash() == Some(scene_hash) {
            return Ok(outcome(true, true, 0, painted.timings));
        }
    }
    let (image, mut timings) = r.raster(painted, opts.backend);
    let Some(b) = baseline else {
        let t = Instant::now();
        let actual = encode_png(&image);
        timings.encode = t.elapsed();
        let mut o = outcome(true, false, 0, timings);
        o.recorded = true;
        o.actual = Some(actual);
        return Ok(o);
    };
    let t = Instant::now();
    let expected = b.image()?;
    timings.decode = t.elapsed();
    let t = Instant::now();
    let allowed = opts.allowed(width, height);
    let cmp = compare(&image, expected, opts.tolerance, Some(allowed));
    timings.compare = t.elapsed();
    let pass = cmp.diff_pixels <= allowed;
    if cmp.equal && opts.backend == Backend::Cpu {
        b.scene_hash.store(scene_hash, Ordering::Release);
    }
    if pass {
        return Ok(outcome(true, false, cmp.diff_pixels, timings));
    }
    // Failed: count every differing pixel for the report, then the images.
    let t = Instant::now();
    let full = compare(&image, expected, opts.tolerance, None);
    let diff = diff_image(&image, expected, opts.tolerance);
    timings.compare += t.elapsed();
    let t = Instant::now();
    let mut o = outcome(false, false, full.diff_pixels, timings);
    o.actual = Some(encode_png(&image));
    o.diff = Some(encode_png(&diff));
    o.timings.encode = t.elapsed();
    Ok(o)
}
