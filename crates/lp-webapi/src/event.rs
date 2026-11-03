//! Event interfaces. One native `EventCore` per event; each realm that sees
//! it gets its own wrapper (isolated worlds), all sharing the state.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use lp_dom::events::{EventState, TargetKey};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope};

use crate::page::PageDom;
use crate::target::{page_of, target_value};
use crate::util::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventInterface {
    Event,
    CustomEvent,
    UiEvent,
    FocusEvent,
    MouseEvent,
    PointerEvent,
    WheelEvent,
    DragEvent,
    KeyboardEvent,
    InputEvent,
    CompositionEvent,
    ProgressEvent,
    ErrorEvent,
    PromiseRejectionEvent,
    SubmitEvent,
    PopStateEvent,
    HashChangeEvent,
    PageTransitionEvent,
    MessageEvent,
    StorageEvent,
    MediaQueryListEvent,
    TouchEvent,
}

/// One touch point (`Touch`): its target and viewport/screen position.
#[derive(Clone, Debug, Default)]
pub struct TouchPoint {
    pub identifier: i64,
    pub target: Option<TargetKey>,
    pub client_x: f64,
    pub client_y: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub rotation_angle: f64,
    pub force: f64,
}

/// `TouchEvent.touches`, `targetTouches` and `changedTouches`.
#[derive(Clone, Debug, Default)]
pub struct TouchLists {
    pub touches: Vec<TouchPoint>,
    pub target_touches: Vec<TouchPoint>,
    pub changed_touches: Vec<TouchPoint>,
}

/// `StorageEvent.storageArea`: the receiving window's own `Storage` (resolved
/// in the observing realm), or one passed to the constructor.
pub enum StorageAreaRef {
    Window { session: bool },
    Object(Persistent),
}

/// Interface-specific fields (flat; unused fields keep defaults).
#[derive(Default)]
pub struct EventFields {
    pub detail: f64,
    pub custom_detail: Option<Persistent>,
    pub screen_x: f64,
    pub screen_y: f64,
    pub client_x: f64,
    pub client_y: f64,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
    pub button: i16,
    pub buttons: u16,
    pub related_target: Option<TargetKey>,
    pub key: String,
    pub code: String,
    pub location: u32,
    pub repeat: bool,
    pub is_composing: bool,
    pub key_code: u32,
    pub char_code: u32,
    pub data: Option<String>,
    pub input_type: String,
    pub pointer_id: i32,
    pub pointer_type: String,
    pub is_primary: bool,
    pub width: f64,
    pub height: f64,
    pub pressure: f64,
    pub delta_x: f64,
    pub delta_y: f64,
    pub delta_z: f64,
    pub delta_mode: u32,
    pub length_computable: bool,
    pub loaded: f64,
    pub total: f64,
    pub message: String,
    pub filename: String,
    pub lineno: u32,
    pub colno: u32,
    pub error: Option<Persistent>,
    pub promise: Option<Persistent>,
    pub reason: Option<Persistent>,
    pub submitter: Option<TargetKey>,
    pub state: Option<Persistent>,
    pub old_url: String,
    pub new_url: String,
    pub persisted: bool,
    pub message_data: Option<Persistent>,
    /// Same-document Window source, resolved in the observing world without
    /// an event-to-global persistent root. Port messages have no source.
    pub message_source: Option<TargetKey>,
    pub origin: String,
    pub(crate) data_transfer: Option<crate::data_transfer::DataTransferRef>,
    pub storage_key: Option<String>,
    pub storage_old: Option<String>,
    pub storage_new: Option<String>,
    pub storage_url: String,
    pub storage_area: Option<StorageAreaRef>,
    /// `MediaQueryListEvent.media` / `.matches`.
    pub media_query: String,
    pub media_matches: bool,
    pub touch: Option<Rc<TouchLists>>,
}

pub struct EventCore {
    pub id: u64,
    pub page: Rc<PageDom>,
    pub iface: EventInterface,
    pub state: RefCell<EventState>,
    pub fields: RefCell<EventFields>,
    held_nodes: RefCell<Vec<crate::collections::NodeHold>>,
}

pub type EventData = Rc<EventCore>;

thread_local! {
    static TIME_ORIGIN: Instant = Instant::now();
}

fn now_ms() -> f64 {
    TIME_ORIGIN.with(|t| t.elapsed().as_secs_f64() * 1000.0)
}

impl EventCore {
    pub fn new(
        page: &Rc<PageDom>,
        iface: EventInterface,
        ty: &str,
        bubbles: bool,
        cancelable: bool,
        composed: bool,
    ) -> EventData {
        let mut st = EventState::new(ty, bubbles, cancelable, composed);
        st.time_stamp = now_ms();
        Rc::new(EventCore {
            id: page.next_event_id(),
            page: page.clone(),
            iface,
            state: RefCell::new(st),
            fields: RefCell::new(EventFields::default()),
            held_nodes: RefCell::new(Vec::new()),
        })
    }

    /// A trusted, browser-generated event.
    pub fn trusted(page: &Rc<PageDom>, iface: EventInterface, ty: &str, bubbles: bool, cancelable: bool) -> EventData {
        let e = Self::new(
            page,
            iface,
            ty,
            bubbles,
            cancelable,
            matches!(
                iface,
                EventInterface::MouseEvent
                    | EventInterface::PointerEvent
                    | EventInterface::KeyboardEvent
                    | EventInterface::InputEvent
                    | EventInterface::FocusEvent
                    | EventInterface::WheelEvent
                    | EventInterface::DragEvent
                    | EventInterface::UiEvent
                    | EventInterface::TouchEvent
            ),
        );
        e.state.borrow_mut().is_trusted = true;
        e
    }

