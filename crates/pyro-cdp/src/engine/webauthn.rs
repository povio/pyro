//! The `WebAuthn` domain (Chrome's virtual authenticator environment): authenticators and their
//! credentials are held by the page's browser context (`pyro_webapi::ContextServices`), which
//! `navigator.credentials` of every document in it asks. Commands and results have Chrome's shapes
//! (`enable`, `addVirtualAuthenticator`, `removeVirtualAuthenticator`, `addCredential`,
//! `getCredential`, `getCredentials`, `removeCredential`, `clearCredentials`, `setUserVerified`,
//! `setAutomaticPresenceSimulation`, `setCredentialProperties`); binary values are base64.

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult};

impl EngineHost {
    pub(super) fn webauthn_domain(&self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let context = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let services = self.thread.browser.context_services(context).ok_or_else(|| CdpError::server("page is gone"))?;
        let params: serde_json::Value = cmd.command.params()?;
        match services.webauthn_command(action, &params) {
            Ok(result) => Ok(Some(result)),
            Err(pyro_webapi::WebAuthnError::Invalid(message)) => Err(CdpError::invalid_params(message)),
            Err(pyro_webapi::WebAuthnError::Failed(message)) => Err(CdpError::server(message)),
        }
    }
}
