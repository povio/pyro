//! Web Audio's `BaseAudioContext.audioWorklet`: an `AudioWorklet` (the `Worklet` of
//! paint_worklet.js, given the natives made here) whose modules run in that context's own
//! AudioWorkletGlobalScope, a realm made on the first `addModule()` (no window, no DOM: the JS
//! built-ins, `registerProcessor`, `AudioWorkletProcessor`, `sampleRate`, `currentFrame` and
//! `currentTime`). The module request is an "audioworklet" request (CSP's `script-src-elem`, not
//! `worker-src`); its imports are fetched as the paint worklet's are. `registerProcessor` validates
//! and records a processor definition; pyro has no `AudioWorkletNode`, so none is instantiated.
//!
//! As in Chrome, `addModule()` rejects when the module or one of its imports fails to fetch, parse
//! or link, and fulfills once evaluation ends, whether or not the module threw (HTML's "fetch a
//! worklet script graph" reports, not rethrows, an evaluation error).
//! `[SecureContext]` (`crate::secure_context`).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyro_js::{JsResult, Object, Persistent, RealmOptions, Scope, Value};

use crate::page::PageDom;
use crate::paint_worklet::{is_link_error, loaded_or_threw, run_module, Settle};

/// One BaseAudioContext's AudioWorkletGlobalScope: owned by the page (`PageDom::audio_worklets`,
/// dropped with it); the worklet's natives hold it weakly.
pub(crate) struct AudioWorkletGlobal {
    realm: RefCell<Option<Rc<pyro_js::Realm>>>,
    /// The global's module map (every module of its graphs: entries and imports).
    modules: pyro_js::WorkletModuleMap,
    /// The context's sample rate (the global's `sampleRate`).
    rate: f64,
    /// The context's render quantum size (the global's `renderQuantumSize`).
    quantum: f64,
    /// The host object audio_worklet_global.js returned (a value of the worklet realm), once made.
    host: RefCell<Option<Persistent>>,
}

/// The global's own members (`registerProcessor`, `AudioWorkletProcessor`, `MessagePort`, the timing
/// attributes) and the host object that constructs and runs processors.
const PRELUDE: &str = include_str!("audio_worklet_global.js");

/// A new `AudioWorklet` for a BaseAudioContext of sample rate `rate` and render quantum size `quantum`
/// (webaudio.js's `audioWorklet` getter, once per context): an array of the `AudioWorklet` and a
/// function returning the global's host object (undefined before the first `addModule()` made the
/// realm), or undefined outside a secure context.
pub(crate) fn make<'s>(s: &Scope<'s>, page: &Rc<PageDom>, rate: f64, quantum: f64) -> JsResult<Value<'s>> {
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    let global = Rc::new(AudioWorkletGlobal {
        realm: RefCell::new(None),
        modules: Default::default(),
        rate,
        quantum,
        host: RefCell::new(None),
    });
    page.audio_worklets.borrow_mut().push(global.clone());
    let backend = natives(s, Rc::downgrade(&global))?;
    let factory = crate::paint_worklet::MODULE.module(s, page)?.get("audioWorklet")?;
    let factory = factory.as_object().expect("paint_worklet.js audioWorklet");
    let worklet = factory.call(None, &[backend.value()])?;
    let weak = Rc::downgrade(&global);
    let host = s.function("workletHost", move |s, _, _| {
        let Some(global) = weak.upgrade() else { return Ok(s.undefined()) };
        let host = global.host.borrow();
        Ok(match &*host {
            Some(host) => host.get(s),
            None => s.undefined(),
        })
    })?;
    Ok(s.array(&[worklet, host.value()])?.value())
}

/// Drops the page's AudioWorkletGlobalScopes (at the page's teardown).
pub(crate) fn dispose(page: &PageDom) {
    let globals = page.audio_worklets.take();
    for global in &globals {
        drop(global.host.take());
        drop(global.realm.take());
    }
}

/// The worklet's natives (paint_worklet.js's list) for `global`.
fn natives<'s>(s: &Scope<'s>, global: Weak<AudioWorkletGlobal>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    let g = global.clone();
    let evaluate_fn = s.function("evaluate", move |s, _, a| {
        let source = a.get(0).to_string()?;
        let url = a.get(1).to_string()?;
        let credentials = a.get(2).to_string()?;
        let graph = a.get(3).to_number()? as u64;
        let Some(global) = g.upgrade() else { return Err(gone()) };
        evaluate(s, &global, &source, &url, &credentials, graph)
    })?;
    native.set_atom("evaluate", evaluate_fn.value())?;
    let g = global.clone();
    let state = s.function("moduleState", move |s, _, a| {
        let url = a.get(0).to_string()?;
        let state = match g.upgrade().and_then(|g| g.modules.borrow().get(&url).map(|m| m.fetched)) {
            Some(true) => "fetched",
            Some(false) => "fetching",
            None => "none",
        };
        Ok(s.string(state))
    })?;
    native.set_atom("moduleState", state.value())?;
    let g = global.clone();
    let module_graph = s.function("moduleGraph", move |s, _, a| {
        let url = a.get(0).to_string()?;
        let graph = g.upgrade().and_then(|g| g.modules.borrow().get(&url).map(|m| m.graph as f64));
        Ok(s.number(graph.unwrap_or(-1.0)))
    })?;
    native.set_atom("moduleGraph", module_graph.value())?;
    let g = global;
    let await_graph_fn = s.function("awaitGraph", move |s, _, a| {
        let url = a.get(0).to_string()?;
        let Some(global) = g.upgrade() else { return Ok(s.null()) };
        await_graph(s, &global, &url)
    })?;
    native.set_atom("awaitGraph", await_graph_fn.value())?;
    // fetch(url, init) -> fetch() of the worklet's module: an "audioworklet" request (CSP's
    // script-src-elem, not worker-src or connect-src).
    let module_fetch = crate::fetch::internal_fetch(s, pyro_net::csp::RequestDest::AudioWorklet)?;
    native.set_atom("fetch", module_fetch.value())?;
    Ok(native)
}

