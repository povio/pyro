//! `Pyro.*`: a native test driver.
//!
//! `Pyro.run` takes a whole scenario, a list of steps (`goto`, `route`, `act`,
//! `check`, `read`, `eval`, `request`), and replies once, when the last step
//! holds or the first one fails, with the values read and the requests the
//! page made meanwhile (`request` steps check that log).
//! `Pyro.act` and `Pyro.expect` are one-step runs. Locators (with `within` and
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
//! the page is quiet (`Browser::page_work_since`): no queued task, no
//! `setTimeout` due within `TIMER_HORIZON`, no pending animation frame, no
//! navigation short of DOMContentLoaded, and no request in flight that was
//! started after the action began (one older than the action, a long poll,
//! is not waited for, whatever else completes meanwhile). Readiness is checked
//! between loop tasks, so the step goes on right after the last piece of work.
//! What the next rendering update owes is work too: `scroll` events to fire,
//! ResizeObservers that have yet to gather or deliver. When that is all that
//! is left, the settle runs the update at once instead of waiting out the
//! frame interval ([`SETTLE_UPDATES`]). It gives up after [`SETTLE_MAX`],
//! carries on, and reports what was still pending. An `idle` step (`Pyro.idle`)
//! waits the same way for every request, up to its timeout.
//!
//! `release`: a held reply that goes straight to the page (not a request
//! released to the network) is delivered, and the page gets an event-loop
//! turn to act on it, before the next step starts ([`Phase::Deliver`]).
//!
//! Turns: each input event of a pointer action (`click`, `dblclick`, `hover`,
//! `drag`) is dispatched in its own task, with an event-loop turn (the frame
//! interval, then a marker task: queued tasks, due timers, animation frames)
//! between two of them, as Chromium delivers input events ([`Phase::Act`]). An
//! action that does not settle gets one such turn before the next step starts,
//! and so does an `eval` unless its script left the page nothing to run (it
//! only read: `Browser::work_stamp`). That turn runs the scroll steps the page
//! owes as well, so a step's own `scroll` event (a `focus` that revealed its
//! element) does not arrive a frame interval later, inside another step.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use pyro_browser::{Browser, NavigateOpts, PageId};
use pyro_dom::layout::Layout;
use pyro_dom::select::SelectorCache;
use pyro_dom::{Dom, NodeId, NodeKind};
use pyro_js::JsException;
use pyro_runtime::{Scheduler, TaskSource};
use pyro_webapi::input::{self, MouseButton, MouseInput, MouseKind};
use pyro_webapi::PageDom;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

/// A step's timeout when neither it nor its run sets one. A step has no
/// upper bound: Playwright's "no limit" (its 0) arrives as a timeout longer
/// than any run (`NO_LIMIT` in the wright), and 0 means one try.
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
/// Longest `delay` of a mocked response.
const MAX_DELAY_MS: u64 = 600_000;
/// Waiting runs per connection (each holds only its parsed steps).
const MAX_WAITERS: usize = 1024;
const MAX_STEPS: usize = 10_000;
/// Longest an action waits for the page to go quiet.
const SETTLE_MAX: Duration = Duration::from_millis(1_000);
/// How many times a turn's marker task lets queued tasks go first.
const MARKER_HOPS: u32 = 4;

/// What a locator names. Exactly one of `css`, `xpath`, `testId`, `text`,
/// `role`, `label`, `placeholder` or `of` (another locator's matches, so a
/// filter or `nth` can follow an `nth`), searched inside each match of
/// `within` (else the document), joined with the matches of `or` (document
/// order) and kept only where every `and` matches too, narrowed by `filter`;
/// `nth` then picks one match instead of requiring a unique one.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Locator {
    css: Option<String>,
    xpath: Option<String>,
    of: Option<Box<Locator>>,
    test_id: Option<String>,
    /// Attribute `testId` matches (default `data-testid`).
    test_id_attribute: Option<String>,
    text: Option<Pattern>,
    role: Option<String>,
    /// Accessible name for `role`.
    name: Option<Pattern>,
    /// `role` state filters (Playwright's `getByRole` options): `checked`
    /// and `pressed` (`true`, `false` or `"mixed"`), `disabled`, `expanded`,
    /// `selected`, `level`; `includeHidden` lets hidden elements match.
    #[serde(skip_serializing_if = "Option::is_none")]
    checked: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pressed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expanded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    include_hidden: bool,
    label: Option<Pattern>,
    placeholder: Option<Pattern>,
    /// Playwright's `aria-ref=e5` selector: the element the document's last
    /// AI-mode ARIA snapshot gave this ref (`f1e5`: in the frame whose
    /// snapshot used the `f1` prefix).
    #[serde(skip_serializing_if = "Option::is_none")]
    aria_ref: Option<String>,
    /// Whole-string, case-sensitive match for text, name, label and
    /// placeholder (after whitespace normalization); otherwise a
    /// case-insensitive substring. A `{"regex"}` pattern ignores it.
    #[serde(default)]
    exact: bool,
    /// `text` follows Playwright's legacy `text="…"` selector engine rather
    /// than `getByText`: an exact string also matches an element one of whose
    /// own text-node runs equals it, and an element whose child matches too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    legacy_text: bool,
    within: Option<Box<Locator>>,
    filter: Option<Filter>,
    /// Index into the matches (negative counts from the end).
    nth: Option<i64>,
    /// Search inside the document of this `<iframe>` (a locator in the
    /// parent document; may itself have a `frame`).
    frame: Option<Box<Locator>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    or: Vec<Locator>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    and: Vec<Locator>,
}

impl Locator {
    // ---- role states ----

    fn has_role_states(&self) -> bool {
        self.checked.is_some()
            || self.pressed.is_some()
            || self.disabled.is_some()
            || self.expanded.is_some()
            || self.selected.is_some()
            || self.level.is_some()
    }

    /// Whether `n` (an element with the locator's role) has the asked states.
    fn role_states_hold(&self, dom: &Dom, n: NodeId) -> bool {
        let tri = |want: &Value, have: Option<&str>| match want {
            Value::Bool(true) => have == Some("true"),
            Value::Bool(false) => have != Some("true") && have != Some("mixed"),
            Value::String(s) => have == Some(s.as_str()),
            _ => false,
        };
        if let Some(want) = &self.checked {
            let have = match checked_state(dom, n, true) {
                Some(Checked::Yes) => "true",
                Some(Checked::Mixed) => "mixed",
                _ => "false",
            };
            if !tri(want, Some(have)) {
                return false;
            }
        }
        if let Some(want) = &self.pressed {
            if !tri(want, attr(dom, n, "aria-pressed")) {
                return false;
            }
        }
        if let Some(want) = self.disabled {
            let aria = self_and_ancestors(dom, n).any(|a| attr(dom, a, "aria-disabled") == Some("true"));
            if (disabled(dom, n) || aria) != want {
                return false;
            }
        }
        if let Some(want) = self.expanded {
            if (attr(dom, n, "aria-expanded") == Some("true")) != want {
                return false;
            }
        }
        if let Some(want) = self.selected {
            let selected = attr(dom, n, "aria-selected") == Some("true")
                || (local(dom, n) == Some("option") && dom.option_selected(n).unwrap_or(false));
            if selected != want {
                return false;
            }
        }
        if let Some(want) = self.level {
            let level = attr(dom, n, "aria-level").and_then(|l| l.trim().parse::<u32>().ok()).or_else(|| {
                local(dom, n)
                    .and_then(|t| t.strip_prefix('h'))
                    .and_then(|d| d.parse::<u32>().ok())
                    .filter(|d| (1..=6).contains(d))
            });
            if level != Some(want) {
                return false;
            }
        }
        true
    }
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
    /// Focus the element as a script's `focus()` does
    /// ([`input::script_focus`]): its listeners run back to back, microtasks
    /// after them. `Fill`, `Press` and `Type` start with the same focus.
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
    /// Remove focus from the element as a script's `blur()` does
    /// ([`input::script_blur`]).
    Blur,
    /// Focus, then one key press per character of `value` (`pressSequentially`).
    Type,
    /// Set the files of an `<input type=file>` (`files`, or `value` as a path).
    Upload,
    DropFiles,
    /// Drag the element onto `to`: native drag and drop when the element is
    /// a drag source, else pointer moves with the button held.
    Drag,
    /// Move the pointer to the element's center, then a wheel turn of
    /// `deltaX`/`deltaY` there (scrolls the nearest scrollable container).
    Wheel,
}

impl Action {
    /// Actions that need an enabled element (Playwright's actionability:
    /// hover, focus, blur, wheel and drag do not; `press` and `type` focus
    /// the element and send the keys whatever its state).
    fn needs_enabled(self) -> bool {
        matches!(self, Action::Click | Action::DblClick | Action::Fill | Action::Check | Action::Uncheck)
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
    /// Modifier keys held during the action (`ctrlKey`, `shiftKey`, ... on its events).
    modifiers: input::Modifiers,
}

impl Default for PointerOpts {
    fn default() -> Self {
        PointerOpts {
            button: MouseButton::Left,
            position: None,
            force: false,
            delta: (0.0, 0.0),
            modifiers: input::Modifiers::default(),
        }
    }
}

/// `["Shift", "ControlOrMeta"]` as held modifiers (`ControlOrMeta` is Meta on macOS).
fn modifiers_of(names: &[String]) -> CdpResult<input::Modifiers> {
    let mut m = input::Modifiers::default();
    for name in names {
        match name.as_str() {
            "Shift" => m.shift = true,
            "Control" => m.ctrl = true,
            "Alt" => m.alt = true,
            "Meta" => m.meta = true,
            "ControlOrMeta" if cfg!(target_os = "macos") => m.meta = true,
            "ControlOrMeta" => m.ctrl = true,
            other => return Err(CdpError::invalid_params(format!("unknown modifier {other:?}"))),
        }
    }
    Ok(m)
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
    Attribute,
    Description,
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
    /// The ARIA tree (aria_snapshot.rs), for `ariaSnapshot()`.
    Aria,
}

/// Which requests a `request` step looks for in the run's log.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestMatch {
    /// Context scope includes prior runs of this same page, never sibling pages.
    scope: Option<RequestScope>,
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
    /// The result carries the matched response's body (`responseBody`, as
    /// text, or base64 with `responseBodyBase64`). The page's log keeps
    /// bodies from the run with the first such step on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    response_body: bool,
    /// The run's values get the matched response's body (with
    /// `responseBody`) or the matched request under this name.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RequestScope {
    Batch,
    Context,
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

/// A file for `upload` (and `DOM.setFileInputFiles`).
#[derive(Clone, Debug)]
pub(super) struct UploadFile {
    name: String,
    mime: String,
    bytes: Vec<u8>,
    /// `File.lastModified` (ms since the epoch); `None`: now.
    last_modified: Option<f64>,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_millis() as f64)
}

/// The file at `path`, at most [`MAX_UPLOAD_BYTES`], named by its last
/// component, typed by its extension and dated by its modification time.
pub(super) fn read_upload_path(path: &str) -> CdpResult<UploadFile> {
    let meta = std::fs::metadata(path).map_err(|e| CdpError::invalid_params(format!("upload {path}: {e}")))?;
    if meta.len() as usize > MAX_UPLOAD_BYTES {
        return Err(CdpError::invalid_params(format!("upload {path}: file is too large")));
    }
    let bytes = std::fs::read(path).map_err(|e| CdpError::invalid_params(format!("upload {path}: {e}")))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mime = mime_of(&name);
    let last_modified =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64);
    Ok(UploadFile { name, mime, bytes, last_modified })
}

/// Why `id` cannot take files, if it cannot: not an `<input type=file>`.
pub(super) fn not_a_file_input(page: &PageDom, id: NodeId) -> Option<&'static str> {
    let dom = page.dom.borrow();
    let is_file_input =
        local(&dom, id) == Some("input") && attr(&dom, id, "type").is_some_and(|t| t.eq_ignore_ascii_case("file"));
    (!is_file_input).then_some("element is not an <input type=file>")
}

/// Gives the file input `id` `files`, then fires `input` and `change`.
pub(super) fn set_file_input(page: &Rc<PageDom>, id: NodeId, files: &[UploadFile]) -> Result<(), String> {
    let now = now_ms();
    let entries: Vec<pyro_webapi::file::FileEntry> = files
        .iter()
        .map(|f| pyro_webapi::file::FileEntry {
            name: f.name.clone(),
            mime: f.mime.clone(),
            bytes: bytes::Bytes::from(f.bytes.clone()),
            last_modified: f.last_modified.unwrap_or(now),
        })
        .collect();
    page.set_input_files(id, entries).map_err(|e| format!("{e:?}"))?;
    input::fire_simple(page, id, "input", true, false);
    input::fire_user_change(page, id);
    Ok(())
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
        body: Option<String>,
    },
    Held {
        hold: String,
        count: usize,
        key: Option<String>,
        status: StatusSet,
    },
    AdvanceTime {
        ms: u64,
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
    DownloadCount {
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
        /// Call the expression's function value instead of returning it.
        call: Option<Box<EvalCall>>,
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
    /// Keys sent to the focused element (the body when none has focus), as
    /// Playwright's `page.keyboard.press` / `keyboard.type`.
    Keyboard {
        /// A key or chord (`Escape`, `Shift+Tab`) for `press`.
        press: Option<String>,
        /// Text typed one key press per character.
        text: Option<String>,
        modifiers: input::Modifiers,
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
        /// `aria` in AI mode: frame seqs by frame id (`QueryParams`).
        ai: Option<Box<std::collections::HashMap<String, u32>>>,
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
    pub(super) max_diff_pixels: Option<u64>,
    pub(super) max_diff_pixel_ratio: Option<f64>,
    /// CSS px, relative to the document.
    pub(super) clip: Option<(f64, f64, f64, f64)>,
    pub(super) full_page: bool,
    /// Capture this element's box.
    pub(super) locator: Option<Locator>,
    /// Ignore the baseline: render and return the image.
    pub(super) update: bool,
    pub(super) capture_only: bool,
    pub(super) key: Option<String>,
    /// Animations as Playwright's `animations: "disabled"` leaves them (see
    /// `PageDom::settled_animation_frame`), else where they are.
    pub(super) settle_animations: bool,
}

impl Step {
    /// Fixed enum labels only: never serialize a selector, URL or payload.
    fn trace_kind(&self) -> &'static str {
        match self {
            Step::Loaded => "loaded",
            Step::Reload => "reload",
            Step::Url(_) => "url",
            Step::Mock(_) => "mock",
            Step::Unmock(_) => "unmock",
            Step::Release { .. } => "release",
            Step::Held { .. } => "held",
            Step::AdvanceTime { .. } => "advanceTime",
            Step::Download { .. } => "download",
            Step::Popup { .. } => "popup",
            Step::DownloadCount { .. } => "downloadCount",
            Step::Goto { .. } => "goto",
            Step::Idle => "idle",
            Step::Route { .. } => "route",
            Step::Eval { .. } => "eval",
            Step::Viewport { .. } => "viewport",
            Step::Media(_) => "media",
            Step::Page { .. } => "page",
            Step::Request(_) => "request",
            Step::Act { .. } => "act",
            Step::Mouse { .. } => "mouse",
            Step::Keyboard { .. } => "keyboard",
            Step::Check { .. } => "check",
            Step::Read { .. } => "read",
            Step::MatchScreenshot(_) => "matchScreenshot",
        }
    }

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
            Step::Release { hold, .. } => format!("release {hold}"),
            Step::Held { hold, .. } => format!("held response {hold}"),
            Step::AdvanceTime { ms } => format!("advanceTime {ms}"),
            Step::Download { .. } => "download".into(),
            Step::DownloadCount { .. } => "downloadCount".into(),
            Step::Popup { .. } => "popup".into(),
            Step::Route { url, .. } => format!("route {url}"),
            Step::Eval { .. } => "eval".into(),
            Step::Viewport { width, height } => format!("viewport {width}x{height}"),
            Step::Media(m) => format!("media {m:?}"),
            Step::Page { target } => format!("page {target}"),
            Step::Request(m) => {
                let mut value = serde_json::to_value(m).map(prune).unwrap_or_default();
                if let Some(headers) = value.get_mut("headers") {
                    *headers = super::network::redact_header_values(headers.take());
                }
                format!("request {value}")
            }
            Step::Act { locator, action, .. } => format!("{action:?} {}", locator.describe()),
            Step::Mouse { action, x, y, .. } => match (x, y) {
                (Some(x), Some(y)) => format!("mouse {action:?} at {x},{y}"),
                _ => format!("mouse {action:?}"),
            },
            Step::Keyboard { press: Some(key), .. } => format!("keyboard press {key}"),
            Step::Keyboard { .. } => "keyboard type".into(),
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
    /// A focus handler replaced or redirected the target; reacquire after an event-loop turn.
    RetrySoon(String),
    /// The action's first input event went out; the rest follow, one per
    /// event-loop turn.
    Continue(Box<Sequence>),
}

/// Diagnostic only. No Drop record: cancellation must leave the last
/// started step unmatched rather than falsely marking it complete.
struct RunStepTrace {
    process_id: u32,
    active: Cell<Option<usize>>,
    attempts_started: Cell<u64>,
    attempts_completed: Cell<u64>,
    attempt_ms: Cell<f64>,
    post_eval_turn: Cell<Option<Instant>>,
    waits: WaitTrace,
}

/// Where a step's time went (`pyro-run-wait`, one record per step): the time
/// the driver itself ran (`drive`), and the time the run was parked, by the
/// phase it waited in and what the page still had to do then (`settle
/// requests`: settling, a request in flight). Sampled between loop tasks, so
/// each interval is one task, or one sleep of the loop. Wall and thread CPU
/// time per bucket: CPU far below wall is a loop that slept (or a machine
/// with no CPU to give).
#[derive(Default)]
struct WaitTrace {
    /// The interval running now: its bucket, start, and the thread's CPU time then.
    open: Cell<Option<(WaitBucket, Instant, Duration)>>,
    /// Per bucket: intervals, wall time, CPU time.
    buckets: RefCell<Vec<(WaitBucket, u64, Duration, Duration)>>,
    /// The thread's CPU time when the step started.
    step: Cell<Option<Duration>>,
}

/// (phase, what the page had pending).
type WaitBucket = (&'static str, &'static str);

impl WaitTrace {
    /// Ends the interval running now and starts one in `next`.
    fn mark(&self, next: Option<WaitBucket>, now: Instant) {
        let cpu = crate::fork_stats::thread_cpu();
        if let Some((bucket, since, cpu_then)) = self.open.take() {
            let wall = now.saturating_duration_since(since);
            let used = cpu.saturating_sub(cpu_then);
            let mut buckets = self.buckets.borrow_mut();
            match buckets.iter_mut().find(|(b, ..)| *b == bucket) {
                Some((_, n, w, c)) => {
                    *n += 1;
                    *w += wall;
                    *c += used;
                }
                None => buckets.push((bucket, 1, wall, used)),
            }
        }
        self.open.set(next.map(|bucket| (bucket, now, cpu)));
    }

    fn start_step(&self, now: Instant) {
        self.open.set(None);
        self.buckets.borrow_mut().clear();
        self.step.set(Some(crate::fork_stats::thread_cpu()));
        self.mark(Some(("drive", "")), now);
    }

    /// The step's record fields; the driver keeps running in `drive`.
    fn finish_step(&self, record: &mut Value) {
        let now = Instant::now();
        self.mark(Some(("drive", "")), now);
        let ms = |d: Duration| (d.as_secs_f64() * 1e6).round() / 1e3;
        let mut waits = serde_json::Map::new();
        for (bucket, n, wall, cpu) in self.buckets.borrow_mut().drain(..) {
            let name = if bucket.1.is_empty() { bucket.0.to_owned() } else { format!("{} {}", bucket.0, bucket.1) };
            waits.insert(name, json!([n, ms(wall), ms(cpu)]));
        }
        record["waits"] = Value::Object(waits);
        if let Some(cpu) = self.step.take() {
            record["cpuMs"] = json!(ms(crate::fork_stats::thread_cpu().saturating_sub(cpu)));
        }
    }
}

/// What holds a page's work up, most immediate first (a queued task runs
/// before the loop sleeps on a request or a timer).
fn waiting_on(work: Option<pyro_browser::PageWork>) -> &'static str {
    match work {
        None => "gone",
        Some(w) if w.tasks > 0 => "tasks",
        Some(w) if w.navigations > 0 => "navigation",
        Some(w) if w.requests > 0 => "requests",
        Some(w) if w.scrolls > 0 => "scrolls",
        Some(w) if w.observers > 0 => "observers",
        Some(w) if w.frames > 0 => "frames",
        Some(w) if w.timers > 0 => "timers",
        Some(_) => "idle",
    }
}

