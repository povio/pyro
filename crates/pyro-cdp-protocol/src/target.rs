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
    /// `service`: a service worker version, a `service_worker` target of the
    /// page's browser context (attached by the browser-level auto-attach)
    /// rather than a `worker` below the page.
    Started {
        worker: WorkerKey,
        page: PageKey,
        url: String,
        service: bool,
    },
    Stopped {
        worker: WorkerKey,
    },
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

/// `Target.createBrowserContext` `proxyServer` and `proxyBypassList`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextProxy<'a> {
    pub server: &'a str,
    pub bypass_list: Option<&'a str>,
}

/// What a browser context has of its own instead of the browser's:
/// `Target.createBrowserContext {proxyServer, proxyBypassList}`, and a
/// prepared profile's `userAgent` and proxy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextOptions<'a> {
    pub proxy: Option<ContextProxy<'a>>,
    /// `User-Agent` of its requests and its pages' `navigator.userAgent`.
    pub user_agent: Option<&'a str>,
}

impl<'a> ContextOptions<'a> {
    /// The context of a prepared profile's page.
    pub fn of(profile: &'a PreparedProfile) -> Self {
        ContextOptions {
            proxy: profile
                .proxy_server
                .as_deref()
                .map(|server| ContextProxy { server, bypass_list: profile.proxy_bypass_list.as_deref() }),
            user_agent: profile.user_agent.as_deref(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.proxy.is_none() && self.user_agent.is_none()
    }
}

/// The engine side, implemented on the browser thread.
///
/// Output contract: everything for one connection goes through that
/// connection's single ordered channel. The `out` sinks passed to
/// [`Host::page_command`] and [`Host::browser_command`] are that channel; a
/// host that answers later (awaited promises, navigation commits, events)
/// must hold its own clone of the same channel ([`crate::outbox::Outbox`]),
/// never a different destination, so that ordering is preserved.
pub trait Host {
    /// Creates an isolated browser context (cookies, storage, cache).
    fn create_context(&mut self) -> CdpResult<ContextKey>;
    /// `create_context` with what `options` gives it of its own.
    fn create_context_with(&mut self, options: ContextOptions<'_>) -> CdpResult<ContextKey> {
        match options {
            o if o.is_empty() => self.create_context(),
            ContextOptions { proxy: Some(_), .. } => {
                Err(CdpError::unsupported("Target.createBrowserContext proxyServer"))
            }
            _ => Err(CdpError::unsupported("a browser context's own userAgent")),
        }
    }
    /// Blocks service worker registration in context `key`
    /// (`Target.createBrowserContext` `serviceWorkers: "block"`, pyro's own
    /// parameter for Playwright's option).
    fn block_service_workers(&mut self, _key: ContextKey) -> CdpResult<()> {
        Err(CdpError::unsupported("Target.createBrowserContext serviceWorkers"))
    }
    fn dispose_context(&mut self, context: ContextKey);
    /// Creates an `about:blank` page. Returns its key and target id; the target
    /// id is the main frame id, as in Chrome and upstream.
    fn create_page(&mut self, context: ContextKey) -> CdpResult<(PageKey, String)>;
    /// Creates the page of a popup the engine requested (`handle` is the
    /// request's): the page the opener's `window.open` made ahead, with the
    /// initial about:blank document its script may have changed already, when
    /// there is one. The flag says whether it was made ahead.
    fn create_popup_page(
        &mut self,
        context: ContextKey,
        _handle: Option<&std::rc::Rc<dyn std::any::Any>>,
    ) -> CdpResult<(PageKey, String, bool)> {
        self.create_page(context).map(|(page, id)| (page, id, false))
    }
    /// Starts a navigation (used by `Target.createTarget` with a URL).
    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()>;
    /// Reports again the commit of `page`'s initial about:blank document
    /// (a popup that keeps the one `window.open` made). False when the page
    /// shows another document: the caller navigates instead.
    fn announce_initial_document(&mut self, _page: PageKey) -> bool {
        false
    }
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
    /// A popup the engine requested is now a target (`Pyro.run` `popup` steps).
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
    /// Sets the page's locale (`Emulation.setLocaleOverride`).
    fn set_locale(&mut self, _page: PageKey, _locale: &str) {}
    /// Sets the page's time zone (`Emulation.setTimezoneOverride`).
    fn set_timezone(&mut self, _page: PageKey, _timezone_id: &str) -> CdpResult<()> {
        Ok(())
    }
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
/// `Pyro.registerPreparedPage`): its own browser context (seeded
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
    /// `Pyro.run` steps run once on the booted page before tests get it
    /// (`serve --fork` only: the template runs them, and every connection
    /// forked afterwards starts from the resulting state).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<serde_json::Value>,
    /// Starts from another profile's page (its id) instead of booting
    /// `url`: `serve --fork` forks that profile's snapshot and runs only this
    /// profile's `setup` on it. Pages of such a profile are never booted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<usize>,
    /// `User-Agent` of the profile's requests (its document, subresources,
    /// fetch/XHR and setup steps, then every connection's) and its pages'
    /// `navigator.userAgent`, instead of the browser's. A profile with a
    /// `base` has its base's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// HTTP proxy of the profile's requests, as `Target.createBrowserContext
    /// proxyServer` takes it (`http://[user:pass@]host:port`; `direct://`: no
    /// proxy), instead of the browser's (`serve --proxy`). A profile with a
    /// `base` has its base's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_server: Option<String>,
    /// Hosts that skip `proxy_server` (Chrome's `--proxy-bypass-list` syntax).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_bypass_list: Option<String>,
    /// Locale of the profile's pages, as `Emulation.setLocaleOverride` sets
    /// it: `navigator.language(s)` and `Accept-Language` (clients add the
    /// init script that makes it Intl's default). A profile with a `base`
    /// has its base's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Time zone of the profile's pages (`Emulation.setTimezoneOverride`).
    /// It is the JavaScript VM's, which every page of the process shares (in
    /// fork mode the template's, and so every fork's): profiles with
    /// different zones are refused on one server. A profile with a `base`
    /// has its base's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone_id: Option<String>,
}

impl PreparedProfile {
    /// Checks what the profile's context gets of its own (`userAgent`,
    /// `proxyServer`, `proxyBypassList`).
    pub fn check_context_options(&self) -> CdpResult<()> {
        if let Some(agent) = &self.user_agent {
            if agent.is_empty() || http::HeaderValue::from_str(agent).is_err() {
                return Err(CdpError::invalid_params("userAgent must be a non-empty header value"));
            }
        }
        match (&self.proxy_server, &self.proxy_bypass_list) {
            (Some(server), bypass) => {
                pyro_net::ProxyConfig::parse(server, bypass.as_deref())
                    .map_err(|e| CdpError::invalid_params(format!("proxyServer: {e}")))?;
            }
            (None, Some(_)) => return Err(CdpError::invalid_params("proxyBypassList needs proxyServer")),
            (None, None) => {}
        }
        if let Some(locale) = &self.locale {
            if !valid_locale(locale) {
                return Err(CdpError::invalid_params("locale must be a BCP 47 language tag such as en-GB"));
            }
        }
        if self.timezone_id.as_deref() == Some("") {
            return Err(CdpError::invalid_params("timezoneId must not be empty"));
        }
        Ok(())
    }

    /// Why this profile cannot join `others` (the active profiles of the
    /// same VM): another one has a different `timezoneId`.
    pub fn timezone_conflict<'a>(&self, others: impl IntoIterator<Item = &'a PreparedProfile>) -> Option<String> {
        let mine = self.timezone_id.as_deref()?;
        let other = others.into_iter().find_map(|p| p.timezone_id.as_deref().filter(|z| *z != mine))?;
        Some(format!(
            "timezoneId {mine}: another profile of this server has {other}, and the time zone is the \
             server's JavaScript VM's (one zone per server; start another `pyro serve` for another zone)"
        ))
    }
}

/// What `Emulation.setLocaleOverride` accepts: letters, digits, `-` and `_`.
pub fn valid_locale(locale: &str) -> bool {
    !locale.is_empty()
        && http::HeaderValue::from_str(locale).is_ok()
        && locale.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// How far a prepared profile is (`serve --fork`, where its page boots and
/// its setup runs once, in the template).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ProfileStatus {
    /// Its page is loading, or its setup has not finished.
    Booting,
    /// Loaded and set up: connections start from it.
    Ready,
    /// Its setup failed, or the profile it starts from failed: `error` says
    /// which step and why (`setup step 2 (Click …): timed out after …`).
    /// No page of it is handed out.
    Failed { error: String },
}

impl ProfileStatus {
    /// The status's `state` on the wire.
    pub fn state(&self) -> &'static str {
        match self {
            ProfileStatus::Booting => "booting",
            ProfileStatus::Ready => "ready",
            ProfileStatus::Failed { .. } => "failed",
        }
    }

