//! Tokio I/O side of the CDP server: accepts TCP connections, answers the
//! discovery endpoints, upgrades to WebSocket and shuttles text messages to and
//! from a per-connection browser session running on its own thread.
//!
//! The browser side is abstracted by [`SessionFactory`]: for every upgraded
//! connection it returns a channel for inbound messages and a channel of
//! encoded outbound frames. Ordering is preserved in both directions.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::http::{self, HeadError, Routed, ServerInfo};
use crate::outbox::{Outbound, OutboundRx};
use crate::ws::{self, Control, Decoder, Message, WsConfig};

/// Pending control frames (pongs/close); a client exceeding it is dropped.
const MAX_PENDING_CONTROL: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Name advertised in `/json/version`; defaults to the bound address.
    pub advertise_host: Option<String>,
    /// Defaults match upstream `lightpanda serve`.
    pub max_connections: usize,
    pub max_http_message_size: usize,
    pub ws: WsConfig,
    /// Time a client gets to send a complete request head.
    pub head_timeout: Duration,
    /// Time a partially received WebSocket frame may stay incomplete.
    pub frame_timeout: Duration,
    /// Time a single socket write may stall (client not reading).
    pub write_timeout: Duration,
    /// Time to finish writing after either direction ended.
    pub close_timeout: Duration,
    /// Time the session may accept no message while the read-ahead buffer is full.
    pub stall_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            advertise_host: None,
            max_connections: 16,
            max_http_message_size: 1024 * 1024,
            ws: WsConfig { max_message_size: 1024 * 1024 },
            head_timeout: Duration::from_secs(10),
            frame_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            close_timeout: Duration::from_secs(5),
            stall_timeout: Duration::from_secs(30),
        }
    }
}

/// Messages delivered to the browser session, in arrival order.
#[derive(Debug)]
pub enum Inbound {
    Text(String),
    /// The client went away (close frame, EOF or protocol error). The session
    /// must dispose its contexts and then drop its outbound sender, which
    /// lets the connection finish.
    Closed,
}

pub struct SessionChannels {
    /// Bounded: when the session falls behind, reading from the socket stops,
    /// pushing back on the client instead of buffering without limit.
    /// Dropping it (or sending [`Inbound::Closed`]) ends the session.
    pub inbound: mpsc::Sender<Inbound>,
    /// Byte-budgeted outbound frames (see [`crate::outbox::Outbox`]).
    pub outbound: OutboundRx,
    /// Resolves when the session (e.g. its browser thread) has fully
    /// stopped; the connection slot is held until then.
    pub finished: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Called once when the connection ends, before waiting for `finished`;
    /// lets the session interrupt work that would otherwise delay teardown.
    pub on_close: Option<Box<dyn FnOnce() + Send>>,
}

/// Starts a browser session for a newly upgraded connection.
pub trait SessionFactory: Send + Sync + 'static {
    fn start(&self, peer: SocketAddr) -> std::io::Result<SessionChannels>;
}

pub struct Server<F> {
    listener: TcpListener,
    authority: String,
    config: ServerConfig,
    factory: Arc<F>,
}

impl<F: SessionFactory> Server<F> {
    pub async fn bind(addr: SocketAddr, config: ServerConfig, factory: F) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let authority = match &config.advertise_host {
            Some(host) => format!("{host}:{}", local.port()),
            None => local.to_string(),
        };
        Ok(Self { listener, authority, config, factory: Arc::new(factory) })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts connections forever. At `max_connections` live connections the
    /// listener stops accepting until one closes (connections queue in the
    /// kernel backlog), as upstream does.
    pub async fn run(self) -> std::io::Result<()> {
        let permits = Arc::new(Semaphore::new(self.config.max_connections));
        let shared = Arc::new((self.authority, self.config));
        loop {
            let permit = permits.clone().acquire_owned().await.expect("semaphore is never closed");
            let (stream, peer) = self.listener.accept().await?;
            let _ = stream.set_nodelay(true);
            let factory = self.factory.clone();
            let shared = shared.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let (authority, config) = &*shared;
                let _ = serve_connection(stream, peer, authority, config, &*factory).await;
            });
        }
    }
}

