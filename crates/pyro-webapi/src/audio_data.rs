//! Immutable WebCodecs audio samples. A wrapper can close independently of
//! its clones and keeps its original timestamp after releasing the samples.
use std::cell::RefCell;

use bytes::Bytes;
use pyro_js::{ClassDef, Engine, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::encoding::{buffer_source_bytes, encoding_helper, uint8_array};
use crate::page::{AgentCluster, PageDom};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    U8,
    S16,
    S32,
    F32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Format {
    encoding: Encoding,
    planar: bool,
}

impl Format {
    fn parse(name: &str) -> JsResult<Self> {
        let base = name.strip_suffix("-planar");
        let encoding = match base.unwrap_or(name) {
            "u8" => Encoding::U8,
            "s16" => Encoding::S16,
            "s32" => Encoding::S32,
            "f32" => Encoding::F32,
            _ => return Err(JsError::Type("Unknown AudioSampleFormat".into())),
        };
        Ok(Self { encoding, planar: base.is_some() })
    }

    fn name(self) -> &'static str {
        match (self.encoding, self.planar) {
            (Encoding::U8, false) => "u8",
            (Encoding::U8, true) => "u8-planar",
            (Encoding::S16, false) => "s16",
            (Encoding::S16, true) => "s16-planar",
            (Encoding::S32, false) => "s32",
            (Encoding::S32, true) => "s32-planar",
            (Encoding::F32, false) => "f32",
            (Encoding::F32, true) => "f32-planar",
        }
    }

    fn width(self) -> usize {
        match self.encoding {
            Encoding::U8 => 1,
            Encoding::S16 => 2,
            Encoding::S32 | Encoding::F32 => 4,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AudioResource {
    format: Format,
    rate: f32,
    channels: u32,
    frames: u32,
    timestamp: i64,
    bytes: Bytes,
    cluster: AgentCluster,
}

impl AudioResource {
    pub(crate) fn byte_length(&self) -> usize {
        self.bytes.len()
    }
    pub(crate) fn readable_in(&self, page: &PageDom) -> bool {
        self.cluster == page.agent_cluster()
    }

    pub(crate) fn encode_packets(
        &self, encoder: &mut pyro_media::audio::encode::PacketEncoder,
    ) -> Result<Vec<pyro_media::audio::encode::EncodedPacket>, String> {
        if self.rate.fract() != 0.0 || f64::from(self.rate) > f64::from(u32::MAX) {
            return Err("Unsupported input sample rate".into());
        }
        let mut samples = Vec::new();
        let count = self.bytes.len() / self.format.width();
        samples.try_reserve_exact(count).map_err(|_| "Audio sample conversion is too large")?;
        for frame in 0..self.frames as usize {
            for channel in 0..self.channels as usize {
                let offset = self.sample_offset(frame, channel);
                let sample = &self.bytes[offset..offset + self.format.width()];
                samples.push(read_sample(sample, self.format.encoding) as f32);
            }
        }
        encoder.encode(self.timestamp, self.rate as u32, self.channels, &samples)
    }

    fn layout(&self, options: CopyOptions) -> JsResult<Layout> {
        let format = options.format.unwrap_or(self.format);
        let planes = if format.planar { self.channels } else { 1 };
        if options.plane >= planes || options.offset >= self.frames {
            return Err(JsError::Range("Audio plane or frame offset out of range".into()));
        }
        let available = self.frames - options.offset;
        let frames = options.frames.unwrap_or(available);
        if frames > available {
            return Err(JsError::Range("Audio frame count out of range".into()));
        }
        let channels = if format.planar { 1 } else { self.channels };
        let size = frames
            .checked_mul(channels)
            .and_then(|n| n.checked_mul(format.width() as u32))
            .ok_or_else(|| JsError::Range("Audio copy size exceeds unsigned long".into()))?;
        Ok(Layout { format, plane: options.plane, offset: options.offset, frames, size })
    }

    fn sample_offset(&self, frame: usize, channel: usize) -> usize {
        let index = if self.format.planar {
            channel * self.frames as usize + frame
        } else {
            frame * self.channels as usize + channel
        };
        index * self.format.width()
    }

    fn copy(&self, layout: &Layout) -> JsResult<Bytes> {
        if self.format == layout.format {
            let offset = self.sample_offset(layout.offset as usize, layout.plane as usize);
            return Ok(self.bytes.slice(offset..offset + layout.size as usize));
        }
        let mut out = Vec::new();
        out.try_reserve_exact(layout.size as usize).map_err(|_| JsError::Range("Audio copy is too large".into()))?;
        let channels = if layout.format.planar { layout.plane..layout.plane + 1 } else { 0..self.channels };
        for frame in layout.offset..layout.offset + layout.frames {
            for channel in channels.clone() {
                let at = self.sample_offset(frame as usize, channel as usize);
                let bytes = &self.bytes[at..at + self.format.width()];
                if self.format.encoding == layout.format.encoding {
                    out.extend_from_slice(bytes);
                } else {
                    write_sample(&mut out, layout.format.encoding, read_sample(bytes, self.format.encoding));
                }
            }
        }
        Ok(Bytes::from(out))
    }
}

// Center unsigned PCM around silence. The two integer half-ranges have
// different magnitudes: e.g. signed 16-bit +32767 and -32768 both reach full scale.
fn normalize(sample: i32, magnitude: f64) -> f64 {
    f64::from(sample) / if sample < 0 { magnitude } else { magnitude - 1.0 }
}

fn read_sample(bytes: &[u8], encoding: Encoding) -> f64 {
    match encoding {
        Encoding::U8 => normalize(i32::from(bytes[0]) - 128, 128.0),
        Encoding::S16 => normalize(i32::from(i16::from_ne_bytes([bytes[0], bytes[1]])), 32768.0),
        Encoding::S32 => normalize(i32::from_ne_bytes(bytes.try_into().expect("s32 sample")), 2147483648.0),
        Encoding::F32 => f64::from(f32::from_ne_bytes(bytes.try_into().expect("f32 sample"))),
    }
}

fn quantize(value: f64, magnitude: f64) -> i32 {
    let value = value.clamp(-1.0, 1.0);
    (value * if value < 0.0 { magnitude } else { magnitude - 1.0 }) as i32
}

fn write_sample(out: &mut Vec<u8>, format: Encoding, value: f64) {
    match format {
        Encoding::U8 => out.push((quantize(value, 128.0) + 128) as u8),
        Encoding::S16 => out.extend_from_slice(&(quantize(value, 32768.0) as i16).to_ne_bytes()),
        Encoding::S32 => out.extend_from_slice(&quantize(value, 2147483648.0).to_ne_bytes()),
        Encoding::F32 => out.extend_from_slice(&(value as f32).to_ne_bytes()),
    }
}

struct Data {
    resource: RefCell<Option<AudioResource>>,
    timestamp: i64,
}

impl Data {
    fn new(resource: AudioResource) -> Self {
        Self { timestamp: resource.timestamp, resource: RefCell::new(Some(resource)) }
    }

    fn open(&self) -> JsResult<AudioResource> {
        self.resource.borrow().clone().ok_or_else(|| JsError::dom("InvalidStateError", "AudioData is closed."))
    }
}

fn number(s: &Scope<'_>, value: Value<'_>) -> JsResult<f64> {
    encoding_helper(s, "webIdlNumber")?.call(None, &[value])?.to_number()
}

fn integer(s: &Scope<'_>, v: Value<'_>, min: f64, max: f64) -> JsResult<f64> {
    let n = number(s, v)?.trunc();
    if (min..=max).contains(&n) {
        Ok(n)
    } else {
        Err(JsError::Type("Audio integer outside its Web IDL range".into()))
    }
}

fn ulong(s: &Scope<'_>, v: Value<'_>) -> JsResult<u32> {
    Ok(integer(s, v, 0.0, f64::from(u32::MAX))? as u32)
}

fn byte_len(s: &Scope<'_>, fixed_view: Value<'_>) -> JsResult<usize> {
    Ok(encoding_helper(s, "encodeIntoCapacity")?.call(None, &[fixed_view])?.to_number()? as usize)
}

fn from_init(s: &Scope<'_>, init: Value<'_>) -> JsResult<AudioResource> {
    let init = init.as_object().ok_or_else(|| JsError::Type("Expected an AudioDataInit dictionary".into()))?;
    // Web IDL reads dictionary members in order. A later getter may detach
    // the original data; inspect its actual view again before snapshot/transfer.
    let data = init.get_atom("data")?;
    let view = encoding_helper(s, "fixedByteView")?;
    view.call(None, &[data, s.boolean(true)])?;
    let format = Format::parse(&init.get_atom("format")?.to_string()?)?;
    let channels = ulong(s, init.get_atom("numberOfChannels")?)?;
    let frames = ulong(s, init.get_atom("numberOfFrames")?)?;
    let rate = number(s, init.get_atom("sampleRate")?)? as f32;
    if !rate.is_finite() {
        return Err(JsError::Type("Audio sample rate must be finite".into()));
    }
    let timestamp = integer(s, init.get_atom("timestamp")?, -9_007_199_254_740_991.0, 9_007_199_254_740_991.0)? as i64;
    let transfers = init.get_atom("transfer")?;
    let transfers = encoding_helper(s, "chunkTransferList")?.call(None, &[transfers])?;
    if rate <= 0.0 || channels == 0 || frames == 0 {
        return Err(JsError::Type("Invalid audio dimensions".into()));
    }
    let required = (channels as usize)
        .checked_mul(frames as usize)
        .and_then(|n| n.checked_mul(format.width()))
        .ok_or_else(|| JsError::Type("Invalid audio data size".into()))?;
    let bytes = view.call(None, &[data, s.boolean(true)])?;
    if byte_len(s, bytes)? < required {
        return Err(JsError::Type("Audio data is too short".into()));
    }
    let snapshot = encoding_helper(s, "copyChunkInput")?.call(None, &[bytes, transfers, s.number(required as f64)])?;
    let bytes = buffer_source_bytes(s, snapshot)?.ok_or_else(|| JsError::Type("Invalid audio data".into()))?;
    Ok(AudioResource {
        format,
        rate,
        channels,
        frames,
        timestamp,
        bytes: Bytes::from(bytes),
        cluster: crate::target::page_of(s)?.agent_cluster(),
    })
}

struct CopyOptions {
    format: Option<Format>,
    frames: Option<u32>,
    offset: u32,
    plane: u32,
}

fn options(s: &Scope<'_>, object: Value<'_>) -> JsResult<CopyOptions> {
    let o = object.as_object().ok_or_else(|| JsError::Type("Expected AudioDataCopyToOptions".into()))?;
    let format = o.get_atom("format")?;
    let format = if format.is_undefined() { None } else { Some(Format::parse(&format.to_string()?)?) };
    let frames = o.get_atom("frameCount")?;
    let frames = if frames.is_undefined() { None } else { Some(ulong(s, frames)?) };
    let offset = o.get_atom("frameOffset")?;
    let offset = if offset.is_undefined() { 0 } else { ulong(s, offset)? };
    let plane = ulong(s, o.get_atom("planeIndex")?)?;
    Ok(CopyOptions { format, frames, offset, plane })
}

struct Layout {
    format: Format,
    plane: u32,
    offset: u32,
    frames: u32,
    size: u32,
}

struct AudioData;
impl JsClass for AudioData {
    const NAME: &'static str = "AudioData";
    type Data = Data;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            a.require(1, "AudioData")?;
            s.wrap_new::<Self>(Data::new(from_init(s, a.get(0))?), nt, None)
        });
        def.getter("format", |s, d| Ok(d.resource.borrow().as_ref().map_or(s.null(), |v| s.string(v.format.name()))));
        def.getter("sampleRate", |s, d| Ok(s.number(d.resource.borrow().as_ref().map_or(0., |v| f64::from(v.rate)))));
        def.getter("numberOfFrames", |s, d| {
            Ok(s.number(d.resource.borrow().as_ref().map_or(0., |v| f64::from(v.frames))))
        });
        def.getter("numberOfChannels", |s, d| {
            Ok(s.number(d.resource.borrow().as_ref().map_or(0., |v| f64::from(v.channels))))
        });
        def.getter("timestamp", |s, d| Ok(s.number(d.timestamp as f64)));
        def.getter("duration", |s, d| {
            Ok(s.number(
                d.resource
                    .borrow()
                    .as_ref()
                    .map_or(0., |v| ((f64::from(v.frames) * 1_000_000.0 / f64::from(v.rate)) as u64) as f64),
            ))
        });
        def.method("allocationSize", 1, |s, d, a| {
            a.require(1, "AudioData.allocationSize")?;
            let options = options(s, a.get(0))?;
            Ok(s.number(f64::from(d.open()?.layout(options)?.size)))
        });
        def.method("copyTo", 2, |s, d, a| {
            a.require(2, "AudioData.copyTo")?;
            let view = encoding_helper(s, "fixedByteView")?;
            let dst = view.call(None, &[a.get(0)])?;
            let options = options(s, a.get(1))?;
            let audio = d.open()?;
            let layout = audio.layout(options)?;
            if byte_len(s, dst)? < layout.size as usize {
                return Err(JsError::Range("Audio copy destination is too short".into()));
            }
            let bytes = audio.copy(&layout)?;
            let source = uint8_array(s, &bytes)?;
            encoding_helper(s, "copyChunkTo")?.call(None, &[dst, source])?;
            Ok(s.undefined())
        });
        def.method("clone", 0, |s, d, _| wrap(s, &d.open()?));
        def.method("close", 0, |s, d, _| {
            d.resource.borrow_mut().take();
            Ok(s.undefined())
        });
    }
}

