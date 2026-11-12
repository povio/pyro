//! Device metrics over the browser's persistent page viewport. Screen
//! orientation overrides are stored on the viewport and reported by
//! `screen.orientation` (as upstream and Chrome). Mobile layout and visual
//! viewport transforms remain explicit gaps. `Page.getLayoutMetrics` reports
//! the same viewport, window scroll offset and document scroll size.
use std::collections::BTreeMap;

use pyro_dom::media::{self, ScreenOrientation, Viewport};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{remote, EngineHost};
use crate::msg::{CdpError, CdpResult};
use crate::target::{PageCommand, PageKey};

const MAX_DIMENSION: u32 = 10_000_000;

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum OrientationType {
    PortraitPrimary,
    PortraitSecondary,
    LandscapePrimary,
    LandscapeSecondary,
}

impl From<OrientationType> for media::OrientationType {
    fn from(kind: OrientationType) -> Self {
        match kind {
            OrientationType::PortraitPrimary => media::OrientationType::PortraitPrimary,
            OrientationType::PortraitSecondary => media::OrientationType::PortraitSecondary,
            OrientationType::LandscapePrimary => media::OrientationType::LandscapePrimary,
            OrientationType::LandscapeSecondary => media::OrientationType::LandscapeSecondary,
        }
    }
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
    // Accepted without a separate layout viewport: `width` is the layout width
    // whatever the page's `<meta name=viewport>` says, as for
    // `width=device-width, initial-scale=1`.
    #[serde(rename = "mobile")]
    _mobile: bool,
    screen_width: Option<u32>,
    screen_height: Option<u32>,
    screen_orientation: Option<Orientation>,
    // There is no separate visible OS window to resize in this headless host.
    #[serde(default, rename = "dontSetVisibleSize")]
    _dont_set_visible_size: bool,
    #[serde(flatten)]
    unsupported: BTreeMap<String, Value>,
}

/// The media emulation of a page (`Emulation.setEmulatedMedia`, the
/// `media` run step): what replaces the viewport's media preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MediaEmulation {
    pub(super) color_scheme: media::ColorScheme,
    pub(super) reduced_motion: bool,
    pub(super) print: bool,
}

impl EngineHost {
    /// Applies `emulation` to the page's viewport: `matchMedia` and `@media`
    /// follow at once, and lists whose `matches` flipped get `change` at the
    /// next safe point.
    pub(super) fn apply_media_emulation(&mut self, page: PageKey, emulation: MediaEmulation) -> CdpResult<()> {
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let mut viewport =
            self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.viewport().clone();
        viewport.color_scheme = emulation.color_scheme;
        viewport.reduced_motion = emulation.reduced_motion;
        viewport.print = emulation.print;
        self.thread.browser.set_viewport(id, viewport).map_err(remote::js_to_cdp)?;
        Ok(())
    }

