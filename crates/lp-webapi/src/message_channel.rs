//! `MessageChannel` / `MessagePort` (HTML §9.4) for same-realm messaging,
//! and `BroadcastChannel` (HTML §9.5) across the documents of a browser
//! context (see the end of this file).
//!
//! Each port is a non-node EventTarget. `postMessage` structured-clones the
//! message (the realm's original `structuredClone`, so non-cloneable values
//! throw DataCloneError) and queues it on the entangled port. A started port
//! delivers each message as its own task, in FIFO order, as a trusted
//! `MessageEvent` (`origin` "", no `source`, no `ports`). Messages wait while
//! the port is not started; `start()` or assigning `onmessage` starts it,
//! `addEventListener("message")` does not.
//!
//! `close()` disentangles both ports; later posts on either side are dropped
//! and the closed port drops its undelivered messages (upstream
//! MessagePort.zig, Chromium).
//!
//! Lifetime: a started, open port that is entangled or still holds
//! undelivered messages is pinned so its listeners keep running without a JS
//! reference (HTML keeps such ports alive). The pin is re-evaluated on
//! enqueue, start, delivery, close (including the peer's) and when the peer
//! is collected, and every pin is released with the page. A queued but
//! unstarted port is not pinned.
//!
//! Transfer: a MessagePort in a transfer list (`postMessage(m, [port])`,
//! `{transfer}`, `structuredClone`, `window.postMessage`) moves to the
//! receiving realm, which may be another frame or page of this thread's VM: a
//! new port there takes over the entanglement and the undelivered messages,
//! and the original is detached (posts on it are dropped; transferring it
//! again throws DataCloneError). ArrayBuffers in the list are detached.
//! Messages are cloned into the receiving port's realm when posted.
//!
//! Across threads (to or from a dedicated worker, through `Worker`'s or the
//! worker global's `postMessage` or another port already there), the pair
//! switches to a `port_link::Pipe`: messages are serialized
//! (`structured_clone::encode_message`), queued on the pipe and announced to
//! the receiving thread's loop by one posted task each; the receiving port
//! deserializes each when it delivers it, firing `messageerror` if that fails.
//! Such a port is registered in `REMOTE` (thread-local, by id) while it
//! stands for a side of a pipe, so those tasks find it.
//!
//! Gaps (explicit): a MessagePort is not cloneable as data (DataCloneError).
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, WeakRealm};
use rustc_hash::FxHashMap;

use crate::port_link::{Link, Owner, Sent, TransitEnd};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, pin_target, target_object, unpin_target, EventTarget, EventTargetData};
use lp_dom::events::TargetKey;

pub(crate) struct PortInner {
    target: EventTargetData,
    realm: WeakRealm,
    peer: RefCell<Weak<PortInner>>,
    /// Undelivered messages: data and the event's `ports` array, both already
    /// in this port's realm.
    queue: RefCell<VecDeque<(Persistent, Option<Persistent>)>>,
    started: Cell<bool>,
    closed: Cell<bool>,
    /// Transferred away: this object no longer stands for the port.
    detached: Cell<bool>,
    /// Whether the wrapper is currently pinned (see `update_pin`).
    pinned: Cell<bool>,
    /// The side of a cross-thread pipe this port stands for (then `peer`
    /// is empty), with its id in `REMOTE`.
    remote: RefCell<Option<Link>>,
    remote_id: Cell<u64>,
}

thread_local! {
    /// This thread's ports that stand for a side of a pipe, by id: the
    /// target of the tasks other threads post for them.
    static REMOTE: RefCell<FxHashMap<u64, Weak<PortInner>>> = RefCell::default();
}
static NEXT_REMOTE: AtomicU64 = AtomicU64::new(1);

/// A message arrived on the pipe side port `id` stands for (a task posted
/// by the sending thread): a started port delivers one.
pub(crate) fn wake_remote(id: u64) {
    let port = REMOTE.with(|r| r.borrow().get(&id).and_then(Weak::upgrade));
    if let Some(port) = port.filter(|p| p.started.get()) {
        port.deliver_next();
    }
}

/// The other side of port `id`'s pipe closed: its pin may go.
pub(crate) fn peer_closed_remote(id: u64) {
    let port = REMOTE.with(|r| r.borrow().get(&id).and_then(Weak::upgrade));
    if let Some(port) = port {
        port.update_pin();
    }
}