    /// Whether it is final (set up, or failed).
    pub fn is_final(&self) -> bool {
        !matches!(self, ProfileStatus::Booting)
    }

    /// Adds `state` (and `error`) to a reply object.
    pub fn add_to(&self, reply: &mut serde_json::Value) {
        reply["state"] = self.state().into();
        if let ProfileStatus::Failed { error } = self {
            reply["error"] = error.as_str().into();
        }
    }
}

/// The error for a profile whose setup failed: the message names it
/// (`label`) and carries the setup's error; `data.setupError` holds that
/// error alone, for clients that name the profile themselves.
pub fn setup_failed_error(label: &str, error: &str) -> CdpError {
    CdpError::server(format!("profile {label}: {error}")).with_data(Some(serde_json::json!({ "setupError": error })))
}

/// The `name` of a shared profile (`serve --fork`) in `Pyro.*` params.
#[derive(Deserialize)]
struct ProfileName {
    name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportSize {
    pub width: u32,
    pub height: u32,
}

fn default_pool() -> usize {
    1
}

/// `serve --fork`: the template process's profile registry, as a connection
/// reaches it. Profiles live in the template, so every connection (every
/// client of the server) shares them. A named profile is held by the
/// connections that registered or adopted it and retired once none holds it
/// and no other profile derives from it; a connection's holds end with it.
pub trait ForkProfiles {
    /// Boots `profile` for every later connection, or finds the same one:
    /// its id and how far it is. With `name`, this connection also holds it
    /// under that name. A profile whose setup failed is not found: the same
    /// definition boots again (under a name, in its place).
    fn register(&self, profile: &PreparedProfile, name: Option<&str>) -> Result<(usize, ProfileStatus), String>;
    /// Holds the profile some connection registered as `name`: its id,
    /// definition and how far it is. One whose setup failed is not held.
    fn adopt(&self, name: &str) -> Result<(usize, PreparedProfile, ProfileStatus), String>;
    /// Drops this connection's hold on `name`.
    fn release(&self, name: &str) -> Result<ProfileRelease, String>;
    /// Retires profile `id` (one registered without a name); whether it was.
    fn unregister(&self, id: usize) -> Result<bool, String>;
    /// The named profiles.
    fn list(&self) -> Result<Vec<NamedProfile>, String>;
    /// How far profile `id` is: booting, set up, or failed and why.
    fn status(&self, id: usize) -> Result<ProfileStatus, String>;
}

/// What releasing a named profile did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRelease {
    /// This connection held the name.
    pub released: bool,
    /// The profile was retired with it (no other holder, nothing derived from it).
    pub retired: bool,
}

