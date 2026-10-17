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

use std::collections::HashMap;

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
pub trait Host {
    /// Creates an isolated browser context (cookies, storage, cache).
    fn create_context(&mut self) -> CdpResult<ContextKey>;
    fn dispose_context(&mut self, context: ContextKey);
    /// Creates an `about:blank` page whose main frame id is `target_id`.
    fn create_page(&mut self, context: ContextKey, target_id: &str) -> CdpResult<PageKey>;
    /// Starts a navigation (used by `Target.createTarget` with a URL).
    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()>;
    fn close_page(&mut self, page: PageKey);
    fn page_url(&self, page: PageKey) -> String;
    fn page_title(&self, page: PageKey) -> String;
    /// A session attached to `page`; the host may start emitting its events.
    fn session_attached(&mut self, page: PageKey, session_id: &str);
    /// A session detached; the host must stop emitting events for it and drop
    /// per-session state (enabled domains, object handles, interception).
    fn session_detached(&mut self, page: PageKey, session_id: &str);
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
}

struct TargetEntry {
    id: String,
    context_id: String,
    page: PageKey,
}

struct SessionEntry {
    target_id: String,
}

/// Per-connection browser protocol state.
pub struct Browser<H: Host> {
    host: H,
    auto_attach: bool,
    discover: bool,
    contexts: Vec<ContextEntry>,
    targets: Vec<TargetEntry>,
    sessions: HashMap<String, SessionEntry>,
    next_id: u64,
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
    browser_context_id: Option<&'a str>,
}

impl<H: Host> Browser<H> {
    pub fn new(host: H) -> Self {
        Self {
            host,
            auto_attach: false,
            discover: false,
            contexts: Vec::new(),
            targets: Vec::new(),
            sessions: HashMap::new(),
            next_id: 1,
        }
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
        let command = match msg::parse_command(text) {
            Ok(command) => command,
            Err(failure) => {
                out.send(msg::error_frame(failure.id, &failure.error, failure.session_id.as_deref()));
                return;
            }
        };
        let session_id = command.session_id().map(str::to_owned);
        if let Err(error) = self.dispatch(&command, out) {
            out.send(msg::error_frame(Some(command.id), &error, session_id.as_deref()));
        }
    }

