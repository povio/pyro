//! Generic Sensor API (https://w3c.github.io/sensors/) over virtual sensors, and the sensor-backed events of
//! Device Orientation and Motion (https://w3c.github.io/deviceorientation/).
//!
//! A headless desktop browser has no motion or light sensors. What a page sees without a virtual sensor is what
//! Chrome shows on a machine without them: `new Accelerometer().start()` fires `error` (NotReadableError), and
//! `deviceorientation` / `devicemotion` fire once with null values. A *virtual sensor* is the WebDriver automation
//! section's data source (`create`, `update`, `remove`, `get information`; CDP `Emulation.setSensorOverrideEnabled`,
//! `setSensorOverrideReadings`, `getOverriddenSensorInformation`): readings it is given feed the same pipeline, so a
//! page cannot tell them from a platform sensor's.
//!
//! The [`SensorHub`] of a browser context holds the virtual sensors: their metadata (`connected`, the sampling
//! frequency limits), the latest reading each platform sensor delivered (rounded as its specification says, the
//! ambient light threshold applied), and the connected clients with the frequency each requested. The `Sensor`
//! classes are made in JS (sensors.js) and talk to the hub through [`natives`]; the events are fired from here by
//! the *pump* of a document and event type, which starts when a listener of the type is added to the window.
//!
//! A reading reaches a document only if it is fully active and visible and its origin is that of the focused
//! document (Generic Sensor "focused area"): a withheld reading is delivered when that changes
//! ([`state_changed`]).

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Instant;

use pyro_dom::events::TargetKey;
use pyro_js::{JsError, JsResult, Object, Persistent, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::event::{EventCore, EventInterface};
use crate::lazy_module::LazyModule;
use crate::navigator_extras::{ContextServices, PermissionSetting};
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpSensors",
    source: include_str!("sensors.js"),
    url: "pyro://internal/sensors.js",
    natives: Some(natives),
};

/// The virtual sensor types (WebDriver `Virtual Sensor Type`; the same names Chrome's `Emulation.SensorType` has).
const TYPES: &[&str] = &[
    "absolute-orientation",
    "accelerometer",
    "ambient-light",
    "gravity",
    "gyroscope",
    "linear-acceleration",
    "magnetometer",
    "relative-orientation",
];

/// The frequency (Hz) a sensor runs at when its client asked for none, and the limits a virtual sensor has
/// when its metadata gives none.
const DEFAULT_FREQUENCY: f64 = 5.0;
const DEFAULT_MIN_FREQUENCY: f64 = 1.0;
const DEFAULT_MAX_FREQUENCY: f64 = 60.0;
/// The rate the event pumps ask their sensors for.
const PUMP_FREQUENCY: f64 = 60.0;
/// `DeviceMotionEvent.interval`: the pump period in milliseconds (Chrome's 16 ms).
const MOTION_INTERVAL: f64 = 16.0;

/// Ambient Light Sensor: readings are rounded to a multiple of 50 lux, and a reading is dropped unless it is at
/// least 25 lux from the last one accepted.
const ILLUMINANCE_ROUNDING: f64 = 50.0;
const ILLUMINANCE_THRESHOLD: f64 = 25.0;

fn canonical(ty: &str) -> Option<&'static str> {
    TYPES.iter().copied().find(|t| *t == ty)
}

/// A reading as the CDP `Emulation.SensorReading` carries it, in the units WebDriver and Chrome use (m/s^2,
/// rad/s, µT, lux; a unit quaternion `x y z w`).
#[derive(Clone, Debug, PartialEq)]
pub enum SensorReading {
    Single(f64),
    Xyz([f64; 3]),
    Quaternion([f64; 4]),
}

impl SensorReading {
    fn finite(&self) -> bool {
        match self {
            SensorReading::Single(v) => v.is_finite(),
            SensorReading::Xyz(v) => v.iter().all(|x| x.is_finite()),
            SensorReading::Quaternion(v) => v.iter().all(|x| x.is_finite()),
        }
    }
}

/// Rounds to a multiple of `step` the way `Math.round(v / step) * step` does.
fn round_to(v: f64, step: f64) -> f64 {
    (v / step + 0.5).floor() * step
}

