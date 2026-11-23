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

/// See [`Dom::set_listener_connected_hook`]: the node's index, whether it is
/// marked [`Dom::mark_pin_on_connect`], and its listeners' callbacks.
pub type ListenerConnectedHook = std::rc::Rc<dyn Fn(u32, bool, &[ListenerHandle])>;

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
    /// Told of each node [`ListenerStore::reconnected`] records, as it is
    /// recorded (`Dom::set_listener_connected_hook`).
    connected_hook: Option<ListenerConnectedHook>,
    /// Document -> Window key for event paths.
    windows: FxHashMap<u32, u64>,
    /// A listener for a CSS animation or transition event was ever added.
    animation_listeners: bool,
    /// A `formdata` listener was ever added.
    formdata_listeners: bool,
    /// Non-node target -> its event path parent (an IndexedDB request's
    /// transaction, a transaction's connection). Empty unless such targets exist.
    other_parents: FxHashMap<u64, u64>,
    /// A listener for a transition event was ever added.
    transition_listeners: bool,
    /// How many listeners `map` holds for each event type, over every
    /// target (nodes, documents, windows, the other targets): what tells a
    /// dispatch nobody can hear apart ([`Dom::listens_for`]). Kept by the
    /// functions below, the only ones that add to or take from `map`.
    by_type: FxHashMap<LocalName, u32>,
}

impl ListenerStore {
    /// Drops the target's listeners, keeping a document's window link.
    fn erase(&mut self, key: TargetKey) {
        if let Some(list) = self.map.remove(&key) {
            for l in &list {
                Self::uncount(&mut self.by_type, &l.ty);
            }
            self.released.extend(list.into_iter().map(|l| (key, l.callback)));
        }
    }

    /// One listener for `ty` left `map`.
    fn uncount(by_type: &mut FxHashMap<LocalName, u32>, ty: &LocalName) {
        if let Some(n) = by_type.get_mut(ty) {
            *n -= 1;
            if *n == 0 {
                by_type.remove(ty);
            }
        }
    }

