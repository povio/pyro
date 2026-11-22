//! Page-budgeted encoded sources and decoded raster frames for WebCodecs.
//! Each JS decoder owns one private source and appends each stream chunk once.
use std::cell::RefCell;
use std::rc::Rc;

use pyro_canvas::image_decoder::{self, DecodedFrame, ImageType};
use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};
use pyro_raster::{PageRasters, Reservation};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::{PageDom, MAX_IMAGE_BYTES_KEPT};
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpImageDecoder",
    source: include_str!("image_decoder.js"),
    url: "pyro://internal/image_decoder.js",
    natives: Some(natives),
};

pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "ImageDecoder", "ImageTrackList", "ImageTrack");
}

pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    macro_rules! install {
        ($name:ident) => {{
            struct $name;
            impl pyro_js::LazyGlobalInterface for $name {
                const NAME: &'static str = stringify!($name);
                fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
                    MODULE.module(s, &crate::target::page_of(s)?)?.get_atom(Self::NAME)
                }
            }
            s.define_lazy_global_interface::<$name>()?;
        }};
    }
    install!(ImageDecoder);
    install!(ImageTrackList);
    install!(ImageTrack);
    Ok(())
}

fn budget_error() -> JsError {
    JsError::dom("QuotaExceededError", "Image decoder exceeds the page raster budget.")
}

fn closed() -> JsError {
    JsError::dom("InvalidStateError", "The image decoder source was closed.")
}

fn image_type(value: Value<'_>) -> JsResult<ImageType> {
    ImageType::from_mime(&value.to_string()?)
        .ok_or_else(|| JsError::dom("NotSupportedError", "Unsupported image type."))
}

struct Source {
    bytes: Vec<u8>,
    charge: Reservation,
}

impl Source {
    /// Grow geometrically for streamed data. Charge the complete replacement
    /// while both allocations exist; fall back to the exact size if needed.
    fn append(&mut self, page: &PageRasters, bytes: Vec<u8>, chunk_charge: Reservation) -> JsResult<()> {
        let new_len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|&n| n <= MAX_IMAGE_BYTES_KEPT)
            .ok_or_else(budget_error)?;
        if self.bytes.is_empty() {
            self.bytes = bytes;
            self.charge = chunk_charge;
            return Ok(());
        }
        if new_len > self.bytes.capacity() {
            let target = new_len.max(self.bytes.capacity().saturating_mul(2).min(MAX_IMAGE_BYTES_KEPT));
            let (size, mut charge) = match page.reserve(target) {
                Ok(charge) => (target, charge),
                Err(_) => (new_len, page.reserve(new_len).map_err(|_| budget_error())?),
            };
            let mut replacement = Vec::new();
            replacement.try_reserve_exact(size).map_err(|_| budget_error())?;
            charge.grow_to(page, replacement.capacity()).map_err(|_| budget_error())?;
            replacement.extend_from_slice(&self.bytes);
            self.bytes = replacement;
            self.charge = charge;
        }
        self.bytes.extend_from_slice(&bytes);
        Ok(())
    }
}

fn create_source<'s>(s: &Scope<'s>, page: PageRasters) -> JsResult<Value<'s>> {
    let charge = page.reserve(0).map_err(|_| budget_error())?;
    let source = Rc::new(RefCell::new(Some(Source { bytes: Vec::new(), charge })));
    let object = s.object();
    let appending = source.clone();
    let budget = page.clone();
    object.set_atom(
        "append",
        s.function("append", move |s, _, args| {
            // api.js gives us an exclusive, fixed Uint8Array. Account for its
            // private snapshot before allocating or reading the bytes.
            let value = args.get(0);
            let n =
                crate::encoding::encoding_helper(s, "encodeIntoCapacity")?.call(None, &[value])?.to_number()? as usize;
            if appending.borrow().as_ref().ok_or_else(closed)?.bytes.len().saturating_add(n) > MAX_IMAGE_BYTES_KEPT {
                return Err(budget_error());
            }
            let mut charge = budget.reserve(n).map_err(|_| budget_error())?;
            let bytes = crate::encoding::buffer_source_bytes(s, value)?
                .ok_or_else(|| JsError::Type("Expected encoded image bytes".into()))?;
            charge.grow_to(&budget, bytes.capacity()).map_err(|_| budget_error())?;
            appending.borrow_mut().as_mut().ok_or_else(closed)?.append(&budget, bytes, charge)?;
            Ok(s.undefined())
        })?
        .value(),
    )?;
    let inspecting = source.clone();
    object.set_atom(
        "inspect",
        s.function("inspect", move |s, _, args| {
            let kind = image_type(args.get(0))?;
            let finished = args.get(1).to_bool();
            let metadata =
                image_decoder::metadata(kind, &inspecting.borrow().as_ref().ok_or_else(closed)?.bytes, finished)
                    .map_err(|error| JsError::dom("EncodingError", error))?;
            let Some(meta) = metadata else { return Ok(s.null()) };
            let record = s.object();
            record.set_prototype(s.null());
            record.set_atom("width", s.number(f64::from(meta.width)))?;
            record.set_atom("height", s.number(f64::from(meta.height)))?;
            record.set_atom("orientation", s.number(f64::from(meta.orientation)))?;
            record.set_atom("frameCount", s.number(f64::from(meta.frame_count)))?;
            record.set_atom("animated", s.boolean(meta.animated))?;
            record.set_atom("repetitionCount", s.number(meta.repetition_count))?;
            Ok(record.value())
        })?
        .value(),
    )?;
    let decoding = source.clone();
    object.set_atom(
        "decode",
        s.function("decode", move |s, _, args| {
            let kind = image_type(args.get(0))?;
            let finished = args.get(1).to_bool();
            let index = args.get(2).to_number()? as u32;
            let desired = if args.get(3).is_undefined() {
                None
            } else {
                Some((args.get(3).to_number()? as u32, args.get(4).to_number()? as u32))
            };
            let convert = args.get(5).to_string()? != "none";
            let frame = image_decoder::decode(
                kind,
                &decoding.borrow().as_ref().ok_or_else(closed)?.bytes,
                finished,
                index,
                desired,
                convert,
                &page,
            )
            .map_err(|error| JsError::dom("EncodingError", error))?;
            match frame {
                Some(frame) => wrap_frame(s, &crate::target::page_of(s)?, &page, frame),
                None => Ok(s.null()),
            }
        })?
        .value(),
    )?;
    object.set_atom(
        "close",
        s.function("close", move |s, _, _| {
            source.borrow_mut().take();
            Ok(s.undefined())
        })?
        .value(),
    )?;
    Ok(object.value())
}