    /// Resizes the page's viewport (the `viewport` run step): `innerWidth`
    /// and `innerHeight` change at once, `resize` fires at the next safe point.
    pub(super) fn resize_viewport(&mut self, page: PageKey, width: u32, height: u32) -> CdpResult<()> {
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(CdpError::invalid_params("Invalid viewport dimensions"));
        }
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let mut viewport =
            self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.viewport().clone();
        viewport.width = width;
        viewport.height = height;
        self.thread.browser.set_viewport(id, viewport).map_err(remote::js_to_cdp)?;
        Ok(())
    }

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
            viewport.orientation = defaults.orientation;
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
            if let Some(option) = p.unsupported.keys().next() {
                return Err(CdpError::unsupported(&format!("Emulation device metrics option '{option}'")));
            }
            viewport.width = if p.width == 0 { defaults.width } else { p.width };
            viewport.height = if p.height == 0 { defaults.height } else { p.height };
            viewport.device_scale_factor =
                if p.device_scale_factor == 0.0 { defaults.device_scale_factor } else { p.device_scale_factor };
            viewport.screen_width = p.screen_width;
            viewport.screen_height = p.screen_height;
            // CDP and Chrome apply the override independently of the screen
            // dimensions: Playwright sends landscapePrimary/0 for every
            // non-mobile viewport, including portrait ones.
            viewport.orientation = match p.screen_orientation {
                Some(o) => {
                    let angle = u16::try_from(o.angle)
                        .ok()
                        .filter(|a| matches!(a, 0 | 90 | 180 | 270))
                        .ok_or_else(|| CdpError::invalid_params("Invalid screen orientation angle"))?;
                    Some(ScreenOrientation { kind: o.r#type.into(), angle })
                }
                None => None,
            };
        }
        self.thread.browser.set_viewport(id, viewport).map_err(|error| match error {
            pyro_js::JsError::Range(message) | pyro_js::JsError::Type(message) => CdpError::invalid_params(message),
            other => remote::js_to_cdp(other),
        })?;
        Ok(Some(json!({})))
    }

    /// `Page.getLayoutMetrics` for the main frame, in CSS px. There is no
    /// pinch zoom, so the visual viewport is the layout viewport. Headless
    /// Chromium reports the same values in the deprecated device-pixel
    /// fields (checked at device scale factors 1 and 2) and so does this.
    pub(super) fn layout_metrics(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct P {}
        cmd.command.params::<P>()?;
        let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let page = self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?;
        let (width, height) = (page.viewport().width, page.viewport().height);
        let (scroll_x, scroll_y, content_width, content_height) = match &page.main_frame.page_dom {
            Some(bindings) => {
                let dom = bindings.dom.borrow();
                let layout = bindings.layout();
                let (w, h) = layout.document_scroll_size(&dom);
                (layout.scroll_x(), layout.scroll_y(), w, h)
            }
            None => (0, 0, width as f64, height as f64),
        };
        let layout_viewport =
            json!({"pageX": scroll_x, "pageY": scroll_y, "clientWidth": width, "clientHeight": height});
        let visual_viewport = json!({
            "offsetX": 0, "offsetY": 0, "pageX": scroll_x, "pageY": scroll_y,
            "clientWidth": width, "clientHeight": height, "scale": 1, "zoom": 1,
        });
        let content_size = json!({"x": 0, "y": 0, "width": content_width, "height": content_height});
        Ok(Some(json!({
            "layoutViewport": layout_viewport,
            "visualViewport": visual_viewport,
            "contentSize": content_size,
            "cssLayoutViewport": layout_viewport,
            "cssVisualViewport": visual_viewport,
            "cssContentSize": content_size,
        })))
    }
}

/// `Emulation.UserAgentMetadata` as navigator.userAgentData reports it.
/// Required fields as in Chrome (a missing one is invalid params); the
/// optional ones left out take the browser's own values.
pub(super) fn user_agent_metadata(raw: Value) -> CdpResult<pyro_webapi::UserAgentMetadata> {
    #[derive(Deserialize)]
    struct Brand {
        brand: String,
        version: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct M {
        brands: Option<Vec<Brand>>,
        full_version_list: Option<Vec<Brand>>,
        full_version: Option<String>,
        platform: String,
        platform_version: String,
        architecture: String,
        model: String,
        mobile: bool,
        bitness: Option<String>,
        #[serde(default)]
        wow64: bool,
        form_factors: Option<Vec<String>>,
    }
    let m: M = serde_json::from_value(raw).map_err(|e| CdpError::invalid_params(format!("userAgentMetadata: {e}")))?;
    // Chrome's checks (printable ASCII), here before anything changes.
    let printable = |v: &str, what: &str| -> CdpResult<()> {
        if v.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            Ok(())
        } else {
            Err(CdpError::invalid_params(format!("Invalid {what} string")))
        }
    };
    for b in m.brands.iter().chain(&m.full_version_list).flatten() {
        printable(&b.brand, "brand")?;
        printable(&b.version, "brand version")?;
    }
    printable(m.full_version.as_deref().unwrap_or(""), "full version")?;
    printable(&m.platform, "platform")?;
    printable(&m.platform_version, "platform version")?;
    printable(&m.architecture, "architecture")?;
    printable(&m.model, "model")?;
    printable(m.bitness.as_deref().unwrap_or(""), "bitness")?;
    for f in m.form_factors.iter().flatten() {
        printable(f, "form factors")?;
    }
    let pairs = |list: Option<Vec<Brand>>| list.map(|l| l.into_iter().map(|b| (b.brand, b.version)).collect());
    Ok(pyro_webapi::UserAgentMetadata {
        brands: pairs(m.brands),
        full_version_list: pairs(m.full_version_list),
        full_version: m.full_version,
        platform: m.platform,
        platform_version: m.platform_version,
        architecture: m.architecture,
        model: m.model,
        mobile: m.mobile,
        bitness: m.bitness,
        wow64: m.wow64,
        form_factors: m.form_factors,
    })
}