    pub(crate) fn forget(&mut self, key: TargetKey) {
        // Called for every freed node (a closed document's whole tree), most
        // of which never had a listener: skip the lookups in empty tables.
        if !self.map.is_empty() {
            self.erase(key);
        }
        if let TargetKey::Other(k) = key {
            if !self.other_parents.is_empty() {
                self.other_parents.remove(&k);
            }
        }
        if let TargetKey::Node(i) = key {
            if !self.windows.is_empty() {
                self.windows.remove(&i);
            }
            if !self.pin_on_connect.is_empty() {
                self.pin_on_connect.remove(&i);
            }
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

    /// Makes `parent` the event path parent of the non-node target `child`
    /// ("get the parent"), or removes its parent. Node targets are ignored.
    pub fn set_target_parent(&mut self, child: TargetKey, parent: Option<TargetKey>) {
        let TargetKey::Other(c) = child else { return };
        match parent {
            Some(TargetKey::Other(p)) => {
                self.events.other_parents.insert(c, p);
            }
            _ => {
                self.events.other_parents.remove(&c);
            }
        }
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
        if !st.transition_listeners && is_animation_event_type(ty) {
            st.animation_listeners = true;
            st.transition_listeners = ty.contains("ransition");
        }
        if !st.formdata_listeners && ty == "formdata" {
            st.formdata_listeners = true;
        }
        let ty = LocalName::from(ty);
        *st.by_type.entry(ty.clone()).or_insert(0) += 1;
        list.push(Listener {
            id: st.next_id,
            ty,
            callback,
            capture: opts.capture,
            once: opts.once,
            passive: opts.passive.unwrap_or(false),
        });
        true
    }

    /// Whether dispatching an event of type `ty` in this tree could run a
    /// listener: one is registered for `ty` (or for the legacy type a
    /// trusted event of that type falls back to) on some target, be it a
    /// node, a document, a window or any other target, connected or not,
    /// for the capture or the bubble phase. `false` means a dispatch runs
    /// no script, whatever its target and path. Event handler attributes
    /// and properties are listeners once the bindings registered them.
    pub fn listens_for(&self, ty: &str) -> bool {
        let by_type = &self.events.by_type;
        if by_type.is_empty() {
            return false;
        }
        by_type.contains_key(&LocalName::from(ty))
            || legacy_event_type(ty).is_some_and(|legacy| by_type.contains_key(&LocalName::from(legacy)))
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
        ListenerStore::uncount(&mut self.events.by_type, &l.ty);
        self.events.released.push((target, l.callback));
        Some(l.callback)
    }

    /// Whether a listener for a CSS animation or transition event (or one of
    /// their legacy types) was ever added in this tree: only then are those
    /// events worth working out.
    pub fn has_animation_listeners(&self) -> bool {
        self.events.animation_listeners
    }

    /// Whether a `formdata` listener was ever added in this tree: only then
    /// does constructing a form's entry list fire the event.
    pub fn has_formdata_listeners(&self) -> bool {
        self.events.formdata_listeners
    }

    /// Whether a listener for a transition event (or `webkitTransitionEnd`)
    /// was ever added in this tree: only then do style changes need a
    /// before-change style.
    pub fn has_transition_listeners(&self) -> bool {
        self.events.transition_listeners
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
        ListenerStore::uncount(&mut self.events.by_type, &listener.ty);
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
        let hook = self.events.connected_hook.clone();
        let mut callbacks = Vec::new();
        let mut i = root;
        while i != NIL {
            let listeners = self.events.map.get(&TargetKey::Node(i));
            let pin = self.events.pin_on_connect.contains(&i);
            if listeners.is_some() || pin {
                self.events.reconnected.push(i);
                if let Some(hook) = &hook {
                    callbacks.clear();
                    callbacks.extend(listeners.into_iter().flatten().map(|l| l.callback));
                    hook(i, pin, &callbacks);
                }
            }
            i = self.next_shadow_including_rev(i, root);
        }
    }

    /// Sets what is told of each node that holds listeners (or is marked
    /// [`Dom::mark_pin_on_connect`]) as an insertion connects it, before the
    /// insertion returns: the bindings keep its wrapper from then on, not
    /// from the next safe point ([`Dom::take_reconnected_listener_nodes`]
    /// still reports it there), so a collection later in the same task
    /// cannot take the wrapper and its listeners. Called with the DOM
    /// borrowed: the hook may not use it.
    pub fn set_listener_connected_hook(&mut self, hook: Option<ListenerConnectedHook>) {
        self.events.connected_hook = hook;
    }

    pub fn has_listener_connected_hook(&self) -> bool {
        self.events.connected_hook.is_some()
    }

    /// Whether this target has a registered listener of the given type.
    pub fn has_listener_for(&self, target: TargetKey, ty: &str) -> bool {
        self.events.map.get(&target).is_some_and(|list| list.iter().any(|listener| &*listener.ty == ty))
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

    /// Pending cleanup makes the next task checkpoint observable to the binding.
    pub fn has_released_listeners(&self) -> bool {
        !self.events.released.is_empty()
    }

    /// Handles whose listener entries were removed natively; bindings
    /// release their storage.
    pub fn take_released_listeners(&mut self) -> Vec<(TargetKey, ListenerHandle)> {
        std::mem::take(&mut self.events.released)
    }

    /// The listeners of `target` for `ty` into `out` (cleared first): the
    /// dispatch's one scratch list, so a path of nodes without listeners
    /// allocates nothing.
    fn snapshot_into(&self, target: TargetKey, ty: &LocalName, out: &mut Vec<Listener>) {
        out.clear();
        if let Some(list) = self.events.map.get(&target) {
            out.extend(list.iter().filter(|l| &l.ty == ty).cloned());
        }
    }
    fn listener_live(&self, target: TargetKey, id: u32) -> bool {
        self.events.map.get(&target).is_some_and(|l| l.iter().any(|x| x.id == id))
    }
    fn remove_listener_id(&mut self, target: TargetKey, id: u32) {
        let st = &mut self.events;
        if let Some(list) = st.map.get_mut(&target) {
            if let Some(pos) = list.iter().position(|l| l.id == id) {
                let l = list.remove(pos);
                ListenerStore::uncount(&mut st.by_type, &l.ty);
            }
        }
    }

    pub(crate) fn id_of_unchecked(&self, idx: u32) -> NodeId {
        // Generation lookup without liveness check; callers verify.
        NodeId { idx, gen: self.generation_at(idx) }
    }

    /// Shadow-including parent for event paths ("get the parent").
    fn event_parent(&self, key: TargetKey, ev_ty: &LocalName, composed: bool, origin_root: u32) -> Option<TargetKey> {
        let TargetKey::Node(i) = key else {
            let TargetKey::Other(k) = key else { return None };
            // Nearly always empty: no hashing for XHR, AbortSignal, Window.
            if self.events.other_parents.is_empty() {
                return None;
            }
            return self.events.other_parents.get(&k).map(|&p| TargetKey::Other(p));
        };
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
    /// [`Dom::retarget`] for keys that may name a freed node (an event's
    /// relatedTarget is only as alive as the bindings keep it).
    pub fn retarget_live(&self, a: TargetKey, b: TargetKey) -> TargetKey {
        let live = |key: TargetKey| !matches!(key, TargetKey::Node(i) if !self.slot_live(i));
        if live(a) && live(b) {
            self.retarget(a, b)
        } else {
            a
        }
    }
    fn is_html_form(&self, key: TargetKey) -> bool {
        matches!(key, TargetKey::Node(i) if self.slot_live(i)
            && self.n(i).element().is_some_and(|e| e.is_html() && &*e.name.local == "form"))
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

    /// `Event.composedPath()`: the dispatch path without the nodes of
    /// closed shadow trees the current target cannot see (the "unclosed
    /// node" relation Chrome applies). Empty when not dispatching.
    pub fn composed_path(&self, ev: &EventState) -> Vec<TargetKey> {
        let Some(cur) = ev.current_target else { return Vec::new() };
        let closed_root = |k: &TargetKey| {
            matches!(*k, TargetKey::Node(i) if self.slot_live(i)
                && matches!(&self.n(i).kind, NodeKind::Fragment(f) if f.shadow == Some(crate::ShadowRootMode::Closed)))
        };
        // Every shadow tree the path passes through has its root on the
        // path, so a path without a closed root (nearly all) hides nothing.
        if !ev.path.iter().any(closed_root) {
            return ev.path.clone();
        }
        // A non-node current target (the Window) sees what its document does.
        let viewer = match cur {
            TargetKey::Node(i) if self.slot_live(i) => i,
            _ => match ev.path.iter().rev().find_map(|k| match *k {
                TargetKey::Node(i) if self.slot_live(i) => Some(i),
                _ => None,
            }) {
                Some(i) => i,
                None => return ev.path.clone(),
            },
        };
        ev.path
            .iter()
            .copied()
            .filter(|&k| match k {
                TargetKey::Node(i) if self.slot_live(i) => self.is_unclosed_node_of(i, viewer),
                _ => true,
            })
            .collect()
    }

    /// `a` is an unclosed node of `b`: `a`'s root is a shadow-including
    /// inclusive ancestor of `b`, or an open shadow root whose host is.
    fn is_unclosed_node_of(&self, mut a: u32, b: u32) -> bool {
        loop {
            let root = self.root_idx(a);
            if self.shadow_including_ancestor(root, b) {
                return true;
            }
            match &self.n(root).kind {
                NodeKind::Fragment(f) if f.shadow == Some(crate::ShadowRootMode::Open) => a = f.host,
                _ => return false,
            }
        }
    }

    /// [`Dom::event_path`] for a dispatch with the legacy target override
    /// flag (DOM "dispatch" step 5): `target`'s own entry has
    /// `target_override` as its shadow-adjusted target, so its listeners run
    /// at the target phase with the event's `target` being `target_override`.
    /// HTML fires a document's `load`, `pageshow`, `pagehide`, `unload` and
    /// `beforeunload` at its Window that way, with the document as override
    /// (a Window has no parent: it is the whole path).
    pub fn event_path_with_target_override(
        &self,
        target: TargetKey,
        target_override: TargetKey,
        ev: &EventState,
    ) -> Vec<(TargetKey, TargetKey)> {
        let mut path = self.event_path(target, ev);
        if let Some(first) = path.first_mut() {
            first.1 = target_override;
        }
        path
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

/// One item of an event's path (DOM "append to an event path"), as built
/// before any listener runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PathItem {
    pub invocation_target: TargetKey,
    /// The event's `target` while this item's listeners run.
    pub target: TargetKey,
    /// The event's `relatedTarget` while this item's listeners run: the
    /// event's own, retargeted against the invocation target.
    pub related_target: Option<TargetKey>,
    at_target: bool,
}

impl PathItem {
    /// The item's shadow-adjusted target is non-null: the dispatch target
    /// itself (whose `target` is the override of a dispatch with the legacy
    /// target override flag), or a shadow host the event is retargeted to.
    /// Its listeners run at the target phase whether or not the event
    /// bubbles.
    pub fn at_target(&self) -> bool {
        self.at_target
    }
}

/// DOM "dispatch" for an initialized event. `event` is shared with the JS
/// Event object. Returns `false` if the event was canceled.
///
/// Activation behavior and legacy pre-activation (checkbox toggling) are
/// handled by the bindings around [`dispatch_with`].
pub fn dispatch(
    dom: &RefCell<Dom>,
    target: TargetKey,
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
) -> bool {
    dispatch_with(dom, target, event, invoker, &mut |_| {})
}

/// [`dispatch`], calling `before_listeners` with the event's path once it is
/// built and before any listener runs (dispatch steps 6.5 to 6.12: the
/// caller picks the activation target from it and runs its
/// legacy-pre-activation behavior). It is not called for an event that is
/// not dispatched at all (step 6's condition).
pub fn dispatch_with(
    dom: &RefCell<Dom>,
    target: TargetKey,
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
    before_listeners: &mut dyn FnMut(&[PathItem]),
) -> bool {
    let path = dom.borrow().event_path(target, &event.borrow());
    dispatch_on_path(dom, target, &path, event, invoker, before_listeners)
}

/// [`dispatch_with`] on `path`, the event's path as [`Dom::event_path`]
/// built it for `target` just now (a caller that needs the path before
/// dispatch, to hold its nodes, builds it once).
pub fn dispatch_on_path(
    dom: &RefCell<Dom>,
    target: TargetKey,
    path: &[(TargetKey, TargetKey)],
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
    before_listeners: &mut dyn FnMut(&[PathItem]),
) -> bool {
    let (path, clear_targets) = {
        let mut ev = event.borrow_mut();
        ev.dispatching = true;
        let d = dom.borrow();
        let related = ev.related_target;
        // Steps 4 and 6: an event whose relatedTarget retargets to the target
        // itself (a node of the target's own shadow tree) is not dispatched.
        // Its targets are reset, as for any event that pointed into a shadow
        // tree.
        if related.is_some_and(|r| r != target && d.retarget_live(r, target) == target) {
            (Vec::new(), true)
        } else {
            let mut items: Vec<PathItem> = Vec::with_capacity(path.len());
            for (k, &(cur, shadow_target)) in path.iter().enumerate() {
                let related_target = related.map(|r| d.retarget_live(r, cur));
                // Step 6.9.7: the path ends below a shadow host that is the
                // relatedTarget as seen from there.
                if k > 0 && shadow_target == cur && related_target == Some(cur) {
                    break;
                }
                let at_target = k == 0 || shadow_target == cur;
                items.push(PathItem { invocation_target: cur, target: shadow_target, related_target, at_target });
            }
            // UI Events permits a trusted wheel input to be non-cancelable
            // when no non-passive wheel listener is on its dispatch path.
            // A legacy mousewheel listener can also cancel the default scroll.
            if ev.is_trusted && ev.cancelable && &*ev.ty == "wheel" {
                ev.cancelable = items.iter().any(|item| {
                    d.events.map.get(&item.invocation_target).is_some_and(|listeners| {
                        listeners.iter().any(|l| !l.passive && matches!(&*l.ty, "wheel" | "mousewheel"))
                    })
                });
            }
            // Steps 6.10 and 6.11 ("clearTargets"), decided before listeners
            // can move nodes: the outermost target or relatedTarget the event
            // exposes is a node of a shadow tree.
            let in_shadow = |key: TargetKey| matches!(key, TargetKey::Node(i) if d.slot_live(i) && d.in_shadow_tree(i));
            let clear = items
                .iter()
                .rev()
                .find(|item| item.at_target())
                .is_some_and(|item| in_shadow(item.target) || item.related_target.is_some_and(in_shadow));
            ev.target = Some(target);
            // The event object keeps its path list between dispatches.
            ev.path.clear();
            ev.path.extend(items.iter().map(|p| p.invocation_target));
            (items, clear)
        }
    };
    if !path.is_empty() {
        before_listeners(&path);
    }
    let ty = event.borrow().ty.clone();
    // With no listener for the type on any target (`Dom::listens_for`) the
    // two passes below would run nothing and leave nothing behind: what
    // they set on the event on their way is reset or set again after them.
    // Nothing has run script since the path was built, so none was added.
    let heard = dom.borrow().listens_for(&ty);
    // One listener list for the whole dispatch (`Dom::snapshot_into`).
    let mut listeners = Vec::new();
    if heard {
        // Capturing: from the outermost ancestor down to (and including) target.
        for item in path.iter().rev() {
            {
                let mut ev = event.borrow_mut();
                if ev.stop_propagation {
                    break;
                }
                ev.phase = if item.at_target() { Phase::AtTarget } else { Phase::Capturing };
                ev.target = Some(item.target);
                ev.related_target = item.related_target;
            }
            invoke_listeners(dom, item.invocation_target, item.target, &ty, event, invoker, true, &mut listeners);
        }
        // Bubbling: target, then ancestors when bubbles.
        for item in &path {
            let at_target = item.at_target();
            {
                let mut ev = event.borrow_mut();
                if ev.stop_propagation {
                    break;
                }
                if !at_target && !ev.bubbles {
                    continue;
                }
                // A `submit` or `reset` event of a nested form (or of any other
                // descendant) does not reach the listeners of an ancestor form,
                // nor anything above it: Chromium, Gecko and WebKit all stop its
                // propagation there.
                if !at_target && matches!(&*ty, "submit" | "reset") && dom.borrow().is_html_form(item.invocation_target) {
                    ev.stop_propagation = true;
                    break;
                }
                ev.phase = if at_target { Phase::AtTarget } else { Phase::Bubbling };
                ev.target = Some(item.target);
                ev.related_target = item.related_target;
            }
            invoke_listeners(dom, item.invocation_target, item.target, &ty, event, invoker, false, &mut listeners);
        }
    }

    let mut ev = event.borrow_mut();
    ev.phase = Phase::None;
    ev.current_target = None;
    ev.dispatching = false;
    ev.stop_propagation = false;
    ev.stop_immediate = false;
    ev.path.clear();
    if clear_targets {
        ev.target = None;
        ev.related_target = None;
    } else if let Some(last) = path.last() {
        // The targets stay as the outermost invocation target saw them.
        ev.target = Some(last.target);
        ev.related_target = last.related_target;
    }
    !ev.canceled
}

#[allow(clippy::too_many_arguments)]
fn invoke_listeners(
    dom: &RefCell<Dom>,
    cur: TargetKey,
    target: TargetKey,
    ty: &LocalName,
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
    capture_pass: bool,
    listeners: &mut Vec<Listener>,
) {
    dom.borrow().snapshot_into(cur, ty, listeners);
    if listeners.is_empty() {
        // DOM "invoke" step 7: a trusted event no listener of the target was
        // registered for is retried under its legacy type (`animationend`
        // as `webkitAnimationEnd`), and reports that type meanwhile.
        let Some(legacy) = legacy_event_type(ty).filter(|_| event.borrow().is_trusted) else { return };
        let legacy = LocalName::from(legacy);
        dom.borrow().snapshot_into(cur, &legacy, listeners);
        if listeners.is_empty() {
            return;
        }
        event.borrow_mut().ty = legacy;
        run_listeners(dom, cur, target, listeners, event, invoker, capture_pass);
        event.borrow_mut().ty = ty.clone();
        return;
    }
    run_listeners(dom, cur, target, listeners, event, invoker, capture_pass);
}

/// The CSS animation and transition event types, with their legacy aliases.
fn is_animation_event_type(ty: &str) -> bool {
    matches!(ty.as_bytes().first(), Some(b'a' | b't' | b'w'))
        && matches!(
            ty,
            "animationstart"
                | "animationiteration"
                | "animationend"
                | "animationcancel"
                | "transitionrun"
                | "transitionstart"
                | "transitionend"
                | "transitioncancel"
                | "webkitAnimationStart"
                | "webkitAnimationIteration"
                | "webkitAnimationEnd"
                | "webkitTransitionEnd"
        )
}

/// The legacy event type a trusted event of type `ty` falls back to.
fn legacy_event_type(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "animationend" => "webkitAnimationEnd",
        "animationiteration" => "webkitAnimationIteration",
        "animationstart" => "webkitAnimationStart",
        "transitionend" => "webkitTransitionEnd",
        "wheel" => "mousewheel",
        _ => return None,
    })
}

/// DOM "inner invoke" over the listeners of `cur` registered for the event's type.
fn run_listeners(
    dom: &RefCell<Dom>,
    cur: TargetKey,
    target: TargetKey,
    listeners: &[Listener],
    event: &RefCell<EventState>,
    invoker: &mut dyn ListenerInvoker,
    capture_pass: bool,
) {
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
