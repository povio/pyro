//! WebCodecs encoded media samples. Wrappers in different realms can share the
//! immutable sample bytes, while copyTo writes into the caller's view.
use bytes::Bytes;
use pyro_js::{ClassDef, Engine, JsClass, JsError, JsResult, NoParent, Scope, Value};

#[derive(Clone, Debug)]
pub(crate) struct EncodedChunk {
    video: bool,
    key: bool,
    pub(crate) timestamp: i64,
    pub(crate) duration: Option<u64>,
    pub(crate) data: Bytes,
    cluster: crate::page::AgentCluster,
}

impl EncodedChunk {
    pub(crate) fn readable_in(&self, page: &crate::PageDom) -> bool {
        self.cluster == page.agent_cluster()
    }

    pub(crate) fn byte_length(&self) -> usize {
        self.data.len()
    }
}

fn integer(value: Value<'_>, minimum: f64) -> JsResult<f64> {
    const MAX: f64 = 9_007_199_254_740_991.0;
    let n = value.to_number()?.trunc();
    if !(minimum..=MAX).contains(&n) {
        return Err(JsError::Type("Chunk timestamp or duration is outside the Web IDL integer range".into()));
    }
    Ok(n)
}

fn from_init(s: &Scope<'_>, init: Value<'_>, video: bool) -> JsResult<EncodedChunk> {
    let init = init.as_object().ok_or_else(|| JsError::Type("Expected an encoded chunk dictionary".into()))?;
    // Convert dictionary members in Web IDL order. Recheck the input buffer
    // after the remaining getters/iterator have run, before detaching anything.
    let view = crate::encoding::encoding_helper(s, "fixedByteView")?;
    let data = init.get_atom("data")?;
    view.call(None, &[data, s.boolean(true)])?;
    let duration = init.get_atom("duration")?;
    let duration = if duration.is_undefined() { None } else { Some(integer(duration, 0.)? as u64) };
    let timestamp = integer(init.get_atom("timestamp")?, -9_007_199_254_740_991.)? as i64;
    let transfer = init.get_atom("transfer")?;
    let list = crate::encoding::encoding_helper(s, "chunkTransferList")?.call(None, &[transfer])?;
    let key = match init.get_atom("type")?.to_string()?.as_str() {
        "key" => true,
        "delta" => false,
        _ => return Err(JsError::Type("Encoded chunk type must be key or delta".into())),
    };
    let bytes = view.call(None, &[data, s.boolean(true)])?;
    let copy = crate::encoding::encoding_helper(s, "copyChunkInput")?.call(None, &[bytes, list])?;
    let data = crate::encoding::buffer_source_bytes(s, copy)?
        .ok_or_else(|| JsError::Type("Invalid encoded chunk data".into()))?;
    let cluster = crate::target::page_of(s)?.agent_cluster();
    Ok(EncodedChunk { video, key, timestamp, duration, data: Bytes::from(data), cluster })
}

macro_rules! encoded_chunk {
    ($class:ident, $video:literal) => {
        struct $class;
        impl JsClass for $class {
            const NAME: &'static str = stringify!($class);
            type Data = EncodedChunk;
            type Parent = NoParent;
            fn define(def: &mut ClassDef<Self>) {
                def.constructor(1, |s, nt, args| {
                    args.require(1, Self::NAME)?;
                    s.wrap_new::<Self>(from_init(s, args.get(0), $video)?, nt, None)
                });
                def.getter("type", |s, c| Ok(s.string(if c.key { "key" } else { "delta" })));
                def.getter("timestamp", |s, c| Ok(s.number(c.timestamp as f64)));
                def.getter("duration", |s, c| Ok(c.duration.map_or(s.null(), |d| s.number(d as f64))));
                def.getter("byteLength", |s, c| Ok(s.number(c.byte_length() as f64)));
                def.method("copyTo", 1, |s, c, args| {
                    args.require(1, "copyTo")?;
                    let dest = crate::encoding::encoding_helper(s, "fixedByteView")?.call(None, &[args.get(0)])?;
                    let bytes = crate::encoding::uint8_array(s, &c.data)?;
                    crate::encoding::encoding_helper(s, "copyChunkTo")?.call(None, &[dest, bytes])?;
                    Ok(s.undefined())
                });
            }
        }
    };
}

encoded_chunk!(EncodedAudioChunk, false);
encoded_chunk!(EncodedVideoChunk, true);

pub(crate) fn register(engine: &Engine) {
    engine.register::<EncodedAudioChunk>();
    engine.register::<EncodedVideoChunk>();
}

pub(crate) fn parts(s: &Scope<'_>, value: Value<'_>) -> Option<EncodedChunk> {
    s.unwrap::<EncodedAudioChunk>(value).or_else(|| s.unwrap::<EncodedVideoChunk>(value)).map(|c| (*c).clone())
}

pub(crate) fn audio_parts(s: &Scope<'_>, value: Value<'_>) -> JsResult<EncodedChunk> {
    s.unwrap::<EncodedAudioChunk>(value).map(|chunk| (*chunk).clone())
        .ok_or_else(|| JsError::Type("Expected an EncodedAudioChunk".into()))
}

pub(crate) fn video_parts(s: &Scope<'_>, value: Value<'_>) -> JsResult<EncodedChunk> {
    s.unwrap::<EncodedVideoChunk>(value).map(|chunk| (*chunk).clone())
        .ok_or_else(|| JsError::Type("Expected an EncodedVideoChunk".into()))
}

pub(crate) fn exposed_in(s: &Scope<'_>) -> bool {
    !crate::worker_scope::is_worker(s) || s.global_data::<crate::worker_scope::DedicatedWorkerGlobalScope>().is_some()
}

pub(crate) fn wrap<'s>(s: &Scope<'s>, chunk: &EncodedChunk) -> JsResult<Value<'s>> {
    if !exposed_in(s) {
        return Err(JsError::dom("DataCloneError", "Encoded chunks are not exposed in this worker"));
    }
    if !chunk.readable_in(crate::target::page_of(s)?.as_ref()) {
        return Err(JsError::dom("DataCloneError", "Encoded chunk belongs to another agent cluster"));
    }
    let object = if chunk.video {
        s.wrap::<EncodedVideoChunk>(chunk.clone(), None)?
    } else {
        s.wrap::<EncodedAudioChunk>(chunk.clone(), None)?
    };
    Ok(object.value())
}
