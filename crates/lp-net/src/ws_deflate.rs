//! permessage-deflate (RFC 7692) for WebSocket client connections.
//!
//! The client offers what Chrome offers (`permessage-deflate;
//! client_max_window_bits`) and accepts what Chromium accepts in the
//! response. A negotiated connection gets a [`Deflate`]; nothing here runs on
//! a connection without it. The compressor and decompressor are allocated on
//! the first message that needs them.
//!
//! - Inbound messages with RSV1 set are inflated with a 32 KiB window (which
//!   decodes any `server_max_window_bits`). The decompressor keeps its window
//!   across messages (context takeover). A final block resets it, and the
//!   bytes after it in the same message are inflated as a new stream (as in
//!   Chromium). The output is bounded by the connection's message limit, so a
//!   small compressed message cannot expand without bound, and it is produced
//!   in slices with a yield between them, so a large expansion does not hold
//!   the shared network thread.
//! - Outbound messages are deflated with a sync flush and the trailing
//!   `00 00 ff ff` removed. The compressor keeps its window across messages
//!   unless the server asked for `client_no_context_takeover`. A large
//!   message is compressed in slices with a yield between them.
//! - The compressor cannot shrink its window, so when the server limits the
//!   client to `client_max_window_bits` < 15, the compressor is reset before
//!   every message and a message longer than the window is sent uncompressed
//!   (RSV1 clear, which RFC 7692 allows for any message). No back-reference
//!   can then reach farther than the window.

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use http::{header, HeaderMap};

/// The request's `Sec-WebSocket-Extensions`, as Chrome sends it.
pub(crate) const OFFER: &str = "permessage-deflate; client_max_window_bits";

/// The tail a sync flush ends with, removed from and added back to every
/// compressed message (RFC 7692 7.2.1, 7.2.2).
const TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

/// The most an inbound message inflates by between yields to the network
/// runtime (about a millisecond of inflating).
const INFLATE_SLICE: usize = 1 << 20;

/// The most of an outbound message deflated between yields to the network
/// runtime (a few milliseconds at the default level).
const COMPRESS_SLICE: usize = 256 << 10;

/// Negotiated parameters (RFC 7692 7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Params {
    pub(crate) server_no_context_takeover: bool,
    pub(crate) client_no_context_takeover: bool,
    pub(crate) server_max_window_bits: u8,
    pub(crate) client_max_window_bits: u8,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            server_no_context_takeover: false,
            client_no_context_takeover: false,
            server_max_window_bits: 15,
            client_max_window_bits: 15,
        }
    }
}

/// One parsed extension: its name and parameters (values unquoted).
#[derive(Debug, PartialEq, Eq)]
struct Extension {
    name: String,
    params: Vec<(String, Option<String>)>,
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_tchar)
}

/// RFC 6455 9.1's `extension-list`, strictly: no empty elements, and a quoted
/// parameter value must be a token once unquoted (as in Chromium).
fn parse_extensions(value: &str) -> Option<Vec<Extension>> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t') {
                self.i += 1;
            }
        }
        fn token(&mut self) -> Option<String> {
            self.ws();
            let start = self.i;
            while self.i < self.s.len() && is_tchar(self.s[self.i]) {
                self.i += 1;
            }
            (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
        }
        fn eat(&mut self, c: u8) -> bool {
            self.ws();
            if self.s.get(self.i) == Some(&c) {
                self.i += 1;
                true
            } else {
                false
            }
        }
        fn quoted(&mut self) -> Option<String> {
            // At the opening quote.
            self.i += 1;
            let mut out = Vec::new();
            loop {
                let c = *self.s.get(self.i)?;
                self.i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        out.push(*self.s.get(self.i)?);
                        self.i += 1;
                    }
                    c => out.push(c),
                }
            }
            let out = String::from_utf8(out).ok()?;
            is_token(&out).then_some(out)
        }
    }
    let mut p = P { s: value.as_bytes(), i: 0 };
    let mut list = Vec::new();
    loop {
        let name = p.token()?;
        let mut params = Vec::new();
        while p.eat(b';') {
            let key = p.token()?;
            let value = if p.eat(b'=') {
                p.ws();
                if p.s.get(p.i) == Some(&b'"') {
                    Some(p.quoted()?)
                } else {
                    Some(p.token()?)
                }
            } else {
                None
            };
            params.push((key, value));
        }
        list.push(Extension { name, params });
        if !p.eat(b',') {
            break;
        }
    }
    p.ws();
    (p.i == p.s.len()).then_some(list)
}

