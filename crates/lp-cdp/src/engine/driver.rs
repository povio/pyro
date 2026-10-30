//! `LP.*`: a native test driver.
//!
//! `LP.run` takes a whole scenario, a list of steps (`goto`, `route`, `act`,
//! `check`, `read`, `eval`, `request`), and replies once, when the last step
//! holds or the first one fails, with the values read and the requests the
//! page made meanwhile (`request` steps check that log).
//! `LP.act` and `LP.expect` are one-step runs. Locators (with `within` and
//! `filter` chains) are resolved against the arena DOM in Rust and the
//! actionability checks (unique, visible, enabled, editable, not covered) run
//! here, so a scenario costs one round trip and no JavaScript.
//!
//! A run whose current step cannot finish yet becomes a [`Waiter`]. Nothing
//! polls: a waiting step is tried again only when the page's DOM version moves
//! (or its document is replaced), a `goto` when its document reaches `load`,
//! and an action's settle when the page goes quiet.
//! `EngineHost::run_until_work` returns to the serve loop when a waiter is
//! ready, and `EngineHost::flush` drives the ready ones before sending queued
//! events.
//!
//! Settle: after an action (unless `settle: false`), the next step waits until
//! the page is quiet (`Browser::page_work`): no queued task, no `setTimeout`
//! due within `TIMER_HORIZON`, no pending animation frame, no navigation short
//! of DOMContentLoaded, and no more requests in flight than before the action.
//! Readiness is checked between loop tasks, so the step goes on right after
//! the last piece of work. It gives up after [`SETTLE_MAX`], carries on, and
//! reports what was still pending. An `idle` step (`LP.idle`) waits the same
//! way with no requests allowed, up to its timeout.
//!
//! Turns: each input event of a pointer action (`click`, `dblclick`, `hover`,
//! `drag`) is dispatched in its own task, with an event-loop turn (the frame
//! interval, then a marker task: queued tasks, due timers, animation frames)
//! between two of them, as Chromium delivers input events ([`Phase::Act`]). An
//! action that does not settle, and an `eval`, gets one such turn before the
//! next step starts.
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use lp_browser::{Browser, NavigateOpts, PageId};
use lp_dom::layout::Layout;
use lp_dom::select::SelectorCache;
use lp_dom::{Dom, NodeId, NodeKind};
use lp_js::JsException;
use lp_runtime::{Scheduler, TaskSource};
use lp_webapi::input::{self, MouseButton, MouseInput, MouseKind};
use lp_webapi::PageDom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
/// Waiting runs per connection (each holds only its parsed steps).
const MAX_WAITERS: usize = 1024;
const MAX_STEPS: usize = 10_000;
/// Longest an action waits for the page to go quiet.
const SETTLE_MAX: Duration = Duration::from_millis(1_000);
/// How many times a turn's marker task lets queued tasks go first.
const MARKER_HOPS: u32 = 4;

/// What a locator names. Exactly one of `css`, `testId`, `text`, `role`,
/// `label` or `placeholder`, searched inside each match of `within` (else the
/// document), narrowed by `filter`; `nth` then picks one match instead of
/// requiring a unique one.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Locator {
    css: Option<String>,
    test_id: Option<String>,
    /// Attribute `testId` matches (default `data-testid`).
    test_id_attribute: Option<String>,
    text: Option<Pattern>,
    role: Option<String>,
    /// Accessible name for `role`.
    name: Option<Pattern>,
    label: Option<Pattern>,
    placeholder: Option<Pattern>,
    /// Whole-string, case-sensitive match for text, name, label and
    /// placeholder (after whitespace normalization); otherwise a
    /// case-insensitive substring. A `{"regex"}` pattern ignores it.
    #[serde(default)]
    exact: bool,
    within: Option<Box<Locator>>,
    filter: Option<Filter>,
    /// Index into the matches (negative counts from the end).
    nth: Option<i64>,
    /// Search inside the document of this `<iframe>` (a locator in the
    /// parent document; may itself have a `frame`).
    frame: Option<Box<Locator>>,
}

/// Narrows a locator's matches. Every given condition must hold.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filter {
    visible: Option<bool>,
    /// Case-insensitive substring of the element's text.
    has_text: Option<Pattern>,
    has_not_text: Option<Pattern>,
    /// Matches at least one element inside.
    has: Option<Box<Locator>>,
    has_not: Option<Box<Locator>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Click,
    Fill,
    Focus,
    Check,
    Uncheck,
    /// Focus, then a key press (`value`: a key name such as `Enter`, or a
    /// chord such as `Shift+Tab`).
    Press,
    /// Move the pointer to the element's center (tooltips, hover toolbars).
    Hover,
    /// Two clicks at the center, the second with click count 2.
    DblClick,
    /// Remove focus from the element (`blur()`).
    Blur,
    /// Focus, then one key press per character of `value` (`pressSequentially`).
    Type,
    /// Set the files of an `<input type=file>` (`files`, or `value` as a path).
    Upload,
    /// Drag the element onto `to`: native drag and drop when the element is
    /// a drag source, else pointer moves with the button held.
    Drag,
    /// Move the pointer to the element's center, then a wheel turn of
    /// `deltaX`/`deltaY` there (scrolls the nearest scrollable container).
    Wheel,
}

impl Action {
    /// Actions that need an enabled element (Playwright's actionability:
    /// hover, focus, blur, wheel and drag do not).
    fn needs_enabled(self) -> bool {
        matches!(
            self,
            Action::Click
                | Action::DblClick
                | Action::Fill
                | Action::Check
                | Action::Uncheck
                | Action::Press
                | Action::Type
        )
    }
}

/// How a pointer action lands on its element.
#[derive(Clone, Copy, Debug)]
struct PointerOpts {
    /// `click`, `dblclick`, `mouse` down/up/click/dblclick (default left).
    button: MouseButton,
    /// Offset from the element's top-left instead of its center.
    position: Option<(f64, f64)>,
    /// Skip the hit test and the enabled check: act at the point whatever
    /// is there.
    force: bool,
    /// `wheel`: the delta.
    delta: (f64, f64),
}

impl Default for PointerOpts {
    fn default() -> Self {
        PointerOpts { button: MouseButton::Left, position: None, force: false, delta: (0.0, 0.0) }
    }
}

/// A `mouse` step's action, at viewport coordinates (`page.mouse.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseAction {
    Click,
    DblClick,
    Move,
    Down,
    Up,
    Wheel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assert {
    Visible,
    Hidden,
    Text,
    ContainsText,
    Value,
    Count,
    Checked,
    /// A computed style property (`property`) equals `expected`.
    Css,
    /// The element is the document's focused element.
    Focused,
    /// The accessible name equals `expected` (whitespace-normalized).
    Name,
    /// The element is not disabled (`expected: false` for disabled).
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    Count,
    Text,
    Value,
    Visible,
    Box,
    /// A computed style property (`property`).
    Css,
    /// The accessible name.
    Name,
    Focused,
    Enabled,
}

/// Which requests a `request` step looks for in the run's log.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestMatch {
    method: Option<String>,
    /// Exact path (no query).
    path: Option<String>,
    /// Substring of (or regex on) the full URL.
    url: Option<Pattern>,
    /// One status code or a list of acceptable ones (`200` or `[200, 304]`).
    status: Option<StatusSet>,
    /// Exactly this many finished matches (default: at least one).
    count: Option<usize>,
    /// Substring of (or regex on) the request body (as sent, UTF-8).
    body: Option<Pattern>,
    /// The request body, parsed as JSON, equals this value.
    body_json: Option<Value>,
    /// The request body is a JSON object with exactly these top-level keys.
    body_keys: Option<Vec<String>>,
    /// Request headers that must be present (names case-insensitive), each
    /// value a substring of (or regex on) the sent value.
    headers: Option<std::collections::BTreeMap<String, Pattern>>,
    /// Which requests count: `finished` (the default: finished or failed,
    /// as `status` needs) or `started` (sent, still in flight or done).
    state: Option<RequestState>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RequestState {
    Started,
    Finished,
}

impl RequestMatch {
    /// Whether a logged request's method, URL, body and headers fit.
    fn accepts(&self, r: &super::network::LoggedRequest) -> bool {
        self.method.as_ref().is_none_or(|x| r.method.eq_ignore_ascii_case(x))
            && self.path.as_ref().is_none_or(|p| r.path() == p)
            && self.url.as_ref().is_none_or(|u| u.matches(&r.url, false))
            && self.body.as_ref().is_none_or(|p| r.post_data_text().is_some_and(|t| p.matches_raw(t)))
            && self.body_json.as_ref().is_none_or(|j| r.post_data_json().as_ref() == Some(j))
            && self.body_keys.as_ref().is_none_or(|keys| {
                r.post_data_json()
                    .as_ref()
                    .and_then(Value::as_object)
                    .is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(k)))
            })
            && self
                .headers
                .as_ref()
                .is_none_or(|h| h.iter().all(|(name, p)| r.request_header_values(name).any(|v| p.matches_raw(v))))
    }
}

/// `status: 200` or `status: [200, 304]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusSet(Vec<u16>);

impl StatusSet {
    fn accepts(&self, status: Option<u16>) -> bool {
        status.is_some_and(|s| self.0.contains(&s))
    }
}

impl<'de> Deserialize<'de> for StatusSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(u16),
            Many(Vec<u16>),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::One(s) => StatusSet(vec![s]),
            Raw::Many(v) if v.is_empty() => return Err(serde::de::Error::custom("status list is empty")),
            Raw::Many(v) => StatusSet(v),
        })
    }
}

impl Serialize for StatusSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [one] => one.serialize(s),
            many => many.serialize(s),
        }
    }
}

/// `{"mock": {...}}`: what the context answers matching requests with.
#[derive(Clone, Debug)]
struct MockSpec {
    url: Pattern,
    method: Option<String>,
    status: u16,
    content_type: Option<String>,
    default_type: &'static str,
    headers: Vec<(String, String)>,
    body: String,
    delay: Duration,
    abort: Option<String>,
    times: Option<u32>,
    /// Let the request reach the real backend (then `patch`, `hold`, `delay`
    /// apply to its response).
    passthrough: bool,
    /// Park the request (or its response) under this name until `release`.
    hold: Option<String>,
    /// `status`, `body` or `json` were given.
    responds: bool,
    patch: Option<PatchSpec>,
}

/// `mock.patch`: rewrites of a passed-through response. `json` is a JSON
/// merge patch (RFC 7396) when the real body is a JSON object, else the
/// new body; `body` replaces the body; `headers` are set.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchSpec {
    status: Option<u16>,
    #[serde(default)]
    headers: serde_json::Map<String, Value>,
    json: Option<Value>,
    body: Option<String>,
}

/// A file for `upload`.
#[derive(Clone, Debug)]
struct UploadFile {
    name: String,
    mime: String,
    bytes: Vec<u8>,
}

/// A run holds a few steps; the large `Act` variant is not worth boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
enum Step {
    /// Waits for the current document's `load` (a prepared page taken while
    /// it was still booting).
    Loaded,
    /// Reloads the current document and waits for its `load`.
    Reload,
    /// Waits until the page URL contains (or matches) the pattern.
    Url(Pattern),
    /// Registers a network mock on the page's browser context.
    Mock(MockSpec),
    /// Removes mocks by URL pattern (all when `None`).
    Unmock(Option<Pattern>),
    /// Releases the requests (or responses) a mock holds under this name;
    /// waits for one to be held.
    Release {
        hold: String,
    },
    /// Waits for the next download the page starts (since the run began).
    Download {
        /// Directory to save it in (as the suggested name).
        save_to: Option<String>,
        key: Option<String>,
    },
    /// Waits for the next popup this page opened; its target id.
    Popup {
        key: Option<String>,
    },
    Goto {
        url: String,
        dom_ready: bool,
    },
    /// Waits until the page is quiet (see the module docs), up to the step's
    /// timeout; never fails, reports what was still pending instead.
    Idle,
    /// Same-document navigation: `history.pushState` and a `popstate` event,
    /// as a client-side router reacts to; settles like an action.
    Route {
        url: String,
        /// Wait for the requests the route change started (default).
        settle: bool,
    },
    Eval {
        expression: String,
        key: Option<String>,
    },
    /// Resizes the viewport mid-run (`innerWidth`, `resize`, media queries,
    /// layout); settles like an action so listeners have run.
    Viewport {
        width: u32,
        height: u32,
    },
    /// Replaces the page's media emulation (`prefers-color-scheme`, ...).
    Media(super::metrics::MediaEmulation),
    /// The following steps drive another page of the context: a popup's
    /// `as` key, a target id, or `main` (the page the run started on).
    Page {
        target: String,
    },
    Request(RequestMatch),
    Act {
        locator: Locator,
        action: Action,
        value: String,
        settle: bool,
        /// `drag`: the drop target.
        to: Option<Locator>,
        /// `upload`: the files.
        files: Vec<UploadFile>,
        /// `click`, `dblclick`, `hover`, `wheel`: how the pointer lands.
        pointer: PointerOpts,
    },
    /// A pointer action at viewport coordinates, not tied to an element.
    Mouse {
        action: MouseAction,
        /// Where; `down`, `up` and `wheel` default to the current position.
        x: Option<f64>,
        y: Option<f64>,
        pointer: PointerOpts,
        /// `move`: intermediate moves from the current position.
        steps: u32,
        settle: bool,
    },
    Check {
        locator: Locator,
        assert: Assert,
        expected: Value,
        not: bool,
        /// `css`: the property name.
        property: Option<String>,
    },
    Read {
        locator: Locator,
        read: Read,
        key: Option<String>,
        /// `css`: the property name.
        property: Option<String>,
    },
    /// Renders the page (or an area, or an element) and compares it with a
    /// baseline image (`screenshot.rs`).
    MatchScreenshot(Box<ShotStep>),
}

/// A `matchScreenshot` step.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "render"), allow(dead_code))]
pub(super) struct ShotStep {
    pub(super) name: String,
    /// PNG bytes; omitted: the baseline cached under `name` on this
    /// connection, else the render is returned (`recorded`).
    pub(super) baseline: Option<Vec<u8>>,
    /// Display-list hash stored with the baseline.
    pub(super) scene_hash: Option<u64>,
    /// 0..1: per-channel difference a pixel may have and still match.
    pub(super) threshold: f64,
    pub(super) max_diff_pixels: u64,
    pub(super) max_diff_pixel_ratio: f64,
    /// CSS px, relative to the document.
    pub(super) clip: Option<(f64, f64, f64, f64)>,
    pub(super) full_page: bool,
    /// Capture this element's box.
    pub(super) locator: Option<Locator>,
    /// Ignore the baseline: render and return the image.
    pub(super) update: bool,
    pub(super) key: Option<String>,
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Goto { url, .. } => format!("goto {url}"),
            Step::Loaded => "loaded".into(),
            Step::Idle => "idle".into(),
            Step::Reload => "reload".into(),
            Step::Url(p) => format!("url {}", p.text),
            Step::Mock(m) => format!("mock {}", m.url.text),
            Step::Unmock(Some(p)) => format!("unmock {}", p.text),
            Step::Unmock(None) => "unmock".into(),
            Step::Release { hold } => format!("release {hold}"),
            Step::Download { .. } => "download".into(),
            Step::Popup { .. } => "popup".into(),
            Step::Route { url, .. } => format!("route {url}"),
            Step::Eval { .. } => "eval".into(),
            Step::Viewport { width, height } => format!("viewport {width}x{height}"),
            Step::Media(m) => format!("media {m:?}"),
            Step::Page { target } => format!("page {target}"),
            Step::Request(m) => format!("request {}", serde_json::to_value(m).map(prune).unwrap_or_default()),
            Step::Act { locator, action, .. } => format!("{action:?} {}", locator.describe()),
            Step::Mouse { action, x, y, .. } => match (x, y) {
                (Some(x), Some(y)) => format!("mouse {action:?} at {x},{y}"),
                _ => format!("mouse {action:?}"),
            },
            Step::Check { locator, assert, .. } => format!("expect {assert:?} {}", locator.describe()),
            Step::Read { locator, read, .. } => format!("read {read:?} {}", locator.describe()),
            Step::MatchScreenshot(shot) => format!("matchScreenshot {}", shot.name),
        }
    }
}

/// One try of a step.
enum Attempt {
    Done(Value),
    /// Not yet: the reason is reported if the deadline passes.
    Retry(String),
    /// The action's first input event went out; the rest follow, one per
    /// event-loop turn.
    Continue(Box<Sequence>),
}

/// A command's steps and what they produced so far.
struct Job {
    id: u64,
    session: String,
    page: PageKey,
    pid: PageId,
    steps: Vec<(Step, Duration)>,
    index: usize,
    results: Vec<Value>,
    reads: serde_json::Map<String, Value>,
    /// `LP.act` / `LP.expect`: reply with the step's own result.
    single: bool,
    started: Instant,
    /// Request log sequence when the run started (`LP.run` only).
    log_from: Option<usize>,
    /// Downloads and popups recorded before the run started are not its.
    downloads_from: usize,
    popups_from: usize,
    /// The page the run started on (`page: "main"`, the reply's requests).
    home: PageKey,
    /// Where the run left the other pages it drove (`page` steps).
    parked: Vec<PageCursor>,
}

/// A page's run state while a `page` step drives another one.
struct PageCursor {
    page: PageKey,
    pid: PageId,
    log_from: Option<usize>,
    downloads_from: usize,
}

