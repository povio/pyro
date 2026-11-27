//! The `WebAuthn` domain (Chrome's virtual authenticator environment): authenticators and their
//! credentials are held by the page's browser context (`pyro_webapi::ContextServices`), which
//! `navigator.credentials` of every document in it asks. Commands and results have Chrome's shapes
//! (`enable`, `disable`, `addVirtualAuthenticator`, `removeVirtualAuthenticator`, `addCredential`,
//! `getCredential`, `getCredentials`, `removeCredential`, `clearCredentials`, `setUserVerified`,
//! `setAutomaticPresenceSimulation`, `setCredentialProperties`, `setResponseOverrideBits`); binary
//! values are base64.
//!
//! `enable` is per CDP session: only an enabled session's commands reach the authenticators, and
//! only enabled sessions receive `credentialAdded`, `credentialAsserted`, `credentialUpdated` and
//! `credentialDeleted`. The authenticators themselves are the context's (the one place
//! `navigator.credentials` of its documents looks), so they exist while any session of the context
//! has the environment enabled and are dropped when the last one disables it, detaches or closes.

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult};
use crate::target::ContextKey;

impl EngineHost {
    pub(super) fn webauthn_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let context = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let services = self.thread.browser.context_services(context).ok_or_else(|| CdpError::server("page is gone"))?;
        let params: serde_json::Value = cmd.command.params()?;
        match action {
            "enable" => self.session_mut(cmd.session_id)?.webauthn_enabled = true,
            "disable" => {
                self.session_mut(cmd.session_id)?.webauthn_enabled = false;
                // Other sessions of the context keep the environment.
                let key = self.pages.get(&cmd.page).map(|p| p.context).ok_or_else(|| CdpError::server("page is gone"))?;
                if self.webauthn_enabled_in(key) {
                    return Ok(Some(serde_json::json!({})));
                }
            }
            _ if !self.sessions.get(cmd.session_id).is_some_and(|s| s.webauthn_enabled) => {
                return Err(CdpError::server("The Virtual Authenticator Environment has not been enabled for this session"));
            }
            _ => {}
        }
        match services.webauthn_command(action, &params) {
            Ok(result) => Ok(Some(result)),
            Err(pyro_webapi::WebAuthnError::Invalid(message)) => Err(CdpError::invalid_params(message)),
            Err(pyro_webapi::WebAuthnError::Failed(message)) => Err(CdpError::server(message)),
        }
    }

    /// A session of one of `context`'s pages has the virtual authenticator environment enabled.
    fn webauthn_enabled_in(&self, context: ContextKey) -> bool {
        self.pages
            .values()
            .filter(|p| p.context == context)
            .flat_map(|p| p.sessions.iter())
            .any(|s| self.sessions.get(s).is_some_and(|s| s.webauthn_enabled))
    }

    /// The last enabled session of `context` is gone: its authenticators go with it.
    pub(super) fn webauthn_release(&self, context: ContextKey, services: &pyro_webapi::ContextServices) {
        if !self.webauthn_enabled_in(context) {
            let _ = services.webauthn_command("disable", &serde_json::json!({}));
        }
    }

    /// A session that had the environment enabled detached from `page`.
    pub(super) fn webauthn_session_gone(&self, page: crate::target::PageKey) {
        let Some(state) = self.pages.get(&page) else { return };
        let Some(services) = self.thread.browser.page(state.id).and_then(|p| self.thread.browser.context_services(p.context)) else {
            return;
        };
        self.webauthn_release(state.context, &services);
    }

    /// The `WebAuthn.credential*` events the virtual authenticators raised, to the sessions that
    /// enabled the environment (those of the context's pages).
    pub(super) fn flush_webauthn(&mut self) {
        if !self.sessions.values().any(|s| s.webauthn_enabled) {
            return;
        }
        let mut seen: Vec<ContextKey> = Vec::new();
        let keys: Vec<(crate::target::PageKey, ContextKey)> = self.pages.iter().map(|(k, p)| (*k, p.context)).collect();
        for (page, context) in keys {
            if seen.contains(&context) {
                continue;
            }
            seen.push(context);
            let Some(id) = self.pages.get(&page).map(|p| p.id) else { continue };
            let Some(services) = self.thread.browser.page(id).and_then(|p| self.thread.browser.context_services(p.context)) else {
                continue;
            };
            let events = services.take_webauthn_events();
            if events.is_empty() {
                continue;
            }
            let sessions: Vec<String> = self
                .pages
                .values()
                .filter(|p| p.context == context)
                .flat_map(|p| p.sessions.iter())
                .filter(|s| self.sessions.get(*s).is_some_and(|s| s.webauthn_enabled))
                .cloned()
                .collect();
            for (method, params) in events {
                self.event_to(&format!("WebAuthn.{method}"), &params, &sessions);
            }
        }
    }
}