fn gone() -> pyro_js::JsError {
    pyro_js::JsError::Type("The AudioWorklet's document is gone.".into())
}

/// Evaluates `source` (from `url`) as a module in `global`, made on first use, its imports fetched
/// with the graph's `credentials` mode and recorded as graph `graph`'s. Returns a promise of the
/// calling realm: rejected with the error of a module (or import) that does not fetch, parse or
/// link; else fulfilled once evaluation ends, also when the module threw (Chrome; the error is the
/// worklet global's to report).
fn evaluate<'s>(
    s: &Scope<'s>,
    global: &AudioWorkletGlobal,
    source: &str,
    url: &str,
    credentials: &str,
    graph: u64,
) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let realm = {
        let mut slot = global.realm.borrow_mut();
        if slot.is_none() {
            let realm = pyro_js::Engine::create_realm_in_current_thread(RealmOptions {
                name: "audio-worklet".into(),
                global_class: None,
                origin: crate::storage::tuple_origin(s, &page),
            })?;
            {
                let ws = realm.scope()?;
                let install = ws.eval(PRELUDE, Some("pyro://internal/audio-worklet-global.js"), 1)?;
                let install = install.as_object().expect("audio worklet prelude");
                // An exception of a message listener or the like in the global is reported there (the
                // console, CDP), never to the document's window.
                let report = ws.function("reportException", |s, _, a| {
                    s.report_exception(&pyro_js::JsError::thrown(Persistent::new(s, a.get(0))));
                    Ok(s.undefined())
                })?;
                let host = install.call(
                    None,
                    &[ws.global().value(), ws.number(global.rate), ws.number(global.quantum), report.value()],
                )?;
                *global.host.borrow_mut() = Some(Persistent::new(&ws, host));
            }
            *slot = Some(Rc::new(realm));
        }
        slot.as_ref().expect("worklet realm").clone()
    };
    // "Fetch a worklet script graph" with the document's fetch client, in the global's own module map.
    realm.set_module_fetcher(
        s.realm().module_fetcher().and_then(|f| f.for_worklet_graph(credentials, global.modules.clone(), graph)),
    );
    let ws = realm.scope()?;
    let settle = Settle::new(s, deferred);
    match ws.load_module_graph(source, url) {
        // Loaded (every import fetched, the graph parsed; or it reached a module that threw before):
        // link and run it as the paint worklet does (paint_worklet.rs `run_module`).
        Ok(Some(loading)) => {
            let loaded = loaded_or_threw(&ws, loading)?;
            let url = url.to_owned();
            settle.when(s, &ws, loaded, move |ws, _| Some(run_module(ws, &url)))?
        }
        // An engine without module graph phases: the evaluation promise also carries an import's
        // fetch failure, which this cannot tell from an evaluation error, so both fulfill (Chrome
        // fulfills on an evaluation error).
        Ok(None) => match ws.eval_module(source, url) {
            Ok(evaluation) => {
                let after = ended(&ws, evaluation)?;
                settle.when(s, &ws, after, |_, _| None)?
            }
            // Thrown synchronously by the module's evaluation: reported, and it fulfills.
            Err(pyro_js::JsError::Thrown(e)) if !is_link_error(&ws, e.get(&ws)) => {
                ws.note_module_evaluation_error(e.get(&ws));
                ws.report_exception(&pyro_js::JsError::Thrown(e));
                settle.now(s, Ok(()))?
            }
            // A module that does not parse or link (or a failed import): the error, in the caller's realm.
            Err(e) => settle.now(s, Err(e.to_value(s)))?,
        },
        Err(e) => settle.now(s, Err(e.to_value(s)))?,
    }
    Ok(promise)
}

/// A promise of the worklet realm that fulfills once `evaluation` (an evaluation promise, or
/// undefined) settles, either way.
fn ended<'w>(ws: &Scope<'w>, evaluation: Value<'w>) -> JsResult<Value<'w>> {
    let Some(p) = evaluation.as_object() else { return Ok(ws.undefined()) };
    let Some(then) = p.get_atom("then")?.as_object() else { return Ok(ws.undefined()) };
    let done = ws.function("ended", |ws, _, _| Ok(ws.undefined()))?;
    then.call(Some(p), &[done.value(), done.value()])
}

/// The graph of `url`, a module of the global's module map (paint_worklet.rs `await_graph`).
fn await_graph<'s>(s: &Scope<'s>, global: &AudioWorkletGlobal, url: &str) -> JsResult<Value<'s>> {
    let Some(realm) = global.realm.borrow().clone() else { return Ok(s.null()) };
    let ws = realm.scope()?;
    let Some(loading) = ws.load_registered_module_graph(url)? else { return Ok(s.null()) };
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let settle = Settle::new(s, deferred);
    // Loaded: link and run it (a no-op for a module that already ran), as an entry is.
    let loaded = loaded_or_threw(&ws, loading)?;
    let url = url.to_owned();
    settle.when(s, &ws, loaded, move |ws, _| Some(run_module(ws, &url)))?;
    Ok(promise)
}

/// `AudioWorklet` on a window global (absent outside a secure context).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    def.global_value_accessor(
        "AudioWorklet",
        |s, w| {
            let page = crate::window::WindowHost::page(w);
            if !crate::secure_context::secure(s, page) {
                return Ok(s.undefined());
            }
            crate::paint_worklet::MODULE.global(s, page, "AudioWorklet")
        },
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "AudioWorklet", value),
    );
}
