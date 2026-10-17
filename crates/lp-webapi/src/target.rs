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
            if !self.page.window_keys.borrow().contains(&self.key) {
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
    if page.window_keys.borrow().contains(&key) {
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
        TargetKey::Other(k) => Ok(s.lookup_identity(page.other_identity(k))),
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

fn handle_for(realm: RealmId, slot: u32) -> ListenerHandle {
    ListenerHandle((u64::from(realm.0) << 32) | u64::from(slot))
}

fn add_listener<'s>(s: &Scope<'s>, t: &EventTargetData, a: Args<'s>) -> JsResult<()> {
    a.require(2, "EventTarget.addEventListener")?;
    let ty = dom_string(a.get(0))?;
    let cb = a.get(1);
    let Some(cb_obj) = cb.as_object() else { return Ok(()) };
    let (opts, signal) = parse_options(a.get(2))?;
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
    for i in 0..len {
        if arr.get_index(i)?.strict_equals(cb) {
            slot = Some(i);
            break;
        }
    }
    let slot = match slot {
        Some(i) => i,
        None => {
            arr.set_index(len, cb_obj.value())?;
            len
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
                sd.on_abort_remove(t.key, ty, handle, opts.capture);
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
            d.key_node(key).and_then(|id| d.is_connected(id).ok()).unwrap_or(false)
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
    {
        let mut pinned = page.pinned.borrow_mut();
        pinned.retain(|(_, key), _| match key {
            TargetKey::Node(_) => dom.key_node(*key).is_some_and(|id| dom.is_connected(id).unwrap_or(false)),
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
            if !dom.is_connected(id).unwrap_or(false) {
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
        let realm_id = RealmId((callback.0 >> 32) as u32);
        let slot = callback.0 as u32;
        let realm = self.page.realms.borrow().get(&realm_id).and_then(|w| w.upgrade());
        let Some(realm) = realm else { return };
        if realm.is_disposed() {
            return;
        }
        let Ok(s) = realm.scope() else { return };
        let result = (|| -> JsResult<()> {
            let Some(this) = target_object(&s, self.page, current)? else { return Ok(()) };
            let Some(arr) = listener_array(&s, this, false)? else { return Ok(()) };
            let f = arr.get_index(slot)?;
            let Some(fo) = f.as_object() else { return Ok(()) };
            let ev = wrap_event(&s, self.event)?;
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
        if let Err(e) = result {
            s.report_exception(&e);
        }
    }
}

/// Dispatches `event` at `target` (DOM "dispatch"). Returns false if canceled.
pub fn fire(page: &PageDom, target: TargetKey, event: &EventData) -> bool {
    let mut inv = Invoker { page, event };
    dispatch(&page.dom, target, &event.state, &mut inv)
}

/// Convenience: fires a trusted event at a node.
pub fn fire_at_node(page: &Rc<PageDom>, node: NodeId, event: &EventData) -> bool {
    event.state.borrow_mut().is_trusted = true;
    let Some(key) = page.dom.borrow().node_key(node) else { return true };
    fire(page, key, event)
}
