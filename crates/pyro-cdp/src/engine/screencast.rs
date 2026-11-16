//! Screencasts: CDP `Page.startScreencast`, `Page.screencastFrameAck` and
//! `Page.stopScreencast`, which Playwright records its videos with (and
//! Puppeteer's `page.screencast()`).
//!
//! A session's screencast sends a `Page.screencastFrame` when it starts and
//! then whenever what the page shows changes, as Chromium sends one per
//! compositor frame with damage. A change is noticed between loop tasks
//! ([`Screencasts::due`], as the driver's waiters notice DOM changes) from
//! what a render snapshot is made of ([`Look`]): the render stamp (DOM and
//! sheets), the viewport, the scroll positions, the hovered and focused
//! elements, the canvas and video bitmaps and the animated values (their
//! state, and the clock while a CSS animation, transition or Web Animation
//! moves: a frame per interval then). While those stay the same nothing is
//! rendered. A frame is rendered as a screenshot is (the page's
//! persistent document on the connection's render thread, `screenshot.rs`),
//! scaled into `maxWidth` x `maxHeight` in the display list rather than
//! after raster, and is dropped before raster when its display list hashes
//! as the last frame's (a change that shows nothing).
//!
//! Frames are capped: one starts at most every [`FRAME_INTERVAL`] (25 per
//! second; `everyNthFrame` divides that further) and one at a time, after the
//! client acked the last one sent (Chromium lets a few go unacked). A change
//! made while a frame renders or waits for its ack gets a frame of its own
//! once that one is acked, so the page's last state always has a frame.
//! Pages without a screencast pay nothing.
//!
//! Pyro's own format `yuv420p` (wright's `recordVideo`) skips image encoding:
//! frames of exactly `maxWidth` x `maxHeight` go as raw video to a Unix socket
//! the client listens on ([`RawSink`]), written on a thread of their own, and
//! the client acks each with a byte on the socket. No `Page.screencastFrame`
//! events.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pyro_browser::{Browser, PageId};
use pyro_render::worker::{Captured, ImageFormat};
use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::network_protocol::base64_encode;
use crate::target::{PageCommand, PageKey};

/// Leads each frame on a [`RawSink`] socket: format and version (`yuv420p`,
/// BT.601 limited range), then width and height (u32), then the timestamp
/// (f64, seconds since the epoch, as `Page.screencastFrame`'s), little-endian.
/// The frame follows; the reader acks it with one byte.
const RAW_MAGIC: &[u8; 4] = b"PYF1";

/// The raw socket's send buffer. macOS gives a Unix socket 8 KB: a 3840x2160
/// frame (12 MB) then took 1,500 reads, and Bun read 280 MB/s; with 4 MB
/// Bun reads it in 25 (3.6 GB/s).
const RAW_SEND_BUFFER: libc::c_int = 4 << 20;

/// A raw screencast's output: a Unix socket the client listens on, written
/// on a thread of its own so that a slow reader holds neither the browser
/// nor the render thread. A frame counts as acked (`done`) when the reader
/// acks it, so frames come no faster than the client takes them. Dropped:
/// the socket is shut down, which also ends a write or a wait for an ack
/// (a frame cut short is the reader's to drop).
pub(super) struct RawSink {
    frames: Option<Sender<(f64, Vec<u8>)>>,
    socket: UnixStream,
}

impl RawSink {
    fn connect(
        path: &str,
        size: (u32, u32),
        done: impl Fn(Result<(), String>) + Send + 'static,
    ) -> std::io::Result<RawSink> {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let socket = UnixStream::connect(path)?;
        // SAFETY: a valid socket descriptor and an int option of the right size.
        unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&RAW_SEND_BUFFER as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
        let mut out = socket.try_clone()?;
        let (frames, rx) = channel::<(f64, Vec<u8>)>();
        std::thread::Builder::new().name("pyro-screencast".into()).spawn(move || {
            for (timestamp, frame) in rx {
                let mut head = [0u8; 20];
                head[..4].copy_from_slice(RAW_MAGIC);
                head[4..8].copy_from_slice(&size.0.to_le_bytes());
                head[8..12].copy_from_slice(&size.1.to_le_bytes());
                head[12..].copy_from_slice(&timestamp.to_le_bytes());
                let mut ack = [0u8; 1];
                let written =
                    out.write_all(&head).and_then(|()| out.write_all(&frame)).and_then(|()| out.read_exact(&mut ack));
                let failed = written.is_err();
                done(written.map_err(|e| e.to_string()));
                if failed {
                    break;
                }
            }
        })?;
        Ok(RawSink { frames: Some(frames), socket })
    }

