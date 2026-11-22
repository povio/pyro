//! The WebCodecs control queue owns one private native decoder handle per
//! configuration. Tags associate reordered pictures with their input timing.
use std::cell::RefCell;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};
use pyro_media::video::{PacketConfig, PacketDecoder, PacketFrame};
use rustc_hash::FxHashMap;

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpVideoDecoder",
    source: concat!(include_str!("video_decoder.js"), "(", include_str!("decoder_queue.js"), ")"),
    url: "pyro://internal/video_decoder.js",
    natives: Some(natives),
};

pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "VideoDecoder");
}

fn config(s: &Scope<'_>, value: Value<'_>) -> JsResult<Option<PacketConfig>> {
    let object = value.as_object().ok_or_else(|| JsError::Type("Expected a VideoDecoderConfig".into()))?;
    if let Some(space) = object.get_atom("colorSpace")?.as_object() {
        // The shipping decoders convert the bitstream's SDR color description.
        // They cannot override the decoded matrix, primaries, transfer or range.
        for name in ["primaries", "transfer", "matrix", "fullRange"] {
            if !space.get_atom(name)?.is_nullish() {
                return Ok(None);
            }
        }
    }
    let width = object.get_atom("codedWidth")?;
    let height = object.get_atom("codedHeight")?;
    if !width.is_undefined() && width.to_number()? * height.to_number()? > (4096 * 2304) as f64 {
        return Ok(None);
    }
    let codec = object.get_atom("codec")?.to_string()?;
    let description = object.get_atom("description")?;
    let bytes = if description.is_undefined() {
        None
    } else {
        Some(crate::encoding::buffer_source_bytes(s, description)?.ok_or_else(|| JsError::Type("Invalid codec description".into()))?)
    };
    Ok(PacketConfig::parse(&codec, bytes.as_deref()))
}

struct Handle {
    decoder: PacketDecoder,
    next_tag: u64,
    timing: FxHashMap<u64, (i64, Option<u64>)>,
}

impl Handle {
    fn pictures(&mut self, frames: Vec<PacketFrame>) -> JsResult<Vec<(PacketFrame, i64, Option<u64>)>> {
        let mut timed = Vec::with_capacity(frames.len());
        for frame in frames {
            let &(timestamp, duration) = self.timing.get(&frame.tag)
                .ok_or_else(|| JsError::dom("EncodingError", "Decoder returned an unknown sample"))?;
            timed.push((frame, timestamp, duration));
        }
        for (frame, _, _) in &timed {
            self.timing.remove(&frame.tag);
        }
        Ok(timed)
    }
}

fn create<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let decoder = config(s, value)?.and_then(|config| config.decoder())
        .ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported video decoder configuration"))?;
    let cell = RefCell::new(Some(Handle { decoder, next_tag: 0, timing: FxHashMap::default() }));
    Ok(s.function("packetDecoder", move |s, _, args| {
        let operation = args.get(0).to_string()?;
        let timed = {
            let mut active = cell.borrow_mut();
            if operation == "close" {
                active.take();
                return Ok(s.undefined());
            }
            let handle = active.as_mut().ok_or_else(|| JsError::dom("InvalidStateError", "Decoder closed"))?;
            match operation.as_str() {
                "decode" => {
                    let chunk = crate::encoded_chunks::video_parts(s, args.get(1))?;
                    // Pictures may be reordered or never displayed. Bound the
                    // timing ledger for a stream that never produces output.
                    if handle.timing.len() >= 4096 {
                        return Err(JsError::dom("EncodingError", "Too many pending video samples"));
                    }
                    let tag = handle.next_tag;
                    handle.next_tag = tag.checked_add(1).ok_or_else(|| JsError::dom("EncodingError", "Sample tag limit"))?;
                    handle.timing.insert(tag, (chunk.timestamp, chunk.duration));
                    let frames = handle.decoder.decode(&chunk.data, tag).map_err(|e| JsError::dom("EncodingError", e))?;
                    handle.pictures(frames)?
                }
                "flush" => {
                    let frames = handle.decoder.flush().map_err(|e| JsError::dom("EncodingError", e))?;
                    let timed = handle.pictures(frames)?;
                    handle.timing.clear();
                    timed
                }
                _ => return Err(JsError::Type("Unknown packet decoder operation".into())),
            }
        };
        let result = s.array(&[])?;
        for (i, (frame, timestamp, duration)) in timed.into_iter().enumerate() {
            let record = s.object();
            record.set_atom("data", crate::encoding::uint8_array(s, &frame.rgba)?)?;
            record.set_atom("format", s.string("RGBA"))?;
            record.set_atom("codedWidth", s.number(f64::from(frame.width)))?;
            record.set_atom("codedHeight", s.number(f64::from(frame.height)))?;
            record.set_atom("timestamp", s.number(timestamp as f64))?;
            record.set_atom("duration", duration.map_or(s.null(), |d| s.number(d as f64)))?;
            result.set_index(i as u32, record.value())?;
        }
        Ok(result.value())
    })?.value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    methods.set_atom("supports", s.function("supports", |s, _, a| Ok(s.boolean(config(s, a.get(0))?.is_some())))?.value())?;
    methods.set_atom("create", s.function("create", |s, _, a| create(s, a.get(0)))?.value())?;
    methods.set_atom("isKeyChunk", s.function("isKeyChunk", |s, _, a| {
        let chunk = crate::encoded_chunks::video_parts(s, a.get(1))?;
        Ok(s.boolean(config(s, a.get(0))?.is_none_or(|c| c.is_key(&chunk.data))))
    })?.value())?;
    install_queue_tasks(s, &methods)?;
    Ok(methods)
}

/// Install event-loop operations shared by WebCodecs control queues.
pub(crate) fn install_queue_tasks<'s>(s: &Scope<'s>, methods: &Object<'s>) -> JsResult<()> {
    methods.set_atom("fireDequeue", s.function("fireDequeue", |s, _, a| {
        let decoder = s.unwrap::<crate::target::EventTarget>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
        let event = crate::event::EventCore::trusted(&decoder.page, crate::event::EventInterface::Event, "dequeue", false, false);
        crate::target::fire(&decoder.page, decoder.key, &event);
        Ok(s.undefined())
    })?.value())?;
    methods.set_atom("postTask", s.function("postTask", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let callback = Persistent::new(s, a.get(0));
        let realm = s.realm().downgrade();
        page.post_task(Box::new(move || {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(scope) = realm.scope() else { return };
            if let Some(function) = callback.get(&scope).as_object() {
                if let Err(error) = function.call(None, &[]) {
                    scope.report_exception(&error);
                }
            }
        }));
        Ok(s.undefined())
    })?.value())?;
    Ok(())
}
