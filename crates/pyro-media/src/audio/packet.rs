//! Stateful decoding of elementary audio packets for WebCodecs. Container
//! parsing stays in the whole-file decoder; packet descriptions supply the
//! headers that a container demuxer would otherwise have extracted.

use symphonia::core::audio::Channels as ChannelLayout;
use symphonia::core::codecs::audio::{well_known::*, AudioCodecId, AudioCodecParameters, AudioDecoder};
use symphonia::core::packet::PacketRef;
use symphonia::core::units::{Duration, Timestamp};

use super::{Channels, Decoded, MAX_CHANNELS, MAX_FRAMES};

/// One output, with timestamp in microseconds and one float plane per channel.
pub struct PacketOutput {
    pub timestamp: i64,
    pub audio: Decoded,
}

#[derive(Clone, Copy)]
enum Codec {
    Pcm { id: AudioCodecId, bytes_per_sample: usize },
    Aac,
    Mp3,
    Flac,
    Vorbis,
    Opus,
}

impl Codec {
    fn parse(name: &str) -> Option<Self> {
        let pcm = |id, bytes_per_sample| Self::Pcm { id, bytes_per_sample };
        Some(match name {
            "pcm-u8" => pcm(CODEC_ID_PCM_U8, 1),
            "pcm-s16" => pcm(CODEC_ID_PCM_S16LE, 2),
            "pcm-s24" => pcm(CODEC_ID_PCM_S24LE, 3),
            "pcm-s32" => pcm(CODEC_ID_PCM_S32LE, 4),
            "pcm-f32" => pcm(CODEC_ID_PCM_F32LE, 4),
            "alaw" => pcm(CODEC_ID_PCM_ALAW, 1),
            "ulaw" => pcm(CODEC_ID_PCM_MULAW, 1),
            "mp4a.40.2" | "mp4a.40.02" | "mp4a.67" => Self::Aac,
            "mp3" | "mp4a.69" | "mp4a.6B" => Self::Mp3,
            "flac" => Self::Flac,
            "vorbis" => Self::Vorbis,
            "opus" => Self::Opus,
            _ => return None,
        })
    }
}

enum Backend {
    Audio(Box<dyn AudioDecoder>),
    // Without a description, each AAC packet has a self-describing ADTS
    // header. Retain the decoder between frames with the same configuration.
    Adts(Option<([u8; 2], Box<dyn AudioDecoder>)>),
    Opus { decoder: opus_pure::OpusMSDecoder, channels: usize, gain: f32, scratch: Vec<f32> },
}

pub struct PacketDecoder {
    backend: Backend,
    pcm_frame_bytes: Option<usize>,
    first_mp3: bool,
    priming_frames: usize,
}

impl PacketDecoder {
    /// Capability query, before initialization with codec-specific headers.
    /// AAC, FLAC, Vorbis and MP3 signal their actual format in the bitstream.
    pub fn supports(codec: &str, sample_rate: u32, channels: u32) -> bool {
        if sample_rate == 0 || channels == 0 {
            return false;
        }
        match Codec::parse(codec) {
            Some(Codec::Pcm { .. }) => channels as usize <= MAX_CHANNELS,
            Some(Codec::Opus) => channels <= 8,
            Some(_) => true,
            None => false,
        }
    }

