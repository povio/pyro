//! Experimental Wasmi browser embedding. Nothing in this module calls JSC's Wasm API.
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{sync_channel, SyncSender};

use pyro_js::{JsError, JsResult, Object, Persistent, RootedVec, Scope, Value, WeakRealm};
use wasmi::{
    Config, Engine, Extern, ExternType, Func, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, Val, ValType,
};

struct CompiledModule {
    encoded_bytes: usize,
    module: Module,
    global_exports: HashMap<String, u32>,
}
type Compiled = Result<CompiledModule, String>;
fn compile(engine: &Engine, bytes: Vec<u8>) -> Compiled {
    let module = Module::new(engine, &bytes).map_err(|e| e.to_string())?;
    // The public Global handle has no identity comparison. Preserve the
    // decoder's global indices for JS re-exports and duplicate export names.
    let mut global_exports = HashMap::new();
    if module.exports().any(|e| matches!(e.ty(), ExternType::Global(_))) {
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            if let wasmparser::Payload::ExportSection(section) = payload.map_err(|e| e.to_string())? {
                for e in section {
                    let e = e.map_err(|e| e.to_string())?;
                    if e.kind == wasmparser::ExternalKind::Global {
                        global_exports.insert(e.name.to_owned(), e.index);
                    }
                }
                break;
            }
        }
    }
    Ok(CompiledModule { encoded_bytes: bytes.len(), module, global_exports })
}
type Job = Box<dyn FnOnce() + Send>;
#[derive(Default)]
struct Runtime {
    engine: OnceCell<Engine>,
    globals: RefCell<Weak<StoreOwner>>,
}
type Compiler = Rc<Runtime>;

// Each JavaScript agent has one Wasmi engine. Keep only a weak Rust entry
// here, and put the stable identity in pyro-js's existing VM-root cache;
// the cache never stores a page's global object or a JS wrapper.
thread_local! {
    static RUNTIMES: RefCell<HashMap<String, Weak<Runtime>>> = RefCell::new(HashMap::new());
}
fn runtime_for_agent(s: &Scope<'_>) -> JsResult<Compiler> {
    let fresh = ID.with(|id| {
        id.set(id.get() + 1);
        id.get()
    });
    let key = s.agent_value("experimentalWasmi.agent", &format!("'{fresh}'"))?.to_string()?;
    Ok(RUNTIMES.with(|all| {
        let mut all = all.borrow_mut();
        all.retain(|_, r| r.strong_count() != 0);
        if let Some(r) = all.get(&key).and_then(Weak::upgrade) {
            return r;
        }
        let runtime = Rc::new(Runtime::default());
        all.insert(key, Rc::downgrade(&runtime));
        runtime
    }))
}

fn engine(cell: &Compiler) -> &Engine {
    cell.engine.get_or_init(|| {
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
    runtime: Compiler,
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
    let result = module_result(&s, result, &pending.runtime).and_then(|value| done.call(None, &[value]));
    if let Err(e) = result {
        s.report_exception(&e);
    }
}

fn module_result<'s>(s: &Scope<'s>, result: Compiled, runtime: &Compiler) -> JsResult<Value<'s>> {
    match result {
        Ok(m) => pair(s, "ok", compiled(s, m, runtime)?.value()),
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
    if let ExternType::Global(g) = ty {
        o.set_atom("valueType", s.string(type_name(g.content()).unwrap_or("unsupported")))?;
        o.set_atom("mutable", s.boolean(g.mutability() == Mutability::Var))?;
    }
    o.set_atom("name", s.string(name))?;
    o.set_atom("kind", s.string(kind))?;
    Ok(o)
}

fn compiled<'s>(s: &Scope<'s>, compiled: CompiledModule, runtime: &Compiler) -> JsResult<Object<'s>> {
    let encoded_bytes = compiled.encoded_bytes;
    let m = compiled.module;
    let global_exports = compiled.global_exports;
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
        let desc = descriptor(s, item.name(), item.ty())?;
        if let Some(index) = global_exports.get(item.name()) {
            desc.set_atom("globalIndex", s.number(f64::from(*index)))?;
        }
        exports.set_index(i as u32, desc.value())?;
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
    let runtime = runtime.clone();
    o.set_atom(
        "start",
        s.function("start", move |s, _, a| start(s, &m, encoded_bytes, &runtime, &global_exports, a.get(0)))?.value(),
    )?;
    Ok(o)
}