pub(crate) fn register(engine: &Engine) {
    engine.register::<AudioData>();
}

pub(crate) fn is_audio_data(s: &Scope<'_>, value: Value<'_>) -> bool {
    s.unwrap::<AudioData>(value).is_some()
}

pub(crate) fn encoder_input(s: &Scope<'_>, value: Value<'_>) -> JsResult<AudioResource> {
    let data = s.unwrap::<AudioData>(value).ok_or_else(|| JsError::Type("Expected AudioData".into()))?;
    let input = data.resource.borrow().clone();
    input.ok_or_else(|| JsError::Type("AudioData is detached.".into()))
}

pub(crate) fn parts(s: &Scope<'_>, value: Value<'_>) -> JsResult<Option<AudioResource>> {
    s.unwrap::<AudioData>(value)
        .map(|d| d.resource.borrow().clone().ok_or_else(|| JsError::dom("DataCloneError", "AudioData is closed.")))
        .transpose()
}

pub(crate) fn detach(s: &Scope<'_>, value: Value<'_>) -> bool {
    if let Some(d) = s.unwrap::<AudioData>(value) {
        d.resource.borrow_mut().take();
        true
    } else {
        false
    }
}

pub(crate) fn wrap<'s>(s: &Scope<'s>, resource: &AudioResource) -> JsResult<Value<'s>> {
    if !crate::encoded_chunks::exposed_in(s) || !resource.readable_in(crate::target::page_of(s)?.as_ref()) {
        return Err(JsError::dom("DataCloneError", "AudioData is not exposed in this realm or agent cluster"));
    }
    Ok(s.wrap::<AudioData>(Data::new(resource.clone()), None)?.value())
}

