//! What a media element learns from a resource without decoding it: the
//! container's metadata (duration, video size, which tracks it has) read
//! from headers in pure Rust, and `canPlayType()` answers as Chrome gives
//! them. Containers: MP4/QuickTime (ISO BMFF), WebM/Matroska, Ogg, WAV,
//! MP3, FLAC and ADTS AAC. Every parser is bounds-checked and iterates a
//! bounded number of times per byte of input: hostile input yields `Err`,
//! never a panic.

/// The metadata of a playable resource.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MediaInfo {
    /// Seconds; `f64::INFINITY` when the container does not say (a live
    /// or unfinished stream).
    pub duration: f64,
    /// The video track's presentation size (rotation applied), if any.
    pub video: Option<(u32, u32)>,
    pub audio: bool,
}

/// Why a resource cannot be played, as Chrome words it in
/// `MediaError.message`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeError {
    /// Not a container Chrome's demuxers recognize.
    Unrecognized,
    /// A recognized container without one track Chrome can decode.
    NoSupportedStreams,
}

impl ProbeError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            ProbeError::Unrecognized => "DEMUXER_ERROR_COULD_NOT_OPEN: FFmpegDemuxer: open context failed",
            ProbeError::NoSupportedStreams => "DEMUXER_ERROR_NO_SUPPORTED_STREAMS: FFmpegDemuxer: no supported streams",
        }
    }
}

type Probe = Result<MediaInfo, ProbeError>;

/// Chrome's limits on a video frame (`media::limits`): a larger video
/// track cannot be decoded.
const MAX_DIMENSION: u32 = (1 << 15) - 1;
const MAX_AREA: u64 = 1 << 28;

/// Reads `body`'s container metadata, sniffing the format from its bytes
/// (like Chrome, the response's MIME type does not matter).
pub(crate) fn probe(body: &[u8]) -> Probe {
    let mut info = sniff(body)?;
    if let Some((w, h)) = info.video {
        if w > MAX_DIMENSION || h > MAX_DIMENSION || u64::from(w) * u64::from(h) > MAX_AREA {
            info.video = None;
            if !info.audio {
                return Err(ProbeError::NoSupportedStreams);
            }
        }
    }
    Ok(info)
}

fn sniff(body: &[u8]) -> Probe {
    if body.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return matroska(body);
    }
    if body.starts_with(b"OggS") {
        return ogg(body);
    }
    if body.len() >= 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WAVE" {
        return wav(body);
    }
    if body.len() >= 8 && is_iso_bmff_box(&body[4..8]) {
        return mp4(body);
    }
    // MP3, FLAC and ADTS may start with an ID3v2 tag.
    let start = id3_len(body).min(body.len());
    let rest = &body[start..];
    if rest.starts_with(b"fLaC") {
        return flac(rest);
    }
    if let Some(info) = adts(rest) {
        return Ok(info);
    }
    mp3(body, start)
}

// ---------------------------------------------------------------------------
// Byte readers

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}
fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}
fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn finite_or_infinite(seconds: f64) -> f64 {
    if seconds.is_finite() && seconds > 0.0 {
        seconds
    } else {
        f64::INFINITY
    }
}

// ---------------------------------------------------------------------------
// ISO BMFF (MP4, QuickTime)

fn is_iso_bmff_box(kind: &[u8]) -> bool {
    matches!(kind, b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" | b"styp")
}

/// The boxes of `data`, in order: (type, payload).
struct Boxes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Boxes<'a> {
    type Item = ([u8; 4], &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        let size32 = be32(self.data, self.at)?;
        let kind: [u8; 4] = self.data.get(self.at + 4..self.at + 8)?.try_into().ok()?;
        let (header, size) = match size32 {
            // To the end of the enclosing data.
            0 => (8, (self.data.len() - self.at) as u64),
            1 => (16, be64(self.data, self.at + 8)?),
            n => (8, u64::from(n)),
        };
        if size < header as u64 {
            return None;
        }
        // A truncated last box yields what is there.
        let end = self.at.saturating_add(usize::try_from(size).unwrap_or(usize::MAX)).min(self.data.len());
        let payload = self.data.get(self.at + header..end)?;
        self.at = end;
        Some((kind, payload))
    }
}

fn boxes(data: &[u8]) -> Boxes<'_> {
    Boxes { data, at: 0 }
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, p)| p)
}

/// (timescale, duration) of an `mvhd` or `mdhd` payload.
fn header_duration(payload: &[u8]) -> Option<(u32, u64)> {
    match payload.first()? {
        0 => {
            let d = be32(payload, 16)?;
            Some((be32(payload, 12)?, if d == u32::MAX { 0 } else { u64::from(d) }))
        }
        // version/flags, creation (8), modification (8), timescale, duration (8).
        1 => {
            let d = be64(payload, 24)?;
            Some((be32(payload, 20)?, if d == u64::MAX { 0 } else { d }))
        }
        _ => None,
    }
}

const MP4_VIDEO: &[&[u8; 4]] = &[b"avc1", b"avc3", b"hvc1", b"hev1", b"vp08", b"vp09", b"av01"];
const MP4_AUDIO: &[&[u8; 4]] = &[b"mp4a", b"Opus", b"fLaC", b".mp3", b"lpcm", b"sowt", b"twos", b"ulaw", b"alaw"];

