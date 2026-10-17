//! Tokio I/O side of the CDP server: accepts TCP connections, answers the
//! discovery endpoints, upgrades to WebSocket and shuttles text messages to and
//! from a per-connection browser session running on its own thread.
//!
//! The browser side is abstracted by [`SessionFactory`]: for every upgraded
//! connection it returns a channel for inbound messages and a channel of
//! encoded outbound frames. Ordering is preserved in both directions.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::http::{self, HeadError, Routed, ServerInfo};
use crate::ws::{self, Control, Decoder, FrameBytes, Message, WsConfig};

#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Name advertised in `/json/version`; defaults to the bound address.
    pub advertise_host: Option<String>,
    /// Defaults match upstream `lightpanda serve`.
    pub max_connections: usize,
    pub max_http_message_size: usize,
    pub ws: WsConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            advertise_host: None,
            max_connections: 16,
            max_http_message_size: 1024 * 1024,
            ws: WsConfig { max_message_size: 1024 * 1024 },
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

/// Outbound wire frames produced by the browser session. Unbounded because
/// protocol events cannot be dropped; a client that stops reading only grows
/// its own connection's queue.
pub type Outbound = FrameBytes;

pub struct SessionChannels {
    /// Bounded: when the session falls behind, reading from the socket stops,
    /// pushing back on the client instead of buffering without limit.
    pub inbound: mpsc::Sender<Inbound>,
    pub outbound: mpsc::UnboundedReceiver<Outbound>,
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
    let mut buf = Vec::with_capacity(4096);
    let head_limit = config.max_http_message_size.min(http::MAX_HEAD);
    let (head, head_len) = loop {
        match http::parse_head(&buf) {
            Ok(Some(parsed)) => break parsed,
            Ok(None) if buf.len() <= head_limit => {}
            Ok(None) | Err(HeadError::TooLarge) => {
                return stream.write_all(&http::response(413, "text/plain", b"request head too large\n")).await;
            }
            Err(HeadError::Malformed) => {
                return stream.write_all(&http::response(400, "text/plain", b"malformed request\n")).await;
            }
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
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

    let SessionChannels { inbound, outbound } = factory.start(peer)?;
    let (reader, writer) = stream.into_split();
    let (control_tx, control_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut decoder = Decoder::new(config.ws);
    decoder.feed(&buf[head_len..]);
    let write_task = tokio::spawn(write_loop(writer, outbound, control_rx));
    let read_result = read_loop(reader, decoder, &inbound, &control_tx).await;
    let _ = inbound.send(Inbound::Closed).await;
    drop(control_tx);
    // The writer ends once the session drops its outbound sender.
    let _ = write_task.await;
    read_result
}

async fn read_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    mut decoder: Decoder,
    inbound: &mpsc::Sender<Inbound>,
    control: &mpsc::UnboundedSender<Vec<u8>>,
) -> std::io::Result<()> {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        loop {
            match decoder.next_message() {
                Ok(Some(Message::Text(text))) => {
                    if inbound.send(Inbound::Text(text)).await.is_err() {
                        // Session ended on its own; close politely.
                        let _ = control.send(ws::control_frame(Control::Close(1000, "")));
                        return Ok(());
                    }
                }
                Ok(Some(Message::Binary(_))) => {
                    let _ = control.send(ws::control_frame(Control::Close(1003, "binary messages are not supported")));
                    return Ok(());
                }
                Ok(Some(Message::Ping(data))) => {
                    let _ = control.send(ws::control_frame(Control::Pong(&data)));
                }
                Ok(Some(Message::Pong(_))) => {}
                Ok(Some(Message::Close(code))) => {
                    let code = code.map_or(1000, |(c, _)| c);
                    let _ = control.send(ws::control_frame(Control::Close(code, "")));
                    return Ok(());
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = control.send(ws::control_frame(Control::Close(error.close_code(), "")));
                    return Ok(());
                }
            }
        }
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        decoder.feed(&chunk[..n]);
    }
}

async fn write_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut outbound: mpsc::UnboundedReceiver<Outbound>,
    mut control: mpsc::UnboundedReceiver<Vec<u8>>,
) -> std::io::Result<()> {
    let mut control_open = true;
    loop {
        tokio::select! {
            // Control frames (pong/close) jump ahead of queued data frames.
            biased;
            frame = control.recv(), if control_open => match frame {
                Some(bytes) => {
                    let is_close = bytes.first() == Some(&0x88);
                    writer.write_all(&bytes).await?;
                    if is_close {
                        return writer.shutdown().await;
                    }
                }
                None => control_open = false,
            },
            frame = outbound.recv() => match frame {
                Some(frame) => writer.write_all(frame.as_ref()).await?,
                None => {
                    if control_open {
                        // Session finished first: tell the client.
                        writer.write_all(&ws::control_frame(Control::Close(1000, ""))).await?;
                    }
                    return writer.shutdown().await;
                }
            },
        }
    }
}
