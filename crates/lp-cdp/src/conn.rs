//! One upgraded CDP connection, served on its browser thread.
//!
//! After the HTTP upgrade the I/O thread hands the socket to the connection's
//! browser thread, which then reads, decodes, answers and writes on its own:
//! no message crosses a thread. The socket is non-blocking and registered with
//! the browser loop ([`lp_runtime::FdWatch`]), so the thread sleeps in one
//! `poll(2)` on its wake pipe and the socket. [`Conn::service`] runs between
//! loop tasks; while JavaScript runs, [`Conn::check_during_script`] (polled by
//! the engine watchdog) notices a client that left and stops the script.
//!
//! The guarantees of the tokio server path are kept: messages are handled in
//! order and at most [`INFLIGHT`] wait undecoded work; replies and events are
//! written in production order, with pongs and close frames at frame
//! boundaries ahead of queued data; the outbound byte budget (1008) still
//! counts frames not yet handed to the socket, since frames are pulled from
//! the [`OutboundRx`] one at a time; incomplete messages, stalled writes, a
//! busy session with a full read-ahead buffer and the final flush are bounded
//! by the same [`ServerConfig`] timeouts.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::rc::Rc;
use std::time::{Duration, Instant};

use lp_runtime::FdWatch;

use crate::outbox::{Outbound, OutboundRx};
use crate::server::{Inbound, ServerConfig};
use crate::ws::{self, Control, Decoder, FrameBytes, Message};

/// Decoded messages waiting for the session; beyond this the socket is not read.
pub(crate) const INFLIGHT: usize = 64;

/// Pending control frames (pongs/close); a client exceeding it is dropped.
const MAX_PENDING_CONTROL: usize = 32;

/// While page work keeps the loop busy, the socket is read at most this often
/// unless the loop woke for it (an idle loop reads as soon as data arrives).
const BUSY_READ_INTERVAL: Duration = Duration::from_micros(100);

/// Frames and bytes handed to one `writev(2)` (well under IOV_MAX).
const MAX_BATCH: usize = 64;
const MAX_BATCH_BYTES: usize = 64 * 1024;

fn is_close(p: &Pending) -> bool {
    matches!(p, Pending::Control(c) if c.first() == Some(&0x88))
}

enum Pending {
    Frame(FrameBytes),
    Control(Vec<u8>),
}

