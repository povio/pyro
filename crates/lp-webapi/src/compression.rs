//! The codec behind `CompressionStream` / `DecompressionStream` (streams.js):
//! "deflate" (zlib, RFC 1950), "deflate-raw" (RFC 1951) and "gzip" (RFC 1952)
//! over flate2's raw (de)compressor. The gzip header and trailer are written
//! and checked here, so every backend works the same. Errors are TypeErrors,
//! as the Compression Standard asks: corrupt data, data after the end of the
//! compressed stream, or input that ends before it.

use std::cell::RefCell;
use std::rc::Rc;

use flate2::{Compress, Compression, Crc, Decompress, FlushCompress, FlushDecompress, Status};
use lp_js::{JsError, JsResult, Object, Scope};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Deflate,
    DeflateRaw,
    Gzip,
}

impl Format {
    fn parse(name: &str) -> Option<Format> {
        Some(match name {
            "deflate" => Format::Deflate,
            "deflate-raw" => Format::DeflateRaw,
            "gzip" => Format::Gzip,
            _ => return None,
        })
    }
}

fn corrupt(message: &str) -> JsError {
    JsError::Type(format!("Failed to decompress: {message}"))
}

/// Output grows in steps of this size while a call has more to write.
const STEP: usize = 16 * 1024;

pub(crate) struct Compressor {
    format: Format,
    inner: Compress,
    crc: Crc,
    header_written: bool,
}

impl Compressor {
    fn new(format: Format) -> Compressor {
        Compressor {
            format,
            inner: Compress::new(Compression::default(), format == Format::Deflate),
            crc: Crc::new(),
            header_written: false,
        }
    }

    fn run(&mut self, mut input: &[u8], flush: FlushCompress, out: &mut Vec<u8>) -> JsResult<()> {
        loop {
            if out.capacity() - out.len() < STEP / 2 {
                out.reserve(STEP);
            }
            let before_in = self.inner.total_in();
            let status = self
                .inner
                .compress_vec(input, out, flush)
                .map_err(|e| JsError::Type(format!("Failed to compress: {e}")))?;
            let consumed = (self.inner.total_in() - before_in) as usize;
            input = &input[consumed..];
            match status {
                Status::StreamEnd => return Ok(()),
                // Everything given is in and the compressor had no more to write.
                _ if flush == FlushCompress::None && input.is_empty() && out.len() < out.capacity() => return Ok(()),
                _ => {}
            }
        }
    }

    fn header(&mut self, out: &mut Vec<u8>) {
        if self.format == Format::Gzip && !self.header_written {
            // No name, no timestamp, deflate, OS unknown.
            out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xff]);
        }
        self.header_written = true;
    }

    pub(crate) fn push(&mut self, input: &[u8]) -> JsResult<Vec<u8>> {
        let mut out = Vec::new();
        self.header(&mut out);
        self.crc.update(input);
        self.run(input, FlushCompress::None, &mut out)?;
        Ok(out)
    }

    pub(crate) fn finish(&mut self) -> JsResult<Vec<u8>> {
        let mut out = Vec::new();
        self.header(&mut out);
        self.run(&[], FlushCompress::Finish, &mut out)?;
        if self.format == Format::Gzip {
            out.extend_from_slice(&self.crc.sum().to_le_bytes());
            out.extend_from_slice(&self.crc.amount().to_le_bytes());
        }
        Ok(out)
    }
}

enum Phase {
    /// The gzip header bytes seen so far.
    Header(Vec<u8>),
    Body,
    /// The gzip trailer bytes seen so far.
    Trailer(Vec<u8>),
    Done,
}

pub(crate) struct Decompressor {
    format: Format,
    inner: Decompress,
    crc: Crc,
    phase: Phase,
}