impl Job {
    fn reply(mut self, requests: Vec<Value>) -> Value {
        if self.single {
            return self.results.pop().unwrap_or_else(|| json!({}));
        }
        json!({
            "results": self.results,
            "reads": self.reads,
            "requests": requests,
            "ms": self.started.elapsed().as_secs_f64() * 1e3,
        })
    }

    fn fail(&self, error: CdpError) -> CdpError {
        if self.single {
            return error;
        }
        let what = self.steps.get(self.index).map(|(s, _)| s.describe()).unwrap_or_default();
        CdpError::server(format!("step {} ({what}): {}", self.index, error.message)).with_data(error.data)
    }
}

/// Where the current step is.
enum Phase {
    /// Tried; tried again when the document changes.
    Try,
    /// `goto` started; waits for the new document's `load` (or DOMContentLoaded).
    Load { loader: String, dom_ready: bool },
    /// The step's action ran (or an `idle` step started); waits until the
    /// page is quiet with at most `requests` requests in flight.
    Settle { requests: usize, since: Instant, until: Instant },
    /// `eval` returned a promise; waits for it to settle.
    Await(Settlement),
    /// A `matchScreenshot` render runs on the render thread.
    Render(RenderSettlement),
    /// An action's input events still to dispatch (`seq`), one per
    /// event-loop turn; with none left, the turn after a non-settling step.
    /// A turn: the frame interval passes (`at`), then a marker task posted
    /// behind the tasks and timers due by then (`hop`) runs.
    Act { seq: Option<Box<Sequence>>, before: Option<usize>, hop: Option<Rc<Cell<bool>>>, at: Instant },
}

/// Where a promise an `eval` step returned lands once it settles: its JSON
/// value, or the rejection's message.
type Settlement = Rc<RefCell<Option<Result<Value, String>>>>;

/// Where a `matchScreenshot` step's outcome lands: its result, or the
/// failure (a mismatch carries the images in `data`).
pub(super) type RenderSettlement = Rc<RefCell<Option<Result<Value, CdpError>>>>;

/// What evaluating an `eval` step produced.
enum Evaluated {
    Value(Value),
    /// A thenable: its settlement arrives later.
    Pending(Settlement),
}

/// An event-loop turn lasts at least this long: the loop goes idle in it,
/// so an animation frame the event's handlers requested runs in it (frames
/// run as soon as the loop is idle while a client acts,
/// `lp_browser::note_client_activity`).
const TURN: Duration = Duration::from_millis(1);

/// Where a mouse event of a [`Sequence`] goes.
#[derive(Clone, Copy)]
enum At {
    /// The located element's center, computed when the event is dispatched
    /// (hover-dependent layout can shift it between events); once the element
    /// left the document, the last point.
    Element,
    /// The last point.
    Here,
    Point(f64, f64),
}

/// One input event of an action.
#[derive(Clone, Copy)]
enum Input {
    Mouse { kind: MouseKind, button: MouseButton, buttons: u16, count: u32, at: At },
    Drag { kind: input::DragKind, x: f64, y: f64 },
}

/// A `drag`'s geometry and, once a move started a native drag, its data.
struct DragSeq {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    native: Option<input::DragData>,
}

/// The input events of an action still to dispatch, each in its own task so
/// that, as in Chromium, the tasks, timers and animation frames one event's
/// handlers queued run before the next event (a React commit that swaps in a
/// control on hover lands before the press; dnd-kit's frame between moves).
pub(super) struct Sequence {
    page: Rc<PageDom>,
    /// The located element. When it leaves the document mid-sequence, the
    /// rest goes to the element under the pointer, as Chromium's does.
    id: NodeId,
    opts: PointerOpts,
    m: MouseInput,
    events: std::collections::VecDeque<Input>,
    drag: Option<DragSeq>,
}

impl Sequence {
    fn new(
        page: &Rc<PageDom>,
        id: NodeId,
        opts: PointerOpts,
        m: MouseInput,
        events: impl IntoIterator<Item = Input>,
    ) -> Sequence {
        Sequence { page: page.clone(), id, opts, m, events: events.into_iter().collect(), drag: None }
    }

    /// Dispatches the next event. `Ok(None)`: more follow; `Ok(Some)`: the
    /// action's result. `Err`: why the step must be tried again.
    fn advance(&mut self) -> Result<Option<Value>, String> {
        let page = self.page.clone();
        if page.is_disposed() {
            return Err("document is gone".into());
        }
        let Some(next) = self.events.pop_front() else { return Ok(Some(self.result())) };
        // The driver is the client driving the page: frames stay on the
        // short interval while a sequence runs.
        lp_browser::note_client_activity();
        match next {
            Input::Mouse { kind, button, buttons, count, at } => {
                match at {
                    At::Element => {
                        let connected = page.dom.borrow().is_connected(self.id).unwrap_or(false);
                        if connected {
                            let (x, y) = pointer_target(&page, self.id, self.opts).map_err(|retry| match retry {
                                Attempt::Retry(reason) => reason,
                                _ => "element moved".to_owned(),
                            })?;
                            self.m.x = x;
                            self.m.y = y;
                        }
                    }
                    At::Here => {}
                    At::Point(x, y) => {
                        self.m.x = x;
                        self.m.y = y;
                    }
                }
                self.m.button = button;
                self.m.buttons = buttons;
                self.m.click_count = count;
                let intercept = self.drag.is_some() && kind == MouseKind::Moved && buttons & 1 != 0;
                match input::dispatch_mouse_intercepting_drags(&page, kind, &self.m, intercept) {
                    Ok(Some(data)) => {
                        // A native drag source: HTML drag and drop takes over
                        // from the pointer moves.
                        let Some(drag) = &mut self.drag else { return Ok(None) };
                        let (mid_x, mid_y) = ((drag.x0 + drag.x1) / 2.0, (drag.y0 + drag.y1) / 2.0);
                        let (x1, y1) = (drag.x1, drag.y1);
                        drag.native = Some(data);
                        self.events = [
                            Input::Drag { kind: input::DragKind::Enter, x: mid_x, y: mid_y },
                            Input::Drag { kind: input::DragKind::Over, x: x1, y: y1 },
                            Input::Drag { kind: input::DragKind::Drop, x: x1, y: y1 },
                            Input::Mouse {
                                kind: MouseKind::Released,
                                button: MouseButton::Left,
                                buttons: 0,
                                count: 0,
                                at: At::Point(x1, y1),
                            },
                        ]
                        .into_iter()
                        .collect();
                    }
                    Ok(None) => {}
                    // The release that ends a native drag is best effort.
                    Err(_) if kind == MouseKind::Released && self.native_drag() => {}
                    Err(e) => return Err(e),
                }
            }
            Input::Drag { kind, x, y } => {
                let Some(data) = self.drag.as_ref().and_then(|d| d.native.as_ref()) else {
                    return Err("drag data is gone".into());
                };
                input::dispatch_drag(&page, kind, x, y, data, self.m.modifiers)?;
            }
        }
        Ok(if self.events.is_empty() { Some(self.result()) } else { None })
    }

    fn native_drag(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| d.native.is_some())
    }

    fn result(&self) -> Value {
        match &self.drag {
            Some(d) => json!({ "native": d.native.is_some() }),
            None => json!({}),
        }
    }
}

pub(super) struct Waiter {
    job: Job,
    phase: Phase,
    step_deadline: Instant,
    /// Document and DOM version the last try saw.
    doc: Weak<PageDom>,
    seen: u64,
    reason: String,
    /// A `url` step: tried again at this time (the URL can change without
    /// the DOM, e.g. `pushState`).
    poll_at: Option<Instant>,
}

/// How often a `url` step looks again.
const URL_POLL: Duration = Duration::from_millis(25);

impl Waiter {
    fn deadline(&self) -> Instant {
        let step = match &self.phase {
            Phase::Settle { until, .. } => *until,
            Phase::Act { hop: None, at, .. } => *at,
            _ => self.step_deadline,
        };
        match self.poll_at {
            Some(at) if matches!(self.phase, Phase::Try) => step.min(at),
            _ => step,
        }
    }

    /// A `request`, `download` or `popup` step: tried again when the log
    /// it waits on grows.
    fn waits_on_log(&self) -> bool {
        matches!(self.phase, Phase::Try)
            && matches!(
                self.job.steps.get(self.job.index),
                Some((Step::Request(_) | Step::Download { .. } | Step::Popup { .. }, _))
            )
    }

    /// Whether driving it can make progress.
    fn ready(&self, browser: &Browser, now: Instant) -> bool {
        if now >= self.deadline() {
            return true;
        }
        match &self.phase {
            Phase::Try if self.waits_on_log() => false,
            Phase::Try if self.poll_at.is_some() => self.poll_at.is_some_and(|at| now >= at),
            Phase::Try => match self.doc.upgrade() {
                Some(doc) => doc.is_disposed() || doc.dom.borrow().version() != self.seen,
                None => true,
            },
            Phase::Load { loader, dom_ready } => load_reached(browser, self.job.pid, loader, *dom_ready),
            Phase::Settle { requests, .. } => browser.page_work(self.job.pid).is_none_or(|w| w.quiet(*requests)),
            Phase::Await(settled) => settled.borrow().is_some(),
            Phase::Render(settled) => settled.borrow().is_some(),
            Phase::Act { hop, at, .. } => match hop {
                Some(done) => done.get(),
                None => now >= *at,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Waiters {
    list: Vec<Waiter>,
    selectors: RefCell<SelectorCache>,
    /// A page's request log grew since the waiters were last serviced.
    pub(super) log_changed: bool,
}

impl Waiters {
    /// Whether a waiter can make progress. Called between loop tasks.
    pub(super) fn ready(&self, browser: &Browser) -> bool {
        if self.list.is_empty() {
            return false;
        }
        let now = Instant::now();
        self.list.iter().any(|w| w.ready(browser, now))
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.list.iter().map(Waiter::deadline).min()
    }

    /// Virtual time: some run is waiting for its page to settle (or for a
    /// promise an `eval` returned), and every waiting run's page has nothing
    /// left but timers (no task, request or navigation), so only the clock
    /// holds them up. Not while a run waits for a check to hold: a check
    /// such as "the toast is shown" holds only for as long as real time
    /// keeps it, and jumping would skip past it.
    pub(super) fn only_timers_left(&self, browser: &Browser) -> bool {
        !self.list.is_empty()
            && self.list.iter().all(|w| {
                matches!(w.phase, Phase::Settle { .. } | Phase::Await(_))
                    && browser
                        .page_work(w.job.pid)
                        .is_some_and(|work| work.tasks == 0 && work.requests == 0 && work.navigations == 0)
            })
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }
}

/// Where a `Phase::Load` wait for `loader`'s document stands.
enum LoadState {
    /// Fetching or loading.
    Waiting,
    Reached,
    /// Another navigation superseded it before it committed: wait for that one.
    Follow(String),
    /// Neither committed nor pending: the navigation failed.
    Failed,
}

fn load_state(browser: &Browser, pid: PageId, loader: &str, dom_ready: bool) -> LoadState {
    let Some(p) = browser.page(pid) else { return LoadState::Reached };
    let f = &p.main_frame;
    if f.loader_id.0 == loader {
        let done = if dom_ready { f.dom_content_loaded } else { f.loaded };
        return if done { LoadState::Reached } else { LoadState::Waiting };
    }
    match p.pending_loader() {
        Some(pending) if pending.0 == loader => LoadState::Waiting,
        Some(pending) => LoadState::Follow(pending.0.clone()),
        None => LoadState::Failed,
    }
}

fn load_reached(browser: &Browser, pid: PageId, loader: &str, dom_ready: bool) -> bool {
    !matches!(load_state(browser, pid, loader, dom_ready), LoadState::Waiting)
}

/// The document a `loaded` step waits for: the navigation being fetched
/// (a prepared page taken while booting), else the current document.
fn loader_to_wait_for(browser: &Browser, pid: PageId) -> String {
    let Some(p) = browser.page(pid) else { return String::new() };
    p.pending_loader().map_or_else(|| p.main_frame.loader_id.0.clone(), |l| l.0.clone())
}

fn requests_of(browser: &Browser, pid: PageId) -> usize {
    browser.page_work(pid).map_or(0, |w| w.requests)
}

/// `{"tasks": 1, "requests": 2, ...}`: the kinds of work still pending.
fn pending_json(work: &lp_browser::PageWork) -> Value {
    let all = [
        ("tasks", work.tasks),
        ("requests", work.requests),
        ("timers", work.timers),
        ("frames", work.frames),
        ("navigations", work.navigations),
    ];
    Value::Object(all.into_iter().filter(|(_, n)| *n > 0).map(|(k, n)| (k.to_owned(), json!(n))).collect())
}

/// Posts a task that sets `done`, re-posting itself (up to `hops` times)
/// while other tasks are queued, so the tasks queued meanwhile run first.
fn post_marker(sched: Scheduler<Browser>, done: Rc<Cell<bool>>, hops: u32) {
    let next = sched.clone();
    sched.post(TaskSource::Internal, move |_| {
        if hops > 0 && next.pending_tasks() > 0 {
            post_marker(next, done, hops - 1);
        } else {
            done.set(true);
        }
    });
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_matches(have: &str, want: &str, exact: bool) -> bool {
    let have = normalize(have);
    if exact {
        have == normalize(want)
    } else {
        have.to_lowercase().contains(&normalize(want).to_lowercase())
    }
}

/// A string to match (see `Locator::exact`), or `{"regex": "...", "flags": "i"}`
/// tested against the whitespace-normalized text.
#[derive(Clone, Debug)]
pub(super) struct Pattern {
    pub(super) text: String,
    /// ECMAScript semantics (look-around, backreferences), like a JS `RegExp`.
    regex: Option<regress::Regex>,
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Regex {
                regex: String,
                #[serde(default)]
                flags: String,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Text(text) => Pattern { text, regex: None },
            Raw::Regex { regex, flags } => {
                if let Some(f) = flags.chars().find(|f| !matches!(f, 'i' | 'm' | 's')) {
                    return Err(serde::de::Error::custom(format!("unsupported regex flag {f:?}")));
                }
                let re = regress::Regex::with_flags(&regex, flags.as_str())
                    .map_err(|e| serde::de::Error::custom(format!("invalid regex {regex:?}: {e}")))?;
                Pattern { text: regex, regex: Some(re) }
            }
        })
    }
}

impl Serialize for Pattern {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.regex {
            Some(_) => json!({ "regex": self.text }).serialize(s),
            None => self.text.serialize(s),
        }
    }
}

impl Pattern {
    pub(super) fn matches(&self, have: &str, exact: bool) -> bool {
        match &self.regex {
            Some(re) => re.find(&normalize(have)).is_some(),
            None => text_matches(have, &self.text, exact),
        }
    }

    /// Against `have` as is: a case-sensitive substring, or the regex on the
    /// raw text (request bodies and header values).
    pub(super) fn matches_raw(&self, have: &str) -> bool {
        match &self.regex {
            Some(re) => re.find(have).is_some(),
            None => have.contains(&self.text),
        }
    }
}

fn local(dom: &Dom, id: NodeId) -> Option<&str> {
    match &dom.node(id).ok()?.kind {
        NodeKind::Element(e) if e.is_html() => Some(&e.name.local),
        _ => None,
    }
}

fn attr<'d>(dom: &'d Dom, id: NodeId, name: &str) -> Option<&'d str> {
    dom.element(id).ok()?.attr(name)
}

/// A JavaScript value as JSON (`null` when it has no JSON form).
fn json_of(v: lp_js::Value<'_>) -> Value {
    v.to_json(0).ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null)
}

fn text_of(dom: &Dom, id: NodeId) -> String {
    dom.text_content(id).ok().flatten().unwrap_or_default()
}

fn elements(dom: &Dom, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    dom.descendants(root).into_iter().flatten().filter(|n| dom.element(*n).is_ok())
}

