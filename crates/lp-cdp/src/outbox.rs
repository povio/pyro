//! The ordered, byte-budgeted outbound channel of one CDP connection.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::target::Sink;
use crate::ws::{FrameBytes, OutFrame};

/// Default limit of encoded bytes queued for a client that is not reading.
pub const DEFAULT_OUTBOUND_BUDGET: usize = 64 * 1024 * 1024;

struct Budget {
    queued: AtomicUsize,
    limit: usize,
    exceeded: AtomicBool,
}

/// Cloneable handle to a connection's outbound frames. Clones share one
/// ordered channel, so asynchronous responses (e.g. `awaitPromise`) and
/// synchronous ones interleave in the order they are produced on the
/// browser thread.
///
/// Protocol events cannot be dropped selectively, so when a client lets more
/// than the budget accumulate the connection is marked exceeded: nothing more
/// is queued and the writer closes the connection (1008).
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::UnboundedSender<FrameBytes>,
    budget: Arc<Budget>,
}

/// Receiving end used by the connection writer.
pub struct OutboundRx {
    rx: mpsc::UnboundedReceiver<FrameBytes>,
    budget: Arc<Budget>,
}

pub enum Outbound {
    Frame(FrameBytes),
    /// The budget was exceeded; the connection must be closed.
    Exceeded,
}

impl Outbox {
    pub fn channel(limit: usize) -> (Outbox, OutboundRx) {
        let (tx, rx) = mpsc::unbounded_channel();
        let budget = Arc::new(Budget { queued: AtomicUsize::new(0), limit, exceeded: AtomicBool::new(false) });
        (Outbox { tx, budget: budget.clone() }, OutboundRx { rx, budget })
    }

    /// Queues a frame; frames for a closed or over-budget connection are dropped.
    pub fn push(&self, frame: OutFrame) {
        if self.budget.exceeded.load(Ordering::Relaxed) {
            return;
        }
        let frame = frame.finish_text();
        let len = frame.as_ref().len();
        let queued = self.budget.queued.fetch_add(len, Ordering::Relaxed) + len;
        if queued > self.budget.limit {
            self.budget.exceeded.store(true, Ordering::Relaxed);
            self.budget.queued.fetch_sub(len, Ordering::Relaxed);
            return;
        }
        if self.tx.send(frame).is_err() {
            self.budget.queued.fetch_sub(len, Ordering::Relaxed);
        }
    }

    /// Encoded bytes waiting to be written (diagnostics/tests).
    pub fn queued_bytes(&self) -> usize {
        self.budget.queued.load(Ordering::Relaxed)
    }
}

impl OutboundRx {
    /// Next frame in order; `None` once every [`Outbox`] is dropped.
    pub async fn recv(&mut self) -> Option<Outbound> {
        if self.budget.exceeded.load(Ordering::Relaxed) {
            return Some(Outbound::Exceeded);
        }
        let frame = self.rx.recv().await?;
        self.budget.queued.fetch_sub(frame.as_ref().len(), Ordering::Relaxed);
        Some(Outbound::Frame(frame))
    }
}

impl OutboundRx {
    /// Next queued frame without waiting (tests, synchronous drivers).
    pub fn try_recv(&mut self) -> Option<Outbound> {
        if self.budget.exceeded.load(Ordering::Relaxed) {
            return Some(Outbound::Exceeded);
        }
        let frame = self.rx.try_recv().ok()?;
        self.budget.queued.fetch_sub(frame.as_ref().len(), Ordering::Relaxed);
        Some(Outbound::Frame(frame))
    }
}

impl Sink for Outbox {
    fn send(&mut self, frame: OutFrame) {
        self.push(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(len: usize) -> OutFrame {
        let mut f = OutFrame::with_capacity(len);
        f.payload().extend(std::iter::repeat_n(b'x', len));
        f
    }

    #[tokio::test]
    async fn budget_marks_exceeded_and_stops_queueing() {
        let (out, mut rx) = Outbox::channel(300);
        out.push(frame(100));
        out.push(frame(100));
        assert!(out.queued_bytes() > 200);
        out.push(frame(100)); // exceeds 300 with headers
        out.push(frame(1));
        assert!(matches!(rx.recv().await, Some(Outbound::Exceeded)));
    }

    #[tokio::test]
    async fn draining_releases_budget() {
        let (out, mut rx) = Outbox::channel(250);
        for _ in 0..10 {
            out.push(frame(100));
            assert!(matches!(rx.recv().await, Some(Outbound::Frame(_))));
        }
        assert_eq!(out.queued_bytes(), 0);
        drop(out);
        assert!(rx.recv().await.is_none());
    }
}
