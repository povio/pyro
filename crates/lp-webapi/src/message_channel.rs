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
//! Gaps (explicit): transferring anything, including MessagePorts, throws
//! NotSupportedError (`postMessage(m, [port])` or `{transfer}`), so ports never
//! leave their realm; a MessagePort is not cloneable as data
//! (DataCloneError); there is no `messageerror` source because same-realm
//! deserialization cannot fail.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, pin_target, target_object, unpin_target, EventTarget, EventTargetData};
use lp_dom::events::TargetKey;

pub(crate) struct PortInner {
    target: EventTargetData,
    realm: WeakRealm,
    peer: RefCell<Weak<PortInner>>,
    queue: RefCell<VecDeque<Persistent>>,
    started: Cell<bool>,
    closed: Cell<bool>,
    /// Whether the wrapper is currently pinned (see `update_pin`).
    pinned: Cell<bool>,
}

impl PortInner {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }

    /// Queues `data` (already cloned into this port's realm).
    fn enqueue(self: &Rc<Self>, data: Persistent) {
        if self.closed.get() {
            return;
        }
        self.queue.borrow_mut().push_back(data);
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
            && (self.peer.borrow().upgrade().is_some() || !self.queue.borrow().is_empty());
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
        let Some(data) = self.queue.borrow_mut().pop_front() else { return };
        let ev = EventCore::trusted(self.page(), EventInterface::MessageEvent, "message", false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.message_data = Some(data);
            f.origin = String::new();
        }
        fire(self.page(), self.target.key, &ev);
        self.update_pin();
    }

    fn start(self: &Rc<Self>) {
        if self.started.get() || self.closed.get() {
            return;
        }
        self.started.set(true);
        self.update_pin();
        for _ in 0..self.queue.borrow().len() {
            self.schedule_one();
        }
    }

    fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.queue.borrow_mut().clear();
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
            let options = match a.opt(1) {
                // `postMessage(message, transfer)`: a sequence is the transfer list.
                Some(v) if v.is_array() => {
                    let o = s.object();
                    o.set("transfer", v)?;
                    o.value()
                }
                Some(v) => v,
                None => s.undefined(),
            };
            // Clone even when the message will be dropped, as in the spec
            // (errors are reported regardless of entanglement).
            let cloned = crate::structured_clone::clone_value(s, a.get(0), options)?;
            if d.0.closed.get() {
                return Ok(s.undefined());
            }
            if let Some(peer) = d.0.peer.borrow().upgrade() {
                peer.enqueue(Persistent::new(s, cloned));
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
        pinned: Cell::new(false),
    });
    Ok((s.wrap::<MessagePort>(PortData(inner.clone()), Some(key))?, inner))
}

/// The channel references its ports only through traced hidden properties
/// on its wrapper (as URL/URLSearchParams), so a listener or expando that
/// references the channel cannot form a GC-invisible cycle.
pub struct ChannelData {
    realm: WeakRealm,
    identity: lp_js::IdentityKey,
}

fn channel_port<'s>(s: &Scope<'s>, c: &ChannelData, name: &str) -> JsResult<lp_js::Value<'s>> {
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
