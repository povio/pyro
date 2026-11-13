//! `EventTarget` bindings and cross-realm listener invocation.
//!
//! Listener functions live in a GC-traced hidden array on the target's
//! wrapper in the realm that registered them (cycles through closures stay
//! collectable). The native `Dom` listener list stores opaque handles
//! `(realm << 32) | slot`. A wrapper holding listeners for a connected node
//! is pinned with a `Persistent` so the GC cannot drop the listeners while
//! the node is reachable natively; it is unpinned once the node disconnects.

use std::rc::Rc;

use pyro_dom::events::{dispatch_on_path, AddOptions, ListenerHandle, ListenerInvoker, PathItem, TargetKey};
use pyro_dom::{Dom, NodeId};
use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Realm, RealmId, Scope, Value};

use crate::event::{wrap_event, EventCore, EventData};
use crate::page::PageDom;
use crate::util::*;

const LISTENERS: &str = "__lpListeners";

/// Data of every EventTarget wrapper (nodes embed it, Window upcasts to it).
pub struct EventTargetData {
    pub page: Rc<PageDom>,
    pub key: TargetKey,
    /// The functions in this wrapper's listener array, by slot.
    listeners: ListenerFns,
}

/// The listener functions of a target wrapper's hidden array ([`LISTENERS`]),
/// by slot, as raw object pointers: dispatch reads the function of a slot
/// here, without entering JavaScriptCore for the hidden property and the
/// element. The array stays the owner: it is traced from the wrapper, so a
/// function is alive while the wrapper is, and the two are written together
/// ([`set_listener_slot`]). The payload (and this mirror) dies with the
/// wrapper, so a wrapper made anew for the same target starts empty, like its
/// array. Unallocated until the first listener.
#[derive(Default)]
struct ListenerFns(std::cell::RefCell<Option<Box<Vec<usize>>>>);

impl ListenerFns {
    fn get(&self, slot: u32) -> Option<usize> {
        self.0.borrow().as_ref()?.get(slot as usize).copied().filter(|&raw| raw != 0)
    }
    fn set(&self, slot: u32, raw: usize) {
        let mut fns = self.0.borrow_mut();
        let fns = fns.get_or_insert_with(Default::default);
        if fns.len() <= slot as usize {
            fns.resize(slot as usize + 1, 0);
        }
        fns[slot as usize] = raw;
    }
}

/// Writes slot `slot` of `holder`'s listener array `array`, and of the
/// mirror in `holder`'s payload (the Window has no such payload: its array
/// is read directly). Every write of a listener array goes through here.
fn set_listener_slot(s: &Scope<'_>, holder: Object<'_>, array: Object<'_>, slot: u32, value: Value<'_>) -> JsResult<()> {
    array.set_index(slot, value)?;
    // SAFETY: `holder` is the receiver or a wrapper found live by the
    // caller, which keeps it for this call.
    if let Some(t) = unsafe { s.unwrap_rooted::<EventTarget>(holder.value()) } {
        t.listeners.set(slot, value.as_object().map_or(0, |o| o.as_raw() as usize));
    }
    Ok(())
}

