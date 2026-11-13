//! Response-body inflation (gzip, zlib, raw deflate) on zlib-rs.
//!
//! zlib-rs inflates a 942 KB gzip body in 2.9 ms against miniz_oxide's 4.0
//! (25 KB: 0.074 vs 0.088 ms). It is used here directly rather than as
//! flate2's backend: Cargo unifies a flate2 feature across the whole build,
//! which would also switch every flate2 compressor (krilla's PDF streams,
//! `CompressionStream`) and change their output bytes. Inflated output is
//! the same for any correct inflater, and the accept/reject decisions follow
//! flate2's write decoders, which this replaces (checked against them in the
//! tests): a truncated zlib or raw deflate stream ends the body with what it
//! decoded, a truncated gzip member fails its trailer check, and bytes after
//! the end of the stream fail. One difference, toward Chromium (which
//! inflates with zlib): a back-reference to before the start of the output
//! ("invalid distance too far back") is an error, where miniz_oxide
//! inflated it as zeros.

use std::io;

use zlib_rs::{Inflate, InflateFlush, Status};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Format {
    Gzip,
    Zlib,
    Raw,
}

/// One inflate stream; its decoded output accumulates in [`Inflater::out`].
pub(crate) struct Inflater {
    z: Inflate,
    format: Format,
    out: Vec<u8>,
    ended: bool,
}

/// The smallest output space offered to one inflate call.
const MIN_SPARE: usize = 32 * 1024;

impl Inflater {
    pub(crate) fn new(format: Format) -> Self {
        let z = match format {
            // 16 + 15: a gzip wrapper, its header, CRC-32 and length checked.
            Format::Gzip => Inflate::new(true, 31),
            Format::Zlib => Inflate::new(true, 15),
            Format::Raw => Inflate::new(false, 15),
        };
        Inflater { z, format, out: Vec::new(), ended: false }
    }

    /// The decoded output so far, which the caller may empty.
    pub(crate) fn out(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }

    /// Inflates `input` into the output, all of it: nothing stays pending
    /// inside the stream once this returns.
    pub(crate) fn write_all(&mut self, mut input: &[u8]) -> io::Result<()> {
        if input.is_empty() {
            return Ok(());
        }
        loop {
            if self.ended {
                // flate2's decoders accept no byte past the stream's end.
                return Err(io::ErrorKind::WriteZero.into());
            }
            // Room for a typical ratio, so a large chunk inflates in a few calls.
            self.out.reserve(MIN_SPARE.max(input.len().saturating_mul(2)));
            let spare = self.out.spare_capacity_mut();
            let room = spare.len();
            let (in0, out0) = (self.z.total_in(), self.z.total_out());
            let status =
                self.z.decompress_uninit(input, spare, InflateFlush::NoFlush).map_err(|_| corrupt_deflate())?;
            let consumed = (self.z.total_in() - in0) as usize;
            let produced = (self.z.total_out() - out0) as usize;
            // SAFETY: inflate initialized the first `produced` spare bytes.
            unsafe { self.out.set_len(self.out.len() + produced) };
            input = &input[consumed..];
            if status == Status::StreamEnd {
                self.ended = true;
                if input.is_empty() {
                    return Ok(());
                }
                continue;
            }
            if input.is_empty() && produced < room {
                return Ok(());
            }
            if consumed == 0 && produced == 0 {
                // Input and room both available yet no progress: not a stream.
                return Err(corrupt_deflate());
            }
        }
    }

    /// The decoded output, once the body ended.
    pub(crate) fn finish(self) -> io::Result<Vec<u8>> {
        if self.format == Format::Gzip && !self.ended {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "corrupt gzip stream does not have a matching checksum",
            ));
        }
        Ok(self.out)
    }
}