    /// Keep native identities alive as long as the event exposes them.
    pub(crate) fn retain_targets(&self) {
        let fields = self.fields.borrow();
        let keys = [self.state.borrow().target, fields.related_target, fields.submitter];
        let ids: Vec<_> = {
            let dom = self.page.dom.borrow();
            keys.into_iter().flatten().filter_map(|k| dom.key_node(k)).collect()
        };
        drop(fields);
        let mut held = self.held_nodes.borrow_mut();
        held.retain(|h| ids.contains(&h.id));
        for id in ids {
            if !held.iter().any(|h| h.id == id) {
                if let Ok(h) = crate::collections::NodeHold::new(&self.page, id) {
                    held.push(h);
                }
            }
        }
    }

    fn trace_targets(&self, s: &Scope<'_>, wrapper: Object<'_>) -> JsResult<()> {
        let fields = self.fields.borrow();
        let keys = [self.state.borrow().target, fields.related_target, fields.submitter];
        let data_transfer = fields.data_transfer.clone();
        drop(fields);
        if let Some(data_transfer) = data_transfer {
            s.set_hidden(wrapper, "__lpEventDataTransfer", data_transfer.get(s)?.value())?;
        }
        for (name, key) in ["__lpEventTarget", "__lpRelatedTarget", "__lpSubmitter"].into_iter().zip(keys) {
            // Hidden edges preserve wrapper identity/expandos without native GC cycles.
            s.set_hidden(wrapper, name, target_value(s, &self.page, key)?)?;
        }
        Ok(())
    }

    pub(crate) fn refresh_wrappers(&self) {
        self.retain_targets();
        let realms: Vec<_> = self.page.realms.borrow().values().filter_map(|r| r.upgrade()).collect();
        for realm in realms {
            let Ok(s) = realm.scope() else { continue };
            if let Some(wrapper) = s.lookup_identity(self.page.event_identity(self.id)) {
                if let Err(err) = self.trace_targets(&s, wrapper) {
                    s.report_exception(&err);
                }
            }
        }
    }

    pub fn canceled(&self) -> bool {
        self.state.borrow().canceled
    }
}

/// Wraps `core` in `s`'s realm using the class matching its interface.
pub fn wrap_event<'s>(s: &Scope<'s>, core: &EventData) -> JsResult<Object<'s>> {
    core.retain_targets();
    let key = core.page.event_identity(core.id);
    if let Some(o) = s.lookup_identity(key) {
        core.trace_targets(s, o)?;
        return Ok(o);
    }
    let d = core.clone();
    let wrapper = match core.iface {
        EventInterface::Event => s.wrap::<Event>(d, Some(key)),
        EventInterface::CustomEvent => s.wrap::<CustomEvent>(d, Some(key)),
        EventInterface::UiEvent => s.wrap::<UiEvent>(d, Some(key)),
        EventInterface::FocusEvent => s.wrap::<FocusEvent>(d, Some(key)),
        EventInterface::MouseEvent => s.wrap::<MouseEvent>(d, Some(key)),
        EventInterface::PointerEvent => s.wrap::<PointerEvent>(d, Some(key)),
        EventInterface::WheelEvent => s.wrap::<WheelEvent>(d, Some(key)),
        EventInterface::DragEvent => s.wrap::<DragEvent>(d, Some(key)),
        EventInterface::KeyboardEvent => s.wrap::<KeyboardEvent>(d, Some(key)),
        EventInterface::InputEvent => s.wrap::<InputEvent>(d, Some(key)),
        EventInterface::CompositionEvent => s.wrap::<CompositionEvent>(d, Some(key)),
        EventInterface::ProgressEvent => s.wrap::<ProgressEvent>(d, Some(key)),
        EventInterface::ErrorEvent => s.wrap::<ErrorEvent>(d, Some(key)),
        EventInterface::PromiseRejectionEvent => s.wrap::<PromiseRejectionEvent>(d, Some(key)),
        EventInterface::SubmitEvent => s.wrap::<SubmitEvent>(d, Some(key)),
        EventInterface::PopStateEvent => s.wrap::<PopStateEvent>(d, Some(key)),
        EventInterface::HashChangeEvent => s.wrap::<HashChangeEvent>(d, Some(key)),
        EventInterface::PageTransitionEvent => s.wrap::<PageTransitionEvent>(d, Some(key)),
        EventInterface::MessageEvent => s.wrap::<MessageEvent>(d, Some(key)),
        EventInterface::StorageEvent => s.wrap::<StorageEvent>(d, Some(key)),
        EventInterface::MediaQueryListEvent => s.wrap::<MediaQueryListEvent>(d, Some(key)),
        EventInterface::TouchEvent => s.wrap::<TouchEvent>(d, Some(key)),
    }?;
    core.trace_targets(s, wrapper)?;
    Ok(wrapper)
}

fn get_bool(init: Option<Object<'_>>, name: &str) -> JsResult<bool> {
    match init {
        Some(o) => Ok(o.get(name)?.to_bool()),
        None => Ok(false),
    }
}
fn get_num(init: Option<Object<'_>>, name: &str, default: f64) -> JsResult<f64> {
    match init {
        Some(o) => {
            let v = o.get(name)?;
            if v.is_undefined() {
                Ok(default)
            } else {
                v.to_number()
            }
        }
        None => Ok(default),
    }
}
fn get_str(init: Option<Object<'_>>, name: &str) -> JsResult<String> {
    match init {
        Some(o) => {
            let v = o.get(name)?;
            if v.is_undefined() {
                Ok(String::new())
            } else {
                v.to_string()
            }
        }
        None => Ok(String::new()),
    }
}

/// Shared constructor: `new XEvent(type, init)`.
fn construct<'s, C: JsClass<Data = EventData>>(
    s: &Scope<'s>,
    new_target: Object<'s>,
    a: Args<'s>,
    iface: EventInterface,
    fill: fn(&Scope<'s>, &mut EventFields, Option<Object<'s>>) -> JsResult<()>,
) -> JsResult<Object<'s>> {
    let ty = arg_string(&a, 0, C::NAME)?;
    let page = page_of(s)?;
    let init = a.get(1).as_object();
    let core = EventCore::new(
        &page,
        iface,
        &ty,
        get_bool(init, "bubbles")?,
        get_bool(init, "cancelable")?,
        get_bool(init, "composed")?,
    );
    fill(s, &mut core.fields.borrow_mut(), init)?;
    let key = page.event_identity(core.id);
    core.retain_targets();
    let wrapper = s.wrap_new::<C>(core.clone(), new_target, Some(key))?;
    core.trace_targets(s, wrapper)?;
    Ok(wrapper)
}

