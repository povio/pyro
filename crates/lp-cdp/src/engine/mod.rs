//! CDP domains backed by the real engine (`lp-browser` + `lp-js` on
//! JavaScriptCore). [`EngineHost`] implements [`Host`] on the browser thread.
//!
//! Engine notifications (page events, console calls, uncaught exceptions)
//! are queued by [`Collector`] while engine code runs and translated into CDP
//! events by [`EngineHost::flush`] afterwards, so no CDP state is borrowed
//! while JavaScript is on the stack.

mod page;
mod remote;
mod runtime;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use lp_browser::{BrowserThread, PageEvent, PageId, PageObserver};
use lp_js::{ConsoleLevel, EngineHooks, JsException, Persistent, Realm, RealmId, Value};
use serde::Deserialize;

use crate::msg::{self, CdpError, CdpResult, Command, Empty};
use crate::outbox::Outbox;
use crate::runtime_protocol::HandleTable;
use crate::target::{ContextKey, Host, PageCommand, PageKey, Sink};

/// Engine notifications waiting to be translated into CDP events.
pub(crate) enum Queued {
    Page(PageId, PageEvent),
    Console { realm: RealmId, level: ConsoleLevel, args: Vec<Persistent>, timestamp: f64 },
    Exception { realm: RealmId, exception: JsException, timestamp: f64 },
}

#[derive(Default)]
pub(crate) struct Collector {
    queue: RefCell<VecDeque<Queued>>,
}

impl Collector {
    fn push(&self, item: Queued) {
        self.queue.borrow_mut().push_back(item);
    }

    fn pop(&self) -> Option<Queued> {
        self.queue.borrow_mut().pop_front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }
}

/// Milliseconds since the Unix epoch (CDP `Runtime.Timestamp`).
fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

impl PageObserver for Collector {
    fn on_page_event(&self, page: PageId, event: &PageEvent) {
        self.push(Queued::Page(page, event.clone()));
    }
}

impl EngineHooks for Collector {
    fn console_api(&self, realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        let Ok(scope) = realm.scope() else { return };
        let args = args.iter().map(|v| Persistent::new(&scope, *v)).collect();
        self.push(Queued::Console { realm: realm.id(), level, args, timestamp: epoch_ms() });
    }

    fn exception_thrown(&self, realm: &Realm, exception: &JsException) {
        self.push(Queued::Exception { realm: realm.id(), exception: exception.clone(), timestamp: epoch_ms() });
    }

    fn unhandled_rejection(&self, realm: &Realm, exception: &JsException) {
        self.push(Queued::Exception { realm: realm.id(), exception: exception.clone(), timestamp: epoch_ms() });
    }
}

/// Per-session CDP state. Handles are shared with pending asynchronous
/// replies (`awaitPromise`), which run as engine microtasks.
pub(crate) struct Session {
    pub(crate) page_enabled: bool,
    pub(crate) lifecycle_events: bool,
    pub(crate) runtime_enabled: bool,
    pub(crate) handles: Rc<RefCell<HandleTable<Persistent>>>,
}

/// Emulated viewport from `Emulation.setDeviceMetricsOverride`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Viewport {
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) device_scale_factor: f64,
    pub(crate) mobile: bool,
}

pub(crate) struct PageState {
    pub(crate) id: PageId,
    pub(crate) context: ContextKey,
    pub(crate) sessions: Vec<String>,
    /// Realms created by the navigation in progress; announced after
    /// `Page.frameNavigated`, matching upstream's event order.
    pub(crate) pending_realms: Vec<(RealmId, Option<String>, String)>,
    pub(crate) navigating: bool,
    /// `Page.navigate`/`reload` requests awaiting commit or failure: (request id, session id).
    pub(crate) pending_navigations: Vec<(u64, String)>,
    pub(crate) viewport: Option<Viewport>,
    pub(crate) emulated_media: Option<serde_json::Value>,
    pub(crate) user_agent: Option<(String, Option<String>)>,
    pub(crate) next_script_id: u64,
}

/// The engine side of one CDP connection, living on its browser thread.
pub struct EngineHost {
    pub thread: BrowserThread,
    pub(crate) out: Outbox,
    collector: Rc<Collector>,
    contexts: HashSet<ContextKey>,
    next_context: ContextKey,
    pub(crate) pages: HashMap<PageKey, PageState>,
    pub(crate) sessions: HashMap<String, Session>,
    pub(crate) next_exception_id: i64,
}