/// A reading brought to what the platform sensor reports: the values rounded as the sensor's specification says
/// (accelerations to 0.1 m/s^2, rotation rates to 0.1 deg/s, illuminance to 50 lux), and for the ambient light
/// sensor also the raw value its significance check compares. `None`: the reading is not of this type's kind.
fn process(ty: &str, reading: &SensorReading) -> Option<(Vec<f64>, Option<f64>)> {
    Some(match (ty, reading) {
        ("accelerometer" | "gravity" | "linear-acceleration", SensorReading::Xyz(v)) => {
            (v.iter().map(|x| round_to(*x, 0.1)).collect(), None)
        }
        ("gyroscope", SensorReading::Xyz(v)) => {
            (v.iter().map(|x| round_to(x.to_degrees(), 0.1).to_radians()).collect(), None)
        }
        ("magnetometer", SensorReading::Xyz(v)) => (v.to_vec(), None),
        ("ambient-light", SensorReading::Single(v)) => (vec![round_to(*v, ILLUMINANCE_ROUNDING)], Some(*v)),
        ("absolute-orientation" | "relative-orientation", SensorReading::Quaternion(q)) => (q.to_vec(), None),
        _ => return None,
    })
}

struct VirtualSensor {
    /// WebDriver `connected`: false makes the platform sensor unavailable (`start()` fails with NotReadableError).
    connected: bool,
    min_frequency: f64,
    max_frequency: f64,
    /// The latest reading the platform sensor accepted (processed), when it was made, and a counter that changes
    /// with every accepted reading.
    values: Option<Vec<f64>>,
    at: Option<Instant>,
    seq: u64,
    /// Ambient light: the raw illuminance last accepted.
    raw: Option<f64>,
    /// Connected clients: id and the frequency each asked for, within the limits.
    clients: Vec<(u64, f64)>,
}

/// The virtual sensors of a browser context, and the documents that look at them.
#[derive(Default)]
pub(crate) struct SensorHub {
    sensors: FxHashMap<&'static str, VirtualSensor>,
    next_client: u64,
    pages: Vec<Weak<PageDom>>,
}

impl SensorHub {
    fn create(&mut self, ty: &str, available: bool, min: Option<f64>, max: Option<f64>) -> Result<(), String> {
        let ty = canonical(ty).ok_or_else(|| format!("Invalid sensor type: {ty}"))?;
        if self.sensors.contains_key(ty) {
            return Err(format!("The {ty} sensor is already overridden"));
        }
        if min.is_some_and(|m| !m.is_finite() || m < 0.0) || max.is_some_and(|m| !m.is_finite() || m < 0.0) {
            return Err("Sampling frequencies must be finite and not negative".into());
        }
        let max_frequency = max.unwrap_or(DEFAULT_MAX_FREQUENCY);
        let min_frequency = min.unwrap_or(DEFAULT_MIN_FREQUENCY).min(max_frequency);
        if min.is_some_and(|m| m > max_frequency) {
            return Err("The minimum sampling frequency is above the maximum".into());
        }
        self.sensors.insert(
            ty,
            VirtualSensor {
                connected: available,
                min_frequency,
                max_frequency,
                values: None,
                at: None,
                seq: 0,
                raw: None,
                clients: Vec::new(),
            },
        );
        Ok(())
    }

    fn remove(&mut self, ty: &str) {
        if let Some(ty) = canonical(ty) {
            self.sensors.remove(ty);
        }
    }

    /// WebDriver "update virtual sensor reading": the platform sensor's "update latest reading". Whether a
    /// reading was accepted (clients are told).
    fn update(&mut self, ty: &str, reading: &SensorReading) -> Result<bool, String> {
        let ty = canonical(ty).ok_or_else(|| format!("Invalid sensor type: {ty}"))?;
        if !reading.finite() {
            return Err("A reading must be made of finite numbers".into());
        }
        let sensor = self.sensors.get_mut(ty).ok_or_else(|| format!("The {ty} sensor is not overridden"))?;
        let (values, raw) = process(ty, reading).ok_or_else(|| format!("The reading does not fit the {ty} sensor"))?;
        // A sensor that is not connected has no platform sensor to receive it.
        if !sensor.connected {
            return Ok(false);
        }
        // With no client the reading is only kept, to be the first a client gets; there is nothing to compare it
        // with then. Otherwise the ambient light sensor drops an insignificant change and one that rounds to the
        // reading it already has.
        if !sensor.clients.is_empty() && ty == "ambient-light" {
            if let (Some(old), Some(new)) = (sensor.raw, raw) {
                if (new - old).abs() < ILLUMINANCE_THRESHOLD {
                    return Ok(false);
                }
            }
            sensor.raw = raw;
            if sensor.values.as_ref() == Some(&values) {
                return Ok(false);
            }
        }
        sensor.raw = raw;
        sensor.values = Some(values);
        sensor.at = Some(Instant::now());
        sensor.seq += 1;
        Ok(true)
    }

    fn is_connected(&self, ty: &str) -> bool {
        self.sensors.get(ty).is_some_and(|s| s.connected)
    }

