//! Incremental ISO BMFF byte-stream input for Media Source Extensions.
//! Payload ownership transfers to the coded frame buffer as shared Bytes slices.
use bytes::{Bytes, BytesMut};
use std::collections::BTreeMap;

mod fragment;
mod init;

const MAX_BOX: usize = 32 << 20;
const MAX_SAMPLES: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported,
    Quota,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub codec: String,
    pub description: Bytes,
    pub timescale: u32,
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub track_id: u32,
    pub dts: f64,
    pub pts: f64,
    pub duration: f64,
    pub key: bool,
    pub data: Bytes,
}

#[derive(Debug)]
pub enum Event {
    Init(Vec<Track>, Option<f64>),
    Samples(Vec<Sample>),
}

struct TrackInfo {
    track: Track,
    duration: u32,
    size: u32,
    flags: u32,
    edit: f64,
}
struct Span {
    offset: u64,
    size: usize,
    sample: Sample,
}

#[derive(Default)]
pub struct Mp4Parser {
    input: BytesMut,
    position: u64,
    ftyp: bool,
    tracks: BTreeMap<u32, TrackInfo>,
    spans: Vec<Span>,
    duration: Option<f64>,
}

impl Mp4Parser {
    pub fn duration(&self) -> Option<f64> {
        self.duration
    }
    pub fn buffered_bytes(&self) -> usize {
        self.input.len() + self.spans.len() * std::mem::size_of::<Span>()
    }
    pub fn partial_segment(&self) -> bool {
        !self.spans.is_empty() || matches!(self.input.get(4..8), Some(b"moof" | b"mdat" | b"styp"))
    }
    pub fn reset(&mut self) {
        self.input = BytesMut::new();
        self.spans = Vec::new();
        self.ftyp = false;
    }

    pub fn append(&mut self, input: &[u8]) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        // Copy only the current box, including when one append contains many
        // complete fragments. Each mdat allocation is shared by its samples.
        let mut remaining = input;
        while !remaining.is_empty() {
            let wanted = if self.input.len() < 8 {
                8
            } else if u32at(&self.input, 0)? == 1 && self.input.len() < 16 {
                16
            } else {
                header(&self.input)?.1
            };
            let n = remaining.len().min(wanted - self.input.len());
            self.input.extend_from_slice(&remaining[..n]);
            remaining = &remaining[n..];
            if self.input.len() < 8 || (u32at(&self.input, 0)? == 1 && self.input.len() < 16) {
                continue;
            }
            let (start, size) = header(&self.input)?;
            if self.input.len() != size {
                continue;
            }
            let bytes = self.input.split().freeze();
            let pos = self.position;
            self.position = self.position.checked_add(size as u64).ok_or(Error::Quota)?;
            let data = bytes.slice(start..);
            match &bytes[4..8] {
                b"ftyp" => {
                    if data.len() < 8 || data.len() % 4 != 0 || !self.spans.is_empty() {
                        return Err(Error::Invalid);
                    }
                    self.ftyp = true;
                }
                b"moov" => {
                    if !self.ftyp || !self.spans.is_empty() {
                        return Err(Error::Invalid);
                    }
                    let (tracks, duration) = init::parse(&data)?;
                    self.tracks = tracks;
                    self.duration = duration;
                    self.ftyp = false;
                    events.push(Event::Init(self.tracks.values().map(|info| info.track.clone()).collect(), duration));
                }
                b"moof" => {
                    if self.tracks.is_empty() || !self.spans.is_empty() {
                        return Err(Error::Invalid);
                    }
                    self.spans = fragment::parse(&data, pos, &self.tracks)?;
                }
                b"mdat" => {
                    if self.spans.is_empty() {
                        return Err(Error::Invalid);
                    }
                    let begin = pos + start as u64;
                    let end = self.position;
                    let count = self.spans.partition_point(|s| s.offset < end);
                    let mut samples = Vec::with_capacity(count);
                    for mut span in self.spans.drain(..count) {
                        let off = usize::try_from(span.offset.checked_sub(begin).ok_or(Error::Invalid)?)
                            .map_err(|_| Error::Invalid)?;
                        let last =
                            off.checked_add(span.size).filter(|&last| last <= data.len()).ok_or(Error::Invalid)?;
                        span.sample.data = data.slice(off..last);
                        samples.push(span.sample);
                    }
                    if !samples.is_empty() {
                        events.push(Event::Samples(samples));
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

fn u16at(b: &[u8], i: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(b.get(i..i + 2).ok_or(Error::Invalid)?.try_into().unwrap()))
}
fn u32at(b: &[u8], i: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(b.get(i..i + 4).ok_or(Error::Invalid)?.try_into().unwrap()))
}
fn u64at(b: &[u8], i: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(b.get(i..i + 8).ok_or(Error::Invalid)?.try_into().unwrap()))
}
fn header(b: &[u8]) -> Result<(usize, usize)> {
    let (start, n) = match u32at(b, 0)? {
        0 => return Err(Error::Unsupported),
        1 => (16, u64at(b, 8)?),
        n => (8, u64::from(n)),
    };
    if n > MAX_BOX as u64 {
        return Err(Error::Quota);
    }
    if n < start as u64 {
        return Err(Error::Invalid);
    }
    Ok((start, n as usize))
}
struct Boxes<'a>(&'a [u8]);
impl<'a> Iterator for Boxes<'a> {
    type Item = Result<(&'a [u8], &'a [u8])>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let b = std::mem::take(&mut self.0);
        Some(header(b).and_then(|(start, end)| {
            let data = b.get(start..end).ok_or(Error::Invalid)?;
            self.0 = &b[end..];
            Ok((&b[4..8], data))
        }))
    }
}
fn child<'a>(data: &'a [u8], kind: &[u8]) -> Result<&'a [u8]> {
    for item in Boxes(data) {
        let (k, b) = item?;
        if k == kind {
            return Ok(b);
        }
    }
    Err(Error::Invalid)
}
fn duration_field(b: &[u8], offset: usize) -> Result<Option<u64>> {
    let (ticks, unknown) = match b.first() {
        Some(0) => (u64::from(u32at(b, offset)?), u64::from(u32::MAX)),
        Some(1) => (u64at(b, offset)?, u64::MAX),
        _ => return Err(Error::Unsupported),
    };
    Ok((ticks != 0 && ticks != unknown).then_some(ticks))
}

fn timing(b: &[u8]) -> Result<(u32, Option<u64>)> {
    let offset = match b.first() {
        Some(0) => 12,
        Some(1) => 20,
        _ => return Err(Error::Unsupported),
    };
    let scale = u32at(b, offset)?;
    if scale == 0 {
        return Err(Error::Invalid);
    }
    Ok((scale, duration_field(b, offset + 4)?))
}
