//! `EventTarget` bindings and cross-realm listener invocation.
//!
//! Listener functions live in a GC-traced hidden array on the target's
//! wrapper in the realm that registered them (cycles through closures stay
//! collectable). The native `Dom` listener list stores opaque handles
//! `(realm << 32) | slot`. A wrapper holding listeners for a connected node
//! is pinned with a `Persistent` so the GC cannot drop the listeners while
//! the node is reachable natively; it is unpinned once the node disconnects.

use std::rc::Rc;

use lp_dom::events::{dispatch, AddOptions, ListenerHandle, ListenerInvoker, TargetKey};
use lp_dom::{Dom, NodeId};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Realm, RealmId, Scope, Value};

use crate::event::{wrap_event, EventCore, EventData};
use crate::page::PageDom;
use crate::util::*;

const LISTENERS: &str = "__lpListeners";

/// Data of every EventTarget wrapper (nodes embed it, Window upcasts to it).
pub struct EventTargetData {
    pub page: Rc<PageDom>,
    pub key: TargetKey,
}

impl EventTargetData {
    /// A fresh non-node target (XMLHttpRequest, AbortSignal, `new EventTarget()`).
    pub fn new(page: &Rc<PageDom>) -> EventTargetData {
        let key = page.dom.borrow_mut().new_target_key();
        EventTargetData { page: page.clone(), key }
    }
    /// The Window target of `document` (see `PageDom::create_window_target`).
    pub fn window(page: &Rc<PageDom>, key: TargetKey) -> EventTargetData {
        EventTargetData { page: page.clone(), key }
    }
}

impl Drop for EventTargetData {
    fn drop(&mut self) {
        // Non-node, non-window targets own their listener list.
        if let TargetKey::Other(_) = self.key {
            if !self.page.is_window_key(self.key) {
                if let Ok(mut d) = self.page.dom.try_borrow_mut() {
                    d.forget_target(self.key);
                }
            }
        }
    }
}

pub struct EventTarget;

impl JsClass for EventTarget {
    const NAME: &'static str = "EventTarget";
    type Data = EventTargetData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, new_target, _a| {
            let page = page_of(s)?;
            let data = EventTargetData::new(&page);
            let key = page.other_identity(match data.key {
                TargetKey::Other(k) => k,
                TargetKey::Node(_) => unreachable!(),
            });
            s.wrap_new::<EventTarget>(data, new_target, Some(key))
        });
        def.method("addEventListener", 2, |s, t, a| {
            add_listener(s, t, a)?;
            Ok(s.undefined())
        });
        def.method("removeEventListener", 2, |s, t, a| {
            remove_listener(s, t, a)?;
            Ok(s.undefined())
        });
        def.method("dispatchEvent", 1, |s, t, a| {
            a.require(1, "EventTarget.dispatchEvent")?;
            let ev = s
                .unwrap::<crate::event::Event>(a.get(0))
                .ok_or_else(|| {
                    JsError::Type(
                        "Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'."
                            .into(),
                    )
                })?
                .clone();
            {
                let st = ev.state.borrow();
                if st.dispatching || !st.initialized {
                    return Err(JsError::dom("InvalidStateError", "The event is already being dispatched."));
                }
            }
            ev.state.borrow_mut().is_trusted = false;
            // Click events run activation behavior (checkbox toggling, links,
            // submit) even when dispatched by script.
            let node = t.page.dom.borrow().key_node(t.key);
            Ok(s.boolean(match node {
                Some(n) => crate::input::dispatch_with_activation(&t.page, n, &ev),
                None => fire(&t.page, t.key, &ev),
            }))
        });
    }
}

/// The realm's `PageDom` (set by `install`).
pub fn page_of(s: &Scope<'_>) -> JsResult<Rc<PageDom>> {
    s.embedder::<PageDom>().ok_or_else(|| JsError::Type("DOM bindings are not installed in this realm".into()))
}