fn fill_none<'s>(_: &Scope<'s>, _: &mut EventFields, _: Option<Object<'s>>) -> JsResult<()> {
    Ok(())
}

fn fill_ui<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.detail = get_num(init, "detail", 0.0)?;
    Ok(())
}

fn related<'s>(s: &Scope<'s>, init: Option<Object<'s>>) -> JsResult<Option<TargetKey>> {
    let Some(o) = init else { return Ok(None) };
    let v = o.get("relatedTarget")?;
    if let Some(n) = s.unwrap::<crate::node::Node>(v) {
        return Ok(Some(n.target.key));
    }
    Ok(s.unwrap::<crate::target::EventTarget>(v).map(|t| t.key))
}

fn fill_modifiers(f: &mut EventFields, init: Option<Object<'_>>) -> JsResult<()> {
    f.ctrl_key = get_bool(init, "ctrlKey")?;
    f.shift_key = get_bool(init, "shiftKey")?;
    f.alt_key = get_bool(init, "altKey")?;
    f.meta_key = get_bool(init, "metaKey")?;
    Ok(())
}

fn fill_mouse<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_ui(s, f, init)?;
    fill_modifiers(f, init)?;
    f.screen_x = get_num(init, "screenX", 0.0)?;
    f.screen_y = get_num(init, "screenY", 0.0)?;
    f.client_x = get_num(init, "clientX", 0.0)?;
    f.client_y = get_num(init, "clientY", 0.0)?;
    f.button = get_num(init, "button", 0.0)? as i16;
    f.buttons = get_num(init, "buttons", 0.0)? as u16;
    f.related_target = related(s, init)?;
    Ok(())
}

fn fill_pointer<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_mouse(s, f, init)?;
    f.pointer_id = get_num(init, "pointerId", 0.0)? as i32;
    f.width = get_num(init, "width", 1.0)?;
    f.height = get_num(init, "height", 1.0)?;
    f.pressure = get_num(init, "pressure", 0.0)?;
    f.pointer_type = get_str(init, "pointerType")?;
    f.is_primary = get_bool(init, "isPrimary")?;
    Ok(())
}

fn fill_wheel<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_mouse(s, f, init)?;
    f.delta_x = get_num(init, "deltaX", 0.0)?;
    f.delta_y = get_num(init, "deltaY", 0.0)?;
    f.delta_z = get_num(init, "deltaZ", 0.0)?;
    f.delta_mode = get_num(init, "deltaMode", 0.0)? as u32;
    Ok(())
}

fn fill_keyboard<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_ui(s, f, init)?;
    fill_modifiers(f, init)?;
    f.key = get_str(init, "key")?;
    f.code = get_str(init, "code")?;
    f.location = get_num(init, "location", 0.0)? as u32;
    f.repeat = get_bool(init, "repeat")?;
    f.is_composing = get_bool(init, "isComposing")?;
    f.key_code = get_num(init, "keyCode", 0.0)? as u32;
    f.char_code = get_num(init, "charCode", 0.0)? as u32;
    Ok(())
}

fn fill_input<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_ui(s, f, init)?;
    f.data = match init {
        Some(o) => opt_dom_string(o.get("data")?)?,
        None => None,
    };
    f.input_type = get_str(init, "inputType")?;
    f.is_composing = get_bool(init, "isComposing")?;
    Ok(())
}

fn fill_focus<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_ui(s, f, init)?;
    f.related_target = related(s, init)?;
    Ok(())
}

fn fill_custom<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    if let Some(o) = init {
        let d = o.get("detail")?;
        if !d.is_undefined() {
            f.custom_detail = Some(Persistent::new(s, d));
        }
    }
    Ok(())
}

fn fill_progress<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.length_computable = get_bool(init, "lengthComputable")?;
    f.loaded = get_num(init, "loaded", 0.0)?;
    f.total = get_num(init, "total", 0.0)?;
    Ok(())
}

fn fill_error<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.message = get_str(init, "message")?;
    f.filename = get_str(init, "filename")?;
    f.lineno = get_num(init, "lineno", 0.0)? as u32;
    f.colno = get_num(init, "colno", 0.0)? as u32;
    if let Some(o) = init {
        let e = o.get("error")?;
        if !e.is_undefined() {
            f.error = Some(Persistent::new(s, e));
        }
    }
    Ok(())
}

fn fill_storage<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    let Some(o) = init else { return Ok(()) };
    let nullable = |name: &str| -> JsResult<Option<String>> {
        let v = o.get(name)?;
        if v.is_undefined() || v.is_null() {
            Ok(None)
        } else {
            Ok(Some(crate::util::dom_string(v)?))
        }
    };
    f.storage_key = nullable("key")?;
    f.storage_old = nullable("oldValue")?;
    f.storage_new = nullable("newValue")?;
    f.storage_url = get_str(Some(o), "url")?;
    let area = o.get("storageArea")?;
    if !area.is_undefined() && !area.is_null() {
        if s.unwrap::<crate::storage::Storage>(area).is_none() {
            return Err(JsError::Type("StorageEvent: storageArea is not a Storage".into()));
        }
        f.storage_area = Some(StorageAreaRef::Object(Persistent::new(s, area)));
    }
    Ok(())
}

fn fill_popstate<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    if let Some(o) = init {
        f.state = Some(Persistent::new(s, o.get("state")?));
    }
    Ok(())
}

fn fill_hashchange<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.old_url = get_str(init, "oldURL")?;
    f.new_url = get_str(init, "newURL")?;
    Ok(())
}

fn fill_media_query<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.media_query = get_str(init, "media")?;
    f.media_matches = get_bool(init, "matches")?;
    Ok(())
}

fn fill_message<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    if let Some(o) = init {
        f.message_data = Some(Persistent::new(s, o.get("data")?));
    }
    f.origin = get_str(init, "origin")?;
    Ok(())
}