async fn serve_connection<F: SessionFactory>(
    mut stream: TcpStream,
    peer: SocketAddr,
    authority: &str,
    config: &ServerConfig,
    factory: &F,
) -> std::io::Result<()> {
    let head_limit = config.max_http_message_size.min(http::MAX_HEAD);
    let (head, buf, head_len) =
        match tokio::time::timeout(config.head_timeout, read_head(&mut stream, head_limit)).await {
            Ok(Ok(Some(parsed))) => parsed,
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(status)) => {
                let body: &[u8] = if status == 413 { b"request head too large\n" } else { b"malformed request\n" };
                return stream.write_all(&http::response(status, "text/plain", body)).await;
            }
            Err(_) => return Ok(()), // head deadline passed: drop the connection
        };
    let info = ServerInfo {
        authority,
        product: crate::target::PRODUCT,
        user_agent: crate::target::USER_AGENT,
        protocol_version: crate::target::PROTOCOL_VERSION,
    };
    let handshake = match http::route(&head, &info) {
        Routed::Response(bytes) => {
            stream.write_all(&bytes).await?;
            return stream.shutdown().await;
        }
        Routed::Upgrade(handshake) => handshake,
    };
    stream.write_all(&handshake).await?;

    let SessionChannels { inbound, outbound, finished, on_close } = factory.start(peer)?;
    let (reader, writer) = stream.into_split();
    let (control_tx, control_rx) = mpsc::channel::<Vec<u8>>(MAX_PENDING_CONTROL);
    let mut decoder = Decoder::new(config.ws);
    decoder.feed(&buf[head_len..]);
    let side = ReadSide {
        reader,
        decoder,
        chunk: vec![0u8; 64 * 1024],
        read_ahead: config.ws.max_message_size.max(64 * 1024).saturating_mul(2),
        frame_timeout: config.frame_timeout,
        stall_timeout: config.stall_timeout,
    };
    let mut write_task = tokio::spawn(write_loop(writer, outbound, control_rx, config.write_timeout));
    let read = read_loop(side, &inbound, &control_tx);
    // Whichever direction ends first ends the connection: a finished writer
    // (session gone, write error/stall) must not leave the reader holding the
    // connection permit, and vice versa.
    let (result, writer_done) = tokio::select! {
        result = read => (result, false),
        _ = &mut write_task => (Ok(()), true),
    };
    // Tell the session exactly once; dropping the sender also signals it.
    let _ = inbound.try_send(Inbound::Closed);
    drop(inbound);
    drop(control_tx);
    if let Some(on_close) = on_close {
        on_close();
    }
    if !writer_done && tokio::time::timeout(config.close_timeout, &mut write_task).await.is_err() {
        write_task.abort();
    }
    // The connection slot also bounds browser threads: hold it until the
    // session's thread has finished tearing down.
    if let Some(finished) = finished {
        let _ = finished.await;
    }
    result
}

/// Reads until a complete request head; `Ok(None)` on EOF, `Err(status)` for
/// oversized (413) or malformed (400) heads.
async fn read_head(
    stream: &mut TcpStream,
    head_limit: usize,
) -> Result<Option<(http::RequestHead, Vec<u8>, usize)>, u16> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        match http::parse_head(&buf) {
            Ok(Some((_, len))) if len > head_limit => return Err(413),
            Ok(Some((head, len))) => return Ok(Some((head, buf, len))),
            Ok(None) if buf.len() <= head_limit => {}
            Ok(None) | Err(HeadError::TooLarge) => return Err(413),
            Err(HeadError::Malformed) => return Err(400),
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return Ok(None),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Queues a control frame; a client that floods pings faster than it reads
/// pongs is disconnected instead of growing the queue.
fn control(tx: &mpsc::Sender<Vec<u8>>, frame: Vec<u8>) -> Result<(), ()> {
    tx.try_send(frame).map_err(|_| ())
}

/// Socket reading state of one connection.
struct ReadSide {
    reader: tokio::net::tcp::OwnedReadHalf,
    decoder: Decoder,
    chunk: Vec<u8>,
    /// Bytes buffered ahead of the session before reading pauses.
    read_ahead: usize,
    frame_timeout: Duration,
    stall_timeout: Duration,
}

async fn read_loop(
    mut side: ReadSide,
    inbound: &mpsc::Sender<Inbound>,
    control_tx: &mpsc::Sender<Vec<u8>>,
) -> std::io::Result<()> {
    // Absolute deadline for the frame/message currently being received: an
    // idle connection may wait indefinitely, an incomplete message may not,
    // however slowly its bytes trickle in.
    let mut message_deadline: Option<tokio::time::Instant> = None;
    loop {
        loop {
            let decode = crate::perf::now();
            let next = side.decoder.next_message();
            if matches!(next, Ok(Some(Message::Text(_)))) {
                crate::perf::record_since("io.ws_decode", decode);
            }
            let close = match next {
                Ok(Some(Message::Text(text))) => match deliver(&mut side, inbound, Inbound::Text(text)).await {
                    Delivery::Sent => None,
                    // Session ended on its own; close politely.
                    Delivery::SessionGone => Some((1000, "")),
                    Delivery::Stalled => Some((1008, "session is not accepting messages")),
                    Delivery::PeerGone => return Ok(()),
                },
                Ok(Some(Message::Binary(_))) => Some((1003, "binary messages are not supported")),
                Ok(Some(Message::Ping(data))) => {
                    if control(control_tx, ws::control_frame(Control::Pong(&data))).is_err() {
                        return Ok(());
                    }
                    None
                }
                Ok(Some(Message::Pong(_))) => None,
                Ok(Some(Message::Close(code))) => Some((code.map_or(1000, |(c, _)| c), "")),
                Ok(None) => break,
                Err(error) => {
                    // The client only sees a closed socket; leave the reason where operators look.
                    eprintln!("lightningpanda: closing CDP connection: {error}");
                    Some((error.close_code(), error.close_reason()))
                }
            };
            if let Some((code, reason)) = close {
                let _ = control(control_tx, ws::control_frame(Control::Close(code, reason)));
                return Ok(());
            }
        }
        if side.decoder.in_progress() {
            message_deadline.get_or_insert_with(|| tokio::time::Instant::now() + side.frame_timeout);
        } else {
            message_deadline = None;
        }
        let read = side.reader.read(&mut side.chunk);
        let n = match message_deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, read).await {
                Ok(n) => n?,
                Err(_) => {
                    let _ = control(control_tx, ws::control_frame(Control::Close(1008, "incomplete message timeout")));
                    return Ok(());
                }
            },
            None => read.await?,
        };
        if n == 0 {
            return Ok(());
        }
        side.decoder.feed(&side.chunk[..n]);
    }
}