    fn send(&self, timestamp: f64, frame: Vec<u8>) {
        if let Some(frames) = &self.frames {
            let _ = frames.send((timestamp, frame));
        }
    }
}

impl Drop for RawSink {
    fn drop(&mut self) {
        self.frames = None;
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

/// Shortest time between the starts of two frames of a screencast.
pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(40);

/// What a frame of a page would show, short of rendering it: what its
/// render snapshot is made of.
#[derive(Debug)]
struct Look {
    /// `PageDom::render_stamp`: the document, its DOM and its sheets.
    stamp: (u64, u64, u64),
    /// Viewport (CSS px), device scale factor, dark and print media.
    view: (u32, u32, u64, bool, bool),
    scroll: (u64, u64),
    element_scroll: Vec<(u64, (f64, f64))>,
    sticky_offsets: Vec<(u64, (f64, f64))>,
    hovered: Option<u64>,
    focused: Option<u64>,
    /// Canvas and video bitmaps: a changed one is a new `Arc` (held here,
    /// so its address is not reused by the next).
    bitmaps: Vec<(pyro_dom::NodeId, Arc<Vec<u8>>)>,
    /// `PageDom::animation_stamp`: the animations' state, and the clock while
    /// one moves.
    animations: u64,
}

impl PartialEq for Look {
    fn eq(&self, other: &Look) -> bool {
        self.stamp == other.stamp
            && self.view == other.view
            && self.scroll == other.scroll
            && self.element_scroll == other.element_scroll
            && self.sticky_offsets == other.sticky_offsets
            && self.hovered == other.hovered
            && self.focused == other.focused
            && self.bitmaps.len() == other.bitmaps.len()
            && self.bitmaps.iter().zip(&other.bitmaps).all(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1))
            && self.animations == other.animations
    }
}

/// The [`Look`] of `pid`'s main document (`None`: no page or document).
fn look(browser: &Browser, pid: PageId) -> Option<Look> {
    let dom = browser.page(pid)?.main_frame.page_dom.as_ref()?;
    let v = dom.render_view();
    Some(Look {
        stamp: dom.render_stamp(),
        view: (v.width, v.height, v.device_scale_factor.to_bits(), v.dark, v.print),
        scroll: (v.scroll_x.to_bits(), v.scroll_y.to_bits()),
        element_scroll: v.element_scroll,
        sticky_offsets: v.sticky_offsets,
        hovered: v.hovered,
        focused: v.focused,
        bitmaps: v.canvases.into_iter().map(|(node, _, _, rgba)| (node, rgba)).collect(),
        animations: dom.animation_stamp(),
    })
}

/// A frame on the render thread.
struct Rendering {
    token: u64,
    /// The page when its snapshot was taken.
    look: Look,
    background: Option<[u8; 4]>,
    /// When the snapshot was taken (seconds since the epoch).
    timestamp: f64,
    metadata: Value,
}

struct Screencast {
    session: String,
    page: PageKey,
    pid: PageId,
    /// Chromium's screencast session id: every frame carries it, an ack
    /// names it.
    id: u32,
    format: ImageFormat,
    /// Where `yuv420p` frames go (instead of events).
    sink: Option<RawSink>,
    /// `maxWidth`, `maxHeight` (device px).
    max: (Option<u32>, Option<u32>),
    interval: Duration,
    /// The page (and background override) the last rendered frame showed;
    /// `None` until the first.
    seen: Option<(Look, Option<[u8; 4]>)>,
    /// Display-list hash of the last frame sent.
    hash: Option<u64>,
    rendering: Option<Rendering>,
    /// A frame was sent and not acked yet.
    unacked: bool,
    /// No frame starts before.
    next: Instant,
}

impl Screencast {
    fn idle(&self) -> bool {
        self.rendering.is_none() && !self.unacked
    }