/// A window-bits value: 8 to 15, decimal digits, no leading zero.
fn window_bits(value: &str) -> Option<u8> {
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) || value.len() > 2 {
        return None;
    }
    value.parse::<u8>().ok().filter(|b| (8..=15).contains(b))
}

/// Chromium's validation of permessage-deflate response parameters.
fn deflate_params(ext: &Extension) -> Result<Params, String> {
    let mut params = Params::default();
    let mut seen: [bool; 4] = [false; 4];
    for (key, value) in &ext.params {
        let slot = match key.as_str() {
            "server_no_context_takeover" => 0,
            "client_no_context_takeover" => 1,
            "server_max_window_bits" => 2,
            "client_max_window_bits" => 3,
            _ => return Err("Received an unexpected permessage-deflate extension parameter".into()),
        };
        if std::mem::replace(&mut seen[slot], true) {
            return Err(format!("Received duplicate permessage-deflate extension parameter {key}"));
        }
        match slot {
            0 | 1 => {
                if value.is_some() {
                    return Err(format!("Received invalid {key} parameter"));
                }
                if slot == 0 {
                    params.server_no_context_takeover = true;
                } else {
                    params.client_no_context_takeover = true;
                }
            }
            _ => {
                let Some(bits) = value.as_deref().and_then(window_bits) else {
                    return Err(format!("Received invalid {key} parameter"));
                };
                if slot == 2 {
                    params.server_max_window_bits = bits;
                } else {
                    params.client_max_window_bits = bits;
                }
            }
        }
    }
    Ok(params)
}

/// Validates the handshake response's `Sec-WebSocket-Extensions` against
/// the offer. `Ok(None)`: no extension was accepted. `Ok(Some(..))`: the
/// negotiated parameters and the value of `WebSocket.extensions` (the
/// response's header values joined, as in Chromium). `Err`: the reason the
/// connection fails.
pub(crate) fn negotiate(headers: &HeaderMap) -> Result<Option<(Params, String)>, String> {
    let mut values = Vec::new();
    let mut negotiated: Option<Params> = None;
    for value in headers.get_all(header::SEC_WEBSOCKET_EXTENSIONS) {
        let text = value.to_str().ok();
        let Some(list) = text.and_then(parse_extensions) else {
            return Err(format!(
                "Error during WebSocket handshake: 'Sec-WebSocket-Extensions' header value is rejected by the parser: {}",
                String::from_utf8_lossy(value.as_bytes())
            ));
        };
        for ext in &list {
            if ext.name != "permessage-deflate" {
                return Err(format!(
                    "Error during WebSocket handshake: Found an unsupported extension '{}' in 'Sec-WebSocket-Extensions' header",
                    ext.name
                ));
            }
            if negotiated.is_some() {
                return Err("Error during WebSocket handshake: Received duplicate permessage-deflate response".into());
            }
            negotiated = Some(
                deflate_params(ext)
                    .map_err(|e| format!("Error during WebSocket handshake: Error in permessage-deflate: {e}"))?,
            );
        }
        values.extend(text.map(str::to_owned));
    }
    Ok(negotiated.map(|params| (params, values.join(", "))))
}

/// The compression state of a negotiated connection.
pub(crate) struct Deflate {
    params: Params,
    deflater: Option<Box<Compress>>,
    inflater: Option<Box<Decompress>>,
}

/// Why an inbound message could not be inflated: the Close code and message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InflateError(pub(crate) u16, pub(crate) String);

impl Deflate {
    pub(crate) fn new(params: Params) -> Self {
        Deflate { params, deflater: None, inflater: None }
    }

