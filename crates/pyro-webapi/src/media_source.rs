//! Media Source Extensions for Window. A native MediaSource keeps its identity
//! for the object URL registry. Its wrapper traces its JS control state, and
//! the presentation and SourceBuffer wrappers share the same coded frames.
use std::cell::RefCell;
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_media::mse_buffer::{AppendMode, CodedBuffer, MediaType};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::target::{page_of, EventTarget, EventTargetData};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpMediaSource",
    source: include_str!("media_source.js"),
    url: "pyro://internal/media_source.js",
    natives: Some(natives),
};

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "SourceBuffer", "SourceBufferList");
}

type Buffers = Vec<Rc<RefCell<CodedBuffer>>>;

pub struct MediaSourceData {
    pub target: EventTargetData,
    buffers: Rc<RefCell<Buffers>>,
}

pub struct MediaSource;

fn control<'s>(s: &Scope<'s>, source: Object<'s>, op: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    let f = s.get_hidden(source, "lpSourceControl")?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)?;
    let mut values = Vec::with_capacity(args.len() + 1);
    values.push(s.string(op));
    values.extend_from_slice(args);
    f.call(None, &values)
}

pub(crate) fn attach<'s>(s: &Scope<'s>, source: Object<'s>, notify: Value<'s>) -> JsResult<bool> {
    Ok(control(s, source, "attach", &[notify])?.to_bool())
}

pub(crate) fn detach<'s>(s: &Scope<'s>, source: Object<'s>) -> JsResult<()> {
    control(s, source, "detach", &[])?;
    Ok(())
}

pub(crate) fn presentation(s: &Scope<'_>, source: Object<'_>) -> JsResult<Buffers> {
    let d = s.unwrap::<MediaSource>(source.value()).ok_or(JsError::ContextDestroyed)?;
    let buffers = d.buffers.borrow().clone();
    Ok(buffers)
}

fn source_object<'s>(s: &Scope<'s>, d: &MediaSourceData) -> JsResult<Object<'s>> {
    crate::target::target_object(s, &d.target.page, d.target.key)?.ok_or(JsError::ContextDestroyed)
}