fn corrupt_deflate() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "corrupt deflate stream")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The decoder this one replaces: flate2's write decoder on miniz_oxide.
    fn reference(format: Format, body: &[u8], chunk: usize) -> Option<Vec<u8>> {
        fn run<W: Write>(mut w: W, body: &[u8], chunk: usize) -> Option<W> {
            for part in body.chunks(chunk) {
                w.write_all(part).ok()?;
            }
            Some(w)
        }
        match format {
            Format::Gzip => run(flate2::write::GzDecoder::new(Vec::new()), body, chunk)?.finish().ok(),
            Format::Zlib => run(flate2::write::ZlibDecoder::new(Vec::new()), body, chunk)?.finish().ok(),
            Format::Raw => run(flate2::write::DeflateDecoder::new(Vec::new()), body, chunk)?.finish().ok(),
        }
    }

    fn ours(format: Format, body: &[u8], chunk: usize) -> Option<Vec<u8>> {
        let mut inf = Inflater::new(format);
        let mut out = Vec::new();
        for part in body.chunks(chunk) {
            inf.write_all(part).ok()?;
            // Output leaves as it is produced, as the response decoder takes it.
            out.append(inf.out());
        }
        out.append(&mut inf.finish().ok()?);
        Some(out)
    }

    fn encode(format: Format, data: &[u8], level: u32) -> Vec<u8> {
        let level = flate2::Compression::new(level);
        match format {
            Format::Gzip => {
                let mut e = flate2::write::GzEncoder::new(Vec::new(), level);
                e.write_all(data).unwrap();
                e.finish().unwrap()
            }
            Format::Zlib => {
                let mut e = flate2::write::ZlibEncoder::new(Vec::new(), level);
                e.write_all(data).unwrap();
                e.finish().unwrap()
            }
            Format::Raw => {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), level);
                e.write_all(data).unwrap();
                e.finish().unwrap()
            }
        }
    }

    /// A small deterministic generator (xorshift64*).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn sample(rng: &mut Rng, len: usize) -> Vec<u8> {
        let words: [&[u8]; 6] =
            [b"<div class=\"a\">", b"function(x){return x}", b" ", b"\n", b"lorem ipsum ", b"\xe2\x82\xac"];
        let mut v = Vec::with_capacity(len);
        while v.len() < len {
            if rng.below(4) == 0 {
                v.push(rng.next() as u8);
            } else {
                v.extend_from_slice(words[rng.below(words.len())]);
            }
        }
        v.truncate(len);
        v
    }

    /// Valid streams of every size and chunking decode to the same bytes as
    /// flate2's decoders; so do truncated, corrupted, padded and empty ones
    /// (both accept with the same output, or both reject).
    /// zlib's message for the first error in `body`, if any. Fed a byte at
    /// a time: zlib-rs's fast loop names a bad distance "repeated call with
    /// bad state", its byte-wise loop names it.
    fn zlib_error(format: Format, body: &[u8]) -> Option<&'static str> {
        let mut inf = Inflater::new(format);
        for byte in body.chunks(1) {
            if inf.write_all(byte).is_err() {
                return inf.z.error_message();
            }
        }
        None
    }

    #[test]
    fn matches_flate2_on_valid_and_damaged_streams() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut far_distance = 0;
        for format in [Format::Gzip, Format::Zlib, Format::Raw] {
            for len in [0, 1, 37, 4096, 70_000, 300_000] {
                for level in [0, 1, 6, 9] {
                    let data = sample(&mut rng, len);
                    let body = encode(format, &data, level);
                    let mut variants = vec![body.clone()];
                    // Truncated at the end, mid-trailer and mid-stream.
                    for cut in [1, 4, 8, body.len() / 2] {
                        if cut < body.len() {
                            variants.push(body[..body.len() - cut].to_vec());
                        }
                    }
                    // Trailing bytes after the stream.
                    variants.push([&body[..], b"x"].concat());
                    variants.push([&body[..], &[0u8; 9][..]].concat());
                    // A second gzip member.
                    variants.push([&body[..], &body[..]].concat());
                    // Flipped bits in the header, the data and the trailer.
                    for at in [0, 1, 3, body.len() / 3, body.len() / 2, body.len().saturating_sub(2)] {
                        if at < body.len() {
                            let mut v = body.clone();
                            v[at] ^= 1 << rng.below(8);
                            variants.push(v);
                        }
                    }
                    variants.push(Vec::new());
                    variants.push(sample(&mut rng, 64));
                    let chunks: &[usize] = if body.len() > 8192 { &[1000, 1 << 20] } else { &[1, 7, 1000, 1 << 20] };
                    for (i, v) in variants.iter().enumerate() {
                        for &chunk in chunks {
                            let want = reference(format, v, chunk);
                            let got = ours(format, v, chunk);
                            if got.is_none() && want.is_some() {
                                // miniz_oxide inflates a reference to before
                                // the output's start as zeros; zlib, like
                                // Chromium, rejects the stream.
                                assert_eq!(
                                    zlib_error(format, v),
                                    Some("invalid distance too far back"),
                                    "{format:?} len {len} level {level} variant {i} chunk {chunk}"
                                );
                                far_distance += 1;
                                continue;
                            }
                            assert_eq!(got, want, "{format:?} len {len} level {level} variant {i} chunk {chunk}");
                            if i == 0 {
                                assert_eq!(want.as_deref(), Some(&data[..]));
                            }
                        }
                    }
                }
            }
        }
        assert!(far_distance > 0, "the corruptions cover the one known difference");
    }
}
