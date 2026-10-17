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

/// macOS editing commands accepted with a key event, keyed by the shortcut
/// Playwright 1.59 builds (`Shift+Control+Alt+Meta+code`, crInput
/// `_commandsForCode` over playwright-core `macEditingCommands.js`). Only
/// shortcuts whose command the engine's key default action performs for the
/// same key (lp-webapi `default_key_action`) are listed, with the DOM `key`
/// values that action is keyed on. Aliases such as Control+KeyD
/// (deleteForward), Control+KeyH (deleteBackward), Control+KeyB/F/N/P or
/// Alt/Meta+Arrow are absent on purpose: the engine performs nothing for them.
const NATIVE_KEY_COMMANDS: &[(&str, &[&str], &str)] = &[
    ("ArrowLeft", &["ArrowLeft"], "moveLeft"),
    ("ArrowRight", &["ArrowRight"], "moveRight"),
    ("Shift+ArrowLeft", &["ArrowLeft"], "moveLeftAndModifySelection"),
    ("Shift+ArrowRight", &["ArrowRight"], "moveRightAndModifySelection"),
    ("Backspace", &["Backspace"], "deleteBackward"),
    ("Shift+Backspace", &["Backspace"], "deleteBackward"),
    ("Delete", &["Delete"], "deleteForward"),
    ("Shift+Delete", &["Delete"], "deleteForward"),
    ("Meta+KeyA", &["a", "A"], "selectAll"),
];

/// Whether `commands` is exactly the macOS encoding of this key event and the
/// engine's own default action for the key performs it.
fn native_key_commands(code: &str, key: &str, bits: u32, commands: &[String]) -> bool {
    let mut shortcut = String::new();
    // Playwright's order: Shift, Control, Alt, Meta (CDP bits 8, 2, 1, 4).
    for (bit, name) in [(8, "Shift"), (2, "Control"), (1, "Alt"), (4, "Meta")] {
        if bits & bit != 0 {
            shortcut.push_str(name);
            shortcut.push('+');
        }
    }
    shortcut.push_str(code);
    NATIVE_KEY_COMMANDS.iter().any(|(entry, keys, command)| {
        *entry == shortcut && keys.contains(&key) && matches!(commands, [c] if c == command)
    })
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
                // Playwright on macOS attaches the editing commands its table
                // derives from the key (e.g. ArrowRight -> ["moveRight"]). They
                // are accepted only for shortcuts whose command the engine's own
                // key default action performs (upstream ignores commands
                // entirely). Anything else (scrolling, clipboard, undo,
                // word/line movement, Emacs-style Control aliases, or commands
                // not implied by the key) is refused explicitly.
                let derived_from_key =
                    p.commands.is_empty() || native_key_commands(&p.code, &p.key, p.modifiers, &p.commands);
                if !derived_from_key
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