fn mp4(body: &[u8]) -> Probe {
    let moov = child(body, b"moov").ok_or(ProbeError::Unrecognized)?;
    let (timescale, mut duration) = child(moov, b"mvhd").and_then(header_duration).ok_or(ProbeError::Unrecognized)?;
    let mut seconds = if timescale > 0 { duration as f64 / f64::from(timescale) } else { 0.0 };
    if duration == 0 {
        // A fragmented file: the fragment duration, else the longest track.
        if let Some(mehd) = child(moov, b"mvex").and_then(|m| child(m, b"mehd")) {
            duration = match mehd.first() {
                Some(1) => be64(mehd, 4).unwrap_or(0),
                _ => be32(mehd, 4).map_or(0, u64::from),
            };
            if timescale > 0 {
                seconds = duration as f64 / f64::from(timescale);
            }
        }
    }
    let mut video = None;
    let mut audio = false;
    let mut track_seconds: f64 = 0.0;
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else { continue };
        let Some(handler) = child(mdia, b"hdlr").and_then(|h| h.get(8..12)) else { continue };
        if let Some((scale, d)) = child(mdia, b"mdhd").and_then(header_duration) {
            if scale > 0 {
                track_seconds = track_seconds.max(d as f64 / f64::from(scale));
            }
        }
        let Some(entry) = child(mdia, b"minf")
            .and_then(|m| child(m, b"stbl"))
            .and_then(|s| child(s, b"stsd"))
            .and_then(|s| s.get(8..))
        else {
            continue;
        };
        let Some(format) = entry.get(4..8) else { continue };
        match handler {
            b"vide" if video.is_none() && MP4_VIDEO.iter().any(|f| f.as_slice() == format) => {
                // Presentation size from tkhd (16.16 fixed point), else the
                // sample entry's coded size.
                let tkhd = child(trak, b"tkhd");
                let at = match tkhd.and_then(|t| t.first()) {
                    Some(1) => 4 + 32 + 16,
                    _ => 4 + 20 + 16,
                };
                let matrix = tkhd.and_then(|t| t.get(at..at + 36));
                let mut size = tkhd
                    .and_then(|t| Some((be32(t, at + 36)? >> 16, be32(t, at + 40)? >> 16)))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .or_else(|| Some((u32::from(be16(entry, 32)?), u32::from(be16(entry, 34)?))))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .unwrap_or((0, 0));
                // A quarter-turn rotation swaps the displayed axes.
                if let Some(m) = matrix {
                    let a = be32(m, 0).unwrap_or(0) as i32;
                    let b = be32(m, 4).unwrap_or(0) as i32;
                    if a == 0 && b.unsigned_abs() == 1 << 16 {
                        size = (size.1, size.0);
                    }
                }
                video = Some(size);
            }
            b"soun" if MP4_AUDIO.iter().any(|f| f.as_slice() == format) => audio = true,
            _ => {}
        }
    }
    if video.is_none() && !audio {
        return Err(ProbeError::NoSupportedStreams);
    }
    if seconds <= 0.0 {
        seconds = track_seconds;
    }
    Ok(MediaInfo { duration: finite_or_infinite(seconds), video, audio })
}

// ---------------------------------------------------------------------------
// Matroska / WebM (EBML)

/// An EBML variable-length integer at `at`: (value, length). `marker`
/// keeps the length marker (element IDs keep it, sizes do not).
fn vint(b: &[u8], at: usize, marker: bool) -> Option<(u64, usize)> {
    let first = *b.get(at)?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    let mut value = u64::from(if marker { first } else { first & (0xFFu16 >> len) as u8 });
    for i in 1..len {
        value = (value << 8) | u64::from(*b.get(at + i)?);
    }
    Some((value, len))
}

/// The elements of `data`: (id, payload). An unknown size (all ones)
/// extends to the end of `data`.
struct Elements<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = (u64, &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        let (id, id_len) = vint(self.data, self.at, true)?;
        let (size, size_len) = vint(self.data, self.at + id_len, false)?;
        let start = self.at + id_len + size_len;
        let unknown = size == (1u64 << (7 * size_len)) - 1;
        let end = if unknown {
            self.data.len()
        } else {
            start.saturating_add(usize::try_from(size).unwrap_or(usize::MAX)).min(self.data.len())
        };
        let payload = self.data.get(start..end)?;
        self.at = end;
        Some((id, payload))
    }
}

fn elements(data: &[u8]) -> Elements<'_> {
    Elements { data, at: 0 }
}

fn ebml_uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |v, &x| (v << 8) | u64::from(x))
}

fn ebml_float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f64::from(f32::from_be_bytes(b.try_into().ok()?))),
        8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

const EBML: u64 = 0x1A45_DFA3;
const DOC_TYPE: u64 = 0x4282;
const SEGMENT: u64 = 0x1853_8067;
const INFO: u64 = 0x1549_A966;
const TIMESTAMP_SCALE: u64 = 0x2A_D7B1;
const DURATION: u64 = 0x4489;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_TYPE: u64 = 0x83;
const CODEC_ID: u64 = 0x86;
const VIDEO: u64 = 0xE0;
const PIXEL_WIDTH: u64 = 0xB0;
const PIXEL_HEIGHT: u64 = 0xBA;
const DISPLAY_WIDTH: u64 = 0x54B0;
const DISPLAY_HEIGHT: u64 = 0x54BA;
const DISPLAY_UNIT: u64 = 0x54B2;
const CLUSTER: u64 = 0x1F43_B675;

