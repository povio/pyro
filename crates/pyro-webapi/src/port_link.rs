//! The thread-safe entanglement of two MessagePorts that live on different
//! threads (a page and a dedicated worker, or two workers).
//!
//! Ports of one thread are entangled directly (`message_channel::PortInner`
//! holds its peer). The first time one of them is transferred to another
//! thread, the pair switches to a [`Pipe`]: each side has an inbox of
//! serialized messages ([`WireMessage`]) and an owner, the thread's poster
//! and the id of the port that stands for that side there. Sending pushes on
//! the other side's inbox and posts one wake-up task to its owner's loop
//! (the loop sleeps until then: there is no polling). The receiving port
//! takes messages off its inbox only when it delivers one, so whatever is
//! still queued follows the port when it moves again: a side being
//! transferred has no owner, its messages wait in the inbox, and the thread
//! that deserializes it becomes the owner.
//!
//! A side on its way somewhere is a [`TransitEnd`] inside a `WireMessage`.
//! If that message is dropped before it is deserialized (the worker ended,
//! the document went away), the side closes, so the other port does not
//! stay pinned waiting for a partner that will never exist.
//!
//! Lock discipline: a pipe's mutex is never held while posting, while
//! dropping a message (which can close other pipes, or this one) or while
//! running JS.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::page::RemotePoster;
use crate::structured_clone::WireMessage;

/// Message bytes that may wait in one side's inbox. Once it holds this
/// much, posting throws DataCloneError before the message is serialized.
pub(crate) const MAX_INBOX_BYTES: usize = 64 * 1024 * 1024;

/// Where one side of a pipe lives now.
#[derive(Clone)]
pub(crate) struct Owner {
    pub(crate) poster: RemotePoster,
    /// The port's id in its thread's registry (`message_channel::REMOTE`).
    pub(crate) id: u64,
}

#[derive(Default)]
struct Side {
    owner: Option<Owner>,
    inbox: VecDeque<WireMessage>,
    bytes: usize,
    closed: bool,
}

pub(crate) struct Pipe {
    sides: Mutex<[Side; 2]>,
}

impl Pipe {
    fn lock(&self) -> MutexGuard<'_, [Side; 2]> {
        self.sides.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One side of a pipe, held by the port that stands for it.
#[derive(Clone)]
pub(crate) struct Link {
    pipe: Arc<Pipe>,
    side: usize,
}

/// What [`Link::send`] did with a message.
pub(crate) enum Sent {
    Queued,
    /// Either side is closed: the message was dropped, as HTML drops a
    /// message posted on a disentangled port.
    Dropped,
}

impl Link {
    /// A new pipe's two sides, both without an owner.
    pub(crate) fn pair() -> (Link, Link) {
        let pipe = Arc::new(Pipe { sides: Mutex::new([Side::default(), Side::default()]) });
        (Link { pipe: pipe.clone(), side: 0 }, Link { pipe, side: 1 })
    }

    fn other(&self) -> usize {
        1 - self.side
    }

    /// Whether `other` is the opposite side of this pipe.
    pub(crate) fn is_peer_of(&self, other: &Link) -> bool {
        Arc::ptr_eq(&self.pipe, &other.pipe) && self.side != other.side
    }

    /// Makes `owner` the receiver of this side's wake-ups (`None` while it
    /// travels). Returns whether this side is closed.
    pub(crate) fn set_owner(&self, owner: Option<Owner>) -> bool {
        let mut sides = self.pipe.lock();
        let side = &mut sides[self.side];
        side.owner = if side.closed { None } else { owner };
        side.closed
    }

    /// Whether the other side's inbox can take another message: it holds
    /// less than [`MAX_INBOX_BYTES`]. Checked before a message is
    /// serialized, so a full inbox throws before anything is transferred
    /// (only this side fills that inbox, so the answer holds until then).
    pub(crate) fn has_room(&self) -> bool {
        self.pipe.lock()[self.other()].bytes < MAX_INBOX_BYTES
    }

    /// Queues `message` for the other side and wakes its owner.
    pub(crate) fn send(&self, message: WireMessage) -> Sent {
        let size = message.size();
        let mut sides = self.pipe.lock();
        let (me, other) = (self.side, self.other());
        if sides[me].closed || sides[other].closed {
            drop(sides);
            release(std::iter::once(message));
            return Sent::Dropped;
        }
        let side = &mut sides[other];
        side.inbox.push_back(message);
        side.bytes = side.bytes.saturating_add(size);
        let owner = side.owner.clone();
        drop(sides);
        if let Some(owner) = owner {
            let id = owner.id;
            (owner.poster)(Box::new(move || crate::message_channel::wake_remote(id)));
        }
        Sent::Queued
    }

