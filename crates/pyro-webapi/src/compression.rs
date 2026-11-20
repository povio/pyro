//! The codec behind `CompressionStream` / `DecompressionStream` (streams.js):
//! "deflate" (zlib, RFC 1950), "deflate-raw" (RFC 1951) and "gzip" (RFC 1952)
//! over flate2's raw (de)compressor. The gzip header and trailer are written
//! and checked here, so every backend works the same. Errors are TypeErrors,
//! as the Compression Standard asks: corrupt data, data after the end of the
//! compressed stream, or input that ends before it. Brotli decompression uses
//! the same RFC 7932 implementation as the HTTP content decoder.

use std::cell::RefCell;
use std::rc::Rc;

use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use flate2::{Compress, Compression, Crc, Decompress, FlushCompress, FlushDecompress, Status};
use pyro_js::{JsError, JsResult, Object, Scope};

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
    JsError::Type(format!("Failed to decompress: {message}").into())
}

// Successful decompression with unconsumed input differs from an invalid
// stream: publish the successfully decoded bytes before reporting the suffix.
enum ChunkStatus {
    Accepted,
    Trailing,
}

impl ChunkStatus {
    fn check(self) -> JsResult<()> {
        match self {
            Self::Accepted => Ok(()),
            Self::Trailing => Err(corrupt("junk found after the end of the compressed data")),
        }
    }
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
                .map_err(|e| JsError::Type(format!("Failed to compress: {e}").into()))?;
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

    fn push_into(&mut self, mut input: &[u8], out: &mut Vec<u8>) -> JsResult<ChunkStatus> {
        while !input.is_empty() {
            match &mut self.phase {
                // A header within one chunk is read in place, without copying the chunk.
                Phase::Header(seen) if seen.is_empty() => match gzip_header_length(input)? {
                    Some(length) => {
                        input = &input[length..];
                        self.phase = Phase::Body;
                    }
                    None => {
                        seen.extend_from_slice(input);
                        input = &[];
                    }
                },
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
                Phase::Body => match self.inflate(input, out)? {
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
                Phase::Done => return Ok(ChunkStatus::Trailing),
            }
        }
        Ok(ChunkStatus::Accepted)
    }

    pub(crate) fn finish(&mut self) -> JsResult<Vec<u8>> {
        match self.phase {
            Phase::Done => Ok(Vec::new()),
            _ => Err(corrupt("unexpected end of the compressed data")),
        }
    }
}

struct BrotliDecoder {
    // The history window is released as soon as the end marker is decoded.
    state: Option<Box<BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>>>,
    total_out: usize,
}

impl BrotliDecoder {
    fn new() -> Self {
        // RFC 7932 permits window bits up to 24. The library's ordinary
        // constructor also accepts an experimental large-window extension.
        Self {
            state: Some(Box::new(BrotliState::new_strict(
                StandardAlloc::default(),
                StandardAlloc::default(),
                StandardAlloc::default(),
            ))),
            total_out: 0,
        }
    }