    /// A client starts using the sensor, asking for `frequency` (clamped to the limits). The `Err` is the
    /// DOMException name `start()` fails with.
    fn connect(&mut self, ty: &str, frequency: Option<f64>) -> Result<u64, &'static str> {
        let sensor = self.sensors.get_mut(ty).filter(|s| s.connected).ok_or("NotReadableError")?;
        self.next_client += 1;
        let id = self.next_client;
        let asked = frequency.unwrap_or(DEFAULT_FREQUENCY);
        // `max` first: a limit pair of 2..1 (a minimum above the maximum) cannot panic `clamp`.
        sensor.clients.push((id, asked.min(sensor.max_frequency).max(sensor.min_frequency)));
        Ok(id)
    }

    fn disconnect(&mut self, ty: &str, id: u64) {
        if let Some(sensor) = self.sensors.get_mut(ty) {
            sensor.clients.retain(|(client, _)| *client != id);
        }
    }

    /// The sampling frequency the platform sensor runs at: the fastest any client asked for (0 with none).
    fn requested_frequency(&self, ty: &str) -> Option<f64> {
        let sensor = self.sensors.get(ty)?;
        Some(sensor.clients.iter().map(|(_, f)| *f).fold(0.0, f64::max))
    }

    fn read(&self, ty: &str) -> Option<(u64, Instant, Vec<f64>)> {
        let sensor = self.sensors.get(ty)?;
        Some((sensor.seq, sensor.at?, sensor.values.clone()?))
    }

    fn register(&mut self, page: &Rc<PageDom>) {
        self.pages.retain(|p| p.upgrade().is_some_and(|p| !p.is_disposed()));
        if !self.pages.iter().any(|p| std::ptr::eq(p.as_ptr(), Rc::as_ptr(page))) {
            self.pages.push(Rc::downgrade(page));
        }
    }
}

// ---- WebDriver / CDP side

impl ContextServices {
    /// Creates (`enabled`) the virtual sensor `ty` with its metadata (`Emulation.setSensorOverrideEnabled`).
    pub fn enable_sensor(&self, ty: &str, available: bool, min: Option<f64>, max: Option<f64>) -> Result<(), String> {
        self.sensors.borrow_mut().create(ty, available, min, max)
    }

    /// Removes the virtual sensor `ty`. A client connected to it keeps no reading and gets none; removing a
    /// sensor that is not there changes nothing.
    pub fn disable_sensor(&self, ty: &str) {
        self.sensors.borrow_mut().remove(ty);
    }

    /// Reports a reading through the virtual sensor `ty` (`Emulation.setSensorOverrideReadings`).
    pub fn set_sensor_reading(&self, ty: &str, reading: &SensorReading) -> Result<(), String> {
        let accepted = self.sensors.borrow_mut().update(ty, reading)?;
        if accepted {
            if let Some(ty) = canonical(ty) {
                notify(self, ty);
            }
        }
        Ok(())
    }

    /// The sampling frequency requested of virtual sensor `ty` (`Emulation.getOverriddenSensorInformation`).
    pub fn sensor_requested_frequency(&self, ty: &str) -> Result<f64, String> {
        self.sensors.borrow().requested_frequency(ty).ok_or_else(|| format!("The {ty} sensor is not overridden"))
    }
}

// ---- documents

thread_local! {
    /// The realms that look at sensors, to be told of a change: the callback is the JS module's.
    static WATCHERS: RefCell<Vec<Watcher>> = RefCell::new(Vec::new());
    static PUMPS: RefCell<Vec<Pump>> = RefCell::new(Vec::new());
}

/// This module's part of [`crate::thread_state::reset_thread`]: the watchers and pumps of dropped
/// pages go; one still held names the static.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|watcher| watcher.page.strong_count() > 0);
        !w.is_empty()
    }) {
        return Some("sensors::WATCHERS");
    }
    PUMPS
        .with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|pump| pump.page.strong_count() > 0);
            !p.is_empty()
        })
        .then_some("sensors::PUMPS")
}

struct Watcher {
    page: Weak<PageDom>,
    realm: WeakRealm,
    callback: Persistent,
}

/// What a sensor-backed event type of a document has connected.
struct Pump {
    page: Weak<PageDom>,
    ty: &'static str,
    clients: Vec<(&'static str, u64)>,
    /// The readings (sensor, counter) the last event was made from.
    sent: Vec<(&'static str, u64)>,
    /// The event with null values, fired once when no sensor can provide data.
    null_sent: bool,
}

const EVENTS: [&str; 3] = ["devicemotion", "deviceorientation", "deviceorientationabsolute"];

/// The sensors an event type reads, the first available being used for the orientation events (relative
/// orientation falls back to the absolute one).
fn sources(ty: &str) -> &'static [&'static str] {
    match ty {
        "devicemotion" => &["accelerometer", "linear-acceleration", "gyroscope"],
        "deviceorientation" => &["relative-orientation", "absolute-orientation"],
        _ => &["absolute-orientation"],
    }
}

/// The permissions a sensor needs (Permissions "powerful feature" names).
fn permissions(sensor: &str) -> &'static [&'static str] {
    match sensor {
        "accelerometer" | "gravity" | "linear-acceleration" => &["accelerometer"],
        "gyroscope" => &["gyroscope"],
        "magnetometer" => &["magnetometer"],
        "relative-orientation" => &["accelerometer", "gyroscope"],
        "absolute-orientation" => &["accelerometer", "gyroscope", "magnetometer"],
        _ => &["ambient-light-sensor"],
    }
}

