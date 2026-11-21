//! The video track of an MP4 (ISO BMFF, `moov` sample tables) or a
//! WebM/Matroska file (`SimpleBlock`s and `BlockGroup`s in clusters): codec,
//! size and every sample's place, presentation time and key flag. Only what
//! is in `data` is listed: a cut body yields the samples it holds.

/// A video codec found in a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    Av1,
    /// H.264 with length-prefixed NAL units (`avcC`): the prefix size and
    /// the SPS/PPS, already in Annex B form.
    H264 {
        length_size: usize,
        parameter_sets: Vec<u8>,
    },
    /// A codec without a decoder here (VP8, VP9, HEVC, ...): its four-cc
    /// or Matroska codec ID.
    Other(String),
}

/// One coded frame, in decode order.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub offset: usize,
    pub size: usize,
    /// Presentation time in seconds.
    pub pts: f64,
    pub key: bool,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub samples: Vec<Sample>,
}

/// The first video track of `data`, if the container is one we read.
pub fn video_track(data: &[u8]) -> Option<Track> {
    if data.len() >= 8 && is_mp4(data) {
        return mp4(data);
    }
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return matroska(data);
    }
    None
}

fn is_mp4(data: &[u8]) -> bool {
    matches!(&data[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"styp")
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

// ---- ISO BMFF -----------------------------------------------------------------

/// Child boxes of `data`: (type, payload, payload offset within `data`).
struct Boxes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Boxes<'a> {
    type Item = (&'a [u8], &'a [u8], usize);
    fn next(&mut self) -> Option<Self::Item> {
        let d = self.data;
        let size = be32(d, self.at)? as u64;
        let kind = d.get(self.at + 4..self.at + 8)?;
        let (header, size) = match size {
            1 => (16, be64(d, self.at + 8)?),
            0 => (8, (d.len() - self.at) as u64),
            n => (8, n),
        };
        let end = self.at.checked_add(usize::try_from(size).ok()?)?;
        if size < header as u64 {
            return None;
        }
        // A box cut by the end of a partial body is read as far as it goes.
        let end = end.min(d.len());
        let start = self.at + header;
        if start > end {
            return None;
        }
        let item = (kind, &d[start..end], start);
        self.at = end;
        Some(item)
    }
}

fn boxes(data: &[u8]) -> Boxes<'_> {
    Boxes { data, at: 0 }
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _, _)| *k == kind).map(|(_, p, _)| p)
}

fn mp4(data: &[u8]) -> Option<Track> {
    let moov = child(data, b"moov")?;
    let movie_scale = child(moov, b"mvhd").and_then(|m| match m.first()? {
        1 => be32(m, 20),
        _ => be32(m, 12),
    });
    for (kind, trak, _) in boxes(moov) {
        if kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else { continue };
        if child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide") {
            continue;
        }
        if let Some(track) = mp4_track(data, trak, mdia, movie_scale) {
            return Some(track);
        }
    }
    None
}