fn fill_pagetransition<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.persisted = get_bool(init, "persisted")?;
    Ok(())
}

fn fill_submit<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    if let Some(o) = init {
        if let Some(n) = s.unwrap::<crate::node::Node>(o.get("submitter")?) {
            f.submitter = Some(n.target.key);
        }
    }
    Ok(())
}

pub struct Event;
impl JsClass for Event {
    const NAME: &'static str = "Event";
    type Data = EventData;
    type Parent = lp_js::NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<Event>(s, nt, a, EventInterface::Event, fill_none));
        def.constant("NONE", 0.0);
        def.constant("CAPTURING_PHASE", 1.0);
        def.constant("AT_TARGET", 2.0);
        def.constant("BUBBLING_PHASE", 3.0);
        def.getter("type", |s, e| Ok(s.string(&e.state.borrow().ty)));
        def.getter("target", |s, e| {
            let k = e.state.borrow().target;
            target_value(s, &e.page, k)
        });
        def.getter("srcElement", |s, e| {
            let k = e.state.borrow().target;
            target_value(s, &e.page, k)
        });
        def.getter("currentTarget", |s, e| {
            let k = e.state.borrow().current_target;
            target_value(s, &e.page, k)
        });
        def.getter("eventPhase", |s, e| Ok(s.number(e.state.borrow().phase as u16 as f64)));
        def.getter("bubbles", |s, e| Ok(s.boolean(e.state.borrow().bubbles)));
        def.getter("cancelable", |s, e| Ok(s.boolean(e.state.borrow().cancelable)));
        def.getter("composed", |s, e| Ok(s.boolean(e.state.borrow().composed)));
        def.getter("defaultPrevented", |s, e| Ok(s.boolean(e.state.borrow().canceled)));
        def.getter("isTrusted", |s, e| Ok(s.boolean(e.state.borrow().is_trusted)));
        def.getter("timeStamp", |s, e| Ok(s.number(e.state.borrow().time_stamp)));
        def.accessor(
            "returnValue",
            |s, e| Ok(s.boolean(!e.state.borrow().canceled)),
            |_s, e, v| {
                if !v.to_bool() {
                    e.state.borrow_mut().prevent_default();
                }
                Ok(())
            },
        );
        def.accessor(
            "cancelBubble",
            |s, e| Ok(s.boolean(e.state.borrow().stop_propagation)),
            |_s, e, v| {
                if v.to_bool() {
                    e.state.borrow_mut().stop_propagation = true;
                }
                Ok(())
            },
        );
        def.method("stopPropagation", 0, |s, e, _a| {
            e.state.borrow_mut().stop_propagation = true;
            Ok(s.undefined())
        });
        def.method("stopImmediatePropagation", 0, |s, e, _a| {
            let mut st = e.state.borrow_mut();
            st.stop_propagation = true;
            st.stop_immediate = true;
            Ok(s.undefined())
        });
        def.method("preventDefault", 0, |s, e, _a| {
            e.state.borrow_mut().prevent_default();
            Ok(s.undefined())
        });
        def.method("composedPath", 0, |s, e, _a| {
            let path = e.state.borrow().path.clone();
            let mut vals = lp_js::RootedVec::new();
            for k in path {
                vals.push(s, target_value(s, &e.page, Some(k))?);
            }
            Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("initEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "Event.initEvent")?;
            let mut st = e.state.borrow_mut();
            if st.dispatching {
                return Ok(s.undefined());
            }
            st.ty = lp_dom::LocalName::from(ty.as_str());
            st.bubbles = a.get(1).to_bool();
            st.cancelable = a.get(2).to_bool();
            st.initialized = true;
            st.stop_propagation = false;
            st.stop_immediate = false;
            st.canceled = false;
            st.is_trusted = false;
            st.target = None;
            drop(st);
            e.refresh_wrappers();
            Ok(s.undefined())
        });
    }
}

pub struct CustomEvent;
impl JsClass for CustomEvent {
    const NAME: &'static str = "CustomEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<CustomEvent>(s, nt, a, EventInterface::CustomEvent, fill_custom));
        def.getter("detail", |s, e| Ok(e.fields.borrow().custom_detail.as_ref().map_or(s.null(), |p| p.get(s))));
        def.method("initCustomEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "CustomEvent.initCustomEvent")?;
            {
                let mut st = e.state.borrow_mut();
                st.ty = lp_dom::LocalName::from(ty.as_str());
                st.bubbles = a.get(1).to_bool();
                st.cancelable = a.get(2).to_bool();
            }
            e.fields.borrow_mut().custom_detail = Some(Persistent::new(s, a.get(3)));
            Ok(s.undefined())
        });
    }
}

pub struct UiEvent;
impl JsClass for UiEvent {
    const NAME: &'static str = "UIEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<UiEvent>(s, nt, a, EventInterface::UiEvent, fill_ui));
        def.getter("detail", |s, e| Ok(s.number(e.fields.borrow().detail)));
        def.getter("view", |s, e| {
            // Trusted UI events carry the window; synthetic ones default to null.
            Ok(if e.state.borrow().is_trusted { s.global().value() } else { s.null() })
        });
        def.getter("which", |s, e| {
            let f = e.fields.borrow();
            Ok(s.number(match e.iface {
                EventInterface::KeyboardEvent => f.key_code as f64,
                EventInterface::MouseEvent
                | EventInterface::PointerEvent
                | EventInterface::WheelEvent
                | EventInterface::DragEvent => (f.button + 1) as f64,
                _ => 0.0,
            }))
        });
    }
}

pub struct FocusEvent;
impl JsClass for FocusEvent {
    const NAME: &'static str = "FocusEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<FocusEvent>(s, nt, a, EventInterface::FocusEvent, fill_focus));
        def.getter("relatedTarget", |s, e| {
            let k = e.fields.borrow().related_target;
            target_value(s, &e.page, k)
        });
    }
}

