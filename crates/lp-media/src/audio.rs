//! Whole-file audio decoding for Web Audio's `decodeAudioData`: any
//! container and codec Chrome decodes there (WAV, MP3, AAC/M4A, Ogg Vorbis,
//! Opus in Ogg/WebM/MP4, FLAC, plus AIFF, CAF, ALAC) to planar `f32`, and
//! the resampling to the context's rate that the API asks for.
use std::io::Cursor;

use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Decoded audio: planar channels at `sample_rate`.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Decoded {
    /// Frames per channel.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Most channels Web Audio buffers hold.
pub const MAX_CHANNELS: usize = 32;
/// Decoded frames past which decoding stops (about 6 h at 48 kHz): a
/// pathological file cannot exhaust memory.
const MAX_FRAMES: usize = 1 << 30;

/// Decodes the first audio track of `bytes`, whatever the container.
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&Hint::new(), source, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("unrecognized audio data: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio track")?.clone();
    let track_id = track.id;
    // The Opus pre-skip, from OpusHead when the container has no delay.
    let delay = track.delay.map(|d| d as usize).or_else(|| {
        let head = params.extra_data.as_deref()?;
        (head.starts_with(b"OpusHead") && head.len() >= 12).then(|| usize::from(u16::from_le_bytes([head[10], head[11]])))
    });
    let delay = delay.unwrap_or(0);
    let frames = track.num_frames.and_then(|n| usize::try_from(n).ok());
    let mut out = Channels::default();
    if params.codec == CODEC_ID_OPUS {
        let mut opus = Opus::new(&params)?;
        let mut trimmed = false;
        while let Some(packet) = format.next_packet().map_err(|e| e.to_string())? {
            if packet.track_id != track_id {
                continue;
            }
            trimmed |= packet.trim_start.get() > 0;
            let frames = opus.decode(&packet.data)?;
            let start = (packet.trim_start.get() as usize).min(frames);
            let end = frames.saturating_sub(packet.trim_end.get() as usize).max(start);
            out.push_interleaved(&opus.pcm[start * opus.channels..end * opus.channels], opus.channels);
            if out.len() > MAX_FRAMES {
                break;
            }
        }
        // Containers that do not trim the pre-skip on packets (WebM, MP4).
        if !trimmed && delay > 0 {
            out.skip(delay);
        }
        return out.finish(48_000);
    }
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("unsupported audio codec: {e}"))?;
    let mut rate = params.sample_rate.unwrap_or(0);
    let mut planes: Vec<Vec<f32>> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A truncated file decodes up to where it ends.
            Err(Error::IoError(_)) if !out.is_empty() => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                rate = buffer.spec().rate();
                planes.clear();
                buffer.copy_to_vecs_planar::<f32>(&mut planes);
                out.push_planes(&planes);
            }
            // A damaged packet is skipped, as Chrome's FFmpeg does.
            Err(Error::DecodeError(_)) => {}
            Err(Error::IoError(_)) => break,
            Err(e) => return Err(e.to_string()),
        }
        if out.len() > MAX_FRAMES {
            break;
        }
    }
    // A container that knows its length (Ogg granule positions) trims the
    // padding of the last packet.
    if let Some(n) = frames.filter(|&n| n > 0 && n < out.len()) {
        out.truncate(n);
    }
    out.finish(rate)
}

#[derive(Default)]
struct Channels(Vec<Vec<f32>>);

