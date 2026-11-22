//! Response-body `zstd` decoding (RFC 8878 frames, the RFC 9659 content
//! coding) on ruzstd, a pure-Rust decoder.
//!
//! Network chunks are pushed in; ruzstd decodes from complete pieces only, so
//! the input waits here until a whole frame header or block (at most 128 KiB
//! plus its 3-byte header) has arrived, then decodes it. As Chromium's
//! `ZstdSourceStream` (libzstd): a body may hold several frames and skippable
//! frames; a window above 8 MiB (RFC 9659 § 3, Chromium's `windowLogMax` 23)
//! fails, as does a frame that needs a dictionary, a checksum mismatch, bytes
//! that are not a frame, and a body that ends inside a frame. An empty body
//! decodes to nothing.

use std::io;

use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

/// The largest window a frame may ask for: 8 MiB.
const MAX_WINDOW: u64 = 8 << 20;
/// A zstd frame's magic number.
const MAGIC: u32 = 0xFD2F_B528;
/// The magic numbers of skippable frames.
const SKIPPABLE: std::ops::RangeInclusive<u32> = 0x184D_2A50..=0x184D_2A5F;
/// The largest block content (RFC 8878 § 3.1.1.2.3: `Block_Maximum_Size`).
const MAX_BLOCK: usize = 128 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Before a frame: the body's start, or after a frame ended.
    Header,
    /// Inside a frame's blocks; whether its last block carries a checksum.
    Blocks { checksum: bool },
    /// Inside a skippable frame: the bytes still to skip.
    Skip(u64),
}

/// One `zstd` coded body; its decoded output accumulates in [`Zstd::out`].
pub(crate) struct Zstd {
    dec: FrameDecoder,
    state: State,
    /// Received input not decoded yet: less than one header or block.
    pending: Vec<u8>,
    out: Vec<u8>,
}

impl Zstd {
    pub(crate) fn new() -> Self {
        let mut dec = FrameDecoder::new();
        dec.set_max_window_size(MAX_WINDOW);
        Zstd { dec, state: State::Header, pending: Vec::new(), out: Vec::new() }
    }

    /// The decoded output so far, which the caller may empty.
    pub(crate) fn out(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }

    /// Takes `input`, decoding every frame header and block it completes.
    pub(crate) fn write_all(&mut self, input: &[u8]) -> io::Result<()> {
        if input.is_empty() {
            return Ok(());
        }
        self.pending.extend_from_slice(input);
        let mut pos = 0;
        loop {
            let rest = &self.pending[pos..];
            match self.state {
                State::Skip(left) => {
                    let take = (rest.len() as u64).min(left);
                    pos += take as usize;
                    if take < left {
                        self.state = State::Skip(left - take);
                        break;
                    }
                    self.state = State::Header;
                }
                State::Header => {
                    if rest.len() < 4 {
                        break;
                    }
                    let magic = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                    if SKIPPABLE.contains(&magic) {
                        if rest.len() < 8 {
                            break;
                        }
                        pos += 8;
                        self.state = State::Skip(u64::from(u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]])));
                        continue;
                    }
                    if magic != MAGIC {
                        return Err(corrupt("not a zstd frame"));
                    }
                    let Some(&descriptor) = rest.get(4) else { break };
                    let len = header_len(descriptor);
                    if rest.len() < len {
                        break;
                    }
                    let mut src = &rest[..len];
                    self.dec.reset(&mut src).map_err(decode_error)?;
                    pos += len;
                    self.state = State::Blocks { checksum: descriptor & 0x04 != 0 };
                }
                State::Blocks { checksum } => {
                    if rest.len() < 3 {
                        break;
                    }
                    let block = u32::from(rest[0]) | u32::from(rest[1]) << 8 | u32::from(rest[2]) << 16;
                    let last = block & 1 == 1;
                    let size = (block >> 3) as usize;
                    let content = match (block >> 1) & 3 {
                        // RLE: one byte repeated `size` times.
                        1 => 1,
                        // Reserved.
                        3 => return Err(corrupt("reserved zstd block type")),
                        _ if size > MAX_BLOCK => return Err(corrupt("zstd block too large")),
                        _ => size,
                    };
                    let len = 3 + content + if last && checksum { 4 } else { 0 };
                    if rest.len() < len {
                        break;
                    }
                    let mut src = &rest[..len];
                    self.dec.decode_blocks(&mut src, BlockDecodingStrategy::UptoBlocks(1)).map_err(decode_error)?;
                    if !src.is_empty() {
                        return Err(corrupt("zstd block shorter than its header says"));
                    }
                    pos += len;
                    // Until the frame ends, the window's bytes stay behind
                    // for later back-references; then all of it drains.
                    self.dec.collect_to_writer(&mut self.out)?;
                    if last {
                        if checksum && self.dec.get_checksum_from_data() != self.dec.get_calculated_checksum() {
                            return Err(corrupt("zstd checksum mismatch"));
                        }
                        self.state = State::Header;
                    }
                }
            }
        }
        self.pending.drain(..pos);
        Ok(())
    }

    /// The decoded output, once the body ended: an error if it ended inside
    /// a frame.
    pub(crate) fn finish(self) -> io::Result<Vec<u8>> {
        if self.state != State::Header || !self.pending.is_empty() {
            return Err(corrupt("truncated zstd stream"));
        }
        Ok(self.out)
    }
}