/// Wrap native-endian typed PCM without converting integer samples
/// through floating point. The codec owns exactly one complete output block.
pub(crate) fn decoded_pcm<'s>(
    s: &Scope<'s>,
    timestamp: i64,
    format: pyro_media::audio::packet::PcmFormat,
    sample_rate: u32,
    channels: u32,
    frames: u32,
    bytes: Bytes,
) -> JsResult<Value<'s>> {
    use pyro_media::audio::packet::PcmFormat;
    let format = match format {
        PcmFormat::U8 => Format { encoding: Encoding::U8, planar: false },
        PcmFormat::S16 => Format { encoding: Encoding::S16, planar: false },
        PcmFormat::S32 => Format { encoding: Encoding::S32, planar: false },
        PcmFormat::F32Planar => Format { encoding: Encoding::F32, planar: true },
    };
    let size = (frames as usize).checked_mul(channels as usize).and_then(|n| n.checked_mul(format.width()));
    if sample_rate == 0 || channels == 0 || frames == 0 || size != Some(bytes.len()) {
        return Err(JsError::Type("Invalid decoded PCM dimensions".into()));
    }
    let resource = AudioResource {
        format,
        rate: sample_rate as f32,
        channels,
        frames,
        timestamp,
        bytes,
        cluster: crate::target::page_of(s)?.agent_cluster(),
    };
    wrap(s, &resource)
}