fn modifier_state(f: &EventFields, key: &str) -> bool {
    match key {
        "Control" => f.ctrl_key,
        "Shift" => f.shift_key,
        "Alt" => f.alt_key,
        "Meta" => f.meta_key,
        _ => false,
    }
}

pub struct MouseEvent;
impl JsClass for MouseEvent {
    const NAME: &'static str = "MouseEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<MouseEvent>(s, nt, a, EventInterface::MouseEvent, fill_mouse));
        def.getter("screenX", |s, e| Ok(s.number(e.fields.borrow().screen_x)));
        def.getter("screenY", |s, e| Ok(s.number(e.fields.borrow().screen_y)));
        def.getter("clientX", |s, e| Ok(s.number(e.fields.borrow().client_x)));
        def.getter("clientY", |s, e| Ok(s.number(e.fields.borrow().client_y)));
        def.getter("x", |s, e| Ok(s.number(e.fields.borrow().client_x)));
        def.getter("y", |s, e| Ok(s.number(e.fields.borrow().client_y)));
        // No scrolling offset model beyond the viewport origin: page == client.
        def.getter("pageX", |s, e| Ok(s.number(e.fields.borrow().client_x)));
        def.getter("pageY", |s, e| Ok(s.number(e.fields.borrow().client_y)));
        def.getter("offsetX", |s, _e| Ok(s.number(0.0)));
        def.getter("offsetY", |s, _e| Ok(s.number(0.0)));
        def.getter("movementX", |s, _e| Ok(s.number(0.0)));
        def.getter("movementY", |s, _e| Ok(s.number(0.0)));
        def.getter("ctrlKey", |s, e| Ok(s.boolean(e.fields.borrow().ctrl_key)));
        def.getter("shiftKey", |s, e| Ok(s.boolean(e.fields.borrow().shift_key)));
        def.getter("altKey", |s, e| Ok(s.boolean(e.fields.borrow().alt_key)));
        def.getter("metaKey", |s, e| Ok(s.boolean(e.fields.borrow().meta_key)));
        def.getter("button", |s, e| Ok(s.number(e.fields.borrow().button as f64)));
        def.getter("buttons", |s, e| Ok(s.number(e.fields.borrow().buttons as f64)));
        def.getter("relatedTarget", |s, e| {
            let k = e.fields.borrow().related_target;
            target_value(s, &e.page, k)
        });
        def.method("getModifierState", 1, |s, e, a| {
            let k = dom_string(a.get(0))?;
            Ok(s.boolean(modifier_state(&e.fields.borrow(), &k)))
        });
    }
}

pub struct PointerEvent;
impl JsClass for PointerEvent {
    const NAME: &'static str = "PointerEvent";
    type Data = EventData;
    type Parent = MouseEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<PointerEvent>(s, nt, a, EventInterface::PointerEvent, fill_pointer));
        def.getter("pointerId", |s, e| Ok(s.number(e.fields.borrow().pointer_id as f64)));
        def.getter("width", |s, e| Ok(s.number(e.fields.borrow().width)));
        def.getter("height", |s, e| Ok(s.number(e.fields.borrow().height)));
        def.getter("pressure", |s, e| Ok(s.number(e.fields.borrow().pressure)));
        def.getter("tangentialPressure", |s, _e| Ok(s.number(0.0)));
        def.getter("tiltX", |s, _e| Ok(s.number(0.0)));
        def.getter("tiltY", |s, _e| Ok(s.number(0.0)));
        def.getter("twist", |s, _e| Ok(s.number(0.0)));
        def.getter("pointerType", |s, e| Ok(s.string(&e.fields.borrow().pointer_type)));
        def.getter("isPrimary", |s, e| Ok(s.boolean(e.fields.borrow().is_primary)));
        def.method("getCoalescedEvents", 0, |s, _e, _a| Ok(s.array(&[])?.value()));
        def.method("getPredictedEvents", 0, |s, _e, _a| Ok(s.array(&[])?.value()));
    }
}

pub struct WheelEvent;
impl JsClass for WheelEvent {
    const NAME: &'static str = "WheelEvent";
    type Data = EventData;
    type Parent = MouseEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<WheelEvent>(s, nt, a, EventInterface::WheelEvent, fill_wheel));
        def.constant("DOM_DELTA_PIXEL", 0.0);
        def.constant("DOM_DELTA_LINE", 1.0);
        def.constant("DOM_DELTA_PAGE", 2.0);
        def.getter("deltaX", |s, e| Ok(s.number(e.fields.borrow().delta_x)));
        def.getter("deltaY", |s, e| Ok(s.number(e.fields.borrow().delta_y)));
        def.getter("deltaZ", |s, e| Ok(s.number(e.fields.borrow().delta_z)));
        def.getter("deltaMode", |s, e| Ok(s.number(e.fields.borrow().delta_mode as f64)));
    }
}

pub struct DragEvent;
impl JsClass for DragEvent {
    const NAME: &'static str = "DragEvent";
    type Data = EventData;
    type Parent = MouseEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            let init = a.get(1).as_object();
            let wrapper = construct::<DragEvent>(s, nt, a, EventInterface::DragEvent, fill_mouse)?;
            // The dictionary getter may reenter JS or force GC. Root only for
            // construction, then keep the value on the event's traced wrapper.
            let _root = Persistent::new(s, wrapper.value());
            let value = match init {
                Some(init) => init.get("dataTransfer")?,
                None => s.null(),
            };
            if !value.is_nullish() {
                let transfer_root = Persistent::new(s, value);
                let transfer = s.unwrap::<crate::data_transfer::DataTransfer>(value).ok_or_else(|| {
                    JsError::Type(
                        "Failed to construct 'DragEvent': member dataTransfer is not of type DataTransfer.".into(),
                    )
                })?;
                let reference = transfer.reference.clone();
                s.set_hidden(wrapper, "__lpEventDataTransfer", transfer_root.get(s))?;
                s.unwrap::<DragEvent>(wrapper.value()).expect("new event").fields.borrow_mut().data_transfer =
                    Some(reference);
            }
            Ok(wrapper)
        });
        def.getter("dataTransfer", |s, e| {
            let reference = e.fields.borrow().data_transfer.clone();
            Ok(match reference {
                Some(reference) => reference.get(s)?.value(),
                None => s.null(),
            })
        });
    }
}

