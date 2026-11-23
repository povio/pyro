//! Packet input for WebCodecs. The caller owns the timestamp/duration map and
//! gives each access unit a tag so reordered output keeps its input metadata.
use super::{Decoded, Decoder};

const MAX_PACKET_BYTES: usize = 16 << 20;
const MAX_CONFIG_BYTES: usize = 1 << 20;
const MAX_UNITS: usize = 4096;
const MAX_PICTURE_PIXELS: u32 = 4096 * 2304;
const MAX_OUTPUT_PIXELS: u64 = 32 << 20;

#[derive(Clone, Debug)]
pub struct PacketConfig(Format);

#[derive(Clone, Debug)]
enum Format {
    Av1(Option<Av1Color>),
    Avc { length_size: usize, parameter_sets: Vec<u8>, limited_bt709: bool },
}

/// Explicit SDR codec parameters used when the AV1 sequence leaves a
/// transfer or matrix unspecified. A signaled bitstream value takes precedence.
#[derive(Clone, Copy, Debug)]
pub(super) struct Av1Color {
    pub srgb_transfer: bool,
    pub bt709_matrix: bool,
}

pub enum PacketPixels {
    Rgba(Vec<u8>),
    I420LimitedBt709(Vec<u8>),
}

pub struct PacketFrame {
    pub tag: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: PacketPixels,
}

pub struct PacketDecoder {
    config: PacketConfig,
    backend: Option<Box<dyn Decoder>>,
}