fn permitted(page: &PageDom, sensor: &str) -> bool {
    permissions(sensor).iter().all(|name| crate::navigator_extras::permission_state(page, name) != PermissionSetting::Denied)
}

fn same_page(weak: &Weak<PageDom>, page: &Rc<PageDom>) -> bool {
    std::ptr::eq(weak.as_ptr(), Rc::as_ptr(page))
}

/// Generic Sensor "focused area" and visibility: whether readings may reach this document now. It must be fully
/// active and visible, and be the focused document or have its origin.
fn may_deliver(page: &PageDom) -> bool {
    if page.is_hidden() || !page.is_fully_active() {
        return false;
    }
    let Some(hooks) = page.hooks() else { return true };
    if hooks.is_focused() {
        return true;
    }
    let origin = page.origin_key();
    hooks.frame_tree().iter().find(|other| other.hooks().is_some_and(|h| h.is_focused())).is_some_and(|focused| focused.origin_key() == origin)
}

/// The document's `Sensor`s and event pumps look again at what they may deliver: after a reading was accepted
/// (`ty`), or when focus or visibility changed (`""`).
fn notify(services: &ContextServices, ty: &'static str) {
    // What belonged to documents that are gone (and the values it protects) goes.
    let live = |page: &Weak<PageDom>| page.upgrade().is_some_and(|p| !p.is_disposed());
    WATCHERS.with(|w| w.borrow_mut().retain(|w| live(&w.page)));
    PUMPS.with(|p| p.borrow_mut().retain(|p| live(&p.page)));
    let pages: Vec<Rc<PageDom>> = {
        let mut hub = services.sensors.borrow_mut();
        hub.pages.retain(|p| p.upgrade().is_some_and(|p| !p.is_disposed()));
        hub.pages.iter().filter_map(Weak::upgrade).collect()
    };
    for page in pages {
        let weak = Rc::downgrade(&page);
        page.post_task(Box::new(move || {
            if let Some(page) = weak.upgrade() {
                run(&page, ty);
            }
        }));
    }
}

/// Focus or the visibility state of a document changed: readings withheld from it are delivered, and readings
/// it may no longer get are held back. Called by the embedder's focus and visibility changes.
pub fn state_changed(page: &PageDom) {
    notify(&page.context_services(), "");
}

fn run(page: &Rc<PageDom>, ty: &'static str) {
    if page.is_disposed() {
        return;
    }
    let watchers: Vec<(WeakRealm, Persistent)> = WATCHERS.with(|w| {
        w.borrow().iter().filter(|w| same_page(&w.page, page)).map(|w| (w.realm.clone(), w.callback.clone())).collect()
    });
    for (realm, callback) in watchers {
        let Some(realm) = realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        if let Some(f) = callback.get(&s).as_object() {
            if let Err(e) = f.call(None, &[s.string(ty)]) {
                s.report_exception(&e);
            }
        }
    }
    for event in EVENTS {
        pump_deliver(page, event, false, ty);
    }
}

// ---- the event pumps

/// A listener was added to `key`: for the window of a secure context and a sensor-backed event type, the pump
/// (re)starts: the sensors are connected and the current reading is delivered in a task of its own (an event is
/// never fired from `addEventListener`).
pub(crate) fn listener_added(s: &Scope<'_>, page: &Rc<PageDom>, key: TargetKey, ty: &str) {
    let Some(ty) = EVENTS.iter().copied().find(|e| *e == ty) else { return };
    if page.is_disposed() || !page.is_window_key(key) || !crate::secure_context::secure(s, page) {
        return;
    }
    // Permissions Policy: the window's events need the features of the sensors behind them (Chrome checks
    // accelerometer and gyroscope for all three, and magnetometer for the absolute orientation) and a refused
    // document gets no events at all. The check reports the violation.
    let features: &[&str] = if ty == "deviceorientationabsolute" { &["accelerometer", "gyroscope", "magnetometer"] } else { &["accelerometer", "gyroscope"] };
    if !features.iter().all(|feature| crate::permissions_policy::check_feature(s, page, feature)) {
        return;
    }
    // A pump that is running already serves the listeners the window has: the new one hears the next reading
    // with them, and the listeners that saw the current one are not given it again (a listener added from an
    // event handler of the pump included).
    if PUMPS.with(|p| p.borrow().iter().any(|x| x.ty == ty && same_page(&x.page, page))) {
        return;
    }
    start_pump(page, ty);
}