use wasmi::{AsContextMut, Caller, Global, Mutability, StoreContextMut};

struct Entry {
    realm: WeakRealm,
    imports: Persistent,
    thrown: Option<JsError>,
}
struct Host {
    owner: u64,
    entries: Vec<Entry>,
    limits: StoreLimits,
}
// Wasmi releases objects at store granularity. A long-lived imported Global
// must not admit an unbounded series of transient instances or JS handles.
const MAX_SHARED_LINKS: usize = 256;
const MAX_SHARED_MODULE_BYTES: usize = 16 << 20;
const MAX_SHARED_GLOBAL_SLOTS: usize = 4096;

#[derive(Clone, Copy, Default)]
struct ArenaUse {
    links: usize,
    module_bytes: usize,
    global_slots: usize,
}

struct StoreOwner {
    used: Cell<ArenaUse>,
    id: u64,
    store: RefCell<Store<Host>>,
    globals: RefCell<Vec<Global>>,
}

impl StoreOwner {
    fn new(engine: &Engine, shared: bool) -> Rc<Self> {
        let id = ID.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        let mut limits = StoreLimitsBuilder::new().memory_size(128 << 20).table_elements(1 << 20);
        if shared {
            limits = limits.instances(MAX_SHARED_LINKS).memories(4).tables(8);
        }
        let mut store = Store::new(engine, Host { owner: id, entries: Vec::new(), limits: limits.build() });
        store.limiter(|h| &mut h.limits);
        Rc::new(Self {
            id,
            used: Cell::new(ArenaUse::default()),
            store: RefCell::new(store),
            globals: RefCell::new(Vec::new()),
        })
    }
    // Reserve before allocation or running the user module's start function.
    // Failed links also count: their host trampolines may already be in the arena.
    fn reserve(&self, links: usize, bytes: usize, slots: usize) -> JsResult<()> {
        let old = self.used.get();
        let next = ArenaUse {
            links: old.links.saturating_add(links),
            module_bytes: old.module_bytes.saturating_add(bytes),
            global_slots: old.global_slots.saturating_add(slots),
        };
        if next.links > MAX_SHARED_LINKS
            || next.module_bytes > MAX_SHARED_MODULE_BYTES
            || next.global_slots > MAX_SHARED_GLOBAL_SLOTS
        {
            return Err(JsError::Range("Experimental shared Wasm store budget exhausted".into()));
        }
        self.used.set(next);
        Ok(())
    }
    fn global_slot(&self, g: Global) -> usize {
        let mut list = self.globals.borrow_mut();
        let id = list.len();
        list.push(g);
        id
    }
    fn with<T>(&self, operation: impl FnOnce(StoreContextMut<'_, Host>) -> JsResult<T>) -> JsResult<T> {
        let mut op = Some(operation);
        let mut result = None;
        if ACTIVE.is_set() {
            ACTIVE.with(|active| {
                active.visit(self.id, &mut |ctx| result = Some(op.take().expect("one active store")(ctx)))
            });
        }
        if let Some(result) = result {
            return result;
        }
        let mut store =
            self.store.try_borrow_mut().map_err(|_| JsError::Type("Wasm store is already active".into()))?;
        op.expect("unused store operation")(store.as_context_mut())
    }
}

fn shared_store(runtime: &Compiler) -> Rc<StoreOwner> {
    if let Some(found) = runtime.globals.borrow().upgrade() {
        return found;
    }
    let store = StoreOwner::new(engine(runtime), true);
    *runtime.globals.borrow_mut() = Rc::downgrade(&store);
    store
}

// A host import runs with Wasmi's documented Caller. Nested JS-to-Wasm
// entries reborrow that Caller, including when a callback nested another
// store in between. The borrowed context never outlives this callback.
trait StoreRouter {
    fn visit(&self, id: u64, op: &mut dyn FnMut(StoreContextMut<'_, Host>));
}
scoped_tls_hkt::scoped_thread_local!(static ACTIVE: for<'a> &'a (dyn StoreRouter + 'a));
struct CallFrame<'a, 'b> {
    id: u64,
    caller: RefCell<Caller<'a, Host>>,
    prior: Option<&'b dyn StoreRouter>,
}
impl StoreRouter for CallFrame<'_, '_> {
    fn visit(&self, id: u64, op: &mut dyn FnMut(StoreContextMut<'_, Host>)) {
        if id == self.id {
            op(self.caller.borrow_mut().as_context_mut())
        } else if let Some(p) = self.prior {
            p.visit(id, op)
        }
    }
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

fn host_call(
    realm: &WeakRealm,
    callbacks: &Persistent,
    index: u32,
    params: &[Val],
    results: &mut [Val],
) -> JsResult<()> {
    let realm = realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let s = realm.scope()?;
    let imports = callbacks.get(&s).as_object().ok_or(JsError::ContextDestroyed)?;
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

fn dispatch(caller: Caller<'_, Host>, index: u32, params: &[Val], results: &mut [Val]) -> Result<(), wasmi::Error> {
    fn run(
        caller: Caller<'_, Host>,
        index: u32,
        params: &[Val],
        results: &mut [Val],
        prior: Option<&dyn StoreRouter>,
    ) -> Result<(), wasmi::Error> {
        let entry = caller.data().entries.last().ok_or_else(|| wasmi::Error::new("Missing JS entry"))?;
        let (realm, callbacks) = (entry.realm.clone(), entry.imports.clone());
        let frame = CallFrame { id: caller.data().owner, caller: RefCell::new(caller), prior };
        match ACTIVE.set(&frame, || host_call(&realm, &callbacks, index, params, results)) {
            Ok(()) => Ok(()),
            Err(e) => {
                frame.caller.borrow_mut().data_mut().entries.last_mut().expect("active JS entry").thrown = Some(e);
                Err(wasmi::Error::new("JavaScript import threw"))
            }
        }
    }
    if ACTIVE.is_set() {
        ACTIVE.with(|p| run(caller, index, params, results, Some(p)))
    } else {
        run(caller, index, params, results, None)
    }
}

const CALL_FUEL: u64 = 500_000_000;
fn enter<T>(
    mut ctx: StoreContextMut<'_, Host>,
    s: &Scope<'_>,
    imports: Value<'_>,
    f: impl FnOnce(&mut StoreContextMut<'_, Host>) -> Result<T, wasmi::Error>,
) -> JsResult<Result<T, wasmi::Error>> {
    // Nested calls on the same store spend the remaining outer budget.
    if ctx.data().entries.is_empty() {
        ctx.set_fuel(CALL_FUEL).expect("fuel enabled")
    }
    ctx.data_mut().entries.push(Entry {
        realm: s.realm().downgrade(),
        imports: Persistent::new(s, imports),
        thrown: None,
    });
    let result = f(&mut ctx);
    let frame = ctx.data_mut().entries.pop().expect("balanced JS entry");
    match frame.thrown {
        Some(e) => Err(e),
        None => Ok(result),
    }
}

fn global_type(name: &str) -> JsResult<ValType> {
    match name {
        "i32" => Ok(ValType::I32),
        "i64" => Ok(ValType::I64),
        "f32" => Ok(ValType::F32),
        "f64" => Ok(ValType::F64),
        _ => Err(JsError::Type("Unsupported Wasm Global value type".into())),
    }
}
fn type_name(ty: ValType) -> Option<&'static str> {
    match ty {
        ValType::I32 => Some("i32"),
        ValType::I64 => Some("i64"),
        ValType::F32 => Some("f32"),
        ValType::F64 => Some("f64"),
        _ => None,
    }
}
fn global_value(ty: ValType, value: Value<'_>) -> JsResult<Val> {
    if ty == ValType::I64 {
        let n = value
            .as_string()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| JsError::Type("Expected a converted i64".into()))?;
        Ok(Val::I64(n))
    } else {
        to_wasm(ty, value)
    }
}
fn global_handle<'s>(s: &Scope<'s>, owner: &Rc<StoreOwner>, g: Global, slot: usize) -> JsResult<Value<'s>> {
    let ty = owner.with(|c| Ok(g.ty(c)))?;
    let name = type_name(ty.content()).ok_or_else(|| JsError::Type("Unsupported exported global".into()))?;
    let o = s.object();
    o.set_atom("slot", s.number(slot as f64))?;
    o.set_atom("owner", s.number(owner.id as f64))?;
    o.set_atom("type", s.string(name))?;
    o.set_atom("mutable", s.boolean(ty.mutability() == Mutability::Var))?;
    let read = owner.clone();
    o.set_atom(
        "get",
        s.function("get", move |s, _, _| {
            let v = read.with(|c| Ok(g.get(c)))?;
            if let Val::I64(n) = v {
                Ok(s.string(&n.to_string()))
            } else {
                to_js(s, &v)
            }
        })?
        .value(),
    )?;
    let write = owner.clone();
    o.set_atom(
        "set",
        s.function("set", move |s, _, a| {
            let val = global_value(ty.content(), a.get(0))?;
            write.with(|c| g.set(c, val).map_err(|e| JsError::Type(e.to_string().into())))?;
            Ok(s.undefined())
        })?
        .value(),
    )?;
    Ok(o.value())
}
fn start<'s>(
    s: &Scope<'s>,
    module: &Module,
    encoded_bytes: usize,
    runtime: &Compiler,
    global_indices: &HashMap<String, u32>,
    imports: Value<'s>,
) -> JsResult<Value<'s>> {
    if module.exports().any(|e| !matches!(e.ty(), ExternType::Func(_) | ExternType::Global(_))) {
        return pair(s, "LinkError", s.string("Unsupported Wasm export kind"));
    }
    if module.exports().any(|e| matches!(e.ty(),ExternType::Global(g) if type_name(g.content()).is_none())) {
        return pair(s, "LinkError", s.string("Unsupported exported global value type"));
    }
    let global_imports = module.imports().filter(|i| matches!(i.ty(), ExternType::Global(_))).count();
    let global_link = !global_indices.is_empty() || global_imports != 0;
    let owner = if global_link { shared_store(runtime) } else { StoreOwner::new(module.engine(), false) };
    if global_link {
        owner.reserve(1, encoded_bytes, global_imports.saturating_add(global_indices.len()))?;
    }
    let array = imports.as_object().ok_or_else(|| JsError::Type("Missing imports".into()))?;
    let mut externals = Vec::new();
    let mut slots = HashMap::new();
    let mut next_global = 0;
    for (index, imp) in module.imports().enumerate() {
        let ext = match imp.ty() {
            ExternType::Func(ty) => {
                let f = owner.with(|mut ctx| {
                    Ok(Func::new(&mut ctx, ty.clone(), move |c, p, r| dispatch(c, index as u32, p, r)))
                })?;
                Extern::Func(f)
            }
            ExternType::Global(ty) => {
                let Some(name) = type_name(ty.content()) else {
                    return pair(s, "LinkError", s.string("Unsupported imported global type"));
                };
                let v = array.get_index(index as u32)?;
                let (g, slot) = if let Some(h) = v.as_object() {
                    if h.get_atom("owner")?.to_number()? != owner.id as f64 {
                        return pair(s, "LinkError", s.string("Global belongs to another store"));
                    }
                    let slot = h.get_atom("slot")?.to_number()? as usize;
                    let Some(g) = owner.globals.borrow().get(slot).copied() else {
                        return pair(s, "LinkError", s.string("Invalid global handle"));
                    };
                    (g, slot)
                } else {
                    if ty.mutability() == Mutability::Var {
                        return pair(s, "LinkError", s.string("A mutable import requires a Global"));
                    }
                    if (name == "i64" && !v.is_string()) || (name != "i64" && !v.is_number()) {
                        return pair(s, "LinkError", s.string("Incorrect global import type"));
                    }
                    let val = global_value(ty.content(), v)?;
                    let g = owner.with(|c| Ok(Global::new(c, val, Mutability::Const)))?;
                    (g, owner.global_slot(g))
                };
                slots.insert(next_global, slot);
                next_global += 1;
                Extern::Global(g)
            }
            _ => return pair(s, "LinkError", s.string("Unsupported Wasm import kind")),
        };
        externals.push(ext);
    }
    let made = owner.with(|ctx| enter(ctx, s, imports, |ctx| Instance::new(ctx, module, &externals)))?;
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
    let exports = s.object();
    exports.set_prototype(s.null());
    for e in module.exports() {
        let item = owner.with(|c| Ok(instance.get_export(c, e.name()).expect("declared export")))?;
        match item {
            Extern::Global(g) => {
                let idx = global_indices[e.name()];
                let slot = *slots.entry(idx).or_insert_with(|| owner.global_slot(g));
                exports.set(e.name(), global_handle(s, &owner, g, slot)?)?;
            }
            Extern::Func(f) => {
                let ty = owner.with(|c| Ok(f.ty(c)))?;
                let state = owner.clone();
                let call = s.function(e.name(), move |s, _, a| {
                    let args = a.get(1).as_object().ok_or_else(|| JsError::Type("Missing arguments".into()))?;
                    let params = ty
                        .params()
                        .iter()
                        .enumerate()
                        .map(|(i, &t)| to_wasm(t, args.get_index(i as u32)?))
                        .collect::<JsResult<Vec<_>>>()?;
                    let mut results = ty.results().iter().copied().map(Val::default_for_ty).collect::<Vec<_>>();
                    let r = state.with(|ctx| enter(ctx, s, a.get(0), |ctx| f.call(ctx, &params, &mut results)))?;
                    if let Err(e) = r {
                        return pair(s, "RuntimeError", s.string(&e.to_string()));
                    }
                    let v = match results.as_slice() {
                        [] => s.undefined(),
                        [v] => to_js(s, v)?,
                        _ => {
                            let out = s.array(&[])?;
                            for (i, v) in results.iter().enumerate() {
                                out.set_index(i as u32, to_js(s, v)?)?
                            }
                            out.value()
                        }
                    };
                    pair(s, "ok", v)
                })?;
                exports.set(e.name(), call.value())?;
            }
            _ => unreachable!("checked export kind"),
        }
    }
    pair(s, "ok", exports.value())
}

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let cell = runtime_for_agent(s)?;
    let n = s.object();
    // Per-agent ephemeron storage, shared by Window realms. Workers have a
    // different agent. Entries do not root their JS wrapper keys.
    n.set_atom("globalSlots", s.agent_value("experimentalWasmi.globalSlots", "new WeakMap()")?)?;
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
        s.function("compile", move |s, _, a| module_result(s, compile(engine(&e), bytes(s, a.get(0))?), &e))?.value(),
    )?;
    let r = cell.clone();
    n.set_atom(
        "newGlobal",
        s.function("newGlobal", move |s, _, a| {
            let ty = global_type(&a.get(0).to_string()?)?;
            let mutability = if a.get(1).to_bool() { Mutability::Var } else { Mutability::Const };
            let value = global_value(ty, a.get(2))?;
            let owner = shared_store(&r);
            owner.reserve(0, 0, 1)?;
            let g = owner.with(|c| Ok(Global::new(c, value, mutability)))?;
            let slot = owner.global_slot(g);
            global_handle(s, &owner, g, slot)
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
                    Pending {
                        runtime: cell.clone(),
                        page: page.id(),
                        realm: s.realm().downgrade(),
                        done: Persistent::new(s, a.get(1)),
                    },
                )
            });
            let work = Box::new(move || {
                let result = compile(&engine, bytes);
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