/// Wrapper object of a target in `s`'s realm, if it exists (nodes are
/// wrapped on demand).
pub(crate) fn target_object<'s>(s: &Scope<'s>, page: &PageDom, key: TargetKey) -> JsResult<Option<Object<'s>>> {
    if page.is_window_key(key) {
        return Ok(Some(s.global()));
    }
    match key {
        TargetKey::Node(_) => {
            let id = page.dom.borrow().key_node(key);
            match id {
                Some(id) => Ok(Some(crate::node::wrap_node(s, page, id)?)),
                None => Ok(None),
            }
        }
        TargetKey::Other(k) => match s.lookup_identity(page.other_identity(k)) {
            Some(o) => Ok(Some(o)),
            // `visualViewport` / `screen.orientation` keep a per-window
            // identity ([SameObject]); this realm's wrapper must own `key`.
            None => Ok(crate::window::viewport_target_object(s, page, key)),
        },
    }
}

pub(crate) fn target_value<'s>(s: &Scope<'s>, page: &PageDom, key: Option<TargetKey>) -> JsResult<Value<'s>> {
    match key {
        Some(k) => Ok(target_object(s, page, k)?.map_or(s.null(), |o| o.value())),
        None => Ok(s.null()),
    }
}

fn listener_array<'s>(s: &Scope<'s>, obj: Object<'_>, create: bool) -> JsResult<Option<Object<'s>>> {
    if obj.as_raw() == s.global().as_raw() {
        return window_listener_array(s, create);
    }
    if let Some(v) = s.get_hidden(obj, LISTENERS)? {
        if let Some(o) = v.as_object() {
            return Ok(Some(o));
        }
    }
    if !create {
        return Ok(None);
    }
    let arr = s.array(&[])?;
    s.set_hidden(obj, LISTENERS, arr.value())?;
    Ok(Some(arr))
}

fn window_listener_array<'s>(s: &Scope<'s>, create: bool) -> JsResult<Option<Object<'s>>> {
    let page = page_of(s)?;
    let realm = s.realm_id();
    if let Some(h) = page.window_holders.borrow().get(&realm) {
        return Ok(h.get(s).as_object());
    }
    if !create {
        return Ok(None);
    }
    let arr = s.array(&[])?;
    page.window_holders.borrow_mut().insert(realm, Persistent::new(s, arr.value()));
    Ok(Some(arr))
}

fn parse_options(v: Value<'_>) -> JsResult<(AddOptions, Option<Object<'_>>)> {
    if let Some(o) = v.as_object() {
        let signal = o.get("signal")?.as_object();
        Ok((
            AddOptions {
                capture: o.get("capture")?.to_bool(),
                once: o.get("once")?.to_bool(),
                passive: {
                    let p = o.get("passive")?;
                    if p.is_undefined() {
                        None
                    } else {
                        Some(p.to_bool())
                    }
                },
            },
            signal,
        ))
    } else {
        Ok((AddOptions { capture: v.to_bool(), once: false, passive: None }, None))
    }
}

/// DOM "default passive value": touchstart, touchmove, wheel and mousewheel
/// listeners on the window, a document, its document element or its body are
/// passive unless `passive` is given, so their `preventDefault()` cannot
/// block scrolling (as in Chrome).
fn default_passive(page: &PageDom, key: TargetKey, ty: &str) -> bool {
    if !matches!(ty, "touchstart" | "touchmove" | "wheel" | "mousewheel") {
        return false;
    }
    if page.window_target() == Some(key) {
        return true;
    }
    let dom = page.dom.borrow();
    let Some(node) = dom.key_node(key) else { return false };
    let Ok(doc) = dom.node_document(node) else { return false };
    node == doc || dom.document_element(doc).ok().flatten() == Some(node) || dom.body(doc).ok().flatten() == Some(node)
}

