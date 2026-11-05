//! Event interfaces. One native `EventCore` per event; each realm that sees
//! it gets its own wrapper (isolated worlds), all sharing the state.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use lp_dom::events::{EventState, TargetKey};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

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
    TextEvent,
    BeforeUnloadEvent,
    DeviceMotionEvent,
    DeviceOrientationEvent,
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
    AnimationEvent,
    TransitionEvent,
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
    /// `UIEvent.view` given to a constructor or init method (a Window).
    pub view: Option<Persistent>,
    pub screen_x: f64,
    pub screen_y: f64,
    pub client_x: f64,
    pub client_y: f64,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
    /// EventModifierInit `modifierAltGraph` and the like, by [`EXTRA_MODIFIERS`] bit.
    pub extra_modifiers: u16,
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
    pub tangential_pressure: f64,
    pub tilt_x: f64,
    pub tilt_y: f64,
    pub twist: f64,
    /// `None`: the default, a pen perpendicular to the surface (π/2).
    pub altitude_angle: Option<f64>,
    pub azimuth_angle: f64,
    pub persistent_device_id: f64,
    /// `coalescedEvents` / `predictedEvents` of the init dictionary.
    pub coalesced_events: Vec<Persistent>,
    pub predicted_events: Vec<Persistent>,
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
    /// `AnimationEvent.animationName`.
    pub animation_name: String,
    /// `TransitionEvent.propertyName`.
    pub property_name: String,
    /// `elapsedTime` (seconds) and `pseudoElement` of both.
    pub elapsed_time: f64,
    pub pseudo_element: String,
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

/// `Event.timeStamp` of an event created now: the document's clock, so that
/// it shares `performance.now()`'s time origin and resolution. A page without
/// one (no embedder) counts from the first event of the thread.
fn now_ms(page: &PageDom) -> f64 {
    page.event_time().unwrap_or_else(|| TIME_ORIGIN.with(|t| t.elapsed().as_secs_f64() * 1000.0))
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
        st.time_stamp = now_ms(page);
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
    // An event keeps its object when dispatched into a sibling document.
    if let Some(page) = crate::target::page_of(s).ok().filter(|p| !Rc::ptr_eq(p, &core.page)) {
        for realm in page.sibling_realms(s) {
            let Ok(owner) = realm.scope() else { continue };
            if let Some(o) = owner.lookup_identity(key) {
                core.trace_targets(&owner, o)?;
                if let Some(o) = lp_js::Persistent::new(&owner, o.value()).get(s).as_object() {
                    return Ok(o);
                }
            }
        }
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
        EventInterface::TextEvent => s.wrap::<TextEvent>(d, Some(key)),
        EventInterface::BeforeUnloadEvent => s.wrap::<BeforeUnloadEvent>(d, Some(key)),
        EventInterface::DeviceMotionEvent => s.wrap::<DeviceMotionEvent>(d, Some(key)),
        EventInterface::DeviceOrientationEvent => s.wrap::<DeviceOrientationEvent>(d, Some(key)),
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
        EventInterface::AnimationEvent => s.wrap::<AnimationEvent>(d, Some(key)),
        EventInterface::TransitionEvent => s.wrap::<TransitionEvent>(d, Some(key)),
    }?;
    define_unforgeables(s, &core.page, wrapper)?;
    core.trace_targets(s, wrapper)?;
    Ok(wrapper)
}

