//! EventTarget listener storage and the DOM dispatch algorithm (DOM §2.9).
//!
//! Listener callbacks are opaque `ListenerHandle`s owned by the bindings
//! (typically an index into a GC-traced per-realm array, so closures that
//! capture their own node stay collectable). Dispatch never holds a `Dom`
//! borrow while invoking a listener: listeners may mutate the tree.

use std::cell::RefCell;

use markup5ever::LocalName;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::node::{NodeId, NodeKind, NIL};

/// Identifies an event target. Nodes use their slot index; non-node targets
/// (Window, XMLHttpRequest, AbortSignal, ...) use keys from `new_target_key`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TargetKey {
    Node(u32),
    Other(u64),
}

/// Bindings-owned callback reference. Equality must match JS identity of
/// the callback within its realm (used for duplicate suppression).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ListenerHandle(pub u64);

#[derive(Clone, Debug)]
struct Listener {
    id: u32,
    ty: LocalName,
    callback: ListenerHandle,
    capture: bool,
    once: bool,
    passive: bool,
}

#[derive(Default)]
pub struct ListenerStore {
    map: FxHashMap<TargetKey, Vec<Listener>>,
    next_id: u32,
    next_key: u64,
    /// Listeners dropped natively (once, target freed), for bindings cleanup.
    released: Vec<(TargetKey, ListenerHandle)>,
    /// Nodes holding listeners that were (re)connected since the last drain.
    reconnected: Vec<u32>,
    /// Nodes whose wrapper script added properties to, also reported when (re)connected.
    pin_on_connect: FxHashSet<u32>,
    /// Document -> Window key for event paths.
    windows: FxHashMap<u32, u64>,
}

impl ListenerStore {
    /// Drops the target's listeners, keeping a document's window link.
    fn erase(&mut self, key: TargetKey) {
        if let Some(list) = self.map.remove(&key) {
            self.released.extend(list.into_iter().map(|l| (key, l.callback)));
        }
    }