    /// The compressed payload of an outbound message (to send with RSV1
    /// set), or `None` to send it as is.
    ///
    /// The payload goes in [`COMPRESS_SLICE`]-byte slices, and the task
    /// yields between slices, so a large message does not hold the network
    /// thread for the whole of its compression.
    pub(crate) async fn compress(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let bits = self.params.client_max_window_bits;
        let limited = bits < 15;
        if limited && payload.len() > 1usize << bits {
            return None;
        }
        let c = self.deflater.get_or_insert_with(|| Box::new(Compress::new(Compression::default(), false)));
        let mut out = Vec::with_capacity((payload.len() / 2).min(COMPRESS_SLICE) + 64);
        let mut consumed = 0usize;
        loop {
            let end = payload.len().min(consumed + COMPRESS_SLICE);
            // Only the last slice flushes; the others leave the stream open.
            let flush = if end == payload.len() { FlushCompress::Sync } else { FlushCompress::None };
            loop {
                if out.capacity() - out.len() < 64 {
                    out.reserve(out.capacity().max(1024));
                }
                let before = c.total_in();
                if c.compress_vec(&payload[consumed..end], &mut out, flush).is_err() {
                    // The in-memory compressor does not fail; if it did, its
                    // state is unknown, so start over and send this one as is.
                    c.reset();
                    return None;
                }
                consumed += (c.total_in() - before) as usize;
                // Done with the slice once its input is in and the flush (if
                // any) did not fill the buffer.
                if consumed == end && out.capacity() - out.len() >= 64 {
                    break;
                }
            }
            if consumed == payload.len() {
                break;
            }
            tokio::task::yield_now().await;
        }
        if out.ends_with(&TAIL) {
            out.truncate(out.len() - TAIL.len());
        }
        if limited || self.params.client_no_context_takeover {
            c.reset();
        }
        Some(out)
    }