/// `isTrusted` is [LegacyUnforgeable]: an own, non-configurable accessor of
/// every event object, with one getter per realm. The realm's installer
/// defines it with the `Object.defineProperty` captured before page script
/// ran.
fn define_unforgeables<'s>(s: &Scope<'s>, page: &PageDom, wrapper: Object<'s>) -> JsResult<()> {
    let realm = s.realm_id();
    let cached = page.event_unforgeables.borrow().get(&realm).map(|p| p.get(s));
    let install = match cached.and_then(|v| v.as_object()) {
        Some(install) => install,
        None => {
            let define = page.define_properties.borrow().get(&realm).map(|p| p.get(s));
            let define = match define {
                Some(define) => define,
                None => match s.global().get_atom("Object")?.as_object() {
                    Some(object) => object.get_atom("defineProperty")?,
                    None => return Ok(()),
                },
            };
            let getter = s.function("get isTrusted", |s, this, _args| {
                let event = s.unwrap::<Event>(this).ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
                let trusted = event.state.borrow().is_trusted;
                Ok(s.boolean(trusted))
            })?;
            let make = s.eval(
                "(function (define, get) { const d = { get, set: undefined, enumerable: true, configurable: false };\n\
                 return function (event) { define(event, 'isTrusted', d); }; })",
                Some("lightningpanda://internal/event-unforgeables.js"),
                1,
            )?;
            let Some(install) = make.as_object().map(|f| f.call(None, &[define, getter.value()])).transpose()? else {
                return Ok(());
            };
            let Some(install) = install.as_object() else { return Ok(()) };
            page.event_unforgeables.borrow_mut().insert(realm, Persistent::new(s, install.value()));
            install
        }
    };
    install.call(None, &[wrapper.value()])?;
    Ok(())
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
    // As in Chrome, only pointer events proper keep fractional coordinates:
    // mouse, wheel and drag events, and the PointerEvents that replaced
    // mouse events (click, auxclick, contextmenu), have integer ones.
    if matches!(iface, EventInterface::MouseEvent | EventInterface::WheelEvent | EventInterface::DragEvent)
        || (iface == EventInterface::PointerEvent && matches!(&*ty, "click" | "auxclick" | "contextmenu"))
    {
        let mut f = core.fields.borrow_mut();
        f.screen_x = f.screen_x.floor();
        f.screen_y = f.screen_y.floor();
        f.client_x = f.client_x.floor();
        f.client_y = f.client_y.floor();
    }
    if matches!(
        iface,
        EventInterface::UiEvent
            | EventInterface::FocusEvent
            | EventInterface::MouseEvent
            | EventInterface::PointerEvent
            | EventInterface::WheelEvent
            | EventInterface::DragEvent
            | EventInterface::KeyboardEvent
            | EventInterface::InputEvent
            | EventInterface::CompositionEvent
            | EventInterface::TouchEvent
    ) {
        core.fields.borrow_mut().view = view_init(s, init)?;
    }
    let key = page.event_identity(core.id);
    core.retain_targets();
    let wrapper = s.wrap_new::<C>(core.clone(), new_target, Some(key))?;
    define_unforgeables(s, &page, wrapper)?;
    core.trace_targets(s, wrapper)?;
    Ok(wrapper)
}

/// `UIEventInit.view`: a Window (this realm's global) or null.
fn view_init<'s>(s: &Scope<'s>, init: Option<Object<'s>>) -> JsResult<Option<Persistent>> {
    let Some(o) = init else { return Ok(None) };
    let v = o.get_atom("view")?;
    view_value(s, v)
}

fn view_value<'s>(s: &Scope<'s>, v: lp_js::Value<'s>) -> JsResult<Option<Persistent>> {
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    // This realm's global, or another Window (its `window` is itself).
    let is_window = v.as_object().is_some_and(|o| {
        o.as_raw() == s.global().as_raw()
            || o.get_atom("window").is_ok_and(|w| w.as_object().is_some_and(|w| w.as_raw() == o.as_raw()))
    });
    if !is_window {
        return Err(JsError::Type("Failed to construct event: member view is not of type Window.".into()));
    }
    Ok(Some(Persistent::new(s, v)))
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
    let v = o.get_atom("relatedTarget")?;
    if let Some(n) = s.unwrap::<crate::node::Node>(v) {
        return Ok(Some(n.target.key));
    }
    Ok(s.unwrap::<crate::target::EventTarget>(v).map(|t| t.key))
}

/// The modifier keys EventModifierInit names beyond ctrl/shift/alt/meta.
const EXTRA_MODIFIERS: &[(&str, &str)] = &[
    ("modifierAltGraph", "AltGraph"),
    ("modifierCapsLock", "CapsLock"),
    ("modifierFn", "Fn"),
    ("modifierFnLock", "FnLock"),
    ("modifierHyper", "Hyper"),
    ("modifierNumLock", "NumLock"),
    ("modifierScrollLock", "ScrollLock"),
    ("modifierSuper", "Super"),
    ("modifierSymbol", "Symbol"),
    ("modifierSymbolLock", "SymbolLock"),
];