impl RunStepTrace {
    fn emit(&self, job: &Job, kind: &'static str, event: &'static str) {
        use std::io::Write;
        let line = format!(
            "pyro-run-step {}\n",
            json!({
                "pid": self.process_id,
                "runId": job.id,
                "step": job.index,
                "kind": kind,
                "event": event,
                "elapsedMs": job.started.elapsed().as_secs_f64() * 1e3,
            })
        );
        // An unavailable diagnostic sink must not fail the browser action.
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }
}

// Detailed records are bounded independently of retries. A terminal summary
// still reports every completed attempt; cancellation never fabricates one.
const TRACE_ATTEMPT_LIMIT: u64 = 8;

#[derive(Clone, Copy)]
struct RunStageContext {
    process_id: u32,
    run_id: u64,
    step: usize,
    started: Instant,
}

impl RunStageContext {
    fn record(self, stage: &'static str, event: &'static str) -> Value {
        json!({ "pid": self.process_id, "runId": self.run_id, "step": self.step,
            "stage": stage, "event": event, "elapsedMs": self.started.elapsed().as_secs_f64() * 1e3 })
    }

    fn span(self, stage: &'static str) -> RunStageSpan {
        emit_run_stage(self.record(stage, "start"));
        RunStageSpan { context: self, stage, started: Instant::now(), finished: false }
    }
}

fn emit_run_stage(record: Value) {
    use std::io::Write;
    let line = format!("pyro-run-stage {}\n", record);
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

struct RunStageSpan {
    context: RunStageContext,
    stage: &'static str,
    started: Instant,
    finished: bool,
}
impl RunStageSpan {
    fn finish(mut self, outcome: &'static str) {
        self.finished = true;
        self.emit("end", outcome);
    }
    fn emit(&self, event: &'static str, outcome: &'static str) {
        let mut record = self.context.record(self.stage, event);
        record["durationMs"] = json!(self.started.elapsed().as_secs_f64() * 1e3);
        record["outcome"] = json!(outcome);
        emit_run_stage(record);
    }
}
impl Drop for RunStageSpan {
    fn drop(&mut self) {
        if !self.finished {
            self.emit("error", "error");
        }
    }
}

struct RunAttemptSpan<'a> {
    trace: &'a RunStepTrace,
    context: RunStageContext,
    stage: &'static str,
    number: u64,
    started: Instant,
}
impl RunAttemptSpan<'_> {
    fn finish(self, attempt: Option<&Attempt>) {
        let ms = self.started.elapsed().as_secs_f64() * 1e3;
        self.trace.attempts_completed.set(self.trace.attempts_completed.get() + 1);
        self.trace.attempt_ms.set(self.trace.attempt_ms.get() + ms);
        if self.number <= TRACE_ATTEMPT_LIMIT {
            let outcome = match attempt {
                Some(Attempt::Done(_)) => "done",
                Some(Attempt::Retry(_)) => "retry",
                Some(Attempt::RetrySoon(_)) => "retrySoon",
                Some(Attempt::Continue(_)) => "continue",
                None => "error",
            };
            let mut record = self.context.record(self.stage, if attempt.is_some() { "end" } else { "error" });
            record["attempt"] = json!(self.number);
            record["durationMs"] = json!(ms);
            record["outcome"] = json!(outcome);
            emit_run_stage(record);
        }
    }
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
    /// `Pyro.act` / `Pyro.expect`: reply with the step's own result.
    single: bool,
    started: Instant,
    trace: Option<RunStepTrace>,
    /// Request log sequence when the run started (`Pyro.run` only).
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
    fn trace_step_start(&self) {
        let (Some(trace), Some((step, _))) = (&self.trace, self.steps.get(self.index)) else { return };
        if trace.active.get() == Some(self.index) {
            return;
        }
        trace.active.set(Some(self.index));
        trace.attempts_started.set(0);
        trace.attempts_completed.set(0);
        trace.attempt_ms.set(0.0);
        trace.post_eval_turn.set(None);
        trace.waits.start_step(Instant::now());
        trace.emit(self, step.trace_kind(), "start");
    }

    /// The driver runs this job's step from `now` on (it was parked).
    fn trace_driving(&self, now: Instant) {
        if let Some(trace) = &self.trace {
            trace.waits.mark(Some(("drive", "")), now);
        }
    }

    /// A settle ended: how long it took and what was still pending.
    fn trace_settle(&self, ms: f64, pending: Option<&pyro_browser::PageWork>) {
        if let Some(context) = self.trace_stage_context() {
            let mut record = context.record("settle", "end");
            record["durationMs"] = json!(ms);
            record["outcome"] = json!(if pending.is_some() { "gaveUp" } else { "quiet" });
            if let Some(work) = pending {
                record["pending"] = pending_json(work);
            }
            emit_run_stage(record);
        }
    }

    fn trace_stage_context(&self) -> Option<RunStageContext> {
        self.trace.as_ref().map(|trace| RunStageContext {
            process_id: trace.process_id,
            run_id: self.id,
            step: self.index,
            started: self.started,
        })
    }

    fn trace_attempt(&self, stage: &'static str) -> Option<RunAttemptSpan<'_>> {
        let trace = self.trace.as_ref()?;
        let context = self.trace_stage_context()?;
        let number = trace.attempts_started.get() + 1;
        trace.attempts_started.set(number);
        if number <= TRACE_ATTEMPT_LIMIT {
            let mut record = context.record(stage, "start");
            record["attempt"] = json!(number);
            emit_run_stage(record);
        } else if number == TRACE_ATTEMPT_LIMIT + 1 {
            let mut record = context.record(stage, "truncated");
            record["completedAttempts"] = json!(trace.attempts_completed.get());
            record["attemptMs"] = json!(trace.attempt_ms.get());
            emit_run_stage(record);
        }
        Some(RunAttemptSpan { trace, context, stage, number, started: Instant::now() })
    }

    fn trace_post_eval_turn_start(&self) {
        if let (Some(trace), Some(context)) = (&self.trace, self.trace_stage_context()) {
            emit_run_stage(context.record("post_eval_turn", "start"));
            trace.post_eval_turn.set(Some(Instant::now()));
        }
    }

    /// An `eval` that left the page nothing to run goes on without a turn.
    fn trace_post_eval_turn_skipped(&self) {
        if let Some(context) = self.trace_stage_context() {
            emit_run_stage(context.record("post_eval_turn", "skipped"));
        }
    }

    fn trace_post_eval_turn_finish(&self, event: &'static str) {
        if let (Some(trace), Some(context)) = (&self.trace, self.trace_stage_context()) {
            if let Some(started) = trace.post_eval_turn.take() {
                let mut record = context.record("post_eval_turn", event);
                record["durationMs"] = json!(started.elapsed().as_secs_f64() * 1e3);
                record["outcome"] = json!(if event == "error" { "error" } else { "done" });
                emit_run_stage(record);
            }
        }
    }

    fn trace_step_end(&self) {
        self.trace_step_finish("end");
    }

    fn trace_step_error(&self) {
        self.trace_step_finish("error");
    }

    fn trace_step_finish(&self, event: &'static str) {
        let (Some(trace), Some((step, _))) = (&self.trace, self.steps.get(self.index)) else { return };
        if trace.active.take() == Some(self.index) {
            self.trace_post_eval_turn_finish(event);
            if trace.attempts_started.get() > 0 {
                let stage = if matches!(step, Step::Request(_)) { "request_attempt" } else { "check_attempt" };
                let mut record = self.trace_stage_context().expect("trace present").record(stage, "summary");
                record["startedAttempts"] = json!(trace.attempts_started.get());
                record["completedAttempts"] = json!(trace.attempts_completed.get());
                record["attemptMs"] = json!(trace.attempt_ms.get());
                record["recordedAttempts"] = json!(trace.attempts_started.get().min(TRACE_ATTEMPT_LIMIT));
                record["truncated"] = json!(trace.attempts_started.get() > TRACE_ATTEMPT_LIMIT);
                emit_run_stage(record);
            }
            let mut record = json!({ "pid": trace.process_id, "runId": self.id, "step": self.index,
                "kind": step.trace_kind(), "event": event });
            trace.waits.finish_step(&mut record);
            {
                use std::io::Write;
                let _ = std::io::stderr().lock().write_all(format!("pyro-run-wait {record}\n").as_bytes());
            }
            trace.emit(self, step.trace_kind(), event);
        }
    }

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
        self.trace_step_error();
        if self.single {
            return error;
        }
        let what = self.steps.get(self.index).map(|(s, _)| s.describe()).unwrap_or_default();
        let data = error.data;
        CdpError::server(format!("step {} ({what}): {}", self.index, error.message)).with_data(data)
    }
}

/// Where the current step is.
enum Phase {
    /// Tried; tried again when the document changes.
    Try,
    /// `goto` started; waits for the new document's `load` (or DOMContentLoaded).
    /// `navigation`: the step navigated (`goto`, `reload`), which fails when
    /// it commits an error document; a `loaded` step waits for any document.
    Load { loader: String, dom_ready: bool, navigation: bool },
    /// The step's action ran (or an `idle` step started); waits until the
    /// page is quiet, counting the requests started after the mark `after`
    /// (`Browser::request_mark`, taken before the action; 0 for all of them).
    /// `updates`: how many more times it may run the page's rendering update
    /// ahead of the frame interval ([`SETTLE_UPDATES`]).
    Settle { after: u64, since: Instant, until: Instant, updates: u8 },
    /// `release` let held replies go straight to the page (`requests`);
    /// waits until they are delivered, at most until `until`. The page then
    /// gets a turn to act on them.
    Deliver { requests: Vec<pyro_net::RequestId>, until: Instant },
    /// `eval` returned a promise; waits for it to settle.
    Await(Settlement),
    /// A `matchScreenshot` render runs on the render thread.
    Render(RenderSettlement),
    /// An action's input events still to dispatch (`seq`), one per
    /// event-loop turn; with none left, the turn after a non-settling step.
    /// A turn: the frame interval passes (`at`), then a marker task posted
    /// behind the tasks and timers due by then (`hop`) runs. `before`: the
    /// request mark taken before the action, when it settles.
    Act { seq: Option<Box<Sequence>>, before: Option<u64>, hop: Option<Rc<Cell<bool>>>, at: Instant },
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
/// `pyro_browser::note_client_activity`).
const TURN: Duration = Duration::from_millis(1);

/// How many times one settle runs the page's rendering update at once when
/// that is all the page still waits for (`Browser::update_rendering`): its
/// queued `scroll` events fire and its ResizeObservers gather. Both are paced
/// (16 ms or more apart) so that a busy page does not lay out after every
/// task; a page with nothing else to do would only sleep the interval out,
/// once per round of handlers that scroll or resize something again. Past
/// this many rounds (observers that keep resizing each other, a scroll
/// handler that keeps scrolling) the pacing applies again.
const SETTLE_UPDATES: u8 = 4;

/// Where a mouse event of a [`Sequence`] goes.
#[derive(Clone, Copy)]
enum At {
    /// The located element's center, computed when the event is dispatched
    /// (hover-dependent layout can shift it between events); once the element
    /// left the document, the last point.
    Element,
    /// The last point.
    Here,
    /// A press's release: the last point while the pressed element is still
    /// under it. When page work that ran between the press and the release
    /// (a list above it finished loading) moved the element away, the
    /// element's new center instead, so the click stays on what was pressed;
    /// when something now covers it, the last point.
    Release,
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

#[derive(Default)]
struct PointerStability {
    frame: Option<Rc<Cell<bool>>>,
    /// The box at the last frame that saw it change, and when.
    previous: Option<(pyro_dom::layout::Rect, Instant)>,
    ready: bool,
    /// How long the box must stay the same: [`STABLE_SPAN`] before the first
    /// move, nothing beyond two frames alike before the press after it.
    span: Duration,
}

impl PointerStability {
    /// Before the first move: Playwright waits for a box that is the same in
    /// two consecutive rendering frames (about 16 ms apart). Frames run back
    /// to back here while a client drives the page, so a box counts as stable
    /// only once it stayed put for a frame's time: a sheet sliding in is
    /// hovered and clicked where it stops, not on its way.
    fn before_move() -> PointerStability {
        PointerStability { span: STABLE_SPAN, ..Default::default() }
    }
}

/// See [`PointerStability::before_move`].
const STABLE_SPAN: Duration = Duration::from_millis(16);

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
    stability: Option<PointerStability>,
    /// The ResizeObserver deliveries counted when the press first waited on
    /// resize work.
    resize_round: Option<u64>,
}

impl Sequence {
    fn new(
        page: &Rc<PageDom>,
        id: NodeId,
        opts: PointerOpts,
        m: MouseInput,
        events: impl IntoIterator<Item = Input>,
    ) -> Sequence {
        Sequence {
            page: page.clone(),
            id,
            opts,
            m,
            events: events.into_iter().collect(),
            drag: None,
            stability: None,
            resize_round: None,
        }
    }

