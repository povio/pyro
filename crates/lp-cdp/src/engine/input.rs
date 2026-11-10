//! CDP input dispatch into the document's native event/default-action path.
use lp_webapi::input::{
    self, DragData, DragItem, DragKind, KeyInput, KeyKind, Modifiers, MouseButton, MouseInput, MouseKind, TouchInput,
    TouchKind,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::rc::Rc;

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
    /// macOS editing commands (Playwright sends them on macOS), run in
    /// place of the key's default action when handled.
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TouchPointParams {
    x: f64,
    y: f64,
    radius_x: Option<f64>,
    radius_y: Option<f64>,
    rotation_angle: Option<f64>,
    force: Option<f64>,
    // Pen properties: range-checked as in Chrome, not reported by Touch.
    tangential_pressure: Option<f64>,
    tilt_x: Option<f64>,
    tilt_y: Option<f64>,
    twist: Option<f64>,
    id: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TouchParams {
    #[serde(rename = "type")]
    kind: String,
    touch_points: Vec<TouchPointParams>,
    #[serde(default)]
    modifiers: u32,
    // `timestamp` is accepted and not used (events carry the page clock's).
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DragItemParams {
    mime_type: String,
    data: String,
    title: Option<String>,
    #[serde(rename = "baseURL")]
    base_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DragDataParams {
    items: Vec<DragItemParams>,
    #[serde(default)]
    files: Vec<String>,
    drag_operations_mask: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DragParams {
    #[serde(rename = "type")]
    kind: String,
    x: f64,
    y: f64,
    data: DragDataParams,
    #[serde(default)]
    modifiers: u32,
}

fn drag_data_json(data: &DragData) -> Value {
    let items: Vec<Value> = data
        .items
        .iter()
        .map(|i| {
            let mut item = json!({"mimeType": i.mime_type, "data": i.data});
            if let Some(title) = &i.title {
                item["title"] = json!(title);
            }
            if let Some(base) = &i.base_url {
                item["baseURL"] = json!(base);
            }
            item
        })
        .collect();
    json!({"data": {"items": items, "dragOperationsMask": data.drag_operations_mask}})
}

fn modifiers(bits: u32) -> CdpResult<Modifiers> {
    if bits > 15 {
        return Err(CdpError::invalid_params("Invalid modifier mask"));
    }
    Ok(Modifiers::from_cdp(bits))
}

impl EngineHost {
    pub(super) fn input_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        let native_id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        if action == "setInterceptDrags" {
            #[derive(Deserialize)]
            struct P {
                enabled: bool,
            }
            let enabled = cmd.command.params::<P>()?.enabled;
            self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.intercept_drags = enabled;
            return Ok(Some(json!({})));
        }
        let page = if matches!(action, "dispatchKeyEvent" | "insertText") {
            self.thread.browser.focused_frame(native_id).and_then(|frame| frame.page_dom)
        } else {
            self.thread.browser.page(native_id).and_then(|page| page.main_frame.page_dom.clone())
        }
        .ok_or_else(|| CdpError::server("input document bindings are unavailable"))?;
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
                // derives from the key (e.g. ArrowRight -> ["moveRight"],
                // Meta+KeyC -> ["copy"], Alt+Backspace ->
                // ["deleteWordBackward"]). The handled ones run in place of
                // the key's default action, as in Chromium; for the others
                // (scrolling, cancelOperation, ...) the key's own default
                // action runs.
                if p.timestamp.is_some()
                    || p.is_system_key
                    || p.native_virtual_key_code != 0
                    || !p.key_identifier.is_empty()
                    || p.unmodified_text.as_ref().is_some_and(|text| *text != p.text)
                {
                    return Err(CdpError::unsupported("Input timestamps, system keys or alternate key metadata"));
                }
                let key = KeyInput {
                    key: p.key,
                    code: p.code,
                    text: p.text,
                    modifiers: modifiers(p.modifiers)?,
                    location: p.location,
                    auto_repeat: p.auto_repeat,
                    key_code: p.windows_virtual_key_code,
                    commands: p.commands,
                };
                // Chrome consumes a pending suppression only for char. A new
                // keydown or keyup clears it; insertText is independent. Do not
                // change sequence state until all command parameters validate.
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                if kind == KeyKind::Char {
                    if std::mem::take(&mut state.suppress_next_char) {
                        return Ok(Some(json!({})));
                    }
                } else {
                    state.suppress_next_char = false;
                }
                input::dispatch_key(&page, kind, &key);
                if kind == KeyKind::RawKeyDown {
                    let same_document = self
                        .thread
                        .browser
                        .focused_frame(native_id)
                        .and_then(|frame| frame.page_dom)
                        .is_some_and(|current| Rc::ptr_eq(&page, &current));
                    if let Some(state) = self.pages.get_mut(&cmd.page) {
                        state.suppress_next_char = !same_document;
                    }
                }
            }
            "dispatchMouseEvent" => {
                let p: MouseParams = cmd.command.params()?;
                let kind = match p.kind.as_str() {
                    "mousePressed" => MouseKind::Pressed,
                    "mouseReleased" => MouseKind::Released,
                    "mouseMoved" => MouseKind::Moved,
                    "mouseWheel" => MouseKind::Wheel,
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
                // A point over an iframe goes to its document, in its own
                // viewport coordinates (recursively); only the innermost
                // document receives the mouse events.
                let (page, x, y) = self.descend_into_frames(native_id, page, p.x, p.y);
                let mouse = MouseInput {
                    x,
                    y,
                    button,
                    buttons: p.buttons,
                    click_count: p.click_count,
                    modifiers: modifiers(p.modifiers)?,
                    delta_x: p.delta_x,
                    delta_y: p.delta_y,
                };
                let intercept = self.pages.get(&cmd.page).is_some_and(|state| state.intercept_drags);
                let dragged = input::dispatch_mouse_intercepting_drags(&page, kind, &mouse, intercept)
                    .map_err(CdpError::server)?;
                if let Some(data) = dragged {
                    self.event("Input.dragIntercepted", &drag_data_json(&data), cmd.session_id);
                }
            }
            "dispatchTouchEvent" => self.dispatch_touch(cmd, native_id, page)?,
            "dispatchDragEvent" => {
                let p: DragParams = cmd.command.params()?;
                let kind = match p.kind.as_str() {
                    "dragEnter" => DragKind::Enter,
                    "dragOver" => DragKind::Over,
                    "drop" => DragKind::Drop,
                    "dragCancel" => DragKind::Cancel,
                    _ => return Err(CdpError::invalid_params("Invalid drag event type")),
                };
                if !p.x.is_finite() || !p.y.is_finite() {
                    return Err(CdpError::invalid_params("Invalid drag coordinates"));
                }
                if !p.data.files.is_empty() {
                    return Err(CdpError::unsupported("Input drag events carrying files"));
                }
                if lp_webapi::layout_api::hit_test(&page, p.x, p.y)
                    .is_some_and(|node| self.thread.browser.frame_of_owner(native_id, node).is_some())
                {
                    return Err(CdpError::unsupported("Input drag targeting a child frame"));
                }
                let data = DragData {
                    items: p
                        .data
                        .items
                        .into_iter()
                        .map(|i| DragItem {
                            mime_type: i.mime_type,
                            data: i.data,
                            title: i.title,
                            base_url: i.base_url,
                        })
                        .collect(),
                    drag_operations_mask: p.data.drag_operations_mask,
                };
                input::dispatch_drag(&page, kind, p.x, p.y, &data, modifiers(p.modifiers)?)
                    .map_err(CdpError::server)?;
            }
            _ => return Err(CdpError::unsupported(&cmd.command.method)),
        }
        Ok(Some(json!({})))
    }
}

impl EngineHost {
    /// Input.dispatchTouchEvent, validated as Chrome does (same messages)
    /// before anything is dispatched. The first touch of a sequence picks
    /// the (innermost frame) document under it; every touch of the sequence
    /// goes to that document.
    fn dispatch_touch(
        &mut self,
        cmd: &PageCommand<'_, '_>,
        native_id: lp_browser::PageId,
        main: Rc<lp_webapi::PageDom>,
    ) -> CdpResult<()> {
        let p: TouchParams = cmd.command.params()?;
        let kind = match p.kind.as_str() {
            "touchStart" => TouchKind::Start,
            "touchMove" => TouchKind::Move,
            "touchEnd" => TouchKind::End,
            "touchCancel" => TouchKind::Cancel,
            other => return Err(CdpError::invalid_params(format!("Unexpected event type '{other}'"))),
        };
        if matches!(kind, TouchKind::Start | TouchKind::Move) && p.touch_points.is_empty() {
            return Err(CdpError::invalid_params("TouchStart and TouchMove must have at least one touch point."));
        }
        if kind == TouchKind::Cancel && !p.touch_points.is_empty() {
            return Err(CdpError::invalid_params("TouchCancel must not have any touch points."));
        }
        let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let active =
            state.touches.is_active() && state.touch_document.as_ref().is_some_and(|(d, _, _)| !d.is_disposed());
        if kind != TouchKind::Start && !active {
            return Err(CdpError::invalid_params("Must send a TouchStart first to start a new touch."));
        }
        for t in &p.touch_points {
            if let Some(id) = t.id.filter(|id| *id < 0.0 || *id >= (i32::MAX - 1) as f64) {
                return Err(CdpError::invalid_params(format!("TouchPoint id '{}' is out of valid range.", id as i64)));
            }
            let outside = |v: Option<f64>, lo: f64, hi: f64| v.is_some_and(|v| !(lo..=hi).contains(&v));
            if outside(t.force, 0.0, 1.0) {
                return Err(CdpError::invalid_params("'force' should be in the range of [0,1]"));
            }
            if outside(t.tangential_pressure, -1.0, 1.0) {
                return Err(CdpError::invalid_params("'tangentialPressure' should be in the range of [-1,1]"));
            }
            if outside(t.tilt_x, -90.0, 90.0) {
                return Err(CdpError::invalid_params("'tiltX' should be in the range of [-90,90]"));
            }
            if outside(t.tilt_y, -90.0, 90.0) {
                return Err(CdpError::invalid_params("'tiltY' should be in the range of [-90,90]"));
            }
            if outside(t.twist, 0.0, 359.0) {
                return Err(CdpError::invalid_params("'twist' should be in the range of [0,359]"));
            }
        }
        let with_id = p.touch_points.iter().filter(|t| t.id.is_some()).count();
        if with_id != 0 && with_id != p.touch_points.len() {
            return Err(CdpError::invalid_params("All or none of the provided TouchPoints must supply ids."));
        }
        let numbers = |t: &TouchPointParams| {
            [Some(t.x), Some(t.y), t.radius_x, t.radius_y, t.rotation_angle, t.force, t.id]
                .into_iter()
                .flatten()
                .all(f64::is_finite)
        };
        if !p.touch_points.iter().all(numbers) {
            return Err(CdpError::invalid_params("Invalid touch point coordinates"));
        }
        let mut points: Vec<TouchInput> = p
            .touch_points
            .iter()
            .enumerate()
            .map(|(i, t)| TouchInput {
                id: t.id.map_or(i as i64, |id| id as i64),
                x: t.x,
                y: t.y,
                radius_x: t.radius_x.unwrap_or(1.0),
                radius_y: t.radius_y.unwrap_or(1.0),
                rotation_angle: t.rotation_angle.unwrap_or(0.0),
                force: t.force.unwrap_or(1.0),
            })
            .collect();
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        if kind == TouchKind::End && points.iter().any(|t| !state.touches.has(t.id)) {
            return Err(CdpError::invalid_params("Must send a TouchStart first to start a new touch."));
        }
        let mut ids: Vec<i64> = if active { state.touches.ids() } else { Vec::new() };
        if matches!(kind, TouchKind::Start | TouchKind::Move) {
            ids.extend(points.iter().map(|t| t.id));
            ids.sort_unstable();
            ids.dedup();
            if ids.len() > 16 {
                return Err(CdpError::server("Exceeded maximum touch points limit of 16"));
            }
        }
        let (document, dx, dy) = match state.touch_document.clone().filter(|_| active) {
            Some(found) => found,
            None => {
                // Touches left down in a document since navigated away from.
                state.touches.clear();
                let first = &points[0];
                let (document, x, y) = self.descend_into_frames(native_id, main, first.x, first.y);
                (document, first.x - x, first.y - y)
            }
        };
        for t in &mut points {
            t.x -= dx;
            t.y -= dy;
        }
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let mut touches = std::mem::take(&mut state.touches);
        let result =
            input::dispatch_touch(&document, &mut touches, kind, &points, Modifiers::from_cdp(p.modifiers & 15));
        if let Some(state) = self.pages.get_mut(&cmd.page) {
            state.touch_document = touches.is_active().then_some((document, dx, dy));
            state.touches = touches;
        }
        result.map_err(CdpError::server)
    }

    /// The innermost frame document under (x, y) and the point in its
    /// viewport coordinates.
    fn descend_into_frames(
        &self,
        page_id: lp_browser::PageId,
        mut page: Rc<lp_webapi::PageDom>,
        mut x: f64,
        mut y: f64,
    ) -> (Rc<lp_webapi::PageDom>, f64, f64) {
        for _ in 0..32 {
            let Some(node) = lp_webapi::layout_api::hit_test(&page, x, y) else { break };
            let Some(frame_id) = self.thread.browser.frame_of_owner(page_id, node) else { break };
            let Some(child) = self.thread.browser.with_frame(page_id, &frame_id, |f| f.page_dom.clone()).flatten()
            else {
                break;
            };
            // The child viewport starts at the iframe's content box.
            let (r, inset) = {
                let layout = page.layout();
                let dom = page.dom.borrow();
                let Some(r) = layout.client_rect(&dom, node) else { break };
                (r, layout.content_offset(&dom, node).unwrap_or_default())
            };
            x -= r.x + inset.0;
            y -= r.y + inset.1;
            page = child;
        }
        (page, x, y)
    }
}