fn fill_modifiers(f: &mut EventFields, init: Option<Object<'_>>) -> JsResult<()> {
    f.ctrl_key = get_bool(init, "ctrlKey")?;
    f.shift_key = get_bool(init, "shiftKey")?;
    f.alt_key = get_bool(init, "altKey")?;
    f.meta_key = get_bool(init, "metaKey")?;
    f.extra_modifiers = 0;
    for (bit, (init_name, _)) in EXTRA_MODIFIERS.iter().enumerate() {
        if get_bool(init, init_name)? {
            f.extra_modifiers |= 1 << bit;
        }
    }
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
    f.tangential_pressure = get_num(init, "tangentialPressure", 0.0)?;
    f.twist = get_num(init, "twist", 0.0)?;
    f.persistent_device_id = get_num(init, "persistentDeviceId", 0.0)?;
    // Pointer Events: tilt and the spherical angles are two forms of one
    // orientation; whichever the dictionary gives determines the other.
    let given = |name: &str| -> JsResult<Option<f64>> {
        match init.map(|o| o.get(name)).transpose()? {
            Some(v) if !v.is_undefined() => Ok(Some(v.to_number()?)),
            _ => Ok(None),
        }
    };
    let (tilt_x, tilt_y) = (given("tiltX")?, given("tiltY")?);
    let (altitude, azimuth) = (given("altitudeAngle")?, given("azimuthAngle")?);
    if tilt_x.is_none() && tilt_y.is_none() && (altitude.is_some() || azimuth.is_some()) {
        let (altitude, azimuth) = (altitude.unwrap_or(std::f64::consts::FRAC_PI_2), azimuth.unwrap_or(0.0));
        (f.tilt_x, f.tilt_y) = spherical_to_tilt(altitude, azimuth);
        (f.altitude_angle, f.azimuth_angle) = (Some(altitude), azimuth);
    } else {
        (f.tilt_x, f.tilt_y) = (tilt_x.unwrap_or(0.0), tilt_y.unwrap_or(0.0));
        if altitude.is_none() && azimuth.is_none() {
            let (altitude, azimuth) = tilt_to_spherical(f.tilt_x, f.tilt_y);
            (f.altitude_angle, f.azimuth_angle) = (Some(altitude), azimuth);
        } else {
            (f.altitude_angle, f.azimuth_angle) =
                (Some(altitude.unwrap_or(std::f64::consts::FRAC_PI_2)), azimuth.unwrap_or(0.0));
        }
    }
    for (name, into) in [("coalescedEvents", &mut f.coalesced_events), ("predictedEvents", &mut f.predicted_events)] {
        let Some(list) = init.map(|o| o.get(name)).transpose()?.filter(|v| !v.is_undefined()) else { continue };
        let events = s.sequence(list, name)?;
        let len = events.get_atom("length")?.to_number()? as u32;
        for i in 0..len {
            let event = events.get_index(i)?;
            if s.unwrap::<PointerEvent>(event).is_none() {
                return Err(JsError::Type(format!("Failed to construct 'PointerEvent': {name} must hold PointerEvents.")));
            }
            into.push(Persistent::new(s, event));
        }
    }
    Ok(())
}

/// Pointer Events "converting between tiltX / tiltY and altitudeAngle /
/// azimuthAngle": spherical angles (radians) to tilt (degrees).
fn spherical_to_tilt(altitude: f64, azimuth: f64) -> (f64, f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let (mut x, mut y) = (0.0, 0.0);
    if altitude == 0.0 {
        if azimuth == 0.0 || azimuth == 2.0 * PI {
            x = FRAC_PI_2;
        } else if azimuth == FRAC_PI_2 {
            y = FRAC_PI_2;
        } else if azimuth == PI {
            x = -FRAC_PI_2;
        } else if azimuth == 3.0 * FRAC_PI_2 {
            y = -FRAC_PI_2;
        } else if azimuth > 0.0 && azimuth < FRAC_PI_2 {
            (x, y) = (FRAC_PI_2, FRAC_PI_2);
        } else if azimuth > FRAC_PI_2 && azimuth < PI {
            (x, y) = (-FRAC_PI_2, FRAC_PI_2);
        } else if azimuth > PI && azimuth < 3.0 * FRAC_PI_2 {
            (x, y) = (-FRAC_PI_2, -FRAC_PI_2);
        } else if azimuth > 3.0 * FRAC_PI_2 && azimuth < 2.0 * PI {
            (x, y) = (FRAC_PI_2, -FRAC_PI_2);
        }
    } else {
        let tan_altitude = altitude.tan();
        x = (azimuth.cos() / tan_altitude).atan();
        y = (azimuth.sin() / tan_altitude).atan();
    }
    (x.to_degrees().round(), y.to_degrees().round())
}

