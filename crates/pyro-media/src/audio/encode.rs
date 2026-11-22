//! Streaming WebCodecs audio encoding with the shipped Opus implementation.
//! An input AudioData need not be one codec packet. Sample history, fractional
//! resampling phase, and incomplete packet data live across encode calls.

use bytes::Bytes;
use opus_pure::{Application, Bandwidth, OpusHead, OpusMSEncoder, RateControl, Repacketizer, Signal};

pub struct EncoderConfig {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: Option<u32>,
    pub constant_bitrate: bool,
    pub frame_duration: u32,
    pub complexity: u8,
    pub packet_loss: u8,
    pub fec: bool,
    pub dtx: bool,
    pub signal: String,
    pub application: String,
    pub format: String,
}

pub struct EncodedPacket {
    pub timestamp: i64,
    pub duration: u64,
    pub data: Bytes,
}

fn native_duration(us: u32) -> bool {
    matches!(us, 2500 | 5000 | 10000 | 20000 | 40000 | 60000 | 80000 | 100000 | 120000)
}

pub struct PacketEncoder {
    encoder: OpusMSEncoder,
    description: Vec<u8>,
    input_rate: u32,
    channels: usize,
    duration: u32,
    packet_frames: usize,
    subframe_frames: usize,
    dtx: bool,
    resampler: Option<Resampler>,
    pending: Vec<f32>,
    scratch: Vec<u8>,
    framer: Repacketizer,
    origin: Option<i64>,
    input_frames: u64,
    packet_index: u64,
}

impl PacketEncoder {
    pub fn supports(config: &EncoderConfig) -> bool {
        // Bound input resampling rates to the normal audio range supported by
        // this encoder. The Opus coding clock itself always uses an Opus rate.
        config.codec == "opus"
            && (3000..=384000).contains(&config.sample_rate)
            && (1..=8).contains(&config.channels)
            && matches!(config.format.as_str(), "opus" | "ogg")
            && (config.channels <= 2 || config.format == "ogg")
            && matches!(config.application.as_str(), "audio" | "voip" | "lowdelay")
            && matches!(config.signal.as_str(), "auto" | "voice" | "music")
            && config.complexity <= 10
            && config.packet_loss <= 100
            && (2500..=120000).contains(&config.frame_duration)
            && config.frame_duration % 2500 == 0
            // Multistream framing has separate TOCs. Only single-stream
            // packets can use the elementary repacketizer below.
            && (config.channels <= 2 || native_duration(config.frame_duration))
            && (!config.dtx || config.channels <= 2)
            && config.bitrate.is_none_or(|rate| (500..=512000 * config.channels).contains(&rate))
    }

    pub fn new(config: &EncoderConfig) -> Result<Self, String> {
        if !Self::supports(config) {
            return Err("unsupported audio encoder configuration".into());
        }
        let rate = match config.sample_rate {
            8000 | 12000 | 16000 | 24000 | 48000 => config.sample_rate,
            _ => 48000,
        };
        let channels = config.channels as usize;
        let family = u8::from(channels > 2);
        let application = match config.application.as_str() {
            "voip" => Application::Voip,
            "lowdelay" => Application::RestrictedLowDelay,
            _ => Application::Audio,
        };
        let mut encoder = OpusMSEncoder::new(rate as i32, channels, family, application).map_err(|e| e.to_string())?;
        if let Some(bitrate) = config.bitrate {
            encoder.set_bitrate(bitrate as i32);
        }
        let sub_duration = if native_duration(config.frame_duration) {
            config.frame_duration
        } else if config.frame_duration % 5000 == 0 {
            5000
        } else {
            2500
        };
        for stream in encoder.streams_mut() {
            stream.rate_control = if config.constant_bitrate { RateControl::Cbr } else { RateControl::Vbr };
            stream.complexity = i32::from(config.complexity);
            stream.use_dtx = config.dtx;
            stream.use_inband_fec = config.fec;
            stream.packet_loss_perc = i32::from(config.packet_loss);
            stream.signal_type = match config.signal.as_str() {
                "voice" => Some(Signal::Voice),
                "music" => Some(Signal::Music),
                _ => None,
            };
            if sub_duration != config.frame_duration {
                // A single Opus packet has one TOC configuration. These short
                // CELT frames must keep the same bandwidth across the packet.
                stream.force_bandwidth = Some(match rate {
                    8000 => Bandwidth::Narrowband,
                    12000 | 16000 => Bandwidth::Wideband,
                    24000 => Bandwidth::Superwideband,
                    _ => Bandwidth::Fullband,
                });
            }
        }
        let head = OpusHead::for_ms_encoder(&encoder, config.sample_rate);
        let mut description = Vec::with_capacity(21 + channels);
        description.extend_from_slice(b"OpusHead");
        description.extend_from_slice(&[1, head.channel_count]);
        description.extend_from_slice(&head.pre_skip.to_le_bytes());
        description.extend_from_slice(&head.input_sample_rate.to_le_bytes());
        description.extend_from_slice(&head.output_gain_q8.to_le_bytes());
        description.push(head.mapping_family);
        if family != 0 {
            description.extend_from_slice(&[head.stream_count, head.coupled_count]);
            description.extend_from_slice(&head.channel_mapping);
        }
        let frames = |us| (u64::from(rate) * u64::from(us) / 1_000_000) as usize;
        let packet_frames = frames(config.frame_duration);
        let subframe_frames = frames(sub_duration);
        // The shipped multistream encoder uses up to (1500 + frames) bytes
        // per stream plus its self-delimited length field.
        let buffer_size = encoder.nb_streams() * (1500 + subframe_frames + 2);
        Ok(Self {
            encoder,
            description,
            input_rate: config.sample_rate,
            channels,
            duration: config.frame_duration,
            packet_frames,
            subframe_frames,
            dtx: config.dtx,
            resampler: (rate != config.sample_rate).then(|| Resampler::new(config.sample_rate, rate, channels)),
            pending: Vec::new(),
            scratch: vec![0; buffer_size],
            framer: Repacketizer::new(),
            origin: None,
            input_frames: 0,
            packet_index: 0,
        })
    }

