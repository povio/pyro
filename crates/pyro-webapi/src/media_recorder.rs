//! MediaStream Recording (`MediaRecorder`, `BlobEvent`), made in JS (media_recorder.js) on a realm's
//! first use. The state machine, events, bitrate rules and the capture of the stream's tracks are
//! there; what is native is what turns captured pictures and samples into a file: a session
//! ([`create`]) that encodes video with OpenH264 and audio with Opus, the encoders WebCodecs already
//! uses (`pyro_media::{video,audio}::encode`), and muxes them into fragmented MP4
//! (`pyro_media::mux::mp4`). Nothing is made, and no timer runs, until a recorder is started.

use std::cell::RefCell;
use std::sync::LazyLock;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_media::audio::encode as audio;
use pyro_media::mux::mp4::{FragmentWriter, Sample};
use pyro_media::video::encode as video;

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::window::WindowHost;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpMediaRecorder",
    source: include_str!("media_recorder.js"),
    url: "pyro://internal/media_recorder.js",
    natives: Some(natives),
};

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "MediaRecorder", "BlobEvent");
}

/// H.264 `level_idc` values (as decimal numbers) the encoder knows, lowest first: the recorder uses the
/// lowest one that fits the picture size, frame rate and bitrate.
const LEVELS: [u32; 16] = [10, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52];

fn video_config(profile: &str, level: u32, width: u32, height: u32, bitrate: u32, fps: f64) -> video::EncoderConfig {
    video::EncoderConfig {
        codec: format!("avc1.{profile}{level:02x}"),
        width,
        height,
        bitrate: Some(bitrate),
        framerate: Some(fps),
        bitrate_mode: "variable".into(),
        format: "avc".into(),
        temporal_layers: 0,
    }
}

fn audio_config(rate: u32, channels: u32, bitrate: u32) -> audio::EncoderConfig {
    audio::EncoderConfig {
        codec: "opus".into(),
        sample_rate: rate,
        channels,
        // The encoder clamps each channel to 300 kbit/s; the attribute the page reads may be higher
        // (Opus allows 510 kbit/s a channel), which only matters where Opus is transparent anyway.
        bitrate: Some(bitrate.clamp(6000, 300_000 * channels)),
        constant_bitrate: false,
        frame_duration: 20_000,
        complexity: 9,
        packet_loss: 0,
        fec: false,
        dtx: false,
        signal: "auto".into(),
        application: "audio".into(),
        format: "opus".into(),
    }
}

struct VideoPart {
    encoder: video::PacketEncoder,
    track: usize,
    width: usize,
    height: usize,
    /// Pictures encoded since the last key picture, and the requested distance (0: only the first).
    since_key: u64,
    key_every: u64,
    nominal_us: u64,
    started: bool,
    /// `avc1.PPCCLL` of the stream once the encoder has produced its parameter sets.
    codec: String,
}

struct AudioPart {
    encoder: audio::PacketEncoder,
    track: usize,
    rate: u32,
    channels: u32,
}

struct Session {
    mux: FragmentWriter,
    video: Option<VideoPart>,
    audio: Option<AudioPart>,
}

// Convert sRGB code values to the BT.709 transfer curve before the Y'CbCr matrix, as the WebCodecs
// encoder does, because the video path shows H.264 pictures through the inverse.
static SRGB_TO_709: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let v = i as f64 / 255.;
        let light = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        (if light < 0.018 { 4.5 * light } else { 1.099 * light.powf(0.45) - 0.099 }) as f32
    })
});

/// Packed limited-range BT.709 I420 of `rgba` (straight alpha, composited over black as a canvas is
/// when it is shown on a black page); `width` and `height` are even.
fn rgba_to_i420(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let lut = &*SRGB_TO_709;
    let area = width * height;
    let mut out = vec![0u8; area + area / 2];
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let (mut cb, mut cr) = (0f32, 0f32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let at = ((y + dy) * width + x + dx) * 4;
                    let alpha = u32::from(rgba[at + 3]);
                    let over = |c: u8| lut[((u32::from(c) * alpha + 127) / 255) as usize];
                    let (r, g, b) = (over(rgba[at]), over(rgba[at + 1]), over(rgba[at + 2]));
                    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    out[(y + dy) * width + x + dx] = (16. + 219. * luma).round() as u8;
                    cb += (b - luma) / (2. * (1. - 0.0722));
                    cr += (r - luma) / (2. * (1. - 0.2126));
                }
            }
            let at = area + (y / 2) * (width / 2) + x / 2;
            out[at] = (128. + 56. * cb).round().clamp(16., 240.) as u8;
            out[at + area / 4] = (128. + 56. * cr).round().clamp(16., 240.) as u8;
        }
    }
    out
}

