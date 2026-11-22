//! WebCodecs audio configuration and packet decoding. Each configuration
//! owns its codec history; the control queue releases it on reset or close.
use std::cell::RefCell;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_media::audio::packet::{PacketAudio, PacketDecoder};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpAudioDecoder",
    source: concat!(include_str!("audio_decoder.js"), "(", include_str!("decoder_queue.js"), ")"),
    url: "pyro://internal/audio_decoder.js",
    natives: Some(natives),
};

pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "AudioDecoder");
}

pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    struct AudioDecoder;
    impl pyro_js::LazyGlobalInterface for AudioDecoder {
        const NAME: &'static str = "AudioDecoder";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
        }
    }
    s.define_lazy_global_interface::<AudioDecoder>()
}

struct Config {
    codec: String,
    rate: u32,
    channels: u32,
}

impl Config {
    // This is the private, validated dictionary snapshot from audio_decoder.js.
    fn read(object: Object<'_>) -> JsResult<Self> {
        Ok(Self {
            codec: object.get_atom("codec")?.to_string()?,
            rate: object.get_atom("sampleRate")?.to_number()? as u32,
            channels: object.get_atom("numberOfChannels")?.to_number()? as u32,
        })
    }
}

fn config_object(value: Value<'_>) -> JsResult<Object<'_>> {
    value.as_object().ok_or_else(|| JsError::Type("Expected an AudioDecoderConfig".into()))
}

fn create<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let object = config_object(value)?;
    let config = Config::read(object)?;
    let description = object.get_atom("description")?;
    let bytes = if description.is_undefined() {
        None
    } else {
        Some(crate::encoding::buffer_source_bytes(s, description)?
            .ok_or_else(|| JsError::Type("Invalid codec description".into()))?)
    };
    let decoder = PacketDecoder::new(&config.codec, config.rate, config.channels, bytes.as_deref())
        .map_err(|e| JsError::dom("NotSupportedError", e))?;
    let active = RefCell::new(Some(decoder));
    let handle = s.function("audioPacketDecoder", move |s, _, args| {
        let operation = args.get(0).to_string()?;
        let output = {
            let mut active = active.borrow_mut();
            if operation == "close" {
                active.take();
                return Ok(s.undefined());
            }
            let decoder = active.as_mut().ok_or_else(|| JsError::dom("InvalidStateError", "Decoder closed"))?;
            match operation.as_str() {
                "decode" => {
                    let chunk = crate::encoded_chunks::audio_parts(s, args.get(1))?;
                    decoder.decode(chunk.timestamp, chunk.data).map_err(|e| JsError::dom("EncodingError", e))?
                }
                "flush" => {
                    decoder.flush().map_err(|e| JsError::dom("EncodingError", e))?;
                    None
                }
                _ => return Err(JsError::Type("Unknown audio decoder operation".into())),
            }
        };
        let frames = s.array(&[])?;
        if let Some(output) = output {
            let data = match output.audio {
                PacketAudio::Planar(audio) => crate::audio_data::decoded(s, output.timestamp, audio)?,
                PacketAudio::Pcm { format, sample_rate, channels, frames, data } => {
                    crate::audio_data::decoded_pcm(s, output.timestamp, format, sample_rate, channels, frames, data)?
                }
            };
            frames.set_index(0, data)?;
        }
        Ok(frames.value())
    })?;
    Ok(handle.value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    let supports = s.function("supports", |s, _, args| {
        let config = Config::read(config_object(args.get(0))?)?;
        Ok(s.boolean(PacketDecoder::supports(&config.codec, config.rate, config.channels)))
    })?;
    methods.set_atom("supports", supports.value())?;
    methods.set_atom("create", s.function("create", |s, _, args| create(s, args.get(0)))?.value())?;
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}