/// Makes `port` stand for `link` on this thread. False (and `link` closed)
/// when its page cannot receive tasks from other threads.
fn attach_remote(port: &Rc<PortInner>, link: Link) -> bool {
    let Some(poster) = port.page().remote_poster().filter(|_| !port.page().is_disposed()) else {
        link.close();
        return false;
    };
    let id = NEXT_REMOTE.fetch_add(1, Ordering::Relaxed);
    port.remote_id.set(id);
    REMOTE.with(|r| r.borrow_mut().insert(id, Rc::downgrade(port)));
    if link.set_owner(Some(Owner { poster, id })) {
        port.closed.set(true);
    }
    *port.remote.borrow_mut() = Some(link);
    true
}

/// Stops `port` standing for its pipe side (it moves or closes).
fn detach_remote(port: &PortInner) -> Option<Link> {
    let link = port.remote.borrow_mut().take()?;
    let id = port.remote_id.replace(0);
    let _ = REMOTE.try_with(|r| r.try_borrow_mut().map(|mut r| r.remove(&id)));
    Some(link)
}

impl PortInner {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }

    /// Queues `data` (already cloned into this port's realm).
    fn enqueue(self: &Rc<Self>, data: Persistent, ports: Option<Persistent>) {
        if self.closed.get() {
            return;
        }
        self.queue.borrow_mut().push_back((data, ports));
        if self.started.get() {
            self.schedule_one();
        }
        self.update_pin();
    }

    /// Keeps the wrapper (and so this `PortInner` and its listeners) alive
    /// exactly while the port can still deliver: started, open, and either
    /// entangled or holding undelivered messages (HTML keeps such ports
    /// alive). Re-evaluated on enqueue, start, delivery and close (including
    /// the peer's), so the pin is dropped once it is no longer needed. A
    /// queued but unstarted port is not pinned: its messages wait for
    /// `start()`, so if nothing can reach the port to start it, it may go.
    fn update_pin(&self) {
        let desired = self.started.get()
            && !self.closed.get()
            && (self.peer.borrow().upgrade().is_some()
                || !self.queue.borrow().is_empty()
                || self.remote.borrow().as_ref().is_some_and(|l| l.peer_open() || l.pending() > 0));
        if desired == self.pinned.get() {
            return;
        }
        let Some(realm) = self.realm.upgrade() else { return };
        let Ok(s) = realm.scope() else { return };
        if desired {
            if let Ok(Some(wrapper)) = target_object(&s, self.page(), self.target.key) {
                pin_target(&s, self.page(), self.target.key, wrapper);
                self.pinned.set(true);
            }
        } else {
            unpin_target(&s, self.page(), self.target.key);
            self.pinned.set(false);
        }
    }

    /// One task per queued message keeps FIFO order and task boundaries.
    fn schedule_one(self: &Rc<Self>) {
        let port = Rc::downgrade(self);
        self.page().post_task(Box::new(move || {
            if let Some(port) = port.upgrade() {
                port.deliver_next();
            }
        }));
    }

    fn deliver_next(&self) {
        if self.closed.get() || self.page().is_disposed() {
            return;
        }
        let local = self.queue.borrow_mut().pop_front().map(|(data, ports)| (Some(data), ports));
        let Some((data, ports)) = local.or_else(|| self.take_remote()) else { return };
        let Some(data) = data else {
            let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, "messageerror", false, false);
            fire(self.page(), self.target.key, &ev);
            self.update_pin();
            return;
        };
        let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, "message", false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.message_data = Some(data);
            f.message_ports = ports;
            f.origin = String::new();
        }
        fire(self.page(), self.target.key, &ev);
        self.update_pin();
    }

    /// The next message of the pipe side this port stands for, deserialized
    /// into its realm: `(None, None)` for one that cannot be (`messageerror`).
    #[allow(clippy::type_complexity)]
    fn take_remote(&self) -> Option<(Option<Persistent>, Option<Persistent>)> {
        let link = self.remote.borrow().clone()?;
        let message = link.pop()?;
        let realm = self.realm.upgrade()?;
        let s = realm.scope().ok()?;
        match crate::structured_clone::decode_with_ports(&s, &message) {
            Ok((value, ports)) => {
                Some((Some(Persistent::new(&s, value)), ports.map(|p| Persistent::new(&s, p.value()))))
            }
            Err(JsError::Terminated) => None,
            Err(_) => Some((None, None)),
        }
    }

    fn start(self: &Rc<Self>) {
        if self.started.get() || self.closed.get() {
            return;
        }
        self.started.set(true);
        self.update_pin();
        let queued = self.queue.borrow().len() + self.remote.borrow().as_ref().map_or(0, Link::pending);
        for _ in 0..queued {
            self.schedule_one();
        }
    }

    fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        let queue = std::mem::take(&mut *self.queue.borrow_mut());
        drop(queue);
        if let Some(link) = detach_remote(self) {
            link.close();
        }
        self.update_pin();
        if let Some(peer) = self.peer.take().upgrade() {
            peer.peer.replace(Weak::new());
            // The peer keeps its pin only while it still has undelivered
            // messages (a sender's close does not drop queued messages).
            peer.update_pin();
        }
    }
}