fn number(o: Object<'_>, key: &'static str) -> JsResult<f64> {
    o.get_atom(key)?.to_number()
}

fn encoding_error(e: String) -> JsError {
    JsError::dom("EncodingError", e)
}

/// `create(options)`: a session for the tracks a recorder records. `options.video` (or null):
/// `{width, height, bitrate, fps, profile: 'PPCC' hex, entry: 'avc1'|'avc3', keyEvery: pictures}`;
/// `options.audio` (or null): `{rate, channels, bitrate}`. Returns the session function:
///   `s('video', rgba, timeUs, forceKey)`  one picture of the session's size,
///   `s('audio', float32Interleaved, timeUs)`  samples, contiguous with the previous call,
///   `s('flush', last)`  the file bytes since the last flush as a Uint8Array (maybe empty),
///   `s('codec')`  the video codec string once known, `s('close')`.
/// Throws NotSupportedError when an encoder cannot take the configuration.
fn create<'s>(s: &Scope<'s>, options: Value<'s>) -> JsResult<Value<'s>> {
    let o = options.as_object().ok_or_else(|| JsError::Type("Expected recorder options".into()))?;
    let unsupported = |what: &str| JsError::dom("NotSupportedError", format!("Unsupported {what} recording configuration"));
    let mut mux = FragmentWriter::new();
    let mut video_part = None;
    let mut audio_part = None;
    if let Some(v) = o.get_atom("video")?.as_object() {
        let (width, height) = (number(v, "width")? as u32, number(v, "height")? as u32);
        let (bitrate, fps) = (number(v, "bitrate")? as u32, number(v, "fps")?);
        let profile = v.get_atom("profile")?.to_string()?;
        let config = LEVELS
            .iter()
            .map(|&level| video_config(&profile, level, width, height, bitrate, fps))
            .find(video::PacketEncoder::supports)
            .ok_or_else(|| unsupported("video"))?;
        let encoder = video::PacketEncoder::new(&config).map_err(|_| unsupported("video"))?;
        let entry = if v.get_atom("entry")?.to_string()? == "avc3" { *b"avc3" } else { *b"avc1" };
        let track = mux.add_video(width, height, entry);
        video_part = Some(VideoPart {
            encoder,
            track,
            width: width as usize,
            height: height as usize,
            since_key: 0,
            key_every: number(v, "keyEvery")? as u64,
            nominal_us: (1_000_000. / fps) as u64,
            started: false,
            codec: String::new(),
        });
    }
    if let Some(a) = o.get_atom("audio")?.as_object() {
        let (rate, channels) = (number(a, "rate")? as u32, number(a, "channels")? as u32);
        let config = audio_config(rate, channels, number(a, "bitrate")? as u32);
        let encoder = audio::PacketEncoder::new(&config).map_err(|_| unsupported("audio"))?;
        let track = mux.add_audio(channels as u16);
        mux.set_config(track, encoder.description().unwrap_or_default().to_vec());
        audio_part = Some(AudioPart { encoder, track, rate, channels });
    }
    let session = RefCell::new(Some(Session { mux, video: video_part, audio: audio_part }));
    Ok(s.function("recorderSession", move |s, _, a| {
        let op = a.get(0).to_string()?;
        if op == "close" {
            session.borrow_mut().take();
            return Ok(s.undefined());
        }
        let mut guard = session.borrow_mut();
        let session = guard.as_mut().ok_or_else(|| JsError::dom("InvalidStateError", "The recording session is closed"))?;
        match op.as_str() {
            "video" => {
                let rgba = crate::encoding::buffer_source_bytes(s, a.get(1))?.unwrap_or_default();
                let time = a.get(2).to_number()?.max(0.) as u64;
                let part = session.video.as_mut().ok_or_else(|| JsError::Type("The session has no video track".into()))?;
                if rgba.len() != part.width * part.height * 4 {
                    return Err(JsError::Type("A picture does not match the recording size".into()));
                }
                let key = a.get(3).to_bool() || !part.started || (part.key_every > 0 && part.since_key >= part.key_every);
                let packet = part
                    .encoder
                    .encode(time as i64, None, key, &rgba_to_i420(&rgba, part.width, part.height))
                    .map_err(encoding_error)?;
                part.started = true;
                part.since_key = if packet.key { 0 } else { part.since_key + 1 };
                if let Some(avcc) = packet.description.filter(|d| d.len() >= 4) {
                    part.codec = format!("avc1.{:02x}{:02x}{:02x}", avcc[1], avcc[2], avcc[3]);
                    session.mux.set_config(part.track, avcc);
                }
                session.mux.push(
                    part.track,
                    Sample { data: packet.data, time_us: time, duration_us: part.nominal_us, key: packet.key },
                );
                Ok(s.undefined())
            }
            "audio" => {
                let bytes = crate::encoding::buffer_source_bytes(s, a.get(1))?.unwrap_or_default();
                let time = a.get(2).to_number()?.max(0.) as i64;
                let part = session.audio.as_mut().ok_or_else(|| JsError::Type("The session has no audio track".into()))?;
                let pcm: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                if pcm.is_empty() {
                    return Ok(s.undefined());
                }
                let packets = part.encoder.encode(time, part.rate, part.channels, &pcm).map_err(encoding_error)?;
                for p in packets {
                    let sample = Sample { data: p.data.to_vec(), time_us: p.timestamp.max(0) as u64, duration_us: p.duration, key: true };
                    session.mux.push(part.track, sample);
                }
                Ok(s.undefined())
            }
            "flush" => {
                let last = a.get(1).to_bool();
                if last {
                    // The encoder's remaining samples (and the padding that recovers its lookahead).
                    if let Some(part) = session.audio.as_mut() {
                        for p in part.encoder.flush().map_err(encoding_error)? {
                            let sample = Sample { data: p.data.to_vec(), time_us: p.timestamp.max(0) as u64, duration_us: p.duration, key: true };
                            session.mux.push(part.track, sample);
                        }
                    }
                }
                crate::encoding::uint8_array(s, &session.mux.flush(last))
            }
            "codec" => Ok(s.string(session.video.as_ref().map_or("", |v| v.codec.as_str()))),
            _ => Err(JsError::Type("Unknown recorder session operation".into())),
        }
    })?
    .value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    native.set_atom("create", s.function("create", |s, _, a| create(s, a.get(0)))?.value())?;
    // What this build can encode: MediaRecorder.isTypeSupported() answers from it.
    let can_video = s.function("canVideo", |s, _, _| {
        Ok(s.boolean(video::PacketEncoder::supports(&video_config("42c0", 30, 320, 240, 1_000_000, 30.))))
    })?;
    native.set_atom("canVideo", can_video.value())?;
    let can_audio = s.function("canAudio", |s, _, _| Ok(s.boolean(audio::PacketEncoder::supports(&audio_config(48_000, 2, 128_000)))))?;
    native.set_atom("canAudio", can_audio.value())?;
    // Whether the realm's document is fully active (false once its frame is removed or navigated).
    let active = s.function("active", |s, _, _| {
        Ok(s.boolean(crate::target::page_of(s).map(|page| page.is_fully_active()).unwrap_or(false)))
    })?;
    native.set_atom("active", active.value())?;
    // dispatchTrusted(target, event): the recorder's own events, `isTrusted` true.
    let trusted = s.function("dispatchTrusted", |s, _, a| {
        crate::cookie_store::events::dispatch_trusted(s, a.get(0), a.get(1))?;
        Ok(s.undefined())
    })?;
    native.set_atom("dispatchTrusted", trusted.value())?;
    // The realm's media stream module: MediaStream and what tells a track's source (tainted or not).
    let page = crate::target::page_of(s)?;
    native.set_atom("stream", crate::media_stream::MODULE.module(s, &page)?.value())?;
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::rgba_to_i420;

    #[test]
    fn black_and_white_map_to_the_limited_range_ends() {
        let mut rgba = vec![0u8; 2 * 2 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        assert_eq!(rgba_to_i420(&rgba, 2, 2), [16, 16, 16, 16, 128, 128]);
        for px in rgba.chunks_exact_mut(4) {
            px[..3].copy_from_slice(&[255, 255, 255]);
        }
        assert_eq!(rgba_to_i420(&rgba, 2, 2), [235, 235, 235, 235, 128, 128]);
    }

    #[test]
    fn a_transparent_pixel_is_black() {
        assert_eq!(rgba_to_i420(&[255, 255, 255, 0].repeat(4), 2, 2), [16, 16, 16, 16, 128, 128]);
    }
}