    /// Puts messages back at the front of this side's own inbox, in order
    /// (the port's undelivered messages, when it moves to another thread).
    pub(crate) fn requeue_front(&self, messages: Vec<WireMessage>) {
        let mut sides = self.pipe.lock();
        let side = &mut sides[self.side];
        if side.closed {
            drop(sides);
            release(messages);
            return;
        }
        for message in messages.into_iter().rev() {
            side.bytes += message.size();
            side.inbox.push_front(message);
        }
    }

    /// The next message for this side.
    pub(crate) fn pop(&self) -> Option<WireMessage> {
        let mut sides = self.pipe.lock();
        let side = &mut sides[self.side];
        let message = side.inbox.pop_front()?;
        side.bytes = side.bytes.saturating_sub(message.size());
        Some(message)
    }

    /// Messages waiting for this side.
    pub(crate) fn pending(&self) -> usize {
        self.pipe.lock()[self.side].inbox.len()
    }

    /// Whether the other side can still post here (it has not closed).
    pub(crate) fn peer_open(&self) -> bool {
        !self.pipe.lock()[self.other()].closed
    }

    /// Whether this side was closed (a closed port was transferred).
    pub(crate) fn is_closed(&self) -> bool {
        self.pipe.lock()[self.side].closed
    }

    /// Closes this side: its undelivered messages are dropped, later posts
    /// from either side are dropped, and the other side's owner is told so
    /// it can release its pin. Idempotent.
    pub(crate) fn close(&self) {
        let mut sides = self.pipe.lock();
        if sides[self.side].closed {
            return;
        }
        let side = &mut sides[self.side];
        side.closed = true;
        side.owner = None;
        side.bytes = 0;
        let inbox = std::mem::take(&mut side.inbox);
        let other = &sides[self.other()];
        let owner = if other.closed { None } else { other.owner.clone() };
        drop(sides);
        release(inbox);
        if let Some(owner) = owner {
            let id = owner.id;
            (owner.poster)(Box::new(move || crate::message_channel::peer_closed_remote(id)));
        }
    }
}

/// A pipe as the page that made it remembers it.
pub(crate) struct WeakPipe(std::sync::Weak<Pipe>);

impl WeakPipe {
    /// Whether a link of the pipe is still alive, on any thread.
    pub(crate) fn is_live(&self) -> bool {
        self.0.strong_count() > 0
    }
}

impl Link {
    pub(crate) fn downgrade(&self) -> WeakPipe {
        WeakPipe(Arc::downgrade(&self.pipe))
    }
}

impl WeakPipe {
    pub(crate) fn is_gone(&self) -> bool {
        self.0.strong_count() == 0
    }
}

/// A pipe side: the pipe's address and the side's index.
type SideKey = (usize, usize);

/// Closes the travelling sides of `pipes` that nothing can reach any more:
/// sides held only in the undelivered messages of other such sides (a
/// cycle of ports in transit inside each other's inboxes keeps itself alive
/// for ever otherwise). Run when the page that made the pipes goes away.
///
/// A side that is owned, or travelling inside a message that is not in one
/// of these inboxes (on its way to a worker, or queued on another pipe), is
/// live, and so is every side its inbox carries. Only the others close: a
/// port still on its way somewhere keeps its messages.
pub(crate) fn close_orphans(pipes: &[WeakPipe]) {
    let pipes: Vec<Arc<Pipe>> = pipes.iter().filter_map(|p| p.0.upgrade()).collect();
    if pipes.is_empty() {
        return;
    }
    // One pipe locked at a time: a side inside an unowned inbox cannot
    // leave it, so a cycle seen here stays one.
    let mut travelling = Vec::new();
    let mut roots = Vec::new();
    let mut carried: FxHashMap<SideKey, Vec<SideKey>> = FxHashMap::default();
    let mut contained = FxHashSet::default();
    for pipe in &pipes {
        let sides = pipe.lock();
        for (index, side) in sides.iter().enumerate() {
            if side.closed {
                continue;
            }
            let key = (Arc::as_ptr(pipe) as usize, index);
            if side.owner.is_some() {
                roots.push(key);
            } else {
                travelling.push((pipe.clone(), index));
            }
            let inner: Vec<SideKey> =
                side.inbox.iter().flat_map(WireMessage::transit_ends).filter_map(TransitEnd::side_key).collect();
            if !inner.is_empty() {
                contained.extend(inner.iter().copied());
                carried.insert(key, inner);
            }
        }
    }
    roots.extend(
        travelling
            .iter()
            .map(|(pipe, index)| (Arc::as_ptr(pipe) as usize, *index))
            .filter(|key| !contained.contains(key)),
    );
    let mut live = FxHashSet::default();
    while let Some(key) = roots.pop() {
        if live.insert(key) {
            roots.extend(carried.get(&key).into_iter().flatten().copied());
        }
    }
    for (pipe, side) in travelling {
        if !live.contains(&(Arc::as_ptr(&pipe) as usize, side)) {
            Link { pipe, side }.close();
        }
    }
}

thread_local! {
    /// Messages waiting to be dropped by [`release`], and whether it runs.
    static RELEASING: RefCell<Vec<WireMessage>> = const { RefCell::new(Vec::new()) };
    static RELEASE_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Drops `messages`. A message carrying sides of pipes closes them, which
/// drops their inboxes, whose messages may carry more sides: those land
/// here and the outermost call drops them one at a time, so a chain of any
/// length never deepens the native stack.
fn release(messages: impl IntoIterator<Item = WireMessage>) {
    let mut messages = messages.into_iter().peekable();
    if messages.peek().is_none() {
        return;
    }
    if RELEASING.try_with(|queue| queue.borrow_mut().extend(&mut messages)).is_err() {
        // The thread is ending: drop in place.
        messages.for_each(drop);
        return;
    }
    if RELEASE_ACTIVE.with(|active| active.replace(true)) {
        return;
    }
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            let _ = RELEASE_ACTIVE.try_with(|active| active.set(false));
        }
    }
    let _done = Done;
    while let Some(message) = RELEASING.with(|queue| queue.borrow_mut().pop()) {
        drop(message);
    }
}

