//! Sans-IO RFC 6455 server-side WebSocket framing.
//!
//! The decoder buffers raw socket bytes, enforces client masking and size
//! limits, reassembles fragmented messages and validates UTF-8 for text. The
//! encoder writes unmasked server frames; [`OutFrame`] lets callers serialize
//! a payload once into a buffer that already reserves room for the header.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsConfig {
    /// Largest reassembled message accepted from the client.
    pub max_message_size: usize,
}

impl Default for WsConfig {
    fn default() -> Self {
        // Playwright's injected scripts are around 1 MiB; setInputFiles payloads
        // may be larger. 64 MiB bounds memory per connection.
        Self { max_message_size: 64 * 1024 * 1024 }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// Close frame; `None` when the client sent no status code.
    Close(Option<(u16, String)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsError {
    Protocol(&'static str),
    InvalidUtf8,
    MessageTooBig,
}

impl WsError {
    /// Close status code to send before dropping the connection.
    pub fn close_code(&self) -> u16 {
        match self {
            WsError::Protocol(_) => 1002,
            WsError::InvalidUtf8 => 1007,
            WsError::MessageTooBig => 1009,
        }
    }

    /// Close frame reason, so a client sees why the connection ended.
    pub fn close_reason(&self) -> &'static str {
        match self {
            WsError::Protocol(reason) => reason,
            WsError::InvalidUtf8 => "text message is not valid UTF-8",
            WsError::MessageTooBig => "message exceeds --cdp-max-message-size",
        }
    }
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WsError::Protocol(reason) => write!(f, "websocket protocol error: {reason}"),
            WsError::InvalidUtf8 => f.write_str("websocket text message is not valid UTF-8"),
            WsError::MessageTooBig => f.write_str("websocket message exceeds the configured limit"),
        }
    }
}

impl std::error::Error for WsError {}

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

pub struct Decoder {
    config: WsConfig,
    buf: Vec<u8>,
    /// Bytes of `buf` already consumed.
    pos: usize,
    /// Opcode and payload of a fragmented message in progress.
    partial: Option<(u8, Vec<u8>)>,
    failed: bool,
}

impl Decoder {
    pub fn new(config: WsConfig) -> Self {
        Self { config, buf: Vec::new(), pos: 0, partial: None, failed: false }
    }

    /// Appends bytes read from the socket.
    pub fn feed(&mut self, data: &[u8]) {
        if self.pos > 0 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(data);
    }

    /// Unconsumed buffered bytes; lets callers bound read-ahead.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// True while a frame or a fragmented message is incomplete.
    pub fn in_progress(&self) -> bool {
        self.buffered() > 0 || self.partial.is_some()
    }