impl JsClass for MediaSource {
    const NAME: &'static str = "MediaSource";
    type Data = MediaSourceData;
    type Parent = EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &MediaSourceData| &d.target);
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            let target = EventTargetData::new(&page);
            let pyro_dom::events::TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            let source = s.wrap_new::<MediaSource>(
                MediaSourceData { target, buffers: Rc::new(RefCell::new(Vec::new())) },
                nt,
                Some(key),
            )?;
            let state = MODULE.helper(s, &page, "control", &[source.value()])?;
            s.set_hidden(source, "lpSourceControl", state)?;
            Ok(source)
        });
        def.getter_this("readyState", |s, _, this| control(s, this, "readyState", &[]));
        def.getter_this("sourceBuffers", |s, _, this| control(s, this, "sourceBuffers", &[]));
        def.getter_this("activeSourceBuffers", |s, _, this| control(s, this, "activeSourceBuffers", &[]));
        def.accessor(
            "duration",
            |s, d| control(s, source_object(s, d)?, "duration", &[]),
            |s, d, v| {
                control(s, source_object(s, d)?, "setDuration", &[v])?;
                Ok(())
            },
        );
        def.method_this("addSourceBuffer", 1, |s, _, this, a| {
            a.require(1, "MediaSource.addSourceBuffer")?;
            control(s, this, "add", &[a.get(0)])
        });
        def.method_this("removeSourceBuffer", 1, |s, _, this, a| {
            a.require(1, "MediaSource.removeSourceBuffer")?;
            control(s, this, "remove", &[a.get(0)])
        });
        def.method_this("endOfStream", 0, |s, _, this, a| control(s, this, "end", &[a.get(0)]));
        def.method_this("setLiveSeekableRange", 2, |s, _, this, a| {
            a.require(2, "MediaSource.setLiveSeekableRange")?;
            control(s, this, "setLive", &[a.get(0), a.get(1)])
        });
        def.method_this("clearLiveSeekableRange", 0, |s, _, this, _| control(s, this, "clearLive", &[]));
        def.static_method("isTypeSupported", 1, |s, a| {
            a.require(1, "MediaSource.isTypeSupported")?;
            Ok(s.boolean(MediaType::parse(&a.get(0).to_string()?).is_some()))
        });
        def.accessor(
            "onsourceopen",
            |s, d| crate::target::event_handler(s, &d.target, "sourceopen"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceopen", v),
        );
        def.accessor(
            "onsourceended",
            |s, d| crate::target::event_handler(s, &d.target, "sourceended"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceended", v),
        );
        def.accessor(
            "onsourceclose",
            |s, d| crate::target::event_handler(s, &d.target, "sourceclose"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceclose", v),
        );
    }
}

/// `URL.createObjectURL(mediaSource)`: a blob URL registered as a media-source
/// entry, which no fetch resolves.
pub(crate) fn object_url(s: &Scope<'_>, page: &PageDom) -> JsResult<String> {
    let origin = crate::storage::tuple_origin(s, page).unwrap_or_else(|| "null".into());
    Ok(format!("blob:{origin}/{}", crate::crypto::random_uuid()?))
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<MediaSource>();
}

fn make_buffer<'s>(s: &Scope<'s>, source: Value<'s>, ty: &str) -> JsResult<Value<'s>> {
    let source = s.unwrap::<MediaSource>(source).ok_or(JsError::ContextDestroyed)?;
    let ty = MediaType::parse(ty).ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported SourceBuffer type."))?;
    let state = Rc::new(RefCell::new(CodedBuffer::new(ty)));
    let registered = Rc::downgrade(&source.buffers);
    let model = state.clone();
    let function = s.function("codedBuffer", move |s, _, a| {
        let op = a.get(0).to_string()?;
        let result = match op.as_str() {
            "canAppend" => s.boolean(model.borrow().can_append(a.get(1).to_number()? as usize)),
            "append" => {
                let bytes = crate::encoding::buffer_source_bytes(s, a.get(1))?.ok_or(JsError::ContextDestroyed)?;
                let init =
                    model.borrow_mut().append(&bytes).map_err(|_| JsError::Type("Invalid media segment.".into()))?;
                s.boolean(init)
            }
            "remove" => {
                model.borrow_mut().remove(a.get(1).to_number()?, a.get(2).to_number()?);
                s.undefined()
            }
            "reset" => {
                model.borrow_mut().reset_parser();
                s.undefined()
            }
            "change" => {
                let ty = MediaType::parse(&a.get(1).to_string()?)
                    .ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported type."))?;
                model.borrow_mut().change_type(ty);
                s.undefined()
            }
            "mode" => {
                let mode =
                    if a.get(1).to_string()? == "sequence" { AppendMode::Sequence } else { AppendMode::Segments };
                model.borrow_mut().set_mode(mode);
                s.undefined()
            }
            "window" => {
                model.borrow_mut().set_window(a.get(1).to_number()?, a.get(2).to_number()?);
                s.undefined()
            }
            "offset" => s.number(model.borrow().timestamp_offset()),
            "setOffset" => {
                model.borrow_mut().set_timestamp_offset(a.get(1).to_number()?);
                s.undefined()
            }
            "partial" => s.boolean(model.borrow().partial_segment()),
            "trackKinds" => {
                let kinds = model
                    .borrow()
                    .track_buffers()
                    .iter()
                    .fold(0u8, |kinds, track| kinds | if track.track().width > 0 { 2 } else { 1 });
                s.number(f64::from(kinds))
            }
            "initialized" => s.boolean(model.borrow().initialized()),
            "initDuration" => model.borrow().init_duration().map_or_else(|| s.null(), |t| s.number(t)),
            "highestStart" => model.borrow().highest_start().map_or_else(|| s.null(), |t| s.number(t)),
            "highestEnd" => s.number(model.borrow().highest_end()),
            "ranges" => {
                let flat: Vec<_> = model
                    .borrow()
                    .ranges(a.get(1).to_bool())
                    .into_iter()
                    .flat_map(|(a, b)| [s.number(a), s.number(b)])
                    .collect();
                s.array(&flat)?.value()
            }
            "release" => {
                if let Some(list) = registered.upgrade() {
                    list.borrow_mut().retain(|buf| !Rc::ptr_eq(buf, &model));
                }
                s.undefined()
            }
            _ => return Err(JsError::Type("Unknown coded-buffer operation.".into())),
        };
        Ok(result)
    })?;
    source.buffers.borrow_mut().push(state);
    Ok(function.value())
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    methods.set_atom(
        "typeKinds",
        s.function("typeKinds", |s, _, a| {
            let kinds = MediaType::parse(&a.get(0).to_string()?)
                .map_or(0, |ty| u8::from(ty.has_audio()) | (u8::from(ty.has_video()) << 1));
            Ok(s.number(f64::from(kinds)))
        })?
        .value(),
    )?;
    methods.set_atom(
        "makeBuffer",
        s.function("makeBuffer", |s, _, a| make_buffer(s, a.get(0), &a.get(1).to_string()?))?.value(),
    )?;
    methods.set_atom(
        "timeRanges",
        s.function("timeRanges", |s, _, a| crate::media::mse_time_ranges(s, a.get(0)))?.value(),
    )?;
    methods.set_atom(
        "fire",
        s.function("fire", |s, _, a| {
            let target = s.unwrap::<EventTarget>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            let ty = a.get(1).to_string()?;
            let event =
                crate::event::EventCore::trusted(&target.page, crate::event::EventInterface::Event, &ty, false, false);
            crate::target::fire(&target.page, target.key, &event);
            Ok(s.undefined())
        })?
        .value(),
    )?;
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}