    pub fn description(&self) -> Option<&[u8]> {
        Some(&self.description)
    }

    /// Consume complete interleaved PCM frames. Start a new timed segment
    /// when the caller supplies a real discontinuity (allowing quantization
    /// to integer microseconds at ordinary AudioData boundaries).
    pub fn encode(
        &mut self,
        timestamp: i64,
        rate: u32,
        channels: u32,
        pcm: &[f32],
    ) -> Result<Vec<EncodedPacket>, String> {
        if rate != self.input_rate
            || channels as usize != self.channels
            || pcm.is_empty()
            || pcm.len() % self.channels != 0
        {
            return Err("audio input does not match the encoder configuration".into());
        }
        let mut packets = Vec::new();
        if let Some(origin) = self.origin {
            let elapsed = i128::from(self.input_frames) * 1_000_000 / i128::from(self.input_rate);
            if (i128::from(timestamp) - i128::from(origin) - elapsed).abs() > 1 {
                packets = self.flush()?;
            }
        }
        self.origin.get_or_insert(timestamp);
        self.input_frames =
            self.input_frames.checked_add((pcm.len() / self.channels) as u64).ok_or("audio stream is too long")?;
        for block in pcm.chunks(1024 * self.channels) {
            if let Some(resampler) = &mut self.resampler {
                resampler.push(block, false, &mut self.pending);
            } else {
                self.pending.extend_from_slice(block);
            }
            self.emit(&mut packets)?;
        }
        Ok(packets)
    }

    /// Drain the resampler and enough padding to recover the encoder’s
    /// lookahead after OpusHead pre-skip. An empty flush emits nothing.
    pub fn flush(&mut self) -> Result<Vec<EncodedPacket>, String> {
        if self.origin.is_none() {
            return Ok(Vec::new());
        }
        if let Some(resampler) = &mut self.resampler {
            resampler.push(&[], true, &mut self.pending);
            resampler.reset();
        }
        let frames = self.pending.len() / self.channels + self.encoder.streams()[0].lookahead();
        let padded = frames.div_ceil(self.packet_frames) * self.packet_frames;
        self.pending.resize(padded * self.channels, 0.0);
        let mut packets = Vec::new();
        self.emit(&mut packets)?;
        self.encoder.reset_state().map_err(|e| e.to_string())?;
        self.input_frames = 0;
        self.packet_index = 0;
        self.origin = None;
        Ok(packets)
    }