pub(crate) fn decoded<'s>(s: &Scope<'s>, timestamp: i64, decoded: pyro_media::audio::Decoded) -> JsResult<Value<'s>> {
    let channels =
        u32::try_from(decoded.channels.len()).map_err(|_| JsError::Range("Too many audio channels".into()))?;
    let frames = u32::try_from(decoded.len()).map_err(|_| JsError::Range("Too many audio frames".into()))?;
    if channels == 0 || frames == 0 || decoded.sample_rate == 0 {
        return Err(JsError::Type("Decoder returned empty audio".into()));
    }
    let size = (channels as usize)
        .checked_mul(frames as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| JsError::Range("Decoded audio is too large".into()))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| JsError::Range("Decoded audio is too large".into()))?;
    for plane in decoded.channels {
        if plane.len() != frames as usize {
            return Err(JsError::Type("Unequal decoded audio plane lengths".into()));
        }
        for sample in plane {
            bytes.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    let resource = AudioResource {
        format: Format { encoding: Encoding::F32, planar: true },
        rate: decoded.sample_rate as f32,
        channels,
        frames,
        timestamp,
        bytes: Bytes::from(bytes),
        cluster: crate::target::page_of(s)?.agent_cluster(),
    };
    wrap(s, &resource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyro_dom::{DocumentKind, Dom};
    use std::rc::Rc;

    fn stereo() -> AudioResource {
        let dom = Rc::new(RefCell::new(Dom::new()));
        let doc = dom.borrow_mut().create_document(DocumentKind::Html, "https://audio.test/");
        let cluster = PageDom::new(dom, doc).agent_cluster();
        let samples: Vec<u8> =
            [-32768i16, 32767, -16384, 8192, 0, 24576].into_iter().flat_map(i16::to_ne_bytes).collect();
        AudioResource {
            format: Format::parse("s16").unwrap(),
            rate: 48000.0,
            channels: 2,
            frames: 3,
            timestamp: -271,
            bytes: Bytes::from(samples),
            cluster,
        }
    }

    #[test]
    fn copies_pcm_by_plane_and_frame_offset() {
        let audio = stereo();
        let opts =
            CopyOptions { format: Some(Format::parse("f32-planar").unwrap()), frames: Some(2), offset: 1, plane: 1 };
        let layout = audio.layout(opts).unwrap();
        let bytes = audio.copy(&layout).unwrap();
        let values: Vec<f32> = bytes.chunks_exact(4).map(|v| f32::from_ne_bytes(v.try_into().unwrap())).collect();
        assert_eq!(values, [8192.0 / 32767.0, 24576.0 / 32767.0]);
        let layout = audio.layout(CopyOptions { format: None, frames: Some(1), offset: 1, plane: 0 }).unwrap();
        assert_eq!(&audio.copy(&layout).unwrap()[..], &audio.bytes[4..8]);
        assert!(audio.layout(CopyOptions { format: None, frames: Some(0), offset: 3, plane: 0 }).is_err());
        assert!(audio.layout(CopyOptions { format: None, frames: None, offset: 0, plane: 1 }).is_err());
    }

    #[test]
    fn pcm_integer_conversion_preserves_silence_and_full_scale() {
        for encoding in [Encoding::U8, Encoding::S16, Encoding::S32] {
            for v in [-1.0, 0.0, 1.0] {
                let mut bytes = Vec::new();
                write_sample(&mut bytes, encoding, v);
                assert_eq!(read_sample(&bytes, encoding), v);
            }
        }
    }
}
