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

/// macOS editing commands Playwright 1.59 derives from a key event's code and
/// modifiers (`_commandsForCode` over playwright-core `macEditingCommands.js`,
/// Apache-2.0, Copyright Microsoft Corporation), with the `insert*` commands
/// Playwright filters out already removed. Keys are `Shift+Control+Alt+Meta+code`.
const MAC_EDITING_COMMANDS: &[(&str, &[&str])] = &[
    ("Backspace", &["deleteBackward"]),
    ("Escape", &["cancelOperation"]),
    ("ArrowUp", &["moveUp"]),
    ("ArrowDown", &["moveDown"]),
    ("ArrowLeft", &["moveLeft"]),
    ("ArrowRight", &["moveRight"]),
    ("F5", &["complete"]),
    ("Delete", &["deleteForward"]),
    ("Home", &["scrollToBeginningOfDocument"]),
    ("End", &["scrollToEndOfDocument"]),
    ("PageUp", &["scrollPageUp"]),
    ("PageDown", &["scrollPageDown"]),
    ("Shift+Backspace", &["deleteBackward"]),
    ("Shift+Escape", &["cancelOperation"]),
    ("Shift+ArrowUp", &["moveUpAndModifySelection"]),
    ("Shift+ArrowDown", &["moveDownAndModifySelection"]),
    ("Shift+ArrowLeft", &["moveLeftAndModifySelection"]),
    ("Shift+ArrowRight", &["moveRightAndModifySelection"]),
    ("Shift+F5", &["complete"]),
    ("Shift+Delete", &["deleteForward"]),
    ("Shift+Home", &["moveToBeginningOfDocumentAndModifySelection"]),
    ("Shift+End", &["moveToEndOfDocumentAndModifySelection"]),
    ("Shift+PageUp", &["pageUpAndModifySelection"]),
    ("Shift+PageDown", &["pageDownAndModifySelection"]),
    ("Shift+Numpad5", &["delete"]),
    ("Control+Tab", &["selectNextKeyView"]),
    ("Control+KeyA", &["moveToBeginningOfParagraph"]),
    ("Control+KeyB", &["moveBackward"]),
    ("Control+KeyD", &["deleteForward"]),
    ("Control+KeyE", &["moveToEndOfParagraph"]),
    ("Control+KeyF", &["moveForward"]),
    ("Control+KeyH", &["deleteBackward"]),
    ("Control+KeyK", &["deleteToEndOfParagraph"]),
    ("Control+KeyL", &["centerSelectionInVisibleArea"]),
    ("Control+KeyN", &["moveDown"]),
    ("Control+KeyO", &["moveBackward"]),
    ("Control+KeyP", &["moveUp"]),
    ("Control+KeyT", &["transpose"]),
    ("Control+KeyV", &["pageDown"]),
    ("Control+KeyY", &["yank"]),
    ("Control+Backspace", &["deleteBackwardByDecomposingPreviousCharacter"]),
    ("Control+ArrowUp", &["scrollPageUp"]),
    ("Control+ArrowDown", &["scrollPageDown"]),
    ("Control+ArrowLeft", &["moveToLeftEndOfLine"]),
    ("Control+ArrowRight", &["moveToRightEndOfLine"]),
    ("Shift+Control+Tab", &["selectPreviousKeyView"]),
    ("Shift+Control+KeyA", &["moveToBeginningOfParagraphAndModifySelection"]),
    ("Shift+Control+KeyB", &["moveBackwardAndModifySelection"]),
    ("Shift+Control+KeyE", &["moveToEndOfParagraphAndModifySelection"]),
    ("Shift+Control+KeyF", &["moveForwardAndModifySelection"]),
    ("Shift+Control+KeyN", &["moveDownAndModifySelection"]),
    ("Shift+Control+KeyP", &["moveUpAndModifySelection"]),
    ("Shift+Control+KeyV", &["pageDownAndModifySelection"]),
    ("Shift+Control+Backspace", &["deleteBackwardByDecomposingPreviousCharacter"]),
    ("Shift+Control+ArrowUp", &["scrollPageUp"]),
    ("Shift+Control+ArrowDown", &["scrollPageDown"]),
    ("Shift+Control+ArrowLeft", &["moveToLeftEndOfLineAndModifySelection"]),
    ("Shift+Control+ArrowRight", &["moveToRightEndOfLineAndModifySelection"]),
    ("Alt+Backspace", &["deleteWordBackward"]),
    ("Alt+Escape", &["complete"]),
    ("Alt+ArrowUp", &["moveBackward", "moveToBeginningOfParagraph"]),
    ("Alt+ArrowDown", &["moveForward", "moveToEndOfParagraph"]),
    ("Alt+ArrowLeft", &["moveWordLeft"]),
    ("Alt+ArrowRight", &["moveWordRight"]),
    ("Alt+Delete", &["deleteWordForward"]),
    ("Alt+PageUp", &["pageUp"]),
    ("Alt+PageDown", &["pageDown"]),
    ("Shift+Alt+Backspace", &["deleteWordBackward"]),
    ("Shift+Alt+Escape", &["complete"]),
    ("Shift+Alt+ArrowUp", &["moveParagraphBackwardAndModifySelection"]),
    ("Shift+Alt+ArrowDown", &["moveParagraphForwardAndModifySelection"]),
    ("Shift+Alt+ArrowLeft", &["moveWordLeftAndModifySelection"]),
    ("Shift+Alt+ArrowRight", &["moveWordRightAndModifySelection"]),
    ("Shift+Alt+Delete", &["deleteWordForward"]),
    ("Shift+Alt+PageUp", &["pageUp"]),
    ("Shift+Alt+PageDown", &["pageDown"]),
    ("Control+Alt+KeyB", &["moveWordBackward"]),
    ("Control+Alt+KeyF", &["moveWordForward"]),
    ("Control+Alt+Backspace", &["deleteWordBackward"]),
    ("Shift+Control+Alt+KeyB", &["moveWordBackwardAndModifySelection"]),
    ("Shift+Control+Alt+KeyF", &["moveWordForwardAndModifySelection"]),
    ("Shift+Control+Alt+Backspace", &["deleteWordBackward"]),
    ("Meta+NumpadSubtract", &["cancel"]),
    ("Meta+Backspace", &["deleteToBeginningOfLine"]),
    ("Meta+ArrowUp", &["moveToBeginningOfDocument"]),
    ("Meta+ArrowDown", &["moveToEndOfDocument"]),
    ("Meta+ArrowLeft", &["moveToLeftEndOfLine"]),
    ("Meta+ArrowRight", &["moveToRightEndOfLine"]),
    ("Shift+Meta+NumpadSubtract", &["cancel"]),
    ("Shift+Meta+Backspace", &["deleteToBeginningOfLine"]),
    ("Shift+Meta+ArrowUp", &["moveToBeginningOfDocumentAndModifySelection"]),
    ("Shift+Meta+ArrowDown", &["moveToEndOfDocumentAndModifySelection"]),
    ("Shift+Meta+ArrowLeft", &["moveToLeftEndOfLineAndModifySelection"]),
    ("Shift+Meta+ArrowRight", &["moveToRightEndOfLineAndModifySelection"]),
    ("Meta+KeyA", &["selectAll"]),
    ("Meta+KeyC", &["copy"]),
    ("Meta+KeyX", &["cut"]),
    ("Meta+KeyV", &["paste"]),
    ("Meta+KeyZ", &["undo"]),
    ("Shift+Meta+KeyZ", &["redo"]),
];

