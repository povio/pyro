//! Web Audio: `AudioContext`, `OfflineAudioContext`, `AudioBuffer`, the
//! source nodes (`AudioBufferSourceNode`, `OscillatorNode`,
//! `ConstantSourceNode`), `GainNode` and `AudioParam` automation, made in JS
//! (webaudio.js) on a realm's first use of any of them.
//!
//! An `OfflineAudioContext` renders its graph in 128-frame quanta when
//! `startRendering()` runs. A real-time `AudioContext` has no output device:
//! its `currentTime` advances on the document clock while it runs and its
//! sources end on time, but no sample is computed.
//!
//! `decodeAudioData` decodes with pyro-media (Symphonia and opus-pure) on a
//! thread of its own, made for the call, and resamples to the context's
//! rate; the promise settles in a task on the page's thread.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_js::sys;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpWebAudio",
    source: include_str!("webaudio.js"),
    url: "pyro://internal/webaudio.js",
    natives: Some(natives),
};

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "BaseAudioContext",
        "AudioContext",
        "OfflineAudioContext",
        "AudioBuffer",
        "AudioParam",
        "AudioNode",
        "AudioDestinationNode",
        "AudioScheduledSourceNode",
        "AudioBufferSourceNode",
        "OscillatorNode",
        "ConstantSourceNode",
        "GainNode",
        "StereoPannerNode",
        "BiquadFilterNode",
        "IIRFilterNode",
        "ChannelMergerNode",
        "ChannelSplitterNode",
        "OfflineAudioCompletionEvent",
    );
}

/// Decodes in flight: their realm and the JS completion callback.
struct Pending {
    realm: WeakRealm,
    done: Persistent,
}

thread_local! {
    static PENDING: RefCell<FxHashMap<u32, Pending>> = RefCell::new(FxHashMap::default());
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

type Decoded = Result<(u32, Vec<Vec<f32>>), String>;

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    let decode = s.function("decode", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let bytes = crate::encoding::buffer_source_bytes(s, a.get(0))?.unwrap_or_default();
        let rate = a.get(1).to_number()?;
        let rate = if rate.is_finite() && rate > 0.0 { rate as u32 } else { 48_000 };
        let done = a.get(2);
        if !done.is_function() {
            return Err(JsError::Type("decode: no callback".into()));
        }
        let id = NEXT.with(|n| {
            let id = n.get();
            n.set(id.wrapping_add(1).max(1));
            id
        });
        PENDING.with(|p| {
            p.borrow_mut().insert(id, Pending { realm: s.realm().downgrade(), done: Persistent::new(s, done) })
        });
        start(&page, id, bytes, rate);
        Ok(s.undefined())
    })?;
    natives.set_atom("decode", decode.value())?;
    let activated = s.function("activated", |s, _this, _a| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(page.has_sticky_activation()))
    })?;
    natives.set_atom("activated", activated.value())?;
    // A timer that does not keep the page busy (a real-time context's
    // sources end on it, as media playback timers do): (ms, fn) -> id.
    let timer = s.function("timer", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let ms = a.get(0).to_number()?;
        let delay = crate::util::delay_from_ms(ms);
        let callback = Persistent::new(s, a.get(1));
        let realm = s.realm().downgrade();
        let Some(hooks) = page.hooks() else { return Ok(s.number(-1.0)) };
        let id = hooks.set_media_timer(
            delay,
            Box::new(move || {
                let Some(realm) = realm.upgrade() else { return };
                let Ok(s) = realm.scope() else { return };
                if let Some(f) = callback.get(&s).as_object() {
                    if let Err(e) = f.call(None, &[]) {
                        s.report_exception(&e);
                    }
                }
            }),
        );
        Ok(s.number(id.map_or(-1.0, |id| id as f64)))
    })?;
    natives.set_atom("timer", timer.value())?;
    let clear = s.function("clearTimer", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()?;
        if id >= 0.0 {
            if let Some(hooks) = page.hooks() {
                hooks.clear_media_timer(id as u64);
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("clearTimer", clear.value())?;
    Ok(natives)
}

fn decode(bytes: &[u8], rate: u32) -> Decoded {
    let decoded = pyro_media::audio::decode(bytes)?;
    let channels = if decoded.sample_rate == rate {
        decoded.channels
    } else {
        decoded.channels.iter().map(|c| pyro_media::audio::resample(c, decoded.sample_rate, rate)).collect()
    };
    Ok((rate, channels))
}

/// Decodes on a thread of its own when the embedder can post back, else in
/// a task.
fn start(page: &Rc<PageDom>, id: u32, bytes: Vec<u8>, rate: u32) {
    if let Some(poster) = page.remote_poster() {
        let spawned = std::thread::Builder::new().name("pyro-audio-decode".into()).spawn(move || {
            let result = decode(&bytes, rate);
            poster(Box::new(move || finish(id, result)));
        });
        if spawned.is_ok() {
            return;
        }
        // Out of threads: the decode fails as a decode error would.
        page.post_task(Box::new(move || finish(id, Err("could not start decoding".into()))));
        return;
    }
    page.post_task(Box::new(move || finish(id, decode(&bytes, rate))));
}

/// Hands a decode's result to its JS callback: `(null, [rate, ...channels])`
/// or `(message)`.
fn finish(id: u32, result: Decoded) {
    let Some(pending) = PENDING.with(|p| p.borrow_mut().remove(&id)) else { return };
    let Some(realm) = pending.realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let run = || -> JsResult<()> {
        let Some(done) = pending.done.get(&s).as_object() else { return Ok(()) };
        match result {
            Ok((rate, channels)) => {
                let mut values = vec![s.number(f64::from(rate))];
                for channel in &channels {
                    values.push(float32_array(&s, channel)?);
                }
                let list = s.array(&values)?;
                done.call(None, &[s.null(), list.value()])?;
            }
            Err(message) => {
                done.call(None, &[s.string(&message)])?;
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        s.report_exception(&e);
    }
}

/// A new `Float32Array` holding `samples`.
fn float32_array<'s>(s: &Scope<'s>, samples: &[f32]) -> JsResult<Value<'s>> {
    let ctx = s.ctx();
    let mut exc = std::ptr::null();
    // SAFETY: JSObjectMakeTypedArray allocates a zeroed array of
    // `samples.len()` floats owned by JSC; they are copied in before control
    // returns to JS.
    unsafe {
        let obj = sys::JSObjectMakeTypedArray(ctx, sys::kJSTypedArrayTypeFloat32Array, samples.len(), &mut exc);
        if !exc.is_null() || obj.is_null() {
            return Err(JsError::Range("Array buffer allocation failed".into()));
        }
        if !samples.is_empty() {
            let ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj, &mut exc) as *mut f32;
            if !exc.is_null() {
                return Err(JsError::Range("Array buffer allocation failed".into()));
            }
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(samples.as_ptr(), ptr, samples.len());
            }
        }
        Ok(Value::from_raw(ctx, obj as sys::JSValueRef))
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if PENDING.with(|p| !p.borrow().is_empty()) {
        return Some("webaudio::PENDING");
    }
    NEXT.set(1);
    None
}
