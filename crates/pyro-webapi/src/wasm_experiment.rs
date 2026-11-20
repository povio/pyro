//! Experimental Wasmi browser embedding. Nothing in this module calls JSC's Wasm API.
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{sync_channel, SyncSender};

use pyro_js::{JsError, JsResult, Object, Persistent, RootedVec, Scope, Value, WeakRealm};
use wasmi::{
    Config, Engine, Extern, ExternType, Func, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, Val, ValType,
};

type Compiled = Result<Module, String>;
type Job = Box<dyn FnOnce() + Send>;
type Compiler = Rc<OnceCell<Engine>>;

fn engine(cell: &Compiler) -> &Engine {
    cell.get_or_init(|| {
        let mut config = Config::default();
        config.compilation_mode(wasmi::CompilationMode::Eager).consume_fuel(true);
        Engine::new(&config)
    })
}

fn bytes(s: &Scope<'_>, value: Value<'_>) -> JsResult<Vec<u8>> {
    crate::encoding::buffer_source_bytes(s, value)?.ok_or_else(|| JsError::Type("Expected a BufferSource".into()))
}

fn pair<'s>(s: &Scope<'s>, kind: &str, value: Value<'s>) -> JsResult<Value<'s>> {
    Ok(s.array(&[s.string(kind), value])?.value())
}

struct Pending {
    page: u64,
    realm: WeakRealm,
    done: Persistent,
}

thread_local! {
    static PENDING: RefCell<HashMap<u64, Pending>> = RefCell::new(HashMap::new());
    static ID: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn dispose(page: u64) {
    PENDING.with(|p| p.borrow_mut().retain(|_, job| job.page != page));
}

fn finish(id: u64, result: Compiled) {
    let Some(pending) = PENDING.with(|p| p.borrow_mut().remove(&id)) else { return };
    let Some(realm) = pending.realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let Some(done) = pending.done.get(&s).as_object() else { return };
    let result = module_result(&s, result).and_then(|value| done.call(None, &[value]));
    if let Err(e) = result {
        s.report_exception(&e);
    }
}

fn module_result<'s>(s: &Scope<'s>, result: Compiled) -> JsResult<Value<'s>> {
    match result {
        Ok(m) => pair(s, "ok", compiled(s, m)?.value()),
        Err(e) => pair(s, "CompileError", s.string(&e)),
    }
}

fn descriptor<'s>(s: &Scope<'s>, name: &str, ty: &ExternType) -> JsResult<Object<'s>> {
    let o = s.object();
    let kind = match ty {
        ExternType::Func(_) => "function",
        ExternType::Global(_) => "global",
        ExternType::Memory(_) => "memory",
        ExternType::Table(_) => "table",
    };
    o.set_atom("name", s.string(name))?;
    o.set_atom("kind", s.string(kind))?;
    Ok(o)
}

fn compiled<'s>(s: &Scope<'s>, m: Module) -> JsResult<Object<'s>> {
    let o = s.object();
    let imports = s.array(&[])?;
    o.set_atom("imports", imports.value())?;
    for (i, item) in m.imports().enumerate() {
        let desc = descriptor(s, item.name(), item.ty())?;
        desc.set_atom("module", s.string(item.module()))?;
        imports.set_index(i as u32, desc.value())?;
    }
    let exports = s.array(&[])?;
    o.set_atom("exports", exports.value())?;
    for (i, item) in m.exports().enumerate() {
        exports.set_index(i as u32, descriptor(s, item.name(), item.ty())?.value())?;
    }
    let sections = m.clone();
    o.set_atom(
        "custom",
        s.function("custom", move |s, _, a| {
            let name = a.get(0).to_string()?;
            let found = s.array(&[])?;
            for (i, item) in sections.custom_sections().filter(|x| x.name() == name).enumerate() {
                found.set_index(i as u32, crate::encoding::array_buffer(s, item.data())?)?;
            }
            Ok(found.value())
        })?
        .value(),
    )?;
    o.set_atom("start", s.function("start", move |s, _, a| start(s, &m, a.get(0)))?.value())?;
    Ok(o)
}

