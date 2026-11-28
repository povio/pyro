//! Emulation.setPressureSourceOverrideEnabled / setPressureStateOverride / setPressureDataOverride: the context's
//! virtual Compute Pressure sources (`pyro_webapi::pressure::PressureHub`), which `PressureObserver` reads in place of
//! the host's. These are Chrome's methods and shapes; the WebDriver commands "create / update / delete virtual
//! pressure source" are made of them (bench/wpt/td/pressure.mjs). Held per browser context, like the geolocation
//! override: a page's override applies to its whole context, workers included.

use pyro_webapi::pressure::PressureState;
use serde::Deserialize;

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult};

impl EngineHost {
    pub(super) fn pressure_override(&self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        #[derive(Deserialize)]
        struct Metadata {
            available: Option<bool>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            source: String,
            enabled: Option<bool>,
            metadata: Option<Metadata>,
            state: Option<String>,
            own_contribution_estimate: Option<f64>,
        }
        let p: P = cmd.command.params()?;
        if p.source != "cpu" {
            return Err(CdpError::invalid_params(format!("Invalid source '{}'", p.source)));
        }
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let context = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let services = self.thread.browser.context_services(context).ok_or_else(|| CdpError::server("page is gone"))?;
        let hub = services.pressure();
        if action == "setPressureSourceOverrideEnabled" {
            let enabled = p.enabled.ok_or_else(|| CdpError::invalid_params("enabled is required"))?;
            let available = p.metadata.and_then(|m| m.available).unwrap_or(true);
            hub.set_override(&p.source, enabled, available).map_err(CdpError::invalid_params)?;
            return Ok(Some(serde_json::json!({})));
        }
        let state = p.state.ok_or_else(|| CdpError::invalid_params("state is required"))?;
        let state = PressureState::parse(&state).ok_or_else(|| CdpError::invalid_params(format!("Invalid state '{state}'")))?;
        // The protocol's `ownContributionEstimate` is validated, then dropped: the web API's `PressureRecord` has no
        // member to carry it (compute-pressure.idl), so nothing would read it.
        let estimate = if action == "setPressureDataOverride" { p.own_contribution_estimate } else { None };
        if estimate.is_some_and(|e| !(0.0..=1.0).contains(&e)) {
            return Err(CdpError::invalid_params("ownContributionEstimate must be between 0 and 1"));
        }
        hub.set_reading(&p.source, state).map_err(CdpError::server)?;
        Ok(Some(serde_json::json!({})))
    }
}