    pub fn new(codec: &str, sample_rate: u32, channels: u32, description: Option<&[u8]>) -> Result<Self, String> {
        if !Self::supports(codec, sample_rate, channels) {
            return Err("unsupported audio decoder configuration".into());
        }
        let codec = Codec::parse(codec).ok_or("unknown audio codec")?;
        let mut pcm_frame_bytes = None;
        let mut priming_frames = 0;
        let backend = match codec {
            Codec::Pcm { id, bytes_per_sample } => {
                let mut params = AudioCodecParameters::new();
                params
                    .for_codec(id)
                    .with_sample_rate(sample_rate)
                    .with_channels(ChannelLayout::Discrete(channels as u16));
                pcm_frame_bytes = Some(bytes_per_sample * channels as usize);
                Backend::Audio(make_decoder(&params)?)
            }
            Codec::Aac => match description {
                Some(asc) => Backend::Audio(decoder_with_header(CODEC_ID_AAC, asc)?),
                None => Backend::Adts(None),
            },
            Codec::Flac => {
                let data = description.ok_or("FLAC requires a STREAMINFO description")?;
                // The registration includes the fLaC marker and metadata block
                // header. Symphonia expects just the 34-byte STREAMINFO body.
                if data.len() < 42 || &data[..4] != b"fLaC" || data[4] & 0x7f != 0 || data[5..8] != [0, 0, 34] {
                    return Err("invalid FLAC STREAMINFO description".into());
                }
                Backend::Audio(decoder_with_header(CODEC_ID_FLAC, &data[8..42])?)
            }
            Codec::Vorbis => {
                let data = description.ok_or("Vorbis requires a Xiph-laced description")?;
                if data.first() != Some(&2) {
                    return Err("invalid Vorbis header count".into());
                }
                Backend::Audio(decoder_with_header(CODEC_ID_VORBIS, data)?)
            }
            Codec::Mp3 => {
                let mut params = AudioCodecParameters::new();
                params.for_codec(CODEC_ID_MP3);
                Backend::Audio(make_decoder(&params)?)
            }
            Codec::Opus => {
                let (channels, family, gain) = if let Some(head) = description {
                    if head.len() < 19 || &head[..8] != b"OpusHead" || head[8] >> 4 != 0 {
                        return Err("invalid Opus identification header".into());
                    }
                    let channels = usize::from(head[9]);
                    let family = head[18];
                    let layout = opus_pure::ChannelLayout::surround(channels, family).map_err(|e| e.to_string())?;
                    if family != 0
                        && (head.get(19) != Some(&(layout.nb_streams as u8))
                            || head.get(20) != Some(&(layout.nb_coupled_streams as u8))
                            || head.get(21..21 + channels) != Some(layout.mapping.as_slice()))
                    {
                        return Err("unsupported Opus channel mapping".into());
                    }
                    priming_frames = usize::from(u16::from_le_bytes([head[10], head[11]]));
                    let gain_db = f32::from(i16::from_le_bytes([head[16], head[17]])) / 256.0;
                    (channels, family, 10.0_f32.powf(gain_db / 20.0))
                } else {
                    (channels as usize, 0, 1.0)
                };
                let decoder = opus_pure::OpusMSDecoder::new(48_000, channels, family).map_err(|e| e.to_string())?;
                Backend::Opus { decoder, channels, gain, scratch: vec![0.0; 5760 * channels] }
            }
        };
        Ok(Self { backend, pcm_frame_bytes, first_mp3: matches!(codec, Codec::Mp3), priming_frames })
    }

    /// A packet may only prime the decoder's overlap window and emit no
    /// frames. Every output carries its decoded sample rate and channel count.
    pub fn decode(&mut self, timestamp: i64, data: &[u8]) -> Result<Option<PacketOutput>, String> {
        if data.is_empty() {
            return Err("empty audio packet".into());
        }
        if let Some(width) = self.pcm_frame_bytes {
            if data.len() % width != 0 || data.len() / width > MAX_FRAMES {
                return Err("PCM packet does not contain a supported number of complete frames".into());
            }
        }
        let mut audio = match &mut self.backend {
            Backend::Audio(decoder) => decode_symphonia(decoder.as_mut(), data)?,
            Backend::Adts(active) => {
                let (asc, payload) = adts_frame(data)?;
                if active.as_ref().is_none_or(|(config, _)| *config != asc) {
                    *active = Some((asc, decoder_with_header(CODEC_ID_AAC, &asc)?));
                }
                let (_, decoder) = active.as_mut().expect("ADTS configuration established");
                decode_symphonia(decoder.as_mut(), payload)?
            }
            Backend::Opus { decoder, channels, gain, scratch } => {
                let frames = decoder.decode(data, 5760, scratch).map_err(|e| e.to_string())?;
                let pcm = &mut scratch[..frames * *channels];
                if *gain != 1.0 {
                    pcm.iter_mut().for_each(|sample| *sample *= *gain);
                }
                let mut out = Channels::default();
                out.push_interleaved(pcm, *channels);
                Decoded { sample_rate: 48_000, channels: out.0 }
            }
        };
        if std::mem::take(&mut self.first_mp3) {
            if let Some(delay) = mp3_encoder_delay(data) {
                self.priming_frames = delay;
                // A Xing/Info frame is metadata, preceding the audio frames.
                return Ok(None);
            }
        }
        let discard = audio.len().min(self.priming_frames);
        self.priming_frames -= discard;
        if discard == audio.len() {
            return Ok(None);
        }
        for plane in &mut audio.channels {
            plane.drain(..discard);
        }
        let rate = i64::from(audio.sample_rate);
        let micros = (discard as i64 * 1_000_000 + rate / 2) / rate;
        Ok(Some(PacketOutput { timestamp: timestamp.saturating_add(micros), audio }))
    }