impl EventTargetData {
    /// The identity the wrapper of a non-node, non-window target must be made
    /// with (`Scope::wrap`), so that dispatch finds it to invoke its listeners.
    pub fn wrapper_identity(&self) -> Option<pyro_js::IdentityKey> {
        match self.key {
            TargetKey::Other(k) if !self.page.is_window_key(self.key) => Some(self.page.other_identity(k)),
            _ => None,
        }
    }
    /// A fresh non-node target (XMLHttpRequest, AbortSignal, `new EventTarget()`).
    pub fn new(page: &Rc<PageDom>) -> EventTargetData {
        let key = page.dom.borrow_mut().new_target_key();
        EventTargetData::for_key(page.clone(), key)
    }
    /// The Window target of `document` (see `PageDom::create_window_target`).
    pub fn window(page: &Rc<PageDom>, key: TargetKey) -> EventTargetData {
        EventTargetData::for_key(page.clone(), key)
    }
    /// The data of a wrapper of target `key`.
    pub(crate) fn for_key(page: Rc<PageDom>, key: TargetKey) -> EventTargetData {
        EventTargetData { page, key, listeners: ListenerFns::default() }
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
        def.method_this("addEventListener", 2, |s, t, this, a| {
            add_listener(s, t, this, a)?;
            Ok(s.undefined())
        });
        def.method_this("removeEventListener", 2, |s, t, this, a| {
            remove_listener(s, t, this, a)?;
            Ok(s.undefined())
        });
        def.method_this("when", 1, |s, _t, this, a| {
            a.require(1, "EventTarget.when")?;
            crate::observable::when(s, this.value(), a.get(0), a.get(1))
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
            // `MediaQueryList`s are kept per realm and query text.
            None => Ok(crate::window::viewport_target_object(s, page, key)
                .or_else(|| crate::layout_api::media_query_object(s, page, key))
                .or_else(|| sibling_window(s, page, key))),
        },
    }
}

/// The window of a sibling document, for an event of one document dispatched
/// at another document's window.
fn sibling_window<'s>(s: &Scope<'s>, page: &PageDom, key: TargetKey) -> Option<Object<'s>> {
    page.sibling_realms(s).iter().find_map(|realm| {
        let scope = realm.scope().ok()?;
        page_of(&scope).ok()?.is_window_key(key).then(|| ())?;
        // The sibling's global lives as long as its realm.
        Some(s.rehome_object(scope.global()))
    })
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

/// DOM "flatten more": `AddEventListenerOptions` or a capture boolean. The
/// members are read in Web IDL dictionary order (`capture`, then `once`,
/// `passive`, `signal`), each once; a present `signal` must be an AbortSignal
/// (it is not nullable).
fn parse_options<'s>(s: &Scope<'_>, v: Value<'s>) -> JsResult<(AddOptions, Option<Object<'s>>)> {
    if let Some(o) = v.as_object() {
        let capture = o.get_atom("capture")?.to_bool();
        let once = o.get_atom("once")?.to_bool();
        let passive = {
            let p = o.get_atom("passive")?;
            if p.is_undefined() {
                None
            } else {
                Some(p.to_bool())
            }
        };
        let signal = o.get_atom("signal")?;
        let signal = if signal.is_undefined() {
            None
        } else {
            if s.unwrap::<crate::abort::AbortSignal>(signal).is_none() {
                return Err(JsError::Type(
                    "Failed to execute 'addEventListener' on 'EventTarget': Failed to read the 'signal' property from 'AddEventListenerOptions': Failed to convert value to 'AbortSignal'.".into(),
                ));
            }
            signal.as_object()
        };
        Ok((AddOptions { capture, once, passive }, signal))
    } else {
        Ok((AddOptions { capture: v.to_bool(), once: false, passive: None }, None))
    }
}