struct Host {
    realm: WeakRealm,
    // Roots exist only during an active native entry. JS wrappers own the imports between calls.
    imports: Option<Persistent>,
    thrown: Option<JsError>,
    limits: StoreLimits,
}

fn to_js<'s>(s: &Scope<'s>, value: &Val) -> JsResult<Value<'s>> {
    Ok(match value {
        Val::I32(x) => s.number(f64::from(*x)),
        Val::F32(x) => s.number(f64::from(x.to_float())),
        Val::F64(x) => s.number(x.to_float()),
        _ => return Err(JsError::Type("Unsupported Wasm boundary value".into())),
    })
}

fn to_wasm(ty: ValType, value: Value<'_>) -> JsResult<Val> {
    Ok(match ty {
        ValType::I32 => Val::I32(crate::util::to_i32(value)?),
        ValType::F32 => Val::F32((value.to_number()? as f32).into()),
        ValType::F64 => Val::F64(value.to_number()?.into()),
        _ => return Err(JsError::Type("Unsupported Wasm boundary value".into())),
    })
}

fn host_call(host: &Host, index: u32, params: &[Val], results: &mut [Val]) -> JsResult<()> {
    let realm = host.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let s = realm.scope()?;
    let imports = host.imports.as_ref().and_then(|v| v.get(&s).as_object()).ok_or(JsError::ContextDestroyed)?;
    let f = imports.get_index(index)?.as_object().ok_or_else(|| JsError::Type("Expected a function".into()))?;
    let mut args = RootedVec::new();
    for param in params {
        args.push(&s, to_js(&s, param)?);
    }
    let result = f.call_rooted(None, &args)?;
    match results {
        [] => Ok(()),
        [out] => {
            *out = to_wasm(out.ty(), result)?;
            Ok(())
        }
        _ => Err(JsError::Type("Multi-value host imports are unsupported".into())),
    }
}

const CALL_FUEL: u64 = 500_000_000;