    pub(crate) fn forget(&mut self, key: TargetKey) {
        if let Some(list) = self.map.remove(&key) {
            self.released.extend(list.into_iter().map(|l| (key, l.callback)));
        }
        if let TargetKey::Node(i) = key {
            self.windows.remove(&i);
            self.pin_on_connect.remove(&i);
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Phase {
    None = 0,
    Capturing = 1,
    AtTarget = 2,
    Bubbling = 3,
}

/// Mutable state of one Event object. Bindings keep it in a `RefCell`
/// shared with the JS Event wrapper so `stopPropagation()` etc. are visible
/// to the running dispatch.
#[derive(Clone, Debug)]
pub struct EventState {
    pub ty: LocalName,
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub is_trusted: bool,
    pub time_stamp: f64,
    pub phase: Phase,
    pub target: Option<TargetKey>,
    pub current_target: Option<TargetKey>,
    pub related_target: Option<TargetKey>,
    pub stop_propagation: bool,
    pub stop_immediate: bool,
    pub canceled: bool,
    pub in_passive_listener: bool,
    pub dispatching: bool,
    pub initialized: bool,
    /// Path snapshot exposed by `composedPath()` during dispatch.
    pub path: Vec<TargetKey>,
}

impl EventState {
    pub fn new(ty: &str, bubbles: bool, cancelable: bool, composed: bool) -> Self {
        EventState {
            ty: LocalName::from(ty),
            bubbles,
            cancelable,
            composed,
            is_trusted: false,
            time_stamp: 0.0,
            phase: Phase::None,
            target: None,
            current_target: None,
            related_target: None,
            stop_propagation: false,
            stop_immediate: false,
            canceled: false,
            in_passive_listener: false,
            dispatching: false,
            initialized: true,
            path: Vec::new(),
        }
    }
    /// `preventDefault()`.
    pub fn prevent_default(&mut self) {
        if self.cancelable && !self.in_passive_listener {
            self.canceled = true;
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AddOptions {
    pub capture: bool,
    pub once: bool,
    /// `None` = default (passive for touch/wheel on window/document/body is
    /// not modelled; treated as false).
    pub passive: Option<bool>,
}

impl Dom {
    /// Allocates a key for a non-node EventTarget.
    pub fn new_target_key(&mut self) -> TargetKey {
        self.events.next_key += 1;
        TargetKey::Other(self.events.next_key)
    }

    /// Associates a document with its Window target (event path parent).
    pub fn set_document_window(&mut self, doc: NodeId, window: Option<TargetKey>) {
        if let Ok(i) = self.check(doc) {
            match window {
                Some(TargetKey::Other(w)) => {
                    self.events.windows.insert(i, w);
                }
                _ => {
                    self.events.windows.remove(&i);
                }
            }
        }
    }

    pub fn node_key(&self, id: NodeId) -> Option<TargetKey> {
        self.check(id).ok().map(TargetKey::Node)
    }
    /// Resolves a node target key back to a live `NodeId`.
    pub fn key_node(&self, key: TargetKey) -> Option<NodeId> {
        match key {
            TargetKey::Node(i) if self.is_alive(self.id_of_unchecked(i)) => Some(self.id_of_unchecked(i)),
            _ => None,
        }
    }

    /// `addEventListener`. Returns false if an equivalent listener exists.
    pub fn add_listener(&mut self, target: TargetKey, ty: &str, callback: ListenerHandle, opts: AddOptions) -> bool {
        let st = &mut self.events;
        let list = st.map.entry(target).or_default();
        if list.iter().any(|l| l.callback == callback && l.capture == opts.capture && &*l.ty == ty) {
            return false;
        }
        st.next_id = st.next_id.wrapping_add(1);
        list.push(Listener {
            id: st.next_id,
            ty: LocalName::from(ty),
            callback,
            capture: opts.capture,
            once: opts.once,
            passive: opts.passive.unwrap_or(false),
        });
        true
    }

    /// `removeEventListener`. Returns the dropped handle for release.
    pub fn remove_listener(
        &mut self,
        target: TargetKey,
        ty: &str,
        callback: ListenerHandle,
        capture: bool,
    ) -> Option<ListenerHandle> {
        let list = self.events.map.get_mut(&target)?;
        let pos = list.iter().position(|l| l.callback == callback && l.capture == capture && &*l.ty == ty)?;
        let l = list.remove(pos);
        if list.is_empty() {
            self.events.map.remove(&target);
        }
        self.events.released.push((target, l.callback));
        Some(l.callback)
    }

    /// Identity of one registration, distinct from the reusable callback slot.
    pub fn listener_registration(
        &self,
        target: TargetKey,
        ty: &str,
        callback: ListenerHandle,
        capture: bool,
    ) -> Option<u32> {
        self.events
            .map
            .get(&target)?
            .iter()
            .find(|l| l.callback == callback && l.capture == capture && &*l.ty == ty)
            .map(|l| l.id)
    }

    /// Removes only the registration originally attached to an AbortSignal.
    pub fn abort_listener(&mut self, target: TargetKey, registration: u32) {
        let Some(list) = self.events.map.get_mut(&target) else { return };
        let Some(pos) = list.iter().position(|l| l.id == registration) else { return };
        let listener = list.remove(pos);
        if list.is_empty() {
            self.events.map.remove(&target);
        }
        self.events.released.push((target, listener.callback));
    }

    /// Whether any listener entry of `target` still uses `callback`.
    pub fn handle_in_use(&self, target: TargetKey, callback: ListenerHandle) -> bool {
        self.events.map.get(&target).is_some_and(|l| l.iter().any(|x| x.callback == callback))
    }

    /// Nodes with listeners that became connected since the last call.
    pub fn take_reconnected_listener_nodes(&mut self) -> Vec<NodeId> {
        let v = std::mem::take(&mut self.events.reconnected);
        v.into_iter().filter(|&i| self.slot_live(i)).map(|i| self.id_of(i)).collect()
    }

    /// Reports `id` from [`Dom::take_reconnected_listener_nodes`] whenever it connects, as if
    /// it held a listener: its wrapper has state that must outlive the wrapper's references.
    pub fn mark_pin_on_connect(&mut self, id: NodeId) {
        if self.slot_live(id.index()) {
            self.events.pin_on_connect.insert(id.index());
        }
    }

    pub fn has_pin_on_connect(&self, id: NodeId) -> bool {
        self.events.pin_on_connect.contains(&id.index())
    }

    /// Records listener-bearing nodes in a newly connected subtree.
    pub(crate) fn note_connected_listeners(&mut self, root: u32) {
        if self.events.map.is_empty() && self.events.pin_on_connect.is_empty() {
            return;
        }
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            if self.events.map.contains_key(&TargetKey::Node(i)) || self.events.pin_on_connect.contains(&i) {
                self.events.reconnected.push(i);
            }
            stack.extend(self.child_idx_iter(i));
            if let Some(e) = self.n(i).element() {
                if e.shadow_root != NIL {
                    stack.push(e.shadow_root);
                }
            }
        }
    }

    pub fn has_listeners(&self, target: TargetKey) -> bool {
        self.events.map.get(&target).is_some_and(|l| !l.is_empty())
    }

    /// HTML "erase all event listeners and handlers" for one target
    /// (`document.open()`); a document keeps its window for event paths.
    pub fn erase_listeners(&mut self, target: TargetKey) {
        self.events.erase(target);
    }

    /// Drops every listener of a non-node target (e.g. XHR finalized).
    pub fn forget_target(&mut self, target: TargetKey) {
        self.events.forget(target);
    }

    /// Handles whose listener entries were removed natively; bindings
    /// release their storage.
    pub fn take_released_listeners(&mut self) -> Vec<(TargetKey, ListenerHandle)> {
        std::mem::take(&mut self.events.released)
    }

    fn snapshot(&self, target: TargetKey, ty: &LocalName) -> Vec<Listener> {
        match self.events.map.get(&target) {
            Some(list) => list.iter().filter(|l| &l.ty == ty).cloned().collect(),
            None => Vec::new(),
        }
    }
    fn listener_live(&self, target: TargetKey, id: u32) -> bool {
        self.events.map.get(&target).is_some_and(|l| l.iter().any(|x| x.id == id))
    }
    fn remove_listener_id(&mut self, target: TargetKey, id: u32) {
        if let Some(list) = self.events.map.get_mut(&target) {
            list.retain(|l| l.id != id);
        }
    }

    pub(crate) fn id_of_unchecked(&self, idx: u32) -> NodeId {
        // Generation lookup without liveness check; callers verify.
        NodeId { idx, gen: self.generation_at(idx) }
    }

    /// Shadow-including parent for event paths ("get the parent").
    fn event_parent(&self, key: TargetKey, ev_ty: &LocalName, composed: bool, origin_root: u32) -> Option<TargetKey> {
        let TargetKey::Node(i) = key else { return None };
        let n = self.n(i);
        // A slottable assigned to a slot: the slot comes next.
        if let Some(slot) = self.find_slot(i, false) {
            return Some(TargetKey::Node(slot));
        }
        if n.parent != NIL {
            return Some(TargetKey::Node(n.parent));
        }
        match &n.kind {
            NodeKind::Fragment(f) if f.shadow.is_some() => {
                if !composed && i == origin_root {
                    None
                } else {
                    Some(TargetKey::Node(f.host))
                }
            }
            NodeKind::Document(_) if &**ev_ty != "load" => self.events.windows.get(&i).map(|&w| TargetKey::Other(w)),
            _ => None,
        }
    }

    /// Retargets `a` against `b` (DOM "retarget").
    fn retarget(&self, a: TargetKey, b: TargetKey) -> TargetKey {
        let TargetKey::Node(mut ai) = a else { return a };
        loop {
            let root = self.root_idx(ai);
            let host = match &self.n(root).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
                _ => return TargetKey::Node(ai),
            };
            let inside = match b {
                TargetKey::Node(bi) => self.shadow_including_ancestor(root, bi),
                TargetKey::Other(_) => false,
            };
            if inside {
                return TargetKey::Node(ai);
            }
            ai = host;
        }
    }
    fn shadow_including_ancestor(&self, anc: u32, mut node: u32) -> bool {
        loop {
            if self.is_inclusive_ancestor(anc, node) {
                return true;
            }
            let r = self.root_idx(node);
            match &self.n(r).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => node = f.host,
                _ => return false,
            }
        }
    }

    /// Builds the event path: (invocation target, shadow-adjusted target).
    pub fn event_path(&self, target: TargetKey, ev: &EventState) -> Vec<(TargetKey, TargetKey)> {
        let origin_root = match target {
            TargetKey::Node(i) if self.slot_live(i) => self.root_idx(i),
            TargetKey::Node(_) => return Vec::new(),
            TargetKey::Other(_) => NIL,
        };
        let mut path = vec![(target, target)];
        let mut cur = target;
        while let Some(p) = self.event_parent(cur, &ev.ty, ev.composed, origin_root) {
            path.push((p, self.retarget(target, p)));
            cur = p;
        }
        path
    }
}

/// Bindings hook invoked for each listener. Must not panic; JS exceptions are
/// reported by the implementation ("report the exception") and swallowed.
pub trait ListenerInvoker {
    fn invoke(&mut self, callback: ListenerHandle, current_target: TargetKey, target: TargetKey);
}

/// DOM "dispatch" for an initialized event. `event` is shared with the JS
/// Event object. Returns `false` if the event was canceled.
///
/// Activation behavior and legacy pre-activation (checkbox toggling) are
/// handled by `crate::activation` around this call.
pub fn dispatch(
    dom: &RefCell<Dom>,
    target: TargetKey,
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
) -> bool {
    let path = {
        let mut ev = event.borrow_mut();
        ev.dispatching = true;
        ev.target = Some(target);
        let d = dom.borrow();
        let path = d.event_path(target, &ev);
        ev.path = path.iter().map(|p| p.0).collect();
        path
    };
    let ty = event.borrow().ty.clone();

    // Capturing: from the outermost ancestor down to (and including) target.
    for k in (0..path.len()).rev() {
        let (cur, shadow_target) = path[k];
        {
            let mut ev = event.borrow_mut();
            if ev.stop_propagation {
                break;
            }
            ev.phase = if k == 0 || shadow_target == cur { Phase::AtTarget } else { Phase::Capturing };
            ev.target = Some(shadow_target);
        }
        invoke_listeners(dom, cur, shadow_target, &ty, event, invoker, true);
    }
    // Bubbling: target, then ancestors when bubbles.
    for (k, &(cur, shadow_target)) in path.iter().enumerate() {
        let at_target = k == 0 || shadow_target == cur;
        {
            let mut ev = event.borrow_mut();
            if ev.stop_propagation {
                break;
            }
            if !at_target && !ev.bubbles {
                continue;
            }
            ev.phase = if at_target { Phase::AtTarget } else { Phase::Bubbling };
            ev.target = Some(shadow_target);
        }
        invoke_listeners(dom, cur, shadow_target, &ty, event, invoker, false);
    }

    let mut ev = event.borrow_mut();
    ev.phase = Phase::None;
    ev.current_target = None;
    ev.dispatching = false;
    ev.stop_propagation = false;
    ev.stop_immediate = false;
    ev.path.clear();
    // The target stays retargeted against the outermost invocation target,
    // and is cleared when it is in a shadow tree ("clearTargets").
    let (final_target, clear) = {
        let d = dom.borrow();
        let last = path.last().map_or(target, |p| p.0);
        let t = d.retarget(target, last);
        let clear = matches!(t, TargetKey::Node(i) if d.in_shadow_tree(i));
        (t, clear)
    };
    ev.target = if clear { None } else { Some(final_target) };
    !ev.canceled
}

fn invoke_listeners(
    dom: &RefCell<Dom>,
    cur: TargetKey,
    target: TargetKey,
    ty: &LocalName,
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
    capture_pass: bool,
) {
    let listeners = dom.borrow().snapshot(cur, ty);
    if listeners.is_empty() {
        return;
    }
    event.borrow_mut().current_target = Some(cur);
    for l in listeners {
        if l.capture != capture_pass {
            continue;
        }
        {
            let mut d = dom.borrow_mut();
            if !d.listener_live(cur, l.id) {
                continue;
            }
            if l.once {
                d.remove_listener_id(cur, l.id);
                d.events.released.push((cur, l.callback));
            }
        }
        event.borrow_mut().in_passive_listener = l.passive;
        invoker.invoke(l.callback, cur, target);
        let mut ev = event.borrow_mut();
        ev.in_passive_listener = false;
        ev.current_target = Some(cur);
        if ev.stop_immediate {
            break;
        }
    }
}