    fn push_into(&mut self, input: &[u8], out: &mut Vec<u8>) -> JsResult<ChunkStatus> {
        // Keep decoder offsets local to each slice (the native bit reader
        // uses 32-bit input indices). The decoder retains split bit headers.
        for input in input.chunks(STEP) {
            let Some(state) = self.state.as_deref_mut() else { return Ok(ChunkStatus::Trailing) };
            let mut remaining = input.len();
            let mut offset = 0;
            loop {
                let mut buffer = [0; STEP];
                let mut capacity = buffer.len();
                let mut written = 0;
                let status = BrotliDecompressStream(
                    &mut remaining,
                    &mut offset,
                    input,
                    &mut capacity,
                    &mut written,
                    &mut buffer,
                    &mut self.total_out,
                    state,
                );
                out.extend_from_slice(&buffer[..written]);
                match status {
                    BrotliResult::ResultSuccess => {
                        self.state = None;
                        if remaining != 0 {
                            return Ok(ChunkStatus::Trailing);
                        }
                        break;
                    }
                    // NeedsMoreInput may also fill this output buffer. Drain
                    // bytes already decoded before waiting for another write.
                    BrotliResult::NeedsMoreInput if capacity == 0 => continue,
                    BrotliResult::NeedsMoreInput => break,
                    BrotliResult::NeedsMoreOutput => continue,
                    BrotliResult::ResultFailure => return Err(corrupt("invalid Brotli stream")),
                }
            }
        }
        Ok(ChunkStatus::Accepted)
    }
}

enum Codec {
    Brotli(BrotliDecoder),
    Compress(Compressor),
    Decompress(Decompressor),
}

impl Codec {
    fn push_into(&mut self, input: &[u8], out: &mut Vec<u8>) -> JsResult<ChunkStatus> {
        match self {
            Codec::Compress(c) => {
                *out = c.push(input)?;
                Ok(ChunkStatus::Accepted)
            }
            Codec::Decompress(d) => d.push_into(input, out),
            Codec::Brotli(d) => d.push_into(input, out),
        }
    }
    fn finish(&mut self) -> JsResult<Vec<u8>> {
        match self {
            Codec::Compress(c) => c.finish(),
            Codec::Decompress(d) => d.finish(),
            Codec::Brotli(d) if d.state.is_none() => Ok(Vec::new()),
            Codec::Brotli(_) => Err(corrupt("unexpected end of the compressed data")),
        }
    }
}

/// Native codec for streams.js. push(chunk, enqueue) emits a Uint8Array
/// when it has bytes, and finish() supplies final output or an error.

pub(crate) fn zlib_function<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    s.function("zlib", |s, _this, a| {
        let name = a.get(0).to_string()?;
        let compress = a.get(1).to_bool();
        let codec = if name == "brotli" && !compress {
            Codec::Brotli(BrotliDecoder::new())
        } else {
            let format = Format::parse(&name).ok_or_else(|| JsError::Type("Unsupported compression format".into()))?;
            if compress {
                Codec::Compress(Compressor::new(format))
            } else {
                Codec::Decompress(Decompressor::new(format))
            }
        };
        let codec = Rc::new(RefCell::new(codec));
        let handle = s.object();
        let pushing = codec.clone();
        let push = s.function("push", move |s, _this, a| {
            let chunk = crate::encoding::buffer_source_bytes(s, a.get(0))?.ok_or_else(|| {
                JsError::Type("The provided chunk is not of type '(ArrayBuffer or ArrayBufferView)'".into())
            })?;
            let mut out = Vec::new();
            let status = pushing.borrow_mut().push_into(&chunk, &mut out)?;
            // A pending read can receive decoded bytes before an error from
            // trailing input in this same write.
            if !out.is_empty() {
                let enqueue = a.get(1).as_object().ok_or(JsError::ContextDestroyed)?;
                enqueue.call(None, &[crate::encoding::uint8_array(s, &out)?])?;
            }
            status.check()?;
            Ok(s.undefined())
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

    impl Decompressor {
        fn push(&mut self, input: &[u8]) -> JsResult<Vec<u8>> {
            let mut output = Vec::new();
            self.push_into(input, &mut output)?.check()?;
            Ok(output)
        }
    }

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
    fn brotli_stream_preserves_chunk_boundaries_and_checks_termination() {
        // A single uncompressed Brotli meta-block containing "foo".
        let encoded = [0x21, 0x08, 0, 4, b'f', b'o', b'o', 3];
        for split in 1..=encoded.len() {
            let mut codec = Codec::Brotli(BrotliDecoder::new());
            let mut output = Vec::new();
            for part in encoded.chunks(split) {
                codec.push_into(part, &mut output).unwrap().check().unwrap();
            }
            assert_eq!(output, b"foo");
            codec.push_into(&[], &mut output).unwrap().check().unwrap();
            assert!(codec.finish().unwrap().is_empty());
            assert!(codec.push_into(&[0], &mut output).unwrap().check().is_err());
        }
        let mut decoder = Codec::Brotli(BrotliDecoder::new());
        let mut output = Vec::new();
        decoder.push_into(&encoded[..encoded.len() - 1], &mut output).unwrap().check().unwrap();
        assert!(decoder.finish().is_err());
        let mut padded = encoded.to_vec();
        padded.push(0);
        let mut decoder = Codec::Brotli(BrotliDecoder::new());
        output.clear();
        assert!(decoder.push_into(&padded, &mut output).unwrap().check().is_err());
        assert_eq!(output, b"foo");
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