/// Tilt (degrees) to (altitudeAngle, azimuthAngle) in radians.
fn tilt_to_spherical(tilt_x: f64, tilt_y: f64) -> (f64, f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let (x, y) = (tilt_x.to_radians(), tilt_y.to_radians());
    let flat = tilt_x.abs() == 90.0 || tilt_y.abs() == 90.0;
    let azimuth = if tilt_x == 0.0 {
        if tilt_y > 0.0 {
            FRAC_PI_2
        } else if tilt_y < 0.0 {
            3.0 * FRAC_PI_2
        } else {
            0.0
        }
    } else if tilt_y == 0.0 {
        if tilt_x < 0.0 {
            PI
        } else {
            0.0
        }
    } else if flat {
        0.0
    } else {
        let a = y.tan().atan2(x.tan());
        if a < 0.0 {
            a + 2.0 * PI
        } else {
            a
        }
    };
    let altitude = if flat {
        0.0
    } else if tilt_x == 0.0 {
        FRAC_PI_2 - y.abs()
    } else if tilt_y == 0.0 {
        FRAC_PI_2 - x.abs()
    } else {
        (1.0 / (x.tan().powi(2) + y.tan().powi(2)).sqrt()).atan()
    };
    (altitude, azimuth)
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
        Some(o) => opt_dom_string(o.get_atom("data")?)?,
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
        let d = o.get_atom("detail")?;
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
        let e = o.get_atom("error")?;
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
    let area = o.get_atom("storageArea")?;
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
        f.state = Some(Persistent::new(s, o.get_atom("state")?));
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
        f.message_data = Some(Persistent::new(s, o.get_atom("data")?));
    }
    f.origin = get_str(init, "origin")?;
    Ok(())
}

fn fill_animation<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.animation_name = get_str(init, "animationName")?;
    f.elapsed_time = get_num(init, "elapsedTime", 0.0)?;
    f.pseudo_element = get_str(init, "pseudoElement")?;
    Ok(())
}

fn fill_transition<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.property_name = get_str(init, "propertyName")?;
    f.elapsed_time = get_num(init, "elapsedTime", 0.0)?;
    f.pseudo_element = get_str(init, "pseudoElement")?;
    Ok(())
}

fn fill_pagetransition<'s>(_: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    f.persisted = get_bool(init, "persisted")?;
    Ok(())
}

fn fill_submit<'s>(s: &Scope<'s>, f: &mut EventFields, init: Option<Object<'s>>) -> JsResult<()> {
    if let Some(o) = init {
        if let Some(n) = s.unwrap::<crate::node::Node>(o.get_atom("submitter")?) {
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
        // `isTrusted` is an own property of each event (`define_unforgeables`).
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
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_event_state(e, &ty, a.get(1).to_bool(), a.get(2).to_bool());
            // `optional any detail = null`.
            let detail = a.get(3);
            e.fields.borrow_mut().custom_detail = (!detail.is_undefined()).then(|| Persistent::new(s, detail));
            e.refresh_wrappers();
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
            if let Some(view) = &e.fields.borrow().view {
                return Ok(view.get(s));
            }
            // Trusted UI events carry the window; synthetic ones default to null.
            Ok(if e.state.borrow().is_trusted { s.global().value() } else { s.null() })
        });
        def.method("initUIEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "UIEvent.initUIEvent")?;
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_ui_event(s, e, &ty, &a)?;
            e.refresh_wrappers();
            Ok(s.undefined())
        });
        // https://w3c.github.io/uievents/#dom-uievent-pseudotarget: the
        // pseudo-element the event targets. Hit testing never resolves to a
        // pseudo-element here, so every UI event reports none.
        def.getter("pseudoTarget", |s, _e| Ok(s.null()));
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