    /// Dispatches the next event. `Ok(None)`: more follow; `Ok(Some)`: the
    /// action's result. `Err`: why the step must be tried again.
    fn advance(&mut self) -> Result<Option<Value>, String> {
        let page = self.page.clone();
        if page.is_disposed() {
            return Err("document is gone".into());
        }
        // The sizes the current layout changed are delivered before the
        // press, as Chromium's rendering update does before input reaches
        // the page. One delivered round settles them: observers that resize
        // something again in every round (Chromium delivers once per frame
        // and never holds input for them) do not hold the press.
        if !self.opts.force
            && matches!(self.events.front(), Some(Input::Mouse { kind: MouseKind::Pressed, at: At::Element, .. }))
            && page.pending_resize_observer_work()
        {
            let delivered = page.resize_observer_deliveries();
            let first = *self.resize_round.get_or_insert(delivered);
            if delivered == first {
                page.request_resize_observer_update_for_input();
                return Ok(None);
            }
        }
        // A stable box before the first move (`PointerStability::before_move`),
        // and again before the press: what the move's hover handlers render
        // (a control swapped in, a menu that shifts it) lands first.
        if matches!(
            self.events.front(),
            Some(Input::Mouse { kind: MouseKind::Moved | MouseKind::Pressed, at: At::Element, .. })
        ) && !self.stable_before_press()?
        {
            return Ok(None);
        }
        let Some(next) = self.events.pop_front() else { return Ok(Some(self.result())) };
        if matches!(next, Input::Mouse { kind: MouseKind::Moved, at: At::Element, .. }) {
            if let Some(stability) = &mut self.stability {
                *stability = PointerStability::default();
            }
        }
        // The driver is the client driving the page: frames stay on the
        // short interval while a sequence runs.
        pyro_browser::note_client_activity();
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
                    At::Release => {
                        let connected = page.dom.borrow().is_connected(self.id).unwrap_or(false);
                        // Hit testing lays the page out: no DOM borrow across it.
                        let hit =
                            if connected { pyro_webapi::layout_api::hit_test(&page, self.m.x, self.m.y) } else { None };
                        let moved = connected && !hit.is_some_and(|hit| contains(&page.dom.borrow(), self.id, hit));
                        if moved {
                            if let Ok((x, y)) = pointer_target(&page, self.id, self.opts) {
                                self.m.x = x;
                                self.m.y = y;
                            }
                        }
                    }
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

    fn stable_before_press(&mut self) -> Result<bool, String> {
        let Some(stability) = &mut self.stability else { return Ok(true) };
        if stability.ready || self.opts.force {
            return Ok(true);
        }
        if let Some(frame) = &stability.frame {
            if !frame.get() {
                return Ok(false);
            }
            let connected = self.page.dom.borrow().is_connected(self.id).unwrap_or(false);
            let target = if connected {
                super::dom::reveal(&self.page, self.id);
                self.id
            } else {
                pyro_webapi::layout_api::hit_test(&self.page, self.m.x, self.m.y).ok_or("no replacement at pointer")?
            };
            let bounds = visible_box(&self.page, target).ok_or("element is not visible")?;
            let now = Instant::now();
            // Only a moving animation or transition can move the box between
            // frames with no DOM change; without one, two frames alike suffice.
            let span = if stability.span.is_zero() || self.page.moving_animation_frame_interval().is_none() {
                Duration::ZERO
            } else {
                stability.span
            };
            match stability.previous {
                Some((previous, at)) if previous == bounds && now.duration_since(at) >= span => {
                    stability.ready = true;
                    return Ok(true);
                }
                Some((previous, _)) if previous == bounds => {}
                _ => stability.previous = Some((bounds, now)),
            }
        }
        let realm = self.page.main_world_realm().ok_or("page has no realm")?;
        stability.frame = Some(pyro_browser::request_native_frame(&realm).map_err(|error| error.to_string())?);
        Ok(false)
    }

    /// The animation frame the press waits on for its stability check, while
    /// it has not run.
    fn stability_frame(&self) -> Option<Rc<Cell<bool>>> {
        let stability = self.stability.as_ref().filter(|s| !s.ready && !self.opts.force)?;
        stability.frame.clone().filter(|frame| !frame.get())
    }

    fn wait_diagnostics(&self) -> String {
        let press_next = matches!(self.events.front(), Some(Input::Mouse { kind: MouseKind::Pressed, .. }));
        let stability = self
            .stability
            .as_ref()
            .map(|state| (state.ready, state.frame.as_ref().map(|frame| frame.get()), state.previous));
        let (observers, queued, wake, next) = self.page.resize_observer_work_state();
        format!(
            "press_next={press_next}, resize_pending={}, resize_observers={observers}, resize_callback_queued={queued}, resize_wake_queued={wake}, resize_next_gather_ms={:?}, stability(ready,frame_done,previous)={stability:?}",
            self.page.pending_resize_observer_work(),
            next.map(|delay| delay.as_secs_f64() * 1000.0),
        )
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
/// How soon an action step whose element was not ready is tried again when
/// nothing in the DOM changes (see [`EngineHost::park_retry`]).
const ACT_POLL: Duration = Duration::from_millis(100);

/// The command running on the browser thread now (a page command, or a
/// `Pyro.run` driven between commands), shared with the connection's
/// watchdog check (conn.rs `check_during_script`): a `Pyro.cancelRun` for
/// its session that arrives while its script runs stops the script, which
/// cannot finish on its own (`while (true) {}`), and the run then answers
/// that it was cancelled.
#[derive(Default)]
pub(crate) struct RunningCommand {
    /// Session and command id running now; `None` between commands.
    now: RefCell<Option<(String, u64)>>,
    /// The command whose script a `Pyro.cancelRun` stopped, until that
    /// cancel is handled.
    stopped: RefCell<Option<(String, u64)>>,
}

/// Restores the command that was running before ([`RunningCommand::enter`]).
pub(super) struct RunningGuard {
    running: Rc<RunningCommand>,
    previous: Option<(String, u64)>,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        *self.running.now.borrow_mut() = self.previous.take();
    }
}

impl RunningCommand {
    /// `session`'s command `id` runs until the guard drops. Nests: a run
    /// driven while a command flushes puts the command back when it is done.
    pub(super) fn enter(self: &Rc<Self>, session: &str, id: u64) -> RunningGuard {
        let previous = self.now.replace(Some((session.to_owned(), id)));
        RunningGuard { running: self.clone(), previous }
    }

    /// Whether `text`, a message read while a script runs, is a
    /// `Pyro.cancelRun` for the session whose command runs that script: the
    /// script is then stopped, and the command remembered as stopped. Only
    /// a message naming the method is parsed.
    pub(crate) fn cancels(&self, text: &str) -> bool {
        if !text.contains("\"Pyro.cancelRun\"") {
            return false;
        }
        let Some(now) = self.now.borrow().clone() else { return false };
        #[derive(Deserialize)]
        struct Message {
            method: String,
            #[serde(rename = "sessionId")]
            session_id: Option<String>,
        }
        let Ok(m) = serde_json::from_str::<Message>(text) else { return false };
        if m.method != "Pyro.cancelRun" || m.session_id.as_deref() != Some(now.0.as_str()) {
            return false;
        }
        *self.stopped.borrow_mut() = Some(now);
        true
    }

    /// Whether a cancel stopped the script of the command running now.
    fn stopped_now(&self) -> bool {
        let now = self.now.borrow();
        now.is_some() && *self.stopped.borrow() == *now
    }

    /// The id of `session`'s command whose script a cancel stopped, taken.
    fn take_stopped(&self, session: &str) -> Option<u64> {
        let mut stopped = self.stopped.borrow_mut();
        match stopped.as_ref() {
            Some((s, _)) if s == session => stopped.take().map(|(_, id)| id),
            _ => None,
        }
    }
}

impl Waiter {
    /// What a cancelled run answers (`Pyro.cancelRun`): that it was
    /// cancelled, at which step, and what the step was still waiting for, as
    /// a timeout would report it; or, `stopped`, that a script it ran was
    /// still running (and was stopped).
    fn cancelled(&self, browser: &Browser, stopped: bool) -> CdpError {
        let step = self.job.steps.get(self.job.index).map(|(s, _)| s.describe()).unwrap_or_default();
        let waiting = match &self.phase {
            _ if stopped => format!("{step}: the script it ran was still running and was stopped"),
            Phase::Try if self.reason.is_empty() => step,
            Phase::Try => format!("{step}: {}", self.reason),
            Phase::Load { dom_ready, .. } => {
                format!("waiting for {}", if *dom_ready { "DOMContentLoaded" } else { "load" })
            }
            Phase::Settle { after, .. } => match browser.page_work_since(self.job.pid, *after) {
                Some(work) if !work.quiet() => {
                    format!("waiting for the page to go quiet (pending {})", pending_json(&work))
                }
                _ => "waiting for the page to go quiet".into(),
            },
            Phase::Deliver { .. } => "waiting for the replies it released to reach the page".into(),
            Phase::Await(_) => "waiting for the promise to settle".into(),
            Phase::Render(_) => "waiting for the screenshot".into(),
            Phase::Act { seq: Some(_), .. } => format!("{step}: dispatching its input events"),
            Phase::Act { seq: None, .. } => "waiting for the event-loop turn after the step".into(),
        };
        self.job.fail(CdpError::server(format!("cancelled: {waiting}")))
    }

    fn deadline(&self) -> Instant {
        let step = match &self.phase {
            Phase::Settle { until, .. } | Phase::Deliver { until, .. } => *until,
            Phase::Act { hop: None, at, .. } => *at,
            _ => self.step_deadline,
        };
        match self.poll_at {
            Some(at) if matches!(self.phase, Phase::Try | Phase::Deliver { .. }) => step.min(at),
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

    /// `PYRO_RUN_STEP_TRACE`: the run is parked from `now` on (or still is,
    /// one loop task later); what it waits in and what the page has pending.
    fn trace_wait(&self, browser: &Browser, now: Instant) {
        let Some(trace) = &self.job.trace else { return };
        let (phase, after) = match &self.phase {
            Phase::Try => ("try", 0),
            Phase::Load { .. } => ("load", 0),
            Phase::Settle { after, .. } => ("settle", *after),
            Phase::Deliver { .. } => ("deliver", 0),
            Phase::Await(_) => ("await", 0),
            Phase::Render(_) => ("render", 0),
            Phase::Act { seq: None, .. } => ("turn", 0),
            Phase::Act { .. } => ("act", 0),
        };
        trace.waits.mark(Some((phase, waiting_on(browser.page_work_since(self.job.pid, after)))), now);
    }

    /// Whether driving it can make progress.
    fn ready(&self, browser: &Browser, now: Instant) -> bool {
        if now >= self.deadline() {
            return true;
        }
        match &self.phase {
            Phase::Try if self.waits_on_log() => false,
            Phase::Try if self.poll_at.is_some_and(|at| now >= at) => true,
            // An action step also goes again as soon as the DOM changes.
            Phase::Try
                if self.poll_at.is_some()
                    && !matches!(self.job.steps.get(self.job.index), Some((Step::Act { .. }, _))) =>
            {
                false
            }
            Phase::Try => match self.doc.upgrade() {
                Some(doc) => doc.is_disposed() || doc.dom.borrow().version() != self.seen,
                None => true,
            },
            Phase::Load { loader, dom_ready, navigation } => {
                load_reached(browser, self.job.pid, loader, *dom_ready, *navigation)
            }
            // Quiet, or held by nothing but a paced rendering update that
            // driving it brings forward.
            Phase::Settle { after, updates, .. } => browser
                .page_work_since(self.job.pid, *after)
                .is_none_or(|w| w.quiet() || (*updates > 0 && w.only_rendering())),
            Phase::Deliver { requests, .. } => !undelivered(browser, self.job.pid, requests),
            Phase::Await(settled) => settled.borrow().is_some(),
            Phase::Render(settled) => settled.borrow().is_some(),
            Phase::Act { hop, at, seq, .. } => match hop {
                // A stability frame never runs in a document that went away.
                Some(done) => done.get() || seq.as_ref().is_some_and(|s| s.page.is_disposed()),
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
        for w in &self.list {
            w.trace_wait(browser, now);
        }
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
                    && browser.page_work(w.job.pid).is_some_and(|work| {
                        work.tasks == 0
                            && work.requests == 0
                            && work.event_streams == 0
                            && work.observers == 0
                            && work.scrolls == 0
                            && work.navigations == 0
                    })
            })
    }

    pub(super) fn clear(&mut self) {
        self.list.clear();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Where a `Phase::Load` wait for `loader`'s document stands.
enum LoadState {
    /// Fetching or loading.
    Waiting,
    Reached,
    /// Another navigation superseded it before it committed: wait for that one.
    Follow(String),
    /// Neither committed nor pending: the navigation failed. Or, for a
    /// navigation, it committed an error document.
    Failed,
}

fn load_state(browser: &Browser, pid: PageId, loader: &str, dom_ready: bool, navigation: bool) -> LoadState {
    let Some(p) = browser.page(pid) else { return LoadState::Reached };
    let f = &p.main_frame;
    if f.loader_id.0 == loader {
        // A new window's failed navigation commits Chrome's error document
        // under its loader: the navigation failed all the same.
        if navigation && f.unreachable.is_some() {
            return LoadState::Failed;
        }
        let done = if dom_ready { f.dom_content_loaded } else { f.loaded };
        return if done { LoadState::Reached } else { LoadState::Waiting };
    }
    match p.pending_loader() {
        Some(pending) if pending.0 == loader => LoadState::Waiting,
        Some(pending) => LoadState::Follow(pending.0.clone()),
        None => LoadState::Failed,
    }
}

fn load_reached(browser: &Browser, pid: PageId, loader: &str, dom_ready: bool, navigation: bool) -> bool {
    !matches!(load_state(browser, pid, loader, dom_ready, navigation), LoadState::Waiting)
}

/// The document a `loaded` step waits for: the navigation being fetched
/// (a prepared page taken while booting), else the current document.
fn loader_to_wait_for(browser: &Browser, pid: PageId) -> String {
    let Some(p) = browser.page(pid) else { return String::new() };
    p.pending_loader().map_or_else(|| p.main_frame.loader_id.0.clone(), |l| l.0.clone())
}

/// Whether one of `requests` (replies a `release` let through to the page)
/// has yet to be handed to the page: its completion is queued on the page's
/// loop, if it has not run already, once this is false.
fn undelivered(browser: &Browser, pid: PageId, requests: &[pyro_net::RequestId]) -> bool {
    let net = browser.page(pid).and_then(|p| browser.context_net(p.context));
    net.is_some_and(|net| requests.iter().any(|id| net.in_flight(*id)))
}

/// `{"tasks": 1, "requests": 2, ...}`: the kinds of work still pending.
fn pending_json(work: &pyro_browser::PageWork) -> Value {
    let all = [
        ("tasks", work.tasks),
        ("requests", work.requests),
        ("timers", work.timers),
        ("frames", work.frames),
        ("observers", work.observers),
        ("scrolls", work.scrolls),
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

pub(super) fn normalize(s: &str) -> String {
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

pub(super) fn local(dom: &Dom, id: NodeId) -> Option<&str> {
    match &dom.node(id).ok()?.kind {
        NodeKind::Element(e) if e.is_html() => Some(&e.name.local),
        _ => None,
    }
}

pub(super) fn attr<'d>(dom: &'d Dom, id: NodeId, name: &str) -> Option<&'d str> {
    dom.element(id).ok()?.attr(name)
}

/// A JavaScript value as JSON (`null` when it has no JSON form).
fn json_of(v: pyro_js::Value<'_>) -> Value {
    v.to_json(0).ok().flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or(Value::Null)
}

pub(super) fn text_of(dom: &Dom, id: NodeId) -> String {
    dom.text_content(id).ok().flatten().unwrap_or_default()
}

fn elements(dom: &Dom, root: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    dom.descendants(root).into_iter().flatten().filter(|n| dom.element(*n).is_ok())
}

/// `aria-*` boolean attribute: `true` case-insensitively.
pub(super) fn aria_true(dom: &Dom, id: NodeId, name: &str) -> bool {
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
pub(super) fn id_refs(dom: &Dom, doc: NodeId, id: NodeId, name: &str) -> Option<Vec<NodeId>> {
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

/// Playwright's `validRoles`: the roles an explicit `role` may name.
const VALID_ROLES: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "mark",
    "marquee",
    "math",
    "meter",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
];

/// Roles that may not be named: Playwright's prohibited roles for
/// `aria-label` / `aria-labelledby` in `kGlobalAriaAttributes`.
const LABEL_PROHIBITED: &[&str] = &[
    "caption",
    "code",
    "deletion",
    "emphasis",
    "generic",
    "insertion",
    "paragraph",
    "presentation",
    "strong",
    "subscript",
    "superscript",
];

/// Playwright's `hasGlobalAriaAttribute(element, forRole)`.
fn has_global_aria(dom: &Dom, id: NodeId, role: &str) -> bool {
    const ANY: &[&str] = &[
        "aria-atomic",
        "aria-busy",
        "aria-controls",
        "aria-current",
        "aria-describedby",
        "aria-details",
        "aria-dropeffect",
        "aria-flowto",
        "aria-grabbed",
        "aria-hidden",
        "aria-keyshortcuts",
        "aria-live",
        "aria-owns",
        "aria-relevant",
    ];
    ANY.iter().any(|a| attr(dom, id, a).is_some())
        || (!LABEL_PROHIBITED.contains(&role)
            && (attr(dom, id, "aria-label").is_some() || attr(dom, id, "aria-labelledby").is_some()))
        || (role != "generic" && attr(dom, id, "aria-roledescription").is_some())
}

/// JavaScript's `Number(s)` for an attribute value: whitespace trimmed, ""
/// is 0, hex/octal/binary prefixes and exponents allowed, NaN otherwise.
pub(super) fn js_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = t.strip_prefix(prefix) {
            return if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) {
                digits.chars().fold(0.0, |acc, c| acc * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64)
            } else {
                f64::NAN
            };
        }
    }
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned == "Infinity" {
        return if t.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    // A decimal literal: digits, an optional fraction, an optional exponent.
    let b = unsigned.as_bytes();
    let mut i = 0;
    let int = b.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac;
    }
    if int + frac == 0 {
        return f64::NAN;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp == 0 {
            return f64::NAN;
        }
        i += exp;
    }
    if i != b.len() {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// Playwright's `isNativelyDisabled`: a native form control that is
/// disabled, or in a disabled `<optgroup>` / `<fieldset>`.
pub(super) fn natively_disabled(dom: &Dom, id: NodeId) -> bool {
    if !matches!(local(dom, id), Some("button" | "input" | "select" | "textarea" | "option" | "optgroup")) {
        return false;
    }
    dom.is_disabled(id).unwrap_or(false) || attr(dom, id, "disabled").is_some()
}

/// Playwright's `isFocusable`: not natively disabled, and natively
/// focusable or with a numeric `tabindex`.
fn pw_focusable(dom: &Dom, id: NodeId) -> bool {
    if natively_disabled(dom, id) {
        return false;
    }
    let native = match local(dom, id) {
        Some("button" | "details" | "select" | "textarea") => true,
        Some("a" | "area") => attr(dom, id, "href").is_some(),
        Some("input") => attr(dom, id, "hidden").is_none(),
        _ => false,
    };
    native || attr(dom, id, "tabindex").is_some_and(|t| !js_number(t).is_nan())
}

/// Playwright's `getAriaRole`: the first valid token of an explicit `role`
/// (case-sensitive), else the implicit role. `none` / `presentation` give
/// way to the implicit role on a focusable element or one with a global
/// `aria-*` attribute (`hasPresentationConflictResolution`).
pub(super) fn role_of(dom: &Dom, id: NodeId) -> Option<String> {
    let explicit = attr(dom, id, "role").and_then(|r| r.split(' ').map(str::trim).find(|t| VALID_ROLES.contains(t)));
    match explicit {
        Some(r @ ("none" | "presentation")) => {
            let implicit = implicit_role(dom, id);
            let conflict = implicit.as_deref().unwrap_or("");
            if has_global_aria(dom, id, conflict) || pw_focusable(dom, id) {
                implicit
            } else {
                Some(r.into())
            }
        }
        Some(r) => Some(r.into()),
        None => implicit_role(dom, id),
    }
}

/// Implicit ARIA role, by Playwright's `kImplicitRoleByTagName`.
fn implicit_role(dom: &Dom, id: NodeId) -> Option<String> {
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
        // Playwright's kImplicitRoleByTagName: SVG is an img.
        "svg" => "img",
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
            // Playwright: "All browsers report it as a button" (named "Choose File").
            "button" | "submit" | "reset" | "image" | "file" => "button",
            "checkbox" => "checkbox",
            "radio" => "radio",
            "range" => "slider",
            "number" => "spinbutton",
            "color" | "date" | "datetime-local" | "month" | "time" | "week" => return None,
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
pub(super) fn labels(dom: &Dom, doc: NodeId, id: NodeId) -> Vec<NodeId> {
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
/// not rendered (`display:none`, `visibility:hidden`), light-DOM children of
/// a shadow host assigned to no slot, and anything inside an
/// `aria-hidden="true"` or `inert` subtree. `ancestors` walks up (through
/// shadow hosts) for the subtree rules; a descendant traversal has already
/// passed its parents.
pub(super) fn hidden_for_aria(dom: &Dom, layout: &Layout, id: NodeId, ancestors: bool) -> bool {
    if matches!(local(dom, id), Some("style" | "script" | "noscript" | "template")) {
        return true;
    }
    let style = layout.style();
    let select = (local(dom, id) == Some("option"))
        .then(|| self_and_ancestors(dom, id).find(|a| local(dom, *a) == Some("select")));
    if let Some(Some(select)) = select {
        // Playwright's `isOptionInsideSelect` skips only the box and
        // visibility check (an option of a drop-down select has no box):
        // its own `display: none` (the `hidden` attribute, author CSS) or an
        // ancestor's still hides it.
        if style.has_display_none(dom, id) {
            return true;
        }
        // Up the tree only display:none counts (belongsToDisplayNoneOrAriaHidden),
        // not whether the select itself has a box.
        let _ = select;
        if ancestors && aria_ancestors(dom, id).skip(1).any(|a| style.has_display_none(dom, a)) {
            return true;
        }
    } else {
        // Chromium gives an optgroup of a drop-down <select> no box
        // (`checkVisibility()` is false), and Playwright has no exception
        // for it: it and its options are left out.
        if local(dom, id) == Some("optgroup") && closed_select(dom, id) {
            return true;
        }
        if ancestors {
            if !layout.is_visible(dom, id) {
                return true;
            }
        } else if style.has_display_none(dom, id) {
            return true;
        }
        if style.has_visibility_hidden_inherited(dom, id) {
            return true;
        }
    }
    // No `inert` rule: Playwright's isElementHiddenForAria has none, and
    // `checkVisibility()` ignores inert, so getByRole finds inert content.
    let hidden = |n: NodeId| aria_true(dom, n, "aria-hidden") || non_slotted(dom, n);
    if ancestors {
        aria_ancestors(dom, id).any(hidden)
    } else {
        hidden(id)
    }
}

/// A light-DOM child of a shadow host that no slot shows
/// (Playwright's `belongsToDisplayNoneOrAriaHiddenOrNonSlotted`).
fn non_slotted(dom: &Dom, id: NodeId) -> bool {
    dom.parent_element(id).ok().flatten().is_some_and(|p| dom.shadow_root(p).ok().flatten().is_some())
        && dom.assigned_slot(id).ok().flatten().is_none()
}

/// An `<optgroup>` of a drop-down (combobox) `<select>`.
fn closed_select(dom: &Dom, id: NodeId) -> bool {
    self_and_ancestors(dom, id).find(|a| local(dom, *a) == Some("select")).is_some_and(|s| {
        let sized = attr(dom, s, "size").is_some_and(|v| v.trim().parse::<u32>().is_ok_and(|n| n > 1));
        attr(dom, s, "multiple").is_none() && !sized
    })
}

/// `id` and its ancestors, crossing from a shadow root to its host
/// (Playwright's `parentElementOrShadowHost`).
fn aria_ancestors(dom: &Dom, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    std::iter::successors(Some(id), move |n| {
        let p = dom.parent(*n).ok().flatten()?;
        if dom.element(p).is_ok() {
            Some(p)
        } else {
            dom.fragment_host(p).ok().flatten().filter(|_| dom.is_shadow_root(p).unwrap_or(false))
        }
    })
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
    /// Whether any element may have a generated `::before` / `::after`
    /// (asked on first use, once per [`PseudoContent`] scope: `false` skips
    /// the per-element lookups).
    pseudo_content: &'a PseudoContent,
}

impl Names<'_> {
    /// Playwright's `getPseudoContent` of `id`'s `pseudo`; "" without a
    /// generated one.
    fn pseudo_content(&self, id: NodeId, pseudo: &pyro_dom::stylo::PseudoElement) -> String {
        let any = *self.pseudo_content.get_or_init(|| self.layout.style().may_have_generated_before_after(self.dom));
        if !any {
            return String::new();
        }
        self.layout.pseudo_name_content(self.dom, id, pseudo)
    }

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
        // 2E: native text alternatives. A form control is visited before its
        // labels are read, as in Playwright, so a label wrapping it does not
        // read the control's own placeholder or title into its name.
        if matches!(tag, "input" | "textarea" | "select" | "button" | "output") {
            self.visited.borrow_mut().insert(id);
        }
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
                    // Playwright's file input: its labels, else the button's text.
                    "file" => {
                        if opts.labelledby.is_none() {
                            if let Some(name) = from_labels() {
                                return Some(name);
                            }
                        }
                        return Some("Choose File".into());
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
            // Playwright has no branch for <meter> / <progress>: their labels
            // never name them; they fall through to content and title.
            "select" if !has_labelledby => Some(from_labels().or_else(|| attr_text("title")).unwrap_or_default()),
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
        // Generated `::before` / `::after` text counts (Playwright's
        // `getPseudoContent`); the element's own `content` alternative text
        // replaces its children.
        let mut out = self.pseudo_content(id, &pyro_dom::stylo::PseudoElement::Before);
        if let Some(content) = self.layout.style().css_content(dom, id, None) {
            out.push_str(&content);
            out.push_str(&self.pseudo_content(id, &pyro_dom::stylo::PseudoElement::After));
            return out;
        }
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
        out.push_str(&self.pseudo_content(id, &pyro_dom::stylo::PseudoElement::After));
        out
    }
}

/// Whether the document may have a generated `::before` / `::after`, asked
/// once for every name computed with it (one DOM state: no mutation between
/// the names).
pub(super) type PseudoContent = std::cell::OnceCell<bool>;

/// The accessible name of an element, whitespace-normalized.
pub(super) fn accessible_name(dom: &Dom, layout: &Layout, doc: NodeId, id: NodeId) -> String {
    accessible_name_in(dom, layout, doc, id, &PseudoContent::new())
}

/// [`accessible_name`] for one of many elements named in one DOM state,
/// sharing `pseudo_content`.
pub(super) fn accessible_name_in(
    dom: &Dom,
    layout: &Layout,
    doc: NodeId,
    id: NodeId,
    pseudo_content: &PseudoContent,
) -> String {
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
    let names = Names { dom, layout, doc, visited: RefCell::new(HashSet::new()), pseudo_content };
    normalize(&names.alternative(id, NameOpts { target: Target::Own, ..NameOpts::default() }))
}

/// Playwright's `shouldSkipForTextMatching`: script, noscript and style
/// elements and everything in `<head>` have no text for text matching.
fn skipped_for_text(dom: &Dom, id: NodeId) -> bool {
    matches!(local(dom, id), Some("script" | "noscript" | "style"))
        || self_and_ancestors(dom, id).any(|a| local(dom, a) == Some("head"))
}

/// Playwright's `elementText`: the element's full text (its text nodes and
/// its child elements' texts, skipped elements contributing nothing; an open
/// shadow root's text after the children; the value of an `<input>` button)
/// and the runs of its own text nodes (`immediate`:
/// `<span><span>|</span>TS-1</span>` has `["TS-1"]`).
#[derive(Clone, Default)]
struct ElementText {
    full: String,
    normalized: String,
    immediate: Vec<String>,
}

#[derive(Default)]
struct TextCache(HashMap<NodeId, Rc<ElementText>>);

impl TextCache {
    fn get(&mut self, dom: &Dom, id: NodeId) -> Rc<ElementText> {
        if let Some(t) = self.0.get(&id) {
            return t.clone();
        }
        let mut value = ElementText::default();
        let is_element = dom.element(id).is_ok();
        if !(is_element && skipped_for_text(dom, id)) {
            let input_button = local(dom, id) == Some("input")
                && matches!(
                    dom.input_type(id),
                    Ok(pyro_dom::forms::InputType::Submit
                        | pyro_dom::forms::InputType::Button
                        | pyro_dom::forms::InputType::Reset)
                );
            if input_button {
                let v = dom.value(id).unwrap_or_default();
                value.immediate = vec![v.clone()];
                value.full = v;
            } else {
                let mut run = String::new();
                for c in dom.children(id).into_iter().flatten() {
                    match dom.node(c).map(|n| &n.kind) {
                        Ok(NodeKind::Text(t) | NodeKind::CData(t)) => {
                            value.full.push_str(t);
                            run.push_str(t);
                        }
                        Ok(NodeKind::Comment(_)) => {}
                        Ok(kind) => {
                            let element = matches!(kind, NodeKind::Element(_));
                            if !run.is_empty() {
                                value.immediate.push(std::mem::take(&mut run));
                            }
                            if element {
                                let child = self.get(dom, c);
                                value.full.push_str(&child.full);
                            }
                        }
                        Err(_) => {}
                    }
                }
                if !run.is_empty() {
                    value.immediate.push(run);
                }
                if is_element {
                    if let Some(sr) = open_shadow_root(dom, id) {
                        let shadow = self.get(dom, sr);
                        value.full.push_str(&shadow.full);
                    }
                }
            }
            value.normalized = normalize(&value.full);
        }
        let value = Rc::new(value);
        self.0.insert(id, value.clone());
        value
    }
}

fn open_shadow_root(dom: &Dom, host: NodeId) -> Option<NodeId> {
    let sr = dom.shadow_root(host).ok().flatten()?;
    (dom.shadow_root_mode(sr).ok().flatten() == Some(pyro_dom::ShadowRootMode::Open)).then_some(sr)
}

/// A text locator's matcher, as Playwright's `createTextMatcher`: a regex
/// (tested against the normalized text), `strict` (an exact string: the
/// whole normalized text, or for the legacy `text="…"` engine one run of
/// the element's own text nodes) or `lax` (a case-insensitive substring).
struct TextMatcher<'a> {
    want: &'a Pattern,
    strict: bool,
    legacy: bool,
    normalized_want: String,
}

impl<'a> TextMatcher<'a> {
    fn new(want: &'a Pattern, exact: bool, legacy: bool) -> Self {
        let strict = exact && want.regex.is_none();
        let n = normalize(&want.text);
        TextMatcher {
            want,
            strict,
            legacy,
            normalized_want: if strict || want.regex.is_some() { n } else { n.to_lowercase() },
        }
    }

    fn lax(&self) -> bool {
        !self.strict && self.want.regex.is_none()
    }

    fn matches(&self, t: &ElementText) -> bool {
        if let Some(re) = &self.want.regex {
            return re.find(&t.normalized).is_some();
        }
        if !self.strict {
            return t.normalized.to_lowercase().contains(&self.normalized_want);
        }
        if !self.legacy {
            return t.normalized == self.normalized_want;
        }
        if self.normalized_want.is_empty() && t.immediate.is_empty() {
            return true;
        }
        t.immediate.iter().any(|run| normalize(run) == self.normalized_want)
    }
}

#[derive(PartialEq, Eq)]
enum TextHit {
    None,
    OnlySelf,
    SelfAndChildren,
}

/// Playwright's `elementMatchesText`.
fn element_matches_text(dom: &Dom, cache: &mut TextCache, id: NodeId, m: &TextMatcher<'_>) -> TextHit {
    if skipped_for_text(dom, id) || !m.matches(&cache.get(dom, id)) {
        return TextHit::None;
    }
    for c in dom.children(id).into_iter().flatten() {
        if dom.element(c).is_ok() && m.matches(&cache.get(dom, c)) {
            return TextHit::SelfAndChildren;
        }
    }
    if let Some(sr) = open_shadow_root(dom, id) {
        if m.matches(&cache.get(dom, sr)) {
            return TextHit::SelfAndChildren;
        }
    }
    TextHit::OnlySelf
}

/// Playwright's text engine over `root` (a candidate itself) and the
/// elements inside it: an element matches when its text does and no child
/// element's text does (the legacy `text="…"` engine also keeps strict
/// matches whose children match too). Lax matching skips the subtrees of
/// elements that do not match.
fn by_text(dom: &Dom, root: NodeId, want: &Pattern, exact: bool, legacy: bool, out: &mut Vec<NodeId>) {
    let m = TextMatcher::new(want, exact, legacy);
    let mut cache = TextCache::default();
    let mut last_miss: Option<NodeId> = None;
    for el in std::iter::once(root).chain(elements(dom, root).filter(|n| *n != root)) {
        if m.lax() && last_miss.is_some_and(|miss| contains(dom, miss, el)) {
            continue;
        }
        match element_matches_text(dom, &mut cache, el, &m) {
            TextHit::None => last_miss = Some(el),
            TextHit::OnlySelf => out.push(el),
            TextHit::SelfAndChildren if m.strict && legacy => out.push(el),
            TextHit::SelfAndChildren => {}
        }
    }
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
            self.xpath.is_some(),
            self.of.is_some(),
            self.test_id.is_some(),
            self.text.is_some(),
            self.role.is_some(),
            self.label.is_some(),
            self.placeholder.is_some(),
            self.aria_ref.is_some(),
        ];
        if kinds.iter().filter(|k| **k).count() != 1 {
            return Err(CdpError::invalid_params(
                "locator needs exactly one of css, xpath, testId, text, role, label, placeholder, ariaRef, of",
            ));
        }
        if self.has_role_states() && self.role.is_none() {
            return Err(CdpError::invalid_params(
                "checked, pressed, disabled, expanded, selected and level need a role",
            ));
        }
        for l in self.within.iter().chain(self.of.iter()).map(|b| &**b).chain(self.or.iter()).chain(self.and.iter()) {
            l.validate()?;
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
        if let Some(of) = &self.of {
            let mut seen = HashSet::new();
            for root in roots {
                found.extend(of.resolve_at(page, selectors, root)?.into_iter().filter(|n| seen.insert(*n)));
            }
        } else if let [root] = roots[..] {
            found = self.matches_in(page, selectors, root)?;
        } else {
            let mut seen = HashSet::new();
            for root in roots {
                found.extend(self.matches_in(page, selectors, root)?.into_iter().filter(|n| seen.insert(*n)));
            }
        }
        if !self.or.is_empty() {
            let mut all: HashSet<NodeId> = found.iter().copied().collect();
            for other in &self.or {
                all.extend(other.resolve_at(page, selectors, base)?);
            }
            let dom = page.dom.borrow();
            found = dom.descendants(base).into_iter().flatten().filter(|n| all.contains(n)).collect();
        }
        for other in &self.and {
            let keep: HashSet<NodeId> = other.resolve_at(page, selectors, base)?.into_iter().collect();
            found.retain(|n| keep.contains(n));
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
        } else if let Some(xpath) = &self.xpath {
            let value = pyro_dom::xpath::evaluate(&dom, xpath, pyro_dom::xpath::XPathNode::Node(root))
                .map_err(|e| CdpError::invalid_params(format!("invalid xpath: {e:?}")))?;
            let pyro_dom::xpath::XPathValue::NodeSet(nodes) = value else {
                return Err(CdpError::invalid_params("xpath must select nodes"));
            };
            found = nodes
                .into_iter()
                .filter_map(|n| match n {
                    pyro_dom::xpath::XPathNode::Node(id) if dom.element(id).is_ok() => Some(id),
                    _ => None,
                })
                .collect();
        } else if let Some(id) = &self.test_id {
            let name = self.test_id_attribute.as_deref().unwrap_or("data-testid");
            found = elements(&dom, root).filter(inside).filter(|n| attr(&dom, *n, name) == Some(id.as_str())).collect();
        } else if let Some(text) = &self.text {
            // As Playwright's text engine, the scope element itself is a
            // candidate: `css('td').text('Domestic', exact)` matches the cell.
            if root == doc {
                if let Ok(Some(html)) = dom.document_element(doc) {
                    by_text(&dom, html, text, self.exact, self.legacy_text, &mut found);
                }
            } else if dom.element(root).is_ok() {
                by_text(&dom, root, text, self.exact, self.legacy_text, &mut found);
            }
        } else if let Some(role) = &self.role {
            let role = role.to_ascii_lowercase();
            let layout = page.layout();
            let pseudo_content = PseudoContent::new();
            found = elements(&dom, root)
                .filter(inside)
                .filter(|n| role_of(&dom, *n).as_deref() == Some(role.as_str()))
                .filter(|n| self.include_hidden || !hidden_for_aria(&dom, &layout, *n, true))
                .filter(|n| self.role_states_hold(&dom, *n))
                .filter(|n| match &self.name {
                    Some(want) => {
                        want.matches(&accessible_name_in(&dom, &layout, doc, *n, &pseudo_content), self.exact)
                    }
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
        } else if let Some(r) = &self.aria_ref {
            // As Playwright's `aria-ref` engine: the scope is not searched,
            // the ref names the element.
            drop(dom);
            found = super::aria_snapshot::ref_node(page, r).into_iter().collect();
        }
        Ok(found)
    }
}

impl Filter {
    fn keeps(&self, page: &PageDom, selectors: &RefCell<SelectorCache>, n: NodeId) -> CdpResult<bool> {
        if let Some(want) = self.visible {
            if is_visible(page, n) != want {
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

fn valid_time_value(value: &str) -> bool {
    let (clock, fraction) = value.split_once('.').map_or((value, None), |(clock, fraction)| (clock, Some(fraction)));
    let parts: Vec<&str> = clock.split(':').collect();
    if !(2..=3).contains(&parts.len()) || (fraction.is_some() && parts.len() != 3) {
        return false;
    }
    if !parts.iter().enumerate().all(|(i, part)| {
        part.len() == 2
            && part.bytes().all(|b| b.is_ascii_digit())
            && part.parse::<u8>().is_ok_and(|n| n < if i == 0 { 24 } else { 60 })
    }) {
        return false;
    }
    fraction.is_none_or(|part| (1..=3).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
}

fn fillable(dom: &Dom, id: NodeId) -> bool {
    match local(dom, id) {
        Some("textarea") => attr(dom, id, "readonly").is_none(),
        Some("input") => {
            let ty = attr(dom, id, "type").unwrap_or("").to_ascii_lowercase();
            matches!(ty.as_str(), "" | "text" | "email" | "password" | "search" | "tel" | "url" | "number" | "time")
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
fn visible_box(page: &PageDom, id: NodeId) -> Option<pyro_dom::layout::Rect> {
    let dom = page.dom.borrow();
    page.layout().client_rect(&dom, id).filter(|r| r.width > 0.0 && r.height > 0.0)
}

/// Playwright's `isVisible` semantics (`isElementVisible`): a box with an
/// area, `visibility: visible` (an element under `visibility: hidden` or
/// `collapse` is not), a `display: contents` element through its children.
/// The `visible()` filter, `read("visible")`, the visible/hidden checks and
/// the actions' visible check use it.
fn is_visible(page: &PageDom, id: NodeId) -> bool {
    let layout = page.layout();
    super::aria_snapshot::box_visible(&page.dom.borrow(), &layout, id)
}

fn act(page: &Rc<PageDom>, id: NodeId, action: Action, value: &str, opts: PointerOpts) -> Attempt {
    {
        let dom = page.dom.borrow();
        if dom.root_node(id, true).ok() != Some(page.document()) {
            return Attempt::Retry("element is detached".into());
        }
    }
    // Not visible to Playwright: no area, or `visibility: hidden` (a forced
    // action only needs the box to aim at).
    if visible_box(page, id).is_none() || (!opts.force && !is_visible(page, id)) {
        let rect = page.layout().client_rect(&page.dom.borrow(), id);
        let detail = match rect {
            Some(r) => format!(" (box {:.1},{:.1} {:.1}x{:.1})", r.x, r.y, r.width, r.height),
            None => " (not rendered)".into(),
        };
        return Attempt::Retry(format!("element is not visible{detail}"));
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
            click(page, check_pointer_target(page, id, opts), opts)
        }
        Action::Click => click(page, id, opts),
        Action::Press => {
            // Checked when the step was read (`ActParams::step`).
            let Ok(keys) = super::keyboard::chord(value, opts.modifiers) else {
                return Attempt::Retry(format!("Unknown key: {value:?}"));
            };
            if input::key_target(page) != Some(id) {
                input::script_focus(page, id);
            }
            for (kind, k) in keys {
                input::dispatch_key(page, kind, &k);
            }
            Attempt::Done(json!({}))
        }
        Action::Focus => {
            input::script_focus(page, id);
            Attempt::Done(json!({}))
        }
        Action::Blur => {
            input::script_blur(page, id);
            Attempt::Done(json!({}))
        }
        Action::Hover => pointer(page, id, Pointer::Hover, opts),
        Action::DblClick => pointer(page, id, Pointer::DblClick, opts),
        Action::Wheel => pointer(page, id, Pointer::Wheel, opts),
        Action::Upload | Action::DropFiles | Action::Drag => unreachable!("handled by the host"),
        Action::Type => {
            if input::key_target(page) != Some(id) {
                input::script_focus(page, id);
            }
            // As Playwright's `keyboard.type`: a key press for each character
            // a key of its layout types, the rest inserted as text.
            for c in value.chars() {
                match super::keyboard::type_char(c, opts.modifiers) {
                    Some(keys) => {
                        for (kind, k) in keys {
                            input::dispatch_key(page, kind, &k);
                        }
                    }
                    None => input::insert_text(page, c.encode_utf8(&mut [0; 4])),
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
            input::script_focus(page, id);
            {
                let dom = page.dom.borrow();
                if dom.root_node(id, true).ok() != Some(page.document())
                    || dom.focused(page.document()).ok().flatten() != Some(id)
                {
                    return Attempt::RetrySoon("the control changed during focus".into());
                }
            }
            if page.dom.borrow().input_type(id).ok() == Some(pyro_dom::forms::InputType::Time) {
                if !value.is_empty() && !valid_time_value(value) {
                    return Attempt::Retry("malformed time value".into());
                }
                if let Err(error) = page.dom.borrow_mut().set_value(id, value) {
                    return Attempt::Retry(format!("{error:?}"));
                }
                input::fire_simple(page, id, "input", true, false);
                input::fire_user_change(page, id);
                return Attempt::Done(json!({}));
            }
            if is_control {
                let _ = page.dom.borrow_mut().set_selection_range(id, 0, u32::MAX);
            } else {
                pyro_webapi::editing::select_all(page, id);
            }
            // Empty text deletes the selection (Input.insertText semantics).
            input::insert_text(page, value);
            Attempt::Done(json!({}))
        }
    }
}

/// Visually hidden native controls can expose their visible associated label.
/// Other pointer failures (including overlays) still use the original control.
fn check_pointer_target(page: &Rc<PageDom>, id: NodeId, opts: PointerOpts) -> NodeId {
    if opts.force || opts.position.is_some() {
        return id;
    }
    let native = {
        let dom = page.dom.borrow();
        local(&dom, id) == Some("input")
            && matches!(
                dom.input_type(id).ok(),
                Some(pyro_dom::forms::InputType::Checkbox | pyro_dom::forms::InputType::Radio)
            )
    };
    if !native {
        return id;
    }
    super::dom::reveal(page, id);
    let Some(rect) = visible_box(page, id) else { return id };
    if clipped_box(page, id, rect).is_some() {
        return id;
    }
    let associated = labels(&page.dom.borrow(), page.document(), id);
    associated.into_iter().find(|label| visible_box(page, *label).is_some()).unwrap_or(id)
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
fn clipped_box(page: &PageDom, id: NodeId, rect: pyro_dom::layout::Rect) -> Option<pyro_dom::layout::Rect> {
    page.layout().visible_client_rect(&page.dom.borrow(), id, rect)
}

/// The part of `rect` inside the viewport, when it covers at least about a
/// pixel (Playwright's quads with an area above 0.99).
fn in_viewport(page: &PageDom, rect: pyro_dom::layout::Rect) -> Option<pyro_dom::layout::Rect> {
    let layout = page.layout();
    let (w, h) = (layout.inner_width() as f64, layout.inner_height() as f64);
    let (x0, y0) = (rect.x.max(0.0), rect.y.max(0.0));
    let (x1, y1) = ((rect.x + rect.width).min(w), (rect.y + rect.height).min(h));
    (x1 > x0 && y1 > y0 && (x1 - x0) * (y1 - y0) > 0.99).then(|| pyro_dom::layout::Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
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
            // As Playwright with `force`: the center of the element's box
            // where it meets the viewport, whatever clips it (a visually
            // hidden control's `clip: rect(0 0 0 0)` or `clip-path`).
            let shown = if opts.force { in_viewport(page, rect) } else { clipped_box(page, id, rect) };
            let Some(shown) = shown else {
                let layout = page.layout();
                return Err(Attempt::Retry(format!(
                    "element is outside the viewport (box {:.1},{:.1} {:.1}x{:.1}; viewport {}x{})",
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    layout.inner_width(),
                    layout.inner_height()
                )));
            };
            (shown.x + shown.width / 2.0, shown.y + shown.height / 2.0)
        }
    };
    if opts.force {
        return Ok((x, y));
    }
    match pyro_webapi::layout_api::hit_test(page, x, y) {
        Some(hit) if contains(&page.dom.borrow(), id, hit) => Ok((x, y)),
        Some(hit) => {
            let tag = local(&page.dom.borrow(), hit).unwrap_or("?").to_owned();
            // A transient pointer block (e.g. a virtual list while scrolling)
            // needs time, not another scroll that can dismiss its popover.
            let hit_testable = page.layout().style().is_hit_testable(&page.dom.borrow(), id);
            if opts.position.is_none() && hit_testable && super::dom::reveal_centered(page, id) {
                return Err(Attempt::RetrySoon(format!("centered target after <{tag}> intercepted pointer events")));
            }
            Err(Attempt::Retry(format!("<{tag}> intercepts pointer events")))
        }
        None => Err(Attempt::Retry(format!(
            "element is outside the viewport (nothing is hit at {x:.1},{y:.1}; box {:.1},{:.1} {:.1}x{:.1})",
            rect.x, rect.y, rect.width, rect.height
        ))),
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
    let m = MouseInput {
        x,
        y,
        delta_x: opts.delta.0,
        delta_y: opts.delta.1,
        modifiers: opts.modifiers,
        ..MouseInput::default()
    };
    // Hover-dependent layout (a tooltip the move hid, a menu it opened) can
    // shift the element between events: a move or press goes where it is
    // then; a wheel where the pointer is, and a release there too unless the
    // pressed element moved away from under it (see `At::Release`).
    let events = pointer_events(what, opts.button).into_iter().map(|(kind, button, count)| {
        let at = match kind {
            MouseKind::Moved | MouseKind::Pressed => At::Element,
            MouseKind::Released => At::Release,
            _ => At::Here,
        };
        let buttons = if kind == MouseKind::Pressed { buttons_mask(button) } else { 0 };
        Input::Mouse { kind, button, buttons, count, at }
    });
    let mut sequence = Sequence::new(page, id, opts, m, events);
    if matches!(what, Pointer::Click | Pointer::DblClick | Pointer::Hover) {
        sequence.stability = Some(PointerStability::before_move());
    }
    start(sequence)
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
    let mut m = MouseInput {
        x,
        y,
        delta_x: opts.delta.0,
        delta_y: opts.delta.1,
        modifiers: opts.modifiers,
        ..MouseInput::default()
    };
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

/// A `keyboard` step: a key press (or chord), or text typed key by key, at
/// the focused element (`press` and `type` of `page.keyboard`).
fn keyboard_step(page: &Rc<PageDom>, press: Option<&str>, text: Option<&str>, modifiers: input::Modifiers) -> Attempt {
    if let Some(key) = press {
        // Checked when the step was read (`KeyboardParams::step`).
        let Ok(keys) = super::keyboard::chord(key, modifiers) else {
            return Attempt::Retry(format!("Unknown key: {key:?}"));
        };
        for (kind, k) in keys {
            input::dispatch_key(page, kind, &k);
        }
    }
    for c in text.unwrap_or_default().chars() {
        match super::keyboard::type_char(c, modifiers) {
            Some(keys) => {
                for (kind, k) in keys {
                    input::dispatch_key(page, kind, &k);
                }
            }
            None => input::insert_text(page, c.encode_utf8(&mut [0; 4])),
        }
    }
    Attempt::Done(json!({}))
}

/// Sets the files of an `<input type=file>`, then `input` and `change`.
fn upload(page: &Rc<PageDom>, id: NodeId, files: &[UploadFile]) -> Attempt {
    if let Some(why) = not_a_file_input(page, id) {
        return Attempt::Retry(why.into());
    }
    if disabled(&page.dom.borrow(), id) {
        return Attempt::Retry("element is disabled".into());
    }
    if let Err(e) = set_file_input(page, id, files) {
        return Attempt::Retry(e);
    }
    Attempt::Done(json!({ "files": files.len() }))
}

fn drop_files(page: &Rc<PageDom>, id: NodeId, files: &[UploadFile]) -> Attempt {
    let (x, y) = match pointer_target(page, id, PointerOpts::default()) {
        Ok(point) => point,
        Err(retry) => return retry,
    };
    let now = now_ms();
    let entries: Vec<_> = files
        .iter()
        .map(|file| pyro_webapi::file::FileEntry {
            name: file.name.clone(),
            mime: file.mime.clone(),
            bytes: bytes::Bytes::from(file.bytes.clone()),
            last_modified: file.last_modified.unwrap_or(now),
        })
        .collect();
    let data = input::DragData { items: Vec::new(), drag_operations_mask: 1 };
    for kind in [input::DragKind::Enter, input::DragKind::Over, input::DragKind::Drop] {
        if let Err(error) = input::dispatch_drag_files(page, kind, x, y, &data, &entries, input::Modifiers::default()) {
            return Attempt::Retry(error);
        }
    }
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
        Assert::Css | Assert::Attribute | Assert::Description | Assert::Focused | Assert::Name | Assert::Enabled => {
            let Some(el) = single(found, &dom, locator)? else {
                return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
            };
            match assert {
                Assert::Css => {
                    let property = property.ok_or_else(|| CdpError::invalid_params("css needs a property"))?;
                    let want = expected.as_str().ok_or_else(|| CdpError::invalid_params("css needs a string"))?;
                    let have = page.layout().computed_style_value(&dom, el, &css_property(property));
                    (have == want, json!(have))
                }
                Assert::Attribute | Assert::Description => {
                    let have = if assert == Assert::Attribute {
                        let property =
                            property.ok_or_else(|| CdpError::invalid_params("attribute needs a property"))?;
                        attr(&dom, el, property).map(str::to_owned)
                    } else {
                        let described = id_refs(&dom, page.document(), el, "aria-describedby")
                            .filter(|ids| !ids.is_empty())
                            .map(|ids| {
                                ids.into_iter()
                                    .filter_map(|id| dom.text_content(id).ok().flatten())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            });
                        Some(normalize(
                            &described
                                .or_else(|| attr(&dom, el, "aria-description").map(str::to_owned))
                                .or_else(|| attr(&dom, el, "title").map(str::to_owned))
                                .unwrap_or_default(),
                        ))
                    };
                    let holds = if expected.is_null() {
                        have.is_none()
                    } else {
                        let pattern: Pattern = serde_json::from_value(expected.clone())
                            .map_err(|e| CdpError::invalid_params(e.to_string()))?;
                        have.as_deref().is_some_and(|value| {
                            if assert == Assert::Attribute {
                                expected.as_str().map_or_else(|| pattern.matches_raw(value), |want| value == want)
                            } else {
                                pattern.matches(value, true)
                            }
                        })
                    };
                    (holds, json!(have))
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
                    let have = accessible_name(&dom, &page.layout(), page.document(), el);
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
            let visible = el.is_some_and(|el| is_visible(page, el));
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
                // A `{"regex"}` expected value is tested as Playwright's
                // `toHaveValue(re)` (the raw value) and `toHaveText(re)` /
                // `toContainText(re)` (the whitespace-normalized text).
                Assert::Value => {
                    let have = dom.value(el).unwrap_or_default();
                    let ok = match expected {
                        Value::String(want) => have == *want,
                        _ => check_regex(expected, "value")?.matches_raw(&have),
                    };
                    (ok, json!(have))
                }
                _ => {
                    let have = normalize(&text_of(&dom, el));
                    let ok = match expected {
                        Value::String(want) if assert == Assert::Text => have == normalize(want),
                        Value::String(want) => have.contains(&normalize(want)),
                        _ => check_regex(expected, "text")?.matches(&have, false),
                    };
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

/// A check's `{"regex": "...", "flags": "i"}` expected value.
fn check_regex(expected: &Value, what: &str) -> CdpResult<Pattern> {
    match expected {
        Value::Object(_) => serde_json::from_value(expected.clone()).map_err(|e| CdpError::invalid_params(e.to_string())),
        _ => Err(CdpError::invalid_params(format!("{what} needs a string or a {{\"regex\"}} pattern"))),
    }
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
    /// Wait for the requests the action started (default: true in `Pyro.run`).
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
    /// Pointer actions: modifier keys held (`Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta`).
    #[serde(default)]
    modifiers: Vec<String>,
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
    /// Modifier keys held (`Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta`).
    #[serde(default)]
    modifiers: Vec<String>,
    /// Wait for the requests the action started (default: true).
    settle: Option<bool>,
    timeout: Option<u64>,
}

/// `{"keyboard": {"press": "Escape"}}` or `{"keyboard": {"type": "abc"}}`:
/// keys at the focused element.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyboardParams {
    press: Option<String>,
    #[serde(rename = "type")]
    text: Option<String>,
    /// Modifier keys held (`Shift`, `Control`, `Alt`, `Meta`, `ControlOrMeta`).
    #[serde(default)]
    modifiers: Vec<String>,
    /// Wait for the requests the keys started (default: true).
    settle: Option<bool>,
    timeout: Option<u64>,
}

impl KeyboardParams {
    fn step(self) -> CdpResult<(Step, Option<u64>)> {
        if self.press.is_some() == self.text.is_some() {
            return Err(CdpError::invalid_params("keyboard needs exactly one of press or type"));
        }
        if let Some(key) = self.press.as_deref().and_then(super::keyboard::unknown_key) {
            return Err(CdpError::invalid_params(format!("Unknown key: \"{key}\"")));
        }
        let modifiers = modifiers_of(&self.modifiers)?;
        let settle = self.settle.unwrap_or(true);
        Ok((Step::Keyboard { press: self.press, text: self.text, modifiers, settle }, self.timeout))
    }
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
            modifiers: modifiers_of(&self.modifiers)?,
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
    if !matches!(action, Action::Upload | Action::DropFiles) {
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
        files.push(UploadFile { name: f.name, mime, bytes, last_modified: None });
    }
    for path in value.lines().map(str::trim).filter(|l| !l.is_empty()) {
        files.push(read_upload_path(path)?);
    }
    if files.is_empty() {
        return Err(CdpError::invalid_params("upload needs files or a path in value"));
    }
    Ok(files)
}

/// MIME type by extension, without charset (a `File.type` has none).
fn mime_of(name: &str) -> String {
    let full = pyro_net::static_files::content_type(std::path::Path::new(name));
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
    /// `aria`: `"ai"` for Playwright's AI snapshot (`"default"` otherwise).
    mode: Option<String>,
    /// `aria` in AI mode: each frame's Playwright `seq` by frame id; refs
    /// in a frame with seq `n > 0` are named `f{n}e…`.
    frame_seqs: Option<std::collections::HashMap<String, u32>>,
    timeout: Option<u64>,
}

/// One `Pyro.run` step: `{"goto": {...}}`, `{"act": {...}}`, `{"check": {...}}`
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
    Held(HeldParams),
    AdvanceTime(AdvanceTimeParams),
    Download(DownloadParams),
    DownloadCount(PopupParams),
    Popup(PopupParams),
    Route(RouteParams),
    Act(ActParams),
    Mouse(MouseParams),
    Keyboard(KeyboardParams),
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
    #[serde(default)]
    capture_only: bool,
    #[serde(rename = "as")]
    key: Option<String>,
    timeout: Option<u64>,
    /// Playwright's screenshot assertion option: `disabled` (the default)
    /// renders finite animations finished and cancels infinite ones and
    /// transitions; `allow` renders them where they are.
    animations: Option<String>,
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
        let ratio = self.max_diff_pixel_ratio;
        if ratio.is_some_and(|r| !(0.0..=1.0).contains(&r)) {
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
        let settle_animations = match self.animations.as_deref() {
            None | Some("disabled") => true,
            Some("allow") => false,
            Some(other) => {
                return Err(CdpError::invalid_params(format!(
                    "matchScreenshot animations must be 'disabled' or 'allow', not '{other}'"
                )))
            }
        };
        let shot = ShotStep {
            settle_animations,
            name: self.name,
            baseline,
            scene_hash,
            threshold,
            max_diff_pixels: self.max_diff_pixels,
            max_diff_pixel_ratio: ratio,
            clip: self.clip.map(|c| (c.x, c.y, c.width, c.height)),
            full_page: self.full_page,
            locator: self.locator,
            update: self.update,
            capture_only: self.capture_only,
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
            Some("dark") => m.color_scheme = pyro_dom::media::ColorScheme::Dark,
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
    body: Option<String>,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeldParams {
    hold: String,
    #[serde(rename = "as")]
    key: Option<String>,
    count: usize,
    status: StatusSet,
    timeout: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdvanceTimeParams {
    ms: u64,
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
            delay: Duration::from_millis(self.delay.unwrap_or(0).min(MAX_DELAY_MS)),
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

/// How an `eval` step calls its function (see [`EvalParams`]).
#[derive(Clone, Debug)]
struct EvalCall {
    locator: Option<Locator>,
    all: bool,
    arg: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvalParams {
    expression: String,
    #[serde(rename = "as")]
    key: Option<String>,
    /// The expression is a function: call it (Playwright's `evaluate(fn, arg)`).
    #[serde(default)]
    call: bool,
    /// Call it with the element this locator matches (waiting for exactly
    /// one), or with the array of all matches when `all`.
    locator: Option<Locator>,
    #[serde(default)]
    all: bool,
    /// Its last argument, as JSON.
    arg: Option<Value>,
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
    Duration::from_millis(step.or(default).unwrap_or(DEFAULT_TIMEOUT_MS))
}

/// When a step started at `now` with `timeout` times out; a timeout past
/// what the clock can count never does (a century stands for it).
fn deadline_after(now: Instant, timeout: Duration) -> Instant {
    const CENTURY: Duration = Duration::from_secs(100 * 365 * 24 * 3600);
    now.checked_add(timeout).or_else(|| now.checked_add(CENTURY)).unwrap_or(now)
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
            "dropFiles" => Action::DropFiles,
            "drag" => Action::Drag,
            "wheel" => Action::Wheel,
            _ => {
                return Err(CdpError::invalid_params(
                    "action must be click, fill, focus, check, uncheck, press, hover, dblclick, blur, type, upload, dropFiles, drag or wheel",
                ));
            }
        };
        if action == Action::Press {
            // At once, as Playwright: a key name no retry can make known.
            if let Some(key) = super::keyboard::unknown_key(&self.value) {
                return Err(CdpError::invalid_params(format!("Unknown key: \"{key}\"")));
            }
        }
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
            modifiers: modifiers_of(&self.modifiers)?,
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
        let assert = match self.assert.as_str() {
            "visible" => Assert::Visible,
            "hidden" => Assert::Hidden,
            "text" => Assert::Text,
            "containsText" => Assert::ContainsText,
            "value" => Assert::Value,
            "count" => Assert::Count,
            "checked" => Assert::Checked,
            "css" => Assert::Css,
            "attribute" => Assert::Attribute,
            "description" => Assert::Description,
            "focused" => Assert::Focused,
            "name" => Assert::Name,
            "enabled" => Assert::Enabled,
            _ => {
                return Err(CdpError::invalid_params(
                    "assert must be visible, hidden, text, containsText, value, count, checked, css, focused, name or enabled",
                ));
            }
        };
        if matches!(assert, Assert::Css | Assert::Attribute) && self.property.is_none() {
            return Err(CdpError::invalid_params(format!("{} needs a property", self.assert)));
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
            "aria" => Read::Aria,
            _ => {
                return Err(CdpError::invalid_params(
                    "read must be count, text, value, visible, box, css, name, focused, enabled or aria",
                ));
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
            StepParams::Release(p) => {
                if p.body.as_ref().is_some_and(|body| body.len() > 32 * 1024 * 1024) {
                    return Err(CdpError::invalid_params("replacement response body is too large"));
                }
                Ok((Step::Release { hold: p.hold, body: p.body }, p.timeout))
            }
            StepParams::Held(p) => {
                Ok((Step::Held { hold: p.hold, key: p.key, count: p.count, status: p.status }, p.timeout))
            }
            StepParams::AdvanceTime(p) => {
                if p.ms > 86_400_000 {
                    return Err(CdpError::invalid_params("advanceTime is limited to one day per step"));
                }
                Ok((Step::AdvanceTime { ms: p.ms }, None))
            }
            StepParams::Download(p) => Ok((Step::Download { save_to: p.save_to, key: p.key }, p.timeout)),
            StepParams::DownloadCount(p) => Ok((Step::DownloadCount { key: p.key }, p.timeout)),
            StepParams::Popup(p) => Ok((Step::Popup { key: p.key }, p.timeout)),
            StepParams::Route(p) => Ok((Step::Route { url: p.url, settle: p.settle.unwrap_or(true) }, p.timeout)),
            StepParams::Eval(p) => {
                if let Some(l) = &p.locator {
                    l.validate()?;
                }
                if p.all && p.locator.is_none() {
                    return Err(CdpError::invalid_params("eval: `all` needs a locator"));
                }
                let call = (p.call || p.locator.is_some() || p.arg.is_some())
                    .then(|| Box::new(EvalCall { locator: p.locator, all: p.all, arg: p.arg }));
                Ok((Step::Eval { expression: p.expression, key: p.key, call }, p.timeout))
            }
            StepParams::Request(p) => Ok((Step::Request(p.matcher), p.timeout)),
            StepParams::Viewport(p) => Ok((Step::Viewport { width: p.width, height: p.height }, None)),
            StepParams::Media(p) => p.step(),
            StepParams::Page(p) => Ok((Step::Page { target: p.target }, None)),
            StepParams::MatchScreenshot(p) => p.step(),
            StepParams::Act(p) => p.step(true),
            StepParams::Mouse(p) => p.step(),
            StepParams::Keyboard(p) => p.step(),
            StepParams::Check(p) => p.step(),
            StepParams::Read(p) => {
                let read = p.read()?;
                let ai = match p.mode.as_deref() {
                    None | Some("default") => None,
                    Some("ai") if read == Read::Aria => Some(Box::new(p.frame_seqs.unwrap_or_default())),
                    Some(_) => return Err(CdpError::invalid_params("mode must be default, or ai for an aria read")),
                };
                Ok((Step::Read { locator: p.locator, read, key: p.key, property: p.property, ai }, p.timeout))
            }
        }
    }
}

/// A child frame's aria ref (`f1e5`, Playwright's `/^f(\d+)e\d+$/`).
fn frame_ref(r: &str) -> bool {
    r.strip_prefix('f')
        .and_then(|r| r.split_once('e'))
        .is_some_and(|(f, e)| [f, e].iter().all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
}

/// The CSS property a `read css` / `check css` step names: the CSS spelling (`scroll-margin-bottom`) as it is, or the
/// camel-cased CSSOM attribute `getComputedStyle(el)[name]` takes (`scrollMarginBottom`, `cssFloat`,
/// `webkitTransform`). A custom property (`--fooBar`) is case-sensitive and stays as written.
fn css_property(name: &str) -> std::borrow::Cow<'_, str> {
    if name.starts_with("--") || !name.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(pyro_webapi::css::camel_to_css(name))
    }
}

fn read_value(page: &PageDom, el: NodeId, read: Read, property: Option<&str>) -> Value {
    match read {
        Read::Count => Value::Null,
        Read::Css => match property {
            Some(p) => json!(page.layout().computed_style_value(&page.dom.borrow(), el, &css_property(p))),
            None => Value::Null,
        },
        Read::Name => {
            json!(accessible_name(&page.dom.borrow(), &page.layout(), page.document(), el))
        }
        Read::Focused => json!(page.dom.borrow().focused(page.document()).ok().flatten() == Some(el)),
        Read::Enabled => json!(!disabled(&page.dom.borrow(), el)),
        Read::Aria => match super::aria_snapshot::aria_tree(&page.dom.borrow(), &page.layout(), page.document(), el) {
            Ok(tree) => tree,
            Err(e) => json!({ "error": e }),
        },
        Read::Text => json!(normalize(&text_of(&page.dom.borrow(), el))),
        Read::Value => json!(page.dom.borrow().value(el).unwrap_or_default()),
        Read::Visible => json!(is_visible(page, el)),
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
        // Playwright's `_jumpToAriaRefFrameIfNeeded`: `aria-ref=f1e5` is
        // looked up in the frame whose snapshot named it, wherever the
        // locator is scoped.
        if let Some(r) = locator.aria_ref.as_deref().filter(|r| frame_ref(r)) {
            let docs = self.page_documents(pid);
            if let Some((_, doc)) = docs.into_iter().find(|(_, d)| super::aria_snapshot::ref_node(d, r).is_some()) {
                return Ok(Ok(doc));
            }
        }
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
        if let Step::Read { read: Read::Aria, ai: Some(seqs), .. } = step {
            let attempt = self.read_aria_ai(pid, &page, locator, seqs)?;
            return Ok((attempt, page));
        }
        let attempt = self.attempt_in(&page, step)?;
        Ok((attempt, page))
    }

    /// The live documents of a page's frames, with their frame ids.
    fn page_documents(&self, pid: PageId) -> Vec<(String, Rc<PageDom>)> {
        let browser = &self.thread.browser;
        browser
            .frames(pid)
            .into_iter()
            .filter_map(|f| browser.frame(pid, &f.id).and_then(|f| f.page_dom.map(|d| (f.id.0, d))))
            .filter(|(_, d)| !d.is_disposed())
            .collect()
    }

    /// `ariaSnapshot({ mode: "ai" })` of the locator's element: refs named
    /// after the document's frame seq; each `<iframe>` with a ref carries
    /// its frame's id (`frameId`) so the client can snapshot it next.
    fn read_aria_ai(
        &mut self,
        pid: PageId,
        page: &Rc<PageDom>,
        locator: &Locator,
        seqs: &std::collections::HashMap<String, u32>,
    ) -> CdpResult<Attempt> {
        let found = locator.resolve(page, &self.waiters.selectors)?;
        let Some(el) = single(&found, &page.dom.borrow(), locator)? else {
            return Ok(Attempt::Retry(format!("{} matched no element", locator.describe())));
        };
        let docs = self.page_documents(pid);
        let seq = docs.iter().find(|(_, d)| Rc::ptr_eq(d, page)).and_then(|(id, _)| seqs.get(id)).copied();
        let prefix = match seq {
            Some(n) if n > 0 => format!("f{n}"),
            _ => String::new(),
        };
        let (mut tree, iframes) = match super::aria_snapshot::aria_tree_ai(page, el, &prefix) {
            Ok(t) => t,
            Err(e) => return Ok(Attempt::Done(json!({ "error": e }))),
        };
        let frames: Vec<Value> = iframes
            .into_iter()
            .filter_map(|(r, owner)| {
                let id = self.thread.browser.frame_of_owner(pid, owner)?;
                Some(json!({ "ref": r, "frameId": id.0 }))
            })
            .collect();
        tree["iframes"] = Value::Array(frames);
        Ok(Attempt::Done(tree))
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
            | Step::AdvanceTime { .. }
            | Step::Held { .. }
            | Step::Download { .. }
            | Step::DownloadCount { .. }
            | Step::Popup { .. }
            | Step::Route { .. }
            | Step::Eval { .. }
            | Step::Viewport { .. }
            | Step::Media(_)
            | Step::Page { .. }
            | Step::Request(_)
            | Step::Mouse { .. }
            | Step::Keyboard { .. }
            | Step::MatchScreenshot(_) => {
                unreachable!("not a locator step")
            }
            Step::Act { locator, action, value, to, files, pointer, .. } => {
                let found = locator.resolve(page, &self.waiters.selectors)?;
                let Some(el) = single(&found, &page.dom.borrow(), locator)? else { return Ok(no_match(locator)) };
                let result = match action {
                    Action::Upload => upload(page, el, files),
                    Action::DropFiles => drop_files(page, el, files),
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
        Err(CdpError::server("matchScreenshot: pyro was built without the render feature"))
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
            "cancelRun" => return Ok(Some(json!({ "cancelled": self.cancel_runs(cmd.session_id) }))),
            // `Pyro.dialogs {block}`: dialogs this page's driver steps open wait
            // for `Page.handleJavaScriptDialog` like other dialogs (dialog.rs)
            // instead of returning what a dismissed one gives.
            "dialogs" => {
                #[derive(Deserialize)]
                struct P {
                    block: bool,
                }
                let p = cmd.command.params::<P>()?;
                let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                self.collector.dialogs.set_driver_blocking(id, p.block);
                return Ok(Some(json!({})));
            }
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        };
        if !single {
            // Requests the net thread announced before the run are not its own.
            self.flush_network();
        }
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
        if steps.iter().any(|(s, _)| matches!(s, Step::Request(m) if m.response_body)) {
            if let Some(log) = self.pages.get_mut(&cmd.page).and_then(|s| s.network.log.as_mut()) {
                log.capture_bodies = true;
            }
        }
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
            // Read once for this Pyro.run; never on a DOM read or retry.
            trace: (!single && std::env::var("PYRO_RUN_STEP_TRACE").is_ok_and(|value| value == "1")).then(|| {
                RunStepTrace {
                    process_id: std::process::id(),
                    active: Cell::new(None),
                    attempts_started: Cell::new(0),
                    attempts_completed: Cell::new(0),
                    attempt_ms: Cell::new(0.0),
                    post_eval_turn: Cell::new(None),
                    waits: WaitTrace::default(),
                }
            }),
            log_from,
            downloads_from,
            popups_from,
            home: cmd.page,
            parked: Vec::new(),
        };
        let waiter = Waiter {
            job,
            phase: Phase::Try,
            step_deadline: deadline_after(now, first),
            doc: Weak::new(),
            seen: 0,
            reason: String::new(),
            poll_at: None,
        };
        match self.drive(waiter)? {
            Drive::Done(value) => Ok(Some(value)),
            Drive::Park(w) => {
                if self.waiters.list.len() >= MAX_WAITERS {
                    return Err(CdpError::server("too many waiting Pyro commands"));
                }
                w.trace_wait(&self.thread.browser, Instant::now());
                self.waiters.list.push(*w);
                // Replied by `service_waiters`.
                Ok(None)
            }
        }
    }

    /// Runs a job's steps until one has to wait or the last one is done.
    fn drive(&mut self, mut w: Waiter) -> CdpResult<Drive> {
        loop {
            if self.running.stopped_now() {
                // A `Pyro.cancelRun` stopped a script this run ran (an event
                // handler of its action that never returned): it ends here.
                return Err(w.cancelled(&self.thread.browser, true));
            }
            w.job.trace_step_start();
            let now = Instant::now();
            let pid = w.job.pid;
            match std::mem::replace(&mut w.phase, Phase::Try) {
                Phase::Settle { after, since, until, mut updates } => {
                    let mut work = self.thread.browser.page_work_since(pid, after);
                    if updates > 0 && now < until && work.is_some_and(|w| w.only_rendering()) {
                        // Only the frame interval holds the page: its
                        // rendering update runs now. The scroll events it
                        // fires and the sizes it finds changed are run by
                        // tasks, which the settle then waits for.
                        updates -= 1;
                        self.thread.browser.update_rendering(pid);
                        work = self.thread.browser.page_work_since(pid, after);
                    }
                    let quiet = work.is_none_or(|w| w.quiet());
                    if !quiet && now < until {
                        w.phase = Phase::Settle { after, since, until, updates };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    w.job.trace_settle(since.elapsed().as_secs_f64() * 1e3, work.as_ref().filter(|_| !quiet));
                    if let Some(Value::Object(r)) = w.job.results.last_mut() {
                        r.insert("settleMs".into(), json!(since.elapsed().as_secs_f64() * 1e3));
                        if let (false, Some(work)) = (quiet, work) {
                            r.insert("settled".into(), json!(false));
                            r.insert("pending".into(), pending_json(&work));
                        }
                    }
                    self.next_step(&mut w);
                }
                Phase::Deliver { requests, until } => {
                    if now < until && undelivered(&self.thread.browser, pid, &requests) {
                        // Woken by the replies' completion tasks. A request
                        // stops counting as in flight a moment after its
                        // last one is posted, so look again shortly in case
                        // that task ran first and nothing else wakes the loop.
                        w.poll_at = Some(now + URL_POLL);
                        w.phase = Phase::Deliver { requests, until };
                        return Ok(Drive::Park(Box::new(w)));
                    }
                    // The turn in which the page handles the replies.
                    w.poll_at = None;
                    w.phase = Phase::Act { seq: None, before: None, hop: None, at: self.turn_end(pid) };
                }
                Phase::Await(settled) => {
                    let outcome = settled.borrow_mut().take();
                    match outcome {
                        Some(Ok(value)) => {
                            let step = w.job.steps[w.job.index].0.clone();
                            self.finish_step(&mut w, &step, value, None, true);
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
                            self.finish_step(&mut w, &step, value, None, true);
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
                        w.job.trace_post_eval_turn_finish("end");
                        self.next_step(&mut w);
                        continue;
                    };
                    let advanced = if now >= w.step_deadline {
                        let reason = "input action did not become ready before its deadline";
                        Err(if std::env::var_os("PYRO_POINTER_WAIT_DIAGNOSTICS").is_some() {
                            format!("{reason}: {}", seq.wait_diagnostics())
                        } else {
                            reason.into()
                        })
                    } else {
                        seq.advance()
                    };
                    self.thread.browser.checkpoint();
                    match advanced {
                        Ok(None) => {
                            // Waiting for the stability check's frame: that frame is
                            // the turn (its page callbacks run in it too), so wake when
                            // it has run instead of after `TURN`, which the pending
                            // frame itself would otherwise add.
                            let (hop, at) = match seq.stability_frame() {
                                Some(frame) => (Some(frame), now),
                                None => (None, self.turn_end(pid)),
                            };
                            w.phase = Phase::Act { seq: Some(seq), before, hop, at };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                        Ok(Some(value)) => {
                            let step = w.job.steps[w.job.index].0.clone();
                            self.finish_step(&mut w, &step, value, before, true);
                        }
                        Err(reason) => {
                            let (step, timeout) = w.job.steps[w.job.index].clone();
                            return self.park_retry(w, &step, timeout, Some(&seq.page), reason, now);
                        }
                    }
                }
                Phase::Load { loader, dom_ready, navigation } => {
                    match load_state(&self.thread.browser, pid, &loader, dom_ready, navigation) {
                        LoadState::Reached => {}
                        LoadState::Follow(next) => {
                            w.phase = Phase::Load { loader: next, dom_ready, navigation };
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
                            w.phase = Phase::Load { loader, dom_ready, navigation };
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
                        // The loop serves the waiters before it reads what the
                        // net thread announced: a request sent in the turn that
                        // ended the run (a `held` step may have read its parked
                        // response already) is one of the run's.
                        self.flush_network();
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
                        w.phase = Phase::Load { loader: loader.0, dom_ready: *dom_ready, navigation: true };
                        continue;
                    }
                    if let Step::Loaded = step {
                        let loader = loader_to_wait_for(&self.thread.browser, pid);
                        w.phase = Phase::Load { loader, dom_ready: false, navigation: false };
                        continue;
                    }
                    if let Step::Idle = step {
                        w.job.results.push(json!({}));
                        w.phase =
                            Phase::Settle { after: 0, since: now, until: w.step_deadline, updates: SETTLE_UPDATES };
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
                        w.phase = Phase::Load { loader: loader.0, dom_ready: false, navigation: true };
                        continue;
                    }
                    let before = matches!(
                        step,
                        Step::Act { settle: true, .. }
                            | Step::Mouse { settle: true, .. }
                            | Step::Keyboard { settle: true, .. }
                            | Step::Route { settle: true, .. }
                            | Step::Viewport { .. }
                            | Step::Media(_)
                    )
                    .then(|| self.thread.browser.request_mark(pid));
                    // Whether a step that does not settle gets its turn: not an
                    // `eval` whose script left the page nothing to run.
                    let mut turn = true;
                    let (attempt, page) = match &step {
                        Step::Request(m) => {
                            let span = w.job.trace_attempt("request_attempt");
                            let attempt = self.match_request(&w.job, m);
                            if let Some(span) = span {
                                span.finish(Some(&attempt));
                            }
                            (attempt, None)
                        }
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
                        Step::DownloadCount { .. } => {
                            let state = self.pages.get(&w.job.page).ok_or_else(|| CdpError::server("page is gone"))?;
                            (Attempt::Done(json!(state.download_count)), None)
                        }
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
                        Step::Release { hold, body } => {
                            match self.release_hold(w.job.page, hold, body.as_deref()).map_err(|e| w.job.fail(e))? {
                                Some((released, delivered)) => {
                                    w.job.results.push(json!({ "released": released }));
                                    if delivered.is_empty() {
                                        // Sent on to the network: nothing reaches the page yet.
                                        self.next_step(&mut w);
                                    } else {
                                        w.phase = Phase::Deliver { requests: delivered, until: now + SETTLE_MAX };
                                    }
                                    continue;
                                }
                                None => (Attempt::Retry(format!("nothing is held under {hold:?} yet")), None),
                            }
                        }
                        Step::Held { hold, count, status, .. } => {
                            let context =
                                self.pages.get(&w.job.page).ok_or_else(|| CdpError::server("page is gone"))?.context;
                            let mocks =
                                &self.contexts.get(&context).ok_or_else(|| CdpError::server("context is gone"))?.mocks;
                            let responses = mocks.held_responses(hold);
                            let statuses: Vec<u16> = responses
                                .iter()
                                .filter_map(|response| response["status"].as_u64().map(|status| status as u16))
                                .collect();
                            let attempt = if statuses.len() == *count
                                && statuses.iter().all(|s| status.accepts(Some(*s)))
                            {
                                Attempt::Done(json!({ "count": count, "statuses": statuses, "responses": responses }))
                            } else {
                                Attempt::Retry(format!(
                                    "held response statuses {statuses:?}, want {count} matching {status:?}"
                                ))
                            };
                            (attempt, None)
                        }
                        Step::AdvanceTime { ms } => {
                            self.thread
                                .browser
                                .advance_time_explicit(Duration::from_millis(*ms))
                                .map_err(|e| w.job.fail(CdpError::server(format!("clock advance failed: {e}"))))?;
                            self.thread.browser.checkpoint();
                            (Attempt::Done(json!({ "advanced": ms })), None)
                        }
                        Step::Eval { expression, call: Some(call), .. } => {
                            let main = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            let (doc, nodes) = match &call.locator {
                                None => (main, Vec::new()),
                                Some(locator) => {
                                    let doc = match self.scope_page(pid, &main, locator).map_err(|e| w.job.fail(e))? {
                                        Ok(doc) => doc,
                                        Err(Attempt::Retry(reason) | Attempt::RetrySoon(reason)) => {
                                            return self.park_retry(w, &step, timeout, Some(&main), reason, now);
                                        }
                                        Err(_) => unreachable!("scope_page only retries"),
                                    };
                                    let found =
                                        locator.resolve(&doc, &self.waiters.selectors).map_err(|e| w.job.fail(e))?;
                                    let nodes = if call.all {
                                        found
                                    } else {
                                        match single(&found, &doc.dom.borrow(), locator) {
                                            Ok(Some(id)) => vec![id],
                                            Ok(None) => {
                                                let reason = format!("{} matched no element", locator.describe());
                                                return self.park_retry(w, &step, timeout, Some(&doc), reason, now);
                                            }
                                            Err(e) => return Err(w.job.fail(e)),
                                        }
                                    };
                                    (doc, nodes)
                                }
                            };
                            let stamp = self.thread.browser.work_stamp(pid);
                            match self
                                .call_step(w.job.page, &doc, expression, call, &nodes)
                                .map_err(|e| w.job.fail(e))?
                            {
                                Evaluated::Value(value) => {
                                    turn = stamp.is_none() || self.thread.browser.work_stamp(pid) != stamp;
                                    (Attempt::Done(value), None)
                                }
                                Evaluated::Pending(settled) => {
                                    w.phase = Phase::Await(settled);
                                    continue;
                                }
                            }
                        }
                        Step::Eval { expression, .. } => {
                            let stamp = self.thread.browser.work_stamp(pid);
                            match self
                                .eval_step(pid, expression, w.job.trace_stage_context())
                                .map_err(|e| w.job.fail(e))?
                            {
                                Evaluated::Value(value) => {
                                    turn = stamp.is_none() || self.thread.browser.work_stamp(pid) != stamp;
                                    (Attempt::Done(value), None)
                                }
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
                            // Nor on the error document of a failed navigation (one
                            // under way may still get there).
                            let failed = self.thread.browser.page(pid).and_then(|p| {
                                p.main_frame.unreachable.clone().filter(|_| p.pending_loader().is_none())
                            });
                            if let Some(failed) = failed {
                                return Err(w.job.fail(CdpError::server(format!(
                                    "the page shows the error document of its navigation to {} ({}): \
                                     route needs a document",
                                    failed.url, failed.error
                                ))));
                            }
                            let blank =
                                self.thread.browser.page(pid).filter(|p| {
                                    p.main_frame.url == "about:blank" || p.main_frame.unreachable.is_some()
                                });
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
                                self.eval_step(pid, &src, None).map_err(|e| w.job.fail(e))?;
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
                        Step::Keyboard { press, text, modifiers, .. } => {
                            let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            let attempt = keyboard_step(&page, press.as_deref(), text.as_deref(), *modifiers);
                            self.thread.browser.checkpoint();
                            (attempt, Some(page))
                        }
                        _ => {
                            let page = self.main_document(w.job.page).map_err(|e| w.job.fail(e))?;
                            let span = if matches!(&step, Step::Check { .. }) {
                                w.job.trace_attempt("check_attempt")
                            } else {
                                None
                            };
                            let attempted = self.attempt(pid, &page, &step);
                            if let Some(span) = span {
                                span.finish(attempted.as_ref().ok().map(|(attempt, _)| attempt));
                            }
                            let (attempt, scope) = attempted.map_err(|e| w.job.fail(e))?;
                            (attempt, Some(scope))
                        }
                    };
                    match attempt {
                        Attempt::Done(value) => self.finish_step(&mut w, &step, value, before, turn),
                        Attempt::Continue(seq) => {
                            w.phase = Phase::Act { seq: Some(seq), before, hop: None, at: self.turn_end(pid) };
                            return Ok(Drive::Park(Box::new(w)));
                        }
                        Attempt::RetrySoon(reason) => {
                            let mut parked = self.park_retry(w, &step, timeout, page.as_ref(), reason, now)?;
                            if let Drive::Park(waiter) = &mut parked {
                                waiter.poll_at = Some(now + TURN);
                            }
                            return Ok(parked);
                        }
                        Attempt::Retry(reason) => {
                            return self.park_retry(w, &step, timeout, page.as_ref(), reason, now);
                        }
                    }
                }
            }
        }
    }

    /// A step's result is in: settle after an action (and a `route`), or
    /// give an action or `eval` that does not settle one event-loop turn so
    /// what it queued (a `setTimeout(save, 0)` from a blur, the `scroll`
    /// event of a focus that revealed its element) runs before the next
    /// step, unless it queued nothing (`turn` is false: an `eval` that only
    /// read); other steps go straight on.
    fn finish_step(&mut self, w: &mut Waiter, step: &Step, value: Value, before: Option<u64>, turn: bool) {
        if let Step::Read { key: Some(key), .. }
        | Step::Eval { key: Some(key), .. }
        | Step::Download { key: Some(key), .. }
        | Step::DownloadCount { key: Some(key) }
        | Step::Popup { key: Some(key) }
        | Step::Held { key: Some(key), .. } = step
        {
            w.job.reads.insert(key.clone(), value.clone());
        } else if let Step::Request(RequestMatch { key: Some(key), response_body, .. }) = step {
            let read =
                if *response_body { value.get("responseBody").cloned().unwrap_or(Value::Null) } else { value.clone() };
            w.job.reads.insert(key.clone(), read);
        } else if let Step::MatchScreenshot(shot) = step {
            if let Some(key) = &shot.key {
                w.job.reads.insert(key.clone(), value.clone());
            }
        }
        w.job.results.push(value);
        let now = Instant::now();
        match before {
            Some(after) => {
                // Frames the action's follow-up work asks for run once idle.
                pyro_browser::note_client_activity();
                w.phase = Phase::Settle { after, since: now, until: now + SETTLE_MAX, updates: SETTLE_UPDATES };
            }
            None if turn && matches!(step, Step::Act { .. } | Step::Eval { .. } | Step::AdvanceTime { .. }) => {
                // The scroll steps the page owes run in this turn instead of
                // at the frame interval: the `scroll` event of a `focus` that
                // revealed its element is the step's own, and 16 ms later it
                // would arrive inside a later step (after that one opened a
                // popover that closes when its trigger's container scrolls).
                let pid = w.job.pid;
                if self.thread.browser.page_work(pid).is_some_and(|work| work.scrolls > 0) {
                    self.thread.browser.update_rendering(pid);
                }
                w.phase = Phase::Act { seq: None, before: None, hop: None, at: self.turn_end(pid) };
                if matches!(step, Step::Eval { .. }) {
                    w.job.trace_post_eval_turn_start();
                }
            }
            None => {
                if !turn {
                    w.job.trace_post_eval_turn_skipped();
                }
                self.next_step(w)
            }
        }
    }

    /// A step that cannot finish yet: fails once past its deadline, else
    /// parks until the document it searched changes (an action step also
    /// until [`ACT_POLL`] passed; URL, route, hold and page steps poll only).
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
        w.poll_at = if matches!(
            step,
            Step::Url(_) | Step::Route { .. } | Step::Release { .. } | Step::Held { .. } | Step::Page { .. }
        ) {
            Some(Instant::now() + URL_POLL)
        } else if matches!(step, Step::Act { .. }) {
            // An element that is outside the viewport, covered or moving gets
            // there through animations, transitions and scrolling as well as
            // DOM changes: tried again on a timer too, as Playwright's retries
            // are (20, 100, 100, then 500 ms apart).
            Some(Instant::now() + ACT_POLL)
        } else {
            None
        };
        w.reason = reason;
        w.phase = Phase::Try;
        Ok(Drive::Park(Box::new(w)))
    }

    /// When the event-loop turn starting now may end (the marker task goes
    /// in then): after [`TURN`] when the page has an animation frame or a
    /// timer due soon that the turn must let run; at once otherwise, the
    /// marker still running behind every queued task. ResizeObserver work
    /// does not lengthen it: a delivery is a queued task, and a gather that
    /// waits out its pacing interval is not due within the turn.
    fn turn_end(&self, pid: PageId) -> Instant {
        let now = Instant::now();
        match self.thread.browser.page_work(pid) {
            Some(w) if w.frames == 0 && w.timers == 0 => now,
            _ => now + TURN,
        }
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

    /// Releases what the context's mocks hold under `name`: how many
    /// replies, and the requests among them whose response goes straight to
    /// the page. `None` while nothing is held (the step polls).
    fn release_hold(
        &mut self,
        page: PageKey,
        name: &str,
        body: Option<&str>,
    ) -> CdpResult<Option<(usize, Vec<pyro_net::RequestId>)>> {
        let context = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let mocks = &self.contexts.get(&context).ok_or_else(|| CdpError::server("context is gone"))?.mocks;
        let released = mocks
            .release(name, body.map(|body| bytes::Bytes::copy_from_slice(body.as_bytes())).as_ref())
            .map_err(CdpError::invalid_params)?;
        Ok(Some(released).filter(|(count, _)| *count > 0))
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
    fn eval_step(&mut self, pid: PageId, src: &str, trace: Option<RunStageContext>) -> CdpResult<Evaluated> {
        let realm = self
            .thread
            .browser
            .page(pid)
            .and_then(|p| p.main_frame.main_realm().cloned())
            .ok_or_else(|| CdpError::server("page has no realm"))?;
        let api = trace.map(|context| context.span("eval_api"));
        let value = {
            let scope = realm.scope().map_err(super::remote::js_to_cdp)?;
            match scope.eval(src, Some("pyro-run"), 1) {
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
                Err(e) => return Err(self.script_error(&scope, e)),
            }
        };
        if let Some(api) = api {
            api.finish(if matches!(&value, Evaluated::Pending(_)) { "pending" } else { "done" });
        }
        let checkpoint = trace.map(|context| context.span("eval_checkpoint"));
        self.thread.browser.checkpoint();
        if let Some(checkpoint) = checkpoint {
            checkpoint.finish("done");
        }
        Ok(value)
    }

    /// Calls the function `src` evaluates to in the realm of `doc` with the
    /// matched element (or the array of them, `all`) and the JSON `arg`.
    fn call_step(
        &mut self,
        page: PageKey,
        doc: &Rc<PageDom>,
        src: &str,
        call: &EvalCall,
        nodes: &[NodeId],
    ) -> CdpResult<Evaluated> {
        let realm = self
            .with_frame_of_document(page, doc, |f| f.main_realm().cloned())
            .flatten()
            .ok_or_else(|| CdpError::server("the element's frame has no realm"))?;
        let value = {
            let scope = realm.scope().map_err(super::remote::js_to_cdp)?;
            let threw = |scope: &pyro_js::Scope<'_>, e: pyro_js::JsError| self.script_error(scope, e);
            let f = scope.eval(&format!("({src}\n)"), Some("pyro-run"), 1).map_err(|e| threw(&scope, e))?;
            let Some(f) = f.as_object().filter(|f| f.is_function()) else {
                return Err(CdpError::invalid_params("eval: the expression is not a function"));
            };
            let mut args = pyro_js::RootedVec::new();
            if call.locator.is_some() {
                if call.all {
                    let list = scope.array(&[]).map_err(super::remote::js_to_cdp)?;
                    for (i, id) in nodes.iter().enumerate() {
                        let el = pyro_webapi::wrap_node(&scope, doc, *id).map_err(super::remote::js_to_cdp)?;
                        list.set_index(i as u32, el.value()).map_err(super::remote::js_to_cdp)?;
                    }
                    args.push(&scope, list.value());
                } else {
                    let el = pyro_webapi::wrap_node(&scope, doc, nodes[0]).map_err(super::remote::js_to_cdp)?;
                    args.push(&scope, el.value());
                }
            }
            let arg = match &call.arg {
                Some(arg) => scope.parse_json(&arg.to_string()).unwrap_or_else(|| scope.undefined()),
                None => scope.undefined(),
            };
            args.push(&scope, arg);
            let v = f.call_rooted(None, &args).map_err(|e| threw(&scope, e))?;
            if super::runtime::is_thenable(v) {
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
            } else {
                Evaluated::Value(json_of(v))
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
        let from = if m.scope == Some(RequestScope::Context) {
            if log.history_truncated() {
                return Attempt::Retry(
                    "context request history was truncated; an exact assertion is unavailable".into(),
                );
            }
            0
        } else {
            from
        };
        let matching: Vec<_> = log.since(from).iter().filter(|r| m.accepts(r)).collect();
        let started = m.state == Some(RequestState::Started);
        let finished: Vec<_> = matching.iter().copied().filter(|r| r.done).collect();
        if let Some(n) = m.count {
            let status_matches = matching.iter().all(|r| m.status.as_ref().is_none_or(|s| s.accepts(r.status)));
            return if matching.len() == n && (started || finished.len() == n) && status_matches {
                Attempt::Done(json!({ "count": n }))
            } else if !status_matches {
                Attempt::Retry(format!(
                    "matching request status differs: {}",
                    matching.last().map(|r| r.to_json()).unwrap_or(Value::Null)
                ))
            } else if started {
                Attempt::Retry(format!("{} matching requests started, want {n}", matching.len()))
            } else {
                Attempt::Retry(format!("{} matching requests ({} finished), want {n}", matching.len(), finished.len()))
            };
        }
        let candidates = if started { &matching } else { &finished };
        match candidates.iter().rev().find(|r| m.status.as_ref().is_none_or(|s| s.accepts(r.status))) {
            Some(r) if m.response_body && !r.has_response_body() && r.error.is_none() => Attempt::Retry(
                "the matching response's body was not kept (it ended before the run asked for bodies)".into(),
            ),
            Some(r) if m.response_body => Attempt::Done(r.to_json_with_body()),
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
        w.job.trace_step_end();
        w.job.index += 1;
        w.phase = Phase::Try;
        if let Some((_, timeout)) = w.job.steps.get(w.job.index) {
            w.step_deadline = deadline_after(Instant::now(), *timeout);
        }
    }

    fn driver_query(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Value> {
        let p: QueryParams = cmd.command.params()?;
        let read = p.read()?;
        let page = self.main_document(cmd.page)?;
        match p.mode.as_deref() {
            None | Some("default") => {}
            Some("ai") if read == Read::Aria => {
                let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                return match self.read_aria_ai(pid, &page, &p.locator, &p.frame_seqs.unwrap_or_default())? {
                    Attempt::Done(v) => Ok(json!({ "value": v })),
                    _ => Err(CdpError::server(format!("{} matched no element", p.locator.describe()))),
                };
            }
            Some(_) => return Err(CdpError::invalid_params("mode must be default, or ai for an aria read")),
        }
        let found = p.locator.resolve(&page, &self.waiters.selectors)?;
        if read == Read::Count {
            return Ok(json!({ "value": found.len() }));
        }
        let el = single(&found, &page.dom.borrow(), &p.locator)?
            .ok_or_else(|| CdpError::server(format!("{} matched no element", p.locator.describe())))?;
        Ok(json!({ "value": read_value(&page, el, read, p.property.as_deref()) }))
    }

    /// `Pyro.cancelRun`: answers `session`'s runs (`Pyro.run`, `act`,
    /// `expect`, `idle`) and its `awaitPromise` evaluations still waiting
    /// with an error saying they were cancelled ([`Waiter::cancelled`]). The
    /// pages stay as they are: a navigation, render or promise under way goes
    /// on unobserved. A run whose script was running when this arrived was
    /// stopped by the watchdog check already ([`RunningCommand::cancels`])
    /// and answered, or waits here now. Replies are queued ahead of this
    /// command's. How many commands it answered (or stopped); with nothing in
    /// flight, none.
    fn cancel_runs(&mut self, session: &str) -> usize {
        let (cancelled, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.waiters.list).into_iter().partition(|w| w.job.session == session);
        self.waiters.list = waiting;
        let stopped = self.running.take_stopped(session);
        let mut count = usize::from(stopped.is_some_and(|id| !cancelled.iter().any(|w| w.job.id == id)));
        for w in cancelled {
            let error = w.cancelled(&self.thread.browser, false);
            self.collector.push(Queued::Frame(msg::error_frame(Some(w.job.id), &error, Some(&w.job.session))));
            count += 1;
        }
        count += self.pending.fail_session(session, &CdpError::server("cancelled: waiting for the promise to settle"));
        self.collector.dialogs.driver_busy.set(!self.waiters.list.is_empty());
        count
    }

    /// A run's script failed: what its step fails with. A script a
    /// `Pyro.cancelRun` stopped says the run was cancelled.
    fn script_error(&self, scope: &pyro_js::Scope<'_>, error: pyro_js::JsError) -> CdpError {
        if matches!(error, pyro_js::JsError::Terminated) && self.running.stopped_now() {
            return CdpError::server("cancelled: the script it ran was still running and was stopped");
        }
        CdpError::server(format!("threw: {}", error.details(scope).message))
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
            let _running = self.running.enter(&session, id);
            w.job.trace_driving(Instant::now());
            let frame = match self.drive(w) {
                Ok(Drive::Park(w)) => {
                    w.trace_wait(&self.thread.browser, Instant::now());
                    self.waiters.list.push(*w);
                    continue;
                }
                Ok(Drive::Done(value)) => msg::result_frame(id, &value, Some(&session)),
                Err(error) => msg::error_frame(Some(id), &error, Some(&session)),
            };
            self.collector.push(Queued::Frame(frame));
        }
        self.collector.dialogs.driver_busy.set(!self.waiters.list.is_empty());
    }
}

/// A page's target id (its main frame id).
fn id_of(browser: &Browser, pid: PageId) -> Option<String> {
    browser.page(pid).map(|p| p.main_frame.id.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_timeouts_have_no_upper_bound() {
        assert_eq!(timeout_of(None, None), Duration::from_millis(DEFAULT_TIMEOUT_MS));
        assert_eq!(timeout_of(None, Some(0)), Duration::ZERO);
        // Longer than the 600 s cap there used to be (Playwright has none).
        assert_eq!(timeout_of(Some(900_000), Some(1)), Duration::from_secs(900));
        // The wright's "no limit" (2^31 - 1 ms), and more than any clock counts.
        let now = Instant::now();
        assert!(deadline_after(now, timeout_of(Some((1 << 31) - 1), None)) > now + Duration::from_secs(86_400 * 24));
        assert!(deadline_after(now, timeout_of(Some(u64::MAX), None)) > now + Duration::from_secs(86_400 * 365));
        assert_eq!(deadline_after(now, Duration::ZERO), now);
    }

    #[test]
    fn an_unknown_key_fails_the_step_when_it_is_read() {
        let step = |value: &str| {
            serde_json::from_value::<ActParams>(
                json!({ "locator": { "css": "body" }, "action": "press", "value": value }),
            )
            .unwrap()
            .step(false)
        };
        assert!(step("Alt+KeyD").is_ok());
        assert!(step("Control+Shift++").is_ok());
        let error = step("Shift+NoSuchKey").err().unwrap();
        assert_eq!(error.message, "Unknown key: \"NoSuchKey\"");
    }
}