impl EngineHost {
    /// Creates the engine for the current thread (at most one per thread).
    pub fn new(out: Outbox) -> CdpResult<EngineHost> {
        let mut thread =
            BrowserThread::new().map_err(|e| CdpError::server(format!("JavaScript engine unavailable: {e}")))?;
        let collector = Rc::new(Collector::default());
        thread.browser.set_observer(Some(collector.clone()));
        thread.browser.set_engine_hooks(Some(collector.clone()));
        Ok(EngineHost {
            thread,
            out,
            collector,
            contexts: HashSet::new(),
            next_context: 1,
            pages: HashMap::new(),
            sessions: HashMap::new(),
            next_exception_id: 1,
        })
    }

    pub fn thread_mut(&mut self) -> &mut BrowserThread {
        &mut self.thread
    }

    /// Runs engine tasks and timers until `wake()` is true or engine
    /// notifications are waiting for [`EngineHost::flush`].
    pub fn run_until_work(
        &mut self,
        deadline: Option<std::time::Instant>,
        mut wake: impl FnMut() -> bool,
    ) -> lp_runtime::RunOutcome {
        let collector = &self.collector;
        let thread = &mut self.thread;
        thread.event_loop.run_until(&mut thread.browser, deadline, false, |_| wake() || !collector.is_empty())
    }

    /// Disposes every page and realm (connection teardown).
    pub fn close_all(&mut self) {
        self.sessions.clear();
        self.pages.clear();
        self.thread.browser.close_all();
        self.collector.queue.borrow_mut().clear();
    }

    /// True when engine notifications are waiting for [`EngineHost::flush`].
    pub fn has_pending_events(&self) -> bool {
        !self.collector.is_empty()
    }

    pub(crate) fn page_key(&self, id: PageId) -> Option<PageKey> {
        self.pages.iter().find(|(_, p)| p.id == id).map(|(k, _)| *k)
    }

    pub(crate) fn realm(&self, page: PageKey, realm: RealmId) -> Option<Realm> {
        let state = self.pages.get(&page)?;
        let page = self.thread.browser.page(state.id)?;
        page.main_frame.realms.iter().find(|r| r.realm.id() == realm).map(|r| r.realm.clone())
    }

    pub(crate) fn main_realm(&self, page: PageKey) -> Option<Realm> {
        let state = self.pages.get(&page)?;
        self.thread.browser.page(state.id)?.main_frame.main_realm().cloned()
    }

    /// Sessions of `page` with a predicate, in attach order.
    pub(crate) fn sessions_of(&self, page: PageKey, pred: impl Fn(&Session) -> bool) -> Vec<String> {
        let Some(state) = self.pages.get(&page) else { return Vec::new() };
        state.sessions.iter().filter(|s| self.sessions.get(*s).is_some_and(&pred)).cloned().collect()
    }

    pub(crate) fn event<T: serde::Serialize + ?Sized>(&self, method: &str, params: &T, session_id: &str) {
        self.out.push(msg::event_frame(method, params, Some(session_id)));
    }

    /// Translates queued engine notifications into CDP events, in order.
    pub fn flush(&mut self) {
        while let Some(item) = self.collector.pop() {
            match item {
                Queued::Page(page, event) => {
                    if let Some(key) = self.page_key(page) {
                        self.on_page_event(key, event);
                    }
                }
                Queued::Console { realm, level, args, timestamp } => self.on_console(realm, level, args, timestamp),
                Queued::Exception { realm, exception, timestamp } => self.on_exception(realm, exception, timestamp),
            }
        }
    }

    /// Page owning `realm`, if any.
    pub(crate) fn page_of_realm(&self, realm: RealmId) -> Option<PageKey> {
        self.pages.iter().find_map(|(key, state)| {
            let page = self.thread.browser.page(state.id)?;
            page.main_frame.realms.iter().any(|r| r.realm.id() == realm).then_some(*key)
        })
    }

    fn reply<T: serde::Serialize + ?Sized>(&self, cmd: &PageCommand<'_, '_>, result: &T) {
        self.out.push(msg::result_frame(cmd.command.id, result, Some(cmd.session_id)));
    }

