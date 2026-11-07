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
//! Across threads (a dedicated or service worker, another VM): a port in the
//! transfer list of a worker or service-worker message becomes one end of a
//! [`Link`]. The entangled port stays where it is and takes the other end.
//! Messages between the ends are serialized (`structured_clone::encode_message`)
//! into the receiving end's inbox; the thread that owns that end is woken with
//! a task, and a started port delivers one message per task, decoding it into
//! its realm (`messageerror` if it cannot). An end in transit (inside a message
//! not yet decoded) keeps its inbox until its new port claims it. Each inbox
//! holds at most `MAX_QUEUED_BYTES`; a post beyond that throws DataCloneError.
//! Closing either end, or the collection of its port, closes the link.
//!
//! Gaps (explicit): a MessagePort is not cloneable as data (DataCloneError),
//! including inside a worker message: only the transfer list carries ports there.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, WeakRealm};

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
    /// Entangled across threads instead of with `peer` (see the module docs).
    link: RefCell<Option<LinkEnd>>,
    /// Key in this thread's `LINKED` while `link` is set; 0 otherwise.
    link_id: Cell<u64>,
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
                || self.link.borrow().as_ref().is_some_and(LinkEnd::live));
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
        let Some((data, ports)) = self.queue.borrow_mut().pop_front() else { return };
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

    fn start(self: &Rc<Self>) {
        if self.started.get() || self.closed.get() {
            return;
        }
        self.started.set(true);
        self.update_pin();
        let queued = self.queue.borrow().len();
        for _ in 0..queued {
            self.schedule_one();
        }
        self.schedule_linked();
    }

    /// A task that delivers the next message of the link's inbox, if any.
    fn schedule_linked(self: &Rc<Self>) {
        if !self.started.get() || self.closed.get() {
            return;
        }
        if !self.link.borrow().as_ref().is_some_and(LinkEnd::has_mail) {
            return;
        }
        let port = Rc::downgrade(self);
        self.page().post_task(Box::new(move || {
            if let Some(port) = port.upgrade() {
                port.deliver_linked();
            }
        }));
    }

    fn deliver_linked(self: &Rc<Self>) {
        if self.closed.get() || self.page().is_disposed() || !self.started.get() {
            return;
        }
        let message = match self.link.borrow().as_ref() {
            Some(end) => end.take(),
            None => return,
        };
        let Some(message) = message else {
            self.update_pin();
            return;
        };
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                fire_wire_message(&s, self.page(), self.target.key, &message, String::new());
            }
        }
        self.update_pin();
        self.schedule_linked();
    }

    fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.queue.borrow_mut().clear();
        if let Some(end) = self.link.take() {
            end.close();
            self.unregister_link();
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
        if let Some(end) = self.link.get_mut().take() {
            end.close();
            let id = self.link_id.get();
            let _ = LINKED.try_with(|l| l.borrow_mut().remove(&id));
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
            let linked = d.0.link.borrow().clone();
            if let Some(end) = linked {
                // Serialized for another thread; errors are reported even
                // when the link is closed, as for a local port.
                let message = crate::structured_clone::encode_message(s, a.get(0), options)?;
                if !d.0.closed.get() {
                    end.send(message)?;
                }
                return Ok(s.undefined());
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
        link: RefCell::new(None),
        link_id: Cell::new(0),
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
    let Some(port) = s.unwrap::<MessagePort>(value) else {
        if crate::streams::is_stream(s, value)? {
            let page = page_of(s)?;
            return crate::streams::helper(s, &page, "transferPrepare", &[value]).map(Some);
        }
        return Ok(None);
    };
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
    if s.unwrap::<MessagePort>(from).is_none() && crate::streams::is_stream(s, from)? {
        let page = page_of(s)?;
        let (from, to) = (s.rehome(from), s.rehome(to));
        return crate::streams::helper(s, &page, "transferComplete", &[from, to]).map(|_| ());
    }
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
    if let Some(end) = from.link.take() {
        from.unregister_link();
        to.adopt_link(end);
    }
    if let Some(peer) = from.peer.take().upgrade() {
        peer.peer.replace(Rc::downgrade(&to));
        to.peer.replace(Rc::downgrade(&peer));
    }
    from.update_pin();
    to.update_pin();
    Ok(())
}

// ---------------------------------------------------------------------------
// Links: channels whose ends are on different threads (see the module docs).

/// Ids of linked ports, unique in the process (a link end names its port by
/// id on the port's thread).
static NEXT_LINKED: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// This thread's linked ports, by id, for the tasks that wake them.
    static LINKED: RefCell<FxHashMap<u64, Weak<PortInner>>> = RefCell::default();
}

/// One end's state in a [`Link`].
#[derive(Default)]
struct EndState {
    /// The thread and port that own this end; `None` while in transit.
    owner: Option<(crate::page::RemotePoster, u64)>,
    /// Messages for this end, oldest first, and their total size.
    inbox: VecDeque<crate::structured_clone::WireMessage>,
    bytes: usize,
}

#[derive(Default)]
struct LinkState {
    ends: [EndState; 2],
    closed: bool,
}

/// A channel whose ends may live on different threads.
#[derive(Default)]
pub(crate) struct Link {
    state: Mutex<LinkState>,
}

/// One end of a [`Link`]: what a transferred port becomes in a worker message.
#[derive(Clone)]
pub struct LinkEnd {
    link: Arc<Link>,
    side: usize,
}

impl LinkEnd {
    fn pair() -> (LinkEnd, LinkEnd) {
        let link = Arc::new(Link::default());
        (LinkEnd { link: link.clone(), side: 0 }, LinkEnd { link, side: 1 })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LinkState> {
        self.link.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn live(&self) -> bool {
        let state = self.lock();
        !state.closed || !state.ends[self.side].inbox.is_empty()
    }

    fn has_mail(&self) -> bool {
        !self.lock().ends[self.side].inbox.is_empty()
    }

    fn take(&self) -> Option<crate::structured_clone::WireMessage> {
        let mut state = self.lock();
        let end = &mut state.ends[self.side];
        let message = end.inbox.pop_front()?;
        end.bytes = end.bytes.saturating_sub(message.size());
        Some(message)
    }

    /// Queues `message` for the other end and wakes its thread.
    fn send(&self, message: crate::structured_clone::WireMessage) -> JsResult<()> {
        let wake = {
            let mut state = self.lock();
            if state.closed {
                return Ok(());
            }
            let other = &mut state.ends[1 - self.side];
            let size = message.size();
            if other.bytes > 0 && other.bytes.saturating_add(size) > crate::worker::MAX_QUEUED_BYTES {
                return Err(JsError::dom("DataCloneError", "The port's message queue is full"));
            }
            other.bytes += size;
            other.inbox.push_back(message);
            other.owner.clone()
        };
        if let Some((poster, id)) = wake {
            poster(Box::new(move || wake_linked(id)));
        }
        Ok(())
    }

    /// Closes the link: later posts on either end are dropped, this end's
    /// undelivered messages go, and the other end is told (its pin).
    pub(crate) fn close(&self) {
        let wake = {
            let mut state = self.lock();
            state.closed = true;
            let end = &mut state.ends[self.side];
            end.inbox.clear();
            end.bytes = 0;
            end.owner = None;
            state.ends[1 - self.side].owner.clone()
        };
        if let Some((poster, id)) = wake {
            poster(Box::new(move || wake_linked(id)));
        }
    }
}

/// Runs on a linked port's thread: it has mail, or its link changed.
fn wake_linked(id: u64) {
    let port = LINKED.with(|l| l.borrow().get(&id).and_then(Weak::upgrade));
    if let Some(port) = port {
        port.update_pin();
        port.schedule_linked();
    }
}

impl PortInner {
    /// Makes `end` this port's entanglement, owned by this thread.
    fn adopt_link(self: &Rc<Self>, end: LinkEnd) {
        let Some(poster) = self.page().remote_poster() else {
            end.close();
            return;
        };
        let id = NEXT_LINKED.fetch_add(1, Ordering::Relaxed);
        LINKED.with(|l| l.borrow_mut().insert(id, Rc::downgrade(self)));
        end.lock().ends[end.side].owner = Some((poster, id));
        self.link_id.set(id);
        *self.link.borrow_mut() = Some(end);
    }

    fn unregister_link(&self) {
        let id = self.link_id.replace(0);
        if id != 0 {
            LINKED.with(|l| l.borrow_mut().remove(&id));
        }
    }
}

/// Fires `message` (or `messageerror`) at `target`, with the event's `ports`
/// the ports it carried.
pub(crate) fn fire_wire_message(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    target: TargetKey,
    message: &crate::structured_clone::WireMessage,
    origin: String,
) {
    match crate::structured_clone::decode_message_with_ports(s, message) {
        Ok((value, ports)) => {
            let ev = EventCore::trusted(page, EventInterface::MessageEvent, "message", false, false);
            {
                let mut fields = ev.fields.borrow_mut();
                fields.message_data = Some(Persistent::new(s, value));
                fields.message_ports = ports.map(|p| Persistent::new(s, p));
                fields.origin = origin;
            }
            fire(page, target, &ev);
        }
        Err(JsError::Terminated) => {}
        Err(_) => {
            let ev = EventCore::trusted(page, EventInterface::MessageEvent, "messageerror", false, false);
            fire(page, target, &ev);
        }
    }
}

/// Whether `value` is a MessagePort that can still be transferred
/// (DataCloneError for a detached one).
pub(crate) fn is_transferable_port(s: &Scope<'_>, value: lp_js::Value<'_>) -> JsResult<bool> {
    let Some(port) = s.unwrap::<MessagePort>(value) else { return Ok(false) };
    if port.0.detached.get() {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    Ok(true)
}

/// Transfers `value`, a MessagePort, out of this thread: detaches it and
/// returns the link end its new port takes. A port entangled on this thread
/// links its peer to the other end; its undelivered messages travel with it.
pub(crate) fn transfer_out(s: &Scope<'_>, value: lp_js::Value<'_>) -> JsResult<LinkEnd> {
    let port = s.unwrap::<MessagePort>(value).ok_or_else(|| JsError::dom("DataCloneError", "Not a MessagePort"))?;
    let port = port.0.clone();
    if port.detached.replace(true) {
        return Err(JsError::dom("DataCloneError", "A MessagePort could not be cloned because it was detached."));
    }
    let closed = port.closed.replace(true);
    let end = match port.link.take() {
        Some(end) => {
            port.unregister_link();
            end.lock().ends[end.side].owner = None;
            end
        }
        None => {
            let (mine, theirs) = LinkEnd::pair();
            match port.peer.take().upgrade() {
                Some(peer) if !closed => {
                    peer.peer.replace(Weak::new());
                    peer.adopt_link(theirs);
                    peer.update_pin();
                    peer.schedule_linked();
                }
                Some(peer) => {
                    peer.peer.replace(Weak::new());
                    peer.update_pin();
                    theirs.close();
                }
                None => theirs.close(),
            }
            mine
        }
    };
    // Messages the port received but did not deliver go with it.
    let queued: Vec<_> = port.queue.borrow_mut().drain(..).collect();
    if !queued.is_empty() {
        let mut encoded = Vec::with_capacity(queued.len());
        for (data, _ports) in queued {
            if let Ok(message) = crate::structured_clone::encode_message(s, s.rehome(data.get(s)), s.undefined()) {
                encoded.push(message);
            }
        }
        let mut state = end.lock();
        let inbox = &mut state.ends[end.side];
        for message in encoded.into_iter().rev() {
            inbox.bytes += message.size();
            inbox.inbox.push_front(message);
        }
    }
    if closed {
        end.close();
    }
    port.update_pin();
    Ok(end)
}

/// Whether `value` is a ReadableStream that can be transferred
/// (DataCloneError for a locked one).
pub(crate) fn is_transferable_stream(s: &Scope<'_>, value: lp_js::Value<'_>) -> JsResult<bool> {
    if !crate::streams::is_stream(s, value)? {
        return Ok(false);
    }
    let page = page_of(s)?;
    if crate::streams::helper(s, &page, "isLocked", &[value])?.to_bool() {
        return Err(JsError::dom("DataCloneError", "A ReadableStream could not be cloned because it was locked"));
    }
    Ok(true)
}

/// The port's own `postMessage` and `close`, for the streams module's pumps.
fn port_methods<'s>(object: lp_js::Object<'s>) -> JsResult<(lp_js::Value<'s>, lp_js::Value<'s>)> {
    Ok((object.get_atom("postMessage")?, object.get_atom("close")?))
}

/// Transfers `stream` out of this thread: a new channel's first port pumps
/// its chunks (`streams.js` `pumpToPort`), the second leaves as a link end.
pub(crate) fn stream_out(s: &Scope<'_>, stream: lp_js::Value<'_>) -> JsResult<LinkEnd> {
    let page = page_of(s)?;
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    let (o1, p1) = new_port(s, &page)?;
    let (o2, p2) = new_port(s, &page)?;
    p1.peer.replace(Rc::downgrade(&p2));
    p2.peer.replace(Rc::downgrade(&p1));
    let (post, close) = port_methods(o1)?;
    crate::streams::helper(s, &page, "pumpToPort", &[stream, o1.value(), post, close])?;
    transfer_out(s, o2.value())
}

/// The ReadableStream that reads `port`, a transferred stream's link.
pub(crate) fn stream_in<'s>(s: &Scope<'s>, port: lp_js::Value<'s>) -> JsResult<lp_js::Value<'s>> {
    let page = page_of(s)?;
    let object = port.as_object().ok_or_else(|| JsError::dom("DataCloneError", "Malformed message"))?;
    let (post, close) = port_methods(object)?;
    crate::streams::helper(s, &page, "fromPort", &[port, post, close])
}

/// The port a transferred link end becomes in `s`'s realm.
pub(crate) fn transfer_in<'s>(s: &Scope<'s>, end: LinkEnd) -> JsResult<lp_js::Value<'s>> {
    let page = page_of(s)?;
    if page.is_disposed() {
        end.close();
        return Err(JsError::ContextDestroyed);
    }
    let (object, port) = new_port(s, &page)?;
    port.adopt_link(end);
    Ok(object.value())
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