fn mp4_track(file: &[u8], trak: &[u8], mdia: &[u8], movie_scale: Option<u32>) -> Option<Track> {
    let mdhd = child(mdia, b"mdhd")?;
    let timescale = match mdhd.first()? {
        1 => be32(mdhd, 20)?,
        _ => be32(mdhd, 12)?,
    };
    if timescale == 0 {
        return None;
    }
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let stsd = child(stbl, b"stsd")?;
    // The first sample entry: [size, type, 6 reserved, data ref index,
    // 16 pre-defined/reserved, width, height, ...], children from byte 78.
    let (entry_kind, entry, _) = boxes(stsd.get(8..)?).next()?;
    let width = u32::from(be16(entry, 24)?);
    let height = u32::from(be16(entry, 26)?);
    let children = entry.get(78..).unwrap_or(&[]);
    let codec = match entry_kind {
        b"av01" => Codec::Av1,
        b"avc1" | b"avc3" => match child(children, b"avcC").and_then(avc_config) {
            Some((length_size, parameter_sets)) => Codec::H264 { length_size, parameter_sets },
            None => Codec::Other(String::from_utf8_lossy(entry_kind).into_owned()),
        },
        other => Codec::Other(String::from_utf8_lossy(other).into_owned()),
    };

    // Decode times (stts), composition offsets (ctts), sizes (stsz/stz2),
    // chunk layout (stsc, stco/co64) and sync samples (stss).
    let stts = child(stbl, b"stts")?;
    let mut dts = Vec::new();
    let mut t: i64 = 0;
    for i in 0..be32(stts, 4)? as usize {
        let (count, delta) = (be32(stts, 8 + i * 8)?, be32(stts, 12 + i * 8)?);
        for _ in 0..count {
            if dts.len() > MAX_SAMPLES {
                return None;
            }
            dts.push(t);
            t += i64::from(delta);
        }
    }
    let count = dts.len();
    let mut cts = vec![0i64; count];
    if let Some(ctts) = child(stbl, b"ctts") {
        // Offsets are signed in either version: files with a version 0 box and negative offsets
        // are common (WPT `media/white.mp4`: -200, -100), and FFmpeg (Chromium's demuxer) reads
        // them as signed 32-bit values too. Read unsigned, a negative offset ordered its frame
        // past the end, and no frame after the first decoded.
        let mut at = 0usize;
        for i in 0..be32(ctts, 4)? as usize {
            let (n, off) = (be32(ctts, 8 + i * 8)?, be32(ctts, 12 + i * 8)?);
            let off = i64::from(off as i32);
            for _ in 0..n {
                if let Some(c) = cts.get_mut(at) {
                    *c = off;
                }
                at += 1;
            }
        }
    }
    let stsz = child(stbl, b"stsz")?;
    let fixed = be32(stsz, 4)?;
    let n = be32(stsz, 8)? as usize;
    let sizes: Vec<u32> = if fixed != 0 {
        vec![fixed; n.min(count)]
    } else {
        (0..n.min(count)).map_while(|i| be32(stsz, 12 + i * 4)).collect()
    };
    let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        (0..be32(stco, 4)? as usize).map_while(|i| be32(stco, 8 + i * 4).map(u64::from)).collect()
    } else {
        let co64 = child(stbl, b"co64")?;
        (0..be32(co64, 4)? as usize).map_while(|i| be64(co64, 8 + i * 8)).collect()
    };
    let stsc = child(stbl, b"stsc")?;
    let runs: Vec<(u32, u32)> =
        (0..be32(stsc, 4)? as usize).map_while(|i| Some((be32(stsc, 8 + i * 12)?, be32(stsc, 12 + i * 12)?))).collect();
    let sync: Option<Vec<u32>> =
        child(stbl, b"stss").and_then(|s| (0..be32(s, 4)? as usize).map(|i| be32(s, 8 + i * 4)).collect());

    // The edit list's first edit shifts presentation: an empty edit delays
    // it, a media time skips into the media (the B-frame delay of H.264).
    let mut shift: i64 = 0;
    if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
        let v1 = elst.first() == Some(&1);
        let entry = |i: usize| -> Option<(u64, i64)> {
            if v1 {
                Some((be64(elst, 8 + i * 20)?, be64(elst, 16 + i * 20)? as i64))
            } else {
                Some((u64::from(be32(elst, 8 + i * 12)?), i64::from(be32(elst, 12 + i * 12)? as i32)))
            }
        };
        let n = be32(elst, 4).unwrap_or(0) as usize;
        let mut i = 0;
        let mut delay: i64 = 0;
        while i < n {
            let Some((duration, media_time)) = entry(i) else { break };
            if media_time == -1 {
                // Empty edit: in the movie's timescale.
                let scale = i64::from(movie_scale.unwrap_or(timescale).max(1));
                delay += duration as i64 * i64::from(timescale) / scale;
                i += 1;
                continue;
            }
            shift = media_time - delay;
            break;
        }
        if i == n {
            shift = -delay;
        }
    }

    let mut samples = Vec::with_capacity(count);
    let mut index = 0usize;
    for (c, &chunk_offset) in chunks.iter().enumerate() {
        let chunk_no = c as u32 + 1;
        let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk_no).map_or(0, |r| r.1);
        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            let (Some(&size), Some(&d)) = (sizes.get(index), dts.get(index)) else { break };
            let start = usize::try_from(offset).ok()?;
            let end = start.checked_add(size as usize)?;
            // Only samples held by the (possibly cut) body.
            if end <= file.len() {
                let key = sync.as_ref().is_none_or(|s| s.binary_search(&(index as u32 + 1)).is_ok());
                let pts = (d + cts[index] - shift) as f64 / f64::from(timescale);
                samples.push(Sample { offset: start, size: size as usize, pts, key });
            }
            offset += u64::from(size);
            index += 1;
        }
    }
    Some(Track { codec, width, height, samples })
}