    fn page_command_result(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let (domain, action) =
            cmd.command.domain_and_action().ok_or_else(|| CdpError::method_not_found(&cmd.command.method))?;
        match domain {
            "Page" => self.page_domain(action, cmd),
            "Runtime" => self.runtime_domain(action, cmd),
            "Emulation" => self.emulation_domain(action, cmd),
            "Browser" => self.window_domain(action, cmd),
            // Domains with nothing to report yet. Enabling them is truthful:
            // no network exists before lp-net is integrated (only about:blank
            // loads), and the engine produces no Log-domain entries (console
            // goes through Runtime.consoleAPICalled as in Chrome).
            "Network" | "Log" if matches!(action, "enable" | "disable") => Ok(Some(serde_json::json!({}))),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Headless window per page target. Bounds are recorded but nothing in
    /// the page observes them yet (no `outerWidth`/screen APIs), like upstream's no-op.
    fn window_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        match action {
            "getWindowForTarget" => {
                let (width, height) = state.viewport.map_or((1280.0, 720.0), |v| (v.width, v.height));
                Ok(Some(serde_json::json!({
                    "windowId": cmd.page,
                    "bounds": { "left": 0, "top": 0, "width": width, "height": height, "windowState": "normal" },
                })))
            }
            "setWindowBounds" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    window_id: u64,
                }
                if cmd.command.params::<P>()?.window_id != cmd.page {
                    return Err(CdpError::server("Browser window not found"));
                }
                Ok(Some(serde_json::json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    fn emulation_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        match action {
            // Headless pages always behave as focused; there is no window focus to emulate.
            "setFocusEmulationEnabled" => Ok(Some(serde_json::json!({}))),
            "setDeviceMetricsOverride" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    width: f64,
                    height: f64,
                    device_scale_factor: f64,
                    mobile: bool,
                }
                let p: P = cmd.command.params()?;
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.viewport = Some(Viewport {
                    width: p.width,
                    height: p.height,
                    device_scale_factor: p.device_scale_factor,
                    mobile: p.mobile,
                });
                Ok(Some(serde_json::json!({})))
            }
            // No matchMedia/CSS media evaluation exists yet, so there is no
            // observable media state to change; the request is recorded.
            "setEmulatedMedia" => {
                let p: serde_json::Value = cmd.command.params()?;
                if let Some(state) = self.pages.get_mut(&cmd.page) {
                    state.emulated_media = Some(p);
                }
                Ok(Some(serde_json::json!({})))
            }
            // navigator and HTTP loading do not exist yet; the override is
            // recorded for the loader and navigator once they land.
            "setUserAgentOverride" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    user_agent: String,
                    accept_language: Option<String>,
                }
                let p: P = cmd.command.params()?;
                if let Some(state) = self.pages.get_mut(&cmd.page) {
                    state.user_agent = Some((p.user_agent, p.accept_language));
                }
                Ok(Some(serde_json::json!({})))
            }
            // JavaScriptCore's Intl/Date use the process locale and time zone;
            // per-page overrides are not implemented. Requests that match the
            // current values are true no-ops; others are refused.
            "setLocaleOverride" | "setTimezoneOverride" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    locale: Option<String>,
                    timezone_id: Option<String>,
                }
                let p: P = cmd.command.params()?;
                let (wanted, probe) = match action {
                    "setLocaleOverride" => (p.locale, "new Intl.DateTimeFormat().resolvedOptions().locale"),
                    _ => (p.timezone_id, "new Intl.DateTimeFormat().resolvedOptions().timeZone"),
                };
                let Some(wanted) = wanted.filter(|w| !w.is_empty()) else { return Ok(Some(serde_json::json!({}))) };
                let current = self
                    .main_realm(cmd.page)
                    .and_then(|r| r.scope().ok().and_then(|s| s.eval(probe, None, 1).ok().and_then(|v| v.as_string())));
                if current.as_deref() == Some(wanted.as_str()) {
                    Ok(Some(serde_json::json!({})))
                } else {
                    Err(CdpError::server(format!(
                        "{} to '{wanted}' is not supported (engine uses '{}')",
                        cmd.command.method,
                        current.unwrap_or_default()
                    )))
                }
            }
            "clearDeviceMetricsOverride" => {
                if let Some(state) = self.pages.get_mut(&cmd.page) {
                    state.viewport = None;
                }
                Ok(Some(serde_json::json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }
}

impl Host for EngineHost {
    fn create_context(&mut self) -> CdpResult<ContextKey> {
        let key = self.next_context;
        self.next_context += 1;
        self.contexts.insert(key);
        Ok(key)
    }

    fn dispose_context(&mut self, context: ContextKey) {
        let pages: Vec<PageKey> = self.pages.iter().filter(|(_, p)| p.context == context).map(|(k, _)| *k).collect();
        for page in pages {
            self.close_page(page);
        }
        self.contexts.remove(&context);
    }

    fn create_page(&mut self, context: ContextKey) -> CdpResult<(PageKey, String)> {
        if !self.contexts.contains(&context) {
            return Err(CdpError::invalid_params("No browser context with the given id found"));
        }
        let id = self.thread.browser.create_page().map_err(|e| CdpError::server(format!("cannot create page: {e}")))?;
        let key = PageKey::from(id.0);
        self.pages.insert(
            key,
            PageState {
                id,
                context,
                sessions: Vec::new(),
                pending_realms: Vec::new(),
                navigating: false,
                pending_navigations: Vec::new(),
                viewport: None,
                emulated_media: None,
                user_agent: None,
                next_script_id: 1,
            },
        );
        // Events of the initial about:blank load precede any session.
        self.flush();
        let target_id = self.thread.browser.page(id).expect("page just created").main_frame.id.0.clone();
        Ok((key, target_id))
    }

    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()> {
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let result = self.thread.browser.navigate(id, lp_browser::NavigateOpts::address_bar(url));
        self.flush();
        result.map(|_| ()).map_err(|e| CdpError::server(format!("navigation failed: {e}")))
    }

    fn close_page(&mut self, page: PageKey) {
        let Some(state) = self.pages.remove(&page) else { return };
        for session in &state.sessions {
            self.sessions.remove(session);
        }
        self.thread.browser.close_page(state.id);
        // Realm/frame teardown events have no session left to go to.
        self.flush();
    }

    fn page_url(&self, page: PageKey) -> String {
        self.pages
            .get(&page)
            .and_then(|s| self.thread.browser.page(s.id))
            .map_or_else(|| "about:blank".to_owned(), |p| p.main_frame.url.clone())
    }

    fn page_title(&self, _page: PageKey) -> String {
        // No document model yet: about:blank has an empty title.
        String::new()
    }

    fn session_attached(&mut self, page: PageKey, session_id: &str) {
        if let Some(state) = self.pages.get_mut(&page) {
            state.sessions.push(session_id.to_owned());
        }
        self.sessions.insert(
            session_id.to_owned(),
            Session {
                page_enabled: false,
                lifecycle_events: false,
                runtime_enabled: false,
                handles: Rc::new(RefCell::new(HandleTable::new(HandleTable::<Persistent>::DEFAULT_LIMIT))),
            },
        );
    }

    fn session_detached(&mut self, page: PageKey, session_id: &str) {
        if let Some(state) = self.pages.get_mut(&page) {
            state.sessions.retain(|s| s != session_id);
        }
        // Dropping the session releases its Persistent handles.
        self.sessions.remove(session_id);
    }

    fn page_command(&mut self, cmd: PageCommand<'_, '_>, _out: &mut dyn Sink) {
        let result = self.page_command_result(&cmd);
        // Events produced while handling precede the response (Chrome order).
        self.flush();
        match result {
            Ok(Some(value)) => self.reply(&cmd, &value),
            // The response is sent later (navigation commit, awaited promise).
            Ok(None) => {}
            Err(error) => self.out.push(msg::error_frame(Some(cmd.command.id), &error, Some(cmd.session_id))),
        }
    }

    fn browser_command(&mut self, command: &Command<'_>, context: Option<ContextKey>, out: &mut dyn Sink) {
        match command.method.as_ref() {
            "Browser.setDownloadBehavior" => {
                #[derive(Deserialize)]
                struct P {
                    behavior: String,
                }
                let result = command.params::<P>().and_then(|p| match p.behavior.as_str() {
                    // Nothing can be downloaded yet (no network), so denying
                    // and the default are exactly what happens.
                    "deny" | "default" => Ok(()),
                    // Playwright asks for allowAndName on every new context.
                    // Downloads cannot start without a network stack, so the
                    // setting is accepted; real download support is tracked in
                    // docs/automation/CDP_COVERAGE.md.
                    "allow" | "allowAndName" => Ok(()),
                    _ => Err(CdpError::invalid_params("Unrecognized download behavior")),
                });
                let _ = context;
                match result {
                    Ok(()) => out.send(msg::result_frame(command.id, &Empty {}, None)),
                    Err(e) => out.send(msg::error_frame(Some(command.id), &e, None)),
                }
            }
            _ => out.send(msg::error_frame(Some(command.id), &CdpError::unsupported(&command.method), None)),
        }
    }
}