    /// These decoders have no delayed PCM to drain. Flush resets overlap
    /// and prediction history before the next key chunk; codec configuration
    /// priming is not reapplied.
    pub fn flush(&mut self) -> Result<(), String> {
        match &mut self.backend {
            Backend::Audio(decoder) => decoder.reset(),
            Backend::Adts(active) => *active = None,
            Backend::Opus { decoder, .. } => decoder.reset_state().map_err(|e| e.to_string())?,
        }
        Ok(())
    }
}

fn make_decoder(params: &AudioCodecParameters) -> Result<Box<dyn AudioDecoder>, String> {
    symphonia::default::get_codecs().make_audio_decoder(params, &Default::default()).map_err(|e| e.to_string())
}

fn decoder_with_header(id: AudioCodecId, header: &[u8]) -> Result<Box<dyn AudioDecoder>, String> {
    let mut params = AudioCodecParameters::new();
    params.for_codec(id).with_extra_data(header.into());
    make_decoder(&params)
}

fn decode_symphonia(decoder: &mut dyn AudioDecoder, data: &[u8]) -> Result<Decoded, String> {
    // These elementary codecs derive the duration from their frames. PTS
    // association is managed by PacketDecoder in WebCodecs microseconds.
    let packet = PacketRef::new(0, Timestamp::ZERO, Duration::ZERO, data);
    let buffer = decoder.decode_ref(&packet).map_err(|e| e.to_string())?;
    let rate = buffer.spec().rate();
    let mut channels = Vec::new();
    buffer.copy_to_vecs_planar::<f32>(&mut channels);
    Ok(Decoded { sample_rate: rate, channels })
}

/// Turn an AAC-LC ADTS frame into its AudioSpecificConfig and raw data block.
fn adts_frame(frame: &[u8]) -> Result<([u8; 2], &[u8]), String> {
    if frame.len() < 7 || frame[0] != 0xff || frame[1] & 0xf6 != 0xf0 {
        return Err("invalid ADTS syncword or layer".into());
    }
    let object_type = (frame[2] >> 6) + 1;
    let frequency = (frame[2] >> 2) & 0xf;
    let channels = ((frame[2] & 1) << 2) | (frame[3] >> 6);
    if object_type != 2 || frequency > 12 || !(1..=2).contains(&channels) || frame[6] & 3 != 0 {
        return Err("unsupported ADTS profile, channels, or block count".into());
    }
    let length = (usize::from(frame[3] & 3) << 11) | (usize::from(frame[4]) << 3) | usize::from(frame[5] >> 5);
    let header_len = if frame[1] & 1 == 0 { 9 } else { 7 };
    if length != frame.len() || length <= header_len {
        return Err("invalid ADTS frame length".into());
    }
    Ok(([(object_type << 3) | (frequency >> 1), (frequency << 7) | (channels << 3)], &frame[header_len..]))
}

/// Read the encoder delay only from an initial Layer III Xing/Info frame with
/// a LAME-compatible extension. The offset depends on MPEG version and mode.
fn mp3_encoder_delay(frame: &[u8]) -> Option<usize> {
    let header = frame.get(..4)?;
    if header[0] != 0xff || header[1] & 0xe6 != 0xe2 {
        return None;
    }
    let mpeg1 = header[1] & 0x18 == 0x18;
    let mono = header[3] >> 6 == 3;
    let side_info = match (mpeg1, mono) {
        (true, false) => 32,
        (false, true) => 9,
        _ => 17,
    };
    let info = frame.get(4 + side_info..)?;
    if !matches!(info.get(..4)?, b"Xing" | b"Info") {
        return None;
    }
    let flags = u32::from_be_bytes(info.get(4..8)?.try_into().ok()?);
    let mut pos = 8;
    for (bit, bytes) in [(1, 4), (2, 4), (4, 100), (8, 4)] {
        if flags & bit != 0 {
            pos += bytes;
        }
    }
    let lame = info.get(pos..pos + 24)?;
    if !matches!(lame.get(..4)?, b"LAME" | b"Lavf" | b"Lavc") {
        return None;
    }
    Some((usize::from(lame[21]) << 4) | usize::from(lame[22] >> 4))
}