/// A frame header's length from its descriptor byte (RFC 8878 § 3.1.1.1):
/// magic, descriptor, window descriptor unless single-segment, dictionary
/// id and frame content size.
fn header_len(descriptor: u8) -> usize {
    let single_segment = descriptor & 0x20 != 0;
    let fcs = match descriptor >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let dict_id = [0, 1, 2, 4][usize::from(descriptor & 3)];
    4 + 1 + usize::from(!single_segment) + dict_id + fcs
}

fn decode_error(e: ruzstd::decoding::errors::FrameDecoderError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("corrupt zstd stream: {e}"))
}

fn corrupt(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WPT's `fetch/content-encoding/zstd/resources/foo.text.zst`: "foo"
    /// 3,500 times, one compressed block, single segment, with a checksum.
    const FOO: &[u8] = &[
        0x28, 0xb5, 0x2f, 0xfd, 0x64, 0x04, 0x28, 0x5d, 0x00, 0x00, 0x18, 0x66, 0x6f, 0x6f, 0x01, 0x00, 0xfe, 0x48,
        0xde, 0x0d, 0x01, 0xc3, 0xbe, 0x9f, 0x9f,
    ];

    fn decode(body: &[u8], chunk: usize) -> io::Result<Vec<u8>> {
        let mut z = Zstd::new();
        let mut out = Vec::new();
        for part in body.chunks(chunk.max(1)) {
            z.write_all(part)?;
            out.append(z.out());
        }
        out.append(&mut z.finish()?);
        Ok(out)
    }

    /// A frame of raw blocks with an optional checksum, built by hand: the
    /// decoder's block and checksum handling in every chunking.
    fn raw_frame(data: &[u8], block: usize, checksum: bool) -> Vec<u8> {
        // Window descriptor 0x00: a 1 KiB window (blocks up to 1 KiB).
        let mut f = MAGIC.to_le_bytes().to_vec();
        f.push(if checksum { 0x04 } else { 0x00 });
        f.push(0x00);
        let mut pieces = data.chunks(block).peekable();
        if data.is_empty() {
            f.extend_from_slice(&[1, 0, 0]); // one empty last raw block
        }
        while let Some(piece) = pieces.next() {
            let h = (piece.len() as u32) << 3 | u32::from(pieces.peek().is_none());
            f.extend_from_slice(&h.to_le_bytes()[..3]);
            f.extend_from_slice(piece);
        }
        if checksum {
            f.extend_from_slice(&(twox_hash::XxHash64::oneshot(0, data) as u32).to_le_bytes());
        }
        f
    }

    #[test]
    fn decodes_in_any_chunking() {
        for chunk in [1, 2, 3, 5, 21, 4096] {
            assert_eq!(decode(FOO, chunk).unwrap(), b"foo".repeat(3500), "chunk {chunk}");
        }
        let text: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        for checksum in [false, true] {
            let frame = raw_frame(&text, 1000, checksum);
            for chunk in [1, 2, 3, 7, 1003, 100_000] {
                assert_eq!(decode(&frame, chunk).unwrap(), text, "chunk {chunk} checksum {checksum}");
            }
        }
        assert_eq!(decode(&raw_frame(b"", 1000, true), 1).unwrap(), b"");
    }

    /// Several frames and skippable frames make one body.
    #[test]
    fn concatenated_and_skippable_frames() {
        let mut body = vec![0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 9, 9, 9];
        body.extend_from_slice(&raw_frame(b"hello ", 1000, true));
        body.extend_from_slice(&[0x5f, 0x2a, 0x4d, 0x18, 0, 0, 0, 0]);
        body.extend_from_slice(&raw_frame(b"world", 2, false));
        for chunk in [1, 4, 9, 1000] {
            assert_eq!(decode(&body, chunk).unwrap(), b"hello world");
        }
    }

    #[test]
    fn bad_bodies_fail() {
        assert!(decode(b"not actually zstd", 4).is_err());
        assert!(decode(&FOO[..FOO.len() - 1], 4).is_err(), "truncated");
        assert!(decode(&FOO[..3], 4).is_err(), "partial magic");
        let mut trailing = FOO.to_vec();
        trailing.push(0);
        assert!(decode(&trailing, 4).is_err(), "bytes after the frame");
        let mut sum = raw_frame(b"checked", 1000, true);
        *sum.last_mut().unwrap() ^= 1;
        assert!(decode(&sum, 3).is_err(), "checksum");
        // A 16 MiB window (the WPT big.window.zst's): over the 8 MiB limit.
        let big = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x70, 0x01, 0x00, 0x00];
        assert!(decode(&big, 9).is_err());
        // 8 MiB is allowed.
        let ok = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x68, 0x01, 0x00, 0x00];
        assert_eq!(decode(&ok, 9).unwrap(), b"");
        assert_eq!(decode(b"", 1).unwrap(), b"");
    }
}