fn start<'s>(s: &Scope<'s>, module: &Module, imports: Value<'s>) -> JsResult<Value<'s>> {
    if module.exports().any(|e| !matches!(e.ty(), ExternType::Func(_))) {
        return pair(s, "LinkError", s.string("This experimental embedding exposes only function exports"));
    }
    let mut store = Store::new(
        module.engine(),
        Host {
            realm: s.realm().downgrade(),
            imports: Some(Persistent::new(s, imports)),
            thrown: None,
            limits: StoreLimitsBuilder::new().memory_size(128 << 20).table_elements(1 << 20).build(),
        },
    );
    store.limiter(|h| &mut h.limits);
    store.set_fuel(CALL_FUEL).expect("fuel enabled");
    let mut externals = Vec::new();
    for (index, import) in module.imports().enumerate() {
        let ExternType::Func(ty) = import.ty() else {
            return pair(s, "LinkError", s.string("This experimental embedding supports only function imports"));
        };
        let f = Func::new(&mut store, ty.clone(), move |mut caller, params, results| {
            if let Err(e) = host_call(caller.data(), index as u32, params, results) {
                caller.data_mut().thrown = Some(e);
                return Err(wasmi::Error::new("JavaScript import threw"));
            }
            Ok(())
        });
        externals.push(Extern::Func(f));
    }
    let made = Instance::new(&mut store, module, &externals);
    store.data_mut().imports = None;
    if let Some(e) = store.data_mut().thrown.take() {
        return Err(e);
    }
    let instance = match made {
        Ok(i) => i,
        Err(e) => {
            return pair(
                s,
                if e.as_trap_code().is_some() { "RuntimeError" } else { "LinkError" },
                s.string(&e.to_string()),
            )
        }
    };
    let store = Rc::new(RefCell::new(store));
    let exports = s.object();
    exports.set_prototype(s.null());
    for e in module.exports() {
        let f = instance.get_func(&*store.borrow(), e.name()).expect("checked function export");
        let ty = f.ty(&*store.borrow());
        let state = store.clone();
        let call = s.function(e.name(), move |s, _, a| {
            let args = a.get(1).as_object().ok_or_else(|| JsError::Type("Missing arguments".into()))?;
            let params = ty
                .params()
                .iter()
                .enumerate()
                .map(|(i, &t)| to_wasm(t, args.get_index(i as u32)?))
                .collect::<JsResult<Vec<_>>>()?;
            let mut results = ty.results().iter().copied().map(Val::default_for_ty).collect::<Vec<_>>();
            let mut store =
                state.try_borrow_mut().map_err(|_| JsError::Type("Same-instance reentry is unsupported".into()))?;
            store.data_mut().imports = Some(Persistent::new(s, a.get(0)));
            store.set_fuel(CALL_FUEL).expect("fuel enabled");
            let r = f.call(&mut *store, &params, &mut results);
            store.data_mut().imports = None;
            if let Some(e) = store.data_mut().thrown.take() {
                return Err(e);
            }
            if let Err(e) = r {
                return pair(s, "RuntimeError", s.string(&e.to_string()));
            }
            let v = match results.as_slice() {
                [] => s.undefined(),
                [v] => to_js(s, v)?,
                _ => {
                    let out = s.array(&[])?;
                    for (i, result) in results.iter().enumerate() {
                        out.set_index(i as u32, to_js(s, result)?)?;
                    }
                    out.value()
                }
            };
            pair(s, "ok", v)
        })?;
        exports.set(e.name(), call.value())?;
    }
    pair(s, "ok", exports.value())
}

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let cell = Rc::new(OnceCell::new());
    let n = s.object();
    let e = cell.clone();
    n.set_atom(
        "validate",
        s.function("validate", move |s, _, a| {
            Ok(s.boolean(Module::validate(engine(&e), &bytes(s, a.get(0))?).is_ok()))
        })?
        .value(),
    )?;
    let e = cell.clone();
    n.set_atom(
        "compile",
        s.function("compile", move |s, _, a| {
            module_result(s, Module::new(engine(&e), bytes(s, a.get(0))?).map_err(|e| e.to_string()))
        })?
        .value(),
    )?;
    let compiler: RefCell<Option<SyncSender<Job>>> = RefCell::new(None);
    n.set_atom(
        "background",
        s.function("background", move |s, _, a| {
            let page = crate::target::page_of(s)?;
            let poster = page.thread_poster().ok_or_else(|| JsError::Type("No owner event loop".into()))?;
            let bytes = bytes(s, a.get(0))?;
            let engine = engine(&cell).clone();
            let mut queue = compiler.borrow_mut();
            if queue.is_none() {
                let (tx, rx) = sync_channel::<Job>(128);
                std::thread::Builder::new()
                    .name("wasmi-compile".into())
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            job()
                        }
                    })
                    .map_err(|e| JsError::Type(e.to_string().into()))?;
                *queue = Some(tx);
            }
            let id = ID.with(|n| {
                n.set(n.get() + 1);
                n.get()
            });
            PENDING.with(|p| {
                p.borrow_mut().insert(
                    id,
                    Pending { page: page.id(), realm: s.realm().downgrade(), done: Persistent::new(s, a.get(1)) },
                )
            });
            let work = Box::new(move || {
                let result = Module::new(&engine, bytes).map_err(|e| e.to_string());
                poster(Box::new(move || finish(id, result)));
            });
            if queue.as_ref().expect("compiler initialized").try_send(work).is_err() {
                PENDING.with(|p| p.borrow_mut().remove(&id));
                return Err(JsError::Range("Compiler queue is full".into()));
            }
            Ok(s.undefined())
        })?
        .value(),
    )?;
    n.set_atom(
        "task",
        s.function("task", |s, _, a| {
            let realm = s.realm().downgrade();
            let cb = Persistent::new(s, a.get(0));
            crate::target::page_of(s)?.post_task(Box::new(move || {
                let Some(r) = realm.upgrade() else { return };
                let Ok(s) = r.scope() else { return };
                if let Some(f) = cb.get(&s).as_object() {
                    if let Err(e) = f.call(None, &[]) {
                        s.report_exception(&e)
                    }
                }
            }));
            Ok(s.undefined())
        })?
        .value(),
    )?;
    s.eval(include_str!("wasm_experiment.js"), Some("pyro://experiment/wasmi"), 1)?
        .as_object()
        .expect("binding factory")
        .call(None, &[n.value()])?;
    Ok(())
}