/// The shared `initUIEvent(type, bubbles, cancelable, view, detail)` prefix of
/// the legacy UI event initializers (`a[0..5]`). The caller has checked the
/// dispatch flag.
fn init_ui_event<'s>(s: &Scope<'s>, e: &EventData, ty: &str, a: &Args<'s>) -> JsResult<()> {
    init_event_state(e, ty, a.get(1).to_bool(), a.get(2).to_bool());
    let view = view_value(s, a.get(3))?;
    let mut f = e.fields.borrow_mut();
    f.view = view;
    f.detail = init_num(a, 4)?;
    Ok(())
}

/// An optional numeric initializer argument: missing is 0.
fn init_num<'s>(a: &Args<'s>, i: usize) -> JsResult<f64> {
    let v = a.get(i);
    if v.is_undefined() {
        Ok(0.0)
    } else {
        v.to_number()
    }
}

/// An optional `EventTarget?` initializer argument.
fn init_target<'s>(s: &Scope<'s>, v: lp_js::Value<'s>) -> Option<TargetKey> {
    if let Some(n) = s.unwrap::<crate::node::Node>(v) {
        return Some(n.target.key);
    }
    s.unwrap::<crate::target::EventTarget>(v).map(|t| t.key)
}

/// `initEvent`-style reinitialization of the shared event state.
fn init_event_state(e: &EventData, ty: &str, bubbles: bool, cancelable: bool) {
    let mut st = e.state.borrow_mut();
    st.ty = lp_dom::LocalName::from(ty);
    st.bubbles = bubbles;
    st.cancelable = cancelable;
    st.initialized = true;
    st.stop_propagation = false;
    st.stop_immediate = false;
    st.canceled = false;
    st.is_trusted = false;
    st.target = None;
}

/// Legacy `TextEvent` (`document.createEvent("TextEvent")`): no constructor.
pub struct TextEvent;
impl JsClass for TextEvent {
    const NAME: &'static str = "TextEvent";
    type Data = EventData;
    type Parent = UiEvent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("data", |s, e| Ok(s.string(e.fields.borrow().data.as_deref().unwrap_or(""))));
        def.method("initTextEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "TextEvent.initTextEvent")?;
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_event_state(e, &ty, a.get(1).to_bool(), a.get(2).to_bool());
            let view = view_value(s, a.get(3))?;
            let data = if a.get(4).is_undefined() { "undefined".to_owned() } else { dom_string(a.get(4))? };
            let mut f = e.fields.borrow_mut();
            f.view = view;
            f.data = Some(data);
            drop(f);
            e.refresh_wrappers();
            Ok(s.undefined())
        });
    }
}

pub struct BeforeUnloadEvent;
impl JsClass for BeforeUnloadEvent {
    const NAME: &'static str = "BeforeUnloadEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "returnValue",
            |s, e| Ok(s.string(e.fields.borrow().data.as_deref().unwrap_or(""))),
            |_s, e, v| {
                e.fields.borrow_mut().data = Some(dom_string(v)?);
                Ok(())
            },
        );
    }
}

pub struct DeviceMotionEvent;
impl JsClass for DeviceMotionEvent {
    const NAME: &'static str = "DeviceMotionEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<DeviceMotionEvent>(s, nt, a, EventInterface::DeviceMotionEvent, fill_none)
        });
        def.getter("acceleration", |s, _e| Ok(s.null()));
        def.getter("accelerationIncludingGravity", |s, _e| Ok(s.null()));
        def.getter("rotationRate", |s, _e| Ok(s.null()));
        def.getter("interval", |s, _e| Ok(s.number(0.0)));
    }
}

pub struct DeviceOrientationEvent;
impl JsClass for DeviceOrientationEvent {
    const NAME: &'static str = "DeviceOrientationEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<DeviceOrientationEvent>(s, nt, a, EventInterface::DeviceOrientationEvent, fill_none)
        });
        def.getter("alpha", |s, _e| Ok(s.null()));
        def.getter("beta", |s, _e| Ok(s.null()));
        def.getter("gamma", |s, _e| Ok(s.null()));
        def.getter("absolute", |s, _e| Ok(s.boolean(false)));
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
        _ => EXTRA_MODIFIERS
            .iter()
            .position(|(_, k)| *k == key)
            .is_some_and(|bit| f.extra_modifiers & (1 << bit) != 0),
    }
}