/// A profile the template keeps under a name (`Pyro.preparedProfiles`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedProfile {
    pub name: String,
    pub profile_id: usize,
    /// Connections holding it.
    pub holders: usize,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<usize>,
    /// How far it is (`state`, and `error` when its setup failed).
    #[serde(flatten)]
    pub status: ProfileStatus,
}

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
    /// The popup's initial about:blank document, made by `window.open`, stays
    /// (no URL to load): releasing the popup announces its commit instead of
    /// replacing it.
    keep_initial: bool,
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
    service: bool,
    /// The browser context, kept: a service worker outlives the page.
    context_id: Option<String>,
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
    /// The last handled message polled the prepared-page pool.
    pool_poll: bool,
    next_id: u64,
    prepared_profiles: Vec<PreparedProfile>,
    retired_profiles: HashSet<usize>,
    /// `serve --fork`: profiles whose setup failed (or whose base's did),
    /// with why. Their pages are closed; acquiring one fails with the error.
    failed_profiles: HashMap<usize, String>,
    prepared: Vec<PreparedEntry>,
    /// The last refill could create nothing (limits, failures): wait for
    /// the next acquisition before trying again.
    refill_blocked: bool,
    /// `serve --fork`: this connection is a fork of the template process
    /// (see `pyro_cdp::fork`), which boots prepared pages for every later
    /// connection. Nothing is booted here; registrations go to the template.
    fork: Option<Box<dyn ForkProfiles>>,
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
            pool_poll: false,
            next_id: 1,
            prepared_profiles: Vec::new(),
            retired_profiles: HashSet::new(),
            failed_profiles: HashMap::new(),
            prepared: Vec::new(),
            refill_blocked: false,
            fork: None,
        }
    }

    /// Makes this connection a fork child (see the `fork` field): no
    /// refills, and profiles are registered with (and shared through) the
    /// template's `registry`.
    pub fn set_fork_child(&mut self, registry: Box<dyn ForkProfiles>) {
        self.fork = Some(registry);
    }

    /// A connection explicitly forked for one profile needs only that
    /// snapshot. Close inherited sibling documents and their contexts so
    /// their timers and requests cannot run alongside the acquired page.
    /// Profile registrations stay intact; the root still owns their leases.
    pub fn retain_prepared_snapshot(&mut self, profile: usize) {
        let others: HashSet<usize> = self.prepared.iter().filter(|e| e.profile != profile).map(|e| e.profile).collect();
        for id in others {
            self.close_waiting_pages(id);
        }
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
        self.failed_profiles.remove(&id);
        self.close_waiting_pages(id);
        let profile = &mut self.prepared_profiles[id];
        profile.url.clear();
        profile.init_scripts.clear();
        profile.setup.clear();
        profile.viewport = None;
        profile.base = None;
        profile.user_agent = None;
        profile.proxy_server = None;
        profile.proxy_bypass_list = None;
        profile.locale = None;
        profile.timezone_id = None;
        Ok(true)
    }

    /// Closes profile `id`'s waiting pages, with their contexts.
    fn close_waiting_pages(&mut self, id: usize) {
        let waiting = std::mem::take(&mut self.prepared);
        for entry in waiting {
            if entry.profile == id {
                self.host.close_page(entry.page);
                self.dispose_context(&entry.context_id, &mut Discard);
            } else {
                self.prepared.push(entry);
            }
        }
    }

    /// `serve --fork`: profile `id`'s setup failed (or that of the profile it
    /// starts from), for `error`. Its waiting pages are closed, half set up
    /// as they are, and acquiring a page of it fails with `error` from now
    /// on, in every connection forked afterwards too. The profile stays
    /// registered (its names and holds) until it is retired as usual.
    pub fn fail_prepared_profile(&mut self, id: usize, error: String) {
        if !self.prepared_profile_active(id) {
            return;
        }
        self.failed_profiles.entry(id).or_insert(error);
        self.close_waiting_pages(id);
    }

    /// Why profile `id`'s setup failed, if it did.
    pub fn prepared_profile_failure(&self, id: usize) -> Option<&str> {
        self.failed_profiles.get(&id).map(String::as_str)
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
        if !self.prepared_profile_active(i) || self.failed_profiles.contains_key(&i) {
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
                let options = ContextOptions::of(&profile);
                let Ok(context_id) = self.new_context_from(|host| host.create_context_with(options), false, true)
                else {
                    break;
                };
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
                if let Some(locale) = &profile.locale {
                    self.host.set_locale(page, locale);
                }
                let zoned = profile.timezone_id.as_deref().is_none_or(|z| self.host.set_timezone(page, z).is_ok());
                if !zoned || self.host.navigate(page, &profile.url).is_err() {
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

    /// `Pyro.*`: the prepared-page extension.
    fn pyro_command(&mut self, action: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let id = cmd.id;
        match action {
            "registerPreparedPage" => {
                let profile: PreparedProfile = cmd.params()?;
                let ProfileName { name } = cmd.params()?;
                if pyro_net::Url::parse(&profile.url).is_err() {
                    return Err(CdpError::invalid_params("url is not an absolute URL"));
                }
                if profile.pool == 0 || profile.pool > MAX_POOL {
                    return Err(CdpError::invalid_params("pool must be between 1 and 8"));
                }
                if (!profile.setup.is_empty() || profile.base.is_some()) && self.fork.is_none() {
                    return Err(CdpError::invalid_params("setup and base need serve --fork"));
                }
                if name.as_deref() == Some("") {
                    return Err(CdpError::invalid_params("name must not be empty"));
                }
                profile.check_context_options()?;
                if let Some(registry) = &self.fork {
                    // The template boots it; connections forked from then on
                    // have it. Its setup runs later: `state` says how far it
                    // is, `Pyro.preparedProfileStatus` how it went.
                    let (profile_id, status) =
                        registry.register(&profile, name.as_deref()).map_err(CdpError::server)?;
                    let mut reply = serde_json::json!({ "profileId": profile_id });
                    status.add_to(&mut reply);
                    out.send(msg::result_frame(id, &reply, cmd.session_id()));
                    return Ok(());
                }
                if name.is_some() {
                    return Err(CdpError::invalid_params("profile names need serve --fork"));
                }
                let active =
                    self.prepared_profiles.iter().enumerate().filter(|(i, _)| self.prepared_profile_active(*i));
                if let Some(conflict) = profile.timezone_conflict(active.map(|(_, p)| p)) {
                    return Err(CdpError::invalid_params(conflict));
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
                    profile_id: Option<usize>,
                    name: Option<String>,
                }
                let p: P = cmd.params()?;
                let result = if let Some(name) = p.name {
                    // This connection's hold on a shared profile: retired with the last one.
                    let Some(registry) = &self.fork else {
                        return Err(CdpError::invalid_params("profile names need serve --fork"));
                    };
                    let r = registry.release(&name).map_err(CdpError::server)?;
                    serde_json::json!({ "removed": r.released, "retired": r.retired })
                } else if let Some(profile_id) = p.profile_id {
                    let removed = match &self.fork {
                        Some(registry) => registry.unregister(profile_id).map_err(CdpError::server)?,
                        None => self.unregister_prepared_profile(profile_id)?,
                    };
                    serde_json::json!({ "removed": removed })
                } else {
                    return Err(CdpError::invalid_params("profileId or name is required"));
                };
                out.send(msg::result_frame(id, &result, cmd.session_id()));
            }
            "adoptPreparedPage" => {
                let Some(registry) = &self.fork else {
                    return Err(CdpError::invalid_params("adopting a profile needs serve --fork"));
                };
                let ProfileName { name } = cmd.params()?;
                let name = name.ok_or_else(|| CdpError::invalid_params("name is required"))?;
                let (profile_id, profile, status) = registry.adopt(&name).map_err(CdpError::server)?;
                // A profile whose setup failed is not held: nothing of it can be used.
                if let ProfileStatus::Failed { error } = &status {
                    return Err(setup_failed_error(&name, error));
                }
                let mut reply = serde_json::json!({
                    "profileId": profile_id,
                    "url": profile.url,
                    "initScripts": profile.init_scripts,
                    "viewport": profile.viewport,
                    "setup": profile.setup,
                    "base": profile.base,
                    "userAgent": profile.user_agent,
                    "proxyServer": profile.proxy_server,
                    "proxyBypassList": profile.proxy_bypass_list,
                    "locale": profile.locale,
                    "timezoneId": profile.timezone_id,
                });
                status.add_to(&mut reply);
                out.send(msg::result_frame(id, &reply, cmd.session_id()));
            }
            "preparedProfiles" => {
                let profiles = match &self.fork {
                    Some(registry) => registry.list().map_err(CdpError::server)?,
                    None => Vec::new(),
                };
                out.send(msg::result_frame(id, &serde_json::json!({ "profiles": profiles }), cmd.session_id()));
            }
            "preparedProfileStatus" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    profile_id: usize,
                }
                let profile_id = cmd.params::<P>()?.profile_id;
                let status = match &self.fork {
                    Some(registry) => registry.status(profile_id).map_err(CdpError::server)?,
                    // Without a template a profile has no setup: nothing to wait for.
                    None if self.prepared_profile_active(profile_id) => match self.failed_profiles.get(&profile_id) {
                        Some(error) => ProfileStatus::Failed { error: error.clone() },
                        None => ProfileStatus::Ready,
                    },
                    None => return Err(CdpError::invalid_params("Unknown prepared profile")),
                };
                out.send(msg::result_frame(id, &status, cmd.session_id()));
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
                // Not "no page waiting" (-32602), after which a client may
                // boot a fresh page: none of this profile is to be used.
                if let Some((profile, error)) = p.profile_id.and_then(|i| Some((i, self.failed_profiles.get(&i)?))) {
                    return Err(setup_failed_error(&profile.to_string(), error));
                }
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
                        "No prepared page for that url: register one with Pyro.registerPreparedPage or serve --prepare",
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
                    keep_initial: false,
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
                // `sharedProfiles`: profiles have names every client of this server can adopt.
                let fork = self.fork.is_some();
                out.send(msg::result_frame(
                    id,
                    &serde_json::json!({ "fork": fork, "sharedProfiles": fork }),
                    cmd.session_id(),
                ));
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
        self.pool_poll = command.method == "Pyro.preparedPages";
        let _t = crate::perf::span_with(|| format!("cdp.dispatch {}", command.method));
        if let Err(error) = self.dispatch(&command, out) {
            out.send(msg::error_frame(Some(command.id), &error, command.session_id()));
        }
    }

    /// Whether the last message [`Browser::handle_text`] handled was a
    /// `Pyro.preparedPages` status poll (not client activity).
    pub fn last_was_pool_poll(&self) -> bool {
        self.pool_poll
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
            ("Pyro", "perfReset") => {
                crate::perf::reset();
                out.send(msg::result_frame(cmd.id, &Empty {}, cmd.session_id()));
                Ok(())
            }
            #[cfg(feature = "perf-trace")]
            ("Pyro", "perfStats") => {
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
            ("Pyro", _) => self.pyro_command(action, cmd, out),
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
                        | "Browser.cancelDownload"
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
        let index = self
            .targets
            .iter()
            .position(|t| t.id == session.target_id)
            .ok_or_else(|| CdpError::server("target of session is gone"))?;
        let page = self.targets[index].page;
        if session.tab {
            let target_id = self.targets[index].id.clone();
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
                    let existing: Vec<usize> = (0..self.workers.len())
                        .filter(|i| self.workers[*i].page == page && !self.workers[*i].service)
                        .collect();
                    for index in existing {
                        self.attach_worker(index, Some(session_id), out);
                    }
                }
            }
            "Target.getTargetInfo" => {
                let url = self.host.page_url(page);
                let title = self.host.page_title(page);
                let target_id = self.targets[index].id.clone();
                let target = self.target(&target_id)?;
                let info = self.info(target, &url, &title, true);
                out.send(msg::result_frame(cmd.id, &serde_json::json!({ "targetInfo": info }), Some(session_id)));
            }
            "Runtime.runIfWaitingForDebugger" => {
                // Only popups start paused; resuming starts their navigation.
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
                let target_id = self.targets[index].id.clone();
                self.release_popup(&target_id, out);
            }
            // The target id is borrowed: no copy per page command.
            _ => {
                let target_id = &self.targets[index].id;
                self.host.page_command(PageCommand { page, target_id, session_id, command: cmd }, out)
            }
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
                    if self.auto_attach {
                        let services: Vec<usize> =
                            (0..self.workers.len()).filter(|i| self.workers[*i].service).collect();
                        for index in services {
                            self.attach_worker(index, None, out);
                        }
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
                // `userAgent` is Pyro's own: the context's default, as a
                // prepared profile's (pages may still override it).
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    proxy_server: Option<String>,
                    proxy_bypass_list: Option<String>,
                    user_agent: Option<String>,
                    /// pyro's own: Playwright's `serviceWorkers` option.
                    service_workers: Option<String>,
                }
                let params: P = cmd.params()?;
                if params.user_agent.as_ref().is_some_and(|a| a.is_empty() || http::HeaderValue::from_str(a).is_err()) {
                    return Err(CdpError::invalid_params("userAgent must be a non-empty header value"));
                }
                let block_service_workers = match params.service_workers.as_deref() {
                    None | Some("allow") => false,
                    Some("block") => true,
                    Some(other) => {
                        return Err(CdpError::invalid_params(format!(
                            "serviceWorkers must be \"allow\" or \"block\", not {other:?}"
                        )))
                    }
                };
                let options = ContextOptions {
                    proxy: params
                        .proxy_server
                        .as_deref()
                        .map(|server| ContextProxy { server, bypass_list: params.proxy_bypass_list.as_deref() }),
                    user_agent: params.user_agent.as_deref(),
                };
                let context_id = if options.is_empty() {
                    self.new_context(false)?
                } else {
                    self.new_context_from(|host| host.create_context_with(options), false, false)?
                };
                if block_service_workers {
                    let key = self.context(&context_id)?.key;
                    if let Err(e) = self.host.block_service_workers(key) {
                        self.dispose_context(&context_id, &mut Discard);
                        return Err(e);
                    }
                }
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
                    keep_initial: false,
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
            let Ok((page, target_id, prebuilt)) = self.host.create_popup_page(context_key, request.handle.as_ref())
            else {
                continue;
            };
            // A popup opened without a URL keeps the initial about:blank
            // document `window.open` made (HTML does not navigate it).
            let keep_initial = prebuilt && request.url == "about:blank";
            let url = (!keep_initial).then_some(request.url);
            let paused = self.auto_attach || self.tab_attach;
            self.targets.push(TargetEntry {
                id: target_id.clone(),
                context_id,
                page,
                opener_id: Some(opener_id),
                can_access_opener: request.opener_access,
                // Paused, a kept document still waits for its announced commit.
                pending_url: paused.then(|| url.clone().unwrap_or_else(|| "about:blank".into())),
                keep_initial,
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
            } else if keep_initial {
                self.host.announce_initial_document(page);
            } else if url.as_deref().is_some_and(|url| self.host.navigate(page, url).is_err()) {
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
                        Some(pending) => {
                            *pending = url;
                            // It loads a document now instead of keeping its initial one.
                            target.keep_initial = false;
                        }
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
            WorkerChange::Started { worker, page, url, service } => {
                // A service worker's target is a browser-level one, beside the
                // pages, whose ids the host picks: a prefix keeps them apart.
                let target_id = self.mint(if service { "SW" } else { "" });
                let context_id = self.targets.iter().find(|t| t.page == page).map(|t| t.context_id.clone());
                self.workers.push(WorkerEntry { key: worker, page, target_id, url, service, context_id });
                let index = self.workers.len() - 1;
                if self.discover {
                    let params = serde_json::json!({ "targetInfo": self.worker_info(index, false) });
                    out.send(msg::event_frame("Target.targetCreated", &params, None));
                }
                if service {
                    if self.auto_attach {
                        self.attach_worker(index, None, out);
                    }
                    return;
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
                    self.attach_worker(index, Some(&parent), out);
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
        serde_json::json!({
            "targetId": worker.target_id, "type": if worker.service { "service_worker" } else { "worker" },
            "title": "", "url": worker.url,
            "attached": attached, "canAccessOpener": false, "browserContextId": worker.context_id,
        })
    }

    /// Attaches `parent`, a session of the worker's page (`None`: the browser
    /// connection, for a service worker), once. Past the session limit the
    /// worker simply has no session there.
    fn attach_worker(&mut self, index: usize, parent: Option<&str>, out: &mut dyn Sink) {
        let worker = self.workers[index].key;
        if self.sessions.values().any(|s| s.worker == Some(worker) && s.parent.as_deref() == parent)
            || self.sessions.len() + self.browser_sessions.len() >= MAX_SESSIONS
        {
            return;
        }
        let session_id = self.mint("");
        self.sessions.insert(
            session_id.clone(),
            SessionEntry {
                target_id: self.workers[index].target_id.clone(),
                parent: parent.map(str::to_owned),
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
        out.send(msg::event_frame("Target.attachedToTarget", &params, parent));
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
        let keep_initial = target.keep_initial;
        self.host.set_initial_popup(page, false);
        // A kept initial document: clients (Playwright's `popup` event) wait
        // for a commit, which the host announces for it.
        if keep_initial && self.host.announce_initial_document(page) {
            return;
        }
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
