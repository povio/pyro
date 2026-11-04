//! Browser-level CDP state: browser contexts, page targets and flattened
//! sessions (`Target.*`, `Browser.getVersion`), plus routing of
//! session-scoped commands to the engine through [`Host`].
//!
//! Behavior follows upstream `src/server/cdp/domains/target.zig` with two
//! deliberate differences, both observable only through the protocol:
//! - several browser contexts may coexist if the host supports them
//!   (upstream allows one per process);
//! - no placeholder `STARTUP` page is attached on `Target.setAutoAttach`
//!   (upstream fakes one for Puppeteer; Playwright does not need it).
//!
//! `Target.targetCreated/Destroyed` are only sent after `Target.setDiscoverTargets`.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::msg::{self, CdpError, CdpResult, Command, Empty};
use crate::ws::OutFrame;

/// Identity reported by `Browser.getVersion`. Matches upstream Lightpanda so
/// Playwright takes the same code paths (non-"Headless" UA, Chrome 124).
pub const PROTOCOL_VERSION: &str = "1.3";
pub const PRODUCT: &str = "Chrome/124.0.6367.29";
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Engine handle for a browser context, chosen by the host.
pub type ContextKey = u64;
/// Engine handle for a page (top-level frame tree), chosen by the host.
pub type PageKey = u64;

/// Engine handle for a dedicated worker, chosen by the host.
pub type WorkerKey = u64;

/// A dedicated worker started or ended ([`Host::take_worker_changes`]).
pub enum WorkerChange {
    Started { worker: WorkerKey, page: PageKey, url: String },
    Stopped { worker: WorkerKey },
}

/// Receives encoded protocol messages in emission order.
pub trait Sink {
    fn send(&mut self, frame: OutFrame);
}

impl Sink for Vec<OutFrame> {
    fn send(&mut self, frame: OutFrame) {
        self.push(frame);
    }
}

/// Context for a command dispatched to a page session.
pub struct PageCommand<'a, 'c> {
    pub page: PageKey,
    pub target_id: &'a str,
    pub session_id: &'a str,
    pub command: &'a Command<'c>,
}

/// The engine side, implemented on the browser thread.
///
/// Output contract: everything for one connection goes through that
/// connection's single ordered channel. The `out` sinks passed to
/// [`Host::page_command`] and [`Host::browser_command`] are that channel; a
/// host that answers later (awaited promises, navigation commits, events)
/// must hold its own clone of the same channel ([`crate::outbox::Outbox`]),
/// never a different destination, so that ordering is preserved.
/// `Target.createBrowserContext` `proxyServer` and `proxyBypassList`.
#[derive(Clone, Copy, Debug)]
pub struct ContextProxy<'a> {
    pub server: &'a str,
    pub bypass_list: Option<&'a str>,
}

pub trait Host {
    /// Creates an isolated browser context (cookies, storage, cache).
    fn create_context(&mut self) -> CdpResult<ContextKey>;
    /// `create_context` with its own proxy (`Target.createBrowserContext`).
    fn create_proxied_context(&mut self, _proxy: ContextProxy<'_>) -> CdpResult<ContextKey> {
        Err(CdpError::unsupported("Target.createBrowserContext proxyServer"))
    }
    fn dispose_context(&mut self, context: ContextKey);
    /// Creates an `about:blank` page. Returns its key and target id; the target
    /// id is the main frame id, as in Chrome and upstream.
    fn create_page(&mut self, context: ContextKey) -> CdpResult<(PageKey, String)>;
    /// Starts a navigation (used by `Target.createTarget` with a URL).
    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()>;
    fn close_page(&mut self, page: PageKey);
    fn page_url(&self, page: PageKey) -> String;
    fn page_title(&self, page: PageKey) -> String;
    /// A session attached to `page`; the host may start emitting its events.
    fn session_attached(&mut self, page: PageKey, session_id: &str);
    /// Popups the engine asked for since the last call (links/`window.open`
    /// targeting a new browsing context), oldest first.
    fn take_popup_requests(&mut self) -> Vec<PopupRequest> {
        Vec::new()
    }
    /// Pages the engine asked to close since the last call (`Page.close`),
    /// oldest first: their targets are closed like `Target.closeTarget`.
    fn take_close_requests(&mut self) -> Vec<PageKey> {
        Vec::new()
    }
    /// Popup actions the engine collected, oldest first. Taken after the popup
    /// requests, so the popups they name are open.
    fn take_popup_actions(&mut self) -> Vec<PopupAction> {
        Vec::new()
    }
    /// The `browserContextId` the protocol gave the host's context `key`.
    fn context_named(&mut self, _key: ContextKey, _id: &str) {}
    /// Marks a paused popup that has not started its first navigation.
    /// Chrome reports such a frame's URL as ":" in `Page.getFrameTree`, which
    /// is how clients know to wait for the first real navigation.
    fn set_initial_popup(&mut self, _page: PageKey, _initial: bool) {}
    /// A popup the engine requested is now a target (`LP.run` `popup` steps).
    fn popup_opened(
        &mut self,
        _opener: PageKey,
        _page: PageKey,
        _target_id: &str,
        _handle: Option<std::rc::Rc<dyn std::any::Any>>,
    ) {
    }
    /// Workers that started or ended since the last call, oldest first.
    fn take_worker_changes(&mut self) -> Vec<WorkerChange> {
        Vec::new()
    }
    /// A command on a worker's session (`Runtime.*`). Must send exactly one
    /// response, now or later, unless the session detaches first.
    fn worker_command(&mut self, _worker: WorkerKey, session_id: &str, command: &Command<'_>, out: &mut dyn Sink) {
        out.send(msg::error_frame(Some(command.id), &CdpError::unsupported(&command.method), Some(session_id)));
    }
    /// A worker's session detached.
    fn worker_session_detached(&mut self, _worker: WorkerKey, _session_id: &str) {}
    /// A session detached; the host must stop emitting events for it and drop
    /// per-session state (enabled domains, object handles, interception).
    fn session_detached(&mut self, page: PageKey, session_id: &str);

    /// Pages the host booted before this connection existed (`serve
    /// --prepare`), to adopt as prepared pages. Taken once.
    fn take_prebooted(&mut self) -> Vec<PrebootedEntry> {
        Vec::new()
    }
    /// Whether the page's main document finished loading (prepared pages).
    fn page_loaded(&self, _page: PageKey) -> bool {
        false
    }
    /// Whether a prepared page is still on its initial `about:blank` with no
    /// navigation under way (the one started at boot failed or never
    /// took), so it must be started again before the page is handed out.
    fn page_needs_navigation(&self, _page: PageKey) -> bool {
        false
    }
    /// Installs a script that runs in every new document of the page.
    fn add_init_script(&mut self, _page: PageKey, _source: &str) {}
    /// Sets the page's viewport size.
    fn set_viewport(&mut self, _page: PageKey, _width: u32, _height: u32) {}
    /// A prepared page finished booting and now waits: shrink it to its
    /// live size (full collection, free pages returned).
    fn settle_page(&mut self, _page: PageKey) {}
    /// Handles a page-scoped command. Must send exactly one response for
    /// `command.id`, now or later, unless the session detaches first.
    fn page_command(&mut self, cmd: PageCommand<'_, '_>, out: &mut dyn Sink);
    /// Browser-scoped commands outside Target/Browser.getVersion (e.g.
    /// Storage.*, Browser.setDownloadBehavior). `context` is resolved from a
    /// `browserContextId` param when present.
    fn browser_command(&mut self, command: &Command<'_>, context: Option<ContextKey>, out: &mut dyn Sink);
}

struct ContextEntry {
    id: String,
    key: ContextKey,
    /// The implicit context of `createTarget` without `browserContextId`.
    /// Not listed by `getBrowserContexts` and not disposable, as in Chrome.
    is_default: bool,
    /// Holds a prepared page the client has not acquired yet: not listed.
    hidden: bool,
}

/// A page booted ahead of a test (`serve --prepare`,
/// `Lightningpanda.registerPreparedPage`): its own browser context (seeded
/// local storage), init scripts, viewport, then a navigation to `url`.
/// `pool` pages per profile are kept ready; each acquisition starts the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedProfile {
    pub url: String,
    /// Run before document scripts in every page of this profile context, including popups.
    #[serde(default)]
    pub init_scripts: Vec<String>,
    #[serde(default)]
    pub viewport: Option<ViewportSize>,
    #[serde(default = "default_pool")]
    pub pool: usize,
    /// Boot the replacement only once the acquired page's context is
    /// closed (instead of when the thread is quiet): at most `pool` booted
    /// apps instead of `pool` + 1, at the price of the next test waiting
    /// for the boot when tests run back to back.
    #[serde(default)]
    pub refill_after_close: bool,
    /// `LP.run` steps run once on the booted page before tests get it
    /// (`serve --fork` only: the template runs them, and every connection
    /// forked afterwards starts from the resulting state).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<serde_json::Value>,
    /// Starts from another profile's page (its id) instead of booting
    /// `url`: `serve --fork` forks that profile's snapshot and runs only this
    /// profile's `setup` on it. Pages of such a profile are never booted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportSize {
    pub width: u32,
    pub height: u32,
}