/// Most samples a track may list.
const MAX_SAMPLES: usize = 1 << 22;

/// `avcC`: the NAL length size and the SPS/PPS in Annex B form.
pub(super) fn avc_config(avcc: &[u8]) -> Option<(usize, Vec<u8>)> {
    let length_size = usize::from(avcc.get(4)? & 3) + 1;
    let mut sets = Vec::new();
    let mut at = 5;
    for kind in 0..2 {
        let n = if kind == 0 { usize::from(avcc.get(at)? & 0x1f) } else { usize::from(*avcc.get(at)?) };
        at += 1;
        for _ in 0..n {
            let len = usize::from(be16(avcc, at)?);
            sets.extend_from_slice(&[0, 0, 0, 1]);
            sets.extend_from_slice(avcc.get(at + 2..at + 2 + len)?);
            at += 2 + len;
        }
    }
    Some((length_size, sets))
}

// ---- Matroska / WebM ---------------------------------------------------------------

/// An EBML variable-size integer at `at`: (value, length). `marker` keeps
/// the length marker bit (element IDs).
fn vint(b: &[u8], at: usize, marker: bool) -> Option<(u64, usize)> {
    let first = *b.get(at)?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut v = if marker { u64::from(first) } else { u64::from(first) & ((1u64 << (8 - len)) - 1) };
    for i in 1..len {
        v = (v << 8) | u64::from(*b.get(at + i)?);
    }
    Some((v, len))
}

const UNKNOWN: u64 = u64::MAX;

/// One element: (id, payload start, payload end); an unknown size runs to
/// `limit`.
fn element(b: &[u8], at: usize, limit: usize) -> Option<(u64, usize, usize)> {
    let (id, id_len) = vint(b, at, true)?;
    let (size, size_len) = vint(b, at + id_len, false)?;
    let start = at + id_len + size_len;
    let all_ones = size == (1u64 << (7 * size_len)) - 1;
    let end = if all_ones { UNKNOWN } else { (start as u64).saturating_add(size) };
    let end = if end == UNKNOWN { limit } else { usize::try_from(end).unwrap_or(usize::MAX).min(limit) };
    (start <= end).then_some((id, start, end))
}

/// Children of [start, end): (id, payload start, payload end).
fn children(b: &[u8], start: usize, end: usize) -> impl Iterator<Item = (u64, usize, usize)> + '_ {
    let mut at = start;
    std::iter::from_fn(move || {
        if at >= end {
            return None;
        }
        let e = element(b, at, end)?;
        at = e.2;
        Some(e)
    })
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |v, &x| (v << 8) | u64::from(x))
}

const SEGMENT: u64 = 0x1853_8067;
const INFO: u64 = 0x1549_A966;
const TIMESTAMP_SCALE: u64 = 0x2A_D7B1;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_NUMBER: u64 = 0xD7;
const TRACK_TYPE: u64 = 0x83;
const CODEC_ID: u64 = 0x86;
const CODEC_PRIVATE: u64 = 0x63A2;
const VIDEO: u64 = 0xE0;
const PIXEL_WIDTH: u64 = 0xB0;
const PIXEL_HEIGHT: u64 = 0xBA;
const CLUSTER: u64 = 0x1F43_B675;
const CLUSTER_TIMESTAMP: u64 = 0xE7;
const SIMPLE_BLOCK: u64 = 0xA3;
const BLOCK_GROUP: u64 = 0xA0;
const BLOCK: u64 = 0xA1;
const REFERENCE_BLOCK: u64 = 0xFB;
/// Level-1 elements: an unknown-size cluster ends at the next one.
const TOP_LEVEL: [u64; 7] = [CLUSTER, 0x1C53_BB6B, 0x1254_C367, 0x1043_A770, 0x1941_A469, INFO, TRACKS];