impl Channels {
    fn len(&self) -> usize {
        self.0.first().map_or(0, Vec::len)
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn ensure(&mut self, count: usize) {
        let len = self.len();
        while self.0.len() < count.min(MAX_CHANNELS) {
            self.0.push(vec![0.0; len]);
        }
    }
    fn push_planes(&mut self, planes: &[Vec<f32>]) {
        self.ensure(planes.len());
        let frames = planes.iter().map(Vec::len).max().unwrap_or(0);
        for (i, channel) in self.0.iter_mut().enumerate() {
            match planes.get(i) {
                Some(p) => {
                    channel.extend_from_slice(p);
                    channel.resize(channel.len() + frames - p.len(), 0.0);
                }
                None => channel.resize(channel.len() + frames, 0.0),
            }
        }
    }
    fn push_interleaved(&mut self, pcm: &[f32], channels: usize) {
        self.ensure(channels);
        for frame in pcm.chunks_exact(channels) {
            for (c, channel) in self.0.iter_mut().enumerate() {
                channel.push(frame.get(c).copied().unwrap_or(0.0));
            }
        }
    }
    fn truncate(&mut self, frames: usize) {
        for c in &mut self.0 {
            c.truncate(frames);
        }
    }
    fn skip(&mut self, frames: usize) {
        for c in &mut self.0 {
            c.drain(..frames.min(c.len()));
        }
    }
    fn finish(self, sample_rate: u32) -> Result<Decoded, String> {
        if self.is_empty() || sample_rate == 0 {
            return Err("no audio decoded".into());
        }
        Ok(Decoded { sample_rate, channels: self.0 })
    }
}

/// Opus packets (any container) through `opus-decoder`, at 48 kHz.
struct Opus {
    decoder: opus_pure::OpusDecoder,
    channels: usize,
    pcm: Vec<f32>,
}

impl Opus {
    fn new(params: &AudioCodecParameters) -> Result<Opus, String> {
        let channels = params.channels.as_ref().map_or(2, |c| c.count()).max(1);
        if channels > 2 {
            return Err("multichannel Opus is not supported".into());
        }
        let decoder = opus_pure::OpusDecoder::new(48_000, channels).map_err(|e| format!("{e:?}"))?;
        // 120 ms at 48 kHz is the longest Opus packet.
        Ok(Opus { decoder, channels, pcm: vec![0.0; 5760 * channels] })
    }
    /// Decodes one packet into `pcm` (interleaved); returns its frames.
    fn decode(&mut self, packet: &[u8]) -> Result<usize, String> {
        match self.decoder.decode(packet, 5760, &mut self.pcm) {
            Ok(frames) => Ok(frames),
            // A damaged packet is concealed.
            Err(_) => Ok(self.decoder.decode(&[], 960, &mut self.pcm).unwrap_or(0)),
        }
    }
}

/// Resamples one channel from `from` Hz to `to` Hz with a windowed-sinc
/// filter (Chrome resamples `decodeAudioData` results to the context's
/// rate). The result has `floor(len * to / from)` frames, as Chrome's.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let out_len = (input.len() as f64 / ratio) as usize;
    // Cut-off at the lower Nyquist frequency when downsampling.
    let cutoff = if ratio > 1.0 { 1.0 / ratio } else { 1.0 };
    const HALF: f64 = 16.0;
    const PHASES: usize = 256;
    let half = (HALF / cutoff).ceil() as isize;
    // The kernel sampled every 1/PHASES input frames over [-half, half].
    let span = 2 * half as usize * PHASES + 1;
    let table: Vec<f64> = (0..=span)
        .map(|i| {
            let x = i as f64 / PHASES as f64 - half as f64;
            cutoff * sinc(cutoff * x) * window(x / half as f64)
        })
        .collect();
    let kernel = |x: f64| {
        let at = (x + half as f64) * PHASES as f64;
        let i = at.floor();
        let (i, f) = (i as usize, at - i);
        match (table.get(i), table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            _ => 0.0,
        }
    };
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = i as f64 * ratio;
        let base = center.floor() as isize;
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for j in (base - half + 1)..=(base + half) {
            let k = kernel(center - j as f64);
            norm += k;
            if j >= 0 && (j as usize) < input.len() {
                acc += f64::from(input[j as usize]) * k;
            }
        }
        out.push(if norm.abs() > 1e-12 { (acc / norm) as f32 } else { 0.0 });
    }
    out
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Blackman window over [-1, 1].
fn window(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let t = std::f64::consts::PI * (x + 1.0);
    0.42 - 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
}