fn default_pool() -> usize {
    1
}

/// `serve --fork`: hands a profile to the template process; its id there.
pub type RegisterPrepared = Box<dyn Fn(&PreparedProfile) -> Result<usize, String>>;
pub type UnregisterPrepared = Box<dyn Fn(usize) -> Result<bool, String>>;

/// At most this many pages booting ahead per profile.
pub const MAX_POOL: usize = 8;

/// A page the host booted ahead of the connection, with its context.
#[derive(Debug, Clone)]
pub struct PrebootedEntry {
    pub context: ContextKey,
    pub page: PageKey,
    pub target_id: String,
    /// Index into the profiles the connection starts with.
    pub profile: usize,
}

/// A booted (or booting) page waiting for a test; not a target yet.
struct PreparedEntry {
    profile: usize,
    context_id: String,
    page: PageKey,
    target_id: String,
    /// `settle_page` ran once the page had loaded.
    settled: bool,
}

/// Per-connection budgets: a client cannot grow protocol state without limit.
pub const MAX_CONTEXTS: usize = 256;
pub const MAX_TARGETS: usize = 256;
pub const MAX_SESSIONS: usize = 512;

/// A page the engine wants opened as a popup of `opener`.
#[derive(Debug, Clone)]
pub struct PopupRequest {
    pub opener: PageKey,
    pub url: String,
    /// Whether the popup may script its opener (`canAccessOpener`).
    pub opener_access: bool,
    /// Opaque to this layer: goes back to the host with `popup_opened`, and
    /// dropping it says the popup was refused.
    pub handle: Option<std::rc::Rc<dyn std::any::Any>>,
}

/// What a page asked of a popup it opened.
#[derive(Debug, Clone)]
pub enum PopupAction {
    Close(PageKey),
    Navigate(PageKey, String),
}

struct TargetEntry {
    id: String,
    context_id: String,
    page: PageKey,
    /// Target that opened this popup (`TargetInfo.openerId`).
    opener_id: Option<String>,
    can_access_opener: bool,
    /// Popup navigation held until the client's `Runtime.runIfWaitingForDebugger`
    /// (Chrome pauses popups so init scripts and listeners are installed first).
    pending_url: Option<String>,
    /// The prepared-page profile this target was acquired from.
    prepared_profile: Option<usize>,
}

struct SessionEntry {
    target_id: String,
    parent: Option<String>,
    auto_attached: bool,
    /// A session on the page's tab target (`target_id` is the page's).
    tab: bool,
    /// A session on a dedicated worker (`target_id` is the worker's).
    worker: Option<WorkerKey>,
    /// A page session that asked for its workers (`Target.setAutoAttach`).
    auto_attach_workers: bool,
}

/// A live dedicated worker. A worker target has no entry in `targets`: it is
/// not a page and only exists for clients through its page's sessions.
struct WorkerEntry {
    key: WorkerKey,
    page: PageKey,
    target_id: String,
    url: String,
}

/// Per-connection browser protocol state.
pub struct Browser<H: Host> {
    host: H,
    auto_attach: bool,
    discover: bool,
    /// Browser-level auto-attach to tab targets (puppeteer: pages excluded,
    /// tabs included). Each page has a virtual tab target, `tab-<page id>`,
    /// whose session attaches the page when asked (`Target.setAutoAttach`
    /// on the tab session), as Chrome's tab targets do.
    tab_attach: bool,
    /// `setDiscoverTargets` included tab targets.
    discover_tabs: bool,
    contexts: Vec<ContextEntry>,
    targets: Vec<TargetEntry>,
    workers: Vec<WorkerEntry>,
    sessions: HashMap<String, SessionEntry>,
    browser_sessions: HashSet<String>,
    next_id: u64,
    prepared_profiles: Vec<PreparedProfile>,
    retired_profiles: HashSet<usize>,
    prepared: Vec<PreparedEntry>,
    /// The last refill could create nothing (limits, failures): wait for
    /// the next acquisition before trying again.
    refill_blocked: bool,
    /// `serve --fork`: this connection is a fork of the template process
    /// (see [`crate::fork`]), which boots prepared pages for every later
    /// connection. Nothing is booted here; registrations go to the template.
    fork: Option<RegisterPrepared>,
    fork_unregister: Option<UnregisterPrepared>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo<'a> {
    target_id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    title: &'a str,
    url: &'a str,
    attached: bool,
    can_access_opener: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    opener_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    opener_frame_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_context_id: Option<&'a str>,
}

impl<H: Host> Browser<H> {
    pub fn new(host: H) -> Self {
        Self {
            host,
            auto_attach: false,
            discover: false,
            tab_attach: false,
            discover_tabs: false,
            contexts: Vec::new(),
            targets: Vec::new(),
            workers: Vec::new(),
            sessions: HashMap::new(),
            browser_sessions: HashSet::new(),
            next_id: 1,
            prepared_profiles: Vec::new(),
            retired_profiles: HashSet::new(),
            prepared: Vec::new(),
            refill_blocked: false,
            fork: None,
            fork_unregister: None,
        }
    }

    /// Makes this connection a fork child (see the `fork` field): no
    /// refills, and `registerPreparedPage` goes through `register`.
    pub fn set_fork_child(&mut self, register: RegisterPrepared) {
        self.fork = Some(register);
    }

    pub fn set_fork_unregister(&mut self, unregister: UnregisterPrepared) {
        self.fork_unregister = Some(unregister);
    }

    pub fn prepared_profile_active(&self, id: usize) -> bool {
        id < self.prepared_profiles.len() && !self.retired_profiles.contains(&id)
    }

    pub fn check_profile_retirement(&self, id: usize) -> CdpResult<bool> {
        if id >= self.prepared_profiles.len() {
            return Err(CdpError::invalid_params("Unknown prepared profile"));
        }
        if self.retired_profiles.contains(&id) {
            return Ok(false);
        }
        if self
            .prepared_profiles
            .iter()
            .enumerate()
            .any(|(child, profile)| self.prepared_profile_active(child) && profile.base == Some(id))
        {
            return Err(CdpError::invalid_params("Prepared profile still has dependent profiles"));
        }
        Ok(true)
    }

    /// Retires only waiting pages. Acquired targets remain valid until their owner closes them.
    pub fn unregister_prepared_profile(&mut self, id: usize) -> CdpResult<bool> {
        if !self.check_profile_retirement(id)? {
            return Ok(false);
        }
        self.retired_profiles.insert(id);
        let waiting = std::mem::take(&mut self.prepared);
        for entry in waiting {
            if entry.profile == id {
                self.host.close_page(entry.page);
                self.dispose_context(&entry.context_id, &mut Discard);
            } else {
                self.prepared.push(entry);
            }
        }
        let profile = &mut self.prepared_profiles[id];
        profile.url.clear();
        profile.init_scripts.clear();
        profile.setup.clear();
        profile.viewport = None;
        profile.base = None;
        Ok(true)
    }