fn take_pump(page: &Rc<PageDom>, ty: &str) -> Option<Pump> {
    PUMPS.with(|p| {
        let mut p = p.borrow_mut();
        let at = p.iter().position(|x| x.ty == ty && same_page(&x.page, page))?;
        Some(p.remove(at))
    })
}

fn start_pump(page: &Rc<PageDom>, ty: &'static str) {
    stop_pump(page, ty);
    let services = page.context_services();
    let mut clients = Vec::new();
    {
        let mut hub = services.sensors.borrow_mut();
        hub.register(page);
        for sensor in sources(ty) {
            if hub.is_connected(sensor) && permitted(page, sensor) {
                if let Ok(id) = hub.connect(sensor, Some(PUMP_FREQUENCY)) {
                    clients.push((*sensor, id));
                }
            }
        }
    }
    PUMPS.with(|p| p.borrow_mut().push(Pump { page: Rc::downgrade(page), ty, clients, sent: Vec::new(), null_sent: false }));
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        if let Some(page) = weak.upgrade() {
            pump_deliver(&page, ty, true, "");
        }
    }));
}

fn stop_pump(page: &Rc<PageDom>, ty: &str) {
    if let Some(pump) = take_pump(page, ty) {
        let services = page.context_services();
        let mut hub = services.sensors.borrow_mut();
        for (sensor, id) in pump.clients {
            hub.disconnect(sensor, id);
        }
    }
}

/// Fires the event of `ty` at the window if the pump has something new to say: `changed` is the sensor whose
/// reading was accepted (`""`: any), `force` delivers the current readings even if the last event was made from
/// them (a listener was just added).
fn pump_deliver(page: &Rc<PageDom>, ty: &'static str, force: bool, changed: &str) {
    if !PUMPS.with(|p| p.borrow().iter().any(|x| x.ty == ty && same_page(&x.page, page))) {
        return;
    }
    if !changed.is_empty() && !sources(ty).iter().any(|source| *source == changed) {
        return;
    }
    let Some(window) = page.window_target() else { return };
    let listening = page.dom.try_borrow().is_ok_and(|dom| dom.has_listener_for(window, ty));
    if page.is_disposed() || !listening {
        stop_pump(page, ty);
        return;
    }
    if !may_deliver(page) {
        return;
    }
    let services = page.context_services();
    // The sensors that can provide data: connected and not denied by the user.
    let mut chosen: Vec<&'static str> = {
        let hub = services.sensors.borrow();
        let list: Vec<&'static str> =
            sources(ty).iter().copied().filter(|sensor| hub.is_connected(sensor) && permitted(page, sensor)).collect();
        list
    };
    if ty != "devicemotion" {
        chosen.truncate(1);
    }
    if chosen.is_empty() {
        let first = PUMPS.with(|p| {
            p.borrow_mut().iter_mut().find(|x| x.ty == ty && same_page(&x.page, page)).is_some_and(|x| !std::mem::replace(&mut x.null_sent, true))
        });
        if first {
            fire(page, window, ty, &[]);
        }
        return;
    }
    // Every sensor that can provide data has to have: the event is made of all of them.
    let mut readings: Vec<(&'static str, u64, Vec<f64>)> = Vec::new();
    {
        let hub = services.sensors.borrow();
        for sensor in &chosen {
            match hub.read(sensor) {
                Some((seq, _, values)) => readings.push((*sensor, seq, values)),
                None => return,
            }
        }
    }
    let seqs: Vec<(&'static str, u64)> = readings.iter().map(|(sensor, seq, _)| (*sensor, *seq)).collect();
    let fresh = PUMPS.with(|p| {
        let mut p = p.borrow_mut();
        let Some(pump) = p.iter_mut().find(|x| x.ty == ty && same_page(&x.page, page)) else { return false };
        if !force && pump.sent == seqs {
            return false;
        }
        pump.sent = seqs;
        true
    });
    if fresh {
        let data: Vec<(&'static str, Vec<f64>)> = readings.into_iter().map(|(sensor, _, values)| (sensor, values)).collect();
        fire(page, window, ty, &data);
    }
}

/// Makes and fires the event of `ty` from the readings of the sensors that provided data (none: the event with
/// null values).
fn fire(page: &Rc<PageDom>, window: TargetKey, ty: &'static str, data: &[(&'static str, Vec<f64>)]) {
    let iface = if ty == "devicemotion" { EventInterface::DeviceMotionEvent } else { EventInterface::DeviceOrientationEvent };
    let event = EventCore::trusted(page, iface, ty, false, false);
    {
        let mut fields = event.fields.borrow_mut();
        if ty == "devicemotion" {
            let triplet = |sensor: &str, scale: f64| -> Option<[Option<f64>; 3]> {
                let values = &data.iter().find(|(s, _)| *s == sensor)?.1;
                Some([Some(round_to(values[0] * scale, 0.1)), Some(round_to(values[1] * scale, 0.1)), Some(round_to(values[2] * scale, 0.1))])
            };
            // A missing sensor leaves its members null; the objects are there either way.
            let null = [None; 3];
            fields.device_motion = crate::event::MotionFields {
                acceleration: Some(triplet("linear-acceleration", 1.0).unwrap_or(null)),
                gravity: Some(triplet("accelerometer", 1.0).unwrap_or(null)),
                rotation: Some(triplet("gyroscope", 180.0 / std::f64::consts::PI).unwrap_or(null)),
                interval: MOTION_INTERVAL,
                ..Default::default()
            };
        } else {
            fields.device_absolute = ty == "deviceorientationabsolute";
            if let Some((sensor, quaternion)) = data.first() {
                let [alpha, beta, gamma] = orientation_degrees(quaternion);
                fields.device_orientation = [Some(alpha), Some(beta), Some(gamma)];
                fields.device_absolute = *sensor == "absolute-orientation";
            }
        }
    }
    crate::target::fire(page, window, &event);
}