impl Drop for PortInner {
    /// When a port's native state goes away (its wrapper was collected), its
    /// surviving peer can no longer be posted to. The drop only records the
    /// peer (pure Rust, no JS, independent of page hooks); `PageDom::after_task`
    /// reconciles its pin at the next safe point, so a started, drained
    /// receiver whose sender became unreachable is released instead of being
    /// retained until page disposal. Queued messages keep it pinned until
    /// delivered.
    fn drop(&mut self) {
        // No port stands for this pipe side any more.
        if let Some(link) = detach_remote(self) {
            link.close();
        }
        let Some(peer) = self.peer.get_mut().upgrade() else { return };
        let page = peer.page().clone();
        if !page.is_disposed() {
            page.pending_port_reconcile.borrow_mut().push(Rc::downgrade(&peer));
        }
    }
}

/// Safe-point step (`PageDom::after_task`): re-evaluates the pins of ports
/// whose peer was collected since the last checkpoint.
pub(crate) fn reconcile_ports(page: &PageDom) {
    let pending = std::mem::take(&mut *page.pending_port_reconcile.borrow_mut());
    for port in pending.iter().filter_map(Weak::upgrade) {
        port.update_pin();
    }
}

/// `PageDom::dispose`: the document's ports that stand for pipe sides
/// close them, so their partners on other threads let go. No JS runs.
pub(crate) fn dispose(page: &PageDom) {
    let ports: Vec<Rc<PortInner>> = REMOTE.with(|r| {
        let r = r.borrow();
        if r.is_empty() {
            return Vec::new();
        }
        r.values().filter_map(Weak::upgrade).filter(|p| std::ptr::eq(Rc::as_ptr(p.page()), page)).collect()
    });
    for port in ports {
        port.closed.set(true);
        if let Some(link) = detach_remote(&port) {
            link.close();
        }
    }
}

pub struct PortData(Rc<PortInner>);

