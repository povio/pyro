//! Coded frame processing for fragmented MP4 SourceBuffers. Packets retain
//! the decoder configuration in effect when they were appended.
use std::mem::size_of;
use std::rc::Rc;

use crate::mse::{Error, Event, Mp4Parser, Sample, Track};

const BUFFER_LIMIT: usize = 64 << 20;
const TIME_EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendMode {
    Segments,
    Sequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferError {
    Invalid,
    Unsupported,
    Quota,
}

impl From<Error> for BufferError {
    fn from(e: Error) -> Self {
        match e {
            Error::Invalid => Self::Invalid,
            Error::Unsupported => Self::Unsupported,
            Error::Quota => Self::Quota,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MediaType {
    video: bool,
    audio: bool,
}

fn is_avc(codec: &str) -> bool {
    (codec.starts_with("avc1.") || codec.starts_with("avc3."))
        && crate::video::PacketConfig::parse(codec, None).is_some()
}

fn is_aac(codec: &str) -> bool {
    matches!(codec, "mp4a.40.2" | "mp4a.40.02" | "mp4a.67")
}

impl MediaType {
    pub fn has_audio(&self) -> bool {
        self.audio
    }

    pub fn has_video(&self) -> bool {
        self.video
    }

    pub fn parse(value: &str) -> Option<Self> {
        let mut params = value.split(';');
        let essence = params.next()?.trim();
        let video_container = essence.eq_ignore_ascii_case("video/mp4");
        if !video_container && !essence.eq_ignore_ascii_case("audio/mp4") {
            return None;
        }
        let mut codecs = None;
        for param in params {
            let (name, raw) = param.trim().split_once('=')?;
            let raw = raw.trim();
            let val = if raw.starts_with('"') { raw.strip_prefix('"')?.strip_suffix('"')? } else { raw };
            if name.trim().eq_ignore_ascii_case("codecs") {
                if codecs.replace(val).is_some() {
                    return None;
                }
            }
        }
        // RFC 6381 makes codecs optional for MP4. Without that parameter,
        // defer codec selection to the actual sample entries in the init.
        let Some(codecs) = codecs else {
            return Some(Self { video: video_container && crate::video::H264, audio: true });
        };
        let mut format = Self { video: false, audio: false };
        for codec in codecs.split(',').map(str::trim) {
            if video_container && is_avc(codec) && !format.video {
                format.video = true;
            } else if is_aac(codec) && !format.audio {
                format.audio = true;
            } else {
                return None;
            }
        }
        Some(format)
    }
}

#[derive(Clone)]
pub struct BufferedSample {
    pub sample: Sample,
    pub config: Rc<Track>,
}

pub struct TrackBuffer {
    config: Rc<Track>,
    samples: Vec<BufferedSample>,
    need_key: bool,
    last_decode: Option<(f64, f64)>,
    group_highest: Option<f64>,
    end: f64,
    sample_bytes: usize,
}

impl TrackBuffer {
    fn new(config: Track) -> Self {
        Self {
            config: Rc::new(config),
            samples: Vec::new(),
            need_key: true,
            last_decode: None,
            group_highest: None,
            end: 0.,
            sample_bytes: 0,
        }
    }

    pub fn track(&self) -> &Track {
        &self.config
    }

    pub fn samples(&self) -> &[BufferedSample] {
        &self.samples
    }

    fn reset_group(&mut self) {
        self.need_key = true;
        self.last_decode = None;
        self.group_highest = None;
    }

    fn remove_matching(&mut self, mut remove: impl FnMut(&Sample) -> bool) {
        // The following dependent pictures cannot be decoded without their
        // reference. Follow decode order, including for reordered pictures.
        let mut dependent = false;
        let mut end: f64 = 0.;
        let mut removed = 0;
        self.samples.retain(|stored| {
            let s = &stored.sample;
            dependent = remove(s) || (dependent && !s.key);
            if dependent {
                removed += s.data.len();
                false
            } else {
                end = end.max(s.pts + s.duration);
                true
            }
        });
        self.sample_bytes -= removed;
        if self.samples.capacity() > self.samples.len().saturating_mul(3).max(32) {
            self.samples.shrink_to_fit();
        }
        self.end = end;
    }

    fn remove(&mut self, start: f64, end: f64) {
        let next_key = self
            .samples
            .iter()
            .map(|s| &s.sample)
            .filter(|s| s.key && s.pts >= end)
            .map(|s| s.pts)
            .fold(f64::INFINITY, f64::min);
        self.remove_matching(|s| start <= s.pts && s.pts < next_key);
        if self.last_decode.is_some_and(|(dts, _)| dts >= start && dts < next_key) {
            self.reset_group();
        }
    }

    fn insert(&mut self, sample: Sample) {
        let end = sample.pts + sample.duration;
        let start = self.group_highest.unwrap_or(sample.pts);
        if start < self.end - TIME_EPSILON && self.group_highest.is_none_or(|last| last <= sample.pts + TIME_EPSILON) {
            let first = self.last_decode.is_none();
            let video = self.config.width > 0;
            self.remove_matching(|old| {
                (old.pts + TIME_EPSILON >= start && old.pts + TIME_EPSILON < end)
                    || (first
                        && old.pts < sample.pts
                        && old.pts + old.duration > sample.pts + TIME_EPSILON
                        && (!video || sample.pts - old.pts < 1e-6))
            });
        }
        self.last_decode = Some((sample.dts, sample.duration));
        self.group_highest = Some(self.group_highest.unwrap_or(end).max(end));
        self.end = self.end.max(end);
        self.sample_bytes += sample.data.len();
        let index = self.samples.partition_point(|v| v.sample.dts <= sample.dts);
        self.samples.insert(index, BufferedSample { sample, config: self.config.clone() });
    }

    fn ranges(&self) -> Vec<(f64, f64)> {
        let mut sorted: Vec<_> =
            self.samples.iter().map(|s| (s.sample.pts, s.sample.pts + s.sample.duration)).collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut ranges: Vec<(f64, f64)> = Vec::new();
        for (start, end) in sorted {
            if let Some(last) = ranges.last_mut().filter(|r| start <= r.1 + TIME_EPSILON) {
                last.1 = last.1.max(end);
            } else {
                ranges.push((start, end));
            }
        }
        ranges
    }
}

pub struct CodedBuffer {
    parser: Mp4Parser,
    media_type: MediaType,
    tracks: Vec<TrackBuffer>,
    mode: AppendMode,
    offset: f64,
    window: (f64, f64),
    group_start: Option<f64>,
    group_end: f64,
    configs: Vec<Rc<Track>>,
    pending_init: bool,
    first_init_duration: Option<f64>,
}

impl CodedBuffer {
    pub fn new(media_type: MediaType) -> Self {
        Self {
            parser: Mp4Parser::default(),
            media_type,
            tracks: Vec::new(),
            mode: AppendMode::Segments,
            offset: 0.,
            window: (0., f64::INFINITY),
            group_start: None,
            group_end: 0.,
            configs: Vec::new(),
            pending_init: false,
            first_init_duration: None,
        }
    }

    pub fn track_buffers(&self) -> &[TrackBuffer] {
        &self.tracks
    }
    pub fn timestamp_offset(&self) -> f64 {
        self.offset
    }
    pub fn initialized(&self) -> bool {
        !self.tracks.is_empty()
    }
    pub fn init_duration(&self) -> Option<f64> {
        self.first_init_duration
    }
    pub fn partial_segment(&self) -> bool {
        self.parser.partial_segment()
    }
    pub fn highest_end(&self) -> f64 {
        self.tracks.iter().map(|t| t.end).fold(0., f64::max)
    }
    pub fn highest_start(&self) -> Option<f64> {
        self.tracks.iter().flat_map(|t| &t.samples).map(|v| v.sample.pts).max_by(f64::total_cmp)
    }
    pub fn set_window(&mut self, start: f64, end: f64) {
        self.window = (start, end);
    }
    pub fn set_mode(&mut self, mode: AppendMode) {
        self.mode = mode;
        if mode == AppendMode::Sequence {
            self.group_start = Some(self.group_end);
        }
    }
    pub fn set_timestamp_offset(&mut self, value: f64) {
        self.offset = value;
        if self.mode == AppendMode::Sequence {
            self.group_start = Some(value);
        }
    }

    pub fn reset_parser(&mut self) {
        self.parser.reset();
        for track in &mut self.tracks {
            track.reset_group();
        }
        if self.mode == AppendMode::Sequence {
            self.group_start = Some(self.group_end);
        }
    }
    pub fn change_type(&mut self, media_type: MediaType) {
        self.reset_parser();
        self.media_type = media_type;
        self.pending_init = true;
    }
    pub fn can_append(&self, bytes: usize) -> bool {
        self.usage().saturating_add(bytes) <= BUFFER_LIMIT
    }
    fn usage(&self) -> usize {
        let frames: usize =
            self.tracks.iter().map(|t| t.sample_bytes + t.samples.len() * size_of::<BufferedSample>()).sum();
        let configs: usize =
            self.configs.iter().map(|c| c.description.len() + c.codec.len() + size_of::<Track>()).sum();
        self.parser.buffered_bytes() + frames + configs
    }
    pub fn append(&mut self, data: &[u8]) -> Result<bool, BufferError> {
        if !self.can_append(data.len()) {
            return Err(BufferError::Quota);
        }
        let result = self.process(data);
        if result.is_err() {
            self.reset_parser();
        }
        result
    }
    fn process(&mut self, data: &[u8]) -> Result<bool, BufferError> {
        let events = self.parser.append(data)?;
        let mut initialized = false;
        for event in events {
            match event {
                Event::Init(tracks, duration) => {
                    self.pending_init = true;
                    self.configure(tracks, duration)?;
                    initialized = true;
                }
                Event::Samples(samples) => {
                    if !self.initialized() || self.pending_init {
                        return Err(BufferError::Invalid);
                    }
                    let bytes = samples.iter().map(|s| s.data.len() + size_of::<BufferedSample>()).sum();
                    if !self.can_append(bytes) {
                        return Err(BufferError::Quota);
                    }
                    for sample in samples {
                        self.process_sample(sample)?;
                    }
                }
            }
        }
        self.prune_configs();
        Ok(initialized)
    }
    fn configure(&mut self, tracks: Vec<Track>, duration: Option<f64>) -> Result<(), BufferError> {
        let mut audio = 0;
        let mut video = 0;
        for t in &tracks {
            if t.width != 0 {
                video += 1;
                if !self.media_type.video
                    || !is_avc(&t.codec)
                    || crate::video::PacketConfig::parse(&t.codec, Some(&t.description)).is_none()
                {
                    return Err(BufferError::Unsupported);
                }
            } else {
                audio += 1;
                if !self.media_type.audio
                    || !is_aac(&t.codec)
                    || crate::audio::packet::PacketDecoder::new(
                        &t.codec,
                        t.sample_rate,
                        t.channels,
                        Some(&t.description),
                    )
                    .is_err()
                {
                    return Err(BufferError::Unsupported);
                }
            }
        }
        if tracks.is_empty() || audio > 1 || video > 1 {
            return Err(BufferError::Unsupported);
        }
        if self.initialized() {
            if tracks.len() != self.tracks.len()
                || tracks.iter().any(|t| !self.tracks.iter().any(|prev| (prev.config.width > 0) == (t.width > 0)))
            {
                return Err(BufferError::Invalid);
            }
            for cfg in tracks {
                let track = self.tracks.iter_mut().find(|t| (t.config.width > 0) == (cfg.width > 0)).unwrap();
                track.config = Rc::new(cfg);
                track.need_key = true;
                self.configs.push(track.config.clone());
            }
        } else {
            self.first_init_duration = duration;
            self.tracks = tracks.into_iter().map(TrackBuffer::new).collect();
            self.configs.extend(self.tracks.iter().map(|t| t.config.clone()));
        }
        self.pending_init = false;
        self.prune_configs();
        Ok(())
    }
    fn prune_configs(&mut self) {
        self.configs.retain(|c| Rc::strong_count(c) > 1);
    }

    fn process_sample(&mut self, mut s: Sample) -> Result<(), BufferError> {
        if !s.pts.is_finite() || !s.dts.is_finite() || !s.duration.is_finite() || s.duration <= 0. {
            return Err(BufferError::Invalid);
        }
        let i = self.tracks.iter().position(|t| t.config.id == s.track_id).ok_or(BufferError::Invalid)?;
        loop {
            if self.mode == AppendMode::Sequence {
                if let Some(start) = self.group_start.take() {
                    self.offset = start - s.pts;
                    self.group_end = start;
                    for track in &mut self.tracks {
                        track.need_key = true;
                    }
                }
            }
            let dts = s.dts + self.offset;
            if !self.tracks[i]
                .last_decode
                .is_some_and(|(last, dur)| dts + TIME_EPSILON < last || dts - last > 2. * dur + TIME_EPSILON)
            {
                break;
            }
            match self.mode {
                AppendMode::Sequence => self.group_start = Some(self.group_end),
                AppendMode::Segments => self.group_end = s.pts + self.offset,
            }
            for track in &mut self.tracks {
                track.reset_group();
            }
        }
        s.dts += self.offset;
        s.pts += self.offset;
        let end = s.pts + s.duration;
        if !s.dts.is_finite() || !s.pts.is_finite() || !end.is_finite() {
            return Err(BufferError::Invalid);
        }
        let track = &mut self.tracks[i];
        if s.pts < self.window.0 || end > self.window.1 {
            track.need_key = true;
            return Ok(());
        }
        if track.need_key && !s.key {
            return Ok(());
        }
        track.need_key = false;
        // Parser samples are slices of an mdat box. Keep just this access
        // unit so removing the others actually frees their quota and bytes.
        s.data = bytes::Bytes::copy_from_slice(&s.data);
        track.insert(s);
        self.group_end = self.group_end.max(end);
        Ok(())
    }

    pub fn remove(&mut self, start: f64, end: f64) {
        for track in &mut self.tracks {
            track.remove(start, end);
        }
        self.prune_configs();
    }
    pub fn ranges(&self, ended: bool) -> Vec<(f64, f64)> {
        let highest = self.highest_end();
        let mut intersection: Vec<(f64, f64)> = vec![(0., highest)];
        for track in &self.tracks {
            let mut ranges = track.ranges();
            if ended {
                if let Some(last) = ranges.last_mut() {
                    last.1 = highest;
                }
            }
            let mut out = Vec::new();
            let (mut a, mut b) = (0, 0);
            while a < intersection.len() && b < ranges.len() {
                let (x, y) = intersection[a];
                let (u, v) = ranges[b];
                if x.max(u) < y.min(v) {
                    out.push((x.max(u), y.min(v)));
                }
                if y < v {
                    a += 1;
                } else {
                    b += 1;
                }
            }
            intersection = out;
        }
        if self.tracks.is_empty() {
            Vec::new()
        } else {
            intersection
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn setup() -> CodedBuffer {
        let mut b = CodedBuffer::new(MediaType { video: true, audio: false });
        b.tracks.push(TrackBuffer::new(Track {
            id: 1,
            codec: "avc1.42001e".into(),
            description: Bytes::new(),
            timescale: 1000,
            width: 64,
            height: 48,
            sample_rate: 0,
            channels: 0,
        }));
        b
    }

    fn sample(time: f64, key: bool, marker: u8) -> Sample {
        Sample { track_id: 1, dts: time, pts: time, duration: 1., key, data: Bytes::from(vec![marker]) }
    }

    #[test]
    fn removing_a_reference_discards_dependent_samples_and_preserves_the_next_key() {
        let mut b = setup();
        for time in 0..6 {
            b.process_sample(sample(time as f64, time % 3 == 0, time)).unwrap();
        }
        b.remove(1., 2.);
        assert_eq!(b.ranges(false), vec![(0., 1.), (3., 6.)]);
        let packets = b.tracks[0].samples().iter().map(|s| s.sample.data[0]).collect::<Vec<_>>();
        assert_eq!(packets, [0, 3, 4, 5]);
        assert_eq!(b.tracks[0].sample_bytes, 4);
    }

    #[test]
    fn sequence_shift_and_append_window_require_a_new_random_access_point() {
        let mut b = setup();
        b.set_mode(AppendMode::Sequence);
        b.set_timestamp_offset(10.);
        b.set_window(11., 15.);
        b.process_sample(sample(7., true, 1)).unwrap(); // mapped to 10: filtered
        b.process_sample(sample(8., false, 2)).unwrap(); // depends on rejected key
        b.process_sample(sample(9., true, 3)).unwrap();
        b.process_sample(sample(10., false, 4)).unwrap();
        assert_eq!(b.ranges(false), [(12., 14.)]);
        b.reset_parser();
        b.process_sample(sample(0., true, 5)).unwrap();
        assert_eq!(b.timestamp_offset(), 14.);
        assert_eq!(b.ranges(false), [(12., 15.)]);
    }

    #[test]
    fn replacement_interprets_frames_in_decode_order() {
        let mut b = setup();
        for (d, p, key) in [(0., 0., true), (1., 2., false), (2., 1., false), (3., 3., true)] {
            let mut s = sample(d, key, 1);
            s.pts = p;
            b.process_sample(s).unwrap();
        }
        assert_eq!(b.ranges(false), [(0., 4.)]);
        b.reset_parser();
        b.process_sample(sample(0., true, 2)).unwrap();
        assert_eq!(b.ranges(false), [(0., 1.), (3., 4.)]);
        assert_eq!(b.tracks[0].samples()[0].sample.data[0], 2);
    }

    #[test]
    fn bare_mp4_types_defer_to_the_initialization_segment() {
        let tone = include_bytes!("../tests/fixtures/fragmented-tone.mp4");
        for mime in ["audio/mp4", "video/mp4"] {
            let mut buffer = CodedBuffer::new(MediaType::parse(mime).unwrap());
            assert!(buffer.append(tone).unwrap());
            assert_eq!(buffer.track_buffers().len(), 1);
            let track = &buffer.track_buffers()[0];
            assert_eq!(track.track().codec, "mp4a.40.2");
            assert!(!track.samples().is_empty());
        }
        for mime in [
            "audio/mp4;codecs=",
            "video/mp4;codecs=\"\"",
            "video/mp4;codecs=avc1.4D4001",
            "audio/mp4;codecs=avc1.42001e",
            "video/mp4;codecs=mp4a.40.5",
            "video/webm",
        ] {
            assert!(MediaType::parse(mime).is_none(), "{mime}");
        }
        let mut audio = CodedBuffer::new(MediaType::parse("audio/mp4").unwrap());
        let input = include_bytes!("../tests/fixtures/fragmented-tone-pattern.mp4");
        assert_eq!(audio.append(input), Err(BufferError::Unsupported));
        assert!(!audio.initialized());
    }

    #[cfg(feature = "h264")]
    #[test]
    fn bare_video_mp4_accepts_supported_muxed_audio_and_video() {
        let mut buffer = CodedBuffer::new(MediaType::parse("video/mp4").unwrap());
        let input = include_bytes!("../tests/fixtures/fragmented-tone-pattern.mp4");
        assert!(buffer.append(input).unwrap());
        assert_eq!(buffer.track_buffers().len(), 2);
        assert!(buffer.track_buffers().iter().all(|t| !t.samples().is_empty()));
    }

    #[cfg(feature = "h264")]
    #[test]
    fn duration_uses_first_accepted_init_in_combined_or_separate_appends() {
        // Copy the real fixture's ftyp and moov, varying only the v0 mvhd duration.
        let bytes = include_bytes!("../tests/fixtures/fragmented-tone-pattern.mp4");
        let u32_at = |at| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        let moov = u32_at(0) as usize;
        let init_end = moov + u32_at(moov) as usize;
        let mvhd = moov + 8;
        assert_eq!(&bytes[mvhd + 4..mvhd + 8], b"mvhd");
        assert_eq!(bytes[mvhd + 8], 0);
        let scale = u32_at(mvhd + 20);
        let init = |seconds: u32| {
            let mut result = bytes[..init_end].to_vec();
            result[mvhd + 24..mvhd + 28].copy_from_slice(&(seconds * scale).to_be_bytes());
            result
        };
        for (first, expected) in [(0, None), (7, Some(7.))] {
            for combined in [true, false] {
                let ty = MediaType::parse("video/mp4; codecs=\"avc1.64000a,mp4a.40.2\"").unwrap();
                let mut buffer = CodedBuffer::new(ty.clone());
                if combined {
                    buffer.append(&[init(first), init(12)].concat()).unwrap();
                } else {
                    buffer.append(&init(first)).unwrap();
                    buffer.change_type(ty);
                    buffer.append(&init(12)).unwrap();
                }
                assert!(buffer.initialized());
                assert_eq!(buffer.init_duration(), expected);
            }
        }
    }

    #[cfg(feature = "h264")]
    #[test]
    fn incremental_muxed_append_retains_real_video_and_audio_packets() {
        let mut b = CodedBuffer::new(MediaType::parse("video/mp4; codecs=\"avc1.64000a,mp4a.40.2\"").unwrap());
        let bytes = include_bytes!("../tests/fixtures/fragmented-tone-pattern.mp4");
        for part in bytes.chunks(131) {
            b.append(part).unwrap();
        }
        assert_eq!(b.track_buffers().len(), 2);
        for t in b.track_buffers() {
            assert!(!t.track().description.is_empty());
            assert!(t.samples().len() > 1);
            assert!(t.samples().iter().all(|s| !s.sample.data.is_empty() && s.sample.duration > 0.));
        }
        let ranges = b.ranges(false);
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].1 > 0.50 && ranges[0].1 < 0.54);
        b.remove(0., f64::INFINITY);
        assert!(b.ranges(true).is_empty());
        assert!(b.track_buffers().iter().all(|t| t.samples().is_empty()));
    }
}