/// Device Orientation "orientation data from a quaternion": the ZXY intrinsic Euler angles `[alpha, beta, gamma]`
/// in degrees (alpha in [0, 360), beta in [-180, 180), gamma in [-90, 90]) of the unit quaternion `x y z w`,
/// rounded to 0.1 degrees as the specification requires of the events.
fn orientation_degrees(q: &[f64]) -> [f64; 3] {
    use std::f64::consts::PI;
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let (x, y, z, w) = if norm > 0.0 { (q[0] / norm, q[1] / norm, q[2] / norm, q[3] / norm) } else { (0.0, 0.0, 0.0, 1.0) };
    let (r00, r01) = (1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w));
    let (r10, r11) = (2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z));
    let (r20, r21, r22) = (2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y));
    let asin = r21.clamp(-1.0, 1.0).asin();
    let (alpha, beta, gamma) = if r22 > 0.0 {
        ((-r01).atan2(r11), asin, (-r20).atan2(r22))
    } else if r22 < 0.0 {
        // |beta| beyond 90 degrees: gamma stays in [-90, 90].
        (r01.atan2(-r11), if r21 >= 0.0 { PI } else { -PI } - asin, r20.atan2(-r22))
    } else if r21.abs() >= 1.0 - 1e-12 {
        // Gimbal lock (beta is +-90 degrees): all the rotation about the vertical goes to alpha.
        (r10.atan2(r00), asin, 0.0)
    } else {
        ((-r01).atan2(r11), asin, (-r20).atan2(r22))
    };
    let mut alpha = alpha.to_degrees();
    if alpha < 0.0 {
        alpha += 360.0;
    }
    let alpha = round_to(alpha, 0.1);
    [if alpha >= 360.0 { 0.0 } else { alpha }, round_to(beta.to_degrees(), 0.1), round_to(gamma.to_degrees(), 0.1)]
}

// ---- requestPermission

/// `DeviceOrientationEvent.requestPermission()` and `DeviceMotionEvent.requestPermission()`: the state of the
/// permissions the event type's sensors need. Denied if any is denied, granted if all are; otherwise the user
/// would be asked, which needs transient activation (a headless browser has nobody to ask: the request is
/// granted, as Chrome's desktop default for sensors is).
pub(crate) fn request_permission<'s>(s: &Scope<'s>, names: &[&str]) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let deferred = s.deferred()?;
    let states: Vec<PermissionSetting> = names.iter().map(|name| crate::navigator_extras::permission_state(&page, name)).collect();
    if !page.is_fully_active() {
        let error = JsError::dom("InvalidStateError", "The document is not fully active.").to_value(s);
        deferred.reject(s, error)?;
    } else if states.contains(&PermissionSetting::Denied) {
        deferred.resolve(s, s.string("denied"))?;
    } else if states.iter().all(|state| *state == PermissionSetting::Granted) {
        deferred.resolve(s, s.string("granted"))?;
    } else if page.has_transient_activation() {
        page.consume_transient_activation();
        deferred.resolve(s, s.string("granted"))?;
    } else {
        let error = JsError::dom("NotAllowedError", "A user gesture is required to request permission.").to_value(s);
        deferred.reject(s, error)?;
    }
    Ok(deferred.promise.get(s))
}

// ---- the JS module