fn handle_for(realm: RealmId, slot: u32) -> ListenerHandle {
    ListenerHandle((u64::from(realm.0) << 32) | u64::from(slot))
}

fn add_listener<'s>(s: &Scope<'s>, t: &EventTargetData, a: Args<'s>) -> JsResult<()> {
    a.require(2, "EventTarget.addEventListener")?;
    let ty = dom_string(a.get(0))?;
    let cb = a.get(1);
    let Some(cb_obj) = cb.as_object() else { return Ok(()) };
    let (mut opts, signal) = parse_options(a.get(2))?;
    if opts.passive.is_none() {
        opts.passive = Some(default_passive(&t.page, t.key, &ty));
    }
    if let Some(sig) = signal {
        if let Some(sd) = s.unwrap::<crate::abort::AbortSignal>(sig.value()) {
            if sd.aborted() {
                return Ok(());
            }
        }
    }
    let obj = target_object(s, &t.page, t.key)?.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    let arr = listener_array(s, obj, true)?.expect("created");
    // Reuse the slot of an identical callback so native de-duplication by
    // handle matches JS identity.
    let len = arr.get("length")?.to_number()? as u32;
    let mut slot = None;
    let mut vacant = None;
    for i in 0..len {
        let value = arr.get_index(i)?;
        if value.is_undefined() && vacant.is_none() {
            vacant = Some(i);
        }
        if value.strict_equals(cb) {
            slot = Some(i);
            break;
        }
    }
    let slot = match slot {
        Some(i) => i,
        None => {
            let slot = vacant.unwrap_or(len);
            arr.set_index(slot, cb_obj.value())?;
            slot
        }
    };
    let realm = s.realm();
    let handle = handle_for(realm.id(), slot);
    t.page.realms.borrow_mut().entry(realm.id()).or_insert_with(|| realm.downgrade());
    let added = t.page.dom.borrow_mut().add_listener(t.key, &ty, handle, opts);
    if added {
        maybe_pin(s, &t.page, t.key, obj);
        if let Some(sig) = signal {
            if let Some(sd) = s.unwrap::<crate::abort::AbortSignal>(sig.value()) {
                let registration = t.page.dom.borrow().listener_registration(t.key, &ty, handle, opts.capture);
                if let Some(registration) = registration {
                    sd.on_abort_remove(t.key, registration);
                }
            }
        }
    }
    Ok(())
}

fn remove_listener<'s>(s: &Scope<'s>, t: &EventTargetData, a: Args<'s>) -> JsResult<()> {
    a.require(2, "EventTarget.removeEventListener")?;
    let ty = dom_string(a.get(0))?;
    let cb = a.get(1);
    if !cb.is_object() {
        return Ok(());
    }
    let (opts, _) = parse_options(a.get(2))?;
    let Some(obj) = target_object(s, &t.page, t.key)? else { return Ok(()) };
    let Some(arr) = listener_array(s, obj, false)? else { return Ok(()) };
    let len = arr.get("length")?.to_number()? as u32;
    for i in 0..len {
        if arr.get_index(i)?.strict_equals(cb) {
            let handle = handle_for(s.realm_id(), i);
            let mut dom = t.page.dom.borrow_mut();
            if dom.remove_listener(t.key, &ty, handle, opts.capture).is_some() && !dom.handle_in_use(t.key, handle) {
                drop(dom);
                arr.set_index(i, s.undefined())?;
            }
            break;
        }
    }
    Ok(())
}

/// Pins the wrapper while the target's node is connected (or always for
/// non-node targets that ask for it via `pin_target`).
fn maybe_pin(s: &Scope<'_>, page: &PageDom, key: TargetKey, obj: Object<'_>) {
    if let TargetKey::Node(_) = key {
        let connected = {
            let d = page.dom.borrow();
            d.key_node(key).is_some_and(|id| d.is_in_active_document(id))
        };
        if !connected {
            return;
        }
        page.pinned.borrow_mut().entry((s.realm_id(), key)).or_insert_with(|| Persistent::new(s, obj.value()));
    }
}

