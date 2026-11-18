//! Which pooled HTTP/2 connections accept WebSockets (RFC 8441).
//!
//! hyper-util's pool does not expose its connections or their peer
//! settings, so [`ProbeConnector`] wraps the connector stack: a connection
//! that negotiated `h2` by ALPN is registered in [`H2Sessions`] under its
//! pool key, and its stream reads the server's first SETTINGS frame (the
//! server connection preface, RFC 9113 §3.4) on the way to hyper to learn
//! `SETTINGS_ENABLE_CONNECT_PROTOCOL`. After that frame the stream is a plain
//! pass-through; HTTP/1.1 connections never sniff. The registry holds weak
//! references, so a session disappears when hyper drops its connection.
//!
//! A WebSocket then sends its extended CONNECT through the same pooled
//! client, which hands it the same connection.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

use http::Uri;
use hyper::rt::{Read, ReadBuf, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use tower_service::Service;

/// SETTINGS frame type and its ENABLE_CONNECT_PROTOCOL parameter (RFC 8441 §3).
const SETTINGS: u8 = 0x4;
const ENABLE_CONNECT_PROTOCOL: u16 = 0x8;
/// A server preface SETTINGS frame longer than this is not inspected.
const MAX_PREFACE: usize = 16 * 1024;

/// One HTTP/2 connection's state as seen by the probe.
#[derive(Debug, Default)]
pub(crate) struct H2Session {
    connect_protocol: AtomicBool,
}

impl H2Session {
    pub(crate) fn connect_protocol(&self) -> bool {
        self.connect_protocol.load(Ordering::Acquire)
    }
}

/// The response extension naming the connection a response came on (hyper-util
/// copies a connection's `Connected::extra` into every response).
#[derive(Clone, Debug)]
pub(crate) struct SessionTag(pub(crate) Arc<H2Session>);

/// Live HTTP/2 connections of one pooled client, by pool key.
#[derive(Debug, Default)]
pub(crate) struct H2Sessions {
    map: Mutex<HashMap<String, Vec<Weak<H2Session>>>>,
}

/// The pool key of a URI: hyper-util pools by scheme and authority.
pub(crate) fn pool_key(uri: &Uri) -> Option<String> {
    Some(format!("{}://{}", uri.scheme_str()?, uri.authority()?.as_str()))
}

impl H2Sessions {
    fn register(&self, key: String) -> Arc<H2Session> {
        let session = Arc::new(H2Session::default());
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let list = map.entry(key).or_default();
        list.retain(|s| s.strong_count() > 0);
        list.push(Arc::downgrade(&session));
        session
    }

    /// A live connection to `uri`'s origin whose server enabled the extended
    /// CONNECT protocol, if there is one.
    pub(crate) fn websocket_session(&self, uri: &Uri) -> Option<Arc<H2Session>> {
        let key = pool_key(uri)?;
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let list = map.get_mut(&key)?;
        list.retain(|s| s.strong_count() > 0);
        let found = list.iter().rev().filter_map(Weak::upgrade).find(|s| s.connect_protocol());
        if list.is_empty() {
            map.remove(&key);
        }
        found
    }
}

/// The connector stack with HTTP/2 sessions registered in `sessions`.
#[derive(Clone)]
pub(crate) struct ProbeConnector<C> {
    inner: C,
    sessions: Arc<H2Sessions>,
}

impl<C> ProbeConnector<C> {
    pub(crate) fn new(inner: C, sessions: Arc<H2Sessions>) -> Self {
        ProbeConnector { inner, sessions }
    }
}

impl<C> Service<Uri> for ProbeConnector<C>
where
    C: Service<Uri>,
    C::Response: Connection + Read + Write + Unpin + Send + 'static,
    C::Future: Send + 'static,
{
    type Response = ProbeStream<C::Response>;
    type Error = C::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, C::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), C::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let key = pool_key(&uri);
        let sessions = self.sessions.clone();
        let connecting = self.inner.call(uri);
        Box::pin(async move {
            let inner = connecting.await?;
            let probe = match key {
                Some(key) if inner.connected().is_negotiated_h2() => {
                    Some(Box::new(Probe { session: sessions.register(key), buf: Vec::new(), done: false }))
                }
                _ => None,
            };
            Ok(ProbeStream { inner, probe })
        })
    }
}

struct Probe {
    session: Arc<H2Session>,
    /// Bytes of the server preface frame received so far.
    buf: Vec<u8>,
    /// The first frame was read: the stream passes bytes through.
    done: bool,
}

