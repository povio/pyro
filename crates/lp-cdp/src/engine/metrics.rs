//! Device metrics over the browser's persistent page viewport. Screen
//! orientation overrides are stored on the viewport and reported by
//! `screen.orientation` (as upstream and Chrome). Mobile layout and visual
//! viewport transforms remain explicit gaps. `Page.getLayoutMetrics` reports
//! the same viewport, window scroll offset and document scroll size.
use std::collections::BTreeMap;

use lp_dom::media::{self, ScreenOrientation, Viewport};
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
            lp_js::JsError::Range(message) | lp_js::JsError::Type(message) => CdpError::invalid_params(message),
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
                let layout = bindings.layout.borrow();
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
