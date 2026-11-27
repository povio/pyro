//! `Emulation.setSensorOverrideEnabled`, `setSensorOverrideReadings` and `getOverriddenSensorInformation`: the
//! virtual sensors of the page's browser context (`pyro_webapi::ContextServices`), as Chrome's CDP has them
//! (what WebDriver's "create / update / delete / get virtual sensor" commands of the Generic Sensor
//! specification are implemented with). A virtual sensor is a real data source: readings it is given go through
//! the sampling, rounding and threshold rules of the sensor, to every `Sensor` object and event listener of the
//! context that uses it.

use serde::Deserialize;

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    available: Option<bool>,
    minimum_frequency: Option<f64>,
    maximum_frequency: Option<f64>,
}

#[derive(Deserialize)]
struct Single {
    value: f64,
}

#[derive(Deserialize)]
struct Xyz {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize)]
struct Quaternion {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

#[derive(Deserialize)]
struct Reading {
    single: Option<Single>,
    xyz: Option<Xyz>,
    quaternion: Option<Quaternion>,
}

impl EngineHost {
    pub(super) fn sensor_override(&self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            enabled: Option<bool>,
            r#type: String,
            metadata: Option<Metadata>,
            reading: Option<Reading>,
        }
        let p: P = cmd.command.params()?;
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let context = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let services = self.thread.browser.context_services(context).ok_or_else(|| CdpError::server("page is gone"))?;
        match action {
            "setSensorOverrideEnabled" => {
                // `enabled` is required (Chrome: "Invalid parameters", `enabled: boolean value expected`).
                let enabled = p.enabled.ok_or_else(|| CdpError::invalid_params("Invalid parameters: enabled: boolean value expected"))?;
                if enabled {
                    let metadata = p.metadata;
                    let available = metadata.as_ref().and_then(|m| m.available).unwrap_or(true);
                    let min = metadata.as_ref().and_then(|m| m.minimum_frequency);
                    let max = metadata.as_ref().and_then(|m| m.maximum_frequency);
                    services.enable_sensor(&p.r#type, available, min, max).map_err(CdpError::invalid_params)?;
                } else {
                    services.disable_sensor(&p.r#type);
                }
                Ok(Some(serde_json::json!({})))
            }
            "setSensorOverrideReadings" => {
                let reading = p.reading.ok_or_else(|| CdpError::invalid_params("Missing reading"))?;
                let reading = match (reading.single, reading.xyz, reading.quaternion) {
                    (Some(single), None, None) => pyro_webapi::SensorReading::Single(single.value),
                    (None, Some(xyz), None) => pyro_webapi::SensorReading::Xyz([xyz.x, xyz.y, xyz.z]),
                    (None, None, Some(q)) => pyro_webapi::SensorReading::Quaternion([q.x, q.y, q.z, q.w]),
                    _ => return Err(CdpError::invalid_params("A reading is exactly one of single, xyz and quaternion")),
                };
                services.set_sensor_reading(&p.r#type, &reading).map_err(CdpError::invalid_params)?;
                Ok(Some(serde_json::json!({})))
            }
            "getOverriddenSensorInformation" => {
                let frequency = services.sensor_requested_frequency(&p.r#type).map_err(CdpError::invalid_params)?;
                Ok(Some(serde_json::json!({ "requestedSamplingFrequency": frequency })))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }
}