pub struct KeyboardEvent;
impl JsClass for KeyboardEvent {
    const NAME: &'static str = "KeyboardEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<KeyboardEvent>(s, nt, a, EventInterface::KeyboardEvent, fill_keyboard)
        });
        def.constant("DOM_KEY_LOCATION_STANDARD", 0.0);
        def.constant("DOM_KEY_LOCATION_LEFT", 1.0);
        def.constant("DOM_KEY_LOCATION_RIGHT", 2.0);
        def.constant("DOM_KEY_LOCATION_NUMPAD", 3.0);
        def.getter("key", |s, e| Ok(s.string(&e.fields.borrow().key)));
        def.getter("code", |s, e| Ok(s.string(&e.fields.borrow().code)));
        def.getter("location", |s, e| Ok(s.number(e.fields.borrow().location as f64)));
        def.getter("repeat", |s, e| Ok(s.boolean(e.fields.borrow().repeat)));
        def.getter("isComposing", |s, e| Ok(s.boolean(e.fields.borrow().is_composing)));
        def.getter("keyCode", |s, e| Ok(s.number(e.fields.borrow().key_code as f64)));
        def.getter("charCode", |s, e| Ok(s.number(e.fields.borrow().char_code as f64)));
        def.getter("ctrlKey", |s, e| Ok(s.boolean(e.fields.borrow().ctrl_key)));
        def.getter("shiftKey", |s, e| Ok(s.boolean(e.fields.borrow().shift_key)));
        def.getter("altKey", |s, e| Ok(s.boolean(e.fields.borrow().alt_key)));
        def.getter("metaKey", |s, e| Ok(s.boolean(e.fields.borrow().meta_key)));
        def.method("getModifierState", 1, |s, e, a| {
            let k = dom_string(a.get(0))?;
            Ok(s.boolean(modifier_state(&e.fields.borrow(), &k)))
        });
    }
}

pub struct InputEvent;
impl JsClass for InputEvent {
    const NAME: &'static str = "InputEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<InputEvent>(s, nt, a, EventInterface::InputEvent, fill_input));
        def.getter("data", |s, e| Ok(e.fields.borrow().data.as_deref().map_or(s.null(), |d| s.string(d))));
        def.getter("inputType", |s, e| Ok(s.string(&e.fields.borrow().input_type)));
        def.getter("isComposing", |s, e| Ok(s.boolean(e.fields.borrow().is_composing)));
        def.getter("dataTransfer", |s, _e| Ok(s.null()));
        def.method("getTargetRanges", 0, |s, _e, _a| Ok(s.array(&[])?.value()));
    }
}

pub struct CompositionEvent;
impl JsClass for CompositionEvent {
    const NAME: &'static str = "CompositionEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<CompositionEvent>(s, nt, a, EventInterface::CompositionEvent, fill_input)
        });
        def.getter("data", |s, e| Ok(s.string(e.fields.borrow().data.as_deref().unwrap_or(""))));
    }
}

pub struct ProgressEvent;
impl JsClass for ProgressEvent {
    const NAME: &'static str = "ProgressEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<ProgressEvent>(s, nt, a, EventInterface::ProgressEvent, fill_progress)
        });
        def.getter("lengthComputable", |s, e| Ok(s.boolean(e.fields.borrow().length_computable)));
        def.getter("loaded", |s, e| Ok(s.number(e.fields.borrow().loaded)));
        def.getter("total", |s, e| Ok(s.number(e.fields.borrow().total)));
    }
}

pub struct ErrorEvent;
impl JsClass for ErrorEvent {
    const NAME: &'static str = "ErrorEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<ErrorEvent>(s, nt, a, EventInterface::ErrorEvent, fill_error));
        def.getter("message", |s, e| Ok(s.string(&e.fields.borrow().message)));
        def.getter("filename", |s, e| Ok(s.string(&e.fields.borrow().filename)));
        def.getter("lineno", |s, e| Ok(s.number(e.fields.borrow().lineno as f64)));
        def.getter("colno", |s, e| Ok(s.number(e.fields.borrow().colno as f64)));
        def.getter("error", |s, e| Ok(e.fields.borrow().error.as_ref().map_or(s.undefined(), |p| p.get(s))));
    }
}

pub struct PromiseRejectionEvent;
impl JsClass for PromiseRejectionEvent {
    const NAME: &'static str = "PromiseRejectionEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(2, |s, nt, a| {
            construct::<PromiseRejectionEvent>(s, nt, a, EventInterface::PromiseRejectionEvent, fill_promise_rejection)
        });
        def.getter("promise", |s, e| Ok(e.fields.borrow().promise.as_ref().map_or(s.undefined(), |p| p.get(s))));
        def.getter("reason", |s, e| Ok(e.fields.borrow().reason.as_ref().map_or(s.undefined(), |p| p.get(s))));
    }
}

fn fill_promise_rejection<'s>(s: &Scope<'s>, fields: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    let init = init.ok_or_else(|| JsError::Type("PromiseRejectionEvent requires a promise".into()))?;
    let promise = init.get("promise")?;
    if promise.as_object().is_none() {
        return Err(JsError::Type("PromiseRejectionEvent promise must be an object".into()));
    }
    fields.promise = Some(Persistent::new(s, promise));
    fields.reason = Some(Persistent::new(s, init.get("reason")?));
    Ok(())
}

pub(crate) fn dispatch_unhandled_rejection<'s>(
    s: &Scope<'s>,
    promise: lp_js::Value<'s>,
    reason: lp_js::Value<'s>,
) -> JsResult<bool> {
    let page = page_of(s)?;
    if page.is_disposed() {
        return Ok(false);
    }
    let Some(target) = page.window_target() else { return Ok(false) };
    let event = EventCore::trusted(&page, EventInterface::PromiseRejectionEvent, "unhandledrejection", false, true);
    {
        let mut fields = event.fields.borrow_mut();
        fields.promise = Some(Persistent::new(s, promise));
        fields.reason = Some(Persistent::new(s, reason));
    }
    Ok(!crate::target::fire(&page, target, &event))
}