impl AsRef<[u8]> for Pending {
    fn as_ref(&self) -> &[u8] {
        match self {
            Pending::Frame(f) => f.as_ref(),
            Pending::Control(c) => c,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    /// A close frame is queued (or sent): no more reading; written data after
    /// it is dropped and the socket is shut down once it is out.
    Closing,
    /// The peer is gone or the connection failed: nothing more is written.
    Gone,
}

pub(crate) struct Conn {
    sock: TcpStream,
    watch: Rc<FdWatch>,
    decoder: Decoder,
    chunk: Vec<u8>,
    read_ahead: usize,
    out: OutboundRx,
    control: VecDeque<Vec<u8>>,
    /// Frames taken from the channel for a batch but not written yet.
    queued: VecDeque<Pending>,
    /// Messages decoded while a script ran, handed to the session next.
    held: VecDeque<Inbound>,
    current: Option<(Pending, usize)>,
    state: State,
    /// The session was told (one `Inbound::Closed`).
    closed_sent: bool,
    /// The close frame was handed to the socket.
    close_written: bool,
    config: ServerConfig,
    message_deadline: Option<Instant>,
    write_blocked_since: Option<Instant>,
    stall_since: Option<Instant>,
    last_read: Instant,
}

impl Conn {
    /// `buffered` holds bytes read past the upgrade request.
    pub(crate) fn new(sock: TcpStream, buffered: &[u8], out: OutboundRx, config: ServerConfig) -> std::io::Result<Conn> {
        sock.set_nonblocking(true)?;
        let mut decoder = Decoder::new(config.ws);
        decoder.feed(buffered);
        let watch = FdWatch::new(raw_fd(&sock));
        let read_ahead = config.ws.max_message_size.max(64 * 1024).saturating_mul(2);
        let conn = Conn {
            sock,
            watch,
            decoder,
            chunk: vec![0u8; 64 * 1024],
            read_ahead,
            out,
            control: VecDeque::new(),
            queued: VecDeque::new(),
            held: VecDeque::new(),
            current: None,
            state: State::Open,
            closed_sent: false,
            close_written: false,
            config,
            message_deadline: None,
            write_blocked_since: None,
            stall_since: None,
            last_read: Instant::now(),
        };
        conn.update_interest(0);
        Ok(conn)
    }

    pub(crate) fn watch(&self) -> Rc<FdWatch> {
        self.watch.clone()
    }

    /// Reads, decodes into `inbox`, writes what is queued and applies the
    /// timeouts. Runs between loop tasks; cheap when there is nothing to do.
    pub(crate) fn service(&mut self, inbox: &mut VecDeque<Inbound>) {
        inbox.extend(self.held.drain(..));
        let now = Instant::now();
        if self.watch.take_ready() || now.duration_since(self.last_read) >= BUSY_READ_INTERVAL {
            self.read(inbox, now);
        } else {
            // Messages already buffered (read while the session was busy).
            self.decode(inbox);
        }
        self.write();
        self.check_timeouts(inbox, now);
        self.update_interest(inbox.len());
    }

    /// Writes queued frames without reading (right after a command ran).
    pub(crate) fn flush(&mut self, inbox: &mut VecDeque<Inbound>) {
        inbox.extend(self.held.drain(..));
        self.write();
        self.check_timeouts(inbox, Instant::now());
        self.update_interest(inbox.len());
    }

    /// The earliest timeout the loop must wake for.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let write = self.write_blocked_since.map(|t| t + self.config.write_timeout);
        [self.message_deadline, write].into_iter().flatten().min()
    }

    /// Whether the connection has ended for the session (it was told).
    pub(crate) fn ended(&self) -> bool {
        self.closed_sent
    }

    /// Called by the engine watchdog while JavaScript runs (the session
    /// cannot take messages meanwhile). Keeps reading and decoding, up to the
    /// read-ahead bound and [`INFLIGHT`] held messages, answers pings, and
    /// returns true when the running script must stop: the client closed or
    /// left, the outbound budget was exceeded, or the session took nothing
    /// for `stall_timeout` while the read-ahead buffer was full.
    pub(crate) fn check_during_script(&mut self) -> bool {
        if self.state == State::Open && self.out.is_exceeded() {
            // The writer would close (1008) at once; the script must stop too.
            self.start_close(1008, "outbound buffer limit exceeded");
        }
        let now = Instant::now();
        while self.state == State::Open {
            let mut held = std::mem::take(&mut self.held);
            self.decode(&mut held);
            self.held = held;
            if self.state != State::Open {
                break;
            }
            if self.decoder.buffered() >= self.read_ahead || self.held.len() >= INFLIGHT {
                let since = *self.stall_since.get_or_insert(now);
                if now.duration_since(since) >= self.config.stall_timeout {
                    self.start_close(1008, "session is not accepting messages");
                }
                break;
            }
            self.stall_since = None;
            match (&self.sock).read(&mut self.chunk) {
                Ok(0) => self.state = State::Gone,
                Ok(n) => self.decoder.feed(&self.chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => self.state = State::Gone,
            }
        }
        // Pongs (and a close frame) go out now, as an I/O thread would send them.
        self.write();
        self.state != State::Open
    }

    /// Ends the connection after the session has stopped: writes what is
    /// still queued and a normal close (unless a close is already under way
    /// or the peer is gone), within `close_timeout`, then shuts the socket.
    pub(crate) fn finish(mut self) {
        let deadline = Instant::now() + self.config.close_timeout;
        if self.state == State::Open {
            // Frames the teardown produced go out before the close frame.
            loop {
                if !self.write_blocking(deadline) || self.state != State::Open {
                    break;
                }
                if self.current.is_none() && self.queued.is_empty() && self.control.is_empty() {
                    // write() stopped with nothing ready: the channel is empty.
                    self.start_close(1000, "");
                    break;
                }
            }
        }
        while self.state == State::Closing && !self.close_written {
            if !self.write_blocking(deadline) {
                break;
            }
        }
        let _ = self.sock.shutdown(Shutdown::Both);
    }

    /// Writes until everything ready is out or the deadline
    /// passes (blocking). Returns false when no more progress is possible.
    fn write_blocking(&mut self, deadline: Instant) -> bool {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || self.sock.set_nonblocking(false).is_err() {
            return false;
        }
        let _ = self.sock.set_write_timeout(Some(remaining));
        self.write();
        let _ = self.sock.set_nonblocking(true);
        self.state != State::Gone && Instant::now() < deadline
    }

    fn read(&mut self, inbox: &mut VecDeque<Inbound>, now: Instant) {
        self.last_read = now;
        loop {
            self.decode(inbox);
            if self.state != State::Open || inbox.len() >= INFLIGHT || self.decoder.buffered() >= self.read_ahead {
                break;
            }
            let read = crate::perf::now();
            match (&self.sock).read(&mut self.chunk) {
                Ok(0) => {
                    // The client went away without a close handshake: queued
                    // frames cannot be delivered.
                    self.state = State::Gone;
                    self.tell_closed(inbox);
                    break;
                }
                Ok(n) => {
                    self.decoder.feed(&self.chunk[..n]);
                    crate::perf::record_since("io.ws_read", read);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => {
                    self.state = State::Gone;
                    self.tell_closed(inbox);
                    break;
                }
            }
        }
        if self.decoder.in_progress() {
            self.message_deadline.get_or_insert(now + self.config.frame_timeout);
        } else {
            self.message_deadline = None;
        }
    }

    fn decode(&mut self, inbox: &mut VecDeque<Inbound>) {
        while self.state == State::Open && inbox.len() < INFLIGHT {
            let decode = crate::perf::now();
            let next = self.decoder.next_message();
            match next {
                Ok(Some(Message::Text(text))) => {
                    crate::perf::record_since("io.ws_decode", decode);
                    inbox.push_back(Inbound::Text(text));
                }
                Ok(Some(Message::Binary(_))) => self.close(inbox, 1003, "binary messages are not supported"),
                Ok(Some(Message::Ping(data))) => {
                    if self.control.len() >= MAX_PENDING_CONTROL {
                        // Floods pings faster than it reads pongs: drop it.
                        self.state = State::Gone;
                        self.tell_closed(inbox);
                        return;
                    }
                    self.control.push_back(ws::control_frame(Control::Pong(&data)));
                }
                Ok(Some(Message::Pong(_))) => {}
                Ok(Some(Message::Close(code))) => self.close(inbox, code.map_or(1000, |(c, _)| c), ""),
                Ok(None) => return,
                Err(error) => {
                    // The client only sees a closed socket; leave the reason where operators look.
                    eprintln!("lightningpanda: closing CDP connection: {error}");
                    self.close(inbox, error.close_code(), error.close_reason());
                }
            }
        }
    }

    /// Queues a close frame ahead of queued data and ends the session.
    fn close(&mut self, inbox: &mut VecDeque<Inbound>, code: u16, reason: &str) {
        self.start_close(code, reason);
        self.tell_closed(inbox);
    }

    fn start_close(&mut self, code: u16, reason: &str) {
        if self.state == State::Open {
            self.control.push_back(ws::control_frame(Control::Close(code, reason)));
            self.state = State::Closing;
        }
    }

    fn tell_closed(&mut self, inbox: &mut VecDeque<Inbound>) {
        if !self.closed_sent {
            self.closed_sent = true;
            inbox.push_back(Inbound::Closed);
        }
    }

    /// The next whole item to write: control frames first (at a frame
    /// boundary), then frames already taken from the channel, then the
    /// channel. Nothing after a close frame; `None` when nothing is ready.
    fn next_item(&mut self) -> Option<Pending> {
        loop {
            if let Some(c) = self.control.pop_front() {
                return Some(Pending::Control(c));
            }
            if self.state != State::Open {
                // Closing: only the close frame (a control frame) is left to send.
                return None;
            }
            if let Some(q) = self.queued.pop_front() {
                return Some(q);
            }
            match self.out.try_recv() {
                Some(Outbound::Frame(frame)) => {
                    #[cfg(feature = "perf-trace")]
                    crate::perf::record_since("io.outbound_queue", frame.queued_at);
                    return Some(Pending::Frame(frame));
                }
                Some(Outbound::Exceeded) => self.start_close(1008, "outbound buffer limit exceeded"),
                None => return None,
            }
        }
    }

    /// Writes queued bytes until the socket would block: the frame in
    /// progress first, then control frames, then data frames in order,
    /// several frames per `writev(2)` (at most [`MAX_BATCH`] or
    /// [`MAX_BATCH_BYTES`]).
    fn write(&mut self) {
        loop {
            if self.state == State::Gone {
                self.current = None;
                self.queued.clear();
                return;
            }
            if self.close_written {
                return;
            }
            if self.current.is_none() {
                match self.next_item() {
                    Some(item) => self.current = Some((item, 0)),
                    None => return,
                }
            }
            // Whole items after the frame in progress, up to the batch bounds
            // and never past a close frame.
            let mut batch: Vec<Pending> = Vec::new();
            let (first, offset) = self.current.as_ref().expect("set above");
            let mut bytes = first.as_ref().len() - offset;
            let mut closes = is_close(first);
            while !closes && batch.len() + 1 < MAX_BATCH && bytes < MAX_BATCH_BYTES {
                let Some(item) = self.next_item() else { break };
                bytes += item.as_ref().len();
                closes = is_close(&item);
                batch.push(item);
            }
            let written = {
                let (first, offset) = self.current.as_ref().expect("set above");
                let mut slices = Vec::with_capacity(batch.len() + 1);
                slices.push(std::io::IoSlice::new(&first.as_ref()[*offset..]));
                slices.extend(batch.iter().map(|p| std::io::IoSlice::new(p.as_ref())));
                let write = crate::perf::now();
                let r = (&self.sock).write_vectored(&slices);
                if r.is_ok() {
                    crate::perf::record_since("io.ws_write", write);
                }
                r
            };
            match written {
                Ok(mut n) => {
                    self.write_blocked_since = None;
                    // Advance through the frame in progress, then the batch.
                    let mut items = std::iter::once(self.current.take().expect("set above")).chain(
                        batch.into_iter().map(|p| (p, 0)),
                    );
                    let mut leftover: Vec<(Pending, usize)> = Vec::new();
                    for (item, offset) in items.by_ref() {
                        let len = item.as_ref().len() - offset;
                        if n >= len {
                            n -= len;
                            if is_close(&item) {
                                // The close frame is out: nothing is written after it.
                                self.close_written = true;
                                self.state = State::Closing;
                                let _ = self.sock.shutdown(Shutdown::Write);
                                return;
                            }
                        } else {
                            leftover.push((item, offset + n));
                            break;
                        }
                    }
                    leftover.extend(items);
                    let mut leftover = leftover.into_iter();
                    if let Some((item, offset)) = leftover.next() {
                        self.current = Some((item, offset));
                        self.requeue(leftover.map(|(p, _)| p));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    self.requeue(batch);
                    self.write_blocked_since.get_or_insert_with(Instant::now);
                    return;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => self.requeue(batch),
                Err(_) => {
                    self.state = State::Gone;
                    self.current = None;
                    self.queued.clear();
                    return;
                }
            }
        }
    }

    /// Puts unwritten whole items back, ahead of anything not yet taken:
    /// control frames return to the control queue, data frames to `queued`.
    fn requeue(&mut self, items: impl IntoIterator<Item = Pending>) {
        let (mut controls, mut data) = (Vec::new(), Vec::new());
        for item in items {
            match item {
                Pending::Control(c) => controls.push(c),
                frame => data.push(frame),
            }
        }
        for c in controls.into_iter().rev() {
            self.control.push_front(c);
        }
        for d in data.into_iter().rev() {
            self.queued.push_front(d);
        }
    }

    fn check_timeouts(&mut self, inbox: &mut VecDeque<Inbound>, now: Instant) {
        if self.state == State::Gone {
            self.tell_closed(inbox);
            return;
        }
        if self.message_deadline.is_some_and(|d| now >= d) {
            self.message_deadline = None;
            self.close(inbox, 1008, "incomplete message timeout");
            self.write();
        }
        if self.write_blocked_since.is_some_and(|t| now.duration_since(t) >= self.config.write_timeout) {
            // The client stopped reading.
            self.state = State::Gone;
            self.current = None;
            self.tell_closed(inbox);
        }
        if self.state == State::Closing && self.close_written {
            self.tell_closed(inbox);
        }
    }

    fn update_interest(&self, inbox_len: usize) {
        let read = self.state == State::Open && inbox_len < INFLIGHT && self.decoder.buffered() < self.read_ahead;
        let write = self.state != State::Gone && self.write_blocked_since.is_some();
        self.watch.set_interest(read, write);
    }
}

#[cfg(unix)]
fn raw_fd(sock: &TcpStream) -> i32 {
    use std::os::fd::AsRawFd;
    sock.as_raw_fd()
}

#[cfg(not(unix))]
fn raw_fd(_sock: &TcpStream) -> i32 {
    -1
}