/// The sensor interfaces on a window global (`[SecureContext]`: absent elsewhere).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    macro_rules! global {
        ($($name:literal),+ $(,)?) => {$(
            def.global_value_accessor(
                $name,
                |s, w| {
                    let page = crate::window::WindowHost::page(w);
                    if !crate::secure_context::secure(s, page) {
                        return Ok(s.undefined());
                    }
                    MODULE.global(s, page, $name)
                },
                |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), $name, value),
            );
        )+};
    }
    global!(
        "Sensor",
        "SensorErrorEvent",
        "Accelerometer",
        "LinearAccelerationSensor",
        "GravitySensor",
        "Gyroscope",
        "AmbientLightSensor",
        "OrientationSensor",
        "AbsoluteOrientationSensor",
        "RelativeOrientationSensor",
    );
}

/// What sensors.js gets from the engine. `read(type)` answers `undefined` when there is nothing to deliver to
/// this document now (no reading, or the document is hidden or unfocused), else `[counter, timestamp, ...values]`
/// with the timestamp on the document's clock.
fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    let permission = s.function("permission", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let name = a.get(0).to_string()?;
        Ok(s.string(crate::navigator_extras::permission_state(&page, &name).as_str()))
    })?;
    native.set_atom("permission", permission.value())?;
    let allowed = s.function("allowed", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let feature = a.get(0).to_string()?;
        // The engine's gate (`check_feature` also queues the `permissions-policy-violation` report).
        Ok(s.boolean(crate::permissions_policy::check_feature(s, &page, &feature)))
    })?;
    native.set_atom("allowed", allowed.value())?;
    let post = s.function("post", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let callback = Persistent::new(s, a.get(0));
        let realm = s.realm().downgrade();
        page.post_task(Box::new(move || {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            if let Some(f) = callback.get(&s).as_object() {
                if let Err(e) = f.call(None, &[]) {
                    s.report_exception(&e);
                }
            }
        }));
        Ok(s.undefined())
    })?;
    native.set_atom("post", post.value())?;
    let watch = s.function("watch", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let callback = Persistent::new(s, a.get(0));
        WATCHERS.with(|w| {
            let mut w = w.borrow_mut();
            w.retain(|w| w.page.upgrade().is_some_and(|p| !p.is_disposed()));
            w.push(Watcher { page: Rc::downgrade(&page), realm: s.realm().downgrade(), callback });
        });
        page.context_services().sensors.borrow_mut().register(&page);
        Ok(s.undefined())
    })?;
    native.set_atom("watch", watch.value())?;
    let connect = s.function("connect", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let ty = a.get(0).to_string()?;
        let frequency = a.get(1);
        let frequency = if frequency.is_undefined() { None } else { Some(frequency.to_number()?) };
        let result = page.context_services().sensors.borrow_mut().connect(&ty, frequency);
        Ok(match result {
            Ok(id) => s.number(id as f64),
            Err(name) => s.string(name),
        })
    })?;
    native.set_atom("connect", connect.value())?;
    let disconnect = s.function("disconnect", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let ty = a.get(0).to_string()?;
        let id = a.get(1).to_number()?;
        page.context_services().sensors.borrow_mut().disconnect(&ty, id as u64);
        Ok(s.undefined())
    })?;
    native.set_atom("disconnect", disconnect.value())?;
    let read = s.function("read", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let ty = a.get(0).to_string()?;
        if !may_deliver(&page) {
            return Ok(s.undefined());
        }
        let Some((seq, at, values)) = page.context_services().sensors.borrow().read(&ty) else { return Ok(s.undefined()) };
        // The reading's own time on this document's clock, in the steps of 100 microseconds that clock has.
        let age = at.elapsed().as_secs_f64() * 1000.0;
        let timestamp = (((crate::event::page_now_ms(&page) - age).max(0.0)) * 10.0).round() / 10.0;
        let mut items = vec![s.number(seq as f64), s.number(timestamp)];
        items.extend(values.into_iter().map(|v| s.number(v)));
        Ok(s.array(&items)?.value())
    })?;
    native.set_atom("read", read.value())?;
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quaternion of Euler angles about Z (alpha), X (beta), Y (gamma), intrinsic, as WebDriver's readings
    /// are turned into quaternions.
    fn quaternion(alpha: f64, beta: f64, gamma: f64) -> [f64; 4] {
        let (sz, cz) = (alpha.to_radians() / 2.0).sin_cos();
        let (sx, cx) = (beta.to_radians() / 2.0).sin_cos();
        let (sy, cy) = (gamma.to_radians() / 2.0).sin_cos();
        [cy * cz * sx - cx * sz * sy, cz * cx * sy + cy * sz * sx, cy * cx * sz + cz * sx * sy, cz * cx * cy - sz * sx * sy]
    }

    #[test]
    fn orientation_round_trips_through_the_quaternion() {
        for (a, b, g) in [(1.1, 2.2, 3.3), (11.1, 22.2, 33.3), (359.0, -45.0, 80.0), (200.0, 10.0, -60.0)] {
            let [alpha, beta, gamma] = orientation_degrees(&quaternion(a, b, g));
            assert!((alpha - a).abs() < 0.11 && (beta - b).abs() < 0.11 && (gamma - g).abs() < 0.11, "{a} {b} {g} -> {alpha} {beta} {gamma}");
        }
        // beta beyond 90 degrees keeps gamma within [-90, 90].
        let [alpha, beta, gamma] = orientation_degrees(&quaternion(30.0, 150.0, 20.0));
        assert!((alpha - 30.0).abs() < 0.11 && (beta - 150.0).abs() < 0.11 && (gamma - 20.0).abs() < 0.11, "{alpha} {beta} {gamma}");
        // A value that rounds up to 360 wraps to 0.
        assert_eq!(orientation_degrees(&quaternion(359.97, 0.0, 0.0))[0], 0.0);
    }

    #[test]
    fn readings_are_rounded_as_the_sensor_specifications_say() {
        let (accel, _) = process("accelerometer", &SensorReading::Xyz([1.12345, 2.12345, 3.12345])).unwrap();
        assert!(accel.iter().zip([1.1, 2.1, 3.1]).all(|(a, b)| (a - b).abs() < 1e-9));
        // 1, 2, 3 rad/s are 57.3, 114.6, 171.9 deg/s once rounded to 0.1 deg/s.
        let (gyro, _) = process("gyroscope", &SensorReading::Xyz([1.0, 2.0, 3.0])).unwrap();
        assert!(gyro.iter().zip([1.00007366, 2.00014732, 3.00022098]).all(|(a, b)| (a - b).abs() < 1e-8));
        assert_eq!(process("ambient-light", &SensorReading::Single(127.0)).unwrap().0, vec![150.0]);
        assert_eq!(process("ambient-light", &SensorReading::Single(24.0)).unwrap().0, vec![0.0]);
        assert!(process("accelerometer", &SensorReading::Single(1.0)).is_none());
    }

    #[test]
    fn ambient_light_drops_insignificant_changes() {
        let mut hub = SensorHub::default();
        hub.create("ambient-light", true, None, None).unwrap();
        let id = hub.connect("ambient-light", None).unwrap();
        let mut accepted = Vec::new();
        for lux in [127.0, 165.0, 24.0, 35.0, 49.0, 35.0, 23.0] {
            if hub.update("ambient-light", &SensorReading::Single(lux)).unwrap() {
                accepted.push(hub.read("ambient-light").unwrap().2[0]);
            }
        }
        // 165 rounds to what 127 did; 35 is within 25 lux of 24 and of 49.
        assert_eq!(accepted, vec![150.0, 0.0, 50.0, 0.0]);
        hub.disconnect("ambient-light", id);
    }

    #[test]
    fn requested_frequency_is_clamped_and_follows_the_clients() {
        let mut hub = SensorHub::default();
        assert!(hub.connect("accelerometer", None).is_err());
        hub.create("accelerometer", true, Some(2.0), Some(5.0)).unwrap();
        assert!(hub.create("accelerometer", true, None, None).is_err());
        assert_eq!(hub.requested_frequency("accelerometer"), Some(0.0));
        let fast = hub.connect("accelerometer", Some(50.0)).unwrap();
        let slow = hub.connect("accelerometer", Some(-1.0)).unwrap();
        assert_eq!(hub.requested_frequency("accelerometer"), Some(5.0));
        hub.disconnect("accelerometer", fast);
        assert_eq!(hub.requested_frequency("accelerometer"), Some(2.0));
        hub.disconnect("accelerometer", slow);
        // Not connected: no platform sensor, so no client.
        hub.create("gyroscope", false, None, None).unwrap();
        assert_eq!(hub.connect("gyroscope", None), Err("NotReadableError"));
        assert_eq!(hub.update("gyroscope", &SensorReading::Xyz([0.0; 3])), Ok(false));
    }

    #[test]
    fn a_reading_updated_while_no_client_is_connected_is_kept_for_the_first() {
        let mut hub = SensorHub::default();
        hub.create("gyroscope", true, None, None).unwrap();
        assert_eq!(hub.update("gyroscope", &SensorReading::Xyz([1.0, 2.0, 3.0])), Ok(true));
        let (seq, _, values) = hub.read("gyroscope").unwrap();
        assert_eq!((seq, values.len()), (1, 3));
        assert!(hub.update("gyroscope", &SensorReading::Single(1.0)).is_err());
        assert!(hub.update("gyroscope", &SensorReading::Xyz([f64::NAN, 0.0, 0.0])).is_err());
        assert!(hub.update("accelerometer", &SensorReading::Xyz([0.0; 3])).is_err());
    }
}