    fn changed(&self, look: &Look) -> bool {
        self.seen.as_ref().is_none_or(|(seen, _)| seen != look)
    }
}

/// The screencasts of a connection's sessions (at most one per session).
#[derive(Default)]
pub(crate) struct Screencasts {
    list: Vec<Screencast>,
    next_id: u32,
}

impl Screencasts {
    /// Whether a screencast would start a frame now: free to, and its page
    /// changed since its last frame. Called between loop tasks.
    pub(super) fn due(&self, browser: &Browser) -> bool {
        if self.list.is_empty() {
            return false;
        }
        let now = Instant::now();
        self.list.iter().any(|s| s.idle() && now >= s.next && look(browser, s.pid).is_some_and(|l| s.changed(&l)))
    }

    /// When a screencast that waits out its interval may start its next
    /// frame: the loop wakes then to look for a change made meanwhile.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        if self.list.is_empty() {
            return None;
        }
        let now = Instant::now();
        self.list.iter().filter(|s| s.idle() && s.next > now).map(|s| s.next).min()
    }

    pub(super) fn forget_page(&mut self, page: PageKey) {
        self.list.retain(|s| s.page != page);
    }

    pub(super) fn forget_session(&mut self, session: &str) {
        self.list.retain(|s| s.session != session);
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }
}