fn matroska(body: &[u8]) -> Probe {
    let mut top = elements(body);
    let (id, header) = top.next().ok_or(ProbeError::Unrecognized)?;
    if id != EBML {
        return Err(ProbeError::Unrecognized);
    }
    let doc_type = elements(header).find(|(id, _)| *id == DOC_TYPE).map(|(_, v)| v).unwrap_or(b"matroska");
    let webm = match doc_type {
        b"webm" => true,
        b"matroska" => false,
        _ => return Err(ProbeError::Unrecognized),
    };
    let segment = top.find(|(id, _)| *id == SEGMENT).map(|(_, p)| p).ok_or(ProbeError::Unrecognized)?;
    let mut scale = 1_000_000u64;
    let mut duration = None;
    let mut tracks = None;
    for (id, payload) in elements(segment) {
        match id {
            INFO => {
                for (id, v) in elements(payload) {
                    match id {
                        TIMESTAMP_SCALE => scale = ebml_uint(v).max(1),
                        DURATION => duration = ebml_float(v),
                        _ => {}
                    }
                }
            }
            TRACKS => tracks = Some(payload),
            // Media data follows the headers.
            CLUSTER => break,
            _ => {}
        }
        if duration.is_some() && tracks.is_some() {
            break;
        }
    }
    let tracks = tracks.ok_or(ProbeError::Unrecognized)?;
    let mut video = None;
    let mut audio = false;
    for (id, entry) in elements(tracks) {
        if id != TRACK_ENTRY {
            continue;
        }
        let mut kind = 0;
        let mut codec: &[u8] = b"";
        let mut size = None;
        for (id, v) in elements(entry) {
            match id {
                TRACK_TYPE => kind = ebml_uint(v),
                CODEC_ID => codec = v,
                VIDEO => {
                    let (mut pw, mut ph, mut dw, mut dh, mut unit) = (0, 0, 0, 0, 0);
                    for (id, v) in elements(v) {
                        match id {
                            PIXEL_WIDTH => pw = ebml_uint(v),
                            PIXEL_HEIGHT => ph = ebml_uint(v),
                            DISPLAY_WIDTH => dw = ebml_uint(v),
                            DISPLAY_HEIGHT => dh = ebml_uint(v),
                            DISPLAY_UNIT => unit = ebml_uint(v),
                            _ => {}
                        }
                    }
                    let (w, h) = if unit == 0 && dw > 0 && dh > 0 { (dw, dh) } else { (pw, ph) };
                    size = Some((w.min(u64::from(u32::MAX)) as u32, h.min(u64::from(u32::MAX)) as u32));
                }
                _ => {}
            }
        }
        let codec = std::str::from_utf8(codec).unwrap_or("").trim_end_matches('\0');
        match kind {
            1 if video.is_none() && matroska_video(codec, webm) => video = Some(size.unwrap_or((0, 0))),
            2 if matroska_audio(codec, webm) => audio = true,
            _ => {}
        }
    }
    if video.is_none() && !audio {
        return Err(ProbeError::NoSupportedStreams);
    }
    let seconds = duration.map_or(f64::INFINITY, |d| d * scale as f64 / 1e9);
    Ok(MediaInfo { duration: finite_or_infinite(seconds), video, audio })
}

fn matroska_video(codec: &str, webm: bool) -> bool {
    matches!(codec, "V_VP8" | "V_VP9" | "V_AV1") || (!webm && matches!(codec, "V_MPEG4/ISO/AVC" | "V_MPEGH/ISO/HEVC"))
}

fn matroska_audio(codec: &str, webm: bool) -> bool {
    matches!(codec, "A_VORBIS" | "A_OPUS")
        || (!webm
            && (codec.starts_with("A_AAC")
                || matches!(codec, "A_FLAC" | "A_MPEG/L3" | "A_PCM/INT/LIT" | "A_PCM/FLOAT/IEEE")))
}

// ---------------------------------------------------------------------------
// Ogg

struct OggStream {
    /// Granule positions per second.
    rate: f64,
    /// Granules before the first sample (Opus pre-skip).
    skip: u64,
    last_granule: Option<u64>,
}

fn ogg(body: &[u8]) -> Probe {
    // Keyed by serial: every page looks its stream up, and a hostile body
    // can begin any number of streams (one per ~40-byte page), so a linear
    // search would be quadratic.
    let mut streams: std::collections::HashMap<u32, OggStream> = std::collections::HashMap::new();
    let mut unsupported = 0usize;
    let mut at = 0;
    while at + 27 <= body.len() {
        if &body[at..at + 4] != b"OggS" {
            // Lost sync: look for the next page.
            match body[at + 1..].windows(4).position(|w| w == b"OggS") {
                Some(i) => {
                    at += 1 + i;
                    continue;
                }
                None => break,
            }
        }
        let flags = body[at + 5];
        let granule = le64(body, at + 6).ok_or(ProbeError::Unrecognized)?;
        let serial = le32(body, at + 14).ok_or(ProbeError::Unrecognized)?;
        let segments = usize::from(body[at + 26]);
        let lacing = body.get(at + 27..at + 27 + segments).ok_or(ProbeError::Unrecognized)?;
        let data_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let data_at = at + 27 + segments;
        let data = &body[data_at..(data_at + data_len).min(body.len())];
        if flags & 0x02 != 0 {
            // Beginning of a stream: its first packet names the codec.
            match ogg_codec(data) {
                Some((rate, skip)) if rate > 0.0 => {
                    streams.entry(serial).or_insert(OggStream { rate, skip, last_granule: None });
                }
                _ => unsupported += 1,
            }
        } else if streams.is_empty() && unsupported == 0 {
            return Err(ProbeError::Unrecognized);
        }
        // -1: no packet ends on this page.
        if granule != u64::MAX {
            if let Some(s) = streams.get_mut(&serial) {
                s.last_granule = Some(granule);
            }
        }
        at = data_at + data_len;
    }
    if streams.is_empty() {
        return Err(if unsupported > 0 { ProbeError::NoSupportedStreams } else { ProbeError::Unrecognized });
    }
    let seconds = streams
        .values()
        .filter_map(|s| s.last_granule.map(|g| g.saturating_sub(s.skip) as f64 / s.rate))
        .fold(0.0, f64::max);
    Ok(MediaInfo { duration: finite_or_infinite(seconds), video: None, audio: true })
}

/// (granule rate, pre-skip) of an Ogg stream's first packet, for the codecs
/// Chrome plays from Ogg: Vorbis, Opus and FLAC (Theora is gone).
fn ogg_codec(packet: &[u8]) -> Option<(f64, u64)> {
    if packet.starts_with(b"\x01vorbis") {
        return Some((f64::from(le32(packet, 12)?), 0));
    }
    if packet.starts_with(b"OpusHead") {
        return Some((48_000.0, u64::from(le16(packet, 10)?)));
    }
    if packet.starts_with(b"\x7FFLAC") {
        // Mapping header (9 bytes), "fLaC", then the STREAMINFO block.
        let info = packet.get(13..)?;
        return Some((f64::from(flac_streaminfo(info)?.0), 0));
    }
    None
}