pub struct MessagePort;
impl JsClass for MessagePort {
    const NAME: &'static str = "MessagePort";
    type Data = PortData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &PortData| &d.0.target);
        def.method("postMessage", 1, |s, d, a| {
            a.require(1, "MessagePort.postMessage")?;
            let options = crate::structured_clone::transfer_options(s, a.opt(1))?;
            if lists_port(s, options, &d.0)? {
                return Err(JsError::dom("DataCloneError", "A port cannot be transferred through itself."));
            }
            let remote = d.0.remote.borrow().clone();
            if let Some(link) = remote {
                // The receiving port is on another thread: serialize here,
                // deserialize there when it is delivered.
                let message = crate::structured_clone::encode_message(s, a.get(0), options)?;
                // Closed or transferred away while the message was cloned, or
                // posting the entangled port through this one: dropped.
                if d.0.closed.get() || message.carries_peer_of(&link) {
                    return Ok(s.undefined());
                }
                return match link.send(message) {
                    Ok(Sent::Queued) | Ok(Sent::Dropped) => Ok(s.undefined()),
                    Err((_message, why)) => Err(JsError::dom("DataCloneError", why)),
                };
            }
            // Clone even when the message will be dropped, as in the spec
            // (errors are reported regardless of entanglement), into the
            // realm of the port that receives it.
            let peer = if d.0.closed.get() { None } else { d.0.peer.borrow().upgrade() };
            // Posting the entangled port through this one loses the channel:
            // the transfer happens, the message is not delivered.
            let doomed = match &peer {
                Some(p) => lists_port(s, options, p)?,
                None => false,
            };
            let receiver = peer.as_ref().and_then(|p| p.realm.upgrade());
            match receiver.as_ref().map(|r| r.scope()).transpose()? {
                Some(rs) => {
                    let (cloned, ports) = crate::structured_clone::clone_into(s, &rs, a.get(0), options)?;
                    let ports =
                        (ports.get_atom("length")?.to_number()? > 0.0).then(|| Persistent::new(&rs, ports.value()));
                    if let Some(peer) = peer.filter(|_| !doomed) {
                        peer.enqueue(Persistent::new(&rs, cloned), ports);
                    }
                }
                None => {
                    crate::structured_clone::clone_value(s, a.get(0), options)?;
                }
            }
            Ok(s.undefined())
        });
        def.method("start", 0, |s, d, _a| {
            d.0.start();
            Ok(s.undefined())
        });
        def.method("close", 0, |s, d, _a| {
            d.0.close();
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, d| crate::target::event_handler(s, &d.0.target, "message"),
            |s, d, value| {
                crate::target::set_event_handler(s, &d.0.target, "message", value)?;
                // Setting onmessage implicitly starts the port (HTML §9.4.4).
                if value.as_object().is_some() {
                    d.0.start();
                }
                Ok(())
            },
        );
        def.accessor(
            "onmessageerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "messageerror"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "messageerror", value),
        );
    }
}

fn new_port<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<(Object<'s>, Rc<PortInner>)> {
    let target = EventTargetData::new(page);
    let TargetKey::Other(k) = target.key else { unreachable!() };
    let key = page.other_identity(k);
    let inner = Rc::new(PortInner {
        target,
        realm: s.realm().downgrade(),
        peer: RefCell::new(Weak::new()),
        queue: RefCell::new(VecDeque::new()),
        started: Cell::new(false),
        closed: Cell::new(false),
        detached: Cell::new(false),
        pinned: Cell::new(false),
        remote: RefCell::new(None),
        remote_id: Cell::new(0),
    });
    Ok((s.wrap::<MessagePort>(PortData(inner.clone()), Some(key))?, inner))
}