/// DOM "flatten": `EventListenerOptions` (only `capture`) or a capture boolean.
fn parse_capture(v: Value<'_>) -> JsResult<bool> {
    match v.as_object() {
        Some(o) => Ok(o.get_atom("capture")?.to_bool()),
        None => Ok(v.to_bool()),
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

/// Slot bit of a handle whose callback is the trampoline of an IDL event
/// handler (`set_event_handler`): the listener's callback, for the realm
/// rules of DOM "inner invoke", is the handler function it calls.
const HANDLER_SLOT: u32 = 1 << 31;

fn handler_handle_for(realm: RealmId, slot: u32) -> ListenerHandle {
    handle_for(realm, slot | HANDLER_SLOT)
}

/// HTML "event handler event type": the legacy `onwebkit*` handlers listen for
/// the mixed-case legacy types.
fn handler_event_type(ty: &'static str) -> &'static str {
    match ty {
        "webkitanimationend" => "webkitAnimationEnd",
        "webkitanimationiteration" => "webkitAnimationIteration",
        "webkitanimationstart" => "webkitAnimationStart",
        "webkittransitionend" => "webkitTransitionEnd",
        _ => ty,
    }
}

/// The wrapper whose listener array dispatch reads for `t`, from a member
/// called on `this`: a node's receiver is its wrapper in the realm the
/// member runs in, which `target_object` would find again; other targets
/// are found as dispatch finds them.
fn listener_holder<'s>(s: &Scope<'s>, t: &EventTargetData, this: Object<'s>) -> JsResult<Option<Object<'s>>> {
    match t.key {
        TargetKey::Node(_) => Ok(Some(this)),
        TargetKey::Other(_) => target_object(s, &t.page, t.key),
    }
}

fn add_listener<'s>(s: &Scope<'s>, t: &EventTargetData, obj: Object<'s>, a: Args<'s>) -> JsResult<()> {
    a.require(2, "EventTarget.addEventListener")?;
    crate::handlers::drain_inline(&t.page);
    let ty = a.get(0);
    let ty = str_of(&ty)?;
    let cb = a.get(1);
    // The options are converted before a null callback returns: their getters
    // run (feature detection) and a bad `signal` throws either way.
    let (mut opts, signal) = parse_options(s, a.get(2))?;
    let Some(cb_obj) = cb.as_object() else { return Ok(()) };
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
    let obj = listener_holder(s, t, obj)?.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    let arr = listener_array(s, obj, true)?.expect("created");
    // Reuse the slot of an identical callback so native de-duplication by
    // handle matches JS identity.
    let len = arr.get_atom("length")?.to_number()? as u32;
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
            set_listener_slot(s, obj, arr, slot, cb_obj.value())?;
            slot
        }
    };
    let realm = s.realm();
    let handle = handle_for(realm.id(), slot);
    t.page.register_realm(&realm);
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

fn remove_listener<'s>(s: &Scope<'s>, t: &EventTargetData, obj: Object<'s>, a: Args<'s>) -> JsResult<()> {
    a.require(2, "EventTarget.removeEventListener")?;
    let ty = a.get(0);
    let ty = str_of(&ty)?;
    let cb = a.get(1);
    let capture = parse_capture(a.get(2))?;
    if !cb.is_object() {
        return Ok(());
    }
    let Some(obj) = listener_holder(s, t, obj)? else { return Ok(()) };
    let Some(arr) = listener_array(s, obj, false)? else { return Ok(()) };
    let len = arr.get_atom("length")?.to_number()? as u32;
    for i in 0..len {
        if arr.get_index(i)?.strict_equals(cb) {
            let handle = handle_for(s.realm_id(), i);
            let mut dom = t.page.dom.borrow_mut();
            if dom.remove_listener(t.key, &ty, handle, capture).is_some() && !dom.handle_in_use(t.key, handle) {
                drop(dom);
                set_listener_slot(s, obj, arr, i, s.undefined())?;
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
            // A detached `<img>` loading keeps its listeners until `load`.
            d.key_node(key).is_some_and(|id| d.is_in_active_document(id) || page.image_loading(id))
        };
        if !connected {
            return;
        }
        if let TargetKey::Node(idx) = key {
            page.pin_node_wrapper(s, idx, obj);
        }
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

/// Script added a property to a node's wrapper. An unreferenced wrapper can be collected and made
/// again without script noticing, unless it holds such state, so keep this one while the node is
/// connected. It is pinned before its node is checked: the DOM may be borrowed here, and the next
/// safe point, which may follow a collection, sorts that out.
pub(crate) fn pin_wrapper_with_properties(s: &Scope<'_>, object: Object<'_>) {
    let Some(node) = s.unwrap::<crate::node::Node>(object.value()) else { return };
    let page = &node.target.page;
    let key = (s.realm_id(), TargetKey::Node(node.id.index()));
    if let Some(r) = page.owned_ref(s.realm_id(), node.id.index()) {
        // SAFETY: a live table entry.
        if !unsafe { r.is_kept() } {
            unsafe { r.set_kept(true) };
            page.new_property_pins.borrow_mut().push((key.0, node.id));
        }
        return;
    }
    let mut pinned = page.pinned.borrow_mut();
    if !pinned.contains_key(&key) {
        pinned.insert(key, Persistent::new(s, object.value()));
        page.new_property_pins.borrow_mut().push((key.0, node.id));
    }
}

/// Whether node `id`'s wrappers stay kept: it is in the active document, or
/// it is an `<img>` whose request is in flight (`PageDom::keep_loading_image`
/// releases them once it fired `load`/`error`).
fn still_kept(page: &PageDom, dom: &Dom, id: NodeId) -> bool {
    dom.is_in_active_document(id) || page.image_loading(id)
}

/// Safe-point maintenance (see `PageDom::after_task`).
pub(crate) fn reconcile_pins(page: &PageDom, dom: &mut Dom) {
    for (realm, id) in page.new_property_pins.take() {
        dom.mark_pin_on_connect(id);
        if !dom.is_in_active_document(id) && !page.image_loading(id) {
            page.pinned.borrow_mut().remove(&(realm, TargetKey::Node(id.index())));
            page.unkeep_node(Some(realm), id.index());
        }
    }
    let reconnected = dom.take_reconnected_listener_nodes();
    // Pins only become stale when a node is removed somewhere.
    let last = page.last_removals.replace(dom.removal_count());
    if last != dom.removal_count() {
        let mut pinned = page.pinned.borrow_mut();
        match dom.nodes_removed_since(last) {
            Some(nodes) => {
                // Pins (the identity-map worlds' wrappers) are few: checked
                // where they are rather than looked up for every removed node.
                let realms: Vec<RealmId> = if pinned.len() <= nodes.len() {
                    pinned.retain(|(_, key), _| match key {
                        TargetKey::Node(_) => dom.key_node(*key).is_some_and(|id| still_kept(page, dom, id)),
                        TargetKey::Other(_) => true,
                    });
                    Vec::new()
                } else {
                    page.realms.borrow().keys().copied().collect()
                };
                for id in nodes {
                    let key = TargetKey::Node(id.index());
                    let kept = page.node_kept(id.index());
                    if (kept || realms.iter().any(|&r| pinned.contains_key(&(r, key)))) && !still_kept(page, dom, id) {
                        for &r in &realms {
                            pinned.remove(&(r, key));
                        }
                        if kept {
                            page.unkeep_node(None, id.index());
                        }
                    }
                }
            }
            None => {
                pinned.retain(|(_, key), _| match key {
                    TargetKey::Node(_) => dom.key_node(*key).is_some_and(|id| still_kept(page, dom, id)),
                    TargetKey::Other(_) => true,
                });
                for idx in page.kept_nodes() {
                    if !dom.key_node(TargetKey::Node(idx)).is_some_and(|id| still_kept(page, dom, id)) {
                        page.unkeep_node(None, idx);
                    }
                }
            }
        }
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
            if let Some(obj) = page.node_wrapper(&s, id) {
                if dom.has_pin_on_connect(id) || matches!(s.get_hidden(obj, LISTENERS), Ok(Some(_))) {
                    page.pin_node_wrapper(&s, id.index(), obj);
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
    for (key, handle) in released {
        // The slot may have been reused since, by a listener or by a handler.
        let other = ListenerHandle(handle.0 ^ u64::from(HANDLER_SLOT));
        let in_use = {
            let dom = page.dom.borrow();
            dom.handle_in_use(key, handle) || dom.handle_in_use(key, other)
        };
        if in_use {
            continue;
        }
        let realm_id = RealmId((handle.0 >> 32) as u32);
        let Some(realm) = page.listener_realm(realm_id) else { continue };
        let Ok(s) = realm.scope() else { continue };
        let Ok(Some(obj)) = target_object(&s, page, key) else { continue };
        if let Ok(Some(arr)) = listener_array(&s, obj, false) {
            let _ = set_listener_slot(&s, obj, arr, handle.0 as u32 & !HANDLER_SLOT, s.undefined());
        }
    }
}

struct Invoker<'a> {
    page: &'a PageDom,
    event: &'a Rc<EventCore>,
    /// Invocation targets whose root was a shadow root when the event path
    /// was built (DOM "invocation-target-in-shadow-tree"); a listener that
    /// moves a node during dispatch does not change it.
    in_shadow: Vec<TargetKey>,
}

/// ECMAScript IsArray throws for a revoked Proxy and runs no trap of a live
/// one, so it tells the two apart without side effects. JavaScriptCore throws
/// the TypeError of calling a revoked Proxy in the Proxy's own realm; a
/// callback is called in its associated realm, whose TypeError it must be.
fn is_revoked_proxy(s: &Scope<'_>, value: Value<'_>) -> bool {
    if !value.has_proxy_target().unwrap_or(false) {
        return false;
    }
    let is_array =
        s.global().get_atom("Array").ok().and_then(|a| a.as_object()).and_then(|a| a.get_atom("isArray").ok());
    is_array.and_then(|f| f.as_object()).is_some_and(|f| f.call(None, &[s.rehome(value)]).is_err())
}

impl ListenerInvoker for Invoker<'_> {
    fn invoke(&mut self, callback: ListenerHandle, current: TargetKey, _target: TargetKey) {
        // HTML "check if we can run script": not with scripting disabled, nor
        // in a document that is no longer fully active (its frame was removed).
        if self.page.scripting_disabled() || !self.page.is_fully_active() {
            return;
        }
        // The relatedTarget as this invocation target sees it (DOM "invoke").
        self.event.fields.borrow_mut().related_target = self.event.state.borrow().related_target;
        let realm_id = RealmId((callback.0 >> 32) as u32);
        let is_handler = callback.0 as u32 & HANDLER_SLOT != 0;
        let slot = callback.0 as u32 & !HANDLER_SLOT;
        let Some(realm) = self.page.listener_realm(realm_id) else { return };
        if realm.is_disposed() {
            return;
        }
        // An uncaught exception's `error` event is the throwing world's
        // affair (`lifecycle::report_exception`).
        if let Some((event, world)) = self.page.error_event_world.get() {
            if event == self.event.id && world != realm_id {
                return;
            }
        }
        let Ok(s) = realm.scope() else { return };
        // The listener was registered in `s`'s realm, on the target's wrapper there.
        let found = (|| -> JsResult<Option<(Object<'_>, Object<'_>, Object<'_>)>> {
            let Some(this) = target_object(&s, self.page, current)? else { return Ok(None) };
            let Some(fo) = listener_function(&s, this, slot)? else { return Ok(None) };
            // Web IDL "associated realm" of the callback: the realm the
            // function or callback interface object was created in; for an
            // event handler, that of the handler function (HTML leaves the
            // realm of its internal listener open, and browsers use this one).
            let owner = if is_handler {
                let key = format!("__lpHandler_{}", &*self.event.state.borrow().ty);
                handler_get(&s, this, &key)?.and_then(|v| v.as_object()).unwrap_or(fo)
            } else {
                fo
            };
            Ok(Some((this, fo, owner)))
        })();
        let (this, fo, owner) = match found {
            Ok(Some(found)) => found,
            Ok(None) => return,
            Err(e) => return s.report_exception(&e),
        };
        let callback_realm = s.object_realm(owner).filter(|r| r.id() != realm_id && !r.is_disposed());
        let callback_scope = callback_realm.as_ref().and_then(|r| r.scope().ok());
        // `cs`: the callback's realm, which is entered for the call, owns the
        // current event and reports the exception.
        let cs = callback_scope.as_ref().unwrap_or(&s);
        // DOM "inner invoke": the callback's global (a Window) exposes the
        // event as its current event unless the invocation target was in a
        // shadow tree; the previous value is restored afterwards, including on
        // throw. The callback's microtask checkpoint runs before the restore.
        // The realm holds the event raw. `ev_root` keeps it on this frame
        // (seen by the conservative scan) until the previous value is put
        // back, past the call, its microtask checkpoint and the exception
        // report, which run script; the previous value is rooted the same way
        // by the listener call one level up.
        let in_shadow = self.in_shadow.contains(&current);
        let mut swapped: Option<Option<Value<'_>>> = None;
        let mut ev_root: Option<Value<'_>> = None;
        let result = (|| -> JsResult<()> {
            let ev = wrap_event(&s, self.event)?;
            ev_root = Some(ev.value());
            if !in_shadow {
                swapped = Some(cs.set_current_event(Some(ev.value())));
            }
            if is_handler {
                // The trampoline runs in its own realm and calls the handler.
                fo.call(Some(this), &[ev.value()])?;
                return Ok(());
            }
            let (Some(fo), ev) = (cs.rehome(fo.value()).as_object(), cs.rehome(ev.value())) else { return Ok(()) };
            let this = cs.rehome(this.value()).as_object();
            if fo.is_function() {
                fo.call(this, &[ev])?;
            } else {
                let handle = fo.get_atom("handleEvent")?;
                match handle.as_object().filter(|h| h.is_function()) {
                    Some(h) if is_revoked_proxy(cs, handle) => {
                        let _ = h;
                        return Err(JsError::Type("The 'handleEvent' callback is a revoked Proxy.".into()));
                    }
                    Some(h) => {
                        h.call(Some(fo), &[ev])?;
                    }
                    None => return Err(JsError::Type("The provided callback is not a function.".into())),
                }
            }
            Ok(())
        })();
        // The exception is reported while the event is still the current
        // one: the `error` event's listeners see it as `window.event`.
        if let Err(e) = result {
            WATCHED.with(|w| {
                let (watched, _) = w.get();
                if std::ptr::eq(watched, Rc::as_ptr(self.event)) {
                    w.set((watched, true));
                }
            });
            cs.report_exception(&e);
        }
        if let Some(prev) = swapped {
            cs.set_current_event(prev);
        }
        std::hint::black_box(ev_root);
    }
}

/// The function in slot `slot` of `holder`'s listener array: from the
/// mirror in `holder`'s payload ([`ListenerFns`]), or, for the Window (whose
/// array is the realm's holder, with no wrapper payload), from the array.
fn listener_function<'s>(s: &Scope<'s>, holder: Object<'s>, slot: u32) -> JsResult<Option<Object<'s>>> {
    // SAFETY: `holder` is a live wrapper the caller keeps for this call.
    if let Some(t) = unsafe { s.unwrap_rooted::<EventTarget>(holder.value()) } {
        return Ok(t.listeners.get(slot).map(|raw| {
            // SAFETY: an element of the live array `holder` owns (see
            // `ListenerFns`), of this VM.
            unsafe { Object::from_raw(s.ctx(), raw as pyro_js::sys::JSObjectRef) }
        }));
    }
    let Some(arr) = listener_array(s, holder, false)? else { return Ok(None) };
    Ok(arr.get_index(slot)?.as_object())
}

thread_local! {
    /// The event whose dispatch [`fire_noting_exceptions`] runs, and whether
    /// one of its listeners threw. Touched only on the exception path.
    static WATCHED: std::cell::Cell<(*const crate::event::EventCore, bool)> =
        const { std::cell::Cell::new((std::ptr::null(), false)) };
}

/// [`fire`], also telling whether a listener of this event threw (IndexedDB
/// aborts the transaction then). Exceptions of other events dispatched by
/// those listeners do not count.
pub(crate) fn fire_noting_exceptions(page: &PageDom, target: TargetKey, event: &EventData) -> (bool, bool) {
    let outer = WATCHED.replace((Rc::as_ptr(event), false));
    let not_canceled = fire(page, target, event);
    let (_, threw) = WATCHED.replace(outer);
    (not_canceled, threw)
}

/// `window.event`: the realm's current event, or undefined outside dispatch.
pub(crate) fn current_event<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    Ok(s.current_event().unwrap_or(s.undefined()))
}

/// Whether an event of type `ty` dispatched in `page` now would reach no
/// listener, whatever its target: none is registered for the type on any
/// node, document, window or other target of the page's tree, as a listener
/// or as an event handler attribute or property (`Dom::listens_for`; the
/// handler attributes the parser left are registered first, as a dispatch
/// does). Such a dispatch runs no script and nothing can observe the event
/// object, so a caller that has no other use for the object need not make
/// it, as long as it goes on as after an event nobody canceled. Asked right
/// before each event: a listener an earlier event's listener added counts.
pub(crate) fn unheard(page: &PageDom, ty: &str) -> bool {
    crate::handlers::drain_inline(page);
    !page.dom.borrow().listens_for(ty)
}

/// Dispatches `event` at `target` (DOM "dispatch"). Returns false if canceled.
pub fn fire(page: &PageDom, target: TargetKey, event: &EventData) -> bool {
    fire_with(page, target, event, &mut |_| {})
}

/// [`fire`], calling `before_listeners` with the event's path before any
/// listener runs (see `pyro_dom::events::dispatch_with`): where the activation
/// target is picked and its legacy-pre-activation behavior runs.
pub(crate) fn fire_with(
    page: &PageDom,
    target: TargetKey,
    event: &EventData,
    before_listeners: &mut dyn FnMut(&[PathItem]),
) -> bool {
    crate::handlers::drain_inline(page);
    {
        // Dispatch retargets the relatedTarget for each invocation target.
        let mut state = event.state.borrow_mut();
        state.target = Some(target);
        state.related_target = event.fields.borrow().related_target;
    }
    event.refresh_wrappers();
    // A callback can detach ancestors and run GC during dispatch. The path
    // is built once, here, and dispatched on below.
    let path = page.dom.borrow().event_path(target, &event.state.borrow());
    // No listener for the type on any target: no callback runs, so the path
    // needs no holding and no invocation target is asked about its tree.
    let heard = page.dom.borrow().listens_for(&event.state.borrow().ty);
    let ids: Box<[NodeId]> = if heard {
        let dom = page.dom.borrow();
        path.iter().filter_map(|(key, _)| dom.key_node(*key)).collect()
    } else {
        Box::default()
    };
    // The path's nodes are live (`key_node`), so each can be held.
    let _path_holds = heard.then(|| crate::collections::NodeHolds::new(&event.page, ids).ok()).flatten();
    // A node whose root is a shadow root has that root on the path after it,
    // so a path without one (nearly all) needs no root walks.
    let in_shadow = {
        let dom = page.dom.borrow();
        let shadow_root = |id: NodeId| dom.is_shadow_root(id).unwrap_or(false);
        if heard && path.iter().any(|(key, _)| dom.key_node(*key).is_some_and(shadow_root)) {
            let in_shadow_tree = |key: &TargetKey| {
                dom.key_node(*key).and_then(|id| dom.root_node(id, false).ok()).is_some_and(shadow_root)
            };
            path.iter().map(|(key, _)| *key).filter(in_shadow_tree).collect()
        } else {
            Vec::new()
        }
    };
    let mut inv = Invoker { page, event, in_shadow };
    let result = dispatch_on_path(&page.dom, target, &path, &event.state, &mut inv, before_listeners);
    // The relatedTarget stays as the outermost invocation target saw it, or
    // is reset with the target ("clearTargets").
    event.fields.borrow_mut().related_target = event.state.borrow().related_target;
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
    s.get_hidden_str(object, key)
}
fn handler_set(s: &Scope<'_>, object: Object<'_>, key: &str, value: Value<'_>) -> JsResult<()> {
    if object.as_raw() == s.global().as_raw() {
        let holder = window_listener_array(s, true)?.expect("created window holder");
        return holder.set(key, value);
    }
    s.set_hidden_str(object, key, value)
}

/// IDL event handlers use an ordinary listener slot and a traced hidden value.
/// Replacing a non-null handler preserves its position among other listeners.
/// HTML "Window-reflecting body element event handler set": `onblur` and
/// the like on the page document's `<body>`/`<frameset>` are the Window's.
fn forwarded_target(target: &EventTargetData, ty: &str) -> Option<EventTargetData> {
    if !crate::handlers::reflects_to_window(ty) {
        return None;
    }
    let page = &target.page;
    let dom = page.dom.borrow();
    let node = dom.key_node(target.key)?;
    let e = dom.element(node).ok()?;
    if !(e.is_html() && matches!(&*e.name.local, "body" | "frameset")) {
        return None;
    }
    if dom.node_document(node).ok() != Some(page.document()) {
        return None;
    }
    drop(dom);
    let window = page.window_target()?;
    Some(EventTargetData::window(page, window))
}

pub(crate) fn event_handler<'s>(s: &Scope<'s>, target: &EventTargetData, ty: &'static str) -> JsResult<Value<'s>> {
    if let Some(window) = forwarded_target(target, ty) {
        return event_handler(s, &window, ty);
    }
    let ty = handler_event_type(ty);
    crate::handlers::drain_inline(&target.page);
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
        let handle = handler_handle_for(s.realm_id(), slot);
        target.page.dom.borrow().listener_registration(target.key, ty, handle, false).is_some()
    }))
}
pub(crate) fn set_event_handler(
    s: &Scope<'_>,
    target: &EventTargetData,
    ty: &'static str,
    value: Value<'_>,
) -> JsResult<()> {
    if let Some(window) = forwarded_target(target, ty) {
        return set_event_handler(s, &window, ty, value);
    }
    let ty = handler_event_type(ty);
    crate::handlers::drain_inline(&target.page);
    let object =
        target_object(s, &target.page, target.key)?.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    let value_key = format!("__lpHandler_{ty}");
    let slot_key = format!("__lpHandlerSlot_{ty}");
    let value = if value.as_object().is_some_and(|o| o.is_function()) { value } else { s.null() };
    let slot = handler_slot(s, target, object, ty)?;
    handler_set(s, object, &value_key, value)?;
    if let Some(slot) = slot {
        if value.is_null() {
            let handle = handler_handle_for(s.realm_id(), slot);
            target.page.dom.borrow_mut().remove_listener(target.key, ty, handle, false);
            if let Some(array) = listener_array(s, object, false)? {
                set_listener_slot(s, object, array, slot, s.undefined())?;
            }
            handler_set(s, object, &slot_key, s.undefined())?;
        }
        return Ok(());
    }
    if value.is_null() {
        return Ok(());
    }
    let cancel_on_false = !matches!(ty, "mouseover" | "error");
    let window_error = ty == "error" && target.page.is_window_key(target.key);
    let callback = s.function("eventHandler", move |s, this, args| {
        if let Some(object) = this.as_object() {
            if let Some(handler) = handler_get(s, object, &value_key)?.and_then(|v| v.as_object()) {
                let event = args.get(0);
                // HTML "special error event handling" (OnErrorEventHandler):
                // the Window's `onerror` gets an ErrorEvent's fields as five
                // arguments and cancels the event by returning true.
                let error_fields = if window_error {
                    s.unwrap::<crate::event::ErrorEvent>(event).map(|e| {
                        let f = e.fields.borrow();
                        (f.message.clone(), f.filename.clone(), f.lineno, f.colno, f.error.clone())
                    })
                } else {
                    None
                };
                let cancel = match error_fields {
                    Some((message, filename, lineno, colno, error)) => {
                        let error = error.as_ref().map_or(s.undefined(), |e| e.get(s));
                        let args = [
                            s.string(&message),
                            s.string(&filename),
                            s.number(lineno.into()),
                            s.number(colno.into()),
                            error,
                        ];
                        let result = handler.call(Some(object), &args)?;
                        result.is_boolean() && result.to_bool()
                    }
                    None => {
                        let result = handler.call(Some(object), &[event])?;
                        // HTML "process the event handler return value". A
                        // `beforeunload` handler (OnBeforeUnloadEventHandler)
                        // returns `DOMString?`: a value other than null
                        // cancels the event and, converted to a string here
                        // (the event is still the current one), becomes its
                        // `returnValue` unless that is already set.
                        let before_unload = (ty == "beforeunload")
                            .then(|| s.unwrap::<crate::event::BeforeUnloadEvent>(event))
                            .flatten();
                        match before_unload {
                            Some(before_unload) if !result.is_nullish() => {
                                let text = dom_string(result)?;
                                let mut fields = before_unload.fields.borrow_mut();
                                if fields.data.as_deref().unwrap_or("").is_empty() {
                                    fields.data = Some(text);
                                }
                                true
                            }
                            Some(_) => false,
                            None => cancel_on_false && result.is_boolean() && !result.to_bool(),
                        }
                    }
                };
                if cancel {
                    if let Some(event) = event.as_object() {
                        if let Some(prevent) = event.get_atom("preventDefault")?.as_object() {
                            prevent.call(Some(event), &[])?;
                        }
                    }
                }
            }
        }
        Ok(s.undefined())
    })?;
    let array = listener_array(s, object, true)?.expect("created listener array");
    let len = array.get_atom("length")?.to_number()? as u32;
    let mut slot = len;
    for i in 0..len {
        if array.get_index(i)?.is_undefined() {
            slot = i;
            break;
        }
    }
    set_listener_slot(s, object, array, slot, callback.value())?;
    handler_set(s, object, &slot_key, s.number(slot as f64))?;
    target.page.dom.borrow_mut().add_listener(
        target.key,
        ty,
        handler_handle_for(s.realm_id(), slot),
        AddOptions { capture: false, once: false, passive: None },
    );
    maybe_pin(s, &target.page, target.key, object);
    Ok(())
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    (!WATCHED.get().0.is_null()).then_some("target::WATCHED")
}