/// CSSOM View `offsetX`/`offsetY`: the event's position relative to the
/// padding edge of its target (the retargeted one a listener sees), or its
/// page position while it has no rendered target element.
fn mouse_offset(e: &EventData) -> (f64, f64) {
    let (x, y) = {
        let f = e.fields.borrow();
        (f.client_x, f.client_y)
    };
    let target = e.state.borrow().target;
    let layout = e.page.layout();
    let dom = e.page.dom.borrow();
    let Some(el) = target.and_then(|key| dom.key_node(key)).filter(|&node| dom.element(node).is_ok()) else {
        return (x, y);
    };
    if !layout.is_visible(&dom, el) {
        return (x, y);
    }
    let rect = layout.bounding_client_rect(&dom, el);
    (x - rect.left() - layout.client_left(&dom, el), y - rect.top() - layout.client_top(&dom, el))
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
        def.getter("offsetX", |s, e| Ok(s.number(mouse_offset(e).0)));
        def.getter("offsetY", |s, e| Ok(s.number(mouse_offset(e).1)));
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
        // https://w3c.github.io/uievents/#idl-mouseevent-initializers
        def.method("initMouseEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "MouseEvent.initMouseEvent")?;
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_ui_event(s, e, &ty, &a)?;
            let related = init_target(s, a.get(14));
            let mut f = e.fields.borrow_mut();
            f.screen_x = init_num(&a, 5)?;
            f.screen_y = init_num(&a, 6)?;
            f.client_x = init_num(&a, 7)?;
            f.client_y = init_num(&a, 8)?;
            f.ctrl_key = a.get(9).to_bool();
            f.alt_key = a.get(10).to_bool();
            f.shift_key = a.get(11).to_bool();
            f.meta_key = a.get(12).to_bool();
            f.button = init_num(&a, 13)? as i16;
            f.related_target = related;
            drop(f);
            e.refresh_wrappers();
            Ok(s.undefined())
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
        def.getter("tangentialPressure", |s, e| Ok(s.number(e.fields.borrow().tangential_pressure)));
        def.getter("tiltX", |s, e| Ok(s.number(e.fields.borrow().tilt_x)));
        def.getter("tiltY", |s, e| Ok(s.number(e.fields.borrow().tilt_y)));
        def.getter("twist", |s, e| Ok(s.number(e.fields.borrow().twist)));
        def.getter("altitudeAngle", |s, e| {
            Ok(s.number(e.fields.borrow().altitude_angle.unwrap_or(std::f64::consts::FRAC_PI_2)))
        });
        def.getter("azimuthAngle", |s, e| Ok(s.number(e.fields.borrow().azimuth_angle)));
        def.getter("persistentDeviceId", |s, e| Ok(s.number(e.fields.borrow().persistent_device_id)));
        def.getter("pointerType", |s, e| Ok(s.string(&e.fields.borrow().pointer_type)));
        def.getter("isPrimary", |s, e| Ok(s.boolean(e.fields.borrow().is_primary)));
        def.method("getCoalescedEvents", 0, |s, e, _a| {
            let events: Vec<Value<'_>> = e.fields.borrow().coalesced_events.iter().map(|p| p.get(s)).collect();
            Ok(s.array(&events)?.value())
        });
        def.method("getPredictedEvents", 0, |s, e, _a| {
            let events: Vec<Value<'_>> = e.fields.borrow().predicted_events.iter().map(|p| p.get(s)).collect();
            Ok(s.array(&events)?.value())
        });
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
                Some(init) => init.get_atom("dataTransfer")?,
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
        // https://w3c.github.io/uievents/#idl-keyboardevent-initializers
        def.method("initKeyboardEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "KeyboardEvent.initKeyboardEvent")?;
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_event_state(e, &ty, a.get(1).to_bool(), a.get(2).to_bool());
            let view = view_value(s, a.get(3))?;
            let key = if a.get(4).is_undefined() { String::new() } else { dom_string(a.get(4))? };
            let mut f = e.fields.borrow_mut();
            f.view = view;
            f.detail = 0.0;
            f.key = key;
            f.location = init_num(&a, 5)? as u32;
            f.ctrl_key = a.get(6).to_bool();
            f.alt_key = a.get(7).to_bool();
            f.shift_key = a.get(8).to_bool();
            f.meta_key = a.get(9).to_bool();
            drop(f);
            e.refresh_wrappers();
            Ok(s.undefined())
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
    let promise = init.get_atom("promise")?;
    if promise.as_object().is_none() {
        return Err(JsError::Type("PromiseRejectionEvent promise must be an object".into()));
    }
    fields.promise = Some(Persistent::new(s, promise));
    fields.reason = Some(Persistent::new(s, init.get_atom("reason")?));
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
        def.method("initStorageEvent", 1, |s, e, a| {
            let ty = arg_string(&a, 0, "StorageEvent.initStorageEvent")?;
            if e.state.borrow().dispatching {
                return Ok(s.undefined());
            }
            init_event_state(e, &ty, a.get(1).to_bool(), a.get(2).to_bool());
            let nullable = |v: lp_js::Value<'_>| -> JsResult<Option<String>> {
                if v.is_undefined() || v.is_null() { Ok(None) } else { Ok(Some(dom_string(v)?)) }
            };
            let area = a.get(7);
            let area = if area.is_undefined() || area.is_null() {
                None
            } else if s.unwrap::<crate::storage::Storage>(area).is_some() {
                Some(StorageAreaRef::Object(Persistent::new(s, area)))
            } else {
                return Err(JsError::Type("parameter 8 is not of type 'Storage'.".into()));
            };
            let mut f = e.fields.borrow_mut();
            f.storage_key = nullable(a.get(3))?;
            f.storage_old = nullable(a.get(4))?;
            f.storage_new = nullable(a.get(5))?;
            f.storage_url = if a.get(6).is_undefined() { String::new() } else { dom_string(a.get(6))? };
            f.storage_area = area;
            drop(f);
            e.refresh_wrappers();
            Ok(s.undefined())
        });
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

/// CSS Animations `AnimationEvent` (`animationstart`, `animationiteration`,
/// `animationend`, `animationcancel`).
pub struct AnimationEvent;
impl JsClass for AnimationEvent {
    const NAME: &'static str = "AnimationEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<AnimationEvent>(s, nt, a, EventInterface::AnimationEvent, fill_animation)
        });
        def.getter("animationName", |s, e| Ok(s.string(&e.fields.borrow().animation_name)));
        def.getter("elapsedTime", |s, e| Ok(s.number(e.fields.borrow().elapsed_time)));
        def.getter("pseudoElement", |s, e| Ok(s.string(&e.fields.borrow().pseudo_element)));
    }
}