/// Keeps a non-node target's wrapper alive (e.g. XHR while a request is in
/// flight) so its listeners can fire.
pub fn pin_target(s: &Scope<'_>, page: &PageDom, key: TargetKey, obj: Object<'_>) {
    page.pinned.borrow_mut().entry((s.realm_id(), key)).or_insert_with(|| Persistent::new(s, obj.value()));
}
pub fn unpin_target(s: &Scope<'_>, page: &PageDom, key: TargetKey) {
    page.pinned.borrow_mut().remove(&(s.realm_id(), key));
}

/// Safe-point maintenance (see `PageDom::after_task`).
pub(crate) fn reconcile_pins(page: &PageDom, dom: &mut Dom) {
    let reconnected = dom.take_reconnected_listener_nodes();
    // Pins only become stale when a node is removed somewhere.
    if page.last_removals.replace(dom.removal_count()) != dom.removal_count() {
        let mut pinned = page.pinned.borrow_mut();
        pinned.retain(|(_, key), _| match key {
            TargetKey::Node(_) => dom.key_node(*key).is_some_and(|id| dom.is_in_active_document(id)),
            TargetKey::Other(_) => true,
        });
    }
    if reconnected.is_empty() {
        return;
    }
    let realms: Vec<Realm> = page.realms.borrow().values().filter_map(|w| w.upgrade()).collect();
    for realm in realms {
        let Ok(s) = realm.scope() else { continue };
        for &id in &reconnected {
            if !dom.is_in_active_document(id) {
                continue;
            }
            if let Some(obj) = s.lookup_identity(page.node_identity(id)) {
                if matches!(s.get_hidden(obj, LISTENERS), Ok(Some(_))) {
                    let key = TargetKey::Node(id.index());
                    page.pinned
                        .borrow_mut()
                        .entry((realm.id(), key))
                        .or_insert_with(|| Persistent::new(&s, obj.value()));
                }
            }
        }
    }
}

/// Clears hidden slots of listeners removed natively (`once`).
pub(crate) fn release_listener_slots(page: &PageDom, released: Vec<(TargetKey, ListenerHandle)>) {
    if released.is_empty() {
        return;
    }
    let realms = page.realms.borrow().clone();
    for (key, handle) in released {
        if page.dom.borrow().handle_in_use(key, handle) {
            continue;
        }
        let realm_id = RealmId((handle.0 >> 32) as u32);
        let Some(realm) = realms.get(&realm_id).and_then(|w| w.upgrade()) else { continue };
        let Ok(s) = realm.scope() else { continue };
        let Ok(Some(obj)) = target_object(&s, page, key) else { continue };
        if let Ok(Some(arr)) = listener_array(&s, obj, false) {
            let _ = arr.set_index(handle.0 as u32, s.undefined());
        }
    }
}

struct Invoker<'a> {
    page: &'a PageDom,
    event: &'a Rc<EventCore>,
}