impl EngineHost {
    /// `Page.startScreencast`: `format` (`png`, Chromium's default, or
    /// `jpeg` at `quality`, default 80), `maxWidth`/`maxHeight` (device px:
    /// frames are scaled down to fit), `everyNthFrame` (frames at most every
    /// n × 40 ms). Starting again restarts with the new parameters.
    ///
    /// Pyro's `format: "yuv420p"` with `socket` (a Unix socket path the
    /// client listens on): frames of exactly `maxWidth` x `maxHeight`, the
    /// page scaled down to fit at the top left and gray around it, go there
    /// raw ([`RawSink`], [`RAW_MAGIC`]) instead of as events.
    pub(super) fn start_screencast(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            format: Option<String>,
            quality: Option<i64>,
            max_width: Option<i64>,
            max_height: Option<i64>,
            every_nth_frame: Option<i64>,
            socket: Option<String>,
        }
        let p = cmd.command.params::<Option<P>>()?.unwrap_or_default();
        let positive = |v: Option<i64>| v.filter(|v| *v > 0).map(|v| v.min(i64::from(u32::MAX)) as u32);
        let max = (positive(p.max_width), positive(p.max_height));
        // As Chromium: a quality out of 0-100 is the default.
        let quality = p.quality.filter(|q| (0..=100).contains(q)).unwrap_or(80) as u8;
        let format = match p.format.as_deref() {
            None | Some("png") => ImageFormat::Png,
            Some("jpeg") => ImageFormat::Jpeg(quality),
            Some("yuv420p") => match max {
                (Some(width), Some(height)) if width <= pyro_render::MAX_WIDTH && height <= pyro_render::MAX_HEIGHT => {
                    ImageFormat::Yuv420p { width, height }
                }
                _ => {
                    return Err(CdpError::invalid_params(format!(
                        "format 'yuv420p' needs maxWidth and maxHeight, the frame size (at most {}x{})",
                        pyro_render::MAX_WIDTH,
                        pyro_render::MAX_HEIGHT
                    )))
                }
            },
            Some(other) => {
                return Err(CdpError::invalid_params(format!(
                    "format '{other}' is not supported (jpeg, png, and pyro's yuv420p)"
                )))
            }
        };
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let every = p.every_nth_frame.filter(|n| *n > 0).unwrap_or(1).min(1000) as u32;
        self.screencasts.forget_session(cmd.session_id);
        let id = self.screencasts.next_id.wrapping_add(1).max(1);
        self.screencasts.next_id = id;
        let sink = match (format, p.socket.as_deref()) {
            (ImageFormat::Yuv420p { width, height }, Some(path)) => {
                let deliver = self.renders.deliver(self.thread.handle());
                let done = move |r| deliver(super::screenshot::Finished::Written(id, r));
                let sink = RawSink::connect(path, (width, height), done).map_err(|e| {
                    CdpError::server(format!("Page.startScreencast: cannot write frames to socket {path}: {e}"))
                })?;
                Some(sink)
            }
            (ImageFormat::Yuv420p { .. }, None) => {
                return Err(CdpError::invalid_params("format 'yuv420p' needs socket, a Unix socket to write frames to"))
            }
            _ => None,
        };
        let casts = &mut self.screencasts;
        casts.list.push(Screencast {
            session: cmd.session_id.to_owned(),
            page: cmd.page,
            pid,
            id,
            format,
            sink,
            max,
            interval: FRAME_INTERVAL * every,
            seen: None,
            hash: None,
            rendering: None,
            unacked: false,
            next: Instant::now(),
        });
        Ok(Some(json!({})))
    }

    /// `Page.screencastFrameAck`: the client has the last frame; an ack of
    /// an earlier screencast changes nothing.
    pub(super) fn screencast_frame_ack(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            session_id: i64,
        }
        let p: P = cmd.command.params()?;
        let session = cmd.session_id;
        if let Some(s) = self.screencasts.list.iter_mut().find(|s| s.session == session) {
            if i64::from(s.id) == p.session_id {
                s.unacked = false;
            }
        }
        Ok(Some(json!({})))
    }

    /// `Page.stopScreencast`: no frame after this reply.
    pub(super) fn stop_screencast(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        self.screencasts.forget_session(cmd.session_id);
        Ok(Some(json!({})))
    }

    /// Starts a frame for each screencast that is free to and whose page
    /// changed since its last frame (from `flush`).
    pub(super) fn service_screencasts(&mut self) {
        if self.screencasts.list.is_empty() {
            return;
        }
        let now = Instant::now();
        for i in 0..self.screencasts.list.len() {
            let s = &self.screencasts.list[i];
            if !s.idle() || now < s.next {
                continue;
            }
            let (pid, page) = (s.pid, s.page);
            let Some(look) = look(&self.thread.browser, pid) else { continue };
            let background = self.pages.get(&page).and_then(|p| p.background_override);
            if s.seen.as_ref().is_some_and(|(seen, b)| *seen == look && *b == background) {
                continue;
            }
            let (max, format, unless) = (s.max, s.format, s.hash);
            // When the snapshot is taken.
            let timestamp = epoch_seconds();
            let started = self.render_frame(pid, max, format, unless);
            let s = &mut self.screencasts.list[i];
            s.next = now + s.interval;
            match started {
                Ok((token, view)) => {
                    let metadata = json!({
                        "offsetTop": 0, "pageScaleFactor": 1,
                        "deviceWidth": view.width, "deviceHeight": view.height,
                        "scrollOffsetX": view.scroll_x, "scrollOffsetY": view.scroll_y,
                        "timestamp": timestamp,
                    });
                    s.rendering = Some(Rendering { token, look, background, timestamp, metadata });
                }
                // Not retried until the page changes again.
                Err(_) => s.seen = Some((look, background)),
            }
        }
    }

    /// A frame left the render thread: sent, unless its display list was
    /// the last frame's (`None`) or it failed (dropped, as Chromium drops a
    /// frame it cannot encode).
    pub(super) fn screencast_rendered(&mut self, token: u64, result: Result<Option<Captured>, String>) {
        let Some(s) = self.screencasts.list.iter_mut().find(|s| s.rendering.as_ref().is_some_and(|r| r.token == token))
        else {
            return;
        };
        let Some(r) = s.rendering.take() else { return };
        s.seen = Some((r.look, r.background));
        let Ok(Some(frame)) = result else { return };
        s.hash = Some(frame.scene_hash);
        s.unacked = true;
        if let Some(sink) = &s.sink {
            // Acked by `screencast_written` once the client acked it on the socket.
            sink.send(r.timestamp, frame.bytes);
            return;
        }
        let params = json!({ "data": base64_encode(&frame.bytes), "metadata": r.metadata, "sessionId": s.id });
        let session = s.session.clone();
        self.event("Page.screencastFrame", &params, &session);
    }

    /// A raw frame of screencast `id` was written to its socket and acked
    /// by the client, or could not be: the reader is gone, and the
    /// screencast ends.
    pub(super) fn screencast_written(&mut self, id: u32, result: Result<(), String>) {
        let list = &mut self.screencasts.list;
        match result {
            Ok(()) => {
                if let Some(s) = list.iter_mut().find(|s| s.id == id) {
                    s.unacked = false;
                }
            }
            Err(_) => list.retain(|s| s.id != id),
        }
    }
}

/// Chromium's frame timestamp: seconds since the Unix epoch.
fn epoch_seconds() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}