/// A side of a pipe inside a serialized message. Claimed once, by the
/// deserialization that makes its port; closed if never claimed.
pub(crate) struct TransitEnd(Mutex<Option<Link>>);

impl TransitEnd {
    pub(crate) fn new(link: Link) -> Self {
        TransitEnd(Mutex::new(Some(link)))
    }

    pub(crate) fn claim(&self) -> Option<Link> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// The side it carries (still unclaimed).
    fn side_key(&self) -> Option<SideKey> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|l| (Arc::as_ptr(&l.pipe) as usize, l.side))
    }

    /// Whether this is the side opposite `link` (still unclaimed).
    pub(crate) fn is_peer_of(&self, link: &Link) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|l| l.is_peer_of(link))
    }
}

impl Drop for TransitEnd {
    fn drop(&mut self) {
        let link = self.0.get_mut().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(link) = link {
            link.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn messages_wait_for_an_owner_and_closing_drops_them() {
        let (a, b) = Link::pair();
        assert!(matches!(a.send(WireMessage::empty()), Sent::Queued));
        assert_eq!(b.pending(), 1);
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let poster: RemotePoster = Arc::new(move |_task| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        assert!(!b.set_owner(Some(Owner { poster, id: 7 })));
        assert!(matches!(a.send(WireMessage::empty()), Sent::Queued));
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        assert!(b.pop().is_some());
        b.requeue_front(vec![WireMessage::empty()]);
        assert_eq!(b.pending(), 2);
        assert!(b.peer_open());
        a.close();
        assert!(!b.peer_open());
        // The other side's owner hears about the close.
        assert_eq!(woken.load(Ordering::Relaxed), 2);
        assert!(matches!(b.send(WireMessage::empty()), Sent::Dropped));
        b.close();
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn orphaned_pipes_in_each_others_inboxes_are_released() {
        // a2 travels inside b's inbox and b1 inside a's: a cycle of pipes.
        let (a1, a2) = Link::pair();
        let (b1, b2) = Link::pair();
        let made = [a1.downgrade(), b1.downgrade()];
        assert!(matches!(a1.send(WireMessage::carrying(TransitEnd::new(b1))), Sent::Queued));
        assert!(matches!(b2.send(WireMessage::carrying(TransitEnd::new(a2))), Sent::Queued));
        a1.close();
        b2.close();
        drop((a1, b2));
        assert!(!made[0].is_gone() && !made[1].is_gone(), "the cycle holds both pipes");
        close_orphans(&made);
        assert!(made[0].is_gone() && made[1].is_gone());
    }

    #[test]
    fn a_side_on_its_way_to_another_thread_keeps_its_messages() {
        // The page posted on c1 after transferring c2 to a service worker,
        // then went away (c1 closed) before the worker received c2.
        let (c1, c2) = Link::pair();
        let made = [c1.downgrade()];
        c1.send(WireMessage::empty());
        c1.close();
        drop(c1);
        let in_flight = WireMessage::carrying(TransitEnd::new(c2));
        close_orphans(&made);
        let c2 = in_flight.transit_ends()[0].claim().unwrap();
        assert!(!c2.is_closed(), "the travelling side stays open");
        assert_eq!(c2.pending(), 1, "with what was posted before its partner closed");

        // Both ends on their way to workers: the channel survives.
        let (d1, d2) = Link::pair();
        let made = [d1.downgrade()];
        let to_one = WireMessage::carrying(TransitEnd::new(d1));
        let to_other = WireMessage::carrying(TransitEnd::new(d2));
        close_orphans(&made);
        let (d1, d2) = (to_one.transit_ends()[0].claim().unwrap(), to_other.transit_ends()[0].claim().unwrap());
        assert!(d1.peer_open() && d2.peer_open());

        // A side carried by a live side's inbox is live too; a side only its
        // own inbox holds is still released.
        let (e1, e2) = Link::pair();
        let (f1, f2) = Link::pair();
        let (g1, g2) = Link::pair();
        let made = [e1.downgrade(), f1.downgrade(), g1.downgrade()];
        e1.send(WireMessage::carrying(TransitEnd::new(f2)));
        let carried_e2 = WireMessage::carrying(TransitEnd::new(e2));
        f1.close();
        g1.send(WireMessage::carrying(TransitEnd::new(g2.clone())));
        drop((f1, g1));
        close_orphans(&made);
        assert!(g2.is_closed(), "unreachable");
        let e2 = carried_e2.transit_ends()[0].claim().unwrap();
        let f2 = e2.pop().unwrap().transit_ends()[0].claim().unwrap();
        assert!(!f2.is_closed(), "carried by a live side");
        drop(e1);
    }

    #[test]
    fn closing_a_long_chain_of_travelling_sides_does_not_recurse() {
        // Each travelling side's inbox carries the next one, so closing the
        // first tears all of them down: recursively, that overflows the stack.
        const LINKS: usize = 200_000;
        let (first_sender, first) = Link::pair();
        let mut senders = vec![first_sender];
        for _ in 0..LINKS {
            let (sender, travelling) = Link::pair();
            senders.last().unwrap().send(WireMessage::carrying(TransitEnd::new(travelling)));
            senders.push(sender);
        }
        first.close();
        assert!(senders.iter().all(|s| !s.peer_open()));
    }

    #[test]
    fn a_full_inbox_is_reported_before_the_message_is_made() {
        let (a, b) = Link::pair();
        assert!(a.has_room());
        assert!(matches!(a.send(WireMessage::sized(MAX_INBOX_BYTES - 1024)), Sent::Queued));
        assert!(a.has_room(), "below the limit");
        assert!(matches!(a.send(WireMessage::sized(2048)), Sent::Queued));
        assert!(!a.has_room(), "at or past the limit");
        assert!(b.pop().is_some());
        assert!(a.has_room(), "room again once delivered");
    }

    #[test]
    fn an_unclaimed_transit_end_closes_its_side() {
        let (a, b) = Link::pair();
        let transit = TransitEnd::new(b);
        assert!(transit.is_peer_of(&a));
        drop(transit);
        assert!(!a.peer_open());
        let (c, d) = Link::pair();
        let transit = TransitEnd::new(d);
        let claimed = transit.claim().unwrap();
        drop(transit);
        assert!(c.peer_open());
        assert!(!claimed.is_closed());
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if RELEASING.with(|r| !r.borrow().is_empty()) {
        return Some("port_link::RELEASING");
    }
    RELEASE_ACTIVE.get().then_some("port_link::RELEASE_ACTIVE")
}