pub struct SubmitEvent;
impl JsClass for SubmitEvent {
    const NAME: &'static str = "SubmitEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<SubmitEvent>(s, nt, a, EventInterface::SubmitEvent, fill_submit));
        def.getter("submitter", |s, e| {
            let k = e.fields.borrow().submitter;
            target_value(s, &e.page, k)
        });
    }
}

pub struct PopStateEvent;
impl JsClass for PopStateEvent {
    const NAME: &'static str = "PopStateEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<PopStateEvent>(s, nt, a, EventInterface::PopStateEvent, fill_popstate)
        });
        def.getter("state", |s, e| Ok(e.fields.borrow().state.as_ref().map_or(s.null(), |p| p.get(s))));
    }
}

fn nullable_string<'s>(s: &Scope<'s>, v: &Option<String>) -> lp_js::Value<'s> {
    v.as_deref().map_or(s.null(), |v| s.string(v))
}

/// `storage` events (HTML Web Storage): the change another same-origin
/// document made to a storage area this window shares.
pub struct StorageEvent;
impl JsClass for StorageEvent {
    const NAME: &'static str = "StorageEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<StorageEvent>(s, nt, a, EventInterface::StorageEvent, fill_storage));
        def.getter("key", |s, e| Ok(nullable_string(s, &e.fields.borrow().storage_key)));
        def.getter("oldValue", |s, e| Ok(nullable_string(s, &e.fields.borrow().storage_old)));
        def.getter("newValue", |s, e| Ok(nullable_string(s, &e.fields.borrow().storage_new)));
        def.getter("url", |s, e| Ok(s.string(&e.fields.borrow().storage_url)));
        def.getter("storageArea", |s, e| {
            let session = match &e.fields.borrow().storage_area {
                None => return Ok(s.null()),
                Some(StorageAreaRef::Object(p)) => return Ok(p.get(s)),
                Some(StorageAreaRef::Window { session }) => *session,
            };
            crate::storage::window_storage(s, &e.page, session)
        });
    }
}

pub struct HashChangeEvent;
impl JsClass for HashChangeEvent {
    const NAME: &'static str = "HashChangeEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<HashChangeEvent>(s, nt, a, EventInterface::HashChangeEvent, fill_hashchange)
        });
        def.getter("oldURL", |s, e| Ok(s.string(&e.fields.borrow().old_url)));
        def.getter("newURL", |s, e| Ok(s.string(&e.fields.borrow().new_url)));
    }
}

/// `change` events of a `MediaQueryList` (its query and whether it matches
/// now; the embedder fires them when an emulation change flips a query).
pub struct MediaQueryListEvent;
impl JsClass for MediaQueryListEvent {
    const NAME: &'static str = "MediaQueryListEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<MediaQueryListEvent>(s, nt, a, EventInterface::MediaQueryListEvent, fill_media_query)
        });
        def.getter("media", |s, e| Ok(s.string(&e.fields.borrow().media_query)));
        def.getter("matches", |s, e| Ok(s.boolean(e.fields.borrow().media_matches)));
    }
}

pub struct PageTransitionEvent;
impl JsClass for PageTransitionEvent {
    const NAME: &'static str = "PageTransitionEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<PageTransitionEvent>(s, nt, a, EventInterface::PageTransitionEvent, fill_pagetransition)
        });
        def.getter("persisted", |s, e| Ok(s.boolean(e.fields.borrow().persisted)));
    }
}

pub struct MessageEvent;
impl JsClass for MessageEvent {
    const NAME: &'static str = "MessageEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<MessageEvent>(s, nt, a, EventInterface::MessageEvent, fill_message));
        def.getter("data", |s, e| Ok(e.fields.borrow().message_data.as_ref().map_or(s.null(), |p| p.get(s))));
        def.getter("origin", |s, e| Ok(s.string(&e.fields.borrow().origin)));
        def.getter("lastEventId", |s, _e| Ok(s.string("")));
        def.getter("source", |s, e| {
            let source = e.fields.borrow().message_source;
            crate::target::target_value(s, &e.page, source)
        });
        def.getter("ports", |s, _e| Ok(s.array(&[])?.value()));
    }
}

/// Registers all event classes on the engine.
pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Event>();
    engine.register::<CustomEvent>();
    engine.register::<UiEvent>();
    engine.register::<FocusEvent>();
    engine.register::<MouseEvent>();
    engine.register::<PointerEvent>();
    engine.register::<WheelEvent>();
    engine.register::<DragEvent>();
    engine.register::<KeyboardEvent>();
    engine.register::<InputEvent>();
    engine.register::<CompositionEvent>();
    engine.register::<ProgressEvent>();
    engine.register::<ErrorEvent>();
    engine.register::<PromiseRejectionEvent>();
    engine.register::<SubmitEvent>();
    engine.register::<PopStateEvent>();
    engine.register::<HashChangeEvent>();
    engine.register::<PageTransitionEvent>();
    engine.register::<MessageEvent>();
    engine.register::<StorageEvent>();
    engine.register::<MediaQueryListEvent>();
}

/// `Touch` and `TouchEvent`, registered after every other class.
pub(crate) fn register_touch(engine: &lp_js::Engine) {
    engine.register::<Touch>();
    engine.register::<TouchEvent>();
}

/// A `Touch` wrapper's state.
pub struct TouchData {
    page: Rc<PageDom>,
    point: TouchPoint,
}