    /// Decodes the next complete message. After an error the decoder stays
    /// failed: the connection must be closed with [`WsError::close_code`].
    pub fn next_message(&mut self) -> Result<Option<Message>, WsError> {
        if self.failed {
            return Err(WsError::Protocol("decoder already failed"));
        }
        let result = self.decode();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn decode(&mut self) -> Result<Option<Message>, WsError> {
        loop {
            let input = &self.buf[self.pos..];
            if input.len() < 2 {
                return Ok(None);
            }
            let fin = input[0] & 0x80 != 0;
            if input[0] & 0x70 != 0 {
                return Err(WsError::Protocol("reserved bits set without a negotiated extension"));
            }
            let opcode = input[0] & 0x0F;
            if input[1] & 0x80 == 0 {
                return Err(WsError::Protocol("client frames must be masked"));
            }
            let (len, mut header_len) = match input[1] & 0x7F {
                126 => {
                    if input.len() < 4 {
                        return Ok(None);
                    }
                    let len = u16::from_be_bytes([input[2], input[3]]);
                    if len < 126 {
                        return Err(WsError::Protocol("payload length is not minimally encoded"));
                    }
                    (u64::from(len), 4)
                }
                127 => {
                    if input.len() < 10 {
                        return Ok(None);
                    }
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&input[2..10]);
                    let len = u64::from_be_bytes(b);
                    if len >> 63 != 0 {
                        return Err(WsError::Protocol("payload length has the high bit set"));
                    }
                    if len <= u64::from(u16::MAX) {
                        return Err(WsError::Protocol("payload length is not minimally encoded"));
                    }
                    (len, 10)
                }
                n => (u64::from(n), 2),
            };
            let is_control = opcode & 0x8 != 0;
            if is_control && (!fin || len > 125) {
                return Err(WsError::Protocol("invalid control frame"));
            }
            let pending = self.partial.as_ref().map_or(0, |(_, p)| p.len() as u64);
            if !is_control && pending + len > self.config.max_message_size as u64 {
                return Err(WsError::MessageTooBig);
            }
            let len = len as usize;
            if input.len() < header_len + 4 + len {
                return Ok(None);
            }
            let mask = [input[header_len], input[header_len + 1], input[header_len + 2], input[header_len + 3]];
            header_len += 4;
            let start = self.pos + header_len;
            let payload = &mut self.buf[start..start + len];
            unmask(payload, mask);
            self.pos = start + len;
            let payload = &self.buf[start..start + len];

            match opcode {
                OP_TEXT | OP_BINARY => {
                    if self.partial.is_some() {
                        return Err(WsError::Protocol("new data frame while a fragmented message is open"));
                    }
                    if fin {
                        return finish_data(opcode, payload.to_vec()).map(Some);
                    }
                    self.partial = Some((opcode, payload.to_vec()));
                }
                OP_CONTINUATION => {
                    let Some((_, data)) = self.partial.as_mut() else {
                        return Err(WsError::Protocol("continuation frame without a fragmented message"));
                    };
                    data.extend_from_slice(payload);
                    if fin {
                        let (op, data) = self.partial.take().expect("checked above");
                        return finish_data(op, data).map(Some);
                    }
                }
                OP_PING => return Ok(Some(Message::Ping(payload.to_vec()))),
                OP_PONG => return Ok(Some(Message::Pong(payload.to_vec()))),
                OP_CLOSE => return parse_close(payload).map(|c| Some(Message::Close(c))),
                _ => return Err(WsError::Protocol("unknown opcode")),
            }
        }
    }
}

fn finish_data(opcode: u8, data: Vec<u8>) -> Result<Message, WsError> {
    if opcode == OP_TEXT {
        String::from_utf8(data).map(Message::Text).map_err(|_| WsError::InvalidUtf8)
    } else {
        Ok(Message::Binary(data))
    }
}

fn parse_close(payload: &[u8]) -> Result<Option<(u16, String)>, WsError> {
    match payload.len() {
        0 => Ok(None),
        1 => Err(WsError::Protocol("close frame with a one-byte payload")),
        _ => {
            let code = u16::from_be_bytes([payload[0], payload[1]]);
            let valid = matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999);
            if !valid {
                return Err(WsError::Protocol("invalid close status code"));
            }
            let reason = std::str::from_utf8(&payload[2..]).map_err(|_| WsError::InvalidUtf8)?;
            Ok(Some((code, reason.to_owned())))
        }
    }
}

fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    let word = u32::from_ne_bytes(mask);
    let wide = u64::from(word) | (u64::from(word) << 32);
    let mut chunks = payload.chunks_exact_mut(8);
    for chunk in &mut chunks {
        let v = u64::from_ne_bytes(chunk.try_into().expect("8-byte chunk")) ^ wide;
        chunk.copy_from_slice(&v.to_ne_bytes());
    }
    // Chunks of 8 keep the 4-byte mask phase aligned for the tail.
    for (i, b) in chunks.into_remainder().iter_mut().enumerate() {
        *b ^= mask[i & 3];
    }
}

/// Largest server frame header: 2 bytes + 8-byte extended length.
pub const MAX_HEADER: usize = 10;

/// A server frame whose payload is written in place after reserved header room,
/// so a serialized JSON message is never copied to prepend the header.
pub struct OutFrame {
    buf: Vec<u8>,
}

impl OutFrame {
    pub fn with_capacity(payload_capacity: usize) -> Self {
        let mut buf = Vec::with_capacity(MAX_HEADER + payload_capacity);
        buf.resize(MAX_HEADER, 0);
        Self { buf }
    }

    /// A frame reusing `buf`'s allocation (its contents are discarded).
    pub fn with_buffer(mut buf: Vec<u8>) -> Self {
        buf.clear();
        buf.resize(MAX_HEADER, 0);
        Self { buf }
    }

    /// Gives the allocation back for [`OutFrame::with_buffer`].
    pub fn into_buffer(self) -> Vec<u8> {
        self.buf
    }

    /// The payload written so far.
    pub fn payload_bytes(&self) -> &[u8] {
        &self.buf[MAX_HEADER..]
    }