/// Whether the transfer list of `options` (an array) holds `port`.
fn lists_port(s: &Scope<'_>, options: lp_js::Value<'_>, port: &Rc<PortInner>) -> JsResult<bool> {
    let Some(list) = options.as_object().map(|o| o.get_atom("transfer")).transpose()?.and_then(|l| l.as_object())
    else {
        return Ok(false);
    };
    if !list.value().is_array() {
        return Ok(false);
    }
    let count = list.get_atom("length")?.to_number()? as u32;
    for i in 0..count {
        if s.unwrap::<MessagePort>(list.get_index(i)?).is_some_and(|p| Rc::ptr_eq(&p.0, port)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Structured clone's transfer step 1 for `value`: a new, unentangled port
/// in `s`'s realm when `value` is a MessagePort (DataCloneError if it was
/// transferred already), `None` when it is not transferable. Nothing changes
/// until [`complete_transfer`], so a clone that fails leaves the port as is.
pub(crate) fn prepare_transfer<'s>(s: &Scope<'s>, value: lp_js::Value<'s>) -> JsResult<Option<lp_js::Value<'s>>> {
    let Some(port) = s.unwrap::<MessagePort>(value) else { return Ok(None) };
    if port.0.detached.get() {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    let page = page_of(s)?;
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    Ok(Some(new_port(s, &page)?.0.value()))
}

/// Moves port `from` into `to` (made by [`prepare_transfer`] in `s`'s
/// realm): `to` takes over the entanglement and the undelivered messages
/// (cloned into `s`'s realm when it is another one), and `from` is detached.
pub(crate) fn complete_transfer(s: &Scope<'_>, from: lp_js::Value<'_>, to: lp_js::Value<'_>) -> JsResult<()> {
    let (Some(from), Some(to)) = (s.unwrap::<MessagePort>(from), s.unwrap::<MessagePort>(to)) else {
        return Err(JsError::dom("DataCloneError", "Invalid MessagePort transfer"));
    };
    let (from, to) = (from.0.clone(), to.0.clone());
    if from.detached.replace(true) {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    let same_realm = from.realm.upgrade().is_some_and(|r| r.id() == s.realm_id());
    let queued: Vec<_> = from.queue.borrow_mut().drain(..).collect();
    {
        let mut queue = to.queue.borrow_mut();
        for (data, ports) in queued {
            if same_realm {
                queue.push_back((data, ports));
            } else if let Ok(copy) = crate::structured_clone::clone_value(s, s.rehome(data.get(s)), s.undefined()) {
                queue.push_back((Persistent::new(s, copy), ports));
            }
        }
    }
    // A closed port moves closed: nothing reaches it any more.
    to.closed.set(from.closed.replace(true));
    if let Some(peer) = from.peer.take().upgrade() {
        peer.peer.replace(Rc::downgrade(&to));
        to.peer.replace(Rc::downgrade(&peer));
    }
    // A pipe side moves to the new port (whose page may be another one).
    if let Some(link) = detach_remote(&from) {
        attach_remote(&to, link);
    }
    from.update_pin();
    to.update_pin();
    Ok(())
}

/// The transfer-list step of the cross-thread format for `value`: whether
/// it is a MessagePort that can be transferred (DataCloneError when it was
/// already). Its page must be able to take tasks from other threads.
pub(crate) fn is_transferable_port(s: &Scope<'_>, value: lp_js::Value<'_>) -> JsResult<bool> {
    let Some(port) = s.unwrap::<MessagePort>(value) else { return Ok(false) };
    if port.0.detached.get() {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    if port.0.page().remote_poster().is_none() {
        return Err(JsError::dom("DataCloneError", "This MessagePort cannot be transferred to another thread."));
    }
    Ok(true)
}

thread_local! {
    /// Nesting of [`export_port`]: a port's undelivered messages may hold
    /// ports with undelivered messages of their own.
    static EXPORT_DEPTH: Cell<u32> = const { Cell::new(0) };
}
/// Past this nesting, the undelivered messages of an exported port are
/// dropped rather than serialized (bounds the native stack on hostile graphs).
const MAX_EXPORT_DEPTH: u32 = 16;

/// Detaches `value` (a MessagePort [`is_transferable_port`] accepted) for a
/// message to another thread. The pipe side it stands for travels with the
/// message, its undelivered messages first in its inbox; a port that had no
/// pipe yet gets a new one, whose other side its entangled port takes over.
pub(crate) fn export_port(s: &Scope<'_>, value: lp_js::Value<'_>) -> JsResult<TransitEnd> {
    let Some(port) = s.unwrap::<MessagePort>(value) else {
        return Err(JsError::dom("DataCloneError", "Invalid MessagePort transfer"));
    };
    let port = port.0.clone();
    if port.detached.replace(true) {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    let was_closed = port.closed.replace(true);
    let messages = serialize_queue(&port);
    let link = match detach_remote(&port) {
        Some(link) => link,
        None => {
            let (mine, theirs) = Link::pair();
            match port.peer.take().upgrade() {
                Some(peer) => {
                    peer.peer.replace(Weak::new());
                    attach_remote(&peer, theirs);
                    peer.update_pin();
                }
                None => theirs.close(),
            }
            mine
        }
    };
    link.requeue_front(messages);
    if was_closed {
        link.close();
    }
    port.update_pin();
    Ok(TransitEnd::new(link))
}

/// A leaving port's undelivered messages, serialized in its realm (the
/// ports they carry are transferred along).
fn serialize_queue(port: &PortInner) -> Vec<crate::structured_clone::WireMessage> {
    let queued: Vec<_> = port.queue.borrow_mut().drain(..).collect();
    if queued.is_empty() {
        return Vec::new();
    }
    let depth = EXPORT_DEPTH.with(|d| d.replace(d.get() + 1));
    let mut out = Vec::with_capacity(queued.len());
    if depth < MAX_EXPORT_DEPTH {
        let realm = port.realm.upgrade();
        if let Some(s) = realm.as_ref().and_then(|r| r.scope().ok()) {
            for (data, ports) in queued {
                let options = match ports {
                    Some(ports) => {
                        let o = s.object();
                        if o.set_atom("transfer", ports.get(&s)).is_err() {
                            continue;
                        }
                        o.value()
                    }
                    None => s.undefined(),
                };
                if let Ok(message) = crate::structured_clone::encode_message(&s, data.get(&s), options) {
                    out.push(message);
                }
            }
        }
    }
    EXPORT_DEPTH.with(|d| d.set(depth));
    out
}

/// The new port, in `s`'s realm, of a MessagePort that a message from
/// another thread transfers: it stands for the side the message carried.
pub(crate) fn import_port<'s>(s: &Scope<'s>, transit: &TransitEnd) -> JsResult<(lp_js::Value<'s>, Rc<PortInner>)> {
    let link = transit.claim().ok_or_else(|| JsError::dom("DataCloneError", "The message was already received"))?;
    let made = page_of(s).and_then(|page| {
        if page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        new_port(s, &page)
    });
    let (object, inner) = match made {
        Ok(made) => made,
        Err(e) => {
            link.close();
            return Err(e);
        }
    };
    if link.is_closed() {
        inner.closed.set(true);
    } else if !attach_remote(&inner, link) {
        return Err(JsError::dom("DataCloneError", "This realm cannot receive a MessagePort from another thread."));
    }
    Ok((object.value(), inner))
}

/// Closes a port [`import_port`] made that will never be reachable.
pub(crate) fn close_inner(port: &PortInner) {
    port.close();
}

/// The channel references its ports only through traced hidden properties
/// on its wrapper (as URL/URLSearchParams), so a listener or expando that
/// references the channel cannot form a GC-invisible cycle.
pub struct ChannelData {
    realm: WeakRealm,
    identity: lp_js::IdentityKey,
}

fn channel_port<'s>(s: &Scope<'s>, c: &ChannelData, name: &'static str) -> JsResult<lp_js::Value<'s>> {
    let realm = c.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let object = owner.lookup_identity(c.identity).ok_or(JsError::ContextDestroyed)?;
    let value = owner.get_hidden(object, name)?.ok_or(JsError::ContextDestroyed)?;
    // SAFETY: both realms share the VM; the receiver's traced property roots
    // the port wrapper throughout this getter.
    Ok(unsafe { lp_js::Value::from_raw(s.ctx(), value.as_raw()) })
}

pub struct MessageChannel;
impl JsClass for MessageChannel {
    const NAME: &'static str = "MessageChannel";
    type Data = ChannelData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let (o1, p1) = new_port(s, &page)?;
            let (o2, p2) = new_port(s, &page)?;
            p1.peer.replace(Rc::downgrade(&p2));
            p2.peer.replace(Rc::downgrade(&p1));
            let identity = page.event_identity(page.next_event_id());
            let data = ChannelData { realm: s.realm().downgrade(), identity };
            let object = s.wrap_new::<MessageChannel>(data, nt, Some(identity))?;
            s.set_hidden(object, "port1", o1.value())?;
            s.set_hidden(object, "port2", o2.value())?;
            Ok(object)
        });
        def.getter("port1", |s, c| channel_port(s, c, "port1"));
        def.getter("port2", |s, c| channel_port(s, c, "port2"));
    }
}

// ---------------------------------------------------------------------------
// BroadcastChannel
//
// A channel reaches every other open channel with the same name whose
// document has the same origin in the same browser context, in every page
// of this browser thread. `postMessage` serializes the message once in the
// sender's realm (`structured_clone::encode_message`, so non-cloneable
// values throw DataCloneError and nothing is transferred) and each receiver
// deserializes it in its own realm, in a task on its own page, as a trusted
// `MessageEvent` (`origin` the sender's, no `source`, no `ports`); a message
// that cannot be deserialized there fires `messageerror`. Channels of the
// sender's own document receive it too, the sending channel does not.
//
// Lifetime: an open channel is pinned so listeners added without keeping a
// JS reference keep running (HTML keeps a channel with an event listener
// alive); `close()` releases the pin, and every pin goes with the page. The
// registry holds channels weakly and is pruned as it is used.
//
// Gaps (explicit): workers have no BroadcastChannel; the "browser context"
// is the page's `ContextServices` (what pages of a CDP browser context
// share), so documents without an embedder each form their own.

thread_local! {
    static CHANNELS: RefCell<Vec<Weak<ChannelInner>>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ChannelInner {
    target: EventTargetData,
    realm: WeakRealm,
    name: String,
    /// The document's tuple origin, or a per-document key for an opaque one.
    origin: String,
    /// Identity of the browser context (`ContextServices` pointer).
    context: usize,
    closed: Cell<bool>,
}

impl ChannelInner {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }

    fn matches(&self, other: &ChannelInner) -> bool {
        self.context == other.context && self.origin == other.origin && self.name == other.name
    }

    /// Delivers `message` (already queued as a task on this page).
    fn deliver(&self, message: &crate::structured_clone::WireMessage, origin: &str) {
        if self.closed.get() || self.page().is_disposed() {
            return;
        }
        let Some(realm) = self.realm.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        let (ty, data) = match crate::structured_clone::decode_message(&scope, message) {
            Ok(value) => ("message", Some(Persistent::new(&scope, value))),
            Err(_) => ("messageerror", None),
        };
        let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, ty, false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.message_data = data;
            f.origin = origin.to_owned();
        }
        fire(self.page(), self.target.key, &ev);
    }

    fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                unpin_target(&s, self.page(), self.target.key);
            }
        }
        CHANNELS.with(|c| c.borrow_mut().retain(|w| w.upgrade().is_some_and(|c| !c.closed.get())));
    }
}

