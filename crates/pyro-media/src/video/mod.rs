//! Video frames on demand. A [`Player`] lists a file's video samples
//! ([`demux`]) when made and decodes nothing until a frame is asked for:
//! [`Player::frame_at`] decodes from the key frame before the frame shown at
//! that time (or goes on from where the last call stopped, during playback)
//! and converts only that frame to RGBA.
use std::sync::Arc;

use bytes::Bytes;

pub mod demux;
pub mod encode;
mod packet;
pub use packet::{PacketConfig, PacketDecoder, PacketFrame, PacketPixels};
#[cfg(any(feature = "av1", feature = "h264"))]
mod yuv;

#[cfg(feature = "av1")]
mod av1;
#[cfg(feature = "h264")]
mod h264;

pub use demux::{Codec, Sample, Track};

/// Whether this build decodes AV1 (cargo feature `av1`).
pub const AV1: bool = cfg!(feature = "av1");
/// Whether this build decodes H.264 (cargo feature `h264`).
pub const H264: bool = cfg!(feature = "h264");

/// Whether this build has a decoder for `codec`.
pub fn decodes(codec: &Codec) -> bool {
    match codec {
        Codec::Av1 => AV1,
        Codec::H264 { .. } => H264,
        Codec::Other(_) => false,
    }
}

/// A decoded picture coming out of a decoder: the tag of the sample it
/// was sent with, and the requested output pixels (RGBA for media elements).
pub(crate) struct Decoded {
    pub tag: u64,
    pub image: Option<(u32, u32, Vec<u8>)>,
}

pub(crate) trait Decoder {
    /// Sends one sample; pictures that come out go to `out`, converted
    /// when `want` says so.
    fn feed(
        &mut self,
        sample: &[u8],
        tag: u64,
        want: &mut dyn FnMut(u64, u32, u32) -> bool,
        out: &mut Vec<Decoded>,
    ) -> Result<(), String>;
    /// The end of the stream: the pictures still held.
    fn finish(&mut self, want: &mut dyn FnMut(u64, u32, u32) -> bool, out: &mut Vec<Decoded>) -> Result<(), String>;
}

/// A decoded frame: its index in presentation order, its time, and straight
/// RGBA rows (opaque).
#[derive(Clone, Debug)]
pub struct Frame {
    pub index: usize,
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// Largest frame decoded (Chrome's limit is similar).
const MAX_AREA: u64 = 1 << 26;

pub struct Player {
    data: Bytes,
    track: Track,
    /// Presentation index -> sample index (decode order).
    order: Vec<usize>,
    /// Sample index -> presentation index.
    rank: Vec<usize>,
    decoder: Option<Box<dyn Decoder>>,
    /// The next sample the decoder gets, and the key frame it started at.
    next: usize,
    run_start: usize,
    /// Presentation index of the last picture out of the decoder.
    last_out: Option<usize>,
    frame: Option<Frame>,
}

impl Player {
    /// Lists the video samples of `data`; `None` without a video track this
    /// build decodes, or without samples.
    pub fn open(data: Bytes) -> Option<Player> {
        let track = demux::video_track(&data)?;
        if !decodes(&track.codec) || track.samples.is_empty() {
            return None;
        }
        let mut order: Vec<usize> = (0..track.samples.len()).collect();
        order.sort_by(|&a, &b| track.samples[a].pts.total_cmp(&track.samples[b].pts));
        let mut rank = vec![0; order.len()];
        for (p, &s) in order.iter().enumerate() {
            rank[s] = p;
        }
        Some(Player { data, track, order, rank, decoder: None, next: 0, run_start: 0, last_out: None, frame: None })
    }

    pub fn codec(&self) -> &Codec {
        &self.track.codec
    }

    /// The container's frame size.
    pub fn size(&self) -> (u32, u32) {
        (self.track.width, self.track.height)
    }

    pub fn frame_count(&self) -> usize {
        self.order.len()
    }

    /// Presentation time of frame `index`.
    pub fn frame_time(&self, index: usize) -> f64 {
        self.order.get(index).map_or(0.0, |&s| self.track.samples[s].pts)
    }

    /// The frame shown at `time`: the last one presented at or before it
    /// (the first before the first frame's time).
    pub fn index_at(&self, time: f64) -> usize {
        let p = self.order.partition_point(|&s| self.track.samples[s].pts <= time + 1e-6);
        p.saturating_sub(1)
    }

    /// The frame shown at `time`, decoded; `None` if it cannot be decoded.
    pub fn frame_at(&mut self, time: f64) -> Option<Frame> {
        let index = self.index_at(time);
        if self.frame.as_ref().is_some_and(|f| f.index == index) {
            return self.frame.clone();
        }
        if let Some(frame) = self.decode(index) {
            self.frame = Some(frame);
        }
        self.frame.clone()
    }

    fn decode(&mut self, target: usize) -> Option<Frame> {
        let sample = *self.order.get(target)?;
        let key = (0..=sample).rev().find(|&s| self.track.samples[s].key).unwrap_or(0);
        // Pictures come out in presentation order: the decoder goes on
        // (playback) while the target is still to come and its key frame
        // lies within what it was sent; otherwise it starts again there.
        let go_on = self.decoder.is_some()
            && self.last_out.is_none_or(|q| q < target)
            && self.run_start <= key
            && key <= self.next;
        if !go_on {
            self.decoder = Some(self.new_decoder()?);
            self.next = key;
            self.last_out = None;
            self.run_start = key;
        }
        let time = self.frame_time(target);
        let rank = &self.rank;
        let samples = &self.track.samples;
        let mut want = |tag: u64, _: u32, _: u32| rank.get(tag as usize) == Some(&target);
        let mut out = Vec::new();
        let mut found = None;
        let mut done = false;
        let mut flushed = false;
        let decoder = self.decoder.as_mut()?;
        while !done {
            if self.next < samples.len() {
                let s = samples[self.next];
                let bytes = self.data.get(s.offset..s.offset + s.size).unwrap_or(&[]);
                if decoder.feed(bytes, self.next as u64, &mut want, &mut out).is_err() {
                    done = true;
                }
                self.next += 1;
            } else {
                // Everything was sent: flush what the decoder holds.
                let _ = decoder.finish(&mut want, &mut out);
                done = true;
                flushed = true;
            }
            for d in out.drain(..) {
                let p = rank.get(d.tag as usize).copied();
                self.last_out = p.or(self.last_out);
                if p == Some(target) {
                    if let Some((w, h, rgba)) = d.image {
                        if u64::from(w) * u64::from(h) <= MAX_AREA {
                            found = Some(Frame { index: target, time, width: w, height: h, rgba: Arc::new(rgba) });
                        }
                    }
                }
            }
            done |= found.is_some() || self.last_out.is_some_and(|q| q >= target);
        }
        if found.is_none() || flushed {
            // A failed or flushed decoder starts again next time.
            self.decoder = None;
        }
        found
    }

    fn new_decoder(&self) -> Option<Box<dyn Decoder>> {
        match &self.track.codec {
            #[cfg(feature = "av1")]
            Codec::Av1 => Some(Box::new(av1::Av1::new()?)),
            #[cfg(feature = "h264")]
            Codec::H264 { length_size, parameter_sets } => {
                Some(Box::new(h264::H264::new(*length_size, parameter_sets.clone())?))
            }
            _ => None,
        }
    }
}