/// The length of a complete gzip header at the start of `bytes`, None while
/// more bytes are needed.
fn gzip_header_length(bytes: &[u8]) -> JsResult<Option<usize>> {
    if bytes.len() >= 2 && (bytes[0] != 0x1f || bytes[1] != 0x8b) {
        return Err(corrupt("incorrect gzip header"));
    }
    if bytes.len() >= 3 && bytes[2] != 8 {
        return Err(corrupt("unknown gzip compression method"));
    }
    if bytes.len() < 10 {
        return Ok(None);
    }
    let flags = bytes[3];
    if flags & 0xe0 != 0 {
        return Err(corrupt("reserved gzip flags are set"));
    }
    let mut at = 10;
    if flags & 0x04 != 0 {
        if bytes.len() < at + 2 {
            return Ok(None);
        }
        at += 2 + u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    }
    for flag in [0x08, 0x10] {
        if flags & flag != 0 {
            match bytes.get(at..).and_then(|rest| rest.iter().position(|&b| b == 0)) {
                Some(end) => at += end + 1,
                None => return Ok(None),
            }
        }
    }
    if flags & 0x02 != 0 {
        at += 2;
    }
    Ok((bytes.len() >= at).then_some(at))
}

impl Decompressor {
    fn new(format: Format) -> Decompressor {
        Decompressor {
            format,
            inner: Decompress::new(format == Format::Deflate),
            crc: Crc::new(),
            phase: if format == Format::Gzip { Phase::Header(Vec::new()) } else { Phase::Body },
        }
    }

    /// Inflates from `input` until it is used up or the stream ends; returns
    /// the bytes after the end.
    fn inflate<'a>(&mut self, mut input: &'a [u8], out: &mut Vec<u8>) -> JsResult<Option<&'a [u8]>> {
        loop {
            if out.capacity() - out.len() < STEP / 2 {
                out.reserve(STEP);
            }
            let start = out.len();
            let before_in = self.inner.total_in();
            let status =
                self.inner.decompress_vec(input, out, FlushDecompress::None).map_err(|e| corrupt(&e.to_string()))?;
            let consumed = (self.inner.total_in() - before_in) as usize;
            input = &input[consumed..];
            if self.format == Format::Gzip {
                self.crc.update(&out[start..]);
            }
            match status {
                Status::StreamEnd => return Ok(Some(input)),
                _ if input.is_empty() && out.len() < out.capacity() => return Ok(None),
                _ if consumed == 0 && out.len() == start => return Err(corrupt("the decompressor made no progress")),
                _ => {}
            }
        }
    }

    pub(crate) fn push(&mut self, mut input: &[u8]) -> JsResult<Vec<u8>> {
        let mut out = Vec::new();
        while !input.is_empty() {
            match &mut self.phase {
                Phase::Header(seen) => {
                    let had = seen.len();
                    seen.extend_from_slice(input);
                    match gzip_header_length(seen)? {
                        Some(length) => {
                            input = &input[length - had..];
                            self.phase = Phase::Body;
                        }
                        None => input = &[],
                    }
                }
                Phase::Body => match self.inflate(input, &mut out)? {
                    Some(rest) => {
                        input = rest;
                        self.phase = if self.format == Format::Gzip { Phase::Trailer(Vec::new()) } else { Phase::Done };
                    }
                    None => input = &[],
                },
                Phase::Trailer(seen) => {
                    let take = (8 - seen.len()).min(input.len());
                    seen.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if seen.len() == 8 {
                        let crc = u32::from_le_bytes(seen[..4].try_into().expect("4 bytes"));
                        let size = u32::from_le_bytes(seen[4..].try_into().expect("4 bytes"));
                        if crc != self.crc.sum() || size != self.crc.amount() {
                            return Err(corrupt("incorrect gzip checksum"));
                        }
                        self.phase = Phase::Done;
                    }
                }
                Phase::Done => return Err(corrupt("junk found after the end of the compressed data")),
            }
        }
        Ok(out)
    }

    pub(crate) fn finish(&mut self) -> JsResult<Vec<u8>> {
        match self.phase {
            Phase::Done => Ok(Vec::new()),
            _ => Err(corrupt("unexpected end of the compressed data")),
        }
    }
}

enum Codec {
    Compress(Compressor),
    Decompress(Decompressor),
}

impl Codec {
    fn push(&mut self, input: &[u8]) -> JsResult<Vec<u8>> {
        match self {
            Codec::Compress(c) => c.push(input),
            Codec::Decompress(d) => d.push(input),
        }
    }
    fn finish(&mut self) -> JsResult<Vec<u8>> {
        match self {
            Codec::Compress(c) => c.finish(),
            Codec::Decompress(d) => d.finish(),
        }
    }
}