pub struct BroadcastData(Rc<ChannelInner>);

pub struct BroadcastChannel;
impl JsClass for BroadcastChannel {
    const NAME: &'static str = "BroadcastChannel";
    type Data = BroadcastData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &BroadcastData| &d.0.target);
        def.constructor(1, |s, nt, a| {
            a.require(1, "BroadcastChannel")?;
            let name = crate::util::dom_string(a.get(0))?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let origin = crate::storage::tuple_origin(s, &page).unwrap_or_else(|| format!("opaque:{}", page.id()));
            let target = EventTargetData::new(&page);
            let TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            let inner = Rc::new(ChannelInner {
                target,
                realm: s.realm().downgrade(),
                name,
                origin,
                context: Rc::as_ptr(&page.context_services()) as usize,
                closed: Cell::new(false),
            });
            let object = s.wrap_new::<BroadcastChannel>(BroadcastData(inner.clone()), nt, Some(key))?;
            pin_target(s, &page, inner.target.key, object);
            CHANNELS.with(|c| {
                let mut c = c.borrow_mut();
                c.retain(|w| w.upgrade().is_some_and(|c| !c.closed.get()));
                c.push(Rc::downgrade(&inner));
            });
            Ok(object)
        });
        def.getter("name", |s, d| Ok(s.string(&d.0.name)));
        def.method("postMessage", 1, |s, d, a| {
            a.require(1, "BroadcastChannel.postMessage")?;
            if d.0.closed.get() {
                return Err(JsError::dom("InvalidStateError", "Channel is closed"));
            }
            if d.0.page().is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let message = Rc::new(crate::structured_clone::encode_message(s, a.get(0), s.undefined())?);
            let origin = d.0.origin.clone();
            let receivers: Vec<Rc<ChannelInner>> = CHANNELS.with(|c| {
                c.borrow()
                    .iter()
                    .filter_map(Weak::upgrade)
                    .filter(|c| !c.closed.get() && !Rc::ptr_eq(c, &d.0) && c.matches(&d.0) && !c.page().is_disposed())
                    .collect()
            });
            for receiver in receivers {
                let (message, origin) = (message.clone(), origin.clone());
                let weak = Rc::downgrade(&receiver);
                receiver.page().post_task(Box::new(move || {
                    if let Some(receiver) = weak.upgrade() {
                        receiver.deliver(&message, &origin);
                    }
                }));
            }
            Ok(s.undefined())
        });
        def.method("close", 0, |s, d, _a| {
            d.0.close();
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, d| crate::target::event_handler(s, &d.0.target, "message"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "message", value),
        );
        def.accessor(
            "onmessageerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "messageerror"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "messageerror", value),
        );
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<MessagePort>();
    engine.register::<MessageChannel>();
    engine.register::<BroadcastChannel>();
}
