//! WebCodecs VideoEncoder bridge between the control queue and packet encoder.
use std::cell::RefCell;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};
use pyro_media::video::encode::{EncoderConfig, PacketEncoder};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpVideoEncoder",
    source: include_str!("video_encoder.js"),
    url: "pyro://internal/video_encoder.js",
    natives: Some(natives),
};

pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "VideoEncoder");
}

pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    struct VideoEncoder;
    impl pyro_js::LazyGlobalInterface for VideoEncoder {
        const NAME: &'static str = "VideoEncoder";
        fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
            MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
        }
    }
    s.define_lazy_global_interface::<VideoEncoder>()
}

struct Config {
    codec: EncoderConfig,
    display_width: u32,
    display_height: u32,
}

fn config(value: Value<'_>) -> JsResult<Option<Config>> {
    let d = value.as_object().ok_or_else(|| JsError::Type("Expected a VideoEncoderConfig".into()))?;
    let optional_number = |name| -> JsResult<Option<f64>> {
        let v = d.get_atom(name)?;
        if v.is_undefined() {
            Ok(None)
        } else {
            Ok(Some(v.to_number()?))
        }
    };
    let width = d.get_atom("width")?.to_number()? as u32;
    let height = d.get_atom("height")?.to_number()? as u32;
    let bitrate = optional_number("bitrate")?;
    let scalability = d.get_atom("scalabilityMode")?;
    if bitrate.is_some_and(|v| v > f64::from(u32::MAX))
        || d.get_atom("alpha")?.to_string()? == "keep"
    {
        return Ok(None);
    }
    // Temporal scalability only (one spatial layer): OpenH264 emits L1T1 to L1T3.
    let temporal_layers = if scalability.is_undefined() {
        0
    } else {
        match scalability.to_string()?.as_str() {
            "L1T1" => 1,
            "L1T2" => 2,
            "L1T3" => 3,
            _ => return Ok(None),
        }
    };
    let format = match d.get_atom("avc")?.as_object() {
        Some(avc) => avc.get_atom("format")?.to_string()?,
        None => "avc".into(),
    };
    let mode = d.get_atom("bitrateMode")?;
    Ok(Some(Config {
        codec: EncoderConfig {
            codec: d.get_atom("codec")?.to_string()?,
            width,
            height,
            bitrate: bitrate.map(|v| v as u32),
            framerate: optional_number("framerate")?,
            bitrate_mode: if mode.is_undefined() { "variable".into() } else { mode.to_string()? },
            format,
            temporal_layers,
        },
        display_width: optional_number("displayWidth")?.map_or(width, |v| v as u32),
        display_height: optional_number("displayHeight")?.map_or(height, |v| v as u32),
    }))
}

fn frame<'s>(s: &Scope<'s>, op: &str, value: Value<'s>) -> JsResult<Value<'s>> {
    if let Some(realm) = value.as_object().and_then(|o| s.object_realm(o)) {
        let home = realm.scope()?;
        let page = crate::target::page_of(&home)?;
        if let Some(module) = crate::canvas::VIDEO_FRAMES.existing(&home, &page)? {
            let get = module.get_atom("encoderFrame")?.as_object().ok_or(JsError::ContextDestroyed)?;
            let result = get.call(None, &[home.string(op), home.rehome(value)])?;
            return Ok(Persistent::new(&home, result).get(s));
        }
    }
    if op == "brand" {
        Ok(s.boolean(false))
    } else {
        Err(JsError::Type("Expected a VideoFrame".into()))
    }
}

struct Picture {
    timestamp: i64,
    duration: Option<u64>,
    rotation: u32,
    flip: bool,
    data: Vec<u8>,
}

fn picture(s: &Scope<'_>, frame_data: Object<'_>, w: usize, h: usize) -> JsResult<Picture> {
    let invalid = || JsError::dom("EncodingError", "Invalid source picture");
    let src = crate::encoding::buffer_source_bytes(s, frame_data.get_atom("data")?)?.ok_or_else(invalid)?;
    let number = |key| frame_data.get_atom(key)?.to_number();
    let sw = number("width")? as usize;
    let sh = number("height")? as usize;
    let offset = number("offset")? as usize;
    let stride = number("stride")? as usize;
    let kind = frame_data.get_atom("kind")?.to_string()?;
    let data = convert(&src, sw, sh, offset, stride, &kind, w, h).ok_or_else(invalid)?;
    let duration = frame_data.get_atom("duration")?;
    Ok(Picture {
        timestamp: number("timestamp")? as i64,
        duration: if duration.is_null() { None } else { Some(duration.to_number()? as u64) },
        rotation: number("rotation")? as u32,
        flip: frame_data.get_atom("flip")?.to_bool(),
        data,
    })
}