/// Editing commands whose effect the engine's key default actions perform for
/// the key they are derived from (lp-webapi `default_key_action`): caret
/// movement and selection extension by one character, single-character
/// deletion and select-all in text controls.
const NATIVE_KEY_COMMANDS: &[&str] = &[
    "moveLeft",
    "moveRight",
    "moveLeftAndModifySelection",
    "moveRightAndModifySelection",
    "deleteBackward",
    "deleteForward",
    "selectAll",
];

/// The commands Playwright on macOS sends with this key: the platform
/// encoding of the key itself, not extra actions.
fn mac_commands_for(code: &str, bits: u32) -> &'static [&'static str] {
    let mut shortcut = String::new();
    // Playwright's order: Shift, Control, Alt, Meta (CDP bits 8, 2, 1, 4).
    for (bit, name) in [(8, "Shift"), (2, "Control"), (1, "Alt"), (4, "Meta")] {
        if bits & bit != 0 {
            shortcut.push_str(name);
            shortcut.push('+');
        }
    }
    shortcut.push_str(code);
    MAC_EDITING_COMMANDS.iter().find(|(key, _)| *key == shortcut).map_or(&[], |(_, commands)| commands)
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
                // derives from the key (e.g. ArrowRight -> ["moveRight"]). Those
                // whose effect the engine's own key default action already
                // performs are accepted (upstream ignores commands entirely).
                // Commands the engine would not carry out (scrolling, clipboard,
                // undo, word/line movement, or anything not implied by the key)
                // are refused explicitly.
                let derived_from_key = p.commands.is_empty()
                    || (p
                        .commands
                        .iter()
                        .map(String::as_str)
                        .eq(mac_commands_for(&p.code, p.modifiers).iter().copied())
                        && p.commands.iter().all(|c| NATIVE_KEY_COMMANDS.contains(&c.as_str())));
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