    fn dispatch(&mut self, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        if let Some(session_id) = cmd.session_id() {
            return self.dispatch_session(session_id, cmd, out);
        }
        let (domain, action) = cmd.domain_and_action().ok_or_else(|| CdpError::method_not_found(&cmd.method))?;
        match (domain, action) {
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
                out.send(msg::result_frame(cmd.id, &version, None));
                Ok(())
            }
            ("Target", _) => self.target_command(action, cmd, out),
            _ => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct ContextParam {
                    browser_context_id: Option<String>,
                }
                let context = match cmd.raw_params().and_then(|p| serde_json::from_str::<ContextParam>(p.get()).ok()) {
                    Some(ContextParam { browser_context_id: Some(id) }) => Some(self.context(&id)?.key),
                    _ => None,
                };
                self.host.browser_command(cmd, context, out);
                Ok(())
            }
        }
    }

    fn dispatch_session(&mut self, session_id: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let session =
            self.sessions.get(session_id).ok_or_else(|| CdpError::new(-32001, "Session with given id not found."))?;
        let target = self
            .targets
            .iter()
            .find(|t| t.id == session.target_id)
            .ok_or_else(|| CdpError::server("target of session is gone"))?;
        let (page, target_id) = (target.page, target.id.clone());
        match cmd.method.as_ref() {
            // Session-scoped Target commands Playwright sends on page sessions.
            "Target.setAutoAttach" => {
                // Child targets (OOPIFs, workers) are not separate targets here:
                // iframes are same-process frames reported through Page events.
                let _: AutoAttachParams = cmd.params()?;
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
            }
            "Target.getTargetInfo" => {
                let url = self.host.page_url(page);
                let title = self.host.page_title(page);
                let target = self.target(&target_id)?;
                let info = self.info(target, &url, &title, true);
                out.send(msg::result_frame(cmd.id, &serde_json::json!({ "targetInfo": info }), Some(session_id)));
            }
            "Runtime.runIfWaitingForDebugger" => {
                // Targets never start paused (waitingForDebugger is always false).
                out.send(msg::result_frame(cmd.id, &Empty {}, Some(session_id)));
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
            can_access_opener: false,
            browser_context_id: Some(&target.context_id),
        }
    }

    fn target_command(&mut self, action: &str, cmd: &Command<'_>, out: &mut dyn Sink) -> CdpResult<()> {
        let id = cmd.id;
        match action {
            "setAutoAttach" => {
                let params: AutoAttachParams = cmd.params()?;
                if !params.flatten {
                    return Err(CdpError::server("Only flatten mode is supported"));
                }
                self.auto_attach = params.auto_attach;
                out.send(msg::result_frame(id, &Empty {}, None));
                if self.auto_attach {
                    let unattached: Vec<String> = self
                        .targets
                        .iter()
                        .filter(|t| !self.sessions.values().any(|s| s.target_id == t.id))
                        .map(|t| t.id.clone())
                        .collect();
                    for target_id in unattached {
                        self.attach(&target_id, out)?;
                    }
                } else {
                    let sessions: Vec<String> = self.sessions.keys().cloned().collect();
                    for session_id in sessions {
                        self.detach_session(&session_id, out);
                    }
                }
            }
            "setDiscoverTargets" => {
                #[derive(Deserialize)]
                struct P {
                    discover: bool,
                }
                self.discover = cmd.params::<P>()?.discover;
                out.send(msg::result_frame(id, &Empty {}, None));
                if self.discover {
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
                        let attached = self.sessions.values().any(|s| s.target_id == target.id);
                        serde_json::to_value(self.info(target, &url, &title, attached)).expect("serializable")
                    }
                };
                out.send(msg::result_frame(id, &serde_json::json!({ "targetInfo": info }), None));
            }
            "getTargets" => {
                let infos: Vec<serde_json::Value> = self
                    .targets
                    .iter()
                    .map(|t| {
                        let url = self.host.page_url(t.page);
                        let title = self.host.page_title(t.page);
                        let attached = self.sessions.values().any(|s| s.target_id == t.id);
                        serde_json::to_value(self.info(t, &url, &title, attached)).expect("serializable")
                    })
                    .collect();
                out.send(msg::result_frame(id, &serde_json::json!({ "targetInfos": infos }), None));
            }
            "getBrowserContexts" => {
                let ids: Vec<&str> = self.contexts.iter().map(|c| c.id.as_str()).collect();
                out.send(msg::result_frame(id, &serde_json::json!({ "browserContextIds": ids }), None));
            }
            "createBrowserContext" => {
                // disposeOnDetach is implied: every context of a connection
                // is disposed when it closes (see `shutdown`).
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    proxy_server: Option<String>,
                }
                let params: P = cmd.params()?;
                if params.proxy_server.is_some() {
                    return Err(CdpError::unsupported("Target.createBrowserContext proxyServer"));
                }
                let key = self.host.create_context()?;
                let context_id = self.mint("");
                self.contexts.push(ContextEntry { id: context_id.clone(), key });
                out.send(msg::result_frame(id, &serde_json::json!({ "browserContextId": context_id }), None));
            }
            "disposeBrowserContext" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    browser_context_id: String,
                }
                let context_id = cmd.params::<P>()?.browser_context_id;
                self.context(&context_id)?;
                self.dispose_context(&context_id, out);
                out.send(msg::result_frame(id, &Empty {}, None));
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
                let target_id = self.mint("");
                let page = self.host.create_page(context_key, &target_id)?;
                self.targets.push(TargetEntry { id: target_id.clone(), context_id, page });
                if self.discover {
                    self.send_target_created(self.targets.len() - 1, out);
                }
                // attachedToTarget must precede the result: Playwright looks the
                // page up by targetId as soon as createTarget returns.
                if self.auto_attach {
                    self.attach(&target_id, out)?;
                }
                if !params.url.is_empty() && params.url != "about:blank" {
                    self.host.navigate(page, &params.url)?;
                }
                out.send(msg::result_frame(id, &serde_json::json!({ "targetId": target_id }), None));
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
                out.send(msg::result_frame(id, &serde_json::json!({ "success": true }), None));
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
                let session_id = self.attach(&params.target_id, out)?;
                out.send(msg::result_frame(id, &serde_json::json!({ "sessionId": session_id }), None));
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
                        .find(|(_, s)| s.target_id == t)
                        .map(|(k, _)| k.clone())
                        .ok_or_else(|| CdpError::invalid_params("No session for the given target"))?,
                    (None, None) => {
                        return Err(CdpError::invalid_params("Either sessionId or targetId must be specified"))
                    }
                };
                if !self.sessions.contains_key(&session_id) {
                    return Err(CdpError::new(-32001, "Session with given id not found."));
                }
                self.detach_session(&session_id, out);
                out.send(msg::result_frame(id, &Empty {}, None));
            }
            "activateTarget" => {
                out.send(msg::result_frame(id, &Empty {}, None));
            }
            _ => return Err(CdpError::method_not_found(&cmd.method)),
        }
        Ok(())
    }

    /// `Target.createTarget` without a context uses an implicit default context.
    fn default_context(&mut self) -> CdpResult<String> {
        if let Some(c) = self.contexts.first() {
            return Ok(c.id.clone());
        }
        let key = self.host.create_context()?;
        let id = self.mint("");
        self.contexts.push(ContextEntry { id: id.clone(), key });
        Ok(id)
    }

    fn attach(&mut self, target_id: &str, out: &mut dyn Sink) -> CdpResult<String> {
        let session_id = self.mint("");
        self.sessions.insert(session_id.clone(), SessionEntry { target_id: target_id.to_owned() });
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
            waiting_for_debugger: false,
        };
        out.send(msg::event_frame("Target.attachedToTarget", &event, None));
        self.host.session_attached(page, &session_id);
        Ok(session_id)
    }

    fn detach_session(&mut self, session_id: &str, out: &mut dyn Sink) {
        let Some(session) = self.sessions.remove(session_id) else { return };
        if let Some(target) = self.targets.iter().find(|t| t.id == session.target_id) {
            self.host.session_detached(target.page, session_id);
        }
        out.send(msg::event_frame(
            "Target.detachedFromTarget",
            &serde_json::json!({ "sessionId": session_id, "targetId": session.target_id }),
            None,
        ));
    }

    fn close_target(&mut self, target_id: &str, out: &mut dyn Sink) {
        let sessions: Vec<String> =
            self.sessions.iter().filter(|(_, s)| s.target_id == target_id).map(|(k, _)| k.clone()).collect();
        for session_id in &sessions {
            if let Some(session) = self.sessions.remove(session_id) {
                if let Some(target) = self.targets.iter().find(|t| t.id == session.target_id) {
                    self.host.session_detached(target.page, session_id);
                }
                out.send(msg::event_frame(
                    "Target.detachedFromTarget",
                    &serde_json::json!({ "sessionId": session_id, "targetId": target_id }),
                    None,
                ));
            }
        }
        if let Some(pos) = self.targets.iter().position(|t| t.id == target_id) {
            let target = self.targets.remove(pos);
            self.host.close_page(target.page);
            if self.discover {
                out.send(msg::event_frame(
                    "Target.targetDestroyed",
                    &serde_json::json!({ "targetId": target_id }),
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

    fn send_target_created(&self, index: usize, out: &mut dyn Sink) {
        let target = &self.targets[index];
        let url = self.host.page_url(target.page);
        let title = self.host.page_title(target.page);
        let attached = self.sessions.values().any(|s| s.target_id == target.id);
        out.send(msg::event_frame(
            "Target.targetCreated",
            &serde_json::json!({ "targetInfo": self.info(target, &url, &title, attached) }),
            None,
        ));
    }

    /// Tears down everything when the client disconnects. This is also when
    /// `disposeOnDetach` contexts are disposed: the flag refers to the
    /// connection, not to page sessions.
    pub fn shutdown(&mut self) {
        let mut sink: Vec<OutFrame> = Vec::new();
        let ids: Vec<String> = self.contexts.iter().map(|c| c.id.clone()).collect();
        for id in ids {
            self.dispose_context(&id, &mut sink);
        }
    }
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
}