fn matroska(b: &[u8]) -> Option<Track> {
    let (_, _, header_end) = element(b, 0, b.len())?;
    let (id, seg_start, seg_end) = element(b, header_end, b.len())?;
    if id != SEGMENT {
        return None;
    }
    let mut scale: u64 = 1_000_000;
    let mut track: Option<(u64, Codec, u32, u32)> = None;
    let mut samples = Vec::new();
    let mut at = seg_start;
    while at < seg_end {
        let Some((id, start, end)) = element(b, at, seg_end) else { break };
        match id {
            INFO => {
                for (id, s, e) in children(b, start, end) {
                    if id == TIMESTAMP_SCALE {
                        scale = uint(&b[s..e]).max(1);
                    }
                }
            }
            TRACKS => track = track.or_else(|| matroska_track(b, start, end)),
            CLUSTER => {
                let Some((number, ..)) = &track else { return None };
                let end = cluster(b, start, end, *number, scale, &mut samples);
                at = end;
                continue;
            }
            _ => {}
        }
        at = end;
    }
    let (_, codec, width, height) = track?;
    Some(Track { codec, width, height, samples })
}

fn matroska_track(b: &[u8], start: usize, end: usize) -> Option<(u64, Codec, u32, u32)> {
    for (id, s, e) in children(b, start, end) {
        if id != TRACK_ENTRY {
            continue;
        }
        let (mut number, mut kind, mut codec, mut private, mut w, mut h) = (0, 0, String::new(), &[][..], 0, 0);
        for (id, s, e) in children(b, s, e) {
            match id {
                TRACK_NUMBER => number = uint(&b[s..e]),
                TRACK_TYPE => kind = uint(&b[s..e]),
                CODEC_ID => codec = String::from_utf8_lossy(&b[s..e]).trim_end_matches('\0').to_owned(),
                CODEC_PRIVATE => private = &b[s..e],
                VIDEO => {
                    for (id, s, e) in children(b, s, e) {
                        match id {
                            PIXEL_WIDTH => w = uint(&b[s..e]) as u32,
                            PIXEL_HEIGHT => h = uint(&b[s..e]) as u32,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if kind != 1 {
            continue;
        }
        let codec = match codec.as_str() {
            "V_AV1" => Codec::Av1,
            "V_MPEG4/ISO/AVC" => match avc_config(private) {
                Some((length_size, parameter_sets)) => Codec::H264 { length_size, parameter_sets },
                None => Codec::Other(codec),
            },
            _ => Codec::Other(codec),
        };
        return Some((number, codec, w, h));
    }
    None
}

/// Reads a cluster's blocks of track `number`; returns where it ends (an
/// unknown-size cluster ends at the next level-1 element).
fn cluster(b: &[u8], start: usize, end: usize, number: u64, scale: u64, out: &mut Vec<Sample>) -> usize {
    let mut base: i64 = 0;
    let mut at = start;
    while at < end {
        let Some((id, s, e)) = element(b, at, end) else { return end };
        if TOP_LEVEL.contains(&id) {
            return at;
        }
        match id {
            CLUSTER_TIMESTAMP => base = uint(&b[s..e]) as i64,
            SIMPLE_BLOCK => {
                if let Some((track, rel, flags, data)) = block(b, s, e) {
                    if track == number {
                        out.push(sample(base + rel, scale, flags & 0x80 != 0, data));
                    }
                }
            }
            BLOCK_GROUP => {
                let mut found = None;
                let mut referenced = false;
                for (id, s, e) in children(b, s, e) {
                    match id {
                        BLOCK => found = block(b, s, e),
                        REFERENCE_BLOCK => referenced = true,
                        _ => {}
                    }
                }
                if let Some((track, rel, _, data)) = found {
                    if track == number {
                        out.push(sample(base + rel, scale, !referenced, data));
                    }
                }
            }
            _ => {}
        }
        if out.len() > MAX_SAMPLES {
            return end;
        }
        at = e;
    }
    end
}

fn sample(timestamp: i64, scale: u64, key: bool, (offset, size): (usize, usize)) -> Sample {
    Sample { offset, size, pts: timestamp as f64 * scale as f64 / 1e9, key }
}

/// A (Simple)Block: track number, relative timestamp, flags and the frame
/// (unlaced blocks only: video is not laced).
fn block(b: &[u8], start: usize, end: usize) -> Option<(u64, i64, u8, (usize, usize))> {
    let (track, len) = vint(b, start, false)?;
    let rel = i64::from(be16(b, start + len)? as i16);
    let flags = *b.get(start + len + 2)?;
    if flags & 0x06 != 0 {
        return None;
    }
    let data = start + len + 3;
    (data <= end).then_some((track, rel, flags, (data, end - data)))
}
