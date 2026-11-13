//! Browser.grantPermissions / resetPermissions / setPermission: the
//! context's permission table (pyro_webapi::ContextServices), which
//! `navigator.permissions.query`, the clipboard and `navigator.geolocation`
//! read, and the context's geolocation override.

use pyro_net::Url;
use serde::Deserialize;

use super::{EngineHost, PageCommand};
use crate::msg::{CdpError, CdpResult, Command};
use crate::target::ContextKey;

/// The `navigator.permissions.query` names a CDP `PermissionType` covers
/// (`None`: not a type Chrome knows). Types with no queryable name are
/// accepted and change nothing.
fn query_names(permission: &str) -> Option<&'static [&'static str]> {
    Some(match permission {
        "audioCapture" => &["microphone"],
        "videoCapture" | "videoCapturePanTiltZoom" | "cameraPanTiltZoom" => &["camera"],
        "backgroundFetch" => &["background-fetch"],
        "backgroundSync" => &["background-sync"],
        "capturedSurfaceControl" => &["captured-surface-control"],
        // Chrome keys the table by type: reading needs clipboardReadWrite,
        // writing clipboardSanitizedWrite (measured: either alone denies
        // the other).
        "clipboardReadWrite" => &["clipboard-read"],
        "clipboardSanitizedWrite" => &["clipboard-write"],
        "displayCapture" => &["display-capture"],
        "durableStorage" => &["persistent-storage"],
        "geolocation" => &["geolocation"],
        "idleDetection" => &["idle-detection"],
        "keyboardLock" => &["keyboard-lock"],
        "localFonts" => &["local-fonts"],
        "midi" | "midiSysex" => &["midi"],
        "nfc" => &["nfc"],
        "notifications" => &["notifications", "push"],
        "paymentHandler" => &["payment-handler"],
        "periodicBackgroundSync" => &["periodic-background-sync"],
        "pointerLock" => &["pointer-lock"],
        "sensors" => &["accelerometer", "gyroscope", "magnetometer"],
        "speakerSelection" => &["speaker-selection"],
        "storageAccess" => &["storage-access"],
        "topLevelStorageAccess" => &["top-level-storage-access"],
        "wakeLockScreen" => &["screen-wake-lock"],
        "wakeLockSystem" => &["system-wake-lock"],
        "windowManagement" => &["window-management"],
        "ar"
        | "automaticFullscreen"
        | "handTracking"
        | "localNetworkAccess"
        | "protectedMediaIdentifier"
        | "smartCard"
        | "vr"
        | "webAppInstallation"
        | "webPrinting" => &[],
        _ => return None,
    })
}

/// A permission's origin as documents report theirs (`None`: absent, every
/// origin). Like Chrome, anything that is not a URL with a tuple origin
/// (`*` included) is an opaque origin.
fn origin(raw: Option<String>) -> CdpResult<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let origin = Url::parse(&raw).ok().map(|url| url.origin()).filter(|o| o.is_tuple());
    let origin = origin.ok_or_else(|| CdpError::invalid_params("Permission can't be granted to opaque origins."))?;
    Ok(Some(origin.ascii_serialization()))
}