// Each wrapper has its own native lease owner. Cloning shares the charge;
// closing or collecting the last wrapper releases it even if another wrapper
// disappeared without an explicit close.
fn image_lease<'s>(s: &Scope<'s>, reservation: Rc<Reservation>) -> JsResult<Object<'s>> {
    let owned = RefCell::new(Some(reservation));
    s.function("imageLease", move |s, _, args| {
        if args.get(0).to_bool() {
            let clone = owned.borrow().as_ref().cloned().ok_or_else(closed)?;
            Ok(image_lease(s, clone)?.value())
        } else {
            owned.borrow_mut().take();
            Ok(s.undefined())
        }
    })
}

fn wrap_frame<'s>(s: &Scope<'s>, page: &Rc<PageDom>, budget: &PageRasters, frame: DecodedFrame) -> JsResult<Value<'s>> {
    let DecodedFrame { width, height, rgba, orientation, timestamp, duration, reservation } = frame;
    let (rotation, flip) = match orientation {
        1 => (0, false),
        2 => (0, true),
        3 => (180, false),
        4 => (180, true),
        5 => (90, true),
        6 => (90, false),
        7 => (270, true),
        8 => (270, false),
        _ => return Err(JsError::dom("EncodingError", "Invalid image orientation.")),
    };
    let init = s.object();
    init.set_prototype(s.null());
    init.set_atom("codedWidth", s.number(f64::from(width)))?;
    init.set_atom("codedHeight", s.number(f64::from(height)))?;
    init.set_atom("format", s.string("RGBA"))?;
    init.set_atom("timestamp", s.number(timestamp as f64))?;
    if let Some(duration) = duration {
        init.set_atom("duration", s.number(duration as f64))?;
    }
    init.set_atom("rotation", s.number(f64::from(rotation)))?;
    init.set_atom("flip", s.boolean(flip))?;
    let make = crate::canvas::VIDEO_FRAMES
        .module(s, page)?
        .get_atom("decodedImage")?
        .as_object()
        .ok_or_else(|| JsError::Type("VideoFrame image factory is unavailable".into()))?;
    let copy_charge = budget.reserve(rgba.len()).map_err(|_| budget_error())?;
    let data = crate::encoding::uint8_array(s, &rgba)?;
    drop(rgba);
    drop(copy_charge);
    let lease = image_lease(s, Rc::new(reservation))?;
    match make.call(None, &[data, init.value(), lease.value()]) {
        Ok(frame) => Ok(frame),
        Err(error) => {
            lease.call(None, &[s.boolean(false)])?;
            Err(error)
        }
    }
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let methods = s.object();
    methods.set_atom(
        "supports",
        s.function("supports", |s, _, args| Ok(s.boolean(ImageType::from_mime(&args.get(0).to_string()?).is_some())))?
            .value(),
    )?;
    methods.set_atom(
        "createSource",
        s.function("createSource", |s, _, _| {
            let page = crate::target::page_of(s)?.raster_budget().ok_or_else(closed)?;
            create_source(s, page)
        })?
        .value(),
    )?;
    methods.set_atom("maxBytes", s.number(MAX_IMAGE_BYTES_KEPT as f64))?;
    for name in ["fixedByteView", "chunkTransferList", "copyChunkInput"] {
        methods.set_atom(name, crate::encoding::encoding_helper(s, name)?.value())?;
    }
    for name in ["isStream", "isLocked", "isDisturbed"] {
        methods.set_atom(
            name,
            s.function(name, move |s, _, args| {
                crate::streams::helper(s, &crate::target::page_of(s)?, name, &[args.get(0)])
            })?
            .value(),
        )?;
    }
    crate::video_decoder::install_queue_tasks(s, &methods)?;
    Ok(methods)
}