// ---------------------------------------------------------------------------
// WAV

fn wav(body: &[u8]) -> Probe {
    let mut at = 12;
    let mut format = None;
    while at + 8 <= body.len() {
        let id = &body[at..at + 4];
        let size = le32(body, at + 4).ok_or(ProbeError::Unrecognized)? as usize;
        let start = at + 8;
        match id {
            b"fmt " => {
                let tag = le16(body, start).ok_or(ProbeError::Unrecognized)?;
                let byte_rate = le32(body, start + 8).ok_or(ProbeError::Unrecognized)?;
                let bits = le16(body, start + 14).ok_or(ProbeError::Unrecognized)?;
                // WAVE_FORMAT_EXTENSIBLE names the real format in its GUID.
                let tag = if tag == 0xFFFE { le16(body, start + 24).ok_or(ProbeError::Unrecognized)? } else { tag };
                let supported = match tag {
                    1 => matches!(bits, 8 | 16 | 24 | 32),
                    3 => bits == 32,
                    6 | 7 => bits == 8,
                    _ => false,
                };
                if !supported {
                    return Err(ProbeError::NoSupportedStreams);
                }
                format = Some(byte_rate);
            }
            b"data" => {
                let byte_rate = format.ok_or(ProbeError::Unrecognized)?;
                if byte_rate == 0 {
                    return Err(ProbeError::Unrecognized);
                }
                // A streamed WAV leaves the size 0 or -1; a truncated one
                // has less than it says.
                let available = body.len() - start;
                let len = if size == 0 || size == u32::MAX as usize { available } else { size.min(available) };
                let seconds = len as f64 / f64::from(byte_rate);
                return Ok(MediaInfo { duration: finite_or_infinite(seconds), video: None, audio: true });
            }
            _ => {}
        }
        at = start.saturating_add(size).saturating_add(size & 1);
    }
    Err(ProbeError::Unrecognized)
}

// ---------------------------------------------------------------------------
// FLAC