/// CSS Transitions `TransitionEvent` (`transitionrun`, `transitionstart`,
/// `transitionend`, `transitioncancel`).
pub struct TransitionEvent;
impl JsClass for TransitionEvent {
    const NAME: &'static str = "TransitionEvent";
    type Data = EventData;
    type Parent = Event;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            construct::<TransitionEvent>(s, nt, a, EventInterface::TransitionEvent, fill_transition)
        });
        def.getter("propertyName", |s, e| Ok(s.string(&e.fields.borrow().property_name)));
        def.getter("elapsedTime", |s, e| Ok(s.number(e.fields.borrow().elapsed_time)));
        def.getter("pseudoElement", |s, e| Ok(s.string(&e.fields.borrow().pseudo_element)));
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
/// The events a worker's realm exposes.
pub(crate) fn register_for_worker(engine: &lp_js::Engine) {
    engine.register::<Event>();
    engine.register::<CustomEvent>();
    engine.register::<ProgressEvent>();
    engine.register::<ErrorEvent>();
    engine.register::<PromiseRejectionEvent>();
    engine.register::<MessageEvent>();
}

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
    engine.register::<TextEvent>();
    engine.register::<BeforeUnloadEvent>();
    engine.register::<DeviceMotionEvent>();
    engine.register::<DeviceOrientationEvent>();
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
    engine.register::<Touch>();
    engine.register::<TouchEvent>();
    engine.register::<AnimationEvent>();
    engine.register::<TransitionEvent>();
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
            let identifier = init.get_atom("identifier")?;
            if identifier.is_undefined() {
                return Err(JsError::Type(
                    "Failed to construct 'Touch': Failed to read the 'identifier' property from 'TouchInit': Required member is undefined.".into(),
                ));
            }
            let target = init.get_atom("target")?;
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
    for i in 0..list.get_atom("length")?.to_number()? as u32 {
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