    /// Inflates a compressed inbound message of at most `max` bytes.
    ///
    /// Each inflate call writes at most [`INFLATE_SLICE`] bytes, and the
    /// task yields after every slice, so a small message that expands a
    /// thousandfold does not hold the network thread until it reaches `max`.
    /// A final block (BFINAL) resets the decompressor and inflating goes on
    /// with the bytes after it, as in Chromium.
    pub(crate) async fn decompress(&mut self, payload: &[u8], max: usize) -> Result<Vec<u8>, InflateError> {
        let d = self.inflater.get_or_insert_with(|| Box::new(Decompress::new(false)));
        let too_big = || InflateError(1009, "Message is too big".into());
        let corrupt = || InflateError(1002, "Invalid frame header".into());
        // Room for one byte past the limit, to tell "exactly max" from more.
        let cap = max.saturating_add(1);
        let mut out: Vec<u8> = Vec::new();
        let mut since_yield = 0usize;
        // Whether the input so far ended in a final block, with nothing of a
        // new stream after it.
        let mut stream_ended = false;
        for chunk in [payload, &TAIL[..]] {
            if stream_ended {
                // The tail completes a sync flush; a stream that ended has
                // none, and a fresh one would misread it as a block header.
                break;
            }
            let mut pos = 0usize;
            loop {
                let filled = out.len();
                if filled >= cap {
                    return Err(too_big());
                }
                // Zero only the room this call may use; `resize` grows the
                // allocation geometrically.
                let room = filled.max(payload.len().saturating_mul(2)).clamp(4096, INFLATE_SLICE).min(cap - filled);
                out.resize(filled + room, 0);
                let (in_before, out_before) = (d.total_in(), d.total_out());
                let status = d.decompress(&chunk[pos..], &mut out[filled..], FlushDecompress::Sync);
                let written = (d.total_out() - out_before) as usize;
                out.truncate(filled + written);
                let status = status.map_err(|_| corrupt())?;
                let read = (d.total_in() - in_before) as usize;
                pos += read;
                if out.len() > max {
                    return Err(too_big());
                }
                since_yield += written;
                let spare = written < room;
                if status == Status::StreamEnd {
                    // A final block ends the deflate stream; what follows in
                    // the message starts a new one.
                    d.reset(false);
                    stream_ended = true;
                } else if read > 0 {
                    stream_ended = false;
                }
                if status != Status::StreamEnd && spare && read == 0 && written == 0 && pos < chunk.len() {
                    // No progress with input and room left: not a stream.
                    return Err(corrupt());
                }
                if pos == chunk.len() && spare {
                    break;
                }
                if since_yield >= INFLATE_SLICE {
                    since_yield = 0;
                    tokio::task::yield_now().await;
                }
            }
        }
        if self.params.server_no_context_takeover {
            d.reset(false);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn response(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(header::SEC_WEBSOCKET_EXTENSIONS, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn negotiation_accepts_chromium_parameters() {
        assert_eq!(negotiate(&HeaderMap::new()), Ok(None));
        let (p, desc) = negotiate(&response(&["permessage-deflate"])).unwrap().unwrap();
        assert_eq!(p, Params::default());
        assert_eq!(desc, "permessage-deflate");
        let v = "permessage-deflate; server_no_context_takeover; client_no_context_takeover; server_max_window_bits=10; client_max_window_bits=\"9\"";
        let (p, desc) = negotiate(&response(&[v])).unwrap().unwrap();
        assert_eq!(
            p,
            Params {
                server_no_context_takeover: true,
                client_no_context_takeover: true,
                server_max_window_bits: 10,
                client_max_window_bits: 9
            }
        );
        assert_eq!(desc, v);
    }

    #[test]
    fn negotiation_rejects_what_chromium_rejects() {
        for bad in [
            "",
            "permessage-deflate;",
            "permessage-deflate,",
            "permessage-deflate; client_max_window_bits",
            "permessage-deflate; server_max_window_bits=16",
            "permessage-deflate; server_max_window_bits=7",
            "permessage-deflate; server_max_window_bits=010",
            "permessage-deflate; server_max_window_bits=\"1 0\"",
            "permessage-deflate; server_no_context_takeover=1",
            "permessage-deflate; server_no_context_takeover; server_no_context_takeover",
            "permessage-deflate; foo",
            "permessage-deflate, permessage-deflate",
            "x-webkit-deflate-frame",
            "permessage-deflate; server_max_window_bits=\"10",
        ] {
            assert!(negotiate(&response(&[bad])).is_err(), "{bad:?} must fail");
        }
        assert!(negotiate(&response(&["permessage-deflate", "permessage-deflate"])).is_err());
    }

    fn inflate_reference(data: &[u8]) -> Vec<u8> {
        let mut d = Decompress::new(false);
        let mut input = data.to_vec();
        input.extend_from_slice(&TAIL);
        let mut out = Vec::with_capacity(1 << 20);
        d.decompress_vec(&input, &mut out, FlushDecompress::Sync).unwrap();
        out
    }

    #[tokio::test]
    async fn round_trip_with_and_without_context_takeover() {
        for no_context in [false, true] {
            let params = Params {
                client_no_context_takeover: no_context,
                server_no_context_takeover: no_context,
                ..Default::default()
            };
            let (mut a, mut b) = (Deflate::new(params), Deflate::new(params));
            let msg = b"Hello, permessage-deflate! Hello, permessage-deflate!".repeat(20);
            let first = a.compress(&msg).await.unwrap();
            assert!(first.len() < msg.len());
            let second = a.compress(&msg).await.unwrap();
            // With context takeover the repeat is a back-reference only.
            if no_context {
                assert_eq!(first, second);
            } else {
                assert!(second.len() < first.len());
            }
            assert_eq!(b.decompress(&first, 1 << 20).await.unwrap(), msg);
            assert_eq!(b.decompress(&second, 1 << 20).await.unwrap(), msg);
            assert_eq!(b.decompress(&a.compress(b"").await.unwrap(), 10).await.unwrap(), b"");
        }
    }

    #[tokio::test]
    async fn limited_client_window_resets_and_skips_long_messages() {
        let params = Params { client_max_window_bits: 8, ..Default::default() };
        let mut a = Deflate::new(params);
        assert!(a.compress(&[b'x'; 300]).await.is_none());
        let small = b"abcabcabcabc".repeat(10);
        let one = a.compress(&small).await.unwrap();
        let two = a.compress(&small).await.unwrap();
        assert_eq!(one, two, "no context carried over");
        assert_eq!(inflate_reference(&one), small);
    }

    #[tokio::test]
    async fn hostile_input_is_bounded_and_rejected() {
        let mut big = Deflate::new(Params::default());
        let zeros = vec![0u8; 4 << 20];
        let bomb = big.compress(&zeros).await.unwrap();
        assert!(bomb.len() < 8 << 10);
        let mut d = Deflate::new(Params::default());
        assert_eq!(d.decompress(&bomb, 1 << 20).await.unwrap_err().0, 1009);
        let mut d = Deflate::new(Params::default());
        assert_eq!(d.decompress(&bomb, 4 << 20).await.unwrap().len(), 4 << 20);
        let mut d = Deflate::new(Params::default());
        assert_eq!(d.decompress(&[0xff, 0xff, 0xff, 0xff, 0x00], 1 << 20).await.unwrap_err().0, 1002);
        // A final block resets the stream for the next message.
        let mut c = flate2::write::DeflateEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut c, b"final").unwrap();
        let fin = c.finish().unwrap();
        let mut d = Deflate::new(Params::default());
        assert_eq!(d.decompress(&fin, 100).await.unwrap(), b"final");
        assert_eq!(d.decompress(&fin, 100).await.unwrap(), b"final");
    }
    #[tokio::test]
    async fn data_after_a_final_block_is_inflated() {
        // Two finished streams in one message, as Chromium inflates them.
        let finished = |text: &[u8]| {
            let mut c = flate2::write::DeflateEncoder::new(Vec::new(), Compression::default());
            std::io::Write::write_all(&mut c, text).unwrap();
            c.finish().unwrap()
        };
        let mut both = finished(b"first, ");
        both.extend(finished(b"second"));
        let mut d = Deflate::new(Params::default());
        assert_eq!(d.decompress(&both, 100).await.unwrap(), b"first, second");
        // A final block followed by sync-flushed blocks of a new stream.
        let mut a = Deflate::new(Params::default());
        let mut mixed = finished(b"head ");
        mixed.extend(a.compress(b"tail").await.unwrap());
        assert_eq!(d.decompress(&mixed, 100).await.unwrap(), b"head tail");
        // The connection goes on with fresh context after the final block.
        let next = a.compress(b"tail again").await.unwrap();
        assert_eq!(d.decompress(&next, 100).await.unwrap(), b"tail again");
    }

    /// Counts how often other tasks run while `work` is awaited.
    async fn polls_of_others_during<F: std::future::Future>(work: F) -> (F::Output, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = ticks.clone();
        let other = tokio::spawn(async move {
            loop {
                counter.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
            }
        });
        // Let the other task start.
        tokio::task::yield_now().await;
        let start = ticks.load(Ordering::Relaxed);
        let output = work.await;
        let ran = ticks.load(Ordering::Relaxed) - start;
        other.abort();
        (output, ran)
    }

    #[tokio::test]
    async fn large_inflate_yields_to_other_tasks() {
        let mut big = Deflate::new(Params::default());
        let bomb = big.compress(&vec![0u8; 8 << 20]).await.unwrap();
        let mut d = Deflate::new(Params::default());
        let (out, ran) = polls_of_others_during(d.decompress(&bomb, 8 << 20)).await;
        assert_eq!(out.unwrap(), vec![0u8; 8 << 20]);
        assert!(ran >= 4, "other tasks ran {ran} times during an 8 MiB inflate");
        // A small message does not yield.
        let small = big.compress(b"small").await.unwrap();
        let (out, ran) = polls_of_others_during(d.decompress(&small, 100)).await;
        assert_eq!(out.unwrap(), b"small");
        assert_eq!(ran, 0);
    }

    #[tokio::test]
    async fn large_compress_yields_and_round_trips() {
        // Varied bytes, so the deflater does real work and the output spans
        // many slices.
        let mut x = 0x2545_f491_u32;
        let msg: Vec<u8> = (0..(3 * COMPRESS_SLICE + 123))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                b'a' + (x % 16) as u8
            })
            .collect();
        let mut a = Deflate::new(Params::default());
        let (wire, ran) = polls_of_others_during(a.compress(&msg)).await;
        let wire = wire.unwrap();
        assert!(ran >= 3, "other tasks ran {ran} times during a sliced compress");
        assert!(!wire.ends_with(&TAIL));
        let mut b = Deflate::new(Params::default());
        assert_eq!(b.decompress(&wire, msg.len()).await.unwrap(), msg);
        // Context carries over across the sliced message.
        let again = a.compress(&msg[..1000]).await.unwrap();
        assert_eq!(b.decompress(&again, msg.len()).await.unwrap(), &msg[..1000]);
        let (_, ran) = polls_of_others_during(a.compress(b"short")).await;
        assert_eq!(ran, 0);
    }
}