// Convert sRGB code values to the BT.709 transfer curve before the Y'CbCr matrix.
static SRGB_TO_709: std::sync::LazyLock<[f64; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| {
        let v = i as f64 / 255.;
        let light = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        if light < 0.018 {
            4.5 * light
        } else {
            1.099 * light.powf(0.45) - 0.099
        }
    })
});

// Map pixel centers when the visible crop and coded output sizes differ.
fn scaled(pos: usize, input: usize, output: usize) -> usize {
    (2 * pos + 1) * input / (2 * output)
}

fn convert(
    src: &[u8],
    sw: usize,
    sh: usize,
    offset: usize,
    stride: usize,
    kind: &str,
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    if sw == 0 || sh == 0 || w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return None;
    }
    let area = w.checked_mul(h)?;
    let len = area.checked_add(area / 2)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).ok()?;
    data.resize(len, 0);
    if kind == "I420" {
        let (mut input, mut output) = (0, 0);
        for div in [1, 2, 2] {
            let (iw, ih) = (sw.div_ceil(div), sh.div_ceil(div));
            let (ow, oh) = (w / div, h / div);
            let plane = src.get(input..input + iw * ih)?;
            let dest = &mut data[output..output + ow * oh];
            if iw == ow && ih == oh {
                dest.copy_from_slice(plane);
            } else {
                for (y, row) in dest.chunks_exact_mut(ow).enumerate() {
                    let line = &plane[scaled(y, ih, oh) * iw..];
                    for (x, val) in row.iter_mut().enumerate() {
                        *val = line[scaled(x, iw, ow)];
                    }
                }
            }
            input += iw * ih;
            output += ow * oh;
        }
        return Some(data);
    }
    let red = match kind {
        "RGBA" => 0,
        "BGRA" => 2,
        _ => return None,
    };
    if stride < sw * 4 || offset.checked_add((sh - 1).checked_mul(stride)?)?.checked_add(sw * 4)? > src.len() {
        return None;
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut cb, mut cr) = (0., 0.);
            for dy in 0..2 {
                let row = offset + scaled(y + dy, sh, h) * stride;
                for dx in 0..2 {
                    let at = row + scaled(x + dx, sw, w) * 4;
                    let r = SRGB_TO_709[src[at + red] as usize];
                    let g = SRGB_TO_709[src[at + 1] as usize];
                    let b = SRGB_TO_709[src[at + 2 - red] as usize];
                    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    data[(y + dy) * w + x + dx] = (16. + 219. * luma).round() as u8;
                    cb += (b - luma) / (2. * (1. - 0.0722));
                    cr += (r - luma) / (2. * (1. - 0.2126));
                }
            }
            let at = area + (y / 2) * (w / 2) + x / 2;
            data[at] = (128. + 56. * cb).round() as u8;
            data[at + area / 4] = (128. + 56. * cr).round() as u8;
        }
    }
    Some(data)
}
fn decoder_config<'s>(s: &Scope<'s>, cfg: &Config, pic: &Picture, description: Option<&[u8]>) -> JsResult<Value<'s>> {
    let color = s.object();
    let proto = color.prototype();
    color.set_prototype(s.null());
    for name in ["primaries", "transfer", "matrix"] {
        color.set_atom(name, s.string("bt709"))?;
    }
    color.set_atom("fullRange", s.boolean(false))?;
    color.set_prototype(proto);
    let out = s.object();
    out.set_prototype(s.null());
    out.set_atom("codec", s.string(&cfg.codec.codec))?;
    for (name, dim) in [
        ("codedWidth", cfg.codec.width),
        ("codedHeight", cfg.codec.height),
        ("displayAspectWidth", cfg.display_width),
        ("displayAspectHeight", cfg.display_height),
        ("rotation", pic.rotation),
    ] {
        out.set_atom(name, s.number(f64::from(dim)))?;
    }
    out.set_atom("flip", s.boolean(pic.flip))?;
    out.set_atom("colorSpace", color.value())?;
    if let Some(bytes) = description {
        out.set_atom("description", crate::encoding::array_buffer(s, bytes)?)?;
    }
    out.set_prototype(proto);
    Ok(out.value())
}

