//! WebCodecs AudioEncoder bridge between the control queue and packet encoder.
use std::cell::RefCell;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_media::audio::encode::{EncoderConfig, PacketEncoder};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpAudioEncoder",
    source: include_str!("audio_encoder.js"),
    url: "pyro://internal/audio_encoder.js",
    natives: Some(natives),
};

pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "AudioEncoder");
}

pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    struct AudioEncoder;
    impl pyro_js::LazyGlobalInterface for AudioEncoder {
        const NAME: &'static str = "AudioEncoder";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
        }
    }
    s.define_lazy_global_interface::<AudioEncoder>()
}

// Only the private Web IDL dictionary snapshot reaches the native encoder.
fn config(value: Value<'_>) -> JsResult<Option<EncoderConfig>> {
    let d = value.as_object().ok_or_else(|| JsError::Type("Expected AudioEncoderConfig".into()))?;
    let opus = d.get_atom("opus")?.as_object();
    let number = |name, default| -> JsResult<f64> {
        let Some(opus) = opus else { return Ok(default) };
        let v = opus.get_atom(name)?;
        if v.is_undefined() {
            Ok(default)
        } else {
            v.to_number()
        }
    };
    let string = |name, default: &str| -> JsResult<String> {
        let Some(opus) = opus else { return Ok(default.into()) };
        let v = opus.get_atom(name)?;
        if v.is_undefined() {
            Ok(default.into())
        } else {
            v.to_string()
        }
    };
    let boolean = |name| -> JsResult<bool> {
        Ok(match opus {
            Some(opus) => opus.get_atom(name)?.to_bool(),
            None => false,
        })
    };
    let bitrate = d.get_atom("bitrate")?;
    let bitrate = if bitrate.is_undefined() {
        None
    } else {
        let n = bitrate.to_number()?;
        if n > f64::from(u32::MAX) {
            return Ok(None);
        }
        Some(n as u32)
    };
    let complexity = number("complexity", 9.)?;
    let packet_loss = number("packetlossperc", 0.)?;
    if complexity > 10. || packet_loss > 100. {
        return Ok(None);
    }
    Ok(Some(EncoderConfig {
        codec: d.get_atom("codec")?.to_string()?,
        sample_rate: d.get_atom("sampleRate")?.to_number()? as u32,
        channels: d.get_atom("numberOfChannels")?.to_number()? as u32,
        bitrate,
        constant_bitrate: d.get_atom("bitrateMode")?.to_string()? == "constant",
        frame_duration: number("frameDuration", 20000.)? as u32,
        complexity: complexity as u8,
        packet_loss: packet_loss as u8,
        fec: boolean("useinbandfec")?,
        dtx: boolean("usedtx")?,
        signal: string("signal", "auto")?,
        application: string("application", "audio")?,
        format: string("format", "opus")?,
    }))
}

fn supports(value: Value<'_>) -> JsResult<bool> {
    Ok(config(value)?.is_some_and(|cfg| PacketEncoder::supports(&cfg)))
}

fn create<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let cfg = config(value)?.ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported AudioEncoderConfig"))?;
    let encoder = PacketEncoder::new(&cfg).map_err(|e| JsError::dom("NotSupportedError", e))?;
    let state = RefCell::new(Some((encoder, true)));
    Ok(s.function("audioPacketEncoder", move |s, _, a| {
        let operation = a.get(0).to_string()?;
        let (packets, first, description) = {
            let mut state = state.borrow_mut();
            if operation == "close" {
                state.take();
                return Ok(s.undefined());
            }
            let (encoder, pending_config) =
                state.as_mut().ok_or_else(|| JsError::dom("InvalidStateError", "Encoder closed"))?;
            let packets = match operation.as_str() {
                "encode" => crate::audio_data::encoder_input(s, a.get(1))?.encode_packets(encoder),
                "flush" => encoder.flush(),
                _ => return Err(JsError::Type("Unknown audio encoder operation".into())),
            }
            .map_err(|e| JsError::dom("EncodingError", e))?;
            let first = *pending_config && !packets.is_empty();
            let description = if first { encoder.description().map(<[u8]>::to_vec) } else { None };
            if first {
                *pending_config = false;
            }
            (packets, first, description)
        };
        let outputs = s.array(&[])?;
        let array_prototype = outputs.prototype();
        outputs.set_prototype(s.null());
        for (index, packet) in packets.into_iter().enumerate() {
            let chunk = crate::encoded_chunks::encoded_audio(s, packet.timestamp, packet.duration, packet.data)?;
            let metadata = s.object();
            let proto = metadata.prototype();
            metadata.set_prototype(s.null());
            if first && index == 0 {
                let decoder = s.object();
                decoder.set_prototype(s.null());
                decoder.set_atom("codec", s.string(&cfg.codec))?;
                decoder.set_atom("sampleRate", s.number(f64::from(cfg.sample_rate)))?;
                decoder.set_atom("numberOfChannels", s.number(f64::from(cfg.channels)))?;
                if let Some(bytes) = &description {
                    decoder.set_atom("description", crate::encoding::array_buffer(s, bytes)?)?;
                }
                decoder.set_prototype(proto);
                metadata.set_atom("decoderConfig", decoder.value())?;
            }
            metadata.set_prototype(proto);
            let output = s.object();
            output.set_prototype(s.null());
            output.set_atom("chunk", chunk)?;
            output.set_atom("metadata", metadata.value())?;
            outputs.set_index(index as u32, output.value())?;
        }
        outputs.set_prototype(array_prototype);
        Ok(outputs.value())
    })?
    .value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    methods.set_atom(
        "isAudioData",
        s.function("isAudioData", |s, _, a| Ok(s.boolean(crate::audio_data::is_audio_data(s, a.get(0)))))?.value(),
    )?;
    methods.set_atom(
        "capture",
        s.function("capture", |s, _, a| {
            let input = crate::audio_data::encoder_input(s, a.get(0))?;
            crate::audio_data::wrap(s, &input)
        })?
        .value(),
    )?;
    methods.set_atom("supports", s.function("supports", |s, _, a| Ok(s.boolean(supports(a.get(0))?)))?.value())?;
    methods.set_atom("create", s.function("create", |s, _, a| create(s, a.get(0)))?.value())?;
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}