    /// `new`, then starts booting `profiles`' pages right away so the first
    /// acquisition finds them ready (or at least under way).
    pub fn with_prepared(host: H, profiles: Vec<PreparedProfile>) -> Self {
        let mut browser = Self::new(host);
        browser.prepared_profiles = profiles;
        for e in browser.host.take_prebooted() {
            if e.profile >= browser.prepared_profiles.len() {
                browser.host.close_page(e.page);
                browser.host.dispose_context(e.context);
                continue;
            }
            let context_id = browser.mint("");
            browser.host.context_named(e.context, &context_id);
            browser.contexts.push(ContextEntry {
                id: context_id.clone(),
                key: e.context,
                is_default: false,
                hidden: true,
            });
            browser.prepared.push(PreparedEntry {
                profile: e.profile,
                context_id,
                page: e.page,
                target_id: e.target_id,
                // The spare thread settles what it booted before the connection.
                settled: true,
            });
        }
        browser.refill_prepared();
        browser
    }

    /// The profiles prepared pages are booted for; a profile's id is its index.
    pub fn prepared_profiles(&self) -> &[PreparedProfile] {
        &self.prepared_profiles
    }

    /// Prepared pages waiting for a test, as (profile id, page).
    pub fn prepared_pages(&self) -> Vec<(usize, PageKey)> {
        self.prepared.iter().map(|e| (e.profile, e.page)).collect()
    }

    /// Adds a profile and starts booting its pages; its id (`serve --fork`
    /// template, which registers profiles for every connection).
    pub fn add_prepared_profile(&mut self, profile: PreparedProfile) -> usize {
        self.prepared_profiles.push(profile);
        self.refill_blocked = false;
        self.refill_prepared();
        self.prepared_profiles.len() - 1
    }

    /// Sets profile `id`, adding never-booted placeholders before it if
    /// needed (a derived `serve --fork` template learns profiles by id).
    pub fn set_prepared_profile(&mut self, id: usize, profile: PreparedProfile) {
        while self.prepared_profiles.len() <= id {
            let placeholder = PreparedProfile { base: Some(self.prepared_profiles.len()), ..profile.clone() };
            self.prepared_profiles.push(placeholder);
        }
        self.prepared_profiles[id] = profile;
    }

    /// Moves a waiting page of profile `from` to profile `to` (a derived
    /// snapshot's page after its setup); whether there was one.
    pub fn relabel_prepared(&mut self, from: usize, to: usize) -> bool {
        match self.prepared.iter_mut().find(|e| e.profile == from) {
            Some(e) => {
                e.profile = to;
                true
            }
            None => false,
        }
    }

    /// Hands an acquired prepared page back to the pool, waiting and hidden
    /// again, with its sessions detached (the `serve --fork` template acquires
    /// a page to run a profile's setup steps on it).
    pub fn return_to_prepared(&mut self, target_id: &str) -> CdpResult<()> {
        let pos = self
            .targets
            .iter()
            .position(|t| t.id == target_id && t.prepared_profile.is_some())
            .ok_or_else(|| CdpError::invalid_params("not an acquired prepared page"))?;
        if !self.prepared_profile_active(self.targets[pos].prepared_profile.expect("checked")) {
            return Err(CdpError::invalid_params("Prepared profile was retired"));
        }
        let sessions: Vec<String> =
            self.sessions.iter().filter(|(_, s)| s.target_id == target_id).map(|(id, _)| id.clone()).collect();
        for session in sessions {
            self.detach_session(&session, &mut Discard);
        }
        let target = self.targets.remove(pos);
        if let Some(c) = self.contexts.iter_mut().find(|c| c.id == target.context_id) {
            c.hidden = true;
        }
        self.prepared.push(PreparedEntry {
            profile: target.prepared_profile.expect("checked"),
            context_id: target.context_id,
            page: target.page,
            target_id: target.id,
            settled: false,
        });
        Ok(())
    }

    /// No page is open on this connection: no test is using it, so a refill
    /// delays nobody.
    pub fn no_open_pages(&self) -> bool {
        self.targets.is_empty()
    }

    /// Whether some profile has fewer pages waiting than its pool asks for
    /// and a refill may succeed. The connection loop refills once the
    /// thread has been quiet for a moment (or right away when no page is
    /// open), so a test's own commands are never queued behind the next
    /// page's boot.
    pub fn needs_refill(&self) -> bool {
        self.fork.is_none()
            && !self.refill_blocked
            && (0..self.prepared_profiles.len()).any(|i| self.profile_wants_page(i))
    }

    /// Whether profile `i` is below its pool and allowed to boot now.
    fn profile_wants_page(&self, i: usize) -> bool {
        if !self.prepared_profile_active(i) {
            return false;
        }
        let profile = &self.prepared_profiles[i];
        if profile.base.is_some() {
            return false; // its page comes from the base profile's
        }
        let waiting = self.prepared.iter().filter(|e| e.profile == i).count();
        let in_use = self.targets.iter().filter(|t| t.prepared_profile == Some(i)).count();
        let pool = profile.pool.clamp(1, MAX_POOL);
        if profile.refill_after_close {
            waiting + in_use < pool
        } else {
            waiting < pool
        }
    }

    /// Settles prepared pages that finished loading since the last call
    /// (one full collection each), so a waiting page costs its live size.
    /// Called by the connection loop when the thread is quiet.
    pub fn settle_prepared(&mut self) {
        for i in 0..self.prepared.len() {
            if !self.prepared[i].settled && self.host.page_loaded(self.prepared[i].page) {
                self.prepared[i].settled = true;
                let page = self.prepared[i].page;
                self.host.settle_page(page);
            }
        }
    }

    /// Whether some loaded prepared page has not been settled yet.
    pub fn needs_settle(&self) -> bool {
        self.prepared.iter().any(|e| !e.settled && self.host.page_loaded(e.page))
    }

    /// Boots the missing pages now (see `needs_refill`).
    pub fn refill_prepared(&mut self) {
        if self.fork.is_some() {
            return;
        }
        if !self.fill_prepared() {
            self.refill_blocked = true;
        }
    }

    /// Boots pages until every profile has `pool` of them waiting; whether
    /// any page was started.
    fn fill_prepared(&mut self) -> bool {
        let mut started = false;
        for index in 0..self.prepared_profiles.len() {
            loop {
                let profile = self.prepared_profiles[index].clone();
                if !self.profile_wants_page(index) {
                    break;
                }
                if self.contexts.len() >= MAX_CONTEXTS || self.targets.len() + self.prepared.len() >= MAX_TARGETS {
                    break;
                }
                let Ok(context_id) = self.new_context_with(false, true) else { break };
                let key = self.contexts.iter().find(|c| c.id == context_id).expect("just created").key;
                let Ok((page, target_id)) = self.host.create_page(key) else {
                    self.dispose_context(&context_id, &mut Discard);
                    break;
                };
                for script in &profile.init_scripts {
                    self.host.add_init_script(page, script);
                }
                if let Some(v) = profile.viewport {
                    self.host.set_viewport(page, v.width, v.height);
                }
                if self.host.navigate(page, &profile.url).is_err() {
                    self.host.close_page(page);
                    self.dispose_context(&context_id, &mut Discard);
                    break;
                }
                self.prepared.push(PreparedEntry { profile: index, context_id, page, target_id, settled: false });
                started = true;
            }
        }
        started
    }