    fn emit(&mut self, outputs: &mut Vec<EncodedPacket>) -> Result<(), String> {
        let stride = self.packet_frames * self.channels;
        let mut consumed = 0;
        while let Some(samples) = self.pending.get(consumed..consumed + stride) {
            let packet = if self.packet_frames == self.subframe_frames {
                let n =
                    self.encoder.encode(samples, self.packet_frames, &mut self.scratch).map_err(|e| e.to_string())?;
                if self.dtx && n == 1 {
                    // A TOC-only packet is an actual codec DTX indication.
                    // Skip its transmission while still advancing media time.
                    None
                } else {
                    Some(Bytes::copy_from_slice(&self.scratch[..n]))
                }
            } else {
                self.framer.clear();
                let mut all_dtx = self.dtx;
                for part in samples.chunks_exact(self.subframe_frames * self.channels) {
                    let n = self
                        .encoder
                        .encode(part, self.subframe_frames, &mut self.scratch)
                        .map_err(|e| e.to_string())?;
                    all_dtx &= n == 1;
                    self.framer.cat(&self.scratch[..n]).map_err(|e| e.to_string())?;
                }
                if all_dtx { None } else { Some(Bytes::from(self.framer.out().map_err(|e| e.to_string())?)) }
            };
            if let Some(data) = packet {
                let time = i128::from(self.origin.expect("timed input"))
                    + i128::from(self.packet_index) * i128::from(self.duration);
                let timestamp = i64::try_from(time).map_err(|_| "encoded timestamp out of range")?;
                outputs.push(EncodedPacket { timestamp, duration: u64::from(self.duration), data });
            }
            self.packet_index += 1;
            consumed += stride;
        }
        self.pending.drain(..consumed);
        Ok(())
    }
}

/// Windowed-sinc conversion on a continuous input timeline. Keep the filter
/// wing across calls so splitting an AudioData does not reset phase or emit
/// edge transients. Downsampling narrows the passband before decimation.
struct Resampler {
    from: u32,
    to: u32,
    channels: usize,
    half: i64,
    weights: Vec<f32>,
    data: Vec<f32>,
    first: u64,
    received: u64,
    produced: u64,
}

impl Resampler {
    const PHASES: usize = 256;

    fn new(from: u32, to: u32, channels: usize) -> Self {
        let cutoff = (f64::from(to) / f64::from(from)).min(1.0);
        let half = (16.0 / cutoff).ceil() as i64;
        let mut weights = Vec::with_capacity(Self::PHASES * (2 * half) as usize);
        for phase in 0..Self::PHASES {
            let fraction = phase as f64 / Self::PHASES as f64;
            for offset in 1 - half..=half {
                let x = fraction - offset as f64;
                weights.push((cutoff * super::sinc(cutoff * x) * super::window(x / half as f64)) as f32);
            }
        }
        Self { from, to, channels, half, weights, data: Vec::new(), first: 0, received: 0, produced: 0 }
    }

    fn push(&mut self, input: &[f32], ending: bool, dst: &mut Vec<f32>) {
        self.data.extend_from_slice(input);
        self.received += (input.len() / self.channels) as u64;
        let rate = u128::from(self.to);
        let target = u128::from(self.received) * rate / u128::from(self.from);
        while u128::from(self.produced) < target {
            let t = u128::from(self.produced) * u128::from(self.from);
            let center = (t / rate) as i64;
            if !ending && center + self.half >= self.received as i64 {
                break;
            }
            let phase = ((t % rate) * Self::PHASES as u128 / rate) as usize;
            let taps = (2 * self.half) as usize;
            let weights = &self.weights[phase * taps..(phase + 1) * taps];
            let mut sum = [0.0f64; 8];
            let mut norm = 0.0f64;
            for (i, weight) in weights.iter().enumerate() {
                let frame = center + 1 - self.half + i as i64;
                if frame < self.first as i64 || frame >= self.received as i64 {
                    continue;
                }
                let begin = (frame as u64 - self.first) as usize * self.channels;
                let samples = &self.data[begin..begin + self.channels];
                let weight = f64::from(*weight);
                for (out, x) in sum.iter_mut().zip(samples) {
                    *out += f64::from(*x) * weight;
                }
                norm += weight;
            }
            dst.extend(sum[..self.channels].iter().map(|&v| if norm != 0.0 { (v / norm) as f32 } else { 0.0 }));
            self.produced += 1;
        }
        let next = (u128::from(self.produced) * u128::from(self.from) / rate) as u64;
        let keep = next.saturating_sub(self.half as u64).min(self.received);
        self.data.drain(..((keep - self.first) as usize * self.channels));
        self.first = keep;
    }

    fn reset(&mut self) {
        self.data.clear();
        self.first = 0;
        self.received = 0;
        self.produced = 0;
    }
}
