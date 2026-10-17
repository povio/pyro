//! Device metrics over the browser's persistent page viewport. Mobile layout,
//! orientation overrides and visual viewport transforms remain explicit gaps.
use std::collections::BTreeMap;

use lp_dom::media::Viewport;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{remote, EngineHost};
use crate::msg::{CdpError, CdpResult};
use crate::target::PageCommand;

const MAX_DIMENSION: u32 = 10_000_000;

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum OrientationType {
    PortraitPrimary,
    PortraitSecondary,
    LandscapePrimary,
    LandscapeSecondary,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Orientation {
    r#type: OrientationType,
    angle: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metrics {
    width: u32,
    height: u32,
    device_scale_factor: f64,
    mobile: bool,
    screen_width: Option<u32>,
    screen_height: Option<u32>,
    screen_orientation: Option<Orientation>,
    // There is no separate visible OS window to resize in this headless host.
    #[serde(default, rename = "dontSetVisibleSize")]
    _dont_set_visible_size: bool,
    #[serde(flatten)]
    unsupported: BTreeMap<String, Value>,
}

impl EngineHost {
    pub(super) fn device_metrics(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let mut viewport =
            self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.viewport().clone();
        let defaults = Viewport::default();
        if action == "clearDeviceMetricsOverride" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Clear {}
            cmd.command.params::<Clear>()?;
            viewport.width = defaults.width;
            viewport.height = defaults.height;
            viewport.device_scale_factor = defaults.device_scale_factor;
            viewport.screen_width = defaults.screen_width;
            viewport.screen_height = defaults.screen_height;
        } else {
            let p: Metrics = cmd.command.params()?;
            if [Some(p.width), Some(p.height), p.screen_width, p.screen_height]
                .into_iter()
                .flatten()
                .any(|n| n > MAX_DIMENSION)
                || !p.device_scale_factor.is_finite()
                || p.device_scale_factor < 0.0
            {
                return Err(CdpError::invalid_params("Invalid device dimensions or scale factor"));
            }
            if p.mobile {
                return Err(CdpError::unsupported("Emulation mobile layout"));
            }
            if let Some(option) = p.unsupported.keys().next() {
                return Err(CdpError::unsupported(&format!("Emulation device metrics option '{option}'")));
            }
            viewport.width = if p.width == 0 { defaults.width } else { p.width };
            viewport.height = if p.height == 0 { defaults.height } else { p.height };
            viewport.device_scale_factor =
                if p.device_scale_factor == 0.0 { defaults.device_scale_factor } else { p.device_scale_factor };
            viewport.screen_width = p.screen_width;
            viewport.screen_height = p.screen_height;
            if let Some(orientation) = p.screen_orientation {
                let landscape = viewport.screen_width.unwrap_or(viewport.width)
                    >= viewport.screen_height.unwrap_or(viewport.height);
                let native =
                    if landscape { OrientationType::LandscapePrimary } else { OrientationType::PortraitPrimary };
                if orientation.angle != 0 || orientation.r#type != native {
                    return Err(CdpError::unsupported("Emulation screen orientation override"));
                }
            }
        }
        self.thread.browser.set_viewport(id, viewport).map_err(|error| match error {
            lp_js::JsError::Range(message) | lp_js::JsError::Type(message) => CdpError::invalid_params(message),
            other => remote::js_to_cdp(other),
        })?;
        Ok(Some(json!({})))
    }
}