    /// `Lightningpanda.*`: the prepared-page extension.
    fn lightningpanda_command(&mut self, action: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let id = cmd.id;
        match action {
            "registerPreparedPage" => {
                let profile: PreparedProfile = cmd.params()?;
                if lp_net::Url::parse(&profile.url).is_err() {
                    return Err(CdpError::invalid_params("url is not an absolute URL"));
                }
                if profile.pool == 0 || profile.pool > MAX_POOL {
                    return Err(CdpError::invalid_params("pool must be between 1 and 8"));
                }
                if (!profile.setup.is_empty() || profile.base.is_some()) && self.fork.is_none() {
                    return Err(CdpError::invalid_params("setup and base need serve --fork"));
                }
                if let Some(register) = &self.fork {
                    // The template boots it; connections forked from then on have it.
                    let profile_id = register(&profile).map_err(CdpError::server)?;
                    out.send(msg::result_frame(id, &serde_json::json!({ "profileId": profile_id }), cmd.session_id()));
                    return Ok(());
                }
                self.prepared_profiles.push(profile);
                self.refill_blocked = false;
                self.refill_prepared();
                let profile_id = self.prepared_profiles.len() - 1;
                out.send(msg::result_frame(id, &serde_json::json!({ "profileId": profile_id }), cmd.session_id()));
            }
            "unregisterPreparedPage" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    profile_id: usize,
                }
                let p: P = cmd.params()?;
                let removed = if let Some(unregister) = &self.fork_unregister {
                    unregister(p.profile_id).map_err(CdpError::server)?
                } else if self.fork.is_some() {
                    return Err(CdpError::server("Profile retirement is unavailable on this fork connection"));
                } else {
                    self.unregister_prepared_profile(p.profile_id)?
                };
                out.send(msg::result_frame(id, &serde_json::json!({ "removed": removed }), cmd.session_id()));
            }
            "acquirePreparedPage" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    url: Option<String>,
                    profile_id: Option<usize>,
                    /// Attach a flat session in the same call (as `attachToTarget`).
                    #[serde(default)]
                    attach: bool,
                }
                let p: P = cmd.params()?;
                let matches = |b: &Self, e: &PreparedEntry| {
                    p.profile_id.is_none_or(|i| e.profile == i)
                        && p.url.as_deref().is_none_or(|u| b.prepared_profiles[e.profile].url == u)
                };
                // A loaded page first, else the one booting longest.
                let pos = self
                    .prepared
                    .iter()
                    .position(|e| matches(self, e) && self.host.page_loaded(e.page))
                    .or_else(|| self.prepared.iter().position(|e| matches(self, e)));
                let Some(pos) = pos else {
                    return Err(CdpError::invalid_params(
                        "No prepared page for that url: register one with Lightningpanda.registerPreparedPage or serve --prepare",
                    ));
                };
                if self.targets.len() >= MAX_TARGETS {
                    return Err(CdpError::server("Too many targets"));
                }
                let entry = self.prepared.remove(pos);
                let url = self.prepared_profiles[entry.profile].url.clone();
                let ready = self.host.page_loaded(entry.page);
                // `ready: false` means the page is navigating to the
                // profile URL: a `loaded` step then waits for that
                // navigation. Start it (again) when nothing is under way.
                if !ready && self.host.page_needs_navigation(entry.page) {
                    if let Err(e) = self.host.navigate(entry.page, &url) {
                        self.host.close_page(entry.page);
                        self.dispose_context(&entry.context_id, &mut Discard);
                        return Err(e);
                    }
                }
                if let Some(c) = self.contexts.iter_mut().find(|c| c.id == entry.context_id) {
                    c.hidden = false;
                }
                let target_id = entry.target_id.clone();
                self.targets.push(TargetEntry {
                    id: target_id.clone(),
                    context_id: entry.context_id.clone(),
                    page: entry.page,
                    opener_id: None,
                    can_access_opener: false,
                    pending_url: None,
                    prepared_profile: Some(entry.profile),
                });
                if self.discover || self.discover_tabs {
                    self.send_target_created(self.targets.len() - 1, out);
                }
                let session_id = if self.auto_attach {
                    Some(self.attach(&target_id, None, true, out)?)
                } else if self.tab_attach {
                    self.attach_tab(&target_id, out)?;
                    None
                } else if p.attach {
                    Some(self.attach(&target_id, cmd.session_id(), false, out)?)
                } else {
                    None
                };
                out.send(msg::result_frame(
                    id,
                    &serde_json::json!({
                        "targetId": target_id,
                        "browserContextId": entry.context_id,
                        "sessionId": session_id,
                        "url": url,
                        "ready": ready,
                    }),
                    cmd.session_id(),
                ));
                // The replacement boots once the thread is quiet (`needs_refill`).
                self.refill_blocked = false;
            }
            "capabilities" => {
                out.send(msg::result_frame(id, &serde_json::json!({ "fork": self.fork.is_some() }), cmd.session_id()));
            }
            "preparedPages" => {
                let pages: Vec<serde_json::Value> = self
                    .prepared
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "profileId": e.profile,
                            "url": self.prepared_profiles[e.profile].url,
                            "ready": self.host.page_loaded(e.page),
                        })
                    })
                    .collect();
                out.send(msg::result_frame(id, &serde_json::json!({ "pages": pages }), cmd.session_id()));
            }
            _ => return Err(CdpError::method_not_found(&cmd.method)),
        }
        Ok(())
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    fn mint(&mut self, prefix: &str) -> String {
        let n = self.next_id;
        self.next_id += 1;
        format!("{prefix}{n:032X}")
    }

    /// Handles one text message from the client.
    pub fn handle_text(&mut self, text: &str, out: &mut dyn Sink) {
        let parse = crate::perf::span("cdp.parse");
        let parsed = msg::parse_command(text);
        drop(parse);
        let command = match parsed {
            Ok(command) => command,
            Err(failure) => {
                out.send(msg::error_frame(failure.id, &failure.error, failure.session_id.as_deref()));
                return;
            }
        };
        let session_id = command.session_id().map(str::to_owned);
        let _t = crate::perf::span_with(|| format!("cdp.dispatch {}", command.method));
        if let Err(error) = self.dispatch(&command, out) {
            out.send(msg::error_frame(Some(command.id), &error, session_id.as_deref()));
        }
    }

    fn dispatch(&mut self, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        if let Some(session_id) = cmd.session_id() {
            if !self.browser_sessions.contains(session_id) {
                return self.dispatch_session(session_id, cmd, out);
            }
            // Browser sessions share context/target identity, but automatic
            // discovery/attachment settings are not implemented per session.
            if matches!(
                cmd.method.as_ref(),
                "Target.setAutoAttach" | "Target.setDiscoverTargets" | "Target.attachToBrowserTarget"
            ) {
                return Err(CdpError::unsupported(&cmd.method));
            }
        }
        let (domain, action) = cmd.domain_and_action().ok_or_else(|| CdpError::method_not_found(&cmd.method))?;
        match (domain, action) {
            #[cfg(feature = "perf-trace")]
            ("LP", "perfReset") => {
                crate::perf::reset();
                out.send(msg::result_frame(cmd.id, &Empty {}, cmd.session_id()));
                Ok(())
            }
            #[cfg(feature = "perf-trace")]
            ("LP", "perfStats") => {
                out.send(msg::result_frame(cmd.id, &crate::perf::snapshot(), cmd.session_id()));
                Ok(())
            }
            ("Browser", "getVersion") => {
                #[derive(Serialize)]
                #[serde(rename_all = "camelCase")]
                struct Version {
                    protocol_version: &'static str,
                    product: &'static str,
                    revision: &'static str,
                    user_agent: &'static str,
                    js_version: &'static str,
                }
                let version = Version {
                    protocol_version: PROTOCOL_VERSION,
                    product: PRODUCT,
                    revision: "",
                    user_agent: USER_AGENT,
                    js_version: "",
                };
                out.send(msg::result_frame(cmd.id, &version, cmd.session_id()));
                Ok(())
            }
            ("Runtime", "runIfWaitingForDebugger") if cmd.session_id().is_some() => {
                out.send(msg::result_frame(cmd.id, &Empty {}, cmd.session_id()));
                Ok(())
            }
            ("Target", _) => self.target_command(action, cmd, out),
            ("Lightningpanda", _) => self.lightningpanda_command(action, cmd, out),
            _ => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct ContextParam {
                    browser_context_id: Option<String>,
                }
                let context = if matches!(
                    cmd.method.as_ref(),
                    "Storage.getCookies"
                        | "Storage.setCookies"
                        | "Storage.clearCookies"
                        | "Browser.grantPermissions"
                        | "Browser.resetPermissions"
                        | "Browser.setPermission"
                ) {
                    let params: ContextParam = cmd.params()?;
                    let id = match params.browser_context_id {
                        Some(id) => id,
                        None => self.default_context()?,
                    };
                    Some(self.context(&id)?.key)
                } else {
                    match cmd.raw_params().and_then(|p| serde_json::from_str::<ContextParam>(p.get()).ok()) {
                        Some(ContextParam { browser_context_id: Some(id) }) => Some(self.context(&id)?.key),
                        _ => None,
                    }
                };
                self.host.browser_command(cmd, context, out);
                Ok(())
            }
        }
    }

    fn dispatch_session(&mut self, session_id: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let session =
            self.sessions.get(session_id).ok_or_else(|| CdpError::new(-32001, "Session with given id not found."))?;
        if let Some(worker) = session.worker {
            return self.worker_session_command(session_id, worker, cmd, out);
        }
        let target = self
            .targets
            .iter()
            .find(|t| t.id == session.target_id)
            .ok_or_else(|| CdpError::server("target of session is gone"))?;
        let (page, target_id) = (target.page, target.id.clone());
        if session.tab {
            return self.tab_session_command(session_id, &target_id, cmd, out);
        }
        match cmd.method.as_ref() {
            "Target.getTargets" => self.target_command("getTargets", cmd, out)?,
            // Session-scoped Target commands Playwright sends on page sessions.
            "Target.setAutoAttach" => {
                // OOPIFs are not separate targets here: iframes are same-process
                // frames reported through Page events. Workers are.
                let params: AutoAttachParams = cmd.params()?;
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
                let wanted = params.auto_attach && includes(params.filter.as_deref(), "worker", true);
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.auto_attach_workers = wanted;
                }
                if wanted {
                    let existing: Vec<usize> =
                        (0..self.workers.len()).filter(|i| self.workers[*i].page == page).collect();
                    for index in existing {
                        self.attach_worker(index, session_id, out);
                    }
                }
            }
            "Target.getTargetInfo" => {
                let url = self.host.page_url(page);
                let title = self.host.page_title(page);
                let target = self.target(&target_id)?;
                let info = self.info(target, &url, &title, true);
                out.send(msg::result_frame(cmd.id, &serde_json::json!({ "targetInfo": info }), Some(session_id)));
            }
            "Runtime.runIfWaitingForDebugger" => {
                // Only popups start paused; resuming starts their navigation.
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
                self.release_popup(&target_id, out);
            }
            _ => self.host.page_command(PageCommand { page, target_id: &target_id, session_id, command: cmd }, out),
        }
        Ok(())
    }

    fn context(&self, id: &str) -> CdpResult<&ContextEntry> {
        self.contexts
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| CdpError::invalid_params("No browser context with the given id found"))
    }

    fn target(&self, id: &str) -> CdpResult<&TargetEntry> {
        self.targets
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| CdpError::invalid_params("No target with given id found"))
    }

    fn info<'a>(&self, target: &'a TargetEntry, url: &'a str, title: &'a str, attached: bool) -> TargetInfo<'a> {
        TargetInfo {
            target_id: &target.id,
            kind: "page",
            title,
            url,
            attached,
            can_access_opener: target.can_access_opener,
            // Page targets' main frame ids equal their target ids.
            opener_id: target.opener_id.as_deref(),
            opener_frame_id: target.opener_id.as_deref(),
            browser_context_id: Some(&target.context_id),
        }
    }

    /// The tab target of `target` (`id` is its `tab_id`).
    fn tab_info<'a>(&self, target: &'a TargetEntry, id: &'a str, url: &'a str, title: &'a str) -> TargetInfo<'a> {
        let attached = self.sessions.values().any(|s| s.tab && s.target_id == target.id);
        TargetInfo {
            kind: "tab",
            target_id: id,
            attached,
            opener_id: None,
            opener_frame_id: None,
            can_access_opener: false,
            ..self.info(target, url, title, attached)
        }
    }

    /// Commands on a tab target's session: attaching its page (puppeteer
    /// sends `setAutoAttach` here and waits for the page's
    /// `attachedToTarget` on this session).
    fn tab_session_command(
        &mut self,
        session_id: &str,
        target_id: &str,
        cmd: &Command<'_>,
        out: &mut dyn Sink,
    ) -> CdpResult<()> {
        match cmd.method.as_ref() {
            "Target.setAutoAttach" => {
                let params: AutoAttachParams = cmd.params()?;
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
                let attached = self
                    .sessions
                    .values()
                    .any(|s| !s.tab && s.target_id == target_id && s.parent.as_deref() == Some(session_id));
                if params.auto_attach && includes(params.filter.as_deref(), "page", true) && !attached {
                    // A popup waits for `Runtime.runIfWaitingForDebugger` on its page session.
                    let paused = self.target(target_id)?.pending_url.is_some();
                    self.attach_paused(target_id, Some(session_id), true, paused, out)?;
                }
            }
            "Runtime.runIfWaitingForDebugger" => out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id))),
            "Target.getTargetInfo" => {
                let target = self.target(target_id)?;
                let (url, title) = (self.host.page_url(target.page), self.host.page_title(target.page));
                let tab_id = tab_id(target_id);
                let info = self.tab_info(target, &tab_id, &url, &title);
                out.send(msg::result_frame(cmd.id, &serde_json::json!({ "targetInfo": info }), Some(session_id)));
            }
            "Target.detachFromTarget" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    session_id: String,
                }
                let child = cmd.params::<P>()?.session_id;
                if self.sessions.get(&child).is_none_or(|s| s.parent.as_deref() != Some(session_id)) {
                    return Err(CdpError::new(-32001, "Session is not attached to this browser session."));
                }
                self.detach_session(&child, out);
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
            }
            _ => return Err(CdpError::unsupported(&cmd.method)),
        }
        Ok(())
    }

    /// Attaches a browser-level session to `target_id`'s tab target.
    fn attach_tab(&mut self, target_id: &str, out: &mut dyn Sink) -> CdpResult<String> {
        if self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS {
            return Err(CdpError::server("Too many sessions"));
        }
        let session_id = self.mint("");
        self.sessions.insert(
            session_id.clone(),
            SessionEntry {
                target_id: target_id.to_owned(),
                parent: None,
                auto_attached: true,
                tab: true,
                worker: None,
                auto_attach_workers: false,
            },
        );
        let target = self.target(target_id)?;
        let (url, title) = (self.host.page_url(target.page), self.host.page_title(target.page));
        let tab_id = tab_id(target_id);
        let info = self.tab_info(target, &tab_id, &url, &title);
        out.send(msg::event_frame(
            "Target.attachedToTarget",
            &serde_json::json!({ "sessionId": session_id, "targetInfo": info, "waitingForDebugger": false }),
            None,
        ));
        Ok(session_id)
    }

    /// A new target is attached as the browser-level auto-attach asks: its
    /// page (Playwright), or its tab (puppeteer). `paused`: a popup waiting
    /// for `Runtime.runIfWaitingForDebugger`.
    fn auto_attach_new(&mut self, target_id: &str, paused: bool, out: &mut dyn Sink) -> CdpResult<()> {
        if self.auto_attach {
            self.attach_paused(target_id, None, true, paused, out)?;
        } else if self.tab_attach {
            self.attach_tab(target_id, out)?;
        }
        Ok(())
    }

    fn target_command(&mut self, action: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let id = cmd.id;
        match action {
            "attachToBrowserTarget" => {
                if self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS {
                    return Err(CdpError::server("Too many sessions"));
                }
                let session_id = self.mint("");
                self.browser_sessions.insert(session_id.clone());
                out.send(msg::result_frame(id, &serde_json::json!({ "sessionId": session_id }), None));
            }
            "setAutoAttach" => {
                let params: AutoAttachParams = cmd.params()?;
                if !params.flatten {
                    return Err(CdpError::server("Only flatten mode is supported"));
                }
                // Pages, or (pages excluded, tabs included) the pages' tab targets.
                let filter = params.filter.as_deref();
                let auto_attach = params.auto_attach && includes(filter, "page", true);
                let tab_attach = params.auto_attach && !auto_attach && includes(filter, "tab", false);
                let unattached: Vec<String> = if auto_attach || tab_attach {
                    self.targets
                        .iter()
                        .filter(|t| {
                            !self
                                .sessions
                                .values()
                                .any(|s| s.target_id == t.id && s.parent.is_none() && s.tab == tab_attach)
                        })
                        .map(|t| t.id.clone())
                        .collect()
                } else {
                    Vec::new()
                };
                // Reserve capacity for the entire configuration before changing
                // state or emitting success/events. A failed enable is atomic.
                if self.sessions.len() + self.browser_sessions.len() + unattached.len() > MAX_SESSIONS {
                    return Err(CdpError::server("Too many sessions"));
                }
                self.auto_attach = auto_attach;
                self.tab_attach = tab_attach;
                out.send(msg::result_frame(id, &Empty {}, cmd.session_id()));
                if self.auto_attach || self.tab_attach {
                    for target_id in unattached {
                        self.auto_attach_new(&target_id, false, out)?;
                    }
                } else {
                    // Tab sessions first detach their pages' sessions.
                    let mut sessions: Vec<(bool, String)> = self
                        .sessions
                        .iter()
                        .filter(|(_, s)| s.auto_attached && s.parent.is_none())
                        .map(|(id, s)| (s.tab, id.clone()))
                        .collect();
                    sessions.sort();
                    let sessions: Vec<String> = sessions.into_iter().map(|(_, id)| id).collect();
                    for session_id in sessions {
                        self.detach_session(&session_id, out);
                    }
                }
            }
            "setDiscoverTargets" => {
                #[derive(Deserialize)]
                struct P {
                    discover: bool,
                    filter: Option<Vec<TargetFilter>>,
                }
                let p = cmd.params::<P>()?;
                self.discover = p.discover && includes(p.filter.as_deref(), "page", true);
                self.discover_tabs = p.discover && includes(p.filter.as_deref(), "tab", false);
                out.send(msg::result_frame(id, &Empty {}, cmd.session_id()));
                if self.discover || self.discover_tabs {
                    for i in 0..self.targets.len() {
                        self.send_target_created(i, out);
                    }
                }
            }
            "getTargetInfo" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    target_id: Option<String>,
                }
                let info = match cmd.params::<P>()?.target_id {
                    None => serde_json::json!({
                        "targetId": "browser", "type": "browser", "title": "", "url": "",
                        "attached": true, "canAccessOpener": false,
                    }),
                    Some(target_id) => {
                        let target = self.target(&target_id)?;
                        let url = self.host.page_url(target.page);
                        let title = self.host.page_title(target.page);
                        let attached = self.sessions.values().any(|s| !s.tab && s.target_id == target.id);
                        serde_json::to_value(self.info(target, &url, &title, attached)).expect("serializable")
                    }
                };
                out.send(msg::result_frame(id, &serde_json::json!({ "targetInfo": info }), cmd.session_id()));
            }
            "getTargets" => {
                #[derive(Deserialize)]
                struct P {
                    filter: Option<Vec<TargetFilter>>,
                }
                // Only page targets exist (the browser target is never listed,
                // as with Chrome's default filter).
                let pages = includes(cmd.params::<P>()?.filter.as_deref(), "page", true);
                let infos: Vec<serde_json::Value> = self
                    .targets
                    .iter()
                    .filter(|_| pages)
                    .map(|t| {
                        let url = self.host.page_url(t.page);
                        let title = self.host.page_title(t.page);
                        let attached = self.sessions.values().any(|s| !s.tab && s.target_id == t.id);
                        serde_json::to_value(self.info(t, &url, &title, attached)).expect("serializable")
                    })
                    .collect();
                out.send(msg::result_frame(id, &serde_json::json!({ "targetInfos": infos }), cmd.session_id()));
            }
            "getBrowserContexts" => {
                let ids: Vec<&str> =
                    self.contexts.iter().filter(|c| !c.is_default && !c.hidden).map(|c| c.id.as_str()).collect();
                out.send(msg::result_frame(id, &serde_json::json!({ "browserContextIds": ids }), cmd.session_id()));
            }
            "createBrowserContext" => {
                // disposeOnDetach is implied: every context of a connection
                // is disposed when it closes (see `shutdown`).
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    proxy_server: Option<String>,
                    proxy_bypass_list: Option<String>,
                }
                let params: P = cmd.params()?;
                let context_id = match &params.proxy_server {
                    Some(server) => {
                        let proxy = ContextProxy { server, bypass_list: params.proxy_bypass_list.as_deref() };
                        self.new_context_from(|host| host.create_proxied_context(proxy), false, false)?
                    }
                    None => self.new_context(false)?,
                };
                out.send(msg::result_frame(
                    id,
                    &serde_json::json!({ "browserContextId": context_id }),
                    cmd.session_id(),
                ));
            }
            "disposeBrowserContext" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    browser_context_id: String,
                }
                let context_id = cmd.params::<P>()?.browser_context_id;
                if self.context(&context_id)?.is_default {
                    return Err(CdpError::server("Cannot dispose the default browser context"));
                }
                self.dispose_context(&context_id, out);
                out.send(msg::result_frame(id, &Empty {}, cmd.session_id()));
            }
            "createTarget" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    #[serde(default)]
                    url: String,
                    browser_context_id: Option<String>,
                }
                let params: P = cmd.params()?;
                let context_id = match params.browser_context_id {
                    Some(context_id) => self.context(&context_id)?.id.clone(),
                    None => self.default_context()?,
                };
                let context_key = self.context(&context_id)?.key;
                if self.targets.len() >= MAX_TARGETS {
                    return Err(CdpError::server("Too many targets"));
                }
                // Preflight every budget the creation needs, so no failure can
                // happen after the page exists (attach is the only fallible step
                // left besides navigation, which rolls back below).
                if (self.auto_attach || self.tab_attach)
                    && self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS
                {
                    return Err(CdpError::server("Too many sessions"));
                }
                let (page, target_id) = self.host.create_page(context_key)?;
                self.targets.push(TargetEntry {
                    id: target_id.clone(),
                    context_id,
                    page,
                    opener_id: None,
                    can_access_opener: false,
                    pending_url: None,
                    prepared_profile: None,
                });
                if self.discover || self.discover_tabs {
                    self.send_target_created(self.targets.len() - 1, out);
                }
                // attachedToTarget must precede the result: Playwright looks the
                // page up by targetId as soon as createTarget returns.
                self.auto_attach_new(&target_id, false, out)?;
                if !params.url.is_empty() && params.url != "about:blank" {
                    if let Err(error) = self.host.navigate(page, &params.url) {
                        // No half-created target survives a failed createTarget.
                        self.close_target(&target_id, out);
                        return Err(error);
                    }
                }
                out.send(msg::result_frame(id, &serde_json::json!({ "targetId": target_id }), cmd.session_id()));
            }
            "closeTarget" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    target_id: String,
                }
                let target_id = cmd.params::<P>()?.target_id;
                self.target(&target_id)?;
                self.close_target(&target_id, out);
                out.send(msg::result_frame(id, &serde_json::json!({ "success": true }), cmd.session_id()));
            }
            "attachToTarget" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    target_id: String,
                    #[serde(default)]
                    flatten: bool,
                }
                let params: P = cmd.params()?;
                if !params.flatten {
                    return Err(CdpError::server("Only flatten mode is supported"));
                }
                self.target(&params.target_id)?;
                let session_id = self.attach(&params.target_id, cmd.session_id(), false, out)?;
                out.send(msg::result_frame(id, &serde_json::json!({ "sessionId": session_id }), cmd.session_id()));
            }
            "detachFromTarget" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    session_id: Option<String>,
                    target_id: Option<String>,
                }
                let params: P = cmd.params()?;
                let session_id = match (params.session_id, params.target_id) {
                    (Some(s), _) => s,
                    (None, Some(t)) => self
                        .sessions
                        .iter()
                        .find(|(_, s)| {
                            s.target_id == t && (cmd.session_id().is_none() || s.parent.as_deref() == cmd.session_id())
                        })
                        .map(|(k, _)| k.clone())
                        .ok_or_else(|| CdpError::invalid_params("No session for the given target"))?,
                    (None, None) => {
                        return Err(CdpError::invalid_params("Either sessionId or targetId must be specified"))
                    }
                };
                if cmd.session_id().is_some()
                    && self.sessions.get(&session_id).is_none_or(|s| s.parent.as_deref() != cmd.session_id())
                {
                    return Err(CdpError::new(-32001, "Session is not attached to this browser session."));
                }
                if !self.sessions.contains_key(&session_id) && !self.browser_sessions.contains(&session_id) {
                    return Err(CdpError::new(-32001, "Session with given id not found."));
                }
                self.detach_session(&session_id, out);
                out.send(msg::result_frame(id, &Empty {}, cmd.session_id()));
            }
            "activateTarget" => {
                // Pages are never hidden: activation only needs a valid target.
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    target_id: String,
                }
                self.target(&cmd.params::<P>()?.target_id)?;
                out.send(msg::result_frame(id, &Empty {}, cmd.session_id()));
            }
            _ => return Err(CdpError::method_not_found(&cmd.method)),
        }
        Ok(())
    }

    /// `Target.createTarget` without a context uses the default context,
    /// created on first use and distinct from every created context.
    fn default_context(&mut self) -> CdpResult<String> {
        if let Some(c) = self.contexts.iter().find(|c| c.is_default) {
            return Ok(c.id.clone());
        }
        self.new_context(true)
    }

    fn new_context(&mut self, is_default: bool) -> CdpResult<String> {
        self.new_context_with(is_default, false)
    }

    fn new_context_with(&mut self, is_default: bool, hidden: bool) -> CdpResult<String> {
        self.new_context_from(|host| host.create_context(), is_default, hidden)
    }

    fn new_context_from(
        &mut self,
        create: impl FnOnce(&mut H) -> CdpResult<ContextKey>,
        is_default: bool,
        hidden: bool,
    ) -> CdpResult<String> {
        if self.contexts.len() >= MAX_CONTEXTS {
            return Err(CdpError::server("Too many browser contexts"));
        }
        let key = create(&mut self.host)?;
        let id = self.mint("");
        self.host.context_named(key, &id);
        self.contexts.push(ContextEntry { id: id.clone(), key, is_default, hidden });
        Ok(id)
    }

    fn attach(
        &mut self,
        target_id: &str,
        parent: Option<&str>,
        auto_attached: bool,
        out: &mut dyn Sink,
    ) -> CdpResult<String> {
        self.attach_paused(target_id, parent, auto_attached, false, out)
    }

    fn attach_paused(
        &mut self,
        target_id: &str,
        parent: Option<&str>,
        auto_attached: bool,
        waiting_for_debugger: bool,
        out: &mut dyn Sink,
    ) -> CdpResult<String> {
        if self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS {
            return Err(CdpError::server("Too many sessions"));
        }
        let session_id = self.mint("");
        self.sessions.insert(
            session_id.clone(),
            SessionEntry {
                target_id: target_id.to_owned(),
                parent: parent.map(str::to_owned),
                auto_attached,
                tab: false,
                worker: None,
                auto_attach_workers: false,
            },
        );
        let target = self.target(target_id)?;
        let page = target.page;
        let url = self.host.page_url(page);
        let title = self.host.page_title(page);
        let target = self.target(target_id)?;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Attached<'a> {
            session_id: &'a str,
            target_info: TargetInfo<'a>,
            waiting_for_debugger: bool,
        }
        let event = Attached {
            session_id: &session_id,
            target_info: self.info(target, &url, &title, true),
            waiting_for_debugger,
        };
        out.send(msg::event_frame("Target.attachedToTarget", &event, parent));
        self.host.session_attached(page, &session_id);
        Ok(session_id)
    }

    fn detach_session(&mut self, session_id: &str, out: &mut dyn Sink) {
        if self.browser_sessions.remove(session_id) {
            let children: Vec<_> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.parent.as_deref() == Some(session_id))
                .map(|(id, _)| id.clone())
                .collect();
            for child in children {
                self.detach_session(&child, out);
            }
            out.send(msg::event_frame("Target.detachedFromTarget", &serde_json::json!({"sessionId":session_id}), None));
            return;
        }
        let Some(session) = self.sessions.get(session_id) else { return };
        if session.tab {
            let children: Vec<String> = self
                .sessions
                .iter()
                .filter(|(_, s)| s.parent.as_deref() == Some(session_id))
                .map(|(id, _)| id.clone())
                .collect();
            for child in children {
                self.detach_session(&child, out);
            }
        } else {
            self.detach_worker_sessions(session_id, out);
        }
        let Some(session) = self.sessions.remove(session_id) else { return };
        if let Some(worker) = session.worker {
            self.host.worker_session_detached(worker, session_id);
        }
        if session.tab {
            out.send(msg::event_frame(
                "Target.detachedFromTarget",
                &serde_json::json!({ "sessionId": session_id, "targetId": tab_id(&session.target_id) }),
                None,
            ));
            return;
        }
        if let Some(target) = self.targets.iter().find(|t| t.id == session.target_id) {
            self.host.session_detached(target.page, session_id);
        }
        out.send(msg::event_frame(
            "Target.detachedFromTarget",
            &serde_json::json!({ "sessionId": session_id, "targetId": session.target_id }),
            session.parent.as_deref(),
        ));
    }

    fn close_target(&mut self, target_id: &str, out: &mut dyn Sink) {
        // Page sessions before tab sessions (a page session's events go to its tab's session).
        let mut sessions: Vec<(bool, String)> =
            self.sessions.iter().filter(|(_, s)| s.target_id == target_id).map(|(k, s)| (s.tab, k.clone())).collect();
        sessions.sort();
        for (tab, session_id) in &sessions {
            if !*tab {
                self.detach_worker_sessions(session_id, out);
            }
            if *tab {
                if self.sessions.remove(session_id).is_some() {
                    out.send(msg::event_frame(
                        "Target.detachedFromTarget",
                        &serde_json::json!({ "sessionId": session_id, "targetId": tab_id(target_id) }),
                        None,
                    ));
                }
                continue;
            }
            if let Some(session) = self.sessions.remove(session_id) {
                if let Some(target) = self.targets.iter().find(|t| t.id == session.target_id) {
                    self.host.session_detached(target.page, session_id);
                }
                out.send(msg::event_frame(
                    "Target.detachedFromTarget",
                    &serde_json::json!({ "sessionId": session_id, "targetId": target_id }),
                    session.parent.as_deref(),
                ));
            }
        }
        if let Some(pos) = self.targets.iter().position(|t| t.id == target_id) {
            let target = self.targets.remove(pos);
            if target.prepared_profile.is_some() {
                self.refill_blocked = false;
            }
            self.host.close_page(target.page);
            if self.discover {
                out.send(msg::event_frame(
                    "Target.targetDestroyed",
                    &serde_json::json!({ "targetId": target_id }),
                    None,
                ));
            }
            if self.discover_tabs {
                out.send(msg::event_frame(
                    "Target.targetDestroyed",
                    &serde_json::json!({ "targetId": tab_id(target_id) }),
                    None,
                ));
            }
        }
    }

    fn dispose_context(&mut self, context_id: &str, out: &mut dyn Sink) {
        let targets: Vec<String> =
            self.targets.iter().filter(|t| t.context_id == context_id).map(|t| t.id.clone()).collect();
        for target_id in targets {
            self.close_target(&target_id, out);
        }
        if let Some(pos) = self.contexts.iter().position(|c| c.id == context_id) {
            let context = self.contexts.remove(pos);
            self.host.dispose_context(context.key);
        }
    }

    /// Opens the popups the engine requested: a new target in the opener's
    /// browser context, announced with `openerId`. With auto-attach the popup
    /// starts paused (`waitingForDebugger`) and navigates on
    /// `Runtime.runIfWaitingForDebugger`, as Chrome does; otherwise it
    /// navigates immediately. Requests from closed openers are dropped.
    pub fn adopt_popups(&mut self, out: &mut dyn Sink) {
        for change in self.host.take_worker_changes() {
            self.worker_changed(change, out);
        }
        for page in self.host.take_close_requests() {
            if let Some(id) = self.targets.iter().find(|t| t.page == page).map(|t| t.id.clone()) {
                self.close_target(&id, out);
            }
        }
        for request in self.host.take_popup_requests() {
            let Some(opener) = self.targets.iter().find(|t| t.page == request.opener) else { continue };
            let (opener_id, context_id) = (opener.id.clone(), opener.context_id.clone());
            let Ok(context_key) = self.context(&context_id).map(|c| c.key) else { continue };
            if self.targets.len() >= MAX_TARGETS
                || ((self.auto_attach || self.tab_attach)
                    && self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS)
            {
                continue;
            }
            let Ok((page, target_id)) = self.host.create_page(context_key) else { continue };
            let paused = self.auto_attach || self.tab_attach;
            self.targets.push(TargetEntry {
                id: target_id.clone(),
                context_id,
                page,
                opener_id: Some(opener_id),
                can_access_opener: request.opener_access,
                pending_url: paused.then(|| request.url.clone()),
                prepared_profile: None,
            });
            if self.discover || self.discover_tabs {
                self.send_target_created(self.targets.len() - 1, out);
            }
            self.host.popup_opened(request.opener, page, &target_id, request.handle);
            if paused {
                self.host.set_initial_popup(page, true);
                if self.auto_attach_new(&target_id, true, out).is_err() {
                    self.close_target(&target_id, out);
                }
            } else if self.host.navigate(page, &request.url).is_err() {
                self.close_target(&target_id, out);
            }
        }
        for action in self.host.take_popup_actions() {
            match action {
                PopupAction::Close(page) => {
                    if let Some(id) = self.targets.iter().find(|t| t.page == page).map(|t| t.id.clone()) {
                        self.close_target(&id, out);
                    }
                }
                PopupAction::Navigate(page, url) => {
                    let Some(target) = self.targets.iter_mut().find(|t| t.page == page) else { continue };
                    // A paused popup has not started: this replaces what it will load.
                    match target.pending_url.as_mut() {
                        Some(pending) => *pending = url,
                        None => {
                            let _ = self.host.navigate(page, &url);
                        }
                    }
                }
            }
        }
    }

    fn worker_changed(&mut self, change: WorkerChange, out: &mut dyn Sink) {
        match change {
            WorkerChange::Started { worker, page, url } => {
                let target_id = self.mint("");
                self.workers.push(WorkerEntry { key: worker, page, target_id, url });
                let index = self.workers.len() - 1;
                if self.discover {
                    let params = serde_json::json!({ "targetInfo": self.worker_info(index, false) });
                    out.send(msg::event_frame("Target.targetCreated", &params, None));
                }
                let Some(target_id) = self.targets.iter().find(|t| t.page == page).map(|t| t.id.clone()) else {
                    return;
                };
                let parents: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| s.target_id == target_id && s.auto_attach_workers)
                    .map(|(id, _)| id.clone())
                    .collect();
                for parent in parents {
                    self.attach_worker(index, &parent, out);
                }
            }
            WorkerChange::Stopped { worker } => {
                let Some(pos) = self.workers.iter().position(|w| w.key == worker) else { return };
                let entry = self.workers.remove(pos);
                let sessions: Vec<String> =
                    self.sessions.iter().filter(|(_, s)| s.worker == Some(worker)).map(|(id, _)| id.clone()).collect();
                for session in sessions {
                    self.detach_session(&session, out);
                }
                if self.discover {
                    let params = serde_json::json!({ "targetId": entry.target_id });
                    out.send(msg::event_frame("Target.targetDestroyed", &params, None));
                }
            }
        }
    }

    fn worker_info(&self, index: usize, attached: bool) -> serde_json::Value {
        let worker = &self.workers[index];
        let context = self.targets.iter().find(|t| t.page == worker.page).map(|t| t.context_id.as_str());
        serde_json::json!({
            "targetId": worker.target_id, "type": "worker", "title": "", "url": worker.url,
            "attached": attached, "canAccessOpener": false, "browserContextId": context,
        })
    }

    /// Attaches `parent`, a session of the worker's page, once. Past the session
    /// limit the worker simply has no session there.
    fn attach_worker(&mut self, index: usize, parent: &str, out: &mut dyn Sink) {
        let worker = self.workers[index].key;
        if self.sessions.values().any(|s| s.worker == Some(worker) && s.parent.as_deref() == Some(parent))
            || self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS
        {
            return;
        }
        let session_id = self.mint("");
        self.sessions.insert(
            session_id.clone(),
            SessionEntry {
                target_id: self.workers[index].target_id.clone(),
                parent: Some(parent.to_owned()),
                auto_attached: true,
                tab: false,
                worker: Some(worker),
                auto_attach_workers: false,
            },
        );
        // A worker never waits for the debugger: its console calls and exceptions
        // are replayed by `Runtime.enable` instead.
        let params = serde_json::json!({
            "sessionId": session_id, "targetInfo": self.worker_info(index, true), "waitingForDebugger": false,
        });
        out.send(msg::event_frame("Target.attachedToTarget", &params, Some(parent)));
    }

    fn detach_worker_sessions(&mut self, parent: &str, out: &mut dyn Sink) {
        let children: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.worker.is_some() && s.parent.as_deref() == Some(parent))
            .map(|(id, _)| id.clone())
            .collect();
        for child in children {
            self.detach_session(&child, out);
        }
    }

    fn worker_session_command(
        &mut self,
        session_id: &str,
        worker: WorkerKey,
        cmd: &Command<'_>,
        out: &mut dyn Sink,
    ) -> CdpResult<()> {
        match cmd.method.as_ref() {
            // No targets below a worker, and it does not wait for the debugger.
            "Target.setAutoAttach" | "Runtime.runIfWaitingForDebugger" => {
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
            }
            "Target.getTargetInfo" => {
                let index = self
                    .workers
                    .iter()
                    .position(|w| w.key == worker)
                    .ok_or_else(|| CdpError::server("The worker is gone"))?;
                let info = self.worker_info(index, true);
                out.send(msg::result_frame(cmd.id, &serde_json::json!({ "targetInfo": info }), Some(session_id)));
            }
            _ => self.host.worker_command(worker, session_id, cmd, out),
        }
        Ok(())
    }

    /// Starts a paused popup's navigation (once).
    fn release_popup(&mut self, target_id: &str, out: &mut dyn Sink) {
        let Some(target) = self.targets.iter_mut().find(|t| t.id == target_id) else { return };
        let Some(url) = target.pending_url.take() else { return };
        let page = target.page;
        self.host.set_initial_popup(page, false);
        // about:blank too: clients wait for the commit that replaces the ":" initial document.
        if self.host.navigate(page, &url).is_err() {
            self.close_target(target_id, out);
        }
    }

    fn send_target_created(&self, index: usize, out: &mut dyn Sink) {
        let target = &self.targets[index];
        let url = self.host.page_url(target.page);
        let title = self.host.page_title(target.page);
        let attached = self.sessions.values().any(|s| !s.tab && s.target_id == target.id);
        if self.discover_tabs {
            let tab_id = tab_id(&target.id);
            out.send(msg::event_frame(
                "Target.targetCreated",
                &serde_json::json!({ "targetInfo": self.tab_info(target, &tab_id, &url, &title) }),
                None,
            ));
        }
        if self.discover {
            out.send(msg::event_frame(
                "Target.targetCreated",
                &serde_json::json!({ "targetInfo": self.info(target, &url, &title, attached) }),
                None,
            ));
        }
    }

    /// Tears down everything when the client disconnects. This is also when
    /// `disposeOnDetach` contexts are disposed: the flag refers to the
    /// connection, not to page sessions.
    pub fn shutdown(&mut self) {
        self.browser_sessions.clear();
        self.prepared_profiles.clear();
        for entry in std::mem::take(&mut self.prepared) {
            self.host.close_page(entry.page);
        }
        let ids: Vec<String> = self.contexts.iter().map(|c| c.id.clone()).collect();
        for id in ids {
            self.dispose_context(&id, &mut Discard);
        }
    }
}

/// A sink for internal operations whose events no client should see.
struct Discard;
impl Sink for Discard {
    fn send(&mut self, _frame: OutFrame) {}
}

/// CDP `Target.TargetFilter` entry: the first entry whose `type` matches (or
/// that has no `type`) decides; no match excludes.
#[derive(Deserialize)]
struct TargetFilter {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    exclude: bool,
}

/// Whether targets of type `kind` pass `filter`; `default` when there is no
/// filter (Chrome's default filter includes pages and excludes tabs).
fn includes(filter: Option<&[TargetFilter]>, kind: &str, default: bool) -> bool {
    let Some(filter) = filter else { return default };
    filter.iter().find(|f| f.kind.as_deref().is_none_or(|k| k == kind)).is_some_and(|f| !f.exclude)
}

/// The id of a page target's tab target.
fn tab_id(page_target_id: &str) -> String {
    format!("tab-{page_target_id}")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct AutoAttachParams {
    auto_attach: bool,
    #[serde(default)]
    wait_for_debugger_on_start: bool,
    #[serde(default)]
    flatten: bool,
    filter: Option<Vec<TargetFilter>>,
}
