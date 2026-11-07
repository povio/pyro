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
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::page::RemotePoster;
use crate::structured_clone::WireMessage;

/// Message bytes that may wait in one side's inbox. Past this, posting
/// throws DataCloneError (unless the inbox is empty).
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

    /// Queues `message` for the other side and wakes its owner.
    pub(crate) fn send(&self, message: WireMessage) -> Result<Sent, (WireMessage, &'static str)> {
        let size = message.size();
        let mut sides = self.pipe.lock();
        let (me, other) = (self.side, self.other());
        if sides[me].closed || sides[other].closed {
            drop(sides);
            drop(message);
            return Ok(Sent::Dropped);
        }
        let side = &mut sides[other];
        if side.bytes > 0 && side.bytes.saturating_add(size) > MAX_INBOX_BYTES {
            drop(sides);
            return Err((message, "The port's message queue is full"));
        }
        side.inbox.push_back(message);
        side.bytes += size;
        let owner = side.owner.clone();
        drop(sides);
        if let Some(owner) = owner {
            let id = owner.id;
            (owner.poster)(Box::new(move || crate::message_channel::wake_remote(id)));
        }
        Ok(Sent::Queued)
    }

    /// Puts messages back at the front of this side's own inbox, in order
    /// (the port's undelivered messages, when it moves to another thread).
    pub(crate) fn requeue_front(&self, messages: Vec<WireMessage>) {
        let mut sides = self.pipe.lock();
        let side = &mut sides[self.side];
        if side.closed {
            drop(sides);
            drop(messages);
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
        drop(inbox);
        if let Some(owner) = owner {
            let id = owner.id;
            (owner.poster)(Box::new(move || crate::message_channel::peer_closed_remote(id)));
        }
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
        assert!(matches!(a.send(WireMessage::empty()), Ok(Sent::Queued)));
        assert_eq!(b.pending(), 1);
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let poster: RemotePoster = Arc::new(move |_task| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        assert!(!b.set_owner(Some(Owner { poster, id: 7 })));
        assert!(matches!(a.send(WireMessage::empty()), Ok(Sent::Queued)));
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        assert!(b.pop().is_some());
        b.requeue_front(vec![WireMessage::empty()]);
        assert_eq!(b.pending(), 2);
        assert!(b.peer_open());
        a.close();
        assert!(!b.peer_open());
        // The other side's owner hears about the close.
        assert_eq!(woken.load(Ordering::Relaxed), 2);
        assert!(matches!(b.send(WireMessage::empty()), Ok(Sent::Dropped)));
        b.close();
        assert_eq!(b.pending(), 0);
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
