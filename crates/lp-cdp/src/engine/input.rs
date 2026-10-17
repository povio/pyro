//! CDP input dispatch into the document's native event/default-action path.
use lp_webapi::input::{self, KeyInput, KeyKind, Modifiers, MouseButton, MouseInput, MouseKind};
use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::target::PageCommand;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyParams {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    modifiers: u32,
    #[serde(default)]
    location: u32,
    #[serde(default)]
    windows_virtual_key_code: u32,
    #[serde(default)]
    auto_repeat: bool,
    #[serde(default)]
    commands: Vec<String>,
    timestamp: Option<f64>,
    #[serde(default)]
    is_system_key: bool,
    #[serde(default)]
    is_keypad: bool,
    #[serde(default)]
    native_virtual_key_code: u32,
    #[serde(default)]
    key_identifier: String,
    unmodified_text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MouseParams {
    #[serde(rename = "type")]
    kind: String,
    x: f64,
    y: f64,
    button: Option<String>,
    #[serde(default)]
    buttons: u16,
    #[serde(default)]
    click_count: u32,
    #[serde(default)]
    modifiers: u32,
    #[serde(default)]
    delta_x: f64,
    #[serde(default)]
    delta_y: f64,
    pointer_type: Option<String>,
    force: Option<f64>,
    #[serde(default)]
    tangential_pressure: f64,
    #[serde(default)]
    tilt_x: i32,
    #[serde(default)]
    tilt_y: i32,
    #[serde(default)]
    twist: u32,
    timestamp: Option<f64>,
}

fn modifiers(bits: u32) -> CdpResult<Modifiers> {
    if bits > 15 {
        return Err(CdpError::invalid_params("Invalid modifier mask"));
    }
    Ok(Modifiers::from_cdp(bits))
}

impl EngineHost {
    pub(super) fn input_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let page = self
            .thread
            .browser
            .page(state.id)
            .and_then(|p| p.main_frame.page_dom.clone())
            .ok_or_else(|| CdpError::server("document bindings are unavailable"))?;
        match action {
            "insertText" => {
                #[derive(Deserialize)]
                struct P {
                    text: String,
                }
                let p: P = cmd.command.params()?;
                input::insert_text(&page, &p.text);
            }
            "dispatchKeyEvent" => {
                let mut p: KeyParams = cmd.command.params()?;
                let kind = match p.kind.as_str() {
                    "keyDown" => KeyKind::KeyDown,
                    "rawKeyDown" => KeyKind::RawKeyDown,
                    "char" => KeyKind::Char,
                    "keyUp" => KeyKind::KeyUp,
                    _ => return Err(CdpError::invalid_params("Invalid key event type")),
                };
                if p.location > 3 || p.windows_virtual_key_code > 255 {
                    return Err(CdpError::invalid_params("Invalid key location or virtual code"));
                }
                if p.is_keypad {
                    if p.location != 0 && p.location != 3 {
                        return Err(CdpError::invalid_params("Keypad conflicts with key location"));
                    }
                    p.location = 3;
                }
                if !p.commands.is_empty()
                    || p.timestamp.is_some()
                    || p.is_system_key
                    || p.native_virtual_key_code != 0
                    || !p.key_identifier.is_empty()
                    || p.unmodified_text.as_ref().is_some_and(|text| *text != p.text)
                {
                    return Err(CdpError::unsupported(
                        "Input editing commands, timestamps, system keys or alternate key metadata",
                    ));
                }
                let key = KeyInput {
                    key: p.key,
                    code: p.code,
                    text: p.text,
                    modifiers: modifiers(p.modifiers)?,
                    location: p.location,
                    auto_repeat: p.auto_repeat,
                    key_code: p.windows_virtual_key_code,
                };
                input::dispatch_key(&page, kind, &key);
            }
            "dispatchMouseEvent" => {
                let p: MouseParams = cmd.command.params()?;
                let kind = match p.kind.as_str() {
                    "mousePressed" => MouseKind::Pressed,
                    "mouseReleased" => MouseKind::Released,
                    "mouseMoved" => MouseKind::Moved,
                    "mouseWheel" => return Err(CdpError::unsupported("Input mouse wheel default scrolling")),
                    _ => return Err(CdpError::invalid_params("Invalid mouse event type")),
                };
                let button = match p.button.as_deref().unwrap_or("none") {
                    "none" => MouseButton::None,
                    "left" => MouseButton::Left,
                    "right" => MouseButton::Right,
                    "middle" => MouseButton::Middle,
                    "back" => MouseButton::Back,
                    "forward" => MouseButton::Forward,
                    _ => return Err(CdpError::invalid_params("Invalid mouse button")),
                };
                if p.buttons > 31 || ![p.x, p.y, p.delta_x, p.delta_y].iter().all(|v| v.is_finite()) {
                    return Err(CdpError::invalid_params("Invalid mouse coordinates or button mask"));
                }
                let pressure = if p.buttons == 0 { 0.0 } else { 0.5 };
                if p.pointer_type.as_deref().is_some_and(|v| v != "mouse")
                    || p.force.is_some_and(|v| v != pressure)
                    || p.tangential_pressure != 0.0
                    || p.tilt_x != 0
                    || p.tilt_y != 0
                    || p.twist != 0
                    || p.timestamp.is_some()
                {
                    return Err(CdpError::unsupported("Input pen, custom pressure or timestamp"));
                }
                let mouse = MouseInput {
                    x: p.x,
                    y: p.y,
                    button,
                    buttons: p.buttons,
                    click_count: p.click_count,
                    modifiers: modifiers(p.modifiers)?,
                    delta_x: p.delta_x,
                    delta_y: p.delta_y,
                };
                input::dispatch_mouse(&page, kind, &mouse).map_err(CdpError::server)?;
            }
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        }
        Ok(Some(json!({})))
    }
}