impl ListenerInvoker for Invoker<'_> {
    fn invoke(&mut self, callback: ListenerHandle, current: TargetKey, _target: TargetKey) {
        if self.page.scripting_disabled() {
            return;
        }
        let realm_id = RealmId((callback.0 >> 32) as u32);
        let slot = callback.0 as u32;
        let realm = self.page.realms.borrow().get(&realm_id).and_then(|w| w.upgrade());
        let Some(realm) = realm else { return };
        if realm.is_disposed() {
            return;
        }
        let Ok(s) = realm.scope() else { return };
        // DOM "inner invoke": the listener's global (a Window) exposes the event
        // as its current event unless the invocation target is in a shadow
        // tree; the previous value is restored afterwards, including on throw.
        // The callback's microtask checkpoint runs before the restore.
        let in_shadow = {
            let d = self.page.dom.borrow();
            d.key_node(current)
                .and_then(|id| d.root_node(id, false).ok())
                .is_some_and(|root| d.is_shadow_root(root).unwrap_or(false))
        };
        let mut saved: Option<Persistent> = None;
        let mut swapped = false;
        let result = (|| -> JsResult<()> {
            let Some(this) = target_object(&s, self.page, current)? else { return Ok(()) };
            let Some(arr) = listener_array(&s, this, false)? else { return Ok(()) };
            let f = arr.get_index(slot)?;
            let Some(fo) = f.as_object() else { return Ok(()) };
            let ev = wrap_event(&s, self.event)?;
            if !in_shadow {
                let holder = window_listener_array(&s, true)?.expect("created window holder");
                saved = Some(Persistent::new(&s, holder.get(CURRENT_EVENT)?));
                holder.set(CURRENT_EVENT, ev.value())?;
                swapped = true;
            }
            if fo.is_function() {
                fo.call(Some(this), &[ev.value()])?;
            } else {
                let handle = fo.get("handleEvent")?;
                match handle.as_object().filter(|h| h.is_function()) {
                    Some(h) => {
                        h.call(Some(fo), &[ev.value()])?;
                    }
                    None => return Err(JsError::Type("The provided callback is not a function.".into())),
                }
            }
            Ok(())
        })();
        if swapped {
            if let Ok(Some(holder)) = window_listener_array(&s, false) {
                let prev = saved.as_ref().map_or(s.undefined(), |p| p.get(&s));
                let _ = holder.set(CURRENT_EVENT, prev);
            }
        }
        if let Err(e) = result {
            s.report_exception(&e);
        }
    }
}

/// Key of the realm's current event on its traced window holder.
const CURRENT_EVENT: &str = "__lpCurrentEvent";

/// `window.event`: the realm's current event, or undefined outside dispatch.
pub(crate) fn current_event<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    Ok(window_listener_array(s, false)?.map(|h| h.get(CURRENT_EVENT)).transpose()?.unwrap_or(s.undefined()))
}

/// Dispatches `event` at `target` (DOM "dispatch"). Returns false if canceled.
pub fn fire(page: &PageDom, target: TargetKey, event: &EventData) -> bool {
    event.state.borrow_mut().target = Some(target);
    event.refresh_wrappers();
    // A callback can detach ancestors and run GC during dispatch.
    let path = page.dom.borrow().event_path(target, &event.state.borrow());
    let ids: Vec<_> = path.iter().filter_map(|(key, _)| page.dom.borrow().key_node(*key)).collect();
    let _path_holds: Vec<_> =
        ids.into_iter().filter_map(|id| crate::collections::NodeHold::new(&event.page, id).ok()).collect();
    let mut inv = Invoker { page, event };
    let result = dispatch(&page.dom, target, &event.state, &mut inv);
    event.refresh_wrappers();
    result
}

/// Convenience: fires a trusted event at a node.
pub fn fire_at_node(page: &Rc<PageDom>, node: NodeId, event: &EventData) -> bool {
    event.state.borrow_mut().is_trusted = true;
    let Some(key) = page.dom.borrow().node_key(node) else { return true };
    fire(page, key, event)
}

/// Handler state lives in traced hidden properties of the wrapper, except
/// for the Window: page realms use the default JSC global (no private
/// properties), so its state goes on the per-realm window listener holder,
/// which scripts cannot reach.
fn handler_get<'s>(s: &Scope<'s>, object: Object<'_>, key: &str) -> JsResult<Option<Value<'s>>> {
    if object.as_raw() == s.global().as_raw() {
        return Ok(window_listener_array(s, false)?.map(|h| h.get(key)).transpose()?.filter(|v| !v.is_undefined()));
    }
    s.get_hidden(object, key)
}
fn handler_set(s: &Scope<'_>, object: Object<'_>, key: &str, value: Value<'_>) -> JsResult<()> {
    if object.as_raw() == s.global().as_raw() {
        let holder = window_listener_array(s, true)?.expect("created window holder");
        return holder.set(key, value);
    }
    s.set_hidden(object, key, value)
}