fn create<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let cfg = config(value)?.ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported VideoEncoderConfig"))?;
    let encoder = PacketEncoder::new(&cfg.codec).map_err(|e| JsError::dom("NotSupportedError", e))?;
    let active = RefCell::new(Some((encoder, true, None::<Vec<u8>>)));
    Ok(s.function("videoPacketEncoder", move |s, _, a| {
        let op = a.get(0).to_string()?;
        match op.as_str() {
            "close" => {
                active.borrow_mut().take();
                return Ok(s.undefined());
            }
            "flush" => return Ok(s.array(&[])?.value()),
            "encode" => {}
            _ => return Err(JsError::Type("Unknown VideoEncoder operation".into())),
        }
        let descriptor = frame(s, "pixels", a.get(1))?.as_object().ok_or(JsError::ContextDestroyed)?;
        let pic = picture(s, descriptor, cfg.codec.width as usize, cfg.codec.height as usize)?;
        let key = a.get(2).as_object().ok_or(JsError::ContextDestroyed)?.get_atom("keyFrame")?.to_bool();
        let (packet, changed) = {
            let mut active = active.borrow_mut();
            let (encoder, first, description) =
                active.as_mut().ok_or_else(|| JsError::dom("InvalidStateError", "Encoder closed"))?;
            let packet = encoder
                .encode(pic.timestamp, pic.duration, key, &pic.data)
                .map_err(|e| JsError::dom("EncodingError", e))?;
            let changed = *first || packet.description.as_ref().is_some_and(|v| Some(v) != description.as_ref());
            if changed {
                *description = packet.description.clone();
            }
            *first = false;
            (packet, changed)
        };
        let metadata = s.object();
        if cfg.codec.temporal_layers > 0 {
            // VideoEncoderOutputMetadata.svc: the temporal layer of this chunk.
            let svc = s.object();
            svc.set_atom("temporalLayerId", s.number(f64::from(packet.temporal_layer)))?;
            metadata.set_atom("svc", svc.value())?;
        }
        if changed {
            let proto = metadata.prototype();
            metadata.set_prototype(s.null());
            metadata.set_atom("decoderConfig", decoder_config(s, &cfg, &pic, packet.description.as_deref())?)?;
            metadata.set_prototype(proto);
        }
        let chunk =
            crate::encoded_chunks::encoded_video(s, packet.key, packet.timestamp, packet.duration, packet.data)?;
        let output = s.object();
        output.set_prototype(s.null());
        output.set_atom("chunk", chunk)?;
        output.set_atom("metadata", metadata.value())?;
        Ok(s.array(&[output.value()])?.value())
    })?
    .value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    for (name, op) in
        [("isVideoFrame", "brand"), ("capture", "capture"), ("release", "release"), ("orientation", "orientation")]
    {
        methods.set_atom(name, s.function(name, move |s, _, a| frame(s, op, a.get(0)))?.value())?;
    }
    methods.set_atom(
        "supports",
        s.function("supports", |s, _, a| {
            Ok(s.boolean(config(a.get(0))?.is_some_and(|cfg| PacketEncoder::supports(&cfg.codec))))
        })?
        .value(),
    )?;
    methods.set_atom("create", s.function("create", |s, _, a| create(s, a.get(0)))?.value())?;
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}

#[cfg(test)]
mod tests {
    use super::convert;

    #[test]
    fn resamples_visible_bgra_crop_and_discards_alpha() {
        let mut src = vec![0; 4 * 4 * 4];
        for y in 1..3 {
            for x in 1..3 {
                src[(y * 4 + x) * 4 + 2] = 255;
            }
        }
        let out = convert(&src, 2, 2, 20, 16, "BGRA", 4, 2).unwrap();
        assert_eq!(&out[..8], &[63; 8]);
        assert_eq!(&out[8..], &[102, 102, 240, 240]);
    }

    #[test]
    fn scales_odd_i420_planes_independently() {
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
        assert_eq!(convert(&src, 3, 3, 0, 3, "I420", 2, 2).unwrap(), [1, 3, 7, 9, 13, 17]);
    }
}