/// `aria-*` boolean attribute: `true` case-insensitively.
fn aria_true(dom: &Dom, id: NodeId, name: &str) -> bool {
    attr(dom, id, name).is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

/// `id` and then its ancestor elements, nearest first (the `closest` order).
fn self_and_ancestors(dom: &Dom, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    std::iter::successors(Some(id), move |n| dom.parent_element(*n).ok().flatten())
}

/// Playwright's `hasExplicitAccessibleName`: `<section>`/`<form>` are
/// landmarks only with an aria name.
fn has_explicit_name(dom: &Dom, id: NodeId) -> bool {
    attr(dom, id, "aria-label").is_some() || attr(dom, id, "aria-labelledby").is_some()
}

/// The elements an IDREF list names, in order, each once (Playwright's
/// `getIdRefs`), or `None` without the attribute.
fn id_refs(dom: &Dom, doc: NodeId, id: NodeId, name: &str) -> Option<Vec<NodeId>> {
    let refs = attr(dom, id, name)?;
    let mut out = Vec::new();
    for r in refs.split_ascii_whitespace() {
        if let Ok(Some(n)) = dom.get_element_by_id(doc, r) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    Some(out)
}

/// Implicit ARIA role, by Playwright's `kImplicitRoleByTagName`, or the
/// first token of an explicit `role`.
fn role_of(dom: &Dom, id: NodeId) -> Option<String> {
    if let Some(role) = attr(dom, id, "role").and_then(|r| r.split_whitespace().next()) {
        return Some(role.to_ascii_lowercase());
    }
    let ty = || attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
    // `<header>`/`<footer>` are landmarks only outside sectioning content.
    let in_sectioning = || {
        self_and_ancestors(dom, id).skip(1).any(|a| {
            (matches!(local(dom, a), Some("article" | "aside" | "main" | "nav" | "section"))
                && attr(dom, a, "role").is_none())
                || matches!(
                    attr(dom, a, "role").map(|r| r.to_ascii_lowercase()).as_deref(),
                    Some("article" | "complementary" | "main" | "navigation" | "region")
                )
        })
    };
    let role = match local(dom, id)? {
        "a" | "area" => {
            if attr(dom, id, "href").is_some() {
                "link"
            } else {
                return None;
            }
        }
        "address" | "hgroup" => "group",
        "article" => "article",
        "aside" => "complementary",
        "blockquote" => "blockquote",
        "button" => "button",
        "caption" => "caption",
        "code" => "code",
        "datalist" => "listbox",
        "dd" => "definition",
        "del" => "deletion",
        "details" => "group",
        "dfn" | "dt" => "term",
        "dialog" => "dialog",
        "em" => "emphasis",
        "fieldset" => "group",
        "figure" => "figure",
        "footer" if in_sectioning() => return None,
        "footer" => "contentinfo",
        "form" if has_explicit_name(dom, id) => "form",
        "form" => return None,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
        "header" if in_sectioning() => return None,
        "header" => "banner",
        "hr" => "separator",
        "html" => "document",
        "img" => {
            let el = dom.element(id).ok()?;
            let global_aria = el.attrs.iter().any(|a| a.name.local.starts_with("aria-"));
            if attr(dom, id, "alt") == Some("")
                && attr(dom, id, "title").is_none()
                && !global_aria
                && attr(dom, id, "tabindex").is_none()
            {
                "presentation"
            } else {
                "img"
            }
        }
        "input" => match ty().as_str() {
            "search" if attr(dom, id, "list").is_some() => "combobox",
            "search" => "searchbox",
            "email" | "tel" | "text" | "url" | "" => {
                let doc = dom.root_node(id, true).unwrap_or(id);
                let list = id_refs(dom, doc, id, "list").and_then(|l| l.first().copied());
                if list.is_some_and(|l| local(dom, l) == Some("datalist")) {
                    "combobox"
                } else {
                    "textbox"
                }
            }
            "hidden" => return None,
            "button" | "submit" | "reset" | "image" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "number" => "spinbutton",
            "file" | "color" | "date" | "datetime-local" | "month" | "time" | "week" => return None,
            _ => "textbox",
        },
        "ins" => "insertion",
        "li" => "listitem",
        "main" => "main",
        "mark" => "mark",
        "math" => "math",
        "menu" | "ol" | "ul" => "list",
        "meter" => "meter",
        "nav" => "navigation",
        "optgroup" => "group",
        "option" => "option",
        "output" => "status",
        "p" => "paragraph",
        "progress" => "progressbar",
        "section" if has_explicit_name(dom, id) => "region",
        "section" => return None,
        "search" => "search",
        "select" => {
            let sized = attr(dom, id, "size").is_some_and(|s| s.trim().parse::<u32>().is_ok_and(|n| n > 1));
            if attr(dom, id, "multiple").is_some() || sized {
                "listbox"
            } else {
                "combobox"
            }
        }
        "strong" => "strong",
        "sub" => "subscript",
        "sup" => "superscript",
        "table" => "table",
        "tbody" | "tfoot" | "thead" => "rowgroup",
        "td" => {
            let table = self_and_ancestors(dom, id).find(|a| local(dom, *a) == Some("table"));
            let table_role = table.and_then(|t| attr(dom, t, "role")).map(|r| r.to_ascii_lowercase());
            if matches!(table_role.as_deref(), Some("grid" | "treegrid")) {
                "gridcell"
            } else {
                "cell"
            }
        }
        "textarea" => "textbox",
        "th" => {
            if attr(dom, id, "scope")
                .is_some_and(|s| s.eq_ignore_ascii_case("row") || s.eq_ignore_ascii_case("rowgroup"))
            {
                "rowheader"
            } else {
                "columnheader"
            }
        }
        "time" => "time",
        "tr" => "row",
        _ => return None,
    };
    Some(role.into())
}

/// Playwright's `allowsNameFromContent`: roles always named from content,
/// and (for descendants of the named element) the generic ones Chromium
/// reads through.
fn allows_name_from_content(role: &str, descendant: bool) -> bool {
    let always = matches!(
        role,
        "button"
            | "cell"
            | "checkbox"
            | "columnheader"
            | "gridcell"
            | "heading"
            | "link"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "option"
            | "radio"
            | "row"
            | "rowheader"
            | "switch"
            | "tab"
            | "tooltip"
            | "treeitem"
    );
    always
        || (descendant
            && matches!(
                role,
                "" | "caption"
                    | "code"
                    | "contentinfo"
                    | "definition"
                    | "deletion"
                    | "emphasis"
                    | "insertion"
                    | "list"
                    | "listitem"
                    | "mark"
                    | "none"
                    | "paragraph"
                    | "presentation"
                    | "region"
                    | "row"
                    | "rowgroup"
                    | "section"
                    | "strong"
                    | "subscript"
                    | "superscript"
                    | "table"
                    | "term"
                    | "time"
            ))
}

/// Labelable elements (HTML `labels`).
fn labelable(dom: &Dom, id: NodeId) -> bool {
    match local(dom, id) {
        Some("input") => !attr(dom, id, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden")),
        Some("button" | "meter" | "output" | "progress" | "select" | "textarea") => true,
        _ => false,
    }
}

/// The `<label>`s of a form control: `label[for=id]` in document order and
/// the nearest enclosing label without `for`.
fn labels(dom: &Dom, doc: NodeId, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    if !labelable(dom, id) {
        return out;
    }
    if let Some(own) = attr(dom, id, "id").filter(|v| !v.is_empty()) {
        for label in elements(dom, doc) {
            if local(dom, label) == Some("label") && attr(dom, label, "for") == Some(own) {
                out.push(label);
            }
        }
    }
    if let Some(p) = self_and_ancestors(dom, id).skip(1).find(|p| local(dom, *p) == Some("label")) {
        if attr(dom, p, "for").is_none() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Text of the `<label>`s of a form control, for the `label` locator.
fn label_text(dom: &Dom, doc: NodeId, id: NodeId) -> Option<String> {
    let parts: Vec<String> = labels(dom, doc, id).into_iter().map(|l| text_of(dom, l)).collect();
    let joined = normalize(&parts.join(" "));
    (!joined.is_empty()).then_some(joined)
}

/// Playwright's `isElementHiddenForAria`: scripts and templates, elements
/// not rendered (`display:none`, `visibility:hidden`), and anything inside
/// an `aria-hidden="true"` or `inert` subtree. `ancestors` walks up for the
/// subtree rules (a descendant traversal has already passed its parents).
fn hidden_for_aria(dom: &Dom, layout: &Layout, id: NodeId, ancestors: bool) -> bool {
    if matches!(local(dom, id), Some("style" | "script" | "noscript" | "template")) {
        return true;
    }
    let in_select =
        || local(dom, id) == Some("option") && self_and_ancestors(dom, id).any(|a| local(dom, a) == Some("select"));
    let style = layout.style();
    if ancestors {
        if !layout.is_visible(dom, id) {
            return true;
        }
    } else if style.has_display_none(dom, id) {
        return true;
    }
    if style.has_visibility_hidden_inherited(dom, id) && !in_select() {
        return true;
    }
    let mut chain = self_and_ancestors(dom, id);
    let hidden = |n: NodeId| aria_true(dom, n, "aria-hidden") || attr(dom, n, "inert").is_some();
    if ancestors {
        chain.any(hidden)
    } else {
        chain.next().is_some_and(hidden)
    }
}

/// How the element being named relates to the one whose text alternative
/// is computed (Playwright's `embeddedInTargetElement`).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Target {
    #[default]
    Other,
    Own,
    Descendant,
}

/// Traversal state of accname (Playwright's `AccessibleNameOptions`).
#[derive(Clone, Copy, Default)]
struct NameOpts {
    target: Target,
    /// Inside an `aria-labelledby` reference (the reference itself).
    labelledby: Option<NodeId>,
    /// Inside a `<label>` (the label itself).
    label: Option<NodeId>,
    /// Inside a native alternative (`<caption>`, `<legend>`, `<figcaption>`).
    native: bool,
    /// The reference being traversed is hidden: still read it (2A).
    allow_hidden: bool,
}

impl NameOpts {
    fn embedded(&self) -> bool {
        self.labelledby.is_some() || self.label.is_some() || self.native
    }
}

/// Accessible name computation (accname 1.2 as Playwright's
/// `getTextAlternativeInternal` implements it).
struct Names<'a> {
    dom: &'a Dom,
    layout: &'a Layout,
    doc: NodeId,
    visited: RefCell<HashSet<NodeId>>,
}

impl Names<'_> {
    fn alternative(&self, id: NodeId, opts: NameOpts) -> String {
        let dom = self.dom;
        if self.visited.borrow().contains(&id) {
            return String::new();
        }
        let child_opts =
            NameOpts { target: if opts.target == Target::Own { Target::Descendant } else { opts.target }, ..opts };
        // 2A: hidden, not referenced.
        if hidden_for_aria(dom, self.layout, id, opts.target != Target::Descendant && !opts.embedded())
            && !opts.allow_hidden
            || matches!(local(dom, id), Some("style" | "script" | "noscript" | "template"))
        {
            self.visited.borrow_mut().insert(id);
            return String::new();
        }
        // Playwright's `getAriaLabelledByElements`: no resolvable IDREF is no
        // labelledby at all.
        let labelledby = id_refs(dom, self.doc, id, "aria-labelledby").filter(|r| !r.is_empty());
        // 2B: labelledby.
        if opts.labelledby.is_none() {
            if let Some(refs) = &labelledby {
                let parts: Vec<String> = refs
                    .iter()
                    .map(|r| {
                        let o = NameOpts {
                            labelledby: Some(*r),
                            allow_hidden: hidden_for_aria(dom, self.layout, *r, true),
                            ..NameOpts::default()
                        };
                        self.alternative(*r, o)
                    })
                    .collect();
                let name = parts.join(" ");
                if !name.trim().is_empty() {
                    return name;
                }
            }
        }
        let role = role_of(dom, id).unwrap_or_default();
        let tag = local(dom, id).unwrap_or("");
        // 2C: an embedded control contributes its value (not the control
        // whose own label or reference this is).
        if opts.label.is_some() || opts.labelledby.is_some() || opts.target == Target::Descendant {
            let own_label = opts.label.is_some_and(|l| labels(dom, self.doc, id).contains(&l));
            let own_ref = opts.labelledby.is_some_and(|r| labelledby.as_ref().is_some_and(|refs| refs.contains(&r)));
            if !own_label && !own_ref {
                match role.as_str() {
                    "textbox" => {
                        self.visited.borrow_mut().insert(id);
                        return match tag {
                            "input" | "textarea" => dom.value(id).unwrap_or_default(),
                            _ => text_of(dom, id),
                        };
                    }
                    "combobox" | "listbox" => {
                        self.visited.borrow_mut().insert(id);
                        let selected = if tag == "select" {
                            let mut s = dom.selected_options(id).unwrap_or_default();
                            if s.is_empty() {
                                s.extend(elements(dom, id).find(|o| local(dom, *o) == Some("option")));
                            }
                            s
                        } else {
                            let listbox = if role == "combobox" {
                                self.aria_owned(id).into_iter().find(|l| role_of(dom, *l).as_deref() == Some("listbox"))
                            } else {
                                Some(id)
                            };
                            listbox
                                .map(|l| {
                                    self.aria_owned(l)
                                        .into_iter()
                                        .filter(|o| {
                                            attr(dom, *o, "aria-selected") == Some("true")
                                                && role_of(dom, *o).as_deref() == Some("option")
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        if selected.is_empty() && tag == "input" {
                            return dom.value(id).unwrap_or_default();
                        }
                        let parts: Vec<String> =
                            selected.into_iter().map(|o| self.alternative(o, child_opts)).collect();
                        return parts.join(" ");
                    }
                    "progressbar" | "scrollbar" | "slider" | "spinbutton" | "meter" => {
                        self.visited.borrow_mut().insert(id);
                        return attr(dom, id, "aria-valuetext")
                            .or_else(|| attr(dom, id, "aria-valuenow"))
                            .or_else(|| attr(dom, id, "value"))
                            .unwrap_or_default()
                            .to_owned();
                    }
                    "menu" => {
                        self.visited.borrow_mut().insert(id);
                        return String::new();
                    }
                    _ => {}
                }
            }
        }
        // 2D: aria-label.
        if let Some(label) = attr(dom, id, "aria-label").filter(|l| !l.trim().is_empty()) {
            self.visited.borrow_mut().insert(id);
            return label.to_owned();
        }
        // 2E: native text alternatives.
        if !matches!(role.as_str(), "presentation" | "none") {
            if let Some(name) = self.native_alternative(id, tag, labelledby.is_some(), opts, child_opts) {
                self.visited.borrow_mut().insert(id);
                return name;
            }
        }
        // 2F, 2H: content.
        let summary = tag == "summary" && !matches!(role.as_str(), "presentation" | "none");
        if allows_name_from_content(&role, opts.target == Target::Descendant) || summary || opts.embedded() {
            self.visited.borrow_mut().insert(id);
            let name = self.inner_text(id, child_opts);
            if !name.trim().is_empty() {
                return name;
            }
        }
        // 2I: title.
        self.visited.borrow_mut().insert(id);
        if !matches!(role.as_str(), "presentation" | "none") || tag == "iframe" {
            if let Some(title) = attr(dom, id, "title").filter(|t| !t.trim().is_empty()) {
                return title.to_owned();
            }
        }
        String::new()
    }

    /// Step 2E for the HTML elements with their own naming rules; `None`
    /// falls through to content and title.
    fn native_alternative(
        &self,
        id: NodeId,
        tag: &str,
        has_labelledby: bool,
        opts: NameOpts,
        child_opts: NameOpts,
    ) -> Option<String> {
        let dom = self.dom;
        let attr_text = |name: &str| attr(dom, id, name).filter(|v| !v.trim().is_empty()).map(str::to_owned);
        let from_labels = || {
            let parts: Vec<String> = labels(dom, self.doc, id)
                .into_iter()
                .map(|l| {
                    let o = NameOpts {
                        label: Some(l),
                        allow_hidden: hidden_for_aria(dom, self.layout, l, true),
                        ..NameOpts::default()
                    };
                    self.alternative(l, o)
                })
                .filter(|s| !s.is_empty())
                .collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        };
        let child_named = |child: &str| {
            let c = dom.children(id).into_iter().flatten().find(|c| local(dom, *c) == Some(child))?;
            let o = NameOpts { native: true, allow_hidden: hidden_for_aria(dom, self.layout, c, true), ..child_opts };
            let name = self.alternative(c, o);
            (!name.trim().is_empty()).then_some(name)
        };
        match tag {
            "input" => {
                let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
                match ty.as_str() {
                    "button" | "submit" | "reset" => {
                        return Some(attr_text("value").unwrap_or_else(|| match ty.as_str() {
                            "submit" => "Submit".into(),
                            "reset" => "Reset".into(),
                            _ => attr_text("title").unwrap_or_default(),
                        }));
                    }
                    "image" => {
                        if opts.labelledby.is_none() {
                            if let Some(name) = from_labels() {
                                return Some(name);
                            }
                        }
                        return Some(
                            attr_text("alt").or_else(|| attr_text("title")).unwrap_or_else(|| "Submit".into()),
                        );
                    }
                    _ => {}
                }
                if has_labelledby {
                    return None;
                }
                if let Some(name) = from_labels() {
                    return Some(name);
                }
                let placeholder = matches!(ty.as_str(), "text" | "password" | "search" | "tel" | "email" | "url" | "");
                Some(
                    attr_text("title")
                        .or_else(|| placeholder.then(|| attr_text("placeholder")).flatten())
                        .unwrap_or_default(),
                )
            }
            "button" if !has_labelledby => from_labels(),
            "output" if !has_labelledby => Some(from_labels().or_else(|| attr_text("title")).unwrap_or_default()),
            "textarea" if !has_labelledby => Some(
                from_labels().or_else(|| attr_text("title")).or_else(|| attr_text("placeholder")).unwrap_or_default(),
            ),
            "select" | "meter" | "progress" if !has_labelledby => {
                Some(from_labels().or_else(|| attr_text("title")).unwrap_or_default())
            }
            "fieldset" if !has_labelledby => {
                Some(child_named("legend").or_else(|| attr_text("title")).unwrap_or_default())
            }
            "figure" if !has_labelledby => {
                Some(child_named("figcaption").or_else(|| attr_text("title")).unwrap_or_default())
            }
            "img" => attr_text("alt"),
            "table" => child_named("caption").or_else(|| attr_text("summary")),
            "area" => Some(attr_text("alt").or_else(|| attr_text("title")).unwrap_or_default()),
            _ => {
                let svg = dom.element(id).ok().is_some_and(|e| e.namespace() == "http://www.w3.org/2000/svg");
                if svg {
                    let title = dom
                        .children(id)
                        .into_iter()
                        .flatten()
                        .find(|c| dom.element(*c).ok().is_some_and(|e| &*e.name.local == "title"))?;
                    return Some(text_of(dom, title));
                }
                None
            }
        }
    }

    /// Playwright's `queryInAriaOwned(el, '*')`: the descendants of `id` and
    /// the elements it `aria-owns` (with theirs).
    fn aria_owned(&self, id: NodeId) -> Vec<NodeId> {
        let dom = self.dom;
        let mut out: Vec<NodeId> = elements(dom, id).skip(1).collect();
        for owned in id_refs(dom, self.doc, id, "aria-owns").unwrap_or_default() {
            out.extend(elements(dom, owned));
        }
        out
    }

    /// Playwright's `innerAccumulatedElementText`: child text with a space
    /// around every child that is not `display:inline` (and `<br>`).
    fn inner_text(&self, id: NodeId, opts: NameOpts) -> String {
        let dom = self.dom;
        let mut out = String::new();
        for c in dom.children(id).into_iter().flatten() {
            match dom.node(c).map(|n| &n.kind) {
                Ok(NodeKind::Text(t)) => out.push_str(t),
                Ok(NodeKind::Element(_)) => {
                    let display = self.layout.computed_style_value(dom, c, "display");
                    let token = self.alternative(c, opts);
                    if (!display.is_empty() && display != "inline") || local(dom, c) == Some("br") {
                        out.push(' ');
                        out.push_str(&token);
                        out.push(' ');
                    } else {
                        out.push_str(&token);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// The accessible name of an element, whitespace-normalized.
fn accessible_name(dom: &Dom, layout: &Layout, doc: NodeId, id: NodeId) -> String {
    // Playwright's `getElementAccessibleName`: these roles prohibit naming.
    let prohibits = matches!(
        role_of(dom, id).as_deref(),
        Some(
            "caption"
                | "code"
                | "definition"
                | "deletion"
                | "emphasis"
                | "generic"
                | "insertion"
                | "mark"
                | "paragraph"
                | "presentation"
                | "strong"
                | "subscript"
                | "suggestion"
                | "superscript"
                | "term"
                | "time"
        )
    );
    if prohibits {
        return String::new();
    }
    let names = Names { dom, layout, doc, visited: RefCell::new(HashSet::new()) };
    normalize(&names.alternative(id, NameOpts { target: Target::Own, ..NameOpts::default() }))
}

fn skipped_for_text(dom: &Dom, id: NodeId) -> bool {
    matches!(local(dom, id), Some("script" | "style" | "noscript" | "template" | "head" | "title"))
}

/// Runs of the element's own text nodes (Playwright's `immediate` text):
/// `<span><span>|</span>TS-1</span>` has `["TS-1"]`.
fn immediate_texts(dom: &Dom, id: NodeId) -> Vec<String> {
    let mut runs = Vec::new();
    let mut run = String::new();
    for c in dom.children(id).into_iter().flatten() {
        match dom.node(c).map(|n| &n.kind) {
            Ok(NodeKind::Text(t) | NodeKind::CData(t)) => run.push_str(t),
            Ok(NodeKind::Comment(_)) => {}
            _ => {
                if !run.is_empty() {
                    runs.push(std::mem::take(&mut run));
                }
            }
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// Whether an element's text matches: its whole text, or, for an exact
/// string, one run of its own text nodes (Playwright's `text="..."`, so
/// exact `TS-1` matches `<span><span>|</span>TS-1</span>`).
fn element_text_matches(dom: &Dom, id: NodeId, want: &Pattern, exact: bool) -> bool {
    if want.matches(&text_of(dom, id), exact) {
        return true;
    }
    exact && want.regex.is_none() && immediate_texts(dom, id).iter().any(|run| normalize(run) == normalize(&want.text))
}

/// The smallest elements whose text matches: an element whose text matches
/// and none of whose child elements' text does. `root` itself is a candidate.
fn by_text(dom: &Dom, root: NodeId, want: &Pattern, exact: bool, out: &mut Vec<NodeId>) -> bool {
    let children: Vec<NodeId> = dom
        .children(root)
        .into_iter()
        .flatten()
        .filter(|c| dom.element(*c).is_ok() && !skipped_for_text(dom, *c))
        .collect();
    let here = element_text_matches(dom, root, want, exact);
    // A substring match in a child implies one here: prune subtrees that
    // cannot match. Exact and regex matches need the whole walk.
    if !here && !exact && want.regex.is_none() {
        return false;
    }
    let before = out.len();
    let mut child_matched = false;
    for c in children {
        child_matched |= by_text(dom, c, want, exact, out);
    }
    if here && !child_matched && out.len() == before {
        out.push(root);
        return true;
    }
    child_matched || here
}

/// Drops nulls and `false` from a serialized locator, for messages.
fn prune(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null() && *v != Value::Bool(false))
                .map(|(k, v)| (k, prune(v)))
                .collect(),
        ),
        v => v,
    }
}

impl Locator {
    fn describe(&self) -> String {
        serde_json::to_value(self).map(prune).map(|v| v.to_string()).unwrap_or_default()
    }

    fn validate(&self) -> CdpResult<()> {
        let kinds = [
            self.css.is_some(),
            self.test_id.is_some(),
            self.text.is_some(),
            self.role.is_some(),
            self.label.is_some(),
            self.placeholder.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(CdpError::invalid_params(
                "locator needs exactly one of css, testId, text, role, label, placeholder",
            ));
        }
        if let Some(within) = &self.within {
            within.validate()?;
        }
        if let Some(f) = &self.filter {
            for l in [&f.has, &f.has_not].into_iter().flatten() {
                l.validate()?;
            }
        }
        Ok(())
    }

    fn resolve(&self, page: &PageDom, selectors: &RefCell<SelectorCache>) -> CdpResult<Vec<NodeId>> {
        self.resolve_at(page, selectors, page.document())
    }

    /// Matches inside `base` (the document, or the element a `has` filter tests).
    fn resolve_at(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, base: NodeId) -> CdpResult<Vec<NodeId>> {
        let roots = match &self.within {
            Some(outer) => outer.resolve_at(page, selectors, base)?,
            None => vec![base],
        };
        let mut found = Vec::new();
        if let [root] = roots[..] {
            found = self.matches_in(page, selectors, root)?;
        } else {
            let mut seen = HashSet::new();
            for root in roots {
                found.extend(self.matches_in(page, selectors, root)?.into_iter().filter(|n| seen.insert(*n)));
            }
        }
        if let Some(f) = &self.filter {
            let mut kept = Vec::with_capacity(found.len());
            for n in found {
                if f.keeps(page, selectors, n)? {
                    kept.push(n);
                }
            }
            found = kept;
        }
        if let Some(nth) = self.nth {
            let len = found.len() as i64;
            let i = if nth < 0 { len + nth } else { nth };
            return Ok(if (0..len).contains(&i) { vec![found[i as usize]] } else { Vec::new() });
        }
        Ok(found)
    }

    /// Matches strictly inside `root` (the document, or a `within` match).
    fn matches_in(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, root: NodeId) -> CdpResult<Vec<NodeId>> {
        let dom = page.dom.borrow();
        let doc = page.document();
        let inside = |n: &NodeId| *n != root;
        let mut found = Vec::new();
        if let Some(css) = &self.css {
            let sel = selectors.borrow_mut().get(css).map_err(|_| CdpError::invalid_params("invalid css selector"))?;
            found = dom.query_all(root, &sel).map_err(|e| CdpError::server(format!("{e:?}")))?;
        } else if let Some(id) = &self.test_id {
            let name = self.test_id_attribute.as_deref().unwrap_or("data-testid");
            found = elements(&dom, root).filter(inside).filter(|n| attr(&dom, *n, name) == Some(id.as_str())).collect();
        } else if let Some(text) = &self.text {
            // As Playwright's text engine, the scope element itself is a
            // candidate: `css('td').text('Domestic', exact)` matches the cell.
            if root == doc {
                if let Ok(Some(html)) = dom.document_element(doc) {
                    by_text(&dom, html, text, self.exact, &mut found);
                }
            } else if dom.element(root).is_ok() && !skipped_for_text(&dom, root) {
                by_text(&dom, root, text, self.exact, &mut found);
            }
        } else if let Some(role) = &self.role {
            let role = role.to_ascii_lowercase();
            let layout = page.layout.borrow();
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| role_of(&dom, *n).as_deref() == Some(role.as_str()))
                .filter(|n| !hidden_for_aria(&dom, &layout, *n, true))
                .filter(|n| match &self.name {
                    Some(want) => want.matches(&accessible_name(&dom, &layout, doc, *n), self.exact),
                    None => true,
                })
                .collect();
        } else if let Some(want) = &self.label {
            for n in elements(&dom, root).filter(inside) {
                let control = matches!(local(&dom, n), Some("input" | "textarea" | "select" | "button"));
                let labelled = attr(&dom, n, "aria-label").is_some_and(|l| want.matches(l, self.exact))
                    || (control && label_text(&dom, doc, n).is_some_and(|l| want.matches(&l, self.exact)));
                if labelled {
                    found.push(n);
                }
            }
        } else if let Some(want) = &self.placeholder {
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| matches!(local(&dom, *n), Some("input" | "textarea")))
                .filter(|n| attr(&dom, *n, "placeholder").is_some_and(|p| want.matches(p, self.exact)))
                .collect();
        }
        Ok(found)
    }
}

impl Filter {
    fn keeps(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, n: NodeId) -> CdpResult<bool> {
        if let Some(want) = self.visible {
            if visible_box(page, n).is_some() != want {
                return Ok(false);
            }
        }
        if self.has_text.is_some() || self.has_not_text.is_some() {
            let text = text_of(&page.dom.borrow(), n);
            if self.has_text.as_ref().is_some_and(|t| !t.matches(&text, false))
                || self.has_not_text.as_ref().is_some_and(|t| t.matches(&text, false))
            {
                return Ok(false);
            }
        }
        for (inner, want) in [(&self.has, true), (&self.has_not, false)] {
            if let Some(inner) = inner {
                if inner.resolve_at(page, selectors, n)?.is_empty() == want {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// The one element a strict locator names, or why there is none yet.
fn single(found: &[NodeId], dom: &Dom, locator: &Locator) -> Result<Option<NodeId>, CdpError> {
    match found {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        many => {
            let names: Vec<String> = many
                .iter()
                .take(5)
                .map(|n| {
                    let tag = local(dom, *n).unwrap_or("?");
                    let text = normalize(&text_of(dom, *n));
                    let text: String = text.chars().take(40).collect();
                    format!("<{tag}> {text:?}")
                })
                .collect();
            Err(CdpError::server(format!(
                "strict mode violation: {} matched {} elements: {}",
                locator.describe(),
                many.len(),
                names.join(", ")
            )))
        }
    }
}

fn fillable(dom: &Dom, id: NodeId) -> bool {
    match local(dom, id) {
        Some("textarea") => attr(dom, id, "readonly").is_none(),
        Some("input") => {
            let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
            matches!(ty.as_str(), "" | "text" | "email" | "password" | "search" | "tel" | "url" | "number")
                && attr(dom, id, "readonly").is_none()
        }
        _ => attr(dom, id, "contenteditable").is_some_and(|v| !v.eq_ignore_ascii_case("false")),
    }
}

fn disabled(dom: &Dom, id: NodeId) -> bool {
    dom.is_disabled(id).unwrap_or(false) || attr(dom, id, "aria-disabled") == Some("true")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Checked {
    Yes,
    No,
    Mixed,
}

impl Checked {
    fn to_json(self) -> Value {
        match self {
            Checked::Yes => json!(true),
            Checked::No => json!(false),
            Checked::Mixed => json!("mixed"),
        }
    }
}

/// Playwright's `getChecked`: the checkedness of a native checkbox/radio,
/// else `aria-checked` on the roles that take it (`mixed` only when
/// `allow_mixed`); `None` for anything else.
fn checked_state(dom: &Dom, id: NodeId, allow_mixed: bool) -> Option<Checked> {
    let native = local(dom, id) == Some("input")
        && attr(dom, id, "type").is_some_and(|t| t.eq_ignore_ascii_case("checkbox") || t.eq_ignore_ascii_case("radio"));
    let indeterminate = || dom.control_state(id).ok().flatten().is_some_and(|c| c.indeterminate);
    if allow_mixed && local(dom, id) == Some("input") && indeterminate() {
        return Some(Checked::Mixed);
    }
    if native {
        return Some(if dom.checked(id).unwrap_or(false) { Checked::Yes } else { Checked::No });
    }
    let role = role_of(dom, id)?;
    if !matches!(
        role.as_str(),
        "checkbox" | "menuitemcheckbox" | "option" | "radio" | "switch" | "menuitemradio" | "treeitem"
    ) {
        return None;
    }
    Some(match attr(dom, id, "aria-checked") {
        Some("true") => Checked::Yes,
        Some("mixed") if allow_mixed => Checked::Mixed,
        _ => Checked::No,
    })
}

fn contains(dom: &Dom, ancestor: NodeId, mut node: NodeId) -> bool {
    loop {
        if node == ancestor {
            return true;
        }
        match dom.parent(node).ok().flatten() {
            Some(p) => node = p,
            None => return false,
        }
    }
}

/// Visible box with area, in viewport coordinates.
fn visible_box(page: &PageDom, id: NodeId) -> Option<lp_dom::layout::Rect> {
    let dom = page.dom.borrow();
    page.layout.borrow().client_rect(&dom, id).filter(|r| r.width > 0.0 && r.height > 0.0)
}

fn act(page: &Rc<PageDom>, id: NodeId, action: Action, value: &str, opts: PointerOpts) -> Attempt {
    {
        let dom = page.dom.borrow();
        if dom.root_node(id, true).ok() != Some(page.document()) {
            return Attempt::Retry("element is detached".into());
        }
    }
    if visible_box(page, id).is_none() {
        return Attempt::Retry("element is not visible".into());
    }
    if action.needs_enabled() && !opts.force && disabled(&page.dom.borrow(), id) {
        return Attempt::Retry("element is disabled".into());
    }
    match action {
        Action::Check | Action::Uncheck => {
            let Some(checked) = checked_state(&page.dom.borrow(), id, false) else {
                return Attempt::Retry("element is not a checkbox or radio button".into());
            };
            if (checked == Checked::Yes) == (action == Action::Check) {
                return Attempt::Done(json!({}));
            }
            click(page, id, opts)
        }
        Action::Click => click(page, id, opts),
        Action::Press => {
            let Some(keys) = key_chord(value) else {
                return Attempt::Retry(format!("unknown key {value:?}"));
            };
            if input::key_target(page) != Some(id) {
                input::focus_element(page, id);
            }
            for (kind, k) in keys {
                input::dispatch_key(page, kind, &k);
            }
            Attempt::Done(json!({}))
        }
        Action::Focus => {
            input::focus_element(page, id);
            Attempt::Done(json!({}))
        }
        Action::Blur => {
            input::blur_element(page, id);
            Attempt::Done(json!({}))
        }
        Action::Hover => pointer(page, id, Pointer::Hover, opts),
        Action::DblClick => pointer(page, id, Pointer::DblClick, opts),
        Action::Wheel => pointer(page, id, Pointer::Wheel, opts),
        Action::Upload | Action::Drag => unreachable!("handled by the host"),
        Action::Type => {
            if input::key_target(page) != Some(id) {
                input::focus_element(page, id);
            }
            for c in value.chars() {
                let name = c.to_string();
                let Some(keys) = key_chord(&name) else {
                    return Attempt::Retry(format!("cannot type {c:?}"));
                };
                for (kind, k) in keys {
                    input::dispatch_key(page, kind, &k);
                }
            }
            Attempt::Done(json!({}))
        }
        Action::Fill => {
            let is_control = {
                let dom = page.dom.borrow();
                if !fillable(&dom, id) {
                    return Attempt::Retry("element is not editable".into());
                }
                matches!(local(&dom, id), Some("input" | "textarea"))
            };
            input::focus_element(page, id);
            if is_control {
                let _ = page.dom.borrow_mut().set_selection_range(id, 0, u32::MAX);
            } else {
                lp_webapi::editing::select_all(page, id);
            }
            // Empty text deletes the selection (Input.insertText semantics).
            input::insert_text(page, value);
            Attempt::Done(json!({}))
        }
    }
}

/// `key`, `code` and Windows key code of a key name (`Enter`, `ArrowDown`,
/// `F5`, `a`, ...).
fn key_def(name: &str) -> Option<(String, String, u32)> {
    let named = |code: &str, kc: u32| Some((name.to_owned(), code.to_owned(), kc));
    match name {
        "Enter" => named("Enter", 13),
        "Tab" => named("Tab", 9),
        "Escape" => named("Escape", 27),
        "Backspace" => named("Backspace", 8),
        "Delete" => named("Delete", 46),
        "Space" | " " => Some((" ".into(), "Space".into(), 32)),
        "ArrowLeft" => named("ArrowLeft", 37),
        "ArrowUp" => named("ArrowUp", 38),
        "ArrowRight" => named("ArrowRight", 39),
        "ArrowDown" => named("ArrowDown", 40),
        "Home" => named("Home", 36),
        "End" => named("End", 35),
        "PageUp" => named("PageUp", 33),
        "PageDown" => named("PageDown", 34),
        "Shift" => named("ShiftLeft", 16),
        "Control" => named("ControlLeft", 17),
        "Alt" => named("AltLeft", 18),
        "Meta" => named("MetaLeft", 91),
        _ => {
            if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n))
            {
                return named(name, 111 + n);
            }
            let mut chars = name.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            let upper = c.to_ascii_uppercase();
            let (code, kc) = if c.is_ascii_alphabetic() {
                (format!("Key{upper}"), upper as u32)
            } else if c.is_ascii_digit() {
                (format!("Digit{c}"), c as u32)
            } else {
                (String::new(), 0)
            };
            Some((name.to_owned(), code, kc))
        }
    }
}

/// The key events of a press: modifiers down, the key down and up,
/// modifiers up. `ControlOrMeta` is Meta on macOS.
fn key_chord(chord: &str) -> Option<Vec<(input::KeyKind, input::KeyInput)>> {
    let parts: Vec<&str> = if chord == "+" { vec!["+"] } else { chord.split('+').collect() };
    let (key, mods) = parts.split_last()?;
    let mut modifiers = input::Modifiers::default();
    let mut held = Vec::new();
    for m in mods {
        let m = match *m {
            "ControlOrMeta" if cfg!(target_os = "macos") => "Meta",
            "ControlOrMeta" => "Control",
            m => m,
        };
        match m {
            "Shift" => modifiers.shift = true,
            "Control" => modifiers.ctrl = true,
            "Alt" => modifiers.alt = true,
            "Meta" => modifiers.meta = true,
            _ => return None,
        }
        let (k, code, key_code) = key_def(m)?;
        held.push(input::KeyInput { key: k, code, modifiers, key_code, ..Default::default() });
    }
    let (mut k, code, key_code) = key_def(key)?;
    if modifiers.shift && k.chars().count() == 1 {
        k = k.to_uppercase();
    }
    let text = match k.as_str() {
        _ if modifiers.ctrl || modifiers.meta || modifiers.alt => String::new(),
        "Enter" => "\r".into(),
        _ if k.chars().count() == 1 => k.clone(),
        _ => String::new(),
    };
    let main = input::KeyInput { key: k, code, text, modifiers, key_code, ..Default::default() };
    let mut events: Vec<_> = held.iter().map(|k| (input::KeyKind::RawKeyDown, k.clone())).collect();
    let down = if main.text.is_empty() { input::KeyKind::RawKeyDown } else { input::KeyKind::KeyDown };
    events.push((down, main.clone()));
    events.push((input::KeyKind::KeyUp, main));
    events.extend(held.into_iter().rev().map(|k| (input::KeyKind::KeyUp, k)));
    Some(events)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Click,
    DblClick,
    Hover,
    Wheel,
}

/// Scrolls the element into view, checks that its center receives pointer
/// events, then dispatches move, press and release there.
fn click(page: &Rc<PageDom>, id: NodeId, opts: PointerOpts) -> Attempt {
    pointer(page, id, Pointer::Click, opts)
}

/// The `buttons` bit of a button while it is held.
fn buttons_mask(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        MouseButton::Back => 8,
        MouseButton::Forward => 16,
        MouseButton::None => 0,
    }
}

/// The part of the element's box inside the viewport and inside each
/// overflow-clipping ancestor (what Playwright aims at: the center of the
/// visible part, not of the whole box), or `None` when nothing shows.
fn clipped_box(page: &PageDom, id: NodeId, rect: lp_dom::layout::Rect) -> Option<lp_dom::layout::Rect> {
    let dom = page.dom.borrow();
    let layout = page.layout.borrow();
    let (mut left, mut top) = (rect.left().max(0.0), rect.top().max(0.0));
    let (mut right, mut bottom) =
        (rect.right().min(layout.inner_width() as f64), rect.bottom().min(layout.inner_height() as f64));
    let mut cur = dom.parent_element(id).ok().flatten();
    while let Some(a) = cur {
        if layout.is_root_container(&dom, a) {
            break;
        }
        let clips = layout.style().overflow_clip_axes(&dom, a);
        if clips.x || clips.y {
            if let Some(r) = layout.client_rect(&dom, a) {
                if clips.x {
                    left = left.max(r.left());
                    right = right.min(r.right());
                }
                if clips.y {
                    top = top.max(r.top());
                    bottom = bottom.min(r.bottom());
                }
            }
        }
        cur = dom.parent_element(a).ok().flatten();
    }
    (right > left && bottom > top).then_some(lp_dom::layout::Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// Where a pointer action lands, once the element is revealed: `position`
/// from its top-left, else the center of its visible part; checked to
/// receive pointer events there unless `force`.
fn pointer_target(page: &Rc<PageDom>, id: NodeId, opts: PointerOpts) -> Result<(f64, f64), Attempt> {
    super::dom::reveal(page, id);
    let Some(rect) = visible_box(page, id) else { return Err(Attempt::Retry("element is not visible".into())) };
    let (x, y) = match opts.position {
        Some((dx, dy)) => (rect.x + dx, rect.y + dy),
        None => {
            let Some(shown) = clipped_box(page, id, rect) else {
                return Err(Attempt::Retry("element is outside the viewport".into()));
            };
            (shown.x + shown.width / 2.0, shown.y + shown.height / 2.0)
        }
    };
    if opts.force {
        return Ok((x, y));
    }
    match lp_webapi::layout_api::hit_test(page, x, y) {
        Some(hit) if contains(&page.dom.borrow(), id, hit) => Ok((x, y)),
        Some(hit) => {
            let tag = local(&page.dom.borrow(), hit).unwrap_or("?").to_owned();
            Err(Attempt::Retry(format!("<{tag}> intercepts pointer events")))
        }
        None => Err(Attempt::Retry("element is outside the viewport".into())),
    }
}

/// The mouse events of a pointer action: kind, button, click count.
fn pointer_events(what: Pointer, button: MouseButton) -> Vec<(MouseKind, MouseButton, u32)> {
    let press = |count| [(MouseKind::Pressed, button, count), (MouseKind::Released, button, count)];
    let mut events = vec![(MouseKind::Moved, MouseButton::None, 0)];
    match what {
        Pointer::Hover => {}
        Pointer::Wheel => events.push((MouseKind::Wheel, MouseButton::None, 0)),
        Pointer::Click => events.extend(press(1)),
        Pointer::DblClick => {
            events.extend(press(1));
            events.extend(press(2));
        }
    }
    events
}

fn pointer(page: &Rc<PageDom>, id: NodeId, what: Pointer, opts: PointerOpts) -> Attempt {
    let (x, y) = match pointer_target(page, id, opts) {
        Ok(p) => p,
        Err(retry) => return retry,
    };
    let m = MouseInput { x, y, delta_x: opts.delta.0, delta_y: opts.delta.1, ..MouseInput::default() };
    // Hover-dependent layout (a tooltip the move hid, a menu it opened) can
    // shift the element between events: a move or press goes where it is
    // then; a release or wheel where the pointer is.
    let events = pointer_events(what, opts.button).into_iter().map(|(kind, button, count)| {
        let at = if matches!(kind, MouseKind::Moved | MouseKind::Pressed) { At::Element } else { At::Here };
        let buttons = if kind == MouseKind::Pressed { buttons_mask(button) } else { 0 };
        Input::Mouse { kind, button, buttons, count, at }
    });
    start(Sequence::new(page, id, opts, m, events))
}

/// Dispatches a sequence's first event now; the rest follow one per
/// event-loop turn.
fn start(mut seq: Sequence) -> Attempt {
    match seq.advance() {
        Ok(Some(value)) => Attempt::Done(value),
        Ok(None) => Attempt::Continue(Box::new(seq)),
        Err(reason) => Attempt::Retry(reason),
    }
}

/// A `mouse` step: the events of `page.mouse.*` at viewport coordinates.
/// A move with `steps` walks from the current position in equal parts.
fn mouse_step(
    page: &Rc<PageDom>,
    action: MouseAction,
    x: Option<f64>,
    y: Option<f64>,
    opts: PointerOpts,
    steps: u32,
) -> Attempt {
    let (x0, y0) = input::pointer_position(page).unwrap_or((0.0, 0.0));
    let (x, y) = (x.unwrap_or(x0), y.unwrap_or(y0));
    let mut m = MouseInput { x, y, delta_x: opts.delta.0, delta_y: opts.delta.1, ..MouseInput::default() };
    // Buttons stay held between steps (`down`, `move`, `up` drags).
    m.buttons = input::pointer_buttons(page);
    let send = |m: &MouseInput, kind: MouseKind| input::dispatch_mouse(page, kind, m).map_err(Attempt::Retry);
    let events = match action {
        MouseAction::Move => {
            let steps = steps.max(1);
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                m.x = x0 + (x - x0) * t;
                m.y = y0 + (y - y0) * t;
                if let Err(retry) = send(&m, MouseKind::Moved) {
                    return retry;
                }
            }
            return Attempt::Done(json!({}));
        }
        MouseAction::Down => vec![(MouseKind::Pressed, opts.button, 1)],
        MouseAction::Up => vec![(MouseKind::Released, opts.button, 1)],
        MouseAction::Click => pointer_events(Pointer::Click, opts.button),
        MouseAction::DblClick => pointer_events(Pointer::DblClick, opts.button),
        MouseAction::Wheel => vec![(MouseKind::Wheel, MouseButton::None, 0)],
    };
    for (kind, button, count) in events {
        m.button = button;
        match kind {
            MouseKind::Pressed => m.buttons |= buttons_mask(button),
            MouseKind::Released => m.buttons &= !buttons_mask(button),
            _ => {}
        }
        m.click_count = count;
        if let Err(retry) = send(&m, kind) {
            return retry;
        }
    }
    Attempt::Done(json!({}))
}

/// Sets the files of an `<input type=file>`, then `input` and `change`.
fn upload(page: &Rc<PageDom>, id: NodeId, files: &[UploadFile]) -> Attempt {
    {
        let dom = page.dom.borrow();
        let is_file_input =
            local(&dom, id) == Some("input") && attr(&dom, id, "type").is_some_and(|t| t.eq_ignore_ascii_case("file"));
        if !is_file_input {
            return Attempt::Retry("element is not an <input type=file>".into());
        }
        if disabled(&dom, id) {
            return Attempt::Retry("element is disabled".into());
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64);
    let entries: Vec<lp_webapi::file::FileEntry> = files
        .iter()
        .map(|f| lp_webapi::file::FileEntry {
            name: f.name.clone(),
            mime: f.mime.clone(),
            bytes: bytes::Bytes::from(f.bytes.clone()),
            last_modified: now,
        })
        .collect();
    if let Err(e) = page.set_input_files(id, entries) {
        return Attempt::Retry(format!("{e:?}"));
    }
    input::fire_simple(page, id, "input", true, false);
    input::fire_simple(page, id, "change", true, false);
    Attempt::Done(json!({ "files": files.len() }))
}

/// Drags `from` onto `to`. A native drag source (draggable, link, image)
/// gets HTML drag and drop: dragstart on the move, then dragenter/dragover
/// and drop on the target. Anything else gets pointer events with the
/// button held, as pointer-based drag libraries expect.
fn drag(page: &Rc<PageDom>, from: NodeId, to: NodeId) -> Attempt {
    let (x0, y0) = match pointer_target(page, from, PointerOpts::default()) {
        Ok(p) => p,
        Err(retry) => return retry,
    };
    super::dom::reveal(page, to);
    let Some(target) = visible_box(page, to) else { return Attempt::Retry("drop target is not visible".into()) };
    let (x1, y1) = (target.x + target.width / 2.0, target.y + target.height / 2.0);
    let m = MouseInput { x: x0, y: y0, ..MouseInput::default() };
    let mouse = |kind, button, buttons, count, x, y| Input::Mouse { kind, button, buttons, count, at: At::Point(x, y) };
    // A first small move past the drag threshold, then the target. A move
    // that starts a native drag replaces the rest (`Sequence::advance`).
    let events = [
        mouse(MouseKind::Moved, MouseButton::None, 0, 0, x0, y0),
        mouse(MouseKind::Pressed, MouseButton::Left, 1, 1, x0, y0),
        mouse(MouseKind::Moved, MouseButton::None, 1, 0, x0 + 12.0, y0 + 12.0),
        mouse(MouseKind::Moved, MouseButton::None, 1, 0, (x0 + x1) / 2.0, (y0 + y1) / 2.0),
        mouse(MouseKind::Moved, MouseButton::None, 1, 0, x1, y1),
        mouse(MouseKind::Released, MouseButton::Left, 0, 1, x1, y1),
    ];
    let mut seq = Sequence::new(page, from, PointerOpts::default(), m, events);
    seq.drag = Some(DragSeq { x0, y0, x1, y1, native: None });
    start(seq)
}

fn expect(
    page: &PageDom,
    found: &[NodeId],
    locator: &Locator,
    assert: Assert,
    expected: &Value,
    not: bool,
    property: Option<&str>,
) -> CdpResult<Attempt> {
    let dom = page.dom.borrow();
    let (holds, actual) = match assert {
        Assert::Css | Assert::Focused | Assert::Name | Assert::Enabled => {
            let Some(el) = single(found, &dom, locator)? else {
                return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
            };
            match assert {
                Assert::Css => {
                    let property = property.ok_or_else(|| CdpError::invalid_params("css needs a property"))?;
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("css needs a string"))?;
                    let have = page.layout.borrow().computed_style_value(&dom, el, property);
                    (have == want, json!(have))
                }
                Assert::Focused => {
                    let focused = dom.focused(page.document()).ok().flatten() == Some(el);
                    (focused == expected.as_bool().unwrap_or(true), json!(focused))
                }
                Assert::Enabled => {
                    let enabled = !disabled(&dom, el);
                    (enabled == expected.as_bool().unwrap_or(true), json!(enabled))
                }
                _ => {
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("name needs a string"))?;
                    let have = accessible_name(&dom, &page.layout.borrow(), page.document(), el);
                    (have == normalize(want), json!(have))
                }
            }
        }
        Assert::Count => {
            let want = expected.as_u64().ok_or_else(|| CdpError::invalid_params("count needs a number"))?;
            (found.len() as u64 == want, json!(found.len()))
        }
        Assert::Visible | Assert::Hidden => {
            let el = if found.is_empty() { None } else { single(found, &dom, locator)? };
            drop(dom);
            let visible = el.is_some_and(|el| visible_box(page, el).is_some());
            (visible == (assert == Assert::Visible), json!(visible))
        }
        Assert::Text | Assert::ContainsText | Assert::Value | Assert::Checked => {
            let Some(el) = single(found, &dom, locator)? else {
                return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
            };
            match assert {
                Assert::Checked => {
                    let Some(checked) = checked_state(&dom, el, true) else {
                        return Ok(Attempt::Retry("element is not a checkbox or radio button".into()));
                    };
                    let want = match expected {
                        Value::Null => Checked::Yes,
                        Value::Bool(true) => Checked::Yes,
                        Value::Bool(false) => Checked::No,
                        Value::String(s) if s == "mixed" => Checked::Mixed,
                        _ => return Err(CdpError::invalid_params("checked expects true, false or \"mixed\"")),
                    };
                    (checked == want, checked.to_json())
                }
                Assert::Value => {
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("value needs a string"))?;
                    let have = dom.value(el).unwrap_or_default();
                    (have == want, json!(have))
                }
                _ => {
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("text needs a string"))?;
                    let have = normalize(&text_of(&dom, el));
                    let ok =
                        if assert == Assert::Text { have == normalize(want) } else { have.contains(&normalize(want)) };
                    (ok, json!(have))
                }
            }
        }
    };
    Ok(if holds != not {
        Attempt::Done(json!({ "actual": actual }))
    } else {
        Attempt::Retry(format!("expected {}{:?} {expected}, got {actual}", if not { "not " } else { "" }, assert))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GotoParams {
    url: String,
    /// `load` (default) or `domcontentloaded`.
    wait_until: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActParams {
    locator: Locator,
    action: String,
    #[serde(default)]
    value: String,
    /// Wait for the requests the action started (default: true in `LP.run`).
    settle: Option<bool>,
    /// `drag`: the drop target.
    to: Option<Locator>,
    /// `upload`: inline files; else `value` is a path (several: one per line).
    #[serde(default)]
    files: Vec<UploadParams>,
    /// `click`, `dblclick`: `left` (default), `right` or `middle`.
    button: Option<String>,
    /// `click`, `dblclick`, `hover`, `wheel`: where on the element, as an
    /// offset from its top-left (default: the center of its visible part).
    position: Option<Position>,
    /// `click`, `dblclick`, `hover`, `wheel`: skip the hit test and the
    /// enabled check.
    #[serde(default)]
    force: bool,
    /// `wheel`: the delta.
    delta_x: Option<f64>,
    delta_y: Option<f64>,
    timeout: Option<u64>,
}

/// `{"x": 5, "y": 7}`.
#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(deny_unknown_fields)]
struct Position {
    x: f64,
    y: f64,
}

/// `{"mouse": {"action": "click", "x": 10, "y": 20}}`: `page.mouse.*` at
/// viewport coordinates. `move` takes `steps` (intermediate moves);
/// `down`/`up` take `button`; `wheel` takes `deltaX`/`deltaY`; `down`, `up`
/// and `wheel` act at the current position unless given `x` and `y`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MouseParams {
    action: String,
    x: Option<f64>,
    y: Option<f64>,
    button: Option<String>,
    delta_x: Option<f64>,
    delta_y: Option<f64>,
    steps: Option<u32>,
    /// Wait for the requests the action started (default: true).
    settle: Option<bool>,
    timeout: Option<u64>,
}

fn mouse_button(name: Option<&str>) -> CdpResult<MouseButton> {
    Ok(match name {
        None | Some("left") => MouseButton::Left,
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => return Err(CdpError::invalid_params("button must be left, right or middle")),
    })
}

impl MouseParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        let action = match self.action.as_str() {
            "click" => MouseAction::Click,
            "dblclick" => MouseAction::DblClick,
            "move" => MouseAction::Move,
            "down" => MouseAction::Down,
            "up" => MouseAction::Up,
            "wheel" => MouseAction::Wheel,
            _ => return Err(CdpError::invalid_params("mouse action must be click, dblclick, move, down, up or wheel")),
        };
        if matches!(action, MouseAction::Click | MouseAction::DblClick | MouseAction::Move)
            && (self.x.is_none() || self.y.is_none())
        {
            return Err(CdpError::invalid_params(format!("mouse {} needs x and y", self.action)));
        }
        if self.x.is_some() != self.y.is_some() {
            return Err(CdpError::invalid_params("mouse needs both x and y, or neither"));
        }
        let pointer = PointerOpts {
            button: mouse_button(self.button.as_deref())?,
            position: None,
            force: true,
            delta: (self.delta_x.unwrap_or(0.0), self.delta_y.unwrap_or(0.0)),
        };
        let steps = self.steps.unwrap_or(1).clamp(1, 1_000);
        let settle = self.settle.unwrap_or(true);
        Ok((Step::Mouse { action, x: self.x, y: self.y, pointer, steps, settle }, self.timeout))
    }
}

/// `{"name": "a.csv", "mimeType"?: "text/csv", "text"?: "..." | "base64"?: "..."}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UploadParams {
    name: String,
    mime_type: Option<String>,
    text: Option<String>,
    base64: Option<String>,
}

/// Largest file `upload` reads from disk or accepts inline.
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

fn upload_files(action: Action, value: &str, inline: Vec<UploadParams>) -> CdpResult<Vec<UploadFile>> {
    if action != Action::Upload {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for f in inline {
        let bytes = match (f.text, f.base64) {
            (Some(t), None) => t.into_bytes(),
            (None, Some(b)) => crate::fetch_protocol::base64_decode(&b)?,
            (None, None) => Vec::new(),
            _ => return Err(CdpError::invalid_params("a file takes text or base64, not both")),
        };
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(CdpError::invalid_params(format!("file {} is too large", f.name)));
        }
        let mime = f.mime_type.unwrap_or_else(|| mime_of(&f.name));
        files.push(UploadFile { name: f.name, mime, bytes });
    }
    for path in value.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let meta = std::fs::metadata(path).map_err(|e| CdpError::invalid_params(format!("upload {path}: {e}")))?;
        if meta.len() as usize > MAX_UPLOAD_BYTES {
            return Err(CdpError::invalid_params(format!("upload {path}: file is too large")));
        }
        let bytes = std::fs::read(path).map_err(|e| CdpError::invalid_params(format!("upload {path}: {e}")))?;
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mime = mime_of(&name);
        files.push(UploadFile { name, mime, bytes });
    }
    if files.is_empty() {
        return Err(CdpError::invalid_params("upload needs files or a path in value"));
    }
    Ok(files)
}

/// MIME type by extension, without charset (a `File.type` has none).
fn mime_of(name: &str) -> String {
    let full = lp_net::static_files::content_type(std::path::Path::new(name));
    match full {
        "application/octet-stream" => String::new(),
        full => full.split(';').next().unwrap_or(full).trim().to_owned(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectParams {
    locator: Locator,
    assert: String,
    #[serde(default)]
    expected: Value,
    #[serde(default)]
    not: bool,
    /// `css`: the property name (`background-color`).
    property: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryParams {
    locator: Locator,
    read: String,
    /// Key of the value in the run's `reads`.
    #[serde(rename = "as")]
    key: Option<String>,
    /// `css`: the property name.
    property: Option<String>,
    timeout: Option<u64>,
}

/// One `LP.run` step: `{"goto": {...}}`, `{"act": {...}}`, `{"check": {...}}`
/// `{"read": {...}}`, `{"route": {...}}`, `{"eval": {...}}` or
/// `{"request": {...}}`.
#[allow(clippy::large_enum_variant)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum StepParams {
    Goto(GotoParams),
    Loaded(LoadedParams),
    Idle(LoadedParams),
    Reload(LoadedParams),
    Url(UrlParams),
    Mock(MockParams),
    Unmock(UnmockParams),
    Release(ReleaseParams),
    Download(DownloadParams),
    Popup(PopupParams),
    Route(RouteParams),
    Act(ActParams),
    Mouse(MouseParams),
    Check(ExpectParams),
    Read(QueryParams),
    Eval(EvalParams),
    Request(RequestParams),
    Viewport(ViewportParams),
    Media(MediaParams),
    Page(PageParams),
    MatchScreenshot(MatchScreenshotParams),
}

/// `{"matchScreenshot": {"name": "home", "baseline": "<base64 PNG>"}}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MatchScreenshotParams {
    name: String,
    baseline: Option<String>,
    /// Hex display-list hash returned with the baseline's render.
    scene_hash: Option<String>,
    threshold: Option<f64>,
    max_diff_pixels: Option<u64>,
    max_diff_pixel_ratio: Option<f64>,
    clip: Option<ClipRect>,
    #[serde(default)]
    full_page: bool,
    locator: Option<Locator>,
    #[serde(default)]
    update: bool,
    #[serde(rename = "as")]
    key: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClipRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl MatchScreenshotParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        if self.name.is_empty() {
            return Err(CdpError::invalid_params("matchScreenshot needs a name"));
        }
        let given = [self.clip.is_some(), self.full_page, self.locator.is_some()];
        if given.iter().filter(|g| **g).count() > 1 {
            return Err(CdpError::invalid_params("matchScreenshot takes one of clip, fullPage and locator"));
        }
        if let Some(l) = &self.locator {
            l.validate()?;
        }
        let threshold = self.threshold.unwrap_or(0.0);
        if !(0.0..=1.0).contains(&threshold) {
            return Err(CdpError::invalid_params("threshold must be between 0 and 1"));
        }
        let ratio = self.max_diff_pixel_ratio.unwrap_or(0.0);
        if !(0.0..=1.0).contains(&ratio) {
            return Err(CdpError::invalid_params("maxDiffPixelRatio must be between 0 and 1"));
        }
        if let Some(c) = &self.clip {
            if !(c.width > 0.0 && c.height > 0.0) {
                return Err(CdpError::invalid_params("clip width and height must be positive"));
            }
        }
        let baseline = self.baseline.as_deref().map(crate::fetch_protocol::base64_decode).transpose()?;
        let scene_hash = match self.scene_hash.as_deref() {
            None => None,
            Some(h) => Some(u64::from_str_radix(h, 16).map_err(|_| CdpError::invalid_params("sceneHash must be hex"))?),
        };
        let shot = ShotStep {
            name: self.name,
            baseline,
            scene_hash,
            threshold,
            max_diff_pixels: self.max_diff_pixels.unwrap_or(0),
            max_diff_pixel_ratio: ratio,
            clip: self.clip.map(|c| (c.x, c.y, c.width, c.height)),
            full_page: self.full_page,
            locator: self.locator,
            update: self.update,
            key: self.key,
        };
        Ok((Step::MatchScreenshot(Box::new(shot)), self.timeout))
    }
}

/// `{"page": {"target": "popup"}}`: the `as` key of an earlier `popup`
/// step, a target id, or `main`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PageParams {
    target: String,
}

/// `{"viewport": {"width": 390, "height": 844}}`: CSS px.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewportParams {
    width: u32,
    height: u32,
}

/// `{"media": {"colorScheme": "dark"}}`: `colorScheme` (`dark`, `light`,
/// `no-preference`), `reducedMotion` (`reduce`, `no-preference`), `media`
/// (`screen`, `print`). Replaces the page's media emulation: an omitted
/// field resets to the default (light, no-preference, screen).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MediaParams {
    color_scheme: Option<String>,
    reduced_motion: Option<String>,
    media: Option<String>,
}

impl MediaParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        let mut m = super::metrics::MediaEmulation::default();
        match self.color_scheme.as_deref() {
            None | Some("light" | "no-preference") => {}
            Some("dark") => m.color_scheme = lp_dom::media::ColorScheme::Dark,
            Some(other) => return Err(CdpError::invalid_params(format!("media: unknown colorScheme '{other}'"))),
        }
        match self.reduced_motion.as_deref() {
            None | Some("no-preference") => {}
            Some("reduce") => m.reduced_motion = true,
            Some(other) => return Err(CdpError::invalid_params(format!("media: unknown reducedMotion '{other}'"))),
        }
        match self.media.as_deref() {
            None | Some("screen") => {}
            Some("print") => m.print = true,
            Some(other) => return Err(CdpError::invalid_params(format!("media: unknown media '{other}'"))),
        }
        Ok((Step::Media(m), None))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UrlParams {
    url: Pattern,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnmockParams {
    url: Option<Pattern>,
}

/// `{"release": {"hold": "name"}}`: lets the requests or responses a mock
/// holds under `hold` go on; waits until one is held.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseParams {
    hold: String,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DownloadParams {
    /// Directory to save the file in, under its suggested name.
    save_to: Option<String>,
    #[serde(rename = "as")]
    key: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PopupParams {
    #[serde(rename = "as")]
    key: Option<String>,
    timeout: Option<u64>,
}

/// `{"mock": {"url": "/api/items", "json": [...]}}`: `body` (text) or
/// `json` (serialized), `status` (200), `contentType`, `headers`, `method`,
/// `delay` (ms before the response), `abort` (a `Network.ErrorReason` such
/// as `Failed` or `ConnectionRefused`; no response), `times` (uses).
/// `passthrough: true` sends the request to the real backend and applies
/// `patch` (status, headers, json merge patch or body; implies
/// passthrough), `delay` and `hold` to its response; `hold: "name"` parks
/// the request (or, with passthrough, its response) until
/// `{"release": {"hold": "name"}}` or `delay` elapses.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MockParams {
    url: Pattern,
    method: Option<String>,
    status: Option<u16>,
    body: Option<String>,
    json: Option<Value>,
    content_type: Option<String>,
    #[serde(default)]
    headers: serde_json::Map<String, Value>,
    delay: Option<u64>,
    abort: Option<String>,
    times: Option<u32>,
    passthrough: Option<bool>,
    hold: Option<String>,
    patch: Option<PatchSpec>,
}

impl MockParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        if self.body.is_some() && self.json.is_some() {
            return Err(CdpError::invalid_params("mock takes body or json, not both"));
        }
        if self.abort.is_some() && (self.body.is_some() || self.json.is_some() || self.status.is_some()) {
            return Err(CdpError::invalid_params("an aborted mock has no response"));
        }
        if self.times == Some(0) {
            return Err(CdpError::invalid_params("times must be at least 1"));
        }
        let responds = self.body.is_some() || self.json.is_some() || self.status.is_some();
        // `patch` rewrites the real response, so it implies `passthrough`.
        if self.patch.is_some() && self.passthrough == Some(false) {
            return Err(CdpError::invalid_params(
                "patch rewrites the real response: it cannot take passthrough: false",
            ));
        }
        let passthrough = self.passthrough.unwrap_or(false) || self.patch.is_some();
        if passthrough && (responds || self.abort.is_some() || self.content_type.is_some() || !self.headers.is_empty())
        {
            return Err(CdpError::invalid_params("a passthrough mock takes patch, delay and hold, not a response"));
        }
        if let Some(patch) = &self.patch {
            if patch.body.is_some() && patch.json.is_some() {
                return Err(CdpError::invalid_params("patch takes body or json, not both"));
            }
        }
        if self.hold.as_deref().is_some_and(str::is_empty) {
            return Err(CdpError::invalid_params("hold must be a name"));
        }
        let (body, default_type) = match (self.body, &self.json) {
            (Some(b), _) => (b, "text/plain; charset=utf-8"),
            (None, Some(j)) => (serde_json::to_string(j).unwrap_or_default(), "application/json"),
            (None, None) => (String::new(), "text/plain; charset=utf-8"),
        };
        let headers = self
            .headers
            .into_iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k, s)),
                other => Err(CdpError::invalid_params(format!("header {k} must be a string, got {other}"))),
            })
            .collect::<CdpResult<Vec<_>>>()?;
        let spec = MockSpec {
            url: self.url,
            method: self.method,
            status: self.status.unwrap_or(200),
            content_type: self.content_type,
            default_type,
            headers,
            body,
            delay: Duration::from_millis(self.delay.unwrap_or(0).min(MAX_TIMEOUT_MS)),
            abort: self.abort,
            times: self.times,
            passthrough,
            hold: self.hold,
            responds,
            patch: self.patch,
        };
        Ok((Step::Mock(spec), None))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LoadedParams {
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RouteParams {
    url: String,
    /// Wait for the requests the route change started (default: true).
    settle: Option<bool>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvalParams {
    expression: String,
    #[serde(rename = "as")]
    key: Option<String>,
    /// How long to wait for a returned promise.
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestParams {
    #[serde(flatten)]
    matcher: RequestMatch,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunParams {
    /// Parsed one at a time, so a malformed step is reported by its index.
    steps: Vec<Box<serde_json::value::RawValue>>,
    /// Per-step timeout unless a step sets its own.
    timeout: Option<u64>,
}

fn timeout_of(step: Option<u64>, default: Option<u64>) -> Duration {
    Duration::from_millis(step.or(default).unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS))
}

impl ActParams {
    fn step(self, settle_default: bool) -> CdpResult<(Step, Option<u64>)> {
        self.locator.validate()?;
        let action = match self.action.as_str() {
            "click" => Action::Click,
            "fill" => Action::Fill,
            "focus" => Action::Focus,
            "check" => Action::Check,
            "uncheck" => Action::Uncheck,
            "press" => Action::Press,
            "hover" => Action::Hover,
            "dblclick" => Action::DblClick,
            "blur" => Action::Blur,
            "type" => Action::Type,
            "upload" => Action::Upload,
            "drag" => Action::Drag,
            "wheel" => Action::Wheel,
            _ => return Err(CdpError::invalid_params(
                "action must be click, fill, focus, check, uncheck, press, hover, dblclick, blur, type, upload, drag or wheel",
            )),
        };
        if action == Action::Drag {
            match &self.to {
                Some(to) => to.validate()?,
                None => return Err(CdpError::invalid_params("drag needs `to`")),
            }
        }
        let files = upload_files(action, &self.value, self.files)?;
        let settle = self.settle.unwrap_or(settle_default);
        let pointer = PointerOpts {
            button: mouse_button(self.button.as_deref())?,
            position: self.position.map(|p| (p.x, p.y)),
            force: self.force,
            delta: (self.delta_x.unwrap_or(0.0), self.delta_y.unwrap_or(0.0)),
        };
        Ok((
            Step::Act { locator: self.locator, action, value: self.value, settle, to: self.to, files, pointer },
            self.timeout,
        ))
    }
}

impl ExpectParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        self.locator.validate()?;
        let assert =
            match self.assert.as_str() {
                "visible" => Assert::Visible,
                "hidden" => Assert::Hidden,
                "text" => Assert::Text,
                "containsText" => Assert::ContainsText,
                "value" => Assert::Value,
                "count" => Assert::Count,
                "checked" => Assert::Checked,
                "css" => Assert::Css,
                "focused" => Assert::Focused,
                "name" => Assert::Name,
                "enabled" => Assert::Enabled,
                _ => return Err(CdpError::invalid_params(
                    "assert must be visible, hidden, text, containsText, value, count, checked, css, focused, name or enabled",
                )),
            };
        if assert == Assert::Css && self.property.is_none() {
            return Err(CdpError::invalid_params("css needs a property"));
        }
        Ok((
            Step::Check {
                locator: self.locator,
                assert,
                expected: self.expected,
                not: self.not,
                property: self.property,
            },
            self.timeout,
        ))
    }
}

impl QueryParams {
    fn read(&self) -> CdpResult<Read> {
        self.locator.validate()?;
        Ok(match self.read.as_str() {
            "count" => Read::Count,
            "text" => Read::Text,
            "value" => Read::Value,
            "visible" => Read::Visible,
            "box" => Read::Box,
            "css" => {
                if self.property.is_none() {
                    return Err(CdpError::invalid_params("css needs a property"));
                }
                Read::Css
            }
            "name" => Read::Name,
            "focused" => Read::Focused,
            "enabled" => Read::Enabled,
            _ => {
                return Err(CdpError::invalid_params(
                    "read must be count, text, value, visible, box, css, name, focused or enabled",
                ))
            }
        })
    }
}

impl StepParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        match self {
            StepParams::Goto(p) => {
                let dom_ready = match p.wait_until.as_deref() {
                    None | Some("load") => false,
                    Some("domcontentloaded") => true,
                    _ => return Err(CdpError::invalid_params("waitUntil must be load or domcontentloaded")),
                };
                Ok((Step::Goto { url: p.url, dom_ready }, p.timeout))
            }
            StepParams::Loaded(p) => Ok((Step::Loaded, p.timeout)),
            StepParams::Idle(p) => Ok((Step::Idle, p.timeout)),
            StepParams::Reload(p) => Ok((Step::Reload, p.timeout)),
            StepParams::Url(p) => Ok((Step::Url(p.url), p.timeout)),
            StepParams::Mock(p) => p.step(),
            StepParams::Unmock(p) => Ok((Step::Unmock(p.url), None)),
            StepParams::Release(p) => Ok((Step::Release { hold: p.hold }, p.timeout)),
            StepParams::Download(p) => Ok((Step::Download { save_to: p.save_to, key: p.key }, p.timeout)),
            StepParams::Popup(p) => Ok((Step::Popup { key: p.key }, p.timeout)),
            StepParams::Route(p) => Ok((Step::Route { url: p.url, settle: p.settle.unwrap_or(true) }, p.timeout)),
            StepParams::Eval(p) => Ok((Step::Eval { expression: p.expression, key: p.key }, p.timeout)),
            StepParams::Request(p) => Ok((Step::Request(p.matcher), p.timeout)),
            StepParams::Viewport(p) => Ok((Step::Viewport { width: p.width, height: p.height }, None)),
            StepParams::Media(p) => p.step(),
            StepParams::Page(p) => Ok((Step::Page { target: p.target }, None)),
            StepParams::MatchScreenshot(p) => p.step(),
            StepParams::Act(p) => p.step(true),
            StepParams::Mouse(p) => p.step(),
            StepParams::Check(p) => p.step(),
            StepParams::Read(p) => {
                let read = p.read()?;
                Ok((Step::Read { locator: p.locator, read, key: p.key, property: p.property }, p.timeout))
            }
        }
    }
}

fn read_value(page: &PageDom, el: NodeId, read: Read, property: Option<&str>) -> Value {
    match read {
        Read::Count => Value::Null,
        Read::Css => match property {
            Some(p) => json!(page.layout.borrow().computed_style_value(&page.dom.borrow(), el, p)),
            None => Value::Null,
        },
        Read::Name => {
            json!(accessible_name(&page.dom.borrow(), &page.layout.borrow(), page.document(), el))
        }
        Read::Focused => json!(page.dom.borrow().focused(page.document()).ok().flatten() == Some(el)),
        Read::Enabled => json!(!disabled(&page.dom.borrow(), el)),
        Read::Text => json!(normalize(&text_of(&page.dom.borrow(), el))),
        Read::Value => json!(page.dom.borrow().value(el).unwrap_or_default()),
        Read::Visible => json!(visible_box(page, el).is_some()),
        Read::Box => match visible_box(page, el) {
            Some(r) => json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}),
            None => Value::Null,
        },
    }
}

/// How driving a run ended for now.
enum Drive {
    Done(Value),
    Park(Box<Waiter>),
}

impl EngineHost {
    fn main_document(&self, page: PageKey) -> CdpResult<Rc<PageDom>> {
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        self.thread
            .browser
            .page(id)
            .and_then(|p| p.main_frame.page_dom.clone())
            .ok_or_else(|| CdpError::server("page has no document"))
    }

    /// The document a locator searches: the page's, or the document of the
    /// `<iframe>` its `frame` names (recursively).
    fn scope_page(
        &self,
        pid: PageId,
        page: &Rc<PageDom>,
        locator: &Locator,
    ) -> CdpResult<Result<Rc<PageDom>, Attempt>> {
        let Some(frame_locator) = &locator.frame else { return Ok(Ok(page.clone())) };
        let parent = match self.scope_page(pid, page, frame_locator)? {
            Ok(p) => p,
            Err(retry) => return Ok(Err(retry)),
        };
        let found = frame_locator.resolve(&parent, &self.waiters.selectors)?;
        let Some(owner) = single(&found, &parent.dom.borrow(), frame_locator)? else {
            return Ok(Err(Attempt::Retry(format!("{} matched no frame", frame_locator.describe()))));
        };
        {
            let dom = parent.dom.borrow();
            if !matches!(local(&dom, owner), Some("iframe" | "frame")) {
                return Ok(Err(Attempt::Retry(format!("{} is not an iframe", frame_locator.describe()))));
            }
        }
        let doc = self
            .thread
            .browser
            .frame_of_owner(pid, owner)
            .and_then(|id| self.thread.browser.frame(pid, &id))
            .and_then(|f| f.page_dom);
        Ok(match doc {
            Some(doc) if !doc.is_disposed() => Ok(doc),
            _ => Err(Attempt::Retry("the frame has no document yet".into())),
        })
    }

    /// One try of a locator step; the document it searched (for retries).
    fn attempt(&mut self, pid: PageId, page: &Rc<PageDom>, step: &Step) -> CdpResult<(Attempt, Rc<PageDom>)> {
        let locator = match step {
            Step::Act { locator, .. } | Step::Check { locator, .. } | Step::Read { locator, .. } => locator,
            _ => unreachable!("not a locator step"),
        };
        let page = match self.scope_page(pid, page, locator)? {
            Ok(p) => p,
            Err(retry) => return Ok((retry, page.clone())),
        };
        let attempt = self.attempt_in(&page, step)?;
        Ok((attempt, page))
    }

    fn attempt_in(&mut self, page: &Rc<PageDom>, step: &Step) -> CdpResult<Attempt> {
        let no_match = |locator: &Locator| Attempt::Retry(format!("{} matched no element", locator.describe()));
        match step {
            Step::Goto { .. }
            | Step::Loaded
            | Step::Idle
            | Step::Reload
            | Step::Url(_)
            | Step::Mock(_)
            | Step::Unmock(_)
            | Step::Release { .. }
            | Step::Download { .. }
            | Step::Popup { .. }
            | Step::Route { .. }
            | Step::Eval { .. }
            | Step::Viewport { .. }
            | Step::Media(_)
            | Step::Page { .. }
            | Step::Request(_)
            | Step::Mouse { .. }
            | Step::MatchScreenshot(_) => {
                unreachable!("not a locator step")
            }
            Step::Act { locator, action, value, to, files, pointer, .. } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                let Some(el) = single(&found, &page.dom.borrow(), locator)? else { return Ok(no_match(locator)) };
                let result = match action {
                    Action::Upload => upload(page, el, files),
                    Action::Drag => {
                        let to = to.as_ref().ok_or_else(|| CdpError::invalid_params("drag needs `to`"))?;
                        let targets = to.resolve(page, &self.waiters.selectors)?;
                        let Some(target) = single(&targets, &page.dom.borrow(), to)? else {
                            return Ok(Attempt::Retry(format!("{} matched no drop target", to.describe())));
                        };
                        if visible_box(page, el).is_none() {
                            return Ok(Attempt::Retry("element is not visible".into()));
                        }
                        drag(page, el, target)
                    }
                    _ => act(page, el, *action, value, *pointer),
                };
                if matches!(result, Attempt::Done(_) | Attempt::Continue(_)) {
                    self.thread.browser.checkpoint();
                }
                Ok(result)
            }
            Step::Check { locator, assert, expected, not, property } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                expect(page, &found, locator, *assert, expected, *not, property.as_deref())
            }
            Step::Read { locator, read, property, .. } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                if *read == Read::Count {
                    return Ok(Attempt::Done(json!(found.len())));
                }
                let Some(el) = single(&found, &page.dom.borrow(), locator)? else { return Ok(no_match(locator)) };
                Ok(Attempt::Done(read_value(page, el, *read, property.as_deref())))
            }
        }
    }

    #[cfg(not(feature = "render"))]
    fn start_shot(&mut self, _pid: PageId, _spec: &ShotStep, _element: Option<NodeId>) -> CdpResult<RenderSettlement> {
        Err(CdpError::server("matchScreenshot: lightningpanda was built without the render feature"))
    }

    pub(super) fn driver_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let (steps, single) = match action {
            "act" => {
                let (step, timeout) = cmd.command.params::<ActParams>()?.step(false)?;
                (vec![(step, timeout_of(timeout, None))], true)
            }
            "expect" => {
                let (step, timeout) = cmd.command.params::<ExpectParams>()?.step()?;
                (vec![(step, timeout_of(timeout, None))], true)
            }
            "idle" => {
                let p = cmd.command.params::<Option<LoadedParams>>()?.unwrap_or(LoadedParams { timeout: None });
                (vec![(Step::Idle, timeout_of(p.timeout, None))], true)
            }
            "run" => {
                let p: RunParams = cmd.command.params()?;
                if p.steps.len() > MAX_STEPS {
                    return Err(CdpError::invalid_params("too many steps"));
                }
                let mut steps = Vec::with_capacity(p.steps.len());
                for (i, raw) in p.steps.iter().enumerate() {
                    let (step, timeout) = serde_json::from_str::<StepParams>(raw.get())
                        .map_err(|e| CdpError::invalid_params(e.to_string()))
                        .and_then(StepParams::step)
                        .map_err(|e| CdpError::invalid_params(format!("step {i}: {}", e.message)))?;
                    steps.push((step, timeout_of(timeout, p.timeout)));
                }
                (steps, false)
            }
            "query" => return self.driver_query(cmd).map(Some),
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        };
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let pid = state.id;
        let downloads_from = state.downloads.len();
        // A run records the page's requests from here on (`request` steps, reply).
        let log_from = if single {
            None
        } else if let Some(log) = &state.network.log {
            Some(log.seq())
        } else {
            state.network.log = Some(Default::default());
            self.update_network_subscriptions();
            Some(0)
        };
        let popups_from = self.popups.len();
        let now = Instant::now();
        let first = steps.first().map_or(Duration::ZERO, |(_, t)| *t);
        let job = Job {
            id: cmd.command.id,
            session: cmd.session_id.to_owned(),
            page: cmd.page,
            pid,
            steps,
            index: 0,
            results: Vec::new(),
            reads: Default::default(),
            single,
            started: now,
            log_from,
            downloads_from,
            popups_from,
            home: cmd.page,
            parked: Vec::new(),
        };
        let waiter = Waiter {
            job,
            phase: Phase::Try,
            step_deadline: now + first,
            doc: Weak::new(),
            seen: 0,
            reason: String::new(),
            poll_at: None,
        };
        match self.drive(waiter)? {
            Drive::Done(value) => Ok(Some(value)),
            Drive::Park(w) => {
                if self.waiters.list.len() >= MAX_WAITERS {
                    return Err(CdpError::server("too many waiting LP commands"));
                }
                self.waiters.list.push(*w);
                // Replied by `service_waiters`.
                Ok(None)
            }
        }
    }

    /// Runs a job's steps until one has to wait or the last one is done.
    fn drive(&mut self, mut w: Waiter) -> CdpResult<Drive> {
        loop {
            let now = Instant::now();
            let pid = w.job.pid;
            match std::mem::replace(&mut w.phase, Phase::Try) {
                Phase::Settle { requests, since, until } => {
                    let work = self.thread.browser.page_work(pid);
                    let quiet = work.is_none_or(|w| w.quiet(requests));
                    if !quiet && now < until {
                        w.phase = Phase::Settle { requests, since, until };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    if let Some(Value::Object(r)) = w.job.results.last_mut() {
                        r.insert("settleMs".into(), json!(since.elapsed().as_secs_f64() * 1e3));
                        if let (false, Some(work)) = (quiet, work) {
                            r.insert("settled".into(), json!(false));
                            r.insert("pending".into(), pending_json(&work));
                        }
                    }
                    self.next_step(&mut w);
                }
                Phase::Await(settled) => {
                    let outcome = settled.borrow_mut().take();
                    match outcome {
                        Some(Ok(value)) => {
                            let step = w.job.steps[w.job.index].0.clone();
                            self.finish_step(&mut w, &step, value, None);
                        }
                        Some(Err(message)) => {
                            return Err(w.job.fail(CdpError::server(format!("rejected: {message}"))));
                        }
                        None => {
                            if now >= w.step_deadline {
                                let error = CdpError::server(format!(
                                    "timed out after {} ms waiting for the promise to settle",
                                    w.job.steps[w.job.index].1.as_millis()
                                ));
                                return Err(w.job.fail(error));
                            }
                            w.phase = Phase::Await(settled);
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                }
                Phase::Render(settled) => {
                    let outcome = settled.borrow_mut().take();
                    match outcome {
                        Some(Ok(value)) => {
                            let step = w.job.steps[w.job.index].0.clone();
                            self.finish_step(&mut w, &step, value, None);
                        }
                        Some(Err(error)) => return Err(w.job.fail(error)),
                        None => {
                            if now >= w.step_deadline {
                                let error = CdpError::server(format!(
                                    "timed out after {} ms waiting for the screenshot",
                                    w.job.steps[w.job.index].1.as_millis()
                                ));
                                return Err(w.job.fail(error));
                            }
                            w.phase = Phase::Render(settled);
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                }
                Phase::Act { seq, before, mut hop, at } => {
                    if now < at {
                        w.phase = Phase::Act { seq, before, hop, at };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    let done = hop.get_or_insert_with(|| self.marker());
                    // A starved loop cannot hold the step past its deadline.
                    if !done.get() && now < w.step_deadline {
                        w.phase = Phase::Act { seq, before, hop, at };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    let Some(mut seq) = seq else {
                        // The turn after a step.
                        self.next_step(&mut w);
                        continue;
                    };
                    let advanced = seq.advance();
                    self.thread.browser.checkpoint();
                    match advanced {
                        Ok(None) => {
                            w.phase = Phase::Act { seq: Some(seq), before, hop: None, at: Instant::now() + TURN };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                        Ok(Some(value)) => {
                            let step = w.job.steps[w.job.index].0.clone();
                            self.finish_step(&mut w, &step, value, before);
                        }
                        Err(reason) => {
                            let (step, timeout) = w.job.steps[w.job.index].clone();
                            return self.park_retry(w, &step, timeout, Some(&seq.page), reason, now);
                        }
                    }
                }
                Phase::Load { loader, dom_ready } => {
                    match load_state(&self.thread.browser, pid, &loader, dom_ready) {
                        LoadState::Reached => {}
                        LoadState::Follow(next) => {
                            w.phase = Phase::Load { loader: next, dom_ready };
                            continue;
                        }
                        LoadState::Failed => {
                            let error = CdpError::server("the navigation failed before its document loaded");
                            return Err(w.job.fail(error));
                        }
                        LoadState::Waiting => {
                            if now >= w.step_deadline {
                                let error = CdpError::server(format!(
                                    "timed out after {} ms waiting for {}",
                                    w.job.steps[w.job.index].1.as_millis(),
                                    if dom_ready { "DOMContentLoaded" } else { "load" }
                                ));
                                return Err(w.job.fail(error));
                            }
                            w.phase = Phase::Load { loader, dom_ready };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                    }
                    if self.thread.browser.page(pid).is_none() {
                        return Err(w.job.fail(CdpError::server("page is gone")));
                    }
                    w.job.results.push(json!({}));
                    self.next_step(&mut w);
                }
                Phase::Try => {
                    let Some((step, timeout)) = w.job.steps.get(w.job.index) else {
                        let requests = self.logged_requests(&w.job);
                        return Ok(Drive::Done(w.job.reply(requests)));
                    };
                    let (step, timeout) = (step.clone(), *timeout);
                    if let Step::Goto { url, dom_ready } = &step {
                        let loader = self
                            .thread
                            .browser
                            .navigate(pid, NavigateOpts::address_bar(url.as_str()))
                            .map_err(|e| w.job.fail(CdpError::server(format!("navigation failed: {e}"))))?;
                        self.thread.browser.checkpoint();
                        w.phase = Phase::Load { loader: loader.0, dom_ready: *dom_ready };
                        continue;
                    }
                    if let Step::Loaded = step {
                        let loader = loader_to_wait_for(&self.thread.browser, pid);
                        w.phase = Phase::Load { loader, dom_ready: false };
                        continue;
                    }
                    if let Step::Idle = step {
                        w.job.results.push(json!({}));
                        w.phase = Phase::Settle { requests: 0, since: now, until: w.step_deadline };
                        continue;
                    }
                    if let Step::Reload = step {
                        let url = self
                            .thread
                            .browser
                            .page(pid)
                            .map(|p| p.main_frame.url.clone())
                            .ok_or_else(|| w.job.fail(CdpError::server("page is gone")))?;
                        let loader = self
                            .thread
                            .browser
                            .navigate(pid, NavigateOpts::address_bar(url.as_str()))
                            .map_err(|e| w.job.fail(CdpError::server(format!("reload failed: {e}"))))?;
                        self.thread.browser.checkpoint();
                        w.phase = Phase::Load { loader: loader.0, dom_ready: false };
                        continue;
                    }
                    let before = matches!(
                        step,
                        Step::Act { settle: true, .. }
                            | Step::Mouse { settle: true, .. }
                            | Step::Route { settle: true, .. }
                            | Step::Viewport { .. }
                            | Step::Media(_)
                    )
                    .then(|| requests_of(&self.thread.browser, pid));
                    let (attempt, page) = match &step {
                        Step::Request(m) => (self.match_request(&w.job, m), None),
                        Step::Url(pattern) => {
                            let url =
                                self.thread.browser.page(pid).map(|p| p.main_frame.url.clone()).unwrap_or_default();
                            let attempt = if pattern.matches(&url, false) {
                                Attempt::Done(json!({ "url": url }))
                            } else {
                                Attempt::Retry(format!("url is {url}"))
                            };
                            (attempt, None)
                        }
                        Step::Download { save_to, .. } => (self.next_download(&mut w.job, save_to.as_deref()), None),
                        Step::Popup { .. } => (self.next_popup(&mut w.job), None),
                        Step::Viewport { width, height } => {
                            self.resize_viewport(w.job.page, *width, *height).map_err(|e| w.job.fail(e))?;
                            (Attempt::Done(json!({ "width": width, "height": height })), None)
                        }
                        Step::Media(emulation) => {
                            self.apply_media_emulation(w.job.page, *emulation).map_err(|e| w.job.fail(e))?;
                            (Attempt::Done(json!({})), None)
                        }
                        Step::Page { target } => (self.switch_page(&mut w.job, target), None),
                        Step::Mock(spec) => (self.register_mock(w.job.page, spec).map_err(|e| w.job.fail(e))?, None),
                        Step::Unmock(pattern) => {
                            let removed = self.remove_mocks(w.job.page, pattern.as_ref()).map_err(|e| w.job.fail(e))?;
                            (Attempt::Done(json!({ "removed": removed })), None)
                        }
                        Step::Release { hold } => {
                            (self.release_hold(w.job.page, hold).map_err(|e| w.job.fail(e))?, None)
                        }
                        Step::Eval { expression, .. } => {
                            match self.eval_step(pid, expression).map_err(|e| w.job.fail(e))? {
                                Evaluated::Value(value) => (Attempt::Done(value), None),
                                Evaluated::Pending(settled) => {
                                    w.phase = Phase::Await(settled);
                                    continue;
                                }
                            }
                        }
                        Step::MatchScreenshot(shot) => {
                            let clip = match &shot.locator {
                                None => None,
                                Some(locator) => {
                                    let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                                    let found =
                                        locator.resolve(&page, &self.waiters.selectors).map_err(|e| w.job.fail(e))?;
                                    let one = single(&found, &page.dom.borrow(), locator);
                                    match one {
                                        Ok(Some(id)) => Some(id),
                                        Ok(None) => {
                                            let reason = format!("{} matches no element", locator.describe());
                                            return self.park_retry(w, &step, timeout, Some(&page), reason, now);
                                        }
                                        Err(e) => return Err(w.job.fail(e)),
                                    }
                                }
                            };
                            let settled = self.start_shot(pid, shot, clip).map_err(|e| w.job.fail(e))?;
                            w.phase = Phase::Render(settled);
                            continue;
                        }
                        Step::Route { url, .. } => {
                            // `pushState` on `about:blank` throws (origin null): a
                            // prepared page whose navigation is under way gets
                            // there; one that never navigated never will.
                            let blank = self.thread.browser.page(pid).filter(|p| p.main_frame.url == "about:blank");
                            if let Some(p) = blank {
                                if p.pending_loader().is_none() {
                                    return Err(w.job.fail(CdpError::server(
                                        "the page is on about:blank: route needs a document \
                                         (goto a URL first, or loaded on a prepared page)",
                                    )));
                                }
                                (
                                    Attempt::Retry("the page is still on about:blank, its navigation pending".into()),
                                    None,
                                )
                            } else {
                                let url = serde_json::to_string(url).unwrap_or_default();
                                let src = format!(
                                    "history.pushState(null, '', {url}); \
                                     dispatchEvent(new PopStateEvent('popstate', {{ state: history.state }})); null"
                                );
                                self.eval_step(pid, &src).map_err(|e| w.job.fail(e))?;
                                (Attempt::Done(json!({})), None)
                            }
                        }
                        Step::Mouse { action, x, y, pointer, steps, .. } => {
                            let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            let attempt = mouse_step(&page, *action, *x, *y, *pointer, *steps);
                            if matches!(attempt, Attempt::Done(_)) {
                                self.thread.browser.checkpoint();
                            }
                            (attempt, Some(page))
                        }
                        _ => {
                            let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            let (attempt, scope) = self.attempt(pid, &page, &step).map_err(|e| w.job.fail(e))?;
                            (attempt, Some(scope))
                        }
                    };
                    match attempt {
                        Attempt::Done(value) => self.finish_step(&mut w, &step, value, before),
                        Attempt::Continue(seq) => {
                            w.phase = Phase::Act { seq: Some(seq), before, hop: None, at: Instant::now() + TURN };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                        Attempt::Retry(reason) => {
                            return self.park_retry(w, &step, timeout, page.as_ref(), reason, now)
                        }
                    }
                }
            }
        }
    }

    /// A step's result is in: settle after an action (and a `route`), or
    /// give an action or `eval` that does not settle one event-loop turn so
    /// what it queued (a `setTimeout(save, 0)` from a blur) runs before the
    /// next step; other steps go straight on.
    fn finish_step(&mut self, w: &mut Waiter, step: &Step, value: Value, before: Option<usize>) {
        if let Step::Read { key: Some(key), .. }
        | Step::Eval { key: Some(key), .. }
        | Step::Download { key: Some(key), .. }
        | Step::Popup { key: Some(key) } = step
        {
            w.job.reads.insert(key.clone(), value.clone());
        } else if let Step::MatchScreenshot(shot) = step {
            if let Some(key) = &shot.key {
                w.job.reads.insert(key.clone(), value.clone());
            }
        }
        w.job.results.push(value);
        let now = Instant::now();
        match before {
            Some(requests) => {
                // Frames the action's follow-up work asks for run once idle.
                lp_browser::note_client_activity();
                w.phase = Phase::Settle { requests, since: now, until: now + SETTLE_MAX };
            }
            None if matches!(step, Step::Act { .. } | Step::Eval { .. }) => {
                w.phase = Phase::Act { seq: None, before: None, hop: None, at: now + TURN };
            }
            None => self.next_step(w),
        }
    }

    /// A step that cannot finish yet: fails once past its deadline, else
    /// parks until the document it searched changes.
    fn park_retry(
        &self,
        mut w: Waiter,
        step: &Step,
        timeout: Duration,
        page: Option<&Rc<PageDom>>,
        reason: String,
        now: Instant,
    ) -> CdpResult<Drive> {
        if now >= w.step_deadline {
            let error =
                CdpError::server(format!("timed out after {} ms: {}: {reason}", timeout.as_millis(), step.describe()));
            return Err(w.job.fail(error));
        }
        if let Some(page) = page {
            w.seen = page.dom.borrow().version();
            w.doc = Rc::downgrade(page);
        }
        w.poll_at = matches!(step, Step::Url(_) | Step::Route { .. } | Step::Release { .. } | Step::Page { .. })
            .then(|| Instant::now() + URL_POLL);
        w.reason = reason;
        w.phase = Phase::Try;
        Ok(Drive::Park(Box::new(w)))
    }

    /// A marker set from a task posted behind the tasks queued now.
    fn marker(&self) -> Rc<Cell<bool>> {
        let done = Rc::new(Cell::new(false));
        post_marker(self.thread.browser.scheduler(), done.clone(), MARKER_HOPS);
        done
    }

    /// The next download of the run not returned yet; saved under its
    /// suggested name in `save_to` when given.
    fn next_download(&mut self, job: &mut Job, save_to: Option<&str>) -> Attempt {
        let Some(state) = self.pages.get(&job.page) else { return Attempt::Retry("page is gone".into()) };
        let Some(record) = state.downloads.get(job.downloads_from).cloned() else {
            return Attempt::Retry("no download yet".into());
        };
        job.downloads_from += 1;
        let mut out = json!({ "url": record.url, "filename": record.filename });
        match &record.body {
            Ok(bytes) => {
                out["size"] = json!(bytes.len());
                if bytes.len() <= 64 * 1024 {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        out["text"] = json!(text);
                    }
                }
                if let Some(dir) = save_to {
                    let name = std::path::Path::new(&record.filename)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "download".into());
                    let path = std::path::Path::new(dir).join(name);
                    match std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, bytes)) {
                        Ok(()) => out["path"] = json!(path.to_string_lossy()),
                        Err(e) => out["error"] = json!(format!("save failed: {e}")),
                    }
                }
            }
            Err(e) => out["error"] = json!(e),
        }
        Attempt::Done(out)
    }

    /// The next popup this page opened during the run, not returned yet.
    fn next_popup(&mut self, job: &mut Job) -> Attempt {
        let Some((i, record)) = self
            .popups
            .iter()
            .enumerate()
            .skip(job.popups_from)
            .find(|(_, p)| p.opener == job.page)
            .map(|(i, p)| (i, p.clone()))
        else {
            return Attempt::Retry("no popup yet".into());
        };
        job.popups_from = i + 1;
        let url =
            self.pages.get(&record.page).and_then(|s| self.thread.browser.page(s.id)).map(|p| p.main_frame.url.clone());
        Attempt::Done(json!({ "targetId": record.target_id, "url": url }))
    }

    /// Registers a mock on the page's browser context and makes sure the
    /// context's interceptor sees it.
    fn register_mock(&mut self, page: PageKey, spec: &MockSpec) -> CdpResult<Attempt> {
        let context = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let headers = super::mocks::headers_for(spec.content_type.as_deref(), spec.default_type, &spec.headers)
            .map_err(CdpError::invalid_params)?;
        let patch = spec
            .patch
            .as_ref()
            .map(|p| {
                let extra = p
                    .headers
                    .iter()
                    .map(|(k, v)| match v {
                        Value::String(s) => Ok((k.clone(), s.clone())),
                        other => Err(format!("patch header {k} must be a string, got {other}")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut headers = http::HeaderMap::new();
                super::mocks::extend_headers(&mut headers, &extra)?;
                Ok::<_, String>(super::mocks::ResponsePatch {
                    status: p.status,
                    headers,
                    json: p.json.clone(),
                    body: p.body.as_ref().map(|b| bytes::Bytes::from(b.clone())),
                })
            })
            .transpose()
            .map_err(CdpError::invalid_params)?;
        let rule = super::mocks::MockRule {
            id: 0,
            url: spec.url.clone(),
            method: spec.method.clone(),
            status: spec.status,
            headers,
            body: bytes::Bytes::from(spec.body.clone()),
            delay: spec.delay,
            abort: spec.abort.clone(),
            times: spec.times,
            passthrough: spec.passthrough,
            hold: spec.hold.clone(),
            responds: spec.responds,
            patch,
        };
        let mocks = self.contexts.get(&context).ok_or_else(|| CdpError::server("context is gone"))?.mocks.clone();
        let id = mocks.add(rule);
        self.update_fetch_interceptors();
        Ok(Attempt::Done(json!({ "mockId": id, "mocks": mocks.len() })))
    }

    /// Releases what the context's mocks hold under `name`; waits (polling)
    /// until something is held.
    fn release_hold(&mut self, page: PageKey, name: &str) -> CdpResult<Attempt> {
        let context = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let mocks = &self.contexts.get(&context).ok_or_else(|| CdpError::server("context is gone"))?.mocks;
        Ok(match mocks.release(name) {
            0 => Attempt::Retry(format!("nothing is held under {name:?} yet")),
            n => Attempt::Done(json!({ "released": n })),
        })
    }

    fn remove_mocks(&mut self, page: PageKey, url: Option<&Pattern>) -> CdpResult<usize> {
        let context = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let mocks = self.contexts.get(&context).ok_or_else(|| CdpError::server("context is gone"))?.mocks.clone();
        let removed = mocks.remove(url);
        self.update_fetch_interceptors();
        Ok(removed)
    }

    /// Evaluates `src` in the page's main world; its JSON value, or the
    /// settlement of the promise it returned (as `page.evaluate` awaits one).
    fn eval_step(&mut self, pid: PageId, src: &str) -> CdpResult<Evaluated> {
        let realm = self
            .thread
            .browser
            .page(pid)
            .and_then(|p| p.main_frame.main_realm().cloned())
            .ok_or_else(|| CdpError::server("page has no realm"))?;
        let value = {
            let scope = realm.scope().map_err(super::remote::js_to_cdp)?;
            match scope.eval(src, Some("lp-run"), 1) {
                Ok(v) if super::runtime::is_thenable(v) => {
                    let settled: Settlement = Rc::new(RefCell::new(None));
                    let (ok, err) = (settled.clone(), settled.clone());
                    scope
                        .then(
                            v,
                            move |_, v| *ok.borrow_mut() = Some(Ok(json_of(v))),
                            move |s, v| *err.borrow_mut() = Some(Err(JsException::from_value(s, v).message)),
                        )
                        .map_err(super::remote::js_to_cdp)?;
                    Evaluated::Pending(settled)
                }
                Ok(v) => Evaluated::Value(json_of(v)),
                Err(e) => return Err(CdpError::server(format!("threw: {}", e.details(&scope).message))),
            }
        };
        self.thread.browser.checkpoint();
        Ok(value)
    }

    /// A `request` step against the requests the run saw so far.
    fn match_request(&self, job: &Job, m: &RequestMatch) -> Attempt {
        let log = self.pages.get(&job.page).and_then(|p| p.network.log.as_ref());
        let (Some(log), Some(from)) = (log, job.log_from) else {
            return Attempt::Retry("the page has no request log".into());
        };
        let matching: Vec<_> = log.since(from).iter().filter(|r| m.accepts(r)).collect();
        let started = m.state == Some(RequestState::Started);
        let finished: Vec<_> = matching.iter().copied().filter(|r| r.done).collect();
        if let Some(n) = m.count {
            return if matching.len() == n && (started || finished.len() == n) {
                Attempt::Done(json!({ "count": n }))
            } else if started {
                Attempt::Retry(format!("{} matching requests started, want {n}", matching.len()))
            } else {
                Attempt::Retry(format!("{} matching requests ({} finished), want {n}", matching.len(), finished.len()))
            };
        }
        let candidates = if started { &matching } else { &finished };
        match candidates.iter().rev().find(|r| m.status.as_ref().is_none_or(|s| s.accepts(r.status))) {
            Some(r) => Attempt::Done(r.to_json()),
            None => Attempt::Retry(match candidates.last() {
                Some(r) => format!("last match was {}", r.to_json()),
                None if matching.is_empty() => "no matching request yet".into(),
                None => "the matching request is still in flight".into(),
            }),
        }
    }

    /// Makes `target` the page the following steps drive, keeping where the
    /// run left the current one (its request log and downloads).
    fn switch_page(&mut self, job: &mut Job, target: &str) -> Attempt {
        let id = match job.reads.get(target) {
            Some(read) => read["targetId"].as_str().unwrap_or(target).to_owned(),
            None => target.to_owned(),
        };
        let context = self.pages.get(&job.home).map(|s| s.context);
        let found = if target == "main" {
            Some(job.home)
        } else {
            self.pages.iter().find_map(|(key, state)| {
                let page = self.thread.browser.page(state.id)?;
                (Some(state.context) == context && page.main_frame.id.0 == id).then_some(*key)
            })
        };
        let Some(key) = found else { return Attempt::Retry(format!("no page {id} in this context")) };
        let Some(state) = self.pages.get_mut(&key) else { return Attempt::Retry("page is gone".into()) };
        let url = self.thread.browser.page(state.id).map(|p| p.main_frame.url.clone());
        if key != job.page {
            let cursor = match job.parked.iter().position(|c| c.page == key) {
                Some(i) => job.parked.swap_remove(i),
                None => {
                    // The run records this page's requests from here on.
                    let log_from = match &state.network.log {
                        Some(log) => log.seq(),
                        None => {
                            state.network.log = Some(Default::default());
                            0
                        }
                    };
                    PageCursor {
                        page: key,
                        pid: state.id,
                        log_from: Some(log_from),
                        downloads_from: state.downloads.len(),
                    }
                }
            };
            let left = PageCursor {
                page: std::mem::replace(&mut job.page, cursor.page),
                pid: std::mem::replace(&mut job.pid, cursor.pid),
                log_from: std::mem::replace(&mut job.log_from, cursor.log_from),
                downloads_from: std::mem::replace(&mut job.downloads_from, cursor.downloads_from),
            };
            job.parked.push(left);
            self.update_network_subscriptions();
        }
        Attempt::Done(json!({ "targetId": id_of(&self.thread.browser, job.pid), "url": url }))
    }

    /// The requests a run saw on the page it started on, for its reply.
    fn logged_requests(&self, job: &Job) -> Vec<Value> {
        let (page, from) = match job.parked.iter().find(|c| c.page == job.home) {
            Some(c) => (c.page, c.log_from),
            None => (job.page, job.log_from),
        };
        let log = self.pages.get(&page).and_then(|p| p.network.log.as_ref());
        match (log, from) {
            (Some(log), Some(from)) => log.since(from).iter().map(|r| r.to_json()).collect(),
            _ => Vec::new(),
        }
    }

    fn next_step(&mut self, w: &mut Waiter) {
        w.job.index += 1;
        w.phase = Phase::Try;
        if let Some((_, timeout)) = w.job.steps.get(w.job.index) {
            w.step_deadline = Instant::now() + *timeout;
        }
    }

    fn driver_query(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        let p: QueryParams = cmd.command.params()?;
        let read = p.read()?;
        let page = self.main_document(cmd.page)?;
        let found = p.locator.resolve(&page, &self.waiters.selectors)?;
        if read == Read::Count {
            return Ok(json!({ "value": found.len() }));
        }
        let el = single(&found, &page.dom.borrow(), &p.locator)?
            .ok_or_else(|| CdpError::server(format!("{} matched no element", p.locator.describe())))?;
        Ok(json!({ "value": read_value(&page, el, read, p.property.as_deref()) }))
    }

    /// Drives the waiters that can make progress and replies to those that
    /// finished or failed. Replies are queued behind the events the steps
    /// produced.
    pub(super) fn service_waiters(&mut self) {
        if self.waiters.list.is_empty() {
            self.waiters.log_changed = false;
            return;
        }
        let now = Instant::now();
        let sessions = &self.sessions;
        self.waiters.list.retain(|w| sessions.contains_key(&w.job.session));
        let browser = &self.thread.browser;
        let log_changed = std::mem::take(&mut self.waiters.log_changed);
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters.list)
            .into_iter()
            .partition(|w| w.ready(browser, now) || (log_changed && w.waits_on_log()));
        self.waiters.list = waiting;
        for w in ready {
            let (id, session) = (w.job.id, w.job.session.clone());
            let frame = match self.drive(w) {
                Ok(Drive::Park(w)) => {
                    self.waiters.list.push(*w);
                    continue;
                }
                Ok(Drive::Done(value)) => msg::result_frame(id, &value, Some(&session)),
                Err(error) => msg::error_frame(Some(id), &error, Some(&session)),
            };
            self.collector.push(Queued::Frame(frame));
        }
    }
}

/// A page's target id (its main frame id).
fn id_of(browser: &Browser, pid: PageId) -> Option<String> {
    browser.page(pid).map(|p| p.main_frame.id.0.clone())
}