/// IDL event handlers use an ordinary listener slot and a traced hidden value.
/// Replacing a non-null handler preserves its position among other listeners.
pub(crate) fn event_handler<'s>(s: &Scope<'s>, target: &EventTargetData, ty: &str) -> JsResult<Value<'s>> {
    let object =
        target_object(s, &target.page, target.key)?.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    if handler_slot(s, target, object, ty)?.is_none() {
        return Ok(s.null());
    }
    Ok(handler_get(s, object, &format!("__lpHandler_{ty}"))?.unwrap_or(s.null()))
}

/// The handler's listener slot while its native listener is still
/// registered; `document.open()` erases handlers natively.
fn handler_slot(s: &Scope<'_>, target: &EventTargetData, object: Object<'_>, ty: &str) -> JsResult<Option<u32>> {
    let slot = handler_get(s, object, &format!("__lpHandlerSlot_{ty}"))?
        .filter(|v| !v.is_undefined())
        .map(|v| v.to_number())
        .transpose()?
        .map(|v| v as u32);
    Ok(slot.filter(|&slot| {
        target.page.dom.borrow().listener_registration(target.key, ty, handle_for(s.realm_id(), slot), false).is_some()
    }))
}
pub(crate) fn set_event_handler(
    s: &Scope<'_>,
    target: &EventTargetData,
    ty: &'static str,
    value: Value<'_>,
) -> JsResult<()> {
    let object =
        target_object(s, &target.page, target.key)?.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    let value_key = format!("__lpHandler_{ty}");
    let slot_key = format!("__lpHandlerSlot_{ty}");
    let value = if value.as_object().is_some_and(|o| o.is_function()) { value } else { s.null() };
    let slot = handler_slot(s, target, object, ty)?;
    handler_set(s, object, &value_key, value)?;
    if let Some(slot) = slot {
        if value.is_null() {
            let handle = handle_for(s.realm_id(), slot);
            target.page.dom.borrow_mut().remove_listener(target.key, ty, handle, false);
            if let Some(array) = listener_array(s, object, false)? {
                array.set_index(slot, s.undefined())?;
            }
            handler_set(s, object, &slot_key, s.undefined())?;
        }
        return Ok(());
    }
    if value.is_null() {
        return Ok(());
    }
    let cancel_on_false = !matches!(ty, "mouseover" | "error");
    let callback = s.function("eventHandler", move |s, this, args| {
        if let Some(object) = this.as_object() {
            if let Some(handler) = handler_get(s, object, &value_key)?.and_then(|v| v.as_object()) {
                let result = handler.call(Some(object), &[args.get(0)])?;
                // HTML "process the event handler return value".
                if cancel_on_false && result.is_boolean() && !result.to_bool() {
                    if let Some(event) = args.get(0).as_object() {
                        if let Some(prevent) = event.get("preventDefault")?.as_object() {
                            prevent.call(Some(event), &[])?;
                        }
                    }
                }
            }
        }
        Ok(s.undefined())
    })?;
    let array = listener_array(s, object, true)?.expect("created listener array");
    let len = array.get("length")?.to_number()? as u32;
    let mut slot = len;
    for i in 0..len {
        if array.get_index(i)?.is_undefined() {
            slot = i;
            break;
        }
    }
    array.set_index(slot, callback.value())?;
    handler_set(s, object, &slot_key, s.number(slot as f64))?;
    target.page.dom.borrow_mut().add_listener(
        target.key,
        ty,
        handle_for(s.realm_id(), slot),
        AddOptions { capture: false, once: false, passive: None },
    );
    maybe_pin(s, &target.page, target.key, object);
    Ok(())
}