/// `zlib(format, compress)` for streams.js: `{push(chunk), finish()}`, each
/// returning a Uint8Array (possibly empty). `format` was validated by the
/// caller; a chunk that is not a BufferSource is a TypeError.
pub(crate) fn zlib_function<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    s.function("zlib", |s, _this, a| {
        let format = Format::parse(&a.get(0).to_string()?)
            .ok_or_else(|| JsError::Type("Unsupported compression format".into()))?;
        let codec = Rc::new(RefCell::new(if a.get(1).to_bool() {
            Codec::Compress(Compressor::new(format))
        } else {
            Codec::Decompress(Decompressor::new(format))
        }));
        let handle = s.object();
        let pushing = codec.clone();
        let push = s.function("push", move |s, _this, a| {
            let chunk = crate::encoding::buffer_source_bytes(s, a.get(0))?.ok_or_else(|| {
                JsError::Type("The provided chunk is not of type '(ArrayBuffer or ArrayBufferView)'".into())
            })?;
            let out = pushing.borrow_mut().push(&chunk)?;
            crate::encoding::uint8_array(s, &out)
        })?;
        handle.set_atom("push", push.value())?;
        let finish = s.function("finish", move |s, _this, _a| {
            let out = codec.borrow_mut().finish()?;
            crate::encoding::uint8_array(s, &out)
        })?;
        handle.set_atom("finish", finish.value())?;
        Ok(handle.value())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(format: Format, data: &[u8], split: usize) -> Vec<u8> {
        let mut c = Compressor::new(format);
        let mut compressed = Vec::new();
        for chunk in data.chunks(split.max(1)) {
            compressed.extend(c.push(chunk).unwrap());
        }
        compressed.extend(c.finish().unwrap());
        let mut d = Decompressor::new(format);
        let mut out = Vec::new();
        for chunk in compressed.chunks(split.max(1)) {
            out.extend(d.push(chunk).unwrap());
        }
        out.extend(d.finish().unwrap());
        out
    }

    #[test]
    fn every_format_round_trips_in_any_chunking() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8 ^ (i / 1000) as u8).collect();
        for format in [Format::Deflate, Format::DeflateRaw, Format::Gzip] {
            for split in [1, 7, 4096, data.len()] {
                assert_eq!(round_trip(format, &data, split), data);
            }
            assert_eq!(round_trip(format, &[], 1), Vec::<u8>::new());
        }
    }

    #[test]
    fn gzip_output_reads_with_flate2_and_flate2_gzip_reads_here() {
        use std::io::{Read, Write};
        let mut c = Compressor::new(Format::Gzip);
        let mut gz = c.push(b"hello gzip").unwrap();
        gz.extend(c.finish().unwrap());
        let mut text = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello gzip");
        // A header with a file name and a comment, from flate2's encoder.
        let mut e = flate2::GzBuilder::new().filename("a.txt").comment("c").write(Vec::new(), Compression::fast());
        e.write_all(b"named").unwrap();
        let named = e.finish().unwrap();
        let mut d = Decompressor::new(Format::Gzip);
        let mut out = Vec::new();
        for b in &named {
            out.extend(d.push(std::slice::from_ref(b)).unwrap());
        }
        d.finish().unwrap();
        assert_eq!(out, b"named");
    }

    #[test]
    fn corrupt_truncated_and_trailing_input_fail() {
        let mut c = Compressor::new(Format::Deflate);
        let mut z = c.push(b"some data some data").unwrap();
        z.extend(c.finish().unwrap());
        let mut d = Decompressor::new(Format::Deflate);
        d.push(&z[..z.len() - 2]).unwrap();
        assert!(d.finish().is_err());
        let mut d = Decompressor::new(Format::Deflate);
        let mut extra = z.clone();
        extra.push(0);
        assert!(d.push(&extra).is_err());
        let mut d = Decompressor::new(Format::Gzip);
        assert!(d.push(&[0x1f, 0x8c]).is_err());
        let mut d = Decompressor::new(Format::Deflate);
        let mut bad = z.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(d.push(&bad).is_err());
    }
}