enum Delivery {
    Sent,
    SessionGone,
    /// The session accepted nothing for `stall_timeout` while the read-ahead
    /// buffer was full.
    Stalled,
    PeerGone,
}

/// Waits for room in the session's inbound queue. Meanwhile it keeps reading
/// the socket into the decoder (up to the read-ahead bound), so a client that
/// leaves is noticed even with unread data queued before its FIN. When the
/// read-ahead bound is reached and the session still accepts nothing for
/// `stall_timeout`, the connection is given up.
async fn deliver(side: &mut ReadSide, inbound: &mpsc::Sender<Inbound>, message: Inbound) -> Delivery {
    let mut message = Some(message);
    let mut stall: Option<tokio::time::Instant> = None;
    loop {
        let can_read = side.decoder.buffered() < side.read_ahead;
        if can_read {
            stall = None;
        } else {
            stall.get_or_insert_with(|| tokio::time::Instant::now() + side.stall_timeout);
        }
        let stall_at = stall.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            permit = inbound.reserve() => {
                return match permit {
                    Ok(permit) => {
                        permit.send(message.take().expect("sent once"));
                        Delivery::Sent
                    }
                    Err(_) => Delivery::SessionGone,
                };
            }
            read = side.reader.read(&mut side.chunk), if can_read => match read {
                Ok(0) | Err(_) => return Delivery::PeerGone,
                Ok(n) => side.decoder.feed(&side.chunk[..n]),
            },
            _ = tokio::time::sleep_until(stall_at), if stall.is_some() => return Delivery::Stalled,
        }
    }
}

async fn write_all(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    bytes: &[u8],
    timeout: Duration,
) -> std::io::Result<()> {
    match tokio::time::timeout(timeout, writer.write_all(bytes)).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "client stopped reading")),
    }
}

async fn write_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut outbound: OutboundRx,
    mut control: mpsc::Receiver<Vec<u8>>,
    timeout: Duration,
) -> std::io::Result<()> {
    loop {
        tokio::select! {
            // Control frames (pong/close) jump ahead of queued data frames.
            biased;
            frame = control.recv() => match frame {
                Some(bytes) => {
                    let is_close = bytes.first() == Some(&0x88);
                    write_all(&mut writer, &bytes, timeout).await?;
                    if is_close {
                        return writer.shutdown().await;
                    }
                }
                // The reader finished without a close handshake (EOF or I/O
                // error): the peer is gone, queued frames cannot be delivered.
                None => return writer.shutdown().await,
            },
            frame = outbound.recv() => match frame {
                Some(Outbound::Frame(frame)) => {
                    #[cfg(feature = "perf-trace")]
                    crate::perf::record_since("io.outbound_queue", frame.queued_at);
                    let write = crate::perf::now();
                    write_all(&mut writer, frame.as_ref(), timeout).await?;
                    crate::perf::record_since("io.ws_write", write);
                }
                Some(Outbound::Exceeded) => {
                    let close = ws::control_frame(Control::Close(1008, "outbound buffer limit exceeded"));
                    write_all(&mut writer, &close, timeout).await?;
                    return writer.shutdown().await;
                }
                None => {
                    // Session finished first: tell the client.
                    write_all(&mut writer, &ws::control_frame(Control::Close(1000, "")), timeout).await?;
                    return writer.shutdown().await;
                }
            },
        }
    }
}