fn decimal(field: &str, digits: usize) -> Option<u8> {
    if field.len() != digits || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

// AV1 codec parameter syntax. This bounded path supports Main profile
// 8- and 10-bit streams, including monochrome and subsampled 4:2:0.
fn av1_codec(codec: &str) -> Option<Format> {
    let mut fields = codec.strip_prefix("av01.")?.split('.');
    if fields.next()? != "0" {
        return None;
    }
    let level_tier = fields.next()?;
    let bytes = level_tier.as_bytes();
    if bytes.len() != 3 || !bytes[..2].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let level = decimal(&level_tier[..2], 2)?;
    if level > 23 || !matches!(bytes[2], b'M' | b'H') || (bytes[2] == b'H' && level < 8) {
        return None;
    }
    if !matches!(fields.next()?, "08" | "10") {
        return None;
    }
    let color = if let Some(mono) = fields.next() {
        if !matches!(mono, "0" | "1") || !matches!(fields.next()?, "110" | "111" | "112") {
            return None;
        }
        // The packet output is sRGB. Only advertise declared primaries
        // matching sRGB, transfer curves we convert, and supported matrices.
        if fields.next()? != "01" {
            return None;
        }
        let srgb_transfer = match fields.next()? {
            "01" => false,
            "13" => true,
            _ => return None,
        };
        let bt709_matrix = match fields.next()? {
            "01" => true,
            "05" | "06" => false,
            _ => return None,
        };
        if !matches!(fields.next()?, "0" | "1") || fields.next().is_some() {
            return None;
        }
        Some(Av1Color { srgb_transfer, bt709_matrix })
    } else {
        None
    };
    Some(Format::Av1(color))
}

impl PacketConfig {
    /// Returns the supported configuration for an AV1 low-overhead temporal
    /// unit or H.264 access unit. Without an AVC description, chunks use
    /// Annex B start codes. AV1 carries its sequence header in the stream.
    pub fn parse(codec: &str, description: Option<&[u8]>) -> Option<Self> {
        if codec.starts_with("av01.") {
            return av1_codec(codec).filter(|_| super::AV1).map(Self);
        }
        let hex = codec.strip_prefix("avc1.").or_else(|| codec.strip_prefix("avc3."))?;
        if !super::H264 || hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let profile = u8::from_str_radix(&hex[..2], 16).ok()?;
        let level = u8::from_str_radix(&hex[4..], 16).ok()?;
        if !matches!(profile, 66 | 77 | 100) || !matches!(level, 10..=13 | 20..=22 | 30..=32 | 40..=42 | 50..=52) {
            return None;
        }
        let (length_size, parameter_sets) = match description {
            None => (0, Vec::new()),
            Some(config) => {
                if config.len() > MAX_CONFIG_BYTES || config.first() != Some(&1) {
                    return None;
                }
                let (size, sets) = super::demux::avc_config(config)?;
                if size == 3 {
                    return None;
                }
                (size, sets)
            }
        };
        Some(Self(Format::Avc { length_size, parameter_sets, limited_bt709: false }))
    }

    /// Keep native AVC planes for an explicit limited-range BT.709 override.
    pub fn with_limited_bt709(mut self) -> Option<Self> {
        let Format::Avc { limited_bt709, .. } = &mut self.0 else { return None };
        *limited_bt709 = true;
        Some(self)
    }

    fn limited_bt709(&self) -> bool {
        matches!(&self.0, Format::Avc { limited_bt709: true, .. })
    }

    fn backend(&self) -> Option<Box<dyn Decoder>> {
        match &self.0 {
            #[cfg(feature = "av1")]
            Format::Av1(color) => Some(Box::new(super::av1::Av1::open(Some(MAX_PICTURE_PIXELS), *color)?)),
            #[cfg(feature = "h264")]
            Format::Avc { length_size, parameter_sets, limited_bt709 } => {
                Some(Box::new(super::h264::H264::packets(*length_size, parameter_sets.clone(), *limited_bt709)?))
            }
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    pub fn decoder(&self) -> Option<PacketDecoder> {
        Some(PacketDecoder { config: self.clone(), backend: Some(self.backend()?) })
    }

    /// True when the access unit can start decoding from this configuration.
    pub fn is_key(&self, data: &[u8]) -> bool {
        self.inspect(data) == Some(true)
    }

    fn inspect(&self, data: &[u8]) -> Option<bool> {
        if data.is_empty() || data.len() > MAX_PACKET_BYTES {
            return None;
        }
        match &self.0 {
            Format::Av1(_) => av1_unit(data),
            Format::Avc { length_size, parameter_sets, .. } => {
                let mut first_vcl_is_key = None;
                let mut sps = !parameter_sets.is_empty();
                let mut pps = !parameter_sets.is_empty();
                let mut visit = |nal: &[u8]| {
                    let header = *nal.first()?;
                    if header & 0x80 != 0 {
                        return None;
                    }
                    match header & 0x1f {
                        1..=5 if first_vcl_is_key.is_none() => {
                            first_vcl_is_key = Some(header & 0x1f == 5 && sps && pps)
                        }
                        7 => sps = true,
                        8 => pps = true,
                        _ => {}
                    }
                    Some(())
                };
                if *length_size == 0 {
                    annex_b(data, &mut visit)?;
                } else {
                    let mut rest = data;
                    for _ in 0..MAX_UNITS {
                        let (prefix, sample) = rest.split_at_checked(*length_size)?;
                        let len = prefix.iter().fold(0usize, |v, &b| (v << 8) | b as usize);
                        let (nal, tail) = sample.split_at_checked(len)?;
                        visit(nal)?;
                        rest = tail;
                        if rest.is_empty() {
                            break;
                        }
                    }
                    if !rest.is_empty() {
                        return None;
                    }
                }
                Some(first_vcl_is_key == Some(true))
            }
        }
    }
}

// ISO/IEC 14496-10 Annex B: the three-byte start-code prefix may be
// preceded by zero bytes (including the zero_byte of a four-byte prefix).
fn annex_b(mut data: &[u8], visit: &mut impl FnMut(&[u8]) -> Option<()>) -> Option<()> {
    let first = data.windows(3).position(|w| w == [0, 0, 1])?;
    if data[..first].iter().any(|&b| b != 0) {
        return None;
    }
    data = &data[first + 3..];
    for _ in 0..MAX_UNITS {
        let Some(next) = data.windows(3).position(|w| w == [0, 0, 1]) else { return visit(data) };
        visit(&data[..next])?;
        data = &data[next + 3..];
    }
    None
}

// AV1 low-overhead OBU ordering: a random-access temporal unit contains
// a sequence header followed by a KEY_FRAME, including reduced still headers.
fn av1_unit(mut data: &[u8]) -> Option<bool> {
    let mut sequence = None;
    let mut first_frame_key = None;
    for _ in 0..MAX_UNITS {
        let (&header, mut payload) = data.split_first()?;
        if header & 0x81 != 0 {
            return None;
        }
        if header & 4 != 0 {
            payload = payload.get(1..)?;
        }
        let (obu, rest) = if header & 2 == 0 {
            (payload, &[][..])
        } else {
            let mut length = 0u64;
            let mut length_bytes = 0;
            for &b in payload.iter().take(8) {
                length |= u64::from(b & 127) << (7 * length_bytes);
                length_bytes += 1;
                if b & 128 == 0 {
                    break;
                }
            }
            if length_bytes == 0 || payload[length_bytes - 1] & 128 != 0 {
                return None;
            }
            payload[length_bytes..].split_at_checked(usize::try_from(length).ok()?)?
        };
        match (header >> 3) & 15 {
            1 => sequence = Some(*obu.first()? & 8 != 0),
            3 | 6 if first_frame_key.is_none() => {
                // Non-reduced header: show_existing_frame=0, frame_type=KEY_FRAME,
                // show_frame=1. Reduced still pictures imply these values.
                let first = *obu.first()?;
                first_frame_key = Some(sequence.is_some_and(|reduced| reduced || first & 0xf0 == 0x10));
            }
            _ => {}
        }
        data = rest;
        if data.is_empty() {
            return Some(first_frame_key == Some(true));
        }
    }
    None
}

impl PacketDecoder {
    pub fn decode(&mut self, data: &[u8], tag: u64) -> Result<Vec<PacketFrame>, String> {
        self.config.inspect(data).ok_or("Unsupported video access unit framing")?;
        if self.backend.is_none() {
            self.backend = self.config.backend();
        }
        let backend = self.backend.as_mut().ok_or("Could not start video decoder")?;
        collect(self.config.limited_bt709(), |want, out| backend.feed(data, tag, want, out))
    }

    /// Drains delayed pictures in display order. A subsequent decode opens a
    /// fresh codec context; the caller must supply a key access unit first.
    pub fn flush(&mut self) -> Result<Vec<PacketFrame>, String> {
        let Some(mut backend) = self.backend.take() else { return Ok(Vec::new()) };
        collect(self.config.limited_bt709(), |want, out| backend.finish(want, out))
    }
}

fn collect(
    limited_bt709: bool,
    run: impl FnOnce(&mut dyn FnMut(u64, u32, u32) -> bool, &mut Vec<Decoded>) -> Result<(), String>,
) -> Result<Vec<PacketFrame>, String> {
    let mut remaining = MAX_OUTPUT_PIXELS;
    let mut out = Vec::new();
    run(
        &mut |_, width, height| {
            let pixels = u64::from(width) * u64::from(height);
            if pixels == 0 || pixels > u64::from(MAX_PICTURE_PIXELS) || pixels > remaining {
                return false;
            }
            remaining -= pixels;
            true
        },
        &mut out,
    )?;
    out.into_iter()
        .map(|frame| {
            let (width, height, bytes) = frame.image.ok_or("Unsupported video picture or output size")?;
            let pixels = if limited_bt709 { PacketPixels::I420LimitedBt709(bytes) } else { PacketPixels::Rgba(bytes) };
            Ok(PacketFrame { tag: frame.tag, width, height, pixels })
        })
        .collect()
}