impl EngineHost {
    /// `Emulation.setGeolocationOverride` / `clearGeolocationOverride`. Held by
    /// the browser context: a page's override applies to its whole context,
    /// which is how Playwright uses it. Without coordinates the position is
    /// unavailable, as in Chrome.
    pub(super) fn geolocation_override(
        &self,
        action: &str,
        cmd: &PageCommand<'_, '_>,
    ) -> CdpResult<Option<serde_json::Value>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            latitude: Option<f64>,
            longitude: Option<f64>,
            accuracy: Option<f64>,
            altitude: Option<f64>,
            altitude_accuracy: Option<f64>,
            heading: Option<f64>,
            speed: Option<f64>,
        }
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let context = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.context;
        let services = self.thread.browser.context_services(context).ok_or_else(|| CdpError::server("page is gone"))?;
        let fix = if action == "clearGeolocationOverride" {
            None
        } else {
            let p: P = cmd.command.params()?;
            let in_range = |v: Option<f64>, max: f64| v.is_none_or(|v| v.abs() <= max);
            if !in_range(p.latitude, 90.0) {
                return Err(CdpError::invalid_params("Invalid latitude"));
            }
            if !in_range(p.longitude, 180.0) {
                return Err(CdpError::invalid_params("Invalid longitude"));
            }
            if p.accuracy.is_some_and(|a| a < 0.0) {
                return Err(CdpError::invalid_params("Invalid accuracy"));
            }
            Some(match (p.latitude, p.longitude, p.accuracy) {
                (Some(latitude), Some(longitude), Some(accuracy)) => Some(pyro_webapi::GeolocationFix {
                    latitude,
                    longitude,
                    accuracy,
                    altitude: p.altitude,
                    altitude_accuracy: p.altitude_accuracy,
                    heading: p.heading,
                    speed: p.speed,
                }),
                _ => None,
            })
        };
        services.set_geolocation(fix);
        Ok(Some(serde_json::json!({})))
    }

    pub(super) fn permissions_command(&self, command: &Command<'_>, context: Option<ContextKey>) -> CdpResult<()> {
        let context = context
            .and_then(|key| self.contexts.get(&key))
            .ok_or_else(|| CdpError::server("Browser context is gone"))?;
        let services = self
            .thread
            .browser
            .context_services(context.id)
            .ok_or_else(|| CdpError::server("Browser context is gone"))?;
        match command.method.as_ref() {
            "Browser.grantPermissions" => {
                #[derive(Deserialize)]
                struct P {
                    permissions: Vec<String>,
                    origin: Option<String>,
                }
                let p: P = command.params()?;
                let mut names = Vec::new();
                for permission in &p.permissions {
                    let covered = query_names(permission)
                        .ok_or_else(|| CdpError::invalid_params(format!("Unknown permission type: {permission}")))?;
                    names.extend(covered.iter().map(|n| n.to_string()));
                }
                services.grant(origin(p.origin)?, &names);
            }
            "Browser.resetPermissions" => services.reset(),
            "Browser.setPermission" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Descriptor {
                    name: String,
                    #[serde(default)]
                    user_visible_only: bool,
                }
                #[derive(Deserialize)]
                struct P {
                    permission: Descriptor,
                    setting: String,
                    origin: Option<String>,
                }
                let p: P = command.params()?;
                let setting = match p.setting.as_str() {
                    "granted" => pyro_webapi::PermissionSetting::Granted,
                    "denied" => pyro_webapi::PermissionSetting::Denied,
                    "prompt" => pyro_webapi::PermissionSetting::Prompt,
                    other => return Err(CdpError::invalid_params(format!("Unknown permission setting: {other}"))),
                };
                let known = [
                    "accelerometer",
                    "background-fetch",
                    "background-sync",
                    "camera",
                    "captured-surface-control",
                    "clipboard-read",
                    "clipboard-write",
                    "display-capture",
                    "geolocation",
                    "gyroscope",
                    "idle-detection",
                    "keyboard-lock",
                    "local-fonts",
                    "magnetometer",
                    "microphone",
                    "midi",
                    "nfc",
                    "notifications",
                    "payment-handler",
                    "periodic-background-sync",
                    "persistent-storage",
                    "pointer-lock",
                    "push",
                    "screen-wake-lock",
                    "speaker-selection",
                    "storage-access",
                    "system-wake-lock",
                    "top-level-storage-access",
                    "window-management",
                ];
                if !known.contains(&p.permission.name.as_str()) {
                    return Err(CdpError::invalid_params(format!(
                        "Invalid PermissionDescriptor name: {}",
                        p.permission.name
                    )));
                }
                if p.permission.name == "push" && !p.permission.user_visible_only {
                    return Err(CdpError::invalid_params(
                        "Push Permission without userVisibleOnly:true isn't supported",
                    ));
                }
                services.set(origin(p.origin)?, &p.permission.name, setting);
            }
            _ => return Err(CdpError::unsupported(&command.method)),
        }
        services.notify_changed();
        Ok(())
    }
}