/// (sample rate, total samples) of a STREAMINFO block with its 4-byte
/// metadata block header.
fn flac_streaminfo(block: &[u8]) -> Option<(u32, u64)> {
    if block.first()? & 0x7F != 0 {
        return None;
    }
    let info = block.get(4..4 + 34)?;
    let rate = (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
    let total = (u64::from(info[13] & 0x0F) << 32) | u64::from(be32(info, 14)?);
    Some((rate, total))
}

fn flac(body: &[u8]) -> Probe {
    let (rate, total) = flac_streaminfo(&body[4..]).ok_or(ProbeError::Unrecognized)?;
    if rate == 0 {
        return Err(ProbeError::Unrecognized);
    }
    Ok(MediaInfo { duration: finite_or_infinite(total as f64 / f64::from(rate)), video: None, audio: true })
}

// ---------------------------------------------------------------------------
// MP3, ADTS

/// The length of an ID3v2 tag at the start of `body` (0 without one).
fn id3_len(body: &[u8]) -> usize {
    if body.len() < 10 || &body[..3] != b"ID3" {
        return 0;
    }
    let size = body[6..10].iter().fold(0usize, |v, &b| (v << 7) | usize::from(b & 0x7F));
    let footer = if body[5] & 0x10 != 0 { 10 } else { 0 };
    10 + size + footer
}

const ADTS_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

fn adts(body: &[u8]) -> Option<MediaInfo> {
    let header = |at: usize| -> Option<(u32, usize)> {
        let b = body.get(at..at + 7)?;
        if b[0] != 0xFF || b[1] & 0xF6 != 0xF0 {
            return None;
        }
        let rate = *ADTS_RATES.get(usize::from((b[2] >> 2) & 0x0F))?;
        let len = (usize::from(b[3] & 0x03) << 11) | (usize::from(b[4]) << 3) | usize::from(b[5] >> 5);
        (len >= 7).then_some((rate, len))
    };
    let (rate, first) = header(0)?;
    // Two frames in a row, unless the file holds one.
    if first < body.len() && header(first).is_none() {
        return None;
    }
    let mut frames = 0u64;
    let mut at = 0;
    while let Some((_, len)) = header(at) {
        frames += 1;
        at += len;
    }
    Some(MediaInfo { duration: finite_or_infinite(frames as f64 * 1024.0 / f64::from(rate)), video: None, audio: true })
}

#[derive(Clone, Copy)]
struct Mp3Frame {
    /// 1 (MPEG-1), 2 (MPEG-2) or 25 (MPEG-2.5).
    version: u8,
    layer: u8,
    bitrate: u32,
    rate: u32,
    len: usize,
    samples: u32,
    mono: bool,
}

fn mp3_frame(b: &[u8], at: usize) -> Option<Mp3Frame> {
    let h = be32(b, at)?;
    if h >> 21 != 0x7FF {
        return None;
    }
    let version = match (h >> 19) & 3 {
        0 => 25,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let layer = match (h >> 17) & 3 {
        1 => 3,
        2 => 2,
        3 => 1,
        _ => return None,
    };
    let bitrate_index = ((h >> 12) & 0xF) as usize;
    let rate_index = ((h >> 10) & 3) as usize;
    if bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
        return None;
    }
    const V1: [[u32; 15]; 3] = [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ];
    const V2: [[u32; 15]; 2] = [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ];
    let kbps = if version == 1 {
        V1[usize::from(layer) - 1][bitrate_index]
    } else {
        V2[usize::from(layer != 1)][bitrate_index]
    };
    let base = [44100, 48000, 32000][rate_index];
    let rate = match version {
        1 => base,
        2 => base / 2,
        _ => base / 4,
    };
    let bitrate = kbps * 1000;
    let padding = ((h >> 9) & 1) as usize;
    let (len, samples) = match layer {
        1 => ((12 * bitrate / rate) as usize * 4 + padding * 4, 384),
        2 => ((144 * bitrate / rate) as usize + padding, 1152),
        _ if version == 1 => ((144 * bitrate / rate) as usize + padding, 1152),
        _ => ((72 * bitrate / rate) as usize + padding, 576),
    };
    if len < 4 {
        return None;
    }
    Some(Mp3Frame { version, layer, bitrate, rate, len, samples, mono: (h >> 6) & 3 == 3 })
}

fn mp3(body: &[u8], start: usize) -> Probe {
    // The first frame starts at once, or after a little padding.
    let window = body.len().min(start.saturating_add(4096));
    let (at, frame) = (start..window)
        .find_map(|at| {
            let frame = mp3_frame(body, at)?;
            // Confirmed by the next frame, unless the file ends first.
            let next = at + frame.len;
            let confirmed = match mp3_frame(body, next) {
                Some(n) => n.version == frame.version && n.layer == frame.layer && n.rate == frame.rate,
                None => next >= body.len(),
            };
            confirmed.then_some((at, frame))
        })
        .ok_or(ProbeError::Unrecognized)?;
    // A Xing/Info or VBRI header gives the frame count of a VBR file.
    let side = match (frame.version, frame.mono) {
        (1, false) => 32,
        (1, true) => 17,
        (_, false) => 17,
        (_, true) => 9,
    };
    let xing = at + 4 + side;
    let frames = match body.get(xing..xing + 4) {
        Some(b"Xing" | b"Info") if be32(body, xing + 4).is_some_and(|f| f & 1 != 0) => be32(body, xing + 8),
        _ if body.get(at + 36..at + 40) == Some(b"VBRI") => be32(body, at + 36 + 14),
        _ => None,
    };
    let seconds = match frames {
        Some(n) => f64::from(n) * f64::from(frame.samples) / f64::from(frame.rate),
        None => {
            let tail = if body.len() >= 128 && &body[body.len() - 128..body.len() - 125] == b"TAG" { 128 } else { 0 };
            let bytes = body.len().saturating_sub(at + tail);
            bytes as f64 * 8.0 / f64::from(frame.bitrate)
        }
    };
    Ok(MediaInfo { duration: finite_or_infinite(seconds), video: None, audio: true })
}

// ---------------------------------------------------------------------------
// canPlayType

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum CanPlay {
    No,
    Maybe,
    Probably,
}

impl CanPlay {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            CanPlay::No => "",
            CanPlay::Maybe => "maybe",
            CanPlay::Probably => "probably",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Mp4,
    WebM,
    Ogg,
    Matroska,
    Wav,
    /// One implicit codec: no `codecs` parameter is needed for "probably".
    Mp3,
    Flac,
    Aac,
}

/// Chrome's answer for a MIME type with an optional `codecs` parameter
/// (desktop Chrome, proprietary codecs included).
pub(crate) fn can_play_type(mime: &str) -> CanPlay {
    let mut parts = mime.split(';');
    let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let (family, audio_only) = match essence.as_str() {
        "video/mp4" => (Family::Mp4, false),
        "audio/mp4" | "audio/x-m4a" => (Family::Mp4, true),
        "video/webm" => (Family::WebM, false),
        "audio/webm" => (Family::WebM, true),
        "video/ogg" => (Family::Ogg, false),
        "audio/ogg" | "application/ogg" => (Family::Ogg, true),
        "video/x-matroska" => (Family::Matroska, false),
        "audio/wav" | "audio/x-wav" | "audio/wave" => (Family::Wav, true),
        "audio/mpeg" | "audio/mp3" | "audio/x-mp3" => (Family::Mp3, true),
        "audio/flac" => (Family::Flac, true),
        "audio/aac" => (Family::Aac, true),
        _ => return CanPlay::No,
    };
    let mut codecs = None;
    for param in parts {
        let Some((name, value)) = param.split_once('=') else { continue };
        if name.trim().eq_ignore_ascii_case("codecs") {
            let value = value.trim();
            let value = value.strip_prefix('"').map_or(value, |v| v.strip_suffix('"').unwrap_or(v));
            codecs = Some(value.to_owned());
        }
    }
    let codecs: Vec<&str> =
        codecs.as_deref().map_or_else(Vec::new, |c| c.split(',').map(str::trim).filter(|c| !c.is_empty()).collect());
    if codecs.is_empty() {
        return match family {
            Family::Mp3 | Family::Flac | Family::Aac => CanPlay::Probably,
            _ => CanPlay::Maybe,
        };
    }
    let mut answer = CanPlay::Probably;
    for codec in codecs {
        let (support, video) = codec_support(family, codec);
        if support == CanPlay::No || (video && audio_only) {
            return CanPlay::No;
        }
        answer = answer.min(support);
    }
    answer
}

/// One codec's support in a container family, and whether it is a video
/// codec.
fn codec_support(family: Family, codec: &str) -> (CanPlay, bool) {
    use CanPlay::*;
    let video = |c: CanPlay| (c, true);
    let audio = |c: CanPlay| (c, false);
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    let mp4_video = |c: &str| -> Option<(CanPlay, bool)> {
        if let Some(rest) = c.strip_prefix("avc1.").or_else(|| c.strip_prefix("avc3.")) {
            return Some(video(if hex(rest, 6) { Probably } else { No }));
        }
        if matches!(c, "avc1" | "avc3") {
            return Some(video(Maybe));
        }
        if c.starts_with("hvc1.") || c.starts_with("hev1.") {
            return Some(video(Probably));
        }
        None
    };
    let vpx_av1 = |c: &str| -> Option<(CanPlay, bool)> {
        if matches!(c, "vp8" | "vp8.0" | "vp9" | "vp9.0") {
            return Some(video(Probably));
        }
        if let Some(rest) = c.strip_prefix("vp09.") {
            let ok = rest.split('.').count() >= 3
                && rest.split('.').all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()));
            return Some(video(if ok { Probably } else { No }));
        }
        if let Some(rest) = c.strip_prefix("av01.") {
            return Some(video(if rest.split('.').count() >= 3 { Probably } else { No }));
        }
        None
    };
    let aac = |c: &str| -> Option<(CanPlay, bool)> {
        let rest = c.strip_prefix("mp4a.")?;
        Some(audio(match rest {
            "40" => Maybe,
            "40.2" | "40.02" | "40.5" | "40.05" | "40.29" | "40.42" | "69" | "6B" | "6b" | "67" | "66" | "68" => {
                Probably
            }
            _ => No,
        }))
    };
    let found = match family {
        Family::Mp4 => mp4_video(codec)
            .or_else(|| vpx_av1(codec).filter(|_| codec.starts_with("vp09") || codec.starts_with("av01")))
            .or_else(|| aac(codec))
            .or(match codec {
                "opus" | "Opus" | "flac" | "fLaC" | "mp3" => Some(audio(Probably)),
                _ => None,
            }),
        Family::WebM => vpx_av1(codec).or(match codec {
            "opus" | "vorbis" => Some(audio(Probably)),
            _ => None,
        }),
        Family::Matroska => vpx_av1(codec).or_else(|| mp4_video(codec)).or_else(|| aac(codec)).or(match codec {
            "opus" | "vorbis" | "flac" | "mp3" => Some(audio(Probably)),
            _ => None,
        }),
        Family::Ogg => match codec {
            "opus" | "vorbis" | "flac" => Some(audio(Probably)),
            _ => None,
        },
        Family::Wav => (codec == "1").then_some(audio(Probably)),
        Family::Mp3 => matches!(codec, "mp3" | "mp4a.69" | "mp4a.6B" | "mp4a.40.34").then_some(audio(Probably)),
        Family::Flac => (codec == "flac").then_some(audio(Probably)),
        Family::Aac => aac(codec),
    };
    found.unwrap_or((No, false))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Minimal media files, as the browser tests serve them too.
    pub(crate) mod samples {
        fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(payload);
            out
        }

        fn trak(handler: &[u8; 4], format: &[u8; 4], size: (u32, u32), timescale: u32, duration: u32) -> Vec<u8> {
            let mut tkhd = vec![0, 0, 0, 1];
            tkhd.extend_from_slice(&[0; 20]);
            tkhd.extend_from_slice(&[0; 16]);
            let identity = [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
            for v in identity {
                tkhd.extend_from_slice(&v.to_be_bytes());
            }
            tkhd.extend_from_slice(&(size.0 << 16).to_be_bytes());
            tkhd.extend_from_slice(&(size.1 << 16).to_be_bytes());
            let mut mdhd = vec![0; 12];
            mdhd.extend_from_slice(&timescale.to_be_bytes());
            mdhd.extend_from_slice(&duration.to_be_bytes());
            mdhd.extend_from_slice(&[0; 4]);
            let mut hdlr = vec![0; 8];
            hdlr.extend_from_slice(handler);
            hdlr.extend_from_slice(&[0; 13]);
            let mut entry = vec![0; 8];
            entry.extend_from_slice(&[0; 16]);
            entry.extend_from_slice(&(size.0 as u16).to_be_bytes());
            entry.extend_from_slice(&(size.1 as u16).to_be_bytes());
            let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
            stsd.extend(mp4_box(format, &entry));
            let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &stsd));
            let minf = mp4_box(b"minf", &stbl);
            let mdia = mp4_box(b"mdia", &[mp4_box(b"mdhd", &mdhd), mp4_box(b"hdlr", &hdlr), minf].concat());
            mp4_box(b"trak", &[mp4_box(b"tkhd", &tkhd), mdia].concat())
        }

        /// An MP4 of `seconds` with an H.264 `size` video and an AAC track.
        pub fn mp4(seconds: f64, size: (u32, u32)) -> Vec<u8> {
            let mut mvhd = vec![0; 12];
            mvhd.extend_from_slice(&1000u32.to_be_bytes());
            mvhd.extend_from_slice(&((seconds * 1000.0) as u32).to_be_bytes());
            mvhd.extend_from_slice(&[0; 80]);
            let moov = mp4_box(
                b"moov",
                &[
                    mp4_box(b"mvhd", &mvhd),
                    trak(b"vide", b"avc1", size, 90000, (seconds * 90000.0) as u32),
                    trak(b"soun", b"mp4a", (0, 0), 44100, (seconds * 44100.0) as u32),
                ]
                .concat(),
            );
            [mp4_box(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41"), moov, mp4_box(b"mdat", &[0; 16])].concat()
        }

        fn ebml(id: u64, payload: &[u8]) -> Vec<u8> {
            let id_bytes = id.to_be_bytes();
            let skip = id_bytes.iter().position(|&b| b != 0).unwrap_or(7);
            let mut out = id_bytes[skip..].to_vec();
            // An 8-byte size.
            out.push(0x01);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
            out.extend_from_slice(payload);
            out
        }

        /// A WebM of `seconds` with a VP9 `size` video and an Opus track.
        pub fn webm(seconds: f64, size: (u32, u32)) -> Vec<u8> {
            let header = ebml(0x1A45DFA3, &ebml(0x4282, b"webm"));
            let info = ebml(
                0x1549A966,
                &[ebml(0x2AD7B1, &[0x0F, 0x42, 0x40]), ebml(0x4489, &(seconds * 1000.0).to_be_bytes())].concat(),
            );
            let video = ebml(
                0xAE,
                &[
                    ebml(0x83, &[1]),
                    ebml(0x86, b"V_VP9"),
                    ebml(
                        0xE0,
                        &[ebml(0xB0, &(size.0 as u16).to_be_bytes()), ebml(0xBA, &(size.1 as u16).to_be_bytes())]
                            .concat(),
                    ),
                ]
                .concat(),
            );
            let audio = ebml(0xAE, &[ebml(0x83, &[2]), ebml(0x86, b"A_OPUS")].concat());
            let tracks = ebml(0x1654AE6B, &[video, audio].concat());
            let cluster = ebml(0x1F43B675, &[0xE7, 0x81, 0x00]);
            [header, ebml(0x18538067, &[info, tracks, cluster].concat())].concat()
        }

        fn ogg_page(flags: u8, granule: u64, serial: u32, seq: u32, packet: &[u8]) -> Vec<u8> {
            let mut out = b"OggS\0".to_vec();
            out.push(flags);
            out.extend_from_slice(&granule.to_le_bytes());
            out.extend_from_slice(&serial.to_le_bytes());
            out.extend_from_slice(&seq.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            let mut lacing = vec![255u8; packet.len() / 255];
            lacing.push((packet.len() % 255) as u8);
            out.push(lacing.len() as u8);
            out.extend_from_slice(&lacing);
            out.extend_from_slice(packet);
            out
        }

        /// An Ogg Opus file of `seconds`.
        pub fn ogg_opus(seconds: f64) -> Vec<u8> {
            let mut head = b"OpusHead\x01\x02".to_vec();
            head.extend_from_slice(&312u16.to_le_bytes());
            head.extend_from_slice(&48000u32.to_le_bytes());
            head.extend_from_slice(&[0, 0, 0]);
            let end = (seconds * 48000.0) as u64 + 312;
            [
                ogg_page(0x02, 0, 7, 0, &head),
                ogg_page(0, 0, 7, 1, b"OpusTags\0\0\0\0\0\0\0\0"),
                ogg_page(0, end / 2, 7, 2, &[0xFC; 40]),
                ogg_page(0x04, end, 7, 3, &[0xFC; 40]),
            ]
            .concat()
        }

        /// A 16-bit mono PCM WAV of `seconds` at 8 kHz.
        pub fn wav(seconds: f64) -> Vec<u8> {
            let data = vec![0u8; (seconds * 16000.0) as usize];
            let mut out = b"RIFF".to_vec();
            out.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
            out.extend_from_slice(b"WAVEfmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            for v in [1u16, 1] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&8000u32.to_le_bytes());
            out.extend_from_slice(&16000u32.to_le_bytes());
            for v in [2u16, 16] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(b"data");
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend(data);
            out
        }

        /// A CBR 128 kbps 44.1 kHz MP3 of about `seconds`, after an ID3v2 tag.
        pub fn mp3(seconds: f64) -> Vec<u8> {
            let mut out = b"ID3\x04\0\0\0\0\0\x0A".to_vec();
            out.extend_from_slice(&[0; 10]);
            // MPEG-1 Layer III, 128 kbps, 44100 Hz, no padding: 417 bytes.
            let frames = (seconds * 44100.0 / 1152.0).round() as usize;
            for _ in 0..frames {
                out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
                out.extend_from_slice(&[0; 413]);
            }
            out
        }
    }

    #[test]
    fn probes_generated_files() {
        let mp4 = probe(&samples::mp4(2.5, (320, 240))).unwrap();
        assert_eq!(mp4, MediaInfo { duration: 2.5, video: Some((320, 240)), audio: true });
        let webm = probe(&samples::webm(1.25, (640, 360))).unwrap();
        assert_eq!(webm, MediaInfo { duration: 1.25, video: Some((640, 360)), audio: true });
        let ogg = probe(&samples::ogg_opus(3.0)).unwrap();
        assert_eq!(ogg, MediaInfo { duration: 3.0, video: None, audio: true });
        let wav = probe(&samples::wav(0.5)).unwrap();
        assert_eq!(wav, MediaInfo { duration: 0.5, video: None, audio: true });
        let mp3 = probe(&samples::mp3(2.0)).unwrap();
        assert!((mp3.duration - 2.0).abs() < 0.02, "{}", mp3.duration);
        assert_eq!(mp3.video, None);
    }

    #[test]
    fn garbage_and_truncated_input_never_panics() {
        assert_eq!(probe(b""), Err(ProbeError::Unrecognized));
        assert_eq!(probe(b"<!doctype html><p>not media"), Err(ProbeError::Unrecognized));
        assert_eq!(probe(&[0xFF; 64]), Err(ProbeError::Unrecognized));
        for file in [
            samples::mp4(1.0, (2, 2)),
            samples::webm(1.0, (2, 2)),
            samples::ogg_opus(1.0),
            samples::wav(0.1),
            samples::mp3(0.2),
        ] {
            for cut in 0..file.len().min(600) {
                let _ = probe(&file[..cut]);
            }
            // Corrupt bytes.
            let mut bad = file.clone();
            for i in (0..bad.len()).step_by(7) {
                bad[i] ^= 0xA5;
                let _ = probe(&bad);
            }
        }
        // Box and element sizes that point far beyond the data.
        let _ = probe(b"\x00\x00\x00\x01moov\xff\xff\xff\xff\xff\xff\xff\xff");
        let _ = probe(&[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn random_bytes_after_every_magic_never_panic() {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let magics: [&[u8]; 9] = [
            &[0x1A, 0x45, 0xDF, 0xA3],
            b"OggS",
            b"RIFF\0\0\0\0WAVE",
            b"\0\0\0\x10ftyp",
            b"\0\0\0\x10moov",
            b"ID3\x04\0\0\0\0\0\x01",
            b"fLaC",
            &[0xFF, 0xF1],
            &[0xFF, 0xFB],
        ];
        for round in 0..4000 {
            let magic = magics[round % magics.len()];
            let len = (next() % 512) as usize;
            let mut data = magic.to_vec();
            data.extend((0..len).map(|_| next() as u8));
            let _ = probe(&data);
        }
    }

    #[test]
    fn ogg_with_many_streams_is_linear() {
        // 200k streams, each begun by a minimal Opus BOS page: a linear
        // stream lookup per page made this quadratic (~2e10 comparisons).
        let mut head = b"OpusHead\x01\x02".to_vec();
        head.extend_from_slice(&[0; 9]);
        let mut page = b"OggS\0\x02".to_vec();
        page.extend_from_slice(&[0; 8]);
        let serial_at = page.len();
        page.extend_from_slice(&[0; 12]);
        page.push(1);
        page.push(head.len() as u8);
        page.extend_from_slice(&head);
        let mut body = Vec::with_capacity(page.len() * 200_000);
        for serial in 0..200_000u32 {
            page[serial_at..serial_at + 4].copy_from_slice(&serial.to_le_bytes());
            body.extend_from_slice(&page);
        }
        let start = std::time::Instant::now();
        assert_eq!(probe(&body).map(|i| i.audio), Ok(true));
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn version_1_movie_header_duration() {
        // A v1 mvhd: version/flags, 64-bit creation and modification times,
        // the timescale, a 64-bit duration, then the rate (0x00010000).
        let mut v1 = vec![1, 0, 0, 0];
        v1.extend_from_slice(&[0; 16]);
        v1.extend_from_slice(&1000u32.to_be_bytes());
        v1.extend_from_slice(&10_000u64.to_be_bytes());
        v1.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        v1.extend_from_slice(&[0; 76]);
        assert_eq!(header_duration(&v1), Some((1000, 10_000)));
        // A fragmented file's zero duration stays zero (the mehd fallback).
        v1[24..32].copy_from_slice(&0u64.to_be_bytes());
        assert_eq!(header_duration(&v1), Some((1000, 0)));
        // Swapped into a whole file.
        let mut file = samples::mp4(2.5, (320, 240));
        let at = file.windows(4).position(|w| w == b"mvhd").unwrap() - 4;
        let old = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        let mut v1 = vec![1, 0, 0, 0];
        v1.extend_from_slice(&[0; 16]);
        v1.extend_from_slice(&1000u32.to_be_bytes());
        v1.extend_from_slice(&2_500u64.to_be_bytes());
        v1.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        v1.extend_from_slice(&[0; 76]);
        let mut mvhd = ((v1.len() + 8) as u32).to_be_bytes().to_vec();
        mvhd.extend_from_slice(b"mvhd");
        mvhd.extend_from_slice(&v1);
        let grow = mvhd.len() - old;
        file.splice(at..at + old, mvhd);
        let moov = file.windows(4).position(|w| w == b"moov").unwrap() - 4;
        let size = u32::from_be_bytes(file[moov..moov + 4].try_into().unwrap()) + grow as u32;
        file[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
        assert_eq!(probe(&file).map(|i| i.duration), Ok(2.5));
    }

    #[test]
    fn oversized_video_cannot_play() {
        assert_eq!(probe(&samples::webm(1.0, (40_000, 10))).map(|i| i.video), Ok(None));
        let mut huge = samples::mp4(1.0, (30_000, 30_000));
        // Only the video track: drop the audio trak by renaming its handler.
        let at = huge.windows(4).position(|w| w == b"soun").unwrap();
        huge[at..at + 4].copy_from_slice(b"text");
        assert_eq!(probe(&huge), Err(ProbeError::NoSupportedStreams));
    }

    #[test]
    fn unsupported_codecs_are_reported() {
        let mut theora = b"\x80theora".to_vec();
        theora.extend_from_slice(&[0; 40]);
        let mut ogg = b"OggS\0\x02".to_vec();
        ogg.extend_from_slice(&[0; 20]);
        ogg.push(1);
        ogg.push(theora.len() as u8);
        ogg.extend(theora);
        assert_eq!(probe(&ogg), Err(ProbeError::NoSupportedStreams));
        let mut wav = samples::wav(0.1);
        wav[20] = 0x55; // MPEG layer 3 in WAV
        assert_eq!(probe(&wav), Err(ProbeError::NoSupportedStreams));
    }

    #[test]
    fn can_play_type_answers_like_chrome() {
        let t = |m: &str| can_play_type(m).as_str();
        assert_eq!(t("video/mp4"), "maybe");
        assert_eq!(t("video/mp4;codecs="), "maybe");
        assert_eq!(t("video/mp4; codecs=\"avc1.42E01E, mp4a.40.2\""), "probably");
        assert_eq!(t("video/mp4; codecs=\"mp4v.20.8\""), "");
        assert_eq!(t("audio/mp4; codecs=\"avc1.42E01E\""), "");
        assert_eq!(t("audio/mpeg"), "probably");
        assert_eq!(t("audio/MP3"), "probably");
        assert_eq!(t("audio/wav"), "maybe");
        assert_eq!(t("audio/wav; codecs=\"1\""), "probably");
        assert_eq!(t("video/webm; codecs=\"vp8, vorbis\""), "probably");
        assert_eq!(t("video/webm; codecs=\"vp9, opus\""), "probably");
        assert_eq!(t("video/webm; codecs=\"vp09.00.10.08\""), "probably");
        assert_eq!(t("video/webm; codecs=\"bogus\""), "");
        assert_eq!(t("audio/webm; codecs=\"vp8\""), "");
        assert_eq!(t("audio/ogg"), "maybe");
        assert_eq!(t("audio/ogg; codecs=opus"), "probably");
        assert_eq!(t("video/ogg; codecs=\"theora\""), "");
        assert_eq!(t("application/octet-stream"), "");
        assert_eq!(t("video/x-new-fictional-format;codecs=\"kittens,bunnies\""), "");
        assert_eq!(t(""), "");
    }
}