    /// Payload writer; everything appended becomes the frame payload.
    pub fn payload(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    pub fn payload_len(&self) -> usize {
        self.buf.len() - MAX_HEADER
    }

    /// Finishes a single unfragmented text frame and returns its wire bytes.
    pub fn finish_text(self) -> FrameBytes {
        self.finish(OP_TEXT)
    }

    fn finish(mut self, opcode: u8) -> FrameBytes {
        let len = self.payload_len();
        let mut header = [0u8; MAX_HEADER];
        let header_len = write_header(&mut header, opcode, len);
        let start = MAX_HEADER - header_len;
        self.buf[start..MAX_HEADER].copy_from_slice(&header[..header_len]);
        FrameBytes {
            buf: self.buf,
            start,
            #[cfg(feature = "perf-trace")]
            queued_at: crate::perf::now(),
        }
    }
}

/// Encoded frame bytes owned without copying.
pub struct FrameBytes {
    buf: Vec<u8>,
    start: usize,
    /// When the frame was finished, for the outbound queue stage.
    #[cfg(feature = "perf-trace")]
    pub queued_at: crate::perf::Stamp,
}

impl AsRef<[u8]> for FrameBytes {
    fn as_ref(&self) -> &[u8] {
        &self.buf[self.start..]
    }
}

fn write_header(out: &mut [u8; MAX_HEADER], opcode: u8, len: usize) -> usize {
    out[0] = 0x80 | opcode;
    if len < 126 {
        out[1] = len as u8;
        2
    } else if len <= usize::from(u16::MAX) {
        out[1] = 126;
        out[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        4
    } else {
        out[1] = 127;
        out[2..10].copy_from_slice(&(len as u64).to_be_bytes());
        10
    }
}

/// Encodes a control frame (pong/close). Payloads over 125 bytes are truncated
/// as required for control frames.
pub fn control_frame(kind: Control<'_>) -> Vec<u8> {
    let (opcode, payload): (u8, Vec<u8>) = match kind {
        Control::Pong(data) => (OP_PONG, data[..data.len().min(125)].to_vec()),
        Control::Close(code, reason) => {
            let mut p = code.to_be_bytes().to_vec();
            let mut end = reason.len().min(123);
            while !reason.is_char_boundary(end) {
                end -= 1;
            }
            p.extend_from_slice(&reason.as_bytes()[..end]);
            (OP_CLOSE, p)
        }
    };
    let mut out = Vec::with_capacity(2 + payload.len());
    out.push(0x80 | opcode);
    out.push(payload.len() as u8);
    out.extend_from_slice(&payload);
    out
}

pub enum Control<'a> {
    Pong(&'a [u8]),
    Close(u16, &'a str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut out = vec![(if fin { 0x80 } else { 0 }) | opcode];
        if payload.len() < 126 {
            out.push(0x80 | payload.len() as u8);
        } else if payload.len() <= 0xFFFF {
            out.push(0x80 | 126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            out.push(0x80 | 127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    fn decoder() -> Decoder {
        Decoder::new(WsConfig { max_message_size: 1 << 20 })
    }

    #[test]
    fn decodes_masked_text_split_across_reads() {
        let frame = client_frame(true, OP_TEXT, b"Hello");
        let mut d = decoder();
        for byte in &frame[..frame.len() - 1] {
            d.feed(std::slice::from_ref(byte));
            assert_eq!(d.next_message(), Ok(None));
        }
        d.feed(&frame[frame.len() - 1..]);
        assert_eq!(d.next_message(), Ok(Some(Message::Text("Hello".into()))));
        assert_eq!(d.next_message(), Ok(None));
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn decodes_extended_lengths_and_odd_unmask_tails() {
        for len in [0, 1, 7, 8, 9, 125, 126, 127, 65535, 65536, 70001] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut d = decoder();
            d.feed(&client_frame(true, OP_BINARY, &payload));
            assert_eq!(d.next_message(), Ok(Some(Message::Binary(payload))), "len {len}");
        }
    }

    #[test]
    fn reassembles_fragments_with_interleaved_ping() {
        let mut d = decoder();
        d.feed(&client_frame(false, OP_TEXT, b"{\"id\":"));
        d.feed(&client_frame(true, OP_PING, b"p"));
        d.feed(&client_frame(true, OP_CONTINUATION, b"1}"));
        assert_eq!(d.next_message(), Ok(Some(Message::Ping(b"p".to_vec()))));
        assert!(d.in_progress(), "fragmented message still open after the ping");
        assert_eq!(d.next_message(), Ok(Some(Message::Text("{\"id\":1}".into()))));
        assert!(!d.in_progress());
    }

    #[test]
    fn rejects_unmasked_frames() {
        let mut d = decoder();
        d.feed(&[0x81, 0x01, b'a']);
        assert_eq!(d.next_message(), Err(WsError::Protocol("client frames must be masked")));
        assert!(d.next_message().is_err(), "decoder stays failed");
    }

    #[test]
    fn rejects_oversized_message_before_buffering_payload() {
        let mut d = Decoder::new(WsConfig { max_message_size: 10 });
        d.feed(&client_frame(true, OP_TEXT, &[b'a'; 11])[..8]);
        assert_eq!(d.next_message(), Err(WsError::MessageTooBig));
        assert_eq!(WsError::MessageTooBig.close_code(), 1009);
        assert_eq!(WsError::MessageTooBig.close_reason(), "message exceeds --cdp-max-message-size");

        let mut d = Decoder::new(WsConfig { max_message_size: 10 });
        d.feed(&client_frame(false, OP_TEXT, &[b'a'; 6]));
        d.feed(&client_frame(true, OP_CONTINUATION, &[b'a'; 5]));
        assert_eq!(d.next_message(), Err(WsError::MessageTooBig));
    }

    #[test]
    fn rejects_invalid_utf8_and_bad_sequences() {
        let mut d = decoder();
        d.feed(&client_frame(true, OP_TEXT, &[0xff, 0xfe]));
        assert_eq!(d.next_message(), Err(WsError::InvalidUtf8));

        let mut d = decoder();
        d.feed(&client_frame(true, OP_CONTINUATION, b"x"));
        assert!(matches!(d.next_message(), Err(WsError::Protocol(_))));

        let mut d = decoder();
        d.feed(&client_frame(false, OP_PING, b"x"));
        assert!(matches!(d.next_message(), Err(WsError::Protocol(_))));

        let mut d = decoder();
        d.feed(&client_frame(true, 0x3, b""));
        assert!(matches!(d.next_message(), Err(WsError::Protocol(_))));
    }

    #[test]
    fn rejects_non_minimal_lengths() {
        let mut d = decoder();
        d.feed(&[0x81, 0xFE, 0x00, 0x01, 0, 0, 0, 0, b'x']);
        assert!(matches!(d.next_message(), Err(WsError::Protocol(_))));
        let mut d = decoder();
        d.feed(&[0x81, 0xFF, 0, 0, 0, 0, 0, 0, 0x01, 0x00, 0, 0, 0, 0]);
        assert!(matches!(d.next_message(), Err(WsError::Protocol(_))));
        let mut d = decoder();
        d.feed(&[0x89, 0xFE, 0x00, 0x01, 0, 0, 0, 0, b'x']);
        assert!(d.next_message().is_err(), "control frames too");
    }

    #[test]
    fn parses_close_frames() {
        let mut d = decoder();
        let mut payload = 1000u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"bye");
        d.feed(&client_frame(true, OP_CLOSE, &payload));
        assert_eq!(d.next_message(), Ok(Some(Message::Close(Some((1000, "bye".into()))))));

        let mut d = decoder();
        d.feed(&client_frame(true, OP_CLOSE, &1005u16.to_be_bytes()));
        assert!(d.next_message().is_err(), "1005 must not appear on the wire");
    }

    #[test]
    fn out_frame_writes_minimal_header_in_place() {
        for len in [0usize, 125, 126, 65535, 65536] {
            let mut f = OutFrame::with_capacity(len);
            f.payload().resize(MAX_HEADER + len, b'x');
            let bytes = f.finish_text();
            let wire = bytes.as_ref();
            assert_eq!(wire[0], 0x81);
            let (decoded_len, header) = match wire[1] {
                126 => (u16::from_be_bytes([wire[2], wire[3]]) as usize, 4),
                127 => (u64::from_be_bytes(wire[2..10].try_into().unwrap()) as usize, 10),
                n => (n as usize, 2),
            };
            assert_eq!(decoded_len, len);
            assert_eq!(wire.len(), header + len);
        }
    }

    #[test]
    fn control_frames_respect_limits() {
        let long = "é".repeat(100);
        let close = control_frame(Control::Close(1000, &long));
        assert!(close.len() - 2 <= 125);
        assert!(std::str::from_utf8(&close[4..]).is_ok(), "truncated on a char boundary");
        let pong = control_frame(Control::Pong(&[1; 200]));
        assert_eq!(pong[1], 125);
    }
}