pub struct Touch;
impl JsClass for Touch {
    const NAME: &'static str = "Touch";
    type Data = Rc<TouchData>;
    type Parent = lp_js::NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            let page = page_of(s)?;
            let init = a
                .get(0)
                .as_object()
                .ok_or_else(|| JsError::Type("Failed to construct 'Touch': 1 argument required, but only 0 present.".into()))?;
            let identifier = init.get("identifier")?;
            if identifier.is_undefined() {
                return Err(JsError::Type(
                    "Failed to construct 'Touch': Failed to read the 'identifier' property from 'TouchInit': Required member is undefined.".into(),
                ));
            }
            let target = init.get("target")?;
            if target.is_undefined() {
                return Err(JsError::Type(
                    "Failed to construct 'Touch': Failed to read the 'target' property from 'TouchInit': Required member is undefined.".into(),
                ));
            }
            let target = event_target_key(s, target).ok_or_else(|| {
                JsError::Type("Failed to construct 'Touch': Failed to read the 'target' property from 'TouchInit': Failed to convert value to 'EventTarget'.".into())
            })?;
            let num = |name: &str, default: f64| get_num(Some(init), name, default);
            let point = TouchPoint {
                identifier: identifier.to_number()? as i64,
                target: Some(target),
                client_x: num("clientX", 0.0)?,
                client_y: num("clientY", 0.0)?,
                screen_x: num("screenX", 0.0)?,
                screen_y: num("screenY", 0.0)?,
                radius_x: num("radiusX", 0.0)?,
                radius_y: num("radiusY", 0.0)?,
                rotation_angle: num("rotationAngle", 0.0)?,
                force: num("force", 0.0)?,
            };
            s.wrap_new::<Touch>(Rc::new(TouchData { page, point }), nt, None)
        });
        def.getter("identifier", |s, t| Ok(s.number(t.point.identifier as f64)));
        def.getter("target", |s, t| target_value(s, &t.page, t.point.target));
        def.getter("clientX", |s, t| Ok(s.number(t.point.client_x)));
        def.getter("clientY", |s, t| Ok(s.number(t.point.client_y)));
        def.getter("screenX", |s, t| Ok(s.number(t.point.screen_x)));
        def.getter("screenY", |s, t| Ok(s.number(t.point.screen_y)));
        def.getter("pageX", |s, t| Ok(s.number(t.point.client_x)));
        def.getter("pageY", |s, t| Ok(s.number(t.point.client_y)));
        def.getter("radiusX", |s, t| Ok(s.number(t.point.radius_x)));
        def.getter("radiusY", |s, t| Ok(s.number(t.point.radius_y)));
        def.getter("rotationAngle", |s, t| Ok(s.number(t.point.rotation_angle)));
        def.getter("force", |s, t| Ok(s.number(t.point.force)));
        def.getter("altitudeAngle", |s, _t| Ok(s.number(std::f64::consts::FRAC_PI_2)));
        def.getter("azimuthAngle", |s, _t| Ok(s.number(0.0)));
        def.getter("touchType", |s, _t| Ok(s.string("direct")));
    }
}

fn event_target_key(s: &Scope<'_>, v: lp_js::Value<'_>) -> Option<TargetKey> {
    if let Some(n) = s.unwrap::<crate::node::Node>(v) {
        return Some(n.target.key);
    }
    s.unwrap::<crate::target::EventTarget>(v).map(|t| t.key)
}

/// A `TouchList`: an array of `Touch` wrappers with `item()`.
fn touch_list<'s>(s: &Scope<'s>, page: &Rc<PageDom>, points: &[TouchPoint]) -> JsResult<lp_js::Value<'s>> {
    let mut items = lp_js::RootedVec::new();
    for point in points {
        let touch = s.wrap::<Touch>(Rc::new(TouchData { page: page.clone(), point: point.clone() }), None)?;
        items.push(s, touch.value());
    }
    let list = s.array(&items.iter(s).collect::<Vec<_>>())?;
    let item = s.eval("(function item(i) { return this[i >>> 0] ?? null; })", None, 1)?;
    list.set_with(&lp_js::JsString::new("item"), item, lp_js::sys::kJSPropertyAttributeDontEnum)?;
    Ok(list.value())
}

fn touch_points<'s>(s: &Scope<'s>, init: Option<Object<'s>>, name: &str) -> JsResult<Vec<TouchPoint>> {
    let Some(list) = init.map(|o| o.get(name)).transpose()?.and_then(|v| v.as_object()) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for i in 0..list.get("length")?.to_number()? as u32 {
        let v = list.get_index(i)?;
        let touch = s.unwrap::<Touch>(v).ok_or_else(|| {
            JsError::Type(format!("Failed to construct 'TouchEvent': Failed to read the '{name}' property from 'TouchEventInit': Failed to convert value to 'Touch'."))
        })?;
        out.push(touch.point.clone());
    }
    Ok(out)
}

fn fill_touch<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    fill_ui(s, f, init)?;
    fill_modifiers(f, init)?;
    f.touch = Some(Rc::new(TouchLists {
        touches: touch_points(s, init, "touches")?,
        target_touches: touch_points(s, init, "targetTouches")?,
        changed_touches: touch_points(s, init, "changedTouches")?,
    }));
    Ok(())
}

pub struct TouchEvent;
impl JsClass for TouchEvent {
    const NAME: &'static str = "TouchEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| construct::<TouchEvent>(s, nt, a, EventInterface::TouchEvent, fill_touch));
        def.getter("touches", |s, e| {
            let lists = e.fields.borrow().touch.clone().unwrap_or_default();
            touch_list(s, &e.page, &lists.touches)
        });
        def.getter("targetTouches", |s, e| {
            let lists = e.fields.borrow().touch.clone().unwrap_or_default();
            touch_list(s, &e.page, &lists.target_touches)
        });
        def.getter("changedTouches", |s, e| {
            let lists = e.fields.borrow().touch.clone().unwrap_or_default();
            touch_list(s, &e.page, &lists.changed_touches)
        });
        def.getter("ctrlKey", |s, e| Ok(s.boolean(e.fields.borrow().ctrl_key)));
        def.getter("shiftKey", |s, e| Ok(s.boolean(e.fields.borrow().shift_key)));
        def.getter("altKey", |s, e| Ok(s.boolean(e.fields.borrow().alt_key)));
        def.getter("metaKey", |s, e| Ok(s.boolean(e.fields.borrow().meta_key)));
    }
}