impl Probe {
    /// Feeds received bytes until the first frame has been seen.
    fn feed(&mut self, bytes: &[u8]) {
        if self.done {
            return;
        }
        self.buf.extend_from_slice(bytes);
        if self.buf.len() < 9 {
            return;
        }
        let len = u32::from_be_bytes([0, self.buf[0], self.buf[1], self.buf[2]]) as usize;
        let (kind, flags) = (self.buf[3], self.buf[4]);
        if kind != SETTINGS || flags & 0x1 != 0 || len > MAX_PREFACE || !len.is_multiple_of(6) {
            return self.finish();
        }
        if self.buf.len() < 9 + len {
            return;
        }
        for entry in self.buf[9..9 + len].chunks_exact(6) {
            if u16::from_be_bytes([entry[0], entry[1]]) == ENABLE_CONNECT_PROTOCOL {
                let value = u32::from_be_bytes([entry[2], entry[3], entry[4], entry[5]]);
                self.session.connect_protocol.store(value == 1, Ordering::Release);
            }
        }
        self.finish();
    }

    fn finish(&mut self) {
        self.done = true;
        self.buf = Vec::new();
    }
}

/// A connection's stream; it keeps its [`H2Session`] alive while open.
pub(crate) struct ProbeStream<T> {
    inner: T,
    probe: Option<Box<Probe>>,
}

impl<T: Connection> Connection for ProbeStream<T> {
    fn connected(&self) -> Connected {
        let connected = self.inner.connected();
        match &self.probe {
            Some(probe) => connected.extra(SessionTag(probe.session.clone())),
            None => connected,
        }
    }
}

/// The end of a TLS connection without close_notify (rustls: `UnexpectedEof`) read as the end of the stream, as
/// Chromium reads it. HTTP's own framing still catches a cut: a body shorter than its Content-Length, a chunked body
/// without its last chunk, an HTTP/2 stream without END_STREAM. What this lets through is a body delimited by the
/// connection's close (neither length nor chunks), which servers that skip close_notify end that way (wptserve,
/// whose Python ssl module closes without it).
fn eof_is_end(read: Poll<std::io::Result<()>>) -> Poll<std::io::Result<()>> {
    match read {
        Poll::Ready(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Poll::Ready(Ok(())),
        other => other,
    }
}

impl<T: Read + Unpin> Read for ProbeStream<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let probe = match &mut this.probe {
            Some(probe) if !probe.done => probe,
            _ => return eof_is_end(Pin::new(&mut this.inner).poll_read(cx, buf)),
        };
        // Until the preface frame is complete, read into a scratch buffer to
        // see the bytes (a cursor does not show what was written to it).
        let mut scratch = vec![0u8; buf.remaining().min(MAX_PREFACE)];
        let mut tmp = ReadBuf::new(&mut scratch);
        match Pin::new(&mut this.inner).poll_read(cx, tmp.unfilled()) {
            Poll::Ready(Ok(())) => {
                let filled = tmp.filled();
                probe.feed(filled);
                buf.put_slice(filled);
                Poll::Ready(Ok(()))
            }
            other => eof_is_end(other),
        }
    }
}

impl<T: Write + Unpin> Write for ProbeStream<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(entries: &[(u16, u32)], flags: u8) -> Vec<u8> {
        let len = entries.len() * 6;
        let mut out = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, SETTINGS, flags, 0, 0, 0, 0];
        for (id, value) in entries {
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&value.to_be_bytes());
        }
        out
    }

    fn probe() -> Probe {
        Probe { session: Arc::default(), buf: Vec::new(), done: false }
    }

    #[test]
    fn the_preface_settings_frame_is_read_across_chunks() {
        let frame = settings(&[(0x3, 100), (ENABLE_CONNECT_PROTOCOL, 1), (0x4, 65535)], 0);
        let mut p = probe();
        for byte in &frame {
            assert!(!p.done);
            p.feed(std::slice::from_ref(byte));
        }
        assert!(p.done && p.session.connect_protocol());
    }

    #[test]
    fn absent_zero_or_misplaced_settings_leave_it_off() {
        for frame in [
            settings(&[(0x3, 100)], 0),
            settings(&[(ENABLE_CONNECT_PROTOCOL, 0)], 0),
            // An ACK carries no parameters; a non-SETTINGS first frame ends the probe.
            settings(&[], 1),
            vec![0, 0, 8, 0x6, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8],
        ] {
            let mut p = probe();
            p.feed(&frame);
            p.feed(&settings(&[(ENABLE_CONNECT_PROTOCOL, 1)], 0));
            assert!(p.done && !p.session.connect_protocol(), "{frame:?}");
        }
    }

    #[test]
    fn sessions_are_found_by_origin_while_alive() {
        let sessions = H2Sessions::default();
        let uri: Uri = "https://example.test:8443/chat?x".parse().unwrap();
        let session = sessions.register(pool_key(&uri).unwrap());
        assert!(sessions.websocket_session(&uri).is_none(), "not enabled yet");
        session.connect_protocol.store(true, Ordering::Release);
        let found = sessions.websocket_session(&"https://example.test:8443/other".parse().unwrap()).unwrap();
        assert!(Arc::ptr_eq(&found, &session));
        assert!(sessions.websocket_session(&"https://example.test/".parse().unwrap()).is_none());
        drop((found, session));
        assert!(sessions.websocket_session(&uri).is_none());
        assert!(sessions.map.lock().unwrap().is_empty());
    }
}
